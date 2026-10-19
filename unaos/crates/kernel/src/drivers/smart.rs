//! CHARTER: Kernel — driver (SMARTLINE, rmbp-ledger B708: the AHCI driver's SMART half — the user's own disk health, read-only; BP-bar-1)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SMARTLINE — ATA SMART, parsed per ATA8-ACS: the disk tells the user how it is, no vendor tool between them.
//!
//! Declared at `ahci.rs`'s tail as `#[path = "smart.rs"] pub mod smart;` — a CHILD of `ahci`, so it exists exactly
//! where the driver does (x86_64 + `ahci`) and `drivers/mod.rs` is untouched. The command is `ahci::smart_at`
//! (READ DATA 0xD0 and RETURN STATUS 0xDA, both read-only); everything here is pure over byte pages, plus one cache.
//!
//! * IDENTIFY: model (words 27-46), serial (10-19), firmware (23-26) through ahci's own `ata_string` / `trimmed`;
//!   TRIM = word 169 bit 0; SMART supported / enabled = words 82 / 85 bit 0 (0x0000 and 0xFFFF read as unreported).
//! * The SMART READ DATA page: revision (bytes 0-1), 30 entries of 12 bytes from offset 2 — id, flags (u16), value,
//!   worst, raw (48-bit LE), reserved; id 0 is an empty slot. The 512 bytes sum to 0 mod 256 (the checksum is
//!   reported, never a refusal: some drives ship a zero checksum byte).
//! * Health, the first trip wins: RETURN STATUS exceeded -> `status`; 5 raw > 0 -> `realloc`; 197 raw > 0 ->
//!   `pending`; 198 raw > 0 -> `offline_unc`; wear >= [`WARN_WEAR_PCT`] -> `wear`; temperature >= [`WARN_TEMP_C`]
//!   -> `temp`; else OK. No page -> `na`, never a guessed OK.
//! * The cache ([`DISKS`]) is written at the probe and by `tests smartline`; readers ([`disks`]) take it with
//!   `try_lock` and copy, so the render task (the Settings pane) never issues a command nor waits on a disk.
//!
//! Wire: `[disk] smart model="<m>" hours=<n> wear_pct=<n> realloc=<n> pending=<n> temp_c=<n> health=<ok|warn|na>
//! port=<p> smart=<ok|unsupported> attrs=<n> fw="<f>" trim=<0|1> tripped=<name|->` (an unreported value is `na`).

use super::{ata_string, id_word, trimmed, MAX_AHCI_DISKS, SECTOR_BYTES};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Display;

/// ATA8-ACS: the attribute table holds 30 entries of 12 bytes from offset 2.
pub const ATTR_SLOTS: usize = 30;
const ATTR_BASE: usize = 2;
const ATTR_LEN: usize = 12;

/// The common attribute ids this module names.
pub const ID_REALLOC: u8 = 5;
pub const ID_HOURS: u8 = 9;
pub const ID_CYCLES: u8 = 12;
pub const ID_WEAR_LEVEL: u8 = 177;
pub const ID_AIRFLOW_TEMP: u8 = 190;
pub const ID_TEMP: u8 = 194;
pub const ID_PENDING: u8 = 197;
pub const ID_OFFLINE_UNC: u8 = 198;
pub const ID_LIFE_LEFT: u8 = 231;

/// Wear (percent of the rated life used) at or above which the disk reads WARN.
pub const WARN_WEAR_PCT: u8 = 90;
/// Temperature (C) at or above which the disk reads WARN.
pub const WARN_TEMP_C: u8 = 70;

/// The attribute's name as the Storage card says it ("" for an id this module does not name).
pub fn name_of(id: u8) -> &'static str {
    match id {
        ID_REALLOC => "Reallocated sectors",
        ID_HOURS => "Power-on hours",
        ID_CYCLES => "Power cycles",
        ID_WEAR_LEVEL => "Wear leveling",
        ID_AIRFLOW_TEMP => "Airflow temperature",
        ID_TEMP => "Temperature",
        ID_PENDING => "Pending sectors",
        ID_OFFLINE_UNC => "Offline uncorrectable",
        ID_LIFE_LEFT => "SSD life left",
        _ => "",
    }
}

/// One attribute entry.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Attr {
    pub id: u8,
    pub flags: u16,
    pub value: u8,
    pub worst: u8,
    pub raw: u64,
}

/// One SMART READ DATA page, parsed.
#[derive(Clone, Copy)]
pub struct Table {
    pub attrs: [Attr; ATTR_SLOTS],
    pub n: usize,
    pub revision: u16,
    pub checksum_ok: bool,
}

