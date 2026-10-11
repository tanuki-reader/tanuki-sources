//! MangaFire — Tanuki's second source, and the first behind **Cloudflare** and **VRF request signing**.
//!
//! Unlike MangaDex, every `/api/*` call must carry a `vrf` query token (see [`vrf`]) or the server
//! rejects it, and the whole site sits behind Cloudflare — which is invisible here: the host's
//! interceptor solves challenges below this code, and image fetches replay the same `cf_clearance` (see
//! CLAUDE.md → Cloudflare Bypass). MangaFire once scrambled page images but no longer does; today it
//! serves a clean JSON API, so — like MangaDex — there is no HTML scraping. The one image-side need is a
//! `Referer`, carried on each [`Page`] via the ABI's per-page `headers`.
//!
//! ## API shape (`https://mangafire.to/api`)
//! - `GET /api/titles?order[views_30d]=desc | order[chapter_updated_at]=desc | keyword=…&page&limit=50`
//!   → `{ items: [{ hid, slug?, title, poster:{small,medium,large} }], meta:{ hasNext } }`.
//! - `GET /api/titles/{hid}` → `{ data: { …full record… } }` (wrapped).
//! - `GET /api/titles/{hid}/chapters?language=en&sort=number&order=desc&page&limit=200`
//!   → `{ items: [{ id, number, name?, createdAt?, type? }], meta:{ hasNext } }` (walked here).
//! - `GET /api/chapters/{id}` → `{ data: { pages: [{ url }] } }` (direct image URLs).
//!
//! Volumes (`/api/titles/{hid}/volumes`) are intentionally not listed yet — chapters only, for now.

mod vrf;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use tanuki_source_api::*;

/// Site + API host. Endpoints are `{BASE}/api/…`.
const BASE: &str = "https://mangafire.to";
/// Sent on every request; MangaFire's image hosts (and some API paths) reject a missing/foreign Referer.
const REFERER: &str = "https://mangafire.to/";
/// MangaFire's tag for this build's language, sent as the chapter feed's `language` parameter.
const LANG: &str = language::API_TAG;
/// Catalog page size (MangaFire's own default).
const PAGE_SIZE: i32 = 50;
/// Chapters per request; walked until `meta.hasNext` is false since the ABI's `chapter_list` is unpaginated.
const CHAPTER_PAGE_SIZE: i32 = 200;
/// Safety valve on the chapter walk so a bad page never loops forever.
const MAX_CHAPTER_REQUESTS: usize = 50;

/// Everything that differs between this crate's language builds — MangaFire translates into several
/// languages behind one API, and the ABI gives a source exactly one `lang`, so the crate is compiled
/// once per language behind a cargo feature. Mirrors the MangaDex crate; see the longer note there.
#[cfg(feature = "lang-en")]
mod language {
    pub const API_TAG: &str = "en";
    pub const ABI_TAG: &str = "en";
    pub const SOURCE_ID: &str = "mangafire";
    pub const SOURCE_NAME: &str = "MangaFire";
}

#[cfg(feature = "lang-pt-br")]
mod language {
    pub const API_TAG: &str = "pt-br";
    pub const ABI_TAG: &str = "pt-BR";
    pub const SOURCE_ID: &str = "mangafire-pt-br";
    pub const SOURCE_NAME: &str = "MangaFire";
}

#[cfg(not(any(feature = "lang-en", feature = "lang-pt-br")))]
compile_error!("Build this source with exactly one language feature: lang-en or lang-pt-br.");
#[cfg(all(feature = "lang-en", feature = "lang-pt-br"))]
compile_error!("Build this source with exactly one language feature, not both.");

struct MangaFire;

