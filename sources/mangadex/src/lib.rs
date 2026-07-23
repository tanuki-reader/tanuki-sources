//! MangaDex — the first real Tanuki source, and the first that reaches the network.
//!
//! Unlike `hello`, this source calls [`fetch`] (the host `http_request` import), so it exercises the
//! whole pipeline: guest → host Ktor client → back to guest with bytes to parse. MangaDex serves a
//! clean JSON:API, so there's no HTML scraping here — that complexity is deferred to a later source
//! (MangaFire). The source is Cloudflare-unaware by design: if a challenge ever appears, the host's
//! interceptor solves it below this code, invisibly.
//!
//! Implemented so far: [`Source::meta`], [`Source::manga_list`] (browse + search),
//! [`Source::manga_details`] and [`Source::chapter_list`]. Pages come next.
//!
//! Browse and details deliberately parse **different** subsets of the same `/manga` shape — see
//! [`ListAttributes`]. A browse page is twenty entries parsed in a WASM interpreter, so every field it
//! doesn't render is worth not parsing; a detail lookup is one entry and can afford the full record.
//!
//! ## API shape (MangaDex v5, `https://api.mangadex.org`)
//!
//! `GET /manga` returns a JSON:API collection: `data[]` each with an `id`, an `attributes` object
//! (`title`/`description` are language-keyed maps; `status` is `ongoing|completed|hiatus|cancelled`),
//! and `relationships[]`. Requesting `includes[]=cover_art` and `includes[]=author` inlines those
//! relationships' `attributes`, so the cover file name and author name arrive in one call. A cover URL
//! is built as `https://uploads.mangadex.org/covers/{manga-id}/{fileName}.512.jpg` (the `.512.jpg`
//! suffix is MangaDex's 512px thumbnail — right-sized for a browse grid).
//!
//! `GET /manga/{id}/feed` returns the same collection shape for chapters: `attributes` carries
//! `volume`/`chapter` (strings, both nullable — MangaDex has unnumbered oneshots), `title`,
//! `translatedLanguage`, `pages`, `externalUrl` and the RFC 3339 `publishAt`/`readableAt` timestamps.
//! `includes[]=scanlation_group` inlines the translating group. The feed is paginated and caps `limit`
//! at 500, so a long series takes several round-trips — the ABI's `chapter_list` is unpaginated, so
//! this source walks the feed itself.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tanuki_source_api::*;

/// API and CDN hosts and paging sizes, in one place.
const API_BASE: &str = "https://api.mangadex.org";
const COVER_BASE: &str = "https://uploads.mangadex.org/covers";
const PAGE_SIZE: i32 = 20;
/// Chapters per feed request. 500 is MangaDex's hard cap on `limit`.
const FEED_PAGE_SIZE: i32 = 500;
/// Safety valve on the feed walk, so a bad `total` can't spin forever: 20 × 500 = 10 000 chapters,
/// which is also MangaDex's own `offset` ceiling. No series comes close.
const MAX_FEED_REQUESTS: usize = 20;
/// Translation language served by this source, matching the `lang` in [`Source::meta`].
const LANG: &str = "en";
/// The source's badge in the app's source list. MangaDex's PWA icon, because it is the only mascot
/// artwork the site serves as a **raster** image — the brand logo is an SVG and the favicon an `.ico`,
/// neither of which the app's image loader decodes. 180×180 is comfortably above the 34dp it's drawn at.
const ICON_URL: &str = "https://mangadex.org/pwa/icons/icon-180.png";
/// A descriptive UA is polite and helps if MangaDex ever rate-limits by client.
const USER_AGENT: &str = "Tanuki (+https://tanuki.app)";

struct MangaDex;

