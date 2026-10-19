//! CHARTER: Comscan — driver
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

//! MTPPHONE (rmbp-ledger B715, BP-bar-25) — MTP, the PTP superset a phone speaks in File-transfer
//! mode, on PTPCAM's transport (`drivers/xhci/ptp.rs`, whose child module this is). Design and the
//! opcode table: `docs/dev/OS/07_USB_STORAGE/ptp.md` §9.
//!
//! * M1: the Microsoft OS descriptor that admits a vendor-class (0xFF/0xFF/0x00) interface as MTP;
//!   `is_mtp` over DeviceInfo; `enumerate` — storage names, the friendly name, the tree.
//! * M2: the tree in ONE GetObjectPropList (0x9805) transaction, parsed by `prop_list` in one pass;
//!   the per-folder `Walker` when the body refuses it; `read_partial` in bounded chunks.
//! * M3: `send_object` (SendObjectInfo + SendObject), `delete_object`; the kernel surface
//!   `write_file` / `delete` under the controller loan.
//! * MTPWRITE (rmbp-ledger B727): `announce` = SendObjectPropList at 4 GiB and over; `send_folder` /
//!   `make_folder` (SendObjectInfo, format 0x3001); GetPartialObject64 past 4 GiB in `read_partial`;
//!   `read_object` for `fs/ptpfs.rs`; `with_transport` (the controller, or the attached fixture).
//!
//! Everything that talks is generic over [`Transport`]: the controller implements it with
//! `ptp_txn`, and `tests mtpphone` implements it with a synthetic responder ([`fixture`]) whose every
//! transaction crosses `ptp::codec` both ways.
//!
//! Sources: MTP 1.1 (USB-IF, 2011) §5.3 (operations 0x9801/0x9805/0x9808), Appendix B (object property
//! codes), Appendix E.2.1.1 (the ObjectPropList dataset), §3.2 (datatype codes); ISO 15740 §5.5.3
//! (StorageInfo), §10.5 (0x1005, 0x100B, 0x100C, 0x100D, 0x1015); Microsoft OS 1.0 Descriptors
//! Specification (string 0xEE "MSFT100", Extended Compat ID, wIndex 4); libmtp `src/libusb1-glue.c`
//! (the same "MTP" compatibleID probe) — hardware facts only, no code copied.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::codec::{self, DeviceInfo, Response};
use super::{fmt, op, rc, Obj, Phase};
use crate::sync::Mutex;

// ── The numbers ─────────────────────────────────────────────────────────────────────────────────────

/// Operations beyond PTPCAM's set (ISO 15740 §10.5; MTP 1.1 §5.3).
pub mod mop {
    pub const GET_STORAGE_INFO: u16 = 0x1005;
    pub const DELETE_OBJECT: u16 = 0x100B;
    pub const SEND_OBJECT_INFO: u16 = 0x100C;
    pub const SEND_OBJECT: u16 = 0x100D;
    pub const GET_DEVICE_PROP_VALUE: u16 = 0x1015;
    pub const GET_OBJECT_PROPS_SUPPORTED: u16 = 0x9801;
    pub const GET_OBJECT_PROP_LIST: u16 = 0x9805;
    pub const SEND_OBJECT_PROP_LIST: u16 = 0x9808;
    /// MTPWRITE: Android's MTP extension GetPartialObject64 (libmtp ptp.h `PTP_OC_ANDROID_GetPartialObject64`):
    /// handle, offset low, offset high, max bytes. (0x95C4 in that table is BeginEditObject.)
    pub const GET_PARTIAL_OBJECT_64: u16 = 0x95C1;
}

/// MTP response codes this module reads (MTP 1.1 §F.3).
pub mod mrc {
    pub const SPEC_BY_GROUP_UNSUPPORTED: u16 = 0xA807;
    pub const SPEC_BY_DEPTH_UNSUPPORTED: u16 = 0xA808;
}

/// Object property codes (MTP 1.1 Appendix B).
pub mod prop {
    pub const STORAGE_ID: u16 = 0xDC01;
    pub const OBJECT_FORMAT: u16 = 0xDC02;
    pub const OBJECT_SIZE: u16 = 0xDC04;
    pub const OBJECT_FILE_NAME: u16 = 0xDC07;
    pub const DATE_CREATED: u16 = 0xDC08;
    pub const DATE_MODIFIED: u16 = 0xDC09;
    pub const PARENT_OBJECT: u16 = 0xDC0B;
    pub const PERSISTENT_UID: u16 = 0xDC41;
    pub const NAME: u16 = 0xDC44;
    /// Device property DeviceFriendlyName (MTP 1.1 Appendix C).
    pub const DEVICE_FRIENDLY_NAME: u16 = 0xD402;
}

/// Datatype codes (MTP 1.1 §3.2 / ISO 15740 §5.3).
pub mod dt {
    pub const UINT16: u16 = 0x0004;
    pub const UINT32: u16 = 0x0006;
    pub const UINT64: u16 = 0x0008;
    pub const UINT128: u16 = 0x000A;
    pub const STR: u16 = 0xFFFF;
}

/// Object formats used for a write (ISO 15740 §6.2).
const FMT_UNDEFINED: u16 = 0x3000;
const FMT_TEXT: u16 = 0x3004;
const FMT_PNG: u16 = 0x380B;
/// `PTP_VENDOR_MTP` (libgphoto2 ptp.h; MTP 1.1 §5.1.1.2: VendorExtensionID 6).
pub const VENDOR_EXT_MTP: u32 = 6;
/// The read chunk on the cable: one GetPartialObject asks for at most this much.
pub const READ_CHUNK: usize = 64 << 20;
/// The prop list of a whole phone: 16 Ki objects x ~7 properties x ~60 bytes, with room.
const PROPLIST_CAP: usize = 32 << 20;

