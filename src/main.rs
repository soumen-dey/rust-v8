mod async_io;
mod module_loader;
mod resolver;

use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

enum Command {
    Help,
    Version,
    Repl,
    Eval {
        source: String,
        args: Vec<String>,
    },
    Script {
        path: PathBuf,
        args: Vec<String>,
        check_only: bool,
    },
}

fn decode_file_data(serialized: &str) -> Result<Vec<u8>, std::io::Error> {
    serde_json::from_str::<Vec<u8>>(serialized)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))
}

const NODE_COMPAT_VERSION: &str = "24.22.0";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Repl) => run_repl(),
        Ok(Command::Help) => {
            print_help();
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("v{NODE_COMPAT_VERSION}");
            ExitCode::SUCCESS
        }
        Ok(Command::Eval { source, args }) => run(&source, "<eval>", &args, false),
        Ok(Command::Script {
            path,
            args,
            check_only,
        }) if path.as_os_str() == "-" => {
            let mut source = String::new();
            match std::io::stdin().read_to_string(&mut source) {
                Ok(_) => run(&source, "-", &args, check_only),
                Err(error) => {
                    eprintln!("node: cannot read from stdin: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Command::Script {
            path,
            args,
            check_only,
        }) => match fs::read_to_string(&path) {
            Ok(source) => {
                let absolute_path = path.canonicalize().unwrap_or(path);
                run(&source, &absolute_path.to_string_lossy(), &args, check_only)
            }
            Err(error) => {
                eprintln!("node: cannot read '{}': {error}", path.display());
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("node: {error}");
            eprintln!("Try 'node --help' for usage.");
            ExitCode::from(2)
        }
    }
}

fn run_repl() -> ExitCode {
    let mut async_io = match async_io::AsyncIo::new() {
        Ok(async_io) => async_io,
        Err(error) => {
            eprintln!("node: {error}");
            return ExitCode::FAILURE;
        }
    };
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    let (result, status) = {
        let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        isolate
            .set_host_initialize_import_meta_object_callback(module_loader::initialize_import_meta);
        isolate.set_host_import_module_dynamically_callback(module_loader::dynamic_import);
        isolate.set_promise_reject_callback(async_io::promise_reject_callback);
        let handle_scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);

        let mut module_loader = module_loader::ModuleLoader::new();
        module_loader.attach(context);
        async_io.attach(context);
        let result = install_globals(scope, "<repl>", &[])
            .and_then(|()| {
                execute_script(scope, include_str!("bootstrap.js"), "<runtime bootstrap>")
            })
            .and_then(|()| repl_loop(scope));
        let (result, status) = match finalize_process(scope) {
            Ok(status) => (result, status),
            Err(error) => (Err(error), 1),
        };
        async_io.detach(context);
        module_loader.detach(context);
        drop(async_io);
        (result, status)
    };

    unsafe {
        v8::V8::dispose();
    }
    v8::V8::dispose_platform();

    match result {
        Ok(()) => ExitCode::from(status),
        Err(error) => {
            eprintln!("node: {error}");
            ExitCode::FAILURE
        }
    }
}

fn repl_loop(scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>) -> Result<(), String> {
    let interactive = io::stdin().is_terminal();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let mut input_closed = false;
    loop {
        match if input_closed {
            Err(TryRecvError::Disconnected)
        } else {
            receiver.try_recv()
        } {
            Ok(Ok(input)) => {
                match input.trim() {
                    ".exit" | ".quit" => return Ok(()),
                    ".help" => {
                        println!(
                            "Enter JavaScript expressions or statements. Commands: .help, .exit"
                        );
                    }
                    "" => {}
                    _ => match execute_repl_input(scope, &input) {
                        Ok(Some(output)) => println!("{output}"),
                        Ok(None) => {}
                        Err(error) => eprintln!("{error}"),
                    },
                }
                if interactive {
                    print!("> ");
                    io::stdout()
                        .flush()
                        .map_err(|error| format!("could not write REPL prompt: {error}"))?;
                }
            }
            Ok(Err(error)) => return Err(format!("could not read REPL input: {error}")),
            Err(TryRecvError::Disconnected) => input_closed = true,
            Err(TryRecvError::Empty) => {}
        }
        match run_event_loop_turn(scope, 25, !input_closed) {
            Ok(true) => {}
            Ok(false) if input_closed => return Ok(()),
            Ok(false) => {}
            Err(error) => eprintln!("{error}"),
        }
    }
}

