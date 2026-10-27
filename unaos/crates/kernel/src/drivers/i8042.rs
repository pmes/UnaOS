//! CHARTER: Kernel — driver (ZENBOOKI8042, zenbook-ledger Z3: the legacy i8042 PS/2 keyboard controller — a TRANSPORT that builds a boot report and hands it to the one boot-keyboard decoder; BP-input-1)
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
//
// ZENBOOKI8042 (zenbook-ledger Z3) — the i8042 PS/2 keyboard controller (ports 0x60 / 0x64).
//
// WHY: the ASUS Zenbook S 14 (UM5606W) wires its internal keyboard to the embedded controller's
// 8042 interface, the standard shape for a Windows laptop; QEMU's q35 carries one too. Every key
// the kernel had decoded so far came over USB (xHCI, EHCI) or Bluetooth.
//
// WHAT IT IS — A TRANSPORT, NOT A DECODER. Set-1 scancodes (scancode set 2 translated by the
// controller's XLAT bit) are turned into HID usages through ONE table (`video::keymap::set1_usage`,
// keymap's own vocabulary), folded into a synthetic 8-byte boot keyboard report (modifier byte +
// six usage slots), and that report is handed to the ONE boot-report decoder the EHCI keyboard and
// the Bluetooth keyboard already share (`drivers::ehci::decode_boot_keyboard`, passed in by the
// pump as a fn pointer). Press edges, layouts, chords, the host typematic, KeyUp, the lock-key
// state, Print Screen and the K2P stamp are therefore the decoder's, never re-written here; the
// PS/2 keyboard's own repeat makes leave the report unchanged and are not re-decoded.
//
// THE PUMP is the one HID pass (`ehci::service_ehci_hid`), which calls `pump` under its
// `EHCI_HID` lock, so the hid-pump task, usb_pump and `pal::pump_and_poll` are serialised here.
//
// IRQ1 reaches the CPU only through the I/O APIC (the 8259 is masked for good and LINT0 is not
// ExtINT), and the IOAPIC is opt-in (`ioapic` feature). With it: a runtime vector
// (`vectors::alloc("i8042")`), the ISA IRQ1 route programmed MASKED, then unmasked, and the ISR
// fills the scancode ring. Without it: the pump polls the status port and fills the same ring, and
// the init line says `irq=polled reason=no-ioapic-in-kernel`.
//
// Owed (Z3): Fn keys via asus-wmi; scancode set 2 raw; typematic rate (0xF3); lock LEDs (0xED);
// Pause (E1, swallowed and counted); the aux port; HIDBOOT-SHARED (the decoder out of the EHCI
// module); the FADT 8042 flag as a second witness; ZENBOOK-TOUCHPAD (I2C-HID, not this file).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use x86_64::instructions::port::Port;

/// Data port (read: output buffer; write: input buffer / keyboard).
const DATA: u16 = 0x60;
/// Status register (read) / command register (write).
const CMD: u16 = 0x64;

/// Status bit 0: output buffer full (a byte waits at 0x60).
const ST_OBF: u8 = 0x01;
/// Status bit 1: input buffer full (the controller has not taken our last write).
const ST_IBF: u8 = 0x02;
/// Status bit 5: the byte in the output buffer came from the aux (mouse) port.
const ST_AUX: u8 = 0x20;

/// Configuration byte bits.
const CFG_IRQ1: u8 = 0x01;
const CFG_IRQ12: u8 = 0x02;
const CFG_XLAT: u8 = 0x40;

/// Controller commands.
const C_READ_CFG: u8 = 0x20;
const C_WRITE_CFG: u8 = 0x60;
const C_DISABLE_AUX: u8 = 0xA7;
const C_SELF_TEST: u8 = 0xAA;
const C_TEST_PORT1: u8 = 0xAB;
const C_DISABLE_KBD: u8 = 0xAD;
const C_ENABLE_KBD: u8 = 0xAE;
const C_WRITE_KBD_OBUF: u8 = 0xD2;

