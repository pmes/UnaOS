//! AETHERINPUT (SR68): form submission bodies byte-equal to Chromium's.
//!
//! Each `tests/data/forms_oracle/<name>.html` fixture was served to Chromium (Playwright, chromium-1194)
//! at `http://oracle.test/<name>.html`, its `#go` submit button clicked, and the submission request it
//! sent recorded as `<name>.expect` (method, URL, Content-Type, body; the multipart boundary replaced by
//! `<B>`) by `tests/data/forms_oracle/oracle.mjs`. The vectors are sha256-pinned below, so an edited
//! vector fails loudly instead of quietly agreeing with a changed engine.
//!
//! Aether loads the same fixture at the same URL and builds the submission the same button's click
//! stages (`AetherEngine::submission_for`: the entry list of HTML §4.10.21.4 and its encoding), and the
//! bytes must match. The request URL never carries a fragment, so Aether's navigation URL is compared
//! without one.

use base64::Engine as _;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/forms_oracle");

const PINNED: &[(&str, &str)] = &[
    ("formattrs", "bc3c5535667be5a14f810aad0ae2c4a3bb4a25f04e0fbe84e7ade5a18bc9a14a"),
    ("get-replaces-query", "9b2bacd94d59c25c4d8c85a1fb8adc92454283bcb69767f791896e666fed0d36"),
    ("getform-postmultipart", "05ce44709af50a59dc74b41948c5212e5b5548f4333fb6a0e4611f02981be84e"),
    ("getform-postplain", "8c85e3918b860d3587a786feaa23207a1e27053d5fc0ff0df6394b0f3ce54654"),
    ("multipart", "07878b7b72288dce27ea5f10269f0b3f8f84cb8b4f32ec43b7e9cc7c64091c32"),
    ("textplain", "e5997957499d0e8959c0e08dac5093aadb0128efa2f1d1a6d44460f4182c1595"),
    ("urlencoded", "b89c6714ae76fcd0a5845ff03d7cb64ad8afa85de628f83cb86b8c00a43c121c"),
];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

struct Expect {
    method: String,
    url: String,
    content_type: String,
    body: Vec<u8>,
}

fn expect(name: &str, sha: &str) -> Expect {
    let raw = std::fs::read(format!("{DIR}/{name}.expect")).expect("vector");
    assert_eq!(hex(&crypto_core::sha2::sha256(&raw)), sha, "{name}.expect: sha256 drifted from its pin");
    let text = String::from_utf8(raw).unwrap();
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).map(|v| v.trim_start().to_string()))
            .unwrap_or_else(|| panic!("{name}: no {k}"))
    };
    Expect {
        method: field("METHOD"),
        url: field("URL"),
        content_type: field("CONTENT-TYPE"),
        body: base64::engine::general_purpose::STANDARD.decode(field("BODY-BASE64")).unwrap(),
    }
}

#[test]
fn submissions_match_chromium_byte_for_byte() {
    for (name, sha) in PINNED {
        let want = expect(name, sha);
        let html = std::fs::read_to_string(format!("{DIR}/{name}.html")).unwrap();
        let mut engine = aether::AetherEngine::new();
        engine.load_html(&format!("http://oracle.test/{name}.html"), &html, true);
        let doc = engine.document.clone().unwrap();
        let go = doc
            .descendants()
            .find(|n| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some("go")))
            .unwrap_or_else(|| panic!("{name}: #go"));
        let nav = engine.submission_for(&go).unwrap_or_else(|| panic!("{name}: no submission"));
        let method = match nav.method {
            aether::forms::HttpMethod::Get => "GET",
            aether::forms::HttpMethod::Post => "POST",
        };
        assert_eq!(method, want.method, "{name}: method");
        let url = nav.url.split('#').next().unwrap().to_string();
        assert_eq!(url, want.url, "{name}: url");
        // The boundary is random per submission on both sides: compare with it replaced.
        let ct = nav.content_type.clone().unwrap_or_default();
        let boundary = ct.split("boundary=").nth(1).map(str::to_string);
        let norm = |s: &str| match &boundary {
            Some(b) => s.replace(b.as_str(), "<B>"),
            None => s.to_string(),
        };
        assert_eq!(norm(&ct), want.content_type, "{name}: content-type");
        let body = norm(nav.body.as_deref().unwrap_or(""));
        assert_eq!(
            body.as_bytes(),
            &want.body[..],
            "{name}: body\n aether:   {:?}\n chromium: {:?}",
            body,
            String::from_utf8_lossy(&want.body)
        );
        if let Some(b) = &boundary {
            assert!(b.starts_with("----WebKitFormBoundary") && b.len() == 38, "{name}: boundary shape {b}");
        }
    }
}
