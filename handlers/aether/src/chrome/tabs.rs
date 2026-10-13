//! The tab strip (std-free): which tabs exist, in what order, and which one is active.
//!
//! The shell runs one engine per tab (its own thread, so the engine's page-thread state — the JS
//! realm, the selection, the media registry — is per tab by construction); this model is what the
//! strip shows and what Ctrl+T / Ctrl+W / Ctrl+Tab move. Ids are never reused, so a stale close or
//! select from the face can only miss, never hit the wrong tab.
//!
//! AETHERSIDETABS (SR140, R118): the many-tabs user's model. A tab may sit in a named, coloured GROUP
//! ([`TabGroup`]; Chrome's rule: a group's tabs are CONTIGUOUS in the strip, and a group with no tabs is
//! gone); a group collapses and expands; [`TabStrip::move_to`] reorders (the drag), and dropping a tab
//! between two tabs of a group puts it in that group, dragging it clear of its own takes it out;
//! [`TabStrip::search`] is Ctrl+Shift+A's title / URL substring match. The vertical side strip's geometry
//! is [`super::sidetabs`]; both strips draw [`TabStrip::rows`] / [`TabStrip::top_segments`].

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a tab with no title and no URL is called.
pub const NEW_TAB: &str = "New Tab";

/// A tab group's id. Like tab ids, never reused.
pub type GroupId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub loading: bool,
    /// The group the tab sits in (None: ungrouped).
    pub group: Option<GroupId>,
}

/// Chrome's nine tab-group colours, in its picker order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupColor {
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Pink,
    Purple,
    Cyan,
    Orange,
}

impl GroupColor {
    pub const ALL: [GroupColor; 9] = [
        GroupColor::Grey,
        GroupColor::Blue,
        GroupColor::Red,
        GroupColor::Yellow,
        GroupColor::Green,
        GroupColor::Pink,
        GroupColor::Purple,
        GroupColor::Cyan,
        GroupColor::Orange,
    ];

    /// The colour as `0xRRGGBB` (the light-theme swatch; a face may darken it for a dark theme).
    pub fn rgb(self) -> u32 {
        match self {
            GroupColor::Grey => 0x5F6368,
            GroupColor::Blue => 0x1A73E8,
            GroupColor::Red => 0xD93025,
            GroupColor::Yellow => 0xF9AB00,
            GroupColor::Green => 0x1E8E3E,
            GroupColor::Pink => 0xD01884,
            GroupColor::Purple => 0x9334E6,
            GroupColor::Cyan => 0x007B83,
            GroupColor::Orange => 0xFA903E,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            GroupColor::Grey => "grey",
            GroupColor::Blue => "blue",
            GroupColor::Red => "red",
            GroupColor::Yellow => "yellow",
            GroupColor::Green => "green",
            GroupColor::Pink => "pink",
            GroupColor::Purple => "purple",
            GroupColor::Cyan => "cyan",
            GroupColor::Orange => "orange",
        }
    }

    pub fn parse(s: &str) -> Option<GroupColor> {
        GroupColor::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(s.trim()))
    }

    /// The colour after this one in the picker's order, wrapping (AETHERSIDETABS2: a face's "next colour").
    pub fn next(self) -> GroupColor {
        let i = GroupColor::ALL.iter().position(|c| *c == self).unwrap_or(0);
        GroupColor::ALL[(i + 1) % GroupColor::ALL.len()]
    }
}

/// A named, coloured group of contiguous tabs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabGroup {
    pub id: GroupId,
    pub name: String,
    pub color: GroupColor,
    /// Collapsed: the side strip shows only the group's header row, the top strip only its segment.
    pub collapsed: bool,
}

/// One row of the strip as a face draws it, in order: a group's header, then (unless the group is collapsed)
/// its tabs; ungrouped tabs stand alone; the `+` row closes the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Group(GroupId),
    Tab(u64),
    NewTab,
}

impl Tab {
    /// The label the strip shows: the title, else the URL, else [`NEW_TAB`].
    pub fn label(&self) -> &str {
        if !self.title.trim().is_empty() {
            &self.title
        } else if !self.url.is_empty() {
            &self.url
        } else {
            NEW_TAB
        }
    }

