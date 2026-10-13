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
