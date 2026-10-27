//! CHARTER: Kernel — driver (the ACPI embedded controller transport, ZENBOOKEC zenbook-ledger Z8; no CODEX §2 handler owns a platform EC, the Apple SMC `drivers/smc.rs` sits in the same seam)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EC — the ACPI embedded controller, rung 1: the TRANSPORT and a census (ZENBOOKEC, zenbook-ledger Z8).
//!
//! On a non-Apple laptop the battery, the lid and the Fn keys live behind the ACPI EC (ACPI 6.5 §12), the job the
//! Apple SMC does on the rMBP. Its RAM layout is the DSDT's (an `EmbeddedControl` OperationRegion and its Fields),
//! which this tree cannot read: there is no AML interpreter (BP-power-4). So this rung reads the transport only:
//! * the ports come from the ECDT (EC_CONTROL GAS @36 = command/status, EC_DATA GAS @48 = data), located through the
//!   existing `acpi::root_sdt` + `acpi::find_table` and decoded by `acpi_power::gas_io_port`. NO ECDT, NO PORT I/O:
//!   an EC declared only in the DSDT (PNP0C09 `_CRS`) answers `[ec] -> SKIP reason=no-ecdt`;
//! * once per boot, at `pci.rs init` right after `hw::platform_say`, and only where `hw::gate(Probe::Ec)` answers
//!   (a positively identified non-Apple machine — the rMBP, QEMU and an unread SMBIOS print nothing), EC RAM
//!   0x00..0xFF is read with RD_EC and printed as eight `[ec] ram` lines: the witness ZENBOOK-EC-BATTERY decodes.
//!
//! WRITE SURFACE: RD_EC (0x80) + its address byte, and QR_EC (0x84) from the `_Qxx` event drain alone (ZENBOOKECQEVENTS
//! Z30, file tail), which dequeues one pending query byte. WR_EC (0x81), BE_EC/BD_EC (0x82/0x83) are never issued.
//! BOUNDED: every IBF/OBF wait has a deadline (`hw_wait_budget() / 200`, ~10 ms calibrated); the census stops at the
//! first stuck handshake and names the step and the status byte. `_Qxx` BYTES drained (Z30); their METHODS need AML: owed.
//! `tests ec` runs the protocol against a fake port (never a real one): `:: EC: proto=ok cases=5/5 -> PASS ::`.

use x86_64::instructions::port::Port;

/// RD_EC (ACPI §12.3.1): read one byte of EC address space.
const RD_EC: u8 = 0x80;
/// EC_SC status: output buffer full (a byte waits on the data port).
const OBF: u8 = 1 << 0;
/// EC_SC status: input buffer full (the EC has not yet taken the last byte written).
const IBF: u8 = 1 << 1;
/// The ECDT bytes read: the fixed part (65) and an EC_ID up to the rest.
const ECDT_MAX: usize = 128;
/// EC_ID bytes kept for the wire.
const ID_LEN: usize = 32;

/// The four port operations and the per-wait clock — the seam the KAT fakes.
trait EcIo {
    fn status(&mut self) -> u8;
    fn cmd(&mut self, v: u8);
    fn wdata(&mut self, v: u8);
    fn rdata(&mut self) -> u8;
    /// Start one wait's deadline.
    fn arm(&mut self);
    /// Has the wait armed last run out?
    fn expired(&mut self) -> bool;
}

/// Which handshake did not settle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Ibf,
    Obf,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Ibf => "ibf",
            Step::Obf => "obf",
        }
    }
}

/// A handshake that ran out of budget: the step and the last status byte read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stuck {
    step: Step,
    status: u8,
}

/// The real ports, bounded by `now_cycles`.
struct PortIo {
    cmd: u16,
    data: u16,
    start: u64,
    budget: u64,
}

impl EcIo for PortIo {
    fn status(&mut self) -> u8 {
        // SAFETY: the ECDT names this System I/O port as the EC's command/status register.
        unsafe { Port::<u8>::new(self.cmd).read() }
    }
    fn cmd(&mut self, v: u8) {
        // SAFETY: as above; only RD_EC or QR_EC is ever passed (the write surface in the header).
        unsafe { Port::<u8>::new(self.cmd).write(v) }
    }
    fn wdata(&mut self, v: u8) {
        // SAFETY: the ECDT names this System I/O port as the EC's data register; only an RD_EC address is written.
        unsafe { Port::<u8>::new(self.data).write(v) }
    }
    fn rdata(&mut self) -> u8 {
        // SAFETY: as above.
        unsafe { Port::<u8>::new(self.data).read() }
    }
    fn arm(&mut self) {
        self.start = crate::arch::now_cycles();
    }
    fn expired(&mut self) -> bool {
        crate::arch::now_cycles().wrapping_sub(self.start) >= self.budget
    }
}

/// Wait (bounded) for IBF clear or OBF set.
fn wait<I: EcIo>(io: &mut I, step: Step) -> Result<(), Stuck> {
    io.arm();
    loop {
        let s = io.status();
        let done = match step {
            Step::Ibf => s & IBF == 0,
            Step::Obf => s & OBF != 0,
        };
        if done {
            return Ok(());
        }
        if io.expired() {
            return Err(Stuck { step, status: s });
        }
        core::hint::spin_loop();
    }
}

/// One RD_EC transaction: IBF clear, 0x80, IBF clear, the address, OBF set, the byte. A byte already waiting at
/// entry (OBF set before the command) is drained once and reported as `true`.
fn rd_ec<I: EcIo>(io: &mut I, addr: u8) -> Result<(u8, bool), Stuck> {
    let stale = io.status() & OBF != 0;
    if stale {
        let _ = io.rdata();
    }
    wait(io, Step::Ibf)?;
    io.cmd(RD_EC);
    wait(io, Step::Ibf)?;
    io.wdata(addr);
    wait(io, Step::Obf)?;
    Ok((io.rdata(), stale))
}

