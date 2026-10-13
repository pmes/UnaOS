//! A11YFACE (LEDGER SR124): the accessibility tree in AT-SPI2's cache shape, for a bridge.
//!
//! AT-SPI2 is a D-Bus protocol: an application registers on the accessibility bus and answers
//! `org.a11y.atspi.Cache.GetItems` with one `((so)(so)(so)iiassusau)` per object — its path, the application,
//! the parent, the index in the parent, the child count, the interfaces it implements, name, role
//! (`AtspiRole`), description and state set (two 32-bit words of `AtspiStateType` bits) — and serves
//! `Accessible`, `Component` (extents) and `Text` on each path. The workspace carries no D-Bus crate (root
//! Cargo.lock at c4c99344: no zbus, dbus, atspi or accesskit) and R83 admits no new heavy dependency without a
//! ruling, so **the bus side is refused**: this module computes exactly what such a bridge would publish, and
//! the shell (vessels/aether-shell) serves it on a unix socket of its own,
//! `$XDG_RUNTIME_DIR/unaos-aether-a11y-<pid>.sock` (mode 0600).
//!
//! The wire: one request line — `GetItems` (every object, document order), `GetRoot`, `GetFocus` (the focused
//! object, else the root) — answered by one object per line and a final line `.`; an unknown request is one
//! `ERR …` line. An object line is twelve tab-separated fields:
//!
//! `path  app  parent  index_in_parent  child_count  interfaces  name  role  description  states  extents  text`
//!
//! `interfaces` comma-separated (`org.a11y.atspi.Accessible`, `…Component` when the object has a box, `…Text`
//! when it has text); `states` the two words in decimal, `low,high`; `extents` `x,y,w,h` in window pixels
//! (`Component.GetExtents(WINDOW)`, empty without Component); `text` `Text.GetText(0, -1)` (empty without
//! Text) — a password field's text is masked. Tab, newline and backslash in a field are escaped `\t` `\n` `\\`.
//! The role and state numbers are at-spi2-core's `atspi-constants.h`, unverified against a live bus (none here).

use std::collections::HashMap;

use super::role;
use super::tree::{AxNode, AxTree, Tristate};
use crate::dom::NodeRef;
use crate::AetherEngine;

/// The application root's object path (AT-SPI's convention).
pub const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// Every other object: this prefix and the DOM node's arena id.
pub const PATH_PREFIX: &str = "/org/a11y/atspi/accessible/";
/// The parent of the root (AT-SPI's null object).
pub const NULL_PATH: &str = "/org/a11y/atspi/null";

pub const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub const COMPONENT: &str = "org.a11y.atspi.Component";
pub const TEXT: &str = "org.a11y.atspi.Text";

// AtspiRole values used by the tests and the shell.
pub const ROLE_CHECK_BOX: u32 = 7;
pub const ROLE_IMAGE: u32 = 27;
pub const ROLE_LIST: u32 = 31;
pub const ROLE_LIST_ITEM: u32 = 32;
pub const ROLE_PASSWORD_TEXT: u32 = 40;
pub const ROLE_PUSH_BUTTON: u32 = 43;
pub const ROLE_ENTRY: u32 = 79;
pub const ROLE_HEADING: u32 = 83;
pub const ROLE_SECTION: u32 = 85;
pub const ROLE_LINK: u32 = 88;
pub const ROLE_DOCUMENT_WEB: u32 = 95;
pub const ROLE_LANDMARK: u32 = 110;
pub const ROLE_STATIC: u32 = 116;

