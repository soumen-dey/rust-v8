use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn runtime() -> Command {
    Command::new(env!("CARGO_BIN_EXE_node"))
}

struct TempProject(std::path::PathBuf);

impl TempProject {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rust-node-runtime-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).expect("temporary project directory should be created");
        Self(path)
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn evaluates_javascript_and_writes_console_output() {
    let output = runtime()
        .args(["--eval", "console.log(1 + 1)"])
        .output()
        .expect("runtime process should start");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn supports_async_filesystem_promises_and_callbacks() {
    let project = TempProject::new();
    let path = project.0.join("async.txt");
    let path = serde_json::to_string(&path.to_string_lossy().as_ref())
        .expect("filesystem path should be encoded");
    let source = format!(
        concat!(
            "const fs = require('node:fs');\n",
            "fs.promises.writeFile({path}, 'async data')\n",
            "  .then(() => fs.promises.readFile({path}, 'utf8'))\n",
            "  .then((value) => {{\n",
            "    console.log('promise:' + value);\n",
            "    fs.readFile({path}, 'utf8', (error, callbackValue) => {{\n",
            "      if (error) throw error;\n",
            "      console.log('callback:' + callbackValue);\n",
            "    }});\n",
            "  }});\n",
        ),
        path = path,
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["promise:async data", "callback:async data"]
    );
}

#[test]
fn creates_temp_directories_and_hard_links_asynchronously() {
    let project = TempProject::new();
    let prefix = serde_json::to_string(&project.0.join("generated-").to_string_lossy().as_ref())
        .expect("temporary directory prefix should be encoded");
    let source = format!(
        concat!(
            "const fs = require('node:fs/promises');\n",
            "fs.mkdtemp({prefix}).then(async (directory) => {{\n",
            "  const original = directory + '/original.txt';\n",
            "  const linked = directory + '/linked.txt';\n",
            "  const copied = directory + '/copied.txt';\n",
            "  const renamed = directory + '/renamed.txt';\n",
            "  await fs.writeFile(original, 'linked-data');\n",
            "  await fs.link(original, linked);\n",
            "  await fs.copyFile(original, copied);\n",
            "  await fs.rename(copied, renamed);\n",
            "  console.log((await fs.readFile(linked, 'utf8')) + ':' + (await fs.readFile(renamed, 'utf8')) + ':' + (await fs.readdir(directory)).length);\n",
            "}});\n",
        ),
        prefix = prefix,
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "linked-data:linked-data:3"
    );
}

#[test]
fn settles_concurrent_libuv_filesystem_requests() {
    let project = TempProject::new();
    let first = serde_json::to_string(&project.0.join("first.txt").to_string_lossy().as_ref())
        .expect("first filesystem path should be encoded");
    let second = serde_json::to_string(&project.0.join("second.txt").to_string_lossy().as_ref())
        .expect("second filesystem path should be encoded");
    let source = format!(
        concat!(
            "const fs = require('node:fs/promises');\n",
            "Promise.all([\n",
            "  fs.writeFile({first}, 'one'),\n",
            "  fs.writeFile({second}, 'two'),\n",
            "]).then(() => Promise.all([\n",
            "  fs.readFile({first}, 'utf8'),\n",
            "  fs.readFile({second}, 'utf8'),\n",
            "])).then((values) => console.log(values.join(':')));\n",
        ),
        first = first,
        second = second,
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "one:two");
}

#[test]
fn asynchronous_filesystem_errors_reject_with_node_error_codes() {
    let project = TempProject::new();
    let path = serde_json::to_string(&project.0.join("missing.txt").to_string_lossy().as_ref())
        .expect("filesystem path should be encoded");
    let source = format!(
        "require('node:fs').promises.readFile({path}).catch((error) => console.log([error.code, error.errno, error.syscall, error.path === {path}].join(':')));"
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ENOENT:-2:open:true"
    );
}

#[test]
fn callback_filesystem_exceptions_use_uncaught_exception_handling() {
    let project = TempProject::new();
    let path = serde_json::to_string(&project.0.join("callback.txt").to_string_lossy().as_ref())
        .expect("filesystem path should be encoded");
    std::fs::write(project.0.join("callback.txt"), "data").expect("test input should be written");
    let source = format!(
        "process.on('uncaughtException', (error) => console.log(error.message)); require('node:fs').readFile({path}, () => {{ throw new Error('callback-error'); }}); setTimeout(() => console.log('continues'), 0);"
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.lines().any(|line| line == "callback-error"));
    assert!(stdout.lines().any(|line| line == "continues"));
}

#[test]
fn services_timers_while_async_filesystem_work_is_pending() {
    let project = TempProject::new();
    let input = project.0.join("input.txt");
    std::fs::write(&input, "ready").expect("test input should be written");
    let path = serde_json::to_string(&input.to_string_lossy().as_ref())
        .expect("filesystem path should be encoded");
    let source = format!(
        concat!(
            "require('node:fs').promises.readFile({path}).then(() => console.log('io'));\n",
            "setTimeout(() => console.log('timer'), 0);\n",
            "console.log('sync');\n",
        ),
        path = path,
    );

    let output = runtime()
        .args(["--eval", &source])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"sync"));
    assert_eq!(lines.len(), 3);
    assert!(lines[1..].contains(&"timer"));
    assert!(lines[1..].contains(&"io"));
}