impl Source for MangaDex {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "mangadex".to_string(),
            name: "MangaDex".to_string(),
            lang: "en".to_string(),
            base_url: Some("https://mangadex.org".to_string()),
            // v2 adds chapter_list and the icon; v3 moves to the contract's listing vocabulary;
            // v4 adds manga_details and trims browse to the fields the grid actually renders.
            version: Some(4),
            icon_url: Some(ICON_URL.to_string()),
            nsfw: false,
            // Both of the contract's listings map onto a MangaDex sort order — see build_list_url.
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }

    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let list: ApiList<ApiManga<ListAttributes>> = get_json(&build_list_url(&request))?;

        let entries: Vec<Manga> = list.data.iter().map(map_list_manga).collect();
        // Another page exists if we haven't yet walked past the total. MangaDex returns both, so we
        // don't have to guess.
        let has_next_page = list.offset + (list.data.len() as i32) < list.total;

        Ok(MangaPage { entries, has_next_page })
    }

    /// The full record for one series — everything the browse grid didn't need.
    ///
    /// Browse deliberately parses a reduced attribute set (see [`ListAttributes`]), so this is where a
    /// description and the original title actually come from. One request, no paging: `/manga/{id}`
    /// returns a single entity rather than a collection.
    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let manga_key = request.manga_key.trim();
        if manga_key.is_empty() {
            return Err(AbiError::with_code("A manga lookup needs a manga key.", "not_found"));
        }

        let entity: ApiEntity<ApiManga<DetailAttributes>> = get_json(&build_details_url(manga_key))?;
        Ok(map_detail_manga(&entity.data))
    }

    /// Every chapter of one series, in this source's language.
    ///
    /// The ABI's `chapter_list` is unpaginated but MangaDex's feed caps out at 500 entries a call, so
    /// this walks the feed until it has passed `total` (or hits [`MAX_FEED_REQUESTS`]). Each request
    /// blocks a host thread for a round-trip, which is why the page size is the maximum: a 1000-chapter
    /// series costs two calls, not fifty.
    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let manga_key = request.manga_key.trim();
        if manga_key.is_empty() {
            return Err(AbiError::with_code("A chapter list needs a manga key.", "not_found"));
        }

        let mut chapters = Vec::new();
        let mut offset = 0;
        for _ in 0..MAX_FEED_REQUESTS {
            let feed: ApiList<ApiChapter> = get_json(&build_feed_url(manga_key, offset))?;

            let fetched = feed.data.len() as i32;
            chapters.extend(feed.data.iter().filter(|api| api.is_readable()).map(map_chapter));

            offset += fetched;
            // A page that came back empty means the feed is exhausted regardless of what `total` says
            // — without that check a wrong `total` would burn all MAX_FEED_REQUESTS on empty replies.
            if fetched == 0 || offset >= feed.total {
                break;
            }
        }
        Ok(chapters)
    }
}

export_source!(MangaDex);

/// GETs a MangaDex endpoint and decodes its JSON, turning a non-2xx status into an `"http"` error and
/// unparseable JSON into a `"parse"` one. The Cloudflare `cf_clearance` cookie (when it exists) is
/// added by the host, not here.
fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, AbiError> {
    let response = fetch(&HttpRequest::get(url).header("User-Agent", USER_AGENT))?;
    if !(200..300).contains(&response.status) {
        return Err(AbiError::with_code(
            format!("MangaDex returned HTTP {}", response.status),
            "http",
        ));
    }
    serde_json::from_slice(response.body())
        .map_err(|e| AbiError::with_code(format!("Could not parse MangaDex response: {e}"), "parse"))
}

/// Builds the `/manga` request URL for a browse/search page.
///
/// A non-blank query searches by `title` (relevance order, MangaDex's default). Otherwise the requested
/// listing chooses the sort: [`Listing::Latest`] by most-recent chapter, [`Listing::Popular`] (and the
/// no-listing default) by follower count. `contentRating` is capped at safe+suggestive for now — an explicit-content toggle
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
            let order = match request.listing {
                Some(Listing::Latest) => "order[latestUploadedChapter]=desc",
                // Popular, and the default catalog when no listing was asked for.
                _ => "order[followedCount]=desc",
            };
            url.push('&');
            url.push_str(order);
        }
    }
    url
}

