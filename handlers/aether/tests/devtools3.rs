//! AETHERDEVTOOLS3 (LEDGER SR128): the Sources panel and a Console frame followed into it, a thrown
//! non-error's position, the console's group / count / time / table shapes, the Network row's headers and
//! body preview, and the Elements panel's box model. No network: every fixture is loaded with
//! `load_html_styled` and has no `<script src>`.

use std::sync::OnceLock;

use aether::chrome::devtools::{self, network::Preview, Panel};

const URL: &str = "https://devtools3.test/app/index.html";

fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-devtools3-{}", std::process::id()));
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
fn sources_panel_lists_scripts_and_follows_a_console_frame() {
    let engine = load(concat!(
        "<!doctype html><html><body>",
        "<script>var first = 1;</script>",
        "<script type=\"application/json\">{\"not\": \"a script\"}</script>",
        "<script>\nfunction boom() {\n  throw new Error('boom');\n}\nboom();\n</script>",
        "</body></html>"
    ));
    let scripts = devtools::sources::scripts(&engine);
    assert_eq!(scripts.len(), 2, "the JSON data block is not a script: {scripts:?}");
    assert_eq!(scripts[0].name, format!("{URL} (inline #1)"));
    assert_eq!(scripts[1].name, format!("{URL} (inline #2)"));
    assert_eq!((scripts[1].inline, scripts[1].file.as_str()), (Some(2), URL));

    let mut tools = devtools::DevTools::new();
    let entries = tools.console.entries();
    let uncaught = entries.iter().find(|e| e.text.starts_with("Uncaught Error: boom")).unwrap_or_else(|| panic!("no uncaught entry in {entries:?}"));
    let frame = uncaught.text.lines().find(|l| l.trim_start().starts_with("at boom")).expect("the boom frame");
    assert_eq!(devtools::sources::frame_target(frame), Some((URL.to_string(), 3, 9)));
    assert!(tools.goto_line(&engine, frame), "a frame line is a target");
    assert_eq!(tools.panel, Panel::Sources);
    assert_eq!(tools.sources, devtools::sources::Sources { selected: Some(1), line: Some(3) }, "the inline script that ran last and covers line 3");

    let text = tools.render(&engine);
    assert!(text[0].contains("[Sources]"), "{}", text[0]);
    assert!(text.iter().any(|l| l == &format!("> 2  {URL} (inline #2)")), "{text:?}");
    assert!(text.iter().any(|l| l == "> 3 |   throw new Error('boom');"), "the current line is marked: {text:?}");
    assert!(text.iter().any(|l| l == "  2 | function boom() {"), "{text:?}");
    assert!(!tools.goto(&engine, "https://elsewhere.test/x.js", 1), "no such script: nothing changes");
    assert_eq!(tools.sources.selected, Some(1));
}

#[test]
fn a_thrown_non_error_gets_its_position_and_excerpt() {
    let mut engine = load(concat!(
        "<!doctype html><html><body>",
        "<script>var got = '';\nwindow.addEventListener('error', function (e) { got = e.lineno + ':' + e.colno; });</script>",
        "<script>\nfunction f() {\n  throw 'nope';\n}\nf();\n</script>",
        "</body></html>"
    ));
    let tools = devtools::DevTools::new();
    let entries = tools.console.entries();
    let e = entries.iter().find(|e| e.text.starts_with("Uncaught nope")).unwrap_or_else(|| panic!("no uncaught entry in {entries:?}"));
    assert!(e.text.contains("> 3 |   throw 'nope';"), "the excerpt marks the throw: {}", e.text);
    assert!(e.text.contains(&format!("    | {}^", " ".repeat(2))), "the caret is under `throw`: {}", e.text);
    let js = engine.js_engine.as_mut().expect("a page realm");
    assert_eq!(js.eval_string("got").unwrap(), "3:3", "the ErrorEvent carries the throw statement's line and column");
}