/// The transaction surface: the controller (`ptp_txn`) and the fixture's responder.
pub trait Transport {
    fn txn(&mut self, op_code: u16, params: &[u32], phase: Phase<'_>) -> Result<Response, &'static str>;
}

impl Transport for super::super::XhciController {
    fn txn(&mut self, op_code: u16, params: &[u32], phase: Phase<'_>) -> Result<Response, &'static str> {
        self.ptp_txn(op_code, params, phase)
    }
}

// ── M1: the Microsoft OS descriptor (pure) ──────────────────────────────────────────────────────────

/// String descriptor 0xEE: bLength 0x12, bDescriptorType 3, "MSFT100" UTF-16LE, bMS_VendorCode, pad.
pub fn os_string_vendor_code(d: &[u8]) -> Option<u8> {
    if d.len() < 18 || d[1] != 0x03 || (d[0] as usize) < 18 {
        return None;
    }
    let sig = b"MSFT100";
    for (i, c) in sig.iter().enumerate() {
        if d[2 + i * 2] != *c || d[3 + i * 2] != 0 {
            return None;
        }
    }
    Some(d[16])
}

/// The Extended Compat ID descriptor (wIndex 4): a 16-byte header (dwLength, bcdVersion, wIndex = 4,
/// bCount) then 24-byte function sections whose compatibleID (8 bytes, at +2) names the class.
pub fn compat_id_is_mtp(d: &[u8]) -> bool {
    if d.len() < 16 + 24 || u16::from_le_bytes([d[6], d[7]]) != 4 {
        return false;
    }
    let count = (d[8] as usize).min((d.len() - 16) / 24);
    (0..count).any(|i| {
        let id = &d[16 + i * 24 + 2..16 + i * 24 + 10];
        id[..3] == *b"MTP" && id[3..].iter().all(|&b| b == 0)
    })
}

/// A session is MTP-moded: the OS descriptor said so, or DeviceInfo names the MTP extension.
pub fn is_mtp(di: &DeviceInfo, os_desc: bool) -> bool {
    os_desc || di.vendor_ext == VENDOR_EXT_MTP || di.vendor_desc.contains("microsoft.com")
}

// ── M2: the ObjectPropList dataset (pure) ───────────────────────────────────────────────────────────

/// One property value: a number (up to 64 bits), a string, or something read past.
enum Val {
    U(u64),
    S(String),
    Other,
}

/// The width of a scalar datatype 0x0001..=0x000A (INT8 .. UINT128).
fn width(code: u16) -> Option<usize> {
    match code {
        1 | 2 => Some(1),
        3 | 4 => Some(2),
        5 | 6 => Some(4),
        7 | 8 => Some(8),
        9 | 10 => Some(16),
        _ => None,
    }
}

fn value(c: &mut codec::Cur<'_>, code: u16) -> Option<Val> {
    match code {
        dt::STR => Some(Val::S(c.string()?)),
        0x0001..=0x000A => Some(match width(code)? {
            1 => Val::U(c.u8()? as u64),
            2 => Val::U(c.u16()? as u64),
            4 => Val::U(c.u32()? as u64),
            8 => Val::U(c.u64()?),
            _ => {
                c.skip(16)?;
                Val::Other
            }
        }),
        0x4001..=0x400A => {
            let n = c.u32()? as usize;
            c.skip(n.checked_mul(width(code & 0x00FF)?)?)?;
            Some(Val::Other)
        }
        _ => None,
    }
}

#[derive(Default)]
struct Partial {
    storage: Option<u32>,
    format: Option<u16>,
    size: Option<u64>,
    file_name: Option<String>,
    name: Option<String>,
    modified: Option<codec::DateTime>,
    created: Option<codec::DateTime>,
    parent: Option<u32>,
}

/// Parse an ObjectPropList dataset (MTP 1.1 Appendix E.2.1.1) into objects, in the order their first
/// element appeared. `None` = truncated or a datatype this parser cannot size (the caller walks instead).
pub fn prop_list(p: &[u8]) -> Option<Vec<Obj>> {
    let mut c = codec::Cur::new(p);
    let n = c.u32()? as usize;
    if n > p.len() / 9 {
        return None; // every element is at least handle + code + datatype + one byte
    }
    let mut order: Vec<u32> = Vec::new();
    let mut map: BTreeMap<u32, Partial> = BTreeMap::new();
    for _ in 0..n {
        let h = c.u32()?;
        let code = c.u16()?;
        let ty = c.u16()?;
        let v = value(&mut c, ty)?;
        let e = map.entry(h).or_insert_with(|| {
            order.push(h);
            Partial::default()
        });
        match (code, v) {
            (prop::STORAGE_ID, Val::U(x)) => e.storage = Some(x as u32),
            (prop::OBJECT_FORMAT, Val::U(x)) => e.format = Some(x as u16),
            (prop::OBJECT_SIZE, Val::U(x)) => e.size = Some(x),
            (prop::OBJECT_FILE_NAME, Val::S(s)) => e.file_name = Some(s),
            (prop::NAME, Val::S(s)) => e.name = Some(s),
            (prop::DATE_MODIFIED, Val::S(s)) => e.modified = codec::date_time(&s),
            (prop::DATE_CREATED, Val::S(s)) => e.created = codec::date_time(&s),
            (prop::PARENT_OBJECT, Val::U(x)) => e.parent = Some(x as u32),
            _ => {}
        }
    }
    Some(
        order
            .into_iter()
            .filter_map(|h| {
                let e = map.remove(&h)?;
                let name = e.file_name.or(e.name).unwrap_or_default();
                Some(Obj {
                    handle: h,
                    storage: e.storage.unwrap_or(0),
                    parent: e.parent.unwrap_or(0),
                    format: e.format.unwrap_or(FMT_UNDEFINED),
                    size: e.size.unwrap_or(0),
                    name: super::sanitize(&name, h),
                    mtime: e.modified.or(e.created),
                })
            })
            .collect(),
    )
}

// ── Encoders (the write path's datasets; the fixture's canned answers use the same) ─────────────────

pub mod enc {
    use alloc::vec::Vec;

    pub fn u16(o: &mut Vec<u8>, v: u16) {
        o.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(o: &mut Vec<u8>, v: u32) {
        o.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(o: &mut Vec<u8>, v: u64) {
        o.extend_from_slice(&v.to_le_bytes());
    }
    /// A PTP String: u8 count of UCS-2 units INCLUDING the NUL (0 = empty), at most 254 characters.
    pub fn string(o: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().take(254).collect();
        if units.is_empty() {
            o.push(0);
            return;
        }
        o.push(units.len() as u8 + 1);
        for u in units {
            u16(o, u);
        }
        u16(o, 0);
    }
    pub fn au16(o: &mut Vec<u8>, v: &[u16]) {
        u32(o, v.len() as u32);
        for x in v {
            u16(o, *x);
        }
    }
    pub fn au32(o: &mut Vec<u8>, v: &[u32]) {
        u32(o, v.len() as u32);
        for x in v {
            u32(o, *x);
        }
    }
}

/// An ObjectInfo dataset for SendObjectInfo (ISO 15740 §5.5.2), the fields a receiver reads.
pub fn enc_object_info(storage: u32, parent: u32, format: u16, size: u64, name: &str) -> Vec<u8> {
    let mut o = Vec::new();
    enc::u32(&mut o, storage);
    enc::u16(&mut o, format);
    enc::u16(&mut o, 0); // protection: none
    enc::u32(&mut o, size.min(0xFFFF_FFFF) as u32);
    enc::u16(&mut o, 0); // thumb format
    for _ in 0..6 {
        enc::u32(&mut o, 0); // thumb size, thumb w/h, image w/h, bit depth
    }
    enc::u32(&mut o, parent);
    enc::u16(&mut o, if format == fmt::ASSOCIATION { 1 } else { 0 }); // association type: generic folder
    enc::u32(&mut o, 0); // association desc
    enc::u32(&mut o, 0); // sequence
    enc::string(&mut o, name);
    enc::string(&mut o, ""); // capture date
    enc::string(&mut o, ""); // modification date
    enc::string(&mut o, ""); // keywords
    o
}

/// SendObjectPropList's dataset (MTP 1.1 §5.3.3.4): an ObjectPropList whose handles are 0.
pub fn enc_send_props(name: &str, size: u64) -> Vec<u8> {
    let mut o = Vec::new();
    enc::u32(&mut o, 2);
    enc::u32(&mut o, 0);
    enc::u16(&mut o, prop::OBJECT_FILE_NAME);
    enc::u16(&mut o, dt::STR);
    enc::string(&mut o, name);
    enc::u32(&mut o, 0);
    enc::u16(&mut o, prop::OBJECT_SIZE);
    enc::u16(&mut o, dt::UINT64);
    enc::u64(&mut o, size);
    o
}

/// The object format a written file is announced as, by its extension.
pub fn format_for(name: &str) -> u16 {
    let ext = name.rsplit('.').next().unwrap_or("");
    if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
        fmt::EXIF_JPEG
    } else if ext.eq_ignore_ascii_case("png") {
        FMT_PNG
    } else if ext.eq_ignore_ascii_case("txt") {
        FMT_TEXT
    } else {
        FMT_UNDEFINED
    }
}

/// An ObjectInfo answer as a table row.
fn obj_of(h: u32, o: codec::ObjectInfo) -> Obj {
    Obj {
        handle: h,
        storage: o.storage,
        parent: o.parent,
        format: o.format,
        size: o.size as u64,
        name: super::sanitize(&o.filename, h),
        mtime: o.modified.or(o.capture),
    }
}

// ── M1 + M2 over a Transport ────────────────────────────────────────────────────────────────────────

/// What an MTP session learned at bring-up.
pub struct Listing {
    /// (StorageID, its folder name), in GetStorageIDs order.
    pub storages: Vec<(u32, String)>,
    pub friendly: Option<String>,
    pub props_supported: usize,
    /// The whole tree when the prop list answered (`fast`); else empty and `walker` set.
    pub objs: Vec<Obj>,
    pub fast: bool,
    pub walker: Option<Walker>,
}

fn ok_in<T: Transport + ?Sized>(t: &mut T, op_code: u16, params: &[u32], cap: usize) -> Result<Option<Vec<u8>>, &'static str> {
    let mut raw = Vec::new();
    let r = t.txn(op_code, params, Phase::In { cap, into: &mut raw })?;
    Ok(if r.code == rc::OK { Some(raw) } else { None })
}

/// StorageInfo (ISO 15740 §5.5.3): the StorageDescription, else the VolumeLabel.
fn storage_desc(p: &[u8]) -> Option<String> {
    let mut c = codec::Cur::new(p);
    c.u16()?; // StorageType
    c.u16()?; // FilesystemType
    c.u16()?; // AccessCapability
    c.u64()?; // MaxCapacity
    c.u64()?; // FreeSpaceInBytes
    c.u32()?; // FreeSpaceInObjects
    let desc = c.string()?;
    let label = c.string().unwrap_or_default();
    Some(if desc.trim().is_empty() { label } else { desc })
}

/// Storage folder names: sanitised, never empty, unique (case-insensitively, as lookup compares).
fn storage_names(raw: Vec<Option<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, n) in raw.into_iter().enumerate() {
        let base = n.map(|s| super::sanitize(s.trim(), 0)).filter(|s| s != "object-00000000").unwrap_or_else(|| alloc::format!("storage{}", i + 1));
        let mut name = base.clone();
        let mut k = 2;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            name = alloc::format!("{}-{}", base, k);
            k += 1;
        }
        out.push(name);
    }
    out
}

