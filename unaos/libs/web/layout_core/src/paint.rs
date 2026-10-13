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
    /// AETHERSTYLE2 (SR95): `transform` (css-transforms-1 §5) as its function list; empty = `none`.
    pub transform: Option<Vec<TransformOp>>,
    /// The individual transform properties (css-transforms-2 §5.1): `translate`, `rotate`, `scale`
    /// (`none` = an identity matrix).
    pub tf_translate: Option<TransformOp>,
    pub tf_rotate: Option<TransformOp>,
    pub tf_scale: Option<TransformOp>,
    /// `transform-origin` (css-transforms-1 §6) as [x, y] length-percentages of the border box.
    pub transform_origin: Option<[LenPct; 2]>,
    /// `clip-path` basic shape (css-masking-1 §5.1); Some(None) = `none`.
    pub clip_path: Option<Option<ClipShape>>,
    /// The px insets [top, right, bottom, left] as specified (`top`/`right`/`bottom`/`left`/`inset`;
    /// a percentage or `auto` side is None): a sticky box's constraint rectangle (css-position-3 §3.4),
    /// read by the painter because a sticky box's insets are not folded into taffy.
    pub inset_px: Option<[Option<f32>; 4]>,
    /// AETHERSTYLE3 (SR103): `filter` (Filter Effects 1 §5) as written, validated by svg_core's parser;
    /// Some(None) = `none`. Painted as a group through [`crate::effects::filter_rgba`].
    pub filter: Option<Option<String>>,
    /// `backdrop-filter` (Filter Effects 2 §3) as written; Some(None) = `none`.
    pub backdrop_filter: Option<Option<String>>,
    /// AETHERSTYLE3 (SR103): `overflow-x` / `overflow-y` (css-overflow-3 §3) as [x, y] codes —
    /// [`crate::scroll::VISIBLE`] 0, `HIDDEN` 1, `CLIP` 2, `SCROLL` 3, `AUTO` 4. `clip` above stays the
    /// paint-time "clips its descendants" flag every non-visible value sets.
    pub overflow: Option<[u8; 2]>,
    /// AETHERSTYLE3 (SR103): css-multicol-1 — `column-count` / `column-width` (Some(None) = `auto`), the
    /// multicol gutter in px (`column-gap`; None = `normal`, 1em), `column-fill` (0 balance, 1 auto) and the
    /// `column-rule` longhands (style [`crate::multicol::RULE_NONE`] = none / hidden).
    pub col_count: Option<Option<u32>>,
    pub col_width: Option<Option<f32>>,
    pub col_gap: Option<f32>,
    pub col_fill: Option<u8>,
    pub col_rule_width: Option<f32>,
    pub col_rule_style: Option<u8>,
    pub col_rule_color: Option<(u8, u8, u8)>,
    /// AETHERSTYLE3 (SR103): `writing-mode` (css-writing-modes-4 §3.1; [`crate::writing`] codes), inherited.
    pub writing_mode: Option<u8>,
    /// `text-orientation` (§5.1): 0 mixed, 1 upright, 2 sideways. Glyphs paint sideways in every vertical
    /// mode (the `mixed` rendering of Latin); upright CJK is not rotated back.
    pub text_orientation: Option<u8>,
    /// AETHERSTYLE3 (SR103): `container-type` (css-contain-3 §3.1): 0 normal, 1 size, 2 inline-size;
    /// `container-name` (Some(empty) = none); `contain`'s size containment (0 none, 1 size, 2 inline-size).
    pub container_type: Option<u8>,
    pub container_name: Option<Vec<String>>,
    pub contain_size: Option<u8>,
    /// AETHERSTYLE3B (SR115): `scrollbar-gutter` (css-overflow-3 §6.1): 0 auto (overlay bars), 1 stable, 2
    /// stable both-edges — a scroll container reserves [`crate::scroll::SCROLLBAR`] px of layout space.
    pub scrollbar_gutter: Option<u8>,
    /// `scroll-behavior` (CSSOM View §5.1): true = smooth (programmatic scrolls animate).
    pub scroll_smooth: Option<bool>,
    /// `scroll-snap-type` (css-scroll-snap-1 §5.1): (axes: 0 none, 1 x, 2 y, 3 both; 1 = mandatory, 0 =
    /// proximity).
    pub snap_type: Option<(u8, u8)>,
    /// `scroll-snap-align` (§6.1) as [x, y]: 0 none, 1 start, 2 end, 3 center.
    pub snap_align: Option<[u8; 2]>,
    /// `scroll-snap-stop: always` (§6.2).
    pub snap_stop: Option<bool>,
    /// `scroll-padding` (§5.2) in px: [top, right, bottom, left] (the snapport's insets).
    pub scroll_padding: Option<[f32; 4]>,
    /// AETHERSTYLE3B (SR115): `column-span` (css-multicol-1 §6.1): true = `all`.
    pub col_span: Option<bool>,
    /// AETHERSTYLE3C (SR122): `box-decoration-break` (css-break-3 §5.4): true = `clone`, false = `slice`.
    pub deco_clone: Option<bool>,
    /// AETHERSTYLE3C (SR122): `text-combine-upright` (css-writing-modes-4 §9.1), inherited: true = `all`.
    pub text_combine: Option<bool>,
    /// AETHERSTYLE3C (SR122): `scroll-margin` (css-scroll-snap-1 §6.3) in px: [top, right, bottom, left] — the
    /// snap area's outsets from the border box.
    pub scroll_margin: Option<[f32; 4]>,
    /// AETHERSTYLE3C (SR122): `container-type` names `scroll-state` (css-conditional-5 §2.1): the box answers
    /// `@container scroll-state()` queries ([`crate::scroll::scroll_state`]).
    pub container_scroll: Option<bool>,
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
            transform: None,
            tf_translate: None,
            tf_rotate: None,
            tf_scale: None,
            transform_origin: None,
            clip_path: None,
            inset_px: None,
            filter: None,
            backdrop_filter: None,
            overflow: None,
            col_count: None,
            col_width: None,
            col_gap: None,
            col_fill: None,
            col_rule_width: None,
            col_rule_style: None,
            col_rule_color: None,
            writing_mode: None,
            text_orientation: None,
            container_type: None,
            container_name: None,
            contain_size: None,
            scrollbar_gutter: None,
            scroll_smooth: None,
            snap_type: None,
            snap_align: None,
            snap_stop: None,
            scroll_padding: None,
            col_span: None,
            deco_clone: None,
            text_combine: None,
            scroll_margin: None,
            container_scroll: None,
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


