
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
    pub damage_rects: Vec<(u32, u32, u32, u32)>, // x, y, w, h
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub history: Vec<String>,
    pub history_idx: usize,
    pub width: u32,
    pub height: u32,
}

impl AetherEngine {
    pub fn new() -> Self {
        Self {
            document: None,
            layout_tree: None,
            js_engine: None,
            needs_repaint: false,
            damage_rects: Vec::new(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            history: Vec::new(),
            history_idx: 0,
            width: 800,
            height: 600,
        }
    }

    pub fn tick(&mut self) -> bool {
        if let Some(js) = &mut self.js_engine {
            let _ = js.context.run_jobs();
        }
        
        let needs_repaint = self.needs_repaint;
        self.needs_repaint = false;
        needs_repaint
    }
    
    pub fn handle_event(&mut self, event: api::events::Event) {
        match event {
            api::events::Event::Scroll(dx, dy) => {
                self.scroll_x = (self.scroll_x + dx).max(0.0);
                self.scroll_y = (self.scroll_y + dy).max(0.0);
                self.needs_repaint = true;
                // Full repaint on scroll for now, could be optimized
                self.damage_rects.push((0, 0, self.width, self.height));
            }
            api::events::Event::Resize(w, h) => {
                self.width = w;
                self.height = h;
                self.needs_repaint = true;
                self.damage_rects.push((0, 0, w, h));
            }
            api::events::Event::Text(_text) => {
                // Focus and text handling
            }
            api::events::Event::KeyDown(_key) => {}
            api::events::Event::MouseMove(_x, _y) => {}
            api::events::Event::MouseDown(_x, _y) => {}
            api::events::Event::MouseUp(_x, _y) => {}
        }
    }

    pub async fn go_back(&mut self) {
        if self.history_idx > 0 {
            self.history_idx -= 1;
            let url = self.history[self.history_idx].clone();
            let _ = self.load_url_internal(&url, false).await;
        }
    }

    pub async fn go_forward(&mut self) {
        if self.history_idx + 1 < self.history.len() {
            self.history_idx += 1;
            let url = self.history[self.history_idx].clone();
            let _ = self.load_url_internal(&url, false).await;
        }
    }

    pub async fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
        self.load_url_internal(url, true).await
    }

    async fn load_url_internal(&mut self, url: &str, add_history: bool) -> anyhow::Result<()> {
        let html = net::fetch_document(url).await?;
        let document = dom::parse_html(&html);
        
        if add_history {
            self.history.truncate(self.history_idx);
            self.history.push(url.to_string());
            self.history_idx = self.history.len() - 1;
        }
        
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
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.needs_repaint = true;
        self.damage_rects.push((0, 0, self.width, self.height));
        
        Ok(())
    }

    pub fn render_frame(&mut self, surface: &mut [u8], w: u32, h: u32) -> Vec<(u32, u32, u32, u32)> {
        let damages = std::mem::take(&mut self.damage_rects);
        
        if let Some(layout) = &self.layout_tree {
            render::render_frame(layout, surface, w, h, self.scroll_x, self.scroll_y);
        } else {
            // Fill white if no document loaded
            for chunk in surface.chunks_exact_mut(4) {
                chunk[0] = 255;
                chunk[1] = 255;
                chunk[2] = 255;
                chunk[3] = 255;
            }
        }
        
        if damages.is_empty() {
            vec![(0, 0, w, h)]
        } else {
            damages
        }
    }
}

