//! CHARTER: Kernel — driver (the D435i IMU's HID interface; the probe is Sonde's, R112, CODEX Amendment IV pending)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! SONDE-RECORD-IMU (LEDGER SR491) — the Intel RealSense D435i's IMU, read over its USB HID interface.
//!
//! The D435i carries a Bosch BMI055 (accelerometer + gyroscope) behind a HID interface on the same USB device as its
//! three video functions. librealsense's UVC-less backend (rsusb) drives it with NO HID report parser: two feature
//! reports power each sensor and set its rate, then 32-byte input reports arrive on the interface's first
//! interrupt-IN endpoint. Every fact below is from librealsense v2.55.1 (Apache-2.0), cited by file and line, R110's
//! shape: the facts enter the tree, no code is copied.
//!
//! - The feature report (`src/hid/hid-types.h:76-85`, packed): reportId @0, connectionType @1, sensorState @2, power @3,
//!   minReport @4, report (u16) @5, unknown (u16) @7 — 9 bytes. Report ids: 1 accelerometer, 2 gyroscope (:17-18).
//!   Report type FEATURE = 3 (:12), power D0 = 2 (on) and D4 = 6 (off) (:14-15).
//! - The enable (`src/hid/hid-device.cpp:241-312` `set_feature_report`): wValue = (FEATURE << 8) | report id, wIndex =
//!   the HID interface; GET_REPORT (0xA1, 1), set power and — when a rate is asked — `report = 1000 / fps`, SET_REPORT
//!   (0x21, 9), GET_REPORT again and refuse unless the power read back is the one written. `open` sends D0 with the
//!   rate per sensor (:99-106), `close` D4 for both (:108-112).
//! - The "fps" `set_feature_report` receives is NOT Hz on this backend: `hid_sensor` maps the stream rate through the
//!   non-Windows `hid_fps_translation` first (`src/hid-sensor.cpp:124`, `src/ds/ds-motion-common.h:41-47`: 63 Hz → 1,
//!   100 → 1, 200 → 2, 250 → 3, 400 → 4). The BMI055's rates are accel {63, 250} and gyro {200, 400}
//!   (`src/ds/ds-motion-common.cpp:282-283,471`). This driver asks for the lower of each: accel 63 Hz (code 1, report
//!   1000) and gyro 200 Hz (code 2, report 500).
//! - The stream (`hid-device.cpp:177-188,377-385`): ONE request of `sizeof(REALSENSE_HID_REPORT)` = 32 bytes on the HID
//!   interface's first interrupt-IN endpoint, resubmitted on every completion; a completion of any other length is
//!   ignored (:164). The report's layout is `probe_core::imu::parse_raw`'s (the one decode).
//!
//! The pipe is `xhci::isoch`'s fifth (`IPIPE`, `IsoHost::int_configure` / `int_start`); its claim calls
//! [`push_report`]. `tests sonderec` (sonde.rs) drains the samples between depth frames as `probe_core::wire` motion
//! records and asks for the D4 power-down at its end ([`request_power_down`], sent by the xHCI service pass).
//!
//! The extrinsic: `uvc::multi::after_census` calls [`after_census`] for a D400, which reads the IMU calibration table
//! over the HW-monitor channel (MMER, `realsense_xu::imu_calibration`) through `uvc::hwmon::exchange` and keeps the
//! depth-to-IMU rotation and translation when the table says they are valid.

use crate::drivers::realsense_xu as rs;
use crate::drivers::uvc::{IsoEp, IsoHost, Xfer};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use probe_core::imu::{self, ImuKind};
use probe_core::wire::{self, MotionRecord, ID_MAX};

// ── HID facts (librealsense `src/hid/hid-types.h`, `src/hid/hid-device.cpp`) ───────────────────────────────────────

