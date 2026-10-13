//! AETHERGEN (SR71): generated content — CSS Generated Content Level 3 and css-lists-3.
//!
//! CHARTER: Aether owns the browser (docs/CODEX.md §2); this is its layout box generation, inside the
//! live handler.
//!
//! The cascade (`css::apply_stylesheets`) hands this module every element's cascaded style and the
//! cascaded styles of its `::marker`, `::before` and `::after`. One walk in tree order then does what
//! needs document order:
//!
//! - **counters** (css-lists-3 §4): `counter-reset`, then `counter-increment`, then `counter-set` on each
//!   element, the implicit `list-item` counter of list items (reset by `ol`/`ul`/`menu`/`dir` in the UA
//!   sheet, `start`/`value` honoured), counter scope by the CSS 2.2 §12.4.1 nesting rule (a counter an
//!   element instantiates is in scope for it, its following siblings and their descendants);
//! - **quotes** (css-content-3 §3): one quote depth across the document, `quotes` inherited, `auto`
//!   giving the UA pairs of the element's content language;
//! - **content** (css-content-3 §2): strings, `attr()`, `counter()`/`counters()` in the predefined
//!   counter styles, the four quote keywords, and `url()` as a replaced inline image.
//!
//! A `::before`/`::after` whose `content` computes to items (not `normal`/`none`) and whose `display`
//! is not `none` becomes a box: a detached element of the document (never inserted into the DOM, so no
//! selector, script or serializer sees it) built by the ordinary box builder and placed as the FIRST or
//! LAST child of the originating element's box. Its style is the pseudo-element's own cascade, folded
//! by the caller like any box's; paint and font properties inherit from the originating box through the
//! box tree, which is what "the originating element as parent" means for inheritance here. It is inline
//! (or block, inline-block, absolutely positioned, ...) by its computed `display`/`position`, so it
//! takes part in the inline formatting context and in painting with no special case. `<q>` has the UA
//! sheet's `q::before { content: open-quote }` / `q::after { content: close-quote }`.
//!
//! A list item's `::marker` style (author `content`, `color`, `font-*`) becomes the item's
//! [`MarkerPaint`], which `render` paints in place of (or styling) the default list marker.

use super::LayoutTree;
use crate::css::{Content, ContentItem, PseudoStyles, Quotes, SpecifiedStyle};
use crate::dom::NodeRef;
use css_core::matching::Element as _;
use html_core::{Document, NodeId};
use std::collections::HashMap;
use std::rc::Rc;

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
    pub weight: Option<super::FontWeight>,
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

/// One piece of a generated box's content.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Image(String),
}

/// The counters in scope during the tree walk: per name, a stack of (the element whose children see
/// it as a sibling's counter, value).
#[derive(Default)]
struct Counters {
    map: HashMap<String, Vec<(NodeId, i32)>>,
}

impl Counters {
    /// Instantiates (or, when a preceding sibling instantiated one, re-sets) a counter on an element
    /// whose parent is `parent`; `created` is the parent's list of names its children instantiated.
    fn reset(&mut self, name: &str, v: i32, parent: NodeId, created: &mut Vec<String>) {
        let st = self.map.entry(name.to_string()).or_default();
        if let Some(top) = st.last_mut() {
            if top.0 == parent {
                top.1 = v;
                return;
            }
        }
        st.push((parent, v));
        created.push(name.to_string());
    }
    fn top(&mut self, name: &str, parent: NodeId, created: &mut Vec<String>) -> &mut i32 {
        if self.map.get(name).is_none_or(|s| s.is_empty()) {
            self.reset(name, 0, parent, created);
        }
        &mut self.map.get_mut(name).unwrap().last_mut().unwrap().1
    }
    fn increment(&mut self, name: &str, by: i32, parent: NodeId, created: &mut Vec<String>) {
        let t = self.top(name, parent, created);
        *t = t.wrapping_add(by);
    }
    fn set(&mut self, name: &str, v: i32, parent: NodeId, created: &mut Vec<String>) {
        *self.top(name, parent, created) = v;
    }
    /// Ends the scopes the children of an element opened.
    fn pop(&mut self, created: &[String]) {
        for n in created.iter().rev() {
            if let Some(st) = self.map.get_mut(n) {
                st.pop();
            }
        }
    }
    /// `counter()`: the innermost counter's value (0 when none is in scope).
    fn value(&self, name: &str) -> i32 {
        self.map.get(name).and_then(|s| s.last()).map(|t| t.1).unwrap_or(0)
    }
    /// `counters()`: every counter of the name in scope, outermost first.
    fn values(&self, name: &str) -> Vec<i32> {
        let v: Vec<i32> = self.map.get(name).map(|s| s.iter().map(|t| t.1).collect()).unwrap_or_default();
        if v.is_empty() { vec![0] } else { v }
    }
}

