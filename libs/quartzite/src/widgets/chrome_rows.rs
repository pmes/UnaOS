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

use bandy::{BrowserTabGroupRow, BrowserTabRow, SMessage};

/// The side column's width open and as the icon rail, and one row's base height (`aether::chrome::sidetabs::{SIDE_W,
/// RAIL_W, ROW_H}`, mirrored: quartzite does not link the handler).
pub const SIDE_W: u32 = 240;
pub const RAIL_W: u32 = 48;
pub const ROW_H: u32 = 32;
/// The longest title a side row shows, in characters (the GTK side column's `SIDE_LABEL_MAX`).
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
}
