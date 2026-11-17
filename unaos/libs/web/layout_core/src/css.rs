//! The taffy-style half of Aether's CSS (moved from handlers/aether/src/css/mod.rs): one winning declaration,
//! as CSS text, folded into a [`SpecifiedStyle`] — the taffy layout fields plus the [`PaintStyle`] — and the
//! value parsers it uses (lengths, `calc()`/`min()`/`max()`/`clamp()`, colours, the flex and box shorthands,
//! generated content). The cascade that picks the winners (css_core) stays with each ring.
//!
//! What were thread-locals is [`ParseCx`]: the viewport `vw`/`vh` resolve against and the font-relative basis
//! (em, rem, ex, ch) `remeasure` re-applies an element's lengths with. The page's custom properties for the
//! build-time `style=""` pass come from [`Host::custom_prop`]; coverage gaps go to [`Host::record_css`].

use crate::prelude::*;
use crate::paint::{ClipShape, LenPct, ShapeRadius, TransformOp, IDENTITY};
use crate::{FontWeight, Host, PaintStyle};
use taffy::geometry::Rect;
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};

/// The context a declaration is parsed in: the viewport (`vw`, `vh`, `vmin`, …) and the font-relative basis
/// (em, rem, ex, ch) in px. The host's cascade parses at the initial 16px font; `remeasure` re-applies an
/// element's font-relative declarations with its own computed font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParseCx {
    pub viewport: (f32, f32),
    pub font: (f32, f32, f32, f32),
    /// AETHERSTYLE3B (SR115): the query container's content-box (width, height) for the container-relative
    /// units (css-contain-3 §6.1: cqw / cqi on the nearest container with a width, cqh / cqb on the nearest
    /// with a height; None = the small viewport's axis). `remeasure` fills it per element after a first layout.
    pub container: (Option<f32>, Option<f32>),
}

impl Default for ParseCx {
    fn default() -> Self {
        ParseCx { viewport: (800.0, 600.0), font: (16.0, 16.0, 8.0, 8.0), container: (None, None) }
    }
}

impl ParseCx {
    /// The context for a viewport (each axis at least 1px), at the initial font basis.
    pub fn new(vw: f32, vh: f32) -> Self {
        ParseCx { viewport: (vw.max(1.0), vh.max(1.0)), ..ParseCx::default() }
    }
}

/// taffy types min-/max-size as length | percentage | auto (the
/// intrinsic keywords are size-only); a keyword there folds to auto.
pub fn lpa(d: Dimension) -> LengthPercentageAuto {
    use taffy::style::ExpandedDimension as E;
    match d.expand() {
        E::Length(v) => LengthPercentageAuto::length(v),
        E::Percent(v) => LengthPercentageAuto::percent(v),
        _ => LengthPercentageAuto::auto(),
    }
}


/// Declarations a rule actually specified. Only `Some` fields are applied,
/// so a rule never stomps another rule's (or the UA default's) values.
#[derive(Default)]
pub struct SpecifiedStyle<H: Host> {
    pub display: Option<Display>,
    pub flex_direction: Option<FlexDirection>,
    pub width: Option<Dimension>,
    pub height: Option<Dimension>,
    pub max_width: Option<Dimension>,
    pub max_height: Option<Dimension>,
    pub min_width: Option<Dimension>,
    pub min_height: Option<Dimension>,
    /// Per side [top, right, bottom, left]: a longhand (`margin-top`)
    /// specifies ONE side and must leave the other three to the cascade
    /// and the UA defaults (CSS Cascade §6: each longhand cascades alone).
    pub padding: [Option<LengthPercentage>; 4],
    pub margin: [Option<LengthPercentageAuto>; 4],
    pub position: Option<taffy::style::Position>,
    pub inset_top: Option<LengthPercentageAuto>,
    pub inset_left: Option<LengthPercentageAuto>,
    pub inset_right: Option<LengthPercentageAuto>,
    pub inset_bottom: Option<LengthPercentageAuto>,
    pub justify: Option<taffy::style::JustifyContent>,
    pub align_items: Option<taffy::style::AlignItems>, pub align_content: Option<taffy::style::AlignContent>, // AETHERCSSALIGNCONTENTFIELD (SR1016): align-content's own field, folded by fold_into on a flex container
    pub align_self: Option<taffy::style::AlignSelf>, pub align_self_auto: bool, pub justify_items: Option<taffy::style::AlignItems>, pub justify_self: Option<taffy::style::AlignSelf>, pub justify_self_auto: bool, // AETHERCSSALIGNSELFARM (SR1068): an explicit align-self auto (taffy None), carried over an earlier rule by merge_specified
    pub box_sizing: Option<taffy::style::BoxSizing>,
    /// `display: flex` (Some(true)) versus any other display (Some(false)).
    /// Aether lays every block out as a column flex box, so a real flex
    /// container is the one that gets CSS flex semantics: row by default,
    /// nowrap by default, stretch by default, children sized by content.
    pub flex_container: Option<bool>,
    pub flex_wrap: Option<taffy::style::FlexWrap>,
    pub row_gap: Option<LengthPercentage>,
    pub column_gap: Option<LengthPercentage>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<Dimension>,
    /// `aspect-ratio` (css-sizing-4 §5): Some(None) = `auto`, Some(Some(w / h)) = a preferred ratio.
    pub aspect_ratio: Option<Option<f32>>, pub revert_spec: u32, // AETHERCSSWIDEREVERTNONINHERITED (SR1037): the revert erase marks of display / margin / padding / border / background (REVERT_*)
    pub paint: PaintStyle<H>,
    /// `cursor` (css-ui-4 §5.1, inherited): the keyword the cascade chose, `auto` included; read
    /// back per element by [`cursor_for`] for the hovered element (AETHERHOVER, SR67).
    pub cursor: Option<&'static str>,
    /// AETHERGEN (SR71): the generated-content properties (css-content-3, css-lists-3).
    pub generated: GenStyle,
    /// AETHERSTYLE3 (SR103): the `transition-*` and `animation-*` lists (`anim`), and the specified text of
    /// every animatable property in cascade order — the before- and after-change values a transition runs
    /// between, and the base value of an implicit keyframe.
    pub transition: crate::anim::TransitionLists,
    pub animation: crate::anim::AnimationLists,
    pub animatable: Vec<(String, String)>, pub inherit_box: u32, // fold seam SR1007: AETHERCSSWIDENONINHERITEDINHERIT's explicit-inherit bits (tail block)
}

/// The generated-content properties of one element or pseudo-element, as specified: `content`
/// (css-content-3 §2), `counter-reset`/`counter-increment`/`counter-set` (css-lists-3 §4) and `quotes`
/// (css-content-3 §3, inherited — `layout::generated` walks it down the tree).
#[derive(Default, Clone, Debug, PartialEq)]
pub struct GenStyle {
    pub content: Option<Content>,
    pub counter_reset: Option<Vec<(String, i32)>>,
    pub counter_increment: Option<Vec<(String, i32)>>,
    pub counter_set: Option<Vec<(String, i32)>>,
    pub quotes: Option<Quotes>,
}

/// A `content` value (css-content-3 §2): `normal`, `none`, or a list of content items (the `/ alt`
/// text after them is accessibility text and generates nothing).
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Normal,
    None,
    Items(Vec<ContentItem>),
}

/// One `content` item. Counter styles are the css-counter-styles-3 predefined styles, coded as
/// `PaintStyle::list_style` codes (0 none, 1 disc, 2 circle, 3 square, 4 decimal, 5/6 lower/upper
/// alpha, 7/8 lower/upper roman, 9 decimal-leading-zero, 10 lower-greek).
#[derive(Clone, Debug, PartialEq)]
pub enum ContentItem {
    Str(String),
    /// `attr(<name>)`: the originating element's attribute (css-values-5 §7.7; "" when absent).
    Attr(String),
    Counter(String, u8),
    Counters(String, String, u8),
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
    /// `url()`: a replaced inline image, through the page's image store.
    Url(String),
}

/// A `quotes` value (css-content-3 §3.1): `auto` (the UA's pairs for the content language), `none`,
/// or explicit open/close pairs, outermost first.
#[derive(Clone, Debug, PartialEq)]
pub enum Quotes {
    Auto,
    None,
    Pairs(Vec<(String, String)>),
}

/// A css-counter-styles-3 predefined counter style name as a `list_style` code; `None` = not one.
pub fn counter_style_code(name: &str) -> Option<u8> {
    Some(match name {
        "none" => 0,
        "disc" => 1,
        "circle" => 2,
        "square" => 3,
        "decimal" => 4,
        "lower-alpha" | "lower-latin" => 5,
        "upper-alpha" | "upper-latin" => 6,
        "lower-roman" => 7,
        "upper-roman" => 8,
        "decimal-leading-zero" => 9,
        "lower-greek" => 10,
        _ => return None,
    })
}

/// Parses a `content` value from its CSS text (re-tokenized by css_core, so strings arrive unescaped:
/// `"\\2713"` is U+2713). `None` = invalid (the declaration is ignored, css-syntax-3 §8).
pub fn parse_content<H: Host>(value: &str) -> Option<Content> {
    use css_core::{Token, CV};
    let cvs = css_core::parser::parse_component_values(value);
    let toks: Vec<&CV> = cvs.iter().filter(|c| !c.is_whitespace()).collect();
    if let [CV::Token(Token::Ident(k))] = toks.as_slice() {
        match k.to_ascii_lowercase().as_str() {
            "normal" | "initial" | "unset" | "revert" => return Some(Content::Normal),
            "none" => return Some(Content::None),
            _ => {}
        }
    }
    // A function's arguments: idents and strings, commas dropped.
    fn args(a: &[CV]) -> Vec<&CV> {
        a.iter().filter(|c| !c.is_whitespace() && !matches!(c, CV::Token(Token::Comma))).collect()
    }
    let style_of = |c: Option<&&CV>| -> Option<u8> {
        match c {
            None => Some(4),
            Some(CV::Token(Token::Ident(s))) => Some(counter_style_code(&s.to_ascii_lowercase()).unwrap_or(4)),
            _ => None,
        }
    };
    let mut items = Vec::new();
    for c in toks {
        let item = match c {
            CV::Token(Token::String(s)) => ContentItem::Str(s.clone()),
            CV::Token(Token::Url(u)) => ContentItem::Url(u.clone()),
            CV::Token(Token::Delim('/')) => break,
            CV::Token(Token::Ident(k)) => match k.to_ascii_lowercase().as_str() {
                "open-quote" => ContentItem::OpenQuote,
                "close-quote" => ContentItem::CloseQuote,
                "no-open-quote" => ContentItem::NoOpenQuote,
                "no-close-quote" => ContentItem::NoCloseQuote,
                _ => return None,
            },
            CV::Function(f, a) => {
                let a = args(a);
                match (f.to_ascii_lowercase().as_str(), a.as_slice()) {
                    ("url", [CV::Token(Token::String(u))]) => ContentItem::Url(u.clone()),
                    ("attr", [CV::Token(Token::Ident(n)), ..]) => ContentItem::Attr(n.to_ascii_lowercase()),
                    ("counter", [CV::Token(Token::Ident(n)), rest @ ..]) if rest.len() <= 1 => {
                        ContentItem::Counter(n.clone(), style_of(rest.first())?)
                    }
                    ("counters", [CV::Token(Token::Ident(n)), CV::Token(Token::String(sep)), rest @ ..]) if rest.len() <= 1 => {
                        ContentItem::Counters(n.clone(), sep.clone(), style_of(rest.first())?)
                    }
                    (other, _) => {
                        H::record_css(&format!("content-value:{}()", clip(other)));
                        return None;
                    }
                }
            }
            _ => return None,
        };
        items.push(item);
    }
    if items.is_empty() {
        return None;
    }
    Some(Content::Items(items))
}

