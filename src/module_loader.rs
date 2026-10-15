use crate::resolver::{self, Resolution, ResolutionMode};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use v8::{Global, HandleScope, Local, Module, Value};

const LOADER_SLOT: i32 = 0;

struct ModuleEntry {
    path: PathBuf,
    module: Global<Module>,
    synthetic: Option<SyntheticKind>,
    export_names: Vec<String>,
}

#[derive(Clone)]
enum SyntheticKind {
    CommonJs(PathBuf),
    Builtin(String),
}

pub struct ModuleLoader {
    entries: Vec<ModuleEntry>,
    file_cache: HashMap<PathBuf, usize>,
    synthetic_cache: HashMap<String, usize>,
}

impl ModuleLoader {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            file_cache: HashMap::new(),
            synthetic_cache: HashMap::new(),
        }
    }

    pub fn attach(&mut self, context: Local<v8::Context>) {
        unsafe {
            context.set_aligned_pointer_in_embedder_data(
                LOADER_SLOT,
                (self as *mut ModuleLoader).cast(),
            );
        }
    }

    pub fn detach(&mut self, context: Local<v8::Context>) {
        unsafe {
            context.set_aligned_pointer_in_embedder_data(LOADER_SLOT, std::ptr::null_mut());
        }
    }

    fn insert(
        &mut self,
        scope: &mut HandleScope,
        path: PathBuf,
        module: Local<Module>,
        synthetic: Option<SyntheticKind>,
        export_names: Vec<String>,
    ) -> usize {
        let index = self.entries.len();
        self.entries.push(ModuleEntry {
            path: path.clone(),
            module: Global::new(scope, module),
            synthetic,
            export_names,
        });
        self.file_cache.insert(path, index);
        index
    }

    fn index_of_module(&self, scope: &mut HandleScope, module: Local<Module>) -> Option<usize> {
        self.entries.iter().position(|entry| {
            let cached = Local::new(scope, &entry.module);
            cached == module
        })
    }

    fn compile_file<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        path: &Path,
        source_override: Option<&str>,
    ) -> Result<Local<'s, Module>, String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("could not resolve module '{}': {error}", path.display()))?;
        if let Some(index) = self.file_cache.get(&path) {
            return Ok(Local::new(scope, &self.entries[*index].module));
        }

        let source = match source_override {
            Some(source) => source.to_owned(),
            None => fs::read_to_string(&path)
                .map_err(|error| format!("could not read module '{}': {error}", path.display()))?,
        };
        let filename = path.to_string_lossy();
        let code = v8::String::new(scope, &source)
            .ok_or_else(|| format!("could not allocate module source for {}", path.display()))?;
        let resource_name = v8::String::new(scope, &filename)
            .ok_or_else(|| format!("could not create module origin for {}", path.display()))?;
        let origin = v8::ScriptOrigin::new(
            scope,
            resource_name.into(),
            0,
            0,
            false,
            -1,
            None,
            false,
            false,
            true,
            None,
        );
        let mut compiler_source = v8::script_compiler::Source::new(code, Some(&origin));
        let module = v8::script_compiler::compile_module(scope, &mut compiler_source)
            .ok_or_else(|| format!("could not compile ES module '{}'", path.display()))?;
        self.insert(scope, path, module, None, Vec::new());
        Ok(module)
    }

    fn resolve<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        specifier: Local<v8::String>,
        attributes: Local<v8::FixedArray>,
        referrer: Local<Module>,
    ) -> Result<Local<'s, Module>, String> {
        let parent_index = self
            .index_of_module(scope, referrer)
            .ok_or_else(|| "could not identify the importing ES module".to_owned())?;
        let parent = self.entries[parent_index].path.clone();
        let request = specifier.to_rust_string_lossy(scope);
        let import_attributes = read_attributes(scope, attributes);
        self.resolve_from(scope, &request, &parent, &import_attributes)
    }

    fn resolve_from<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        request: &str,
        parent: &Path,
        import_attributes: &HashMap<String, String>,
    ) -> Result<Local<'s, Module>, String> {
        match resolver::resolve(request, parent, ResolutionMode::Import)? {
            Resolution::Builtin(name) => self.load_builtin(scope, &name),
            Resolution::File(path) => {
                let path = path.canonicalize().map_err(|error| {
                    format!(
                        "could not resolve imported module '{}': {error}",
                        path.display()
                    )
                })?;
                if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                    let kind = import_attributes.get("type").map(String::as_str);
                    if kind != Some("json") {
                        return Err(format!(
                            "Module \"{}\" needs an import attribute of \"type: json\"",
                            path.display()
                        ));
                    }
                    return self.load_json(scope, &path);
                }
                if path.extension().and_then(|extension| extension.to_str()) == Some("cjs")
                    || !resolver::is_es_module(&path)
                {
                    return self.load_commonjs(scope, &path);
                }
                self.compile_file(scope, &path, None)
            }
        }
    }

    fn load_builtin<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        name: &str,
    ) -> Result<Local<'s, Module>, String> {
        let key = format!("builtin:{name}");
        if let Some(index) = self.synthetic_cache.get(&key) {
            return Ok(Local::new(scope, &self.entries[*index].module));
        }

        let builtin_name = new_string(scope, name)?;
        let value = call_global(scope, "__runtimeGetBuiltinForEsm", &[builtin_name.into()])
            .ok_or_else(|| format!("could not load built-in module 'node:{name}'"))?;
        let mut exports = object_export_names(scope, value)?;
        if !exports.iter().any(|name| name == "default") {
            exports.push("default".to_owned());
        }
        let module = create_synthetic(scope, &key, &exports)?;
        let index = self.insert(
            scope,
            PathBuf::from(&key),
            module,
            Some(SyntheticKind::Builtin(name.to_owned())),
            exports,
        );
        self.synthetic_cache.insert(key, index);
        Ok(module)
    }

    fn load_commonjs<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        path: &Path,
    ) -> Result<Local<'s, Module>, String> {
        let path = path.to_path_buf();
        let key = format!("commonjs:{}", path.display());
        if let Some(index) = self.synthetic_cache.get(&key) {
            return Ok(Local::new(scope, &self.entries[*index].module));
        }

        let source = fs::read_to_string(&path).map_err(|error| {
            format!(
                "could not read CommonJS module '{}': {error}",
                path.display()
            )
        })?;
        let mut exports = vec!["default".to_owned()];
        for name in detect_commonjs_exports(&source) {
            if !exports.contains(&name) {
                exports.push(name);
            }
        }
        let module = create_synthetic(scope, &key, &exports)?;
        let index = self.insert(
            scope,
            path,
            module,
            Some(SyntheticKind::CommonJs(PathBuf::from(
                key.strip_prefix("commonjs:").unwrap_or_default(),
            ))),
            exports,
        );
        self.synthetic_cache.insert(key, index);
        Ok(module)
    }

    fn load_json<'s>(
        &mut self,
        scope: &mut HandleScope<'s>,
        path: &Path,
    ) -> Result<Local<'s, Module>, String> {
        let key = format!("json:{}", path.display());
        if let Some(index) = self.synthetic_cache.get(&key) {
            return Ok(Local::new(scope, &self.entries[*index].module));
        }
        let module = create_synthetic(scope, &key, &["default".to_owned()])?;
        let index = self.insert(
            scope,
            path.to_path_buf(),
            module,
            Some(SyntheticKind::CommonJs(path.to_path_buf())),
            vec!["default".to_owned()],
        );
        self.synthetic_cache.insert(key, index);
        Ok(module)
    }
}

