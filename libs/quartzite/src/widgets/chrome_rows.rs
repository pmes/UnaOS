//! CHARTER: Aether (CODEX §2 Web) — the browser chrome face's pure rows and text, read from bandy's `Browser*` wire.
//!
//! AETHERMACCHROME (SR205): the shell's `Coordinator` owns the one tab model (`aether::chrome::tabs::TabStrip`) and
//! publishes it as `SMessage::BrowserTabs` (`aether::chrome::tabs_message`); a face only paints that wire and fires
//! verbs back (R104). This module is what a face derives from the wire, written ONCE and platform-neutral (std +
//! bandy, no toolkit), so the Linux host tests it and the AppKit face (`platforms/macos/{tab_strip, find_bar,
//! status_line, downloads}.rs`) reads it: the side column's rows (the model's `TabStrip::rows` walk over the wire),
//! the verb a row's click fires, the label clip, the find count, the status line and a download row's text and
//! fraction. Quartzite does not link `aether`; the side column's widths are mirrored here and a host KAT in
//! aether-shell holds them equal to `aether::chrome::sidetabs`.
//!
//! ONETEXT (SR210): the GTK face reads it too (`platforms/gtk/tab_strip.rs` `fill_side` walks [`side_rows`] and
//! fires [`row_verb`], with [`clip`], [`SIDE_W`], [`RAIL_W`] and [`SIDE_LABEL_MAX`]; `platforms/gtk/image_view.rs`
//! `bootstrap_page_view` takes [`find_msg`], [`find_count`], [`download_text`] and [`download_fraction`]): these
//! widths are the one face copy, and no face keeps a second walk or a second text.
//!
//! STRIP2 (SR219): the strips' remaining shared parts live here too, read by both faces (`platforms/gtk/tab_strip.rs`
//! and `platforms/macos/{tab_strip, tab_search, mod}.rs`): the top strip's walk ([`top_rows`], `side_rows` without
//! `+`), the favicon tile rule ([`tile_face`], [`favicon_rgba`], [`FAVICON_PX`]), the strip as last sent
//! ([`TabWire`]: its tab search and where a dragged tab lands, [`TabWire::drop_verb`]), the drag payload
//! ([`drag_payload`] / [`drag_id`]), the right-click rows ([`tab_menu`], [`group_menu`]), Principia's layout row
//! ([`stored_layout`], `aether.tabs.layout`) and the chrome's keys ([`key_verbs`], each the bandy verb it fires).

use bandy::{BrowserTabGroupRow, BrowserTabRow, PrefValue, PrincipiaCommand, SMessage};

/// The side column's width open and as the icon rail, and one row's base height (`aether::chrome::sidetabs::{SIDE_W,
/// RAIL_W, ROW_H}`, mirrored: quartzite does not link the handler).
pub const SIDE_W: u32 = 240;
pub const RAIL_W: u32 = 48;
pub const ROW_H: u32 = 32;
/// The longest title a side row shows, in characters (both faces' side column).
pub const SIDE_LABEL_MAX: usize = 26;

/// One row of the side column, in the order the model's `TabStrip::rows` gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideRow {
    /// A group's header: a click collapses or expands it.
    Group { id: u64, name: String, count: usize, collapsed: bool },
    /// A tab (hidden while its group is collapsed).
    Tab { id: u64, label: String, active: bool, grouped: bool },
    /// The `+` row, last.
    New,
}

/// The side column's rows from the wire: a header where the group run starts, the tabs of an open group or of no
/// group, and `+` last (`TabStrip::rows` over `BrowserTabs`' rows).
pub fn side_rows(active: u64, tabs: &[BrowserTabRow], groups: &[BrowserTabGroupRow]) -> Vec<SideRow> {
    let group = |id: u64| groups.iter().find(|g| g.id == id);
    let mut out = Vec::with_capacity(tabs.len() + groups.len() + 1);
    let mut cur: Option<u64> = None;
    for t in tabs {
        if t.group != cur {
            cur = t.group;
            if let Some(g) = cur.and_then(group) {
                let count = tabs.iter().filter(|x| x.group == Some(g.id)).count();
                out.push(SideRow::Group { id: g.id, name: g.name.clone(), count, collapsed: g.collapsed });
            }
        }
        let hidden = t.group.and_then(group).is_some_and(|g| g.collapsed);
        if !hidden {
            out.push(SideRow::Tab { id: t.id, label: t.label.clone(), active: t.id == active, grouped: t.group.is_some() });
        }
    }
    out.push(SideRow::New);
    out
}