// ---- AETHERSTYLE2 (SR95): transforms and basic shapes ------------------------------------------------------

/// A length-percentage resolved at paint time: `[px, fraction]` = px + fraction x the reference length.
pub type LenPct = [f32; 2];

/// Resolves a [`LenPct`] against `basis`.
#[inline]
pub fn resolve_lp(v: LenPct, basis: f32) -> f32 {
    v[0] + v[1] * basis
}

/// One transform function (css-transforms-1 §7), flattened to 2D (css-transforms-2 §5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformOp {
    /// `translate()` and its axis forms: x and y as length-percentages of the border box.
    Translate(LenPct, LenPct),
    /// Any other function as the affine `[a b c d e f]` of CSS `matrix()`: (x, y) -> (ax + cy + e, bx + dy + f).
    Matrix([f32; 6]),
}

/// The identity affine.
pub const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `p · q` (q applies first).
pub fn mat_mul(p: [f32; 6], q: [f32; 6]) -> [f32; 6] {
    [
        p[0] * q[0] + p[2] * q[1],
        p[1] * q[0] + p[3] * q[1],
        p[0] * q[2] + p[2] * q[3],
        p[1] * q[2] + p[3] * q[3],
        p[0] * q[4] + p[2] * q[5] + p[4],
        p[1] * q[4] + p[3] * q[5] + p[5],
    ]
}

