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

// ---- AETHERCONTROLINTRINSIC (SR448): image / file / color intrinsics ------------------------------------------

/// Each input alone in a flex row, so its border box is its intrinsic size.
fn lone(id: &'static str, attrs: &[(&'static str, &'static str)]) -> Spec {
    let mut a: Vec<(&'static str, &str)> = vec![("id", id)];
    a.extend_from_slice(attrs);
    el("div", &[("style", "display: flex")], vec![el("input", &a, vec![])])
}

#[test]
fn file_intrinsic_width() {
    let doc = page(vec![
        lone("f", &[("type", "file")]),
        lone("fm", &[("type", "file"), ("multiple", "")]),
        lone("f2", &[("type", "file"), ("style", "font-size: 26.666px")]),
        lone("s", &[("type", "submit")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let b = |id: &str| box_of(&tree, &doc.by_id(id));
    // The test font: a character is half the size wide. Chromium's floor, 34 x "0", beats
    // "Choose File" (button edges 16) + 4 + "No file chosen" at the UA size: 226.66 vs 186.66.
    let floor = 34.0 * 13.333 * 0.5;
    // box_of reports the laid box on whole pixels.
    assert_eq!(b("f").2, f32::round(floor), "file border box is 34 digits wide: {}", b("f").2);
    assert_eq!(b("fm").2, b("f").2, "multiple keeps the floor while it dominates");
    assert_eq!(b("f2").2, f32::round(34.0 * 26.666 * 0.5), "the floor is in the element's font: {}", b("f2").2);
    assert_eq!(b("f").3, b("s").3, "a file input is one button line tall");
    assert_eq!(layout_core::file_label(false), ("Choose File", "No file chosen"));
    assert_eq!(layout_core::file_label(true), ("Choose Files", "No file chosen"));
}

#[test]
fn color_intrinsic_fixed() {
    let doc = page(vec![
        lone("c", &[("type", "color")]),
        lone("c2", &[("type", "color"), ("style", "font-size: 30px")]),
        // AETHERCOLORUAEDGES (SR458): html.css's color edges (border 1px, padding 1px 2px), not the push button's.
        lone("c0", &[("type", "color"), ("style", "padding: 0; border: 0")]),
        lone("cu", &[("type", " COLOR ")]),
        lone("cx", &[("type", "color"), ("style", "padding: 1px 6px; border: 2px solid")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let b = |id: &str| box_of(&tree, &doc.by_id(id));
    assert_eq!((b("c").2, b("c").3), (50.0, 27.0), "the UA swatch is 50 x 27");
    assert_eq!((b("c2").2, b("c2").3), (50.0, 27.0), "the swatch does not follow the font");
    assert_eq!((b("c0").2, b("c0").3), (44.0, 23.0), "the content is html.css's 44 x 23 (edges 6 x 4)");
    assert_eq!((b("cu").2, b("cu").3), (50.0, 27.0), "the type is trimmed and case-insensitive");
    assert_eq!((b("cx").2, b("cx").3), (60.0, 29.0), "author padding and border still apply");
}

#[test]
fn image_intrinsic_alt() {
    let doc = page(vec![
        lone("ia", &[("type", "image"), ("alt", "Search")]),
        lone("sa", &[("type", "submit"), ("value", "Search")]),
        lone("i0", &[("type", "image")]),
        lone("s0", &[("type", "submit")]),
        lone("iv", &[("type", "image"), ("value", "Go")]),
        lone("sv", &[("type", "submit"), ("value", "Go")]),
        lone("ie", &[("type", "image"), ("alt", "")]),
        lone("iev", &[("type", "image"), ("alt", ""), ("value", "Go")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let b = |id: &str| box_of(&tree, &doc.by_id(id));
    // The test host has no decoded image, so every image input is its alt-text label. AETHERIMAGEINPUTBOX (SR455):
    // an image input has no UA edges, so it is the same-labelled submit less its 16 x 6 (padding 1px 6px, border 2px).
    assert_eq!(b("ia"), (b("ia").0, b("ia").1, b("sa").2 - 16.0, b("sa").3 - 6.0), "alt labels the image input");
    assert_eq!(b("i0").2, b("s0").2 - 16.0, "no alt and no value: Submit");
    assert_eq!(b("iv").2, b("sv").2 - 16.0, "a missing alt falls to the value");
    assert_eq!(b("ie").2, b("s0").2 - 16.0, "an empty alt and no value: Submit");
    // Chromium's AltText: the value stands in only for a MISSING alt; an empty alt is Submit.
    assert_eq!(b("iev").2, b("s0").2 - 16.0, "an empty alt with a value: Submit");
}

// ---- AETHERIMAGEINPUTBOX (SR455): an image input has no UA edges (html.css) ------------------------------------

#[test]
fn image_box_has_no_edges() {
    let doc = page(vec![
        lone("ia", &[("type", "image"), ("alt", "Search")]),
        lone("iu", &[("type", " IMAGE "), ("alt", "Search")]),
        lone("sa", &[("type", "submit"), ("value", "Search")]),
        lone("ix", &[("type", "image"), ("alt", "Search"), ("style", "padding: 1px 6px; border: 2px solid")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let b = |id: &str| box_of(&tree, &doc.by_id(id));
    // The test font: a character is half the size wide, so "Search" is 6 x 13.333 x 0.5 = 40 px of content.
    assert_eq!(b("ia").2, f32::round(6.0 * 13.333 * 0.5), "the image input is its content, no edges: {}", b("ia").2);
    assert_eq!((b("ia").2, b("ia").3), (b("sa").2 - 16.0, b("sa").3 - 6.0), "the submit's 16 x 6 UA edges are gone");
    assert_eq!((b("iu").2, b("iu").3), (b("ia").2, b("ia").3), "the type is trimmed and case-insensitive");
    assert_eq!((b("ix").2, b("ix").3), (b("sa").2, b("sa").3), "author padding and border still apply");
    assert!(layout_core::is_image_type(" Image ") && !layout_core::is_image_type("submit"));
}