#[test]
fn keeps_interval_alive_until_callback_clears_it() {
    let output = runtime()
        .args([
            "--eval",
            "let count = 0; const interval = setInterval(() => { if (++count === 3) { clearInterval(interval); console.log(count); } }, 1);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
}

#[test]
fn supports_timer_reference_controls_and_numeric_cancellation() {
    let unreferenced = runtime()
        .args([
            "--eval",
            "const timer = setTimeout(() => console.log('should-not-fire'), 10).unref(); console.log(timer.hasRef());",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        unreferenced.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&unreferenced.stdout),
        String::from_utf8_lossy(&unreferenced.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&unreferenced.stdout).trim(),
        "false"
    );

    let referenced = runtime()
        .args([
            "--eval",
            "const unref = setTimeout(() => console.log('unref-fired'), 1).unref(); setTimeout(() => { console.log('ref-fired'); clearTimeout(+unref); }, 20);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        referenced.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&referenced.stdout),
        String::from_utf8_lossy(&referenced.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&referenced.stdout).trim(),
        "unref-fired\nref-fired"
    );
}

#[test]
fn refreshes_a_timer_deadline_without_changing_its_reference_state() {
    let output = runtime()
        .args([
            "--eval",
            "const timer = setTimeout(() => console.log('fired'), 20); timer.unref().refresh(); console.log(timer.hasRef()); setTimeout(() => timer.ref(), 1);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "false\nfired"
    );
}

#[test]
fn runs_microtasks_between_due_timer_callbacks() {
    let output = runtime()
        .args([
            "--eval",
            "setTimeout(() => { console.log('timer-one'); Promise.resolve().then(() => console.log('microtask')); }, 0); setTimeout(() => console.log('timer-two'), 0);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["timer-one", "microtask", "timer-two"]
    );
}

#[test]
fn runs_next_ticks_before_promise_microtasks_and_microtasks_between_immediates() {
    let output = runtime()
        .args([
            "--eval",
            "process.nextTick(() => console.log('nextTick')); Promise.resolve().then(() => console.log('promise')); setImmediate(() => { console.log('immediate-one'); Promise.resolve().then(() => console.log('immediate-microtask')); }); setImmediate(() => console.log('immediate-two'));",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        [
            "nextTick",
            "promise",
            "immediate-one",
            "immediate-microtask",
            "immediate-two"
        ]
    );
}

#[test]
fn immediates_run_before_timers_scheduled_from_a_timer_callback() {
    let output = runtime()
        .args([
            "--eval",
            "setTimeout(() => { console.log('timer-one'); setTimeout(() => console.log('timer-two'), 0); setImmediate(() => console.log('immediate')); }, 0);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["timer-one", "immediate", "timer-two"]
    );
}

#[test]
fn process_exit_stops_script_and_emits_exit_with_requested_code() {
    let output = runtime()
        .args([
            "--eval",
            "process.on('exit', (code) => console.log('exit:' + code)); console.log('before'); process.exit(7); console.log('after');",
        ])
        .output()
        .expect("runtime process should start");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["before", "exit:7"]
    );
}

#[test]
fn process_exit_cannot_be_caught_and_stops_timer_callbacks() {
    let output = runtime()
        .args([
            "--eval",
            "process.on('exit', () => console.log('exit')); setInterval(() => {}, 100000); try { process.exit(6); } catch { console.log('caught'); } console.log('after');",
        ])
        .output()
        .expect("runtime process should start");

    assert_eq!(output.status.code(), Some(6));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "exit");

    let timer_output = runtime()
        .args([
            "--eval",
            "process.on('exit', () => console.log('exit')); setTimeout(() => { process.exit(8); console.log('after'); }, 0);",
        ])
        .output()
        .expect("runtime process should start");

    assert_eq!(timer_output.status.code(), Some(8));
    assert_eq!(String::from_utf8_lossy(&timer_output.stdout).trim(), "exit");
}

#[test]
fn process_exit_code_is_used_after_normal_event_loop_drain() {
    let output = runtime()
        .args([
            "--eval",
            "process.exitCode = 9; process.on('exit', (code) => console.log('exit:' + code));",
        ])
        .output()
        .expect("runtime process should start");

    assert_eq!(output.status.code(), Some(9));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "exit:9");
}

#[test]
fn delivers_uncaught_synchronous_and_timer_errors_to_process_listeners() {
    let synchronous = runtime()
        .args([
            "--eval",
            "process.on('uncaughtException', (error, origin) => console.log(error.message + ':' + origin)); throw new Error('sync-error'); console.log('unreachable');",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        synchronous.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&synchronous.stdout),
        String::from_utf8_lossy(&synchronous.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&synchronous.stdout).trim(),
        "sync-error:uncaughtException"
    );

    let timer = runtime()
        .args([
            "--eval",
            "process.on('uncaughtException', (error) => console.log(error.message)); setTimeout(() => { throw new Error('timer-error'); }, 0); setTimeout(() => console.log('still-running'), 0);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        timer.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&timer.stdout),
        String::from_utf8_lossy(&timer.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&timer.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["timer-error", "still-running"]
    );
}

#[test]
fn delivers_top_level_esm_rejections_to_uncaught_exception_listeners() {
    let project = TempProject::new();
    let entry = project.0.join("entry.mjs");
    std::fs::write(
        &entry,
        "process.on('uncaughtException', (error) => console.log(error.message)); throw new Error('esm-error');",
    )
    .expect("ES module entry should be written");

    let output = runtime()
        .arg(&entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "esm-error");
}

#[test]
fn reports_unhandled_rejections_created_during_before_exit() {
    let output = runtime()
        .args([
            "--eval",
            "process.on('beforeExit', () => Promise.reject(new Error('beforeExit rejection')));",
        ])
        .output()
        .expect("runtime process should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("beforeExit rejection"));
}

#[test]
fn unhandled_rejections_set_failure_status_and_handled_rejections_do_not() {
    let unhandled = runtime()
        .args(["--eval", "Promise.reject(new Error('unhandled'))"])
        .output()
        .expect("runtime process should start");
    assert_eq!(unhandled.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unhandled.stderr).contains("unhandled"));

    let handled = runtime()
        .args([
            "--eval",
            "Promise.reject(new Error('handled')).catch(() => console.log('caught'));",
        ])
        .output()
        .expect("runtime process should start");
    assert!(handled.status.success());
    assert_eq!(String::from_utf8_lossy(&handled.stdout).trim(), "caught");
}

#[test]
fn emits_rejection_handled_after_a_late_catch() {
    let output = runtime()
        .args([
            "--eval",
            "process.on('unhandledRejection', () => console.log('unhandled')); process.on('rejectionHandled', () => console.log('handled')); const rejection = Promise.reject('late'); setTimeout(() => rejection.catch(() => {}), 1);",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["unhandled", "handled"]
    );
}

#[cfg(unix)]
#[test]
fn dispatches_libuv_signal_to_process_listeners() {
    let mut child = runtime()
        .args([
            "--eval",
            "process.on('SIGUSR1', () => { console.log('signal-received'); process.removeAllListeners('SIGUSR1'); });",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runtime process should start");
    let signal = std::process::Command::new("sh")
        .args(["-c", &format!("sleep 0.1; kill -USR1 {}", child.id())])
        .status()
        .expect("signal helper should start");
    assert!(signal.success(), "signal helper did not send SIGUSR1");
    let output = child
        .wait_with_output()
        .expect("runtime should finish after signal listener removal");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "signal-received"
    );
}

#[test]
fn closing_last_signal_listener_releases_libuv_handle() {
    let output = runtime()
        .args([
            "--eval",
            "process.on('SIGTERM', () => {}); process.removeAllListeners('SIGTERM'); console.log('signal-watch-closed');",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "signal-watch-closed"
    );
}

#[test]
fn before_exit_listener_can_schedule_more_work() {
    let output = runtime()
        .args([
            "--eval",
            "let beforeExitCount = 0; process.on('beforeExit', () => { if (beforeExitCount++ === 0) setTimeout(() => console.log('scheduled'), 1); });",
        ])
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "scheduled");
}

#[test]
fn top_level_await_can_wait_for_async_filesystem_io() {
    let project = TempProject::new();
    let contents_path = project.0.join("contents.txt");
    std::fs::write(&contents_path, "from async io").expect("test input should be written");
    let contents_path = serde_json::to_string(&contents_path.to_string_lossy().as_ref())
        .expect("filesystem path should be encoded");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        format!(
            concat!(
                "import fs from 'node:fs/promises';\n",
                "console.log(await fs.readFile({contents_path}, 'utf8'));\n",
            ),
            contents_path = contents_path,
        ),
    )
    .expect("ES module entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "from async io"
    );
}

#[test]
fn starts_persistent_repl_when_no_script_is_given() {
    let mut child = runtime()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runtime process should start");
    child
        .stdin
        .take()
        .expect("REPL stdin should be available")
        .write_all(b"let replValue = 40;\nreplValue + 2\n.exit\n")
        .expect("REPL input should be written");

    let output = child
        .wait_with_output()
        .expect("runtime should finish after REPL exit");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn services_timers_while_repl_waits_for_input() {
    let mut child = runtime()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runtime process should start");
    let mut stdin = child.stdin.take().expect("REPL stdin should be available");
    stdin
        .write_all(b"setTimeout(() => console.log('timer-fired'), 20);\n")
        .expect("REPL input should be written");
    std::thread::sleep(std::time::Duration::from_millis(100));
    stdin
        .write_all(b".exit\n")
        .expect("REPL exit command should be written");
    drop(stdin);

    let output = child
        .wait_with_output()
        .expect("runtime should finish after REPL exit");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("timer-fired"));
}

#[test]
fn loads_static_esm_dependencies_and_reexports() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.mjs"),
        "export const answer = 41;\nexport default 'esm';\n",
    )
    .expect("ES module dependency should be written");
    std::fs::write(
        project.0.join("reexport.mjs"),
        "export { answer } from './dependency.mjs';\n",
    )
    .expect("ES module re-export should be written");
    std::fs::write(
        project.0.join("common.cjs"),
        "module.exports = 'commonjs';\n",
    )
    .expect("CommonJS dependency should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "import label from './dependency.mjs';\n",
            "import { answer } from './reexport.mjs';\n",
            "import common from './common.cjs';\n",
            "console.log(label + ':' + answer + ':' + common);\n",
        ),
    )
    .expect("ES module entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "esm:41:commonjs"
    );
}

#[test]
fn preserves_live_bindings_across_esm_cycles() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("a.mjs"),
        concat!(
            "import { readA } from './b.mjs';\n",
            "export let value = 'before';\n",
            "export function change() { value = 'after'; }\n",
            "export { readA };\n",
        ),
    )
    .expect("first cyclic ES module should be written");
    std::fs::write(
        project.0.join("b.mjs"),
        concat!(
            "import { value } from './a.mjs';\n",
            "export function readA() { return value; }\n",
        ),
    )
    .expect("second cyclic ES module should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "import { change, readA } from './a.mjs';\n",
            "console.log(readA());\n",
            "change();\n",
            "console.log(readA());\n",
        ),
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["before", "after"]
    );
}

#[test]
fn supports_top_level_await_and_import_meta_url() {
    let project = TempProject::new();
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "await Promise.resolve();\n",
            "console.log(import.meta.url);\n",
            "await new Promise((resolve) => setTimeout(resolve, 1));\n",
            "console.log('settled');\n",
        ),
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(&entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.contains(&format!(
            "file:///{}",
            entry.to_string_lossy().replace('\\', "/")
        )),
        "unexpected import.meta.url output: {output}"
    );
    assert!(output.contains("settled"));
}

