//! The paint half of a box's style: what the cascade specified for paint (and the few layout facts the
//! builder's tag defaults and the cascade hand to the later passes), keyed by box in `LayoutTree::paint_map`.
//! Moved verbatim from handlers/aether/src/layout/mod.rs; the gradient and shadow value types are the host's
//! ([`Host::Gradient`], [`Host::Shadow`]), so the host's renderer reads the same fields it always did.

use crate::prelude::*;
use crate::Host;

/// Specified paint properties for one box. `None` = not specified here;
/// color and font-size inherit down the tree at render time.
#[derive(Debug, Clone)]
pub struct PaintStyle<H: Host> {
    pub background: Option<(u8, u8, u8)>,
    pub color: Option<(u8, u8, u8)>,
    pub font_size: Option<f32>,
    /// font-weight (css-fonts-4 §2.2): absolute, or relative to the parent's.
    pub weight: Option<FontWeight>,
    /// Per-side borders [top, right, bottom, left]: (width px, color).
    /// None side = no stroke. Whole-Option None = unspecified.
    pub border: Option<[Option<(f32, (u8, u8, u8))>; 4]>,
    /// Resolved line height: multiplier of font size (px values are
    /// converted at parse time against the 16px base — approximation).
    pub line_height: Option<f32>,
    /// background-image url (as written; resolved via images::get at paint).
    pub bg_image: Option<String>,
    /// visibility:hidden / opacity:0 — box keeps its space, paints nothing.
    pub hidden: Option<bool>,
    /// overflow != visible — descendants clip to this box's rect.
    pub clip: Option<bool>,
    /// text-decoration underline on/off (None = tag default).
    pub underline: Option<bool>,
    /// white-space:nowrap — text measures and draws on one line.
    pub nowrap: Option<bool>,
    /// font-family: an interned family-list id (`fonts::family_list`; 0 sans-serif, 1 serif,
    /// 2 monospace, 3 the standard font).
    pub family: Option<u16>,
    /// Font style: 0 = normal, 1 = italic.
    pub italic: Option<bool>,
    /// Text transform: 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize.
    pub text_transform: Option<u8>,
    /// border-top-width, border-right-width, border-bottom-width, border-left-width.
    /// Overrides border[side].0 if set.
    pub border_width: Option<[Option<f32>; 4]>,
    /// background-size: (width, height) in pixels or as fractional shorthand values.
    /// Empty string means default (cover), other values stored as-is for rendering.
    pub bg_size: Option<String>,
    /// background-position: stored as-is for rendering (e.g., "center", "50% 50%").
    pub bg_position: Option<String>,
    /// background-repeat: 0 = repeat, 1 = no-repeat, 2 = repeat-x, 3 = repeat-y.
    pub bg_repeat: Option<u8>,
    /// The image-replacement idiom (text-indent:-9999px): the fallback TEXT
    /// is hidden, but the box and its background image still paint.
    pub text_hidden: Option<bool>,
    /// mask-image url — an alpha stencil for this box's background paint.
    pub mask_image: Option<String>,
    /// mask-size / mask-position / mask-repeat, same grammar as their
    /// background-* counterparts.
    pub mask_size: Option<String>,
    pub mask_position: Option<String>,
    pub mask_repeat: Option<u8>,
    /// text-align: 0 = left/start, 1 = center, 2 = right/end. INHERITED —
    /// remeasure walks it down the box tree and turns it into the flex
    /// alignment of each descendant's own formatting context.
    pub text_align: Option<u8>,
    /// object-fit (replaced content: `<video>` frames and posters, see `media::object_fit_rect`):
    /// 0 fill, 1 contain, 2 cover, 3 none, 4 scale-down. None = the element's default
    /// (contain for video).
    pub object_fit: Option<u8>,
    /// object-position, as written.
    pub object_position: Option<String>,
    /// `display: flex` (true) versus a block-ish display (false). A real
    /// flex container's children are flex items: each establishes its own
    /// formatting context, so no margin collapses through them.
    pub flex_container: Option<bool>,
    /// Per-side border-style [top, right, bottom, left]: 0 solid, 1 dashed,
    /// 2 dotted, 3 double (groove/ridge/inset/outset paint solid).
    pub border_style: Option<[u8; 4]>,
    /// border-radius per corner [top-left, top-right, bottom-right,
    /// bottom-left]: px when >= 0, a fraction of the box width when < 0
    /// (-0.5 = 50%).
    pub radius: Option<[f32; 4]>,
    /// white-space (css-text-3 §3): 0 normal, 1 nowrap, 2 pre, 3 pre-wrap,
    /// 4 pre-line, 5 break-spaces. Inherited.
    pub white_space: Option<u8>,
    /// word-break: 0 normal, 1 break-all, 2 keep-all. Inherited.
    pub word_break: Option<u8>,
    /// overflow-wrap: 0 normal, 1 break-word/anywhere. Inherited.
    pub overflow_wrap: Option<u8>,
    /// letter-spacing in px. Inherited.
    pub letter_spacing: Option<f32>,
    /// text-decoration: line-through.
    pub line_through: Option<bool>,
    /// On TEXT nodes (set by the inline whitespace pass): a collapsible
    /// space survives before / after this run.
    pub ws_lead: Option<bool>,
    pub ws_trail: Option<bool>,
    /// Author outer display: 0 inline, 1 block-level, 2 inline-level box.
    pub display_kind: Option<u8>,
    /// The UA vertical margin of this element in em of its own font size,
    /// and the px it was built with (against the 16px default): remeasure
    /// rescales an untouched UA margin to the element's real font size.
    pub ua_vmargin: Option<(f32, f32)>,
    /// [width, min-width, max-width] given as a math function mixing % and
    /// lengths (`min(300px, 80%)`), resolved after a first layout against
    /// the containing block's content width. A plain value clears it.
    pub pct_math: Option<[Option<String>; 3]>,
    /// border-collapse: collapse (true) / separate (false). On tables.
    pub border_collapse: Option<bool>,
    /// border-spacing in px (horizontal = vertical here). On tables.
    pub border_spacing: Option<f32>,
    /// The author gave this box a `width` (any value).
    pub has_width: Option<bool>,
    /// position: 0 static, 1 relative, 2 absolute, 3 fixed, 4 sticky.
    pub position_kind: Option<u8>,
    /// z-index: Some(None) = auto, Some(Some(z)) = an integer.
    pub z_index: Option<Option<i32>>,
    /// display: list-item (true) / any other display (false).
    pub list_item: Option<bool>,
    /// list-style-type: 0 none, 1 disc, 2 circle, 3 square, 4 decimal,
    /// 5 lower-alpha, 6 upper-alpha, 7 lower-roman, 8 upper-roman,
    /// 9 decimal-leading-zero. Inherited.
    pub list_style: Option<u8>,
    /// A linear-gradient background layer (Some(None) = `none` cleared it).
    pub bg_gradient: Option<Option<H::Gradient>>,
    /// Outer box shadows (empty = none).
    pub shadows: Option<Vec<H::Shadow>>,
    /// vertical-align (CSS 2.2 §10.8.1): (kind, value) — 0 baseline, 1 sub,
    /// 2 super, 3 middle, 4 text-top, 5 text-bottom, 6 top, 7 bottom,
    /// 8 a length in px (raise), 9 a percentage of line-height (fraction).
    pub vertical_align: Option<(u8, f32)>,
    /// float: 0 none, 1 left, 2 right.
    pub float: Option<u8>,
    /// opacity in [0, 1] (0 also sets `hidden`).
    pub opacity: Option<f32>,
    /// table-layout: fixed (true) / auto (false). On tables.
    pub table_fixed: Option<bool>,
    /// Alpha of the background colour (css-color-4; absent = opaque).
    pub bg_alpha: Option<f32>,
    /// Alpha of the border colour(s) (absent = opaque).
    pub border_alpha: Option<f32>,
    /// How `font_size` was specified: (0, f) = f x the parent's font-size
    /// (em, %, smaller/larger), (1, f) = f x the root's (rem), (2, _) =
    /// absolute (font_size as is).
    pub font_size_rel: Option<(u8, f32)>,
    /// The element's computed font-size in px, written by remeasure (the
    /// painter reads it instead of re-deriving the em chain).
    pub used_font_size: Option<f32>,
    /// Every length-valued declaration in cascade order (property, value):
    /// remeasure re-applies them against the element's own font when any is
    /// font-relative (em/rem/ex/ch, css-values-4 §6.1).
    pub font_rel: Option<Vec<(String, String)>>,
    /// font-stretch in percent (css-fonts-4 §2.3).
    pub stretch: Option<u16>,
    /// word-spacing in px (css-text-3 §7.1).
    pub word_spacing: Option<f32>,
    /// direction: rtl (true) / ltr (false) (css-writing-modes-4 §2.1; `dir` maps onto it).
    pub rtl: Option<bool>,
    /// text-shadow (css-text-decor-3 §4): none = Some(empty).
    pub text_shadows: Option<Vec<H::Shadow>>,
    /// AETHERGEN (SR71): the author-styled `::marker` of a list item (`generated::generate`).
    pub marker: Option<Rc<MarkerPaint>>,
    /// clear (CSS 2.2 §9.5.2): 0 none, 1 left, 2 right, 3 both. Acted on for a generated clearfix
    /// box at the end of an inline formatting context (`generated`, `clear_floats`).
    pub clear: Option<u8>,
}


