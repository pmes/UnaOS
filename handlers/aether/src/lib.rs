
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
pub mod media;
pub mod selection;

#[cfg(test)]
mod engine_tests;
pub struct AetherEngine {
    pub document: Option<crate::dom::NodeRef>,
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
    pub focused_node: Option<crate::dom::NodeRef>,
    pub surface: Vec<u8>,
    /// All stylesheet text applied to the current document (document
    /// <style> blocks + external sheets), kept for script-driven relayout.
    pub stylesheets: Vec<String>,
    /// Navigation staged by a link click or form submit. The SHELL performs
    /// it asynchronously: the engine thread runs a current-thread runtime,
    /// where tokio::task::block_in_place panics — the old inline block here
    /// killed the engine thread on the first link click.
    pending_nav: Option<forms::OpenDocument>,
    /// AETHERHOVER (SR67): the hover path — the element under the pointer, then each ancestor element
    /// (innermost first). Empty when the pointer is over nothing or has not moved yet.
    pub hover_path: Vec<crate::dom::NodeRef>,
    /// The last pointer position (viewport px) a `MouseMove` delivered.
    pub pointer: Option<(f64, f64)>,
    /// The cursor keyword the shell was last told about, and one it has not been told about yet.
    cursor: String,
    pending_cursor: Option<String>,
    /// AETHERSELECT (SR69): the engine's own "button down" flag — motion extends the selection only
    /// while it is set (`pointer_moved`), and release clears it (`pointer_released`).
    button_down: bool,
    /// The drag in progress: its granularity (1 character, 2 word, 3 block) and the range the press
    /// selected (a word or block drag keeps it whole on either side).
    sel_drag: Option<SelDrag>,
    /// The last press (when, where, click count) — two presses within 500 ms and 4 px count up.
    last_press: Option<(std::time::Instant, f64, f64, u8)>,
    /// Shift held by the shell's pointer: a press extends the selection instead of starting one.
    pub shift_held: bool,
    /// The selection generation last painted.
    sel_gen: u64,
    /// Bumped whenever the layout tree is replaced; `painted_epoch` is the one the last full frame drew
    /// (the frame whose text fragments a pointer is hit-tested against).
    layout_epoch: u64,
    painted_epoch: u64,
}