#[test]
fn supports_dynamic_import_and_import_meta_resolve() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.mjs"),
        "await Promise.resolve(); export const value = 'loaded';\n",
    )
    .expect("dynamic ES module should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "const dependency = await import('./dependency.mjs');\n",
            "console.log(dependency.value);\n",
            "console.log(import.meta.resolve('./dependency.mjs'));\n",
        ),
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(&entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(output.contains("loaded"));
    assert!(output.contains(&format!(
        "file:///{}",
        project
            .0
            .join("dependency.mjs")
            .to_string_lossy()
            .replace('\\', "/")
    )));
}

#[test]
fn supports_dynamic_import_from_commonjs() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.mjs"),
        "export const value = 'from-esm';\n",
    )
    .expect("dynamic ES module should be written");
    let entry = project.0.join("app.cjs");
    std::fs::write(
        &entry,
        "import('./dependency.mjs').then((module) => console.log(module.value)).catch((error) => console.error(error));\n",
    )
    .expect("CommonJS entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "from-esm",
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_missing_dynamic_imports_through_the_returned_promise() {
    let project = TempProject::new();
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        "import('./missing.mjs').catch((error) => console.log(error.message.includes('Cannot find module')));\n",
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
}

#[test]
fn requires_synchronous_esm_from_commonjs_as_a_module_namespace() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.mjs"),
        "export const answer = 42;\nexport default 'esm';\n",
    )
    .expect("ES module should be written");
    let entry = project.0.join("app.cjs");
    std::fs::write(
        &entry,
        concat!(
            "const esm = require('./dependency.mjs');\n",
            "console.log(esm.default + ':' + esm.answer);\n",
        ),
    )
    .expect("CommonJS entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "esm:42");
}

