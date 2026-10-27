// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — the PDF4 face's link and form layer over pdf_core (PDFFORMS SR326, PDFWRITE SR325); PDF4ANNOTS
//! (LEDGER SR343). No parser, rasterizer, fill or writer code lives here: the annotations are `Page::annots_with`, the
//! geometry is the renderer's own base (`Page::device`), a fill is `Form::set_value` -> `write::Edit::save_update` ->
//! `Document::parse` (the incremental update, newest wins, IS the re-render source).
//!
//! * links: hover sets the pointer cursor and the status line (`hovered_link`: the URI, or "Page n"); a click (press and
//!   release on the same link, PDFium's mouse-down-state match) follows /URI through the `<a>` path's `pending_nav`
//!   (a `javascript:` URI refused alike), /GoTo and /Named (Next / Prev / First / LastPage) through the one scroll.
//! * form widgets: a check box toggles on click; a text field takes focus and the keys through the engine's
//!   `forms::edit::EditState` (the DOM control's caret model). One focus session is ONE incremental section: each
//!   change is recomputed against the bytes taken at focus; blur makes the current bytes the base.
//! * a refused fill (save_update of an encrypted or rebuilt file, a set_value error) leaves the page as it was and is
//!   named in `PdfDocument::form_refused`.

use std::rc::Rc;

use pdf_core::annots::form::ff;
use pdf_core::annots::{Annot, Dest, Field, FieldKind, FieldRef, LinkAction, View, flags};
use pdf_core::object::text_string;
use pdf_core::write::Edit;
use pdf_core::{Document, Object, Ref};

use super::{PT, PdfDocument};

/// What a pointer is over: a page's link or editable form widget, as (page, index in the page's /Annots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Link(usize, usize),
    Field(usize, usize),
}

/// The focused text field: its widget, its field, the file it is edited against, and the typed value.
pub struct FieldFocus {
    pub target: Target,
    pub obj: Option<Ref>,
    pub multiline: bool,
    /// The bytes (and their parse) at focus: every change is one update over these.
    base: Vec<u8>,
    base_doc: Document,
    pub value: String,
}

/// A user-space /Rect through a page's base (`apply`: `Page::device(72.0)`'s) as a device bbox `[x0, y0, x1, y1]`.
fn device_box(apply: impl Fn(f64, f64) -> (f64, f64), r: [f64; 4]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (dx, dy) = apply(x, y);
        b = [b[0].min(dx), b[1].min(dy), b[2].max(dx), b[3].max(dy)];
    }
    b
}

/// Whether the face edits this field: a text field (not read-only or a password) or a check box (not read-only).
fn editable(f: &Field) -> bool {
    match f.kind {
        FieldKind::Text => f.flags & (ff::READ_ONLY | ff::PASSWORD) == 0,
        FieldKind::Checkbox => f.flags & ff::READ_ONLY == 0,
        _ => false,
    }
}

/// Whether a check box is on: its /V a name other than /Off.
fn is_on(f: &Field) -> bool {
    matches!(&f.value, Object::Name(n) if !n.is_empty() && n.as_slice() != b"Off")
}

/// A `javascript:` URL (the `<a>` path's refusal: the page's, never a navigation).
fn is_javascript(u: &str) -> bool {
    let t = u.trim_start_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control());
    t.len() >= 11 && t.is_char_boundary(11) && t[..11].eq_ignore_ascii_case("javascript:")
}

/// The page a /Named action goes to from page `cur` of `n`.
pub fn named_page(name: &[u8], cur: usize, n: usize) -> Option<usize> {
    match name {
        b"NextPage" => Some((cur + 1).min(n - 1)),
        b"PrevPage" => Some(cur.saturating_sub(1)),
        b"FirstPage" => Some(0),
        b"LastPage" => Some(n - 1),
        _ => None,
    }
}

/// Fill: `field` (by its reference) set to `value` in `doc`, as the incremental update over `base`.
fn fill(doc: &Document, base: &[u8], obj: Option<Ref>, value: &str) -> Result<Vec<u8>, pdf_core::Error> {
    let form = doc.form().ok_or(pdf_core::Error::Missing)?;
    let field = form.fields.iter().find(|f| f.obj.is_some() && f.obj == obj).ok_or(pdf_core::Error::Missing)?;
    let edits = form.set_value(doc, field, value)?;
    let mut ed = Edit::new(doc);
    for (r, o) in edits {
        ed.set(r, o);
    }
    ed.save_update(base)
}

