//! MangaPill — English, and the first source that scrapes **HTML** rather than reading a JSON API.
//!
//! There is no API and no HTML parser in the guest, so every page is cut up with string slicing
//! around markers the site's templates emit verbatim. If MangaPill reshuffles its markup, the marker
//! constants below are what to re-check.
//!
//! ## Pages (`https://mangapill.com`)
//! - `GET /search?q={q}&page={n}` → 50 cards a page. A next page exists when the pager links
//!   `page={n+1}`. An empty `q` alone returns nothing; with `type=manga` it is the catalog.
//! - `GET /` → the "Trending Mangas" section is the Popular listing (one page).
//! - `GET /mangas/new` → the 50 newest series (one page), same cards as search.
//! - `GET /chapters` → the 120 newest chapters, one card each; collapsed to series for Latest.
//! - `GET /manga/{id}/{slug}` → details and the full chapter list (no dates on the site).
//! - `GET /chapters/{id}/{slug}` → the reader, one `<img class="js-page">` per page.
//!
//! The image CDN 403s without `Referer: https://mangapill.com/`, covers included.

use std::collections::BTreeMap;
use tanuki_source_api::*;

const BASE: &str = "https://mangapill.com";
const REFERER: &str = "https://mangapill.com/";

/// The title `<div>` of a card on `/search` and `/`; the card's link and cover precede it.
const CARD_TITLE: &str = "font-black leading-tight line-clamp-2\">";
/// The series-title `<div>` of a card on `/chapters`.
const LATEST_CARD_TITLE: &str = "line-clamp-2 text-sm font-bold\">";

struct MangaPill;

impl Source for MangaPill {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "mangapill".to_string(),
            name: "MangaPill".to_string(),
            lang: "en".to_string(),
            base_url: Some(BASE.to_string()),
            version: Some(1),
            icon_url: None,
            nsfw: false,
            listings: vec![Listing::Popular, Listing::Latest, Listing::New],
        }
    }

    fn manga_list(request: MangaListRequest) -> Result<MangaPage, AbiError> {
        let query = request.query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        match (query, request.listing) {
            (None, Some(Listing::Popular)) => {
                let html = get_html(&format!("{BASE}/"))?;
                let trending = html.find("Trending Mangas").map_or("", |at| &html[at..]);
                Ok(MangaPage { entries: parse_cards(trending, CARD_TITLE), has_next_page: false })
            }
            (None, Some(Listing::Latest)) => {
                let html = get_html(&format!("{BASE}/chapters"))?;
                Ok(MangaPage { entries: parse_cards(&html, LATEST_CARD_TITLE), has_next_page: false })
            }
            (None, Some(Listing::New)) => {
                let html = get_html(&format!("{BASE}/mangas/new"))?;
                Ok(MangaPage { entries: parse_cards(&html, CARD_TITLE), has_next_page: false })
            }
            // A search, or the plain catalog: an empty search only lists anything with a filter set.
            _ => {
                let page = request.page.max(1);
                let mut params = vec![("q", query.unwrap_or("").to_string()), ("page", page.to_string())];
                if query.is_none() {
                    params.push(("type", "manga".to_string()));
                }
                let html = get_html(&format!("{BASE}/search?{}", query_string(&params)))?;
                Ok(MangaPage {
                    entries: parse_cards(&html, CARD_TITLE),
                    has_next_page: html.contains(&format!("page={}\"", page + 1)),
                })
            }
        }
    }

    fn manga_details(request: MangaDetailsRequest) -> Result<Manga, AbiError> {
        let key = non_blank_key(&request.manga_key)?;
        let html = get_html(&format!("{BASE}/manga/{key}"))?;
        parse_details(&html, key)
            .ok_or_else(|| AbiError::with_code("Could not read the MangaPill series page.", "parse"))
    }

    fn chapter_list(request: ChapterListRequest) -> Result<Vec<Chapter>, AbiError> {
        let key = non_blank_key(&request.manga_key)?;
        let html = get_html(&format!("{BASE}/manga/{key}"))?;
        Ok(parse_chapters(&html))
    }

    fn chapter_pages(request: ChapterPagesRequest) -> Result<Vec<Page>, AbiError> {
        let key = non_blank_key(&request.chapter_key)?;
        let html = get_html(&format!("{BASE}/chapters/{key}"))?;
        let pages = html
            .split("class=\"js-page\" data-src=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .enumerate()
            .map(|(index, url)| Page {
                index: index as i32,
                image_url: Some(url.to_string()),
                headers: referer(),
                ..Default::default()
            })
            .collect();
        Ok(pages)
    }
}