/// The inverse affine; `None` when singular (a `scale(0)` box paints nothing).
pub fn mat_invert(m: [f32; 6]) -> Option<[f32; 6]> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 || det.is_nan() {
        return None;
    }
    Some([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

/// Applies an affine to a point.
#[inline]
pub fn mat_apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// (sin, cos) of an angle in radians: reduced to [-pi/2, pi/2] and summed as a Taylor series to x^13
/// (error below 1e-7 there) — a `no_std` core has no libm.
pub fn sin_cos(x: f32) -> (f32, f32) {
    use core::f64::consts::{FRAC_PI_2, PI, TAU};
    let x = x as f64;
    // beyond ~1e7 rad an f32 angle has no fractional turn left to resolve
    if !x.is_finite() || x.abs() > 1e7 {
        return (0.0, 1.0);
    }
    let q = x / TAU;
    let qi = if q >= 0.0 { (q + 0.5) as i64 } else { (q - 0.5) as i64 } as f64;
    let mut r = x - TAU * qi;
    while r > PI {
        r -= TAU;
    }
    while r < -PI {
        r += TAU;
    }
    // sin(pi - r) = sin r, cos(pi - r) = -cos r
    let (r, flip) = if r > FRAC_PI_2 {
        (PI - r, true)
    } else if r < -FRAC_PI_2 {
        (-PI - r, true)
    } else {
        (r, false)
    };
    let r2 = r * r;
    let mut s = 0.0f64;
    let mut c = 0.0f64;
    let mut ts = r;
    let mut tc = 1.0f64;
    for k in 0..7 {
        s += ts;
        c += tc;
        let n = (2 * k + 2) as f64;
        ts *= -r2 / (n * (n + 1.0));
        tc *= -r2 / ((n - 1.0) * n);
    }
    (s as f32, if flip { -c as f32 } else { c as f32 })
}

/// sqrt by Newton's iteration (no libm); 0 for x <= 0.
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 || x.is_nan() {
        return 0.0;
    }
    let x = x as f64;
    let mut g = if x > 1.0 { x / 2.0 } else { 1.0 };
    for _ in 0..40 {
        let n = 0.5 * (g + x / g);
        if (n - g).abs() <= 1e-12 * n {
            g = n;
            break;
        }
        g = n;
    }
    g as f32
}

impl TransformOp {
    /// The function's affine for a `w` x `h` border box.
    pub fn matrix(&self, w: f32, h: f32) -> [f32; 6] {
        match *self {
            TransformOp::Translate(x, y) => [1.0, 0.0, 0.0, 1.0, resolve_lp(x, w), resolve_lp(y, h)],
            TransformOp::Matrix(m) => m,
        }
    }
}

/// A `circle()`/`ellipse()` radius (css-shapes-1 §3.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeRadius {
    Len(LenPct),
    ClosestSide,
    FarthestSide,
}

/// A `clip-path` basic shape against the border box (css-shapes-1 §3.1).
#[derive(Clone, Debug, PartialEq)]
pub enum ClipShape {
    /// `inset(t r b l [round r])`: insets (fractions of the box height / width) and one corner radius.
    Inset([LenPct; 4], LenPct),
    /// `circle(r at x y)`: a percentage radius is of sqrt((w^2 + h^2) / 2).
    Circle(ShapeRadius, [LenPct; 2]),
    /// `ellipse(rx ry at x y)`.
    Ellipse(ShapeRadius, ShapeRadius, [LenPct; 2]),
    /// `polygon(x y, ...)`, the nonzero fill rule (`evenodd` when the flag is set).
    Polygon(Vec<[LenPct; 2]>, bool),
}