/// The tree in one transaction: depth 0xFFFFFFFF from handle 0, then (a body that refuses depth) the
/// all-objects form, handle 0xFFFFFFFF depth 0. `None` = refused or unparseable: the caller walks.
pub fn list_fast<T: Transport + ?Sized>(t: &mut T) -> Result<Option<Vec<Obj>>, &'static str> {
    for (h, depth) in [(0u32, 0xFFFF_FFFFu32), (0xFFFF_FFFF, 0)] {
        let mut raw = Vec::new();
        let r = t.txn(mop::GET_OBJECT_PROP_LIST, &[h, 0, 0xFFFF_FFFF, 0, depth], Phase::In { cap: PROPLIST_CAP, into: &mut raw })?;
        if r.code == rc::OK {
            return Ok(prop_list(&raw));
        }
        if r.code != mrc::SPEC_BY_DEPTH_UNSUPPORTED && r.code != mrc::SPEC_BY_GROUP_UNSUPPORTED {
            return Ok(None);
        }
    }
    Ok(None)
}

/// M1 + M2: storages and their names, the friendly name, the property count, the tree (or a walker).
pub fn enumerate<T: Transport + ?Sized>(t: &mut T, di: &DeviceInfo, storages: &[u32]) -> Result<Listing, &'static str> {
    let mut raw_names = Vec::new();
    for &s in storages {
        let n = if di.ops.contains(&mop::GET_STORAGE_INFO) { ok_in(t, mop::GET_STORAGE_INFO, &[s], 4096)?.and_then(|p| storage_desc(&p)) } else { None };
        raw_names.push(n);
    }
    let names = storage_names(raw_names);
    let friendly = if di.ops.contains(&mop::GET_DEVICE_PROP_VALUE) && di.props.contains(&prop::DEVICE_FRIENDLY_NAME) {
        ok_in(t, mop::GET_DEVICE_PROP_VALUE, &[prop::DEVICE_FRIENDLY_NAME as u32], 1024)?.and_then(|p| codec::Cur::new(&p).string()).filter(|s| !s.trim().is_empty())
    } else {
        None
    };
    let props_supported = if di.ops.contains(&mop::GET_OBJECT_PROPS_SUPPORTED) {
        ok_in(t, mop::GET_OBJECT_PROPS_SUPPORTED, &[FMT_UNDEFINED as u32], 4096)?.and_then(|p| codec::Cur::new(&p).au16()).map(|v| v.len()).unwrap_or(0)
    } else {
        0
    };
    let fast = if di.ops.contains(&mop::GET_OBJECT_PROP_LIST) { list_fast(t)? } else { None };
    let storages: Vec<(u32, String)> = storages.iter().copied().zip(names).collect();
    Ok(match fast {
        Some(mut objs) => {
            objs.truncate(super::MAX_OBJECTS);
            Listing { storages, friendly, props_supported, objs, fast: true, walker: None }
        }
        None => {
            let w = Walker::new(&storages.iter().map(|s| s.0).collect::<Vec<_>>());
            Listing { storages, friendly, props_supported, objs: Vec::new(), fast: false, walker: Some(w) }
        }
    })
}

/// The per-folder walk (a body without a usable 0x9805): GetObjectHandles(storage, 0, folder) from each
/// storage's root (0xFFFFFFFF), GetObjectInfo per handle, folders queued; resumable a budget at a time.
pub struct Walker {
    folders: VecDeque<(u32, u32)>,
    pending: VecDeque<u32>,
    found: usize,
}

impl Walker {
    pub fn new(storages: &[u32]) -> Self {
        Self { folders: storages.iter().map(|&s| (s, 0xFFFF_FFFF)).collect(), pending: VecDeque::new(), found: 0 }
    }
    pub fn done(&self) -> bool {
        (self.folders.is_empty() && self.pending.is_empty()) || self.found >= super::MAX_OBJECTS
    }
    /// At most `budget` transactions; the objects they found.
    pub fn step<T: Transport + ?Sized>(&mut self, t: &mut T, budget: usize) -> Result<Vec<Obj>, &'static str> {
        let mut out = Vec::new();
        for _ in 0..budget {
            if self.done() {
                break;
            }
            if let Some(h) = self.pending.pop_front() {
                if let Some(p) = ok_in(t, op::GET_OBJECT_INFO, &[h], 4096)? {
                    if let Some(o) = codec::object_info(&p) {
                        let o = obj_of(h, o);
                        if o.is_dir() {
                            self.folders.push_back((o.storage, h));
                        }
                        self.found += 1;
                        out.push(o);
                    }
                }
            } else if let Some((s, parent)) = self.folders.pop_front() {
                if let Some(p) = ok_in(t, op::GET_OBJECT_HANDLES, &[s, 0, parent], (super::MAX_OBJECTS + 1) * 4)? {
                    self.pending.extend(codec::u32_array(&p).unwrap_or_default());
                }
            }
        }
        Ok(out)
    }
}