/// Builds the single-series `/manga/{id}` request URL. Same `includes[]` as browse, so the cover and
/// author arrive inline rather than costing a relationship lookup each.
fn build_details_url(manga_key: &str) -> String {
    format!(
        "{API_BASE}/manga/{}?includes[]=cover_art&includes[]=author",
        percent_encode(manga_key),
    )
}

/// Builds one page of the `/manga/{id}/feed` request URL.
///
/// Filtered to this source's [`LANG`] and the same safe+suggestive content ratings as browse (the feed
/// would otherwise default to a wider set than the catalog that led here). Ordered ascending by
/// volume then chapter so the walk is stable across requests, and `includes[]=scanlation_group` inlines
/// the translating group so the scanlator arrives without a second call.
fn build_feed_url(manga_key: &str, offset: i32) -> String {
    format!(
        "{API_BASE}/manga/{}/feed?limit={FEED_PAGE_SIZE}&offset={offset}\
         &translatedLanguage[]={LANG}&includes[]=scanlation_group\
         &contentRating[]=safe&contentRating[]=suggestive\
         &order[volume]=asc&order[chapter]=asc",
        percent_encode(manga_key),
    )
}

/// Maps one feed entry to a wire [`Chapter`]. Lenient like [`map_manga`]: a chapter MangaDex didn't
/// number (a oneshot, an extra) still comes through, and the app copes with the `None`s.
fn map_chapter(api: &ApiChapter) -> Chapter {
    let attrs = &api.attributes;

    // A chapter with no title is the norm on MangaDex — leave it None rather than inventing
    // "Chapter 12" here; the app derives a display name from the number.
    let name = attrs
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string);

    // `chapter` is a string ("4", "4.5", sometimes ""), and MangaDex leaves it null on oneshots.
    let number = attrs.chapter.as_deref().and_then(|raw| raw.trim().parse::<f64>().ok());

    // Several groups can share a chapter; credit them all rather than picking one arbitrarily.
    let scanlators: Vec<&str> = api
        .relationships
        .iter()
        .filter(|rel| rel.rel_type == "scanlation_group")
        .filter_map(|rel| rel.attributes.as_ref()?.name.as_deref())
        .filter(|name| !name.is_empty())
        .collect();

    Chapter {
        key: api.id.clone(),
        name,
        number,
        // `publishAt` is when the chapter went live; `readableAt` covers the rare entry that lacks it.
        date_upload: attrs
            .publish_at
            .as_deref()
            .or(attrs.readable_at.as_deref())
            .and_then(parse_rfc3339_millis),
        scanlator: Some(scanlators.join(", ")).filter(|joined| !joined.is_empty()),
    }
}

/// Parses an RFC 3339 timestamp (`2018-01-01T12:30:00+00:00`, and the `Z` / fractional-second forms)
/// into Unix epoch **milliseconds**, or `None` if it doesn't look like one.
///
/// Hand-rolled because a guest has no clock and no date crate worth ~100 KB of `.wasm`: MangaDex emits
/// exactly one format, and the ABI wants epoch millis. Second precision is enough — a fractional part
/// is skipped, not rounded.
fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    let field = |range: Range<usize>| -> Option<i64> { value.get(range)?.parse::<i64>().ok() };

    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Whatever follows the seconds is the zone, possibly behind a fractional part we don't need.
    let zone = value[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let zone_offset_seconds = match zone.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let magnitude = hours * 3600 + minutes * 60;
            if *sign == b'-' { -magnitude } else { magnitude }
        }
        Some(_) => return None,
    };

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some((seconds - zone_offset_seconds) * 1000)
}

