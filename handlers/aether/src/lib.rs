
pub mod net;
pub mod dom;
pub mod layout;
pub mod render;
pub mod js;
pub mod images;
pub mod canvas;
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
    /// AETHERINPUT (SR68): the focused text control's caret and selection anchor (byte offsets into its
    /// value; see `forms::edit`).
    pub edit: forms::edit::EditState,
    /// When the caret last moved or the value changed: the blink phase counts from here (it restarts
    /// visible on every edit, as Chromium's does).
    caret_epoch: std::time::Instant,
    /// The blink phase the last frame painted.
    caret_on: bool,
}

/// A selection drag (AETHERSELECT).
#[derive(Clone, Copy, Debug)]
struct SelDrag {
    granularity: u8,
    lo: selection::Point,
    hi: selection::Point,
}

/// Chromium's caret blink half-period (`LayoutTheme::CaretBlinkInterval`, 500 ms).
const CARET_BLINK_MS: u128 = 500;
/// Chromium's keyboard scroll steps: a line is 40 px (`kPixelsPerLineStep`), a page 87.5 % of the
/// viewport (`kMinFractionToStepWhenPaging`).
const LINE_STEP: f64 = 40.0;
const PAGE_FRACTION: f64 = 0.875;

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
            edit: forms::edit::EditState::default(),
            caret_epoch: std::time::Instant::now(),
            caret_on: true,
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
            self.pending_nav = Some(forms::OpenDocument::get(url));
        }
        if js::take_mutated() {
            self.relayout();
        }
        self.sel_changed();
        // The focused field's caret blinks: repaint its box when the phase flips.
        if let Some(node) = self.editing_node() {
            let on = (self.caret_epoch.elapsed().as_millis() / CARET_BLINK_MS) % 2 == 0;
            if on != self.caret_on {
                self.caret_on = on;
                self.damage_node(&node);
            }
        }

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
            self.pending_nav = Some(forms::OpenDocument { url, method: forms::HttpMethod::Get, body: None, content_type: None });
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
                // Typed characters land at the caret of the focused text control (input or textarea
                // alike), replacing the selection.
                self.insert_text(&text);
            }
            api::events::Event::KeyDown(key) => {
                // A focused text control takes its editing keys first (forms::edit); the page only
                // scrolls by keyboard when no field holds the keys.
                if let Some(node) = self.editing_node() {
                    let multiline = Self::is_textarea(&node);
                    let mut value = forms::control_value(&node);
                    let before = value.clone();
                    match self.edit.key(&mut value, &key, multiline) {
                        forms::edit::Outcome::Edited => {
                            if value != before {
                                js::set_control_value(&node, value);
                                self.after_edit(&node, true);
                            } else {
                                self.after_edit(&node, false);
                            }
                            return;
                        }
                        forms::edit::Outcome::Moved | forms::edit::Outcome::Consumed => {
                            self.after_edit(&node, false);
                            return;
                        }
                        forms::edit::Outcome::Submit => {
                            self.implicit_submission(&node);
                            return;
                        }
                        forms::edit::Outcome::Unhandled => {}
                    }
                } else if let Some(node) = self.focused_node.clone() {
                    // A focused checkbox, radio or button: Space (and Return on a button) activates it.
                    let (_, k) = forms::edit::parse_key(&key);
                    let checkable = matches!(
                        layout::control_kind(&node),
                        Some(layout::Control::Checkbox | layout::Control::Radio)
                    );
                    let button = layout::control_kind(&node) == Some(layout::Control::Button);
                    if (k == Some(forms::edit::Key::Space) && (checkable || button))
                        || (k == Some(forms::edit::Key::Return) && button)
                    {
                        self.activate(&node, (0, 0));
                        return;
                    }
                }
                self.key_scroll(&key);
            }
            api::events::Event::MouseMove(x, y) => self.pointer_moved(x, y),
            api::events::Event::MouseDown(x, y) => self.selection_press(x, y),
            api::events::Event::MouseUp(x, y) => {
                self.pointer_released(); // AETHERSELECT (SR69): the drag ends before the click is routed
                css::note_focus_modality(true); // AETHERGEN (SR71): click focus is pointer-modal
                let Some(node) = self.hit_test(x, y) else {
                    self.set_focus(None);
                    return;
                };
                // Script click handlers run first (bubbling); a handled
                // click still follows links, matching default behavior.
                // The click's activation behavior runs here too: a checkbox
                // toggles (a radio checks, unchecking its group) before the
                // listeners see it, reverted if one cancels.
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
                // inside the link or the button — so walk ancestors for
                // the element whose default action the click runs.
                let mut cur = Some(node.clone());
                while let Some(n) = cur {
                    if let Some(el) = n.as_element() {
                        match el.name.local.as_ref() {
                            "a" => {
                                self.set_focus(None);
                                if let Some(href) = el.attributes.borrow().get("href") {
                                    let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
                                    let url = images::resolve(&base, href);
                                    self.pending_nav = Some(forms::OpenDocument::get(url));
                                }
                                return;
                            }
                            "input" | "textarea" | "select" | "button" => {
                                if forms::is_disabled(&n) {
                                    return;
                                }
                                let coords = self
                                    .box_of(&n)
                                    .map(|(bx, by, _, _)| {
                                        ((x + self.scroll_x - bx as f64).max(0.0) as i32, (y + self.scroll_y - by as f64).max(0.0) as i32)
                                    })
                                    .unwrap_or((0, 0));
                                self.set_focus(Some(n.clone()));
                                if forms::is_text_editable(&n) {
                                    // The caret goes where the click landed (the nearest grapheme
                                    // boundary on the painted run).
                                    let at = self.caret_from_point(&n, x, y);
                                    self.edit = forms::edit::EditState::at(at);
                                    self.after_edit(&n, false);
                                } else {
                                    self.control_clicked(&n, coords);
                                }
                                return;
                            }
                            "label" => {
                                // A label's activation behavior: the click goes on to its control.
                                if let Some(control) = Self::labeled_control(&n) {
                                    if forms::is_text_editable(&control) {
                                        self.set_focus(Some(control.clone()));
                                    } else if !forms::is_disabled(&control) {
                                        self.set_focus(Some(control.clone()));
                                        self.activate(&control, (0, 0));
                                    }
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                    cur = n.parent();
                }
                self.set_focus(None);
            }
        }
    }

    /// The focused control when it is a text control the user can type into.
    fn editing_node(&self) -> Option<crate::dom::NodeRef> {
        self.focused_node.clone().filter(forms::is_text_editable)
    }

    fn is_textarea(node: &crate::dom::NodeRef) -> bool {
        node.as_element().is_some_and(|e| &*e.name.local == "textarea")
    }

    /// Moves focus (None blurs). A newly focused text control gets a collapsed caret at the start of its
    /// value (Chromium restores a never-set selection to 0 on `focus()`/autofocus; a click then places it).
    fn set_focus(&mut self, node: Option<crate::dom::NodeRef>) {
        if self.focused_node == node {
            return;
        }
        if let Some(old) = self.focused_node.take() {
            self.damage_node(&old);
        }
        if let Some(n) = &node {
            self.edit = forms::edit::EditState::at(0);
            self.caret_epoch = std::time::Instant::now();
            self.caret_on = true;
            self.damage_node(n);
        }
        self.focused_node = node;
    }

    /// Marks `node`'s box damaged for the next frame.
    fn damage_node(&mut self, node: &crate::dom::NodeRef) {
        if let Some(r) = self.screen_rect_of(node) {
            self.damage_rects.push(r);
        }
        self.needs_repaint = true;
    }

    /// After a key or text changed the focused field (`changed`: its value): the caret restarts its
    /// blink visible, the page hears `input`, and the field repaints.
    fn after_edit(&mut self, node: &crate::dom::NodeRef, changed: bool) {
        self.caret_epoch = std::time::Instant::now();
        self.caret_on = true;
        if changed {
            if let Some(js_engine) = &mut self.js_engine {
                js_engine.fire(node, "input", true);
                if js::take_mutated() {
                    self.relayout();
                }
            }
        }
        self.damage_node(node);
    }

    /// Inserts `text` at the focused text control's caret (replacing its selection): line breaks are
    /// stripped in a single-line field and normalized to LF in a textarea; `maxlength` is honored.
    fn insert_text(&mut self, text: &str) {
        let Some(node) = self.editing_node() else { return };
        let multiline = Self::is_textarea(&node);
        let mut ins = forms::edit::sanitize_insert(text, multiline);
        let mut value = forms::control_value(&node);
        self.edit.clamp(&value);
        let max = node
            .as_element()
            .and_then(|e| e.attributes.borrow().get("maxlength").and_then(|m| m.trim().parse::<usize>().ok()));
        if let Some(max) = max {
            // maxlength counts UTF-16 code units (HTML §4.10.18.3); the selection is replaced.
            let (a, b) = self.edit.selection();
            let kept = value.encode_utf16().count() - value[a..b].encode_utf16().count();
            let room = max.saturating_sub(kept);
            let mut used = 0;
            let mut cut = 0;
            for (i, c) in ins.char_indices() {
                if used + c.len_utf16() > room {
                    break;
                }
                used += c.len_utf16();
                cut = i + c.len_utf8();
            }
            ins.truncate(cut);
        }
        if ins.is_empty() && !self.edit.has_selection() {
            return;
        }
        self.edit.insert(&mut value, &ins);
        js::set_control_value(&node, value);
        self.after_edit(&node, true);
    }

    /// Pastes `text` (the shell's `BrowserPaste`, from the platform clipboard) into the focused text
    /// control at its caret: the same insertion as typing, line breaks kept only in a textarea.
    pub fn paste(&mut self, text: &str) {
        self.insert_text(text);
    }

    /// The caret position (byte offset into the value) nearest to the viewport point (`x`, `y`) inside
    /// text control `node`: the grapheme boundary whose caret x is closest on the painted run (the
    /// `text_host::line::Line::hit` rule), on the logical line under `y` in a textarea.
    fn caret_from_point(&self, node: &crate::dom::NodeRef, x: f64, y: f64) -> usize {
        let value = forms::control_value(node);
        let Some((bx, by, _, _)) = self.box_of(node) else { return value.len() };
        let Some(adv) = self.control_advancer(node) else { return value.len() };
        let (pad_l, pad_t) = self.control_insets(node);
        let lx = (x + self.scroll_x) as f32 - bx - pad_l;
        let (start, end) = if Self::is_textarea(node) {
            let line_h = crate::fonts::face(&adv.sel).map(|f| crate::fonts::line_height(f, adv.size, 0.0)).unwrap_or(adv.size * 1.2);
            let row = (((y + self.scroll_y) as f32 - by - pad_t) / line_h).floor().max(0.0) as usize;
            let mut s = 0;
            for _ in 0..row {
                match value[s..].find('\n') {
                    Some(p) => s += p + 1,
                    None => break,
                }
            }
            (s, forms::edit::line_end(&value, s))
        } else {
            (0, value.len())
        };
        let password = forms::input_type(node) == "password" && !Self::is_textarea(node);
        let line = &value[start..end];
        let mut best = (f32::INFINITY, end);
        let bounds = font_core::grapheme::clusters(line).into_iter().map(|(a, _)| a).chain(std::iter::once(line.len()));
        for b in bounds {
            let prefix = if password { "\u{2022}".repeat(line[..b].chars().count()) } else { line[..b].to_string() };
            let d = (adv.str(&prefix) - lx).abs();
            if d < best.0 {
                best = (d, start + b);
            }
        }
        best.1
    }

    /// The Advancer the control's value is painted with (its computed font).
    fn control_advancer(&self, node: &crate::dom::NodeRef) -> Option<crate::fonts::lines::Advancer> {
        let layout = self.layout_tree.as_ref()?;
        let id = layout.node_map.iter().find(|(_, n)| *n == node).map(|(id, _)| *id)?;
        render::control_advancer(layout, id)
    }

    /// (left, top) inset of the control's content box: border plus padding.
    fn control_insets(&self, node: &crate::dom::NodeRef) -> (f32, f32) {
        let Some(layout) = self.layout_tree.as_ref() else { return (0.0, 0.0) };
        let Some(id) = layout.node_map.iter().find(|(_, n)| *n == node).map(|(id, _)| *id) else { return (0.0, 0.0) };
        layout
            .taffy
            .layout(id)
            .map(|l| (l.padding.left + l.border.left, l.padding.top + l.border.top))
            .unwrap_or((0.0, 0.0))
    }

    /// The control a `<label>` labels: the element its `for` names, else its first labelable descendant.
    fn labeled_control(label: &crate::dom::NodeRef) -> Option<crate::dom::NodeRef> {
        let labelable = |n: &crate::dom::NodeRef| {
            n.as_element().is_some_and(|e| {
                matches!(e.name.local.as_ref(), "button" | "select" | "textarea")
                    || (&*e.name.local == "input" && forms::input_type(n) != "hidden")
            })
        };
        let for_id = label.as_element().and_then(|e| e.attributes.borrow().get("for").map(str::to_string));
        match for_id {
            Some(id) => label
                .document_node()
                .descendants()
                .find(|n| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some(id.as_str())))
                .filter(labelable),
            None => label.descendants().find(labelable),
        }
    }

    /// Activation of a control by label, Space or Return: the click event (with its checkbox/radio
    /// pre-activation, which a listener may cancel), then the control's default action.
    fn activate(&mut self, node: &crate::dom::NodeRef, coords: (i32, i32)) {
        if let Some(js_engine) = &mut self.js_engine {
            js_engine.set_viewport(self.width as f32, self.height as f32, (self.scroll_x, self.scroll_y));
            js_engine.click(node);
            let canceled = js::last_click_canceled();
            if js::take_mutated() {
                self.relayout();
            }
            if canceled {
                return;
            }
        }
        self.control_clicked(node, coords);
    }

    /// A click's default action on a non-text control (the click event already ran): a submit button
    /// submits its form with itself as the submitter; a checkbox or radio repaints (its checkedness
    /// toggled in the click's pre-activation, radio groups going exclusive).
    fn control_clicked(&mut self, node: &crate::dom::NodeRef, coords: (i32, i32)) {
        if forms::is_submit_button(node) {
            self.submit_form(node, Some(node), coords);
            return;
        }
        if matches!(layout::control_kind(node), Some(layout::Control::Checkbox | layout::Control::Radio)) {
            // A radio's group mates may sit anywhere in the form: repaint the form.
            match forms::form_owner(node) {
                Some(f) => self.damage_node(&f),
                None => {
                    self.damage_rects.push((0, 0, self.width, self.height));
                    self.needs_repaint = true;
                }
            }
        }
    }

    /// Return in a single-line field (HTML §4.10.21.2 implicit submission): the form's default button
    /// (its first submit button) is clicked; with none, the form submits itself — unless more than one
    /// of its fields blocks implicit submission.
    fn implicit_submission(&mut self, field: &crate::dom::NodeRef) {
        let Some(form) = forms::form_owner(field) else { return };
        let controls = forms::form_controls(&form);
        if let Some(default) = controls.iter().find(|c| forms::is_submit_button(c)).cloned() {
            if !forms::is_disabled(&default) {
                self.activate(&default, (0, 0));
            }
            return;
        }
        let blocking = controls
            .iter()
            .filter(|c| {
                c.as_element().is_some_and(|e| &*e.name.local == "input")
                    && matches!(
                        forms::input_type(c).as_str(),
                        "text" | "search" | "url" | "tel" | "email" | "password" | "date" | "month" | "week" | "time"
                            | "datetime-local" | "number"
                    )
            })
            .count();
        if blocking <= 1 {
            self.submit_form(field, None, (0, 0));
        }
    }

    /// Submits `control`'s form (HTML §4.10.21.3): the cancelable `submit` event at the form, then the
    /// navigation `forms::build_form_submission` builds, staged for the shell.
    fn submit_form(&mut self, control: &crate::dom::NodeRef, submitter: Option<&crate::dom::NodeRef>, coords: (i32, i32)) {
        let Some(form) = forms::form_owner(submitter.unwrap_or(control)) else { return };
        if !self.fire_submit(&form) {
            return;
        }
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        if let Some(nav) = forms::build_form_submission(&base, control, submitter, coords) {
            self.pending_nav = Some(nav);
        }
    }

    /// Fires a bubbling, cancelable `submit` at `form`; false when a listener canceled it (a scripted
    /// form handles its own submission). The script engine's `fire` reports no cancelation, so the
    /// event is dispatched from script: the form is tagged for one lookup and the tag removed before
    /// any listener runs.
    fn fire_submit(&mut self, form: &crate::dom::NodeRef) -> bool {
        let Some(js_engine) = &mut self.js_engine else { return true };
        let Some(el) = form.as_element() else { return true };
        el.attributes.borrow_mut().insert("data-aether-submitting", "1".to_string());
        let r = js_engine.eval_string(
            "(function(){var f=document.querySelector('form[data-aether-submitting]');if(!f)return true;\
             f.removeAttribute('data-aether-submitting');\
             var e=typeof SubmitEvent==='function'?new SubmitEvent('submit',{bubbles:true,cancelable:true}):new Event('submit',{bubbles:true,cancelable:true});\
             return f.dispatchEvent(e);})()",
        );
        el.attributes.borrow_mut().remove("data-aether-submitting");
        if js::take_mutated() {
            self.relayout();
        }
        !matches!(r.as_deref(), Ok("false"))
    }

    /// Keyboard page scrolling (no field holds the keys): Space / Shift+Space and Page Down / Page Up
    /// move a page (87.5 % of the viewport), Down / Up a 40 px line, Home / End to the top / bottom —
    /// Chromium's steps, in the engine so every shell gets them.
    fn key_scroll(&mut self, key: &str) {
        use forms::edit::Key;
        let (m, Some(k)) = forms::edit::parse_key(key) else { return };
        if m.alt || m.meta {
            return;
        }
        let page = (self.height as f64 * PAGE_FRACTION).max(1.0);
        let dy = match k {
            Key::Space if m.ctrl => return,
            Key::Space if m.shift => -page,
            Key::Space | Key::PageDown => page,
            Key::PageUp => -page,
            Key::Down => LINE_STEP,
            Key::Up => -LINE_STEP,
            Key::Home => -self.scroll_y,
            Key::End => f64::MAX / 4.0,
            _ => return,
        };
        self.handle_event(api::events::Event::Scroll(0.0, dy));
    }

    /// Builds the submission of `control`'s form with no submitter (HTML §4.10.21.3 through
    /// `forms::build_form_submission`), against the current page URL.
    pub fn build_form_submission(&self, control: &crate::dom::NodeRef) -> Option<forms::OpenDocument> {
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        forms::build_form_submission(&base, control, None, (0, 0))
    }

    /// The submission a click on submit button `submitter` performs (its name/value in the entry list,
    /// its `formaction`/`formmethod`/`formenctype` in force) — what the click stages, without the
    /// `submit` event.
    pub fn submission_for(&self, submitter: &crate::dom::NodeRef) -> Option<forms::OpenDocument> {
        let base = self.history.get(self.history_idx).cloned().unwrap_or_default();
        forms::build_form_submission(&base, submitter, Some(submitter), (0, 0))
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
            self.pending_nav = Some(forms::OpenDocument::get(nav));
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
        // Focus never survives a navigation; the new document's first `autofocus` control that can
        // take focus gets it (HTML §6.6.8 "flush autofocus candidates").
        self.focused_node = None;
        let autofocus = self.document.as_ref().and_then(|d| {
            d.descendants().find(|n| {
                n.as_element().is_some_and(|e| {
                    matches!(e.name.local.as_ref(), "input" | "textarea" | "select" | "button")
                        && e.attributes.borrow().get("autofocus").is_some()
                }) && !forms::is_disabled(n)
                    && forms::input_type(n) != "hidden"
            })
        });
        self.set_focus(autofocus);
    }

    /// Test hook: force the caret's blink phase.
    #[cfg(test)]
    pub(crate) fn caret_on_for_test(&mut self, on: bool) {
        self.caret_on = on;
    }

    /// The paint-side view of the focused text control: caret, selection anchor and blink phase.
    fn focus_paint(&self) -> Option<render::FocusPaint> {
        let node = self.editing_node()?;
        let mut e = self.edit;
        e.clamp(&forms::control_value(&node));
        Some(render::FocusPaint { node, caret: e.caret, anchor: e.anchor, caret_visible: self.caret_on })
    }

    pub fn render_frame(&mut self) -> Vec<(u32, u32, u32, u32)> {
        let damages = std::mem::take(&mut self.damage_rects);
        render::set_focus(self.focus_paint());
        
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
