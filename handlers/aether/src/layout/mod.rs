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
    /// Font family class: 0 = sans (default), 1 = serif, 2 = monospace.
    pub family: Option<u8>,
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
    // noscript: scripting IS enabled here (page scripts run), so its
    // fallback content must not render. iframe/object/embed: no frame
    // support yet — an empty box is honest, raw content leaking is not.
    matches!(
        name,
        "head" | "script" | "style" | "title" | "meta" | "link" | "template"
            | "noscript" | "iframe" | "object" | "embed"
    )
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

/// Quirks mode: no doctype at all, or anything other than a bare
/// `<!DOCTYPE html>`. Only the handful of rendering quirks this engine
/// actually implements key off it.
pub fn is_quirks(dom: &NodeRef) -> bool {
    let doc = if dom.as_document().is_some() {
        dom.clone()
    } else {
        let mut n = dom.clone();
        while let Some(p) = n.parent() {
            n = p;
        }
        n
    };
    for child in doc.children() {
        if let Some(dt) = child.as_doctype() {
            return !(dt.name.eq_ignore_ascii_case("html")
                && dt.public_id.is_empty()
                && dt.system_id.is_empty());
        }
    }
    true
}

/// Table-internal boxes. They are neither block nor inline level: their
/// parent lays them out by table rules, so the inline/block machinery
/// (anonymous block boxes, the inline-with-block-children demotion) must
/// leave them alone.
fn is_table_part(name: &str) -> bool {
    matches!(
        name,
        "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption"
            | "colgroup" | "col"
    )
}

