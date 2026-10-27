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
//! Z30, file tail), which dequeues one pending query byte; GPE0_STS W1C of the ECDT's one GPE bit after a drain (ZENBOOKECGPE Z36, file tail); on the Zenbook the SCI arm: GPE0_EN (the EC's bit alone), every GPE0_STS bit W1C'd by the SCI handler, ACPI_ENABLE at boot (ZENBOOKSCI Z41, file tail). WR_EC (0x81), BE_EC/BD_EC (0x82/0x83) are never issued.
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
pub fn probe() { crate::arch::acpi_power::gpe0_say(); // ZENBOOKECGPE (zenbook-ledger Z36): the `[acpi] gpe0=` census, once per x86 boot, BEFORE the EC gate (pci.rs init calls this on every x86 boot). LINE-NEUTRAL fold, code before the comment.
    if !crate::hw::gate(crate::hw::Probe::Ec) {
        return;
    }
    qnames_boot(); let Some((sdt, es)) = crate::arch::acpi::root_sdt(crate::arch::acpi::rsdp_addr()) else { // ZENBOOKECQNAMES (zenbook-ledger Z51): the DSDT's `_Qxx` method names and Notify targets, read once past the EC gate and before the ECDT lookup, so a no-ECDT machine still prints them and the drain below can name its bytes (`qnames_boot`, file tail). LINE-NEUTRAL fold, code before the comment.
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
    let mut io = PortIo { cmd: e.cmd, data: e.data, start: 0, budget: (crate::arch::hw_wait_budget() / 200).max(1) }; PORTS.call_once(|| (e.cmd, e.data)); EC_GPE.call_once(|| e.gpe); // ZENBOOKECGPE (zenbook-ledger Z36): the ECDT's GPE number kept for `gpe_pass` (file tail). ZENBOOKECBATTERY (zenbook-ledger Z20): the ECDT ports kept for `battery()` (file tail). LINE-NEUTRAL fold, code before the comment.
    let mut ram = [0u8; 256];
    let t0 = crate::arch::ms();
    let (n, stale, stuck) = census(&mut io, &mut ram);
    let ms = crate::arch::ms().saturating_sub(t0); let q = if stuck.is_none() { qevent_boot(&mut io) } else { 0 }; let sci = if stuck.is_none() { sci_arm() } else { "census-stuck" }; // ZENBOOKSCI (zenbook-ledger Z41): the SCI arm after a census that did not stick and after the boot drain (`sci_arm`, file tail; the Zenbook alone). ZENBOOKECQEVENTS (zenbook-ledger Z30): the boot drain of the `_Qxx` bytes queued before this census, with the census's own io (`qevent_boot`, file tail). LINE-NEUTRAL fold, code before the comment.
    let mut base = 0;
    while base < n {
        say_row(base, &ram[base..n.min(base + 32)]);
        base += 32;
    }
    match stuck {
        None => serial_println!("[ec] census bytes={} stale={} ms={} -> ok lid={} qevents={} sci={}", n, stale, ms, lid_census(&ram[..n]).word(), q, sci),
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
    budget: u32, q: [u8; 12], qn: usize, gsts: u8, gclr: u32, // ZENBOOKECGPE (zenbook-ledger Z36): the fake's GPE0_STS byte and its W1C count (`impl GpeSts for Fake`, file tail). ZENBOOKECQEVENTS (zenbook-ledger Z30): the fake's pending `_Qxx` queue, front first (`qr_take`, file tail)
}

impl Fake {
    fn new() -> Fake {
        let mut ram = [0u8; 16];
        for (i, b) in ram.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37) ^ 0x5a;
        }
        Fake { ram, st: 0, out: 0, lat: 3, pend: 0, held: 0, held_v: 0, want_addr: false, bad_cmd: false, ibf_wedge: false, obf_wedge: false, polls: 0, budget: 64, q: [0; 12], qn: 0, gsts: 0, gclr: 0 }
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
    let cases: [(&str, bool); 9] =
        [("charge", charge_ok), ("discharge", bkat_discharge()), ("range", bkat_range()), ("stuck", bkat_stuck()), ("table", bkat_table()), ("lid", bkat_lid()), ("qevents", bkat_qevents()), ("gpe", bkat_gpe()), ("sci", bkat_sci())]; // ZENBOOKSCI (zenbook-ledger Z41): the sci leg (file tail); ZENBOOKECGPE (zenbook-ledger Z36): the gpe leg (file tail); ZENBOOKECLID (zenbook-ledger Z27): the lid leg (file tail); ZENBOOKECQEVENTS (zenbook-ledger Z30): the qevents leg (file tail)
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
            serial_println!("[ec] q={:#04x} total={}{}", q, total, QArrow(q));
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
    qsay(&b[..n]); gpe_pass(n); // ZENBOOKECGPE (zenbook-ledger Z36): the EC's GPE0_STS bit read, and cleared after a drain that consumed events (`gpe_pass`, file tail). LINE-NEUTRAL fold.
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
    let now = crate::arch::ms(); sci_service(now); // ZENBOOKSCI (zenbook-ledger Z41): the SCI's storm mask and its `[acpi] sci n=` line, task context (`sci_service`, file tail). LINE-NEUTRAL fold.
    let last = LAST_Q_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < Q_POLL_MS && !sci_due() { // ZENBOOKSCI (Z41): an EC SCI makes the drain due now, not at the 10 s poll.
        return;
    }
    if LAST_Q_MS.compare_exchange(last, now.max(1), Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    let mut b = [0u8; Q_PER_PASS]; sci_take(); // ZENBOOKSCI (Z41): the pending flag cleared BEFORE the drain, so an SCI during it re-sets it. LINE-NEUTRAL fold.
    let r = conversation(|io| qdrain(io, &mut b)); sci_busy(r.is_err()); if let Ok((n, st)) = r { // ZENBOOKSCI (Z41): no conversation = the SCI stays pending, the EC's enable stays off.
        qsay(&b[..n]); gpe_pass(n); sci_rearm(); // ZENBOOKSCI (zenbook-ledger Z41): the EC's GPE0_EN bit back on after the drain (`sci_rearm`, file tail). ZENBOOKECGPE (zenbook-ledger Z36): the same GPE step on the service pass (`gpe_pass`, file tail). LINE-NEUTRAL fold.
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

// ── ZENBOOKECGPE (zenbook-ledger Z36) — the EC's GPE0_STS bit as a second witness of its events. TAIL-APPENDED ───────────
// The ECDT names the EC's GPE number (`parse_ecdt`, t[64]); the FADT names the GPE0 block (`acpi_power::gpe0`, its file
// tail). An EC with an event queued sets SCI_EVT AND its GPE status bit; this tree routes no SCI (ZENBOOK-SCI owed), so
// the bit is POLLED on the drain passes Z30 already runs (`qevent_boot`, `qevent_service`: no new pass, no new cadence):
// read on each pass, printed once per change, and cleared (write-1-to-clear of the ONE bit, never another) only after a
// pass whose drain consumed a query byte, and only in ACPI mode (SCI_EN=1 at the census): in legacy mode the GPE registers
// belong to the firmware's SMI handler, so there the bit is read and never written (`why=legacy`). No GPE0_EN write, no
// SCI route, no ACPI_ENABLE. The ONE writer of GPE0_STS: the boot drain (single-threaded) and the service drain
// (serialized by `LAST_Q_MS`'s compare_exchange). Silent on every machine `hw::gate(Probe::Ec)` refuses (the drains are).
// `tests ecbattery`'s `gpe` case runs it over the fake EC's GPE0_STS byte.

use core::sync::atomic::AtomicU8;

/// The ECDT's GPE number, kept by `probe` (the fold on its `PortIo` line). Unset = no ECDT was decoded.
static EC_GPE: spin::Once<u8> = spin::Once::new();
/// The bit's last printed state (2 = none printed yet).
static GPE_LAST: AtomicU8 = AtomicU8::new(2);
/// The `[ec] gpe -> SKIP` line has been printed (once per boot).
static GPE_SKIP_SAID: AtomicBool = AtomicBool::new(false);

/// One GPE0_STS byte: read, and write-1-to-clear — the seam the KAT fakes.
trait GpeSts {
    fn sts(&mut self) -> u8;
    fn w1c(&mut self, mask: u8);
}

/// The real GPE0_STS byte at the FADT's port.
struct GpePort(u16);

impl GpeSts for GpePort {
    fn sts(&mut self) -> u8 {
        // SAFETY: the FADT's own GPE0_STS byte (`acpi_power::Gpe0::sts_bit`); a read has no side effect.
        unsafe { Port::<u8>::new(self.0).read() }
    }
    fn w1c(&mut self, mask: u8) {
        // SAFETY: write-1-to-clear; zero bits are no-ops, so only the EC's one bit is touched.
        unsafe { Port::<u8>::new(self.0).write(mask) }
    }
}

impl GpeSts for Fake {
    fn sts(&mut self) -> u8 {
        self.gsts
    }
    fn w1c(&mut self, mask: u8) {
        self.gsts &= !mask;
        self.gclr += 1;
    }
}

/// One pass's GPE step: (the bit was set, it was cleared). Cleared only when set, the drain consumed events and
/// `may_clear` (ACPI mode).
fn gpe_step<G: GpeSts>(g: &mut G, mask: u8, drained: usize, may_clear: bool) -> (bool, bool) {
    let set = g.sts() & mask != 0;
    if set && drained > 0 && may_clear {
        g.w1c(mask);
        return (true, true);
    }
    (set, false)
}

/// `[ec] gpe -> SKIP reason=<why> gpe=<n>` — once per boot.
fn gpe_skip(why: &str, gpe: u8) {
    if !GPE_SKIP_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[ec] gpe -> SKIP reason={} gpe={}", why, gpe);
    }
}

/// The GPE step after a drain pass of `drained` bytes (`qevent_boot`, `qevent_service`):
/// `[ec] gpe sts=<0|1> gpe=<n> port=0x<p> drained=<k> cleared=<0|1>` once per change of the bit.
fn gpe_pass(drained: usize) {
    let Some(&gpe) = EC_GPE.get() else { return };
    let Some(g) = crate::arch::acpi_power::gpe0() else { return gpe_skip("no-fadt", gpe) };
    let Some((port, mask)) = g.sts_bit(gpe) else { return gpe_skip(if g.blk == 0 { "no-gpe0" } else { "gpe1" }, gpe) };
    let may_clear = g.sci_en == Some(true) || sci_armed(); // ZENBOOKSCI (Z41): `gpe0()`'s Once froze the pre-arm SCI_EN.
    let (set, cleared) = gpe_step(&mut GpePort(port), mask, drained, may_clear);
    if GPE_LAST.swap(set as u8, Ordering::Relaxed) != set as u8 {
        serial_println!(
            "[ec] gpe sts={} gpe={} port={:#x} drained={} cleared={}{}",
            set as u8,
            gpe,
            port,
            drained,
            cleared as u8,
            if set && !may_clear { " why=legacy" } else { "" }
        );
    }
}

/// `tests ecbattery`'s `gpe` case: the GPE0_STS locate (q35's block, a GPE1 bit, no block), then over the fake EC's
/// GPE0_STS byte: the bit set and a drain of 0 = read, no write; a drain of 1 = the one bit cleared, the others kept; an
/// idle pass = no write; legacy mode (`may_clear=false`) = no write. The boot's statics are never touched.
fn bkat_gpe() -> bool {
    let q35 = crate::arch::acpi_power::Gpe0 { blk: 0x620, len: 16, sci: 9, sci_en: Some(true) };
    let locate = q35.sts_bit(0x0b) == Some((0x621, 0x08))
        && q35.sts_bit(0x40).is_none()
        && crate::arch::acpi_power::Gpe0 { blk: 0, ..q35 }.sts_bit(0x0b).is_none();

    let mut f = Fake::with_q(&[0x52]);
    f.gsts = 0x89;
    let before = gpe_step(&mut f, 0x08, 0, true) == (true, false) && f.gclr == 0 && f.gsts == 0x89;
    let mut b = [0u8; Q_PER_PASS];
    let (n, st) = qdrain(&mut f, &mut b);
    let drained = n == 1 && st.is_none() && b[0] == 0x52;
    let clear = gpe_step(&mut f, 0x08, n, true) == (true, true) && f.gsts == 0x81 && f.gclr == 1;
    let (n2, _) = qdrain(&mut f, &mut b);
    let idle = n2 == 0 && gpe_step(&mut f, 0x08, n2, true) == (false, false) && f.gclr == 1;

    let mut l = Fake::new();
    l.gsts = 0x08;
    let legacy = gpe_step(&mut l, 0x08, 1, false) == (true, false) && l.gclr == 0 && l.gsts == 0x08;

    serial_println!("[ec] gpe kat locate={} drained={} gsts={:#04x} clears={} legacy_writes={}", locate as u8, n, f.gsts, f.gclr, l.gclr);
    locate && before && drained && clear && idle && legacy && !f.bad_cmd
}

// ── ZENBOOKSCI (zenbook-ledger Z41) — the EC's SCI: ACPI mode, GPE0_EN for the EC bit alone, the SCI GSI routed. TAIL ──────
// Before Z41 the drain was polled only (`qevent_service` every `Q_POLL_MS`): no GPE0_EN writer, the SCI GSI never routed,
// ACPI mode only at S5. `sci_arm` (from `probe`, after a census that did not stick and after `qevent_boot`; the Zenbook
// ALONE — `hw::platform()`, narrower than `hw::gate(Probe::Ec)`, so no other non-Apple machine enters ACPI mode at boot)
// enters ACPI mode through `acpi_power::acpi_mode_enter` (the existing `enter_acpi_mode`), writes GPE0_EN whole through
// `acpi_power::gpe0_en_only` (every enable 0, every status cleared, then the ECDT's GPE bit), allocates a vector
// (`interrupts::vectors::alloc`), resolves the SCI through the ONE ISO walk (`ioapic::iso_gsi`, level / active-low by
// default) and routes it in i8042's shape (`route_gsi` masked, then `set_mask`). The handler is LOCK-FREE (port I/O and
// atomics): the EC's status bit set = its enable byte 0 (held off until drained) and `SCI_PENDING`; EVERY set GPE0_STS bit
// is write-1-cleared (the firmware-armed GPEs this tree cannot service never hold the level line); more than
// `SCI_STORM_N` SCIs inside one `SCI_WIN_MS` with no EC bit = every GPE0_EN byte 0 and `SCI_STORM`. The service pass
// (task context) drains (the ONE `qdrain` through the ONE `conversation`), re-enables the EC bit, masks the IOAPIC entry
// after a storm, and prints. PM1_EN is never touched (the fixed power button stays polled, BP-power-11).
// `tests ecbattery`'s `sci` case runs the step, the drain on its flag, the re-arm and the storm guard over fakes.

use core::sync::atomic::AtomicU16;

/// Non-EC SCIs inside one window that mean a storm.
const SCI_STORM_N: u32 = 50;
/// The storm window.
const SCI_WIN_MS: u64 = 1000;

/// The GPE0 block port the handler reads (0 = not armed; stored LAST before the vector exists).
static SCI_BLK: AtomicU16 = AtomicU16::new(0);
/// GPE0_BLK_LEN / 2: the status half's length (the enable half follows it).
static SCI_HALF: AtomicU8 = AtomicU8::new(0);
/// The EC's bit: status-byte index << 8 | mask.
static SCI_ECBIT: AtomicU16 = AtomicU16::new(0);
/// The routed GSI (the storm's IOAPIC mask).
static SCI_GSI: AtomicU32 = AtomicU32::new(u32::MAX);
/// The SCI is armed (routed and unmasked).
static SCI_ON: AtomicBool = AtomicBool::new(false);
/// An EC SCI arrived and its drain has not run.
static SCI_PENDING: AtomicBool = AtomicBool::new(false);
/// The pending flag this service pass took (`sci_take`), so a busy conversation can give it back.
static SCI_TAKEN: AtomicBool = AtomicBool::new(false);
/// The storm guard fired: every GPE0_EN byte is 0 and stays 0.
static SCI_STORM: AtomicBool = AtomicBool::new(false);
/// The storm line has been printed (and the IOAPIC entry masked).
static SCI_STORM_SAID: AtomicBool = AtomicBool::new(false);
/// SCIs taken / with the EC bit / with another GPE bit / with no GPE0 status bit at all.
static SCI_N: AtomicU32 = AtomicU32::new(0);
static SCI_EC: AtomicU32 = AtomicU32::new(0);
static SCI_OTHER: AtomicU32 = AtomicU32::new(0);
static SCI_NONGPE: AtomicU32 = AtomicU32::new(0);
/// The storm window's start (`arch::ms()`, 0 = none) and its non-EC count.
#[cfg(feature = "ioapic")]
static SCI_WIN: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "ioapic")]
static SCI_WN: AtomicU32 = AtomicU32::new(0);
/// `SCI_N` at the last `[acpi] sci n=` line, and when it printed.
static SCI_SAID_N: AtomicU32 = AtomicU32::new(0);
static SCI_SAID_MS: AtomicU64 = AtomicU64::new(0);

/// The GPE0 block byte by byte (offsets from the block: status half first, enable half at `half`) — the seam the KAT fakes.
trait GpeBlock {
    fn rd(&mut self, off: u16) -> u8;
    fn wr(&mut self, off: u16, v: u8);
}

/// The real GPE0 block at the FADT's port.
struct GpePorts(u16);

impl GpeBlock for GpePorts {
    fn rd(&mut self, off: u16) -> u8 {
        // SAFETY: an offset inside the FADT's GPE0 block (`sci_arm` stored its port and half); a read has no side effect.
        unsafe { Port::<u8>::new(self.0 + off).read() }
    }
    fn wr(&mut self, off: u16, v: u8) {
        // SAFETY: a status byte (write-1-to-clear of bits read set) or an enable byte this kernel owns since `sci_arm`.
        unsafe { Port::<u8>::new(self.0 + off).write(v) }
    }
}

/// The KAT's GPE0 block: status bytes write-1-to-clear below `half`, enable bytes plain from `half`.
struct FakeGpe {
    r: [u8; 16],
    half: u16,
    writes: u32,
}

impl GpeBlock for FakeGpe {
    fn rd(&mut self, off: u16) -> u8 {
        self.r[off as usize]
    }
    fn wr(&mut self, off: u16, v: u8) {
        self.writes += 1;
        if off < self.half {
            self.r[off as usize] &= !v;
        } else {
            self.r[off as usize] = v;
        }
    }
}

/// What one SCI found in GPE0_STS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SciHit {
    /// The EC's bit was set (its enable byte is now 0).
    ec: bool,
    /// Another bit was set.
    other: bool,
    /// Any bit was set.
    any: bool,
}