/// A selection drag (AETHERSELECT).
#[derive(Clone, Copy, Debug)]
struct SelDrag {
    granularity: u8,
    lo: selection::Point,
    hi: selection::Point,
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
            pending_nav: None,
            hover_path: Vec::new(),
            pointer: None,
            cursor: "default".to_string(),
            pending_cursor: None,
            button_down: false,
            sel_drag: None,
            last_press: None,
            shift_held: false,
            sel_gen: selection::generation(),
            layout_epoch: 1,
            painted_epoch: 0,
        }
    }

    /// Rebuilds layout from the (possibly script-mutated) DOM and re-applies
    /// the page's stylesheets. The M3 mutation→relayout half-loop.
    pub fn relayout(&mut self) {
        let Some(document) = self.document.clone() else { return };
        // A script may have inserted media elements: register them (poster requests queue).
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        media::scan(&document, &base);
        let mut layout_tree = layout::build_tree(&document, self.width as f32, self.height as f32);
        css::apply_stylesheets(&mut layout_tree, &self.stylesheets);
        self.layout_tree = Some(layout_tree);
        self.layout_epoch += 1;
        self.needs_repaint = true;
        self.damage_rects.push((0, 0, self.width, self.height));
    }

    // ---------------------------------------------------------------------------------------------
    // AETHERSELECT (SR69): the selection the pointer makes
    // ---------------------------------------------------------------------------------------------

    /// Paints a full frame when the text fragments on record are not this layout's (a pointer is
    /// hit-tested against what was painted).
    fn ensure_painted(&mut self) {
        if self.layout_tree.is_some() && self.painted_epoch != self.layout_epoch {
            self.damage_rects.push((0, 0, self.width, self.height));
            self.render_frame();
        }
    }

    /// Repaints when the selection changed since the last paint (by the pointer or by script).
    fn sel_changed(&mut self) {
        let g = selection::generation();
        if g != self.sel_gen {
            self.sel_gen = g;
            self.needs_repaint = true;
            self.damage_rects.push((0, 0, self.width, self.height));
        }
    }

    fn hit_text(&mut self, x: f64, y: f64) -> Option<selection::Hit> {
        self.ensure_painted();
        let layout = self.layout_tree.as_ref()?;
        selection::hit(layout, &render::text_boxes(), (self.scroll_x, self.scroll_y), x, y)
    }

    /// The whole block a triple click selects: its first and last boundary points.
    fn block_range(&self, block: usize) -> Option<(selection::Point, selection::Point)> {
        let doc = self.document.as_ref()?;
        let len = doc.with_doc(|d, _| selection::node_length(d, block));
        Some((selection::Point { node: block, offset: 0 }, selection::Point { node: block, offset: len }))
    }

    /// The press at viewport (`x`, `y`) with a click count and Shift state: one click places a caret (a
    /// collapsed selection), two select the word (UAX #29) and three the block under the pointer, and
    /// Shift extends the current selection's focus to the pointer. The drag that follows extends it at
    /// the same granularity while the button is held.
    pub fn press_at(&mut self, x: f64, y: f64, clicks: u8, shift: bool) {
        self.button_down = true;
        let Some(hit) = self.hit_text(x, y) else {
            selection::set(None);
            self.sel_drag = None;
            self.sel_changed();
            return;
        };
        if shift {
            if let Some(cur) = selection::get() {
                selection::set(Some(selection::Selection { anchor: cur.anchor, focus: hit.point }));
                self.sel_drag = Some(SelDrag { granularity: 1, lo: cur.anchor, hi: cur.anchor });
                self.sel_changed();
                return;
            }
        }
        let (lo, hi, granularity) = match clicks {
            2 => (hit.word.0, hit.word.1, 2),
            3 => match hit.block.and_then(|b| self.block_range(b)) {
                Some((a, b)) => (a, b, 3),
                None => (hit.word.0, hit.word.1, 2),
            },
            _ => (hit.point, hit.point, 1),
        };
        selection::set(Some(selection::Selection { anchor: lo, focus: hi }));
        self.sel_drag = Some(SelDrag { granularity, lo, hi });
        self.sel_changed();
    }

    /// A press from the shell: the click count comes from the time and distance since the last one.
    fn selection_press(&mut self, x: f64, y: f64) {
        let now = std::time::Instant::now();
        let clicks = match self.last_press {
            Some((t, px, py, n)) if now.duration_since(t) < std::time::Duration::from_millis(500) && (px - x).abs() <= 4.0 && (py - y).abs() <= 4.0 => n % 3 + 1,
            _ => 1,
        };
        self.last_press = Some((now, x, y, clicks));
        self.press_at(x, y, clicks, self.shift_held);
    }

    /// Pointer motion (viewport px). While the button is held, extends the selection's focus to the
    /// pointer at the press's granularity. Called by the hover arc's `pointer_moved` (the fold's wiring).
    pub fn selection_pointer_moved(&mut self, x: f64, y: f64) {
        if !self.button_down {
            return;
        }
        let Some(drag) = self.sel_drag else { return };
        let Some(hit) = self.hit_text(x, y) else { return };
        let Some(doc) = self.document.clone() else { return };
        let ord = selection::order(&doc);
        let before = doc.with_doc(|d, _| match (ord.key(d, hit.point), ord.key(d, drag.lo)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        });
        let (anchor, focus) = match drag.granularity {
            2 => if before { (drag.hi, hit.word.0) } else { (drag.lo, hit.word.1) },
            3 => match hit.block.and_then(|b| self.block_range(b)) {
                Some((a, b)) => if before { (drag.hi, a) } else { (drag.lo, b) },
                None => (drag.lo, hit.point),
            },
            _ => (drag.lo, hit.point),
        };
        selection::set(Some(selection::Selection { anchor, focus }));
        self.sel_changed();
    }

    /// The button went up: the drag ends (the selection stays). SR68's `MouseUp` arm calls this.
    pub fn pointer_released(&mut self) {
        self.button_down = false;
    }

    /// Ctrl/Cmd+A: select the document's contents (the body's children).
    pub fn select_all(&mut self) {
        let Some(doc) = self.document.clone() else { return };
        let root = doc.select_first("body").map(|b| b.as_node().clone()).ok().or_else(|| doc.first_child());
        let Some(root) = root else { return };
        let len = root.with_doc(|d, id| selection::node_length(d, id.0));
        let id = root.id().0;
        selection::set(Some(selection::Selection {
            anchor: selection::Point { node: id, offset: 0 },
            focus: selection::Point { node: id, offset: len },
        }));
        self.sel_drag = None;
        self.sel_changed();
    }

    /// The selected text, serialized per the HTML `innerText` algorithm (`Selection.toString()`).
    pub fn selected_text(&self) -> String {
        match (&self.layout_tree, &self.document) {
            (Some(l), Some(d)) => selection::selection_text(l, d),
            _ => String::new(),
        }
    }

    /// `BrowserCopy`: the selected text for the shell's clipboard, as `BrowserClipboardSet`; None when
    /// nothing is selected.
    pub fn copy_selection(&self) -> Option<bandy::SMessage> {
        let t = self.selected_text();
        (!t.is_empty()).then_some(bandy::SMessage::BrowserClipboardSet(t))
    }

    /// Requests for Stria queued by the page's media elements (posters on layout, play/pause
    /// on click, stops on navigation). The shell fires each on the bus.
    pub fn take_media_requests(&mut self) -> Vec<bandy::SMessage> {
        media::take_outbox()
    }

    /// Feed one of Stria's replies (`MediaOpened` / `MediaFrame` / `MediaEnded` /
    /// `MediaError`) to the page. Returns true when the page needs a repaint (the element's
    /// box is pushed as damage; a changed intrinsic size relayouts).
    pub fn on_media_message(&mut self, msg: &bandy::SMessage) -> bool {
        let fx = media::on_message(msg);
        if !fx.handled {
            return false;
        }
        if fx.relayout {
            self.relayout();
            return true;
        }
        for node in &fx.repaint {
            if let Some(r) = self.screen_rect_of(node) {
                self.damage_rects.push(r);
                self.needs_repaint = true;
            }
        }
        self.needs_repaint
    }

    /// The page's media elements and what Aether knows of each.
    pub fn media_elements(&self) -> Vec<media::Element> {
        media::snapshot()
    }

    /// Play the `i`th media element (document order). False when there is none.
    pub fn media_play(&mut self, i: usize) -> bool {
        media::snapshot().get(i).is_some_and(|e| media::play(&e.node))
    }

    /// Pause the `i`th media element.
    pub fn media_pause(&mut self, i: usize) -> bool {
        media::snapshot().get(i).is_some_and(|e| media::pause(&e.node))
    }

    /// Seek the `i`th media element to `position_ns`.
    pub fn media_seek(&mut self, i: usize, position_ns: u64) -> bool {
        media::snapshot().get(i).is_some_and(|e| media::seek(&e.node, position_ns))
    }

    /// Absolute (document) border box of `node`'s layout box: (x, y, w, h).
    pub fn box_of(&self, node: &crate::dom::NodeRef) -> Option<(f32, f32, f32, f32)> {
        let layout = self.layout_tree.as_ref()?;
        fn walk(
            id: taffy::prelude::NodeId,
            cx: f32,
            cy: f32,
            layout: &layout::LayoutTree,
            want: &crate::dom::NodeRef,
        ) -> Option<(f32, f32, f32, f32)> {
            let l = layout.taffy.layout(id).ok()?;
            let (nx, ny) = (cx + l.location.x, cy + l.location.y);
            if layout.node_map.get(&id) == Some(want) {
                return Some((nx, ny, l.size.width, l.size.height));
            }
            for c in layout.taffy.children(id).ok()? {
                if let Some(r) = walk(c, nx, ny, layout, want) {
                    return Some(r);
                }
            }
            None
        }
        walk(layout.root_node, 0.0, 0.0, layout, node)
    }

    /// `node`'s box as a damage rect in viewport pixels, clamped; None when off screen.
    fn screen_rect_of(&self, node: &crate::dom::NodeRef) -> Option<(u32, u32, u32, u32)> {
        let (x, y, w, h) = self.box_of(node)?;
        let x0 = (x as f64 - self.scroll_x).floor().max(0.0) as u32;
        let y0 = (y as f64 - self.scroll_y).floor().max(0.0) as u32;
        let x1 = ((x + w) as f64 - self.scroll_x).ceil().clamp(0.0, self.width as f64) as u32;
        let y1 = ((y + h) as f64 - self.scroll_y).ceil().clamp(0.0, self.height as f64) as u32;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }

    pub fn surface(&self) -> &[u8] {
        &self.surface
    }

    /// One turn of the event loop: fire the timers due right now, then settle
    /// the microtasks they queued.
    ///
    /// Both halves are bounded by construction (see `event_loop`): the due set
    /// is snapshotted before any callback runs, so a callback that re-arms
    /// itself waits for the NEXT tick, and the job drain has a ceiling. A tick
    /// therefore always returns — which is the whole point, since the shell's
    /// `select!` loop cannot service navigation while it is inside one.
    pub fn tick(&mut self) -> bool {
        if let Some(js) = &mut self.js_engine {
            js.set_viewport(self.width as f32, self.height as f32, (self.scroll_x, self.scroll_y));
            js.tick();
        }
        if let Some(url) = js::take_script_navigation() {
            self.pending_nav = Some(forms::OpenDocument { url, method: forms::HttpMethod::Get, body: None });
        }
        if js::take_mutated() {
            self.relayout();
        }
        self.sel_changed();

        let needs_repaint = self.needs_repaint;
        self.needs_repaint = false;
        needs_repaint
    }
    
    /// The element a pointer at viewport (`x`, `y`) hits — see [`Self::hit_test_at`].
    fn hit_test(&self, x: f64, y: f64) -> Option<crate::dom::NodeRef> {
        self.hit_test_at(x, y).map(|(n, _)| n)
    }

    /// AETHERHOVER (SR67): hit testing in PAINT order (CSS 2.2 Appendix E, the order
    /// `render::render_frame` paints in), not tree order: within a stacking context the topmost
    /// thing painted wins — positioned / z-indexed layers by (z, order) above the normal flow (z >= 0)
    /// or below it (z < 0), and in the flow a later box over an earlier one, an atomic inline over its
    /// line, a glyph run over its inline box. `visibility: hidden` and `display: none` subtrees are not
    /// targets, an `overflow` clip bounds its descendants. Returns the hit ELEMENT (a text run resolves
    /// to its parent element, as UI Events targets do) and whether the point is over a glyph run (the
    /// `auto` cursor's I-beam). A point inside the viewport over no box hits the root element.
    pub fn hit_test_at(&self, x: f64, y: f64) -> Option<(crate::dom::NodeRef, bool)> {
        let layout = self.layout_tree.as_ref()?;
        let cx = HitCtx {
            layout,
            px: (x + self.scroll_x) as f32,
            py: (y + self.scroll_y) as f32,
            sx: self.scroll_x as i32 as f32,
            sy: self.scroll_y as i32 as f32,
            vw: self.width as f32,
            vh: self.height as f32,
        };
        let all = (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY);
        let (mut tid, over_text) = hit_context(&cx, layout.root_node, (0.0, 0.0), all).unwrap_or((layout.root_node, false));
        loop {
            if let Some(n) = layout.node_map.get(&tid) {
                if n.is_element() {
                    return Some((n.clone(), over_text));
                }
                if let Some(p) = n.parent().filter(|p| p.is_element()) {
                    return Some((p, over_text));
                }
                // The layout root is the Document: the canvas hits the document element.
                if n.as_document().is_some() {
                    return n.children().find(|c| c.is_element()).map(|e| (e, false));
                }
            }
            tid = layout.taffy.parent(tid)?;
        }
    }

    /// AETHERHOVER (SR67): the pointer moved to viewport (`x`, `y`). Hit tests in paint order; when the
    /// hovered element changes, fires UI Events §5.3's boundary events at the page — `mouseout` at the
    /// old target, `mouseleave` at each element left (innermost first), `mouseover` at the new target,
    /// `mouseenter` at each element entered (outermost first), Chromium's order — moves css_core's
    /// `:hover` flag onto the new path and re-styles ONCE (only when the page has a `:hover` rule or a
    /// listener mutated the DOM); then `mousemove` at the target, and the cursor on the hovered element
    /// is recomputed (a change is staged for [`Self::take_cursor_change`]). A motion that stays on the
    /// same element costs one hit test and one `mousemove`.
    pub fn pointer_moved(&mut self, x: f64, y: f64) {
        self.selection_pointer_moved(x, y); // AETHERSELECT (SR69): the drag extends while the button is held
        self.pointer = Some((x, y));
        let hit = self.hit_test_at(x, y);
        let target = hit.as_ref().map(|(n, _)| n.clone());
        let new_path: Vec<crate::dom::NodeRef> = match &target {
            Some(t) => std::iter::once(t.clone()).chain(t.ancestors().filter(|a| a.is_element())).collect(),
            None => Vec::new(),
        };
        let client = (x, y);
        let mut mutated = false;
        if new_path != self.hover_path {
            let old_path = std::mem::replace(&mut self.hover_path, new_path.clone());
            if let Some(js_engine) = &mut self.js_engine {
                js_engine.set_viewport(self.width as f32, self.height as f32, (self.scroll_x, self.scroll_y));
                let vm = &mut js_engine.vm;
                let id = |n: &crate::dom::NodeRef| n.id().0;
                let (old_t, new_t) = (old_path.first().map(id), new_path.first().map(id));
                if let Some(o) = old_t {
                    js::events::user_mouse(vm, o, "mouseout", client, new_t);
                }
                for n in old_path.iter().filter(|n| !new_path.contains(n)) {
                    js::events::user_mouse(vm, id(n), "mouseleave", client, new_t);
                }
                if let Some(t) = new_t {
                    js::events::user_mouse(vm, t, "mouseover", client, old_t);
                }
                for n in new_path.iter().rev().filter(|n| !old_path.contains(n)) {
                    js::events::user_mouse(vm, id(n), "mouseenter", client, old_t);
                }
                mutated = js::take_mutated();
            }
            self.apply_hover_chain();
            if mutated || self.stylesheets.iter().any(|s| s.contains(":hover")) {
                self.relayout();
                mutated = false;
            }
        }
        if let (Some(t), Some(js_engine)) = (&target, &mut self.js_engine) {
            js_engine.set_viewport(self.width as f32, self.height as f32, (self.scroll_x, self.scroll_y));
            js::events::user_mouse(&mut js_engine.vm, t.id().0, "mousemove", client, None);
            mutated |= js::take_mutated();
        }
        if mutated {
            self.relayout();
        }
        if let Some(url) = js::take_script_navigation() {
            self.pending_nav = Some(forms::OpenDocument { url, method: forms::HttpMethod::Get, body: None });
        }
        let cursor = match &target {
            Some(t) => css::cursor_for(t, hit.as_ref().is_some_and(|(_, text)| *text)),
            None => "default".to_string(),
        };
        if cursor != self.cursor {
            self.cursor = cursor.clone();
            self.pending_cursor = Some(cursor);
        }
    }

    /// Sets css_core's `:hover` state to the current hover path (the matcher reads it per element).
    fn apply_hover_chain(&self) {
        match self.hover_path.first() {
            Some(t) => {
                let ids: Vec<html_core::NodeId> = self.hover_path.iter().map(|n| n.id()).collect();
                t.with_doc(|doc, _| crate::dom::El::set_hover_chain(doc, &ids));
            }
            None => crate::dom::El::clear_hover_chain(),
        }
    }

    /// The cursor keyword (css-ui-4 §5.1; also the GTK/freedesktop cursor name) the hovered element
    /// asks for, when it changed since the last call. The shell raises it as `BrowserCursorChanged`.
    pub fn take_cursor_change(&mut self) -> Option<String> {
        self.pending_cursor.take()
    }

    /// The cursor keyword over the hovered element right now.
    pub fn cursor(&self) -> &str {
        &self.cursor
    }

    pub fn handle_event(&mut self, event: api::events::Event) {
        match event {
            api::events::Event::Scroll(_dx, dy) => {
                let old_sy = self.scroll_y;
                // Clamp to the document: [0, content height - viewport].
                let max_scroll = self
                    .layout_tree
                    .as_ref()
                    .and_then(|t| t.taffy.layout(t.root_node).ok().map(|l| l.size.height))
                    .map(|h| (h as f64 - self.height as f64).max(0.0))
                    .unwrap_or(f64::MAX);
                self.scroll_y = (self.scroll_y + dy).clamp(0.0, max_scroll);
                // Integer shift must equal the difference of the TRUNCATED
                // positions the renderer will actually paint at — deriving it
                // from the fractional delta lets retained pixels drift against
                // fresh paint under precise-trackpad deltas (duplicated lines
                // a few px apart).
                let idy = (self.scroll_y as i32) - (old_sy as i32);

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
                                    self.pending_nav = Some(doc_req);
                                }
                            }
                        }
                    }
                }
            }
            api::events::Event::MouseMove(x, y) => self.pointer_moved(x, y),
            api::events::Event::MouseDown(x, y) => self.selection_press(x, y),
            api::events::Event::MouseUp(x, y) => {
                self.pointer_released(); // AETHERSELECT (SR69): the drag ends before the click is routed
                if let Some(node) = self.hit_test(x, y) {
                    // Script click handlers run first (bubbling); a handled
                    // click still follows links, matching default behavior.
                    let mut canceled = false;
                    if let Some(js_engine) = &mut self.js_engine {
                        js_engine.set_viewport(self.width as f32, self.height as f32, (self.scroll_x, self.scroll_y));
                        js_engine.click(&node);
                        canceled = js::last_click_canceled();
                        if js::take_mutated() {
                            self.relayout();
                        }
                    }
                    if canceled {
                        return;
                    }
                    // A click on (or inside) a media element toggles it:
                    // play (PlayMedia toward Stria — the page's own stream,
                    // no site code) when not playing, MediaPause when it is.
                    media::toggle(&node);
                    // The hit is the DEEPEST box — usually the text run
                    // inside the link — so walk ancestors for the <a>.
                    let mut cur = Some(node.clone());
                    while let Some(n) = cur {
                        if let Some(el) = n.as_element() {
                            match el.name.local.as_ref() {
                                "a" => {
                                    if let Some(href) = el.attributes.borrow().get("href") {
                                        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
                                        let url = images::resolve(&base, href);
                                        self.pending_nav = Some(forms::OpenDocument {
                                            url,
                                            method: forms::HttpMethod::Get,
                                            body: None,
                                        });
                                    }
                                    break;
                                }
                                "input" | "textarea" | "select" => {
                                    self.focused_node = Some(n.clone());
                                    css::note_focus_modality(true); // AETHERGEN (SR71) fold: click focus is pointer-modal (:focus-visible stays off)
                                    break;
                                }
                                _ => {}
                            }
                        }
                        cur = n.parent();
                    }
                }
            }
        }
    }

    /// Builds a form submission from the focused control's enclosing <form>:
    /// collects named inputs, resolves the action against the current page.
    fn build_form_submission(&self, control: &crate::dom::NodeRef) -> Option<forms::OpenDocument> {
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
                let dirty = js::control_value(input.as_node()).or_else(|| {
                    (&*input.name.local == "select")
                        .then(|| layout::select_selected_option(input.as_node()).map(|(v, _)| v))
                        .flatten()
                });
                let attrs = input.attributes.borrow();
                let Some(name) = attrs.get("name") else { continue };
                let value = dirty.unwrap_or_else(|| attrs.get("value").unwrap_or("").to_string());
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

    /// The staged navigation from the last link click / form submit, if
    /// any. The shell consumes it and drives the async load.
    pub fn take_pending_nav(&mut self) -> Option<forms::OpenDocument> {
        self.pending_nav.take()
    }

    /// The staged play request from the last media-element click, if any:
    /// (url, title, mime). Removes that `PlayMedia` from the media outbox, so
    /// a shell uses either this or [`Self::take_media_requests`], not both.
    pub fn take_pending_media(&mut self) -> Option<(String, String, String)> {
        media::take_play_request()
    }

    /// Media the current page references: each `<video>`/`<audio>` element's
    /// selected source (see `media::source_for`). Returns (absolute src, mime).
    pub fn media_sources(&self) -> Vec<(String, String)> {
        let Some(doc) = &self.document else { return Vec::new() };
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        let Ok(found) = doc.select("video, audio") else { return Vec::new() };
        found.filter_map(|el| media::source_for(el.as_node(), &base)).collect()
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
        self.load_impl(
            &page.base_url,
            &page.html,
            &page.sheets,
            Some((&page.scripts, &page.script_urls)),
            add_history,
        );
    }

    fn load_impl(
        &mut self,
        url: &str,
        html: &str,
        external_css: &[String],
        scripts_override: Option<(&[String], &[String])>,
        add_history: bool,
    ) {
        if add_history {
            // Drop any forward entries beyond the current position, then
            // append. (Truncating AT the index wiped the current entry too —
            // history never grew past one, so Back never had anywhere to go.)
            if !self.history.is_empty() {
                self.history.truncate(self.history_idx + 1);
            }
            self.history.push(url.to_string());
            self.history_idx = self.history.len() - 1;
        }
        
        // The previous document's media stop; this one's register below.
        media::reset("Aether Browser");
        // Nothing of the new document is hovered until the pointer moves over it (SR67).
        self.hover_path.clear();
        crate::dom::El::clear_hover_chain();
        // A new document has no selection.
        selection::reset();
        self.sel_drag = None;
        self.button_down = false;

        // AETHERJS (SR63): the document is parsed with its scripts executing in HTML §4.12.1 order
        // (parser-blocking, defer, async, module; document.write during parse), then "the end"
        // (DOMContentLoaded, load) and the bounded boot drain — all before first layout.
        let mut pre = js::loader::Prefetched::default();
        if let Some((scripts, urls)) = scripts_override {
            for (text, url) in scripts.iter().zip(urls.iter()) {
                if !url.is_empty() {
                    pre.scripts.insert(url.clone(), text.clone());
                }
            }
        }
        let (document, mut js_engine) =
            js::loader::load(url, html, pre, (self.width as f32, self.height as f32), external_css.to_vec());
        // AETHERSELECT: `getSelection()` answers with the live Selection (window.rs installs a stub).
        js::dom::selection::bind_get_selection(&mut js_engine.vm);
        self.title = "Aether Browser".to_string();
        if let Ok(mut titles) = document.select("title") {
            if let Some(title_node) = titles.next() {
                self.title = title_node.as_node().text_contents();
            }
        }

        media::set_title(&self.title);

        // UNAOS_JSEVAL=<expr>: evaluate one expression against the booted page and print its value.
        if let Ok(expr) = std::env::var("UNAOS_JSEVAL") {
            match js_engine.eval_string(&expr) {
                Ok(v) => eprintln!("[jseval] {}", v),
                Err(e) => eprintln!("[jseval] ERR: {}", e),
            }
        }
        if let Some(nav) = js::take_script_navigation() {
            self.pending_nav = Some(forms::OpenDocument { url: nav, method: forms::HttpMethod::Get, body: None });
        }
        js::take_mutated();

        let mut sheets: Vec<String> = Vec::new();
        if let Ok(styles) = document.select("style") {
            for style_node in styles {
                sheets.push(style_node.as_node().text_contents());
            }
        }
        sheets.extend(external_css.iter().cloned());

        // Register the page's <video>/<audio> elements: each video's poster request
        // (MediaPoster) queues now, so its first frame is on the way as layout lands.
        media::scan(&document, url);

        let mut layout_tree = layout::build_tree(&document, self.width as f32, self.height as f32);
        css::apply_stylesheets(&mut layout_tree, &sheets);
        self.stylesheets = sheets;
        
        self.document = Some(document);
        self.layout_tree = Some(layout_tree);
        self.layout_epoch += 1;
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
            if !damages.is_empty() {
                self.painted_epoch = self.layout_epoch;
            }
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

// =================================================================================================
// AETHERHOVER (SR67): the paint-order hit test behind `AetherEngine::hit_test_at`.
// =================================================================================================

/// Document-space rect (x0, y0, x1, y1).
type HitRect = (f32, f32, f32, f32);

struct HitCtx<'a> {
    layout: &'a layout::LayoutTree,
    /// The pointer in document coordinates.
    px: f32,
    py: f32,
    /// The scroll offset render paints at (truncated), and the viewport (fixed boxes resolve against it).
    sx: f32,
    sy: f32,
    vw: f32,
    vh: f32,
}

impl HitCtx<'_> {
    fn inside(&self, r: HitRect, clip: HitRect) -> bool {
        let (x, y) = (self.px, self.py);
        x >= r.0.max(clip.0) && x < r.2.min(clip.2) && y >= r.1.max(clip.1) && y < r.3.min(clip.3)
    }
}