/// Parses `counter-reset` / `counter-increment` / `counter-set` (css-lists-3 §4.1-4.3): `none`, or
/// counter names each optionally followed by an integer (`default` when omitted).
pub fn parse_counter_list(value: &str, default: i32) -> Option<Vec<(String, i32)>> {
    use css_core::{Token, CV};
    let cvs = css_core::parser::parse_component_values(value);
    let toks: Vec<&CV> = cvs.iter().filter(|c| !c.is_whitespace()).collect();
    if let [CV::Token(Token::Ident(k))] = toks.as_slice() {
        if matches!(k.to_ascii_lowercase().as_str(), "none" | "initial" | "unset" | "revert") {
            return Some(Vec::new());
        }
    }
    let mut out: Vec<(String, i32)> = Vec::new();
    for c in toks {
        match c {
            CV::Token(Token::Ident(n)) if !matches!(n.to_ascii_lowercase().as_str(), "none" | "inherit") => {
                out.push((n.clone(), default));
            }
            // `reversed(<name>)` (css-lists-3 §4.1): counted as the plain counter here.
            CV::Function(f, a) if f.eq_ignore_ascii_case("reversed") => {
                let n = a.iter().find_map(|c| c.ident())?;
                out.push((n.to_string(), default));
            }
            CV::Token(Token::Number(n)) => {
                let last = out.last_mut()?;
                last.1 = n.value as i32;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Parses `quotes` (css-content-3 §3.1).
pub fn parse_quotes(value: &str) -> Option<Quotes> {
    use css_core::{Token, CV};
    let cvs = css_core::parser::parse_component_values(value);
    let toks: Vec<&CV> = cvs.iter().filter(|c| !c.is_whitespace()).collect();
    if let [CV::Token(Token::Ident(k))] = toks.as_slice() {
        return match k.to_ascii_lowercase().as_str() {
            "auto" | "match-parent" | "initial" | "unset" | "revert" => Some(Quotes::Auto),
            "none" => Some(Quotes::None),
            _ => None,
        };
    }
    let strs: Vec<String> = toks
        .iter()
        .map(|c| match c {
            CV::Token(Token::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    if strs.is_empty() || strs.len() % 2 != 0 {
        return None;
    }
    Some(Quotes::Pairs(strs.chunks(2).map(|p| (p[0].clone(), p[1].clone())).collect()))
}

impl<H: Host> SpecifiedStyle<H> {
    /// Folds the specified layout declarations into a taffy style.
    pub fn fold_into(&self, node_style: &mut Style) {
        if let Some(d) = self.display { node_style.display = d; }
        if let Some(fd) = self.flex_direction { node_style.flex_direction = fd; }
        if self.flex_container == Some(true) {
            // CSS initial values for a flex container, replacing the
            // column/wrap/baseline defaults of Aether's block approximation.
            if self.flex_direction.is_none() {
                node_style.flex_direction = FlexDirection::Row;
            }
            node_style.flex_wrap = self.flex_wrap.unwrap_or(taffy::style::FlexWrap::NoWrap);
            node_style.align_items = self.align_items.unwrap_or(taffy::style::AlignItems::NORMAL); node_style.align_content = self.align_content.unwrap_or(taffy::style::AlignContent::NORMAL);
        } else if let Some(w) = self.flex_wrap {
            node_style.flex_wrap = w;
        }
        if let Some(g) = self.row_gap { node_style.gap.height = g; }
        if let Some(g) = self.column_gap { node_style.gap.width = g; }
        if let Some(v) = self.flex_grow { node_style.flex_grow = v; }
        if let Some(v) = self.flex_shrink { node_style.flex_shrink = v; }
        if let Some(v) = self.flex_basis { node_style.flex_basis = v; }
        if let Some(w) = self.width { node_style.size.width = w; }
        if let Some(h) = self.height {
            node_style.size.height = h;
            node_style.min_size.height = lpa(h);
        }
        // min wins over max in taffy, and blocks carry a UA min-height
        // default — a specified max must clear it (an explicit min-* below
        // still overrides, it folds after).
        if let Some(v) = self.max_width {
            node_style.max_size.width = lpa(v);
            node_style.min_size.width = LengthPercentageAuto::auto();
        }
        if let Some(v) = self.max_height {
            node_style.max_size.height = lpa(v);
            node_style.min_size.height = LengthPercentageAuto::auto();
        }
        if let Some(v) = self.min_width { node_style.min_size.width = lpa(v); }
        if let Some(v) = self.min_height { node_style.min_size.height = lpa(v); }
        let [pt, pr, pb, pl] = self.padding;
        if let Some(v) = pt { node_style.padding.top = v; }
        if let Some(v) = pr { node_style.padding.right = v; }
        if let Some(v) = pb { node_style.padding.bottom = v; }
        if let Some(v) = pl { node_style.padding.left = v; }
        let [mt, mr, mb, ml] = self.margin;
        if let Some(v) = mt { node_style.margin.top = v; }
        if let Some(v) = mr { node_style.margin.right = v; }
        if let Some(v) = mb { node_style.margin.bottom = v; }
        if let Some(v) = ml { node_style.margin.left = v; }
        if let Some(pos) = self.position {
            node_style.position = pos;
        }
        if let Some(r) = self.aspect_ratio {
            node_style.aspect_ratio = r;
        }
        // css-position-3 §3.4: a sticky box sits at its normal-flow place; its insets are the sticky
        // constraint the painter applies (`PaintStyle::inset_px`), not a relative offset.
        if self.paint.position_kind != Some(4) {
            if let Some(v) = self.inset_top { node_style.inset.top = v; }
            if let Some(v) = self.inset_left { node_style.inset.left = v; }
            if let Some(v) = self.inset_right { node_style.inset.right = v; }
            if let Some(v) = self.inset_bottom { node_style.inset.bottom = v; }
        }
        if let Some(j) = self.justify {
            node_style.justify_content = j;
        }
        // align-items only reaches ROW boxes. A column box here is the
        // block-container approximation, not a real column flex container:
        // an `align-items:center` written for a row would land on its CROSS
        // axis and centre the page's block content horizontally.
        if let Some(a) = self.align_items {
            if self.flex_container == Some(true)
                || matches!(node_style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse)
            {
                node_style.align_items = a;
            }
        }
        if let Some(a) = self.align_self {
            node_style.align_self = Some(a);
        } if let Some(j) = self.justify_items { node_style.justify_items = j; } if let Some(j) = self.justify_self { node_style.justify_self = Some(j); }
        if let Some(sides) = self.paint.border {
            let w = |i: usize| LengthPercentage::length(sides[i].map(|(w, _)| w).unwrap_or(0.0));
            node_style.border = Rect { top: w(0), right: w(1), bottom: w(2), left: w(3) };
        } else { border_width_fold(self.paint.border_width, &mut node_style.border); }
        if let Some(bs) = self.box_sizing {
            node_style.box_sizing = bs;
        }
        // AETHERSTYLE3 (SR103): size containment (css-contain-2 §3.2) — an auto height is laid out as if the
        // box were empty (0 plus its padding and border); inline-size containment leaves a block's width to its
        // containing block, which already does not depend on its content.
        if (self.paint.container_type == Some(1) || self.paint.contain_size == Some(1)) && self.height.is_none() {
            node_style.size.height = Dimension::length(0.0);
            node_style.min_size.height = LengthPercentageAuto::auto();
        }
        // AETHERSTYLE3 (SR103): a scroll container's automatic minimum size is 0 (css-flexbox-1 §4.5), which
        // taffy reads from its own `overflow`; scrollbars are overlays, so no gutter is reserved.
        if let Some(o) = self.paint.overflow {
            use taffy::style::Overflow as O;
            let map = |v: u8| match v {
                crate::scroll::HIDDEN | crate::scroll::AUTO => O::Hidden,
                crate::scroll::SCROLL => O::Scroll,
                crate::scroll::CLIP => O::Clip,
                _ => O::Visible,
            };
            node_style.overflow = taffy::geometry::Point { x: map(o[0]), y: map(o[1]) };
            node_style.scrollbar_width = 0.0;
            // AETHERSTYLE3B (SR115): `scrollbar-gutter: stable` (css-overflow-3 §6.1) reserves the vertical
            // bar's space on a scroll container whether or not it overflows — taffy reserves `scrollbar_width`
            // on the inline-end edge for a `Scroll` y axis; `both-edges` mirrors it on the start edge.
            let scroller = o.iter().any(|v| matches!(*v, crate::scroll::HIDDEN | crate::scroll::SCROLL | crate::scroll::AUTO));
            if scroller && matches!(self.paint.scrollbar_gutter, Some(1 | 2)) {
                node_style.overflow.y = O::Scroll;
                node_style.scrollbar_width = crate::scroll::SCROLLBAR;
                if self.paint.scrollbar_gutter == Some(2) {
                    let raw = node_style.padding.left.into_raw();
                    if raw.tag() == taffy::style::CompactLength::LENGTH_TAG {
                        node_style.padding.left = LengthPercentage::length(raw.value() + crate::scroll::SCROLLBAR);
                        // The start-edge gutter is padding to taffy: under content-box sizing it would grow
                        // the box, so a fixed width gives it up (the gutter lives inside the specified width).
                        let w = node_style.size.width.into_raw();
                        if node_style.box_sizing == taffy::style::BoxSizing::ContentBox && w.tag() == taffy::style::CompactLength::LENGTH_TAG {
                            node_style.size.width = Dimension::length((w.value() - crate::scroll::SCROLLBAR).max(0.0));
                        }
                    }
                }
            }
        }
    }
}

/// One declaration, shared by the stylesheet cascade and inline styles.
/// Values arrive as raw strings so function values (rgb()...) parse uniformly.
pub fn apply_declaration<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    let value = value.trim();
    // AETHERSTYLE3 (SR103): transitions and animations (css-transitions-1, css-animations-1).
    let unprefixed = prop
        .strip_prefix("-webkit-")
        .or_else(|| prop.strip_prefix("-moz-"))
        .or_else(|| prop.strip_prefix("-o-"))
        .unwrap_or(prop);
    if unprefixed.starts_with("transition") || unprefixed.starts_with("animation") {
        if !crate::anim::apply_declaration(unprefixed, value, style) && !is_neutral_keyword(value) {
            H::record_css(&format!("{}-value:{}", unprefixed, clip(value)));
        }
        return;
    }
    if crate::anim::is_animatable(prop) {
        style.animatable.retain(|(p, _)| p != prop);
        style.animatable.push((prop.to_string(), value.to_string()));
    }
    let fr = font_rel_record(prop, value, &mut style.paint); // Kept in cascade order for the font-relative re-resolution; a refusing arm undoes it (SR1055).
    let value = noninherited_initial(prop, value); if css_wide(prop, value, &mut style.paint) { return; } if css_wide_box(prop, value, style) { return; } if apply_revert_spec(prop, value, style, cx) { return; } match prop { "all" => apply_css_wide_all(value, style, cx),
        "display" => {
            // Outer display type (css-display-3 §2.1) for the inline
            // whitespace pass: 0 inline, 1 block-level, 2 inline-level box.
            style.paint.display_kind = match value {
                "inline" | "contents" => Some(0),
                "inline-block" | "inline-flex" | "inline-grid" | "inline-table"
                | "-webkit-inline-box" | "-webkit-inline-flex" | "-ms-inline-flexbox" => Some(2),
                "none" | "inherit" | "initial" | "unset" | "revert" => None,
                _ => Some(1),
            };
            if !matches!(value, "inherit" | "initial" | "unset" | "revert") {
                style.paint.list_item = Some(value == "list-item");
            }
            apply_display(value, style);
        }
        // css-lists-3 §3: the marker style (inherited); `list-style` takes
        // its type keyword (position/image parts are not painted).
        "list-style-type" | "list-style" => {
            for part in value.split_whitespace() {
                let code = match part {
                    "none" => Some(0),
                    "disc" => Some(1),
                    "circle" => Some(2),
                    "square" => Some(3),
                    "decimal" => Some(4),
                    "lower-alpha" | "lower-latin" => Some(5),
                    "upper-alpha" | "upper-latin" => Some(6),
                    "lower-roman" => Some(7),
                    "upper-roman" => Some(8),
                    "decimal-leading-zero" => Some(9),
                    _ => None,
                };
                if code.is_some() {
                    style.paint.list_style = code;
                }
            }
        }
        // AETHERGEN (SR71): generated content (css-content-3) and counters (css-lists-3 §4).
        "content" => {
            if let Some(c) = parse_content::<H>(value) {
                style.generated.content = Some(c);
            }
        }
        "counter-reset" | "counter-set" | "counter-increment" => {
            let list = parse_counter_list(value, if prop == "counter-increment" { 1 } else { 0 });
            if let Some(list) = list {
                match prop {
                    "counter-reset" => style.generated.counter_reset = Some(list),
                    "counter-set" => style.generated.counter_set = Some(list),
                    _ => style.generated.counter_increment = Some(list),
                }
            }
        }
        "quotes" => {
            if let Some(q) = parse_quotes(value) {
                style.generated.quotes = Some(q);
            }
        }
        "flex-direction" => match value {
            "row" => style.flex_direction = Some(FlexDirection::Row),
            "column" => style.flex_direction = Some(FlexDirection::Column),
            "row-reverse" => style.flex_direction = Some(FlexDirection::RowReverse),
            "column-reverse" => style.flex_direction = Some(FlexDirection::ColumnReverse),
            _ => {}
        },
        "flex-wrap" => match value {
            "wrap" => style.flex_wrap = Some(taffy::style::FlexWrap::Wrap),
            "wrap-reverse" => style.flex_wrap = Some(taffy::style::FlexWrap::WrapReverse),
            "nowrap" => style.flex_wrap = Some(taffy::style::FlexWrap::NoWrap),
            "inherit" | "initial" | "unset" | "revert" => {}
            other => H::record_css(&format!("flex-wrap-value:{}", other)),
        },
        "flex-flow" => {
            for part in value.split_whitespace() {
                match part {
                    "row" | "column" | "row-reverse" | "column-reverse" => {
                        apply_declaration("flex-direction", part, style, cx)
                    }
                    _ => apply_declaration("flex-wrap", part, style, cx),
                }
            }
        }
        "gap" | "grid-gap" => {
            let parts: Vec<&str> = split_top_level(value);
            let row = parts.first().and_then(|v| gap_length(v, cx));
            let col = parts.get(1).and_then(|v| gap_length(v, cx)).or(row);
            match (row, col) {
                (Some(r), Some(c)) => {
                    style.row_gap = Some(r);
                    style.column_gap = Some(c);
                }
                _ => { font_rel_undo(&mut style.paint, prop, &fr); H::record_css(&format!("gap-value:{}", clip(value))) }
            }
        }
        "row-gap" | "grid-row-gap" => match gap_length(value, cx) {
            Some(v) => style.row_gap = Some(v),
            None => { font_rel_undo(&mut style.paint, prop, &fr); H::record_css(&format!("gap-value:{}", clip(value))) }
        },
        "column-gap" | "grid-column-gap" => {
            // AETHERSTYLE3 (SR103): the multicol gutter too (css-multicol-1 §4.1; `normal` = 1em there).
            if prop == "column-gap" {
                if let Some(px) = parse_px(value, cx).filter(|_| !value.ends_with('%')) {
                    style.paint.col_gap = Some(Some(px));
                } else if value == "normal" {
                    style.paint.col_gap = Some(None);
                }
            }
            apply_column_gap(value, style, cx); if value != "normal" && parse_length_percentage_str(value, cx).is_none() { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "flex-grow" => match value.parse::<f32>() {
            Ok(v) if v >= 0.0 => style.flex_grow = Some(v),
            _ => H::record_css(&format!("flex-grow-value:{}", clip(value))),
        },
        "flex-shrink" => match value.parse::<f32>() {
            Ok(v) if v >= 0.0 => style.flex_shrink = Some(v),
            _ => H::record_css(&format!("flex-shrink-value:{}", clip(value))),
        },
        "flex-basis" => match parse_flex_basis(value, cx) {
            Some(v) => style.flex_basis = Some(v),
            None => { font_rel_undo(&mut style.paint, prop, &fr); H::record_css(&format!("flex-basis-value:{}", clip(value))) }
        },
        "flex" => match parse_flex_shorthand(value, cx) {
            Some((g, sh, b)) => {
                style.flex_grow = Some(g);
                style.flex_shrink = Some(sh);
                style.flex_basis = Some(b);
            }
            None => { font_rel_undo(&mut style.paint, prop, &fr); H::record_css(&format!("flex-value:{}", clip(value))) }
        },
        "width" | "min-width" | "max-width" => {
            let d = parse_dimension_str(value, cx); if d.is_none() && !(value.contains('(') && value.contains('%')) { font_rel_undo(&mut style.paint, prop, &fr); }
            let i = match prop { "width" => 0, "min-width" => 1, _ => 2 };
            // css-values-4 §10: a math function mixing % with lengths
            // resolves against the containing block's width — known only
            // after layout, so it is kept as an expression for remeasure.
            let mut m = style.paint.pct_math.clone().unwrap_or_default();
            m[i] = (d.is_none() && value.contains('(') && value.contains('%')).then(|| value.to_string());
            if d.is_some() || m[i].is_some() {
                style.paint.pct_math = Some(m);
            }
            if i == 0 {
                // AETHERFIT (SR189) / AETHERFIT2 (SR191): a sizing-keyword width (fit-content, min-content,
                // max-content, fit-content()) is content-sized, like `auto` (css-sizing-3 §5.2).
                style.paint.has_width =
                    Some(!(value.trim() == "auto" || d.is_some_and(|d| d.into_raw().is_sizing_keyword())));
            }
            match i {
                0 => style.width = d,
                1 => style.min_width = d,
                _ => style.max_width = d,
            }
        }
        "height" => { style.height = parse_dimension_str(value, cx); if style.height.is_none() { font_rel_undo(&mut style.paint, prop, &fr); } }
        "max-height" => { style.max_height = parse_dimension_str(value, cx); if style.max_height.is_none() { font_rel_undo(&mut style.paint, prop, &fr); } }
        "table-layout" => match value {
            "fixed" => style.paint.table_fixed = Some(true),
            "auto" => style.paint.table_fixed = Some(false),
            _ => {}
        },
        "border-collapse" => match value {
            "collapse" => style.paint.border_collapse = Some(true),
            "separate" => style.paint.border_collapse = Some(false),
            _ => {}
        },
        // AETHERCAPTIONSIDECSS (SR462): caption-side, top | bottom only (Blink's parser); inherited, so inherit / unset /
        // revert leave it unspecified and the reader walks up (tree.rs `caption_side_order`).
        "caption-side" => match value {
            "top" | "initial" => style.paint.caption_side = Some(false),
            "bottom" => style.paint.caption_side = Some(true),
            "inherit" | "unset" | "revert" => {}
            other => H::record_css(&format!("caption-side:{}", clip(other))),
        },
        "border-spacing" => {
            if let Some(px) = split_top_level(value).first().and_then(|v| parse_px(v, cx)) {
                style.paint.border_spacing = Some(px.max(0.0));
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "min-height" => { style.min_height = parse_dimension_str(value, cx); if style.min_height.is_none() { font_rel_undo(&mut style.paint, prop, &fr); } }
        // overflow hidden/clip/auto/scroll all CLIP paint here (no inner
        // scrollbars yet — clipping is the honest approximation; visible
        // overflow was smearing collapsed menus over the page).
        // AETHERSTYLE3 (SR103): css-overflow-3 §3 — `overflow: <x> <y>?` and the axis longhands (the
        // flow-relative `overflow-inline` / `-block` map in horizontal-tb). hidden / scroll / auto make a
        // scroll container with its own offset (`scroll`); every non-visible value clips paint.
        "overflow" | "overflow-x" | "overflow-y" | "overflow-inline" | "overflow-block" => {
            let code = |v: &str| -> Option<u8> {
                Some(match v {
                    "visible" => crate::scroll::VISIBLE,
                    "hidden" => crate::scroll::HIDDEN,
                    "clip" => crate::scroll::CLIP,
                    "scroll" => crate::scroll::SCROLL,
                    "auto" | "overlay" => crate::scroll::AUTO,
                    _ => return None,
                })
            };
            let parts: Vec<&str> = value.split_whitespace().collect();
            let mut o = style.paint.overflow.unwrap_or([crate::scroll::VISIBLE; 2]);
            let ok = match (prop, parts.as_slice()) {
                ("overflow", [a]) => code(a).map(|a| o = [a, a]).is_some(),
                ("overflow", [a, b]) => match (code(a), code(b)) {
                    (Some(a), Some(b)) => {
                        o = [a, b];
                        true
                    }
                    _ => false,
                },
                ("overflow-x" | "overflow-inline", [a]) => code(a).map(|a| o[0] = a).is_some(),
                ("overflow-y" | "overflow-block", [a]) => code(a).map(|a| o[1] = a).is_some(),
                _ => false,
            };
            if ok {
                style.paint.overflow = Some(o);
                style.paint.clip = Some(o != [crate::scroll::VISIBLE; 2]);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("{}-value:{}", prop, clip(value)));
            }
        }
        // AETHERSTYLE3B (SR115): css-overflow-3 §6.1, CSSOM View §5.1, css-scroll-snap-1 §5-§6.
        "scrollbar-gutter" => match value {
            "auto" => style.paint.scrollbar_gutter = Some(0),
            "stable" => style.paint.scrollbar_gutter = Some(1),
            "stable both-edges" | "both-edges stable" => style.paint.scrollbar_gutter = Some(2),
            v if is_neutral_keyword(v) => {}
            v => H::record_css(&format!("scrollbar-gutter-value:{}", clip(v))),
        },
        "scroll-behavior" => match value {
            "smooth" => style.paint.scroll_smooth = Some(true),
            "auto" => style.paint.scroll_smooth = Some(false),
            _ => {}
        },
        "scroll-snap-type" => {
            let parts: Vec<&str> = value.split_whitespace().collect();
            let axis = |a: &str| -> Option<u8> {
                Some(match a {
                    "none" => 0,
                    "x" | "inline" => 1,
                    "y" | "block" => 2,
                    "both" => 3,
                    _ => return None,
                })
            };
            let parsed = match parts.as_slice() {
                [a] => axis(a).map(|a| (a, 0)),
                [a, "mandatory"] => axis(a).filter(|a| *a != 0).map(|a| (a, 1)),
                [a, "proximity"] => axis(a).filter(|a| *a != 0).map(|a| (a, 0)),
                _ => None,
            };
            match parsed {
                Some(t) => style.paint.snap_type = Some(t),
                None if is_neutral_keyword(value) => {}
                None => H::record_css(&format!("scroll-snap-type-value:{}", clip(value))),
            }
        }
        "scroll-snap-align" => {
            let code = |a: &str| -> Option<u8> {
                Some(match a {
                    "none" => 0,
                    "start" => 1,
                    "end" => 2,
                    "center" => 3,
                    _ => return None,
                })
            };
            // `<block> <inline>?` — one value sets both axes
            let parts: Vec<Option<u8>> = value.split_whitespace().map(code).collect();
            match parts.as_slice() {
                [Some(b)] => style.paint.snap_align = Some([*b, *b]),
                [Some(b), Some(i)] => style.paint.snap_align = Some([*i, *b]),
                _ if is_neutral_keyword(value) => {}
                _ => H::record_css(&format!("scroll-snap-align-value:{}", clip(value))),
            }
        }
        "scroll-snap-stop" => match value {
            "always" => style.paint.snap_stop = Some(true),
            "normal" => style.paint.snap_stop = Some(false),
            _ => {}
        },
        "scroll-padding" | "scroll-padding-top" | "scroll-padding-right" | "scroll-padding-bottom" | "scroll-padding-left" => {
            let px = |v: &str| if v == "auto" { Some(0.0) } else { parse_px(v, cx).filter(|_| !v.ends_with('%')) };
            let mut sp = style.paint.scroll_padding.unwrap_or([0.0; 4]);
            let parts: Vec<Option<f32>> = split_top_level(value).into_iter().map(px).collect();
            let ok = if prop == "scroll-padding" {
                match parts.as_slice() {
                    [Some(a)] => { sp = [*a; 4]; true }
                    [Some(a), Some(b)] => { sp = [*a, *b, *a, *b]; true }
                    [Some(a), Some(b), Some(c)] => { sp = [*a, *b, *c, *b]; true }
                    [Some(a), Some(b), Some(c), Some(d)] => { sp = [*a, *b, *c, *d]; true }
                    _ => false,
                }
            } else if let [Some(v)] = parts.as_slice() {
                let i = match prop { "scroll-padding-top" => 0, "scroll-padding-right" => 1, "scroll-padding-bottom" => 2, _ => 3 };
                sp[i] = *v;
                true
            } else {
                false
            };
            if ok {
                style.paint.scroll_padding = Some(sp);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("{}-value:{}", prop, clip(value)));
            }
        }
        // AETHERSTYLE3C (SR122): css-scroll-snap-1 §6.3 — the snap area's outsets (lengths only; logical sides
        // through the element's own writing mode).
        "scroll-margin" | "scroll-margin-top" | "scroll-margin-right" | "scroll-margin-bottom" | "scroll-margin-left" => {
            let px = |v: &str| parse_px(v, cx).filter(|_| !v.ends_with('%'));
            let mut sm = style.paint.scroll_margin.unwrap_or([0.0; 4]);
            let parts: Vec<Option<f32>> = split_top_level(value).into_iter().map(px).collect();
            let ok = if prop == "scroll-margin" {
                match parts.as_slice() {
                    [Some(a)] => { sm = [*a; 4]; true }
                    [Some(a), Some(b)] => { sm = [*a, *b, *a, *b]; true }
                    [Some(a), Some(b), Some(c)] => { sm = [*a, *b, *c, *b]; true }
                    [Some(a), Some(b), Some(c), Some(d)] => { sm = [*a, *b, *c, *d]; true }
                    _ => false,
                }
            } else if let [Some(v)] = parts.as_slice() {
                let i = match prop { "scroll-margin-top" => 0, "scroll-margin-right" => 1, "scroll-margin-bottom" => 2, _ => 3 };
                sm[i] = *v;
                true
            } else {
                false
            };
            if ok {
                style.paint.scroll_margin = Some(sm);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("{}-value:{}", prop, clip(value)));
            }
        }
        "scroll-margin-block-start" | "scroll-margin-block-end" | "scroll-margin-inline-start" | "scroll-margin-inline-end" => {
            let mode = style.paint.writing_mode.unwrap_or(0);
            let (is, ie, bs, be) = crate::writing::logical_sides(mode, style.paint.rtl == Some(true));
            let side = match prop {
                "scroll-margin-block-start" => bs,
                "scroll-margin-block-end" => be,
                "scroll-margin-inline-start" => is,
                _ => ie,
            };
            apply_declaration(&format!("scroll-margin-{side}"), value, style, cx);
        }
        "scroll-margin-block" | "scroll-margin-inline" => {
            let parts: Vec<&str> = split_top_level(value);
            let (a, b) = match parts.as_slice() {
                [a] => (*a, *a),
                [a, b] => (*a, *b),
                _ => return H::record_css(&format!("{}-value:{}", prop, clip(value))),
            };
            apply_declaration(&format!("{prop}-start"), a, style, cx);
            apply_declaration(&format!("{prop}-end"), b, style, cx);
        }
        "padding" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_str(v, cx)) {
                style.padding = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length_percentage_str(value, cx) {
                style.padding[side_index(prop)] = Some(v);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "margin" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_auto_str(v, cx)) {
                style.margin = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length_percentage_auto_str(value, cx) {
                style.margin[side_index(prop)] = Some(v);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "position" => {
            style.paint.position_kind = match value {
                "static" => Some(0),
                "relative" => Some(1),
                "absolute" => Some(2),
                "fixed" => Some(3),
                "sticky" => Some(4),
                _ => None,
            };
            match value {
                "absolute" | "fixed" => style.position = Some(taffy::style::Position::Absolute),
                "static" | "relative" | "sticky" => style.position = Some(taffy::style::Position::Relative),
                other => H::record_css(&format!("position:{}", other)),
            }
        }
        "z-index" => {
            if value == "auto" {
                style.paint.z_index = Some(None);
            } else if let Ok(z) = value.parse::<i32>() {
                style.paint.z_index = Some(Some(z));
            }
        }
        "top" | "right" | "bottom" | "left" => {
            let v = parse_length_percentage_auto_str(value, cx); if v.is_none() { font_rel_undo(&mut style.paint, prop, &fr); }
            match prop {
                "top" => style.inset_top = v,
                "right" => style.inset_right = v,
                "bottom" => style.inset_bottom = v,
                _ => style.inset_left = v,
            }
            let side = match prop { "top" => 0, "right" => 1, "bottom" => 2, _ => 3 };
            set_inset_px(style, side, value, cx);
        }
        // css-position-3 §3.1: the 1-4 value shorthand of top/right/bottom/left.
        "inset" => match parse_sides(value, |v| parse_length_percentage_auto_str(v, cx)) {
            Some(r) => {
                style.inset_top = Some(r.top);
                style.inset_right = Some(r.right);
                style.inset_bottom = Some(r.bottom);
                style.inset_left = Some(r.left);
                let parts = split_top_level(value);
                let pick = |i: usize| -> &str {
                    match (parts.len(), i) {
                        (1, _) => parts[0],
                        (2, 0 | 2) | (3, 0) => parts[0],
                        (2, _) | (3, 1 | 3) => parts[1],
                        (3, _) => parts[2],
                        _ => parts[i],
                    }
                };
                for i in 0..4 {
                    set_inset_px(style, i, pick(i), cx);
                }
            }
            None => { font_rel_undo(&mut style.paint, prop, &fr);
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("inset-value:{}", clip(value)));
                }
            }
        },
        // css-sizing-4 §5: `auto` | `<ratio>` | `auto && <ratio>`.
        "aspect-ratio" => match parse_aspect_ratio(value) {
            Some(r) => style.aspect_ratio = Some(r),
            None => {
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("aspect-ratio-value:{}", clip(value)));
                }
            }
        },
        // css-transforms-1 §5 / css-transforms-2 §5.1 (AETHERSTYLE2, SR95): parsed here, drawn by the painter.
        "transform" => match parse_transform_list(value, cx) {
            Some(list) => style.paint.transform = Some(list),
            None => {
                if !matches!(value, "inherit" | "initial" | "unset" | "revert") {
                    H::record_css(&format!("transform-value:{}", clip(value)));
                }
            }
        },
        "translate" | "rotate" | "scale" => match parse_individual_transform(prop, value, cx) {
            Some(op) => match prop {
                "translate" => style.paint.tf_translate = Some(op),
                "rotate" => style.paint.tf_rotate = Some(op),
                _ => style.paint.tf_scale = Some(op),
            },
            None => {
                if !matches!(value, "inherit" | "initial" | "unset" | "revert") {
                    H::record_css(&format!("{}-value:{}", prop, clip(value)));
                }
            }
        },
        "transform-origin" => match parse_position_pair(value, cx, true) {
            Some(o) => style.paint.transform_origin = Some(o),
            None => {
                if !matches!(value, "inherit" | "initial" | "unset" | "revert") {
                    H::record_css(&format!("transform-origin-value:{}", clip(value)));
                }
            }
        },
        // css-transforms-2 §4: a flattened 3D context and the hidden back face are what 2D paints anyway.
        "transform-style" | "backface-visibility" | "perspective" | "perspective-origin" | "transform-box" => {}
        // css-masking-1 §5.1: a basic shape against the border box.
        "clip-path" => match parse_clip_path(value, cx) {
            Some(shape) => style.paint.clip_path = Some(shape),
            None => {
                if !matches!(value, "inherit" | "initial" | "unset" | "revert") {
                    let what = if value.contains("url(") { "url" } else { clip(value) };
                    H::record_css(&format!("clip-path-value:{}", what));
                }
            }
        },
        // css-will-change-1 §2: a hint a UA may ignore.
        "will-change" => {}
        // AETHERSTYLE3 (SR103): css-contain-3 §3 — query containers (the host's second cascade evaluates
        // `@container` against their sizes) and size containment (`fold_into`).
        // AETHERSTYLE3C (SR122): `normal | [ [ size | inline-size ] || scroll-state ]` (css-conditional-5 §2.1).
        "container-type" => {
            let (mut ty, mut scroll, mut ok) = (None, false, true);
            for k in value.split_whitespace() {
                match k {
                    "normal" if ty.is_none() && !scroll => ty = Some(0),
                    "size" if ty.is_none() => ty = Some(1),
                    "inline-size" if ty.is_none() => ty = Some(2),
                    "scroll-state" if !scroll && ty != Some(0) => scroll = true,
                    _ => ok = false,
                }
            }
            if ok && (ty.is_some() || scroll) {
                style.paint.container_type = Some(ty.unwrap_or(0));
                style.paint.container_scroll = Some(scroll);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("container-type-value:{}", clip(value)));
            }
        }
        "container-name" => {
            let names: Vec<String> = value.split_whitespace().filter(|n| *n != "none").map(|n| n.to_string()).collect();
            style.paint.container_name = Some(names);
        }
        "container" => {
            let (names, ty) = match split_top_level_slash(value) { // AETHERCSSSPLITAUDIT (SR426): the depth-0 `/`
                (n, Some(t)) => (n.trim(), Some(t.trim())),
                (_, None) => (value, None),
            };
            apply_declaration("container-name", names, style, cx);
            apply_declaration("container-type", ty.unwrap_or("normal"), style, cx);
        }
        "contain" => {
            let mut size = 0u8;
            let mut paint = false;
            for k in value.split_whitespace() {
                match k {
                    "size" => size = 1,
                    "inline-size" if size != 1 => size = 2,
                    "strict" => {
                        size = 1;
                        paint = true;
                    }
                    "content" | "paint" => paint = true,
                    "none" | "layout" | "style" => {}
                    _ => {}
                }
            }
            style.paint.contain_size = Some(size);
            if paint {
                style.paint.clip = Some(true);
            }
        }
        // AETHERSTYLE3 (SR103): css-writing-modes-4 §3.1 / §5.1 (laid out by `writing`).
        "writing-mode" => match crate::writing::parse_writing_mode(value) {
            Some(m) => style.paint.writing_mode = Some(m),
            None if is_neutral_keyword(value) => {}
            None => H::record_css(&format!("writing-mode-value:{}", clip(value))),
        },
        // AETHERSTYLE3C (SR122): css-writing-modes-4 §9.1 (`-webkit-text-combine: horizontal` is the legacy `all`).
        "text-combine-upright" | "-webkit-text-combine" | "-ms-text-combine-horizontal" => match value {
            "all" | "horizontal" => style.paint.text_combine = Some(true),
            "none" => style.paint.text_combine = Some(false),
            v if is_neutral_keyword(v) => {}
            v => H::record_css(&format!("text-combine-upright-value:{}", clip(v))),
        },
        "text-orientation" => match value {
            "mixed" => style.paint.text_orientation = Some(0),
            // AETHERSTYLE3B (SR115): painted upright (`writing::upright`).
            "upright" => style.paint.text_orientation = Some(1),
            "sideways" | "sideways-right" => style.paint.text_orientation = Some(2),
            _ => {}
        },
        // AETHERSTYLE3 (SR103): css-multicol-1 §3-§6 (laid out by `multicol`).
        "column-count" => match value {
            "auto" => style.paint.col_count = Some(None),
            v => match v.parse::<u32>() {
                Ok(n) if n > 0 => style.paint.col_count = Some(Some(n)),
                _ if is_neutral_keyword(v) => {}
                _ => H::record_css(&format!("column-count-value:{}", clip(v))),
            },
        },
        "column-width" => match value {
            "auto" => style.paint.col_width = Some(None),
            v => match parse_px(v, cx).filter(|px| *px >= 0.0 && !v.ends_with('%')) {
                Some(px) => style.paint.col_width = Some(Some(px)),
                None if is_neutral_keyword(v) => {}
                None => H::record_css(&format!("column-width-value:{}", clip(v))),
            },
        },
        "columns" => {
            // `<'column-width'> || <'column-count'>`: an omitted half resets to auto.
            let (mut count, mut width, mut ok) = (None, None, true);
            for part in split_top_level(value) {
                if part == "auto" {
                    continue;
                }
                if let Ok(n) = part.parse::<u32>() {
                    ok &= n > 0 && count.is_none();
                    count = Some(n);
                } else if let Some(px) = parse_px(part, cx).filter(|_| !part.ends_with('%')) {
                    ok &= width.is_none();
                    width = Some(px);
                } else {
                    ok = false;
                }
            }
            if ok {
                style.paint.col_count = Some(count);
                style.paint.col_width = Some(width);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("columns-value:{}", clip(value)));
            }
        }
        "column-fill" => match value {
            "balance" | "balance-all" => style.paint.col_fill = Some(0),
            "auto" => style.paint.col_fill = Some(1),
            _ => {}
        },
        "column-rule" | "column-rule-width" | "column-rule-style" | "column-rule-color" => {
            for part in split_top_level(value) {
                let st = match part {
                    "none" | "hidden" => Some(crate::multicol::RULE_NONE),
                    p => border_style_code(p),
                };
                let w = match part {
                    "thin" => Some(1.0),
                    "medium" => Some(3.0),
                    "thick" => Some(5.0),
                    p => parse_px(p, cx),
                };
                if prop != "column-rule-color" && prop != "column-rule-width" && st.is_some() {
                    style.paint.col_rule_style = st;
                } else if prop != "column-rule-color" && prop != "column-rule-style" && w.is_some() {
                    style.paint.col_rule_width = w;
                } else if prop != "column-rule-width" && prop != "column-rule-style" {
                    if let Some(c) = parse_color_str::<H>(part) {
                        style.paint.col_rule_color = Some(c);
                    }
                }
            }
        }
        // AETHERSTYLE3B (SR115): css-multicol-1 §6.1 — a spanning element splits the column flow (`multicol`).
        "column-span" => match value {
            "none" => style.paint.col_span = Some(false),
            "all" => style.paint.col_span = Some(true),
            v if is_neutral_keyword(v) => {}
            v => H::record_css(&format!("column-span-value:{}", clip(v))),
        },
        // AETHERSTYLE3C (SR122): css-break-3 §5.4 — how a decorated box split across columns paints (`multicol`).
        "box-decoration-break" => match value {
            "slice" => style.paint.deco_clone = Some(false),
            "clone" => style.paint.deco_clone = Some(true),
            v if is_neutral_keyword(v) => {}
            v => H::record_css(&format!("box-decoration-break-value:{}", clip(v))),
        },
        // AETHERSTYLE3 (SR103): Filter Effects 1 §5 / Filter Effects 2 §3 — the list svg_core parses.
        "filter" | "backdrop-filter" => {
            let v = value.trim();
            let parsed = if v.eq_ignore_ascii_case("none") {
                Some(None)
            } else if crate::effects::filter_valid(v) {
                Some(Some(v.to_string()))
            } else {
                None
            };
            match parsed {
                Some(f) if prop == "filter" => style.paint.filter = Some(f),
                Some(f) => style.paint.backdrop_filter = Some(f),
                None if is_neutral_keyword(v) => {}
                None => H::record_css(&format!("{}-value:{}", prop, clip(v))),
            }
        }
        // css-break-3 §3: break controls apply only to fragmented flows; Aether paginates nothing and
        // lays out no columns, so there is nothing for them to do (the column-* properties stay ledgered).
        "break-before" | "break-after" | "break-inside" | "page-break-before" | "page-break-after"
        | "page-break-inside" | "-webkit-column-break-before" | "-webkit-column-break-after"
        | "-webkit-column-break-inside" => {}
        // Float approximation: no real float layout (text does not wrap
        // around the box), but a floated box sizes to content and hugs its
        // edge instead of stretching full width.
        "vertical-align" => {
            style.paint.vertical_align = match value {
                "baseline" => Some((0, 0.0)),
                "sub" => Some((1, 0.0)),
                "super" => Some((2, 0.0)),
                "middle" => Some((3, 0.0)),
                "text-top" => Some((4, 0.0)),
                "text-bottom" => Some((5, 0.0)),
                "top" => Some((6, 0.0)),
                "bottom" => Some((7, 0.0)),
                v if v.ends_with('%') => v.trim_end_matches('%').trim().parse::<f32>().ok().map(|p| (9, p / 100.0)),
                v => parse_px(v, cx).map(|px| (8, px)),
            }; if style.paint.vertical_align.is_none() { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        // `clear` (CSS 2.2 §9.5.2) is consumed only by a generated clearfix box (`layout::generated`);
        // a real element's clearance is still unimplemented, so it stays ledgered.
        "clear" => {
            // SR1039 fold: `property:clear` is the "unimplemented property" record @supports reads; the arm parses
            // the value, so it records the UNHANDLED value only (a real element's clearance stays owed: `clear-value:`).
            style.paint.clear = match value {
                "none" => Some(0),
                "left" | "inline-start" => Some(1),
                "right" | "inline-end" => Some(2),
                "both" => Some(3),
                _ => { if !is_neutral_keyword(value) { H::record_css(&format!("clear-value:{}", clip(value))); } style.paint.clear }
            };
        }
        "float" => match value {
            "left" => {
                style.width = Some(Dimension::auto());
                style.align_self = Some(taffy::style::AlignSelf::START);
                style.paint.float = Some(1);
            }
            "right" => {
                style.width = Some(Dimension::auto());
                style.align_self = Some(taffy::style::AlignSelf::END);
                style.paint.float = Some(2);
            }
            "none" => style.paint.float = Some(0),
            "inherit" | "initial" | "unset" => {}
            other => H::record_css(&format!("float:{}", other)),
        },
        // The image-replacement idiom: a huge negative text-indent pushes
        // the fallback text off-box while the element's own background
        // image stays visible (Wikipedia's sprite wordmark is exactly
        // this). Hiding the whole box would drop that image — only the
        // TEXT goes away. Small indents are ignored (no first-line
        // indent support).
        "text-indent" => {
            if let Some(px) = parse_px(value, cx) {
                if px <= -999.0 {
                    style.paint.text_hidden = Some(true);
                }
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        // text-align is INHERITED: it aligns the inline content of every
        // descendant block, not just this box's own flex children. The
        // paint field carries it down (see layout::remeasure); the justify
        // here is the immediate effect on this box's own row.
        "text-align-last" => { if let Some(c) = text_align_last_code::<H>(value) { style.paint.text_align_last = Some(c); } } "text-align" => match value { // AETHERTEXTALIGNLAST (SR1062): text-align-last parses through text_align_last_code
            "center" => {
                style.justify = Some(taffy::style::JustifyContent::CENTER);
                style.paint.text_align = Some(1);
            }
            // AETHERCENTERLAYOUT (SR421): -webkit-center (5) / -webkit-right (6) also place narrow block children
            // (tree.rs `place_narrow_blocks`); the inline formatter reads them as center / right.
            "-webkit-center" | "-moz-center" => {
                style.justify = Some(taffy::style::JustifyContent::CENTER);
                style.paint.text_align = Some(5);
            }
            "-webkit-right" | "-moz-right" => {
                style.justify = Some(taffy::style::JustifyContent::END);
                style.paint.text_align = Some(6);
            }
            // AETHERTEXTALIGNCODES (SR724): -webkit-left (7, Blink's align=left) is the physical left: blocks stay left in rtl.
            "-webkit-left" | "-moz-left" => {
                style.justify = Some(taffy::style::JustifyContent::START);
                style.paint.text_align = Some(7);
            }
            // codes: 0 start, 1 center, 2 right, 3 end, 4 left, 5 -webkit-center, 6 -webkit-right, 7 -webkit-left, 8 justify — start/end
            // resolve against `direction` (css-text-3 §6.1) where lines are placed (layout::inline).
            "right" => {
                style.justify = Some(taffy::style::JustifyContent::END);
                style.paint.text_align = Some(2);
            }
            "end" => {
                style.justify = Some(taffy::style::JustifyContent::END);
                style.paint.text_align = Some(3);
            }
            "left" => {
                style.justify = Some(taffy::style::JustifyContent::START);
                style.paint.text_align = Some(4);
            }
            "justify" => { style.justify = Some(taffy::style::JustifyContent::START); style.paint.text_align = Some(8); } "start" => { // AETHERTEXTALIGNJUSTIFY (SR1057): justify is its own code 8 (inline::justify_line)
                style.justify = Some(taffy::style::JustifyContent::START);
                style.paint.text_align = Some(0);
            }
            // css-text-4 §6.1: match-parent is the parent's value — inheritance, which an unspecified
            // text-align already is (the parent's start/end against ITS direction is the nuance left).
            "inherit" | "initial" | "unset" | "revert" | "match-parent" | "-webkit-match-parent" => {}
            other => H::record_css(&format!("text-align:{}", other)),
        },
        "justify-content" => match align_word::<H>(value) {
            "center" => style.justify = Some(taffy::style::JustifyContent::CENTER),
            "flex-end" | "end" => style.justify = Some(taffy::style::JustifyContent::END),
            "flex-start" | "start" | "normal" => style.justify = Some(taffy::style::JustifyContent::START),
            "space-between" => style.justify = Some(taffy::style::JustifyContent::SPACE_BETWEEN),
            "space-around" => style.justify = Some(taffy::style::JustifyContent::SPACE_AROUND),
            "space-evenly" => style.justify = Some(taffy::style::JustifyContent::SPACE_EVENLY),
            "inherit" | "initial" | "unset" => {}
            other => H::record_css(&format!("justify-content-value:{}", other)),
        },
        "background-color" => { bg_currentcolor::<H>(&mut style.paint, value); sys_slot(&mut style.paint.sys_background, value);
            if value.eq_ignore_ascii_case("transparent") {
                style.paint.bg_alpha = Some(0.0);
            } else if !is_neutral_keyword(value) {
                match H::parse_color_alpha(value) {
                    // css-color-4 §4.2: an rgba()/#rrggbbaa background is
                    // composited source-over, not painted opaque.
                    Some((c, a)) => {
                        style.paint.background = Some(c);
                        style.paint.bg_alpha = Some(a);
                    }
                    None => H::record_css(&format!("background-value:{}", clip(value))),
                }
            }
        }
        "background-image" if value.to_ascii_lowercase().contains("-gradient(") => {
            bg_image_layers::<H>(&mut style.paint, value);
        }
        "background" if value.to_ascii_lowercase().contains("-gradient(") => { bg_shorthand_reset::<H>(&mut style.paint, value); bg_currentcolor::<H>(&mut style.paint, value); sys_slot(&mut style.paint.sys_background, value);
            // A gradient layer (css-images-3 §3.1, §3.2) plus any colour outside it.
            bg_image_layers::<H>(&mut style.paint, value); bg_shorthand_layer_props::<H>(&mut style.paint, value);
            let lower = value.to_ascii_lowercase();
            let start = ["repeating-linear-gradient(", "repeating-radial-gradient(", "linear-gradient(", "radial-gradient("]
                .iter()
                .filter_map(|n| lower.find(n))
                .min()
                .unwrap_or(0);
            let mut depth = 0;
            let mut end = lower.len();
            for (i, c) in lower[start..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = start + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let rest = format!("{} {}", &value[..start], &value[end..]);
            for part in split_top_level(&rest) {
                if let Some(c) = parse_color_str::<H>(part) {
                    style.paint.background = Some(c);
                    style.paint.bg_alpha = Some(color_alpha::<H>(part));
                }
            }
        }
        "box-shadow" | "-webkit-box-shadow" => {
            style.paint.shadows = Some(H::parse_box_shadow(value));
        }
        "background-image" => match extract_css_url(value) {
            Some(_) => bg_image_layers::<H>(&mut style.paint, value),
            None => { bg_image_clear::<H>(&mut style.paint, value);
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("background-image-value:{}", clip(value)));
                }
            }
        },
        "background" => { bg_shorthand_reset::<H>(&mut style.paint, value); bg_currentcolor::<H>(&mut style.paint, value); sys_slot(&mut style.paint.sys_background, value);
            // Shorthand: a color and/or an image url, plus the repeat
            // keyword and a `position / size` pair when present.
            bg_shorthand_none::<H>(&mut style.paint, value); bg_shorthand_layer_props::<H>(&mut style.paint, value); if extract_css_url(value).is_some() {
                bg_image_layers::<H>(&mut style.paint, value);
            }
            let mut got_color = false;
            for part in split_top_level(value) {
                if part.starts_with("url(") { continue; }
                // Non-colour shorthand components (position, repeat,
                // attachment, origin/clip box, the `/` size separator) are
                // not colour candidates — probing them logged a bogus
                // `named-color:` miss for every sprite background on a page.
                if is_position_token(part)
                    || parse_repeat(part).is_some()
                    || part.starts_with('/')
                    || matches!(
                        part,
                        "scroll" | "fixed" | "local" | "border-box" | "padding-box"
                            | "content-box" | "text" | "cover" | "contain" | "none"
                    )
                {
                    continue;
                }
                // `transparent` is a colour layer at alpha 0 (`background: none transparent`).
                if part.eq_ignore_ascii_case("transparent") {
                    style.paint.bg_alpha = Some(0.0);
                    got_color = true;
                    break;
                }
                if let Some(c) = parse_color_str::<H>(part) {
                    style.paint.background = Some(c);
                    style.paint.bg_alpha = Some(color_alpha::<H>(part));
                    got_color = true;
                    break;
                }
            }
            // Function colors with spaces (rgb(1, 2, 3)) survive as whole-value.
            if !got_color && value.trim().eq_ignore_ascii_case("transparent") {
                style.paint.bg_alpha = Some(0.0);
            } else if !got_color && !is_neutral_keyword(value) && style.paint.bg_image.is_none() {
                match H::parse_color_alpha(value) {
                    Some((c, a)) => {
                        style.paint.background = Some(c);
                        style.paint.bg_alpha = Some(a);
                    }
                    None => H::record_css(&format!("background-value:{}", clip(value))),
                }
            }
        }
        "color" => { sys_slot(&mut style.paint.sys_color, value);
            if !is_neutral_keyword(value) {
                match parse_color_str::<H>(value) {
                    Some(c) => style.paint.color = Some(c),
                    None => H::record_css(&format!("color-value:{}", clip(value))),
                }
            }
        }
        // css-fonts-4 §2.1: the family list, interned (fonts::family_list); matched per family at layout.
        "font-family" => match H::intern_family_list(value) {
            Some(id) => style.paint.family = Some(id),
            None => {
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("font-family-value:{}", clip(value)));
                }
            }
        },
        // css-fonts-4 §2.3
        "font-stretch" => {
            if let Some(p) = parse_font_stretch(value) {
                style.paint.stretch = Some(p);
            }
        }
        "font-synthesis" | "font-synthesis-weight" | "font-synthesis-style" | "font-synthesis-small-caps" => apply_font_synthesis(prop, value, &mut style.paint), "font-variant" | "font-variant-caps" => apply_font_variant_caps(prop, value, &mut style.paint),
        // css-text-3 §7.1
        "word-spacing" => {
            if value == "normal" {
                style.paint.word_spacing = Some(0.0);
            } else if let Some(px) = parse_px(value, cx) {
                style.paint.word_spacing = Some(px);
            }
        }
        // css-writing-modes-4 §2.1 (the `dir` attribute maps onto it through the UA sheet)
        "direction" => match value {
            "rtl" => style.paint.rtl = Some(true),
            "ltr" => style.paint.rtl = Some(false),
            _ => {}
        },
        "unicode-bidi" => style.paint.unicode_bidi = unicode_bidi_code(value).or(style.paint.unicode_bidi),
        "color-scheme" => { if let Some(s) = parse_color_scheme(value) { style.paint.color_scheme = Some(s); } } // AETHERCSSWIDECOLORCONTROLSDARK (SR1031)
        // css-text-decor-3 §4: offset-x offset-y [blur] && [color], comma-separated layers.
        "text-shadow" => {
            let layers: Vec<H::Shadow> = H::parse_box_shadow(value).into_iter().filter(|s| !H::shadow_is_inset(s)).collect();
            style.paint.text_shadows = Some(layers);
        }
        // The `font` shorthand: [style] [weight] size[/line-height] family.
        // Legacy pages set their controls entirely through it
        // (`font:15px sans-serif`), so ignoring it left every such element
        // at the inherited size and family.
        "font" => {
            if matches!(value, "inherit" | "initial" | "unset" | "revert")
                || matches!(
                    value,
                    "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar"
                )
            {
                return apply_font_wide(value, style, cx);
            }
            let mut rest = value; let font_prior = reset_font_prefix(&mut style.paint);
            // Leading style / variant / weight / stretch keywords.
            loop {
                let Some((head, tail)) = rest.split_once(char::is_whitespace) else { break };
                let h = head.trim().to_ascii_lowercase();
                if h == "italic" || h == "oblique" {
                    style.paint.italic = Some(true);
                } else if h == "normal" || h == "small-caps" { if h == "small-caps" { style.paint.small_caps = Some(crate::paint::Caps::SmallCaps); }
                    // no effect here, but still part of the prefix
                } else if let Some(p) = parse_font_stretch(&h).filter(|_| !h.ends_with('%')) {
                    style.paint.stretch = Some(p);
                } else if let Some(w) = parse_font_weight(&h) {
                    style.paint.weight = Some(w);
                } else {
                    break;
                }
                rest = tail.trim_start();
            }
            // size[/line-height] then the family list. AETHERCSSSPLITAUDIT (SR426): the size component and its `/`
            // at paren depth 0 (`calc(10px + 2px) serif`, `12px/calc(10px + 6px) serif`).
            let rest = rest.trim_start();
            let size_part = split_top_level(rest).first().copied().unwrap_or("");
            let family = rest[size_part.len()..].trim();
            let (size, line) = split_top_level_slash(size_part);
            match parse_font_size(size, cx) {
                Some(px) => { style.paint.font_size = Some(px); reset_font_shorthand_rest(size, style, cx) }
                None => {
                    restore_font_prefix(&mut style.paint, font_prior); H::record_css(&format!("font-shorthand:{}", clip(value)));
                    return;
                }
            }
            if let Some(l) = line {
                apply_declaration("line-height", l, style, cx);
            }
            if !family.is_empty() {
                apply_declaration("font-family", family, style, cx);
            }
        }
        "place-content" => apply_place::<H>("align-content", "justify-content", value, style, cx), "place-items" => apply_place::<H>("align-items", "justify-items", value, style, cx), "place-self" => apply_place::<H>("align-self", "justify-self", value, style, cx), "align-self" => apply_align_self::<H>(align_word::<H>(value), style), "align-content" => apply_align_content(align_word::<H>(value), style), "align-items" => if let Some(Some(a)) = self_alignment::<H>("align-items", align_word::<H>(value), false) { style.align_items = Some(a); }, "justify-items" => if let Some(Some(a)) = self_alignment::<H>("justify-items", align_word::<H>(value), false) { style.justify_items = Some(a); }, "justify-self" => apply_justify_self::<H>(align_word::<H>(value), style),
        "font-size" => match parse_font_size(value, cx) {
            Some(px) => {
                style.paint.font_size = Some(px);
                // css-fonts-4 §2.5: em and % are of the PARENT's computed
                // font-size, rem of the root's; `smaller`/`larger` scale the
                // parent by 1.2 (Blink's ratio). Resolved in remeasure.
                let v = value.trim().to_ascii_lowercase();
                let num = |s: &str| s.trim().parse::<f32>().ok();
                style.paint.font_size_rel = Some(if v.contains('(') {
                    (2, 0.0)
                } else if let Some(n) = v.strip_suffix("rem").and_then(num) {
                    (1, n)
                } else if let Some(n) = v.strip_suffix("em").and_then(num) {
                    (0, n)
                } else if let Some(n) = v.strip_suffix('%').and_then(num) {
                    (0, n / 100.0)
                } else if v == "smaller" {
                    (0, 1.0 / 1.2)
                } else if v == "larger" {
                    (0, 1.2)
                } else {
                    (2, 0.0)
                });
            }
            None => match value.trim() {
                "smaller" => style.paint.font_size_rel = Some((0, 1.0 / 1.2)),
                "larger" => style.paint.font_size_rel = Some((0, 1.2)),
                // font-size is inherited: `inherit`/`unset` are the unspecified value; `initial` and
                // `revert` (the UA's) are `medium`.
                "inherit" | "unset" => {}
                "initial" | "revert" => {
                    style.paint.font_size = Some(BASE_FONT_PX);
                    style.paint.font_size_rel = Some((2, 0.0));
                }
                _ => H::record_css(&format!("font-size-value:{}", clip(value))),
            },
        },
        "font-weight" => match parse_font_weight(value) {
            Some(w) => style.paint.weight = Some(w),
            None => H::record_css(&format!("font-weight-value:{}", clip(value))),
        },
        "visibility" => match value {
            "hidden" | "collapse" => style.paint.hidden = Some(true),
            "visible" => style.paint.hidden = Some(false),
            "inherit" | "initial" | "unset" => {}
            other => H::record_css(&format!("visibility:{}", other)),
        },
        // opacity:0 hides like visibility:hidden; a fraction paints the
        // subtree as a group composited at that opacity (render::draw_node).
        "opacity" => {
            // Always specified, both ways: a higher-specificity opacity:1
            // has to be able to un-hide what an earlier opacity:0 rule hid,
            // and that only works if the winning declaration merges a value
            // instead of leaving the field unspecified.
            let a = match value.strip_suffix('%') {
                Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
                None => value.parse::<f32>().ok(),
            };
            if let Some(a) = a {
                let a = a.clamp(0.0, 1.0);
                style.paint.hidden = Some(a == 0.0);
                style.paint.opacity = Some(a);
            }
        }
        "line-height" => {
            // CSS 2.2 §10.8.1: a <number> is inherited as the number (each
            // element multiplies its own font-size); a <length> or
            // <percentage> computes to an absolute length, inherited as is.
            // Stored as > 0 multiplier, < 0 = -(px), 0 = normal, paint::LINE_HEIGHT_ZERO = an author 0 (SR843).
            let v = value.trim();
            let (em, ..) = cx.font;
            if matches!(v, "inherit" | "unset") {
                return; // inherited: the unspecified value
            }
            let lh = if matches!(v, "normal" | "initial" | "revert") {
                Some(0.0)
            } else if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()).filter(|p| *p >= 0.0) {
                Some(crate::paint::line_height_zero_or(pct * em, -(pct / 100.0 * em)))
            } else if let Some(n) = v.parse::<f32>().ok().filter(|n| *n >= 0.0) {
                Some(crate::paint::line_height_zero_or(n, n.max(0.0)))
            } else {
                parse_px(v, cx).filter(|px| *px >= 0.0 || v.contains('(')).map(|px| crate::paint::line_height_zero_or(px.max(0.0), -px.max(0.0)))
            };
            match lh {
                Some(f) => style.paint.line_height = Some(f),
                None => { font_rel_undo(&mut style.paint, prop, &fr); H::record_css(&format!("line-height-value:{}", clip(value))) }
            }
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let v = value.trim();
            let side = match prop {
                "border-top" => 0,
                "border-right" => 1,
                "border-bottom" => 2,
                _ => 3,
            };
            let mut sides = style.paint.border.unwrap_or_default();
            if v == "none" || v == "0" {
                sides[side] = None; border_width_store(&mut style.paint, 1 << side, None); side_alpha_set(&mut style.paint, 1 << side, 1.0);
                style.paint.border = Some(sides);
                return;
            }
            let mut width = border_medium();
            let mut color = (128, 128, 128); let mut alpha = 1.0f32;
            let mut got_any = false;
            for part in split_top_level(v) {
                if let Some(px) = parse_px(part, cx).filter(|_| part.ends_with("px") || is_font_relative(part)) {
                    width = px;
                    got_any = true;
                } else if let Some(k) = border_style_code(part) {
                    got_any = true;
                    let mut bs = style.paint.border_style.unwrap_or([0; 4]);
                    bs[side] = k;
                    style.paint.border_style = Some(bs);
                } else if let Some(c) = border_rgb::<H>(part) {
                    color = c;
                    got_any = true;
                    alpha = color_alpha::<H>(part);
                }
            }
            if got_any {
                sides[side] = Some((width, color)); border_cc(&mut style.paint, 1 << side, v); border_width_store(&mut style.paint, 1 << side, Some(width)); side_alpha_set(&mut style.paint, 1 << side, alpha);
                style.paint.border = Some(sides);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "text-decoration" | "text-decoration-line" => {
            let v = value.trim(); deco_shorthand_rest::<H>(prop, v, &mut style.paint, cx);
            if v.contains("underline") {
                style.paint.underline = Some(true);
            } else if v.starts_with("none") {
                style.paint.underline = Some(false);
                style.paint.line_through = Some(false);
            }
            if v.contains("line-through") {
                style.paint.line_through = Some(true);
            }
        }
        "text-decoration-color" => {
            if let Some(c) = deco_color_value::<H>(value) {
                style.paint.decoration_color = Some(c);
            }
        }
        "text-decoration-style" => {
            if let Some(k) = deco_style_code(value) {
                style.paint.decoration_style = Some(k);
            }
        }
        "text-decoration-thickness" => {
            if let Some(t) = deco_thickness_value(value, cx) {
                style.paint.decoration_thickness = Some(t);
            }
        }
        "text-underline-offset" => {
            if let Some(o) = underline_offset_value(value, cx) {
                style.paint.underline_offset = Some(o);
            }
        }
        "text-underline-position" => {
            if let Some(p) = underline_position_code(value) {
                style.paint.underline_position = Some(p);
            }
        }
        "white-space-collapse" | "text-wrap-mode" | "text-wrap" => apply_white_space_longhand::<H>(prop, value, style), "white-space" => {
            let code = match value {
                "normal" => Some(0),
                "nowrap" => Some(1),
                "pre" => Some(2),
                "pre-wrap" => Some(3),
                "pre-line" => Some(4),
                "break-spaces" => Some(5),
                _ => None,
            };
            if let Some(c) = code {
                style.paint.white_space = Some(c);
                style.paint.nowrap = Some(matches!(c, 1 | 2)); style.paint.ws_collapse = None; style.paint.ws_wrap = None;
            }
        }
        "word-break" => match value {
            "normal" => style.paint.word_break = Some(0),
            "break-all" => style.paint.word_break = Some(1),
            "keep-all" => style.paint.word_break = Some(2),
            // Legacy: word-break: break-word = overflow-wrap: anywhere.
            "break-word" => style.paint.overflow_wrap = Some(1),
            _ => {}
        },
        "overflow-wrap" | "word-wrap" => match value {
            "normal" => style.paint.overflow_wrap = Some(0),
            "break-word" | "anywhere" => style.paint.overflow_wrap = Some(1),
            _ => {}
        },
        "letter-spacing" => {
            if value == "normal" {
                style.paint.letter_spacing = Some(0.0);
            } else if let Some(px) = parse_px(value, cx) {
                style.paint.letter_spacing = Some(px);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "box-sizing" => match value {
            "border-box" => style.box_sizing = Some(taffy::style::BoxSizing::BorderBox),
            "content-box" => style.box_sizing = Some(taffy::style::BoxSizing::ContentBox),
            _ => {}
        },
        "font-style" => match value {
            "italic" | "oblique" => style.paint.italic = Some(true),
            "normal" => style.paint.italic = Some(false),
            _ => {}
        },
        "text-transform" => match value {
            "uppercase" => style.paint.text_transform = Some(1),
            "lowercase" => style.paint.text_transform = Some(2),
            "capitalize" => style.paint.text_transform = Some(3),
            "none" | "normal" => style.paint.text_transform = Some(0),
            _ => {}
        },
        "background-size" => {
            if !is_neutral_keyword(value) {
                if let Some((first, more)) = bg_layer_prop_list(value, |v| Some(v.to_string())) { style.paint.bg_size = Some(first); style.paint.bg_size_more = Some(more); }
            }
        }
        "background-position" => {
            if !is_neutral_keyword(value) {
                if let Some((first, more)) = bg_layer_prop_list(value, |v| Some(v.to_string())) { style.paint.bg_position = Some(first); style.paint.bg_position_more = Some(more); }
            }
        }
        "background-repeat" => {
            if let Some((r, more)) = bg_layer_prop_list(value, parse_repeat) {
                style.paint.bg_repeat = Some(r); style.paint.bg_repeat_more = Some(more);
            }
        }
        // Replaced-content fitting (CSS Images 3 §5.5–5.6): how a <video>
        // frame or poster sits in the element's content box.
        "object-fit" => {
            if let Some(f) = H::parse_object_fit(value) {
                style.paint.object_fit = Some(f);
            }
        }
        "object-position" => {
            if !is_neutral_keyword(value) {
                style.paint.object_position = Some(value.to_string());
            }
        }
        // mask-image (and the -webkit- alias) turns the box's background
        // paint into a stencil: the mask's alpha decides where the fill
        // lands. The icon-font-replacement idiom (a solid background-color
        // shaped by an SVG mask) is the whole reason UI icons exist as
        // one-colour boxes; without the stencil they paint as blobs.
        "mask-image" | "-webkit-mask-image" => match extract_css_url(value) {
            Some(u) => style.paint.mask_image = Some(u),
            None => {
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("mask-image-value:{}", clip(value)));
                }
            }
        },
        "mask" | "-webkit-mask" => {
            if let Some(u) = extract_css_url(value) {
                style.paint.mask_image = Some(u);
            }
        }
        "mask-size" | "-webkit-mask-size" => {
            if !is_neutral_keyword(value) {
                style.paint.mask_size = Some(value.to_string());
            }
        }
        "mask-position" | "-webkit-mask-position" => {
            if !is_neutral_keyword(value) {
                style.paint.mask_position = Some(value.to_string());
            }
        }
        "mask-repeat" | "-webkit-mask-repeat" => {
            if let Some(r) = parse_repeat(value) {
                style.paint.mask_repeat = Some(r);
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            if let Some(w) = parse_px(value, cx) {
                let side = match prop {
                    "border-top-width" => 0,
                    "border-right-width" => 1,
                    "border-bottom-width" => 2,
                    _ => 3,
                };
                border_width_store(&mut style.paint, 1 << side, Some(w));
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        // border-radius: parsed but not rendered (taffy doesn't support it yet).
        // At minimum, stops the property from appearing in unsupported ledgers.
        // css-backgrounds-3 §5.1: 1-4 corner radii (the elliptical `/`
        // part takes its horizontal radii), or one corner's longhand.
        // AETHERRADIUSCALC (SR416): a radius may be a math function (css-values-4 §10): the components and the `/` are
        // split at paren depth 0, so `calc(10px + 10px)` or `calc(20px / 2)` is one radius, not a dropped declaration.
        "border-radius" => {
            let horiz = split_top_level_slash(value).0;
            let parts: Option<Vec<f32>> = split_top_level(horiz).into_iter().map(|v| parse_radius(v, cx)).collect();
            if let Some(p) = parts {
                let r = match p.as_slice() {
                    [a] => [*a; 4],
                    [a, b] => [*a, *b, *a, *b],
                    [a, b, c] => [*a, *b, *c, *b],
                    [a, b, c, d] => [*a, *b, *c, *d],
                    _ => return font_rel_undo(&mut style.paint, prop, &fr),
                };
                style.paint.radius = Some(r);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "border-top-left-radius" | "border-top-right-radius"
        | "border-bottom-right-radius" | "border-bottom-left-radius" => {
            if let Some(v) = split_top_level(value).first().and_then(|v| parse_radius(v, cx)) {
                let i = match prop {
                    "border-top-left-radius" => 0,
                    "border-top-right-radius" => 1,
                    "border-bottom-right-radius" => 2,
                    _ => 3,
                };
                let mut r = style.paint.radius.unwrap_or([0.0; 4]);
                r[i] = v;
                style.paint.radius = Some(r);
            }
        }
        // A11YFACE (SR124): css-ui-4 §4 — the outline is its own paint (outside the border box, no space), never
        // the border: `button:focus { outline: none }` no longer removes the button's border, and `outline: 2px
        // solid` no longer draws one. The shorthand resets the longhands it omits (§4.1: style none, width
        // medium, colour currentColor).
        "outline" => {
            let (mut st, mut width, mut color) = (crate::paint::OUTLINE_NONE, crate::paint::OUTLINE_MEDIUM, None);
            let mut ok = !value.is_empty();
            for part in split_top_level(value) {
                if let Some(k) = crate::paint::outline_style_code(part) {
                    st = k;
                } else if let Some(px) = outline_width_value(part, cx) {
                    width = px;
                } else if let Some(c) = outline_color_value::<H>(part) {
                    color = c;
                } else {
                    ok = false;
                }
            }
            if !ok { font_rel_undo(&mut style.paint, prop, &fr); } if ok {
                style.paint.outline_style = Some(st);
                style.paint.outline_width = Some(width);
                style.paint.outline_color = Some(color);
            } else if !is_neutral_keyword(value) {
                H::record_css(&format!("outline-value:{}", clip(value)));
            }
        }
        "outline-style" => match crate::paint::outline_style_code(value) {
            Some(k) => style.paint.outline_style = Some(k),
            None if is_neutral_keyword(value) => {}
            None => H::record_css(&format!("outline-style-value:{}", clip(value))),
        },
        "outline-width" => {
            if let Some(px) = outline_width_value(value, cx) {
                style.paint.outline_width = Some(px);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "outline-color" => {
            if let Some(c) = outline_color_value::<H>(value) {
                style.paint.outline_color = Some(c);
            }
        }
        "caret-color" => {
            if let Some(c) = caret_color_value::<H>(value) {
                style.paint.caret_color = Some(c);
            }
        }
        "outline-offset" => {
            if let Some(px) = parse_px(value, cx) {
                style.paint.outline_offset = Some(px);
            } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "border" => { sys_border_slot(&mut style.paint, crate::paint::CC_SIDES, value);
            let v = value.trim();
            if v == "none" || v == "0" {
                style.paint.border = Some([None; 4]); border_width_store(&mut style.paint, crate::paint::CC_SIDES, None); side_alpha_set(&mut style.paint, crate::paint::CC_SIDES, 1.0);
                return;
            }
            let mut width = border_medium();
            let mut color = (128, 128, 128); let mut alpha = 1.0f32;
            let mut got_any = false;
            for part in split_top_level(v) {
                if let Some(px) = parse_px(part, cx).filter(|_| part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '.')) {
                    width = px;
                    got_any = true;
                } else if let Some(k) = border_style_code(part) {
                    got_any = true;
                    style.paint.border_style = Some([k; 4]);
                } else if let Some(c) = border_rgb::<H>(part) {
                    color = c;
                    got_any = true;
                    alpha = color_alpha::<H>(part);
                }
            }
            if got_any {
                style.paint.border = Some([Some((width, color)); 4]); border_cc(&mut style.paint, crate::paint::CC_SIDES, v); border_width_store(&mut style.paint, crate::paint::CC_SIDES, Some(width)); side_alpha_set(&mut style.paint, crate::paint::CC_SIDES, alpha);
            } else { font_rel_undo(&mut style.paint, prop, &fr);
                H::record_css(&format!("border-value:{}", clip(value)));
            }
        }
        "border-style" => {
            // A styleless border draws nothing, whatever its width says.
            if matches!(value.trim(), "none" | "hidden") {
                style.paint.border = Some([None; 4]);
            } else if let Some(r) = parse_sides(value, border_style_code) {
                style.paint.border_style = Some([r.top, r.right, r.bottom, r.left]); border_pending_create(&mut style.paint, crate::paint::CC_SIDES);
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            let side = side_index(prop.trim_end_matches("-style"));
            if let Some(k) = border_style_code(value) {
                let mut bs = style.paint.border_style.unwrap_or([0; 4]);
                bs[side] = k;
                style.paint.border_style = Some(bs); border_pending_create(&mut style.paint, 1 << side);
            } else if matches!(value, "none" | "hidden") {
                let mut sides = style.paint.border.unwrap_or_default();
                sides[side] = None;
                style.paint.border = Some(sides);
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => { sys_border_slot(&mut style.paint, 1 << side_index(prop.trim_end_matches("-color")), value); side_color_of::<H>(side_index(prop.trim_end_matches("-color")), value, &mut style.paint) } "border-color" => { sys_border_slot(&mut style.paint, crate::paint::CC_SIDES, value);
            if let Some(c) = border_color_of::<H>(&mut style.paint, value) {
                side_alpha_set(&mut style.paint, crate::paint::CC_SIDES, color_alpha::<H>(value));
                let Some(mut sides) = border_color_pend(&mut style.paint, c) else { return };
                for side in sides.iter_mut() {
                    let Some((w, _)) = *side else { continue };
                    *side = Some((w, c));
                }
                style.paint.border = Some(sides);
            }
        }
        "border-width" => {
            if let Some(w) = parse_px(value, cx) { border_width_store(&mut style.paint, crate::paint::CC_SIDES, Some(w)); } else { font_rel_undo(&mut style.paint, prop, &fr); }
        }
        "border-style" => {} // stroke style is uniform; nothing to record
        // css-ui-4 §5.1: `[<url> [<x> <y>]?,]* <keyword>` — images are not loaded, so the fallback
        // keyword (the last comma-separated item) is the cursor; `inherit`/`unset` leave it to the
        // parent (it is inherited), an unknown keyword invalidates the declaration.
        "cursor" => {
            let kw = value.rsplit(',').next().unwrap_or("").trim().to_ascii_lowercase();
            match cursor_keyword(&kw) {
                Some(k) => style.cursor = Some(k),
                None if matches!(kw.as_str(), "inherit" | "unset") => {}
                None => H::record_css(&format!("cursor-value:{}", clip(value))),
            }
        }
        // Vendor-prefixed spellings of properties this engine implements
        // with the SAME value grammar. Only these are aliased: the old
        // `-webkit-box-*` flexbox properties (box-flex/box-align/box-pack)
        // take different values and stay honestly unimplemented.
        other if vendor_alias_base(other).is_some() => apply_declaration(vendor_alias_base(other).unwrap_or(other), value, style, cx), // SR1039: the list is vendor_alias_base's
        other => {
            if !apply_logical(other, value, style, cx) {
                H::record_css(&format!("property:{}", other));
            }
        }
    }
}

/// The flex / grid `column-gap` (taffy's horizontal gap), as the arm always folded it.
fn apply_column_gap<H: Host>(value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    match gap_length(value, cx) {
        Some(v) => style.column_gap = Some(v),
        None => H::record_css(&format!("gap-value:{}", clip(value))),
    }
}

/// css-logical-1 §4-§6: a flow-relative property is re-applied as its physical longhand(s), through this
/// element's own `writing-mode` and `direction` (css-writing-modes-4 §6.4, `writing::logical_sides`;
/// AETHERSTYLE3 SR103 added the vertical modes). An inherited writing mode or rtl is not visible at
/// declaration time. `false` = not a logical property this engine maps.
fn apply_logical<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) -> bool {
    // AETHERSTYLE3 (SR103): through this element's own `writing-mode` (a vertical mode swaps the axes).
    let mode = style.paint.writing_mode.unwrap_or(crate::writing::HORIZONTAL_TB);
    let vertical = crate::writing::is_vertical(mode);
    let size = match prop {
        "inline-size" => Some(if vertical { "height" } else { "width" }),
        "block-size" => Some(if vertical { "width" } else { "height" }),
        "min-inline-size" => Some(if vertical { "min-height" } else { "min-width" }),
        "min-block-size" => Some(if vertical { "min-width" } else { "min-height" }),
        "max-inline-size" => Some(if vertical { "max-height" } else { "max-width" }),
        "max-block-size" => Some(if vertical { "max-width" } else { "max-height" }),
        _ => None,
    };
    if let Some(p) = size {
        apply_declaration(p, value, style, cx);
        return true;
    }
    let rtl = style.paint.rtl == Some(true);
    let (is_, ie, bs, be) = crate::writing::logical_sides(mode, rtl);
    for fam in ["margin", "padding", "inset", "border"] {
        let Some(rest) = prop.strip_prefix(fam).and_then(|r| r.strip_prefix('-')) else { continue };
        let (inline, rest) = if let Some(r) = rest.strip_prefix("inline") {
            (true, r)
        } else if let Some(r) = rest.strip_prefix("block") {
            (false, r)
        } else {
            return false;
        };
        let (start, end) = if inline { (is_, ie) } else { (bs, be) };
        let (edge, suffix) = if let Some(s) = rest.strip_prefix("-start") {
            (1u8, s)
        } else if let Some(s) = rest.strip_prefix("-end") {
            (2, s)
        } else {
            (0, rest)
        };
        let suffix_ok = match fam {
            "border" => matches!(suffix, "" | "-width" | "-style"),
            _ => suffix.is_empty(),
        };
        if !suffix_ok {
            return false;
        }
        let phys = |side: &str| -> String {
            if fam == "inset" { side.to_string() } else { format!("{fam}-{side}{suffix}") }
        };
        match edge {
            1 => apply_declaration(&phys(start), value, style, cx),
            2 => apply_declaration(&phys(end), value, style, cx),
            _ if fam == "border" => {
                apply_declaration(&phys(start), value, style, cx);
                apply_declaration(&phys(end), value, style, cx);
            }
            _ => {
                // `margin-inline: <start> <end>?` (css-logical-1 §4.2)
                let parts = split_top_level(value);
                let (a, b) = match parts.as_slice() {
                    [a] => (*a, *a),
                    [a, b] => (*a, *b),
                    _ => {
                        H::record_css(&format!("{}-value:{}", prop, clip(value)));
                        return true;
                    }
                };
                apply_declaration(&phys(start), a, style, cx);
                apply_declaration(&phys(end), b, style, cx);
            }
        }
        return true;
    }
    false
}

/// Records one side's px inset for the sticky painter (`PaintStyle::inset_px`): a length is kept, a
/// percentage or `auto` clears the side.
fn set_inset_px<H: Host>(style: &mut SpecifiedStyle<H>, side: usize, value: &str, cx: &ParseCx) {
    let v = value.trim();
    let px = if v.ends_with('%') || v == "auto" { None } else { parse_px(v, cx) };
    let mut sides = style.paint.inset_px.unwrap_or([None; 4]);
    sides[side] = px;
    style.paint.inset_px = Some(sides);
}

/// `aspect-ratio` (css-sizing-4 §5.1): Some(None) = `auto`; a degenerate ratio (a zero term) is `auto`.
pub fn parse_aspect_ratio(value: &str) -> Option<Option<f32>> {
    let v = value.trim().to_ascii_lowercase();
    let rest: String = v.split_whitespace().filter(|t| *t != "auto").collect::<Vec<_>>().join(" ");
    if rest.is_empty() {
        return if v.split_whitespace().any(|t| t == "auto") { Some(None) } else { None };
    }
    let (a, b) = match rest.split_once('/') {
        Some((a, b)) => (a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?),
        None => (rest.trim().parse::<f32>().ok()?, 1.0),
    };
    if a < 0.0 || b < 0.0 || !a.is_finite() || !b.is_finite() {
        return None;
    }
    Some(if a == 0.0 || b == 0.0 { None } else { Some(a / b) })
}

/// A `<length-percentage>` as a paint-time [`LenPct`].
fn len_pct(v: &str, cx: &ParseCx) -> Option<LenPct> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some([0.0, p / 100.0]);
    }
    parse_px(v, cx).filter(|x| x.is_finite()).map(|px| [px, 0.0])
}

/// An `<angle>` in radians (css-values-4 §7.1); a unitless 0 is accepted (css-transforms-1 §7).
fn parse_angle(v: &str) -> Option<f32> {
    let v = v.trim();
    let num = |s: &str| s.trim().parse::<f32>().ok();
    let r = if let Some(n) = v.strip_suffix("deg") {
        num(n)?.to_radians()
    } else if let Some(n) = v.strip_suffix("grad") {
        num(n)? * core::f32::consts::PI / 200.0
    } else if let Some(n) = v.strip_suffix("rad") {
        num(n)?
    } else if let Some(n) = v.strip_suffix("turn") {
        num(n)? * core::f32::consts::TAU
    } else if num(v) == Some(0.0) {
        0.0
    } else {
        return None;
    };
    r.is_finite().then_some(r)
}

/// A `<number> | <percentage>` scale factor (css-transforms-2 §5.1).
fn parse_factor(v: &str) -> Option<f32> {
    let v = v.trim();
    let f = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok()? / 100.0,
        None => v.parse::<f32>().ok()?,
    };
    f.is_finite().then_some(f)
}

/// Splits a function's arguments on top-level commas, or on whitespace when there are none.
fn func_args(args: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut comma) = (0i32, 0usize, false);
    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(args[start..i].trim());
                start = i + 1;
                comma = true;
            }
            _ => {}
        }
    }
    if comma {
        out.push(args[start..].trim());
        out
    } else {
        split_top_level(args)
    }
}

/// Splits a value into `name(args)` function calls; `None` when anything else is in it.
fn split_functions(value: &str) -> Option<Vec<(String, &str)>> {
    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let name = rest[..open].trim().to_ascii_lowercase();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let mut depth = 0i32;
        let mut close = None;
        for (i, c) in rest[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close?;
        out.push((name, &rest[open + 1..close]));
        rest = rest[close + 1..].trim_start();
    }
    Some(out)
}

/// The 2D part of a `rotate3d(x, y, z, a)` (css-transforms-2 §12): the upper-left 2x2 of the 3x3
/// rotation matrix — what the flattened (orthographic) box shows.
fn rotate3d(x: f32, y: f32, z: f32, a: f32) -> Option<[f32; 6]> {
    let len = crate::paint::sqrt(x * x + y * y + z * z);
    if len == 0.0 {
        return None;
    }
    let (x, y, z) = (x / len, y / len, z / len);
    let (s, c) = crate::paint::sin_cos(a);
    let t = 1.0 - c;
    let m11 = t * x * x + c;
    let m12 = t * x * y + s * z;
    let m21 = t * x * y - s * z;
    let m22 = t * y * y + c;
    Some([m11, m12, m21, m22, 0.0, 0.0])
}

/// `transform` (css-transforms-1 §7, -2 §12) → its function list; `none` = empty. 3D functions flatten.
pub fn parse_transform_list(value: &str, cx: &ParseCx) -> Option<Vec<TransformOp>> {
    use TransformOp::{Matrix, Translate};
    let v = value.trim();
    if v.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for (name, args) in split_functions(v)? {
        let a = func_args(args);
        let n = |i: usize| a.get(i).and_then(|s| s.trim().parse::<f32>().ok()).filter(|f| f.is_finite());
        let op = match (name.as_str(), a.len()) {
            ("matrix", 6) => Matrix([n(0)?, n(1)?, n(2)?, n(3)?, n(4)?, n(5)?]),
            ("matrix3d", 16) => Matrix([n(0)?, n(1)?, n(4)?, n(5)?, n(12)?, n(13)?]),
            ("translate", 1) => Translate(len_pct(a[0], cx)?, [0.0, 0.0]),
            ("translate", 2) => Translate(len_pct(a[0], cx)?, len_pct(a[1], cx)?),
            ("translate3d", 3) => {
                parse_px(a[2], cx)?;
                Translate(len_pct(a[0], cx)?, len_pct(a[1], cx)?)
            }
            ("translatex", 1) => Translate(len_pct(a[0], cx)?, [0.0, 0.0]),
            ("translatey", 1) => Translate([0.0, 0.0], len_pct(a[0], cx)?),
            ("translatez", 1) => {
                parse_px(a[0], cx)?;
                Matrix(IDENTITY)
            }
            ("scale", 1) => {
                let f = parse_factor(a[0])?;
                Matrix([f, 0.0, 0.0, f, 0.0, 0.0])
            }
            ("scale", 2) | ("scale3d", 3) => Matrix([parse_factor(a[0])?, 0.0, 0.0, parse_factor(a[1])?, 0.0, 0.0]),
            ("scalex", 1) => Matrix([parse_factor(a[0])?, 0.0, 0.0, 1.0, 0.0, 0.0]),
            ("scaley", 1) => Matrix([1.0, 0.0, 0.0, parse_factor(a[0])?, 0.0, 0.0]),
            ("scalez", 1) => {
                parse_factor(a[0])?;
                Matrix(IDENTITY)
            }
            ("rotate", 1) | ("rotatez", 1) => {
                let (s, c) = crate::paint::sin_cos(parse_angle(a[0])?);
                Matrix([c, s, -s, c, 0.0, 0.0])
            }
            ("rotatex", 1) => Matrix(rotate3d(1.0, 0.0, 0.0, parse_angle(a[0])?)?),
            ("rotatey", 1) => Matrix(rotate3d(0.0, 1.0, 0.0, parse_angle(a[0])?)?),
            ("rotate3d", 4) => Matrix(rotate3d(n(0)?, n(1)?, n(2)?, parse_angle(a[3])?).unwrap_or(IDENTITY)),
            ("skew", 1 | 2) => {
                let ax = parse_angle(a[0])?;
                let ay = match a.get(1) {
                    Some(s) => parse_angle(s)?,
                    None => 0.0,
                };
                Matrix([1.0, tan(ay)?, tan(ax)?, 1.0, 0.0, 0.0])
            }
            ("skewx", 1) => Matrix([1.0, 0.0, tan(parse_angle(a[0])?)?, 1.0, 0.0, 0.0]),
            ("skewy", 1) => Matrix([1.0, tan(parse_angle(a[0])?)?, 0.0, 1.0, 0.0, 0.0]),
            // css-transforms-2 §12: a flattened box shows no perspective foreshortening.
            ("perspective", 1) => Matrix(IDENTITY),
            _ => return None,
        };
        out.push(op);
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// tan, `None` at the poles (an infinite skew is invalid at paint, css-transforms-1 §7.1).
fn tan(a: f32) -> Option<f32> {
    let (s, c) = crate::paint::sin_cos(a);
    if c.abs() < 1e-6 {
        return None;
    }
    Some(s / c)
}

/// The individual `translate` / `rotate` / `scale` properties (css-transforms-2 §5.1); `none` = identity.
pub fn parse_individual_transform(prop: &str, value: &str, cx: &ParseCx) -> Option<TransformOp> {
    let v = value.trim().to_ascii_lowercase();
    if v == "none" {
        return Some(TransformOp::Matrix(IDENTITY));
    }
    let parts = split_top_level(&v);
    match prop {
        "translate" => match parts.as_slice() {
            [x] => Some(TransformOp::Translate(len_pct(x, cx)?, [0.0, 0.0])),
            [x, y] => Some(TransformOp::Translate(len_pct(x, cx)?, len_pct(y, cx)?)),
            [x, y, z] => {
                parse_px(z, cx)?;
                Some(TransformOp::Translate(len_pct(x, cx)?, len_pct(y, cx)?))
            }
            _ => None,
        },
        "rotate" => {
            // `<angle> | [x | y | z | <number>{3}] && <angle>`: the angle is the first or the last part
            let has_unit = |p: &str| p.parse::<f32>().is_err() && parse_angle(p).is_some();
            let (a, rest): (f32, &[&str]) = match parts.as_slice() {
                [] => return None,
                [.., last] if has_unit(*last) => (parse_angle(last)?, &parts[..parts.len() - 1]),
                [first, ..] if has_unit(*first) => (parse_angle(first)?, &parts[1..]),
                _ => return None,
            };
            let m = match rest {
                [] | ["z"] => rotate3d(0.0, 0.0, 1.0, a),
                ["x"] => rotate3d(1.0, 0.0, 0.0, a),
                ["y"] => rotate3d(0.0, 1.0, 0.0, a),
                [x, y, z] => rotate3d(x.parse().ok()?, y.parse().ok()?, z.parse().ok()?, a),
                _ => return None,
            };
            Some(TransformOp::Matrix(m.unwrap_or(IDENTITY)))
        }
        _ => match parts.as_slice() {
            [f] => {
                let f = parse_factor(f)?;
                Some(TransformOp::Matrix([f, 0.0, 0.0, f, 0.0, 0.0]))
            }
            [x, y] | [x, y, _] => Some(TransformOp::Matrix([parse_factor(x)?, 0.0, 0.0, parse_factor(y)?, 0.0, 0.0])),
            _ => None,
        },
    }
}

/// One `<position>` component keyword as a fraction, with its axis (0 x, 1 y, 2 either).
fn position_keyword(k: &str) -> Option<(u8, f32)> {
    Some(match k {
        "left" => (0, 0.0),
        "right" => (0, 1.0),
        "top" => (1, 0.0),
        "bottom" => (1, 1.0),
        "center" => (2, 0.5),
        _ => return None,
    })
}

/// A 1-2 value `<position>` (css-values-4 §9.1) — or, for `transform-origin` (css-transforms-1 §6),
/// 1-3 values with the z length ignored — as [x, y] length-percentages. Omitted axes are `center`.
pub fn parse_position_pair(value: &str, cx: &ParseCx, allow_z: bool) -> Option<[LenPct; 2]> {
    let v = value.trim().to_ascii_lowercase();
    let mut parts = split_top_level(&v);
    if allow_z && parts.len() == 3 {
        parse_px(parts[2], cx)?;
        parts.truncate(2);
    }
    let comp = |s: &str| -> Option<(u8, LenPct)> {
        match position_keyword(s) {
            Some((axis, f)) => Some((axis, [0.0, f])),
            None => len_pct(s, cx).map(|lp| (3, lp)),
        }
    };
    let center = [0.0, 0.5];
    match parts.as_slice() {
        [a] => {
            let (axis, lp) = comp(*a)?;
            Some(if axis == 1 { [center, lp] } else { [lp, center] })
        }
        [a, b] => {
            let (ax, la) = comp(*a)?;
            let (bx, lb) = comp(*b)?;
            // x then y; keywords alone may come y first (`top left`, `center left`)
            if ax != 1 && bx != 0 {
                Some([la, lb])
            } else if matches!(ax, 1 | 2) && matches!(bx, 0 | 2) {
                Some([lb, la])
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `clip-path` (css-masking-1 §5.1): `none` = Some(None); a basic shape (css-shapes-1 §3.1) with an
/// optional geometry-box keyword (read as the border box). `url()` and anything else are `None`.
pub fn parse_clip_path(value: &str, cx: &ParseCx) -> Option<Option<ClipShape>> {
    let v = value.trim().to_ascii_lowercase();
    if v == "none" {
        return Some(None);
    }
    if v.contains("url(") {
        return None;
    }
    // drop a trailing or leading geometry-box keyword
    let boxes = ["border-box", "padding-box", "content-box", "margin-box", "fill-box", "stroke-box", "view-box"];
    let mut shape_text = v.as_str();
    for b in boxes {
        if let Some(r) = shape_text.strip_suffix(b) {
            shape_text = r.trim();
        } else if let Some(r) = shape_text.strip_prefix(b) {
            shape_text = r.trim();
        }
    }
    let funcs = split_functions(shape_text)?;
    let [(name, args)] = funcs.as_slice() else { return None };
    let radius = |s: &str| -> Option<ShapeRadius> {
        match s {
            "closest-side" => Some(ShapeRadius::ClosestSide),
            "farthest-side" => Some(ShapeRadius::FarthestSide),
            _ => len_pct(s, cx).filter(|lp| lp[0] >= 0.0 && lp[1] >= 0.0).map(ShapeRadius::Len),
        }
    };
    let (shape_part, at) = match args.split_once(" at ") {
        Some((s, p)) => (s.trim(), Some(p.trim())),
        None if args.trim_start().starts_with("at ") => ("", Some(args.trim_start()[3..].trim())),
        None => (args.trim(), None),
    };
    let center = match at {
        Some(p) => parse_position_pair(p, cx, false)?,
        None => [[0.0, 0.5], [0.0, 0.5]],
    };
    let shape = match name.as_str() {
        "inset" => {
            let (ins, round) = match args.split_once(" round ") {
                Some((i, r)) => (i.trim(), Some(r.trim())),
                None => (args.trim(), None),
            };
            let r = parse_sides(ins, |p| len_pct(p, cx))?;
            let rad = match round {
                Some(r) => len_pct(split_top_level(split_top_level_slash(r).0).first()?, cx)?, // AETHERCSSSPLITAUDIT (SR426)
                None => [0.0, 0.0],
            };
            ClipShape::Inset([r.top, r.right, r.bottom, r.left], rad)
        }
        "circle" => {
            let r = match split_top_level(shape_part).as_slice() {
                [] => ShapeRadius::ClosestSide,
                [r] => radius(*r)?,
                _ => return None,
            };
            ClipShape::Circle(r, center)
        }
        "ellipse" => {
            let (rx, ry) = match split_top_level(shape_part).as_slice() {
                [] => (ShapeRadius::ClosestSide, ShapeRadius::ClosestSide),
                [a, b] => (radius(*a)?, radius(*b)?),
                _ => return None,
            };
            ClipShape::Ellipse(rx, ry, center)
        }
        "polygon" => {
            let mut items: Vec<&str> = args.split(',').map(str::trim).collect();
            let mut evenodd = false;
            if let Some(first) = items.first().copied() {
                if first == "evenodd" || first == "nonzero" {
                    evenodd = first == "evenodd";
                    items.remove(0);
                }
            }
            let pts: Option<Vec<[LenPct; 2]>> = items
                .iter()
                .map(|it| match split_top_level(it).as_slice() {
                    [x, y] => Some([len_pct(x, cx)?, len_pct(y, cx)?]),
                    _ => None,
                })
                .collect();
            let pts = pts?;
            if pts.len() < 3 {
                return None;
            }
            ClipShape::Polygon(pts, evenodd)
        }
        _ => return None,
    };
    Some(Some(shape))
}


/// Substitutes `var(--x)` / `var(--x, fallback)` from the page's custom
/// property map. Unknown vars use the fallback or resolve to "" (which the
/// property parser then ledgers). Depth-capped against cycles.
pub fn resolve_vars<H: Host>(value: &str, depth: u8) -> String {
    if depth > 4 || !value.contains("var(") {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find("var(") {
        out.push_str(&rest[..pos]);
        let inner_start = pos + 4;
        // Find the matching ')' (fallbacks may contain nested parens).
        let mut depth_p = 1i32;
        let mut end = None;
        for (o, c) in rest[inner_start..].char_indices() {
            match c {
                '(' => depth_p += 1,
                ')' => {
                    depth_p -= 1;
                    if depth_p == 0 {
                        end = Some(inner_start + o);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let inner = &rest[inner_start..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let resolved = H::custom_prop(name);
        match resolved.or_else(|| fallback.map(|f| f.to_string())) {
            Some(v) => out.push_str(&resolve_vars::<H>(&v, depth + 1)),
            None => H::record_css(&format!("var-unresolved:{}", clip(name))),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Extracts the url from a `url(...)` component (quotes stripped). Returns
/// None for gradients/none/values without a url() component.
pub fn extract_css_url(value: &str) -> Option<String> {
    let start = value.find("url(")?;
    let rest = &value[start + 4..];
    let end = rest.find(')')?;
    let inner = rest[..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_string())
}

/// Removes every `url(...)` token from a value, leaving the other
/// shorthand components (a url can contain slashes, spaces and keywords
/// that would otherwise be mistaken for them).
pub fn strip_css_urls(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("url(") {
        out.push_str(&rest[..start]);
        out.push(' ');
        match rest[start + 4..].find(')') {
            Some(end) => rest = &rest[start + 4 + end + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// background-repeat / mask-repeat keyword: 0 repeat, 1 no-repeat,
/// 2 repeat-x, 3 repeat-y. `space`/`round` approximate to repeat.
pub fn parse_repeat(value: &str) -> Option<u8> {
    match value.trim() {
        "no-repeat" => Some(1),
        "repeat-x" => Some(2),
        "repeat-y" => Some(3),
        "repeat" | "space" | "round" | "repeat repeat" => Some(0),
        _ => None,
    }
}

/// True for a token that can only be a background-position component
/// (keyword, percentage or length) — used to pick the position out of the
/// `background` shorthand without mistaking a colour or repeat keyword.
fn is_position_token(t: &str) -> bool {
    let t = t.trim();
    if matches!(t, "left" | "right" | "top" | "bottom" | "center") {
        return true;
    }
    // A bare `0` is a valid position component and the commonest one:
    // `background:url(sprite.png) 0 -261px repeat-x` dropped its x offset,
    // leaving a one-value position that shifted the sprite in the wrong axis.
    if t == "0" {
        return true;
    }
    // AETHERCSSSPLITAUDIT (SR426): a math function is a <length-percentage> (css-values-4 §10).
    if let Some(open) = t.find('(') {
        return t.ends_with(')') && matches!(t[..open].to_ascii_lowercase().as_str(), "calc" | "min" | "max" | "clamp");
    }
    for unit in ["px", "em", "rem", "pt", "%"] {
        if let Some(n) = t.strip_suffix(unit) {
            return !n.is_empty() && n.trim().parse::<f32>().is_ok();
        }
    }
    false
}

/// Truncates a value for a stable, bounded ledger key.
pub fn clip(v: &str) -> &str {
    clip_n(v, 24)
}

/// Truncates to at most `n` bytes, on a char boundary.
fn clip_n(v: &str, n: usize) -> &str {
    if v.len() <= n {
        return v;
    }
    let mut end = n;
    while end > 0 && !v.is_char_boundary(end) {
        end -= 1;
    }
    &v[..end]
}


/// Overlays `src`'s specified paint fields onto `dst`. ONE list, used by
/// every cascade path — a field missing here is a declaration that parses
/// and then never reaches the renderer.
pub fn merge_paint<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    macro_rules! copy {
        ($($f:ident),* $(,)?) => { $( if src.$f.is_some() { dst.$f = src.$f; } )* };
    }
    macro_rules! clone {
        ($($f:ident),* $(,)?) => { $( if src.$f.is_some() { dst.$f = src.$f.clone(); } )* };
    }
    merge_border_pending(dst, src); merge_border_alpha(dst, src); copy!(
        background, color, font_size, weight, border, line_height, hidden, clip, underline,
        nowrap, family, italic, text_transform, border_width, bg_repeat, text_hidden,
        mask_repeat, text_align, text_align_last, object_fit, flex_container, border_style, radius,
        white_space, word_break, overflow_wrap, letter_spacing, line_through, display_kind, decoration_color, decoration_style, decoration_thickness, underline_offset, underline_position,
        ua_vmargin, border_collapse, caption_side, border_spacing, has_width, position_kind, z_index,
        list_item, list_style, vertical_align, float, opacity, font_size_rel, table_fixed, bg_alpha,
        stretch, word_spacing, rtl, unicode_bidi, clear, tf_translate, tf_rotate, tf_scale, transform_origin, inset_px,
        overflow, col_count, col_width, col_gap, col_fill, col_rule_width, col_rule_style, col_rule_color,
        writing_mode, text_orientation, container_type, contain_size,
        scrollbar_gutter, scroll_smooth, snap_type, snap_align, snap_stop, scroll_padding, col_span,
        deco_clone, text_combine, scroll_margin, container_scroll,
        outline_style, outline_width, outline_color, caret_color, outline_offset, synth_weight, synth_style, synth_small_caps, small_caps, color_scheme,
    );
    clone!(text_shadows, transform, clip_path, filter, backdrop_filter, container_name); dst.merge_inherit(src); dst.merge_currentcolor(src); dst.merge_system_colors(src); merge_white_space_halves(dst, src); border_width_apply(dst); // SR1069
    if let Some(src_list) = &src.font_rel {
        let list = dst.font_rel.get_or_insert_with(Vec::new);
        for (p, v) in src_list {
            list.retain(|(q, _)| q != p);
            list.push((p.clone(), v.clone()));
        }
    }
    merge_bg_image_none(dst, src); merge_bg_layers(dst, src); clone!(pct_math, bg_gradient, shadows);
    clone!(bg_image, bg_more, bg_size, bg_position, bg_repeat_more, bg_position_more, bg_size_more, mask_image, mask_size, mask_position, object_position);
}


/// css-ui-4 §5.1's cursor keywords (plus the legacy `hand` and the `-webkit-` grab spellings Chromium
/// still accepts), interned.
pub fn cursor_keyword(kw: &str) -> Option<&'static str> {
    const KW: [&str; 36] = [
        "auto", "default", "none", "context-menu", "help", "pointer", "progress", "wait", "cell", "crosshair", "text",
        "vertical-text", "alias", "copy", "move", "no-drop", "not-allowed", "grab", "grabbing", "e-resize",
        "n-resize", "ne-resize", "nw-resize", "s-resize", "se-resize", "sw-resize", "w-resize", "ew-resize",
        "ns-resize", "nesw-resize", "nwse-resize", "col-resize", "row-resize", "all-scroll", "zoom-in", "zoom-out",
    ];
    let kw = match kw {
        "hand" => "pointer",
        "initial" => "auto",
        "-webkit-grab" => "grab",
        "-webkit-grabbing" => "grabbing",
        "-webkit-zoom-in" => "zoom-in",
        "-webkit-zoom-out" => "zoom-out",
        k => k,
    };
    KW.iter().copied().find(|k| *k == kw)
}


/// Parses a declaration block into (normal, !important) tiers plus a flag
/// saying whether the important tier holds anything. Declarations split by
/// their own priority, per CSS — not per rule.
/// Splits a declaration block on the semicolons that actually separate
/// declarations — a `;` inside url(), a function, or a string does not
/// (`background:url(data:image/svg+xml;base64,...)` is one declaration,
/// and splitting it naively truncates the value at the media type).
pub fn split_declarations(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut depth) = (0usize, 0i32);
    let mut quote: Option<char> = None;
    for (i, c) in body.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = (depth - 1).max(0),
                ';' if depth == 0 => {
                    out.push(&body[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    out.push(&body[start..]);
    out
}

/// Splits `prop: value` at the separating colon — the first colon outside
/// any function/string, so a data: URI value survives intact.
pub fn split_declaration(decl: &str) -> Option<(&str, &str)> {
    let (mut depth, mut quote) = (0i32, None::<char>);
    for (i, c) in decl.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = (depth - 1).max(0),
                ':' if depth == 0 => return Some((&decl[..i], &decl[i + 1..])),
                _ => {}
            },
        }
    }
    None
}

pub fn parse_declaration_block_tiers<H: Host>(body: &str, cx: &ParseCx) -> (SpecifiedStyle<H>, SpecifiedStyle<H>, bool) {
    let mut normal = SpecifiedStyle::default();
    let mut important = SpecifiedStyle::default();
    let mut has_important = false;
    for decl in split_declarations(body) {
        let Some((prop, value)) = split_declaration(decl) else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        if prop.is_empty() || prop.starts_with("--") {
            continue; // custom properties: no cascade var() support yet
        }
        let value = value.trim();
        match value
            .strip_suffix("!important")
            .or_else(|| value.strip_suffix("! important"))
        {
            Some(v) => {
                has_important = true;
                apply_declaration(&prop, &resolve_vars::<H>(v.trim(), 0), &mut important, cx);
            }
            None => apply_declaration(&prop, &resolve_vars::<H>(value, 0), &mut normal, cx),
        }
    }
    (normal, important, has_important)
}

/// Parses a `prop: value; prop: value` declaration block (no braces) into a
/// single set — `!important` declarations simply win within the block. Used
/// where tiers don't matter (build-time inline defaults).
pub fn parse_declaration_block<H: Host>(body: &str, cx: &ParseCx) -> SpecifiedStyle<H> {
    let (mut normal, important, has_important) = parse_declaration_block_tiers(body, cx);
    if has_important {
        merge_specified(&mut normal, &important);
    }
    normal
}

/// Overlays `src`'s specified fields onto `dst`.
pub fn merge_specified<H: Host>(dst: &mut SpecifiedStyle<H>, src: &SpecifiedStyle<H>) {
    macro_rules! take {
        ($($f:ident),*) => { $( if src.$f.is_some() { dst.$f = src.$f; } )* };
    }
    erase_reverted(dst, src.revert_spec); dst.revert_spec |= src.revert_spec; if src.align_self_auto { dst.align_self = None; dst.align_self_auto = true; } if src.justify_self_auto { dst.justify_self = None; dst.justify_self_auto = true; } for i in 0..4 { // AETHERCSSWIDEREVERTNONINHERITED (SR1037): a later revert erases the earlier rule's field before the Some-copy
        if src.padding[i].is_some() { dst.padding[i] = src.padding[i]; }
        if src.margin[i].is_some() { dst.margin[i] = src.margin[i]; }
    }
    take!(display, flex_direction, width, height, position,
          inset_top, inset_left, inset_right, inset_bottom, justify, align_items, align_content, align_self, justify_items, justify_self,
          max_width, max_height, min_width, min_height, box_sizing,
          flex_container, flex_wrap, row_gap, column_gap, flex_grow, flex_shrink, flex_basis, aspect_ratio);
    merge_paint(&mut dst.paint, &src.paint); dst.merge_inherit_box(src);
    dst.transition.merge(&src.transition);
    dst.animation.merge(&src.animation);
    for (p, v) in &src.animatable {
        dst.animatable.retain(|(q, _)| q != p);
        dst.animatable.push((p.clone(), v.clone()));
    }
    let (d, g) = (&mut dst.generated, &src.generated);
    if g.content.is_some() { d.content = g.content.clone(); }
    if g.counter_reset.is_some() { d.counter_reset = g.counter_reset.clone(); }
    if g.counter_increment.is_some() { d.counter_increment = g.counter_increment.clone(); }
    if g.counter_set.is_some() { d.counter_set = g.counter_set.clone(); }
    if g.quotes.is_some() { d.quotes = g.quotes.clone(); }
}


/// Parses a CSS color from a string: named, #rgb/#rrggbb, rgb()/rgba().
/// rgba() alpha is ignored (no compositing yet) unless fully transparent.
pub fn parse_color_str<H: Host>(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return hex_color(hex);
    }
    let lower = v.to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split([',', ' ', '/']).filter(|s| !s.trim().is_empty()).collect();
        if parts.len() >= 3 {
            let ch = |s: &str| -> Option<u8> {
                let s = s.trim();
                if let Some(p) = s.strip_suffix('%') {
                    p.trim().parse::<f32>().ok().map(|f| (f / 100.0 * 255.0) as u8)
                } else {
                    s.parse::<f32>().ok().map(|f| f.clamp(0.0, 255.0) as u8)
                }
            };
            if let (Some(r), Some(g), Some(b)) = (ch(parts[0]), ch(parts[1]), ch(parts[2])) {
                // Fully transparent = paint nothing.
                if let Some(a) = parts.get(3).and_then(|s| s.trim().parse::<f32>().ok()) {
                    if a == 0.0 {
                        return None;
                    }
                }
                return Some((r, g, b));
            }
        }
        return None;
    }
    // Function notations the arm above does not read (hsl(), hwb(), the modern syntaxes): css_core's
    // parser; fully transparent paints nothing, as above.
    // Only a COMPLETE function (it ends in `)`): a whitespace-split fragment of a shorthand (`rgb(238,`) is
    // no colour, and the host's parser must never be asked to re-enter this one with it.
    if lower.contains('(') {
        if !lower.ends_with(')') {
            return None;
        }
        return match H::parse_color_alpha(v) {
            Some((c, a)) if a > 0.0 => Some(c),
            _ => None,
        };
    }
    named_color::<H>(&lower)
}

/// Keywords that specify "no concrete value here" — not coverage gaps.
pub fn is_neutral_keyword(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "transparent" | "inherit" | "initial" | "unset" | "currentcolor" | "none"
    )
}

const BASE_FONT_PX: f32 = 16.0;

/// The alpha of a colour value (1 for opaque or unparsable colours).
fn color_alpha<H: Host>(v: &str) -> f32 {
    H::parse_color_alpha(v).map(|c| c.1).unwrap_or(1.0)
}

/// Parses `decls` (property, value) in order into one specified style, at
/// the current font basis.
pub fn reapply_declarations<H: Host>(decls: &[(String, String)], cx: &ParseCx) -> SpecifiedStyle<H> {
    let mut spec = SpecifiedStyle::default();
    for (p, v) in decls {
        apply_declaration(p, v, &mut spec, cx);
    }
    spec
}

/// px per one `unit` for the font-relative units, from the current basis.
fn font_unit_px(unit: &str, cx: &ParseCx) -> Option<f32> {
    let (em, rem, ex, ch) = cx.font;
    match unit {
        "em" => Some(em),
        "rem" => Some(rem),
        "ex" => Some(ex),
        "ch" => Some(ch),
        _ => None,
    }
}

/// A number with a font-relative unit, as a value in px at the current
/// basis (`1.5em`, `2rem`, `3ex`, `10ch`).
fn parse_font_relative(v: &str, cx: &ParseCx) -> Option<f32> {
    for unit in ["rem", "em", "ex", "ch"] {
        if let Some(n) = v.strip_suffix(unit) {
            let n = n.trim();
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e')) {
                if let (Ok(n), Some(px)) = (n.parse::<f32>(), font_unit_px(unit, cx)) {
                    return Some(n * px);
                }
            }
        }
    }
    None
}

/// True when a value uses a font-relative unit anywhere (`1em`, `calc(2rem
/// + 4px)`, `0 1.5ch`): its px depend on the element's font.
pub fn is_font_relative(value: &str) -> bool {
    let b = value.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() || (b[i] == b'.' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit())) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let start = i;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            if matches!(&value[start..i], "em" | "rem" | "ex" | "ch") {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// AETHERSTYLE3B (SR115): a value holding a container-relative length (cqw, cqh, cqi, cqb, cqmin, cqmax).
pub fn is_container_relative(value: &str) -> bool {
    let b = value.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() || (b[i] == b'.' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit())) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let start = i;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            if matches!(value[start..i].to_ascii_lowercase().as_str(), "cqw" | "cqh" | "cqi" | "cqb" | "cqmin" | "cqmax") {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Properties whose value can be a length: their declarations are kept in
/// cascade order (`PaintStyle::font_rel`) so the font-relative ones can be
/// re-resolved against the element's own font.
pub fn is_length_property(prop: &str) -> bool {
    matches!(
        prop,
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height" | "margin" | "margin-top"
            | "margin-right" | "margin-bottom" | "margin-left" | "padding" | "padding-top" | "padding-right"
            | "padding-bottom" | "padding-left" | "top" | "right" | "bottom" | "left" | "inset" | "gap" | "row-gap"
            | "column-gap" | "flex-basis" | "flex" | "border" | "border-top" | "border-right" | "border-bottom"
            | "border-left" | "border-width" | "border-top-width" | "border-right-width" | "border-bottom-width"
            | "border-left-width" | "border-radius" | "letter-spacing" | "line-height" | "vertical-align"
            | "border-spacing" | "outline" | "outline-width" | "outline-offset" | "text-indent"
    )
}

/// Parses a font-size string: px, em/rem (relative to the 16px UA base —
/// approximation, not parent-relative), %, or absolute keywords.
pub fn parse_font_size(value: &str, cx: &ParseCx) -> Option<f32> {
    let v = value.trim().to_ascii_lowercase();
    // css-values-4 math (`clamp(1.25rem, 4vw, 2.5rem)`); % is of the
    // (approximated 16px) parent size.
    if v.contains('(') {
        return eval_length(&v, Some(BASE_FONT_PX), cx);
    }
    if let Some(px) = v.strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(px);
    }
    if let Some(em) = v
        .strip_suffix("rem")
        .or_else(|| v.strip_suffix("em"))
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        return Some(em * BASE_FONT_PX);
    }
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(pct / 100.0 * BASE_FONT_PX);
    }
    // Absolute print units. Legacy pages still size their small print in
    // pt (`font-size:10pt` on a footer), and dropping them left that text
    // at the inherited 16px.
    for (unit, per_px) in [("pt", 96.0 / 72.0), ("pc", 16.0), ("in", 96.0), ("cm", 96.0 / 2.54), ("mm", 96.0 / 25.4)] {
        if let Some(n) = v.strip_suffix(unit).and_then(|n| n.trim().parse::<f32>().ok()) {
            return Some(n * per_px);
        }
    }
    match v.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        _ => v.parse::<f32>().ok(),
    }
}

/// Parses a font-weight into "bold or not".
pub fn parse_font_weight(value: &str) -> Option<crate::FontWeight> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "bold" => Some(FontWeight::Abs(700)),
        "normal" => Some(FontWeight::Abs(400)),
        "bolder" => Some(FontWeight::Bolder),
        "lighter" => Some(FontWeight::Lighter),
        _ => v.parse::<f32>().ok().filter(|n| (1.0..=1000.0).contains(n)).map(|n| FontWeight::Abs(n.round() as u16)),
    }
}

/// css-fonts-4 §2.3 `font-stretch`: a keyword or a percentage, in percent.
pub fn parse_font_stretch(value: &str) -> Option<u16> {
    let v = value.trim().to_ascii_lowercase();
    Some(match v.as_str() {
        "normal" => 100,
        "ultra-condensed" => 50,
        "extra-condensed" => 63,
        "condensed" => 75,
        "semi-condensed" => 88,
        "semi-expanded" => 113,
        "expanded" => 125,
        "extra-expanded" => 150,
        "ultra-expanded" => 200,
        p => p.strip_suffix('%')?.trim().parse::<f32>().ok().filter(|x| *x >= 0.0)?.round() as u16,
    })
}

/// Parses a length into pixels: px, rem/em (16px base — em is not
/// parent-relative, same approximation as font-size), a math function
/// (`calc()`/`min()`/`max()`/`clamp()`), or a bare number.
pub fn parse_px(value: &str, cx: &ParseCx) -> Option<f32> {
    let v = value.trim();
    if v.contains('(') {
        return eval_length(v, None, cx);
    }
    if let Some(px) = parse_viewport_length(v, cx) {
        return Some(px);
    }
    if let Some(px) = parse_font_relative(v, cx) {
        return Some(px);
    }
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}


/// those columns to their content and stacked the page wrong.
pub fn parse_viewport_length(v: &str, cx: &ParseCx) -> Option<f32> {
    let v = v.trim();
    let (vw, vh) = cx.viewport;
    // AETHERSTYLE3B (SR115): the container-relative units (css-contain-3 §6.1), the small viewport's axis
    // when no container answers (horizontal writing: cqi = cqw, cqb = cqh).
    let (qw, qh) = (cx.container.0.unwrap_or(vw), cx.container.1.unwrap_or(vh));
    // Longest suffixes first: `dvh` must not be read as `vh` with a stray d.
    for (unit, basis) in [
        ("cqmin", qw.min(qh)), ("cqmax", qw.max(qh)),
        ("cqw", qw), ("cqi", qw), ("cqh", qh), ("cqb", qh),
        ("dvmin", vw.min(vh)), ("svmin", vw.min(vh)), ("lvmin", vw.min(vh)),
        ("dvmax", vw.max(vh)), ("svmax", vw.max(vh)), ("lvmax", vw.max(vh)),
        ("vmin", vw.min(vh)), ("vmax", vw.max(vh)),
        ("dvh", vh), ("svh", vh), ("lvh", vh),
        ("dvw", vw), ("svw", vw), ("lvw", vw),
        ("vh", vh), ("vw", vw),
    ] {
        if let Some(n) = v.strip_suffix(unit) {
            let n = n.trim();
            // Guard against `rem`/`em` etc. ending in a matched substring.
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+') {
                return n.parse::<f32>().ok().map(|n| n / 100.0 * basis);
            }
        }
    }
    None
}

/// Evaluates a CSS math expression into pixels: `calc()`, `min()`, `max()`,
/// `clamp()`, nested parentheses and `+ - * /` over absolute lengths
/// (px, pt, rem/em at the 16px base) and percentages. `reference` is the
/// percentage basis; without one a percentage term makes the whole
/// expression unresolvable so the caller can fall back honestly.
///
/// Modern component CSS states nearly every box metric as
/// `calc(var(--x) + 4px)` / `max(..., 10px)`; without this the declaration
/// is dropped and the element collapses to its min-* floor.
pub fn eval_length(value: &str, reference: Option<f32>, cx: &ParseCx) -> Option<f32> {
    let mut p = MathParser { s: value.trim().as_bytes(), i: 0, reference, cx };
    let v = p.expr()?;
    p.ws();
    if p.i != p.s.len() {
        return None;
    }
    v.is_finite().then_some(v)
}

struct MathParser<'a> {
    cx: &'a ParseCx,
    s: &'a [u8],
    i: usize,
    reference: Option<f32>,
}

impl MathParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.i < self.s.len() && self.s[self.i] == c {
            self.i += 1;
            return true;
        }
        false
    }
    /// sum := product (('+' | '-') product)*
    fn expr(&mut self) -> Option<f32> {
        let mut acc = self.product()?;
        loop {
            self.ws();
            match self.s.get(self.i) {
                Some(b'+') => {
                    self.i += 1;
                    acc += self.product()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    acc -= self.product()?;
                }
                _ => return Some(acc),
            }
        }
    }
    /// product := unary (('*' | '/') unary)*
    fn product(&mut self) -> Option<f32> {
        let mut acc = self.unary()?;
        loop {
            self.ws();
            match self.s.get(self.i) {
                Some(b'*') => {
                    self.i += 1;
                    acc *= self.unary()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None;
                    }
                    acc /= d;
                }
                _ => return Some(acc),
            }
        }
    }
    fn unary(&mut self) -> Option<f32> {
        self.ws();
        if self.eat(b'-') {
            return self.unary().map(|v| -v);
        }
        if self.eat(b'+') {
            return self.unary();
        }
        self.atom()
    }
    /// atom := '(' sum ')' | function '(' args ')' | number [unit]
    fn atom(&mut self) -> Option<f32> {
        self.ws();
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'-')
        {
            self.i += 1;
        }
        if self.i > start {
            let name = core::str::from_utf8(&self.s[start..self.i]).ok()?.to_ascii_lowercase();
            if !self.eat(b'(') {
                return None;
            }
            let mut args = vec![self.expr()?];
            while self.eat(b',') {
                args.push(self.expr()?);
            }
            if !self.eat(b')') {
                return None;
            }
            return match (name.as_str(), args.len()) {
                ("calc", 1) => Some(args[0]),
                ("min", _) => args.iter().copied().reduce(f32::min),
                ("max", _) => args.iter().copied().reduce(f32::max),
                ("clamp", 3) => Some(args[1].clamp(args[0], args[2])),
                _ => None,
            };
        }
        // number [unit]
        let nstart = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
        {
            self.i += 1;
        }
        if self.i == nstart {
            return None;
        }
        let n: f32 = core::str::from_utf8(&self.s[nstart..self.i]).ok()?.parse().ok()?;
        let ustart = self.i;
        if self.s.get(self.i) == Some(&b'%') {
            self.i += 1;
            return self.reference.map(|r| n / 100.0 * r);
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        let unit = core::str::from_utf8(&self.s[ustart..self.i]).ok()?.to_ascii_lowercase();
        match unit.as_str() {
            "" | "px" => Some(n),
            "pt" => Some(n * 4.0 / 3.0),
            "rem" | "em" | "ex" | "ch" => font_unit_px(&unit, self.cx).map(|u| n * u),
            // `calc(100vh - 64px)` is the standard full-height idiom.
            u => parse_viewport_length(&format!("{}{}", n, u), self.cx),
        }
    }
}

/// `display` value -> Aether's box approximation (flex-backed).
fn apply_display<H: Host>(value: &str, style: &mut SpecifiedStyle<H>) {
    match value {
        "none" => style.display = Some(Display::None),
        // Real flex containers: CSS flex semantics (see fold_into).
        "flex" | "inline-flex" | "-webkit-box" | "-webkit-inline-box" | "-webkit-flex"
        | "-webkit-inline-flex" | "-ms-flexbox" | "-ms-inline-flexbox" | "-moz-box" => {
            style.display = Some(Display::Flex);
            style.flex_container = Some(true);
            style.paint.flex_container = Some(true);
        }
        // Column-flex approximations of block-ish display types.
        "block" | "inline-block" | "inline" | "list-item"
        | "flow-root" | "table" | "table-cell" | "table-caption" | "table-row-group"
        | "table-header-group" | "table-footer-group" => {
            style.display = Some(Display::Flex);
            style.flex_container = Some(false);
            style.paint.flex_container = Some(false);
        }
        "table-row" => {
            style.display = Some(Display::Flex);
            style.flex_container = Some(false);
            style.flex_direction = Some(FlexDirection::Row);
        }
        "inherit" | "initial" | "unset" | "revert" => {}
        other => H::record_css(&format!("display:{}", other)),
    }
}

/// border-style keyword -> paint code (0 solid, 1 dashed, 2 dotted,
/// 3 double); the 3D styles paint solid in their colour.
pub fn border_style_code(v: &str) -> Option<u8> {
    match v.trim() {
        "solid" | "groove" | "ridge" | "inset" | "outset" => Some(0),
        "dashed" => Some(1),
        "dotted" => Some(2),
        "double" => Some(3),
        _ => None,
    }
}

/// A11YFACE (SR124): an `outline-width` (`<line-width>`): `thin` / `medium` / `thick`, or a non-negative length
/// (a bare number only when it is 0, as for borders).
fn outline_width_value(v: &str, cx: &ParseCx) -> Option<f32> {
    let v = v.trim();
    if let Some(px) = crate::paint::outline_width_keyword(v) {
        return Some(px);
    }
    if !v.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let bare = v.chars().all(|c| c.is_ascii_digit() || c == '.');
    if bare && v.parse::<f32>().ok() != Some(0.0) {
        return None;
    }
    parse_px(v, cx).filter(|px| *px >= 0.0)
}

/// A11YFACE (SR124): an `outline-color` as the RGBA [`PaintStyle::outline_color`] holds: `currentColor` and
/// `invert` (a UA that cannot invert paints currentColor, css-ui-4 §4.4) are `Some(None)`; any other colour's RGB through [`border_rgb`] (SR1119: alpha 0 included, `transparent` (0, 0, 0, 0)) and its alpha through [`color_alpha`].
fn outline_color_value<H: Host>(v: &str) -> Option<Option<(u8, u8, u8, u8)>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("currentcolor") || v.eq_ignore_ascii_case("invert") {
        return Some(None);
    }
    let (r, g, b) = border_rgb::<H>(v)?;
    let a = (color_alpha::<H>(v).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Some(Some((r, g, b, a)))
}

/// One radius: px (>= 0) or a percentage as a negative width fraction.
fn parse_radius(v: &str, cx: &ParseCx) -> Option<f32> {
    if let Some(p) = v.trim().strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(-(p / 100.0).max(0.0));
    }
    parse_px(v, cx).map(|x| x.max(0.0))
}

/// Splits a value on whitespace outside parentheses: `calc(1rem + 2px) 4px`
/// is two components, not four.
pub fn split_top_level(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, None::<usize>);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(st) = start.take() {
                out.push(&value[st..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        out.push(&value[st..]);
    }
    out
}

/// AETHERRADIUSCALC (SR416): splits a value at its first `/` outside parentheses (css-backgrounds-3 §5.1
/// `border-radius: <radii> / <radii>`): `calc(20px / 2) 4px / 2px` is (`calc(20px / 2) 4px `, Some(` 2px`)), never
/// a cut inside the function. The paren-depth walk is `split_top_level`'s; None when the value has no top-level `/`.
pub fn split_top_level_slash(value: &str) -> (&str, Option<&str>) {
    let mut depth = 0i32;
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '/' if depth == 0 => return (&value[..i], Some(&value[i + 1..])),
            _ => {}
        }
    }
    (value, None)
}

/// [top, right, bottom, left] index of a `*-top`/`*-right`/... longhand.
fn side_index(prop: &str) -> usize {
    if prop.ends_with("-top") { 0 } else if prop.ends_with("-right") { 1 } else if prop.ends_with("-bottom") { 2 } else { 3 }
}

/// Parses a 1-4 value box shorthand ("10px", "0 auto", "1px 2px 3px 4px")
/// into a sides rect using CSS's top/right/bottom/left expansion.
fn parse_sides<T: Copy>(value: &str, parse_one: impl Fn(&str) -> Option<T>) -> Option<Rect<T>> {
    let parts: Vec<T> = split_top_level(value).into_iter().map(|p| parse_one(p)).collect::<Option<_>>()?;
    let (t, r, b, l) = match parts.as_slice() {
        [a] => (*a, *a, *a, *a),
        [v, h] => (*v, *h, *v, *h),
        [t, h, b] => (*t, *h, *b, *h),
        [t, r, b, l] => (*t, *r, *b, *l),
        _ => return None,
    };
    Some(Rect { top: t, right: r, bottom: b, left: l })
}

/// `flex-basis`: `auto`, `content` (treated as auto), or a length/percentage.
pub fn parse_flex_basis(value: &str, cx: &ParseCx) -> Option<Dimension> {
    match value.trim() {
        "auto" | "content" => Some(Dimension::auto()),
        v => parse_dimension_str(v, cx),
    }
}

/// The `flex` shorthand → (grow, shrink, basis), per CSS Flexbox §7.2:
/// `none` = 0 0 auto, `auto` = 1 1 auto, `initial` = 0 1 auto; a unitless
/// first number is grow, an optional second number is shrink, and a lone
/// or trailing length is the basis (an omitted basis is 0%).
pub fn parse_flex_shorthand(value: &str, cx: &ParseCx) -> Option<(f32, f32, Dimension)> {
    match value.trim() {
        "none" => return Some((0.0, 0.0, Dimension::auto())),
        "auto" => return Some((1.0, 1.0, Dimension::auto())),
        "initial" => return Some((0.0, 1.0, Dimension::auto())),
        _ => {}
    }
    let mut nums: Vec<f32> = Vec::new();
    let mut basis: Option<Dimension> = None;
    for part in split_top_level(value) {
        if basis.is_none() && nums.len() < 2 {
            if let Ok(n) = part.parse::<f32>() {
                nums.push(n);
                continue;
            }
        }
        if basis.is_some() {
            return None;
        }
        basis = Some(parse_flex_basis(part, cx)?);
    }
    match (nums.as_slice(), basis) {
        ([], Some(b)) => Some((1.0, 1.0, b)),
        ([g], b) => Some((*g, 1.0, b.unwrap_or(Dimension::percent(0.0)))),
        ([g, s], b) => Some((*g, *s, b.unwrap_or(Dimension::percent(0.0)))),
        _ => None,
    }
}

pub fn parse_dimension_str(value: &str, cx: &ParseCx) -> Option<Dimension> {
    let v = value.trim();
    if v == "auto" {
        return Some(Dimension::auto());
    }
    // AETHERFIT (SR189): css-sizing-3 §2.1 `fit-content` — taffy's own sizing keyword (shrink-to-fit:
    // min(max-content, max(min-content, the stretch-fit size)), and abspos auto margins centre against it).
    if v == "fit-content" {
        return Some(Dimension::fit_content());
    }
    // AETHERFIT2 (SR191): css-sizing-3 §2.1 / §3.2 the other sizing keywords, onto taffy's own — `min-content`,
    // `max-content` and `fit-content(<length-percentage>)` (min(max-content, max(min-content, the argument))).
    // On `min-*` / `max-*` they fold to auto through `lpa` (taffy's min/max sizes cannot hold a keyword).
    match v {
        "min-content" => return Some(Dimension::min_content()),
        "max-content" => return Some(Dimension::max_content()),
        _ => {}
    }
    if let Some(arg) = v.strip_prefix("fit-content(").and_then(|r| r.strip_suffix(')')) {
        let raw = parse_length_percentage_str(arg, cx)?.into_raw();
        return match raw.tag() {
            taffy::style::CompactLength::LENGTH_TAG => Some(Dimension::fit_content_px(raw.value())),
            taffy::style::CompactLength::PERCENT_TAG => Some(Dimension::fit_content_percent(raw.value())),
            _ => None,
        };
    }
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(Dimension::percent(pct / 100.0));
    }
    parse_px(v, cx).map(Dimension::length)
}

pub fn parse_length_percentage_str(value: &str, cx: &ParseCx) -> Option<LengthPercentage> {
    let v = value.trim();
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(LengthPercentage::percent(pct / 100.0));
    }
    parse_px(v, cx).map(LengthPercentage::length)
}

pub fn parse_length_percentage_auto_str(value: &str, cx: &ParseCx) -> Option<LengthPercentageAuto> {
    let v = value.trim();
    if v == "auto" {
        return Some(LengthPercentageAuto::auto());
    }
    parse_length_percentage_str(v, cx).map(Into::into)
}

fn named_color<H: Host>(name: &str) -> Option<(u8, u8, u8)> {
    match name {
        "black" => Some((0, 0, 0)),
        "white" => Some((255, 255, 255)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 128, 0)),
        "blue" => Some((0, 0, 255)),
        "yellow" => Some((255, 255, 0)),
        "orange" => Some((255, 165, 0)),
        "purple" => Some((128, 0, 128)),
        "gray" | "grey" => Some((128, 128, 128)),
        "silver" => Some((192, 192, 192)),
        "navy" => Some((0, 0, 128)),
        "teal" => Some((0, 128, 128)),
        "maroon" => Some((128, 0, 0)),
        "olive" => Some((128, 128, 0)),
        "aqua" | "cyan" => Some((0, 255, 255)),
        "fuchsia" | "magenta" => Some((255, 0, 255)),
        "lime" => Some((0, 255, 0)),
        // Keywords that ARE valid colour syntax but carry no paintable RGB
        // here. They are answered, not missing — ledgering them buried the
        // real unknown-colour misses under `transparent` noise.
        "transparent" | "currentcolor" | "inherit" | "initial" | "unset" | "revert" | "none" => None,
        // css-color-4 §6.1: the full named-colour table is css_core's.
        other => match css_core::color::named(other) {
            Some(c) if c.a > 0.0 => {
                let ch = |x: f64| -> u8 { if x <= 0.0 { 0 } else if x >= 255.0 { 255 } else { (x + 0.5) as u8 } };
                Some((ch(c.r), ch(c.g), ch(c.b)))
            }
            Some(_) => None,
            None => {
                H::record_css(&format!("named-color:{}", clip(other)));
                None
            }
        },
    }
}

fn hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    match hex.len() {
        3 => {
            let mut it = hex.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8));
            if let (Some(Some(r)), Some(Some(g)), Some(Some(b))) = (it.next(), it.next(), it.next()) {
                return Some((r, g, b));
            }
            None
        }
        6 | 8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

// ---- AETHERFONTSYNTHESISPROPERTY (LEDGER SR750): css-fonts-4 §6 font-synthesis and its weight / style longhands (named tail block) ----

/// AETHERFONTSYNTHESISPROPERTY (SR750): css-fonts-4 §6 `font-synthesis: none | [weight || style || small-caps]` (the
/// shorthand: a keyword it omits sets that longhand to `none`; `small-caps` sets `font-synthesis-small-caps`, SR772, which
/// nothing paints yet: Aether has no small-caps synthesis) and the `font-synthesis-weight` / `font-synthesis-style` longhands (`auto | none`), into
/// `PaintStyle::synth_weight` / `synth_style` (Some(true) = auto). `initial` is `weight style` / `auto`; an invalid
/// value, `inherit` and `unset` change nothing (the property inherits). `font` does not reset it (css-fonts-4 §2.8's
/// reset list omits it; Chromium's `font` longhands too).
fn apply_font_synthesis<H: Host>(prop: &str, value: &str, paint: &mut PaintStyle<H>) {
    let Some((w, s, c)) = parse_font_synthesis(prop, value) else { return };
    if w.is_some() {
        paint.synth_weight = w;
    }
    if s.is_some() {
        paint.synth_style = s;
    }
    if c.is_some() { paint.synth_small_caps = c; }
}

/// AETHERFONTSYNTHESISCSSOM (SR764): the ONE `font-synthesis` keyword parse, shared by the setter above and Aether's
/// CSSOM (`expand`): the shorthand gives both halves (`initial` both auto, `none` both none, a keyword it omits none,
/// `small-caps` its third half, AETHERFONTSYNTHESISSMALLCAPS SR772), a longhand `auto | none | initial` its own half (Some(true) = auto). None for an
/// invalid value and for `inherit` / `unset` (the values the setter ignores: the property inherits).
pub fn parse_font_synthesis(prop: &str, value: &str) -> Option<(Option<bool>, Option<bool>, Option<bool>)> {
    let v = value.trim().to_ascii_lowercase();
    let auto = |s: &str| match s {
        "auto" | "initial" => Some(true),
        "none" => Some(false),
        _ => None,
    };
    let (w, s, c) = match prop {
        "font-synthesis-weight" => (Some(auto(&v)?), None, None),
        "font-synthesis-style" => (None, Some(auto(&v)?), None),
        "font-synthesis-small-caps" => (None, None, Some(auto(&v)?)),
        _ => match v.as_str() {
            "initial" => (Some(true), Some(true), Some(true)),
            "none" => (Some(false), Some(false), Some(false)),
            _ => {
                let (mut w, mut s, mut c) = (false, false, false);
                for k in v.split_whitespace() {
                    let seen = match k {
                        "weight" => &mut w,
                        "style" => &mut s,
                        "small-caps" => &mut c,
                        _ => return None,
                    };
                    if *seen {
                        return None;
                    }
                    *seen = true;
                }
                if !(w || s || c) {
                    return None;
                }
                (Some(w), Some(s), Some(c))
            }
        },
    };
    Some((w, s, c))
}

// ---- AETHERFONTVARIANTSMALLCAPS (LEDGER SR800): css-fonts-4 §6.6 font-variant-caps and font-variant's caps half (named tail block) ----

/// AETHERFONTVARIANTSMALLCAPS (SR800): `font-variant-caps` (the seven keywords, AETHERFONTVARIANTCAPSALL SR806) and the
/// caps half of the `font-variant` shorthand into `PaintStyle::small_caps` (inherited by the walkers' caps mask,
/// `PaintStyle::caps`). The other font-variant longhands are parsed and dropped (owed: AETHER-FONT-VARIANT-OTHERS).
fn apply_font_variant_caps<H: Host>(prop: &str, value: &str, paint: &mut PaintStyle<H>) {
    match parse_font_variant_caps(prop, value) {
        Some(on) => paint.small_caps = Some(on),
        None => {
            if !is_neutral_keyword(value) {
                H::record_css(&format!("{prop}-value:{}", clip(value)));
            }
        }
    }
}

/// AETHERFONTVARIANTSMALLCAPS (SR800): the caps value a `font-variant-caps` / `font-variant` declaration sets
/// (AETHERFONTVARIANTCAPSALL SR806: any of the seven keywords): `initial` is Normal; the shorthand's `normal` / `none` and
/// any value without a caps keyword are Normal (the shorthand resets its caps longhand), its last caps keyword wins;
/// None for an invalid longhand value and `inherit` / `unset` (the property inherits).
pub fn parse_font_variant_caps(prop: &str, value: &str) -> Option<crate::paint::Caps> {
    use crate::paint::Caps;
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "initial" => return Some(Caps::Normal),
        "inherit" | "unset" | "revert" | "revert-layer" | "" => return None,
        _ => {}
    }
    if prop == "font-variant-caps" {
        return Caps::of(&v);
    }
    let mut caps = Some(Caps::Normal);
    for k in v.split_whitespace() {
        if let Some(c) = Caps::of(k).filter(|c| *c != Caps::Normal) {
            caps = Some(c);
        }
    }
    caps
}

// ---- AETHERFONTSHORTHANDRESET (LEDGER SR805): css-fonts-4 §4.3 the font shorthand resets its omitted longhands (named tail block) ----

/// AETHERFONTSHORTHANDRESET (SR805): the four longhands the `font` prefix loop writes (font-style, font-weight,
/// font-stretch, font-variant-caps), as they stood before a `font` declaration.
type FontPrefix = (Option<bool>, Option<FontWeight>, Option<u16>, Option<crate::paint::Caps>); // fold seam SR805/SR806: Caps (AETHERFONTVARIANTCAPSALL)

/// AETHERFONTSHORTHANDRESET (SR805): `font` is a reset-all shorthand (css-fonts-4 §4.3): before its prefix applies,
/// the prefix longhands take their INITIAL values (Some, never None: None inherits): font-style normal, font-weight
/// 400, font-stretch 100%, font-variant-caps normal. Returns what they were, for [`restore_font_prefix`] when the
/// declaration turns out invalid. font-synthesis is not in the reset set and is not touched.
fn reset_font_prefix<H: Host>(paint: &mut PaintStyle<H>) -> FontPrefix {
    let prior = (paint.italic, paint.weight, paint.stretch, paint.small_caps);
    paint.italic = Some(false);
    paint.weight = Some(FontWeight::Abs(400));
    paint.stretch = Some(100);
    paint.small_caps = Some(crate::paint::Caps::Normal); // fold seam SR805/SR806
    prior
}

/// AETHERFONTSHORTHANDRESET (SR805): an invalid `font` declaration (its size does not parse) is dropped whole: the
/// prefix longhands go back to what they were before it (neither the reset nor the prefix leaks).
fn restore_font_prefix<H: Host>(paint: &mut PaintStyle<H>, prior: FontPrefix) {
    (paint.italic, paint.weight, paint.stretch, paint.small_caps) = prior;
}

/// AETHERFONTSHORTHANDRESET (SR805): once the size parsed, the shorthand's font-size goes through the `font-size` arm
/// (its relative mark, so an earlier `font-size: 2em` does not survive) and line-height resets to `normal` through
/// the `line-height` arm (its `font_rel` entry, so the font-relative replay does not bring an earlier line-height
/// back); the arm's own `/line-height` applies after this and wins.
fn reset_font_shorthand_rest<H: Host>(size: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    apply_declaration("font-size", size, style, cx);
    apply_declaration("line-height", "normal", style, cx);
}

// ---- AETHERFONTSHORTHANDINHERIT (LEDGER SR810): css-cascade-4 §7.3 the CSS-wide keywords on font reach its longhands (named tail block) ----

/// AETHERFONTSHORTHANDINHERIT (SR810): a CSS-wide keyword on the `font` shorthand applies to each longhand it resets
/// (css-cascade-4 §7.3). `inherit` / `unset` (every font longhand is inherited, so unset == inherit) take the
/// longhands' own inherit road, None: font-style / -weight / -stretch / -variant-caps through [`restore_font_prefix`],
/// line-height through the `line-height` arm (its `font_rel` entry, so the font-relative replay cannot bring an earlier
/// line-height back) then None, font-family None; the size takes the parent-relative road `smaller` / `larger` use
/// (font_size None, font_size_rel (0, 1.0)), because a None size is the tag's code default (13.333 px on a control),
/// not the parent's. `initial` is SR805's reset ([`reset_font_prefix`], [`reset_font_shorthand_rest`] at `medium`)
/// plus the standard family. `revert` (SR1028) reverts the seven fields to their tag roads; the six system fonts are SR1022's.
/// AETHERCSSWIDELONGHANDS (SR815): inherit / unset write the seven explicit inherit bits (`PaintStyle::set_inherit`).
fn apply_font_wide<H: Host>(kw: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    match kw {
        "inherit" | "unset" => {
            apply_declaration("line-height", "inherit", style, cx);
            style.paint.set_inherit(crate::paint::INHERIT_FONT);
        }
        "initial" => {
            reset_font_prefix(&mut style.paint);
            reset_font_shorthand_rest("medium", style, cx);
            style.paint.family = Some(crate::STANDARD);
        }
        "revert" => { apply_declaration("line-height", "revert", style, cx); style.paint.set_revert(crate::paint::INHERIT_FONT); } w if SYSTEM_FONTS.contains(&w) => { apply_font_wide("initial", style, cx); apply_declaration("font-size", &format!("{}px", crate::CONTROL_FONT_PX), style, cx); style.paint.family = Some(H::control_family()); } // SR1022
        _ => {} // SR1028: revert -> each font field's tag road; the six system fonts: dropped (AETHER-FONT-SHORTHAND-SYSTEM owed)
    }
}

// ---- AETHERCSSWIDELONGHANDS (LEDGER SR815): the font longhands' inherit / unset write the explicit inherit bit (named tail block) ----

/// AETHERCSSWIDELONGHANDS (SR815): css-cascade-4 §7.3.1 / §7.3.3: `inherit`, and `unset` on these inherited properties,
/// on one of the seven font longhands writes its explicit inherit bit (`PaintStyle::set_inherit`: the field's earlier
/// value goes) and ends the declaration: every arm's own inherit road was a no-op (None), which a later rule's Some or
/// the tag default overrode. Runs after the `font_rel` push, so line-height's replay entry stays `inherit`.
/// AETHERCSSWIDECOLOR (SR823): renamed from `font_css_wide`: every property with an inherit bit (`color` too); each is
/// inherited, so `unset` == `inherit` (css-cascade-4 §7.3.3).
fn css_wide<H: Host>(prop: &str, value: &str, paint: &mut PaintStyle<H>) -> bool {
    let bit = crate::paint::inherit_bit(prop); if bit != 0 && value.eq_ignore_ascii_case("revert") { paint.set_revert(bit); return true; } // AETHERCSSWIDEREVERT (SR1028): revert -> the tag road
    if bit == 0 || !(value.eq_ignore_ascii_case("inherit") || value.eq_ignore_ascii_case("unset") || (bit == crate::paint::INHERIT_COLOR && value.eq_ignore_ascii_case("currentcolor"))) { // AETHERCSSCOLORCURRENTCOLOR (SR841): css-color-4 §6.2 color: currentcolor == inherit
        return false;
    }
    paint.set_inherit(bit);
    true
}

// ---- AETHERCSSWIDENONINHERITEDINITIAL (LEDGER SR832): initial / unset on a non-inherited property writes its initial value (named tail block) ----

/// AETHERCSSWIDENONINHERITEDINITIAL (SR832): css-cascade-4 §7.3.1 / §7.3.3: `initial`, and `unset` on a NON-inherited
/// property, mean the property's initial value. Each rule parses into its own SpecifiedStyle and `merge_specified` /
/// `merge_paint` copy only Somes, so an arm that dropped the keyword let an earlier rule's value stand
/// (`.x { margin: 20px } .x.r { margin: initial }` kept 20px; `display: initial` fell to the tag road: block on a div).
/// The keyword is rewritten ONCE, before the dispatch, to the initial literal each arm already parses into its own
/// Some: margin / padding / background-color from `anim::initial_value` (its table not widened); `display` inline;
/// the sizes `auto` (max-width / max-height too: taffy's auto max is no maximum, `parse_dimension_str` has no `none`);
/// the border family and border styles `none`; `border-width` 3px (medium); `background` transparent (its image,
/// repeat, position and size layers stay as they were: AETHER-CSS-WIDE-NONINHERITED-REST owed). `revert` is
/// rewritten for the six sizes only (no tag road: SR1028); elsewhere the UA value is the tag road, which no Some
/// reaches, so it stays dropped (AETHER-CSS-WIDE-REVERT-NONINHERITED owed), nor `inherit` (AETHER-CSS-WIDE-NONINHERITED-INHERIT owed), nor any other property.
/// AETHERCSSWIDENONINHERITEDREST (SR848): opacity / transform / flex-grow / flex-shrink through the same anim call; position static, float / clear none, overflow visible, z-index and the insets auto, flex-direction row, flex-wrap nowrap, flex-flow `row nowrap`, flex-basis auto, flex `0 1 auto`, align-items / justify-content normal, gap / row-gap 0px, background-repeat / -position / -size repeat / `0% 0%` / auto; not column-gap (AETHER-CSS-WIDE-COLUMN-GAP-NORMAL), align-content (AETHER-CSS-ALIGN-CONTENT-FIELD) or the background shorthand's image layers (AETHER-CSS-WIDE-BACKGROUND-IMAGE-NONE).
/// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): `background` and `background-image` rewrite to `none`, which the arms now write as an explicit no-image (`bg_image_clear`, `bg_shorthand_none`), so `background: initial` resets the image, the colour and the repeat / position / size layers.
fn noninherited_initial<'a>(prop: &str, value: &'a str) -> &'a str {
    if !(value.eq_ignore_ascii_case("initial") || value.eq_ignore_ascii_case("unset") || (value.eq_ignore_ascii_case("revert") && matches!(prop, "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height"))) { // SR1028: no tag road for the sizes, so revert == initial
        return value;
    }
    match prop {
        "margin" | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding" | "padding-top"
        | "opacity" | "transform" | "flex-grow" | "flex-shrink"
        | "padding-right" | "padding-bottom" | "padding-left" | "background-color" => {
            crate::anim::initial_value(prop).unwrap_or(value)
        }
        "background" | "background-image" => "none",
        "display" => "inline",
        "color" if value.eq_ignore_ascii_case("initial") => "black", // SR1021: css-color-4 CanvasText (light scheme); unset stays inherit (css_wide)
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height" => "auto",
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" | "border-style"
        | "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => "none",
        "border-width" => "3px", "text-decoration-style" => "solid", "text-decoration-thickness" => "auto", "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => "currentcolor", "text-underline-offset" => "auto", // SR1014 fold: the branch's table row; SR1045 the offset's
        // AETHERCSSWIDENONINHERITEDREST (SR848): the rest of the non-inherited arms, each its arm's own literal.
        "position" => "static",
        "float" | "clear" => "none",
        "overflow" | "overflow-x" | "overflow-y" | "overflow-inline" | "overflow-block" => "visible",
        "z-index" => "auto",
        "top" | "right" | "bottom" | "left" | "inset" => "auto",
        "flex-direction" => "row",
        "flex-wrap" => "nowrap",
        "flex-flow" => "row nowrap",
        "flex-basis" => "auto",
        "flex" => "0 1 auto",
        "align-items" | "justify-content" | "align-content" | "justify-items" => "normal", "align-self" | "justify-self" => "auto",
        "gap" | "grid-gap" | "row-gap" | "grid-row-gap" => "0px", "column-gap" | "grid-column-gap" => "normal",
        "background-repeat" => "repeat",
        "background-position" => "0% 0%",
        "background-size" => "auto",
        _ => value,
    }
}

// ---- AETHERCSSCURRENTCOLORREST (LEDGER SR849): border / background currentColor marker writes (named tail block) ----

/// The background arms' marker (css-color-4 §6.2): a top-level `currentcolor` token writes the carrier and sets
/// [`crate::paint::CC_BG`] (paint resolves it to the element's ink); a concrete colour clears the bit (the arm's body
/// then writes that colour as before); anything else (transparent, none, an image alone) leaves it.
fn bg_currentcolor<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    let parts = split_top_level(value);
    if parts.iter().any(|p| p.eq_ignore_ascii_case("currentcolor")) {
        paint.background = Some((0, 0, 0));
        paint.bg_alpha = Some(1.0);
        paint.currentcolor |= crate::paint::CC_BG;
    } else if parts.iter().any(|p| parse_color_str::<H>(p).is_some()) || H::parse_color_alpha(value).is_some() {
        paint.currentcolor &= !crate::paint::CC_BG;
    }
}