#[test]
fn console_groups_counts_timers_and_tables() {
    let _engine = load(concat!(
        "<!doctype html><html><body><script>",
        "console.group('outer'); console.log('in one'); console.group(); console.log('in two');",
        "console.groupEnd(); console.groupEnd(); console.groupEnd(); console.log('out');",
        "console.count(); console.count(); console.count('x'); console.countReset(); console.count(); console.countReset('nope');",
        "console.time('t'); console.time('t'); console.timeLog('t', 'mid'); console.timeEnd('t'); console.timeEnd('t');",
        "console.table([{a: 1, b: 'y'}, {a: 2}]); console.table([5, 6]); console.table('plain');",
        "</script></body></html>"
    ));
    let lines = aether::js::console_lines();
    let want_prefix: Vec<(u8, &str)> = vec![
        (0, "outer"),
        (0, "  in one"),
        (0, "  console.group"),
        (0, "    in two"),
        (0, "out"),
        (4, "default: 1"),
        (4, "default: 2"),
        (4, "x: 1"),
        (4, "default: 1"),
        (1, "Count for 'nope' does not exist"),
        (1, "Timer 't' already exists"),
    ];
    let got: Vec<(u8, &str)> = lines.iter().take(want_prefix.len()).map(|(l, t)| (*l, t.as_str())).collect();
    assert_eq!(got, want_prefix, "{lines:?}");
    let rest = &lines[want_prefix.len()..];
    let ms = |t: &str, label: &str, tail: &str| {
        let n = t.strip_prefix(label).and_then(|r| r.strip_suffix(tail)).unwrap_or_else(|| panic!("{t:?} is not {label}<ms>{tail}"));
        assert!(n.parse::<f64>().is_ok_and(|v| v >= 0.0), "{t:?}: the delta is a number of ms");
    };
    ms(&rest[0].1, "t: ", " ms mid");
    ms(&rest[1].1, "t: ", " ms");
    assert_eq!(rest[2], (1, "Timer 't' does not exist".to_string()));
    let table = [
        "┌─────────┬───┬─────┐",
        "│ (index) │ a │ b   │",
        "├─────────┼───┼─────┤",
        "│ 0       │ 1 │ 'y' │",
        "│ 1       │ 2 │     │",
        "└─────────┴───┴─────┘",
    ]
    .join("\n");
    assert_eq!(rest[3], (0, table));
    let values = ["┌─────────┬────────┐", "│ (index) │ Values │", "├─────────┼────────┤", "│ 0       │ 5      │", "│ 1       │ 6      │", "└─────────┴────────┘"].join("\n");
    assert_eq!(rest[4], (0, values));
    assert_eq!(rest[5], (0, "plain".to_string()), "a non-object table argument logs as console.log does");
}

