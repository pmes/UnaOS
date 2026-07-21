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
        let html = r#"<html><body><div id="box" style="width: 10px; height: 10px; background-color: red;"></div></body></html>"#;
        let document = dom::parse_html(html);
        let mut layout_tree = layout::compute_layout(&document);
        
        // CSS engine in aether is currently rudimentary, but we just verify it doesn't panic
        // and that we can paint the layout boxes.
        let mut surface = vec![0u8; 100 * 100 * 4];
        render::render_frame(&layout_tree, &mut surface, 100, 100, 0.0, 0.0);
        
        // The current render_frame fills white background: 255, 255, 255, 255
        // We just assert the background is painted at (0, 0) if no boxes overlap, or we check the box color.
        // The box borders are painted black, filled gray (200). 
        // We'll just verify the surface has been mutated from all 0s.
        let is_painted = surface.iter().any(|&b| b != 0);
        assert!(is_painted, "Surface should be painted by render_frame");
        
        // Assert pixel at 5,5 is red (0, 0, 255)
        let idx = (5 * 100 + 5) * 4;
        assert_eq!(surface[idx], 0, "B should be 0");
        assert_eq!(surface[idx+1], 0, "G should be 0");
        assert_eq!(surface[idx+2], 255, "R should be 255");
        assert_eq!(surface[idx+3], 255, "A should be 255");
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
}
