//! The box tree and the layout passes (moved from handlers/aether/src/layout/mod.rs): box generation from tag
//! defaults and `style=""` ([`build_tree`]), the cascade fold ([`apply_specs`]), and [`remeasure`] — formatting
//! contexts, inherited text properties, font-relative re-resolution, replaced sizes, the taffy passes, tables,
//! and the inline formatting contexts fed back until heights settle.

use crate::css::{self, ParseCx, SpecifiedStyle};
use crate::prelude::*;
use crate::{blockwidth, collapse, inline, table};
use crate::{Adv, Dom, DomNode, FontSel, FontWeight, Fonts, Host, NodeKind, NodeMap, PaintStyle, TextMode};
use taffy::prelude::*;

pub struct LayoutTree<H: Host> {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    /// Maps layout boxes back to DOM nodes.
    pub node_map: NodeMap<H::Node>,
    pub paint_map: NodeMap<PaintStyle<H>>,
    /// Viewport this tree lays out against (media queries + wrap width).
    pub viewport: (f32, f32),
    pub dirty: bool,
    /// Inline formatting contexts (CSS 2.2 §9.4.2) by their root box: the
    /// line-box fragments render paints in place of the root's children.
    pub inline: NodeMap<inline::InlineLayout>,
    /// Cells spanning rows, as (cell, the rows it spans), placed by the table pass and sized by
    /// `table::fix_rowspans` once the rows settle (was table.rs's thread-local).
    pub rowspans: Vec<(taffy::NodeId, Vec<taffy::NodeId>)>,
    /// AETHERSTYLE3 (SR103): each scroll container's scroll offset (css-overflow-3 §2.3), keyed by box; the
    /// host restores it from its per-element record after a relayout ([`crate::scroll`]).
    pub scroll: NodeMap<(f32, f32)>,
    /// AETHERSTYLE3 (SR103): where multi-column layout moved a block-level child of a multicol container
    /// from its single-column place (css-multicol-1 §3: the column box it fragments into); painters and hit
    /// tests add it to the child's origin ([`col_shift`]).
    pub col_shift: NodeMap<(f32, f32)>,
    /// AETHERSTYLE3 (SR103): each multicol container's used column geometry (the painter's column rules).
    pub multicol: NodeMap<crate::multicol::Columns>,
    /// AETHERSTYLE3B (SR115): a multicol container a `column-span: all` element splits — its column sets in
    /// order (css-multicol-1 §6.1); absent when the container is one set ([`LayoutTree::multicol`] alone).
    pub col_sets: NodeMap<Vec<crate::multicol::ColumnSet>>,
    /// AETHERSTYLE3C (SR122): a decorated box multi-column layout split across columns — its fragments in
    /// order (css-break-3 §5.4); painters draw the box once per fragment instead of at its single-column place.
    pub col_frags: NodeMap<Vec<crate::multicol::BoxFragment>>,
    /// AETHERPAINTPERF2 (SR132): the boxes taffy re-laid in the last [`remeasure`] / [`remeasure_kept`] — before each
    /// taffy pass, the dirty boxes reachable from the root through dirty boxes (a clean box is a cache hit and
    /// taffy does not descend into it), summed over the passes.
    pub relaid: u64,
    /// AETHERPAINTPERF2 (SR132): the host's measurement epoch the boxes were measured in (the host's font generation:
    /// a kept tree is laid out again only under the same one). Set and read by the host only.
    pub measure_epoch: u64,
    /// AETHERPAINTPERF2 (SR132): nonzero on a twin [`retarget`] built: the token of the kept tree it targets.
    pub kept_token: u64,
    /// AETHERPAINTPERF2 (SR132): each measured leaf's measure inputs at the last taffy pass (a kept tree re-measures
    /// a leaf only when they moved).
    measured: NodeMap<Measured>,
    /// AETHERPAINTPERF2 (SR132): the block-width placeholders (`blockwidth::apply`) a kept-tree remeasure left
    /// applied, with the specified style each replaced. No reader after layout reads those fields (width, the
    /// horizontal margins, align-self); [`apply_specs`] and the in-place [`remeasure`] put them back before
    /// they fold or lay out again.
    bw_pending: blockwidth::Saved,
}

/// The multicol displacement of box `id` (0, 0 when it is not in a balanced column).
pub fn col_shift<H: Host>(tree: &LayoutTree<H>, id: taffy::NodeId) -> (f32, f32) {
    tree.col_shift.get(&id).copied().unwrap_or((0.0, 0.0))
}

impl<H: Host> LayoutTree<H> {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// The cascaded style of each element (AETHERMETAL.md §M2's `ComputedStyles`): what [`layout`] folds onto the
/// element's boxes. `None` = no declaration applies.
pub trait ComputedStyles<H: Host> {
    fn style_of(&self, node: &H::Node) -> Option<Rc<SpecifiedStyle<H>>>;
}

/// THE entry (AETHERMETAL.md §M2): the box tree of `dom` from tag defaults and `style=""`, the cascaded
/// `styles` folded onto it, laid out at `viewport` with real text measurement.
pub fn layout<H: Host, D: Dom<Node = H::Node>, F: Fonts>(
    dom: &D,
    styles: &impl ComputedStyles<H>,
    fonts: &mut F,
    viewport: (f32, f32),
) -> LayoutTree<H> {
    let root = dom.root();
    let mut tree = build_tree::<H>(&root, viewport.0, viewport.1);
    let mut specs: Vec<(taffy::NodeId, Rc<SpecifiedStyle<H>>)> = Vec::new();
    for (tid, n) in &tree.node_map {
        if n.is_element() {
            if let Some(s) = styles.style_of(n) {
                specs.push((*tid, s));
            }
        }
    }
    apply_specs(&mut tree, &specs);
    remeasure(&mut tree, &*fonts);
    tree
}

/// The `Style` every box starts from. taffy 0.15's default `position` is `Static` (0.14's was `Relative`); a
/// relatively positioned box is the containing block of its absolutely positioned children, which is what the
/// box tree was built for (every parent a containing block), so boxes start `Relative`.
pub fn base_style() -> Style {
    Style { position: Position::Relative, ..Style::default() }
}

/// True when the iterator yields exactly one node and it is text.
fn exactly_one_text<N: DomNode>(mut it: impl Iterator<Item = N>) -> bool {
    matches!((it.next(), it.next()), (Some(n), None) if n.is_text())
}

/// Elements whose subtrees produce no boxes.
fn is_non_rendered(name: &str) -> bool {
    // noscript: scripting IS enabled here (page scripts run), so its
    // fallback content must not render. iframe/object/embed: no frame
    // support yet — an empty box is honest, raw content leaking is not.
    // option/optgroup: a <select>'s choices live in a popup, not in flow —
    // boxed, every select dumped its whole option list as page text.
    matches!(
        name,
        "head" | "script" | "style" | "title" | "meta" | "link" | "template"
            | "noscript" | "object" | "embed"
            | "option" | "optgroup"
    )
}

/// The (submit value, display label) of a `<select>`'s selected option (HTML §4.10.7 selectedness):
/// the last option whose selectedness is set — script-set selectedness (`option.selected`,
/// `select.value`) first, else the `selected` attribute — else the first option. Value is the
/// option's `value` attribute, falling back to its text; label is the option's text. Layout only
/// reads: it never writes attributes into the document.
pub fn select_selected_option<H: Host>(select: &H::Node) -> Option<(String, String)> {
    let mut first: Option<(String, String)> = None;
    let mut chosen: Option<(String, String)> = None;
    for desc in select.descendants() {
        if desc.tag().as_deref() != Some("option") {
            continue;
        }
        let label = desc.text().split_whitespace().collect::<Vec<_>>().join(" ");
        let attr_selected = desc.has_attr("selected");
        let value = desc.attr("value").unwrap_or_else(|| label.clone());
        let selected = H::option_selected(&desc).unwrap_or(attr_selected);
        let pair = (value, label);
        if selected {
            chosen = Some(pair.clone());
        }
        if first.is_none() {
            first = Some(pair);
        }
    }
    chosen.or(first)
}

/// Inline-level elements: they size to content and flow in wrapping rows.
pub fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "a" | "span" | "b" | "strong" | "i" | "em" | "u" | "s" | "code" | "small" | "big"
            | "sup" | "sub" | "label" | "abbr" | "cite" | "q" | "time" | "img" | "wbr" | "br"
            | "video" | "audio"
            | "mark" | "del" | "ins" | "strike" | "var" | "dfn" | "kbd" | "samp" | "tt"
            | "bdi" | "bdo" | "data" | "output" | "nobr" | "font" | "ruby"
            | "td" | "th" | "button" | "input" | "select"
    )
}

/// Quirks mode: no doctype at all, or anything other than a bare
/// `<!DOCTYPE html>`. Only the handful of rendering quirks this engine
/// actually implements key off it.
pub fn is_quirks<N: DomNode>(dom: &N) -> bool {
    let doc = if dom.kind() == NodeKind::Document {
        dom.clone()
    } else {
        let mut n = dom.clone();
        while let Some(p) = n.parent() {
            n = p;
        }
        n
    };
    for child in doc.children() {
        if let Some((name, public_id, system_id)) = child.doctype() {
            return !(name.eq_ignore_ascii_case("html") && public_id.is_empty() && system_id.is_empty());
        }
    }
    true
}

/// Table-internal boxes. They are neither block nor inline level: their
/// parent lays them out by table rules, so the inline/block machinery
/// (anonymous block boxes, the inline-with-block-children demotion) must
/// leave them alone.
pub(crate) fn is_table_part(name: &str) -> bool {
    matches!(
        name,
        "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption"
            | "colgroup" | "col"
    )
}

/// True when the node produces a box at all (mirrors the build-time skips).
fn generates_box<N: DomNode>(node: &N) -> bool {
    if let Some(tag) = node.tag() {
        if is_non_rendered(&tag) {
            return false;
        }
        if node.has_attr("hidden") || node.attr("aria-hidden").as_deref() == Some("true") {
            return false;
        }
        if tag == "input" && node.attr("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("hidden")) {
            return false;
        }
        true
    } else {
        node.is_text() && !node.text().trim().is_empty()
    }
}

/// A whitespace-only text node whose nearest box-generating siblings on
/// both sides are inline content (text or inline elements).
fn is_interelement_space<N: DomNode>(node: &N) -> bool {
    let inline_neighbour = |it: Vec<N>| it.into_iter().find(|n| generates_box(n)).is_some_and(|n| is_inline_node(&n));
    node.parent().is_some_and(|p| {
        p.tag().is_some_and(|t| !is_table_part(&t) || matches!(t.as_str(), "td" | "th" | "caption"))
    }) && inline_neighbour(node.preceding_siblings())
        && inline_neighbour(node.following_siblings())
}

/// True when a DOM node lays out as inline content (text or inline element).
pub fn is_inline_node<N: DomNode>(node: &N) -> bool {
    if node.is_text() {
        return true;
    }
    let Some(tag) = node.tag() else { return false };
    if !is_inline(&tag) {
        return false;
    }
    // Table cells flow by table rules whatever they contain.
    if is_table_part(&tag) {
        return true;
    }
    // Replaced media elements are inline boxes whatever their (never rendered) <source>
    // children are — testing those made a <video><source></video> a full-width block.
    if matches!(tag.as_str(), "video" | "audio") {
        return true;
    }
    // An inline element that CONTAINS block-level children is broken around
    // them by CSS; the practical approximation is to lay it out as a block.
    // Treated as inline it shrank to fit while its block children still
    // wanted 100% — <span id=footer> wrapping the whole page footer came out
    // half width.
    // Tag-level test only (no recursion): the check runs at every level of
    // the build, and a subtree-deep test would make it quadratic.
    !node.children().into_iter().any(|c| generates_box(&c) && c.tag().is_some_and(|t| !is_inline(&t)))
}

/// Builds the box tree and lays it out with real text measurement.
pub fn compute_layout_sized<H: Host, F: Fonts + ?Sized>(dom: &H::Node, vw: f32, vh: f32, fonts: &F) -> LayoutTree<H> {
    let mut tree = build_tree::<H>(dom, vw, vh);
    remeasure(&mut tree, fonts);
    tree
}

/// Builds the box tree WITHOUT the measuring layout pass. Callers that
/// immediately run a cascade (which remeasures at the end) use this — the
/// double full layout per page load was a profiled cost.
pub fn build_tree<H: Host>(dom: &H::Node, vw: f32, vh: f32) -> LayoutTree<H> {
    // Inline `style="height:100vh"` resolves against this same viewport.
    let cx = ParseCx::new(vw, vh);
    let mut taffy = taffy::TaffyTree::new();
    let mut node_map = NodeMap::new();
    let mut paint_map = NodeMap::new();

    let root_node = build_taffy_tree::<H>(dom, &mut taffy, &mut node_map, &mut paint_map, is_quirks(dom), &cx)
        .unwrap_or_else(|| taffy.new_leaf(base_style()).unwrap());

    LayoutTree {
        taffy,
        root_node,
        node_map,
        paint_map,
        viewport: (vw, vh),
        dirty: false,
        inline: NodeMap::new(),
        rowspans: Vec::new(),
        scroll: NodeMap::new(),
        col_shift: NodeMap::new(),
        multicol: NodeMap::new(),
        col_sets: NodeMap::new(),
        col_frags: NodeMap::new(),
        relaid: 0,
        measure_epoch: 0,
        kept_token: 0,
        measured: NodeMap::new(),
        bw_pending: Vec::new(),
    }
}