/// `len` bytes of `handle` from `off` by GetPartialObject in chunks of at most `chunk`; a chunk that
/// comes back short ends the read (the object's end). One chunk = PTPCAM's single transaction.
pub fn read_partial<T: Transport + ?Sized>(t: &mut T, handle: u32, off: u64, len: usize, chunk: usize) -> Result<Vec<u8>, &'static str> {
    let mut v = Vec::with_capacity(len.min(READ_CHUNK));
    while v.len() < len {
        let at = off + v.len() as u64;
        let want = (len - v.len()).min(chunk.max(1));
        let mut part = Vec::with_capacity(want); // fresh per chunk: Phase::In's cap counts what `into` holds
        let r = if at > u32::MAX as u64 {
            // MTPWRITE: an offset past 4 GiB — GetPartialObject64 (handle, offset low, offset high, max).
            t.txn(mop::GET_PARTIAL_OBJECT_64, &[handle, at as u32, (at >> 32) as u32, want as u32], Phase::In { cap: want, into: &mut part })?
        } else {
            t.txn(op::GET_PARTIAL_OBJECT, &[handle, at as u32, want as u32], Phase::In { cap: want, into: &mut part })?
        };
        if r.code != rc::OK {
            return Err("GetPartialObject refused");
        }
        let short = part.len() < want;
        v.extend_from_slice(&part);
        if short {
            break;
        }
    }
    Ok(v)
}

// ── M3 over a Transport ─────────────────────────────────────────────────────────────────────────────

/// SendObjectInfo + SendObject: `data` as `name` inside (`storage`, `parent`; 0xFFFFFFFF = the root).
/// The new object's handle (the SendObjectInfo response's third parameter).
pub fn send_object<T: Transport + ?Sized>(t: &mut T, storage: u32, parent: u32, name: &str, data: &[u8]) -> Result<u32, &'static str> {
    let handle = announce(t, storage, parent, name, data.len() as u64)?;
    let r = t.txn(mop::SEND_OBJECT, &[], Phase::Out(data))?;
    if r.code != rc::OK {
        return Err("SendObject refused");
    }
    Ok(handle)
}

/// MTPWRITE: the object's announcement, the half of a send before SendObject. Below 4 GiB SendObjectInfo
/// (ObjectInfo's size is 32 bits); at 4 GiB and over SendObjectPropList (MTP 1.1 §5.3.3.4: storage, parent,
/// format, size high, size low; the dataset `enc_send_props`; the response's third parameter is the handle).
pub fn announce<T: Transport + ?Sized>(t: &mut T, storage: u32, parent: u32, name: &str, size: u64) -> Result<u32, &'static str> {
    let r = if size >= 0xFFFF_FFFF {
        let props = enc_send_props(name, size);
        t.txn(mop::SEND_OBJECT_PROP_LIST, &[storage, parent, format_for(name) as u32, (size >> 32) as u32, size as u32], Phase::Out(&props))?
    } else {
        let info = enc_object_info(storage, parent, format_for(name), size, name);
        t.txn(mop::SEND_OBJECT_INFO, &[storage, parent], Phase::Out(&info))?
    };
    if r.code != rc::OK || r.n < 3 {
        return Err(if size >= 0xFFFF_FFFF { "SendObjectPropList refused" } else { "SendObjectInfo refused" });
    }
    Ok(r.params[2])
}

/// MTPWRITE: a folder — SendObjectInfo with format 0x3001 (association, generic folder) and no SendObject
/// (an association carries no data; libmtp's folder create sends the info alone). The new handle.
pub fn send_folder<T: Transport + ?Sized>(t: &mut T, storage: u32, parent: u32, name: &str) -> Result<u32, &'static str> {
    let info = enc_object_info(storage, parent, fmt::ASSOCIATION, 0, name);
    let r = t.txn(mop::SEND_OBJECT_INFO, &[storage, parent], Phase::Out(&info))?;
    if r.code != rc::OK || r.n < 3 {
        return Err("SendObjectInfo (folder) refused");
    }
    Ok(r.params[2])
}

pub fn delete_object<T: Transport + ?Sized>(t: &mut T, handle: u32) -> Result<(), &'static str> {
    let r = t.txn(mop::DELETE_OBJECT, &[handle, 0], Phase::None)?;
    if r.code == rc::OK { Ok(()) } else { Err("DeleteObject refused") }
}

// ── The kernel side: the session's MTP state ────────────────────────────────────────────────────────

/// The Microsoft OS descriptor named this device MTP (set by the probe, read at bring-up).
pub(super) static OS_DESC_MTP: AtomicBool = AtomicBool::new(false);
static T0: AtomicU64 = AtomicU64::new(0);
static WALKER: Mutex<Option<Walker>> = Mutex::new(None);
static LINE: Mutex<Option<String>> = Mutex::new(None);
/// MTPWRITE: the synthetic responder while `tests mtpphone` has it attached (`fixture::attach`).
static FIXTURE: Mutex<Option<fixture::Responder>> = Mutex::new(None);

