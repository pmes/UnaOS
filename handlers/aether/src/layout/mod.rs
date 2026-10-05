use crate::dom::NodeRef;
use taffy::prelude::*;
use std::collections::HashMap;

mod blockwidth;
mod collapse;
pub mod inline;
mod table;

/// Specified paint properties for one box. `None` = not specified here;
/// color and font-size inherit down the tree at render time.
#[derive(Debug, Clone, Default)]
pub struct PaintStyle {
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
    pub bg_gradient: Option<Option<crate::render::effects::Gradient>>,
    /// Outer box shadows (empty = none).
    pub shadows: Option<Vec<crate::render::effects::Shadow>>,
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
    pub text_shadows: Option<Vec<crate::render::effects::Shadow>>,
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

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    pub node_map: HashMap<taffy::NodeId, NodeRef>, // Maps layout boxes back to DOM nodes
    pub paint_map: HashMap<taffy::NodeId, PaintStyle>,
    /// Viewport this tree lays out against (media queries + wrap width).
    pub viewport: (f32, f32),
    pub dirty: bool,
    /// Inline formatting contexts (CSS 2.2 §9.4.2) by their root box: the
    /// line-box fragments render paints in place of the root's children.
    pub inline: HashMap<taffy::NodeId, inline::InlineLayout>,
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
        self.inline = new_tree.inline;
        self.dirty = false;
    }
}

trait ExactlyOneText {
    fn exactly_one_text(self) -> bool;
}
impl<I: Iterator<Item = NodeRef>> ExactlyOneText for I {
    /// True when the iterator yields exactly one node and it is text.
    fn exactly_one_text(mut self) -> bool {
        matches!((self.next(), self.next()), (Some(n), None) if n.as_text().is_some())
    }
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
            | "noscript" | "iframe" | "object" | "embed"
            | "option" | "optgroup"
    )
}

/// The (submit value, display label) of a `<select>`'s selected option:
/// the first option carrying `selected`, else the first option at all.
/// Value is the option's `value` attribute, falling back to its text —
/// the HTML rule. Label is always the option's text.
fn select_selected_option(select: &NodeRef) -> Option<(String, String)> {
    let mut first: Option<(String, String)> = None;
    for desc in select.descendants() {
        let Some(el) = desc.as_element() else { continue };
        if el.name.local.as_ref() != "option" {
            continue;
        }
        let label = desc.text_contents().split_whitespace().collect::<Vec<_>>().join(" ");
        let attrs = el.attributes.borrow();
        let selected = attrs.get("selected").is_some();
        let value = attrs
            .get("value")
            .map(|v| v.to_string())
            .unwrap_or_else(|| label.clone());
        drop(attrs);
        let pair = (value, label);
        if selected {
            return Some(pair);
        }
        if first.is_none() {
            first = Some(pair);
        }
    }
    first
}

/// Publishes a `<select>`'s effective value onto the element, so the paint
/// path (which draws a control's `value` attribute) and form submission
/// (which reads the same attribute) both see the selected option rather
/// than nothing. `data-aether-label` carries the human-visible text, which
/// differs from the submit value whenever the option has a `value` attr
/// (`<option value=en>English</option>`).
fn publish_select_value(node: &NodeRef) {
    let Some(el) = node.as_element() else { return };
    if el.name.local.as_ref() != "select" {
        return;
    }
    let Some((value, label)) = select_selected_option(node) else { return };
    let mut attrs = el.attributes.borrow_mut();
    if attrs.get("value").is_none_or(|v| v.is_empty()) {
        attrs.insert("value", value);
    }
    attrs.insert("data-aether-label", label);
}

