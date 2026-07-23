//! # Tanuki source SDK
//!
//! Everything a `.wasm` manga source needs to speak the Tanuki ABI. The host counterpart lives in
//! `App/core/source/` — see `SourceAbi.kt` for the contract and `Wire.kt` for the payloads.
//!
//! ## Writing a source
//!
//! Implement [`Source`] and hand it to [`export_source!`]. Every export is optional except
//! [`Source::meta`]; the ones you skip report "unsupported" to the app instead of failing to load.
//!
//! ```ignore
//! use tanuki_source_api::*;
//!
//! struct Example;
//!
//! impl Source for Example {
//!     fn meta() -> SourceMeta { /* … */ }
//!
//!     fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
//!         let response = fetch(&HttpRequest::get("https://example.com/popular"))?;
//!         let json: serde_json::Value = serde_json::from_slice(response.body())
//!             .map_err(|e| AbiError::with_code(e.to_string(), "parse"))?;
//!         /* map json → MangaPage */
//!     }
//! }
//!
//! export_source!(Example);
//! ```
//!
//! ## What a source can and cannot do
//!
//! A source runs in a WASM sandbox with **no ambient capabilities**: no sockets, no filesystem, no
//! clock, no syscalls. The only way out is [`fetch`] and [`log`], which the host supplies. That's
//! deliberate — routing all HTTP through the host is what lets its Cloudflare interceptor apply to
//! every source uniformly, and what makes a source unable to route around it.
//!
//! ## Panics
//!
//! Don't. A panic in a `wasm32-unknown-unknown` guest aborts the module, and the host sees an opaque
//! trap rather than a useful message. Return [`AbiError`] instead — it surfaces app-side as a
//! `SourceAbiException` carrying your message.

mod abi;
mod wire;

pub use abi::ABI_VERSION;
pub use wire::*;

/// Performs an HTTP request through the host and returns its response.
///
/// Synchronous from the guest's perspective — the host blocks a worker thread on a suspending Ktor
/// call. The response is whatever the host got, including 4xx/5xx: a non-2xx status is **not** an
/// error here, so check [`HttpResponse::status`] yourself if you care.
pub fn fetch(request: &HttpRequest) -> Result<HttpResponse, AbiError> {
    let body = serde_json::to_vec(request)
        .map_err(|e| AbiError::with_code(format!("Could not encode request: {e}"), "encode"))?;

    // The host reads these bytes out of our memory; `body` stays alive across the call, and the
    // reply comes back in a host-allocated buffer that take_from_host adopts (and later frees).
    let packed = unsafe { abi::http_request(body.as_ptr() as i32, body.len() as i32) };
    let frame = unsafe { abi::take_from_host(packed) };

    if frame.is_empty() {
        return Err(AbiError::with_code("Host returned an empty HTTP response.", "http"));
    }
    decode_frame(frame)
}

/// Bytes preceding the header JSON in a reply frame: `status` and `headersLength`, both little-endian
/// `i32`. Mirrors `HttpFrame.PREFIX_BYTES` on the host.
const FRAME_PREFIX_BYTES: usize = 8;

/// Decodes the `http_request` reply frame — `[status][headersLength][headers JSON][body…]`.
///
/// Only the fixed prefix and the (small) header JSON are read here; the body is left in place and
/// addressed by offset, so a response of any size costs the same nothing to "decode". See
/// `HttpFrame.kt` on the host for the layout and the measurements that motivated it.
fn decode_frame(frame: Vec<u8>) -> Result<HttpResponse, AbiError> {
    let malformed =
        |what: &str| AbiError::with_code(format!("Malformed HTTP response frame: {what}."), "parse");

    if frame.len() < FRAME_PREFIX_BYTES {
        return Err(malformed("shorter than its header"));
    }
    let read_i32 = |at: usize| i32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]);

    let status = read_i32(0);
    let headers_len = read_i32(4);
    if headers_len < 0 {
        return Err(malformed("negative header length"));
    }
    let body_offset = FRAME_PREFIX_BYTES + headers_len as usize;
    if body_offset > frame.len() {
        return Err(malformed("header length past the end of the frame"));
    }

    let headers = if headers_len == 0 {
        std::collections::BTreeMap::new()
    } else {
        serde_json::from_slice(&frame[FRAME_PREFIX_BYTES..body_offset])
            .map_err(|e| AbiError::with_code(format!("Could not decode response headers: {e}"), "parse"))?
    };

    Ok(HttpResponse { status, headers, frame, body_offset })
}

impl HttpResponse {
    /// The response body, borrowed straight out of the host's buffer — no copy, no decode.
    pub fn body(&self) -> &[u8] {
        &self.frame[self.body_offset..]
    }

    /// The response body as UTF-8 text, lossily decoded — HTML in the wild is rarely clean.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(self.body()).into_owned()
    }
}

/// Emits a diagnostic through the host logger.
pub fn log(level: LogLevel, message: &str) {
    unsafe { abi::log(level as i32, message.as_ptr() as i32, message.len() as i32) }
}

/// What a `.wasm` source implements. Only [`Source::meta`] is required.
///
/// Methods are associated functions (no `self`) because a source is a stateless module: the host
/// instantiates it once and calls exports directly. Any state you need should be re-derived per call
/// or fetched — there is no guaranteed lifetime between calls.
pub trait Source {
    /// Describes this source: id, name, language, base URL, version, and browse tabs.
    /// Called on every app launch to build the source list, so keep it cheap and never fetch here.
    fn meta() -> SourceMeta;

