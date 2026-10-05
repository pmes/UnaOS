// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The API key file. Never in the image, never on the FAT volume in clear: the file is read ONLY when
//! SYS_STAT says it carries an inode id (`STAT_HAS_ID` — the native UnaFS backend; FAT has no ids, no
//! owner and no mode, so a key there is readable by anything that mounts the card and is refused).

use crate::sys::sys;
use una_abi::{ENOENT, STAT_HAS_ID, SYS_CLOSE, SYS_OPEN, SYS_READ, SYS_STAT, USER_STAT_LEN};
use vein_core::prefs::KeyState;

/// Read the key at `path` into `buf`. `(state, bytes)`; bytes > 0 only for [`KeyState::UnaFs`].
pub fn read(path: Option<&str>, buf: &mut [u8]) -> (KeyState, usize) {
    let Some(path) = path else { return (KeyState::None, 0) };
    let mut st = [0u8; USER_STAT_LEN];
    let r = sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
    if r == ENOENT || r < 0 {
        return (KeyState::None, 0);
    }
    let flags = u32::from_le_bytes([st[4], st[5], st[6], st[7]]);
    if flags & STAT_HAS_ID == 0 {
        return (KeyState::OnFat, 0);
    }
    let h = sys(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if h < 0 {
        return (KeyState::None, 0);
    }
    let mut n = 0;
    while n < buf.len() {
        let k = sys(SYS_READ, h as u64, buf[n..].as_mut_ptr() as u64, (buf.len() - n) as u64, 0);
        if k <= 0 {
            break;
        }
        n += k as usize;
    }
    sys(SYS_CLOSE, h as u64, 0, 0, 0);
    if n == 0 { (KeyState::None, 0) } else { (KeyState::UnaFs, n) }
}

/// RING3ABI2 M3 (rmbp-ledger B333): the key file's conventional path, `<home>/.config/unaos/vein.key`, for
/// the user this program runs as — `<home>` from `SYS_WHOAMI` (the users store's record, never a
/// `/home/<name>` literal). Written into `buf`; `None` when the program runs anonymously (no session:
/// `-ENOENT`), the kernel predates the verb, or the path does not fit. `vein.key_file` (Principia's
/// preference) still overrides it — this is only the default when the preference is unset.
pub fn default_path(buf: &mut [u8]) -> Option<&str> {
    const TAIL: &[u8] = b"/.config/unaos/vein.key";
    let mut rec = [0u8; una_abi::WHOAMI_MAX];
    let r = sys(una_abi::SYS_WHOAMI, rec.as_mut_ptr() as u64, rec.len() as u64, 0, 0);
    if r <= 0 {
        return None;
    }
    let w = una_abi::whoami_parse(&rec[..(r as usize).min(rec.len())])?;
    let home = w.home.strip_suffix(b"/").unwrap_or(w.home);
    if home.first() != Some(&b'/') {
        return None;
    }
    let n = home.len() + TAIL.len();
    let out = buf.get_mut(..n)?;
    out[..home.len()].copy_from_slice(home);
    out[home.len()..].copy_from_slice(TAIL);
    core::str::from_utf8(&buf[..n]).ok()
}
