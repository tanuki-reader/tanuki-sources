//! Nexus Mangás — pt-BR, served from a **Supabase** backend: PostgREST for the catalog and an edge
//! function for the reader.
//!
//! Every call carries the site's public `anon` key (it ships in the site's own bundle and only grants
//! what an anonymous visitor may read), as both `apikey` and a bearer token.
//!
//! ## API (`https://supabase.nexusmangas.com`)
//! - `GET /rest/v1/works?select=…&order=…&limit&offset` → a bare JSON array of works. PostgREST
//!   filters ride in the query (`slug=eq.{slug}`, `or=(title.ilike.*q*,…)`), embedded relations in
//!   `select` (`chapters(…)`, `work_genres(genres(name))`). No total, so a full page means "maybe more".
//! - `POST /functions/v1/read-chapter` `{"chapterId": uuid}` (with `x-nexus-client: reader-v3`) →
//!   `{ access: { allowed }, chapter: { pages: [url…] } }`. Page images on `assets.nexusmangas.com`
//!   need no headers.
//! - Covers are storage keys (`works/{id}/cover/…webp`) served from `assets.nexusmangas.com`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tanuki_source_api::*;

const BASE: &str = "https://nexusmangas.com";
const API: &str = "https://supabase.nexusmangas.com";
const ASSETS: &str = "https://assets.nexusmangas.com";
/// The site's public Supabase `anon` key (role `anon`, expires 2126).
const ANON_KEY: &str = concat!(
    "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.",
    "eyJpc3MiOiJzdXBhYmFzZSIsImlhdCI6MTc4NzgwMjAwMCwiZXhwIjo0OTQzNDc1NjAwLCJyb2xlIjoiYW5vbiJ9.",
    "Cnl8Jw2DeKe84OAkmJYfO33xlcZsw0TC2Nw_il0tpRs",
);
const PAGE_SIZE: i32 = 30;
const LIST_COLUMNS: &str = "slug,title,cover_url,status,type";
const DETAIL_COLUMNS: &str =
    "slug,title,description,cover_url,alternative_title,status,type,author,work_genres(genres(name))";

struct NexusMangas;

impl Source for NexusMangas {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "nexusmangas".to_string(),
            name: "Nexus Mangás".to_string(),
            lang: "pt-BR".to_string(),
            base_url: Some(BASE.to_string()),
            version: Some(1),
            icon_url: None,
            // Mixed, not adult-only: adult works are filtered per request (see manga_list).
            nsfw: false,
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }

    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let page = request.page.max(1);
        let order = match request.listing {
            Some(Listing::Popular) => "avg_rating.desc.nullslast",
            _ => "updated_at.desc",
        };
        let mut params = vec![
            ("select", LIST_COLUMNS.to_string()),
            ("order", format!("{order},id.desc")),
            ("limit", PAGE_SIZE.to_string()),
            ("offset", ((page - 1) * PAGE_SIZE).to_string()),
        ];
        // The catalog mixes in adult works, each tagged with a `content_rating`.
        if !request.allow_sensitive {
            params.push(("content_rating", "not.in.(ADULT,EROTICA,SEXUAL_VIOLENCE)".to_string()));
        }
        // PostgREST's filter syntax uses `,()*`; strip them so a title can't break the `or` clause.
        let query: String = request.query.as_deref().unwrap_or("").replace([',', '(', ')', '*'], " ");
        let query = query.trim();
        if !query.is_empty() {
            params.push(("or", format!("(title.ilike.*{query}*,alternative_title.ilike.*{query}*)")));
        }

        let works: Vec<WorkDto> = get_json(&format!("{API}/rest/v1/works?{}", query_string(&params)))?;
        Ok(MangaPage {
            has_next_page: works.len() == PAGE_SIZE as usize,
            entries: works.into_iter().map(WorkDto::into_wire).collect(),
        })
    }

    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        Ok(work(&request.manga_key, DETAIL_COLUMNS)?.into_wire())
    }

    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let work = work(&request.manga_key, "slug,chapters(id,number,title,published_at)")?;
        Ok(work.chapters.into_iter().map(ChapterDto::into_wire).collect())
    }

    fn chapter_pages(request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        let id = request.chapter_key.trim();
        if id.is_empty() {
            return Err(AbiError::with_code("A page list needs a chapter key.", "not_found"));
        }
        let body = serde_json::json!({ "chapterId": id }).to_string();
        let mut http = authorized(HttpRequest::get(format!("{API}/functions/v1/read-chapter")))
            .header("x-nexus-client", "reader-v3")
            .header("Content-Type", "application/json");
        http.method = "POST".to_string();
        http.body_base64 = Some(STANDARD.encode(body));

        let read: ReadChapterDto = decode(fetch(&http)?)?;
        if !read.access.allowed {
            return Err(AbiError::with_code("This chapter is not available to read.", "unsupported"));
        }
        let pages = read.chapter.map(|c| c.pages).unwrap_or_default();
        Ok(pages
            .into_iter()
            .enumerate()
            .map(|(index, url)| Page { index: index as i32, image_url: Some(url), ..Default::default() })
            .collect())
    }
}

