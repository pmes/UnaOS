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

//! PTPCAM (rmbp-ledger B687, R130 law 3 "THE USER'S HARDWARE IS THE USER'S") — the USB Still Image
//! class (bInterfaceClass 0x06 / 0x01 / 0x01, PTP = ISO 15740 / PIMA 15740:2000) on the shared xHCI
//! driver: every camera's card over PTP/MTP, and Sony's PC-Remote vendor ops (live view, shutter).
//! Design and the opcode table with a source per opcode: `docs/dev/OS/07_USB_STORAGE/ptp.md`.
//!
//! SHAPE, `usbnet.rs`'s. The controller owns the bulk pair (`configure_endpoints` /
//! `configure_bulk_endpoints_sync` — the storage path's own Configure-Endpoint) and the slot's 32 KiB
//! 64 KiB-aligned `scsi_data_buffer` (a PTP slot never runs BOT, so the buffer is free). THIS module owns
//! the device identity, the walk notes, the one-outstanding transfer arm/claim, the interrupt-IN EVENT
//! pipe (its own ring + 64-byte buffer, added by a second Configure-Endpoint), the session and the object
//! table. Every site in `mod.rs` is a same-line `#[cfg(feature = "uvc")]` fold (the camera knob: UVCCAM's
//! isochronous path and this class ride the same `UNAOS_UVC=1`), so the knob-off image is unchanged.
//!
//! TRANSPORT (USB Still Image Capture Device Definition 1.0 §7; PTP §9.3): a transaction is a COMMAND
//! container on bulk-OUT, an optional DATA container (either direction), then a RESPONSE container on
//! bulk-IN. Container = u32 length, u16 type (1 command, 2 data, 3 response, 4 event), u16 code, u32
//! TransactionID, payload — all little-endian. OpenSession carries TransactionID 0; the session's
//! transactions count from 1. A data phase whose length is a multiple of wMaxPacketSize ends with a
//! zero-length packet; a 0-byte completion where a container was expected is that ZLP and is read past.
//!
//! WIRE: `[ptp] candidate slot= dev=vid:pid iface= in= out= int= hub=` · `[ptp] dev=<vid:pid>
//! model=<name> storages=<n> objects=<n>` (M1) · `[ptp] sony sdio=connected ext=0x.. props= controls=
//! priority=` · `[ptp] objects listed n= dirs= files=` · `[ptp] volume /volumes/<name> mounted` ·
//! `[ptp] liveview frame=<n> bytes= jpeg= ms=` · `[ptp] shutter …` · `[ptp] event code= p1=` ·
//! `[ptp] transport fail …` · `:: PTP: codec= deviceinfo= stream= -> PASS|FAIL ::` (`tests ptp`).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use super::trb::Trb;
use super::{dma_coherency, ring, XhciController};
use crate::sync::Mutex;

/// MTPPHONE (rmbp-ledger B715): MTP, the PTP superset a phone speaks — a child module so it reads this
/// file's session, table and transport without widening them, and `drivers/xhci/mod.rs` is untouched.
#[path = "mtp.rs"]
pub mod mtp;

// ── The numbers, each with its source (docs/dev/OS/07_USB_STORAGE/ptp.md carries the table) ─────────

/// Operation codes. 0x1xxx: ISO 15740 §10.4 (libgphoto2 `camlibs/ptp2/ptp.h` `PTP_OC_*`, same values).
/// 0x92xx: Sony's SDIO vendor set, libgphoto2 `ptp.h` `PTP_OC_SONY_SDIO_*` (lines 702-714 at master).
pub mod op {
    pub const GET_DEVICE_INFO: u16 = 0x1001;
    pub const OPEN_SESSION: u16 = 0x1002;
    pub const CLOSE_SESSION: u16 = 0x1003;
    pub const GET_STORAGE_IDS: u16 = 0x1004;
    pub const GET_OBJECT_HANDLES: u16 = 0x1007;
    pub const GET_OBJECT_INFO: u16 = 0x1008;
    pub const GET_OBJECT: u16 = 0x1009;
    pub const GET_PARTIAL_OBJECT: u16 = 0x101B;
    /// `PTP_OC_SONY_SDIO_Connect` — the PC-Remote handshake, phases 1, 2, 3 (data-IN, discarded).
    pub const SONY_SDIO_CONNECT: u16 = 0x9201;
    /// `PTP_OC_SONY_SDIO_GetExtDeviceInfo` — the vendor property and control code lists.
    pub const SONY_SDIO_GET_EXT_DEVICE_INFO: u16 = 0x9202;
    /// `PTP_OC_SONY_SDIO_SetExtDevicePropValue` — libgphoto2's `setdevicecontrolvaluea`.
    pub const SONY_SDIO_SET_EXT_DEVICE_PROP_VALUE: u16 = 0x9205;
    /// `PTP_OC_SONY_SDIO_ControlDevice` — libgphoto2's `setdevicecontrolvalueb` (the shutter buttons).
    pub const SONY_SDIO_CONTROL_DEVICE: u16 = 0x9207;
}

/// Response codes, ISO 15740 §11 (`PTP_RC_*`).
pub mod rc {
    pub const OK: u16 = 0x2001;
    pub const SESSION_NOT_OPEN: u16 = 0x2003;
    pub const INVALID_OBJECT_HANDLE: u16 = 0x2009;
    pub const ACCESS_DENIED: u16 = 0x200F;
    pub const DEVICE_BUSY: u16 = 0x2019;
    pub const SESSION_ALREADY_OPEN: u16 = 0x201E;
}

/// Event codes, ISO 15740 §12 (`PTP_EC_*`); 0xC2xx libgphoto2 `ptp.h` `PTP_EC_Sony_*` (lines 1351-1353).
pub mod ev {
    pub const OBJECT_ADDED: u16 = 0x4002;
    pub const OBJECT_REMOVED: u16 = 0x4003;
    pub const STORE_ADDED: u16 = 0x4004;
    pub const STORE_REMOVED: u16 = 0x4005;
    pub const STORAGE_INFO_CHANGED: u16 = 0x400C;
    pub const SONY_OBJECT_ADDED: u16 = 0xC201;
    pub const SONY_OBJECT_REMOVED: u16 = 0xC202;
}

/// Object format codes, ISO 15740 §6.2 (`PTP_OFC_*`); 0xB101 libgphoto2 `PTP_OFC_SONY_RAW` (ARW).
pub mod fmt {
    pub const ASSOCIATION: u16 = 0x3001;
    pub const EXIF_JPEG: u16 = 0x3801;
    pub const SONY_ARW: u16 = 0xB101;
}

/// Sony PC-Remote facts, every one libgphoto2's (`ptp.h` / `ptp.c` / `library.c` at master).
pub mod sony {
    /// `PTP_VENDOR_SONY` (ptp.h:209).
    pub const VENDOR_EXT: u32 = 0x0000_0011;
    /// `PTP_VENDOR_MTP` — Sony in MTP mode reports it with Manufacturer "Sony Corporation" (library.c fixup).
    pub const VENDOR_EXT_MTP: u32 = 0x0000_0006;
    /// Sony's USB vendor id.
    pub const VID: u16 = 0x054C;
    /// `SONY_PROTOCOL_3_00` (ptp.c:4311): GetExtDeviceInfo's first parameter; an older body answers its own (0xC8).
    pub const PROTOCOL_3_00: u32 = 0x12C;
    /// `PTP_DPC_SONY_PriorityMode` (ptp.h:3580), INT8 1 = application (library.c, after SDIO_Connect phase 3).
    pub const PRIORITY_MODE: u16 = 0xD25A;
    /// `PTP_DPC_SONY_ShutterHalfRelease` (ptp.h:3607): UINT16 2 = press, 1 = release.
    pub const SHUTTER_HALF: u16 = 0xD2C1;
    /// `PTP_DPC_SONY_ShutterRelease` (ptp.h:3608): UINT16 2 = press, 1 = release.
    pub const SHUTTER_FULL: u16 = 0xD2C2;
    /// The live-view object (library.c `preview_object = 0xffffc002`): GetObjectInfo, then GetObject.
    pub const LIVEVIEW_HANDLE: u32 = 0xFFFF_C002;
    /// The just-captured image held in the camera's RAM (library.c `0xffffc001`).
    pub const CAPTURED_HANDLE: u32 = 0xFFFF_C001;
}

/// Container types (Still Image class §7.1.1).
pub mod ctype {
    pub const COMMAND: u16 = 1;
    pub const DATA: u16 = 2;
    pub const RESPONSE: u16 = 3;
    pub const EVENT: u16 = 4;
}

// ── The codec: pure, no I/O — `tests ptp` runs it over fixtures ──────────────────────────────────────
pub mod codec {
    use super::ctype;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// The container header: u32 length, u16 type, u16 code, u32 TransactionID.
    pub const HDR: usize = 12;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Header {
        pub len: u32,
        pub kind: u16,
        pub code: u16,
        pub tid: u32,
    }

    pub fn header(b: &[u8]) -> Option<Header> {
        if b.len() < HDR {
            return None;
        }
        Some(Header {
            len: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            kind: u16::from_le_bytes([b[4], b[5]]),
            code: u16::from_le_bytes([b[6], b[7]]),
            tid: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        })
    }

    /// Write a container (header + `payload`) into `out`; the length written, or `None` if it does not fit.
    pub fn container(out: &mut [u8], kind: u16, code: u16, tid: u32, payload: &[u8]) -> Option<usize> {
        let n = HDR + payload.len();
        if out.len() < n {
            return None;
        }
        out[0..4].copy_from_slice(&(n as u32).to_le_bytes());
        out[4..6].copy_from_slice(&kind.to_le_bytes());
        out[6..8].copy_from_slice(&code.to_le_bytes());
        out[8..12].copy_from_slice(&tid.to_le_bytes());
        out[HDR..n].copy_from_slice(payload);
        Some(n)
    }

    /// A COMMAND container: up to five u32 parameters (ISO 15740 §9.3.2).
    pub fn command(out: &mut [u8], code: u16, tid: u32, params: &[u32]) -> Option<usize> {
        let k = params.len().min(5);
        let mut p = [0u8; 20];
        for (i, v) in params.iter().take(k).enumerate() {
            p[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        container(out, ctype::COMMAND, code, tid, &p[..k * 4])
    }

    /// A RESPONSE (or EVENT) container's code, TransactionID and parameters.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Response {
        pub code: u16,
        pub tid: u32,
        pub params: [u32; 5],
        pub n: usize,
    }

    fn with_params(b: &[u8], want: u16) -> Option<Response> {
        let h = header(b)?;
        if h.kind != want || (h.len as usize) < HDR || (h.len as usize) > b.len() {
            return None;
        }
        let mut params = [0u32; 5];
        let n = ((h.len as usize - HDR) / 4).min(5);
        for (i, p) in params.iter_mut().enumerate().take(n) {
            let o = HDR + i * 4;
            *p = u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        }
        Some(Response { code: h.code, tid: h.tid, params, n })
    }

    pub fn response(b: &[u8]) -> Option<Response> {
        with_params(b, ctype::RESPONSE)
    }

    /// An EVENT container on the interrupt pipe (up to three parameters, §12.1).
    pub fn event(b: &[u8]) -> Option<Response> {
        with_params(b, ctype::EVENT)
    }

    /// A little-endian dataset cursor (ISO 15740 §5.3: integers LE; String = u8 count of UCS-2 units
    /// INCLUDING the NUL, 0 = empty; arrays = u32 count then elements).
    pub struct Cur<'a> {
        b: &'a [u8],
        at: usize,
    }

