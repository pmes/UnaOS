//! AETHERDEVTOOLS (SR112): one test per developer-tools panel over a fixture page, plus the chrome's
//! toggle and dock. No network: the fixture is loaded with `load_html_styled`, whose references
//! (stylesheet, image) are listed by the Network panel without being fetched.

use std::sync::OnceLock;

use aether::chrome::devtools::{self, console::Level, network::Initiator, Panel};
use aether::chrome::{is_devtools_toggle, DevtoolsChrome, DevtoolsDock};

const FIXTURE_URL: &str = "https://devtools.test/app/index.html";

const FIXTURE: &str = r#"<!doctype html>
<html><head><title>fixture</title>
<link rel="stylesheet" href="/app/site.css">
<style>#box { color: rgb(255, 0, 0); padding-top: 7px; } .hidden { display: none; }</style>
</head><body>
<div id="box" class="card wide">hello</div>
<p class="hidden">gone</p>
<img src="img/logo.png" srcset="img/logo2x.png 2x" alt="logo">
<!-- a comment -->
<script>
  console.log('booted', 1 + 1);
  console.warn('careful');
  localStorage.setItem('theme', 'dark');
  sessionStorage.setItem('tab', '3');
  document.cookie = 'sid=abc; path=/';
  var counter = 41;
</script>
</body></html>"#;

/// Every test process keeps its storage buckets in its own directory under the target dir.
fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-devtools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

fn fixture() -> aether::AetherEngine {
    init();
    let mut engine = aether::AetherEngine::new();
    engine.load_html_styled(FIXTURE_URL, FIXTURE, &[], true);
    engine
}

fn find_element(engine: &aether::AetherEngine, label_prefix: &str) -> usize {
    devtools::elements::tree(engine, devtools::elements::MAX_ROWS)
        .into_iter()
        .find(|r| r.label.starts_with(label_prefix))
        .unwrap_or_else(|| panic!("no row starting {label_prefix}"))
        .id
}