/// The `border` / side shorthands' marker: css-backgrounds-3 §3.1 `border-color`'s initial value is currentcolor, so
/// a shorthand whose colour is omitted or `currentcolor` sets the `mask` side bits (the arm's grey is the carrier);
/// a concrete colour clears them.
fn border_cc<H: Host>(paint: &mut PaintStyle<H>, mask: u8, v: &str) {
    let parts = split_top_level(v);
    let mut rest = parts.iter().filter(|p| {
        border_style_code(p).is_none()
            && !p.starts_with(|c: char| c.is_ascii_digit() || c == '.')
            && !matches!(p.to_ascii_lowercase().as_str(), "thin" | "medium" | "thick")
    });
    let first = rest.next();
    if first.is_none() || first.into_iter().chain(rest).any(|p| p.eq_ignore_ascii_case("currentcolor")) {
        paint.currentcolor |= mask;
    } else {
        paint.currentcolor &= !mask;
    }
}

/// `border-color`'s colour: a parsed colour clears the side bits; `currentcolor` sets them and answers the grey
/// carrier, so the arm builds the sides as it does for a colour; anything else None.
fn border_color_of<H: Host>(paint: &mut PaintStyle<H>, value: &str) -> Option<(u8, u8, u8)> {
    if let Some(c) = border_rgb::<H>(value) {
        paint.currentcolor &= !crate::paint::CC_SIDES;
        Some(c)
    } else if value.trim().eq_ignore_ascii_case("currentcolor") {
        paint.currentcolor |= crate::paint::CC_SIDES;
        Some((128, 128, 128))
    } else {
        None
    }
}