pub(crate) unsafe fn execute(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
    filename: &str,
    check_only: bool,
    loader: *mut ModuleLoader,
) -> Result<(), String> {
    let path = PathBuf::from(filename);

    let evaluation = {
        let handle_scope = &mut v8::HandleScope::new(scope);
        let try_catch = &mut v8::TryCatch::new(handle_scope);
        let root = match unsafe { (*loader).compile_file(try_catch, &path, Some(source)) } {
            Ok(module) => module,
            Err(_error) if try_catch.has_caught() => {
                return Err(caught_exception(try_catch, "module compilation", filename))
            }
            Err(error) => return Err(error),
        };
        if check_only {
            None
        } else {
            let instantiated = root.instantiate_module(try_catch, resolve_callback);
            if instantiated != Some(true) {
                return Err(caught_exception(try_catch, "module linking", filename));
            }
            let evaluated = match root.evaluate(try_catch) {
                Some(value) => value,
                None => return Err(caught_exception(try_catch, "module evaluation", filename)),
            };
            v8::Local::<v8::Promise>::try_from(evaluated)
                .ok()
                .map(|promise| Global::new(try_catch, promise))
        }
    };

    let result = if let Some(promise) = evaluation {
        crate::drain_event_loop(scope).and_then(|()| {
            let handle_scope = &mut v8::HandleScope::new(scope);
            let promise = Local::new(handle_scope, &promise);
            match promise.state() {
                v8::PromiseState::Fulfilled => Ok(()),
                v8::PromiseState::Rejected => Err(format!(
                    "JavaScript module evaluation failed in {filename}: {}",
                    promise
                        .result(handle_scope)
                        .to_rust_string_lossy(handle_scope)
                )),
                v8::PromiseState::Pending => Err(format!(
                    "Top-level await in {filename} did not settle before the event loop became idle"
                )),
            }
        })
    } else {
        Ok(())
    };

    result
}

