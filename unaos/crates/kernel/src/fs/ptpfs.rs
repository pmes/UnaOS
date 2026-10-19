//! CHARTER: Matrix — fs-core
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! PTPCAM M2 (rmbp-ledger B687, R130 law 3) — a camera's card as a READ-ONLY volume at
//! `/volumes/<model>`, through the mount table like every other volume (BP-storage-1: the table is the
//! one name authority). Registered with [`super::register_hotplug`] when the PTP session opens, so every
//! table built while the camera is attached carries it — `ls /volumes`, Quarry and SYS_OPEN see one
//! namespace — and gone from the next table the instant the slot disconnects.
//!
//! * A LISTING never touches the cable: it reads the driver's object table, which the xHCI service pass
//!   fills a chunk per pass (`drivers/xhci/ptp.rs` `ptp_fill`). A card still being listed shows what is
//!   read so far, and its rows complete on later listings (the namespace generation bumps when it does).
//! * A READ is the cable: GetPartialObject for exactly the range when the body has it, else GetObject of
//!   the whole object, kept for the next range (`ptp_read_object`). An ARW read this way opens in Facet.
//! * `LIVEVIEW.JPG` (a Sony body in PC-Remote mode): every open-and-read is a fresh frame — `stat` fetches
//!   one when the cached frame is older than 1 s, a read at offset 0 when it is older than 300 ms, and a
//!   read past 0 serves the frame that read 0 returned.
//! * Posture: world-readable (the card is the user's), `write_veto` names the volume read-only.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::{volid_mix, DirEnt, NodeKind, Stat, VfsBackend, VfsError, VfsTime, VOLID_SEED};
use crate::drivers::xhci::ptp;

pub struct PtpFs {
    name: String,
}

/// The hot-plug provider: the camera's mount while a session is open.
pub fn provide() -> Option<(String, Box<dyn VfsBackend>)> {
    let name = ptp::volume_name()?;
    Some((alloc::format!("/volumes/{}", name), Box::new(PtpFs { name })))
}

fn vfs_time(t: &ptp::codec::DateTime) -> VfsTime {
    VfsTime { year: t.y, month: t.mo, day: t.d, hour: t.h, min: t.mi, sec: t.s }
}

impl VfsBackend for PtpFs {
    fn volume_name(&self) -> &str {
        &self.name
    }

    fn volume_id(&self) -> Option<u64> {
        let (_, ident) = ptp::describe()?;
        Some(volid_mix(volid_mix(VOLID_SEED, b"vfs:ptp:"), ident.as_bytes()))
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        match ptp::lookup(rel).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::Dir { storage, handle } => Ok(ptp::list(storage, handle)
                .into_iter()
                .map(|r| DirEnt {
                    name: r.name,
                    kind: if r.dir { NodeKind::Dir } else { NodeKind::File },
                    size: r.size,
                    mtime: r.mtime.as_ref().map(vfs_time),
                })
                .collect()),
            _ => Err(VfsError::NotADirectory),
        }
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        match ptp::lookup(rel).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::Dir { handle, .. } => Ok(Stat { kind: NodeKind::Dir, size: 0, id: handle.map(|h| h as u64), mtime: None }),
            ptp::Node::File(o) => Ok(Stat {
                kind: NodeKind::File,
                size: o.size,
                id: Some(o.handle as u64),
                mtime: o.mtime.as_ref().map(ptp::codec::unix_secs),
            }),
            ptp::Node::LiveView => {
                let f = ptp::liveview_frame(1000).map_err(VfsError::Backend)?;
                Ok(Stat { kind: NodeKind::File, size: f.len() as u64, id: None, mtime: None })
            }
        }
    }

    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        match ptp::lookup(rel).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::Dir { .. } => Err(VfsError::IsADirectory),
            ptp::Node::File(o) => ptp::read_object(o.handle, o.size, offset, len).map_err(VfsError::Backend),
            ptp::Node::LiveView => {
                let f = if offset == 0 {
                    ptp::liveview_frame(300).map_err(VfsError::Backend)?
                } else {
                    ptp::liveview_cached().ok_or(VfsError::Backend("live view: no frame read at offset 0"))?
                };
                let s = (offset as usize).min(f.len());
                Ok(f[s..s.saturating_add(len).min(f.len())].to_vec()) // crawl B 25: `len` near usize::MAX never overflows the slice end
            }
        }
    }

    fn authorize_read(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Ok(()) // world-readable: the card is the user's (R130 law 3), as a USB stick's FAT is
    }

    fn write_veto(&self) -> Option<&'static str> {
        Some("camera volume (PTP) is read-only")
    }

    fn describe(&self) -> Option<String> {
        ptp::describe().map(|(d, _)| d)
    }
}
