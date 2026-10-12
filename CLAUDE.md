# tanuki-sources — Claude Code Context

First-party **WASM sources** for [Tanuki](../App/CLAUDE.md) plus the guest-side SDK they compile
against. Each source is a Rust crate built to a `.wasm` bundle; CI validates and signs the bundles and
publishes them as a static repo the app installs from. The app side (loader, ABI host, Cloudflare
layer) lives in `../App` — read `../App/CLAUDE.md` first for the product, the "Extension System — WASM
only" decision and the "Key Interface — Never Break This" rule. This file covers only this repo.

The app mounts this repo as a git submodule at `native/tanuki-sources` (`.gitmodules` url
`../tanuki-sources`) and builds its engine-test fixtures from it. Locally the two repos sit side by side.

---

## Layout

| Path | What |
|---|---|
| `source-api/` | The SDK. `abi.rs` (the memory ABI, **all** the `unsafe`), `wire.rs` (JSON payloads), `lib.rs` (`Source` trait, `fetch`, `log`, `export_source!`), `util.rs` (shared helpers), `fixtures/v<ABI>/` (golden wire JSON), `tests/fixtures.rs` |
| `sources/<crate>/` | One crate per site. A crate can yield several bundles through cargo features |
| `bundles.toml` | The **published** bundles: `id`, `crate`, optional `feature` |
| `xtask/` | `build` → validate → write and sign the index; `keygen` |
| `docs/repo-format.md` | The published repo layout, every field, and what the app checks |
| `docs/source-candidates.md` | Researched list of sources worth porting (with Keiyoushi references and feasibility) |
| `.github/workflows/publish.yml` | Tests, build, validate, sign, publish to the orphan `repo` branch |

There is **no root Cargo workspace.** Every crate (`source-api`, `xtask`, each `sources/*`) has its own
`Cargo.toml`, `Cargo.lock` and `target/`; run cargo from inside the crate or pass `--manifest-path`.

Current sources: `mangadex` (en + pt-BR bundles), `mangafire` (en + pt-BR), `hipercool` (pt-BR),
`mangapill` (en, HTML scraping), `nexusmangas` (pt-BR, Supabase API).
`hello` (conformance fixture, keep it) and `manhastro` (pt-BR) build but are **not** in `bundles.toml`,
so they aren't published.

---

## Commands

```bash
# Setup: rustup only. rust-toolchain.toml pins the compiler (1.97.1) and adds wasm32-unknown-unknown.
(cd source-api && cargo test --locked)        # wire fixtures; CI runs this
(cd xtask && cargo test --locked)             # CI runs this too

# Build every published bundle, validate it, write the static repo (unsigned without the key)
cargo run --manifest-path xtask/Cargo.toml -- build --out out
cargo run --manifest-path xtask/Cargo.toml -- keygen   # new signing seed + public key

# One source, one language build
(cd sources/mangadex && cargo build --release --target wasm32-unknown-unknown \
   --no-default-features --features lang-pt-br)
```

`xtask build` is the check that matters: it builds each bundle, instantiates it (wasmi), checks the
required exports and `abi_version()`, reads `source_meta`, and refuses changed bytes under an
already-published version. Run it before pushing a source change. `--previous <dir>` is the last
published repo, used for that version gate.

### Testing in the app (local repo)

The app only adds a repo that is **`https://`** and **signed**, so `http://10.0.2.2:8080` and an unsigned
`out/` are both refused. There is no debug bypass. To test without publishing:

```bash
# 1. A throwaway key, never the real one. Keep the seed out of the repo and out of the log.
cargo run -q --manifest-path xtask/Cargo.toml -- keygen > /tmp/tanuki-test-key.txt
export TANUKI_SIGNING_KEY=$(grep TANUKI_SIGNING_KEY /tmp/tanuki-test-key.txt | awk '{print $NF}')
# 2. Signed build, served locally
rm -rf out && cargo run --manifest-path xtask/Cargo.toml -- build --out out
python3 -m http.server -d out 8080
# 3. An HTTPS tunnel to it (brew install cloudflared); prints https://<random>.trycloudflare.com
cloudflared tunnel --url http://localhost:8080
```

