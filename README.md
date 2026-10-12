# native/

Rust code, for two unrelated reasons.

| Directory | What | Why Rust |
|---|---|---|
| `wasmi-cabi/` | C-ABI shim over the [wasmi](https://github.com/wasmi-labs/wasmi) interpreter | iOS has no WASM engine and can't JIT; Kotlin/Native can only call C |
| `source-api/` | Guest-side SDK every `.wasm` source compiles against | Guests must be linear-memory WASM (see below) |
| `sources/` | First-party sources, compiled to `.wasm` | same |

`wasmi-cabi` is part of the **host**; `source-api` and `sources` are **guests**. They never
reference each other — they meet at the ABI, over JSON.

## Why sources aren't written in Kotlin

Kotlin/Wasm compiles to **WasmGC**, which needs the garbage-collection and exception-handling
proposals, and puts objects in a GC heap that is a separate memory space from linear memory. Two
consequences, either one fatal here:

1. Chicory and wasmi are core-WASM interpreters — they can't parse a WasmGC module at all. The one
   runtime that can (Wasmtime) is the one iOS forbids, because it JITs.
2. The ABI moves bytes through **linear memory** via `tanuki_alloc`/`tanuki_dealloc`. Kotlin/Wasm
   exports no allocator and offers no way to write one, because in its model there's no pointer to
   return.

So a guest must be a language that compiles to linear-memory WASM: Rust, AssemblyScript, Zig, C.

## Setup

```bash
# once
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown        # for sources
rustup target add aarch64-apple-ios aarch64-apple-ios-sim   # for wasmi-cabi
```

## Building

Sources are built automatically by Gradle — `:shared:buildDevSources` compiles each crate in
`sources/` and copies the `.wasm` into `shared/src/androidMain/resources/sources/`,
where `DevSourceInstaller` picks it up on launch.

The task **skips itself** if cargo isn't installed, so Android-only contributors don't need Rust.

```bash
./gradlew :shared:buildDevSources     # explicit rebuild
cd sources/hello && cargo build --release --target wasm32-unknown-unknown   # manual
```

## Writing a source

Implement `Source`, call `export_source!`. Only `meta()` is required; unimplemented exports report
"unsupported" to the app rather than failing to load. See `sources/hello/src/lib.rs` — it's 30 lines
and has no `unsafe`.

All `unsafe` lives in `source-api/src/abi.rs` (~60 lines: the allocator, pointer packing, and the two
host imports). A source never touches it.

### Rules

- **Never panic.** A panic aborts the module and the host sees an opaque trap. Return `AbiError`.
- **Bump `version` in `meta()`** on every release — it's what drives update detection.
- **`Wire.kt` and `wire.rs` must agree.** Both test against the golden JSON in
  `source-api/fixtures/v<ABI>/` (`cargo test` here, `WireFixtureTest` in `:core:source`). Change a wire
  shape → update the fixture, then both sides, in one commit.
- **Listings are the contract's, not yours.** `meta()` declares which of the `Listing` variants
  (`Popular`, `Latest`) your source serves — you can't invent one or label it. The app owns the tab
  name so it can be translated. Serve none and the app shows your default catalog with no tabs.
- **Only a breaking change bumps the ABI.** A new optional field or export keeps `ABI_VERSION`. A
  breaking one bumps `ABI_VERSION` and `SourceAbi.VERSION` together; the host runs
  `SourceAbi.MIN_SUPPORTED..VERSION` and refuses anything outside it at load time.
