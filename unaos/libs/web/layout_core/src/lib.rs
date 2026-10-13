//! LAYOUTCORE (LEDGER SR76, AETHERMETAL M2): Aether's layout engine as ONE shared core.
//!
//! CHARTER: Aether (CODEX §2: Aether owns the Web) — seam `shared-core`, a `no_std` + `alloc` library both rings
//! link: the host browser (handlers/aether) and the ring-3 program APPS/AETHER.ELF (crates/user-aether).
//!
//! What it is: the box generation, the taffy style mapping, the inline formatting context, table layout, margin
//! collapsing and block widths that handlers/aether/src/layout/ and the taffy-style half of
//! handlers/aether/src/css/ were, cut free of std over three seams:
//!
//! * [`Dom`] / [`DomNode`] — the document (html_core's arena behind each ring's own handle; no Rc/RefCell here);
//! * [`Fonts`] — measuring, line metrics and line breaking (the host's `fonts/` database, the image's
//!   `font_core::ui` engine);
//! * [`Host`] — the static hooks that stay ring-specific: the coverage ledger, the paint value types
//!   (gradients, shadows) and their parsers, family-list interning, replaced-element intrinsic sizes, `var()`.
//!
//! `std::collections::HashMap` became [`NodeMap`] (a `BTreeMap`; taffy's `NodeId` is not `Ord`), the
//! `OnceLock` and thread-locals became fields ([`LayoutTree::rowspans`], [`css::ParseCx`]) or `Host` hooks, and
//! the `Instant`/`env` profiling stayed in the host's cascade. [`layout`] is the one entry the spec names; the
//! host calls the same pieces ([`build_tree`], [`apply_specs`], [`remeasure`]) around its own cascade.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub use taffy;

pub mod blockwidth;
pub mod collapse;
pub mod css;
pub mod fmath;
pub mod hit;
pub mod inline;
pub mod lines;
mod map;
pub mod paint;
pub mod table;
mod tree;

pub use map::{Entry, NodeMap};
pub use paint::{ClipShape, FontWeight, MarkerPaint, PaintStyle, ShapeRadius, TransformOp};
pub use tree::*;

pub(crate) mod prelude {
    pub use crate::fmath::FloatExt as _;
    pub use alloc::borrow::ToOwned;
    pub use alloc::collections::{BTreeMap, BTreeSet};
    pub use alloc::format;
    pub use alloc::rc::Rc;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec;
    pub use alloc::vec::Vec;
}

use prelude::*;

// ---- the document ------------------------------------------------------------------------------------------

/// What a DOM node is, as far as layout cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Doctype,
    Element,
    Text,
    Comment,
    Other,
}

/// One node handle of the document (html_core's arena + a `NodeId`, in each ring's own shape). Navigation
/// returns snapshots, so layout never holds a borrow of the document.
pub trait DomNode: Clone {
    fn kind(&self) -> NodeKind;
    /// An element's local name.
    fn tag(&self) -> Option<String>;
    /// An element's attribute value.
    fn attr(&self, name: &str) -> Option<String>;
    fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }
    /// Text content: a text node's data, else the concatenated text of the inclusive subtree.
    fn text(&self) -> String;
    fn children(&self) -> Vec<Self>;
    fn parent(&self) -> Option<Self>;
    /// Identity.
    fn same(&self, other: &Self) -> bool;
    /// A doctype's (name, public id, system id).
    fn doctype(&self) -> Option<(String, String, String)>;
    /// Siblings before `self`, nearest first.
    fn preceding_siblings(&self) -> Vec<Self> {
        let Some(p) = self.parent() else { return Vec::new() };
        let kids = p.children();
        let i = kids.iter().position(|k| k.same(self)).unwrap_or(0);
        kids[..i].iter().rev().cloned().collect()
    }
    /// Siblings after `self`, in order.
    fn following_siblings(&self) -> Vec<Self> {
        let Some(p) = self.parent() else { return Vec::new() };
        let kids = p.children();
        match kids.iter().position(|k| k.same(self)) {
            Some(i) => kids[i + 1..].to_vec(),
            None => Vec::new(),
        }
    }
    /// Ancestors, nearest first.
    fn ancestors(&self) -> Vec<Self> {
        let mut out = Vec::new();
        let mut cur = self.parent();
        while let Some(c) = cur {
            cur = c.parent();
            out.push(c);
        }
        out
    }
    /// Descendants in tree order, excluding `self`.
    fn descendants(&self) -> Vec<Self> {
        let mut out = Vec::new();
        fn walk<N: DomNode>(n: &N, out: &mut Vec<N>) {
            for c in n.children() {
                out.push(c.clone());
                walk(&c, out);
            }
        }
        walk(self, &mut out);
        out
    }
    fn is_element(&self) -> bool {
        self.kind() == NodeKind::Element
    }
    fn is_text(&self) -> bool {
        self.kind() == NodeKind::Text
    }
}