fn execute_repl_input(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
) -> Result<Option<String>, String> {
    let handle_scope = &mut v8::HandleScope::new(scope);
    let try_catch = &mut v8::TryCatch::new(handle_scope);
    let code = v8::String::new(try_catch, source)
        .ok_or_else(|| "could not allocate JavaScript REPL input".to_owned())?;
    let script = v8::Script::compile(try_catch, code, None)
        .ok_or_else(|| caught_exception(try_catch, "compilation", "<repl>"))?;
    let value = script
        .run(try_catch)
        .ok_or_else(|| caught_exception(try_catch, "execution", "<repl>"))?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value.to_rust_string_lossy(try_catch)))
    }
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some(first) = args.first() else {
        return Ok(Command::Repl);
    };

    match first.as_str() {
        "--help" | "-h" => return Ok(Command::Help),
        "--version" | "-v" => return Ok(Command::Version),
        "--eval" | "-e" => {
            let source = args
                .get(1)
                .ok_or_else(|| "expected JavaScript source after -e".to_owned())?;
            return Ok(Command::Eval {
                source: source.clone(),
                args: args[2..].to_vec(),
            });
        }
        _ => {}
    }

    let (script_index, check_only) = if first == "--check" || first == "-c" {
        (1, true)
    } else {
        (0, false)
    };
    let path = args
        .get(script_index)
        .ok_or_else(|| "expected a JavaScript file path".to_owned())?;

    if path.starts_with('-') && path != "-" {
        return Err(format!("unknown option '{path}'"));
    }

    Ok(Command::Script {
        path: PathBuf::from(path),
        args: args[script_index + 1..].to_vec(),
        check_only,
    })
}

fn run(source: &str, filename: &str, args: &[String], check_only: bool) -> ExitCode {
    let mut async_io = match async_io::AsyncIo::new() {
        Ok(async_io) => async_io,
        Err(error) => {
            eprintln!("node: {error}");
            return ExitCode::FAILURE;
        }
    };
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    let (result, status) = {
        let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        isolate
            .set_host_initialize_import_meta_object_callback(module_loader::initialize_import_meta);
        isolate.set_host_import_module_dynamically_callback(module_loader::dynamic_import);
        isolate.set_promise_reject_callback(async_io::promise_reject_callback);
        let handle_scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);

        let mut module_loader = module_loader::ModuleLoader::new();
        module_loader.attach(context);
        async_io.attach(context);
        let result = if let Err(error) = install_globals(scope, filename, args) {
            Err(error)
        } else {
            let bootstrap = include_str!("bootstrap.js");
            if let Err(error) = execute_script(scope, bootstrap, "<runtime bootstrap>") {
                Err(error)
            } else if check_only {
                if resolver::is_es_module(PathBuf::from(filename).as_path()) {
                    check_module(
                        scope,
                        source,
                        filename,
                        std::ptr::addr_of_mut!(module_loader),
                    )
                } else {
                    execute_script(scope, source, filename)
                }
            } else if filename == "<eval>" || filename == "-" {
                execute_script(scope, source, filename).and_then(|()| drain_event_loop(scope))
            } else if resolver::is_es_module(PathBuf::from(filename).as_path()) {
                unsafe {
                    module_loader::execute(
                        scope,
                        source,
                        filename,
                        false,
                        std::ptr::addr_of_mut!(module_loader),
                    )
                }
            } else {
                let filename = serde_json::to_string(filename)
                    .map_err(|error| format!("could not encode script path: {error}"));
                filename.and_then(|filename| {
                    execute_script(
                        scope,
                        &format!("__runtimeRunMain({filename})"),
                        "<main module>",
                    )
                    .and_then(|()| drain_event_loop(scope))
                })
            }
        };
        let finalized = if check_only {
            Ok(0)
        } else {
            finalize_process(scope)
        };
        let (result, status) = match finalized {
            Ok(status) => (result, status),
            Err(error) => (Err(error), 1),
        };
        async_io.detach(context);
        module_loader.detach(context);
        drop(async_io);
        (result, status)
    };

    unsafe {
        v8::V8::dispose();
    }
    v8::V8::dispose_platform();

    match result {
        Ok(()) => ExitCode::from(status),
        Err(error) => {
            eprintln!("node: {error}");
            ExitCode::FAILURE
        }
    }
}