/// One flow item of a stacking context, in paint order.
enum HitEntry {
    /// A box's border box (and the clip its ancestors impose).
    Box(taffy::prelude::NodeId, HitRect, HitRect),
    /// An inline formatting context: its glyph runs and inline-box fragments, from its content origin.
    Ifc(taffy::prelude::NodeId, (f32, f32), HitRect),
}

/// A layer of a stacking context: hit as its own context (its subtree whole).
struct HitLayer {
    z: i32,
    node: taffy::prelude::NodeId,
    parent_origin: (f32, f32),
    clip: HitRect,
}

fn hit_skipped(layout: &layout::LayoutTree, id: taffy::prelude::NodeId) -> bool {
    layout.paint_map.get(&id).and_then(|p| p.hidden).unwrap_or(false)
        || layout.taffy.style(id).map(|s| s.display == taffy::style::Display::None).unwrap_or(false)
}

/// Painted as a layer of its stacking context (render's `is_layered`).
fn hit_layered(layout: &layout::LayoutTree, id: taffy::prelude::NodeId) -> bool {
    layout.paint_map.get(&id).and_then(|p| p.position_kind).is_some_and(|k| k != 0)
        || render::is_stacking_context(layout, id)
}

/// `id`'s border-box origin given its parent's: `location` from the parent, or for `position: fixed`
/// the viewport (render's rule: insets against the viewport, scroll ignored).
fn hit_origin(cx: &HitCtx, id: taffy::prelude::NodeId, parent: (f32, f32)) -> (f32, f32) {
    let layout = cx.layout;
    let Ok(l) = layout.taffy.layout(id) else { return parent };
    let (mut x, mut y) = (parent.0 + l.location.x, parent.1 + l.location.y);
    if layout.paint_map.get(&id).and_then(|p| p.position_kind) == Some(3) {
        if let Ok(st) = layout.taffy.style(id) {
            let len = |v: taffy::style::LengthPercentageAuto, basis: f32| -> Option<f32> {
                let raw = v.into_raw();
                match raw.tag() {
                    taffy::style::CompactLength::LENGTH_TAG => Some(raw.value()),
                    taffy::style::CompactLength::PERCENT_TAG => Some(raw.value() * basis),
                    _ => None,
                }
            };
            let (w, h) = (l.size.width, l.size.height);
            if let Some(v) = len(st.inset.left, cx.vw) {
                x = cx.sx + v;
            } else if let Some(v) = len(st.inset.right, cx.vw) {
                x = cx.sx + cx.vw - v - w;
            }
            if let Some(v) = len(st.inset.top, cx.vh) {
                y = cx.sy + v;
            } else if let Some(v) = len(st.inset.bottom, cx.vh) {
                y = cx.sy + cx.vh - v - h;
            }
        }
    }
    (x, y)
}