    impl<'a> Cur<'a> {
        pub fn new(b: &'a [u8]) -> Self {
            Self { b, at: 0 }
        }
        pub fn at(&self) -> usize {
            self.at
        }
        pub fn u8(&mut self) -> Option<u8> {
            let v = *self.b.get(self.at)?;
            self.at += 1;
            Some(v)
        }
        pub fn u16(&mut self) -> Option<u16> {
            let s = self.b.get(self.at..self.at + 2)?;
            self.at += 2;
            Some(u16::from_le_bytes([s[0], s[1]]))
        }
        pub fn u32(&mut self) -> Option<u32> {
            let s = self.b.get(self.at..self.at + 4)?;
            self.at += 4;
            Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        }
        /// MTPPHONE: UINT64 (an MTP ObjectSize, a StorageInfo capacity).
        pub fn u64(&mut self) -> Option<u64> {
            let lo = self.u32()? as u64;
            let hi = self.u32()? as u64;
            Some(lo | (hi << 32))
        }
        /// MTPPHONE: read past `n` bytes (a property value this reader does not keep).
        pub fn skip(&mut self, n: usize) -> Option<()> {
            let end = self.at.checked_add(n)?;
            if end > self.b.len() {
                return None;
            }
            self.at = end;
            Some(())
        }
        pub fn string(&mut self) -> Option<String> {
            let n = self.u8()? as usize;
            let mut s = String::new();
            for _ in 0..n {
                let u = self.u16()?;
                if u == 0 {
                    continue;
                }
                s.push(char::from_u32(u as u32).unwrap_or('?'));
            }
            Some(s)
        }
        pub fn au16(&mut self) -> Option<Vec<u16>> {
            let n = self.u32()? as usize;
            if n > (self.b.len().saturating_sub(self.at)) / 2 {
                return None;
            }
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(self.u16()?);
            }
            Some(v)
        }
        pub fn au32(&mut self) -> Option<Vec<u32>> {
            let n = self.u32()? as usize;
            if n > (self.b.len().saturating_sub(self.at)) / 4 {
                return None;
            }
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(self.u32()?);
            }
            Some(v)
        }
    }

    /// The DeviceInfo dataset (ISO 15740 §5.5.1), field for field.
    #[derive(Clone, Debug, Default)]
    pub struct DeviceInfo {
        pub std_version: u16,
        pub vendor_ext: u32,
        pub vendor_ext_version: u16,
        pub vendor_desc: String,
        pub functional_mode: u16,
        pub ops: Vec<u16>,
        pub events: Vec<u16>,
        pub props: Vec<u16>,
        pub capture_formats: Vec<u16>,
        pub image_formats: Vec<u16>,
        pub manufacturer: String,
        pub model: String,
        pub device_version: String,
        pub serial: String,
    }

    pub fn device_info(p: &[u8]) -> Option<DeviceInfo> {
        let mut c = Cur::new(p);
        Some(DeviceInfo {
            std_version: c.u16()?,
            vendor_ext: c.u32()?,
            vendor_ext_version: c.u16()?,
            vendor_desc: c.string()?,
            functional_mode: c.u16()?,
            ops: c.au16()?,
            events: c.au16()?,
            props: c.au16()?,
            capture_formats: c.au16()?,
            image_formats: c.au16()?,
            manufacturer: c.string()?,
            model: c.string()?,
            device_version: c.string()?,
            serial: c.string().unwrap_or_default(), // some bodies end the dataset before SerialNumber
        })
    }

    /// A PTP DateTime string, "YYYYMMDDThhmmss[.s][Z|±hhmm]" (ISO 15740 §5.3.5).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct DateTime {
        pub y: u16,
        pub mo: u8,
        pub d: u8,
        pub h: u8,
        pub mi: u8,
        pub s: u8,
    }

    pub fn date_time(s: &str) -> Option<DateTime> {
        let b = s.as_bytes();
        if b.len() < 15 || b[8] != b'T' {
            return None;
        }
        let num = |r: core::ops::Range<usize>| -> Option<u32> {
            let mut v = 0u32;
            for &c in &b[r] {
                if !c.is_ascii_digit() {
                    return None;
                }
                v = v * 10 + (c - b'0') as u32;
            }
            Some(v)
        };
        let dt = DateTime {
            y: num(0..4)? as u16,
            mo: num(4..6)? as u8,
            d: num(6..8)? as u8,
            h: num(9..11)? as u8,
            mi: num(11..13)? as u8,
            s: num(13..15)? as u8,
        };
        if dt.mo == 0 || dt.mo > 12 || dt.d == 0 || dt.d > 31 || dt.h > 23 || dt.mi > 59 || dt.s > 60 {
            return None;
        }
        Some(dt)
    }

    /// Unix seconds of a (UTC-read) DateTime — days-from-civil (Hinnant).
    pub fn unix_secs(t: &DateTime) -> u64 {
        let (y, m) = if t.mo <= 2 { (t.y as i64 - 1, t.mo as i64 + 9) } else { (t.y as i64, t.mo as i64 - 3) };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + t.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        (days * 86_400 + t.h as i64 * 3600 + t.mi as i64 * 60 + t.s as i64).max(0) as u64
    }

    /// The ObjectInfo dataset (ISO 15740 §5.5.2): the fields the volume needs.
    #[derive(Clone, Debug)]
    pub struct ObjectInfo {
        pub storage: u32,
        pub format: u16,
        pub size: u32,
        pub parent: u32,
        pub assoc_type: u16,
        pub filename: String,
        pub capture: Option<DateTime>,
        pub modified: Option<DateTime>,
    }

    pub fn object_info(p: &[u8]) -> Option<ObjectInfo> {
        let mut c = Cur::new(p);
        let storage = c.u32()?;
        let format = c.u16()?;
        let _protection = c.u16()?;
        let size = c.u32()?;
        let _thumb_format = c.u16()?;
        let _thumb_size = c.u32()?;
        let _thumb_w = c.u32()?;
        let _thumb_h = c.u32()?;
        let _image_w = c.u32()?;
        let _image_h = c.u32()?;
        let _bit_depth = c.u32()?;
        let parent = c.u32()?;
        let assoc_type = c.u16()?;
        let _assoc_desc = c.u32()?;
        let _sequence = c.u32()?;
        let filename = c.string()?;
        let capture = c.string().and_then(|s| date_time(&s));
        let modified = c.string().and_then(|s| date_time(&s));
        Some(ObjectInfo { storage, format, size, parent, assoc_type, filename, capture, modified })
    }

    /// A bare AUINT32 dataset (GetStorageIDs, GetObjectHandles).
    pub fn u32_array(p: &[u8]) -> Option<Vec<u32>> {
        Cur::new(p).au32()
    }

    /// Sony SDIO_GetExtDeviceInfo's dataset (ptp.c `ptp_sony_get_vendorpropcodes`): u16 version, then two
    /// AUINT16 lists — device properties, then controls.
    pub fn sony_ext_info(p: &[u8]) -> Option<(u16, Vec<u16>, Vec<u16>)> {
        let mut c = Cur::new(p);
        let v = c.u16()?;
        let props = c.au16()?;
        let controls = c.au16().unwrap_or_default();
        Some((v, props, controls))
    }

    /// The JPEG inside a Sony live-view object (library.c `camera_capture_preview`, PTP_VENDOR_SONY case):
    /// the first u32 is the offset of the JPEG when it lands on an SOI; else the first FF D8; the frame
    /// ends after the first FF D9 that follows (`save_jpeg_in_data_to_preview`). Returns (start, end).
    pub fn liveview_jpeg(b: &[u8]) -> Option<(usize, usize)> {
        let mut start = None;
        if b.len() > 4 {
            let off = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
            if off + 1 < b.len() && b[off] == 0xFF && b[off + 1] == 0xD8 {
                start = Some(off);
            }
        }
        let start = match start {
            Some(s) => s,
            None => (0..b.len().saturating_sub(1)).find(|&i| b[i] == 0xFF && b[i + 1] == 0xD8)?,
        };
        let end = (start + 2..b.len().saturating_sub(1)).find(|&i| b[i] == 0xFF && b[i + 1] == 0xD9)? + 2;
        Some((start, end))
    }

    // ── fixtures ──

    /// A Sony-shaped DeviceInfo DATA container, written by hand from §5.5.1 (109 bytes): vendor 0x11,
    /// nine operations (the session set, GetObject, and Sony's 0x9201/0x9207), event 0xC201, property
    /// 0xD2C1, image formats JPEG + ARW, Manufacturer "Sony", Model "ILCE-7M3", DeviceVersion "1.0".
    pub const FIXTURE_DEVICE_INFO: [u8; 109] = [
        0x6D, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x10, 0x01, 0x00, 0x00, 0x00, // header: len 109, DATA, 0x1001, tid 1
        0x64, 0x00, // StandardVersion 100
        0x11, 0x00, 0x00, 0x00, // VendorExtensionID 0x11 (Sony)
        0x64, 0x00, // VendorExtensionVersion
        0x00, // VendorExtensionDesc ""
        0x00, 0x00, // FunctionalMode
        0x09, 0x00, 0x00, 0x00, 0x01, 0x10, 0x02, 0x10, 0x03, 0x10, 0x04, 0x10, 0x07, 0x10, 0x08, 0x10, 0x09, 0x10, 0x01,
        0x92, 0x07, 0x92, // OperationsSupported[9]
        0x01, 0x00, 0x00, 0x00, 0x01, 0xC2, // EventsSupported[1] = 0xC201
        0x01, 0x00, 0x00, 0x00, 0xC1, 0xD2, // DevicePropertiesSupported[1] = 0xD2C1
        0x00, 0x00, 0x00, 0x00, // CaptureFormats[0]
        0x02, 0x00, 0x00, 0x00, 0x01, 0x38, 0x01, 0xB1, // ImageFormats[2] = 0x3801, 0xB101
        0x05, b'S', 0, b'o', 0, b'n', 0, b'y', 0, 0, 0, // Manufacturer "Sony"
        0x09, b'I', 0, b'L', 0, b'C', 0, b'E', 0, b'-', 0, b'7', 0, b'M', 0, b'3', 0, 0, 0, // Model "ILCE-7M3"
        0x04, b'1', 0, b'.', 0, b'0', 0, 0, 0, // DeviceVersion "1.0"
        0x00, // SerialNumber ""
    ];

    /// The codec leg: a command encodes to the bytes the spec gives, a response and an event decode, an
    /// ObjectInfo round-trips its name/format/parent/date, and the live-view JPEG is found both ways.
    pub fn selftest_codec() -> Result<(), &'static str> {
        let mut b = [0u8; 32];
        let n = command(&mut b, super::op::OPEN_SESSION, 0, &[1]).ok_or("command")?;
        if b[..n] != [0x10, 0, 0, 0, 0x01, 0, 0x02, 0x10, 0, 0, 0, 0, 0x01, 0, 0, 0] {
            return Err("command bytes");
        }
        let r = response(&[0x10, 0, 0, 0, 0x03, 0, 0x01, 0x20, 0x07, 0, 0, 0, 0x2A, 0, 0, 0]).ok_or("response")?;
        if r.code != super::rc::OK || r.tid != 7 || r.n != 1 || r.params[0] != 42 {
            return Err("response fields");
        }
        if response(&b[..n]).is_some() {
            return Err("command read as response");
        }
        let e = event(&[0x10, 0, 0, 0, 0x04, 0, 0x01, 0xC2, 0, 0, 0, 0, 0x34, 0x12, 0, 0]).ok_or("event")?;
        if e.code != super::ev::SONY_OBJECT_ADDED || e.params[0] != 0x1234 {
            return Err("event fields");
        }
        // ObjectInfo: storage 0x00010001, ARW, 24 MB, parent 7, "DSC00001.ARW", captured 2026-10-19 12:34:56.
        let mut oi: Vec<u8> = Vec::new();
        oi.extend_from_slice(&0x0001_0001u32.to_le_bytes());
        oi.extend_from_slice(&super::fmt::SONY_ARW.to_le_bytes());
        oi.extend_from_slice(&0u16.to_le_bytes());
        oi.extend_from_slice(&24_000_000u32.to_le_bytes());
        oi.extend_from_slice(&[0u8; 2 + 4 * 7]); // thumb format/size/w/h, image w/h, bit depth
        oi.extend_from_slice(&7u32.to_le_bytes()); // parent
        oi.extend_from_slice(&[0u8; 2 + 4 + 4]); // association type/desc, sequence
        for s in ["DSC00001.ARW", "20261019T123456", ""] {
            if s.is_empty() {
                oi.push(0);
                continue;
            }
            oi.push(s.len() as u8 + 1);
            for c in s.bytes() {
                oi.extend_from_slice(&[c, 0]);
            }
            oi.extend_from_slice(&[0, 0]);
        }
        let o = object_info(&oi).ok_or("objectinfo")?;
        if o.filename != "DSC00001.ARW" || o.format != super::fmt::SONY_ARW || o.parent != 7 || o.size != 24_000_000 {
            return Err("objectinfo fields");
        }
        let t = o.capture.ok_or("objectinfo date")?;
        if unix_secs(&t) != 1_792_413_296 {
            return Err("objectinfo unix time");
        }
        // Live view: an 8-byte header whose u32 is the JPEG offset, then SOI .. EOI, then trailing bytes.
        let lv = [8u8, 0, 0, 0, 0xFF, 0xD8, 0, 0, 0xFF, 0xD8, 0x11, 0x22, 0xFF, 0xD9, 0x33];
        if liveview_jpeg(&lv) != Some((8, 14)) {
            return Err("liveview offset");
        }
        let lv2 = [0x99u8, 0x99, 0x99, 0x99, 0x00, 0xFF, 0xD8, 0x44, 0xFF, 0xD9];
        if liveview_jpeg(&lv2) != Some((5, 10)) {
            return Err("liveview scan");
        }
        Ok(())
    }

    /// The DeviceInfo leg over [`FIXTURE_DEVICE_INFO`].
    pub fn selftest_deviceinfo() -> Result<(), &'static str> {
        let b = &FIXTURE_DEVICE_INFO[..];
        let h = header(b).ok_or("header")?;
        if h.kind != ctype::DATA || h.code != super::op::GET_DEVICE_INFO || h.len as usize != b.len() {
            return Err("container");
        }
        let d = device_info(&b[HDR..]).ok_or("dataset")?;
        if d.vendor_ext != super::sony::VENDOR_EXT || d.ops.len() != 9 || !d.ops.contains(&super::op::SONY_SDIO_CONNECT) {
            return Err("operations");
        }
        if d.events != [super::ev::SONY_OBJECT_ADDED] || d.props != [super::sony::SHUTTER_HALF] {
            return Err("events/props");
        }
        if d.image_formats != [super::fmt::EXIF_JPEG, super::fmt::SONY_ARW] {
            return Err("formats");
        }
        if d.manufacturer != "Sony" || d.model != "ILCE-7M3" || d.device_version != "1.0" || !d.serial.is_empty() {
            return Err("strings");
        }
        Ok(())
    }
}