/// Inline-level elements: they size to content and flow in wrapping rows.
pub(crate) fn is_inline(name: &str) -> bool {
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

/// A whitespace-only text node whose nearest box-generating siblings on
/// both sides are inline content (text or inline elements).
fn is_interelement_space(node: &NodeRef) -> bool {
    let inline_neighbour = |mut it: Box<dyn Iterator<Item = NodeRef>>| {
        it.find(|n| generates_box(n)).is_some_and(|n| is_inline_node(&n))
    };
    node.parent().is_some_and(|p| {
        p.as_element().is_some_and(|e| !is_table_part(e.name.local.as_ref()) || matches!(e.name.local.as_ref(), "td" | "th" | "caption"))
    }) && inline_neighbour(Box::new(node.preceding_siblings()))
        && inline_neighbour(Box::new(node.following_siblings()))
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
    // Replaced media elements are inline boxes whatever their (never rendered) <source>
    // children are — testing those made a <video><source></video> a full-width block.
    if matches!(tag, "video" | "audio") {
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
            // <audio> without `controls` is display:none in the UA sheet.
            if el.name.local.as_ref() == "audio" && attrs.get("controls").is_none() {
                return None;
            }
        } else if dom_node.as_comment().is_some()
            || dom_node.as_doctype().is_some()
        {
            // Comments and the doctype are not
            // rendered (they generate no box). Boxed, an empty block between
            // inline siblings split their line into anonymous blocks and
            // moved absolutely positioned media off their insets.
            return None;
        } else if dom_node.as_text().is_some() {
            // Whitespace-only text produces no box — except between two
            // inline siblings, where it is the word space of the line
            // ("<b>a</b> <i>b</i>"; css-text-3 §4.1.1 collapses it to one
            // space, or to nothing, in the whitespace pass).
            if dom_node.text_contents().trim().is_empty() && !is_interelement_space(dom_node) {
                return None;
            }
        }
        // <video>/<audio> are replaced elements: their <source>/<track>
        // children and fallback content never render.
        let replaced_media = dom_node
            .as_element()
            .is_some_and(|el| matches!(el.name.local.as_ref(), "video" | "audio"));

        // A <select> keeps its box but not its options: publish the selected
        // option onto the element so the control paints/submits a value.
        publish_select_value(dom_node);

        let mut kids: Vec<(taffy::NodeId, bool)> = Vec::new();
        // A textarea's text is its VALUE, painted inside the control, not
        // flow content (the control is a leaf with an intrinsic size).
        let is_textarea = dom_node.as_element().is_some_and(|e| e.name.local.as_ref() == "textarea");
        for child in dom_node.children().filter(|_| !is_textarea && !replaced_media) {
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

        // A text run that is its box's ONLY child may shrink to the box's
        // width and wrap inside it (its min-content is its widest word).
        // With shrink 0 its flex base size was its max-content width capped
        // at the VIEWPORT, so every paragraph narrower than the viewport
        // painted past its own right edge. Runs that share a line with
        // siblings keep shrink 0: shrunk side by side, each would wrap into
        // its own column instead of flowing onto the next line.
        if let [only] = child_ids.as_slice() {
            let only_is_text = dom_node
                .children()
                .filter(|c| generates_box(c))
                .exactly_one_text();
            if only_is_text {
                if let Ok(st) = taffy.style(*only) {
                    let mut st = st.clone();
                    st.flex_shrink = 1.0;
                    let _ = taffy.set_style(*only, st);
                }
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
            ..Default::default()
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
        let mut paint = PaintStyle::default();
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
        inline: HashMap::new(),
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

pub fn control_kind(node: &NodeRef) -> Option<Control> {
    let el = node.as_element()?;
    match el.name.local.as_ref() {
        "button" => Some(Control::Button),
        "select" => Some(Control::Select),
        "textarea" => Some(Control::Textarea),
        "input" => {
            let a = el.attributes.borrow();
            let t = a.get("type").unwrap_or("text").trim().to_ascii_lowercase();
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
fn text_advance(text: &str, family: u16, size: f32) -> f32 {
    let sel = crate::fonts::FontSel::new(family, 400, false);
    if crate::fonts::face(&sel).is_none() {
        return text.len() as f32 * size * 0.5;
    }
    crate::fonts::lines::Advancer::new(sel, size, 0.0).str(text)
}

fn control_line_height(family: u16) -> f32 {
    crate::fonts::face(&crate::fonts::FontSel::new(family, 400, false))
        .map(|f| crate::fonts::line_height(f, CONTROL_FONT_PX, 0.0))
        .unwrap_or(15.0)
}

/// UA (html.css + Chromium's control theme) box metrics of form controls:
/// text fields `padding: 1px 2px; border: 2px inset` with one line of the
/// 13.333px control font and the `size=20` default width; checkbox and
/// radio 13x13 with their 3px/4px (radio 3px 3px 0 5px) margins; buttons
/// `padding: 1px 6px; border: 2px outset`; select a 1px border around its
/// label and the arrow; textarea `padding: 2px; border: 1px` sized by
/// rows x cols of the monospace control font.
fn ua_control(node: &NodeRef, _tag: &str, style: &mut Style) {
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
fn control_intrinsic(node: &NodeRef) -> Option<(f32, f32)> {
    let kind = control_kind(node)?;
    let attr = |name: &str| node.as_element().and_then(|e| e.attributes.borrow().get(name).map(str::to_string));
    let lh = control_line_height(0);
    match kind {
        Control::Field => {
            let size = attr("size").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(20.0);
            Some((size * 7.45, lh))
        }
        Control::Button if node.as_element().is_some_and(|e| e.name.local.as_ref() == "input") => {
            let t = attr("type").unwrap_or_default().to_ascii_lowercase();
            let label = attr("value").unwrap_or_else(|| if t == "reset" { "Reset".into() } else { "Submit".into() });
            Some((text_advance(&label, 0, CONTROL_FONT_PX), lh))
        }
        Control::Select => {
            let label = select_selected_option(node).map(|(_, l)| l).unwrap_or_default();
            Some((text_advance(&label, 0, CONTROL_FONT_PX) + 4.0 + 20.0, lh + 2.0))
        }
        Control::Textarea => {
            let rows = attr("rows").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(2.0);
            let cols = attr("cols").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(20.0);
            let cw = text_advance("0", 2, CONTROL_FONT_PX);
            Some((cols * cw + 17.0, rows * control_line_height(2)))
        }
        _ => None,
    }
}

/// Controls the UA sheet sizes as border-box: buttons, selects, and the
/// button-like / checkable inputs.
fn ua_border_box(node: &NodeRef, tag: &str) -> bool {
    match tag {
        "button" | "select" => true,
        "input" => node.as_element().is_some_and(|e| {
            let a = e.attributes.borrow();
            let t = a.get("type").unwrap_or("text").trim().to_ascii_lowercase();
            matches!(t.as_str(), "submit" | "reset" | "button" | "checkbox" | "radio" | "image" | "color" | "file")
        }),
        _ => false,
    }
}

/// True when a list sits inside another list: the UA sheet drops the
/// block margins of nested lists (`ul ul, ol ul, ... { margin-block: 0 }`).
fn nested_list(node: &NodeRef) -> bool {
    let is_list = |n: &NodeRef| {
        n.as_element()
            .is_some_and(|e| matches!(e.name.local.as_ref(), "ul" | "ol" | "menu" | "dir" | "dl"))
    };
    is_list(node) && node.ancestors().any(|a| is_list(&a) && a.as_element().is_some_and(|e| e.name.local.as_ref() != "dl"))
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
pub fn default_family(tag: &str, inherited: u16) -> u16 {
    match tag {
        "code" | "pre" | "kbd" | "samp" | "tt" | "textarea" | "listing" | "xmp" | "plaintext" => crate::fonts::MONO,
        "input" | "select" | "button" => control_family(),
        _ => inherited,
    }
}

/// The family-list id of `Arial` (the control font's computed family).
pub fn control_family() -> u16 {
    static ID: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *ID.get_or_init(|| crate::fonts::intern_family_list(vec![crate::fonts::Family::Named("Arial".into())]))
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
pub fn default_rtl(node: &NodeRef, inherited: bool) -> bool {
    let Some(el) = node.as_element() else { return inherited };
    match el.attributes.borrow().get("dir").map(|d| d.trim().to_ascii_lowercase()) {
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

/// css-text-3 §4.1.1 phase I across element boundaries: within one inline
/// formatting context, a collapsible space at the start of a run is
/// removed when the content before it already ended in a space (or at the
/// start of the line), and the last run's trailing space hangs (removed).
/// The surviving spaces become the runs' `ws_lead`/`ws_trail` flags, which
/// the line breaker turns into real advances.
fn is_block_level(tree: &LayoutTree, id: taffy::NodeId) -> bool {
    let Some(n) = tree.node_map.get(&id) else { return true };
    let Some(el) = n.as_element() else { return false };
    match tree.paint_map.get(&id).and_then(|p| p.display_kind) {
        Some(k) => k == 1,
        None => !is_inline_node(n) || matches!(el.name.local.as_ref(), "td" | "th"),
    }
}

/// The previous (`next` = false) or next sibling box of `id` in the box tree.
fn neighbour_box(tree: &LayoutTree, id: taffy::NodeId, next: bool) -> Option<taffy::NodeId> {
    let parent = tree.taffy.parent(id)?;
    let kids = tree.taffy.children(parent).ok()?;
    let i = kids.iter().position(|&k| k == id)?;
    if next { kids.get(i + 1).copied() } else { i.checked_sub(1).and_then(|j| kids.get(j).copied()) }
}

fn collapse_inline_whitespace(tree: &mut LayoutTree) {
    struct Ifc {
        prev_space: bool,
        last_text: Option<taffy::NodeId>,
    }
    fn end_ifc(tree: &mut LayoutTree, ifc: &mut Ifc) {
        if let Some(t) = ifc.last_text.take() {
            tree.paint_map.entry(t).or_default().ws_trail = Some(false);
        }
        ifc.prev_space = true;
    }
    fn walk(tree: &mut LayoutTree, id: taffy::NodeId, ws: u8, ifc: &mut Ifc) {
        let dom = tree.node_map.get(&id).cloned();
        let mut ws = ws;
        let mut block = dom.is_none(); // anonymous boxes wrap block-level runs
        if let Some(n) = &dom {
            if let Some(el) = n.as_element() {
                let tag = el.name.local.as_ref();
                ws = tree.paint_map.get(&id).and_then(|p| p.white_space).unwrap_or_else(|| default_white_space(tag, ws));
                block = is_block_level(tree, id);
                if !block && (tag == "br" || inline::is_atomic(tree, id)) {
                    // Atomic inline: content that is not a space.
                    ifc.prev_space = tag == "br";
                    ifc.last_text = None;
                    return;
                }
            } else if n.as_text().is_some() {
                let raw = n.text_contents();
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
fn is_inline_level(tree: &LayoutTree, id: taffy::NodeId) -> bool {
    let Some(n) = tree.node_map.get(&id) else { return false }; // anonymous block
    if n.as_text().is_some() {
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
fn fix_display_contexts(tree: &mut LayoutTree) {
    let ids: Vec<taffy::NodeId> = tree.node_map.keys().copied().collect();
    for id in ids {
        let Some(n) = tree.node_map.get(&id) else { continue };
        let Some(el) = n.as_element() else { continue };
        if is_table_part(el.name.local.as_ref()) {
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
        let block_in = |tree: &LayoutTree, k: taffy::NodeId| {
            tree.node_map.get(&k).is_some_and(|kn| {
                kn.as_element().is_some()
                    && tree.paint_map.get(&k).and_then(|p| p.display_kind) == Some(1)
                    && is_inline_node(kn)
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
                kn.as_element().is_some() && tree.paint_map.get(&k).and_then(|p| p.display_kind).is_some_and(|d| (d != 1) != is_inline_node(kn))
            })
        });
        if !contradicts {
            continue;
        }
        // The children's own boxes follow their new outer type.
        for &k in &kids {
            let Some(kn) = tree.node_map.get(&k) else { continue };
            if kn.as_element().is_none() {
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
            if st.align_items.is_none() {
                st.align_items = Some(taffy::style::AlignItems::BASELINE);
            }
            let _ = tree.taffy.set_style(id, st);
            continue;
        }
        st.flex_direction = FlexDirection::Column;
        st.flex_wrap = taffy::style::FlexWrap::NoWrap;
        if st.align_items == Some(taffy::style::AlignItems::BASELINE) {
            st.align_items = None;
        }
        let _ = tree.taffy.set_style(id, st);
        // Wrap each run of inline children (two or more, or any text) in
        // an anonymous block box.
        let mut out: Vec<taffy::NodeId> = Vec::new();
        let mut run: Vec<taffy::NodeId> = Vec::new();
        let flush = |run: &mut Vec<taffy::NodeId>, out: &mut Vec<taffy::NodeId>, tree: &mut LayoutTree| {
            // Spaces at the edges of a run sit next to block boxes: they
            // are in no line (§4.1.1) and must not make one.
            let ws_only = |tree: &LayoutTree, k: &taffy::NodeId| {
                tree.node_map.get(k).is_some_and(|n| n.as_text().is_some() && n.text_contents().trim().is_empty())
            };
            while run.first().is_some_and(|k| ws_only(tree, k)) {
                let k = run.remove(0);
                let _ = tree.taffy.remove_child(id, k);
            }
            while run.last().is_some_and(|k| ws_only(tree, k)) {
                let k = run.pop().unwrap();
                let _ = tree.taffy.remove_child(id, k);
            }
            let has_text = run.iter().any(|k| tree.node_map.get(k).is_some_and(|n| n.as_text().is_some()));
            if run.len() < 2 && !has_text {
                out.append(run);
                return;
            }
            let anon = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_wrap: taffy::style::FlexWrap::Wrap,
                align_items: Some(taffy::style::AlignItems::BASELINE),
                size: Size { width: Dimension::percent(1.0), height: Dimension::auto() },
                box_sizing: taffy::style::BoxSizing::ContentBox,
                ..Default::default()
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
fn rescale_ua_margins(tree: &mut LayoutTree, sizes: &HashMap<taffy::NodeId, f32>) {
    for (&id, &fs) in sizes {
        if fs < 0.0 {
            // sup/sub: Blink's shift (parent/3 + 1 up, parent/5 + 1 down)
            // as margin on the shifted side, so the line box contains it.
            let parent = -fs;
            let sup = tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| e.name.local.as_ref() == "sup")).unwrap_or(false);
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
fn min_content_width(tree: &LayoutTree, id: taffy::NodeId, text: &HashMap<taffy::NodeId, TextRun>) -> f32 {
    if let Some(run) = text.get(&id) {
        return measure_text_run(run, 0.0).0;
    }
    let Ok(l) = tree.taffy.layout(id) else { return 0.0 };
    let edges = l.padding.left + l.padding.right + l.border.left + l.border.right;
    let kids = tree.taffy.children(id).unwrap_or_default();
    if kids.is_empty() {
        // A leaf: replaced/control boxes keep their width; empty boxes are 0
        // unless the author sized them.
        return if tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true)
            || tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| matches!(e.name.local.as_ref(), "img" | "input" | "select" | "button" | "textarea" | "video" | "svg" | "canvas"))).unwrap_or(false)
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
            min_content_width(tree, k, text) + m
        })
        .fold(0.0f32, f32::max);
    if tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true) {
        return l.size.width.max(inner + edges);
    }
    inner + edges
}

/// The computed font-size of an element (css-fonts-4 §2.5): an em/% value
/// of the parent's size, rem of the root's (the rem basis in the font
/// context), an absolute value as given, else the UA default for the tag.
fn computed_font_size(paint: Option<&PaintStyle>, tag: &str, parent: f32, parent_family: u16, own_family: u16, parent_absolute: bool) -> f32 {
    match paint.and_then(|p| p.font_size_rel) {
        Some((0, f)) => parent * f,
        Some((1, f)) => crate::css::font_ctx().1 * f,
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
fn root_font_size(tree: &LayoutTree) -> f32 {
    let html = tree.node_map.iter().find_map(|(id, n)| {
        n.as_element().filter(|e| e.name.local.as_ref() == "html").map(|_| *id)
    });
    let Some(p) = html.and_then(|h| tree.paint_map.get(&h)) else { return 16.0 };
    match p.font_size_rel {
        Some((0 | 1, f)) => 16.0 * f,
        _ => p.font_size.unwrap_or(16.0),
    }
}

/// Re-applies the length declarations of every element that has a
/// font-relative one, with the element's own font as the basis. Returns
/// whether anything was re-applied.
fn reresolve_font_relative(tree: &mut LayoutTree, elems: &HashMap<taffy::NodeId, TextRun>, root_fs: f32) -> bool {
    let todo: Vec<(taffy::NodeId, Vec<(String, String)>)> = tree
        .paint_map
        .iter()
        .filter_map(|(id, p)| {
            let list = p.font_rel.as_ref()?;
            list.iter()
                .any(|(prop, v)| crate::css::is_font_relative(v) || (prop == "line-height" && v.trim_end().ends_with('%')))
                .then(|| (*id, list.clone()))
        })
        .collect();
    if todo.is_empty() {
        return false;
    }
    for (id, decls) in todo {
        let Some(run) = elems.get(&id) else { continue };
        let fs = run.font_size;
        let (ex, ch) = match crate::fonts::face(&run.sel()) {
            Some(f) => {
                let m = f.metrics();
                let ex = m.x_height * fs / m.units_per_em as f32;
                let ch = crate::fonts::lines::Advancer::new(run.sel(), fs, 0.0).char('0');
                (if ex > 0.0 { ex } else { fs * 0.5 }, ch)
            }
            None => (fs * 0.5, fs * 0.5),
        };
        crate::css::set_font_ctx((fs, root_fs, ex, ch));
        let spec = crate::css::reapply_declarations(&decls);
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            spec.fold_into(&mut st);
            let _ = tree.taffy.set_style(id, st);
        }
        if let Some(p) = tree.paint_map.get_mut(&id) {
            crate::css::merge_paint(p, &spec.paint);
        }
    }
    crate::css::set_font_ctx((16.0, root_fs, 8.0, 8.0));
    true
}

/// Recomputes layout with real text measurement: resolves each text run's
/// inherited font size, then lets taffy size text leaves by wrapped extent.
/// Called after building the tree and after every cascade application.
pub fn remeasure(tree: &mut LayoutTree) {
    fix_display_contexts(tree);
    propagate_text_align(tree);
    collapse_inline_whitespace(tree);
    // Pass 1: resolve font size down the box tree for text leaves, and
    // intrinsic sizes for images.
    let mut text_info: HashMap<taffy::NodeId, TextRun> = HashMap::new();
    let mut img_info: HashMap<taffy::NodeId, (f32, f32)> = HashMap::new();
    // Replaced media boxes: (width, height) with the intrinsic aspect ratio, so a box sized on
    // one axis by CSS derives the other (CSS 2 §10.3.2 / §10.6.2 for replaced elements).
    let mut media_info: HashMap<taffy::NodeId, (f32, f32, f32)> = HashMap::new();
    fn resolve(
        node_id: taffy::NodeId,
        inherited: TextRun, // `text` unused here: the inherited text properties
        tree: &LayoutTree,
        out: &mut HashMap<taffy::NodeId, TextRun>,
        imgs: &mut HashMap<taffy::NodeId, (f32, f32)>,
        media: &mut HashMap<taffy::NodeId, (f32, f32, f32)>,
        sizes: &mut HashMap<taffy::NodeId, f32>,
        elems: &mut HashMap<taffy::NodeId, TextRun>,
    ) {
        let mut size = inherited.clone();
        if let Some(ta) = tree.paint_map.get(&node_id).and_then(|p| p.text_align) {
            size.text_align = ta;
        }
        if let Some(dom_node) = tree.node_map.get(&node_id) {
            if let Some(el) = dom_node.as_element() {
                let paint = tree.paint_map.get(&node_id);
                let tag = el.name.local.as_ref();
                let own_family = paint
                    .and_then(|p| p.family)
                    .unwrap_or_else(|| default_family(tag, inherited.family));
                size.font_size = computed_font_size(paint, tag, inherited.font_size, inherited.family, own_family, inherited.fs_absolute);
                match paint.and_then(|p| p.font_size_rel) {
                    Some((0, _)) => {}
                    Some(_) => size.fs_absolute = true,
                    None if paint.and_then(|p| p.font_size).is_some() => size.fs_absolute = true,
                    None => {}
                }
                if paint.is_some_and(|p| p.ua_vmargin.is_some()) {
                    sizes.insert(node_id, size.font_size);
                }
                if matches!(tag, "sup" | "sub") {
                    // vertical-align: super/sub grows the line box by the
                    // shift (the painter raises/lowers the glyphs by it);
                    // the parent's size is keyed negative-free by tag below.
                    sizes.insert(node_id, -inherited.font_size);
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
                if let Some(v) = paint.and_then(|p| p.word_break) { size.mode.word_break = v; }
                if let Some(v) = paint.and_then(|p| p.overflow_wrap) { size.mode.overflow_wrap = v; }
                if let Some(v) = paint.and_then(|p| p.letter_spacing) { size.mode.letter_spacing = v; }
                if let Some(v) = paint.and_then(|p| p.word_spacing) { size.mode.word_spacing = v; }
                size.family = paint
                    .and_then(|p| p.family)
                    .unwrap_or_else(|| default_family(tag, inherited.family));
                size.weight = paint
                    .and_then(|p| p.weight)
                    .map(|w| w.resolve(inherited.weight))
                    .unwrap_or_else(|| default_weight(tag, inherited.weight));
                size.italic =
                    paint.and_then(|p| p.italic).unwrap_or_else(|| default_italic(tag, inherited.italic));
                if let Some(st) = paint.and_then(|p| p.stretch) {
                    size.stretch = st;
                }
                size.mode.rtl = paint.and_then(|p| p.rtl).unwrap_or_else(|| default_rtl(dom_node, inherited.mode.rtl));
                if let Some(tt) = paint.and_then(|p| p.text_transform) {
                    size.text_transform = tt;
                }
                if let Some((iw, ih)) = crate::media::intrinsic_size(dom_node) {
                    // width/height attributes win, the other axis keeps the intrinsic ratio.
                    let attrs = el.attributes.borrow();
                    let attr_px = |name: &str| attrs.get(name).and_then(|v| v.trim().parse::<f32>().ok());
                    let ratio = if ih > 0.0 { iw / ih } else { 2.0 };
                    let (w, h) = match (attr_px("width"), attr_px("height")) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, w / ratio),
                        (None, Some(h)) => (h * ratio, h),
                        (None, None) => (iw, ih),
                    };
                    media.insert(node_id, (w, h, ratio));
                }
                if let Some(wh) = control_intrinsic(dom_node) {
                    imgs.insert(node_id, wh);
                }
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
                // Measured exactly as painted: the raw run (line breaking
                // owns whitespace), transformed, same face, same mode.
                let raw = dom_node.text_contents();
                let text = crate::render::transform_text(&raw, inherited.text_transform);
                let p = tree.paint_map.get(&node_id);
                let mut mode = inherited.mode;
                mode.lead = p.and_then(|p| p.ws_lead).unwrap_or(false);
                mode.trail = p.and_then(|p| p.ws_trail).unwrap_or(false);
                out.insert(node_id, TextRun { text, mode, ..inherited.clone() });
            }
        }
        if tree.node_map.get(&node_id).is_none_or(|n| n.as_text().is_none()) {
            elems.insert(node_id, size.clone());
        }
        if let Ok(children) = tree.taffy.children(node_id) {
            for child in children {
                resolve(child, size.clone(), tree, out, imgs, media, sizes, elems);
            }
        }
    }
    // The initial font: Chromium's default standard font, Times New Roman
    // (Liberation Serif) at 16px — an unstyled page is serif.
    let root = TextRun { font_size: 16.0, family: crate::fonts::STANDARD, weight: 400, stretch: 100, ..Default::default() };
    let mut sizes: HashMap<taffy::NodeId, f32> = HashMap::new();
    let mut elem_info: HashMap<taffy::NodeId, TextRun> = HashMap::new();
    // The rem basis: the root element's computed font-size.
    let root_fs = root_font_size(tree);
    let prev_ctx = crate::css::set_font_ctx((16.0, root_fs, 8.0, 8.0));
    resolve(tree.root_node, root.clone(), tree, &mut text_info, &mut img_info, &mut media_info, &mut sizes, &mut elem_info);
    // css-values-4 §6.1: lengths in em/ex/ch are of the element's own
    // computed font (font-size: of the parent's), rem of the root's. The
    // cascade parsed them at 16px; each element holding any re-applies its
    // length declarations, in cascade order, against its real font.
    if reresolve_font_relative(tree, &elem_info, root_fs) {
        text_info.clear();
        img_info.clear();
        media_info.clear();
        sizes.clear();
        elem_info.clear();
        resolve(tree.root_node, root, tree, &mut text_info, &mut img_info, &mut media_info, &mut sizes, &mut elem_info);
    }
    crate::css::set_font_ctx(prev_ctx);
    for (id, run) in &elem_info {
        if tree.node_map.get(id).is_some_and(|n| n.as_element().is_some()) {
            if let Some(p) = tree.paint_map.get_mut(id) {
                p.used_font_size = Some(run.font_size);
            }
        }
    }
    rescale_ua_margins(tree, &sizes);
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

    // Taffy caches leaf measurements; a cascade pass can change resolved
    // font sizes without touching the leaf's style, so stale cached sizes
    // survive set_style dirtying. Invalidate every measured leaf.
    for node_id in text_info.keys().chain(img_info.keys()).chain(media_info.keys()) {
        let _ = tree.taffy.mark_dirty(*node_id);
    }

    let viewport = Size {
        width: AvailableSpace::Definite(tree.viewport.0),
        height: AvailableSpace::Definite(tree.viewport.1),
    };
    let vw_cap = tree.viewport.0;
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
    let passes = if pct_nodes.is_empty() && !has_tables { 1 } else { 2 };
    let run_pass = |tree: &mut LayoutTree| {
    let _ = tree.taffy.compute_layout_with_measure(
        tree.root_node,
        viewport,
        |inputs, node_id, _ctx, style| taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
            if let Some(&(w, h, ratio)) = media_info.get(&node_id) {
                return match (known.width, known.height) {
                    (Some(kw), Some(kh)) => Size { width: kw, height: kh },
                    (Some(kw), None) => Size { width: kw, height: kw / ratio },
                    (None, Some(kh)) => Size { width: kh * ratio, height: kh },
                    (None, None) => Size { width: w, height: h },
                };
            }
            if let Some(&(w, h)) = img_info.get(&node_id) {
                return Size {
                    width: known.width.unwrap_or(w),
                    height: known.height.unwrap_or(h),
                };
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
            let (w, h) = measure_text_run(run, wrap_width);
            Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(h),
            }
        }),
    );
    };
    for pass in 0..passes {
    if pass == 1 && has_tables {
        // Column grid from the first pass's max-content cell widths and
        // each cell's min-content width.
        let min_content = |t: &LayoutTree, id: taffy::NodeId| min_content_width(t, id, &text_info);
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
                let Some(v) = e.as_deref().and_then(|e| crate::css::eval_length(e, Some(cb))) else { continue };
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
    // Inline formatting contexts (CSS 2.2 §9.4.2): line boxes at the widths
    // the block pass settled; each root's height becomes the sum of its
    // line boxes, and the tree is laid out again until heights are stable
    // (an inline-block's height feeds the line that holds it).
    let mut saved_heights: Vec<(taffy::NodeId, Dimension)> = Vec::new();
    let mut saved_min: Vec<(taffy::NodeId, LengthPercentageAuto)> = Vec::new();
    let is_cell = |tree: &LayoutTree, id: taffy::NodeId| {
        tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| matches!(e.name.local.as_ref(), "td" | "th"))).unwrap_or(false)
    };
    for _ in 0..4 {
        let heights = inline::layout_all(tree, &text_info, &elem_info);
        let mut changed = false;
        for (id, h) in heights {
            let Ok(st) = tree.taffy.style(id) else { continue };
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
    collapse::restore(tree, collapsed);
    blockwidth::restore(tree, widths);
}

/// One text run's resolved text properties (inherited down the box tree the
/// same way render::draw_node inherits them).
#[derive(Clone, Default)]
struct TextRun {
    text: String,
    font_size: f32,
    /// Multiplier of font size; 0 = the face's natural line height.
    line_height: f32,
    nowrap: bool,
    mode: crate::fonts::lines::TextMode,
    /// font-family list id (fonts::family_list).
    family: u16,
    /// Computed font-weight (1..=1000).
    weight: u16,
    italic: bool,
    /// font-stretch, percent.
    stretch: u16,
    text_transform: u8,
    /// Inherited text-align: 0 start, 1 center, 2 end.
    text_align: u8,
    /// The font-size still derives from the initial `medium` keyword (no
    /// absolute size up the chain): only then does switching to monospace
    /// scale it by 13/16 (Blink's fixed-pitch default).
    fs_absolute: bool,
}

impl TextRun {
    /// The font selection of the run.
    fn sel(&self) -> crate::fonts::FontSel {
        crate::fonts::FontSel { family: self.family, weight: self.weight, italic: self.italic, stretch: self.stretch }
    }
    /// The run's measurer: its face stack, size, spacing and direction.
    fn advancer(&self) -> crate::fonts::lines::Advancer {
        crate::fonts::lines::Advancer::new(self.sel(), self.font_size, self.mode.letter_spacing)
            .with_word_spacing(self.mode.word_spacing)
            .with_dir(self.mode.rtl)
    }
}

/// Measures a text run: (widest line, lines x line height), through the
/// same line breaker the painter uses (fonts::lines).
fn measure_text_run(run: &TextRun, max_width: f32) -> (f32, f32) {
    let Some(font) = crate::fonts::face(&run.sel()) else {
        return (max_width, run.font_size * 1.25);
    };
    let mut mode = run.mode;
    if run.nowrap && mode.white_space == 0 {
        mode.white_space = 1;
    }
    let adv = run.advancer();
    let lines = crate::fonts::lines::break_lines(&adv, &run.text, &mode, max_width);
    let widest = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    let lh = crate::fonts::line_height(font, run.font_size, run.line_height);
    // A whitespace-only run that collapsed away is zero-sized.
    if lines.len() == 1 && lines[0].text.is_empty() {
        return (0.0, 0.0);
    }
    (widest, lines.len() as f32 * lh)
}

/// Applies a `style="..."` attribute through the shared declaration parser.
fn apply_inline_style(inline: &str, style: &mut Style, paint: &mut PaintStyle) {
    let spec = crate::css::parse_declaration_block(inline);
    spec.fold_into(style);
    crate::css::merge_paint(paint, &spec.paint);
}
