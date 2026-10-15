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

enum LoopEvent {
    Completion(Completion),
    Timer(u64),
    Signal(String),
}

pub struct AsyncIo {
    event_loop: libuv::Loop,
    wait_timer: libuv::TimerHandle,
    sender: mpsc::Sender<LoopEvent>,
    receiver: mpsc::Receiver<LoopEvent>,
    pending: HashMap<u64, Global<PromiseResolver>>,
    next_id: u64,
    timers: HashMap<u64, libuv::TimerHandle>,
    signals: HashMap<String, libuv::SignalHandle>,
    exit_code: Option<i32>,
    pending_rejections: Vec<(Global<v8::Promise>, Global<v8::Value>)>,
    pending_rejection_handled: Vec<Global<v8::Promise>>,
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
            timers: HashMap::new(),
            signals: HashMap::new(),
            exit_code: None,
            pending_rejections: Vec::new(),
            pending_rejection_handled: Vec::new(),
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
        !self.pending.is_empty() || !self.timers.is_empty() || !self.signals.is_empty()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    fn track_promise_rejection(
        &mut self,
        scope: &mut HandleScope,
        event: v8::PromiseRejectEvent,
        promise: Local<v8::Promise>,
        reason: Option<Local<v8::Value>>,
    ) {
        match event {
            v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
                if let Some(reason) = reason {
                    self.pending_rejections
                        .push((Global::new(scope, promise), Global::new(scope, reason)));
                }
            }
            v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
                let mut was_pending = false;
                self.pending_rejections.retain(|(pending, _)| {
                    let matches = Local::new(scope, pending).strict_equals(promise.into());
                    was_pending |= matches;
                    !matches
                });
                if !was_pending {
                    self.pending_rejection_handled
                        .push(Global::new(scope, promise));
                }
            }
            _ => {}
        }
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
                if sender
                    .send(LoopEvent::Completion(Completion { id, result }))
                    .is_err()
                {
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

    fn start_timer(&mut self, id: u64, timeout: u64, repeat: u64) -> Result<(), String> {
        self.cancel_timer(id)?;
        let mut timer = self
            .event_loop
            .timer()
            .map_err(|error| format!("could not create libuv timer: {error}"))?;
        let sender = self.sender.clone();
        if let Err(error) = timer.start(timeout, repeat, move |_| {
            if sender.send(LoopEvent::Timer(id)).is_err() {
                eprintln!("node: timer event receiver disconnected");
            }
        }) {
            timer.close(());
            return Err(format!("could not start libuv timer: {error}"));
        }
        self.timers.insert(id, timer);
        Ok(())
    }

    fn cancel_timer(&mut self, id: u64) -> Result<(), String> {
        if let Some(mut timer) = self.timers.remove(&id) {
            let stop_result = timer.stop();
            timer.close(());
            stop_result.map_err(|error| format!("could not stop libuv timer: {error}"))?;
        }
        Ok(())
    }

    fn watch_signal(&mut self, name: String, signal: i32, watch: bool) -> Result<(), String> {
        if watch {
            if self.signals.contains_key(&name) {
                return Ok(());
            }
            let mut handle = self
                .event_loop
                .signal()
                .map_err(|error| format!("could not create libuv signal handle: {error}"))?;
            let sender = self.sender.clone();
            let event_name = name.clone();
            if let Err(error) = handle.start(
                move |_handle, _signal| {
                    if sender.send(LoopEvent::Signal(event_name.clone())).is_err() {
                        eprintln!("node: signal event receiver disconnected");
                    }
                },
                signal,
            ) {
                handle.close(());
                return Err(format!("could not watch {name}: {error}"));
            }
            self.signals.insert(name, handle);
        } else if let Some(mut handle) = self.signals.remove(&name) {
            let stop_result = handle.stop();
            handle.close(());
            stop_result.map_err(|error| format!("could not stop watching {name}: {error}"))?;
        }
        Ok(())
    }

    fn pump(&mut self, scope: &mut HandleScope, timeout_ms: u64) -> Result<Vec<Value>, String> {
        if timeout_ms == 0 && !self.event_loop.is_alive() {
            return Ok(Vec::new());
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

        let mut events = Vec::new();
        let received = self.receiver.try_iter().collect::<Vec<_>>();
        for event in received {
            match event {
                LoopEvent::Completion(completion) => {
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
                }
                LoopEvent::Timer(id) => {
                    if self
                        .timers
                        .get(&id)
                        .is_some_and(|timer| timer.get_repeat() == 0)
                    {
                        self.cancel_timer(id)?;
                    }
                    events.push(json!({"timer": id}));
                }
                LoopEvent::Signal(name) => {
                    events.push(json!({"signal": name}));
                }
            }
        }
        Ok(events)
    }
}

impl Drop for AsyncIo {
    fn drop(&mut self) {
        for (_, mut timer) in self.timers.drain() {
            timer.close(());
        }
        for (_, mut signal) in self.signals.drain() {
            if let Err(error) = signal.stop() {
                eprintln!("node: could not stop libuv signal handle: {error}");
            }
            signal.close(());
        }
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

pub fn schedule_timer(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let id = args.get(0).integer_value(scope).unwrap_or(0).max(0) as u64;
    let timeout = args
        .get(1)
        .integer_value(scope)
        .unwrap_or(0)
        .clamp(0, 2_147_483_647) as u64;
    let repeat = args
        .get(2)
        .integer_value(scope)
        .unwrap_or(0)
        .clamp(0, 2_147_483_647) as u64;
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        throw_error(scope, &mut return_value, "libuv event loop is unavailable");
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    if let Err(error) = state.start_timer(id, timeout, repeat) {
        throw_error(scope, &mut return_value, &error);
    }
}

pub fn cancel_timer(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let id = args.get(0).integer_value(scope).unwrap_or(0).max(0) as u64;
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    if let Err(error) = state.cancel_timer(id) {
        throw_error(scope, &mut return_value, &error);
    }
}

pub fn watch_signal(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);
    let signal = args.get(1).integer_value(scope).unwrap_or(0) as i32;
    let watch = args.get(2).boolean_value(scope);
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        throw_error(scope, &mut return_value, "libuv event loop is unavailable");
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    if let Err(error) = state.watch_signal(name, signal, watch) {
        throw_error(scope, &mut return_value, &error);
    }
}

pub fn request_exit(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    _return_value: v8::ReturnValue,
) {
    let code = args.get(0).integer_value(scope).unwrap_or(0) as i32;
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return;
    }
    unsafe { &mut *pointer.cast::<AsyncIo>() }.exit_code = Some(code);
    let Some(message) = v8::String::new(scope, "process.exit was called") else {
        return;
    };
    let exception = v8::Exception::error(scope, message);
    if let Ok(error) = v8::Local::<v8::Object>::try_from(exception) {
        if let Some(key) = v8::String::new(scope, "name") {
            if let Some(value) = v8::String::new(scope, "RuntimeExit") {
                let _ = error.set(scope, key.into(), value.into());
            }
        }
    }
    scope.throw_exception(exception);
}

pub fn is_exit_requested(
    scope: &mut HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    let requested =
        !pointer.is_null() && unsafe { &*pointer.cast::<AsyncIo>() }.exit_code.is_some();
    return_value.set(v8::Boolean::new(scope, requested).into());
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
        return_value.set(v8::Array::new(scope, 0).into());
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    match state.pump(scope, wait_ms) {
        Ok(events) => {
            let encoded = serde_json::to_string(&events)
                .unwrap_or_else(|error| format!(r#"["serialization error: {error}"]"#));
            if let Some(value) = v8::String::new(scope, &encoded) {
                return_value.set(value.into());
            } else {
                throw_error(scope, &mut return_value, "could not encode libuv events");
            }
        }
        Err(error) => {
            throw_error(scope, &mut return_value, &error);
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

pub fn take_unhandled_rejections(
    scope: &mut HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return_value.set(v8::Array::new(scope, 0).into());
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    let pending = std::mem::take(&mut state.pending_rejections);
    let length = i32::try_from(pending.len()).unwrap_or(i32::MAX);
    let rejections = v8::Array::new(scope, length);
    for (index, (promise, reason)) in pending.into_iter().enumerate() {
        let pair = v8::Array::new(scope, 2);
        let promise = Local::new(scope, &promise);
        let reason = Local::new(scope, &reason);
        if pair.set_index(scope, 0, promise.into()) != Some(true)
            || pair.set_index(scope, 1, reason) != Some(true)
            || rejections.set_index(scope, index as u32, pair.into()) != Some(true)
        {
            let Some(message) =
                v8::String::new(scope, "could not create unhandled rejection event")
            else {
                return;
            };
            let error = v8::Exception::error(scope, message);
            scope.throw_exception(error);
            return;
        }
    }
    return_value.set(rejections.into());
}

pub fn take_handled_rejections(
    scope: &mut HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return_value.set(v8::Array::new(scope, 0).into());
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    let pending = std::mem::take(&mut state.pending_rejection_handled);
    let length = i32::try_from(pending.len()).unwrap_or(i32::MAX);
    let promises = v8::Array::new(scope, length);
    for (index, promise) in pending.into_iter().enumerate() {
        let promise = Local::new(scope, &promise);
        if promises.set_index(scope, index as u32, promise.into()) != Some(true) {
            let Some(message) = v8::String::new(scope, "could not create rejectionHandled event")
            else {
                return;
            };
            let error = v8::Exception::error(scope, message);
            scope.throw_exception(error);
            return;
        }
    }
    return_value.set(promises.into());
}

pub unsafe extern "C" fn promise_reject_callback(message: v8::PromiseRejectMessage) {
    let mut scope = unsafe { v8::CallbackScope::new(&message) };
    let context = scope.get_current_context();
    let pointer = context.get_aligned_pointer_from_embedder_data(ASYNC_IO_SLOT);
    if pointer.is_null() {
        return;
    }
    let state = unsafe { &mut *pointer.cast::<AsyncIo>() };
    state.track_promise_rejection(
        &mut scope,
        message.get_event(),
        message.get_promise(),
        message.get_value(),
    );
}

fn reject(scope: &mut HandleScope, resolver: Local<PromiseResolver>, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::error(scope, message);
        let _ = resolver.reject(scope, error);
    }
}

fn throw_error(scope: &mut HandleScope, _return_value: &mut v8::ReturnValue, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
    }
}