/// Keyboard commands and answers.
const K_RESET: u8 = 0xFF;
const K_SET_SCANCODE: u8 = 0xF0;
const K_GET_ID: u8 = 0xF2;
const K_ACK: u8 = 0xFA;
const K_BAT_OK: u8 = 0xAA;

/// Bounded waits (ms).
const WAIT_MS: u64 = 50;
const BAT_MS: u64 = 600;

// ── The scancode ring: SPSC, one producer at a time (the ISR when IRQ_LIVE, else the pump's poll) ──
const RING_N: usize = 64;
static RING: [AtomicU8; RING_N] = [const { AtomicU8::new(0) }; RING_N];
static HEAD: AtomicUsize = AtomicUsize::new(0); // producer index
static TAIL: AtomicUsize = AtomicUsize::new(0); // consumer index
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// The controller passed init; the pump does nothing before this.
static READY: AtomicBool = AtomicBool::new(false);
/// IRQ1 is routed and unmasked: the ISR is the ring's producer and the pump does not poll.
static IRQ_LIVE: AtomicBool = AtomicBool::new(false);
/// Init's verdict, for the fixture: 0 = not run, 1 = ready, 2 = skipped, 3 = refused.
static STATE: AtomicU8 = AtomicU8::new(0);
static WHY: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("not-run");
/// Reports handed to the decoder.
static DECODED: AtomicU32 = AtomicU32::new(0);
/// Lock-key LED changes the decoder asked for (0xED is owed: counted, not sent).
static LED_OWED: AtomicU32 = AtomicU32::new(0);
/// E1 (Pause) sequences swallowed.
static PAUSE: AtomicU32 = AtomicU32::new(0);
/// Makes with all six slots taken.
static ROLLOVER: AtomicU32 = AtomicU32::new(0);

/// Serialises controller COMMAND sequences (init, `inject`). The pump never writes commands.
static CTRL: crate::sync::Mutex<()> = crate::sync::Mutex::new(());

/// The set-1 machine and the synthetic boot report, plus the decoder's caller-owned state.
struct Dec {
    e0: bool,
    e1_skip: u8,
    report: [u8; 8],
    kprev: [u8; 6],
    kmods: u8,
    kleds: u8,
}

static DEC: crate::sync::Mutex<Dec> =
    crate::sync::Mutex::new(Dec { e0: false, e1_skip: 0, report: [0; 8], kprev: [0; 6], kmods: 0, kleds: 0 });

/// The one boot-keyboard decoder's shape (`drivers::ehci::decode_boot_keyboard`).
pub type BootDecode = unsafe fn(&[u8], &mut [u8; 6], &mut u8, &mut u8) -> bool;

#[inline]
fn status() -> u8 {
    unsafe { Port::<u8>::new(CMD).read() }
}

#[inline]
fn read_data() -> u8 {
    unsafe { Port::<u8>::new(DATA).read() }
}

