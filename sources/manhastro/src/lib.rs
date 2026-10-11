//! Manhastro — Tanuki's first **pt-BR** source.
//!
//! # NOT USABLE: the site requires an account
//!
//! **Manhastro is gated behind a site login, so this source does not work end to end and should be
//! treated as unfinished.** Tanuki has no source-authentication mechanism yet — no login UI, no
//! credential storage, no way for a guest to carry a session it did not earn itself — so there is
//! nothing this file can do about it. It is kept, and kept building, because everything below the
//! login is correct and re-usable the moment source auth exists; it is not kept because it works.
//!
//! What is confirmed from outside an account: the metadata endpoints (`/dados`, `/rank/diario`,
//! `/lancamentos`, `/dados/{id}`) still answer anonymously, and so does `/paginas/{id}` for some
//! chapters, which is why this reads like a working source right up until you try to use it. Do not
//! take an anonymous `200` from those as evidence the gate is gone — check the reader, in the app,
//! against a series you have not already cached.
//!
//! Two other things to re-check whenever this is picked up again, both observed after it was written:
//! `/paginas/{id}` now returns **sliced** images (`1_part0.avif`, `1_part1.avif` — one page cut into
//! several files), which this guest would emit as separate pages and the reader would show as half
//! images; and the origin intermittently answers `525`.
//!
//! See `docs/decisions.md` (2026-07-29) for the fuller note, and [`Source::meta`] for how this is
//! surfaced. The rest of this doc describes the API as it was built against.
//!
//! ---
//!
//! No signing (MangaFire's `vrf`) and no proof-of-work gate (MangaLivre's): every endpoint on
//! `api2.manhastro.net` answers a plain `GET` with plain JSON, and the page images are direct URLs
//! their CDN serves without a `Referer`.
//!
//! ## API shape (`https://api2.manhastro.net`)
//!
//! Every response is `{ "success": bool, "data": … }`.
//!
//! - `GET /dados?page={n}` → the catalog, 100 series a page, plus a `meta` with `has_more`.
//!   The same endpoint carries the site's own filters; this source uses two of them:
//!   `nome={query}` to search and `manga_id={id}` to fetch one series ([`Manhastro::manga_details`]).
//! - `GET /rank/diario` and `GET /lancamentos` → the Popular and Latest listings. Both return **whole
//!   series records**, not id stubs, so neither needs a second lookup to render a grid.
//! - `GET /dados/{mangaId}` → that series' chapters (`capitulo_id`, `capitulo_nome`, `capitulo_data`).
//! - `GET /paginas/{capituloId}` → `{ data: { chapter: { baseUrl, hash, data: [file…] } } }`;
//!   a page URL is `{baseUrl}/{hash}/{file}`.
//!
//! ## Titles are bilingual
//!
//! Manhastro carries both the Brazilian title/synopsis (`titulo_brasil`, `descricao_brasil`) and the
//! original-language ones (`titulo`, `descricao`). Being a pt-BR source, the Brazilian text is the
//! display text and the other becomes [`Manga::original_title`] — see [`MangaDto::display_title`].

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tanuki_source_api::*;

/// The site, and the API host it talks to. They are different hosts on purpose — `base_url` is the
/// permalink origin shown to a reader, `API_BASE` is where the JSON lives.
const BASE: &str = "https://manhastro.net";
const API_BASE: &str = "https://api2.manhastro.net";
/// Manhastro's own logo, the only raster brand asset the site serves (the favicon is an `.ico`, which
/// the app's image loader can't decode).
const ICON_URL: &str =
    "https://capa.manhastro.net/wp-content/uploads/2024/12/25231353/manhastrologov2.png";

struct Manhastro;

