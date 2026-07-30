//! Hipercool (`lerhentais.com`) — pt-BR, adult, and the first source that has to **earn a session**
//! before the API will answer it.
//!
//! ## The two things the API insists on
//!
//! 1. **A `__st` cookie.** Every `/api/trpc/*` call without one is a bare
//!    `401 {"error":"Unauthorized: Invalid or missing API Key"}`. The site mints it as a `Set-Cookie` on
//!    an ordinary HTML page load, so [`authorize`] fetches the site root with an HTML `Accept` and lets
//!    the host's cookie jar keep what comes back — the same move the reference extension makes, and the
//!    reason `SourceCookieJar` exists host-side. The guest never sees or handles the cookie itself.
//! 2. **An `x-flux-node` header.** Without it the catalog still answers, but `reader.chapterPages`
//!    replies `FORBIDDEN` with *"Use o aplicativo oficial em lerhentais.com"* — so pages, the part that
//!    matters, need it. Its name and value are assembled at runtime in the site's own bundle (from
//!    char codes and a split base64 literal) to keep them out of a plain-text search; deobfuscated they
//!    are the constants in [`FLUX_NODE`]. Expect this to rot — see the note there.
//!
//! Notably **no WebView is needed**, which is where this diverges from the reference extension: it
//! drives one to capture the headers the site's own JS sends. Those headers turn out to be exactly the
//! cookie plus `x-flux-node`, both of which a guest can obtain by itself.
//!
//! ## API shape (`https://lerhentais.com/api/trpc`)
//!
//! tRPC, so each call is `GET /api/trpc/{procedure}?batch=1&input={"0":{"json":{…}}}` and the reply is
//! an array of `{"result":{"data":{"json": …}}}` — one element per batched procedure, or an
//! `{"error":{"json":{…}}}` element in its place. The reference batches `auth.me` alongside every call;
//! nothing needs it, so each procedure is called on its own here:
//!
//! - `search.query` → `{ hits: [...], totalHits }` — catalog, search, and both listings (`sort`).
//! - `series.bySlugWithGenres` → the full series record.
//! - `series.chapters` → **every** chapter of a series, unpaginated.
//! - `reader.chapterPages` → the pages, addressed by *series slug + chapter number* rather than by a
//!   chapter id, which is why the keys this source hands out are composite (see [`pack_key`]).

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use tanuki_source_api::*;

const BASE: &str = "https://lerhentais.com";
/// Sent on every image fetch — page *and* cover. The CDN 403s a request with no `Referer`, and checks
/// the value (any other origin is refused too; verified against the live host), so it rides along via
/// the ABI's per-page [`Page::headers`] and per-series [`Manga::cover_headers`].
const REFERER: &str = "https://lerhentais.com/";

/// The header that unlocks `reader.chapterPages`, as `(name, value)`.
///
/// A hardcoded shared secret, and treated as one: the site can rotate it whenever it likes, and when it
/// does, page loading breaks with the site's own *"Use o aplicativo oficial"* message while browsing
/// keeps working. That asymmetry is the symptom to recognise. Re-reading it means finding the `fetch`
/// override in the site's `assets/index-*.js` bundle, where the name is built from char codes and the
/// value from `atob(...)` plus a literal tail.
const FLUX_NODE: (&str, &str) = ("x-flux-node", "G2ZsDdWhUwdU82Vw");

/// Catalog page size. The API takes any `limit`; 30 matches the site's own grid.
const PAGE_SIZE: i32 = 30;
/// Widest content rating to return. This source *is* the adult catalog — narrowing it here would hide
/// most of the site — so it asks for everything and reports `nsfw` in [`Source::meta`] instead.
const MAX_RATING: &str = "pornographic";
/// Separator packing two identifiers into one ABI key. A slug is `[a-z0-9-]`, so it can't contain this.
const KEY_SEPARATOR: char = '|';

struct Hipercool;