export_source!(MangaPill);

fn get_html(url: &str) -> Result<String, AbiError> {
    let response = fetch(&HttpRequest::get(url).header("Referer", REFERER))?;
    if !(200..300).contains(&response.status) {
        return Err(AbiError::with_code(format!("MangaPill returned HTTP {}", response.status), "http"));
    }
    Ok(response.text())
}

fn non_blank_key(key: &str) -> Result<&str, AbiError> {
    let key = key.trim().trim_matches('/');
    if key.is_empty() {
        return Err(AbiError::with_code("MangaPill needs a key for this lookup.", "not_found"));
    }
    Ok(key)
}

fn referer() -> BTreeMap<String, String> {
    BTreeMap::from([("Referer".to_string(), REFERER.to_string())])
}

/// Every card whose title `<div>` starts with `title_marker`. The text before each marker (back to
/// the previous one) holds that card's `/manga/` link and cover, so they are the *last* ones in it.
/// A card missing its link is skipped; a series seen twice (Latest lists chapters) is kept once.
fn parse_cards(html: &str, title_marker: &str) -> Vec<Manga> {
    let chunks: Vec<&str> = html.split(title_marker).collect();
    let mut entries: Vec<Manga> = Vec::new();
    for pair in chunks.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        let Some(key) = last_after(before, "href=\"/manga/") else { continue };
        if entries.iter().any(|manga| manga.key == key) {
            continue;
        }
        let title = text(after.split("</div>").next().unwrap_or(""));
        entries.push(Manga {
            url: Some(format!("{BASE}/manga/{key}")),
            key: key.to_string(),
            title,
            cover_url: last_after(before, "data-src=\"").map(str::to_string),
            cover_headers: referer(),
            // Only `/chapters` cards carry one (`<div …>#19</div>`); elsewhere `>#` could match page chrome.
            latest_chapter: (title_marker == LATEST_CARD_TITLE)
                .then(|| last_after(before, ">#").and_then(first_number))
                .flatten(),
            ..Default::default()
        });
    }
    entries
}

fn parse_details(html: &str, key: &str) -> Option<Manga> {
    let h1 = html.find("<h1")?;
    let (head, body) = html.split_at(h1);
    let title = text(between(body, ">", "</h1>")?);

    let status = between(body, ">Status</label>", "</div>")
        .map(|block| match text(block).to_ascii_lowercase().as_str() {
            "publishing" => "ongoing".to_string(),
            "finished" => "completed".to_string(),
            other => other.to_string(),
        });
    let genres = body
        .split("href=\"/search?genre=")
        .skip(1)
        .filter_map(|rest| between(rest, ">", "</a>"))
        .map(text)
        .collect();

    Some(Manga {
        key: key.to_string(),
        url: Some(format!("{BASE}/manga/{key}")),
        title,
        // The cover sits just before the title.
        cover_url: last_after(head, "data-src=\"").map(str::to_string),
        cover_headers: referer(),
        description: between(body, "<p class=\"text-sm text--secondary\">", "</p>")
            .map(text)
            .filter(|d| !d.is_empty()),
        status,
        genres,
        ..Default::default()
    })
}

fn parse_chapters(html: &str) -> Vec<Chapter> {
    let Some(at) = html.find("id=\"chapters\"") else { return Vec::new() };
    html[at..]
        .split("href=\"/chapters/")
        .skip(1)
        .filter_map(|rest| {
            let key = rest.split('"').next()?;
            let name = text(between(rest, ">", "</a>")?);
            // Some series prefix a numbering track ("Group 2 Chapter 386"): the number follows "Chapter".
            let number = first_number(name.rsplit("Chapter").next().unwrap_or(&name));
            // A bare "Chapter 12" repeats the number the app already shows; keep only names that add something.
            let plain = name.strip_prefix("Chapter ").is_some_and(|rest| rest.parse::<f64>().is_ok());
            Some(Chapter {
                key: key.to_string(),
                number,
                name: (!plain).then_some(name),
                ..Default::default()
            })
        })
        .collect()
}