/// Read `ram.len()` bytes from address 0 up, stopping at the first stuck handshake:
/// (bytes read, stale bytes drained, the stuck address and step).
fn census<I: EcIo>(io: &mut I, ram: &mut [u8]) -> (usize, u32, Option<(u8, Stuck)>) {
    let mut stale = 0u32;
    for (i, b) in ram.iter_mut().enumerate() {
        match rd_ec(io, i as u8) {
            Ok((v, s)) => {
                *b = v;
                stale += s as u32;
            }
            Err(e) => return (i, stale, Some((i as u8, e))),
        }
    }
    (ram.len(), stale, None)
}

/// The ECDT facts this rung uses.
struct Ecdt {
    cmd: u16,
    data: u16,
    uid: u32,
    gpe: u8,
    id: [u8; ID_LEN],
    idlen: usize,
}

/// Decode an ECDT image (header included). Both registers must be System I/O ports (`acpi_power::gas_io_port`).
fn parse_ecdt(t: &[u8]) -> Result<Ecdt, &'static str> {
    if t.len() < 65 || &t[0..4] != b"ECDT" {
        return Err("ecdt-short");
    }
    // SAFETY: `t.len() >= 65`, so the 12-byte GAS at 36 and the one at 48 lie inside the slice.
    let cmd = unsafe { crate::arch::acpi_power::gas_io_port(t.as_ptr() as usize + 36) };
    let data = unsafe { crate::arch::acpi_power::gas_io_port(t.as_ptr() as usize + 48) };
    let (Some(cmd), Some(data)) = (cmd, data) else { return Err("ecdt-not-io") };
    let uid = u32::from_le_bytes([t[60], t[61], t[62], t[63]]);
    let mut id = [0u8; ID_LEN];
    let mut idlen = 0;
    for &c in t[65..].iter().take_while(|&&c| c != 0).take(ID_LEN) {
        id[idlen] = if (0x20..0x7f).contains(&c) && c != b'"' { c } else { b'?' };
        idlen += 1;
    }
    Ok(Ecdt { cmd, data, uid, gpe: t[64], id, idlen })
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// `[ec] ram 0x<base>: <bytes as hex, space-separated>` (at most 32 bytes).
fn say_row(base: usize, bytes: &[u8]) {
    let mut buf = [0u8; 96];
    let mut n = 0;
    for (k, &b) in bytes.iter().take(32).enumerate() {
        if k > 0 {
            buf[n] = b' ';
            n += 1;
        }
        buf[n] = HEX[(b >> 4) as usize];
        buf[n + 1] = HEX[(b & 15) as usize];
        n += 2;
    }
    serial_println!("[ec] ram {:#04x}: {}", base, core::str::from_utf8(&buf[..n]).unwrap_or("?"));
}

/// The once-per-boot census (pci.rs init, after `hw::platform_say`). Silent unless `hw::gate(Probe::Ec)`.
pub fn probe() {
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        return;
    }
    let Some((sdt, es)) = crate::arch::acpi::root_sdt(crate::arch::acpi::rsdp_addr()) else {
        serial_println!("[ec] -> SKIP reason=no-rsdp");
        return;
    };
    // SAFETY: `root_sdt` vouched for the XSDT/RSDT and its entry size; firmware tables are identity-mapped.
    let Some(addr) = (unsafe { crate::arch::acpi::find_table(sdt, es, b"ECDT") }) else {
        serial_println!("[ec] -> SKIP reason=no-ecdt lid=?");
        return;
    };
    // SAFETY: `addr` is a table header listed by the root SDT; at most its own length (capped) is read.
    let len = unsafe { crate::arch::acpi::table_len(addr) }.min(ECDT_MAX);
    let t = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    let e = match parse_ecdt(t) {
        Ok(e) => e,
        Err(why) => {
            serial_println!("[ec] -> SKIP reason={}", why);
            return;
        }
    };
    serial_println!(
        "[ec] ecdt=yes ports=data:{:#x}/cmd:{:#x} uid={} gpe={} id=\"{}\"",
        e.data,
        e.cmd,
        e.uid,
        e.gpe,
        core::str::from_utf8(&e.id[..e.idlen]).unwrap_or("?")
    );
    let mut io = PortIo { cmd: e.cmd, data: e.data, start: 0, budget: (crate::arch::hw_wait_budget() / 200).max(1) }; PORTS.call_once(|| (e.cmd, e.data)); // ZENBOOKECBATTERY (zenbook-ledger Z20): the ECDT ports kept for `battery()` (file tail). LINE-NEUTRAL fold, code before the comment.
    let mut ram = [0u8; 256];
    let t0 = crate::arch::ms();
    let (n, stale, stuck) = census(&mut io, &mut ram);
    let ms = crate::arch::ms().saturating_sub(t0); let q = if stuck.is_none() { qevent_boot(&mut io) } else { 0 }; // ZENBOOKECQEVENTS (zenbook-ledger Z30): the boot drain of the `_Qxx` bytes queued before this census, with the census's own io (`qevent_boot`, file tail). LINE-NEUTRAL fold, code before the comment.
    let mut base = 0;
    while base < n {
        say_row(base, &ram[base..n.min(base + 32)]);
        base += 32;
    }
    match stuck {
        None => serial_println!("[ec] census bytes={} stale={} ms={} -> ok lid={} qevents={}", n, stale, ms, lid_census(&ram[..n]).word(), q),
        Some((at, s)) => serial_println!(
            "[ec] census bytes={} stale={} ms={} -> stuck at={:#04x} step={} status={:#04x} lid={}",
            n,
            stale,
            ms,
            at,
            s.step.name(),
            s.status,
            lid_census(&ram[..n]).word()
        ),
    }
}