fn install_globals(
    scope: &mut v8::HandleScope<'_>,
    filename: &str,
    args: &[String],
) -> Result<(), String> {
    let context = scope.get_current_context();
    let global = context.global(scope);

    let console = v8::Object::new(scope);
    install_callback(scope, console, "log", console_log)?;
    install_callback(scope, console, "info", console_log)?;
    install_callback(scope, console, "warn", console_error)?;
    install_callback(scope, console, "error", console_error)?;

    let console_key = v8::String::new(scope, "console")
        .ok_or_else(|| "could not create console global".to_owned())?;
    if !global
        .set(scope, console_key.into(), console.into())
        .ok_or_else(|| "could not set console global".to_owned())?
    {
        return Err("could not install console global".to_owned());
    }

    let process = v8::Object::new(scope);
    let executable = env::current_exe()
        .map_err(|error| format!("could not determine executable path: {error}"))?
        .to_string_lossy()
        .into_owned();
    let mut process_args = vec![executable];
    if filename != "<eval>" {
        process_args.push(filename.to_owned());
    }
    process_args.extend(args.iter().cloned());
    let argv_length = i32::try_from(process_args.len())
        .map_err(|_| "too many command-line arguments".to_owned())?;
    let argv = v8::Array::new(scope, argv_length);
    for (index, arg) in process_args.iter().enumerate() {
        let value = v8::String::new(scope, arg)
            .ok_or_else(|| "could not create process.argv value".to_owned())?;
        if !argv
            .set_index(scope, index as u32, value.into())
            .ok_or_else(|| "could not set process.argv value".to_owned())?
        {
            return Err("could not populate process.argv".to_owned());
        }
    }
    set_object_property(scope, process, "argv", argv.into())?;
    set_string_property(
        scope,
        process,
        "version",
        &format!("v{NODE_COMPAT_VERSION}"),
    )?;
    set_string_property(scope, process, "platform", platform_name())?;
    set_string_property(scope, process, "arch", arch_name())?;

    let process_key = v8::String::new(scope, "process")
        .ok_or_else(|| "could not create process global".to_owned())?;
    if !global
        .set(scope, process_key.into(), process.into())
        .ok_or_else(|| "could not set process global".to_owned())?
    {
        return Err("could not install process global".to_owned());
    }

    install_callback(scope, global, "__runtimeResolve", runtime_resolve)?;
    install_callback(scope, global, "__runtimeReadFile", runtime_read_file)?;
    install_callback(
        scope,
        global,
        "__runtimeRequireEsm",
        module_loader::require_esm_callback,
    )?;
    install_callback(scope, global, "__runtimeIsEsm", runtime_is_esm)?;
    install_callback(scope, global, "__runtimeFs", runtime_fs)?;
    install_callback(scope, global, "__runtimeFsAsync", async_io::submit_fs)?;
    install_callback(scope, global, "__runtimePumpAsync", async_io::pump)?;
    install_callback(
        scope,
        global,
        "__runtimeScheduleTimer",
        async_io::schedule_timer,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeCancelTimer",
        async_io::cancel_timer,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeUpdateTimer",
        async_io::update_timer,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeWatchSignal",
        async_io::watch_signal,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeRequestExit",
        async_io::request_exit,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeHasPendingAsync",
        async_io::has_pending,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeHasQueuedAsyncEvents",
        async_io::has_queued_events,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeTakeUnhandledRejections",
        async_io::take_unhandled_rejections,
    )?;
    install_callback(
        scope,
        global,
        "__runtimeTakeHandledRejections",
        async_io::take_handled_rejections,
    )?;
    install_callback(scope, global, "__runtimeProcess", runtime_process)?;
    Ok(())
}