/// Builds the box (taffy node) subtree of one DOM node from TAG defaults and its `style` attribute;
/// `None` when the node generates no box. (The host's generated-content boxes are built through it too.)
pub fn build_taffy_tree<H: Host>(
    dom_node: &H::Node,
    taffy: &mut taffy::TaffyTree,
    node_map: &mut NodeMap<H::Node>,
    paint_map: &mut NodeMap<PaintStyle<H>>,
    quirks: bool,
    cx: &ParseCx,
) -> Option<taffy::NodeId> {
    let node_tag = dom_node.tag();
    if let Some(t) = node_tag.as_deref() {
        if is_non_rendered(t) {
            return None;
        }
        // The HTML hidden attribute / aria-hidden remove the subtree.
        if dom_node.has_attr("hidden") || dom_node.attr("aria-hidden").as_deref() == Some("true") {
            return None;
        }
        // <input type=hidden> is a form value carrier, not a control:
        // the UA sheet gives it display:none. Pages carry many of them
        // (tokens, locale, charset); boxed, they paint as a stack of
        // phantom fields above the real content.
        if t == "input" && dom_node.attr("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("hidden")) {
            return None;
        }
        // <audio> without `controls` is display:none in the UA sheet.
        if t == "audio" && !dom_node.has_attr("controls") {
            return None;
        }
    } else if matches!(dom_node.kind(), NodeKind::Comment | NodeKind::Doctype) {
        // Comments and the doctype are not
        // rendered (they generate no box). Boxed, an empty block between
        // inline siblings split their line into anonymous blocks and
        // moved absolutely positioned media off their insets.
        return None;
    } else if dom_node.is_text() {
        // Whitespace-only text produces no box — except between two
        // inline siblings, where it is the word space of the line
        // ("<b>a</b> <i>b</i>"; css-text-3 §4.1.1 collapses it to one
        // space, or to nothing, in the whitespace pass).
        if dom_node.text().trim().is_empty() && !is_interelement_space(dom_node) {
            return None;
        }
    }
    // <video>/<audio> are replaced elements: their <source>/<track>
    // children and fallback content never render.
    let replaced_media = node_tag.as_deref().is_some_and(|t| matches!(t, "video" | "audio"));

    let mut kids: Vec<(taffy::NodeId, bool)> = Vec::new();
    // A textarea's text is its VALUE, painted inside the control, not
    // flow content (the control is a leaf with an intrinsic size).
    let is_textarea = node_tag.as_deref() == Some("textarea");
    let dom_children = dom_node.children();
    for child in dom_children.iter().filter(|_| !is_textarea && !replaced_media) {
        if let Some(id) = build_taffy_tree::<H>(child, taffy, node_map, paint_map, quirks, cx) {
            kids.push((id, is_inline_node(child)));
        }
    }
    let any_inline = kids.iter().any(|&(_, i)| i);
    let any_block = kids.iter().any(|&(_, i)| !i);
    // Mixed inline and block siblings: CSS wraps each run of consecutive
    // inline children in an ANONYMOUS BLOCK BOX, which is what keeps them
    // flowing on a shared line. Without it the whole container fell back
    // to a column and every inline sibling got its own full-width row —
    // two submit buttons meant to sit side by side stacked instead.
    let parent_tag = node_tag.clone();
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
                align_items: taffy::style::AlignItems::BASELINE,
                size: Size { width: Dimension::percent(1.0), height: Dimension::auto() },
                ..base_style()
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

    // A text run that is its box's ONLY child may shrink to the box's
    // width and wrap inside it (its min-content is its widest word).
    // With shrink 0 its flex base size was its max-content width capped
    // at the VIEWPORT, so every paragraph narrower than the viewport
    // painted past its own right edge. Runs that share a line with
    // siblings keep shrink 0: shrunk side by side, each would wrap into
    // its own column instead of flowing onto the next line.
    if let [only] = child_ids.as_slice() {
        let only_is_text = exactly_one_text(dom_children.iter().filter(|c| generates_box(*c)).cloned());
        if only_is_text {
            if let Ok(st) = taffy.style(*only) {
                let mut st = st.clone();
                st.flex_shrink = 1.0;
                let _ = taffy.set_style(*only, st);
            }
        }
    }

    let tag = node_tag.clone().unwrap_or_default();
    let inline = is_inline_node(dom_node);
    // A box whose rendered children are all inline content flows them as
    // a wrapping row (the inline-formatting-context approximation).
    let children_inline = !dom_children.is_empty()
        && dom_children
            .iter()
            .filter(|c| c.is_element() || c.is_text() && !c.text().trim().is_empty())
            .all(|c| is_inline_node(c));

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
            taffy::style::AlignItems::BASELINE
        } else {
            taffy::style::AlignItems::NORMAL
        },
        // Inline boxes keep their measured width; the row wraps instead
        // of shrinking them (shrunk text would draw more lines than the
        // measured height and overlap the next block).
        flex_shrink: if inline { 0.0 } else { 1.0 },
        // CSS initial `box-sizing: content-box` (taffy defaults to
        // border-box). html.css makes push buttons and selects
        // border-box; their UA min sizes below are border-box sizes.
        box_sizing: if ua_border_box(dom_node, &tag) {
            taffy::style::BoxSizing::BorderBox
        } else {
            taffy::style::BoxSizing::ContentBox
        },
        size: Size {
            width: if inline { Dimension::auto() } else { Dimension::percent(1.0) },
            height: Dimension::auto(),
        },
        // No UA minimum: an empty block box is zero-tall in CSS (form
        // controls get their intrinsic sizes in ua_control below).
        min_size: Size { width: LengthPercentageAuto::auto(), height: LengthPercentageAuto::auto() },
        margin: {
            let [t, r, b, l] = if inline { [0.0; 4] } else { ua_margin(&tag, nested_list(dom_node)) };
            Rect {
                top: LengthPercentageAuto::length(t),
                right: LengthPercentageAuto::length(r),
                bottom: LengthPercentageAuto::length(b),
                left: LengthPercentageAuto::length(l),
            }
        },
        padding: if matches!(tag.as_str(), "td" | "th") {
            // html.css: `td, th { padding: 1px }`.
            Rect::length(1.0)
        } else {
            Rect {
                left: LengthPercentage::length(if !inline && matches!(tag.as_str(), "ul" | "ol" | "menu" | "dir") { 40.0 } else { 0.0 }),
                right: LengthPercentage::length(0.0),
                top: LengthPercentage::length(0.0),
                bottom: LengthPercentage::length(0.0),
            }
        },
        border: if tag == "hr" {
            Rect { left: LengthPercentage::length(1.0), right: LengthPercentage::length(1.0), top: LengthPercentage::length(1.0), bottom: LengthPercentage::length(1.0) }
        } else {
            Rect::zero()
        },
        ..base_style()
    };

    ua_control(dom_node, &tag, &mut style);

    // <br>: a zero-height full-width item forces a wrap break in the
    // inline row without adding vertical space of its own.
    if tag == "br" {
        style.size.width = Dimension::percent(1.0);
        style.size.height = Dimension::length(0.0);
        style.min_size = Size { width: LengthPercentageAuto::auto(), height: LengthPercentageAuto::length(0.0) };
    }

    // Inline style="..." — paint properties plus width/height.
    let mut paint = PaintStyle::<H>::default();
    if !inline {
        if let Some(k) = ua_margin_em(&tag, nested_list(dom_node)) {
            let built = ua_margin(&tag, nested_list(dom_node))[0];
            paint.ua_vmargin = Some((k, built));
        }
    }
    if control_kind(dom_node) == Some(Control::Button) {
        // Chromium's ButtonFace.
        paint.background = Some((239, 239, 239));
    }
    if tag == "mark" {
        // html.css: `mark { background-color: yellow; color: black }`.
        paint.background = Some((255, 255, 0));
        paint.color = Some((0, 0, 0));
    }
    if tag == "hr" {
        // html.css: `hr { color: gray; border-style: inset; border-width: 1px }`
        // — an inset stroke paints its top/left darker than its bottom/right.
        let (dark, light) = ((154, 154, 154), (238, 238, 238));
        paint.border = Some([Some((1.0, dark)), Some((1.0, light)), Some((1.0, light)), Some((1.0, dark))]);
    }
    if dom_node.is_element() {
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
        if let Some(a) = dom_node.attr("align") {
            match a.trim().to_ascii_lowercase().as_str() {
                "center" | "middle" => paint.text_align = Some(1),
                "right" => paint.text_align = Some(2),
                "left" => paint.text_align = Some(4),
                _ => {}
            }
        }
        if let Some(inline) = dom_node.attr("style") {
            let saved = paint.text_align;
            apply_inline_style::<H>(&inline, &mut style, &mut paint, cx);
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

/// The kind of form control a node is, by Chromium's UA rendering:
/// text-like field, checkbox, radio, push button, select, textarea.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Field,
    Checkbox,
    Radio,
    Button,
    Select,
    Textarea,
}

pub fn control_kind<N: DomNode>(node: &N) -> Option<Control> {
    let tag = node.tag()?;
    match tag.as_str() {
        "button" => Some(Control::Button),
        "select" => Some(Control::Select),
        "textarea" => Some(Control::Textarea),
        "input" => {
            let t = node.attr("type").unwrap_or_else(|| "text".to_string()).trim().to_ascii_lowercase();
            Some(match t.as_str() {
                "checkbox" => Control::Checkbox,
                "radio" => Control::Radio,
                "submit" | "reset" | "button" | "image" | "file" | "color" => Control::Button,
                _ => Control::Field,
            })
        }
        _ => None,
    }
}

/// Chromium's control font: 13.333px (`font: -webkit-small-control`).
pub const CONTROL_FONT_PX: f32 = 13.333;

/// Single-line advance of `text` at `size` in the given family.
fn text_advance<F: Fonts + ?Sized>(fonts: &F, text: &str, family: u16, size: f32) -> f32 {
    let sel = FontSel::new(family, 400, false);
    if !fonts.has_face(sel) {
        return text.len() as f32 * size * 0.5;
    }
    fonts.measure(text, &Adv::new(sel, size, 0.0))
}

fn control_line_height<F: Fonts + ?Sized>(fonts: &F, family: u16) -> f32 {
    let sel = FontSel::new(family, 400, false);
    if fonts.has_face(sel) { fonts.line_height(sel, CONTROL_FONT_PX, 0.0) } else { 15.0 }
}

/// UA (html.css + Chromium's control theme) box metrics of form controls:
/// text fields `padding: 1px 2px; border: 2px inset` with one line of the
/// 13.333px control font and the `size=20` default width; checkbox and
/// radio 13x13 with their 3px/4px (radio 3px 3px 0 5px) margins; buttons
/// `padding: 1px 6px; border: 2px outset`; select a 1px border around its
/// label and the arrow; textarea `padding: 2px; border: 1px` sized by
/// rows x cols of the monospace control font.
fn ua_control<N: DomNode>(node: &N, _tag: &str, style: &mut Style) {
    let Some(kind) = control_kind(node) else { return };
    let lp = LengthPercentage::length;
    let rect = |t: f32, r: f32, b: f32, l: f32| Rect { top: lp(t), right: lp(r), bottom: lp(b), left: lp(l) };
    match kind {
        Control::Field => {
            style.padding = rect(1.0, 2.0, 1.0, 2.0);
            style.border = rect(2.0, 2.0, 2.0, 2.0);
        }
        Control::Checkbox | Control::Radio => {
            style.size = Size { width: Dimension::length(13.0), height: Dimension::length(13.0) };
            style.padding = rect(0.0, 0.0, 0.0, 0.0);
            style.border = rect(0.0, 0.0, 0.0, 0.0);
            let m = if kind == Control::Radio { [3.0, 3.0, 0.0, 5.0] } else { [3.0, 3.0, 3.0, 4.0] };
            style.margin = Rect {
                top: LengthPercentageAuto::length(m[0]),
                right: LengthPercentageAuto::length(m[1]),
                bottom: LengthPercentageAuto::length(m[2]),
                left: LengthPercentageAuto::length(m[3]),
            };
            style.align_self = Some(taffy::style::AlignSelf::CENTER);
        }
        Control::Button => {
            style.padding = rect(1.0, 6.0, 1.0, 6.0);
            style.border = rect(2.0, 2.0, 2.0, 2.0);
        }
        Control::Select => {
            style.padding = rect(0.0, 0.0, 0.0, 0.0);
            style.border = rect(1.0, 1.0, 1.0, 1.0);
        }
        Control::Textarea => {
            style.padding = rect(2.0, 2.0, 2.0, 2.0);
            style.border = rect(1.0, 1.0, 1.0, 1.0);
        }
    }
}

/// Intrinsic CONTENT size of a leaf form control (taffy adds its padding
/// and border, so author `box-sizing` works as in CSS): one control-font
/// line by the `size=20` average-character width for a field, the label
/// for a button-like input, the selected label plus the arrow for a
/// select, rows x cols of the monospace control font for a textarea.
fn control_intrinsic<H: Host, F: Fonts + ?Sized>(node: &H::Node, fonts: &F) -> Option<(f32, f32)> {
    let kind = control_kind(node)?;
    let attr = |name: &str| node.attr(name);
    let lh = control_line_height(fonts, 0);
    match kind {
        Control::Field => {
            let size = attr("size").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(20.0);
            Some((size * 7.45, lh))
        }
        Control::Button if node.tag().as_deref() == Some("input") => {
            let t = attr("type").unwrap_or_default().to_ascii_lowercase();
            let label = attr("value").unwrap_or_else(|| if t == "reset" { "Reset".into() } else { "Submit".into() });
            Some((text_advance(fonts, &label, 0, CONTROL_FONT_PX), lh))
        }
        Control::Select => {
            let label = select_selected_option::<H>(node).map(|(_, l)| l).unwrap_or_default();
            Some((text_advance(fonts, &label, 0, CONTROL_FONT_PX) + 4.0 + 20.0, lh + 2.0))
        }
        Control::Textarea => {
            let rows = attr("rows").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(2.0);
            let cols = attr("cols").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(20.0);
            let cw = text_advance(fonts, "0", 2, CONTROL_FONT_PX);
            Some((cols * cw + 17.0, rows * control_line_height(fonts, 2)))
        }
        _ => None,
    }
}

/// Controls the UA sheet sizes as border-box: buttons, selects, and the
/// button-like / checkable inputs.
fn ua_border_box<N: DomNode>(node: &N, tag: &str) -> bool {
    match tag {
        "button" | "select" => true,
        "input" => node.is_element() && {
            let t = node.attr("type").unwrap_or_else(|| "text".to_string()).trim().to_ascii_lowercase();
            matches!(t.as_str(), "submit" | "reset" | "button" | "checkbox" | "radio" | "image" | "color" | "file")
        },
        _ => false,
    }
}

/// True when a list sits inside another list: the UA sheet drops the
/// block margins of nested lists (`ul ul, ol ul, ... { margin-block: 0 }`).
fn nested_list<N: DomNode>(node: &N) -> bool {
    let is_list = |n: &N| n.tag().is_some_and(|t| matches!(t.as_str(), "ul" | "ol" | "menu" | "dir" | "dl"));
    is_list(node) && node.ancestors().iter().any(|a| is_list(a) && a.tag().is_some_and(|t| t != "dl"))
}

/// UA default margins [top, right, bottom, left] in px — Chromium's html.css
/// (the WHATWG rendering section, §15.3.3 / §15.3.6 / §15.3.8): `em` values
/// resolve against the element's own UA font size.
fn ua_margin(tag: &str, nested_list: bool) -> [f32; 4] {
    let em = match tag {
        "pre" | "listing" | "xmp" | "plaintext" => 13.0, // monospace default size
        _ => default_font_size(tag, 16.0),
    };
    let v = |k: f32| [k * em, 0.0, k * em, 0.0];
    match tag {
        "body" => [8.0; 4],
        "p" | "dl" | "pre" | "listing" | "xmp" | "plaintext" => v(1.0),
        "ul" | "ol" | "menu" | "dir" => if nested_list { [0.0; 4] } else { v(1.0) },
        "blockquote" | "figure" => [em, 40.0, em, 40.0],
        "dd" => [0.0, 0.0, 0.0, 40.0],
        "h1" => v(0.67),
        "h2" => v(0.83),
        "h3" => v(1.0),
        "h4" => v(1.33),
        "h5" => v(1.67),
        "h6" => v(2.33),
        "hr" => v(0.5),
        "fieldset" => [0.0, 2.0, 0.0, 2.0],
        // Generic blocks have NO UA margin in CSS. (An old 2px-all-round
        // default accumulated once per nesting level.)
        _ => [0.0; 4],
    }
}

/// The em factor of an element's UA vertical margins (None = no em margin).
fn ua_margin_em(tag: &str, nested_list: bool) -> Option<f32> {
    match tag {
        "p" | "dl" | "pre" | "listing" | "xmp" | "plaintext" | "blockquote" | "figure" | "h3" => Some(1.0),
        "ul" | "ol" | "menu" | "dir" if !nested_list => Some(1.0),
        "h1" => Some(0.67),
        "h2" => Some(0.83),
        "h4" => Some(1.33),
        "h5" => Some(1.67),
        "h6" => Some(2.33),
        "hr" => Some(0.5),
        _ => None,
    }
}

/// UA default font FAMILY per element (html.css): code-ish tags are `monospace`; form controls take
/// Chromium's `-webkit-small-control` system font, whose computed family on Linux is `Arial`.
pub fn default_family<H: Host>(tag: &str, inherited: u16) -> u16 {
    match tag {
        "code" | "pre" | "kbd" | "samp" | "tt" | "textarea" | "listing" | "xmp" | "plaintext" => crate::MONO,
        "input" | "select" | "button" => H::control_family(),
        _ => inherited,
    }
}

/// UA default white-space per element (html.css): pre-ish elements
/// preserve, `nobr` does not wrap; everything else inherits.
pub fn default_white_space(tag: &str, inherited: u8) -> u8 {
    match tag {
        "pre" | "listing" | "xmp" | "plaintext" => 2,
        "nobr" => 1,
        "textarea" => 3,
        _ => inherited,
    }
}

/// UA default line-through (html.css: s, strike, del).
pub fn default_line_through(tag: &str, inherited: bool) -> bool {
    inherited || matches!(tag, "s" | "strike" | "del")
}

/// UA default font-weight per element (html.css: `b, strong { font-weight: bolder }`, headings and
/// `th` bold), shared by the measurer and the painter.
pub fn default_weight(tag: &str, inherited: u16) -> u16 {
    match tag {
        "b" | "strong" => FontWeight::Bolder.resolve(inherited),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => 700,
        _ => inherited,
    }
}

/// The inherited direction unless the element's `dir` attribute sets it (HTML §3.2.6.4 → the UA sheet's
/// `[dir=rtl] { direction: rtl }`).
pub fn default_rtl<N: DomNode>(node: &N, inherited: bool) -> bool {
    if !node.is_element() {
        return inherited;
    }
    match node.attr("dir").map(|d| d.trim().to_ascii_lowercase()) {
        Some(d) if d == "rtl" => true,
        Some(d) if d == "ltr" => false,
        _ => inherited,
    }
}

/// UA default italic per element, shared by the measurer and the painter.
pub fn default_italic(tag: &str, inherited: bool) -> bool {
    inherited || matches!(tag, "i" | "em" | "cite" | "var" | "dfn" | "address")
}

/// The used font size of an element with no author `font-size`: the UA
/// size for its tag, and the generic-monospace rule — an element whose
/// family switches to `monospace` from a proportional parent drops from
/// the 16px default to 13px (Chromium's default fixed font size), so
/// `<pre>`/`<code>` text is 13/16 of its surroundings.
pub fn ua_font_size(tag: &str, inherited: f32, inherited_family: u16, own_family: u16) -> f32 {
    let s = default_font_size(tag, inherited);
    let control = matches!(tag, "input" | "select" | "textarea" | "button");
    if own_family == 2 && inherited_family != 2 && !control { s * 13.0 / 16.0 } else { s }
}

/// UA default font sizes per element (shared with the renderer).
pub fn default_font_size(tag: &str, inherited: f32) -> f32 {
    match tag {
        // html.css: h1 2em, h2 1.5em, h3 1.17em, h4 1em, h5 .83em, h6 .67em
        // of the PARENT's size; `small` is `smaller` (/1.2).
        "h1" => inherited * 2.0,
        "h2" => inherited * 1.5,
        "h3" => inherited * 1.17,
        "h4" => inherited,
        "h5" => inherited * 0.83,
        "h6" => inherited * 0.67,
        "small" => inherited / 1.2,
        "input" | "select" | "textarea" | "button" => CONTROL_FONT_PX,
        // vertical-align: super/sub with font-size: smaller (html.css).
        "sup" | "sub" => inherited / 1.2,
        _ => inherited,
    }
}

/// Walks the INHERITED text-align down the box tree and turns it into the
/// flex alignment of each box's own formatting context: a row (an inline
/// formatting context) aligns along its main axis, a column aligns its
/// children on the cross axis. Only boxes with no explicit alignment of
/// their own are touched, so a real `justify-content`/`text-align` rule
/// anywhere down the tree still wins.
fn propagate_text_align<H: Host>(tree: &mut LayoutTree<H>) {
    fn walk<H: Host>(tree: &mut LayoutTree<H>, id: taffy::NodeId, inherited: Option<u8>) {
        let effective = tree.paint_map.get(&id).and_then(|p| p.text_align).or(inherited);
        if let Some(align) = effective.filter(|&a| a != 0 && a != 4) {
            if let Ok(st) = tree.taffy.style(id) {
                let mut st = st.clone();
                let row = matches!(st.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
                let mut changed = false;
                if row && st.justify_content == taffy::style::JustifyContent::NORMAL {
                    st.justify_content = if align == 1 {
                        taffy::style::JustifyContent::CENTER
                    } else {
                        taffy::style::JustifyContent::END
                    };
                    changed = true;
                } else if !row && st.align_items == taffy::style::AlignItems::NORMAL {
                    st.align_items = if align == 1 {
                        taffy::style::AlignItems::CENTER
                    } else {
                        taffy::style::AlignItems::END
                    };
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
    let root = tree.root_node;
    walk(tree, root, None);
}

/// Block-level by the cascade (the author's outer display), else by tag; anonymous boxes are block boxes.
pub fn is_block_level<H: Host>(tree: &LayoutTree<H>, id: taffy::NodeId) -> bool {
    let Some(n) = tree.node_map.get(&id) else { return true };
    let Some(tag) = n.tag() else { return false };
    match tree.paint_map.get(&id).and_then(|p| p.display_kind) {
        Some(k) => k == 1,
        None => !is_inline_node(n) || matches!(tag.as_str(), "td" | "th"),
    }
}

/// The previous (`next` = false) or next sibling box of `id` in the box tree.
fn neighbour_box<H: Host>(tree: &LayoutTree<H>, id: taffy::NodeId, next: bool) -> Option<taffy::NodeId> {
    let parent = tree.taffy.parent(id)?;
    let kids = tree.taffy.children(parent).ok()?;
    let i = kids.iter().position(|&k| k == id)?;
    if next { kids.get(i + 1).copied() } else { i.checked_sub(1).and_then(|j| kids.get(j).copied()) }
}

/// css-text-3 §4.1.1 phase I across element boundaries: within one inline
/// formatting context, a collapsible space at the start of a run is
/// removed when the content before it already ended in a space (or at the
/// start of the line), and the last run's trailing space hangs (removed).
/// The surviving spaces become the runs' `ws_lead`/`ws_trail` flags, which
/// the line breaker turns into real advances.
fn collapse_inline_whitespace<H: Host>(tree: &mut LayoutTree<H>) {
    struct Ifc {
        prev_space: bool,
        last_text: Option<taffy::NodeId>,
    }
    fn end_ifc<H: Host>(tree: &mut LayoutTree<H>, ifc: &mut Ifc) {
        if let Some(t) = ifc.last_text.take() {
            tree.paint_map.entry(t).or_default().ws_trail = Some(false);
        }
        ifc.prev_space = true;
    }
    fn walk<H: Host>(tree: &mut LayoutTree<H>, id: taffy::NodeId, ws: u8, ifc: &mut Ifc) {
        let dom = tree.node_map.get(&id).cloned();
        let mut ws = ws;
        let mut block = dom.is_none(); // anonymous boxes wrap block-level runs
        if let Some(n) = &dom {
            if let Some(tag) = n.tag() {
                ws = tree.paint_map.get(&id).and_then(|p| p.white_space).unwrap_or_else(|| default_white_space(&tag, ws));
                block = is_block_level(tree, id);
                if !block && (tag == "br" || inline::is_atomic(tree, id)) {
                    // Atomic inline: content that is not a space.
                    ifc.prev_space = tag == "br";
                    ifc.last_text = None;
                    return;
                }
            } else if n.is_text() {
                let raw = n.text();
                let entry = tree.paint_map.entry(id).or_default();
                if !matches!(ws, 0 | 1 | 4) {
                    entry.ws_lead = Some(false);
                    entry.ws_trail = Some(false);
                    ifc.prev_space = raw.ends_with(' ');
                    ifc.last_text = None;
                    return;
                }
                let starts = raw.starts_with(|c: char| c.is_ascii_whitespace());
                let ends = raw.ends_with(|c: char| c.is_ascii_whitespace());
                if raw.trim().is_empty() {
                    // Next to a block-level box (author `display: block` on
                    // a tag Aether builds as inline) the space is not in any
                    // line at all.
                    let beside_block = [neighbour_box(tree, id, false), neighbour_box(tree, id, true)]
                        .into_iter()
                        .any(|n| n.is_some_and(|n| is_block_level(tree, n)));
                    let entry = tree.paint_map.entry(id).or_default();
                    let lead = !ifc.prev_space && !beside_block;
                    entry.ws_lead = Some(lead);
                    entry.ws_trail = Some(false);
                    if lead {
                        ifc.prev_space = true;
                        ifc.last_text = Some(id);
                    }
                    // A hanging space at the end of the line: handled by
                    // end_ifc clearing `trail`; a lone lead is cleared below.
                    return;
                }
                entry.ws_lead = Some(starts && !ifc.prev_space);
                entry.ws_trail = Some(ends);
                ifc.prev_space = ends;
                ifc.last_text = Some(id);
                return;
            }
        }
        if block {
            end_ifc(tree, ifc);
            let mut inner = Ifc { prev_space: true, last_text: None };
            for k in tree.taffy.children(id).unwrap_or_default() {
                walk(tree, k, ws, &mut inner);
            }
            end_ifc(tree, &mut inner);
            ifc.prev_space = true;
            ifc.last_text = None;
        } else {
            for k in tree.taffy.children(id).unwrap_or_default() {
                walk(tree, k, ws, ifc);
            }
        }
    }
    let root = tree.root_node;
    let mut ifc = Ifc { prev_space: true, last_text: None };
    walk(tree, root, 0, &mut ifc);
}

/// Inline-level or block-level by the cascade (css-display-3 §2): the
/// author's outer display when given, else the tag default.
pub(crate) fn is_inline_level<H: Host>(tree: &LayoutTree<H>, id: taffy::NodeId) -> bool {
    let Some(n) = tree.node_map.get(&id) else { return false }; // anonymous block
    if n.is_text() {
        return true;
    }
    match tree.paint_map.get(&id).and_then(|p| p.display_kind) {
        Some(k) => k != 1,
        None => is_inline_node(n),
    }
}

/// The box tree is built from TAG defaults before the cascade runs; an
/// author `display` that changes a box's outer type (a `label` made
/// `display: block`, an `li` made `inline-block`) changes its parent's
/// formatting context (CSS 2.2 §9.2.1.1). This pass re-derives each
/// affected container: all-inline children flow as a wrapping line row;
/// mixed children make a block column whose inline runs are wrapped in
/// anonymous block boxes. Only containers holding a child whose cascaded
/// type contradicts its tag are touched; anonymous boxes count as block,
/// so a second pass finds nothing to do.
fn fix_display_contexts<H: Host>(tree: &mut LayoutTree<H>) {
    let ids: Vec<taffy::NodeId> = tree.node_map.keys().copied().collect();
    for id in ids {
        let Some(n) = tree.node_map.get(&id) else { continue };
        let Some(tag) = n.tag() else { continue };
        if is_table_part(&tag) {
            continue;
        }
        if tree.paint_map.get(&id).and_then(|p| p.flex_container) == Some(true) {
            continue;
        }
        let mut kids = tree.taffy.children(id).unwrap_or_default();
        if kids.is_empty() {
            continue;
        }
        // An anonymous line box the builder made around an inline run that
        // now holds a block-level child is dissolved into this container.
        let block_in = |tree: &LayoutTree<H>, k: taffy::NodeId| {
            tree.node_map.get(&k).is_some_and(|kn| {
                kn.is_element() && tree.paint_map.get(&k).and_then(|p| p.display_kind) == Some(1) && is_inline_node(kn)
            })
        };
        if kids.iter().any(|&k| !tree.node_map.contains_key(&k) && tree.taffy.children(k).unwrap_or_default().iter().any(|&g| block_in(tree, g))) {
            let mut flat = Vec::new();
            for &k in &kids {
                let grand = tree.taffy.children(k).unwrap_or_default();
                if !tree.node_map.contains_key(&k) && grand.iter().any(|&g| block_in(tree, g)) {
                    let _ = tree.taffy.set_children(k, &[]);
                    flat.extend(grand);
                } else {
                    flat.push(k);
                }
            }
            let _ = tree.taffy.set_children(id, &flat);
            kids = flat;
        }
        let contradicts = kids.iter().any(|&k| {
            tree.node_map.get(&k).is_some_and(|kn| {
                kn.is_element() && tree.paint_map.get(&k).and_then(|p| p.display_kind).is_some_and(|d| (d != 1) != is_inline_node(kn))
            })
        });
        if !contradicts {
            continue;
        }
        // The children's own boxes follow their new outer type.
        for &k in &kids {
            let Some(kn) = tree.node_map.get(&k) else { continue };
            if !kn.is_element() {
                continue;
            }
            let Some(d) = tree.paint_map.get(&k).and_then(|p| p.display_kind) else { continue };
            let Ok(st) = tree.taffy.style(k) else { continue };
            let mut st = st.clone();
            if d == 1 && is_inline_node(kn) && st.size.width == Dimension::auto() {
                st.size.width = Dimension::percent(1.0);
                st.flex_shrink = 1.0;
            } else if d != 1 && !is_inline_node(kn) && st.size.width == Dimension::percent(1.0) {
                st.size.width = Dimension::auto();
                st.flex_shrink = 0.0;
            }
            let _ = tree.taffy.set_style(k, st);
        }
        let inline: Vec<bool> = kids.iter().map(|&k| is_inline_level(tree, k)).collect();
        let Ok(st) = tree.taffy.style(id) else { continue };
        let mut st = st.clone();
        if inline.iter().all(|&i| i) {
            st.flex_direction = FlexDirection::Row;
            st.flex_wrap = taffy::style::FlexWrap::Wrap;
            if st.align_items == taffy::style::AlignItems::NORMAL {
                st.align_items = taffy::style::AlignItems::BASELINE;
            }
            let _ = tree.taffy.set_style(id, st);
            continue;
        }
        st.flex_direction = FlexDirection::Column;
        st.flex_wrap = taffy::style::FlexWrap::NoWrap;
        if st.align_items == taffy::style::AlignItems::BASELINE {
            st.align_items = taffy::style::AlignItems::NORMAL;
        }
        let _ = tree.taffy.set_style(id, st);
        // Wrap each run of inline children (two or more, or any text) in
        // an anonymous block box.
        let mut out: Vec<taffy::NodeId> = Vec::new();
        let mut run: Vec<taffy::NodeId> = Vec::new();
        let flush = |run: &mut Vec<taffy::NodeId>, out: &mut Vec<taffy::NodeId>, tree: &mut LayoutTree<H>| {
            // Spaces at the edges of a run sit next to block boxes: they
            // are in no line (§4.1.1) and must not make one.
            let ws_only = |tree: &LayoutTree<H>, k: &taffy::NodeId| {
                tree.node_map.get(k).is_some_and(|n| n.is_text() && n.text().trim().is_empty())
            };
            while run.first().is_some_and(|k| ws_only(tree, k)) {
                let k = run.remove(0);
                let _ = tree.taffy.remove_child(id, k);
            }
            while run.last().is_some_and(|k| ws_only(tree, k)) {
                let k = run.pop().unwrap();
                let _ = tree.taffy.remove_child(id, k);
            }
            let has_text = run.iter().any(|k| tree.node_map.get(k).is_some_and(|n| n.is_text()));
            if run.len() < 2 && !has_text {
                out.append(run);
                return;
            }
            let anon = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_wrap: taffy::style::FlexWrap::Wrap,
                align_items: taffy::style::AlignItems::BASELINE,
                size: Size { width: Dimension::percent(1.0), height: Dimension::auto() },
                box_sizing: taffy::style::BoxSizing::ContentBox,
                ..base_style()
            };
            // Detach first: a taffy node has one parent.
            for k in run.iter() {
                let _ = tree.taffy.remove_child(id, *k);
            }
            if let Ok(a) = tree.taffy.new_with_children(anon, run) {
                tree.paint_map.insert(a, PaintStyle::default());
                out.push(a);
            } else {
                out.append(run);
            }
            run.clear();
        };
        for (i, &k) in kids.iter().enumerate() {
            if inline[i] {
                run.push(k);
            } else {
                flush(&mut run, &mut out, tree);
                out.push(k);
            }
        }
        flush(&mut run, &mut out, tree);
        let _ = tree.taffy.set_children(id, &out);
    }
}

/// UA margins are `em` of the element's own font size: the builder set
/// them against 16px; once font sizes are resolved, a margin side still
/// holding the built value (no author rule replaced it) is rescaled.
fn rescale_ua_margins<H: Host>(tree: &mut LayoutTree<H>, sizes: &NodeMap<f32>) {
    for (&id, &fs) in sizes {
        if fs < 0.0 {
            // sup/sub: Blink's shift (parent/3 + 1 up, parent/5 + 1 down)
            // as margin on the shifted side, so the line box contains it.
            let parent = -fs;
            let sup = tree.node_map.get(&id).and_then(|n| n.tag().map(|t| t == "sup")).unwrap_or(false);
            if let Ok(st) = tree.taffy.style(id) {
                let mut st = st.clone();
                if sup {
                    st.margin.top = LengthPercentageAuto::length(parent / 3.0 + 1.0);
                } else {
                    st.margin.bottom = LengthPercentageAuto::length(parent / 5.0 + 1.0);
                }
                let _ = tree.taffy.set_style(id, st);
            }
            continue;
        }
        let Some((k, built)) = tree.paint_map.get(&id).and_then(|p| p.ua_vmargin) else { continue };
        let want = k * fs;
        let Ok(st) = tree.taffy.style(id) else { continue };
        let b = LengthPercentageAuto::length(built);
        if (st.margin.top != b && st.margin.bottom != b) || (want - built).abs() < 0.001 {
            continue;
        }
        let mut st = st.clone();
        if st.margin.top == b {
            st.margin.top = LengthPercentageAuto::length(want);
        }
        if st.margin.bottom == b {
            st.margin.bottom = LengthPercentageAuto::length(want);
        }
        let _ = tree.taffy.set_style(id, st);
        if let Some(p) = tree.paint_map.get_mut(&id) {
            p.ua_vmargin = Some((k, want));
        }
    }
}

/// The min-content width of a box's border box (css-sizing-3 §5.1): the
/// widest unbreakable piece of its content — a text run broken at every
/// opportunity, an atomic box's width — plus its own padding and border.
fn min_content_width<H: Host, F: Fonts + ?Sized>(tree: &LayoutTree<H>, id: taffy::NodeId, text: &NodeMap<TextRun>, fonts: &F) -> f32 {
    if let Some(run) = text.get(&id) {
        return measure_text_run(run, 0.0, fonts).0;
    }
    let Ok(l) = tree.taffy.layout(id) else { return 0.0 };
    let edges = l.padding.left + l.padding.right + l.border.left + l.border.right;
    let kids = tree.taffy.children(id).unwrap_or_default();
    if kids.is_empty() {
        // A leaf: replaced/control boxes keep their width; empty boxes are 0
        // unless the author sized them.
        return if tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true)
            || tree
                .node_map
                .get(&id)
                .and_then(|n| n.tag().map(|t| matches!(t.as_str(), "img" | "input" | "select" | "button" | "textarea" | "video" | "svg" | "canvas" | "iframe")))
                .unwrap_or(false)
        {
            l.size.width
        } else {
            edges
        };
    }
    let inner = kids
        .iter()
        .map(|&k| {
            let m = tree.taffy.layout(k).map(|l| l.margin.left.max(0.0) + l.margin.right.max(0.0)).unwrap_or(0.0);
            min_content_width(tree, k, text, fonts) + m
        })
        .fold(0.0f32, f32::max);
    if tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true) {
        return l.size.width.max(inner + edges);
    }
    inner + edges
}

/// The computed font-size of an element (css-fonts-4 §2.5): an em/% value
/// of the parent's size, rem of the root's, an absolute value as given, else
/// the UA default for the tag.
fn computed_font_size<H: Host>(paint: Option<&PaintStyle<H>>, tag: &str, parent: f32, parent_family: u16, own_family: u16, parent_absolute: bool, root_fs: f32) -> f32 {
    match paint.and_then(|p| p.font_size_rel) {
        Some((0, f)) => parent * f,
        Some((1, f)) => root_fs * f,
        _ => paint.and_then(|p| p.font_size).unwrap_or_else(|| {
            if parent_absolute {
                default_font_size(tag, parent)
            } else {
                ua_font_size(tag, parent, parent_family, own_family)
            }
        }),
    }
}

/// The root element's computed font-size (16px unless the author sizes
/// `html`; a rem there is of the initial 16px).
fn root_font_size<H: Host>(tree: &LayoutTree<H>) -> f32 {
    let html = tree.node_map.iter().find_map(|(id, n)| n.tag().filter(|t| t == "html").map(|_| *id));
    let Some(p) = html.and_then(|h| tree.paint_map.get(&h)) else { return 16.0 };
    match p.font_size_rel {
        Some((0 | 1, f)) => 16.0 * f,
        _ => p.font_size.unwrap_or(16.0),
    }
}

/// Re-applies the length declarations of every element that has a
/// font-relative one, with the element's own font as the basis. Returns
/// whether anything was re-applied.
fn reresolve_font_relative<H: Host, F: Fonts + ?Sized>(tree: &mut LayoutTree<H>, elems: &NodeMap<TextRun>, root_fs: f32, fonts: &F) -> bool {
    let todo: Vec<(taffy::NodeId, Vec<(String, String)>)> = tree
        .paint_map
        .iter()
        .filter_map(|(id, p)| {
            let list = p.font_rel.as_ref()?;
            list.iter()
                .any(|(prop, v)| css::is_font_relative(v) || (prop == "line-height" && v.trim_end().ends_with('%')))
                .then(|| (*id, list.clone()))
        })
        .collect();
    if todo.is_empty() {
        return false;
    }
    let viewport = ParseCx::new(tree.viewport.0, tree.viewport.1).viewport;
    for (id, decls) in todo {
        let Some(run) = elems.get(&id) else { continue };
        let fs = run.font_size;
        let (ex, ch) = if fonts.has_face(run.sel()) {
            let ex = fonts.x_height(run.sel(), fs);
            let ch = fonts.measure("0", &Adv::new(run.sel(), fs, 0.0));
            (if ex > 0.0 { ex } else { fs * 0.5 }, ch)
        } else {
            (fs * 0.5, fs * 0.5)
        };
        let cx = ParseCx { viewport, font: (fs, root_fs, ex, ch), container: (None, None) };
        let spec: SpecifiedStyle<H> = css::reapply_declarations(&decls, &cx);
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            spec.fold_into(&mut st);
            let _ = tree.taffy.set_style(id, st);
        }
        if let Some(p) = tree.paint_map.get_mut(&id) {
            css::merge_paint(p, &spec.paint);
        }
    }
    true
}

/// Folds the cascaded style of each box onto it (the layout fold and the paint merge), sizes real flex
/// containers' auto-width items by content, and keeps an absolutely positioned box with no inset in flow (its
/// static position). The host's `apply_stylesheets` and [`layout`] both end here.
pub fn apply_specs<H: Host>(tree: &mut LayoutTree<H>, specs: &[(taffy::NodeId, Rc<SpecifiedStyle<H>>)]) {
    // AETHERPAINTPERF2 (SR132): on a kept tree, the boxes this fold rewrites and the items `size_flex_items_by_content`
    // reads start from their specified style, as they always did (the block-width placeholders put back).
    if !tree.bw_pending.is_empty() {
        let mut at: BTreeSet<u64> = BTreeSet::new();
        for (id, _) in specs {
            at.insert(u64::from(*id));
            for k in tree.taffy.children(*id).unwrap_or_default() {
                at.insert(u64::from(k));
            }
        }
        undo_blockwidth(tree, Some(&at));
    }
    let mut abs_nodes: BTreeSet<u64> = BTreeSet::new();
    let mut inset_nodes: BTreeSet<u64> = BTreeSet::new();
    for (node_id, spec) in specs {
        let node_id = *node_id;
        if let Ok(node_style_ref) = tree.taffy.style(node_id) {
            let mut node_style = node_style_ref.clone();
            spec.fold_into(&mut node_style);
            let _ = tree.taffy.set_style(node_id, node_style);
        }
        match spec.position {
            Some(Position::Absolute) => {
                abs_nodes.insert(u64::from(node_id));
            }
            Some(Position::Relative) => {
                abs_nodes.remove(&u64::from(node_id));
            }
            _ => {}
        }
        if spec.inset_top.is_some() || spec.inset_left.is_some() || spec.inset_right.is_some() || spec.inset_bottom.is_some() {
            inset_nodes.insert(u64::from(node_id));
        }
        let entry = tree.paint_map.entry(node_id).or_default();
        css::merge_paint(entry, &spec.paint);
    }
    size_flex_items_by_content(tree, specs);
    // Static-position fallback: `position:absolute` with no inset specified
    // keeps its in-flow (static) position in real CSS. Taffy would pin such
    // a box to the parent origin, smearing it over siblings (the wikipedia
    // header overlap), so leave it in flow instead.
    for raw in abs_nodes.difference(&inset_nodes) {
        let node_id = taffy::NodeId::from(*raw);
        if let Ok(style_ref) = tree.taffy.style(node_id) {
            let mut s = style_ref.clone();
            s.position = Position::Relative;
            let _ = tree.taffy.set_style(node_id, s);
        }
    }
}

/// A flex item's `width: auto` is its content size, not its container's.
/// Aether gives every block the block-flow default `width: 100%`; inside a
/// real flex container that default would make each item claim the whole
/// line (one item per row under wrap). Items whose author CSS sets no
/// width fall back to `auto` here, after the cascade has settled.
fn size_flex_items_by_content<H: Host>(tree: &mut LayoutTree<H>, specs: &[(taffy::NodeId, Rc<SpecifiedStyle<H>>)]) {
    let by_box: NodeMap<&SpecifiedStyle<H>> = specs.iter().map(|(k, v)| (*k, &**v)).collect();
    for (node_id, spec) in specs {
        if spec.flex_container != Some(true) {
            continue;
        }
        let row = tree
            .taffy
            .style(*node_id)
            .is_ok_and(|s| matches!(s.flex_direction, FlexDirection::Row | FlexDirection::RowReverse));
        let kids = tree.taffy.children(*node_id).unwrap_or_default();
        for kid in kids {
            let author_width = by_box.get(&kid).is_some_and(|s| s.width.is_some());
            let Ok(st) = tree.taffy.style(kid) else { continue };
            if author_width || st.size.width != Dimension::percent(1.0) {
                continue;
            }
            let mut st = st.clone();
            // Row: main-axis size from content/basis. Column: stretch
            // (the default align-items) already fills the cross axis.
            if row {
                st.size.width = Dimension::auto();
                let _ = tree.taffy.set_style(kid, st);
            }
        }
    }
}

/// Recomputes layout with real text measurement: resolves each text run's
/// inherited font size, then lets taffy size text leaves by wrapped extent.
/// Called after building the tree and after every cascade application.
///
/// AETHERPAINTPERF2 (SR132): this is the IN-PLACE remeasure (a fresh tree, an animation frame that moved a layout
/// property, `apply_css`): it lays out every box, leaves every style as the passes found it, and counts every
/// laid-out box in [`LayoutTree::relaid`]. A relayout that keeps the previous tree goes through [`retarget`] and
/// [`remeasure_kept`] instead.
pub fn remeasure<H: Host, F: Fonts + ?Sized>(tree: &mut LayoutTree<H>, fonts: &F) {
    // A kept tree carries the block-width placeholders its last remeasure left applied: put them back first, so
    // this pass starts where the in-place passes always started (every specified style restored).
    undo_blockwidth(tree, None);
    tree.relaid = 0;
    let prep = prepass(tree, fonts);
    // Taffy caches leaf measurements; a cascade pass can change resolved
    // font sizes without touching the leaf's style, so stale cached sizes
    // survive set_style dirtying. Invalidate every measured leaf.
    for node_id in prep.text_info.keys().chain(prep.img_info.keys()).chain(prep.media_info.keys()) {
        let _ = tree.taffy.mark_dirty(*node_id);
    }

    tree.measured = measured_of(&prep);
    let _ = passes(tree, prep, fonts, true);
}

/// AETHERPAINTPERF2 (SR132): what the passes before the first taffy pass hand the taffy passes: the measure inputs
/// (text runs, replaced and media sizes), the resolved element fonts, the rem basis, and the saved specified
/// values the rewrites replaced for the layout run (vertical writing, block widths, collapsed margins).
struct Prep {
    text_info: NodeMap<TextRun>,
    img_info: NodeMap<(f32, f32)>,
    media_info: NodeMap<(f32, f32, f32)>,
    elem_info: NodeMap<TextRun>,
    root_fs: f32,
    writing_saved: Vec<(taffy::NodeId, Style)>,
    widths: blockwidth::Saved,
    collapsed: collapse::Saved,
    pct_nodes: Vec<(taffy::NodeId, [Option<String>; 3])>,
    has_tables: bool,
}

/// The style rewrites before the first taffy pass (formatting contexts, inherited text properties, font-relative
/// re-resolution, UA margins, vertical writing, replaced sizes, block widths, margin collapsing). Reads and writes
/// styles only — no box is laid out here, so a twin tree ([`retarget`]) can run them for a kept one.
fn prepass<H: Host, F: Fonts + ?Sized>(tree: &mut LayoutTree<H>, fonts: &F) -> Prep {
    fix_display_contexts(tree);
    crate::scroll::prepare(tree);
    tree.col_shift.clear();
    tree.multicol.clear();
    tree.col_sets.clear();
    tree.col_frags.clear();
    propagate_text_align(tree);
    collapse_inline_whitespace(tree);
    // Pass 1: resolve font size down the box tree for text leaves, and
    // intrinsic sizes for images.
    let mut text_info: NodeMap<TextRun> = NodeMap::new();
    let mut img_info: NodeMap<(f32, f32)> = NodeMap::new();
    // Replaced media boxes: (width, height) with the intrinsic aspect ratio, so a box sized on
    // one axis by CSS derives the other (CSS 2 §10.3.2 / §10.6.2 for replaced elements).
    let mut media_info: NodeMap<(f32, f32, f32)> = NodeMap::new();
    struct Out<'a> {
        text: &'a mut NodeMap<TextRun>,
        imgs: &'a mut NodeMap<(f32, f32)>,
        media: &'a mut NodeMap<(f32, f32, f32)>,
        sizes: &'a mut NodeMap<f32>,
        elems: &'a mut NodeMap<TextRun>,
    }
    fn resolve<H: Host, F: Fonts + ?Sized>(
        node_id: taffy::NodeId,
        inherited: TextRun, // `text` unused here: the inherited text properties
        tree: &LayoutTree<H>,
        out: &mut Out<'_>,
        root_fs: f32,
        fonts: &F,
    ) {
        let mut size = inherited.clone();
        if let Some(ta) = tree.paint_map.get(&node_id).and_then(|p| p.text_align) {
            size.text_align = ta;
        }
        if let Some(dom_node) = tree.node_map.get(&node_id) {
            if let Some(tag) = dom_node.tag() {
                let tag = tag.as_str();
                let paint = tree.paint_map.get(&node_id);
                let own_family = paint.and_then(|p| p.family).unwrap_or_else(|| default_family::<H>(tag, inherited.family));
                size.font_size = computed_font_size(paint, tag, inherited.font_size, inherited.family, own_family, inherited.fs_absolute, root_fs);
                match paint.and_then(|p| p.font_size_rel) {
                    Some((0, _)) => {}
                    Some(_) => size.fs_absolute = true,
                    None if paint.and_then(|p| p.font_size).is_some() => size.fs_absolute = true,
                    None => {}
                }
                if paint.is_some_and(|p| p.ua_vmargin.is_some()) {
                    out.sizes.insert(node_id, size.font_size);
                }
                if matches!(tag, "sup" | "sub") {
                    // vertical-align: super/sub grows the line box by the
                    // shift (the painter raises/lowers the glyphs by it);
                    // the parent's size is keyed negative-free by tag below.
                    out.sizes.insert(node_id, -inherited.font_size);
                }
                if let Some(lh) = paint.and_then(|p| p.line_height) {
                    size.line_height = lh;
                }
                if let Some(nw) = paint.and_then(|p| p.nowrap) {
                    size.nowrap = nw;
                }
                size.mode.white_space = paint
                    .and_then(|p| p.white_space)
                    .unwrap_or_else(|| default_white_space(tag, inherited.mode.white_space));
                if let Some(v) = paint.and_then(|p| p.word_break) {
                    size.mode.word_break = v;
                }
                if let Some(v) = paint.and_then(|p| p.overflow_wrap) {
                    size.mode.overflow_wrap = v;
                }
                if let Some(v) = paint.and_then(|p| p.letter_spacing) {
                    size.mode.letter_spacing = v;
                }
                if let Some(v) = paint.and_then(|p| p.word_spacing) {
                    size.mode.word_spacing = v;
                }
                size.family = paint.and_then(|p| p.family).unwrap_or_else(|| default_family::<H>(tag, inherited.family));
                size.weight = paint
                    .and_then(|p| p.weight)
                    .map(|w| w.resolve(inherited.weight))
                    .unwrap_or_else(|| default_weight(tag, inherited.weight));
                size.italic = paint.and_then(|p| p.italic).unwrap_or_else(|| default_italic(tag, inherited.italic));
                if let Some(st) = paint.and_then(|p| p.stretch) {
                    size.stretch = st;
                }
                size.mode.rtl = paint.and_then(|p| p.rtl).unwrap_or_else(|| default_rtl(dom_node, inherited.mode.rtl));
                if let Some(tt) = paint.and_then(|p| p.text_transform) {
                    size.text_transform = tt;
                }
                if let Some(wm) = paint.and_then(|p| p.writing_mode) {
                    size.writing_mode = wm;
                }
                let attr_px = |name: &str| dom_node.attr(name).and_then(|v| v.trim().parse::<f32>().ok());
                if let Some((iw, ih)) = H::media_intrinsic(dom_node) {
                    // width/height attributes win, the other axis keeps the intrinsic ratio.
                    let ratio = if ih > 0.0 { iw / ih } else { 2.0 };
                    let (w, h) = match (attr_px("width"), attr_px("height")) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, w / ratio),
                        (None, Some(h)) => (h * ratio, h),
                        (None, None) => (iw, ih),
                    };
                    out.media.insert(node_id, (w, h, ratio));
                }
                if let Some(wh) = control_intrinsic::<H, F>(dom_node, fonts) {
                    out.imgs.insert(node_id, wh);
                }
                if tag == "img" {
                    // width/height attributes win; else intrinsic dimensions.
                    let intrinsic = H::img_intrinsic(dom_node);
                    let w = attr_px("width").or(intrinsic.map(|(w, _)| w));
                    let h = attr_px("height").or(intrinsic.map(|(_, h)| h));
                    if let (Some(w), Some(h)) = (w, h) {
                        out.imgs.insert(node_id, (w, h));
                    }
                }
            } else if dom_node.is_text() {
                // Measured exactly as painted: the raw run (line breaking
                // owns whitespace), transformed, same face, same mode.
                let raw = dom_node.text();
                let text = H::transform_text(&raw, inherited.text_transform);
                let p = tree.paint_map.get(&node_id);
                let mut mode = inherited.mode;
                mode.lead = p.and_then(|p| p.ws_lead).unwrap_or(false);
                mode.trail = p.and_then(|p| p.ws_trail).unwrap_or(false);
                out.text.insert(node_id, TextRun { text, mode, ..inherited.clone() });
            }
        }
        if tree.node_map.get(&node_id).is_none_or(|n| !n.is_text()) {
            out.elems.insert(node_id, size.clone());
        }
        if let Ok(children) = tree.taffy.children(node_id) {
            for child in children {
                resolve(child, size.clone(), tree, out, root_fs, fonts);
            }
        }
    }
    // The initial font: Chromium's default standard font, Times New Roman
    // (Liberation Serif) at 16px — an unstyled page is serif.
    let root = TextRun { font_size: 16.0, family: crate::STANDARD, weight: 400, stretch: 100, ..Default::default() };
    let mut sizes: NodeMap<f32> = NodeMap::new();
    let mut elem_info: NodeMap<TextRun> = NodeMap::new();
    // The rem basis: the root element's computed font-size.
    let root_fs = root_font_size(tree);
    {
        let mut out = Out { text: &mut text_info, imgs: &mut img_info, media: &mut media_info, sizes: &mut sizes, elems: &mut elem_info };
        resolve(tree.root_node, root.clone(), tree, &mut out, root_fs, fonts);
    }
    // css-values-4 §6.1: lengths in em/ex/ch are of the element's own
    // computed font (font-size: of the parent's), rem of the root's. The
    // cascade parsed them at 16px; each element holding any re-applies its
    // length declarations, in cascade order, against its real font.
    if reresolve_font_relative(tree, &elem_info, root_fs, fonts) {
        text_info.clear();
        img_info.clear();
        media_info.clear();
        sizes.clear();
        elem_info.clear();
        let mut out = Out { text: &mut text_info, imgs: &mut img_info, media: &mut media_info, sizes: &mut sizes, elems: &mut elem_info };
        resolve(tree.root_node, root, tree, &mut out, root_fs, fonts);
    }
    for (id, run) in &elem_info {
        if tree.node_map.get(id).is_some_and(|n| n.is_element()) {
            if let Some(p) = tree.paint_map.get_mut(id) {
                p.used_font_size = Some(run.font_size);
            }
        }
    }
    rescale_ua_margins(tree, &sizes);
    // AETHERSTYLE3 (SR103): vertical writing modes' flow (restored after the passes).
    let writing_saved = crate::writing::apply(tree, &elem_info);
    // A replaced media box with an auto width is its own (attribute / intrinsic) width — not
    // stretched across a column container the way an auto-width block is. Height follows when
    // both axes are auto; with only the width specified the measure keeps the aspect ratio.
    for (&id, &(w, h, _)) in &media_info {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            let wa = st.size.width == Dimension::auto();
            let ha = st.size.height == Dimension::auto();
            if wa {
                st.size.width = Dimension::length(w);
                if ha {
                    st.size.height = Dimension::length(h);
                }
                let _ = tree.taffy.set_style(id, st);
            }
        }
    }

    // Block formatting context: adjoining vertical margins collapse for
    // this layout run (CSS 2.2 §8.3.1); specified margins return after.
    let widths = blockwidth::apply(tree);
    let collapsed = collapse::apply(tree);
    let pct_nodes: Vec<(taffy::NodeId, [Option<String>; 3])> = tree
        .paint_map
        .iter()
        .filter_map(|(id, p)| p.pct_math.clone().filter(|m| m.iter().any(Option::is_some)).map(|m| (*id, m)))
        .collect();
    let has_tables = !table::tables(tree).is_empty();
    Prep { text_info, img_info, media_info, elem_info, root_fs, writing_saved, widths, collapsed, pct_nodes, has_tables }
}

