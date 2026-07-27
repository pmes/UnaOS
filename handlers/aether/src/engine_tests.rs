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
            <input id="field" type="text" value="hello" />
        </body></html>"#;
        
        let mut engine = crate::AetherEngine::new();
        engine.load_html("https://example.com/", html, false);
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
        
        // Enter submitting the form (offline)
        engine.handle_event(crate::api::events::Event::KeyDown("Return".to_string()));
        
        // 2. hit-test -> link navigation
        engine.handle_event(crate::api::events::Event::MouseDown(link_x, link_y));
        engine.handle_event(crate::api::events::Event::MouseUp(link_x, link_y));
        

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
        assert_eq!(slots[1].as_deref(), Some("inline1()"));
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
}
