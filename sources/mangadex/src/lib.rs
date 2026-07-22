//! MangaDex — the first real Tanuki source, and the first that reaches the network.
//!
//! Unlike `hello`, this source calls [`fetch`] (the host `http_request` import), so it exercises the
//! whole pipeline: guest → host Ktor client → back to guest with bytes to parse. MangaDex serves a
//! clean JSON:API, so there's no HTML scraping here — that complexity is deferred to a later source
//! (MangaFire). The source is Cloudflare-unaware by design: if a challenge ever appears, the host's
//! interceptor solves it below this code, invisibly.
//!
//! Implemented so far: [`Source::meta`] and [`Source::manga_list`] (browse + search). Details, chapter
//! lists, and pages come next.
//!
//! ## API shape (MangaDex v5, `https://api.mangadex.org`)
//!
//! `GET /manga` returns a JSON:API collection: `data[]` each with an `id`, an `attributes` object
//! (`title`/`description` are language-keyed maps; `status` is `ongoing|completed|hiatus|cancelled`),
//! and `relationships[]`. Requesting `includes[]=cover_art` and `includes[]=author` inlines those
//! relationships' `attributes`, so the cover file name and author name arrive in one call. A cover URL
//! is built as `https://uploads.mangadex.org/covers/{manga-id}/{fileName}.512.jpg` (the `.512.jpg`
//! suffix is MangaDex's 512px thumbnail — right-sized for a browse grid).

use std::collections::BTreeMap;

use serde::Deserialize;
use tanuki_source_api::*;

/// API + CDN hosts and paging size, in one place.
const API_BASE: &str = "https://api.mangadex.org";
const COVER_BASE: &str = "https://uploads.mangadex.org/covers";
const PAGE_SIZE: i32 = 20;

struct MangaDex;

impl Source for MangaDex {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "mangadex".to_string(),
            name: "MangaDex".to_string(),
            lang: "en".to_string(),
            base_url: Some("https://mangadex.org".to_string()),
            version: Some(1),
            nsfw: false,
            listings: vec![
                Listing { key: "popular".to_string(), name: "Popular".to_string() },
                Listing { key: "latest".to_string(), name: "Latest Updates".to_string() },
            ],
        }
    }

    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let url = build_list_url(&request);

        // A descriptive UA is polite and helps if MangaDex ever rate-limits by client. The Cloudflare
        // cf_clearance cookie (when it exists) is added by the host, not here.
        let response = fetch(&HttpRequest::get(&url).header("User-Agent", "Tanuki (+https://tanuki.app)"))?;
        if !(200..300).contains(&response.status) {
            return Err(AbiError::with_code(
                format!("MangaDex returned HTTP {}", response.status),
                "http",
            ));
        }

        let list: ApiList = serde_json::from_slice(&response.body()?)
            .map_err(|e| AbiError::with_code(format!("Could not parse MangaDex response: {e}"), "parse"))?;

        let entries: Vec<Manga> = list.data.iter().map(map_manga).collect();
        // Another page exists if we haven't yet walked past the total. MangaDex returns both, so we
        // don't have to guess.
        let has_next_page = list.offset + (list.data.len() as i32) < list.total;

        Ok(MangaPage { entries, has_next_page })
    }
}

export_source!(MangaDex);

/// Builds the `/manga` request URL for a browse/search page.
///
/// A non-blank query searches by `title` (relevance order, MangaDex's default). Otherwise the declared
/// listing chooses the sort: `latest` by most-recent chapter, anything else (including `popular`) by
/// follower count. `contentRating` is capped at safe+suggestive for now — an explicit-content toggle
/// can widen it later.
fn build_list_url(request: &MangaListRequest) -> String {
    let page = request.page.max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let mut url = format!(
        "{API_BASE}/manga?limit={PAGE_SIZE}&offset={offset}\
         &includes[]=cover_art&includes[]=author\
         &contentRating[]=safe&contentRating[]=suggestive"
    );

    match request.query.as_deref() {
        Some(query) if !query.trim().is_empty() => {
            url.push_str("&title=");
            url.push_str(&percent_encode(query.trim()));
        }
        _ => {
            let order = match request.listing.as_deref() {
                Some("latest") => "order[latestUploadedChapter]=desc",
                _ => "order[followedCount]=desc",
            };
            url.push('&');
            url.push_str(order);
        }
    }
    url
}