/// The taffy passes and what feeds back into them (tables, % widths, container units, multicol, the inline
/// formatting contexts), then the restores. `restore_widths` false leaves the block-width placeholders applied and
/// returns them (a kept tree: see [`remeasure_kept`]); true restores them and returns nothing.
fn passes<H: Host, F: Fonts + ?Sized>(tree: &mut LayoutTree<H>, prep: Prep, fonts: &F, restore_widths: bool) -> blockwidth::Saved {
    let Prep { text_info, img_info, media_info, elem_info, root_fs, writing_saved, widths, collapsed, pct_nodes, has_tables } = prep;
    let viewport = Size {
        width: AvailableSpace::Definite(tree.viewport.0),
        height: AvailableSpace::Definite(tree.viewport.1),
    };
    let vw_cap = tree.viewport.0;
    // The basis the % math of the second pass parses at (the cascade's: the viewport, the initial font).
    let pct_cx = ParseCx::new(tree.viewport.0, tree.viewport.1);
    let passes = if pct_nodes.is_empty() && !has_tables { 1 } else { 2 };
    let run_pass = |tree: &mut LayoutTree<H>| {
        // AETHERPAINTPERF2 (SR132): the boxes this taffy pass re-lays (its cache misses start at the dirty ones).
        tree.relaid += dirty_count(tree);
        let _ = tree.taffy.compute_layout_with_measure(tree.root_node, viewport, |inputs, node_id, _ctx, style| {
            taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                if let Some(&(w, h, ratio)) = media_info.get(&node_id) {
                    return match (known.width, known.height) {
                        (Some(kw), Some(kh)) => Size { width: kw, height: kh },
                        (Some(kw), None) => Size { width: kw, height: kw / ratio },
                        (None, Some(kh)) => Size { width: kh * ratio, height: kh },
                        (None, None) => Size { width: w, height: h },
                    };
                }
                if let Some(&(w, h)) = img_info.get(&node_id) {
                    return Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) };
                }
                let Some(run) = text_info.get(&node_id) else {
                    return Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) };
                };
                // Min-content (a flex item's automatic minimum size) is the
                // widest unbreakable word: wrap at every opportunity. Without
                // it a `flex: 1` column of text claimed its whole max-content
                // width as its minimum and overflowed its container.
                let wrap_width = known.width.unwrap_or(match avail.width {
                    AvailableSpace::Definite(w) => w,
                    AvailableSpace::MinContent => 0.0,
                    AvailableSpace::MaxContent => vw_cap,
                });
                let (w, h) = measure_text_run(run, wrap_width, fonts);
                Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
            })
        });
    };
    for pass in 0..passes {
        if pass == 1 && has_tables {
            // Column grid from the first pass's max-content cell widths and
            // each cell's min-content width.
            let min_content = |t: &LayoutTree<H>, id: taffy::NodeId| min_content_width(t, id, &text_info, fonts);
            table::apply(tree, &min_content);
        }
        if pass == 1 {
            // Containing-block widths are known now: resolve the % math.
            for (id, m) in &pct_nodes {
                let Some(parent) = tree.taffy.parent(*id) else { continue };
                let Ok(pl) = tree.taffy.layout(parent) else { continue };
                let cb = pl.size.width - pl.padding.left - pl.padding.right - pl.border.left - pl.border.right;
                let Ok(st) = tree.taffy.style(*id) else { continue };
                let mut st = st.clone();
                for (i, e) in m.iter().enumerate() {
                    let Some(v) = e.as_deref().and_then(|e| css::eval_length(e, Some(cb), &pct_cx)) else { continue };
                    match i {
                        0 => st.size.width = Dimension::length(v),
                        1 => st.min_size.width = LengthPercentageAuto::length(v),
                        _ => st.max_size.width = LengthPercentageAuto::length(v),
                    }
                }
                let _ = tree.taffy.set_style(*id, st);
            }
        }
        run_pass(tree);
    }
    // Row-spanning cells take the height of the rows they span.
    if has_tables && table::fix_rowspans(tree) {
        run_pass(tree);
    }
    // AETHERSTYLE3B (SR115): container-relative lengths resolve against the query containers this layout
    // sized (css-contain-3 §6.1), then the tree lays out again.
    if reresolve_container_units(tree, &elem_info, root_fs, fonts) {
        run_pass(tree);
    }
    // AETHERSTYLE3 (SR103): a block multicol container lays its in-flow children out in ONE column of the
    // used column width (css-multicol-1 §3.4); they keep their content height (no shrinking into it). The
    // IFC-root multicol container narrows its own lines in `inline::layout_root`.
    let mut mc_geom: Vec<(taffy::NodeId, u32, f32, f32)> = Vec::new();
    let mut mc_saved: Vec<(taffy::NodeId, LengthPercentageAuto, f32)> = Vec::new();
    {
        let ids: Vec<taffy::NodeId> = tree.paint_map.keys().copied().collect();
        for id in ids {
            if !crate::multicol::is_multicol(tree, id) || inline::is_ifc_root(tree, id) {
                continue;
            }
            let Ok(l) = tree.taffy.layout(id) else { continue };
            let cw = l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right;
            let fs = tree.paint_map.get(&id).and_then(|p| p.used_font_size).unwrap_or(16.0);
            let Some((n, w, gap)) = crate::multicol::used_columns(tree, id, cw, fs) else { continue };
            mc_geom.push((id, n, w, gap));
            // AETHERSTYLE3B (SR115): a `column-span: all` child keeps the container's full width;
            // AETHERSTYLE3C (SR122): so does a wrapper holding one, whose other children narrow instead.
            for k in crate::multicol::narrow_targets(tree, id) {
                let Ok(st) = tree.taffy.style(k) else { continue };
                mc_saved.push((k, st.max_size.width, st.flex_shrink));
                let mut st = st.clone();
                // AETHERRENDER3 (SR126): `max-width` is a content-box bound under content-box sizing, so a
                // decorated box gives up its own horizontal border and padding — its BORDER box is the column.
                let mut cap = w;
                if st.box_sizing == taffy::style::BoxSizing::ContentBox {
                    for side in [st.border.left.into_raw(), st.border.right.into_raw(), st.padding.left.into_raw(), st.padding.right.into_raw()] {
                        if side.tag() == taffy::style::CompactLength::LENGTH_TAG {
                            cap -= side.value();
                        }
                    }
                }
                st.max_size.width = LengthPercentageAuto::length(cap.max(0.0));
                st.flex_shrink = 0.0;
                let _ = tree.taffy.set_style(k, st);
            }
        }
        if !mc_geom.is_empty() {
            run_pass(tree);
        }
    }
    // Inline formatting contexts (CSS 2.2 §9.4.2): line boxes at the widths
    // the block pass settled; each root's height becomes the sum of its
    // line boxes, and the tree is laid out again until heights are stable
    // (an inline-block's height feeds the line that holds it).
    let mut saved_heights: Vec<(taffy::NodeId, Dimension)> = Vec::new();
    let mut saved_min: Vec<(taffy::NodeId, LengthPercentageAuto)> = Vec::new();
    let is_cell = |tree: &LayoutTree<H>, id: taffy::NodeId| {
        tree.node_map.get(&id).and_then(|n| n.tag().map(|t| matches!(t.as_str(), "td" | "th"))).unwrap_or(false)
    };
    let mut saved_widths: Vec<(taffy::NodeId, Dimension)> = Vec::new();
    for _ in 0..4 {
        let heights = inline::layout_all(tree, &text_info, &elem_info, fonts);
        let mut changed = false;
        for (id, h) in heights {
            let h = clear_floats(tree, id, h);
            let Ok(st) = tree.taffy.style(id) else { continue };
            // AETHERSTYLE3 (SR103): a vertical IFC root is as wide as its lines' block extent and as tall as
            // its inline size (or its longest line).
            if let Some(ext) = tree.inline.get(&id).filter(|il| il.vertical != 0).map(|il| il.inline_extent) {
                let Ok(l) = tree.taffy.layout(id).cloned() else { continue };
                let ex = l.padding.left + l.padding.right + l.border.left + l.border.right;
                let ey = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
                let bb = st.box_sizing == taffy::style::BoxSizing::BorderBox;
                let mut st2 = st.clone();
                let has_width = tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true);
                if !has_width {
                    let want = Dimension::length(if bb { h + ex } else { h });
                    if st2.size.width != want {
                        if !saved_widths.iter().any(|(s, _)| *s == id) {
                            saved_widths.push((id, st.size.width));
                        }
                        st2.size.width = want;
                        if (l.size.width - ex - h).abs() >= 0.01 {
                            changed = true;
                        }
                    }
                }
                let orig_h = saved_heights.iter().find(|(s, _)| *s == id).map(|(_, d)| *d).unwrap_or(st.size.height);
                if orig_h == Dimension::auto() {
                    let want = Dimension::length(if bb { ext + ey } else { ext });
                    if st2.size.height != want {
                        if !saved_heights.iter().any(|(s, _)| *s == id) {
                            saved_heights.push((id, st.size.height));
                        }
                        st2.size.height = want;
                        if (l.size.height - ey - ext).abs() >= 0.01 {
                            changed = true;
                        }
                    }
                }
                let _ = tree.taffy.set_style(id, st2);
                continue;
            }
            if is_cell(tree, id) {
                // A table cell stretches to its row (§17.5.3): its line boxes
                // set a MINIMUM height; vertical-align places them inside.
                let Ok(l) = tree.taffy.layout(id).cloned() else { continue };
                let edges = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
                let want = if st.box_sizing == taffy::style::BoxSizing::BorderBox { h + edges } else { h };
                let orig = saved_min.iter().find(|(s, _)| *s == id).map(|(_, d)| *d).unwrap_or(st.min_size.height);
                let author = match orig.into_raw().tag() {
                    taffy::style::CompactLength::LENGTH_TAG => orig.into_raw().value(),
                    _ => 0.0,
                };
                let target = LengthPercentageAuto::length(want.max(author));
                if st.min_size.height != target {
                    if !saved_min.iter().any(|(s, _)| *s == id) {
                        saved_min.push((id, st.min_size.height));
                    }
                    let mut st = st.clone();
                    st.min_size.height = target;
                    let _ = tree.taffy.set_style(id, st);
                    if (l.size.height - edges - h).abs() >= 0.01 {
                        changed = true;
                    }
                }
                continue;
            }
            let saved = saved_heights.iter().find(|(s, _)| *s == id).map(|(_, d)| *d);
            let orig = saved.unwrap_or(st.size.height);
            if orig != Dimension::auto() {
                continue;
            }
            let Ok(l) = tree.taffy.layout(id) else { continue };
            let edges = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
            let want = if st.box_sizing == taffy::style::BoxSizing::BorderBox { h + edges } else { h };
            let content_now = l.size.height - edges;
            if st.size.height == Dimension::length(want) && (content_now - h).abs() < 0.01 {
                continue;
            }
            if saved.is_none() {
                saved_heights.push((id, st.size.height));
            }
            let mut st = st.clone();
            st.size.height = Dimension::length(want);
            let _ = tree.taffy.set_style(id, st);
            if (content_now - h).abs() >= 0.01 {
                changed = true;
            }
        }
        if !changed {
            break;
        }
        run_pass(tree);
    }
    // AETHERSTYLE3 (SR103): the block multicol containers fragment their settled content into the columns
    // and take the tallest column's height (one more pass places what follows them).
    let mut mc_heights: Vec<(taffy::NodeId, Dimension)> = Vec::new();
    for &(id, n, w, gap) in &mc_geom {
        let cols = crate::multicol::balance_blocks(tree, id, n, w, gap);
        tree.multicol.insert(id, cols);
        let Ok(st) = tree.taffy.style(id) else { continue };
        if st.size.height != Dimension::auto() {
            continue;
        }
        let Ok(l) = tree.taffy.layout(id) else { continue };
        let edges = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
        let want = if st.box_sizing == taffy::style::BoxSizing::BorderBox { cols.height + edges } else { cols.height };
        mc_heights.push((id, st.size.height));
        let mut st = st.clone();
        st.size.height = Dimension::length(want);
        let _ = tree.taffy.set_style(id, st);
    }
    if !mc_geom.is_empty() {
        run_pass(tree);
    }
    for (id, d) in mc_heights {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.size.height = d;
            let _ = tree.taffy.set_style(id, st);
        }
    }
    for (id, mw, shrink) in mc_saved {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.max_size.width = mw;
            st.flex_shrink = shrink;
            let _ = tree.taffy.set_style(id, st);
        }
    }
    for (id, d) in saved_min {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.min_size.height = d;
            let _ = tree.taffy.set_style(id, st);
        }
    }
    for (id, d) in saved_heights {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.size.height = d;
            let _ = tree.taffy.set_style(id, st);
        }
    }
    for (id, d) in saved_widths {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.size.width = d;
            let _ = tree.taffy.set_style(id, st);
        }
    }
    crate::writing::restore(tree, writing_saved);
    collapse::restore(tree, collapsed);
    if restore_widths {
        blockwidth::restore(tree, widths);
        return Vec::new();
    }
    widths
}

