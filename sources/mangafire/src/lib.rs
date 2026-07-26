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
//!   → `{ items: [{ hid, slug?, title, poster:{url} }], … }` (browse / latest / search).
//! - `GET /api/titles/{hid}` → the full record (`synopsisHtml`, `authors[]`, `status`).
//! - `GET /api/titles/{hid}/chapters?language=en&sort=number&order=desc&page&limit=200`
//!   → `{ items: [{ id, number, name?, createdAt?, type? }] }` (paginated; walked here).
//! - `GET /api/chapters/{id}` → `{ data: { pages: [{ url }] } }` (direct image URLs).
//!
//! ## ⚠️ Not yet operational — needs the reference VrfSigner + a few DTO confirmations
//! [`vrf::CONFIGURED`] is `false`: the signing constants must be transcribed verbatim from the reference
//! `VrfSigner.kt` before any API call works (until then browse fails with a clear `"config"` error, by
//! design — a wrong token would silently 403). Fields marked `TODO(dto)` below are best-effort guesses to
//! confirm against the real `Dto.kt` / a live response: the poster/author field names, the `createdAt`
//! unit, the manga permalink scheme, and whether responses are wrapped.

mod vrf;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use tanuki_source_api::*;

/// Site + API host. Endpoints are `{BASE}/api/…`.
const BASE: &str = "https://mangafire.to";
/// Sent on every request; MangaFire's image hosts (and some API paths) reject a missing/foreign Referer.
const REFERER: &str = "https://mangafire.to/";
const USER_AGENT: &str = "Tanuki (+https://tanuki.app)";
/// Translation language this source serves, matching [`Source::meta`]'s `lang`.
const LANG: &str = "en";
/// Catalog page size (MangaFire's own default).
const PAGE_SIZE: i32 = 50;
/// Chapters per request; walked until exhausted since the ABI's `chapter_list` is unpaginated.
const CHAPTER_PAGE_SIZE: i32 = 200;
/// Safety valve on the chapter walk so a bad page never loops forever.
const MAX_CHAPTER_REQUESTS: usize = 50;

struct MangaFire;

impl Source for MangaFire {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "mangafire".to_string(),
            name: "MangaFire".to_string(),
            lang: LANG.to_string(),
            base_url: Some(BASE.to_string()),
            version: Some(1),
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
        let mut query = format!("page={page}&limit={PAGE_SIZE}");
        match request.query.as_deref() {
            Some(keyword) if !keyword.trim().is_empty() => {
                query.push_str("&keyword=");
                query.push_str(&percent_encode(keyword.trim()));
            }
            _ => {
                let order = match request.listing {
                    Some(Listing::Latest) => "order[chapter_updated_at]=desc",
                    _ => "order[views_30d]=desc",
                };
                query.push('&');
                query.push_str(order);
            }
        }

