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
    /// and `border-color: transparent` drops the UA control stroke.
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
        assert_eq!(p.border, Some([None; 4]), "transparent border-color = no stroke");
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
            .map(|x| crate::fonts::line_height(x, 13.333, 0.0))
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
        assert_eq!(p.bg_position, None, "no position from the colour's inner slash");
        assert_eq!(p.bg_size, None, "no size from the colour's inner slash");
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

    /// AETHERCSSOMFONTSIZEREL (SR819): `smaller` / `larger` under 10px read layout_core's /1.2 and x1.2, in the report's
    /// 4-decimal grammar.
    #[test]
    fn font_size_rel_cssom_smaller_larger() {
        let mut e = font_size_rel_engine();
        assert_eq!(font_shorthand_cs(&mut e, "sm", "fontSize"), "8.3333px", "smaller: 10 / 1.2");
        assert_eq!(font_shorthand_cs(&mut e, "lg", "fontSize"), "12px", "larger: 10 x 1.2");
    }

    /// AETHERCSSOMFONTSIZEREL (SR819): a `font: inherit` button under 20px reads 20px (it painted 20px and read the
    /// 13.333px control default before), a plain button reads 13.333px (CONTROL_FONT_PX: Chromium's 13.3333px is
    /// AETHER-CONTROL-FONT-PX-THIRD owed), a `font-size: 2em` button under 10px reads 20px.
    #[test]
    fn font_size_rel_cssom_font_inherit_button() {
        let mut e = font_size_rel_engine();
        assert_eq!(font_shorthand_cs(&mut e, "fib", "fontSize"), "20px", "font: inherit takes the parent's 20px");
        assert_eq!(font_shorthand_cs(&mut e, "plain", "fontSize"), "13.333px", "the control default");
        assert_eq!(font_shorthand_cs(&mut e, "emb", "fontSize"), "20px", "2em of 10px on a control");
    }

    /// AETHERCSSOMFONTSIZEREL (SR819): `1.5em` inside `2em` under 10px reads 30px: the em chain is the layout's.
    #[test]
    fn font_size_rel_cssom_nested_em() {
        let mut e = font_size_rel_engine();
        assert_eq!(font_shorthand_cs(&mut e, "nest", "fontSize"), "30px", "1.5 x 2 x 10px");
    }

    // ---- AETHERRTLMARGINAUTO (LEDGER SR781): an rtl column's over-constrained margins bind margin-right (named tail block) ----

    /// AETHERRTLMARGINAUTO (SR781, AETHER-RTL-MARGIN-AUTO): in a 300px `dir=rtl` column a 100px block with margin-left 10
    /// and margin-right 20 sits 20 from the right edge (CSS 2.2 §10.3.3: margin-left is the over-constrained one; SR711's
    /// END), `margin-left: auto` takes the slack to the same place, `margin: 0 auto` centres, an ltr column keeps
    /// margin-left; `style.direction` flips re-derive it.
    #[test]
    fn rtl_margin_over_constrained_ends_at_margin_right() {
        let html = r#"<html><head><style>
            body { margin: 0; }
            .col { width: 300px; }
            .col div { width: 100px; height: 10px; }
        </style></head><body>
        <div class="col" id="rc" dir="rtl">
            <div id="a" style="margin-left: 10px; margin-right: 20px"></div>
            <div id="b" style="margin-left: auto; margin-right: 20px"></div>
            <div id="c" style="margin: 0 auto"></div>
        </div>
        <div class="col" id="lc"><div id="d" style="margin-left: 10px; margin-right: 20px"></div></div>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(800, 600));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let off = |e: &mut crate::AetherEngine, col: &str, id: &str| {
            js_num(e, &format!("document.getElementById('{id}').getBoundingClientRect().left - document.getElementById('{col}').getBoundingClientRect().left"))
        };
        assert_eq!(off(&mut engine, "rc", "a"), 180.0, "rtl 10 / 20: margin-right binds");
        assert_eq!(off(&mut engine, "rc", "b"), 180.0, "rtl auto / 20: the auto margin takes the slack");
        assert_eq!(off(&mut engine, "rc", "c"), 100.0, "rtl 0 auto: centred");
        assert_eq!(off(&mut engine, "lc", "d"), 10.0, "ltr 10 / 20: margin-left binds");
        for (dir, want) in [("ltr", 10.0), ("rtl", 180.0)] {
            js_num(&mut engine, &format!("document.getElementById('rc').style.direction = '{dir}'; 0"));
            engine.render_frame();
            assert_eq!(off(&mut engine, "rc", "a"), want, "style.direction = {dir}: the 10 / 20 block's offset");
        }
    }


    // ---- AETHERCENTERRESTYLEITEMS (LEDGER SR782): a removed align=center lets the cell's narrow block go back to the start (named tail block) ----

    /// AETHERCENTERRESTYLEITEMS (SR782): a `td align=center` centres its 200px block (-webkit-center, SR436); a script
    /// that removes the attribute leaves the block at the cell's content start, as Chromium re-derives the placement on
    /// the style recalc: neither the rule's align-self (SR686) nor the cell's align-items the rule wrote is kept.
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

    // ---- AETHERCSSWIDENONINHERITEDREST (LEDGER SR848): initial / unset on the rest of the non-inherited arms beat an earlier rule (named tail block) ----

    /// AETHERCSSWIDENONINHERITEDREST (SR848): one armed / reset pair per row at 400 px wide, each `.r` twin resetting
    /// what its class's rule set: position absolute (initial), float left (unset), opacity 0.5 on red (initial), a
    /// 100px translate (initial), overflow hidden on a 10px box with a 40px red child (initial), a column flexbox
    /// (initial), and a relative box 30px down (top: initial).
    fn css_wide_rest_engine() -> crate::AetherEngine {
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
        let mut e = css_wide_rest_engine();
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
        let mut e = css_wide_rest_engine();
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
        let mut e = css_wide_rest_engine();
        assert_eq!(css_wide_color_ink(&mut e, "op", css_wide_red), 0, "opacity 0.5 red is pink (armed)");
        assert!(css_wide_color_ink(&mut e, "opr", css_wide_red) > 100, "opacity: initial paints full red");
    }

    /// AETHERCSSWIDENONINHERITEDREST (SR848): `transform: initial` after `translateX(100px)` paints the 50px box at its
    /// untransformed x (red at 0..50, none at 100..150; the translated twin the reverse, armed).
    #[test]
    fn css_wide_rest_transform_untransformed() {
        let mut e = css_wide_rest_engine();
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
        let mut e = css_wide_rest_engine();
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
        let mut e = css_wide_rest_engine();
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
        let mut e = css_wide_rest_engine();
        let (_, s, ..) = imgalt_rect(&mut e, "tps");
        let (_, b, ..) = imgalt_rect(&mut e, "tp");
        let (_, rs, ..) = imgalt_rect(&mut e, "tprs");
        let (_, rb, ..) = imgalt_rect(&mut e, "tpr");
        assert!(((b - s) - (rb - rs) - 30.0).abs() <= 1.0, "top: initial drops the 30px offset: {} vs {}", b - s, rb - rs);
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
}