// ---- AETHERCSSWIDEBACKGROUNDIMAGENONE (LEDGER SR1015): an explicit background-image none clears an earlier rule's image (named tail block) ----

/// AETHERCSSWIDEBACKGROUNDIMAGENONE (SR1015): css-backgrounds-3 §3.3: `background-image: none` is a value (no image
/// layer), but `PaintStyle::bg_image`'s None means "unspecified", so `merge_paint` (Somes only) kept an earlier rule's
/// url. The arm's `none` (and `initial` / `unset`, rewritten to `none` by `noninherited_initial`) clears both image
/// roads of this block (`bg_image` None, `bg_gradient` Some(None): background-image is ONE property) and sets
/// `bg_image_none`, which `merge_bg_image_none` turns into the same clear on the cascade's accumulated style.
fn bg_image_clear<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    if value.trim().eq_ignore_ascii_case("none") {
        paint.bg_image = None;
        paint.bg_gradient = Some(None);
        paint.bg_image_none = true; paint.bg_more = Some(Vec::new());
    }
}

/// The `background` shorthand's empty spellings (`none`, `transparent`, `none transparent`, and `initial` / `unset`
/// through `noninherited_initial`'s `none`): css-backgrounds-3 §3.10 resets every omitted longhand to its initial
/// value, so the image goes (`bg_image_clear`), the colour is transparent, and repeat / position / size take SR848's
/// initials (repeat / `0% 0%` / auto, stored as their arms store them; not through `apply_declaration`, which would
/// enter background-color / -position in `animatable`). Any other shorthand keeps its omitted layers
/// (no longer: SR1061 resets every shorthand through `bg_shorthand_reset` at both arm heads; this guard calls it too).
fn bg_shorthand_none<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    let parts = split_top_level(value);
    if parts.is_empty() || !parts.iter().all(|p| p.eq_ignore_ascii_case("none") || p.eq_ignore_ascii_case("transparent")) {
        return;
    }
    bg_shorthand_reset(paint, "none");
}