Add the `trycloudflare.com` URL in the app (Settings → Sources → Add repo); confirming pins the throwaway
key for that URL only. Then install the source from "Available". Notes:
- Use cloudflared, not ngrok's free tier: ngrok serves a browser-warning HTML page to browser-like user
  agents (the app's client is one), so the app gets HTML instead of `repo.json` and says it can't find a repo.
- After a rebuild, remove and reinstall the source in the app. Chapter lists it already stored aren't
  rewritten, so check changes on a series you haven't opened yet.
- "Show adult content" (Settings) is sent to the source as `allowSensitive`; check both states if the
  source filters.
- The quick-tunnel URL changes every run; remove the old repo from the app when done.

---

## How a source runs (what the code can and can't do)

- A source is a **stateless module in a sandbox with no ambient capabilities**: no sockets, filesystem,
  clock or syscalls. The only way out is `fetch()` and `log()`, supplied by the host. All HTTP goes
  through the app's Cloudflare-aware client, which is the point: a source can't route around it, and it
  never solves Cloudflare itself. Don't try to keep state between calls; re-derive it or fetch it.
- `fetch()` returns non-2xx responses as `Ok`. Check `HttpResponse::status` yourself.
- `Source::meta()` runs on every app launch to build the source list: keep it cheap, **never fetch**.
- Only `meta()` is required. Unimplemented exports report "unsupported" instead of failing the load.
  `home()` is optional; implement it only when the site serves several listings together.
- **Never panic.** A panic aborts the module and the host sees an opaque trap. Return `AbiError`
  (`AbiError::with_code(msg, "parse")`, `AbiError::unsupported(..)`).
- Sources are written in a language that compiles to **linear-memory** WASM (Rust here). Kotlin/Wasm is
  out: WasmGC can't run on the core-WASM interpreters (Chicory on Android, wasmi on iOS) and exports no
  allocator. Details in the README.
- Bundles are downloaded by users, so **size matters**: release profile is `opt-level = "z"`, `lto`,
  `strip`; there's no HTML parser or date crate in the guest. Prefer a JSON API; scrape with string
  slicing. Add a dependency only if it earns its bytes.
- `source-api/src/util.rs` holds helpers shared by sources (`percent_encode`, `query_string`,
  `strip_html`, `parse_datetime_millis`, `first_number`). The bar for adding one is that a **second**
  source needed it. Pure functions only; nothing in it talks to the host.

### Multi-language sites

The ABI gives a source exactly one `lang`. A site that serves several languages (MangaDex, MangaFire)
is **one crate compiled once per language** behind cargo features (`lang-en`, `lang-pt-br`), with a
`mod language` selecting the constants. Each language is a separate `[[bundle]]` with its own id
(`mangadex`, `mangadex-pt-br`). Keep the English id stable so existing libraries aren't re-pointed at
another language. Rules: the build always passes `--no-default-features`; `default` exists only so a
bare `cargo build` and rust-analyzer work; exactly one language feature must be on (add the
`compile_error!` guard, as `mangafire` does). MangaDex tags the language `pt-br`; the ABI wants BCP-47
`pt-BR`.

---

## Rules (CI enforces some of these)

1. **`meta().id` must equal the bundle `id`** in `bundles.toml`. CI checks it.
2. **Bump `version` in `meta()`** whenever the bundle's bytes change. It drives update detection, and CI
   refuses to publish changed bytes under an already-published version. `Cargo.toml` versions are not
   used.
3. **`wire.rs` and the app's `Wire.kt` must agree**, field for field (camelCase, optionals omitted, not
   `null`). Both test against the golden JSON in `source-api/fixtures/v<ABI>/` (`cargo test` here,
   `WireFixtureTest` in the app's `:core:source`). Change a wire shape → update the fixture → change both
   sides in the **same change**, which means a commit here and a submodule bump in the app.
4. **Only a breaking change bumps the ABI.** A new optional field or export keeps `ABI_VERSION` (now `4`,
   `source-api/src/abi.rs`). A breaking one bumps it here **and** `SourceAbi.VERSION` in the app
   (`App/core/source/.../abi/SourceAbi.kt`) together, and adds a new `fixtures/v<N>/`. The app runs
   `SourceAbi.MIN_SUPPORTED..VERSION` and refuses anything outside it at load time.
5. **Listings are the contract's.** `meta()` declares which `Listing` variants (`Popular`, `Latest`,
   `New`) the source serves; it can't invent or label one. The app owns the tab names so they can be
   translated. Serve none and the app shows the default catalog with no tabs.