/// The verb a click on `row` fires (`close`: on the tab row's close box): select, close, toggle the group, a new tab.
pub fn row_verb(row: &SideRow, close: bool) -> SMessage {
    match row {
        SideRow::Group { id, .. } => SMessage::BrowserTabGroupToggle(*id),
        SideRow::Tab { id, .. } if close => SMessage::BrowserTabClose(Some(*id)),
        SideRow::Tab { id, .. } => SMessage::BrowserTabSelect(*id),
        SideRow::New => SMessage::BrowserTabNew,
    }
}

/// A group header's text: the chevron (open / collapsed), the name (`Group` when unnamed) and the tab count.
pub fn group_label(name: &str, count: usize, collapsed: bool) -> String {
    let name = if name.trim().is_empty() { "Group" } else { name.trim() };
    format!("{} {name} ({count})", if collapsed { "▸" } else { "▾" })
}

/// `label` cut to `max` characters, the last an ellipsis.
pub fn clip(label: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if label.chars().count() <= max {
        label.to_string()
    } else {
        let mut s: String = label.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

/// The find bar's verb: search `query` (Enter and the down arrow: forward; the up arrow: back).
pub fn find_msg(query: &str, forward: bool) -> SMessage {
    SMessage::BrowserFind { query: query.to_string(), forward }
}

/// The find bar's count from `BrowserFindResult`: `No results`, else `current/matches`.
pub fn find_count(matches: u32, current: u32) -> String {
    if matches == 0 { "No results".to_string() } else { format!("{current}/{matches}") }
}

/// The status line: the hovered link (`BrowserStatus`) when there is one, else `Loading…` while the active tab loads
/// (`BrowserNavState.loading`), else nothing.
pub fn status_text(hover: &str, loading: bool) -> String {
    if !hover.is_empty() {
        hover.to_string()
    } else if loading {
        "Loading…".to_string()
    } else {
        String::new()
    }
}

/// A byte count in decimal units (`999 B`, `1.5 KB`, … `GB`).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// A download row's text from `BrowserDownload`.
pub fn download_text(name: &str, received: u64, total: Option<u64>, done: bool, error: Option<&str>) -> String {
    match (error, done, total) {
        (Some(e), _, _) => format!("{name} — failed: {e}"),
        (None, true, _) => format!("{name} — saved"),
        (None, false, Some(t)) => format!("{name} — {} of {}", human_bytes(received), human_bytes(t)),
        (None, false, None) => format!("{name} — {}", human_bytes(received)),
    }
}

/// A download row's bar: `Some(0..=1)`, or `None` while the size is unknown (an indeterminate bar). Done is full,
/// failed is empty.
pub fn download_fraction(received: u64, total: Option<u64>, done: bool, failed: bool) -> Option<f64> {
    if done || failed {
        return Some(if failed { 0.0 } else { 1.0 });
    }
    total.filter(|&t| t > 0).map(|t| (received as f64 / t as f64).clamp(0.0, 1.0))
}

// ---- STRIP2 (SR219): the top strip, the tile, the wire, the drag, the menus, the layout, the keys ---------------

/// The top strip's cells, in strip order: a group's chip where its run starts (the run hidden while collapsed), then
/// its tabs — the side column's walk ([`side_rows`]) without the `+` row (the top strip's `+` stands outside it).
pub fn top_rows(active: u64, tabs: &[BrowserTabRow], groups: &[BrowserTabGroupRow]) -> Vec<SideRow> {
    let mut rows = side_rows(active, tabs, groups);
    rows.pop();
    rows
}

/// The longest title a top-strip tab shows, in characters.
pub const TOP_LABEL_MAX: usize = 24;
/// The favicon slot's side in the top strip and in the side column (px / pt).
pub const TILE_TOP: u32 = 18;
pub const TILE_SIDE: u32 = 22;
/// The favicon tile's raster side (`aether::chrome::sidetabs::FAVICON_PX`, mirrored; a host KAT holds them equal).
pub const FAVICON_PX: u32 = 16;

/// What a tab's favicon slot shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tile {
    /// The tile the engine sent (`BrowserTabFavicon`).
    Icon,
    /// The letter tile: the tab's initial, `◌` while it loads.
    Letter(String),
}

/// The slot's face: the favicon when the face holds one and the tab is not loading, else the letter tile.
pub fn tile_face(t: &BrowserTabRow, has_icon: bool) -> Tile {
    if has_icon && !t.loading {
        Tile::Icon
    } else {
        Tile::Letter(if t.loading { "◌".to_string() } else { t.initial.clone() })
    }
}

/// A `BrowserTabFavicon`'s pixels: the first `size * size * 4` bytes, or `None` (the tile clears: an empty or short
/// payload, a zero size).
pub fn favicon_rgba(size: u32, rgba: &[u8]) -> Option<&[u8]> {
    let n = size as usize;
    if n == 0 || rgba.len() < n * n * 4 { None } else { Some(&rgba[..n * n * 4]) }
}

/// The strip as the shell last sent it (`SMessage::BrowserTabs`): what both strips draw and the tab search lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabWire {
    pub active: u64,
    pub tabs: Vec<BrowserTabRow>,
    pub groups: Vec<BrowserTabGroupRow>,
}

