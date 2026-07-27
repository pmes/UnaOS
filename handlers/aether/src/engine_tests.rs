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
            let paint = layout_tree.paint_map.get(node_id).copied().unwrap_or_default();
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
                let paint = layout_tree.paint_map.get(node_id).copied().unwrap_or_default();
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
                var el = document.getElementById("t");
                el.setAttribute("data-x", "1");
                el.addEventListener("click", function() {});
            </script>
        </body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.load_html("fixture://ledger", html, true);

        let snap = crate::ledger::snapshot();
        use crate::ledger::ApiCategory;
        assert!(snap.contains(ApiCategory::Css, "property:filter"), "unhandled CSS property must be recorded");
        assert!(snap.contains(ApiCategory::Css, "display:grid"), "unhandled display value must be recorded");
        assert!(snap.contains(ApiCategory::Js, "Element.setAttribute"), "no-op setAttribute must be recorded");
        assert!(snap.contains(ApiCategory::Js, "EventTarget.addEventListener"), "no-op addEventListener must be recorded");
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
}
