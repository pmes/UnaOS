//! QUARTZFONT (LEDGER SR64): GTK widgets whose text is quartzite's own — measured with `font_core`'s
//! `shape_fallback` and painted by its Skia-mode rasterizer through `text_host` — instead of Pango's.
//!
//! - [`GlyphLabel`] (CSS node `label`): a single line, centred in its allocation; the Tetra `Button`'s child.
//! - [`GlyphEntry`] (CSS node `entry`, so the theme's frame, background and focus ring apply): a one-line
//!   editor — an input-method context for every committed string, grapheme-cluster caret moves, click and
//!   drag selection, Home/End, Backspace/Delete, Ctrl+A/C/X/V through the display clipboard, horizontal
//!   scrolling that keeps the caret in view, a placeholder, and `activate` on Enter.
//!
//! Both take the desktop's UI font from `gtk-font-name` (a Pango description; [`crate::text::parse_font_name`])
//! at `gtk-xft-dpi`, ink from the widget's CSS `color`, and render at the surface's scale factor.
//!
//! `QUARTZFONT_TRACE=1` prints each painted string's face, size and pen origin in window coordinates
//! (`[quartzfont] ...` on stderr): the xvfb-smoke oracle builds its Chromium reference from those lines.

use crate::text::{parse_font_name, ui_style, Line, PixelOrder, TextStyle};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};

/// The desktop UI font for `w`: `gtk-font-name` at `gtk-xft-dpi` (96 when unset), else [`ui_style`].
pub fn style_for(w: &impl IsA<gtk4::Widget>) -> TextStyle {
    let settings = w.as_ref().settings();
    let dpi = match settings.gtk_xft_dpi() {
        v if v > 0 => v as f32 / 1024.0,
        _ => 96.0,
    };
    settings.gtk_font_name().and_then(|n| parse_font_name(n.as_str(), dpi)).unwrap_or_else(ui_style)
}

fn rgba8(c: &gdk::RGBA, alpha: f32) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c.red()), q(c.green()), q(c.blue()), q(c.alpha() * alpha)]
}

/// A device-pixel canvas for one widget's text.
struct Canvas {
    scale: f32,
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Canvas {
    fn new(widget: &gtk4::Widget) -> Option<Canvas> {
        let scale = widget.scale_factor().max(1) as f32;
        let (w, h) = ((widget.width() as f32 * scale) as u32, (widget.height() as f32 * scale) as u32);
        (w > 0 && h > 0).then(|| Canvas { scale, w, h, px: vec![0; (w * h * 4) as usize] })
    }

    /// Paints `line` (shaped at device size) with its pen at logical (`x`, `baseline`).
    fn paint(&mut self, line: &Line, x: f32, baseline: f32, ink: [u8; 4], clip: (f32, f32)) {
        let s = self.scale;
        let stride = self.w as usize * 4;
        let clip = ((clip.0 * s).floor() as i32, (clip.1 * s).ceil() as i32);
        line.paint(&mut self.px, self.w, self.h, stride, PixelOrder::Rgba, x * s, baseline * s, ink, clip);
    }

    fn fill(&mut self, x0: f32, x1: f32, y0: f32, y1: f32, rgba: [u8; 4]) {
        let s = self.scale;
        let (x0, x1) = (((x0 * s).round().max(0.0)) as u32, ((x1 * s).round().max(0.0) as u32).min(self.w));
        let (y0, y1) = (((y0 * s).round().max(0.0)) as u32, ((y1 * s).round().max(0.0) as u32).min(self.h));
        let a = rgba[3] as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * self.w + x) * 4) as usize;
                let p = &mut self.px[i..i + 4];
                for k in 0..3 {
                    p[k] = ((rgba[k] as u32 * a + p[k] as u32 * (255 - a) + 127) / 255) as u8;
                }
                p[3] = ((a * 255 + p[3] as u32 * (255 - a) + 127) / 255) as u8;
            }
        }
    }

    fn append(self, widget: &gtk4::Widget, snapshot: &gtk4::Snapshot) {
        let bytes = glib::Bytes::from_owned(self.px);
        let tex = gdk::MemoryTexture::new(
            self.w as i32,
            self.h as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &bytes,
            self.w as usize * 4,
        );
        snapshot.append_texture(&tex, &graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32));
    }
}

/// The line's baseline in a box `h` tall: the glyph area (ascent + descent) centred, whole px.
fn baseline_in(line: &Line, scale: f32, h: f32) -> f32 {
    let (a, d) = (line.ascent / scale, line.descent / scale);
    ((h - (a + d)) / 2.0).floor() + a
}

