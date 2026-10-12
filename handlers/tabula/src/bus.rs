// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! TABULABUS (SR74) — Tabula on the Synapse.
//!
//! [`ignite`] is the handler's entry point in the Facet/Matrix shape: a task subscribed to the
//! Synapse that owns every document it opens, keyed by the path the opener named, and serves the
//! [`TabulaCommand`] verbs (`EditorOpen`/`Save`/`Goto`/`Insert`, the vessel's `EditorClosed` fact).
//! The documents are today's [`TabulaDocument`] — `open` (which name-routes a console log to the
//! read-only Console view), `set_buffer` and `save` — so the log treatment and the read-only
//! refusal are the document core's, not re-decided here.
//!
//! The pane vocabulary that predates the verbs keeps working:
//! - after every `EditorLoaded` (and after a bus `EditorInsert` into the pane's document) Tabula
//!   fires [`SMessage::EditorLoad`], the message quartzite's editor pane renders;
//! - [`SMessage::EditorEdited`] (the view's full-buffer echo, which carries no path) is applied to
//!   the document Tabula last loaded — "the pane's document".
//!
//! [`Tabula::dispatch`] is the whole behaviour, pure over the held documents and the filesystem;
//! [`serve`] only wires it to the bus.

use std::collections::HashMap;
use std::future::Future;

use bandy::signals::{SMessage, TabulaCommand};
use bandy::Synapse;

use crate::document::TabulaDocument;

/// One held document and its cursor (a byte offset into `doc.buffer`, always on a char boundary
/// when used).
#[derive(Debug, Clone)]
struct Held {
    doc: TabulaDocument,
    cursor: usize,
}

/// The documents Tabula owns, keyed by path, and which one is in the pane.
#[derive(Debug, Default)]
pub struct Tabula {
    docs: HashMap<String, Held>,
    active: Option<String>,
}

impl Tabula {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `path` is held open.
    pub fn is_open(&self, path: &str) -> bool {
        self.docs.contains_key(path)
    }

    /// The held document for `path`, if open.
    pub fn document(&self, path: &str) -> Option<&TabulaDocument> {
        self.docs.get(path).map(|h| &h.doc)
    }

    /// The path of the pane's document (the one last loaded).
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Serve one bus message; returns what Tabula fires in answer, in order. Tabula's own answers
    /// and every other channel's traffic answer nothing.
    pub fn dispatch(&mut self, msg: &SMessage) -> Vec<SMessage> {
        match msg {
            SMessage::Tabula(cmd) if cmd.is_request() => self.serve_cmd(cmd),
            SMessage::EditorEdited { content } => self.edited(content),
            _ => Vec::new(),
        }
    }

    fn serve_cmd(&mut self, cmd: &TabulaCommand) -> Vec<SMessage> {
        match cmd {
            TabulaCommand::EditorOpen { path, .. } => self.open(path),
            TabulaCommand::EditorSave { path, .. } => self.save(path),
            TabulaCommand::EditorGoto { path, line, col, .. } => match self.docs.get_mut(path) {
                Some(h) => {
                    h.cursor = offset_of(&h.doc.buffer, *line, *col);
                    Vec::new()
                }
                None => vec![error(path, "not open")],
            },
            TabulaCommand::EditorInsert { path, text, .. } => self.insert(path, text),
            TabulaCommand::EditorClosed { path } => {
                self.docs.remove(path);
                if self.active.as_deref() == Some(path.as_str()) {
                    self.active = None;
                }
                Vec::new()
            }
            // Answers are inert as input (`is_request` filtered them already).
            _ => Vec::new(),
        }
    }

    fn open(&mut self, path: &str) -> Vec<SMessage> {
        if !self.docs.contains_key(path) {
            match TabulaDocument::open(path) {
                Ok(doc) => {
                    self.docs.insert(path.to_string(), Held { doc, cursor: 0 });
                }
                Err(e) => return vec![error(path, &e.to_string())],
            }
        }
        self.active = Some(path.to_string());
        let doc = &self.docs[path].doc;
        vec![
            SMessage::Tabula(TabulaCommand::EditorLoaded {
                path: path.to_string(),
                content: doc.buffer.clone(),
                lines: u32::try_from(doc.buffer.lines().count()).unwrap_or(u32::MAX),
                language: doc.language.clone(),
                read_only: doc.read_only,
            }),
            pane_load(path, doc),
        ]
    }

