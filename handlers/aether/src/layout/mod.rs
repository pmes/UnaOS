use kuchiki::NodeRef;
use taffy::prelude::*;
use std::collections::HashMap;

/// Specified paint properties for one box. `None` = not specified here;
/// color and font-size inherit down the tree at render time.
#[derive(Debug, Clone, Default)]
pub struct PaintStyle {
    pub background: Option<(u8, u8, u8)>,
    pub color: Option<(u8, u8, u8)>,
    pub font_size: Option<f32>,
    pub bold: Option<bool>,
    /// (width px, color). Painted as a rect stroke.
    pub border: Option<(f32, (u8, u8, u8))>,
    /// Resolved line height: multiplier of font size (px values are
    /// converted at parse time against the 16px base — approximation).
    pub line_height: Option<f32>,
    /// background-image url (as written; resolved via images::get at paint).
    pub bg_image: Option<String>,
    /// visibility:hidden / opacity:0 — box keeps its space, paints nothing.
    pub hidden: Option<bool>,
    /// overflow != visible — descendants clip to this box's rect.
    pub clip: Option<bool>,
}

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    pub node_map: HashMap<taffy::NodeId, NodeRef>, // Maps layout boxes back to DOM nodes
    pub paint_map: HashMap<taffy::NodeId, PaintStyle>,
    /// Viewport this tree lays out against (media queries + wrap width).
    pub viewport: (f32, f32),
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
        let new_tree = compute_layout_sized(dom, self.viewport.0, self.viewport.1);
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
            | "sup" | "sub" | "label" | "abbr" | "cite" | "q" | "time" | "img" | "wbr" | "br"
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
    compute_layout_sized(dom, 800.0, 600.0)
}