    /// The letter tile a face shows when the engine has no favicon: the first alphanumeric of the host (for a
    /// URL) or of the label, upper-cased; `+` for a new tab.
    pub fn initial(&self) -> char {
        let host = self.url.split("://").nth(1).map(|r| r.split('/').next().unwrap_or(""));
        let host = host.map(|h| h.strip_prefix("www.").unwrap_or(h));
        let from = match host {
            Some(h) if !h.is_empty() && self.title.trim().is_empty() => h,
            _ => self.label(),
        };
        if self.url.is_empty() && self.title.trim().is_empty() {
            return '+';
        }
        match from.chars().find(|c| c.is_alphanumeric()) {
            Some(c) => c.to_uppercase().next().unwrap_or(c),
            None => '#',
        }
    }

    /// Whether `needle` (already lower-cased) is in the title or the URL, case-insensitively.
    fn matches(&self, needle: &str) -> bool {
        contains_folded(&self.title, needle) || contains_folded(&self.url, needle)
    }
}

/// Simple case-folded substring test (`needle` already lower-cased).
fn contains_folded(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay: String = hay.chars().flat_map(|c| c.to_lowercase()).collect();
    hay.contains(needle)
}

#[derive(Clone, Debug)]
pub struct TabStrip {
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
    groups: Vec<TabGroup>,
    next_group: GroupId,
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl TabStrip {
    pub fn new() -> Self {
        Self { tabs: Vec::new(), active: 0, next_id: 1, groups: Vec::new(), next_group: 1 }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The active tab's id (0 when the strip is empty).
    pub fn active(&self) -> u64 {
        self.active
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.get(self.active)
    }

    pub fn get(&self, id: u64) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn pos(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// A new tab at the end of the strip (Ctrl+T), made active. Returns its id.
    pub fn open(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab { id, title: String::new(), url: String::new(), loading: false, group: None });
        self.active = id;
        id
    }

    /// Close `id`. When it was the active tab, the one to its right (else its left) becomes active.
    /// Returns the active id afterwards — `None` when the strip is now empty (the shell opens a fresh
    /// tab rather than leave a window with nothing in it) — or `None` too when `id` is unknown and the
    /// strip is empty.
    pub fn close(&mut self, id: u64) -> Option<u64> {
        if let Some(i) = self.pos(id) {
            self.tabs.remove(i);
            self.prune_groups();
            if self.active == id {
                self.active = match self.tabs.get(i).or_else(|| i.checked_sub(1).and_then(|j| self.tabs.get(j))) {
                    Some(t) => t.id,
                    None => 0,
                };
            }
        }
        (self.active != 0).then_some(self.active)
    }

    /// Make `id` active (its group, if collapsed, expands: the active tab is always on view). False when there
    /// is no such tab.
    pub fn select(&mut self, id: u64) -> bool {
        if let Some(i) = self.pos(id) {
            self.active = id;
            self.reveal(i);
            true
        } else {
            false
        }
    }

    /// Move the active tab `n` places along the strip, wrapping. Returns the new active id.
    pub fn step(&mut self, n: i32) -> Option<u64> {
        let len = self.tabs.len() as i64;
        let i = self.pos(self.active)? as i64;
        let j = (i + n as i64).rem_euclid(len) as usize;
        self.active = self.tabs[j].id;
        self.reveal(j);
        Some(self.active)
    }

    /// Record what tab `id`'s engine reported. False when the tab is gone (a late report).
    pub fn update(&mut self, id: u64, title: Option<&str>, url: Option<&str>, loading: Option<bool>) -> bool {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return false };
        if let Some(v) = title {
            t.title = v.to_string();
        }
        if let Some(v) = url {
            t.url = v.to_string();
        }
        if let Some(v) = loading {
            t.loading = v;
        }
        true
    }

    /// The strip as the face draws it: `(id, label, loading)` in order.
    pub fn snapshot(&self) -> Vec<(u64, String, bool)> {
        self.tabs.iter().map(|t| (t.id, t.label().to_string(), t.loading)).collect()
    }

    // ---- AETHERSIDETABS (SR140): reorder, groups, search ------------------------------------------------------

    /// Expand the group of the tab at index `i`, if it is collapsed.
    fn reveal(&mut self, i: usize) {
        if let Some(g) = self.tabs.get(i).and_then(|t| t.group) {
            if let Some(grp) = self.groups.iter_mut().find(|x| x.id == g) {
                grp.collapsed = false;
            }
        }
    }

    /// Drop every group that no longer has a tab.
    fn prune_groups(&mut self) {
        let tabs = &self.tabs;
        self.groups.retain(|g| tabs.iter().any(|t| t.group == Some(g.id)));
    }

    /// Move tab `id` to index `to` (clamped to the strip; the index it ends up at, counted after it is taken
    /// out). Dropped between two tabs of one group, it joins that group; dragged clear of its own group (no
    /// neighbour in it any more), it leaves it — so every group stays contiguous. False when `id` is unknown.
    pub fn move_to(&mut self, id: u64, to: usize) -> bool {
        let Some(i) = self.pos(id) else { return false };
        let mut t = self.tabs.remove(i);
        let j = to.min(self.tabs.len());
        let left = j.checked_sub(1).and_then(|k| self.tabs.get(k)).and_then(|x| x.group);
        let right = self.tabs.get(j).and_then(|x| x.group);
        if left.is_some() && left == right {
            t.group = left;
        } else if t.group.is_some() && t.group != left && t.group != right {
            t.group = None;
        }
        self.tabs.insert(j, t);
        self.prune_groups();
        true
    }

    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    pub fn group(&self, gid: GroupId) -> Option<&TabGroup> {
        self.groups.iter().find(|g| g.id == gid)
    }

    /// The tabs of group `gid`, in strip order.
    pub fn group_tabs(&self, gid: GroupId) -> Vec<u64> {
        self.tabs.iter().filter(|t| t.group == Some(gid)).map(|t| t.id).collect()
    }

    /// A new group named `name` in `color` holding `tabs` (unknown ids skipped), gathered contiguously where the
    /// first of them stands. None when no tab in `tabs` exists.
    pub fn group_new(&mut self, name: &str, color: GroupColor, tabs: &[u64]) -> Option<GroupId> {
        let first = tabs.iter().filter_map(|&id| self.pos(id)).min()?;
        let gid = self.next_group;
        self.next_group += 1;
        self.groups.push(TabGroup { id: gid, name: name.to_string(), color, collapsed: false });
        // Take the members out in strip order, then put them back as one run at the first one's place.
        // No member stands before `first`, so after the take-out index `first` is still the place.
        let mut members = Vec::new();
        let mut k = first;
        while k < self.tabs.len() {
            if tabs.contains(&self.tabs[k].id) {
                let mut t = self.tabs.remove(k);
                t.group = Some(gid);
                members.push(t);
            } else {
                k += 1;
            }
        }
        // If that place splits another group's run, the new run goes after that group.
        let at = self.clear_of_groups(first.min(self.tabs.len()));
        for (n, t) in members.into_iter().enumerate() {
            self.tabs.insert(at + n, t);
        }
        self.prune_groups();
        Some(gid)
    }

    /// The nearest index at or after `at` that does not fall inside a group's run.
    fn clear_of_groups(&self, mut at: usize) -> usize {
        while at > 0 && at < self.tabs.len() {
            let (l, r) = (self.tabs[at - 1].group, self.tabs[at].group);
            if l.is_some() && l == r {
                at += 1;
            } else {
                break;
            }
        }
        at
    }

    /// Put tab `id` in group `gid` (it moves to the end of the group's run). False when either is unknown.
    pub fn group_add(&mut self, id: u64, gid: GroupId) -> bool {
        if self.group(gid).is_none() || self.pos(id).is_none() {
            return false;
        }
        if self.get(id).and_then(|t| t.group) == Some(gid) {
            return true;
        }
        let i = self.pos(id).unwrap_or(0);
        let mut t = self.tabs.remove(i);
        t.group = Some(gid);
        let end = self.tabs.iter().rposition(|x| x.group == Some(gid)).map(|k| k + 1).unwrap_or(self.tabs.len());
        self.tabs.insert(end, t);
        self.prune_groups();
        true
    }

    /// The colour a new group gets when the face names none: the first in the picker's order that no group uses,
    /// else the one after the newest group's (AETHERSIDETABS2, SR143).
    pub fn next_color(&self) -> GroupColor {
        GroupColor::ALL
            .into_iter()
            .find(|c| !self.groups.iter().any(|g| g.color == *c))
            .unwrap_or_else(|| self.groups.last().map(|g| g.color.next()).unwrap_or(GroupColor::Grey))
    }

    /// Take tab `id` out of its group (it moves just past the group's run, so the run stays whole).
    pub fn ungroup(&mut self, id: u64) -> bool {
        let Some(gid) = self.get(id).and_then(|t| t.group) else { return false };
        let i = self.pos(id).unwrap_or(0);
        let mut t = self.tabs.remove(i);
        t.group = None;
        let end = self.tabs.iter().rposition(|x| x.group == Some(gid)).map(|k| k + 1).unwrap_or(i.min(self.tabs.len()));
        self.tabs.insert(end, t);
        self.prune_groups();
        true
    }

    /// Dissolve group `gid`: its tabs stay where they are, ungrouped.
    pub fn group_dissolve(&mut self, gid: GroupId) -> bool {
        let found = self.group(gid).is_some();
        for t in self.tabs.iter_mut().filter(|t| t.group == Some(gid)) {
            t.group = None;
        }
        self.prune_groups();
        found
    }

    pub fn group_rename(&mut self, gid: GroupId, name: &str) -> bool {
        match self.groups.iter_mut().find(|g| g.id == gid) {
            Some(g) => {
                g.name = name.to_string();
                true
            }
            None => false,
        }
    }

    pub fn group_recolor(&mut self, gid: GroupId, color: GroupColor) -> bool {
        match self.groups.iter_mut().find(|g| g.id == gid) {
            Some(g) => {
                g.color = color;
                true
            }
            None => false,
        }
    }

    /// Collapse or expand group `gid`; answers whether it is now collapsed. Collapsing the group that holds the
    /// active tab moves the active tab to the nearest tab outside it (Chrome's rule); when every tab is in it,
    /// the group stays expanded.
    pub fn group_toggle(&mut self, gid: GroupId) -> Option<bool> {
        let now = !self.group(gid)?.collapsed;
        if now && self.get(self.active).and_then(|t| t.group) == Some(gid) {
            let i = self.pos(self.active).unwrap_or(0);
            let after = self.tabs[i..].iter().find(|t| t.group != Some(gid)).map(|t| t.id);
            let before = self.tabs[..i].iter().rev().find(|t| t.group != Some(gid)).map(|t| t.id);
            match after.or(before) {
                Some(id) => self.active = id,
                None => return Some(false),
            }
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
            g.collapsed = now;
        }
        Some(now)
    }

    /// Ctrl+Shift+A: the tabs whose title or URL contains `query` (case-insensitive), in strip order; every tab
    /// for an empty query.
    pub fn search(&self, query: &str) -> Vec<u64> {
        let needle: String = query.trim().chars().flat_map(|c| c.to_lowercase()).collect();
        self.tabs.iter().filter(|t| t.matches(&needle)).map(|t| t.id).collect()
    }

    /// The rows a vertical strip draws (see [`Row`]).
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::with_capacity(self.tabs.len() + self.groups.len() + 1);
        let mut cur: Option<GroupId> = None;
        for t in &self.tabs {
            if t.group != cur {
                cur = t.group;
                if let Some(g) = cur {
                    out.push(Row::Group(g));
                }
            }
            let hidden = t.group.and_then(|g| self.group(g)).is_some_and(|g| g.collapsed);
            if !hidden {
                out.push(Row::Tab(t.id));
            }
        }
        out.push(Row::NewTab);
        out
    }

