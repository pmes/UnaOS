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
//!
//! MTPWRITE (rmbp-ledger B727, BP-storage-23): an MTP session (a phone, `ptp::is_mtp_session`) takes writes;
//! a PTP camera keeps the veto. MTP cannot append to an object (SendObjectInfo announces the size,
//! SendObject carries all of it) and the VFS has no close verb, so ONE pending file is held in RAM here:
//! * `create(File)` holds an empty buffer (listed at once); `write` fills it (no holes; off 0 on a file
//!   already on the phone opens a rewrite of it).
//! * A SEND (`mtp::write_file`: SendObjectInfo + SendObject of the whole buffer, once per send; a file
//!   sent before has its earlier object deleted first) happens on CLOSE — a stat or read of that path, a
//!   write shorter than the one before it (the tail of a chunked copy), any other mutation on the volume —
//!   and on the 4 MiB chunk boundaries 4, 8, 16 … MiB (doubling, so the cable carries ≤ ~2x the file).
//!   A listing never sends; it shows the pending row. A pending file is capped at [`PENDING_CAP`].
//! * `create(Dir)` = `mtp::make_folder` (SendObjectInfo, format 0x3001); `unlink` / `remove_dir` (empty
//!   folders) = `mtp::delete`. A read of an MTP file past 4 GiB is GetPartialObject64 (`mtp::read_object`).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::{volid_mix, DirEnt, NodeKind, Stat, VfsBackend, VfsError, VfsTime, VOLID_SEED};
use crate::drivers::xhci::ptp;
use crate::drivers::xhci::ptp::mtp;
use crate::sync::Mutex;

/// The veto a PTP camera's volume answers (unchanged since PTPCAM).
const READ_ONLY: &str = "camera volume (PTP) is read-only";
/// The first send boundary of a pending file; each later one doubles it.
const MARK0: usize = 4 << 20;
/// A pending file lives in RAM until sent: past this a write is refused by name (streaming is owed).
pub const PENDING_CAP: usize = 256 << 20;