fn resolve_callback<'s>(
    context: Local<'s, v8::Context>,
    specifier: Local<'s, v8::String>,
    attributes: Local<'s, v8::FixedArray>,
    referrer: Local<'s, Module>,
) -> Option<Local<'s, Module>> {
    let pointer = context.get_aligned_pointer_from_embedder_data(LOADER_SLOT);
    if pointer.is_null() {
        return None;
    }
    let mut callback_scope = unsafe { v8::CallbackScope::new(context) };
    let loader = unsafe { &mut *pointer.cast::<ModuleLoader>() };
    match loader.resolve(&mut callback_scope, specifier, attributes, referrer) {
        Ok(module) => Some(module),
        Err(error) => {
            let message = new_string(&mut callback_scope, &error).unwrap();
            let exception = v8::Exception::error(&mut callback_scope, message);
            callback_scope.throw_exception(exception);
            None
        }
    }
}

fn synthetic_evaluation<'s>(
    context: Local<'s, v8::Context>,
    module: Local<'s, Module>,
) -> Option<Local<'s, Value>> {
    let pointer = context.get_aligned_pointer_from_embedder_data(LOADER_SLOT);
    if pointer.is_null() {
        return None;
    }
    let mut callback_scope = unsafe { v8::CallbackScope::new(context) };
    let loader = unsafe { &*pointer.cast::<ModuleLoader>() };
    let (kind, exports) = {
        let index = loader.index_of_module(&mut callback_scope, module)?;
        let entry = &loader.entries[index];
        (entry.synthetic.clone()?, entry.export_names.clone())
    };
    evaluate_synthetic_module(&mut callback_scope, module, kind, exports)
}

fn evaluate_synthetic_module<'s>(
    scope: &mut HandleScope<'s>,
    module: Local<Module>,
    kind: SyntheticKind,
    export_names: Vec<String>,
) -> Option<Local<'s, Value>> {
    let loaded = match kind {
        SyntheticKind::CommonJs(path)
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") =>
        {
            let source = fs::read_to_string(path).ok()?;
            let code = v8::String::new(scope, &source)?;
            v8::json::parse(scope, code)?
        }
        SyntheticKind::CommonJs(path) => {
            let filename = new_string(scope, &path.to_string_lossy()).ok()?;
            call_global(scope, "__runtimeLoadCjsForEsm", &[filename.into()])?
        }
        SyntheticKind::Builtin(name) => {
            let name = new_string(scope, &name).ok()?;
            call_global(scope, "__runtimeGetBuiltinForEsm", &[name.into()])?
        }
    };

    for name in export_names {
        let exported = if name == "default" {
            loaded
        } else {
            let object = v8::Local::<v8::Object>::try_from(loaded).ok()?;
            let key = new_string(scope, &name).ok()?;
            object
                .get(scope, key.into())
                .unwrap_or_else(|| v8::undefined(scope).into())
        };
        let export_name = new_string(scope, &name).ok()?;
        module.set_synthetic_module_export(scope, export_name, exported)?;
    }
    Some(v8::undefined(scope).into())
}

