
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
    pub title: String,
    pub focused_node: Option<kuchiki::NodeRef>,
    pub surface: Vec<u8>,
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
            title: "Aether Browser".to_string(),
            focused_node: None,
            surface: vec![255; 800 * 600 * 4],
        }
    }

    pub fn surface(&self) -> &[u8] {
        &self.surface
    }

    pub fn tick(&mut self) -> bool {
        if let Some(js) = &mut self.js_engine {
            let _ = js.context.run_jobs();
        }
        
        let needs_repaint = self.needs_repaint;
        self.needs_repaint = false;
        needs_repaint
    }
    
    fn hit_test(&self, x: f64, y: f64) -> Option<kuchiki::NodeRef> {
        let layout = self.layout_tree.as_ref()?;
        let abs_x = x + self.scroll_x;
        let abs_y = y + self.scroll_y;
        
        let mut hit = None;
        fn walk(
            node_id: taffy::prelude::NodeId, 
            cx: f32, 
            cy: f32, 
            abs_x: f64, 
            abs_y: f64, 
            layout: &layout::LayoutTree,
            hit: &mut Option<kuchiki::NodeRef>
        ) {
            if let Ok(l) = layout.taffy.layout(node_id) {
                let nx = cx + l.location.x;
                let ny = cy + l.location.y;
                let nw = l.size.width;
                let nh = l.size.height;
                
                if abs_x >= nx as f64 && abs_x <= (nx + nw) as f64 &&
                   abs_y >= ny as f64 && abs_y <= (ny + nh) as f64 {
                    if let Some(dom_node) = layout.node_map.get(&node_id) {
                        *hit = Some(dom_node.clone());
                    }
                }
                
                if let Ok(children) = layout.taffy.children(node_id) {
                    for child in children {
                        walk(child, nx, ny, abs_x, abs_y, layout, hit);
                    }
                }
            }
        }
        
        walk(layout.root_node, 0.0, 0.0, abs_x, abs_y, layout, &mut hit);
        hit
    }

    pub fn handle_event(&mut self, event: api::events::Event) {
        match event {
            api::events::Event::Scroll(_dx, dy) => {
                let old_sy = self.scroll_y;
                self.scroll_y = (self.scroll_y + dy).max(0.0);
                let actual_dy = self.scroll_y - old_sy;
                let idy = actual_dy as i32;

                if idy != 0 {
                    let w = self.width as usize;
                    let h = self.height as usize;
                    
                    if idy > 0 && idy < h as i32 {
                        // Scrolling down, document moves up, shift pixels UP
                        let shift = idy as usize * w * 4;
                        self.surface.copy_within(shift.., 0);
                        self.damage_rects.push((0, (h as i32 - idy) as u32, self.width, idy as u32));
                    } else if idy < 0 && -idy < h as i32 {
                        // Scrolling up, document moves down, shift pixels DOWN
                        let shift = (-idy) as usize * w * 4;
                        let src_len = (h - (-idy) as usize) * w * 4;
                        self.surface.copy_within(0..src_len, shift);
                        self.damage_rects.push((0, 0, self.width, (-idy) as u32));
                    } else {
                        self.damage_rects.push((0, 0, self.width, self.height));
                    }
                    self.needs_repaint = true;
                }
            }
            api::events::Event::Resize(w, h) => {
                self.width = w;
                self.height = h;
                self.surface = vec![255; (w * h * 4) as usize];
                self.needs_repaint = true;
                self.damage_rects.push((0, 0, w, h));
            }
            api::events::Event::Text(text) => {
                if let Some(node) = &self.focused_node {
                    if let Some(el) = node.as_element() {
                        if &*el.name.local == "input" {
                            let mut attrs = el.attributes.borrow_mut();
                            let mut val = attrs.get("value").unwrap_or("").to_string();
                            val.push_str(&text);
                            attrs.insert("value", val);
                            self.needs_repaint = true;
                            // Approximate field damage rect: push full for now, layout mapping is needed
                            self.damage_rects.push((0, 0, self.width, self.height));
                        }
                    }
                }
            }
            api::events::Event::KeyDown(key) => {
                if let Some(node) = &self.focused_node {
                    if let Some(el) = node.as_element() {
                        if &*el.name.local == "input" {
                            if key == "BackSpace" {
                                let mut attrs = el.attributes.borrow_mut();
                                let mut val = attrs.get("value").unwrap_or("").to_string();
                                val.pop();
                                attrs.insert("value", val);
                                self.needs_repaint = true;
                                self.damage_rects.push((0, 0, self.width, self.height));
                            } else if key == "Return" {
                                // Form submission logic
                            }
                        }
                    }
                }
            }
            api::events::Event::MouseMove(_x, _y) => {}
            api::events::Event::MouseDown(_x, _y) => {}
            api::events::Event::MouseUp(x, y) => {
                if let Some(node) = self.hit_test(x, y) {
                    if let Some(el) = node.as_element() {
                        if &*el.name.local == "a" {
                            if let Some(href) = el.attributes.borrow().get("href") {
                                let href = href.to_string();
                                let _ = tokio::task::block_in_place(|| {
                                    tokio::runtime::Handle::current().block_on(async {
                                        let _ = self.load_url_internal(&href, true).await;
                                    })
                                });
                            }
                        } else if &*el.name.local == "input" {
                            self.focused_node = Some(node);
                        }
                    }
                }
            }
        }
    }

    pub async fn go_back(&mut self) {
        if let Some(url) = self.get_back_url() {
            let _ = self.load_url_internal(&url, false).await;
        }
    }

    pub fn get_back_url(&mut self) -> Option<String> {
        if self.history_idx > 0 {
            self.history_idx -= 1;
            Some(self.history[self.history_idx].clone())
        } else {
            None
        }
    }

    pub async fn go_forward(&mut self) {
        if let Some(url) = self.get_forward_url() {
            let _ = self.load_url_internal(&url, false).await;
        }
    }

    pub fn get_forward_url(&mut self) -> Option<String> {
        if self.history_idx + 1 < self.history.len() {
            self.history_idx += 1;
            Some(self.history[self.history_idx].clone())
        } else {
            None
        }
    }

    pub async fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
        self.load_url_internal(url, true).await
    }

    async fn load_url_internal(&mut self, url: &str, add_history: bool) -> anyhow::Result<()> {
        let html = match net::fetch_document(url).await {
            Ok(content) => content,
            Err(e) => {
                format!(
                    "<html><head><title>Error</title></head><body style=\"background-color: #f8d7da; color: #721c24; padding: 20px; font-family: sans-serif;\"><h1>Navigation Error</h1><p>Failed to load {}: {}</p></body></html>",
                    url, e
                )
            }
        };
        self.load_html(url, &html, add_history);
        Ok(())
    }

    pub fn load_html(&mut self, url: &str, html: &str, add_history: bool) {
        let document = dom::parse_html(html);
        
        self.title = "Aether Browser".to_string();
        if let Ok(mut titles) = document.select("title") {
            if let Some(title_node) = titles.next() {
                self.title = title_node.as_node().text_contents();
            }
        }
        
        if add_history {
            self.history.truncate(self.history_idx);
            self.history.push(url.to_string());
            self.history_idx = self.history.len() - 1;
        }
        
        let mut js_engine = js::Engine::new(document.clone());
        if let Ok(scripts) = document.select("script") {
            for script_node in scripts {
                let text = script_node.as_node().text_contents();
                if !text.trim().is_empty() {
                    let _ = js_engine.execute(&text);
                }
            }
        }
        
        let mut layout_tree = layout::compute_layout(&document);
        if let Ok(styles) = document.select("style") {
            for style_node in styles {
                let text = style_node.as_node().text_contents();
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
    }

    pub fn render_frame(&mut self) -> Vec<(u32, u32, u32, u32)> {
        let damages = std::mem::take(&mut self.damage_rects);
        
        if let Some(layout) = &self.layout_tree {
            render::render_frame(layout, &mut self.surface, self.width, self.height, self.scroll_x, self.scroll_y, &damages);
        } else {
            for chunk in self.surface.chunks_exact_mut(4) {
                chunk[0] = 255;
                chunk[1] = 255;
                chunk[2] = 255;
                chunk[3] = 255;
            }
        }
        
        if damages.is_empty() {
            vec![(0, 0, self.width, self.height)]
        } else {
            damages
        }
    }
}

