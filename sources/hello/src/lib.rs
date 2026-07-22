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
            version: Some(1),
            nsfw: false,
            // Two tabs so the Browse screen has something to render once it exists. Neither is
            // backed by manga_list yet — the default impl reports "unsupported", which is the
            // behaviour we want to see surface cleanly in the UI.
            listings: vec![
                Listing { key: "popular".to_string(), name: "Popular".to_string() },
                Listing { key: "latest".to_string(), name: "Latest Updates".to_string() },
            ],
        }
    }
}

export_source!(Hello);
