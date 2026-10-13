//! Roles: WAI-ARIA 1.2 explicit roles (§5.4 the role attribute, §5.3 categories) and the HTML-AAM 1.0
//! implicit role of every HTML element (§3.4 "HTML Element Role Mappings"), with the presentational-role
//! conflict resolution of WAI-ARIA 1.2 §9.1 and the required-owned-element inheritance of
//! `role="presentation"` (§5.3.3 presentation: "user agents MUST apply an inherited role of presentation to
//! any owned elements that do not have an explicit role").
//!
//! A role is its ARIA spelling (`&'static str`); three engine roles sit beside ARIA's: [`ROOT`] (the
//! document, Chromium's `RootWebArea`), [`TEXT`] (a text run, `StaticText`) and the HTML-AAM elements with
//! no ARIA equivalent, which map to [`GENERIC`].

use super::{attr, tag};
use crate::dom::NodeRef;

/// The document's root node.
pub const ROOT: &str = "RootWebArea";
/// A text run.
pub const TEXT: &str = "StaticText";
pub const GENERIC: &str = "generic";
pub const NONE: &str = "none";

/// WAI-ARIA 1.2's concrete roles — what a `role` attribute token may name (§5.4, the role definitions
/// of §5.4 minus the abstract roles of §5.3.1). Sorted (binary-searched).
pub const ARIA_ROLES: [&str; 82] = [
    "alert", "alertdialog", "application", "article", "banner", "blockquote", "button", "caption", "cell", "checkbox",
    "code", "columnheader", "combobox", "complementary", "contentinfo", "definition", "deletion", "dialog", "directory",
    "document", "emphasis", "feed", "figure", "form", "generic", "grid", "gridcell", "group", "heading", "img",
    "insertion", "link", "list", "listbox", "listitem", "log", "main", "marquee", "math", "menu", "menubar", "menuitem",
    "menuitemcheckbox", "menuitemradio", "meter", "navigation", "none", "note", "option", "paragraph", "presentation",
    "progressbar", "radio", "radiogroup", "region", "row", "rowgroup", "rowheader", "scrollbar", "search", "searchbox",
    "separator", "slider", "spinbutton", "status", "strong", "subscript", "superscript", "switch", "tab", "table",
    "tablist", "tabpanel", "term", "textbox", "time", "timer", "toolbar", "tooltip", "tree", "treegrid", "treeitem",
];

/// The abstract roles (§5.3.1): never valid in a `role` attribute ("Authors MUST NOT use abstract roles").
pub const ABSTRACT_ROLES: [&str; 12] =
    ["command", "composite", "input", "landmark", "range", "roletype", "section", "sectionhead", "select", "structure", "widget", "window"];

/// The landmark roles (§5.3.4).
pub const LANDMARKS: [&str; 8] = ["banner", "complementary", "contentinfo", "form", "main", "navigation", "region", "search"];

/// Roles whose name may come from their content (accname 1.2 §4.3 step 2F; ARIA 1.2 "Name From: contents").
pub const NAME_FROM_CONTENT: [&str; 18] = [
    "button", "cell", "checkbox", "columnheader", "gridcell", "heading", "link", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "radio", "row", "rowheader", "switch", "tab", "tooltip", "treeitem",
];

/// Roles whose naming is prohibited (ARIA 1.2 "Name From: prohibited"): `aria-label` / `aria-labelledby`
/// give them no name.
pub const NAME_PROHIBITED: [&str; 13] = [
    "caption", "code", "deletion", "emphasis", "generic", "insertion", "none", "paragraph", "presentation", "strong",
    "subscript", "superscript", TEXT,
];

/// Roles whose children are presentational (ARIA 1.2 "Children Presentational: True"): the tree folds their
/// descendants into them.
pub const PRESENTATIONAL_CHILDREN: [&str; 14] = [
    "button", "checkbox", "img", "math", "menuitemcheckbox", "menuitemradio", "meter", "progressbar", "radio",
    "scrollbar", "separator", "slider", "switch", "tab",
];

/// The embedded-control roles of accname 1.2 step 2E (their value stands in for them inside another name).
pub const EMBEDDED_CONTROLS: [&str; 8] = ["textbox", "searchbox", "combobox", "listbox", "range", "progressbar", "scrollbar", "slider"];

pub fn is_landmark(role: &str) -> bool {
    LANDMARKS.contains(&role)
}