/// The children of `node` (border box at `o`) with the parent origin each is placed from — render's
/// `collect_layers` walk: an IFC root's atomic inlines and floats from their fragments, its
/// absolute/fixed children from taffy; any other box's taffy children.
fn hit_kids(layout: &layout::LayoutTree, node: taffy::prelude::NodeId, o: (f32, f32)) -> Vec<(taffy::prelude::NodeId, (f32, f32))> {
    let mut kids = Vec::new();
    let Ok(lb) = layout.taffy.layout(node) else { return kids };
    if let Some(il) = layout.inline.get(&node) {
        let (cx, cy) = (o.0 + lb.border.left + lb.padding.left, o.1 + lb.border.top + lb.padding.top);
        for f in &il.frags {
            if let layout::inline::Frag::Atomic { node: a, x, y } = f {
                let loc = layout.taffy.layout(*a).map(|l| l.location).unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
                kids.push((*a, (cx + x - loc.x, cy + y - loc.y)));
            }
        }
        for c in layout.taffy.children(node).unwrap_or_default() {
            if matches!(layout.paint_map.get(&c).and_then(|p| p.position_kind), Some(2 | 3)) {
                kids.push((c, o));
            }
        }
    } else {
        for c in layout.taffy.children(node).unwrap_or_default() {
            kids.push((c, o));
        }
    }
    kids
}