/// `hid-types.h:12` — `HID_REPORT_TYPE_FEATURE`.
pub const HID_REPORT_TYPE_FEATURE: u16 = 3;
/// `hid-types.h:14-15`.
pub const DEVICE_POWER_D0: u8 = 2;
pub const DEVICE_POWER_D4: u8 = 6;
/// `hid-types.h:32-33` — the request types (class, interface; IN / OUT).
pub const BM_HID_GET: u8 = 0xA1;
pub const BM_HID_SET: u8 = 0x21;
/// `hid-types.h:37,40`.
pub const HID_GET_REPORT: u8 = 0x01;
pub const HID_SET_REPORT: u8 = 0x09;
/// `sizeof(FEATURE_REPORT)` (`hid-types.h:76-85`, packed).
pub const FEATURE_REPORT_LEN: usize = 9;
/// `FEATURE_REPORT.power` / `.report` offsets.
pub const FEATURE_POWER: usize = 3;
pub const FEATURE_REPORT: usize = 5;
/// The rates asked for, and their `hid_fps_translation` codes (`ds-motion-common.h:43,45`).
pub const ACCEL_RATE_HZ: u32 = 63;
pub const ACCEL_FPS_CODE: u16 = 1;
pub const GYRO_RATE_HZ: u32 = 200;
pub const GYRO_FPS_CODE: u16 = 2;
/// USB HID class (HID 1.11 §4.1).
const CC_HID: u8 = 0x03;

/// `set_feature_report`'s wValue for `report_id` (`hid-device.cpp:246`).
pub const fn feature_wvalue(report_id: u8) -> u16 {
    (HID_REPORT_TYPE_FEATURE << 8) | report_id as u16
}

/// `hid-device.cpp:271-274`: power, and `report = 1000 / fps` when a rate code is given (`None` leaves it as read).
pub fn feature_enable(fr: &mut [u8; FEATURE_REPORT_LEN], power: u8, fps_code: Option<u16>) {
    fr[FEATURE_POWER] = power;
    if let Some(c) = fps_code {
        if c > 0 {
            fr[FEATURE_REPORT..FEATURE_REPORT + 2].copy_from_slice(&(1000 / c).to_le_bytes());
        }
    }
}

// ── The HID interface's interrupt-IN endpoint, from the configuration descriptor ──────────────────────────────────

/// The first interrupt-IN endpoint of the first HID interface in `cfg` (USB 2.0 §9.5, Tables 9-12 / 9-13; a SuperSpeed
/// Endpoint Companion after it, USB 3.2 §9.6.7, fills the burst and bytes per interval). `None` when there is none.
pub fn hid_ep_of(cfg: &[u8]) -> Option<IsoEp> {
    let mut off = 0usize;
    let (mut in_hid, mut cur_intf, mut cur_alt) = (false, 0u8, 0u8);
    let mut found: Option<IsoEp> = None;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 || off + len > cfg.len() {
            break;
        }
        let d = &cfg[off..off + len];
        match d[1] {
            4 if len >= 9 => {
                if found.is_some() {
                    break; // the endpoint's companion, if any, came before this interface
                }
                in_hid = d[5] == CC_HID;
                cur_intf = d[2];
                cur_alt = d[3];
            }
            5 if len >= 7 && in_hid && found.is_none() => {
                if (d[3] & 0x03) == 0x03 && (d[2] & 0x80) != 0 {
                    let w = u16::from_le_bytes([d[4], d[5]]);
                    found = Some(IsoEp {
                        intf: cur_intf,
                        alt: cur_alt,
                        addr: d[2],
                        mps: w & 0x07FF,
                        hs_extra: ((w >> 11) & 0x03) as u8,
                        interval: d[6],
                        ..IsoEp::default()
                    });
                }
            }
            5 if found.is_some() => break, // the next endpoint: no companion followed ours
            0x30 if len >= 6 => {
                if let Some(e) = found.as_mut() {
                    e.ss = true;
                    e.ss_burst = d[2] & 0x0F;
                    e.ss_bpi = u16::from_le_bytes([d[4], d[5]]);
                    break;
                }
            }
            _ => {}
        }
        off += len;
    }
    found
}

static HID_EP: crate::sync::Mutex<Option<IsoEp>> = crate::sync::Mutex::new(None);

/// Census time (`uvc::multi::after_census`, the line that walks `full` before its first transfer): remember the HID
/// interface's interrupt-IN endpoint. Pure but for the one store.
pub fn note_cfg(cfg: &[u8]) {
    *HID_EP.lock() = hid_ep_of(cfg);
}

/// The endpoint [`note_cfg`] found.
pub fn hid_ep() -> Option<IsoEp> {
    *HID_EP.lock()
}

// ── The extrinsic (MMER, read at census) ───────────────────────────────────────────────────────────────────────────