// AtspiStateType bit numbers.
pub const STATE_BUSY: u32 = 3;
pub const STATE_CHECKED: u32 = 4;
pub const STATE_COLLAPSED: u32 = 5;
pub const STATE_EDITABLE: u32 = 7;
pub const STATE_ENABLED: u32 = 8;
pub const STATE_EXPANDABLE: u32 = 9;
pub const STATE_EXPANDED: u32 = 10;
pub const STATE_FOCUSABLE: u32 = 11;
pub const STATE_FOCUSED: u32 = 12;
pub const STATE_MODAL: u32 = 16;
pub const STATE_MULTI_LINE: u32 = 17;
pub const STATE_MULTISELECTABLE: u32 = 18;
pub const STATE_PRESSED: u32 = 20;
pub const STATE_SELECTABLE: u32 = 22;
pub const STATE_SELECTED: u32 = 23;
pub const STATE_SENSITIVE: u32 = 24;
pub const STATE_SHOWING: u32 = 25;
pub const STATE_SINGLE_LINE: u32 = 26;
pub const STATE_VISIBLE: u32 = 30;
pub const STATE_INDETERMINATE: u32 = 32;
pub const STATE_REQUIRED: u32 = 33;
pub const STATE_INVALID_ENTRY: u32 = 36;
pub const STATE_CHECKABLE: u32 = 41;
pub const STATE_HAS_POPUP: u32 = 42;
pub const STATE_READ_ONLY: u32 = 43;

/// The `AtspiRole` for a computed role (WAI-ARIA 1.2 / HTML-AAM names; Core-AAM 1.2's ATK/AT-SPI column).
pub fn atspi_role(r: &str) -> u32 {
    match r {
        role::ROOT => ROLE_DOCUMENT_WEB,
        role::TEXT => ROLE_STATIC,
        "alert" => 2,
        "alertdialog" | "dialog" => 16,
        "application" => 75,
        "article" => 109,
        "banner" | "complementary" | "contentinfo" | "main" | "navigation" | "region" | "search" => ROLE_LANDMARK,
        "blockquote" => 105,
        "button" => ROLE_PUSH_BUTTON,
        "caption" => 81,
        "cell" | "gridcell" => 56,
        "checkbox" => ROLE_CHECK_BOX,
        "code" | "emphasis" | "strong" | "time" => ROLE_STATIC,
        "columnheader" => 10,
        "combobox" => 11,
        "comment" | "note" => 97,
        "definition" => 123,
        "deletion" => 125,
        "document" => 82,
        "feed" | "figure" | "rowgroup" | "tabpanel" | "group" => 39,
        "form" => 87,
        "grid" | "table" => 55,
        "heading" => ROLE_HEADING,
        "img" | "image" => ROLE_IMAGE,
        "insertion" => 126,
        "link" => ROLE_LINK,
        "list" | "directory" => ROLE_LIST,
        "listbox" => 98,
        "listitem" | "option" => ROLE_LIST_ITEM,
        "log" => 111,
        "marquee" => 112,
        "math" => 113,
        "menu" => 33,
        "menubar" => 34,
        "menuitem" => 35,
        "menuitemcheckbox" => 8,
        "menuitemradio" => 45,
        "meter" => 103,
        "paragraph" => 73,
        "progressbar" => 42,
        "radio" => 44,
        "radiogroup" => 99,
        "row" => 90,
        "rowheader" => 47,
        "scrollbar" => 48,
        "searchbox" | "textbox" => ROLE_ENTRY,
        "separator" => 50,
        "slider" => 51,
        "spinbutton" => 52,
        "status" => 54,
        "subscript" => 119,
        "superscript" => 120,
        "switch" => 130,
        "tab" => 37,
        "tablist" => 38,
        "term" => 122,
        "timer" => 115,
        "toolbar" => 63,
        "tooltip" => 64,
        "tree" => 65,
        "treegrid" => 66,
        "treeitem" => 91,
        _ => ROLE_SECTION,
    }
}