// ── State ──────────────────────────────────────────────────────────────────────────────────────────
const ST_ABSENT: u8 = 0;
const ST_CANDIDATE: u8 = 1; // the walk saw a PTP interface with a bulk pair
const ST_CONFIGURING: u8 = 2; // root path: Configure-Endpoint issued, completion pending
const ST_CONFIGURED: u8 = 3; // endpoints live; session bring-up pending (service pass)
const ST_OPEN: u8 = 4; // session open; the volume is mounted
const ST_FAILED: u8 = 5; // named on the wire once; replug to retry
const ST_VENDOR: u8 = 6; // MTPPHONE: a root-port vendor-class (0xFF/0xFF/0x00) candidate awaiting its OS-descriptor probe (service pass)

static STATE: AtomicU8 = AtomicU8::new(ST_ABSENT);
static SLOT: AtomicU8 = AtomicU8::new(0);
static VID: AtomicU16 = AtomicU16::new(0);
static PID: AtomicU16 = AtomicU16::new(0);
static HUBBED: AtomicBool = AtomicBool::new(false);
static PENDING_CFG: AtomicU8 = AtomicU8::new(0);
// The walk (one configuration at a time).
static CFG_VALUE: AtomicU8 = AtomicU8::new(1);
static IN_PTP: AtomicBool = AtomicBool::new(false); // the interface being walked is class 0x06/0x01/0x01
static VENDOR: AtomicBool = AtomicBool::new(false); // MTPPHONE: the taken interface is vendor-class 0xFF/0xFF/0x00 (MTP only by its OS descriptor)
static IFACE: AtomicU8 = AtomicU8::new(0xFF);
static IN_EP: AtomicU8 = AtomicU8::new(0);
static IN_MPS: AtomicU16 = AtomicU16::new(0);
static OUT_EP: AtomicU8 = AtomicU8::new(0);
static OUT_MPS: AtomicU16 = AtomicU16::new(0);
static INT_EP: AtomicU8 = AtomicU8::new(0);
static INT_MPS: AtomicU16 = AtomicU16::new(0);
static INT_INTERVAL: AtomicU8 = AtomicU8::new(0);
// The session.
static TID: AtomicU32 = AtomicU32::new(0);
static SONY_READY: AtomicBool = AtomicBool::new(false);
static PARTIAL_OK: AtomicBool = AtomicBool::new(false); // DeviceInfo lists GetPartialObject
static REFRESH: AtomicBool = AtomicBool::new(false); // an object/store event asked for a re-list
static FILL_SAID: AtomicBool = AtomicBool::new(false);
// The one outstanding bulk transfer (arm/claim, `usbnet`'s protocol).
static XF_ARMED: AtomicBool = AtomicBool::new(false);
static XF_DCI: AtomicU8 = AtomicU8::new(0);
static XF_TRB: AtomicU64 = AtomicU64::new(0);
static XF_DONE: AtomicBool = AtomicBool::new(false);
static XF_CODE: AtomicU8 = AtomicU8::new(0);
static XF_RES: AtomicU32 = AtomicU32::new(0);
// The interrupt-IN event pipe.
static EV_ARMED: AtomicBool = AtomicBool::new(false);
static EV_DCI: AtomicU8 = AtomicU8::new(0);
static EV_TRB: AtomicU64 = AtomicU64::new(0);
static EV_DONE: AtomicBool = AtomicBool::new(false);
static EV_CODE: AtomicU8 = AtomicU8::new(0);
static EV_RES: AtomicU32 = AtomicU32::new(0);
static EV_BUF: AtomicUsize = AtomicUsize::new(0); // 64-byte, 64-aligned, allocated once and kept
static EV_SEEN: AtomicU32 = AtomicU32::new(0);
static EV_RING: Mutex<Option<ring::TransferRing>> = Mutex::new(None);
// Live view.
static LV_FRAMES: AtomicU64 = AtomicU64::new(0);
static LV_SAID: AtomicU64 = AtomicU64::new(0);

/// What the session learned at bring-up.
struct Info {
    model: String,
    manufacturer: String,
    serial: String,
    volume: String,
    storages: Vec<u32>,
    /// MTPPHONE: an MTP session's storage folder names (always top-level folders); empty for PTP.
    storage_names: Vec<String>,
}
static INFO: Mutex<Option<Info>> = Mutex::new(None);

/// One object of the card, from GetObjectInfo.
#[derive(Clone)]
pub struct Obj {
    pub handle: u32,
    pub storage: u32,
    pub parent: u32,
    pub format: u16,
    pub size: u64,
    pub name: String,
    pub mtime: Option<codec::DateTime>,
}
impl Obj {
    pub fn is_dir(&self) -> bool {
        self.format == fmt::ASSOCIATION
    }
}

/// The object table: every handle GetObjectHandles returned, its info once fetched (filled a chunk per
/// service pass so no listing ever waits on the cable).
struct Table {
    handles: Vec<u32>,
    infos: BTreeMap<u32, Obj>,
    failed: BTreeMap<u32, u16>,
}
static TABLE: Mutex<Table> = Mutex::new(Table { handles: Vec::new(), infos: BTreeMap::new(), failed: BTreeMap::new() });
/// One whole object fetched by GetObject, kept for range reads on a body without GetPartialObject.
static OBJ_CACHE: Mutex<Option<(u32, Vec<u8>)>> = Mutex::new(None);
/// The newest live-view JPEG and when it was fetched (ms).
static LV_CACHE: Mutex<Option<(u64, Vec<u8>)>> = Mutex::new(None);

/// Objects a session lists (a card holds thousands; past this the table stops and says so).
const MAX_OBJECTS: usize = 16_384;
/// GetObjectInfo transactions per service pass.
const FILL_PER_PASS: usize = 32;
/// The largest object fetched whole when the body has no GetPartialObject.
const WHOLE_CAP: usize = 128 << 20;
/// One bulk TD: the slot's `scsi_data_buffer` (STORAGE_DATA_BYTES, 64 KiB-aligned: one TRB never crosses
/// a 64 KiB boundary, xHCI 1.2 §4.11.7.1).
const XFER_MAX: usize = super::STORAGE_DATA_BYTES;
/// A response container is at most 32 bytes; one TD of this size takes it as a short packet.
const RESP_REQ: usize = 1024;

pub fn is_open() -> bool {
    STATE.load(Ordering::Relaxed) == ST_OPEN
}
pub fn sony_ready() -> bool {
    SONY_READY.load(Ordering::Relaxed)
}
pub fn vid_pid() -> (u16, u16) {
    (VID.load(Ordering::Relaxed), PID.load(Ordering::Relaxed))
}

// ── The walk (root path: called from the descriptor-completion lines of mod.rs) ────────────────────
fn walk_open() -> bool {
    STATE.load(Ordering::Relaxed) < ST_CONFIGURING
}
fn walk_reset() {
    IN_PTP.store(false, Ordering::Relaxed);
    VENDOR.store(false, Ordering::Relaxed);
    IFACE.store(0xFF, Ordering::Relaxed);
    for a in [&IN_EP, &OUT_EP, &INT_EP] {
        a.store(0, Ordering::Relaxed);
    }
    for a in [&IN_MPS, &OUT_MPS, &INT_MPS] {
        a.store(0, Ordering::Relaxed);
    }
    INT_INTERVAL.store(0, Ordering::Relaxed);
}

/// A DEVICE descriptor arrived. One camera per boot at a time: a session in progress ignores the rest.
pub fn note_device(slot: u8, vid: u16, pid: u16) {
    if !walk_open() {
        return;
    }
    SLOT.store(slot, Ordering::Relaxed);
    VID.store(vid, Ordering::Relaxed);
    PID.store(pid, Ordering::Relaxed);
    STATE.store(ST_ABSENT, Ordering::Relaxed);
    mtp::OS_DESC_MTP.store(false, Ordering::Relaxed);
    walk_reset();
}

/// The configuration descriptor's header: bConfigurationValue (byte 5).
pub fn note_config_header(slot: u8, cfg_value: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || !walk_open() {
        return;
    }
    CFG_VALUE.store(cfg_value, Ordering::Relaxed);
    walk_reset();
}