fn install_callback<'s, F>(
    scope: &mut v8::HandleScope<'s>,
    object: v8::Local<'s, v8::Object>,
    name: &str,
    callback: F,
) -> Result<(), String>
where
    F: v8::MapFnTo<v8::FunctionCallback>,
{
    let function = v8::Function::new(scope, callback)
        .ok_or_else(|| format!("could not create {name} function"))?;
    let key = v8::String::new(scope, name).ok_or_else(|| format!("could not create {name} key"))?;
    if object
        .set(scope, key.into(), function.into())
        .ok_or_else(|| format!("could not set {name}"))?
    {
        Ok(())
    } else {
        Err(format!("could not install {name}"))
    }
}

fn execute_script(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
    filename: &str,
) -> Result<(), String> {
    let failure = {
        let handle_scope = &mut v8::HandleScope::new(scope);
        let try_catch = &mut v8::TryCatch::new(handle_scope);
        let code = v8::String::new(try_catch, source)
            .ok_or_else(|| format!("could not allocate JavaScript source for {filename}"))?;
        let script = match v8::Script::compile(try_catch, code, None) {
            Some(script) => script,
            None => return Err(caught_exception(try_catch, "compilation", filename)),
        };
        if script.run(try_catch).is_some() {
            return Ok(());
        }
        let detail = caught_exception(try_catch, "execution", filename);
        let exception = try_catch
            .exception()
            .map(|exception| v8::Global::new(try_catch, exception));
        (exception, detail)
    };
    if let Some(exception) = failure.0 {
        if dispatch_uncaught_exception(scope, exception, "uncaughtException")? {
            return Ok(());
        }
    }
    Err(failure.1)
}

pub(crate) fn dispatch_uncaught_exception(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    exception: v8::Global<v8::Value>,
    origin: &str,
) -> Result<bool, String> {
    let handle_scope = &mut v8::HandleScope::new(scope);
    let try_catch = &mut v8::TryCatch::new(handle_scope);
    let context = try_catch.get_current_context();
    let global = context.global(try_catch);
    let key = v8::String::new(try_catch, "__runtimeHandleUncaughtException")
        .ok_or_else(|| "could not create uncaught-exception handler key".to_owned())?;
    let Some(handler) = global
        .get(try_catch, key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return Ok(false);
    };
    let exception = v8::Local::new(try_catch, &exception);
    let origin_value = v8::String::new(try_catch, origin)
        .ok_or_else(|| "could not create uncaught-exception origin".to_owned())?;
    let arguments = [exception, origin_value.into()];
    let handled = handler
        .call(try_catch, global.into(), &arguments)
        .ok_or_else(|| caught_exception(try_catch, "uncaughtException handler", origin))?
        .boolean_value(try_catch);
    Ok(handled)
}

fn finalize_process(scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>) -> Result<u8, String> {
    execute_script(scope, "__runtimeFinalizeProcess()", "<process exit>")?;
    let handle_scope = &mut v8::HandleScope::new(scope);
    let try_catch = &mut v8::TryCatch::new(handle_scope);
    let code = v8::String::new(try_catch, "Number(process.exitCode)")
        .ok_or_else(|| "could not allocate process exit-code query".to_owned())?;
    let script = v8::Script::compile(try_catch, code, None)
        .ok_or_else(|| caught_exception(try_catch, "compilation", "<process exit>"))?;
    let status = script
        .run(try_catch)
        .ok_or_else(|| caught_exception(try_catch, "execution", "<process exit>"))?
        .integer_value(try_catch)
        .unwrap_or(0);
    Ok(status.rem_euclid(256) as u8)
}

fn run_event_loop_turn(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    wait_ms: u64,
    wait_when_idle: bool,
) -> Result<bool, String> {
    execute_script(scope, "__runtimeDrainNextTicks()", "<nextTick queue>")?;
    AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
    execute_script(
        scope,
        "__runtimeDispatchUnhandledRejections()",
        "<unhandled rejection handler>",
    )?;
    AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
    let source = format!("__runtimeRunEventLoopTurn({wait_ms}, {wait_when_idle})");
    execute_script(scope, &source, "<event loop>")?;
    AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
    execute_script(
        scope,
        "__runtimeDispatchUnhandledRejections()",
        "<unhandled rejection handler>",
    )?;
    AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
    read_pending_work(scope)
}