impl Source for Manhastro {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "manhastro".to_string(),
            name: "Manhastro".to_string(),
            lang: "pt-BR".to_string(),
            base_url: Some(BASE.to_string()),
            version: Some(3),
            icon_url: Some(ICON_URL.to_string()),
            // The catalog mixes all-ages series with adult ones and the site has no content-rating
            // filter to separate them, so the source as a whole is flagged.
            // Mixed, not adult-only: each title carries its own `adulta` flag (see manga_list).
            nsfw: false,
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }

    /// One catalog/search page.
    ///
    /// A non-blank query goes to the paginated `/dados` search. The two listings each have their own
    /// unpaginated endpoint that returns whole records, so they report no next page rather than
    /// pretending `page=2` would work.
    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let page = request.page.max(1);

        if let Some(query) = request.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            let params = [("page", page.to_string()), ("nome", query.to_string())];
            let response: ApiResponse<Vec<MangaDto>> =
                get_json(&format!("{API_BASE}/dados?{}", query_string(&params)))?;
            return Ok(response.to_page(request.allow_sensitive));
        }

        let path = match request.listing {
            Some(Listing::Latest) => "/lancamentos".to_string(),
            Some(Listing::Popular) => "/rank/diario".to_string(),
            // No listing asked for (or `New`, which this source doesn't declare): the plain catalog.
            Some(Listing::New) | None => format!("/dados?page={page}"),
        };
        let response: ApiResponse<Vec<MangaDto>> = get_json(&format!("{API_BASE}{path}"))?;

        // `/lancamentos` is a feed of chapter releases keyed by series, so one series appears once per
        // recent chapter. Collapse it, or Latest shows the same cover several times.
        let mut page_result = response.to_page(request.allow_sensitive);
        if matches!(request.listing, Some(Listing::Latest)) {
            let mut seen = Vec::new();
            page_result.entries.retain(|manga| {
                let first_time = !seen.contains(&manga.key);
                if first_time {
                    seen.push(manga.key.clone());
                }
                first_time
            });
        }
        Ok(page_result)
    }

    /// The full record for one series. `/dados` filtered by `manga_id` returns a one-element list
    /// rather than a bare entity, so this reads the first element and treats an empty list as
    /// "not found" — the same shape the catalog already parses, for free.
    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let manga_id = request.manga_key.trim();
        if manga_id.is_empty() {
            return Err(AbiError::with_code("A manga lookup needs a manga key.", "not_found"));
        }

        let response: ApiResponse<Vec<MangaDto>> =
            get_json(&format!("{API_BASE}/dados?manga_id={}", percent_encode(manga_id)))?;
        response
            .data
            .first()
            .map(MangaDto::to_wire)
            .ok_or_else(|| AbiError::with_code(format!("Manhastro has no series {manga_id}."), "not_found"))
    }

    /// Every chapter of one series. Unpaginated at the source, so this is a single request.
    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let manga_id = request.manga_key.trim();
        if manga_id.is_empty() {
            return Err(AbiError::with_code("A chapter list needs a manga key.", "not_found"));
        }

        let response: ApiResponse<Vec<ChapterDto>> =
            get_json(&format!("{API_BASE}/dados/{}", percent_encode(manga_id)))?;
        Ok(response.data.iter().map(ChapterDto::to_wire).collect())
    }

    /// The ordered page images of one chapter, as `{baseUrl}/{hash}/{file}`. Manhastro's CDN serves
    /// these without a `Referer`, so no per-page headers are needed (verified against the live host).
    fn chapter_pages(request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        let chapter_id = request.chapter_key.trim();
        if chapter_id.is_empty() {
            return Err(AbiError::with_code("A page list needs a chapter key.", "not_found"));
        }

        let response: ApiResponse<PageDataDto> =
            get_json(&format!("{API_BASE}/paginas/{}", percent_encode(chapter_id)))?;
        let Some(chapter) = response.data.chapter else {
            return Ok(Vec::new());
        };

        let base = chapter.base_url.trim_end_matches('/');
        let hash = chapter.hash.trim_matches('/');
        let pages = chapter
            .data
            .iter()
            .enumerate()
            .map(|(index, file)| Page {
                index: index as i32,
                image_url: Some(format!("{base}/{hash}/{file}")),
                ..Default::default()
            })
            .collect();
        Ok(pages)
    }
}

export_source!(Manhastro);

/// GETs a Manhastro endpoint and decodes its JSON, turning a non-2xx status into an `"http"` error and
/// an unparseable body into a `"parse"` one.
fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, AbiError> {
    let response = fetch(&HttpRequest::get(url))?;
    if !(200..300).contains(&response.status) {
        return Err(AbiError::with_code(
            format!("Manhastro returned HTTP {}", response.status),
            "http",
        ));
    }
    serde_json::from_slice(response.body())
        .map_err(|e| AbiError::with_code(format!("Could not parse Manhastro response: {e}"), "parse"))
}

// --- API response subset (only the fields we read) ----------------------------------------------

/// The `{ success, data, meta? }` envelope every endpoint returns. `meta` is only present on the
/// paginated `/dados`; the listing endpoints omit it, which is what makes them report no next page.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de> + Default"))]
struct ApiResponse<T> {
    #[serde(default)]
    data: T,
    #[serde(default)]
    meta: Option<MetaDto>,
}

impl ApiResponse<Vec<MangaDto>> {
    /// The page, minus adult titles unless the user allows them. The API has no adult filter, so this
    /// one is client-side: a page can come back shorter than the site's, never with a hole the user sees.
    fn to_page(&self, allow_sensitive: bool) -> MangaPage {
        MangaPage {
            entries: self
                .data
                .iter()
                .filter(|manga| allow_sensitive || !manga.adulta)
                .map(MangaDto::to_wire)
                .collect(),
            has_next_page: self.meta.as_ref().map_or(false, |meta| meta.has_more),
        }
    }
}

#[derive(Deserialize)]
struct MetaDto {
    #[serde(default)]
    has_more: bool,
}