/// An INTERFACE descriptor. Still Image class = 0x06/0x01/0x01 (USB Still Image Capture Device Definition
/// 1.0 §4.3); the first one found is the one used. MTPPHONE: OR the vendor-class interface 0xFF/0xFF/0x00
/// an Android-style MTP responder exposes — taken only if the Microsoft OS descriptor names it "MTP"
/// (`mtp::mtp_os_probe`, run where synchronous EP0 is legal).
pub fn note_interface(slot: u8, iface: u8, class: u8, sub: u8, proto: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || !walk_open() {
        return;
    }
    let first = IFACE.load(Ordering::Relaxed) == 0xFF;
    let still = class == 0x06 && sub == 0x01 && proto == 0x01;
    let vendor = class == 0xFF && sub == 0xFF && proto == 0x00;
    let is = (still || vendor) && first;
    IN_PTP.store(is, Ordering::Relaxed);
    if is {
        IFACE.store(iface, Ordering::Relaxed);
        VENDOR.store(vendor, Ordering::Relaxed);
    }
}

/// An ENDPOINT descriptor (bEndpointAddress, bmAttributes, wMaxPacketSize, bInterval) inside the PTP
/// interface: the bulk pair and the interrupt-IN event pipe (§7: "one bulk-in, one bulk-out, one interrupt").
pub fn note_ep(slot: u8, addr: u8, attr: u8, mps_raw: u16, interval: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || !walk_open() || !IN_PTP.load(Ordering::Relaxed) {
        return;
    }
    let mps = mps_raw & 0x07FF;
    match (attr & 0x03, addr & 0x80 != 0) {
        (0x02, true) if IN_EP.load(Ordering::Relaxed) == 0 => {
            IN_EP.store(addr, Ordering::Relaxed);
            IN_MPS.store(mps, Ordering::Relaxed);
        }
        (0x02, false) if OUT_EP.load(Ordering::Relaxed) == 0 => {
            OUT_EP.store(addr, Ordering::Relaxed);
            OUT_MPS.store(mps, Ordering::Relaxed);
        }
        (0x03, true) if INT_EP.load(Ordering::Relaxed) == 0 => {
            INT_EP.store(addr, Ordering::Relaxed);
            INT_MPS.store(mps, Ordering::Relaxed);
            INT_INTERVAL.store(interval, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// The walk ended: the bulk pair to configure when it found a PTP interface with one.
fn walk_candidate(slot: u8) -> Option<(u8, u16, u8, u16)> {
    if SLOT.load(Ordering::Relaxed) != slot || !walk_open() || IFACE.load(Ordering::Relaxed) == 0xFF {
        return None;
    }
    let (ia, oa) = (IN_EP.load(Ordering::Relaxed), OUT_EP.load(Ordering::Relaxed));
    if ia == 0 || oa == 0 {
        serial_println!("[ptp] interface {} on slot {} has no bulk pair (in={:#04x} out={:#04x}) — not taken", IFACE.load(Ordering::Relaxed), slot, ia, oa);
        return None;
    }
    if VENDOR.load(Ordering::Relaxed) && INT_EP.load(Ordering::Relaxed) == 0 {
        return None; // MTPPHONE: a vendor interface without MTP's interrupt-IN event pipe is not an MTP candidate
    }
    STATE.store(ST_CANDIDATE, Ordering::Relaxed);
    serial_println!(
        "[ptp] candidate slot={} dev={:04x}:{:04x} iface={} in={:#04x}/{} out={:#04x}/{} int={:#04x}/{} hub={}",
        slot, VID.load(Ordering::Relaxed), PID.load(Ordering::Relaxed), IFACE.load(Ordering::Relaxed),
        ia, IN_MPS.load(Ordering::Relaxed), oa, OUT_MPS.load(Ordering::Relaxed),
        INT_EP.load(Ordering::Relaxed), INT_MPS.load(Ordering::Relaxed), HUBBED.load(Ordering::Relaxed) as u8
    );
    Some((ia, IN_MPS.load(Ordering::Relaxed), oa, OUT_MPS.load(Ordering::Relaxed)))
}

/// Root path: the Configure-Endpoint completion for our slot? Consumes the mark (mod.rs then advances
/// the port queue).
pub fn cfg_done(slot: u8) -> bool {
    if slot != 0 && PENDING_CFG.load(Ordering::Relaxed) == slot {
        PENDING_CFG.store(0, Ordering::Relaxed);
        STATE.store(ST_CONFIGURED, Ordering::Relaxed);
        crate::bootlog_println!("xHCI: PTP endpoints configured (slot {}); session bring-up pending.", slot);
        return true;
    }
    false
}

/// The slot went away (port change / reset): forget everything, withdraw the volume.
pub fn disconnect(slot: u8) {
    if slot == 0 || SLOT.load(Ordering::Relaxed) != slot {
        return;
    }
    let was = STATE.swap(ST_ABSENT, Ordering::Relaxed);
    SLOT.store(0, Ordering::Relaxed);
    PENDING_CFG.store(0, Ordering::Relaxed);
    SONY_READY.store(false, Ordering::Relaxed);
    PARTIAL_OK.store(false, Ordering::Relaxed);
    REFRESH.store(false, Ordering::Relaxed);
    FILL_SAID.store(false, Ordering::Relaxed);
    HUBBED.store(false, Ordering::Relaxed);
    XF_ARMED.store(false, Ordering::Relaxed);
    EV_ARMED.store(false, Ordering::Relaxed);
    mtp::forget();
    *EV_RING.lock() = None; // the slot is disabled with it; the controller holds no pointer into it now
    let vol = INFO.lock().take().map(|i| i.volume);
    {
        let mut t = TABLE.lock();
        t.handles.clear();
        t.infos.clear();
        t.failed.clear();
    }
    *OBJ_CACHE.lock() = None;
    *LV_CACHE.lock() = None;
    super::uvc::disarm_ptp();
    walk_reset();
    if was >= ST_CONFIGURED {
        let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
        serial_println!("[ptp] camera gone slot={} -> /volumes/{} withdrawn", slot, vol.as_deref().unwrap_or("-"));
    }
}

/// Event-ring hook (mod.rs, beside the usbnet/isoch claims): our bulk TD's completion or the event pipe's.
/// An error on our armed endpoint is ours whatever TRB it names.
pub fn claim(slot: u8, dci: u8, param: u64, code: u8, residue: u32) -> bool {
    if slot == 0 || SLOT.load(Ordering::Relaxed) != slot {
        return false;
    }
    let err = code != 1 && code != 13;
    if XF_ARMED.load(Ordering::Relaxed) && dci == XF_DCI.load(Ordering::Relaxed) && (param == XF_TRB.load(Ordering::Relaxed) || err) {
        if !XF_DONE.swap(true, Ordering::AcqRel) {
            XF_CODE.store(code, Ordering::Relaxed);
            XF_RES.store(residue, Ordering::Relaxed);
        }
        return true;
    }
    if EV_ARMED.load(Ordering::Relaxed) && dci == EV_DCI.load(Ordering::Relaxed) && (param == EV_TRB.load(Ordering::Relaxed) || err) {
        if !EV_DONE.swap(true, Ordering::AcqRel) {
            EV_CODE.store(code, Ordering::Relaxed);
            EV_RES.store(residue, Ordering::Relaxed);
        }
        return true;
    }
    false
}

/// A transaction's data phase.
pub enum Phase<'a> {
    None,
    Out(&'a [u8]),
    /// Collect up to `cap` payload bytes (the rest of a longer phase is read and dropped).
    In { cap: usize, into: &'a mut Vec<u8> },
}

fn next_tid(op_code: u16) -> u32 {
    if op_code == op::OPEN_SESSION {
        TID.store(1, Ordering::Relaxed);
        0
    } else {
        TID.fetch_add(1, Ordering::Relaxed)
    }
}

fn fail(at: &str, why: &str) {
    if STATE.swap(ST_FAILED, Ordering::Relaxed) != ST_FAILED {
        serial_println!("[ptp] transport fail at={} why={} slot={} -> session closed (replug the camera to retry)", at, why, SLOT.load(Ordering::Relaxed));
    }
}

/// The millisecond clock every wait here reads.
fn now_ms() -> u64 {
    crate::arch::ms()
}

impl XhciController {
    // ── Enumeration hooks ──

    /// Root path, after the configuration walk (mod.rs, beside `usbnet_after_walk`): take the PTP candidate
    /// and issue its Configure-Endpoint on the async FSM; the completion lands in [`cfg_done`].
    pub(super) fn ptp_after_walk(&mut self, slot: u8) -> bool {
        HUBBED.store(false, Ordering::Relaxed);
        let Some((ia, im, oa, om)) = walk_candidate(slot) else { return false };
        if VENDOR.load(Ordering::Relaxed) {
            // CRAWLC: an AX88179 is 0xFF/0xFF/0x00 too — when usbnet's walk took the slot, the PTP walk lets go
            // and the usbnet arm (after this one in mod.rs) configures it.
            #[cfg(feature = "usbnet")]
            if super::usbnet::walk_is_candidate(slot) {
                STATE.store(ST_ABSENT, Ordering::Relaxed);
                SLOT.store(0, Ordering::Relaxed);
                walk_reset();
                return false;
            }
            // MTPPHONE: no synchronous EP0 inside the event dispatch — park the candidate; the service
            // pass asks its OS descriptor and configures it then. CRAWLC: the parked slot is ours (`true`
            // gates the usbnet and UAC1 arms), and a root-port phone with no HID interface releases the
            // port here — nothing else would, and the root FSM's watchdog would dispose the parked slot.
            STATE.store(ST_VENDOR, Ordering::Relaxed);
            serial_println!("[mtp] vendor interface slot={} iface={} — the OS descriptor is asked on the service pass", slot, IFACE.load(Ordering::Relaxed));
            let s = &self.slots[slot as usize];
            if !s.is_downstream && !s.is_keyboard && !s.is_mouse {
                self.start_next_port();
            }
            return true;
        }
        STATE.store(ST_CONFIGURING, Ordering::Relaxed);
        PENDING_CFG.store(slot, Ordering::Relaxed);
        self.configure_endpoints(slot, ia, im, oa, om);
        true
    }

    /// Hub path (mod.rs `enumerate_downstream`, before the usbnet/HID walks): the synchronous twin — device
    /// descriptor, configuration 0 (256 bytes), the same notes, `configure_bulk_endpoints_sync`. `true` =
    /// this slot is the camera's (configured, or refused and named); `false` = not a PTP device.
    pub(super) fn ptp_downstream(&mut self, slot: u8) -> bool {
        if !walk_open() {
            return false;
        }
        let buf = self.slots[slot as usize].descriptor_buffer as u64;
        if buf == 0 {
            return false;
        }
        unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 256) };
        if self.sync_control(slot, 0x80, 0x06, 0x0100, 0, 18, buf, true).is_err() || self.last_control_len < 18 {
            return false;
        }
        let (vid, pid) = unsafe {
            let p = buf as *const u8;
            ((*p.add(8) as u16) | ((*p.add(9) as u16) << 8), (*p.add(10) as u16) | ((*p.add(11) as u16) << 8))
        };
        note_device(slot, vid, pid);
        unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 256) };
        if self.sync_control(slot, 0x80, 0x06, 0x0200, 0, 256, buf, true).is_err() {
            return false;
        }
        let d = unsafe { core::slice::from_raw_parts(buf as *const u8, 256) };
        let total = (((d[2] as usize) | ((d[3] as usize) << 8)).min(self.last_control_len as usize)).min(256);
        note_config_header(slot, d[5]);
        let mut off = 0usize;
        while off + 2 <= total {
            let len = d[off] as usize;
            if len == 0 {
                break;
            }
            match d[off + 1] {
                0x04 if off + 8 <= total => note_interface(slot, d[off + 2], d[off + 5], d[off + 6], d[off + 7]),
                0x05 if off + 7 <= total => note_ep(slot, d[off + 2], d[off + 3], (d[off + 4] as u16) | ((d[off + 5] as u16) << 8), d[off + 6]),
                _ => {}
            }
            off += len;
        }
        HUBBED.store(true, Ordering::Relaxed);
        let Some((ia, im, oa, om)) = walk_candidate(slot) else {
            HUBBED.store(false, Ordering::Relaxed);
            return false;
        };
        if VENDOR.load(Ordering::Relaxed) && !self.mtp_os_probe(slot, IFACE.load(Ordering::Relaxed)) {
            // MTPPHONE: not MTP — release the walk so usbnet's downstream walk (next in mod.rs) sees the slot.
            STATE.store(ST_ABSENT, Ordering::Relaxed);
            HUBBED.store(false, Ordering::Relaxed);
            walk_reset();
            return false;
        }
        if !self.configure_bulk_endpoints_sync(slot, ia, im, oa, om) {
            fail("configure-endpoint(hub)", "refused");
            return true;
        }
        STATE.store(ST_CONFIGURED, Ordering::Relaxed);
        true
    }

    // ── The per-pass service (mod.rs `service_ftdi`, beside `service_uvc`) ──

    /// Session bring-up once the desktop's services are up, then the event pipe and the object table, a
    /// bounded chunk per pass. Idempotent no-op without a camera.
    pub fn service_ptp(&mut self) {
        let slot = SLOT.load(Ordering::Relaxed);
        if slot == 0 {
            return;
        }
        match STATE.load(Ordering::Relaxed) {
            ST_CONFIGURED if crate::boot::services_up() => self.ptp_bringup(slot),
            ST_VENDOR if crate::boot::services_up() && !self.enum_active => self.mtp_vendor_claim(slot), // CRAWLC: the root port was released at the walk, so its synchronous EP0 waits out any enumeration in flight (the one-port-at-a-time rule `service_uvc` keeps)
            ST_OPEN => {
                self.ptp_events(slot);
                self.ptp_fill(slot);
            }
            _ => {}
        }
    }

    /// MTPPHONE: a parked root-port vendor candidate (ST_VENDOR) — its OS descriptor asked now that EP0
    /// may be driven synchronously; MTP → the hub path's own synchronous Configure-Endpoint → ST_CONFIGURED
    /// (bring-up on the next pass); anything else is released to the stock path.
    fn mtp_vendor_claim(&mut self, slot: u8) {
        let release = || {
            STATE.store(ST_ABSENT, Ordering::Relaxed);
            SLOT.store(0, Ordering::Relaxed);
            walk_reset();
        };
        #[cfg(feature = "usbnet")]
        if super::usbnet::is_link_slot(slot) {
            return release(); // the link driver holds this slot (an AX88179 is 0xFF/0xFF/0x00 too)
        }
        if !self.mtp_os_probe(slot, IFACE.load(Ordering::Relaxed)) {
            return release();
        }
        let (ia, im) = (IN_EP.load(Ordering::Relaxed), IN_MPS.load(Ordering::Relaxed));
        let (oa, om) = (OUT_EP.load(Ordering::Relaxed), OUT_MPS.load(Ordering::Relaxed));
        if !self.configure_bulk_endpoints_sync(slot, ia, im, oa, om) {
            return fail("configure-endpoint(mtp)", "refused");
        }
        STATE.store(ST_CONFIGURED, Ordering::Relaxed);
    }

    fn ptp_bringup(&mut self, slot: u8) {
        let t0 = now_ms();
        let cfg = CFG_VALUE.load(Ordering::Relaxed).max(1);
        if !matches!(self.sync_control(slot, 0x00, 0x09, cfg as u16, 0, 0, 0, false), Ok(1)) {
            fail("SET_CONFIGURATION", "refused");
            return;
        }
        let ev_pipe = self.ptp_event_pipe(slot);
        if ev_pipe {
            self.ptp_arm_event(slot);
        }
        match self.ptp_txn(op::OPEN_SESSION, &[1], Phase::None) {
            Ok(r) if r.code == rc::OK || r.code == rc::SESSION_ALREADY_OPEN => {}
            Ok(r) => {
                serial_println!("[ptp] OpenSession answered {:#06x}", r.code);
                fail("OpenSession", "response");
                return;
            }
            Err(e) => {
                fail("OpenSession", e);
                return;
            }
        }
        let mut raw = Vec::new();
        let di = match self.ptp_txn(op::GET_DEVICE_INFO, &[], Phase::In { cap: 64 << 10, into: &mut raw }) {
            Ok(r) if r.code == rc::OK => match codec::device_info(&raw) {
                Some(d) => d,
                None => {
                    fail("GetDeviceInfo", "dataset unreadable");
                    return;
                }
            },
            Ok(_) => {
                fail("GetDeviceInfo", "response");
                return;
            }
            Err(e) => {
                fail("GetDeviceInfo", e);
                return;
            }
        };
        PARTIAL_OK.store(di.ops.contains(&op::GET_PARTIAL_OBJECT), Ordering::Relaxed);
        let vid = VID.load(Ordering::Relaxed);
        let is_sony = di.vendor_ext == sony::VENDOR_EXT
            || (di.vendor_ext == sony::VENDOR_EXT_MTP && di.manufacturer == "Sony Corporation")
            || vid == sony::VID;
        if is_sony && di.ops.contains(&op::SONY_SDIO_CONNECT) {
            self.ptp_sony_connect();
        }
        if STATE.load(Ordering::Relaxed) == ST_FAILED {
            return;
        }
        let mut raw = Vec::new();
        let storages = match self.ptp_txn(op::GET_STORAGE_IDS, &[], Phase::In { cap: 4096, into: &mut raw }) {
            Ok(r) if r.code == rc::OK => codec::u32_array(&raw).unwrap_or_default(),
            Ok(_) => Vec::new(), // PC-Remote bodies may hold no card storage: honest zero
            Err(e) => {
                fail("GetStorageIDs", e);
                return;
            }
        };
        // MTPPHONE: a phone (never a Sony body: its PTPCAM path above is untouched) lists its tree by MTP.
        let mtp_l = if !is_sony && mtp::is_mtp(&di, mtp::OS_DESC_MTP.load(Ordering::Relaxed)) {
            match mtp::enumerate(self, &di, &storages) {
                Ok(l) => Some(l),
                Err(e) => return fail("MTP enumerate", e),
            }
        } else {
            None
        };
        let handles = match &mtp_l {
            Some(l) => l.objs.iter().map(|o| o.handle).collect(),
            None => self.ptp_handles(),
        };
        if STATE.load(Ordering::Relaxed) == ST_FAILED {
            return;
        }
        let model = if di.model.is_empty() { String::from("camera") } else { di.model.clone() };
        let volume = volume_name_of(mtp_l.as_ref().and_then(|l| l.friendly.as_deref()).unwrap_or(&model));
        let storage_names: Vec<String> = mtp_l.as_ref().map(|l| l.storages.iter().map(|s| s.1.clone()).collect()).unwrap_or_default();
        serial_println!(
            "[ptp] dev={:04x}:{:04x} model={} storages={} objects={}",
            vid, PID.load(Ordering::Relaxed), model, storages.len(), handles.len()
        );
        serial_println!(
            "[ptp] session open slot={} vendor_ext={:#x} ops={} partial={} events={} sony={} manufacturer={}",
            slot, di.vendor_ext, di.ops.len(), PARTIAL_OK.load(Ordering::Relaxed) as u8, if ev_pipe { "pipe" } else { "none" },
            SONY_READY.load(Ordering::Relaxed) as u8, di.manufacturer
        );
        {
            let mut t = TABLE.lock();
            t.handles = handles;
            t.infos.clear();
            t.failed.clear();
        }
        if let Some(l) = mtp_l {
            mtp::install(l, t0, vid, PID.load(Ordering::Relaxed), &volume);
        }
        *INFO.lock() = Some(Info { model, manufacturer: di.manufacturer, serial: di.serial, volume: volume.clone(), storages, storage_names });
        STATE.store(ST_OPEN, Ordering::Relaxed);
        crate::fs::vfs::register_hotplug(crate::fs::vfs::ptpfs::provide);
        let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
        serial_println!("[ptp] volume /volumes/{} mounted (read-only; objects listed a chunk per pass)", volume);
        if SONY_READY.load(Ordering::Relaxed) {
            super::uvc::arm_ptp(slot); // the Camera window (UVCCAM's viewer) pulls live-view JPEGs from this session
        }
    }

    /// Sony PC-Remote handshake, libgphoto2's order (library.c camera_init, PTP_VENDOR_SONY): SDIO_Connect
    /// 1, 2; SDIO_GetExtDeviceInfo until it answers a list (≤ 20 tries); SDIO_Connect 3; PriorityMode =
    /// application (non-fatal there, non-fatal here). Any refusal leaves the card volume up without live view.
    fn ptp_sony_connect(&mut self) {
        for phase in [1u32, 2] {
            let mut sink = Vec::new();
            match self.ptp_txn(op::SONY_SDIO_CONNECT, &[phase, 0, 0], Phase::In { cap: 256, into: &mut sink }) {
                Ok(r) if r.code == rc::OK => {}
                Ok(r) => {
                    serial_println!("[ptp] sony sdio=refused phase={} rc={:#06x} (card access only)", phase, r.code);
                    return;
                }
                Err(e) => return fail("SDIO_Connect", e),
            }
        }
        let mut ext = (0u16, Vec::new(), Vec::new());
        for _ in 0..20 {
            let mut raw = Vec::new();
            match self.ptp_txn(op::SONY_SDIO_GET_EXT_DEVICE_INFO, &[sony::PROTOCOL_3_00, 1], Phase::In { cap: 16 << 10, into: &mut raw }) {
                Ok(r) if r.code == rc::OK && !raw.is_empty() => {
                    ext = codec::sony_ext_info(&raw).unwrap_or_default();
                    break;
                }
                Ok(_) => self.ptp_wait_ms(20),
                Err(e) => return fail("SDIO_GetExtDeviceInfo", e),
            }
        }
        let mut sink = Vec::new();
        match self.ptp_txn(op::SONY_SDIO_CONNECT, &[3, 0, 0], Phase::In { cap: 256, into: &mut sink }) {
            Ok(r) if r.code == rc::OK => {}
            Ok(r) => {
                serial_println!("[ptp] sony sdio=refused phase=3 rc={:#06x} (card access only)", r.code);
                return;
            }
            Err(e) => return fail("SDIO_Connect", e),
        }
        let pri = matches!(self.ptp_txn(op::SONY_SDIO_SET_EXT_DEVICE_PROP_VALUE, &[sony::PRIORITY_MODE as u32], Phase::Out(&[1u8])), Ok(r) if r.code == rc::OK);
        SONY_READY.store(true, Ordering::Relaxed);
        serial_println!(
            "[ptp] sony sdio=connected ext={:#x} props={} controls={} priority={}",
            ext.0, ext.1.len(), ext.2.len(), if pri { "application" } else { "refused" }
        );
    }

    /// GetObjectHandles(all storages, all formats, all objects) — ISO 15740 §10.5.7: StorageID 0xFFFFFFFF,
    /// ObjectFormatCode 0, ParentObject 0 = every object regardless of its parent.
    fn ptp_handles(&mut self) -> Vec<u32> {
        let mut raw = Vec::new();
        match self.ptp_txn(op::GET_OBJECT_HANDLES, &[0xFFFF_FFFF, 0, 0], Phase::In { cap: (MAX_OBJECTS + 1) * 4, into: &mut raw }) {
            Ok(r) if r.code == rc::OK => {
                let mut v = codec::u32_array(&raw).unwrap_or_else(|| {
                    // A list longer than the cap arrives truncated: the count overstates the bytes kept.
                    raw.get(4..).map(|b| b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()).unwrap_or_default()
                });
                if v.len() > MAX_OBJECTS {
                    serial_println!("[ptp] object list capped at {} of {}", MAX_OBJECTS, v.len());
                    v.truncate(MAX_OBJECTS);
                }
                v
            }
            Ok(_) => Vec::new(),
            Err(e) => {
                fail("GetObjectHandles", e);
                Vec::new()
            }
        }
    }

    /// Fill the object table, FILL_PER_PASS GetObjectInfo per pass; an event that asked for a re-list
    /// re-reads the handles first and keeps every info still valid.
    fn ptp_fill(&mut self, _slot: u8) {
        if mtp::walk_pending() {
            return self.mtp_walk_step(); // MTPPHONE: the per-folder walk (a body without a usable 0x9805)
        }
        if REFRESH.swap(false, Ordering::Relaxed) {
            let handles = self.ptp_handles();
            if STATE.load(Ordering::Relaxed) != ST_OPEN {
                return;
            }
            let mut t = TABLE.lock();
            let keep: alloc::collections::BTreeSet<u32> = handles.iter().copied().collect();
            t.infos.retain(|h, _| keep.contains(h));
            t.failed.retain(|h, _| keep.contains(h));
            t.handles = handles;
            *OBJ_CACHE.lock() = None;
            FILL_SAID.store(false, Ordering::Relaxed);
        }
        let todo: Vec<u32> = {
            let t = TABLE.lock();
            t.handles.iter().copied().filter(|h| !t.infos.contains_key(h) && !t.failed.contains_key(h)).take(FILL_PER_PASS).collect()
        };
        if todo.is_empty() {
            if !FILL_SAID.swap(true, Ordering::Relaxed) {
                let t = TABLE.lock();
                let dirs = t.infos.values().filter(|o| o.is_dir()).count();
                serial_println!("[ptp] objects listed n={} dirs={} files={} unreadable={}", t.handles.len(), dirs, t.infos.len() - dirs, t.failed.len());
                drop(t);
                let _ = crate::fs::ns_bump::<(), ()>(Ok(()));
            }
            return;
        }
        for h in todo {
            let mut raw = Vec::new();
            let r = self.ptp_txn(op::GET_OBJECT_INFO, &[h], Phase::In { cap: 4096, into: &mut raw });
            let mut t = TABLE.lock();
            match r {
                Ok(r) if r.code == rc::OK => match codec::object_info(&raw) {
                    Some(o) => {
                        let obj = Obj {
                            handle: h,
                            storage: o.storage,
                            parent: o.parent,
                            format: o.format,
                            size: o.size as u64,
                            name: sanitize(&o.filename, h),
                            mtime: o.capture.or(o.modified),
                        };
                        t.infos.insert(h, obj);
                    }
                    None => {
                        t.failed.insert(h, 0);
                    }
                },
                Ok(r) => {
                    t.failed.insert(h, r.code);
                }
                Err(_) => return, // `fail` named it
            }
        }
    }

    /// The event pipe's completions: log, mark the table for a re-list on object/store changes, re-arm.
    fn ptp_events(&mut self, slot: u8) {
        if !EV_DONE.swap(false, Ordering::AcqRel) {
            return;
        }
        EV_ARMED.store(false, Ordering::Relaxed);
        let code = EV_CODE.load(Ordering::Relaxed);
        if code != 1 && code != 13 {
            serial_println!("[ptp] event pipe completion code={} — pipe stopped (the session continues without events)", code);
            return;
        }
        let buf = EV_BUF.load(Ordering::Relaxed);
        if buf != 0 {
            let n = 64usize.saturating_sub(EV_RES.load(Ordering::Relaxed) as usize).min(64);
            dma_coherency::inval(buf, 64);
            let b = unsafe { core::slice::from_raw_parts(buf as *const u8, n) };
            if let Some(e) = codec::event(b) {
                let seen = EV_SEEN.fetch_add(1, Ordering::Relaxed) + 1;
                if seen <= 8 || seen.is_power_of_two() {
                    serial_println!("[ptp] event code={:#06x} p1={:#010x} n={}", e.code, e.params[0], seen);
                }
                if matches!(e.code, ev::OBJECT_ADDED | ev::OBJECT_REMOVED | ev::STORE_ADDED | ev::STORE_REMOVED | ev::STORAGE_INFO_CHANGED | ev::SONY_OBJECT_ADDED | ev::SONY_OBJECT_REMOVED) {
                    REFRESH.store(true, Ordering::Relaxed);
                }
            }
        }
        self.ptp_arm_event(slot);
    }

    /// Add the interrupt-IN event endpoint with a second Configure-Endpoint (A0 + its DCI, Context Entries
    /// raised to cover it), the hub status-change endpoint's exact recipe (mod.rs `bring_up_hub`'s
    /// interrupt endpoint: interval by speed, EP Type 7, CErr 3, DW4 = avg TRB len | max ESIT payload).
    fn ptp_event_pipe(&mut self, slot: u8) -> bool {
        let (addr, mps, interval) = (INT_EP.load(Ordering::Relaxed), INT_MPS.load(Ordering::Relaxed), INT_INTERVAL.load(Ordering::Relaxed));
        if addr == 0 || mps == 0 {
            return false;
        }
        let dci = ((addr & 0x0F) * 2 + 1) as u32;
        let ring = ring::TransferRing::new(16);
        let phys = ring.get_ptr();
        let input_ctx_virt;
        unsafe {
            let output_ctx_virt = self.slots[slot as usize].output_context;
            input_ctx_virt = self.slots[slot as usize].input_context;
            let base = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base as *mut u8, 0, core::mem::size_of::<super::context::InputContext>());
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<super::context::DeviceContext>());
            let out_dw0 = core::ptr::read_volatile(output_ctx_virt as *const u32);
            let speed = (out_dw0 >> 20) & 0x0F;
            let enc: u32 = if speed >= 3 {
                (interval as u32).clamp(1, 16) - 1
            } else if interval > 0 {
                (31 - (interval as u32).leading_zeros()) + 3
            } else {
                0
            };
            base.add(1).write_volatile(1 | (1 << dci));
            let slot_ctx = base.add(super::context::CTX_WORDS);
            for i in 0..8 {
                slot_ctx.add(i).write_volatile(core::ptr::read_volatile((output_ctx_virt as *const u32).add(i)));
            }
            let dw0 = slot_ctx.read_volatile();
            let entries = ((dw0 >> 27) & 0x1F).max(dci);
            slot_ctx.write_volatile((dw0 & !(0x1F << 27)) | (entries << 27));
            let ep = base.add((1 + dci as usize) * super::context::CTX_WORDS);
            ep.add(0).write_volatile(enc << 16);
            ep.add(1).write_volatile((7 << 3) | (3 << 1) | ((mps as u32) << 16));
            ep.add(2).write_volatile((phys as u32) | 1);
            ep.add(3).write_volatile((phys >> 32) as u32);
            ep.add(4).write_volatile((mps as u32) | ((mps as u32) << 16));
        }
        let trb = Trb { parameter: input_ctx_virt as u64, status: 0, control: (12 << 10) | ((slot as u32) << 24) };
        match self.run_command_sync(trb) {
            Ok((1, _)) => {}
            Ok((c, _)) => {
                serial_println!("[ptp] event pipe Configure-Endpoint code={} — no events this session", c);
                return false;
            }
            Err(_) => {
                serial_println!("[ptp] event pipe Configure-Endpoint timed out — no events this session");
                return false;
            }
        }
        *EV_RING.lock() = Some(ring);
        if EV_BUF.load(Ordering::Relaxed) == 0 {
            let l = core::alloc::Layout::from_size_align(64, 64).unwrap();
            let p = unsafe { alloc::alloc::alloc_zeroed(l) };
            if p.is_null() {
                return false;
            }
            EV_BUF.store(p as usize, Ordering::Relaxed);
        }
        EV_DCI.store(dci as u8, Ordering::Relaxed);
        true
    }

    fn ptp_arm_event(&mut self, slot: u8) {
        let buf = EV_BUF.load(Ordering::Relaxed);
        let dci = EV_DCI.load(Ordering::Relaxed);
        if buf == 0 || dci == 0 {
            return;
        }
        dma_coherency::clean(buf, 64);
        let trb_phys = {
            let mut g = EV_RING.lock();
            let Some(r) = g.as_mut() else { return };
            let base = r.get_ptr();
            match r.push(Trb { parameter: buf as u64, status: 64, control: (1 << 10) | (1 << 5) }) {
                Ok(i) => base + (i as u64) * 16,
                Err(_) => return,
            }
        };
        EV_TRB.store(trb_phys, Ordering::Relaxed);
        EV_DONE.store(false, Ordering::Relaxed);
        EV_ARMED.store(true, Ordering::Relaxed);
        self.ring_doorbell(slot, dci as u32);
    }

    // ── Transport ──

    /// Wait `ms` while the event ring keeps being drained (input never stalls behind a camera wait).
    fn ptp_wait_ms(&mut self, ms: u64) {
        let t0 = now_ms();
        while now_ms().saturating_sub(t0) < ms {
            if !self.drain_event_ring_once() {
                crate::hlt();
            }
        }
    }

    /// One Normal TD on the bulk pipe, awaited: (completion code, residue). `zlp` chains a zero-length TRB
    /// (an OUT whose length is a multiple of wMaxPacketSize, §7.2).
    fn ptp_xfer(&mut self, slot: u8, is_in: bool, phys: u64, len: usize, zlp: bool) -> Result<(u8, u32), &'static str> {
        let s = &self.slots[slot as usize];
        let ep = if is_in { s.bulk_in_ep } else { s.bulk_out_ep };
        if ep == 0 {
            return Err("no bulk endpoint");
        }
        let dci = if is_in { (ep & 0x0F) * 2 + 1 } else { (ep & 0x0F) * 2 };
        dma_coherency::clean(phys as usize, len); // OUT: the bytes reach DRAM; IN: no dirty line evicts over the DMA
        let trb_phys = {
            let s = &mut self.slots[slot as usize];
            let ring = if is_in { s.bulk_in_ring.as_mut() } else { s.bulk_out_ring.as_mut() }.ok_or("no ring")?;
            let base = ring.get_ptr();
            let i = if zlp {
                ring.push(Trb { parameter: phys, status: len as u32, control: (1 << 10) | (1 << 4) }).map_err(|_| "ring")?;
                ring.push(Trb { parameter: 0, status: 0, control: (1 << 10) | (1 << 5) }).map_err(|_| "ring")?
            } else {
                ring.push(Trb { parameter: phys, status: len as u32, control: (1 << 10) | (1 << 5) }).map_err(|_| "ring")?
            };
            base + (i as u64) * 16
        };
        XF_DCI.store(dci, Ordering::Relaxed);
        XF_TRB.store(trb_phys, Ordering::Relaxed);
        XF_DONE.store(false, Ordering::Relaxed);
        XF_ARMED.store(true, Ordering::Release);
        self.ring_doorbell(slot, dci as u32);
        let start = crate::arch::now_cycles();
        let budget = crate::arch::hw_wait_budget();
        let spin = Self::spin_window();
        while !XF_DONE.load(Ordering::Acquire) {
            if self.drain_event_ring_once() {
                continue;
            }
            if self.spin_for_event(spin) {
                continue;
            }
            crate::hlt();
            if crate::arch::now_cycles().wrapping_sub(start) >= budget && !XF_DONE.load(Ordering::Acquire) {
                return Err("timeout"); // XF stays armed: a late completion is still claimed, never misrouted
            }
        }
        XF_ARMED.store(false, Ordering::Relaxed);
        let code = XF_CODE.load(Ordering::Relaxed);
        if code != 1 && code != 13 {
            return Err(if code == 6 { "stall" } else if code == 3 { "babble" } else { "transfer error" });
        }
        if is_in {
            dma_coherency::inval(phys as usize, len);
        }
        Ok((code, XF_RES.load(Ordering::Relaxed)))
    }

    /// Bulk-IN of up to `want` bytes into the slot buffer; the bytes received.
    fn ptp_in(&mut self, slot: u8, phys: u64, want: usize) -> Result<usize, &'static str> {
        let (_, res) = self.ptp_xfer(slot, true, phys, want, false)?;
        Ok(want.saturating_sub(res as usize))
    }

    /// Bulk-IN where a container must arrive: a 0-byte completion is a previous data phase's ZLP, read past.
    fn ptp_in_container(&mut self, slot: u8, phys: u64, want: usize) -> Result<usize, &'static str> {
        for _ in 0..3 {
            let n = self.ptp_in(slot, phys, want)?;
            if n > 0 {
                return Ok(n);
            }
        }
        Err("only zero-length packets")
    }

    fn ptp_out(&mut self, slot: u8, phys: u64, len: usize) -> Result<(), &'static str> {
        let mps = OUT_MPS.load(Ordering::Relaxed) as usize;
        self.ptp_xfer(slot, false, phys, len, mps != 0 && len % mps == 0).map(|_| ())
    }

    /// One PTP transaction: command, the data phase, the response (§9.3). `Err` is a transport failure,
    /// which closes the session (named once); a non-OK response code is the caller's to read.
    pub(crate) fn ptp_txn(&mut self, op_code: u16, params: &[u32], phase: Phase<'_>) -> Result<codec::Response, &'static str> {
        let slot = SLOT.load(Ordering::Relaxed);
        let st = STATE.load(Ordering::Relaxed);
        if slot == 0 || !(st == ST_CONFIGURED || st == ST_OPEN) {
            return Err("no session");
        }
        let r = self.ptp_txn_inner(slot, op_code, params, phase);
        if let Err(e) = r {
            serial_println!("[ptp] op={:#06x} transport error {}", op_code, e);
            fail("transaction", e);
        }
        r
    }

    fn ptp_txn_inner(&mut self, slot: u8, op_code: u16, params: &[u32], phase: Phase<'_>) -> Result<codec::Response, &'static str> {
        let buf = self.slots[slot as usize].scsi_data_buffer.ok_or("no buffer")?;
        let phys = buf as u64;
        let tid = next_tid(op_code);
        let all = unsafe { core::slice::from_raw_parts_mut(buf, XFER_MAX) };
        let n = codec::command(all, op_code, tid, params).ok_or("command")?;
        self.ptp_out(slot, phys, n)?;
        match phase {
            Phase::None => {}
            Phase::Out(data) => {
                // MTPPHONE: a container longer than one TD is streamed — header + the first bytes, then
                // full TDs (multiples of wMaxPacketSize, so no short packet mid-phase), a ZLP only after
                // the last when the whole container is a multiple of wMaxPacketSize (§7.2). One TD's worth
                // is exactly the old single `ptp_out`.
                let all = unsafe { core::slice::from_raw_parts_mut(buf, XFER_MAX) };
                let total = codec::HDR + data.len();
                let first = data.len().min(XFER_MAX - codec::HDR);
                codec::container(all, ctype::DATA, op_code, tid, &data[..first]).ok_or("data container")?;
                all[0..4].copy_from_slice(&(if total > u32::MAX as usize { u32::MAX } else { total as u32 }).to_le_bytes());
                let mps = OUT_MPS.load(Ordering::Relaxed) as usize;
                let (mut sent, mut n) = (first, codec::HDR + first);
                loop {
                    let last = sent >= data.len();
                    self.ptp_xfer(slot, false, phys, n, last && mps != 0 && total % mps == 0)?;
                    if last {
                        break;
                    }
                    let k = (data.len() - sent).min(XFER_MAX);
                    let all = unsafe { core::slice::from_raw_parts_mut(buf, XFER_MAX) };
                    all[..k].copy_from_slice(&data[sent..sent + k]);
                    sent += k;
                    n = k;
                }
            }
            Phase::In { cap, into } => {
                let n = self.ptp_in_container(slot, phys, XFER_MAX)?;
                let first = unsafe { core::slice::from_raw_parts(buf as *const u8, n) };
                let h = codec::header(first).ok_or("short container")?;
                if h.kind == ctype::RESPONSE {
                    // No data phase: the device answered at once (an error code, or an empty dataset).
                    return codec::response(first).ok_or("bad response");
                }
                if h.kind != ctype::DATA {
                    return Err("unexpected container");
                }
                let unknown = h.len == 0xFFFF_FFFF; // MTP: a phase past 4 GiB says so and ends with a short packet
                let total = h.len as u64;
                let keep = |into: &mut Vec<u8>, b: &[u8]| {
                    let room = cap.saturating_sub(into.len());
                    into.extend_from_slice(&b[..b.len().min(room)]);
                };
                keep(into, &first[codec::HDR.min(n)..]);
                let mut got = n as u64;
                let mut short = n < XFER_MAX;
                let mps = (IN_MPS.load(Ordering::Relaxed) as usize).max(64);
                while !short && (unknown || got < total) {
                    let want = if unknown {
                        XFER_MAX
                    } else {
                        let rem = (total - got) as usize;
                        (rem.div_ceil(mps) * mps).min(XFER_MAX)
                    };
                    let k = self.ptp_in(slot, phys, want)?;
                    let b = unsafe { core::slice::from_raw_parts(buf as *const u8, k) };
                    keep(into, b);
                    got += k as u64;
                    short = k < want;
                }
                if !unknown && got < total {
                    return Err("data phase short");
                }
            }
        }
        let n = self.ptp_in_container(slot, phys, RESP_REQ)?;
        let b = unsafe { core::slice::from_raw_parts(buf as *const u8, n) };
        let r = codec::response(b).ok_or("bad response")?;
        if r.tid != tid {
            serial_println!("[ptp] op={:#06x} response tid={} expected {} (kept)", op_code, r.tid, tid);
        }
        Ok(r)
    }

    // ── What the volume and the viewer ask for (callers hold the controller loan) ──

    /// `len` bytes of object `handle` from `off`: GetPartialObject when the body lists it (one
    /// transaction for exactly the range), else the whole object by GetObject, kept for the next range.
    pub(crate) fn ptp_read_object(&mut self, handle: u32, size: u64, off: u64, len: usize) -> Result<Vec<u8>, &'static str> {
        if !is_open() {
            return Err("camera not connected");
        }
        if off >= size || len == 0 {
            return Ok(Vec::new());
        }
        let len = len.min((size - off) as usize);
        if PARTIAL_OK.load(Ordering::Relaxed) && off <= u32::MAX as u64 {
            // MTPPHONE: in chunks of at most 64 MiB (a range that fits one is the old single transaction).
            return mtp::read_partial(self, handle, off, len, mtp::READ_CHUNK);
        }
        {
            let c = OBJ_CACHE.lock();
            if let Some((h, ref b)) = *c {
                if h == handle {
                    let s = (off as usize).min(b.len());
                    return Ok(b[s..(s + len).min(b.len())].to_vec());
                }
            }
        }
        if size as usize > WHOLE_CAP {
            return Err("object larger than the whole-object cap and the camera has no GetPartialObject");
        }
        let mut v = Vec::with_capacity(size as usize);
        let r = self.ptp_txn(op::GET_OBJECT, &[handle], Phase::In { cap: WHOLE_CAP, into: &mut v })?;
        if r.code != rc::OK {
            return Err("GetObject refused");
        }
        let s = (off as usize).min(v.len());
        let out = v[s..(s + len).min(v.len())].to_vec();
        *OBJ_CACHE.lock() = Some((handle, v));
        Ok(out)
    }

    /// One Sony live-view JPEG (library.c's loop: GetObjectInfo(0xFFFFC002) until the object exists,
    /// GetObject, AccessDenied = "too fast", retried), cut out of its header.
    pub(crate) fn ptp_liveview(&mut self) -> Result<Vec<u8>, &'static str> {
        if !is_open() || !sony_ready() {
            return Err("no live view (not a Sony body in PC-Remote mode)");
        }
        let t0 = now_ms();
        for _ in 0..20 {
            let mut info = Vec::new();
            let r = self.ptp_txn(op::GET_OBJECT_INFO, &[sony::LIVEVIEW_HANDLE], Phase::In { cap: 4096, into: &mut info })?;
            if r.code == rc::INVALID_OBJECT_HANDLE {
                self.ptp_wait_ms(50);
                continue;
            }
            let mut raw = Vec::new();
            let r = self.ptp_txn(op::GET_OBJECT, &[sony::LIVEVIEW_HANDLE], Phase::In { cap: 4 << 20, into: &mut raw })?;
            if r.code == rc::ACCESS_DENIED || r.code == rc::DEVICE_BUSY {
                self.ptp_wait_ms(20);
                continue;
            }
            if r.code != rc::OK {
                return Err("live-view GetObject refused");
            }
            let (s, e) = codec::liveview_jpeg(&raw).ok_or("live-view object holds no JPEG")?;
            let n = LV_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n >= LV_SAID.load(Ordering::Relaxed) * 2 {
                LV_SAID.store(n, Ordering::Relaxed);
                serial_println!("[ptp] liveview frame={} bytes={} jpeg={} ms={}", n, raw.len(), e - s, now_ms().saturating_sub(t0));
            }
            return Ok(raw[s..e].to_vec());
        }
        Err("live view not ready")
    }

    /// The remote shutter (library.c's Sony capture): ShutterHalfRelease=2, ShutterRelease=2, held while
    /// the event pipe is watched for ObjectAdded (≤ 1 s, libgphoto2's bound), then ShutterRelease=1,
    /// ShutterHalfRelease=1 — SDIO_ControlDevice (0x9207) with a UINT16 data phase each.
    pub(crate) fn ptp_shutter(&mut self) -> Result<bool, &'static str> {
        if !is_open() || !sony_ready() {
            return Err("no remote shutter (not a Sony body in PC-Remote mode)");
        }
        let press = |c: &mut Self, prop: u16, v: u16| -> Result<(), &'static str> {
            let r = c.ptp_txn(op::SONY_SDIO_CONTROL_DEVICE, &[prop as u32], Phase::Out(&v.to_le_bytes()))?;
            if r.code == rc::OK { Ok(()) } else { Err("ControlDevice refused") }
        };
        press(self, sony::SHUTTER_HALF, 2)?;
        press(self, sony::SHUTTER_FULL, 2)?;
        let before = EV_SEEN.load(Ordering::Relaxed);
        let t0 = now_ms();
        let mut added = false;
        while now_ms().saturating_sub(t0) < 1000 && !added {
            self.ptp_wait_ms(10);
            let slot = SLOT.load(Ordering::Relaxed);
            self.ptp_events(slot);
            added = REFRESH.load(Ordering::Relaxed) && EV_SEEN.load(Ordering::Relaxed) > before;
        }
        press(self, sony::SHUTTER_FULL, 1)?;
        press(self, sony::SHUTTER_HALF, 1)?;
        serial_println!("[ptp] shutter fired press=ok release=ok object_added={} ms={}", added as u8, now_ms().saturating_sub(t0));
        Ok(added)
    }
}