#[test]
fn rejects_synchronous_require_of_top_level_await_esm() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("async.mjs"),
        "await Promise.resolve(); export const value = 42;\n",
    )
    .expect("async ES module should be written");
    let entry = project.0.join("app.cjs");
    std::fs::write(&entry, "require('./async.mjs');\n").expect("CommonJS entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("top-level await"));
}

#[test]
fn rejects_extensionless_esm_relative_imports() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.mjs"),
        "export const value = 1;\n",
    )
    .expect("ES module dependency should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(&entry, "import './dependency';\n").expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot find module"));
}

#[test]
fn imports_json_with_attributes_and_commonjs_named_exports() {
    let project = TempProject::new();
    std::fs::write(project.0.join("data.json"), r#"{"answer":42}"#)
        .expect("JSON dependency should be written");
    std::fs::write(
        project.0.join("common.cjs"),
        "exports.message = 'from-commonjs';\n",
    )
    .expect("CommonJS dependency should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "import data from './data.json' with { type: 'json' };\n",
            "import { message } from './common.cjs';\n",
            "import { Buffer } from 'node:buffer';\n",
            "console.log(data.answer + ':' + message + ':' + Buffer.from('ok').toString());\n",
        ),
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "42:from-commonjs:ok"
    );
}

#[test]
fn imports_named_commonjs_object_exports() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("common.cjs"),
        "module.exports = { answer: 42, label: 'common' };\n",
    )
    .expect("CommonJS object export should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        "import { answer, label } from './common.cjs'; console.log(answer + ':' + label);\n",
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42:common");
}

