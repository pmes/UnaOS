//! AETHERSIDETABS (SR140, R118): the many-tabs model — move (the drag), groups (contiguous, collapsible, the top
//! strip's segments), Ctrl+Shift+A's search, the layout preference and the side column's geometry.

use aether::chrome::sidetabs::{is_tab_search_key, SideHit, SideStrip, TabLayout, TabSearch, RAIL_W, ROW_H, SIDE_W};
use aether::chrome::tabs::{GroupColor, Row, TabStrip};

/// A strip of `n` tabs titled `t1..tn` with URLs `https://site<k>.test/`.
fn strip(n: usize) -> (TabStrip, Vec<u64>) {
    let mut s = TabStrip::new();
    let ids: Vec<u64> = (0..n).map(|_| s.open()).collect();
    for (k, id) in ids.iter().enumerate() {
        let title = format!("t{}", k + 1);
        let url = format!("https://site{}.test/", k + 1);
        s.update(*id, Some(&title), Some(&url), None);
    }
    (s, ids)
}

fn order(s: &TabStrip) -> Vec<u64> {
    s.tabs().iter().map(|t| t.id).collect()
}

#[test]
fn move_to_reorders_and_keeps_ids() {
    let (mut s, ids) = strip(4);
    assert!(s.move_to(ids[3], 0));
    assert_eq!(order(&s), vec![ids[3], ids[0], ids[1], ids[2]]);
    assert!(s.move_to(ids[3], 99), "clamped to the end");
    assert_eq!(order(&s), ids);
    assert!(!s.move_to(999, 0), "an unknown id misses");
    assert_eq!(s.active(), ids[3], "a move never changes the active tab");
}

#[test]
fn groups_are_contiguous_and_vanish_when_empty() {
    let (mut s, ids) = strip(5);
    let g = s.group_new("work", GroupColor::Blue, &[ids[1], ids[3]]).unwrap();
    assert_eq!(order(&s), vec![ids[0], ids[1], ids[3], ids[2], ids[4]], "the members gather where the first stands");
    assert_eq!(s.group_tabs(g), vec![ids[1], ids[3]]);
    assert_eq!(s.top_segments(), vec![(g, 1, 2)]);
    assert!(s.group_add(ids[4], g));
    assert_eq!(s.group_tabs(g), vec![ids[1], ids[3], ids[4]], "an added tab joins the end of the run");
    assert_eq!(s.top_segments(), vec![(g, 1, 3)]);
    // Dropped between two members, a tab joins; dragged clear, it leaves.
    assert!(s.move_to(ids[0], 2));
    assert_eq!(s.get(ids[0]).unwrap().group, Some(g));
    assert!(s.move_to(ids[0], 0));
    assert_eq!(s.get(ids[0]).unwrap().group, Some(g), "still touching its group's run: it stays in");
    assert!(s.move_to(ids[0], 99));
    assert_eq!(s.get(ids[0]).unwrap().group, None, "clear of its group: it leaves");
    assert!(s.ungroup(ids[1]));
    assert_eq!(s.top_segments().len(), 1, "the run stays whole");
    assert!(s.group_rename(g, "play") && s.group_recolor(g, GroupColor::Green));
    assert_eq!(s.group(g).unwrap().name, "play");
    assert_eq!(GroupColor::parse("GREEN"), Some(GroupColor::Green));
    for id in s.group_tabs(g) {
        s.close(id);
    }
    assert!(s.group(g).is_none(), "a group with no tabs is gone");
    let g2 = s.group_new("x", GroupColor::Red, &[ids[0]]).unwrap();
    assert!(g2 > g, "group ids are never reused");
    assert!(s.group_dissolve(g2) && s.groups().is_empty());
}

#[test]
fn a_collapsed_group_hides_its_tabs_but_never_the_active_one() {
    let (mut s, ids) = strip(4);
    let g = s.group_new("g", GroupColor::Purple, &[ids[1], ids[2]]).unwrap();
    assert_eq!(s.rows(), vec![Row::Tab(ids[0]), Row::Group(g), Row::Tab(ids[1]), Row::Tab(ids[2]), Row::Tab(ids[3]), Row::NewTab]);
    s.select(ids[1]);
    assert_eq!(s.group_toggle(g), Some(true));
    assert_eq!(s.active(), ids[3], "collapsing the active tab's group moves the active tab out of it");
    assert_eq!(s.rows(), vec![Row::Tab(ids[0]), Row::Group(g), Row::Tab(ids[3]), Row::NewTab]);
    assert!(s.select(ids[2]));
    assert!(!s.group(g).unwrap().collapsed, "selecting a hidden tab expands its group");
}

