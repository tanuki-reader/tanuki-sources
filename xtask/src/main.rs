//! `cargo run --manifest-path xtask/Cargo.toml -- <command>`
//!
//! - `build --out DIR [--previous DIR]` — builds every bundle in `bundles.toml`, validates it, and
//!   writes the static repo (`repo.json`, `index.json`, `index.json.sig`, `wasm/`) to `DIR`. Signs when
//!   `TANUKI_SIGNING_KEY` (base64 ed25519 seed) is set. `--previous` is the last published repo, for
//!   the version-bump gate.
//! - `keygen` — prints a new signing seed and its public key.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

type Result<T> = std::result::Result<T, String>;

const INDEX_FORMAT: u32 = 1;
const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Exports the host requires of every source (`SourceAbi.Memory` / `SourceAbi.Exports` in the app).
const REQUIRED_EXPORTS: &[&str] = &[
    "memory",
    "tanuki_alloc",
    "tanuki_dealloc",
    "abi_version",
    "source_meta",
    "manga_list",
    "manga_details",
    "chapter_list",
    "chapter_pages",
];

#[derive(Deserialize)]
struct Bundles {
    name: String,
    bundle: Vec<Bundle>,
}

#[derive(Deserialize)]
struct Bundle {
    id: String,
    #[serde(rename = "crate")]
    krate: String,
    feature: Option<String>,
}

/// What `source_meta` returns; the fields the index carries.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    id: String,
    name: String,
    lang: String,
    base_url: Option<String>,
    version: Option<i32>,
    icon_url: Option<String>,
    #[serde(default)]
    nsfw: bool,
    #[serde(default)]
    listings: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    format_version: u32,
    generated_at: String,
    sources: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    name: String,
    lang: String,
    version: i32,
    abi: i32,
    nsfw: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    listings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon_url: Option<String>,
    wasm_path: String,
    sha256: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoJson {
    format_version: u32,
    name: String,
    /// Base64 ed25519 public key; `null` for an unsigned (local dev) repo.
    public_key: Option<String>,
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("keygen") => keygen(),
        _ => Err("usage: xtask build --out DIR [--previous DIR] | xtask keygen".into()),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
}

fn build(args: &[String]) -> Result<()> {
    let out = flag(args, "--out").ok_or("--out DIR is required")?;
    let root = repo_root();
    let bundles: Bundles = toml::from_str(&read_text(&root.join("bundles.toml"))?)
        .map_err(|e| format!("bundles.toml: {e}"))?;
    let previous = match flag(args, "--previous").map(|dir| dir.join("index.json")) {
        Some(path) if path.exists() => {
            let index: Index = serde_json::from_str(&read_text(&path)?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            index.sources
        }
        _ => Vec::new(),
    };
    let previous: HashMap<&str, &Entry> = previous.iter().map(|e| (e.id.as_str(), e)).collect();

    fs::create_dir_all(out.join("wasm")).map_err(|e| e.to_string())?;
    let mut sources = Vec::new();
    for bundle in &bundles.bundle {
        let bytes = cargo_build(&root, bundle)?;
        let entry = describe(bundle, &bytes)?;
        if let Some(prev) = previous.get(entry.id.as_str()) {
            check_version(prev, &entry)?;
        }
        fs::write(out.join(&entry.wasm_path), &bytes).map_err(|e| e.to_string())?;
        println!("{} v{} ({} bytes)", entry.id, entry.version, entry.size);
        sources.push(entry);
    }

    let index = Index { format_version: INDEX_FORMAT, generated_at: utc_now(), sources };
    let index_bytes = serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?;
    fs::write(out.join("index.json"), &index_bytes).map_err(|e| e.to_string())?;

    let public_key = match env::var("TANUKI_SIGNING_KEY") {
        Ok(seed) if !seed.is_empty() => {
            let key = signing_key(&seed)?;
            fs::write(out.join("index.json.sig"), sign(&key, &index_bytes)).map_err(|e| e.to_string())?;
            Some(BASE64.encode(key.verifying_key().to_bytes()))
        }
        _ => {
            eprintln!("warning: TANUKI_SIGNING_KEY not set; the index is unsigned (local dev only)");
            None
        }
    };
    let repo = RepoJson { format_version: INDEX_FORMAT, name: bundles.name, public_key };
    fs::write(out.join("repo.json"), serde_json::to_vec_pretty(&repo).unwrap()).map_err(|e| e.to_string())
}

/// Builds one bundle to `.wasm` in its own target dir (two language builds of a crate differ only by
/// feature, and would otherwise overwrite each other's artifact).
fn cargo_build(root: &Path, bundle: &Bundle) -> Result<Vec<u8>> {
    let crate_dir = root.join("sources").join(&bundle.krate);
    let target_dir = root.join("target/bundles").join(&bundle.id);
    let mut cargo = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(&crate_dir).args(["build", "--release", "--target", WASM_TARGET]);
    cargo.arg("--target-dir").arg(&target_dir);
    if let Some(feature) = &bundle.feature {
        cargo.args(["--no-default-features", "--features", feature]);
    }
    let status = cargo.status().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("{}: cargo build failed", bundle.id));
    }
    let artifact = format!("tanuki_source_{}.wasm", bundle.krate.replace('-', "_"));
    fs::read(target_dir.join(WASM_TARGET).join("release").join(artifact)).map_err(|e| format!("{}: {e}", bundle.id))
}