/// Where a dragged tab was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropOn {
    /// On a tab: it takes that tab's place.
    Tab(u64),
    /// On a group's header or chip: it joins the group.
    Group(u64),
    /// On `+`: it goes to the end of the strip.
    End,
}

impl TabWire {
    /// Take `BrowserTabs` into the wire; true when it was one.
    pub fn take(&mut self, m: &SMessage) -> bool {
        match m {
            SMessage::BrowserTabs { active, tabs, groups } => {
                *self = TabWire { active: *active, tabs: tabs.clone(), groups: groups.clone() };
                true
            }
            _ => false,
        }
    }

    pub fn group(&self, id: u64) -> Option<&BrowserTabGroupRow> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn pos(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Where a tab dragged onto tab `onto` lands (the model's `move_to` index, counted after it is taken out):
    /// dragged rightwards it goes just after `onto`, leftwards just before it — `onto`'s own index either way.
    pub fn drop_index(&self, id: u64, onto: u64) -> Option<u32> {
        self.pos(id)?;
        self.pos(onto).map(|k| k as u32)
    }

    /// The verb a drop of tab `id` fires: on a tab the move (the model joins or leaves groups by where it lands;
    /// none onto itself or for an unknown tab), on a group the join, on `+` the move to the end.
    pub fn drop_verb(&self, id: u64, on: DropOn) -> Option<SMessage> {
        match on {
            DropOn::Tab(onto) if onto == id => None,
            DropOn::Tab(onto) => self.drop_index(id, onto).map(|to| SMessage::BrowserTabMove { id, to }),
            DropOn::Group(group) => Some(SMessage::BrowserTabGroupAdd { id, group }),
            DropOn::End => Some(SMessage::BrowserTabMove { id, to: self.tabs.len() as u32 }),
        }
    }

    /// The tabs whose label or URL holds `query` (case-insensitive), in strip order; every tab for an empty query
    /// (the model's `TabStrip::search`, over the wire's rows).
    pub fn search(&self, query: &str) -> Vec<BrowserTabRow> {
        let q = query.trim().to_lowercase();
        self.tabs.iter().filter(|t| t.label.to_lowercase().contains(&q) || t.url.to_lowercase().contains(&q)).cloned().collect()
    }
}

/// The drag's payload: a tab id, as text.
pub const DRAG_PREFIX: &str = "aether-tab:";

/// The payload a dragged tab `id` carries.
pub fn drag_payload(id: u64) -> String {
    format!("{DRAG_PREFIX}{id}")
}

/// The tab id a dropped payload names, if it is one of ours.
pub fn drag_id(payload: &str) -> Option<u64> {
    payload.strip_prefix(DRAG_PREFIX).and_then(|x| x.parse::<u64>().ok())
}

/// One right-click row: its label, the verb it fires (`None`: the group's rename entry, which fires
/// `BrowserTabGroupRename` with the typed name), and the group it is about (0 on a tab's menu).
pub type MenuRow = (String, Option<SMessage>, u64);

/// A tab's menu: a new group with it, into each other group, out of its own, close.
pub fn tab_menu(s: &TabWire, id: u64) -> Vec<MenuRow> {
    let own = s.tabs.iter().find(|t| t.id == id).and_then(|t| t.group);
    let mut v = vec![("Add to a new group".to_string(), Some(SMessage::BrowserTabGroupNew { tabs: vec![id], name: String::new(), color: None }), 0)];
    for g in s.groups.iter().filter(|g| Some(g.id) != own) {
        let name = if g.name.is_empty() { format!("Add to the {} group", g.color) } else { format!("Add to “{}”", g.name) };
        v.push((name, Some(SMessage::BrowserTabGroupAdd { id, group: g.id }), 0));
    }
    if own.is_some() {
        v.push(("Remove from group".to_string(), Some(SMessage::BrowserTabUngroup(id)), 0));
    }
    v.push(("Close tab".to_string(), Some(SMessage::BrowserTabClose(Some(id))), 0));
    v
}

/// A group's menu: collapse or expand, rename, the next colour, ungroup.
pub fn group_menu(s: &TabWire, gid: u64) -> Vec<MenuRow> {
    let collapsed = s.group(gid).is_some_and(|g| g.collapsed);
    vec![
        ((if collapsed { "Expand group" } else { "Collapse group" }).to_string(), Some(SMessage::BrowserTabGroupToggle(gid)), gid),
        ("Name the group (Enter)".to_string(), None, gid),
        ("Next colour".to_string(), Some(SMessage::BrowserTabGroupRecolor { group: gid, color: None }), gid),
        ("Ungroup".to_string(), Some(SMessage::BrowserTabGroupDissolve(gid)), gid),
    ]
}

/// Principia's namespace and key for where the tabs stand (prefs_core `schema.rs`, docs/dev/PREFS-SCHEMA.md;
/// `aether::chrome::sidetabs::{PREF_NS, PREF_KEY}`).
pub const LAYOUT_NS: &str = "aether";
pub const LAYOUT_KEY: &str = "tabs.layout";

/// The stored value means the side strip (`side`); anything else, and an unset row, is the top strip.
pub fn layout_is_side(v: &str) -> bool {
    v.trim().eq_ignore_ascii_case("side")
}

/// Whether the tabs stand on the side now: Principia's row, else the schema default (`top`).
pub fn stored_layout() -> bool {
    let Ok(store) = principia::prefs::PrefStore::load(principia::default_prefs_path()) else { return false };
    if let Some(PrefValue::Str(s)) = store.get(LAYOUT_NS, LAYOUT_KEY) {
        return layout_is_side(&s);
    }
    match store.effective(LAYOUT_NS, LAYOUT_KEY, |k| std::env::var(k).ok()) {
        Some((PrefValue::Str(s), _)) => layout_is_side(&s),
        _ => false,
    }
}

/// A Principia change to the layout row.
pub fn is_layout_change(msg: &SMessage) -> bool {
    matches!(msg, SMessage::Principia(PrincipiaCommand::PrefChanged { ns, key, .. }) if ns == LAYOUT_NS && key == LAYOUT_KEY)
}

/// A chrome key's modifiers: `Primary` is Ctrl on GTK and Cmd on AppKit; `Control` is Ctrl on both (the tab step).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMods {
    Primary,
    PrimaryShift,
    Control,
    ControlShift,
}

/// One chrome key: the key (a GTK keysym name: a letter, `Tab`, `Page_Down`, `plus`, …), its modifiers, the verb.
#[derive(Clone, Debug)]
pub struct KeyVerb {
    pub key: &'static str,
    pub mods: KeyMods,
    pub msg: SMessage,
}

/// The tab strip's keys and the bandy verb each fires (each face spells the key its own way).
pub fn key_verbs() -> Vec<KeyVerb> {
    use KeyMods::*;
    let k = |key: &'static str, mods: KeyMods, msg: SMessage| KeyVerb { key, mods, msg };
    vec![
        k("t", Primary, SMessage::BrowserTabNew),
        k("w", Primary, SMessage::BrowserTabClose(None)),
        k("Tab", Control, SMessage::BrowserTabStep(1)),
        k("Page_Down", Control, SMessage::BrowserTabStep(1)),
        k("Tab", ControlShift, SMessage::BrowserTabStep(-1)),
        k("Page_Up", Control, SMessage::BrowserTabStep(-1)),
        k("d", Primary, SMessage::BrowserBookmarkToggle),
        k("plus", Primary, SMessage::BrowserZoom(1)),
        k("equal", Primary, SMessage::BrowserZoom(1)),
        k("minus", Primary, SMessage::BrowserZoom(-1)),
        k("0", Primary, SMessage::BrowserZoom(0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u64, label: &str, group: Option<u64>) -> BrowserTabRow {
        BrowserTabRow { id, label: label.into(), url: String::new(), loading: false, group, initial: "A".into() }
    }

    fn grp(id: u64, name: &str, collapsed: bool) -> BrowserTabGroupRow {
        BrowserTabGroupRow { id, name: name.into(), color: "blue".into(), rgb: 0x1a73e8, collapsed }
    }

    #[test]
    fn chrome_rows_side_rows_kat() {
        let tabs = [tab(1, "one", None), tab(2, "two", Some(7)), tab(3, "three", Some(7)), tab(4, "four", Some(8)), tab(5, "five", None)];
        let groups = [grp(7, "work", false), grp(8, "", true)];
        let rows = side_rows(3, &tabs, &groups);
        assert_eq!(
            rows,
            vec![
                SideRow::Tab { id: 1, label: "one".into(), active: false, grouped: false },
                SideRow::Group { id: 7, name: "work".into(), count: 2, collapsed: false },
                SideRow::Tab { id: 2, label: "two".into(), active: false, grouped: true },
                SideRow::Tab { id: 3, label: "three".into(), active: true, grouped: true },
                SideRow::Group { id: 8, name: "".into(), count: 1, collapsed: true },
                SideRow::Tab { id: 5, label: "five".into(), active: false, grouped: false },
                SideRow::New,
            ]
        );
        assert_eq!(side_rows(0, &[], &[]), vec![SideRow::New]);
        assert_eq!(group_label("", 1, true), "▸ Group (1)");
        assert_eq!(group_label("work", 2, false), "▾ work (2)");
    }

    #[test]
    fn chrome_rows_verbs_kat() {
        let d = |m: SMessage| format!("{m:?}");
        let t = SideRow::Tab { id: 4, label: "x".into(), active: false, grouped: false };
        assert_eq!(d(row_verb(&t, false)), "BrowserTabSelect(4)");
        assert_eq!(d(row_verb(&t, true)), "BrowserTabClose(Some(4))");
        let g = SideRow::Group { id: 9, name: "g".into(), count: 1, collapsed: false };
        assert_eq!(d(row_verb(&g, false)), "BrowserTabGroupToggle(9)");
        assert_eq!(d(row_verb(&SideRow::New, false)), "BrowserTabNew");
        assert_eq!(d(find_msg("cat", false)), "BrowserFind { query: \"cat\", forward: false }");
    }

    #[test]
    fn chrome_rows_text_kat() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(find_count(0, 0), "No results");
        assert_eq!(find_count(5, 2), "2/5");
        assert_eq!(status_text("https://a.example/", true), "https://a.example/");
        assert_eq!(status_text("", true), "Loading…");
        assert_eq!(status_text("", false), "");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1500), "1.5 KB");
        assert_eq!(download_text("f.zip", 0, None, false, Some("io")), "f.zip — failed: io");
        assert_eq!(download_text("f.zip", 10, Some(10), true, None), "f.zip — saved");
        assert_eq!(download_text("f.zip", 1500, Some(3_000_000), false, None), "f.zip — 1.5 KB of 3.0 MB");
        assert_eq!(download_text("f.zip", 12, None, false, None), "f.zip — 12 B");
        assert_eq!(download_fraction(5, Some(10), false, false), Some(0.5));
        assert_eq!(download_fraction(5, None, false, false), None);
        assert_eq!(download_fraction(5, Some(0), false, false), None);
        assert_eq!(download_fraction(5, None, true, false), Some(1.0));
        assert_eq!(download_fraction(5, Some(10), true, true), Some(0.0));
    }

