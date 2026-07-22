//! The JSON payload types crossing the ABI — the guest-side mirror of `app.tanuki.core.source.abi.Wire`.
//!
//! **These must stay field-for-field identical to `Wire.kt`.** Nothing enforces that but care and the
//! conformance fixtures, so when you change one, change the other in the same commit.
//!
//! Two encoding rules are inherited from the host's `SourceCodec`:
//!  - `rename_all = "camelCase"` — Kotlin writes `coverUrl`, not `cover_url`.
//!  - `skip_serializing_if = "Option::is_none"` — matches Kotlin's `explicitNulls = false`, so absent
//!    optionals are omitted rather than written as `null`.
//!
//! Unknown fields are ignored on both sides (serde's default; Kotlin's `ignoreUnknownKeys = true`),
//! which is what lets a newer host add optional fields without breaking older guests.

use serde::{Deserialize, Serialize};

/// Result wrapper for every fallible export, so a guest reports failure in-band instead of trapping.
/// Exactly one of `ok` / `error` is set. Mirrors `Wire.Envelope`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AbiError>,
}

/// A structured guest-side failure. Mirrors `Wire.AbiError`; surfaces host-side as `SourceAbiException`.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AbiError {
    pub message: String,
    /// Optional machine-readable kind, e.g. `"http"`, `"parse"`, `"not_found"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl AbiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: None }
    }

    pub fn with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self { message: message.into(), code: Some(code.into()) }
    }

    /// The error a default `Source` method returns for an export the source doesn't implement.
    pub fn unsupported(export: &str) -> Self {
        Self::with_code(format!("This source does not implement '{export}'."), "unsupported")
    }
}

/// Source self-description, returned by the `source_meta` export. Mirrors `Wire.SourceMetaWire`.
///
/// Note: `source_meta` returns this **bare**, not wrapped in an [`Envelope`] — the host decodes it
/// directly. It is the one export that cannot fail in-band.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMeta {
    /// Source-native id, e.g. `"mangadex"`. Must match the bundle's file name in the store.
    pub id: String,
    pub name: String,
    /// BCP-47 language tag, e.g. `"en"`, `"pt-BR"`.
    pub lang: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Monotonic version for update checks. Bump on every release of this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    pub nsfw: bool,
    /// Browse tabs this source offers. Empty means "only a default catalog".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub listings: Vec<Listing>,
}

/// One browse tab a source declares. Mirrors `Wire.ListingWire`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    /// Echoed back in [`MangaListRequest::listing`]. Opaque to the app.
    pub key: String,
    pub name: String,
}

/// A catalog/search entry. Mirrors `Wire.MangaWire`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Manga {
    /// Source-native manga id. Addresses this series in every later call.
    pub key: String,
    /// Absolute permalink to this series on the source's site. The source builds it (it owns its URL
    /// scheme); persisted host-side as the manga's natural key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Must match a `PublishStatus` name (`ONGOING`, `COMPLETED`, `HIATUS`, …) case-insensitively;
    /// anything unrecognised maps to `UNKNOWN` host-side rather than failing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// One page of catalog results. Mirrors `Wire.MangaPageWire`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MangaPage {
    pub entries: Vec<Manga>,
    /// Whether `page + 1` would return more. Report `false` if you can't tell — the app stops paging.
    pub has_next_page: bool,
}

/// One chapter entry. Mirrors `Wire.ChapterWire`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    /// Source-native chapter id, passed back to `chapter_pages`.
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `4`, `4.5`, … Negative or non-finite degrades to "unknown" host-side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<f64>,
    /// Unix epoch **milliseconds**. Send `None` (not `0`) when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_upload: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanlator: Option<String>,
}

/// One page of a chapter. Mirrors `Wire.PageWire`. Set exactly one of `image_url` / `url`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub index: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// --- Requests (host → guest) ---------------------------------------------------------------------

/// Input to `manga_list`. Mirrors `Wire.MangaListRequest`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MangaListRequest {
    /// Free-text search. Already normalised host-side: blank arrives as `None`.
    pub query: Option<String>,
    /// 1-based page index.
    pub page: i32,
    /// Which declared listing to browse; `None` means your default catalog. If you don't recognise
    /// the key, fall back to the default rather than erroring.
    pub listing: Option<String>,
}

/// Input to `manga_details`. Mirrors `Wire.MangaDetailsRequest`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MangaDetailsRequest {
    pub manga_key: String,
}

/// Input to `chapter_list`. Mirrors `Wire.ChapterListRequest`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChapterListRequest {
    pub manga_key: String,
}

/// Input to `chapter_pages`. Mirrors `Wire.ChapterPagesRequest`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChapterPagesRequest {
    pub chapter_key: String,
}

// --- Host import payloads ------------------------------------------------------------------------

/// A request the guest asks the host to perform. Mirrors `Wire.HttpRequestWire`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub url: String,
    /// Uppercase verb. Defaults to `GET` via [`HttpRequest::get`].
    pub method: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Base64 body, so binary survives the JSON hop. `None` for bodyless methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self { url: url.into(), method: "GET".to_string(), ..Default::default() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// The host's reply. Mirrors `Wire.HttpResponseWire`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpResponse {
    pub status: i32,
    pub headers: std::collections::BTreeMap<String, String>,
    /// Base64 body — use [`HttpResponse::body`] rather than decoding by hand.
    pub body_base64: String,
}

/// Severity for the `log` import; the `i32` passed is this enum's ordinal, matching `Wire.LogLevel`.
#[derive(Clone, Copy)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}
