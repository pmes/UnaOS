use kuchiki::NodeRef;
use taffy::prelude::*;
use std::collections::HashMap;

/// Specified paint properties for one box. `None` = not specified here;
/// color and font-size inherit down the tree at render time.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaintStyle {
    pub background: Option<(u8, u8, u8)>,
    pub color: Option<(u8, u8, u8)>,
    pub font_size: Option<f32>,
    pub bold: Option<bool>,
}

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    pub node_map: HashMap<taffy::NodeId, NodeRef>, // Maps layout boxes back to DOM nodes
    pub paint_map: HashMap<taffy::NodeId, PaintStyle>,
    pub dirty: bool,
}

impl LayoutTree {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn recompute(&mut self, dom: &NodeRef) {
        if !self.dirty {
            return;
        }
        let new_tree = compute_layout(dom);
        self.taffy = new_tree.taffy;
        self.root_node = new_tree.root_node;
        self.node_map = new_tree.node_map;
        self.paint_map = new_tree.paint_map;
        self.dirty = false;
    }
}

/// Elements whose subtrees produce no boxes.
fn is_non_rendered(name: &str) -> bool {
    matches!(name, "head" | "script" | "style" | "title" | "meta" | "link" | "template")
}

/// Inline-level elements: they size to content and flow in wrapping rows.
fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "a" | "span" | "b" | "strong" | "i" | "em" | "u" | "s" | "code" | "small" | "big"
            | "sup" | "sub" | "label" | "abbr" | "cite" | "q" | "time" | "img" | "wbr"
            | "td" | "th" | "button" | "input" | "select"
    )
}

/// True when a DOM node lays out as inline content (text or inline element).
fn is_inline_node(node: &NodeRef) -> bool {
    if node.as_text().is_some() {
        return true;
    }
    node.as_element()
        .map(|el| is_inline(el.name.local.as_ref()))
        .unwrap_or(false)
}

pub fn compute_layout(dom: &NodeRef) -> LayoutTree {
    let mut taffy = taffy::TaffyTree::new();
    let mut node_map = HashMap::new();
    let mut paint_map = HashMap::new();

    fn build_taffy_tree(
        dom_node: &NodeRef,
        taffy: &mut taffy::TaffyTree,
        node_map: &mut HashMap<taffy::NodeId, NodeRef>,
        paint_map: &mut HashMap<taffy::NodeId, PaintStyle>,
    ) -> Option<taffy::NodeId> {
        if let Some(el) = dom_node.as_element() {
            if is_non_rendered(el.name.local.as_ref()) {
                return None;
            }
        } else if dom_node.as_text().is_some() {
            // Whitespace-only text produces no box.
            if dom_node.text_contents().trim().is_empty() {
                return None;
            }
        }

        let mut child_ids = Vec::new();
        for child in dom_node.children() {
            if let Some(id) = build_taffy_tree(&child, taffy, node_map, paint_map) {
                child_ids.push(id);
            }
        }

        let tag = dom_node
            .as_element()
            .map(|el| el.name.local.as_ref().to_string())
            .unwrap_or_default();
        let inline = is_inline_node(dom_node);
        // A box whose rendered children are all inline content flows them as
        // a wrapping row (the inline-formatting-context approximation).
        let children_inline = dom_node.children().any(|_| true)
            && dom_node
                .children()
                .filter(|c| {
                    c.as_element().is_some()
                        || c.as_text().is_some() && !c.text_contents().trim().is_empty()
                })
                .all(|c| is_inline_node(&c));

        let mut style = Style {
            display: Display::Flex,
            flex_direction: if tag == "tr" || (children_inline && !child_ids.is_empty()) {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            },
            flex_wrap: if children_inline && !child_ids.is_empty() {
                taffy::style::FlexWrap::Wrap
            } else {
                taffy::style::FlexWrap::NoWrap
            },
            align_items: if children_inline && !child_ids.is_empty() {
                Some(taffy::style::AlignItems::BASELINE)
            } else {
                None
            },
            size: Size {
                width: if inline { Dimension::auto() } else { Dimension::percent(1.0) },
                height: Dimension::auto(),
            },
            min_size: Size {
                width: Dimension::auto(),
                height: if inline { Dimension::auto() } else { Dimension::length(20.0) },
            },
            margin: {
                let m = if inline { 0.0 } else { 2.0 };
                Rect {
                    left: LengthPercentage::length(m).into(),
                    right: LengthPercentage::length(m).into(),
                    top: LengthPercentage::length(m).into(),
                    bottom: LengthPercentage::length(m).into(),
                }
            },
            ..Default::default()
        };

        // Inline style="..." — paint properties plus width/height.
        let mut paint = PaintStyle::default();
        if let Some(el) = dom_node.as_element() {
            if let Some(inline) = el.attributes.borrow().get("style") {
                apply_inline_style(inline, &mut style, &mut paint);
            }
        }

        if let Ok(node_id) = taffy.new_with_children(style, &child_ids) {
            node_map.insert(node_id, dom_node.clone());
            paint_map.insert(node_id, paint);
            Some(node_id)
        } else {
            None
        }
    }

    let root_node = build_taffy_tree(dom, &mut taffy, &mut node_map, &mut paint_map)
        .unwrap_or_else(|| taffy.new_leaf(Style::default()).unwrap());

    let mut tree = LayoutTree {
        taffy,
        root_node,
        node_map,
        paint_map,
        dirty: false,
    };
    remeasure(&mut tree);
    tree
}