    fn save(&mut self, path: &str) -> Vec<SMessage> {
        let Some(h) = self.docs.get_mut(path) else {
            return vec![error(path, "not open")];
        };
        let was_dirty = h.doc.dirty;
        match h.doc.save() {
            Ok(()) => {
                let mut out = vec![SMessage::Tabula(TabulaCommand::EditorSaved { path: path.to_string() })];
                if was_dirty {
                    out.push(dirty(path, false));
                }
                out
            }
            Err(e) => vec![error(path, &e.to_string())],
        }
    }

    fn insert(&mut self, path: &str, text: &str) -> Vec<SMessage> {
        let Some(h) = self.docs.get_mut(path) else {
            return vec![error(path, "not open")];
        };
        if h.doc.read_only {
            return vec![error(path, "document is read-only (console log view)")];
        }
        let at = floor_boundary(&h.doc.buffer, h.cursor);
        let was_dirty = h.doc.dirty;
        let mut next = String::with_capacity(h.doc.buffer.len() + text.len());
        next.push_str(&h.doc.buffer[..at]);
        next.push_str(text);
        next.push_str(&h.doc.buffer[at..]);
        h.doc.set_buffer(next);
        h.cursor = at + text.len();
        let mut out = Vec::new();
        if !was_dirty {
            out.push(dirty(path, true));
        }
        if self.active.as_deref() == Some(path) {
            out.push(pane_load(path, &self.docs[path].doc));
        }
        out
    }

    /// The view's full-buffer echo, for the pane's document. No `EditorLoad` back — the view
    /// already shows this text, and pushing it again would fight the typist.
    fn edited(&mut self, content: &str) -> Vec<SMessage> {
        let Some(path) = self.active.clone() else {
            return Vec::new();
        };
        let Some(h) = self.docs.get_mut(&path) else {
            return Vec::new();
        };
        if h.doc.read_only {
            return Vec::new();
        }
        let was_dirty = h.doc.dirty;
        h.doc.set_buffer(content);
        h.cursor = floor_boundary(&h.doc.buffer, h.cursor);
        if was_dirty { Vec::new() } else { vec![dirty(&path, true)] }
    }
}

fn error(path: &str, why: &str) -> SMessage {
    SMessage::Tabula(TabulaCommand::EditorError { path: path.to_string(), why: why.to_string() })
}

fn dirty(path: &str, dirty: bool) -> SMessage {
    SMessage::Tabula(TabulaCommand::EditorDirty { path: path.to_string(), dirty })
}

fn pane_load(path: &str, doc: &TabulaDocument) -> SMessage {
    SMessage::EditorLoad {
        path: Some(path.to_string()),
        content: doc.buffer.clone(),
        language: doc.language.clone(),
    }
}

/// The largest char boundary `<= at` (and `<= text.len()`).
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The byte offset of 1-based `line`, `col` (col in chars), clamped to the text: line 0 and col 0
/// read as 1, a line past the end is the last line, a col past the end of its line is the line's
/// end (before its `\n` / `\r\n`).
pub fn offset_of(text: &str, line: u32, col: u32) -> usize {
    let want_line = line.max(1) as usize;
    let want_col = col.max(1) as usize;
    let mut start = 0usize;
    let mut body = text;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        start = l.as_ptr() as usize - text.as_ptr() as usize;
        body = l;
        if i + 1 == want_line {
            break;
        }
    }
    let body = body.strip_suffix('\n').unwrap_or(body);
    let body = body.strip_suffix('\r').unwrap_or(body);
    let into = body.char_indices().nth(want_col - 1).map(|(b, _)| b).unwrap_or(body.len());
    start + into
}

/// Subscribe to `synapse` NOW (so nothing fired after this call returns is missed) and return the
/// serving loop. A vessel that fires `EditorOpen` right after spawning calls this, then spawns.
pub fn serve(synapse: Synapse) -> impl Future<Output = ()> + Send + 'static {
    let mut rx = synapse.subscribe();
    async move {
        let mut tabula = Tabula::new();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    for out in tabula.dispatch(&msg) {
                        synapse.fire(out);
                    }
                }
                Err(e) => {
                    let why = e.to_string();
                    if why.contains("lagged") {
                        // A dropped request is a caller waiting forever on its answer; say so.
                        eprintln!("[TABULA] :: {why} behind the Synapse");
                    } else {
                        eprintln!("[TABULA] :: Synapse closed; Tabula terminating.");
                        break;
                    }
                }
            }
        }
    }
}