#[allow(clippy::declare_interior_mutable_const)]
const EZ: AtomicU32 = AtomicU32::new(0);
/// Depth-to-IMU rotation (9) then translation (3, metres), `f32` bit patterns, valid once [`EXTR_OK`] is set.
static EXTR: [AtomicU32; 12] = [EZ; 12];
static EXTR_OK: AtomicBool = AtomicBool::new(false);
/// The device address [`after_census`] ran for (a D400), + 1 (0 = none): [`arm`] arms only that camera.
static CENSUS_ADDR: AtomicU32 = AtomicU32::new(0);

/// The depth-to-IMU extrinsic the camera's table carries, or `None` (not read, or `extrinsic_valid != 1`: librealsense
/// then substitutes CAD values, `ds-calib-parsers.cpp:189-192`, which this driver never writes as measured).
pub fn extrinsic() -> Option<[f32; 12]> {
    if !EXTR_OK.load(Ordering::Acquire) {
        return None;
    }
    let mut e = [0f32; 12];
    for (i, v) in e.iter_mut().enumerate() {
        *v = f32::from_bits(EXTR[i].load(Ordering::Relaxed));
    }
    Some(e)
}

fn hex9(b: &[u8; FEATURE_REPORT_LEN]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for x in b.iter() {
        s.push_str(&alloc::format!("{:02x}", x));
    }
    s
}

/// GET_REPORT(feature, `id`) into `buf` and copied out (`hid-device.cpp:256-263`).
fn get_feature(xfer: Xfer, buf: *mut u8, intf: u8, id: u8) -> Result<[u8; FEATURE_REPORT_LEN], &'static str> {
    let n = xfer(BM_HID_GET, HID_GET_REPORT, feature_wvalue(id), intf as u16, FEATURE_REPORT_LEN as u16, true)?;
    if (n as usize) < FEATURE_REPORT_LEN {
        return Err("runt");
    }
    let mut fr = [0u8; FEATURE_REPORT_LEN];
    // SAFETY: `buf` is the live EP0 data buffer of the controller `xfer` drives; the transfer above wrote >= 9 bytes.
    fr.copy_from_slice(unsafe { core::slice::from_raw_parts(buf as *const u8, FEATURE_REPORT_LEN) });
    Ok(fr)
}

/// `set_feature_report` (`hid-device.cpp:241-312`): GET, set power (and the rate), SET, GET, the power checked.
/// Err((stage, reason)).
fn feature_set(xfer: Xfer, buf: *mut u8, intf: u8, id: u8, power: u8, fps_code: Option<u16>) -> Result<[u8; FEATURE_REPORT_LEN], (&'static str, &'static str)> {
    let mut fr = get_feature(&mut *xfer, buf, intf, id).map_err(|e| ("get-report", e))?;
    feature_enable(&mut fr, power, fps_code);
    // SAFETY: as in `get_feature`; no transfer is in flight while this copy runs, and the SET below sends it.
    unsafe { core::slice::from_raw_parts_mut(buf, FEATURE_REPORT_LEN) }.copy_from_slice(&fr);
    xfer(BM_HID_SET, HID_SET_REPORT, feature_wvalue(id), intf as u16, FEATURE_REPORT_LEN as u16, false).map_err(|e| ("set-report", e))?;
    let back = get_feature(&mut *xfer, buf, intf, id).map_err(|e| ("verify-get", e))?;
    if back[FEATURE_POWER] != power {
        return Err(("verify", "power-not-taken"));
    }
    Ok(back)
}