pub fn allows_name_from_content(role: &str) -> bool {
    NAME_FROM_CONTENT.contains(&role)
}

pub fn name_prohibited(role: &str) -> bool {
    NAME_PROHIBITED.contains(&role)
}

pub fn children_presentational(role: &str) -> bool {
    PRESENTATIONAL_CHILDREN.contains(&role)
}

pub fn is_embedded_control(role: &str) -> bool {
    EMBEDDED_CONTROLS.contains(&role) || role == "spinbutton"
}

/// A presentational role (`none` / `presentation`, synonyms in ARIA 1.2).
pub fn is_presentational(role: &str) -> bool {
    role == NONE || role == "presentation"
}

/// The explicit role: the first token of the `role` attribute that is a concrete WAI-ARIA 1.2 role
/// (§5.4: "user agents MUST use the first token in the sequence of tokens in the role attribute value that
/// matches the name of any non-abstract WAI-ARIA role"), lowercased. `presentation` is answered as `none`.
pub fn explicit_role(n: &NodeRef) -> Option<&'static str> {
    let v = attr(n, "role")?;
    for tok in v.split_ascii_whitespace() {
        let t = tok.to_ascii_lowercase();
        if let Ok(i) = ARIA_ROLES.binary_search(&t.as_str()) {
            let r = ARIA_ROLES[i];
            return Some(if r == "presentation" { NONE } else { r });
        }
    }
    None
}

/// Whether one of `n`'s ancestors is sectioning content or a sectioning root that scopes `header` /
/// `footer` out of banner / contentinfo (HTML-AAM: "not a descendant of an article, aside, main, nav or
/// section element, or an element with role=article, complementary, main, navigation or region").
fn scoped_by_section(n: &NodeRef) -> bool {
    n.ancestors().any(|a| {
        matches!(tag(&a).as_str(), "article" | "aside" | "main" | "nav" | "section")
            || matches!(explicit_role(&a), Some("article" | "complementary" | "main" | "navigation" | "region"))
    })
}

/// The `<input>` type keyword (forms' own parse: unknown and missing are `text`).
fn input_type(n: &NodeRef) -> String {
    crate::forms::input_type(n)
}

/// HTML-AAM 1.0 §3.4: the implicit role of an HTML element. `named` answers whether the element has an
/// accessible name (it decides `section` → region and is computed lazily by the caller). The empty `alt`
/// image is `none` (HTML-AAM: `img` with `alt=""` → presentation).
pub fn implicit_role(n: &NodeRef, named: &dyn Fn() -> bool) -> &'static str {
    let t = tag(n);
    match t.as_str() {
        "a" | "area" => {
            if attr(n, "href").is_some() {
                "link"
            } else {
                GENERIC
            }
        }
        "article" => "article",
        "aside" => "complementary",
        "blockquote" => "blockquote",
        "button" => "button",
        "caption" => "caption",
        "code" => "code",
        "datalist" => "listbox",
        "dd" => "definition",
        "del" | "s" => "deletion",
        "details" => "group",
        "dfn" | "dt" => "term",
        "dialog" => "dialog",
        "em" => "emphasis",
        "fieldset" | "optgroup" | "address" => "group",
        "figure" => "figure",
        "footer" => {
            if scoped_by_section(n) {
                GENERIC
            } else {
                "contentinfo"
            }
        }
        "form" => "form",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "header" => {
            if scoped_by_section(n) {
                GENERIC
            } else {
                "banner"
            }
        }
        "hgroup" => "group",
        "hr" => "separator",
        "img" => {
            if attr(n, "alt").is_some_and(|a| a.is_empty()) && attr(n, "title").is_none() && attr(n, "aria-label").is_none() {
                NONE
            } else {
                "img"
            }
        }
        "input" => match input_type(n).as_str() {
            "button" | "image" | "reset" | "submit" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "number" => "spinbutton",
            "search" => {
                if attr(n, "list").is_some() {
                    "combobox"
                } else {
                    "searchbox"
                }
            }
            // Chromium's mappings where HTML-AAM has "no corresponding role": a colour well and a file
            // chooser are buttons, the date/time inputs are text fields.
            "color" | "file" => "button",
            "hidden" => NONE,
            _ => {
                if attr(n, "list").is_some() {
                    "combobox"
                } else {
                    "textbox"
                }
            }
        },
        "ins" => "insertion",
        "li" => {
            // A listitem only inside a list (HTML-AAM: "li … parent is ol, ul or menu").
            match n.parent() {
                Some(p) if matches!(tag(&p).as_str(), "ul" | "ol" | "menu") || explicit_role(&p) == Some("list") => "listitem",
                _ => GENERIC,
            }
        }
        "main" => "main",
        "math" => "math",
        "menu" | "ol" | "ul" => "list",
        "meter" => "meter",
        "nav" => "navigation",
        "option" => "option",
        "output" => "status",
        "p" => "paragraph",
        "progress" => "progressbar",
        "search" => "search",
        "section" => {
            if named() {
                "region"
            } else {
                GENERIC
            }
        }
        "select" => {
            let size = attr(n, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
            if attr(n, "multiple").is_some() || size > 1 {
                "listbox"
            } else {
                "combobox"
            }
        }
        "strong" => "strong",
        "sub" => "subscript",
        "sup" => "superscript",
        "svg" => {
            if named() {
                "img"
            } else {
                GENERIC
            }
        }
        "table" => "table",
        "tbody" | "thead" | "tfoot" => "rowgroup",
        "td" => "cell",
        "textarea" => "textbox",
        "th" => {
            let scope = attr(n, "scope").map(|s| s.to_ascii_lowercase());
            if matches!(scope.as_deref(), Some("row" | "rowgroup")) {
                "rowheader"
            } else {
                "columnheader"
            }
        }
        "time" => "time",
        "tr" => "row",
        _ => GENERIC,
    }
}