/// The handler's GPE step: every status byte read; the EC's bit set = its enable byte 0; every set bit written back
/// (write-1-to-clear). No lock, no EC conversation.
fn sci_step<G: GpeBlock>(g: &mut G, half: u16, ecbit: u16) -> SciHit {
    let (eb, em) = (ecbit >> 8, (ecbit & 0xff) as u8);
    let mut hit = SciHit { ec: false, other: false, any: false };
    for i in 0..half {
        let s = g.rd(i);
        if s == 0 {
            continue;
        }
        hit.any = true;
        let ec_mask = if i == eb { em } else { 0 };
        if s & ec_mask != 0 {
            hit.ec = true;
            g.wr(half + eb, 0);
        }
        if s & !ec_mask != 0 {
            hit.other = true;
        }
        g.wr(i, s);
    }
    hit
}

/// One non-EC SCI at `now`: `true` = more than `SCI_STORM_N` inside the current `SCI_WIN_MS` window.
fn storm_hit(win: &AtomicU64, cnt: &AtomicU32, now: u64) -> bool {
    let w = win.load(Ordering::Relaxed);
    if w == 0 || now.wrapping_sub(w) >= SCI_WIN_MS {
        win.store(now.max(1), Ordering::Relaxed);
        cnt.store(1, Ordering::Relaxed);
        return false;
    }
    cnt.fetch_add(1, Ordering::Relaxed) + 1 > SCI_STORM_N
}