fn hit_box_entries(cx: &HitCtx, id: taffy::prelude::NodeId, o: (f32, f32), clip: HitRect, out: &mut Vec<HitEntry>) -> HitRect {
    let layout = cx.layout;
    let Ok(l) = layout.taffy.layout(id) else { return clip };
    let r = (o.0, o.1, o.0 + l.size.width, o.1 + l.size.height);
    out.push(HitEntry::Box(id, r, clip));
    let inner = if layout.paint_map.get(&id).and_then(|p| p.clip).unwrap_or(false) {
        (clip.0.max(r.0), clip.1.max(r.1), clip.2.min(r.2), clip.3.min(r.3))
    } else {
        clip
    };
    if layout.inline.contains_key(&id) {
        let content = (o.0 + l.border.left + l.padding.left, o.1 + l.border.top + l.padding.top);
        out.push(HitEntry::Ifc(id, content, inner));
    }
    inner
}

/// The normal-flow entries under `node` (paint order) and the layers it hoists.
fn hit_collect(cx: &HitCtx, node: taffy::prelude::NodeId, o: (f32, f32), clip: HitRect, out: &mut Vec<HitEntry>, layers: &mut Vec<HitLayer>) {
    for (c, po) in hit_kids(cx.layout, node, o) {
        if hit_skipped(cx.layout, c) {
            continue;
        }
        if hit_layered(cx.layout, c) {
            let z = cx.layout.paint_map.get(&c).and_then(|p| p.z_index).flatten().unwrap_or(0);
            layers.push(HitLayer { z, node: c, parent_origin: po, clip });
            continue;
        }
        let co = hit_origin(cx, c, po);
        let inner = hit_box_entries(cx, c, co, clip, out);
        hit_collect(cx, c, co, inner, out, layers);
    }
}