// ── The volume's view of the table (pure reads; no cable I/O) ──────────────────────────────────────

/// The volume's name: the model, as a path component (`/` and control characters dropped, spaces to `-`).
fn volume_name_of(model: &str) -> String {
    let mut s: String = model.trim().chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').map(|c| if c == ' ' { '-' } else { c }).collect();
    if s.is_empty() || s == "." || s == ".." {
        s = String::from("camera");
    }
    s
}

/// A filename as a path component; an unnamed object gets its handle.
fn sanitize(name: &str, handle: u32) -> String {
    let s: String = name.chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').collect();
    if s.is_empty() || s == "." || s == ".." {
        alloc::format!("object-{:08x}", handle)
    } else {
        s
    }
}

/// The mounted volume's name while a session is open.
pub fn volume_name() -> Option<String> {
    if !is_open() {
        return None;
    }
    INFO.lock().as_ref().map(|i| i.volume.clone())
}

/// `ptp <vid:pid> <model> storages= objects=` for the `mount` listing, and the identity bytes.
pub fn describe() -> Option<(String, String)> {
    let g = INFO.lock();
    let i = g.as_ref()?;
    let n = TABLE.lock().handles.len();
    let (v, p) = vid_pid();
    Some((
        alloc::format!("{} {:04x}:{:04x} {} ({}) storages={} objects={}{}", if i.storage_names.is_empty() { "ptp" } else { "mtp" }, v, p, i.model, i.manufacturer, i.storages.len(), n, if sony_ready() { " liveview" } else { "" }),
        alloc::format!("{:04x}:{:04x}:{}:{}", v, p, i.model, i.serial),
    ))
}

