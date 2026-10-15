use crate::fs_operation;
use libuv::HandleTrait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use v8::{Global, HandleScope, Local, PromiseResolver};

const ASYNC_IO_SLOT: i32 = 1;

struct Completion {
    id: u64,
    result: Value,
}

pub struct AsyncIo {
    event_loop: libuv::Loop,
    wait_timer: libuv::TimerHandle,
    sender: mpsc::Sender<Completion>,
    receiver: mpsc::Receiver<Completion>,
    pending: HashMap<u64, Global<PromiseResolver>>,
    next_id: u64,
}

impl AsyncIo {
    pub fn new() -> Result<Self, String> {
        let event_loop =
            libuv::Loop::new().map_err(|error| format!("could not create libuv loop: {error}"))?;
        let wait_timer = event_loop
            .timer()
            .map_err(|error| format!("could not create libuv wait timer: {error}"))?;
        let (sender, receiver) = mpsc::channel();
        Ok(Self {
            event_loop,
            wait_timer,
            sender,
            receiver,
            pending: HashMap::new(),
            next_id: 1,
        })
    }

    pub fn attach(&mut self, context: Local<v8::Context>) {
        unsafe {
            context
                .set_aligned_pointer_in_embedder_data(ASYNC_IO_SLOT, (self as *mut AsyncIo).cast());
        }
    }

    pub fn detach(&mut self, context: Local<v8::Context>) {
        unsafe {
            context.set_aligned_pointer_in_embedder_data(ASYNC_IO_SLOT, std::ptr::null_mut());
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn submit(
        &mut self,
        scope: &mut HandleScope,
        resolver: Local<PromiseResolver>,
        operation: String,
        first: String,
        second: String,
    ) -> Result<(), String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let sender = self.sender.clone();
        let result = Arc::new(Mutex::new(Err(
            "libuv filesystem worker did not return a result".to_owned(),
        )));
        let worker_result = Arc::clone(&result);
        self.pending.insert(id, Global::new(scope, resolver));
        let request = self.event_loop.queue_work(
            move |_request| {
                if let Ok(mut result) = worker_result.lock() {
                    *result = Ok(fs_operation(&operation, &first, &second));
                }
            },
            move |_request, status| {
                let result = result
                    .lock()
                    .map(|result| match &*result {
                        Ok(value) => value.clone(),
                        Err(message) => json!({
                            "error": {
                                "code": "ERR_RUNTIME",
                                "message": message,
                            }
                        }),
                    })
                    .unwrap_or_else(|_| {
                        json!({
                            "error": {
                                "code": "ERR_RUNTIME",
                                "message": "libuv filesystem worker result was poisoned",
                            }
                        })
                    });
                let result = match status {
                    Ok(_) => result,
                    Err(error) => json!({
                        "error": {
                            "code": "ERR_RUNTIME",
                            "message": format!("libuv filesystem work failed: {error}"),
                        }
                    }),
                };
                if sender.send(Completion { id, result }).is_err() {
                    eprintln!("node: async I/O completion receiver disconnected");
                }
            },
        );
        if let Err(error) = request {
            self.pending.remove(&id);
            return Err(format!("could not queue libuv filesystem work: {error}"));
        }
        Ok(())
    }

    fn pump(&mut self, scope: &mut HandleScope, timeout_ms: u64) -> Result<usize, String> {
        if timeout_ms == 0 && !self.event_loop.is_alive() {
            return Ok(0);
        }

        if timeout_ms > 0 {
            self.wait_timer
                .start(timeout_ms, 0, |_| {})
                .map_err(|error| format!("could not start libuv wait timer: {error}"))?;
        }

        self.event_loop
            .run(if timeout_ms == 0 {
                libuv::RunMode::NoWait
            } else {
                libuv::RunMode::Once
            })
            .map_err(|error| format!("libuv event loop failed: {error}"))?;

        if self.wait_timer.is_active() {
            self.wait_timer
                .stop()
                .map_err(|error| format!("could not stop libuv wait timer: {error}"))?;
        }

        let mut completed = 0;
        for completion in self.receiver.try_iter() {
            let Some(resolver) = self.pending.remove(&completion.id) else {
                continue;
            };
            let encoded = serde_json::to_string(&completion.result)
                .map_err(|error| format!("could not encode async I/O result: {error}"))?;
            let Some(value) = v8::String::new(scope, &encoded) else {
                return Err("could not allocate async I/O result".to_owned());
            };
            let resolver = Local::new(scope, &resolver);
            if resolver.resolve(scope, value.into()) != Some(true) {
                return Err("could not resolve async I/O promise".to_owned());
            }
            completed += 1;
        }
        Ok(completed)
    }
}

impl Drop for AsyncIo {
    fn drop(&mut self) {
        if !self.wait_timer.is_closing() {
            self.wait_timer.close(());
        }
        if let Err(error) = self.event_loop.run(libuv::RunMode::Default) {
            eprintln!("node: could not close libuv event loop cleanly: {error}");
        }
    }
}

pub fn submit_fs(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let Some(resolver) = PromiseResolver::new(scope) else {
        return;
    };
    let promise = resolver.get_promise(scope);
    let operation = args.get(0).to_rust_string_lossy(scope);
    let first = args.get(1).to_rust_string_lossy(scope);
    let second = args.get(2).to_rust_string_lossy(scope);
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        reject(scope, resolver, "async I/O is unavailable in this context");
        return_value.set(promise.into());
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    if let Err(error) = state.submit(scope, resolver, operation, first, second) {
        reject(scope, resolver, &error);
    }
    return_value.set(promise.into());
}

pub fn pump(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let wait_ms = args.get(0).integer_value(scope).unwrap_or(0).clamp(0, 250) as u64;
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return_value.set(v8::Integer::new(scope, 0).into());
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    match state.pump(scope, wait_ms) {
        Ok(completed) => return_value.set(v8::Integer::new(scope, completed as i32).into()),
        Err(error) => {
            let Some(message) = v8::String::new(scope, &error) else {
                return;
            };
            let exception = v8::Exception::error(scope, message);
            scope.throw_exception(exception);
        }
    }
}

pub fn has_pending(
    scope: &mut HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    let pending = !pointer.is_null() && unsafe { &*pointer.cast::<AsyncIo>() }.has_pending();
    return_value.set(v8::Boolean::new(scope, pending).into());
}

fn reject(scope: &mut HandleScope, resolver: Local<PromiseResolver>, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::error(scope, message);
        let _ = resolver.reject(scope, error);
    }
}
