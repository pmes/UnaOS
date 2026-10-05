use crate::layout::{LayoutTree, PaintStyle};
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};
use taffy::geometry::Rect;

/// taffy 0.14 types min-/max-size as length | percentage | auto (the
/// intrinsic keywords are size-only); a keyword there folds to auto.
pub(crate) fn lpa(d: Dimension) -> LengthPercentageAuto {
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
pub(crate) struct SpecifiedStyle {
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
    pub paint: PaintStyle,
}

impl SpecifiedStyle {
    /// Folds the specified layout declarations into a taffy style.
    pub(crate) fn fold_into(&self, node_style: &mut Style) {
        if let Some(d) = self.display { node_style.display = d; }
        if let Some(fd) = self.flex_direction { node_style.flex_direction = fd; }
        if self.flex_container == Some(true) {
            // CSS initial values for a flex container, replacing the
            // column/wrap/baseline defaults of Aether's block approximation.
            if self.flex_direction.is_none() {
                node_style.flex_direction = FlexDirection::Row;
            }
            node_style.flex_wrap = self.flex_wrap.unwrap_or(taffy::style::FlexWrap::NoWrap);
            node_style.align_items = self.align_items;
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
        if let Some(v) = self.inset_top { node_style.inset.top = v; }
        if let Some(v) = self.inset_left { node_style.inset.left = v; }
        if let Some(v) = self.inset_right { node_style.inset.right = v; }
        if let Some(v) = self.inset_bottom { node_style.inset.bottom = v; }
        if let Some(j) = self.justify {
            node_style.justify_content = Some(j);
        }
        // align-items only reaches ROW boxes. A column box here is the
        // block-container approximation, not a real column flex container:
        // an `align-items:center` written for a row would land on its CROSS
        // axis and centre the page's block content horizontally.
        if let Some(a) = self.align_items {
            if self.flex_container == Some(true)
                || matches!(node_style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse)
            {
                node_style.align_items = Some(a);
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
    }
}

/// One declaration, shared by the stylesheet cascade and inline styles.
/// Values arrive as raw strings so function values (rgb()...) parse uniformly.
pub(crate) fn apply_declaration(prop: &str, value: &str, style: &mut SpecifiedStyle) {
    let value = value.trim();
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
            other => crate::ledger::record_css(&format!("flex-wrap-value:{}", other)),
        },
        "flex-flow" => {
            for part in value.split_whitespace() {
                match part {
                    "row" | "column" | "row-reverse" | "column-reverse" => {
                        apply_declaration("flex-direction", part, style)
                    }
                    _ => apply_declaration("flex-wrap", part, style),
                }
            }
        }
        "gap" | "grid-gap" => {
            let parts: Vec<&str> = value.split_whitespace().collect();
            let row = parts.first().and_then(|v| parse_length_percentage_str(v));
            let col = parts.get(1).and_then(|v| parse_length_percentage_str(v)).or(row);
            match (row, col) {
                (Some(r), Some(c)) => {
                    style.row_gap = Some(r);
                    style.column_gap = Some(c);
                }
                _ => crate::ledger::record_css(&format!("gap-value:{}", clip(value))),
            }
        }
        "row-gap" | "grid-row-gap" => match parse_length_percentage_str(value) {
            Some(v) => style.row_gap = Some(v),
            None => crate::ledger::record_css(&format!("gap-value:{}", clip(value))),
        },
        "column-gap" | "grid-column-gap" => match parse_length_percentage_str(value) {
            Some(v) => style.column_gap = Some(v),
            None => crate::ledger::record_css(&format!("gap-value:{}", clip(value))),
        },
        "flex-grow" => match value.parse::<f32>() {
            Ok(v) if v >= 0.0 => style.flex_grow = Some(v),
            _ => crate::ledger::record_css(&format!("flex-grow-value:{}", clip(value))),
        },
        "flex-shrink" => match value.parse::<f32>() {
            Ok(v) if v >= 0.0 => style.flex_shrink = Some(v),
            _ => crate::ledger::record_css(&format!("flex-shrink-value:{}", clip(value))),
        },
        "flex-basis" => match parse_flex_basis(value) {
            Some(v) => style.flex_basis = Some(v),
            None => crate::ledger::record_css(&format!("flex-basis-value:{}", clip(value))),
        },
        "flex" => match parse_flex_shorthand(value) {
            Some((g, sh, b)) => {
                style.flex_grow = Some(g);
                style.flex_shrink = Some(sh);
                style.flex_basis = Some(b);
            }
            None => crate::ledger::record_css(&format!("flex-value:{}", clip(value))),
        },
        "width" | "min-width" | "max-width" => {
            let d = parse_dimension_str(value);
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
        "height" => style.height = parse_dimension_str(value),
        "max-height" => style.max_height = parse_dimension_str(value),
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
            if let Some(px) = split_top_level(value).first().and_then(|v| parse_px(v)) {
                style.paint.border_spacing = Some(px.max(0.0));
            }
        }
        "min-height" => style.min_height = parse_dimension_str(value),
        // overflow hidden/clip/auto/scroll all CLIP paint here (no inner
        // scrollbars yet — clipping is the honest approximation; visible
        // overflow was smearing collapsed menus over the page).
        "overflow" | "overflow-x" | "overflow-y" => match value {
            "hidden" | "clip" | "auto" | "scroll" => style.paint.clip = Some(true),
            "visible" => style.paint.clip = Some(false),
            _ => {}
        },
        "padding" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_str(v)) {
                style.padding = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length_percentage_str(value) {
                style.padding[side_index(prop)] = Some(v);
            }
        }
        "margin" => {
            if let Some(r) = parse_sides(value, |v| parse_length_percentage_auto_str(v)) {
                style.margin = [Some(r.top), Some(r.right), Some(r.bottom), Some(r.left)];
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length_percentage_auto_str(value) {
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
                other => crate::ledger::record_css(&format!("position:{}", other)),
            }
        }
        "z-index" => {
            if value == "auto" {
                style.paint.z_index = Some(None);
            } else if let Ok(z) = value.parse::<i32>() {
                style.paint.z_index = Some(Some(z));
            }
        }
        "top" => style.inset_top = parse_length_percentage_auto_str(value),
        "left" => style.inset_left = parse_length_percentage_auto_str(value),
        "right" => style.inset_right = parse_length_percentage_auto_str(value),
        "bottom" => style.inset_bottom = parse_length_percentage_auto_str(value),
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
                v => parse_px(v).map(|px| (8, px)),
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
            other => crate::ledger::record_css(&format!("float:{}", other)),
        },
        // The image-replacement idiom: a huge negative text-indent pushes
        // the fallback text off-box while the element's own background
        // image stays visible (Wikipedia's sprite wordmark is exactly
        // this). Hiding the whole box would drop that image — only the
        // TEXT goes away. Small indents are ignored (no first-line
        // indent support).
        "text-indent" => {
            if let Some(px) = parse_px(value) {
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
            "inherit" | "initial" | "unset" | "revert" => {}
            other => crate::ledger::record_css(&format!("text-align:{}", other)),
        },
        "justify-content" => match value {
            "center" => style.justify = Some(taffy::style::JustifyContent::CENTER),
            "flex-end" | "end" => style.justify = Some(taffy::style::JustifyContent::END),
            "flex-start" | "start" | "normal" => style.justify = Some(taffy::style::JustifyContent::START),
            "space-between" => style.justify = Some(taffy::style::JustifyContent::SPACE_BETWEEN),
            "space-around" => style.justify = Some(taffy::style::JustifyContent::SPACE_AROUND),
            "space-evenly" => style.justify = Some(taffy::style::JustifyContent::SPACE_EVENLY),
            "inherit" | "initial" | "unset" => {}
            other => crate::ledger::record_css(&format!("justify-content-value:{}", other)),
        },
        "background-color" => {
            if value.eq_ignore_ascii_case("transparent") {
                style.paint.bg_alpha = Some(0.0);
            } else if !is_neutral_keyword(value) {
                match crate::render::effects::parse_color_alpha(value) {
                    // css-color-4 §4.2: an rgba()/#rrggbbaa background is
                    // composited source-over, not painted opaque.
                    Some((c, a)) => {
                        style.paint.background = Some(c);
                        style.paint.bg_alpha = Some(a);
                    }
                    None => crate::ledger::record_css(&format!("background-value:{}", clip(value))),
                }
            }
        }
        "background-image" if value.to_ascii_lowercase().contains("-gradient(") => {
            style.paint.bg_gradient = crate::render::effects::parse_gradient(value).map(Some);
        }
        "background" if value.to_ascii_lowercase().contains("-gradient(") => {
            // A gradient layer (css-images-3 §3.1, §3.2) plus any colour outside it.
            style.paint.bg_gradient = crate::render::effects::parse_gradient(value).map(Some);
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
                if let Some(c) = parse_color_str(part) {
                    style.paint.background = Some(c);
                    style.paint.bg_alpha = Some(color_alpha(part));
                }
            }
        }
        "box-shadow" | "-webkit-box-shadow" => {
            style.paint.shadows = Some(crate::render::effects::parse_box_shadow(value));
        }
        "background-image" => match extract_css_url(value) {
            Some(u) => style.paint.bg_image = Some(u),
            None => {
                if !is_neutral_keyword(value) {
                    crate::ledger::record_css(&format!("background-image-value:{}", clip(value)));
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
                if let Some(c) = parse_color_str(part) {
                    style.paint.background = Some(c);
                    style.paint.bg_alpha = Some(color_alpha(part));
                    got_color = true;
                    break;
                }
            }
            // Function colors with spaces (rgb(1, 2, 3)) survive as whole-value.
            if !got_color && value.trim().eq_ignore_ascii_case("transparent") {
                style.paint.bg_alpha = Some(0.0);
            } else if !got_color && !is_neutral_keyword(value) && style.paint.bg_image.is_none() {
                match crate::render::effects::parse_color_alpha(value) {
                    Some((c, a)) => {
                        style.paint.background = Some(c);
                        style.paint.bg_alpha = Some(a);
                    }
                    None => crate::ledger::record_css(&format!("background-value:{}", clip(value))),
                }
            }
        }
        "color" => {
            if !is_neutral_keyword(value) {
                match parse_color_str(value) {
                    Some(c) => style.paint.color = Some(c),
                    None => crate::ledger::record_css(&format!("color-value:{}", clip(value))),
                }
            }
        }
        // css-fonts-4 §2.1: the family list, interned (fonts::family_list); matched per family at layout.
        "font-family" => match crate::fonts::parse_family_list(value) {
            Some(list) => style.paint.family = Some(crate::fonts::intern_family_list(list)),
            None => {
                if !is_neutral_keyword(value) {
                    crate::ledger::record_css(&format!("font-family-value:{}", clip(value)));
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
            } else if let Some(px) = parse_px(value) {
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
            let layers: Vec<crate::render::effects::Shadow> =
                crate::render::effects::parse_box_shadow(value).into_iter().filter(|s| !s.inset).collect();
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
            match parse_font_size(size) {
                Some(px) => style.paint.font_size = Some(px),
                None => {
                    crate::ledger::record_css(&format!("font-shorthand:{}", clip(value)));
                    return;
                }
            }
            if let Some(l) = line {
                apply_declaration("line-height", l, style);
            }
            if !family.is_empty() {
                apply_declaration("font-family", family, style);
            }
        }
        "align-items" | "align-content" => match value {
            "center" => style.align_items = Some(taffy::style::AlignItems::CENTER),
            "flex-end" | "end" => style.align_items = Some(taffy::style::AlignItems::END),
            "flex-start" | "start" => style.align_items = Some(taffy::style::AlignItems::START),
            "baseline" => style.align_items = Some(taffy::style::AlignItems::BASELINE),
            "stretch" | "normal" => style.align_items = Some(taffy::style::AlignItems::STRETCH),
            "inherit" | "initial" | "unset" => {}
            other => crate::ledger::record_css(&format!("align-items-value:{}", other)),
        },
        "font-size" => match parse_font_size(value) {
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
                _ => crate::ledger::record_css(&format!("font-size-value:{}", clip(value))),
            },
        },
        "font-weight" => match parse_font_weight(value) {
            Some(w) => style.paint.weight = Some(w),
            None => crate::ledger::record_css(&format!("font-weight-value:{}", clip(value))),
        },
        "visibility" => match value {
            "hidden" | "collapse" => style.paint.hidden = Some(true),
            "visible" => style.paint.hidden = Some(false),
            "inherit" | "initial" | "unset" => {}
            other => crate::ledger::record_css(&format!("visibility:{}", other)),
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
            let (em, ..) = FONT_CTX.with(|c| c.get());
            let lh = if v == "normal" {
                Some(0.0)
            } else if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(-(pct / 100.0 * em))
            } else if let Ok(n) = v.parse::<f32>() {
                Some(n.max(0.0))
            } else {
                parse_px(v).map(|px| -px.max(0.0))
            };
            match lh {
                Some(f) => style.paint.line_height = Some(f),
                None => crate::ledger::record_css(&format!("line-height-value:{}", clip(value))),
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
                if let Some(px) = parse_px(part).filter(|_| part.ends_with("px") || is_font_relative(part)) {
                    width = px;
                    got_any = true;
                } else if let Some(k) = border_style_code(part) {
                    got_any = true;
                    let mut bs = style.paint.border_style.unwrap_or([0; 4]);
                    bs[side] = k;
                    style.paint.border_style = Some(bs);
                } else if let Some(c) = parse_color_str(part) {
                    color = c;
                    got_any = true;
                    style.paint.border_alpha = Some(color_alpha(part));
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
            } else if let Some(px) = parse_px(value) {
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
            if let Some(f) = crate::media::parse_object_fit(value) {
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
                    crate::ledger::record_css(&format!("mask-image-value:{}", clip(value)));
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
            if let Some(w) = parse_px(value) {
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
            let parts: Option<Vec<f32>> = horiz.split_whitespace().map(parse_radius).collect();
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
            if let Some(v) = value.split_whitespace().next().and_then(parse_radius) {
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
                if let Some(px) = parse_px(part).filter(|_| part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '.')) {
                    width = px;
                    got_any = true;
                } else if let Some(k) = border_style_code(part) {
                    got_any = true;
                    style.paint.border_style = Some([k; 4]);
                } else if let Some(c) = parse_color_str(part) {
                    color = c;
                    got_any = true;
                    style.paint.border_alpha = Some(color_alpha(part));
                }
            }
            if got_any {
                style.paint.border = Some([Some((width, color)); 4]);
            } else {
                crate::ledger::record_css(&format!("border-value:{}", clip(value)));
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
            if v == "transparent" || (v.starts_with("rgba(") && parse_color_str(&v).is_none()) {
                style.paint.border = Some([None; 4]);
                return;
            }
            if let Some(c) = parse_color_str(value) {
                style.paint.border_alpha = Some(color_alpha(value));
                let mut sides = style.paint.border.unwrap_or([Some((1.0, (128, 128, 128))); 4]);
                for side in sides.iter_mut() {
                    let w = side.map(|(w, _)| w).unwrap_or(1.0);
                    *side = Some((w, c));
                }
                style.paint.border = Some(sides);
            }
        }
        "border-width" => {
            if let Some(w) = parse_px(value) {
                let mut sides = style.paint.border.unwrap_or([Some((1.0, (128, 128, 128))); 4]);
                for side in sides.iter_mut() {
                    let c = side.map(|(_, c)| c).unwrap_or((128, 128, 128));
                    *side = Some((w, c));
                }
                style.paint.border = Some(sides);
            }
        }
        "border-style" => {} // stroke style is uniform; nothing to record
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
                    )
                }) =>
        {
            let base = other
                .trim_start_matches("-webkit-")
                .trim_start_matches("-moz-")
                .trim_start_matches("-ms-")
                .trim_start_matches("-o-");
            apply_declaration(base, value, style);
        }
        other => crate::ledger::record_css(&format!("property:{}", other)),
    }
}

thread_local! {
    /// The document element's computed custom properties (css_core, set by
    /// `apply_stylesheets`), for the one path outside the cascade: the
    /// layout builder's build-time inline-style pass. The cascade itself
    /// substitutes `var()` per element against that element's own computed
    /// custom properties.
    static CUSTOM_PROPS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Substitutes `var(--x)` / `var(--x, fallback)` from the page's custom
/// property map. Unknown vars use the fallback or resolve to "" (which the
/// property parser then ledgers). Depth-capped against cycles.
pub(crate) fn resolve_vars(value: &str, depth: u8) -> String {
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
        let resolved = CUSTOM_PROPS.with(|m| m.borrow().get(name).cloned());
        match resolved.or_else(|| fallback.map(|f| f.to_string())) {
            Some(v) => out.push_str(&resolve_vars(&v, depth + 1)),
            None => crate::ledger::record_css(&format!("var-unresolved:{}", clip(name))),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Extracts the url from a `url(...)` component (quotes stripped). Returns
/// None for gradients/none/values without a url() component.
pub(crate) fn extract_css_url(value: &str) -> Option<String> {
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
pub(crate) fn strip_css_urls(value: &str) -> String {
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
pub(crate) fn parse_repeat(value: &str) -> Option<u8> {
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
fn clip(v: &str) -> &str {
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

// ---------------------------------------------------------------------
// The cascade (AETHERSTYLE, SR54): UnaOS's own css_core end to end.
//
// Parsing (CSS Syntax 3 + CSS Nesting), selector matching (Selectors 4 over html_core's arena through
// `dom::El`), `@media` (MQ4 against Aether's viewport), `@supports` (against `property_supported`),
// `@layer`, the cascade sort (origin, importance, style attribute, layers, specificity, order), the rule
// hash and per-element custom properties with `var()` substitution are css_core's. What stays Aether's is
// the computed-value step: every declaration that wins its place in the cascade order is handed, as CSS
// text, to `apply_declaration`, which folds it into the `SpecifiedStyle` that layout and paint consume.
// ---------------------------------------------------------------------

thread_local! {
    /// (style, re-layout) wall time of the last `apply_stylesheets`.
    static STYLE_TIMING: std::cell::Cell<(std::time::Duration, std::time::Duration)> =
        const { std::cell::Cell::new((std::time::Duration::ZERO, std::time::Duration::ZERO)) };
}

/// Wall time of the last `apply_stylesheets` call: (the cascade up to the folded box styles, the
/// re-layout that follows). Read by `tests/style_time.rs`.
pub fn last_style_timing() -> (std::time::Duration, std::time::Duration) {
    STYLE_TIMING.with(|t| t.get())
}

/// What `@media` is evaluated against: Aether's viewport, a light colour scheme, no reduced motion, a
/// fine pointer that can hover, `screen`, scripting enabled (css_core's desktop defaults).
pub(crate) fn media_environment(vw: f32, vh: f32) -> css_core::media::Environment {
    css_core::media::Environment::viewport(vw.max(1.0) as f64, vh.max(1.0) as f64)
}

/// Ledgers what a sheet asks for that Aether does not do with it: rules that style a pseudo-element
/// (no generated boxes yet) and at-rules css_core parses into data Aether does not consume.
fn ledger_rules(rules: &[css_core::stylesheet::CssRule]) {
    use css_core::stylesheet::CssRule;
    for r in rules {
        match r {
            CssRule::Style(s) => {
                if s.selectors.0.iter().any(|sel| sel.pseudo_element.is_some()) {
                    crate::ledger::record_css("pseudo-element-rule");
                }
                ledger_rules(&s.children);
            }
            CssRule::Media(_, inner) | CssRule::Supports(_, inner) | CssRule::LayerBlock(_, inner) => ledger_rules(inner),
            CssRule::LayerStatement(_) => {}
            CssRule::Import(_) => crate::ledger::record_css("at-rule:@import"),
            CssRule::FontFace(_) => {}
            CssRule::Keyframes(_) => crate::ledger::record_css("at-rule:@keyframes"),
            CssRule::Other(a) => crate::ledger::record_css(&format!("at-rule:@{}", a.name.to_ascii_lowercase())),
        }
    }
}

/// A declaration's value as CSS text for the computed-value step.
fn value_text(v: &[css_core::CV]) -> String {
    css_core::serialize::to_css(css_core::parser::trim_ws(v))
}

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    apply_stylesheets(layout_tree, std::slice::from_ref(&css.to_string()));
}

/// Applies one specified-declaration set to one node (layout fold + paint
/// merge). Records absolute-positioning facts for the static-position
/// fixup that runs after the cascade.
fn apply_spec_to_node(
    layout_tree: &mut LayoutTree,
    node_id: taffy::NodeId,
    style: &SpecifiedStyle,
    abs_nodes: &mut std::collections::HashSet<taffy::NodeId>,
    inset_nodes: &mut std::collections::HashSet<taffy::NodeId>,
) {
    if let Ok(node_style_ref) = layout_tree.taffy.style(node_id) {
        let mut node_style = node_style_ref.clone();
        style.fold_into(&mut node_style);
        let _ = layout_tree.taffy.set_style(node_id, node_style);
    }
    match style.position {
        Some(taffy::style::Position::Absolute) => { abs_nodes.insert(node_id); }
        Some(taffy::style::Position::Relative) => { abs_nodes.remove(&node_id); }
        None => {}
    }
    if style.inset_top.is_some() || style.inset_left.is_some()
        || style.inset_right.is_some() || style.inset_bottom.is_some()
    {
        inset_nodes.insert(node_id);
    }
    let entry = layout_tree.paint_map.entry(node_id).or_default();
    merge_paint(entry, &style.paint);
}

/// Overlays `src`'s specified paint fields onto `dst`. ONE list, used by
/// every cascade path — a field missing here is a declaration that parses
/// and then never reaches the renderer.
pub(crate) fn merge_paint(dst: &mut PaintStyle, src: &PaintStyle) {
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
        stretch, word_spacing, rtl,
    );
    clone!(text_shadows);
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

/// The cascaded style of every element of the document: for each element with a box, the
/// `SpecifiedStyle` its winning declarations fold into (`None` when no declaration applies).
///
/// Per element: css_core's `cascade` (rule-hash candidates, matching, the full Cascade 5 sort, the style
/// attribute) → its custom properties computed against the parent's (`compute_custom_properties`:
/// inheritance, cycles, guaranteed-invalid) → every other declaration, in cascade order, `var()`
/// substituted (`substitute_vars`; a failed substitution is invalid at computed-value time and dropped)
/// and handed to `apply_declaration`. Elements whose winning declarations are the same list (and use no
/// `var()`) share one folded style — the common case of every `<p>` or every `.reference` on a page.
fn cascade_document(
    doc: &html_core::Document,
    rules: &css_core::cascade::RuleSet<'_>,
    boxed: &std::collections::HashSet<html_core::NodeId>,
) -> (
    std::collections::HashMap<html_core::NodeId, std::rc::Rc<SpecifiedStyle>>,
    std::collections::BTreeMap<String, Vec<css_core::CV>>,
) {
    use css_core::values::{compute_custom_properties, contains_var, substitute_vars};
    use std::collections::{BTreeMap, HashMap};
    use std::rc::Rc;
    type Customs = Rc<BTreeMap<String, Vec<css_core::CV>>>;
    let empty: Customs = Rc::new(BTreeMap::new());
    let mut customs: HashMap<html_core::NodeId, Customs> = HashMap::new();
    // each element's children's ancestor Bloom filter (css_core's quick reject)
    let mut child_filter: HashMap<html_core::NodeId, css_core::cascade::AncestorFilter> = HashMap::new();
    let mut shared: HashMap<Vec<usize>, Rc<SpecifiedStyle>> = HashMap::new();
    let mut out = HashMap::new();
    let mut root_customs = BTreeMap::new();
    for id in doc.descendants(html_core::Document::ROOT) {
        let Some(el) = doc.element(id) else { continue };
        let inline = el.attr("style").map(css_core::stylesheet::parse_style_attribute).unwrap_or_default();
        let me_el = crate::dom::El::new(doc, id);
        let filter = doc.parent_element(id).and_then(|p| child_filter.get(&p)).copied().unwrap_or_default();
        let applied = css_core::cascade::cascade_filtered(rules, &me_el, &inline, None, Some(&filter));
        child_filter.insert(id, filter.with_element(&me_el));
        // custom properties: inherited from the parent element, overridden by this element's own
        let parent = doc.parent_element(id).and_then(|p| customs.get(&p)).cloned().unwrap_or_else(|| empty.clone());
        let own: Vec<(String, Vec<css_core::CV>)> = applied
            .iter()
            .filter(|a| a.declaration.name.starts_with("--"))
            .map(|a| (a.declaration.name.clone(), a.declaration.value.clone()))
            .collect();
        let mine = if own.is_empty() { parent } else { Rc::new(compute_custom_properties(&own, &parent)) };
        if doc.parent(id) == Some(html_core::Document::ROOT) {
            root_customs = (*mine).clone();
        }
        if !boxed.contains(&id) {
            customs.insert(id, mine);
            continue;
        }
        let decls: Vec<&css_core::parser::Declaration> =
            applied.iter().map(|a| a.declaration).filter(|d| !d.name.starts_with("--")).collect();
        if decls.is_empty() {
            customs.insert(id, mine);
            continue;
        }
        let uses_var = decls.iter().any(|d| contains_var(&d.value));
        // The share key is the winning declarations' addresses in the rule set. A style attribute's
        // declarations live in `inline`, which is freed at the end of this iteration, so the next
        // element's attribute can reuse the same addresses: an element with a style attribute never
        // shares (the AETHERFONT join found two inline-styled siblings folded into one style).
        let shareable = !uses_var && inline.is_empty();
        let key: Vec<usize> = decls.iter().map(|d| *d as *const _ as usize).collect();
        let style = match shareable.then(|| shared.get(&key)).flatten() {
            Some(s) => s.clone(),
            None => {
                let mut s = SpecifiedStyle::default();
                for d in &decls {
                    let name = d.name.to_ascii_lowercase();
                    if contains_var(&d.value) {
                        match substitute_vars(&d.value, &|n| mine.get(n).cloned()) {
                            Ok(v) => apply_declaration(&name, &value_text(&v), &mut s),
                            Err(()) => crate::ledger::record_css(&format!("var-unresolved:{}", clip(&name))),
                        }
                    } else {
                        apply_declaration(&name, &value_text(&d.value), &mut s);
                    }
                }
                let s = Rc::new(s);
                if shareable {
                    shared.insert(key, s.clone());
                }
                s
            }
        };
        out.insert(id, style);
        customs.insert(id, mine);
    }
    (out, root_customs)
}

/// Applies a set of stylesheets (document order, author origin) as ONE cascade through css_core, with
/// each element's `style` attribute at its CSS priority (above normal rules, below `!important` ones;
/// an `!important` style attribute beats every author rule).
pub fn apply_stylesheets(layout_tree: &mut LayoutTree, sheets: &[String]) {
    let t0 = std::time::Instant::now();
    let (vw, vh) = layout_tree.viewport;
    set_viewport(vw, vh);
    let parsed: Vec<css_core::stylesheet::Stylesheet> =
        sheets.iter().map(|s| css_core::stylesheet::parse_stylesheet(s)).collect();
    for s in &parsed {
        ledger_rules(&s.rules);
    }
    // css-fonts-4 §4: the document's @font-face faces (AETHERFONT); a changed set bumps the font caches.
    crate::fonts::webfont::apply_rules(&parsed, &crate::images::page_base());
    let env = media_environment(vw, vh);
    let supports = |d: &css_core::parser::Declaration| property_supported(&d.name.to_ascii_lowercase());
    let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
    let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = css_core::cascade::RuleSet::new();
    for s in &parsed {
        rules.add_sheet(s, css_core::cascade::Origin::Author, &cond);
    }
    let t_parse = t0.elapsed();
    // AETHER_NO_RULE_INDEX: the full scan, for measuring the hash (tests/style_time.rs);
    // AETHER_STYLE_PROFILE: print the phase times.
    if std::env::var_os("AETHER_NO_RULE_INDEX").is_none() {
        rules.build_index();
    }

    // The boxes of each DOM element (a layout node maps back to one DOM node).
    let mut boxes: std::collections::HashMap<html_core::NodeId, Vec<taffy::NodeId>> = std::collections::HashMap::new();
    let mut any: Option<crate::dom::NodeRef> = None;
    for (tid, n) in &layout_tree.node_map {
        if n.is_element() {
            boxes.entry(n.id()).or_default().push(*tid);
            any.get_or_insert_with(|| n.clone());
        }
    }
    let Some(any) = any else {
        STYLE_TIMING.with(|t| t.set((t0.elapsed(), std::time::Duration::ZERO)));
        return;
    };
    let boxed: std::collections::HashSet<html_core::NodeId> = boxes.keys().copied().collect();
    let t_c = std::time::Instant::now();
    let (styles, root_customs) = any.with_doc(|doc, _| cascade_document(doc, &rules, &boxed));
    if std::env::var_os("AETHER_STYLE_PROFILE").is_some() {
        eprintln!(
            "[style] parse + rule set {:?}, cascade {:?}, {} selector entries, rule hash {}",
            t_parse,
            t_c.elapsed(),
            rules.entries.len(),
            if rules.index().is_some() { "on" } else { "off (AETHER_NO_RULE_INDEX)" }
        );
    }
    // The build-time inline-style pass (layout) resolves var() against the document's tokens.
    CUSTOM_PROPS.with(|m| {
        let mut map = m.borrow_mut();
        map.clear();
        for (k, v) in &root_customs {
            map.insert(k.clone(), value_text(v));
        }
    });

    let mut acc: std::collections::HashMap<taffy::NodeId, std::rc::Rc<SpecifiedStyle>> = std::collections::HashMap::new();
    for (dom_id, style) in styles {
        for tid in boxes.get(&dom_id).into_iter().flatten() {
            acc.insert(*tid, style.clone());
        }
    }
    let mut abs_nodes = std::collections::HashSet::new();
    let mut inset_nodes = std::collections::HashSet::new();
    for (node_id, spec) in &acc {
        apply_spec_to_node(layout_tree, *node_id, spec, &mut abs_nodes, &mut inset_nodes);
    }
    size_flex_items_by_content(layout_tree, &acc);
    LAST_CASCADE.with(|l| *l.borrow_mut() = acc.clone());

    // Static-position fallback: `position:absolute` with no inset specified
    // keeps its in-flow (static) position in real CSS. Taffy would pin such
    // a box to the parent origin, smearing it over siblings (the wikipedia
    // header overlap), so leave it in flow instead.
    for node_id in abs_nodes.difference(&inset_nodes) {
        if let Ok(style_ref) = layout_tree.taffy.style(*node_id) {
            let mut s = style_ref.clone();
            s.position = taffy::style::Position::Relative;
            let _ = layout_tree.taffy.set_style(*node_id, s);
        }
    }
    let t1 = std::time::Instant::now();
    // set_style only marks nodes dirty; re-lay out with text measurement.
    crate::layout::remeasure(layout_tree);
    STYLE_TIMING.with(|t| t.set((t1 - t0, t1.elapsed())));
}

thread_local! {
    /// The folded `SpecifiedStyle` of each box from the last `apply_stylesheets`, for
    /// [`computed_report`] (the facts the box tree no longer distinguishes: `display: flex` versus
    /// Aether's block-as-column-flex approximation, `position`, `flex-direction`).
    static LAST_CASCADE: std::cell::RefCell<std::collections::HashMap<taffy::NodeId, std::rc::Rc<SpecifiedStyle>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The properties [`computed_report`] serializes, in css_core's oracle order (CSSCORE SR47).
pub const REPORT_PROPS: [&str; 20] = [
    "display", "position", "color", "background-color", "font-size", "font-weight", "font-style", "font-family", "line-height",
    "text-align", "text-decoration-line", "white-space", "visibility", "list-style-type", "border-top-width", "border-top-style",
    "border-left-color", "padding-top", "margin-top", "flex-direction",
];

/// AETHERSTYLE (SR54) M5: AETHER's computed values — what its layout and paint actually consume after
/// `apply_stylesheets` — serialized the way `getComputedStyle` serializes them, for every element of the
/// tree's document (keyed by DOM node). Inherited properties follow the renderer's own inheritance
/// (`render::draw_node`: the per-tag UA defaults of `layout::default_*`, the link colour, underline
/// propagation); box properties come from the taffy style; the values Aether does not model are
/// reported as what it effectively uses (sans/serif/monospace classes, solid borders, no list markers,
/// static for every non-absolute position). An element without a box reports `display: none`.
pub fn computed_report(tree: &LayoutTree) -> std::collections::HashMap<html_core::NodeId, [String; 20]> {
    use std::collections::HashMap;
    #[derive(Clone)]
    struct Inh {
        color: (u8, u8, u8),
        font_size: f32,
        weight: u16,
        italic: bool,
        family: u16,
        line_height: f32,
        text_align: u8,
        underline: bool,
        nowrap: bool,
        hidden: bool,
    }
    fn num(v: f32) -> String {
        let r = (v as f64 * 10000.0).round() / 10000.0;
        if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
    }
    let px = |v: f32| format!("{}px", num(v));
    let rgb = |c: (u8, u8, u8)| format!("rgb({}, {}, {})", c.0, c.1, c.2);
    let lp = |v: LengthPercentage| {
        use taffy::style::ExpandedLengthPercentage as E;
        match v.expand() {
            E::Percent(p) => format!("{}%", num(p * 100.0)),
            E::Length(l) => px(l),
            #[allow(unreachable_patterns)]
            _ => "calc".to_string(),
        }
    };
    let lpa_s = |v: LengthPercentageAuto| {
        use taffy::style::ExpandedLengthPercentageAuto as E;
        match v.expand() {
            E::Auto => "auto".to_string(),
            E::Percent(p) => format!("{}%", num(p * 100.0)),
            E::Length(l) => px(l),
            #[allow(unreachable_patterns)]
            _ => "calc".to_string(),
        }
    };
    let mut out = HashMap::new();
    let Some(any) = tree.node_map.values().find(|n| n.is_element()) else { return out };
    let mut box_of: HashMap<html_core::NodeId, taffy::NodeId> = HashMap::new();
    for (tid, n) in &tree.node_map {
        if n.is_element() {
            box_of.insert(n.id(), *tid);
        }
    }
    let last = LAST_CASCADE.with(|l| l.borrow().clone());
    any.with_doc(|doc, _| {
        let root = Inh {
            color: (0, 0, 0), font_size: 16.0, weight: 400, italic: false, family: crate::fonts::STANDARD, line_height: 0.0,
            text_align: 0, underline: false, nowrap: false, hidden: false,
        };
        let mut inh: HashMap<html_core::NodeId, Inh> = HashMap::new();
        for id in doc.descendants(html_core::Document::ROOT) {
            let Some(el) = doc.element(id) else { continue };
            let tag = el.local.as_str();
            let parent = doc.parent_element(id).and_then(|p| inh.get(&p)).cloned().unwrap_or_else(|| root.clone());
            let tid = box_of.get(&id).copied();
            let paint = tid.and_then(|t| tree.paint_map.get(&t)).cloned().unwrap_or_default();
            let spec = tid.and_then(|t| last.get(&t)).cloned();
            let mut me = parent.clone();
            me.font_size = paint.font_size.unwrap_or_else(|| crate::layout::default_font_size(tag, parent.font_size));
            me.weight = paint
                .weight
                .map(|w| w.resolve(parent.weight))
                .unwrap_or_else(|| crate::layout::default_weight(tag, parent.weight));
            me.italic = paint.italic.unwrap_or_else(|| crate::layout::default_italic(tag, parent.italic));
            me.family = paint.family.unwrap_or_else(|| crate::layout::default_family(tag, parent.family));
            if let Some(lh) = paint.line_height {
                me.line_height = lh;
            }
            if let Some(a) = paint.text_align {
                me.text_align = a;
            }
            if tag == "a" || tag == "u" {
                me.underline = true;
            }
            if let Some(u) = paint.underline {
                me.underline = u;
            }
            if let Some(nw) = paint.nowrap {
                me.nowrap = nw;
            }
            if let Some(h) = paint.hidden {
                me.hidden = h;
            }
            me.color = paint.color.unwrap_or(if tag == "a" { (0, 0, 238) } else { parent.color });
            let st = tid.and_then(|t| tree.taffy.style(t).ok()).cloned();
            let display = match (&st, &spec) {
                (None, _) => "none",
                (Some(s), _) if s.display == Display::None => "none",
                (_, Some(sp)) if sp.flex_container == Some(true) => "flex",
                _ if crate::layout::is_inline(tag) => "inline",
                _ => "block",
            };
            // AETHERJS: the five position kinds Aether paints (relative/sticky offsets, fixed to the
            // viewport) — the report used to fold everything but absolute into static.
            let position = match paint.position_kind {
                Some(1) => "relative",
                Some(2) => "absolute",
                Some(3) => "fixed",
                Some(4) => "sticky",
                _ => match spec.as_ref().and_then(|s| s.position) {
                    Some(taffy::style::Position::Absolute) => "absolute",
                    _ => "static",
                },
            };
            let flex_direction = match spec.as_ref().and_then(|s| s.flex_direction) {
                Some(FlexDirection::Column) => "column",
                Some(FlexDirection::ColumnReverse) => "column-reverse",
                Some(FlexDirection::RowReverse) => "row-reverse",
                _ => "row",
            };
            let side = |i: usize| paint.border.and_then(|b| b[i]);
            let bw_top = paint.border_width.and_then(|w| w[0]).or(side(0).map(|s| s.0)).unwrap_or(0.0);
            let lh = if me.line_height <= 0.0 { "normal".to_string() } else { px(me.line_height * me.font_size) };
            let values = [
                display.to_string(),
                position.to_string(),
                rgb(me.color),
                paint.background.map(rgb).unwrap_or_else(|| "rgba(0, 0, 0, 0)".to_string()),
                px(me.font_size),
                me.weight.to_string(),
                if me.italic { "italic" } else { "normal" }.to_string(),
                crate::fonts::serialize_family_list(&crate::fonts::family_list(me.family)),
                lh,
                match me.text_align { 1 => "center", 2 => "right", 3 => "end", 4 => "left", _ => "start" }.to_string(),
                if me.underline { "underline" } else { "none" }.to_string(),
                if me.nowrap { "nowrap" } else { "normal" }.to_string(),
                if me.hidden { "hidden" } else { "visible" }.to_string(),
                "disc".to_string(),
                px(if side(0).is_some() || bw_top > 0.0 { bw_top } else { 0.0 }),
                if side(0).is_some() && bw_top > 0.0 { "solid" } else { "none" }.to_string(),
                rgb(side(3).map(|s| s.1).unwrap_or(me.color)),
                st.as_ref().map(|s| lp(s.padding.top)).unwrap_or_else(|| px(0.0)),
                st.as_ref().map(|s| lpa_s(s.margin.top)).unwrap_or_else(|| px(0.0)),
                flex_direction.to_string(),
            ];
            out.insert(id, values);
            inh.insert(id, me);
        }
    });
    out
}

/// A flex item's `width: auto` is its content size, not its container's.
/// Aether gives every block the block-flow default `width: 100%`; inside a
/// real flex container that default would make each item claim the whole
/// line (one item per row under wrap). Items whose author CSS sets no
/// width fall back to `auto` here, after the cascade has settled.
fn size_flex_items_by_content(
    layout_tree: &mut LayoutTree,
    acc: &std::collections::HashMap<taffy::NodeId, std::rc::Rc<SpecifiedStyle>>,
) {
    for (node_id, spec) in acc {
        if spec.flex_container != Some(true) {
            continue;
        }
        let row = layout_tree.taffy.style(*node_id).is_ok_and(|s| {
            matches!(s.flex_direction, FlexDirection::Row | FlexDirection::RowReverse)
        });
        let kids = layout_tree.taffy.children(*node_id).unwrap_or_default();
        for kid in kids {
            let author_width = acc.get(&kid).is_some_and(|s| s.width.is_some());
            let Ok(st) = layout_tree.taffy.style(kid) else { continue };
            if author_width || st.size.width != Dimension::percent(1.0) {
                continue;
            }
            let mut st = st.clone();
            // Row: main-axis size from content/basis. Column: stretch
            // (the default align-items) already fills the cross axis.
            if row {
                st.size.width = Dimension::auto();
                let _ = layout_tree.taffy.set_style(kid, st);
            }
        }
    }
}

/// Parses a declaration block into (normal, !important) tiers plus a flag
/// saying whether the important tier holds anything. Declarations split by
/// their own priority, per CSS — not per rule.
/// Splits a declaration block on the semicolons that actually separate
/// declarations — a `;` inside url(), a function, or a string does not
/// (`background:url(data:image/svg+xml;base64,...)` is one declaration,
/// and splitting it naively truncates the value at the media type).
pub(crate) fn split_declarations(body: &str) -> Vec<&str> {
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
pub(crate) fn split_declaration(decl: &str) -> Option<(&str, &str)> {
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

pub(crate) fn parse_declaration_block_tiers(body: &str) -> (SpecifiedStyle, SpecifiedStyle, bool) {
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
                apply_declaration(&prop, &resolve_vars(v.trim(), 0), &mut important);
            }
            None => apply_declaration(&prop, &resolve_vars(value, 0), &mut normal),
        }
    }
    (normal, important, has_important)
}

/// Parses a `prop: value; prop: value` declaration block (no braces) into a
/// single set — `!important` declarations simply win within the block. Used
/// where tiers don't matter (build-time inline defaults).
pub(crate) fn parse_declaration_block(body: &str) -> SpecifiedStyle {
    let (mut normal, important, has_important) = parse_declaration_block_tiers(body);
    if has_important {
        merge_specified(&mut normal, &important);
    }
    normal
}

/// Overlays `src`'s specified fields onto `dst`.
fn merge_specified(dst: &mut SpecifiedStyle, src: &SpecifiedStyle) {
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
          flex_container, flex_wrap, row_gap, column_gap, flex_grow, flex_shrink, flex_basis);
    merge_paint(&mut dst.paint, &src.paint);
}

/// Properties this engine genuinely implements (the honest support set
/// for @supports; extend when apply_declaration grows an arm).
fn property_supported(prop: &str) -> bool {
    matches!(
        prop,
        "display" | "flex-direction" | "width" | "height" | "padding" | "margin"
            | "padding-top" | "padding-right" | "padding-bottom" | "padding-left"
            | "margin-top" | "margin-right" | "margin-bottom" | "margin-left"
            | "position" | "top" | "left" | "right" | "bottom" | "text-align" | "justify-content"
            | "font" | "align-items" | "align-content"
            | "background-color" | "background" | "background-image" | "color"
            | "background-size" | "background-position" | "background-repeat"
            | "object-fit" | "object-position"
            // Alpha-stencil masks really are implemented (render::draw_node),
            // so the component idiom `@supports (mask-image: none)` must take
            // the mask branch, not the background-image fallback branch.
            | "mask" | "mask-image" | "mask-size" | "mask-position" | "mask-repeat"
            | "-webkit-mask" | "-webkit-mask-image" | "-webkit-mask-size"
            | "-webkit-mask-position" | "-webkit-mask-repeat"
            | "font-size" | "font-weight" | "line-height" | "visibility"
            | "max-width" | "max-height" | "min-width" | "min-height"
            | "overflow" | "overflow-x" | "overflow-y"
            | "border" | "outline" | "border-color" | "border-width" | "border-style"
            | "border-top" | "border-right" | "border-bottom" | "border-left"
            | "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width"
            | "text-decoration" | "text-decoration-line" | "text-transform" | "white-space" | "box-sizing"
            | "font-family" | "font-style" | "font-stretch" | "letter-spacing" | "word-spacing" | "direction"
            | "text-shadow"
            | "flex" | "flex-grow" | "flex-shrink" | "flex-basis" | "flex-wrap" | "flex-flow"
            | "gap" | "row-gap" | "column-gap"
    )
}

/// Parses a CSS color from a string: named, #rgb/#rrggbb, rgb()/rgba().
/// rgba() alpha is ignored (no compositing yet) unless fully transparent.
pub fn parse_color_str(value: &str) -> Option<(u8, u8, u8)> {
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
    named_color(&lower)
}

/// Keywords that specify "no concrete value here" — not coverage gaps.
pub fn is_neutral_keyword(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "transparent" | "inherit" | "initial" | "unset" | "currentcolor" | "none"
    )
}

const BASE_FONT_PX: f32 = 16.0;

thread_local! {
    /// The font-relative length basis (css-values-4 §6.1) in effect: (em,
    /// rem, ex, ch) in px. The cascade parses at the initial 16px font;
    /// `layout::remeasure` re-applies an element's font-relative
    /// declarations with its own computed font-size, the root's, and its
    /// face's x-height and `0` advance.
    static FONT_CTX: std::cell::Cell<(f32, f32, f32, f32)> = const { std::cell::Cell::new((16.0, 16.0, 8.0, 8.0)) };
}

/// Sets the font-relative basis (em, rem, ex, ch) for the declarations
/// parsed next; returns the previous one.
pub(crate) fn set_font_ctx(ctx: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    FONT_CTX.with(|c| c.replace(ctx))
}

/// The alpha of a colour value (1 for opaque or unparsable colours).
fn color_alpha(v: &str) -> f32 {
    crate::render::effects::parse_color_alpha(v).map(|c| c.1).unwrap_or(1.0)
}

/// The current font-relative basis (em, rem, ex, ch).
pub(crate) fn font_ctx() -> (f32, f32, f32, f32) {
    FONT_CTX.with(|c| c.get())
}

/// Parses `decls` (property, value) in order into one specified style, at
/// the current font basis.
pub(crate) fn reapply_declarations(decls: &[(String, String)]) -> SpecifiedStyle {
    let mut spec = SpecifiedStyle::default();
    for (p, v) in decls {
        apply_declaration(p, v, &mut spec);
    }
    spec
}

/// px per one `unit` for the font-relative units, from the current basis.
fn font_unit_px(unit: &str) -> Option<f32> {
    let (em, rem, ex, ch) = FONT_CTX.with(|c| c.get());
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
fn parse_font_relative(v: &str) -> Option<f32> {
    for unit in ["rem", "em", "ex", "ch"] {
        if let Some(n) = v.strip_suffix(unit) {
            let n = n.trim();
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e')) {
                if let (Ok(n), Some(px)) = (n.parse::<f32>(), font_unit_px(unit)) {
                    return Some(n * px);
                }
            }
        }
    }
    None
}

/// True when a value uses a font-relative unit anywhere (`1em`, `calc(2rem
/// + 4px)`, `0 1.5ch`): its px depend on the element's font.
pub(crate) fn is_font_relative(value: &str) -> bool {
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
pub(crate) fn is_length_property(prop: &str) -> bool {
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
pub fn parse_font_size(value: &str) -> Option<f32> {
    let v = value.trim().to_ascii_lowercase();
    // css-values-4 math (`clamp(1.25rem, 4vw, 2.5rem)`); % is of the
    // (approximated 16px) parent size.
    if v.contains('(') {
        return eval_length(&v, Some(BASE_FONT_PX));
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
pub fn parse_font_weight(value: &str) -> Option<crate::layout::FontWeight> {
    use crate::layout::FontWeight;
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
pub fn parse_px(value: &str) -> Option<f32> {
    let v = value.trim();
    if v.contains('(') {
        return eval_length(v, None);
    }
    if let Some(px) = parse_viewport_length(v) {
        return Some(px);
    }
    if let Some(px) = parse_font_relative(v) {
        return Some(px);
    }
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

thread_local! {
    /// Viewport the cascade resolves `vh`/`vw` against. Set by the layout
    /// builder and the cascade entry point; both know the real size.
    static VIEWPORT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((800.0, 600.0)) };
}

/// Records the viewport that viewport-relative units resolve against.
pub fn set_viewport(w: f32, h: f32) {
    VIEWPORT.with(|v| v.set((w.max(1.0), h.max(1.0))));
}

/// Resolves a viewport-relative length (`vh`, `vw`, `vmin`, `vmax`, and the
/// dynamic/small/large `dvh`/`svh`/`lvh` family — with no browser chrome to
/// collapse, all three equal the viewport). `height: 100vh` is how nearly
/// every modern shell states its full-height column; dropping it collapsed
/// those columns to their content and stacked the page wrong.
pub(crate) fn parse_viewport_length(v: &str) -> Option<f32> {
    let v = v.trim();
    let (vw, vh) = VIEWPORT.with(|c| c.get());
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
pub fn eval_length(value: &str, reference: Option<f32>) -> Option<f32> {
    let mut p = MathParser { s: value.trim().as_bytes(), i: 0, reference };
    let v = p.expr()?;
    p.ws();
    if p.i != p.s.len() {
        return None;
    }
    v.is_finite().then_some(v)
}

struct MathParser<'a> {
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
            let name = std::str::from_utf8(&self.s[start..self.i]).ok()?.to_ascii_lowercase();
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
        let n: f32 = std::str::from_utf8(&self.s[nstart..self.i]).ok()?.parse().ok()?;
        let ustart = self.i;
        if self.s.get(self.i) == Some(&b'%') {
            self.i += 1;
            return self.reference.map(|r| n / 100.0 * r);
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        let unit = std::str::from_utf8(&self.s[ustart..self.i]).ok()?.to_ascii_lowercase();
        match unit.as_str() {
            "" | "px" => Some(n),
            "pt" => Some(n * 4.0 / 3.0),
            "rem" | "em" | "ex" | "ch" => font_unit_px(&unit).map(|u| n * u),
            // `calc(100vh - 64px)` is the standard full-height idiom.
            u => parse_viewport_length(&format!("{}{}", n, u)),
        }
    }
}

/// `display` value -> Aether's box approximation (flex-backed).
fn apply_display(value: &str, style: &mut SpecifiedStyle) {
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
        other => crate::ledger::record_css(&format!("display:{}", other)),
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
fn parse_radius(v: &str) -> Option<f32> {
    if let Some(p) = v.trim().strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(-(p / 100.0).max(0.0));
    }
    parse_px(v).map(|x| x.max(0.0))
}

/// Splits a value on whitespace outside parentheses: `calc(1rem + 2px) 4px`
/// is two components, not four.
pub(crate) fn split_top_level(value: &str) -> Vec<&str> {
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
fn parse_flex_basis(value: &str) -> Option<Dimension> {
    match value.trim() {
        "auto" | "content" => Some(Dimension::auto()),
        v => parse_dimension_str(v),
    }
}

/// The `flex` shorthand → (grow, shrink, basis), per CSS Flexbox §7.2:
/// `none` = 0 0 auto, `auto` = 1 1 auto, `initial` = 0 1 auto; a unitless
/// first number is grow, an optional second number is shrink, and a lone
/// or trailing length is the basis (an omitted basis is 0%).
pub(crate) fn parse_flex_shorthand(value: &str) -> Option<(f32, f32, Dimension)> {
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
        basis = Some(parse_flex_basis(part)?);
    }
    match (nums.as_slice(), basis) {
        ([], Some(b)) => Some((1.0, 1.0, b)),
        ([g], b) => Some((*g, 1.0, b.unwrap_or(Dimension::percent(0.0)))),
        ([g, s], b) => Some((*g, *s, b.unwrap_or(Dimension::percent(0.0)))),
        _ => None,
    }
}

fn parse_dimension_str(value: &str) -> Option<Dimension> {
    let v = value.trim();
    if v == "auto" {
        return Some(Dimension::auto());
    }
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(Dimension::percent(pct / 100.0));
    }
    parse_px(v).map(Dimension::length)
}

fn parse_length_percentage_str(value: &str) -> Option<LengthPercentage> {
    let v = value.trim();
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(LengthPercentage::percent(pct / 100.0));
    }
    parse_px(v).map(LengthPercentage::length)
}

fn parse_length_percentage_auto_str(value: &str) -> Option<LengthPercentageAuto> {
    let v = value.trim();
    if v == "auto" {
        return Some(LengthPercentageAuto::auto());
    }
    parse_length_percentage_str(v).map(Into::into)
}

fn named_color(name: &str) -> Option<(u8, u8, u8)> {
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
        other => {
            crate::ledger::record_css(&format!("named-color:{}", clip(other)));
            None
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_size_and_weight_parsing() {
        assert_eq!(parse_font_size("14px"), Some(14.0));
        assert_eq!(parse_font_size("1.5em"), Some(24.0));
        assert_eq!(parse_font_size("2rem"), Some(32.0));
        assert_eq!(parse_font_size("110%"), Some(17.6));
        assert_eq!(parse_font_size("medium"), Some(16.0));
        assert_eq!(parse_font_size("banana"), None);
        use crate::layout::FontWeight;
        assert_eq!(parse_font_weight("bold"), Some(FontWeight::Abs(700)));
        assert_eq!(parse_font_weight("400"), Some(FontWeight::Abs(400)));
        assert_eq!(parse_font_weight("650"), Some(FontWeight::Abs(650)));
        assert_eq!(parse_font_weight("bolder"), Some(FontWeight::Bolder));
        assert_eq!(FontWeight::Bolder.resolve(400), 700);
        assert_eq!(FontWeight::Bolder.resolve(700), 900);
        assert_eq!(FontWeight::Lighter.resolve(700), 400);
        assert_eq!(FontWeight::Lighter.resolve(400), 100);
        assert_eq!(parse_font_stretch("condensed"), Some(75));
        assert_eq!(parse_font_stretch("120%"), Some(120));
        assert!(is_neutral_keyword("transparent"));
        assert!(is_neutral_keyword("Inherit"));
        assert!(!is_neutral_keyword("red"));
    }

    #[test]
    fn test_color_functions() {
        assert_eq!(parse_color_str("rgb(10, 20, 30)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0.5)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0)"), None);
        assert_eq!(parse_color_str("#abc"), Some((170, 187, 204)));
        assert_eq!(parse_color_str("#336699"), Some((51, 102, 153)));
    }

    /// Colour of the element with `id` after applying `css` to `html`.
    fn color_of(html: &str, css: &str, id: &str) -> Option<(u8, u8, u8)> {
        let mut tree = crate::layout::compute_layout(&crate::dom::parse_html(html));
        apply_css(&mut tree, css);
        for (node_id, dom_node) in &tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            if el.attributes.borrow().get("id") == Some(id) {
                return tree.paint_map.get(node_id).and_then(|p| p.color);
            }
        }
        None
    }

    const LOWER_HTML: &str = r#"<html><body>
        <div id="a" class="x"><span id="s">s</span></div>
        <p id="b" class="y">y</p>
        <input id="i" required>
    </body></html>"#;

    /// Escaped punctuation is part of an identifier (CSS Syntax 3 §4.3.7): the
    /// whole `md:`/`lg:` utility layer of a utility-CSS page keys and matches
    /// on the class string the DOM actually carries.
    #[test]
    fn test_escaped_utility_classes() {
        let html = r#"<html><body><div id="a" class="md:block w-1/2 h-[5.75rem]">x</div></body></html>"#;
        for sel in [r".md\:block", r".w-1\/2", r".h-\[5\.75rem\]", r"body .md\:block", r".md\3a block"] {
            let css = format!("{sel} {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(html, &css, "a"), Some((1, 2, 3)), "{sel}");
        }
    }

    /// Selectors 4 through css_core (AETHERSTYLE): `:is()`/`:where()`, `:not()`
    /// with a list, `:has()`, form states. (The legacy `:-webkit-any()` that
    /// the old lowering rewrote is not a css_core pseudo-class: owed.)
    #[test]
    fn test_selectors4_match() {
        for sel in ["body :is(.x, .y)", "body :where(.x, .y)"] {
            let css = format!("{sel} {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(LOWER_HTML, &css, "a"), Some((1, 2, 3)), "{sel}");
            assert_eq!(color_of(LOWER_HTML, &css, "b"), Some((1, 2, 3)), "{sel}");
        }
        assert_eq!(color_of(LOWER_HTML, "p:is(.y, .z) { color: rgb(4, 5, 6); }", "b"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, "body :is(.x > span) { color: rgb(7, 8, 9); }", "s"), Some((7, 8, 9)));
        assert_eq!(color_of(LOWER_HTML, "div.x:is(body > div) { color: rgb(7, 8, 9); }", "a"), Some((7, 8, 9)));
        assert_eq!(color_of(LOWER_HTML, ".x:not(.q, .r) { color: rgb(1, 2, 3); }", "a"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, ".x:not(.q, .x) { color: rgb(1, 2, 3); }", "a"), None);
        assert_eq!(color_of(LOWER_HTML, ".x:not(:focus-visible) { color: rgb(4, 5, 6); }", "a"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, ".x:has(> span) { color: rgb(4, 5, 6); }", "a"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, ".y:has(> span) { color: rgb(4, 5, 6); }", "b"), None);
        assert_eq!(color_of(LOWER_HTML, "input:required { color: rgb(1, 2, 3); }", "i"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, "input:optional { color: rgb(1, 2, 3); }", "i"), None);
        assert_eq!(color_of(LOWER_HTML, "input:placeholder-shown { color: rgb(1, 2, 3); }", "i"), None);
        assert_eq!(color_of(LOWER_HTML, ":lang(en) { color: rgb(1, 2, 3); }", "a"), None);
        assert_eq!(color_of(LOWER_HTML, ".x:focus-within { color: rgb(1, 2, 3); }", "a"), None);
    }

    /// Specificity is the spec's (Selectors 4 §17): `:where()` weighs nothing,
    /// `:is()`/`:not()`/`:has()` weigh their most specific argument.
    #[test]
    fn test_selectors4_specificity() {
        let css = "#a:where(.x) { color: rgb(1, 1, 1); } .x.x { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "a"), Some((1, 1, 1)));
        let css = ".y:is(.y) { color: rgb(1, 1, 1); } .y.y { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((2, 2, 2)));
        // `:is(#b, p)` weighs (1,0,0) even when it matches through `p`
        let css = "p:is(#zz, p) { color: rgb(1, 1, 1); } p.y.y { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((1, 1, 1)));
        let css = ":where(#b) { color: rgb(1, 1, 1); } p { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((2, 2, 2)));
    }

    /// A selector list with an INVALID member is an invalid selector, and the
    /// whole style rule is dropped (Selectors 4 §4.1, what Chromium does); a
    /// member that is valid but never matches (`::before`, `:focus-visible`)
    /// leaves its siblings alone. Pseudo-element rules are ledgered: Aether
    /// generates no `::before`/`::after` boxes yet.
    #[test]
    fn test_selector_list_validity() {
        crate::ledger::reset();
        let css = ".x::before, .y:focus-visible, .y { color: rgb(1, 2, 3); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, css, "a"), None);
        let dump = format!("{:?}", crate::ledger::snapshot());
        assert!(dump.contains("pseudo-element-rule"), "{dump}");
        let css = ".x:-moz-ui-invalid, .y { color: rgb(1, 2, 3); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), None, "an unknown pseudo-class invalidates the rule");
        let css = ".x:is(:-moz-ui-invalid, .x), .y { color: rgb(1, 2, 3); }";
        assert_eq!(color_of(LOWER_HTML, css, "a"), Some((1, 2, 3)), ":is() is forgiving");
    }

    /// Custom properties cascade and inherit per element (css-variables 1):
    /// element-scoped overrides, fallbacks, and cycles (guaranteed-invalid).
    #[test]
    fn test_custom_properties_per_element() {
        let html = r#"<html><body><div id="o" class="t"><p id="i">x</p></div><p id="p">y</p>
            <p id="c" class="cyc">z</p></body></html>"#;
        let css = r#"
            :root { --fg: rgb(1, 2, 3); }
            .t { --fg: rgb(9, 8, 7); }
            p { color: var(--fg); }
            .cyc { --a: var(--b); --b: var(--a); color: var(--a, rgb(5, 5, 5)); }
            #o { color: var(--missing, rgb(4, 4, 4)); }
        "#;
        assert_eq!(color_of(html, css, "p"), Some((1, 2, 3)));
        assert_eq!(color_of(html, css, "i"), Some((9, 8, 7)), "inherited from the scoped override");
        assert_eq!(color_of(html, css, "c"), Some((5, 5, 5)), "a cycle falls back");
        assert_eq!(color_of(html, css, "o"), Some((4, 4, 4)));
    }

    /// `@media` is Media Queries 4 (css_core) against Aether's viewport.
    #[test]
    fn test_media_queries() {
        let html = r#"<html><body><p id="p">y</p></body></html>"#;
        let at = |q: &str, w: f32| {
            let mut tree = crate::layout::compute_layout_sized(&crate::dom::parse_html(html), w, 600.0);
            apply_css(&mut tree, &format!("@media {q} {{ p {{ color: rgb(1, 2, 3); }} }}"));
            tree.node_map.iter().any(|(id, n)| {
                n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some("p"))
                    && tree.paint_map.get(id).and_then(|p| p.color) == Some((1, 2, 3))
            })
        };
        assert!(at("screen", 800.0));
        assert!(!at("print", 800.0));
        assert!(at("screen and (min-width: 600px)", 800.0));
        assert!(!at("screen and (min-width: 1200px)", 800.0));
        assert!(at("screen and (min-width: 1200px)", 1280.0));
        assert!(at("(max-width: 900px)", 800.0));
        assert!(at("print, screen", 800.0));
        assert!(!at("(prefers-reduced-motion: reduce)", 800.0));
        assert!(at("(prefers-color-scheme: light)", 800.0));
        assert!(!at("(prefers-color-scheme: dark)", 800.0));
        assert!(at("(hover: hover)", 800.0));
        assert!(at("(orientation: landscape)", 800.0));
        assert!(!at("not all", 800.0));
        assert!(at("not print", 800.0));
        assert!(at("(400px <= width < 900px)", 800.0));
        assert!(!at("(400px <= width < 900px)", 1000.0));
        assert!(at("(width >= 40em)", 800.0));
    }

    #[test]
    fn test_viewport_units() {
        set_viewport(1000.0, 500.0);
        assert_eq!(parse_px("100vh"), Some(500.0));
        assert_eq!(parse_px("50vw"), Some(500.0));
        assert_eq!(parse_px("100dvh"), Some(500.0));
        assert_eq!(parse_px("100svh"), Some(500.0));
        assert_eq!(parse_px("100vmin"), Some(500.0));
        assert_eq!(parse_px("100vmax"), Some(1000.0));
        assert_eq!(eval_length("calc(100vh - 64px)", None), Some(436.0));
        // Units that merely contain "vw"/"vh" letters must not be misread.
        assert_eq!(parse_px("2rem"), Some(32.0));
        assert_eq!(parse_px("10px"), Some(10.0));
        set_viewport(800.0, 600.0);
    }


}