/// MTPWRITE: run `f` on the session's transport — the attached fixture, else the controller under the loan.
fn with_transport<R>(f: impl FnOnce(&mut dyn Transport) -> Result<R, &'static str>) -> Result<R, &'static str> {
    {
        let mut g = FIXTURE.lock();
        if let Some(r) = g.as_mut() {
            return f(r);
        }
    }
    super::with_controller(|c| f(c))
}

pub(super) fn forget() {
    OS_DESC_MTP.store(false, Ordering::Relaxed);
    *WALKER.lock() = None;
    *LINE.lock() = None;
}

pub fn walk_pending() -> bool {
    WALKER.lock().is_some()
}

fn say_listed(fast: bool) {
    let g = LINE.lock();
    let Some(head) = g.as_ref() else { return };
    let n = super::TABLE.lock().infos.len();
    let ms = super::now_ms().saturating_sub(T0.load(Ordering::Relaxed));
    serial_println!("[mtp] {} objects={} props={} ms={}", head, n, if fast { "fast" } else { "walk" }, ms);
}

/// Bring-up's MTP half, after the object table was reset: the rows (fast) or the walker (walk).
pub(super) fn install(l: Listing, t0: u64, vid: u16, pid: u16, volume: &str) {
    T0.store(t0, Ordering::Relaxed);
    *LINE.lock() = Some(alloc::format!("dev={:04x}:{:04x} name={} storages={}", vid, pid, volume, l.storages.len()));
    serial_println!(
        "[mtp] session storages=[{}] friendly={} props_supported={} list={}",
        l.storages.iter().map(|s| alloc::format!("{:#010x}:{}", s.0, s.1)).collect::<Vec<_>>().join(","),
        l.friendly.as_deref().unwrap_or("-"),
        l.props_supported,
        if l.fast { "GetObjectPropList" } else { "walk" }
    );
    if l.fast {
        let mut t = super::TABLE.lock();
        t.handles = l.objs.iter().map(|o| o.handle).collect();
        for o in l.objs {
            t.infos.insert(o.handle, o);
        }
        drop(t);
        say_listed(true);
    } else {
        super::TABLE.lock().handles.clear();
        *WALKER.lock() = l.walker;
    }
}

impl super::super::XhciController {
    /// The service pass's walk step (`ptp_fill`, while a walker is pending): FILL_PER_PASS transactions.
    pub(super) fn mtp_walk_step(&mut self) {
        let Some(mut w) = WALKER.lock().take() else { return };
        let r = w.step(self, super::FILL_PER_PASS);
        let objs = match r {
            Ok(v) => v,
            Err(_) => return, // `ptp_txn` named the transport failure; the session is closed
        };
        {
            let mut t = super::TABLE.lock();
            for o in objs {
                t.handles.push(o.handle);
                t.infos.insert(o.handle, o);
            }
        }
        if w.done() {
            say_listed(false);
            let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
        } else {
            *WALKER.lock() = Some(w);
        }
    }

    /// The Microsoft OS descriptor probe (synchronous EP0 — the hub path and the service pass only):
    /// string 0xEE, then the Extended Compat ID (device recipient, then interface recipient). A STALL
    /// halts the xHCI control endpoint; it is un-halted before returning, so the next driver's EP0 lives.
    pub(super) fn mtp_os_probe(&mut self, slot: u8, iface: u8) -> bool {
        let buf = self.slots[slot as usize].descriptor_buffer as u64;
        if buf == 0 {
            return false;
        }
        let read = |c: &mut Self, bm: u8, req: u8, val: u16, idx: u16, len: u16| -> Option<Vec<u8>> {
            unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 256) };
            let r = c.sync_control(slot, bm, req, val, idx, len, buf, true);
            if !matches!(r, Ok(1) | Ok(13)) {
                c.mtp_ep0_unhalt(slot);
                return None;
            }
            let n = (c.last_control_len as usize).min(len as usize);
            Some(unsafe { core::slice::from_raw_parts(buf as *const u8, n) }.to_vec())
        };
        let Some(code) = read(self, 0x80, 0x06, 0x03EE, 0, 18).and_then(|d| os_string_vendor_code(&d)) else {
            serial_println!("[mtp] slot={} iface={} os-descriptor=none -> not MTP (left to the stock path)", slot, iface);
            return false;
        };
        let dev = read(self, 0xC0, code, 0, 4, 40).map(|d| compat_id_is_mtp(&d)).unwrap_or(false);
        let yes = dev || read(self, 0xC1, code, iface as u16, 4, 40).map(|d| compat_id_is_mtp(&d)).unwrap_or(false);
        serial_println!("[mtp] slot={} iface={} os-descriptor=MSFT100 vendor_code={:#04x} compat={}", slot, iface, code, if yes { "MTP" } else { "other" });
        if yes {
            OS_DESC_MTP.store(true, Ordering::Relaxed);
        }
        yes
    }

    /// Reset Endpoint (DCI 1) + Set TR Dequeue Pointer to the ep0 ring's enqueue (xHCI 1.2 §4.6.8,
    /// §4.6.10) — only when the control endpoint reads Halted/Error.
    fn mtp_ep0_unhalt(&mut self, slot: u8) {
        let st = self.ep_state_of(slot, 1);
        if st != 2 && st != 4 {
            return;
        }
        let ctx = (1u32 << 16) | ((slot as u32) << 24);
        let (ok, cc, _) = self.recover_cmd(super::super::trb::Trb { parameter: 0, status: 0, control: (14 << 10) | ctx });
        let deq = self.slots[slot as usize].ep0_ring.as_ref().map(|r| r.enqueue_ptr_dcs());
        let set = match (ok, deq) {
            (true, Some(d)) => self.recover_cmd(super::super::trb::Trb { parameter: d, status: 0, control: (16 << 10) | ctx }).0,
            _ => false,
        };
        serial_println!("[mtp] ep0 un-halt slot={} reset={} cc={} set_deq={}", slot, ok as u8, cc, set as u8);
    }
}

// ── The kernel surface: writes and deletes under the controller loan ───────────────────────────────

/// Copy `data` onto the phone as `name` inside the folder `dir_rel` (volume-relative; a storage folder
/// or any folder below one). The new handle; the table holds the row at once.
pub fn write_file(dir_rel: &str, name: &str, data: &[u8]) -> Result<u32, &'static str> {
    let (storage, parent) = place(dir_rel, name)?;
    let t0 = super::now_ms();
    let h = with_transport(|c| send_object(c, storage, parent, name, data))?;
    add_row(h, storage, parent, format_for(name), data.len() as u64, name);
    serial_println!("[mtp] write name={} bytes={} storage={:#010x} parent={:#010x} handle={:#010x} ms={}", name, data.len(), storage, parent, h, super::now_ms().saturating_sub(t0));
    Ok(h)
}

/// MTPWRITE: where a new `name` inside the folder `dir_rel` goes — (StorageID, parent handle; 0xFFFFFFFF =
/// the storage's root) — or why it cannot.
pub fn place(dir_rel: &str, name: &str) -> Result<(u32, u32), &'static str> {
    if !super::is_mtp_session() {
        return Err("not an MTP session (the camera volume is read-only)");
    }
    let at = match super::lookup(dir_rel).ok_or("no such folder")? {
        super::Node::Dir { storage: Some(s), handle } => (s, handle.unwrap_or(0xFFFF_FFFF)),
        super::Node::Dir { storage: None, .. } => return Err("write into a storage folder, not the volume root"),
        _ => return Err("not a folder"),
    };
    if name.is_empty() || super::sanitize(name, 0) != name {
        return Err("name is not a path component");
    }
    Ok(at)
}

/// The table holds a new object's row at once (the listing never waits for a re-list).
fn add_row(h: u32, storage: u32, parent: u32, format: u16, size: u64, name: &str) {
    let row = Obj { handle: h, storage, parent: if parent == 0xFFFF_FFFF { 0 } else { parent }, format, size, name: String::from(name), mtime: None };
    {
        let mut t = super::TABLE.lock();
        t.handles.push(h);
        t.infos.insert(h, row);
    }
    let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
}

/// MTPWRITE: a folder `name` inside `dir_rel` (SendObjectInfo, format 0x3001). The new handle.
pub fn make_folder(dir_rel: &str, name: &str) -> Result<u32, &'static str> {
    let (storage, parent) = place(dir_rel, name)?;
    let h = with_transport(|c| send_folder(c, storage, parent, name))?;
    add_row(h, storage, parent, fmt::ASSOCIATION, 0, name);
    serial_println!("[mtp] mkdir name={} storage={:#010x} parent={:#010x} handle={:#010x}", name, storage, parent, h);
    Ok(h)
}

/// MTPWRITE: `len` bytes of an MTP object from `off` for the volume's read. A range starting past 4 GiB
/// goes by GetPartialObject64 (`read_partial`); the rest is PTPCAM's path unchanged (`ptp::read_object`),
/// except under the attached fixture, which answers every range by `read_partial`.
pub fn read_object(handle: u32, size: u64, off: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    if off >= size || len == 0 {
        return Ok(Vec::new());
    }
    let len = len.min((size - off) as usize);
    if FIXTURE.lock().is_some() || (off > u32::MAX as u64 && super::PARTIAL_OK.load(Ordering::Relaxed)) {
        return with_transport(|c| read_partial(c, handle, off, len, READ_CHUNK));
    }
    super::read_object(handle, size, off, len)
}

/// Delete the object at `rel` (a file, or a folder with what it holds — the phone deletes the subtree).
pub fn delete(rel: &str) -> Result<(), &'static str> {
    if !super::is_mtp_session() {
        return Err("not an MTP session (the camera volume is read-only)");
    }
    let h = match super::lookup(rel).ok_or("no such path")? {
        super::Node::File(o) => o.handle,
        super::Node::Dir { handle: Some(h), .. } => h,
        _ => return Err("a storage folder cannot be deleted"),
    };
    with_transport(|c| delete_object(c, h))?;
    let gone = {
        let mut t = super::TABLE.lock();
        let mut gone: alloc::collections::BTreeSet<u32> = alloc::collections::BTreeSet::new();
        gone.insert(h);
        loop {
            let more: Vec<u32> = t.infos.values().filter(|o| gone.contains(&o.parent) && !gone.contains(&o.handle)).map(|o| o.handle).collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
        t.handles.retain(|x| !gone.contains(x));
        t.infos.retain(|x, _| !gone.contains(x));
        gone.len()
    };
    *super::OBJ_CACHE.lock() = None;
    let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
    serial_println!("[mtp] delete path={} handle={:#010x} rows_removed={}", rel, h, gone);
    Ok(())
}

