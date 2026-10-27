// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDFFORMS (SR326): annotations, destinations, link actions and the AcroForm
//! (ISO 32000-1 §12.3.2, §12.5, §12.6, §12.7): read, paint through PDF2's interpreter, fill as object edits.
//!
//! * this module: the page's /Annots ([`Page::annots`] -> [`Annot`]): subtype, /Rect, the /F flags ([`flags`]), the
//!   normal appearance (/AP /N, a state dictionary picking /AS), /Contents, the link action ([`LinkAction`]: /Dest or
//!   /A with /URI /GoTo /Named; every other action refused BY NAME, /GoToR and /JavaScript among them) and, for a
//!   widget, its field ([`FieldRef`]).
//! * [`dests`]: explicit destinations (`[page /XYZ left top zoom]` and the /Fit family), named ones (the catalog's
//!   /Dests dictionary, the /Names /Dests name tree), `Document::dest`.
//! * [`form`]: the AcroForm (`Document::form` -> [`Form`]): the field tree through /Kids and /Parent, fully qualified
//!   names, the ONE inheritance walk ([`form::inherited`]), and FILL: [`Form::set_value`] returns the object edits
//!   (`Vec<(Ref, Object)>`) a writer serializes as an incremental update.
//! * [`appearance`]: the §12.5.5 placement of an appearance stream on its /Rect, the list `Page::render` paints AFTER
//!   the content (each through PDF2's form-XObject path, `render::Interp::form`), and the generated appearance of a
//!   widget without /AP: a form XObject CONTENT STREAM (text through the caller's `TextSink`, the check as a path).
//!
//! Nothing here rasterizes or decodes glyphs; no shared state (every fn takes `&Document`, returns owned values).

pub mod appearance;
pub mod dests;
pub mod form;

use alloc::string::String;
use alloc::vec::Vec;

use crate::doc::{Document, Page};
use crate::object::{Dict, Object, Ref, Stream, text_string};

pub use dests::{Dest, View};
pub use form::{Field, FieldKind, Form};

/// The most annotations read from one page.
pub const MAX_ANNOTS: usize = 1 << 16;

/// The annotation flags (/F, §12.5.3).
pub mod flags {
    pub const INVISIBLE: u32 = 1;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_ZOOM: u32 = 1 << 3;
    pub const NO_ROTATE: u32 = 1 << 4;
    pub const NO_VIEW: u32 = 1 << 5;
    pub const READ_ONLY: u32 = 1 << 6;
    pub const LOCKED: u32 = 1 << 7;
    pub const TOGGLE_NO_VIEW: u32 = 1 << 8;
    pub const LOCKED_CONTENTS: u32 = 1 << 9;
}

/// What activating a link (or an annotation's /A) does.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkAction {
    /// /URI: the target, as written (7-bit ASCII by the spec).
    Uri(String),
    /// /GoTo or /Dest: a destination in this document.
    GoTo(Dest),
    /// /Named: NextPage, PrevPage, FirstPage, LastPage, or a viewer's own name.
    Named(Vec<u8>),
    /// An action this core does not carry out, by name: GoToR, GoToE, JavaScript, Launch, SubmitForm, ResetForm, ...
    Refused(&'static str),
}

/// The field a widget annotation belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRef {
    /// The field dictionary's reference (the widget's own when the two are merged).
    pub obj: Option<Ref>,
    /// The fully qualified name (the /T chain joined by `.`).
    pub name: String,
}

/// One annotation of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Annot {
    /// The annotation dictionary's reference (`None` when written directly inside /Annots).
    pub obj: Option<Ref>,
    /// The annotation dictionary as written.
    pub dict: Dict,
    /// /Subtype (`Link`, `Widget`, `Text`, `Highlight`, ...), empty when absent.
    pub subtype: Vec<u8>,
    /// /Rect in default user space, normalised (zeros when absent).
    pub rect: [f64; 4],
    /// /F ([`flags`]).
    pub flags: u32,
    /// The normal appearance stream: /AP /N, or the entry of its state dictionary that /AS names.
    pub appearance: Option<Stream>,
    /// /Contents.
    pub contents: Option<String>,
    /// The link action (/Dest, else /A); `None` when absent or when its destination does not resolve.
    pub link: Option<LinkAction>,
    /// A widget's field.
    pub field: Option<FieldRef>,
}

impl Annot {
    /// Whether a screen viewer draws it: neither Hidden nor NoView.
    pub fn visible(&self) -> bool {
        self.flags & (flags::HIDDEN | flags::NO_VIEW) == 0
    }
}