/// `merge_paint`'s half for the bit, BEFORE the image / gradient clones: a later block that declared none clears the
/// accumulated image and gradient and carries the bit (a chained merge clears again); that block's own later url or
/// gradient (`none; url(b)`) is then copied by the clones, so the later value wins either way.
fn merge_bg_image_none<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    if src.bg_image_none {
        dst.bg_image = None;
        dst.bg_gradient = Some(None);
        dst.bg_image_none = true;
    }
}

// ---- AETHERCSSALIGNCONTENTFIELD (LEDGER SR1016): align-content writes its own field; column-gap an explicit normal (named tail block) ----

/// AETHERCSSALIGNCONTENTFIELD (SR1016): css-align-3 §5.4 / css-flexbox-1 §8.4 `align-content` writes
/// `SpecifiedStyle::align_content`, taffy's own `Style::align_content`, folded on a real flex container by `fold_into`
/// (normal = taffy's NORMAL, stretch for flex; a single-line container shows no effect). The arm it shared with
/// `align-items` wrote `align_items` (`align-content: center` centred the items, `space-between` was refused).
/// `initial` / `unset` arrive here as `normal` through `noninherited_initial`; `inherit` stays neutral
/// (AETHER-CSS-WIDE-NONINHERITED-INHERIT). The multicol gutter's explicit `normal` is `PaintStyle::col_gap`'s Some(None).
fn apply_align_content<H: Host>(value: &str, style: &mut SpecifiedStyle<H>) {
    use taffy::style::AlignContent as A;
    style.align_content = Some(match value {
        "normal" => A::NORMAL,
        "flex-start" => A::FLEX_START,
        "start" => A::START,
        "flex-end" => A::FLEX_END,
        "end" => A::END,
        "center" => A::CENTER,
        "stretch" => A::STRETCH,
        "space-between" => A::SPACE_BETWEEN,
        "space-around" => A::SPACE_AROUND,
        "space-evenly" => A::SPACE_EVENLY,
        "inherit" | "initial" | "unset" => return,
        other => return H::record_css(&format!("align-content-value:{}", other)),
    });
}