// ── `tests mtpphone`: a synthetic MTP responder over an in-RAM transport ────────────────────────────

pub mod fixture {
    use super::super::codec::{self, Response, HDR};
    use super::super::{ctype, fmt, op, rc, Phase};
    use super::{dt, enc, mop, mrc, prop, Transport};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;

    pub const S1: u32 = 0x0001_0001;
    pub const S2: u32 = 0x0002_0001;
    pub const BIG: u32 = 6;
    pub const BIG_SIZE: u64 = 3 << 20;
    pub const DOWNLOAD: u32 = 3;
    pub const OBJECTS: usize = 40;
    /// MTPWRITE: the 5 GiB sparse object `with_big` adds (Download/BIG.MOV); its bytes are `pat`.
    pub const HUGE: u32 = 50;
    pub const HUGE_SIZE: u64 = 5 << 30;

    struct FObj {
        storage: u32,
        parent: u32,
        format: u16,
        name: String,
        size: u64,
        data: Option<Vec<u8>>,
    }

    /// The canned byte at `i` of object `h` (never all one value; the read check compares every byte).
    pub fn pat(h: u32, i: u64) -> u8 {
        (i.wrapping_mul(31) ^ (i >> 9) ^ (h as u64).wrapping_mul(7)) as u8
    }

    /// A phone: two storages, 40 objects (DCIM, DCIM/Camera, Download, Music on the first; Backup on the
    /// second; a 3 MB picture; 34 small files spread over the four leaf folders).
    pub struct Responder {
        objs: BTreeMap<u32, FObj>,
        next: u32,
        tid: u32,
        pending: Option<(u32, u32, u32)>, // SendObjectInfo's (storage, parent, handle) awaiting SendObject
        pub refuse_proplist: bool,
        pub txns: usize,
    }

    impl Responder {
        pub fn new(refuse_proplist: bool) -> Self {
            let mut objs = BTreeMap::new();
            let mut put = |h: u32, storage: u32, parent: u32, format: u16, name: &str, size: u64| {
                objs.insert(h, FObj { storage, parent, format, name: String::from(name), size, data: None });
            };
            put(1, S1, 0, fmt::ASSOCIATION, "DCIM", 0);
            put(2, S1, 1, fmt::ASSOCIATION, "Camera", 0);
            put(3, S1, 0, fmt::ASSOCIATION, "Download", 0);
            put(4, S1, 0, fmt::ASSOCIATION, "Music", 0);
            put(5, S2, 0, fmt::ASSOCIATION, "Backup", 0);
            put(BIG, S1, 2, fmt::EXIF_JPEG, "PXL_20261019_120000.jpg", BIG_SIZE);
            for h in 7..=OBJECTS as u32 {
                let parent = [2u32, 3, 4, 5][(h % 4) as usize];
                let storage = if parent == 5 { S2 } else { S1 };
                let name = alloc::format!("FILE{:02}.TXT", h);
                put(h, storage, parent, 0x3004, &name, 1000 + h as u64);
            }
            Self { objs, next: 100, tid: 0, pending: None, refuse_proplist, txns: 0 }
        }

        pub fn holds(&self, h: u32) -> bool {
            self.objs.contains_key(&h)
        }

        /// MTPWRITE: the phone with one more object, a 5 GiB sparse `BIG.MOV` in Download (never materialised).
        pub fn with_big(mut self) -> Self {
            self.objs.insert(HUGE, FObj { storage: S1, parent: DOWNLOAD, format: 0x3000, name: String::from("BIG.MOV"), size: HUGE_SIZE, data: None });
            self
        }

        /// MTPWRITE: an object's (format, size, name), as the responder holds it.
        pub fn info_of(&self, h: u32) -> Option<(u16, u64, String)> {
            self.objs.get(&h).map(|f| (f.format, f.size, f.name.clone()))
        }

        pub fn bytes_of(&self, h: u32) -> Option<Vec<u8>> {
            let o = self.objs.get(&h)?;
            Some(match &o.data {
                Some(d) => d.clone(),
                None => (0..o.size).map(|i| pat(h, i)).collect(),
            })
        }

        fn device_info() -> Vec<u8> {
            let mut o = Vec::new();
            enc::u16(&mut o, 100);
            enc::u32(&mut o, super::VENDOR_EXT_MTP);
            enc::u16(&mut o, 100);
            enc::string(&mut o, "microsoft.com: 1.0; android.com: 1.0;");
            enc::u16(&mut o, 0);
            enc::au16(
                &mut o,
                &[
                    op::GET_DEVICE_INFO, op::OPEN_SESSION, op::CLOSE_SESSION, op::GET_STORAGE_IDS, mop::GET_STORAGE_INFO,
                    op::GET_OBJECT_HANDLES, op::GET_OBJECT_INFO, op::GET_OBJECT, mop::DELETE_OBJECT, mop::SEND_OBJECT_INFO,
                    mop::SEND_OBJECT, mop::GET_DEVICE_PROP_VALUE, op::GET_PARTIAL_OBJECT, mop::GET_OBJECT_PROPS_SUPPORTED,
                    mop::GET_OBJECT_PROP_LIST, mop::SEND_OBJECT_PROP_LIST,
                ],
            );
            enc::au16(&mut o, &[0x4002, 0x4003, 0x4004, 0x4005, 0x400C]);
            enc::au16(&mut o, &[prop::DEVICE_FRIENDLY_NAME]);
            enc::au16(&mut o, &[]);
            enc::au16(&mut o, &[0x3000, fmt::ASSOCIATION, fmt::EXIF_JPEG]);
            enc::string(&mut o, "Google");
            enc::string(&mut o, "Pixel 8");
            enc::string(&mut o, "1.0");
            enc::string(&mut o, "UNA0001");
            o
        }

        fn storage_info(desc: &str) -> Vec<u8> {
            let mut o = Vec::new();
            enc::u16(&mut o, 3); // fixed RAM
            enc::u16(&mut o, 2); // generic hierarchical
            enc::u16(&mut o, 0); // read-write
            enc::u64(&mut o, 64 << 30);
            enc::u64(&mut o, 32 << 30);
            enc::u32(&mut o, 0xFFFF_FFFF);
            enc::string(&mut o, desc);
            enc::string(&mut o, "");
            o
        }

        fn prop_list(&self) -> Vec<u8> {
            fn el(o: &mut Vec<u8>, h: u32, code: u16, ty: u16) {
                enc::u32(o, h);
                enc::u16(o, code);
                enc::u16(o, ty);
            }
            let mut o = Vec::new();
            enc::u32(&mut o, (self.objs.len() * 7) as u32);
            for (&h, f) in &self.objs {
                el(&mut o, h, prop::STORAGE_ID, dt::UINT32);
                enc::u32(&mut o, f.storage);
                el(&mut o, h, prop::OBJECT_FORMAT, dt::UINT16);
                enc::u16(&mut o, f.format);
                el(&mut o, h, prop::PERSISTENT_UID, dt::UINT128); // read past: a width the parser must honour
                o.extend_from_slice(&[h as u8; 16]);
                el(&mut o, h, prop::OBJECT_SIZE, dt::UINT64);
                enc::u64(&mut o, f.size);
                el(&mut o, h, prop::OBJECT_FILE_NAME, dt::STR);
                enc::string(&mut o, &f.name);
                el(&mut o, h, prop::DATE_MODIFIED, dt::STR);
                enc::string(&mut o, "20261019T120000");
                el(&mut o, h, prop::PARENT_OBJECT, dt::UINT32);
                enc::u32(&mut o, f.parent);
            }
            o
        }