impl Table {
    pub fn get(&self, id: u8) -> Option<&Attr> {
        self.attrs[..self.n].iter().find(|a| a.id == id)
    }
}

/// Parse a SMART READ DATA page. Pure.
pub fn parse_table(buf: &[u8; SECTOR_BYTES]) -> Table {
    let sum = buf.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    let mut t = Table { attrs: [Attr::default(); ATTR_SLOTS], n: 0, revision: u16::from_le_bytes([buf[0], buf[1]]), checksum_ok: sum == 0 };
    for i in 0..ATTR_SLOTS {
        let e = &buf[ATTR_BASE + i * ATTR_LEN..ATTR_BASE + (i + 1) * ATTR_LEN];
        if e[0] == 0 {
            continue;
        }
        let mut raw = 0u64;
        for k in 0..6 {
            raw |= (e[5 + k] as u64) << (8 * k);
        }
        t.attrs[t.n] = Attr { id: e[0], flags: u16::from_le_bytes([e[1], e[2]]), value: e[3], worst: e[4], raw };
        t.n += 1;
    }
    t
}

/// The IDENTIFY facts the Storage card and the boot line carry.
#[derive(Clone, Copy)]
pub struct Ident {
    pub model: [u8; 40],
    pub serial: [u8; 20],
    pub firmware: [u8; 8],
    pub sectors: u64,
    pub trim: bool,
    pub smart_supported: bool,
    pub smart_enabled: bool,
}

impl Ident {
    pub fn model(&self) -> &str {
        trimmed(&self.model)
    }
    pub fn serial(&self) -> &str {
        trimmed(&self.serial)
    }
    pub fn firmware(&self) -> &str {
        trimmed(&self.firmware)
    }
}

/// Decode the IDENTIFY words this module needs; `sectors` is ahci's own decode (words 60-61 / 100-103). Pure.
pub fn parse_ident(buf: &[u8; SECTOR_BYTES], sectors: u64) -> Ident {
    let (mut model, mut serial, mut firmware) = ([b' '; 40], [b' '; 20], [b' '; 8]);
    ata_string(buf, 27, &mut model);
    ata_string(buf, 10, &mut serial);
    ata_string(buf, 23, &mut firmware);
    let reported = |w: u16| w != 0 && w != 0xFFFF;
    let (w82, w85, w169) = (id_word(buf, 82), id_word(buf, 85), id_word(buf, 169));
    let smart_supported = reported(w82) && w82 & 1 != 0;
    Ident {
        model,
        serial,
        firmware,
        sectors,
        trim: reported(w169) && w169 & 1 != 0,
        smart_supported,
        smart_enabled: smart_supported && reported(w85) && w85 & 1 != 0,
    }
}

/// SMART RETURN STATUS's answer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Exceeded,
    Unknown,
}

/// The status FIS's LBA mid / high, read. Pure.
pub fn status_of(mid: u8, high: u8) -> Status {
    match (mid, high) {
        (0x4F, 0xC2) => Status::Ok,
        (0xF4, 0x2C) => Status::Exceeded,
        _ => Status::Unknown,
    }
}

/// What tripped a WARN.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trip {
    Status,
    Realloc(u64),
    Pending(u64),
    OfflineUnc(u64),
    Wear(u8),
    Temp(u8),
}

impl Trip {
    /// The wire's `tripped=` word.
    pub fn wire(&self) -> &'static str {
        match self {
            Trip::Status => "status",
            Trip::Realloc(_) => "realloc",
            Trip::Pending(_) => "pending",
            Trip::OfflineUnc(_) => "offline_unc",
            Trip::Wear(_) => "wear",
            Trip::Temp(_) => "temp",
        }
    }
    /// The card's words for it.
    pub fn label(&self) -> String {
        match *self {
            Trip::Status => String::from("the drive reports a threshold exceeded"),
            Trip::Realloc(n) => alloc::format!("{} {}", name_of(ID_REALLOC), n),
            Trip::Pending(n) => alloc::format!("{} {}", name_of(ID_PENDING), n),
            Trip::OfflineUnc(n) => alloc::format!("{} {}", name_of(ID_OFFLINE_UNC), n),
            Trip::Wear(p) => alloc::format!("Wear {}%", p),
            Trip::Temp(c) => alloc::format!("Temperature {} C", c),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warn(Trip),
    Na,
}

impl Health {
    pub fn wire(&self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Warn(_) => "warn",
            Health::Na => "na",
        }
    }
}