// ---- AETHERCSSBORDERSIDECOLOR (LEDGER SR1014): the border-*-color longhands write one side's colour (named tail block) ----

/// AETHERCSSBORDERSIDECOLOR (SR1014): `transparent`, or any colour the host reads at alpha 0 (SR1119; `parse_color_str` refuses those), as a
/// border colour: a colour at alpha 0 (SR1112: the side is kept, its width sizes the box, `border_rgb` reads its RGB).
fn border_transparent<H: Host>(v: &str) -> bool {
    let v = v.trim().to_ascii_lowercase();
    v == "transparent" || H::parse_color_alpha(&v).is_some_and(|c| c.1 <= 0.0)
}

/// AETHERCSSBORDERSIDECOLOR (SR1014): `border-top-color` / `-right-` / `-bottom-` / `-left-color` (css-backgrounds-3
/// §3.1) write ONE side's colour, its width kept. A side with no entry has style none in Aether's model (a side paints
/// iff its entry is Some), so the longhand creates nothing there and no ink paints, as Chromium (the colour is not
/// stored: AETHER-CSS-BORDER-COLOR-STORED owed). The colour goes through `border_color_of` unchanged; only this side's
/// currentcolor bit keeps its answer, the other three are put back. `transparent` is alpha 0 on that side (SR1112).
fn side_color_of<H: Host>(side: usize, value: &str, paint: &mut PaintStyle<H>) {
    let Some(mut sides) = paint.border.filter(|b| b[side].is_some()) else { return side_color_pend::<H>(side, value, paint) };
    let before = paint.currentcolor;
    let Some(c) = border_color_of::<H>(paint, value) else {
        if !is_neutral_keyword(value) {
            H::record_css(&format!("border-side-color-value:{}", clip(value)));
        }
        return;
    };
    let bit = 1u8 << side;
    paint.currentcolor = (before & !bit) | (paint.currentcolor & bit);
    side_alpha_set(paint, bit, color_alpha::<H>(value));
    if let Some((w, _)) = sides[side] {
        sides[side] = Some((w, c));
    }
    paint.border = Some(sides);
}

// ---- AETHERCSSTEXTDECORATIONCOLOR (LEDGER SR1018): text-decoration-color and the shorthand's colour word (named tail block) ----

/// AETHERCSSTEXTDECORATIONCOLOR (SR1018): a `text-decoration-color` value in the [`PaintStyle::decoration_color`] marker
/// shape: `currentcolor`, `initial` and `unset` (not inherited: unset == initial, css-cascade-4 §7.3.3) are `Some(None)`;
/// a colour [`parse_color_str`] reads is `Some(Some(c))`; anything else (inherit, transparent, an invalid value) None.
fn deco_color_value<H: Host>(v: &str) -> Option<Option<(u8, u8, u8)>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("currentcolor") || v.eq_ignore_ascii_case("initial") || v.eq_ignore_ascii_case("unset") {
        return Some(None);
    }
    parse_color_str::<H>(v).map(Some)
}

/// AETHERCSSTEXTDECORATIONCOLOR (SR1018): the `text-decoration` shorthand (css-text-decor-3 §2.4) sets its colour
/// longhand: the first top-level token that is a colour (or `currentcolor`), else the initial currentcolor (the reset);
/// `inherit` / `revert` leave it. `text-decoration-line` never touches it.
/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): the same ONE pass reads the style word ([`deco_style_code`]) and the
/// thickness (`auto` / `from-font` / a length, [`deco_thickness_value`]) in any order beside the colour; each the value
/// omits resets to its initial (solid, auto).
fn deco_shorthand_rest<H: Host>(prop: &str, v: &str, paint: &mut PaintStyle<H>, cx: &ParseCx) {
    if prop != "text-decoration" || v.eq_ignore_ascii_case("inherit") || v.eq_ignore_ascii_case("revert") {
        return;
    }
    let (mut c, mut style, mut thick) = (None, None, None);
    for p in split_top_level(v) {
        if style.is_none() && !p.eq_ignore_ascii_case("initial") && !p.eq_ignore_ascii_case("unset") {
            if let Some(k) = deco_style_code(p) {
                style = Some(k);
                continue;
            }
        }
        if thick.is_none() && !p.eq_ignore_ascii_case("initial") && !p.eq_ignore_ascii_case("unset") {
            if let Some(t) = deco_thickness_value(p, cx) {
                thick = Some(t);
                continue;
            }
        }
        if c.is_none() {
            c = deco_color_value::<H>(p);
        }
    }
    paint.decoration_color = Some(c.unwrap_or(None));
    paint.decoration_style = Some(style.unwrap_or(0));
    paint.decoration_thickness = Some(thick.unwrap_or(None));
}

// ---- AETHERCSSWIDENONINHERITEDINHERIT (LEDGER SR1007): inherit on a non-inherited property writes an explicit-inherit bit the layout resolves against the parent (named tail block) ----

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): css-cascade-4 §7.3.2: `inherit` on a NON-inherited property is the
/// parent's computed value. The arms dropped it, so an earlier rule's value stood (`td { padding: inherit }` kept its
/// earlier padding). Eight families carry an explicit-inherit bit on [`SpecifiedStyle::inherit_box`], SR815's shape
/// (`PaintStyle::inherit`): the bit clears the family's earlier value, a later value beats the bit (`valued_box`), and
/// `merge_specified` carries later-wins both ways (`merge_inherit_box`). layout_core's `apply_specs` resolves each set
/// bit against the parent's resolved style (tree.rs `inherit_box_pass`). Margin and padding: one bit per side.
pub const INHERIT_BOX_MARGIN: u32 = 0xF;
pub const INHERIT_BOX_PADDING: u32 = 0xF0;
pub const INHERIT_BOX_DISPLAY: u32 = 256;
pub const INHERIT_BOX_WIDTH: u32 = 512;
pub const INHERIT_BOX_HEIGHT: u32 = 1024;
pub const INHERIT_BOX_POSITION: u32 = 2048;
/// The `border` shorthand only (its longhands: AETHER-CSS-WIDE-NONINHERITED-INHERIT-REST owed).
pub const INHERIT_BOX_BORDER: u32 = 4096;
pub const INHERIT_BOX_BACKGROUND_COLOR: u32 = 8192;

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): THE table: the explicit-inherit bit(s) of a non-inherited property,
/// else 0 (its `inherit` is dropped as before).
pub fn inherit_box_bit(prop: &str) -> u32 {
    match prop {
        "margin" => INHERIT_BOX_MARGIN,
        "margin-top" => 1,
        "margin-right" => 2,
        "margin-bottom" => 4,
        "margin-left" => 8,
        "padding" => INHERIT_BOX_PADDING,
        "padding-top" => 16,
        "padding-right" => 32,
        "padding-bottom" => 64,
        "padding-left" => 128,
        "display" => INHERIT_BOX_DISPLAY,
        "width" => INHERIT_BOX_WIDTH,
        "height" => INHERIT_BOX_HEIGHT,
        "position" => INHERIT_BOX_POSITION,
        "border" => INHERIT_BOX_BORDER,
        "background-color" => INHERIT_BOX_BACKGROUND_COLOR,
        _ => 0,
    }
}

impl<H: Host> SpecifiedStyle<H> {
    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): the box bits whose family holds a value here.
    pub fn valued_box(&self) -> u32 {
        let on = |v: bool, bit: u32| if v { bit } else { 0 };
        let mut bits = 0;
        for i in 0..4 {
            bits |= on(self.margin[i].is_some(), 1 << i) | on(self.padding[i].is_some(), 16 << i);
        }
        bits | on(self.display.is_some() || self.paint.display_kind.is_some(), INHERIT_BOX_DISPLAY)
            | on(self.width.is_some(), INHERIT_BOX_WIDTH)
            | on(self.height.is_some(), INHERIT_BOX_HEIGHT)
            | on(self.position.is_some() || self.paint.position_kind.is_some(), INHERIT_BOX_POSITION)
            | on(self.paint.border.is_some(), INHERIT_BOX_BORDER)
            | on(self.paint.background.is_some() || self.paint.bg_alpha.is_some(), INHERIT_BOX_BACKGROUND_COLOR)
    }

    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): the families this style explicitly inherits (bit set, no later value).
    pub fn inherits_box(&self) -> u32 {
        self.inherit_box & !self.valued_box()
    }

    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): writes the explicit inherit of `bits`: their families lose any
    /// earlier value.
    pub fn set_inherit_box(&mut self, bits: u32) {
        for i in 0..4 {
            if bits & (1 << i) != 0 { self.margin[i] = None; }
            if bits & (16 << i) != 0 { self.padding[i] = None; }
        }
        if bits & INHERIT_BOX_DISPLAY != 0 {
            (self.display, self.flex_container) = (None, None);
            (self.paint.display_kind, self.paint.flex_container, self.paint.list_item) = (None, None, None);
        }
        if bits & INHERIT_BOX_WIDTH != 0 { (self.width, self.paint.has_width) = (None, None); }
        if bits & INHERIT_BOX_HEIGHT != 0 { self.height = None; }
        if bits & INHERIT_BOX_POSITION != 0 { (self.position, self.paint.position_kind) = (None, None); }
        if bits & INHERIT_BOX_BORDER != 0 {
            (self.paint.border, self.paint.border_width, self.paint.border_style, self.paint.border_alpha) = (None, None, None, None); self.paint.border_pending = None;
        }
        if bits & INHERIT_BOX_BACKGROUND_COLOR != 0 { (self.paint.background, self.paint.bg_alpha) = (None, None); }
        self.inherit_box |= bits;
    }

    /// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `merge_specified`'s half for the box bits, after its Somes are
    /// copied (SR815's `merge_inherit` shape): a value `src` gives clears this style's bit, an explicit inherit it
    /// gives clears this style's earlier value and sets the bit.
    pub fn merge_inherit_box(&mut self, src: &Self) {
        let valued = src.valued_box();
        self.inherit_box &= !valued;
        self.set_inherit_box(src.inherit_box & !valued);
    }
}

/// AETHERCSSWIDENONINHERITEDINHERIT (SR1007): `inherit` on a property of [`inherit_box_bit`]'s table writes its bit and
/// ends the declaration (`unset` on these is SR832's initial rewrite, before this gate).
fn css_wide_box<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>) -> bool {
    let bit = inherit_box_bit(prop);
    if bit == 0 || !value.eq_ignore_ascii_case("inherit") {
        return false;
    }
    style.set_inherit_box(bit);
    true
}

// ---- AETHERCSSWIDECOLORCONTROLSDARK (LEDGER SR1031): the `color-scheme` property parsed into PaintStyle::color_scheme (named tail block) ----

/// AETHERCSSWIDECOLORCONTROLSDARK (SR1031): css-color-adjust-1 §2.1 `normal | [ light | dark | <custom-ident> ]+ && only?`
/// as the field's bits: bit 0 light, bit 1 dark; `normal` (and `initial`: the initial value) Some(0), so a `normal` under a
/// dark root resets (None is the inherit road). `only` and custom idents are accepted and ignored; `normal` beside another
/// keyword, a lone `only`, a second `only` or a non-ident token is invalid: None (the declaration is dropped). `inherit` /
/// `unset` never reach here (`css_wide`: the INHERIT_COLOR_SCHEME bit); `revert` is invalid here (dropped, as elsewhere).
pub fn parse_color_scheme(value: &str) -> Option<u8> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("normal") || v.eq_ignore_ascii_case("initial") {
        return Some(0);
    }
    let (mut bits, mut only, mut idents) = (0u8, false, 0usize);
    for tok in v.split_ascii_whitespace() {
        let ident = tok.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '-' || c == '_')
            && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ident {
            return None;
        }
        if tok.eq_ignore_ascii_case("only") {
            if only {
                return None;
            }
            only = true;
            continue;
        }
        if tok.eq_ignore_ascii_case("normal") || tok.eq_ignore_ascii_case("revert") || tok.eq_ignore_ascii_case("initial") {
            return None;
        }
        idents += 1;
        if tok.eq_ignore_ascii_case("light") {
            bits |= 1;
        } else if tok.eq_ignore_ascii_case("dark") {
            bits |= 2;
        }
    }
    if idents == 0 {
        return None;
    }
    Some(bits)
}

// ---- AETHERCSSOMUNICODEBIDI (LEDGER SR1008): css-writing-modes-4 §2.2 unicode-bidi keyword codes (named tail block) ----

/// AETHERCSSOMUNICODEBIDI (SR1008): the `unicode-bidi` keyword as its `PaintStyle::unicode_bidi` code: 0 normal, 1 embed,
/// 2 isolate, 3 bidi-override, 4 isolate-override, 5 plaintext (ASCII-case-insensitive). The property is NOT inherited, so
/// `initial` and `unset` are `normal` (0) and beat an earlier rule; `inherit` (owed to SR1007's non-inherited road),
/// `revert` and an unknown keyword give None: the declaration keeps the earlier value.
pub fn unicode_bidi_code(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" | "initial" | "unset" => Some(0),
        "embed" => Some(1),
        "isolate" => Some(2),
        "bidi-override" => Some(3),
        "isolate-override" => Some(4),
        "plaintext" => Some(5),
        _ => None,
    }
}