impl Source for MangaFire {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: language::SOURCE_ID.to_string(),
            name: language::SOURCE_NAME.to_string(),
            lang: language::ABI_TAG.to_string(),
            base_url: Some(BASE.to_string()),
            // v2 makes the language a build-time choice, adding the pt-BR bundle.
            version: Some(3),
            // TODO(icon): point at a raster (PNG/WebP) MangaFire icon; the favicon is an .ico the loader
            // can't decode, so None (lettered avatar) is correct until a raster URL is confirmed.
            icon_url: None,
            nsfw: false,
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }

    /// One catalog/search page. A non-blank query searches by `keyword`; otherwise the listing chooses
    /// the ordering (Popular → 30-day views, Latest → most recently updated).
    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let page = request.page.max(1);
        let mut params: Vec<(&str, String)> =
            vec![("page", page.to_string()), ("limit", PAGE_SIZE.to_string())];
        match request.query.as_deref() {
            Some(keyword) if !keyword.trim().is_empty() => {
                params.push(("keyword", keyword.trim().to_string()));
            }
            _ => match request.listing {
                Some(Listing::Latest) => params.push(("order[chapter_updated_at]", "desc".to_string())),
                _ => params.push(("order[views_30d]", "desc".to_string())),
            },
        }

        let response: ApiList<MangaDto> = signed_get("/api/titles", &params)?;
        let has_next_page = response.meta.as_ref().map_or(false, |m| m.has_next);
        let entries = response.items.iter().map(MangaDto::to_wire).collect();
        Ok(MangaPage { entries, has_next_page })
    }

    /// The full record for one series — the description/author/status browse omits. The response is
    /// wrapped in a `data` object (unlike the list endpoints).
    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let hid = request.manga_key.trim();
        if hid.is_empty() {
            return Err(AbiError::with_code("A manga lookup needs a manga key.", "not_found"));
        }
        let response: DataResponse<MangaDetailsDto> = signed_get(&format!("/api/titles/{hid}"), &[])?;
        Ok(response.data.to_wire())
    }

    /// Every chapter of one series in [`LANG`], newest first. Walks the paginated feed until `meta.hasNext`
    /// clears (or the safety cap). Volumes are a separate endpoint and not listed here.
    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let hid = request.manga_key.trim();
        if hid.is_empty() {
            return Err(AbiError::with_code("A chapter list needs a manga key.", "not_found"));
        }

        let mut chapters = Vec::new();
        for page in 1..=MAX_CHAPTER_REQUESTS {
            let params: Vec<(&str, String)> = vec![
                ("language", LANG.to_string()),
                ("sort", "number".to_string()),
                ("order", "desc".to_string()),
                ("page", page.to_string()),
                ("limit", CHAPTER_PAGE_SIZE.to_string()),
            ];
            let response: ApiList<ChapterDto> =
                signed_get(&format!("/api/titles/{hid}/chapters"), &params)?;

            chapters.extend(response.items.iter().map(ChapterDto::to_wire));
            if !response.meta.as_ref().map_or(false, |m| m.has_next) {
                break;
            }
        }
        Ok(chapters)
    }

    /// The ordered page images of one chapter. MangaFire returns direct image URLs, but its CDN enforces
    /// a `Referer`, so each page carries one via the ABI's per-page headers — the app applies it on the
    /// (Cloudflare-carrying) image fetch.
    fn chapter_pages(request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        let chapter_id = request.chapter_key.trim();
        if chapter_id.is_empty() {
            return Err(AbiError::with_code("A page list needs a chapter key.", "not_found"));
        }
        let response: DataResponse<ChapterDataDto> = signed_get(&format!("/api/chapters/{chapter_id}"), &[])?;
        let pages = response
            .data
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| Page {
                index: index as i32,
                image_url: Some(page.url),
                url: None,
                headers: BTreeMap::from([("Referer".to_string(), REFERER.to_string())]),
            })
            .collect();
        Ok(pages)
    }
}

export_source!(MangaFire);

/// GETs a VRF-signed MangaFire API endpoint and decodes its JSON.
///
/// Signs the request's [`canonical_signing_input`] into the `vrf` token, appends it, fetches with the
/// site headers, and decodes. `params` are (key, **decoded** value) pairs; they are re-emitted
/// percent-encoded on the wire while the signature is computed over their decoded, sorted, `[]`-indexed
/// canonical form (see [`vrf`]).
fn signed_get<T: DeserializeOwned>(path: &str, params: &[(&str, String)]) -> Result<T, AbiError> {
    let vrf = vrf::sign(&canonical_signing_input(path, params))
        .ok_or_else(|| AbiError::with_code("Could not sign the MangaFire request (vrf).", "config"))?;

    // Wire URL: params in the same key-sorted order (original keys, encoded values), then vrf.
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut url = format!("{BASE}{path}?");
    for (key, value) in sorted {
        url.push_str(key);
        url.push('=');
        url.push_str(&percent_encode(value));
        url.push('&');
    }
    url.push_str("vrf=");
    url.push_str(&vrf);

    get_json(&url)
}

/// Builds the exact string MangaFire signs: the `/api`-stripped path, then — if any — the parameters
/// **sorted by key** and `&`-joined, with a `key[]` rewritten to `key[0]`, `key[1]`, … per repeat, using
/// the **decoded** values. Mirrors `VrfSigner.sortedQueryUrl` in the reference extension.
fn canonical_signing_input(path: &str, params: &[(&str, String)]) -> String {
    let stripped = path.strip_prefix("/api").unwrap_or(path);
    let mut out = String::from(stripped);
    if params.is_empty() {
        return out;
    }

    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));

    out.push('?');
    let mut last_key = "";
    let mut index = 0usize;
    for (i, (key, value)) in sorted.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        if let Some(base) = key.strip_suffix("[]") {
            if last_key != *key {
                index = 0;
            }
            last_key = key;
            out.push_str(base);
            out.push('[');
            out.push_str(&index.to_string());
            out.push(']');
            index += 1;
        } else {
            out.push_str(key);
        }
        out.push('=');
        out.push_str(value);
    }
    out
}