/// Days from the Unix epoch to a proleptic-Gregorian date, by Howard Hinnant's `days_from_civil`.
/// Branch-free and exact for every year we'll ever see; the shifted-year trick puts the leap day last
/// so February's length never needs a special case.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Maps one **browse** entry to a wire [`Manga`]. Everything is best-effort: a missing title becomes a
/// placeholder rather than dropping the entry, and an absent cover/author is simply `None`.
///
/// `description` and `original_title` are left `None` on purpose — they aren't in [`ListAttributes`]
/// at all, so the grid never pays to parse them (see that type). [`MangaDex::manga_details`] fills
/// them in when a series is actually opened.
fn map_list_manga(api: &ApiManga<ListAttributes>) -> Manga {
    Manga {
        key: api.id.clone(),
        // The source owns its URL scheme, so the permalink is built here, not in the app.
        url: Some(format!("https://mangadex.org/title/{}", api.id)),
        title: pick_title(&api.attributes.title, &api.attributes.alt_titles),
        original_title: None,
        cover_url: api.cover_url(),
        author: api.author(),
        description: None,
        status: api.attributes.status.clone(),
    }
}

/// Maps the single-entity `/manga/{id}` response to a full wire [`Manga`] — [`map_list_manga`] plus the
/// two fields only the detail screen shows.
fn map_detail_manga(api: &ApiManga<DetailAttributes>) -> Manga {
    let attrs = &api.attributes;
    let title = pick_title(&attrs.title, &attrs.alt_titles);
    Manga {
        key: api.id.clone(),
        url: Some(format!("https://mangadex.org/title/{}", api.id)),
        title: title.clone(),
        original_title: original_title(&title, &attrs.title, &attrs.alt_titles, &attrs.original_language),
        cover_url: api.cover_url(),
        author: api.author(),
        description: pick_localized(&attrs.description),
        status: attrs.status.clone(),
    }
}

/// Picks a display string from a MangaDex language-keyed map: this source's language if present, else
/// any entry, else `None`. MangaDex titles/descriptions are `{"en": "...", "ja": "...", ...}`.
fn pick_localized(map: &BTreeMap<String, String>) -> Option<String> {
    map.get(LANG)
        .cloned()
        .or_else(|| map.values().next().cloned())
        .filter(|s| !s.is_empty())
}

/// The title to show, in this source's language wherever MangaDex has one.
///
/// `title` alone is not enough. MangaDex keys it by the *original* language, so a Korean or Chinese
/// series often carries only a romanization there — Solo Leveling's is literally
/// `{"ko-ro": "Na Honjaman Level-Up"}` — while the English title sits in `altTitles` as
/// `{"en": "Solo Leveling"}`. Reading `title` first and stopping (what [`pick_localized`] does on its
/// own) is what made the grid show romanized names.
///
/// So: the [`LANG`] entry of `title`, then the first `altTitles` entry offering [`LANG`], then
/// whatever `title` has (romanization or native script) so a series is never left untitled.
fn pick_title(title: &BTreeMap<String, String>, alt_titles: &[BTreeMap<String, String>]) -> String {
    title
        .get(LANG)
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            alt_titles
                .iter()
                .find_map(|alt| alt.get(LANG))
                .filter(|value| !value.is_empty())
                .cloned()
        })
        .or_else(|| pick_localized(title))
        .unwrap_or_else(|| "Untitled".to_string())
}

/// The series' own name, when it differs from the [`pick_title`] display name — for a translated title
/// that's the romanization or native script MangaDex keyed `title` by, which is exactly the entry the
/// display name stepped over. `None` when the two would read the same.
fn original_title(
    display: &str,
    title: &BTreeMap<String, String>,
    alt_titles: &[BTreeMap<String, String>],
    original_language: &str,
) -> Option<String> {
    title
        .values()
        .next()
        .cloned()
        .or_else(|| alt_titles.iter().find_map(|alt| alt.get(original_language).cloned()))
        .filter(|candidate| !candidate.is_empty() && candidate != display)
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

/// The JSON:API collection envelope every list endpoint returns, over manga or chapters alike.
#[derive(Deserialize)]
struct ApiList<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
    #[serde(default)]
    offset: i32,
    #[serde(default)]
    total: i32,
}

/// The single-entity envelope `/manga/{id}` returns, as opposed to [`ApiList`]'s collection.
#[derive(Deserialize)]
struct ApiEntity<T> {
    data: T,
}