/// Hits one inline formatting context: a glyph run (its text node; `true`), else the deepest inline
/// box fragment under the point.
fn hit_ifc(cx: &HitCtx, root: taffy::prelude::NodeId, content: (f32, f32), clip: HitRect) -> Option<(taffy::prelude::NodeId, bool)> {
    let layout = cx.layout;
    let il = layout.inline.get(&root)?;
    for f in il.frags.iter().rev() {
        if let layout::inline::Frag::Text { node, x, baseline, width, .. } = f {
            let Some(&(top, h, _)) = il.lines.iter().find(|(t, h, _)| *baseline >= *t && *baseline <= *t + *h) else { continue };
            let r = (content.0 + x, content.1 + top, content.0 + x + width, content.1 + top + h);
            if cx.inside(r, clip) {
                return Some((*node, true));
            }
        }
    }
    let depth = |mut n: taffy::prelude::NodeId| {
        let mut d = 0;
        while let Some(p) = layout.taffy.parent(n) {
            d += 1;
            n = p;
        }
        d
    };
    let mut best: Option<(usize, taffy::prelude::NodeId)> = None;
    for f in &il.frags {
        if let layout::inline::Frag::Box { node, x, y, w, h, .. } = f {
            let r = (content.0 + x, content.1 + y, content.0 + x + w, content.1 + y + h);
            if cx.inside(r, clip) && !hit_skipped(layout, *node) {
                let d = depth(*node);
                if best.is_none_or(|(bd, _)| d >= bd) {
                    best = Some((d, *node));
                }
            }
        }
    }
    best.map(|(_, n)| (n, false))
}

