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
}

impl Default for ParseCx {
    fn default() -> Self {
        ParseCx { viewport: (800.0, 600.0), font: (16.0, 16.0, 8.0, 8.0) }
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
    pub align_items: Option<taffy::style::AlignItems>,
    pub align_self: Option<taffy::style::AlignSelf>,
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
    pub aspect_ratio: Option<Option<f32>>,
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
    pub animatable: Vec<(String, String)>,
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
            node_style.align_items = self.align_items.unwrap_or(taffy::style::AlignItems::NORMAL);
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
        }
        if let Some(sides) = self.paint.border {
            let w = |i: usize| LengthPercentage::length(sides[i].map(|(w, _)| w).unwrap_or(0.0));
            node_style.border = Rect { top: w(0), right: w(1), bottom: w(2), left: w(3) };
        }
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
    if is_length_property(prop) {
        // Kept in cascade order for the font-relative re-resolution.
        let list = style.paint.font_rel.get_or_insert_with(Vec::new);
        list.retain(|(p, _)| p != prop);
        list.push((prop.to_string(), value.to_string()));
    }
    match prop {
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
            let parts: Vec<&str> = value.split_whitespace().collect();
            let row = parts.first().and_then(|v| parse_length_percentage_str(v, cx));
            let col = parts.get(1).and_then(|v| parse_length_percentage_str(v, cx)).or(row);
            match (row, col) {
                (Some(r), Some(c)) => {
                    style.row_gap = Some(r);
                    style.column_gap = Some(c);
                }
                _ => H::record_css(&format!("gap-value:{}", clip(value))),
            }
        }
        "row-gap" | "grid-row-gap" => match parse_length_percentage_str(value, cx) {
            Some(v) => style.row_gap = Some(v),
            None => H::record_css(&format!("gap-value:{}", clip(value))),
        },
        "column-gap" | "grid-column-gap" => {
            // AETHERSTYLE3 (SR103): the multicol gutter too (css-multicol-1 §4.1; `normal` = 1em there).
            if prop == "column-gap" {
                if let Some(px) = parse_px(value, cx).filter(|_| !value.ends_with('%')) {
                    style.paint.col_gap = Some(px);
                } else if value == "normal" {
                    style.paint.col_gap = None;
                }
            }
            apply_column_gap(value, style, cx);
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
            None => H::record_css(&format!("flex-basis-value:{}", clip(value))),
        },
        "flex" => match parse_flex_shorthand(value, cx) {
            Some((g, sh, b)) => {
                style.flex_grow = Some(g);
                style.flex_shrink = Some(sh);
                style.flex_basis = Some(b);
            }
            None => H::record_css(&format!("flex-value:{}", clip(value))),
        },
        "width" | "min-width" | "max-width" => {
            let d = parse_dimension_str(value, cx);
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
                style.paint.has_width = Some(!matches!(value, "auto"));
            }
            match i {
                0 => style.width = d,
                1 => style.min_width = d,
                _ => style.max_width = d,
            }
        }
        "height" => style.height = parse_dimension_str(value, cx),
        "max-height" => style.max_height = parse_dimension_str(value, cx),
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
        "border-spacing" => {
            if let Some(px) = split_top_level(value).first().and_then(|v| parse_px(v, cx)) {
                style.paint.border_spacing = Some(px.max(0.0));
            }
        }
        "min-height" => style.min_height = parse_dimension_str(value, cx),
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
            let parts: Vec<Option<f32>> = value.split_whitespace().map(px).collect();
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
        "padding" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_str(v, cx)) {
                style.padding = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length_percentage_str(value, cx) {
                style.padding[side_index(prop)] = Some(v);
            }
        }
        "margin" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_auto_str(v, cx)) {
                style.margin = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length_percentage_auto_str(value, cx) {
                style.margin[side_index(prop)] = Some(v);
            }
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
            let v = parse_length_percentage_auto_str(value, cx);
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
            None => {
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
        "container-type" => match value {
            "normal" => style.paint.container_type = Some(0),
            "size" => style.paint.container_type = Some(1),
            "inline-size" => style.paint.container_type = Some(2),
            v if is_neutral_keyword(v) => {}
            v => H::record_css(&format!("container-type-value:{}", clip(v))),
        },
        "container-name" => {
            let names: Vec<String> = value.split_whitespace().filter(|n| *n != "none").map(|n| n.to_string()).collect();
            style.paint.container_name = Some(names);
        }
        "container" => {
            let (names, ty) = match value.split_once('/') {
                Some((n, t)) => (n.trim(), Some(t.trim())),
                None => (value, None),
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
        "text-orientation" => match value {
            "mixed" => style.paint.text_orientation = Some(0),
            "upright" => {
                style.paint.text_orientation = Some(1);
                H::record_css("text-orientation-value:upright");
            }
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
            for part in value.split_whitespace() {
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
        // css-multicol-1 §6: a spanning element splits the column flow; Aether does not split it.
        "column-span" if value == "none" => {}
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
            };
        }
        // `clear` (CSS 2.2 §9.5.2) is consumed only by a generated clearfix box (`layout::generated`);
        // a real element's clearance is still unimplemented, so it stays ledgered.
        "clear" => {
            style.paint.clear = match value {
                "none" => Some(0),
                "left" | "inline-start" => Some(1),
                "right" | "inline-end" => Some(2),
                "both" => Some(3),
                _ => style.paint.clear,
            };
            H::record_css("property:clear");
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
            }
        }
        // text-align is INHERITED: it aligns the inline content of every
        // descendant block, not just this box's own flex children. The
        // paint field carries it down (see layout::remeasure); the justify
        // here is the immediate effect on this box's own row.
        "text-align" => match value {
            "center" | "-webkit-center" | "-moz-center" => {
                style.justify = Some(taffy::style::JustifyContent::CENTER);
                style.paint.text_align = Some(1);
            }
            // codes: 0 start, 1 center, 2 right, 3 end, 4 left — start/end resolve against `direction`
            // (css-text-3 §6.1) where lines are placed (layout::inline).
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
            "start" | "justify" => {
                style.justify = Some(taffy::style::JustifyContent::START);
                style.paint.text_align = Some(0);
            }
            // css-text-4 §6.1: match-parent is the parent's value — inheritance, which an unspecified
            // text-align already is (the parent's start/end against ITS direction is the nuance left).
            "inherit" | "initial" | "unset" | "revert" | "match-parent" | "-webkit-match-parent" => {}
            other => H::record_css(&format!("text-align:{}", other)),
        },
        "justify-content" => match value {
            "center" => style.justify = Some(taffy::style::JustifyContent::CENTER),
            "flex-end" | "end" => style.justify = Some(taffy::style::JustifyContent::END),
            "flex-start" | "start" | "normal" => style.justify = Some(taffy::style::JustifyContent::START),
            "space-between" => style.justify = Some(taffy::style::JustifyContent::SPACE_BETWEEN),
            "space-around" => style.justify = Some(taffy::style::JustifyContent::SPACE_AROUND),
            "space-evenly" => style.justify = Some(taffy::style::JustifyContent::SPACE_EVENLY),
            "inherit" | "initial" | "unset" => {}
            other => H::record_css(&format!("justify-content-value:{}", other)),
        },
        "background-color" => {
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
            style.paint.bg_gradient = H::parse_gradient(value).map(Some);
        }
        "background" if value.to_ascii_lowercase().contains("-gradient(") => {
            // A gradient layer (css-images-3 §3.1, §3.2) plus any colour outside it.
            style.paint.bg_gradient = H::parse_gradient(value).map(Some);
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
            Some(u) => style.paint.bg_image = Some(u),
            None => {
                if !is_neutral_keyword(value) {
                    H::record_css(&format!("background-image-value:{}", clip(value)));
                }
            }
        },
        "background" => {
            // Shorthand: a color and/or an image url, plus the repeat
            // keyword and a `position / size` pair when present.
            if let Some(u) = extract_css_url(value) {
                style.paint.bg_image = Some(u);
            }
            // Components outside url(...) — the url may itself hold slashes
            // and keywords, so scan only what is left after removing it.
            let outside = strip_css_urls(value);
            for part in outside.split_whitespace() {
                if let Some(r) = parse_repeat(part) {
                    style.paint.bg_repeat = Some(r);
                }
            }
            if let Some((pos, size)) = outside.split_once('/') {
                let pos: String = pos
                    .split_whitespace()
                    .filter(|t| is_position_token(t))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !pos.is_empty() {
                    style.paint.bg_position = Some(pos);
                }
                let size = size.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                if !size.is_empty() {
                    style.paint.bg_size = Some(size);
                }
            } else {
                let pos: String = outside
                    .split_whitespace()
                    .filter(|t| is_position_token(t))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !pos.is_empty() {
                    style.paint.bg_position = Some(pos);
                }
            }
            let mut got_color = false;
            for part in value.split_whitespace() {
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
        "color" => {
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
                return;
            }
            let mut rest = value;
            // Leading style / variant / weight / stretch keywords.
            loop {
                let Some((head, tail)) = rest.split_once(char::is_whitespace) else { break };
                let h = head.trim().to_ascii_lowercase();
                if h == "italic" || h == "oblique" {
                    style.paint.italic = Some(true);
                } else if h == "normal" || h == "small-caps" {
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
            // size[/line-height] then the family list.
            let (size_part, family) = match rest.find(char::is_whitespace) {
                Some(i) => (&rest[..i], rest[i..].trim()),
                None => (rest, ""),
            };
            let (size, line) = match size_part.split_once('/') {
                Some((s, l)) => (s, Some(l)),
                None => (size_part, None),
            };
            match parse_font_size(size, cx) {
                Some(px) => style.paint.font_size = Some(px),
                None => {
                    H::record_css(&format!("font-shorthand:{}", clip(value)));
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
        "align-items" | "align-content" => match value {
            "center" => style.align_items = Some(taffy::style::AlignItems::CENTER),
            "flex-end" | "end" => style.align_items = Some(taffy::style::AlignItems::END),
            "flex-start" | "start" => style.align_items = Some(taffy::style::AlignItems::START),
            "baseline" => style.align_items = Some(taffy::style::AlignItems::BASELINE),
            "stretch" | "normal" => style.align_items = Some(taffy::style::AlignItems::STRETCH),
            "inherit" | "initial" | "unset" => {}
            other => H::record_css(&format!("align-items-value:{}", other)),
        },
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
            // Stored as > 0 multiplier, < 0 = -(px), 0 = normal.
            let v = value.trim();
            let (em, ..) = cx.font;
            if matches!(v, "inherit" | "unset") {
                return; // inherited: the unspecified value
            }
            let lh = if matches!(v, "normal" | "initial" | "revert") {
                Some(0.0)
            } else if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(-(pct / 100.0 * em))
            } else if let Ok(n) = v.parse::<f32>() {
                Some(n.max(0.0))
            } else {
                parse_px(v, cx).map(|px| -px.max(0.0))
            };
            match lh {
                Some(f) => style.paint.line_height = Some(f),
                None => H::record_css(&format!("line-height-value:{}", clip(value))),
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
                sides[side] = None;
                style.paint.border = Some(sides);
                return;
            }
            let mut width = 1.0f32;
            let mut color = (128, 128, 128);
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
                } else if let Some(c) = parse_color_str::<H>(part) {
                    color = c;
                    got_any = true;
                    style.paint.border_alpha = Some(color_alpha::<H>(part));
                }
            }
            if got_any {
                sides[side] = Some((width, color));
                style.paint.border = Some(sides);
            }
        }
        "text-decoration" | "text-decoration-line" => {
            let v = value.trim();
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
        "white-space" | "white-space-collapse" => {
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
                style.paint.nowrap = Some(matches!(c, 1 | 2));
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
            }
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
                style.paint.bg_size = Some(value.to_string());
            }
        }
        "background-position" => {
            if !is_neutral_keyword(value) {
                style.paint.bg_position = Some(value.to_string());
            }
        }
        "background-repeat" => {
            if let Some(r) = parse_repeat(value) {
                style.paint.bg_repeat = Some(r);
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
                let mut widths = style.paint.border_width.unwrap_or([None; 4]);
                widths[side] = Some(w);
                style.paint.border_width = Some(widths);
            }
        }
        // border-radius: parsed but not rendered (taffy doesn't support it yet).
        // At minimum, stops the property from appearing in unsupported ledgers.
        // css-backgrounds-3 §5.1: 1-4 corner radii (the elliptical `/`
        // part takes its horizontal radii), or one corner's longhand.
        "border-radius" => {
            let horiz = value.split('/').next().unwrap_or(value);
            let parts: Option<Vec<f32>> = horiz.split_whitespace().map(|v| parse_radius(v, cx)).collect();
            if let Some(p) = parts {
                let r = match p.as_slice() {
                    [a] => [*a; 4],
                    [a, b] => [*a, *b, *a, *b],
                    [a, b, c] => [*a, *b, *c, *b],
                    [a, b, c, d] => [*a, *b, *c, *d],
                    _ => return,
                };
                style.paint.radius = Some(r);
            }
        }
        "border-top-left-radius" | "border-top-right-radius"
        | "border-bottom-right-radius" | "border-bottom-left-radius" => {
            if let Some(v) = value.split_whitespace().next().and_then(|v| parse_radius(v, cx)) {
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
        "border" | "outline" => {
            let v = value.trim();
            if v == "none" || v == "0" {
                style.paint.border = Some([None; 4]);
                return;
            }
            let mut width = 1.0f32;
            let mut color = (128, 128, 128);
            let mut got_any = false;
            for part in split_top_level(v) {
                if let Some(px) = parse_px(part, cx).filter(|_| part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '.')) {
                    width = px;
                    got_any = true;
                } else if let Some(k) = border_style_code(part) {
                    got_any = true;
                    style.paint.border_style = Some([k; 4]);
                } else if let Some(c) = parse_color_str::<H>(part) {
                    color = c;
                    got_any = true;
                    style.paint.border_alpha = Some(color_alpha::<H>(part));
                }
            }
            if got_any {
                style.paint.border = Some([Some((width, color)); 4]);
            } else {
                H::record_css(&format!("border-value:{}", clip(value)));
            }
        }
        "border-style" => {
            // A styleless border draws nothing, whatever its width says.
            if matches!(value.trim(), "none" | "hidden") {
                style.paint.border = Some([None; 4]);
            } else if let Some(r) = parse_sides(value, border_style_code) {
                style.paint.border_style = Some([r.top, r.right, r.bottom, r.left]);
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            let side = side_index(prop.trim_end_matches("-style"));
            if let Some(k) = border_style_code(value) {
                let mut bs = style.paint.border_style.unwrap_or([0; 4]);
                bs[side] = k;
                style.paint.border_style = Some(bs);
            } else if matches!(value, "none" | "hidden") {
                let mut sides = style.paint.border.unwrap_or_default();
                sides[side] = None;
                style.paint.border = Some(sides);
            }
        }
        "border-color" => {
            // `transparent` (or a fully transparent rgba) is how a page
            // says "keep the box, drop the stroke" — the UA control border
            // must go with it, or every quiet button paints an empty frame.
            let v = value.trim().to_ascii_lowercase();
            if v == "transparent" || (v.starts_with("rgba(") && parse_color_str::<H>(&v).is_none()) {
                style.paint.border = Some([None; 4]);
                return;
            }
            if let Some(c) = parse_color_str::<H>(value) {
                style.paint.border_alpha = Some(color_alpha::<H>(value));
                let mut sides = style.paint.border.unwrap_or([Some((1.0, (128, 128, 128))); 4]);
                for side in sides.iter_mut() {
                    let w = side.map(|(w, _)| w).unwrap_or(1.0);
                    *side = Some((w, c));
                }
                style.paint.border = Some(sides);
            }
        }
        "border-width" => {
            if let Some(w) = parse_px(value, cx) {
                let mut sides = style.paint.border.unwrap_or([Some((1.0, (128, 128, 128))); 4]);
                for side in sides.iter_mut() {
                    let c = side.map(|(_, c)| c).unwrap_or((128, 128, 128));
                    *side = Some((w, c));
                }
                style.paint.border = Some(sides);
            }
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
        other
            if other
                .strip_prefix("-webkit-")
                .or_else(|| other.strip_prefix("-moz-"))
                .or_else(|| other.strip_prefix("-ms-"))
                .or_else(|| other.strip_prefix("-o-"))
                .is_some_and(|base| {
                    matches!(
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
                            | "column-rule-style" | "column-rule-color" | "column-span"
                            | "writing-mode" | "text-orientation"
                    )
                }) =>
        {
            let base = other
                .trim_start_matches("-webkit-")
                .trim_start_matches("-moz-")
                .trim_start_matches("-ms-")
                .trim_start_matches("-o-");
            apply_declaration(base, value, style, cx);
        }
        other => {
            if !apply_logical(other, value, style, cx) {
                H::record_css(&format!("property:{}", other));
            }
        }
    }
}

/// The flex / grid `column-gap` (taffy's horizontal gap), as the arm always folded it.
fn apply_column_gap<H: Host>(value: &str, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    match parse_length_percentage_str(value, cx) {
        Some(v) => style.column_gap = Some(v),
        // `normal`: the flex/grid gap's initial 0 (css-align-3 §8.1) — nothing to fold.
        None if value == "normal" => {}
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
                Some(r) => len_pct(split_top_level(r.split('/').next().unwrap_or(r)).first()?, cx)?,
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
    copy!(
        background, color, font_size, weight, border, line_height, hidden, clip, underline,
        nowrap, family, italic, text_transform, border_width, bg_repeat, text_hidden,
        mask_repeat, text_align, object_fit, flex_container, border_style, radius,
        white_space, word_break, overflow_wrap, letter_spacing, line_through, display_kind,
        ua_vmargin, border_collapse, border_spacing, has_width, position_kind, z_index,
        list_item, list_style, vertical_align, float, opacity, font_size_rel, table_fixed, bg_alpha, border_alpha,
        stretch, word_spacing, rtl, clear, tf_translate, tf_rotate, tf_scale, transform_origin, inset_px,
        overflow, col_count, col_width, col_gap, col_fill, col_rule_width, col_rule_style, col_rule_color,
        writing_mode, text_orientation, container_type, contain_size,
        scrollbar_gutter, scroll_smooth, snap_type, snap_align, snap_stop, scroll_padding,
    );
    clone!(text_shadows, transform, clip_path, filter, backdrop_filter, container_name);
    if let Some(src_list) = &src.font_rel {
        let list = dst.font_rel.get_or_insert_with(Vec::new);
        for (p, v) in src_list {
            list.retain(|(q, _)| q != p);
            list.push((p.clone(), v.clone()));
        }
    }
    clone!(pct_math, bg_gradient, shadows);
    clone!(bg_image, bg_size, bg_position, mask_image, mask_size, mask_position, object_position);
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
    for i in 0..4 {
        if src.padding[i].is_some() { dst.padding[i] = src.padding[i]; }
        if src.margin[i].is_some() { dst.margin[i] = src.margin[i]; }
    }
    take!(display, flex_direction, width, height, position,
          inset_top, inset_left, inset_right, inset_bottom, justify, align_items, align_self,
          max_width, max_height, min_width, min_height, box_sizing,
          flex_container, flex_wrap, row_gap, column_gap, flex_grow, flex_shrink, flex_basis, aspect_ratio);
    merge_paint(&mut dst.paint, &src.paint);
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
    if lower.contains('(') {
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
            | "border-spacing" | "outline" | "text-indent"
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
    // Longest suffixes first: `dvh` must not be read as `vh` with a stray d.
    for (unit, basis) in [
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
fn border_style_code(v: &str) -> Option<u8> {
    match v.trim() {
        "solid" | "groove" | "ridge" | "inset" | "outset" => Some(0),
        "dashed" => Some(1),
        "dotted" => Some(2),
        "double" => Some(3),
        _ => None,
    }
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
    for part in value.split_whitespace() {
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