/// The UA quote pairs for a content language (`quotes: auto`, css-content-3 §3.1; CLDR delimiters).
fn ua_quotes(lang: Option<&str>) -> Vec<(String, String)> {
    let primary = lang.unwrap_or("en").split(['-', '_']).next().unwrap_or("en").to_ascii_lowercase();
    let q: [(&str, &str); 2] = match primary.as_str() {
        "de" | "cs" | "sk" | "lt" | "is" => [("\u{201E}", "\u{201C}"), ("\u{201A}", "\u{2018}")],
        "fr" | "es" | "it" | "ca" | "el" => [("\u{00AB}", "\u{00BB}"), ("\u{201C}", "\u{201D}")],
        "ru" | "uk" | "be" => [("\u{00AB}", "\u{00BB}"), ("\u{201E}", "\u{201C}")],
        "pl" | "hu" | "ro" => [("\u{201E}", "\u{201D}"), ("\u{00AB}", "\u{00BB}")],
        "ja" => [("\u{300C}", "\u{300D}"), ("\u{300E}", "\u{300F}")],
        _ => [("\u{201C}", "\u{201D}"), ("\u{2018}", "\u{2019}")],
    };
    q.iter().map(|(o, c)| (o.to_string(), c.to_string())).collect()
}

/// Where the tree walk collects its results.
struct Walk<'a> {
    doc: &'a Document,
    styles: &'a HashMap<NodeId, Rc<SpecifiedStyle>>,
    pseudos: &'a HashMap<NodeId, PseudoStyles>,
    boxed: &'a HashMap<NodeId, Vec<taffy::NodeId>>,
    counters: Counters,
    quote_depth: usize,
    /// (originating element, 1 = ::before / 2 = ::after, content, the pseudo-element's style).
    boxes: Vec<(NodeId, u8, Vec<Part>, Rc<SpecifiedStyle>)>,
    markers: Vec<(NodeId, MarkerPaint)>,
    q_before: Rc<SpecifiedStyle>,
    q_after: Rc<SpecifiedStyle>,
}

/// Elements that take no ::before/::after box here: replaced elements and form controls (css-content-3
/// §2: no generated content on replaced elements), and the table-internal boxes the table layout
/// owns.
fn no_generated_boxes(tag: &str) -> bool {
    matches!(
        tag,
        "img" | "input" | "textarea" | "select" | "video" | "audio" | "iframe" | "object" | "embed"
            | "canvas" | "br" | "wbr" | "meter" | "progress" | "html"
            | "table" | "thead" | "tbody" | "tfoot" | "tr" | "colgroup" | "col"
    )
}