#[test]
fn requires_json_import_attributes() {
    let project = TempProject::new();
    std::fs::write(project.0.join("data.json"), r#"{"answer":42}"#)
        .expect("JSON dependency should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        "import data from './data.json'; console.log(data.answer);\n",
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("type: json"));
}

#[test]
fn resolves_package_exports_with_import_and_require_conditions() {
    let project = TempProject::new();
    let package = project.0.join("node_modules").join("conditional-pkg");
    std::fs::create_dir_all(&package).expect("conditional package directory should be created");
    std::fs::write(
        package.join("package.json"),
        concat!(
            "{\"exports\":{\".\":{",
            "\"import\":\"./import.mjs\",",
            "\"require\":\"./require.cjs\"",
            "}}}",
        ),
    )
    .expect("conditional package metadata should be written");
    std::fs::write(
        package.join("import.mjs"),
        "export const value = 'import';\n",
    )
    .expect("import condition module should be written");
    std::fs::write(package.join("require.cjs"), "module.exports = 'require';\n")
        .expect("require condition module should be written");
    std::fs::write(
        project.0.join("consumer.cjs"),
        "module.exports = require('conditional-pkg');\n",
    )
    .expect("CommonJS consumer should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        concat!(
            "import { value } from 'conditional-pkg';\n",
            "import common from './consumer.cjs';\n",
            "console.log(value + ':' + common);\n",
        ),
    )
    .expect("ES module entry should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "import:require"
    );
}