/// Every GPE0_EN byte 0 (the storm guard).
fn mask_all<G: GpeBlock>(g: &mut G, half: u16) {
    for i in 0..half {
        g.wr(half + i, 0);
    }
}

/// The EC's enable bit back on (we own every enable bit and only the EC's is ever 1, so the byte is written whole).
fn sci_rearm_on<G: GpeBlock>(g: &mut G, half: u16, ecbit: u16) {
    g.wr(half + (ecbit >> 8), (ecbit & 0xff) as u8);
}

/// `[acpi] sci -> SKIP reason=<reason>[-<detail>]`; returns the census line's `sci=` word.
fn sci_skip(reason: &'static str, detail: &str) -> &'static str {
    if detail.is_empty() {
        serial_println!("[acpi] sci -> SKIP reason={}", reason);
    } else {
        serial_println!("[acpi] sci -> SKIP reason={}-{}", reason, detail);
    }
    reason
}

/// The Zenbook alone reaches the SCI arm.
fn sci_platform() -> bool {
    matches!(crate::hw::platform(), crate::hw::Platform::ZenbookUm5606w)
}

/// The route's mode word.
#[cfg(feature = "ioapic")]
fn mode_word(trig: crate::arch::ioapic::Trigger, pol: crate::arch::ioapic::Polarity) -> &'static str {
    use crate::arch::ioapic::{Polarity, Trigger};
    match (trig, pol) {
        (Trigger::Level, Polarity::ActiveLow) => "level-low",
        (Trigger::Level, _) => "level-high",
        (Trigger::Edge, Polarity::ActiveLow) => "edge-low",
        _ => "edge-high",
    }
}