impl Walk<'_> {
    fn element(&mut self, id: NodeId, parent: NodeId, inherited_quotes: &Quotes, sibling_created: &mut Vec<String>) {
        if !self.boxed.contains_key(&id) {
            return;
        }
        let style = self.styles.get(&id).cloned();
        if style.as_ref().is_some_and(|s| s.display == Some(taffy::style::Display::None)) {
            return;
        }
        let Some(el) = self.doc.element(id) else { return };
        let tag = el.local.as_str();
        let html = el.ns == html_core::Namespace::Html;
        let gs = style.as_ref().map(|s| s.generated.clone()).unwrap_or_default();
        let quotes = gs.quotes.clone().unwrap_or_else(|| inherited_quotes.clone());
        let list_item = style.as_ref().and_then(|s| s.paint.list_item).unwrap_or(html && tag == "li");

        // css-lists-3 §4.5: reset, then increment, then set.
        let attr_int = |n: &str| el.attr(n).and_then(|v| v.trim().parse::<i32>().ok());
        match &gs.counter_reset {
            Some(list) => {
                for (n, v) in list {
                    self.counters.reset(n, *v, parent, sibling_created);
                }
            }
            // The UA sheet: `ol, ul, menu, dir { counter-reset: list-item }` (`ol[start]` from start - 1).
            None if html && matches!(tag, "ol" | "ul" | "menu" | "dir") => {
                let start = if tag == "ol" { attr_int("start").map(|s| s - 1).unwrap_or(0) } else { 0 };
                self.counters.reset("list-item", start, parent, sibling_created);
            }
            None => {}
        }
        let inc = gs.counter_increment.clone().unwrap_or_default();
        for (n, v) in &inc {
            self.counters.increment(n, *v, parent, sibling_created);
        }
        if list_item && !inc.iter().any(|(n, _)| n == "list-item") {
            self.counters.increment("list-item", 1, parent, sibling_created);
        }
        for (n, v) in gs.counter_set.iter().flatten() {
            self.counters.set(n, *v, parent, sibling_created);
        }
        if list_item && html && tag == "li" {
            if let Some(v) = attr_int("value") {
                self.counters.set("list-item", v, parent, sibling_created);
            }
        }

        let mut created: Vec<String> = Vec::new();
        let ps = self.pseudos.get(&id).cloned().unwrap_or_default();
        if list_item {
            if let Some(m) = &ps[0] {
                self.pseudo_counters(m, id, &mut created);
                let mut mp = MarkerPaint {
                    color: m.paint.color,
                    font_size: m.paint.font_size,
                    font_size_rel: m.paint.font_size_rel,
                    weight: m.paint.weight,
                    italic: m.paint.italic,
                    family: m.paint.family,
                    ..Default::default()
                };
                match &m.generated.content {
                    Some(Content::None) => mp.none = true,
                    Some(Content::Items(items)) => {
                        let parts = self.resolve(id, items, &quotes);
                        mp.text = Some(
                            parts
                                .iter()
                                .filter_map(|p| match p {
                                    Part::Text(t) => Some(t.as_str()),
                                    Part::Image(_) => None,
                                })
                                .collect(),
                        );
                    }
                    _ => {}
                }
                self.markers.push((id, mp));
            }
        }
        let boxes_here = html && !no_generated_boxes(tag);
        if boxes_here {
            let before = ps[1].clone().or_else(|| (tag == "q").then(|| self.q_before.clone()));
            self.pseudo_box(id, 1, before, &quotes, &mut created);
        }
        for c in self.doc.element_children(id).collect::<Vec<_>>() {
            self.element(c, id, &quotes, &mut created);
        }
        if boxes_here {
            let after = ps[2].clone().or_else(|| (tag == "q").then(|| self.q_after.clone()));
            self.pseudo_box(id, 2, after, &quotes, &mut created);
        }
        self.counters.pop(&created);
    }

    /// A pseudo-element's own counter properties: it is a child of its originating element.
    fn pseudo_counters(&mut self, s: &SpecifiedStyle, origin: NodeId, created: &mut Vec<String>) {
        for (n, v) in s.generated.counter_reset.iter().flatten() {
            self.counters.reset(n, *v, origin, created);
        }
        for (n, v) in s.generated.counter_increment.iter().flatten() {
            self.counters.increment(n, *v, origin, created);
        }
        for (n, v) in s.generated.counter_set.iter().flatten() {
            self.counters.set(n, *v, origin, created);
        }
    }

    fn pseudo_box(&mut self, origin: NodeId, kind: u8, style: Option<Rc<SpecifiedStyle>>, quotes: &Quotes, created: &mut Vec<String>) {
        let Some(s) = style else { return };
        // css-content-3 §2: `normal` computes to `none` on ::before/::after; `none` generates no box,
        // and neither does `display: none`.
        let Some(Content::Items(items)) = &s.generated.content else { return };
        if s.display == Some(taffy::style::Display::None) {
            return;
        }
        let quotes = s.generated.quotes.clone().unwrap_or_else(|| quotes.clone());
        self.pseudo_counters(&s, origin, created);
        let parts = self.resolve(origin, items, &quotes);
        self.boxes.push((origin, kind, parts, s.clone()));
    }

    /// A content list's text and images, adjacent text joined.
    fn resolve(&mut self, origin: NodeId, items: &[ContentItem], quotes: &Quotes) -> Vec<Part> {
        let pairs = match quotes {
            Quotes::None => Vec::new(),
            Quotes::Pairs(p) => p.clone(),
            Quotes::Auto => ua_quotes(crate::dom::El::new(self.doc, origin).lang().as_deref()),
        };
        let quote = |depth: usize, open: bool| -> String {
            match pairs.get(depth.min(pairs.len().saturating_sub(1))) {
                Some((o, c)) => if open { o.clone() } else { c.clone() },
                None => String::new(),
            }
        };
        let mut parts: Vec<Part> = Vec::new();
        let push = |parts: &mut Vec<Part>, t: String| {
            if t.is_empty() {
                return;
            }
            if let Some(Part::Text(last)) = parts.last_mut() {
                last.push_str(&t);
            } else {
                parts.push(Part::Text(t));
            }
        };
        for item in items {
            match item {
                ContentItem::Str(s) => push(&mut parts, s.clone()),
                ContentItem::Attr(n) => {
                    let v = self.doc.element(origin).and_then(|e| e.attr(n)).map(|v| v.to_string()).unwrap_or_default();
                    push(&mut parts, v);
                }
                ContentItem::Counter(n, st) => push(&mut parts, crate::render::counter_text(*st, self.counters.value(n) as i64)),
                ContentItem::Counters(n, sep, st) => {
                    let v: Vec<String> =
                        self.counters.values(n).into_iter().map(|v| crate::render::counter_text(*st, v as i64)).collect();
                    push(&mut parts, v.join(sep));
                }
                ContentItem::OpenQuote => {
                    let q = quote(self.quote_depth, true);
                    self.quote_depth += 1;
                    push(&mut parts, q);
                }
                ContentItem::CloseQuote => {
                    if self.quote_depth > 0 {
                        self.quote_depth -= 1;
                        let q = quote(self.quote_depth, false);
                        push(&mut parts, q);
                    }
                }
                ContentItem::NoOpenQuote => self.quote_depth += 1,
                ContentItem::NoCloseQuote => self.quote_depth = self.quote_depth.saturating_sub(1),
                ContentItem::Url(u) => parts.push(Part::Image(u.clone())),
            }
        }
        parts
    }
}

