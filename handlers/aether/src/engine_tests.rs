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
        
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form_data.iter())
            .finish();
            
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
        
        // Type text
        engine.handle_event(crate::api::events::Event::Text(" world".to_string()));
        
        // Verify text was added
        {
            let focused_node = engine.focused_node.as_ref().unwrap();
            let el = focused_node.as_element().unwrap();
            assert_eq!(el.attributes.borrow().get("value"), Some("hello world"));
        }
        
        // Backspace
        engine.handle_event(crate::api::events::Event::KeyDown("BackSpace".to_string()));
        
        {
            let focused_node = engine.focused_node.as_ref().unwrap();
            let el = focused_node.as_element().unwrap();
            assert_eq!(el.attributes.borrow().get("value"), Some("hello worl"));
        }
        
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
            div { filter: blur(2px); display: grid; }
        </style></head><body>
            <div id="t">x</div>
            <script>
                clearTimeout(1);
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://ledger", html, true);

        let snap = crate::ledger::snapshot();
        use crate::ledger::ApiCategory;
        assert!(snap.contains(ApiCategory::Css, "property:filter"), "unhandled CSS property must be recorded");
        assert!(snap.contains(ApiCategory::Css, "display:grid"), "unhandled display value must be recorded");
        assert!(snap.contains(ApiCategory::Js, "window.clearTimeout"), "no-op clearTimeout must be recorded");
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

    /// :hover/:focus rules must not apply to a static render.
    #[test]
    fn test_hover_focus_not_applied() {
        let html = r#"<html><body><a id="x" href="/">link</a></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        css::apply_css(&mut layout_tree, r#"
            a:hover { color: red; }
            a:focus { color: yellow; }
            a { font-size: 20px; }
        "#);
        let paint = layout_tree.node_map.iter()
            .find(|(_, n)| n.as_element().map_or(false, |e| e.name.local.as_ref() == "a"))
            .map(|(id, _)| layout_tree.paint_map.get(id).cloned().unwrap_or_default())
            .unwrap();
        assert_eq!(paint.color, None, ":hover/:focus must not match a static render");
        assert_eq!(paint.font_size, Some(20.0), "plain tag rule still applies");
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
    /// verified offline by mocking the native layer underneath it.
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
            __native_fetch = function (u, m, b) {
                return { status: 200, url: u, body: '{"n": 7, "method": "' + m + '"}' };
            };
            fetch('/api/data', { method: 'POST', body: 'p=1' })
                .then(function (r) { return r.ok ? r.json() : null; })
                .then(function (j) {
                    document.getElementById('x').setAttribute('data-n', String(j.n) + j.method);
                });
        "#).expect("fetch chain must run");
        let _ = js.context.run_jobs();
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        assert_eq!(x.attributes.borrow().get("data-n"), Some("7POST"),
            "fetch → json → DOM mutation must complete through the job queue");
    }

    /// rAF callbacks fire once per load in bounded passes: a framework
    /// paint lands in the DOM; a self-re-registering animation loop
    /// terminates instead of recursing; XHR completes synchronously.
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
                 xhr.open('GET', '/api');
                 xhr.onload = function () {
                   document.getElementById('x').setAttribute('data-xhr', 'state' + xhr.readyState);
                 };
                 __native_fetch = function () { return { status: 200, url: '/api', body: 'ok' }; };
                 xhr.send();
                 document.getElementById('x').setAttribute('data-loops-pre', String(loops));
               </script></body></html>"#,
            &[],
            true,
        );
        let doc = engine.document.clone().unwrap();
        let x = doc.select("#x").unwrap().next().unwrap();
        let attrs = x.attributes.borrow();
        assert_eq!(attrs.get("data-paint"), Some("tstrue"), "one-shot rAF must fire with a timestamp");
        assert_eq!(attrs.get("data-xhr"), Some("state4"), "XHR onload must fire synchronously");
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
            "https://example.com/watch/",
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
        assert_eq!(staged.0, "https://example.com/media/clip.webm");
        assert_eq!(staged.2, "video/webm");
        assert!(engine.take_pending_media().is_none(), "take must consume");

        // media_sources still reports the page's streams for enumeration.
        let sources = engine.media_sources();
        assert_eq!(sources, vec![("https://example.com/media/clip.webm".to_string(), "video/webm".to_string())]);
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
                    assert_eq!(paint.family, Some(1), "Georgia maps to serif");
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
        assert_eq!(f[3], "onabort,l1", "abort dispatches once, to onabort then listeners");
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
        assert_eq!(
            later.as_node().text_contents(),
            "null",
            "currentScript is null outside a running script"
        );
    }

    /// The fetched-page path: the collected source list is NOT aligned with
    /// the document's `<script>` elements (a non-JS type sits between them
    /// here), so `currentScript` has to follow the per-slot ordinal rather
    /// than the slot index. Getting that wrong hands a script the wrong
    /// element, which is worse than handing it none.
    #[test]
    fn current_script_follows_the_slot_ordinal_not_the_slot_index() {
        let html = r#"<html><body>
            <script id="s0">A()</script>
            <script type="application/ld+json">{"not":"js"}</script>
            <script id="s2">B()</script>
        </body></html>"#;
        let (slots, external) = crate::net::collect_scripts("https://example.com/", html);
        assert!(external.is_empty());
        let scripts: Vec<String> = vec![
            "window.__seen = [document.currentScript.id];".to_string(),
            "window.__seen.push(document.currentScript.id);".to_string(),
        ];
        let script_nodes: Vec<usize> = slots.iter().map(|s| s.ordinal).collect();
        assert_eq!(script_nodes, vec![0, 2]);
        let page = crate::net::Page {
            base_url: "https://example.com/".to_string(),
            html: html.to_string(),
            sheets: Vec::new(),
            images: Vec::new(),
            scripts,
            script_nodes,
        };
        let mut engine = crate::AetherEngine::new();
        engine.load_page(page, true);
        let js = engine.js_engine.as_mut().unwrap();
        let seen = js
            .execute("window.__seen.join(',')")
            .unwrap()
            .to_string(&mut js.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(seen, "s0,s2", "each script saw its OWN element");
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
}