// ---- AETHERPAINTPERF2 (SR132): a relayout that keeps the previous tree ------------------------------------------
//
// Every relayout used to lay out a fresh box tree: taffy's cache starts empty, so every box was laid out again
// (4,004 boxes for a 2,000-row list where one row's text changed). Keeping the taffy tree across relayouts keeps
// that cache, but the passes cannot simply run again over the kept tree: they rewrite the styles they are handed
// (`fix_display_contexts`, `propagate_text_align`, `rescale_ua_margins`, the replaced sizes, the % widths, the
// block widths, margin collapsing, the multicol narrowing, the inline heights) and several rewrites read their own
// earlier output as if it were specified (collapsing margins twice collapses them again).
//
// So the specified style stays the fresh build's, and the passes keep running from it — on a TWIN of the kept tree.
// [`retarget`] builds the twin: a copy of the kept tree's taffy arena (same slot keys) holding the fresh build, each
// fresh box placed on the kept box of the same DOM node (or the anonymous box at the same place under the same
// parent), every other box on a new slot. The host's cascade, generated content and [`apply_specs`] then run on
// the twin, and [`remeasure_kept`] runs the style rewrites ([`prepass`]) there too: the twin is the fresh tree's
// values, box for box, under the kept tree's keys. It then makes the kept tree's taffy hold what the twin holds —
// the same number of new slots (the arena hands out the same keys: both arenas did the same inserts from the same
// state, which is checked), each box's style and children set only where they differ, a leaf whose measure inputs
// moved (its text run, a replaced or media size) dirtied, every dirtied box's ancestors dirtied — and lays the kept
// tree out ([`passes`]). Taffy re-lays the dirty boxes; a clean box answers from its cache, which holds the layout
// of the same style, the same subtree and the same measure inputs. The values the passes see are the fresh tree's,
// so the result is the fresh tree's. The taffy cache was already relied on across the passes of one remeasure.
//
// Two rewrites are not put back at the end of a kept tree's remeasure: the block-width placeholders. A row's style
// at the first taffy pass is its placeholder (`blockwidth::apply` runs before it); restoring it after the last
// pass and applying it again at the next relayout would dirty every block of the page. No reader after layout reads
// those fields, and [`apply_specs`] / the in-place [`remeasure`] put them back before they fold or lay out again.
// Every other rewrite is restored as before, so a box it touches (an auto-height inline formatting root, a box
// whose margins collapse) is re-laid on every relayout, with its ancestors.
//
// What relayouts whole: a tree holding a table (its passes keep cross-box lists keyed by box), a multicol container
// (its narrowing restores in box-key order), or an author outline (painted in box-key order), and any twin whose
// key replay does not match.

