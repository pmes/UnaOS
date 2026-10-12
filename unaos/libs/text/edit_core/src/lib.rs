// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EDITCORE (LEDGER SR73) — UnaOS's own text-editing core.
//!
//! CHARTER: Tabula owns Text (docs/CODEX.md §2); seam `shared-core` (R104: this core is INSERTED beside the
//! GtkSourceView face, which stays as the other-OS port; the host Tabula and a ring-3 editor link the same bytes).
//!
//! `no_std` + `alloc`, zero dependencies, no `unsafe`. What is here:
//!
//! * [`Text`] — a **piece table** over UTF-8 with an incrementally maintained line index (line → byte).
//! * [`Buffer`] — the editor state of record: the text, selections / multiple cursors, grouped undo/redo,
//!   edit operations (insert, delete by grapheme / word / line, replace, newline with auto-indent,
//!   indent / outdent), search, and a soft-wrapping [`Viewport`]. Every edit returns [`Change`]s carrying the
//!   changed line range and the new text so a view re-renders only that range.
//! * [`unicode`] — UAX #29 extended grapheme clusters and word boundaries over generated UCD 17.0.0 tables.
//!
//! ## Piece table, not rope — the choice and why
//!
//! The operations an IDE pane performs are: load a file once, insert/delete at a few cursors (overwhelmingly
//! next to where the previous edit happened), undo/redo those edits, map line ↔ byte for every repaint, and
//! search. A piece table fits that profile better than a rope for this core:
//!
//! * **Load is free.** The loaded file becomes the immutable *original* buffer and one piece; nothing is
//!   copied into nodes or rebalanced. A rope pays O(n) node construction before the first frame.
//! * **Edits are append-only.** Inserted text goes to the end of the *add* buffer and is never moved or
//!   freed, so a piece is three words and typing at one spot extends a single piece in place (no
//!   fragmentation for the common case). The undo log can therefore hold plain `(offset, deleted, inserted)`
//!   records — nothing a later edit can invalidate.
//! * **Simplicity is a correctness property here.** The kernel-side consumer links this code; a piece list
//!   with two buffers has a tiny invariant (pieces non-empty, lengths sum to `len`) that the fuzz checks on
//!   every operation, against a `Vec<char>` model. A balanced rope's rebalancing has far more states to get
//!   wrong for no gain at source-file sizes.
//! * **The rope's advantage — O(log n) edits on huge, heavily fragmented documents — is not this workload.**
//!   Piece lookup here is a linear walk of the piece list (pieces ≪ bytes: one per edit site, coalesced
//!   while typing), and line ↔ byte never walks pieces at all: the line index is a sorted `Vec` of line
//!   starts, binary-searched, updated incrementally per edit (shift the tail, splice in the new starts).
//!
//! Positions are byte offsets internally and [`Pos`] `(line, byte-in-line)` at the API edge. Lines end at
//! LF; a CR before the LF belongs to the line ending, and each line keeps the ending it was loaded with
//! (CRLF preserved per line; [`Buffer::newline`] repeats the current line's ending).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod buffer;
pub mod search;
mod tables;
mod text;
pub mod unicode;
pub mod viewport;

pub use buffer::{Buffer, Change, Direction, IndentStyle, Motion, Selection, Unit};
pub use search::SearchOpts;
pub use text::{LineEnding, Pos, Text};
pub use viewport::{Viewport, VisualRow};

/// The Unicode version the generated tables were cut from.
pub const UNICODE_VERSION: (u8, u8, u8) = tables::UNICODE_VERSION;