/// The file the live view is read through, at the volume root of a Sony body in PC-Remote mode.
pub const LIVEVIEW_NAME: &str = "LIVEVIEW.JPG";

/// What a volume-relative path names.
pub enum Node {
    /// The volume root, or a storage's directory (`slot<n>`, only when the camera holds two or more).
    Dir { storage: Option<u32>, handle: Option<u32> },
    File(Obj),
    LiveView,
}

/// One row of a listing.
pub struct Row {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: Option<codec::DateTime>,
}

fn storages() -> Vec<u32> {
    INFO.lock().as_ref().map(|i| i.storages.clone()).unwrap_or_default()
}

/// MTPPHONE: the session is MTP (its storages are named top-level folders, and it takes writes).
pub fn is_mtp_session() -> bool {
    is_open() && INFO.lock().as_ref().map(|i| !i.storage_names.is_empty()).unwrap_or(false)
}

/// The volume root's storage folders: an MTP session's names (always, even for one storage); else
/// PTPCAM's `slot<n>` when the camera holds two or more; else none (the one storage IS the root).
fn storage_dirs() -> Vec<String> {
    let g = INFO.lock();
    let Some(i) = g.as_ref() else { return Vec::new() };
    if !i.storage_names.is_empty() {
        return i.storage_names.clone();
    }
    if i.storages.len() > 1 {
        return (0..i.storages.len()).map(|n| alloc::format!("slot{}", n + 1)).collect();
    }
    Vec::new()
}