/// The arm's undo before the SCI is live: every GPE0_EN byte 0 and the handler disarmed.
#[cfg(feature = "ioapic")]
fn sci_off(g: &crate::arch::acpi_power::Gpe0) {
    SCI_ON.store(false, Ordering::Release);
    SCI_BLK.store(0, Ordering::Release);
    let _ = crate::arch::acpi_power::gpe0_en_only(g, None);
}

/// The SCI arm (`probe`, once): `on`, or the SKIP reason (the census line's `sci=` word). The Zenbook alone.
#[cfg(feature = "ioapic")]
fn sci_arm() -> &'static str {
    use crate::arch::ioapic::{self, Polarity, Trigger};
    if !sci_platform() {
        return "platform";
    }
    let Some(&gpe) = EC_GPE.get() else { return sci_skip("no-ecdt-gpe", "") };
    let Some(g) = crate::arch::acpi_power::gpe0() else { return sci_skip("no-fadt", "") };
    let (Some((sts, mask)), Some(_)) = (g.sts_bit(gpe), g.en_bit(gpe)) else {
        return sci_skip(if g.blk == 0 { "no-gpe0" } else { "gpe1" }, "");
    };
    let already = match crate::arch::acpi_power::acpi_mode_enter() {
        Ok(a) => a,
        Err(why) => return sci_skip("acpi-mode", why),
    };
    let (was, en) = crate::arch::acpi_power::gpe0_en_only(&g, Some(gpe));
    SCI_HALF.store(g.len / 2, Ordering::Relaxed);
    SCI_ECBIT.store(((sts - g.blk) << 8) | mask as u16, Ordering::Relaxed);
    SCI_BLK.store(g.blk, Ordering::Release);
    let Some(vector) = crate::arch::interrupts::vectors::alloc("acpi-sci", sci_isr) else {
        sci_off(&g);
        return sci_skip("no-vector", "");
    };
    let (gsi, pol, trig, via) = if g.sci <= 0xFF {
        ioapic::iso_gsi(g.sci as u8, Polarity::ActiveLow, Trigger::Level)
    } else {
        (g.sci as u32, Polarity::ActiveLow, Trigger::Level, "identity")
    };
    if let Err(e) = ioapic::route_gsi(gsi, vector, pol, trig, crate::arch::apic::apic_id()) {
        sci_off(&g);
        return sci_skip("route", e.as_str());
    }
    SCI_GSI.store(gsi, Ordering::Relaxed);
    SCI_ON.store(true, Ordering::Release);
    if let Err(e) = ioapic::set_mask(gsi, false) {
        sci_off(&g);
        let _ = ioapic::unroute(gsi);
        return sci_skip("unmask", e.as_str());
    }
    serial_println!(
        "[acpi] sci on gsi={} via={} mode={} vector={:#04x} gpe={} gpe0_en={:#x} was={:#x} acpi_mode={}",
        gsi,
        via,
        mode_word(trig, pol),
        vector,
        gpe,
        en,
        was,
        if already { "already" } else { "entered" }
    );
    "on"
}

/// No IOAPIC in this kernel: nothing is touched (no ACPI mode, no GPE0_EN write).
#[cfg(not(feature = "ioapic"))]
fn sci_arm() -> &'static str {
    if !sci_platform() {
        return "platform";
    }
    sci_skip("no-ioapic-in-kernel", "")
}

/// The SCI's vector: the lock-free step, then the local APIC's EOI.
#[cfg(feature = "ioapic")]
extern "x86-interrupt" fn sci_isr(_f: x86_64::structures::idt::InterruptStackFrame) {
    sci_irq();
    crate::arch::apic::eoi();
}

/// The handler body: count, `sci_step` on the real block, the EC's flag, the storm guard. Port I/O and atomics only.
#[cfg(feature = "ioapic")]
fn sci_irq() {
    SCI_N.fetch_add(1, Ordering::Relaxed);
    let blk = SCI_BLK.load(Ordering::Acquire);
    if blk == 0 {
        return;
    }
    let half = SCI_HALF.load(Ordering::Relaxed) as u16;
    let mut g = GpePorts(blk);
    let hit = sci_step(&mut g, half, SCI_ECBIT.load(Ordering::Relaxed));
    if hit.ec {
        SCI_EC.fetch_add(1, Ordering::Relaxed);
        SCI_PENDING.store(true, Ordering::Release);
    }
    if hit.other {
        SCI_OTHER.fetch_add(1, Ordering::Relaxed);
    }
    if !hit.any {
        SCI_NONGPE.fetch_add(1, Ordering::Relaxed);
    }
    if !hit.ec && !SCI_STORM.load(Ordering::Relaxed) && storm_hit(&SCI_WIN, &SCI_WN, crate::arch::ms()) {
        mask_all(&mut g, half);
        SCI_STORM.store(true, Ordering::Release);
    }
}

/// The storm's IOAPIC mask, task context (the `CENSUS` lock is never taken in the handler).
fn sci_mask_route(gsi: u32) {
    #[cfg(feature = "ioapic")]
    {
        let _ = crate::arch::ioapic::set_mask(gsi, true);
    }
    #[cfg(not(feature = "ioapic"))]
    {
        let _ = gsi;
    }
}