fn read_pending_work(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<bool, String> {
    let handle_scope = &mut v8::HandleScope::new(scope);
    let try_catch = &mut v8::TryCatch::new(handle_scope);
    let status_code = v8::String::new(try_catch, "__runtimeHasPendingWork()")
        .ok_or_else(|| "could not allocate event-loop status query".to_owned())?;
    let status_script = v8::Script::compile(try_catch, status_code, None)
        .ok_or_else(|| caught_exception(try_catch, "compilation", "<event loop>"))?;
    let pending = status_script
        .run(try_catch)
        .ok_or_else(|| caught_exception(try_catch, "execution", "<event loop>"))?
        .boolean_value(try_catch);
    Ok(pending)
}

fn drain_event_loop(scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>) -> Result<(), String> {
    loop {
        if run_event_loop_turn(scope, 250, false)? {
            continue;
        }
        execute_script(scope, "__runtimeEmitBeforeExit()", "<beforeExit event>")?;
        execute_script(scope, "__runtimeDrainNextTicks()", "<nextTick queue>")?;
        AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
        execute_script(
            scope,
            "__runtimeDispatchUnhandledRejections()",
            "<unhandled rejection handler>",
        )?;
        AsMut::<v8::Isolate>::as_mut(scope).perform_microtask_checkpoint();
        if !read_pending_work(scope)? {
            return Ok(());
        }
    }
}

fn check_module(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
    filename: &str,
    loader: *mut module_loader::ModuleLoader,
) -> Result<(), String> {
    unsafe { module_loader::execute(scope, source, filename, true, loader) }
}

fn caught_exception(
    try_catch: &mut v8::TryCatch<'_, v8::HandleScope<'_>>,
    phase: &str,
    filename: &str,
) -> String {
    let detail = try_catch
        .stack_trace()
        .or_else(|| try_catch.exception())
        .map(|value| value.to_rust_string_lossy(try_catch))
        .unwrap_or_else(|| "no JavaScript exception details available".to_owned());
    format!("JavaScript {phase} failed in {filename}: {detail}")
}

fn set_string_property<'s>(
    scope: &mut v8::HandleScope<'s>,
    object: v8::Local<'s, v8::Object>,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let value =
        v8::String::new(scope, value).ok_or_else(|| format!("could not create {name} value"))?;
    set_object_property(scope, object, name, value.into())
}

fn set_object_property<'s>(
    scope: &mut v8::HandleScope<'s>,
    object: v8::Local<'s, v8::Object>,
    name: &str,
    value: v8::Local<'s, v8::Value>,
) -> Result<(), String> {
    let key =
        v8::String::new(scope, name).ok_or_else(|| format!("could not create {name} property"))?;
    if object
        .set(scope, key.into(), value)
        .ok_or_else(|| format!("could not set {name} property"))?
    {
        Ok(())
    } else {
        Err(format!("could not install {name} property"))
    }
}

fn console_log(
    scope: &mut v8::HandleScope<'_>,
    args: v8::FunctionCallbackArguments<'_>,
    _return_value: v8::ReturnValue<'_, v8::Value>,
) {
    let values = (0..args.length())
        .map(|index| args.get(index).to_rust_string_lossy(scope))
        .collect::<Vec<_>>();
    println!("{}", values.join(" "));
}

fn runtime_is_esm(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let filename = args.get(0).to_rust_string_lossy(scope);
    let is_esm = resolver::is_es_module(PathBuf::from(filename).as_path());
    return_value.set(v8::Boolean::new(scope, is_esm).into());
}

fn console_error(
    scope: &mut v8::HandleScope<'_>,
    args: v8::FunctionCallbackArguments<'_>,
    _return_value: v8::ReturnValue<'_, v8::Value>,
) {
    let values = (0..args.length())
        .map(|index| args.get(index).to_rust_string_lossy(scope))
        .collect::<Vec<_>>();
    eprintln!("{}", values.join(" "));
}

