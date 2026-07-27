
pub mod net;
pub mod dom;
pub mod layout;
pub mod render;
pub mod js;
pub mod images;
pub mod forms;
pub mod css;

pub mod headless;
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
    /// All stylesheet text applied to the current document (document
    /// <style> blocks + external sheets), kept for script-driven relayout.
    pub stylesheets: Vec<String>,
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
            stylesheets: Vec::new(),
        }
    }

    /// Rebuilds layout from the (possibly script-mutated) DOM and re-applies
    /// the page's stylesheets. The M3 mutation→relayout half-loop.
    pub fn relayout(&mut self) {
        let Some(document) = self.document.clone() else { return };
        let mut layout_tree = layout::compute_layout_sized(&document, self.width as f32, self.height as f32);
        css::apply_stylesheets(&mut layout_tree, &self.stylesheets);
        self.layout_tree = Some(layout_tree);
        self.needs_repaint = true;
        self.damage_rects.push((0, 0, self.width, self.height));
    }

    pub fn surface(&self) -> &[u8] {
        &self.surface
    }

    pub fn tick(&mut self) -> bool {
        if let Some(js) = &mut self.js_engine {
            let _ = js.context.run_jobs();
        }
        if js::take_mutated() {
            self.relayout();
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
                // Real reflow: media queries and wrap widths depend on the
                // viewport, so relayout — not just repaint.
                self.relayout();
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
                let focused = self.focused_node.clone();
                if let Some(node) = focused.as_ref() {
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
                                if let Some(doc_req) = self.build_form_submission(node) {
                                    let _ = tokio::task::block_in_place(|| {
                                        tokio::runtime::Handle::current().block_on(async {
                                            match doc_req.method {
                                                forms::HttpMethod::Get => {
                                                    let _ = self.load_url_internal(&doc_req.url, true).await;
                                                }
                                                forms::HttpMethod::Post => {
                                                    let body = doc_req.body.unwrap_or_default();
                                                    match net::post_document(&doc_req.url, &body).await {
                                                        Ok(html) => self.load_html(&doc_req.url, &html, true),
                                                        Err(e) => self.load_error_page(&doc_req.url, &e.to_string()),
                                                    }
                                                }
                                            }
                                        })
                                    });
                                }
                            }
                        }
                    }
                }
            }
            api::events::Event::MouseMove(_x, _y) => {}
            api::events::Event::MouseDown(_x, _y) => {}
            api::events::Event::MouseUp(x, y) => {
                if let Some(node) = self.hit_test(x, y) {
                    // Script click handlers run first (bubbling); a handled
                    // click still follows links, matching default behavior.
                    if let Some(js_engine) = &mut self.js_engine {
                        if js::dispatch_event(&mut js_engine.context, &node, "click") {
                            self.relayout();
                        }
                    }
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

    /// Builds a form submission from the focused control's enclosing <form>:
    /// collects named inputs, resolves the action against the current page.
    fn build_form_submission(&self, control: &kuchiki::NodeRef) -> Option<forms::OpenDocument> {
        // Walk up to the enclosing form.
        let mut cur = Some(control.clone());
        let form_node = loop {
            let n = cur?;
            if let Some(el) = n.as_element() {
                if &*el.name.local == "form" {
                    break n;
                }
            }
            cur = n.parent();
        };

        let form_el = form_node.as_element()?;
        let attrs = form_el.attributes.borrow();
        let action = attrs.get("action").unwrap_or("").to_string();
        let method = if attrs.get("method").map(|m| m.eq_ignore_ascii_case("post")).unwrap_or(false) {
            forms::HttpMethod::Post
        } else {
            forms::HttpMethod::Get
        };
        drop(attrs);

        // Resolve action against the current page URL.
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        let resolved = if action.is_empty() {
            base.clone()
        } else {
            url::Url::parse(&base)
                .and_then(|b| b.join(&action))
                .map(|u| u.to_string())
                .unwrap_or(action)
        };

        let mut form = forms::Form::new(resolved, method);
        if let Ok(inputs) = form_node.select("input, textarea, select") {
            for input in inputs {
                let attrs = input.attributes.borrow();
                let Some(name) = attrs.get("name") else { continue };
                let value = attrs.get("value").unwrap_or("").to_string();
                form.add_input(name.to_string(), value);
            }
        }
        Some(form.submit())
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

    #[deprecated(since = "0.1.0", note = "use aether::net::fetch_document and engine.load_html instead to prevent borrow panics")]
    pub async fn load_url(&mut self, url: &str) -> anyhow::Result<()> {
        self.load_url_internal(url, true).await
    }

    async fn load_url_internal(&mut self, url: &str, add_history: bool) -> anyhow::Result<()> {
        let html = match net::fetch_document(url).await {
            Ok(content) => content,
            Err(e) => {
                self.load_error_page(url, &e.to_string());
                return Ok(());
            }
        };
        self.load_html(url, &html, add_history);
        Ok(())
    }

    pub fn load_error_page(&mut self, url: &str, error: &str) {
        let html = format!(
            "<html><head><title>Error</title></head><body style=\"background-color: #f8d7da; color: #721c24; padding: 20px; font-family: sans-serif;\"><h1>Navigation Error</h1><p>Failed to load {}: {}</p></body></html>",
            url, error
        );
        self.load_html(url, &html, true);
    }

    pub fn load_html(&mut self, url: &str, html: &str, add_history: bool) {
        self.load_html_styled(url, html, &[], add_history)
    }

    /// Media the current page references, ready to hand to Stria. We own the
    /// browser and the OS, so a `<video>`/`<audio>` source the page already
    /// resolved is ours to play — same passthrough as audio, no site-specific
    /// resolver. Returns (absolute src, mime) for each playable element.
    pub fn media_sources(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Some(doc) = &self.document else { return out };
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        let Ok(media) = doc.select("video, audio, source") else { return out };
        for el in media {
            let attrs = el.attributes.borrow();
            let Some(src) = attrs.get("src") else { continue };
            if src.is_empty() {
                continue;
            }
            let abs = images::resolve(&base, src);
            let mime = attrs.get("type").map(str::to_string).unwrap_or_else(|| {
                match abs.rsplit('.').next() {
                    Some("mp4") => "video/mp4",
                    Some("webm") => "video/webm",
                    Some("mp3") => "audio/mpeg",
                    Some("ogg") => "audio/ogg",
                    Some("wav") => "audio/wav",
                    _ => "application/octet-stream",
                }
                .to_string()
            });
            out.push((abs, mime));
        }
        out
    }

    /// Like `load_html`, with pre-fetched external stylesheets (see
    /// `net::fetch_page`) applied after the document's own `<style>` blocks.
    pub fn load_html_styled(&mut self, url: &str, html: &str, external_css: &[String], add_history: bool) {
        self.load_impl(url, html, external_css, None, add_history);
    }

    /// Loads a fully fetched Page: installs its images and runs its scripts
    /// (inline AND fetched external, in document order) — the charter path
    /// for scripted sites: the page's own JS drives its state; no per-site
    /// code anywhere.
    pub fn load_page(&mut self, page: net::Page, add_history: bool) {
        images::set_page(&page.base_url, page.images);
        self.load_impl(&page.base_url, &page.html, &page.sheets, Some(&page.scripts), add_history);
    }

    fn load_impl(
        &mut self,
        url: &str,
        html: &str,
        external_css: &[String],
        scripts_override: Option<&[String]>,
        add_history: bool,
    ) {
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
        match scripts_override {
            Some(scripts) => {
                for text in scripts {
                    if let Err(e) = js_engine.execute(text) {
                        let msg = e.to_string();
                        ledger::record_js(&format!("script-error:{}", &msg[..msg.len().min(64)]));
                    }
                }
            }
            None => {
                if let Ok(scripts) = document.select("script") {
                    for script_node in scripts {
                        let text = script_node.as_node().text_contents();
                        if !text.trim().is_empty() {
                            if let Err(e) = js_engine.execute(&text) {
                                let msg = e.to_string();
                                ledger::record_js(&format!("script-error:{}", &msg[..msg.len().min(64)]));
                            }
                        }
                    }
                }
            }
        }
        
        // Drain zero-delay boot timers before first layout — pages queue
        // their init through setTimeout(0) and expect it before paint.
        let _ = js_engine.context.run_jobs();

        let mut sheets: Vec<String> = Vec::new();
        if let Ok(styles) = document.select("style") {
            for style_node in styles {
                sheets.push(style_node.as_node().text_contents());
            }
        }
        sheets.extend(external_css.iter().cloned());

        let mut layout_tree = layout::compute_layout_sized(&document, self.width as f32, self.height as f32);
        css::apply_stylesheets(&mut layout_tree, &sheets);
        self.stylesheets = sheets;
        
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

