// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — fulfiller
//!
//! SETTINGSFILES (rmbp-ledger B407, R98) — the Settings panel's and Quarry's view of Principia's settings
//! FILES (`<home>/settings/<domain>`, `crate::prefs`). Two things, both read-only over the store:
//!
//! - the panel's one line per pane, `Stored in settings/<domain>`, drawn as a link; a press LATCHES a reveal
//!   (the click router never reads a directory) and [`service`] — chained from the settings pass — opens
//!   Quarry AT `<home>/settings` (`quarry::live::open_at`, DOCK2's door);
//! - Quarry's Show Info on a settings file: [`info_lines`] answers the auto-saved line and every key with
//!   its value. ATTRCOLUMNS (B402) builds the Get Info inspector in `video/quarry/getinfo.rs`; it joins this
//!   function at the merge (this file stays the settings-file pane, that one the inspector).
//!
//! Witness: `[settingsfiles] reveal dir=<home>/settings opened=<0|1> from=<tab>`; `[settingsfiles] info
//! path=<p> keys=<n> saved=<iso>`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

/// The settings tabs (`settings::TAB_NAMES` order) and the domain files their panes write.
fn domains_of(tab: usize) -> Option<&'static str> {
    match tab {
        0 => Some("sound, trackpad, desktop, general"),
        2 => Some("display"),
        4 => Some("login"),
        #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
        t if t == super::settings::NP_TAB => Some("notify"), // NOTIFYPANE (B435)
        _ => None,
    }
}

/// The footer link of a tab: `(row, x, text)` in the window's logical layout. The Login Items pane uses
/// every row, so its line sits at the right of its title row.
pub fn footer(tab: usize, rows: usize, win_w: usize, label_x: usize) -> Option<(usize, usize, String)> {
    let d = domains_of(tab)?;
    let t = alloc::format!("Stored in settings/{} - show in Quarry", d);
    Some(match tab {
        4 => (0, win_w.saturating_sub(200), alloc::format!("Stored in settings/{}", d)),
        _ => (rows - 1, label_x, t),
    })
}

/// 0 = none latched, else the tab + 1 whose link was pressed.
static REVEAL: AtomicU8 = AtomicU8::new(0);

/// The press router: `true` = the press was the footer link (a reveal is latched for [`service`]).
pub fn press(tab: usize, row: usize, cx: usize, rows: usize, win_w: usize, label_x: usize) -> bool {
    let Some((r, x, _)) = footer(tab, rows, win_w, label_x) else { return false };
    if row != r || cx < x {
        return false;
    }
    REVEAL.store(tab as u8 + 1, Ordering::Release);
    true
}

/// Drain a latched reveal: open Quarry at `<home>/settings`. Chained from `settings::service`.
pub fn service() {
    let t = REVEAL.swap(0, Ordering::AcqRel);
    if t == 0 {
        return;
    }
    let dir = crate::prefs::path();
    #[cfg(feature = "quarry")]
    let opened = crate::video::quarry::live::open_at(&dir);
    #[cfg(not(feature = "quarry"))]
    let opened = false;
    serial_println!("[settingsfiles] reveal dir={} opened={} from=tab{} (R98)", dir, opened as u8, t - 1);
}

/// Show Info lines for `path` when it is a settings file: `auto-saved <iso> by <who>` then `<ns>.<key> =
/// <value>` per key (a refused file says so). Empty = not a settings file.
pub fn info_lines(path: &str) -> Vec<String> {
    let dir = crate::prefs::path();
    let Some(name) = path.strip_prefix(dir.as_str()).and_then(|r| r.strip_prefix('/')) else { return Vec::new() };
    if name.contains('/') || !prefs_core::files::valid_domain(name) {
        return Vec::new();
    }
    let mt = crate::shell::vfs_mount_table();
    let Ok(st) = mt.stat(path) else { return Vec::new() };
    let Some(text) = mt.read(path, 0, (st.size as usize).min(64 * 1024)).ok().and_then(|b| String::from_utf8(b).ok()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let saved = prefs_core::files::stamp_of(&text);
    out.push(match saved {
        Some((iso, by)) => alloc::format!("auto-saved {} by {}", iso, by),
        None => String::from("settings file (no auto-saved line: edited by hand)"),
    });
    let n = match prefs_core::PrefTree::parse(&text) {
        Ok(t) => {
            let l = prefs_core::files::key_lines(&t);
            let n = l.len();
            out.extend(l);
            n
        }
        Err(e) => {
            out.push(alloc::format!("refused at line {}: {} (its saves are held)", e.line, e.why));
            0
        }
    };
    serial_println!("[settingsfiles] info path={} keys={} saved={}", path, n, saved.map(|s| s.0).unwrap_or("-"));
    out
}