fn runtime_resolve(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let specifier = args.get(0).to_rust_string_lossy(scope);
    let parent = args.get(1).to_rust_string_lossy(scope);
    let mode = if args.get(2).boolean_value(scope) {
        resolver::ResolutionMode::Import
    } else {
        resolver::ResolutionMode::Require
    };
    let parent = if parent == "null" || parent == "undefined" {
        env::current_dir()
            .map(|directory| directory.join("__entry__.js"))
            .unwrap_or_else(|_| PathBuf::from("__entry__.js"))
    } else {
        PathBuf::from(parent)
    };

    match resolver::resolve(&specifier, &parent, mode) {
        Ok(resolver::Resolution::Builtin(name)) => {
            if let Some(value) = v8::String::new(scope, &format!("node:{name}")) {
                return_value.set(value.into());
            } else {
                return_value.set_null();
            }
        }
        Ok(resolver::Resolution::File(path)) => {
            let path = path.to_string_lossy();
            if let Some(value) = v8::String::new(scope, &path) {
                return_value.set(value.into());
            } else {
                return_value.set_null();
            }
        }
        Err(_) => return_value.set_null(),
    }
}

fn runtime_read_file(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let filename = args.get(0).to_rust_string_lossy(scope);
    match fs::read_to_string(&filename) {
        Ok(contents) => {
            if let Some(value) = v8::String::new(scope, &contents) {
                return_value.set(value.into());
            } else {
                return_value.set_null();
            }
        }
        Err(_) => return_value.set_null(),
    }
}

fn runtime_fs(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let operation = args.get(0).to_rust_string_lossy(scope);
    let first = args.get(1).to_rust_string_lossy(scope);
    let second = args.get(2).to_rust_string_lossy(scope);
    let result = fs_operation(&operation, &first, &second);
    let encoded = serde_json::to_string(&result).unwrap_or_else(|error| {
        format!(r#"{{"error":{{"code":"ERR_RUNTIME","message":"{error}"}}}}"#)
    });
    if let Some(value) = v8::String::new(scope, &encoded) {
        return_value.set(value.into());
    } else {
        return_value.set_null();
    }
}

pub(crate) fn fs_operation(operation: &str, first: &str, second: &str) -> serde_json::Value {
    use serde_json::json;

    let first_path = node_path(first);
    let second_path = node_path(second);
    let result: Result<serde_json::Value, std::io::Error> = match operation {
        "readFile" => fs::read(&first_path).map(|contents| json!(contents)),
        "writeFile" => decode_file_data(second)
            .and_then(|bytes| fs::write(&first_path, bytes))
            .map(|()| json!(null)),
        "appendFile" => {
            use std::io::Write;
            decode_file_data(second)
                .and_then(|bytes| {
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&first_path)
                        .and_then(|mut file| file.write_all(&bytes))
                })
                .map(|()| json!(null))
        }
        "exists" => Ok(json!(first_path.exists())),
        "mkdir" => fs::create_dir(&first_path).map(|()| json!(null)),
        "mkdirRecursive" => fs::create_dir_all(&first_path).map(|()| json!(null)),
        "mkdtemp" => create_temp_directory(first).map(|path| json!(path)),
        "copyFile" => fs::copy(&first_path, &second_path).map(|bytes| json!(bytes)),
        "hardLink" => fs::hard_link(&first_path, &second_path).map(|()| json!(null)),
        "symlinkFile" => create_symlink(&first_path, &second_path, false).map(|()| json!(null)),
        "symlinkDir" => create_symlink(&first_path, &second_path, true).map(|()| json!(null)),
        "access" => fs::metadata(&first_path).map(|_| json!(null)),
        "truncate" => parse_u64(second)
            .and_then(|length| {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&first_path)
                    .and_then(|file| file.set_len(length))
            })
            .map(|()| json!(null)),
        "chmod" => set_permissions(&first_path, second).map(|()| json!(null)),
        "realpath" => {
            fs::canonicalize(&first_path).map(|path| json!(path.to_string_lossy().into_owned()))
        }
        "readlink" => {
            fs::read_link(&first_path).map(|path| json!(path.to_string_lossy().into_owned()))
        }
        "removeFile" => fs::remove_file(&first_path).map(|()| json!(null)),
        "removeDir" => fs::remove_dir(&first_path).map(|()| json!(null)),
        "removeDirRecursive" => fs::remove_dir_all(&first_path).map(|()| json!(null)),
        "lstat" => fs::symlink_metadata(&first_path).map(|metadata| {
            json!({
                "size": metadata.len(),
                "mode": metadata_mode(&metadata),
                "file": metadata.file_type().is_file(),
                "directory": metadata.file_type().is_dir(),
                "symlink": metadata.file_type().is_symlink()
            })
        }),
        "stat" => fs::metadata(&first_path).map(|metadata| {
            json!({
                "size": metadata.len(),
                "mode": metadata_mode(&metadata),
                "file": metadata.is_file(),
                "directory": metadata.is_dir(),
                "symlink": false
            })
        }),
        "readdir" => fs::read_dir(&first_path).and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<Result<Vec<_>, _>>()
                .map(|names| json!(names))
        }),
        "unlink" => fs::remove_file(&first_path).map(|()| json!(null)),
        "rename" => fs::rename(&first_path, &second_path).map(|()| json!(null)),
        _ => {
            return json!({
                "error": {
                    "code": "ERR_INVALID_ARG_VALUE",
                    "message": format!("Unsupported filesystem operation '{operation}'")
                }
            })
        }
    };

    match result {
        Ok(value) => json!({ "value": value }),
        Err(error) => {
            let code = filesystem_error_code(&error);
            let mut details = json!({
                "code": code,
                "errno": filesystem_errno(code),
                "message": error.to_string(),
                "syscall": filesystem_syscall(operation),
                "path": first_path.to_string_lossy(),
            });
            if matches!(operation, "rename" | "copyFile") {
                details["dest"] = json!(second_path.to_string_lossy());
            }
            json!({
                "error": details
            })
        }
    }
}