/// The service pass's SCI step (`qevent_service`, before its throttle): after a storm, the IOAPIC entry masked and
/// `[acpi] sci storm masked n=<n> other=<k> src=<gpe|non-gpe>` once; otherwise `[acpi] sci n=<n> ec=<e> other=<k>` at
/// the first SCI and then at most once per `Q_POLL_MS` when the count moved.
fn sci_service(now: u64) {
    if !SCI_ON.load(Ordering::Acquire) {
        return;
    }
    let n = SCI_N.load(Ordering::Relaxed);
    let other = SCI_OTHER.load(Ordering::Relaxed);
    if SCI_STORM.load(Ordering::Acquire) {
        if !SCI_STORM_SAID.swap(true, Ordering::Relaxed) {
            sci_mask_route(SCI_GSI.load(Ordering::Relaxed));
            let src = if SCI_NONGPE.load(Ordering::Relaxed) > 0 { "non-gpe" } else { "gpe" };
            serial_println!("[acpi] sci storm masked n={} other={} src={}", n, other, src);
        }
        return;
    }
    let said = SCI_SAID_N.load(Ordering::Relaxed);
    if n == said {
        return;
    }
    let last = SCI_SAID_MS.load(Ordering::Relaxed);
    if said != 0 && now.wrapping_sub(last) < Q_POLL_MS {
        return;
    }
    SCI_SAID_N.store(n, Ordering::Relaxed);
    SCI_SAID_MS.store(now, Ordering::Relaxed);
    serial_println!("[acpi] sci n={} ec={} other={}", n, SCI_EC.load(Ordering::Relaxed), other);
}

/// An EC SCI is waiting for its drain.
fn sci_due() -> bool {
    SCI_PENDING.load(Ordering::Acquire)
}

/// The service pass takes the pending flag before its drain (an SCI during the drain sets it again).
fn sci_take() {
    SCI_TAKEN.store(SCI_PENDING.swap(false, Ordering::AcqRel), Ordering::Relaxed);
}

/// No conversation this pass: a taken flag goes back, so the next pass drains (the EC's enable stays off meanwhile).
fn sci_busy(busy: bool) {
    if busy && SCI_TAKEN.load(Ordering::Relaxed) {
        SCI_PENDING.store(true, Ordering::Release);
    }
}

/// After a drain: the EC's GPE0_EN bit back on, unless the SCI is unarmed or the storm guard fired.
fn sci_rearm() {
    if !SCI_ON.load(Ordering::Acquire) || SCI_STORM.load(Ordering::Acquire) {
        return;
    }
    let blk = SCI_BLK.load(Ordering::Acquire);
    if blk == 0 {
        return;
    }
    sci_rearm_on(&mut GpePorts(blk), SCI_HALF.load(Ordering::Relaxed) as u16, SCI_ECBIT.load(Ordering::Relaxed));
}

/// The SCI is armed (ACPI mode is then known on, whatever `gpe0()`'s Once read before the arm).
fn sci_armed() -> bool {
    SCI_ON.load(Ordering::Acquire)
}

/// `tests ecbattery`'s `sci` case over a fake GPE0 block (half 4, the EC at GPE 0x0b = byte 1 mask 0x08) and the fake
/// EC: the EC bit and another set = both cleared, the EC's enable off, the flag set; the flag drains the fake's queued byte
/// through `qdrain`; the re-arm puts the enable back; an idle step writes nothing; the storm guard fires at the 51st hit
/// in one window and resets in the next; `mask_all` zeroes every enable. The boot's statics are never touched.
fn bkat_sci() -> bool {
    let mut g = FakeGpe { r: [0; 16], half: 4, writes: 0 };
    let ecbit: u16 = (1 << 8) | 0x08;
    g.r[1] = 0x09;
    g.r[2] = 0x40;
    g.r[5] = 0x08;
    let pending = AtomicBool::new(false);
    let hit = sci_step(&mut g, 4, ecbit);
    if hit.ec {
        pending.store(true, Ordering::Relaxed);
    }
    let step = hit == SciHit { ec: true, other: true, any: true } && g.r[..4] == [0u8; 4] && g.r[5] == 0;

    let mut f = Fake::with_q(&[0x52]);
    let mut b = [0u8; Q_PER_PASS];
    let (n, st) = if pending.swap(false, Ordering::Relaxed) { qdrain(&mut f, &mut b) } else { (0, None) };
    let drained = n == 1 && st.is_none() && b[0] == 0x52 && !f.bad_cmd;

    sci_rearm_on(&mut g, 4, ecbit);
    let en = g.r[5];
    let rearm = en == 0x08;
    let w = g.writes;
    let idle = sci_step(&mut g, 4, ecbit) == SciHit { ec: false, other: false, any: false } && g.writes == w && g.r[5] == 0x08;

    let (win, cnt) = (AtomicU64::new(0), AtomicU32::new(0));
    let mut storm_at = 0u32;
    for k in 1..=SCI_STORM_N + 1 {
        if storm_hit(&win, &cnt, 5_000) {
            storm_at = k;
            break;
        }
    }
    let reset = !storm_hit(&win, &cnt, 5_000 + SCI_WIN_MS);
    mask_all(&mut g, 4);
    let masked = g.r[4..8] == [0u8; 4];

    serial_println!("[ec] sci kat ec={} drained={} en={:#04x} storm_at={}", hit.ec as u8, n, en, storm_at);
    step && drained && rearm && idle && storm_at == SCI_STORM_N + 1 && reset && masked
}

// ── ZENBOOKECQNAMES (zenbook-ledger Z51) — the EC's `_Qxx` method NAMES and their Notify targets, read from the DSDT — FILE TAIL ──
// Z30 drains the query BYTES; the `_Qxx` METHODS that give them meaning are AML, and this tree runs none (BP-power-4). But a
// Method's NAME is its byte (`_Q0B` answers query 0x0B), and inside its body a `Notify` (opcode 0x86 + the target NameString +
// the value) and the EC Field names it touches are plain bytes. So `qnames_scan` is a BYTE SCAN, not an interpreter: the ONE
// PNP0C09 Device (EISAID or string `_HID`; none = `no-ec`, two = `ambiguous`), every `Scope` whose last NameSeg is that
// Device's name (the `Scope (\_SB.PCI0.SBRG.EC0) { Method (_Qxx) }` blocks outside it), the EmbeddedControl OperationRegions
// and their Field unit names, then every `_Q<hex><hex>` Method in those spans with its Notify targets and the fields its body
// names. The honest claim is "byte xx's handler notifies <device> with <code> and names fields <...>"; what the handler
// COMPUTES is not read. The AML helpers are DWI2CACPI's (`dwi2c::find_from`/`pkg_len`/`last_seg`/`device_of`, widened to
// pub(crate) in place; AML-HELPERS-ONE owes their lift to one home). Once per boot from `probe`, past `hw::gate(Probe::Ec)`
// (the rMBP and QEMU print nothing): `[ec] qnames n= dev= spans= fields=` + one `[ec] qname 0x<xx> notify= fields=` per
// method, or `[ec] qnames -> REFUSED why=`; the table is kept write-once in `QNAMES` and the drain's `[ec] q=` line gains
// ` -> notify=<TGT>:<v>` (or `-`) for a byte it holds. SSDT methods (ZENBOOK-EC-QSSDT) and the calls inside a body
// (ZENBOOK-EC-QCALLS) are owed. `tests ecqnames`: the live line, then the scan over `QN_KAT` and three refusal mutants.