/// Called at the end of `uvc::multi::after_census` for a D400 (after the depth-units and GETINTCAL reads): the IMU
/// calibration table (MMER, `realsense_xu::imu_calibration`) through `uvc::hwmon::exchange`, then both feature reports
/// READ (GET_REPORT only — the enable is [`arm`]'s, on the xHCI path where a pipe follows). ONE attempt each.
pub fn after_census(addr: u8, unit: u8, vc: u8, hid_intf: Option<u8>, buf: *mut u8, cap: usize, xfer: Xfer) {
    use crate::drivers::uvc::hwmon::{exchange, Fail};
    EXTR_OK.store(false, Ordering::Release);
    CENSUS_ADDR.store(addr as u32 + 1, Ordering::Release);
    match exchange(unit, vc, buf, cap, &mut *xfer, rs::OP_MMER, &rs::hwmon_build_mmer) {
        Ok(g) => {
            // SAFETY: `exchange` returned Ok: the reply's 1024 bytes are in the live EP0 buffer until the next transfer.
            let r = unsafe { core::slice::from_raw_parts(buf as *const u8, rs::HWMON_BUF) };
            match rs::imu_calibration(&r[g.range.clone()]) {
                Ok(c) => {
                    if c.extrinsic_valid {
                        for (i, v) in c.depth_to_imu.iter().enumerate() {
                            EXTR[i].store(v.to_bits(), Ordering::Relaxed);
                        }
                        EXTR_OK.store(true, Ordering::Release);
                    }
                    let um = |v: f32| -> i64 { (v * 1_000_000.0) as i64 };
                    crate::census_println!(
                        "[sonde] imucal op=MMER(0,{}) unit={} get_cur={} echo={:#x} size={} id={:#06x} crc=ok extrinsic_valid={} intrinsic_valid={} t_um={},{},{}",
                        rs::IMU_EEPROM_LEN, unit, g.got, g.echo, g.size, rs::DM_V2_EEPROM_ID, c.extrinsic_valid as u8, c.intrinsic_valid as u8,
                        um(c.depth_to_imu[9]), um(c.depth_to_imu[10]), um(c.depth_to_imu[11])
                    );
                }
                Err(why) => serial_println!("[sonde] imucal=failed addr={} stage=table size={} reason={} extr=none", addr, g.size, why),
            }
        }
        Err(Fail::Withheld) => serial_println!("[sonde] imucal=withheld reason=ep0-buf-{}-lt-{} extr=none", cap, rs::HWMON_BUF),
        Err(Fail::Build) => serial_println!("[sonde] imucal=failed addr={} stage=build reason=buffer-short extr=none", addr),
        Err(Fail::SetCur(e)) => serial_println!("[sonde] imucal=failed addr={} stage=set-cur unit={} reason={} extr=none — NOT retried", addr, unit, e),
        Err(Fail::GetCur(e)) => serial_println!("[sonde] imucal=failed addr={} stage=get-cur unit={} reason={} extr=none — NOT retried", addr, unit, e),
        Err(Fail::Runt(got)) => serial_println!("[sonde] imucal=failed addr={} stage=get-cur got={} need={} reason=runt extr=none", addr, got, rs::HWMON_BUF),
        Err(Fail::Reply { echo, size }) => serial_println!(
            "[sonde] imucal=failed addr={} stage=reply echo={:#x} want_echo={:#x} size={} reason={} extr=none",
            addr, echo, rs::OP_MMER, size, if echo != rs::OP_MMER as u32 { "error-echo" } else { "size" }
        ),
    }
    let ep = hid_ep();
    let Some(intf) = ep.map(|e| e.intf).or(hid_intf) else {
        serial_println!("[sonde] imu addr={} hid_intf=none reason=no-hid-interface-in-census", addr);
        return;
    };
    let mut line = alloc::format!("[sonde] imu addr={} hid_intf={}", addr, intf);
    match ep {
        Some(e) => line.push_str(&alloc::format!(" ep={:#04x} mps={} interval={} ss={}", e.addr, e.mps, e.interval, e.ss)),
        None => line.push_str(" ep=none"),
    }
    for (name, id) in [("accel", imu::REPORT_ID_ACCEL), ("gyro", imu::REPORT_ID_GYRO)] {
        match get_feature(&mut *xfer, buf, intf, id) {
            Ok(fr) => line.push_str(&alloc::format!(
                " {}_feature={} power={} report={}",
                name, hex9(&fr), fr[FEATURE_POWER], u16::from_le_bytes([fr[FEATURE_REPORT], fr[FEATURE_REPORT + 1]])
            )),
            Err(e) => line.push_str(&alloc::format!(" {}_feature=failed({})", name, e)),
        }
    }
    crate::census_println!("{}", line);
}

// ── The enable and the pipe (xHCI path: `uvc::probe_iso`, right after `after_census`) ──────────────────────────────

/// The xHCI slot whose IMU is powered and piped (0 = none).
static EN_SLOT: AtomicU8 = AtomicU8::new(0);
/// `tests sonderec` asked for the D4 power-down; the xHCI service pass sends it ([`power_down_due`]).
static POWER_DUE: AtomicBool = AtomicBool::new(false);

/// Is an IMU pipe armed (the enable taken and the interrupt-IN started)?
pub fn armed() -> bool {
    EN_SLOT.load(Ordering::Acquire) != 0
}

