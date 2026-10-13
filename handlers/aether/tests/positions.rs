//! JSPOS (LEDGER SR127): source positions through the engine — the devtools Console prints an uncaught
//! error's frames (`    at fn (url:line:col)`) and the source excerpt under its line, and the window's
//! ErrorEvent carries `filename` / `lineno` / `colno` from the throw site.

use std::sync::OnceLock;

use aether::chrome::devtools;

const URL: &str = "https://positions.test/index.html";

fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-positions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

fn load(html: &str) -> aether::AetherEngine {
    init();
    let mut engine = aether::AetherEngine::new();
    engine.load_html_styled(URL, html, &[], true);
    engine
}

#[test]
fn console_prints_the_frames_and_the_source_excerpt_under_an_uncaught_error() {
    let _engine = load("<!doctype html><html><body><script>\nfunction boom() {\n  throw new Error('boom');\n}\nboom();\n</script></body></html>");
    let tools = devtools::DevTools::new();
    let entries = tools.console.entries();
    let e = entries.iter().find(|e| e.text.starts_with("Uncaught Error: boom")).unwrap_or_else(|| panic!("no uncaught entry in {entries:?}"));
    let want = [
        "Uncaught Error: boom".to_string(),
        format!("    at boom ({URL}:3:9)"),
        format!("    at {URL}:5:1"),
        "  1 | ".to_string(),
        "  2 | function boom() {".to_string(),
        "> 3 |   throw new Error('boom');".to_string(),
        format!("    | {}^", " ".repeat(8)),
        "  4 | }".to_string(),
        "  5 | boom();".to_string(),
    ]
    .join("\n");
    assert_eq!(e.text, want);
    assert_eq!(aether::js::reported_errors(), ["Error: boom"], "the reported message stays one line");
    assert!(aether::js::console_lines().iter().any(|(l, t)| *l == 2 && t == "Uncaught Error: boom"), "the page console keeps the one-line form");
}

#[test]
fn error_event_carries_filename_lineno_and_colno_of_the_throw_site() {
    let mut engine = load(
        "<!doctype html><html><body><script>var got = '';\nwindow.addEventListener('error', function (e) { got = e.filename + '|' + e.lineno + '|' + e.colno; });</script><script>\nfunction f() {\n  throw new Error('x');\n}\nf();\n</script></body></html>",
    );
    let js = engine.js_engine.as_mut().expect("a page realm");
    assert_eq!(js.eval_string("got").unwrap(), format!("{URL}|3|9"));
}
