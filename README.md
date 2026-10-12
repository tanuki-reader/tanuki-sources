# tanuki-sources

First-party sources for [Tanuki](https://github.com/tanuki-reader/tanuki), plus the guest-side SDK
they compile against. Each source is a Rust crate built to a `.wasm` bundle. CI validates and signs
the bundles and publishes them as a static repo the app installs from.

| Directory | What |
|---|---|
| `source-api/` | The SDK: the ABI (`abi.rs`), the wire types (`wire.rs`), `export_source!`, and the golden wire fixtures (`fixtures/v<ABI>/`) the app tests against too |
| `sources/<crate>/` | One crate per site. A crate can yield several bundles through cargo features (`mangadex` → `mangadex`, `mangadex-pt-br`) |
| `bundles.toml` | The **published** bundles: id, crate, feature |
| `xtask/` | Build → validate → write and sign the index |

The app ([`tanuki-reader/tanuki`](https://github.com/tanuki-reader/tanuki)) mounts this repo as a
submodule at `native/tanuki-sources` and builds its engine-test fixtures from it.

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
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# rust-toolchain.toml pins the compiler and adds the wasm32-unknown-unknown target on first use.
```

## Writing a source

Implement `Source`, call `export_source!`. Only `meta()` is required; unimplemented exports report
"unsupported" to the app rather than failing to load. See `sources/hello/src/lib.rs` — it's 30 lines
and has no `unsafe`.

All `unsafe` lives in `source-api/src/abi.rs` (~60 lines: the allocator, pointer packing, and the two
host imports). A source never touches it.

To publish it, add a `[[bundle]]` to `bundles.toml`.

### Rules

- **Never panic.** A panic aborts the module and the host sees an opaque trap. Return `AbiError`.
- **Bump `version` in `meta()`** whenever the bundle changes. It's what drives update detection, and
  CI refuses to publish changed bytes under an already-published version. `Cargo.toml` versions are
  not used.
- **`meta().id` must equal the bundle id** in `bundles.toml`. CI checks it.
- **`Wire.kt` and `wire.rs` must agree.** Both test against the golden JSON in
  `source-api/fixtures/v<ABI>/` (`cargo test` here, `WireFixtureTest` in the app's `:core:source`).
  Change a wire shape → update the fixture, then both sides.
- **Listings are the contract's, not yours.** `meta()` declares which of the `Listing` variants
  (`Popular`, `Latest`, `New`) your source serves — you can't invent one or label it. The app owns the
  tab name so it can be translated. Serve none and the app shows your default catalog with no tabs.
- **Only a breaking change bumps the ABI.** A new optional field or export keeps `ABI_VERSION`. A
  breaking one bumps `ABI_VERSION` here and `SourceAbi.VERSION` in the app; the app runs
  `SourceAbi.MIN_SUPPORTED..VERSION` and refuses anything outside it at load time.

## Publishing

The published layout, every field, and what the app checks are specified in
[`docs/repo-format.md`](docs/repo-format.md).

Every push to `main` runs `.github/workflows/publish.yml`. It runs the tests, then
`xtask build --previous <last published>`, which for each bundle in `bundles.toml`:

1. builds it to `wasm32-unknown-unknown`;
2. instantiates it (wasmi), checks the required exports and `abi_version()`, and reads `source_meta`;
3. refuses changed bytes under a version that's already published;
4. writes `wasm/<id>-v<version>.wasm` and an entry in `index.json` (metadata comes from the binary, so
   the index can't drift from it);
5. signs `index.json` with ed25519 (`index.json.sig`, base64) and writes `repo.json` with the public
   key.

The output is force-pushed to the orphan **`repo`** branch and served by GitHub Pages
(`https://tanuki-reader.github.io/tanuki-sources/`), with jsDelivr as a mirror
(`https://cdn.jsdelivr.net/gh/tanuki-reader/tanuki-sources@repo/`). Users add the repo by URL in
the app.

**Signing key:** `cargo run --manifest-path xtask/Cargo.toml -- keygen` prints a seed and its public
key. Store the seed as the `TANUKI_SIGNING_KEY` Actions secret; never commit it. Changing the key makes
every app that pinned the old one reject the repo. **The app also ships this key**
(`RemoteSourceCatalog.PINNED_REPO_KEYS` in the app repo): rotate it only after an app release that carries
the new key, or every install refuses the repo.

The current seed is backed up in the maintainer's macOS Keychain, item **"tanuki-sources signing key"**
(account `tanuki-reader`). To restore it:
`security find-generic-password -a tanuki-reader -s "tanuki-sources signing key" -w | gh secret set TANUKI_SIGNING_KEY`.

### Local repo for the dev loop

```bash
cargo run --manifest-path xtask/Cargo.toml -- build --out out   # unsigned without TANUKI_SIGNING_KEY
python3 -m http.server -d out 8080
```

Add `http://10.0.2.2:8080` (Android emulator) or `http://localhost:8080` (iOS simulator) as a repo in
a debug build of the app.