pub fn dynamic_import<'s>(
    scope: &mut HandleScope<'s>,
    _host_defined_options: Local<'s, v8::Data>,
    resource_name: Local<'s, Value>,
    specifier: Local<'s, v8::String>,
    attributes: Local<'s, v8::FixedArray>,
) -> Option<Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);
    let context = scope.get_current_context();
    let pointer = context.get_aligned_pointer_from_embedder_data(LOADER_SLOT);
    if pointer.is_null() {
        let message = v8::String::new(scope, "dynamic import is unavailable in this context")?;
        let error = v8::Exception::error(scope, message);
        let _ = resolver.reject(scope, error);
        return Some(promise);
    }

    let mut parent_name = resource_name.to_rust_string_lossy(scope);
    if parent_name == "undefined" || parent_name == "null" {
        if let Some(active_filename) = call_global(scope, "__runtimeCurrentModuleFilename", &[]) {
            parent_name = active_filename.to_rust_string_lossy(scope);
        }
    }
    let parent = if parent_name == "undefined" || parent_name == "null" {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("__entry__.js")
    } else {
        PathBuf::from(parent_name)
    };
    let request = specifier.to_rust_string_lossy(scope);
    let import_attributes = read_dynamic_attributes(scope, attributes);
    let resolved = {
        let loader = unsafe { &mut *pointer.cast::<ModuleLoader>() };
        loader.resolve_from(scope, &request, &parent, &import_attributes)
    };
    let module = match resolved {
        Ok(module) => module,
        Err(message) => {
            let message = format!("{message} (imported from '{}')", parent.display());
            let error_string = v8::String::new(scope, &message)?;
            let error = v8::Exception::error(scope, error_string);
            let _ = resolver.reject(scope, error);
            return Some(promise);
        }
    };

    if module.get_status() == v8::ModuleStatus::Uninstantiated
        && module.instantiate_module(scope, resolve_callback) != Some(true)
    {
        let message = v8::String::new(scope, "dynamic module linking failed")?;
        let error = v8::Exception::error(scope, message);
        let _ = resolver.reject(scope, error);
        return Some(promise);
    }
    if module.get_status() == v8::ModuleStatus::Errored {
        let _ = resolver.reject(scope, module.get_exception());
        return Some(promise);
    }

    if module.get_status() == v8::ModuleStatus::Evaluated {
        let _ = resolver.resolve(scope, module.get_module_namespace());
        return Some(promise);
    }

    let evaluation = match module.evaluate(scope) {
        Some(value) => v8::Local::<v8::Promise>::try_from(value).ok(),
        None => None,
    };
    let Some(evaluation) = evaluation else {
        let message = v8::String::new(scope, "dynamic module evaluation failed")?;
        let error = v8::Exception::error(scope, message);
        let _ = resolver.reject(scope, error);
        return Some(promise);
    };
    let namespace = Global::new(scope, module.get_module_namespace());
    let namespace = Local::new(scope, &namespace);
    let namespace_callback = v8::Function::builder(dynamic_import_namespace)
        .data(namespace)
        .build(scope)?;
    evaluation.then(scope, namespace_callback)
}

pub fn require_esm_callback(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let filename = args.get(0).to_rust_string_lossy(scope);
    let pointer = scope
        .get_current_context()
        .get_aligned_pointer_from_embedder_data(LOADER_SLOT);
    if pointer.is_null() {
        throw_loader_error(scope, "ES module loader is unavailable");
        return;
    }
    let path = PathBuf::from(&filename);
    let module = match unsafe { (*pointer.cast::<ModuleLoader>()).compile_file(scope, &path, None) }
    {
        Ok(module) => module,
        Err(error) => {
            throw_loader_error(scope, &error);
            return;
        }
    };
    if module.get_status() == v8::ModuleStatus::Uninstantiated
        && module.instantiate_module(scope, resolve_callback) != Some(true)
    {
        throw_loader_error(scope, &format!("could not link ES module '{filename}'"));
        return;
    }
    if module.get_status() == v8::ModuleStatus::Errored {
        scope.throw_exception(module.get_exception());
        return;
    }
    if module.is_graph_async() {
        throw_loader_error(
            scope,
            &format!("Cannot require() an ES module graph with top-level await: {filename}"),
        );
        return;
    }
    if module.get_status() != v8::ModuleStatus::Evaluated {
        let Some(evaluation) = module.evaluate(scope) else {
            throw_loader_error(scope, &format!("could not evaluate ES module '{filename}'"));
            return;
        };
        if let Ok(promise) = v8::Local::<v8::Promise>::try_from(evaluation) {
            match promise.state() {
                v8::PromiseState::Fulfilled => {}
                v8::PromiseState::Rejected => {
                    let reason = promise.result(scope);
                    scope.throw_exception(reason);
                    return;
                }
                v8::PromiseState::Pending => {
                    throw_loader_error(
                        scope,
                        &format!("ES module evaluation did not finish synchronously: {filename}"),
                    );
                    return;
                }
            }
        }
    }
    return_value.set(module.get_module_namespace());
}