/// The text between the first `start` and the `end` after it.
fn between<'a>(hay: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = &hay[hay.find(start)? + start.len()..];
    Some(&rest[..rest.find(end)?])
}

/// The text after the **last** `marker`, up to the next `"` (an attribute value) or `<`.
fn last_after<'a>(hay: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &hay[hay.rfind(marker)? + marker.len()..];
    rest.split(['"', '<']).next().filter(|value| !value.is_empty())
}

/// Markup → plain text. `strip_html` covers the common entities; the site also writes `&#34;`.
fn text(html: &str) -> String {
    strip_html(&html.replace("&#34;", "\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_and_latest_cards() {
        let search = r#"
            <div><a href="/manga/2/one-piece" class="relative block"><figure>
            <img data-src="https://cdn.example/i/2.webp" alt="x"/></figure></a>
            <a href="/manga/2/one-piece" class="mb-2">
            <div class="mt-3 font-black leading-tight line-clamp-2">One &amp; Piece</div>
            <div class="line-clamp-2 text-xs text-secondary mt-1">Alt</div></a></div>
            <div><a href="/manga/3/naruto"><img data-src="https://cdn.example/i/3.jpeg"/></a>
            <div class="mt-3 font-black leading-tight line-clamp-2">Naruto</div></div>"#;
        let cards = parse_cards(search, CARD_TITLE);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].key, "2/one-piece");
        assert_eq!(cards[0].title, "One & Piece");
        assert_eq!(cards[0].cover_url.as_deref(), Some("https://cdn.example/i/2.webp"));
        assert_eq!(cards[1].key, "3/naruto");
        assert_eq!(cards[1].latest_chapter, None);

        let latest = r#"
            <div><a href="/chapters/8-1019000/x-chapter-19"><img data-src="https://c/8.jpeg"/></a>
            <a href="/chapters/8-1019000/x-chapter-19"><div class="mt-3 text-lg font-black leading-tight">#19</div></a>
            <a href="/manga/8/x"><div class="line-clamp-2 text-sm font-bold">X</div></a></div>
            <div><a href="/chapters/8-1018000/x-chapter-18"><img data-src="https://c/8.jpeg"/></a>
            <div>#18</div><a href="/manga/8/x"><div class="line-clamp-2 text-sm font-bold">X</div></a></div>"#;
        let cards = parse_cards(latest, LATEST_CARD_TITLE);
        assert_eq!(cards.len(), 1, "same series collapses");
        assert_eq!(cards[0].latest_chapter, Some(19.0));
    }

    #[test]
    fn parses_details_and_chapters() {
        let html = r#"<img data-src="https://cdn.example/i/2.webp"/>
            <h1 class="font-bold">One Piece</h1>
            <p class="text-sm text--secondary">The &#34;Pirate King&#34;<br/>
            sets sail.</p>
            <label class="text-secondary">Status</label>
                        <div>publishing</div>
                    </div>
            <a class="text-sm" href="/search?genre=Action">Action</a>
            <div id="chapters"><a class="b" href="/chapters/2-11195000/one-piece-chapter-1195" title=" Chapter 1195">Chapter 1195</a>
            <a href="/chapters/2-10005500/one-piece-chapter-5.5">Chapter 5.5</a>
            <a href="/chapters/1-20386000/berserk-chapter-386">Group 2 Chapter 386</a></div>"#;
        let manga = parse_details(html, "2/one-piece").unwrap();
        assert_eq!(manga.title, "One Piece");
        assert_eq!(manga.description.as_deref(), Some("The \"Pirate King\" sets sail."));
        assert_eq!(manga.status.as_deref(), Some("ongoing"));
        assert_eq!(manga.genres, vec!["Action"]);
        assert_eq!(manga.cover_url.as_deref(), Some("https://cdn.example/i/2.webp"));

        let chapters = parse_chapters(html);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].key, "2-11195000/one-piece-chapter-1195");
        assert_eq!(chapters[0].name, None);
        assert_eq!(chapters[1].number, Some(5.5));
        assert_eq!(chapters[2].number, Some(386.0));
        assert_eq!(chapters[2].name.as_deref(), Some("Group 2 Chapter 386"));
    }
}