        /// ObjectInfo as the walk reads it: the same fields, and the same mtime the prop list carries.
        fn object_info(&self, h: u32) -> Option<Vec<u8>> {
            let f = self.objs.get(&h)?;
            let mut o = super::enc_object_info(f.storage, f.parent, f.format, f.size, &f.name);
            o.truncate(o.len() - 3); // the three empty strings: capture, modified, keywords
            enc::string(&mut o, "20261019T120000");
            enc::string(&mut o, "20261019T120000");
            enc::string(&mut o, "");
            Some(o)
        }

        /// One operation: (response code, response params, DATA-IN payload).
        fn serve(&mut self, op_code: u16, p: &[u32], out: Option<Vec<u8>>) -> (u16, Vec<u32>, Option<Vec<u8>>) {
            let arg = |i: usize| p.get(i).copied().unwrap_or(0);
            match op_code {
                op::OPEN_SESSION => (rc::OK, Vec::new(), None),
                op::GET_DEVICE_INFO => (rc::OK, Vec::new(), Some(Self::device_info())),
                op::GET_STORAGE_IDS => {
                    let mut o = Vec::new();
                    enc::au32(&mut o, &[S1, S2]);
                    (rc::OK, Vec::new(), Some(o))
                }
                mop::GET_STORAGE_INFO => match arg(0) {
                    S1 => (rc::OK, Vec::new(), Some(Self::storage_info("Internal shared storage"))),
                    S2 => (rc::OK, Vec::new(), Some(Self::storage_info("SD card"))),
                    _ => (0x2008, Vec::new(), None), // InvalidStorageID
                },
                mop::GET_DEVICE_PROP_VALUE if arg(0) == prop::DEVICE_FRIENDLY_NAME as u32 => {
                    let mut o = Vec::new();
                    enc::string(&mut o, "Una Phone");
                    (rc::OK, Vec::new(), Some(o))
                }
                mop::GET_OBJECT_PROPS_SUPPORTED => {
                    let mut o = Vec::new();
                    enc::au16(&mut o, &[prop::STORAGE_ID, prop::OBJECT_FORMAT, prop::OBJECT_SIZE, prop::OBJECT_FILE_NAME, prop::DATE_MODIFIED, prop::PARENT_OBJECT, prop::PERSISTENT_UID, prop::NAME]);
                    (rc::OK, Vec::new(), Some(o))
                }
                mop::GET_OBJECT_PROP_LIST => {
                    if self.refuse_proplist {
                        (0x2005, Vec::new(), None) // OperationNotSupported
                    } else if arg(0) == 0 && arg(4) == 0xFFFF_FFFF && arg(1) == 0 && arg(2) == 0xFFFF_FFFF {
                        (rc::OK, Vec::new(), Some(self.prop_list()))
                    } else {
                        (mrc::SPEC_BY_DEPTH_UNSUPPORTED, Vec::new(), None)
                    }
                }
                op::GET_OBJECT_HANDLES => {
                    let (s, parent) = (arg(0), arg(2));
                    // ISO 15740 §10.5.7: parent 0 = every object, 0xFFFFFFFF = the storage's root level.
                    let want = if parent == 0xFFFF_FFFF { 0 } else { parent };
                    let v: Vec<u32> = self.objs.iter().filter(|(_, f)| (s == 0xFFFF_FFFF || f.storage == s) && (parent == 0 || f.parent == want)).map(|(&h, _)| h).collect();
                    let mut o = Vec::new();
                    enc::au32(&mut o, &v);
                    (rc::OK, Vec::new(), Some(o))
                }
                op::GET_OBJECT_INFO => match self.object_info(arg(0)) {
                    Some(o) => (rc::OK, Vec::new(), Some(o)),
                    None => (rc::INVALID_OBJECT_HANDLE, Vec::new(), None),
                },
                op::GET_OBJECT => match self.bytes_of(arg(0)) {
                    Some(b) => (rc::OK, Vec::new(), Some(b)),
                    None => (rc::INVALID_OBJECT_HANDLE, Vec::new(), None),
                },
                op::GET_PARTIAL_OBJECT => {
                    let Some(f) = self.objs.get(&arg(0)) else { return (rc::INVALID_OBJECT_HANDLE, Vec::new(), None) };
                    let (off, max) = (arg(1) as u64, arg(2) as u64);
                    let end = (off + max).min(f.size);
                    let h = arg(0);
                    let b: Vec<u8> = match &f.data {
                        Some(d) => d[off.min(end) as usize..end as usize].to_vec(),
                        None => (off..end).map(|i| pat(h, i)).collect(),
                    };
                    let n = b.len() as u32;
                    (rc::OK, alloc::vec![n], Some(b))
                }
                mop::SEND_OBJECT_INFO => {
                    let Some(d) = out else { return (0x2002, Vec::new(), None) };
                    let Some(oi) = codec::object_info(&d) else { return (0x2002, Vec::new(), None) };
                    let (s, parent) = (arg(0), arg(1));
                    if s != S1 && s != S2 {
                        return (0x2008, Vec::new(), None);
                    }
                    let h = self.next;
                    self.next += 1;
                    let par = if parent == 0xFFFF_FFFF { 0 } else { parent };
                    self.objs.insert(h, FObj { storage: s, parent: par, format: oi.format, name: oi.filename, size: oi.size as u64, data: None });
                    self.pending = Some((s, par, h));
                    (rc::OK, alloc::vec![s, par, h], None)
                }
                mop::GET_PARTIAL_OBJECT_64 => {
                    // MTPWRITE: handle, offset low, offset high, max bytes; a sparse object answers `pat`.
                    let h = arg(0);
                    let Some(f) = self.objs.get(&h) else { return (rc::INVALID_OBJECT_HANDLE, Vec::new(), None) };
                    let off = (arg(1) as u64) | ((arg(2) as u64) << 32);
                    let end = (off + arg(3) as u64).min(f.size);
                    let b: Vec<u8> = match &f.data {
                        Some(d) => d[(off.min(end) as usize).min(d.len())..(end as usize).min(d.len())].to_vec(),
                        None => (off.min(end)..end).map(|i| pat(h, i)).collect(),
                    };
                    let n = b.len() as u32;
                    (rc::OK, alloc::vec![n], Some(b))
                }
                mop::SEND_OBJECT_PROP_LIST => {
                    // MTPWRITE: storage, parent, format, size high, size low; the dataset names the object.
                    let Some(d) = out else { return (0x2002, Vec::new(), None) };
                    let (s, parent) = (arg(0), arg(1));
                    let size = ((arg(3) as u64) << 32) | arg(4) as u64;
                    let Some(rows) = super::prop_list(&d) else { return (0x2002, Vec::new(), None) };
                    let Some(row) = rows.into_iter().next() else { return (0x2002, Vec::new(), None) };
                    if (s != S1 && s != S2) || row.size != size {
                        return (0x2002, Vec::new(), None);
                    }
                    let h = self.next;
                    self.next += 1;
                    let par = if parent == 0xFFFF_FFFF { 0 } else { parent };
                    self.objs.insert(h, FObj { storage: s, parent: par, format: arg(2) as u16, name: row.name, size, data: None });
                    self.pending = Some((s, par, h));
                    (rc::OK, alloc::vec![s, par, h], None)
                }
                mop::SEND_OBJECT => {
                    let (Some(d), Some((_, _, h))) = (out, self.pending.take()) else { return (0x2002, Vec::new(), None) };
                    match self.objs.get_mut(&h) {
                        Some(f) if f.size == d.len() as u64 => {
                            f.data = Some(d);
                            (rc::OK, Vec::new(), None)
                        }
                        _ => (0x2002, Vec::new(), None), // GeneralError: size differs from the info
                    }
                }
                mop::DELETE_OBJECT => {
                    if self.objs.remove(&arg(0)).is_some() {
                        let h = arg(0);
                        self.objs.retain(|_, f| f.parent != h);
                        (rc::OK, Vec::new(), None)
                    } else {
                        (rc::INVALID_OBJECT_HANDLE, Vec::new(), None)
                    }
                }
                _ => (0x2005, Vec::new(), None),
            }
        }
    }