fn throw_loader_error(scope: &mut HandleScope, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let exception = v8::Exception::error(scope, message);
        scope.throw_exception(exception);
    }
}

fn dynamic_import_namespace(
    _scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    return_value.set(args.data());
}

fn create_synthetic<'s>(
    scope: &mut HandleScope<'s>,
    name: &str,
    exports: &[String],
) -> Result<Local<'s, Module>, String> {
    let module_name = new_string(scope, name)?;
    let export_names = exports
        .iter()
        .map(|name| new_string(scope, name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Module::create_synthetic_module(
        scope,
        module_name,
        &export_names,
        synthetic_evaluation,
    ))
}

fn object_export_names<'s>(
    scope: &mut HandleScope<'s>,
    value: Local<'s, Value>,
) -> Result<Vec<String>, String> {
    let object = v8::Local::<v8::Object>::try_from(value)
        .map_err(|_| "ES module namespace value must be an object".to_owned())?;
    let names = object
        .get_own_property_names(scope, Default::default())
        .ok_or_else(|| "could not enumerate built-in module exports".to_owned())?;
    let mut exports = Vec::new();
    for index in 0..names.length() {
        if let Some(name) = names.get_index(scope, index) {
            let name = name.to_rust_string_lossy(scope);
            if name != "__esModule" {
                exports.push(name);
            }
        }
    }
    Ok(exports)
}

fn call_global<'s>(
    scope: &mut HandleScope<'s>,
    name: &str,
    args: &[Local<'s, Value>],
) -> Option<Local<'s, Value>> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, name)?;
    let function = v8::Local::<v8::Function>::try_from(global.get(scope, key.into())?).ok()?;
    function.call(scope, global.into(), args)
}

fn new_string<'s>(
    scope: &mut HandleScope<'s>,
    value: &str,
) -> Result<Local<'s, v8::String>, String> {
    v8::String::new(scope, value).ok_or_else(|| format!("could not allocate string '{value}'"))
}

fn read_attributes(
    scope: &mut HandleScope,
    attributes: Local<v8::FixedArray>,
) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut index = 0;
    while index + 1 < attributes.length() {
        let key = attributes
            .get(scope, index)
            .and_then(|value| v8::Local::<v8::String>::try_from(value).ok());
        let value = attributes
            .get(scope, index + 1)
            .and_then(|value| v8::Local::<v8::String>::try_from(value).ok());
        if let (Some(key), Some(value)) = (key, value) {
            result.insert(
                key.to_rust_string_lossy(scope),
                value.to_rust_string_lossy(scope),
            );
        }
        index += 3;
    }
    result
}

fn read_dynamic_attributes(
    scope: &mut HandleScope,
    attributes: Local<v8::FixedArray>,
) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut index = 0;
    while index + 1 < attributes.length() {
        let key = attributes
            .get(scope, index)
            .and_then(|value| v8::Local::<v8::String>::try_from(value).ok());
        let value = attributes
            .get(scope, index + 1)
            .and_then(|value| v8::Local::<v8::String>::try_from(value).ok());
        if let (Some(key), Some(value)) = (key, value) {
            result.insert(
                key.to_rust_string_lossy(scope),
                value.to_rust_string_lossy(scope),
            );
        }
        index += 2;
    }
    result
}

