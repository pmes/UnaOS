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
//! WRITE SURFACE: the RD_EC command byte (0x80) to the command port and the address byte to the data port — the read
//! protocol's arguments. WR_EC (0x81), BE_EC/BD_EC (0x82/0x83) and QR_EC (0x84) are never issued.
//! BOUNDED: every IBF/OBF wait has a deadline (`hw_wait_budget() / 200`, ~10 ms calibrated); the census stops at the
//! first stuck handshake and names the step and the status byte. `_Qxx` events need AML and a GPE handler: owed.
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
        // SAFETY: as above; only RD_EC is ever passed (the write surface in the header).
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
        serial_println!("[ec] -> SKIP reason=no-ecdt");
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
    let mut io = PortIo { cmd: e.cmd, data: e.data, start: 0, budget: (crate::arch::hw_wait_budget() / 200).max(1) };
    let mut ram = [0u8; 256];
    let t0 = crate::arch::ms();
    let (n, stale, stuck) = census(&mut io, &mut ram);
    let ms = crate::arch::ms().saturating_sub(t0);
    let mut base = 0;
    while base < n {
        say_row(base, &ram[base..n.min(base + 32)]);
        base += 32;
    }
    match stuck {
        None => serial_println!("[ec] census bytes={} stale={} ms={} -> ok", n, stale, ms),
        Some((at, s)) => serial_println!(
            "[ec] census bytes={} stale={} ms={} -> stuck at={:#04x} step={} status={:#04x}",
            n,
            stale,
            ms,
            at,
            s.step.name(),
            s.status
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
    budget: u32,
}

impl Fake {
    fn new() -> Fake {
        let mut ram = [0u8; 16];
        for (i, b) in ram.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37) ^ 0x5a;
        }
        Fake { ram, st: 0, out: 0, lat: 3, pend: 0, held: 0, held_v: 0, want_addr: false, bad_cmd: false, ibf_wedge: false, obf_wedge: false, polls: 0, budget: 64 }
    }
    fn take(&mut self) {
        let v = self.held_v;
        match self.held {
            1 => {
                if v == RD_EC {
                    self.want_addr = true;
                } else {
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
        self.st | if self.ibf_wedge { IBF } else { 0 }
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
