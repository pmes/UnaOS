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
