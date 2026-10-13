//! A tab's session history (std-free) — HTML §7.4 "session history", the joint-history-of-one-
//! navigable model: an ordered list of session history entries and the index of the current one.
//!
//! Each [`Entry`] carries its URL, its serialized state (`history.pushState`'s data after
//! StructuredSerializeForStorage — JSON here, the same approximation `js::window` uses), its title and
//! its DOCUMENT: entries that share a document id are same-document entries (made by `pushState`,
//! `replaceState`'s target, or a fragment navigation), and traversing between two of them never fetches
//! — the engine fires `popstate` (and `hashchange` when only the fragment differs) instead
//! (§7.4.6.3 "update document for history step application"). Traversing to an entry of ANOTHER
//! document is a cross-document load: the shell fetches the entry's URL and the engine
//! [`SessionHistory::commit_traversal_load`]s it, which gives that document a fresh id (its old
//! document is gone — Aether keeps no bfcache — and every entry that pointed at it now points at the
//! reloaded one).
//!
//! A new navigation (link, form, typed URL, `location.assign`) or a `pushState` truncates every entry
//! after the current one first (§7.4.4 "finalize a cross-document navigation": the forward history is
//! cleared).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One session history entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    /// The serialized state (`None` is `null`).
    pub state: Option<String>,
    pub title: String,
    /// The document this entry belongs to (same id = same document).
    pub doc: u64,
}

/// What a traversal has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Traversal {
    /// The target entry is in the current document: update the URL and fire `popstate` with `state`.
    SameDocument { url: String, state: Option<String>, from_url: String },
    /// The target entry is another document's: load `url`, then [`SessionHistory::commit_traversal_load`].
    CrossDocument { url: String },
}

/// A tab's session history.
#[derive(Clone, Debug, Default)]
pub struct SessionHistory {
    entries: Vec<Entry>,
    index: usize,
    next_doc: u64,
}

impl SessionHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The current entry's index (0 when empty).
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.index)
    }

    /// The entries' URLs, in order (the engine's `history` mirror).
    pub fn urls(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.url.clone()).collect()
    }

    /// Whether `history.go(delta)` has an entry to land on.
    pub fn can_go(&self, delta: i32) -> bool {
        self.target(delta).is_some()
    }

    fn target(&self, delta: i32) -> Option<usize> {
        if delta == 0 || self.entries.is_empty() {
            return None;
        }
        let t = self.index as i64 + delta as i64;
        (0..self.entries.len() as i64).contains(&t).then_some(t as usize)
    }

    fn fresh_doc(&mut self) -> u64 {
        self.next_doc += 1;
        self.next_doc
    }

    fn truncate_forward(&mut self) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
    }

    /// A cross-document navigation committed `url`: forward entries go, a new entry of a new document
    /// becomes current. Returns its document id.
    pub fn push_document(&mut self, url: &str) -> u64 {
        self.truncate_forward();
        let doc = self.fresh_doc();
        self.entries.push(Entry { url: url.to_string(), state: None, title: String::new(), doc });
        self.index = self.entries.len() - 1;
        doc
    }

    /// `history.pushState(state, "", url)` or a fragment navigation: a new entry of the CURRENT
    /// document (§7.4.2 "URL and history update steps", historyHandling "push").
    pub fn push_same_document(&mut self, url: &str, state: Option<String>) {
        let Some(cur) = self.current() else {
            self.push_document(url);
            self.entries[self.index].state = state;
            return;
        };
        let (doc, title) = (cur.doc, cur.title.clone());
        self.truncate_forward();
        self.entries.push(Entry { url: url.to_string(), state, title, doc });
        self.index = self.entries.len() - 1;
    }

    /// `history.replaceState(state, "", url)` (historyHandling "replace"): the current entry's URL and
    /// state change in place.
    pub fn replace(&mut self, url: &str, state: Option<String>) {
        match self.entries.get_mut(self.index) {
            Some(e) => {
                e.url = url.to_string();
                e.state = state;
            }
            None => {
                self.push_document(url);
                self.entries[self.index].state = state;
            }
        }
    }

    /// The current document's title changed (`<title>`, `document.title`).
    pub fn set_title(&mut self, title: &str) {
        if let Some(e) = self.entries.get_mut(self.index) {
            e.title = title.to_string();
        }
    }

    /// `history.go(delta)`: move the current index and say what the engine must do. `None` (and no
    /// move) when there is no such entry.
    pub fn go(&mut self, delta: i32) -> Option<Traversal> {
        let t = self.target(delta)?;
        let from = self.entries[self.index].clone();
        self.index = t;
        let to = &self.entries[t];
        Some(if to.doc == from.doc {
            Traversal::SameDocument { url: to.url.clone(), state: to.state.clone(), from_url: from.url }
        } else {
            Traversal::CrossDocument { url: to.url.clone() }
        })
    }

    /// The current entry's document was (re)loaded — a cross-document traversal, a reload, or a
    /// redirect that landed on `url`: the entry takes the final URL, and the document is a new one for
    /// every entry that shared the old.
    pub fn commit_traversal_load(&mut self, url: &str) {
        if self.entries.is_empty() {
            self.push_document(url);
            return;
        }
        let old = self.entries[self.index].doc;
        let doc = self.fresh_doc();
        for e in self.entries.iter_mut().filter(|e| e.doc == old) {
            e.doc = doc;
        }
        self.entries[self.index].url = url.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_state_entries_traverse_without_a_load_and_documents_do() {
        let mut h = SessionHistory::new();
        h.push_document("https://a.test/");
        h.push_same_document("https://a.test/p1", Some("{\"n\":1}".into()));
        h.push_document("https://b.test/");
        assert_eq!(h.go(-1), Some(Traversal::CrossDocument { url: "https://a.test/p1".into() }));
        h.commit_traversal_load("https://a.test/p1");
        assert_eq!(
            h.go(-1),
            Some(Traversal::SameDocument { url: "https://a.test/".into(), state: None, from_url: "https://a.test/p1".into() })
        );
        assert_eq!(
            h.go(1),
            Some(Traversal::SameDocument { url: "https://a.test/p1".into(), state: Some("{\"n\":1}".into()), from_url: "https://a.test/".into() })
        );
        assert!(h.can_go(1) && !h.can_go(2));
        // pushState from the middle drops the forward tail (b.test).
        h.push_same_document("https://a.test/p2", None);
        assert_eq!(h.urls(), ["https://a.test/", "https://a.test/p1", "https://a.test/p2"]);
        assert!(!h.can_go(1));
        h.replace("https://a.test/p3", Some("1".into()));
        assert_eq!(h.current().map(|e| e.url.as_str()), Some("https://a.test/p3"));
        assert_eq!(h.go(5), None);
    }
}
