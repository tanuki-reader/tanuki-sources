//! Golden wire fixtures: the guest half of the contract check (the host half is `WireFixtureTest` in
//! `:core:source`). Both read `fixtures/v4/`, so a shape that drifts on either side fails that side.
//!
//! Guest → host types are built here and serialized; the JSON must equal the fixture. Host → guest
//! types are deserialized from the fixture and checked field by field. The `full` fixtures set every
//! field, so a field added to a struct here forces the fixture (and so `Wire.kt`) to follow.

use std::collections::BTreeMap;
use tanuki_source_api::*;

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/fixtures/v{}/{name}.json", env!("CARGO_MANIFEST_DIR"), ABI_VERSION);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn assert_writes<T: serde::Serialize>(value: &T, name: &str) {
    assert_eq!(serde_json::to_value(value).unwrap(), fixture(name), "{name}");
}

fn read<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_value(fixture(name)).unwrap()
}

fn headers(name: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(name.to_string(), value.to_string())])
}

fn minimal_manga() -> Manga {
    Manga { key: "a1b2".into(), title: "Solo Leveling".into(), ..Default::default() }
}

#[test]
fn envelopes() {
    let ok = Envelope { ok: Some(MangaPage::default()), error: None };
    assert_writes(&ok, "envelope.ok");
    let error: Envelope<MangaPage> =
        Envelope { ok: None, error: Some(AbiError::with_code("HTTP 503 from mangadex.org", "http")) };
    assert_writes(&error, "envelope.error");
}

#[test]
fn source_meta() {
    let full = SourceMeta {
        id: "mangadex".into(),
        name: "MangaDex".into(),
        lang: "en".into(),
        base_url: Some("https://mangadex.org".into()),
        version: Some(9),
        icon_url: Some("https://mangadex.org/favicon.png".into()),
        nsfw: true,
        listings: vec![Listing::Popular, Listing::Latest, Listing::New],
    };
    assert_writes(&full, "source_meta.full");
    let minimal = SourceMeta {
        id: "hello".into(),
        name: "Hello".into(),
        lang: "en".into(),
        base_url: None,
        version: None,
        icon_url: None,
        nsfw: false,
        listings: vec![],
    };
    assert_writes(&minimal, "source_meta.minimal");
}

#[test]
fn manga() {
    let full = Manga {
        key: "a1b2".into(),
        url: Some("https://mangadex.org/title/a1b2".into()),
        title: "Solo Leveling".into(),
        original_title: Some("나 혼자만 레벨업".into()),
        cover_url: Some("https://uploads.mangadex.org/covers/a1b2/c.jpg".into()),
        cover_headers: headers("Referer", "https://mangadex.org/"),
        author: Some("Chugong".into()),
        description: Some("A weak hunter gets a second chance.".into()),
        status: Some("COMPLETED".into()),
        latest_chapter: Some(200.5),
        latest_chapter_name: Some("Epilogue".into()),
        updated_at: Some(1727438400000),
        genres: vec!["Action".into(), "Fantasy".into()],
        anilist_id: Some(105398),
        mal_id: Some(121496),
        mangadex_id: Some("32d76d19-8a05-4db0-9fc2-e0b0648fe9d0".into()),
    };
    assert_writes(&full, "manga.full");
    assert_writes(&minimal_manga(), "manga.minimal");
}

#[test]
fn pages_and_home() {
    assert_writes(&MangaPage::default(), "manga_page.empty");
    assert_writes(&MangaPage { entries: vec![minimal_manga()], has_next_page: true }, "manga_page.full");
    let home = SourceHome {
        sections: vec![HomeSection { listing: Listing::Latest, entries: vec![minimal_manga()] }],
    };
    assert_writes(&home, "source_home.full");
}

#[test]
fn chapter_and_page() {
    let chapter = Chapter {
        key: "ch-1".into(),
        name: Some("The Weakest Hunter".into()),
        number: Some(1.5),
        date_upload: Some(1727438400000),
        scanlator: Some("Asura".into()),
    };
    assert_writes(&chapter, "chapter.full");
    let page = Page {
        index: 3,
        image_url: Some("https://cdn.example/p3.jpg".into()),
        url: Some("https://example/p/3".into()),
        headers: headers("Referer", "https://example/"),
    };
    assert_writes(&page, "page.full");
}

#[test]
fn http_request() {
    let request = HttpRequest {
        method: "POST".into(),
        body_base64: Some("e30=".into()),
        ..HttpRequest::get("https://api.mangadex.org/manga").header("Accept", "application/json")
    };
    assert_writes(&request, "http_request.full");
}

#[test]
fn requests() {
    assert!(read::<SourceHomeRequest>("source_home_request.full").allow_sensitive);

    let list: MangaListRequest = read("manga_list_request.full");
    assert_eq!(list.query.as_deref(), Some("solo"));
    assert_eq!(list.page, 2);
    assert_eq!(list.listing, Some(Listing::Latest));
    assert!(list.allow_sensitive);

    let unknown: MangaListRequest = read("manga_list_request.unknown_listing");
    assert_eq!(unknown.listing, None, "an unknown listing degrades to the default catalog");
    assert!(!unknown.allow_sensitive, "an absent flag reads false");

    assert_eq!(read::<MangaDetailsRequest>("manga_details_request.full").manga_key, "a1b2");
    assert_eq!(read::<ChapterListRequest>("chapter_list_request.full").manga_key, "a1b2");
    assert_eq!(read::<ChapterPagesRequest>("chapter_pages_request.full").chapter_key, "ch-1");
}
