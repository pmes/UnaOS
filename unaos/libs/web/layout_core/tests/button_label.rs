//! AETHERBUTTONLABELONE (SR423): a push button's caption is ONE shared-core rule, `layout_core::button_label`
//! (HTML §4.10.5.1.18 / .20 / .21: the value attribute, else "Submit" / "Reset", and NOTHING for a valueless
//! type=button), which the intrinsic size (`control_intrinsic`) and the host paint (`forms::button_label`) both
//! read. The laid box of each push type, with and without a value, is as wide as the label the rule names.
mod common;
use common::*;
use layout_core::button_label;

#[test]
fn button_label_rule() {
    assert_eq!(button_label("submit", None), "Submit");
    assert_eq!(button_label("reset", None), "Reset");
    assert_eq!(button_label("button", None), "", "a valueless type=button has an empty label");
    assert_eq!(button_label(" RESET ", None), "Reset", "the type is trimmed and case-insensitive");
    assert_eq!(button_label("Submit", None), "Submit");
    for ty in ["submit", "reset", "button"] {
        assert_eq!(button_label(ty, Some("")), "", "{ty}: an empty value is an empty caption");
        assert_eq!(button_label(ty, Some("Go")), "Go", "{ty}: the value wins");
    }
}

#[test]
fn button_intrinsic_width_matches_label() {
    // Each input alone in a flex row, so its border box is its intrinsic size.
    let row = |id: &'static str, attrs: &[(&'static str, &str)]| {
        let mut a: Vec<(&'static str, &str)> = vec![("id", id)];
        a.extend_from_slice(attrs);
        el("div", &[("style", "display: flex")], vec![el("input", &a, vec![])])
    };
    let doc = page(vec![
        row("s0", &[("type", "submit")]),
        row("s1", &[("type", "submit"), ("value", "Submit")]),
        row("r0", &[("type", "reset")]),
        row("r1", &[("type", "reset"), ("value", "Reset")]),
        row("b0", &[("type", "button")]),
        row("b1", &[("type", "button"), ("value", "")]),
        row("bg", &[("type", "button"), ("value", "Go")]),
        row("sg", &[("type", "submit"), ("value", "Go")]),
    ]);
    let tree = lay(&doc, 400.0, 600.0);
    let w = |id: &str| box_of(&tree, &doc.by_id(id)).2;
    assert_eq!(w("s0"), w("s1"), "a valueless submit is as wide as its Submit label");
    assert_eq!(w("r0"), w("r1"), "a valueless reset is as wide as its Reset label");
    assert_eq!(w("b0"), w("b1"), "a valueless type=button is as wide as an empty label");
    assert!(w("b0") < w("s0"), "an empty button is narrower than Submit: {} vs {}", w("b0"), w("s0"));
    assert!(w("b0") > 0.0, "the empty button keeps its padding and border: {}", w("b0"));
    assert_eq!(w("bg"), w("sg"), "a valued button measures its value whatever its type");
}