impl Source for Hipercool {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "hipercool".to_string(),
            name: "Hipercool".to_string(),
            lang: "pt-BR".to_string(),
            base_url: Some(BASE.to_string()),
            version: Some(1),
            // The site's icon is an .ico the app's loader can't decode, and it publishes no raster
            // logo; a lettered avatar is the correct fallback until one turns up.
            icon_url: None,
            nsfw: true,
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }

    /// One catalog/search page. Query and listing share `search.query` — a blank `q` with a sort is the
    /// catalog, a non-blank one is a search — so both go through the same call.
    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let page = request.page.max(1);
        let query = request.query.as_deref().map(str::trim).unwrap_or_default();
        let sort = match request.listing {
            Some(Listing::Latest) => "recent",
            // Popular, and the default catalog when no listing was asked for.
            _ => "popular",
        };

        // `meta.values` is tRPC's marker for "this key was literally `undefined`, not null". The server
        // rejects the filter object without it, so every filter we don't set has to be declared here.
        let input = json!({
            "0": {
                "json": {
                    "q": query,
                    "sort": sort,
                    "filters": {
                        "genres": null, "type": null, "status": null, "contentRating": null,
                        "author": null, "artist": null, "year": null,
                    },
                    "limit": PAGE_SIZE,
                    "offset": (page - 1) * PAGE_SIZE,
                    "maxRating": MAX_RATING,
                },
                "meta": { "values": {
                    "filters.genres": ["undefined"], "filters.type": ["undefined"],
                    "filters.status": ["undefined"], "filters.contentRating": ["undefined"],
                    "filters.author": ["undefined"], "filters.artist": ["undefined"],
                    "filters.year": ["undefined"],
                }},
            }
        });

        let results: SearchResults = trpc("search.query", &input)?;
        let entries: Vec<Manga> = results.hits.iter().map(MangaDto::to_wire).collect();
        // `totalHits` counts the whole result set, so "is there more" is arithmetic, not a guess.
        let seen = (page - 1) * PAGE_SIZE + entries.len() as i32;
        Ok(MangaPage { has_next_page: seen < results.total_hits, entries })
    }

    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let (_, slug) = unpack_key(&request.manga_key)?;
        let dto: MangaDto = trpc("series.bySlugWithGenres", &json!({ "0": { "json": { "slug": slug } } }))?;
        Ok(dto.to_wire())
    }

    /// Every chapter of one series — the API returns them all in one call, no paging.
    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let (series_id, slug) = unpack_key(&request.manga_key)?;
        // Must go out as a JSON *number*: the procedure's schema is typed, and a quoted id is rejected.
        let series_id: i64 = series_id
            .parse()
            .map_err(|_| AbiError::with_code(format!("Not a series id: '{series_id}'."), "not_found"))?;

        let chapters: Vec<ChapterDto> =
            trpc("series.chapters", &json!({ "0": { "json": { "seriesId": series_id } } }))?;
        Ok(chapters.iter().map(|dto| dto.to_wire(slug)).collect())
    }

    /// The ordered page images of one chapter.
    ///
    /// Addressed by series slug + chapter number, not by chapter id — the API offers no lookup by id —
    /// which is what the composite chapter key carries.
    fn chapter_pages(request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        let (slug, number) = unpack_key(&request.chapter_key)?;
        let number: f64 = number
            .parse()
            .map_err(|_| AbiError::with_code(format!("Not a chapter number: '{number}'."), "not_found"))?;

        let input = json!({ "0": { "json": { "seriesSlug": slug, "chapterNumber": number } } });
        let pages: Vec<PageDto> = trpc("reader.chapterPages", &input)?;
        Ok(pages.iter().map(PageDto::to_wire).collect())
    }
}

export_source!(Hipercool);