/// One measured leaf's measure inputs (what `run_pass`'s measure function reads for it).
#[derive(Clone, Default, PartialEq)]
struct Measured {
    text: Option<TextRun>,
    img: Option<(f32, f32)>,
    media: Option<(f32, f32, f32)>,
}

fn measured_of(prep: &Prep) -> NodeMap<Measured> {
    let mut m: NodeMap<Measured> = NodeMap::new();
    for (id, r) in &prep.text_info {
        m.entry(*id).or_default().text = Some(r.clone());
    }
    for (id, v) in &prep.img_info {
        m.entry(*id).or_default().img = Some(*v);
    }
    for (id, v) in &prep.media_info {
        m.entry(*id).or_default().media = Some(*v);
    }
    m
}

/// Puts back the block-width placeholders a kept-tree remeasure left applied: all of them, or those of the boxes
/// in `only`.
fn undo_blockwidth<H: Host>(tree: &mut LayoutTree<H>, only: Option<&BTreeSet<u64>>) {
    if tree.bw_pending.is_empty() {
        return;
    }
    let pending = core::mem::take(&mut tree.bw_pending);
    let (undo, keep): (blockwidth::Saved, blockwidth::Saved) =
        pending.into_iter().partition(|(id, _)| only.is_none_or(|s| s.contains(&u64::from(*id))));
    tree.bw_pending = keep;
    blockwidth::restore(tree, undo);
}

