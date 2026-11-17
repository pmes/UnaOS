#[cfg(test)]
mod tests {
    use crate::dom;
    use crate::layout;
    use crate::css;
    use crate::render;
    use crate::storage::LocalStorage;
    use crate::forms;
    use std::path::PathBuf;

    #[test]
    fn test_html_parsing_and_layout() {
        let html = r#"<html><body><div id="target"></div></body></html>"#;
        let document = dom::parse_html(html);
        let layout_tree = layout::compute_layout(&document);
        assert!(!layout_tree.node_map.is_empty(), "Layout tree should not be empty");
    }

    #[test]
    fn test_render_paint_assertions() {
        let html = r#"<html><body><div id="box" style="width: 10px; height: 10px; background-color: red;"></div><p>Hi</p></body></html>"#;
        let document = dom::parse_html(html);
        let layout_tree = layout::compute_layout(&document);
        let mut surface = vec![0u8; 100 * 100 * 4];
        let damage = vec![(0, 0, 100, 100)];
        render::render_frame(&layout_tree, &mut surface, 100, 100, 0.0, 0.0, &damage);

        // Page background is white; unstyled boxes paint no gray fill or border.
        let idx_body = (3 * 100 + 3) * 4;
        assert_eq!(&surface[idx_body..idx_body + 4], &[255, 255, 255, 255], "page background is white");

        // The styled div paints red (BGRA: 0,0,255) somewhere.
        let has_red = surface
            .chunks_exact(4)
            .any(|p| p[0] == 0 && p[1] == 0 && p[2] == 255);
        assert!(has_red, "background-color: red must be painted");

        // No pixel carries the old default gray-box fill.
        let has_gray = surface
            .chunks_exact(4)
            .any(|p| p[0] == 200 && p[1] == 200 && p[2] == 200);
        assert!(!has_gray, "unstyled boxes must not paint gray");

        // Text glyphs paint dark pixels (blended toward black); skip if the
        // host has no sans-serif font at all.
        let has_dark = surface
            .chunks_exact(4)
            .any(|p| p[0] < 100 && p[1] < 100 && p[2] < 100);
        assert!(has_dark, "text must rasterize to dark pixels");
    }

    #[test]
    fn test_storage_path_sanitization() {
        // The origin sanitization logic happens in JS constructor, but we can test LocalStorage directly
        let storage = LocalStorage::new(PathBuf::from("/tmp/aether_storage/test_origin.json"));
        // we just verify it constructs without panicking
        assert_eq!(format!("{:?}", storage).contains("test_origin"), true);
    }
    
    #[test]
    fn test_form_urlencoded() {
        let mut form_data = std::collections::HashMap::new();
        form_data.insert("q".to_string(), "hello world & rust".to_string());
        
        let encoded = html_core::url::form::serialize(form_data.iter());
            
        assert_eq!(encoded, "q=hello+world+%26+rust");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_input_events_and_navigation() {
        // Setup engine with fixture DOM
        let html = r#"<html><head><style>
            a { width: 100px; height: 30px; }
            input { width: 100px; height: 30px; }
        </style></head><body>
            <a id="link" href="https://example.com/dest">Link</a>
            <form action="/search"><input id="field" name="q" type="text" value="hello" /></form>
        </body></html>"#;
        
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/", html, true);
        engine.render_frame(); // to compute bounds
        
        // Find link and field absolute coordinates by walking layout tree
        fn get_abs_pos(layout: &crate::layout::LayoutTree, target_id: &str) -> Option<(f64, f64)> {
            fn walk(node_id: taffy::prelude::NodeId, cx: f32, cy: f32, layout: &crate::layout::LayoutTree, target_id: &str) -> Option<(f64, f64)> {
                let l = layout.taffy.layout(node_id).unwrap();
                let nx = cx + l.location.x;
                let ny = cy + l.location.y;
                if let Some(dom_node) = layout.node_map.get(&node_id) {
                    if let Some(el) = dom_node.as_element() {
                        if el.attributes.borrow().get("id") == Some(target_id) {
                            return Some((nx as f64 + 1.0, ny as f64 + 1.0));
                        }
                    }
                }
                for child in layout.taffy.children(node_id).unwrap() {
                    if let Some(pos) = walk(child, nx, ny, layout, target_id) {
                        return Some(pos);
                    }
                }
                None
            }
            walk(layout.root_node, 0.0, 0.0, layout, target_id)
        }
        
        // 1. focus + Text/Backspace editing a field
        let (link_x, link_y) = get_abs_pos(engine.layout_tree.as_ref().unwrap(), "link").unwrap();
        let (field_x, field_y) = get_abs_pos(engine.layout_tree.as_ref().unwrap(), "field").unwrap();
        
        engine.handle_event(crate::api::events::Event::MouseDown(field_x, field_y));
        engine.handle_event(crate::api::events::Event::MouseUp(field_x, field_y));
        
        // Verify focus
        assert!(engine.focused_node.is_some(), "Field should be focused");
        
        // The click put the caret at the field's left edge; End moves it after the value, then type.
        assert_eq!(engine.edit.caret, 0, "a click at the field's left edge puts the caret at 0");
        engine.handle_event(crate::api::events::Event::KeyDown("End".to_string()));
        engine.handle_event(crate::api::events::Event::Text(" world".to_string()));

        // The typed text is the field's (dirty) VALUE; the `value` attribute stays the default.
        {
            let focused_node = engine.focused_node.as_ref().unwrap();
            assert_eq!(forms::control_value(focused_node), "hello world");
            let el = focused_node.as_element().unwrap();
            assert_eq!(el.attributes.borrow().get("value"), Some("hello"), "the default value is untouched");
        }

        // Backspace
        engine.handle_event(crate::api::events::Event::KeyDown("BackSpace".to_string()));
        assert_eq!(forms::control_value(engine.focused_node.as_ref().unwrap()), "hello worl");
        
        // Enter stages the form submission (the shell performs it async —
        // blocking on the engine thread killed it on current-thread runtimes).
        engine.handle_event(crate::api::events::Event::KeyDown("Return".to_string()));
        let nav = engine.take_pending_nav().expect("Return must stage the form nav");
        assert_eq!(nav.url, "https://example.com/search?q=hello+worl", "form nav resolves against page");

        // 2. hit-test -> link click stages a GET navigation
        engine.handle_event(crate::api::events::Event::MouseDown(link_x, link_y));
        engine.handle_event(crate::api::events::Event::MouseUp(link_x, link_y));
        let nav = engine.take_pending_nav().expect("link click must stage nav");
        assert_eq!(nav.url, "https://example.com/dest");
        assert_eq!(nav.method, crate::forms::HttpMethod::Get);
        assert!(engine.take_pending_nav().is_none(), "take must consume");
    }

    #[tokio::test]
    async fn test_shell_borrow_panic_prevention() {
        use std::rc::Rc;
        use std::cell::RefCell;
        let engine = Rc::new(RefCell::new(crate::AetherEngine::new()));
        
        let engine_clone = engine.clone();
        let local = tokio::task::LocalSet::new();
        
        local.run_until(async move {
            tokio::task::spawn_local(async move {
                let html = {
                    // Do the fetch WITHOUT holding the borrow
                    let _ = crate::net::fetch_document("http://invalid.test.domain.for.test").await;
                    "<html></html>".to_string()
                };
                // Then borrow to apply
                engine_clone.borrow_mut().load_html("http://invalid.test.domain.for.test", &html, true);
            });
            
            // Yield to allow the spawn_local to run until its await point (simulating GTK timeout)
            tokio::task::yield_now().await;
            
            // This will panic if the spawn_local held the borrow across the await, proving safety.
            engine.borrow_mut().tick();
        }).await;
    }

    #[test]
    fn test_css_selectors_and_specified_only_application() {
        let html = r#"<html><body>
            <div class="hero other">a</div>
            <p id="lead">b</p>
            <p>c</p>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            .hero { background-color: red; }
            #lead { color: #00ff00; }
            p { font-size: 20px; }
        "#);

        let mut hero_bg = None;
        let mut lead_color = None;
        let mut plain_p = None;
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let attrs = el.attributes.borrow();
            let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
            if attrs.get("class") == Some("hero other") {
                hero_bg = paint.background;
            } else if attrs.get("id") == Some("lead") {
                lead_color = paint.color;
                assert_eq!(paint.font_size, Some(20.0), "tag rule must still apply to #lead");
            } else if el.name.local.as_ref() == "p" {
                plain_p = Some(paint);
            }
        }
        assert_eq!(hero_bg, Some((255, 0, 0)), "class selector must match");

        // Descendant and compound selectors via the real selector engine.
        css::apply_css(&mut layout_tree, r#"
            body .hero { color: navy; }
            div.other { font-size: 30px; }
        "#);
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            if el.attributes.borrow().get("class") == Some("hero other") {
                let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
                assert_eq!(paint.color, Some((0, 0, 128)), "descendant selector must match");
                assert_eq!(paint.font_size, Some(30.0), "compound selector must match");
            }
        }
        assert_eq!(lead_color, Some((0, 255, 0)), "id selector must match");
        let plain_p = plain_p.expect("plain <p> present");
        assert_eq!(plain_p.font_size, Some(20.0));
        assert_eq!(plain_p.background, None, "rule must not leak unspecified properties");

        // A rule specifying only a color must not reset flex_direction to Row.
        let mut col_ok = false;
        for (node_id, dom_node) in &layout_tree.node_map {
            if let Some(el) = dom_node.as_element() {
                if el.name.local.as_ref() == "body" {
                    let s = layout_tree.taffy.style(*node_id).unwrap();
                    col_ok = s.flex_direction == taffy::style::FlexDirection::Column;
                }
            }
        }
        assert!(col_ok, "unspecified flex-direction must stay Column");
    }

    #[test]
    fn test_ledger_records_unimplemented_apis() {
        crate::ledger::reset();
        let html = r#"<html><head><style>
            div { filter: blur(2px); grid-template-columns: 1fr 1fr; display: grid; }
        </style></head><body>
            <div id="t">x</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://ledger", html, true);

        let snap = crate::ledger::snapshot();
        use crate::ledger::ApiCategory;
        // AETHERSTYLE3 (SR103, 8b695a6f) implemented `filter` (layout_core css.rs, the "filter" arm): a
        // handled property is NOT a gap. Grid layout is still unimplemented: `grid-template-columns` falls to
        // the unhandled-property arm and `display: grid` to apply_display's unhandled-value arm.
        assert!(!snap.contains(ApiCategory::Css, "property:filter"), "a handled CSS property must not be recorded");
        assert!(snap.contains(ApiCategory::Css, "property:grid-template-columns"), "unhandled CSS property must be recorded");
        assert!(snap.contains(ApiCategory::Css, "display:grid"), "unhandled display value must be recorded");
        // AETHERAPIS (SR145): no self-reporting observer shim remains (MutationObserver SR82, PerformanceObserver
        // SR135, IntersectionObserver / ResizeObserver SR145 all deliver), so the CSS rows alone carry the
        // "an unhandled thing records itself" contract here.
    }

    #[test]
    fn test_media_sources_for_stria() {
        let html = r#"<html><body>
            <video src="clips/intro.mp4"></video>
            <audio src="https://cdn.example.org/song.ogg" type="audio/ogg"></audio>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/watch", html, true);
        let media = engine.media_sources();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0], ("https://example.com/clips/intro.mp4".into(), "video/mp4".into()));
        assert_eq!(media[1], ("https://cdn.example.org/song.ogg".into(), "audio/ogg".into()));
    }

    /// <input type=hidden> is a value carrier, not a control: it must
    /// generate no box at all. Boxed, a page's tokens and locale fields
    /// painted as a stack of phantom bordered fields.
    #[test]
    fn test_hidden_input_generates_no_box() {
        let html = r#"<html><body>
            <input type="hidden" name="ie" value="ISO-8859-1">
            <input type="HIDDEN" name="hl" value="en">
            <input type="text" name="q">
        </body></html>"#;
        let tree = layout::compute_layout(&dom::parse_html(html));
        let inputs: Vec<_> = tree
            .node_map
            .values()
            .filter(|n| n.as_element().is_some_and(|e| &*e.name.local == "input"))
            .collect();
        assert_eq!(inputs.len(), 1, "only the visible input generates a box");
        assert_eq!(
            inputs[0]
                .as_element()
                .unwrap()
                .attributes
                .borrow()
                .get("type"),
            Some("text")
        );
    }

    /// Inline siblings mixed with block siblings get an anonymous block box
    /// so they still share a line — two submit buttons between two divs
    /// belong side by side, not stacked.
    #[test]
    fn test_mixed_inline_and_block_children_share_a_line() {
        let html = r#"<html><body style="margin:0"><div style="width:600px">
            <div style="height:10px"></div>
            <span style="width:80px;height:20px">one</span>
            <span style="width:80px;height:20px">two</span>
            <div style="height:10px"></div>
        </div></body></html>"#;
        let mut tree = layout::compute_layout(&dom::parse_html(html));
        css::apply_css(&mut tree, "");
        let mut ys = Vec::new();
        let mut xs = Vec::new();
        for (id, node) in &tree.node_map {
            if node.as_element().is_some_and(|e| &*e.name.local == "span") {
                let l = tree.taffy.layout(*id).unwrap();
                xs.push(l.location.x);
                ys.push(l.location.y);
            }
        }
        assert_eq!(xs.len(), 2, "both spans laid out");
        assert_eq!(ys[0], ys[1], "spans share a line (anonymous block box)");
        assert_ne!(xs[0], xs[1], "spans sit beside each other");
    }

    #[test]
    fn test_form_submission_builds_get_url() {
        let html = r#"<html><body>
            <form action="/search" method="get">
                <input name="q" value="una os">
                <input name="lang" value="en">
            </form>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/page", html, true);
        let doc = engine.document.as_ref().unwrap();
        let input = doc.select("input").unwrap().next().unwrap();
        let od = engine
            .build_form_submission(input.as_node())
            .expect("submission built");
        assert_eq!(od.url, "https://example.com/search?q=una+os&lang=en");
        assert_eq!(od.method, crate::forms::HttpMethod::Get);
        assert!(od.body.is_none());
    }

    #[test]
    fn test_click_dispatch_and_relayout() {
        let html = r#"<html><body>
            <div id="btn">Click me</div>
            <script>
                document.getElementById("btn").addEventListener("click", function() {
                    document.getElementById("btn").textContent = "clicked!";
                });
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://events", html, true);

        // Find the button's on-screen position from the layout tree.
        let (bx, by) = {
            let layout = engine.layout_tree.as_ref().unwrap();
            let mut found = None;
            for (node_id, dom_node) in &layout.node_map {
                if let Some(el) = dom_node.as_element() {
                    if el.attributes.borrow().get("id") == Some("btn") {
                        // Accumulate absolute position by walking from root.
                        fn abs_pos(
                            target: taffy::NodeId,
                            node: taffy::NodeId,
                            x: f32,
                            y: f32,
                            layout: &crate::layout::LayoutTree,
                        ) -> Option<(f32, f32)> {
                            let l = layout.taffy.layout(node).ok()?;
                            let (nx, ny) = (x + l.location.x, y + l.location.y);
                            if node == target {
                                return Some((nx, ny));
                            }
                            for child in layout.taffy.children(node).ok()? {
                                if let Some(hit) = abs_pos(target, child, nx, ny, layout) {
                                    return Some(hit);
                                }
                            }
                            None
                        }
                        found = abs_pos(*node_id, layout.root_node, 0.0, 0.0, layout);
                    }
                }
            }
            let (x, y) = found.expect("#btn laid out");
            (x + 4.0, y + 4.0)
        };

        engine.handle_event(crate::api::events::Event::MouseUp(bx as f64, by as f64));

        let doc = engine.document.as_ref().unwrap();
        let el = doc.select("#btn").unwrap().next().unwrap();
        assert_eq!(
            el.as_node().text_contents().trim(),
            "clicked!",
            "click handler must run and mutate the DOM"
        );
        assert!(engine.needs_repaint, "mutation must schedule a repaint");
    }

    #[test]
    fn test_js_dom_mutations_are_real() {
        let html = r#"<html><body>
            <div id="t">old</div>
            <script>
                var el = document.getElementById("t");
                el.setAttribute("data-x", "42");
                el.innerHTML = "<p>new <b>content</b></p>";
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://jsdom", html, true);

        let doc = engine.document.as_ref().unwrap();
        let el = doc.select("#t").unwrap().next().expect("#t present");
        assert_eq!(
            el.attributes.borrow().get("data-x"),
            Some("42"),
            "setAttribute must mutate the DOM"
        );
        assert!(
            doc.select("#t b").unwrap().next().is_some(),
            "innerHTML must replace children with parsed markup"
        );
        assert_eq!(el.as_node().text_contents().trim(), "new content");
    }

    #[tokio::test]
    async fn test_headless_render_writes_png_and_ledger() {
        let dir = std::env::temp_dir().join("aether_headless_test");
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("fixture.html");
        std::fs::write(&fixture, r#"<html><body>
            <div style="width: 50px; height: 50px; background-color: red;"></div>
            <p>hello</p>
        </body></html>"#).unwrap();
        let out = dir.join("out.png");
        let ledger_path = dir.join("ledger.txt");

        let (w, h, _missing) = crate::headless::render_headless(
            None, Some(&fixture), &out, &ledger_path,
        ).await.unwrap();

        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (w, h));
        let first = *img.get_pixel(0, 0);
        assert!(
            img.pixels().any(|p| *p != first),
            "rendered PNG must not be a single flat color"
        );
        let ledger_text = std::fs::read_to_string(&ledger_path).unwrap();
        assert!(ledger_text.contains("Aether M5 API Ledger"), "ledger dump must be written");
    }

    /// !important is a real cascade tier: it beats higher specificity and
    /// inline styles; inline !important beats sheet !important.
    #[test]
    fn test_important_cascade_tier() {
        let html = r#"<html><body>
            <p id="lead" style="color: purple">a</p>
            <p id="second" style="color: teal !important">b</p>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            p { color: red !important; font-size: 20px !important; }
            #lead { color: blue; font-size: 30px; }
        "#);
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
            match el.attributes.borrow().get("id") {
                Some("lead") => {
                    assert_eq!(paint.color, Some((255, 0, 0)),
                        "sheet !important must beat higher specificity AND inline");
                    assert_eq!(paint.font_size, Some(20.0));
                }
                Some("second") => {
                    assert_eq!(paint.color, Some((0, 128, 128)),
                        "inline !important must beat sheet !important");
                }
                _ => {}
            }
        }
    }

    /// Inline style="" beats normal sheet rules of any specificity.
    #[test]
    fn test_inline_beats_normal_rules() {
        let html = r#"<html><body><p id="x" style="color: green">a</p></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, "#x { color: red; }");
        let paint = layout_tree.node_map.iter()
            .find(|(_, n)| n.as_element().map_or(false, |e| e.attributes.borrow().get("id") == Some("x")))
            .map(|(id, _)| layout_tree.paint_map.get(id).cloned().unwrap_or_default())
            .unwrap();
        assert_eq!(paint.color, Some((0, 128, 0)), "inline must beat normal id rule");
    }

    /// position:absolute with no insets keeps its static (in-flow) position
    /// instead of pinning to the parent origin over its siblings.
    #[test]
    fn test_absolute_without_insets_stays_in_flow() {
        let html = r#"<html><body>
            <div id="a">first</div>
            <div id="b">second</div>
            <div id="c">third</div>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            #b { position: absolute; }
            #c { position: absolute; top: 5px; left: 5px; }
        "#);
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let s = layout_tree.taffy.style(*node_id).unwrap();
            match el.attributes.borrow().get("id") {
                Some("b") => assert_eq!(s.position, taffy::style::Position::Relative,
                    "no insets → static-position fallback"),
                Some("c") => assert_eq!(s.position, taffy::style::Position::Absolute,
                    "insets specified → genuinely absolute"),
                _ => {}
            }
        }
    }

    #[test]
    fn test_background_image_parse_and_url_collection() {
        assert_eq!(css::extract_css_url("url(/img/hero.png)"), Some("/img/hero.png".into()));
        assert_eq!(css::extract_css_url(r#"url("a.jpg") no-repeat center"#), Some("a.jpg".into()));
        assert_eq!(css::extract_css_url("linear-gradient(red, blue)"), None);

        let html = r#"<html><body><div style="background-image: url('/hero.png')">x</div></body></html>"#;
        let document = dom::parse_html(html);
        let layout_tree = layout::compute_layout(&document);
        let has_bg = layout_tree.paint_map.values().any(|p| p.bg_image.as_deref() == Some("/hero.png"));
        assert!(has_bg, "inline background-image must reach the paint map");

        let mut urls = Vec::new();
        crate::net::collect_css_image_urls(
            "https://example.com/page",
            &[".hero { background: #333 url(/img/bg.jpg) }", "body { background-image: url(x.svg) }"],
            &mut urls,
        );
        assert_eq!(urls, vec![
            "https://example.com/img/bg.jpg".to_string(),
            "https://example.com/x.svg".to_string(),
        ], "raster AND svg css urls resolve against the base");
    }

    /// A sheet-relative url() fetches from the SHEET's host but stores a
    /// page-base paint key too (paint resolves raw urls against the page).
    #[test]
    fn test_css_image_refs_sheet_base() {
        let mut refs = Vec::new();
        crate::net::collect_css_image_refs(
            "https://cdn.example.org/styles/main.css",
            "https://example.com/page/",
            ".hero { background-image: url(../img/bg.png) }",
            &mut refs,
        );
        assert_eq!(refs, vec![(
            "https://cdn.example.org/img/bg.png".to_string(),
            "https://example.com/img/bg.png".to_string(),
        )]);
    }

    /// SVG decodes to straight-alpha RGBA, from bytes and from a plain
    /// (non-base64) data: URI.
    #[test]
    fn test_svg_decode() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
        let img = crate::images::decode_svg(svg).expect("svg must decode");
        assert_eq!((img.width(), img.height()), (10, 10));
        assert_eq!(img.get_pixel(5, 5).0, [255, 0, 0, 255]);

        let uri = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'><rect width='4' height='4' fill='%2300ff00'/></svg>";
        let img = crate::images::get(uri).expect("plain svg data uri must decode");
        assert_eq!(img.get_pixel(1, 1).0, [0, 255, 0, 255]);
    }

    /// AETHERHOVER (SR67): a render with no pointer matches no `:hover` (and `:focus` never matches);
    /// a pointer on the link puts `:hover` on the link AND its ancestors (Selectors 4 §9.1) and
    /// re-styles; moving off clears it.
    #[test]
    fn test_hover_applies_on_the_pointer_path_only() {
        let html = r#"<html><head><style>
            body { margin: 0; }
            a { display: block; height: 40px; font-size: 20px; }
            a:hover { color: red; }
            a:focus { background-color: yellow; }
            p { height: 40px; margin: 0; }
        </style></head><body><a id="x" href="/">link</a><p id="y">other</p></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://hover", html, true);
        let paint_of = |engine: &crate::AetherEngine, id: &str| {
            let layout = engine.layout_tree.as_ref().unwrap();
            layout.node_map.iter()
                .find(|(_, n)| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some(id)))
                .map(|(t, _)| layout.paint_map.get(t).cloned().unwrap_or_default())
                .unwrap()
        };
        assert_eq!(paint_of(&engine, "x").color, None, "no pointer: :hover matches nothing");
        assert_eq!(paint_of(&engine, "x").font_size, Some(20.0), "plain tag rule still applies");

        engine.handle_event(crate::api::events::Event::MouseMove(10.0, 10.0));
        assert_eq!(paint_of(&engine, "x").color, Some((255, 0, 0)), "pointer on the link: a:hover applies");
        assert_eq!(paint_of(&engine, "x").background, None, ":focus still never matches");
        let js = engine.js_engine.as_mut().unwrap();
        assert_eq!(
            js.eval_string("Array.from(document.querySelectorAll(':hover')).map(e => e.localName).join(',')").unwrap(),
            "html,body,a",
            "the hover chain is the hit element and every ancestor"
        );

        engine.handle_event(crate::api::events::Event::MouseMove(10.0, 50.0));
        assert_eq!(paint_of(&engine, "x").color, None, "pointer off the link: :hover cleared");
        let js = engine.js_engine.as_mut().unwrap();
        assert_eq!(js.eval_string("document.querySelector('p').matches(':hover')").unwrap(), "true");
    }

    /// The page with UI Events §5.3.3's nested A > B > C, each listening for the five motion events.
    /// The bubbling three are logged where they are targeted (`currentTarget == target`), with
    /// their relatedTarget; mouseenter/mouseleave are logged wherever they land (they never bubble).
    const NESTED_ABC: &str = r#"<html><head><style>
        body { margin: 0; }
        #a { padding: 40px; height: 200px; }
        #b { padding: 40px; height: 120px; }
        #c { height: 40px; }
    </style></head><body><div id="a"><div id="b"><div id="c"></div></div></div>
    <script>
        var log = [], bubbled = [];
        for (const id of ["a", "b", "c"]) {
            const el = document.getElementById(id);
            for (const ty of ["mouseover", "mouseout", "mouseenter", "mouseleave", "mousemove"]) {
                el.addEventListener(ty, function (e) {
                    if (e.currentTarget !== e.target) { bubbled.push(ty + ":" + e.target.id + "@" + id); return; }
                    var rel = (ty === "mouseover" || ty === "mouseout") ? "<" + (e.relatedTarget ? (e.relatedTarget.id || e.relatedTarget.localName) : "null") : "";
                    log.push(ty + " " + id + rel + (e.bubbles ? "" : " nobubble"));
                });
            }
        }
    </script></body></html>"#;

    /// UI Events §5.3.3.1's event order for a pointer that moves into A, into nested B, into nested C,
    /// then out of all three: a KAT, sequence for sequence (Chromium's order: out, leave…, over,
    /// enter…, move).
    #[test]
    fn test_mouse_boundary_events_follow_ui_events_order() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://abc", NESTED_ABC, true);
        let mv = |e: &mut crate::AetherEngine, x: f64, y: f64| e.handle_event(crate::api::events::Event::MouseMove(x, y));
        mv(&mut engine, 10.0, 400.0); // below A: over body/html, none of A/B/C
        mv(&mut engine, 10.0, 10.0); // into A
        mv(&mut engine, 20.0, 20.0); // still A: a mousemove, no boundary
        mv(&mut engine, 50.0, 50.0); // into B
        mv(&mut engine, 90.0, 90.0); // into C
        mv(&mut engine, 10.0, 400.0); // out of C, B and A at once
        let js = engine.js_engine.as_mut().unwrap();
        let log = js.eval_string("log.join('|')").unwrap();
        let want = [
            "mouseover a<html", "mouseenter a nobubble", "mousemove a",
            "mousemove a",
            "mouseout a<b", "mouseover b<a", "mouseenter b nobubble", "mousemove b",
            "mouseout b<c", "mouseover c<b", "mouseenter c nobubble", "mousemove c",
            "mouseout c<html", "mouseleave c nobubble", "mouseleave b nobubble", "mouseleave a nobubble",
        ]
        .join("|");
        assert_eq!(log, want);
    }

    /// mouseover/mouseout/mousemove bubble to every ancestor's listener; mouseenter/mouseleave never
    /// do (each element hears only its own boundary), so A hears C's over/out but not C's enter.
    #[test]
    fn test_mouse_enter_leave_do_not_bubble() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://abc", NESTED_ABC, true);
        engine.handle_event(crate::api::events::Event::MouseMove(90.0, 90.0)); // straight into C
        let js = engine.js_engine.as_mut().unwrap();
        // The first motion enters C (and A, B) from nowhere: relatedTarget null, three non-bubbling
        // enters, outermost first.
        assert_eq!(
            js.eval_string("log.join('|')").unwrap(),
            "mouseover c<null|mouseenter a nobubble|mouseenter b nobubble|mouseenter c nobubble|mousemove c"
        );
        assert_eq!(
            js.eval_string("bubbled.join('|')").unwrap(),
            "mouseover:c@b|mouseover:c@a|mousemove:c@b|mousemove:c@a",
            "over/move bubble C -> B -> A; enter does not"
        );
    }

    /// AETHERHOVER (SR67): hit testing follows PAINT order. A positioned box painted over a later
    /// in-flow block wins (tree order picked the later block); between two positioned siblings the
    /// higher z-index wins regardless of tree order; a hidden box is no target.
    #[test]
    fn test_hit_test_is_paint_order() {
        let html = r#"<html><head><style>
            body { margin: 0; }
            #over { position: absolute; top: 0; left: 0; width: 100px; height: 100px; }
            #flow { height: 200px; }
            #hi { position: absolute; top: 300px; left: 0; width: 100px; height: 100px; z-index: 2; }
            #lo { position: absolute; top: 300px; left: 0; width: 100px; height: 100px; z-index: 1; }
            #ghost { position: absolute; top: 450px; left: 0; width: 100px; height: 100px; visibility: hidden; }
            #floor { position: absolute; top: 450px; left: 0; width: 100px; height: 100px; z-index: -1; }
        </style></head><body><div id="over"></div><div id="flow"></div><div id="hi"></div><div id="lo"></div>
        <div id="floor"></div><div id="ghost"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://hit", html, true);
        let id_at = |e: &crate::AetherEngine, x: f64, y: f64| {
            e.hit_test_at(x, y)
                .and_then(|(n, _)| n.as_element().map(|el| el.attributes.borrow().get("id").unwrap_or("").to_string()))
                .unwrap_or_default()
        };
        assert_eq!(id_at(&engine, 50.0, 50.0), "over", "positioned layer paints above the later in-flow block");
        assert_eq!(id_at(&engine, 50.0, 150.0), "flow");
        assert_eq!(id_at(&engine, 50.0, 350.0), "hi", "z-index 2 above the later z-index 1 sibling");
        assert_eq!(id_at(&engine, 50.0, 500.0), "floor", "a visibility:hidden box is not a target");
    }

    /// AETHERFIT (SR189): a modal dialog is centred in the viewport (the UA sheet's `inset: 0; margin: auto; width /
    /// height: fit-content` over layout_core's fixed-box pass) and the top layer is hit before the document: the
    /// viewport's centre hits the dialog's control even though a document box lies under it, and a point outside the
    /// dialog hits the (inert) document box beneath, which the input path then retargets.
    #[test]
    fn test_hit_test_hoists_the_top_layer() {
        let html = r#"<html><head><style>
            body { margin: 0; }
            #under { height: 600px; }
            dialog { padding: 0; border: 0; }
            #inner { display: block; width: 100px; height: 40px; }
        </style></head><body><div id="under"></div><dialog><div id="inner"></div></dialog>
        <script>document.querySelector('dialog').showModal();</script></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://toplayer", html, true);
        let id_at = |e: &crate::AetherEngine, x: f64, y: f64| {
            e.hit_test_at(x, y)
                .and_then(|(n, _)| n.as_element().map(|el| el.attributes.borrow().get("id").unwrap_or("").to_string()))
                .unwrap_or_default()
        };
        let (vw, vh) = (engine.width as f64, engine.height as f64);
        assert_eq!(id_at(&engine, vw / 2.0, vh / 2.0), "inner", "the centred modal dialog is hit above the document");
        assert_eq!(id_at(&engine, vw / 2.0 - 45.0, vh / 2.0 - 15.0), "inner", "its box spans 100 x 40 around the centre");
        assert_eq!(id_at(&engine, vw / 2.0 + 60.0, vh / 2.0), "under", "outside the dialog: the document box beneath");
        assert_eq!(id_at(&engine, 5.0, 5.0), "under", "the dialog no longer sits at the top-left");
    }

    /// AETHERHOVER (SR67): `cursor` is computed on the hovered element (inherited; a link carries the
    /// UA's `pointer`; `auto` is the I-beam over text, the arrow elsewhere; a url list falls back to
    /// its keyword) and a change is raised once.
    #[test]
    fn test_cursor_follows_the_hovered_element() {
        let html = r#"<html><head><style>
            body { margin: 0; font-size: 16px; }
            div { height: 40px; }
            .busy { cursor: wait; }
            .img { cursor: url(hand.cur) 4 4, move; }
            a.plain { cursor: default; }
        </style></head><body>
            <div id="blank"></div>
            <div class="busy"><span>child inherits</span></div>
            <div class="img"></div>
            <div><a href="/" style="display:block;height:40px"></a></div>
            <div><a class="plain" href="/" style="display:block;height:40px"></a></div>
            <div>Text Text Text</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://cursor", html, true);
        let mut at = |y: f64| {
            engine.handle_event(crate::api::events::Event::MouseMove(5.0, y));
            (engine.cursor().to_string(), engine.take_cursor_change())
        };
        assert_eq!(at(20.0), ("default".into(), None), "an empty block: the arrow, no change raised");
        assert_eq!(at(45.0).0, "wait", "inherited from the .busy parent");
        assert_eq!(at(100.0), ("move".into(), Some("move".into())), "url list falls back to its keyword");
        assert_eq!(at(101.0), ("move".into(), None), "same cursor: nothing raised again");
        assert_eq!(at(140.0).0, "pointer", "a link carries the UA cursor: pointer");
        assert_eq!(at(180.0).0, "default", "an author cursor on the link beats the UA's");
        assert_eq!(at(210.0).0, "text", "auto over a glyph run is the I-beam");
    }

    /// The re-style happens when the hover PATH changes, not per motion: a move inside the same
    /// element neither relayouts nor repaints.
    #[test]
    fn test_hover_restyles_only_when_the_path_changes() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://abc", &NESTED_ABC.replace("</style>", "#a:hover { background-color: red; }</style>"), true);
        engine.handle_event(crate::api::events::Event::MouseMove(10.0, 10.0));
        assert!(engine.needs_repaint, "entering A re-styles (a :hover rule exists)");
        engine.needs_repaint = false;
        engine.handle_event(crate::api::events::Event::MouseMove(12.0, 14.0));
        assert!(!engine.needs_repaint, "moving within A: same path, no re-style");
        engine.handle_event(crate::api::events::Event::MouseMove(50.0, 50.0));
        assert!(engine.needs_repaint, "into B: the path changed");
    }

    /// Custom properties resolve through var() (flat page-global map),
    /// including fallbacks, nesting, and rgb(var(--x)) composition.
    #[test]
    fn test_custom_property_resolution() {
        let html = r#"<html><body><p id="x">a</p><p id="y">b</p></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            :root { --brand-rgb: 10, 20, 30; --accent: #ff0000; }
            #x { color: rgb(var(--brand-rgb)); background-color: var(--missing, var(--accent)); }
            #y { color: var(--nope); }
        "#);
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
            match el.attributes.borrow().get("id") {
                Some("x") => {
                    assert_eq!(paint.color, Some((10, 20, 30)), "rgb(var()) must compose");
                    assert_eq!(paint.background, Some((255, 0, 0)), "fallback chain must resolve");
                }
                Some("y") => assert_eq!(paint.color, None, "unknown var without fallback stays unset"),
                _ => {}
            }
        }
    }

    /// visibility:hidden and opacity:0 keep layout space but paint nothing.
    #[test]
    fn test_visibility_hidden_paints_nothing() {
        let html = r#"<html><body>
            <div id="v" style="width: 100px; height: 40px">SECRET</div>
            <div id="o" style="width: 100px; height: 40px">ALSO</div>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, "#v { visibility: hidden; } #o { opacity: 0; }");
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let id = el.attributes.borrow().get("id").map(|s| s.to_string());
            if matches!(id.as_deref(), Some("v") | Some("o")) {
                let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
                assert_eq!(paint.hidden, Some(true), "{:?} must be paint-hidden", id);
                let l = layout_tree.taffy.layout(*node_id).unwrap();
                assert!(l.size.height >= 40.0, "hidden box must keep its layout space");
            }
        }
        // And the renderer honors it: paint the frame, assert no glyph ink.
        let mut surface = vec![0u8; 800 * 600 * 4];
        crate::render::render_frame(&layout_tree, &mut surface, 800, 600, 0.0, 0.0, &[(0, 0, 800, 600)]);
        let inked = surface.chunks_exact(4).any(|p| p[0] < 200 && p[3] == 255);
        assert!(!inked, "hidden subtrees must leave the surface uninked");
    }

    /// Empty container/portal divs are zero-tall and carry no UA margin, so
    /// a deep app shell does not push its first real content off the fold,
    /// and `border-width: 0` (the opening line of every CSS reset) draws
    /// nothing rather than outlining every box with a hairline.
    #[test]
    fn test_empty_shell_boxes_cost_nothing() {
        let html = r#"<html><body><div><div><div>
            <div id="portals"><div></div><div></div><div></div><div></div><div></div>
            <div></div><div></div><div></div><div></div><div></div></div>
            <p id="lead">LEAD STORY</p>
        </div></div></div></body></html>"#;
        let mut tree = layout::compute_layout(&dom::parse_html(html));
        let find = |tree: &layout::LayoutTree, want: &str| {
            tree.node_map
                .iter()
                .find(|(_, n)| {
                    n.as_element()
                        .and_then(|e| e.attributes.borrow().get("id").map(str::to_string))
                        .as_deref()
                        == Some(want)
                })
                .map(|(id, _)| *id)
                .unwrap()
        };
        let portals = find(&tree, "portals");
        assert_eq!(
            tree.taffy.layout(portals).unwrap().size.height,
            0.0,
            "ten empty divs must occupy no vertical space"
        );
        // The lead paragraph sits near the top, not ~250px down.
        let mut y = 0.0;
        let mut cur = find(&tree, "lead");
        loop {
            y += tree.taffy.layout(cur).unwrap().location.y;
            match tree.taffy.parent(cur) {
                Some(p) => cur = p,
                None => break,
            }
        }
        assert!(y < 40.0, "lead content pushed to y={} by empty shell boxes", y);

        // border-width:0 with a style and colour still set paints nothing.
        css::apply_css(&mut tree, "* { border: 1px solid rgb(200,0,0); border-width: 0 }");
        let mut surface = vec![255u8; 800 * 600 * 4];
        crate::render::render_frame(&tree, &mut surface, 800, 600, 0.0, 0.0, &[(0, 0, 800, 600)]);
        let red = surface
            .chunks_exact(4)
            .filter(|p| p[2] > 150 && p[1] < 80 && p[0] < 80)
            .count();
        assert_eq!(red, 0, "zero-width borders must not paint ({} px inked)", red);
    }

    /// Script collection preserves document order and skips non-JS types;
    /// the classList/querySelectorAll/documentElement DOM surface drives
    /// real mutations that survive into relayout.
    #[test]
    fn test_script_pipeline_and_dom_surface() {
        let html = r#"<html><head>
            <script src="/a.js"></script>
            <script type="application/ld+json">{"not":"js"}</script>
            <script>inline1()</script>
            <script src="https://cdn.x.com/b.js"></script>
        </head><body></body></html>"#;
        let (slots, external) = crate::net::collect_scripts("https://example.com/", html);
        assert_eq!(slots.len(), 3, "ld+json must not occupy a slot");
        assert_eq!(slots[1].text.as_deref(), Some("inline1()"));
        // Ordinals address the ELEMENT list, which the ld+json script is in:
        // slot 1 is element 2, slot 2 is element 3.
        assert_eq!(
            slots.iter().map(|s| s.ordinal).collect::<Vec<_>>(),
            vec![0, 2, 3],
            "slot ordinals must skip the filtered ld+json element"
        );
        assert_eq!(external, vec![
            (0, "https://example.com/a.js".to_string()),
            (2, "https://cdn.x.com/b.js".to_string()),
        ]);

        // DOM surface: the canonical app-shell boot line + query breadth.
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html class="no-js"><body>
                <div class="menu hidden" id="m">menu</div>
                <p class="item">a</p><p class="item">b</p>
            </body></html>"#,
            &[],
            true,
        );
        let js = engine.js_engine.as_mut().unwrap();
        js.execute(r#"
            document.documentElement.classList.remove('no-js');
            document.documentElement.classList.add('js');
            var m = document.getElementById('m');
            m.classList.toggle('hidden');
            m.removeAttribute('data-x');
            var items = document.querySelectorAll('p.item');
            m.className = m.className + ' count-' + items.length;
        "#).expect("script must run clean");
        let doc = engine.document.clone().unwrap();
        let html_el = doc.select("html").unwrap().next().unwrap();
        assert_eq!(html_el.attributes.borrow().get("class"), Some("js"));
        let m = doc.select("#m").unwrap().next().unwrap();
        assert_eq!(m.attributes.borrow().get("class"), Some("menu count-2"),
            "toggle must drop 'hidden'; querySelectorAll must count 2");
    }

    /// The collapsed-menu pattern: max-height:0 + overflow:hidden must
    /// clamp layout AND clip paint (its text was smearing over the page).
    #[test]
    fn test_max_height_overflow_clip() {
        let html = r#"<html><body>
            <div id="menu">MENU CONTENT THAT MUST NOT PAINT</div>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, "#menu { max-height: 0; overflow: hidden; }");
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            if el.attributes.borrow().get("id") == Some("menu") {
                let l = layout_tree.taffy.layout(*node_id).unwrap();
                assert_eq!(l.size.height, 0.0, "max-height:0 must clamp the box");
            }
        }
        let mut surface = vec![0u8; 800 * 600 * 4];
        crate::render::render_frame(&layout_tree, &mut surface, 800, 600, 0.0, 0.0, &[(0, 0, 800, 600)]);
        let inked = surface.chunks_exact(4).any(|p| p[0] < 200 && p[3] == 255);
        assert!(!inked, "clipped menu must leave no ink");
    }

    /// setTimeout(0) work queued during page scripts runs BEFORE first
    /// layout (load drains the job queue).
    #[test]
    fn test_boot_timers_drain_before_layout() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>
                 setTimeout(function () {
                   document.getElementById('x').setAttribute('data-boot', 'ran');
                 }, 0);
               </script></body></html>"#,
            &[],
            true,
        );
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        assert_eq!(x.attributes.borrow().get("data-boot"), Some("ran"),
            "zero-delay boot timer must run during load");
    }

    /// Reads `expr` off a booted page as a number.
    fn js_num(engine: &mut crate::AetherEngine, expr: &str) -> f64 {
        let js = engine.js_engine.as_mut().expect("page must have booted");
        js.eval_number(expr).expect("probe must evaluate to a number")
    }

    /// THE incident regression: a `setTimeout` callback that re-arms itself
    /// must not be able to hold a tick forever.
    ///
    /// The old wiring made every timer an immediate boa job, and `run_jobs`
    /// drains until the queue is EMPTY — a self-re-arming poll timer (which is
    /// what google's search UI starts on typing) refilled it faster than the
    /// drain emptied it, so `tick()` never returned and the shell's select!
    /// loop starved. Now the due set is snapshotted before any callback runs:
    /// each tick fires exactly one generation, whatever the callback does.
    #[test]
    fn test_self_rearming_timer_fires_one_generation_per_tick() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>
                 window.__n = 0;
                 function poll() { window.__n++; setTimeout(poll, 0); }
                 setTimeout(poll, 0);
               </script></body></html>"#,
            &[],
            true,
        );
        // The load itself terminated (it would not have, before), and the boot
        // drain is bounded to its pass count rather than the queue emptying.
        let boot = js_num(&mut engine, "window.__n");
        assert!(boot >= 1.0, "a zero-delay boot timer must run during load");
        assert!(
            boot <= 3.0 * crate::event_loop::BOOT_PASSES as f64,
            "boot drain must be bounded to its passes (3 stages x {}), saw {}",
            crate::event_loop::BOOT_PASSES,
            boot
        );
        // From here the clock only moves when we move it: each tick must add
        // exactly one generation, no matter how eagerly the callback re-arms.
        // (Past nesting level 5 HTML clamps the re-arm to 4 ms, so each tick
        // first moves the clock by that much.)
        crate::event_loop::freeze_clock();
        for i in 1..=5 {
            crate::event_loop::advance_clock(4);
            engine.tick();
            assert_eq!(
                js_num(&mut engine, "window.__n"),
                boot + i as f64,
                "tick {} must fire exactly one generation",
                i
            );
        }
    }

    /// clearTimeout cancels a pending timer — both halves, the Rust record and
    /// the JS callback slot.
    #[test]
    fn test_clear_timeout_cancels() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>window.__n = 0;</script></body></html>"#,
            &[],
            true,
        );
        crate::event_loop::freeze_clock();
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute(
                "window.__id = setTimeout(function () { window.__n++; }, 50);
                 window.__kept = setTimeout(function () { window.__n += 100; }, 50);
                 clearTimeout(window.__id);",
            )
            .expect("arming must run clean");
        }
        assert_eq!(crate::event_loop::armed_count(), 1, "cleared timer must leave the queue");
        crate::event_loop::advance_clock(50);
        engine.tick();
        assert_eq!(
            js_num(&mut engine, "window.__n"),
            100.0,
            "the cleared callback must never run; the other one must"
        );
    }

    /// setInterval repeats across ticks and clearInterval stops it. The clock
    /// is the seam — no test anywhere sleeps.
    #[test]
    fn test_set_interval_repeats_until_cleared() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>window.__n = 0;</script></body></html>"#,
            &[],
            true,
        );
        crate::event_loop::freeze_clock();
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute("window.__iv = setInterval(function () { window.__n++; }, 50);")
                .expect("arming must run clean");
        }
        engine.tick();
        assert_eq!(js_num(&mut engine, "window.__n"), 0.0, "not due yet");
        for i in 1..=3 {
            crate::event_loop::advance_clock(50);
            engine.tick();
            assert_eq!(js_num(&mut engine, "window.__n"), i as f64, "one fire per period");
        }
        // A period that elapsed several times over still fires ONCE per tick:
        // re-arming happens at fire time, so a fallen-behind interval never
        // bursts to catch up.
        crate::event_loop::advance_clock(500);
        engine.tick();
        assert_eq!(js_num(&mut engine, "window.__n"), 4.0, "no catch-up burst");

        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute("clearInterval(window.__iv);").expect("clear must run clean");
        }
        assert_eq!(crate::event_loop::armed_count(), 0, "clearInterval empties the queue");
        crate::event_loop::advance_clock(500);
        engine.tick();
        assert_eq!(js_num(&mut engine, "window.__n"), 4.0, "cleared interval must stop");
    }

    /// MessageChannel delivers through the timer queue — React's scheduler
    /// yields on exactly this, so a port that never delivers stalls a page.
    #[test]
    fn test_message_channel_delivers_through_timer_queue() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>
                 var ch = new MessageChannel();
                 ch.port1.onmessage = function (e) {
                   document.getElementById('x').setAttribute('data-msg', String(e.data));
                 };
                 ch.port2.postMessage('yielded');
               </script></body></html>"#,
            &[],
            true,
        );
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        assert_eq!(
            x.attributes.borrow().get("data-msg"),
            Some("yielded"),
            "postMessage must be delivered by the boot drain"
        );
    }

    /// el.style.x = ... writes the inline style attribute (replacing the
    /// property, keeping the rest) and traversal getters walk elements.
    #[test]
    fn test_style_assignment_and_traversal() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body>
                <div id="wrap" style="color: red; display: block">
                    <p id="a">one</p>
                    text between
                    <p id="b">two</p>
                </div>
            </body></html>"#,
            &[],
            true,
        );
        let js = engine.js_engine.as_mut().unwrap();
        js.execute(r#"
            var w = document.getElementById('wrap');
            w.style.display = 'none';
            w.style.maxHeight = '0';
            var a = document.getElementById('a');
            var result = [
                w.style.display,
                a.parentNode.id,
                w.children.length,
                w.firstElementChild.id,
                a.nextElementSibling.id,
                a.tagName,
            ].join('|');
            document.getElementById('b').setAttribute('data-r', result);
        "#).expect("script must run clean");
        let doc = engine.document.clone().unwrap();
        let w = doc.select("#wrap").unwrap().next().unwrap();
        let style = w.attributes.borrow().get("style").unwrap().to_string();
        assert!(style.contains("color: red"), "untouched decls survive: {}", style);
        assert!(style.contains("display: none"), "assignment replaces: {}", style);
        assert!(style.contains("max-height: 0"), "camelCase maps to kebab: {}", style);
        let b = doc.select("#b").unwrap().next().unwrap();
        assert_eq!(b.attributes.borrow().get("data-r"), Some("none|wrap|2|a|b|P"));
    }

    /// The fetch wrapper is whatwg-shaped: Promise → Response.ok/json(),
    /// verified offline through a `data:` URL (fetched without a network; AETHERNET's streaming
    /// fetch over a live server is `tests/net.rs`).
    #[test]
    fn test_fetch_wrapper_shape() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div></body></html>"#,
            &[],
            true,
        );
        let js = engine.js_engine.as_mut().unwrap();
        js.execute(r#"
            fetch('data:application/json,{"n": 7, "method": "POST"}', { method: 'POST', body: 'p=1' })
                .then(function (r) { return r.ok ? r.json() : null; })
                .then(function (j) {
                    document.getElementById('x').setAttribute('data-n', String(j.n) + j.method);
                });
        "#).expect("fetch chain must run");
        js.checkpoint();
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        assert_eq!(x.attributes.borrow().get("data-n"), Some("7POST"),
            "fetch → json → DOM mutation must complete through the job queue");
    }

    /// rAF callbacks fire once per load in bounded passes: a framework
    /// paint lands in the DOM; a self-re-registering animation loop
    /// terminates instead of recursing; an asynchronous XHR completes on the
    /// event loop (the boot drain), not inside `send()` (XHR §3.5.6).
    #[test]
    fn test_raf_bounded_drain_and_xhr() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div>
               <script>
                 var loops = 0;
                 function animate() { loops++; requestAnimationFrame(animate); }
                 requestAnimationFrame(animate);
                 requestAnimationFrame(function (ts) {
                   document.getElementById('x').setAttribute('data-paint', 'ts' + (ts > 0));
                 });
                 var xhr = new XMLHttpRequest();
                 xhr.open('GET', 'data:text/plain,ok');
                 xhr.onload = function () {
                   document.getElementById('x').setAttribute('data-xhr', 'state' + xhr.readyState + xhr.responseText);
                 };
                 xhr.send();
                 document.getElementById('x').setAttribute('data-xhr-sync', String(xhr.readyState));
                 document.getElementById('x').setAttribute('data-loops-pre', String(loops));
               </script></body></html>"#,
            &[],
            true,
        );
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        let attrs = x.attributes.borrow();
        assert_eq!(attrs.get("data-paint"), Some("tstrue"), "one-shot rAF must fire with a timestamp");
        assert_eq!(attrs.get("data-xhr"), Some("state4ok"), "XHR onload fires on the event loop with the body");
        assert_eq!(attrs.get("data-xhr-sync"), Some("1"), "an async XHR is still OPENED when send() returns");
        assert_eq!(attrs.get("data-loops-pre"), Some("0"), "rAF must NOT fire during script execution");
        // The animation loop ran bounded passes (8) and stopped — the load
        // completing at all proves termination.
    }

    /// Clicking a media element (or a child of one) stages a PlayMedia
    /// handoff with the page-resolved stream — the charter passthrough.
    #[test]
    fn test_media_click_stages_playmedia() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "file:///site/watch/",
            r#"<html><body>
                <video style="width: 300px; height: 150px">
                    <source src="/media/clip.webm" type="video/webm">
                </video>
                <p>after</p>
            </body></html>"#,
            &[],
            true,
        );
        engine.render_frame();
        // Click inside the video box (top-left area of the page).
        engine.handle_event(crate::api::events::Event::MouseDown(50.0, 50.0));
        engine.handle_event(crate::api::events::Event::MouseUp(50.0, 50.0));
        let staged = engine.take_pending_media().expect("click must stage media");
        assert_eq!(staged.0, "file:///media/clip.webm");
        assert_eq!(staged.2, "video/webm");
        assert!(engine.take_pending_media().is_none(), "take must consume");

        // media_sources still reports the page's streams for enumeration.
        let sources = engine.media_sources();
        assert_eq!(sources, vec![("file:///media/clip.webm".to_string(), "video/webm".to_string())]);
        // (an http(s) page's media is fetched into the cache first and staged as that file —
        // AUDIOTRACK M4, `media::tests::http_media_is_cached_then_handed_to_stria_as_a_file…`)
    }

    /// History: load A, B, C → back lands on B then A; a new load from B
    /// drops the forward tail. (Truncate-at-index wiped the whole history,
    /// so Back never worked in the shell.)
    #[test]
    fn test_history_back_forward() {
        let mut engine = crate::AetherEngine::new();
        for url in ["https://a.test/", "https://b.test/", "https://c.test/"] {
            engine.load_html_styled(url, "<html><body>x</body></html>", &[], true);
        }
        assert_eq!(engine.history.len(), 3, "three loads must record three entries");
        assert_eq!(engine.get_back_url().as_deref(), Some("https://b.test/"));
        assert_eq!(engine.get_back_url().as_deref(), Some("https://a.test/"));
        assert_eq!(engine.get_back_url(), None, "at the oldest entry");
        assert_eq!(engine.get_forward_url().as_deref(), Some("https://b.test/"));
        // New navigation from B drops C.
        engine.load_html_styled("https://d.test/", "<html><body>x</body></html>", &[], true);
        assert_eq!(engine.history, vec!["https://a.test/", "https://b.test/", "https://d.test/"]);
        assert_eq!(engine.get_forward_url(), None, "forward tail dropped");
    }

    /// noscript fallback and iframe innards must not render (scripting is
    /// enabled; frames are unsupported empty boxes).
    #[test]
    fn test_noscript_iframe_hidden() {
        let html = r#"<html><body>
            <noscript>&lt;iframe src="x"&gt;NOSCRIPT LEAK&lt;/iframe&gt;</noscript>
            <iframe src="https://x.test/">FRAME FALLBACK</iframe>
            <p>visible</p>
        </body></html>"#;
        let document = dom::parse_html(html);
        let layout_tree = layout::compute_layout(&document);
        for (_, dom_node) in &layout_tree.node_map {
            let text = dom_node.text_contents();
            if dom_node.as_text().is_some() {
                assert!(!text.contains("NOSCRIPT LEAK"), "noscript content must produce no box");
                assert!(!text.contains("FRAME FALLBACK"), "iframe content must produce no box");
            }
        }
    }

    /// Lifecycle: DOMContentLoaded/load fire during load (document AND
    /// window registrations); srcset picks the viewport-fit candidate;
    /// scroll clamps to the document.
    #[test]
    fn test_lifecycle_srcset_scroll_clamp() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x" style="height: 900px">tall</div>
               <script>
                 document.addEventListener('DOMContentLoaded', function () {
                   document.getElementById('x').setAttribute('data-dcl', 'y');
                 });
                 window.addEventListener('load', function () {
                   document.getElementById('x').setAttribute('data-load', 'y');
                 });
               </script></body></html>"#,
            &[],
            true,
        );
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        assert_eq!(x.attributes.borrow().get("data-dcl"), Some("y"), "DOMContentLoaded fires at load");
        assert_eq!(x.attributes.borrow().get("data-load"), Some("y"), "window load fires at load");

        // Scroll clamp: content ~900px+chrome, viewport 600 — huge scroll
        // must stop near (content - viewport), not run away.
        engine.handle_event(crate::api::events::Event::Scroll(0.0, 100000.0));
        assert!(engine.scroll_y < 2000.0, "scroll must clamp to the document: {}", engine.scroll_y);
        assert!(engine.scroll_y > 100.0, "tall page must allow some scroll: {}", engine.scroll_y);

        // srcset: smallest sufficient width wins; density ranks vs 800.
        assert_eq!(
            crate::images::pick_srcset("/a-400.png 400w, /a-1200.png 1200w, /a-900.png 900w"),
            Some("/a-900.png".to_string())
        );
        assert_eq!(
            crate::images::pick_srcset("/lo.png 1x, /hi.png 2x"),
            Some("/lo.png".to_string()),
            "1x = 800 effective, smallest sufficient"
        );
        assert_eq!(
            crate::images::pick_srcset("/only-small.png 200w"),
            Some("/only-small.png".to_string())
        );
    }

    /// Per-side borders, text-decoration override, white-space:nowrap.
    #[test]
    fn test_side_borders_decoration_nowrap() {
        let html = r#"<html><body>
            <div id="b" style="width: 60px; height: 30px">x</div>
            <a id="a" href="/">nolink</a>
            <span id="n">one two three four five six seven eight nine ten</span>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            #b { border-bottom: 3px solid red; }
            #a { text-decoration: none; }
            #n { white-space: nowrap; }
        "#);
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
            match el.attributes.borrow().get("id") {
                Some("b") => {
                    let sides = paint.border.expect("side border set");
                    assert_eq!(sides[2], Some((3.0, (255, 0, 0))), "bottom side only");
                    assert_eq!(sides[0], None, "top untouched");
                    let s = layout_tree.taffy.style(*node_id).unwrap();
                    assert_eq!(s.border.bottom, taffy::style::LengthPercentage::length(3.0));
                    assert_eq!(s.border.top, taffy::style::LengthPercentage::length(0.0));
                }
                Some("a") => assert_eq!(paint.underline, Some(false), "decoration none overrides link default"),
                Some("n") => {
                    assert_eq!(paint.nowrap, Some(true));
                    let l = layout_tree.taffy.layout(*node_id).unwrap();
                    assert!(l.size.height < 40.0, "nowrap text must measure one line: {}", l.size.height);
                }
                _ => {}
            }
        }
    }

    /// font-family classes: css keywords map to sans/serif/mono; code-ish
    /// tags default to monospace; the mono face measures wider than sans.
    #[test]
    fn test_font_family_classes() {
        let html = r#"<html><body>
            <p id="s">iiiiiiiiii</p>
            <code id="m">iiiiiiiiii</code>
            <p id="g" style="font-family: Georgia, serif">x</p>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, "");
        // Measure the TEXT LEAVES (the block boxes stretch to the row).
        let mut sans_w = 0.0f32;
        let mut mono_w = 0.0f32;
        for (node_id, dom_node) in &layout_tree.node_map {
            if let Some(el) = dom_node.as_element() {
                let paint = layout_tree.paint_map.get(node_id).cloned().unwrap_or_default();
                if el.attributes.borrow().get("id") == Some("g") {
                    let fam = paint.family.expect("a family list");
                    assert_eq!(crate::fonts::serialize_family_list(&crate::fonts::family_list(fam)), "Georgia, serif");
                    // Georgia is not installed: the generic serif (Times New Roman → Liberation Serif) is used
                    let face = crate::fonts::face(&crate::fonts::FontSel::new(fam, 400, false)).map(|f| f.family.clone());
                    let serif = crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::SERIF, 400, false)).map(|f| f.family.clone());
                    assert_eq!(face, serif, "Georgia, serif resolves through serif");
                }
                continue;
            }
            if dom_node.as_text().is_some() && dom_node.text_contents().trim() == "iiiiiiiiii" {
                let w = layout_tree.taffy.layout(*node_id).unwrap().size.width;
                let parent_tag = dom_node
                    .parent()
                    .and_then(|p| p.as_element().map(|e| e.name.local.as_ref().to_string()))
                    .unwrap_or_default();
                if parent_tag == "code" { mono_w = w; } else { sans_w = w; }
            }
        }
        // Ten 'i's: monospace is far wider than proportional sans.
        assert!(mono_w > sans_w * 1.5,
            "mono must measure wider: mono={} sans={}", mono_w, sans_w);
    }

    /// The sprite-sheet idiom: a data:-URI-bearing declaration block must
    /// survive the `;` inside the URI, and background-position / -size /
    /// -repeat must all reach the paint map (they are what selects the
    /// slice of the sheet; without them the whole sheet paints).
    #[test]
    fn test_sprite_background_declarations_reach_paint() {
        let html = r#"<html><body><span id="s">Wikipedia</span></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(
            &mut layout_tree,
            "#s { background: url(data:image/svg+xml;base64,AAAA) no-repeat; \
                  background-position: 0 -304px; background-size: 100%; \
                  text-indent: -10000px; mask-image: url(m.svg) }",
        );
        let p = layout_tree
            .node_map
            .iter()
            .find_map(|(id, n)| {
                let el = n.as_element()?;
                (el.attributes.borrow().get("id") == Some("s")).then(|| layout_tree.paint_map.get(id))?
            })
            .expect("styled span has a paint entry");
        assert_eq!(p.bg_image.as_deref(), Some("data:image/svg+xml;base64,AAAA"),
            "the `;` inside a data: URI must not split the declaration");
        assert_eq!(p.bg_position.as_deref(), Some("0 -304px"));
        assert_eq!(p.bg_size.as_deref(), Some("100%"));
        assert_eq!(p.bg_repeat, Some(1));
        assert_eq!(p.mask_image.as_deref(), Some("m.svg"));
        // Image replacement hides the TEXT, never the box's background.
        assert_eq!(p.text_hidden, Some(true));
        assert_eq!(p.hidden, None);
    }

    /// CSS math functions resolve to pixels: nesting, operator precedence,
    /// unit mixing, min/max/clamp, and an honest None for a percentage with
    /// no reference.
    #[test]
    fn test_css_math_functions() {
        use crate::css::{eval_length, parse_px};
        assert_eq!(eval_length("calc(1rem + 4px)", None), Some(20.0));
        assert_eq!(eval_length("calc(max(calc(1rem + 4px),10px))", None), Some(20.0));
        assert_eq!(eval_length("max(calc(1rem - 4px),10px)", None), Some(12.0));
        assert_eq!(eval_length("min(2rem, 12px, 40px)", None), Some(12.0));
        assert_eq!(eval_length("clamp(10px, 1rem, 12px)", None), Some(12.0));
        assert_eq!(eval_length("calc(2px + 3px * 4)", None), Some(14.0));
        assert_eq!(eval_length("calc(100% - 20px)", Some(200.0)), Some(180.0));
        // No percentage reference: unresolvable, not a silent 0.
        assert_eq!(eval_length("calc(100% - 20px)", None), None);
        assert_eq!(eval_length("calc(1px / 0)", None), None);
        assert_eq!(eval_length("rotate(20deg)", None), None);
        // parse_px routes math functions through the evaluator; plain
        // lengths keep their old fast path.
        assert_eq!(parse_px("calc(1rem + 4px)"), Some(20.0));
        assert_eq!(parse_px("12px"), Some(12.0));
    }

    /// The component-CSS icon idiom: the box is sized by `calc()`, and the
    /// mask geometry lives behind `@supports (mask-image: none)`. Both must
    /// land, or the stencil paints oversized in a min-width-floor box (the
    /// icon shows only its top-left corner).
    #[test]
    fn test_supports_mask_branch_and_calc_icon_box() {
        let html = r#"<html><body><span id="i" class="icon"></span></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(
            &mut layout_tree,
            ".icon { min-width: 10px; min-height: 10px; display: inline-block; \
                     width: calc(var(--fs, 1rem) + 4px); height: calc(var(--fs, 1rem) + 4px); \
                     mask-image: url(icon.svg) } \
             @supports not ((-webkit-mask-image: none) or (mask-image: none)) { \
                .icon { background-size: 99px } } \
             @supports (-webkit-mask-image: none) or (mask-image: none) { \
                .icon { mask-position: center; mask-repeat: no-repeat; \
                        mask-size: calc(max(calc(var(--fs, 1rem) + 4px),10px)) } }",
        );
        let (node_id, _) = layout_tree
            .node_map
            .iter()
            .find(|(_, n)| {
                n.as_element()
                    .is_some_and(|e| e.attributes.borrow().get("id") == Some("i"))
            })
            .map(|(id, n)| (*id, n.clone()))
            .expect("styled span is in the layout tree");
        let p = layout_tree.paint_map.get(&node_id).cloned().unwrap_or_default();
        assert_eq!(p.mask_position.as_deref(), Some("center"), "@supports mask branch must apply");
        assert_eq!(p.mask_repeat, Some(1));
        assert_eq!(p.mask_size.as_deref(), Some("calc(max(calc(1rem + 4px),10px))"));
        // The `@supports not (...)` fallback branch must NOT apply.
        assert_eq!(p.bg_size, None, "mask support means the background fallback is dead");
        let l = layout_tree.taffy.layout(node_id).unwrap();
        assert_eq!(l.size.width, 20.0, "calc() width must beat the min-width floor");
        assert_eq!(l.size.height, 20.0);
    }

    /// A mask sized by a math function must fit the box exactly: the sampler
    /// covers the whole element, not a corner of an oversized stencil.
    #[test]
    fn test_mask_geometry_fits_box() {
        // 20x20 element, 20x20 stencil, mask-size max(calc(1rem+4px),10px):
        // the four corners of the box map to the four corners of the image.
        let g = crate::render::test_bg_geometry(
            Some("max(calc(1rem + 4px),10px)"),
            Some("center"),
            Some(1),
            20.0,
            20.0,
            20.0,
            20.0,
        );
        assert_eq!((g.0, g.1, g.2, g.3), (0.0, 0.0, 20.0, 20.0));
        // Without math support the size would fall back to intrinsic and a
        // half-size box would clip; check the centring path too.
        let g = crate::render::test_bg_geometry(
            Some("calc(1rem - 6px)"),
            Some("center"),
            Some(1),
            20.0,
            20.0,
            20.0,
            20.0,
        );
        assert_eq!((g.0, g.1, g.2, g.3), (5.0, 5.0, 10.0, 10.0));
    }

    /// A later, more specific `opacity: 1` un-hides what `opacity: 0` hid,
    /// and `border-color: transparent` drops the UA control stroke (SR1112: the sides kept at alpha 0, painting nothing).
    #[test]
    fn test_opacity_and_transparent_border_override() {
        let html = r#"<html><body class="on"><button class="b" id="q">go</button></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(
            &mut layout_tree,
            ".b { opacity: 0; border: 1px solid #000 } \
             .on .b { opacity: 1; border-color: transparent }",
        );
        let p = layout_tree
            .node_map
            .iter()
            .find_map(|(id, n)| {
                let el = n.as_element()?;
                (el.attributes.borrow().get("id") == Some("q")).then(|| layout_tree.paint_map.get(id))?
            })
            .expect("button has a paint entry");
        assert_eq!(p.hidden, Some(false), "opacity:1 must win over an earlier opacity:0");
        assert_eq!(p.border, Some([Some((1.0, (0, 0, 0))); 4]), "SR1112 (flipped in place; was Some([None; 4])): the sides are kept, black RGB");
        assert_eq!(p.border_alpha, Some([Some(0.0); 4]), "SR1112: transparent border-color = alpha 0 on every side, no stroke (the UA frame stays off: spec.border is Some)");
    }

    /// float:left/right sizes to content and hugs its edge (approximation).
    #[test]
    fn test_float_approximation() {
        let html = r#"<html><body><div id="f">float</div><div>after</div></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, "#f { float: right; }");
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            if el.attributes.borrow().get("id") == Some("f") {
                let s = layout_tree.taffy.style(*node_id).unwrap();
                assert_eq!(s.align_self, Some(taffy::style::AlignSelf::END));
                assert_eq!(s.size.width, taffy::style::Dimension::auto());
            }
        }
    }

    #[test]
    fn test_document_cookie_round_trips_through_the_jar() {
        // Script-set cookies land in the same session jar the network stack
        // sends from, and read back through the accessor for this origin.
        let html = r#"<html><body><div id="out"></div>
            <script>
                document.cookie = 'theme=dark; Path=/';
                document.cookie = 'sid=xyz; Path=/';
                document.getElementById('out').textContent = document.cookie;
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://cookie-doc.example/page", html, true);

        let doc = engine.document.as_ref().unwrap();
        let out = doc.select("#out").unwrap().next().unwrap();
        let seen = out.as_node().text_contents();
        assert!(seen.contains("theme=dark"), "document.cookie read back {seen:?}");
        assert!(seen.contains("sid=xyz"), "document.cookie read back {seen:?}");
        // Same jar the wire uses, and host-scoped.
        let sent = crate::net::cookies_for("https://cookie-doc.example/other");
        assert!(sent.contains("theme=dark"), "jar would send {sent:?}");
        assert_eq!(crate::net::cookies_for("https://elsewhere.example/"), "");
    }

    #[test]
    fn test_create_element_direct_construction_and_platform_breadth() {
        // createElement builds the node directly: unknown/custom tags work,
        // and the result appends into the live tree.
        let html = r#"<html><body><div id="host"></div>
            <script>
                var custom = document.createElement('my-widget');
                custom.setAttribute('data-k', 'v');
                custom.textContent = 'hi';
                document.getElementById('host').appendChild(custom);
                var c = document.createComment('note');
                document.getElementById('host').appendChild(c);
                var probe = document.createElement('div');
                probe.style.display = 'none';
                probe.id = 'probe';
                probe.textContent = [
                    typeof queueMicrotask,
                    typeof MutationObserver,
                    typeof MessageChannel,
                    btoa('hi'),
                    atob('aGk='),
                    getComputedStyle(custom).display,
                    document.readyState,
                ].join('|');
                document.getElementById('host').appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://create", html, true);

        let doc = engine.document.as_ref().unwrap();
        let made = doc.select("#host my-widget").unwrap().next()
            .expect("custom tag must construct and attach");
        assert_eq!(made.attributes.borrow().get("data-k"), Some("v"));
        assert_eq!(made.as_node().text_contents(), "hi");

        let probe = doc.select("#probe").unwrap().next().unwrap();
        let fields: Vec<String> = probe.as_node().text_contents()
            .split('|').map(|s| s.to_string()).collect();
        assert_eq!(fields[0], "function", "queueMicrotask");
        assert_eq!(fields[1], "function", "MutationObserver");
        assert_eq!(fields[2], "function", "MessageChannel");
        assert_eq!(fields[3], "aGk=", "btoa");
        assert_eq!(fields[4], "hi", "atob");
        // Inline style wins in getComputedStyle; nothing is invented.
        assert_eq!(fields[5], "block", "initial display for an undeclared element");
        assert_eq!(fields[6], "loading", "readyState while scripts execute");
        // ...and it advances by the time the lifecycle events have fired.
        assert!(doc.select("#host").unwrap().next().is_some());
    }

    /// URL/URLSearchParams over the `url` crate, and UTF-8
    /// TextEncoder/TextDecoder. Relative resolution, component writes and
    /// the live searchParams link are all checked against the real answers.
    #[test]
    fn test_url_and_text_codec_platform_apis() {
        let html = r#"<html><body><div id="host"></div>
            <script>
                var u = new URL('../c/d.html?x=1&y=two#frag', 'https://ex.test:8443/a/b/page');
                var sp = new URLSearchParams('a=1&b=hello+world&a=2');
                var u2 = new URL('https://ex.test/p');
                u2.searchParams.set('q', 'a b&c');
                u2.pathname = '/other';
                var enc = new TextEncoder().encode('héllo€');
                var dec = new TextDecoder().decode(enc);
                var probe = document.createElement('div');
                probe.id = 'probe';
                probe.textContent = [
                    u.href, u.origin, u.hostname, u.port, u.pathname, u.search, u.hash,
                    u.searchParams.get('y'),
                    sp.getAll('a').join(','), sp.get('b'), sp.toString(),
                    u2.href,
                    Array.prototype.slice.call(enc).join(','),
                    dec, String(dec === 'héllo€'),
                ].join('|');
                document.getElementById('host').appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://url", html, true);
        let doc = engine.document.as_ref().unwrap();
        let probe = doc.select("#probe").unwrap().next().expect("probe must attach");
        let f: Vec<String> = probe.as_node().text_contents().split('|').map(String::from).collect();
        assert_eq!(f[0], "https://ex.test:8443/a/c/d.html?x=1&y=two#frag", "relative resolution");
        assert_eq!(f[1], "https://ex.test:8443", "origin");
        assert_eq!(f[2], "ex.test", "hostname");
        assert_eq!(f[3], "8443", "port");
        assert_eq!(f[4], "/a/c/d.html", "pathname");
        assert_eq!(f[5], "?x=1&y=two", "search");
        assert_eq!(f[6], "#frag", "hash");
        assert_eq!(f[7], "two", "searchParams.get");
        assert_eq!(f[8], "1,2", "getAll keeps repeats in order");
        assert_eq!(f[9], "hello world", "'+' decodes to space");
        assert_eq!(f[10], "a=1&b=hello+world&a=2", "form-urlencoded round trip");
        assert_eq!(f[11], "https://ex.test/other?q=a+b%26c", "live searchParams + path write");
        // 'héllo€' = 68 C3A9 6C 6C 6F E282AC
        assert_eq!(f[12], "104,195,169,108,108,111,226,130,172", "UTF-8 encode");
        assert_eq!(f[14], "true", "decode round-trips");
    }

    /// AbortController's object graph, DOMParser over the real HTML parser,
    /// and crypto backed by OS entropy.
    #[test]
    fn test_abort_domparser_and_crypto() {
        let html = r#"<html><body><div id="host"></div>
            <script>
                var log = [];
                var ac = new AbortController();
                ac.signal.addEventListener('abort', function () { log.push('l1'); });
                ac.signal.onabort = function () { log.push('onabort'); };
                var before = ac.signal.aborted;
                ac.abort('why');
                ac.abort('twice');

                var pd = new DOMParser().parseFromString(
                    '<html><body><p class="x">parsed</p><i id="j">y</i></body></html>', 'text/html');
                var pText = pd.querySelector('p.x').textContent;
                var pId = pd.getElementById('j').textContent;

                var a = new Uint8Array(16), b = new Uint8Array(16);
                crypto.getRandomValues(a); crypto.getRandomValues(b);
                var same = 0, inRange = true;
                for (var i = 0; i < 16; i++) {
                    if (a[i] === b[i]) { same++; }
                    if (!(a[i] >= 0 && a[i] <= 255)) { inRange = false; }
                }
                var uu = crypto.randomUUID();

                var probe = document.createElement('div');
                probe.id = 'probe';
                probe.textContent = [
                    String(before), String(ac.signal.aborted), String(ac.signal.reason),
                    log.join(','), pText, pId,
                    String(inRange), String(same < 12),
                    String(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(uu)),
                    String(uu !== crypto.randomUUID()),
                ].join('|');
                document.getElementById('host').appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://abort", html, true);
        let doc = engine.document.as_ref().unwrap();
        let probe = doc.select("#probe").unwrap().next().expect("probe must attach");
        let f: Vec<String> = probe.as_node().text_contents().split('|').map(String::from).collect();
        assert_eq!(f[0], "false", "signal starts unaborted");
        assert_eq!(f[1], "true", "abort flips aborted");
        assert_eq!(f[2], "why", "reason is recorded");
        // DOM §2.10 / HTML §8.1.8.1: the `onabort` handler's listener is registered when the handler is
        // first set — after `l1` here — so it runs second (AETHERNET made AbortSignal a real EventTarget).
        assert_eq!(f[3], "l1,onabort", "abort dispatches once, to listeners in registration order");
        assert_eq!(f[4], "parsed", "DOMParser document answers querySelector");
        assert_eq!(f[5], "y", "DOMParser document answers getElementById");
        assert_eq!(f[6], "true", "getRandomValues stays inside the element width");
        assert_eq!(f[7], "true", "two draws differ — the fill is not fixed");
        assert_eq!(f[8], "true", "randomUUID is a well-formed v4");
        assert_eq!(f[9], "true", "randomUUID does not repeat");
    }

    /// `element.dataset` is a live view over `data-*` attributes, in both
    /// directions and through delete — a snapshot object would pass the
    /// first read and lose everything after it. Next.js reads
    /// `documentElement.dataset.dplId` and then deletes it during its
    /// bootstrap, which is the shape exercised here.
    #[test]
    fn dataset_is_a_live_view_over_data_attributes() {
        let html = r#"<html><body>
            <div id="host" data-dpl-id="sha-1" data-plain="p"></div>
            <script>
                var h = document.getElementById('host');
                var out = [];
                out.push(h.dataset.dplId);
                out.push(String(h.dataset.missing));
                out.push(Object.keys(h.dataset).sort().join(','));
                h.dataset.newKey = 'v';
                out.push(h.getAttribute('data-new-key'));
                h.setAttribute('data-from-attr', 'a');
                out.push(h.dataset.fromAttr);
                delete h.dataset.dplId;
                out.push(String(h.dataset.dplId) + '/' + String(h.getAttribute('data-dpl-id')));
                out.push(String('plain' in h.dataset) + ',' + String('nope' in h.dataset));
                out.push(String(typeof document.dataset));
                var probe = document.createElement('div');
                probe.id = 'probe';
                probe.textContent = out.join('|');
                document.body.appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://dataset", html, true);
        let doc = engine.document.as_ref().unwrap();
        let probe = doc.select("#probe").unwrap().next().expect("probe must attach");
        let f: Vec<String> = probe.as_node().text_contents().split('|').map(String::from).collect();
        assert_eq!(f[0], "sha-1", "data-dpl-id reads back as dplId");
        assert_eq!(f[1], "undefined", "an absent data-* name is undefined, not empty string");
        assert_eq!(f[2], "dplId,plain", "enumeration lists exactly the data-* attributes");
        assert_eq!(f[3], "v", "a dataset write lands on the real attribute");
        assert_eq!(f[4], "a", "an attribute write is visible through dataset");
        assert_eq!(f[5], "undefined/null", "delete removes the attribute itself");
        assert_eq!(f[6], "true,false", "`in` follows the attribute list");
        assert_eq!(f[7], "undefined", "non-elements have no dataset");
    }

    /// The `HTML*Element` interface constructors exist as one prototype
    /// chain. Bundles feature-detect them by name (a missing name is a
    /// ReferenceError that kills the whole script) and branch on
    /// `instanceof`, so identity and inheritance both have to hold.
    #[test]
    fn dom_interface_constructors_form_a_chain() {
        let html = r#"<html><body>
            <script>
                var out = [];
                out.push([typeof HTMLScriptElement, typeof HTMLDialogElement,
                          typeof HTMLAnchorElement, typeof HTMLVideoElement].join(','));
                out.push(String(Object.create(HTMLAnchorElement.prototype) instanceof HTMLElement));
                out.push(String(Object.create(HTMLDivElement.prototype) instanceof Node));
                out.push(String(Object.create(HTMLVideoElement.prototype) instanceof HTMLMediaElement));
                out.push(String(Object.create(Text.prototype) instanceof CharacterData));
                out.push(String(Object.create(HTMLElement.prototype) instanceof HTMLScriptElement));
                var threw = '';
                try { new HTMLScriptElement(); } catch (e) { threw = e.name; }
                out.push(threw);
                var probe = document.createElement('div');
                probe.id = 'probe';
                probe.textContent = out.join('|');
                document.body.appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://interfaces", html, true);
        let doc = engine.document.as_ref().unwrap();
        let probe = doc.select("#probe").unwrap().next().expect("probe must attach");
        let f: Vec<String> = probe.as_node().text_contents().split('|').map(String::from).collect();
        assert_eq!(f[0], "function,function,function,function", "the family is installed whole");
        assert_eq!(f[1], "true", "HTMLAnchorElement inherits from HTMLElement");
        assert_eq!(f[2], "true", "the chain reaches Node");
        assert_eq!(f[3], "true", "<video> hangs off HTMLMediaElement");
        assert_eq!(f[4], "true", "Text inherits from CharacterData");
        assert_eq!(f[5], "false", "inheritance does not run the wrong way");
        assert_eq!(f[6], "TypeError", "the interfaces are not callable constructors");
    }

    /// Wrapped nodes carry the prototype of the interface they actually are,
    /// so `instanceof` on a REAL element — not just on a synthetic
    /// `Object.create` — answers the way a browser answers. And
    /// `document.currentScript` is the running `<script>` element, which is
    /// what webpack's chunk loader asserts on before it will boot.
    #[test]
    fn wrapped_nodes_inherit_their_interface_and_currentscript_is_real() {
        let html = r#"<html><body>
            <a id="lnk" href="/x">x</a>
            <my-widget id="custom"></my-widget>
            <blink id="weird"></blink>
            <script id="s1">
                var cs = document.currentScript;
                var a = document.getElementById('lnk');
                var out = [];
                out.push(String(a instanceof HTMLAnchorElement));
                out.push(String(a instanceof HTMLElement));
                out.push(String(a instanceof Node));
                out.push(String(a instanceof HTMLDivElement));
                out.push(String(document.getElementById('custom') instanceof HTMLElement));
                out.push(String(document.getElementById('weird') instanceof HTMLUnknownElement));
                out.push(cs ? cs.tagName : 'null');
                out.push(cs ? String(cs instanceof HTMLScriptElement) : 'null');
                out.push(cs ? cs.id : 'null');
                // Own behavior must survive the prototype wiring.
                a.classList.add('marked');
                out.push(a.getAttribute('href'));
                // Deferred contexts see null, per spec.
                var later = 'unset';
                Promise.resolve().then(function () {
                    later = String(document.currentScript);
                    var p = document.createElement('p');
                    p.id = 'later';
                    p.textContent = later;
                    document.body.appendChild(p);
                });
                var probe = document.createElement('div');
                probe.id = 'probe';
                probe.textContent = out.join('|');
                document.body.appendChild(probe);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://protos", html, true);
        let doc = engine.document.as_ref().unwrap();
        let probe = doc.select("#probe").unwrap().next().expect("probe must attach");
        let f: Vec<String> = probe.as_node().text_contents().split('|').map(String::from).collect();
        assert_eq!(f[0], "true", "<a> is an HTMLAnchorElement");
        assert_eq!(f[1], "true", "and an HTMLElement");
        assert_eq!(f[2], "true", "and a Node");
        assert_eq!(f[3], "false", "but not some other element's interface");
        assert_eq!(f[4], "true", "custom elements are HTMLElement");
        assert_eq!(f[5], "true", "unrecognized tags are HTMLUnknownElement");
        assert_eq!(f[6], "SCRIPT", "currentScript is the running script element");
        assert_eq!(f[7], "true", "and it presents as HTMLScriptElement");
        assert_eq!(f[8], "s1", "and it is THAT script, identified by its own attrs");
        assert_eq!(f[9], "/x", "own properties still win over the prototype");
        let later = doc.select("#later").unwrap().next().expect("microtask must run");
        // HTML "execute the script element" keeps currentScript set through "clean up after running
        // script", so microtasks the script queued see the element — what Chromium does too
        // (script oracle page 16).
        assert_eq!(
            later.as_node().text_contents(),
            "[object HTMLScriptElement]",
            "a microtask queued by a running script runs in its checkpoint, currentScript still set"
        );
    }

    /// The fetched-page path: external sources the shell pre-fetched are found by URL when the parser
    /// reaches their `<script src>`, inline scripts run from the document's own text, and each sees
    /// its OWN element as `document.currentScript` (a non-JS script between them takes no turn).
    #[test]
    fn prefetched_external_scripts_run_in_document_order_with_their_own_current_script() {
        let html = r#"<html><body>
            <script id="s0" src="/a.js"></script>
            <script type="application/ld+json">{"not":"js"}</script>
            <script id="s1">window.__seen.push(document.currentScript.id);</script>
            <script id="s2" src="https://cdn.example.com/b.js"></script>
        </body></html>"#;
        let (slots, external) = crate::net::collect_scripts("https://example.com/", html);
        assert_eq!(external.len(), 2);
        assert_eq!(slots.iter().map(|s| s.ordinal).collect::<Vec<_>>(), vec![0, 2, 3]);
        let page = crate::net::Page {
            base_url: "https://example.com/".to_string(),
            html: html.to_string(),
            scripts: vec![
                "window.__seen = [document.currentScript.id];".to_string(),
                "window.__seen.push(document.currentScript.id);".to_string(),
            ],
            script_urls: vec!["https://example.com/a.js".to_string(), "https://cdn.example.com/b.js".to_string()],
            script_nodes: vec![0, 3],
            ..Default::default()
        };
        let mut engine = crate::AetherEngine::new();
        engine.load_page(page, true);
        let js = engine.js_engine.as_mut().unwrap();
        let seen = js.eval_string("window.__seen.join(',')").unwrap();
        assert_eq!(seen, "s0,s1,s2", "each script saw its OWN element, in document order");
    }

    /// Every decode path must hand the renderer straight-alpha RGBA in the
    /// same channel order, so one pure-blue source paints one pure-blue box
    /// whether it arrived as PNG, JPEG, or SVG. (The surface is BGRA — see
    /// render::put_px — so blue lands in byte 0.)
    #[test]
    fn test_image_decode_channel_order_matches_across_formats() {
        use base64::Engine as _;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let blue = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]));

        let mut png = std::io::Cursor::new(Vec::new());
        blue.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut jpg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(blue.clone())
            .to_rgb8()
            .write_to(&mut jpg, image::ImageFormat::Jpeg)
            .unwrap();
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='8' height='8'>\
                    <rect width='8' height='8' fill='#0000ff'/></svg>";

        let html = format!(
            r#"<html><body style="margin:0">
               <img id="p" style="width:20px;height:20px" src="data:image/png;base64,{}">
               <img id="j" style="width:20px;height:20px" src="data:image/jpeg;base64,{}">
               <img id="s" style="width:20px;height:20px" src="data:image/svg+xml;base64,{}">
               </body></html>"#,
            b64(png.get_ref()),
            b64(jpg.get_ref()),
            b64(svg),
        );
        let mut tree = layout::compute_layout(&dom::parse_html(&html));
        css::apply_css(&mut tree, "");
        let mut surface = vec![255u8; 200 * 200 * 4];
        render::render_frame(&tree, &mut surface, 200, 200, 0.0, 0.0, &[(0, 0, 200, 200)]);

        // Blue must dominate: byte 0 (B) high, bytes 1/2 (G/R) low. A
        // channel swap or a premultiply mismatch on any single path shows
        // up as an orange/red box and fails here.
        let blueish = surface
            .chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
            .count();
        let redish = surface
            .chunks_exact(4)
            .filter(|p| p[2] > 200 && p[1] < 60 && p[0] < 60)
            .count();
        assert!(blueish >= 3 * 20 * 20 - 40, "three 20x20 blue boxes expected, got {blueish} px");
        assert_eq!(redish, 0, "no decode path may swap R and B ({redish} red px)");
    }

    /// Painting is a pure function of the document translated by the scroll
    /// offset: the frame at scroll S must equal the frame at scroll 0 shifted
    /// up by S. Clamping a box ORIGIN into the viewport (rather than only its
    /// painted span) pins scrolled-off images and borders to the viewport
    /// edge, and the shell's shift-and-repaint-the-strip scroll then smears
    /// copies of them down the page.
    #[test]
    fn test_paint_translates_with_scroll() {
        use base64::Engine as _;
        let blue = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        blue.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let src = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png.get_ref())
        );
        let html = format!(
            r#"<html><body style="margin:0">
               <div style="height:60px"></div>
               <img style="width:120px;height:90px" src="{src}">
               <div style="height:400px;border:3px solid rgb(0,128,0)">boxed</div>
               <img style="width:120px;height:90px" src="{src}">
               <div style="height:900px"></div>
               </body></html>"#
        );
        let (w, h, s) = (300u32, 400u32, 150u32);
        let mut tree = layout::compute_layout(&dom::parse_html(&html));
        css::apply_css(&mut tree, "");
        let mut base = vec![255u8; (w * (h + s) * 4) as usize];
        render::render_frame(&tree, &mut base, w, h + s, 0.0, 0.0, &[(0, 0, w, h + s)]);
        let mut scrolled = vec![255u8; (w * h * 4) as usize];
        render::render_frame(&tree, &mut scrolled, w, h, 0.0, s as f64, &[(0, 0, w, h)]);

        let row = (w * 4) as usize;
        for y in 0..h as usize {
            let a = &base[(y + s as usize) * row..(y + s as usize) * row + row];
            let b = &scrolled[y * row..y * row + row];
            assert_eq!(a, b, "row {y} at scroll {s} does not match the unscrolled frame");
        }
    }

    #[test]
    fn test_select_options_produce_no_boxes() {
        let html = r#"<html><body><select name="lang">
            <optgroup label="Group"><option value="de">Deutsch</option></optgroup>
            <option value="en" selected>English</option>
        </select></body></html>"#;
        let document = dom::parse_html(html);
        let tree = layout::compute_layout(&document);

        for node in tree.node_map.values() {
            if let Some(el) = node.as_element() {
                let tag = el.name.local.as_ref();
                assert!(
                    tag != "option" && tag != "optgroup",
                    "<{tag}> must produce no box — its text belongs in the popup, not the page"
                );
            }
            if let Some(text) = node.as_text() {
                let t = text.borrow();
                assert!(
                    !t.contains("Deutsch") && !t.contains("English"),
                    "option text leaked into the box tree: {t:?}"
                );
            }
        }

        // The selected option reaches the control paint/submit path — read from the options, never
        // written into the document (AETHERJS: layout used to insert `value`/`data-aether-label`
        // attributes, which then showed in the serialized DOM and in script).
        let select = document.select_first("select").unwrap();
        assert_eq!(
            layout::select_selected_option(select.as_node()),
            Some(("en".to_string(), "English".to_string())),
            "select submits the selected option's value and paints its text"
        );
        let attrs = select.attributes.borrow();
        assert_eq!(attrs.get("value"), None, "layout does not mutate the DOM");
        assert_eq!(attrs.get("data-aether-label"), None, "layout does not mutate the DOM");
    }

    // -----------------------------------------------------------------------
    // Process survival: no page may kill the browser
    // -----------------------------------------------------------------------

    /// A page that overflows the call stack inside a promise reaction: js_core throws a RangeError
    /// (catchable, as in every browser), the reaction's derived promise rejects, the chain stops, and
    /// the engine is neither poisoned nor the process ended. The VM's call-depth ceiling is squeezed to
    /// make the trip cheap — a real page reaches the same path by recursing deeper.
    #[test]
    fn test_stack_overflow_in_promise_reaction_rejects_the_chain() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">kept</div></body></html>"#,
            &[],
            true,
        );
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.vm.max_depth = 64;
            js.execute(
                "window.__settled = 0; window.__err = '';
                 Promise.resolve().then(function () {
                     function f(n) { return n <= 0 ? 0 : f(n - 1) + 1; }
                     return f(100000);
                 }).then(function () { window.__settled = 1; },
                         function (e) { window.__err = e.name; });",
            )
            .expect("arming the reaction must run clean");
            js.vm.max_depth = 1800;
        }
        engine.tick();
        assert!(!crate::js::engine_poisoned(), "a stack overflow is an exception, not an engine panic");
        let js = engine.js_engine.as_mut().unwrap();
        assert_eq!(js.eval_number("window.__settled").unwrap(), 0.0, "the overflow must stop the chain");
        assert_eq!(js.eval_string("window.__err").unwrap(), "RangeError", "the rejection carries a RangeError");
        let document = engine.document.as_ref().expect("document survives");
        assert!(document.select_first("#x").is_ok(), "the DOM is intact");
    }

    /// A stack overflow inside a timer callback is an ordinary, catchable RangeError (the page's own
    /// catch sees it), and the rest of the timer generation still runs.
    #[test]
    fn test_stack_overflow_in_timer_callback_is_catchable() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div></body></html>"#,
            &[],
            true,
        );
        crate::event_loop::freeze_clock();
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute(
                "window.__caught = 0; window.__after = 0;
                 setTimeout(function () {
                     try {
                         (function f(n) { return n <= 0 ? 0 : f(n - 1) + 1; })(100000);
                     } catch (e) { window.__caught = e instanceof RangeError ? 1 : 2; }
                 }, 10);
                 setTimeout(function () { window.__after = 1; }, 10);",
            )
            .expect("arming must run clean");
        }
        crate::event_loop::advance_clock(10);
        engine.tick();
        assert!(!crate::js::engine_poisoned(), "no engine panic");
        assert_eq!(js_num(&mut engine, "window.__caught"), 1.0, "the page's own catch sees the RangeError");
        assert_eq!(js_num(&mut engine, "window.__after"), 1.0, "the generation continues");
    }

    /// A runaway task (`while (true) {}`) is terminated by the per-task instruction budget: reported in
    /// the ledger, uncatchable by the page, and the next task still runs on a live engine.
    #[test]
    fn test_runaway_task_is_terminated_by_the_budget() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled("https://example.com/", "<html><body>a</body></html>", &[], true);
        crate::event_loop::freeze_clock();
        crate::ledger::reset();
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute(
                "window.__caught = 0; window.__after = 0;
                 setTimeout(function () { try { while (true) {} } catch (e) { window.__caught = 1; } }, 5);
                 setTimeout(function () { window.__after = 1; }, 5);",
            )
            .unwrap();
        }
        crate::event_loop::advance_clock(5);
        crate::js::set_task_budget(2_000_000);
        engine.tick();
        crate::js::set_task_budget(crate::js::TASK_BUDGET);
        assert!(!crate::js::engine_poisoned());
        assert_eq!(js_num(&mut engine, "window.__caught"), 0.0, "termination is uncatchable");
        assert_eq!(js_num(&mut engine, "window.__after"), 1.0, "the next task runs");
        assert!(
            crate::ledger::snapshot().contains(crate::ledger::ApiCategory::Js, "js-task-terminated:timer-callback"),
            "the termination is ledgered"
        );
    }

    /// DOM wrapper methods invoked with a non-object `this` — `appendChild
    /// .call(null, x)`, a method pulled off its receiver — used to unwrap a
    /// `None` and abort the process. Observed live on google search.
    #[test]
    fn test_dom_methods_with_detached_this_do_not_panic() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled(
            "https://example.com/",
            r#"<html><body><div id="x">a</div></body></html>"#,
            &[],
            true,
        );
        let js = engine.js_engine.as_mut().unwrap();
        js.execute(
            "var d = document.getElementById('x');
             var f = d.appendChild;
             window.__r = 0;
             try { f.call(null, document.createElement('span')); } catch (e) { window.__r = 1; }
             try { d.setAttribute.call(undefined, 'a', 'b'); } catch (e) { window.__r = 1; }
             try { d.removeChild.call(7, d); } catch (e) { window.__r = 1; }",
        )
        .expect("a detached receiver must not take the process down");
        assert!(
            !crate::js::engine_poisoned(),
            "a detached receiver is an ordinary no-op, not an engine panic"
        );
    }

    /// `console.log` runs the page's own `toString`. One that throws used to
    /// unwrap into a process abort.
    #[test]
    fn test_console_log_with_throwing_tostring_does_not_panic() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled("https://example.com/", "<html><body>a</body></html>", &[], true);
        let js = engine.js_engine.as_mut().unwrap();
        js.execute(
            "window.__r = 0;
             try { console.log({ toString: function () { throw new Error('no'); } }); }
             catch (e) { window.__r = 1; }",
        )
        .expect("a throwing toString must surface as a JS error, not a panic");
        assert!(!crate::js::engine_poisoned(), "no engine panic");
    }

    /// The rAF drain is bounded and driven from Rust: each pass runs the batch
    /// registered when it began, a callback that throws is ledgered and the
    /// pass continues, and a self-re-registering render loop is cut off.
    #[test]
    fn test_raf_drain_is_bounded_and_isolates_throwing_callbacks() {
        let mut engine = crate::AetherEngine::new();
        engine.load_html_styled("https://example.com/", "<html><body>a</body></html>", &[], true);
        {
            let js = engine.js_engine.as_mut().unwrap();
            js.execute(
                "window.__n = 0; window.__loops = 0;
                 requestAnimationFrame(function () { window.__n++; throw new Error('bad'); });
                 requestAnimationFrame(function () { window.__n++; });
                 (function loop() {
                     requestAnimationFrame(function () { window.__loops++; loop(); });
                 })();",
            )
            .expect("registration must run clean");
            js.drain_raf();
        }
        assert_eq!(
            js_num(&mut engine, "window.__n"),
            2.0,
            "a throwing callback must not stop the rest of its batch"
        );
        assert_eq!(
            js_num(&mut engine, "window.__loops"),
            8.0,
            "a self-re-registering render loop gets exactly the bounded pass count"
        );
    }

    #[test]
    fn test_ledger_dump_is_one_record_per_line() {
        let mut ledger = crate::ledger::ApiCoverageLedger::new();
        let multiline = "js-error:TypeError: x is not a function\n    at foo (eval:1:1)\r\n    at bar | baz (eval:2:2)";
        ledger.record(crate::ledger::ApiCategory::Js, multiline);
        ledger.record(crate::ledger::ApiCategory::Js, multiline);
        ledger.record(crate::ledger::ApiCategory::Dom, &"x".repeat(500));

        let path = std::env::temp_dir().join("aether_ledger_multiline_test.txt");
        ledger.dump_to_file(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let records: Vec<&str> = text.lines().skip(4).filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(records.len(), 2, "one line per record, got: {records:#?}");
        for line in &records {
            let cols: Vec<&str> = line.split('|').collect();
            assert_eq!(cols.len(), 3, "record must parse as category|name|count: {line:?}");
            assert!(
                cols[2].trim().parse::<usize>().is_ok(),
                "count column must be a number: {line:?}"
            );
            assert!(cols[1].trim().chars().count() <= 201, "name must be capped: {line:?}");
        }
        let js = records.iter().find(|l| l.starts_with("JS")).unwrap();
        assert!(js.trim_end().ends_with(" 2"), "both records merged into one count: {js:?}");
    }

    /// Modern selector syntax servo's parser rejects must not take the
    /// whole rule with it. `:is()`/`:where()` lower by distribution, and
    /// `:focus-visible` joins `:hover` as a state a static render never
    /// enters — dropped, not fatal to its siblings in the list.
    #[test]
    fn test_modern_selector_syntax_lowered() {
        let html = r#"<html><body>
            <nav id="n" class="hdr"><a id="a" class="lnk" href="/">x</a></nav>
        </body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            nav :is(.lnk, .btn) { font-size: 20px; }
            :where(.hdr) { color: rgb(1, 2, 3); }
            .lnk:focus-visible, .hdr:has(> form) { color: rgb(9, 9, 9); }
            .lnk:not(:focus-visible) { font-weight: bold; }
        "#);
        let paint = |id: &str| {
            layout_tree.node_map.iter()
                .find(|(_, n)| n.as_element().map_or(false, |e| {
                    e.attributes.borrow().get("id") == Some(id)
                }))
                .map(|(nid, _)| layout_tree.paint_map.get(nid).cloned().unwrap_or_default())
                .unwrap()
        };
        assert_eq!(paint("a").font_size, Some(20.0), ":is() must distribute");
        assert_eq!(paint("n").color, Some((1, 2, 3)), ":where() must distribute");
        assert_eq!(paint("a").color, None, ":focus-visible/:has must not match");
        assert_eq!(paint("a").weight, Some(crate::layout::FontWeight::Abs(700)), ":not(:focus-visible) is a tautology");
    }

    /// AETHERSEE helper: the layout box of the element with this id.
    fn box_by_id(tree: &layout::LayoutTree, id: &str) -> taffy::Layout {
        for (node_id, dom_node) in &tree.node_map {
            if let Some(el) = dom_node.as_element() {
                if el.attributes.borrow().get("id") == Some(id) {
                    return *tree.taffy.layout(*node_id).unwrap();
                }
            }
        }
        panic!("no element #{id}");
    }

    #[test]
    fn aethersee_display_flex_is_a_row_with_flex_properties() {
        // AETHERSEE fix 1 (corpus 02/03/09): display:flex was kept as
        // Aether's internal column, and flex/gap/flex-wrap were ignored.
        let html = r#"<!DOCTYPE html><html><body style="margin:0">
            <div id="row"><div id="side">s</div><div id="main">m</div></div>
            <div id="wrap"><div id="c1">a</div><div id="c2">b</div><div id="c3">c</div></div>
        </body></html>"#;
        let mut tree = layout::compute_layout_sized(&dom::parse_html(html), 800.0, 600.0);
        css::apply_css(&mut tree, r#"
            #row { display: flex; gap: 20px; }
            #side { flex: 0 0 200px; }
            #main { flex: 1; }
            #wrap { display: flex; flex-wrap: wrap; gap: 10px; }
            #wrap div { width: 300px; height: 50px; }
        "#);
        let (side, main) = (box_by_id(&tree, "side"), box_by_id(&tree, "main"));
        assert_eq!(side.size.width, 200.0, "flex-basis 200px, no grow/shrink");
        assert_eq!(side.location.y, main.location.y, "flex items share one row");
        assert_eq!(main.location.x, 220.0, "column gap between items");
        assert_eq!(main.size.width, 580.0, "flex:1 takes the remaining space");
        let (c1, c2, c3) = (box_by_id(&tree, "c1"), box_by_id(&tree, "c2"), box_by_id(&tree, "c3"));
        assert_eq!((c1.location.y, c2.location.y), (0.0, 0.0), "two 300px items fit a row");
        assert_eq!(c2.location.x, 310.0);
        assert_eq!(c3.location.y, 60.0, "third wraps below with the row gap");
    }

    #[test]
    fn aethersee_flex_shorthand_grammar() {
        use taffy::style::Dimension;
        let f = css::parse_flex_shorthand;
        assert_eq!(f("1"), Some((1.0, 1.0, Dimension::percent(0.0))));
        assert_eq!(f("0 0 200px"), Some((0.0, 0.0, Dimension::length(200.0))));
        assert_eq!(f("none"), Some((0.0, 0.0, Dimension::auto())));
        assert_eq!(f("auto"), Some((1.0, 1.0, Dimension::auto())));
        assert_eq!(f("2 30%"), Some((2.0, 1.0, Dimension::percent(0.3))));
        assert_eq!(f("120px"), Some((1.0, 1.0, Dimension::length(120.0))));
    }

    #[test]
    fn aethersee_sole_text_run_wraps_inside_its_box() {
        // A paragraph narrower than the viewport: its only text run must
        // wrap at the paragraph's width, not at the viewport's.
        let html = r#"<!DOCTYPE html><html><body style="margin:0"><p id="p" style="width:200px;margin:0">
            one two three four five six seven eight nine ten eleven twelve</p></body></html>"#;
        let mut tree = layout::compute_layout_sized(&dom::parse_html(html), 800.0, 600.0);
        css::apply_css(&mut tree, "");
        for (node_id, dom_node) in &tree.node_map {
            if dom_node.as_text().is_some_and(|t| t.borrow().contains("twelve")) {
                let l = tree.taffy.layout(*node_id).unwrap();
                assert!(l.size.width <= 200.0, "text run {} wide in a 200px box", l.size.width);
                assert!(l.size.height > 30.0, "it wraps onto several lines");
                return;
            }
        }
        panic!("text run not found");
    }

    #[test]
    fn aethersee_body_background_fills_the_canvas() {
        // AETHERSEE fix 2 (corpus 15-landing): a body background painted
        // only the body box; below the content the canvas stayed white.
        let html = r#"<!DOCTYPE html><html><body style="margin:0"><p>short</p></body></html>"#;
        let mut tree = layout::compute_layout_sized(&dom::parse_html(html), 100.0, 100.0);
        css::apply_css(&mut tree, "body { background: #102030; }");
        assert_eq!(render::canvas_background(&tree), Some((0x10, 0x20, 0x30)));
        let mut surface = vec![0u8; 100 * 100 * 4];
        render::render_frame(&tree, &mut surface, 100, 100, 0.0, 0.0, &[(0, 0, 100, 100)]);
        let px = |x: usize, y: usize| surface[(y * 100 + x) * 4..(y * 100 + x) * 4 + 3].to_vec();
        assert_eq!(px(50, 95), vec![0x30, 0x20, 0x10], "canvas below the content is body's colour (BGRA)");
        // The root's own background wins over body's.
        css::apply_css(&mut tree, "html { background: #ff0000; } body { background: #102030; }");
        assert_eq!(render::canvas_background(&tree), Some((255, 0, 0)));
    }

    #[test]
    fn aethersee_text_is_measured_with_the_face_it_is_painted_with() {
        // AETHERSEE fix 3 (corpus 01-blog-article): the measurer always used
        // the family's REGULAR face, the painter the bold face, so every bold
        // heading painted wider than its box and overlapped the next block.
        let html = r#"<!DOCTYPE html><html><body>
            <div><span id="r">Three stages of a browser</span></div>
            <div><b id="b">Three stages of a browser</b></div>
            <div><span id="u" style="text-transform:uppercase">Three stages of a browser</span></div>
        </body></html>"#;
        let mut tree = layout::compute_layout_sized(&dom::parse_html(html), 800.0, 600.0);
        css::apply_css(&mut tree, "");
        let (r, b, u) = (box_by_id(&tree, "r"), box_by_id(&tree, "b"), box_by_id(&tree, "u"));
        assert!(b.size.width > r.size.width, "bold run measured with the bold face ({} vs {})", b.size.width, r.size.width);
        assert!(u.size.width > r.size.width, "uppercase run measured after its transform");
        // One resolver: every family x weight x style the painter can pick is its own face.
        let ids: std::collections::HashSet<u32> = (0..3u16)
            .flat_map(|f| [(f, 400, false), (f, 700, false), (f, 400, true), (f, 700, true)])
            .filter_map(|(f, w, i)| crate::fonts::face(&crate::fonts::FontSel::new(f, w, i)).map(|x| x.id))
            .collect();
        assert_eq!(ids.len(), 12);
    }

    /// Absolute (page) rect of the first element with `id`, after a cascade.
    fn rect_by_id(tree: &layout::LayoutTree, want: &str) -> Option<(f32, f32, f32, f32)> {
        fn walk(t: &layout::LayoutTree, n: taffy::NodeId, x: f32, y: f32, want: &str) -> Option<(f32, f32, f32, f32)> {
            let l = t.taffy.layout(n).ok()?;
            let (ax, ay) = (x + l.location.x, y + l.location.y);
            if let Some(el) = t.node_map.get(&n).and_then(|d| d.as_element()) {
                if el.attributes.borrow().get("id") == Some(want) {
                    return Some((ax, ay, l.size.width, l.size.height));
                }
            }
            for k in t.taffy.children(n).unwrap_or_default() {
                if let Some(r) = walk(t, k, ax, ay, want) {
                    return Some(r);
                }
            }
            None
        }
        walk(tree, tree.root_node, 0.0, 0.0, want)
    }

    fn laid_out(html: &str, css: &str) -> layout::LayoutTree {
        let mut tree = layout::compute_layout(&dom::parse_html(html));
        css::apply_css(&mut tree, css);
        tree
    }

    /// CSS 2.2 §8.3.1 known answers: adjoining sibling margins collapse to
    /// the larger; a negative joins as max-positive + min-negative; a first
    /// child's top margin escapes a parent with no border/padding (and the
    /// body), but not one with padding; an empty block collapses through.
    #[test]
    fn test_margin_collapsing_kat() {
        let css = "body{margin:0} div{height:10px}";
        // Siblings: 16 and 24 collapse to 24 (not 40).
        let t = laid_out(r#"<html><body><div id=a style="margin-bottom:16px"></div><div id=b style="margin-top:24px"></div></body></html>"#, css);
        let (a, b) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!(b.1 - (a.1 + a.3), 24.0, "sibling margins collapse to the max");
        // Positive and negative: 20 + (-8) = 12.
        let t = laid_out(r#"<html><body><div id=a style="margin-bottom:20px"></div><div id=b style="margin-top:-8px"></div></body></html>"#, css);
        let (a, b) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!(b.1 - (a.1 + a.3), 12.0, "max positive + min negative");
        // Parent/first child: the child's 30px escapes the wrapper (and body).
        let t = laid_out(r#"<html><body><section id=w style="display:block"><p id=c style="margin:30px 0;height:10px"></p></section></body></html>"#, "body{margin:0}");
        let (w, c) = (rect_by_id(&t, "w").unwrap(), rect_by_id(&t, "c").unwrap());
        assert_eq!(c.1, 30.0, "first child's margin collapses through");
        assert_eq!(w.1, 30.0, "the wrapper starts where the collapsed margin ends");
        // Padding separates: the child's margin stays inside.
        let t = laid_out(r#"<html><body><section id=w style="padding-top:1px"><p id=c style="margin:30px 0;height:10px"></p></section></body></html>"#, "body{margin:0}");
        let (w, c) = (rect_by_id(&t, "w").unwrap(), rect_by_id(&t, "c").unwrap());
        assert_eq!(w.1, 0.0);
        assert_eq!(c.1, 31.0, "padding blocks the collapse");
        // Empty block between siblings: 10, (12/14 empty), 8 → one 14 gap.
        let t = laid_out(r#"<html><body><div id=a style="margin-bottom:10px"></div><section style="margin:12px 0 14px"></section><div id=b style="margin-top:8px"></div></body></html>"#, css);
        let (a, b) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!(b.1 - (a.1 + a.3), 14.0, "an empty block collapses through");
        // The specified margins survive the layout run (restore).
        let id = t.node_map.iter().find(|(_, n)| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some("b"))).map(|(i, _)| *i).unwrap();
        assert_eq!(t.taffy.style(id).unwrap().margin.top, taffy::style::LengthPercentageAuto::length(8.0));
    }

    /// html.css UA margins: h1 0.67em of 32px, p 1em, a margin longhand
    /// replaces only its own side.
    #[test]
    fn test_ua_margins_and_longhands() {
        let t = laid_out(r#"<html><body><div id=top style="height:1px"></div><h1 id=h>T</h1><p id=p>x</p></body></html>"#, "body{margin:0} h1{margin-bottom:4px}");
        let (top, h, p) = (rect_by_id(&t, "top").unwrap(), rect_by_id(&t, "h").unwrap(), rect_by_id(&t, "p").unwrap());
        assert!((h.1 - (top.1 + top.3) - 21.44).abs() <= 1.0, "h1 keeps its UA top margin: {:?}", h);
        assert!((p.1 - (h.1 + h.3) - 16.0).abs() <= 1.0, "max(4, 16) between h1 and p: {:?} {:?}", h, p);
    }

    /// CSS 2.2 §10.3.3: an auto-width block fills its container minus its
    /// margins, border and padding (content-box); `margin: 0 auto` under a
    /// max-width centres the clamped box.
    #[test]
    fn test_block_width_kat() {
        let t = laid_out(
            r#"<html><body><div style="width:400px"><div id=a style="margin:0 10px;padding:0 12px;border:2px solid red">x</div><div id=b style="max-width:200px;margin:0 auto;padding:0 10px">y</div></div></body></html>"#,
            "body{margin:0}",
        );
        let (a, b) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!((a.0, a.2), (10.0, 380.0), "fills 400 - 2x10 margin, border-box");
        assert_eq!((b.0, b.2), (90.0, 220.0), "content 200 + padding 20, centred {b:?}");
    }

    /// CSS 2.2 §10.8: `line-height: normal` is the rounded ascent + descent
    /// + gap (Liberation Sans/Arial 16px = 18, not 18.4), and the baseline
    /// sits half the leading below the line top.
    #[test]
    fn test_line_metrics_kat() {
        let Some(f) = crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::SANS, 400, false)) else { return };
        let (a, d, g) = crate::fonts::line_metrics(f, 16.0);
        assert_eq!(crate::fonts::line_height(&f, 16.0, 0.0), a + d + g);
        assert_eq!(crate::fonts::baseline_offset(&f, 16.0, 0.0), (g / 2.0).floor() + a);
        assert_eq!(crate::fonts::baseline_offset(&f, 16.0, 2.0), ((32.0 - a - d) / 2.0).floor() + a);
        let sp = crate::fonts::space_advance(&f, 16.0);
        assert!(sp > 3.0 && sp < 5.0, "space advance {sp}");
    }

    /// Chromium control metrics: a text field is one 13.333px line + 1px/2px
    /// padding + 2px border (21px tall at UA padding; author padding and
    /// border-box work on top), a checkbox 13x13.
    #[test]
    fn test_form_control_metrics_kat() {
        let t = laid_out(
            r#"<html><body><input id=f><input id=g style="box-sizing:border-box;padding:6px;width:200px"><input id=c type=checkbox></body></html>"#,
            "body{margin:0}",
        );
        let (f, g, c) = (rect_by_id(&t, "f").unwrap(), rect_by_id(&t, "g").unwrap(), rect_by_id(&t, "c").unwrap());
        let lh = crate::fonts::face(&crate::fonts::FontSel::new(crate::layout::control_family(), 400, false))
            .map(|x| crate::fonts::line_height(x, crate::layout::CONTROL_FONT_PX, 0.0)) // SR1023: the const
            .unwrap_or(15.0);
        assert_eq!(f.3, lh + 2.0 + 4.0, "field: line + padding + border");
        assert_eq!((g.2, g.3), (200.0, lh + 12.0 + 4.0), "border-box width, padding 6");
        assert_eq!((c.2, c.3), (13.0, 13.0));
    }

    /// CSS 2.2 §9.2.1.1: an inline tag made `display: block` splits its
    /// parent's line into block boxes (label above input, not beside it).
    #[test]
    fn test_display_block_on_inline_tag() {
        let t = laid_out(
            r#"<html><body><form><label id=l>Name</label><input id=i> <label id=m>Mail</label><input id=j></form></body></html>"#,
            "body{margin:0} label{display:block} input{width:100px}",
        );
        let (l, i, m) = (rect_by_id(&t, "l").unwrap(), rect_by_id(&t, "i").unwrap(), rect_by_id(&t, "m").unwrap());
        assert_eq!(l.0, 0.0);
        assert!(i.1 >= l.1 + l.3, "input below its block label: {l:?} {i:?}");
        assert!(m.1 >= i.1 + i.3 && m.0 == 0.0, "next label on its own line: {m:?}");
    }

    /// html.css heading sizes are em of the parent: h2 in a 14px body is 21px.
    #[test]
    fn test_heading_em_sizes() {
        assert_eq!(layout::default_font_size("h2", 14.0), 21.0);
        assert_eq!(layout::default_font_size("h1", 16.0), 32.0);
    }

    /// css-values-4: clamp()/min() with % resolve against the containing
    /// block after layout; math in a box shorthand is one component;
    /// clamp() font sizes.
    #[test]
    fn test_css_math_kat() {
        assert_eq!(css::split_top_level("calc(0.5rem + 1vw) 4px"), vec!["calc(0.5rem + 1vw)", "4px"]);
        assert_eq!(css::parse_font_size("clamp(10px, 50px, 20px)"), Some(20.0));
        let t = laid_out(
            r#"<html><body><div style="width:500px"><div id=a style="width:min(300px, 80%);height:1px"></div><div id=b style="width:max(10px, 50%);height:1px;padding:calc(2px + 3px)"></div></div></body></html>"#,
            "body{margin:0}",
        );
        let (a, b) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!(a.2, 300.0);
        assert_eq!(b.2, 250.0 + 10.0, "50% of 500 + padding 5+5");
    }

    /// CSS 2.2 §17.5.2.2 / §17.6: columns line up across rows, a 100% wide
    /// table widens its columns in proportion, collapsed borders overlap.
    #[test]
    fn test_table_layout_kat() {
        let t = laid_out(
            r#"<html><body><table style="width:400px;border-collapse:collapse"><tr><td id=a style="border:1px solid red">x</td><td id=b style="border:1px solid red">yyyyyyyy</td></tr><tr><td id=c style="border:1px solid red">zzzz zzzz</td><td id=d style="border:1px solid red">1</td></tr></table></body></html>"#,
            "body{margin:0}",
        );
        let (a, b, c, d) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap(), rect_by_id(&t, "c").unwrap(), rect_by_id(&t, "d").unwrap());
        assert_eq!(a.2, c.2, "column 1 cells share a width");
        assert_eq!(b.0, d.0, "column 2 starts at one x");
        assert_eq!(b.0, a.0 + a.2 - 1.0, "collapsed: the shared border overlaps by 1px");
        assert!((d.0 + d.2 - a.0 - 400.0).abs() <= 1.0, "the columns fill the 400px table: {a:?} {d:?}");
        assert_eq!(c.1, a.1 + a.3 - 1.0, "rows overlap by the shared border");
    }

    /// vertical-align: super grows its line box by Blink's shift
    /// (parent/3 + 1 = 6.33px at 16px, less the smaller sup glyph box).
    #[test]
    fn test_sup_grows_line_box() {
        let a = laid_out(r#"<html><body><p id=p>E=mc2 x</p></body></html>"#, "body{margin:0} p{margin:0}");
        let b = laid_out(r#"<html><body><p id=p>E=mc<sup>2</sup> x</p></body></html>"#, "body{margin:0} p{margin:0}");
        let (pa, pb) = (rect_by_id(&a, "p").unwrap(), rect_by_id(&b, "p").unwrap());
        assert!(pb.3 >= pa.3 + 2.0, "line box grows: {pa:?} -> {pb:?}");
    }

    fn node_by_id(t: &layout::LayoutTree, want: &str) -> Option<taffy::NodeId> {
        t.node_map.iter().find_map(|(id, n)| {
            n.as_element().filter(|e| e.attributes.borrow().get("id") == Some(want)).map(|_| *id)
        })
    }

    /// CSS 2.2 §9.4.2 / §10.8 / css-break-3 slice known answers on a
    /// monospace face (every glyph the same advance): a span that wraps is
    /// one fragment per line (inline-start edge on the first, inline-end on
    /// the last); the text after it continues on its last line; each line
    /// box is `line-height` tall; text-align centres each line; a taller
    /// inline-block grows its line around the shared baseline; a float
    /// shortens the line boxes beside it.
    #[test]
    fn test_inline_formatting_context_kat() {
        use crate::layout::inline::Frag;
        if crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::MONO, 400, false)).is_none() {
            return;
        }
        let css = "body{margin:0} p{margin:0;font:10px monospace;line-height:2;width:100px}";
        let t = laid_out(r#"<html><body><p id=p>aaaa <span id=s style="padding:0 3px">bb cc dd ee ff</span> gg</p></body></html>"#, css);
        let (p, sp) = (node_by_id(&t, "p").unwrap(), node_by_id(&t, "s").unwrap());
        let il = &t.inline[&p];
        let boxes: Vec<_> = il.frags.iter().filter_map(|f| match f { Frag::Box { node, x, w, first, last, .. } if *node == sp => Some((*x, *w, *first, *last)), _ => None }).collect();
        assert_eq!(boxes.len(), 2, "the span is split into two line fragments: {boxes:?}");
        assert!(boxes[0].2 && !boxes[0].3 && !boxes[1].2 && boxes[1].3, "slice: start edge first, end edge last");
        assert_eq!(boxes[1].0, 0.0, "the continuation fragment starts at the line start");
        assert_eq!(il.lines.len(), 2);
        assert!(il.lines.iter().all(|l| l.1 == 20.0), "line-height: 2 x 10px: {:?}", il.lines);
        assert_eq!(rect_by_id(&t, "p").unwrap().3, 40.0, "the paragraph is two line boxes tall");
        let gg = il.frags.iter().find_map(|f| match f { Frag::Text { text, x, baseline, .. } if text.contains("gg") => Some((*x, *baseline)), _ => None }).unwrap();
        assert_eq!(gg.1, il.lines[1].2, "text after the span continues on the span's last line");
        assert!(gg.0 >= boxes[1].0 + boxes[1].1 - 0.5, "after the span's end edge: {gg:?} {boxes:?}");

        // text-align: center — the one line's content is centred.
        let t = laid_out(r#"<html><body><p id=p style="text-align:center">abcd</p></body></html>"#, css);
        let p = node_by_id(&t, "p").unwrap();
        let (x, w) = t.inline[&p].frags.iter().find_map(|f| match f { Frag::Text { x, width, .. } => Some((*x, *width)), _ => None }).unwrap();
        assert!((x - (100.0 - w) / 2.0).abs() <= 0.5, "centred (to the whole pixel): x {x} w {w}");

        // A 50px inline-block on the baseline: the line grows to hold it.
        let t = laid_out(r#"<html><body><p id=p>ab <span style="display:inline-block;width:10px;height:50px"></span> cd</p></body></html>"#, css);
        let p = node_by_id(&t, "p").unwrap();
        let l = t.inline[&p].lines[0];
        assert!(l.1 > 50.0 && l.2 >= 50.0, "line holds the inline-block above its baseline: {l:?}");

        // float: left 30px wide and 30px tall: the first line starts after it.
        let t = laid_out(r#"<html><body><p id=p><span style="float:left;width:30px;height:30px"></span>ab cd</p></body></html>"#, css);
        let p = node_by_id(&t, "p").unwrap();
        let x = t.inline[&p].frags.iter().find_map(|f| match f { Frag::Text { x, .. } => Some(*x), _ => None }).unwrap();
        assert_eq!(x, 30.0, "the line box is shortened by the float");
    }

    /// css-color-4 / css-backgrounds-3 known answers: a 50% black background
    /// over white is mid grey (source-over), a transparent one paints
    /// nothing; an inset 0-blur shadow fills its offset band inside the
    /// padding box; overflow:hidden with a radius hides a child's corner.
    #[test]
    fn test_paint_effects_kat() {
        let t = laid_out(
            r#"<html><body>
            <div style="height:20px;background:rgba(0,0,0,0.5)"></div>
            <div style="height:20px;background:#ff0000;background-color:transparent"></div>
            <div style="height:40px;background:#ffffff;box-shadow:inset 10px 0 0 #0000ff"></div>
            <div style="height:60px;width:60px;border-radius:30px;overflow:hidden;background:#00ff00"><div style="height:60px;background:#ff0000"></div></div>
            </body></html>"#,
            "body{margin:0}",
        );
        let (w, h) = (100u32, 160u32);
        let mut surface = vec![0u8; (w * h * 4) as usize];
        crate::render::render_frame(&t, &mut surface, w, h, 0.0, 0.0, &[(0, 0, w, h)]);
        let px = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; (surface[i + 2], surface[i + 1], surface[i]) };
        let g = px(50, 10);
        assert!((g.0 as i32 - 128).abs() <= 2 && g.0 == g.1, "rgba(0,0,0,.5) over white: {g:?}");
        assert_eq!(px(50, 30), (255, 255, 255), "transparent overrides the earlier colour");
        assert_eq!(px(5, 60), (0, 0, 255), "inset shadow band at the left inside edge");
        assert_eq!(px(50, 60), (255, 255, 255), "inside the shadow's inner shape: the background");
        assert_eq!(px(2, 82), (255, 255, 255), "the rounded clip hides the child's corner");
        assert_eq!(px(30, 110), (255, 0, 0), "the child paints inside the rounded box");
    }

    /// CSS 2.2 §17.5.2 / §17.2.1 known answers: a rowspan cell covers its
    /// column in the rows below (their first cell starts in the next
    /// column) and is as tall as the rows it spans; a narrow table never
    /// shrinks a column below its min-content; table-layout: fixed takes the
    /// first row's width and splits the rest equally.
    #[test]
    fn test_table_spans_kat() {
        let css = "body{margin:0;font:14px monospace} table{border-spacing:0} td{padding:0}";
        let t = laid_out(
            r#"<html><body><table><tr><td id=s rowspan=2>ab</td><td id=a>x</td></tr><tr><td id=b>y</td></tr></table>
            <table style="width:40px"><tr><td id=w>abcdefghij</td><td>k l</td></tr></table>
            <table style="table-layout:fixed;width:300px"><tr><td id=f1 style="width:60px">a</td><td id=f2>b</td><td id=f3>c</td></tr></table></body></html>"#,
            css,
        );
        let (s, a, b) = (rect_by_id(&t, "s").unwrap(), rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap());
        assert_eq!(a.0, b.0, "the second row's cell sits in column 2 under the first row's: {a:?} {b:?}");
        assert!((s.3 - (a.3 + b.3)).abs() < 0.5, "the spanning cell is both rows tall: {s:?} {a:?} {b:?}");
        let w = rect_by_id(&t, "w").unwrap();
        let cw = crate::fonts::lines::Advancer::new(crate::fonts::FontSel::new(crate::fonts::MONO, 400, false), 14.0, 0.0).char('a');
        assert!(w.2 >= cw * 10.0 - 0.5, "min-content: the unbreakable word keeps its width: {w:?} cw {cw}");
        let (f1, f2, f3) = (rect_by_id(&t, "f1").unwrap(), rect_by_id(&t, "f2").unwrap(), rect_by_id(&t, "f3").unwrap());
        assert_eq!(f1.2, 60.0, "fixed: the first row's 60px column");
        assert!((f2.2 - 120.0).abs() < 0.5 && (f3.2 - 120.0).abs() < 0.5, "fixed: the rest shared equally: {f2:?} {f3:?}");
    }

    /// css-values-4 §6.1 / css-fonts-4 §2.5 / CSS 2.2 §10.8.1 known answers:
    /// em lengths are of the element's own font-size, a font-size in em of
    /// the parent's, rem of the root's; a line-height number inherits as the
    /// number (re-multiplied by the child's font), a length as the length.
    #[test]
    fn test_font_relative_units_kat() {
        let t = laid_out(
            r#"<html><body><div id=a><div id=b>x</div><div id=c>y</div></div><div id=d>z</div>
            <div id=n><span id=ns>n</span></div><div id=l><span id=ls>l</span></div></body></html>"#,
            "html{font-size:10px} body{margin:0;font-family:monospace} #a{font-size:20px} \
             #b{width:10em;padding-left:1em;box-sizing:content-box} #c{font-size:1.5em;width:2em} #d{width:5rem} \
             #n{font-size:10px;line-height:2} #ns{font-size:20px} #l{font-size:10px;line-height:20px} #ls{font-size:30px}",
        );
        let b = rect_by_id(&t, "b").unwrap();
        assert_eq!(b.2, 220.0, "10em + 1em padding at the element's 20px font: {b:?}");
        let c = node_by_id(&t, "c").unwrap();
        assert_eq!(t.paint_map[&c].used_font_size, Some(30.0), "1.5em of the parent's 20px");
        assert_eq!(rect_by_id(&t, "c").unwrap().2, 60.0, "2em of its own 30px");
        assert_eq!(rect_by_id(&t, "d").unwrap().2, 50.0, "5rem of the root's 10px");
        let n = node_by_id(&t, "n").unwrap();
        assert!(t.inline[&n].lines[0].1 >= 40.0, "number line-height re-multiplied by the 20px child: {:?}", t.inline[&n].lines);
        let l = node_by_id(&t, "l").unwrap();
        assert_eq!(t.inline[&l].lines[0].1.round(), 20.0f32.max(t.inline[&l].lines[0].1.round()), "length line-height inherited as 20px");
        assert!(t.inline[&l].lines[0].1 < 40.0, "length line-height is not re-multiplied: {:?}", t.inline[&l].lines);
    }

    /// CSS 2.2 Appendix E / §9.9.1 known answers: a z-index inside a
    /// z-indexed parent never rises above the parent's context (z100 in z1
    /// stays under a z2 sibling); positioned descendants of a z-index:auto
    /// parent are hoisted into the root context (z3 under an auto parent
    /// beats a later z2); a negative z-index paints under in-flow content of
    /// the root context but over the canvas; opacity 0.5 composites the
    /// group once (an overlap of two opaque children shows only the top one,
    /// half mixed with white).
    #[test]
    fn test_stacking_context_kat() {
        // AETHERABSCB (SR431): the cyan box has no positioned ancestor, so its containing block is the ICB (top:100px, was top:0 under the old every-parent-is-a-containing-block rule).
        let t = laid_out(
            r#"<html><body>
            <div style="position:relative;height:100px">
              <div style="position:relative;z-index:1"><div style="position:absolute;left:0;top:0;width:40px;height:40px;background:#ff0000;z-index:100"></div></div>
              <div style="position:absolute;left:20px;top:20px;width:40px;height:40px;background:#00ff00;z-index:2"></div>
              <div style="position:relative"><div style="position:absolute;left:100px;top:0;width:40px;height:40px;background:#0000ff;z-index:3"></div></div>
              <div style="position:absolute;left:120px;top:20px;width:40px;height:40px;background:#ffff00;z-index:2"></div>
            </div>
            <div style="height:40px;width:100px;background:#ff00ff"><div style="position:absolute;z-index:-1;left:0;top:100px;width:200px;height:40px;background:#00ffff"></div></div>
            <div style="opacity:0.5;position:relative;height:60px"><div style="position:absolute;left:0;top:0;width:40px;height:40px;background:#ff0000"></div><div style="position:absolute;left:20px;top:20px;width:40px;height:40px;background:#0000ff"></div></div>
            </body></html>"#,
            "body{margin:0}",
        );
        let (w, h) = (200u32, 220u32);
        let mut surface = vec![0u8; (w * h * 4) as usize];
        crate::render::render_frame(&t, &mut surface, w, h, 0.0, 0.0, &[(0, 0, w, h)]);
        let px = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; (surface[i + 2], surface[i + 1], surface[i]) }; // BGRA
        assert_eq!(px(30, 30), (0, 255, 0), "z100 inside a z1 context stays under the z2 sibling");
        assert_eq!(px(130, 30), (0, 0, 255), "z3 under a z-index:auto parent is hoisted over the later z2");
        assert_eq!(px(10, 110), (255, 0, 255), "in-flow background over the negative z-index box");
        assert_eq!(px(150, 110), (0, 255, 255), "the negative z-index box over the canvas beside the flow");
        let ov = px(30, 170);
        assert!(ov.0 < 140 && ov.1 > 110 && ov.1 < 145 && ov.2 > 240, "group opacity: only the top box, half mixed: {ov:?}");
        let red = px(5, 145);
        assert!(red.0 > 240 && red.1 > 110 && red.1 < 145, "red at half opacity over white: {red:?}");
    }

    /// CSS 2.2 Appendix E: a higher z-index paints over a later sibling;
    /// position: fixed with bottom: 0 sits at the viewport's bottom edge.
    #[test]
    fn test_z_index_and_fixed_kat() {
        let t = laid_out(
            r#"<html><body><div style="position:relative;height:100px"><div style="position:absolute;left:0;top:0;width:50px;height:50px;background:#ff0000;z-index:2"></div><div style="position:absolute;left:25px;top:25px;width:50px;height:50px;background:#0000ff;z-index:1"></div></div><div style="position:fixed;left:0;right:0;bottom:0;height:10px;background:#00ff00"></div></body></html>"#,
            "body{margin:0}",
        );
        let (w, h) = (t.viewport.0 as u32, t.viewport.1 as u32); // AETHERFIT (SR189): paint at the layout's viewport
        let mut surface = vec![0u8; (w * h * 4) as usize];
        crate::render::render_frame(&t, &mut surface, w, h, 0.0, 0.0, &[(0, 0, w, h)]);
        let px = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; (surface[i + 2], surface[i + 1], surface[i]) }; // BGRA
        let red = px(40, 40);
        assert!(red.0 > 200 && red.2 < 50, "z-index 2 over z-index 1: {red:?}");
        let blue = px(60, 60);
        assert!(blue.2 > 200, "the lower box shows outside the overlap: {blue:?}");
        let bar = px(100, h - 5);
        assert!(bar.1 > 200 && bar.0 < 50, "fixed bar at the viewport bottom: {bar:?}");
    }

    /// css-counter-styles-3 predefined styles as list markers.
    #[test]
    fn test_marker_text_kat() {
        use crate::render::marker_text;
        assert_eq!(marker_text(4, 3), "3. ");
        assert_eq!(marker_text(5, 28), "ab. ");
        assert_eq!(marker_text(6, 1), "A. ");
        assert_eq!(marker_text(7, 1994), "mcmxciv. ");
        assert_eq!(marker_text(8, 4), "IV. ");
        assert_eq!(marker_text(9, 7), "07. ");
    }

    // ---------------------------------------------------------------------------------------------
    // AETHERSELECT (SR69): the selection model, its paint, copy and the script API
    // ---------------------------------------------------------------------------------------------

    const SEL_PAGE: &str = r#"<!DOCTYPE html><html><body style="margin:0;font:16px sans-serif">
<p id="p" style="margin:10px;width:200px">alpha beta <b>gamma</b> delta epsilon zeta eta theta</p>
<p id="q" style="margin:10px">second</p></body></html>"#;

    fn sel_engine(html: &str) -> crate::AetherEngine {
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 200));
        e.load_html("file:///sel.html", html, true);
        e.render_frame();
        e
    }

    fn mid(b: &crate::render::TextBox) -> f64 {
        ((b.top + b.bottom) / 2.0) as f64
    }

    #[test]
    fn test_selection_drag_paints_and_copies() {
        let mut e = sel_engine(SEL_PAGE);
        let boxes = crate::render::text_boxes();
        assert!(boxes.len() >= 3, "the paragraph's fragments are on record: {}", boxes.len());
        let first = boxes[0].clone();
        let last_line = boxes.iter().filter(|b| b.top > first.top && b.top < first.top + 40.0).last().cloned().expect("a second line");
        // MouseDown (this arc's arm) at the first glyph, motion to the end of the second line, release.
        e.handle_event(crate::api::events::Event::MouseDown(first.x as f64 + 0.5, mid(&first)));
        let s = crate::selection::get().expect("a press places a caret");
        assert!(s.is_collapsed() && s.anchor.offset == 0, "{s:?}");
        e.pointer_moved((last_line.x + last_line.w) as f64 + 30.0, mid(&last_line));
        e.pointer_released();
        let text = e.selected_text();
        assert!(text.starts_with("alpha beta gamma delta"), "{text:?}");
        assert!(!text.contains("second"), "{text:?}");
        // Motion after the release does not extend.
        e.pointer_moved(5.0, 190.0);
        assert_eq!(e.selected_text(), text);
        // Paint: the highlight (rgb(0,65,198) at 204/255 over white = 51,103,209; BGRA) on both lines.
        assert!(e.tick(), "the selection change repaints");
        e.render_frame();
        let w = e.width as usize;
        let hl = |y: f32, x0: f32, x1: f32| {
            let y = y as usize;
            (x0 as usize..x1 as usize).any(|x| e.surface[(y * w + x) * 4..(y * w + x) * 4 + 3] == [209, 103, 51])
        };
        assert!(hl(first.top + 1.0, first.x, first.x + 30.0), "line 1 highlighted");
        assert!(hl(last_line.top + 1.0, last_line.x, last_line.x + 30.0), "line 2 highlighted");
        // Copy: BrowserClipboardSet carries the innerText serialization.
        match e.copy_selection() {
            Some(bandy::SMessage::BrowserClipboardSet(t)) => assert_eq!(t, text),
            other => panic!("copy: {other:?}"),
        }
        // A plain click elsewhere collapses it: nothing to copy, nothing painted.
        e.press_at(first.x as f64 + 0.5, mid(&first), 1, false);
        e.pointer_released();
        assert!(e.copy_selection().is_none());
    }

    #[test]
    fn test_selection_word_block_shift_and_select_all() {
        let mut e = sel_engine(SEL_PAGE);
        let first = crate::render::text_boxes()[0].clone();
        // "alpha beta " — a point inside "beta".
        let beta_x = first.x as f64 + crate::selection::caret_x(&first.adv, "alpha be", 8) as f64;
        e.press_at(beta_x, mid(&first), 2, false);
        assert_eq!(e.selected_text(), "beta", "double click: the word");
        e.pointer_released();
        e.press_at(beta_x, mid(&first), 3, false);
        assert_eq!(e.selected_text(), "alpha beta gamma delta epsilon zeta eta theta", "triple click: the block");
        e.pointer_released();
        // Shift+click extends from the caret.
        e.press_at(first.x as f64 + 0.5, mid(&first), 1, false);
        e.pointer_released();
        e.press_at(beta_x, mid(&first), 1, true);
        e.pointer_released();
        assert_eq!(e.selected_text(), "alpha be");
        // Ctrl/Cmd+A.
        e.select_all();
        assert_eq!(e.selected_text(), "alpha beta gamma delta epsilon zeta eta theta\n\nsecond");
        // A drag after a double click extends by words.
        e.press_at(first.x as f64 + 1.0, mid(&first), 2, false);
        e.pointer_moved(beta_x, mid(&first));
        e.pointer_released();
        assert_eq!(e.selected_text(), "alpha beta");
    }

    #[test]
    fn test_selection_script_api_and_selectionchange() {
        let page = SEL_PAGE.replace(
            "</body>",
            "<script>window.n = 0; document.addEventListener('selectionchange', () => { window.n++; });</script></body>",
        );
        let mut e = sel_engine(&page);
        let js = |e: &mut crate::AetherEngine, src: &str| e.js_engine.as_mut().unwrap().eval_string(src).unwrap();
        assert_eq!(js(&mut e, "[getSelection().rangeCount, getSelection().type, getSelection().anchorNode].join()"), "0,None,");
        assert_eq!(js(&mut e, "getSelection() === document.getSelection() && getSelection() instanceof Selection"), "true");
        let r = js(
            &mut e,
            "var t = document.getElementById('p').firstChild; var s = getSelection(); s.setBaseAndExtent(t, 0, t, 5); \
             [s.anchorNode === t, s.anchorOffset, s.focusOffset, s.isCollapsed, s.rangeCount, s.toString(), s.type, s.direction].join()",
        );
        assert_eq!(r, "true,0,5,false,1,alpha,Range,forward");
        // The engine sees the script's selection (and repaints for it).
        let sel = crate::selection::get().unwrap();
        assert_eq!((sel.anchor.offset, sel.focus.offset), (0, 5));
        assert_eq!(e.selected_text(), "alpha");
        js(&mut e, "s.collapse(t, 2); s.extend(t, 4);");
        assert_eq!(js(&mut e, "[s.toString(), s.isCollapsed, s.type].join()"), "ph,false,Range");
        js(&mut e, "s.extend(t, 0)");
        assert_eq!(js(&mut e, "[s.toString(), s.direction, s.getRangeAt(0).startOffset, s.getRangeAt(0).endOffset].join()"), "al,backward,0,2");
        js(&mut e, "s.removeAllRanges()");
        assert_eq!(js(&mut e, "[s.rangeCount, s.anchorNode, s.isCollapsed].join()"), "0,,true");
        js(&mut e, "var r = document.createRange(); r.setStart(t, 1); r.setEnd(t, 3); s.addRange(r);");
        assert_eq!(js(&mut e, "[s.toString(), r.toString(), r instanceof Range, s.getRangeAt(0).toString(), s.containsNode(t, true), s.containsNode(t)].join()"), "lp,lp,true,lp,true,false");
        assert_eq!(js(&mut e, "try { s.collapse(t, 999); 'no' } catch (x) { x.name }"), "IndexSizeError");
        assert_eq!(js(&mut e, "try { s.getRangeAt(1); 'no' } catch (x) { x.name }"), "IndexSizeError");
        js(&mut e, "s.selectAllChildren(document.getElementById('q'))");
        assert_eq!(js(&mut e, "s.toString()"), "second");
        // selectionchange: a task, coalesced while one is queued.
        assert_eq!(js(&mut e, "String(window.n)"), "0");
        e.tick();
        assert_eq!(js(&mut e, "String(window.n)"), "1");
        // A user selection fires it too.
        e.select_all();
        e.tick();
        assert_eq!(js(&mut e, "String(window.n)"), "2");
    }

    // ---- AETHERGEN (SR71): generated content, counters, quotes, ::marker, focus states ----

    fn gen_tree(html: &str, css_text: &str) -> layout::LayoutTree {
        let doc = dom::parse_html(html);
        let mut t = layout::build_tree(&doc, 800.0, 600.0);
        css::apply_stylesheets(&mut t, &[css_text.to_string()]);
        t
    }

    fn tid_of(tree: &layout::LayoutTree, id: &str) -> taffy::NodeId {
        tree.node_map
            .iter()
            .find(|(_, n)| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some(id)))
            .map(|(t, _)| *t)
            .unwrap_or_else(|| panic!("no box for #{id}"))
    }

    /// A generated box: a detached element of the tree (no DOM parent).
    fn is_generated(tree: &layout::LayoutTree, t: taffy::NodeId) -> bool {
        tree.node_map.get(&t).is_some_and(|n| n.as_element().is_some() && n.parent().is_none())
    }

    /// The text of every generated box under `id`'s box, in box-tree order.
    fn generated_texts(tree: &layout::LayoutTree, id: &str) -> Vec<String> {
        fn walk(tree: &layout::LayoutTree, t: taffy::NodeId, out: &mut Vec<String>) {
            if is_generated(tree, t) {
                out.push(tree.node_map[&t].text_contents());
                return;
            }
            for k in tree.taffy.children(t).unwrap_or_default() {
                walk(tree, k, out);
            }
        }
        let mut out = Vec::new();
        walk(tree, tid_of(tree, id), &mut out);
        out
    }

    /// css-content-3 §2: strings and attr() make ::before/::after boxes, the first and last child
    /// of the originating element's box, inline, styled by their own rules.
    #[test]
    fn test_generated_before_after_boxes() {
        let html = r#"<html><body><p id="p" data-x="X">mid</p><div id="d"><p>block</p></div></body></html>"#;
        let t = gen_tree(html, r#"#p::before { content: "[" attr(data-x) "]"; color: rgb(1, 2, 3); }
            #p::after { content: "!" attr(data-missing); }
            #d::after { content: ""; display: block; height: 7px; }"#);
        assert_eq!(generated_texts(&t, "p"), vec!["[X]".to_string(), "!".to_string()]);
        let p = tid_of(&t, "p");
        let kids = t.taffy.children(p).unwrap();
        assert!(is_generated(&t, kids[0]) && is_generated(&t, *kids.last().unwrap()), "first and last child");
        assert_eq!(t.paint_map[&kids[0]].color, Some((1, 2, 3)));
        // The ::before text is laid out in the paragraph's line box, ahead of "mid".
        let frags = &t.inline.get(&p).expect("p is an inline formatting context").frags;
        let texts: Vec<&str> = frags.iter().filter_map(|f| match f { layout::inline::Frag::Text { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(texts.first().map(|s| s.trim()), Some("[X]"), "{texts:?}");
        // A block-level ::after is a block box with its own height.
        let d = tid_of(&t, "d");
        let last = *t.taffy.children(d).unwrap().last().unwrap();
        assert!(is_generated(&t, last));
        assert_eq!(t.taffy.layout(last).unwrap().size.height, 7.0);
    }

    /// `content: none | normal`, no `content` at all, and `display: none` generate no box; a
    /// replaced element takes none.
    #[test]
    fn test_generated_suppression() {
        let html = r#"<html><body><p id="a">a</p><p id="b">b</p><p id="c">c</p><p id="e">e</p><span id="w"><img id="i" src="x.png"></span></body></html>"#;
        let t = gen_tree(html, r#"#a::before { content: none; } #b::before { content: normal; }
            #c::before { color: red; } #e::before { content: "x"; display: none; }
            img::before { content: "no"; }"#);
        for id in ["a", "b", "c", "e", "w"] {
            assert!(generated_texts(&t, id).is_empty(), "#{id}");
        }
    }

    /// css-lists-3 §4: counter-reset/-increment/-set, nested scopes through counters(), the
    /// predefined counter styles, and the implicit list-item counter (ol start, li value).
    #[test]
    fn test_generated_counters() {
        let html = r#"<html><body><ol id="o">
            <li>a<ol><li>a1</li><li>a2</li></ol></li>
            <li>b<ol><li>b1<ol><li>b11</li></ol></li></ol></li>
        </ol>
        <div id="s"><span class="r"></span><span class="r"></span><span class="r"></span><span class="z"></span></div>
        <ol id="l" start="3"><li>x</li><li value="10">y</li><li>z</li></ol>
        </body></html>"#;
        let t = gen_tree(html, r#"#o, #o ol { counter-reset: item; list-style: none; }
            #o li { counter-increment: item; }
            #o li::before { content: counters(item, ".") " "; }
            #s { counter-reset: n 1; }
            .r { counter-increment: n 2; }
            .r::before { content: counter(n, lower-roman) "/" counter(n, upper-alpha) "/" counter(n, decimal-leading-zero) "/" counter(n, disc); }
            .z { counter-set: n 40; }
            .z::after { content: counter(n, lower-greek) counter(none-such); }
            #l li::before { content: counter(list-item) ":"; }"#);
        let got: Vec<String> = generated_texts(&t, "o").iter().map(|s| s.trim().to_string()).collect();
        assert_eq!(got, ["1", "1.1", "1.2", "2", "2.1", "2.1.1"]);
        // lower-greek is `system: alphabetic` over 24 symbols (css-counter-styles-3 §6.2.2, the alphabetic
        // algorithm §3.1.4): 40 = 1×24 + 16 → "α" then the 16th symbol "π" ("μ", the 12th, is 12).
        assert_eq!(generated_texts(&t, "s"), ["iii/C/03/\u{2022}", "v/E/05/\u{2022}", "vii/G/07/\u{2022}", "\u{3B1}\u{3C0}0"]);
        assert_eq!(generated_texts(&t, "l"), ["3:", "10:", "11:"]);
    }

    /// css-content-3 §3: open/close quotes nest through one depth, by `quotes` or the UA pairs of
    /// the language; `<q>` quotes itself through the UA sheet.
    #[test]
    fn test_generated_quotes() {
        let html = r#"<html><body><p id="en"><q>a <q>b</q> c</q></p>
            <p id="de" lang="de"><q>a <q>b</q></q></p>
            <p id="cu"><q>x</q><span class="n"></span></p></body></html>"#;
        let t = gen_tree(html, r##"#cu { quotes: "<" ">" "(" ")"; } .n::before { content: no-open-quote close-quote; }"##);
        assert_eq!(generated_texts(&t, "en"), ["\u{201C}", "\u{2018}", "\u{2019}", "\u{201D}"]);
        assert_eq!(generated_texts(&t, "de"), ["\u{201E}", "\u{201A}", "\u{2018}", "\u{201C}"]);
        assert_eq!(generated_texts(&t, "cu"), ["<", ">", ">"]);
    }

    /// `content: url()` is a replaced inline image (here a data: SVG, decoded by the image store).
    #[test]
    fn test_generated_url_image() {
        let html = r#"<html><body><p id="p">t</p></body></html>"#;
        let t = gen_tree(html, r#"#p::before { content: url("data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='12' height='10'><rect width='12' height='10' fill='red'/></svg>"); }"#);
        let p = tid_of(&t, "p");
        let g = t.taffy.children(p).unwrap()[0];
        assert!(is_generated(&t, g));
        let img = t.taffy.children(g).unwrap()[0];
        assert_eq!(t.node_map[&img].local_name().as_deref(), Some("img"));
        let l = t.taffy.layout(img).unwrap();
        assert_eq!((l.size.width, l.size.height), (12.0, 10.0));
    }

    /// css-lists-3 §3.1: an author-styled ::marker carries its content, colour and font to the
    /// item; rendered, the marker paints in its colour left of the content edge.
    #[test]
    fn test_generated_marker() {
        let html = r#"<html><body><ul><li id="a">one</li><li id="b">two</li></ul><ol><li id="c">x</li></ol></body></html>"#;
        let t = gen_tree(html, r#"#a::marker { content: "> "; color: rgb(250, 0, 0); font-size: 20px; }
            #b::marker { color: rgb(0, 0, 250); } #c::marker { content: none; }"#);
        let mk = |id: &str| t.paint_map[&tid_of(&t, id)].marker.clone().expect("marker paint");
        assert_eq!(mk("a").text.as_deref(), Some("> "));
        assert_eq!(mk("a").color, Some((250, 0, 0)));
        assert_eq!(mk("a").size(16.0), 20.0);
        assert_eq!(mk("b").text, None);
        assert!(mk("c").none);
        let mut surface = vec![0u8; 300 * 120 * 4];
        render::render_frame(&t, &mut surface, 300, 120, 0.0, 0.0, &[(0, 0, 300, 120)]);
        let content_x = t.taffy.layout(tid_of(&t, "a")).unwrap().location.x;
        let colored = |r: u8, b: u8| surface.chunks_exact(4).enumerate().any(|(i, p)| {
            ((i % 300) as f32) < content_x + 40.0 && p[2] > 200 && p[1] < 60 && p[0] < 60 && r > 0
                || ((i % 300) as f32) < content_x + 40.0 && p[0] > 200 && p[1] < 60 && p[2] < 60 && b > 0
        });
        assert!(colored(1, 0), "a red marker (BGRA)");
        assert!(colored(0, 1), "a blue bullet");
    }

    /// :focus, :focus-visible and :focus-within follow the engine's focus (the page's active
    /// element); focus-visible drops after a pointer focus except on a text field.
    #[test]
    fn test_focus_visible_within() {
        let html = r#"<html><body><form id="f"><input id="i"><button id="b">go</button></form><p id="o">o</p></body></html>"#;
        let doc = dom::parse_html(html);
        let id_of = |id: &str| doc.select_first(&format!("#{id}")).unwrap().as_node().id().0;
        let css_text = "#i:focus-visible, #b:focus-visible { color: rgb(1, 1, 1); } #f:focus-within { color: rgb(2, 2, 2); } #o:focus-within { color: rgb(3, 3, 3); } #b:focus { font-size: 30px; }";
        let color = |focus: Option<usize>, pointer: bool, id: &str| {
            crate::js::page(|p| {
                p.doc = Some(doc.clone());
                p.focused = focus;
            });
            css::note_focus_modality(pointer);
            let mut t = layout::build_tree(&doc, 800.0, 600.0);
            css::apply_stylesheets(&mut t, &[css_text.to_string()]);
            let p = t.paint_map[&tid_of(&t, id)].clone();
            (p.color, p.font_size)
        };
        assert_eq!(color(None, false, "f").0, None, "nothing focused");
        assert_eq!(color(Some(id_of("i")), false, "i").0, Some((1, 1, 1)));
        assert_eq!(color(Some(id_of("i")), false, "f").0, Some((2, 2, 2)));
        assert_eq!(color(Some(id_of("i")), false, "o").0, None);
        assert_eq!(color(Some(id_of("b")), false, "b"), (Some((1, 1, 1)), Some(30.0)));
        assert_eq!(color(Some(id_of("b")), true, "b"), (None, Some(30.0)), "pointer focus on a button");
        assert_eq!(color(Some(id_of("i")), true, "i").0, Some((1, 1, 1)), "a text field always shows focus");
        crate::js::page(|p| {
            p.doc = None;
            p.focused = None;
        });
        css::note_focus_modality(false);
    }

    // ---------------------------------------------------------------------------------------------
    // AETHERINPUT (SR68): the editing model, mouse submission, checkable controls, keyboard scrolling.
    // ---------------------------------------------------------------------------------------------

    fn dom_by_id(engine: &crate::AetherEngine, id: &str) -> crate::dom::NodeRef {
        engine
            .document
            .as_ref()
            .unwrap()
            .descendants()
            .find(|n| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some(id)))
            .unwrap_or_else(|| panic!("#{id}"))
    }

    /// A click (MouseDown + MouseUp) at the centre of `#id`'s box, in viewport coordinates.
    fn click_id(engine: &mut crate::AetherEngine, id: &str) {
        let n = dom_by_id(engine, id);
        let (x, y, w, h) = engine.box_of(&n).expect("box");
        let (cx, cy) = ((x + w / 2.0) as f64 - engine.scroll_x, (y + h / 2.0) as f64 - engine.scroll_y);
        engine.handle_event(crate::api::events::Event::MouseDown(cx, cy));
        engine.handle_event(crate::api::events::Event::MouseUp(cx, cy));
    }

    fn key(engine: &mut crate::AetherEngine, k: &str) {
        engine.handle_event(crate::api::events::Event::KeyDown(k.to_string()));
    }

    /// Caret movement is by grapheme cluster: a combining sequence, an emoji ZWJ sequence and a flag
    /// are each ONE step; Backspace removes a combining mark alone (Blink's BackspaceStateMachine) but
    /// an emoji sequence whole; Delete removes the next whole cluster.
    #[test]
    fn edit_caret_moves_by_grapheme_kat() {
        use forms::edit::EditState;
        // "e" + COMBINING ACUTE, a family (man ZWJ woman ZWJ girl), the flag of Norway, "x"
        let t0 = "e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{1F1F3}\u{1F1F4}x";
        let mut t = t0.to_string();
        let mut e = EditState::at(0);
        let stops: Vec<usize> = (0..5)
            .map(|_| {
                e.key(&mut t, "Right", false);
                e.caret
            })
            .collect();
        assert_eq!(stops, vec![3, 3 + 18, 3 + 18 + 8, 3 + 18 + 8 + 1, 30], "one Right per cluster");
        assert_eq!(t, t0, "moving never edits");
        e.key(&mut t, "Left", false);
        e.key(&mut t, "Left", false);
        assert_eq!(e.caret, 21, "Left steps back over the flag whole");
        // Backspace before the flag removes the WHOLE family sequence
        e.key(&mut t, "BackSpace", false);
        assert_eq!(t, "e\u{301}\u{1F1F3}\u{1F1F4}x");
        assert_eq!(e.caret, 3);
        // Backspace after "é" (decomposed) removes only the accent
        e.key(&mut t, "BackSpace", false);
        assert_eq!(t, "e\u{1F1F3}\u{1F1F4}x");
        // Delete removes the next whole cluster (the flag)
        e.key(&mut t, "Delete", false);
        assert_eq!(t, "ex");
        assert_eq!(e.caret, 1);
        // a caret set inside a cluster snaps to its start before it moves
        let mut t = "a\u{1F468}\u{200D}\u{1F469}b".to_string();
        let mut e = EditState::at(3);
        e.key(&mut t, "Right", false);
        assert_eq!(e.caret, t.len() - 1, "snapped to the cluster start, then stepped over it");
    }

    /// Mid-string insertion and deletion, Shift selection, word moves, Home/End, Ctrl+A, and the
    /// textarea's logical-line keys.
    #[test]
    fn edit_insert_delete_select_words_kat() {
        use forms::edit::{EditState, Outcome};
        let mut t = "hello world".to_string();
        let mut e = EditState::at(5);
        e.insert(&mut t, ",");
        assert_eq!((t.as_str(), e.caret), ("hello, world", 6));
        assert_eq!(e.key(&mut t, "Ctrl+Right", false), Outcome::Moved);
        assert_eq!(e.caret, 12, "Unix Ctrl+Right stops at the word's END");
        e.key(&mut t, "Ctrl+Left", false);
        assert_eq!(e.caret, 7, "Ctrl+Left goes to the word's start");
        e.key(&mut t, "Alt+Left", false);
        assert_eq!(e.caret, 0, "Alt is a word move too, over the comma and space");
        e.key(&mut t, "Shift+End", false);
        assert_eq!(e.selection(), (0, 12));
        e.key(&mut t, "Left", false);
        assert_eq!((e.caret, e.has_selection()), (0, false), "Left collapses to the selection start");
        e.key(&mut t, "Shift+Ctrl+Right", false);
        e.insert(&mut t, "HELLO");
        assert_eq!(t, "HELLO, world", "typing replaces the selection");
        e.key(&mut t, "Ctrl+a", false);
        e.key(&mut t, "Delete", false);
        assert_eq!((t.as_str(), e.caret), ("", 0), "Ctrl+A then Delete empties the field");
        assert_eq!(e.key(&mut t, "BackSpace", false), Outcome::Consumed, "Backspace at 0 is a no-op");
        // Ctrl+Backspace deletes the word before the caret
        let mut t = "one two three".to_string();
        let mut e = EditState::at(t.len());
        e.key(&mut t, "Ctrl+BackSpace", false);
        assert_eq!(t, "one two ");
        // textarea: Return inserts LF; Home/End are the logical line's; Up/Down keep the column
        let mut t = "ab\ncdef".to_string();
        let mut e = EditState::at(t.len());
        e.key(&mut t, "Home", true);
        assert_eq!(e.caret, 3);
        e.key(&mut t, "Right", true);
        e.key(&mut t, "Right", true);
        e.key(&mut t, "Up", true);
        assert_eq!(e.caret, 2, "column 2 of line 0 (its end)");
        e.key(&mut t, "Down", true);
        assert_eq!(e.caret, 5);
        assert_eq!(e.key(&mut t, "Return", true), Outcome::Edited);
        assert_eq!(t, "ab\ncd\nef");
        let mut one = "x".to_string();
        assert_eq!(EditState::at(1).key(&mut one, "Return", false), Outcome::Submit, "Return submits a single-line field");
        assert_eq!(forms::edit::sanitize_insert("a\r\nb\rc", true), "a\nb\nc");
        assert_eq!(forms::edit::sanitize_insert("a\r\nb\nc", false), "abc");
    }

    /// The engine end to end: a textarea takes typing (it did not: Text gated on <input>), a mid-string
    /// insert after arrow moves, multi-line paste; an input's paste strips line breaks; Return in a
    /// textarea is a newline, not a submit; the submission carries both values CRLF-normalized.
    #[test]
    fn edit_engine_textarea_typing_and_paste() {
        let html = r#"<html><body><form action="/s" method="get">
            <input id="i" name="i" value="ab">
            <textarea id="t" name="t" rows="3" cols="20">xy</textarea>
            <input type="submit" id="go" name="go" value="Go">
        </form></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/p", html, true);
        engine.render_frame();
        click_id(&mut engine, "t");
        let t = dom_by_id(&engine, "t");
        assert_eq!(engine.focused_node.as_ref(), Some(&t));
        key(&mut engine, "Ctrl+End");
        engine.handle_event(crate::api::events::Event::Text("z".into()));
        key(&mut engine, "Left");
        key(&mut engine, "Left");
        engine.handle_event(crate::api::events::Event::Text("-".into()));
        assert_eq!(forms::control_value(&t), "x-yz", "typed mid-string in a textarea");
        key(&mut engine, "Ctrl+End");
        key(&mut engine, "Return");
        assert!(engine.take_pending_nav().is_none(), "Return in a textarea does not submit");
        engine.paste("one\r\ntwo");
        assert_eq!(forms::control_value(&t), "x-yz\none\ntwo");
        assert_eq!(engine.edit.caret, "x-yz\none\ntwo".len());
        // the input: paste strips line breaks, Return submits through the default button
        click_id(&mut engine, "i");
        key(&mut engine, "End");
        engine.paste("c\nd");
        assert_eq!(forms::control_value(&dom_by_id(&engine, "i")), "abcd");
        key(&mut engine, "Return");
        let nav = engine.take_pending_nav().expect("Return submits");
        assert_eq!(nav.url, "https://example.com/s?i=abcd&t=x-yz%0D%0Aone%0D%0Atwo&go=Go", "the default button is the submitter");
        // painting: the focused field shows a caret column in its text colour
        engine.damage_rects.push((0, 0, engine.width, engine.height));
        engine.render_frame();
    }

    /// A click on a submit button submits with the button as the submitter; checkboxes and radios
    /// toggle on click, radios exclusively, and only checked ones submit; a click on a label activates
    /// its control; `formmethod`/`formenctype` on the submitter win.
    #[test]
    fn click_submits_and_toggles_checkables() {
        let html = r#"<html><body><form action="/go" method="get">
            <input type="checkbox" id="c" name="c">
            <input type="checkbox" id="d" name="d" value="dv" checked>
            <input type="radio" id="r1" name="r" value="1" checked>
            <input type="radio" id="r2" name="r" value="2">
            <label id="lab" for="c">tick c</label>
            <select name="s"><option>A</option><option value="bee" selected>B</option></select>
            <button id="b" name="act" value="save">Save</button>
            <button id="p" name="act" value="post" formmethod="post" formenctype="text/plain">Post</button>
            <button type="button" id="nb">Nothing</button>
        </form></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/x/y", html, true);
        engine.render_frame();
        let (c, d, r1, r2) = (dom_by_id(&engine, "c"), dom_by_id(&engine, "d"), dom_by_id(&engine, "r1"), dom_by_id(&engine, "r2"));
        assert!(!forms::checkedness(&c) && forms::checkedness(&d));
        click_id(&mut engine, "c");
        click_id(&mut engine, "d");
        click_id(&mut engine, "r2");
        assert!(forms::checkedness(&c), "click checks");
        assert!(!forms::checkedness(&d), "click unchecks");
        assert!(forms::checkedness(&r2) && !forms::checkedness(&r1), "a radio group is exclusive");
        assert!(d.as_element().unwrap().attributes.borrow().get("checked").is_some(), "the attribute stays the default");
        click_id(&mut engine, "nb");
        assert!(engine.take_pending_nav().is_none(), "a type=button button does not submit");
        click_id(&mut engine, "b");
        let nav = engine.take_pending_nav().expect("the submit button submits");
        assert_eq!(nav.url, "https://example.com/go?c=on&r=2&s=bee&act=save");
        click_id(&mut engine, "lab");
        assert!(!forms::checkedness(&c), "a label click toggles its control");
        click_id(&mut engine, "p");
        let nav = engine.take_pending_nav().expect("post");
        assert_eq!(nav.method, forms::HttpMethod::Post);
        assert_eq!(nav.content_type.as_deref(), Some("text/plain"));
        // A `method=get` form posted by `formmethod=post formenctype=text/plain` sends its body URLENCODED
        // under a text/plain header in Chromium (tests/data/forms_oracle/getform-postplain.expect, the
        // sha-pinned measured vector for this exact shape; forms/mod.rs plan_navigation reproduces it).
        assert_eq!(nav.body.as_deref(), Some("r=2&s=bee&act=post"));
    }

    /// A `submit` listener that cancels keeps the page (a scripted form submits itself).
    #[test]
    fn canceled_submit_event_stages_no_navigation() {
        let html = r#"<html><body><form id="f" action="/go"><input name="q" value="1"><button id="b">Go</button></form>
            <script>document.getElementById('f').addEventListener('submit', function (e) { e.preventDefault(); document.title = 'handled'; });</script>
            </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        click_id(&mut engine, "b");
        assert!(engine.take_pending_nav().is_none(), "preventDefault on submit cancels the navigation");
        let f = dom_by_id(&engine, "f");
        assert!(f.as_element().unwrap().attributes.borrow().get("data-aether-submitting").is_none(), "no tag left behind");
    }

    /// Keyboard scrolling is the engine's: Space / Page Down move 87.5 % of the viewport, Down 40 px,
    /// Shift+Space and Page Up back, End to the bottom, Home to the top — and none of it while a text
    /// field holds the keys. `autofocus` focuses on load with the caret at 0.
    #[test]
    fn keys_scroll_the_page_unless_a_field_is_focused() {
        let html = r#"<html><body style="margin:0"><input id="f" autofocus value="abc"><div style="height:5000px"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(800, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let f = dom_by_id(&engine, "f");
        assert_eq!(engine.focused_node.as_ref(), Some(&f), "autofocus");
        assert_eq!(engine.edit.caret, 0, "Chromium's autofocus caret is at 0");
        key(&mut engine, "space");
        key(&mut engine, "Page_Down");
        assert_eq!(engine.scroll_y, 0.0, "a focused field keeps Space and Page Down");
        // blur by clicking the empty page below the field
        engine.handle_event(crate::api::events::Event::MouseUp(400.0, 300.0));
        assert!(engine.focused_node.is_none(), "a click outside a control blurs");
        key(&mut engine, "space");
        assert_eq!(engine.scroll_y, 525.0);
        key(&mut engine, "Page_Down");
        assert_eq!(engine.scroll_y, 1050.0);
        key(&mut engine, "Down");
        assert_eq!(engine.scroll_y, 1090.0);
        key(&mut engine, "Shift+space");
        assert_eq!(engine.scroll_y, 565.0);
        key(&mut engine, "Up");
        key(&mut engine, "Page_Up");
        assert_eq!(engine.scroll_y, 0.0);
        key(&mut engine, "End");
        let max = engine.scroll_y;
        assert!(max >= 5000.0 - 600.0, "End reaches the bottom: {max}");
        key(&mut engine, "Home");
        assert_eq!(engine.scroll_y, 0.0);
    }

    /// The focused field paints a caret: one column of the text colour at the caret's x, gone in the
    /// blink's off phase; a focused field also carries the 2px #101010 focus ring.
    #[test]
    fn focused_field_paints_caret_and_ring() {
        let html = r#"<html><body style="margin:0"><input id="f" autofocus style="margin:10px" value=""></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(200, 60));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let f = dom_by_id(&engine, "f");
        let (x, y, w, h) = engine.box_of(&f).unwrap();
        let px = |e: &crate::AetherEngine, px: u32, py: u32| {
            let i = ((py * e.width + px) * 4) as usize;
            (e.surface[i + 2], e.surface[i + 1], e.surface[i])
        };
        assert_eq!(px(&engine, x as u32, (y + h / 2.0) as u32), (16, 16, 16), "focus ring on the left edge");
        // the caret column: the first dark column inside the content box at mid-height
        let mid = (y + h / 2.0) as u32;
        let dark: Vec<u32> = ((x as u32 + 2)..((x + w) as u32 - 2)).filter(|&cx| px(&engine, cx, mid).0 < 60).collect();
        assert_eq!(dark.len(), 1, "exactly one caret column in an empty field: {dark:?}");
        // off phase: no caret
        engine.caret_on_for_test(false);
        engine.damage_rects.push((0, 0, engine.width, engine.height));
        engine.render_frame();
        let dark = ((x as u32 + 2)..((x + w) as u32 - 2)).filter(|&cx| px(&engine, cx, mid).0 < 60).count();
        assert_eq!(dark, 0, "the blink's off phase paints no caret");
    }
    /// AETHERSCALE (SR190): the header's metrics (`chrome::scale`, the bytes AETHER.ELF links by `#[path]`) at each text
    /// step of the font-size schema — today's heights at and below the default, every band holding the face's line,
    /// the page keeping lines on the 288 px surface, the menu fitted inside the window (AETHERBAR SR192: its grown row
    /// kept and its rows on view counted, the face no longer held); and the first font size whose header would leave
    /// no page line, found and asserted past the schema's 32.
    #[test]
    fn aetherscale_header_reflows_kat() {
        use crate::chrome::scale::{self, Metrics};
        const DEFAULT: i32 = 13;
        const MENU_ROWS: i32 = 10;
        const MENU_PAD: i32 = 4;
        const RING: i32 = 2;
        let at = |css: i32| (scale::face_for(css, DEFAULT), Metrics::at(scale::face_for(css, DEFAULT)));
        assert_eq!(scale::line(scale::FACE_BASE), scale::LINE_BASE, "LINE_BASE is the default face's line");
        let base = |m: &Metrics| (m.tab_h, m.bar_h, m.ctl, m.menu_row, m.status_h);
        for css in [9, DEFAULT] {
            let (_, m) = at(css);
            assert_eq!(base(&m), (26, 30, 22, 22, 14), "css {css}: today's heights");
            assert_eq!(m.chrome_h(), 56, "css {css}: chrome_h");
        }
        // (css, chrome_h, status_h, ctl, page lines at 288)
        let table = [(9, 56, 14, 22, 10), (13, 56, 14, 22, 10), (16, 62, 17, 25, 10), (20, 70, 21, 29, 9), (26, 84, 28, 36, 8), (32, 96, 34, 42, 7)];
        let mut prev: Option<Metrics> = None;
        for &(css, chrome_h, status_h, ctl, lines) in &table {
            let (face, m) = at(css);
            assert_eq!((m.chrome_h(), m.status_h, m.ctl), (chrome_h, status_h, ctl), "css {css}: the grown header");
            if let Some(p) = prev {
                assert!(m.tab_h >= p.tab_h && m.bar_h >= p.bar_h && m.ctl >= p.ctl && m.menu_row >= p.menu_row && m.status_h >= p.status_h, "css {css}: monotone");
            }
            prev = Some(m);
            assert!(m.ctl - 2 * RING >= m.line, "css {css}: the field holds the face (no FONT_MAX)");
            assert!(m.tab_h - 4 >= m.line, "css {css}: a tab holds the face");
            assert!(m.status_h >= m.line, "css {css}: the status line holds the face");
            assert!(m.bar_h >= m.ctl, "css {css}: the bar holds its controls");
            let content = m.content_h(scale::SURFACE_H);
            assert!(content >= scale::PAGE_LINE, "css {css}: a page line left at 288");
            assert_eq!(content / scale::PAGE_LINE, lines, "css {css}: page lines at 288");
            // The menu at a full 288 window, under the top layout's header.
            let mu = m.menu(face, MENU_ROWS, MENU_PAD, m.chrome_h(), scale::SURFACE_H);
            assert!(mu.y >= 2, "css {css}: menu top inside");
            assert!(mu.y + mu.vis * mu.row + 2 * MENU_PAD <= scale::SURFACE_H, "css {css}: menu foot inside");
            assert!(mu.vis >= 1 && mu.vis <= MENU_ROWS && mu.row == m.menu_row, "css {css}: the grown row, its rows on view");
            assert!(scale::line(mu.face) <= mu.row, "css {css}: a menu row holds its face");
            if css == DEFAULT {
                assert_eq!((mu.y, mu.row, mu.face, mu.vis), (58, 22, scale::FACE_BASE, MENU_ROWS), "the default menu is today's");
            }
        }
        // AETHERBAR (SR192): the surface cap is gone — largest (26) keeps its 36 px rows and its own face, 7 rows on
        // view on the 288 window (the menu scrolls).
        let (face, m) = at(26);
        let mu = m.menu(face, MENU_ROWS, MENU_PAD, m.chrome_h(), scale::SURFACE_H);
        assert_eq!((mu.row, mu.vis, mu.y), (36, 7, 26), "largest: the grown row, 7 on view, lifted");
        assert_eq!(mu.face, face, "largest: the menu face is the chrome's");
        // The first font size whose header and status line would leave less than one page line on the surface.
        let first = (9..=200).find(|&css| at(css).1.content_h(scale::SURFACE_H) < scale::PAGE_LINE).unwrap();
        assert_eq!(first, 77, "the header bound sits at css 77");
        assert!(first > 32, "no schema step (9..=32) breaks the header bound");
    }

    /// AETHERBAR (SR192): the toolbar's optional controls by one rule of `ctl` (`scale::Metrics::bar`), the side
    /// column's row grown from the face's line (`sidetabs::row_h`, read by the layout and the hit test), and the menu
    /// (its rows on view and its width bounds) at css 9 / 13 / 32 on 288 x 288 and 1280 x 800 windows.
    #[test]
    fn aetherbar_fit_kat() {
        use crate::chrome::scale::{self, Metrics};
        use crate::chrome::sidetabs::{self, SideHit, SideStrip};
        use crate::chrome::tabs::{Row, TabStrip};
        const DEFAULT: i32 = 13;
        const MENU_PAD: i32 = 4;
        let at = |css: i32| (scale::face_for(css, DEFAULT), Metrics::at(scale::face_for(css, DEFAULT)));
        // M1: the thresholds. (css, bar width, reload, star, zoom): 288 the top layout, 192 / 240 the side column open /
        // railed on 288, 1280 / 1040 the same on the wide window.
        let bars = [
            (9, 288, true, true, false),
            (13, 288, true, true, false),
            (13, 192, true, false, false),
            (13, 1280, true, true, true),
            (32, 288, true, true, false),
            (32, 240, true, false, false),
            (32, 192, false, false, false),
            (32, 1040, true, true, true),
        ];
        for &(css, bw, reload, star, zoom) in &bars {
            let (_, m) = at(css);
            let b = m.bar(bw);
            assert_eq!((b.reload, b.star, b.zoom), (reload, star, zoom), "css {css} bw {bw}: the bar's controls");
            // The omnibox keeps OMNI_MIN beside what the bar carries, whenever Back + Fwd + the menu fit at all.
            let (left, right) = (2 + b.reload as i32, 1 + b.star as i32 + 2 * b.zoom as i32);
            if bw >= scale::bar_need(m.ctl, 2, 1) {
                assert!(bw >= scale::bar_need(m.ctl, left, right), "css {css} bw {bw}: the omnibox overlaps");
            }
        }
        let (_, m) = at(DEFAULT);
        assert_eq!((scale::bar_need(m.ctl, 3, 1), scale::bar_need(m.ctl, 3, 2), scale::bar_need(m.ctl, 3, 4)), (152, 178, 230), "the measured rule at ctl 22");
        let (_, m) = at(32);
        assert_eq!((m.ctl, scale::bar_need(m.ctl, 3, 1), scale::bar_need(m.ctl, 3, 2), scale::bar_need(m.ctl, 3, 4)), (42, 232, 278, 370), "css 32");
        // M2: the side row grows from the face's line; the layout and the hit test read the same row.
        for (css, row) in [(9, 32u32), (13, 32), (32, 52)] {
            let (face, _) = at(css);
            let line = scale::line(face);
            assert_eq!(sidetabs::row_h(line), row, "css {css}: the side row");
            assert!(sidetabs::row_h(line) as i32 >= line, "css {css}: a side row holds the face's line");
        }
        assert_eq!(sidetabs::row_h(0), sidetabs::ROW_H, "line 0 is the default row");
        let mut s = TabStrip::new();
        let ids: Vec<u64> = (0..12).map(|_| s.open()).collect();
        let side = SideStrip { line: scale::line(at(32).0), ..SideStrip::default() };
        let rh = side.row_h();
        assert_eq!(rh, 52);
        let h = 6 * rh;
        let rows = side.layout(&s, h);
        assert_eq!(rows[0].row, Row::Tab(ids[0]));
        for (k, r) in rows.iter().enumerate() {
            assert_eq!(r.y, (k as u32 + 1) as i32 * rh as i32, "row {k} at its grown top");
        }
        assert_eq!(side.hit_w(&s, 240, h, 10, rh as i32 - 1), Some(SideHit::Toggle), "the toggle row is a grown row");
        assert_eq!(side.hit_w(&s, 240, h, 10, rh as i32 + 5), Some(SideHit::Tab(ids[0])));
        assert_eq!(side.hit_w(&s, 240, h, 10, 2 * rh as i32 + 5), Some(SideHit::Tab(ids[1])));
        assert_eq!(side.max_scroll_at(&s, h), (13 - 5) * rh as i32, "13 rows (12 tabs + `+`) in 5 of room");
        assert_eq!(SideStrip::max_scroll(&s, h), (13 * 32 - (h as i32 - 32)).max(0), "the default-face API keeps 32 px rows");
        // M3: the menu at each step on both windows (top layout: the page under the whole header).
        // (css, win_w, win_h, row, vis, y)
        let menus = [
            (9, 288, 288, 22, 10, 58),
            (13, 288, 288, 22, 10, 58),
            (32, 288, 288, 42, 6, 26),
            (9, 1280, 800, 22, 10, 58),
            (13, 1280, 800, 22, 10, 58),
            (32, 1280, 800, 42, 10, 98),
        ];
        for &(css, win_w, win_h, row, vis, y) in &menus {
            let (face, m) = at(css);
            let rows = 10 + !m.bar(win_w).reload as i32;
            let mu = m.menu(face, rows, MENU_PAD, m.chrome_h(), win_h);
            assert_eq!((mu.row, mu.vis, mu.y), (row, vis, y), "css {css} {win_w}x{win_h}: the menu");
            assert_eq!(mu.face, face, "css {css} {win_w}x{win_h}: the face is the chrome's (no cap)");
            assert!(mu.y >= 2 && mu.y + mu.vis * mu.row + 2 * MENU_PAD <= win_h - 2, "css {css} {win_w}x{win_h}: inside");
            assert!(scale::line(mu.face) <= mu.row, "css {css}: a row holds its face");
            assert!(scale::menu_w(10_000, win_w) <= win_w - 2 * scale::BAR_EDGE, "css {css}: the width is bounded by the window");
        }
        assert_eq!(scale::menu_w(0, 288), scale::MENU_W, "an unmeasured menu is SR143's width");
        assert_eq!(scale::menu_w(100, 1280), 176);
        assert_eq!(scale::menu_w(300, 1280), 300, "the widest label sets the width");
        assert_eq!(scale::menu_w(400, 288), 272, "bounded by the window");
        // One row that cannot fit: the row and the face are held (the one cap left).
        let (face, m) = at(32);
        let mu = m.menu(face, 10, MENU_PAD, 0, 40);
        assert_eq!((mu.row, mu.vis), (28, 1));
        assert!(mu.face < face && scale::line(mu.face) <= mu.row);
    }


    /// AETHERFIT2 (SR191): HTML §6.6.3 — an inert node is not a focusable area: under a modal dialog `focus()` on the
    /// blocked document REFUSES (activeElement stays), while a control inside the dialog still takes focus.
    #[test]
    fn test_focus_refuses_an_inert_element() {
        let html = r#"<html><body><input id="outside"><dialog><input id="inside"></dialog>
            <script>document.querySelector('dialog').showModal();</script></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/", html, true);
        let js = |e: &mut crate::AetherEngine, src: &str| e.js_engine.as_mut().unwrap().eval_string(src).unwrap();
        let before = js(&mut engine, "String(document.activeElement && document.activeElement.id)");
        js(&mut engine, "document.getElementById('outside').focus()");
        assert_eq!(js(&mut engine, "String(document.activeElement && document.activeElement.id)"), before, "focus() on an inert element is refused");
        assert_ne!(before, "outside");
        js(&mut engine, "document.getElementById('inside').focus()");
        assert_eq!(js(&mut engine, "document.activeElement.id"), "inside", "a control inside the modal dialog still focuses");
        js(&mut engine, "document.querySelector('dialog').close(); document.getElementById('outside').focus()");
        assert_eq!(js(&mut engine, "document.activeElement.id"), "outside", "with the dialog closed the document focuses again");
    }

    /// AETHERFIT2 (SR191): MouseUp's defaults take the RETARGETED hit (HTML §6.3) — a click on an inert link under a
    /// modal dialog follows no link; without the dialog the same click navigates.
    #[test]
    fn test_mouseup_defaults_take_the_retargeted_hit() {
        let page = |modal: bool| {
            format!(
                r#"<html><head><style>body {{ margin: 0; }} #l {{ display: block; height: 600px; }}</style></head><body>
                <a id="l" href="/next">link</a><dialog><p>modal</p></dialog>
                <script>{}</script></body></html>"#,
                if modal { "document.querySelector('dialog').showModal();" } else { "" }
            )
        };
        for modal in [true, false] {
            let mut engine = crate::AetherEngine::new();
            engine.load_html("https://example.com/", &page(modal), true);
            engine.render_frame();
            engine.handle_event(crate::api::events::Event::MouseDown(5.0, 5.0));
            engine.handle_event(crate::api::events::Event::MouseUp(5.0, 5.0));
            let nav = engine.take_pending_nav();
            if modal {
                assert!(nav.is_none(), "a click on the inert link under a modal dialog stages no navigation");
            } else {
                assert!(nav.is_some(), "without the dialog the click follows the link");
            }
        }
    }

    /// AETHERFIT2 (SR191): CSSOM reads the fixed box's viewport origin and the top layer — a modal dialog's
    /// getBoundingClientRect() does not move with window.scroll, elementFromPoint at its centre is inside it over a
    /// document box, and the root's scrollHeight leaves the fixed box out (its clientHeight is the viewport's).
    #[test]
    fn test_cssom_fixed_rects_hit_the_top_layer() {
        let html = r#"<html><head><style>
            body { margin: 0; }
            #under { height: 3000px; }
            dialog { padding: 0; border: 0; }
            #inner { display: block; width: 100px; height: 40px; }
        </style></head><body><div id="under"></div><dialog><div id="inner"></div></dialog>
        <script>document.querySelector('dialog').showModal();</script></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(800, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let js = |e: &mut crate::AetherEngine, src: &str| e.js_engine.as_mut().unwrap().eval_string(src).unwrap();
        let top0 = js(&mut engine, "String(document.getElementById('inner').getBoundingClientRect().top)");
        let left0 = js(&mut engine, "String(document.getElementById('inner').getBoundingClientRect().left)");
        assert_eq!(top0, "280", "centred in the 600px viewport: (600 - 40) / 2");
        assert_eq!(left0, "350", "centred in the 800px viewport: (800 - 100) / 2");
        assert_eq!(js(&mut engine, "String(document.elementFromPoint(400, 300).id)"), "inner", "the top layer is hit above the document box");
        assert_eq!(js(&mut engine, "String(document.elementFromPoint(5, 5).id)"), "under", "outside the dialog: the document box");
        js(&mut engine, "window.scrollTo(0, 500)");
        assert_eq!(js(&mut engine, "String(window.scrollY)"), "500");
        assert_eq!(js(&mut engine, "String(document.getElementById('inner').getBoundingClientRect().top)"), top0, "a fixed box does not move with the scroll");
        assert_eq!(js(&mut engine, "String(document.getElementById('under').getBoundingClientRect().top)"), "-500", "a document box does");
        assert_eq!(js(&mut engine, "String(document.elementFromPoint(400, 300).id)"), "inner", "still hit at the same viewport point after the scroll");
        assert_eq!(js(&mut engine, "String(document.scrollingElement.scrollHeight)"), "3000", "the document's extent, without the fixed box");
        assert_eq!(js(&mut engine, "String(document.documentElement.clientHeight)"), "600", "the root's clientHeight is the viewport's");
    }

    /// AETHERSUBMITCAPTION (SR410): (ink pixels, ink x-centre, box x-centre) inside the control `id`'s box, inset
    /// past a 2px UA border and its padding: a pixel is ink when any channel is darker than 160 (ButtonFace is
    /// 239, the captions are dark).
    fn caption_ink(engine: &mut crate::AetherEngine, id: &str) -> (usize, f32, f32) {
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
        let (l, t, w, h) = (r(engine, "left"), r(engine, "top"), r(engine, "width"), r(engine, "height"));
        assert!(w > 14.0 && h > 6.0, "{id}: the control has a box ({w}x{h})");
        let sw = engine.width as usize;
        let s = engine.surface();
        let (x0, x1) = ((l + 7.0).ceil() as usize, (l + w - 7.0).floor() as usize);
        let (y0, y1) = ((t + 3.0).ceil() as usize, (t + h - 3.0).floor() as usize);
        let (mut n, mut sx) = (0usize, 0f64);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * sw + x) * 4;
                if s.get(i..i + 3).is_some_and(|p| p.iter().any(|&c| c < 160)) {
                    n += 1;
                    sx += x as f64;
                }
            }
        }
        let cx = if n > 0 { (sx / n as f64) as f32 } else { 0.0 };
        (n, cx, (l + w / 2.0) as f32)
    }

    /// AETHERSUBMITCAPTION (SR410): a submit button paints its caption — the value, the default "Submit" when it
    /// has none, and Google's home-page buttons (Peter's capture: `.lsb` 15px sans-serif inside `.lsbb`, the
    /// sprite url left out) centred in their boxes.
    #[test]
    fn submit_caption_paints() {
        let html = r#"<!doctype html><html><head><style>
            .lsbb{background:#f3f5f6;border:solid 1px;border-color:#d2d2d2 #70757a #70757a #d2d2d2;height:30px}
            .lsbb{display:block}
            .lsb{color:#1f1f1f;border:none;cursor:pointer;height:30px;margin:0;outline:0;font:15px sans-serif;vertical-align:top}
            .ds{display:inline-box;display:inline-block;margin:3px 0 4px;margin-left:4px}
        </style></head><body><form action="/s">
            <input type="submit" id="a" value="Go"> <input type="submit" id="b"><br>
            <span class="ds"><span class="lsbb"><input class="lsb" id="c" value="Google Search" name="btnG" type="submit"></span></span><span class="ds"><span class="lsbb"><input class="lsb" id="d" value="I'm Feeling Lucky" name="btnI" type="submit"></span></span>
        </form></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        for id in ["a", "b", "c", "d"] {
            let (n, cx, bx) = caption_ink(&mut engine, id);
            assert!(n > 0, "{id}: the caption paints glyph ink inside the button");
            if matches!(id, "c" | "d") {
                assert!((cx - bx).abs() <= 3.0, "{id}: the caption is centred (ink centre {cx}, box centre {bx})");
            }
        }
    }

    /// AETHERSUBMITCAPTION (SR410): a `<button>`'s text child paints inside its box.
    #[test]
    fn button_text_paints() {
        let html = r#"<html><body><button id="r">Run</button></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (n, _, _) = caption_ink(&mut engine, "r");
        assert!(n > 0, "the button's text paints glyph ink inside it");
    }

    /// AETHERFILECOLORPAINT (SR456): (the control `id`'s border box as (left, top, width, height), and the ink pixels
    /// in its x band [left + `f0` * width, left + `f1` * width) clamped inside SR410's 7 px / 3 px inset), the same
    /// darker-than-160 ink test as `caption_ink`.
    fn band_ink(engine: &mut crate::AetherEngine, id: &str, f0: f64, f1: f64) -> usize {
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
        let (l, t, w, h) = (r(engine, "left"), r(engine, "top"), r(engine, "width"), r(engine, "height"));
        assert!(w > 14.0 && h > 6.0, "{id}: the control has a box ({w}x{h})");
        let sw = engine.width as usize;
        let s = engine.surface();
        let x0 = (l + (f0 * w).max(7.0)).ceil() as usize;
        let x1 = (l + (f1 * w).min(w - 7.0)).floor() as usize;
        let (y0, y1) = ((t + 3.0).ceil() as usize, (t + h - 3.0).floor() as usize);
        let mut n = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * sw + x) * 4;
                if s.get(i..i + 3).is_some_and(|p| p.iter().any(|&c| c < 160)) {
                    n += 1;
                }
            }
        }
        n
    }

    /// AETHERFILECOLORPAINT (SR456): a file input paints its Choose-File button caption at its start and the "No file
    /// chosen" status after it (Chromium's file control; layout_core `file_label`), with and without `multiple`; a
    /// value attribute paints as the status name (AETHERFILECHOSEN SR682, layout_core `file_status`). Before, the box was an empty ButtonFace frame.
    #[test]
    fn file_caption_paints() {
        let html = r#"<!doctype html><html><body>
            <div><input type="file" id="f"></div>
            <div><input type="file" id="m" multiple value="x.txt"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        for id in ["f", "m"] {
            assert!(band_ink(&mut engine, id, 0.0, 0.3) > 0, "{id}: the button caption paints ink at the box's start");
            assert!(band_ink(&mut engine, id, 0.4, 1.0) > 0, "{id}: the status text paints ink after the button");
        }
    }

    /// AETHERFILEFACE (SR463): a file input paints ButtonFace and the 1px #767676 frame on its Choose-File button only
    /// (Chromium: html.css gives the input no background and no border): the button's top-left corner is the frame
    /// (square, the push rule; before, the input's kept corner), inside it the face, and the input's top-right, right of
    /// the button, is the page background (before, the input's own frame over a whole-box face).
    #[test]
    fn file_face_on_button() {
        let html = r#"<!doctype html><html><body><div><input type="file" id="f"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let px = |e: &mut crate::AetherEngine, x: f64, y: f64| -> (u8, u8, u8) {
            let i = (y as usize * e.width as usize + x as usize) * 4;
            let s = e.surface();
            (s[i + 2], s[i + 1], s[i])
        };
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('f').getBoundingClientRect().{k}"));
        let (l, t, w, h) = (r(&mut engine, "left"), r(&mut engine, "top"), r(&mut engine, "width"), r(&mut engine, "height"));
        assert!(l >= 2.0 && w > 40.0 && h > 14.0, "the file input has a box ({l},{t} {w}x{h})");
        let (x0, y0) = (l.trunc(), t.trunc());
        let bg = px(&mut engine, x0 - 2.0, y0);
        assert_eq!(px(&mut engine, x0, y0), (118, 118, 118), "the button's top-left corner carries the frame colour");
        assert_eq!(px(&mut engine, x0 + 2.0, y0 + 2.0), layout_core::BUTTON_FACE, "the button is filled with ButtonFace");
        assert_eq!(px(&mut engine, (l + w).trunc() - 1.0, y0), bg, "right of the button the input paints no face and no frame");
    }

    /// AETHERFILECOLORPAINT (SR456): a color input paints its value as Chromium's swatch (layout_core `COLOR_SWATCH`: 5 px
    /// / 6 px in from the border box, a 1 px #777777 frame round the fill): the attribute's colour, black with no
    /// value, a script-set (dirty) value over the attribute; never the value as text.
    #[test]
    fn color_swatch_paints() {
        let html = r##"<!doctype html><html><body>
            <div><input type="color" id="r" value="#ff0000"></div>
            <div><input type="color" id="k"></div>
            <div><input type="color" id="g" value="#0000ff"></div>
            <script>document.getElementById('g').value = '#00ff00';</script>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let px = |e: &mut crate::AetherEngine, x: f64, y: f64| -> (u8, u8, u8) {
            let i = (y as usize * e.width as usize + x as usize) * 4;
            let s = e.surface();
            (s[i + 2], s[i + 1], s[i])
        };
        for (id, want) in [("r", (255, 0, 0)), ("k", (0, 0, 0)), ("g", (0, 255, 0))] {
            let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
            let (l, t, w, h) = (r(&mut engine, "left"), r(&mut engine, "top"), r(&mut engine, "width"), r(&mut engine, "height"));
            assert!(w > 20.0 && h > 14.0, "{id}: the control has a box ({w}x{h})");
            let (cx, cy) = ((l + w / 2.0).floor(), (t + h / 2.0).floor());
            assert_eq!(px(&mut engine, cx, cy), want, "{id}: the swatch is filled with the value");
            assert_eq!(px(&mut engine, cx + 6.0, cy), want, "{id}: the fill is solid, not a glyph");
            assert_eq!(px(&mut engine, l.ceil() + 5.0, cy), (119, 119, 119), "{id}: the swatch's 1 px #777 frame, 5 px in");
        }
    }

    /// AETHERSUBMITCAPTION (SR410): with no fontconfig configuration a Mac walks its CoreText font domains (a stock
    /// macOS has no /etc/fonts, so the Linux directories found zero faces); every other OS keeps the four.
    #[test]
    fn font_dirs_cover_macos() {
        let home = Some(PathBuf::from("/Users/p"));
        let mac = crate::fonts::fontconfig::conventional_dirs(true, home.clone());
        assert_eq!(mac, vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts"), PathBuf::from("/Users/p/Library/Fonts")]);
        assert!(!mac.contains(&PathBuf::from("/usr/share/fonts")));
        let other = crate::fonts::fontconfig::conventional_dirs(false, home);
        assert_eq!(
            other,
            vec![
                PathBuf::from("/usr/share/fonts"),
                PathBuf::from("/usr/local/share/fonts"),
                PathBuf::from("/Users/p/.local/share/fonts"),
                PathBuf::from("/Users/p/.fonts"),
            ]
        );
    }

    /// AETHERRADIUSCALC (SR416): a `border-radius: calc(...)` is one radius (css-backgrounds-3 §5.1 with css-values-4
    /// §10), so a 100x100 box rounds its corners by the calc's 20px: each corner pixel outside the radius shows the page
    /// white, the centre and the top edge's middle the box's blue. Before, the calc was cut at its spaces and the whole
    /// declaration dropped (four square corners).
    #[test]
    fn test_aetherradiuscalc_calc_radius_paints() {
        let t = laid_out(
            r#"<html><body><div style="width:100px;height:100px;background:#0000ff;border-radius:calc(10px + 10px)"></div></body></html>"#,
            "body{margin:0}",
        );
        let (w, h) = (100u32, 100u32);
        let mut surface = vec![0u8; (w * h * 4) as usize];
        crate::render::render_frame(&t, &mut surface, w, h, 0.0, 0.0, &[(0, 0, w, h)]);
        let px = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; (surface[i + 2], surface[i + 1], surface[i]) };
        for (x, y) in [(1, 1), (98, 1), (1, 98), (98, 98)] {
            assert_eq!(px(x, y), (255, 255, 255), "corner ({x},{y}) lies outside the 20px calc radius");
        }
        assert_eq!(px(50, 50), (0, 0, 255), "the box's centre");
        assert_eq!(px(50, 1), (0, 0, 255), "the top edge's middle is inside the rounded box");
    }

    /// AETHERCENTERLAYOUT (SR421): google.com's home form (an inline reduction of its markup: `<center>` > `<form>` >
    /// the cellpadding=0 layout table with the 25% / `td align=center nowrap` search field and btnG / btnI submits /
    /// 25% Advanced-search cells) sits centred under `<center>`, as Chrome's `-webkit-center` places it, not at the
    /// left (Peter's Mac run: x 87-787); getComputedStyle names the UA value.
    #[test]
    fn center_form_table_centred() {
        let html = r#"<!doctype html><html><head><style>
            .lsbb{background:#f3f5f6;border:solid 1px;border-color:#d2d2d2 #70757a #70757a #d2d2d2;height:30px;display:block}
            .lsb{color:#1f1f1f;border:none;height:30px;margin:0;outline:0;font:15px sans-serif;vertical-align:top}
            .ds{display:inline-box;display:inline-block;margin:3px 0 4px;margin-left:4px}
        </style></head><body><center id="c"><br clear="all"><div id="lga"><img alt="Google" height="92" width="272" style="padding:28px 0 14px"><br><br></div>
            <form action="/search" name="f"><table id="t" cellpadding="0" cellspacing="0"><tr valign="top"><td width="25%">&nbsp;</td><td align="center" nowrap=""><div class="ds" style="height:32px;margin:4px 0"><input class="lst" style="margin:0;padding:5px 8px 0 6px;vertical-align:top;color:#000" autocomplete="off" value="" title="Google Search" maxlength="2048" name="q" size="57"></div><br style="line-height:0"><span class="ds"><span class="lsbb"><input class="lsb" value="Google Search" name="btnG" type="submit"></span></span><span class="ds"><span class="lsbb"><input class="lsb" value="I'm Feeling Lucky" name="btnI" type="submit"></span></span></td><td class="fl sblc" align="left" nowrap="" width="25%"><a href="/advanced_search?hl=en">Advanced search</a></td></tr></table></form></center></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(1200, 800));
        engine.load_html("https://www.google.com/", html, true);
        engine.render_frame();
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('t').getBoundingClientRect().{k}"));
        let (left, width) = (r(&mut engine, "left"), r(&mut engine, "width"));
        assert!(width > 0.0 && width < 1100.0, "the form table is narrower than the page ({width})");
        assert!(left > 40.0, "the form table is not at the left edge (left {left})");
        assert!((left + width / 2.0 - 600.0).abs() <= 2.0, "the form table is centred (left {left}, width {width})");
        // AETHERCENTERCELL (SR436): the `td align=center` cell holds inline content only (the inline-block search field), which
        // the cell rule leaves alone: the field stays centred in its cell.
        let q = |e: &mut crate::AetherEngine, sel: &str| { let mut b = |k: &str| js_num(e, &format!("document.querySelector('{sel}').getBoundingClientRect().{k}")); (b("left"), b("width")) };
        let ((cl, cw), (ql, qw)) = (q(&mut engine, "td[align=center]"), q(&mut engine, "input[name=q]"));
        assert!(qw > 0.0 && (ql + qw / 2.0 - (cl + cw / 2.0)).abs() <= 2.0, "the search field is centred in its align=center cell (field {ql} + {qw}, cell {cl} + {cw})");
        let ta = engine.js_engine.as_mut().unwrap().eval_string("getComputedStyle(document.getElementById('c')).textAlign").unwrap();
        assert_eq!(ta, "-webkit-center", "<center>'s computed text-align is the UA's -webkit-center");
        // AETHERTEXTALIGNCODES (SR724): align=left is Blink's -webkit-left (code 7), not CSS left.
        assert_eq!(engine.js_engine.as_mut().unwrap().eval_string("getComputedStyle(document.querySelector('td[align=left]')).textAlign").unwrap(), "-webkit-left", "align=left's computed text-align is -webkit-left");
    }

    /// ABSSHRINK (LEDGER SR420): CSS 2.2 §10.3.7 — a `right: 8px` absolutely positioned pill with an auto width is
    /// shrink-to-fit and paints inside the viewport's right edge (the builder's 100% placeholder laid it out as wide
    /// as its containing block at x = -8, SR412's header pill).
    #[test]
    fn abs_right_pill_paints_inside_the_viewport() {
        let html = r#"<html><body style="margin: 0"><div id="pill" style="position: absolute; top: 8px; right: 8px; padding: 2px 8px; background: #ff0000">Sign in</div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('pill').getBoundingClientRect().{k}"));
        let (l, t, w, h) = (r(&mut engine, "left"), r(&mut engine, "top"), r(&mut engine, "width"), r(&mut engine, "height"));
        assert!(w > 16.0 && w < 160.0, "the pill shrinks to its text: w {w}");
        assert!(l > 200.0 && l + w <= 400.0, "and sits inside the viewport's right edge: left {l} w {w}");
        let sw = engine.width as usize;
        let s = engine.surface();
        let red = |x: usize, y: usize| s.get((y * sw + x) * 4..(y * sw + x) * 4 + 3).is_some_and(|p| p[0] < 40 && p[1] < 40 && p[2] > 215);
        let (mx, my) = ((l + 2.0).ceil() as usize, (t + h / 2.0) as usize);
        assert!(red(mx, my), "the pill's background paints inside it at ({mx}, {my})");
        assert!(!(0..100).any(|x| red(x, my)), "and nothing red at the viewport's left (x < 100)");
    }

    /// AETHERHEADERBAND (SR412) M1 instrument, #[ignore]d: renders each headerband fixture at 1810x1280 through
    /// the ordinary load + render_frame path and prints, per blue of the candidate family, the painted extent in the
    /// top 400 rows, whether its four corner pixels are painted, the CSSOM owner at its centre (elementFromPoint), and
    /// each candidate element's rect and computed style. `cargo test -p aether --lib header_band_measure -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn header_band_measure() {
        let fixtures: [(&str, &str, &[&str]); 3] = [
            ("google-home", include_str!("../tests/data/headerband/google-home.html"), &[]),
            ("screenshot-facts", include_str!("../tests/data/headerband/screenshot-facts.html"), &["skip", "acct", "signin", "mngb", "gbar", "lga", "hplogo", "f", "ftab", "q"]),
            ("fixed-nav", include_str!("../tests/data/headerband/fixed-nav.html"), &["nav", "first", "chipbox", "chip"]),
        ];
        let blues: [(&str, (u8, u8, u8)); 4] = [("#1a73e8", (0x1a, 0x73, 0xe8)), ("#4285f4", (0x42, 0x85, 0xf4)), ("#0b57d0", (0x0b, 0x57, 0xd0)), ("#c9d7f1", (0xc9, 0xd7, 0xf1))];
        for (name, html, ids) in fixtures {
            let mut engine = crate::AetherEngine::new();
            engine.handle_event(crate::api::events::Event::Resize(1810, 1280));
            engine.load_html("https://example.com/", html, true);
            engine.render_frame();
            let (w, h) = (engine.width, engine.height);
            let surf = engine.surface().to_vec();
            let px = |x: u32, y: u32| -> (u8, u8, u8) {
                let i = ((y * w + x) * 4) as usize;
                (surf[i + 2], surf[i + 1], surf[i])
            };
            println!("[headerband] fixture={} surface={}x{}", name, w, h);
            for (label, c) in blues {
                let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
                for y in 0..h.min(400) {
                    for x in 0..w {
                        if px(x, y) == c {
                            x0 = x0.min(x);
                            y0 = y0.min(y);
                            x1 = x1.max(x);
                            y1 = y1.max(y);
                            n += 1;
                        }
                    }
                }
                if n == 0 {
                    println!("[headerband] {} blue={} none", name, label);
                    continue;
                }
                let corner = |x: u32, y: u32| if px(x, y) == c { 1 } else { 0 };
                let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
                let js = |e: &mut crate::AetherEngine, src: &str| e.js_engine.as_mut().unwrap().eval_string(src).unwrap_or_else(|err| format!("ERR {}", err));
                let owner = js(&mut engine, &format!(
                    "(function(){{var e=document.elementFromPoint({},{}),o=[];for(var i=0;e&&i<4;i++,e=e.parentElement){{var r=e.getBoundingClientRect(),s=getComputedStyle(e);\
                     o.push(e.tagName+'#'+e.id+'.'+String(e.className).split(' ').join('.')+' rect='+r.left+','+r.top+','+r.width+'x'+r.height+' display='+s.display+' position='+s.position+\
                     ' top='+s.top+' right='+s.right+' width='+s.width+' height='+s.height+' overflow='+s.overflow+' radius='+s.borderRadius+' bg='+s.backgroundColor);}}return o.join(' | ');}})()",
                    cx, cy
                ));
                println!(
                    "[headerband] {} blue={} box=x{}..{} y{}..{} px={} corners(tl,tr,br,bl)={},{},{},{} owner@{},{}={}",
                    name, label, x0, x1, y0, y1, n, corner(x0, y0), corner(x1, y0), corner(x1, y1), corner(x0, y1), cx, cy, owner
                );
            }
            // Any blue-dominant colour (the band's hex may be none of the four): the five most painted, with extents.
            let mut hist: std::collections::HashMap<(u8, u8, u8), (u32, u32, u32, u32, u32)> = std::collections::HashMap::new();
            for y in 0..h.min(400) {
                for x in 0..w {
                    let (r, g, b) = px(x, y);
                    if b as i32 > r as i32 + 40 && b as i32 > g as i32 + 15 {
                        let e = hist.entry((r, g, b)).or_insert((0, u32::MAX, u32::MAX, 0, 0));
                        e.0 += 1;
                        e.1 = e.1.min(x);
                        e.2 = e.2.min(y);
                        e.3 = e.3.max(x);
                        e.4 = e.4.max(y);
                    }
                }
            }
            let mut top: Vec<_> = hist.into_iter().collect();
            top.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            for ((r, g, b), (n, x0, y0, x1, y1)) in top.into_iter().take(5) {
                println!("[headerband] {} blueish=#{:02x}{:02x}{:02x} px={} box=x{}..{} y{}..{}", name, r, g, b, n, x0, x1, y0, y1);
            }
            for id in ids {
                let src = format!(
                    "(function(){{var e=document.getElementById('{}');if(!e)return 'missing';var r=e.getBoundingClientRect();var s=getComputedStyle(e);\
                     return 'rect='+r.left+','+r.top+','+r.width+'x'+r.height+' display='+s.display+' position='+s.position+' top='+s.top+' left='+s.left+' right='+s.right+\
                     ' width='+s.width+' height='+s.height+' overflow='+s.overflow+' radius='+s.borderRadius+' margin-top='+s.marginTop+' bg='+s.backgroundColor;}})()",
                    id
                );
                let out = engine.js_engine.as_mut().unwrap().eval_string(&src).unwrap_or_else(|err| format!("ERR {}", err));
                println!("[headerband] {} #{} {}", name, id, out);
            }
            let rects = engine.js_engine.as_mut().unwrap().eval_string(
                "(function(){var b=document.body.getBoundingClientRect(),h=document.documentElement.getBoundingClientRect();return 'body='+b.left+','+b.top+','+b.width+'x'+b.height+' html='+h.left+','+h.top+','+h.width+'x'+h.height;})()",
            ).unwrap_or_else(|err| format!("ERR {}", err));
            println!("[headerband] {} {}", name, rects);
        }
    }

    // ---- AETHERHEADERBAND (SR412): header_band_paints_in_its_box ----
    /// The Mac's empty font database, in-process: no face for any selection (text_host's process-global DB is never
    /// touched). Only the four required `layout_core::Fonts` methods; the rest are the trait's defaults.
    struct HeaderBandNoFaces;
    impl layout_core::Fonts for HeaderBandNoFaces {
        fn has_face(&self, _sel: layout_core::FontSel) -> bool {
            false
        }
        fn measure(&self, _run: &str, _adv: &layout_core::Adv) -> f32 {
            0.0
        }
        fn line_metrics(&self, _sel: layout_core::FontSel, size: f32) -> (f32, f32, f32) {
            (size * 0.8, size * 0.2, 0.0)
        }
        fn x_height(&self, _sel: layout_core::FontSel, size: f32) -> f32 {
            size * 0.5
        }
    }

    /// The `#pill` box of a laid tree: (x, y, w, h) through `layout_core::scroll::box_origin` and the taffy size.
    fn header_band_pill_box(tree: &crate::layout::LayoutTree) -> (f32, f32, f32, f32) {
        let mut work = vec![tree.root_node];
        while let Some(t) = work.pop() {
            let is_pill = tree.node_map.get(&t).and_then(|n| n.as_element().map(|e| e.attributes.borrow().get("id").map(|v| v == "pill").unwrap_or(false))).unwrap_or(false);
            if is_pill {
                let (x, y) = layout_core::scroll::box_origin(tree, t, (0.0, 0.0)).expect("the pill has a box");
                let sz = tree.taffy.layout(t).unwrap().size;
                return (x, y, sz.width, sz.height);
            }
            work.extend(tree.taffy.children(t).unwrap_or_default());
        }
        panic!("no #pill box");
    }

    /// The pill's painted extent equals its layout box, and all four corner radii are applied. `surf` is BGRA.
    fn header_band_check(what: &str, surf: &[u8], w: u32, h: u32, b: (f32, f32, f32, f32)) {
        let blue = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            (surf[i + 2], surf[i + 1], surf[i]) == (0x0b, 0x57, 0xd0)
        };
        let (bx, by, bw, bh) = b;
        assert!(bx >= 0.0 && bx + bw <= w as f32 && by >= 0.0, "{what}: the pill's box {b:?} lies inside the {w}px viewport");
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..h.min(200) {
            for x in 0..w {
                if blue(x, y) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        assert!(x0 != u32::MAX, "{what}: the pill paints");
        let near = |a: u32, e: f32| (a as f32 - e).abs() <= 1.0;
        assert!(
            near(x0, bx) && near(x1 + 1, bx + bw) && near(y0, by) && near(y1 + 1, by + bh),
            "{what}: painted x{x0}..{x1} y{y0}..{y1} equals the layout box {b:?}"
        );
        let (l, t, r, btm) = (bx.round() as u32, by.round() as u32, (bx + bw).round() as u32 - 1, (by + bh).round() as u32 - 1);
        for (cx, cy) in [(l, t), (r, t), (r, btm), (l, btm)] {
            assert!(!blue(cx, cy), "{what}: corner ({cx},{cy}) is outside its radius");
        }
        let (mx, my) = ((l + r) / 2, (t + btm) / 2);
        for (ex, ey) in [(mx, t + 1), (r - 2, my), (mx, btm - 1), (l + 2, my)] {
            assert!(blue(ex, ey), "{what}: edge midpoint ({ex},{ey}) is painted");
        }
    }

    /// AETHERHEADERBAND (SR412): Peter's Mac (zero font faces) painted google.com's Sign-in pill as a band across the
    /// top, clipped at y=0 and rounded only at its right end: `measure_text_run` gave a faceless run the whole
    /// available width (the max-content probe's 1810 per run) while the IFC lays and the painter draws it at 0. The
    /// reduced case is google's header row; laid with no faces the pill is its padding (46 px) inside the viewport,
    /// painted in its box with four rounded corners; the real fonts are the control.
    #[test]
    fn header_band_paints_in_its_box() {
        let html = r##"<html><head><style>
            body { margin: 0; font-family: arial, sans-serif; font-size: 16px }
            #row { display: flex; justify-content: flex-end; align-items: center; height: 48px; padding: 6px }
            #row a { color: #000; padding: 0 8px; text-decoration: none }
            #row #pill { background: #0b57d0; color: #fff; border-radius: 100px; padding: 9px 23px }
        </style></head><body><div id="row"><a href="#">Gmail</a> <a href="#">Images</a> <a id="pill" href="#">Sign in</a></div></body></html>"##;
        let (vw, vh) = (1810u32, 1280u32);
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(vw, vh));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        // Control: the real fonts.
        let real = header_band_pill_box(engine.layout_tree.as_ref().expect("laid out"));
        header_band_check("real fonts", engine.surface(), vw, vh, real);
        // The Mac's condition: the same document laid with no faces and painted.
        let doc = engine.document.clone().expect("a document");
        let mut tree = crate::layout::build_tree(&doc, vw as f32, vh as f32);
        crate::css::apply_stylesheets(&mut tree, &engine.stylesheets); // the page's cascade, as the nested-frame paint does
        layout_core::remeasure(&mut tree, &HeaderBandNoFaces);
        let b = header_band_pill_box(&tree);
        assert!((b.2 - 46.0).abs() < 0.5, "no faces: the pill is its padding, 46 px wide, not the available width (got {b:?})");
        let mut buf = vec![255u8; (vw * vh * 4) as usize];
        crate::render::render_frame(&tree, &mut buf, vw, vh, 0.0, 0.0, &[(0, 0, vw, vh)]);
        header_band_check("no faces", &buf, vw, vh, b);
    }

    /// AETHERCSSSPLITAUDIT (SR426): `background: rgb(0 0 0 / 50%)` is one colour layer at half alpha — the slash
    /// inside the function is not the shorthand's position / size separator — so a 100x100 box over the white page
    /// paints mid grey, and its paint entry carries no bogus position ("0 0") or size ("50%)").
    #[test]
    fn test_aethercsssplitaudit_slash_alpha_background_paints() {
        let t = laid_out(
            r#"<html><body><div id="h" style="width:100px;height:100px;background:rgb(0 0 0 / 50%)"></div></body></html>"#,
            "body{margin:0}",
        );
        let p = t
            .node_map
            .iter()
            .find_map(|(id, n)| {
                let el = n.as_element()?;
                (el.attributes.borrow().get("id") == Some("h")).then(|| t.paint_map.get(id))?
            })
            .expect("the box has a paint entry");
        assert_eq!(p.bg_position.as_deref(), Some("0% 0%"), "no position from the colour's inner slash (SR1061: the reset's initial)");
        assert_eq!(p.bg_size.as_deref(), Some("auto"), "no size from the colour's inner slash (SR1061: the reset's initial)");
        let (w, h) = (100u32, 100u32);
        let mut surface = vec![0u8; (w * h * 4) as usize];
        crate::render::render_frame(&t, &mut surface, w, h, 0.0, 0.0, &[(0, 0, w, h)]);
        let i = ((50 * w + 50) * 4) as usize;
        for c in [surface[i], surface[i + 1], surface[i + 2]] {
            assert!((126..=129).contains(&c), "half-alpha black over white is mid grey, got {c}");
        }
    }

    // ---- AETHERABSCB (SR431): abs_cb_pill_and_icb_probe ----
    /// CSS 2.2 §10.1.4 on the SR412 fixtures at 1810x1280: google-home's Sign-in pill (a static inline-block in a
    /// `float: right` row under `#gb { position: relative }`, no absolute box above it) keeps SR412's with-fonts
    /// place, right edge 1783 (the float's edge, font-independent); screenshot-facts' `#acct` (`position: absolute;
    /// top: 6px; right: 8px` under a static `#mngb`) sits against the initial containing block, right 1802, top 6.
    #[test]
    fn abs_cb_pill_and_icb_probe() {
        let rect = |html: &str, sel: &str| -> (f64, f64) {
            let mut engine = crate::AetherEngine::new();
            engine.handle_event(crate::api::events::Event::Resize(1810, 1280));
            engine.load_html("https://example.com/", html, true);
            engine.render_frame();
            let src = format!("(function(){{var r=document.querySelector('{}').getBoundingClientRect();return r.right+','+r.top;}})()", sel);
            let out = engine.js_engine.as_mut().unwrap().eval_string(&src).unwrap_or_else(|e| format!("ERR {}", e));
            let mut it = out.split(',').map(|v| v.trim().parse::<f64>().unwrap_or(f64::NAN));
            (it.next().unwrap_or(f64::NAN), it.next().unwrap_or(f64::NAN))
        };
        let (right, _) = rect(include_str!("../tests/data/headerband/google-home.html"), "a.gb_6a");
        assert!((right - 1783.0).abs() <= 1.5, "google-home's Sign-in pill keeps SR412's right edge 1783: got {right}");
        let (right, top) = rect(include_str!("../tests/data/headerband/screenshot-facts.html"), "#acct");
        assert!((right - 1802.0).abs() <= 0.5 && (top - 6.0).abs() <= 0.5, "#acct against the initial containing block at right 1802 top 6: got {right}, {top}");
    }

    /// AETHERPUSHTRIM (SR440): a push button's padded / mixed-case type (`type=" Submit "`) is the same push button as
    /// its plain twin: layout_core's ONE `is_push_type` (trimmed, as `control_intrinsic` sized it) drives the host
    /// paint's push arm, so the padded box is as wide as the twin's and paints the same centred caption ink (before,
    /// the untrimmed host predicate painted it as a text field: a left-aligned value, and nothing for a reset button
    /// without one).
    #[test]
    fn push_trim_caption_paints() {
        assert!(layout_core::is_push_type(" SUBMIT ") && layout_core::is_push_type("reset") && layout_core::is_push_type("\tButton\n"));
        assert!(!layout_core::is_push_type(" image ") && !layout_core::is_push_type("text") && !layout_core::is_push_type(""));
        let html = r#"<!doctype html><html><body>
            <input type=" Submit " id="ps" value="Go"><br><input type="submit" id="ts" value="Go"><br>
            <input type=" reset " id="pr"><br><input type="reset" id="tr">
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        for (padded, twin) in [("ps", "ts"), ("pr", "tr")] {
            let w = |e: &mut crate::AetherEngine, id: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().width"));
            let (pw, tw) = (w(&mut engine, padded), w(&mut engine, twin));
            assert!((pw - tw).abs() <= 0.5, "{padded}: the padded type sizes as its twin ({pw} vs {tw})");
            let (pn, pcx, pbx) = caption_ink(&mut engine, padded);
            let (tn, tcx, tbx) = caption_ink(&mut engine, twin);
            assert!(pn > 0 && tn > 0, "{padded}: both paint caption ink ({pn}, {tn})");
            assert!((pn as f64 - tn as f64).abs() <= 0.15 * tn as f64 && ((pcx - pbx) - (tcx - tbx)).abs() <= 1.0, "{padded}: the padded type paints its twin's caption (ink {pn} vs {tn}, offset {} vs {})", pcx - pbx, tcx - tbx);
        }
    }

    /// AETHERIMAGEINPUTFETCH (SR461): an `<input type=image>` rides the img path (layout_core's ONE `is_image_input`): its
    /// data: src is looked up and blitted as an img's, its box is the decoded size, or its width / height attributes
    /// (the bitmap scaled into it, no value text over it); with no image it paints its AltText (`image_alt_label`).
    #[test]
    fn image_input_paints_src() {
        use base64::Engine as _;
        let blue = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        blue.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let src = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.get_ref()));
        let html = format!(
            r#"<!doctype html><html><body>
            <div><input type="image" id="n" src="{src}"></div>
            <div><input type="image" id="s" width="40" height="30" value="Go" src="{src}"></div>
            <div><input type="image" id="a" alt="Search the web"></div>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        let rect = |e: &mut crate::AetherEngine, id: &str| -> (f64, f64, f64, f64) {
            let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
            (r(e, "left"), r(e, "top"), r(e, "width"), r(e, "height"))
        };
        let px = |e: &mut crate::AetherEngine, x: usize, y: usize| -> (u8, u8, u8) {
            let i = (y * e.width as usize + x) * 4;
            let s = e.surface();
            (s[i + 2], s[i + 1], s[i])
        };
        let (l, t, w, h) = rect(&mut engine, "n");
        assert!((w - 8.0).abs() <= 0.5 && (h - 8.0).abs() <= 0.5, "n: the box is the decoded 8 x 8 ({w}x{h})");
        assert_eq!(px(&mut engine, (l + 4.0) as usize, (t + 4.0) as usize), (0, 0, 255), "n: the bitmap paints");
        let (l, t, w, h) = rect(&mut engine, "s");
        assert!((w - 40.0).abs() <= 0.5 && (h - 30.0).abs() <= 0.5, "s: width / height attributes size the box ({w}x{h})");
        for y in (t.ceil() as usize)..((t + h).floor() as usize) {
            for x in (l.ceil() as usize)..((l + w).floor() as usize) {
                assert_eq!(px(&mut engine, x, y), (0, 0, 255), "s: every box pixel is the scaled bitmap, no value text ({x},{y})");
            }
        }
        let (n, _, _) = caption_ink(&mut engine, "a");
        assert!(n > 0, "a: an image input with no image paints its alt text");
    }

    // ---- AETHERFILECHOSEN (LEDGER SR682): a file input's status text from its value (named tail block) ----

    /// AETHERFILECHOSEN (SR682): the file input `id`'s border box (left, top, width, height), the column one past its
    /// button's right edge (the end of the button's #767676 top frame row, SR463), and the sorted columns from there to
    /// the box's right edge that carry ink darker than 160 (the `caption_ink` test) in the rows 3 px inside the box.
    fn file_status_ink(engine: &mut crate::AetherEngine, id: &str) -> ((f64, f64, f64, f64), usize, Vec<usize>) {
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
        let (l, t, w, h) = (r(engine, "left"), r(engine, "top"), r(engine, "width"), r(engine, "height"));
        assert!(w > 40.0 && h > 14.0, "{id}: the file input has a box ({w}x{h})");
        let sw = engine.width as usize;
        let s = engine.surface();
        let px = |x: usize, y: usize| -> (u8, u8, u8) {
            let i = (y * sw + x) * 4;
            (s[i + 2], s[i + 1], s[i])
        };
        let (x0, y0, x1) = (l.trunc() as usize, t.trunc() as usize, ((l + w).floor() as usize).min(sw));
        let mut end = x0;
        while end < x1 && px(end, y0) == (118, 118, 118) {
            end += 1;
        }
        let (ya, yb) = ((t + 3.0).ceil() as usize, (t + h - 3.0).floor() as usize);
        let cols = (end..x1)
            .filter(|&x| (ya..yb).any(|y| {
                let p = px(x, y);
                p.0 < 160 || p.1 < 160 || p.2 < 160
            }))
            .collect();
        ((l, t, w, h), end, cols)
    }

    /// AETHERFILECHOSEN (SR682): with no value the status is layout_core `file_status`'s "No file chosen", painted 4 px
    /// (kAfterButtonSpacing) after the button's border box: the columns just right of the button carry no ink, the
    /// status's first glyph starts within a few px after the gap.
    #[test]
    fn file_status_no_file_chosen_painted() {
        let html = r#"<!doctype html><html><body><div><input type="file" id="f"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let ((l, _, _, _), end, cols) = file_status_ink(&mut engine, "f");
        assert!(end as f64 > l + 20.0, "the button's top frame row was found ({l} .. {end})");
        assert!(!cols.iter().any(|&x| x < end + 3), "the 4 px gap after the button carries no ink (button end {end}): {cols:?}");
        assert!(cols.iter().any(|&x| x >= end + 3 && x <= end + 30), "the status's first glyph follows the gap (button end {end}): {cols:?}");
        assert_eq!(layout_core::file_status(None, false, 1), "No file chosen");
    }

    /// AETHERFILECHOSEN (SR682): a chosen file shows its basename: value `C:\fakepath\report.pdf` paints the same status
    /// ink extent as value `report.pdf`, and a different one from no value ("No file chosen").
    #[test]
    fn file_status_shows_basename() {
        let html = r#"<!doctype html><html><body>
            <div><input type="file" id="p" value="C:\fakepath\report.pdf"></div>
            <div><input type="file" id="b" value="report.pdf"></div>
            <div><input type="file" id="n"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let extent = |e: &mut crate::AetherEngine, id: &str| -> (usize, usize) {
            let (_, end, cols) = file_status_ink(e, id);
            assert!(!cols.is_empty(), "{id}: the status paints ink");
            (cols[0] - end, cols[cols.len() - 1] - end)
        };
        let (p, b, n) = (extent(&mut engine, "p"), extent(&mut engine, "b"), extent(&mut engine, "n"));
        assert_eq!(p, b, "the fakepath value paints its basename");
        assert_ne!(p, n, "a chosen name is not the No-file-chosen text");
        assert_eq!(layout_core::file_status(Some("C:\\fakepath\\report.pdf"), false, 1), "report.pdf");
    }

    /// AETHERFILECHOSEN (SR682): a status wider than the box is cut at the input's content right edge with a trailing
    /// `…` (the render's `pdf::thumbs::ellipsize`): a 60-char name in a 227 px box keeps its ink inside the box, reaches
    /// near its right edge, and its last glyph is a short dot band in the line's lower half (the ellipsis), not a
    /// clipped letter.
    #[test]
    fn file_status_ellipsized() {
        let name = "Quarterly_Budget_Report_Final_Draft_Version_Seven_2026_A.pdf";
        assert_eq!(name.len(), 60);
        let html = format!(
            r#"<!doctype html><html><body><div><input type="file" id="e" style="width: 227px" value="C:\fakepath\{name}"></div></body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 200));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        let ((l, t, w, h), end, cols) = file_status_ink(&mut engine, "e");
        assert!((w - 227.0).abs() <= 0.5, "the author width holds the box at 227 px ({w})");
        let last = *cols.last().expect("the status paints ink");
        assert!((last as f64) < l + w, "the status stays inside the box ({last} vs {})", l + w);
        assert!(last as f64 >= l + w - 20.0, "the cut status runs to near the box's right edge ({last} vs {})", l + w);
        let sw = engine.width as usize;
        let s = engine.surface();
        let ink = |x: usize, y: usize| {
            let i = (y * sw + x) * 4;
            s[i] < 160 || s[i + 1] < 160 || s[i + 2] < 160
        };
        let (ya, yb) = ((t + 3.0).ceil() as usize, (t + h - 3.0).floor() as usize);
        let rows = |xa: usize, xb: usize| -> Vec<usize> { (ya..yb).filter(|&y| (xa..=xb).any(|x| ink(x, y))).collect() };
        let all = rows(end, last);
        let dot = rows(last.saturating_sub(1), last);
        let (top, bottom) = (all[0], all[all.len() - 1]);
        assert!(bottom - top >= 7, "the status line has letter-height ink ({top}..{bottom})");
        assert!(!dot.is_empty() && dot[dot.len() - 1] - dot[0] <= 3, "the last glyph is a dot, not a clipped letter: {dot:?}");
        assert!(dot[0] * 2 >= top + bottom, "the dot sits in the line's lower half: {dot:?} in {top}..{bottom}");
    }

    // ---- AETHERIMGALT (LEDGER SR681): a failed img's alt text (named tail block) --------------------------------

    /// AETHERIMGALT (SR681): the border box of `id` and the dark-ink count over [x0, x1) x [y0, y1) of the surface.
    fn imgalt_rect(e: &mut crate::AetherEngine, id: &str) -> (f64, f64, f64, f64) {
        let r = |e: &mut crate::AetherEngine, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
        (r(e, "left"), r(e, "top"), r(e, "width"), r(e, "height"))
    }

    fn imgalt_ink(e: &crate::AetherEngine, x0: usize, x1: usize, y0: usize, y1: usize) -> usize {
        let sw = e.width as usize;
        let s = e.surface();
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * sw + x) * 4;
                if s.get(i..i + 3).is_some_and(|p| p.iter().all(|&c| c < 128)) {
                    n += 1;
                }
            }
        }
        n
    }

    fn imgalt_px(e: &crate::AetherEngine, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * e.width as usize + x) * 4;
        let s = e.surface();
        (s[i + 2], s[i + 1], s[i])
    }

    /// AETHERIMGALT (SR681): a failed img with a non-empty alt and no size is its alt text laid out inline (Chromium's
    /// HTMLImageFallbackHelper: a non-replaced phrasing element whose content is the text): the box is the text's
    /// width, the text paints. In standards mode a SIZED one is inline text too (AdjustHostStyle drops the size).
    #[test]
    fn img_alt_inline_when_unsized_and_failed() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="i" src="missing.png" alt="Hello"></div>
            <div><span id="s">Hello</span></div>
            <div><img id="z" src="missing.png" alt="Hello" width="200" height="100"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (_, _, sw, _) = imgalt_rect(&mut engine, "s");
        let (l, t, w, h) = imgalt_rect(&mut engine, "i");
        assert!(sw > 10.0 && (w - sw - layout_core::INLINE_ICON_ADVANCE as f64).abs() <= 1.0, "i: the box is the alt text's width + the icon ({w} vs the span's {sw})"); // AETHERIMGALTINLINEICON (SR726): the icon before the text
        assert!(h > 8.0 && h < 40.0, "i: one line tall ({h})");
        let ink = imgalt_ink(&engine, l.floor() as usize, (l + w).ceil() as usize, t.floor() as usize, (t + h).ceil() as usize);
        assert!(ink > 0, "i: the alt text paints");
        let (_, _, zw, zh) = imgalt_rect(&mut engine, "z");
        assert!((zw - sw - layout_core::INLINE_ICON_ADVANCE as f64).abs() <= 1.0 && zh < 40.0, "z: a standards-mode sized img with alt is inline text, not 200x100 ({zw}x{zh})");
    }

    /// AETHERIMGALT (SR681): in quirks mode a sized failed img is a box of its size (TreatImageAsReplaced) with the 1px
    /// solid silver border, 1px padding, the 16px broken-image icon (SR702), and the alt text after it on the first line.
    #[test]
    fn img_alt_boxed_when_sized() {
        let html = r#"<html><body style="margin:0;font:16px sans-serif">
            <div><img id="b" src="missing.png" alt="Hi" width="200" height="100"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (l, t, w, h) = imgalt_rect(&mut engine, "b");
        assert!((w - 200.0).abs() <= 0.5 && (h - 100.0).abs() <= 0.5, "b: the box keeps its 200x100 ({w}x{h})");
        let (l, t) = (l.round() as usize, t.round() as usize);
        let silver = (192, 192, 192);
        assert_eq!(imgalt_px(&engine, l, t + 50), silver, "b: the left border");
        assert_eq!(imgalt_px(&engine, l + 199, t + 50), silver, "b: the right border");
        assert_eq!(imgalt_px(&engine, l + 100, t), silver, "b: the top border");
        assert_eq!(imgalt_px(&engine, l + 100, t + 99), silver, "b: the bottom border");
        assert_eq!(imgalt_px(&engine, l + 100, t + 70), (255, 255, 255), "b: the inside is not filled");
        assert!(imgalt_ink(&engine, l + 18, l + 199, t + 2, t + 30) > 0, "b: the alt text paints after the icon slot");
        assert!(imgalt_painted(&engine, l + 2, l + 18, t + 2, t + 18) > 0, "b: the broken-image icon paints in its slot");
        assert_eq!(imgalt_px(&engine, l + 3, t + 7), (128, 128, 128), "b: the icon's page outline");
    }

    /// AETHERIMGALT (SR681): a failed, unsized img with alt="" represents nothing: no box, no ink.
    #[test]
    fn img_empty_alt_paints_nothing() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="e" src="missing.png" alt=""></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (_, _, w, _) = imgalt_rect(&mut engine, "e");
        assert!(w <= 0.5, "e: an empty alt takes no width ({w})");
        assert_eq!(imgalt_ink(&engine, 0, 400, 0, 60), 0, "e: nothing paints");
    }

    /// AETHERIMGALT (SR681): a `loading=lazy` img not yet fetched is pending, not failed: it keeps its attribute size
    /// and paints no alt text and no border (a file page, so the lazy pass starts no fetch).
    #[test]
    fn img_pending_paints_nothing_yet() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="p" src="x.png" loading="lazy" alt="Hello" width="200" height="100"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("file:///imgalt.html", html, true);
        engine.render_frame();
        let (l, t, w, h) = imgalt_rect(&mut engine, "p");
        assert!((w - 200.0).abs() <= 0.5 && (h - 100.0).abs() <= 0.5, "p: the attributes size the pending box ({w}x{h})");
        let (l, t) = (l.round() as usize, t.round() as usize);
        assert_eq!(imgalt_px(&engine, l, t + 50), (255, 255, 255), "p: no border");
        assert_eq!(imgalt_ink(&engine, l, l + 200, t, t + 100), 0, "p: no alt text yet");
    }

    // ---- AETHERHOSTWGHT (LEDGER SR690): font-weight on a variable face paints its instance (named tail block) ----

    /// AETHERHOSTWGHT (SR690): the face the installed-family roads give (`fonts::styled_face`, what `db_face` loads)
    /// for the tree's Inter variable face at 600 is the wght 600 instance, not the 400 one: a distinct face, no
    /// synthetic bold, more ink in `H`; at 700 (where `select` asks for synthetic bold) the axis reaches the weight and
    /// no embolden path is taken. Through `styled_face` rather than a page: the process-wide font database holds no
    /// installed variable face on the seat.
    #[test]
    fn font_weight_600_paints_the_instance() {
        use crate::fonts::{db, raster};
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fonts/inter/InterVariable.ttf");
        let info = db::faces_in_file(&p).into_iter().next().expect("the tree's Inter variable face");
        let at = |w: f32, bold: bool| {
            let want = db::Style { weight: w, ..db::Style::default() };
            crate::fonts::styled_face(&info, bold, false, want).expect("Inter loads")
        };
        let (regular, semibold, bold) = (at(400.0, false), at(600.0, false), at(700.0, true));
        assert_ne!(regular.id, semibold.id, "600 is its own instance");
        assert!(!regular.synth_bold && !semibold.synth_bold && !bold.synth_bold, "no embolden path on a wght face");
        let gid = regular.font.glyph_index('H');
        let ink = |f: &crate::fonts::Face| raster::glyph(f, gid, 32.0, 0).map_or(0u64, |m| m.data.iter().map(|&v| v as u64).sum());
        let (r, s, b) = (ink(regular), ink(semibold), ink(bold));
        assert!(r < s && s < b, "ink grows with the instanced weight: 400 {r}, 600 {s}, 700 {b}");
    }

    // ---- AETHERCAPTIONCSSOM (LEDGER SR686): getComputedStyle(caption).captionSide (named tail block) ----

    /// AETHERCAPTIONCSSOM (SR686): `getComputedStyle(el).captionSide` reports the used value layout_core folded, as
    /// Chromium does: style="caption-side: bottom" and the align=bottom presentation hint read bottom, a bare caption
    /// reads the initial top, and an author rule on the table is inherited by its bare caption.
    #[test]
    fn computed_caption_side_reports_used_value() {
        let html = r#"<!doctype html><html><head><style>#t2 { caption-side: bottom }</style></head><body>
            <table><caption id="s" style="caption-side: bottom">S</caption><tr><td>1</td></tr></table>
            <table><caption id="a" align="bottom">A</caption><tr><td>2</td></tr></table>
            <table><caption id="n">N</caption><tr><td>3</td></tr></table>
            <table id="t2"><caption id="i">I</caption><tr><td>4</td></tr></table>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(800, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        for (id, want) in [("s", "bottom"), ("a", "bottom"), ("n", "top"), ("i", "bottom")] {
            let got = engine
                .js_engine
                .as_mut()
                .unwrap()
                .eval_string(&format!("getComputedStyle(document.getElementById('{id}')).captionSide"))
                .unwrap();
            assert_eq!(got, want, "#{id}: computed caption-side");
        }
    }

    /// AETHERIMGBROKENICON (SR702): the broken-image icon shows only when the box has room for it (Chromium's
    /// ImageSmallerThanAltImage, 16 + 2 px): a 40x40 failed img with alt="" paints the icon (layout_core's
    /// `BROKEN_ICON`) in its slot; a 12x12 one paints nothing (no frame, no icon, no text).
    #[test]
    fn img_alt_broken_icon_only_when_room() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="r" src="missing.png" alt="" width="40" height="40"></div>
            <div><img id="n" src="missing.png" alt="" width="12" height="12"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (l, t, w, h) = imgalt_rect(&mut engine, "r");
        assert!((w - 40.0).abs() <= 0.5 && (h - 40.0).abs() <= 0.5, "r: the box keeps its 40x40 ({w}x{h})");
        let (l, t) = (l.round() as usize, t.round() as usize);
        assert!(imgalt_painted(&engine, l + 2, l + 18, t + 2, t + 18) > 0, "r: the icon paints");
        assert_eq!(imgalt_px(&engine, l + 3, t + 7), (128, 128, 128), "r: the icon's page outline");
        let (l, t, w, h) = imgalt_rect(&mut engine, "n");
        assert!((w - 12.0).abs() <= 0.5 && (h - 12.0).abs() <= 0.5, "n: the box keeps its 12x12 ({w}x{h})");
        let (l, t) = (l.round() as usize, t.round() as usize);
        assert_eq!(imgalt_painted(&engine, l, l + 12, t, t + 12), 0, "n: too small for the icon: nothing paints");
    }

    /// AETHERIMGBROKENICON (SR702): the pixels in (x0..x1, y0..y1) that are not the white page (`imgalt_ink` counts
    /// only dark text ink; the icon's grey, blue and green are not dark).
    fn imgalt_painted(e: &crate::AetherEngine, x0: usize, x1: usize, y0: usize, y1: usize) -> usize {
        (y0..y1).map(|y| (x0..x1).filter(|&x| imgalt_px(e, x, y) != (255, 255, 255)).count()).sum()
    }

    // ---- AETHERIMGALTINLINEWRAP (LEDGER SR707): an InlineAlt img's text wraps in a narrow column (named tail block) ----

    /// AETHERIMGALTINLINEWRAP (SR707): a failed, unsized img whose 20-word alt is far wider than its 120px column wraps
    /// as text does: its box is no wider than the column and several lines tall, the second line carries ink, and nothing
    /// paints right of the column. (SR681's short alt keeps its one line: `img_alt_inline_when_unsized_and_failed`.)
    #[test]
    fn img_alt_inline_wraps_in_a_narrow_column() {
        let alt = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
        let html = format!(
            r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div style="width:120px"><img id="w" src="missing.png" alt="{alt}"></div>
            <div><span id="s">Hello</span></div>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        let (_, _, _, sh) = imgalt_rect(&mut engine, "s");
        let (l, t, w, h) = imgalt_rect(&mut engine, "w");
        assert!(w > 20.0 && w <= 120.5, "w: the box is no wider than the column ({w})");
        assert!(sh > 8.0 && h > sh * 1.5, "w: more than one line tall ({h} vs a line's {sh})");
        let (x0, x1) = (l.floor() as usize, (l + w).ceil() as usize);
        let ink2 = imgalt_ink(&engine, x0, x1, (t + sh).floor() as usize, (t + 2.0 * sh).ceil() as usize);
        assert!(ink2 > 0, "w: the second line paints");
        assert_eq!(imgalt_ink(&engine, 122, 400, t.floor() as usize, (t + h).ceil() as usize), 0, "w: nothing paints right of the column");
    }

    // ---- AETHERFILECHOSENJSFILES (LEDGER SR706): the script-set FileList feeds the file status and value (named tail block) ----

    /// AETHERFILECHOSENJSFILES (SR706): a page that sets `input.files` from a DataTransfer (SR93's setter).
    fn file_chosen_jsfiles_page() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body>
            <div><input type="file" id="m2" multiple></div>
            <div><input type="file" id="v2" multiple value="2 files"></div>
            <div><input type="file" id="s2"></div>
            <div><input type="file" id="v2s" value="2 files"></div>
            <div><input type="file" id="o" value="old-name.txt"></div>
            <div><input type="file" id="r" value="report.pdf"></div>
            <div><input type="file" id="on" value="old-name.txt"></div>
            <div><input type="file" id="n"></div>
            <script>
              function list(names) { var d = new DataTransfer(); names.forEach(function (n) { d.items.add(new File(["x"], n)); }); return d.files; }
              document.getElementById("m2").files = list(["a.txt", "b.txt"]);
              document.getElementById("s2").files = list(["a.txt", "b.txt"]);
              document.getElementById("o").files = list(["report.pdf"]);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFILECHOSENJSFILES (SR706): the painted status reads the FileList first: two Files paint `2 files` (the same
    /// status ink extent as the stand-in `value="2 files"` beside the same button) on a multiple input and on a non-multiple one (Blink's
    /// FileStatusText goes by the list's length), one File `report.pdf` over `value="old-name.txt"` paints report.pdf's
    /// extent, and an input with no FileList keeps SR682's attribute stand-in (old-name.txt, not No file chosen).
    #[test]
    fn file_chosen_jsfiles_paints_filelist() {
        let mut engine = file_chosen_jsfiles_page();
        let extent = |e: &mut crate::AetherEngine, id: &str| -> (usize, usize) {
            let (_, end, cols) = file_status_ink(e, id);
            assert!(!cols.is_empty(), "{id}: the status paints ink");
            (cols[0] - end, cols[cols.len() - 1] - end)
        };
        let (m2, v2, s2, v2s) = (extent(&mut engine, "m2"), extent(&mut engine, "v2"), extent(&mut engine, "s2"), extent(&mut engine, "v2s"));
        let (o, r, on, n) = (extent(&mut engine, "o"), extent(&mut engine, "r"), extent(&mut engine, "on"), extent(&mut engine, "n"));
        assert_eq!(m2, v2, "two Files on a multiple input paint `2 files`");
        assert_eq!(s2, v2s, "two Files on a non-multiple input paint `2 files` (beside the same Choose File button)");
        assert_eq!(o, r, "the FileList's one name paints over the value attribute");
        assert_ne!(o, on, "the value attribute is not painted once a FileList is set");
        assert_ne!(on, n, "an input with no FileList keeps the attribute stand-in");
    }

    /// AETHERFILECHOSENJSFILES (SR706): `input.value` of a file input reads `C:\fakepath\` and the first selected file's
    /// name (HTML filename mode, as Chromium), and the empty string with no FileList (the attribute is not the value).
    #[test]
    fn file_chosen_jsfiles_value_fakepath() {
        let mut engine = file_chosen_jsfiles_page();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        assert_eq!(js.eval_string("document.getElementById('o').value").unwrap(), "C:\\fakepath\\report.pdf");
        assert_eq!(js.eval_string("document.getElementById('m2').value").unwrap(), "C:\\fakepath\\a.txt");
        assert_eq!(js.eval_string("document.getElementById('on').value").unwrap(), "");
        assert_eq!(js.eval_string("String(document.getElementById('m2').files.length)").unwrap(), "2");
    }

    // ---- AETHERIMGALTICONONLY (LEDGER SR714): an unsized failed img with no alt is the icon alone (named tail block) ----

    /// AETHERIMGALTICONONLY (SR714): a failed, unsized img with a src and no alt or title shows Chromium's broken-image
    /// icon ALONE (HTMLImageFallbackHelper's represents-text arm): a 16x16 box, the icon at its (0, 0) (the page outline
    /// grey at icon pixel (1, 5)), no silver frame, nothing right of it; an img with neither src nor alt represents
    /// nothing. (alt="" stays nothing: `img_empty_alt_paints_nothing`.)
    #[test]
    fn img_alt_icon_only_when_no_alt() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="o" src="missing.png"></div>
            <div><img id="x"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (l, t, w, h) = imgalt_rect(&mut engine, "o");
        assert!((w - 16.0).abs() <= 0.5 && (h - 16.0).abs() <= 0.5, "o: the icon box is 16x16 ({w}x{h})");
        let (l, t) = (l.round() as usize, t.round() as usize);
        assert!(imgalt_painted(&engine, l, l + 16, t, t + 16) > 0, "o: the icon paints");
        assert_eq!(imgalt_px(&engine, l + 1, t + 5), (128, 128, 128), "o: the icon's page outline at the box's (1, 5)");
        assert_eq!(imgalt_px(&engine, l, t), (255, 255, 255), "o: no frame");
        assert_eq!(imgalt_painted(&engine, l + 16, 400, t, t + 16), 0, "o: nothing paints right of the icon");
        let (_, _, xw, _) = imgalt_rect(&mut engine, "x");
        assert!(xw <= 0.5, "x: no src and no alt takes no width ({xw})");
    }

    // ---- AETHERHOSTWGHTCOVERFACE (LEDGER SR725): the covering face draws a variable fallback at the asked weight (named tail block) ----

    /// AETHERHOSTWGHTCOVERFACE (SR725): the face `fonts::fallback_for` returns when the style-matched face lacks the
    /// character, and `fonts::stack`'s last resort, load through `text_host::cover_face`: for the tree's Inter variable
    /// face at 700 that is the wght 700 instance (the pre-SR725 roads loaded the default one), a distinct face with no
    /// synthetic bold and more ink in `H`; at 400 it is the `load_face` object itself. Through the helper rather than a
    /// page: the process-wide font database holds no installed variable face on the seat (SR690's reason).
    #[test]
    fn fallback_cover_face_instances() {
        use crate::fonts::{db, raster};
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fonts/inter/InterVariable.ttf");
        let info = db::faces_in_file(&p).into_iter().next().expect("the tree's Inter variable face");
        let at = |w: f32| text_host::cover_face(&info, db::Style { weight: w, ..db::Style::default() }).expect("Inter loads");
        let (regular, bold) = (at(400.0), at(700.0));
        assert!(std::ptr::eq(regular, crate::fonts::load_face(&info).unwrap()), "400 is today's object");
        assert_ne!(regular.id, bold.id, "700 is its own instance");
        assert!(bold.font.is_instanced() && !bold.synth_bold, "the axis reaches 700: no embolden path");
        let gid = regular.font.glyph_index('H');
        let ink = |f: &crate::fonts::Face| raster::glyph(f, gid, 32.0, 0).map_or(0u64, |m| m.data.iter().map(|&v| v as u64).sum());
        assert!(ink(regular) < ink(bold), "the covering face inks at the asked weight");
    }

    // ---- AETHERIMGALTINLINEICON (LEDGER SR726): the broken-image icon before inline alt text (named tail block) ----

    /// AETHERIMGALTINLINEICON (SR726): a failed, unsized img with alt="Hello" shows Chromium's 16x16 broken-image icon
    /// before its text (HTMLImageFallbackHelper's represents-text arm: the icon floats left with margin 0): the box is
    /// the span's width + 16, the icon's page outline grey sits at the box's (1, 5), no text ink lies under the icon and
    /// the text paints after it; in a 120px column only the first line is indented (line 2 carries ink left of x + 16).
    #[test]
    fn img_alt_inline_icon_before_text() {
        let alt = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
        let html = format!(
            r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><img id="i" src="missing.png" alt="Hello"></div>
            <div><span id="s">Hello</span></div>
            <div style="width:120px"><img id="w" src="missing.png" alt="{alt}"></div>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        let adv = layout_core::INLINE_ICON_ADVANCE as f64;
        let (_, _, sw, sh) = imgalt_rect(&mut engine, "s");
        let (l, t, w, h) = imgalt_rect(&mut engine, "i");
        assert!(sw > 10.0 && (w - sw - adv).abs() <= 1.0, "i: the box is the text's width + the icon ({w} vs {sw} + {adv})");
        let (li, ti) = (l.round() as usize, t.round() as usize);
        assert!(imgalt_painted(&engine, li, li + 16, ti, ti + 16) > 0, "i: the icon paints");
        assert_eq!(imgalt_px(&engine, li + 1, ti + 5), (128, 128, 128), "i: the icon's page outline at the box's (1, 5)");
        let (y0, y1) = (t.floor() as usize, (t + h).ceil() as usize);
        assert_eq!(imgalt_ink(&engine, li, li + 16, y0, y1), 0, "i: no text under the icon");
        assert!(imgalt_ink(&engine, li + 16, (l + w).ceil() as usize, y0, y1) > 0, "i: the text paints after the icon");
        let (l, t, w, h) = imgalt_rect(&mut engine, "w");
        assert!(w <= 120.5 && h > sh * 1.5, "w: wraps inside the column ({w}x{h})");
        let lw = l.round() as usize;
        assert_eq!(imgalt_ink(&engine, lw, lw + 16, t.floor() as usize, (t + sh).floor() as usize), 0, "w: line 1 starts after the icon");
        assert!(imgalt_ink(&engine, lw, lw + 16, (t + sh).ceil() as usize, (t + 2.0 * sh).floor() as usize) > 0, "w: line 2 is not indented");
    }

    // ---- AETHERIMGALTBASELINE (LEDGER SR730): an InlineAlt img sits on the line's text baseline (named tail block) ----

    /// AETHERIMGALTBASELINE (SR730): a failed, unsized img with alt="Hello" beside a span "Hello" on one line: Chromium
    /// lays the alt as ordinary inline text, so both words paint on ONE baseline (the bottom ink row of each "Hello", which
    /// has no descender, is the same row), the img box's top is the span's top, and the line is one text line tall (the
    /// replaced-element bottom-edge rule put the alt's baseline a descent above the span's and grew the line by it).
    #[test]
    fn img_alt_inline_on_the_text_baseline() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div id="d"><span id="s">Hello</span> <img id="i" src="missing.png" alt="Hello"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (sl, st, sw, sh) = imgalt_rect(&mut engine, "s");
        let (il, it, iw, ih) = imgalt_rect(&mut engine, "i");
        let (_, _, _, dh) = imgalt_rect(&mut engine, "d");
        assert!(sw > 10.0 && iw > sw && il >= sl + sw, "the img follows the span on the line ({sl}+{sw} then {il}+{iw})");
        assert!((it - st).abs() <= 2.0, "the img box's top is the span's top ({it} vs {st})");
        assert!(dh < 24.0 && dh <= sh.max(ih) + 1.0, "one text line tall ({dh} vs span {sh} / img {ih})");
        let y1 = (st.max(it) + sh.max(ih)).ceil() as usize + 4;
        let bottom_ink = |e: &crate::AetherEngine, x0: usize, x1: usize| (0..y1).rev().find(|&y| imgalt_ink(e, x0, x1, y, y + 1) > 0);
        let sb = bottom_ink(&engine, sl.floor() as usize, (sl + sw).ceil() as usize).expect("the span paints");
        let ix0 = (il + layout_core::INLINE_ICON_ADVANCE as f64).ceil() as usize + 1;
        let ib = bottom_ink(&engine, ix0, (il + iw).ceil() as usize).expect("the img's alt paints");
        assert!((sb as i64 - ib as i64).abs() <= 1, "both Hellos sit on one baseline (bottom ink rows {sb} vs {ib})");
    }

    // ---- AETHERFILEVALUECLEAR (LEDGER SR715): `input.value = ""` and the form reset empty a file input's FileList (named tail block) ----

    /// AETHERFILEVALUECLEAR (SR715): a form holding file input `in`, and outside it `out`, `cl`, `th` and a fresh `fresh`
    /// (all multiple); every one but `fresh` is given two Files through SR706's DataTransfer helper.
    fn file_value_clear_page() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body>
            <form id="fm"><div><input type="file" id="in" multiple></div></form>
            <div><input type="file" id="out" multiple></div>
            <div><input type="file" id="cl" multiple></div>
            <div><input type="file" id="th" multiple></div>
            <div><input type="file" id="fresh" multiple></div>
            <script>
              function list(names) { var d = new DataTransfer(); names.forEach(function (n) { d.items.add(new File(["x"], n)); }); return d.files; }
              ["in", "out", "cl", "th"].forEach(function (id) { document.getElementById(id).files = list(["a.txt", "b.txt"]); });
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFILEVALUECLEAR (SR715): the file input `id`'s status ink extent from its button's right edge (SR682's probe).
    fn file_value_clear_extent(engine: &mut crate::AetherEngine, id: &str) -> (usize, usize) {
        let (_, end, cols) = file_status_ink(engine, id);
        assert!(!cols.is_empty(), "{id}: the status paints ink");
        (cols[0] - end, cols[cols.len() - 1] - end)
    }

    /// AETHERFILEVALUECLEAR (SR715): `input.value = ""` empties the selected files (HTML §4.10.5.1.18): the FileList held
    /// before the clear reads length 0 and loses its index, `value` reads "", and the painted status returns to a fresh
    /// input's `No files chosen`.
    #[test]
    fn file_value_clear_empties_list() {
        let mut engine = file_value_clear_page();
        let before = file_value_clear_extent(&mut engine, "cl");
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { var e = document.getElementById("cl"), k = e.files, n = k.length; e.value = ""; return [n, k.length, e.files.length, k[0] === undefined, k === e.files, JSON.stringify(e.value)].join(","); })()"#)
            .unwrap();
        assert_eq!(got, "2,0,0,true,true,\"\"");
        engine.tick(); engine.render_frame();
        let (cl, fresh) = (file_value_clear_extent(&mut engine, "cl"), file_value_clear_extent(&mut engine, "fresh"));
        assert_ne!(before, fresh, "two Files painted `2 files` before the clear");
        assert_eq!(cl, fresh, "the cleared input paints a fresh input's status");
    }

    /// AETHERFILEVALUECLEAR (SR715): any other string throws an InvalidStateError DOMException and leaves the selection.
    #[test]
    fn file_value_clear_nonempty_throws() {
        let mut engine = file_value_clear_page();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { var e = document.getElementById("th"), r; try { e.value = "x"; r = "no throw"; } catch (x) { r = x.name + "," + (x instanceof DOMException); } return r + "," + e.files.length; })()"#)
            .unwrap();
        assert_eq!(got, "InvalidStateError,true,2");
        assert_eq!(js.eval_string("document.getElementById('th').value").unwrap(), "C:\\fakepath\\a.txt");
    }

    /// AETHERFILEVALUECLEAR (SR715): the form reset algorithm empties the list of a file input in the form (its status
    /// paints a fresh input's) and leaves a file input outside the form.
    #[test]
    fn file_value_clear_form_reset() {
        let mut engine = file_value_clear_page();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { document.getElementById("fm").reset(); return document.getElementById("in").files.length + "," + document.getElementById("out").files.length; })()"#)
            .unwrap();
        assert_eq!(got, "0,2");
        engine.tick(); engine.render_frame();
        let (inn, fresh) = (file_value_clear_extent(&mut engine, "in"), file_value_clear_extent(&mut engine, "fresh"));
        assert_eq!(inn, fresh, "the reset input paints a fresh input's status");
    }

    // ---- AETHERIMGALTVALIGN (LEDGER SR748): an InlineAlt img's vertical-align reads its alt run's line box (named tail block) ----

    /// AETHERIMGALTVALIGN (SR748): the (top, bottom) dark-ink rows in columns x0..x1 above row y1; None when nothing paints.
    fn imgalt_ink_rows(e: &crate::AetherEngine, x0: usize, x1: usize, y1: usize) -> Option<(usize, usize)> {
        let top = (0..y1).find(|&y| imgalt_ink(e, x0, x1, y, y + 1) > 0)?;
        let bottom = (0..y1).rev().find(|&y| imgalt_ink(e, x0, x1, y, y + 1) > 0)?;
        Some((top, bottom))
    }

    /// AETHERIMGALTVALIGN (SR748): Chromium lays an InlineAlt img's alt inside a non-replaced inline box, so its
    /// `vertical-align` places the alt text exactly as the same keyword places a span "Hello" (blink ApplyBaselineShift:
    /// middle / text-top / text-bottom read the box's own font metrics plus leading, sub / super the parent's size, a
    /// length is a length). The img carries `padding: 8px 0 16px`, which the replaced-box arms read (middle 4 px, text-top
    /// 8 px, text-bottom 16 px off), so only the alt-run line box passes: each target's ink rows minus its own line's
    /// reference span's are the span engine's within 1, and `5px` lifts the alt 5 rows above its baseline placement.
    #[test]
    fn img_alt_valign_as_a_span() {
        let rows = |target: &str| -> (i64, i64) {
            let html = format!(
                r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div><span id="r">Hello</span> {target}</div>
        </body></html>"#
            );
            let mut engine = crate::AetherEngine::new();
            engine.handle_event(crate::api::events::Event::Resize(400, 300));
            engine.load_html("https://example.com/", &html, true);
            engine.render_frame();
            let (rl, _, rw, _) = imgalt_rect(&mut engine, "r");
            let (tl, _, tw, _) = imgalt_rect(&mut engine, "t");
            let skip = if target.starts_with("<img") { layout_core::INLINE_ICON_ADVANCE as f64 + 1.0 } else { 0.0 };
            let (rt, rb) = imgalt_ink_rows(&engine, rl.floor() as usize, (rl + rw).ceil() as usize, 300).expect("the reference span paints");
            let (tt, tb) = imgalt_ink_rows(&engine, (tl + skip).ceil() as usize, (tl + tw).ceil() as usize, 300).expect("the target paints");
            (tt as i64 - rt as i64, tb as i64 - rb as i64)
        };
        let mut img_baseline = (0, 0);
        for kw in ["baseline", "middle", "text-top", "text-bottom", "sub", "5px"] {
            let img = rows(&format!(r#"<img id="t" src="missing.png" alt="Hello" style="vertical-align:{kw};padding:8px 0 16px">"#));
            let span = rows(&format!(r#"<span id="t" style="vertical-align:{kw}">Hello</span>"#));
            assert!(
                (img.0 - span.0).abs() <= 1 && (img.1 - span.1).abs() <= 1,
                "{kw}: the alt's ink rows sit where a span's do (img top/bottom {:?} vs span {:?}, each from its reference span)",
                img,
                span
            );
            if kw == "baseline" {
                img_baseline = img;
            }
            if kw == "5px" {
                assert!((img.1 - (img_baseline.1 - 5)).abs() <= 1, "5px lifts the alt 5 rows ({:?} vs baseline {:?})", img, img_baseline);
            }
        }
    }

    /// AETHERIMGALTVALIGN (SR748): a quirks-mode sized failed img stays a replaced box (BoxedAlt, SR681), so `middle`
    /// keeps the margin-box rule: the 200x100 box's vertical centre is half the x-height above the line's text baseline
    /// (the reference "Hello"'s bottom ink row + 1), within [bottom ink row - 8, bottom ink row + 1].
    #[test]
    fn img_alt_valign_boxed_middle_keeps_the_replaced_rule() {
        let html = r#"<html><body style="margin:0;font:16px sans-serif">
            <div><span id="r">Hello</span> <img id="b" src="missing.png" alt="Hi" width="200" height="100" style="vertical-align:middle"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let (rl, _, rw, _) = imgalt_rect(&mut engine, "r");
        let (_, bt, _, bh) = imgalt_rect(&mut engine, "b");
        assert!((bh - 100.0).abs() <= 0.5, "b: the box keeps its 100 px ({bh})");
        let (_, rb) = imgalt_ink_rows(&engine, rl.floor() as usize, (rl + rw).ceil() as usize, 300).expect("the reference span paints");
        let mid = bt + bh / 2.0;
        assert!(mid >= rb as f64 - 8.0 && mid <= rb as f64 + 1.0, "b: centred half an x-height above the baseline (centre {mid}, reference bottom ink row {rb})");
    }

    // ---- AETHERFONTSYNTHESISPROPERTY (LEDGER SR750): font-synthesis gates the synthesized face on every road (named tail block) ----

    /// AETHERFONTSYNTHESISPROPERTY (SR750): a page whose only face is the tree's static regular fixture (fontbidi_synth,
    /// usWeightClass 400, roman) as an `@font-face` family, so 700 and italic are synthesized (the webfont road), one
    /// 48px row per case; returns the engine and each row's pixels (the whole surface width, rows y..y+48).
    fn font_synthesis_page() -> (crate::AetherEngine, std::collections::HashMap<&'static str, Vec<u8>>) {
        let font = std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf"))
            .expect("the tree's fontbidi_synth fixture");
        let html = format!(
            r#"<!doctype html><html><head><style>
              @font-face {{ font-family: SynthFix; src: url(file://{}); }}
              div {{ margin: 0; height: 48px; line-height: 48px; font-family: SynthFix; font-size: 32px; white-space: nowrap }}
            </style></head><body style="margin:0">
              <div id="r400">HHHH</div>
              <div id="b700" style="font-weight:700">HHHH</div>
              <div id="n700" style="font-weight:700;font-synthesis:none">HHHH</div>
              <div id="w700" style="font-weight:700;font-synthesis:weight">HHHH</div>
              <div id="i400" style="font-style:italic">HHHH</div>
              <div id="iw" style="font-style:italic;font-synthesis:weight">HHHH</div>
              <div id="is700" style="font-style:italic;font-weight:700;font-synthesis:style">HHHH</div>
              <div id="lw" style="font-weight:700;font-synthesis-weight:none">HHHH</div>
              <section id="par" style="font-synthesis:none"><div id="inh"><span id="kid" style="font-weight:700">HHHH</span></div></section>
            </body></html>"#,
            font.display()
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        let mut rows = std::collections::HashMap::new();
        for id in ["r400", "b700", "n700", "w700", "i400", "iw", "is700", "lw", "inh"] {
            let top = js_num(&mut engine, &format!("document.getElementById('{id}').getBoundingClientRect().top")).round() as usize;
            let sw = engine.width as usize * 4;
            rows.insert(id, engine.surface()[top * sw..(top + 48) * sw].to_vec());
        }
        (engine, rows)
    }

    /// AETHERFONTSYNTHESISPROPERTY (SR750): css-fonts-4 §6 as Chromium's `SyntheticBoldAllowed` / `SyntheticItalicAllowed`:
    /// the default synthesizes (700 inks other than 400, italic other than upright); `none` paints the 400 row byte for
    /// byte; `weight` keeps the bold and drops the oblique; `style` keeps the oblique and drops the bold; the
    /// `font-synthesis-weight` longhand alone drops the bold; and the property inherits (a 700 span under a
    /// `font-synthesis:none` ancestor paints the 400 row).
    #[test]
    fn font_synthesis_gates_the_synthesized_face() {
        let (_, r) = font_synthesis_page();
        assert!(r["r400"].chunks_exact(4).any(|p| p[0] < 128), "the fixture face paints ink");
        assert_ne!(r["b700"], r["r400"], "default: 700 is synthetic bold");
        assert_ne!(r["i400"], r["r400"], "default: italic is synthetic oblique");
        assert_eq!(r["n700"], r["r400"], "font-synthesis:none: 700 paints the regular face");
        assert_eq!(r["w700"], r["b700"], "font-synthesis:weight: the bold stays");
        assert_eq!(r["iw"], r["r400"], "font-synthesis:weight: no oblique");
        assert_eq!(r["is700"], r["i400"], "font-synthesis:style: the oblique stays, no bold");
        assert_eq!(r["lw"], r["r400"], "font-synthesis-weight:none alone drops the bold");
        assert_eq!(r["inh"], r["r400"], "font-synthesis inherits to a child span");
    }

    /// AETHERFONTSYNTHESISPROPERTY (SR750): getComputedStyle reads the initial `weight style`, a declared value, and the
    /// value inherited from an ancestor.
    #[test]
    fn font_synthesis_computed_style() {
        let (mut engine, _) = font_synthesis_page();
        for (id, want) in [("r400", "weight style small-caps"), ("n700", "none"), ("w700", "weight"), ("kid", "none")] {
            let got = engine
                .js_engine
                .as_mut()
                .unwrap()
                .eval_string(&format!("getComputedStyle(document.getElementById('{id}')).getPropertyValue('font-synthesis')"))
                .unwrap();
            assert_eq!(got, want, "#{id}: computed font-synthesis");
        }
    }

    /// AETHERFONTSYNTHESISPROPERTY (SR750): the installed road through the gate: the static fixture's synthetic-bold
    /// object gated with `NO_SYNTH_WEIGHT` is the very `load_face` object (text_host's loaded-face cache, hinting kept),
    /// the default mask and a style-only mask return it unchanged, and an oblique+bold face gated on weight is the
    /// cache's oblique-only object.
    #[test]
    fn font_synthesis_gate_installed_twin() {
        use crate::fonts::db;
        use layout_core::paint::{NO_SYNTH_STYLE, NO_SYNTH_WEIGHT};
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unaos/libs/text/font_core/tests/data/fontbidi_synth.ttf");
        let info = db::faces_in_file(&p).into_iter().next().expect("the tree's fontbidi_synth fixture");
        let bold = crate::fonts::load_face_synth(&info, true, false).unwrap();
        let plain = crate::fonts::load_face(&info).unwrap();
        assert!(std::ptr::eq(crate::fonts::synthesis_gate(bold, 0), bold), "mask 0 moves nothing");
        assert!(std::ptr::eq(crate::fonts::synthesis_gate(bold, NO_SYNTH_STYLE), bold), "a style-only mask keeps the bold");
        let g = crate::fonts::synthesis_gate(bold, NO_SYNTH_WEIGHT);
        assert!(std::ptr::eq(g, plain), "none: the unsynthesized loaded object");
        assert!(text_host::is_installed(g));
        let bo = crate::fonts::load_face_synth(&info, true, true).unwrap();
        let o = crate::fonts::synthesis_gate(bo, NO_SYNTH_WEIGHT);
        assert!(!o.synth_bold && o.synth_oblique && text_host::is_installed(o));
        assert!(std::ptr::eq(o, crate::fonts::load_face_synth(&info, false, true).unwrap()), "the cache's oblique-only object");
    }

    // ---- AETHERFONTSYNTHESISCSSOM (LEDGER SR764): the font-synthesis shorthand expands and serializes canonically (named tail block) ----

    /// AETHERFONTSYNTHESISCSSOM (SR764): a page for the CSSOM checks (no font needed: nothing here reads pixels).
    fn font_synth_cssom_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body>
              <div id="a"></div>
              <div id="lw" style="font-synthesis-weight:none"></div>
              <div id="ls" style="font-synthesis-style:none"></div>
              <div id="sw" style="font-synthesis:style weight"></div>
              <section style="font-synthesis-weight:none"><div id="kid" style="font-synthesis-style:none"></div><div id="kid2"></div></section>
            </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn font_synth_eval(engine: &mut crate::AetherEngine, expr: &str) -> String {
        engine.js_engine.as_mut().unwrap().eval_string(expr).unwrap()
    }

    /// AETHERFONTSYNTHESISCSSOM (SR764): `el.style.fontSynthesis` expands into its two longhands and serializes from
    /// them in css-fonts-4 / Chromium canonical order; `small-caps` sets its own longhand (SR772); an invalid value leaves the
    /// declaration; a CSS-wide keyword reaches both longhands; cssText recomposes the shorthand.
    #[test]
    fn font_synthesis_cssom_expands_and_serializes() {
        let mut e = font_synth_cssom_engine();
        let set = |e: &mut crate::AetherEngine, v: &str| {
            font_synth_eval(e, &format!("var s = document.getElementById('a').style; s.fontSynthesis = '{v}'; [s.fontSynthesis, s.fontSynthesisWeight, s.fontSynthesisStyle].join('|')"))
        };
        assert_eq!(set(&mut e, "style weight"), "weight style|auto|auto");
        assert_eq!(set(&mut e, "weight style"), "weight style|auto|auto");
        assert_eq!(set(&mut e, "none"), "none|none|none");
        assert_eq!(set(&mut e, "weight"), "weight|auto|none");
        assert_eq!(set(&mut e, "style"), "style|none|auto");
        assert_eq!(set(&mut e, "small-caps"), "small-caps|none|none", "small-caps sets font-synthesis-small-caps (SR772)");
        assert_eq!(set(&mut e, "weight small-caps"), "weight small-caps|auto|none");
        assert_eq!(set(&mut e, "bold"), "weight small-caps|auto|none", "an invalid value leaves the declaration");
        assert_eq!(set(&mut e, "weight weight"), "weight small-caps|auto|none", "a repeated keyword is invalid");
        assert_eq!(set(&mut e, "inherit"), "inherit|inherit|inherit");
        let r = font_synth_eval(
            &mut e,
            "var s = document.getElementById('a').style; s.fontSynthesis = 'style weight'; s.fontSynthesisStyle = 'none'; \
             var a = [s.fontSynthesis, s.cssText]; s.fontSynthesisWeight = 'bogus'; a.push(s.fontSynthesisWeight); \
             s.setProperty('font-synthesis-weight', 'none'); a.push(s.getPropertyValue('font-synthesis')); \
             s.removeProperty('font-synthesis'); a.push(s.fontSynthesisWeight + '/' + s.cssText); a.join('|')",
        );
        assert_eq!(r, "weight|font-synthesis: weight;|auto|none|/");
    }

    /// AETHERFONTSYNTHESISCSSOM (SR764): getComputedStyle recomposes `font-synthesis` from its longhands' computed
    /// values: the initial `weight style small-caps` (SR772), a shorthand set in reversed order, a longhand alone (`style` / `weight`, not
    /// the parent's shorthand), each longhand inherited on its own, and a value set through `el.style`.
    #[test]
    fn font_synthesis_cssom_computed_style() {
        let mut e = font_synth_cssom_engine();
        let cs = |e: &mut crate::AetherEngine, id: &str, p: &str| {
            font_synth_eval(e, &format!("getComputedStyle(document.getElementById('{id}')).getPropertyValue('{p}')"))
        };
        for (id, want) in [("a", "weight style small-caps"), ("sw", "weight style"), ("lw", "style small-caps"), ("ls", "weight small-caps"), ("kid", "small-caps"), ("kid2", "style small-caps")] {
            assert_eq!(cs(&mut e, id, "font-synthesis"), want, "#{id}: computed font-synthesis");
        }
        assert_eq!(cs(&mut e, "lw", "font-synthesis-weight"), "none");
        assert_eq!(cs(&mut e, "lw", "font-synthesis-style"), "auto");
        assert_eq!(cs(&mut e, "kid2", "font-synthesis-weight"), "none", "the longhand inherits");
        font_synth_eval(&mut e, "document.getElementById('a').style.fontSynthesis = 'none'; ''");
        assert_eq!(cs(&mut e, "a", "font-synthesis"), "none");
        font_synth_eval(&mut e, "document.getElementById('a').style.fontSynthesisWeight = 'auto'; ''");
        assert_eq!(cs(&mut e, "a", "font-synthesis"), "weight");
        assert_eq!(font_synth_eval(&mut e, "getComputedStyle(document.getElementById('sw')).fontSynthesis"), "weight style");
    }

    // ---- AETHERFONTSYNTHESISSMALLCAPS (LEDGER SR772): the font-synthesis-small-caps longhand and the small-caps keyword (named tail block) ----

    /// AETHERFONTSYNTHESISSMALLCAPS (SR772): layout_core's ONE parse gives the small-caps half: the shorthand's keyword
    /// (omitted = none), `initial` / `none` all three, the longhand its own half, an invalid value nothing.
    #[test]
    fn font_synthesis_smallcaps_parse() {
        use layout_core::css::parse_font_synthesis as p;
        assert_eq!(p("font-synthesis", "small-caps"), Some((Some(false), Some(false), Some(true))));
        assert_eq!(p("font-synthesis", "weight style"), Some((Some(true), Some(true), Some(false))));
        assert_eq!(p("font-synthesis", "initial"), Some((Some(true), Some(true), Some(true))));
        assert_eq!(p("font-synthesis", "none"), Some((Some(false), Some(false), Some(false))));
        assert_eq!(p("font-synthesis-small-caps", "none"), Some((None, None, Some(false))));
        assert_eq!(p("font-synthesis-small-caps", "auto"), Some((None, None, Some(true))));
        assert_eq!(p("font-synthesis-small-caps", "bogus"), None);
        assert_eq!(p("font-synthesis-weight", "none"), Some((Some(false), None, None)));
    }

    /// AETHERFONTSYNTHESISSMALLCAPS (SR772): `el.style.fontSynthesis` expands into three longhands and serializes in
    /// Chromium's order (weight, style, small-caps); `fontSynthesisSmallCaps` is its own property, recomposing the shorthand.
    #[test]
    fn font_synthesis_smallcaps_cssom() {
        let mut e = font_synth_cssom_engine();
        let set = |e: &mut crate::AetherEngine, v: &str| {
            font_synth_eval(e, &format!("var s = document.getElementById('a').style; s.fontSynthesis = '{v}'; [s.fontSynthesis, s.fontSynthesisWeight, s.fontSynthesisStyle, s.fontSynthesisSmallCaps].join('|')"))
        };
        assert_eq!(set(&mut e, "small-caps weight"), "weight small-caps|auto|none|auto");
        assert_eq!(set(&mut e, "none"), "none|none|none|none");
        assert_eq!(set(&mut e, "weight style small-caps"), "weight style small-caps|auto|auto|auto");
        let r = font_synth_eval(
            &mut e,
            "var s = document.getElementById('a').style; s.fontSynthesisSmallCaps = 'none'; var a = [s.fontSynthesis]; \
             s.fontSynthesisSmallCaps = 'bogus'; a.push(s.fontSynthesisSmallCaps); \
             s.setProperty('font-synthesis-small-caps', 'auto'); a.push(s.getPropertyValue('font-synthesis')); a.join('|')",
        );
        assert_eq!(r, "weight style|none|weight style small-caps");
    }

    /// AETHERFONTSYNTHESISSMALLCAPS (SR772): getComputedStyle reads the initial `weight style small-caps` and the longhand
    /// `auto`; `font-synthesis-small-caps` inherits on its own; a declared `weight style` drops small-caps.
    #[test]
    fn font_synthesis_smallcaps_computed() {
        let html = r#"<!doctype html><html><body>
              <div id="a"></div>
              <section style="font-synthesis-small-caps:none"><div id="k"></div></section>
              <div id="ws" style="font-synthesis:weight style"></div>
            </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let cs = |e: &mut crate::AetherEngine, id: &str, p: &str| {
            font_synth_eval(e, &format!("getComputedStyle(document.getElementById('{id}')).getPropertyValue('{p}')"))
        };
        assert_eq!(cs(&mut e, "a", "font-synthesis"), "weight style small-caps");
        assert_eq!(cs(&mut e, "a", "font-synthesis-small-caps"), "auto");
        assert_eq!(cs(&mut e, "k", "font-synthesis"), "weight style", "the small-caps longhand inherits");
        assert_eq!(cs(&mut e, "k", "font-synthesis-small-caps"), "none");
        assert_eq!(cs(&mut e, "ws", "font-synthesis"), "weight style");
        assert_eq!(font_synth_eval(&mut e, "getComputedStyle(document.getElementById('a')).fontSynthesisSmallCaps"), "auto");
    }

    // ---- AETHERIMGALTINLINEICONFLOAT (LEDGER SR779): the inline alt's icon floats beside every line it spans (named tail block) ----

    /// AETHERIMGALTINLINEICONFLOAT (SR779): the 20-word alt in a 100px column at `font` (a 12px face), as `id`.
    fn img_alt_float_page(dir: &str, font: &str) -> crate::AetherEngine {
        let alt = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
        let html = format!(
            r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div{dir} style="width:100px;font:{font}"><img id="f" src="missing.png" alt="{alt}"></div>
            <div style="width:100px;font:{font}"><img id="g" src="missing.png" alt="{alt}"></div>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERIMGALTINLINEICONFLOAT (SR779): Chromium's 16x16 icon is a float, so at a 12px line height lines 1 AND 2 sit
    /// beside it (no text ink in the icon's 16 columns on their rows) and line 3 starts at the box's left (ink there);
    /// at a 16px line height only line 1 is indented (SR726's rule). The box is whole lines tall.
    #[test]
    fn img_alt_inline_icon_floats_past_short_lines() {
        let mut engine = img_alt_float_page("", "12px/12px sans-serif");
        let (l, t, w, h) = imgalt_rect(&mut engine, "f");
        assert!(w <= 100.5 && h >= 36.0 && (h / 12.0 - (h / 12.0).round()).abs() < 0.05, "f: three or more 12px lines inside the column ({w}x{h})");
        let (lw, tt) = (l.round() as usize, t.round() as usize);
        let band = |i: usize| (tt + 12 * i + 2, tt + 12 * i + 10);
        for i in 0..2 {
            let (y0, y1) = band(i);
            assert_eq!(imgalt_ink(&engine, lw, lw + 16, y0, y1), 0, "f: line {} is beside the icon", i + 1);
            assert!(imgalt_ink(&engine, lw + 16, (l + w).ceil() as usize, y0, y1) > 0, "f: line {} paints after the icon", i + 1);
        }
        let (y0, y1) = band(2);
        assert!(imgalt_ink(&engine, lw, lw + 16, y0, y1) > 0, "f: line 3 starts at the box's left");
        let mut engine = img_alt_float_page("", "12px/16px sans-serif");
        let (l, t, _, _) = imgalt_rect(&mut engine, "f");
        let (lw, tt) = (l.round() as usize, t.round() as usize);
        assert_eq!(imgalt_ink(&engine, lw, lw + 16, tt + 2, tt + 14), 0, "16px lines: line 1 is beside the icon");
        assert!(imgalt_ink(&engine, lw, lw + 16, tt + 18, tt + 30) > 0, "16px lines: line 2 is not indented");
    }

    /// AETHERIMGALTINLINEICONFLOAT (SR779): in an rtl column Chromium floats the icon RIGHT: its page outline grey sits at
    /// the box's (width - 16 + 1, 5), lines 1 and 2 (12px lines) end before the icon's 16 columns, and the box is as wide
    /// as the ltr twin's (the measure is direction-agnostic).
    #[test]
    fn img_alt_inline_icon_floats_right_in_rtl() {
        let mut engine = img_alt_float_page(r#" dir="rtl""#, "12px/12px sans-serif");
        let (l, t, w, h) = imgalt_rect(&mut engine, "f");
        let (_, _, gw, gh) = imgalt_rect(&mut engine, "g");
        assert!((w - gw).abs() <= 0.5 && (h - gh).abs() <= 0.5, "rtl box {w}x{h} vs the ltr twin {gw}x{gh}");
        let (r, tt) = ((l + w).round() as usize, t.round() as usize);
        let grey = (r - 17..=r - 13).any(|x| (tt + 4..=tt + 6).any(|y| imgalt_px(&engine, x, y) == (128, 128, 128)));
        assert!(grey, "rtl: the icon's page outline at the box's right edge");
        for i in 0..2 {
            let (y0, y1) = (tt + 12 * i + 2, tt + 12 * i + 10);
            assert_eq!(imgalt_ink(&engine, r - 16, r, y0, y1), 0, "rtl: line {} ends before the icon", i + 1);
            assert!(imgalt_ink(&engine, l.floor() as usize, r - 16, y0, y1) > 0, "rtl: line {} paints", i + 1);
        }
    }

    // ---- AETHERIMGALTINLINEICONFLOATCLEAR (LEDGER SR795): the inline alt's box clears its 16 px icon float (named tail block) ----

    /// AETHERIMGALTINLINEICONFLOATCLEAR (SR795): a reference span "Hello", then `target`, then a second line "Hello" (`n`),
    /// in a div `d` at `font`.
    fn img_alt_float_clear_page(font: &str, target: &str) -> crate::AetherEngine {
        let html = format!(
            r#"<!doctype html><html><body style="margin:0;font:16px sans-serif">
            <div id="d" style="font:{font}"><span id="r">Hello</span> {target}<br><span id="n">Hello</span></div>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERIMGALTINLINEICONFLOATCLEAR (SR795): Chromium's 16x16 icon floats inside the alt's container, which contains
    /// it, so a one-line alt on a 12 or 14 px line is a 16 px box (20 px lines keep 20); the alt text stays on SR730's
    /// first-line baseline (its bottom ink row is the reference span's), and the extra lies below it: the next line's
    /// first ink row is 4 px lower than in the twin page whose target is a plain span.
    #[test]
    fn img_alt_inline_icon_float_clears_short_lines() {
        let img = r#"<img id="t" src="missing.png" alt="Hello">"#;
        for (font, want) in [("12px/12px sans-serif", 16.0), ("12px/14px sans-serif", 16.0), ("12px/20px sans-serif", 20.0)] {
            let mut engine = img_alt_float_clear_page(font, img);
            let (_, _, _, h) = imgalt_rect(&mut engine, "t");
            assert!((h - want).abs() <= 0.5, "{font}: the alt box is {want} px tall ({h})");
        }
        let first_ink = |e: &crate::AetherEngine, x0: usize, x1: usize, y0: usize| (y0..300).find(|&y| imgalt_ink(e, x0, x1, y, y + 1) > 0);
        let bottom_ink = |e: &crate::AetherEngine, x0: usize, x1: usize, y1: usize| (0..y1).rev().find(|&y| imgalt_ink(e, x0, x1, y, y + 1) > 0);
        let mut engine = img_alt_float_clear_page("12px/12px sans-serif", img);
        let (rl, _, rw, _) = imgalt_rect(&mut engine, "r");
        let (tl, tt, tw, _) = imgalt_rect(&mut engine, "t");
        let y1 = (tt + 12.0).round() as usize;
        let rb = bottom_ink(&engine, rl.floor() as usize, (rl + rw).ceil() as usize, y1).expect("the reference span paints");
        let tb = bottom_ink(&engine, (tl + 17.0).ceil() as usize, (tl + tw).ceil() as usize, y1).expect("the alt text paints");
        assert!((rb as i64 - tb as i64).abs() <= 1, "SR730: the alt sits on the reference span's baseline (bottom ink rows {tb} vs {rb})");
        let (_, dt, _, _) = imgalt_rect(&mut engine, "d");
        let (nl, _, nw, _) = imgalt_rect(&mut engine, "n");
        let (x0, x1, y0) = (nl.floor() as usize + 17, (nl + nw).ceil() as usize, (dt + 12.0).round() as usize);
        let with_img = first_ink(&engine, x0, x1, y0).expect("line 2 paints under the img");
        let mut twin = img_alt_float_clear_page("12px/12px sans-serif", r#"<span id="t">Hello</span>"#);
        let (_, twin_dt, _, _) = imgalt_rect(&mut twin, "d");
        let with_span = first_ink(&twin, x0, x1, (twin_dt + 12.0).round() as usize).expect("line 2 paints under the span");
        let shift = with_img as i64 - with_span as i64;
        assert!((shift - 4).abs() <= 1, "the line below starts 16 px down, 4 below a 12 px line (line 2 first ink row {with_img} vs {with_span})");
    }

    // ---- AETHERFONTVARIANTSMALLCAPS (LEDGER SR800): font-variant: small-caps synthesizes small capitals (named tail block) ----

    /// AETHERFONTVARIANTSMALLCAPS (SR800): one line per probe at 16px serif: the small-caps "Hello" and its references
    /// ("H", "ELLO", a plain "Hello"), the two synthesis-none spellings, two block paragraphs for the line box, and the
    /// `font` shorthand's `small-caps` prefix with an inheriting and a resetting child.
    fn font_variant_small_caps_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div><span id="sc" style="font-variant:small-caps">Hello</span></div>
            <div><span id="h">H</span></div>
            <div><span id="ello">ELLO</span></div>
            <div><span id="plain">Hello</span></div>
            <div><span id="none" style="font-variant:small-caps;font-synthesis-small-caps:none">Hello</span></div>
            <div><span id="none2" style="font-variant:small-caps;font-synthesis:none">Hello</span></div>
            <div id="dsc" style="font-variant:small-caps">Hello</div>
            <div id="dplain">Hello</div>
            <div id="f" style="font: small-caps 16px serif">x<span id="fk">y</span><span id="fn" style="font-variant-caps:normal">z</span></div>
            <div id="dflt">q</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): the dark-pixel count of each column across element `id`'s line (its rect,
    /// 2 px of margin above and below), from x 0: two runs painted with the same glyphs at the same pen agree exactly.
    fn small_caps_ink_columns(e: &mut crate::AetherEngine, id: &str) -> Vec<usize> {
        let (_, t, _, h) = imgalt_rect(e, id);
        let (y0, y1) = ((t - 2.0).max(0.0) as usize, (t + h + 2.0).ceil() as usize);
        (0..200).map(|x| imgalt_ink(e, x, x + 1, y0, y1)).collect()
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): Chromium shapes the lowercase "ello" of a small-caps "Hello" as "ELLO" at
    /// 0.7 x the font size (kSmallCapsFontSizeMultiplier, recalled), the "H" at full size: the run's advance less the
    /// "H"'s is 0.7 x a full-size "ELLO"'s, within 1 px.
    #[test]
    fn font_variant_small_caps_advance() {
        let mut e = font_variant_small_caps_engine();
        let w = |e: &mut crate::AetherEngine, id: &str| imgalt_rect(e, id).2;
        let (sc, h, ello, plain) = (w(&mut e, "sc"), w(&mut e, "h"), w(&mut e, "ello"), w(&mut e, "plain"));
        assert!(h > 5.0 && ello > 20.0, "the references lay out ({h} / {ello})");
        assert!((sc - h - 0.7 * ello).abs() <= 1.0, "small-caps Hello {sc} = H {h} + 0.7 x ELLO {ello}");
        assert!((sc - plain).abs() > 0.5, "the small-caps run is not the plain run ({sc} vs {plain})");
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): the small segments are shaped smaller but the line box reads the full-size
    /// face: a small-caps paragraph is exactly as tall as a plain one.
    #[test]
    fn font_variant_small_caps_linebox() {
        let mut e = font_variant_small_caps_engine();
        let (dsc, dplain) = (imgalt_rect(&mut e, "dsc").3, imgalt_rect(&mut e, "dplain").3);
        assert!(dplain > 10.0, "the plain paragraph has a line ({dplain})");
        assert!((dsc - dplain).abs() < 0.01, "the line box does not shrink ({dsc} vs {dplain})");
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): `font-synthesis-small-caps: none` (and the `font-synthesis: none` shorthand,
    /// SR772) turns the synthesis off: the lowercase renders as is — the advance and the painted ink are the plain run's.
    #[test]
    fn font_variant_small_caps_synthesis_none() {
        let mut e = font_variant_small_caps_engine();
        let plain_w = imgalt_rect(&mut e, "plain").2;
        let plain = small_caps_ink_columns(&mut e, "plain");
        assert!(plain.iter().sum::<usize>() > 20, "the plain run paints");
        for id in ["none", "none2"] {
            let w = imgalt_rect(&mut e, id).2;
            assert!((w - plain_w).abs() < 0.01, "#{id}: the plain advance ({w} vs {plain_w})");
            assert_eq!(small_caps_ink_columns(&mut e, id), plain, "#{id}: the plain run's ink");
        }
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): the split reaches the painter: the small-caps run's ink is not the plain
    /// run's, and its last inked column is within 1 px of its laid-out advance (the painters draw the pieces layout
    /// measured).
    #[test]
    fn font_variant_small_caps_ink() {
        let mut e = font_variant_small_caps_engine();
        let sc = small_caps_ink_columns(&mut e, "sc");
        assert!(sc.iter().sum::<usize>() > 20, "the small-caps run paints");
        assert_ne!(sc, small_caps_ink_columns(&mut e, "plain"), "small capitals are not the lowercase");
        let w = imgalt_rect(&mut e, "sc").2;
        let last = sc.iter().rposition(|&n| n > 0).unwrap() as f64;
        assert!(last <= w + 1.0 && last >= w - 4.0, "the ink ends at the advance ({last} vs {w})");
    }

    /// AETHERFONTVARIANTSMALLCAPS (SR800): getComputedStyle's `fontVariantCaps`: the `font` shorthand's `small-caps`
    /// prefix sets it (the CSSOM does not expand `font`: read from the layout), a child inherits it, a child's
    /// `font-variant-caps: normal` resets it, the longhand-less default is `normal`, `font-variant: small-caps` sets it.
    #[test]
    fn font_variant_small_caps_cssom() {
        let mut e = font_variant_small_caps_engine();
        let cs = |e: &mut crate::AetherEngine, id: &str| font_synth_eval(e, &format!("getComputedStyle(document.getElementById('{id}')).fontVariantCaps"));
        for (id, want) in [("f", "small-caps"), ("fk", "small-caps"), ("fn", "normal"), ("dflt", "normal"), ("sc", "small-caps"), ("dsc", "small-caps")] {
            assert_eq!(cs(&mut e, id), want, "#{id}: computed font-variant-caps");
        }
        assert_eq!(font_synth_eval(&mut e, "getComputedStyle(document.getElementById('f')).getPropertyValue('font-variant-caps')"), "small-caps");
    }

    // ---- AETHERFONTSHORTHANDRESET (LEDGER SR805): the font shorthand resets its omitted longhands (named tail block) ----

    /// AETHERFONTSHORTHANDRESET (SR805): one line per probe at 16px serif: a longhand declared BEFORE `font: 16px serif`
    /// on the same element (italic / bold / small-caps / line-height / font-synthesis), a longhand AFTER it, an italic
    /// parent with a `font` child, a plain reference and an italic reference.
    fn font_shorthand_reset_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div><span id="plain">Hello</span></div>
            <div><span id="itref" style="font-style:italic">Hello</span></div>
            <div><span id="it" style="font-style:italic;font:16px serif">Hello</span></div>
            <div><span id="bd" style="font-weight:bold;font:16px serif">Hello</span></div>
            <div><span id="sc" style="font-variant:small-caps;font:16px serif">Hello</span></div>
            <div id="lh" style="line-height:3;font:16px serif">Hello</div>
            <div><span id="late" style="font:16px serif;font-style:italic">Hello</span></div>
            <div style="font-style:italic"><span id="kid" style="font:16px serif">Hello</span></div>
            <div><span id="syn" style="font-synthesis:none;font:16px serif">Hello</span></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn font_shorthand_cs(e: &mut crate::AetherEngine, id: &str, prop: &str) -> String {
        font_synth_eval(e, &format!("getComputedStyle(document.getElementById('{id}')).{prop}"))
    }

    /// AETHERFONTSHORTHANDRESET (SR805): getComputedStyle after `font: 16px serif`: an earlier font-style / font-weight /
    /// font-variant-caps / line-height reads its initial value, a later font-style wins, an italic parent's child reads
    /// `normal` (the reset writes the initial, not inherit), and font-synthesis (not in the reset set) survives.
    #[test]
    fn font_shorthand_reset_cssom() {
        let mut e = font_shorthand_reset_engine();
        assert_eq!(font_shorthand_cs(&mut e, "itref", "fontStyle"), "italic", "the reference is italic");
        assert_eq!(font_shorthand_cs(&mut e, "it", "fontStyle"), "normal");
        assert_eq!(font_shorthand_cs(&mut e, "bd", "fontWeight"), "400");
        assert_eq!(font_shorthand_cs(&mut e, "sc", "fontVariantCaps"), "normal");
        assert_eq!(font_shorthand_cs(&mut e, "lh", "lineHeight"), "normal");
        assert_eq!(font_shorthand_cs(&mut e, "late", "fontStyle"), "italic", "a longhand after the shorthand wins");
        assert_eq!(font_shorthand_cs(&mut e, "kid", "fontStyle"), "normal", "the child resets, it does not inherit");
        assert_eq!(font_shorthand_cs(&mut e, "syn", "fontSynthesis"), "none", "font does not reset font-synthesis");
    }

    /// AETHERFONTSHORTHANDRESET (SR805): the reset reaches layout and paint: the italic-then-font run and the italic
    /// parent's font child paint the plain run's ink (the italic reference does not), and the small-caps-then-font run
    /// has the plain advance.
    #[test]
    fn font_shorthand_reset_ink() {
        let mut e = font_shorthand_reset_engine();
        let plain = small_caps_ink_columns(&mut e, "plain");
        assert!(plain.iter().sum::<usize>() > 20, "the plain run paints");
        assert_ne!(small_caps_ink_columns(&mut e, "itref"), plain, "the italic reference is not the plain ink (armed)");
        for id in ["it", "kid"] {
            assert_eq!(small_caps_ink_columns(&mut e, id), plain, "#{id}: upright, the plain run's ink");
        }
        let plain_w = imgalt_rect(&mut e, "plain").2;
        for id in ["sc", "bd"] {
            let w = imgalt_rect(&mut e, id).2;
            assert!((w - plain_w).abs() < 0.01, "#{id}: the plain advance ({w} vs {plain_w})");
        }
    }

    // ---- AETHERFONTVARIANTCAPSALL (LEDGER SR806): the seven font-variant-caps values on SR800's small-caps road (named tail block) ----

    /// AETHERFONTVARIANTCAPSALL (SR806): one line per probe at 16px serif: "Hello" under each caps value, its references
    /// (a plain "Hello", a full-size "HELLO"), the four synthesized values under `font-synthesis-small-caps: none`, a
    /// `normal` child under an all-small-caps parent, and the invalid `font: all-small-caps 20px serif`.
    fn font_variant_caps_all_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div><span id="asc" style="font-variant-caps:all-small-caps">Hello</span></div>
            <div><span id="sc" style="font-variant-caps:small-caps">Hello</span></div>
            <div><span id="pc" style="font-variant-caps:petite-caps">Hello</span></div>
            <div><span id="apc" style="font-variant-caps:all-petite-caps">Hello</span></div>
            <div><span id="uc" style="font-variant-caps:unicase">Hello</span></div>
            <div><span id="tc" style="font-variant-caps:titling-caps">Hello</span></div>
            <div><span id="up">HELLO</span></div>
            <div><span id="plain">Hello</span></div>
            <div><span id="nsc" style="font-variant-caps:small-caps;font-synthesis-small-caps:none">Hello</span></div>
            <div><span id="nasc" style="font-variant-caps:all-small-caps;font-synthesis-small-caps:none">Hello</span></div>
            <div><span id="npc" style="font-variant-caps:petite-caps;font-synthesis-small-caps:none">Hello</span></div>
            <div><span id="napc" style="font-variant-caps:all-petite-caps;font-synthesis-small-caps:none">Hello</span></div>
            <div style="font-variant-caps:all-small-caps"><span id="nchild" style="font-variant-caps:normal">Hello</span></div>
            <div id="fsh" style="font: all-small-caps 20px serif">q</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFONTVARIANTCAPSALL (SR806): all-small-caps shapes every letter of "Hello" as "HELLO" at 0.7 x the size (the
    /// "H" too): the run's advance is 0.7 x a full-size "HELLO"'s within 1 px, and narrower than small-caps' (full H).
    #[test]
    fn font_variant_caps_all_advance() {
        let mut e = font_variant_caps_all_engine();
        let w = |e: &mut crate::AetherEngine, id: &str| imgalt_rect(e, id).2;
        let (asc, up, sc) = (w(&mut e, "asc"), w(&mut e, "up"), w(&mut e, "sc"));
        assert!(up > 25.0, "the reference lays out ({up})");
        assert!((asc - 0.7 * up).abs() <= 1.0, "all-small-caps Hello {asc} = 0.7 x HELLO {up}");
        assert!(asc + 0.5 < sc, "the H is small too ({asc} vs small-caps {sc})");
        let ink = small_caps_ink_columns(&mut e, "asc");
        assert!(ink.iter().sum::<usize>() > 20, "the all-small-caps run paints");
        let last = ink.iter().rposition(|&n| n > 0).unwrap() as f64;
        assert!(last <= asc + 1.0 && last >= asc - 4.0, "the ink ends at the advance ({last} vs {asc})");
    }

    /// AETHERFONTVARIANTCAPSALL (SR806): the face has no `pcap`, so petite-caps draws as small-caps and all-petite-caps
    /// as all-small-caps (Chromium's fallback, recalled): the same advance and the same ink.
    #[test]
    fn font_variant_caps_petite_ink() {
        let mut e = font_variant_caps_all_engine();
        for (petite, small) in [("pc", "sc"), ("apc", "asc")] {
            let (wp, ws) = (imgalt_rect(&mut e, petite).2, imgalt_rect(&mut e, small).2);
            assert!((wp - ws).abs() < 0.01, "#{petite} advance = #{small} ({wp} vs {ws})");
            assert_eq!(small_caps_ink_columns(&mut e, petite), small_caps_ink_columns(&mut e, small), "#{petite} ink = #{small}");
        }
    }

    /// AETHERFONTVARIANTCAPSALL (SR806): unicase and titling-caps are not synthesized (no `unic` / `titl` in the face):
    /// the plain run's advance and ink; so is a `font-variant-caps: normal` child of an all-small-caps parent (its
    /// value clears both caps bits).
    #[test]
    fn font_variant_caps_unicase_titling_plain() {
        let mut e = font_variant_caps_all_engine();
        let plain_w = imgalt_rect(&mut e, "plain").2;
        let plain = small_caps_ink_columns(&mut e, "plain");
        assert!(plain.iter().sum::<usize>() > 20, "the plain run paints");
        for id in ["uc", "tc", "nchild"] {
            let w = imgalt_rect(&mut e, id).2;
            assert!((w - plain_w).abs() < 0.01, "#{id}: the plain advance ({w} vs {plain_w})");
            assert_eq!(small_caps_ink_columns(&mut e, id), plain, "#{id}: the plain run's ink");
        }
    }

    /// AETHERFONTVARIANTCAPSALL (SR806): `font-synthesis-small-caps: none` gates all four synthesized values
    /// (css-fonts-4 §4.5): each draws the plain run.
    #[test]
    fn font_variant_caps_all_synthesis_none() {
        let mut e = font_variant_caps_all_engine();
        let plain_w = imgalt_rect(&mut e, "plain").2;
        let plain = small_caps_ink_columns(&mut e, "plain");
        for id in ["nsc", "nasc", "npc", "napc"] {
            let w = imgalt_rect(&mut e, id).2;
            assert!((w - plain_w).abs() < 0.01, "#{id}: the plain advance ({w} vs {plain_w})");
            assert_eq!(small_caps_ink_columns(&mut e, id), plain, "#{id}: the plain run's ink");
        }
    }

    /// AETHERFONTVARIANTCAPSALL (SR806): getComputedStyle's `fontVariantCaps` names each keyword; the `font` shorthand
    /// takes only `small-caps` in its prefix, so `font: all-small-caps 20px serif` is invalid and dropped whole:
    /// `normal` and the inherited 16px.
    #[test]
    fn font_variant_caps_all_cssom() {
        let mut e = font_variant_caps_all_engine();
        let cs = |e: &mut crate::AetherEngine, id: &str, p: &str| font_synth_eval(e, &format!("getComputedStyle(document.getElementById('{id}')).{p}"));
        for (id, want) in [
            ("asc", "all-small-caps"), ("sc", "small-caps"), ("pc", "petite-caps"), ("apc", "all-petite-caps"),
            ("uc", "unicase"), ("tc", "titling-caps"), ("nchild", "normal"), ("plain", "normal"),
        ] {
            assert_eq!(cs(&mut e, id, "fontVariantCaps"), want, "#{id}: computed font-variant-caps");
        }
        assert_eq!(cs(&mut e, "fsh", "fontVariantCaps"), "normal", "the invalid shorthand sets no caps");
        assert_eq!(cs(&mut e, "fsh", "fontSize"), "16px", "the invalid shorthand sets no size");
    }

    // ---- AETHERFONTSHORTHANDINHERIT (LEDGER SR810): font: inherit / unset / initial reach the longhands (named tail block) ----

    /// AETHERFONTSHORTHANDINHERIT (SR810): one line per probe under a `font: italic 20px serif` parent: a plain child
    /// (the reference), `font: inherit` and `font: unset` after same-block longhands, `font: initial`, and three buttons
    /// (`font: inherit`, a `font-size: 20px` twin, and a plain one at the control default).
    fn font_wide_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font:italic 20px serif"><span id="ref">Hello</span></div>
            <div style="font:italic 20px serif"><span id="inh" style="font-style:normal;font-size:12px;font:inherit">Hello</span></div>
            <div style="font:italic 20px serif"><span id="uns" style="font-size:12px;font:unset">Hello</span></div>
            <div style="font:italic 20px serif"><span id="ini" style="font:initial">Hello</span></div>
            <div style="font:italic 20px serif"><button id="btn" style="font:inherit">Go</button></div>
            <div style="font:italic 20px serif"><button id="pbtn" style="font:italic 20px serif">Go</button></div>
            <div style="font:italic 20px serif"><button id="ubtn">Go</button></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFONTSHORTHANDINHERIT (SR810): getComputedStyle: the `font: inherit` / `font: unset` children read the
    /// parent's italic 20px past their own earlier longhands; the `font: initial` child reads upright 16px.
    #[test]
    fn font_inherit_cssom() {
        let mut e = font_wide_engine();
        for id in ["inh", "uns"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "fontStyle"), "italic", "#{id} inherits the parent's style");
            assert_eq!(font_shorthand_cs(&mut e, id, "fontSize"), "20px", "#{id} inherits the parent's size");
        }
        assert_eq!(font_shorthand_cs(&mut e, "ini", "fontStyle"), "normal", "font: initial is upright");
        assert_eq!(font_shorthand_cs(&mut e, "ini", "fontSize"), "16px", "font: initial is medium");
    }

    /// AETHERFONTSHORTHANDINHERIT (SR810): the inherit reaches layout and paint: the `font: inherit` and `font: unset`
    /// runs paint the reference run's ink column for column; the `font: initial` run does not (armed).
    #[test]
    fn font_inherit_ink() {
        let mut e = font_wide_engine();
        let reference = small_caps_ink_columns(&mut e, "ref");
        assert!(reference.iter().sum::<usize>() > 20, "the reference run paints");
        for id in ["inh", "uns"] {
            assert_eq!(small_caps_ink_columns(&mut e, id), reference, "#{id}: the parent's italic 20px ink");
        }
        assert_ne!(small_caps_ink_columns(&mut e, "ini"), reference, "font: initial is not the parent's font (armed)");
    }

    /// AETHERFONTSHORTHANDINHERIT (SR810): `button { font: inherit }` (every reset sheet's line) takes the parent's
    /// 20px: its box is the `font: italic 20px serif` twin's, not the 13.333px control default's (armed by the plain
    /// button). AETHERCSSWIDELONGHANDS (SR815) re-pinned the twin: the inherit button now takes the parent's family too.
    #[test]
    fn font_inherit_button_takes_parent_size() {
        let mut e = font_wide_engine();
        let (_, _, w, h) = imgalt_rect(&mut e, "btn");
        let (_, _, pw, ph) = imgalt_rect(&mut e, "pbtn");
        let (_, _, uw, uh) = imgalt_rect(&mut e, "ubtn");
        assert!((w - pw).abs() <= 0.5 && (h - ph).abs() <= 0.5, "btn {w}x{h} is the 20px twin's {pw}x{ph}");
        assert!((h - uh).abs() > 0.5 || (w - uw).abs() > 0.5, "the plain button {uw}x{uh} differs (armed)");
    }

    // ---- AETHERCSSWIDELONGHANDS (LEDGER SR815): an explicit inherit beats an earlier rule and the tag default (named tail block) ----

    /// AETHERCSSWIDELONGHANDS (SR815): one line per probe under a 20px serif upright body, the rules in a sheet: an
    /// upright reference, `p { font-style: italic }` and its `p.x { font-style: inherit }` twin, `em` with
    /// `font-style: inherit` / `unset` beside a plain (tag-default italic) em, an italic parent's plain child and its
    /// same-block `font-style: normal; font-style: inherit` twin, and three buttons under `button { font-size: 13px }`:
    /// `button.i { font: inherit }`, `button.t { font: 20px serif }` and a plain one. Rows 12px apart, so one probe's
    /// ink window (its rect +- 2px) does not take a neighbour's glyph edge.
    fn css_wide_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div, p { margin: 0 0 12px }
            p { font-style: italic }
            p.x { font-style: inherit }
            em.n { font-style: inherit }
            em.u { font-style: unset }
            button { font-size: 13px }
            button.i { font: inherit }
            button.t { font: 20px serif }
            </style></head><body style="margin:0;font:20px serif">
            <div><span id="plain">Hello</span></div>
            <p id="itp">Hello</p>
            <p id="px" class="x">Hello</p>
            <div><em id="emref">Hello</em></div>
            <div><em id="emn" class="n">Hello</em></div>
            <div><em id="emu" class="u">Hello</em></div>
            <div style="font-style:italic"><span id="itkid">Hello</span></div>
            <div style="font-style:italic"><span id="inhkid" style="font-style:normal;font-style:inherit">Hello</span></div>
            <div><button id="bi" class="i">Go</button></div>
            <div><button id="bt" class="t">Go</button></div>
            <div><button id="bp">Go</button></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDELONGHANDS (SR815): `p.x { font-style: inherit }` after `p { font-style: italic }` under an upright
    /// parent paints upright (the plain reference's ink; the italic p does not, armed) and cssom reads `normal`.
    #[test]
    fn css_wide_cross_rule_upright() {
        let mut e = css_wide_engine();
        let plain = small_caps_ink_columns(&mut e, "plain");
        assert!(plain.iter().sum::<usize>() > 20, "the reference run paints");
        assert_ne!(small_caps_ink_columns(&mut e, "itp"), plain, "the italic p is not the upright ink (armed)");
        assert_eq!(small_caps_ink_columns(&mut e, "px"), plain, "p.x inherits the upright parent past the earlier rule");
        assert_eq!(font_shorthand_cs(&mut e, "itp", "fontStyle"), "italic");
        assert_eq!(font_shorthand_cs(&mut e, "px", "fontStyle"), "normal", "cssom reads the parent's style");
    }

    /// AETHERCSSWIDELONGHANDS (SR815): `em { font-style: inherit }` beats the tag's UA italic, and `unset` is inherit for
    /// an inherited property (css-cascade-4 §7.3.3): both paint the upright reference's ink; the plain em does not.
    #[test]
    fn css_wide_tag_default_beaten() {
        let mut e = css_wide_engine();
        let plain = small_caps_ink_columns(&mut e, "plain");
        assert_ne!(small_caps_ink_columns(&mut e, "emref"), plain, "a plain em is italic by the tag default (armed)");
        for id in ["emn", "emu"] {
            assert_eq!(small_caps_ink_columns(&mut e, id), plain, "#{id}: upright, the parent's style");
            assert_eq!(font_shorthand_cs(&mut e, id, "fontStyle"), "normal", "#{id}: cssom");
        }
    }

    /// AETHERCSSWIDELONGHANDS (SR815): `font-style: normal; font-style: inherit` on a child of an italic parent (the later
    /// declaration wins) paints the parent's italic: the italic parent's plain child's ink, not the upright reference's.
    #[test]
    fn css_wide_child_takes_italic_parent() {
        let mut e = css_wide_engine();
        let italic_kid = small_caps_ink_columns(&mut e, "itkid");
        assert_ne!(italic_kid, small_caps_ink_columns(&mut e, "plain"), "the italic parent's child is italic (armed)");
        assert_eq!(small_caps_ink_columns(&mut e, "inhkid"), italic_kid, "the same-block inherit takes the parent's italic");
        assert_eq!(font_shorthand_cs(&mut e, "inhkid", "fontStyle"), "italic");
    }

    /// AETHERCSSWIDELONGHANDS (SR815): `button.i { font: inherit }` after `button { font-size: 13px }` under a 20px serif
    /// parent takes 20px serif: its box is the `font: 20px serif` button's, not the plain 13px button's (armed), and
    /// cssom reads `20px`.
    #[test]
    fn css_wide_control_font_inherit() {
        let mut e = css_wide_engine();
        let (_, _, w, h) = imgalt_rect(&mut e, "bi");
        let (_, _, tw, th) = imgalt_rect(&mut e, "bt");
        let (_, _, pw, ph) = imgalt_rect(&mut e, "bp");
        assert!((w - tw).abs() <= 0.5 && (h - th).abs() <= 0.5, "bi {w}x{h} is the 20px serif button's {tw}x{th}");
        assert!((h - ph).abs() > 0.5 || (w - pw).abs() > 0.5, "the plain 13px button {pw}x{ph} differs (armed)");
        assert_eq!(font_shorthand_cs(&mut e, "bi", "fontSize"), "20px", "cssom reads the parent's size");
        assert_ne!(font_shorthand_cs(&mut e, "bp", "fontSize"), "20px", "the plain button keeps the earlier rule (armed)");
    }

    // ---- AETHERCSSOMFONTSIZEREL (LEDGER SR819): getComputedStyle's fontSize is the used size (named tail block) ----

    /// AETHERCSSOMFONTSIZEREL (SR819): one probe per relative road: em and % of the parent's size, smaller / larger
    /// (layout_core's /1.2 and x1.2), `font: inherit` and em on a control, a plain control, and nested em.
    fn font_size_rel_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font-size:10px"><span id="em" style="font-size:2em">Hello</span></div>
            <div style="font-size:16px"><span id="pct" style="font-size:150%">Hello</span></div>
            <div style="font-size:10px"><span id="sm" style="font-size:smaller">Hello</span></div>
            <div style="font-size:10px"><span id="lg" style="font-size:larger">Hello</span></div>
            <div style="font-size:20px"><button id="fib" style="font:inherit">Go</button></div>
            <div style="font-size:20px"><button id="plain">Go</button></div>
            <div style="font-size:10px"><button id="emb" style="font-size:2em">Go</button></div>
            <div style="font-size:10px"><div style="font-size:2em"><span id="nest" style="font-size:1.5em">Hello</span></div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMFONTSIZEREL (SR819): `font-size: 2em` under 10px reads 20px and `150%` under 16px reads 24px (the
    /// specified road read the parent's size: 10px / 16px).
    #[test]
    fn font_size_rel_cssom_em_percent() {
        let mut e = font_size_rel_engine();
        assert_eq!(font_shorthand_cs(&mut e, "em", "fontSize"), "20px", "2em of 10px");
        assert_eq!(font_shorthand_cs(&mut e, "pct", "fontSize"), "24px", "150% of 16px");
    }

    // ---- AETHERCSSCONTROLUAFONT (LEDGER SR1009): a form control takes the UA weight 400 and upright style, not the parent's (named tail block) ----

    /// AETHERCSSCONTROLUAFONT (SR1009): one row per probe under `body { font: 24px serif }`, the rules in a sheet, every
    /// button label red so its ink is countable. A text span; a plain button; a button under `<b>` and under `<i>`; an
    /// author `font-weight: bold` button (the armed heavy ink); `.h { font: inherit }` button / input / select / textarea
    /// under a bold italic row; plain input / select / textarea under `<b><i>`; an author `font-size: 20px` button.
    /// Rows 12px apart, so one probe's rect does not take a neighbour's ink.
    fn control_ua_font_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            button { color: rgb(255, 0, 0) }
            .h { font: inherit }
            </style></head><body style="margin:0;font:24px serif">
            <div><span id="txt">Hello</span></div>
            <div><button id="bp">Hello</button></div>
            <div><b><button id="bb">Hello</button></b></div>
            <div><i><button id="bi">Hello</button></i></div>
            <div><button id="bw" style="font-weight:bold">Hello</button></div>
            <div style="font-weight:bold;font-style:italic"><button id="bh" class="h">Hello</button></div>
            <div style="font-weight:bold;font-style:italic"><input id="ih" class="h" value="Hello"></div>
            <div style="font-weight:bold;font-style:italic"><select id="sh" class="h"><option>Hello</option></select></div>
            <div style="font-weight:bold;font-style:italic"><textarea id="th" class="h">Hello</textarea></div>
            <div><b><i><input id="ip" value="Hello"></i></b></div>
            <div><b><i><select id="sp"><option>Hello</option></select></i></b></div>
            <div><b><i><textarea id="tp">Hello</textarea></i></b></div>
            <div><button id="bs" style="font-size:20px">Hello</button></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 900));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCONTROLUAFONT (SR1009): the size and family were already the UA control font's (layout_core's
    /// `default_font_size` / `default_family` control arms): an unset button reads 13.333px Arial under the 24px serif
    /// body, a textarea monospace, and `font: inherit` takes the body's 24px serif (a taller box).
    #[test]
    fn control_ua_font_size_family() {
        let mut e = control_ua_font_engine();
        assert_eq!(font_shorthand_cs(&mut e, "txt", "fontSize"), "24px", "the body text is 24px");
        let bp = font_shorthand_cs(&mut e, "bp", "fontSize");
        assert!(bp.starts_with("13.3333"), "an unset button reads the control size, got {bp}");
        assert_eq!(font_shorthand_cs(&mut e, "bp", "fontFamily"), "Arial", "the control family");
        assert_eq!(font_shorthand_cs(&mut e, "tp", "fontFamily"), "monospace", "Chromium's textarea monospace");
        assert_eq!(font_shorthand_cs(&mut e, "bh", "fontSize"), "24px", "font: inherit takes the body's size");
        assert_eq!(font_shorthand_cs(&mut e, "bh", "fontFamily"), "serif", "font: inherit takes the body's family");
        let (_, _, _, h) = imgalt_rect(&mut e, "bp");
        let (_, _, _, hh) = imgalt_rect(&mut e, "bh");
        assert!(h < hh, "the 13.3333px button {h} is shorter than the 24px font: inherit twin {hh}");
    }

    /// AETHERCSSCONTROLUAFONT (SR1009): `font: -webkit-small-control` resets the weight and style, so a control under
    /// `<b>` / `<i>` is 400 upright (it was 700 / italic: `default_weight` / `default_italic` fell to the parent): the
    /// report reads it, and the painter draws it: the `<b>` button's red label ink is the plain button's, the author-bold
    /// button's is heavier (armed).
    #[test]
    fn control_ua_font_weight_style() {
        let mut e = control_ua_font_engine();
        for id in ["bb", "ip", "sp", "tp"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "fontWeight"), "400", "{id} under <b> takes the UA weight");
        }
        for id in ["bi", "ip", "sp", "tp"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "fontStyle"), "normal", "{id} under <i> is upright");
        }
        let plain = css_wide_color_ink(&mut e, "bp", css_wide_red);
        let under_b = css_wide_color_ink(&mut e, "bb", css_wide_red);
        let bold = css_wide_color_ink(&mut e, "bw", css_wide_red);
        assert!(plain > 3, "the plain button paints its red label ({plain})");
        assert!(under_b.abs_diff(plain) * 10 <= plain, "the <b> button's ink {under_b} is the plain button's {plain}");
        assert!(bold * 100 > plain * 115, "the author-bold button's ink {bold} is heavier than {plain} (armed)");
    }

    /// AETHERCSSCONTROLUAFONT (SR1009): `font: inherit` (the INHERIT_FONT bits) beats the UA arm: the controls under the
    /// bold italic row read 700 italic.
    #[test]
    fn control_ua_font_inherit() {
        let mut e = control_ua_font_engine();
        for id in ["bh", "ih", "sh", "th"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "fontWeight"), "700", "{id} font: inherit takes the parent's bold");
            assert_eq!(font_shorthand_cs(&mut e, id, "fontStyle"), "italic", "{id} font: inherit takes the parent's italic");
        }
    }

    /// AETHERCSSCONTROLUAFONT (SR1009): an author value on the control wins over the UA font as before.
    #[test]
    fn center_restyle_items_follow_removed_align() {
        let html = r#"<!doctype html><html><body style="margin: 0"><table style="width: 600px; border-spacing: 0"><tr><td id="cell" align="center"><div id="d" style="width: 200px">ab</div></td></tr></table></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(800, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let r = |e: &mut crate::AetherEngine, id: &str, k: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().{k}"));
        let (cl, cw, dl, dw) = (r(&mut engine, "cell", "left"), r(&mut engine, "cell", "width"), r(&mut engine, "d", "left"), r(&mut engine, "d", "width"));
        assert!(dw > 0.0 && (dl + dw / 2.0 - (cl + cw / 2.0)).abs() <= 1.0, "td align=center centres #d (d {dl} + {dw}, cell {cl} + {cw})");
        engine.js_engine.as_mut().unwrap().execute("document.getElementById('cell').removeAttribute('align');").expect("script must run clean");
        engine.render_frame();
        let (cl, dl) = (r(&mut engine, "cell", "left"), r(&mut engine, "d", "left"));
        assert!((dl - (cl + 1.0)).abs() <= 1.0, "align removed: #d sits at the cell's content start (d {dl}, cell {cl})");
        let ta = engine.js_engine.as_mut().unwrap().eval_string("getComputedStyle(document.getElementById('cell')).textAlign").unwrap();
        assert_ne!(ta, "-webkit-center", "align removed: the cell's computed text-align is no longer the align= value");
    }

    // ---- AETHERFILETYPECHANGE (LEDGER SR783): leaving type=file drops the selection (named tail block) ----

    /// AETHERFILETYPECHANGE (SR783): file input `f` (multiple, two Files through SR706's DataTransfer helper), `fv`
    /// (value="kept.txt", one File), text input `t` and a fresh multiple file input `fresh`; `input` / `change` on f, fv
    /// and t count into `window.ev`.
    fn file_type_change_page() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body>
            <div><input type="file" id="f" multiple></div>
            <div><input type="file" id="fv" value="kept.txt"></div>
            <div><input type="text" id="t"></div>
            <div><input type="file" id="fresh" multiple></div>
            <script>
              function list(names) { var d = new DataTransfer(); names.forEach(function (n) { d.items.add(new File(["x"], n)); }); return d.files; }
              document.getElementById("f").files = list(["a.txt", "b.txt"]);
              document.getElementById("fv").files = list(["report.pdf"]);
              window.ev = 0;
              ["f", "fv", "t"].forEach(function (id) { ["input", "change"].forEach(function (k) { document.getElementById(id).addEventListener(k, function () { window.ev++; }); }); });
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFILETYPECHANGE (SR783): `type = 'text'` on a file input holding a FileList drops the selection (Chromium
    /// destroys FileInputType and its FileList): `files` reads null, `value` reads the value content attribute (HTML
    /// §4.10.5 type change: "" with none, `kept.txt` on fv, never the fakepath), the FileList held before reads length
    /// 0 with no index, and no `input` / `change` event fires, then or after a tick.
    #[test]
    fn file_type_change_leaving_clears() {
        let mut engine = file_type_change_page();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { var f = document.getElementById("f"), fv = document.getElementById("fv"), k = f.files, n = k.length, fp = fv.value; f.type = "text"; fv.type = "text"; return [n, f.files === null, JSON.stringify(f.value), k.length, k[0] === undefined, fp, fv.value, fv.files === null, window.ev].join(","); })()"#)
            .unwrap();
        assert_eq!(got, "2,true,\"\",0,true,C:\\fakepath\\report.pdf,kept.txt,true,0");
        engine.tick();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        assert_eq!(js.eval_string("String(window.ev)").unwrap(), "0", "no event is queued by the type change");
    }

    /// AETHERFILETYPECHANGE (SR783): a file input taken to text and back to file holds an empty FileList, `value` "",
    /// no event fired, and its painted status is a fresh input's (SR715's probe), where two Files painted `2 files`.
    #[test]
    fn file_type_change_return_empty() {
        let mut engine = file_type_change_page();
        let before = file_value_clear_extent(&mut engine, "f");
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { var f = document.getElementById("f"); f.type = "text"; f.type = "file"; return [f.files.length, JSON.stringify(f.value), window.ev].join(","); })()"#)
            .unwrap();
        assert_eq!(got, "0,\"\",0");
        engine.tick(); engine.render_frame();
        let (f, fresh) = (file_value_clear_extent(&mut engine, "f"), file_value_clear_extent(&mut engine, "fresh"));
        assert_ne!(before, fresh, "two Files painted `2 files` before the type change");
        assert_eq!(f, fresh, "the returned input paints a fresh input's status");
    }

    /// AETHERFILETYPECHANGE (SR783): a text input's `files` is null (HTML: null off the File Upload state); taken to
    /// `file` it holds an empty FileList.
    #[test]
    fn file_type_change_text_to_file() {
        let mut engine = file_type_change_page();
        let js = engine.js_engine.as_mut().expect("page must have booted");
        let got = js
            .eval_string(r#"(function () { var t = document.getElementById("t"), was = t.files === null; t.type = "file"; return [was, t.files !== null, t.files.length].join(","); })()"#)
            .unwrap();
        assert_eq!(got, "true,true,0");
    }

    // ---- AETHERCSSOMLINEHEIGHTUSED (LEDGER SR824): getComputedStyle's lineHeight is the used px (named tail block) ----

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): one probe per line-height road: a number (under a px and an em font size), a
    /// length, `normal`, a % and em (at 16px and 20px, the latter through `reresolve_font_relative`), and inheritance
    /// (a number re-multiplies, a length is kept).
    fn line_height_used_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font-size:20px"><span id="num" style="line-height:1.5">Hello</span></div>
            <div style="font-size:10px"><span id="numem" style="font-size:2em;line-height:1.5">Hello</span></div>
            <div><span id="len" style="line-height:24px">Hello</span></div>
            <div><span id="nrm" style="line-height:normal">Hello</span></div>
            <div style="font-size:16px"><span id="pct" style="line-height:150%">Hello</span></div>
            <div style="font-size:16px"><span id="em16" style="line-height:1.5em">Hello</span></div>
            <div><span id="em20" style="font-size:20px;line-height:1.5em">Hello</span></div>
            <div style="font-size:10px;line-height:2"><span id="inum" style="font-size:20px">Hello</span></div>
            <div style="font-size:10px;line-height:20px"><span id="ilen" style="font-size:30px">Hello</span></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): a number reads number x the USED font size: `1.5` under 20px reads 30px
    /// (Chromium), and `font-size: 2em; line-height: 1.5` under 10px reads 30px (1.5 x the used 20px).
    #[test]
    fn line_height_used_cssom_number() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "num", "lineHeight"), "30px", "1.5 x 20px");
        assert_eq!(font_shorthand_cs(&mut e, "numem", "lineHeight"), "30px", "1.5 x the used 2em of 10px");
    }

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): a px line-height reads its px (it read `normal`: the stored length is negative).
    #[test]
    fn line_height_used_cssom_length() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "len", "lineHeight"), "24px");
    }

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): `normal` stays the keyword (Chromium).
    #[test]
    fn line_height_used_cssom_normal() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "nrm", "lineHeight"), "normal");
    }

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): a % and an em resolve against the element's own used font size: `150%` and
    /// `1.5em` at 16px read 24px, `1.5em` with `font-size: 20px` reads 30px.
    #[test]
    fn line_height_used_cssom_percent_em() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "pct", "lineHeight"), "24px", "150% of 16px");
        assert_eq!(font_shorthand_cs(&mut e, "em16", "lineHeight"), "24px", "1.5em of 16px");
        assert_eq!(font_shorthand_cs(&mut e, "em20", "lineHeight"), "30px", "1.5em of the element's own 20px");
    }

    /// AETHERCSSOMLINEHEIGHTUSED (SR824): CSS 2.2 §10.8.1 inheritance: a number inherits as the number (a 20px child of
    /// `line-height: 2` reads 40px), a length as the length (a 30px child of `line-height: 20px` reads 20px).
    #[test]
    fn line_height_used_cssom_inherit() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "inum", "lineHeight"), "40px", "2 x the child's 20px");
        assert_eq!(font_shorthand_cs(&mut e, "ilen", "lineHeight"), "20px", "the parent's 20px kept");
    }

    // ---- AETHERCSSWIDECOLOR (LEDGER SR823): an explicit color: inherit beats an earlier rule and the a tag's link blue (named tail block) ----

    /// AETHERCSSWIDECOLOR (SR823): one row per probe at 20px serif, the rules in a sheet. Under a red parent: a plain
    /// link (the UA link blue), `a.c { color: blue }` and its `a.c.x { color: inherit }` twin, `a.u { color: unset }`,
    /// `a.l { color: inherit } a.l.k { color: rgb(0, 160, 0) }` and a same-block `color: inherit; color: rgb(0, 160, 0)`
    /// link. Under a black-background yellow-text row (the dark theme): the reset sheet's `a.r { color: inherit }` and a
    /// plain link. Rows 12px apart, so one probe's rect does not take a neighbour's ink.
    fn css_wide_color_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            a.c { color: blue }
            a.c.x { color: inherit }
            a.r { color: inherit }
            a.u { color: unset }
            a.l { color: inherit }
            a.l.k { color: rgb(0, 160, 0) }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(255, 0, 0)"><a id="aref" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0)"><a id="ac" class="c" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0)"><a id="ax" class="c x" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0)"><a id="au" class="u" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0)"><a id="ak" class="l k" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0)"><a id="ai" href="#" style="color:inherit;color:rgb(0, 160, 0)">Hello</a></div>
            <div style="background:#000;color:rgb(255, 255, 0)"><a id="ar" class="r" href="#">Hello</a></div>
            <div style="background:#000;color:rgb(255, 255, 0)"><a id="aref2" href="#">Hello</a></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDECOLOR (SR823): the count of pixels in element `id`'s rect whose (r, g, b) passes `hue`.
    fn css_wide_color_ink(e: &mut crate::AetherEngine, id: &str, hue: fn((u8, u8, u8)) -> bool) -> usize {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (l.floor().max(0.0) as usize, ((l + w).ceil() as usize).min(sw));
        let (y0, y1) = (t.floor().max(0.0) as usize, ((t + h).ceil() as usize + 1).min(sh));
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if hue(imgalt_px(e, x, y)) {
                    n += 1;
                }
            }
        }
        n
    }

    fn css_wide_red(c: (u8, u8, u8)) -> bool {
        c.0 > 150 && c.1 < 90 && c.2 < 90
    }

    fn css_wide_blue(c: (u8, u8, u8)) -> bool {
        c.2 > 150 && c.0 < 90 && c.1 < 90
    }

    fn css_wide_green(c: (u8, u8, u8)) -> bool {
        c.1 > 110 && c.0 < 90 && c.2 < 90
    }

    fn css_wide_yellow(c: (u8, u8, u8)) -> bool {
        c.0 > 150 && c.1 > 150 && c.2 < 90
    }

    /// AETHERCSSWIDECOLOR (SR823): `a.c.x { color: inherit }` after `a.c { color: blue }` under a red parent paints red
    /// (no blue ink); the `a.c` twin and the plain link paint blue (armed); cssom reads the parent's rgb(255, 0, 0).
    #[test]
    fn css_wide_color_cross_rule_red() {
        let mut e = css_wide_color_engine();
        for id in ["aref", "ac"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_blue) > 3, "#{id} paints blue (armed)");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id} paints no red (armed)");
        }
        assert!(css_wide_color_ink(&mut e, "ax", css_wide_red) > 3, "a.c.x takes the red parent past the earlier rule");
        assert_eq!(css_wide_color_ink(&mut e, "ax", css_wide_blue), 0, "a.c.x paints no blue");
        assert_eq!(font_shorthand_cs(&mut e, "aref", "color"), "rgb(0, 0, 238)", "the plain link reads the UA link blue");
        assert_eq!(font_shorthand_cs(&mut e, "ax", "color"), "rgb(255, 0, 0)", "cssom reads the parent's colour");
    }

    /// AETHERCSSWIDECOLOR (SR823): the reset sheet's `a { color: inherit }` under a dark (black, yellow-text) row paints
    /// the row's yellow, not the UA link blue; the plain link beside it stays blue (armed).
    #[test]
    fn css_wide_color_reset_link_dark() {
        let mut e = css_wide_color_engine();
        assert!(css_wide_color_ink(&mut e, "aref2", css_wide_blue) > 3, "the plain link on the dark row is blue (armed)");
        assert!(css_wide_color_ink(&mut e, "ar", css_wide_yellow) > 3, "the reset link paints the row's yellow");
        assert_eq!(css_wide_color_ink(&mut e, "ar", css_wide_blue), 0, "the reset link paints no blue");
        assert_eq!(font_shorthand_cs(&mut e, "ar", "color"), "rgb(255, 255, 0)");
    }

    /// AETHERCSSWIDECOLOR (SR823): `color: unset` is inherit for an inherited property (css-cascade-4 §7.3.3): red.
    #[test]
    fn css_wide_color_unset_is_inherit() {
        let mut e = css_wide_color_engine();
        assert!(css_wide_color_ink(&mut e, "au", css_wide_red) > 3, "a.u takes the red parent");
        assert_eq!(css_wide_color_ink(&mut e, "au", css_wide_blue), 0, "a.u paints no blue");
        assert_eq!(font_shorthand_cs(&mut e, "au", "color"), "rgb(255, 0, 0)");
    }

    /// AETHERCSSWIDECOLOR (SR823): a later `color: rgb(0, 160, 0)` beats an earlier inherit, across rules (`a.l.k`) and
    /// in one block (the inline style): green, neither red nor blue.
    #[test]
    fn css_wide_color_later_value_wins() {
        let mut e = css_wide_color_engine();
        for id in ["ak", "ai"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_green) > 3, "#{id} paints the later green");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id} paints no red");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_blue), 0, "#{id} paints no blue");
            assert_eq!(font_shorthand_cs(&mut e, id, "color"), "rgb(0, 160, 0)", "#{id}: cssom");
        }
    }

    // ---- AETHERCSSWIDEINHERITEDREST (LEDGER SR831): an explicit inherit beats the white-space / direction / list-style tag roads (named tail block) ----

    /// AETHERCSSWIDEINHERITEDREST (SR831): one probe per row at 20px serif in a 400px viewport, the rules in a sheet: three
    /// pre with one 100-character line (plain, `white-space: inherit`, `inherit` then `pre`), three `Hello` blocks (`dir=rtl`,
    /// `dir=rtl` with `direction: inherit`, plain ltr), and lists nested one level inside a `ul` item: an ol with
    /// `list-style-type: inherit`, one with `unset`, a disc-styled twin and a plain ol; a ul with `inherit`, a disc-styled
    /// twin and a plain ul. Rows 12px apart, so one probe's ink window (its rect +- 2px) does not take a neighbour's.
    fn css_wide_rest_engine() -> crate::AetherEngine {
        let line = "the quick brown fox jumps over the lazy dog and keeps on running far past the edge of the box again";
        let html = format!(
            r#"<!doctype html><html><head><style>
            div, pre, ul, ol {{ margin: 0 0 12px }}
            pre.w {{ white-space: inherit }}
            pre.v {{ white-space: inherit; white-space: pre }}
            div.d {{ direction: inherit }}
            ol.i {{ list-style-type: inherit }}
            ol.u {{ list-style-type: unset }}
            ul ul.i {{ list-style-type: inherit }}
            </style></head><body style="margin:0;font:20px serif">
            <pre id="pp">{line}</pre>
            <pre id="pw" class="w">{line}</pre>
            <pre id="pv" class="v">{line}</pre>
            <div id="rref" dir="rtl">Hello</div>
            <div id="rd" dir="rtl" class="d">Hello</div>
            <div id="lref">Hello</div>
            <ul><li><ol class="i"><li id="oli">Hello</li></ol></li></ul>
            <ul><li><ol class="u"><li id="olu">Hello</li></ol></li></ul>
            <ul><li><ol style="list-style-type:disc"><li id="old">Hello</li></ol></li></ul>
            <ul><li><ol><li id="olp">Hello</li></ol></li></ul>
            <ul><li><ul class="i"><li id="uli">Hello</li></ul></li></ul>
            <ul><li><ul style="list-style-type:disc"><li id="uld">Hello</li></ul></li></ul>
            <ul><li><ul><li id="ulp">Hello</li></ul></li></ul>
        </body></html>"#
        );
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 900));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): the dark-pixel count of each column in [x0, x1) across element `id`'s rows (its
    /// rect, 2 px of margin above and below).
    fn css_wide_rest_columns(e: &mut crate::AetherEngine, id: &str, x0: usize, x1: usize) -> Vec<usize> {
        let (_, t, _, h) = imgalt_rect(e, id);
        let (y0, y1) = ((t - 2.0).max(0.0) as usize, (t + h + 2.0).ceil() as usize);
        (x0..x1).map(|x| imgalt_ink(e, x, x + 1, y0, y1)).collect()
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): the marker columns of list item `id`: the 36 px left of its box.
    fn css_wide_rest_marker(e: &mut crate::AetherEngine, id: &str) -> Vec<usize> {
        let l = imgalt_rect(e, id).0.max(0.0) as usize;
        css_wide_rest_columns(e, id, l.saturating_sub(36), l)
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): `pre.w { white-space: inherit }` under a `normal` body wraps its long line (more
    /// than 1.5 x the plain pre's one-line height; the plain pre does not wrap, armed); a later `white-space: pre` in the
    /// same block beats the inherit (one line, the plain pre's height).
    #[test]
    fn css_wide_rest_pre_wraps() {
        let mut e = css_wide_rest_engine();
        let h = |e: &mut crate::AetherEngine, id: &str| imgalt_rect(e, id).3;
        let (pp, pw, pv) = (h(&mut e, "pp"), h(&mut e, "pw"), h(&mut e, "pv"));
        assert!(pp > 5.0 && pp < 40.0, "the plain pre is one line ({pp})");
        assert!(pw > 1.5 * pp, "pre.w wraps like its normal parent ({pw} vs one line {pp})");
        assert!((pv - pp).abs() <= 0.5, "the later pre beats the inherit ({pv} vs {pp})");
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): `div.d { direction: inherit }` on a `dir=rtl` block under an ltr body lays out
    /// ltr: its ink is the plain ltr block's, starting in the left half; the plain `dir=rtl` block's starts in the right
    /// half (armed).
    #[test]
    fn css_wide_rest_direction_ltr() {
        let mut e = css_wide_rest_engine();
        let first = |c: &[usize]| c.iter().position(|&n| n > 0).unwrap_or(usize::MAX);
        let lref = css_wide_rest_columns(&mut e, "lref", 0, 400);
        let rref = css_wide_rest_columns(&mut e, "rref", 0, 400);
        let rd = css_wide_rest_columns(&mut e, "rd", 0, 400);
        assert!(lref.iter().sum::<usize>() > 20, "the reference run paints");
        assert!(first(&rref) > 200 && first(&rref) != usize::MAX, "the plain dir=rtl block starts right (armed)");
        assert!(first(&rd) < 200, "the inheriting block starts left");
        assert_eq!(rd, lref, "the inheriting block lays out as the ltr reference");
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): `ol.i { list-style-type: inherit }` and `ol.u { list-style-type: unset }` inside
    /// a `ul` item take the ul's disc past the ol's decimal tag road: their item's marker ink is the disc-styled ol twin's,
    /// and the plain ol's "1." differs (armed).
    #[test]
    fn css_wide_rest_list_ol_takes_disc() {
        let mut e = css_wide_rest_engine();
        let disc = css_wide_rest_marker(&mut e, "old");
        assert!(disc.iter().sum::<usize>() > 3, "the disc twin paints a marker");
        assert_ne!(css_wide_rest_marker(&mut e, "olp"), disc, "the plain ol's decimal is not the disc (armed)");
        for id in ["oli", "olu"] {
            assert_eq!(css_wide_rest_marker(&mut e, id), disc, "#{id}: the parent's disc");
        }
    }

    /// AETHERCSSWIDEINHERITEDREST (SR831): `ul ul.i { list-style-type: inherit }` takes the outer ul's disc past the depth
    /// match's circle: the disc-styled twin's marker ink, not the plain nested ul's (armed).
    #[test]
    fn css_wide_rest_list_nested_ul_takes_disc() {
        let mut e = css_wide_rest_engine();
        let disc = css_wide_rest_marker(&mut e, "uld");
        assert!(disc.iter().sum::<usize>() > 3, "the disc twin paints a marker");
        assert_ne!(css_wide_rest_marker(&mut e, "ulp"), disc, "the plain nested ul's circle is not the disc (armed)");
        assert_eq!(css_wide_rest_marker(&mut e, "uli"), disc, "the inheriting ul takes the outer disc");
    }

    // ---- AETHERCSSWIDENONINHERITEDINITIAL (LEDGER SR832): initial / unset on a non-inherited property beat an earlier rule (named tail block) ----

    /// AETHERCSSWIDENONINHERITEDINITIAL (SR832): one probe pair per row at 400 px wide, the rules in a sheet, each `.r`
    /// twin resetting what its class's rule set: margin-left (initial), padding (unset), a red background (initial), a
    /// 100px width (unset) and a 4px red border (initial); plus a `display: initial` div after a span beside a plain
    /// div after a span. Rows 12px apart.
    fn css_wide_initial_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div, section { margin: 0 0 12px }
            .m { margin-left: 20px }
            .m.r { margin-left: initial }
            .p { padding: 10px }
            .p.r { padding: unset }
            div.di { display: initial }
            .bg { background-color: rgb(255, 0, 0); height: 20px }
            .bg.r { background-color: initial }
            .w { width: 100px }
            .w.r { width: unset }
            .b { border: 4px solid rgb(255, 0, 0); height: 20px }
            .b.r { border: initial }
            </style></head><body style="margin:0;font:20px serif">
            <div id="m" class="m">Hello</div>
            <div id="mr" class="m r">Hello</div>
            <div id="p" class="p"><span id="pin">Hello</span></div>
            <div id="pr" class="p r"><span id="prin">Hello</span></div>
            <section><span id="dbsib">Hello</span><div id="db">World</div></section>
            <section><span id="disib">Hello</span><div id="di" class="di">World</div></section>
            <div id="bg" class="bg"></div>
            <div id="bgr" class="bg r"></div>
            <div id="w" class="w">Hi</div>
            <div id="wr" class="w r">Hi</div>
            <div id="b" class="b"></div>
            <div id="br" class="b r"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDENONINHERITEDINITIAL (SR832): `.m.r { margin-left: initial }` after `.m { margin-left: 20px }` puts
    /// the box at x 0 (the `.m` twin at 20, armed); `.p.r { padding: unset }` puts the span at the box's edge (the `.p`
    /// twin's span 10px in, armed).
    #[test]
    fn css_wide_initial_margin_padding() {
        let mut e = css_wide_initial_engine();
        let (ml, ..) = imgalt_rect(&mut e, "m");
        let (mrl, ..) = imgalt_rect(&mut e, "mr");
        assert!((ml - 20.0).abs() <= 0.5, "the .m box at 20 (armed): {ml}");
        assert!(mrl.abs() <= 0.5, "margin-left: initial is 0: {mrl}");
        let (pl, ..) = imgalt_rect(&mut e, "p");
        let (pinl, ..) = imgalt_rect(&mut e, "pin");
        let (prl, ..) = imgalt_rect(&mut e, "pr");
        let (prinl, ..) = imgalt_rect(&mut e, "prin");
        assert!((pinl - pl - 10.0).abs() <= 0.5, "the .p span 10px in (armed): {pinl} vs {pl}");
        assert!((prinl - prl).abs() <= 0.5, "padding: unset is 0: {prinl} vs {prl}");
    }

    /// AETHERCSSWIDENONINHERITEDINITIAL (SR832): `div { display: initial }` is inline (css-display-3: the initial value),
    /// not the tag's block: it sits on its sibling span's line, right of it, and paints; a plain div after a span starts
    /// a new line below it (armed).
    #[test]
    fn css_wide_initial_display_inline() {
        let mut e = css_wide_initial_engine();
        let (_, st, ..) = imgalt_rect(&mut e, "dbsib");
        let (_, bt, ..) = imgalt_rect(&mut e, "db");
        assert!(bt > st + 5.0, "a plain div after a span is on a new line (armed): {bt} vs {st}");
        let (il, it, iw, _) = imgalt_rect(&mut e, "disib");
        let (dl, dt, dw, dh) = imgalt_rect(&mut e, "di");
        assert!((dt - it).abs() <= 2.0, "display: initial sits on the span's line: {dt} vs {it}");
        assert!(dl >= il + iw - 1.0, "right of the span: {dl} vs {il} + {iw}");
        let x0 = dl.floor().max(0.0) as usize;
        let (x1, y0, y1) = ((dl + dw).ceil() as usize, dt.floor().max(0.0) as usize, (dt + dh).ceil() as usize);
        assert!(imgalt_ink(&e, x0, x1.min(e.width as usize), y0, y1.min(e.height as usize)) > 20, "the inline div paints");
    }

    /// AETHERCSSWIDENONINHERITEDINITIAL (SR832): `background-color: initial` after a red rule is transparent (no red
    /// ink; the `.bg` twin is red, armed); `border: initial` after a 4px red border paints no border (the `.b` twin's
    /// border is red, armed).
    #[test]
    fn css_wide_initial_background_border() {
        let mut e = css_wide_initial_engine();
        assert!(css_wide_color_ink(&mut e, "bg", css_wide_red) > 100, "the .bg box is red (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "bgr", css_wide_red), 0, "background-color: initial is transparent");
        assert!(css_wide_color_ink(&mut e, "b", css_wide_red) > 20, "the .b border is red (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "br", css_wide_red), 0, "border: initial paints no border");
    }

    /// AETHERCSSWIDENONINHERITEDINITIAL (SR832): `width: unset` after `width: 100px` is auto: the block fills its 400px
    /// container (the `.w` twin is 100, armed).
    #[test]
    fn css_wide_initial_width_auto() {
        let mut e = css_wide_initial_engine();
        let (_, _, w, _) = imgalt_rect(&mut e, "w");
        let (_, _, wr, _) = imgalt_rect(&mut e, "wr");
        assert!((w - 100.0).abs() <= 0.5, "the .w box is 100 (armed): {w}");
        assert!((wr - 400.0).abs() <= 0.5, "width: unset fills the container: {wr}");
    }

    // ---- AETHERCSSOMINHERITEDRESTREPORT (LEDGER SR842): getComputedStyle reads the resolved white-space / list-style-type (named tail block) ----

    /// AETHERCSSOMINHERITEDRESTREPORT (SR842): one probe per row; the rules in a sheet so SR831's inherit bits reach the
    /// paint map the report reads.
    fn cssom_inherited_rest_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            pre.w { white-space: inherit }
            ul ol.i { list-style-type: inherit }
            li.sq { list-style-type: square }
            </style></head><body style="margin:0;font:16px serif">
            <pre id="pre">a  b</pre>
            <div><pre id="prew" class="w">a  b</pre></div>
            <div id="nw" style="white-space:nowrap">a b</div>
            <div id="prex" style="white-space:pre">a  b</div>
            <div id="pw" style="white-space:pre-wrap">a b</div>
            <div id="pl" style="white-space:pre-line">a b</div>
            <div id="bs" style="white-space:break-spaces">a b</div>
            <textarea id="ta">a</textarea>
            <nobr id="nb">a b</nobr>
            <ol id="ol"><li id="olli">a</li></ol>
            <ul id="ul1"><li><ul id="ul2"><li><ul id="ul3"><li id="ul3li">a</li></ul></li></ul></li></ul>
            <ul><li><ol id="oli" class="i"><li id="olili">a</li></ol></li></ul>
            <ul><li id="sq" class="sq">a</li></ul>
            <ul id="none" style="list-style-type:none"><li>a</li></ul>
            <ol id="lr" style="list-style-type:lower-roman"><li>a</li></ol>
            <div id="plain">a</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn cssom_inherited_rest_cs(e: &mut crate::AetherEngine, id: &str, p: &str) -> String {
        e.js_engine.as_mut().unwrap().eval_string(&format!("getComputedStyle(document.getElementById('{id}')).{p}")).unwrap()
    }

    /// AETHERCSSOMINHERITEDRESTREPORT (SR842): white-space reads the paint code the renderer resolves (render/mod.rs's
    /// or_tag + `default_white_space`): `pre` is `pre` (was `normal`), an explicit `pre` is `pre` (was `nowrap`), SR831's
    /// `pre { white-space: inherit }` under a normal div reads `normal`, `textarea` pre-wrap, `nobr` nowrap.
    #[test]
    fn cssom_inherited_rest_white_space() {
        let mut e = cssom_inherited_rest_engine();
        for (id, want) in [
            ("pre", "pre"), ("prew", "normal"), ("nw", "nowrap"), ("prex", "pre"), ("pw", "pre-wrap"), ("pl", "pre-line"),
            ("bs", "break-spaces"), ("ta", "pre-wrap"), ("nb", "nowrap"), ("plain", "normal"),
        ] {
            assert_eq!(cssom_inherited_rest_cs(&mut e, id, "whiteSpace"), want, "#{id}: computed white-space");
        }
    }

    /// AETHERCSSOMINHERITEDRESTREPORT (SR842): list-style-type reads `render::list_style_of` (the code the marker paints):
    /// `ol` and its item decimal (was disc), `ul ul` circle, `ul ul ul` square, SR831's `ul ol { list-style-type: inherit }`
    /// the outer ul's disc, a declared square / none / lower-roman, a plain div the initial disc.
    #[test]
    fn cssom_inherited_rest_list_style() {
        let mut e = cssom_inherited_rest_engine();
        for (id, want) in [
            ("ol", "decimal"), ("olli", "decimal"), ("ul1", "disc"), ("ul2", "circle"), ("ul3", "square"), ("ul3li", "square"),
            ("oli", "disc"), ("olili", "disc"), ("sq", "square"), ("none", "none"), ("lr", "lower-roman"), ("plain", "disc"),
        ] {
            assert_eq!(cssom_inherited_rest_cs(&mut e, id, "listStyleType"), want, "#{id}: computed list-style-type");
        }
    }

    /// AETHERCSSOMINHERITEDRESTREPORT (SR842): the other columns of a plain div read as before the two emits changed.
    #[test]
    fn cssom_inherited_rest_other_columns_unchanged() {
        let mut e = cssom_inherited_rest_engine();
        for (p, want) in [
            ("display", "block"), ("position", "static"), ("color", "rgb(0, 0, 0)"), ("backgroundColor", "rgba(0, 0, 0, 0)"),
            ("fontSize", "16px"), ("fontWeight", "400"), ("fontStyle", "normal"), ("lineHeight", "normal"), ("textAlign", "start"),
            ("textDecorationLine", "none"), ("visibility", "visible"), ("borderTopWidth", "0px"), ("borderTopStyle", "none"),
            ("paddingTop", "0px"), ("marginTop", "0px"), ("flexDirection", "row"), ("direction", "ltr"),
        ] {
            assert_eq!(cssom_inherited_rest_cs(&mut e, "plain", p), want, "#plain: computed {p}");
        }
    }

    // ---- AETHERLINEHEIGHTZERO (LEDGER SR843): line-height 0 / 0px / 0% is a zero line box, apart from normal (named tail block) ----

    /// AETHERLINEHEIGHTZERO (SR843): two-line divs ("Hello Hello" in 60px) at line-height 0 / 0px / 0% and normal, a span
    /// under a zero and under a normal parent, and two paint probes in 100px wrappers (40px top padding): `p2` the two-line
    /// div at 0, `p1` a one-line "Hello" at 0.
    fn line_height_zero_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div id="z0" style="width:60px;line-height:0">Hello Hello</div>
            <div id="zpx" style="width:60px;line-height:0px">Hello Hello</div>
            <div id="zpct" style="width:60px;line-height:0%">Hello Hello</div>
            <div id="nrm" style="width:60px;line-height:normal">Hello Hello</div>
            <div style="line-height:0"><span id="i0">Hello</span></div>
            <div style="line-height:normal"><span id="inrm">Hello</span></div>
            <div id="w2" style="height:100px"><div id="p2" style="width:60px;padding-top:40px;line-height:0">Hello Hello</div></div>
            <div id="w1" style="height:100px"><div id="p1" style="width:60px;padding-top:40px;line-height:0">Hello</div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERLINEHEIGHTZERO (SR843): a zero line-height gives zero line boxes, so the two-line block is 0 px tall
    /// (Chromium); the `normal` twin is two lines tall (armed).
    #[test]
    fn line_height_zero_box_height() {
        let mut e = line_height_zero_engine();
        for id in ["z0", "zpx", "zpct"] {
            let h = imgalt_rect(&mut e, id).3;
            assert_eq!(h, 0.0, "#{id}: two zero line boxes");
        }
        let h = imgalt_rect(&mut e, "nrm").3;
        assert!(h > 20.0, "#nrm: two normal lines ({h})");
    }

    /// AETHERLINEHEIGHTZERO (SR843): getComputedStyle reads `0px` for 0 / 0px / 0% (it read `normal`), `normal` stays.
    #[test]
    fn line_height_zero_cssom() {
        let mut e = line_height_zero_engine();
        for id in ["z0", "zpx", "zpct"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "lineHeight"), "0px", "#{id}");
        }
        assert_eq!(font_shorthand_cs(&mut e, "nrm", "lineHeight"), "normal");
    }

    /// AETHERLINEHEIGHTZERO (SR843): a zero inherits as the zero (`0px`), `normal` as `normal`.
    #[test]
    fn line_height_zero_inherit() {
        let mut e = line_height_zero_engine();
        assert_eq!(font_shorthand_cs(&mut e, "i0", "lineHeight"), "0px");
        assert_eq!(font_shorthand_cs(&mut e, "inrm", "lineHeight"), "normal");
    }

    /// AETHERLINEHEIGHTZERO (SR843): the two lines of `p2` paint at one y: row by row, its inked rows are the one-line
    /// twin `p1`'s (the same "Hello" twice on top of itself), and some rows are inked (armed).
    #[test]
    fn line_height_zero_lines_overlap() {
        let mut e = line_height_zero_engine();
        let t2 = imgalt_rect(&mut e, "w2").1.round() as usize;
        let t1 = imgalt_rect(&mut e, "w1").1.round() as usize;
        let mut inked = 0;
        for k in 0..100 {
            let a = imgalt_ink(&e, 0, 60, t2 + k, t2 + k + 1) > 0;
            let b = imgalt_ink(&e, 0, 60, t1 + k, t1 + k + 1) > 0;
            assert_eq!(a, b, "row {k}: the two-line probe inks where the one-line twin does");
            inked += a as usize;
        }
        assert!(inked > 0, "the probes paint");
    }

    // ---- AETHERCSSWIDECOLORCONTROLS (LEDGER SR840): the form-control ink roads take color: inherit / unset (named tail block) ----

    /// AETHERCSSWIDECOLORCONTROLS (SR840): one control per row under a red parent: `.i { color: inherit }` on an input
    /// value, a textarea, a select, a file input, a placeholder-only input and a `<button>`; `.u { color: unset }` on an
    /// input; and the unstyled input / textarea / select / file twins (the UA black). Rows 12px apart.
    fn css_wide_controls_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .i { color: inherit }
            .u { color: unset }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(255, 0, 0)"><input id="inp" class="i" value="Hello"></div>
            <div style="color:rgb(255, 0, 0)"><input id="inpu" class="u" value="Hello"></div>
            <div style="color:rgb(255, 0, 0)"><textarea id="ta" class="i">Hello</textarea></div>
            <div style="color:rgb(255, 0, 0)"><select id="sel" class="i"><option>Hello</option></select></div>
            <div style="color:rgb(255, 0, 0)"><input id="fi" class="i" type="file"></div>
            <div style="color:rgb(255, 0, 0)"><input id="ph" class="i" placeholder="Hello"></div>
            <div style="color:rgb(255, 0, 0)"><button id="btn" class="i">Hello</button></div>
            <div style="color:rgb(255, 0, 0)"><input id="inp0" value="Hello"></div>
            <div style="color:rgb(255, 0, 0)"><textarea id="ta0">Hello</textarea></div>
            <div style="color:rgb(255, 0, 0)"><select id="sel0"><option>Hello</option></select></div>
            <div style="color:rgb(255, 0, 0)"><input id="fi0" type="file"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 700));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn css_wide_controls_black(c: (u8, u8, u8)) -> bool {
        c.0 < 90 && c.1 < 90 && c.2 < 90
    }

    /// AETHERCSSWIDECOLORCONTROLS (SR840): `color: inherit` on an input value, a textarea, a select (label and arrow) and
    /// a file input's caption / status paints the red parent; the input's cssom reads rgb(255, 0, 0).
    #[test]
    fn css_wide_controls_inherit_red() {
        let mut e = css_wide_controls_engine();
        for id in ["inp", "ta", "sel", "fi"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_red) > 3, "#{id} takes the red parent");
        }
        assert_eq!(font_shorthand_cs(&mut e, "inp", "color"), "rgb(255, 0, 0)", "cssom reads the parent's colour");
    }

    /// AETHERCSSWIDECOLORCONTROLS (SR840): `color: unset` on a control is inherit (css-cascade-4 §7.3.3): red.
    #[test]
    fn css_wide_controls_unset_red() {
        let mut e = css_wide_controls_engine();
        assert!(css_wide_color_ink(&mut e, "inpu", css_wide_red) > 3, "the unset input takes the red parent");
    }

    /// AETHERCSSWIDECOLORCONTROLS (SR840): with no colour rule a control keeps the UA FieldText / ButtonText black under
    /// the red parent (Chromium's light scheme): black ink, no red.
    #[test]
    fn css_wide_controls_unstyled_black() {
        let mut e = css_wide_controls_engine();
        for id in ["inp0", "ta0", "sel0", "fi0"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_controls_black) > 3, "#{id} paints black (armed)");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id} paints no red");
        }
    }

    /// AETHERCSSWIDECOLORCONTROLS (SR840): a placeholder stays the placeholder's grey under `color: inherit`: no red.
    #[test]
    fn css_wide_controls_placeholder_grey() {
        let mut e = css_wide_controls_engine();
        assert_eq!(css_wide_color_ink(&mut e, "ph", css_wide_red), 0, "the placeholder paints no red");
    }

    /// AETHERCSSWIDECOLORCONTROLS (SR840): `<button class=i>`'s label (child text on `inherit_element`'s road) is red.
    #[test]
    fn css_wide_controls_button_inherit_red() {
        let mut e = css_wide_controls_engine();
        assert!(css_wide_color_ink(&mut e, "btn", css_wide_red) > 3, "the button label takes the red parent");
    }

    // ---- AETHERCSSCOLORCURRENTCOLOR (LEDGER SR841): color: currentcolor is inherit; outline's currentColor pinned (named tail block) ----

    /// AETHERCSSCOLORCURRENTCOLOR (SR841): one row per probe at 20px serif under a red parent, the rules in a sheet:
    /// `.a { color: blue }` (`#ca`, the armed twin) and its `.a.b { color: currentcolor }` (`#cb`), a same-block
    /// `color: blue; color: currentColor` (`#cs`), a link `a.b2 { color: currentcolor }` (`#cl`, past the UA link blue),
    /// and an empty 40x20 box with `outline: 2px solid currentcolor` (`#co`). Rows 12px apart.
    fn currentcolor_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .a { color: blue }
            .a.b { color: currentcolor }
            a.b2 { color: currentcolor }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(255, 0, 0)"><span id="ca" class="a">Hello</span></div>
            <div style="color:rgb(255, 0, 0)"><span id="cb" class="a b">Hello</span></div>
            <div style="color:rgb(255, 0, 0)"><span id="cs" style="color:blue;color:currentColor">Hello</span></div>
            <div style="color:rgb(255, 0, 0)"><a id="cl" class="b2" href="#">Hello</a></div>
            <div style="color:rgb(255, 0, 0);padding:8px"><div id="co" style="width:40px;height:20px;margin:0;outline:2px solid currentcolor"></div></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCOLORCURRENTCOLOR (SR841): css-color-4 §6.2: `color: currentcolor` is `inherit`: after `.a { color:
    /// blue }`, in one block after `color: blue`, and on a link, each paints the red parent's ink (no blue) and cssom
    /// reads rgb(255, 0, 0); the `.a` twin paints blue (armed).
    #[test]
    fn currentcolor_color_is_inherit() {
        let mut e = currentcolor_engine();
        assert!(css_wide_color_ink(&mut e, "ca", css_wide_blue) > 3, "#ca paints blue (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "ca", css_wide_red), 0, "#ca paints no red (armed)");
        for id in ["cb", "cs", "cl"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_red) > 3, "#{id} paints the red parent's ink");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_blue), 0, "#{id} paints no blue");
            assert_eq!(font_shorthand_cs(&mut e, id, "color"), "rgb(255, 0, 0)", "#{id}: cssom");
        }
    }

    /// AETHERCSSCOLORCURRENTCOLOR (SR841): pinned: `outline: 2px solid currentcolor` on an empty box under a red parent
    /// paints red outline ink around it (outline_color Some(None) -> focusring's `current_color`), no blue.
    #[test]
    fn currentcolor_outline_pinned() {
        let mut e = currentcolor_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "co");
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (((l - 3.0).floor().max(0.0)) as usize, ((l + w + 3.0).ceil() as usize).min(sw));
        let (y0, y1) = (((t - 3.0).floor().max(0.0)) as usize, ((t + h + 3.0).ceil() as usize).min(sh));
        let (mut red, mut blue) = (0, 0);
        for y in y0..y1 {
            for x in x0..x1 {
                let c = imgalt_px(&e, x, y);
                red += css_wide_red(c) as usize;
                blue += css_wide_blue(c) as usize;
            }
        }
        assert!(red > 40, "the outline paints the element's red ({red} px)");
        assert_eq!(blue, 0, "no blue");
    }

    // ---- AETHERCSSOMDIRECTIONCOLUMN (LEDGER SR1000): getComputedStyle direction reads the render's rtl as the 21st report column (named tail block) ----

    /// AETHERCSSOMDIRECTIONCOLUMN (SR1000): a `dir=rtl` div and its children (a plain p, an inline `direction: ltr`, an
    /// earlier `p.e { direction: ltr }` beaten by `p.e.i { direction: inherit }`, a `dir=ltr` child), an inline
    /// `direction: rtl` div and its child, and a plain div.
    fn cssom_direction_column_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            p.e { direction: ltr }
            p.e.i { direction: inherit }
            </style></head><body style="margin:0;font:16px serif">
            <div id="r" dir="rtl"><p id="rc">a</p><p id="rl" style="direction:ltr">a</p><p id="re" class="e">a</p><p id="ri" class="e i">a</p><p id="rd" dir="ltr">a</p></div>
            <div id="c" style="direction:rtl"><p id="cc">a</p></div>
            <div id="plain">a</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMDIRECTIONCOLUMN (SR1000): `getComputedStyle(el).direction` for element `id`.
    fn cssom_direction_column_cs(e: &mut crate::AetherEngine, id: &str) -> String {
        e.js_engine.as_mut().unwrap().eval_string(&format!("getComputedStyle(document.getElementById('{id}')).direction")).unwrap()
    }

    /// AETHERCSSOMDIRECTIONCOLUMN (SR1000): the `dir` attribute maps onto direction and inherits (was ltr everywhere); an
    /// author `direction: ltr` and a `dir=ltr` child read ltr; SR831's `direction: inherit` beats the earlier ltr rule.
    #[test]
    fn cssom_direction_column_dir_attr() {
        let mut e = cssom_direction_column_engine();
        for (id, want) in [("r", "rtl"), ("rc", "rtl"), ("rl", "ltr"), ("re", "ltr"), ("ri", "rtl"), ("rd", "ltr"), ("plain", "ltr")] {
            assert_eq!(cssom_direction_column_cs(&mut e, id), want, "#{id}: computed direction");
        }
    }

    /// AETHERCSSOMDIRECTIONCOLUMN (SR1000): an author `direction: rtl` reads rtl and its child inherits it.
    #[test]
    fn cssom_direction_column_css() {
        let mut e = cssom_direction_column_engine();
        for (id, want) in [("c", "rtl"), ("cc", "rtl")] {
            assert_eq!(cssom_direction_column_cs(&mut e, id), want, "#{id}: computed direction");
        }
    }

    /// AETHERCSSOMDIRECTIONCOLUMN (SR1000): direction is the 21st column, appended at the END, so the twenty positions
    /// the a11y tree indexes (display 0, visibility 12) and the style oracle's fixture prefix do not move.
    #[test]
    fn cssom_direction_column_report_width() {
        assert_eq!(crate::css::REPORT_PROPS.len(), crate::css::REPORT_COLS);
        assert_eq!(crate::css::REPORT_COLS, 25); // SR1036 fold: unicode-bidi (SR1008) is column 21, SR1036 appends white-space-collapse / text-wrap-mode / text-decoration-color as 22-24
        assert_eq!(crate::css::REPORT_PROPS[20], "direction");
        assert_eq!((crate::css::REPORT_PROPS[0], crate::css::REPORT_PROPS[12]), ("display", "visibility"));
    }

    // ---- AETHERCSSCONTROLUACOLOR (LEDGER SR1001): an unset control paints the UA ButtonText / FieldText black, not the parent's colour (named tail block) ----

    /// AETHERCSSCONTROLUACOLOR (SR1001): one control per row under a red parent, rows 12px apart: an unstyled `<button>`,
    /// `button.i { color: inherit }`, `button.u { color: unset }`, an unstyled `<select>` and `<input type=submit>`, a
    /// `<button>` inside `a.ai { color: inherit }`, and a `<button>` with an author green.
    fn control_ua_color_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            button.i { color: inherit }
            button.u { color: unset }
            a.ai { color: inherit }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(255, 0, 0)"><button id="b0">Hello</button></div>
            <div style="color:rgb(255, 0, 0)"><button id="bi" class="i">Hello</button></div>
            <div style="color:rgb(255, 0, 0)"><button id="bu" class="u">Hello</button></div>
            <div style="color:rgb(255, 0, 0)"><select id="s0"><option>Hello</option></select></div>
            <div style="color:rgb(255, 0, 0)"><input id="sub0" type="submit" value="Go"></div>
            <div style="color:rgb(255, 0, 0)"><a class="ai" href="#"><button id="ba">Hello</button></a></div>
            <div style="color:rgb(255, 0, 0)"><button id="bg" style="color:rgb(0, 160, 0)">Hello</button></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): an unstyled `<button>` under a red parent paints its label black (Chromium's
    /// ButtonText), no red.
    #[test]
    fn control_ua_color_button_black() {
        let mut e = control_ua_color_engine();
        assert!(css_wide_color_ink(&mut e, "b0", css_wide_controls_black) > 3, "the unstyled button paints black (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "b0", css_wide_red), 0, "the unstyled button paints no red");
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): `button { color: inherit }` opts back into the parent: red.
    #[test]
    fn control_ua_color_button_inherit_red() {
        let mut e = control_ua_color_engine();
        assert!(css_wide_color_ink(&mut e, "bi", css_wide_red) > 3, "the inherit button takes the red parent");
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): `button { color: unset }` is inherit for an inherited property: red.
    #[test]
    fn control_ua_color_button_unset_red() {
        let mut e = control_ua_color_engine();
        assert!(css_wide_color_ink(&mut e, "bu", css_wide_red) > 3, "the unset button takes the red parent");
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): an unstyled `<select>`'s label and `<input type=submit>`'s caption (SR840's
    /// reads) stay black under the red parent: no red.
    #[test]
    fn control_ua_color_select_submit_black() {
        let mut e = control_ua_color_engine();
        for id in ["s0", "sub0"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_controls_black) > 3, "#{id} paints black (armed)");
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id} paints no red");
        }
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): getComputedStyle reads the UA black on the unstyled button and submit input
    /// (Chromium: rgb(0, 0, 0)), and the parent's red on the inherit button.
    #[test]
    fn control_ua_color_cssom() {
        let mut e = control_ua_color_engine();
        assert_eq!(font_shorthand_cs(&mut e, "b0", "color"), "rgb(0, 0, 0)", "the unstyled button reads black");
        assert_eq!(font_shorthand_cs(&mut e, "sub0", "color"), "rgb(0, 0, 0)", "the unstyled submit input reads black");
        assert_eq!(font_shorthand_cs(&mut e, "bi", "color"), "rgb(255, 0, 0)", "the inherit button reads the parent's red");
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): a `<button>` inside `a { color: inherit }` under red: the link takes red, the
    /// button's own UA default beats it: black, no red.
    #[test]
    fn control_ua_color_in_inherit_link_black() {
        let mut e = control_ua_color_engine();
        assert!(css_wide_color_ink(&mut e, "ba", css_wide_controls_black) > 3, "the button in the link paints black (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "ba", css_wide_red), 0, "the button in the link paints no red");
    }

    /// AETHERCSSCONTROLUACOLOR (SR1001): an author colour on the control wins as before: green, no red.
    #[test]
    fn control_ua_color_author_wins() {
        let mut e = control_ua_color_engine();
        assert!(css_wide_color_ink(&mut e, "bg", css_wide_green) > 3, "the author green wins");
        assert_eq!(css_wide_color_ink(&mut e, "bg", css_wide_red), 0, "the author-green button paints no red");
    }

    // ---- AETHERCSSCURRENTCOLORREST (LEDGER SR849): border / background currentColor through the element's ink (named tail block) ----

    /// AETHERCSSCURRENTCOLORREST (SR849): one 40x20 box per row under a red parent, rows 12px apart: `#bb` `border: 2px
    /// solid currentcolor`; `#bl` `border: 2px solid blue; border-left: 2px solid currentcolor`; `#bg` `color: blue;
    /// background-color: currentcolor`; `#bc` `.x { border: 2px solid currentcolor } .x.y { color: rgb(0, 160, 0) }`;
    /// `#bu` `border: 2px solid` (no colour: the initial currentcolor).
    fn currentcolorrest_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .x { border: 2px solid currentcolor }
            .x.y { color: rgb(0, 160, 0) }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(255, 0, 0)"><div id="bb" style="width:40px;height:20px;margin:0;border:2px solid currentcolor"></div></div>
            <div style="color:rgb(255, 0, 0)"><div id="bl" style="width:40px;height:20px;margin:0;border:2px solid blue;border-left:2px solid currentcolor"></div></div>
            <div style="color:rgb(255, 0, 0)"><div id="bg" style="width:40px;height:20px;margin:0;color:blue;background-color:currentcolor"></div></div>
            <div style="color:rgb(255, 0, 0)"><div id="bc" class="x y" style="width:40px;height:20px;margin:0"></div></div>
            <div style="color:rgb(255, 0, 0)"><div id="bu" style="width:40px;height:20px;margin:0;border:2px solid"></div></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn currentcolorrest_grey(c: (u8, u8, u8)) -> bool {
        c == (128, 128, 128)
    }

    /// AETHERCSSCURRENTCOLORREST (SR849): `border: 2px solid currentcolor` paints the red parent's ink (no grey), and so
    /// does an uncoloured `border: 2px solid` (the initial currentcolor, as Chromium); cssom reads rgb(255, 0, 0).
    #[test]
    fn currentcolorrest_border_paints_ink() {
        let mut e = currentcolorrest_engine();
        for id in ["bb", "bu"] {
            assert!(css_wide_color_ink(&mut e, id, css_wide_red) > 40, "#{id} paints a red border");
            assert_eq!(css_wide_color_ink(&mut e, id, currentcolorrest_grey), 0, "#{id} paints no grey");
            assert_eq!(font_shorthand_cs(&mut e, id, "borderLeftColor"), "rgb(255, 0, 0)", "#{id}: cssom");
        }
    }

    /// AETHERCSSCURRENTCOLORREST (SR849): `border-left: 2px solid currentcolor` after `border: 2px solid blue`: the left
    /// edge is red, the top edge stays blue; cssom borderLeftColor rgb(255, 0, 0).
    #[test]
    fn currentcolorrest_border_one_side() {
        let mut e = currentcolorrest_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "bl");
        let left = imgalt_px(&e, (l + 1.0) as usize, (t + h / 2.0) as usize);
        let top = imgalt_px(&e, (l + w / 2.0) as usize, (t + 1.0) as usize);
        assert!(css_wide_red(left), "the left edge is the element's red ink: {left:?}");
        assert!(css_wide_blue(top), "the top edge stays blue: {top:?}");
        assert_eq!(font_shorthand_cs(&mut e, "bl", "borderLeftColor"), "rgb(255, 0, 0)", "#bl: cssom");
    }

    /// AETHERCSSCURRENTCOLORREST (SR849): `color: blue; background-color: currentcolor` fills blue (the fill is the
    /// element's own text ink, css-color-4 §6.2), no red; cssom backgroundColor rgb(0, 0, 255).
    #[test]
    fn currentcolorrest_background_fill() {
        let mut e = currentcolorrest_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "bg");
        let mid = imgalt_px(&e, (l + w / 2.0) as usize, (t + h / 2.0) as usize);
        assert!(css_wide_blue(mid), "the fill is blue: {mid:?}");
        assert!(css_wide_color_ink(&mut e, "bg", css_wide_blue) > 400, "the box fills blue");
        assert_eq!(css_wide_color_ink(&mut e, "bg", css_wide_red), 0, "no red");
        assert_eq!(font_shorthand_cs(&mut e, "bg", "backgroundColor"), "rgb(0, 0, 255)", "#bg: cssom");
    }

    /// AETHERCSSCURRENTCOLORREST (SR849): paint-time resolution: `.x { border: 2px solid currentcolor }` with a later
    /// `.x.y { color: rgb(0, 160, 0) }` paints a green border, no red; cssom borderLeftColor rgb(0, 160, 0).
    #[test]
    fn currentcolorrest_later_color_rule() {
        let mut e = currentcolorrest_engine();
        assert!(css_wide_color_ink(&mut e, "bc", css_wide_green) > 40, "#bc paints a green border");
        assert_eq!(css_wide_color_ink(&mut e, "bc", css_wide_red), 0, "#bc paints no red");
        assert_eq!(font_shorthand_cs(&mut e, "bc", "borderLeftColor"), "rgb(0, 160, 0)", "#bc: cssom");
    }

    /// AETHERCSSCURRENTCOLORREST (SR849): the canvas: `html { color: rgb(0, 160, 0); background-color: currentcolor }`
    /// paints the canvas green far from the content (canvas_background through focusring's `current_color`).
    #[test]
    fn currentcolorrest_canvas() {
        let html = r##"<!doctype html><html style="color:rgb(0, 160, 0);background-color:currentcolor"><body style="margin:0"><p>hi</p></body></html>"##;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 600));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let c = imgalt_px(&e, 390, 590);
        assert!(css_wide_green(c), "the canvas is the root's green ink: {c:?}");
    }

    // ---- AETHERCSSWIDENONINHERITEDREST (LEDGER SR848): initial / unset on the rest of the non-inherited arms beat an earlier rule (named tail block) ---- // SR848 fold: helper renamed css_wide_noninherited_rest_engine (SR831 owns css_wide_rest_engine), as SR1014/SR1015/SR1016 did

    /// AETHERCSSWIDENONINHERITEDREST (SR848): one armed / reset pair per row at 400 px wide, each `.r` twin resetting
    /// what its class's rule set: position absolute (initial), float left (unset), opacity 0.5 on red (initial), a
    /// 100px translate (initial), overflow hidden on a 10px box with a 40px red child (initial), a column flexbox
    /// (initial), and a relative box 30px down (top: initial).
    fn css_wide_noninherited_rest_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div, section { margin: 0 0 12px }
            .pa { position: absolute; top: 0; left: 200px; width: 50px; height: 30px }
            .pa.r { position: initial }
            .fl { float: left; width: 50px; height: 40px }
            .fl.r { float: unset }
            .op { opacity: 0.5; background-color: rgb(255, 0, 0); height: 20px }
            .op.r { opacity: initial }
            .tr { transform: translateX(100px); width: 50px; height: 20px; background-color: rgb(255, 0, 0) }
            .tr.r { transform: initial }
            .ov { overflow: hidden; height: 10px; margin-bottom: 50px }
            .ov.r { overflow: initial }
            .fd { display: flex; flex-direction: column }
            .fd.r { flex-direction: initial }
            .tp { position: relative; top: 30px }
            .tp.r { top: initial }
            </style></head><body style="margin:0;font:20px serif">
            <section id="pasec"><div id="pa" class="pa"></div><div id="pas">B</div></section>
            <section id="parsec"><div id="par" class="pa r"></div><div id="pars">B</div></section>
            <section><div id="fl" class="fl"></div><span id="fls">Hi</span></section>
            <section style="clear:both"><div id="flr" class="fl r"></div><span id="flrs">Hi</span></section>
            <div id="op" class="op"></div>
            <div id="opr" class="op r"></div>
            <div id="tr" class="tr"></div>
            <div id="trr" class="tr r"></div>
            <div id="ov" class="ov"><div style="height:40px;background-color:rgb(255,0,0)"></div></div>
            <div id="ovr" class="ov r"><div style="height:40px;background-color:rgb(255,0,0)"></div></div>
            <div class="fd"><span id="fd1">A</span><span id="fd2">B</span></div>
            <div class="fd r"><span id="fdr1">A</span><span id="fdr2">B</span></div>
            <section><div id="tps">A</div><div id="tp" class="tp">B</div></section>
            <section><div id="tprs">A</div><div id="tpr" class="tp r">B</div></section>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 1000));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): the red pixels in a surface rect (clamped).
    fn css_wide_rest_red(e: &crate::AetherEngine, x0: usize, x1: usize, y0: usize, y1: usize) -> usize {
        let (sw, sh) = (e.width as usize, e.height as usize);
        let mut n = 0;
        for y in y0.min(sh)..y1.min(sh) {
            for x in x0.min(sw)..x1.min(sw) {
                if css_wide_red(imgalt_px(e, x, y)) {
                    n += 1;
                }
            }
        }
        n
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `position: initial` after `absolute` lays the box in flow: its sibling
    /// sits below it; the absolute twin (inset, so not tree.rs's static-position fallback) takes no space, so its
    /// sibling is at the section's top (armed).
    #[test]
    fn css_wide_rest_position_in_flow() {
        let mut e = css_wide_noninherited_rest_engine();
        let (_, st, ..) = imgalt_rect(&mut e, "pasec");
        let (_, sbt, ..) = imgalt_rect(&mut e, "pas");
        assert!((sbt - st).abs() <= 1.0, "the absolute box takes no space (armed): {sbt} vs {st}");
        let (_, rt, ..) = imgalt_rect(&mut e, "par");
        let (_, rbt, ..) = imgalt_rect(&mut e, "pars");
        assert!(rbt >= rt + 29.0, "position: initial is static: the sibling below the 30px box: {rbt} vs {rt}");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `float: unset` after `left`: the text no longer wraps beside the 50px box
    /// but sits below it (the floated twin's text starts right of it, armed).
    #[test]
    fn css_wide_rest_float_unset() {
        let mut e = css_wide_noninherited_rest_engine();
        let (sl, ..) = imgalt_rect(&mut e, "fls");
        assert!(sl >= 49.0, "the float's text beside it (armed): {sl}");
        let (_, ft, ..) = imgalt_rect(&mut e, "flr");
        let (rl, rt, ..) = imgalt_rect(&mut e, "flrs");
        assert!(rl < 5.0, "float: unset: the text at the line start: {rl}");
        assert!(rt >= ft + 39.0, "float: unset: the text below the 40px box: {rt} vs {ft}");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `opacity: initial` after 0.5 paints full red (the 0.5 twin is pink, armed).
    #[test]
    fn css_wide_rest_opacity_full_ink() {
        let mut e = css_wide_noninherited_rest_engine();
        assert_eq!(css_wide_color_ink(&mut e, "op", css_wide_red), 0, "opacity 0.5 red is pink (armed)");
        assert!(css_wide_color_ink(&mut e, "opr", css_wide_red) > 100, "opacity: initial paints full red");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `transform: initial` after `translateX(100px)` paints the 50px box at its
    /// untransformed x (red at 0..50, none at 100..150; the translated twin the reverse, armed).
    #[test]
    fn css_wide_rest_transform_untransformed() {
        let mut e = css_wide_noninherited_rest_engine();
        let (_, t, _, h) = imgalt_rect(&mut e, "tr");
        let (y0, y1) = (t.ceil() as usize + 2, (t + h).floor() as usize - 2);
        assert!(css_wide_rest_red(&e, 102, 148, y0, y1) > 100, "the translated box at 100 (armed)");
        assert_eq!(css_wide_rest_red(&e, 2, 48, y0, y1), 0, "nothing at 0 (armed)");
        let (_, t, _, h) = imgalt_rect(&mut e, "trr");
        let (y0, y1) = (t.ceil() as usize + 2, (t + h).floor() as usize - 2);
        assert!(css_wide_rest_red(&e, 2, 48, y0, y1) > 100, "transform: initial: the box at 0");
        assert_eq!(css_wide_rest_red(&e, 102, 148, y0, y1), 0, "transform: initial: nothing at 100");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `overflow: initial` after `hidden`: the 40px red child paints below its
    /// 10px parent (the hidden twin clips it, armed).
    #[test]
    fn css_wide_rest_overflow_paints() {
        let mut e = css_wide_noninherited_rest_engine();
        let (_, t, ..) = imgalt_rect(&mut e, "ov");
        let t = t.ceil() as usize;
        assert_eq!(css_wide_rest_red(&e, 0, 400, t + 14, t + 36), 0, "overflow: hidden clips the child (armed)");
        let (_, t, ..) = imgalt_rect(&mut e, "ovr");
        let t = t.ceil() as usize;
        assert!(css_wide_rest_red(&e, 0, 400, t + 14, t + 36) > 1000, "overflow: initial: the child paints below");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `flex-direction: initial` after `column` puts the items in a row (the
    /// column twin stacks them, armed).
    #[test]
    fn css_wide_rest_flex_direction_row() {
        let mut e = css_wide_noninherited_rest_engine();
        let (_, t1, ..) = imgalt_rect(&mut e, "fd1");
        let (_, t2, ..) = imgalt_rect(&mut e, "fd2");
        assert!(t2 >= t1 + 10.0, "the column twin stacks (armed): {t2} vs {t1}");
        let (l1, t1, w1, _) = imgalt_rect(&mut e, "fdr1");
        let (l2, t2, ..) = imgalt_rect(&mut e, "fdr2");
        assert!((t2 - t1).abs() <= 2.0, "flex-direction: initial: one row: {t2} vs {t1}");
        assert!(l2 >= l1 + w1 - 1.0, "flex-direction: initial: the second item right of the first: {l2} vs {l1} + {w1}");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `top: initial` on a relative box is auto: no offset (the twin is 30px
    /// further down from its sibling, armed).
    #[test]
    fn css_wide_rest_top_no_offset() {
        let mut e = css_wide_noninherited_rest_engine();
        let (_, s, ..) = imgalt_rect(&mut e, "tps");
        let (_, b, ..) = imgalt_rect(&mut e, "tp");
        let (_, rs, ..) = imgalt_rect(&mut e, "tprs");
        let (_, rb, ..) = imgalt_rect(&mut e, "tpr");
        assert!(((b - s) - (rb - rs) - 30.0).abs() <= 1.0, "top: initial drops the 30px offset: {} vs {}", b - s, rb - rs);
    }

    // ---- AETHERCSSWIDEBACKGROUNDIMAGENONE (LEDGER SR1015): an explicit background-image none clears an earlier rule's image (named tail block) ----

    /// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): one 20px row per probe at 400 px wide, rows 12px apart, the image a
    /// 2x2 opaque red PNG data: URL (decoded synchronously, tiled): `.im` and its `.im.r { background-image: none }`
    /// twin, `.ii.r { background-image: initial }`, a green-plus-image `.bi` and its `.bi.r { background: initial }`
    /// twin, `.bn.r { background: none }`, `.im.r.u` (a later url after none), and a green-plus-image `.ic` whose
    /// `.ic.r { background-image: none }` twin keeps the green.
    fn css_wide_bgnone_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div { margin: 0 0 12px; height: 20px }
            .im, .ii, .bn { background-image: url(PNG) }
            .im.r { background-image: none }
            .ii.r { background-image: initial }
            .bn.r { background: none }
            .im.r.u { background-image: url(PNG) }
            .bi { background: rgb(0, 160, 0) url(PNG) }
            .bi.r { background: initial }
            .ic { background: rgb(0, 160, 0) url(PNG) }
            .ic.r { background-image: none }
            </style></head><body style="margin:0">
            <div id="im" class="im"></div>
            <div id="imr" class="im r"></div>
            <div id="iir" class="ii r"></div>
            <div id="bi" class="bi"></div>
            <div id="bir" class="bi r"></div>
            <div id="bnr" class="bn r"></div>
            <div id="imu" class="im r u"></div>
            <div id="ic" class="ic"></div>
            <div id="icr" class="ic r"></div>
        </body></html>"#
            .replace("PNG", "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=");
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): `background-image: none` / `initial` and `background: none` after an
    /// image rule paint no image ink (the `.im` twin paints red, armed).
    #[test]
    fn css_wide_bgnone_image_cleared() {
        let mut e = css_wide_bgnone_engine();
        assert!(css_wide_color_ink(&mut e, "im", css_wide_red) > 100, "the .im image paints red (armed)");
        for id in ["imr", "iir", "bnr"] {
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id} paints no image");
        }
    }

    /// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): `background: initial` after a green colour plus an image resets both:
    /// no red, no green (the `.bi` twin paints the image, armed).
    #[test]
    fn css_wide_bgnone_shorthand_initial() {
        let mut e = css_wide_bgnone_engine();
        assert!(css_wide_color_ink(&mut e, "bi", css_wide_red) > 100, "the .bi image paints (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "bir", css_wide_red), 0, "background: initial: no image");
        assert_eq!(css_wide_color_ink(&mut e, "bir", css_wide_green), 0, "background: initial: transparent");
    }

    /// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): a later url after none paints; `background-image: none` over a green
    /// colour plus an image keeps the green (the `.ic` twin paints red over it, armed).
    #[test]
    fn css_wide_bgnone_later_url_and_colour() {
        let mut e = css_wide_bgnone_engine();
        assert!(css_wide_color_ink(&mut e, "imu", css_wide_red) > 100, "a later url after none paints");
        assert!(css_wide_color_ink(&mut e, "ic", css_wide_red) > 100, "the .ic image covers the green (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "icr", css_wide_red), 0, "background-image: none: no image");
        assert!(css_wide_color_ink(&mut e, "icr", css_wide_green) > 100, "the green colour stays");
    }

    // ---- AETHERCSSALIGNCONTENTFIELD (LEDGER SR1016): align-content its own field; column-gap an explicit normal (named tail block) ----

    /// AETHERCSSALIGNCONTENTFIELD (SR1016): wrapping 100 x 200 flex containers, the rules in a sheet. `.sb` two 100 x 20
    /// lines with `align-content: space-between`, `.sb.r` the same with a later `align-content: initial` (normal =
    /// stretch: two 100px lines), `.ie` align-items center + align-content flex-end ([a 50x20, b 50x10] then [c 100x20]).
    fn align_content_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; flex-wrap: wrap; width: 100px; height: 200px; margin: 0 0 10px }
            .sb { align-content: space-between }
            .sb.r { align-content: initial }
            .ie { align-items: center; align-content: flex-end }
            </style></head><body style="margin:0">
            <div id="sb" class="f sb"><div id="sb1" style="width:100px;height:20px"></div><div id="sb2" style="width:100px;height:20px"></div></div>
            <div id="sbr" class="f sb r"><div id="sbr1" style="width:100px;height:20px"></div><div id="sbr2" style="width:100px;height:20px"></div></div>
            <div id="ie" class="f ie"><div id="iea" style="width:50px;height:20px"></div><div id="ieb" style="width:50px;height:10px"></div><div id="iec" style="width:100px;height:20px"></div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 800));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSALIGNCONTENTFIELD (SR1016): space-between puts the second line at the container's bottom (180), a later
    /// `align-content: initial` gives normal's stretched lines (100), and align-items center beside align-content
    /// flex-end both apply (160 / 165 / 180): the shared arm wrote align-items for both.
    #[test]
    fn align_content_space_between_engine() {
        let mut e = align_content_engine();
        let top = |e: &mut crate::AetherEngine, id: &str, c: &str| imgalt_rect(e, id).1 - imgalt_rect(e, c).1;
        assert!((top(&mut e, "sb1", "sb") - 0.0).abs() <= 0.5, "space-between: the first line at the top");
        assert!((top(&mut e, "sb2", "sb") - 180.0).abs() <= 0.5, "space-between: the second line at the bottom: {}", top(&mut e, "sb2", "sb"));
        assert!((top(&mut e, "sbr2", "sbr") - 100.0).abs() <= 0.5, "initial = normal = stretch: {}", top(&mut e, "sbr2", "sbr"));
        assert!((top(&mut e, "iea", "ie") - 160.0).abs() <= 0.5, "flex-end packs the lines at the bottom: {}", top(&mut e, "iea", "ie"));
        assert!((top(&mut e, "ieb", "ie") - 165.0).abs() <= 0.5, "align-items center inside its line: {}", top(&mut e, "ieb", "ie"));
        assert!((top(&mut e, "iec", "ie") - 180.0).abs() <= 0.5, "the second line: {}", top(&mut e, "iec", "ie"));
    }

    /// AETHERCSSALIGNCONTENTFIELD (SR1016): a two-column 410px div at 20px with red blocks: `column-gap: 40px` leaves
    /// x 220 in the gutter (185..225, white), a later rule's `column-gap: normal` is 1em (gutter 195..215), so x 220 is
    /// the second column's red; `initial` the same. Before, normal kept 40px.
    #[test]
    fn column_gap_normal_engine() {
        let html = r#"<!doctype html><html><head><style>
            .mc { width: 410px; column-count: 2; column-gap: 40px; font-size: 20px; margin: 0 0 20px }
            .mc div { height: 20px; background-color: rgb(255, 0, 0) }
            .mc.n { column-gap: normal }
            .mc.i { column-gap: initial }
            </style></head><body style="margin:0">
            <div class="mc"><div></div><div></div></div>
            <div class="mc n"><div></div><div></div></div>
            <div class="mc i"><div></div><div></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(500, 200));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(imgalt_px(&e, 100, 10), (255, 0, 0), "the first column paints red (armed)");
        assert_ne!(imgalt_px(&e, 220, 10), (255, 0, 0), "column-gap: 40px: x 220 is gutter (armed)");
        assert_eq!(imgalt_px(&e, 220, 50), (255, 0, 0), "column-gap: normal after 40px: 1em gutter, x 220 is column two");
        assert_eq!(imgalt_px(&e, 220, 90), (255, 0, 0), "column-gap: initial after 40px: 1em gutter");
    }

    // ---- AETHERCSSBORDERSIDECOLOR (LEDGER SR1014): the border-*-color longhands paint one side (named tail block) ----

    /// AETHERCSSBORDERSIDECOLOR (SR1014): one 40x20 box per row, rows 12px apart, under a green parent: `#sl` `border: 2px
    /// solid blue; border-left-color: red`; `#st` / `#sr` / `#sb` the same with the top / right / bottom longhand; `#sc`
    /// `border-left-color: currentcolor` on a blue border; `#sn` `border-top-color: red` alone (no border); `#si` the sheet's
    /// `.i { border: 2px solid blue; border-left-color: red } .i.j { border-left-color: initial }`.
    fn border_side_color_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .i { border: 2px solid blue; border-left-color: red }
            .i.j { border-left-color: initial }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="sl" style="width:40px;height:20px;border:2px solid blue;border-left-color:red"></div>
            <div id="st" style="width:40px;height:20px;border:2px solid blue;border-top-color:red"></div>
            <div id="sr" style="width:40px;height:20px;border:2px solid blue;border-right-color:red"></div>
            <div id="sb" style="width:40px;height:20px;border:2px solid blue;border-bottom-color:red"></div>
            <div id="sc" style="width:40px;height:20px;border:2px solid blue;border-left-color:currentcolor"></div>
            <div id="sn" style="width:40px;height:20px;border-top-color:red"></div>
            <div id="si" class="i j" style="width:40px;height:20px"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERSIDECOLOR (SR1014): the four edge midpoints of `id`'s border box, 1px inside: [top, right, bottom, left].
    fn border_side_color_edges(e: &mut crate::AetherEngine, id: &str) -> [(u8, u8, u8); 4] {
        let (l, t, w, h) = imgalt_rect(e, id);
        [
            imgalt_px(e, (l + w / 2.0) as usize, (t + 1.0) as usize),
            imgalt_px(e, (l + w - 1.0) as usize, (t + h / 2.0) as usize),
            imgalt_px(e, (l + w / 2.0) as usize, (t + h - 1.0) as usize),
            imgalt_px(e, (l + 1.0) as usize, (t + h / 2.0) as usize),
        ]
    }

    /// AETHERCSSBORDERSIDECOLOR (SR1014): each longhand paints its own side red, the other three stay blue; cssom
    /// borderLeftColor reads rgb(255, 0, 0) for `#sl` and the blue for the others.
    #[test]
    fn border_side_color_paints_one_side() {
        let mut e = border_side_color_engine();
        for (id, side) in [("st", 0), ("sr", 1), ("sb", 2), ("sl", 3)] {
            let edges = border_side_color_edges(&mut e, id);
            for (i, c) in edges.iter().enumerate() {
                if i == side {
                    assert!(css_wide_red(*c), "#{id}: side {i} is red: {c:?}");
                } else {
                    assert!(css_wide_blue(*c), "#{id}: side {i} stays blue: {c:?}");
                }
            }
        }
        assert_eq!(font_shorthand_cs(&mut e, "sl", "borderLeftColor"), "rgb(255, 0, 0)", "#sl: cssom");
        assert_eq!(font_shorthand_cs(&mut e, "st", "borderLeftColor"), "rgb(0, 0, 255)", "#st: the left stays blue");
    }

    /// AETHERCSSBORDERSIDECOLOR (SR1014): `border-left-color: currentcolor` paints the element's green ink (SR849's bit);
    /// `initial` after red is the initial currentcolor (SR832's table): green, no red.
    #[test]
    fn border_side_color_currentcolor_and_initial() {
        let mut e = border_side_color_engine();
        for id in ["sc", "si"] {
            let edges = border_side_color_edges(&mut e, id);
            assert!(css_wide_green(edges[3]), "#{id}: the left edge is the green ink: {:?}", edges[3]);
            assert!(css_wide_blue(edges[0]), "#{id}: the top stays blue: {:?}", edges[0]);
            assert_eq!(css_wide_color_ink(&mut e, id, css_wide_red), 0, "#{id}: no red");
            assert_eq!(font_shorthand_cs(&mut e, id, "borderLeftColor"), "rgb(0, 160, 0)", "#{id}: cssom");
        }
    }

    /// AETHERCSSBORDERSIDECOLOR (SR1014): `border-top-color: red` on a borderless box paints no ink (its style is none,
    /// as Chromium); armed: `#st` paints red.
    #[test]
    fn border_side_color_alone_paints_nothing() {
        let mut e = border_side_color_engine();
        assert_eq!(css_wide_color_ink(&mut e, "sn", css_wide_red), 0, "#sn paints no red");
        assert!(css_wide_color_ink(&mut e, "st", css_wide_red) > 40, "armed: #st paints a red top");
    }

    // ---- AETHERCSSWIDEALL (LEDGER SR1021): css-cascade-4 §3.1 the all shorthand over ONE longhand list (named tail block) ----

    /// AETHERCSSWIDEALL (SR1021): a blue parent holding `.x` (red, margin-left 20px, bold, block) and its `.x.r` twin
    /// (`all: unset`), each alone in a section and each after a span; a red parent holding an `all: initial` div and a
    /// plain one; an `all: unset; color: green` div. 400 px wide, rows 12px apart.
    fn css_wide_all_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            section { display: block; margin: 0 0 12px }
            .x { color: rgb(255, 0, 0); margin-left: 20px; font-weight: bold; display: block }
            .x.r { all: unset }
            .i { all: initial }
            .g { all: unset; color: green }
            </style></head><body style="margin:0;font:20px serif">
            <div style="color:rgb(0, 0, 255)">
              <section><div id="ax" class="x">Hello</div></section>
              <section><div id="axr" class="x r">Hello</div></section>
              <section><span id="sx">Hi</span><div id="bx" class="x">World</div></section>
              <section><span id="sxr">Hi</span><div id="bxr" class="x r">World</div></section>
            </div>
            <div style="color:rgb(255, 0, 0)"><div id="ai" class="i">Hello</div><div id="ai0">Hello</div></div>
            <section><div id="ag" class="g">Hello</div></section>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDEALL (SR1021): `.x.r { all: unset }` after `.x`: color reads the blue parent, weight 400, the box at
    /// x 0, and it is inline (on its sibling span's line); the `.x` twin reads red / 700, sits at 20 and starts a new line
    /// (armed).
    #[test]
    fn css_wide_all_unset_resets() {
        let mut e = css_wide_all_engine();
        assert_eq!(font_shorthand_cs(&mut e, "ax", "color"), "rgb(255, 0, 0)", "the .x twin is red (armed)");
        assert_eq!(font_shorthand_cs(&mut e, "ax", "fontWeight"), "700", "the .x twin is bold (armed)");
        assert_eq!(font_shorthand_cs(&mut e, "axr", "color"), "rgb(0, 0, 255)", "all: unset: color inherits the parent");
        assert_eq!(font_shorthand_cs(&mut e, "axr", "fontWeight"), "400", "all: unset: weight inherits normal");
        let (axl, ..) = imgalt_rect(&mut e, "ax");
        let (axrl, ..) = imgalt_rect(&mut e, "axr");
        assert!((axl - 20.0).abs() <= 0.5, "the .x box at 20 (armed): {axl}");
        assert!(axrl.abs() <= 0.5, "all: unset: margin-left 0: {axrl}");
        let (_, st, ..) = imgalt_rect(&mut e, "sx");
        let (_, bt, ..) = imgalt_rect(&mut e, "bx");
        assert!(bt > st + 5.0, "the block .x after a span is on a new line (armed): {bt} vs {st}");
        let (_, srt, ..) = imgalt_rect(&mut e, "sxr");
        let (_, brt, ..) = imgalt_rect(&mut e, "bxr");
        assert!((brt - srt).abs() <= 5.0, "all: unset: display inline, on the span's line: {brt} vs {srt}");
        assert!(css_wide_color_ink(&mut e, "axr", css_wide_blue) > 3, "the unset box paints the blue parent");
        assert_eq!(css_wide_color_ink(&mut e, "axr", css_wide_red), 0, "no red");
    }

    /// AETHERCSSWIDEALL (SR1021): `all: initial` under a red parent paints CanvasText black (initial, not inherit); the
    /// plain div beside it paints red (armed).
    #[test]
    fn css_wide_all_initial_black() {
        let mut e = css_wide_all_engine();
        assert!(css_wide_color_ink(&mut e, "ai0", css_wide_red) > 3, "the plain div is red (armed)");
        assert!(css_wide_color_ink(&mut e, "ai", css_wide_controls_black) > 3, "all: initial paints black");
        assert_eq!(css_wide_color_ink(&mut e, "ai", css_wide_red), 0, "all: initial paints no red");
        assert_eq!(font_shorthand_cs(&mut e, "ai", "color"), "rgb(0, 0, 0)", "cssom reads black");
    }

    /// AETHERCSSWIDEALL (SR1021): `all: unset; color: green`: the later colour wins.
    #[test]
    fn css_wide_all_later_color_wins() {
        let mut e = css_wide_all_engine();
        assert_eq!(font_shorthand_cs(&mut e, "ag", "color"), "rgb(0, 128, 0)", "the later green wins");
    }

    // ---- AETHERCSSTEXTDECORATIONCOLOR (LEDGER SR1018): text-decoration-color paints the decoration (named tail block) ----

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): one 20px serif row per probe, black text, rows 12px apart: `#tl`
    /// `text-decoration: underline; text-decoration-color: red`; `#ts` `text-decoration: underline red`; `#tc` `color:
    /// blue; text-decoration: underline red; text-decoration-color: currentcolor`; `#tp` a blue span under a parent with a
    /// red underline; `#ti` `text-decoration: underline red; text-decoration-color: initial`; `#tg` `.d { text-decoration:
    /// underline currentcolor } .d.g { color: rgb(0, 160, 0) }`; `#tn` a plain `text-decoration: underline` (control).
    fn text_decoration_color_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .d { text-decoration: underline currentcolor }
            .d.g { color: rgb(0, 160, 0) }
            </style></head><body style="margin:0;font:20px serif;color:black">
            <div id="tl" style="text-decoration:underline;text-decoration-color:red">Hgjx Hgjx</div>
            <div id="ts" style="text-decoration:underline red">Hgjx Hgjx</div>
            <div id="tc" style="color:blue;text-decoration:underline red;text-decoration-color:currentcolor">Hgjx Hgjx</div>
            <div style="text-decoration:underline;text-decoration-color:red"><span id="tp" style="color:blue">Hgjx Hgjx</span></div>
            <div id="ti" style="text-decoration:underline red;text-decoration-color:initial">Hgjx Hgjx</div>
            <div id="tg" class="d g">Hgjx Hgjx</div>
            <div id="tn" style="text-decoration:underline">Hgjx Hgjx</div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): the count of pixels passing `hue` in element `id`'s rect extended 4px down
    /// (the underline sits at the line box's foot).
    fn text_decoration_ink(e: &mut crate::AetherEngine, id: &str, hue: fn((u8, u8, u8)) -> bool) -> usize {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (l.floor().max(0.0) as usize, ((l + w).ceil() as usize).min(sw));
        let (y0, y1) = (t.floor().max(0.0) as usize, ((t + h).ceil() as usize + 4).min(sh));
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if hue(imgalt_px(e, x, y)) {
                    n += 1;
                }
            }
        }
        n
    }

    fn text_decoration_black(c: (u8, u8, u8)) -> bool {
        c.0 < 90 && c.1 < 90 && c.2 < 90
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): `text-decoration: underline; text-decoration-color: red` under black text
    /// paints a red underline (a run of red pixels) and black glyphs; the plain underline control paints no red.
    #[test]
    fn text_decoration_color_longhand_paints_red() {
        let mut e = text_decoration_color_engine();
        assert!(text_decoration_ink(&mut e, "tl", css_wide_red) > 40, "#tl paints a red underline");
        assert!(text_decoration_ink(&mut e, "tl", text_decoration_black) > 40, "#tl's glyphs stay black");
        assert_eq!(text_decoration_ink(&mut e, "tn", css_wide_red), 0, "the plain underline paints no red (armed)");
        assert!(text_decoration_ink(&mut e, "tn", text_decoration_black) > 40, "the plain underline paints");
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): the shorthand's colour word `text-decoration: underline red` the same.
    #[test]
    fn text_decoration_color_shorthand_paints_red() {
        let mut e = text_decoration_color_engine();
        assert!(text_decoration_ink(&mut e, "ts", css_wide_red) > 40, "#ts paints a red underline");
        assert!(text_decoration_ink(&mut e, "ts", text_decoration_black) > 40, "#ts's glyphs stay black");
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): a later `text-decoration-color: currentcolor` paints the line in the text's
    /// blue: no red.
    #[test]
    fn text_decoration_color_currentcolor() {
        let mut e = text_decoration_color_engine();
        assert!(text_decoration_ink(&mut e, "tc", css_wide_blue) > 40, "#tc paints blue");
        assert_eq!(text_decoration_ink(&mut e, "tc", css_wide_red), 0, "#tc paints no red");
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): a blue span under a parent whose underline is red paints the parent's red
    /// underline through its own text, its glyphs blue (the colour rides the inherited underline road).
    #[test]
    fn text_decoration_color_child_span() {
        let mut e = text_decoration_color_engine();
        assert!(text_decoration_ink(&mut e, "tp", css_wide_red) > 40, "#tp paints the parent's red underline");
        assert!(text_decoration_ink(&mut e, "tp", css_wide_blue) > 40, "#tp's glyphs are blue");
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): `text-decoration-color: initial` is currentcolor: the shorthand's red goes,
    /// the line paints black.
    #[test]
    fn text_decoration_color_initial() {
        let mut e = text_decoration_color_engine();
        assert_eq!(text_decoration_ink(&mut e, "ti", css_wide_red), 0, "#ti paints no red");
        assert!(text_decoration_ink(&mut e, "ti", text_decoration_black) > 40, "#ti paints black");
    }

    /// AETHERCSSTEXTDECORATIONCOLOR (SR1018): `.d { text-decoration: underline currentcolor }` with a later `.d.g { color:
    /// rgb(0, 160, 0) }` paints the line green with the text: no black, no red.
    #[test]
    fn text_decoration_color_later_color() {
        let mut e = text_decoration_color_engine();
        assert!(text_decoration_ink(&mut e, "tg", css_wide_green) > 40, "#tg paints green");
        assert_eq!(text_decoration_ink(&mut e, "tg", css_wide_red), 0, "#tg paints no red");
        assert_eq!(text_decoration_ink(&mut e, "tg", text_decoration_black), 0, "#tg paints no black");
    }

    // ---- AETHERCSSWIDENONINHERITEDINHERIT (LEDGER SR1007): inherit on a non-inherited property takes the parent's resolved value (named tail block) ----

    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): one parent per family at 400 px wide, each with a plain child (its own
    /// earlier value, the armed twin) and an `.i` child whose later rule says `inherit`: padding 20px over 5px, display
    /// flex over block (two 30px kids), width 120px over 50px, margin-left 15px, a 4px red border, a red background over
    /// the children's blue.
    fn inherit_box_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            .p { padding: 20px }
            .p > .c { padding: 5px }
            .p > .c.i { padding: inherit }
            .f { display: flex }
            .f > .c { display: block }
            .f > .c.i { display: inherit }
            .k { width: 30px; height: 10px }
            .w { width: 120px }
            .w > .c { width: 50px; height: 10px }
            .w > .c.i { width: inherit }
            .m { margin-left: 15px }
            .m > .c { height: 10px }
            .m > .c.i { margin: inherit }
            .b { border: 4px solid rgb(255, 0, 0); padding: 4px }
            .b > .c { height: 20px }
            .b > .c.i { border: inherit }
            .bg { background-color: rgb(255, 0, 0); padding: 4px }
            .bg > .c { background-color: rgb(0, 0, 255); height: 20px }
            .bg > .c.i { background-color: inherit }
            </style></head><body style="margin:0;font:20px serif">
            <div class="p"><div id="pc" class="c"><span id="pcs">Hi</span></div></div>
            <div class="p"><div id="pi" class="c i"><span id="pis">Hi</span></div></div>
            <div class="f"><div id="fc" class="c"><div id="fc1" class="k"></div><div id="fc2" class="k"></div></div></div>
            <div class="f"><div id="fi" class="c i"><div id="fi1" class="k"></div><div id="fi2" class="k"></div></div></div>
            <div class="w"><div id="wc" class="c"></div></div>
            <div class="w"><div id="wi" class="c i"></div></div>
            <div class="m"><div id="mc" class="c"></div></div>
            <div class="m"><div id="mi" class="c i"></div></div>
            <div class="b"><div id="bc" class="c"></div></div>
            <div class="b"><div id="bi" class="c i"></div></div>
            <div class="bg"><div id="gc" class="c"></div></div>
            <div class="bg"><div id="gi" class="c i"></div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `padding: inherit` puts the span 20px in and reads `paddingTop` 20px
    /// (the 5px twin, armed); `width: inherit` is the parent's 120 (the 50 twin); `margin: inherit` is the parent's 15.
    #[test]
    fn inherit_box_engine_padding_width_margin() {
        let mut e = inherit_box_engine();
        let ((cl, ..), (sl, ..)) = (imgalt_rect(&mut e, "pc"), imgalt_rect(&mut e, "pcs"));
        assert!((sl - cl - 5.0).abs() <= 0.5, "the plain child's own 5px (armed): {sl} vs {cl}");
        assert_eq!(font_shorthand_cs(&mut e, "pc", "paddingTop"), "5px", "armed");
        let ((cl, ..), (sl, ..)) = (imgalt_rect(&mut e, "pi"), imgalt_rect(&mut e, "pis"));
        assert!((sl - cl - 20.0).abs() <= 0.5, "padding: inherit is the parent's 20px: {sl} vs {cl}");
        assert_eq!(font_shorthand_cs(&mut e, "pi", "paddingTop"), "20px", "getComputedStyle reads the parent's 20px");
        let (_, _, wc, _) = imgalt_rect(&mut e, "wc");
        let (_, _, wi, _) = imgalt_rect(&mut e, "wi");
        assert!((wc - 50.0).abs() <= 0.5, "the plain child is 50 (armed): {wc}");
        assert!((wi - 120.0).abs() <= 0.5, "width: inherit is the parent's 120: {wi}");
        let (mcl, ..) = imgalt_rect(&mut e, "mc");
        let (mil, ..) = imgalt_rect(&mut e, "mi");
        assert!((mcl - 15.0).abs() <= 0.5, "the plain child sits at the parent's 15 (armed): {mcl}");
        assert!((mil - 30.0).abs() <= 0.5, "margin: inherit adds the parent's 15: {mil}");
    }

    // ---- AETHERCSSTEXTUNDERLINEOFFSET (LEDGER SR1045): text-underline-offset / -position place the underline (named tail block) ----

    /// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): 20px serif rows of "gyp gyp" (black text, red decorations, 20px apart): `#ua`
    /// the auto underline (the twin), `#uo` `text-underline-offset: 4px`, `#uu` `text-underline-position: under`; `#pu` a
    /// `text-underline-position: under` parent and `#po` a `text-underline-offset: 6px` parent, each holding an underlined
    /// span (`#pus` / `#pos`) that sets neither.
    fn text_underline_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 20px }
            div, span { text-decoration-color: red }
            </style></head><body style="margin:0;font:20px serif;color:black">
            <div id="ua" style="text-decoration-line:underline">gyp gyp</div>
            <div id="uo" style="text-decoration-line:underline;text-underline-offset:4px">gyp gyp</div>
            <div id="uu" style="text-decoration-line:underline;text-underline-position:under">gyp gyp</div>
            <div id="pu" style="text-underline-position:under"><span id="pus" style="text-decoration-line:underline">gyp gyp</span></div>
            <div id="po" style="text-underline-offset:6px"><span id="pos" style="text-decoration-line:underline">gyp gyp</span></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): the sorted distinct rows of element `id`'s rect, extended 8px down, holding a
    /// pixel `hue` accepts.
    fn text_underline_rows(e: &mut crate::AetherEngine, id: &str, hue: fn((u8, u8, u8)) -> bool) -> Vec<usize> {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (l.floor().max(0.0) as usize, ((l + w).ceil() as usize).min(sw));
        let (y0, y1) = (t.floor().max(0.0) as usize, ((t + h).ceil() as usize + 8).min(sh));
        let mut rows: Vec<usize> = (y0..y1).filter(|&y| (x0..x1).any(|x| hue(imgalt_px(e, x, y)))).collect();
        rows.dedup();
        rows
    }

    /// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): `text-underline-offset: 4px` paints the same band 4 rows lower than the auto
    /// twin (each measured from its own box top).
    #[test]
    fn text_underline_offset_four_rows_lower() {
        let mut e = text_underline_engine();
        let auto = text_underline_rows(&mut e, "ua", css_wide_red);
        let off = text_underline_rows(&mut e, "uo", css_wide_red);
        assert!(!auto.is_empty() && !off.is_empty(), "both paint red: {auto:?} {off:?}");
        let (ta, to) = (imgalt_rect(&mut e, "ua").1.floor() as i64, imgalt_rect(&mut e, "uo").1.floor() as i64);
        assert_eq!(off[0] as i64 - to, auto[0] as i64 - ta + 4, "4 rows lower: {off:?} vs {auto:?}");
        assert_eq!(off.len(), auto.len(), "the same band height");
    }

    /// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): `text-underline-position: under` paints below every row of the "gyp" glyph
    /// ink; the auto twin's band runs through the descenders (armed).
    #[test]
    fn text_underline_position_under_clears_descenders() {
        let mut e = text_underline_engine();
        let (ink, red) = (text_underline_rows(&mut e, "ua", text_decoration_black), text_underline_rows(&mut e, "ua", css_wide_red));
        assert!(!ink.is_empty() && !red.is_empty(), "armed: #ua paints text and an underline");
        assert!(red[0] <= *ink.last().unwrap(), "armed: the auto underline cuts the descenders: red {red:?} ink to {}", ink.last().unwrap());
        let (ink, red) = (text_underline_rows(&mut e, "uu", text_decoration_black), text_underline_rows(&mut e, "uu", css_wide_red));
        assert!(!ink.is_empty() && !red.is_empty(), "#uu paints text and an underline");
        assert!(red[0] > *ink.last().unwrap(), "under is below the descenders: red {red:?} ink to {}", ink.last().unwrap());
    }

    /// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): text-underline-position is inherited: the span under the `under` parent paints
    /// below its descenders; text-underline-offset is not: the span under the 6px parent paints at the auto rows, through
    /// its descenders.
    #[test]
    fn text_underline_position_inherits_offset_not() {
        let mut e = text_underline_engine();
        let (ink, red) = (text_underline_rows(&mut e, "pus", text_decoration_black), text_underline_rows(&mut e, "pus", css_wide_red));
        assert!(!ink.is_empty() && !red.is_empty(), "#pus paints text and an underline");
        assert!(red[0] > *ink.last().unwrap(), "the inherited under: red {red:?} ink to {}", ink.last().unwrap());
        let (ink, red) = (text_underline_rows(&mut e, "pos", text_decoration_black), text_underline_rows(&mut e, "pos", css_wide_red));
        assert!(!ink.is_empty() && !red.is_empty(), "#pos paints text and an underline");
        assert!(red[0] <= *ink.last().unwrap(), "the parent's offset does not reach the span: red {red:?} ink to {}", ink.last().unwrap());
    }

    // ---- AETHERCSSWIDECOLORCONTROLSDARK (LEDGER SR1031): the UA control colours and the canvas under color-scheme: dark (named tail block) ----

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): one row per control, 12px apart, under `:root { color-scheme: <scheme> }`:
    /// an unstyled `<button>` (#b0), a text field (#f0), a `<button>` with an author red background (#br), a `<button>`
    /// inside `div { color-scheme: normal }` (#bn), and a paragraph (#t, the root's CanvasText).
    fn color_scheme_engine(scheme: &str) -> crate::AetherEngine {
        let html = format!(r##"<!doctype html><html><head><style>
            :root {{ color-scheme: {scheme} }}
            div {{ margin: 0 0 12px }}
            </style></head><body style="margin:0;font:20px serif">
            <div><button id="b0">Hello</button></div>
            <div><input id="f0" type="text" style="width:120px"></div>
            <div><button id="br" style="background:rgb(255, 0, 0)">Hello</button></div>
            <div style="color-scheme:normal"><button id="bn">Hello</button></div>
            <p id="t">Text</p>
        </body></html>"##);
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    fn color_scheme_dark_face(c: (u8, u8, u8)) -> bool {
        c == (107, 107, 107)
    }

    fn color_scheme_light_face(c: (u8, u8, u8)) -> bool {
        c == (239, 239, 239)
    }

    fn color_scheme_dark_field(c: (u8, u8, u8)) -> bool {
        c == (59, 59, 59)
    }

    fn color_scheme_white_ink(c: (u8, u8, u8)) -> bool {
        c.0 > 200 && c.1 > 200 && c.2 > 200
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): under a dark root an unstyled button paints Chromium's dark ButtonFace
    /// (#6b6b6b) with a white label, and no light face.
    #[test]
    fn color_scheme_dark_button_face_and_label() {
        let mut e = color_scheme_engine("dark");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_dark_face) > 20, "the dark face fills the button");
        assert_eq!(css_wide_color_ink(&mut e, "b0", color_scheme_light_face), 0, "no light face under dark");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_white_ink) > 3, "the label paints white ButtonText");
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): the dark root's canvas is #121212 and its text white (CanvasText).
    #[test]
    fn color_scheme_dark_canvas_and_text() {
        let mut e = color_scheme_engine("dark");
        assert_eq!(imgalt_px(&e, 395, 595), (18, 18, 18), "the dark canvas");
        assert!(css_wide_color_ink(&mut e, "t", color_scheme_white_ink) > 3, "the paragraph paints white CanvasText");
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): a text field under dark fills the dark Field (#3b3b3b).
    #[test]
    fn color_scheme_dark_field_fill() {
        let mut e = color_scheme_engine("dark");
        assert!(css_wide_color_ink(&mut e, "f0", color_scheme_dark_field) > 20, "the dark field fill");
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): an author background on a dark button wins: red, no dark face.
    #[test]
    fn color_scheme_dark_author_background_wins() {
        let mut e = color_scheme_engine("dark");
        assert!(css_wide_color_ink(&mut e, "br", css_wide_red) > 20, "the author red wins");
        assert_eq!(css_wide_color_ink(&mut e, "br", color_scheme_dark_face), 0, "no dark face under the author red");
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): `color-scheme: normal` under the dark root resets (Some(0), not the
    /// inherit road): the light face and a black label.
    #[test]
    fn color_scheme_normal_resets_under_dark() {
        let mut e = color_scheme_engine("dark");
        assert!(css_wide_color_ink(&mut e, "bn", color_scheme_light_face) > 20, "the normal button paints the light face");
        assert!(css_wide_color_ink(&mut e, "bn", color_scheme_dark_face) <= 2, "no dark face under normal (a black label glyph's antialias can land on (107, 107, 107) over the light face)");
        assert!(css_wide_color_ink(&mut e, "bn", css_wide_controls_black) > 3, "the normal button's label is black");
    }

    /// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): ARMED twins: `light dark` is light (the host prefers light) and `normal`
    /// is today's light road: the light face, no dark face or field, a white canvas, black text.
    #[test]
    fn color_scheme_light_twins() {
        for scheme in ["light dark", "normal"] {
            let mut e = color_scheme_engine(scheme);
            assert!(css_wide_color_ink(&mut e, "b0", color_scheme_light_face) > 20, "{scheme}: the light face");
            assert!(css_wide_color_ink(&mut e, "b0", color_scheme_dark_face) <= 2, "{scheme}: no dark face (a label glyph's antialias pixel may hit (107, 107, 107))");
            assert_eq!(css_wide_color_ink(&mut e, "f0", color_scheme_dark_field), 0, "{scheme}: no dark field");
            assert_eq!(imgalt_px(&e, 395, 595), (255, 255, 255), "{scheme}: the white canvas");
            assert!(css_wide_color_ink(&mut e, "t", css_wide_controls_black) > 3, "{scheme}: black text");
        }
    }

    // ---- AETHERCOLORSCHEMEMETA (LEDGER SR1047): `<meta name="color-scheme">` sets the root's scheme at the UA origin (named tail block) ----

    /// AETHERCOLORSCHEMEMETA (SR1047): a page whose head holds `meta` (the tag's text, raw) and `style` (author CSS), a
    /// button `#b0` and a paragraph `#t`, on a 400x600 viewport.
    fn color_scheme_meta_engine(meta: &str, style: &str) -> crate::AetherEngine {
        let html = format!(r##"<!doctype html><html><head>{meta}<style>{style} div {{ margin: 0 0 12px }}</style></head>
            <body style="margin:0;font:20px serif"><div><button id="b0">Hello</button></div><p id="t">Text</p></body></html>"##);
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCOLORSCHEMEMETA (SR1047): `<meta name="color-scheme" content="dark">` with no CSS paints the dark canvas
    /// (#121212) and the dark button face, as `:root { color-scheme: dark }` does.
    #[test]
    fn color_scheme_meta_dark_paints_dark() {
        let mut e = color_scheme_meta_engine(r#"<meta name="color-scheme" content="dark">"#, "");
        assert_eq!(imgalt_px(&e, 395, 595), (18, 18, 18), "the meta's dark canvas");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_dark_face) > 20, "the dark face fills the button");
        assert_eq!(css_wide_color_ink(&mut e, "b0", color_scheme_light_face), 0, "no light face");
        let mut e = color_scheme_meta_engine(r#"<meta name="COLOR-SCHEME" content=" only dark ">"#, "");
        assert_eq!(imgalt_px(&e, 395, 595), (18, 18, 18), "the name is ASCII case-insensitive; only is accepted");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_dark_face) > 20, "and the button is dark");
    }

    /// AETHERCOLORSCHEMEMETA (SR1047): the meta is a UA-origin declaration: an author `:root { color-scheme: light }`
    /// beats it (the white canvas and the light face).
    #[test]
    fn color_scheme_meta_author_root_wins() {
        let mut e = color_scheme_meta_engine(r#"<meta name="color-scheme" content="dark">"#, ":root { color-scheme: light }");
        assert_eq!(imgalt_px(&e, 395, 595), (255, 255, 255), "the author's light canvas");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_light_face) > 20, "the light face");
        assert!(css_wide_color_ink(&mut e, "b0", color_scheme_dark_face) <= 2, "no dark face (a label antialias pixel may hit it)");
    }

    /// AETHERCOLORSCHEMEMETA (SR1047): ARMED twins of the meta: `light dark` follows the host preference (light by
    /// default: this test binary never records a dark preference), an invalid content (`dark;x`, `normal dark`) and a
    /// meta outside the head are ignored, and no meta is today's light road.
    #[test]
    fn color_scheme_meta_light_twins() {
        let cases: [(&str, &str); 4] = [
            ("light dark", r#"<meta name="color-scheme" content="light dark">"#),
            ("invalid", r#"<meta name="color-scheme" content="dark;x">"#),
            ("normal beside", r#"<meta name="color-scheme" content="normal dark">"#),
            ("none", ""),
        ];
        for (what, meta) in cases {
            let mut e = color_scheme_meta_engine(meta, "");
            assert_eq!(imgalt_px(&e, 395, 595), (255, 255, 255), "{what}: the white canvas");
            assert!(css_wide_color_ink(&mut e, "b0", color_scheme_light_face) > 20, "{what}: the light face");
        }
        let html = r##"<!doctype html><html><head></head><body style="margin:0"><meta name="color-scheme" content="dark"><p>x</p></body></html>"##;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 600));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(imgalt_px(&e, 395, 595), (255, 255, 255), "a body meta is not the head's: the white canvas");
    }

    // ---- AETHERCSSOMWHITESPACELONGHANDSWRITE (LEDGER SR1044): white-space-collapse / text-wrap-mode / text-wrap write the one white-space code (named tail block) ----

    /// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): one div per longhand case at 400 px wide: pre + wrap in one block and
    /// across two rules, a nowrap child of a pre parent, preserve alone, the shorthand after a longhand, a `<pre>` with
    /// wrap; a 60px four-word pair (plain / `text-wrap-mode: nowrap`) and a two-line text pair (plain / `white-space-collapse:
    /// preserve`), each beside a one-line reference.
    fn white_space_longhands_write_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            .a { white-space: pre }
            .a.w { text-wrap-mode: wrap }
            .n { width: 60px }
            .n.x { text-wrap-mode: nowrap }
            .c.p { white-space-collapse: preserve }
            </style></head><body style="margin:0;font:16px serif">
            <div id="pw" style="white-space:pre; text-wrap-mode:wrap">a</div>
            <div id="ar" class="a w">a</div>
            <div id="ap" class="a">a</div>
            <div style="white-space:pre"><div id="np" style="text-wrap-mode:nowrap">a</div></div>
            <div style="white-space:pre-wrap"><div id="nr" style="text-wrap:nowrap balance">a</div></div>
            <div id="pv" style="white-space-collapse:preserve">a</div>
            <div id="sw" style="text-wrap-mode:nowrap; white-space:normal">a</div>
            <pre id="pt" style="text-wrap-mode:wrap">a</pre>
            <div id="ref">a</div>
            <div id="n0" class="n">aaa bbb ccc ddd</div>
            <div id="n1" class="n x">aaa bbb ccc ddd</div>
            <div id="c0" class="c">a
b</div>
            <div id="c1" class="c p">a
b</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): getComputedStyle reads the combined code: `pre` + `text-wrap-mode:
    /// wrap` is pre-wrap (one block and across rules; the plain `.a` twin is pre, armed), nowrap under pre is pre,
    /// `text-wrap: nowrap balance` under pre-wrap is pre, preserve alone is pre-wrap, the later shorthand wins, a `<pre>`
    /// with wrap is pre-wrap; SR1036's columns read the same code.
    #[test]
    fn white_space_longhands_write_cssom() {
        let mut e = white_space_longhands_write_engine();
        for (id, ws, collapse, wrap) in [
            ("pw", "pre-wrap", "preserve", "wrap"),
            ("ar", "pre-wrap", "preserve", "wrap"),
            ("ap", "pre", "preserve", "nowrap"),
            ("np", "pre", "preserve", "nowrap"),
            ("nr", "pre", "preserve", "nowrap"),
            ("pv", "pre-wrap", "preserve", "wrap"),
            ("sw", "normal", "collapse", "wrap"),
            ("pt", "pre-wrap", "preserve", "wrap"),
            ("ref", "normal", "collapse", "wrap"),
        ] {
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('white-space')"), ws, "#{id}: white-space");
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('white-space-collapse')"), collapse, "#{id}: white-space-collapse");
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('text-wrap-mode')"), wrap, "#{id}: text-wrap-mode");
        }
    }

    /// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): layout reads the longhands: the 60px four-word div wraps (armed) and
    /// its `text-wrap-mode: nowrap` twin is one line; the two-line text collapses to one line (armed) and its
    /// `white-space-collapse: preserve` twin keeps the break.
    #[test]
    fn white_space_longhands_write_layout() {
        let mut e = white_space_longhands_write_engine();
        let (.., one) = imgalt_rect(&mut e, "ref");
        let (.., h0) = imgalt_rect(&mut e, "n0");
        let (.., h1) = imgalt_rect(&mut e, "n1");
        assert!(h0 > one + 5.0, "the plain 60px div wraps (armed): {h0} vs {one}");
        assert!((h1 - one).abs() <= 1.0, "text-wrap-mode: nowrap is one line: {h1} vs {one}");
        let (.., c0) = imgalt_rect(&mut e, "c0");
        let (.., c1) = imgalt_rect(&mut e, "c1");
        assert!((c0 - one).abs() <= 1.0, "the newline collapses (armed): {c0} vs {one}");
        assert!(c1 > one + 5.0, "white-space-collapse: preserve keeps the newline: {c1} vs {one}");
    }

    // ---- AETHERCSSOMUNICODEBIDI (LEDGER SR1008): unicode-bidi end to end: the override paints, the 22nd column reads (named tail block) ----

    /// AETHERCSSOMUNICODEBIDI (SR1008): 20px serif rows 12px apart: a `bdo dir=rtl` (the UA's isolate-override), an author
    /// `unicode-bidi: bidi-override; direction: rtl` span, the visual reference `cba`, the armed control `span dir=rtl`
    /// (isolate, no override: paints abc), a bdo whose author `unicode-bidi: normal` beats the tag, and the cssom probes.
    fn unicode_bidi_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:20px serif">
            <div style="margin:0 0 12px"><bdo id="bdo" dir="rtl">abc</bdo></div>
            <div style="margin:0 0 12px"><span id="ovr" style="unicode-bidi: bidi-override; direction: rtl">abc</span></div>
            <div style="margin:0 0 12px"><span id="cba">cba</span></div>
            <div style="margin:0 0 12px"><span id="ctl" dir="rtl">abc</span></div>
            <div style="margin:0 0 12px"><bdo id="bdon" dir="rtl" style="unicode-bidi: normal">abc</bdo></div>
            <div style="margin:0 0 12px"><span id="abc">abc</span></div>
            <div id="plain" style="margin:0 0 12px"><bdi id="bdi">abc</bdi> <output id="out">1</output> <span id="sp">x</span>
                <span id="spd" dir="ltr">x</span> <span id="up" style="unicode-bidi: BIDI-OVERRIDE">x</span>
                <bdo id="bdo2" dir="ltr"><span id="kid">x</span></bdo> <span id="emb" style="unicode-bidi: embed">x</span></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): element `id`'s rect as a dark-pixel mask (rows x columns from its top-left).
    fn unicode_bidi_mask(e: &mut crate::AetherEngine, id: &str) -> Vec<Vec<bool>> {
        let (l, t, w, h) = imgalt_rect(e, id);
        let e: &crate::AetherEngine = e;
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, y0) = (l.floor().max(0.0) as usize, t.floor().max(0.0) as usize);
        let (x1, y1) = (((l + w).ceil() as usize).min(sw), ((t + h).ceil() as usize).min(sh));
        (y0..y1).map(|y| (x0..x1).map(|x| { let (r, g, b) = imgalt_px(e, x, y); (r as u32 + g as u32 + b as u32) < 384 }).collect()).collect()
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): the count of pixels two masks disagree on over their common area (plus the area
    /// only one covers).
    fn unicode_bidi_diff(a: &[Vec<bool>], b: &[Vec<bool>]) -> usize {
        let mut n = 0;
        for y in 0..a.len().max(b.len()) {
            let (ra, rb) = (a.get(y).map(|r| r.as_slice()).unwrap_or(&[]), b.get(y).map(|r| r.as_slice()).unwrap_or(&[]));
            for x in 0..ra.len().max(rb.len()) {
                if ra.get(x).copied().unwrap_or(false) != rb.get(x).copied().unwrap_or(false) {
                    n += 1;
                }
            }
        }
        n
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): `<bdo dir=rtl>abc</bdo>` paints c-b-a left to right (the UA's isolate-override: the
    /// run between RLO and PDF, no reordering), the ink of a plain `cba`; the armed control `<span dir=rtl>abc</span>`
    /// (rtl paragraph, L letters at level 2) still paints a-b-c, far from `cba`.
    #[test]
    fn unicode_bidi_bdo_paints_override() {
        let mut e = unicode_bidi_engine();
        let (bdo, cba, ctl, abc) = (unicode_bidi_mask(&mut e, "bdo"), unicode_bidi_mask(&mut e, "cba"), unicode_bidi_mask(&mut e, "ctl"), unicode_bidi_mask(&mut e, "abc"));
        let ink: usize = cba.iter().flatten().filter(|&&p| p).count();
        assert!(ink > 20, "the cba reference painted ({ink} dark px)");
        let (d_bdo, d_ctl) = (unicode_bidi_diff(&bdo, &cba), unicode_bidi_diff(&ctl, &cba));
        assert!(d_ctl > 0 && unicode_bidi_diff(&ctl, &abc) * 4 < d_ctl, "armed: dir=rtl alone paints abc (vs abc {}, vs cba {d_ctl})", unicode_bidi_diff(&ctl, &abc));
        assert!(d_bdo * 4 < d_ctl, "bdo dir=rtl paints cba: {d_bdo} px off cba, the control {d_ctl}");
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): an author `unicode-bidi: bidi-override; direction: rtl` span paints like the bdo; a
    /// bdo whose author `unicode-bidi: normal` beats the tag default loses the override (paints abc).
    #[test]
    fn unicode_bidi_css_override_and_author_normal() {
        let mut e = unicode_bidi_engine();
        let (ovr, cba, bdon, abc) = (unicode_bidi_mask(&mut e, "ovr"), unicode_bidi_mask(&mut e, "cba"), unicode_bidi_mask(&mut e, "bdon"), unicode_bidi_mask(&mut e, "abc"));
        let d_far = unicode_bidi_diff(&abc, &cba);
        assert!(d_far > 0, "abc and cba differ");
        assert!(unicode_bidi_diff(&ovr, &cba) * 4 < d_far, "bidi-override + rtl paints cba");
        assert!(unicode_bidi_diff(&bdon, &abc) * 4 < d_far, "unicode-bidi: normal on a bdo paints abc");
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): `unicode-bidi` is NOT inherited and isolate is by construction (every run is its own
    /// paragraph; AETHER-BIDI-ISOLATE owed): `<bdi>` paints its text as a plain span would, pinned as today.
    #[test]
    fn unicode_bidi_bdi_pinned_as_today() {
        let mut e = unicode_bidi_engine();
        assert!(unicode_bidi_mask(&mut e, "bdi").iter().flatten().any(|&p| p), "the bdi's text paints");
    }

    fn cssom_unicode_bidi_cs(e: &mut crate::AetherEngine, id: &str) -> String {
        e.js_engine.as_mut().unwrap().eval_string(&format!("getComputedStyle(document.getElementById('{id}')).unicodeBidi")).unwrap()
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): getComputedStyle(el).unicodeBidi reads the 22nd report column: the UA's
    /// isolate-override on bdo, isolate on bdi / output / any dir= element, the author keyword (case-folded), normal on a
    /// plain span and div, and normal on a bdo's child (not inherited).
    #[test]
    fn cssom_unicode_bidi_reads() {
        let mut e = unicode_bidi_engine();
        for (id, want) in [
            ("bdo", "isolate-override"), ("bdo2", "isolate-override"), ("bdi", "isolate"), ("out", "isolate"), ("ctl", "isolate"),
            ("spd", "isolate"), ("ovr", "bidi-override"), ("up", "bidi-override"), ("bdon", "normal"), ("emb", "embed"),
            ("sp", "normal"), ("cba", "normal"), ("plain", "normal"), ("kid", "normal"),
        ] {
            assert_eq!(cssom_unicode_bidi_cs(&mut e, id), want, "#{id}: computed unicode-bidi");
        }
    }

    /// AETHERCSSOMUNICODEBIDI (SR1008): unicode-bidi is the 22nd column, appended after SR1000's direction.
    #[test]
    fn cssom_unicode_bidi_report_width() {
        assert_eq!(crate::css::REPORT_PROPS.len(), crate::css::REPORT_COLS);
        assert_eq!(crate::css::REPORT_COLS, 25); // SR1036 fold: three columns after unicode-bidi
        assert_eq!(crate::css::REPORT_PROPS[21], "unicode-bidi");
        assert_eq!(crate::css::REPORT_PROPS[20], "direction");
    }

    // ---- AETHERFONTRELINVALIDSUPERSEDES (LEDGER SR1055): font_rel records only the length declarations an arm accepted (named tail block) ----

    /// AETHERFONTRELINVALIDSUPERSEDES (SR1055): a child with `width: 2em; width: bogus` follows its font through the
    /// font-relative replay: 20px under a 10px parent, 40px under a 20px one, as the plain `width: 2em` control does.
    /// Before: the replay list held `bogus` (not font-relative), so the element left the replay and kept the first pass's
    /// refused None width: auto, the 400px viewport.
    #[test]
    fn font_rel_invalid_replay_follows_font() {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font-size:10px"><div id="b10" style="width:2em;width:bogus">x</div><div id="c10" style="width:2em">x</div></div>
            <div style="font-size:20px"><div id="b20" style="width:2em;width:bogus">x</div><div id="c20" style="width:2em">x</div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 400));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let w = |e: &mut crate::AetherEngine, id: &str| js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().width"));
        assert_eq!(w(&mut e, "c10"), 20.0, "control: 2em of 10px");
        assert_eq!(w(&mut e, "c20"), 40.0, "control: 2em of 20px");
        assert_eq!(w(&mut e, "b10"), 20.0, "2em; bogus: 2em of 10px through the replay (before: 400, auto)");
        assert_eq!(w(&mut e, "b20"), 40.0, "2em; bogus: 2em of 20px through the replay (before: 400, auto)");
    }

    // ---- AETHERCSSOMFONTFAMILYUSED (LEDGER SR1024): getComputedStyle fontFamily is the specified list, pinned (named tail block) ----

    fn cssom_font_family_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0">
            <div id="root">Hello</div>
            <div id="list" style='font-family: Arial, "Times New Roman", serif'>Hello</div>
            <pre id="pre">Hello</pre>
            <button id="btn">Hello</button>
            <div style="font-family: Georgia, serif"><pre id="inh" style="font-family: inherit">Hello</pre></div>
            <div style="font-family: Georgia, serif"><span id="bad" style="font-family: 12px">Hello</span></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMFONTFAMILYUSED (SR1024): an author list reads back as written (Chromium: generics bare, a one-ident
    /// name bare, other names quoted); no family at all reads the standard font.
    #[test]
    fn cssom_font_family_specified_list() {
        let mut e = cssom_font_family_engine();
        assert_eq!(font_shorthand_cs(&mut e, "list", "fontFamily"), r#"Arial, "Times New Roman", serif"#);
        assert_eq!(font_shorthand_cs(&mut e, "root", "fontFamily"), r#""Times New Roman""#);
    }

    /// AETHERCSSOMFONTFAMILYUSED (SR1024): the UA tag defaults: `pre` reads `monospace`; a button reads Aether's control
    /// family word (`Arial`; Chromium's per-platform word is owed AETHER-CSSOM-FONT-FAMILY-UA-CONTROL).
    #[test]
    fn cssom_font_family_tag_defaults() {
        let mut e = cssom_font_family_engine();
        assert_eq!(font_shorthand_cs(&mut e, "pre", "fontFamily"), "monospace");
        assert_eq!(font_shorthand_cs(&mut e, "btn", "fontFamily"), "Arial");
    }

    /// AETHERCSSOMFONTFAMILYUSED (SR1024): `font-family: inherit` on a `pre` takes the parent's list, not the tag's monospace.
    #[test]
    fn cssom_font_family_inherit() {
        let mut e = cssom_font_family_engine();
        assert_eq!(font_shorthand_cs(&mut e, "inh", "fontFamily"), "Georgia, serif");
    }

    /// AETHERCSSOMFONTFAMILYUSED (SR1024): an unquoted word that is not an identifier (`12px`) makes the declaration
    /// invalid: it is dropped and the child reads the inherited list (it used to read `"12px"`).
    #[test]
    fn cssom_font_family_bogus_dropped() {
        let mut e = cssom_font_family_engine();
        assert_eq!(font_shorthand_cs(&mut e, "bad", "fontFamily"), "Georgia, serif");
        for bad in ["12px", "Font 2", "#x", "a/b", "-1x", "Arial, 12px"] {
            assert!(crate::fonts::parse_family_list(bad).is_none(), "{bad} is not a family list");
        }
        for good in ["-x", "_a", "Times   New Roman", "Font2, sans-serif", "--x"] {
            assert!(crate::fonts::parse_family_list(good).is_some(), "{good} is a family list");
        }
    }

    // ---- AETHERCSSWIDEREVERT (LEDGER SR1028): css-cascade-4 §7.3.2 revert rolls an inherit-bit field back to the tag road (named tail block) ----

    /// AETHERCSSWIDEREVERT (SR1028): each `.r` twin reverts what its armed twin's earlier rule set: a red link, a red div
    /// under a green parent, a `white-space: normal` pre, a 10px h1, a serif button, an `all: revert` div under a blue
    /// parent, and a margin-left (owed: stays). Body 16px serif, 400 px wide.
    fn css_wide_revert_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            section { display: block; margin: 0 0 12px }
            a.k { color: rgb(255, 0, 0) }
            a.k.r { color: revert }
            .c { color: rgb(255, 0, 0) }
            .c.r { color: revert }
            pre.n { white-space: normal }
            pre.n.r { white-space: revert }
            h1.f { font-size: 10px }
            h1.f.r { font-size: revert }
            button.s { font-family: serif }
            button.s.r { font-family: revert }
            .x { color: rgb(255, 0, 0); font-weight: bold }
            .x.r { all: revert }
            .m { margin-left: 20px }
            .m.r { margin-left: revert }
            </style></head><body style="margin:0;font:16px serif">
            <section><a id="ak" class="k" href="x">Hello</a></section>
            <section><a id="akr" class="k r" href="x">Hello</a></section>
            <div style="color:rgb(0, 128, 0)">
              <section><div id="dc" class="c">Hello</div></section>
              <section><div id="dcr" class="c r">Hello</div></section>
            </div>
            <pre id="pn" class="n">a  b</pre>
            <pre id="pnr" class="n r">a  b</pre>
            <h1 id="hf" class="f">H</h1>
            <h1 id="hfr" class="f r">H</h1>
            <button id="b0">B</button><button id="bs" class="s">B</button><button id="bsr" class="s r">B</button>
            <div style="color:rgb(0, 0, 255)"><section><div id="xr" class="x r">Hello</div></section></div>
            <section><div id="mr" class="m r">Hello</div></section>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 900));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDEREVERT (SR1028): `a.r { color: revert }` after `a { color: red }` is the UA link blue (cssom and ink);
    /// the armed twin is red.
    #[test]
    fn css_wide_revert_link_blue() {
        let mut e = css_wide_revert_engine();
        assert_eq!(font_shorthand_cs(&mut e, "ak", "color"), "rgb(255, 0, 0)", "the armed link is red");
        assert_eq!(font_shorthand_cs(&mut e, "akr", "color"), "rgb(0, 0, 238)", "color: revert is the UA link blue");
        assert!(css_wide_color_ink(&mut e, "akr", css_wide_blue) > 3, "the reverted link paints blue");
        assert_eq!(css_wide_color_ink(&mut e, "akr", css_wide_red), 0, "no red");
    }

    /// AETHERCSSWIDEREVERT (SR1028): a div has no UA colour, so `color: revert` inherits the green parent (armed red).
    #[test]
    fn css_wide_revert_div_inherits() {
        let mut e = css_wide_revert_engine();
        assert_eq!(font_shorthand_cs(&mut e, "dc", "color"), "rgb(255, 0, 0)", "armed red");
        assert_eq!(font_shorthand_cs(&mut e, "dcr", "color"), "rgb(0, 128, 0)", "revert: the parent's green");
        assert!(css_wide_color_ink(&mut e, "dcr", css_wide_green) > 3, "paints green");
        assert_eq!(css_wide_color_ink(&mut e, "dcr", css_wide_red), 0, "no red");
    }

    /// AETHERCSSWIDEREVERT (SR1028): white-space / font-size / font-family revert to the tag roads: pre `pre`, h1 2em
    /// (32px), the button's control family; each armed twin keeps the author value.
    #[test]
    fn css_wide_revert_tag_roads() {
        let mut e = css_wide_revert_engine();
        assert_eq!(font_shorthand_cs(&mut e, "pn", "whiteSpace"), "normal", "armed: the author normal");
        assert_eq!(font_shorthand_cs(&mut e, "pnr", "whiteSpace"), "pre", "white-space: revert is pre's UA pre");
        assert_eq!(font_shorthand_cs(&mut e, "hf", "fontSize"), "10px", "armed: 10px");
        assert_eq!(font_shorthand_cs(&mut e, "hfr", "fontSize"), "32px", "font-size: revert is h1's 2em");
        let plain = font_shorthand_cs(&mut e, "b0", "fontFamily");
        assert_ne!(font_shorthand_cs(&mut e, "bs", "fontFamily"), plain, "armed: the serif button differs");
        assert_eq!(font_shorthand_cs(&mut e, "bsr", "fontFamily"), plain, "font-family: revert is the control family");
    }

    /// AETHERCSSWIDEREVERT (SR1028): `all: revert` on a div under a blue parent: colour blue (no UA colour), weight 400;
    /// margin-left: revert stays 20px (AETHER-CSS-WIDE-REVERT-NONINHERITED owed).
    #[test]
    fn css_wide_revert_all_and_owed_margin() {
        let mut e = css_wide_revert_engine();
        assert_eq!(font_shorthand_cs(&mut e, "xr", "color"), "rgb(0, 0, 255)", "all: revert: the parent's blue");
        assert_eq!(font_shorthand_cs(&mut e, "xr", "fontWeight"), "400", "all: revert: weight 400");
        let (ml, ..) = imgalt_rect(&mut e, "mr");
        assert!(ml.abs() <= 0.5, "margin-left: revert is the div's UA 0: {ml}"); /* SR1037 */
    }

    // ---- AETHERCSSWIDEREVERTNONINHERITED (LEDGER SR1037): revert on display / margin / padding / border / background erases to the tag road (named tail block) ----

    /// AETHERCSSWIDEREVERTNONINHERITED (SR1037): each `.r` twin reverts its armed twin's earlier rule: a `margin: 0` p, an
    /// inline div, a block span, a red button, and a revert followed by `margin: 20px`. Each sits in a 1px-padded section
    /// (no margin collapses through), body 16px serif, 400 px wide.
    fn css_wide_revert_noninherited_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            section { display: block; margin: 0; padding: 1px 0 }
            p.z { margin: 0 }
            p.z.r { margin: revert }
            div.n { display: inline }
            div.n.r { display: revert }
            span.b { display: block }
            span.b.r { display: revert }
            button.g { background: rgb(255, 0, 0) }
            button.g.r { background: revert }
            div.w { margin: revert }
            div.w.r { margin-left: 20px }
            </style></head><body style="margin:0;font:16px serif">
            <section id="sz"><p id="pz" class="z">A</p></section>
            <section id="szr"><p id="pzr" class="z r">A</p></section>
            <section><div id="dn" class="n">Hi</div></section>
            <section><div id="dnr" class="n r">Hi</div></section>
            <section><span id="sb" class="b">Hi</span></section>
            <section><span id="sbr" class="b r">Hi</span></section>
            <section><button id="bg" class="g">Go</button> <button id="bgr" class="g r">Go</button></section>
            <section><div id="dw" class="w r">Hi</div></section>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 900));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `p { margin: 0 } p.r { margin: revert }` is the UA 1em top margin (the p
    /// sits 16px lower in its section than the armed twin).
    #[test]
    fn css_wide_revert_noninherited_p_margin() {
        let mut e = css_wide_revert_noninherited_engine();
        let off = |e: &mut crate::AetherEngine, p: &str, s: &str| imgalt_rect(e, p).1 - imgalt_rect(e, s).1;
        let zero = off(&mut e, "pz", "sz");
        let rev = off(&mut e, "pzr", "szr");
        assert!((rev - zero - 16.0).abs() <= 1.0, "margin: revert is p's 1em: armed {zero}, reverted {rev}");
    }

    /// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `display: revert` is the tag road: the div a 400px block, the span as
    /// narrow as its text; each armed twin differs.
    #[test]
    fn css_wide_revert_noninherited_display() {
        let mut e = css_wide_revert_noninherited_engine();
        let (_, _, dnr, _) = imgalt_rect(&mut e, "dnr");
        assert!((dnr - 400.0).abs() <= 0.5, "div display: revert is a block: {dnr}");
        let (_, _, sbr, _) = imgalt_rect(&mut e, "sbr");
        let (_, _, sb, _) = imgalt_rect(&mut e, "sb");
        assert!(sbr < 100.0 && sb > sbr, "span display: revert is inline ({sbr}), the armed block wider ({sb})");
    }

    /// AETHERCSSWIDEREVERTNONINHERITED (SR1037): `button { background: red } button.r { background: revert }` paints
    /// ButtonFace (239, 239, 239), no red; the armed twin paints red.
    #[test]
    fn css_wide_revert_noninherited_button_face() {
        let mut e = css_wide_revert_noninherited_engine();
        assert!(css_wide_color_ink(&mut e, "bg", css_wide_red) > 20, "armed: the button paints red");
        assert_eq!(css_wide_color_ink(&mut e, "bgr", css_wide_red), 0, "background: revert paints no red");
        assert!(css_wide_color_ink(&mut e, "bgr", css_wide_revert_noninherited_face) > 20, "background: revert paints ButtonFace");
    }

    fn css_wide_revert_noninherited_face(c: (u8, u8, u8)) -> bool {
        (c.0 as i32 - 239).abs() <= 2 && (c.1 as i32 - 239).abs() <= 2 && (c.2 as i32 - 239).abs() <= 2
    }

    /// AETHERCSSWIDEREVERTNONINHERITED (SR1037): a later `margin-left: 20px` beats an earlier rule's `margin: revert`.
    #[test]
    fn css_wide_revert_noninherited_later_value_wins() {
        let mut e = css_wide_revert_noninherited_engine();
        let (ml, ..) = imgalt_rect(&mut e, "dw");
        assert!((ml - 20.0).abs() <= 0.5, "the later margin-left: 20px wins: {ml}");
    }

    // ---- AETHERFONTSHORTHANDSYSTEM (LEDGER SR1022): font: caption / icon / menu / message-box / small-caption / status-bar (named tail block) ----

    /// AETHERFONTSHORTHANDSYSTEM (SR1022): under a 30px serif parent: a `font: menu` div with a child span, a
    /// `font: italic bold 20px serif; font: status-bar` span, a plain button (the control-font reference) and a plain span
    /// (the armed parent reference).
    fn system_font_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font:30px serif"><span id="par">Hello</span></div>
            <div style="font:30px serif"><div id="sys" style="font:menu">Hello <span id="kid">World</span></div></div>
            <div style="font:30px serif"><span id="sb" style="font:italic bold 20px serif;font:status-bar">Hello</span></div>
            <div style="font:30px serif"><button id="pbtn">Go</button></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERFONTSHORTHANDSYSTEM (SR1022): `font: menu` reads the plain button's used size (CONTROL_FONT_PX, 13.333px)
    /// and its child span inherits it, not the 30px parent (armed: the parent reference reads 30px).
    #[test]
    fn system_font_used_size() {
        let mut e = system_font_engine();
        let control = font_shorthand_cs(&mut e, "pbtn", "fontSize");
        assert!(control.starts_with("13.3333"), "the control size: {control}");
        assert_eq!(font_shorthand_cs(&mut e, "par", "fontSize"), "30px", "armed: the parent is 30px");
        assert_eq!(font_shorthand_cs(&mut e, "sys", "fontSize"), control, "font: menu is the control size");
        assert_eq!(font_shorthand_cs(&mut e, "kid", "fontSize"), control, "the child inherits the control size, not 30px");
        assert_eq!(font_shorthand_cs(&mut e, "sb", "fontSize"), control, "font: status-bar is the same font");
    }

    /// AETHERFONTSHORTHANDSYSTEM (SR1022): the system font resets weight and style (the initial road) and takes the
    /// control family: the family word Aether reports for a plain button.
    #[test]
    fn system_font_weight_style_family() {
        let mut e = system_font_engine();
        assert_eq!(font_shorthand_cs(&mut e, "sb", "fontWeight"), "400", "bold reset");
        assert_eq!(font_shorthand_cs(&mut e, "sb", "fontStyle"), "normal", "italic reset");
        let family = font_shorthand_cs(&mut e, "pbtn", "fontFamily");
        assert_eq!(font_shorthand_cs(&mut e, "sys", "fontFamily"), family, "the control family");
        assert_eq!(font_shorthand_cs(&mut e, "sb", "fontFamily"), family, "the control family");
        assert_ne!(font_shorthand_cs(&mut e, "par", "fontFamily"), family, "armed: the parent is serif");
    }

    // ---- AETHERCONTROLFONTPXTHIRD (LEDGER SR1023): Chromium's 40/3 px control font, six-significant-digit px (named tail block) ----

    /// AETHERCONTROLFONTPXTHIRD (SR1023): a plain button and six divs whose font-size exercises the report's serializer.
    fn control_font_px_third_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <button id="btn">Go</button>
            <div id="a" style="font-size:13.333333px">Hello</div>
            <div id="b" style="font-size:16px">Hello</div>
            <div id="c" style="font-size:0.5px">Hello</div>
            <div id="d" style="font-size:12.5px">Hello</div>
            <div id="e" style="font-size:1.23456789px">Hello</div>
            <div id="f" style="font-size:123.456789px">Hello</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCONTROLFONTPXTHIRD (SR1023): the control font is Chromium's `-webkit-small-control` 40/3 px, and a plain
    /// button's getComputedStyle fontSize reads Chromium's `13.3333px`.
    #[test]
    fn control_font_px_third_const() {
        assert_eq!(crate::layout::CONTROL_FONT_PX, 40.0f32 / 3.0, "the one source is 40/3");
        let mut e = control_font_px_third_engine();
        assert_eq!(font_shorthand_cs(&mut e, "btn", "fontSize"), "13.3333px", "the control default");
    }

    /// AETHERCONTROLFONTPXTHIRD (SR1023): the report's px serializer keeps Chromium's six significant digits; a whole
    /// number stays an integer.
    #[test]
    fn control_font_px_third_cssom_six_digits() {
        let mut e = control_font_px_third_engine();
        for (id, want) in [("a", "13.3333px"), ("b", "16px"), ("c", "0.5px"), ("d", "12.5px"), ("e", "1.23457px"), ("f", "123.457px")] {
            assert_eq!(font_shorthand_cs(&mut e, id, "fontSize"), want, "#{id}");
        }
    }

    /// AETHERCONTROLFONTPXTHIRD (SR1023): the 1/3000 px the control font gained does not move the control's line box
    /// (measured: the line height at 40/3 and at the old size round to the same px).
    #[test]
    fn control_font_px_third_line_box() {
        let Some(x) = crate::fonts::face(&crate::fonts::FontSel::new(crate::layout::control_family(), 400, false)) else { return };
        let new = crate::fonts::line_height(x, crate::layout::CONTROL_FONT_PX, 0.0);
        let old = crate::fonts::line_height(x, crate::layout::CONTROL_FONT_PX - 1.0 / 3000.0, 0.0);
        assert!((new - old).abs() < 0.01, "line height {new} vs {old}");
        assert_eq!(new.round(), old.round(), "the rounded line box holds: {new} vs {old}");
    }

    /// AETHERCONTROLFONTPXTHIRD (SR1023): CONTROL_FONT_PX is the one source: no code line (before its first `//`) in
    /// aether's src or layout_core's src / tests holds the old three-decimal literal as a number token.
    #[test]
    fn control_font_px_third_scan() {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            for ent in std::fs::read_dir(dir).unwrap().flatten() {
                let p = ent.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    out.push(p);
                }
            }
        }
        let needle = ["13", "333"].join(".");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for d in ["src", "../../unaos/libs/web/layout_core/src", "../../unaos/libs/web/layout_core/tests"] {
            walk(&root.join(d), &mut files);
        }
        assert!(files.len() > 10, "the scan saw the trees: {}", files.len());
        let mut hits = Vec::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                let b = code.as_bytes();
                let mut at = 0;
                while let Some(i) = code[at..].find(&needle) {
                    let s = at + i;
                    let e = s + needle.len();
                    let before = s > 0 && b[s - 1].is_ascii_digit();
                    let after = e < b.len() && b[e].is_ascii_digit();
                    if !before && !after {
                        hits.push(format!("{}:{}", f.display(), n + 1));
                    }
                    at = e;
                }
            }
        }
        assert!(hits.is_empty(), "literal control font size outside CONTROL_FONT_PX: {hits:?}");
    }

    // ---- AETHERCSSOMFONTSIZEBOXLESS (LEDGER SR1029): a box-less node reads its parent's computed font-size (named tail block) ----

    /// AETHERCSSOMFONTSIZEBOXLESS (SR1029): author `display: none` subtrees (they keep a taffy node, so the prepass writes
    /// their used size) beside laid-out twins, and the build-skipped nodes (`hidden`, `aria-hidden`, a `hidden` subtree)
    /// under a `2em` parent of 10px, whose report chain used to carry the parent's 16px-base parse (32px).
    fn font_size_boxless_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div style="font-size:20px"><div id="h" style="display:none;font-size:1.5em"><span id="hk" style="font-size:50%">x</span></div></div>
            <div style="font-size:20px"><div id="t" style="font-size:1.5em"><span id="tk" style="font-size:50%">x</span></div></div>
            <div style="font-size:24px"><div id="hs" style="display:none;font-size:smaller">x</div></div>
            <div style="font-size:24px"><div id="ts" style="font-size:smaller">x</div></div>
            <div style="font-size:20px"><div id="ha" style="display:none;font-size:12px">x</div></div>
            <div style="font-size:20px"><div id="ta" style="font-size:12px">x</div></div>
            <div style="font-size:10px"><div style="font-size:2em"><span id="hid" hidden>x</span><span id="ah" aria-hidden="true">x</span><div id="hp" hidden><span id="hpk">x</span></div><span id="vis">x</span></div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMFONTSIZEBOXLESS (SR1029): a `display: none` element reads the computed size (font-size needs no layout):
    /// 1.5em of 20px = 30px, its 50% child 15px, `smaller` of 24px 20px, an absolute 12px 12px; the laid-out twins agree.
    #[test]
    fn font_size_boxless_display_none_parity() {
        let mut e = font_size_boxless_engine();
        assert_eq!(font_shorthand_cs(&mut e, "h", "fontSize"), "30px", "1.5em of 20px under display:none");
        assert_eq!(font_shorthand_cs(&mut e, "hk", "fontSize"), "15px", "50% of the hidden 30px");
        assert_eq!(font_shorthand_cs(&mut e, "hs", "fontSize"), "20px", "smaller: 24 / 1.2 under display:none");
        assert_eq!(font_shorthand_cs(&mut e, "ha", "fontSize"), "12px", "an absolute size under display:none");
        assert_eq!(font_shorthand_cs(&mut e, "t", "fontSize"), "30px", "the laid-out twin");
        assert_eq!(font_shorthand_cs(&mut e, "tk", "fontSize"), "15px", "the laid-out twin's child");
        assert_eq!(font_shorthand_cs(&mut e, "ts", "fontSize"), "20px", "the laid-out smaller twin");
        assert_eq!(font_shorthand_cs(&mut e, "ta", "fontSize"), "12px", "the laid-out absolute twin");
    }

    /// AETHERCSSOMFONTSIZEBOXLESS (SR1029): a `hidden` / `aria-hidden` node and a `hidden` subtree's child under `2em` of
    /// 10px read 20px, the same as their laid-out sibling (they read 32px: the parent's specified size parsed at 16px).
    #[test]
    fn font_size_boxless_hidden_reads_parent_computed() {
        let mut e = font_size_boxless_engine();
        assert_eq!(font_shorthand_cs(&mut e, "vis", "fontSize"), "20px", "the laid-out sibling (reference)");
        assert_eq!(font_shorthand_cs(&mut e, "hid", "fontSize"), "20px", "a hidden span inherits the parent's computed px");
        assert_eq!(font_shorthand_cs(&mut e, "ah", "fontSize"), "20px", "an aria-hidden span the same");
        assert_eq!(font_shorthand_cs(&mut e, "hp", "fontSize"), "20px", "a hidden div the same");
        assert_eq!(font_shorthand_cs(&mut e, "hpk", "fontSize"), "20px", "the hidden div's child chains it");
    }

    // ---- AETHERCSSALIGNSELFARM (LEDGER SR1068): css-align-3 §6.2 align-self on a flex item (named tail block) ----

    /// AETHERCSSALIGNSELFARM (SR1068): 200px-tall flex rows, one 50 x 50 item each, the rules in a sheet: `align-self:
    /// center` sits at y 75 and `flex-end` at y 150 (before, the declaration was dropped and every item sat at y 0, the
    /// control row's place); `auto` under `align-items: center` follows the container (75); `-webkit-align-self: center`
    /// is the same arm (75); a later rule's `auto` beats an earlier `center` under `flex-start` (0); `bogus` is recorded.
    #[test]
    fn align_self_engine() {
        crate::ledger::reset();
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; width: 200px; height: 200px }
            .k { width: 50px; height: 50px }
            .c { align-self: center }
            .e { align-self: flex-end }
            .ai { align-items: center }
            .au { align-self: auto }
            .w { -webkit-align-self: center }
            .fs { align-items: flex-start }
            .c.au2 { align-self: auto }
            .b { align-self: bogus }
            </style></head><body style="margin:0">
            <div id="n" class="f"><div id="nk" class="k"></div></div>
            <div id="c" class="f"><div id="ck" class="k c"></div></div>
            <div id="e" class="f"><div id="ek" class="k e"></div></div>
            <div id="a" class="f ai"><div id="ak" class="k au"></div></div>
            <div id="w" class="f"><div id="wk" class="k w"></div></div>
            <div id="x" class="f fs"><div id="xk" class="k c au2"></div></div>
            <div id="b" class="f"><div id="bk" class="k b"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 1600));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let top = |e: &mut crate::AetherEngine, k: &str, row: &str| imgalt_rect(e, k).1 - imgalt_rect(e, row).1;
        let near = |v: f64, want: f64| (v - want).abs() <= 0.5;
        let n = top(&mut e, "nk", "n");
        assert!(near(n, 0.0), "control: no align-self sits at the top: {n}");
        let c = top(&mut e, "ck", "c");
        assert!(near(c, 75.0), "armed: align-self: center at y 75: {c}");
        let f = top(&mut e, "ek", "e");
        assert!(near(f, 150.0), "align-self: flex-end at y 150: {f}");
        let a = top(&mut e, "ak", "a");
        assert!(near(a, 75.0), "auto follows align-items: center: {a}");
        let w = top(&mut e, "wk", "w");
        assert!(near(w, 75.0), "-webkit-align-self: center: {w}");
        let x = top(&mut e, "xk", "x");
        assert!(near(x, 0.0), "a later rule's auto beats center under flex-start: {x}");
        let snap = crate::ledger::snapshot();
        assert!(snap.contains(crate::ledger::ApiCategory::Css, "align-self-value:bogus"), "the refused value is recorded");
    }


    // ---- AETHERCOLORSCHEMESYSTEMCOLORS (LEDGER SR1046): the system colour keywords paint and read under the element's color-scheme (named tail block) ----

    /// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): a red body (red ink, red ground, so black / white / #121212 all stand out)
    /// under `:root { color-scheme: <scheme> }`: `#t` CanvasText text, `#bg` a Canvas box, `#bb` a 4px ButtonBorder frame,
    /// `#lk` LinkText text, `#ch` a span with no colour of its own inside a CanvasText `#cp`.
    fn system_color_engine(scheme: &str) -> crate::AetherEngine {
        let html = format!(r##"<!doctype html><html><head><style>
            :root {{ color-scheme: {scheme} }}
            body {{ margin: 0; font: 20px serif; color: rgb(255, 0, 0); background: rgb(255, 0, 0) }}
            div {{ margin: 0 0 12px }}
            #t {{ color: CanvasText }}
            #bg {{ background: Canvas; height: 20px }}
            #bb {{ border: 4px solid; border-color: ButtonBorder; width: 100px; height: 20px }}
            #lk {{ color: LinkText }}
            #cp {{ color: CanvasText }}
            </style></head><body>
            <div id="t">Text</div>
            <div id="bg"></div>
            <div id="bb"></div>
            <div id="lk">Link</div>
            <div id="cp"><span id="ch">Kid</span></div>
        </body></html>"##);
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    fn system_color_white(c: (u8, u8, u8)) -> bool {
        c.0 > 200 && c.1 > 200 && c.2 > 200
    }

    fn system_color_black(c: (u8, u8, u8)) -> bool {
        c.0 < 60 && c.1 < 60 && c.2 < 60
    }

    fn system_color_dark_canvas(c: (u8, u8, u8)) -> bool {
        c == (18, 18, 18)
    }

    fn system_color_dark_frame(c: (u8, u8, u8)) -> bool {
        c == (133, 133, 133)
    }

    fn system_color_dark_link(c: (u8, u8, u8)) -> bool {
        c.2 > 230 && (140..176).contains(&c.0) && (140..176).contains(&c.1)
    }

    /// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): `color: CanvasText` under dark paints white text and getComputedStyle
    /// reads `rgb(255, 255, 255)`; a child with no colour of its own inherits the white.
    #[test]
    fn system_color_dark_canvastext_ink() {
        let mut e = system_color_engine("dark");
        assert!(css_wide_color_ink(&mut e, "t", system_color_white) > 3, "CanvasText is white under dark");
        assert_eq!(css_wide_color_ink(&mut e, "t", system_color_black), 0, "no black text");
        assert_eq!(font_shorthand_cs(&mut e, "t", "color"), "rgb(255, 255, 255)", "the CSSOM reads the resolved white");
        assert!(css_wide_color_ink(&mut e, "ch", system_color_white) > 3, "the child inherits the white");
    }

    /// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): Canvas paints #121212, ButtonBorder the dark frame #858585, LinkText
    /// #9e9eff under dark.
    #[test]
    fn system_color_dark_background_border_link() {
        let mut e = system_color_engine("dark");
        assert!(css_wide_color_ink(&mut e, "bg", system_color_dark_canvas) > 100, "Canvas is #121212 under dark");
        assert!(css_wide_color_ink(&mut e, "bb", system_color_dark_frame) > 100, "ButtonBorder is #858585 under dark");
        assert!(css_wide_color_ink(&mut e, "lk", system_color_dark_link) > 3, "LinkText is #9e9eff under dark");
    }

    /// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): the light twins (armed): css_core's light values, as before this arc.
    #[test]
    fn system_color_light_twins() {
        for scheme in ["light", "normal", "light dark"] {
            let mut e = system_color_engine(scheme);
            assert!(css_wide_color_ink(&mut e, "t", system_color_black) > 3, "{scheme}: CanvasText is black");
            assert_eq!(css_wide_color_ink(&mut e, "t", system_color_white), 0, "{scheme}: no white text");
            assert_eq!(font_shorthand_cs(&mut e, "t", "color"), "rgb(0, 0, 0)", "{scheme}: the CSSOM reads black");
            assert!(css_wide_color_ink(&mut e, "bg", system_color_white) > 100, "{scheme}: Canvas is white");
            assert_eq!(css_wide_color_ink(&mut e, "bg", system_color_dark_canvas), 0, "{scheme}: no #121212");
            assert_eq!(css_wide_color_ink(&mut e, "bb", system_color_dark_frame), 0, "{scheme}: no dark frame");
            assert_eq!(css_wide_color_ink(&mut e, "lk", system_color_dark_link), 0, "{scheme}: no dark link blue");
        }
    }

    // ---- AETHERCSSTEXTDECORATIONSTYLETHICKNESS (LEDGER SR1038): text-decoration-style / -thickness paint (named tail block) ----

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): one 20px serif row per probe (the face's thickness 2px), black
    /// text, red decorations, rows 20px apart: `#ds` solid underline (the twin), `#dw` wavy, `#dd` double, `#do` dotted,
    /// `#dk` `text-decoration-thickness: 4px`, `#dh` the shorthand `underline wavy red 2px`; `#ls` / `#lw` line-through
    /// solid / wavy.
    fn text_decoration_style_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 20px; text-decoration-color: red }
            </style></head><body style="margin:0;font:20px serif;color:black">
            <div id="ds" style="text-decoration-line:underline">Hgjx Hgjx</div>
            <div id="dw" style="text-decoration-line:underline;text-decoration-style:wavy">Hgjx Hgjx</div>
            <div id="dd" style="text-decoration-line:underline;text-decoration-style:double">Hgjx Hgjx</div>
            <div id="do" style="text-decoration-line:underline;text-decoration-style:dotted">Hgjx Hgjx</div>
            <div id="dk" style="text-decoration-line:underline;text-decoration-thickness:4px">Hgjx Hgjx</div>
            <div id="dh" style="text-decoration:underline wavy red 2px">Hgjx Hgjx</div>
            <div id="ls" style="text-decoration-line:line-through">Hgjx Hgjx</div>
            <div id="lw" style="text-decoration-line:line-through;text-decoration-style:wavy">Hgjx Hgjx</div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 800));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): the red pixels of element `id`'s rect extended 8px down, as
    /// (the sorted distinct rows holding red, the sorted distinct columns holding red).
    fn text_decoration_style_red(e: &mut crate::AetherEngine, id: &str) -> (Vec<usize>, Vec<usize>) {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (l.floor().max(0.0) as usize, ((l + w).ceil() as usize).min(sw));
        let (y0, y1) = (t.floor().max(0.0) as usize, ((t + h).ceil() as usize + 8).min(sh));
        let (mut rows, mut cols) = (Vec::new(), Vec::new());
        for y in y0..y1 {
            for x in x0..x1 {
                if css_wide_red(imgalt_px(e, x, y)) {
                    rows.push(y);
                    cols.push(x);
                }
            }
        }
        rows.sort_unstable();
        rows.dedup();
        cols.sort_unstable();
        cols.dedup();
        (rows, cols)
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a wavy underline paints red above AND below the rows the solid
    /// twin paints (the twin: one band of the face's thickness, 2 rows); the shorthand `underline wavy red 2px` the same.
    #[test]
    fn text_decoration_style_wavy_above_and_below() {
        let mut e = text_decoration_style_engine();
        let (solid, _) = text_decoration_style_red(&mut e, "ds");
        assert_eq!(solid.len(), 2, "armed: the solid twin paints one 2-row band, got {solid:?}");
        let solid_top = solid[0] as i64 - imgalt_rect(&mut e, "ds").1.floor() as i64;
        let solid_bot = *solid.last().unwrap() as i64 - imgalt_rect(&mut e, "ds").1.floor() as i64;
        for id in ["dw", "dh"] {
            let (wavy, _) = text_decoration_style_red(&mut e, id);
            assert!(!wavy.is_empty(), "#{id} paints red");
            let top = imgalt_rect(&mut e, id).1.floor() as i64;
            assert!((wavy[0] as i64 - top) < solid_top, "#{id} has red above the solid rows: {wavy:?} vs {solid:?}");
            assert!((*wavy.last().unwrap() as i64 - top) > solid_bot, "#{id} has red below the solid rows: {wavy:?} vs {solid:?}");
        }
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a double underline paints two bands with a red-free row between;
    /// the solid twin's rows are contiguous.
    #[test]
    fn text_decoration_style_double_two_rows() {
        let mut e = text_decoration_style_engine();
        let gap = |r: &[usize]| r.windows(2).any(|w| w[1] > w[0] + 1);
        let (double, _) = text_decoration_style_red(&mut e, "dd");
        assert!(double.len() >= 4 && gap(&double), "#dd paints two separated bands: {double:?}");
        let (solid, _) = text_decoration_style_red(&mut e, "ds");
        assert!(!gap(&solid), "armed: the solid twin is one band: {solid:?}");
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a dotted underline leaves red-free columns between its dots; the
    /// solid twin's columns are contiguous.
    #[test]
    fn text_decoration_style_dotted_gaps() {
        let mut e = text_decoration_style_engine();
        let gap = |c: &[usize]| c.windows(2).filter(|w| w[1] > w[0] + 1).count();
        let (_, dotted) = text_decoration_style_red(&mut e, "do");
        assert!(dotted.len() > 10 && gap(&dotted) > 5, "#do leaves gaps between dots: {} gaps", gap(&dotted));
        let (_, solid) = text_decoration_style_red(&mut e, "ds");
        assert!(solid.len() > 40 && gap(&solid) == 0, "armed: the solid twin has no gap column");
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): `text-decoration-thickness: 4px` paints a 4-row band (the solid
    /// twin at the face's thickness: 2 rows).
    #[test]
    fn text_decoration_style_thickness_band() {
        let mut e = text_decoration_style_engine();
        let (thick, _) = text_decoration_style_red(&mut e, "dk");
        assert_eq!(thick.len(), 4, "#dk paints 4 rows: {thick:?}");
        let (solid, _) = text_decoration_style_red(&mut e, "ds");
        assert_eq!(solid.len(), 2, "armed: the twin paints 2 rows: {solid:?}");
    }

    /// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a line-through takes the style the same way: wavy spans more rows
    /// than the solid line-through, above and below it.
    #[test]
    fn text_decoration_style_line_through_wavy() {
        let mut e = text_decoration_style_engine();
        let (solid, _) = text_decoration_style_red(&mut e, "ls");
        let (wavy, _) = text_decoration_style_red(&mut e, "lw");
        assert_eq!(solid.len(), 2, "armed: the solid line-through is one 2-row band: {solid:?}");
        let (ts, tw) = (imgalt_rect(&mut e, "ls").1.floor() as i64, imgalt_rect(&mut e, "lw").1.floor() as i64);
        assert!(!wavy.is_empty(), "#lw paints red");
        assert!((wavy[0] as i64 - tw) < (solid[0] as i64 - ts), "#lw reaches above: {wavy:?} vs {solid:?}");
        assert!((*wavy.last().unwrap() as i64 - tw) > (*solid.last().unwrap() as i64 - ts), "#lw reaches below");
    }


    // ---- AETHERCSSTEXTDECORATIONPROPAGATEINK (LEDGER SR1053): currentcolor resolves at the decorating box, not the run (named tail block) ----

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): 20px serif rows of black text 12px apart, each probe a child span of a
    /// decorating parent: `#pu` blue under a red `u`; `#pc` blue with its own underline under a red `u`; `#pg` blue under a
    /// red parent whose line is an explicit green; `#pa` red under a link (UA link blue (0, 0, 238)); `#pn` an uncoloured
    /// span under a plain underlined div (the control).
    fn text_decoration_propagate_ink_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            </style></head><body style="margin:0;font:20px serif;color:black">
            <div><u style="color:red"><span id="pu" style="color:blue">Hgjx Hgjx</span></u></div>
            <div><u style="color:red"><span id="pc" style="color:blue;text-decoration:underline">Hgjx Hgjx</span></u></div>
            <div style="color:red;text-decoration:underline green"><span id="pg" style="color:blue">Hgjx Hgjx</span></div>
            <div><a href="#x"><span id="pa" style="color:red">Hgjx Hgjx</span></a></div>
            <div style="text-decoration:underline"><span id="pn">Hgjx Hgjx</span></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): `<u style="color:red"><span style="color:blue">` underlines in the
    /// decorating box's RED under blue glyphs (Chromium); before SR1053 the line took the run's blue.
    #[test]
    fn text_decoration_propagate_ink_u_red_child_blue() {
        let mut e = text_decoration_propagate_ink_engine();
        assert!(text_decoration_ink(&mut e, "pu", css_wide_red) > 40, "#pu paints the u's red underline");
        assert!(text_decoration_ink(&mut e, "pu", css_wide_blue) > 40, "armed: #pu's glyphs are blue");
    }

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): a child with its own `text-decoration: underline` is its own decorating
    /// box: its line is its own blue, no red.
    #[test]
    fn text_decoration_propagate_ink_child_own_line() {
        let mut e = text_decoration_propagate_ink_engine();
        assert!(text_decoration_ink(&mut e, "pc", css_wide_blue) > 40, "#pc paints blue");
        assert_eq!(text_decoration_ink(&mut e, "pc", css_wide_red), 0, "#pc's own line is blue, not the u's red");
    }

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): an explicit `text-decoration-color` (green) on the decorating parent
    /// wins over both its red colour and the child's blue.
    #[test]
    fn text_decoration_propagate_ink_explicit_parent_wins() {
        let mut e = text_decoration_propagate_ink_engine();
        assert!(text_decoration_ink(&mut e, "pg", css_wide_green) > 40, "#pg paints the parent's green line");
        assert_eq!(text_decoration_ink(&mut e, "pg", css_wide_red), 0, "#pg paints no red");
    }

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): a link's underline is the link colour (the UA (0, 0, 238), Chromium's
    /// -webkit-link) under a red child.
    #[test]
    fn text_decoration_propagate_ink_link_colour() {
        let mut e = text_decoration_propagate_ink_engine();
        assert!(text_decoration_ink(&mut e, "pa", css_wide_blue) > 40, "#pa paints the link-blue underline");
        assert!(text_decoration_ink(&mut e, "pa", css_wide_red) > 40, "armed: #pa's glyphs are red");
    }

    /// AETHERCSSTEXTDECORATIONPROPAGATEINK (SR1053): the control: an uncoloured child of a plain underlined div paints its
    /// line black, no red.
    #[test]
    fn text_decoration_propagate_ink_plain_control() {
        let mut e = text_decoration_propagate_ink_engine();
        assert!(text_decoration_ink(&mut e, "pn", text_decoration_black) > 40, "#pn paints black");
        assert_eq!(text_decoration_ink(&mut e, "pn", css_wide_red), 0, "#pn paints no red");
    }

    // ---- AETHERCSSOMCOLUMNSREST (LEDGER SR1036): white-space's longhands and text-decoration-color as report columns (named tail block) ----

    /// AETHERCSSOMCOLUMNSREST (SR1036): one div per white-space value (and a child of the pre div), and one div per
    /// text-decoration-color case: an author red, unset on green text, `currentcolor` on green text, the
    /// `text-decoration` shorthand's reset on green text, and a `color: inherit` child of a blue parent.
    fn cssom_columns_rest_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            #dg, #dc, #ds { color: green }
            #dc { text-decoration-color: currentcolor }
            #ds { text-decoration: underline }
            </style></head><body style="margin:0;font:16px serif">
            <div id="wn">a</div>
            <div id="wp" style="white-space:pre"><div id="wpc">a</div></div>
            <div id="ww" style="white-space:nowrap">a</div>
            <div id="wr" style="white-space:pre-wrap">a</div>
            <div id="wl" style="white-space:pre-line">a</div>
            <div id="wb" style="white-space:break-spaces">a</div>
            <div id="dr" style="text-decoration-color:red">a</div>
            <div id="dg">a</div>
            <div id="dc">a</div>
            <div id="ds">a</div>
            <div style="color:blue"><div id="di" style="color:inherit">a</div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOMCOLUMNSREST (SR1036): white-space-collapse / text-wrap-mode read the css-text-4 pair of the one
    /// white-space code (Chromium: normal is collapse / wrap); the pre div's child inherits preserve / nowrap.
    #[test]
    fn cssom_columns_rest_white_space_longhands() {
        let mut e = cssom_columns_rest_engine();
        for (id, collapse, wrap) in [
            ("wn", "collapse", "wrap"),
            ("wp", "preserve", "nowrap"),
            ("wpc", "preserve", "nowrap"),
            ("ww", "collapse", "nowrap"),
            ("wr", "preserve", "wrap"),
            ("wl", "preserve-breaks", "wrap"),
            ("wb", "break-spaces", "wrap"),
        ] {
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('white-space-collapse')"), collapse, "#{id}: white-space-collapse");
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('text-wrap-mode')"), wrap, "#{id}: text-wrap-mode");
        }
    }

    /// AETHERCSSOMCOLUMNSREST (SR1036): text-decoration-color reads SR1018's field, currentcolor (and unset) resolved
    /// to the element's colour as Chromium reports it.
    #[test]
    fn cssom_columns_rest_decoration_color() {
        let mut e = cssom_columns_rest_engine();
        for (id, want) in [
            ("dr", "rgb(255, 0, 0)"),
            ("dg", "rgb(0, 128, 0)"),
            ("dc", "rgb(0, 128, 0)"),
            ("ds", "rgb(0, 128, 0)"),
            ("di", "rgb(0, 0, 255)"),
            ("wn", "rgb(0, 0, 0)"),
        ] {
            assert_eq!(font_shorthand_cs(&mut e, id, "getPropertyValue('text-decoration-color')"), want, "#{id}: text-decoration-color");
            assert_eq!(font_shorthand_cs(&mut e, id, "textDecorationColor"), want, "#{id}: textDecorationColor");
        }
    }

    /// AETHERCSSOMCOLUMNSREST (SR1036): the three columns sit after direction (no earlier index moves) and the
    /// computed declaration enumerates all 24.
    #[test]
    fn cssom_columns_rest_report_width() {
        assert_eq!(crate::css::REPORT_PROPS.len(), crate::css::REPORT_COLS);
        assert_eq!(crate::css::REPORT_COLS, 25); // SR1036 fold: unicode-bidi (SR1008) sits at 21 before the three
        assert_eq!(crate::css::REPORT_PROPS[20], "direction");
        assert_eq!(&crate::css::REPORT_PROPS[22..], &["white-space-collapse", "text-wrap-mode", "text-decoration-color"]);
        let mut e = cssom_columns_rest_engine();
        assert_eq!(font_synth_eval(&mut e, "String(getComputedStyle(document.getElementById('wn')).length)"), "25"); // SR1036 fold: 25 columns with unicode-bidi
        assert_eq!(font_synth_eval(&mut e, "getComputedStyle(document.getElementById('wn')).item(24)"), "text-decoration-color");
    }

    // ---- AETHERCSSPLACESHORTHANDS (LEDGER SR1074): css-align-3 §6.4 / §7.4 / §8.3 place-content / place-items / place-self (named tail block) ----

    /// AETHERCSSPLACESHORTHANDS (SR1074): 200 x 200 flex containers, one 50 x 50 child each, the rules in a sheet:
    /// `place-content: center` on a wrapping container centres the child on both axes (75, 75; before, the declaration
    /// was dropped and the child sat at 0, 0, the control's place); on a nowrap (single-line) container align-content
    /// has no effect (css-flexbox §9.4), so only the justify half moves it (75, 0); `bogus` is refused by both arms.
    #[test]
    fn place_content_center_engine() {
        crate::ledger::reset();
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; width: 200px; height: 200px }
            .w { flex-wrap: wrap }
            .k { width: 50px; height: 50px }
            .pc { place-content: center }
            .b { place-content: bogus }
            </style></head><body style="margin:0">
            <div id="n" class="f w"><div id="nk" class="k"></div></div>
            <div id="c" class="f w pc"><div id="ck" class="k"></div></div>
            <div id="s" class="f pc"><div id="sk" class="k"></div></div>
            <div id="b" class="f w b"><div id="bk" class="k"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 1000));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let at = |e: &mut crate::AetherEngine, k: &str, row: &str| {
            let (a, b) = (imgalt_rect(e, k), imgalt_rect(e, row));
            (a.0 - b.0, a.1 - b.1)
        };
        let near = |v: (f64, f64), want: (f64, f64)| (v.0 - want.0).abs() <= 0.5 && (v.1 - want.1).abs() <= 0.5;
        let n = at(&mut e, "nk", "n");
        assert!(near(n, (0.0, 0.0)), "control: no place-content sits at the start: {n:?}");
        let c = at(&mut e, "ck", "c");
        assert!(near(c, (75.0, 75.0)), "armed: place-content: center on a wrapping row at (75, 75): {c:?}");
        let s = at(&mut e, "sk", "s");
        assert!(near(s, (75.0, 0.0)), "single-line: the justify half only: {s:?}");
        let b = at(&mut e, "bk", "b");
        assert!(near(b, (0.0, 0.0)), "bogus: nothing written: {b:?}");
        let snap = crate::ledger::snapshot();
        assert!(snap.contains(crate::ledger::ApiCategory::Css, "align-content-value:bogus"), "the align half's refusal is recorded");
    }

    /// AETHERCSSPLACESHORTHANDS (SR1074): `place-items: center` on a 200px-tall flex row puts its 50 x 50 child at
    /// y 75 (x stays 0: SR1088 applies the justify-items half, which flex layout ignores, css-align-3 §6.1; the
    /// `place-justify-dropped:` record is retired); `place-self: center` on an item the same.
    #[test]
    fn place_items_center_engine() {
        crate::ledger::reset();
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; width: 200px; height: 200px }
            .k { width: 50px; height: 50px }
            .pi { place-items: center }
            .ps { place-self: center }
            </style></head><body style="margin:0">
            <div id="n" class="f"><div id="nk" class="k"></div></div>
            <div id="i" class="f pi"><div id="ik" class="k"></div></div>
            <div id="p" class="f"><div id="pk" class="k ps"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 1000));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let at = |e: &mut crate::AetherEngine, k: &str, row: &str| {
            let (a, b) = (imgalt_rect(e, k), imgalt_rect(e, row));
            (a.0 - b.0, a.1 - b.1)
        };
        let near = |v: (f64, f64), want: (f64, f64)| (v.0 - want.0).abs() <= 0.5 && (v.1 - want.1).abs() <= 0.5;
        let n = at(&mut e, "nk", "n");
        assert!(near(n, (0.0, 0.0)), "control: the child sits at the top: {n:?}");
        let i = at(&mut e, "ik", "i");
        assert!(near(i, (0.0, 75.0)), "armed: place-items: center at y 75, x 0: {i:?}");
        let p = at(&mut e, "pk", "p");
        assert!(near(p, (0.0, 75.0)), "place-self: center at y 75: {p:?}");
        let snap = crate::ledger::snapshot();
        assert!(!snap.contains(crate::ledger::ApiCategory::Css, "place-justify-dropped:center"), "SR1088: the justify half is applied, not recorded");
    }

    // ---- AETHERCSSJUSTIFYITEMSSELF (LEDGER SR1088): css-align-3 §6.2 / §6.3 justify-items / justify-self (named tail block) ----

    /// AETHERCSSJUSTIFYITEMSSELF (SR1088): (a) `justify-items: center` / `justify-self: end` on a 200 x 200 flex row are
    /// parsed (no `property:` unhandled record) and ignored by flex layout (css-align-3 §6.1): the 50 x 50 child stays
    /// at x 0; a bogus value is refused and recorded. (b) an abspos 50 x 50 box with `left: 0; right: 0` in a 200px
    /// relative box is aligned by `justify-self: center` in its inset-modified containing block: x 75 (control: 0).
    #[test]
    fn justify_items_self_engine() {
        crate::ledger::reset();
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; width: 200px; height: 200px }
            .k { width: 50px; height: 50px }
            .ji { justify-items: center }
            .js { justify-self: end }
            .jb { justify-items: bogus }
            .r { position: relative; width: 200px; height: 100px }
            .a { position: absolute; left: 0; right: 0; top: 0; width: 50px; height: 50px }
            .jc { justify-self: center }
            </style></head><body style="margin:0">
            <div id="i" class="f ji"><div id="ik" class="k"></div></div>
            <div id="s" class="f"><div id="sk" class="k js"></div></div>
            <div id="b" class="f jb"><div id="bk" class="k"></div></div>
            <div id="r" class="r"><div id="rk" class="a jc"></div></div>
            <div id="c" class="r"><div id="ck" class="a"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 1000));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let at = |e: &mut crate::AetherEngine, k: &str, row: &str| {
            let (a, b) = (imgalt_rect(e, k), imgalt_rect(e, row));
            (a.0 - b.0, a.1 - b.1)
        };
        let near = |v: (f64, f64), want: (f64, f64)| (v.0 - want.0).abs() <= 0.5 && (v.1 - want.1).abs() <= 0.5;
        for (k, row) in [("ik", "i"), ("sk", "s"), ("bk", "b")] {
            let v = at(&mut e, k, row);
            assert!(near(v, (0.0, 0.0)), "{k}: flex ignores justify-items / justify-self: {v:?}");
        }
        let c = at(&mut e, "ck", "c");
        assert!(near(c, (0.0, 0.0)), "control: the abspos box without justify-self sits at x 0: {c:?}");
        let r = at(&mut e, "rk", "r");
        assert!(near(r, (75.0, 0.0)), "armed: justify-self: center centres the abspos box at x 75: {r:?}");
        let snap = crate::ledger::snapshot();
        assert!(!snap.contains(crate::ledger::ApiCategory::Css, "property:justify-items"), "justify-items has an arm");
        assert!(!snap.contains(crate::ledger::ApiCategory::Css, "property:justify-self"), "justify-self has an arm");
        assert!(snap.contains(crate::ledger::ApiCategory::Css, "justify-items-value:bogus"), "the bogus value is refused and recorded");
    }

    // ---- AETHERCSSCARETCOLOR (LEDGER SR1040): caret-color paints the focused text control's caret (named tail block) ----

    /// AETHERCSSCARETCOLOR (SR1040): `body` holds one `autofocus` control `#f` (the `focused_field_paints_caret_and_ring`
    /// shape: margin 10px, empty value); the engine after its first frame, with the caret in its visible phase.
    fn caret_color_engine(body: &str) -> crate::AetherEngine {
        let html = format!(r#"<html><body style="margin:0">{body}</body></html>"#);
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 120));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCARETCOLOR (SR1040): the columns of `#f`'s mid-height row inside its 2px edges whose pixel passes `hue`.
    fn caret_color_cols(e: &crate::AetherEngine, hue: fn((u8, u8, u8)) -> bool) -> Vec<u32> {
        let f = dom_by_id(e, "f");
        let (x, y, w, h) = e.box_of(&f).unwrap();
        let mid = (y + h / 2.0) as u32;
        ((x as u32 + 2)..((x + w) as u32 - 2)).filter(|&cx| hue(imgalt_px(e, cx as usize, mid as usize))).collect()
    }

    fn caret_color_dark(c: (u8, u8, u8)) -> bool {
        c.0 < 60
    }

    /// AETHERCSSCARETCOLOR (SR1040): `caret-color: red` paints the caret one red column (the field text stays FieldText);
    /// armed: the plain field's caret is one dark column and no red.
    #[test]
    fn caret_color_red_paints_red() {
        let e = caret_color_engine(r#"<input id="f" autofocus style="margin:10px;caret-color:red" value="">"#);
        assert_eq!(caret_color_cols(&e, css_wide_red).len(), 1, "one red caret column");
        assert!(caret_color_cols(&e, caret_color_dark).is_empty(), "no dark caret");
        let plain = caret_color_engine(r#"<input id="f" autofocus style="margin:10px" value="">"#);
        assert_eq!(caret_color_cols(&plain, caret_color_dark).len(), 1, "armed: the plain caret is the ink");
        assert!(caret_color_cols(&plain, css_wide_red).is_empty(), "armed: the plain caret is not red");
    }

    /// AETHERCSSCARETCOLOR (SR1040): caret-color is inherited: an input under `caret-color: red` paints a red caret, and
    /// so does one that says `caret-color: inherit`.
    #[test]
    fn caret_color_inherits_to_input() {
        let e = caret_color_engine(r#"<div style="caret-color:red"><input id="f" autofocus style="margin:10px" value=""></div>"#);
        assert_eq!(caret_color_cols(&e, css_wide_red).len(), 1, "the parent's red caret");
        let e = caret_color_engine(r#"<div style="caret-color:red"><input id="f" autofocus style="margin:10px;caret-color:inherit" value=""></div>"#);
        assert_eq!(caret_color_cols(&e, css_wide_red).len(), 1, "caret-color: inherit takes the parent's red");
    }

    /// AETHERCSSCARETCOLOR (SR1040): `caret-color: auto` is the control's currentcolor: under `color: blue` (and a parent's
    /// red caret-color it resets) the caret is blue.
    #[test]
    fn caret_color_auto_is_currentcolor() {
        let e = caret_color_engine(r#"<div style="caret-color:red"><input id="f" autofocus style="margin:10px;color:blue;caret-color:auto" value=""></div>"#);
        assert_eq!(caret_color_cols(&e, css_wide_blue).len(), 1, "one blue caret column");
        assert!(caret_color_cols(&e, css_wide_red).is_empty(), "auto drops the parent's red");
    }

    /// AETHERCSSCARETCOLOR (SR1040): `caret-color: transparent` (the hide-the-caret idiom) paints no caret.
    #[test]
    fn caret_color_transparent_hides_caret() {
        let e = caret_color_engine(r#"<input id="f" autofocus style="margin:10px;caret-color:transparent" value="">"#);
        assert!(caret_color_cols(&e, caret_color_dark).is_empty(), "no dark caret");
        assert!(caret_color_cols(&e, css_wide_red).is_empty(), "no red");
    }

    /// AETHERCSSCARETCOLOR (SR1040): a focused textarea's caret reads caret-color too: red pixels inside its 2px edges;
    /// armed: a plain textarea paints none.
    #[test]
    fn caret_color_textarea() {
        let red_px = |e: &crate::AetherEngine| {
            let f = dom_by_id(e, "f");
            let (x, y, w, h) = e.box_of(&f).unwrap();
            let mut n = 0;
            for py in (y as u32 + 2)..((y + h) as u32 - 2) {
                for px in (x as u32 + 2)..((x + w) as u32 - 2) {
                    if css_wide_red(imgalt_px(e, px as usize, py as usize)) {
                        n += 1;
                    }
                }
            }
            n
        };
        let e = caret_color_engine(r#"<textarea id="f" autofocus style="margin:10px;caret-color:red"></textarea>"#);
        assert!(red_px(&e) >= 8, "a red caret in the textarea");
        let plain = caret_color_engine(r#"<textarea id="f" autofocus style="margin:10px"></textarea>"#);
        assert_eq!(red_px(&plain), 0, "armed: the plain textarea caret is not red");
    }

    // ---- AETHERCSSSUPPORTSVALUE (LEDGER SR1048): @supports checks the whole declaration, the value through layout_core's parse road (named tail block) ----

    /// AETHERCSSSUPPORTSVALUE (SR1048): a grid-or-fallback page: `@supports (display: grid)` makes `.x` red,
    /// `@supports not (display: grid)` makes it green; Aether cannot lay grid out, so the fallback applies (before SR1048
    /// the name-only check answered true and `.x` painted red). `@supports (display: flex)` makes `.y` red (armed: a
    /// supported condition still applies its block).
    #[test]
    fn supports_value_engine_grid_fallback() {
        let html = r#"<!doctype html><html><head><style>
            @supports (display: grid) { .x { color: rgb(255, 0, 0) } }
            @supports not (display: grid) { .x { color: green } }
            @supports (display: flex) { .y { color: rgb(255, 0, 0) } }
            </style></head><body style="margin:0;font:20px serif">
            <div id="x" class="x">Hello</div>
            <div id="y" class="y">Hello</div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 600));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(font_shorthand_cs(&mut e, "y", "color"), "rgb(255, 0, 0)", "armed: @supports (display: flex) applies");
        assert!(css_wide_color_ink(&mut e, "y", css_wide_red) > 3, "armed: #y paints red");
        assert_eq!(font_shorthand_cs(&mut e, "x", "color"), "rgb(0, 128, 0)", "the not (display: grid) fallback wins");
        assert!(css_wide_color_ink(&mut e, "x", css_wide_green) > 3, "#x paints green");
        assert_eq!(css_wide_color_ink(&mut e, "x", css_wide_red), 0, "#x paints no red");
    }

    // ---- AETHERLINEHEIGHTCALCNEGATIVECLAMP (LEDGER SR1054): a negative calc() line-height is a zero line box (named tail block) ----

    /// AETHERLINEHEIGHTCALCNEGATIVECLAMP (SR1054): two-line divs ("Hello Hello" in 60px) at `calc(-4px)`,
    /// `calc(0px - 5px)` and the `normal` twin.
    fn line_height_calc_negative_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><body style="margin:0;font:16px serif">
            <div id="c4" style="width:60px;line-height:calc(-4px)">Hello Hello</div>
            <div id="c5" style="width:60px;line-height:calc(0px - 5px)">Hello Hello</div>
            <div id="cn" style="width:60px;line-height:normal">Hello Hello</div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERLINEHEIGHTCALCNEGATIVECLAMP (SR1054): css-values-4 §10.9 clamps a negative calc() to 0, so the two-line block
    /// is 0 px tall (it laid out as `normal`); the `normal` twin is two lines tall (armed).
    #[test]
    fn line_height_calc_negative_box_height() {
        let mut e = line_height_calc_negative_engine();
        for id in ["c4", "c5"] {
            let h = imgalt_rect(&mut e, id).3;
            assert_eq!(h, 0.0, "#{id}: two zero line boxes");
        }
        let h = imgalt_rect(&mut e, "cn").3;
        assert!(h > 20.0, "#cn: two normal lines ({h})");
    }

    /// AETHERLINEHEIGHTCALCNEGATIVECLAMP (SR1054): getComputedStyle reads `0px` for a negative calc() (it read `normal`).
    #[test]
    fn line_height_calc_negative_cssom() {
        let mut e = line_height_calc_negative_engine();
        for id in ["c4", "c5"] {
            assert_eq!(font_shorthand_cs(&mut e, id, "lineHeight"), "0px", "#{id}");
        }
        assert_eq!(font_shorthand_cs(&mut e, "cn", "lineHeight"), "normal");
    }

    // ---- AETHERCSSBORDERCOLORONEROAD (LEDGER SR1098): border-color takes ONE road; a colour creates no side (named tail block) ----

    /// AETHERCSSBORDERCOLORONEROAD (SR1098): two 40x20 boxes, rows 12px apart, under a green body ink: `#oa` `border: 2px
    /// solid; border-top-style: none; border-color: red`; `#ob` `border-top-style: solid; border-color: red`.
    fn border_color_one_road_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="oa" style="width:40px;height:20px;border:2px solid;border-top-style:none;border-color:red"></div>
            <div id="ob" style="width:40px;height:20px;border-top-style:solid;border-color:red"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERCOLORONEROAD (SR1098): css-backgrounds-3 §4.3 a colour creates no side: `#oa` paints no top edge
    /// (before SR1098 the one-value road re-created a 2px red top), its left edge red (armed); `#ob` paints its top red
    /// (armed) and no other edge (before SR1098 a fabricated 3px red frame on the three styleless sides).
    #[test]
    fn border_color_one_road_paints() {
        let mut e = border_color_one_road_engine();
        let a = border_side_color_edges(&mut e, "oa");
        assert!(!css_wide_red(a[0]), "#oa: no top edge: {:?}", a[0]);
        assert!(css_wide_red(a[3]), "armed: #oa's left edge is red: {:?}", a[3]);
        let b = border_side_color_edges(&mut e, "ob");
        assert!(css_wide_red(b[0]), "armed: #ob's top is red: {:?}", b[0]);
        for (i, c) in b.iter().enumerate().skip(1) {
            assert!(!css_wide_red(*c), "#ob: side {i} paints no red: {c:?}");
        }
    }

    // ---- AETHERCSSBACKGROUNDMULTILAYER (LEDGER SR1052): background-image is a comma layer list, painted bottom-up (named tail block) ----

    /// AETHERCSSBACKGROUNDMULTILAYER (SR1052): one 20px row per probe at 400 px wide, rows 12px apart. The image is a
    /// 2x2 checkerboard PNG data: URL (two opaque red pixels, two transparent: tiled, half the box red, the rest a hole),
    /// the gradient solid blue. `.ug` url over gradient, `.u` / `.g` its one-layer twins, `.gu` gradient over url,
    /// `.rp.g2` a url rule then a later gradient rule, `.sh` the shorthand's url over gradient.
    fn background_multi_layer_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div { margin: 0 0 12px; height: 20px }
            .ug { background-image: url(PNG), linear-gradient(rgb(0, 0, 255), rgb(0, 0, 255)) }
            .u { background-image: url(PNG) }
            .g { background-image: linear-gradient(rgb(0, 0, 255), rgb(0, 0, 255)) }
            .gu { background-image: linear-gradient(rgb(0, 0, 255), rgb(0, 0, 255)), url(PNG) }
            .rp { background-image: url(PNG) }
            .rp.g2 { background-image: linear-gradient(rgb(0, 0, 255), rgb(0, 0, 255)) }
            .sh { background: url(PNG), linear-gradient(rgb(0, 0, 255), rgb(0, 0, 255)) }
            </style></head><body style="margin:0">
            <div id="ug" class="ug"></div>
            <div id="u" class="u"></div>
            <div id="g" class="g"></div>
            <div id="gu" class="gu"></div>
            <div id="rp" class="rp g2"></div>
            <div id="sh" class="sh"></div>
        </body></html>"#
            .replace("PNG", "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8AARFAA4gAAI+4D/QU7+oQAAAAASUVORK5CYII=");
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBACKGROUNDMULTILAYER (SR1052): `url(PNG), linear-gradient(..)` paints the image's opaque pixels on top and
    /// the gradient through its holes; the url-only twin shows no blue and the gradient-only twin no red (armed).
    #[test]
    fn background_multi_layer_url_over_gradient() {
        let mut e = background_multi_layer_engine();
        assert!(css_wide_color_ink(&mut e, "u", css_wide_red) > 100, "the url-only twin paints red (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "u", css_wide_blue), 0, "the url-only twin has no blue (armed)");
        assert!(css_wide_color_ink(&mut e, "g", css_wide_blue) > 100, "the gradient-only twin paints blue (armed)");
        assert_eq!(css_wide_color_ink(&mut e, "g", css_wide_red), 0, "the gradient-only twin has no red (armed)");
        assert!(css_wide_color_ink(&mut e, "ug", css_wide_red) > 100, "the top image's opaque pixels");
        assert!(css_wide_color_ink(&mut e, "ug", css_wide_blue) > 100, "the gradient through the image's holes");
    }

    /// AETHERCSSBACKGROUNDMULTILAYER (SR1052): the first layer is on top: an opaque gradient listed before the url covers
    /// it (no red); a later rule's gradient replaces an earlier rule's url (no red); the shorthand's two layers both paint.
    #[test]
    fn background_multi_layer_order_replace_shorthand() {
        let mut e = background_multi_layer_engine();
        assert!(css_wide_color_ink(&mut e, "gu", css_wide_blue) > 100, "the top gradient paints");
        assert_eq!(css_wide_color_ink(&mut e, "gu", css_wide_red), 0, "the url under an opaque gradient is covered");
        assert!(css_wide_color_ink(&mut e, "rp", css_wide_blue) > 100, "the later rule's gradient paints");
        assert_eq!(css_wide_color_ink(&mut e, "rp", css_wide_red), 0, "the earlier rule's url is replaced");
        assert!(css_wide_color_ink(&mut e, "sh", css_wide_red) > 100, "shorthand: the url layer");
        assert!(css_wide_color_ink(&mut e, "sh", css_wide_blue) > 100, "shorthand: the gradient layer");
    }

    // ---- AETHERCSSBACKGROUNDSHORTHANDRESET (LEDGER SR1061): the background shorthand resets every omitted longhand (named tail block) ----

    /// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): SR1052's 2x2 red checker PNG; `.k` the image alone, `.r` the image then
    /// `background: rgb(0, 128, 0)` in one block, `.x.y` the image rule then a later shorthand rule. css-backgrounds-3
    /// §3.10: the shorthand resets the image, so `.r` and `.x.y` paint only green (Chromium: plain green).
    #[test]
    fn background_shorthand_reset_paints_colour_only() {
        let html = r#"<!doctype html><html><head><style>
            div { margin: 0 0 12px; height: 20px }
            .k { background-image: url(PNG) }
            .r { background-image: url(PNG); background: rgb(0, 128, 0) }
            .x { background-image: url(PNG) }
            .x.y { background: rgb(0, 128, 0) }
            </style></head><body style="margin:0">
            <div id="k" class="k"></div>
            <div id="r" class="r"></div>
            <div id="xy" class="x y"></div>
        </body></html>"#
            .replace("PNG", "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8AARFAA4gAAI+4D/QU7+oQAAAAASUVORK5CYII=");
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 600));
        e.load_html("https://example.com/", &html, true);
        e.render_frame();
        assert!(css_wide_color_ink(&mut e, "k", css_wide_red) > 100, "the image alone paints its checker (armed)");
        assert!(css_wide_color_ink(&mut e, "r", css_wide_green) > 100, "one block: the shorthand's green");
        assert_eq!(css_wide_color_ink(&mut e, "r", css_wide_red), 0, "one block: no checker pixel");
        assert!(css_wide_color_ink(&mut e, "xy", css_wide_green) > 100, "a later rule: the shorthand's green");
        assert_eq!(css_wide_color_ink(&mut e, "xy", css_wide_red), 0, "a later rule: the earlier image is reset");
    }

    // ---- AETHERCSSBACKGROUNDLAYERPROPS (LEDGER SR1070): background-repeat / -position / -size are per-layer lists (named tail block) ----

    /// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): 400x20 rows 12px apart; G is a 1x1 opaque green PNG, B a 1x1 opaque blue
    /// one. `.lh` the longhand lists, `.sh` the shorthand (a logo over a texture), `.one` G alone repeating (control).
    fn background_layer_props_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            div { margin: 0 0 12px; height: 20px }
            .lh { background-image: url(G), url(B); background-repeat: no-repeat, repeat; background-position: center, 0 0;
                  background-size: 2px 2px, auto }
            .sh { background: url(G) no-repeat center / 2px 2px, url(B) repeat }
            .one { background: url(G) }
            </style></head><body style="margin:0">
            <div id="lh" class="lh"></div>
            <div id="sh" class="sh"></div>
            <div id="one" class="one"></div>
        </body></html>"#
            .replace("url(G)", "url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgOMHwHwADXAHInah2ygAAAABJRU5ErkJggg==)")
            .replace("url(B)", "url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPj/HwADAgH/5ncLrgAAAABJRU5ErkJggg==)");
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", &html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): the top layer `no-repeat` at `center / 2px 2px` paints its green ONCE at
    /// the box centre and the repeating bottom layer tiles blue everywhere else, for the longhand lists and the
    /// shorthand alike (before: the repeat list was dropped / the last repeat word won, so green tiled the whole box and
    /// hid the blue); the one-layer repeating twin paints green across the box (armed control).
    #[test]
    fn background_layer_props_top_once_bottom_tiles() {
        let mut e = background_layer_props_engine();
        assert!(css_wide_color_ink(&mut e, "one", css_wide_green) > 1000, "a repeating G tiles the whole box (armed)");
        for id in ["lh", "sh"] {
            let green = css_wide_color_ink(&mut e, id, css_wide_green);
            assert!((1..=9).contains(&green), "{id}: the top layer paints once, about 2x2 px, got {green} green px");
            assert!(css_wide_color_ink(&mut e, id, css_wide_blue) > 1000, "{id}: the bottom layer tiles the box");
            let (l, t, w, h) = imgalt_rect(&mut e, id);
            let (cx, cy) = ((l + w / 2.0) as usize, (t + h / 2.0) as usize);
            assert!(css_wide_green(imgalt_px(&e, cx, cy)), "{id}: the centre pixel is the top layer's green");
            assert!(css_wide_blue(imgalt_px(&e, l as usize + 2, t as usize + 2)), "{id}: a corner pixel is the bottom layer's blue");
        }
    }


    // ---- AETHERTEXTALIGNJUSTIFY (LEDGER SR1057): css-text-3 §7.1 text-align: justify (named tail block) ----

    /// AETHERTEXTALIGNJUSTIFY (SR1057): getComputedStyle reads `justify` back (its own code, not start); the start twin
    /// reads `start`.
    #[test]
    fn text_align_justify_reads_back() {
        let html = r#"<!doctype html><html><body style="margin:0">
            <p id="j" style="text-align: justify">a b</p><p id="s" style="text-align: start">a b</p>
            <div style="text-align: justify"><p id="i">a b</p></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(font_shorthand_cs(&mut e, "j", "textAlign"), "justify", "text-align: justify reads back");
        assert_eq!(font_shorthand_cs(&mut e, "s", "textAlign"), "start", "the start twin");
        assert_eq!(font_shorthand_cs(&mut e, "i", "textAlign"), "justify", "justify is inherited");
    }

    /// AETHERTEXTALIGNJUSTIFY (SR1057): a justified monospace paragraph over three lines: every line but the last ends
    /// flush at the 100px box (± 0.5), the last is start (ragged); the start twin's first line is ragged.
    #[test]
    fn text_align_justify_monospace_flush() {
        use crate::layout::inline::Frag;
        if crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::MONO, 400, false)).is_none() {
            return;
        }
        let css = "body{margin:0} p{margin:0;font:10px monospace;line-height:2;width:100px}";
        let edges = |ta: &str| -> Vec<f32> {
            let t = laid_out(&format!(r#"<html><body><p id=p style="text-align:{ta}">ab cd ef gh ij kl mn op qr st uv wx</p></body></html>"#), css);
            let il = &t.inline[&node_by_id(&t, "p").unwrap()];
            il.lines
                .iter()
                .map(|l| il.frags.iter().filter_map(|f| match f { Frag::Text { x, width, baseline, .. } if *baseline == l.2 => Some(x + width), _ => None }).fold(0.0f32, f32::max))
                .collect()
        };
        let j = edges("justify");
        assert!(j.len() >= 2, "the paragraph wraps: {j:?}");
        for e in &j[..j.len() - 1] {
            assert!((e - 100.0).abs() <= 0.5, "a justified line ends flush at 100: {j:?}");
        }
        assert!(*j.last().unwrap() < 99.0, "the last line is start: {j:?}");
        let s = edges("start");
        assert!(s[0] < 99.5, "the start twin is ragged: {s:?}");
    }

    // ---- AETHERTEXTALIGNLAST (LEDGER SR1062): css-text-3 §7.2 text-align-last (named tail block) ----

    /// AETHERTEXTALIGNLAST (SR1062): getComputedStyle reads text-align-last back: `center` on the element, `center`
    /// inherited by a child that does not declare it, `justify` on a justify element.
    #[test]
    fn text_align_last_reads_back() {
        let html = r#"<!doctype html><html><body style="margin:0">
            <p id="c" style="text-align: justify; text-align-last: center">a b</p>
            <div style="text-align-last: center"><p id="i">a b</p></div>
            <p id="j" style="text-align-last: justify">a b</p>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(font_shorthand_cs(&mut e, "c", "textAlignLast"), "center", "text-align-last: center reads back");
        assert_eq!(font_shorthand_cs(&mut e, "i", "textAlignLast"), "center", "text-align-last is inherited");
        assert_eq!(font_shorthand_cs(&mut e, "j", "textAlignLast"), "justify", "justify reads back");
    }

    // ---- AETHERCSSBORDERCOLORSTORED (LEDGER SR1058): a pending border colour paints only where a side exists (named tail block) ----

    /// AETHERCSSBORDERCOLORSTORED (SR1058): one 40x20 box per row, rows 12px apart, under a green parent: `#ca`
    /// `border-color: red` alone; `#cb` `border: 2px solid blue; border-color: red`; `#cc` the sheet's `.p { border: 2px
    /// solid blue } .p.q { border-top-color: red }`; `#cd` `.p.r { border-color: red }` over `.p`; `#ce` `border-color: red;
    /// border-width: 2px`; `#cf` a button with `border-color: red`; `#cg` a plain button (control).
    fn border_color_stored_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            .p { border: 2px solid blue }
            .p.q { border-top-color: red }
            .p.r { border-color: red }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="ca" style="width:40px;height:20px;border-color:red"></div>
            <div id="cb" style="width:40px;height:20px;border:2px solid blue;border-color:red"></div>
            <div id="cc" class="p q" style="width:40px;height:20px"></div>
            <div id="cd" class="p r" style="width:40px;height:20px"></div>
            <div id="ce" style="width:40px;height:20px;border-color:red;border-width:2px"></div>
            <div><button id="cf" style="width:60px;height:24px;border-color:red"></button></div>
            <div><button id="cg" style="width:60px;height:24px"></button></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERCOLORSTORED (SR1058): `border-color: red` alone paints no red (style none, as Chromium; before SR1058
    /// a 1px red frame); armed: `#cb` paints red and `#ce` (`border-width` creates the sides from the pending red) too.
    /// SR1069 (flipped in place): `#ce` declares no style, so it paints nothing (Chromium; `border-width` stores only).
    #[test]
    fn border_color_stored_alone_paints_nothing() {
        let mut e = border_color_stored_engine();
        assert_eq!(css_wide_color_ink(&mut e, "ca", css_wide_red), 0, "#ca paints no red");
        assert!(css_wide_color_ink(&mut e, "cb", css_wide_red) > 40, "armed: #cb paints a red frame");
        assert_eq!(css_wide_color_ink(&mut e, "ce", css_wide_red), 0, "SR1069: #ce has no style, no red (was a 2px red frame)");
    }

    /// AETHERCSSBORDERCOLORSTORED (SR1058): the merge road: a later rule's `border-top-color` reaches the earlier rule's top
    /// (before: dropped, the top stayed blue), the other edges stay blue; a later `border-color` recolours all four.
    #[test]
    fn border_color_stored_merge_paints() {
        let mut e = border_color_stored_engine();
        let edges = border_side_color_edges(&mut e, "cc");
        assert!(css_wide_red(edges[0]), "#cc: the top is red: {:?}", edges[0]);
        for (i, c) in edges.iter().enumerate().skip(1) {
            assert!(css_wide_blue(*c), "#cc: side {i} stays blue: {c:?}");
        }
        for (i, c) in border_side_color_edges(&mut e, "cd").iter().enumerate() {
            assert!(css_wide_red(*c), "#cd: side {i} is red: {c:?}");
        }
    }

    /// AETHERCSSBORDERCOLORSTORED (SR1058): a control's theme frame takes the pending colour (`pending_frame`): `#cf`
    /// paints red; armed: the plain `#cg` paints none.
    #[test]
    fn border_color_stored_button_frame() {
        let mut e = border_color_stored_engine();
        assert!(css_wide_color_ink(&mut e, "cf", css_wide_red) > 20, "#cf: a red frame");
        assert_eq!(css_wide_color_ink(&mut e, "cg", css_wide_red), 0, "armed: #cg paints no red");
    }

    // ---- AETHERCSSOMFONTFAMILYQUOTEDGENERIC (LEDGER SR1107): a quoted generic name reads back quoted (named tail block) ----

    /// AETHERCSSOMFONTFAMILYQUOTEDGENERIC (SR1107): `font-family: 'serif'` is a family NAMED serif (css-fonts-4 §2.1):
    /// getComputedStyle reads `"serif"` (it read bare `serif`, which re-parses as the generic); the generic twin reads
    /// `serif`; el.style already quoted it (cssom's token serializer) and stays pinned.
    #[test]
    fn cssom_font_family_quoted_generic() {
        let html = r#"<!doctype html><html><body style="margin:0">
            <div id="q" style="font-family: 'serif'">Hello</div>
            <div id="g" style="font-family: serif">Hello</div>
            <div id="m" style="font-family: 'serif', serif">Hello</div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 400));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        assert_eq!(font_shorthand_cs(&mut e, "q", "fontFamily"), r#""serif""#, "a quoted generic is a name");
        assert_eq!(font_shorthand_cs(&mut e, "g", "fontFamily"), "serif", "the generic twin");
        assert_eq!(font_shorthand_cs(&mut e, "m", "fontFamily"), r#""serif", serif"#, "the name, then the generic");
        assert_eq!(font_synth_eval(&mut e, "document.getElementById('q').style.fontFamily"), r#""serif""#, "el.style (kept)");
    }

    // ---- AETHERCSSBORDERSIDESTYLEWIDTH (LEDGER SR1069): a style alone paints a medium frame; a width alone paints nothing (named tail block) ----

    /// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): under a green body colour, `#sa` a 40x20 box with `border-style: solid`
    /// alone, `#sb` the armed twin with `border-width: 3px` alone, and `#pp` an open `dialog` popover (displayed by the UA
    /// sheet's `:not(dialog[open])`) whose border is the UA `[popover] { border: solid }`.
    fn border_side_style_width_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="sa" style="width:40px;height:20px;border-style:solid"></div>
            <div id="sb" style="width:40px;height:20px;border-width:3px"></div>
            <dialog popover open id="pp" style="width:40px;height:20px;padding:0"></dialog>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): `#sa` (style alone) is a 46x26 border box with a 3px frame in the ink colour
    /// (rows 0-2 at mid-x green, row 3 not; before SR1069 no frame and 40x20); armed: `#sb` (width alone) is 40x20 and
    /// paints no green (before SR1069 a 3px frame).
    #[test]
    fn border_side_style_width_paints_medium() {
        let mut e = border_side_style_width_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "sa");
        assert_eq!((w, h), (46.0, 26.0), "#sa: the 3px sides size the box");
        let x = (l + w / 2.0) as usize;
        for dy in 0..3 {
            let c = imgalt_px(&mut e, x, t as usize + dy);
            assert!(css_wide_green(c), "#sa: row {dy} is the ink: {c:?}");
        }
        let c = imgalt_px(&mut e, x, t as usize + 3);
        assert!(!css_wide_green(c), "#sa: row 3 is inside the frame: {c:?}");
        let (_, _, w, h) = imgalt_rect(&mut e, "sb");
        assert_eq!((w, h), (40.0, 20.0), "#sb: a width alone makes no border box");
        assert_eq!(css_wide_color_ink(&mut e, "sb", css_wide_green), 0, "#sb: paints nothing");
    }

    /// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): the UA `[popover] { border: solid }` is Chromium's medium 3px (the shorthand
    /// without a width part; before SR1069 1px): the 40x20 popover's border box is 46x26.
    #[test]
    fn border_side_style_width_popover_medium() {
        let mut e = border_side_style_width_engine();
        let (_, _, w, h) = imgalt_rect(&mut e, "pp");
        assert_eq!((w, h), (46.0, 26.0), "#pp: the popover's medium border");
    }

    // ---- AETHERCSSOMLINEHEIGHTNORMALPX (LEDGER SR1063): the used line-height px, `normal` included (named tail block) ----

    /// AETHERCSSOMLINEHEIGHTNORMALPX (SR1063): Chromium pin: `getComputedStyle(el).lineHeight` for `line-height: normal`
    /// is the keyword `normal` (CSSOM resolved values: line-height resolves to the used value only when it is not
    /// `normal`); the px road `css::line_height_used_px` does not change the string.
    #[test]
    fn line_height_normal_px_cssom_pin() {
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "nrm", "lineHeight"), "normal", "Chromium keeps the keyword");
    }

    /// AETHERCSSOMLINEHEIGHTNORMALPX (SR1063): `normal` (mult 0) is the serif face's ascent + descent + line gap at the used
    /// size, at 16px and at the control font's 40/3 px (no face on the host: the size, HostFonts' fallback).
    #[test]
    fn line_height_normal_px_metrics() {
        let sel = layout_core::FontSel { family: crate::fonts::SERIF, ..Default::default() };
        let face = crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::SERIF, 400, false));
        for size in [16.0f32, layout_core::CONTROL_FONT_PX] {
            let want = face.map(|f| {
                let (a, d, g) = crate::fonts::line_metrics(f, size);
                a + d + g
            }).unwrap_or(size);
            let got = crate::css::line_height_used_px(&crate::layout::PaintStyle::default(), 0.0, size, sel);
            assert_eq!(got, want, "normal at {size}px is the metrics sum");
            assert!(got > 0.0, "normal at {size}px is a positive px");
        }
    }

    /// AETHERCSSOMLINEHEIGHTNORMALPX (SR1063): the non-`normal` roads: `1.5` at 16px is 24, a 20px length (mult -20) is 20,
    /// the used font size (20px) wins over the specified 16px (1.5 x 20 = 30); and `line_height_used` routes through the
    /// px fn: the report's string for `num` (1.5 under 20px) and `len` (24px) is the px fn's value.
    #[test]
    fn line_height_used_px_roads() {
        let sel = layout_core::FontSel { family: crate::fonts::SERIF, ..Default::default() };
        let plain = crate::layout::PaintStyle::default();
        assert_eq!(crate::css::line_height_used_px(&plain, 1.5, 16.0, sel), 24.0, "1.5 x 16px");
        assert_eq!(crate::css::line_height_used_px(&plain, -20.0, 16.0, sel), 20.0, "a 20px length");
        let used = crate::layout::PaintStyle { used_font_size: Some(20.0), ..Default::default() };
        assert_eq!(crate::css::line_height_used_px(&used, 1.5, 16.0, sel), 30.0, "1.5 x the used 20px");
        let mut e = line_height_used_engine();
        assert_eq!(font_shorthand_cs(&mut e, "num", "lineHeight"), format!("{}px", crate::css::line_height_used_px(&used, 1.5, 16.0, sel)));
        assert_eq!(font_shorthand_cs(&mut e, "len", "lineHeight"), format!("{}px", crate::css::line_height_used_px(&plain, -24.0, 16.0, sel)));
    }

    // ---- AETHERCSSALIGNCONTENTSAFE (LEDGER SR1064): safe / unsafe / first|last baseline on the alignment arms (named tail block) ----

    /// AETHERCSSALIGNCONTENTSAFE (SR1064): two 200 x 20 flex rows, one 50 x 20 child each, the rules in a sheet:
    /// `justify-content: safe center` centres its child like the plain `center` twin (x 75; before, the declaration was
    /// dropped and the child sat at 0), and the `safe` it could not honour is on the CSS ledger (`align-overflow:safe`).
    #[test]
    fn align_safe_center_engine() {
        crate::ledger::reset();
        let html = r#"<!doctype html><html><head><style>
            .f { display: flex; width: 200px; height: 20px }
            .c { justify-content: center }
            .s { justify-content: safe center }
            </style></head><body style="margin:0">
            <div id="c" class="f c"><div id="ck" style="width:50px;height:20px"></div></div>
            <div id="s" class="f s"><div id="sk" style="width:50px;height:20px"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 400));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let left = |e: &mut crate::AetherEngine, id: &str, c: &str| imgalt_rect(e, id).0 - imgalt_rect(e, c).0;
        let (c, s) = (left(&mut e, "ck", "c"), left(&mut e, "sk", "s"));
        assert!((c - 75.0).abs() <= 0.5, "armed: center puts the child at 75: {c}");
        assert!((s - c).abs() <= 0.5, "safe center centres like center: {s} vs {c}");
        let snap = crate::ledger::snapshot();
        assert!(snap.contains(crate::ledger::ApiCategory::Css, "align-overflow:safe"), "the safe modifier is recorded");
    }

    // ---- AETHERMARGINAUTOOVERFLOW (LEDGER SR1065): an over-constrained auto margin is 0, the box overflows to the end side (named tail block) ----

    /// AETHERMARGINAUTOOVERFLOW (SR1065): CSS 2.2 §10.3.3 through the host engine: a 600px `margin: 0 auto` block in a
    /// 400px parent sits at x 0 and is 600 wide (taffy gave it x -100 before); in an rtl parent its right edge stays at
    /// 400 (x -200: it overflows to the left); a 200px twin still centres at x 100.
    #[test]
    fn test_margin_auto_overflow_kat() {
        let t = laid_out(
            r#"<html><body><div style="width:400px"><div id=a style="width:600px;margin:0 auto">a</div><div id=b style="width:200px;margin:0 auto">b</div></div><div style="width:400px;direction:rtl"><div id=c style="width:600px;margin:0 auto">c</div></div></body></html>"#,
            "body{margin:0}",
        );
        let (a, b, c) = (rect_by_id(&t, "a").unwrap(), rect_by_id(&t, "b").unwrap(), rect_by_id(&t, "c").unwrap());
        assert_eq!((a.0, a.2), (0.0, 600.0), "ltr over-constrained: the auto margins are 0 {a:?}");
        assert_eq!((b.0, b.2), (100.0, 200.0), "a box that fits still centres {b:?}");
        assert_eq!((c.0, c.2), (-200.0, 600.0), "rtl over-constrained: the right edge at 400 {c:?}");
    }

    // ---- AETHERTABLEALIGN (LEDGER SR1071): a table's align= is auto margins / float, never text-align (named tail block) ----

    /// AETHERTABLEALIGN (SR1071): `<table align=center>` centres the table BOX (HTML rendering §15.3.11: the two auto
    /// margins) and leaves its cells' text alone: getBoundingClientRect left 100 in a 400px body (it was 0) and the
    /// cell's computed text-align `start` (it was `-webkit-center`).
    #[test]
    fn table_align_center_box_not_text() {
        let html = r#"<!doctype html><html><body style="margin: 0"><table id="t" align="center" style="width: 200px; border-spacing: 0"><tr><td id="c">cell</td></tr></table></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let left = js_num(&mut engine, "document.getElementById('t').getBoundingClientRect().left");
        let width = js_num(&mut engine, "document.getElementById('t').getBoundingClientRect().width");
        assert_eq!((left, width), (100.0, 200.0), "align=center: the 200px table box is centred in the 400px body");
        let ta = engine.js_engine.as_mut().unwrap().eval_string("getComputedStyle(document.getElementById('c')).textAlign").unwrap();
        assert_eq!(ta, "start", "align=center sets no text-align on the table's cells");
    }

    // ---- AETHERFONTSGENERICRESOLVENAMEDSERIF (LEDGER SR1120): a family NAMED serif falls through to the next family (named tail block) ----

    /// AETHERFONTSGENERICRESOLVENAMEDSERIF (SR1120): `font-family: 'serif', monospace` paints at the monospace advance (no
    /// installed family is named serif; it painted with fontconfig's serif alias), and getComputedStyle reads the list kept.
    #[test]
    fn named_serif_paints_monospace_engine() {
        if crate::fonts::db::db().faces.is_empty() {
            return;
        }
        let html = r#"<!doctype html><html><body style="margin:0;font-size:20px">
            <div><span id="n" style="font-family: 'serif', monospace">iiiiiiiiii</span></div>
            <div><span id="m" style="font-family: monospace">iiiiiiiiii</span></div>
            <div><span id="s" style="font-family: serif">iiiiiiiiii</span></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 400));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let (n, m, s) = (imgalt_rect(&mut e, "n").2, imgalt_rect(&mut e, "m").2, imgalt_rect(&mut e, "s").2);
        assert_eq!(font_shorthand_cs(&mut e, "n", "fontFamily"), r#""serif", monospace"#, "the list reads back kept");
        if (m - s).abs() <= 0.5 {
            return; // the host's monospace and serif faces share an advance: nothing to tell apart
        }
        assert!((n - m).abs() <= 0.5, "armed: the named serif paints at the monospace advance {m}, not the serif {s}: {n}");
    }

    // ---- AETHERCSSWIDECOLUMNGAPNORMALFLEX (LEDGER SR1077): `normal` on the flex / grid gaps writes an explicit zero (named tail block) ----

    /// AETHERCSSWIDECOLUMNGAPNORMALFLEX (SR1077): two flex rows of two 50px items under `column-gap: 10px`; the second
    /// row's later rule `column-gap: normal` is the flex gap's 0 (css-align-3 §8.1), so its second item sits at x 50,
    /// not 60. The first row (no normal) keeps the 10px gap (armed).
    #[test]
    fn gap_normal_flex_column_gap_engine() {
        let html = r#"<!doctype html><html><head><style>
            .r { display: flex; column-gap: 10px; width: 300px; height: 20px }
            .r.t { column-gap: normal }
            .r div { width: 50px; height: 20px; flex: none }
            </style></head><body style="margin:0">
            <div id="g" class="r"><div id="g1"></div><div id="g2"></div></div>
            <div id="t" class="r t"><div id="t1"></div><div id="t2"></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        let left = |e: &mut crate::AetherEngine, id: &str, c: &str| imgalt_rect(e, id).0 - imgalt_rect(e, c).0;
        assert!((left(&mut e, "g2", "g") - 60.0).abs() <= 0.5, "column-gap: 10px: x 60 (armed): {}", left(&mut e, "g2", "g"));
        assert!((left(&mut e, "t2", "t") - 50.0).abs() <= 0.5, "a later column-gap: normal is 0: x 50: {}", left(&mut e, "t2", "t"));
    }

    // ---- AETHERCSSGAPSHORTHANDMULTICOL (LEDGER SR1079): the gap shorthand writes the multicol gutter (named tail block) ----

    /// AETHERCSSGAPSHORTHANDMULTICOL (SR1079): SR1016's shape, a two-column 410px div at 20px with red blocks: alone
    /// the gutter is 1em (195..215) so x 220 is column two's red; `gap: 40px`, `grid-gap: 40px` and `gap: 0px 40px`
    /// each set the 40px gutter (185..225), so x 220 is gutter. Before, the shorthand left the 1em gutter.
    #[test]
    fn gap_shorthand_multicol_engine() {
        let html = r#"<!doctype html><html><head><style>
            .mc { width: 410px; column-count: 2; font-size: 20px; margin: 0 0 20px }
            .mc div { height: 20px; background-color: rgb(255, 0, 0) }
            .mc.g { gap: 40px }
            .mc.gg { grid-gap: 40px }
            .mc.g2 { gap: 0px 40px }
            </style></head><body style="margin:0">
            <div class="mc"><div></div><div></div></div>
            <div class="mc g"><div></div><div></div></div>
            <div class="mc gg"><div></div><div></div></div>
            <div class="mc g2"><div></div><div></div></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(500, 200));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        for y in [10, 50, 90, 130] {
            assert_eq!(imgalt_px(&e, 100, y), (255, 0, 0), "y {y}: the first column paints red");
        }
        assert_eq!(imgalt_px(&e, 220, 10), (255, 0, 0), "no gap: 1em gutter, x 220 is column two (armed)");
        assert_ne!(imgalt_px(&e, 220, 50), (255, 0, 0), "gap: 40px: x 220 is gutter");
        assert_ne!(imgalt_px(&e, 220, 90), (255, 0, 0), "grid-gap: 40px: x 220 is gutter");
        assert_ne!(imgalt_px(&e, 220, 130), (255, 0, 0), "gap: 0px 40px: the column token, x 220 is gutter");
    }

    // ---- AETHERCSSBORDERTHINTHICK (LEDGER SR1075): `thin` / `thick` paint 1px / 5px frames; border-width takes 1-4 values (named tail block) ----

    /// AETHERCSSBORDERTHINTHICK (SR1075): 40x20 boxes in green ink, rows 12px apart: `#tn` `border: thin solid`, `#tk`
    /// `border: thick solid`, `#tz` `border-width: 1px 0; border-style: solid`.
    fn border_thin_thick_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="tn" style="width:40px;height:20px;border:thin solid"></div>
            <div id="tk" style="width:40px;height:20px;border:thick solid"></div>
            <div id="tz" style="width:40px;height:20px;border-width:1px 0;border-style:solid"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERTHINTHICK (SR1075): `#tn` is a 42x22 border box whose row 0 (mid-x) is the ink and row 1 not; `#tk`
    /// is 50x30 with rows 0-4 inked and row 5 not (before SR1075 both were medium 3px, 46x26); `#tz` is 40x22 (top / bottom
    /// 1px, left / right 0; before SR1075 the two-value declaration was dropped). getComputedStyle reads 1px / 5px.
    #[test]
    fn border_thin_thick_paints() {
        let mut e = border_thin_thick_engine();
        for (id, bw, size) in [("tn", 1usize, (42.0, 22.0)), ("tk", 5, (50.0, 30.0))] {
            let (l, t, w, h) = imgalt_rect(&mut e, id);
            assert_eq!((w, h), size, "#{id}: the border box");
            let x = (l + w / 2.0) as usize;
            for dy in 0..bw {
                let c = imgalt_px(&mut e, x, t as usize + dy);
                assert!(css_wide_green(c), "#{id}: row {dy} is the ink: {c:?}");
            }
            let c = imgalt_px(&mut e, x, t as usize + bw);
            assert!(!css_wide_green(c), "#{id}: row {bw} is inside the frame: {c:?}");
            assert_eq!(font_shorthand_cs(&mut e, id, "borderTopWidth"), format!("{bw}px"), "#{id}: getComputedStyle");
        }
        let (_, _, w, h) = imgalt_rect(&mut e, "tz");
        assert_eq!((w, h), (40.0, 22.0), "#tz: border-width: 1px 0");
    }

    // ---- AETHERCSSBORDERCOLORSIDES (LEDGER SR1085): border-color takes 1-4 colours through the per-side road (named tail block) ----

    /// AETHERCSSBORDERCOLORSIDES (SR1085): under a green body ink, `#bs` a 40x20 box with `border: 2px solid;
    /// border-color: red blue` (top / bottom red, left / right blue; before SR1085 the value was dropped and the frame
    /// took the ink).
    fn border_color_sides_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { margin: 0 0 12px }
            </style></head><body style="margin:0;font:20px serif;color:rgb(0, 160, 0)">
            <div id="bs" style="width:40px;height:20px;border:2px solid;border-color:red blue"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSBORDERCOLORSIDES (SR1085): `border-color: red blue` paints the top and bottom edges red and the left and
    /// right edges blue; getComputedStyle reads borderLeftColor rgb(0, 0, 255) (border-top-color is no report column:
    /// AETHER-CSSOM-BORDER-SIDE-COLOR-COLUMNS owed).
    #[test]
    fn border_color_sides_paints_two_colours() {
        let mut e = border_color_sides_engine();
        let edges = border_side_color_edges(&mut e, "bs");
        for (i, c) in edges.iter().enumerate() {
            if i % 2 == 0 {
                assert!(css_wide_red(*c), "#bs: side {i} is red: {c:?}");
            } else {
                assert!(css_wide_blue(*c), "#bs: side {i} is blue: {c:?}");
            }
        }
        assert_eq!(font_shorthand_cs(&mut e, "bs", "borderLeftColor"), "rgb(0, 0, 255)");
    }

    // ---- AETHERCSSSTICKYEMINSETRELATIVE (LEDGER SR1130): a sticky box's font-relative inset stays the sticky constraint on the re-resolution (named tail block) ----

    /// AETHERCSSSTICKYEMINSETRELATIVE (SR1130): 400x300, 20px-tall boxes in flow from y 0: `#c` a control, `#a` sticky
    /// `top: 1em`, `#b` sticky `top: 16px`, a 20px-font wrapper holding `#r` relative `top: 1em`, `#d` a red 20px-font
    /// sticky `top: 1em`, then a 2000px spacer.
    fn sticky_em_inset_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            .h { height: 20px }
            </style></head><body style="margin:0">
            <div id="c" class="h"></div>
            <div id="a" class="h" style="position:sticky;top:1em"></div>
            <div id="b" class="h" style="position:sticky;top:16px"></div>
            <div style="font-size:20px"><div id="r" class="h" style="position:relative;top:1em"></div></div>
            <div id="d" class="h" style="font-size:20px;position:sticky;top:1em;background-color:rgb(255, 0, 0)"></div>
            <div style="height:2000px"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSSTICKYEMINSETRELATIVE (SR1130): unscrolled, each sticky box sits at its normal-flow y (css-position-3
    /// §3.4): `#a` at 20 like the px-inset `#b` at 40 and the control `#c` at 0; `#d` at 80. Before SR1130 the
    /// font-relative re-resolution folded `top: 1em` into taffy as a relative offset (`#a` 36, `#d` 100).
    #[test]
    fn sticky_em_inset_normal_flow() {
        let mut e = sticky_em_inset_engine();
        let top = |e: &mut crate::AetherEngine, id: &str| imgalt_rect(e, id).1;
        assert_eq!(top(&mut e, "c"), 0.0, "#c: the control");
        assert_eq!(top(&mut e, "a"), 20.0, "#a: sticky top: 1em sits at its normal-flow y (armed: 36 before SR1130)");
        assert_eq!(top(&mut e, "b"), 40.0, "#b: sticky top: 16px sits at its normal-flow y");
        assert_eq!(top(&mut e, "d"), 80.0, "#d: sticky top: 1em at 20px sits at its normal-flow y (armed: 100 before SR1130)");
    }

    /// AETHERCSSSTICKYEMINSETRELATIVE (SR1130): a relative box's font-relative inset still offsets it, at its own
    /// font: `#r` normal y 60 + 1em at 20px = 80 (the re-resolution's fold for a kind that is not sticky, unchanged).
    #[test]
    fn sticky_em_inset_relative_unchanged() {
        let mut e = sticky_em_inset_engine();
        assert_eq!(imgalt_rect(&mut e, "r").1, 80.0, "#r: relative top: 1em at 20px offsets 20");
    }

    /// AETHERCSSSTICKYEMINSETRELATIVE (SR1130): scrolled 400px, the red `#d` (document y 80) holds at its inset, 1em of
    /// its own 20px font: rows 20..39 red, rows 19 and 40 not (`inset_px` is re-resolved inside the re-application). The
    /// scroll is a full viewport or more so the frame repaints whole: a shorter scroll blits the surface and repaints
    /// only the exposed strip, leaving a stuck box unpainted (owed AETHER-CSS-STICKY-SCROLL-BLIT-DAMAGE).
    #[test]
    fn sticky_em_inset_sticks_at_own_font() {
        let mut e = sticky_em_inset_engine();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 400.0));
        e.render_frame();
        assert_eq!(e.scroll_y, 400.0, "the page scrolled 400px");
        assert_eq!(imgalt_px(&e, 10, 20), (255, 0, 0), "#d: row 20 is its first row (stuck at 1em = 20px)");
        assert_eq!(imgalt_px(&e, 10, 39), (255, 0, 0), "#d: row 39 is its last row");
        assert_ne!(imgalt_px(&e, 10, 19), (255, 0, 0), "row 19 is above the stuck box");
        assert_ne!(imgalt_px(&e, 10, 40), (255, 0, 0), "row 40 is below the stuck box");
    }

    // ---- AETHERCSSBORDERPENDINGTRANSPARENT (LEDGER SR1112): a transparent border colour keeps the side at alpha 0 (named tail block) ----

    /// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): one 40x20 box per row under a green parent: `#ta` `border: 2px solid
    /// transparent`, `#tb` its borderless twin, `#td` `border: 2px solid red; border-color: transparent`, and `#th` the
    /// reserve-space idiom: a transparent frame that `:hover` colours red.
    fn border_transparent_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div div { width: 40px; height: 20px; margin: 0 0 12px }
            .f { border: 2px solid; border-color: transparent }
            .f:hover { border-color: red }
            </style></head><body style="margin:0">
            <div style="background:rgb(0, 160, 0);padding:8px">
            <div id="ta" style="border:2px solid transparent"></div>
            <div id="tb"></div>
            <div id="td" style="border:2px solid red;border-color:transparent"></div>
            <div id="th" class="f"></div>
            </div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 400));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    fn border_transparent_not_green(c: (u8, u8, u8)) -> bool {
        !css_wide_green(c)
    }

    /// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `border: 2px solid transparent` reserves its 2px (a 44x24 border box)
    /// and paints nothing over the green parent (before SR1112 a grey 2px frame: the shorthand read no colour); the
    /// borderless twin is 40x20.
    #[test]
    fn border_transparent_shorthand_reserves_and_paints_nothing() {
        let mut e = border_transparent_engine();
        let (_, _, w, h) = imgalt_rect(&mut e, "ta");
        assert_eq!((w, h), (44.0, 24.0), "#ta: the transparent 2px sides size the box");
        assert_eq!(css_wide_color_ink(&mut e, "ta", border_transparent_not_green), 0, "#ta: no ink over the green parent");
        let (_, _, w, h) = imgalt_rect(&mut e, "tb");
        assert_eq!((w, h), (40.0, 20.0), "#tb: the borderless twin");
    }

    /// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `border-color: transparent` over `border: 2px solid red` keeps the
    /// sides (44x24; before SR1112 the sides were dropped and the box was 40x20) and paints no red.
    #[test]
    fn border_transparent_color_keeps_box() {
        let mut e = border_transparent_engine();
        let (_, _, w, h) = imgalt_rect(&mut e, "td");
        assert_eq!((w, h), (44.0, 24.0), "#td: border-color: transparent keeps the 2px sides");
        assert_eq!(css_wide_color_ink(&mut e, "td", css_wide_red), 0, "#td: paints no red");
        assert_eq!(css_wide_color_ink(&mut e, "td", border_transparent_not_green), 0, "#td: paints nothing");
    }

    /// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): the reserve-space idiom: `#th`'s transparent frame is 44x24 before and
    /// under the pointer, so `:hover { border-color: red }` paints red without a layout shift (before SR1112 the frame
    /// was dropped: 40x20 and no red on hover).
    #[test]
    fn border_transparent_hover_no_shift() {
        let mut e = border_transparent_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "th");
        assert_eq!((w, h), (44.0, 24.0), "#th: the transparent frame reserves its space");
        assert_eq!(css_wide_color_ink(&mut e, "th", css_wide_red), 0, "#th: no red before hover");
        e.handle_event(crate::api::events::Event::MouseMove(l + w / 2.0, t + h / 2.0));
        e.render_frame();
        let (_, _, w2, h2) = imgalt_rect(&mut e, "th");
        assert_eq!((w2, h2), (44.0, 24.0), "#th: hover does not shift the box");
        assert!(css_wide_color_ink(&mut e, "th", css_wide_red) > 0, "#th: the hover colour paints the reserved frame");
    }

    // ---- AETHERCSSOUTLINETRANSPARENT (LEDGER SR1119): an alpha-0 outline-color paints nothing and reads its cascaded text (named tail block) ----

    /// AETHERCSSOUTLINETRANSPARENT (SR1119): three 40x20 boxes 12px apart, each outline pulled inside its box by
    /// `outline-offset: -6px` (so the box's own rect holds the band): `#or` `2px solid red` (the reference), `#oa` the same
    /// then `outline-color: rgba(255, 0, 0, 0)`, `#ob` `outline: 2px solid rgba(255, 0, 0, 0)`.
    fn outline_transparent_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            div { width: 40px; height: 20px; margin: 0 0 12px; outline-offset: -6px }
            </style></head><body style="margin:0">
            <div id="or" style="outline:2px solid red"></div>
            <div id="oa" style="outline:2px solid red;outline-color:rgba(255, 0, 0, 0)"></div>
            <div id="ob" style="outline:2px solid rgba(255, 0, 0, 0)"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSOUTLINETRANSPARENT (SR1119): an alpha-0 outline colour paints no ink (focusring's `paint_outlines` skips
    /// alpha 0): `#oa` (red before SR1119: the alpha-0 longhand was dropped and the red stayed) and `#ob` paint no red
    /// while `#or` does; getComputedStyle(#oa).outlineColor reads the cascaded `rgba(255, 0, 0, 0)` (no outline column:
    /// AETHER-CSSOM-OUTLINE-COLUMNS owed).
    #[test]
    fn outline_transparent_paints_nothing() {
        let mut e = outline_transparent_engine();
        assert!(css_wide_color_ink(&mut e, "or", css_wide_red) > 20, "#or: the reference paints its red outline");
        assert_eq!(css_wide_color_ink(&mut e, "oa", css_wide_red), 0, "armed: #oa's alpha-0 outline-color paints no red");
        assert_eq!(css_wide_color_ink(&mut e, "ob", css_wide_red), 0, "#ob: the alpha-0 shorthand paints no red");
        assert_eq!(font_shorthand_cs(&mut e, "oa", "outlineColor"), "rgba(255, 0, 0, 0)", "#oa: cssom");
    }

    // ---- AETHERCSSSTICKYSCROLLBLITDAMAGE (LEDGER SR1135): a short scroll repaints the whole viewport when a box holds its place (named tail block) ----

    /// AETHERCSSSTICKYSCROLLBLITDAMAGE (SR1135): scrolled 200px (shorter than the 300px viewport), the stuck red `#d` holds
    /// at 1em = 20px: rows 20..39 red, rows 19 and 40 not. Before SR1135 the blit carried `#d`'s old rows 80..99 off-screen
    /// and repainted only the exposed strip 100..300, so row 20 stayed white.
    #[test]
    fn sticky_scroll_blit_short_scroll_paints_stuck_box() {
        let mut e = sticky_em_inset_engine();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 200.0));
        e.render_frame();
        assert_eq!(e.scroll_y, 200.0, "the page scrolled 200px");
        assert_eq!(imgalt_px(&e, 10, 20), (255, 0, 0), "#d: row 20 is its first row (armed: white before SR1135)");
        assert_eq!(imgalt_px(&e, 10, 39), (255, 0, 0), "#d: row 39 is its last row");
        assert_ne!(imgalt_px(&e, 10, 19), (255, 0, 0), "row 19 is above the stuck box");
        assert_ne!(imgalt_px(&e, 10, 40), (255, 0, 0), "row 40 is below the stuck box");
    }

    /// AETHERCSSSTICKYSCROLLBLITDAMAGE (SR1135): after a full repaint at 400px, a +50 scroll keeps `#d` at rows 20..39
    /// (before SR1135 the blit moved its rows off the top and left old document rows there), and a -50 scroll keeps it
    /// there with no ghost at rows 70..89 (before SR1135 the blit shifted the stuck rows down by 50).
    #[test]
    fn sticky_scroll_blit_follow_on_scrolls() {
        let mut e = sticky_em_inset_engine();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 400.0));
        e.render_frame();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 50.0));
        e.render_frame();
        assert_eq!(e.scroll_y, 450.0, "the page scrolled to 450px");
        assert_eq!(imgalt_px(&e, 10, 20), (255, 0, 0), "+50: row 20 is still #d's first row (armed)");
        assert_eq!(imgalt_px(&e, 10, 39), (255, 0, 0), "+50: row 39 is still #d's last row");
        e.handle_event(crate::api::events::Event::Scroll(0.0, -50.0));
        e.render_frame();
        assert_eq!(e.scroll_y, 400.0, "the page scrolled back to 400px");
        assert_eq!(imgalt_px(&e, 10, 20), (255, 0, 0), "-50: row 20 is #d's first row");
        assert_eq!(imgalt_px(&e, 10, 39), (255, 0, 0), "-50: row 39 is #d's last row");
        assert_ne!(imgalt_px(&e, 10, 70), (255, 0, 0), "-50: no ghost at row 70 (armed: the blit shifted row 20 here)");
        assert_ne!(imgalt_px(&e, 10, 89), (255, 0, 0), "-50: no ghost at row 89 (armed: the blit shifted row 39 here)");
    }

    /// AETHERCSSSTICKYSCROLLBLITDAMAGE (SR1135): a page with no fixed or sticky box keeps the blit: a 50px scroll damages
    /// only the exposed 50px strip (unchanged); the sticky fixture's 50px scroll damages the whole viewport.
    #[test]
    fn sticky_scroll_blit_plain_page_damage_is_strip() {
        let html = r#"<!doctype html><html><body style="margin:0"><div style="height:2000px"></div></body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 50.0));
        assert_eq!(e.damage_rects, vec![(0, 250, 400, 50)], "plain page: the exposed strip only");
        let mut s = sticky_em_inset_engine();
        s.handle_event(crate::api::events::Event::Scroll(0.0, 50.0));
        assert_eq!(s.damage_rects, vec![(0, 0, 400, 300)], "sticky page: the whole viewport");
    }

    /// AETHERCSSSTICKYSCROLLBLITDAMAGE (SR1135): a `position: fixed` box (top 10px, 20px tall) stays at rows 10..29 under
    /// a 50px scroll; before SR1135 the blit moved its rows to -40..-21 and the strip repaint never revisited 10..29.
    #[test]
    fn sticky_scroll_blit_fixed_box_stays_put() {
        let html = r#"<!doctype html><html><body style="margin:0">
            <div style="position:fixed;top:10px;left:0;width:100px;height:20px;background-color:rgb(255, 0, 0)"></div>
            <div style="height:2000px"></div>
        </body></html>"#;
        let mut e = crate::AetherEngine::new();
        e.handle_event(crate::api::events::Event::Resize(400, 300));
        e.load_html("https://example.com/", html, true);
        e.render_frame();
        e.handle_event(crate::api::events::Event::Scroll(0.0, 50.0));
        e.render_frame();
        assert_eq!(e.scroll_y, 50.0, "the page scrolled 50px");
        assert_eq!(imgalt_px(&e, 10, 10), (255, 0, 0), "row 10 is the fixed box's first row (armed)");
        assert_eq!(imgalt_px(&e, 10, 29), (255, 0, 0), "row 29 is its last row");
        assert_ne!(imgalt_px(&e, 10, 9), (255, 0, 0), "row 9 is above it");
        assert_ne!(imgalt_px(&e, 10, 30), (255, 0, 0), "row 30 is below it");
    }

    // ---- AETHERCSSCOLORALPHAZEROONEROAD (LEDGER SR1126): the text colour's alpha reaches the glyph ink and the cssom (named tail block) ----

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): one 20px line per row on white: `#op` opaque black (the control), `#hf`
    /// `rgba(0, 0, 0, 0.5)` with an inheriting span `#hfs`, `#tr` `transparent`, `#pr` a half-alpha parent of a
    /// `color: red` span `#prc`, `#bd` half-alpha text in a `4px solid currentcolor` frame, `#sh` transparent text with
    /// a red `text-shadow`.
    fn color_alpha_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            body { margin: 0; background: #fff }
            div { font-size: 20px; line-height: 30px; margin: 0 0 10px; width: 300px }
            </style></head><body>
            <div id="op" style="color:#000"><span id="ops">MMMM</span></div>
            <div id="hf" style="color:rgba(0, 0, 0, 0.5)"><span id="hfs">MMMM</span></div>
            <div id="tr" style="color:transparent"><span id="trs">MMMM</span></div>
            <div id="pr" style="color:rgba(0, 0, 0, 0.5)"><span id="prc" style="color:red">MMMM</span></div>
            <div id="bd" style="color:rgba(0, 0, 0, 0.5);border:4px solid currentcolor"><span>MMMM</span></div>
            <div id="sh" style="color:transparent;text-shadow:2px 2px 0 red"><span id="shs">MMMM</span></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): the darkest channel of any pixel in element `id`'s rect, and the count of
    /// pixels in it that are not white.
    fn color_alpha_ink(e: &mut crate::AetherEngine, id: &str) -> (u8, usize) {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (sw, sh) = (e.width as usize, e.height as usize);
        let (x0, x1) = (l.floor().max(0.0) as usize, ((l + w).ceil() as usize).min(sw));
        let (y0, y1) = (t.floor().max(0.0) as usize, ((t + h).ceil() as usize).min(sh));
        let (mut dark, mut n) = (255u8, 0usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let (r, g, b) = imgalt_px(e, x, y);
                dark = dark.min(r.min(g).min(b));
                if (r, g, b) != (255, 255, 255) {
                    n += 1;
                }
            }
        }
        (dark, n)
    }

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): half-alpha text paints at half ink (its darkest pixel ~127 where the opaque
    /// control's is ~0; before SR1126 both were opaque), and transparent text paints no ink while its span keeps the
    /// control's box (the run is measured and laid out; only the glyph pixels are skipped).
    #[test]
    fn color_alpha_paints_half_and_none() {
        let mut e = color_alpha_engine();
        let (op_dark, op_n) = color_alpha_ink(&mut e, "ops");
        assert!(op_n > 20 && op_dark < 30, "the opaque control paints black ink: darkest {op_dark}, {op_n} px");
        let (hf_dark, hf_n) = color_alpha_ink(&mut e, "hfs");
        assert!(hf_n > 20, "the half-alpha text paints ink: {hf_n} px");
        assert!((110..=150).contains(&hf_dark), "armed: the half-alpha text's darkest pixel is half ink: {hf_dark}");
        assert_eq!(color_alpha_ink(&mut e, "trs").1, 0, "armed: transparent text paints no ink");
        let (ow, oh) = (imgalt_rect(&mut e, "ops").2, imgalt_rect(&mut e, "ops").3);
        let (tw, th) = (imgalt_rect(&mut e, "trs").2, imgalt_rect(&mut e, "trs").3);
        assert_eq!((tw, th), (ow, oh), "transparent text keeps its laid-out box");
    }

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): `color: red` under a half-alpha parent is opaque red (the alpha is part of
    /// the colour value, so the child's own colour resets it), and transparent text keeps its text-shadow (the shadow
    /// paints from its own mask in its own colour, as Chromium's blur-text idiom).
    #[test]
    fn color_alpha_child_reset_and_shadow() {
        let mut e = color_alpha_engine();
        assert!(css_wide_color_ink(&mut e, "prc", css_wide_red) > 10, "the red child paints opaque red");
        assert!(css_wide_color_ink(&mut e, "shs", css_wide_red) > 10, "transparent text keeps its red shadow");
    }

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): the `color` column reads the alpha (Chromium's strings): `rgba(0, 0, 0,
    /// 0.5)`, the inheriting span the same, `rgba(0, 0, 0, 0)` for transparent, `rgb(255, 0, 0)` for the reset child
    /// (before SR1126 `rgb(0, 0, 0)` for the first three).
    #[test]
    fn color_alpha_cssom_column() {
        let mut e = color_alpha_engine();
        assert_eq!(font_shorthand_cs(&mut e, "op", "color"), "rgb(0, 0, 0)", "the opaque control");
        assert_eq!(font_shorthand_cs(&mut e, "hf", "color"), "rgba(0, 0, 0, 0.5)", "armed: the half alpha");
        assert_eq!(font_shorthand_cs(&mut e, "hfs", "color"), "rgba(0, 0, 0, 0.5)", "armed: the span inherits the alpha");
        assert_eq!(font_shorthand_cs(&mut e, "tr", "color"), "rgba(0, 0, 0, 0)", "armed: transparent");
        assert_eq!(font_shorthand_cs(&mut e, "prc", "color"), "rgb(255, 0, 0)", "the child's own colour is opaque");
    }

    /// AETHERCSSCOLORALPHAZEROONEROAD (SR1126): a `currentcolor` border under half-alpha text paints at the text's half
    /// alpha at the top edge's mid-x, as Chromium (flipped by AETHERCSSCURRENTCOLORALPHA SR1134: before, the opaque RGB (0, 0, 0)).
    #[test]
    fn color_alpha_currentcolor_border_half() {
        let mut e = color_alpha_engine();
        let (l, t, w, _) = imgalt_rect(&mut e, "bd");
        let c = imgalt_px(&e, (l + w / 2.0) as usize, t as usize + 1);
        assert!([c.0, c.1, c.2].iter().all(|v| (110..=150).contains(v)), "SR1134: the currentcolor frame is half-alpha grey: {c:?}");
    }

    // ---- AETHERCSSCURRENTCOLORALPHA (LEDGER SR1134): currentcolor carries the text colour's alpha to the border and background (named tail block) ----

    /// AETHERCSSCURRENTCOLORALPHA (SR1134): 40x20 empty boxes on white, rows 12px apart: `#hb` half-alpha text in a
    /// `4px solid` (currentcolor) frame, `#tb` transparent text in the same frame, `#rb` half-alpha text in a
    /// `4px solid red` frame, `#cb` half-alpha text over `background-color: currentcolor`.
    fn currentcolor_alpha_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            body { margin: 0; background: #fff }
            div { width: 40px; height: 20px; margin: 0 0 12px }
            </style></head><body>
            <div id="hb" style="color:rgba(0, 0, 0, 0.5);border:4px solid"></div>
            <div id="tb" style="color:transparent;border:4px solid"></div>
            <div id="rb" style="color:rgba(0, 0, 0, 0.5);border:4px solid red"></div>
            <div id="cb" style="color:rgba(0, 0, 0, 0.5);background-color:currentcolor"></div>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCURRENTCOLORALPHA (SR1134): the currentcolor frame under half-alpha text composites at 0.5 at the top edge's
    /// mid-x (~127 grey on white; before SR1134 opaque black; a corner pixel takes the top and left strokes twice:
    /// AETHER-CSS-BORDER-ALPHA-CORNER-OVERLAP owed) and cssom `borderLeftColor` reads `rgba(0, 0, 0, 0.5)` (before: `rgb(0, 0, 0)`);
    /// a declared red frame under the same text stays opaque red.
    #[test]
    fn currentcolor_alpha_border_half() {
        let mut e = currentcolor_alpha_engine();
        let (l, t, w, _) = imgalt_rect(&mut e, "hb");
        let c = imgalt_px(&e, (l + w / 2.0) as usize, t as usize + 1);
        assert!([c.0, c.1, c.2].iter().all(|v| (110..=150).contains(v)), "armed: the currentcolor frame is half-alpha grey: {c:?}");
        assert_eq!(font_shorthand_cs(&mut e, "hb", "borderLeftColor"), "rgba(0, 0, 0, 0.5)", "armed: the column reads the ink alpha");
        let (l, t, _, _) = imgalt_rect(&mut e, "rb");
        assert_eq!(imgalt_px(&e, l as usize + 1, t as usize + 1), (255, 0, 0), "a declared red frame stays opaque");
        assert_eq!(font_shorthand_cs(&mut e, "rb", "borderLeftColor"), "rgb(255, 0, 0)", "the declared colour's column");
    }

    /// AETHERCSSCURRENTCOLORALPHA (SR1134): `color: transparent; border: 4px solid` paints no border (the currentcolor
    /// sides drop at ink alpha 0) while the border still sizes the box: 48x28 (SR1112's width rule).
    #[test]
    fn currentcolor_alpha_transparent_no_border() {
        let mut e = currentcolor_alpha_engine();
        assert_eq!(color_alpha_ink(&mut e, "tb").1, 0, "armed: no border ink");
        let (_, _, w, h) = imgalt_rect(&mut e, "tb");
        assert_eq!((w, h), (48.0, 28.0), "the 4px border still sizes the box");
    }

    /// AETHERCSSCURRENTCOLORALPHA (SR1134): `background-color: currentcolor` under half-alpha text fills at 0.5 (~127 grey;
    /// before SR1134 opaque black) and cssom `backgroundColor` reads `rgba(0, 0, 0, 0.5)`.
    #[test]
    fn currentcolor_alpha_background_half() {
        let mut e = currentcolor_alpha_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "cb");
        let c = imgalt_px(&e, (l + w / 2.0) as usize, (t + h / 2.0) as usize);
        assert!([c.0, c.1, c.2].iter().all(|v| (110..=150).contains(v)), "armed: the currentcolor fill is half-alpha grey: {c:?}");
        assert_eq!(font_shorthand_cs(&mut e, "cb", "backgroundColor"), "rgba(0, 0, 0, 0.5)", "armed: the column reads the ink alpha");
    }

    // ---- AETHERCSSCONTAINERUNITSREAPPLYKIND (LEDGER SR1136): ONE refold tail for both re-resolutions, seeded with the paint fields fold_into reads (named tail block) ----

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): 400x300, from y 0: `#q` a `container-type: size` 200px-tall
    /// container holding a 20px control and `#s` sticky `top: 10cqh`; then three 100px-wide 50px-tall both-edges
    /// gutter scrollers — `#g` `padding-left: 1em`, `#gp` `padding-left: 16px`, `#gn` no padding and `width: 10em`
    /// (each holding a 20px child `#gc` / `#gpc` / `#gnc`); then `#iq` a `container-type: inline-size; width: 10em`
    /// container whose child `#ic` is `width: 100cqw`; then a 2000px spacer.
    fn reapply_kind_engine() -> crate::AetherEngine {
        let html = r#"<!doctype html><html><head><style>
            .sc { overflow: auto; scrollbar-gutter: stable both-edges; height: 50px }
            .k { height: 20px }
            </style></head><body style="margin:0">
            <div id="q" style="container-type:size;height:200px"><div class="k"></div><div id="s" class="k" style="position:sticky;top:10cqh"></div></div>
            <div id="g" class="sc" style="width:100px;padding-left:1em"><div id="gc" class="k"></div></div>
            <div id="gp" class="sc" style="width:100px;padding-left:16px"><div id="gpc" class="k"></div></div>
            <div id="gn" class="sc" style="width:10em"><div id="gnc" class="k"></div></div>
            <div id="iq" style="container-type:inline-size;width:10em"><div id="ic" class="k" style="width:100cqw"></div></div>
            <div style="height:2000px"></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): unscrolled, the sticky `#s` with a container-unit inset sits at its
    /// normal-flow y 20 (css-position-3 §3.4). Before SR1136 `reresolve_container_units` folded `top: 10cqh` (20px of
    /// the 200px container) into taffy as a relative offset (40).
    #[test]
    fn reapply_kind_sticky_cqh_normal_flow() {
        let mut e = reapply_kind_engine();
        assert_eq!(imgalt_rect(&mut e, "q").1, 0.0, "#q: the container at the top");
        assert_eq!(imgalt_rect(&mut e, "s").1, 20.0, "#s: sticky top: 10cqh sits at its normal-flow y (armed: 40 before SR1136)");
    }

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): a both-edges gutter scroller with `padding-left: 1em` keeps its
    /// start-edge gutter on the font-relative re-resolution: content x = 10 (gutter) + 16 (1em). Before SR1136 the
    /// re-applied padding overwrote the first fold's 26 with 16 and the fresh spec had no overflow to re-add the gutter.
    #[test]
    fn reapply_kind_gutter_em_padding() {
        let mut e = reapply_kind_engine();
        let dx = imgalt_rect(&mut e, "gc").0 - imgalt_rect(&mut e, "g").0;
        assert_eq!(dx, 26.0, "#gc: gutter 10 + padding 1em = 26 (armed: 16 before SR1136)");
    }

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): the control — `padding-left: 16px` (no re-resolution of its own
    /// padding) reads the same 26 as the em box.
    #[test]
    fn reapply_kind_gutter_px_padding_control() {
        let mut e = reapply_kind_engine();
        let dx = imgalt_rect(&mut e, "gpc").0 - imgalt_rect(&mut e, "gp").0;
        assert_eq!(dx, 26.0, "#gpc: gutter 10 + padding 16px = 26");
    }

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): a both-edges scroller re-resolved for `width: 10em` with NO
    /// padding-left keeps ONE start gutter (10): the seed skips overflow / scrollbar-gutter there, since the first
    /// fold's gutter is still the node's padding and a seeded fold would add it again (20).
    #[test]
    fn reapply_kind_gutter_no_padding_em_width() {
        let mut e = reapply_kind_engine();
        let dx = imgalt_rect(&mut e, "gnc").0 - imgalt_rect(&mut e, "gn").0;
        assert_eq!(dx, 10.0, "#gnc: one start gutter, never two");
    }

    /// AETHERCSSCONTAINERUNITSREAPPLYKIND (SR1136): a `container-type: inline-size; width: 10em` container still
    /// answers its child's `width: 100cqw` at 160px (container_type is read from the paint map by the query walk,
    /// not seeded into the refold).
    #[test]
    fn reapply_kind_inline_size_cqw() {
        let mut e = reapply_kind_engine();
        assert_eq!(imgalt_rect(&mut e, "iq").2, 160.0, "#iq: 10em");
        assert_eq!(imgalt_rect(&mut e, "ic").2, 160.0, "#ic: 100cqw of the 160px container");
    }

    // ---- AETHERCSSCONTAINSIZEMINHEIGHT (LEDGER SR1140): size containment keeps a declared min-height (named tail block) ----

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): the height of `#want` in a page with body margin 0 laid out by the cascade.
    fn contain_size_min_height_of(body: &str) -> f32 {
        let html = format!(r#"<html><body style="margin:0">{body}</body></html>"#);
        let t = laid_out(&html, "");
        rect_by_id(&t, "want").expect("#want is laid out").3
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): css-contain-2 §3.2 lays a size container out as if empty, and an empty box
    /// still takes its min-height: `container-type: size; min-height: 32px` is 32px tall (before SR1140 the size-containment
    /// fold reset the declared min-height to auto and the box was 0 tall).
    #[test]
    fn contain_size_min_height_px_is_kept() {
        let h = contain_size_min_height_of(r#"<div id="want" style="container-type: size; min-height: 32px"><div style="height: 100px"></div></div>"#);
        assert_eq!(h, 32.0, "a size container takes its px min-height (armed: 0 before SR1140)");
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): `min-height: 2em` at the 16px default is 32px; before SR1140 it survived only
    /// because the font-relative re-application re-folded it after the clear; now the fold keeps it.
    #[test]
    fn contain_size_min_height_em_is_kept() {
        let h = contain_size_min_height_of(r#"<div id="want" style="container-type: size; min-height: 2em"><div style="height: 100px"></div></div>"#);
        assert_eq!(h, 32.0, "a size container takes its em min-height");
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): with no min-height a size container is the empty box: 0 content plus its
    /// 10px padding top and bottom, whatever its child's height (unchanged).
    #[test]
    fn contain_size_min_height_none_is_empty_box() {
        let h = contain_size_min_height_of(r#"<div id="want" style="container-type: size; padding: 10px"><div style="height: 100px"></div></div>"#);
        assert_eq!(h, 20.0, "no min-height: 0 plus the padding");
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): `contain: size` runs through the same fold: `min-height: 40px` makes it 40px.
    #[test]
    fn contain_size_min_height_contain_size_is_kept() {
        let h = contain_size_min_height_of(r#"<div id="want" style="contain: size; min-height: 40px"><div style="height: 100px"></div></div>"#);
        assert_eq!(h, 40.0, "contain: size takes its min-height (armed: 0 before SR1140)");
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): a size-container flex item with no min-height stays 0 tall: the auto clear
    /// keeps the automatic minimum from pulling its 100px child back in (the reason the clear stays for the auto case).
    #[test]
    fn contain_size_min_height_flex_item_auto_stays_empty() {
        let h = contain_size_min_height_of(r#"<div style="display: flex; flex-direction: column; height: 300px"><div id="want" style="container-type: size"><div style="height: 100px"></div></div></div>"#);
        assert_eq!(h, 0.0, "a size-container flex item with an auto min-height is the empty box");
    }

    /// AETHERCSSCONTAINSIZEMINHEIGHT (SR1140): a declared height skips the size-containment fold: `height: 50px` stays 50.
    #[test]
    fn contain_size_min_height_declared_height_wins() {
        let h = contain_size_min_height_of(r#"<div id="want" style="container-type: size; height: 50px; min-height: 10px"><div style="height: 100px"></div></div>"#);
        assert_eq!(h, 50.0, "a declared height is the box's height");
    }

    // ---- AETHERCSSBORDERALPHACORNEROVERLAP (LEDGER SR1143): each plain border pixel composites once, the corners stay the vertical side's (named tail block) ----

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): 40x20 boxes (48x28 with the frame) on white, rows 12px apart: `#ha` a
    /// `4px solid rgba(0, 0, 0, 0.5)` frame, `#oc` an opaque frame with red top / bottom and blue left / right, `#cl` a
    /// `box-sizing: border-box` 6px box under the same half-alpha frame (8x8: the borders floor the box), and a `<p>`
    /// holding `#is`, an inline span with the half-alpha frame and white text.
    fn border_alpha_corner_engine() -> crate::AetherEngine {
        let html = r##"<!doctype html><html><head><style>
            body { margin: 0; background: #fff }
            div { width: 40px; height: 20px; margin: 0 0 12px }
            p { margin: 0 0 12px; padding: 0 8px; font: 16px sans-serif }
            </style></head><body>
            <div id="ha" style="border:4px solid rgba(0, 0, 0, 0.5)"></div>
            <div id="oc" style="border:4px solid red;border-left-color:blue;border-right-color:blue"></div>
            <div id="cl" style="box-sizing:border-box;width:6px;height:6px;border:4px solid rgba(0, 0, 0, 0.5)"></div>
            <p><span id="is" style="border:4px solid rgba(0, 0, 0, 0.5);color:#fff">x</span></p>
        </body></html>"##;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        engine
    }

    /// The four corner pixels (one pixel in from each corner of the border box) and the top edge's mid pixel.
    fn border_alpha_corner_px(e: &mut crate::AetherEngine, id: &str) -> ([(u8, u8, u8); 4], (u8, u8, u8)) {
        let (l, t, w, h) = imgalt_rect(e, id);
        let (l, t, w, h) = (l.round() as usize, t.round() as usize, w.round() as usize, h.round() as usize);
        let c = [imgalt_px(e, l + 1, t + 1), imgalt_px(e, l + w - 2, t + 1), imgalt_px(e, l + 1, t + h - 2), imgalt_px(e, l + w - 2, t + h - 2)];
        (c, imgalt_px(e, l + w / 2, t + 1))
    }

    fn border_alpha_half_grey(c: (u8, u8, u8)) -> bool {
        [c.0, c.1, c.2].iter().all(|v| (110..=150).contains(v))
    }

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): a half-alpha frame's four corner pixels equal its top edge (~127 grey on
    /// white). Before SR1143 the top / bottom and left / right rects both covered each corner square: 63 grey (0.75).
    #[test]
    fn border_alpha_corner_half() {
        let mut e = border_alpha_corner_engine();
        let (corners, edge) = border_alpha_corner_px(&mut e, "ha");
        assert!(border_alpha_half_grey(edge), "the top edge is half-alpha grey: {edge:?}");
        for (i, c) in corners.iter().enumerate() {
            assert_eq!(*c, edge, "armed: corner {i} composites once, like the edge (before: 63 grey)");
        }
    }

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): an opaque two-colour frame keeps its corner owner: the left / right
    /// sides paint after the top / bottom (render's order top, bottom, left, right), so all four corners were and stay
    /// blue; the top edge is red. Pinned: the horizontal rects are the ones inset.
    #[test]
    fn border_alpha_corner_opaque_owner() {
        let mut e = border_alpha_corner_engine();
        let (corners, edge) = border_alpha_corner_px(&mut e, "oc");
        assert_eq!(edge, (255, 0, 0), "the top edge is red");
        assert_eq!(corners, [(0, 0, 255); 4], "every corner stays the vertical side's blue");
    }

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): SR1134's currentcolor frame under half-alpha text (`#hb`): the corner
    /// equals the top edge (SR1134 sampled mid-x because the corner read 63).
    #[test]
    fn border_alpha_corner_currentcolor() {
        let mut e = currentcolor_alpha_engine();
        let (corners, edge) = border_alpha_corner_px(&mut e, "hb");
        assert!(border_alpha_half_grey(edge), "the currentcolor top edge is half-alpha grey: {edge:?}");
        assert_eq!(corners, [edge; 4], "armed: every corner composites once");
    }

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): a box no wider than its side borders (8x8 under 4px sides): the inset
    /// top / bottom rects are empty (no underflow, no panic) and the left / right rects tile the box, every pixel once.
    #[test]
    fn border_alpha_corner_collapse() {
        let mut e = border_alpha_corner_engine();
        let (l, t, w, h) = imgalt_rect(&mut e, "cl");
        assert_eq!((w, h), (8.0, 8.0), "the 4px sides floor the border-box 6px box at 8x8");
        for y in t as usize..(t + h) as usize {
            for x in l as usize..(l + w) as usize {
                let c = imgalt_px(&e, x, y);
                assert!(border_alpha_half_grey(c), "({x},{y}) composites once: {c:?}");
            }
        }
    }

    /// AETHERCSSBORDERALPHACORNEROVERLAP (SR1143): the inline road (a square solid span frame) has the same four-rect shape:
    /// its corner pixel equals its top edge.
    #[test]
    fn border_alpha_corner_inline() {
        let mut e = border_alpha_corner_engine();
        let (corners, edge) = border_alpha_corner_px(&mut e, "is");
        assert!(border_alpha_half_grey(edge), "the span's top edge is half-alpha grey: {edge:?}");
        assert_eq!(corners[0], edge, "armed: the inline frame's top-left corner composites once");
    }
}