impl PdfDocument {
    /// Page `i`'s annotations (destinations resolved), read on first use.
    pub fn annots(&self, i: usize) -> Rc<Vec<Annot>> {
        if let Some(a) = &self.annots.borrow()[i] {
            return a.clone();
        }
        let a = Rc::new(self.pages[i].annots_with(&self.doc, &self.page_refs));
        self.annots.borrow_mut()[i] = Some(a.clone());
        a
    }

    /// The form field a widget names (by reference, else by its fully qualified name).
    fn field_for(&self, fr: &FieldRef) -> Option<&Field> {
        let form = self.form.as_ref()?;
        form.fields.iter().find(|f| f.obj.is_some() && f.obj == fr.obj).or_else(|| form.fields.iter().find(|f| f.name == fr.name))
    }

    /// The field of page `p`'s annotation `i`.
    pub fn annot_field(&self, p: usize, i: usize) -> Option<Field> {
        let a = self.annots(p);
        self.field_for(a.get(i)?.field.as_ref()?).cloned()
    }

    /// Page `p`'s annotation `i`'s box in 72-dpi page space (the space `Page::text` spans and `marks` use).
    pub fn annot_box(&self, p: usize, i: usize) -> Option<[f64; 4]> {
        let a = self.annots(p);
        let m = self.pages[p].device(72.0);
        Some(device_box(|x, y| m.apply(x, y), a.get(i)?.rect))
    }

    /// The link or editable widget under viewport CSS (`x`, `y`): the topmost (last painted) visible one.
    pub fn target_at(&self, x: f64, y: f64, vw: f64, scroll_y: f64) -> Option<Target> {
        let dy = y + scroll_y;
        let page = self.page_at(dy);
        let (px, py, _, _) = self.page_rect(page, vw);
        let (ux, uy) = ((x - px) / PT, (dy - py) / PT);
        let m = self.pages[page].device(72.0);
        let annots = self.annots(page);
        for (i, a) in annots.iter().enumerate().rev() {
            if !a.visible() {
                continue;
            }
            let t = if a.subtype == b"Link" && a.link.is_some() {
                Target::Link(page, i)
            } else if a.subtype == b"Widget"
                && a.flags & flags::READ_ONLY == 0
                && a.field.as_ref().and_then(|fr| self.field_for(fr)).is_some_and(editable)
            {
                Target::Field(page, i)
            } else {
                continue;
            };
            let r = device_box(|x, y| m.apply(x, y), a.rect);
            if ux >= r[0] && ux <= r[2] && uy >= r[1] && uy <= r[3] {
                return Some(t);
            }
        }
        None
    }

    /// A destination's document CSS y: its page's top, plus the /XYZ (or /FitH, /FitBH) top through the page's base.
    pub fn dest_y(&self, d: &Dest) -> f64 {
        let i = d.page.min(self.pages.len() - 1);
        let x0 = self.pages[i].crop_box[0];
        let at = match d.view {
            View::Xyz { left, top: Some(t), .. } => Some((left.unwrap_or(x0), t)),
            View::FitH(Some(t)) | View::FitBH(Some(t)) => Some((x0, t)),
            _ => None,
        };
        match at {
            Some((ux, uy)) => self.tops[i] + self.pages[i].device(72.0).apply(ux, uy).1.max(0.0) * PT,
            None => self.tops[i],
        }
    }

    /// The status line over a link: the resolved URI, or "Page n"; none for a refused action.
    fn link_status(&self, link: &LinkAction, base: &str, cur: usize) -> Option<String> {
        match link {
            LinkAction::Uri(u) if !is_javascript(u) => Some(crate::images::resolve(base, u)),
            LinkAction::GoTo(d) => Some(format!("Page {}", d.page + 1)),
            LinkAction::Named(n) => named_page(n, cur, self.pages.len()).map(|p| format!("Page {}", p + 1)),
            _ => None,
        }
    }