/// A document to lay out (AETHERMETAL.md §M2: "html_core::Document + NodeId: children, element(), text — no
/// Rc/RefCell"): its root node, whose handles answer the queries.
pub trait Dom {
    type Node: DomNode;
    /// The node layout starts from (the Document node).
    fn root(&self) -> Self::Node;
}

// ---- the ring-specific hooks -------------------------------------------------------------------------------

/// The static hooks layout and the declaration parser call that stay ring-specific. Every hook but the types
/// has a default good for a ring without that facility (the image's).
pub trait Host: Sized + Clone + core::fmt::Debug + Default + 'static {
    /// The DOM node handle the layout tree maps boxes back to.
    type Node: DomNode;
    /// A parsed gradient background layer (the host's `render::effects::Gradient`).
    type Gradient: Clone + core::fmt::Debug;
    /// A parsed box/text shadow layer (the host's `render::effects::Shadow`).
    type Shadow: Clone + core::fmt::Debug;

    /// Records a CSS coverage gap (the host's `ledger::record_css`).
    fn record_css(_what: &str) {}
    fn parse_gradient(_value: &str) -> Option<Self::Gradient> {
        None
    }
    fn parse_box_shadow(_value: &str) -> Vec<Self::Shadow> {
        Vec::new()
    }
    fn shadow_is_inset(_s: &Self::Shadow) -> bool {
        false
    }
    /// A colour with its alpha: ((r, g, b), a).
    fn parse_color_alpha(value: &str) -> Option<((u8, u8, u8), f32)> {
        use css_core::color::{parse_color, Color};
        let cvs = css_core::parser::parse_component_values(value);
        match parse_color(&cvs)? {
            Color::Rgba(c) => {
                let ch = |x: f64| -> u8 { if x <= 0.0 { 0 } else if x >= 255.0 { 255 } else { (x + 0.5) as u8 } };
                Some(((ch(c.r), ch(c.g), ch(c.b)), c.a as f32))
            }
            Color::CurrentColor => None,
        }
    }
    /// A `font-family` value as an interned family-list id (0 sans-serif, 1 serif, 2 monospace, 3 the
    /// standard font are fixed). The default classifies the first family into those four.
    fn intern_family_list(value: &str) -> Option<u16> {
        let first = value.split(',').next()?.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        if first.is_empty() || matches!(first.as_str(), "inherit" | "initial" | "unset" | "revert") {
            return None;
        }
        Some(if first == "monospace" || first.contains("mono") || first.contains("courier") || first.contains("consolas") {
            MONO
        } else if first == "serif" || first.contains("times") || first.contains("georgia") {
            SERIF
        } else {
            SANS
        })
    }
    /// The family-list id of the form controls' font (Chromium: `Arial`).
    fn control_family() -> u16 {
        SANS
    }
    /// `object-fit` keyword → 0 fill, 1 contain, 2 cover, 3 none, 4 scale-down.
    fn parse_object_fit(value: &str) -> Option<u8> {
        Some(match value.trim() {
            "fill" => 0,
            "contain" => 1,
            "cover" => 2,
            "none" => 3,
            "scale-down" => 4,
            _ => return None,
        })
    }
    /// The document's computed custom property `name` (`--x`), for the build-time `style=""` pass.
    fn custom_prop(_name: &str) -> Option<String> {
        None
    }
    /// `text-transform` (1 uppercase, 2 lowercase, 3 capitalize) applied to a run.
    fn transform_text(text: &str, tt: u8) -> String {
        match tt {
            1 => text.to_uppercase(),
            2 => text.to_lowercase(),
            3 => {
                let mut out = String::with_capacity(text.len());
                let mut start = true;
                for c in text.chars() {
                    if start && c.is_alphabetic() {
                        out.extend(c.to_uppercase());
                    } else {
                        out.push(c);
                    }
                    start = c.is_whitespace();
                }
                out
            }
            _ => text.to_string(),
        }
    }
    /// A replaced media element's intrinsic size (`<video>`, `<audio controls>`).
    fn media_intrinsic(_node: &Self::Node) -> Option<(f32, f32)> {
        None
    }
    /// An `<img>`'s intrinsic size from its decoded source.
    fn img_intrinsic(_node: &Self::Node) -> Option<(f32, f32)> {
        None
    }
    /// Script-set selectedness of an `<option>` (`None` = the attribute decides).
    fn option_selected(_node: &Self::Node) -> Option<bool> {
        None
    }
}

