//! Bookmarks (std-free): an ordered list of (title, URL), and its file format.
//!
//! The file is the person's own, under Aether's data directory (`host::bookmarks_path`: the same
//! `ProjectDirs("", "UnaOS", "Aether")` data dir `storage::LocalStorage` uses). It is plain text a
//! person can edit: one bookmark per line, `URL<TAB>title`, `#` lines are comments. A tab or line
//! break inside a title is written as a space, so a line always parses back to the bookmark it was.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmarks {
    items: Vec<Bookmark>,
}

const HEADER: &str = "# Aether bookmarks: one per line, URL<TAB>title\n";

impl Bookmarks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse the file. Malformed lines (no URL) are skipped; a line without a tab is a URL with no
    /// title. A URL listed twice keeps its first entry.
    pub fn parse(text: &str) -> Self {
        let mut b = Self::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (url, title) = line.split_once('\t').unwrap_or((line, ""));
            let url = url.trim();
            if url.is_empty() || b.contains(url) {
                continue;
            }
            b.items.push(Bookmark { title: title.trim().to_string(), url: url.to_string() });
        }
        b
    }

    /// The file's text.
    pub fn serialize(&self) -> String {
        let mut out = String::from(HEADER);
        for bm in &self.items {
            out.push_str(&clean(&bm.url));
            out.push('\t');
            out.push_str(&clean(&bm.title));
            out.push('\n');
        }
        out
    }

    pub fn items(&self) -> &[Bookmark] {
        &self.items
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }

    /// Add `url` (at the end) when it is not bookmarked, remove it when it is (Ctrl+D, the star).
    /// Returns whether it is bookmarked afterwards. An empty URL is never bookmarked.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        if url.is_empty() {
            return false;
        }
        if let Some(i) = self.items.iter().position(|b| b.url == url) {
            self.items.remove(i);
            false
        } else {
            self.items.push(Bookmark { title: title.to_string(), url: url.to_string() });
            true
        }
    }

    /// `(title, url)` pairs for the wire (`SMessage::BrowserBookmarks`).
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.items.iter().map(|b| (b.title.clone(), b.url.clone())).collect()
    }
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_toggle_flips() {
        let mut b = Bookmarks::new();
        assert!(b.toggle("https://una.os/", "Una\tOS"));
        assert!(b.toggle("https://a.test/", ""));
        let text = b.serialize();
        let back = Bookmarks::parse(&text);
        assert_eq!(back.items()[0], Bookmark { title: "Una OS".into(), url: "https://una.os/".into() });
        assert_eq!(back.items().len(), 2);
        assert!(!b.toggle("https://una.os/", "x"), "a second toggle removes it");
        assert!(!b.contains("https://una.os/"));
        assert_eq!(Bookmarks::parse("# c\n\nhttps://x.test/\nhttps://x.test/\tdup\n").items().len(), 1);
    }
}