#[test]
fn search_matches_title_or_url_case_insensitively() {
    let (mut s, ids) = strip(12);
    s.update(ids[6], Some("Rust Book"), None, None);
    assert_eq!(s.search("rust"), vec![ids[6]]);
    assert_eq!(s.search("SITE1"), vec![ids[0], ids[9], ids[10], ids[11]], "site1, site10, site11, site12");
    assert_eq!(s.search("").len(), 12, "an empty query lists every tab");
    let mut q = TabSearch::default();
    q.set_query("site1");
    q.step(&s, -1);
    assert_eq!(q.accept(&mut s), Some(ids[11]), "the highlight wraps");
    assert_eq!(s.active(), ids[11]);
    assert!(is_tab_search_key("A", true, true, false));
    assert!(!is_tab_search_key("a", true, false, false), "Ctrl+A stays select-all");
}

#[test]
fn the_layout_pref_reads_top_by_default() {
    assert_eq!(TabLayout::default(), TabLayout::Top);
    assert_eq!(TabLayout::parse("side"), TabLayout::Side);
    assert_eq!(TabLayout::parse(" Side "), TabLayout::Side);
    assert_eq!(TabLayout::parse("left"), TabLayout::Top);
    assert_eq!(TabLayout::Side.as_str(), "side");
    assert_eq!(aether::chrome::sidetabs::PREF_KEY, "tabs.layout");
}

#[test]
fn the_side_column_lays_out_scrolls_and_hits() {
    let (s, ids) = strip(40);
    let mut side = SideStrip::default();
    assert_eq!(side.width(), SIDE_W);
    let h = 10 * ROW_H; // the toggle row + 9 rows on view
    let rows = side.layout(&s, h);
    assert_eq!(rows.len(), 9);
    assert_eq!(rows[0].row, Row::Tab(ids[0]));
    assert_eq!(rows[0].y, ROW_H as i32);
    let y = ROW_H as i32 + 5;
    assert_eq!(side.hit(&s, h, 10, 2), Some(SideHit::Toggle));
    assert_eq!(side.hit(&s, h, 10, y), Some(SideHit::Tab(ids[0])));
    assert_eq!(side.hit(&s, h, SIDE_W as i32 - 5, y), Some(SideHit::Close(ids[0])));
    // 41 rows (40 tabs + `+`) in 9 rows of room.
    assert_eq!(SideStrip::max_scroll(&s, h), (41 - 9) * ROW_H as i32);
    side.scroll_by(&s, h, 1_000_000);
    assert_eq!(side.scroll, SideStrip::max_scroll(&s, h));
    assert_eq!(side.layout(&s, h).last().unwrap().row, Row::NewTab);
    side.reveal(&s, h, ids[0]);
    assert_eq!(side.scroll, 0);
    side.reveal(&s, h, ids[20]);
    assert!(side.layout(&s, h).iter().any(|r| r.row == Row::Tab(ids[20])));
    assert!(side.toggle());
    assert_eq!(side.width(), RAIL_W);
    let y20 = side.layout(&s, h).iter().find(|r| r.row == Row::Tab(ids[20])).unwrap().y + 2;
    assert_eq!(side.hit(&s, h, RAIL_W as i32 - 2, y20), Some(SideHit::Tab(ids[20])), "the rail has no close box");
    assert_eq!(side.hit(&s, h, RAIL_W as i32 + 1, y20), None);
}

#[test]
fn the_letter_tile_reads_the_host_or_the_title() {
    let mut s = TabStrip::new();
    let a = s.open();
    assert_eq!(s.get(a).unwrap().initial(), '+');
    s.update(a, None, Some("https://www.example.org/x"), None);
    assert_eq!(s.get(a).unwrap().initial(), 'E');
    s.update(a, Some("  über alles"), None, None);
    assert_eq!(s.get(a).unwrap().initial(), 'Ü');
}

// ---- AETHERSIDETABS2 (SR143): the wire, the favicon tile, the narrow column ------------------------------------------

use aether::chrome::{apply_tab_verb, tabs_message};
use bandy::SMessage;

/// Through the wire as the bus carries it: JSON out and back.
fn wire(m: &SMessage) -> SMessage {
    serde_json::from_str(&serde_json::to_string(m).unwrap()).unwrap()
}