#[test]
fn elements_panel_lists_the_live_dom_and_inspects_computed_values() {
    let mut engine = fixture();
    let rows = devtools::elements::tree(&engine, devtools::elements::MAX_ROWS);
    assert_eq!(rows[0].label, "#document");
    assert!(rows.iter().any(|r| r.label == "<!DOCTYPE html>"));
    assert!(rows.iter().any(|r| r.label == "<!-- a comment -->"));
    let box_row = rows.iter().find(|r| r.label == r#"<div id="box" class="card wide">"#).expect("the div row");
    assert!(rows.iter().any(|r| r.label == "\"hello\"" && r.depth == box_row.depth + 1));

    let ins = devtools::elements::inspect(&engine, box_row.id).expect("inspect the div");
    assert_eq!(ins.tag, "div");
    assert_eq!(ins.attributes, vec![("id".to_string(), "box".to_string()), ("class".to_string(), "card wide".to_string())]);
    let computed = |k: &str| ins.computed.iter().find(|(p, _)| *p == k).map(|(_, v)| v.clone());
    assert_eq!(computed("color").as_deref(), Some("rgb(255, 0, 0)"));
    assert_eq!(computed("padding-top").as_deref(), Some("7px"));
    assert!(ins.border_box.is_some(), "the div has a box");

    let hidden = find_element(&engine, r#"<p class="hidden">"#);
    let ins = devtools::elements::inspect(&engine, hidden).expect("inspect the p");
    assert_eq!(ins.computed, vec![("display", "none".to_string())]);
    assert_eq!(devtools::elements::breadcrumb(&engine, box_row.id), "html > body > div#box.card.wide");

    // A script mutation shows on the next read (nothing is cached).
    let mut tools = devtools::DevTools::new();
    tools.evaluate(&mut engine, "document.getElementById('box').setAttribute('data-x', 'y')");
    let ins = devtools::elements::inspect(&engine, box_row.id).unwrap();
    assert!(ins.attributes.contains(&("data-x".to_string(), "y".to_string())));
}

#[test]
fn console_panel_shows_page_output_and_evaluates_in_the_page_realm() {
    let mut engine = fixture();
    let mut tools = devtools::DevTools::new();
    let answer = tools.evaluate(&mut engine, "counter + 1");
    assert_eq!(answer, devtools::console::Entry { level: Level::Result, text: "42".to_string() });
    let thrown = tools.evaluate(&mut engine, "nope.x");
    assert_eq!(thrown.level, Level::Exception);
    tools.evaluate(&mut engine, "console.error('from devtools'); 'done'");

    let entries = tools.console.entries();
    let pos = |level: Level, text: &str| entries.iter().position(|e| e.level == level && e.text == text).unwrap_or_else(|| panic!("no {level:?} {text:?} in {entries:?}"));
    let booted = pos(Level::Log, "booted 2");
    let careful = pos(Level::Warn, "careful");
    let input = pos(Level::Input, "counter + 1");
    let result = pos(Level::Result, "42");
    let logged = pos(Level::Error, "from devtools");
    let done = pos(Level::Result, "done");
    assert!(booted < careful && careful < input && input < result, "the page's output precedes the evaluator's");
    assert!(result < logged && logged < done, "a line's own console output precedes its answer");
    assert_eq!(tools.console.history(), ["counter + 1", "nope.x", "console.error('from devtools'); 'done'"]);
    assert!(tools.console.filtered(Level::Error).iter().all(|e| matches!(e.level, Level::Error | Level::Exception)));

    tools.panel = Panel::Console;
    let text = tools.render(&engine);
    assert!(text.iter().any(|l| l == "< 42"));
}

#[test]
fn network_panel_lists_the_documents_requests() {
    let engine = fixture();
    let reqs = devtools::network::requests(&engine);
    let find = |url: &str| reqs.iter().find(|r| r.url == url).unwrap_or_else(|| panic!("no {url} in {reqs:?}"));
    let doc = find(FIXTURE_URL);
    assert_eq!((doc.method, doc.initiator), ("GET", Initiator::Document));
    assert_eq!(find("https://devtools.test/app/site.css").initiator, Initiator::Stylesheet);
    assert_eq!(find("https://devtools.test/app/img/logo.png").initiator, Initiator::Image);
    assert_eq!(find("https://devtools.test/app/img/logo2x.png").initiator, Initiator::Image);
    // Nothing was fetched, so the cache holds no record: the columns say so instead of guessing.
    let css = find("https://devtools.test/app/site.css");
    assert_eq!((css.status, css.size, css.time_ms), (None, None, None));
    assert!(css.row().contains("stylesheet"));
}

#[test]
fn storage_panel_reads_cookies_web_storage_idb_and_caches() {
    let engine = fixture();
    let v = devtools::storage::view(&engine);
    assert!(v.storage_key.is_some(), "an https page has a storage key");
    assert!(v.local.contains(&("theme".to_string(), "dark".to_string())), "{:?}", v.local);
    assert!(v.session.contains(&("tab".to_string(), "3".to_string())), "{:?}", v.session);
    let sid = v.cookies.iter().find(|c| c.name == "sid").expect("the page's cookie");
    assert_eq!((sid.value.as_str(), sid.path.as_str(), sid.expires), ("abc", "/", None));
    assert!(v.indexed_db.is_empty() && v.caches.is_empty());
    let lines = v.lines();
    assert!(lines.iter().any(|l| l == "  theme = dark"));
    assert!(lines.iter().any(|l| l.starts_with("Cookies (")));
}

#[test]
fn performance_panel_samples_script_style_layout_and_paint() {
    let mut engine = fixture();
    let mut tools = devtools::DevTools::new();
    tools.timeline.sample_load(&mut engine);
    assert_eq!(tools.timeline.len(), 1, "the load sample");
    // A script mutation the next frame lays out: that frame carries a cascade.
    engine.js_engine.as_mut().unwrap().eval_string("document.getElementById('box').style.paddingTop = '20px'").unwrap();
    let damage = tools.timeline.frame(&mut engine);
    assert!(damage.is_some(), "the mutation repaints");
    let last = *tools.timeline.samples().last().unwrap();
    assert_eq!(last.frame, 1);
    assert!(last.style_ms + last.layout_ms > 0.0, "the relayout's cascade is attributed to the frame: {last:?}");
    assert!(last.paint_ms > 0.0);
    let t = tools.timeline.totals();
    assert_eq!(t.frame, 2);
    tools.panel = Panel::Performance;
    assert!(tools.render(&engine).iter().any(|l| l.trim_start().starts_with("total")));
}

#[test]
fn the_chrome_toggles_and_docks_the_pane() {
    assert!(is_devtools_toggle("F12", false, false, false));
    assert!(is_devtools_toggle("I", true, true, false));
    assert!(is_devtools_toggle("i", true, true, false));
    assert!(!is_devtools_toggle("i", true, false, false));
    assert!(!is_devtools_toggle("F12", true, false, false));

    let mut c = DevtoolsChrome::default();
    assert_eq!(c.split(1000, 800), ((0, 0, 1000, 800), None));
    assert!(c.on_key("F12", false, false, false));
    assert!(c.open);
    assert_eq!(c.split(1000, 800), ((0, 0, 600, 800), Some((600, 0, 400, 800))));
    assert_eq!(c.flip_dock(), DevtoolsDock::Bottom);
    assert_eq!(c.split(1000, 800), ((0, 0, 1000, 480), Some((0, 480, 1000, 320))));
    c.set_share(99);
    assert_eq!(c.share_pct, 85);
    assert!(!c.on_key("a", false, false, false));
    assert!(c.on_key("I", true, true, false));
    assert!(!c.open);

    // The host's frame call: the page follows the split, the pane's lines come back while it is open.
    let mut engine = fixture();
    let mut tools = devtools::DevTools::new();
    let mut c = DevtoolsChrome { open: true, ..DevtoolsChrome::default() };
    let (_, lines) = aether::chrome::host::devtools_frame(&mut engine, &c, &mut tools, (1000, 800));
    assert_eq!(engine.device_size(), (600, 800));
    assert!(lines.expect("the pane is open")[0].contains("[Elements]"));
    c.toggle();
    let (_, lines) = aether::chrome::host::devtools_frame(&mut engine, &c, &mut tools, (1000, 800));
    assert!(lines.is_none());
    assert_eq!(engine.device_size(), (1000, 800));
}
