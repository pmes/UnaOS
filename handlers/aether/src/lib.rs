
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
    /// AETHERINPUT (SR68): the focused text control's caret and selection anchor (byte offsets into its
    /// value; see `forms::edit`).
    pub edit: forms::edit::EditState,
    /// When the caret last moved or the value changed: the blink phase counts from here (it restarts
    /// visible on every edit, as Chromium's does).
    caret_epoch: std::time::Instant,
    /// The blink phase the last frame painted.
    caret_on: bool,
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
        self.needs_repaint = true;
        self.damage_rects.push((0, 0, self.width, self.height));
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
    
    fn hit_test(&self, x: f64, y: f64) -> Option<crate::dom::NodeRef> {
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
            hit: &mut Option<crate::dom::NodeRef>
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
            api::events::Event::MouseMove(_x, _y) => {}
            api::events::Event::MouseDown(_x, _y) => {}
            api::events::Event::MouseUp(x, y) => {
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