/// UA default font sizes per element (shared with the renderer).
pub fn default_font_size(tag: &str, inherited: f32) -> f32 {
    match tag {
        "h1" => 32.0,
        "h2" => 24.0,
        "h3" => 19.0,
        "h4" => 16.0,
        "small" => 13.0,
        _ => inherited,
    }
}

/// Recomputes layout with real text measurement: resolves each text run's
/// inherited font size, then lets taffy size text leaves by wrapped extent.
/// Called after building the tree and after every cascade application.
pub fn remeasure(tree: &mut LayoutTree) {
    // Pass 1: resolve font size down the box tree for text leaves.
    let mut text_info: HashMap<taffy::NodeId, (String, f32)> = HashMap::new();
    fn resolve(
        node_id: taffy::NodeId,
        inherited: f32,
        tree: &LayoutTree,
        out: &mut HashMap<taffy::NodeId, (String, f32)>,
    ) {
        let mut size = inherited;
        if let Some(dom_node) = tree.node_map.get(&node_id) {
            if let Some(el) = dom_node.as_element() {
                let spec = tree.paint_map.get(&node_id).and_then(|p| p.font_size);
                size = spec.unwrap_or_else(|| default_font_size(el.name.local.as_ref(), inherited));
            } else if dom_node.as_text().is_some() {
                let text = dom_node.text_contents();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    out.insert(node_id, (text, inherited));
                }
            }
        }
        if let Ok(children) = tree.taffy.children(node_id) {
            for child in children {
                resolve(child, size, tree, out);
            }
        }
    }
    resolve(tree.root_node, 16.0, tree, &mut text_info);

    let font = crate::fonts::FontEngine::new().load_font(
        &[font_kit::family_name::FamilyName::SansSerif],
        &font_kit::properties::Properties::new(),
    );

    let viewport = Size {
        width: AvailableSpace::Definite(800.0),
        height: AvailableSpace::Definite(600.0),
    };
    let _ = tree.taffy.compute_layout_with_measure(
        tree.root_node,
        viewport,
        |known, avail, node_id, _ctx, _style| {
            let Some((text, font_size)) = text_info.get(&node_id) else {
                return Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) };
            };
            let wrap_width = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                _ => 800.0,
            });
            let (w, h) = measure_text(font.as_deref(), text, *font_size, wrap_width.max(1.0));
            Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(h),
            }
        },
    );
}

/// Measures wrapped text: returns (widest line, total height). Mirrors the
/// renderer's wrap algorithm so painted text fits its measured box.
fn measure_text(
    font: Option<&font_kit::font::Font>,
    text: &str,
    font_size: f32,
    max_width: f32,
) -> (f32, f32) {
    let Some(font) = font else {
        return (max_width, font_size * 1.25);
    };
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    let line_height = (metrics.ascent - metrics.descent + metrics.line_gap) * scale;
    let space = font_size * 0.3;

    let mut pen = 0.0f32;
    let mut lines = 1u32;
    let mut widest = 0.0f32;
    for word in text.split_whitespace() {
        let word_width: f32 = word
            .chars()
            .filter_map(|c| font.glyph_for_char(c))
            .filter_map(|g| font.advance(g).ok())
            .map(|a| a.x() * scale)
            .sum();
        if pen > 0.0 && pen + word_width > max_width {
            lines += 1;
            pen = 0.0;
        }
        pen += word_width + space;
        widest = widest.max(pen);
    }
    (widest.min(max_width), lines as f32 * line_height)
}

/// Applies a `style="..."` attribute through the shared declaration parser.
fn apply_inline_style(inline: &str, style: &mut Style, paint: &mut PaintStyle) {
    let spec = crate::css::parse_declaration_block(inline);
    spec.fold_into(style);
    *paint = spec.paint;
}