/// `addr` is the xHCI slot `uvc::probe_iso` is configuring; a no-op unless [`after_census`] ran for it (a D400) and
/// [`note_cfg`] found the HID interrupt-IN. Powers each sensor to D0 at its rate (`set_feature_report`, librealsense
/// `open`), then `IsoHost::int_configure` / `int_start`. A pipe that cannot be armed powers the sensors back to D4.
pub fn arm(addr: u8, h: &mut dyn IsoHost) {
    if CENSUS_ADDR.load(Ordering::Acquire) != addr as u32 + 1 {
        return;
    }
    let Some(ep) = hid_ep() else {
        serial_println!("[sonde] imu arm slot={} reason=no-hid-interrupt-in pipe=none", addr);
        return;
    };
    if armed() {
        serial_println!("[sonde] imu arm slot={} reason=already-armed(slot={}) pipe=none", addr, EN_SLOT.load(Ordering::Relaxed));
        return;
    }
    let buf = h.buf();
    let mut x = |bm: u8, br: u8, wv: u16, wi: u16, wl: u16, di: bool| h.control(bm, br, wv, wi, wl, di);
    let a = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_ACCEL, DEVICE_POWER_D0, Some(ACCEL_FPS_CODE));
    let g = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_GYRO, DEVICE_POWER_D0, Some(GYRO_FPS_CODE));
    let word = |r: &Result<[u8; FEATURE_REPORT_LEN], (&'static str, &'static str)>| match r {
        Ok(fr) => alloc::format!("D0(report={})", u16::from_le_bytes([fr[FEATURE_REPORT], fr[FEATURE_REPORT + 1]])),
        Err((stage, why)) => alloc::format!("failed({}:{})", stage, why),
    };
    let (aw, gw) = (word(&a), word(&g));
    if a.is_err() && g.is_err() {
        serial_println!("[sonde] imu arm slot={} accel={} gyro={} pipe=none reason=no-sensor-enabled", addr, aw, gw);
        return;
    }
    let started = h.int_configure(&ep).map_err(|(_, n)| n).and_then(|()| h.int_start());
    match started {
        Ok(tds) => {
            EN_SLOT.store(addr, Ordering::Release);
            serial_println!(
                "[sonde] imu arm slot={} ep={:#04x} accel={}@{}Hz gyro={}@{}Hz pipe=armed tds={}",
                addr, ep.addr, aw, ACCEL_RATE_HZ, gw, GYRO_RATE_HZ, tds
            );
        }
        Err(why) => {
            let buf = h.buf();
            let mut x = |bm: u8, br: u8, wv: u16, wi: u16, wl: u16, di: bool| h.control(bm, br, wv, wi, wl, di);
            let _ = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_ACCEL, DEVICE_POWER_D4, None);
            let _ = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_GYRO, DEVICE_POWER_D4, None);
            serial_println!("[sonde] imu arm slot={} accel={} gyro={} pipe=failed({}) power=D4", addr, aw, gw, why);
        }
    }
}

/// `tests sonderec`, at its end: ask the xHCI service pass for the D4 power-down (librealsense `close`).
pub fn request_power_down() {
    if armed() {
        POWER_DUE.store(true, Ordering::Release);
    }
}

/// The xHCI service pass: the slot whose IMU is due a D4, if one is.
pub fn power_down_due() -> Option<u8> {
    if !POWER_DUE.load(Ordering::Acquire) {
        return None;
    }
    match EN_SLOT.load(Ordering::Acquire) {
        0 => {
            POWER_DUE.store(false, Ordering::Release);
            None
        }
        s => Some(s),
    }
}

/// The service pass could not reach the slot: the request is dropped, by name.
pub fn power_down_lost(why: &str) {
    POWER_DUE.store(false, Ordering::Release);
    serial_println!("[sonde] imu power=D4 slot={} result=failed({})", EN_SLOT.load(Ordering::Relaxed), why);
}

