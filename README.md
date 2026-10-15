# Rust Node Runtime

A Rust-hosted JavaScript runtime embedding V8. The executable is named `node` to
make the eventual command-line replacement goal clear.

The compatibility target is **Node.js 24.22.0**. That release embeds V8
13.6.233.17; this project pins the `v8` Rust crate to its 136.0.0 release on
the V8 13.6 line as the starting point for matching JavaScript engine behavior.
Matching V8 alone does not provide Node APIs or guarantee identical behavior.

## Current status

This is an early compatibility implementation, **not a drop-in replacement
for Node.js**. The current implementation includes:

- V8 script and `-e` execution with a CJS-style entry wrapper.
- CommonJS `require`, JSON modules, caching, package lookup, and a partial
  implementation of package `exports` and `imports`.
- Native V8 ES module graph linking and evaluation, including static and dynamic
  imports, re-exports, cycles/live bindings, top-level await, `import.meta.url`
  and `import.meta.resolve`, JSON imports with `type: "json"`, built-in module
  imports, and CommonJS default/named interoperation. Package `import` and
  `require` export conditions are selected separately; CommonJS can
  synchronously `require()` ES module graphs without top-level await.
- Synchronous and libuv-thread-pool-backed asynchronous filesystem operations
  for a subset of common file operations, including callback and Promise APIs;
  a small `path` implementation and a partial `Buffer` implementation.
- Basic `console`, `process.argv`, platform/architecture, current-directory and
  environment helpers, `process.nextTick`, timeout/interval/immediate queues,
  a basic event emitter, minimal stream classes, and a line-oriented persistent
  REPL with expression results and `.help`/`.exit` commands.
- A libuv-backed event-loop wait/work queue with a JavaScript task scheduler
  for next-tick/immediate/timer callbacks; the REPL continues servicing work
  while waiting for input, and ES modules can await filesystem Promises.

The compatibility layers are incomplete and have observable differences from
Node. Native module linking does not yet implement every Node resolution rule,
all import-attribute forms, complete CommonJS named-export detection, or the
entire built-in-module namespace. The runtime now uses libuv for its loop,
timer-based waits, and thread-pool work requests, but it does not yet expose
Node's complete libuv-backed handle and request APIs. Networking, DNS,
filesystem watchers, child processes, signals, complete filesystem semantics,
production-grade streams and workers, npm install behavior, advanced REPL
features, and native addons are not implemented. Timer phase ordering and
error handling also differ from Node. Several built-in module names resolve
but are not implemented. Do not use this runtime as a drop-in Node replacement
yet.

## Build

### Windows prerequisites

Install Rust and Cargo with `rustup`. From PowerShell with `winget` available:

```powershell
winget install --exact --id Rustlang.Rustup
```

Restart the terminal, choose the default stable toolchain if prompted, and
confirm the installation:

```powershell
rustc --version
cargo --version
```

The V8 bindings also need the Microsoft C++ linker and Windows SDK. Install
[Build Tools for Visual Studio 2022](https://visualstudio.microsoft.com/downloads/),
selecting **Desktop development with C++** (including the MSVC x64/x86 build
tools and a Windows 10 or 11 SDK). Restart the terminal after setup. If Cargo
cannot locate the linker, run it from **Developer PowerShell for VS 2022**.
The libuv Rust bindings also generate platform bindings at build time and
require LLVM's `libclang`. Install it with:

```powershell
winget install --exact --id LLVM.LLVM
```

If bindgen cannot find `libclang.dll`, set `LIBCLANG_PATH` to LLVM's `bin`
directory (normally `C:\Program Files\LLVM\bin`) before running Cargo.

### macOS and Linux prerequisites

Install the stable Rust toolchain through the official `rustup` instructions at
[rust-lang.org/tools/install](https://www.rust-lang.org/tools/install), then
install the C++ compiler and linker provided by your platform. On macOS, install
the Xcode Command Line Tools; on Debian/Ubuntu, install `build-essential`.
Install LLVM/libclang as well (for example, `brew install llvm` on macOS or
`apt install libclang-dev` on Debian/Ubuntu), and set `LIBCLANG_PATH` if
bindgen cannot locate its shared library.

### Build

Once Rust/Cargo and the platform C++ toolchain are installed:

```text
cargo build --release
```

The V8 crate fetches platform-specific V8 artifacts during the build. Supported
build targets depend on the artifacts provided by that pinned V8 release.

## Run

```text
cargo run --release -- --version
cargo run --release -- -e "console.log('Hello from V8')"
cargo run --release -- .\app.js
cargo run --release -- --check .\app.js
```

The target is Node.js 24.22.0, across Windows, macOS, and Linux. The V8 binding
version is selected from the V8 13.6 line used by that Node release; this does
not make the Rust runtime ABI-compatible with Node's native addon interfaces.
Compatibility must be verified against Node's test suite and real applications
on all supported operating systems before making a drop-in claim. A
compatibility auditor, reproducible signed deployment artifacts, and an opt-in
production flight recorder remain future features.