impl Page {
    /// The page's annotations in /Annots order (destinations resolved against `doc.pages()`, listed once).
    pub fn annots(&self, doc: &Document) -> Vec<Annot> {
        let refs: Vec<Option<Ref>> = doc.pages().map(|p| p.iter().map(|p| p.obj).collect()).unwrap_or_default();
        self.annots_with(doc, &refs)
    }

    /// The same with the document's page references already listed (`Document::pages`' `obj`s in order); an empty
    /// list resolves no destination (the renderer's call: painting needs none).
    pub fn annots_with(&self, doc: &Document, page_refs: &[Option<Ref>]) -> Vec<Annot> {
        let list = doc.get(&self.annots);
        let mut out = Vec::new();
        for o in list.as_array().unwrap_or(&[]).iter().take(MAX_ANNOTS) {
            if let Object::Dict(d) = doc.get(o) {
                out.push(read(doc, o.as_ref(), d, page_refs));
            }
        }
        out
    }
}

fn read(doc: &Document, obj: Option<Ref>, d: Dict, page_refs: &[Option<Ref>]) -> Annot {
    let subtype = d.get(b"Subtype").map(|o| doc.get(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec)).unwrap_or_default();
    let rect = doc.rect(d.get(b"Rect")).unwrap_or([0.0; 4]);
    let flags = d.get(b"F").map(|o| doc.get(o)).and_then(|o| o.as_int()).unwrap_or(0) as u32;
    let appearance = normal_appearance(doc, &d);
    let contents = d.get(b"Contents").map(|o| doc.get(o)).and_then(|o| o.as_str().map(text_string));
    let link = link(doc, &d, page_refs);
    let field = if subtype == b"Widget" { form::field_of(doc, obj, &d) } else { None };
    Annot { obj, dict: d, subtype, rect, flags, appearance, contents, link, field }
}

/// /AP /N: a stream, or the stream its state dictionary holds under /AS (no /AS: none, §12.5.5).
pub fn normal_appearance(doc: &Document, d: &Dict) -> Option<Stream> {
    let ap = doc.get(d.get(b"AP")?);
    match doc.get(ap.as_dict()?.get(b"N")?) {
        Object::Stream(s) => Some(s),
        Object::Dict(states) => {
            let st = doc.get(d.get(b"AS")?);
            match doc.get(states.get(st.as_name()?)?) {
                Object::Stream(s) => Some(s),
                _ => None,
            }
        }
        _ => None,
    }
}

fn link(doc: &Document, d: &Dict, page_refs: &[Option<Ref>]) -> Option<LinkAction> {
    if let Some(dest) = d.get(b"Dest") {
        return dests::resolve(doc, dest, page_refs).map(LinkAction::GoTo);
    }
    let a = doc.get(d.get(b"A")?);
    action(doc, a.as_dict()?, page_refs)
}

/// An action dictionary (§12.6.4).
pub fn action(doc: &Document, a: &Dict, page_refs: &[Option<Ref>]) -> Option<LinkAction> {
    let s = doc.get(a.get(b"S")?);
    match s.as_name()? {
        b"URI" => {
            let u = doc.get(a.get(b"URI")?);
            Some(LinkAction::Uri(String::from_utf8_lossy(u.as_str()?).into_owned()))
        }
        b"GoTo" => dests::resolve(doc, a.get(b"D")?, page_refs).map(LinkAction::GoTo),
        b"Named" => {
            let n = doc.get(a.get(b"N")?);
            Some(LinkAction::Named(n.as_name()?.to_vec()))
        }
        other => Some(LinkAction::Refused(refused_name(other))),
    }
}

fn refused_name(s: &[u8]) -> &'static str {
    match s {
        b"GoToR" => "GoToR",
        b"GoToE" => "GoToE",
        b"JavaScript" => "JavaScript",
        b"Launch" => "Launch",
        b"SubmitForm" => "SubmitForm",
        b"ResetForm" => "ResetForm",
        b"ImportData" => "ImportData",
        b"Thread" => "Thread",
        b"Sound" => "Sound",
        b"Movie" => "Movie",
        b"Hide" => "Hide",
        b"SetOCGState" => "SetOCGState",
        b"Rendition" => "Rendition",
        b"Trans" => "Trans",
        b"GoTo3DView" => "GoTo3DView",
        _ => "action",
    }
}