/// True when the node produces a box at all (mirrors the build-time skips).
fn generates_box(node: &NodeRef) -> bool {
    if let Some(el) = node.as_element() {
        if is_non_rendered(el.name.local.as_ref()) {
            return false;
        }
        let attrs = el.attributes.borrow();
        if attrs.get("hidden").is_some() || attrs.get("aria-hidden") == Some("true") {
            return false;
        }
        if el.name.local.as_ref() == "input"
            && attrs.get("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("hidden"))
        {
            return false;
        }
        true
    } else {
        node.as_text().is_some() && !node.text_contents().trim().is_empty()
    }
}

/// True when a DOM node lays out as inline content (text or inline element).
fn is_inline_node(node: &NodeRef) -> bool {
    if node.as_text().is_some() {
        return true;
    }
    let Some(el) = node.as_element() else { return false };
    let tag = el.name.local.as_ref();
    if !is_inline(tag) {
        return false;
    }
    // Table cells flow by table rules whatever they contain.
    if is_table_part(tag) {
        return true;
    }
    // An inline element that CONTAINS block-level children is broken around
    // them by CSS; the practical approximation is to lay it out as a block.
    // Treated as inline it shrank to fit while its block children still
    // wanted 100% — <span id=footer> wrapping the whole page footer came out
    // half width.
    // Tag-level test only (no recursion): the check runs at every level of
    // the build, and a subtree-deep test would make it quadratic.
    !node.children().any(|c| {
        generates_box(&c)
            && c.as_element()
                .is_some_and(|e| !is_inline(e.name.local.as_ref()))
    })
}

pub fn compute_layout(dom: &NodeRef) -> LayoutTree {
    compute_layout_sized(dom, 800.0, 600.0)
}

pub fn compute_layout_sized(dom: &NodeRef, vw: f32, vh: f32) -> LayoutTree {
    let mut tree = build_tree(dom, vw, vh);
    remeasure(&mut tree);
    tree
}

/// Builds the box tree WITHOUT the measuring layout pass. Callers that
/// immediately run a cascade (which remeasures at the end) use this — the
/// double full layout per page load was a profiled cost.
pub fn build_tree(dom: &NodeRef, vw: f32, vh: f32) -> LayoutTree {
    // Inline `style="height:100vh"` resolves against this same viewport.
    crate::css::set_viewport(vw, vh);
    let mut taffy = taffy::TaffyTree::new();
    let mut node_map = HashMap::new();
    let mut paint_map = HashMap::new();

    fn build_taffy_tree(
        dom_node: &NodeRef,
        taffy: &mut taffy::TaffyTree,
        node_map: &mut HashMap<taffy::NodeId, NodeRef>,
        paint_map: &mut HashMap<taffy::NodeId, PaintStyle>,
        quirks: bool,
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
            // <input type=hidden> is a form value carrier, not a control:
            // the UA sheet gives it display:none. Pages carry many of them
            // (tokens, locale, charset); boxed, they paint as a stack of
            // phantom fields above the real content.
            if el.name.local.as_ref() == "input"
                && attrs
                    .get("type")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("hidden"))
            {
                return None;
            }
        } else if dom_node.as_text().is_some() {
            // Whitespace-only text produces no box.
            if dom_node.text_contents().trim().is_empty() {
                return None;
            }
        }

        let mut kids: Vec<(taffy::NodeId, bool)> = Vec::new();
        for child in dom_node.children() {
            if let Some(id) = build_taffy_tree(&child, taffy, node_map, paint_map, quirks) {
                kids.push((id, is_inline_node(&child)));
            }
        }
        let any_inline = kids.iter().any(|&(_, i)| i);
        let any_block = kids.iter().any(|&(_, i)| !i);
        // Mixed inline and block siblings: CSS wraps each run of consecutive
        // inline children in an ANONYMOUS BLOCK BOX, which is what keeps them
        // flowing on a shared line. Without it the whole container fell back
        // to a column and every inline sibling got its own full-width row —
        // two submit buttons meant to sit side by side stacked instead.
        let parent_tag = dom_node.as_element().map(|el| el.name.local.as_ref().to_string());
        let table_container = parent_tag
            .as_deref()
            .is_some_and(|t| is_table_part(t) && !matches!(t, "td" | "th" | "caption"));
        let child_ids: Vec<taffy::NodeId> = if any_inline && any_block && !table_container {
            let mut out: Vec<taffy::NodeId> = Vec::new();
            let mut run: Vec<taffy::NodeId> = Vec::new();
            let mut flush = |run: &mut Vec<taffy::NodeId>,
                             out: &mut Vec<taffy::NodeId>,
                             taffy: &mut taffy::TaffyTree| {
                // A run of ONE keeps its place as a direct child: it already
                // had a line to itself, and boxing it would hide its own
                // alignment (the float/align_self approximation) from the
                // real container. Only runs that must SHARE a line need the
                // anonymous box.
                if run.len() < 2 {
                    out.append(run);
                    return;
                }
                let anon = Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    flex_wrap: taffy::style::FlexWrap::Wrap,
                    align_items: Some(taffy::style::AlignItems::BASELINE),
                    size: Size { width: Dimension::percent(1.0), height: Dimension::auto() },
                    ..Default::default()
                };
                if let Ok(id) = taffy.new_with_children(anon, run) {
                    paint_map.insert(id, PaintStyle::default());
                    out.push(id);
                } else {
                    out.append(run);
                }
                run.clear();
            };
            for &(id, inline) in &kids {
                if inline {
                    run.push(id);
                } else {
                    flush(&mut run, &mut out, taffy);
                    out.push(id);
                }
            }
            flush(&mut run, &mut out, taffy);
            out
        } else {
            kids.iter().map(|&(id, _)| id).collect()
        };

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
                // Everything else: no UA minimum. An empty block box is
                // zero-tall in CSS; a floor here compounds — pages mount
                // dozens of empty container/portal divs, and 20px each
                // pushed the real content below the fold.
                _ => Size { width: Dimension::auto(), height: Dimension::auto() },
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
                        "li" => (0.0, 4.0),
                        "body" => (8.0, 8.0),
                        // Generic blocks have NO UA margin in CSS. The old
                        // 2px-all-round default accumulated once per nesting
                        // level: a 12-deep shell gained ~24px of indent and
                        // ~48px of vertical air before any content.
                        _ => (0.0, 0.0),
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
            // UA / presentational alignment defaults, applied BEFORE the
            // author cascade so any real rule wins: <center> and <th> centre
            // their content, and the legacy align="" attribute still aligns.
            paint.text_align = match tag.as_str() {
                "center" | "th" => Some(1),
                // The quirks-mode table quirk: a table does NOT inherit
                // text-align from its ancestors. Every legacy page that
                // wraps its layout table in <center> (Hacker News) relies
                // on it — without the reset the whole page centres.
                "table" if quirks => Some(0),
                _ => None,
            };
            if let Some(a) = el.attributes.borrow().get("align") {
                match a.trim().to_ascii_lowercase().as_str() {
                    "center" | "middle" => paint.text_align = Some(1),
                    "right" => paint.text_align = Some(2),
                    "left" => paint.text_align = Some(0),
                    _ => {}
                }
            }
            if let Some(inline) = el.attributes.borrow().get("style") {
                let saved = paint.text_align;
                apply_inline_style(inline, &mut style, &mut paint);
                if paint.text_align.is_none() {
                    paint.text_align = saved;
                }
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

    let root_node = build_taffy_tree(dom, &mut taffy, &mut node_map, &mut paint_map, is_quirks(dom))
        .unwrap_or_else(|| taffy.new_leaf(Style::default()).unwrap());

    LayoutTree {
        taffy,
        root_node,
        node_map,
        paint_map,
        viewport: (vw, vh),
        dirty: false,
    }
}

