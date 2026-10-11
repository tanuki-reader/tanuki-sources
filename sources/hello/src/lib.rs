//! The smallest source that can exist: it describes itself and nothing else.
//!
//! Its only job is to answer one question — **can a `.wasm` built against this SDK be loaded,
//! version-checked, and have an export called, on both Chicory (Android) and wasmi (iOS)?** It makes
//! no network calls, so a failure here is unambiguously the memory ABI or the engine binding, never
//! a website changing its HTML.
//!
//! Once `listSources()` returns "Hello" on both platforms, the pipeline is proven and the next source
//! can be a real one. Keep this crate around after that — it doubles as a conformance fixture.

use tanuki_source_api::*;

struct Hello;

impl Source for Hello {
    fn meta() -> SourceMeta {
        SourceMeta {
            id: "hello".to_string(),
            name: "Hello".to_string(),
            lang: "en".to_string(),
            base_url: Some("https://example.com".to_string()),
            version: Some(3),
            // No icon: the app falls back to a lettered avatar, which is the path worth exercising
            // here since a real source (mangadex) covers the other one.
            icon_url: None,
            nsfw: false,
            // Every contract listing, so the source home has rails to render. None is backed by
            // manga_list — the default impl reports "unsupported", which is the behaviour we want to
            // see surface cleanly in the UI (each rail fails on its own) — except through `home`.
            listings: vec![Listing::Popular, Listing::Latest, Listing::New],
        }
    }

    /// The optional one-call home, answered from memory so a test can pin the export's wire shape
    /// (and every optional `Manga` field) without a network. Serves Popular only: the app must fetch
    /// the listings a home leaves out on its own.
    fn home(_request: SourceHomeRequest) -> Result<SourceHome, AbiError> {
        Ok(SourceHome {
            sections: vec![HomeSection {
                listing: Listing::Popular,
                entries: vec![Manga {
                    key: "hello-1".to_string(),
                    title: "Hello, World".to_string(),
                    latest_chapter: Some(12.5),
                    latest_chapter_name: Some("The Twelfth and a Half".to_string()),
                    updated_at: Some(1_760_000_000_000),
                    genres: vec!["Comedy".to_string(), "Slice of Life".to_string()],
                    ..Default::default()
                }],
            }],
        })
    }
}

export_source!(Hello);