pub fn compute_layout_sized(dom: &NodeRef, vw: f32, vh: f32) -> LayoutTree {
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
            // The HTML hidden attribute / aria-hidden remove the subtree.
            let attrs = el.attributes.borrow();
            if attrs.get("hidden").is_some() || attrs.get("aria-hidden") == Some("true") {
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
            // Inline boxes keep their measured width; the row wraps instead
            // of shrinking them (shrunk text would draw more lines than the
            // measured height and overlap the next block).
            flex_shrink: if inline { 0.0 } else { 1.0 },
            size: Size {
                width: if inline { Dimension::auto() } else { Dimension::percent(1.0) },
                height: Dimension::auto(),
            },
            min_size: match tag.as_str() {
                // UA default control sizes so empty controls are visible.
                "input" | "select" => Size {
                    width: Dimension::length(160.0),
                    height: Dimension::length(24.0),
                },
                "textarea" => Size {
                    width: Dimension::length(160.0),
                    height: Dimension::length(60.0),
                },
                "button" => Size {
                    width: Dimension::length(24.0),
                    height: Dimension::length(24.0),
                },
                _ => Size {
                    width: Dimension::auto(),
                    height: if inline { Dimension::auto() } else { Dimension::length(20.0) },
                },
            },
            margin: {
                // UA default spacing: block gaps for paragraphs/headings,
                // list indentation, nothing for inline content.
                let (v, left) = if inline {
                    (0.0, 0.0)
                } else {
                    match tag.as_str() {
                        "p" | "blockquote" | "pre" => (8.0, 2.0),
                        "h1" | "h2" => (12.0, 2.0),
                        "h3" | "h4" | "h5" | "h6" => (10.0, 2.0),
                        "ul" | "ol" => (8.0, 24.0),
                        "li" => (2.0, 4.0),
                        "body" => (8.0, 8.0),
                        _ => (2.0, 2.0),
                    }
                };
                Rect {
                    left: LengthPercentage::length(left).into(),
                    right: LengthPercentage::length(2.0_f32.min(left)).into(),
                    top: LengthPercentage::length(v).into(),
                    bottom: LengthPercentage::length(v).into(),
                }
            },
            ..Default::default()
        };

        // <br>: a zero-height full-width item forces a wrap break in the
        // inline row without adding vertical space of its own.
        if tag == "br" {
            style.size.width = Dimension::percent(1.0);
            style.size.height = Dimension::length(0.0);
            style.min_size = Size { width: Dimension::auto(), height: Dimension::length(0.0) };
        }

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
        viewport: (vw, vh),
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
    // Pass 1: resolve font size down the box tree for text leaves, and
    // intrinsic sizes for images.
    let mut text_info: HashMap<taffy::NodeId, (String, f32, f32)> = HashMap::new();
    let mut img_info: HashMap<taffy::NodeId, (f32, f32)> = HashMap::new();
    fn resolve(
        node_id: taffy::NodeId,
        inherited: (f32, f32), // (font size, line-height multiplier; 0 = natural)
        tree: &LayoutTree,
        out: &mut HashMap<taffy::NodeId, (String, f32, f32)>,
        imgs: &mut HashMap<taffy::NodeId, (f32, f32)>,
    ) {
        let mut size = inherited;
        if let Some(dom_node) = tree.node_map.get(&node_id) {
            if let Some(el) = dom_node.as_element() {
                let paint = tree.paint_map.get(&node_id);
                size.0 = paint
                    .and_then(|p| p.font_size)
                    .unwrap_or_else(|| default_font_size(el.name.local.as_ref(), inherited.0));
                if let Some(lh) = paint.and_then(|p| p.line_height) {
                    size.1 = lh;
                }
                if el.name.local.as_ref() == "img" {
                    let attrs = el.attributes.borrow();
                    // width/height attributes win; else intrinsic dimensions.
                    let attr_px = |name: &str| {
                        attrs.get(name).and_then(|v| v.trim().parse::<f32>().ok())
                    };
                    let intrinsic = attrs
                        .get("src")
                        .and_then(crate::images::get)
                        .map(|i| (i.width() as f32, i.height() as f32));
                    let w = attr_px("width").or(intrinsic.map(|(w, _)| w));
                    let h = attr_px("height").or(intrinsic.map(|(_, h)| h));
                    if let (Some(w), Some(h)) = (w, h) {
                        imgs.insert(node_id, (w, h));
                    }
                }
            } else if dom_node.as_text().is_some() {
                let text = dom_node.text_contents();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    out.insert(node_id, (text, inherited.0, inherited.1));
                }
            }
        }
        if let Ok(children) = tree.taffy.children(node_id) {
            for child in children {
                resolve(child, size, tree, out, imgs);
            }
        }
    }
    resolve(tree.root_node, (16.0, 0.0), tree, &mut text_info, &mut img_info);

    // Taffy caches leaf measurements; a cascade pass can change resolved
    // font sizes without touching the leaf's style, so stale cached sizes
    // survive set_style dirtying. Invalidate every measured leaf.
    for node_id in text_info.keys().chain(img_info.keys()) {
        let _ = tree.taffy.mark_dirty(*node_id);
    }

    let font = crate::fonts::FontEngine::new().load_font(
        &[font_kit::family_name::FamilyName::SansSerif],
        &font_kit::properties::Properties::new(),
    );

    let viewport = Size {
        width: AvailableSpace::Definite(tree.viewport.0),
        height: AvailableSpace::Definite(tree.viewport.1),
    };
    let vw_cap = tree.viewport.0;
    let _ = tree.taffy.compute_layout_with_measure(
        tree.root_node,
        viewport,
        |known, avail, node_id, _ctx, _style| {
            if let Some(&(w, h)) = img_info.get(&node_id) {
                return Size {
                    width: known.width.unwrap_or(w),
                    height: known.height.unwrap_or(h),
                };
            }
            let Some((text, font_size, line_mult)) = text_info.get(&node_id) else {
                return Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) };
            };
            let wrap_width = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                _ => vw_cap,
            });
            let (w, h) = measure_text(font.as_deref(), text, *font_size, *line_mult, wrap_width.max(1.0));
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
    line_mult: f32,
    max_width: f32,
) -> (f32, f32) {
    let Some(font) = font else {
        return (max_width, font_size * 1.25);
    };
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    let natural = (metrics.ascent - metrics.descent + metrics.line_gap) * scale;
    let line_height = if line_mult > 0.0 { font_size * line_mult } else { natural };
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
    *paint = spec.paint.clone();
}