/// UA default font FAMILY per element: code-ish tags are monospace.
pub fn default_family(tag: &str, inherited: u8) -> u8 {
    match tag {
        "code" | "pre" | "kbd" | "samp" | "tt" => 2,
        _ => inherited,
    }
}

/// Loads the (family, bold=false) fonts once per thread: [sans, serif, mono].
pub fn family_font(family: u8) -> Option<std::sync::Arc<font_kit::font::Font>> {
    thread_local! {
        static FONTS: std::cell::RefCell<[Option<Option<std::sync::Arc<font_kit::font::Font>>>; 3]> =
            const { std::cell::RefCell::new([None, None, None]) };
    }
    let idx = (family as usize).min(2);
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        if f[idx].is_none() {
            let name = match idx {
                1 => font_kit::family_name::FamilyName::Serif,
                2 => font_kit::family_name::FamilyName::Monospace,
                _ => font_kit::family_name::FamilyName::SansSerif,
            };
            f[idx] = Some(
                crate::fonts::FontEngine::new()
                    .load_font(&[name], &font_kit::properties::Properties::new()),
            );
        }
        f[idx].clone().flatten()
    })
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

/// Walks the INHERITED text-align down the box tree and turns it into the
/// flex alignment of each box's own formatting context: a row (an inline
/// formatting context) aligns along its main axis, a column aligns its
/// children on the cross axis. Only boxes with no explicit alignment of
/// their own are touched, so a real `justify-content`/`text-align` rule
/// anywhere down the tree still wins.
fn propagate_text_align(tree: &mut LayoutTree) {
    fn walk(tree: &mut LayoutTree, id: taffy::NodeId, inherited: Option<u8>) {
        let effective = tree
            .paint_map
            .get(&id)
            .and_then(|p| p.text_align)
            .or(inherited);
        if let Some(align) = effective.filter(|&a| a != 0) {
            if let Ok(st) = tree.taffy.style(id) {
                let mut st = st.clone();
                let row = matches!(
                    st.flex_direction,
                    FlexDirection::Row | FlexDirection::RowReverse
                );
                let mut changed = false;
                if row && st.justify_content.is_none() {
                    st.justify_content = Some(if align == 1 {
                        taffy::style::JustifyContent::CENTER
                    } else {
                        taffy::style::JustifyContent::END
                    });
                    changed = true;
                } else if !row && st.align_items.is_none() {
                    st.align_items = Some(if align == 1 {
                        taffy::style::AlignItems::CENTER
                    } else {
                        taffy::style::AlignItems::END
                    });
                    changed = true;
                }
                if changed {
                    let _ = tree.taffy.set_style(id, st);
                }
            }
        }
        let kids = tree.taffy.children(id).unwrap_or_default();
        for k in kids {
            walk(tree, k, effective);
        }
    }
    walk(tree, tree.root_node, None);
}