fn trace(widget: &gtk4::Widget, text: &str, line: &Line, style: &TextStyle, x: f32, baseline: f32, ink: [u8; 4]) {
    if std::env::var_os("QUARTZFONT_TRACE").is_none() || text.is_empty() {
        return;
    }
    let Some(root) = widget.root() else { return };
    let Some(o) = widget.compute_point(&root, &graphene::Point::new(0.0, 0.0)) else { return };
    let face = line.glyphs.first().map(|g| g.face);
    let db = text_host::db::db();
    let file = face
        .and_then(|f| db.select(&f.family, f.style).map(|(i, _, _)| (db.faces[i].path.display().to_string(), db.faces[i].index)))
        .unwrap_or_default();
    let esc: String = text.chars().flat_map(|c| if c == '\t' { vec!['\\', 't'] } else { vec![c] }).collect();
    let s = widget.scale_factor().max(1) as f32;
    // each glyph's pen and advance in logical px from the string's pen origin (the oracle's glyph boxes)
    let pens: Vec<String> = line.glyphs.iter().map(|g| format!("{:.4}:{:.4}", g.pen / s, g.adv / s)).collect();
    eprintln!(
        "[quartzfont]\t{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t#{:02x}{:02x}{:02x}\t{}\t{}\t{}\t{}",
        esc,
        file.0,
        file.1,
        style.size,
        o.x() + x,
        o.y() + baseline,
        ink[0],
        ink[1],
        ink[2],
        ink[3],
        line.ascent / s,
        line.descent / s,
        pens.join(",")
    );
}

fn scaled(style: &TextStyle, s: f32) -> TextStyle {
    TextStyle { size: style.size * s, ..style.clone() }
}

mod imp_label {
    use super::*;

    #[derive(Default)]
    pub struct GlyphLabel {
        pub text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlyphLabel {
        const NAME: &'static str = "QuartziteGlyphLabel";
        type Type = super::GlyphLabel;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("label");
        }
    }

    impl ObjectImpl for GlyphLabel {}

