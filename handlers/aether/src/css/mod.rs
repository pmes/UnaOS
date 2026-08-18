use crate::layout::{LayoutTree, PaintStyle};
use cssparser::{Parser, ParserInput, Token};
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};
use taffy::geometry::Rect;


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
    pub padding: Option<Rect<LengthPercentage>>,
    pub margin: Option<Rect<LengthPercentageAuto>>,
    pub position: Option<taffy::style::Position>,
    pub inset_top: Option<LengthPercentageAuto>,
    pub inset_left: Option<LengthPercentageAuto>,
    pub inset_right: Option<LengthPercentageAuto>,
    pub inset_bottom: Option<LengthPercentageAuto>,
    pub justify: Option<taffy::style::JustifyContent>,
    pub align_items: Option<taffy::style::AlignItems>,
    pub align_self: Option<taffy::style::AlignSelf>,
    pub box_sizing: Option<taffy::style::BoxSizing>,
    pub paint: PaintStyle,
}

impl SpecifiedStyle {
    /// Folds the specified layout declarations into a taffy style.
    pub(crate) fn fold_into(&self, node_style: &mut Style) {
        if let Some(d) = self.display { node_style.display = d; }
        if let Some(fd) = self.flex_direction { node_style.flex_direction = fd; }
        if let Some(w) = self.width { node_style.size.width = w; }
        if let Some(h) = self.height {
            node_style.size.height = h;
            node_style.min_size.height = h;
        }
        // min wins over max in taffy, and blocks carry a UA min-height
        // default — a specified max must clear it (an explicit min-* below
        // still overrides, it folds after).
        if let Some(v) = self.max_width {
            node_style.max_size.width = v;
            node_style.min_size.width = Dimension::auto();
        }
        if let Some(v) = self.max_height {
            node_style.max_size.height = v;
            node_style.min_size.height = Dimension::auto();
        }
        if let Some(v) = self.min_width { node_style.min_size.width = v; }
        if let Some(v) = self.min_height { node_style.min_size.height = v; }
        if let Some(p) = self.padding {
            node_style.padding = p;
        }
        if let Some(m) = self.margin {
            node_style.margin = m;
        }
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
            if matches!(
                node_style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            ) {
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
    match prop {
        "display" => match value {
            "none" => style.display = Some(Display::None),
            // Column-flex approximations of block-ish display types.
            "flex" | "block" | "inline-block" | "inline-flex" | "inline" | "list-item"
            | "flow-root" | "table" | "table-cell" | "table-caption" | "table-row-group"
            | "table-header-group" | "table-footer-group"
            // Legacy/prefixed flexbox spellings are the same box type here.
            | "-webkit-box" | "-webkit-inline-box" | "-webkit-flex" | "-webkit-inline-flex"
            | "-ms-flexbox" | "-ms-inline-flexbox" | "-moz-box" => {
                style.display = Some(Display::Flex)
            }
            "table-row" => {
                style.display = Some(Display::Flex);
                style.flex_direction = Some(FlexDirection::Row);
            }
            "inherit" | "initial" | "unset" | "revert" => {}
            other => crate::ledger::record_css(&format!("display:{}", other)),
        },
        "flex-direction" => match value {
            "row" => style.flex_direction = Some(FlexDirection::Row),
            "column" => style.flex_direction = Some(FlexDirection::Column),
            _ => {}
        },
        "width" => style.width = parse_dimension_str(value),
        "height" => style.height = parse_dimension_str(value),
        "max-width" => style.max_width = parse_dimension_str(value),
        "max-height" => style.max_height = parse_dimension_str(value),
        "min-width" => style.min_width = parse_dimension_str(value),
        "min-height" => style.min_height = parse_dimension_str(value),
        // overflow hidden/clip/auto/scroll all CLIP paint here (no inner
        // scrollbars yet — clipping is the honest approximation; visible
        // overflow was smearing collapsed menus over the page).
        "overflow" | "overflow-x" | "overflow-y" => match value {
            "hidden" | "clip" | "auto" | "scroll" => style.paint.clip = Some(true),
            "visible" => style.paint.clip = Some(false),
            _ => {}
        },
        "padding" => style.padding = parse_sides(value, |v| parse_length_percentage_str(v)),
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length_percentage_str(value) {
                let mut p = style.padding.unwrap_or(Rect {
                    left: LengthPercentage::length(0.0),
                    right: LengthPercentage::length(0.0),
                    top: LengthPercentage::length(0.0),
                    bottom: LengthPercentage::length(0.0),
                });
                match prop {
                    "padding-top" => p.top = v,
                    "padding-right" => p.right = v,
                    "padding-bottom" => p.bottom = v,
                    _ => p.left = v,
                }
                style.padding = Some(p);
            }
        }
        "margin" => style.margin = parse_sides(value, |v| parse_length_percentage_auto_str(v)),
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length_percentage_auto_str(value) {
                let mut m = style.margin.unwrap_or(Rect {
                    left: LengthPercentageAuto::length(0.0),
                    right: LengthPercentageAuto::length(0.0),
                    top: LengthPercentageAuto::length(0.0),
                    bottom: LengthPercentageAuto::length(0.0),
                });
                match prop {
                    "margin-top" => m.top = v,
                    "margin-right" => m.right = v,
                    "margin-bottom" => m.bottom = v,
                    _ => m.left = v,
                }
                style.margin = Some(m);
            }
        }
        "position" => match value {
            "absolute" | "fixed" => style.position = Some(taffy::style::Position::Absolute),
            "static" | "relative" | "sticky" => style.position = Some(taffy::style::Position::Relative),
            other => crate::ledger::record_css(&format!("position:{}", other)),
        },
        "top" => style.inset_top = parse_length_percentage_auto_str(value),
        "left" => style.inset_left = parse_length_percentage_auto_str(value),
        "right" => style.inset_right = parse_length_percentage_auto_str(value),
        "bottom" => style.inset_bottom = parse_length_percentage_auto_str(value),
        // Float approximation: no real float layout (text does not wrap
        // around the box), but a floated box sizes to content and hugs its
        // edge instead of stretching full width.
        "float" => match value {
            "left" => {
                style.width = Some(Dimension::auto());
                style.align_self = Some(taffy::style::AlignSelf::START);
            }
            "right" => {
                style.width = Some(Dimension::auto());
                style.align_self = Some(taffy::style::AlignSelf::END);
            }
            "none" | "inherit" | "initial" | "unset" => {}
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
            "right" | "end" => {
                style.justify = Some(taffy::style::JustifyContent::END);
                style.paint.text_align = Some(2);
            }
            "left" | "start" | "justify" => {
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
            if !is_neutral_keyword(value) {
                match parse_color_str(value) {
                    Some(c) => style.paint.background = Some(c),
                    None => crate::ledger::record_css(&format!("background-value:{}", clip(value))),
                }
            }
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
                    got_color = true;
                    break;
                }
            }
            // Function colors with spaces (rgb(1, 2, 3)) survive as whole-value.
            if !got_color && !is_neutral_keyword(value) && style.paint.bg_image.is_none() {
                match parse_color_str(value) {
                    Some(c) => style.paint.background = Some(c),
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
        "font-family" => {
            let v = value.to_ascii_lowercase();
            let fam = if v.contains("monospace") || v.contains("courier") || v.contains("menlo") || v.contains("consolas") || v.contains("mono") {
                2
            } else if v.contains("sans") {
                0
            } else if v.contains("serif") || v.contains("georgia") || v.contains("times") || v.contains("libertine") {
                1
            } else {
                0
            };
            style.paint.family = Some(fam);
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
                } else if h == "normal" || h == "small-caps" || h.starts_with("ultra")
                    || h.starts_with("extra") || h == "condensed" || h == "expanded"
                    || h == "semi-condensed" || h == "semi-expanded"
                {
                    // no effect here, but still part of the prefix
                } else if let Some(b) = parse_font_weight(&h) {
                    style.paint.bold = Some(b);
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
            Some(px) => style.paint.font_size = Some(px),
            None => crate::ledger::record_css(&format!("font-size-value:{}", clip(value))),
        },
        "font-weight" => match parse_font_weight(value) {
            Some(b) => style.paint.bold = Some(b),
            None => crate::ledger::record_css(&format!("font-weight-value:{}", clip(value))),
        },
        "visibility" => match value {
            "hidden" | "collapse" => style.paint.hidden = Some(true),
            "visible" => style.paint.hidden = Some(false),
            "inherit" | "initial" | "unset" => {}
            other => crate::ledger::record_css(&format!("visibility:{}", other)),
        },
        // Only full transparency is honored (no compositing): opacity:0
        // hides like visibility:hidden; anything else paints normally.
        "opacity" => {
            // Always specified, both ways: a higher-specificity opacity:1
            // has to be able to un-hide what an earlier opacity:0 rule hid,
            // and that only works if the winning declaration merges a value
            // instead of leaving the field unspecified.
            if let Ok(a) = value.parse::<f32>() {
                style.paint.hidden = Some(a == 0.0);
            }
        }
        "line-height" => {
            let v = value.trim();
            let factor = if let Some(px) = v.strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(px / 16.0)
            } else if let Some(n) = v
                .strip_suffix("rem")
                .or_else(|| v.strip_suffix("em"))
                .and_then(|n| n.trim().parse::<f32>().ok())
            {
                // 1.5rem = 24px against the 16px base = 1.5x (approximation:
                // em treated like rem, not parent-relative).
                Some(n)
            } else if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(pct / 100.0)
            } else if v == "normal" {
                Some(1.2)
            } else {
                v.parse::<f32>().ok()
            };
            match factor {
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
            for part in v.split_whitespace() {
                if let Some(px) = parse_px(part).filter(|_| part.ends_with("px") || part.ends_with("em")) {
                    width = px;
                    got_any = true;
                } else if matches!(part, "solid" | "dotted" | "dashed" | "double" | "groove" | "ridge" | "inset" | "outset") {
                    got_any = true;
                } else if let Some(c) = parse_color_str(part) {
                    color = c;
                    got_any = true;
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
            }
        }
        "white-space" => match value {
            "nowrap" | "pre" => style.paint.nowrap = Some(true),
            "normal" | "pre-wrap" | "pre-line" | "break-spaces" => style.paint.nowrap = Some(false),
            _ => {}
        },
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
        "border-radius" | "border-top-left-radius" | "border-top-right-radius"
        | "border-bottom-left-radius" | "border-bottom-right-radius" => {} // silently ignore; taffy limitation
        "border" | "outline" => {
            let v = value.trim();
            if v == "none" || v == "0" {
                style.paint.border = Some([None; 4]);
                return;
            }
            let mut width = 1.0f32;
            let mut color = (128, 128, 128);
            let mut got_any = false;
            for part in v.split_whitespace() {
                if let Some(px) = parse_px(part).filter(|_| part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '.')) {
                    width = px;
                    got_any = true;
                } else if matches!(part, "solid" | "dotted" | "dashed" | "double" | "groove" | "ridge" | "inset" | "outset") {
                    got_any = true;
                } else if let Some(c) = parse_color_str(part) {
                    color = c;
                    got_any = true;
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
                            | "flex-wrap" | "opacity" | "border-radius"
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
    /// Custom properties (--x: value) gathered from the current page's
    /// sheets, last-wins. A flat global map — no per-element cascade of
    /// custom props yet (the dominant design-token-on-:root pattern works;
    /// element-scoped overrides don't and stay visible in the ledger).
    static CUSTOM_PROPS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Scans CSS text for `--name: value` declarations in DOCUMENT-SCOPED
/// rules only (:root/html/body/*). Theme-variant rules like
/// `.dark-mode { --bg: black }` must NOT poison the flat token map —
/// that class isn't on the document (a flat map has no element scope;
/// scoped redefinitions stay honest gaps).
fn collect_custom_props(css: &str, map: &mut std::collections::HashMap<String, String>) {
    let mut i = 0;
    while let Some(open) = css[i..].find('{') {
        let open = i + open;
        // Prelude: back to the previous '}' , ';', or start.
        let prelude_start = css[..open].rfind(['}', ';', '{']).map(|p| p + 1).unwrap_or(0);
        let prelude = css[prelude_start..open].trim();
        let Some(close) = css[open..].find('}') else { break };
        let close = open + close;
        let body = &css[open + 1..close];
        i = close + 1;
        let doc_scoped = !prelude.is_empty()
            && prelude.split(',').any(|s| matches!(s.trim(), ":root" | "html" | "body" | "*" | "html body"));
        if !doc_scoped || !body.contains("--") {
            continue;
        }
        for decl in split_declarations(body) {
            let Some((name, value)) = split_declaration(decl) else { continue };
            let name = name.trim();
            let value = value.trim();
            if name.starts_with("--")
                && name.len() > 2
                && name[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !value.is_empty()
                && value.len() < 512
            {
                map.insert(name.to_string(), value.to_string());
            }
        }
    }
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

/// One applicable rule: a single compiled selector with its declarations.
/// A rule with `!important` declarations is split in two — the important
/// half carries `important: true` and applies in a later cascade tier.
struct Rule {
    selector: kuchiki::Selector,
    style: std::rc::Rc<SpecifiedStyle>,
    important: bool,
    /// Cascade weight. Normally the compiled selector's own specificity;
    /// a selector recovered by lowering carries the specificity of the
    /// ORIGINAL construct instead (`:where()`'s arguments weigh nothing).
    specificity: kuchiki::Specificity,
    key: RuleKey,
    plan: MatchPlan,
    /// Every depth-0 `.class`/`#id` in the selector: ALL must exist
    /// somewhere on the page for the rule to possibly match. Real pages
    /// ship swaths of feature-flag rules whose flag class is absent —
    /// they drop before any matching.
    required: Vec<(bool, String)>, // (is_id, name)
}

/// How a rule matches. A single compound of tag/id/class simple selectors
/// (no combinators, no pseudo/attr) — the overwhelming majority on real
/// pages — matches with precomputed per-element sets; everything else goes
/// through the real selector engine. The profiled wall was kuchiki's
/// has_class re-splitting class strings during ancestor walks.
enum MatchPlan {
    Simple {
        tag: Option<String>,
        id: Option<String>,
        classes: Vec<String>,
    },
    Engine,
}

/// Collects the depth-0 class/id tokens a selector REQUIRES on the page.
/// Tokens inside parens (:not(.x), :is(...)) or brackets are skipped —
/// they are not unconditional requirements.
fn required_tokens(selector_text: &str) -> Vec<(bool, String)> {
    let s = selector_text;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            b'\\' => i += 1, // escaped char is literal, never a marker
            m @ (b'.' | b'#') if depth == 0 => {
                // An escaped character inside an identifier (`.md\:block`,
                // `.w-1\/2`) is part of the NAME; consume it and record the
                // unescaped token the DOM actually carries.
                let start = i + 1;
                let mut j = start;
                let mut name = String::new();
                while j < bytes.len() {
                    let c = bytes[j] as char;
                    if c == '\\' && j + 1 < bytes.len() {
                        // Only single-character escapes are handled; a hex
                        // escape (`\3a `) would need the full CSS grammar.
                        let n = bytes[j + 1] as char;
                        if n.is_ascii_hexdigit() {
                            name.clear();
                            break;
                        }
                        name.push(n);
                        j += 2;
                    } else if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        name.push(c);
                        j += 1;
                    } else {
                        break;
                    }
                }
                if !name.is_empty() {
                    out.push((m == b'#', name));
                }
                i = j.max(start);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Derives the MatchPlan from a single selector's text.
fn match_plan(selector_text: &str) -> MatchPlan {
    let s = selector_text.trim();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ' ' | '\t' | '>' | '+' | '~' if depth == 0 => return MatchPlan::Engine,
            ':' | '[' if depth == 0 => return MatchPlan::Engine,
            _ => {}
        }
    }
    if s.contains(':') || s.contains('[') {
        return MatchPlan::Engine;
    }
    // Compound of [tag]?(#id|.class)*
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut rest = s;
    // Leading tag or universal.
    let head_end = rest.find(['.', '#']).unwrap_or(rest.len());
    let head = &rest[..head_end];
    if !head.is_empty() && head != "*" {
        if !head.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return MatchPlan::Engine;
        }
        tag = Some(head.to_ascii_lowercase());
    }
    rest = &rest[head_end..];
    while !rest.is_empty() {
        let marker = rest.as_bytes()[0];
        let body = &rest[1..];
        let end = body.find(['.', '#']).unwrap_or(body.len());
        let name = &body[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return MatchPlan::Engine;
        }
        match marker {
            b'.' => classes.push(name.to_string()),
            b'#' => id = Some(name.to_string()),
            _ => return MatchPlan::Engine,
        }
        rest = &body[end..];
    }
    MatchPlan::Simple { tag, id, classes }
}

/// Rule-hash key: the rightmost compound's most selective simple selector.
/// An element only needs to test rules whose key it can possibly satisfy —
/// the standard cascade optimization (matching was the profiled hot path).
#[derive(Debug, PartialEq)]
enum RuleKey {
    Id(String),
    Class(String),
    Tag(String),
    Universal,
}

/// Derives the RuleKey from a single selector's text. Conservative: any
/// pseudo/attr syntax in the rightmost compound falls back to Universal
/// (always tested) rather than risking a wrong bucket.
fn rule_key(selector_text: &str) -> RuleKey {
    let s = selector_text.trim();
    // The rightmost compound, honouring CSS identifier escapes: `\:` and
    // `\[` inside a class name (`.md\:block`, `.w-1\/2`) are ORDINARY
    // characters, not the start of a pseudo-class or attribute selector.
    // Reading them as syntax bucketed every responsive/state utility class
    // under a truncated key no element could ever carry, so the rule was
    // never even tested — the whole `md:`/`lg:` layer of a utility-CSS page
    // silently did nothing.
    let cut = |hay: &str, stops: &[char]| -> usize {
        let (mut depth, mut esc) = (0i32, false);
        for (i, c) in hay.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            match c {
                '\\' => esc = true,
                // Stop test comes FIRST: `[` is itself a stop character for
                // the pseudo/attr cut, so it must not open a group before
                // being recognised (`input[type=text]` keys on `input`).
                c if depth <= 0 && stops.contains(&c) => return i,
                '[' | '(' => depth += 1,
                ']' | ')' => depth -= 1,
                _ => {}
            }
        }
        hay.len()
    };
    // Last unescaped depth-0 combinator starts the rightmost compound.
    let mut start = 0;
    {
        let (mut depth, mut esc) = (0i32, false);
        for (i, c) in s.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            match c {
                '\\' => esc = true,
                '[' | '(' => depth += 1,
                ']' | ')' => depth -= 1,
                ' ' | '\t' | '>' | '+' | '~' if depth <= 0 => start = i + c.len_utf8(),
                _ => {}
            }
        }
    }
    let comp = s[start..].trim();
    // Pseudo-classes and attribute selectors only NARROW a compound, so the
    // simple-selector prefix before the first ':'/'[' is still a valid
    // bucket key ("li:first-child" can only match an <li>). A compound that
    // STARTS with ':'/'[' gives no such guarantee → Universal.
    let comp = &comp[..cut(comp, &[':', '['])];
    // Rightmost unescaped '.' / '#' marker in the compound.
    let (mut last_dot, mut last_hash, mut first_marker) = (None, None, None);
    {
        let mut esc = false;
        for (i, c) in comp.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            match c {
                '\\' => esc = true,
                '.' | '#' => {
                    if first_marker.is_none() {
                        first_marker = Some(i);
                    }
                    if c == '.' {
                        last_dot = Some(i);
                    } else {
                        last_hash = Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    // Strips the backslashes from an escaped identifier so the key matches
    // the class/id string the DOM actually carries.
    let unescape = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        let mut esc = false;
        for c in s.chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    };
    if let Some(p) = last_dot {
        let rest = &comp[p + 1..];
        let name = unescape(&rest[..cut(rest, &['.', '#'])]);
        if !name.is_empty() {
            return RuleKey::Class(name);
        }
    }
    if let Some(p) = last_hash {
        let rest = &comp[p + 1..];
        let name = unescape(&rest[..cut(rest, &['.', '#'])]);
        if !name.is_empty() {
            return RuleKey::Id(name);
        }
    }
    let end = first_marker.unwrap_or(comp.len());
    let tag = comp[..end].trim();
    if tag.is_empty() || tag == "*" {
        RuleKey::Universal
    } else {
        RuleKey::Tag(tag.to_ascii_lowercase())
    }
}

/// Splits a selector list on the commas that actually separate selectors:
/// a comma inside `:is(...)`, `[attr="a,b"]`, or any function is part of
/// one selector, not a separator.
pub(crate) fn split_selector_list(prelude: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut quote, mut escaped) = (0i32, 0usize, None::<char>, false);
    for (i, c) in prelude.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            q if Some(q) == quote => quote = None,
            _ if quote.is_some() => {}
            '"' | '\'' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth <= 0 => {
                out.push(prelude[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(prelude[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

// ---------------------------------------------------------------------
// Selector lowering (rewrite-before-compile salvage)
//
// Servo's selector parser is a 2019-era snapshot: `:is()`, `:where()`,
// `:has()`, `:focus-visible`, `:focus-within`, `:not()` with an argument
// LIST, every pseudo-element, and most form-state pseudo-classes are all
// rejected outright — and a rejected selector drops the WHOLE RULE, not one
// declaration. On a modern component/utility stylesheet that is hundreds of
// live rules lost with nothing on screen to show for it.
//
// The salvage runs ONLY after a member has already failed to compile, so
// the fast path (whole list compiles first try) pays nothing. Each failing
// member is rewritten to a semantically equivalent form the parser does
// accept; whatever survives is compiled, and whatever cannot be rewritten
// is ledgered under a NAMED bucket so every dropped rule has a reason.
// ---------------------------------------------------------------------

/// Pseudo-classes naming a dynamic state this engine never enters: there is
/// no pointer, no focus, and no navigation target in a static render.
///
/// `:hover`/`:active`/`:focus`/`:visited` are the interesting precedent —
/// servo's parser *accepts* them, so today they compile into rules that
/// simply never match (`engine_tests::test_hover_focus_not_applied` pins
/// exactly that). The names below are their unparseable siblings; dropping
/// such a member reproduces the same observable behaviour instead of
/// throwing away the rest of the selector list along with it.
const DYNAMIC_STATE_PSEUDOS: &[&str] = &[
    "hover", "active", "focus", "focus-visible", "focus-within", "focus-ring",
    "visited", "target", "target-within", "user-invalid", "user-valid",
    "autofill", "-webkit-autofill", "-moz-focusring", "-moz-drag-over",
    "-moz-focus-inner", "modal", "popover-open", "fullscreen", "open",
    "picture-in-picture", "playing", "paused", "current", "past", "future",
    "local-link", "muted", "seeking", "stalled", "buffering",
];

/// Form/validity pseudo-classes whose state we do not model. (`:required`
/// and `:optional` are deliberately absent — they lower to an attribute
/// test; `:checked`/`:disabled`/`:enabled`/`:indeterminate` already parse.)
const FORM_STATE_PSEUDOS: &[&str] = &[
    "valid", "invalid", "in-range", "out-of-range", "read-only", "read-write",
    "placeholder-shown", "default", "blank", "user-error",
];

/// Pseudo-ELEMENTS that predate the `::` spelling, including the vendor
/// placeholder spellings every reset sheet still ships. A rule targeting
/// any pseudo-element styles a generated box we never create, so it can
/// never match our tree — that is a known, quiet non-application, not a
/// parse defect, and it shares the existing `pseudo-element-rule` key.
const LEGACY_PSEUDO_ELEMENTS: &[&str] = &[
    "before", "after", "first-line", "first-letter",
    "-ms-input-placeholder", "-moz-placeholder", "-webkit-input-placeholder",
    "-ms-clear", "-ms-reveal", "-ms-expand", "-ms-check",
];

/// Vendor prefixes. An unrecognised `:-vendor-thing` pseudo-CLASS is always
/// some UI or media state (`:-moz-ui-invalid`, `:-ms-fullscreen`,
/// `:-o-prefocus`) that a static render never enters, so it gets its own
/// named bucket rather than the residual compile-failure key.
const VENDOR_PREFIXES: &[&str] = &["-webkit-", "-moz-", "-ms-", "-o-"];

/// Functional pseudo-classes that are pure selector GROUPING: the element
/// matches if it matches any argument. Includes the legacy spellings that
/// shipped before `:is()` was standardised.
const GROUPING_PSEUDOS: &[&str] =
    &["is", "where", "matches", "any", "-moz-any", "-webkit-any"];

/// Byte index of the `)` matching the `(` at `open`.
fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0i32, open);
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Locates the first `:name(...)` at nesting depth 0 whose name is listed.
/// Returns `(colon index, lowercased name, open paren, close paren)`.
fn find_functional(s: &str, names: &[&str]) -> Option<(usize, String, usize, usize)> {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b':' if depth == 0 => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
                    j += 1;
                }
                if j > start && j < b.len() && b[j] == b'(' {
                    let name = s[start..j].to_ascii_lowercase();
                    if names.iter().any(|n| *n == name) {
                        if let Some(close) = matching_paren(s, j) {
                            return Some((i, name, j, close));
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Every pseudo token at nesting depth 0: `(is_pseudo_element, name)`.
/// Depth 0 only, deliberately — a `:focus-visible` inside `:is(...)` kills
/// only its own branch, and is seen after the group is distributed.
fn depth0_pseudos(s: &str) -> Vec<(bool, String)> {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    let mut quote: Option<u8> = None;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b':' if depth == 0 => {
                let mut start = i + 1;
                let is_element = start < b.len() && b[start] == b':';
                if is_element {
                    start += 1;
                }
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
                    j += 1;
                }
                if j > start {
                    out.push((is_element, s[start..j].to_ascii_lowercase()));
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// True when the selector has a depth-0 combinator (i.e. is not a single
/// compound). Used to decide whether a group argument can be substituted.
fn is_complex(s: &str) -> bool {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b' ' | b'\t' | b'\n' | b'>' | b'+' | b'~' if depth == 0 => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// True when the span `[before, after)` is a whole compound on its own —
/// i.e. what surrounds it is a combinator, a comma, or nothing.
fn is_whole_compound(s: &str, before: usize, after: usize) -> bool {
    let edge = |c: Option<char>| {
        matches!(c, None | Some(' ') | Some('\t') | Some('\n') | Some('>') | Some('+') | Some('~') | Some(','))
    };
    edge(s[..before].chars().next_back()) && edge(s[after..].chars().next())
}

/// Classifies a member that no rewrite can save, returning its ledger
/// bucket. Only depth-0 syntax is considered.
fn unmatchable_bucket(sel: &str) -> Option<&'static str> {
    if sel.contains('&') {
        return Some("selector-nesting");
    }
    for (is_element, name) in depth0_pseudos(sel) {
        if is_element || LEGACY_PSEUDO_ELEMENTS.contains(&name.as_str()) {
            // Vendor pseudo-elements (`::-webkit-input-placeholder`,
            // `::-moz-focus-inner`, `::-webkit-scrollbar`, …) land here too:
            // a box we never generate, so the rule can never apply.
            return Some("pseudo-element-rule");
        }
        match name.as_str() {
            // Real work, a later arc: `:has()` needs forward matching.
            "has" => return Some("selector-has"),
            "host" | "host-context" | "defined" | "part" | "slotted" => {
                return Some("selector-shadow-dom")
            }
            "lang" | "dir" => return Some("selector-lang-dir"),
            n if DYNAMIC_STATE_PSEUDOS.contains(&n) => return Some("selector-dynamic-state"),
            n if FORM_STATE_PSEUDOS.contains(&n) => return Some("selector-form-state"),
            // `:-moz-any()`/`:-webkit-any()` are vendor spellings of a
            // GROUP, not a state — they lower, so never bucket them here.
            n if GROUPING_PSEUDOS.contains(&n) => {}
            n if VENDOR_PREFIXES.iter().any(|p| n.starts_with(p)) => {
                return Some("selector-vendor-pseudo")
            }
            _ => {}
        }
    }
    if sel.contains(":nth") && sel.contains(" of ") {
        return Some("selector-nth-of");
    }
    // `:not(X)` where X is something we cannot EVALUATE (as opposed to
    // something that never matches) makes the whole negation unknown —
    // report X's bucket rather than silently over-matching by dropping it.
    let mut rest = sel;
    while let Some((_, _, open, close)) = find_functional(rest, &["not"]) {
        for arg in split_selector_list(&rest[open + 1..close]) {
            if never_matches(arg) {
                continue; // a tautology; `rewrite_not` deletes it
            }
            match unmatchable_bucket(arg) {
                Some(b) => return Some(b),
                None => {}
            }
        }
        rest = &rest[close + 1..];
    }
    None
}

/// True when this argument of a `:not()` can never match anything we
/// render, which makes the negation unconditionally true.
fn never_matches(arg: &str) -> bool {
    matches!(
        unmatchable_bucket(arg),
        Some("pseudo-element-rule") | Some("selector-dynamic-state")
            | Some("selector-form-state") | Some("selector-shadow-dom")
            | Some("selector-vendor-pseudo")
    )
}

/// Rewrites one depth-0 `:not(...)`: an argument LIST becomes a chain of
/// single-argument negations (`:not(a, b)` ≡ `:not(a):not(b)`, and servo
/// only parses the latter), and arguments that can never match are dropped
/// because negating them is a tautology (`.x:not(:focus-visible)` styles
/// `.x` in a render that never focuses anything).
fn rewrite_not(text: &str) -> Option<String> {
    let (colon, _, open, close) = find_functional(text, &["not"])?;
    let args = split_selector_list(&text[open + 1..close]);
    let kept: Vec<&str> = args.iter().copied().filter(|a| !never_matches(a)).collect();
    let replacement = if kept.is_empty() {
        if is_whole_compound(text, colon, close + 1) { "*".to_string() } else { String::new() }
    } else {
        kept.iter().map(|a| format!(":not({a})")).collect::<Vec<_>>().join("")
    };
    let out = format!("{}{}{}", &text[..colon], replacement, &text[close + 1..]);
    (out != text).then_some(out)
}

/// A candidate selector plus the selector whose specificity the cascade
/// should charge it. The two differ only where `:where()` was distributed:
/// `:where()`'s arguments contribute NOTHING to specificity, so the twin
/// keeps the same shape with the argument replaced by a hole. Compiling
/// the twin and reading its specificity is exact for the common case.
type Candidate = (String, String);

/// Distributes the first depth-0 grouping pseudo over its arguments:
/// `a :is(b, c)` -> `a b`, `a c`.
///
/// An argument that is itself complex can only be substituted when the
/// group stands alone as a whole compound: `a :is(b > c)` -> `a b > c` is
/// sound, `a.x:is(b > c)` is not (it would need the compound folded onto
/// the argument's subject), so that argument is dropped and ledgered.
///
/// Specificity: for `:where()` the twin loses the argument entirely, which
/// is exact. For `:is()` each branch is charged its OWN specificity rather
/// than the spec's "max over all arguments" — a documented approximation
/// that can only under-charge a branch, and one that keeps every rule
/// alive. Dropped rules are worse than slightly-wrong specificity.
fn distribute_grouping(text: &str, spec: &str) -> Option<Vec<Candidate>> {
    let (colon, name, open, close) = find_functional(text, GROUPING_PSEUDOS)?;
    // The twin has had the SAME rewrites applied, so its first depth-0
    // group is the same construct — unless an argument we already spliced
    // in carried a nested group, which the twin dropped. Detect that and
    // fall back to approximating the specificity with the rewritten form.
    let twin = find_functional(spec, GROUPING_PSEUDOS)
        .filter(|(_, n, _, _)| *n == name);
    let whole = is_whole_compound(text, colon, close + 1);
    let (pre, suf) = (&text[..colon], &text[close + 1..]);
    let mut out = Vec::new();
    for arg in split_selector_list(&text[open + 1..close]) {
        if is_complex(arg) && !whole {
            crate::ledger::record_css("selector-group-complex-arg");
            continue;
        }
        let matched = format!("{pre}{arg}{suf}");
        let twin_text = match &twin {
            Some((tc, _, _, tclose)) => {
                let fill = if name == "where" {
                    if whole { "*" } else { "" }
                } else {
                    arg
                };
                format!("{}{}{}", &spec[..*tc], fill, &spec[tclose + 1..])
            }
            None => {
                if name == "where" {
                    crate::ledger::record_css("selector-where-specificity-approx");
                }
                matched.clone()
            }
        };
        out.push((matched, twin_text));
    }
    Some(out)
}

/// Form pseudo-classes that are really attribute tests in disguise.
fn rewrite_form_shorthand(text: &str) -> Option<String> {
    for (from, to) in [(":required", "[required]"), (":optional", ":not([required])")] {
        if text.contains(from) {
            return Some(text.replace(from, to));
        }
    }
    None
}

/// One rewrite step on a candidate, cheapest first. `None` = nothing left
/// to try.
fn rewrite_once((text, spec): &Candidate) -> Option<Vec<Candidate>> {
    if let Some(t) = rewrite_not(text) {
        let s = rewrite_not(spec).unwrap_or_else(|| t.clone());
        return Some(vec![(t, s)]);
    }
    if let Some(v) = distribute_grouping(text, spec) {
        return Some(v);
    }
    if let Some(t) = rewrite_form_shorthand(text) {
        let s = rewrite_form_shorthand(spec).unwrap_or_else(|| t.clone());
        return Some(vec![(t, s)]);
    }
    None
}

/// Salvages one selector-list member servo rejected, pushing whatever
/// compiles onto `out` with the specificity the cascade should charge it.
fn lower_member(part: &str, out: &mut Vec<(kuchiki::Selector, kuchiki::Specificity)>) {
    // Bounded: a pathological nest of groups must not explode. Real pages
    // sit far under this (`:is()` lists are short and rarely nested).
    const BUDGET: usize = 48;
    let mut work: Vec<Candidate> = vec![(part.to_string(), part.to_string())];
    let mut done: Vec<Candidate> = Vec::new();
    let mut steps = 0usize;
    while let Some(cand) = work.pop() {
        steps += 1;
        if steps > BUDGET || work.len() + done.len() > BUDGET {
            crate::ledger::record_css("selector-lower-budget");
            break;
        }
        if kuchiki::Selectors::compile(&cand.0).is_ok() {
            done.push(cand);
            continue;
        }
        if let Some(bucket) = unmatchable_bucket(&cand.0) {
            crate::ledger::record_css(bucket);
            continue;
        }
        match rewrite_once(&cand) {
            Some(next) => work.extend(next),
            None => crate::ledger::record_css(&format!(
                "selector-compile-failed:{}",
                clip_n(&cand.0, 96)
            )),
        }
    }
    if !done.is_empty() {
        // Telemetry: this rule is only alive because it was rewritten, and
        // its match set is equivalent-modulo-the-approximations documented
        // on `distribute_grouping`. One record per rescued member.
        crate::ledger::record_css("selector-lowered");
    }
    for (text, spec) in done {
        let Ok(sels) = kuchiki::Selectors::compile(&text) else { continue };
        let charged = kuchiki::Selectors::compile(&spec)
            .ok()
            .and_then(|s| s.0.first().map(|sel| sel.specificity()));
        for sel in sels.0 {
            let s = charged.unwrap_or_else(|| sel.specificity());
            out.push((sel, s));
        }
    }
}

/// Compiles a selector list, keeping every selector the engine accepts,
/// paired with the specificity the cascade should charge it.
///
/// Servo's selector parser rejects the whole list when ANY member uses
/// syntax it doesn't implement. Component CSS (and every Tailwind build)
/// ships long comma lists where one `:focus-visible` or `:has()` member
/// would otherwise discard the declarations for all its siblings — on a
/// typical shell that is hundreds of live rules lost, which is why whole
/// headers and nav bars never got their layout properties. Recompiling
/// member-by-member keeps the supported ones; lowering (above) recovers
/// most of the rest; only what is left is ledgered.
fn compile_selector_list(prelude: &str) -> Vec<(kuchiki::Selector, kuchiki::Specificity)> {
    let mut out = Vec::new();
    // Fast path: the whole list parses, no salvage machinery runs at all.
    if let Ok(s) = kuchiki::Selectors::compile(prelude) {
        for sel in s.0 {
            let spec = sel.specificity();
            out.push((sel, spec));
        }
        return out;
    }
    for part in split_selector_list(prelude) {
        match kuchiki::Selectors::compile(part) {
            Ok(s) => {
                for sel in s.0 {
                    let spec = sel.specificity();
                    out.push((sel, spec));
                }
            }
            Err(_) => lower_member(part, &mut out),
        }
    }
    out
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
fn merge_paint(dst: &mut PaintStyle, src: &PaintStyle) {
    macro_rules! copy {
        ($($f:ident),* $(,)?) => { $( if src.$f.is_some() { dst.$f = src.$f; } )* };
    }
    macro_rules! clone {
        ($($f:ident),* $(,)?) => { $( if src.$f.is_some() { dst.$f = src.$f.clone(); } )* };
    }
    copy!(
        background, color, font_size, bold, border, line_height, hidden, clip, underline,
        nowrap, family, italic, text_transform, border_width, bg_repeat, text_hidden,
        mask_repeat, text_align,
    );
    clone!(bg_image, bg_size, bg_position, mask_image, mask_size, mask_position);
}

/// Applies a set of stylesheets as ONE cascade — rules from every sheet
/// sort together by (importance, specificity, source order). Inline
/// `style=""` declarations slot in at their CSS priority: above all
/// normal rules, below `!important` rules; inline `!important` wins all.
pub fn apply_stylesheets(layout_tree: &mut LayoutTree, sheets: &[String]) {
    let vw = layout_tree.viewport.0;
    set_viewport(vw, layout_tree.viewport.1);
    // Custom properties first, so every rule's var() can resolve.
    CUSTOM_PROPS.with(|m| {
        let mut map = m.borrow_mut();
        map.clear();
        for css in sheets {
            collect_custom_props(css, &mut map);
        }
    });
    let mut rules = Vec::new();
    for css in sheets {
        collect_rules(css, 0, &mut rules, vw);
    }
    rules.sort_by(|a, b| {
        a.important
            .cmp(&b.important)
            .then(a.specificity.cmp(&b.specificity))
    });

    // Element refs computed ONCE — matching is O(rules × elements) either
    // way, but the per-pair node_map lookup + ref construction was the
    // hot-path churn on rule-heavy pages.
    // Element refs + local match facts (tag/id/classes) computed ONCE:
    // simple compound rules match on these sets directly, bypassing the
    // selector engine (whose has_class re-split class strings on every
    // test — the profiled hot path).
    struct ElemInfo {
        el_ref: kuchiki::NodeDataRef<kuchiki::ElementData>,
        tag: String,
        id_attr: Option<String>,
        classes: std::collections::HashSet<String>,
    }
    let elements: Vec<(taffy::NodeId, ElemInfo)> = layout_tree
        .node_map
        .iter()
        .filter_map(|(id, n)| n.clone().into_element_ref().map(|el| (*id, el)))
        .map(|(id, el_ref)| {
            let attrs = el_ref.attributes.borrow();
            let info = ElemInfo {
                tag: el_ref.name.local.as_ref().to_ascii_lowercase(),
                id_attr: attrs.get("id").map(str::to_string),
                classes: attrs
                    .get("class")
                    .map(|c| c.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default(),
                el_ref: { drop(attrs); el_ref },
            };
            (id, info)
        })
        .collect();

    // Inline style="" tiers, parsed once per node.
    let mut inline_tiers: Vec<(taffy::NodeId, SpecifiedStyle, SpecifiedStyle, bool)> = Vec::new();
    for (node_id, info) in &elements {
        let Some(inline) = info.el_ref.attributes.borrow().get("style").map(|s| s.to_string()) else { continue };
        let (normal, important, has_important) = parse_declaration_block_tiers(&inline);
        inline_tiers.push((*node_id, normal, important, has_important));
    }

    // Accumulate each node's winning declarations in cascade order, then
    // apply ONCE per node — one taffy style clone/set per styled node
    // instead of one per matching rule.
    let mut acc: std::collections::HashMap<taffy::NodeId, SpecifiedStyle> =
        std::collections::HashMap::new();
    // Page-level presence sets for the requirement filter.
    let mut page_classes: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut page_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (_, info) in &elements {
        for c in &info.classes {
            page_classes.insert(c.as_str());
        }
        if let Some(id) = &info.id_attr {
            page_ids.insert(id.as_str());
        }
    }

    let merge_rules = |rules: &[&Rule], acc: &mut std::collections::HashMap<taffy::NodeId, SpecifiedStyle>| {
        // Drop rules requiring a class/id absent from the entire page.
        let rules: Vec<&Rule> = rules
            .iter()
            .filter(|r| {
                r.required.iter().all(|(is_id, name)| {
                    if *is_id { page_ids.contains(name.as_str()) } else { page_classes.contains(name.as_str()) }
                })
            })
            .copied()
            .collect();
        let rules = &rules[..];
        // Rule hash: an element only tests rules whose rightmost key it
        // carries (plus Universal), in original (cascade) index order.
        use std::collections::HashMap;
        let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut by_class: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut by_tag: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut universal: Vec<usize> = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            match &rule.key {
                RuleKey::Id(k) => by_id.entry(k).or_default().push(i),
                RuleKey::Class(k) => by_class.entry(k).or_default().push(i),
                RuleKey::Tag(k) => by_tag.entry(k).or_default().push(i),
                RuleKey::Universal => universal.push(i),
            }
        }
        let mut candidates: Vec<usize> = Vec::new();
        for (node_id, info) in &elements {
            candidates.clear();
            candidates.extend_from_slice(&universal);
            if let Some(v) = by_tag.get(info.tag.as_str()) {
                candidates.extend_from_slice(v);
            }
            if let Some(id) = &info.id_attr {
                if let Some(v) = by_id.get(id.as_str()) {
                    candidates.extend_from_slice(v);
                }
            }
            for class in &info.classes {
                if let Some(v) = by_class.get(class.as_str()) {
                    candidates.extend_from_slice(v);
                }
            }
            candidates.sort_unstable();
            for &i in candidates.iter() {
                let matched = match &rules[i].plan {
                    MatchPlan::Simple { tag, id, classes } => {
                        tag.as_deref().map_or(true, |t| t == info.tag)
                            && id.as_deref().map_or(true, |x| info.id_attr.as_deref() == Some(x))
                            && classes.iter().all(|c| info.classes.contains(c))
                    }
                    MatchPlan::Engine => rules[i].selector.matches(&info.el_ref),
                };
                if matched {
                    merge_specified(acc.entry(*node_id).or_default(), &rules[i].style);
                }
            }
        }
    };

    let normal: Vec<&Rule> = rules.iter().filter(|r| !r.important).collect();
    let important: Vec<&Rule> = rules.iter().filter(|r| r.important).collect();

    merge_rules(&normal, &mut acc);
    for (node_id, normal_spec, _, _) in &inline_tiers {
        merge_specified(acc.entry(*node_id).or_default(), normal_spec);
    }
    merge_rules(&important, &mut acc);
    for (node_id, _, important_spec, has_important) in &inline_tiers {
        if *has_important {
            merge_specified(acc.entry(*node_id).or_default(), important_spec);
        }
    }

    let mut abs_nodes = std::collections::HashSet::new();
    let mut inset_nodes = std::collections::HashSet::new();
    for (node_id, spec) in &acc {
        apply_spec_to_node(layout_tree, *node_id, spec, &mut abs_nodes, &mut inset_nodes);
    }

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

    // set_style only marks nodes dirty; re-lay out with text measurement.
    crate::layout::remeasure(layout_tree);
}

fn collect_rules(css: &str, depth: u8, rules: &mut Vec<Rule>, vw: f32) {
    if depth > 4 {
        return; // pathological nesting guard
    }
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);

    loop {
        // Slice the raw prelude up to the next `{`.
        let start = parser.position();
        let mut saw_block = false;
        let mut at_rule: Option<String> = None;

        loop {
            let Ok(token) = parser.next() else { break };
            match token {
                Token::CurlyBracketBlock => {
                    saw_block = true;
                    break;
                }
                Token::AtKeyword(kw) => at_rule = Some(kw.as_ref().to_string()),
                _ => {}
            }
        }
        if !saw_block {
            break; // end of stylesheet
        }

        let prelude = parser.slice_from(start);
        let prelude = prelude.trim_end().trim_end_matches('{').trim().to_string();

        // Capture the raw block body so it can be recursed or skipped.
        let body = parser
            .parse_nested_block(|p| {
                let s = p.position();
                while p.next().is_ok() {}
                Ok::<String, cssparser::ParseError<'_, ()>>(p.slice_from(s).to_string())
            })
            .unwrap_or_default();

        if let Some(kw) = at_rule {
            match kw.as_str() {
                "media" => {
                    let condition = prelude.trim_start_matches("@media").trim();
                    if media_matches(condition, vw) {
                        collect_rules(&body, depth + 1, rules, vw);
                    }
                }
                "supports" => {
                    let condition = prelude.trim_start_matches("@supports").trim();
                    if supports_matches(condition) {
                        collect_rules(&body, depth + 1, rules, vw);
                    }
                }
                other => crate::ledger::record_css(&format!("at-rule:@{}", other)),
            }
            continue;
        }

        // Pseudo-element rules (::before/::after/…) style generated boxes
        // we don't create (no `content` support): known-unsupported, one
        // ledger key — not thousands of compile failures.
        if prelude.split(',').all(|sel| {
            let sel = sel.trim();
            sel.contains("::")
                || [":before", ":after", ":placeholder", ":selection", ":marker",
                    ":first-line", ":first-letter", ":backdrop"]
                    .iter()
                    .any(|p| sel.contains(p))
        }) {
            crate::ledger::record_css("pseudo-element-rule");
            continue;
        }
        // Compile with kuchiki's real selector engine (servo selectors),
        // salvaging modern syntax it rejects (see `lower_member`).
        let selectors = compile_selector_list(&prelude);
        if selectors.is_empty() {
            continue;
        }

        let (normal, important, has_important) = parse_declaration_block_tiers(&body);
        let normal = std::rc::Rc::new(normal);
        for (selector, specificity) in selectors {
            let text = selector.to_string();
            let key = rule_key(&text);
            let plan = match_plan(&text);
            let required = required_tokens(&text);
            rules.push(Rule { selector, style: normal.clone(), important: false, specificity, key, plan, required });
        }
        if has_important {
            // Selector isn't Clone; the important tier compiles its own copy.
            let important = std::rc::Rc::new(important);
            for (selector, specificity) in compile_selector_list(&prelude) {
                let text = selector.to_string();
                let key = rule_key(&text);
                let plan = match_plan(&text);
                let required = required_tokens(&text);
                rules.push(Rule { selector, style: important.clone(), important: true, specificity, key, plan, required });
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
    take!(display, flex_direction, width, height, padding, margin, position,
          inset_top, inset_left, inset_right, inset_bottom, justify, align_items, align_self,
          max_width, max_height, min_width, min_height, box_sizing);
    merge_paint(&mut dst.paint, &src.paint);
}

/// Evaluates an @media condition against the fixed viewport. Comma = OR,
/// "and" = AND. Unknown features evaluate false (and are ledgered), so a
/// query is never wrongly applied.
fn media_matches(condition: &str, vw: f32) -> bool {
    if condition.is_empty() {
        return true;
    }
    condition.split(',').any(|clause| {
        clause.split(" and ").all(|part| {
            let p = part.trim();
            // `not X` negates one clause term.
            if let Some(rest) = p.strip_prefix("not ") {
                return !media_matches(rest.trim(), vw);
            }
            let p = p.trim_start_matches('(').trim_end_matches(')').trim();
            match p {
                "screen" | "all" => true,
                "print" => false,
                _ => {
                    if let Some((feature, value)) = p.split_once(':') {
                        let value = value.trim();
                        match feature.trim() {
                            "min-width" => parse_px(value).map(|v| vw >= v).unwrap_or(false),
                            "max-width" => parse_px(value).map(|v| vw <= v).unwrap_or(false),
                            // Honest environment answers, matching the JS
                            // matchMedia prelude: light scheme, motion ok,
                            // hover-capable pointer, landscape viewport.
                            "prefers-color-scheme" => value == "light",
                            "prefers-reduced-motion" => value == "no-preference",
                            "prefers-reduced-transparency" => value == "no-preference",
                            "prefers-contrast" => value == "no-preference",
                            "hover" | "any-hover" => value == "hover",
                            "pointer" | "any-pointer" => value == "fine",
                            "orientation" => value == "landscape",
                            f => {
                                crate::ledger::record_css(&format!("media-feature:{}", f));
                                false
                            }
                        }
                    } else {
                        crate::ledger::record_css(&format!("media-condition:{}", clip(p)));
                        false
                    }
                }
            }
        })
    })
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
            | "font-family"
    )
}

/// Evaluates an @supports condition against our real capability set.
/// `not` inverts; and/or compose; unknown syntax evaluates false.
fn supports_matches(condition: &str) -> bool {
    let c = condition.trim();
    if let Some(rest) = c.strip_prefix("not ") {
        return !supports_matches(rest);
    }
    if c.contains(" or ") {
        return c.split(" or ").any(supports_matches);
    }
    if c.contains(" and ") {
        return c.split(" and ").all(supports_matches);
    }
    let inner = c.trim().trim_start_matches('(').trim_end_matches(')').trim();
    match inner.split_once(':') {
        Some((prop, _value)) => property_supported(prop.trim()),
        None => {
            crate::ledger::record_css(&format!("supports-condition:{}", clip(inner)));
            false
        }
    }
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

/// Parses a font-size string: px, em/rem (relative to the 16px UA base —
/// approximation, not parent-relative), %, or absolute keywords.
pub fn parse_font_size(value: &str) -> Option<f32> {
    let v = value.trim().to_ascii_lowercase();
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
pub fn parse_font_weight(value: &str) -> Option<bool> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        _ => v.parse::<f32>().ok().map(|n| n >= 600.0),
    }
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
    if let Some(n) = v
        .strip_suffix("rem")
        .or_else(|| v.strip_suffix("em"))
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        return Some(n * 16.0);
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
            "rem" | "em" => Some(n * 16.0),
            // `calc(100vh - 64px)` is the standard full-height idiom.
            u => parse_viewport_length(&format!("{}{}", n, u)),
        }
    }
}

/// Parses a 1-4 value box shorthand ("10px", "0 auto", "1px 2px 3px 4px")
/// into a sides rect using CSS's top/right/bottom/left expansion.
fn parse_sides<T: Copy>(value: &str, parse_one: impl Fn(&str) -> Option<T>) -> Option<Rect<T>> {
    let parts: Vec<T> = value.split_whitespace().map(|p| parse_one(p)).collect::<Option<_>>()?;
    let (t, r, b, l) = match parts.as_slice() {
        [a] => (*a, *a, *a, *a),
        [v, h] => (*v, *h, *v, *h),
        [t, h, b] => (*t, *h, *b, *h),
        [t, r, b, l] => (*t, *r, *b, *l),
        _ => return None,
    };
    Some(Rect { top: t, right: r, bottom: b, left: l })
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
        assert_eq!(parse_font_weight("bold"), Some(true));
        assert_eq!(parse_font_weight("400"), Some(false));
        assert_eq!(parse_font_weight("700"), Some(true));
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

    #[test]
    fn test_rule_key() {
        assert_eq!(rule_key("div.hero"), RuleKey::Class("hero".into()));
        assert_eq!(rule_key("#nav > ul li.item"), RuleKey::Class("item".into()));
        assert_eq!(rule_key("body #main"), RuleKey::Id("main".into()));
        assert_eq!(rule_key("nav ul > li"), RuleKey::Tag("li".into()));
        assert_eq!(rule_key("*"), RuleKey::Universal);
        // Pseudo/attr narrow a compound; the prefix still buckets it.
        assert_eq!(rule_key("li:first-child"), RuleKey::Tag("li".into()));
        assert_eq!(rule_key("input[type=text]"), RuleKey::Tag("input".into()));
        assert_eq!(rule_key(".menu:hover"), RuleKey::Class("menu".into()));
        assert_eq!(rule_key(":checked"), RuleKey::Universal);
        assert_eq!(rule_key("[hidden]"), RuleKey::Universal);
        // Combinator inside a functional pseudo must not split the compound.
        assert_eq!(rule_key(":is(a > b).x"), RuleKey::Universal);
        // Escaped punctuation is part of the identifier, not syntax: the
        // bucket key must be the class string the DOM actually carries.
        assert_eq!(rule_key(r".md\:block"), RuleKey::Class("md:block".into()));
        assert_eq!(rule_key(r".lg\:flex:hover"), RuleKey::Class("lg:flex".into()));
        assert_eq!(rule_key(r".w-1\/2"), RuleKey::Class("w-1/2".into()));
        assert_eq!(rule_key(r".h-\[5\.75rem\]"), RuleKey::Class("h-[5.75rem]".into()));
        assert_eq!(rule_key(r"div .md\:grid-cols-2"), RuleKey::Class("md:grid-cols-2".into()));
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

    /// `:is()`/`:where()` are not in servo's 2019 selector set. Distributing
    /// them over their arguments keeps the rule alive.
    #[test]
    fn test_lower_is_where_distributes() {
        for sel in [
            "body :is(.x, .y)",
            "body :where(.x, .y)",
            "body :matches(.x, .y)",
            "body :-moz-any(.x, .y)",
        ] {
            let css = format!("{sel} {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(LOWER_HTML, &css, "a"), Some((1, 2, 3)), "{sel}");
            assert_eq!(color_of(LOWER_HTML, &css, "b"), Some((1, 2, 3)), "{sel}");
        }
        // Mid-compound, and with a complex argument in whole-compound
        // position — `div :is(.x > span)` must reach the span.
        assert_eq!(
            color_of(LOWER_HTML, "p:is(.y, .z) { color: rgb(4, 5, 6); }", "b"),
            Some((4, 5, 6))
        );
        assert_eq!(
            color_of(LOWER_HTML, "body :is(.x > span) { color: rgb(7, 8, 9); }", "s"),
            Some((7, 8, 9))
        );
    }

    /// `:where()` contributes NOTHING to specificity, so a later-but-plainer
    /// rule of equal weight wins on source order and a heavier one always
    /// wins. Distributing must not smuggle the argument's weight in.
    #[test]
    fn test_lower_where_specificity() {
        // `:where(.x)` weighs nothing => `#a:where(.x)` is (1,0,0), which a
        // two-class selector (0,2,0) must NOT beat.
        let css = r#"
            #a:where(.x) { color: rgb(1, 1, 1); }
            .x.x { color: rgb(2, 2, 2); }
        "#;
        assert_eq!(color_of(LOWER_HTML, css, "a"), Some((1, 1, 1)));
        // `:is(.x)` DOES weigh: here the `:is` branch is (0,2,0) and, tied
        // with the plain rule, later source order wins.
        let css = r#"
            .y:is(.y) { color: rgb(1, 1, 1); }
            .y.y { color: rgb(2, 2, 2); }
        "#;
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((2, 2, 2)));
    }

    /// Servo parses `:not(simple)` only. An argument LIST becomes a chain,
    /// and an argument that can never match makes the negation a tautology.
    #[test]
    fn test_lower_not_list_and_tautology() {
        assert_eq!(
            color_of(LOWER_HTML, ".x:not(.q, .r) { color: rgb(1, 2, 3); }", "a"),
            Some((1, 2, 3))
        );
        assert_eq!(
            color_of(LOWER_HTML, ".x:not(.q, .x) { color: rgb(1, 2, 3); }", "a"),
            None,
            "a negation that DOES match must still exclude the element"
        );
        // `:focus-visible` is a state we never enter, so negating it is
        // always true — the rule must apply, not be dropped.
        assert_eq!(
            color_of(LOWER_HTML, ".x:not(:focus-visible) { color: rgb(4, 5, 6); }", "a"),
            Some((4, 5, 6))
        );
    }

    /// `:required`/`:optional` are attribute tests in disguise.
    #[test]
    fn test_lower_required_optional() {
        assert_eq!(
            color_of(LOWER_HTML, "input:required { color: rgb(1, 2, 3); }", "i"),
            Some((1, 2, 3))
        );
        assert_eq!(
            color_of(LOWER_HTML, "input:optional { color: rgb(1, 2, 3); }", "i"),
            None
        );
    }

    /// Everything that cannot be lowered still drops — but only its own
    /// list member, and each under a named bucket rather than the residual
    /// compile-failure key.
    #[test]
    fn test_lower_buckets() {
        for (sel, bucket) in [
            (".x:has(> span)", "selector-has"),
            (".x:focus-within", "selector-dynamic-state"),
            (".x:-moz-ui-invalid", "selector-vendor-pseudo"),
            (".x:placeholder-shown", "selector-form-state"),
            (".x:lang(en)", "selector-lang-dir"),
            (".x::-webkit-input-placeholder", "pseudo-element-rule"),
            (".x:-ms-input-placeholder", "pseudo-element-rule"),
            (".x:not(:has(> span))", "selector-has"),
        ] {
            crate::ledger::reset();
            // The sibling member must survive the unsupported one.
            let css = format!("{sel}, .y {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(LOWER_HTML, &css, "b"), Some((1, 2, 3)), "{sel}");
            assert_eq!(color_of(LOWER_HTML, &css, "a"), None, "{sel} must not match");
            let dump = format!("{:?}", crate::ledger::snapshot());
            assert!(dump.contains(bucket), "{sel} should ledger {bucket}: {dump}");
            assert!(
                !dump.contains("selector-compile-failed"),
                "{sel} should have a NAMED bucket, not the residual key"
            );
        }
    }

    /// A selector list survives one unsupported member: the rest of the
    /// list still compiles and still styles its elements.
    #[test]
    fn test_selector_list_partial_compile() {
        let mut tree = crate::layout::compute_layout(&crate::dom::parse_html(
            r#"<html><body><div class="md:block" id="a">x</div><p id="b">y</p></body></html>"#,
        ));
        // `:has()`/`:focus-visible` are not in this engine's selector set;
        // the `p` and the escaped utility class must still get their colour.
        apply_css(
            &mut tree,
            r#"p:has(> em), .md\:block, p { color: rgb(1, 2, 3); }"#,
        );
        let mut painted = 0;
        for (node_id, dom_node) in &tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            let id = el.attributes.borrow().get("id").map(str::to_string);
            if matches!(id.as_deref(), Some("a") | Some("b")) {
                assert_eq!(
                    tree.paint_map.get(node_id).and_then(|p| p.color),
                    Some((1, 2, 3)),
                    "{:?} lost its declarations to an unsupported list member",
                    id
                );
                painted += 1;
            }
        }
        assert_eq!(painted, 2);
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

    #[test]
    fn test_split_selector_list() {
        assert_eq!(split_selector_list("a, b"), vec!["a", "b"]);
        assert_eq!(split_selector_list(":is(a, b), c"), vec![":is(a, b)", "c"]);
        assert_eq!(split_selector_list(r#"[x="a,b"], d"#), vec![r#"[x="a,b"]"#, "d"]);
        assert_eq!(split_selector_list("*,:after,:before"), vec!["*", ":after", ":before"]);
    }

    #[test]
    fn test_media_matches() {
        assert!(media_matches("screen", 800.0));
        assert!(!media_matches("print", 800.0));
        assert!(media_matches("screen and (min-width: 600px)", 800.0));
        assert!(!media_matches("screen and (min-width: 1200px)", 800.0));
        assert!(media_matches("screen and (min-width: 1200px)", 1280.0));
        assert!(media_matches("(max-width: 900px)", 800.0));
        assert!(media_matches("print, screen", 800.0));
        assert!(!media_matches("(prefers-reduced-motion: reduce)", 800.0));
        // Honest environment answers + `not` negation.
        assert!(media_matches("(prefers-color-scheme: light)", 800.0));
        assert!(!media_matches("(prefers-color-scheme: dark)", 800.0));
        assert!(media_matches("(prefers-reduced-motion: no-preference)", 800.0));
        assert!(media_matches("(hover: hover)", 800.0));
        assert!(media_matches("(orientation: landscape)", 800.0));
        assert!(!media_matches("not all", 800.0));
        assert!(media_matches("not print", 800.0));
        assert!(!media_matches("screen and not (min-width: 600px)", 800.0));
    }
}