/// Every field unspecified (written by hand: a derive would demand `Default` of the host's gradient and
/// shadow types).
impl<H: Host> Default for PaintStyle<H> {
    fn default() -> Self {
        PaintStyle {
            background: None,
            color: None,
            font_size: None,
            weight: None,
            border: None,
            line_height: None,
            bg_image: None,
            hidden: None,
            clip: None,
            underline: None,
            nowrap: None,
            family: None,
            italic: None,
            text_transform: None,
            border_width: None,
            bg_size: None,
            bg_position: None,
            bg_repeat: None,
            text_hidden: None,
            mask_image: None,
            mask_size: None,
            mask_position: None,
            mask_repeat: None,
            text_align: None,
            object_fit: None,
            object_position: None,
            flex_container: None,
            border_style: None,
            radius: None,
            white_space: None,
            word_break: None,
            overflow_wrap: None,
            letter_spacing: None,
            line_through: None,
            ws_lead: None,
            ws_trail: None,
            display_kind: None,
            ua_vmargin: None,
            pct_math: None,
            border_collapse: None,
            border_spacing: None,
            has_width: None,
            position_kind: None,
            z_index: None,
            list_item: None,
            list_style: None,
            bg_gradient: None,
            shadows: None,
            vertical_align: None,
            float: None,
            opacity: None,
            table_fixed: None,
            bg_alpha: None,
            border_alpha: None,
            font_size_rel: None,
            used_font_size: None,
            font_rel: None,
            stretch: None,
            word_spacing: None,
            rtl: None,
            text_shadows: None,
            marker: None,
            clear: None,
        }
    }
}

