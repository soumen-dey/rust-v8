(() => {
  const moduleCache = Object.create(null);
  let activeModuleFilename = null;
  const nextTicks = [];
  const immediates = [];
  const timers = [];
  let nextTimerId = 1;

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
    const split = (value) => String(value).split(/[\\/]+/).filter(Boolean);
    const normalize = (value) => {
      const absolute = value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value);
      const parts = [];
      for (const part of value.replace(/\\/g, "/").split("/")) {
        if (!part || part === ".") continue;
        if (part === ".." && parts.length && parts[parts.length - 1] !== "..") parts.pop();
        else if (part !== ".." || !absolute) parts.push(part);
      }
      const prefix = /^[A-Za-z]:/.exec(value)?.[0] || (absolute ? sep : "");
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
        const parts = split(value);
        parts.pop();
        if (!parts.length) return value.startsWith("/") ? "/" : ".";
        return (value.startsWith("/") ? sep : "") + parts.join(sep);
      },
      extname(value) {
        const base = split(value).pop() || "";
        const index = base.lastIndexOf(".");
        return index <= 0 ? "" : base.slice(index);
      },
      isAbsolute(value) {
        return value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value);
      },
      join(...parts) {
        return normalize(parts.filter(Boolean).join(sep));
      },
      normalize,
      resolve,
      relative(from, to) {
        const a = split(resolve(from));
        const b = split(resolve(to));
        while (a.length && b.length && a[0].toLowerCase() === b[0].toLowerCase()) {
          a.shift();
          b.shift();
        }
        return [...a.map(() => ".."), ...b].join(sep);
      },
      parse(value) {
        const root = value.startsWith("/") ? sep : "";
        const base = this.basename(value);
        const ext = this.extname(base);
        return { root, dir: this.dirname(value), base, ext, name: base.slice(0, base.length - ext.length) };
      },
      format(value) {
        return `${value.dir || value.root || ""}${value.dir || value.root ? sep : ""}${value.name || ""}${value.ext || ""}`;
      },
      toNamespacedPath(value) {
        return String(value);
      },
    };
  }

  const fsModule = (() => {
    const call = (operation, path, data) => {
      const result = JSON.parse(__runtimeFs(operation, path, JSON.stringify(data)));
      if (result.error) {
        const error = new Error(result.error.message);
        error.code = result.error.code;
        error.path = path;
        throw error;
      }
      return result.value;
    };
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
    for (const name of ["readFile", "writeFile", "appendFile", "readdir", "mkdir", "unlink", "rename", "stat", "lstat", "access", "copyFile", "truncate", "chmod", "realpath", "readlink", "rm", "rmdir"]) {
      const syncName = `${name}Sync`;
      fs.promises[name] = (...args) => Promise.resolve().then(() => fs[syncName](...args));
      fs[name] = (...args) => {
        const callback = args.pop();
        if (typeof callback !== "function") {
          throw new TypeError(`The "${name}" callback must be of type function`);
        }
        fs.promises[name](...args).then(
          (value) => callback(null, value),
          (error) => callback(error),
        );
      };
    }
    fs.exists = (path, callback) => {
      if (typeof callback !== "function") throw new TypeError("The callback must be of type function");
      Promise.resolve().then(() => callback(fs.existsSync(path)));
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
    const clean = String(value).replace(/[^A-Za-z0-9+/]/g, "");
    const bytes = [];
    let bits = 0;
    let accumulator = 0;
    for (const char of clean) {
      accumulator = (accumulator << 6) | alphabet.indexOf(char);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes.push((accumulator >> bits) & 255);
      }
    }
    return bytes;
  }

  class Buffer extends Uint8Array {
    static from(value, encoding = "utf8") {
      if (typeof value === "string") {
        if (encoding === "hex") {
          const bytes = [];
          for (let i = 0; i + 1 < value.length; i += 2) bytes.push(parseInt(value.slice(i, i + 2), 16) || 0);
          return new Buffer(bytes);
        }
        if (encoding === "base64" || encoding === "base64url") return new Buffer(decodeBase64(value));
        if (encoding === "ascii" || encoding === "latin1" || encoding === "binary") {
          return new Buffer(Array.from(value, (char) => char.charCodeAt(0) & 255));
        }
        return new Buffer(encodeUtf8(value));
      }
      if (ArrayBuffer.isView(value)) return new Buffer(value);
      if (value instanceof ArrayBuffer) return new Buffer(new Uint8Array(value));
      if (Array.isArray(value)) return new Buffer(value);
      if (value && Array.isArray(value.data)) return new Buffer(value.data);
      throw new TypeError("The first argument must be a string, Buffer, ArrayBuffer, or array");
    }
    static alloc(size, fill = 0, encoding = "utf8") {
      const buffer = new Buffer(Math.max(0, Number(size) | 0));
      if (fill !== 0) buffer.fill(typeof fill === "string" ? Buffer.from(fill, encoding)[0] || 0 : fill);
      return buffer;
    }
    static allocUnsafe(size) {
      return new Buffer(Math.max(0, Number(size) | 0));
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
    toString(encoding = "utf8", start = 0, end = this.length) {
      const bytes = this.subarray(Math.max(0, start), Math.min(this.length, end));
      if (encoding === "hex") return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
      if (encoding === "base64" || encoding === "base64url") {
        const result = encodeBase64(bytes);
        return encoding === "base64url" ? result.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") : result;
      }
      if (encoding === "ascii") return Array.from(bytes, (byte) => String.fromCharCode(byte & 127)).join("");
      if (encoding === "latin1" || encoding === "binary") return Array.from(bytes, (byte) => String.fromCharCode(byte)).join("");
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
      const bytes = Buffer.from(value, encoding);
      const count = Math.min(bytes.length, this.length - offset);
      this.set(bytes.subarray(0, count), offset);
      return count;
    }
    toJSON() {
      return { type: "Buffer", data: Array.from(this) };
    }
  }

  globalThis.Buffer = Buffer;

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
        return { Buffer, SlowBuffer: Buffer };
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
    }
    on(name, listener) {
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
      if (listeners) this._events[name] = listeners.filter((item) => item !== listener && item.listener !== listener);
      return this;
    }
    off(name, listener) {
      return this.removeListener(name, listener);
    }
    removeAllListeners(name) {
      if (name === undefined) this._events = Object.create(null);
      else delete this._events[name];
      return this;
    }
    listeners(name) {
      return (this._events[name] || []).map((listener) => listener.listener || listener);
    }
    listenerCount(name) {
      return (this._events[name] || []).length;
    }
    setMaxListeners() {
      return this;
    }
    getMaxListeners() {
      return 10;
    }
    eventNames() {
      return Reflect.ownKeys(this._events);
    }
  }

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
    (this._events[name] ||= []).unshift(listener);
    return this;
  };
  EventEmitter.prototype.prependOnceListener = function (name, listener) {
    const wrapped = (...args) => {
      this.removeListener(name, wrapped);
      listener.apply(this, args);
    };
    wrapped.listener = listener;
    (this._events[name] ||= []).unshift(wrapped);
    return this;
  };
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

  globalThis.setTimeout = (callback, delay = 0, ...args) => {
    const timer = { id: nextTimerId++, callback, args, due: Date.now() + Math.max(0, Number(delay) || 0), interval: 0, active: true };
    timers.push(timer);
    return timer;
  };
  globalThis.clearTimeout = (timer) => { if (timer) timer.active = false; };
  globalThis.setInterval = (callback, delay = 1, ...args) => {
    const timer = globalThis.setTimeout(callback, delay, ...args);
    timer.interval = Math.max(1, Number(delay) || 1);
    return timer;
  };
  globalThis.clearInterval = globalThis.clearTimeout;
  globalThis.setImmediate = (callback, ...args) => {
    const immediate = { id: nextTimerId++, callback, args, active: true };
    immediates.push(immediate);
    return immediate;
  };
  globalThis.clearImmediate = (timer) => { if (timer) timer.active = false; };
  globalThis.queueMicrotask ||= (callback) => Promise.resolve().then(callback);

  globalThis.__runtimeDrainTasks = () => {
    while (processNextTick.length || immediates.length || timers.some((timer) => timer.active)) {
      while (processNextTick.length) processNextTick.shift()();
      while (immediates.length) {
        const immediate = immediates.shift();
        if (immediate.active) immediate.callback(...immediate.args);
        while (processNextTick.length) processNextTick.shift()();
      }
      let next = null;
      for (const timer of timers) {
        if (timer.active && (!next || timer.due < next.due)) next = timer;
      }
      if (next) {
        const wait = Math.max(0, next.due - Date.now());
        if (wait) __runtimeSleep(wait);
        if (next.active) next.callback(...next.args);
        if (next.interval && next.active) next.due = Date.now() + next.interval;
        else next.active = false;
        while (processNextTick.length) processNextTick.shift()();
      }
    }
  };
})();