/// The named values of one disk and its verdict.
#[derive(Clone, Copy)]
pub struct Summary {
    pub hours: Option<u64>,
    pub cycles: Option<u64>,
    pub realloc: Option<u64>,
    pub pending: Option<u64>,
    pub offline_unc: Option<u64>,
    pub wear_pct: Option<u8>,
    pub temp_c: Option<u8>,
    pub health: Health,
}

/// Reduce a page (and the status) to the named values and the verdict. Pure.
pub fn summarize(t: Option<&Table>, status: Status) -> Summary {
    let Some(t) = t else {
        return Summary { hours: None, cycles: None, realloc: None, pending: None, offline_unc: None, wear_pct: None, temp_c: None, health: Health::Na };
    };
    let raw = |id: u8| t.get(id).map(|a| a.raw);
    let wear_pct = t.get(ID_WEAR_LEVEL).or_else(|| t.get(ID_LIFE_LEFT)).map(|a| 100 - a.value.min(100));
    let temp_c = t.get(ID_TEMP).or_else(|| t.get(ID_AIRFLOW_TEMP)).map(|a| (a.raw & 0xFF) as u8).filter(|&c| c != 0);
    let (realloc, pending, offline_unc) = (raw(ID_REALLOC), raw(ID_PENDING), raw(ID_OFFLINE_UNC));
    let health = if status == Status::Exceeded {
        Health::Warn(Trip::Status)
    } else if let Some(n) = realloc.filter(|&n| n > 0) {
        Health::Warn(Trip::Realloc(n))
    } else if let Some(n) = pending.filter(|&n| n > 0) {
        Health::Warn(Trip::Pending(n))
    } else if let Some(n) = offline_unc.filter(|&n| n > 0) {
        Health::Warn(Trip::OfflineUnc(n))
    } else if let Some(p) = wear_pct.filter(|&p| p >= WARN_WEAR_PCT) {
        Health::Warn(Trip::Wear(p))
    } else if let Some(c) = temp_c.filter(|&c| c >= WARN_TEMP_C) {
        Health::Warn(Trip::Temp(c))
    } else {
        Health::Ok
    };
    Summary { hours: raw(ID_HOURS).map(|h| h & 0xFFFF_FFFF), cycles: raw(ID_CYCLES), realloc, pending, offline_unc, wear_pct, temp_c, health }
}