/// Maps one API entry to a wire [`Manga`]. Everything is best-effort: a missing title becomes a
/// placeholder rather than dropping the entry, and absent cover/author/description are simply `None`.
fn map_manga(api: &ApiManga) -> Manga {
    let cover = api
        .relationship("cover_art")
        .and_then(|rel| rel.attributes.as_ref())
        .and_then(|attrs| attrs.file_name.as_ref())
        .map(|file| format!("{COVER_BASE}/{}/{}.512.jpg", api.id, file));

    let author = api
        .relationship("author")
        .and_then(|rel| rel.attributes.as_ref())
        .and_then(|attrs| attrs.name.clone());

    Manga {
        key: api.id.clone(),
        // The source owns its URL scheme, so the permalink is built here, not in the app.
        url: Some(format!("https://mangadex.org/title/{}", api.id)),
        title: pick_localized(&api.attributes.title).unwrap_or_else(|| "Untitled".to_string()),
        original_title: original_title(&api.attributes),
        cover_url: cover,
        author,
        description: pick_localized(&api.attributes.description),
        status: api.attributes.status.clone(),
    }
}

/// Picks a display string from a MangaDex language-keyed map: English if present, else any entry, else
/// `None`. MangaDex titles/descriptions are `{"en": "...", "ja": "...", ...}`.
fn pick_localized(map: &BTreeMap<String, String>) -> Option<String> {
    map.get("en")
        .cloned()
        .or_else(|| map.values().next().cloned())
        .filter(|s| !s.is_empty())
}

/// The romanized/original title, when the source offers one that differs from the display title.
/// Prefers a romaji alt title (`ja-ro`), then the original-language entry.
fn original_title(attrs: &ApiAttributes) -> Option<String> {
    let display = pick_localized(&attrs.title);
    attrs
        .alt_titles
        .iter()
        .find_map(|alt| alt.get("ja-ro").or_else(|| alt.get(&attrs.original_language)).cloned())
        .filter(|candidate| Some(candidate) != display.as_ref())
}

/// Percent-encodes a query value (RFC 3986 unreserved set passes through; everything else is `%XX`).
/// Small enough to avoid a dependency, and the only user-controlled text that reaches the URL.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// --- API response subset (only the fields we read) ----------------------------------------------

#[derive(Deserialize)]
struct ApiList {
    #[serde(default)]
    data: Vec<ApiManga>,
    #[serde(default)]
    offset: i32,
    #[serde(default)]
    total: i32,
}

#[derive(Deserialize)]
struct ApiManga {
    #[serde(default)]
    id: String,
    #[serde(default)]
    attributes: ApiAttributes,
    #[serde(default)]
    relationships: Vec<ApiRelationship>,
}

impl ApiManga {
    /// First relationship of the given type (e.g. `"cover_art"`, `"author"`), if inlined.
    fn relationship(&self, rel_type: &str) -> Option<&ApiRelationship> {
        self.relationships.iter().find(|rel| rel.rel_type == rel_type)
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApiAttributes {
    #[serde(default)]
    title: BTreeMap<String, String>,
    #[serde(default)]
    alt_titles: Vec<BTreeMap<String, String>>,
    #[serde(default)]
    description: BTreeMap<String, String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    original_language: String,
}

#[derive(Deserialize)]
struct ApiRelationship {
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    attributes: Option<ApiRelationshipAttributes>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApiRelationshipAttributes {
    /// Present on `cover_art` relationships.
    #[serde(default)]
    file_name: Option<String>,
    /// Present on `author`/`artist` relationships.
    #[serde(default)]
    name: Option<String>,
}