        let response: ApiList<MangaDto> = signed_get("/api/titles", &query)?;
        let entries: Vec<Manga> = response.items.iter().map(MangaDto::to_wire).collect();
        // No reliable page-count field is confirmed yet, so infer: a full page implies more may follow.
        let has_next_page = entries.len() as i32 >= PAGE_SIZE;
        Ok(MangaPage { entries, has_next_page })
    }

    /// The full record for one series — the description/author/status browse omits.
    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let hid = request.manga_key.trim();
        if hid.is_empty() {
            return Err(AbiError::with_code("A manga lookup needs a manga key.", "not_found"));
        }
        let detail: MangaDetailsDto = signed_get(&format!("/api/titles/{hid}"), "")?;
        Ok(detail.to_wire())
    }

    /// Every chapter of one series in [`LANG`], newest first. Walks the paginated feed until it empties.
    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let hid = request.manga_key.trim();
        if hid.is_empty() {
            return Err(AbiError::with_code("A chapter list needs a manga key.", "not_found"));
        }

        let mut chapters = Vec::new();
        for page in 1..=MAX_CHAPTER_REQUESTS {
            let query = format!(
                "language={LANG}&sort=number&order=desc&page={page}&limit={CHAPTER_PAGE_SIZE}"
            );
            let response: ApiList<ChapterDto> =
                signed_get(&format!("/api/titles/{hid}/chapters"), &query)?;

            let fetched = response.items.len() as i32;
            chapters.extend(response.items.iter().map(ChapterDto::to_wire));
            if fetched < CHAPTER_PAGE_SIZE {
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
        let response: PagesResponse = signed_get(&format!("/api/chapters/{chapter_id}"), "")?;
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
/// `query` is the canonical parameter string (without `vrf`); it is signed into the `vrf` token appended
/// to the request. An empty `query` still signs (some endpoints take no params). Fails with a clear
/// `"config"` error while [`vrf::CONFIGURED`] is false, so the source never sends an invalid token.
fn signed_get<T: DeserializeOwned>(path: &str, query: &str) -> Result<T, AbiError> {
    let vrf = vrf::sign(query).ok_or_else(|| {
        AbiError::with_code(
            "MangaFire request signing (vrf) is not configured — fill in the VrfSigner constants in \
             native/sources/mangafire/src/vrf.rs before this source can reach the API.",
            "config",
        )
    })?;
    let url = if query.is_empty() {
        format!("{BASE}{path}?vrf={vrf}")
    } else {
        format!("{BASE}{path}?{query}&vrf={vrf}")
    };
    get_json(&url)
}

/// Fetches [url] with the site headers and decodes JSON, turning non-2xx into an `"http"` error and
/// unparseable bodies into a `"parse"` one. Cloudflare's `cf_clearance` is added by the host, not here.
fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, AbiError> {
    let response = fetch(
        &HttpRequest::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Referer", REFERER),
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

/// The manga's permalink on the site. TODO(dto): confirm MangaFire's URL scheme — assumed
/// `/manga/{slug}.{hid}`, falling back to `/manga/{hid}` when the listing carries no slug.
fn manga_url(hid: &str, slug: Option<&str>) -> String {
    match slug {
        Some(slug) if !slug.is_empty() => format!("{BASE}/manga/{slug}.{hid}"),
        _ => format!("{BASE}/manga/{hid}"),
    }
}

// --- JSON DTOs (guest-side mirror of the API; confirm TODO(dto) fields against the real Dto.kt) --------

/// A list response: `{ items: [...] }`. TODO(dto): confirm there's no outer `{ status, result }` wrapper
/// and whether pagination metadata (a `meta` object) should drive `has_next_page` instead of the count.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApiList<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    hid: String,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    poster: Option<Poster>,
}

impl MangaDto {
    fn to_wire(&self) -> Manga {
        Manga {
            key: self.hid.clone(),
            url: Some(manga_url(&self.hid, self.slug.as_deref())),
            title: self.title.clone(),
            cover_url: self.poster.as_ref().and_then(|p| p.url.clone()),
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
    poster: Option<Poster>,
    #[serde(default)]
    synopsis_html: Option<String>,
    #[serde(default)]
    authors: Vec<Entity>,
    #[serde(default)]
    status: Option<String>,
}

impl MangaDetailsDto {
    fn to_wire(&self) -> Manga {
        Manga {
            key: self.hid.clone(),
            url: Some(manga_url(&self.hid, self.slug.as_deref())),
            title: self.title.clone(),
            cover_url: self.poster.as_ref().and_then(|p| p.url.clone()),
            author: self.authors.iter().find_map(|a| a.name.clone()),
            // TODO(dto): synopsisHtml is HTML; strip tags to plain text once the exact markup is known.
            description: self.synopsis_html.clone(),
            // Passed through; the host maps an unrecognized name to UNKNOWN. TODO(dto): map MangaFire's
            // status vocabulary (e.g. "releasing"/"completed"/"on_hiatus") to the ABI's PublishStatus names.
            status: self.status.clone(),
            ..Default::default()
        }
    }
}

/// TODO(dto): confirm the poster/cover field name (assumed `url`).
#[derive(Deserialize)]
struct Poster {
    #[serde(default)]
    url: Option<String>,
}

/// TODO(dto): confirm the author entity's display field (assumed `name`).
#[derive(Deserialize)]
struct Entity {
    #[serde(default)]
    name: Option<String>,
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
}

impl ChapterDto {
    fn to_wire(&self) -> Chapter {
        Chapter {
            key: self.id.to_string(),
            name: self.name.clone(),
            number: self.number,
            // TODO(dto): confirm createdAt's unit. Assumed epoch **seconds** → the ABI wants milliseconds.
            date_upload: self.created_at.map(|seconds| seconds * 1_000),
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
struct PagesResponse {
    data: ChapterData,
}

#[derive(Deserialize)]
struct ChapterData {
    #[serde(default = "Vec::new")]
    pages: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    url: String,
}

/// Percent-encodes a query value (RFC 3986 unreserved passes through, everything else `%XX`). Hand-rolled
/// like the MangaDex source, to avoid a dependency for the one bit of user-controlled text in a URL.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