#[test]
fn network_rows_carry_headers_and_a_body_preview() {
    // The preview kinds, from the bytes.
    assert_eq!(devtools::network::preview(Some("application/json; charset=utf-8"), br#"{"a":[1,2],"b":{},"s":"x,y:{z}"}"#), Preview::Json("{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"s\": \"x,y:{z}\"\n}".to_string()));
    assert_eq!(devtools::network::pretty_json("{\"a\":1]"), None, "a bracket that does not match is not JSON");
    assert_eq!(devtools::network::preview(Some("text/css"), b"p { color: red }"), Preview::Text("p { color: red }".to_string()));
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&16u32.to_be_bytes());
    png.extend_from_slice(&8u32.to_be_bytes());
    assert_eq!(devtools::network::preview(Some("image/png"), &png), Preview::Image { format: "PNG", width: 16, height: 8 });
    assert_eq!(devtools::network::image_size(b"GIF89a\x03\0\x02\0"), Some(("GIF", 3, 2)));
    let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x20, 0x00, 0x40, 0x01, 0x01, 0x11, 0x00];
    assert_eq!(devtools::network::image_size(&jpeg), Some(("JPEG", 64, 32)));
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
    webp.extend_from_slice(&[99, 0, 0, 49, 0, 0]);
    assert_eq!(devtools::network::image_size(&webp), Some(("WebP", 100, 50)));
    assert_eq!(devtools::network::preview(None, &[0, 159, 146, 150]), Preview::Binary(4));
    assert_eq!(devtools::network::preview(Some("text/plain"), b""), Preview::Empty);

    // A recorded row's detail block.
    let row = devtools::network::Request {
        url: "https://devtools3.test/api/data.json".to_string(),
        method: "GET",
        initiator: devtools::network::Initiator::Fetch,
        status: Some(200),
        mime: Some("application/json".to_string()),
        size: Some(9),
        time_ms: Some(0),
        blocked: None,
        error: None,
        response_headers: vec![("content-type".to_string(), "application/json".to_string()), ("vary".to_string(), "accept-language".to_string())],
        request_headers: vec![("accept-language".to_string(), "en".to_string())],
        preview: Some(devtools::network::preview(Some("application/json"), br#"{"n":1}"#)),
    };
    assert_eq!(
        row.detail_lines(),
        [
            "    Response headers (2)",
            "      content-type: application/json",
            "      vary: accept-language",
            "    Request headers (1, the ones Vary named)",
            "      accept-language: en",
            "    Preview (JSON, 3 lines)",
            "      {",
            "        \"n\": 1",
            "      }",
        ]
    );

    // A row the cache holds nothing of says so, and the pane shows no detail under it.
    let engine = load("<!doctype html><html><head><link rel=stylesheet href=/app/site.css></head><body></body></html>");
    let css = devtools::network::requests(&engine).into_iter().find(|r| r.url.ends_with("/app/site.css")).expect("the stylesheet row");
    assert!(!css.has_detail());
    assert_eq!(css.detail_lines(), ["    headers and body: not recorded"]);
}

#[test]
fn elements_panel_draws_the_selected_nodes_box_model() {
    let engine = load(concat!(
        "<!doctype html><html><head><style>",
        "#b { margin: 10px; border: 2px solid black; padding: 5px 6px; width: 100px; height: 40px; }",
        "</style></head><body><div id=\"b\">box</div></body></html>"
    ));
    let id = devtools::elements::tree(&engine, devtools::elements::MAX_ROWS).into_iter().find(|r| r.label == r#"<div id="b">"#).expect("the div row").id;
    let bm = devtools::elements::box_model(&engine, id).expect("the div has a box");
    assert_eq!((bm.margin.top, bm.margin.bottom, bm.margin.left), (10.0, 10.0, 10.0));
    assert_eq!(bm.border, devtools::elements::Edges { top: 2.0, right: 2.0, bottom: 2.0, left: 2.0 });
    assert_eq!(bm.padding, devtools::elements::Edges { top: 5.0, right: 6.0, bottom: 5.0, left: 6.0 });
    assert_eq!(bm.content, (100.0, 40.0), "content-box sizing: the width and height are the content box");

    let drawing = devtools::elements::box_model_lines(&bm);
    assert_eq!(drawing.len(), 15, "four nested rectangles: {drawing:#?}");
    let w = drawing[0].chars().count();
    assert!(drawing.iter().all(|l| l.chars().count() == w), "every row is as wide as the margin box: {drawing:#?}");
    assert!(drawing[0].starts_with('+') && drawing[14].starts_with('+'));
    assert!(drawing[1].starts_with("| margin") && drawing[1].contains("10"));
    assert!(drawing[3].contains("| border") && drawing[5].contains("| padding"));
    let mid = &drawing[7];
    assert!(mid.contains(" 100 x 40 "), "{mid}");
    let cells: Vec<&str> = mid.split('|').map(str::trim).collect();
    assert_eq!(&cells[..7], ["", "10", "2", "6", "100 x 40", "6", "2"], "the left edges outside in, the content, the right edges: {mid}");

    let mut tools = devtools::DevTools::new();
    tools.selected = Some(id);
    let text = tools.render(&engine);
    assert!(text.iter().any(|l| l.contains(" 100 x 40 ")), "the pane draws the box model under the inspection: {text:?}");
}