    /// The driver's three phases over RAM: every container is encoded and decoded by `ptp::codec`.
    impl Transport for Responder {
        fn txn(&mut self, op_code: u16, params: &[u32], phase: Phase<'_>) -> Result<Response, &'static str> {
            self.txns += 1;
            let tid = if op_code == op::OPEN_SESSION {
                self.tid = 1;
                0
            } else {
                self.tid += 1;
                self.tid - 1
            };
            let mut cmd = [0u8; 32];
            let n = codec::command(&mut cmd, op_code, tid, params).ok_or("command")?;
            let h = codec::header(&cmd[..n]).ok_or("command header")?;
            if h.kind != ctype::COMMAND || h.code != op_code || h.tid != tid || h.len as usize != n {
                return Err("command container");
            }
            let p: Vec<u32> = cmd[HDR..n].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let (out, sink) = match phase {
                Phase::None => (None, None),
                Phase::Out(d) => {
                    let mut c = alloc::vec![0u8; HDR + d.len()];
                    codec::container(&mut c, ctype::DATA, op_code, tid, d).ok_or("data out")?;
                    let dh = codec::header(&c).ok_or("data out header")?;
                    if dh.kind != ctype::DATA || dh.code != op_code || dh.tid != tid || dh.len as usize != c.len() {
                        return Err("data-out container");
                    }
                    (Some(c.split_off(HDR)), None)
                }
                Phase::In { cap, into } => (None, Some((cap, into))),
            };
            let (code, rparams, data) = self.serve(op_code, &p, out);
            if let (Some(d), Some((cap, into))) = (data, sink) {
                let mut c = alloc::vec![0u8; HDR + d.len()];
                codec::container(&mut c, ctype::DATA, op_code, tid, &d).ok_or("data in")?;
                let dh = codec::header(&c).ok_or("data in header")?;
                if dh.kind != ctype::DATA || dh.code != op_code || dh.tid != tid {
                    return Err("data-in container");
                }
                let body = &c[HDR..];
                let room = cap.saturating_sub(into.len());
                into.extend_from_slice(&body[..body.len().min(room)]);
            }
            let mut pb = [0u8; 20];
            let k = rparams.len().min(5);
            for (i, v) in rparams.iter().take(k).enumerate() {
                pb[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            let mut r = [0u8; 32];
            let rn = codec::container(&mut r, ctype::RESPONSE, code, tid, &pb[..k * 4]).ok_or("response")?;
            codec::response(&r[..rn]).ok_or("response decode")
        }
    }

    /// MTPWRITE: the responder as THE session's phone, so `tests mtpphone` reaches it through the VFS
    /// (`MountTable::new()` -> `fs/ptpfs.rs` -> this module's kernel surface -> `with_transport`). Only when
    /// no PTP/MTP device is known (STATE absent, SLOT 0): `service_ptp` then stays a no-op and `ptp_txn`
    /// refuses, so the controller is never asked. Bring-up's own steps over the responder; the volume name.
    pub fn attach(mut r: Responder) -> Result<String, &'static str> {
        use super::super::{Info, INFO, SLOT, STATE, ST_ABSENT, ST_OPEN, TABLE};
        use core::sync::atomic::Ordering;
        if STATE.load(Ordering::Relaxed) != ST_ABSENT || SLOT.load(Ordering::Relaxed) != 0 || super::FIXTURE.lock().is_some() {
            return Err("a PTP/MTP device is attached");
        }
        let _ = r.txn(op::OPEN_SESSION, &[1], Phase::None)?;
        let mut raw = Vec::new();
        r.txn(op::GET_DEVICE_INFO, &[], Phase::In { cap: 64 << 10, into: &mut raw })?;
        let di = codec::device_info(&raw).ok_or("deviceinfo")?;
        let mut raw = Vec::new();
        r.txn(op::GET_STORAGE_IDS, &[], Phase::In { cap: 4096, into: &mut raw })?;
        let storages = codec::u32_array(&raw).ok_or("storage ids")?;
        let l = super::enumerate(&mut r, &di, &storages)?;
        let model = di.model.clone();
        let volume = super::super::volume_name_of(l.friendly.as_deref().unwrap_or(&model));
        let storage_names: Vec<String> = l.storages.iter().map(|s| s.1.clone()).collect();
        {
            let mut t = TABLE.lock();
            t.handles.clear();
            t.infos.clear();
            t.failed.clear();
        }
        super::install(l, super::super::now_ms(), 0, 0, &volume);
        *INFO.lock() = Some(Info { model, manufacturer: di.manufacturer, serial: di.serial, volume: volume.clone(), storages, storage_names });
        *super::FIXTURE.lock() = Some(r);
        STATE.store(ST_OPEN, Ordering::Relaxed);
        crate::fs::vfs::register_hotplug(crate::fs::vfs::ptpfs::provide);
        let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
        Ok(volume)
    }

    /// MTPWRITE: the session closed as a detach closes it; the responder back for inspection.
    pub fn detach() -> Option<Responder> {
        use super::super::{INFO, OBJ_CACHE, STATE, ST_ABSENT, TABLE};
        use core::sync::atomic::Ordering;
        let r = super::FIXTURE.lock().take()?;
        STATE.store(ST_ABSENT, Ordering::Relaxed);
        *INFO.lock() = None;
        {
            let mut t = TABLE.lock();
            t.handles.clear();
            t.infos.clear();
            t.failed.clear();
        }
        *OBJ_CACHE.lock() = None;
        super::forget();
        let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
        Some(r)
    }

    /// Hand-written OS descriptors: string 0xEE (vendor code 0x54) and an Extended Compat ID naming MTP.
    pub fn os_descriptors() -> ([u8; 18], [u8; 40]) {
        let mut s = [0u8; 18];
        s[0] = 0x12;
        s[1] = 0x03;
        for (i, c) in b"MSFT100".iter().enumerate() {
            s[2 + i * 2] = *c;
        }
        s[16] = 0x54;
        let mut x = [0u8; 40];
        x[0] = 40;
        x[4] = 0x00;
        x[5] = 0x01; // bcdVersion 1.00
        x[6] = 4; // wIndex = extended compat ID
        x[8] = 1; // one function section
        x[16] = 0; // bFirstInterfaceNumber
        x[17] = 1;
        x[18..21].copy_from_slice(b"MTP");
        (s, x)
    }
}