thread_local! {
    /// The detached nodes standing for generated boxes, by (document arena, originating element,
    /// kind), with the content they were built for: a relayout with unchanged content reuses them
    /// instead of growing the arena.
    static NODES: std::cell::RefCell<HashMap<(usize, NodeId, u8), (NodeRef, Vec<Part>)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// The detached element standing for one generated box: a `span` (no UA style of its own) holding a
/// text node per text part and an `img` per image.
fn box_node(doc: &NodeRef, origin: NodeId, kind: u8, parts: &[Part]) -> NodeRef {
    let arena = Rc::as_ptr(&doc.doc) as usize;
    NODES.with(|n| {
        let mut n = n.borrow_mut();
        // Entries of an earlier document would keep its arena alive.
        n.retain(|k, _| k.0 == arena);
        if let Some((node, built)) = n.get(&(arena, origin, kind)) {
            if built.as_slice() == parts {
                return node.clone();
            }
            for c in node.children() {
                c.detach();
            }
        }
        let node = match n.get(&(arena, origin, kind)) {
            Some((node, _)) => node.clone(),
            None => doc.new_element(html_core::Namespace::Html, "span"),
        };
        for p in parts {
            match p {
                Part::Text(t) => node.append(doc.new_text(t.clone())),
                Part::Image(u) => {
                    let img = doc.new_element(html_core::Namespace::Html, "img");
                    if let Some(el) = img.as_element() {
                        el.attributes.borrow_mut().insert("src", u.clone());
                    }
                    node.append(img);
                }
            }
        }
        n.insert((arena, origin, kind), (node.clone(), parts.to_vec()));
        node
    })
}

/// Counters, quotes, `::marker` paint and the `::before`/`::after` boxes of a document whose boxes are
/// built (`boxes`: each element's boxes) and cascaded (`styles`, `pseudos`). Returns the generated
/// boxes with the style the caller folds onto each.
pub(crate) fn generate(
    tree: &mut LayoutTree,
    any: &NodeRef,
    styles: &HashMap<NodeId, Rc<SpecifiedStyle>>,
    pseudos: &HashMap<NodeId, PseudoStyles>,
    boxes: &HashMap<NodeId, Vec<taffy::NodeId>>,
) -> Vec<(taffy::NodeId, Rc<SpecifiedStyle>)> {
    let quote_style = |item: ContentItem| {
        let mut s = SpecifiedStyle::default();
        s.generated.content = Some(Content::Items(vec![item]));
        Rc::new(s)
    };
    let (gen_boxes, markers) = any.with_doc(|doc, _| {
        let mut w = Walk {
            doc,
            styles,
            pseudos,
            boxed: boxes,
            counters: Counters::default(),
            quote_depth: 0,
            boxes: Vec::new(),
            markers: Vec::new(),
            q_before: quote_style(ContentItem::OpenQuote),
            q_after: quote_style(ContentItem::CloseQuote),
        };
        let root = Document::ROOT;
        let mut created = Vec::new();
        for c in doc.element_children(root).collect::<Vec<_>>() {
            w.element(c, root, &Quotes::Auto, &mut created);
        }
        (w.boxes, w.markers)
    });
    for (id, mp) in markers {
        for tid in boxes.get(&id).into_iter().flatten() {
            tree.paint_map.entry(*tid).or_default().marker = Some(Rc::new(mp.clone()));
        }
    }
    if gen_boxes.is_empty() {
        return Vec::new();
    }
    let doc = any.document_node();
    let quirks = super::is_quirks(&doc);
    let mut out = Vec::with_capacity(gen_boxes.len());
    for (origin, kind, parts, mut style) in gen_boxes {
        let Some(&host) = boxes.get(&origin).and_then(|v| v.first()) else { continue };
        let node = box_node(&doc, origin, kind, &parts);
        let Some(gid) = super::build_taffy_tree(&node, &mut tree.taffy, &mut tree.node_map, &mut tree.paint_map, quirks) else {
            continue;
        };
        // The clearfix (`::after { content: ""; display: table; clear: both }`) after content that is
        // all inline-level or floated: CSS puts the floats and the line content before it and gives
        // it clearance. Kept as an empty inline box, the container stays one inline formatting
        // context (floats placed side by side) and `clear_floats` grows it past the floats.
        let kids = tree.taffy.children(host).unwrap_or_default();
        if kind == 2
            && parts.is_empty()
            && style.paint.display_kind == Some(1)
            && style.paint.clear.is_some_and(|c| c != 0)
            && style.paint.position_kind.is_none_or(|p| p < 2)
            && !kids.is_empty()
            && kids.iter().all(|&k| inline_or_float(tree, styles, k))
        {
            let mut s = SpecifiedStyle::default();
            s.paint.clear = style.paint.clear;
            s.paint.display_kind = Some(0);
            style = Rc::new(s);
        }
        // An inline-level pseudo-element joins the anonymous line box the builder made around the
        // originating element's leading (trailing) inline run; a block-level one is a child of its own.
        let inline_level = style.paint.display_kind != Some(1);
        let edge = if kind == 1 { kids.first() } else { kids.last() }.copied();
        let parent = match edge {
            Some(k) if inline_level && !tree.node_map.contains_key(&k) => k,
            _ => host,
        };
        if kind == 1 {
            let _ = tree.taffy.insert_child_at_index(parent, 0, gid);
        } else {
            let _ = tree.taffy.add_child(parent, gid);
        }
        if inline_level && parent == host {
            share_line(tree, styles, host, gid, &style);
        }
        out.push((gid, style));
    }
    out
}

/// True for a box this module generated (a detached element standing in the box tree).
pub(crate) fn is_generated_box(tree: &LayoutTree, id: taffy::NodeId) -> bool {
    tree.node_map.get(&id).is_some_and(|n| n.as_element().is_some() && n.parent().is_none())
}

/// Inline-level (or floated) by the cascade about to be folded: text, an element whose cascaded
/// `display` is inline-level or that floats, else the tag default. Anonymous boxes are block boxes.
fn inline_or_float(tree: &LayoutTree, styles: &HashMap<NodeId, Rc<SpecifiedStyle>>, k: taffy::NodeId) -> bool {
    let Some(n) = tree.node_map.get(&k) else { return false };
    if n.as_text().is_some() {
        return true;
    }
    let st = styles.get(&n.id());
    if st.and_then(|s| s.paint.float).is_some_and(|f| f != 0) {
        return true;
    }
    match st.and_then(|s| s.paint.display_kind) {
        Some(d) => d != 1,
        None => super::is_inline_node(n),
    }
}

/// An inline generated box placed directly in a container that also holds block-level boxes shares
/// its line with the inline run beside it (`li::before` + the item's text, before a nested list):
/// the run is wrapped in an anonymous block box, as the builder wraps inline runs (CSS 2.2 §9.2.1.1).
fn share_line(tree: &mut LayoutTree, styles: &HashMap<NodeId, Rc<SpecifiedStyle>>, host: taffy::NodeId, gid: taffy::NodeId, style: &SpecifiedStyle) {
    let kids = tree.taffy.children(host).unwrap_or_default();
    let is_inline = |tree: &LayoutTree, k: taffy::NodeId| {
        if k == gid { style.paint.display_kind != Some(1) } else { inline_or_float(tree, styles, k) }
    };
    if kids.iter().all(|&k| is_inline(tree, k)) {
        return;
    }
    let Some(i) = kids.iter().position(|&k| k == gid) else { return };
    let (mut a, mut b) = (i, i);
    while a > 0 && is_inline(tree, kids[a - 1]) {
        a -= 1;
    }
    while b + 1 < kids.len() && is_inline(tree, kids[b + 1]) {
        b += 1;
    }
    if a == b {
        return;
    }
    let run: Vec<taffy::NodeId> = kids[a..=b].to_vec();
    for k in &run {
        let _ = tree.taffy.remove_child(host, *k);
    }
    let anon = taffy::Style {
        display: taffy::style::Display::Flex,
        flex_direction: taffy::style::FlexDirection::Row,
        flex_wrap: taffy::style::FlexWrap::Wrap,
        align_items: taffy::style::AlignItems::BASELINE,
        size: taffy::geometry::Size { width: taffy::style::Dimension::percent(1.0), height: taffy::style::Dimension::auto() },
        position: taffy::style::Position::Relative,
        ..Default::default()
    };
    let Ok(id) = tree.taffy.new_with_children(anon, &run) else { return };
    tree.paint_map.insert(id, super::PaintStyle::default());
    let _ = tree.taffy.insert_child_at_index(host, a, id);
}
