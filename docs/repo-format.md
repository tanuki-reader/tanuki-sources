# Source repo format (v1)

What a Tanuki source repo serves, and what the app checks before it trusts any of it. This is the
format `xtask build` writes and `.github/workflows/publish.yml` publishes to the `repo` branch. Any
static host that serves these files works as a repo: GitHub Pages, jsDelivr, R2, S3 or a plain web
server.

The live first-party repo, at `https://tanuki-reader.github.io/tanuki-sources/` (mirror:
`https://cdn.jsdelivr.net/gh/tanuki-reader/tanuki-sources@repo/`), is the reference instance.

## Layout

Everything sits under one **base URL**, which must be `https://` and end in `/`:

```
repo.json              who the repo is and the key it signs with
index.json             what it offers (the signed payload)
index.json.sig         ed25519 signature over the exact bytes of index.json
wasm/<id>-v<version>.wasm
.nojekyll              GitHub Pages only: serve files as-is
```

Bundle file names carry their version, so a published file never changes. That's what makes CDN
caching safe.

## `repo.json`

```json
{
  "formatVersion": 1,
  "name": "Tanuki Sources",
  "publicKey": "T+bZfLVGYzHZ0g7lDWQMUdgXkKD8eDLtmH4jqEZSgPI="
}
```

| Field | Type | Meaning |
|---|---|---|
| `formatVersion` | int | The format of this whole layout. The app rejects anything newer than it knows (today `1`). |
| `name` | string | Display name, shown when the user adds the repo and in the Repos list. |
| `publicKey` | string \| null | Base64 of the raw 32-byte ed25519 public key. `null` means unsigned; the app refuses to add an unsigned repo. |

## `index.json`

```json
{
  "formatVersion": 1,
  "generatedAt": "2026-10-12T17:02:03Z",
  "sources": [
    {
      "id": "mangadex",
      "name": "MangaDex",
      "lang": "en",
      "version": 10,
      "abi": 4,
      "nsfw": false,
      "baseUrl": "https://mangadex.org",
      "listings": ["popular", "latest", "new"],
      "iconUrl": "https://mangadex.org/pwa/icons/icon-180.png",
      "wasmPath": "wasm/mangadex-v10.wasm",
      "sha256": "6e2d6d14142fc4440eb1d0ee91762b354ee82e152c022c667da154f3ff578417",
      "size": 161110
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `formatVersion` | int | As in `repo.json`. |
| `generatedAt` | string | RFC 3339 UTC time of the build. Informational. |
| `sources[].id` | string | The source's id, the same as its `source_meta().id`. Ids are global: two repos offering the same id offer the same source. |
| `name`, `lang` | string | From `source_meta()`. `lang` is a BCP-47 tag (`en`, `pt-BR`); the app shows it as a language name. |
| `version` | int | `source_meta().version`. Monotonic: it drives updates, and changed bytes must come with a higher version. |
| `abi` | int | The bundle's `abi_version()`. The app installs only `SourceAbi.MIN_SUPPORTED..VERSION` (4..4 today). Others are listed as "Needs a newer Tanuki" or "Outdated", with no action. |
| `nsfw` | bool | Adult-only source. Hidden unless the user turned on "Show adult content". |
| `baseUrl`, `iconUrl` | string? | From `source_meta()`, omitted when absent. The icon is fetched from the source's site, not hosted in the repo. |
| `listings` | string[] | Which of the contract's listings it serves (`popular`, `latest`, `new`). |
| `wasmPath` | string | The bundle, relative to the base URL. |
| `sha256` | string | Lowercase hex sha256 of the bundle's bytes. |
| `size` | int | The bundle's size in bytes. |

The metadata isn't written by hand. `xtask` instantiates each built bundle and reads it from
`source_meta()`, so the index can't disagree with the binary.

## `index.json.sig`

The base64 of the 64-byte ed25519 signature over **the exact bytes** of `index.json`, as served.
Re-serializing or pretty-printing `index.json` after signing breaks it.

Signing happens only in CI, with the seed from the `TANUKI_SIGNING_KEY` Actions secret (see the
README's *Signing key*).

## What the app checks (the trust chain)

```
pinned key ──verifies──▶ index.json ──lists sha256 + size──▶ wasm/<id>-v<version>.wasm
```

1. **Adding a repo.** The app normalizes the address: it adds `https://` and a trailing `/`, accepts a
   pasted `…/repo.json`, and refuses `http://`. Then it reads `repo.json` and refuses it if it's
   unsigned or its `formatVersion` is too new. It verifies `index.json` against `publicKey`, and shows
   the user the name, source count, host and key fingerprint before saving anything. The fingerprint is
   the first 128 bits of sha256(key) as two lines of four groups of four hex digits:
   `48D0 503F C21D 8940 / F8CE F184 0D79 0851`.
   Confirming **pins** the key for that repo.
2. **First-party repo.** The app ships the `tanuki-sources` key for both of its addresses (Pages and
   jsDelivr). If either one presents any other key, the add fails before the user sees a fingerprint.
3. **Refresh** (at launch, in the background library-update job, and on pull-to-refresh). The app
   fetches `index.json` and its `.sig` and verifies them against the **pinned** key, never against
   whatever `repo.json` says now. A failure keeps the last verified index and flags the repo:
   - *key changed*: `repo.json` now publishes a different key;
   - otherwise *tampered*: same key, bad signature.

   Either way it fails closed.
4. **Install or update.** The app downloads `wasmPath` and refuses it unless the size and sha256 both
   match the verified index entry. Only then is the bundle written to disk.
5. **Origin.** The app records which repo each source was installed from, and updates and reinstalls
   come only from that repo. Another repo offering the same id can't replace it, even after the
   original repo is removed.

## Publishing rules

`xtask build` enforces these; CI fails the publish if any breaks:

- the bundle exports what the host requires, and `abi_version()` equals the SDK's `ABI_VERSION`;
- `source_meta().id` equals the bundle id in `bundles.toml`, and `source_meta().version` is set;
- **changed bytes need a higher version**: a bundle whose sha256 differs from the published one under
  the same version is refused, and so is a version that goes backwards;
- builds are byte-reproducible on CI: a pinned `rust-toolchain.toml`, committed lockfiles, and embedded
  source paths remapped. A local macOS build differs by a few hundred bytes of codegen, so only CI's
  hashes are authoritative.

## Changing the format

- **Additive** (a new optional field): keep `formatVersion`. The app ignores unknown fields.
- **Breaking:** bump `formatVersion`. Apps that don't know it refuse the repo with "This repo needs a
  newer Tanuki". Ship the app that reads the new format before publishing it.
- **Rotating the signing key** breaks every app that pinned the old one, and the app also ships this
  repo's key. Release an app carrying the new key first. Rotation through a new key signed by the old
  one isn't designed yet.