use crate::drivers::dwi2c as aml;

/// Methods kept (more are counted, ` more=` on the summary line).
const QN_MAX: usize = 64;
/// EC spans: the Device plus at most seven `Scope` blocks naming it.
const QN_SPANS: usize = 8;
/// EmbeddedControl OperationRegions kept.
const QN_REGIONS: usize = 4;
/// EC Field unit names kept for the body match (more are counted).
const QN_FIELDS: usize = 192;

/// One `_Qxx` method: its query byte, up to two Notify (target, value: `None` = not a constant), up to four EC fields its body
/// names; `nn`/`nf` count all (saturating).
#[derive(Clone, Copy)]
struct QName {
    byte: u8,
    notify: [([u8; 4], Option<u8>); 2],
    nn: u8,
    fields: [[u8; 4]; 4],
    nf: u8,
}

const QNAME_NONE: QName = QName { byte: 0, notify: [([0; 4], None); 2], nn: 0, fields: [[0; 4]; 4], nf: 0 };

/// What the scan read: the EC Device's name, the spans scanned, the EC field units seen, the methods (`n` counts all).
#[derive(Clone, Copy)]
struct QNames {
    dev: [u8; 4],
    spans: usize,
    nfields: usize,
    n: usize,
    q: [QName; QN_MAX],
}

/// A refusal, by name.
#[derive(Clone, Copy)]
enum QNo {
    NoDsdt,
    NoEc,
    Ambiguous(usize),
    NoMethods([u8; 4]),
}

/// The boot's table, written once by `qnames_boot` (pci init, BSP, before the drain); read by the drain's `QArrow`.
static QNAMES: spin::Once<QNames> = spin::Once::new();

/// The index just past the NameString at `i` (`\`/`^` prefixes, a dual or multi name, the null name, else one NameSeg).
fn name_end(a: &[u8], mut i: usize) -> Option<usize> {
    while matches!(a.get(i), Some(0x5c) | Some(0x5e)) {
        i += 1;
    }
    let e = match *a.get(i)? {
        0x00 => i + 1,
        0x2e => i + 9,
        0x2f => i + 2 + 4 * (*a.get(i + 1)? as usize),
        _ => i + 4,
    };
    if e <= a.len() { Some(e) } else { None }
}

/// A NameSeg character.
fn seg_char(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'
}

/// The ONE PNP0C09 Device and the `Scope` blocks naming it: (its name, the spans, how many).
fn qn_spans(a: &[u8]) -> Result<([u8; 4], [(usize, usize); QN_SPANS], usize), QNo> {
    let pats: [&[u8]; 2] = [b"\x08_HID\x0c\x41\xd0\x0c\x09", b"\x08_HID\x0dPNP0C09\x00"];
    let mut dev: Option<(usize, usize, [u8; 4])> = None;
    let mut seen = [0usize; 8];
    let mut ndev = 0usize;
    for p in pats {
        let mut i = 0;
        while let Some(h) = aml::find_from(a, p, i, a.len()) {
            i = h + 1;
            let Some(d) = aml::device_of(a, h) else { continue };
            if seen[..ndev.min(8)].contains(&d.0) {
                continue;
            }
            if ndev < 8 {
                seen[ndev] = d.0;
            }
            if dev.is_none() {
                dev = Some(d);
            }
            ndev += 1;
        }
    }
    let Some((s, e, nm)) = dev else { return Err(QNo::NoEc) };
    if ndev > 1 {
        return Err(QNo::Ambiguous(ndev));
    }
    let mut spans = [(0usize, 0usize); QN_SPANS];
    spans[0] = (s, e);
    let mut ns = 1;
    let mut k = 0;
    while ns < QN_SPANS {
        let Some(at) = aml::find_from(a, &[0x10], k, a.len()) else { break };
        k = at + 1;
        if (s..e).contains(&at) {
            continue;
        }
        let Some((len, w)) = aml::pkg_len(a, at + 1) else { continue };
        let end = at + 1 + len;
        if end > a.len() || len <= w || aml::last_seg(a, at + 1 + w) != Some(nm) {
            continue;
        }
        if name_end(a, at + 1 + w).is_none_or(|ne| ne > end) {
            continue;
        }
        spans[ns] = (at, end);
        ns += 1;
    }
    Ok((nm, spans, ns))
}

/// The EmbeddedControl (space 0x03) OperationRegions inside the spans.
fn qn_regions(a: &[u8], spans: &[(usize, usize)]) -> ([[u8; 4]; QN_REGIONS], usize) {
    let mut regs = [[0u8; 4]; QN_REGIONS];
    let mut nr = 0;
    for &(s, e) in spans {
        let mut k = s;
        while let Some(at) = aml::find_from(a, &[0x5b, 0x80], k, e) {
            k = at + 1;
            let Some(ne) = name_end(a, at + 2) else { continue };
            if ne >= e || a[ne] != 0x03 {
                continue;
            }
            let Some(r) = aml::last_seg(a, at + 2) else { continue };
            if nr < QN_REGIONS && !regs[..nr].contains(&r) {
                regs[nr] = r;
                nr += 1;
            }
        }
    }
    (regs, nr)
}

/// The Field unit names of every Field on an EC region inside the spans, into `out`; returns how many were seen. The
/// FieldList walk: 0x00 reserved (+PkgLength), 0x01 access (+2), 0x03 extended access (+3), a NameSeg + PkgLength; a
/// ConnectField (0x02) or anything else ends that list.
fn qn_fields(a: &[u8], spans: &[(usize, usize)], regs: &[[u8; 4]], out: &mut [[u8; 4]; QN_FIELDS]) -> usize {
    let mut n = 0;
    for &(s, e) in spans {
        let mut k = s;
        while let Some(at) = aml::find_from(a, &[0x5b, 0x81], k, e) {
            k = at + 1;
            let Some((len, w)) = aml::pkg_len(a, at + 2) else { continue };
            let fe = at + 2 + len;
            let rn = at + 2 + w;
            if fe > e || aml::last_seg(a, rn).is_none_or(|r| !regs.contains(&r)) {
                continue;
            }
            let Some(ne) = name_end(a, rn) else { continue };
            let mut i = ne + 1;
            while i < fe {
                match a[i] {
                    0x00 => {
                        let Some((_, fw)) = aml::pkg_len(a, i + 1) else { break };
                        i += 1 + fw;
                    }
                    0x01 => i += 3,
                    0x03 => i += 4,
                    c if c.is_ascii_uppercase() || c == b'_' => {
                        let Some(sg) = a.get(i..i + 4) else { break };
                        if !sg.iter().all(|&c| seg_char(c)) {
                            break;
                        }
                        let Some((_, fw)) = aml::pkg_len(a, i + 4) else { break };
                        if n < QN_FIELDS {
                            out[n].copy_from_slice(sg);
                        }
                        n += 1;
                        i += 4 + fw;
                    }
                    _ => break,
                }
            }
            k = fe;
        }
    }
    n
}

