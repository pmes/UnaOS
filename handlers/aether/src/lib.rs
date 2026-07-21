
pub mod net;
pub mod dom;
pub mod layout;
pub mod render;
pub mod js;
pub mod images;
pub mod forms;
pub mod css;

pub mod ledger;
pub mod storage;
pub mod workers;
pub mod event_loop;
pub mod fonts;
pub mod api;

#[cfg(test)]
mod engine_tests;
pub struct AetherEngine {
    pub document: Option<kuchiki::NodeRef>,
    pub layout_tree: Option<layout::LayoutTree>,
    pub js_engine: Option<js::Engine>,
    pub needs_repaint: bool,
}

impl AetherEngine {
    pub fn new() -> Self {
        Self {
            document: None,
            layout_tree: None,
            js_engine: None,
            needs_repaint: false,
        }
    }

    pub async fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
        let html = net::fetch_document(url).await?;
        let document = dom::parse_html(&html);
        
        let mut js_engine = js::Engine::new(document.clone());
        if let Ok(scripts) = document.select("script") {
            for script_node in scripts {
                let text = script_node.text_contents();
                if !text.trim().is_empty() {
                    let _ = js_engine.execute(&text);
                }
            }
        }
        
        let mut layout_tree = layout::compute_layout(&document);
        if let Ok(styles) = document.select("style") {
            for style_node in styles {
                let text = style_node.text_contents();
                css::apply_css(&mut layout_tree, &text);
            }
        }

        self.document = Some(document);
        self.layout_tree = Some(layout_tree);
        self.js_engine = Some(js_engine);
        self.needs_repaint = true;
        
        Ok(())
    }

    pub fn render_frame(&mut self, surface: &mut [u8], w: u32, h: u32) {
        if let Some(layout) = &self.layout_tree {
            render::render_frame(layout, surface, w, h);
            self.needs_repaint = false;
        } else {
            // Fill white if no document loaded
            for chunk in surface.chunks_exact_mut(4) {
                chunk[0] = 255;
                chunk[1] = 255;
                chunk[2] = 255;
                chunk[3] = 255;
            }
        }
    }
}