/// The KAT's EC: IBF holds `lat` status polls after each written byte; RD_EC's address loads OBF with `ram[addr]`.
struct Fake {
    ram: [u8; 16],
    st: u8,
    out: u8,
    lat: u32,
    pend: u32,
    /// The byte the EC has not yet taken: 1 = a command, 2 = data.
    held: u8,
    held_v: u8,
    /// RD_EC taken, its address next.
    want_addr: bool,
    /// A command byte other than RD_EC was written.
    bad_cmd: bool,
    ibf_wedge: bool,
    obf_wedge: bool,
    polls: u32,
    budget: u32, q: [u8; 12], qn: usize, // ZENBOOKECQEVENTS (zenbook-ledger Z30): the fake's pending `_Qxx` queue, front first (`qr_take`, file tail)
}

impl Fake {
    fn new() -> Fake {
        let mut ram = [0u8; 16];
        for (i, b) in ram.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37) ^ 0x5a;
        }
        Fake { ram, st: 0, out: 0, lat: 3, pend: 0, held: 0, held_v: 0, want_addr: false, bad_cmd: false, ibf_wedge: false, obf_wedge: false, polls: 0, budget: 64, q: [0; 12], qn: 0 }
    }
    fn take(&mut self) {
        let v = self.held_v;
        match self.held {
            1 => {
                if v == RD_EC {
                    self.want_addr = true;
                } else if v == QR_EC { self.qr_take(); } else { // ZENBOOKECQEVENTS (zenbook-ledger Z30): QR_EC answers the queue's front (file tail)
                    self.bad_cmd = true;
                }
            }
            2 if self.want_addr => {
                self.want_addr = false;
                self.out = self.ram[(v as usize) % self.ram.len()];
                if !self.obf_wedge {
                    self.st |= OBF;
                }
            }
            _ => {}
        }
        self.held = 0;
        self.st &= !IBF;
    }
    fn hold(&mut self, kind: u8, v: u8) {
        self.held = kind;
        self.held_v = v;
        self.st |= IBF;
        self.pend = self.lat;
    }
}

impl EcIo for Fake {
    fn status(&mut self) -> u8 {
        if self.held != 0 && !self.ibf_wedge {
            if self.pend == 0 {
                self.take();
            } else {
                self.pend -= 1;
            }
        }
        self.st | self.sci() | if self.ibf_wedge { IBF } else { 0 } // ZENBOOKECQEVENTS (zenbook-ledger Z30): SCI_EVT while the queue holds a byte
    }
    fn cmd(&mut self, v: u8) {
        self.hold(1, v);
    }
    fn wdata(&mut self, v: u8) {
        self.hold(2, v);
    }
    fn rdata(&mut self) -> u8 {
        let v = if self.st & OBF != 0 { self.out } else { 0xff };
        self.st &= !OBF;
        v
    }
    fn arm(&mut self) {
        self.polls = 0;
    }
    fn expired(&mut self) -> bool {
        self.polls += 1;
        self.polls > self.budget
    }
}

fn kat_read() -> bool {
    let mut f = Fake::new();
    let ok = [0u8, 5, 9, 15].iter().all(|&a| rd_ec(&mut f, a) == Ok((f.ram[a as usize], false)));
    let mut ram = [0u8; 16];
    let (n, stale, stuck) = census(&mut f, &mut ram);
    ok && n == 16 && stale == 0 && stuck.is_none() && ram == f.ram && !f.bad_cmd
}

fn kat_stale() -> bool {
    let mut f = Fake::new();
    f.st = OBF;
    f.out = 0xee;
    rd_ec(&mut f, 3) == Ok((f.ram[3], true)) && !f.bad_cmd
}

fn kat_ibf() -> bool {
    let mut f = Fake::new();
    f.ibf_wedge = true;
    matches!(rd_ec(&mut f, 1), Err(Stuck { step: Step::Ibf, .. }))
}

fn kat_obf() -> bool {
    let mut f = Fake::new();
    f.obf_wedge = true;
    let mut ram = [0u8; 16];
    let (n, _, stuck) = census(&mut f, &mut ram);
    n == 0 && matches!(stuck, Some((0, Stuck { step: Step::Obf, .. })))
}

fn kat_ecdt() -> bool {
    let id = b"\\_SB.PCI0.LPC0.EC0\0";
    let mut t = [0u8; 65 + 19];
    t[0..4].copy_from_slice(b"ECDT");
    let tlen = t.len() as u32;
    t[4..8].copy_from_slice(&tlen.to_le_bytes());
    for (off, port) in [(36usize, 0x66u64), (48, 0x62)] {
        t[off] = 1; // System I/O
        t[off + 1] = 8;
        t[off + 3] = 1;
        t[off + 4..off + 12].copy_from_slice(&port.to_le_bytes());
    }
    t[64] = 0x0b;
    t[65..].copy_from_slice(id);
    let good = match parse_ecdt(&t) {
        Ok(e) => e.cmd == 0x66 && e.data == 0x62 && e.uid == 0 && e.gpe == 0x0b && &e.id[..e.idlen] == &id[..18],
        Err(_) => false,
    };
    t[36] = 0; // System Memory: refused, never a fabricated port
    good && matches!(parse_ecdt(&t), Err("ecdt-not-io")) && matches!(parse_ecdt(&t[..40]), Err("ecdt-short"))
}

/// `tests ec`: the RD_EC handshake and the ECDT decode against fakes; no real port is touched.
pub fn selftest() {
    let cases: [(&str, fn() -> bool); 5] = [("read", kat_read), ("stale", kat_stale), ("ibf", kat_ibf), ("obf", kat_obf), ("ecdt", kat_ecdt)];
    let mut ok = 0;
    let mut first: Option<&str> = None;
    for (name, f) in cases {
        let r = f();
        serial_println!("[ec] kat case={} {}", name, if r { "ok" } else { "FAIL" });
        if r {
            ok += 1;
        } else if first.is_none() {
            first = Some(name);
        }
    }
    match first {
        None => serial_println!(":: EC: proto=ok cases={}/{} -> PASS ::", ok, cases.len()),
        Some(name) => serial_println!(":: EC: proto=fail case={} -> FAIL ::", name),
    }
}