// ---- fonts ---------------------------------------------------------------------------------------------------

/// The fixed family-list ids.
pub const SANS: u16 = 0;
pub const SERIF: u16 = 1;
pub const MONO: u16 = 2;
pub const STANDARD: u16 = 3;

/// What a text run asks for. `family` is a family-LIST id ([`Host::intern_family_list`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontSel {
    pub family: u16,
    pub weight: u16,
    pub italic: bool,
    /// font-stretch in percent.
    pub stretch: u16,
}

impl Default for FontSel {
    fn default() -> Self {
        FontSel { family: STANDARD, weight: 400, italic: false, stretch: 100 }
    }
}

impl FontSel {
    pub fn new(family: u16, weight: u16, italic: bool) -> Self {
        FontSel { family, weight, italic, stretch: 100 }
    }
}

/// A run's measurer: selection, size, spacing and direction (the host's `fonts::shape::Advancer` in data).
#[derive(Clone, Copy, Debug)]
pub struct Adv {
    pub sel: FontSel,
    pub size: f32,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub rtl: bool,
}

impl Adv {
    pub fn new(sel: FontSel, size: f32, letter_spacing: f32) -> Self {
        Adv { sel, size, letter_spacing, word_spacing: 0.0, rtl: false }
    }
    pub fn with_word_spacing(mut self, ws: f32) -> Self {
        self.word_spacing = ws;
        self
    }
    pub fn with_dir(mut self, rtl: bool) -> Self {
        self.rtl = rtl;
        self
    }
}

/// The text properties line breaking depends on (css-text-3).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMode {
    /// 0 normal, 1 nowrap, 2 pre, 3 pre-wrap, 4 pre-line, 5 break-spaces.
    pub white_space: u8,
    /// 0 normal, 1 break-all, 2 keep-all.
    pub word_break: u8,
    /// 0 normal, 1 break-word / anywhere.
    pub overflow_wrap: u8,
    /// Extra px after every character.
    pub letter_spacing: f32,
    /// Extra px on every word separator (css-text-3 §7.1).
    pub word_spacing: f32,
    /// The paragraph direction is right-to-left.
    pub rtl: bool,
    /// A collapsible space precedes the run's first word.
    pub lead: bool,
    /// A collapsible space follows the run's last word.
    pub trail: bool,
}

/// One laid-out line: the characters to draw (spaces included) and its advance width in px.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub width: f32,
}

/// The font seam (AETHERMETAL.md §M2). Methods take `&self` so the inline formatter's closures can share one
/// provider; an implementor that caches (the image's glyph engine) keeps its state in a `RefCell`.
pub trait Fonts {
    /// The selection resolves to a face.
    fn has_face(&self, sel: FontSel) -> bool;
    /// Advance of `run` in px: shaped width plus `adv`'s letter/word spacing.
    fn measure(&self, run: &str, adv: &Adv) -> f32;
    /// (ascent, descent, line gap) in px of the selection's primary face, rounded as Chromium rounds them.
    fn line_metrics(&self, sel: FontSel, size: f32) -> (f32, f32, f32);
    /// The face's x-height in px.
    fn x_height(&self, sel: FontSel, size: f32) -> f32;
    /// The (ascent, descent, line gap) of every face other than the primary that shaping `run` falls back to,
    /// each once, in first-use order (Blink's `line-height: normal` union).
    fn fallback_metrics(&self, _run: &str, _adv: &Adv) -> Vec<(f32, f32, f32)> {
        Vec::new()
    }
    /// The used line height: `mult` x size, `-mult` px when negative, or (0 = `normal`) the metrics sum.
    fn line_height(&self, sel: FontSel, size: f32, mult: f32) -> f32 {
        if mult < 0.0 {
            -mult
        } else if mult > 0.0 {
            size * mult
        } else {
            let (a, d, g) = self.line_metrics(sel, size);
            a + d + g
        }
    }
    /// Offset from a line box's top to its baseline (CSS 2.2 §10.8.1 half-leading).
    fn baseline_offset(&self, sel: FontSel, size: f32, mult: f32) -> f32 {
        let (a, d, _) = self.line_metrics(sel, size);
        let lh = self.line_height(sel, size, mult);
        ((lh - (a + d)) / 2.0).floor() + a
    }
    /// Breaks one run of an inline formatting context whose current line already holds `start_x` px
    /// (`prior` = some of it is content). The default is [`lines::break_lines_from`] over [`Fonts::measure`].
    fn break_lines(&self, text: &str, mode: &TextMode, adv: &Adv, max_w: f32, start_x: f32, prior: bool) -> Vec<Line> {
        lines::break_lines_from(self, adv, text, mode, max_w, start_x, prior)
    }
}