/// A `font-weight` value as specified (css-fonts-4 §2.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontWeight {
    Abs(u16),
    Bolder,
    Lighter,
}

impl FontWeight {
    /// The computed weight against the parent's (the relative-weight table of §2.2).
    pub fn resolve(self, parent: u16) -> u16 {
        match self {
            FontWeight::Abs(w) => w,
            FontWeight::Bolder => match parent {
                0..=349 => 400,
                350..=549 => 700,
                550..=899 => 900,
                w => w,
            },
            FontWeight::Lighter => match parent {
                0..=99 => parent,
                100..=549 => 100,
                550..=749 => 400,
                _ => 700,
            },
        }
    }
}

/// A list item's `::marker` as author-styled (css-lists-3 §3.1, css-pseudo-4 §3.2). `None` fields
/// fall back to the item's own (inherited) values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkerPaint {
    /// The marker's text from a `content` list; `None` = `content: normal` (the list-style marker).
    pub text: Option<String>,
    /// `content: none`: no marker.
    pub none: bool,
    pub color: Option<(u8, u8, u8)>,
    pub font_size: Option<f32>,
    /// As `PaintStyle::font_size_rel`: (0, f) = f x the item's font-size, (1, f) = f x the root's.
    pub font_size_rel: Option<(u8, f32)>,
    pub weight: Option<FontWeight>,
    pub italic: Option<bool>,
    pub family: Option<u16>,
}

impl MarkerPaint {
    /// The marker's font-size against the list item's.
    pub fn size(&self, item: f32) -> f32 {
        match self.font_size_rel {
            Some((0, f)) => f * item,
            Some((1, f)) => f * 16.0,
            _ => self.font_size.unwrap_or(item),
        }
    }
}