// ── ZENBOOKECBATTERY (zenbook-ledger Z20) — the battery decode by TABLE. TAIL-APPENDED, so no line above moves ─────────
// The desktop's battery item reads ONE seam, `video::status::board_raw` and its EC arm `status::ec_arm` (status.rs file
// tail), which asks `battery()` here. The EC RAM layout is the DSDT's (its `EmbeddedControl` Fields), and this tree runs
// no AML (BP-power-4), so the offsets come from a TABLE keyed by the platform token (`hw::platform().token()`), never from
// a guess. The table ships EMPTY: no row exists for zenbook_um5606w until the first boot's `[ec] ram` census (two dumps,
// on the adapter and off it, to separate the charging bit) or the DSDT's EC Field list names the offsets
// (ZENBOOK-EC-OFFSETS). Until then `battery()` answers `None` (absent) after one `[power] source=none … why=no-offsets`
// line. READ ONLY: the bytes go through `rd_ec` (RD_EC only, every wait bounded), at most six per read, stopping at the
// first stuck handshake. Silent on every machine `hw::gate(Probe::Ec)` refuses (the rMBP, QEMU), so their wires do not move.
// `tests ecbattery` decodes a canned row over the KAT's fake EC: `:: ECBATTERY: src=ec pct=73 charging=yes cases=5/5 -> PASS ::`.

/// The ECDT's (command, data) ports, kept by `probe` (the fold on its `PortIo` line). Unset = no ECDT was decoded.
static PORTS: spin::Once<(u16, u16)> = spin::Once::new();
/// The EC conversation of the runtime battery reads (the status `poll` and `live_tick` can run on two cores). Taken by
/// `try_lock` ONLY, never waited: busy = no reading this sweep, which the status model holds through (BATMON-HOLD). The
/// boot census runs at pci init, before any desktop service pass exists, and does not take it.
static TXN: crate::sync::Mutex<()> = crate::sync::Mutex::new(());
/// The `[power] source=` line has been printed (once per boot, at the first read).
static SOURCE_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// One battery reading out of EC RAM, in the status model's units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reading {
    /// State of charge, percent, 0..=100 (a byte past 100 is refused, never clamped).
    pub pct: u16,
    /// The row's charging bit is set.
    pub charging: bool,
    /// Current, signed mA: the rate's magnitude, positive while charging, negative otherwise.
    pub ma: i16,
    /// Voltage, mV.
    pub mv: u16,
}

/// Where one platform's battery fields sit in EC RAM. EC address space is little-endian; the rate is a magnitude whose
/// sign is the charging bit (the ACPI `_BST` rate + state convention).
struct BatteryOffsets {
    /// `hw::Platform::token()` of the machine this row describes.
    platform: &'static str,
    /// The state-of-charge byte (percent).
    pct: u8,
    /// The status byte, and the bit in it that is set while charging.
    status: u8,
    charging_bit: u8,
    /// The present rate, u16 LE, mA.
    rate: u8,
    /// The voltage, u16 LE, mV.
    volt: u8,
}

/// The rows. EMPTY: the Zenbook's offsets come from the first boot's `[ec] ram` dump (ZENBOOK-EC-OFFSETS); no row is
/// guessed, so until one lands every platform answers `[power] source=none … why=no-offsets`.
const EC_BATTERY_OFFSETS: &[BatteryOffsets] = &[];

/// The row for platform `tok` in `table`.
fn row_for<'a, R: EcRow>(tok: &str, table: &'a [R]) -> Option<&'a R> {
    table.iter().find(|r| r.platform() == tok)
}

/// Why a row's read gave no reading: a stuck handshake at an address, or a byte out of range at an address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Why {
    Stuck(u8, Stuck),
    Range(u8, u8),
}

/// Why `battery()` gave no reading.
enum Miss {
    NoOffsets,
    NoEcdt,
    Busy,
    Read(Why),
}

fn rd_byte<I: EcIo>(io: &mut I, addr: u8) -> Result<u8, Why> {
    rd_ec(io, addr).map(|(v, _)| v).map_err(|e| Why::Stuck(addr, e))
}

fn rd_le16<I: EcIo>(io: &mut I, addr: u8) -> Result<u16, Why> {
    let lo = rd_byte(io, addr)?;
    let hi = rd_byte(io, addr.wrapping_add(1))?;
    Ok(u16::from_le_bytes([lo, hi]))
}

/// Read and decode one row: percent, status, rate, voltage, in that order, stopping at the first failure.
fn read_row<I: EcIo>(io: &mut I, row: &BatteryOffsets) -> Result<Reading, Why> {
    let pct = rd_byte(io, row.pct)?;
    if pct > 100 {
        return Err(Why::Range(row.pct, pct));
    }
    let charging = (rd_byte(io, row.status)? >> (row.charging_bit & 7)) & 1 != 0;
    let mag = rd_le16(io, row.rate)?.min(i16::MAX as u16) as i16;
    let mv = rd_le16(io, row.volt)?;
    Ok(Reading { pct: pct as u16, charging, ma: if charging { mag } else { -mag }, mv })
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The battery through the EC, or `None` (absent). Silent and port-free unless `hw::gate(Probe::Ec)`; prints ONE
/// `[power] source=` line per boot at the first call. Called from the status model's EC arm only (`status::ec_arm`).
pub fn battery() -> Option<Reading> {
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        return None;
    }
    let tok = crate::hw::platform().token();
    let out = match row_for(tok, EC_BATTERY_OFFSETS) {
        None => Err(Miss::NoOffsets),
        Some(row) => conversation(|io| read_row(io, row)).and_then(|r| r.map_err(Miss::Read)), // ZENBOOKECLID (zenbook-ledger Z27): the ONE conversation (file tail)
    };
    if !SOURCE_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) {
        say_source(tok, &out);
    }
    out.ok()
}