/// The D4 power-down on the slot's EP0 (`hid-device.cpp:108-112`), from the xHCI service pass. The interrupt pipe
/// stays armed (a powered-down sensor sends nothing); the counters are printed.
pub fn power_down(h: &mut dyn IsoHost) {
    POWER_DUE.store(false, Ordering::Release);
    let slot = EN_SLOT.load(Ordering::Relaxed);
    let Some(ep) = hid_ep() else {
        serial_println!("[sonde] imu power=D4 slot={} result=failed(no-hid-ep)", slot);
        return;
    };
    let buf = h.buf();
    let mut x = |bm: u8, br: u8, wv: u16, wi: u16, wl: u16, di: bool| h.control(bm, br, wv, wi, wl, di);
    let a = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_ACCEL, DEVICE_POWER_D4, None);
    let g = feature_set(&mut x, buf, ep.intf, imu::REPORT_ID_GYRO, DEVICE_POWER_D4, None);
    let w = |r: &Result<[u8; FEATURE_REPORT_LEN], (&'static str, &'static str)>| match r {
        Ok(_) => alloc::string::String::from("ok"),
        Err((s, why)) => alloc::format!("failed({}:{})", s, why),
    };
    let (pushed, over, other, short) = counts();
    let (ev, rep, err, dup) = crate::drivers::xhci::isoch::imu_stats();
    serial_println!(
        "[sonde] imu power=D4 slot={} accel={} gyro={} pushed={} ring_over={} other_id={} short={} events={} reports={} err={} dup={}",
        slot, w(&a), w(&g), pushed, over, other, short, ev, rep, err, dup
    );
}

// ── The samples: the claim pushes, the recorder drains ─────────────────────────────────────────────────────────────

/// One report as the driver keeps it: the sensor, the raw counts, the device's timestamp, and the CPU cycle counter at
/// arrival (`arch::now_cycles`, the recorder's clock).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sample {
    pub kind: ImuKind,
    pub raw: [i16; 3],
    pub device_ts: u64,
    pub cycles: u64,
}

const SAMPLE_ZERO: Sample = Sample { kind: ImuKind::Accel, raw: [0; 3], device_ts: 0, cycles: 0 };

/// Why a report was not kept.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skip {
    /// Fewer than 32 bytes (`hid-device.cpp:164` ignores those too).
    Short,
    /// A report id that is neither sensor (3 is librealsense's "custom", `hid-types.h:19`).
    OtherId(u8),
}

/// A report → a sample, through `probe_core::imu::parse_raw` (the one decode).
pub fn sample_of(report: &[u8], cycles: u64) -> Result<Sample, Skip> {
    let (id, device_ts, raw) = imu::parse_raw(report).ok_or(Skip::Short)?;
    let kind = match id {
        imu::REPORT_ID_ACCEL => ImuKind::Accel,
        imu::REPORT_ID_GYRO => ImuKind::Gyro,
        other => return Err(Skip::OtherId(other)),
    };
    Ok(Sample { kind, raw, device_ts, cycles })
}