    /// ONETEXT (SR210): the edges the GTK face's own copies carried, held on the one copy both faces now read.
    #[test]
    fn chrome_rows_gtk_edges_kat() {
        let ids = |rows: &[SideRow]| -> Vec<String> {
            rows.iter()
                .map(|r| match r {
                    SideRow::Group { id, count, .. } => format!("g{id}:{count}"),
                    SideRow::Tab { id, active: true, .. } => format!("*t{id}"),
                    SideRow::Tab { id, .. } => format!("t{id}"),
                    SideRow::New => "+".into(),
                })
                .collect()
        };
        // A tab naming a group the wire does not carry: shown, no header (GTK drew no stripe for it).
        let rows = side_rows(1, &[tab(1, "a", Some(99))], &[grp(7, "w", false)]);
        assert_eq!(ids(&rows), ["*t1", "+"]);
        assert!(matches!(rows[0], SideRow::Tab { grouped: true, .. }));
        // A group split into two runs: a header at each run, each the group's whole count; an empty group: none.
        let tabs = [tab(1, "a", Some(7)), tab(2, "b", None), tab(3, "c", Some(7))];
        assert_eq!(ids(&side_rows(2, &tabs, &[grp(7, "w", false), grp(8, "empty", false)])), ["g7:2", "t1", "*t2", "g7:2", "t3", "+"]);
        // The active tab inside a collapsed group: no row is marked (the GTK column unselects).
        assert_eq!(ids(&side_rows(3, &tabs, &[grp(7, "w", true)])), ["g7:2", "t2", "g7:2", "+"]);
        // The clip at the GTK widths: the boundary, multibyte titles, the top strip's 24.
        assert_eq!(clip("abcd", 4), "abcd");
        assert_eq!(clip("abcde", 4), "abc…");
        assert_eq!(clip("ééééé", 3), "éé…");
        assert_eq!(clip(&"x".repeat(30), 24).chars().count(), 24);
        assert_eq!(clip(&"y".repeat(SIDE_LABEL_MAX), SIDE_LABEL_MAX), "y".repeat(SIDE_LABEL_MAX));
        // human_bytes at the unit edges and the GB cap.
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(999_999), "1000.0 KB");
        assert_eq!(human_bytes(5_000_000_000_000), "5000.0 GB");
        // The GTK bar: a failure before the end keeps the byte fraction (failed = done && error); after it, empty.
        assert_eq!(download_fraction(5, Some(10), false, false), Some(0.5));
        assert_eq!(download_fraction(5, Some(10), true, true), Some(0.0));
        assert_eq!(download_fraction(15, Some(10), false, false), Some(1.0));
        assert_eq!(download_text("f", 5, Some(10), false, Some("io")), "f — failed: io");
    }

    /// STRIP2 (SR219): the top strip's walk is the side column's without `+`; the label width is the GTK strip's.
    #[test]
    fn chrome_rows_top_rows_kat() {
        let tabs = [tab(1, "one", None), tab(2, "two", Some(7)), tab(3, "three", Some(7)), tab(4, "four", Some(8)), tab(5, "five", None)];
        let groups = [grp(7, "work", false), grp(8, "", true)];
        let mut side = side_rows(3, &tabs, &groups);
        assert_eq!(side.pop(), Some(SideRow::New));
        assert_eq!(top_rows(3, &tabs, &groups), side);
        assert!(!top_rows(3, &tabs, &groups).contains(&SideRow::New));
        assert_eq!(top_rows(0, &[], &[]), Vec::<SideRow>::new());
        assert_eq!(clip(&"x".repeat(30), TOP_LABEL_MAX).chars().count(), 24);
    }

    /// STRIP2 (SR219): the favicon slot and the favicon payload check (the GTK `tile` / `take` rule).
    #[test]
    fn chrome_rows_tile_kat() {
        let mut t = tab(1, "a", None);
        assert_eq!(tile_face(&t, true), Tile::Icon);
        assert_eq!(tile_face(&t, false), Tile::Letter("A".into()));
        t.loading = true;
        assert_eq!(tile_face(&t, true), Tile::Letter("◌".into()));
        assert_eq!(tile_face(&t, false), Tile::Letter("◌".into()));
        let px = vec![7u8; 16 * 16 * 4 + 3];
        assert_eq!(favicon_rgba(16, &px).map(|p| p.len()), Some(1024));
        assert_eq!(favicon_rgba(16, &px[..1023]), None);
        assert_eq!(favicon_rgba(0, &px), None);
        assert_eq!(favicon_rgba(16, &[]), None);
        assert_eq!((FAVICON_PX, TILE_TOP, TILE_SIDE), (16, 18, 22));
    }

    /// STRIP2 (SR219): the drag (payload, landing, verbs) and the right-click rows.
    #[test]
    fn chrome_rows_drag_menu_kat() {
        let d = |m: Option<SMessage>| format!("{m:?}");
        let mut w = TabWire::default();
        let tabs = vec![tab(1, "a", None), tab(2, "b", Some(7)), tab(3, "c", None)];
        assert!(w.take(&SMessage::BrowserTabs { active: 2, tabs: tabs.clone(), groups: vec![grp(7, "work", false), grp(9, "", true)] }));
        assert!(!w.take(&SMessage::BrowserTabNew));
        assert_eq!((w.active, w.tabs.len(), w.groups.len()), (2, 3, 2));
        assert_eq!(drag_id(&drag_payload(42)), Some(42));
        assert_eq!(drag_payload(5), "aether-tab:5");
        assert_eq!(drag_id("aether-tab:x"), None);
        assert_eq!(drag_id("42"), None);
        assert_eq!(d(w.drop_verb(1, DropOn::Tab(3))), "Some(BrowserTabMove { id: 1, to: 2 })");
        assert_eq!(d(w.drop_verb(3, DropOn::Tab(1))), "Some(BrowserTabMove { id: 3, to: 0 })");
        assert_eq!(d(w.drop_verb(1, DropOn::Tab(1))), "None");
        assert_eq!(d(w.drop_verb(99, DropOn::Tab(1))), "None");
        assert_eq!(d(w.drop_verb(1, DropOn::Group(7))), "Some(BrowserTabGroupAdd { id: 1, group: 7 })");
        assert_eq!(d(w.drop_verb(1, DropOn::End)), "Some(BrowserTabMove { id: 1, to: 3 })");
        let labels = |rows: Vec<MenuRow>| rows.into_iter().map(|(l, m, g)| format!("{l}|{}|{g}", m.is_some())).collect::<Vec<_>>();
        assert_eq!(labels(tab_menu(&w, 1)), ["Add to a new group|true|0", "Add to “work”|true|0", "Add to the blue group|true|0", "Close tab|true|0"]);
        assert_eq!(labels(tab_menu(&w, 2)), ["Add to a new group|true|0", "Add to the blue group|true|0", "Remove from group|true|0", "Close tab|true|0"]);
        assert_eq!(labels(group_menu(&w, 9)), ["Expand group|true|9", "Name the group (Enter)|false|9", "Next colour|true|9", "Ungroup|true|9"]);
        assert_eq!(format!("{:?}", tab_menu(&w, 2)[2].1), "Some(BrowserTabUngroup(2))");
        assert_eq!(format!("{:?}", group_menu(&w, 7)[0].1), "Some(BrowserTabGroupToggle(7))");
    }

    /// STRIP2 (SR219): the tab search over the wire (title or URL, case-insensitive, trimmed; all for empty).
    #[test]
    fn chrome_rows_search_kat() {
        let mut a = tab(1, "Rust Docs", None);
        a.url = "https://doc.rust-lang.org/".into();
        let mut b = tab(2, "News", None);
        b.url = "https://news.example/RUST".into();
        let c = tab(3, "Mail", None);
        let w = TabWire { active: 1, tabs: vec![a, b, c], groups: vec![] };
        let ids = |q: &str| w.search(q).iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids("rust"), [1, 2]);
        assert_eq!(ids("  MAIL "), [3]);
        assert_eq!(ids(""), [1, 2, 3]);
        assert_eq!(ids("nothing"), Vec::<u64>::new());
    }

    /// STRIP2 (SR219): the keys and their verbs; the layout row's parse and its change test.
    #[test]
    fn chrome_rows_keys_layout_kat() {
        let keys: Vec<String> = key_verbs().iter().map(|k| format!("{}:{:?}:{:?}", k.key, k.mods, k.msg)).collect();
        assert_eq!(
            keys,
            [
                "t:Primary:BrowserTabNew",
                "w:Primary:BrowserTabClose(None)",
                "Tab:Control:BrowserTabStep(1)",
                "Page_Down:Control:BrowserTabStep(1)",
                "Tab:ControlShift:BrowserTabStep(-1)",
                "Page_Up:Control:BrowserTabStep(-1)",
                "d:Primary:BrowserBookmarkToggle",
                "plus:Primary:BrowserZoom(1)",
                "equal:Primary:BrowserZoom(1)",
                "minus:Primary:BrowserZoom(-1)",
                "0:Primary:BrowserZoom(0)",
            ]
        );
        assert!(layout_is_side(" Side "));
        assert!(!layout_is_side("top"));
        assert!(!layout_is_side(""));
        let change = |ns: &str, key: &str| {
            SMessage::Principia(PrincipiaCommand::PrefChanged { ns: ns.into(), key: key.into(), value: PrefValue::Str("side".into()), clamped: false })
        };
        assert!(is_layout_change(&change("aether", "tabs.layout")));
        assert!(!is_layout_change(&change("aether", "tabs.other")));
        assert!(!is_layout_change(&SMessage::BrowserTabNew));
    }
}
