//! The Console panel: the page's console output and uncaught errors, a one-line evaluator in the page's
//! realm, and the engine's honest-failure lines as Issues.
//!
//! Seam: `js::console_lines` — PageState.console, which `js::host::console_out` fills for every
//! `console.*` call (log 0, warn 1, error 2, debug/trace 3, info 4), every uncaught exception ("Uncaught
//! …", level 2, js/events.rs) and every security refusal ("[security] …", level 2, SR86); the evaluator is
//! `js::Engine::eval_string` on `AetherEngine::js_engine` — the page's own VM and global, so a line sees
//! the page's variables and its `console.log` lands in the page's console like the page's own; the
//! Issues list is `ledger::snapshot().entries()` (the API coverage ledger's read API).
//!
//! The pane's own state is only the transcript of what was typed and answered: each transcript entry
//! carries the page-console length at the moment it was made, so the merged view interleaves the two in
//! the order they happened. A new document (its console starts empty) drops the transcript.

use crate::AetherEngine;

/// A console line's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Log,
    Warn,
    Error,
    Debug,
    Info,
    /// A line typed into the evaluator.
    Input,
    /// The evaluator's answer.
    Result,
    /// The evaluator's thrown exception.
    Exception,
}

impl Level {
    /// The page console's numeric level (`js::host::console_out`) as a [`Level`].
    pub fn from_page(level: u8) -> Level {
        match level {
            1 => Level::Warn,
            2 => Level::Error,
            3 => Level::Debug,
            4 => Level::Info,
            _ => Level::Log,
        }
    }

    /// The gutter mark a text face shows.
    pub fn glyph(self) -> &'static str {
        match self {
            Level::Log => " ",
            Level::Warn => "!",
            Level::Error => "x",
            Level::Debug => ".",
            Level::Info => "i",
            Level::Input => ">",
            Level::Result => "<",
            Level::Exception => "x",
        }
    }
}

/// One console line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub level: Level,
    pub text: String,
}

/// One honest-failure line of the engine's ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub category: String,
    pub name: String,
    pub count: usize,
}

/// The evaluator's transcript and input history.
#[derive(Debug, Default)]
pub struct Console {
    /// (page console length when made, the URL of the page it was made on, the entry).
    transcript: Vec<(usize, String, Entry)>,
    /// Lines typed, oldest first (the input field's Up/Down).
    history: Vec<String>,
}

impl Console {
    /// The lines typed so far, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Runs `line` in the page's realm: the input and its answer (or exception) join the transcript, a
    /// mutation the line made is laid out at once, and the answer is returned.
    pub fn evaluate(&mut self, engine: &mut AetherEngine, line: &str) -> Entry {
        let url = crate::js::page_url();
        self.prune(&url);
        let line = line.trim_end_matches(['\r', '\n']);
        if !line.trim().is_empty() && self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
        let before = crate::js::console_lines().len();
        self.transcript.push((before, url.clone(), Entry { level: Level::Input, text: line.to_string() }));
        let answer = match engine.js_engine.as_mut() {
            None => Entry { level: Level::Exception, text: "no page realm: nothing is loaded".to_string() },
            Some(js) => match js.eval_string(line) {
                Ok(v) => Entry { level: Level::Result, text: v },
                Err(e) => Entry { level: Level::Exception, text: format!("Uncaught {e}") },
            },
        };
        // What the line logged precedes its answer.
        let after = crate::js::console_lines().len();
        self.transcript.push((after, url, answer.clone()));
        if crate::js::take_mutated() {
            engine.relayout();
        }
        answer
    }

    /// Drops the transcript of another document.
    fn prune(&mut self, url: &str) {
        let len = crate::js::console_lines().len();
        self.transcript.retain(|(at, u, _)| u == url && *at <= len);
    }

    /// The page's console and the evaluator's transcript, merged in the order they happened.
    pub fn entries(&self) -> Vec<Entry> {
        let page = crate::js::console_lines();
        let url = crate::js::page_url();
        let mine: Vec<&(usize, String, Entry)> = self.transcript.iter().filter(|(at, u, _)| *u == url && *at <= page.len()).collect();
        let mut out = Vec::with_capacity(page.len() + mine.len());
        let mut t = mine.iter().peekable();
        for (i, (level, text)) in page.iter().enumerate() {
            while let Some((_, _, e)) = t.next_if(|(at, _, _)| *at <= i) {
                out.push(e.clone());
            }
            out.push(Entry { level: Level::from_page(*level), text: text.clone() });
        }
        out.extend(t.map(|(_, _, e)| e.clone()));
        out
    }

    /// Only the lines at or above `level`'s severity: errors (Error, Exception), warnings and errors, or
    /// everything.
    pub fn filtered(&self, min: Level) -> Vec<Entry> {
        let rank = |l: Level| match l {
            Level::Error | Level::Exception => 3,
            Level::Warn => 2,
            Level::Log | Level::Info | Level::Input | Level::Result => 1,
            Level::Debug => 0,
        };
        self.entries().into_iter().filter(|e| rank(e.level) >= rank(min)).collect()
    }
}

/// Uncaught errors the page reported (the count beside the Console tab).
pub fn error_count() -> usize {
    crate::js::reported_errors().len()
}

/// The engine's ledger of what it does not support yet (DOM, CSS, JS — security refusals included, as
/// `security-<rule>` lines), sorted by category then name.
pub fn issues() -> Vec<Issue> {
    crate::ledger::snapshot()
        .entries()
        .map(|(c, n, k)| Issue { category: c.to_string(), name: n.to_string(), count: k })
        .collect()
}