/// One series. `titulo`/`descricao` hold the original-language text and `*_brasil` the translated
/// text; either side can be null or empty, so both are treated as optional throughout.
#[derive(Deserialize)]
struct MangaDto {
    manga_id: i64,
    #[serde(default)]
    titulo: String,
    #[serde(default)]
    titulo_brasil: Option<String>,
    #[serde(default)]
    descricao: Option<String>,
    #[serde(default)]
    descricao_brasil: Option<String>,
    /// Cover URL, sometimes **scheme-relative** (`capa.manhastro.net/…`) — see [`MangaDto::cover_url`].
    #[serde(default)]
    imagem: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    generos: Vec<String>,
    /// Whether the title is adult content.
    #[serde(default)]
    adulta: bool,
    /// When the series last got a chapter ("yyyy-MM-dd HH:mm:ss", UTC like every date here).
    #[serde(default)]
    ultimo_capitulo: Option<String>,
    /// Only on `/lancamentos` rows: the release this row announces.
    #[serde(default)]
    capitulo_nome: Option<String>,
    #[serde(default)]
    capitulo_data: Option<String>,
}

impl MangaDto {
    fn to_wire(&self) -> Manga {
        let title = self.display_title();
        Manga {
            key: self.manga_id.to_string(),
            url: Some(format!("{BASE}/manga/{}", self.manga_id)),
            original_title: non_empty(&self.titulo).filter(|original| *original != title),
            title,
            cover_url: self.cover_url(),
            // Manhastro's cover host serves images with no Referer requirement (verified live).
            cover_headers: Default::default(),
            description: self
                .descricao_brasil
                .as_deref()
                .and_then(non_empty)
                .or_else(|| self.descricao.as_deref().and_then(non_empty))
                .map(|text| strip_html(&text)),
            status: self.status.as_deref().and_then(map_status),
            // Manhastro's catalog carries no author field, and `scan_atual` (the translating group) is
            // null on nearly every record, so neither is worth reading.
            author: None,
            genres: self.generos.iter().filter_map(|g| non_empty(g)).collect(),
            // A `/lancamentos` row names its release; elsewhere only the date of the newest is known.
            latest_chapter: self.capitulo_nome.as_deref().and_then(first_number),
            latest_chapter_name: self.capitulo_nome.as_deref().and_then(non_empty),
            updated_at: self
                .capitulo_data
                .as_deref()
                .or(self.ultimo_capitulo.as_deref())
                .and_then(parse_datetime_millis),
            // Manhastro links to no other site, so its series are matched by title search instead.
            ..Default::default()
        }
    }

    /// The Brazilian title where there is one, else the original. A pt-BR source shows pt-BR text.
    fn display_title(&self) -> String {
        self.titulo_brasil
            .as_deref()
            .and_then(non_empty)
            .or_else(|| non_empty(&self.titulo))
            .unwrap_or_else(|| format!("#{}", self.manga_id))
    }

    /// Covers arrive both fully-qualified and bare (`capa.manhastro.net/wp-content/…`); the app needs
    /// an absolute URL either way.
    fn cover_url(&self) -> Option<String> {
        let raw = self.imagem.as_deref().and_then(non_empty)?;
        Some(if raw.starts_with("http://") || raw.starts_with("https://") {
            raw
        } else {
            format!("https://{}", raw.trim_start_matches("//"))
        })
    }
}

#[derive(Deserialize)]
struct ChapterDto {
    capitulo_id: i64,
    #[serde(default)]
    capitulo_nome: String,
    #[serde(default)]
    capitulo_data: Option<String>,
}

impl ChapterDto {
    fn to_wire(&self) -> Chapter {
        Chapter {
            key: self.capitulo_id.to_string(),
            name: non_empty(&self.capitulo_nome),
            // There is no numeric field — the number lives in the name ("Capitulo 12.5").
            number: first_number(&self.capitulo_nome),
            // `capitulo_data` is "yyyy-MM-dd HH:mm:ss" with no zone; read as UTC.
            date_upload: self.capitulo_data.as_deref().and_then(parse_datetime_millis),
            scanlator: None,
        }
    }
}

#[derive(Deserialize, Default)]
struct PageDataDto {
    /// Absent when the "chapter" is a text release rather than images (`data.text` is then true).
    #[serde(default)]
    chapter: Option<ChapterDataDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDataDto {
    #[serde(default)]
    base_url: String,
    /// Directory path under `base_url`, itself containing a slash (`manga_<id>/capitulo-<n>-<slug>`).
    #[serde(default)]
    hash: String,
    /// Page file names, in reading order.
    #[serde(default = "Vec::new")]
    data: Vec<String>,
}

/// Maps Manhastro's two status values to an ABI `PublishStatus` name; anything else is `None`, which
/// the host reads as `UNKNOWN`.
fn map_status(status: &str) -> Option<String> {
    let name = match status.trim().to_ascii_lowercase().as_str() {
        "on-going" | "ongoing" => "ongoing",
        "end" | "completed" => "completed",
        _ => return None,
    };
    Some(name.to_string())
}

/// The trimmed string, or `None` when it is blank — the site writes `""` about as often as `null`.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
