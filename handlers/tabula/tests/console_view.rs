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

//! The Console view against REAL captured logs.
//!
//! The unit tests in `logview.rs` pin each rule with a hand-built input; these
//! open bytes that came off this bench's serial cable and FAT volume
//! (`tests/fixtures/README.md` records the provenance). A rule that only holds
//! for a synthetic input is not a rule.

use std::path::{Path, PathBuf};
use tabula::{logview, TabulaDocument};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn flight_recorder_log_opens_read_only_and_clean() {
    let path = fixture("s73-UNAOS-slice.LOG");
    let raw = std::fs::read(&path).unwrap();
    assert_eq!(raw.len(), 8192, "fixture changed");
    assert_eq!(raw.iter().filter(|&&b| b == 0).count(), 4096, "fixture lost its padding");

    // Routed as a log purely by its name.
    assert!(logview::is_log_path(&path));
    let doc = TabulaDocument::open(&path).unwrap();
    assert!(doc.read_only, "a console log must open read-only");
    assert_eq!(doc.language, "txt");
    assert_eq!(doc.path.as_deref(), Some(path.as_path()));

    // The reservation's NUL padding is gone — not one NUL, and not one
    // Control Picture standing in for one either.
    assert!(!doc.buffer.contains('\u{0}'));
    assert!(!doc.buffer.contains('\u{2400}'), "padding leaked in as ␀");
    // The buffer is exactly the 4096-byte head slice (the fixture cuts
    // mid-line there); every one of the 4096 padding bytes is gone.
    assert_eq!(
        doc.buffer.len(),
        4096,
        "the log half survives byte-for-byte; only the padding half is dropped"
    );
    assert!(doc.buffer.lines().count() > 10);

    // The real content survived, multi-byte UTF-8 included.
    assert!(doc.buffer.starts_with(":: FR-BOOT: hz="));
    assert!(doc.buffer.contains(":: UnaOS flight-recorder boot log (UNAOS.LOG) ::"));
    assert!(doc.buffer.contains("fb-wc: retyped 15 leaf(s) WC"));

    let rendered = logview::load_log(&path).unwrap();
    assert_eq!(rendered.padding_bytes, 4096);
    assert_eq!(rendered.elided_bytes, 0);
    assert!(!rendered.lossy, "the flight recorder emits valid UTF-8");
}

#[test]
fn ftdi_capture_shows_its_stray_control_byte() {
    let path = fixture("squawk-ttyUSB0-head8k.log");
    let raw = std::fs::read(&path).unwrap();
    assert_eq!(raw.iter().filter(|&&b| b == 0).count(), 1, "fixture lost its interior NUL");

    let rendered = logview::load_log(&path).unwrap();
    // Interior NUL is *shown*, not dropped and not passed through: exactly one
    // ␀, and no raw NUL anywhere in the buffer.
    assert_eq!(rendered.controls_shown, 1);
    assert_eq!(rendered.text.matches('\u{2400}').count(), 1);
    assert!(!rendered.text.contains('\u{0}'));
    assert_eq!(rendered.padding_bytes, 0);

    // Lines either side of it are intact and still line-addressable.
    assert!(rendered.text.contains("=== SQUAWK MARK 2026-08-11T16:03:55Z session-start ==="));
    assert!(rendered.text.lines().any(|l| l.contains(":: PWR: window_ms=10088")));
}

#[test]
fn a_real_log_never_writes_itself_back() {
    let path = fixture("s73-UNAOS-slice.LOG");
    let before = std::fs::read(&path).unwrap();

    let mut doc = TabulaDocument::open(&path).unwrap();
    // The editor pane fires an edit; the held document ignores it.
    doc.set_buffer("rm -rf /\n");
    assert!(!doc.dirty);
    assert!(doc.buffer.starts_with(":: FR-BOOT:"));

    // Cmd+S is refused, and the file on disk is byte-identical.
    let err = doc.save().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(std::fs::read(&path).unwrap(), before, "the log was modified");
}

#[test]
fn a_full_size_flight_recorder_reservation_loads_promptly() {
    // The real file is 256 KiB + 512 of reservation. Rebuild that shape from
    // the fixture's own head so the timing claim is about a realistic size.
    let head = std::fs::read(fixture("s73-UNAOS-slice.LOG")).unwrap();
    let mut raw = head[..4096].to_vec();
    while raw.len() < 262_656 {
        raw.push(0);
    }
    let dir = std::env::temp_dir().join(format!("tabula_console_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("UNAOS.LOG");
    std::fs::write(&path, &raw).unwrap();

    let t0 = std::time::Instant::now();
    let doc = TabulaDocument::open(&path).unwrap();
    let dt = t0.elapsed();

    assert!(doc.read_only);
    // 258 048 padding bytes cost nothing: the buffer is the 4 KiB of log.
    assert!(doc.buffer.len() < 8192, "padding reached the buffer: {} bytes", doc.buffer.len());
    assert!(dt < std::time::Duration::from_millis(250), "load took {:?}", dt);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_files_are_not_treated_as_logs() {
    // The Console view must not capture the ordinary editor path.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/logview.rs");
    let doc = TabulaDocument::open(&path).unwrap();
    assert!(!doc.read_only);
    assert_eq!(doc.language, "rust");
}
