use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum Resolution {
    Builtin(String),
    File(PathBuf),
}

#[derive(Clone, Copy)]
pub enum ResolutionMode {
    Require,
    Import,
}

pub fn is_es_module(filename: &Path) -> bool {
    match filename
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("mjs") => return true,
        Some("cjs") => return false,
        _ => {}
    }

    let mut directory = filename.parent().unwrap_or_else(|| Path::new("."));
    loop {
        if let Some(package) = read_package_json(directory) {
            return package.get("type").and_then(Value::as_str) == Some("module");
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        if parent == directory {
            break;
        }
        directory = parent;
    }
    false
}

pub fn resolve(specifier: &str, parent: &Path, mode: ResolutionMode) -> Result<Resolution, String> {
    let builtin_name = specifier.strip_prefix("node:").unwrap_or(specifier);
    if is_builtin(builtin_name) {
        return Ok(Resolution::Builtin(builtin_name.to_owned()));
    }

    if specifier.starts_with("node:") {
        return Err(format!("No such built-in module: {specifier}"));
    }

    if specifier.starts_with('#') {
        return resolve_package_import(specifier, parent, mode);
    }

    let request = Path::new(specifier);
    if request.is_absolute() || is_relative(specifier) {
        let target = if request.is_absolute() {
            request.to_path_buf()
        } else {
            parent
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(request)
        };
        let resolved = match mode {
            ResolutionMode::Require => resolve_path(&target, &mut HashSet::new()),
            ResolutionMode::Import => resolve_exact(&target),
        };
        return resolved
            .map(Resolution::File)
            .ok_or_else(|| format!("Cannot find module '{specifier}'"));
    }

    resolve_package(specifier, parent, mode)
}

fn is_relative(specifier: &str) -> bool {
    specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with(".\\")
        || specifier.starts_with("..\\")
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "_http_agent"
            | "_http_client"
            | "_http_common"
            | "_http_incoming"
            | "_http_outgoing"
            | "_http_server"
            | "_stream_duplex"
            | "_stream_passthrough"
            | "_stream_readable"
            | "_stream_transform"
            | "_stream_wrap"
            | "_stream_writable"
            | "_tls_common"
            | "_tls_wrap"
            | "assert"
            | "assert/strict"
            | "async_hooks"
            | "buffer"
            | "child_process"
            | "cluster"
            | "console"
            | "constants"
            | "crypto"
            | "dgram"
            | "diagnostics_channel"
            | "dns"
            | "dns/promises"
            | "domain"
            | "events"
            | "fs"
            | "fs/promises"
            | "http"
            | "https"
            | "http2"
            | "inspector"
            | "inspector/promises"
            | "module"
            | "net"
            | "os"
            | "path"
            | "path/posix"
            | "path/win32"
            | "perf_hooks"
            | "process"
            | "punycode"
            | "querystring"
            | "readline"
            | "readline/promises"
            | "repl"
            | "stream"
            | "stream/consumers"
            | "stream/promises"
            | "stream/web"
            | "string_decoder"
            | "sys"
            | "test"
            | "test/reporters"
            | "timers"
            | "timers/promises"
            | "tls"
            | "trace_events"
            | "tty"
            | "url"
            | "util"
            | "util/types"
            | "v8"
            | "vm"
            | "wasi"
            | "worker_threads"
            | "zlib"
    )
}

fn resolve_package(
    specifier: &str,
    parent: &Path,
    mode: ResolutionMode,
) -> Result<Resolution, String> {
    let (package_name, subpath) = split_package_specifier(specifier)?;
    let mut directory = parent.parent().unwrap_or_else(|| Path::new("."));

    loop {
        let package_dir = directory.join("node_modules").join(package_name);
        if package_dir.is_dir() {
            let package_json = read_package_json(&package_dir);
            if let Some(exports) = package_json.as_ref().and_then(|json| json.get("exports")) {
                if let Some(target) = resolve_exports(exports, subpath, false, mode) {
                    if valid_package_target(&target) {
                        let path = package_dir.join(target);
                        let resolved = match mode {
                            ResolutionMode::Require => resolve_path(&path, &mut HashSet::new()),
                            ResolutionMode::Import => resolve_exact(&path),
                        };
                        if let Some(resolved) = resolved {
                            return Ok(Resolution::File(resolved));
                        }
                    }
                }
                if exports_blocks_subpath(exports, subpath) {
                    return Err(format!(
                        "Package subpath '{subpath}' is not defined by \"exports\" in {}",
                        package_dir.join("package.json").display()
                    ));
                }
            }

            if !subpath.is_empty() {
                let path = package_dir.join(subpath);
                let resolved = match mode {
                    ResolutionMode::Require => resolve_path(&path, &mut HashSet::new()),
                    ResolutionMode::Import => resolve_exact(&path),
                };
                if let Some(resolved) = resolved {
                    return Ok(Resolution::File(resolved));
                }
            } else {
                if let Some(target) = package_json
                    .as_ref()
                    .and_then(|json| json.get("main"))
                    .and_then(Value::as_str)
                {
                    if let Some(resolved) =
                        resolve_path(&package_dir.join(target), &mut HashSet::new())
                    {
                        return Ok(Resolution::File(resolved));
                    }
                }
                if let Some(resolved) = resolve_path(&package_dir, &mut HashSet::new()) {
                    return Ok(Resolution::File(resolved));
                }
            }
        }

        let Some(parent_dir) = directory.parent() else {
            break;
        };
        if parent_dir == directory {
            break;
        }
        directory = parent_dir;
    }

    Err(format!("Cannot find module '{specifier}'"))
}