/// The objects directly inside (`storage`, `parent`): `parent = None` is a storage's root — ParentObject 0
/// or 0xFFFFFFFF, or a parent that is not a listed association.
fn children_of(t: &Table, storage: Option<u32>, parent: Option<u32>) -> Vec<Obj> {
    let assoc: alloc::collections::BTreeSet<u32> = t.infos.values().filter(|o| o.is_dir()).map(|o| o.handle).collect();
    t.handles
        .iter()
        .filter_map(|h| t.infos.get(h))
        .filter(|o| storage.map(|s| o.storage == s).unwrap_or(true))
        .filter(|o| match parent {
            Some(p) => o.parent == p,
            None => o.parent == 0 || o.parent == 0xFFFF_FFFF || !assoc.contains(&o.parent),
        })
        .cloned()
        .collect()
}

/// Resolve a volume-relative path (case-insensitive, as a DCF card's FAT is).
pub fn lookup(rel: &str) -> Option<Node> {
    if !is_open() {
        return None;
    }
    let st = storages();
    let dirs = storage_dirs();
    let multi = !dirs.is_empty();
    let mut parts = rel.split('/').filter(|c| !c.is_empty() && *c != ".");
    let mut storage = None;
    let mut first = parts.next();
    if first.is_none() {
        return Some(Node::Dir { storage: None, handle: None });
    }
    if multi {
        let c = first?;
        let ix = dirs.iter().position(|d| d.eq_ignore_ascii_case(c)).filter(|&i| i < st.len())?;
        storage = Some(st[ix]);
        first = parts.next();
        if first.is_none() {
            return Some(Node::Dir { storage, handle: None });
        }
    } else if first.map(|c| c.eq_ignore_ascii_case(LIVEVIEW_NAME)).unwrap_or(false) && sony_ready() {
        return if parts.next().is_none() { Some(Node::LiveView) } else { None };
    }
    let t = TABLE.lock();
    let mut parent: Option<u32> = None;
    let mut comp = first;
    while let Some(c) = comp {
        let hit = children_of(&t, storage, parent).into_iter().find(|o| o.name.eq_ignore_ascii_case(c))?;
        comp = parts.next();
        if comp.is_none() {
            return Some(if hit.is_dir() { Node::Dir { storage, handle: Some(hit.handle) } } else { Node::File(hit) });
        }
        if !hit.is_dir() {
            return None;
        }
        parent = Some(hit.handle);
    }
    None
}