/// The dirty boxes reachable from the root through dirty boxes: the boxes the next taffy pass re-lays (a clean box
/// is a cache hit, and taffy does not descend into it).
fn dirty_count<H: Host>(tree: &LayoutTree<H>) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![tree.root_node];
    while let Some(id) = stack.pop() {
        if !tree.taffy.dirty(id).unwrap_or(true) {
            continue;
        }
        n += 1;
        stack.extend(tree.taffy.children(id).unwrap_or_default());
    }
    n
}

/// Marks `id` and every box above it dirty. taffy's `mark_dirty` stops climbing at a box whose cache is already
/// empty (a box under `display: none` is never cached), which would leave a cached ancestor over a changed box.
fn dirty_up<H: Host>(tree: &mut LayoutTree<H>, id: taffy::NodeId) {
    let mut cur = Some(id);
    while let Some(c) = cur {
        let _ = tree.taffy.mark_dirty(c);
        cur = tree.taffy.parent(c);
    }
}

/// Every box of a tree — those reachable from the root in preorder, then those only its maps name (boxes the
/// passes detached) — and their keys.
fn boxes_of<H: Host>(tree: &LayoutTree<H>) -> (Vec<taffy::NodeId>, BTreeSet<u64>) {
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node];
    while let Some(id) = stack.pop() {
        if !seen.insert(u64::from(id)) {
            continue;
        }
        out.push(id);
        stack.extend(tree.taffy.children(id).unwrap_or_default().into_iter().rev());
    }
    for id in tree.node_map.keys().chain(tree.paint_map.keys()) {
        if seen.insert(u64::from(*id)) {
            out.push(*id);
        }
    }
    (out, seen)
}

