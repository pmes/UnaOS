//! JSCORE gate: the Chromium oracle. Each page's DOM after scripts (js_core on the tiny DOM host) must equal the
//! recorded `chromium --dump-dom` of the same page (tests/oracle/page*.chromium.html, from `js_core-oracle
//! --record`).

#[path = "oracle/host.rs"]
mod host;

#[test]
fn oracle_pages_match_chromium() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle");
    for n in 1..=3 {
        let html = std::fs::read_to_string(dir.join(format!("page{}.html", n))).unwrap();
        let expected = std::fs::read_to_string(dir.join(format!("page{}.chromium.html", n))).unwrap();
        let (ours, console) = host::run_page(&html);
        assert!(console.is_empty(), "page{} console output: {}", n, console);
        assert_eq!(ours.trim(), expected.trim(), "page{} DOM differs from Chromium", n);
    }
}
