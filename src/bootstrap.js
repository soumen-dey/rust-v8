(() => {
  const moduleCache = Object.create(null);
  let activeModuleFilename = null;
  const nextTicks = [];
  const immediates = [];
  const timers = new Map();
  let nextTimerId = 1;
  let inCheckPhase = false;

  function createRequire(parentFilename, importMode = false) {
    function requireModule(specifier) {
      if (typeof specifier !== "string") {
        throw new TypeError("The \"id\" argument must be of type string");
      }
      const resolved = __runtimeResolve(specifier, parentFilename, importMode);
      if (resolved === null) {
        const error = new Error(`Cannot find module '${specifier}'`);
        error.code = "MODULE_NOT_FOUND";
        throw error;
      }
      if (resolved.startsWith("node:")) {
        return builtinModule(resolved.slice(5));
      }
      if (Object.prototype.hasOwnProperty.call(moduleCache, resolved)) {
        return moduleCache[resolved].exports;
      }
      if (__runtimeIsEsm(resolved)) {
        return __runtimeRequireEsm(resolved);
      }
      if (resolved.endsWith(".node")) {
        const error = new Error(`Native addon loading is not implemented: ${resolved}`);
        error.code = "ERR_DLOPEN_FAILED";
        throw error;
      }

      const source = __runtimeReadFile(resolved);
      if (source === null) {
        const error = new Error(`Cannot find module '${specifier}'`);
        error.code = "MODULE_NOT_FOUND";
        throw error;
      }
      if (resolved.endsWith(".json")) {
        const module = createModule(resolved, parentFilename);
        moduleCache[resolved] = module;
        try {
          module.exports = JSON.parse(source);
          module.loaded = true;
        } catch (error) {
          delete moduleCache[resolved];
          throw error;
        }
        return module.exports;
      }

      const module = createModule(resolved, parentFilename);
      module.require = createRequire(resolved, __runtimeIsEsm(resolved));
      moduleCache[resolved] = module;
      if (module.parent) module.parent.children.push(module);
      try {
        executeCommonJs(module, resolved, source);
      } catch (error) {
        delete moduleCache[resolved];
        throw error;
      }
      return module.exports;
    }

    requireModule.resolve = (specifier) => {
      const result = __runtimeResolve(specifier, parentFilename, importMode);
      if (result === null) {
        const error = new Error(`Cannot find module '${specifier}'`);
        error.code = "MODULE_NOT_FOUND";
        throw error;
      }
      return result;
    };
    process.versions = {
      node: "24.22.0",
      v8: "13.6.233.17",
    };
    process.release = { name: "node", lts: "Krypton" };
    requireModule.cache = moduleCache;
    requireModule.main = moduleCache[mainFilename] || null;
    requireModule.extensions = {
      ".js": (module, filename) => executeCommonJs(module, filename),
      ".cjs": (module, filename) => executeCommonJs(module, filename),
      ".json": (module, filename) => {
        module.exports = JSON.parse(__runtimeReadFile(filename));
      },
    };
    requireModule.createRequire = createRequire;
    return requireModule;
  }

  let mainFilename = null;

  function createModule(filename, parentFilename) {
    const parent = parentFilename ? moduleCache[parentFilename] || null : null;
    return {
      id: parent ? filename : ".",
      filename,
      path: dirname(filename),
      exports: {},
      loaded: false,
      parent,
      children: [],
      paths: [],
      require: null,
    };
  }

  function executeCommonJs(module, filename, originalSource) {
    const source = originalSource === undefined ? __runtimeReadFile(filename) : originalSource;
    if (source === null) throw new Error(`Cannot read module '${filename}'`);
    const wrapper = new Function(
      "exports",
      "require",
      "module",
      "__filename",
      "__dirname",
      `${source}\n//# sourceURL=${filename.replace(/[\r\n]/g, "")}`,
    );
    const previousModuleFilename = activeModuleFilename;
    activeModuleFilename = filename;
    try {
      wrapper.call(
        module.exports,
        module.exports,
        module.require,
        module,
        filename,
        dirname(filename),
      );
    } finally {
      activeModuleFilename = previousModuleFilename;
    }
    module.loaded = true;
  }

  function dirname(filename) {
    const normalized = filename.replace(/\\/g, "/");
    const index = normalized.lastIndexOf("/");
    if (index < 0) return ".";
    if (index === 0) return "/";
    return normalized.slice(0, index).replace(/\//g, filename.includes("\\") ? "\\" : "/");
  }

  function pathModule(windows = process.platform === "win32") {
    const sep = windows ? "\\" : "/";
    const split = (value) => String(value).split(windows ? /[\\/]+/ : /\/+/).filter(Boolean);
    const normalize = (value) => {
      value = String(value);
      const drive = windows ? /^([A-Za-z]:)/.exec(value)?.[1] : "";
      const afterDrive = drive ? value.slice(drive.length) : value;
      const absolute = afterDrive.startsWith("/") || (windows && afterDrive.startsWith("\\"));
      const parts = [];
      const normalized = windows ? afterDrive.replace(/\\/g, "/") : afterDrive;
      for (const part of normalized.split("/")) {
        if (!part || part === ".") continue;
        if (part === ".." && parts.length && parts[parts.length - 1] !== "..") parts.pop();
        else if (part !== ".." || !absolute) parts.push(part);
      }
      const prefix = drive ? `${drive}${absolute ? sep : ""}` : (absolute ? sep : "");
      const result = prefix + parts.join(sep);
      return result || (absolute ? sep : ".");
    };
    const resolve = (...parts) => {
      let output = "";
      for (let i = parts.length - 1; i >= -1; i--) {
        const part = i >= 0 ? String(parts[i]) : process.cwd();
        if (!part) continue;
        output = `${part}${sep}${output}`;
        if (part.startsWith("/") || /^[A-Za-z]:[\\/]/.test(part)) break;
      }
      return normalize(output);
    };
    return {
      sep,
      delimiter: windows ? ";" : ":",
      basename(value, suffix) {
        const base = split(value).pop() || "";
        return suffix && base.endsWith(suffix) ? base.slice(0, -suffix.length) : base;
      },
      dirname(value) {
        value = String(value);
        const normalized = windows ? value.replace(/\//g, "\\") : value;
        let end = normalized.length - 1;
        while (end > 0 && normalized[end] === sep) end--;
        const index = normalized.lastIndexOf(sep, end);
        if (index < 0) return windows && /^[A-Za-z]:/.test(normalized) ? normalized.slice(0, 2) : ".";
        if (index === 0) return sep;
        if (windows && index === 2 && /^[A-Za-z]:/.test(normalized)) return normalized.slice(0, 3);
        return normalized.slice(0, index);
      },
      extname(value) {
        const base = split(value).pop() || "";
        const index = base.lastIndexOf(".");
        return index <= 0 ? "" : base.slice(index);
      },
      isAbsolute(value) {
        return windows
          ? /^[\\/]/.test(value) || /^[A-Za-z]:[\\/]/.test(value)
          : value.startsWith("/");
      },
      join(...parts) {
        return normalize(parts.filter(Boolean).join(sep));
      },
      normalize,
      resolve,
      relative(from, to) {
        const a = split(resolve(from));
        const b = split(resolve(to));
        while (a.length && b.length && (windows ? a[0].toLowerCase() === b[0].toLowerCase() : a[0] === b[0])) {
          a.shift();
          b.shift();
        }
        return [...a.map(() => ".."), ...b].join(sep);
      },
      parse(value) {
        const root = windows
          ? (/^[A-Za-z]:[\\/]/.exec(value)?.[0] || (/^[\\/]/.test(value) ? sep : ""))
          : (value.startsWith("/") ? sep : "");
        const base = this.basename(value);
        const ext = this.extname(base);
        const dir = this.dirname(value);
        return { root, dir: dir === "." ? "" : dir, base, ext, name: base.slice(0, base.length - ext.length) };
      },
      format(value) {
        const dir = value.dir || value.root || "";
        const base = value.base || `${value.name || ""}${value.ext || ""}`;
        if (!dir) return base;
        return dir === value.root ? `${dir}${base}` : `${dir}${sep}${base}`;
      },
      toNamespacedPath(value) {
        return String(value);
      },
    };
  }

  function formDecode(value) {
    return decodeURIComponent(value.replace(/\+/g, " "));
  }

  function formEncode(value) {
    return encodeURIComponent(String(value))
      .replace(/%20/g, "+")
      .replace(/[!'()~]/g, (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`);
  }

  class URLSearchParams {
    constructor(init = "") {
      this._entries = [];
      if (typeof init === "string") {
        for (const pair of init.replace(/^\?/, "").split("&")) {
          if (!pair) continue;
          const separator = pair.indexOf("=");
          this._entries.push([
            formDecode(separator < 0 ? pair : pair.slice(0, separator)),
            formDecode(separator < 0 ? "" : pair.slice(separator + 1)),
          ]);
        }
      } else if (Array.isArray(init) || (init && typeof init[Symbol.iterator] === "function")) {
        for (const pair of init) {
          if (!pair || pair.length !== 2) throw new TypeError("Each query pair must contain exactly two items");
          this._entries.push([String(pair[0]), String(pair[1])]);
        }
      } else if (init && typeof init === "object") {
        for (const key of Object.keys(init)) this._entries.push([key, String(init[key])]);
      } else {
        throw new TypeError("The URLSearchParams initializer must be a string, iterable, or record");
      }
    }
    append(name, value) { this._entries.push([String(name), String(value)]); }
    delete(name, value) {
      name = String(name);
      this._entries = this._entries.filter(([key, item]) => key !== name || (value !== undefined && item !== String(value)));
    }
    get(name) {
      const entry = this._entries.find(([key]) => key === String(name));
      return entry ? entry[1] : null;
    }
    getAll(name) { return this._entries.filter(([key]) => key === String(name)).map(([, value]) => value); }
    has(name, value) {
      return this._entries.some(([key, item]) => key === String(name) && (value === undefined || item === String(value)));
    }
    set(name, value) {
      name = String(name);
      value = String(value);
      const index = this._entries.findIndex(([key]) => key === name);
      if (index < 0) this._entries.push([name, value]);
      else {
        this._entries[index][1] = value;
        this._entries = this._entries.filter((entry, entryIndex) => entry[0] !== name || entryIndex === index);
      }
    }
    sort() {
      this._entries = this._entries.map((entry, index) => ({ entry, index }))
        .sort((a, b) => a.entry[0] < b.entry[0] ? -1 : a.entry[0] > b.entry[0] ? 1 : a.index - b.index)
        .map(({ entry }) => entry);
    }
    forEach(callback, thisArg) {
      for (const [name, value] of [...this._entries]) callback.call(thisArg, value, name, this);
    }
    *entries() { for (const entry of this._entries) yield [...entry]; }
    *keys() { for (const [name] of this._entries) yield name; }
    *values() { for (const [, value] of this._entries) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return this._entries.map(([name, value]) => `${formEncode(name)}=${formEncode(value)}`).join("&"); }
  }

  function normalizeUrlPath(path) {
    const segments = [];
    for (const segment of path.split("/")) {
      if (segment === "." || /^%2e$/i.test(segment)) continue;
      if (segment === ".." || /^(?:\.|%2e)(?:\.|%2e)$/i.test(segment)) segments.pop();
      else segments.push(segment);
    }
    let normalized = segments.join("/");
    if (path.startsWith("/") && !normalized.startsWith("/")) normalized = `/${normalized}`;
    if (path.endsWith("/") && !normalized.endsWith("/")) normalized += "/";
    return normalized || "/";
  }

  function parseUrl(input, base) {
    const source = String(input).trim().replace(/[\t\r\n]/g, "");
    const absoluteMatch = /^([A-Za-z][A-Za-z\d+.-]*:)(\/\/)?([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(source);
    if (!absoluteMatch) {
      if (!base) throw new TypeError(`Invalid URL: ${source}`);
      const parent = parseUrl(base);
      if (source.startsWith("//")) return parseUrl(`${parent.protocol}${source}`);
      if (source.startsWith("#")) return { ...parent, hash: source };
      if (source.startsWith("?")) return { ...parent, search: source, hash: "" };
      if (!parent.hostname) throw new TypeError(`Invalid URL: ${source}`);
      const path = source.startsWith("/") ? source : `${parent.pathname.slice(0, parent.pathname.lastIndexOf("/") + 1)}${source}`;
      return { ...parent, pathname: normalizeUrlPath(path.split(/[?#]/, 1)[0]), search: source.match(/\?[^#]*/)?.[0] || "", hash: source.match(/#.*$/)?.[0] || "" };
    }
    const [, protocolRaw, slashMarker, authorityRaw, pathnameRaw, searchRaw = "", hashRaw = ""] = absoluteMatch;
    const protocol = protocolRaw.toLowerCase();
    const special = /^(https?|wss?|ftp|file):$/.test(protocol);
    if (special && slashMarker !== "//") throw new TypeError(`Invalid URL: ${source}`);
    let username = "";
    let password = "";
    let hostname = "";
    let port = "";
    if (slashMarker === "//") {
      const at = authorityRaw.lastIndexOf("@");
      const authority = at >= 0 ? authorityRaw.slice(at + 1) : authorityRaw;
      if (at >= 0) {
        const auth = authorityRaw.slice(0, at);
        const colon = auth.indexOf(":");
        username = colon < 0 ? auth : auth.slice(0, colon);
        password = colon < 0 ? "" : auth.slice(colon + 1);
      }
      if (authority.startsWith("[")) {
        const end = authority.indexOf("]");
        if (end < 0) throw new TypeError(`Invalid URL: ${source}`);
        hostname = authority.slice(0, end + 1).toLowerCase();
        if (authority[end + 1] === ":") port = authority.slice(end + 2);
        else if (authority.length !== end + 1) throw new TypeError(`Invalid URL: ${source}`);
      } else {
        const colon = authority.lastIndexOf(":");
        hostname = (colon < 0 ? authority : authority.slice(0, colon)).toLowerCase();
        if (colon >= 0) port = authority.slice(colon + 1);
      }
      if (special && protocol !== "file:" && !hostname) throw new TypeError(`Invalid URL: ${source}`);
      if (port && (!/^\d+$/.test(port) || Number(port) > 65535)) throw new TypeError(`Invalid URL: ${source}`);
      if ((protocol === "http:" && port === "80") || (protocol === "https:" && port === "443") ||
          (protocol === "ws:" && port === "80") || (protocol === "wss:" && port === "443") ||
          (protocol === "ftp:" && port === "21")) port = "";
    }
    const pathname = slashMarker === "//" ? normalizeUrlPath(pathnameRaw || "/") : pathnameRaw;
    return { protocol, username, password, hostname, port, pathname, search: searchRaw, hash: hashRaw };
  }

  class URL {
    constructor(input, base) {
      this._state = parseUrl(input, base);
      this.searchParams = new URLSearchParams(this._state.search);
    }
    get href() { return this.toString(); }
    set href(value) { this._state = parseUrl(value); this._syncSearchParams(); }
    get protocol() { return this._state.protocol; }
    set protocol(value) {
      const protocol = String(value).toLowerCase();
      if (/^[a-z][a-z\d+.-]*:$/.test(protocol)) this._state.protocol = protocol;
    }
    get username() { return this._state.username; }
    set username(value) { this._state.username = encodeURIComponent(String(value)); }
    get password() { return this._state.password; }
    set password(value) { this._state.password = encodeURIComponent(String(value)); }
    get hostname() { return this._state.hostname; }
    set hostname(value) { this._state.hostname = String(value).toLowerCase(); }
    get port() { return this._state.port; }
    set port(value) {
      const port = String(value);
      if (!port) { this._state.port = ""; return; }
      if (/^\d+$/.test(port) && Number(port) <= 65535) this._state.port = port;
    }
    get host() { return this._state.hostname + (this._state.port ? `:${this._state.port}` : ""); }
    set host(value) {
      const parsed = parseUrl(`${this.protocol}//${value}/`);
      this._state.hostname = parsed.hostname;
      this._state.port = parsed.port;
    }
    get pathname() { return this._state.pathname; }
    set pathname(value) { this._state.pathname = String(value).startsWith("/") ? String(value) : `/${value}`; }
    get search() { return this._state.search; }
    set search(value) {
      const search = String(value);
      this._state.search = search && search !== "?" ? (search.startsWith("?") ? search : `?${search}`) : "";
      this._syncSearchParams();
    }
    get hash() { return this._state.hash; }
    set hash(value) {
      const hash = String(value);
      this._state.hash = hash && hash !== "#" ? (hash.startsWith("#") ? hash : `#${hash}`) : "";
    }
    get origin() {
      return /^(https?|wss?|ftp):$/.test(this.protocol) ? `${this.protocol}//${this.host}` : "null";
    }
    _syncSearchParams() { this.searchParams = new URLSearchParams(this._state.search); }
    _updateSearch() { this._state.search = this.searchParams.toString() ? `?${this.searchParams}` : ""; }
    toString() {
      const auth = this.username || this.password ? `${this.username}${this.password ? `:${this.password}` : ""}@` : "";
      const authority = this._state.hostname ? `//${auth}${this.host}` : this._state.protocol === "file:" ? "//" : "";
      return `${this.protocol}${authority}${this.pathname}${this.search}${this.hash}`;
    }
    toJSON() { return this.toString(); }
  }
  Object.defineProperty(URL.prototype, "searchParams", {
    configurable: true,
    get() { return this._searchParams; },
    set(value) {
      this._searchParams = value;
      value._onChange = () => this._updateSearch();
    },
  });
  for (const method of ["append", "delete", "set", "sort"]) {
    const original = URLSearchParams.prototype[method];
    URLSearchParams.prototype[method] = function (...args) {
      const result = original.apply(this, args);
      if (this._onChange) this._onChange();
      return result;
    };
  }

  const fsModule = (() => {
    const createFsError = (result, path) => {
      const error = new Error(result.error.message);
      error.code = result.error.code;
      error.errno = result.error.errno;
      error.path = path;
      error.syscall = result.error.syscall;
      if (result.error.dest !== undefined) error.dest = result.error.dest;
      return error;
    };
    const call = (operation, path, data) => {
      const result = JSON.parse(__runtimeFs(operation, path, JSON.stringify(data)));
      if (result.error) throw createFsError(result, path);
      return result.value;
    };
    const asyncCall = (operation, path, data) =>
      __runtimeFsAsync(
        operation,
        path,
        data === undefined ? "" : typeof data === "string" ? data : JSON.stringify(data),
      ).then((encoded) => {
        const result = JSON.parse(encoded);
        if (result.error) throw createFsError(result, path);
        return result.value;
      });
    const fs = {
      readFileSync(path, options) {
        const encoding = typeof options === "string" ? options : options && options.encoding;
        const result = Buffer.from(call("readFile", String(path)));
        return encoding ? result.toString(encoding) : result;
      },
      writeFileSync(path, data) {
        return call("writeFile", String(path), Buffer.from(data).toJSON().data);
      },
      appendFileSync(path, data) {
        return call("appendFile", String(path), Buffer.from(data).toJSON().data);
      },
      existsSync(path) {
        return call("exists", String(path));
      },
      mkdirSync(path, options) {
        return call(options && options.recursive ? "mkdirRecursive" : "mkdir", String(path));
      },
      mkdtempSync(prefix) {
        return call("mkdtemp", String(prefix));
      },
      readdirSync(path) {
        return call("readdir", String(path));
      },
      unlinkSync(path) {
        return call("unlink", String(path));
      },
      renameSync(oldPath, newPath) {
        return call("rename", String(oldPath), String(newPath));
      },
      statSync(path) {
        const stat = call("stat", String(path));
        return createStats(stat);
      },
      lstatSync(path) {
        return createStats(call("lstat", String(path)));
      },
      accessSync(path) {
        return call("access", String(path));
      },
      copyFileSync(source, destination) {
        return call("copyFile", String(source), String(destination));
      },
      linkSync(existingPath, newPath) {
        return call("hardLink", String(existingPath), String(newPath));
      },
      symlinkSync(target, path, type) {
        const operation = type === "dir" ? "symlinkDir" : "symlinkFile";
        return call(operation, String(target), String(path));
      },
      truncateSync(path, length = 0) {
        return call("truncate", String(path), Number(length));
      },
      chmodSync(path, mode) {
        return call("chmod", String(path), Number(mode));
      },
      realpathSync(path) {
        return call("realpath", String(path));
      },
      readlinkSync(path) {
        return call("readlink", String(path));
      },
      rmSync(path, options) {
        return call(options && options.recursive ? "removeDirRecursive" : "removeFile", String(path));
      },
      rmdirSync(path, options) {
        return call(options && options.recursive ? "removeDirRecursive" : "removeDir", String(path));
      },
      promises: {},
    };
    function createStats(stat) {
      return {
        size: stat.size,
        mode: stat.mode,
        isFile: () => stat.file,
        isDirectory: () => stat.directory,
        isSymbolicLink: () => stat.symlink,
        isBlockDevice: () => false,
        isCharacterDevice: () => false,
        isFIFO: () => false,
        isSocket: () => false,
      };
    }
    for (const name of ["readFile", "writeFile", "appendFile", "readdir", "mkdir", "mkdtemp", "unlink", "rename", "stat", "lstat", "access", "copyFile", "link", "symlink", "truncate", "chmod", "realpath", "readlink", "rm", "rmdir"]) {
      fs.promises[name] = (...args) => {
        const path = String(args[0]);
        switch (name) {
          case "readFile": {
            const options = args[1];
            const encoding = typeof options === "string" ? options : options && options.encoding;
            return asyncCall("readFile", path).then((bytes) => {
              const buffer = Buffer.from(bytes);
              return encoding ? buffer.toString(encoding) : buffer;
            });
          }
          case "writeFile":
            return asyncCall("writeFile", path, Buffer.from(args[1]).toJSON().data);
          case "appendFile":
            return asyncCall("appendFile", path, Buffer.from(args[1]).toJSON().data);
          case "mkdir":
            return asyncCall(args[1] && args[1].recursive ? "mkdirRecursive" : "mkdir", path);
          case "mkdtemp":
            return asyncCall("mkdtemp", path);
          case "unlink":
            return asyncCall("unlink", path);
          case "rename":
            return asyncCall("rename", path, String(args[1]));
          case "stat":
          case "lstat":
            return asyncCall(name, path).then(createStats);
          case "access":
            return asyncCall("access", path);
          case "copyFile":
            return asyncCall("copyFile", path, String(args[1]));
          case "link":
            return asyncCall("hardLink", path, String(args[1]));
          case "symlink":
            return asyncCall(args[2] === "dir" ? "symlinkDir" : "symlinkFile", path, String(args[1]));
          case "truncate":
            return asyncCall("truncate", path, Number(args[1] ?? 0));
          case "chmod":
            return asyncCall("chmod", path, Number(args[1]));
          case "realpath":
            return asyncCall("realpath", path);
          case "readlink":
            return asyncCall("readlink", path);
          case "rm":
            return asyncCall(args[1] && args[1].recursive ? "removeDirRecursive" : "removeFile", path);
          case "rmdir":
            return asyncCall(args[1] && args[1].recursive ? "removeDirRecursive" : "removeDir", path);
          case "readdir":
            return asyncCall("readdir", path);
          default:
            return Promise.reject(new Error(`Unsupported filesystem operation '${name}'`));
        }
      };
      fs[name] = (...args) => {
        const callback = args.pop();
        if (typeof callback !== "function") {
          throw new TypeError(`The "${name}" callback must be of type function`);
        }
        fs.promises[name](...args).then(
          (value) => process.nextTick(() => callback(null, value)),
          (error) => process.nextTick(() => callback(error)),
        );
      };
    }
    fs.exists = (path, callback) => {
      if (typeof callback !== "function") throw new TypeError("The callback must be of type function");
      asyncCall("exists", String(path)).then(
        (exists) => process.nextTick(() => callback(exists)),
        () => process.nextTick(() => callback(false)),
      );
    };
    return fs;
  })();

  function encodeUtf8(value) {
    const encoded = encodeURIComponent(String(value));
    const bytes = [];
    for (let index = 0; index < encoded.length; index++) {
      if (encoded[index] === "%") {
        bytes.push(parseInt(encoded.slice(index + 1, index + 3), 16));
        index += 2;
      } else {
        bytes.push(encoded.charCodeAt(index));
      }
    }
    return bytes;
  }

  function encodeUtf16(value) {
    const bytes = [];
    for (let index = 0; index < value.length; index++) {
      const code = value.charCodeAt(index);
      bytes.push(code & 255, code >>> 8);
    }
    return bytes;
  }

  function decodeUtf8(bytes) {
    let encoded = "";
    for (const byte of bytes) encoded += `%${byte.toString(16).padStart(2, "0")}`;
    try {
      return decodeURIComponent(encoded);
    } catch {
      return String.fromCharCode(...bytes);
    }
  }

  function encodeBase64(bytes) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let output = "";
    for (let index = 0; index < bytes.length; index += 3) {
      const a = bytes[index];
      const b = bytes[index + 1];
      const c = bytes[index + 2];
      output += alphabet[a >> 2];
      output += alphabet[((a & 3) << 4) | ((b || 0) >> 4)];
      output += b === undefined ? "=" : alphabet[((b & 15) << 2) | ((c || 0) >> 6)];
      output += c === undefined ? "=" : alphabet[c & 63];
    }
    return output;
  }

  function decodeBase64(value) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const clean = String(value).replace(/-/g, "+").replace(/_/g, "/").replace(/[^A-Za-z0-9+/]/g, "");
    const bytes = [];
    let bits = 0;
    let accumulator = 0;
    for (const char of clean) {
      accumulator = (accumulator << 6) | alphabet.indexOf(char);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes.push((accumulator >> bits) & 255);
        accumulator &= (1 << bits) - 1;
      }
    }
    return bytes;
  }

  class Buffer extends Uint8Array {
    static from(value, encoding = "utf8", length) {
      if (typeof value === "string") {
        encoding = normalizeBufferEncoding(encoding);
        if (encoding === "hex") {
          const bytes = [];
          for (let i = 0; i + 1 < value.length; i += 2) {
            const byte = value.slice(i, i + 2);
            if (!/^[\da-f]{2}$/i.test(byte)) break;
            bytes.push(parseInt(byte, 16));
          }
          return new Buffer(bytes);
        }
        if (encoding === "base64" || encoding === "base64url") return new Buffer(decodeBase64(value));
        if (encoding === "ascii" || encoding === "latin1" || encoding === "binary") {
          return new Buffer(Array.from(value, (char) => char.charCodeAt(0) & 255));
        }
        if (encoding === "ucs2") return new Buffer(encodeUtf16(value));
        return new Buffer(encodeUtf8(value));
      }
      if (value instanceof ArrayBuffer) {
        const offset = encoding === undefined ? 0 : Number(encoding);
        const byteLength = length === undefined ? value.byteLength - offset : Number(length);
        if (!Number.isInteger(offset) || !Number.isInteger(byteLength) || offset < 0 || byteLength < 0 || offset + byteLength > value.byteLength) {
          throw new RangeError("The byteOffset and length must be within the bounds of the buffer");
        }
        return new Buffer(value, offset, byteLength);
      }
      if (ArrayBuffer.isView(value)) {
        if (value instanceof DataView) return new Buffer(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
        return new Buffer(value);
      }
      if (Array.isArray(value)) return new Buffer(value);
      if (value && Array.isArray(value.data)) return new Buffer(value.data);
      throw new TypeError("The first argument must be a string, Buffer, ArrayBuffer, or array");
    }
    static alloc(size, fill = 0, encoding = "utf8") {
      size = Number(size);
      if (!Number.isInteger(size) || size < 0 || size > 0x7fffffff) throw new RangeError("The value of \"size\" is out of range");
      const buffer = new Buffer(size);
      if (typeof fill === "string") {
        const bytes = Buffer.from(fill, encoding);
        if (bytes.length) for (let index = 0; index < buffer.length; index++) buffer[index] = bytes[index % bytes.length];
      } else if (fill !== 0) {
        buffer.fill(fill);
      }
      return buffer;
    }
    static allocUnsafe(size) {
      return Buffer.alloc(size);
    }
    static byteLength(value, encoding = "utf8") {
      return Buffer.from(value, encoding).length;
    }
    static concat(list, totalLength) {
      const buffers = list.map((item) => Buffer.from(item));
      const length = totalLength === undefined ? buffers.reduce((sum, item) => sum + item.length, 0) : totalLength;
      const output = new Buffer(length);
      let offset = 0;
      for (const item of buffers) {
        output.set(item.subarray(0, Math.max(0, length - offset)), offset);
        offset += item.length;
        if (offset >= length) break;
      }
      return output;
    }
    static isBuffer(value) {
      return value instanceof Buffer;
    }
    static isEncoding(encoding) {
      try { normalizeBufferEncoding(encoding); return true; }
      catch { return false; }
    }
    toString(encoding = "utf8", start = 0, end = this.length) {
      encoding = normalizeBufferEncoding(encoding);
      start = Math.trunc(Number(start) || 0);
      end = end === undefined ? this.length : Math.trunc(Number(end) || 0);
      const bytes = this.subarray(start, end);
      if (encoding === "hex") return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
      if (encoding === "base64" || encoding === "base64url") {
        const result = encodeBase64(bytes);
        return encoding === "base64url" ? result.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") : result;
      }
      if (encoding === "ascii") return Array.from(bytes, (byte) => String.fromCharCode(byte & 127)).join("");
      if (encoding === "latin1" || encoding === "binary") return Array.from(bytes, (byte) => String.fromCharCode(byte)).join("");
      if (encoding === "ucs2") {
        let output = "";
        for (let index = 0; index + 1 < bytes.length; index += 2) output += String.fromCharCode(bytes[index] | (bytes[index + 1] << 8));
        return output;
      }
      return decodeUtf8(bytes);
    }
    equals(other) {
      const bytes = Buffer.from(other);
      return this.length === bytes.length && this.every((byte, index) => byte === bytes[index]);
    }
    includes(value, byteOffset = 0, encoding = "utf8") {
      return this.indexOf(value, byteOffset, encoding) !== -1;
    }
    indexOf(value, byteOffset = 0, encoding = "utf8") {
      const needle = typeof value === "number" ? [value & 255] : Buffer.from(value, encoding);
      for (let index = Math.max(0, byteOffset); index <= this.length - needle.length; index++) {
        if (needle.every((byte, offset) => this[index + offset] === byte)) return index;
      }
      return -1;
    }
    write(value, offset = 0, encoding = "utf8") {
      encoding = normalizeBufferEncoding(encoding);
      const bytes = Buffer.from(value, encoding);
      offset = Number(offset);
      if (!Number.isInteger(offset) || offset < 0 || offset > this.length) throw new RangeError("The offset is out of range");
      const count = Math.min(bytes.length, this.length - offset);
      this.set(bytes.subarray(0, count), offset);
      return count;
    }
    toJSON() {
      return { type: "Buffer", data: Array.from(this) };
    }
  }

  function normalizeBufferEncoding(encoding) {
    encoding = String(encoding).toLowerCase();
    const aliases = { utf8: "utf8", "utf-8": "utf8", hex: "hex", base64: "base64", base64url: "base64url", ascii: "ascii", latin1: "latin1", binary: "latin1", ucs2: "ucs2", "ucs-2": "ucs2", utf16le: "ucs2", "utf-16le": "ucs2" };
    const normalized = aliases[encoding];
    if (!normalized) throw new TypeError(`Unknown encoding: ${encoding}`);
    return normalized;
  }

  globalThis.Buffer = Buffer;
  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;

  function fileURLToPath(value) {
    const url = value instanceof URL ? value : new URL(value);
    if (url.protocol !== "file:") throw new TypeError("The URL must be of scheme file");
    if (url.hostname && url.hostname !== "localhost") throw new TypeError("File URL host must be empty or localhost");
    let pathname = decodeURIComponent(url.pathname);
    if (process.platform === "win32") {
      pathname = pathname.replace(/^\/([A-Za-z]:)/, "$1").replace(/\//g, "\\");
    }
    return pathname;
  }

  function pathToFileURL(value) {
    let pathname = pathModule().resolve(String(value)).replace(/\\/g, "/");
    if (/^[A-Za-z]:/.test(pathname)) pathname = `/${pathname}`;
    const encoded = pathname.split("/").map((segment) => encodeURIComponent(segment).replace(/%3A/gi, ":")).join("/");
    return new URL(`file://${encoded.startsWith("/") ? "" : "/"}${encoded}`);
  }

  const urlModule = { URL, URLSearchParams, fileURLToPath, pathToFileURL };

  function builtinModule(name) {
    switch (name) {
      case "path":
      case "path/posix":
        return pathModule(false);
      case "path/win32":
        return pathModule(true);
      case "fs":
        return fsModule;
      case "fs/promises":
        return fsModule.promises;
      case "buffer":
        return { Buffer, SlowBuffer: Buffer, constants: { MAX_LENGTH: 0x7fffffff, MAX_STRING_LENGTH: 0x1fffffe8 } };
      case "url":
        return urlModule;
      case "process":
        return process;
      case "console":
        return console;
      case "assert":
      case "assert/strict":
        return assertModule;
      case "util":
        return utilModule;
      case "os":
        return {
          platform: () => process.platform,
          arch: () => process.arch,
          endianness: () => "LE",
          EOL: process.platform === "win32" ? "\r\n" : "\n",
          cpus: () => [],
          totalmem: () => 0,
          freemem: () => 0,
          homedir: () => process.env.HOME || process.env.USERPROFILE || "",
          tmpdir: () => process.env.TMPDIR || process.env.TEMP || "/tmp",
          hostname: () => process.env.HOSTNAME || "",
          type: () => process.platform === "win32" ? "Windows_NT" : process.platform,
          release: () => "",
        };
      case "module":
        return { createRequire: (filename) => createRequire(filename) };
      case "timers":
        return { setTimeout, clearTimeout, setInterval, clearInterval, setImmediate, clearImmediate };
      case "stream":
        return { Readable, Writable, Duplex, Transform, PassThrough, finished, pipeline };
      case "stream/promises":
        return {
          finished: (stream) => new Promise((resolve, reject) => finished(stream, (error) => error ? reject(error) : resolve())),
          pipeline: (...items) => new Promise((resolve, reject) => pipeline(...items, (error) => error ? reject(error) : resolve())),
        };
      case "events":
        return eventsModule;
      case "string_decoder":
        return {
          StringDecoder: class StringDecoder {
            constructor(encoding = "utf8") { this.encoding = encoding; }
            write(buffer) { return Buffer.from(buffer).toString(this.encoding); }
            end(buffer) { return buffer === undefined ? "" : this.write(buffer); }
          },
        };
      default:
        throw new Error(`Built-in module '${name}' is not implemented`);
    }
  }

  class EventEmitter {
    constructor() {
      this._events = Object.create(null);
      this._maxListeners = undefined;
    }
    on(name, listener) {
      if (typeof listener !== "function") throw new TypeError("The listener must be of type function");
      if (name !== "newListener" && this.listenerCount("newListener")) {
        this.emit("newListener", name, listener.listener || listener);
      }
      (this._events[name] ||= []).push(listener);
      return this;
    }
    addListener(name, listener) {
      return this.on(name, listener);
    }
    once(name, listener) {
      const wrapped = (...args) => {
        this.removeListener(name, wrapped);
        listener.apply(this, args);
      };
      wrapped.listener = listener;
      return this.on(name, wrapped);
    }
    emit(name, ...args) {
      const listeners = this._events[name];
      if (!listeners || !listeners.length) {
        if (name === "error") throw args[0] instanceof Error ? args[0] : new Error(String(args[0]));
        return false;
      }
      for (const listener of [...listeners]) listener.apply(this, args);
      return true;
    }
    removeListener(name, listener) {
      const listeners = this._events[name];
      if (!listeners) return this;
      for (let index = listeners.length - 1; index >= 0; index--) {
        const item = listeners[index];
        if (item === listener || item.listener === listener) {
          listeners.splice(index, 1);
          if (!listeners.length) delete this._events[name];
          if (name !== "removeListener") this.emit("removeListener", name, item.listener || item);
          break;
        }
      }
      return this;
    }
    off(name, listener) {
      return this.removeListener(name, listener);
    }
    removeAllListeners(name) {
      if (name !== undefined) {
        for (const listener of this.rawListeners(name).reverse()) this.removeListener(name, listener);
        return this;
      }
      if (!this.listenerCount("removeListener")) {
        this._events = Object.create(null);
        return this;
      }
      for (const eventName of this.eventNames()) {
        if (eventName !== "removeListener") this.removeAllListeners(eventName);
      }
      this.removeAllListeners("removeListener");
      return this;
    }
    listeners(name) {
      return (this._events[name] || []).map((listener) => listener.listener || listener);
    }
    rawListeners(name) {
      return [...(this._events[name] || [])];
    }
    listenerCount(name) {
      return (this._events[name] || []).length;
    }
    setMaxListeners(count) {
      count = Number(count);
      if (!Number.isInteger(count) || count < 0) throw new RangeError("The value of \"n\" is out of range");
      this._maxListeners = count;
      return this;
    }
    getMaxListeners() {
      return this._maxListeners === undefined ? EventEmitter.defaultMaxListeners : this._maxListeners;
    }
    eventNames() {
      return Reflect.ownKeys(this._events);
    }
  }

  process._events = Object.create(null);
  Object.setPrototypeOf(process, EventEmitter.prototype);
  const signalNumbers = process.platform === "win32"
    ? { SIGINT: 2, SIGILL: 4, SIGFPE: 8, SIGSEGV: 11, SIGTERM: 15, SIGBREAK: 21, SIGABRT: 22 }
    : process.platform === "darwin"
      ? {
          SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5, SIGABRT: 6,
          SIGEMT: 7, SIGFPE: 8, SIGBUS: 10, SIGSEGV: 11, SIGSYS: 12, SIGPIPE: 13,
          SIGALRM: 14, SIGTERM: 15, SIGURG: 16, SIGTSTP: 18, SIGCONT: 19, SIGCHLD: 20,
          SIGTTIN: 21, SIGTTOU: 22, SIGIO: 23, SIGXCPU: 24, SIGXFSZ: 25, SIGVTALRM: 26,
          SIGPROF: 27, SIGWINCH: 28, SIGINFO: 29, SIGUSR1: 30, SIGUSR2: 31,
        }
      : {
          SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5, SIGABRT: 6,
          SIGBUS: 7, SIGFPE: 8, SIGUSR1: 10, SIGSEGV: 11, SIGUSR2: 12, SIGPIPE: 13,
          SIGALRM: 14, SIGTERM: 15, SIGSTKFLT: 16, SIGCHLD: 17, SIGCONT: 18,
          SIGTSTP: 20, SIGTTIN: 21, SIGTTOU: 22, SIGURG: 23, SIGXCPU: 24,
          SIGXFSZ: 25, SIGVTALRM: 26, SIGPROF: 27, SIGWINCH: 28, SIGIO: 29,
          SIGPWR: 30, SIGSYS: 31,
        };
  const watchSignalIfNeeded = (name) => {
    if (Object.prototype.hasOwnProperty.call(signalNumbers, name)) {
      __runtimeWatchSignal(name, signalNumbers[name], process.listenerCount(name) > 0);
    }
  };
  for (const method of ["on", "once"]) {
    const original = EventEmitter.prototype[method];
    if (!original) continue;
    process[method] = function (name, listener) {
      const result = original.call(this, name, listener);
      watchSignalIfNeeded(String(name));
      return result;
    };
  }
  for (const method of ["removeListener", "off"]) {
    process[method] = function (name, listener) {
      const result = EventEmitter.prototype.removeListener.call(this, name, listener);
      watchSignalIfNeeded(String(name));
      return result;
    };
  }
  process.removeAllListeners = function (name) {
    if (name === undefined) {
      EventEmitter.prototype.removeAllListeners.call(this);
      for (const signal of Object.keys(signalNumbers)) watchSignalIfNeeded(signal);
      return this;
    }
    const result = EventEmitter.prototype.removeAllListeners.call(this, name);
    watchSignalIfNeeded(String(name));
    return result;
  };
  process.exitCode = 0;
  let processExitEmitted = false;
  const emitProcessExit = () => {
    if (processExitEmitted) return;
    processExitEmitted = true;
    process.emit("exit", Number(process.exitCode) || 0);
  };
  process.exit = (code = process.exitCode) => {
    const value = Number(code);
    if (!Number.isFinite(value)) throw new TypeError("The exit code must be a finite number");
    process.exitCode = ((Math.trunc(value) % 256) + 256) % 256;
    try {
      emitProcessExit();
    } finally {
      __runtimeRequestExit(Number(process.exitCode) || 0);
    }
  };

  class Readable extends EventEmitter {
    constructor(options = {}) {
      super();
      this.readable = true;
      this.readableFlowing = null;
      this._buffer = [];
      this._ended = false;
      this._read = options.read || (() => {});
      this._objectMode = Boolean(options.objectMode);
    }
    push(chunk) {
      if (chunk === null) {
        this._ended = true;
        if (this.readableFlowing) process.nextTick(() => this.emit("end"));
        return false;
      }
      const value = this._objectMode || Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      if (this.readableFlowing) process.nextTick(() => this.emit("data", value));
      else this._buffer.push(value);
      return true;
    }
    read() {
      if (this._buffer.length) return this._buffer.shift();
      if (this._ended) return null;
      this._read();
      return this._buffer.shift() || null;
    }
    on(name, listener) {
      super.on(name, listener);
      if (name === "data") {
        this.readableFlowing = true;
        for (const chunk of this._buffer.splice(0)) process.nextTick(() => this.emit("data", chunk));
        if (this._ended) process.nextTick(() => this.emit("end"));
      }
      return this;
    }
    pause() {
      this.readableFlowing = false;
      return this;
    }
    resume() {
      this.readableFlowing = true;
      for (const chunk of this._buffer.splice(0)) this.emit("data", chunk);
      return this;
    }
    pipe(destination) {
      this.on("data", (chunk) => {
        if (destination.write(chunk) === false) this.pause();
      });
      destination.on("drain", () => this.resume());
      this.on("end", () => destination.end());
      this.on("error", (error) => destination.emit("error", error));
      return destination;
    }
    destroy(error) {
      this.readable = false;
      this._buffer.length = 0;
      if (error) this.emit("error", error);
      process.nextTick(() => this.emit("close"));
      return this;
    }
    static from(iterable, options = {}) {
      const stream = new Readable(options);
      process.nextTick(async () => {
        try {
          for await (const value of iterable) stream.push(value);
          stream.push(null);
        } catch (error) {
          stream.destroy(error);
        }
      });
      return stream;
    }
  }

  class Writable extends EventEmitter {
    constructor(options = {}) {
      super();
      this.writable = true;
      this.writableEnded = false;
      this._writeImpl = options.write || ((_chunk, _encoding, callback) => callback());
      this._finalImpl = options.final || ((callback) => callback());
    }
    write(chunk, encoding, callback) {
      if (typeof encoding === "function") {
        callback = encoding;
        encoding = undefined;
      }
      if (this.writableEnded) throw new Error("write after end");
      this._writeImpl(chunk, encoding || "buffer", (error) => {
        if (error) this.emit("error", error);
        if (callback) callback(error || null);
      });
      return true;
    }
    end(chunk, encoding, callback) {
      if (typeof chunk === "function") {
        callback = chunk;
        chunk = undefined;
      } else if (typeof encoding === "function") {
        callback = encoding;
        encoding = undefined;
      }
      if (chunk !== undefined) this.write(chunk, encoding);
      this.writableEnded = true;
      this._finalImpl((error) => {
        if (error) this.emit("error", error);
        else this.emit("finish");
        if (callback) callback(error || null);
      });
      return this;
    }
    destroy(error) {
      this.writable = false;
      if (error) this.emit("error", error);
      process.nextTick(() => this.emit("close"));
      return this;
    }
  }

  class Duplex extends Readable {
    constructor(options = {}) {
      super(options);
      this._writeImpl = options.write || ((_chunk, _encoding, callback) => callback());
      this._finalImpl = options.final || ((callback) => callback());
      this.writable = true;
      this.writableEnded = false;
    }
    write(...args) {
      return Writable.prototype.write.apply(this, args);
    }
    end(...args) {
      return Writable.prototype.end.apply(this, args);
    }
  }

  class Transform extends Duplex {
    constructor(options = {}) {
      super(options);
      this._transformImpl = options.transform || ((_chunk, _encoding, callback) => callback());
    }
    write(chunk, encoding, callback) {
      if (typeof encoding === "function") {
        callback = encoding;
        encoding = undefined;
      }
      this._transformImpl(chunk, encoding || "buffer", (error, value) => {
        if (error) this.emit("error", error);
        else if (value !== undefined && value !== null) this.push(value);
        if (callback) callback(error || null);
      });
      return true;
    }
  }

  class PassThrough extends Transform {
    constructor(options = {}) {
      super({
        ...options,
        transform: options.transform || ((chunk, _encoding, callback) => callback(null, chunk)),
      });
    }
  }

  function finished(stream, callback) {
    let settled = false;
    const done = (error) => {
      if (settled) return;
      settled = true;
      if (callback) callback(error);
    };
    stream.once("error", done);
    stream.once("finish", () => done());
    stream.once("end", () => done());
    return () => {
      settled = true;
      stream.removeListener("error", done);
    };
  }

  function pipeline(...items) {
    const callback = typeof items[items.length - 1] === "function" ? items.pop() : null;
    const destination = items[items.length - 1];
    try {
      for (let index = 1; index < items.length; index++) items[index - 1].pipe(items[index]);
      if (callback) finished(destination, callback);
    } catch (error) {
      if (callback) callback(error);
      else throw error;
    }
    return destination;
  }

  class AssertionError extends Error {
    constructor(message = "The expression evaluated to a falsy value") {
      super(message);
      this.name = "AssertionError";
      this.code = "ERR_ASSERTION";
    }
  }

  function assertModule(value, message) {
    if (!value) throw new AssertionError(message);
  }
  assertModule.ok = assertModule;
  assertModule.fail = (message) => { throw new AssertionError(message || "Failed"); };
  assertModule.equal = (actual, expected, message) => {
    if (actual != expected) throw new AssertionError(message || `${actual} == ${expected}`);
  };
  assertModule.notEqual = (actual, expected, message) => {
    if (actual == expected) throw new AssertionError(message || `${actual} != ${expected}`);
  };
  assertModule.strictEqual = (actual, expected, message) => {
    if (!Object.is(actual, expected)) throw new AssertionError(message || `${actual} === ${expected}`);
  };
  assertModule.notStrictEqual = (actual, expected, message) => {
    if (Object.is(actual, expected)) throw new AssertionError(message || `${actual} !== ${expected}`);
  };
  assertModule.deepEqual = assertModule.deepStrictEqual = (actual, expected, message) => {
    if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new AssertionError(message || "Values are not deeply equal");
  };
  assertModule.notDeepEqual = assertModule.notDeepStrictEqual = (actual, expected, message) => {
    if (JSON.stringify(actual) === JSON.stringify(expected)) throw new AssertionError(message || "Values are deeply equal");
  };
  assertModule.throws = (callback, expected, message) => {
    let thrown;
    try { callback(); } catch (error) { thrown = error; }
    if (!thrown) throw new AssertionError(message || "Missing expected exception");
    if (expected instanceof RegExp && !expected.test(String(thrown))) throw new AssertionError(message || "Exception did not match");
    return thrown;
  };
  assertModule.rejects = async (callback, expected, message) => {
    try { await (typeof callback === "function" ? callback() : callback); }
    catch (error) {
      if (expected instanceof RegExp && !expected.test(String(error))) throw new AssertionError(message || "Rejection did not match");
      return error;
    }
    throw new AssertionError(message || "Missing expected rejection");
  };
  assertModule.AssertionError = AssertionError;

  const utilModule = {
    format(format, ...args) {
      if (typeof format !== "string") return [format, ...args].map((value) => typeof value === "string" ? value : JSON.stringify(value)).join(" ");
      let index = 0;
      const output = format.replace(/%[sdifjoO%]/g, (token) => {
        if (token === "%%") return "%";
        if (index >= args.length) return token;
        const value = args[index++];
        if (token === "%s") return String(value);
        if (token === "%d" || token === "%i" || token === "%f") return String(Number(value));
        if (token === "%j" || token === "%o" || token === "%O") {
          try { return JSON.stringify(value); } catch { return "[Circular]"; }
        }
        return token;
      });
      return [output, ...args.slice(index).map((value) => typeof value === "string" ? value : JSON.stringify(value))].join(" ");
    },
    inspect(value) {
      try { return JSON.stringify(value, null, 2); } catch { return String(value); }
    },
    promisify(callback) {
      return (...args) => new Promise((resolve, reject) => {
        callback(...args, (error, value) => error ? reject(error) : resolve(value));
      });
    },
    callbackify(callback) {
      return (...args) => {
        const done = args.pop();
        callback(...args).then((value) => done(null, value), done);
      };
    },
    types: {
      isBuffer: (value) => Buffer.isBuffer(value),
      isArrayBuffer: (value) => value instanceof ArrayBuffer,
      isDate: (value) => value instanceof Date,
      isRegExp: (value) => value instanceof RegExp,
      isPromise: (value) => value instanceof Promise,
    },
  };

  const eventsModule = EventEmitter;
  eventsModule.EventEmitter = EventEmitter;
  eventsModule.defaultMaxListeners = 10;
  eventsModule.once = (emitter, name) => new Promise((resolve, reject) => {
    const onError = (error) => {
      emitter.removeListener(name, onEvent);
      reject(error);
    };
    const onEvent = (...args) => {
      emitter.removeListener("error", onError);
      resolve(args);
    };
    emitter.once(name, onEvent);
    if (name !== "error") emitter.once("error", onError);
  });
  EventEmitter.EventEmitter = EventEmitter;
  EventEmitter.once = eventsModule.once;
  EventEmitter.defaultMaxListeners = 10;
  EventEmitter.listenerCount = (emitter, name) => emitter.listenerCount(name);
  EventEmitter.prototype.prependListener = function (name, listener) {
    if (typeof listener !== "function") throw new TypeError("The listener must be of type function");
    if (name !== "newListener" && this.listenerCount("newListener")) {
      this.emit("newListener", name, listener.listener || listener);
    }
    (this._events[name] ||= []).unshift(listener);
    return this;
  };
  EventEmitter.prototype.prependOnceListener = function (name, listener) {
    if (typeof listener !== "function") throw new TypeError("The listener must be of type function");
    if (name !== "newListener" && this.listenerCount("newListener")) {
      this.emit("newListener", name, listener);
    }
    const wrapped = (...args) => {
      this.removeListener(name, wrapped);
      listener.apply(this, args);
    };
    wrapped.listener = listener;
    (this._events[name] ||= []).unshift(wrapped);
    return this;
  };
  for (const method of ["prependListener", "prependOnceListener"]) {
    const original = EventEmitter.prototype[method];
    process[method] = function (name, listener) {
      const result = original.call(this, name, listener);
      watchSignalIfNeeded(String(name));
      return result;
    };
  }
  globalThis.require = createRequire(null);
  globalThis.__runtimeRunMain = (filename) => {
    mainFilename = filename;
    const module = createModule(filename, null);
    module.id = ".";
    module.require = createRequire(filename, false);
    moduleCache[filename] = module;
    moduleCache[filename] = module;
    module.require.main = module;
    const source = __runtimeReadFile(filename);
    if (source === null) throw new Error(`Cannot read entry file '${filename}'`);
    const wrapper = new Function(
      "exports",
      "require",
      "module",
      "__filename",
      "__dirname",
      `${source}\n//# sourceURL=${filename.replace(/[\r\n]/g, "")}`,
    );
    const previousModuleFilename = activeModuleFilename;
    activeModuleFilename = filename;
    try {
      wrapper.call(
        module.exports,
        module.exports,
        module.require,
        module,
        filename,
        dirname(filename),
      );
    } finally {
      activeModuleFilename = previousModuleFilename;
    }
    module.loaded = true;
  };
  globalThis.__runtimeLoadCjsForEsm = (filename) => createRequire(filename, false)(filename);
  globalThis.__runtimeGetBuiltinForEsm = (name) => builtinModule(name);
  globalThis.__runtimeCurrentModuleFilename = () => activeModuleFilename;

  const processNextTick = [];
  process.nextTick = (callback, ...args) => {
    if (typeof callback !== "function") throw new TypeError("callback must be a function");
    processNextTick.push(() => callback(...args));
  };
  process.cwd = () => JSON.parse(__runtimeProcess("cwd", ""));
  process.chdir = (path) => {
    const result = JSON.parse(__runtimeProcess("chdir", String(path)));
    if (result.error) {
      const error = new Error(result.error.message);
      error.code = result.error.code;
      throw error;
    }
  };
  process.env = new Proxy(Object.create(null), {
    get(_target, key) {
      return typeof key === "string" ? JSON.parse(__runtimeProcess("getenv", key)) ?? undefined : undefined;
    },
    set(_target, key, value) {
      __runtimeProcess("setenv", String(key), String(value));
      return true;
    },
    ownKeys() {
      return JSON.parse(__runtimeProcess("envkeys", ""));
    },
    getOwnPropertyDescriptor(_target, key) {
      const value = typeof key === "string" ? JSON.parse(__runtimeProcess("getenv", key)) ?? undefined : undefined;
      return value === undefined ? undefined : { configurable: true, enumerable: true, value, writable: true };
    },
  });

  globalThis.setTimeout = (callback, delay = 1, ...args) => {
    if (typeof callback !== "function") throw new TypeError("The callback argument must be of type function");
    delay = Math.min(2_147_483_647, Math.max(1, Number(delay) || 1));
    const timer = { id: nextTimerId++, callback, args, delay, interval: 0, active: true };
    timers.set(timer.id, timer);
    __runtimeScheduleTimer(timer.id, delay, 0);
    return createTimerHandle(timer);
  };
  globalThis.clearTimeout = (timer) => {
    const handle = typeof timer === "number" ? timers.get(timer) : timer;
    if (!handle || !handle.active) return;
    handle.active = false;
    timers.delete(handle.id);
    __runtimeCancelTimer(handle.id);
  };
  globalThis.setInterval = (callback, delay = 1, ...args) => {
    if (typeof callback !== "function") throw new TypeError("The callback argument must be of type function");
    delay = Math.min(2_147_483_647, Math.max(1, Number(delay) || 1));
    const timer = { id: nextTimerId++, callback, args, delay, interval: delay, active: true };
    timers.set(timer.id, timer);
    __runtimeScheduleTimer(timer.id, delay, delay);
    return createTimerHandle(timer);
  };
  globalThis.clearInterval = globalThis.clearTimeout;
  const createTimerHandle = (timer) => {
    timer.ref = () => {
      if (timer.active) __runtimeUpdateTimer(timer.id, "ref");
      return timer;
    };
    timer.unref = () => {
      if (timer.active) __runtimeUpdateTimer(timer.id, "unref");
      return timer;
    };
    timer.hasRef = () => timer.active && __runtimeUpdateTimer(timer.id, "hasRef");
    timer.refresh = () => {
      if (timer.active) __runtimeUpdateTimer(timer.id, "refresh");
      return timer;
    };
    timer.close = () => {
      globalThis.clearTimeout(timer);
      return timer;
    };
    timer[Symbol.toPrimitive] = () => timer.id;
    return timer;
  };
  globalThis.setImmediate = (callback, ...args) => {
    const immediate = { id: nextTimerId++, callback, args, active: true };
    immediates.push(immediate);
    return immediate;
  };
  globalThis.clearImmediate = (timer) => { if (timer) timer.active = false; };
  globalThis.queueMicrotask ||= (callback) => Promise.resolve().then(callback);

  const drainNextTicks = () => {
    while (processNextTick.length) processNextTick.shift()();
  };
  globalThis.__runtimeDrainNextTicks = drainNextTicks;
  const hasPendingWork = () =>
    processNextTick.length > 0 ||
    immediates.some((immediate) => immediate.active) ||
    __runtimeHasPendingAsync();
  globalThis.__runtimeHasPendingWork = hasPendingWork;
  const dispatchAsyncEvent = (event) => {
    if (event.timer !== undefined) {
      const timer = timers.get(event.timer);
      if (!timer || !timer.active) return;
      if (!timer.interval) {
        timer.active = false;
        timers.delete(timer.id);
      }
      timer.callback(...timer.args);
    } else if (event.signal !== undefined) {
      process.emit(event.signal);
    } else {
      throw new Error(`Unexpected libuv event: ${JSON.stringify(event)}`);
    }
    drainNextTicks();
  };
  globalThis.__runtimeRunEventLoopTurn = (maxWait = 0, waitWhenIdle = false) => {
    drainNextTicks();
    if (inCheckPhase) {
      if (__runtimeHasQueuedAsyncEvents()) {
        const queuedEvents = JSON.parse(__runtimePumpAsync(0));
        if (queuedEvents.length) {
          dispatchAsyncEvent(queuedEvents[0]);
        }
        return hasPendingWork();
      }

      while (immediates.length) {
        const immediate = immediates.shift();
        if (!immediate.active) continue;
        immediate.callback(...immediate.args);
        immediate.active = false;
        drainNextTicks();
        return hasPendingWork();
      }
      inCheckPhase = false;
    }

    const readyEvents = JSON.parse(__runtimePumpAsync(0));
    inCheckPhase = true;
    if (readyEvents.length) {
      dispatchAsyncEvent(readyEvents[0]);
      return hasPendingWork();
    }

    while (immediates.length) {
      const immediate = immediates.shift();
      if (!immediate.active) continue;
      immediate.callback(...immediate.args);
      immediate.active = false;
      drainNextTicks();
      return hasPendingWork();
    }

    let pending = hasPendingWork();
    if (!pending && !waitWhenIdle) return false;

    const events = JSON.parse(__runtimePumpAsync(Math.max(0, Number(maxWait) || 0)));
    inCheckPhase = true;
    if (events.length) {
      dispatchAsyncEvent(events[0]);
    }

    pending = hasPendingWork();
    return pending;
  };
  globalThis.__runtimeEmitBeforeExit = () => {
    if (process.listenerCount("beforeExit")) process.emit("beforeExit", Number(process.exitCode) || 0);
    return hasPendingWork();
  };
  globalThis.__runtimeDispatchUnhandledRejections = () => {
    for (const [promise, reason] of __runtimeTakeUnhandledRejections()) {
      if (!process.emit("unhandledRejection", reason, promise)) {
        console.error(reason && reason.stack || reason);
        if (!process.exitCode) process.exitCode = 1;
      }
    }
    for (const promise of __runtimeTakeHandledRejections()) {
      process.emit("rejectionHandled", promise);
    }
  };
  globalThis.__runtimeHandleUncaughtException = (error, origin) => {
    if (!process.listenerCount("uncaughtException")) return false;
    process.emit("uncaughtException", error, origin);
    return true;
  };
  globalThis.__runtimeFinalizeProcess = () => {
    emitProcessExit();
    return Number(process.exitCode) || 0;
  };
})();