/// AETHERPAINTPERF2 (SR132): the twin of `kept` for the freshly built tree `fresh` (see the section comment): a copy
/// of `kept`'s taffy arena holding `fresh`'s boxes — each on the kept box of the same DOM node by `key` (a node's
/// identity in its document), an anonymous box on the anonymous kept box at the same place under the same parent,
/// the rest on new slots — with `fresh`'s styles, children, DOM nodes and paint styles. `token` marks it as
/// `kept`'s twin for [`remeasure_kept`]. Which kept box a fresh box lands on never changes a value (every box takes
/// `fresh`'s), only which taffy caches survive.
pub fn retarget<H: Host>(fresh: LayoutTree<H>, kept: &LayoutTree<H>, token: u64, key: &dyn Fn(&H::Node) -> u64) -> LayoutTree<H> {
    let mut by_key: BTreeMap<u64, Vec<taffy::NodeId>> = BTreeMap::new();
    for (id, n) in &kept.node_map {
        by_key.entry(key(n)).or_default().push(*id);
    }
    let mut order: Vec<taffy::NodeId> = Vec::new();
    let mut stack = vec![fresh.root_node];
    while let Some(b) = stack.pop() {
        order.push(b);
        stack.extend(fresh.taffy.children(b).unwrap_or_default().into_iter().rev());
    }
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let mut pair: NodeMap<taffy::NodeId> = NodeMap::new();
    for &b in &order {
        let cand = match fresh.node_map.get(&b) {
            Some(n) => by_key.get(&key(n)).and_then(|v| v.iter().copied().find(|k| !taken.contains(&u64::from(*k)))),
            None => fresh.taffy.parent(b).and_then(|p| {
                let kp = *pair.get(&p)?;
                let i = fresh.taffy.children(p).ok()?.iter().position(|&k| k == b)?;
                let k = *kept.taffy.children(kp).ok()?.get(i)?;
                (!kept.node_map.contains_key(&k) && kept.paint_map.contains_key(&k) && !taken.contains(&u64::from(k))).then_some(k)
            }),
        };
        if let Some(k) = cand {
            taken.insert(u64::from(k));
            pair.insert(b, k);
        }
    }
    let mut taffy = kept.taffy.clone();
    for &b in &order {
        if pair.contains_key(&b) {
            continue;
        }
        let st = fresh.taffy.style(b).map(|s| s.clone()).unwrap_or_default();
        let Ok(t) = taffy.new_leaf(st) else { return fresh };
        pair.insert(b, t);
    }
    for &b in &order {
        let Some(&t) = pair.get(&b) else { return fresh };
        if let Ok(st) = fresh.taffy.style(b) {
            let _ = taffy.set_style(t, st.clone());
        }
        let kids: Vec<taffy::NodeId> = fresh.taffy.children(b).unwrap_or_default().iter().filter_map(|k| pair.get(k).copied()).collect();
        let _ = taffy.set_children(t, &kids);
    }
    let Some(&root) = pair.get(&fresh.root_node) else { return fresh };
    if let Some(p) = taffy.parent(root) {
        let _ = taffy.remove_child(p, root);
    }
    let mut node_map = NodeMap::new();
    for (b, n) in &fresh.node_map {
        if let Some(t) = pair.get(b) {
            node_map.insert(*t, n.clone());
        }
    }
    let mut paint_map = NodeMap::new();
    for (b, p) in &fresh.paint_map {
        if let Some(t) = pair.get(b) {
            paint_map.insert(*t, p.clone());
        }
    }
    LayoutTree {
        taffy,
        root_node: root,
        node_map,
        paint_map,
        viewport: fresh.viewport,
        dirty: false,
        inline: NodeMap::new(),
        rowspans: Vec::new(),
        scroll: NodeMap::new(),
        col_shift: NodeMap::new(),
        multicol: NodeMap::new(),
        col_sets: NodeMap::new(),
        col_frags: NodeMap::new(),
        relaid: 0,
        measure_epoch: 0,
        kept_token: token,
        measured: NodeMap::new(),
        bw_pending: Vec::new(),
    }
}

/// Whether a twin's layout may run on the kept tree: no table, no multicol container, no author outline (see the
/// section comment).
fn keepable<H: Host>(tree: &LayoutTree<H>) -> bool {
    table::tables(tree).is_empty()
        && !tree.paint_map.iter().any(|(id, p)| crate::paint::outline_of(p).is_some() || crate::multicol::is_multicol(tree, *id))
}