impl ClipShape {
    /// Whether the box-local point (x, y) of a `w` x `h` border box is inside the shape.
    pub fn contains(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        match self {
            ClipShape::Inset(ins, round) => {
                let (t, r, b, l) = (resolve_lp(ins[0], h), resolve_lp(ins[1], w), resolve_lp(ins[2], h), resolve_lp(ins[3], w));
                let (x0, y0, x1, y1) = (l, t, w - r, h - b);
                if x < x0 || x >= x1 || y < y0 || y >= y1 {
                    return false;
                }
                let rad = resolve_lp(*round, w).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
                if rad <= 0.0 {
                    return true;
                }
                let cx = if x < x0 + rad { x0 + rad } else if x > x1 - rad { x1 - rad } else { return true };
                let cy = if y < y0 + rad { y0 + rad } else if y > y1 - rad { y1 - rad } else { return true };
                let (dx, dy) = (x - cx, y - cy);
                dx * dx + dy * dy <= rad * rad
            }
            ClipShape::Circle(r, c) => {
                let (cx, cy) = (resolve_lp(c[0], w), resolve_lp(c[1], h));
                let rad = match r {
                    ShapeRadius::Len(v) => v[0] + v[1] * sqrt((w * w + h * h) / 2.0),
                    ShapeRadius::ClosestSide => cx.min(w - cx).min(cy).min(h - cy).max(0.0),
                    ShapeRadius::FarthestSide => cx.max(w - cx).max(cy).max(h - cy),
                };
                let (dx, dy) = (x - cx, y - cy);
                dx * dx + dy * dy <= rad * rad
            }
            ClipShape::Ellipse(rx, ry, c) => {
                let (cx, cy) = (resolve_lp(c[0], w), resolve_lp(c[1], h));
                let axis = |r: &ShapeRadius, c: f32, len: f32| -> f32 {
                    match r {
                        ShapeRadius::Len(v) => resolve_lp(*v, len),
                        ShapeRadius::ClosestSide => c.min(len - c).max(0.0),
                        ShapeRadius::FarthestSide => c.max(len - c),
                    }
                };
                let (ax, ay) = (axis(rx, cx, w), axis(ry, cy, h));
                if ax <= 0.0 || ay <= 0.0 {
                    return false;
                }
                let (dx, dy) = ((x - cx) / ax, (y - cy) / ay);
                dx * dx + dy * dy <= 1.0
            }
            ClipShape::Polygon(pts, evenodd) => {
                if pts.len() < 3 {
                    return false;
                }
                let mut winding = 0i32;
                let mut crossings = 0u32;
                for i in 0..pts.len() {
                    let (ax, ay) = (resolve_lp(pts[i][0], w), resolve_lp(pts[i][1], h));
                    let j = (i + 1) % pts.len();
                    let (bx, by) = (resolve_lp(pts[j][0], w), resolve_lp(pts[j][1], h));
                    if (ay <= y) != (by <= y) {
                        let t = (y - ay) / (by - ay);
                        if x < ax + t * (bx - ax) {
                            crossings += 1;
                            winding += if by > ay { 1 } else { -1 };
                        }
                    }
                }
                if *evenodd { crossings % 2 == 1 } else { winding != 0 }
            }
        }
    }
}

impl<H: Host> PaintStyle<H> {
    /// The box's transformation matrix in border-box-local px (css-transforms-2 §6: translate, rotate,
    /// scale, then `transform`, about the transform-origin); `None` = no transform (or one that is the
    /// identity).
    pub fn transform_matrix(&self, w: f32, h: f32) -> Option<[f32; 6]> {
        let mut m = IDENTITY;
        for op in [self.tf_translate, self.tf_rotate, self.tf_scale].into_iter().flatten() {
            m = mat_mul(m, op.matrix(w, h));
        }
        for op in self.transform.iter().flatten() {
            m = mat_mul(m, op.matrix(w, h));
        }
        if m == IDENTITY {
            return None;
        }
        let o = self.transform_origin.unwrap_or([[0.0, 0.5], [0.0, 0.5]]);
        let (ox, oy) = (resolve_lp(o[0], w), resolve_lp(o[1], h));
        Some(mat_mul(mat_mul([1.0, 0.0, 0.0, 1.0, ox, oy], m), [1.0, 0.0, 0.0, 1.0, -ox, -oy]))
    }
}