fn filesystem_error_code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => "EINVAL",
        std::io::ErrorKind::NotADirectory => "ENOTDIR",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        std::io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        std::io::ErrorKind::StorageFull => "ENOSPC",
        std::io::ErrorKind::ReadOnlyFilesystem => "EROFS",
        std::io::ErrorKind::TimedOut => "ETIMEDOUT",
        std::io::ErrorKind::Interrupted => "EINTR",
        std::io::ErrorKind::Unsupported => "ENOSYS",
        _ => "ERR_FS",
    }
}

fn filesystem_errno(code: &str) -> i32 {
    match code {
        "ENOENT" => -2,
        "EINTR" => -4,
        "EIO" => -5,
        "EBADF" => -9,
        "EAGAIN" => -11,
        "ENOMEM" => -12,
        "EACCES" => -13,
        "EBUSY" => -16,
        "EEXIST" => -17,
        "EXDEV" => -18,
        "ENOTDIR" => -20,
        "EISDIR" => -21,
        "EINVAL" => -22,
        "ENFILE" => -23,
        "EMFILE" => -24,
        "EFBIG" => -27,
        "ENOSPC" => -28,
        "EROFS" => -30,
        "EMLINK" => -31,
        "EPIPE" => -32,
        "ENAMETOOLONG" => -36,
        "ENOSYS" => -38,
        "ENOTEMPTY" => -39,
        "ETIMEDOUT" => -110,
        _ => -1,
    }
}

fn filesystem_syscall(operation: &str) -> &'static str {
    match operation {
        "readFile" => "open",
        "writeFile" | "appendFile" => "open",
        "exists" => "access",
        "mkdir" | "mkdirRecursive" => "mkdir",
        "mkdtemp" => "mkdtemp",
        "copyFile" => "copyfile",
        "hardLink" => "link",
        "symlinkFile" | "symlinkDir" => "symlink",
        "access" => "access",
        "truncate" => "truncate",
        "chmod" => "chmod",
        "realpath" => "realpath",
        "readlink" => "readlink",
        "removeFile" | "unlink" => "unlink",
        "removeDir" | "removeDirRecursive" => "rmdir",
        "lstat" => "lstat",
        "stat" => "stat",
        "readdir" => "scandir",
        "rename" => "rename",
        _ => "unknown",
    }
}