/// Makes `kept`'s taffy tree hold what the twin holds (see the section comment); false, with the twin untouched,
/// when the twin's new slots are not the ones `kept`'s arena hands out.
fn sync<H: Host>(twin: &mut LayoutTree<H>, kept: &mut LayoutTree<H>, prep: &Prep) -> bool {
    let (used, used_set) = boxes_of(twin);
    let (_, known) = boxes_of(kept);
    // The twin is a copy of the kept arena plus its own inserts (retarget's new boxes, generated content, the
    // anonymous boxes of `fix_display_contexts`) and no removal: the kept arena makes as many, and they must be the
    // keys the twin's new boxes hold.
    let Some(made) = twin.taffy.total_node_count().checked_sub(kept.taffy.total_node_count()) else { return false };
    let created: BTreeSet<u64> = used_set.iter().copied().filter(|id| !known.contains(id)).collect();
    if created.len() != made {
        return false;
    }
    let mut fresh: BTreeSet<u64> = BTreeSet::new();
    for _ in 0..made {
        match kept.taffy.new_leaf(Style::default()) {
            Ok(id) => {
                fresh.insert(u64::from(id));
            }
            Err(_) => return false,
        }
    }
    if fresh != created {
        return false;
    }
    let mut touched: Vec<taffy::NodeId> = Vec::new();
    for &id in &used {
        let Ok(ts) = twin.taffy.style(id) else { continue };
        if kept.taffy.style(id).map_or(true, |ks| ks != ts) {
            let _ = kept.taffy.set_style(id, ts.clone());
            touched.push(id);
        }
        let tc = twin.taffy.children(id).unwrap_or_default();
        if kept.taffy.children(id).unwrap_or_default() != tc {
            for &c in &tc {
                if let Some(op) = kept.taffy.parent(c).filter(|op| *op != id) {
                    touched.push(op);
                }
            }
            let _ = kept.taffy.set_children(id, &tc);
            touched.push(id);
        }
    }
    let root = twin.root_node;
    if let Some(p) = kept.taffy.parent(root) {
        let _ = kept.taffy.remove_child(p, root);
    }
    let now = measured_of(prep);
    for (id, m) in &now {
        if kept.measured.get(id) != Some(m) {
            touched.push(*id);
        }
    }
    for id in kept.measured.keys() {
        if !now.contains_key(id) && used_set.contains(&u64::from(*id)) {
            touched.push(*id);
        }
    }
    for id in touched {
        if used_set.contains(&u64::from(id)) {
            dirty_up(kept, id);
        }
    }
    for raw in known.difference(&used_set) {
        let _ = kept.taffy.remove(taffy::NodeId::from(*raw));
    }
    kept.root_node = root;
    kept.node_map = core::mem::take(&mut twin.node_map);
    kept.paint_map = core::mem::take(&mut twin.paint_map);
    kept.viewport = twin.viewport;
    kept.dirty = twin.dirty;
    kept.inline = core::mem::take(&mut twin.inline);
    kept.rowspans = core::mem::take(&mut twin.rowspans);
    kept.scroll = core::mem::take(&mut twin.scroll);
    kept.col_shift = core::mem::take(&mut twin.col_shift);
    kept.multicol = core::mem::take(&mut twin.multicol);
    kept.col_sets = core::mem::take(&mut twin.col_sets);
    kept.col_frags = core::mem::take(&mut twin.col_frags);
    kept.measured = now;
    kept.bw_pending = Vec::new();
    kept.kept_token = 0;
    true
}

/// AETHERPAINTPERF2 (SR132): [`remeasure`] for a twin [`retarget`] built: the style rewrites run on the twin, the
/// kept tree takes the twin's values and is laid out (its taffy passes re-lay the boxes whose style, children or
/// measure inputs moved). true: `kept` is the laid-out tree (the caller swaps it in); false: the twin was laid out
/// in place, every box (no token, a table, a multicol container, an author outline, a key replay that did not
/// match), and `kept` is to be dropped.
pub fn remeasure_kept<H: Host, F: Fonts + ?Sized>(twin: &mut LayoutTree<H>, kept: &mut LayoutTree<H>, fonts: &F) -> bool {
    twin.relaid = 0;
    let prep = prepass(twin, fonts);
    let token = core::mem::take(&mut twin.kept_token);
    if token == 0 || !keepable(twin) || !sync(twin, kept, &prep) {
        // Whole: `retarget` set every box's style, which emptied its cache.
        for node_id in prep.text_info.keys().chain(prep.img_info.keys()).chain(prep.media_info.keys()) {
            let _ = twin.taffy.mark_dirty(*node_id);
        }
        twin.measured = measured_of(&prep);
        let _ = passes(twin, prep, fonts, true);
        return false;
    }
    kept.relaid = 0;
    kept.bw_pending = passes(kept, prep, fonts, false);
    true
}

/// AETHERPAINTPERF2 (SR132): [`layout`] keeping `kept`, the tree laid out from the same document before: a fresh box
/// tree [`retarget`]ed onto `kept` by `key`, the cascaded styles folded, then [`remeasure_kept`]. Returns the laid-out
/// tree (`kept` itself when it could be kept).
pub fn relayout_kept<H: Host, D: Dom<Node = H::Node>, F: Fonts>(
    dom: &D,
    styles: &impl ComputedStyles<H>,
    fonts: &mut F,
    viewport: (f32, f32),
    mut kept: LayoutTree<H>,
    key: &dyn Fn(&H::Node) -> u64,
) -> LayoutTree<H> {
    let root = dom.root();
    let fresh = build_tree::<H>(&root, viewport.0, viewport.1);
    let mut twin = if kept.viewport == viewport { retarget(fresh, &kept, 1, key) } else { fresh };
    let mut specs: Vec<(taffy::NodeId, Rc<SpecifiedStyle<H>>)> = Vec::new();
    for (tid, n) in &twin.node_map {
        if n.is_element() {
            if let Some(s) = styles.style_of(n) {
                specs.push((*tid, s));
            }
        }
    }
    apply_specs(&mut twin, &specs);
    if twin.kept_token == 0 {
        remeasure(&mut twin, &*fonts);
        return twin;
    }
    if remeasure_kept(&mut twin, &mut kept, &*fonts) {
        kept
    } else {
        twin
    }
}

/// AETHERSTYLE3B (SR115): every element holding a container-relative length re-applies its length
/// declarations (cascade order, `PaintStyle::font_rel`) with the content-box size of its query containers as
/// laid out now: the nearest ancestor whose `container-type` gives a width (`size`, `inline-size`) for
/// cqw / cqi, the nearest that gives a height (`size`) for cqh / cqb; its own font for em / ex / ch. `true`
/// when any element did.
fn reresolve_container_units<H: Host, F: Fonts + ?Sized>(tree: &mut LayoutTree<H>, elems: &NodeMap<TextRun>, root_fs: f32, fonts: &F) -> bool {
    let todo: Vec<(taffy::NodeId, Vec<(String, String)>)> = tree
        .paint_map
        .iter()
        .filter_map(|(id, p)| {
            let list = p.font_rel.as_ref()?;
            list.iter().any(|(_, v)| css::is_container_relative(v)).then(|| (*id, list.clone()))
        })
        .collect();
    if todo.is_empty() {
        return false;
    }
    let viewport = ParseCx::new(tree.viewport.0, tree.viewport.1).viewport;
    for (id, decls) in todo {
        let (mut qw, mut qh) = (None, None);
        let mut cur = tree.taffy.parent(id);
        while let Some(p) = cur {
            let ty = tree.paint_map.get(&p).and_then(|s| s.container_type).unwrap_or(0);
            if ty != 0 {
                if let Ok(l) = tree.taffy.layout(p) {
                    if qw.is_none() {
                        qw = Some((l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right).max(0.0));
                    }
                    if qh.is_none() && ty == 1 {
                        qh = Some((l.size.height - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom).max(0.0));
                    }
                }
            }
            if qw.is_some() && qh.is_some() {
                break;
            }
            cur = tree.taffy.parent(p);
        }
        let font = match elems.get(&id) {
            Some(run) => {
                let fs = run.font_size;
                let (ex, ch) = if fonts.has_face(run.sel()) {
                    let ex = fonts.x_height(run.sel(), fs);
                    (if ex > 0.0 { ex } else { fs * 0.5 }, fonts.measure("0", &Adv::new(run.sel(), fs, 0.0)))
                } else {
                    (fs * 0.5, fs * 0.5)
                };
                (fs, root_fs, ex, ch)
            }
            None => ParseCx::default().font,
        };
        let cx = ParseCx { viewport, font, container: (qw, qh) };
        let spec: SpecifiedStyle<H> = css::reapply_declarations(&decls, &cx);
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            spec.fold_into(&mut st);
            let _ = tree.taffy.set_style(id, st);
        }
        if let Some(p) = tree.paint_map.get_mut(&id) {
            css::merge_paint(p, &spec.paint);
        }
    }
    true
}

/// True for a box the host's generated-content pass made (a detached element standing in the box tree).
pub fn is_generated_box<H: Host>(tree: &LayoutTree<H>, id: taffy::NodeId) -> bool {
    tree.node_map.get(&id).is_some_and(|n| n.is_element() && n.parent().is_none())
}

/// AETHERGEN (SR71): an inline formatting context whose content ends with a clearing box (the
/// `::after { content: ""; clear: both }` clearfix, which the host's `generated` keeps in the line content as
/// an empty inline box) grows to the bottom of the floats that box clears — the height CSS 2.2 §9.5.2
/// clearance gives the clearfix's container.
fn clear_floats<H: Host>(tree: &LayoutTree<H>, root: taffy::NodeId, h: f32) -> f32 {
    let kids = tree.taffy.children(root).unwrap_or_default();
    let Some(clear) = kids
        .last()
        .filter(|k| is_generated_box(tree, **k))
        .and_then(|k| tree.paint_map.get(k).and_then(|p| p.clear))
        .filter(|&c| c != 0)
    else {
        return h;
    };
    let Some(il) = tree.inline.get(&root) else { return h };
    let mut bottom = h;
    for f in &il.frags {
        let inline::Frag::Atomic { node, y, .. } = f else { continue };
        let side = tree.paint_map.get(node).and_then(|p| p.float).unwrap_or(0);
        if side == 0 || (clear != 3 && clear != side) {
            continue;
        }
        if let Ok(l) = tree.taffy.layout(*node) {
            bottom = bottom.max(y + l.size.height + l.margin.bottom);
        }
    }
    bottom
}

/// One text run's resolved text properties (inherited down the box tree the
/// same way the host's render::draw_node inherits them).
#[derive(Clone, Default, PartialEq)]
pub(crate) struct TextRun {
    pub(crate) text: String,
    pub(crate) font_size: f32,
    /// Multiplier of font size; 0 = the face's natural line height.
    pub(crate) line_height: f32,
    pub(crate) nowrap: bool,
    pub(crate) mode: TextMode,
    /// font-family list id.
    pub(crate) family: u16,
    /// Computed font-weight (1..=1000).
    pub(crate) weight: u16,
    pub(crate) italic: bool,
    /// font-stretch, percent.
    pub(crate) stretch: u16,
    pub(crate) text_transform: u8,
    /// Inherited text-align: 0 start, 1 center, 2 end.
    pub(crate) text_align: u8,
    /// The font-size still derives from the initial `medium` keyword (no
    /// absolute size up the chain): only then does switching to monospace
    /// scale it by 13/16 (Blink's fixed-pitch default).
    pub(crate) fs_absolute: bool,
    /// AETHERSTYLE3 (SR103): the inherited `writing-mode` (`writing` codes).
    pub(crate) writing_mode: u8,
}

impl TextRun {
    /// The font selection of the run.
    pub(crate) fn sel(&self) -> FontSel {
        FontSel { family: self.family, weight: self.weight, italic: self.italic, stretch: self.stretch }
    }
    /// The run's measurer: its face stack, size, spacing and direction.
    pub(crate) fn advancer(&self) -> Adv {
        Adv::new(self.sel(), self.font_size, self.mode.letter_spacing)
            .with_word_spacing(self.mode.word_spacing)
            .with_dir(self.mode.rtl)
    }
}

/// Measures a text run: (widest line, lines x line height), through the
/// same line breaker the painter uses.
fn measure_text_run<F: Fonts + ?Sized>(run: &TextRun, max_width: f32, fonts: &F) -> (f32, f32) {
    if !fonts.has_face(run.sel()) {
        return (max_width, run.font_size * 1.25);
    }
    let mut mode = run.mode;
    if run.nowrap && mode.white_space == 0 {
        mode.white_space = 1;
    }
    let adv = run.advancer();
    let lines = fonts.break_lines(&run.text, &mode, &adv, max_width, 0.0, false);
    let widest = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    let lh = fonts.line_height(run.sel(), run.font_size, run.line_height);
    // A whitespace-only run that collapsed away is zero-sized.
    if lines.len() == 1 && lines[0].text.is_empty() {
        return (0.0, 0.0);
    }
    (widest, lines.len() as f32 * lh)
}

/// Applies a `style="..."` attribute through the shared declaration parser.
fn apply_inline_style<H: Host>(inline: &str, style: &mut Style, paint: &mut PaintStyle<H>, cx: &ParseCx) {
    let spec: SpecifiedStyle<H> = css::parse_declaration_block(inline, cx);
    spec.fold_into(style);
    css::merge_paint(paint, &spec.paint);
}
