//! The tab strip (std-free): which tabs exist, in what order, and which one is active.
//!
//! The shell runs one engine per tab (its own thread, so the engine's page-thread state — the JS
//! realm, the selection, the media registry — is per tab by construction); this model is what the
//! strip shows and what Ctrl+T / Ctrl+W / Ctrl+Tab move. Ids are never reused, so a stale close or
//! select from the face can only miss, never hit the wrong tab.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a tab with no title and no URL is called.
pub const NEW_TAB: &str = "New Tab";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub loading: bool,
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
}

#[derive(Clone, Debug)]
pub struct TabStrip {
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl TabStrip {
    pub fn new() -> Self {
        Self { tabs: Vec::new(), active: 0, next_id: 1 }
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
        self.tabs.push(Tab { id, title: String::new(), url: String::new(), loading: false });
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
            if self.active == id {
                self.active = match self.tabs.get(i).or_else(|| i.checked_sub(1).and_then(|j| self.tabs.get(j))) {
                    Some(t) => t.id,
                    None => 0,
                };
            }
        }
        (self.active != 0).then_some(self.active)
    }

    /// Make `id` active. False when there is no such tab.
    pub fn select(&mut self, id: u64) -> bool {
        if self.pos(id).is_some() {
            self.active = id;
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