    /// The horizontal strip's group segments: `(group, first tab index, tab count)` in order — a face paints a
    /// coloured bar under that run (or, collapsed, only a chip with the group's name).
    pub fn top_segments(&self) -> Vec<(GroupId, usize, usize)> {
        let mut out: Vec<(GroupId, usize, usize)> = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(g) = t.group {
                match out.last_mut() {
                    Some(last) if last.0 == g && last.1 + last.2 == i => last.2 += 1,
                    _ => out.push((g, i, 1)),
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_close_select_and_step_keep_one_active_tab() {
        let mut s = TabStrip::new();
        let a = s.open();
        let b = s.open();
        let c = s.open();
        assert_eq!(s.active(), c);
        assert_eq!(s.step(1), Some(a), "Ctrl+Tab wraps");
        assert_eq!(s.step(-1), Some(c), "Ctrl+Shift+Tab wraps back");
        assert!(s.select(b));
        assert_eq!(s.close(b), Some(c), "closing the active tab activates its right neighbour");
        assert_eq!(s.close(c), Some(a), "…or its left one at the end of the strip");
        assert_eq!(s.close(a), None, "the strip is empty");
        assert!(!s.select(b), "ids are never reused");
        s.open();
        assert_eq!(s.snapshot()[0].1, NEW_TAB);
    }
}
