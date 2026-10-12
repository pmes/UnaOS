//! AETHERSELECT (SR69) proof: the text a whole-document selection copies is Chromium's `innerText`.
//!
//! - subject: each page of `tests/data/innertext/` loaded into an [`aether::AetherEngine`], Ctrl/Cmd+A
//!   (`select_all`), then `selected_text()` — the selection serialized by the HTML "rendered text
//!   collection steps" over Aether's laid-out tree (what `BrowserCopy` puts on the clipboard and what
//!   `getSelection().toString()` returns);
//! - oracle: `tests/data/innertext/chromium.json`, Chromium's `document.body.innerText` for each page
//!   (produced by `tests/innertext/chromium.mjs`; its sha-256 is pinned below so a silently edited vector
//!   fails the test).
//!
//! Every page must match byte for byte.

use std::path::{Path, PathBuf};

const VECTORS_SHA256: &str = "86187c756504b402a32ca5535a50ffdd96fc0b3807723555c259ef06a6598222";

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/innertext")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn whole_document_selection_text_equals_chromium_inner_text() {
    let raw = std::fs::read(data().join("chromium.json")).expect("chromium.json");
    assert_eq!(hex(&crypto_core::sha2::sha256(&raw)), VECTORS_SHA256, "innerText vectors changed: re-pin");
    let vectors: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let vectors = vectors.as_object().unwrap();
    assert_eq!(vectors.len(), 10, "ten pages");
    let mut fails = Vec::new();
    for (name, v) in vectors {
        let want = v["innerText"].as_str().unwrap();
        let path = data().join(name);
        let html = std::fs::read_to_string(&path).unwrap();
        let mut engine = aether::AetherEngine::new();
        engine.handle_event(aether::api::events::Event::Resize(800, 600));
        engine.load_html(&format!("file://{}", path.display()), &html, true);
        engine.select_all();
        let got = engine.selected_text();
        let ok = got == want;
        println!("innertext {name:20} {:4} bytes  {}", want.len(), if ok { "EQUAL" } else { "DIFF" });
        if !ok {
            println!("  want {want:?}\n  got  {got:?}");
            fails.push(name.clone());
        }
        // The script side answers the same text.
        if let Some(js) = engine.js_engine.as_mut() {
            let s = js.eval_string("getSelection().toString()").unwrap();
            assert_eq!(s, got, "{name}: getSelection().toString() vs the engine's copy");
        }
    }
    assert!(fails.is_empty(), "innerText differs on {fails:?}");
}