/// The state set of `n` as AT-SPI's two words (`has_box`: the object is laid out — VISIBLE and SHOWING).
pub fn atspi_states(n: &AxNode, has_box: bool) -> [u32; 2] {
    let s = &n.states;
    let mut bits: u64 = 0;
    let mut set = |b: u32| bits |= 1u64 << b;
    if !s.disabled {
        set(STATE_ENABLED);
        set(STATE_SENSITIVE);
    }
    if has_box {
        set(STATE_VISIBLE);
        set(STATE_SHOWING);
    }
    if s.focusable {
        set(STATE_FOCUSABLE);
    }
    if s.focused {
        set(STATE_FOCUSED);
    }
    if let Some(c) = s.checked {
        set(STATE_CHECKABLE);
        match c {
            Tristate::True => set(STATE_CHECKED),
            Tristate::Mixed => set(STATE_INDETERMINATE),
            Tristate::False => {}
        }
    }
    if s.pressed == Some(Tristate::True) {
        set(STATE_PRESSED);
    }
    if let Some(e) = s.expanded {
        set(STATE_EXPANDABLE);
        set(if e { STATE_EXPANDED } else { STATE_COLLAPSED });
    }
    if let Some(sel) = s.selected {
        set(STATE_SELECTABLE);
        if sel {
            set(STATE_SELECTED);
        }
    }
    if matches!(n.role, "textbox" | "searchbox" | "combobox") {
        set(if s.multiline { STATE_MULTI_LINE } else { STATE_SINGLE_LINE });
        if !s.readonly && !s.disabled {
            set(STATE_EDITABLE);
        }
    }
    for (on, b) in [
        (s.required, STATE_REQUIRED),
        (s.readonly, STATE_READ_ONLY),
        (s.multiselectable, STATE_MULTISELECTABLE),
        (s.invalid, STATE_INVALID_ENTRY),
        (s.modal, STATE_MODAL),
        (s.busy, STATE_BUSY),
        (s.haspopup.as_deref().is_some_and(|v| v != "false"), STATE_HAS_POPUP),
    ] {
        if on {
            set(b);
        }
    }
    [bits as u32, (bits >> 32) as u32]
}

/// One object of the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheItem {
    pub path: String,
    pub parent: String,
    pub index_in_parent: i32,
    pub child_count: i32,
    pub interfaces: Vec<&'static str>,
    pub name: String,
    pub role: u32,
    pub description: String,
    pub states: [u32; 2],
    /// `Component.GetExtents(WINDOW)`: x, y, w, h in window pixels.
    pub extents: Option<(i32, i32, i32, i32)>,
    /// `Text.GetText(0, -1)`.
    pub text: Option<String>,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

impl CacheItem {
    /// The object's wire line (the module doc's twelve fields), for application `app`.
    pub fn line(&self, app: &str) -> String {
        let ext = self.extents.map(|(x, y, w, h)| format!("{x},{y},{w},{h}")).unwrap_or_default();
        [
            esc(&self.path),
            esc(app),
            esc(&self.parent),
            self.index_in_parent.to_string(),
            self.child_count.to_string(),
            self.interfaces.join(","),
            esc(&self.name),
            self.role.to_string(),
            esc(&self.description),
            format!("{},{}", self.states[0], self.states[1]),
            ext,
            esc(self.text.as_deref().unwrap_or("")),
        ]
        .join("\t")
    }
}

fn path_of(tree: &AxTree, i: usize) -> String {
    if i == 0 { ROOT_PATH.to_string() } else { format!("{PATH_PREFIX}{}", tree.nodes[i].node) }
}

/// The text of `i`'s subtree: its value, else its own static text, else its descendants' text runs joined by
/// single spaces (AT-SPI's hypertext without the embedded-object characters).
fn text_of(tree: &AxTree, i: usize) -> Option<String> {
    let n = &tree.nodes[i];
    if let Some(v) = &n.value {
        return Some(v.clone());
    }
    if n.role == role::TEXT {
        return Some(n.name.clone());
    }
    let mut runs = Vec::new();
    let mut stack: Vec<usize> = n.children.iter().rev().copied().collect();
    while let Some(c) = stack.pop() {
        let m = &tree.nodes[c];
        if m.role == role::TEXT {
            runs.push(m.name.as_str());
        } else if let Some(v) = &m.value {
            runs.push(v.as_str());
        } else {
            stack.extend(m.children.iter().rev().copied());
        }
    }
    (!runs.is_empty()).then(|| runs.join(" "))
}