fn create_temp_directory(prefix: &str) -> Result<String, std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let base = format!(
        "{prefix}{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos()
    );
    for _ in 0..32 {
        let candidate = format!("{base}-{}", NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary directory name",
    ))
}

#[cfg(unix)]
fn create_symlink(
    target: &PathBuf,
    link: &PathBuf,
    _directory: bool,
) -> Result<(), std::io::Error> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &PathBuf, link: &PathBuf, directory: bool) -> Result<(), std::io::Error> {
    if directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

fn node_path(path: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(path)
    }
}

fn parse_u64(value: &str) -> Result<u64, std::io::Error> {
    value
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))
}

fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        u32::from(metadata.permissions().readonly())
    }
}

fn set_permissions(path: &std::path::Path, mode: &str) -> Result<(), std::io::Error> {
    let mode = parse_u64(mode)? as u32;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        fs::set_permissions(path, permissions)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, mode);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "chmod is not supported on this platform",
        ))
    }
}

fn runtime_process(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let operation = args.get(0).to_rust_string_lossy(scope);
    let value = args.get(1).to_rust_string_lossy(scope);
    let result = match operation.as_str() {
        "cwd" => env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .map(|path| serde_json::json!(path))
            .unwrap_or_else(|error| {
                serde_json::json!({ "error": { "code": "ERR_CWD", "message": error.to_string() } })
            }),
        "chdir" => match env::set_current_dir(&value) {
            Ok(()) => serde_json::Value::Null,
            Err(error) => {
                serde_json::json!({ "error": { "code": "ERR_CHDIR", "message": error.to_string() } })
            }
        },
        "getenv" => serde_json::json!(env::var(&value).ok()),
        "setenv" => {
            let new_value = args.get(2).to_rust_string_lossy(scope);
            unsafe {
                env::set_var(&value, new_value);
            }
            serde_json::Value::Null
        }
        "envkeys" => {
            let keys: Vec<String> = env::vars_os()
                .map(|(key, _)| key.to_string_lossy().into_owned())
                .collect();
            serde_json::json!(keys)
        }
        _ => serde_json::Value::Null,
    };
    let result = serde_json::to_string(&result).unwrap_or_else(|_| "null".to_owned());
    if let Some(value) = v8::String::new(scope, &result) {
        return_value.set(value.into());
    } else {
        return_value.set_null();
    }
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

fn arch_name() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

fn print_help() {
    println!(
        "Usage: node [options] [script.js] [arguments]\n\
         \n\
         Options:\n\
           -e, --eval <source>  Evaluate JavaScript source\n\
           -c, --check <file>   Check JavaScript syntax without running it\n\
           -v, --version        Print runtime version\n\
           -h, --help           Show this help\n\
         \n\
         With no script, start the interactive REPL.\n\
         This runtime embeds V8 and is not yet a Node.js-compatible replacement."
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_args, Command};

    #[test]
    fn parses_eval_source() {
        let args = vec!["-e".to_owned(), "1 + 1".to_owned()];
        assert!(matches!(
            parse_args(&args),
            Ok(Command::Eval { source, args })
                if source == "1 + 1" && args.is_empty()
        ));
    }

    #[test]
    fn parses_script_and_arguments() {
        let args = vec!["app.js".to_owned(), "one".to_owned(), "two".to_owned()];
        assert!(matches!(
            parse_args(&args),
            Ok(Command::Script { path, args, check_only: false })
                if path.as_path() == std::path::Path::new("app.js")
                    && args == vec!["one".to_owned(), "two".to_owned()]
        ));
    }

    #[test]
    fn parses_syntax_check() {
        let args = vec!["--check".to_owned(), "app.js".to_owned()];
        assert!(matches!(
            parse_args(&args),
            Ok(Command::Script { path, check_only: true, .. })
                if path.as_path() == std::path::Path::new("app.js")
        ));
    }

    #[test]
    fn rejects_unknown_options() {
        let args = vec!["--not-an-option".to_owned()];
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn reports_missing_eval_source() {
        let args = vec!["--eval".to_owned()];
        assert!(parse_args(&args).is_err());
    }
}