/// SMART for one disk: the page and the status, or why there is none.
#[derive(Clone, Copy)]
pub enum Smart {
    Unsupported(&'static str),
    Read(Table, Status),
}

/// One disk's record — what the boot line, the Storage card and the fixture read.
#[derive(Clone, Copy)]
pub struct Disk {
    /// The AHCI registry index (`ahci::read_block_at`'s key) and the HBA port.
    pub ix: usize,
    pub port: u8,
    pub ident: Ident,
    pub smart: Smart,
    pub sum: Summary,
}

impl Disk {
    pub fn attrs(&self) -> usize {
        match &self.smart {
            Smart::Read(t, _) => t.n,
            Smart::Unsupported(_) => 0,
        }
    }
}

/// One entry per AHCI registry index. Written at the probe and by the fixture; read by [`disks`].
static DISKS: crate::sync::Mutex<[Option<Disk>; MAX_AHCI_DISKS]> = crate::sync::Mutex::new([None; MAX_AHCI_DISKS]);

/// Issue SMART on registry index `ix` (skipped when IDENTIFY does not advertise it), parse, cache, return.
pub fn read(ix: usize, port: u8, ident: Ident) -> Disk {
    let smart = if !ident.smart_supported {
        Smart::Unsupported("not-supported")
    } else if !ident.smart_enabled {
        Smart::Unsupported("disabled")
    } else {
        let mut page = [0u8; SECTOR_BYTES];
        match super::smart_at(ix, &mut page) {
            Ok(st) => Smart::Read(parse_table(&page), st.map_or(Status::Unknown, |(m, h)| status_of(m, h))),
            Err(()) => Smart::Unsupported("aborted"),
        }
    };
    let sum = match &smart {
        Smart::Read(t, st) => summarize(Some(t), *st),
        Smart::Unsupported(_) => summarize(None, Status::Unknown),
    };
    let d = Disk { ix, port, ident, smart, sum };
    if ix < MAX_AHCI_DISKS {
        DISKS.lock()[ix] = Some(d);
    }
    d
}

/// The probe's call, once per published disk (`ahci::bring_up_port`): read, cache, say.
pub fn boot(ix: usize, port: u8, idbuf: &[u8; SECTOR_BYTES], sectors: u64) {
    let d = read(ix, port, parse_ident(idbuf, sectors));
    say(&d);
}

fn na<T: Display>(v: Option<T>) -> String {
    match v {
        Some(x) => alloc::format!("{}", x),
        None => String::from("na"),
    }
}

/// The `[disk] smart` line.
pub fn say(d: &Disk) {
    let s = &d.sum;
    let smart = match &d.smart {
        Smart::Read(..) => String::from("ok"),
        Smart::Unsupported(why) => alloc::format!("unsupported reason={}", why),
    };
    let tripped = match s.health {
        Health::Warn(t) => t.wire(),
        _ => "-",
    };
    serial_println!(
        "[disk] smart model=\"{}\" hours={} wear_pct={} realloc={} pending={} temp_c={} health={} port={} smart={} attrs={} fw=\"{}\" trim={} tripped={}",
        d.ident.model(), na(s.hours), na(s.wear_pct), na(s.realloc), na(s.pending), na(s.temp_c), s.health.wire(),
        d.port, smart, d.attrs(), d.ident.firmware(), d.ident.trim as u8, tripped
    );
}

/// The cached disks, copied out under `try_lock` (busy = none this frame): the render task's door, never a command.
pub fn disks() -> Vec<Disk> {
    match DISKS.try_lock() {
        Some(g) => g.iter().flatten().copied().collect(),
        None => Vec::new(),
    }
}

/// `tests smartline` leg 1 — the parse on a synthetic page: the named values, the temperature's low raw byte (194
/// packs min/max above it), a reallocated sector trips `realloc`, an exceeded status trips `status`, a flipped byte
/// reads checksum bad. `Err` names the first leg that misread. Pure.
pub fn parse_selftest() -> Result<(), &'static str> {
    fn put(page: &mut [u8; SECTOR_BYTES], slot: usize, id: u8, value: u8, raw: u64) {
        let o = ATTR_BASE + slot * ATTR_LEN;
        page[o] = id;
        page[o + 1] = 0x03;
        page[o + 3] = value;
        page[o + 4] = value;
        for k in 0..6 {
            page[o + 5 + k] = (raw >> (8 * k)) as u8;
        }
    }
    fn seal(page: &mut [u8; SECTOR_BYTES]) {
        page[511] = 0;
        let sum = page.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        page[511] = 0u8.wrapping_sub(sum);
    }
    let mut page = [0u8; SECTOR_BYTES];
    page[0] = 0x10;
    put(&mut page, 0, ID_REALLOC, 100, 0);
    put(&mut page, 1, ID_HOURS, 99, 1234);
    put(&mut page, 2, ID_CYCLES, 99, 56);
    put(&mut page, 3, ID_WEAR_LEVEL, 97, 41);
    put(&mut page, 5, ID_TEMP, 59, 41 | (20 << 16) | (55 << 32)); // slot 4 left empty: an id-0 hole is skipped
    put(&mut page, 6, ID_PENDING, 100, 0);
    put(&mut page, 7, ID_OFFLINE_UNC, 100, 0);
    seal(&mut page);
    let t = parse_table(&page);
    if !t.checksum_ok || t.n != 7 || t.revision != 0x10 {
        return Err("table");
    }
    let s = summarize(Some(&t), Status::Ok);
    if s.hours != Some(1234) || s.cycles != Some(56) || s.wear_pct != Some(3) || s.temp_c != Some(41) || s.realloc != Some(0) || s.health != Health::Ok {
        return Err("named");
    }
    if summarize(Some(&t), Status::Exceeded).health != Health::Warn(Trip::Status) {
        return Err("status");
    }
    put(&mut page, 0, ID_REALLOC, 100, 8);
    seal(&mut page);
    let t = parse_table(&page);
    if summarize(Some(&t), Status::Ok).health != Health::Warn(Trip::Realloc(8)) {
        return Err("realloc");
    }
    page[ATTR_BASE + 4 * ATTR_LEN + 3] ^= 0x01; // inside the empty slot 4: the table reads the same, the sum does not
    if parse_table(&page).checksum_ok {
        return Err("checksum");
    }
    if status_of(0x4F, 0xC2) != Status::Ok || status_of(0xF4, 0x2C) != Status::Exceeded || summarize(None, Status::Ok).health != Health::Na {
        return Err("status-fis");
    }
    Ok(())
}