/// Calls one tRPC procedure and decodes the `result.data.json` out of its reply.
///
/// Handles the session for its caller: a `401` means the `__st` cookie is missing or expired, so it
/// [`authorize`]s and retries — **once**, so a site that refuses to hand out a session can't put this
/// in a loop.
fn trpc<T: DeserializeOwned>(procedure: &str, input: &serde_json::Value) -> Result<T, AbiError> {
    let url = format!(
        "{BASE}/api/trpc/{procedure}?batch=1&input={}",
        percent_encode(&input.to_string()),
    );

    let mut response = get(&url)?;
    if response.status == 401 {
        authorize()?;
        response = get(&url)?;
    }
    // The body is read BEFORE the status is judged, because tRPC puts its real explanation there even
    // on a non-2xx: a deleted series still listed in the search index answers `404` with
    // "Série não encontrada", and `403` is the `x-flux-node` gate's "use the official app". Checking the
    // status first would replace both with a bare "HTTP 404", which is exactly the information the user
    // needs and the only clue that tells the two failures apart.
    let batch: Result<Vec<TrpcEnvelope<T>>, _> = serde_json::from_slice(response.body());

    match batch.ok().and_then(|elements| elements.into_iter().next_back()) {
        // One procedure per call, so the reply is a one-element batch; take the last either way.
        Some(TrpcEnvelope { result: Some(result), .. }) => Ok(result.data.json),
        Some(TrpcEnvelope { error: Some(error), .. }) => Err(AbiError::with_code(
            format!("Hipercool refused '{procedure}': {}", error.json.message),
            if response.status == 404 { "not_found" } else { "http" },
        )),
        // No decodable envelope: fall back to whatever the status says. A 2xx here means the shape
        // changed under us, which is a parse problem, not a network one.
        _ if !(200..300).contains(&response.status) => Err(AbiError::with_code(
            format!("Hipercool returned HTTP {} for '{procedure}'.", response.status),
            "http",
        )),
        _ => Err(AbiError::with_code(
            format!("Could not parse Hipercool's reply to '{procedure}'."),
            "parse",
        )),
    }
}

/// Fetches the site root as a browser would, so the site issues the `__st` session cookie the API
/// requires. The cookie itself is never read here — the host's jar keeps it and attaches it to
/// everything that follows. The HTML `Accept` is what makes this a page load rather than an API call;
/// without it the root answers as JSON and sets nothing.
fn authorize() -> Result<(), AbiError> {
    let response = fetch(
        &HttpRequest::get(BASE)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    )?;
    if !(200..400).contains(&response.status) {
        return Err(AbiError::with_code(
            format!("Hipercool would not start a session (HTTP {}).", response.status),
            "http",
        ));
    }
    Ok(())
}

/// GETs an API URL with the headers every call needs.
fn get(url: &str) -> Result<HttpResponse, AbiError> {
    fetch(
        &HttpRequest::get(url)
            .header("Referer", REFERER)
            .header(FLUX_NODE.0, FLUX_NODE.1),
    )
}

/// Joins the two identifiers an ABI key has to carry into one string.
///
/// The ABI gives a manga and a chapter one opaque key each, but this API addresses a series by *id* in
/// one call and by *slug* in another, and a chapter by *series slug + number* rather than by its own id.
/// Packing both halves into the key keeps every export to a single request; the alternative is an extra
/// slug→id round-trip on every chapter list, which in a WASM interpreter is the expensive kind.
fn pack_key(first: impl std::fmt::Display, second: &str) -> String {
    format!("{first}{KEY_SEPARATOR}{second}")
}

/// Splits a [`pack_key`] key back into its halves.
fn unpack_key(key: &str) -> Result<(&str, &str), AbiError> {
    key.split_once(KEY_SEPARATOR)
        .map(|(first, second)| (first.trim(), second.trim()))
        .filter(|(first, second)| !first.is_empty() && !second.is_empty())
        .ok_or_else(|| AbiError::with_code(format!("Not a Hipercool key: '{key}'."), "not_found"))
}

/// Formats a chapter number the way the site writes it in a URL — `103`, not `103.0`.
fn format_number(number: f64) -> String {
    let rendered = number.to_string();
    rendered.strip_suffix(".0").unwrap_or(&rendered).to_string()
}

// --- API response subset (only the fields we read) ----------------------------------------------