/// Tabula's handler entry point: serve the Editor verbs on `synapse` until it closes.
pub async fn ignite(synapse: Synapse) {
    serve(synapse).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use bandy::Origin;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tabula_bus_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn me() -> Origin {
        Origin::LocalUser("una".into())
    }

    /// The next message on `rx` that is not a request (what Tabula answered).
    async fn answer(rx: &mut tokio::sync::broadcast::Receiver<SMessage>) -> SMessage {
        loop {
            let m = rx.recv().await.expect("bus open");
            match &m {
                SMessage::Tabula(c) if c.is_request() => continue,
                SMessage::EditorEdited { .. } => continue,
                _ => return m,
            }
        }
    }

    fn tc(m: SMessage) -> TabulaCommand {
        match m {
            SMessage::Tabula(c) => c,
            other => panic!("expected a Tabula answer, got {other:?}"),
        }
    }

    /// THE BUS KAT: open → loaded (+ the pane's EditorLoad) → goto → insert → dirty → save →
    /// saved (+ dirty false), over a real Synapse with `ignite`'s serving loop.
    #[tokio::test(flavor = "current_thread")]
    async fn bus_open_insert_dirty_save_saved() {
        let d = scratch("kat");
        let f = d.join("x.rs");
        std::fs::write(&f, "fn main() {\n}\n").unwrap();
        let p = f.to_string_lossy().into_owned();

        let syn = Synapse::new();
        let mut rx = syn.subscribe();
        let task = tokio::spawn(serve(syn.clone()));

        syn.fire(SMessage::Tabula(TabulaCommand::EditorOpen { principal: me(), path: p.clone() }));
        match tc(answer(&mut rx).await) {
            TabulaCommand::EditorLoaded { path, content, lines, language, read_only } => {
                assert_eq!(path, p);
                assert_eq!(content, "fn main() {\n}\n");
                assert_eq!(lines, 2);
                assert_eq!(language, "rust");
                assert!(!read_only);
            }
            other => panic!("expected EditorLoaded, got {other:?}"),
        }
        match answer(&mut rx).await {
            SMessage::EditorLoad { path, content, language } => {
                assert_eq!(path.as_deref(), Some(p.as_str()));
                assert_eq!(content, "fn main() {\n}\n");
                assert_eq!(language, "rust");
            }
            other => panic!("expected the pane's EditorLoad, got {other:?}"),
        }

        // Line 2, col 1 is the closing brace: insert before it.
        syn.fire(SMessage::Tabula(TabulaCommand::EditorGoto { principal: me(), path: p.clone(), line: 2, col: 1 }));
        syn.fire(SMessage::Tabula(TabulaCommand::EditorInsert { principal: me(), path: p.clone(), text: "    run();\n".into() }));
        match tc(answer(&mut rx).await) {
            TabulaCommand::EditorDirty { path, dirty } => {
                assert_eq!(path, p);
                assert!(dirty);
            }
            other => panic!("expected EditorDirty true, got {other:?}"),
        }
        match answer(&mut rx).await {
            SMessage::EditorLoad { content, .. } => assert_eq!(content, "fn main() {\n    run();\n}\n"),
            other => panic!("expected the pane refresh, got {other:?}"),
        }

        syn.fire(SMessage::Tabula(TabulaCommand::EditorSave { principal: me(), path: p.clone() }));
        match tc(answer(&mut rx).await) {
            TabulaCommand::EditorSaved { path } => assert_eq!(path, p),
            other => panic!("expected EditorSaved, got {other:?}"),
        }
        match tc(answer(&mut rx).await) {
            TabulaCommand::EditorDirty { dirty, .. } => assert!(!dirty),
            other => panic!("expected EditorDirty false, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "fn main() {\n    run();\n}\n");

        task.abort();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An unreadable path (missing, or a directory) answers EditorError, never silence.
    #[tokio::test(flavor = "current_thread")]
    async fn bus_unreadable_path_is_an_error() {
        let d = scratch("unreadable");
        let missing = d.join("nope.rs").to_string_lossy().into_owned();
        let dir = d.to_string_lossy().into_owned();

        let syn = Synapse::new();
        let mut rx = syn.subscribe();
        let task = tokio::spawn(serve(syn.clone()));

        for p in [&missing, &dir] {
            syn.fire(SMessage::Tabula(TabulaCommand::EditorOpen { principal: me(), path: p.clone() }));
            match tc(answer(&mut rx).await) {
                TabulaCommand::EditorError { path, why } => {
                    assert_eq!(&path, p);
                    assert!(!why.is_empty());
                }
                other => panic!("expected EditorError for {p}, got {other:?}"),
            }
        }
        task.abort();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A console log opens through the name route: loaded read-only, padding trimmed; an insert
    /// and a save both answer EditorError and the file is untouched.
    #[tokio::test(flavor = "current_thread")]
    async fn bus_log_loads_read_only() {
        let d = scratch("log");
        let f = d.join("UNAOS.LOG");
        let mut bytes = b":: boot ::\n".to_vec();
        bytes.extend(std::iter::repeat(0u8).take(64));
        std::fs::write(&f, &bytes).unwrap();
        let p = f.to_string_lossy().into_owned();

        let syn = Synapse::new();
        let mut rx = syn.subscribe();
        let task = tokio::spawn(serve(syn.clone()));

        syn.fire(SMessage::Tabula(TabulaCommand::EditorOpen { principal: me(), path: p.clone() }));
        match tc(answer(&mut rx).await) {
            TabulaCommand::EditorLoaded { content, read_only, language, .. } => {
                assert!(read_only, "a log is read-only");
                assert_eq!(language, "txt");
                assert!(content.starts_with(":: boot ::"));
                assert!(!content.contains('\0'), "NUL padding is trimmed");
            }
            other => panic!("expected EditorLoaded, got {other:?}"),
        }
        assert!(matches!(answer(&mut rx).await, SMessage::EditorLoad { .. }));

        syn.fire(SMessage::Tabula(TabulaCommand::EditorInsert { principal: me(), path: p.clone(), text: "x".into() }));
        assert!(matches!(tc(answer(&mut rx).await), TabulaCommand::EditorError { .. }));
        syn.fire(SMessage::Tabula(TabulaCommand::EditorSave { principal: me(), path: p.clone() }));
        assert!(matches!(tc(answer(&mut rx).await), TabulaCommand::EditorError { .. }));
        assert_eq!(std::fs::read(&f).unwrap(), bytes, "the record is untouched");

        task.abort();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The view's legacy full-buffer echo lands on the pane's document; close drops it; verbs on a
    /// closed path answer EditorError; Tabula's own answers are inert as input.
    #[test]
    fn edited_close_and_inert_answers() {
        let d = scratch("edited");
        let f = d.join("n.txt");
        std::fs::write(&f, "a\n").unwrap();
        let p = f.to_string_lossy().into_owned();

        let mut t = Tabula::new();
        assert_eq!(t.dispatch(&SMessage::EditorEdited { content: "z".into() }).len(), 0, "no pane document yet");
        t.dispatch(&SMessage::Tabula(TabulaCommand::EditorOpen { principal: me(), path: p.clone() }));
        assert_eq!(t.active(), Some(p.as_str()));
        let out = t.dispatch(&SMessage::EditorEdited { content: "b\n".into() });
        assert!(matches!(&out[..], [SMessage::Tabula(TabulaCommand::EditorDirty { dirty: true, .. })]));
        assert_eq!(t.document(&p).unwrap().buffer, "b\n");
        assert!(t.dispatch(&SMessage::EditorEdited { content: "c\n".into() }).is_empty(), "already dirty");

        // Re-opening a held document re-presents it, edits included.
        let out = t.dispatch(&SMessage::Tabula(TabulaCommand::EditorOpen { principal: me(), path: p.clone() }));
        assert!(matches!(&out[0], SMessage::Tabula(TabulaCommand::EditorLoaded { content, .. }) if content == "c\n"));

        // Answers are inert.
        assert!(t.dispatch(&SMessage::Tabula(TabulaCommand::EditorSaved { path: p.clone() })).is_empty());

        t.dispatch(&SMessage::Tabula(TabulaCommand::EditorClosed { path: p.clone() }));
        assert!(!t.is_open(&p));
        assert_eq!(t.active(), None);
        for req in [
            TabulaCommand::EditorSave { principal: me(), path: p.clone() },
            TabulaCommand::EditorGoto { principal: me(), path: p.clone(), line: 1, col: 1 },
            TabulaCommand::EditorInsert { principal: me(), path: p.clone(), text: "x".into() },
        ] {
            let out = t.dispatch(&SMessage::Tabula(req));
            assert!(matches!(&out[..], [SMessage::Tabula(TabulaCommand::EditorError { .. })]));
        }
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a\n", "closing never writes");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn goto_offsets_are_one_based_and_clamped() {
        let t = "ab\r\nçd\n\nlast";
        assert_eq!(offset_of(t, 1, 1), 0);
        assert_eq!(offset_of(t, 0, 0), 0);
        assert_eq!(offset_of(t, 1, 99), 2, "before the CRLF");
        assert_eq!(offset_of(t, 2, 2), 4 + 'ç'.len_utf8(), "col counts chars");
        assert_eq!(offset_of(t, 3, 5), 8, "an empty line");
        assert_eq!(offset_of(t, 4, 3), 11);
        assert_eq!(offset_of(t, 99, 99), t.len(), "past the end is the end");
        assert_eq!(offset_of("", 3, 3), 0);
    }
}