/// One hex digit of a `_Qxx` name (upper case, as the ASL compiler emits it).
fn qn_hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// A `Method (_Qxx)` at `m` ending within `e`: (the query byte, its body's start after the flags byte, its end).
fn qn_method(a: &[u8], m: usize, e: usize) -> Option<(u8, usize, usize)> {
    if a.get(m) != Some(&0x14) {
        return None;
    }
    let (len, w) = aml::pkg_len(a, m + 1)?;
    let end = m + 1 + len;
    let nm = m + 1 + w;
    if end > e || nm + 5 > end || a[nm] != b'_' || a[nm + 1] != b'Q' {
        return None;
    }
    Some(((qn_hex(a[nm + 2])? << 4) | qn_hex(a[nm + 3])?, nm + 5, end))
}

/// A method body's Notify targets (with the value when it is a constant) and the EC fields whose names it carries.
fn qn_body(a: &[u8], s: usize, e: usize, fields: &[[u8; 4]], q: &mut QName) {
    let mut i = s;
    while let Some(at) = aml::find_from(a, &[0x86], i, e) {
        i = at + 1;
        let Some(t) = aml::last_seg(a, at + 1) else { continue };
        let Some(ne) = name_end(a, at + 1) else { continue };
        if ne >= e {
            continue;
        }
        let v = match a[ne] {
            0x00 => Some(0),
            0x01 => Some(1),
            0x0a if ne + 1 < e => Some(a[ne + 1]),
            0x0b if ne + 2 < e && a[ne + 2] == 0 => Some(a[ne + 1]),
            _ => None,
        };
        if (q.nn as usize) < q.notify.len() {
            q.notify[q.nn as usize] = (t, v);
        }
        q.nn = q.nn.saturating_add(1);
        i = ne;
    }
    for f in fields {
        if aml::find_from(a, f, s, e).is_some() {
            if (q.nf as usize) < q.fields.len() {
                q.fields[q.nf as usize] = *f;
            }
            q.nf = q.nf.saturating_add(1);
        }
    }
}

/// The scan, bounded to `a` (the DSDT). Pure: the KAT runs it over `QN_KAT`.
fn qnames_scan(a: &[u8]) -> Result<QNames, QNo> {
    let (dev, sp, ns) = qn_spans(a)?;
    let spans = &sp[..ns];
    let (regs, nr) = qn_regions(a, spans);
    let mut fl = [[0u8; 4]; QN_FIELDS];
    let nfields = qn_fields(a, spans, &regs[..nr], &mut fl);
    let mut out = QNames { dev, spans: ns, nfields, n: 0, q: [QNAME_NONE; QN_MAX] };
    for &(s, e) in spans {
        let mut k = s;
        while let Some(m) = aml::find_from(a, &[0x14], k, e) {
            let Some((b, bs, be)) = qn_method(a, m, e) else {
                k = m + 1;
                continue;
            };
            k = be;
            let mut q = QName { byte: b, ..QNAME_NONE };
            qn_body(a, bs, be, &fl[..nfields.min(QN_FIELDS)], &mut q);
            if out.n < QN_MAX {
                out.q[out.n] = q;
            }
            out.n += 1;
        }
    }
    if out.n == 0 {
        return Err(QNo::NoMethods(dev));
    }
    Ok(out)
}

/// The kept method for query byte `b`.
fn qname_of(t: &QNames, b: u8) -> Option<&QName> {
    t.q[..t.n.min(QN_MAX)].iter().find(|q| q.byte == b)
}

/// A NameSeg for the wire.
struct Seg([u8; 4]);
impl core::fmt::Display for Seg {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(&self.0).unwrap_or("????"))
    }
}

/// `LID0:0x80`, or `LID0:?` when the value is not a constant.
struct NotifyAt(([u8; 4], Option<u8>));
impl core::fmt::Display for NotifyAt {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}:", Seg(self.0.0))?;
        match self.0.1 {
            Some(v) => write!(f, "{:#04x}", v),
            None => f.write_str("?"),
        }
    }
}

/// A method's Notify list: `LID0:0x80[,PWRB:0x80][+k]`, or `-`.
struct QNotes<'a>(&'a QName);
impl core::fmt::Display for QNotes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let q = self.0;
        if q.nn == 0 {
            return f.write_str("-");
        }
        let kept = (q.nn as usize).min(q.notify.len());
        for (k, n) in q.notify[..kept].iter().enumerate() {
            if k > 0 {
                f.write_str(",")?;
            }
            write!(f, "{}", NotifyAt(*n))?;
        }
        if q.nn as usize > kept {
            write!(f, "+{}", q.nn as usize - kept)?;
        }
        Ok(())
    }
}

/// A method's EC fields: `LIDS,BATS[+k]`, or `-`.
struct QFields<'a>(&'a QName);
impl core::fmt::Display for QFields<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let q = self.0;
        if q.nf == 0 {
            return f.write_str("-");
        }
        let kept = (q.nf as usize).min(q.fields.len());
        for (k, s) in q.fields[..kept].iter().enumerate() {
            if k > 0 {
                f.write_str(",")?;
            }
            write!(f, "{}", Seg(*s))?;
        }
        if q.nf as usize > kept {
            write!(f, "+{}", q.nf as usize - kept)?;
        }
        Ok(())
    }
}

/// The drain's arrow (`qsay`): ` -> notify=<first target>:<v>` or ` -> notify=-` for a byte `QNAMES` holds; nothing else.
struct QArrow(u8);
impl core::fmt::Display for QArrow {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let Some(q) = QNAMES.get().and_then(|t| qname_of(t, self.0)) else { return Ok(()) };
        if q.nn == 0 {
            return f.write_str(" -> notify=-");
        }
        write!(f, " -> notify={}", NotifyAt(q.notify[0]))
    }
}

