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

//! BOOT80 (rmbp-ledger B350): READ-AHEAD sized to the medium's multi-block command.
//!
//! A cold UnaFS mount on the rMBP's SD card read every 4 KiB block as eight single-sector commands;
//! the card's cost is per COMMAND, so a 64-sector CMD18 costs about what one sector does. This module
//! holds the pure pieces both rings can test: [`window`] (which span to fetch for a miss) and
//! [`ReadAheadCache`] (one window of sectors, write-through). The kernel's SD sector device owns one
//! cache per mount and hands it a fetch closure that issues the multi-block read; the window is the
//! driver's own multi-block bound (`sdhc::MB_MAX_BLOCKS`, 64 sectors = 32 KiB = 8 UnaFS blocks).
//!
//! Coherence: the cache lives inside the ONE mount's device (the kernel's `with_unafs` rule — two live
//! mounts are already forbidden), every write through that device updates the cached copy
//! ([`ReadAheadCache::write_through`]), and a remount builds a fresh device, so a cached sector is
//! never older than the medium.

use crate::adapter::{SECTOR_SIZE, SectorError};
use alloc::vec::Vec;

/// The span `[start, start + len)` to fetch so that `[lba, lba + want)` is served, given a window of
/// `window` sectors on a device of `dev_sectors` sectors. Windows are ALIGNED to `window` (so blocks
/// read near each other share one), clamped at the device's end; when the wanted span straddles an
/// aligned boundary the window starts at `lba` instead. `None` = no read-ahead for this request (a
/// window of 0 or 1, a request at least a window long, or a span past the device) — the caller reads
/// exactly what it wants.
pub fn window(lba: u64, want: u64, window: u64, dev_sectors: u64) -> Option<(u64, u64)> {
    if window <= 1 || want == 0 || want >= window {
        return None;
    }
    let end = lba.checked_add(want)?;
    if end > dev_sectors {
        return None;
    }
    let aligned = lba - lba % window;
    let start = if end <= aligned + window { aligned } else { lba };
    let len = window.min(dev_sectors - start);
    if start + len < end {
        return None;
    }
    Some((start, len))
}

/// One window of cached sectors.
pub struct ReadAheadCache {
    window: u64,
    start: u64,
    len: u64,
    buf: Vec<u8>,
    /// Requests served from the window without touching the medium.
    pub hits: u64,
    /// Windows fetched from the medium.
    pub fills: u64,
}

impl ReadAheadCache {
    /// An empty cache whose fetches are `window` sectors (0 or 1 disables read-ahead).
    pub fn new(window: u64) -> Self {
        Self { window, start: 0, len: 0, buf: Vec::new(), hits: 0, fills: 0 }
    }

    /// The configured window, in sectors.
    pub fn window_sectors(&self) -> u64 {
        self.window
    }

    /// Drop the cached window.
    pub fn invalidate(&mut self) {
        self.len = 0;
    }

    /// Copy `[lba, lba + out.len()/512)` from the window if it is wholly cached.
    pub fn lookup(&self, lba: u64, out: &mut [u8]) -> bool {
        let n = out.len() as u64 / SECTOR_SIZE;
        if self.len == 0 || lba < self.start || lba.saturating_add(n) > self.start + self.len {
            return false;
        }
        let off = ((lba - self.start) * SECTOR_SIZE) as usize;
        out.copy_from_slice(&self.buf[off..off + out.len()]);
        true
    }

    /// A write of `data` at `lba` reached the medium: refresh every cached sector it overlaps.
    pub fn write_through(&mut self, lba: u64, data: &[u8]) {
        if self.len == 0 {
            return;
        }
        let n = data.len() as u64 / SECTOR_SIZE;
        let (ws, we) = (self.start, self.start + self.len);
        let (ds, de) = (lba, lba.saturating_add(n));
        let (s, e) = (ws.max(ds), we.min(de));
        if s >= e {
            return;
        }
        let ss = SECTOR_SIZE as usize;
        let src = ((s - ds) as usize) * ss;
        let dst = ((s - ws) as usize) * ss;
        let len = ((e - s) as usize) * ss;
        self.buf[dst..dst + len].copy_from_slice(&data[src..src + len]);
    }