fn detect_commonjs_exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for marker in ["exports.", "module.exports."] {
        let mut remaining = source;
        while let Some(position) = remaining.find(marker) {
            remaining = &remaining[position + marker.len()..];
            let length = remaining
                .char_indices()
                .take_while(|(index, character)| {
                    if *index == 0 {
                        character.is_ascii_alphabetic() || *character == '_' || *character == '$'
                    } else {
                        character.is_ascii_alphanumeric() || *character == '_' || *character == '$'
                    }
                })
                .map(|(index, character)| index + character.len_utf8())
                .last()
                .unwrap_or(0);
            if length > 0 {
                let name = remaining[..length].to_owned();
                if !names.contains(&name) && name != "default" {
                    names.push(name);
                }
            }
        }
    }
    let mut remaining = source;
    while let Some(position) = remaining.find("module.exports") {
        remaining = &remaining[position + "module.exports".len()..];
        let Some(equal) = remaining.find('=') else {
            break;
        };
        remaining = &remaining[equal + 1..];
        let Some(opening) = remaining.find('{') else {
            continue;
        };
        let object = &remaining[opening + 1..];
        for name in object_literal_export_names(object) {
            if !names.contains(&name) && name != "default" {
                names.push(name);
            }
        }
    }
    names
}

fn object_literal_export_names(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    let mut depth = 1usize;
    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == quote {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                index += 1;
            }
            byte if depth == 1 && is_identifier_start(byte) => {
                let start = index;
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| is_identifier_continue(*byte))
                {
                    index += 1;
                }
                let mut next = index;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if bytes.get(next) == Some(&b':') {
                    let name = String::from_utf8_lossy(&bytes[start..index]).into_owned();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            _ => index += 1,
        }
    }
    names
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
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

pub unsafe extern "C" fn initialize_import_meta(
    context: Local<v8::Context>,
    module: Local<Module>,
    meta: Local<v8::Object>,
) {
    let pointer = context.get_aligned_pointer_from_embedder_data(LOADER_SLOT);
    if pointer.is_null() {
        return;
    }
    let mut callback_scope = unsafe { v8::CallbackScope::new(context) };
    let loader = unsafe { &mut *pointer.cast::<ModuleLoader>() };
    if let Some(index) = loader.index_of_module(&mut callback_scope, module) {
        let path = loader.entries[index].path.clone();
        if let Ok(url) = path_to_file_url(&path) {
            if let Ok(url) = new_string(&mut callback_scope, &url) {
                let key = v8::String::new(&mut callback_scope, "url").unwrap();
                let _ = meta.create_data_property(&mut callback_scope, key.into(), url.into());
            }
        }
        if let Ok(filename) = new_string(&mut callback_scope, &path.to_string_lossy()) {
            let resolve = v8::Function::builder(import_meta_resolve)
                .data(filename.into())
                .build(&mut callback_scope);
            if let Some(resolve) = resolve {
                let key = v8::String::new(&mut callback_scope, "resolve").unwrap();
                let _ = meta.create_data_property(&mut callback_scope, key.into(), resolve.into());
            }
        }
    }
}

fn import_meta_resolve(
    scope: &mut HandleScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let specifier = args.get(0).to_rust_string_lossy(scope);
    let parent = PathBuf::from(args.data().to_rust_string_lossy(scope));
    match resolver::resolve(&specifier, &parent, ResolutionMode::Import) {
        Ok(Resolution::Builtin(name)) => {
            if let Some(url) = v8::String::new(scope, &format!("node:{name}")) {
                return_value.set(url.into());
            }
        }
        Ok(Resolution::File(path)) => match path_to_file_url(&path) {
            Ok(url) => {
                if let Some(url) = v8::String::new(scope, &url) {
                    return_value.set(url.into());
                }
            }
            Err(error) => {
                if let Some(message) = v8::String::new(scope, &error) {
                    let exception = v8::Exception::error(scope, message);
                    scope.throw_exception(exception);
                }
            }
        },
        Err(error) => {
            if let Some(message) = v8::String::new(scope, &error) {
                let exception = v8::Exception::error(scope, message);
                scope.throw_exception(exception);
            }
        }
    }
}

fn path_to_file_url(path: &Path) -> Result<String, String> {
    let mut path = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) && path.starts_with("//?/UNC/") {
        path = format!("//{}", &path[8..]);
    } else if cfg!(windows) && path.starts_with("//?/") {
        path.drain(..4);
    }
    let is_unc = cfg!(windows) && path.starts_with("//");
    let path = if cfg!(windows) && !is_unc {
        format!("/{path}")
    } else {
        path
    };
    let mut encoded = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(format!("file://{encoded}"))
}