6. **All `unsafe` stays in `source-api/src/abi.rs`.** A source never writes `unsafe`.
7. **Expose meaning, not display copy.** Chapter/series text comes from the site and stays a plain
   `String`; error messages go to the user via `AbiError` as written, so keep them short and factual.
   Don't try to localize inside the guest.
8. Don't leak engine details (memory layout, host function names) into anything a source author sees
   beyond `abi.rs`. The app keeps `SourceLoader` runtime-agnostic; this SDK keeps the guest side the same.

---

## Adding a source

1. Pick one from `docs/source-candidates.md` (it lists the Keiyoushi folder, e.g. `src/en/mangapill`, to
   read for endpoints, headers and quirks). Check the site's own license/ToS and Keiyoushi's license
   before reusing anything beyond ideas: **reimplement in Rust, don't transliterate their Kotlin.**
2. `cp -r sources/hello sources/<crate>`; set the package name `tanuki-source-<crate>`, keep
   `crate-type = ["cdylib"]` and the release profile.
3. Implement `Source` (start with `meta`, `manga_list`, `manga_details`, `chapter_list`,
   `chapter_pages`), call `export_source!`. Use the existing crates as the pattern: `mangadex` (JSON
   API), `hipercool` (tRPC), `mangafire` (signed token, `vrf.rs`), `manhastro` (JSON API),
   `mangapill` (HTML scraping), `nexusmangas` (Supabase/PostgREST, POST body).
4. Add a `[[bundle]]` to `bundles.toml` (`id` = `meta().id`; add `feature` for per-language builds).
5. Run `xtask build --out out`, then load the bundle in the app via a local repo (see "Testing in the app").
   Test against the real site, including a Cloudflare-protected path if the site has one.
6. Sites that need a login, a WebView-only token or paid access are usually not a fit for a stateless
   guest; note that in `docs/source-candidates.md` rather than half-supporting them.

---

## Publishing & the signing key

Every push to `main` runs `publish.yml`: tests → `xtask build --previous <last published>` → force-push
the output to the orphan **`repo`** branch → GitHub Pages
(`https://tanuki-reader.github.io/tanuki-sources/`), jsDelivr as a mirror. PRs build and validate but
don't publish.

- **Never commit the signing seed.** It's the `TANUKI_SIGNING_KEY` Actions secret
  (base64 ed25519 seed); a backup is in the maintainer's macOS Keychain, item "tanuki-sources signing
  key" (account `tanuki-reader`). Don't read, print or move it.
- The app **pins this repo's public key** (`RemoteSourceCatalog.PINNED_REPO_KEYS` in the app). Rotate the
  key only after an app release carrying the new one, or every install refuses the repo.
- Never rewrite or delete the `repo` branch by hand.

---

## Git

Conventional-commit style, as in the history: `feat(source): …`, `docs: …`, `build: …`, `ci: …`,
`refactor(…): …`. A change that touches the wire or ABI is two coordinated changes (this repo, then the
submodule pointer + `Wire.kt`/`SourceAbi.kt` in the app); say so in the commit message.

---

## What NOT to do

- Don't panic or `unwrap()`/`expect()` on data from the network; return `AbiError`.
- Don't fetch inside `meta()`, and don't keep state between calls.
- Don't add `unsafe` outside `abi.rs`, or a heavy dependency (HTML parser, date/time, regex) for one source.
- Don't change `wire.rs` without the matching `Wire.kt` change and fixture update; don't bump `ABI_VERSION`
  for an additive change.
- Don't publish changed bytes under an unchanged `meta().version`, and don't let `meta().id` drift from
  `bundles.toml`.
- Don't implement Cloudflare solving, cookie jars or retries in a source: the host owns that layer.
- Don't scrape adult-only sites into first-party bundles without a deliberate decision from the maintainer.
- Don't use "Mihon" or "Tachiyomi" branding or package names, and don't copy Keiyoushi extension code
  verbatim (see "Adding a source").
- Don't commit `target/` or `out/` (both are in `.gitignore`).

---

## References

- App context: `../App/CLAUDE.md` (Extension System, Cloudflare Bypass, Key Interface)
- Repo format spec: `docs/repo-format.md`; port candidates: `docs/source-candidates.md`
- ABI host side: `App/core/source/src/commonMain/kotlin/app/tanuki/core/source/abi/` (`SourceAbi.kt`, `Wire.kt`)
- Aidoku (closest WASM source model): https://github.com/Aidoku/Aidoku
- Keiyoushi extension sources (reference for site quirks): https://github.com/keiyoushi/extensions-source