/// `[power] source=ec|none platform=<token> …` — once per boot.
fn say_source(tok: &str, out: &Result<Reading, Miss>) {
    match out {
        Ok(r) => serial_println!("[power] source=ec platform={} pct={} charging={} ma={} mv={}", tok, r.pct, yes_no(r.charging), r.ma, r.mv),
        Err(Miss::NoOffsets) => serial_println!("[power] source=none platform={} why=no-offsets", tok),
        Err(Miss::NoEcdt) => serial_println!("[power] source=none platform={} why=no-ecdt", tok),
        Err(Miss::Busy) => serial_println!("[power] source=none platform={} why=busy", tok),
        Err(Miss::Read(Why::Stuck(at, s))) => serial_println!(
            "[power] source=none platform={} why=stuck at={:#04x} step={} status={:#04x}",
            tok,
            at,
            s.step.name(),
            s.status
        ),
        Err(Miss::Read(Why::Range(at, v))) => serial_println!("[power] source=none platform={} why=range at={:#04x} v={}", tok, at, v),
    }
}

/// The KAT's canned row (never in `EC_BATTERY_OFFSETS`): inside the fake EC's 16 bytes.
const KAT_ROW: BatteryOffsets = BatteryOffsets { platform: "kat", pct: 2, status: 3, charging_bit: 1, rate: 4, volt: 6 };

/// A fake EC carrying a battery at `KAT_ROW`: `pct`, `status`, 1200 mA, 12300 mV (little-endian).
fn kat_battery(pct: u8, status: u8) -> Fake {
    let mut f = Fake::new();
    f.ram[2] = pct;
    f.ram[3] = status;
    f.ram[4..6].copy_from_slice(&1200u16.to_le_bytes());
    f.ram[6..8].copy_from_slice(&12300u16.to_le_bytes());
    f
}

fn bkat_discharge() -> bool {
    let mut f = kat_battery(41, 0x01); // another bit set, the charging bit (1) clear
    read_row(&mut f, &KAT_ROW) == Ok(Reading { pct: 41, charging: false, ma: -1200, mv: 12300 }) && !f.bad_cmd
}

fn bkat_range() -> bool {
    let mut f = kat_battery(0xc8, 0x02);
    read_row(&mut f, &KAT_ROW) == Err(Why::Range(2, 0xc8))
}

fn bkat_stuck() -> bool {
    let mut f = kat_battery(73, 0x02);
    f.obf_wedge = true;
    matches!(read_row(&mut f, &KAT_ROW), Err(Why::Stuck(2, Stuck { step: Step::Obf, .. })))
}

fn bkat_table() -> bool {
    row_for("zenbook_um5606w", EC_BATTERY_OFFSETS).is_none()
        && row_for("kat", &[KAT_ROW]).map(|r| r.pct) == Some(2)
        && row_for("qemu", &[KAT_ROW]).is_none()
}

/// `tests ecbattery`: the table decode through `rd_ec` over the fake EC; no real port is touched.
pub fn battery_selftest() {
    let mut f = kat_battery(73, 0x02);
    let charge = read_row(&mut f, &KAT_ROW);
    let charge_ok = charge == Ok(Reading { pct: 73, charging: true, ma: 1200, mv: 12300 }) && !f.bad_cmd;
    let cases: [(&str, bool); 7] =
        [("charge", charge_ok), ("discharge", bkat_discharge()), ("range", bkat_range()), ("stuck", bkat_stuck()), ("table", bkat_table()), ("lid", bkat_lid()), ("qevents", bkat_qevents())]; // ZENBOOKECLID (zenbook-ledger Z27): the lid leg (file tail); ZENBOOKECQEVENTS (zenbook-ledger Z30): the qevents leg (file tail)
    let mut ok = 0;
    let mut first: Option<&str> = None;
    for (name, r) in cases {
        serial_println!("[ec] battery kat case={} {}", name, if r { "ok" } else { "FAIL" });
        if r {
            ok += 1;
        } else if first.is_none() {
            first = Some(name);
        }
    }
    match (first, charge) {
        (None, Ok(r)) => serial_println!(":: ECBATTERY: src=ec pct={} charging={} cases={}/{} -> PASS ::", r.pct, yes_no(r.charging), ok, cases.len()),
        (name, _) => serial_println!(":: ECBATTERY: case={} -> FAIL ::", name.unwrap_or("charge")),
    }
}

// ── ZENBOOKECLID (zenbook-ledger Z27) — the lid through the EC, by TABLE, and the probe that finds its offset. TAIL-APPENDED ─
// The lid's EC field is the DSDT's (`_LID` reads an `EmbeddedControl` Field) and this tree runs no AML (BP-power-4); no DSDT
// dump exists in the tree and nothing has flown, so the lid is a TABLE row keyed by the platform token like the battery's,
// and the table ships EMPTY (no guessed row): until ZENBOOK-EC-LIDROW lands every platform answers `lid=?`. `lid()` is the
// EC source beside the SMC's `MSLD` in `video::lidsleep::read_msld` (its `Absent` arm, `ec_lid`); the census line carries
// `lid=` decoded from the RAM it already read. `tests eclid` (registered, never at boot) is the bench's way to the offset:
// three 256-byte snapshots, lid open / closed / open, and every address with A == C and A != B printed. READ ONLY: RD_EC
// through `rd_byte`/`census`, under the ONE `conversation` (the ECDT ports + the `TXN` try_lock + a bounded `PortIo`),
// which `battery()` shares. `tests ecbattery`'s `lid` case decodes a canned row over the KAT's fake EC.

/// A table row's platform token — the ONE lookup `row_for` serves both tables through.
trait EcRow {
    fn platform(&self) -> &'static str;
}

impl EcRow for BatteryOffsets {
    fn platform(&self) -> &'static str {
        self.platform
    }
}

/// Where one platform's lid bit sits in EC RAM.
struct LidOffset {
    /// `hw::Platform::token()` of the machine this row describes.
    platform: &'static str,
    /// The byte, and the bit in it.
    addr: u8,
    bit: u8,
    /// The bit reads 1 while the lid is closed (else 0 while closed).
    closed_when_set: bool,
}

impl EcRow for LidOffset {
    fn platform(&self) -> &'static str {
        self.platform
    }
}