/// HTML-AAM: elements that need an owning context, and the contexts — for the inherited-presentation rule.
fn required_owner(child: &str) -> &'static [&'static str] {
    match child {
        "li" => &["ul", "ol", "menu"],
        "tr" | "thead" | "tbody" | "tfoot" | "caption" => &["table", "thead", "tbody", "tfoot"],
        "td" | "th" => &["tr"],
        "dt" | "dd" => &["dl", "div"],
        _ => &[],
    }
}

/// Global ARIA states and properties (§6.4) — any of them on a presentational element overrides the
/// presentation (§9.1 conflict resolution).
const GLOBAL_PROPS: [&str; 10] = [
    "aria-label", "aria-labelledby", "aria-describedby", "aria-description", "aria-live", "aria-owns", "aria-controls",
    "aria-details", "aria-keyshortcuts", "aria-roledescription",
];

/// The computed role of element `n`: the explicit role when valid, else the implicit one; a presentational
/// role is ignored (the implicit role used) when the element is focusable or carries a global ARIA
/// property, and inherited by required owned elements without an explicit role. `focusable` and `named`
/// are the caller's (they need the rendering and the name computation).
pub fn computed_role(n: &NodeRef, focusable: bool, named: &dyn Fn() -> bool) -> &'static str {
    let implicit = || implicit_role(n, named);
    match explicit_role(n) {
        Some(r) if is_presentational(r) => {
            if focusable || GLOBAL_PROPS.iter().any(|p| attr(n, p).is_some()) {
                implicit()
            } else {
                NONE
            }
        }
        Some(r) => r,
        None => {
            let t = tag(n);
            let owners = required_owner(&t);
            if !owners.is_empty() {
                // Walk up through the owning chain (td → tr → tbody → table) while it is HTML structure.
                let mut cur = n.parent();
                while let Some(p) = cur {
                    let pt = tag(&p);
                    if !owners.contains(&pt.as_str()) && required_owner(&pt).is_empty() {
                        break;
                    }
                    if explicit_role(&p).is_some_and(is_presentational) {
                        return NONE;
                    }
                    if matches!(pt.as_str(), "table" | "ul" | "ol" | "menu" | "dl") {
                        break;
                    }
                    cur = p.parent();
                }
            }
            implicit()
        }
    }
}

/// A heading's level: `aria-level` (a positive integer) first, else `<hN>`'s N, else 2 for an ARIA heading
/// (ARIA 1.2 heading: "aria-level … default value 2").
pub fn heading_level(n: &NodeRef) -> Option<u32> {
    if let Some(l) = attr(n, "aria-level").and_then(|v| v.trim().parse::<u32>().ok()).filter(|l| *l > 0) {
        return Some(l);
    }
    let t = tag(n);
    match t.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some((d - b'0') as u32),
        _ => Some(2),
    }
}