    /// Serve a read of `out.len()/512` sectors at `lba` on a device of `dev_sectors` sectors: from the
    /// window when cached, else by ONE `fetch(start, buf)` of the read-ahead window (then cached), else
    /// (no window applies) by one `fetch` of exactly the request, uncached. `fetch` must fill `buf`
    /// whole or fail; a failed fetch leaves the cache empty.
    pub fn read<F>(&mut self, lba: u64, out: &mut [u8], dev_sectors: u64, mut fetch: F) -> Result<(), SectorError>
    where
        F: FnMut(u64, &mut [u8]) -> Result<(), SectorError>,
    {
        if out.len() as u64 % SECTOR_SIZE != 0 || out.is_empty() {
            return Err(SectorError::Io(alloc::format!("read span {} not whole sectors", out.len())));
        }
        if self.lookup(lba, out) {
            self.hits += 1;
            return Ok(());
        }
        let want = out.len() as u64 / SECTOR_SIZE;
        match window(lba, want, self.window, dev_sectors) {
            Some((start, len)) => {
                self.len = 0;
                let bytes = (len * SECTOR_SIZE) as usize;
                self.buf.resize(bytes, 0);
                fetch(start, &mut self.buf[..bytes])?;
                self.start = start;
                self.len = len;
                self.fills += 1;
                let hit = self.lookup(lba, out);
                debug_assert!(hit);
                Ok(())
            }
            None => fetch(lba, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_aligns_and_clamps() {
        // A 4 KiB block (8 sectors) inside an aligned 64-sector window.
        assert_eq!(window(130, 8, 64, 1 << 20), Some((128, 64)));
        // Straddling an aligned boundary: the window starts at the request.
        assert_eq!(window(124, 8, 64, 1 << 20), Some((124, 64)));
        // Clamped at the device's end.
        assert_eq!(window(1000, 8, 64, 1010), Some((960, 50)));
        // A request a window long or longer gets no read-ahead; nor does a disabled window.
        assert_eq!(window(0, 64, 64, 1 << 20), None);
        assert_eq!(window(0, 8, 1, 1 << 20), None);
        assert_eq!(window(0, 8, 0, 1 << 20), None);
        // Past the device: refused.
        assert_eq!(window(1005, 8, 64, 1010), None);
    }

    fn medium(sectors: u64) -> Vec<u8> {
        (0..sectors * SECTOR_SIZE).map(|i| (i / SECTOR_SIZE) as u8 ^ (i as u8)).collect()
    }

    #[test]
    fn eight_blocks_cost_one_fetch() {
        let disk = medium(4096);
        let mut c = ReadAheadCache::new(64);
        let mut fetches = 0u32;
        let mut out = alloc::vec![0u8; 4096];
        for blk in 0..8u64 {
            let lba = 64 + blk * 8;
            c.read(lba, &mut out, 4096, |s, b| {
                fetches += 1;
                b.copy_from_slice(&disk[(s * 512) as usize..(s * 512) as usize + b.len()]);
                Ok(())
            })
            .unwrap();
            assert_eq!(&out[..], &disk[(lba * 512) as usize..(lba * 512) as usize + 4096]);
        }
        assert_eq!(fetches, 1);
        assert_eq!((c.fills, c.hits), (1, 7));
    }

    #[test]
    fn write_through_keeps_the_window_true() {
        let mut disk = medium(256);
        let mut c = ReadAheadCache::new(64);
        let mut out = alloc::vec![0u8; 512];
        c.read(3, &mut out, 256, |s, b| {
            b.copy_from_slice(&disk[(s * 512) as usize..(s * 512) as usize + b.len()]);
            Ok(())
        })
        .unwrap();
        // A two-sector write straddling the window's end (sectors 63..65): the cached half refreshes.
        let data = alloc::vec![0xAB; 1024];
        disk[63 * 512..65 * 512].copy_from_slice(&data);
        c.write_through(63, &data);
        c.read(63, &mut out, 256, |_, _| panic!("must be a hit")).unwrap();
        assert!(out.iter().all(|&b| b == 0xAB));
        // Outside the window: a miss that fetches the medium's (new) bytes.
        let mut fetched = false;
        c.read(64, &mut out, 256, |s, b| {
            fetched = true;
            b.copy_from_slice(&disk[(s * 512) as usize..(s * 512) as usize + b.len()]);
            Ok(())
        })
        .unwrap();
        assert!(fetched && out.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn failed_fetch_caches_nothing() {
        let mut c = ReadAheadCache::new(64);
        let mut out = alloc::vec![0u8; 512];
        assert!(c.read(0, &mut out, 256, |_, _| Err(SectorError::Io("boom".into()))).is_err());
        assert!(!c.lookup(0, &mut out));
    }
}