/// One element of a tRPC batch reply: either a `result` or an `error`, never both.
///
/// The explicit bound keeps serde from demanding `T: Default` — the `#[serde(default)]`s below are on
/// the `Option` fields, which default to `None` whatever `T` is.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct TrpcEnvelope<T> {
    #[serde(default)]
    result: Option<TrpcResult<T>>,
    #[serde(default)]
    error: Option<TrpcError>,
}

#[derive(Deserialize)]
struct TrpcResult<T> {
    data: TrpcData<T>,
}

/// tRPC's transformer wraps every payload in a `json` key (that's where non-JSON types would be
/// reconstructed from the sibling `meta`); nothing this source reads needs the `meta`.
#[derive(Deserialize)]
struct TrpcData<T> {
    json: T,
}

#[derive(Deserialize)]
struct TrpcError {
    json: TrpcErrorBody,
}

#[derive(Deserialize)]
struct TrpcErrorBody {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResults {
    #[serde(default = "Vec::new")]
    hits: Vec<MangaDto>,
    #[serde(default)]
    total_hits: i32,
}

/// A series. The same shape backs the catalog and the details call — the latter simply fills in more of
/// it — so one DTO covers both and absent fields stay `None`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    id: i64,
    slug: String,
    title: String,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    authors: Option<Vec<String>>,
}

impl MangaDto {
    fn to_wire(&self) -> Manga {
        Manga {
            key: pack_key(self.id, &self.slug),
            url: Some(format!("{BASE}/manga/{}", self.slug)),
            title: self.title.clone(),
            cover_url: self.cover_url.clone(),
            // Covers sit on the same hotlink-guarded CDN as the pages and 403 without a Referer, so
            // they carry one exactly as the pages do.
            cover_headers: BTreeMap::from([("Referer".to_string(), REFERER.to_string())]),
            author: self
                .authors
                .as_ref()
                .map(|names| names.join(", "))
                .filter(|joined| !joined.is_empty()),
            // The synopsis is plain text on this site, but it is user-submitted and occasionally
            // carries markup; stripping it is cheap and can only help.
            description: self.synopsis.as_deref().map(strip_html),
            status: self.status.as_deref().and_then(map_status),
            original_title: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    #[serde(default)]
    number: f64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl ChapterDto {
    /// `series_slug` rather than the chapter's own id, because that is what `reader.chapterPages` takes.
    fn to_wire(&self, series_slug: &str) -> Chapter {
        Chapter {
            key: pack_key(series_slug, &format_number(self.number)),
            // Most chapters here are untitled; the app names those from the number itself.
            name: self.title.as_deref().and_then(|title| {
                let trimmed = title.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }),
            number: Some(self.number),
            date_upload: self.created_at.as_deref().and_then(parse_datetime_millis),
            scanlator: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageDto {
    #[serde(default)]
    page_order: i32,
    #[serde(default)]
    webp_url: String,
    /// AVIF where the site has transcoded one — smaller, and every platform the app runs on decodes it.
    #[serde(default)]
    avif_url: Option<String>,
}

impl PageDto {
    fn to_wire(&self) -> Page {
        Page {
            // `pageOrder` is 1-based; the ABI's index is 0-based.
            index: (self.page_order - 1).max(0),
            image_url: Some(
                self.avif_url
                    .clone()
                    .filter(|url| !url.is_empty())
                    .unwrap_or_else(|| self.webp_url.clone()),
            ),
            url: None,
            headers: BTreeMap::from([("Referer".to_string(), REFERER.to_string())]),
        }
    }
}

/// Maps the site's status vocabulary to an ABI `PublishStatus` name (the host matches case-insensitively);
/// an unrecognized value returns `None`, which the host reads as `UNKNOWN`.
fn map_status(status: &str) -> Option<String> {
    let name = match status.trim().to_ascii_lowercase().as_str() {
        "ongoing" => "ongoing",
        "completed" => "completed",
        "hiatus" => "hiatus",
        "cancelled" => "cancelled",
        _ => return None,
    };
    Some(name.to_string())
}
