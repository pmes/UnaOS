//! AETHERSTYLE3C (SR122): what SR115's vertical text refused. In a `vertical-rl` paragraph (20px font, 10px
//! glyphs in the harness) the `text-combine-upright: all` run "123" between 年 and 月 is one upright cell 1em
//! (20px) long, its glyphs scaled by 2/3 to fit, so 月 follows at 30px; under `text-orientation: upright` the
//! Latin "ab" takes a 1em vertical advance per glyph (each centred in its 20px cell), so the column is 40px
//! long instead of 20px; a caret in vertical text is a horizontal bar across the column (`writing::caret_rect`
//! per mode, and `hit::caret_rect` through a text box of the laid-out paragraph).
mod common;
use common::*;
use layout_core::inline::Frag;
use layout_core::writing::{self, UprightCell};

#[test]
fn combined_runs_upright_advance_and_vertical_caret() {
    let st = |extra: &str| format!("writing-mode: vertical-rl; font-size: 20px; line-height: 20px; {extra}");
    let (s1, s2) = (st("height: 100px"), st("text-orientation: upright"));
    let doc = page(vec![
        el("div", &[("id", "tcu"), ("style", s1.as_str())], vec![text("年"), el("span", &[("style", "text-combine-upright: all")], vec![text("123")]), text("月")]),
        el("div", &[("id", "up"), ("style", s2.as_str())], vec![text("ab")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);

    // text-combine-upright: one 1em cell, glyphs compressed into it
    let tcu = doc.by_id("tcu");
    let il = &tree.inline[&id_of(&tree, &tcu)];
    let texts: Vec<(String, f32, f32)> = il
        .frags
        .iter()
        .filter_map(|f| match f {
            Frag::Text { text, x, width, .. } => Some((text.clone(), *x, *width)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec![("年".to_string(), 0.0, 10.0), ("123".to_string(), 10.0, 20.0), ("月".to_string(), 30.0, 10.0)]);
    let cell = UprightCell { x: 10.0, w: 20.0, top: 0.0, h: 20.0 };
    assert_eq!(il.combined.len(), 1);
    assert_eq!(il.combined[0].cell, cell);
    assert!((il.combined[0].scale - 2.0 / 3.0).abs() < 1e-4, "30px of glyphs into a 20px cell");
    assert_eq!(il.upright, vec![UprightCell { x: 0.0, w: 10.0, top: 0.0, h: 20.0 }, cell, UprightCell { x: 30.0, w: 10.0, top: 0.0, h: 20.0 }]);
    // the combined cell is translated, not turned: its horizontal axis stays horizontal in the column
    assert_eq!(writing::to_physical_upright(writing::VERTICAL_RL, 12, 5, 20, &il.upright), Some((2, 15)));

    // forced-upright Latin: a 1em advance per glyph, centred in its cell
    let up = doc.by_id("up");
    let il = &tree.inline[&id_of(&tree, &up)];
    assert_eq!(il.upright, vec![UprightCell { x: 0.0, w: 20.0, top: 0.0, h: 20.0 }, UprightCell { x: 20.0, w: 20.0, top: 0.0, h: 20.0 }]);
    let xs: Vec<f32> = il.frags.iter().filter_map(|f| if let Frag::Text { x, .. } = f { Some(*x) } else { None }).collect();
    assert_eq!(xs, vec![5.0, 25.0], "each 10px glyph centred in its 20px cell");
    assert_eq!(box_of(&tree, &up).3, 40.0, "two 1em advances, not two 10px widths");

    // the caret in vertical text
    assert_eq!(writing::caret_rect(writing::HORIZONTAL_TB, 10.0, 0.0, 20.0, 20.0, 100.0), (10.0, 0.0, 1.0, 20.0));
    assert_eq!(writing::caret_rect(writing::VERTICAL_RL, 10.0, 0.0, 20.0, 40.0, 100.0), (20.0, 10.0, 20.0, 1.0));
    assert_eq!(writing::caret_rect(writing::VERTICAL_LR, 10.0, 20.0, 20.0, 40.0, 100.0), (20.0, 10.0, 20.0, 1.0));
    assert_eq!(writing::caret_rect(writing::SIDEWAYS_LR, 10.0, 0.0, 20.0, 40.0, 100.0), (0.0, 90.0, 20.0, 1.0));
    let (x0, y0, _, _) = box_of(&tree, &tcu);
    let boxes = layout_core::hit::text_boxes(&tree);
    let tcu_id = id_of(&tree, &tcu);
    let b = boxes.iter().find(|b| b.ifc == tcu_id && b.frag == 0).expect("the first fragment's box");
    let il = &tree.inline[&tcu_id];
    assert_eq!(layout_core::hit::caret_rect(il, b, 10.0), (x0, y0 + 10.0, 20.0, 1.0), "after 年: across the column, 10px down");
}