fn wait_ibf_clear() -> bool {
    let t0 = crate::arch::ms();
    loop {
        if status() & ST_IBF == 0 {
            return true;
        }
        if crate::arch::ms().wrapping_sub(t0) > WAIT_MS {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn read_byte(budget_ms: u64) -> Option<u8> {
    let t0 = crate::arch::ms();
    loop {
        if status() & ST_OBF != 0 {
            return Some(read_data());
        }
        if crate::arch::ms().wrapping_sub(t0) > budget_ms {
            return None;
        }
        core::hint::spin_loop();
    }
}

fn command(c: u8) -> bool {
    if !wait_ibf_clear() {
        return false;
    }
    unsafe { Port::<u8>::new(CMD).write(c) };
    true
}

fn write_data(b: u8) -> bool {
    if !wait_ibf_clear() {
        return false;
    }
    unsafe { Port::<u8>::new(DATA).write(b) };
    true
}

/// A controller command with a one-byte answer.
fn command_read(c: u8) -> Option<u8> {
    if !command(c) {
        return None;
    }
    read_byte(WAIT_MS)
}

/// Write the configuration byte and read it back.
fn write_cfg(cfg: u8) -> Option<u8> {
    if !command(C_WRITE_CFG) || !write_data(cfg) {
        return None;
    }
    command_read(C_READ_CFG)
}

/// A keyboard command answered by ACK.
fn kbd_cmd(b: u8) -> Option<u8> {
    if !write_data(b) {
        return None;
    }
    read_byte(WAIT_MS)
}

fn flush() {
    for _ in 0..16 {
        if status() & ST_OBF == 0 {
            break;
        }
        let _ = read_data();
    }
}

fn verdict(state: u8, why: &'static str) {
    *WHY.lock() = why;
    STATE.store(state, Ordering::Release);
}

fn refuse(step: &'static str) {
    serial_println!("[i8042] step={} -> REFUSED reason=timeout", step);
    verdict(3, step);
}

/// The platform gate: ZENBOOKSURVEY's `crate::hw::platform()` (Z1), composed at the fold. The rMBP has no 8042 behind
/// its LPC (its keyboard is USB), so the probe is skipped by name there; elsewhere the controller probe below
/// (0xFF = no 8042) is the hardware's own answer.
fn platform_skip() -> Option<&'static str> {
    if matches!(crate::hw::platform(), crate::hw::Platform::MacBookPro10_1) { return Some("platform=mac"); }
    None
}

/// Bring the controller up. Called once, from `ehci::hid_task_start` (post-ACPI, post-PCI, the
/// scheduler live), before the hid-pump task exists. Every step reads back; every failure is named.
pub fn init() {
    if let Some(why) = platform_skip() {
        serial_println!("[i8042] -> SKIP reason={}", why);
        verdict(2, why);
        return;
    }
    let _g = CTRL.lock();
    let st = status();
    if st == 0xFF {
        serial_println!("[i8042] status=0xff -> SKIP reason=no-controller");
        verdict(2, "no-controller");
        return;
    }
    if !command(C_DISABLE_KBD) {
        return refuse("disable-kbd");
    }
    if !command(C_DISABLE_AUX) {
        return refuse("disable-aux");
    }
    flush();
    let Some(cfg0) = command_read(C_READ_CFG) else { return refuse("read-cfg") };
    let quiet = cfg0 & !(CFG_IRQ1 | CFG_IRQ12 | CFG_XLAT);
    let Some(cfg1) = write_cfg(quiet) else { return refuse("write-cfg") };
    let Some(test) = command_read(C_SELF_TEST) else { return refuse("self-test") };
    if test != 0x55 {
        serial_println!("[i8042] self-test={:#04x} -> REFUSED", test);
        verdict(3, "self-test");
        return;
    }
    // Some controllers reset the configuration byte on self-test: write it again.
    let Some(cfg2) = write_cfg(quiet) else { return refuse("rewrite-cfg") };
    let Some(port1) = command_read(C_TEST_PORT1) else { return refuse("port1-test") };
    if port1 != 0x00 {
        serial_println!("[i8042] self-test=0x55 port1={:#04x} -> REFUSED", port1);
        verdict(3, "port1-test");
        return;
    }
    serial_println!(
        "[i8042] status={:#04x} self-test=0x55 port1=0x00 cfg={:#04x}->{:#04x}->{:#04x} -> ok",
        st, cfg0, cfg1, cfg2
    );
    if !command(C_ENABLE_KBD) {
        return refuse("enable-kbd");
    }
    // Keyboard reset: ACK, then the basic assurance test's 0xAA.
    let ack = kbd_cmd(K_RESET);
    let bat = read_byte(BAT_MS);
    if ack != Some(K_ACK) || bat != Some(K_BAT_OK) {
        serial_println!(
            "[i8042] keyboard reset={:02x},{:02x} -> REFUSED",
            ack.unwrap_or(0), bat.unwrap_or(0)
        );
        verdict(3, "kbd-reset");
        return;
    }
    // Scancode set 2 on the keyboard; the controller's XLAT bit hands us set 1.
    let s1 = kbd_cmd(K_SET_SCANCODE);
    let s2 = kbd_cmd(0x02);
    if s1 != Some(K_ACK) || s2 != Some(K_ACK) {
        serial_println!("[i8042] keyboard set=2 ack={:02x},{:02x} -> REFUSED", s1.unwrap_or(0), s2.unwrap_or(0));
        verdict(3, "set-scancode");
        return;
    }
    // ID with XLAT still off, so the second byte is the keyboard's own (0x83), not translated.
    let ida = kbd_cmd(K_GET_ID);
    let id0 = read_byte(WAIT_MS).unwrap_or(0);
    let id1 = read_byte(WAIT_MS).unwrap_or(0);
    if ida != Some(K_ACK) {
        serial_println!("[i8042] keyboard id ack={:02x} -> REFUSED", ida.unwrap_or(0));
        verdict(3, "get-id");
        return;
    }
    serial_println!("[i8042] keyboard reset=fa,aa set=2 id={:02x}{:02x} -> ok", id0, id1);
    flush();
    let routed = route_irq1();
    let want = quiet | CFG_XLAT | if routed { CFG_IRQ1 } else { 0 };
    match write_cfg(want) {
        Some(back) if back & (CFG_XLAT | CFG_IRQ1) == want & (CFG_XLAT | CFG_IRQ1) => {}
        other => {
            serial_println!("[i8042] cfg want={:#04x} back={:#04x} -> REFUSED", want, other.unwrap_or(0));
            verdict(3, "cfg-xlat");
            return;
        }
    }
    READY.store(true, Ordering::Release);
    #[cfg(feature = "ioapic")]
    {
        if routed {
            unmask_irq1();
        }
    }
    serial_println!(
        "[i8042] keyboard id={:02x}{:02x} set=1xlat irq={} -> ready",
        id0, id1, if IRQ_LIVE.load(Ordering::Acquire) { "routed" } else { "polled" }
    );
    verdict(1, "ready");
}

#[cfg(feature = "ioapic")]
static GSI: AtomicU32 = AtomicU32::new(u32::MAX);

/// Allocate a vector and program the ISA IRQ1 route MASKED. `true` = programmed (unmasked later).
#[cfg(feature = "ioapic")]
fn route_irq1() -> bool {
    use crate::arch::ioapic;
    let Some(vector) = crate::arch::interrupts::vectors::alloc("i8042", crate::arch::interrupts::i8042_irq_handler) else {
        serial_println!("[i8042] irq=polled reason=no-vector");
        return false;
    };
    let (gsi, pol, trig, via) = ioapic::isa_gsi(1);
    match ioapic::route_gsi(gsi, vector, pol, trig, crate::arch::apic::apic_id()) {
        Ok(entry) => {
            GSI.store(gsi, Ordering::Release);
            serial_println!(
                "[i8042] irq=1 gsi={} via={} vector={:#04x} polarity={} trigger={} entry={:#x} -> routed",
                gsi, via, vector, pol.as_str(), trig.as_str(), entry
            );
            true
        }
        Err(e) => {
            serial_println!("[i8042] irq=polled reason=route-{} gsi={}", e.as_str(), gsi);
            false
        }
    }
}

#[cfg(not(feature = "ioapic"))]
fn route_irq1() -> bool {
    serial_println!("[i8042] irq=polled reason=no-ioapic-in-kernel");
    false
}

#[cfg(feature = "ioapic")]
fn unmask_irq1() {
    let gsi = GSI.load(Ordering::Acquire);
    match crate::arch::ioapic::set_mask(gsi, false) {
        Ok(_) => IRQ_LIVE.store(true, Ordering::Release),
        Err(e) => serial_println!("[i8042] irq=polled reason=unmask-{} gsi={}", e.as_str(), gsi),
    }
}

/// Push one byte; drop-newest when full.
#[inline]
fn ring_push(b: u8) {
    let h = HEAD.load(Ordering::Relaxed);
    let t = TAIL.load(Ordering::Acquire);
    if h.wrapping_sub(t) >= RING_N {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    RING[h % RING_N].store(b, Ordering::Relaxed);
    HEAD.store(h.wrapping_add(1), Ordering::Release);
}

#[inline]
fn ring_pop() -> Option<u8> {
    let t = TAIL.load(Ordering::Relaxed);
    if HEAD.load(Ordering::Acquire) == t {
        return None;
    }
    let b = RING[t % RING_N].load(Ordering::Relaxed);
    TAIL.store(t.wrapping_add(1), Ordering::Release);
    Some(b)
}

/// Move what the controller holds into the ring (bounded); aux bytes are read and discarded.
fn harvest() {
    for _ in 0..16 {
        let st = status();
        if st & ST_OBF == 0 {
            break;
        }
        let b = read_data();
        if st & ST_AUX == 0 {
            ring_push(b);
        }
    }
}

/// The ISR half (`interrupts::i8042_irq_handler`): lock-free, no printing.
pub fn interrupt_ack() {
    harvest();
}

/// The pump half, called by `ehci::service_ehci_hid` under its `EHCI_HID` lock with the one
/// boot-keyboard decoder. Polls when IRQ1 is not live, drains the ring through the set-1 machine,
/// and hands each CHANGED report to `decode`.
pub fn pump(decode: BootDecode) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    if !IRQ_LIVE.load(Ordering::Acquire) {
        harvest();
    }
    let mut d = DEC.lock();
    while let Some(b) = ring_pop() {
        step(&mut d, b, decode);
    }
}

/// One scancode byte through the set-1 machine.
fn step(d: &mut Dec, b: u8, decode: BootDecode) {
    if d.e1_skip > 0 {
        d.e1_skip -= 1;
        return;
    }
    match b {
        0xE0 => {
            d.e0 = true;
            return;
        }
        0xE1 => {
            // Pause: E1 1D 45 E1 9D C5 — swallowed (owed).
            d.e1_skip = 5;
            d.e0 = false;
            PAUSE.fetch_add(1, Ordering::Relaxed);
            return;
        }
        0xFA | 0xFE | 0x00 | 0xFF => {
            // ACK / resend / overrun answers, never key codes.
            d.e0 = false;
            return;
        }
        _ => {}
    }
    let e0 = core::mem::replace(&mut d.e0, false);
    let brk = b & 0x80 != 0;
    let usage = crate::video::keymap::set1_usage(e0, b & 0x7F);
    if usage == 0 {
        return;
    }
    let before = d.report;
    if (0xE0..=0xE7).contains(&usage) {
        let bit = 1u8 << (usage - 0xE0);
        if brk { d.report[0] &= !bit } else { d.report[0] |= bit }
    } else if brk {
        for s in d.report[2..].iter_mut() {
            if *s == usage {
                *s = 0;
            }
        }
    } else if !d.report[2..].contains(&usage) {
        match d.report[2..].iter().position(|&s| s == 0) {
            Some(i) => d.report[2 + i] = usage,
            None => {
                ROLLOVER.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    if d.report == before {
        return; // a typematic make or a release of a key not held: nothing new for the decoder
    }
    let r = d.report;
    let led = unsafe { decode(&r, &mut d.kprev, &mut d.kmods, &mut d.kleds) };
    DECODED.fetch_add(1, Ordering::Relaxed);
    if led {
        LED_OWED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Place `byte` in the controller's output buffer as if the keyboard had sent it (0xD2). It raises
/// IRQ1 when that is live, and is polled otherwise. Untranslated (XLAT applies to the keyboard's
/// bytes, not to 0xD2's).
pub fn inject(byte: u8) -> bool {
    let _g = CTRL.lock();
    command(C_WRITE_KBD_OBUF) && write_data(byte)
}

/// The current modifier byte of the synthetic report and the decoder hand-off count.
fn snapshot() -> (u8, u32) {
    (DEC.lock().report[0], DECODED.load(Ordering::Relaxed))
}

/// Wait until the pump has decoded `want` reports in total, or the budget runs out.
fn wait_decoded(want: u32, budget_ms: u64) -> bool {
    let t0 = crate::arch::ms();
    while DECODED.load(Ordering::Relaxed) < want {
        if crate::arch::ms().wrapping_sub(t0) > budget_ms {
            return false;
        }
        crate::arch::sched::sleep_ticks(1);
    }
    true
}

/// `tests i8042` (M3): the table KAT, the init verdict, and a loopback through 0xD2 — Right Ctrl
/// down and up (E0 1D, E0 9D): no character, no chord, and not Shift (Shift at login is safe mode).
pub fn selftest() {
    use crate::video::keymap::set1_usage;
    let table_ok = set1_usage(false, 0x1E) == 0x04
        && set1_usage(false, 0x1C) == 0x28
        && set1_usage(false, 0x39) == 0x2C
        && set1_usage(false, 0x2A) == 0xE1
        && set1_usage(true, 0x48) == 0x52
        && set1_usage(true, 0x5B) == 0xE3
        && set1_usage(true, 0x2A) == 0
        && set1_usage(false, 0x7F) == 0;
    // init runs at usb-pump entry; give it the boot's time to get there.
    let t0 = crate::arch::ms();
    while STATE.load(Ordering::Acquire) == 0 && crate::arch::ms().wrapping_sub(t0) < 5000 {
        crate::arch::sched::sleep_ticks(1);
    }
    let irq = if IRQ_LIVE.load(Ordering::Acquire) { "routed" } else { "polled" };
    let tbl = if table_ok { "ok" } else { "bad" };
    match STATE.load(Ordering::Acquire) {
        1 => {}
        2 => {
            serial_println!(":: I8042: init=skip table={} -> SKIP reason={} ::", tbl, *WHY.lock());
            return;
        }
        0 => {
            serial_println!(":: I8042: init=not-run table={} -> FAIL reason=init-not-run ::", tbl);
            return;
        }
        _ => {
            serial_println!(":: I8042: init=refused table={} -> FAIL reason={} ::", tbl, *WHY.lock());
            return;
        }
    }
    let (_, base) = snapshot();
    let down = inject(0xE0) && inject(0x1D);
    let got_down = down && wait_decoded(base + 1, 500);
    let (m_down, _) = snapshot();
    let up = inject(0xE0) && inject(0x9D);
    let got_up = up && wait_decoded(base + 2, 500);
    let (m_up, n) = snapshot();
    let dropped = DROPPED.load(Ordering::Relaxed);
    let pass = table_ok && got_down && got_up && m_down == 0x10 && m_up == 0x00;
    serial_println!(
        ":: I8042: init=ready irq={} table={} inject=e0,1d,e0,9d mods={:02x},{:02x} decoded={} dropped={} -> {} ::",
        irq, tbl, m_down, m_up, n - base, dropped,
        if pass { "PASS" } else if !(down && up) { "FAIL reason=inject" } else { "FAIL" }
    );
}

/// One rollup line for a capture (`[i8042] decoded= dropped= led_owed= pause= rollover=`).
pub fn rollup() {
    serial_println!(
        "[i8042] decoded={} dropped={} led_owed={} pause={} rollover={} irq={}",
        DECODED.load(Ordering::Relaxed),
        DROPPED.load(Ordering::Relaxed),
        LED_OWED.load(Ordering::Relaxed),
        PAUSE.load(Ordering::Relaxed),
        ROLLOVER.load(Ordering::Relaxed),
        if IRQ_LIVE.load(Ordering::Acquire) { "routed" } else { "polled" }
    );
}