/// A fixed FIFO of `N` samples: a full ring refuses the newest (counted by the caller), never allocates.
pub struct Ring<const N: usize> {
    buf: [Sample; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Ring<N> {
    pub const fn new() -> Self {
        Ring { buf: [SAMPLE_ZERO; N], head: 0, len: 0 }
    }

    /// `false` = full, `s` dropped.
    pub fn push(&mut self, s: Sample) -> bool {
        if self.len == N {
            return false;
        }
        self.buf[(self.head + self.len) % N] = s;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<Sample> {
        if self.len == 0 {
            return None;
        }
        let s = self.buf[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(s)
    }

    /// Empty the ring; the number of samples dropped.
    pub fn clear(&mut self) -> usize {
        let n = self.len;
        self.head = 0;
        self.len = 0;
        n
    }
}

/// The samples between the claim and the recorder: 512 (2.5 s of both sensors at 63 + 200 Hz).
pub const RING_LEN: usize = 512;
static RING: crate::sync::Mutex<Ring<RING_LEN>> = crate::sync::Mutex::new(Ring::new());
static ST_PUSHED: AtomicU32 = AtomicU32::new(0);
static ST_OVER: AtomicU32 = AtomicU32::new(0);
static ST_OTHER: AtomicU32 = AtomicU32::new(0);
static ST_SHORT: AtomicU32 = AtomicU32::new(0);

/// THE CLAIM TARGET (`xhci::isoch::claim_int`, every completed interrupt TD's bytes): decode and queue. The ring's lock
/// is held for one entry's copy only.
pub fn push_report(report: &[u8]) {
    match sample_of(report, crate::arch::now_cycles()) {
        Ok(s) => {
            if RING.lock().push(s) {
                ST_PUSHED.fetch_add(1, Ordering::Relaxed);
            } else {
                ST_OVER.fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(Skip::Short) => {
            ST_SHORT.fetch_add(1, Ordering::Relaxed);
        }
        Err(Skip::OtherId(_)) => {
            ST_OTHER.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The oldest queued sample (the ring's lock held for the pop only).
pub fn pop() -> Option<Sample> {
    RING.lock().pop()
}

/// Drop every queued sample (the recorder's start: what arrived before the capture is not in it); how many.
pub fn reset() -> usize {
    RING.lock().clear()
}

/// (samples queued, dropped on a full ring, reports of another id, short reports) since boot.
pub fn counts() -> (u32, u32, u32, u32) {
    (ST_PUSHED.load(Ordering::Relaxed), ST_OVER.load(Ordering::Relaxed), ST_OTHER.load(Ordering::Relaxed), ST_SHORT.load(Ordering::Relaxed))
}

/// The motion record for `s`: accel scaled (0.001 g per LSB, `imu::ACCEL_MPS2_PER_LSB`), gyro unscaled (its LSB needs
/// the firmware version, SONDE-RECORD-IMU-GVD owed), `ts_us` the recorder's clock.
pub fn motion_record(s: &Sample, seq: u64, ts_us: u64, extrinsic: Option<[f32; 12]>, id: [u8; ID_MAX]) -> MotionRecord {
    MotionRecord {
        kind: s.kind,
        raw: s.raw,
        device_ts: s.device_ts,
        scale: if s.kind == ImuKind::Accel { imu::ACCEL_MPS2_PER_LSB } else { 0.0 },
        seq,
        timestamp_us: ts_us,
        extrinsic,
        id,
    }
}

// ── The fixture (`tests sonderec` leg 1) ───────────────────────────────────────────────────────────────────────────

/// Every pure piece of this driver and the IMU table reader against hand-laid bytes; the first failure by name. Uses a
/// LOCAL ring, so a live pipe's samples never touch it.
pub fn fixture() -> Option<&'static str> {
    // 1. The HID interrupt-IN out of a configuration: a VideoStreaming interface with an isochronous IN (not taken), a HID
    //    interface (5) with a HID class descriptor, an OUT interrupt endpoint (not taken), then the IN at 0x84.
    #[rustfmt::skip]
    let cfg: [u8; 50] = [
        9, 2, 50, 0, 2, 1, 0, 0x80, 250,
        9, 4, 1, 1, 1, 0x0E, 0x02, 0, 0,
        7, 5, 0x81, 0x05, 0x00, 0x04, 1,
        9, 4, 5, 0, 2, 0x03, 0, 0, 0,
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 0x20, 0,
        7, 5, 0x04, 0x03, 0x40, 0x00, 4,
    ];
    if hid_ep_of(&cfg).is_some() {
        return Some("hid-ep.out-only");
    }
    let mut full = [0u8; 57];
    full[..50].copy_from_slice(&cfg);
    full[2] = 57;
    full[50..57].copy_from_slice(&[7, 5, 0x84, 0x03, 0x40, 0x00, 4]);
    match hid_ep_of(&full) {
        Some(e) if e.addr == 0x84 && e.mps == 64 && e.interval == 4 && e.intf == 5 && !e.ss => {}
        _ => return Some("hid-ep.in"),
    }
    // 2. The feature report: D0 at accel's code writes power @3 and 1000 / 1 @5; D4 with no rate leaves the report.
    let mut fr = [1u8, 0, 0, DEVICE_POWER_D4, 0, 0x22, 0x11, 0, 0];
    feature_enable(&mut fr, DEVICE_POWER_D0, Some(ACCEL_FPS_CODE));
    if fr != [1, 0, 0, 2, 0, 0xE8, 0x03, 0, 0] {
        return Some("feature.d0");
    }
    feature_enable(&mut fr, DEVICE_POWER_D4, None);
    if fr != [1, 0, 0, 6, 0, 0xE8, 0x03, 0, 0] {
        return Some("feature.d4");
    }
    feature_enable(&mut fr, DEVICE_POWER_D0, Some(GYRO_FPS_CODE));
    if u16::from_le_bytes([fr[5], fr[6]]) != 500 || feature_wvalue(2) != 0x0302 {
        return Some("feature.gyro");
    }
    // 3. The IMU table: MMER's command, and a dm_v2 table (valid flags, the D435i's CAD translation as data) with its CRC.
    let mut cmd = [0u8; rs::HWMON_CMD_LEN];
    let le32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if rs::hwmon_build_mmer(&mut cmd) != rs::HWMON_CMD_LEN || le32(&cmd[..], 4) != 0x4F || le32(&cmd[..], 8) != 0 || le32(&cmd[..], 12) != 520 {
        return Some("mmer.command");
    }
    let mut t = alloc::vec![0u8; rs::IMU_EEPROM_LEN as usize];
    t[0..2].copy_from_slice(&rs::DM_V2_EEPROM_ID.to_le_bytes());
    t[rs::DM_V2_EXTRINSIC_VALID] = 1;
    let extr: [f32; 12] = [-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, -0.00552, 0.0051, 0.01174];
    for (i, v) in extr.iter().enumerate() {
        let o = rs::DM_V2_DEPTH_TO_IMU + 4 * i;
        t[o..o + 4].copy_from_slice(&v.to_bits().to_le_bytes());
    }
    t[rs::DM_V2_GYRO + 36..rs::DM_V2_GYRO + 40].copy_from_slice(&0.5f32.to_bits().to_le_bytes());
    let crc = crate::hash::crc32(&t[rs::CALIB_HEADER_LEN..]);
    t[rs::CALIB_CRC_OFFSET..rs::CALIB_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    match rs::imu_calibration(&t) {
        Ok(c) if c.extrinsic_valid && !c.intrinsic_valid && c.depth_to_imu == extr && c.gyro[9] == 0.5 => {}
        _ => return Some("imucal.parse"),
    }
    let mut bad = t.clone();
    bad[300] ^= 1;
    if rs::imu_calibration(&bad).err() != Some("crc") {
        return Some("imucal.crc");
    }
    let mut tm1 = t.clone();
    tm1[0..2].copy_from_slice(&2u16.to_le_bytes());
    if rs::imu_calibration(&tm1).err() != Some("not-dm-v2") || rs::imu_calibration(&t[..100]).err() != Some("short-table") {
        return Some("imucal.refusals");
    }
    // 4. Reports → samples → the ring → motion records, through probe_core's wire.
    let report = |id: u8, ts: u64, x: i16, y: i16, z: i16| {
        let mut r = [0u8; imu::HID_IMU_REPORT_LEN];
        r[0] = id;
        r[2..10].copy_from_slice(&ts.to_le_bytes());
        r[10..12].copy_from_slice(&x.to_le_bytes());
        r[12..14].copy_from_slice(&y.to_le_bytes());
        r[14..16].copy_from_slice(&z.to_le_bytes());
        r
    };
    if sample_of(&report(3, 1, 0, 0, 0), 0) != Err(Skip::OtherId(3)) || sample_of(&report(1, 1, 0, 0, 0)[..31], 0) != Err(Skip::Short) {
        return Some("sample.refusals");
    }
    let mut ring: Ring<4> = Ring::new();
    let a = match sample_of(&report(1, 7, 12, -1003, 40), 100) {
        Ok(s) => s,
        Err(_) => return Some("sample.accel"),
    };
    let g = match sample_of(&report(2, 8, -7, 3, 32767), 200) {
        Ok(s) => s,
        Err(_) => return Some("sample.gyro"),
    };
    if !(ring.push(a) && ring.push(g) && ring.push(a) && ring.push(g)) || ring.push(a) {
        return Some("ring.full");
    }
    if ring.pop() != Some(a) || ring.pop() != Some(g) || ring.clear() != 2 || ring.pop().is_some() {
        return Some("ring.order");
    }
    let id = match wire::motion_id("uvc:8086:0b3a") {
        Ok(id) => id,
        Err(_) => return Some("wire.id"),
    };
    for (s, e) in [(a, Some(extr)), (g, None)] {
        let m = motion_record(&s, 0, 5, e, id);
        let Ok((b, n)) = wire::encode_motion(&m) else { return Some("wire.encode") };
        match wire::decode_motion(&b[..n]) {
            Ok(back) if back == m => {}
            _ => return Some("wire.round-trip"),
        }
        if wire::decode(&b[..n]).is_ok() || !wire::is_motion(&b[..n]) {
            return Some("wire.frame-refuses-motion");
        }
    }
    match motion_record(&a, 0, 5, None, id).sample() {
        Some(s) if s.xyz[1] == -1003.0 * imu::ACCEL_MPS2_PER_LSB && s.timestamp == 7 => {}
        _ => return Some("sample.si"),
    }
    if motion_record(&g, 0, 5, None, id).sample().is_some() {
        return Some("gyro.unscaled");
    }
    None
}