/// The publish gate: instantiates the bundle the way the app does, checks its exports and ABI, and
/// reads its index entry out of `source_meta` — so the index can't drift from the binary.
fn describe(bundle: &Bundle, bytes: &[u8]) -> Result<Entry> {
    let fail = |what: String| format!("{}: {what}", bundle.id);
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, bytes).map_err(|e| fail(format!("invalid wasm: {e}")))?;
    for name in REQUIRED_EXPORTS {
        if module.get_export(name).is_none() {
            return Err(fail(format!("missing export `{name}`")));
        }
    }
    let mut store = wasmi::Store::new(&engine, ());
    let mut linker = wasmi::Linker::<()>::new(&engine);
    // The gate makes no network calls: http_request fails, as it does offline in the app.
    linker.func_wrap("tanuki", "http_request", |_: i32, _: i32| -> i64 { 0 }).unwrap();
    linker.func_wrap("tanuki", "log", |_: i32, _: i32, _: i32| {}).unwrap();
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| fail(format!("instantiate: {e}")))?;

    let abi = instance
        .get_typed_func::<(), i32>(&store, "abi_version")
        .and_then(|f| f.call(&mut store, ()).map_err(Into::into))
        .map_err(|e| fail(format!("abi_version: {e}")))?;
    if abi != tanuki_source_api::ABI_VERSION {
        return Err(fail(format!("ABI v{abi}, but this SDK is v{}", tanuki_source_api::ABI_VERSION)));
    }
    let packed = instance
        .get_typed_func::<(i32, i32), i64>(&store, "source_meta")
        .and_then(|f| f.call(&mut store, (0, 0)).map_err(Into::into))
        .map_err(|e| fail(format!("source_meta: {e}")))?;
    let (ptr, len) = (((packed as u64) >> 32) as usize, (packed & 0xFFFF_FFFF) as usize);
    let mut json = vec![0u8; len];
    instance
        .get_memory(&store, "memory")
        .unwrap()
        .read(&store, ptr, &mut json)
        .map_err(|e| fail(format!("source_meta out of bounds: {e}")))?;
    let meta: Meta = serde_json::from_slice(&json).map_err(|e| fail(format!("source_meta: {e}")))?;

    if meta.id != bundle.id {
        return Err(fail(format!("meta().id is `{}`; it must equal the bundle id", meta.id)));
    }
    let version = meta.version.ok_or_else(|| fail("meta().version is required to publish".into()))?;
    Ok(Entry {
        wasm_path: format!("wasm/{}-v{version}.wasm", meta.id),
        id: meta.id,
        name: meta.name,
        lang: meta.lang,
        version,
        abi,
        nsfw: meta.nsfw,
        base_url: meta.base_url,
        listings: meta.listings,
        icon_url: meta.icon_url,
        sha256: hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
    })
}

/// Refuses changed bytes under an already-published version (an app that has that version would
/// never see the change), and versions going backwards.
fn check_version(previous: &Entry, next: &Entry) -> Result<()> {
    if next.version < previous.version {
        return Err(format!("{}: version went from {} back to {}", next.id, previous.version, next.version));
    }
    if next.version == previous.version && next.sha256 != previous.sha256 {
        return Err(format!(
            "{}: the bundle changed but is still v{}; bump source_meta().version",
            next.id, next.version
        ));
    }
    Ok(())
}

fn keygen() -> Result<()> {
    let mut seed = [0u8; 32];
    use std::io::Read;
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).map_err(|e| e.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    println!("TANUKI_SIGNING_KEY (secret, store as an Actions secret): {}", BASE64.encode(seed));
    println!("public key (published in repo.json):                    {}", BASE64.encode(key.verifying_key().to_bytes()));
    Ok(())
}

fn signing_key(seed_base64: &str) -> Result<SigningKey> {
    let seed: [u8; 32] = BASE64
        .decode(seed_base64.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("TANUKI_SIGNING_KEY must be a base64 32-byte seed (see `xtask keygen`)")?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Base64 detached signature over the exact bytes of `index.json`.
fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    BASE64.encode(key.sign(bytes).to_bytes())
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Current UTC time as RFC 3339 (`2026-10-12T12:00:00Z`), via the days→civil conversion (Howard
/// Hinnant's), to avoid a date-time dependency for one timestamp.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    fn entry(version: i32, sha256: &str) -> Entry {
        Entry {
            id: "mangadex".into(),
            name: "MangaDex".into(),
            lang: "en".into(),
            version,
            abi: 4,
            nsfw: false,
            base_url: None,
            listings: vec![],
            icon_url: None,
            wasm_path: format!("wasm/mangadex-v{version}.wasm"),
            sha256: sha256.into(),
            size: 1,
        }
    }

    #[test]
    fn changed_bytes_need_a_version_bump() {
        assert!(check_version(&entry(9, "aa"), &entry(9, "aa")).is_ok());
        assert!(check_version(&entry(9, "aa"), &entry(10, "bb")).is_ok());
        assert!(check_version(&entry(9, "aa"), &entry(9, "bb")).is_err());
        assert!(check_version(&entry(9, "aa"), &entry(8, "bb")).is_err());
    }

    #[test]
    fn the_signature_verifies_against_the_published_key() {
        let key = signing_key(&BASE64.encode([7u8; 32])).unwrap();
        let index = br#"{"formatVersion":1}"#;
        let signature: [u8; 64] = BASE64.decode(sign(&key, index)).unwrap().try_into().unwrap();
        let public = key.verifying_key();
        assert!(public.verify(index, &Signature::from_bytes(&signature)).is_ok());
        assert!(public.verify(b"tampered", &Signature::from_bytes(&signature)).is_err());
    }

    #[test]
    fn utc_now_is_rfc3339() {
        let now = utc_now();
        assert_eq!(now.len(), 20);
        assert!(now.starts_with("20") && now.ends_with('Z'));
    }
}