/// Fetches [url] with the site headers and decodes JSON, turning non-2xx into an `"http"` error and
/// unparseable bodies into a `"parse"` one. The `User-Agent` and Cloudflare's `cf_clearance` are both
/// applied by the host, not here — the app owns one browser-shaped UA for all sources, and a source
/// setting its own is what got MangaFire permanently challenged.
fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, AbiError> {
    let response = fetch(
        &HttpRequest::get(url).header("Referer", REFERER),
    )?;
    if !(200..300).contains(&response.status) {
        return Err(AbiError::with_code(
            format!("MangaFire returned HTTP {}", response.status),
            "http",
        ));
    }
    serde_json::from_slice(response.body())
        .map_err(|e| AbiError::with_code(format!("Could not parse MangaFire response: {e}"), "parse"))
}

/// The absolute permalink to a series on the site: `/title/{hid}-{slug}` (or `/title/{hid}` with no slug),
/// matching the reference extension's `SManga.url`.
fn manga_url(hid: &str, slug: Option<&str>) -> String {
    match slug {
        Some(slug) if !slug.is_empty() => format!("{BASE}/title/{hid}-{slug}"),
        _ => format!("{BASE}/title/{hid}"),
    }
}

// --- JSON DTOs (guest-side mirror of the API, from the reference Dto.kt) ------------------------------

/// A list response: `{ items, meta }`. Titles and chapters share this shape.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiList<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    #[serde(default)]
    meta: Option<ApiMeta>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMeta {
    #[serde(default)]
    has_next: bool,
}

/// A single-entity response wrapped in `data` — details and pages use this (the list endpoints don't).
#[derive(Deserialize)]
struct DataResponse<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    hid: String,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    poster: Option<PosterDto>,
}

impl MangaDto {
    fn to_wire(&self) -> Manga {
        Manga {
            key: self.hid.clone(),
            url: Some(manga_url(&self.hid, self.slug.as_deref())),
            title: self.title.clone(),
            cover_url: self.poster.as_ref().and_then(PosterDto::best),
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetailsDto {
    hid: String,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    poster: Option<PosterDto>,
    #[serde(default)]
    synopsis_html: Option<String>,
    #[serde(default)]
    authors: Option<Vec<EntityDto>>,
}

impl MangaDetailsDto {
    fn to_wire(&self) -> Manga {
        let author = self.authors.as_ref().and_then(|list| {
            let joined = list.iter().map(|e| e.title.as_str()).collect::<Vec<_>>().join(", ");
            (!joined.is_empty()).then_some(joined)
        });
        Manga {
            key: self.hid.clone(),
            url: Some(manga_url(&self.hid, self.slug.as_deref())),
            title: self.title.clone(),
            cover_url: self.poster.as_ref().and_then(PosterDto::best),
            author,
            description: self.synopsis_html.as_deref().map(strip_html),
            status: self.status.as_deref().and_then(map_status),
            ..Default::default()
        }
    }
}

/// Cover variants, largest first. Mirrors the reference's `poster.large ?: medium ?: small`.
#[derive(Deserialize)]
struct PosterDto {
    #[serde(default)]
    small: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    large: Option<String>,
}

impl PosterDto {
    fn best(&self) -> Option<String> {
        self.large.clone().or_else(|| self.medium.clone()).or_else(|| self.small.clone())
    }
}

#[derive(Deserialize)]
struct EntityDto {
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    id: i64,
    #[serde(default)]
    number: Option<f64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

impl ChapterDto {
    fn to_wire(&self) -> Chapter {
        Chapter {
            // chapter_pages addresses the chapter by its numeric id (GET /api/chapters/{id}).
            key: self.id.to_string(),
            name: self.name.clone(),
            number: self.number,
            // createdAt is epoch **seconds**; the ABI wants milliseconds.
            date_upload: self.created_at.map(|seconds| seconds * 1_000),
            // MangaFire has no scanlation groups; its `type` (e.g. the release track) stands in.
            scanlator: self.kind.clone(),
        }
    }
}

#[derive(Deserialize)]
struct ChapterDataDto {
    #[serde(default = "Vec::new")]
    pages: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    url: String,
}

/// Maps MangaFire's status vocabulary to an ABI `PublishStatus` name (host matches case-insensitively);
/// an unrecognized value returns `None`, which the host reads as `UNKNOWN`.
fn map_status(status: &str) -> Option<String> {
    let name = match status.to_ascii_lowercase().as_str() {
        "releasing" => "ongoing",
        "finished" => "completed",
        "on_hiatus" => "hiatus",
        "discontinued" => "cancelled",
        _ => return None,
    };
    Some(name.to_string())
}

