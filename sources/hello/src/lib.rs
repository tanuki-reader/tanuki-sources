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
            version: Some(2),
            // No icon: the app falls back to a lettered avatar, which is the path worth exercising
            // here since a real source (mangadex) covers the other one.
            icon_url: None,
            nsfw: false,
            // Both contract listings, so the Browse screen has two tabs to render. Neither is backed
            // by manga_list yet — the default impl reports "unsupported", which is the behaviour we
            // want to see surface cleanly in the UI.
            listings: vec![Listing::Popular, Listing::Latest],
        }
    }
}

export_source!(Hello);