/// The rows. EMPTY: the Zenbook's lid offset comes from the bench's `tests eclid` (or the DSDT's `_LID` Field), never a guess.
const EC_LID_OFFSETS: &[LidOffset] = &[];

/// The lid as the EC answers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lid {
    Open,
    Closed,
    /// A row exists but this read gave nothing (the conversation busy, a stuck handshake): hold the last reading.
    Unknown,
    /// No EC lid here: the gate refused, no row for this platform, or no ECDT.
    Absent,
}

impl Lid {
    pub fn word(self) -> &'static str {
        match self {
            Lid::Open => "open",
            Lid::Closed => "closed",
            Lid::Unknown | Lid::Absent => "?",
        }
    }
}

/// The `[ec] lid source=` line has been printed (once per boot, at the first `lid()`).
static LID_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The ONE runtime EC conversation: the ECDT ports `probe` kept, the `TXN` lock by `try_lock` only (busy = no reading),
/// and a bounded `PortIo` handed to `f` while the lock is held.
fn conversation<T>(f: impl FnOnce(&mut PortIo) -> T) -> Result<T, Miss> {
    let Some(&(cmd, data)) = PORTS.get() else { return Err(Miss::NoEcdt) };
    let Some(_txn) = TXN.try_lock() else { return Err(Miss::Busy) };
    let mut io = PortIo { cmd, data, start: 0, budget: (crate::arch::hw_wait_budget() / 200).max(1) };
    Ok(f(&mut io))
}

/// `true` = closed, by the row's bit and polarity.
fn lid_bit(byte: u8, row: &LidOffset) -> bool {
    ((byte >> (row.bit & 7)) & 1 != 0) == row.closed_when_set
}

/// One lid read through `rd_byte` (`true` = closed).
fn read_lid<I: EcIo>(io: &mut I, row: &LidOffset) -> Result<bool, Why> {
    rd_byte(io, row.addr).map(|b| lid_bit(b, row))
}

/// The lid decoded from RAM already read (no port I/O): `Absent` with no row, `Unknown` past the bytes read.
fn lid_census_in(ram: &[u8], row: Option<&LidOffset>) -> Lid {
    let Some(r) = row else { return Lid::Absent };
    match ram.get(r.addr as usize) {
        Some(&b) if lid_bit(b, r) => Lid::Closed,
        Some(_) => Lid::Open,
        None => Lid::Unknown,
    }
}

/// The census line's `lid=`: this platform's row over the census bytes.
fn lid_census(ram: &[u8]) -> Lid {
    lid_census_in(ram, row_for(crate::hw::platform().token(), EC_LID_OFFSETS))
}

fn miss_word(m: &Miss) -> &'static str {
    match m {
        Miss::NoOffsets => "no-offsets",
        Miss::NoEcdt => "no-ecdt",
        Miss::Busy => "busy",
        Miss::Read(Why::Stuck(..)) => "stuck",
        Miss::Read(Why::Range(..)) => "range",
    }
}

/// The lid through the EC. Silent and port-free (`Absent`) unless `hw::gate(Probe::Ec)`; prints ONE
/// `[ec] lid source=ec|none platform=<token> …` line per boot at the first call. Called from `video::lidsleep`'s `ec_lid`.
pub fn lid() -> Lid {
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        return Lid::Absent;
    }
    let tok = crate::hw::platform().token();
    let out = match row_for(tok, EC_LID_OFFSETS) {
        None => Err(Miss::NoOffsets),
        Some(row) => conversation(|io| read_lid(io, row)).and_then(|r| r.map_err(Miss::Read)),
    };
    if !LID_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) {
        match &out {
            Ok(c) => serial_println!("[ec] lid source=ec platform={} lid={}", tok, if *c { "closed" } else { "open" }),
            Err(m) => serial_println!("[ec] lid source=none platform={} why={}", tok, miss_word(m)),
        }
    }
    match out {
        Ok(true) => Lid::Closed,
        Ok(false) => Lid::Open,
        Err(Miss::NoOffsets) | Err(Miss::NoEcdt) => Lid::Absent,
        Err(Miss::Busy) | Err(Miss::Read(_)) => Lid::Unknown,
    }
}

/// One 256-byte snapshot through `census` inside the conversation; a busy lock is retried once after 50 ms.
fn lid_snap(ram: &mut [u8; 256]) -> Result<(), Miss> {
    let mut tries = 0;
    loop {
        match conversation(|io| census(io, &mut ram[..])) {
            Ok((_, _, None)) => return Ok(()),
            Ok((_, _, Some((at, s)))) => return Err(Miss::Read(Why::Stuck(at, s))),
            Err(Miss::Busy) if tries == 0 => {
                tries += 1;
                crate::arch::sched::sleep_ms(50);
            }
            Err(m) => return Err(m),
        }
    }
}

/// `tests eclid` (registered, never at boot): the bench's way to the lid offset. Snapshot A (lid open), 10 s to CLOSE the
/// lid, snapshot B, 10 s to OPEN it, snapshot C; every address with A == C and A != B is a candidate. READ ONLY (RD_EC).
pub fn lid_probe() {
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        serial_println!(":: ECLID: -> SKIP reason=no-ec ::");
        return;
    }
    if PORTS.get().is_none() {
        serial_println!(":: ECLID: -> SKIP reason=no-ecdt ::");
        return;
    }
    let mut snaps = [[0u8; 256]; 3];
    for k in 0..3 {
        if k == 1 {
            serial_println!("[ec] lid probe: CLOSE the lid within 10 s");
            crate::arch::sched::sleep_ms(10_000);
        } else if k == 2 {
            serial_println!("[ec] lid probe: OPEN the lid within 10 s");
            crate::arch::sched::sleep_ms(10_000);
        }
        match lid_snap(&mut snaps[k]) {
            Ok(()) => {}
            Err(Miss::Read(Why::Stuck(at, s))) => {
                serial_println!(":: ECLID: snap={} -> FAIL :: reason=stuck at={:#04x} step={} status={:#04x}", k, at, s.step.name(), s.status);
                return;
            }
            Err(m) => {
                serial_println!(":: ECLID: snap={} -> FAIL :: reason={}", k, miss_word(&m));
                return;
            }
        }
    }
    let mut n = 0u32;
    for a in 0..256usize {
        let (o, c, o2) = (snaps[0][a], snaps[1][a], snaps[2][a]);
        if o == o2 && o != c {
            n += 1;
            serial_println!("[ec] lid probe addr={:#04x} open={:#04x} closed={:#04x} xor={:#04x}", a, o, c, o ^ c);
        }
    }
    if n >= 1 {
        serial_println!(":: ECLID: candidates={} -> PASS ::", n);
    } else {
        serial_println!(":: ECLID: candidates=0 -> FAIL :: reason=no-flip");
    }
}