#[test]
fn the_group_and_move_wire_round_trips_through_the_model() {
    let (mut s, ids) = strip(4);
    // The face's verbs, each through the wire, applied by the model.
    let verb = |s: &mut TabStrip, m: SMessage| apply_tab_verb(s, &wire(&m));
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupNew { tabs: vec![ids[1], ids[2]], name: "work".into(), color: None }), Some(true));
    let g = s.groups()[0].id;
    assert_eq!(s.group(g).unwrap().color, GroupColor::Grey, "no colour named: the first one no group uses");
    assert_eq!(verb(&mut s, SMessage::BrowserTabMove { id: ids[3], to: 0 }), Some(true));
    assert_eq!(order(&s), vec![ids[3], ids[0], ids[1], ids[2]]);
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupAdd { id: ids[0], group: g }), Some(true));
    assert_eq!(s.group_tabs(g), vec![ids[1], ids[2], ids[0]]);
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupRecolor { group: g, color: None }), Some(true));
    assert_eq!(s.group(g).unwrap().color, GroupColor::Blue, "no colour named: the next one");
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupRename { group: g, name: "play".into() }), Some(true));
    assert_eq!(verb(&mut s, SMessage::BrowserTabUngroup(ids[0])), Some(true));
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupToggle(g)), Some(true));
    assert!(s.group(g).unwrap().collapsed);
    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupAdd { id: ids[0], group: 999 }), Some(false), "an unknown group misses");
    assert_eq!(verb(&mut s, SMessage::BrowserTabSelect(ids[0])), None, "not a strip verb: the shell routes it");

    // The strip out on the wire: rows in order with URL, group and letter tile; the groups with their colour.
    let SMessage::BrowserTabs { active, tabs, groups } = wire(&tabs_message(&s)) else { panic!("not BrowserTabs") };
    assert_eq!(active, s.active());
    assert_eq!(tabs.iter().map(|t| t.id).collect::<Vec<_>>(), order(&s));
    let row = tabs.iter().find(|t| t.id == ids[1]).unwrap();
    assert_eq!((row.label.as_str(), row.url.as_str(), row.group, row.initial.as_str()), ("t2", "https://site2.test/", Some(g), "T"));
    assert_eq!(tabs.iter().find(|t| t.id == ids[0]).unwrap().group, None);
    assert_eq!(groups.len(), 1);
    assert_eq!((groups[0].name.as_str(), groups[0].color.as_str(), groups[0].rgb, groups[0].collapsed), ("play", "blue", GroupColor::Blue.rgb(), true));

    assert_eq!(verb(&mut s, SMessage::BrowserTabGroupDissolve(g)), Some(true));
    assert!(s.groups().is_empty());
}

#[test]
fn the_favicon_decodes_to_the_strip_tile() {
    use aether::chrome::sidetabs::{favicon_tile, FAVICON_PX};
    // A 32 px PNG icon, left half red, right half blue — through the engine's own decoder (pixel_core).
    let icon = image::RgbaImage::from_fn(32, 32, |x, _| if x < 16 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) });
    let mut png = Vec::new();
    icon.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let decoded = aether::images::decode_raster(&png).expect("the PNG decodes");
    let (w, h) = decoded.dimensions();
    let tile = favicon_tile(w, h, decoded.as_raw()).expect("a tile");
    let n = FAVICON_PX as usize;
    assert_eq!(tile.len(), n * n * 4);
    let px = |x: usize, y: usize| &tile[(y * n + x) * 4..(y * n + x) * 4 + 4];
    assert_eq!(px(2, 8), &[255, 0, 0, 255]);
    assert_eq!(px(13, 8), &[0, 0, 255, 255]);
    // A wide icon is fitted, centred on transparency; a short buffer is refused.
    let wide = vec![255u8; 32 * 16 * 4];
    let t = favicon_tile(32, 16, &wide).unwrap();
    assert_eq!(t[3], 0, "the band above a wide icon is transparent");
    assert_eq!(t[(8 * n + 8) * 4 + 3], 255);
    assert!(favicon_tile(4, 4, &[0u8; 8]).is_none());
}

#[test]
fn the_side_column_takes_a_third_of_a_narrow_window() {
    let (s, ids) = strip(3);
    let mut side = SideStrip::default();
    assert_eq!(side.width_for(1280), SIDE_W);
    assert_eq!(side.width_for(288), 96, "the UnaOS face's 288 px surface");
    assert_eq!(side.width_for(90), RAIL_W, "never narrower than the rail");
    let h = 8 * ROW_H;
    let y = ROW_H as i32 + 5;
    assert_eq!(side.hit_w(&s, 96, h, 90, y), Some(SideHit::Close(ids[0])), "the close box at the narrow column's edge");
    assert_eq!(side.hit_w(&s, 96, h, 97, y), None);
    side.toggle();
    assert_eq!(side.width_for(1280), RAIL_W);
}