#[test]
fn resolves_package_import_maps_for_native_esm() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("package.json"),
        r##"{"imports":{"#dependency":"./dependency.mjs"}}"##,
    )
    .expect("package import map should be written");
    std::fs::write(
        project.0.join("dependency.mjs"),
        "export const value = 'mapped';\n",
    )
    .expect("mapped ES module should be written");
    let entry = project.0.join("app.mjs");
    std::fs::write(
        &entry,
        "import { value } from '#dependency'; console.log(value);\n",
    )
    .expect("entry ES module should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "mapped");
}

#[test]
fn loads_relative_commonjs_and_json_modules_and_uses_filesystem() {
    let project = TempProject::new();
    std::fs::write(
        project.0.join("dependency.cjs"),
        "exports.message = 'loaded';",
    )
    .expect("CommonJS dependency should be written");
    std::fs::write(project.0.join("data.json"), r#"{"value":7}"#)
        .expect("JSON dependency should be written");
    let entry = project.0.join("app.cjs");
    std::fs::write(
        &entry,
        concat!(
            "const fs = require('fs');\n",
            "const dependency = require('./dependency.cjs');\n",
            "const data = require('./data.json');\n",
            "const output = __dirname + '/output.txt';\n",
            "fs.writeFileSync(output, dependency.message + ':' + data.value);\n",
            "console.log(fs.readFileSync(output, 'utf8'));\n",
        ),
    )
    .expect("entry script should be written");

    let output = runtime()
        .arg(entry)
        .output()
        .expect("runtime process should start");

    assert!(
        output.status.success(),
        "runtime failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "loaded:7");
}

#[test]
fn exposes_basic_process_platform_information() {
    let output = runtime()
        .args([
            "--eval",
            "console.log(process.platform + '/' + process.arch)",
        ])
        .output()
        .expect("runtime process should start");

    assert!(output.status.success());
    let value = String::from_utf8_lossy(&output.stdout);
    assert!(value.trim().contains('/'));
}

#[test]
fn returns_failure_for_invalid_javascript() {
    let output = runtime()
        .args(["--eval", "function ("])
        .output()
        .expect("runtime process should start");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("compilation failed"));
}