// ---- AETHERFONTRELINVALIDSUPERSEDES (LEDGER SR1055): font_rel records only the length declarations an arm accepted (named tail block) ----

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): what [`font_rel_record`] displaced: `None` when the property is not a length
/// property, else the earlier entry's index and value (`None` when there was none).
type FontRelUndo = Option<Option<(usize, String)>>;

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): records `(prop, value)` last in `PaintStyle::font_rel` (cascade order, one entry
/// per property: the earlier one goes) BEFORE the arm runs, so the css-wide roads and the arms' accepting early returns
/// stay recorded; returns what [`font_rel_undo`] puts back when the arm refuses the value.
fn font_rel_record<H: Host>(prop: &str, value: &str, paint: &mut PaintStyle<H>) -> FontRelUndo {
    if !is_length_property(prop) {
        return None;
    }
    let list = paint.font_rel.get_or_insert_with(Vec::new);
    let prev = list.iter().position(|(p, _)| p == prop).map(|i| (i, list.remove(i).1));
    list.retain(|(p, _)| p != prop);
    list.push((prop.to_string(), value.to_string()));
    Some(prev)
}

/// AETHERFONTRELINVALIDSUPERSEDES (SR1055): a length arm's refusal road: the refused value leaves `font_rel` and the entry it
/// displaced goes back at its original index, so the font-relative replay keeps the last ACCEPTED declaration
/// (`width: 2em; width: bogus` replays 2em). A neutral keyword ([`is_neutral_keyword`]) stays recorded.
fn font_rel_undo<H: Host>(paint: &mut PaintStyle<H>, prop: &str, fr: &FontRelUndo) {
    let (Some(prev), Some(list)) = (fr, paint.font_rel.as_mut()) else { return };
    let Some(i) = list.iter().rposition(|(p, _)| p == prop) else { return };
    if is_neutral_keyword(&list[i].1) {
        return;
    }
    list.remove(i);
    if let Some((at, v)) = prev {
        list.insert((*at).min(list.len()), (prop.to_string(), v.clone()));
    }
}

// ---- AETHERCSSWIDEALL (LEDGER SR1021): css-cascade-4 §3.1 the all shorthand over ONE longhand list (named tail block) ----

/// AETHERCSSWIDEALL (SR1021): the longhands `all` resets (css-cascade-4 §3.1): every longhand arm of
/// [`apply_declaration`] except `direction`. THE one list of the parsed longhand names; `css_wide_all_list_covers_arms`
/// (tests/css_wide_initial.rs) pins that every arm name is here or in [`ALL_SKIPPED`], so a new arm reds the pin.
pub const ALL_LONGHANDS: &[&str] = &[
    "display", "list-style-type", "content", "counter-reset", "counter-set", "counter-increment", "quotes",
    "flex-direction", "flex-wrap", "row-gap", "column-gap", "flex-grow", "flex-shrink", "flex-basis",
    "width", "min-width", "max-width", "height", "min-height", "max-height",
    "table-layout", "border-collapse", "caption-side", "border-spacing",
    "overflow-x", "overflow-y", "scrollbar-gutter", "scroll-behavior", "scroll-snap-type", "scroll-snap-align", "scroll-snap-stop",
    "scroll-padding-top", "scroll-padding-right", "scroll-padding-bottom", "scroll-padding-left",
    "scroll-margin-top", "scroll-margin-right", "scroll-margin-bottom", "scroll-margin-left",
    "padding-top", "padding-right", "padding-bottom", "padding-left",
    "margin-top", "margin-right", "margin-bottom", "margin-left",
    "position", "z-index", "top", "right", "bottom", "left",
    "aspect-ratio", "transform", "translate", "rotate", "scale", "transform-origin", "transform-style", "backface-visibility", "perspective", "perspective-origin", "transform-box",
    "clip-path", "will-change", "container-type", "container-name", "contain",
    "writing-mode", "text-combine-upright", "text-orientation",
    "column-count", "column-width", "column-fill", "column-rule-width", "column-rule-style", "column-rule-color", "column-span", "box-decoration-break",
    "filter", "backdrop-filter", "break-before", "break-after", "break-inside",
    "vertical-align", "clear", "float", "text-indent", "text-align", "text-align-last", "justify-content", "align-items", "align-content", "align-self", "justify-items", "justify-self", // SR1068 SR1088
    "background-color", "background-image", "background-size", "background-position", "background-repeat", "box-shadow",
    "color", "font-family", "font-stretch", "font-synthesis-weight", "font-synthesis-style", "font-synthesis-small-caps", "font-variant-caps", "font-size", "font-weight", "font-style", "line-height",
    "word-spacing", "letter-spacing", "text-shadow", "visibility", "opacity",
    "text-decoration-line", "white-space", "white-space-collapse", "text-wrap-mode", "word-break", "overflow-wrap", "box-sizing", "text-transform",
    "object-fit", "object-position", "mask-image", "mask-size", "mask-position", "mask-repeat",
    "border-top-width", "border-right-width", "border-bottom-width", "border-left-width",
    "border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius",
    "border-top-style", "border-right-style", "border-bottom-style", "border-left-style",
    "outline-style", "outline-width", "outline-color", "outline-offset",
    "cursor",
    // SR1021 fold: the arms landed on trunk before this fold (SR1014 border side colours, SR1018 text-decoration-color).
    "border-top-color", "border-right-color", "border-bottom-color", "border-left-color", "text-decoration-color",
    // SR1038 / SR1045 folds: text-decoration-style / -thickness, text-underline-offset / -position (the arms landed with their folds).
    "text-decoration-style", "text-decoration-thickness", "text-underline-offset", "text-underline-position",
    "caret-color", // SR1040 fold
    "color-scheme", // SR1031 fold: the color-scheme arm (inherited; `all` resets it)
];

/// AETHERCSSWIDEALL (SR1021): the arm names `all` does not iterate, by reason.
pub const ALL_SKIPPED: &[&str] = &[
    // css-cascade-4 §3.1: `all` leaves `direction` and `unicode-bidi` (its arm is SR1008's; the SR1021 fold added the name); `all` is this arm.
    "direction", "unicode-bidi", "all",
    // SHORTHANDS: reset through their longhands, which are listed (the `border-width` shorthand under initial / unset would write 3px into every side of `paint.border` and re-create the sides `border-*-style: none` cleared).
    "list-style", "text-wrap", "flex-flow", "gap", "flex", "overflow", "scroll-padding", "scroll-margin", "padding", "margin", "inset", "container", "columns", "column-rule", "background", "font-synthesis", "font-variant", "font", "border", "border-top", "border-right", "border-bottom", "border-left", "border-style", "border-color", "border-width", "border-radius", "text-decoration", "mask", "outline",
    // LOGICAL names: `apply_logical` / the scroll-margin arms re-apply them as the physical longhands listed.
    "overflow-inline", "overflow-block", "scroll-margin-block", "scroll-margin-inline", "scroll-margin-block-start", "scroll-margin-block-end", "scroll-margin-inline-start", "scroll-margin-inline-end",
    // ALIAS spellings of a listed name (the same field, the same value grammar).
    "grid-gap", "grid-row-gap", "grid-column-gap", "-webkit-text-combine", "-ms-text-combine-horizontal", "page-break-before", "page-break-after", "page-break-inside", "-webkit-column-break-before", "-webkit-column-break-after", "-webkit-column-break-inside", "-webkit-box-shadow", "word-wrap", "-webkit-mask", "-webkit-mask-image", "-webkit-mask-size", "-webkit-mask-position", "-webkit-mask-repeat", "place-content", "place-items", "place-self", // SR1074
];

/// AETHERCSSWIDEALL (SR1021): `all: inherit | initial | unset` applies the keyword to each of [`ALL_LONGHANDS`] through
/// [`apply_declaration`] itself, so each rides its own road: the inherit bits (SR815 / SR823 / SR831) and SR832's
/// [`noninherited_initial`] table; an arm with no keyword road drops it as when the keyword is written alone. The later
/// declaration wins through the same field writes. `revert` (SR1028) rides the same loop (`css_wide`'s revert road);
/// `revert-layer` is dropped (AETHER-CSS-WIDE-REVERT-LAYER owed); any other value is invalid: dropped and recorded.
fn apply_css_wide_all<H: Host>(value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    let kw = value.trim().to_ascii_lowercase();
    match kw.as_str() {
        "inherit" | "initial" | "unset" | "revert" => {
            for &name in ALL_LONGHANDS {
                apply_declaration(name, &kw, style, cx);
            }
        }
        "revert-layer" => {}
        _ => H::record_css(&format!("all-value:{}", clip(value))),
    }
}

// ---- AETHERCSSWIDEREVERTNONINHERITED (LEDGER SR1037): revert on display / margin / padding / border / background erases to the tag road (named tail block) ----

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the `revert_spec` bit of `display` (its five fields: display, flex_container
/// and paint's flex_container / display_kind / list_item).
pub const REVERT_DISPLAY: u32 = 1;
/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the four margin sides, side i = `2 << i` ([top, right, bottom, left]).
pub const REVERT_MARGIN: u32 = 0b1111 << 1;
/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the four padding sides, side i = `32 << i`.
pub const REVERT_PADDING: u32 = 0b1111 << 5;
/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the four border sides, side i = `512 << i`; all four erase the whole border.
pub const REVERT_BORDER: u32 = 0b1111 << 9;
/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): the background colour (background, bg_alpha, CC_BG).
pub const REVERT_BACKGROUND: u32 = 1 << 13;

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): css-cascade-4 §7.3.2 `revert` on the five non-inherited families whose UA
/// value is a per-tag table applied where the field is None at fold time (tree.rs `ua_margin`, the td / ul padding, the
/// display tag road `is_inline_node`, the hr / control border, `BUTTON_FACE`): the field is erased here (aether folds
/// every declaration into ONE style) and its bit set in `revert_spec`, which `merge_specified` reads to erase the earlier
/// rule's value before its Some-copy (SR1028's `set_revert` shape). The four background layer longhands have no per-tag
/// table, so revert == initial there. Border width / colour revert is dropped (AETHER-CSS-WIDE-REVERT-BORDER-PARTS owed).
fn apply_revert_spec<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) -> bool {
    if !value.eq_ignore_ascii_case("revert") {
        return false;
    }
    let side = |group: u32, p: &str| (group & group.wrapping_neg()) << side_index(p);
    let bits = match prop {
        "display" => REVERT_DISPLAY,
        "margin" => REVERT_MARGIN,
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => side(REVERT_MARGIN, prop),
        "padding" => REVERT_PADDING,
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => side(REVERT_PADDING, prop),
        "border" | "border-style" => REVERT_BORDER,
        "border-top" | "border-right" | "border-bottom" | "border-left" => side(REVERT_BORDER, prop),
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => side(REVERT_BORDER, prop.trim_end_matches("-style")),
        "background-color" => REVERT_BACKGROUND,
        "background" => {
            apply_declaration("background", "initial", style, cx);
            REVERT_BACKGROUND
        }
        "background-image" | "background-size" | "background-position" | "background-repeat" => {
            apply_declaration(prop, "initial", style, cx);
            return true;
        }
        _ => return false,
    };
    erase_reverted(style, bits);
    style.revert_spec |= bits;
    true
}

/// AETHERCSSWIDEREVERTNONINHERITED (SR1037): erases the fields of `bits` to None (the tag road). One border side None is
/// style none (every non-control tag's UA value); all four sides erase the whole border, so a control gets its UA frame.
pub fn erase_reverted<H: Host>(s: &mut SpecifiedStyle<H>, bits: u32) {
    if bits == 0 {
        return;
    }
    if bits & REVERT_DISPLAY != 0 {
        s.display = None;
        s.flex_container = None;
        s.paint.flex_container = None;
        s.paint.display_kind = None;
        s.paint.list_item = None;
    }
    for i in 0..4 {
        if bits & (2 << i) != 0 {
            s.margin[i] = None;
        }
        if bits & (32 << i) != 0 {
            s.padding[i] = None;
        }
        if bits & (512 << i) != 0 {
            if let Some(b) = s.paint.border.as_mut() {
                b[i] = None;
            }
            if let Some(w) = s.paint.border_width.as_mut() {
                w[i] = None;
            }
            s.paint.currentcolor &= !(1u8 << i);
        }
    }
    if bits & REVERT_BORDER == REVERT_BORDER {
        s.paint.border = None;
        s.paint.border_width = None;
        s.paint.border_style = None;
        s.paint.border_alpha = None;
    }
    if bits & REVERT_BACKGROUND != 0 {
        s.paint.background = None;
        s.paint.bg_alpha = None;
        s.paint.currentcolor &= !crate::paint::CC_BG;
    }
}

// ---- AETHERFONTSHORTHANDSYSTEM (LEDGER SR1022): css-fonts-4 §4.3.1 the six system-font keywords of `font` (named tail block) ----

/// AETHERFONTSHORTHANDSYSTEM (SR1022): the `font` shorthand's system-font keywords. Chromium resolves all six to the same
/// UI font (13.333px, normal weight and style); [`apply_font_wide`] gives them the `initial` road, then the control
/// family (`Host::control_family`, the UA control font) at [`crate::CONTROL_FONT_PX`] through the `font-size` arm.
const SYSTEM_FONTS: [&str; 6] = ["caption", "icon", "menu", "message-box", "small-caption", "status-bar"];

// ---- AETHERCOLORSCHEMESYSTEMCOLORS (LEDGER SR1046): the system colour keyword slots written at parse (named tail block) ----

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): css-color-4 §6.2 a system colour keyword's index in [`crate::SYSTEM_COLORS`]
/// (case-insensitive), else None. The parse still writes css_core's light value as the field's carrier; the index is
/// the marker `tree::system_color_pass` resolves against the element's used `color-scheme`.
pub fn system_color_index(name: &str) -> Option<u8> {
    let name = name.trim();
    crate::SYSTEM_COLORS.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): the `color` / background arms' slot: a CSS-wide keyword leaves it (the arm
/// writes nothing either); otherwise the first top-level component that is a system colour keyword, else None (a plain
/// colour, `currentcolor`, an image: the field holds no system colour from this declaration).
fn sys_slot(slot: &mut Option<u8>, value: &str) {
    if !is_neutral_keyword(value) {
        *slot = split_top_level(value).into_iter().find_map(system_color_index);
    }
}

/// AETHERCOLORSCHEMESYSTEMCOLORS (SR1046): the border arms' slot, `(index, side mask)` in the SR849 `CC_SIDES` shape:
/// `border` / `border-color` give `mask` = all four sides, a `border-*-color` longhand its one side. A CSS-wide keyword
/// leaves it. A system keyword puts `mask`'s sides on it; a side longhand's bit joins only when the other sides hold the
/// same keyword or none (one slot holds one keyword), else that side keeps its light carrier. No keyword clears
/// `mask`'s bits (a later `border-left-color: black` is not recoloured); an empty mask is None.
fn sys_border_slot<H: Host>(paint: &mut PaintStyle<H>, mask: u8, value: &str) {
    if is_neutral_keyword(value) {
        return;
    }
    let (old_k, old_m) = paint.sys_border.unwrap_or((0, 0));
    let rest = old_m & !mask;
    let (k, m) = match split_top_level(value).into_iter().find_map(system_color_index) {
        Some(k) if rest == 0 || old_k == k => (k, rest | mask),
        _ => (old_k, rest),
    };
    paint.sys_border = (m != 0).then_some((k, m));
}

// ---- AETHERCSSTEXTDECORATIONSTYLETHICKNESS (LEDGER SR1038): text-decoration-style and -thickness values (named tail block) ----

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a `text-decoration-style` keyword as [`PaintStyle::decoration_style`]'s
/// code: 0 solid 1 double 2 dotted 3 dashed 4 wavy; `initial` / `unset` (not inherited) are solid; anything else None.
pub fn deco_style_code(v: &str) -> Option<u8> {
    let v = v.trim();
    let is = |k: &str| v.eq_ignore_ascii_case(k);
    if is("solid") || is("initial") || is("unset") {
        Some(0)
    } else if is("double") {
        Some(1)
    } else if is("dotted") {
        Some(2)
    } else if is("dashed") {
        Some(3)
    } else if is("wavy") {
        Some(4)
    } else {
        None
    }
}

/// AETHERCSSTEXTDECORATIONSTYLETHICKNESS (SR1038): a `text-decoration-thickness` value in the
/// [`PaintStyle::decoration_thickness`] marker shape: `auto`, `from-font` (the face's thickness either way:
/// AETHER-CSS-TEXT-DECORATION-FROM-FONT owed), `initial` and `unset` are `Some(None)`; a non-negative length is
/// `Some(Some(px))` through [`parse_px`] (em at the parse context's font); a bare non-zero number, a percentage
/// (AETHER-CSS-TEXT-DECORATION-THICKNESS-PERCENT owed) or anything else None.
fn deco_thickness_value(v: &str, cx: &ParseCx) -> Option<Option<f32>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("from-font") || v.eq_ignore_ascii_case("initial")
        || v.eq_ignore_ascii_case("unset")
    {
        return Some(None);
    }
    if v.ends_with('%') || !v.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let bare = v.chars().all(|c| c.is_ascii_digit() || c == '.');
    if bare && v.parse::<f32>().ok() != Some(0.0) {
        return None;
    }
    parse_px(v, cx).filter(|px| *px >= 0.0).map(Some)
}

// ---- AETHERCSSTEXTUNDERLINEOFFSET (LEDGER SR1045): text-underline-offset and text-underline-position values (named tail block) ----

/// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): a `text-underline-position` value (css-text-decor-4 §2.5:
/// `auto | [ from-font | under ] || [ left | right ]`) as [`PaintStyle::underline_position`]'s code: 0 auto (and
/// `initial`), 1 from-font, 2 under. `left` / `right` place the line in VERTICAL writing only, so they keep the
/// horizontal part (bare `left` / `right` = auto; AETHER-CSS-TEXT-UNDERLINE-POSITION-VERTICAL owed). Anything else None
/// (`inherit` / `unset` never reach here: the inherit bit takes them in `css_wide`).
pub fn underline_position_code(v: &str) -> Option<u8> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("initial") {
        return Some(0);
    }
    let (mut horiz, mut side) = (None, false);
    for w in v.split_ascii_whitespace() {
        let is = |k: &str| w.eq_ignore_ascii_case(k);
        if (is("from-font") || is("under")) && horiz.is_none() {
            horiz = Some(if is("under") { 2 } else { 1 });
        } else if (is("left") || is("right")) && !side {
            side = true;
        } else {
            return None;
        }
    }
    if horiz.is_none() && !side {
        return None;
    }
    Some(horiz.unwrap_or(0))
}

/// AETHERCSSTEXTUNDERLINEOFFSET (SR1045): a `text-underline-offset` value (css-text-decor-4 §2.6) in the
/// [`PaintStyle::underline_offset`] marker shape: `auto` (and `initial` / `unset`, which SR832's table rewrites to it)
/// is `Some(None)`; a length, negative allowed, is `Some(Some(px))` through [`parse_px`] (em at the parse context's
/// font); a percentage is of 1em (`N%` = `N/100 em`); a bare non-zero number, `inherit` or anything else None.
fn underline_offset_value(v: &str, cx: &ParseCx) -> Option<Option<f32>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("initial") || v.eq_ignore_ascii_case("unset") {
        return Some(None);
    }
    if let Some(n) = v.strip_suffix('%') {
        let pct = n.trim().parse::<f32>().ok()?;
        return parse_px(&format!("{}em", pct / 100.0), cx).map(Some);
    }
    if !v.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+') {
        return None;
    }
    let bare = v.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
    if bare && v.parse::<f32>().ok() != Some(0.0) {
        return None;
    }
    parse_px(v, cx).filter(|px| px.is_finite()).map(Some)
}

// ---- AETHERCSSPLACESHORTHANDS (LEDGER SR1074): css-align-3 §6.4 / §7.4 / §8.3 place-content / place-items / place-self (named tail block) ----

/// AETHERCSSPLACESHORTHANDS (SR1074): `place-*: <align> <justify>?`, one or two [`alignment_words`] values (a
/// `safe center` / `first baseline` pair is ONE), the second defaulting to the first, each applied through its
/// longhand's arm (as `flex-flow` recurses); the justify half through justify-content / justify-items / justify-self
/// (AETHERCSSJUSTIFYITEMSSELF SR1088: before, the items / self half was recorded `place-justify-dropped:`); a value
/// with no two valid halves is refused whole (`place-value:`) and nothing is written.
fn apply_place<H: Host>(align: &str, justify: &str, value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    let t: Vec<&str> = value.split_ascii_whitespace().collect();
    let halves = (0..=2usize).find_map(|k| {
        let (a, b) = if k == 0 { (t.join(" "), t.join(" ")) } else if k < t.len() { (t[..k].join(" "), t[k..].join(" ")) } else { return None };
        (alignment_words(&a).is_some() && alignment_words(&b).is_some()).then_some((a, b))
    });
    let Some((a, b)) = halves else { return H::record_css(&format!("place-value:{}", clip(value))) };
    apply_declaration(align, &a, style, cx);
    apply_declaration(justify, &b, style, cx);
}

// ---- AETHERCSSJUSTIFYITEMSSELF (LEDGER SR1088): css-align-3 §6.2 / §6.3 justify-items / justify-self and the one self/items keyword map (named tail block) ----

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): THE keyword map of `align-items`, `align-self`, `justify-items` and
/// `justify-self` over the word [`align_word`] gives (taffy's `AlignSelf` / `JustifyItems` / `JustifySelf` are all
/// `AlignItems`). `Some(Some(v))` writes v; `auto` (only when `auto`: the *-self twins) is `Some(None)`, taffy's None;
/// `inherit` (and, for the *-items twins, `initial` / `unset`) is dropped silently; anything else is refused and
/// recorded `<prop>-value:<word>`. Byte-identical to the align-items arm's inline match and SR1068's map it replaces.
fn self_alignment<H: Host>(prop: &str, word: &str, auto: bool) -> Option<Option<taffy::style::AlignItems>> {
    use taffy::style::AlignItems as A;
    match word {
        "auto" if auto => Some(None),
        "center" => Some(Some(A::CENTER)),
        "flex-end" | "end" => Some(Some(A::END)),
        "flex-start" | "start" => Some(Some(A::START)),
        "baseline" => Some(Some(A::BASELINE)),
        "stretch" | "normal" => Some(Some(A::STRETCH)),
        "inherit" => None,
        "initial" | "unset" if !auto => None,
        other => {
            H::record_css(&format!("{}-value:{}", prop, other));
            None
        }
    }
}

/// AETHERCSSJUSTIFYITEMSSELF (SR1088): css-align-3 §6.2 `justify-self` into its own field, SR1068's shape: `auto` (and
/// `initial` / `unset` through [`noninherited_initial`]) is taffy's None marked `justify_self_auto`, so
/// [`merge_specified`] carries it over an earlier rule. No float guard (the float approximation writes align-self only).
/// taffy's flexbox ignores it (correct: css-align-3 §6.1); its out-of-flow pass aligns an abspos box by it.
fn apply_justify_self<H: Host>(word: &str, style: &mut SpecifiedStyle<H>) {
    if let Some(v) = self_alignment::<H>("justify-self", word, true) {
        style.justify_self = v;
        style.justify_self_auto = v.is_none();
    }
}

// ---- AETHERCSSOMWHITESPACELONGHANDSWRITE (LEDGER SR1044): white-space-collapse / text-wrap-mode / text-wrap write the one white-space code (named tail block) ----

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044; lifted from aether's css/mod.rs, AETHERCSSOMCOLUMNSREST SR1036): a
/// `PaintStyle::white_space` code as its css-text-4 §4 longhand pair `(white-space-collapse, text-wrap-mode)`, as
/// Chromium's getComputedStyle reports them. THE one table: the longhand arms, the resolve and the cssom read it.
pub fn white_space_longhands(c: u8) -> (&'static str, &'static str) {
    match c {
        1 => ("collapse", "nowrap"),
        2 => ("preserve", "nowrap"),
        3 => ("preserve", "wrap"),
        4 => ("preserve-breaks", "wrap"),
        5 => ("break-spaces", "wrap"),
        _ => ("collapse", "wrap"),
    }
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): the inverse of [`white_space_longhands`]: the code of a longhand pair,
/// None for a pair no code holds (preserve-breaks / break-spaces with nowrap: AETHER-CSS-WHITE-SPACE-PAIRS-REST owed).
pub fn white_space_from_longhands(collapse: &str, wrap: &str) -> Option<u8> {
    (0..6u8).find(|&c| white_space_longhands(c) == (collapse, wrap))
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): THE resolve every white-space walker calls over its `or_tag` result
/// `base` (the parent's code through the tag default): a lone longhand half replaces that half of `base`; no half, or a
/// pair no code holds, is `base`.
pub fn white_space_halves<H: Host>(paint: Option<&PaintStyle<H>>, base: u8) -> u8 {
    let Some(p) = paint else { return base };
    if p.ws_collapse.is_none() && p.ws_wrap.is_none() {
        return base;
    }
    let collapse = white_space_longhands(p.ws_collapse.unwrap_or(base)).0;
    let wrap = white_space_longhands(p.ws_wrap.unwrap_or(base)).1;
    white_space_from_longhands(collapse, wrap).unwrap_or(base)
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): THE combine (the arm within a block, `merge_white_space_halves` across
/// rules): onto a full code the given half makes the new full code (and its nowrap); with no code the half is recorded
/// for the walkers' resolve, a wrap half writing its nowrap (the run bool only promotes normal to nowrap).
fn ws_put_halves<H: Host>(p: &mut PaintStyle<H>, collapse: Option<u8>, wrap: Option<u8>) {
    if collapse.is_none() && wrap.is_none() {
        return;
    }
    match p.white_space {
        Some(c) => {
            let pair = (white_space_longhands(collapse.unwrap_or(c)).0, white_space_longhands(wrap.unwrap_or(c)).1);
            if let Some(n) = white_space_from_longhands(pair.0, pair.1) {
                p.white_space = Some(n);
                p.nowrap = Some(matches!(n, 1 | 2));
            }
        }
        None => {
            if collapse.is_some() {
                p.ws_collapse = collapse;
            }
            if let Some(w) = wrap {
                p.ws_wrap = Some(w);
                p.nowrap = Some(matches!(w, 1 | 2));
            }
        }
    }
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): the arms of `white-space-collapse`, `text-wrap-mode` and the
/// `text-wrap` shorthand (css-text-4 §4 / §5): each value is found THROUGH [`white_space_longhands`] (a half is stored
/// as a code whose that half is the value). `text-wrap` is `<text-wrap-mode> || <text-wrap-style>`; an omitted mode is
/// its initial `wrap`; a style other than `auto` is recorded and dropped (AETHER-CSS-TEXT-WRAP-STYLE owed). Any other
/// value (`white-space-collapse: pre`, `discard`, `preserve-spaces`, a bogus word) is dropped.
fn apply_white_space_longhand<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>) {
    let v = value.to_ascii_lowercase();
    let collapse_code = |w: &str| (0..6u8).find(|&c| white_space_longhands(c).0 == w);
    let wrap_code = |w: &str| (0..6u8).find(|&c| white_space_longhands(c).1 == w);
    match prop {
        "white-space-collapse" => {
            if let Some(c) = collapse_code(&v) {
                ws_put_halves(&mut style.paint, Some(c), None);
            }
        }
        "text-wrap-mode" => {
            if let Some(c) = wrap_code(&v) {
                ws_put_halves(&mut style.paint, None, Some(c));
            }
        }
        _ => {
            let (mut mode, mut wrap_style, mut n) = (None, None, 0);
            for tok in v.split_whitespace() {
                n += 1;
                match tok {
                    "wrap" | "nowrap" if mode.is_none() => mode = Some(tok),
                    "auto" | "balance" | "pretty" | "stable" if wrap_style.is_none() => wrap_style = Some(tok),
                    _ => return,
                }
            }
            if n == 0 {
                return;
            }
            if let Some(s) = wrap_style.filter(|s| *s != "auto") {
                H::record_css(&format!("text-wrap-style:{}", s));
            }
            if let Some(c) = wrap_code(mode.unwrap_or("wrap")) {
                ws_put_halves(&mut style.paint, None, Some(c));
            }
        }
    }
}

/// AETHERCSSOMWHITESPACELONGHANDSWRITE (SR1044): `merge_paint`'s half for the longhands, after `merge_inherit` (a later
/// rule's `white-space: inherit` has cleared the earlier halves): a later full code (already copied) clears the earlier
/// halves; later halves combine onto the earlier code, or are recorded over the earlier halves.
pub fn merge_white_space_halves<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    if src.white_space.is_some() {
        dst.ws_collapse = None;
        dst.ws_wrap = None;
    } else {
        ws_put_halves(dst, src.ws_collapse, src.ws_wrap);
    }
}

// ---- AETHERCSSCARETCOLOR (LEDGER SR1040): caret-color in the outline_color RGBA marker shape (named tail block) ----

/// AETHERCSSCARETCOLOR (SR1040): a `caret-color` value (css-ui-4 §4.1: `auto | <color>`) as [`PaintStyle::caret_color`]
/// holds it: `auto` and `initial` are `Some(None)` (the caret paints the control's currentcolor); `invert` is not a
/// caret-color value (None); the rest through [`outline_color_value`] (`currentcolor` `Some(None)`, `transparent` alpha
/// 0, which paints no caret, a colour with its alpha; an invalid value None). `inherit` / `unset` never reach here: the
/// property is inherited, so [`css_wide`] writes `INHERIT_CARET_COLOR`.
fn caret_color_value<H: Host>(v: &str) -> Option<Option<(u8, u8, u8, u8)>> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("initial") {
        return Some(None);
    }
    if v.eq_ignore_ascii_case("invert") {
        return None;
    }
    outline_color_value::<H>(v)
}

// ---- AETHERCSSSUPPORTSLIST (LEDGER SR1039): the vendor-alias spellings, ONE list the arm and @supports both ask (named tail block) ----

/// AETHERCSSSUPPORTSLIST (SR1039): the unprefixed property a `-webkit-` / `-moz-` / `-ms-` / `-o-` spelling aliases,
/// when this engine implements it with the SAME value grammar (moved out of [`apply_declaration`]'s alias arm, which
/// calls it). Only these are aliased: the old `-webkit-box-*` flexbox properties (box-flex / box-align / box-pack) take
/// different values and stay honestly unimplemented. None = not an alias spelling (aether's `@supports` asks this too).
pub fn vendor_alias_base(prop: &str) -> Option<&str> {
    let base = prop
        .strip_prefix("-webkit-")
        .or_else(|| prop.strip_prefix("-moz-"))
        .or_else(|| prop.strip_prefix("-ms-"))
        .or_else(|| prop.strip_prefix("-o-"))?;
    if matches!(
        base,
        "box-sizing" | "background-size" | "background-position"
            | "background-repeat" | "align-items" | "align-content"
            | "align-self" | "justify-content" | "flex-direction"
            | "flex-wrap" | "opacity" | "border-radius" | "flex" | "flex-grow"
            | "flex-shrink" | "flex-basis" | "flex-flow" | "gap"
            | "box-shadow" | "text-decoration" | "text-decoration-line" | "transform"
            | "transform-origin" | "aspect-ratio" | "clip-path" | "transform-style"
            | "backface-visibility" | "perspective" | "perspective-origin"
            | "filter" | "backdrop-filter" | "columns" | "column-count" | "column-width"
            | "column-gap" | "column-fill" | "column-rule" | "column-rule-width"
            | "column-rule-style" | "column-rule-color" | "column-span" | "box-decoration-break"
            | "writing-mode" | "text-orientation"
    ) {
        Some(base)
    } else {
        None
    }
}

// ---- AETHERCSSSUPPORTSVALUE (LEDGER SR1048): @supports asks the parse road whether the VALUE parses (named tail block) ----

/// AETHERCSSSUPPORTSVALUE (SR1048): where a [`ProbeHost`]'s coverage records go — the host's own per-thread flag
/// (this crate is `no_std` and keeps no state: a process-global flag would race between threads).
pub trait ProbeSink: Clone + core::fmt::Debug + Default + 'static {
    /// Clears the flag before a probe.
    fn reset();
    /// A record arrived: the probed value was dropped.
    fn mark();
    /// Whether any record arrived since [`ProbeSink::reset`].
    fn marked() -> bool;
}