/// MTPWRITE: the one file being written onto the phone.
struct Pending {
    /// `ptp::describe`'s identity of the session it belongs to (another session drops it).
    ident: String,
    rel: String,
    dir: String,
    leaf: String,
    buf: Vec<u8>,
    /// The length of the last write (0 = none yet): a shorter write is a copy loop's tail.
    last: usize,
    /// The object already on the phone for this path (sent earlier, or the file a rewrite replaces).
    sent: Option<u32>,
    /// The buffer differs from what the phone holds.
    dirty: bool,
    /// The next send boundary.
    mark: usize,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// A volume-relative path as `a/b/c` (no empty or `.` components).
fn norm(rel: &str) -> String {
    rel.split('/').filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join("/")
}

/// (folder, leaf) of a normalised path.
fn split(rel: &str) -> (String, String) {
    match rel.rfind('/') {
        Some(i) => (String::from(&rel[..i]), String::from(&rel[i + 1..])),
        None => (String::new(), String::from(rel)),
    }
}

fn ident() -> String {
    ptp::describe().map(|(_, i)| i).unwrap_or_default()
}

fn writable() -> Result<(), VfsError> {
    if ptp::is_mtp_session() { Ok(()) } else { Err(VfsError::Backend(READ_ONLY)) }
}

/// The pending file, if it still belongs to this session (else it is dropped).
fn live(g: &mut Option<Pending>) {
    let stale = match g.as_ref() {
        Some(p) => !ptp::is_mtp_session() || p.ident != ident(),
        None => false,
    };
    if stale {
        *g = None;
    }
}

fn matches(g: &Option<Pending>, rel: &str) -> bool {
    g.as_ref().map(|p| p.rel.eq_ignore_ascii_case(rel)).unwrap_or(false)
}

/// SendObjectInfo + SendObject of the whole buffer; a file the phone already holds is deleted first.
fn send(p: &mut Pending, why: &str) -> Result<(), VfsError> {
    if !p.dirty {
        return Ok(());
    }
    let rewrite = p.sent.take().is_some();
    if rewrite {
        mtp::delete(&p.rel).map_err(VfsError::Backend)?;
    }
    let h = mtp::write_file(&p.dir, &p.leaf, &p.buf).map_err(VfsError::Backend)?;
    p.sent = Some(h);
    p.dirty = false;
    serial_println!("[ptpfs] send path=/{} bytes={} handle={:#010x} why={} rewrite={}", p.rel, p.buf.len(), h, why, rewrite as u8);
    Ok(())
}

/// Close the pending file: send what the phone does not hold yet, then forget the buffer.
fn close(g: &mut Option<Pending>) -> Result<(), VfsError> {
    live(g);
    let Some(mut p) = g.take() else { return Ok(()) };
    let r = send(&mut p, "close");
    if let Err(VfsError::Backend(e)) = &r {
        serial_println!("[ptpfs] send path=/{} bytes={} -> FAILED {}", p.rel, p.buf.len(), e);
    }
    r
}

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
            ptp::Node::Dir { storage, handle } => {
                let mut v: Vec<DirEnt> = ptp::list(storage, handle)
                    .into_iter()
                    .map(|r| DirEnt {
                        name: r.name,
                        kind: if r.dir { NodeKind::Dir } else { NodeKind::File },
                        size: r.size,
                        mtime: r.mtime.as_ref().map(vfs_time),
                    })
                    .collect();
                // MTPWRITE: the pending file is listed with the bytes written so far (a listing never sends).
                let n = norm(rel);
                let mut g = PENDING.lock();
                live(&mut g);
                if let Some(p) = g.as_ref().filter(|p| p.dir.eq_ignore_ascii_case(&n)) {
                    v.retain(|e| !e.name.eq_ignore_ascii_case(&p.leaf));
                    v.push(DirEnt { name: p.leaf.clone(), kind: NodeKind::File, size: p.buf.len() as u64, mtime: None });
                }
                Ok(v)
            }
            _ => Err(VfsError::NotADirectory),
        }
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        {
            // MTPWRITE: a stat of the pending file closes it (its identity is the handle the send makes).
            let n = norm(rel);
            let mut g = PENDING.lock();
            live(&mut g);
            if matches(&g, &n) {
                close(&mut g)?;
            }
        }
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
        {
            // MTPWRITE: the pending file reads from its buffer.
            let n = norm(rel);
            let mut g = PENDING.lock();
            live(&mut g);
            if let Some(p) = g.as_ref().filter(|p| p.rel.eq_ignore_ascii_case(&n)) {
                let s = (offset.min(p.buf.len() as u64)) as usize;
                return Ok(p.buf[s..(s + len).min(p.buf.len())].to_vec());
            }
        }
        match ptp::lookup(rel).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::Dir { .. } => Err(VfsError::IsADirectory),
            ptp::Node::File(o) if ptp::is_mtp_session() => mtp::read_object(o.handle, o.size, offset, len).map_err(VfsError::Backend), // MTPWRITE: past 4 GiB by GetPartialObject64
            ptp::Node::File(o) => ptp::read_object(o.handle, o.size, offset, len).map_err(VfsError::Backend),
            ptp::Node::LiveView => {
                let f = if offset == 0 {
                    ptp::liveview_frame(300).map_err(VfsError::Backend)?
                } else {
                    ptp::liveview_cached().ok_or(VfsError::Backend("live view: no frame read at offset 0"))?
                };
                let s = (offset as usize).min(f.len());
                Ok(f[s..(s + len).min(f.len())].to_vec())
            }
        }
    }

    fn authorize_read(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Ok(()) // world-readable: the card is the user's (R130 law 3), as a USB stick's FAT is
    }

    fn write_veto(&self) -> Option<&'static str> {
        if ptp::is_mtp_session() { None } else { Some(READ_ONLY) } // MTPWRITE: a phone takes writes; a camera stays read-only
    }

    fn authorize_write(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        writable() // the phone is the user's, as its read posture says (R130 law 3)
    }

    fn create(&self, rel: &str, kind: NodeKind, _principal: &str) -> Result<Stat, VfsError> {
        writable()?;
        let n = norm(rel);
        let mut g = PENDING.lock();
        close(&mut g)?;
        let (dir, leaf) = split(&n);
        if leaf.is_empty() || ptp::lookup(&n).is_some() {
            return Err(VfsError::Backend("exists"));
        }
        match kind {
            NodeKind::Dir => {
                let h = mtp::make_folder(&dir, &leaf).map_err(VfsError::Backend)?;
                Ok(Stat { kind: NodeKind::Dir, size: 0, id: Some(h as u64), mtime: None })
            }
            _ => {
                mtp::place(&dir, &leaf).map_err(VfsError::Backend)?;
                *g = Some(Pending { ident: ident(), rel: n, dir, leaf, buf: Vec::new(), last: 0, sent: None, dirty: true, mark: MARK0 });
                Ok(Stat { kind: NodeKind::File, size: 0, id: None, mtime: None })
            }
        }
    }

    fn write(&self, rel: &str, offset: u64, data: &[u8], _principal: &str) -> Result<usize, VfsError> {
        writable()?;
        let n = norm(rel);
        let mut g = PENDING.lock();
        live(&mut g);
        if !matches(&g, &n) {
            close(&mut g)?;
            match ptp::lookup(&n).ok_or(VfsError::NoSuchPath)? {
                ptp::Node::File(o) if offset == 0 => {
                    let (dir, leaf) = split(&n);
                    *g = Some(Pending { ident: ident(), rel: n, dir, leaf, buf: Vec::new(), last: 0, sent: Some(o.handle), dirty: true, mark: MARK0 });
                }
                ptp::Node::File(_) => return Err(VfsError::Unsupported), // an append to a sent file would re-read it whole
                _ => return Err(VfsError::IsADirectory),
            }
        }
        let Some(p) = g.as_mut() else { return Err(VfsError::NoSuchPath) };
        let off = offset as usize;
        if offset > p.buf.len() as u64 {
            return Err(VfsError::Backend("offset past the end (no holes)"));
        }
        let end = off + data.len();
        if end > PENDING_CAP {
            return Err(VfsError::Backend("a phone write over 256 MiB (buffered in RAM; streaming owed)"));
        }
        if end > p.buf.len() {
            p.buf.resize(end, 0);
        }
        p.buf[off..end].copy_from_slice(data);
        p.dirty = true;
        let tail = p.last > 0 && data.len() < p.last;
        p.last = data.len();
        let crossed = p.buf.len() >= p.mark;
        while p.mark <= p.buf.len() {
            p.mark = p.mark.saturating_mul(2);
        }
        if tail || crossed {
            if let Err(e) = send(p, if tail { "tail" } else { "mark" }) {
                *g = None;
                return Err(e);
            }
        }
        Ok(data.len())
    }

    fn unlink(&self, rel: &str, _principal: &str) -> Result<(), VfsError> {
        writable()?;
        let n = norm(rel);
        let mut g = PENDING.lock();
        live(&mut g);
        if matches(&g, &n) {
            let p = g.take();
            if p.map(|p| p.sent.is_some()).unwrap_or(false) {
                mtp::delete(&n).map_err(VfsError::Backend)?;
            }
            return Ok(());
        }
        close(&mut g)?;
        match ptp::lookup(&n).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::File(_) => mtp::delete(&n).map_err(VfsError::Backend),
            _ => Err(VfsError::IsADirectory),
        }
    }

    fn remove_dir(&self, rel: &str, _principal: &str) -> Result<(), VfsError> {
        writable()?;
        let n = norm(rel);
        let mut g = PENDING.lock();
        close(&mut g)?;
        match ptp::lookup(&n).ok_or(VfsError::NoSuchPath)? {
            ptp::Node::Dir { storage, handle: Some(h) } => {
                if !ptp::list(storage, Some(h)).is_empty() {
                    return Err(VfsError::Backend("not-empty"));
                }
                mtp::delete(&n).map_err(VfsError::Backend)
            }
            ptp::Node::Dir { .. } => Err(VfsError::IsADirectory),
            _ => Err(VfsError::NotADirectory),
        }
    }

    fn describe(&self) -> Option<String> {
        ptp::describe().map(|(d, _)| d)
    }
}