/// The KAT's canned lid rows (never in `EC_LID_OFFSETS`): bit 2 of byte 9, both polarities.
const KAT_LID: LidOffset = LidOffset { platform: "kat", addr: 9, bit: 2, closed_when_set: true };
const KAT_LID_INV: LidOffset = LidOffset { platform: "kat", addr: 9, bit: 2, closed_when_set: false };

/// `tests ecbattery`'s `lid` case: set/clear x polarity through `rd_ec`, the census decode agrees with the read, a stuck
/// handshake names the address, and no Zenbook row is shipped.
fn bkat_lid() -> bool {
    let mut f = Fake::new();
    f.ram[9] = 0x04 | 0x01;
    let set = read_lid(&mut f, &KAT_LID) == Ok(true) && read_lid(&mut f, &KAT_LID_INV) == Ok(false);
    let census_closed = lid_census_in(&f.ram, Some(&KAT_LID)) == Lid::Closed;
    f.ram[9] = 0x01 | 0x08;
    let clear = read_lid(&mut f, &KAT_LID) == Ok(false) && read_lid(&mut f, &KAT_LID_INV) == Ok(true);
    let census_open = lid_census_in(&f.ram, Some(&KAT_LID)) == Lid::Open
        && lid_census_in(&f.ram, None) == Lid::Absent
        && lid_census_in(&f.ram[..4], Some(&KAT_LID)) == Lid::Unknown;
    let mut w = Fake::new();
    w.obf_wedge = true;
    let stuck = matches!(read_lid(&mut w, &KAT_LID), Err(Why::Stuck(9, Stuck { step: Step::Obf, .. })));
    set && census_closed && clear && census_open && stuck && !f.bad_cmd && row_for("zenbook_um5606w", EC_LID_OFFSETS).is_none()
}

// ── ZENBOOKECQEVENTS (zenbook-ledger Z30) — the EC's `_Qxx` query bytes, drained by a polled pass. TAIL-APPENDED ────────────
// An ACPI EC raises SCI_EVT (EC_SC bit 5) when it has an event queued; QR_EC (0x84) dequeues it and answers the query byte
// whose `_Qxx` method the OS would run (ACPI 6.5 §12.3.5). This tree runs no AML (BP-power-4), so the METHODS cannot run:
// this section delivers the BYTES (ZENBOOK-EC-QHANDLERS owed). There is no SCI path either: no GPE0 block reader exists, the
// SCI GSI is never routed (`ioapic::route_gsi`'s callers are i8042 and PCI INTx) and ACPI mode is requested only at S5, so
// no SCI reaches the kernel, nothing storms and no GPE status needs clearing (ZENBOOK-EC-GPE owed). The shape is the rMBP's:
// the SMC is polled, the fixed power button is a polled PM1 read, and the drain is a polled pass — once at the census
// (`qevent_boot`, the census's own io) and then from the desktop service pass beside `status::poll`, at the battery sweep's
// cadence (`qevent_service`, `Q_POLL_MS`), through the ONE `conversation`. At most `Q_PER_PASS` bytes per pass; each byte
// is counted (`qevents=` on the census line) and a byte not seen before this boot prints `[ec] q=0x<nn> total=<n>` once.
// Silent and port-free on every machine `hw::gate(Probe::Ec)` refuses (the rMBP, QEMU). `tests ecbattery`'s `qevents` case.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// QR_EC (ACPI §12.3.5): dequeue the pending query byte.
const QR_EC: u8 = 0x84;
/// EC_SC status: an SCI event is pending (a query byte waits for QR_EC).
const SCI_EVT: u8 = 1 << 5;
/// The most query bytes one pass drains.
const Q_PER_PASS: usize = 8;
/// The service drain's cadence: the battery sweep's, `video::status::POLL_MS` (10 s). Restated, not named: `status` is
/// `wc`-gated and this module is always-on x86.
const Q_POLL_MS: u64 = 10_000;

/// Query bytes drained this boot.
static QEVENTS: AtomicU32 = AtomicU32::new(0);
/// The seen-table: one bit per query byte, set at its first sighting this boot.
static SEEN: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
/// `arch::ms()` of the last service drain (0 = none yet); a compare_exchange, so two cores never both sweep.
static LAST_Q_MS: AtomicU64 = AtomicU64::new(0);
/// The `[ec] qevents stuck` line has been printed (once per boot).
static QSTUCK_SAID: AtomicBool = AtomicBool::new(false);

/// One QR_EC transaction: a stale byte drained, IBF clear, 0x84, OBF set, the query byte (0 = no event pending).
fn qr_ec<I: EcIo>(io: &mut I) -> Result<u8, Stuck> {
    if io.status() & OBF != 0 {
        let _ = io.rdata();
    }
    wait(io, Step::Ibf)?;
    io.cmd(QR_EC);
    wait(io, Step::Obf)?;
    Ok(io.rdata())
}

/// Drain while SCI_EVT is set, at most `Q_PER_PASS` bytes into `out`: (bytes drained, the stuck handshake if one stopped
/// it). A 0 answer (no event after all) stops the pass uncounted.
fn qdrain<I: EcIo>(io: &mut I, out: &mut [u8; Q_PER_PASS]) -> (usize, Option<Stuck>) {
    for i in 0..Q_PER_PASS {
        if io.status() & SCI_EVT == 0 {
            return (i, None);
        }
        match qr_ec(io) {
            Ok(0) => return (i, None),
            Ok(q) => out[i] = q,
            Err(e) => return (i, Some(e)),
        }
    }
    (Q_PER_PASS, None)
}