export_source!(NexusMangas);

/// One work by slug, with the given `select` columns.
fn work(slug: &str, columns: &str) -> Result<WorkDto, AbiError> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err(AbiError::with_code("A lookup needs a manga key.", "not_found"));
    }
    let params = [("select", columns.to_string()), ("slug", format!("eq.{slug}")), ("limit", "1".to_string())];
    let works: Vec<WorkDto> = get_json(&format!("{API}/rest/v1/works?{}", query_string(&params)))?;
    works
        .into_iter()
        .next()
        .ok_or_else(|| AbiError::with_code(format!("Nexus Mangás has no series {slug}."), "not_found"))
}

fn authorized(request: HttpRequest) -> HttpRequest {
    request
        .header("apikey", ANON_KEY)
        .header("Authorization", format!("Bearer {ANON_KEY}"))
        .header("Accept", "application/json")
}

fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, AbiError> {
    decode(fetch(&authorized(HttpRequest::get(url)))?)
}

fn decode<T: DeserializeOwned>(response: HttpResponse) -> Result<T, AbiError> {
    if !(200..300).contains(&response.status) {
        return Err(AbiError::with_code(format!("Nexus Mangás returned HTTP {}", response.status), "http"));
    }
    serde_json::from_slice(response.body())
        .map_err(|e| AbiError::with_code(format!("Could not parse Nexus Mangás response: {e}"), "parse"))
}

// --- API response subset (only the fields we read) ----------------------------------------------

#[derive(Deserialize)]
struct WorkDto {
    slug: String,
    #[serde(default)]
    title: String,
    description: Option<String>,
    cover_url: Option<String>,
    alternative_title: Option<String>,
    status: Option<String>,
    /// `MANGA`, `MANHWA`, `MANHUA`, … shown as the first genre.
    #[serde(rename = "type")]
    kind: Option<String>,
    author: Option<String>,
    #[serde(default)]
    work_genres: Vec<WorkGenreDto>,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

impl WorkDto {
    fn into_wire(self) -> Manga {
        let mut genres: Vec<String> = self.kind.as_deref().map(capitalize).into_iter().collect();
        genres.extend(self.work_genres.into_iter().filter_map(|g| g.genres).map(|g| g.name));
        Manga {
            url: Some(format!("{BASE}/obra/{}", self.slug)),
            title: if self.title.is_empty() { self.slug.clone() } else { self.title },
            key: self.slug,
            original_title: non_blank(self.alternative_title),
            cover_url: non_blank(self.cover_url).map(|key| format!("{ASSETS}/{}", key.trim_start_matches('/'))),
            author: non_blank(self.author),
            description: non_blank(self.description).map(|d| strip_html(&d)),
            // RELEASING/COMPLETED/HIATUS/CANCELLED; the host matches the rest by name.
            status: self.status.map(|s| if s == "RELEASING" { "ongoing".to_string() } else { s }),
            genres,
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
struct WorkGenreDto {
    genres: Option<GenreDto>,
}

#[derive(Deserialize)]
struct GenreDto {
    name: String,
}

#[derive(Deserialize)]
struct ChapterDto {
    id: String,
    number: f64,
    title: Option<String>,
    published_at: Option<String>,
}

impl ChapterDto {
    fn into_wire(self) -> Chapter {
        Chapter {
            key: self.id,
            // The chapter's own title ("Fim"); the app already shows the number.
            name: non_blank(self.title),
            number: Some(self.number),
            date_upload: self.published_at.as_deref().and_then(parse_datetime_millis),
            scanlator: None,
        }
    }
}

#[derive(Deserialize)]
struct ReadChapterDto {
    access: AccessDto,
    chapter: Option<ReadChapterPagesDto>,
}

#[derive(Deserialize)]
struct AccessDto {
    allowed: bool,
}

#[derive(Deserialize)]
struct ReadChapterPagesDto {
    #[serde(default)]
    pages: Vec<String>,
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// `MANHWA` → `Manhwa`.
fn capitalize(word: &str) -> String {
    let lower = word.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}