/// Print one scan: the summary and one line per kept method, or the refusal.
fn qnames_say(r: &Result<QNames, QNo>) {
    match r {
        Err(QNo::NoDsdt) => serial_println!("[ec] qnames -> REFUSED why=no-dsdt"),
        Err(QNo::NoEc) => serial_println!("[ec] qnames -> REFUSED why=no-ec"),
        Err(QNo::Ambiguous(n)) => serial_println!("[ec] qnames -> REFUSED why=ambiguous n={}", n),
        Err(QNo::NoMethods(d)) => serial_println!("[ec] qnames -> REFUSED why=no-methods dev={}", Seg(*d)),
        Ok(t) => {
            if t.n > QN_MAX {
                serial_println!("[ec] qnames n={} dev={} spans={} fields={} more={}", t.n, Seg(t.dev), t.spans, t.nfields, t.n - QN_MAX);
            } else {
                serial_println!("[ec] qnames n={} dev={} spans={} fields={}", t.n, Seg(t.dev), t.spans, t.nfields);
            }
            for q in &t.q[..t.n.min(QN_MAX)] {
                serial_println!("[ec] qname {:#04x} notify={} fields={}", q.byte, QNotes(q), QFields(q));
            }
        }
    }
}

/// The live DSDT's scan (read-only), or `no-dsdt`.
fn qnames_live() -> Result<QNames, QNo> {
    match crate::arch::acpi_power::dsdt() {
        Some(d) => qnames_scan(d),
        None => Err(QNo::NoDsdt),
    }
}

/// `probe`'s once-per-boot scan (past the EC gate, before the drain); keeps the table for the drain's arrow.
fn qnames_boot() {
    let r = qnames_live();
    qnames_say(&r);
    if let Ok(t) = r {
        QNAMES.call_once(|| t);
    }
}

const QN_KAT_LEN: usize = 110;
/// The Device's bytes at the fragment's head (the `ambiguous` mutant is the Device twice).
const QN_KAT_DEV: usize = 84;
/// A hand-built DSDT body: `Device (EC0) { Name (_HID, EisaId ("PNP0C09")) OperationRegion (ECOR, EmbeddedControl, Zero,
/// 0xFF) Field (ECOR, ByteAcc, NoLock, Preserve) { Offset (0x46), LIDS, 1, , 7, BATS, 8 } Method (_Q0B) { If (LIDS)
/// { Notify (LID0, 0x80) } } Method (_Q52) { Store (BATS, Local0) } }` then `Scope (EC0) { Method (_QA0) { Notify
/// (\_SB.PWRB, 0x80) } }`.
const QN_KAT: [u8; QN_KAT_LEN] = [
    0x5b, 0x82, 0x42, 0x05, 0x45, 0x43, 0x30, 0x5f, 0x08, 0x5f, 0x48, 0x49, 0x44, 0x0c, 0x41, 0xd0, 0x0c, 0x09,
    0x5b, 0x80, 0x45, 0x43, 0x4f, 0x52, 0x03, 0x00, 0x0a, 0xff, 0x5b, 0x81, 0x15, 0x45, 0x43, 0x4f, 0x52, 0x01,
    0x00, 0x40, 0x23, 0x4c, 0x49, 0x44, 0x53, 0x01, 0x00, 0x07, 0x42, 0x41, 0x54, 0x53, 0x08, 0x14, 0x13, 0x5f,
    0x51, 0x30, 0x42, 0x00, 0xa0, 0x0c, 0x4c, 0x49, 0x44, 0x53, 0x86, 0x4c, 0x49, 0x44, 0x30, 0x0a, 0x80, 0x14,
    0x0c, 0x5f, 0x51, 0x35, 0x32, 0x00, 0x70, 0x42, 0x41, 0x54, 0x53, 0x60, 0x10, 0x19, 0x45, 0x43, 0x30, 0x5f,
    0x14, 0x13, 0x5f, 0x51, 0x41, 0x30, 0x00, 0x86, 0x5c, 0x2e, 0x5f, 0x53, 0x42, 0x5f, 0x50, 0x57, 0x52, 0x42,
    0x0a, 0x80,
];

/// `tests ecqnames`: the live DSDT's line (read-only), then the scan over `QN_KAT` and three refusal mutants.
pub fn qnames_selftest() {
    qnames_say(&qnames_live());
    let good = qnames_scan(&QN_KAT);
    let g = good.as_ref().ok();
    let q0b = g.and_then(|t| qname_of(t, 0x0b));
    let q52 = g.and_then(|t| qname_of(t, 0x52));
    let qa0 = g.and_then(|t| qname_of(t, 0xa0));
    let lid = q0b.is_some_and(|q| q.nn == 1 && q.notify[0] == (*b"LID0", Some(0x80)) && q.nf == 1 && q.fields[0] == *b"LIDS");
    let quiet = q52.is_some_and(|q| q.nn == 0 && q.nf == 1 && q.fields[0] == *b"BATS");
    let scope = g.is_some_and(|t| t.spans == 2 && t.n == 3) && qa0.is_some_and(|q| q.nn == 1 && q.notify[0] == (*b"PWRB", Some(0x80)) && q.nf == 0);
    let fields = g.is_some_and(|t| t.nfields == 2 && t.dev == *b"EC0_");
    let arrow = g.is_some_and(|t| qname_of(t, 0x0c).is_none()) && q0b.is_some_and(|q| q.byte == 0x0b);
    let mut m = QN_KAT;
    m[17] = 0x0a; // EISAID ("PNP0C09") -> EISAID ("PNP0C0A")
    let noec = QN_KAT[17] == 0x09 && matches!(qnames_scan(&m), Err(QNo::NoEc));
    let mut m = QN_KAT;
    for k in 0..QN_KAT_LEN - 1 {
        if m[k] == b'_' && m[k + 1] == b'Q' {
            m[k + 1] = b'R'; // `_Qxx` -> `_Rxx`
        }
    }
    let nometh = matches!(qnames_scan(&m), Err(QNo::NoMethods(d)) if d == *b"EC0_");
    let mut two = [0u8; 2 * QN_KAT_DEV];
    two[..QN_KAT_DEV].copy_from_slice(&QN_KAT[..QN_KAT_DEV]);
    two[QN_KAT_DEV..].copy_from_slice(&QN_KAT[..QN_KAT_DEV]);
    let amb = matches!(qnames_scan(&two), Err(QNo::Ambiguous(2)));
    match (g, q0b, qa0) {
        (Some(t), Some(b), Some(c)) => serial_println!("[ec] qnames kat n={} q0b={} fields={} qa0={}", t.n, QNotes(b), QFields(b), QNotes(c)),
        _ => serial_println!("[ec] qnames kat scan={}", if g.is_some() { "partial" } else { "refused" }),
    }
    let refuse = noec as u32 + nometh as u32 + amb as u32;
    let cases = [("lid", lid), ("quiet", quiet), ("scope", scope), ("fields", fields), ("arrow", arrow), ("no-ec", noec), ("no-methods", nometh), ("ambiguous", amb)];
    match cases.iter().find(|c| !c.1) {
        None => serial_println!(":: ECQNAMES: n={} notify=LID0:0x80 fields=ok scope=ok refuse={}/3 -> PASS ::", g.map_or(0, |t| t.n), refuse),
        Some(c) => serial_println!(":: ECQNAMES: case={} -> FAIL ::", c.0),
    }
}