/// Hits the stacking context (or layer) rooted at `node`, placed from `parent` under `clip`:
/// z >= 0 layers topmost first, then the flow from the last painted, then z < 0 layers, then the
/// root's own box.
fn hit_context(cx: &HitCtx, node: taffy::prelude::NodeId, parent: (f32, f32), clip: HitRect) -> Option<(taffy::prelude::NodeId, bool)> {
    if hit_skipped(cx.layout, node) {
        return None;
    }
    let o = hit_origin(cx, node, parent);
    let mut entries = Vec::new();
    let mut layers = Vec::new();
    let inner = hit_box_entries(cx, node, o, clip, &mut entries);
    hit_collect(cx, node, o, inner, &mut entries, &mut layers);
    layers.sort_by_key(|l| l.z); // stable: equal z keeps tree order
    let hit_entry = |e: &HitEntry| match *e {
        HitEntry::Box(id, r, c) => cx.inside(r, c).then_some((id, false)),
        HitEntry::Ifc(id, content, c) => hit_ifc(cx, id, content, c),
    };
    for l in layers.iter().rev().filter(|l| l.z >= 0) {
        if let Some(h) = hit_context(cx, l.node, l.parent_origin, l.clip) {
            return Some(h);
        }
    }
    for e in entries.iter().skip(1).rev() {
        if let Some(h) = hit_entry(e) {
            return Some(h);
        }
    }
    for l in layers.iter().rev().filter(|l| l.z < 0) {
        if let Some(h) = hit_context(cx, l.node, l.parent_origin, l.clip) {
            return Some(h);
        }
    }
    entries.first().and_then(hit_entry)
}
