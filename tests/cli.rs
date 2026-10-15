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