    /// The focused field's box on page `i` (72-dpi page space).
    pub(crate) fn focus_box(&self, i: usize) -> Option<[f64; 4]> {
        match self.focus.as_ref()?.target {
            Target::Field(p, a) if p == i => self.annot_box(p, a),
            _ => None,
        }
    }

    /// Drop the text field's focus (its edits are already the current bytes). Whether one was focused.
    pub(crate) fn blur(&mut self) -> bool {
        self.focus.take().is_some()
    }

    /// The edited file replaces the open one: re-parsed, every cache, the find hits and the selection dropped (widget
    /// text is in `Page::text`, so span indices move); the page geometry is unchanged.
    fn reload(&mut self, bytes: Vec<u8>) -> Result<(), pdf_core::Error> {
        let doc = Document::parse(&bytes)?;
        let pages = doc.pages()?;
        if pages.len() != self.pages.len() {
            return Err(pdf_core::Error::Unsupported("the edited file has another page count"));
        }
        let n = pages.len();
        self.page_refs = pages.iter().map(|p| p.obj).collect();
        self.form = doc.form();
        self.doc = doc;
        self.pages = pages;
        self.bytes = bytes;
        self.cache.borrow_mut().clear();
        *self.text.borrow_mut() = vec![None; n];
        *self.annots.borrow_mut() = vec![None; n];
        self.hits.clear();
        self.cur = None;
        self.sel = Default::default();
        Ok(())
    }

    /// A fill's outcome: the new file reloaded, or the refusal named (the page as it was).
    fn settle(&mut self, r: Result<Vec<u8>, pdf_core::Error>) -> bool {
        match r.and_then(|b| self.reload(b)) {
            Ok(()) => {
                self.form_refused = None;
                true
            }
            Err(e) => {
                self.form_refused = Some(format!("{e:?}"));
                false
            }
        }
    }
}

// =================================================================================================
// The engine's hooks (lib.rs folds each call onto an existing line while `pdf` is Some).
// =================================================================================================

impl crate::AetherEngine {
    /// `pointer_moved` on a PDF: the cursor over a link / check box (pointer) or a text field (text).
    pub(crate) fn pdf_hover(&mut self, x: f64, y: f64) {
        self.pointer = Some((x, y));
        let (vw, sy) = (self.width as f64, self.scroll_y);
        let Some(pdf) = self.pdf.as_mut() else { return };
        let t = pdf.target_at(x, y, vw, sy);
        pdf.hover = t;
        let cursor = match t {
            Some(Target::Link(..)) => "pointer",
            Some(Target::Field(p, i)) if pdf.annot_field(p, i).is_some_and(|f| f.kind == FieldKind::Text) => "text",
            Some(Target::Field(..)) => "pointer",
            None => "default",
        };
        if cursor != self.cursor {
            self.cursor = cursor.to_string();
            self.pending_cursor = Some(cursor.to_string());
        }
    }

    /// `hovered_link` on a PDF: the hovered link's URI or "Page n" (else the shell shows `pdf_status`).
    pub(crate) fn pdf_hovered_link(&self) -> Option<String> {
        let pdf = self.pdf.as_ref()?;
        let Target::Link(p, i) = pdf.hover? else { return None };
        let link = pdf.annots(p).get(i)?.link.clone()?;
        let cur = pdf.page_at(self.scroll_y + self.height as f64 / 2.0);
        pdf.link_status(&link, &self.current_url(), cur)
    }

    /// The MouseUp on a PDF: a release on the link or widget the press was on activates it.
    pub(crate) fn pdf_release(&mut self, x: f64, y: f64) {
        let (vw, sy) = (self.width as f64, self.scroll_y);
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some(t) = pdf.pressed.take().filter(|p| pdf.target_at(x, y, vw, sy) == Some(*p)) else { return };
        match t {
            Target::Link(p, i) => {
                if let Some(link) = pdf.annots(p).get(i).and_then(|a| a.link.clone()) {
                    self.pdf_follow(link);
                }
            }
            Target::Field(p, i) => self.pdf_focus_field(Target::Field(p, i)),
        }
    }