/// The listing of a directory node (what the table holds right now; a card still being listed shows
/// the objects read so far).
pub fn list(storage: Option<u32>, handle: Option<u32>) -> Vec<Row> {
    let dirs = storage_dirs();
    let mut rows = Vec::new();
    if storage.is_none() && handle.is_none() {
        if !dirs.is_empty() {
            for name in dirs {
                rows.push(Row { name, dir: true, size: 0, mtime: None });
            }
            return rows;
        }
        if sony_ready() {
            let size = LV_CACHE.lock().as_ref().map(|(_, b)| b.len() as u64).unwrap_or(0);
            rows.push(Row { name: String::from(LIVEVIEW_NAME), dir: false, size, mtime: None });
        }
    }
    let t = TABLE.lock();
    for o in children_of(&t, storage, handle) {
        rows.push(Row { name: o.name.clone(), dir: o.is_dir(), size: o.size, mtime: o.mtime });
    }
    rows
}

// ── Callers outside the controller: the loan ───────────────────────────────────────────────────────

/// Run `f` with the controller loaned, waiting (bounded, unmasked only) the way the block layer does.
fn with_controller<T>(f: impl FnOnce(&mut XhciController) -> Result<T, &'static str>) -> Result<T, &'static str> {
    let mut loan = match super::claim() {
        Ok(l) => l,
        Err(super::XhciClaimError::NotReady) => return Err("no USB controller"),
        Err(super::XhciClaimError::Busy) => {
            if crate::arch::irqs_masked() {
                return Err("USB busy");
            }
            let start = crate::arch::now_cycles();
            let budget = crate::arch::hw_wait_budget();
            loop {
                crate::hlt();
                match super::claim() {
                    Ok(l) => break l,
                    Err(super::XhciClaimError::NotReady) => return Err("no USB controller"),
                    Err(super::XhciClaimError::Busy) => {}
                }
                if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                    return Err("USB busy");
                }
            }
        }
    };
    f(&mut *loan)
}

/// Object bytes for the volume's `read`.
pub fn read_object(handle: u32, size: u64, off: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    with_controller(|c| c.ptp_read_object(handle, size, off, len))
}

/// The newest live-view JPEG no older than `fresh_ms` (0 = always a new one).
pub fn liveview_frame(fresh_ms: u64) -> Result<Vec<u8>, &'static str> {
    let now = now_ms();
    if fresh_ms > 0 {
        if let Some((at, ref b)) = *LV_CACHE.lock() {
            if now.saturating_sub(at) <= fresh_ms {
                return Ok(b.clone());
            }
        }
    }
    let f = with_controller(|c| c.ptp_liveview())?;
    *LV_CACHE.lock() = Some((now_ms(), f.clone()));
    Ok(f)
}

/// The cached live-view frame, whatever its age (range reads past offset 0 of one open file).
pub fn liveview_cached() -> Option<Vec<u8>> {
    LV_CACHE.lock().as_ref().map(|(_, b)| b.clone())
}

/// Fire the shutter once (M3): the `ptp shutter` verb and Space in the Camera window (`uvc::key_route`).
pub fn shutter() -> Result<bool, &'static str> {
    with_controller(|c| c.ptp_shutter())
}

// ── The `ptp` verb ─────────────────────────────────────────────────────────────────────────────────

/// `ptp` / `ptp status` · `ptp view` (open the Camera window on the live view) · `ptp shutter` · `ptp frame`
/// (fetch one live-view JPEG and say its size).
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    let say = |c: &mut crate::console::Console, l: &str| {
        c.println(l);
        serial_println!("[ptp] verb: {}", l);
    };
    match args.first().copied().unwrap_or("status") {
        "status" => match describe() {
            Some((d, _)) if is_open() => {
                let line = alloc::format!("{} volume=/volumes/{}", d, volume_name().unwrap_or_default());
                say(console, &line)
            }
            _ => say(console, "ptp: no camera session (plug a camera in PC-Remote or MTP mode)"),
        },
        "view" => {
            if is_open() && sony_ready() {
                super::uvc::arm_ptp(SLOT.load(Ordering::Relaxed));
                say(console, "ptp: live view armed — the Camera window opens on the first frame (Space fires the shutter)");
            } else {
                say(console, "ptp view: no live view (a Sony body in PC-Remote mode is needed)");
            }
        }
        "shutter" => match shutter() {
            Ok(added) => {
                let line = alloc::format!("ptp shutter: fired (object_added={})", added as u8);
                say(console, &line)
            }
            Err(e) => {
                let line = alloc::format!("ptp shutter: {}", e);
                say(console, &line)
            }
        },
        "frame" => match liveview_frame(0) {
            Ok(f) => {
                let line = alloc::format!("ptp frame: {} bytes JPEG", f.len());
                say(console, &line)
            }
            Err(e) => {
                let line = alloc::format!("ptp frame: {}", e);
                say(console, &line)
            }
        },
        _ => say(console, "usage: ptp [status] | ptp view | ptp shutter | ptp frame"),
    }
}

// ── `tests ptp` ────────────────────────────────────────────────────────────────────────────────────

pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("ptp", selftest);
    }
}

/// `:: PTP: codec=ok deviceinfo=ok(fixture) stream=skip(no-device) -> PASS ::` with no camera; with a Sony
/// body in PC-Remote mode `stream=ok(<n>B)` is one live-view JPEG fetched over the cable.
pub fn selftest() {
    let codec_r = codec::selftest_codec();
    let di_r = codec::selftest_deviceinfo();
    let (stream, stream_ok) = if !is_open() {
        (String::from("skip(no-device)"), true)
    } else if !sony_ready() {
        (String::from("skip(no-liveview)"), true)
    } else {
        match liveview_frame(0) {
            Ok(f) => (alloc::format!("ok({}B)", f.len()), true),
            Err(e) => (alloc::format!("fail({})", e), false),
        }
    };
    let pass = codec_r.is_ok() && di_r.is_ok() && stream_ok;
    serial_println!(
        ":: PTP: codec={} deviceinfo={} stream={} -> {} ::",
        match codec_r { Ok(()) => "ok", Err(e) => e },
        match di_r { Ok(()) => "ok(fixture)", Err(e) => e },
        stream,
        if pass { "PASS" } else { "FAIL" }
    );
}
