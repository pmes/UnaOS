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
    pub align_self: Option<taffy::style::AlignSelf>,
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
        if let Some(a) = self.align_self {
            node_style.align_self = Some(a);
        }
        if let Some((w, _)) = self.paint.border {
            let b = LengthPercentage::length(w);
            node_style.border = Rect { left: b, right: b, top: b, bottom: b };
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
            | "table-header-group" | "table-footer-group" => {
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
        "text-align" => match value {
            "center" => style.justify = Some(taffy::style::JustifyContent::CENTER),
            "right" | "end" => style.justify = Some(taffy::style::JustifyContent::END),
            "left" | "start" | "justify" => style.justify = Some(taffy::style::JustifyContent::START),
            other => crate::ledger::record_css(&format!("text-align:{}", other)),
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
            // Shorthand: a color and/or an image url; other components
            // (position/repeat/size) are ignored — cover paint below.
            if let Some(u) = extract_css_url(value) {
                style.paint.bg_image = Some(u);
            }
            let mut got_color = false;
            for part in value.split_whitespace() {
                if part.starts_with("url(") { continue; }
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
            if let Ok(a) = value.parse::<f32>() {
                if a == 0.0 {
                    style.paint.hidden = Some(true);
                } else if style.paint.hidden == Some(true) {
                    style.paint.hidden = Some(false);
                }
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
        "border" | "outline" => {
            let v = value.trim();
            if v == "none" || v == "0" {
                style.paint.border = None;
                return;
            }
            let mut width = 1.0f32;
            let mut color = (128, 128, 128);
            let mut got_any = false;
            for part in v.split_whitespace() {
                if let Some(px) = part.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
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
                style.paint.border = Some((width, color));
            } else {
                crate::ledger::record_css(&format!("border-value:{}", clip(value)));
            }
        }
        "border-color" => {
            if let Some(c) = parse_color_str(value) {
                let w = style.paint.border.map(|(w, _)| w).unwrap_or(1.0);
                style.paint.border = Some((w, c));
            }
        }
        "border-width" => {
            if let Some(w) = parse_px(value) {
                let c = style.paint.border.map(|(_, c)| c).unwrap_or((128, 128, 128));
                style.paint.border = Some((w, c));
            }
        }
        "border-style" => {} // stroke style is uniform; nothing to record
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
        for decl in body.split(';') {
            let Some((name, value)) = decl.split_once(':') else { continue };
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

/// Truncates a value for a stable, bounded ledger key.
fn clip(v: &str) -> &str {
    &v[..v.len().min(24)]
}

/// One applicable rule: a single compiled selector with its declarations.
/// A rule with `!important` declarations is split in two — the important
/// half carries `important: true` and applies in a later cascade tier.
struct Rule {
    selector: kuchiki::Selector,
    style: std::rc::Rc<SpecifiedStyle>,
    important: bool,
    key: RuleKey,
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
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ' ' | '\t' | '>' | '+' | '~' if depth == 0 => start = i + c.len_utf8(),
            _ => {}
        }
    }
    let comp = s[start..].trim();
    // Pseudo-classes and attribute selectors only NARROW a compound, so the
    // simple-selector prefix before the first ':'/'[' is still a valid
    // bucket key ("li:first-child" can only match an <li>). A compound that
    // STARTS with ':'/'[' gives no such guarantee → Universal.
    let comp = &comp[..comp.find([':', '[']).unwrap_or(comp.len())];
    if let Some(p) = comp.rfind('.') {
        let rest = &comp[p + 1..];
        let end = rest.find(['.', '#']).unwrap_or(rest.len());
        if !rest[..end].is_empty() {
            return RuleKey::Class(rest[..end].to_string());
        }
    }
    if let Some(p) = comp.rfind('#') {
        let rest = &comp[p + 1..];
        let end = rest.find(['.', '#']).unwrap_or(rest.len());
        if !rest[..end].is_empty() {
            return RuleKey::Id(rest[..end].to_string());
        }
    }
    let end = comp.find(['.', '#']).unwrap_or(comp.len());
    let tag = comp[..end].trim();
    if tag.is_empty() || tag == "*" {
        RuleKey::Universal
    } else {
        RuleKey::Tag(tag.to_ascii_lowercase())
    }
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
    if style.paint.background.is_some() { entry.background = style.paint.background; }
    if style.paint.color.is_some() { entry.color = style.paint.color; }
    if style.paint.font_size.is_some() { entry.font_size = style.paint.font_size; }
    if style.paint.bold.is_some() { entry.bold = style.paint.bold; }
    if style.paint.border.is_some() { entry.border = style.paint.border; }
    if style.paint.line_height.is_some() { entry.line_height = style.paint.line_height; }
    if style.paint.bg_image.is_some() { entry.bg_image = style.paint.bg_image.clone(); }
    if style.paint.hidden.is_some() { entry.hidden = style.paint.hidden; }
    if style.paint.clip.is_some() { entry.clip = style.paint.clip; }
}

/// Applies a set of stylesheets as ONE cascade — rules from every sheet
/// sort together by (importance, specificity, source order). Inline
/// `style=""` declarations slot in at their CSS priority: above all
/// normal rules, below `!important` rules; inline `!important` wins all.
pub fn apply_stylesheets(layout_tree: &mut LayoutTree, sheets: &[String]) {
    let vw = layout_tree.viewport.0;
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
            .then(a.selector.specificity().cmp(&b.selector.specificity()))
    });

    // Element refs computed ONCE — matching is O(rules × elements) either
    // way, but the per-pair node_map lookup + ref construction was the
    // hot-path churn on rule-heavy pages.
    let elements: Vec<(taffy::NodeId, kuchiki::NodeDataRef<kuchiki::ElementData>)> = layout_tree
        .node_map
        .iter()
        .filter_map(|(id, n)| n.clone().into_element_ref().map(|el| (*id, el)))
        .collect();

    // Inline style="" tiers, parsed once per node.
    let mut inline_tiers: Vec<(taffy::NodeId, SpecifiedStyle, SpecifiedStyle, bool)> = Vec::new();
    for (node_id, el_ref) in &elements {
        let Some(inline) = el_ref.attributes.borrow().get("style").map(|s| s.to_string()) else { continue };
        let (normal, important, has_important) = parse_declaration_block_tiers(&inline);
        inline_tiers.push((*node_id, normal, important, has_important));
    }

    // Accumulate each node's winning declarations in cascade order, then
    // apply ONCE per node — one taffy style clone/set per styled node
    // instead of one per matching rule.
    let mut acc: std::collections::HashMap<taffy::NodeId, SpecifiedStyle> =
        std::collections::HashMap::new();
    let merge_rules = |rules: &[&Rule], acc: &mut std::collections::HashMap<taffy::NodeId, SpecifiedStyle>| {
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
        for (node_id, el_ref) in &elements {
            candidates.clear();
            candidates.extend_from_slice(&universal);
            if let Some(v) = by_tag.get(el_ref.name.local.as_ref()) {
                candidates.extend_from_slice(v);
            }
            {
                let attrs = el_ref.attributes.borrow();
                if let Some(id) = attrs.get("id") {
                    if let Some(v) = by_id.get(id) {
                        candidates.extend_from_slice(v);
                    }
                }
                if let Some(classes) = attrs.get("class") {
                    for class in classes.split_whitespace() {
                        if let Some(v) = by_class.get(class) {
                            candidates.extend_from_slice(v);
                        }
                    }
                }
            }
            candidates.sort_unstable();
            for &i in candidates.iter() {
                if rules[i].selector.matches(el_ref) {
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

        // Compile with kuchiki's real selector engine (servo selectors).
        let selectors = match kuchiki::Selectors::compile(&prelude) {
            Ok(s) => s,
            Err(_) => {
                crate::ledger::record_css(&format!("selector-compile-failed:{}", clip(&prelude)));
                continue;
            }
        };

        let (normal, important, has_important) = parse_declaration_block_tiers(&body);
        let normal = std::rc::Rc::new(normal);
        for selector in selectors.0 {
            let key = rule_key(&selector.to_string());
            rules.push(Rule { selector, style: normal.clone(), important: false, key });
        }
        if has_important {
            // Selector isn't Clone; the important tier compiles its own copy.
            if let Ok(selectors) = kuchiki::Selectors::compile(&prelude) {
                let important = std::rc::Rc::new(important);
                for selector in selectors.0 {
                    let key = rule_key(&selector.to_string());
                    rules.push(Rule { selector, style: important.clone(), important: true, key });
                }
            }
        }
    }
}

/// Parses a declaration block into (normal, !important) tiers plus a flag
/// saying whether the important tier holds anything. Declarations split by
/// their own priority, per CSS — not per rule.
pub(crate) fn parse_declaration_block_tiers(body: &str) -> (SpecifiedStyle, SpecifiedStyle, bool) {
    let mut normal = SpecifiedStyle::default();
    let mut important = SpecifiedStyle::default();
    let mut has_important = false;
    for decl in body.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
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
          inset_top, inset_left, inset_right, inset_bottom, justify, align_self,
          max_width, max_height, min_width, min_height);
    let p = &src.paint;
    if p.background.is_some() { dst.paint.background = p.background; }
    if p.color.is_some() { dst.paint.color = p.color; }
    if p.font_size.is_some() { dst.paint.font_size = p.font_size; }
    if p.bold.is_some() { dst.paint.bold = p.bold; }
    if p.border.is_some() { dst.paint.border = p.border; }
    if p.line_height.is_some() { dst.paint.line_height = p.line_height; }
    if p.bg_image.is_some() { dst.paint.bg_image = p.bg_image.clone(); }
    if p.hidden.is_some() { dst.paint.hidden = p.hidden; }
    if p.clip.is_some() { dst.paint.clip = p.clip; }
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
            let p = part.trim().trim_start_matches('(').trim_end_matches(')').trim();
            match p {
                "screen" | "all" => true,
                "print" => false,
                _ => {
                    if let Some((feature, value)) = p.split_once(':') {
                        let value = value.trim();
                        match feature.trim() {
                            "min-width" => parse_px(value).map(|v| vw >= v).unwrap_or(false),
                            "max-width" => parse_px(value).map(|v| vw <= v).unwrap_or(false),
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
            | "position" | "top" | "left" | "right" | "bottom" | "text-align"
            | "background-color" | "background" | "background-image" | "color"
            | "font-size" | "font-weight" | "line-height" | "visibility"
            | "max-width" | "max-height" | "min-width" | "min-height"
            | "overflow" | "overflow-x" | "overflow-y"
            | "border" | "outline" | "border-color" | "border-width" | "border-style"
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

/// Parses "NNpx" (or a bare number) into pixels.
pub fn parse_px(value: &str) -> Option<f32> {
    let v = value.trim();
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
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
    }
}