/// AETHERCSSSUPPORTSVALUE (SR1048): the host `H` under a probe: every hook delegates to `H` (the probe parses exactly as
/// the engine does) except `record_css`, which marks `S` and never reaches `H`'s ledger. A hook added to [`Host`] must be
/// delegated here too, or the probe parses with the trait default.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeHost<H, S>(core::marker::PhantomData<(H, S)>);

impl<H: Host, S: ProbeSink> Host for ProbeHost<H, S> {
    type Node = H::Node;
    type Gradient = H::Gradient;
    type Shadow = H::Shadow;

    fn record_css(_what: &str) {
        S::mark();
    }
    fn parse_gradient(value: &str) -> Option<Self::Gradient> {
        H::parse_gradient(value)
    }
    fn parse_box_shadow(value: &str) -> Vec<Self::Shadow> {
        H::parse_box_shadow(value)
    }
    fn shadow_is_inset(s: &Self::Shadow) -> bool {
        H::shadow_is_inset(s)
    }
    fn parse_color_alpha(value: &str) -> Option<((u8, u8, u8), f32)> {
        H::parse_color_alpha(value)
    }
    fn intern_family_list(value: &str) -> Option<u16> {
        H::intern_family_list(value)
    }
    fn control_family() -> u16 {
        H::control_family()
    }
    fn parse_object_fit(value: &str) -> Option<u8> {
        H::parse_object_fit(value)
    }
    fn custom_prop(name: &str) -> Option<String> {
        H::custom_prop(name)
    }
    fn transform_text(text: &str, tt: u8) -> String {
        H::transform_text(text, tt)
    }
    fn media_intrinsic(node: &Self::Node) -> Option<(f32, f32)> {
        H::media_intrinsic(node)
    }
    fn img_intrinsic(node: &Self::Node) -> Option<(f32, f32)> {
        H::img_intrinsic(node)
    }
    fn img_state(node: &Self::Node) -> crate::ImgState {
        H::img_state(node)
    }
    fn option_selected(node: &Self::Node) -> Option<bool> {
        H::option_selected(node)
    }
}

/// AETHERCSSSUPPORTSVALUE (SR1048): the arms that record a coverage note for a VALID value (`clear` records
/// `property:clear` after writing any value), answered by name: the record oracle would call them unsupported.
pub const NAME_ONLY_PROPS: &[&str] = &["clear"];

/// AETHERCSSSUPPORTSVALUE (SR1048): css-conditional-3 §6.1 `@supports (prop: value)` — does this engine parse the
/// declaration? The parse road is the oracle: [`apply_declaration`] on a scratch style under [`ProbeHost`]; a dropped
/// value records (`<prop>-value:`, `display:grid`, `property:<name>`, ...) and answers false. A CSS-wide keyword, a
/// value carrying `var()` (css-variables-1 §3: valid at parse time) and a [`NAME_ONLY_PROPS`] name answer true; a value
/// an arm drops without a record still answers true (AETHER-CSS-SUPPORTS-NAME-ONLY-AUDIT owed). The name gate is the
/// caller's.
pub fn declaration_parses<H: Host, S: ProbeSink>(prop: &str, value: &str, cx: &ParseCx) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if ["inherit", "initial", "unset", "revert", "revert-layer"].iter().any(|k| value.eq_ignore_ascii_case(k))
        || value.to_ascii_lowercase().contains("var(")
        || NAME_ONLY_PROPS.contains(&prop)
    {
        return true;
    }
    S::reset();
    let mut scratch = SpecifiedStyle::<ProbeHost<H, S>>::default();
    apply_declaration(prop, value, &mut scratch, cx);
    !S::marked()
}

// ---- AETHERCSSBACKGROUNDMULTILAYER (LEDGER SR1052): background-image is a comma layer list, painted bottom-up (named tail block) ----

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): one comma-separated layer of `background-image` (or of the `background`
/// shorthand): a `url(...)` written before any `-gradient(` is a url layer, else a gradient the host parses; `none`
/// and a value neither road reads are no layer.
fn parse_bg_layer<H: Host>(layer: &str) -> Option<crate::paint::BgLayer<H>> {
    let lower = layer.to_ascii_lowercase();
    match (lower.find("url("), lower.find("-gradient(")) {
        (Some(u), Some(g)) if u < g => extract_css_url(layer).map(crate::paint::BgLayer::Url),
        (_, Some(_)) => H::parse_gradient(layer).map(crate::paint::BgLayer::Gradient),
        (Some(_), None) => extract_css_url(layer).map(crate::paint::BgLayer::Url),
        (None, None) => None,
    }
}

/// AETHERCSSBACKGROUNDMULTILAYER (SR1052): css-backgrounds-3 §2.2 / §3.3: `background-image` is a layer list, the first
/// layer topmost. The value splits at depth-0 commas (`anim::split_commas`: `rgb(0, 0, 0)`, `url(a,b)` and data: urls
/// stay whole); the first layer writes its own field (`bg_image` or `bg_gradient`, the other cleared: one property),
/// the rest go to `bg_more` (Some even when empty, so `merge_bg_layers` replaces an earlier rule's whole list). A value
/// with no readable layer changes nothing.
fn bg_image_layers<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    let mut layers = crate::anim::split_commas(value).into_iter().filter_map(parse_bg_layer::<H>);
    let Some(first) = layers.next() else {
        return;
    };
    paint.bg_image = None;
    paint.bg_gradient = Some(None);
    match first {
        crate::paint::BgLayer::Url(u) => paint.bg_image = Some(u),
        crate::paint::BgLayer::Gradient(g) => paint.bg_gradient = Some(Some(g)),
    }
    paint.bg_more = Some(layers.collect());
}

/// `merge_paint`'s half for the list, BEFORE the image / gradient / `bg_more` clones: a later block that declared a
/// layer list clears the accumulated first layer, so its own first layer (copied by the clones) and its `bg_more`
/// replace the whole earlier list (a url from one rule and a gradient from a later rule no longer both paint).
fn merge_bg_layers<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    if src.bg_more.is_some() {
        dst.bg_image = None;
        dst.bg_gradient = Some(None);
    }
}

// ---- AETHERCSSBACKGROUNDSHORTHANDRESET (LEDGER SR1061): the background shorthand resets every omitted longhand (named tail block) ----

/// AETHERCSSBACKGROUNDSHORTHANDRESET (SR1061): css-backgrounds-3 §3.10: both `background` arms call this at their head,
/// BEFORE `bg_currentcolor` (which re-sets CC_BG and the opaque alpha for a currentcolor token), so every longhand the
/// value omits takes its initial as an explicit Some that `merge_paint` copies over an earlier rule: no image
/// (`bg_image_clear`, SR1052's empty `bg_more`), transparent, repeat / `0% 0%` / auto; the arm's scans then write the
/// parts present. `inherit` / `revert` / `revert-layer` are left as they were (dropped: owed).
fn bg_shorthand_reset<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    if matches!(value.trim().to_ascii_lowercase().as_str(), "inherit" | "revert" | "revert-layer") {
        return;
    }
    bg_image_clear(paint, "none");
    paint.bg_alpha = Some(0.0);
    paint.currentcolor &= !crate::paint::CC_BG;
    paint.bg_repeat = parse_repeat("repeat");
    paint.bg_position = Some("0% 0%".to_string());
    paint.bg_size = Some("auto".to_string()); paint.bg_repeat_more = Some(Vec::new()); paint.bg_position_more = Some(Vec::new()); paint.bg_size_more = Some(Vec::new());
}

// ---- AETHERCSSBACKGROUNDLAYERPROPS (LEDGER SR1070): background-repeat / -position / -size are per-layer lists (named tail block) ----

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): css-backgrounds-3 §2.3: `background-repeat` / `-position` / `-size` are comma
/// layer lists matched to the image layers. The value splits at depth-0 commas (`anim::split_commas`, SR1052's
/// splitter); the first value goes to the first layer's field, the rest to its `_more` list (Some even when empty, so
/// `merge_paint` replaces an earlier block's whole list). Any empty or unparsable item drops the declaration (None).
fn bg_layer_prop_list<T>(value: &str, parse: impl Fn(&str) -> Option<T>) -> Option<(T, Vec<T>)> {
    let mut items = crate::anim::split_commas(value)
        .into_iter()
        .map(|v| if v.is_empty() { None } else { parse(v) })
        .collect::<Option<Vec<T>>>()?
        .into_iter();
    let first = items.next()?;
    Some((first, items.collect()))
}

/// AETHERCSSBACKGROUNDLAYERPROPS (SR1070): the `background` shorthand's repeat / position / size, per comma layer (moved
/// from the url arm, where it scanned every layer's words as one: the last repeat word won for all layers, the size
/// took the next layer's word). Each layer's components outside `url(...)` and any `-gradient(...)` (the url may hold
/// slashes and keywords) are scanned the way that arm scanned them. AETHERCSSSPLITAUDIT (SR426): the components, the
/// position / size `/` and the size pair at paren depth 0 (`rgb(0 0 0 / 50%)`, `calc(10px + 5px) 0 / calc(50% - 2px)
/// auto` stay whole). The first layer writes only the parts present (`bg_shorthand_reset` wrote the initials; an
/// `inherit` value writes nothing); with more than one layer, layers 2..n take their part or the initial
/// (repeat / `0% 0%` / auto) into the `_more` lists.
fn bg_shorthand_layer_props<H: Host>(paint: &mut PaintStyle<H>, value: &str) {
    let layers = crate::anim::split_commas(value);
    let (mut repeats, mut positions, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
    for (i, layer) in layers.iter().enumerate() {
        let stripped = strip_css_urls(layer);
        let outside = split_top_level(&stripped)
            .into_iter()
            .filter(|t| !t.to_ascii_lowercase().contains("-gradient("))
            .collect::<Vec<_>>()
            .join(" ");
        let repeat = split_top_level(&outside).into_iter().filter_map(parse_repeat).last();
        let (pos_part, size_part) = split_top_level_slash(&outside);
        let pos: String = split_top_level(pos_part)
            .into_iter()
            .filter(|t| is_position_token(t))
            .collect::<Vec<_>>()
            .join(" ");
        let size = size_part
            .map(|sz| split_top_level(sz).into_iter().take(2).collect::<Vec<_>>().join(" "))
            .filter(|sz| !sz.is_empty());
        let pos = Some(pos).filter(|p| !p.is_empty());
        if i == 0 {
            if repeat.is_some() {
                paint.bg_repeat = repeat;
            }
            if pos.is_some() {
                paint.bg_position = pos;
            }
            if size.is_some() {
                paint.bg_size = size;
            }
        } else {
            repeats.push(repeat.unwrap_or(0));
            positions.push(pos.unwrap_or_else(|| "0% 0%".to_string()));
            sizes.push(size.unwrap_or_else(|| "auto".to_string()));
        }
    }
    if layers.len() > 1 {
        paint.bg_repeat_more = Some(repeats);
        paint.bg_position_more = Some(positions);
        paint.bg_size_more = Some(sizes);
    }
}

// ---- AETHERCSSOMBORDERSIDESTYLECOLUMNS (LEDGER SR1103): the border-style paint code back to its keyword (named tail block) ----

/// AETHERCSSOMBORDERSIDESTYLECOLUMNS (SR1103): the inverse of [`border_style_code`] — 1 dashed, 2 dotted, 3 double,
/// every other code `solid` (the render's `style_on` default; groove / ridge / inset / outset collapse to 0 and read solid).
pub fn border_style_word(code: u8) -> &'static str {
    match code {
        1 => "dashed",
        2 => "dotted",
        3 => "double",
        _ => "solid",
    }
}

// ---- AETHERTEXTALIGNLAST (LEDGER SR1062): css-text-3 §7.2 text-align-last parses to the text-align codes (named tail block) ----

/// AETHERTEXTALIGNLAST (SR1062): css-text-3 §7.2 `text-align-last` to its code (the `text-align` table: 0 start,
/// 1 center, 2 right, 3 end, 4 left, 8 justify; 255 = `auto`, which `initial` also writes, so an explicit auto overrides
/// an inherited value). `inherit` / `unset` / `revert` are dropped as the `text-align` arm drops them (the field stays
/// unspecified and inherits: AETHER-TEXT-ALIGN-LAST-WIDE owed); anything else is recorded and dropped.
fn text_align_last_code<H: Host>(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" | "initial" => Some(255),
        "start" => Some(0),
        "center" => Some(1),
        "right" => Some(2),
        "end" => Some(3),
        "left" => Some(4),
        "justify" => Some(8),
        "inherit" | "unset" | "revert" | "revert-layer" => None,
        other => {
            H::record_css(&format!("text-align-last:{}", clip(other)));
            None
        }
    }
}

// ---- AETHERCSSOMTEXTDECORATIONCOLUMNS (LEDGER SR1108): the text-decoration-style and text-underline-position codes back to their keywords (named tail block) ----

/// AETHERCSSOMTEXTDECORATIONCOLUMNS (SR1108): the inverse of [`deco_style_code`] (0 solid 1 double 2 dotted 3 dashed 4 wavy).
pub fn text_decoration_style_word(code: u8) -> &'static str {
    ["solid", "double", "dotted", "dashed", "wavy"].get(code as usize).copied().unwrap_or("solid")
}

/// AETHERCSSOMTEXTDECORATIONCOLUMNS (SR1108): the inverse of [`underline_position_code`] (0 auto 1 from-font 2 under).
pub fn underline_position_word(code: u8) -> &'static str {
    ["auto", "from-font", "under"].get(code as usize).copied().unwrap_or("auto")
}

// ---- AETHERCSSBORDERCOLORSTORED (LEDGER SR1058): a pending per-side border colour; border-color alone builds no frame (named tail block) ----

/// AETHERCSSBORDERCOLORSTORED (SR1058): the ONE side-creating helper over `PaintStyle::border_pending`. Each side in
/// `mask` with no entry takes its pending colour (width: the stored `border_width` else medium 3px, SR1069's `side_width`;
/// before SR1069 1.0, the width the fabricated frame had) and the pending entry's currentcolor bit; with `grey`, a side with no pending colour takes the
/// grey carrier and the bit (the initial currentcolor: `border-width`'s road before SR1058). Sides with an entry stay.
fn border_create_sides<H: Host>(paint: &mut PaintStyle<H>, mask: u8, grey: bool) -> [Option<(f32, (u8, u8, u8))>; 4] {
    let mut sides = paint.border.unwrap_or([None; 4]);
    let pend = paint.border_pending.unwrap_or([None; 4]);
    for i in 0..4 {
        let bit = 1u8 << i;
        if mask & bit == 0 || sides[i].is_some() {
            continue;
        }
        match pend[i] {
            Some((c, cc)) => {
                sides[i] = Some((side_width(paint.border_width, i), c));
                if cc { paint.currentcolor |= bit } else { paint.currentcolor &= !bit }
            }
            None if grey => {
                sides[i] = Some((side_width(paint.border_width, i), (128, 128, 128)));
                paint.currentcolor |= bit;
            }
            None => {}
        }
    }
    sides
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): `border-style` / `border-*-style` with a style: the sides in `mask` with no entry
/// are created (`border-color: red; border-style: solid` keeps its frame). AETHERCSSBORDERSIDESTYLEWIDTH (SR1069):
/// css-backgrounds-3 §4.2-4.3: a side exists iff its style is not none, so a style alone creates the masked sides at the
/// stored width else medium 3px, in the pending colour else currentcolor (before SR1069: only where a colour pended).
fn border_pending_create<H: Host>(paint: &mut PaintStyle<H>, mask: u8) {
    paint.border = Some(border_create_sides(paint, mask, true));
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): the `border-color` arm's colour `c` (from `border_color_of`, which set or
/// cleared all four currentcolor bits) is pended for all four sides; the answer is the sides the arm recolours: this
/// block's entries, created first for the absent sides when the block declared a style (`border-style: solid;
/// border-color: red`), else as they are (an absent side stays absent: css-backgrounds-3 §3.2 style none paints
/// nothing). None (no border in this block, no style) = the arm writes nothing: the colour waits in the store, and
/// `merge_border_pending` lays it on an earlier rule's sides.
fn border_color_pend<H: Host>(paint: &mut PaintStyle<H>, c: (u8, u8, u8)) -> Option<[Option<(f32, (u8, u8, u8))>; 4]> {
    let cc = paint.currentcolor & 1 != 0;
    paint.border_pending = Some([Some((c, cc)); 4]);
    if paint.border_style.is_some() {
        return Some(border_create_sides(paint, crate::paint::CC_SIDES, false));
    }
    paint.border
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): `side_color_of`'s side with no entry: the colour is pended for that side (its
/// currentcolor answer in the entry, the four live bits put back as they were), not dropped. `transparent` pends its RGB
/// with that side's alpha 0 (SR1112: AETHER-CSS-BORDER-PENDING-TRANSPARENT); an invalid value is recorded as before.
fn side_color_pend<H: Host>(side: usize, value: &str, paint: &mut PaintStyle<H>) {
    let before = paint.currentcolor;
    let Some(c) = border_color_of::<H>(paint, value) else {
        if !is_neutral_keyword(value) {
            H::record_css(&format!("border-side-color-value:{}", clip(value)));
        }
        return;
    };
    let cc = paint.currentcolor & (1 << side) != 0;
    paint.currentcolor = before;
    side_alpha_set(paint, 1 << side, color_alpha::<H>(value));
    let mut p = paint.border_pending.unwrap_or([None; 4]);
    p[side] = Some((c, cc));
    paint.border_pending = Some(p);
}

/// AETHERCSSBORDERCOLORSTORED (SR1058): `merge_paint`'s token, BEFORE the whole-array `border` copy: a later block's
/// pending colours recolour `dst`'s earlier sides (width kept, the side's currentcolor bit from the entry), so
/// `.a { border: 3px solid blue } .a:hover { border-color: red }` paints 3px red and `.a:hover { border-top-color: red }`
/// reaches the top; when `dst` has no border at all but a declared style, the pending sides are created at 1px (the
/// in-block `border_color_pend` rule across rules). The entries then carry later-wins per side; a later block's own
/// `border` replaces whatever was recoloured (the copy that follows), so chained merges agree.
fn merge_border_pending<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    let Some(p) = src.border_pending else { return };
    let create = dst.border.is_none() && dst.border_style.is_some() && p.iter().any(|e| e.is_some());
    if create {
        dst.border = Some([None; 4]);
    }
    if let Some(sides) = dst.border.as_mut() {
        for i in 0..4 {
            let Some((c, cc)) = p[i] else { continue };
            match sides[i] {
                Some((w, _)) => sides[i] = Some((w, c)),
                None if create => sides[i] = Some((side_width(dst.border_width, i), c)),
                None => continue,
            }
            let bit = 1u8 << i;
            if cc { dst.currentcolor |= bit } else { dst.currentcolor &= !bit }
        }
    }
    let mut dp = dst.border_pending.unwrap_or([None; 4]);
    for i in 0..4 {
        if p[i].is_some() {
            dp[i] = p[i];
        }
    }
    dst.border_pending = Some(dp);
}

// ---- AETHERCSSBORDERSIDESTYLEWIDTH (LEDGER SR1069): a style alone creates medium sides; border-width stores, never fabricates (named tail block) ----

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): css-backgrounds-3 §4.3 `medium`, read from the ONE literal (the SR832
/// `noninherited_initial` row `"border-width" => "3px"`): the width of a side its style creates with no declared width,
/// and of a `border` / `border-<side>` shorthand without a width part.
fn border_medium() -> f32 {
    parse_px(noninherited_initial("border-width", "initial"), &ParseCx::default()).unwrap_or(0.0)
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): the width side `i` is created at: the stored `border_width` entry, else medium.
fn side_width(widths: Option<[Option<f32>; 4]>, i: usize) -> f32 {
    widths.and_then(|w| w[i]).unwrap_or_else(border_medium)
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): `PaintStyle::border_width` is the ONE per-side width store: every width
/// writer (`border-width`, `border-<side>-width`, and the shorthands' width, None for `none`) stores the sides in `mask`,
/// and a Some also moves an existing side's width (so the layout box, which reads `border`, agrees with the paint). No
/// side is created: a width alone paints nothing (style none; AETHER-CSS-BORDER-WIDTH-STORED, closed here).
fn border_width_store<H: Host>(paint: &mut PaintStyle<H>, mask: u8, w: Option<f32>) {
    let mut ws = paint.border_width.unwrap_or([None; 4]);
    for i in 0..4 {
        if mask & (1 << i) == 0 {
            continue;
        }
        ws[i] = w;
        if let (Some(w), Some(b)) = (w, paint.border.as_mut()) {
            if let Some((sw, _)) = b[i].as_mut() {
                *sw = w;
            }
        }
    }
    paint.border_width = Some(ws);
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): `merge_paint`'s token AFTER the copy: `dst`'s sides take `dst`'s stored widths,
/// so an earlier rule's `border-width: 2px` sizes a later rule's style-created side, and a later width moves earlier sides.
fn border_width_apply<H: Host>(dst: &mut PaintStyle<H>) {
    let Some(ws) = dst.border_width else { return };
    if let Some(b) = dst.border.as_mut() {
        for i in 0..4 {
            if let (Some(w), Some((sw, _))) = (ws[i], b[i].as_mut()) {
                *sw = w;
            }
        }
    }
}

/// AETHERCSSBORDERSIDESTYLEWIDTH (SR1069): `fold_into`'s road when the cascade made no side: a stored width reaches only a
/// taffy edge the UA layer already gave a width (a control's `ua_control` edges: its UA style is not none), so
/// `input { border-width: 0 }` still drops the 2px edge; a 0 edge (style none) stays 0.
fn border_width_fold(widths: Option<[Option<f32>; 4]>, border: &mut Rect<LengthPercentage>) {
    let Some(ws) = widths else { return };
    let zero = LengthPercentage::length(0.0);
    for (i, edge) in [&mut border.top, &mut border.right, &mut border.bottom, &mut border.left].into_iter().enumerate() {
        if let Some(w) = ws[i] {
            if *edge != zero {
                *edge = LengthPercentage::length(w);
            }
        }
    }
}

// ---- AETHERCSSALIGNCONTENTSAFE (LEDGER SR1064): safe / unsafe / first|last baseline on the alignment arms (named tail block) ----

/// AETHERCSSALIGNCONTENTSAFE (SR1064): css-align-3 §4.4 / §6 — `<overflow-position>? <content-position>` and
/// `[ first | last ]? baseline`. Answers (the bare position word, safe, baseline: 0 none / 1 first / 2 last). One token
/// passes through unchanged (bare `safe` reaches the arm and is refused as before); `first|last baseline` is
/// `baseline`; `safe|unsafe` before a content-position word strips to that word; anything else (`safe safe center`,
/// `baseline first`, `safe baseline`, `safe space-between`, three tokens) is None.
pub fn alignment_words(value: &str) -> Option<(&str, bool, u8)> {
    let mut it = value.split_ascii_whitespace();
    let (a, b) = (it.next()?, it.next());
    if it.next().is_some() {
        return None;
    }
    let Some(b) = b else { return Some((a, false, 0)) };
    match (a, b) {
        ("first", "baseline") => Some(("baseline", false, 1)),
        ("last", "baseline") => Some(("baseline", false, 2)),
        ("safe" | "unsafe", "center" | "start" | "end" | "flex-start" | "flex-end" | "self-start" | "self-end" | "left" | "right") => {
            Some((b, a == "safe", 0))
        }
        _ => None,
    }
}

/// AETHERCSSALIGNCONTENTSAFE (SR1064): the word the justify-content / align-content / align-items arms match. `safe`
/// is recorded (`align-overflow:safe`: taffy has no overflow-position, the start fallback on overflow is owed) and
/// otherwise ignored; `unsafe` is a no-op; `last baseline` is first-baseline + recorded (`align-baseline:last`). An
/// invalid form comes back whole, so the arm's own refusal records the full text.
fn align_word<H: Host>(value: &str) -> &str {
    match alignment_words(value) {
        Some((w, safe, base)) => {
            if safe {
                H::record_css("align-overflow:safe");
            }
            if base == 2 {
                H::record_css("align-baseline:last");
            }
            w
        }
        None => value,
    }
}

// ---- AETHERCSSBORDERPENDINGTRANSPARENT (LEDGER SR1112): a transparent border colour keeps the side at alpha 0 (named tail block) ----

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): a border colour's RGB: what [`parse_color_str`] reads, else a transparent
/// colour (`border_transparent`: `transparent`, any zero-alpha colour) as its own RGB (black for the keyword). A transparent
/// colour is a COLOUR, not an eraser: the side is kept (its width sizes the box) and paints nothing at alpha 0.
fn border_rgb<H: Host>(v: &str) -> Option<(u8, u8, u8)> {
    parse_color_str::<H>(v).or_else(|| border_transparent::<H>(v).then(|| H::parse_color_alpha(v).map_or((0, 0, 0), |p| p.0)))
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `PaintStyle::border_alpha` per side: the sides in `mask` take alpha `a`,
/// the others keep theirs (None = not written here = opaque).
fn side_alpha_set<H: Host>(paint: &mut PaintStyle<H>, mask: u8, a: f32) {
    let mut al = paint.border_alpha.unwrap_or([None; 4]);
    for (i, slot) in al.iter_mut().enumerate() {
        if mask & (1 << i) != 0 {
            *slot = Some(a);
        }
    }
    paint.border_alpha = Some(al);
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): `merge_paint`'s token in place of the wholesale copy: a later block's side
/// alphas win per side where it wrote them (as `merge_border_pending`'s entries), so `.a:hover { border-top-color: red }`
/// over `.a { border: 2px solid transparent }` makes the top opaque and leaves the other three at alpha 0.
fn merge_border_alpha<H: Host>(dst: &mut PaintStyle<H>, src: &PaintStyle<H>) {
    let Some(sa) = src.border_alpha else { return };
    let mut da = dst.border_alpha.unwrap_or([None; 4]);
    for i in 0..4 {
        if sa[i].is_some() {
            da[i] = sa[i];
        }
    }
    dst.border_alpha = Some(da);
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): side `i`'s border alpha (1 when unwritten).
pub fn border_side_alpha<H: Host>(paint: &PaintStyle<H>, i: usize) -> f32 {
    paint.border_alpha.and_then(|a| a[i]).unwrap_or(1.0)
}

/// AETHERCSSBORDERPENDINGTRANSPARENT (SR1112): the render's ONE per-side alpha read: a side at alpha 0 is dropped from the
/// paint (its width already sized the box at layout), and the answer is the stroke alpha of the sides left, the largest
/// of theirs (exact when they agree; two different non-zero side alphas: AETHER-CSS-BORDER-SIDE-ALPHA-MIXED owed).
pub fn border_visible<H: Host, T>(paint: &PaintStyle<H>, sides: &mut [Option<T>; 4]) -> f32 {
    let mut a = 0.0f32;
    for (i, side) in sides.iter_mut().enumerate() {
        let ai = border_side_alpha(paint, i);
        if ai <= 0.0 {
            *side = None;
        } else if side.is_some() {
            a = a.max(ai);
        }
    }
    a
}

// ---- AETHERCSSALIGNSELFARM (LEDGER SR1068): css-align-3 §6.2 align-self through align_word (named tail block) ----

/// AETHERCSSALIGNSELFARM (SR1068): css-align-3 §6.2 `align-self: auto | normal | stretch | <baseline-position> |
/// <overflow-position>? <self-position>` on a flex item, the align-items keyword map over the word [`align_word`] gives
/// (safe / unsafe / first|last baseline as there). `auto` (and `initial` / `unset` through [`noninherited_initial`]) is
/// taffy's None, marked `align_self_auto` so [`merge_specified`] carries it over an earlier rule's value; `inherit` is
/// dropped (as align-items); anything else is refused and recorded (`align-self-value:`). A floated box keeps the
/// float arm's START / END (Aether's float approximation; Chromium ignores align-self on a float in block flow).
fn apply_align_self<H: Host>(word: &str, style: &mut SpecifiedStyle<H>) {
    use taffy::style::AlignSelf as A;
    let v = match word {
        "auto" => None,
        "center" => Some(A::CENTER),
        "flex-end" | "end" => Some(A::END),
        "flex-start" | "start" => Some(A::START),
        "baseline" => Some(A::BASELINE),
        "stretch" | "normal" => Some(A::STRETCH),
        "inherit" => return,
        other => {
            H::record_css(&format!("align-self-value:{}", other));
            return;
        }
    };
    if matches!(style.paint.float, Some(1 | 2)) {
        return;
    }
    style.align_self = v;
    style.align_self_auto = v.is_none();
}

// ---- AETHERCSSOMOUTLINECOLUMNS (LEDGER SR1124): the outline-style codes back to their keywords (named tail block) ----

/// AETHERCSSOMOUTLINECOLUMNS (SR1124): the inverse of paint.rs' [`crate::paint::outline_style_code`] (the codes
/// `OUTLINE_NONE` 0 / `OUTLINE_AUTO` 1 / `OUTLINE_SOLID` 2 / `OUTLINE_DASHED` 3 / `OUTLINE_DOTTED` 4 / `OUTLINE_DOUBLE` 5).
pub fn outline_style_word(code: u8) -> &'static str {
    ["none", "auto", "solid", "dashed", "dotted", "double"].get(code as usize).copied().unwrap_or("none")
}

// ---- AETHERCSSWIDECOLUMNGAPNORMALFLEX (LEDGER SR1077): `normal` on the flex / grid gaps writes an explicit zero (named tail block) ----

/// A flex / grid gap value (`gap`, `row-gap`, `column-gap`): `normal` is the gap's initial 0 (css-align-3 §8.1),
/// written as an explicit zero so a later `normal` beats an earlier length in one block and across rules (merge
/// copies Somes); anything else is `parse_length_percentage_str`. The multicol gutter's `normal` (1em) stays
/// `PaintStyle::col_gap`'s Some(None), written by the column-gap arm.
fn gap_length(value: &str, cx: &ParseCx) -> Option<LengthPercentage> {
    if value.trim() == "normal" {
        return Some(LengthPercentage::length(0.0));
    }
    parse_length_percentage_str(value, cx)
}