/// Mark `q` in `bits`; `true` at its first sighting.
fn note_seen(bits: &[AtomicU32; 8], q: u8) -> bool {
    let m = 1u32 << (q & 31);
    bits[(q >> 5) as usize].fetch_or(m, Ordering::Relaxed) & m == 0
}

/// Count each drained byte; print `[ec] q=0x<nn> total=<n>` for a byte not seen before this boot.
fn qsay(bytes: &[u8]) {
    for &q in bytes {
        let total = QEVENTS.fetch_add(1, Ordering::Relaxed) + 1;
        if note_seen(&SEEN, q) {
            serial_println!("[ec] q={:#04x} total={}", q, total);
        }
    }
}

/// `[ec] qevents stuck step=<ibf|obf> status=0x<ss>` — once per boot.
fn say_qstuck(s: Stuck) {
    if !QSTUCK_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[ec] qevents stuck step={} status={:#04x}", s.step.name(), s.status);
    }
}

/// The boot drain (`probe`, after a census that did not stick), with the census's own io: no service pass exists yet, so
/// no `TXN`, exactly as the census runs. Returns the bytes drained this boot (the census line's `qevents=`).
fn qevent_boot(io: &mut PortIo) -> u32 {
    let mut b = [0u8; Q_PER_PASS];
    let (n, st) = qdrain(io, &mut b);
    qsay(&b[..n]);
    if let Some(s) = st {
        say_qstuck(s);
    }
    QEVENTS.load(Ordering::Relaxed)
}

/// The service drain (`desktop_uefi::desktop_app_service`, beside `status::poll`): every `Q_POLL_MS`, through the ONE
/// `conversation`; no ECDT or a busy lock = nothing this pass, silently. Port-free unless `hw::gate(Probe::Ec)`.
pub fn qevent_service() {
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        return;
    }
    let now = crate::arch::ms();
    let last = LAST_Q_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < Q_POLL_MS {
        return;
    }
    if LAST_Q_MS.compare_exchange(last, now.max(1), Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    let mut b = [0u8; Q_PER_PASS];
    if let Ok((n, st)) = conversation(|io| qdrain(io, &mut b)) {
        qsay(&b[..n]);
        if let Some(s) = st {
            say_qstuck(s);
        }
    }
}

impl Fake {
    /// QR_EC taken: the queue's front (0 when empty) loads OBF and leaves the queue.
    fn qr_take(&mut self) {
        let v = if self.qn > 0 { self.q[0] } else { 0 };
        if self.qn > 0 {
            self.q.copy_within(1.., 0);
            self.qn -= 1;
        }
        self.out = v;
        if !self.obf_wedge {
            self.st |= OBF;
        }
    }
    /// SCI_EVT while the queue holds a byte (a real EC drops it once the last query byte is read).
    fn sci(&self) -> u8 {
        if self.qn > 0 { SCI_EVT } else { 0 }
    }
    /// A fake EC with `bytes` queued (at most 12).
    fn with_q(bytes: &[u8]) -> Fake {
        let mut f = Fake::new();
        f.q[..bytes.len()].copy_from_slice(bytes);
        f.qn = bytes.len();
        f
    }
}

/// `tests ecbattery`'s `qevents` case: three bytes drained in order with SCI_EVT clear after and the seen-table counting two
/// distinct; the bound at `Q_PER_PASS`; no SCI_EVT = no command; a 0 answer uncounted; a wedged OBF named; RD_EC after a
/// drain still reads RAM. Local seen-table only: the boot's `SEEN`/`QEVENTS` are never touched.
fn bkat_qevents() -> bool {
    let mut f = Fake::with_q(&[0x52, 0x1d, 0x52]);
    let mut b = [0u8; Q_PER_PASS];
    let (n, st) = qdrain(&mut f, &mut b);
    let seen: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
    let fresh = [note_seen(&seen, b[0]), note_seen(&seen, b[1]), note_seen(&seen, b[2])];
    let distinct = fresh.iter().filter(|&&x| x).count();
    let three = n == 3 && st.is_none() && b[..3] == [0x52, 0x1d, 0x52] && f.status() & SCI_EVT == 0 && fresh == [true, true, false] && !f.bad_cmd;
    let after = rd_ec(&mut f, 5) == Ok((f.ram[5], false)) && !f.bad_cmd;

    let mut g = Fake::with_q(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    let mut gb = [0u8; Q_PER_PASS];
    let (gn, gst) = qdrain(&mut g, &mut gb);
    let bound = gn == Q_PER_PASS && gst.is_none() && gb == [1, 2, 3, 4, 5, 6, 7, 8] && g.qn == 2 && g.status() & SCI_EVT != 0;

    let mut h = Fake::new();
    let mut hb = [0u8; Q_PER_PASS];
    let (hn, hst) = qdrain(&mut h, &mut hb);
    let idle = hn == 0 && hst.is_none() && h.held_v == 0 && h.status() & OBF == 0;

    let mut z = Fake::with_q(&[0]);
    let mut zb = [0u8; Q_PER_PASS];
    let (zn, zst) = qdrain(&mut z, &mut zb);
    let zero = zn == 0 && zst.is_none() && z.qn == 0 && z.held_v == QR_EC;

    let mut w = Fake::with_q(&[0x52]);
    w.obf_wedge = true;
    let mut wb = [0u8; Q_PER_PASS];
    let (wn, wst) = qdrain(&mut w, &mut wb);
    let wedge = wn == 0 && matches!(wst, Some(Stuck { step: Step::Obf, .. }));

    serial_println!("[ec] qevents kat drained={} distinct={} q={:#04x} q={:#04x} bound={}", n, distinct, b[0], b[1], gn);
    three && after && bound && idle && zero && wedge
}
