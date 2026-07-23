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
    /// Remote URL of the source's icon, shown beside its name in the source list. Omit it and the app
    /// draws a lettered avatar instead. Must be a format the app can decode — a PNG/JPEG/WebP; an
    /// `.ico` or `.svg` favicon will not render.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    pub nsfw: bool,
    /// Which of the contract's [`Listing`]s this source serves. Empty means "only a default catalog".
    /// Declaration order is ignored — the app renders them in the contract's own order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub listings: Vec<Listing>,
}

/// The **closed set** of browse tabs the contract defines. Mirrors the host's `SourceListing` enum;
/// on the wire it is the plain string the host's `Wire` carries.
///
/// A source does not invent listings: it declares which of these it serves in [`SourceMeta::listings`],
/// and receives one back in [`MangaListRequest::listing`]. That is what lets the app know up front what
/// a source can be asked for — it only decides *which* of a known set to show (both, one, or neither).
///
/// Travels as its lowercase name (`"popular"`, `"latest"`), per the ABI's "enums travel as their string
/// name" rule, so adding a variant later doesn't disturb the existing two.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Listing {
    /// The source's most-followed/most-read series.
    Popular,
    /// The source's most recently updated series.
    Latest,
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
    /// anything unrecognized maps to `UNKNOWN` host-side rather than failing.
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
    /// Which [`Listing`] to browse; `None` means your default catalog. The host only ever sends one you
    /// declared in [`SourceMeta::listings`], and a variant this build doesn't know decodes to `None`
    /// (see [`lenient_listing`]) rather than failing the whole request.
    #[serde(deserialize_with = "lenient_listing")]
    pub listing: Option<Listing>,
}

/// Decodes [`MangaListRequest::listing`], degrading an unrecognized value to `None` — a guest built
/// against an older contract than the host then browses its default catalog instead of erroring, which
/// mirrors how the host maps unknown enum names to a documented fallback.
fn lenient_listing<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Listing>, D::Error> {
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.and_then(|name| match name.as_str() {
        "popular" => Some(Listing::Popular),
        "latest" => Some(Listing::Latest),
        _ => None,
    }))
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
    /// Base64-encoded request body; `None` for bodyless methods. Still base64 (unlike the *response*
    /// body, which is framed raw) because a request crosses as JSON and is small — no source sends
    /// bulk bytes upward. Encode it yourself; the SDK carries no base64 dependency for this.
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

/// The host's reply. Mirrors `Wire.HttpResponseWire`, decoded from the binary frame the
/// `http_request` import returns (see `HttpFrame.kt` on the host).
///
/// The body is **not** a separate allocation: `frame` is the host's buffer, adopted whole, and
/// [`HttpResponse::body`] hands back the slice of it after the header block. Nothing about the
/// response body is copied, decoded, or parsed unless a source asks for it.
#[derive(Default)]
pub struct HttpResponse {
    pub status: i32,
    pub headers: std::collections::BTreeMap<String, String>,
    pub(crate) frame: Vec<u8>,
    pub(crate) body_offset: usize,
}

/// Severity for the `log` import; the `i32` passed is this enum's ordinal, matching `Wire.LogLevel`.
#[derive(Clone, Copy)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}