    /// Carry out a link: /URI navigates (the `<a>` path's `pending_nav`); /GoTo and /Named scroll.
    fn pdf_follow(&mut self, link: LinkAction) {
        let vh = self.height as f64;
        let Some(pdf) = self.pdf.as_ref() else { return };
        let y = match link {
            LinkAction::Uri(u) => {
                if !is_javascript(&u) {
                    let url = crate::images::resolve(&self.current_url(), &u);
                    self.pending_nav = Some(crate::forms::OpenDocument::get(url));
                }
                return;
            }
            LinkAction::GoTo(d) => pdf.dest_y(&d),
            LinkAction::Named(n) => {
                let cur = pdf.page_at(self.scroll_y + vh / 2.0);
                match named_page(&n, cur, pdf.page_count()) {
                    Some(i) => pdf.page_rect(i, self.width as f64).1,
                    None => return,
                }
            }
            LinkAction::Refused(_) => return,
        };
        self.handle_event(crate::api::events::Event::Scroll(0.0, y - self.scroll_y));
        self.pdf_dirty();
    }

    /// A click on a form widget: a check box toggles now; a text field takes focus (the caret at its value's end).
    pub(crate) fn pdf_focus_field(&mut self, t: Target) {
        let Target::Field(p, i) = t else { return };
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some(field) = pdf.annot_field(p, i) else { return };
        match field.kind {
            FieldKind::Checkbox => {
                pdf.blur();
                let on = match &field.on_state {
                    Some(s) => String::from_utf8_lossy(s).into_owned(),
                    None => "Yes".to_string(),
                };
                let v = if is_on(&field) { String::new() } else { on };
                let r = fill(&pdf.doc, &pdf.bytes, field.obj, &v);
                pdf.settle(r);
            }
            FieldKind::Text => {
                if pdf.focus.as_ref().is_some_and(|f| f.target == t) {
                    return;
                }
                pdf.blur();
                let Ok(base_doc) = Document::parse(&pdf.bytes) else { return };
                let value = match &field.value {
                    Object::String(s) => text_string(s),
                    _ => String::new(),
                };
                self.edit = crate::forms::edit::EditState::at(value.len());
                pdf.focus = Some(FieldFocus {
                    target: t,
                    obj: field.obj,
                    multiline: field.flags & ff::MULTILINE != 0,
                    base: pdf.bytes.clone(),
                    base_doc,
                    value,
                });
            }
            _ => return,
        }
        self.pdf_dirty();
    }

    /// The focused text field's value changed: refill it over the focus's base and re-render.
    fn pdf_refill(&mut self) {
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some(f) = pdf.focus.take() else { return };
        let r = fill(&f.base_doc, &f.base, f.obj, &f.value);
        pdf.settle(r);
        pdf.focus = Some(f);
        self.pdf_dirty();
    }

    /// `insert_text` on a PDF: typed text goes into the focused text field at the caret.
    pub(crate) fn pdf_insert(&mut self, text: &str) {
        let Some(f) = self.pdf.as_mut().and_then(|p| p.focus.as_mut()) else { return };
        let ins = crate::forms::edit::sanitize_insert(text, f.multiline);
        self.edit.clamp(&f.value);
        if ins.is_empty() && !self.edit.has_selection() {
            return;
        }
        self.edit.insert(&mut f.value, &ins);
        self.pdf_refill();
    }

    /// `key_default` on a PDF: the focused text field's editing keys (Return blurs). False when no field holds the
    /// keys or the key is not an editing one (scrolling and Tab go on).
    pub(crate) fn pdf_key(&mut self, key: &str) -> bool {
        use crate::forms::edit::Outcome;
        let Some(f) = self.pdf.as_mut().and_then(|p| p.focus.as_mut()) else { return false };
        let before = f.value.clone();
        let multiline = f.multiline;
        match self.edit.key(&mut f.value, key, multiline) {
            Outcome::Edited => {
                if f.value != before {
                    self.pdf_refill();
                }
                true
            }
            Outcome::Moved | Outcome::Consumed => true,
            Outcome::Submit => {
                if let Some(p) = self.pdf.as_mut() {
                    p.blur();
                }
                self.pdf_dirty();
                true
            }
            Outcome::Unhandled => false,
        }
    }
}