/// Recomputes layout with real text measurement: resolves each text run's
/// inherited font size, then lets taffy size text leaves by wrapped extent.
/// Called after building the tree and after every cascade application.
pub fn remeasure(tree: &mut LayoutTree) {
    propagate_text_align(tree);
    // Pass 1: resolve font size down the box tree for text leaves, and
    // intrinsic sizes for images.
    let mut text_info: HashMap<taffy::NodeId, (String, f32, f32, bool, u8)> = HashMap::new();
    let mut img_info: HashMap<taffy::NodeId, (f32, f32)> = HashMap::new();
    fn resolve(
        node_id: taffy::NodeId,
        inherited: (f32, f32, bool, u8), // (font size, line-height; 0=natural, nowrap, family)
        tree: &LayoutTree,
        out: &mut HashMap<taffy::NodeId, (String, f32, f32, bool, u8)>,
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
                if let Some(nw) = paint.and_then(|p| p.nowrap) {
                    size.2 = nw;
                }
                size.3 = paint
                    .and_then(|p| p.family)
                    .unwrap_or_else(|| default_family(el.name.local.as_ref(), inherited.3));
                if el.name.local.as_ref() == "img" {
                    let attrs = el.attributes.borrow();
                    // width/height attributes win; else intrinsic dimensions.
                    let attr_px = |name: &str| {
                        attrs.get(name).and_then(|v| v.trim().parse::<f32>().ok())
                    };
                    let intrinsic = crate::images::effective_img_src(&attrs)
                        .and_then(|s| crate::images::get(&s))
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
                    out.insert(node_id, (text, inherited.0, inherited.1, inherited.2, inherited.3));
                }
            }
        }
        if let Ok(children) = tree.taffy.children(node_id) {
            for child in children {
                resolve(child, size, tree, out, imgs);
            }
        }
    }
    resolve(tree.root_node, (16.0, 0.0, false, 0), tree, &mut text_info, &mut img_info);

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
            let Some((text, font_size, line_mult, nowrap, family)) = text_info.get(&node_id) else {
                return Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) };
            };
            let wrap_width = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                _ => vw_cap,
            });
            let effective_wrap = if *nowrap { f32::MAX } else { wrap_width.max(1.0) };
            let fam_font = family_font(*family);
            let use_font = fam_font.as_deref().or(font.as_deref());
            let (w, h) = measure_text_family(use_font, *family, text, *font_size, *line_mult, effective_wrap);
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
    measure_text_family(font, 0, text, font_size, line_mult, max_width)
}

fn measure_text_family(
    font: Option<&font_kit::font::Font>,
    family: u8,
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

    // Advance cache: taffy's flexbox runs several measure passes per node
    // and the per-char glyph lookup was hot. Advances are in FONT UNITS
    // (size-independent); scale applies after.
    thread_local! {
        static ADVANCES: std::cell::RefCell<HashMap<(u8, char), f32>> = RefCell::new(HashMap::new());
    }
    use std::cell::RefCell;
    let advance_units = |c: char| -> f32 {
        ADVANCES.with(|m| {
            if let Some(a) = m.borrow().get(&(family, c)) {
                return *a;
            }
            let a = font
                .glyph_for_char(c)
                .and_then(|g| font.advance(g).ok())
                .map(|a| a.x())
                .unwrap_or(0.0);
            m.borrow_mut().insert((family, c), a);
            a
        })
    };

    let mut pen = 0.0f32;
    let mut lines = 1u32;
    let mut widest = 0.0f32;
    for word in text.split_whitespace() {
        let word_width: f32 = word.chars().map(|c| advance_units(c) * scale).sum();
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