/// One manga entry, generic over how much of `attributes` the caller asked for — [`ListAttributes`]
/// for browse, [`DetailAttributes`] for a single series. The relationship handling is identical
/// either way, so it lives here rather than in each mapper.
#[derive(Deserialize)]
#[serde(bound(deserialize = "A: Deserialize<'de> + Default"))]
struct ApiManga<A> {
    #[serde(default)]
    id: String,
    #[serde(default)]
    attributes: A,
    #[serde(default)]
    relationships: Vec<ApiRelationship>,
}

impl<A> ApiManga<A> {
    /// First relationship of the given type (e.g. `"cover_art"`, `"author"`), if inlined.
    fn relationship(&self, rel_type: &str) -> Option<&ApiRelationship> {
        self.relationships.iter().find(|rel| rel.rel_type == rel_type)
    }

    /// The 512px thumbnail URL built from the inlined `cover_art` file name.
    fn cover_url(&self) -> Option<String> {
        self.relationship("cover_art")
            .and_then(|rel| rel.attributes.as_ref())
            .and_then(|attrs| attrs.file_name.as_ref())
            .map(|file| format!("{COVER_BASE}/{}/{}.512.jpg", self.id, file))
    }

    fn author(&self) -> Option<String> {
        self.relationship("author")
            .and_then(|rel| rel.attributes.as_ref())
            .and_then(|attrs| attrs.name.clone())
    }
}

/// The attributes a **browse grid** needs: enough to title the tile, and the status the app persists
/// when a tapped result is cached.
///
/// `altTitles` is here despite the grid not showing it, because [`pick_title`] reads through it: the
/// English title of a Korean or Chinese series is routinely *only* in `altTitles`, with `title` holding
/// nothing but a romanization. `description` stays out — it is ~38% of a `/manga` page (105 KB of a
/// 272 KB "Popular" response) and nothing in browse renders it; [`DetailAttributes`] pays for the full
/// record once, when a series is actually opened.
///
/// Do **not** expect this to speed up a browse page. It was tried for that and measured no change: on
/// Android a 20-entry page costs ~3.2 s inside the WASM interpreter, of which mapping entries is 11 ms
/// — the rest is lexing bytes (base64-decoding the host's response envelope, then `serde_json` over the
/// raw JSON), and serde must lex a field it skips just the same as one it keeps. The cost is bytes
/// arriving, not fields declared. The fix that does move it is the ABI's, not this file's: passing the
/// response body as raw bytes instead of base64-inside-JSON removes ~40% of the call.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListAttributes {
    #[serde(default)]
    title: BTreeMap<String, String>,
    #[serde(default)]
    alt_titles: Vec<BTreeMap<String, String>>,
    #[serde(default)]
    status: Option<String>,
}

/// The full attribute set, parsed only by [`MangaDex::manga_details`] — one series, not twenty.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DetailAttributes {
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
struct ApiChapter {
    #[serde(default)]
    id: String,
    #[serde(default)]
    attributes: ApiChapterAttributes,
    #[serde(default)]
    relationships: Vec<ApiRelationship>,
}

impl ApiChapter {
    /// Whether Tanuki can actually open this chapter.
    ///
    /// `externalUrl` marks a chapter hosted off MangaDex (official publisher links) — the at-home
    /// server has no pages for it, so listing it would only produce a dead tap. A chapter that
    /// explicitly reports zero pages is delisted; one that reports none at all is kept, since an
    /// absent field is likelier a response shape we don't know than an empty chapter.
    fn is_readable(&self) -> bool {
        self.attributes.external_url.is_none() && self.attributes.pages.map_or(true, |pages| pages > 0)
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApiChapterAttributes {
    /// Chapter number as a string ("4", "4.5"); null on unnumbered oneshots.
    #[serde(default)]
    chapter: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    pages: Option<i32>,
    /// Set when the chapter lives on another site; such chapters are skipped.
    #[serde(default)]
    external_url: Option<String>,
    /// RFC 3339 timestamps.
    #[serde(default)]
    publish_at: Option<String>,
    #[serde(default)]
    readable_at: Option<String>,
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