fn split_package_specifier(specifier: &str) -> Result<(&str, &str), String> {
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or_default();

    if first.is_empty() || first.starts_with('@') && parts.clone().next().is_none() {
        return Err(format!("Invalid module specifier '{specifier}'"));
    }

    if first.starts_with('@') {
        let second = parts
            .next()
            .ok_or_else(|| format!("Invalid scoped package specifier '{specifier}'"))?;
        let package_name = &specifier[..first.len() + 1 + second.len()];
        let remainder = specifier[package_name.len()..].trim_start_matches('/');
        Ok((package_name, remainder))
    } else {
        let remainder = specifier[first.len()..].trim_start_matches('/');
        Ok((first, remainder))
    }
}

fn resolve_package_import(
    specifier: &str,
    parent: &Path,
    mode: ResolutionMode,
) -> Result<Resolution, String> {
    if specifier == "#" || specifier.starts_with("#/") {
        return Err(format!("Invalid package import specifier '{specifier}'"));
    }

    let mut directory = parent.parent().unwrap_or_else(|| Path::new("."));
    loop {
        let package_file = directory.join("package.json");
        if let Some(package) = read_package_json(directory) {
            if let Some(imports) = package.get("imports") {
                if let Some(target) = resolve_exports(imports, specifier, true, mode) {
                    if target.starts_with("./") {
                        if valid_package_target(&target) {
                            let path = directory.join(target);
                            let resolved = match mode {
                                ResolutionMode::Require => resolve_path(&path, &mut HashSet::new()),
                                ResolutionMode::Import => resolve_exact(&path),
                            };
                            if let Some(resolved) = resolved {
                                return Ok(Resolution::File(resolved));
                            }
                        }
                    } else {
                        return resolve_package(&target, &package_file, mode);
                    }
                }
                return Err(format!(
                    "Package import specifier '{specifier}' is not defined in {}",
                    package_file.display()
                ));
            }
        }

        let Some(parent_dir) = directory.parent() else {
            break;
        };
        if parent_dir == directory {
            break;
        }
        directory = parent_dir;
    }

    Err(format!(
        "Package import specifier '{specifier}' is not defined"
    ))
}

fn resolve_exports(
    exports: &Value,
    subpath: &str,
    imports: bool,
    mode: ResolutionMode,
) -> Option<String> {
    let request = if imports {
        subpath.to_owned()
    } else if subpath.is_empty() {
        ".".to_owned()
    } else {
        format!("./{subpath}")
    };

    if let Some(map) = exports.as_object() {
        let has_subpath_keys = map.keys().any(|key| key.starts_with('.'));
        if has_subpath_keys || imports && map.keys().any(|key| key.starts_with('#')) {
            if let Some(value) = map.get(&request) {
                return resolve_export_target(value, "", mode);
            }

            let mut patterns: Vec<_> = map
                .iter()
                .filter_map(|(key, value)| key.find('*').map(|star| (key, value, star)))
                .collect();
            patterns.sort_by_key(|(key, _, _)| std::cmp::Reverse(key.len()));
            for (key, value, star) in patterns {
                let prefix = &key[..star];
                let suffix = &key[star + 1..];
                if request.starts_with(prefix)
                    && request.ends_with(suffix)
                    && request.len() >= prefix.len() + suffix.len()
                {
                    let replacement = &request[prefix.len()..request.len() - suffix.len()];
                    return resolve_export_target(value, replacement, mode);
                }
            }
            None
        } else if request == "." {
            resolve_export_target(exports, "", mode)
        } else {
            None
        }
    } else if request == "." {
        resolve_export_target(exports, "", mode)
    } else {
        None
    }
}

fn resolve_export_target(value: &Value, replacement: &str, mode: ResolutionMode) -> Option<String> {
    match value {
        Value::String(target) => Some(target.replace('*', replacement)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| resolve_export_target(value, replacement, mode)),
        Value::Object(conditions) => {
            let mode_condition = match mode {
                ResolutionMode::Require => "require",
                ResolutionMode::Import => "import",
            };
            for condition in ["node", mode_condition, "default"] {
                if let Some(value) = conditions.get(condition) {
                    if let Some(target) = resolve_export_target(value, replacement, mode) {
                        return Some(target);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn valid_package_target(target: &str) -> bool {
    target.starts_with("./")
        && !Path::new(target)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn exports_blocks_subpath(exports: &Value, subpath: &str) -> bool {
    let request = if subpath.is_empty() {
        ".".to_owned()
    } else {
        format!("./{subpath}")
    };
    exports
        .as_object()
        .map(|map| map.keys().any(|key| key.starts_with('.')) && !map.contains_key(&request))
        .unwrap_or(false)
}

fn read_package_json(directory: &Path) -> Option<Value> {
    let contents = fs::read_to_string(directory.join("package.json")).ok()?;
    serde_json::from_str(&contents).ok()
}

fn resolve_path(path: &Path, visited: &mut HashSet<PathBuf>) -> Option<PathBuf> {
    let normalized = normalize(path);
    if !visited.insert(normalized.clone()) {
        return None;
    }

    if normalized.is_file() {
        return Some(normalized);
    }

    for extension in ["js", "mjs", "json", "node", "cjs"] {
        let candidate = normalized.with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if normalized.is_dir() {
        let package = read_package_json(&normalized);
        if let Some(main) = package
            .as_ref()
            .and_then(|json| json.get("main"))
            .and_then(Value::as_str)
        {
            if let Some(candidate) = resolve_path(&normalized.join(main), visited) {
                return Some(candidate);
            }
        }
        for index in [
            "index.js",
            "index.mjs",
            "index.json",
            "index.node",
            "index.cjs",
        ] {
            let candidate = normalized.join(index);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn resolve_exact(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| normalize(path))
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    if result.is_absolute() {
        result
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&result))
            .unwrap_or(result)
    }
}