struct Ctx<'a> {
    engine: &'a AetherEngine,
    tree: AxTree,
    dom: HashMap<usize, NodeRef>,
}

impl Ctx<'_> {
    fn new(engine: &AetherEngine) -> Ctx<'_> {
        let tree = super::tree(engine);
        let dom = engine
            .document
            .as_ref()
            .map(|d| d.document_node().inclusive_descendants().map(|n| (n.id().0, n)).collect())
            .unwrap_or_default();
        Ctx { engine, tree, dom }
    }

    fn item(&self, i: usize) -> CacheItem {
        let (tree, e) = (&self.tree, self.engine);
        let n = &tree.nodes[i];
        let dom = self.dom.get(&n.node);
        let password = dom.is_some_and(|d| super::tag(d) == "input" && crate::forms::input_type(d) == "password");
        let extents = if i == 0 {
            Some((0, 0, e.width as i32, e.height as i32))
        } else {
            dom.filter(|d| d.is_element())
                .and_then(|d| e.box_of(d))
                .map(|(x, y, w, h)| ((x as f64 - e.scroll_x).round() as i32, (y as f64 - e.scroll_y).round() as i32, w.round() as i32, h.round() as i32))
        };
        let text = if password { n.value.as_ref().map(|v| "\u{2022}".repeat(v.chars().count())) } else { text_of(tree, i) };
        let mut interfaces = vec![ACCESSIBLE];
        if extents.is_some() {
            interfaces.push(COMPONENT);
        }
        if text.is_some() {
            interfaces.push(TEXT);
        }
        CacheItem {
            path: path_of(tree, i),
            parent: n.parent.map(|p| path_of(tree, p)).unwrap_or_else(|| NULL_PATH.to_string()),
            index_in_parent: n.parent.and_then(|p| tree.nodes[p].children.iter().position(|&c| c == i)).map_or(-1, |x| x as i32),
            child_count: n.children.len() as i32,
            interfaces,
            name: n.name.clone(),
            role: if password { ROLE_PASSWORD_TEXT } else { atspi_role(n.role) },
            description: n.description.clone(),
            states: atspi_states(n, extents.is_some()),
            extents,
            text,
        }
    }

    fn focused(&self) -> usize {
        self.tree.nodes.iter().position(|n| n.states.focused).unwrap_or(0)
    }
}

/// Every object of the engine's page, in document order (empty before the first load).
pub fn items(engine: &AetherEngine) -> Vec<CacheItem> {
    let cx = Ctx::new(engine);
    if cx.tree.nodes.is_empty() {
        return Vec::new();
    }
    cx.tree.walk().into_iter().map(|(i, _)| cx.item(i)).collect()
}

/// The root object.
pub fn root(engine: &AetherEngine) -> Option<CacheItem> {
    let cx = Ctx::new(engine);
    (!cx.tree.nodes.is_empty()).then(|| cx.item(0))
}

/// The focused object (the root when nothing in the page holds focus).
pub fn focus(engine: &AetherEngine) -> Option<CacheItem> {
    let cx = Ctx::new(engine);
    (!cx.tree.nodes.is_empty()).then(|| cx.item(cx.focused()))
}

/// Answers one request line of the shell's socket (the module doc's wire) for application `app`.
pub fn answer(engine: &AetherEngine, request: &str, app: &str) -> String {
    let objects = match request.trim() {
        "GetItems" => items(engine),
        "GetRoot" => root(engine).into_iter().collect(),
        "GetFocus" => focus(engine).into_iter().collect(),
        other => return format!("ERR unknown request \"{}\" (GetItems, GetRoot, GetFocus)\n", esc(other)),
    };
    let mut out = String::new();
    for o in &objects {
        out.push_str(&o.line(app));
        out.push('\n');
    }
    out.push_str(".\n");
    out
}