    /// One page of the catalog, or search results when `request.query` is set.
    fn manga_list(_request: MangaListRequest) -> Result<MangaPage, AbiError> {
        Err(AbiError::unsupported("manga_list"))
    }

    /// The full record for one series — description, author, status.
    fn manga_details(_request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        Err(AbiError::unsupported("manga_details"))
    }

    /// Every chapter of one series. Unpaginated; order doesn't matter, the app sorts by number.
    fn chapter_list(_request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        Err(AbiError::unsupported("chapter_list"))
    }

    /// The ordered pages of one chapter.
    fn chapter_pages(_request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        Err(AbiError::unsupported("chapter_pages"))
    }
}

/// Generates the `#[no_mangle]` exports the host calls, delegating to your [`Source`] impl.
///
/// Invoke exactly once per source crate, at the crate root. Without it the compiled `.wasm` has no
/// entry points and the host refuses to load it.
#[macro_export]
macro_rules! export_source {
    ($source:ty) => {
        // The memory-ABI exports are emitted HERE, in the source crate, on purpose: a `#[no_mangle]`
        // symbol living in the SDK rlib isn't guaranteed to survive into the final `.wasm` (the
        // linker may strip it as unreachable — `tanuki_dealloc` especially, since the guest never
        // calls it itself). Emitting them at the call site makes them roots of the cdylib, so the
        // host can always allocate into and free guest memory.
        #[no_mangle]
        pub extern "C" fn tanuki_alloc(size: i32) -> i32 {
            $crate::__internal::alloc(size)
        }

        #[no_mangle]
        pub extern "C" fn tanuki_dealloc(ptr: i32, size: i32) {
            $crate::__internal::dealloc(ptr, size)
        }

        #[no_mangle]
        pub extern "C" fn abi_version() -> i32 {
            $crate::ABI_VERSION
        }

        /// Returns `SourceMeta` **bare** — this is the one export the host decodes without an
        /// envelope, because it cannot fail in-band.
        ///
        /// Takes `(ptr, len)` and ignores them, rather than being arity-0, so EVERY bytes-export has
        /// the identical `(i32, i32) -> i64` signature. The host's memory ABI calls them uniformly;
        /// wasmi's `get_typed_func` is strict about arity (Chicory is lenient), so a no-arg
        /// `source_meta` loads on Android but fails on iOS. Uniform signatures keep both engines happy.
        #[no_mangle]
        pub extern "C" fn source_meta(_ptr: i32, _len: i32) -> i64 {
            $crate::__internal::respond_bare(&<$source as $crate::Source>::meta())
        }

        #[no_mangle]
        pub extern "C" fn manga_list(ptr: i32, len: i32) -> i64 {
            $crate::__internal::respond(
                $crate::__internal::decode(ptr, len)
                    .and_then(<$source as $crate::Source>::manga_list),
            )
        }

        #[no_mangle]
        pub extern "C" fn manga_details(ptr: i32, len: i32) -> i64 {
            $crate::__internal::respond(
                $crate::__internal::decode(ptr, len)
                    .and_then(<$source as $crate::Source>::manga_details),
            )
        }

        #[no_mangle]
        pub extern "C" fn chapter_list(ptr: i32, len: i32) -> i64 {
            $crate::__internal::respond(
                $crate::__internal::decode(ptr, len)
                    .and_then(<$source as $crate::Source>::chapter_list),
            )
        }

        #[no_mangle]
        pub extern "C" fn chapter_pages(ptr: i32, len: i32) -> i64 {
            $crate::__internal::respond(
                $crate::__internal::decode(ptr, len)
                    .and_then(<$source as $crate::Source>::chapter_pages),
            )
        }
    };
}

/// Plumbing for [`export_source!`]. Public because macros expand at the call site; not an API.
#[doc(hidden)]
pub mod __internal {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    // Re-exported so `export_source!`'s `tanuki_alloc`/`tanuki_dealloc` wrappers can reach them
    // without the source crate importing the private `abi` module.
    pub use crate::abi::{alloc, dealloc};

    /// Decodes an export's input buffer, turning malformed JSON into an in-band error.
    pub fn decode<T: DeserializeOwned>(ptr: i32, len: i32) -> Result<T, AbiError> {
        let bytes = unsafe { abi::borrow_from_host(ptr, len) };
        serde_json::from_slice(bytes)
            .map_err(|e| AbiError::with_code(format!("Could not decode request: {e}"), "parse"))
    }

    /// Wraps a result in an [`Envelope`] and hands it back to the host.
    pub fn respond<T: Serialize>(result: Result<T, AbiError>) -> i64 {
        let envelope = match result {
            Ok(value) => Envelope { ok: Some(value), error: None },
            Err(error) => Envelope::<T> { ok: None, error: Some(error) },
        };
        match serde_json::to_vec(&envelope) {
            Ok(bytes) => abi::bytes_to_host(&bytes),
            // Encoding our own envelope failed — emit a hand-built one so the host still gets a
            // decodable error rather than a trap.
            Err(_) => abi::bytes_to_host(
                br#"{"error":{"message":"Source failed to encode its response.","code":"encode"}}"#,
            ),
        }
    }

    /// Hands back a value with no envelope. Only `source_meta` uses this shape.
    pub fn respond_bare<T: Serialize>(value: &T) -> i64 {
        match serde_json::to_vec(value) {
            Ok(bytes) => abi::bytes_to_host(&bytes),
            Err(_) => abi::bytes_to_host(b"{}"),
        }
    }
}
