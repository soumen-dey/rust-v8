use crate::fs_operation;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use tokio::runtime::{Builder, Handle, Runtime};
use v8::{Global, HandleScope, Local, PromiseResolver};

const ASYNC_IO_SLOT: i32 = 1;

struct Completion {
    id: u64,
    result: Value,
}

pub struct AsyncIo {
    _runtime: Runtime,
    handle: Handle,
    sender: Sender<Completion>,
    receiver: Receiver<Completion>,
    pending: HashMap<u64, Global<PromiseResolver>>,
    next_id: u64,
}

impl AsyncIo {
    pub fn new() -> Result<Self, String> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("runtime-io")
            .enable_all()
            .build()
            .map_err(|error| format!("could not start async I/O workers: {error}"))?;
        let handle = runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        Ok(Self {
            _runtime: runtime,
            handle,
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
    ) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.pending.insert(id, Global::new(scope, resolver));
        let sender = self.sender.clone();
        self.handle.spawn_blocking(move || {
            let result = fs_operation(&operation, &first, &second);
            let _ = sender.send(Completion { id, result });
        });
    }

    fn pump(&mut self, scope: &mut HandleScope, timeout: Duration) -> Result<usize, String> {
        if self.pending.is_empty() {
            return Ok(0);
        }

        let first = match self.receiver.recv_timeout(timeout) {
            Ok(completion) => Some(completion),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("async I/O completion channel disconnected".to_owned())
            }
        };
        let mut completions = first.into_iter().collect::<Vec<_>>();
        completions.extend(self.receiver.try_iter());
        let mut completed = 0;
        for completion in completions {
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
    state.submit(scope, resolver, operation, first, second);
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
    match state.pump(scope, Duration::from_millis(wait_ms)) {
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