    impl WidgetImpl for GlyphLabel {
        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let style = style_for(&*self.obj());
            let line = style.shape(&self.text.borrow());
            match orientation {
                gtk4::Orientation::Horizontal => {
                    let w = line.width.ceil() as i32;
                    (w, w, -1, -1)
                }
                _ => {
                    let h = line.height() as i32;
                    (h, h, line.ascent as i32, line.ascent as i32)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let widget = obj.upcast_ref::<gtk4::Widget>();
            let Some(mut canvas) = Canvas::new(widget) else { return };
            let style = style_for(widget);
            let text = self.text.borrow();
            let line = scaled(&style, canvas.scale).shape(&text);
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            // centred like a GtkLabel with xalign 0.5 in a wider allocation (whole px)
            let x = ((w - line.width / canvas.scale) / 2.0).floor().max(0.0);
            let baseline = baseline_in(&line, canvas.scale, h);
            let ink = rgba8(&widget.color(), 1.0);
            trace(widget, &text, &line, &style, x, baseline, ink);
            canvas.paint(&line, x, baseline, ink, (0.0, w));
            canvas.append(widget, snapshot);
        }
    }
}

glib::wrapper! {
    /// A one-line label drawn with quartzite's text.
    pub struct GlyphLabel(ObjectSubclass<imp_label::GlyphLabel>) @extends gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl GlyphLabel {
    pub fn new(text: &str) -> Self {
        let l: Self = glib::Object::new();
        l.set_text(text);
        l
    }
    pub fn set_text(&self, text: &str) {
        *self.imp().text.borrow_mut() = text.to_string();
        self.queue_resize();
    }
    pub fn text(&self) -> String {
        self.imp().text.borrow().clone()
    }
}

mod imp_entry {
    use super::*;

    #[derive(Default)]
    pub struct GlyphEntry {
        pub text: RefCell<String>,
        pub placeholder: RefCell<String>,
        /// Caret and selection anchor, byte offsets on grapheme boundaries.
        pub caret: Cell<usize>,
        pub anchor: Cell<usize>,
        /// Horizontal scroll in logical px.
        pub scroll: Cell<f32>,
        pub focused: Cell<bool>,
        pub drag_from: Cell<f64>,
        pub im: RefCell<Option<gtk4::IMMulticontext>>,
        pub on_activate: RefCell<Vec<Box<dyn Fn(&super::GlyphEntry)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlyphEntry {
        const NAME: &'static str = "QuartziteGlyphEntry";
        type Type = super::GlyphEntry;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("entry");
        }
    }

    impl ObjectImpl for GlyphEntry {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_can_focus(true);
            obj.set_cursor_from_name(Some("text"));

            let im = gtk4::IMMulticontext::new();
            im.set_client_widget(Some(&*obj));
            let weak = obj.downgrade();
            im.connect_commit(move |_, s| {
                if let Some(e) = weak.upgrade() {
                    e.insert(s);
                }
            });
            let key = gtk4::EventControllerKey::new();
            key.set_im_context(Some(&im));
            let weak = obj.downgrade();
            key.connect_key_pressed(move |_, keyval, _code, mods| {
                weak.upgrade().map_or(glib::Propagation::Proceed, |e| e.key(keyval, mods))
            });
            obj.add_controller(key);

            let focus = gtk4::EventControllerFocus::new();
            let (w1, w2) = (obj.downgrade(), obj.downgrade());
            let (im1, im2) = (im.clone(), im.clone());
            focus.connect_enter(move |_| {
                if let Some(e) = w1.upgrade() {
                    e.imp().focused.set(true);
                    im1.focus_in();
                    e.queue_draw();
                }
            });
            focus.connect_leave(move |_| {
                if let Some(e) = w2.upgrade() {
                    e.imp().focused.set(false);
                    im2.focus_out();
                    e.queue_draw();
                }
            });
            obj.add_controller(focus);

            let drag = gtk4::GestureDrag::new();
            let weak = obj.downgrade();
            drag.connect_drag_begin(move |g, x, _| {
                let Some(e) = weak.upgrade() else { return };
                e.grab_focus();
                let shift = g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
                let at = e.hit(x as f32);
                e.imp().drag_from.set(x);
                e.set_caret(at, shift);
            });
            let weak = obj.downgrade();
            drag.connect_drag_update(move |_, dx, _| {
                let Some(e) = weak.upgrade() else { return };
                let at = e.hit((e.imp().drag_from.get() + dx) as f32);
                e.set_caret(at, true);
            });
            obj.add_controller(drag);

            *self.im.borrow_mut() = Some(im);
        }
    }

    impl WidgetImpl for GlyphEntry {
        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let style = style_for(&*self.obj());
            let (a, d, g) = style.metrics();
            match orientation {
                gtk4::Orientation::Horizontal => {
                    let w = (style.size * 4.0).ceil() as i32;
                    (w, (style.size * 12.0).ceil() as i32, -1, -1)
                }
                _ => {
                    let h = (a + d + g) as i32;
                    (h, h, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let widget = obj.upcast_ref::<gtk4::Widget>();
            let Some(mut canvas) = Canvas::new(widget) else { return };
            let style = style_for(widget);
            let s = canvas.scale;
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let color = widget.color();
            let text = self.text.borrow().clone();
            let dev = scaled(&style, s);
            if text.is_empty() {
                let ph = self.placeholder.borrow().clone();
                let line = dev.shape(&ph);
                let baseline = baseline_in(&line, s, h);
                let ink = rgba8(&color, 0.5);
                trace(widget, &ph, &line, &style, 0.0, baseline, ink);
                canvas.paint(&line, 0.0, baseline, ink, (0.0, w));
                if self.focused.get() {
                    canvas.fill(0.0, 1.0, baseline - line.ascent / s, baseline + line.descent / s, rgba8(&color, 1.0));
                }
                canvas.append(widget, snapshot);
                return;
            }
            let line = dev.shape(&text);
            let caret_x = line.caret_x(&text, self.caret.get()) / s;
            // keep the caret inside the box
            let mut scroll = self.scroll.get().clamp(0.0, (line.width / s - w + 1.0).max(0.0));
            if caret_x - scroll > w - 1.0 {
                scroll = caret_x - w + 1.0;
            }
            if caret_x - scroll < 0.0 {
                scroll = caret_x;
            }
            self.scroll.set(scroll);
            let baseline = baseline_in(&line, s, h);
            let (top, bottom) = (baseline - line.ascent / s, baseline + line.descent / s);
            let (a, c) = (self.anchor.get(), self.caret.get());
            if a != c {
                let (x0, x1) = (line.caret_x(&text, a.min(c)) / s, line.caret_x(&text, a.max(c)) / s);
                canvas.fill(x0.min(x1) - scroll, x0.max(x1) - scroll, top, bottom, rgba8(&color, 0.25));
            }
            let ink = rgba8(&color, 1.0);
            trace(widget, &text, &line, &style, -scroll, baseline, ink);
            canvas.paint(&line, -scroll, baseline, ink, (0.0, w));
            if self.focused.get() {
                let x = (caret_x - scroll).round();
                canvas.fill(x, x + 1.0, top, bottom, ink);
            }
            canvas.append(widget, snapshot);
        }
    }
}

glib::wrapper! {
    /// A one-line text field drawn and edited with quartzite's text.
    pub struct GlyphEntry(ObjectSubclass<imp_entry::GlyphEntry>) @extends gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl GlyphEntry {
    pub fn new(placeholder: &str) -> Self {
        let e: Self = glib::Object::new();
        *e.imp().placeholder.borrow_mut() = placeholder.to_string();
        e
    }

    pub fn text(&self) -> String {
        self.imp().text.borrow().clone()
    }

    pub fn set_text(&self, t: &str) {
        *self.imp().text.borrow_mut() = t.to_string();
        self.imp().caret.set(t.len());
        self.imp().anchor.set(t.len());
        self.queue_draw();
    }

    /// Calls `f` when Enter is pressed.
    pub fn connect_activate(&self, f: impl Fn(&GlyphEntry) + 'static) {
        self.imp().on_activate.borrow_mut().push(Box::new(f));
    }

    fn line(&self) -> (String, Line) {
        let t = self.text();
        let l = style_for(self).shape(&t);
        (t, l)
    }

    fn hit(&self, x: f32) -> usize {
        let (t, l) = self.line();
        l.hit(&t, x + self.imp().scroll.get())
    }

    fn set_caret(&self, at: usize, extend: bool) {
        self.imp().caret.set(at);
        if !extend {
            self.imp().anchor.set(at);
        }
        self.queue_draw();
    }

    fn selection(&self) -> (usize, usize) {
        let (a, c) = (self.imp().anchor.get(), self.imp().caret.get());
        (a.min(c), a.max(c))
    }

    fn replace_selection(&self, s: &str) {
        let (a, b) = self.selection();
        let mut t = self.text();
        let (a, b) = (a.min(t.len()), b.min(t.len()));
        t.replace_range(a..b, s);
        *self.imp().text.borrow_mut() = t;
        self.set_caret(a + s.len(), false);
    }

    fn insert(&self, s: &str) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        self.replace_selection(&clean);
    }

    /// The grapheme boundary before / after `i`.
    fn step(&self, i: usize, forward: bool) -> usize {
        let t = self.text();
        let bounds: Vec<usize> = text_host::font_core::grapheme::clusters(&t)
            .into_iter()
            .map(|(a, _)| a)
            .chain(std::iter::once(t.len()))
            .collect();
        if forward {
            bounds.into_iter().find(|&b| b > i).unwrap_or(t.len())
        } else {
            bounds.into_iter().rev().find(|&b| b < i).unwrap_or(0)
        }
    }

    fn key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let (a, b) = self.selection();
        let caret = self.imp().caret.get();
        let len = self.text().len();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                for f in self.imp().on_activate.borrow().iter() {
                    f(self);
                }
            }
            gdk::Key::BackSpace => {
                if a == b && a > 0 {
                    self.imp().anchor.set(self.step(a, false));
                }
                self.replace_selection("");
            }
            gdk::Key::Delete | gdk::Key::KP_Delete => {
                if a == b && b < len {
                    self.imp().anchor.set(self.step(b, true));
                }
                self.replace_selection("");
            }
            gdk::Key::Left | gdk::Key::KP_Left => {
                let to = if a != b && !shift { a } else { self.step(caret, false) };
                self.set_caret(to, shift);
            }
            gdk::Key::Right | gdk::Key::KP_Right => {
                let to = if a != b && !shift { b } else { self.step(caret, true) };
                self.set_caret(to, shift);
            }
            gdk::Key::Home | gdk::Key::KP_Home => self.set_caret(0, shift),
            gdk::Key::End | gdk::Key::KP_End => self.set_caret(len, shift),
            gdk::Key::a | gdk::Key::A if ctrl => {
                self.imp().anchor.set(0);
                self.set_caret(len, true);
            }
            gdk::Key::c | gdk::Key::C | gdk::Key::x | gdk::Key::X if ctrl => {
                if a != b {
                    let t = self.text();
                    self.clipboard().set_text(&t[a..b]);
                    if matches!(key, gdk::Key::x | gdk::Key::X) {
                        self.replace_selection("");
                    }
                }
            }
            gdk::Key::v | gdk::Key::V if ctrl => {
                let weak = self.downgrade();
                self.clipboard().read_text_async(None::<&gtk4::gio::Cancellable>, move |r| {
                    if let (Some(e), Ok(Some(s))) = (weak.upgrade(), r) {
                        e.insert(s.as_str());
                    }
                });
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}
