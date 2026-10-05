// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! LUMENAPP (rmbp-ledger B323, R82) — `tests lumen`: a RING-3-FREE witness that the one-program Lumen is
//! on the volume and is the program the loader will take. It runs nothing and fulfils nothing (the old
//! LUMENBIN fixture drove the window against a VEIN.BIN chat daemon over bus verbs; both are retired).
//!
//! WHAT IT CHECKS. `/apps/LUMEN.ELF` is staged; the production validator (`arch::x86_64::elf::
//! validate_elf`, the one `bg` runs) accepts it in the ELF-window model; its `.note.unaos.app` names
//! "UnaOS", type 1, flags bit0 = windowed. And it reports what the program will run on, by the SAME rule
//! the program applies (`vein_core::prefs::plan` — the shared core, so the two cannot disagree): the
//! provider from Principia's `vein` namespace, and the key's state from `vein.key_file` stat'ed through
//! the VFS (an inode id = UnaFS; FAT is refused).
//!
//! WITNESS. `:: LUMENAPP: image=/apps/LUMEN.ELF window=<elf|fixed|bad> provider=<claude|echo>
//! key=<unafs|none|fat-refused> -> PASS|SKIP|FAIL ::`, preceded on SKIP/FAIL by `:: LUMENAPP: reason=… ::`.
//!
//! LUMENCRASH (rmbp-ledger B326) adds a SPAWN step after a PASS: the image is RUN as `bg` runs it, and the
//! fixture reads the program's first wire line or its fault — `:: LUMENCRASH: spawned=1 first_line=<ok|fault
//! vec=N rip=+0x..|timeout> -> PASS|FAIL ::` (see `spawn_step` at the file tail). The "runs nothing" above is
//! LUMENAPP's static half only.

use vein_core::prefs::{self as rules, KeyState, Plan};

const IMAGE: &str = "/apps/LUMEN.ELF";

/// The provider and key state the program will see (the shared rule over the kernel's own store).
fn session() -> (&'static str, KeyState) {
    let lit = |k: &str| crate::prefs::get("vein", k).map(|v| v.to_literal());
    let s = |k: &str| crate::prefs::get("vein", k).and_then(|v| v.as_str().map(alloc::string::String::from));
    let key = match s("key_file") {
        None => KeyState::None,
        Some(path) => {
            let mut out = alloc::vec::Vec::new();
            if crate::fs::attrsys::do_stat(path.as_bytes(), crate::fs::vfs::KERNEL_PRINCIPAL, &mut out) != 0 || out.len() < 8 {
                KeyState::None
            } else if u32::from_le_bytes([out[4], out[5], out[6], out[7]]) & una_abi::STAT_HAS_ID != 0 {
                KeyState::UnaFs
            } else {
                KeyState::OnFat
            }
        }
    };
    let prov = lit("provider");
    // VEINTLS (SR36): the program's `TlsSetup::verify` over the kernel's view — a trust store on the volume and a set clock (the provider is linked or the image does not build).
    let verify = if crate::fs::attrsys::do_stat(b"/system/trust/roots.pem", crate::fs::vfs::KERNEL_PRINCIPAL, &mut alloc::vec::Vec::new()) != 0 { rules::Verify::NoTrustStore } else if crate::clock::unix_now().map_or(true, |t| (t as i64) < 1_790_985_600) { rules::Verify::NoClock } else { rules::Verify::Ready };
    let ep_s = s("endpoint");
    let ep = match ep_s.as_deref() {
        None | Some("") => Some(rules::DEFAULT_ENDPOINT),
        Some(u) => rules::parse_endpoint(u),
    };
    // Same rule, same inputs as the program (vein_ring3::TlsSetup::verify; floor = vein_ring3::tls::CLOCK_FLOOR).
    let plan = rules::plan(rules::provider_pref(prov.as_deref().map(str::as_bytes)), ep.as_ref(), key, verify);
    (if matches!(plan, Plan::Claude { .. }) { "claude" } else { "echo" }, key)
}

/// The `.note.unaos.app` flags word, if the image carries one (PT_NOTE, name "UnaOS", type 1).
fn app_note_flags(elf: &[u8]) -> Option<u32> {
    let rd16 = |o: usize| elf.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let rd32 = |o: usize| elf.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let rd64 = |o: usize| elf.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize);
    let (phoff, phent, phnum) = (rd64(0x20)?, rd16(0x36)?, rd16(0x38)?);
    for i in 0..phnum {
        let p = phoff + i * phent;
        if rd32(p)? != 4 {
            continue; // PT_NOTE only
        }
        let (off, sz) = (rd64(p + 8)?, rd64(p + 0x20)?);
        let mut o = off;
        while o + 12 <= off + sz {
            let (nsz, dsz, ty) = (rd32(o)? as usize, rd32(o + 4)? as usize, rd32(o + 8)?);
            let name = elf.get(o + 12..o + 12 + nsz)?;
            let d = o + 12 + ((nsz + 3) & !3);
            if name == b"UnaOS\0" && ty == 1 && dsz >= 4 {
                return rd32(d);
            }
            o = d + ((dsz + 3) & !3);
        }
    }
    None
}

/// LUMENAPP's static checks, then (LUMENCRASH M3) the spawn step on the image they accepted.
#[cfg(target_arch = "x86_64")]
pub fn selftest() {
    match check() {
        Some(img) => spawn_step(&img),
        None => serial_println!(":: LUMENCRASH: spawned=0 first_line=none -> SKIP reason=not-spawned ::"),
    }
    lumenux();
}

/// The LUMENAPP verdict; `Some(image)` only on PASS (the image the spawn step then runs).
#[cfg(target_arch = "x86_64")]
fn check() -> Option<alloc::vec::Vec<u8>> {
    let (provider, key) = session();
    let verdict = |window: &str, v: &str| -> Option<alloc::vec::Vec<u8>> {
        serial_println!(":: LUMENAPP: image={} window={} provider={} key={} -> {} ::", IMAGE, window, provider, key.as_str(), v);
        None
    };
    let why = |r: &str| serial_println!(":: LUMENAPP: reason={} ::", r);
    let Ok(fs) = crate::fs::fat::mount_program_source() else {
        why("no-program-volume");
        return verdict("bad", "SKIP");
    };
    let cap = crate::arch::syscall::user_image_cap();
    let img = match fs.find_app("LUMEN.ELF") {
        Ok(de) if de.size != 0 && de.size as usize <= cap => {
            let mut b = alloc::vec::Vec::new();
            match fs.read_file(&de, &mut b, cap) {
                Ok(_) => b,
                Err(_) => {
                    why("read-failed");
                    return verdict("bad", "FAIL");
                }
            }
        }
        Ok(_) => {
            why("size-out-of-range");
            return verdict("bad", "FAIL");
        }
        Err(_) => {
            why("LUMEN.ELF-not-on-the-volume");
            return verdict("bad", "SKIP");
        }
    };
    let plan = match crate::arch::x86_64::elf::validate_elf(&img, crate::arch::syscall::user_window_size()) {
        Ok(p) => p,
        Err(e) => {
            serial_println!(":: LUMENAPP: reason=loader-refused ({}) ::", e);
            return verdict("bad", "FAIL");
        }
    };
    let window = if plan.model_elf { "elf" } else { "fixed" };
    let note = app_note_flags(&img);
    serial_println!(
        "[lumenapp] bytes={} entry={:#x} segs={} stack={} note_flags={:?}",
        img.len(),
        plan.entry,
        plan.nsegs,
        plan.stack,
        note
    );
    if !plan.model_elf {
        why("not-the-elf-model");
        return verdict(window, "FAIL");
    }
    if note.map_or(true, |f| f & una_abi::APP_FLAG_WINDOWED == 0) {
        why("no-windowed-app-note");
        return verdict(window, "FAIL");
    }
    verdict(window, "PASS");
    Some(img)
}

/// aarch64: no ELF window on that loader yet, so no LUMEN.ELF image is built for it (owed).
#[cfg(not(target_arch = "x86_64"))]
pub fn selftest() {
    let (provider, key) = session();
    let _ = app_note_flags;
    serial_println!(":: LUMENAPP: reason=aarch64-image-owed ::");
    serial_println!(":: LUMENAPP: image={} window=bad provider={} key={} -> SKIP reason=window-bad ::", IMAGE, provider, key.as_str());
    lumenux();
}

// ── LUMENCRASH M3 (rmbp-ledger B326) — the SPAWN step: run the real image as `bg` does and read its first line ──
// Boot 20 killed `lumen` at entry+0x3f (a GOT load through a discarded `.got`, LUMENCRASH M1) and no fixture
// could have seen it: LUMENAPP's checks never run the program. This step spawns the accepted LUMEN.ELF through
// `spawn_user_image_bg` — the call `bg` and the bare-name launch make — and waits up to 2 s for the program's
// first wire line `:: LUMEN: start …` (watched at the ring-3 console seam, `serial_line::line_watch_*`) or a
// ring-3 fault of its pid (`note_ring3_fault`, called by the x86 fault-kill path), then kills the job so the
// fixture leaves no window and no network session behind.
//
// WITNESS. `:: LUMENCRASH: spawned=1 first_line=<ok|fault vec=N rip=+0x..|timeout> -> PASS|FAIL ::` (rip
// relative to the entry the loader reported); `spawned=0 … -> FAIL` with the loader's reason if the spawn is
// refused.

#[cfg(target_arch = "x86_64")]
static FAULT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
static FAULT_VEC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
static FAULT_RIP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Called by `arch::x86_64::interrupts::ring3_fault_kill` (the faulting task is still current): record the
/// last ring-3 fault's pid, vector and rip. Vector and rip are published before the pid (Release), so a
/// reader that sees its pid sees that fault's values.
#[cfg(target_arch = "x86_64")]
pub fn note_ring3_fault(vec: u8, rip: u64) {
    use core::sync::atomic::Ordering;
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let pid = crate::arch::sched::current_task_id(cpu).unwrap_or(0);
    FAULT_VEC.store(vec as u64, Ordering::Relaxed);
    FAULT_RIP.store(rip, Ordering::Relaxed);
    FAULT_PID.store(pid, Ordering::Release);
}

#[cfg(target_arch = "x86_64")]
fn spawn_step(img: &[u8]) {
    use core::sync::atomic::Ordering;
    const FIRST: &str = ":: LUMEN: start";
    const WAIT_MS: u64 = 2_000;
    FAULT_PID.store(0, Ordering::Release);
    crate::serial_line::line_watch_arm(FIRST);
    let (pid, slot, entry) = match crate::arch::syscall::spawn_user_image_bg(img) {
        Ok(t) => t,
        Err(e) => {
            crate::serial_line::line_watch_disarm();
            serial_println!(":: LUMENCRASH: reason=spawn-refused ({}) ::", e);
            serial_println!(":: LUMENCRASH: spawned=0 first_line=none -> FAIL ::");
            return;
        }
    };
    let deadline = crate::arch::ticks() + WAIT_MS;
    let mut fault: Option<(u64, u64)> = None;
    let mut ok = false;
    while crate::arch::ticks() < deadline {
        if crate::serial_line::line_watch_hit() {
            ok = true;
            break;
        }
        if FAULT_PID.load(Ordering::Acquire) == pid {
            fault = Some((FAULT_VEC.load(Ordering::Relaxed), FAULT_RIP.load(Ordering::Relaxed)));
            break;
        }
        crate::arch::sched::yield_now();
    }
    crate::serial_line::line_watch_disarm();
    // A faulted task is already dead; a live one (PASS or timeout) is killed so the fixture leaves nothing.
    let killed = if fault.is_none() { crate::arch::syscall::bg_kill(pid, slot) } else { "faulted" };
    serial_println!("[lumencrash] pid={} slot={} entry={:#x} wait_ms={} kill={}", pid, slot, entry, WAIT_MS, killed);
    match fault {
        Some((vec, rip)) => serial_println!(
            ":: LUMENCRASH: spawned=1 first_line=fault vec={} rip=+{:#x} -> FAIL ::",
            vec,
            rip.wrapping_sub(entry)
        ),
        None if ok => serial_println!(":: LUMENCRASH: spawned=1 first_line=ok -> PASS ::"),
        None => serial_println!(":: LUMENCRASH: spawned=1 first_line=timeout -> FAIL ::"),
    }
}

// ── LUMENUX (rmbp-ledger B348) — the window's shared core, re-run in the kernel ──────────────────────────
// LUMEN.ELF renders replies with `vein_core::md`, keeps its scrollback in a `vein_core::scroll::Ring` and saves
// each conversation in `vein_core::history`'s file format; the clipboard it copies to is `video::clipboard`
// through `SYS_CLIP_SET`/`SYS_CLIP_GET`. This leg runs the SAME code on fixed inputs (no ring 3, no file I/O:
// the history leg proves the codec and the 40-byte path, not a write — the file surface is the program's):
//   md       a heading, inline bold/italic/code/link, a list hang, a fence carried across lines;
//   history  header + three turns (one carrying a marker line) parse back exactly; the path fits SYS_OPEN's cap;
//   clip     a set/get round trip through the clipboard the verbs fulfil over (the prior content restored),
//            and the kernel's CLIP_CAP equals una-abi's;
//   scroll   the rows LUMEN.ELF keeps (`LUMEN_ROWS`), after a ring of 8 drops and rebases as the program's does.
// WITNESS. `:: LUMENUX: md=<ok|bad> history=<ok|bad> clip=<ok|bad> scroll=<rows> -> PASS|FAIL font=<dejavu-sans|font8x8> ::`.
fn lumenux() {
    use vein_core::md::{self, Block, Span, State, Tint};
    let md_ok = {
        let mut st = State::default();
        let mut t = [0u8; 128];
        let mut sp = [Span::EMPTY; 16];
        let mut one = |src: &str, st: &mut State| {
            let l = md::line(st, src.as_bytes(), &mut t, &mut sp);
            (l.block, alloc::string::String::from_utf8_lossy(&t[..l.len]).into_owned(), sp[..l.spans].to_vec(), l.hang)
        };
        let h = one("## Head **x**", &mut st);
        let i = one("a **b** *c* `d` [e](f)", &mut st);
        let li = one("  - item", &mut st);
        let f1 = one("```rust", &mut st);
        let f2 = one("# not a heading", &mut st);
        let f3 = one("```", &mut st);
        h.0 == Block::Heading
            && h.1 == "Head x"
            && i.1 == "a b c d e (f)"
            && i.2.iter().any(|s| s.bold)
            && i.2.iter().any(|s| s.italic)
            && i.2.iter().any(|s| s.tint == Tint::Code)
            && i.2.iter().any(|s| s.tint == Tint::Link)
            && li.0 == Block::Bullet
            && li.3 == 4
            && f1.0 == Block::Fence
            && f2.0 == Block::Code
            && f3.0 == Block::Fence
            && !st.fence
    };
    let history_ok = {
        use vein_core::history::{self, Ev, Who};
        let mut buf = alloc::vec![0u8; 1024];
        let mut o = vein_core::Out::new(&mut buf);
        history::header(1, 7, &mut o);
        history::turn(Who::User, b"hi", &mut o);
        history::turn(Who::Assistant, b"# T\n<!-- lumen:user -->\n\n- x", &mut o);
        history::turn(Who::Note, b"n", &mut o);
        let n = o.done().unwrap_or(0);
        let mut got: alloc::vec::Vec<(Who, alloc::vec::Vec<u8>)> = alloc::vec::Vec::new();
        history::parse(&buf[..n], &mut |e| match e {
            Ev::Begin(w) => got.push((w, alloc::vec::Vec::new())),
            Ev::Line(l) => {
                if let Some(t) = got.last_mut() {
                    if !t.1.is_empty() || l.is_empty() {
                        t.1.push(b'\n');
                    }
                    t.1.extend_from_slice(l);
                }
            }
        });
        let mut pb = [0u8; history::PATH_LEN];
        let pn = history::path(1, &mut pb);
        n > 0
            && pn <= 40
            && got.len() == 3
            && got[0] == (Who::User, b"hi".to_vec())
            && got[1] == (Who::Assistant, b"# T\n<!-- lumen:user -->\n\n- x".to_vec())
            && got[2] == (Who::Note, b"n".to_vec())
    };
    let clip_ok = {
        use crate::video::clipboard as clip;
        let mut old = alloc::vec![0u8; clip::CLIP_CAP];
        let on = clip::get(&mut old);
        const PROBE: &[u8] = b"LUMENUX clip probe\n";
        let set = clip::set(PROBE);
        let mut back = alloc::vec![0u8; clip::CLIP_CAP];
        let bn = clip::get(&mut back);
        if on > 0 {
            clip::set(&old[..on]);
        } else {
            clip::clear("lumenux-restore");
        }
        set && &back[..bn] == PROBE && clip::CLIP_CAP == una_abi::CLIP_CAP
    };
    let scroll_ok = {
        use vein_core::scroll::{Rec, Ring};
        let mut store = [Rec::ZERO; 8];
        let mut r = Ring::new(&mut store);
        for k in 0..12u32 {
            r.push(Rec { src: k * 10, ..Rec::ZERO });
        }
        let kept = r.len() == 8 && r.dropped == 4 && r.get(0).map(|x| x.src) == Some(40);
        r.truncate_from_src(100);
        r.rebase(50);
        kept && r.len() == 5 && r.get(0).map(|x| x.src) == Some(0)
    };
    let ok = |b: bool| if b { "ok" } else { "bad" };
    // KERNELFONT2 (B363): the face LUMEN.ELF draws its chat with — DejaVu Sans read off the volume it reads (found
    // there AND parsed by the same font_core), else the font8x8 grid it falls back to. Informational: the verdict
    // is the shared core's, the face is DATA the builder stages.
    let font = crate::video::text::volume_face("DejaVuSans.ttf").map_or("font8x8", |_| "dejavu-sans");
    serial_println!(
        ":: LUMENUX: md={} history={} clip={} scroll={} -> {} font={} ::",
        ok(md_ok),
        ok(history_ok),
        ok(clip_ok),
        if scroll_ok { vein_core::scroll::LUMEN_ROWS } else { 0 },
        if md_ok && history_ok && clip_ok && scroll_ok { "PASS" } else { "FAIL" },
        font
    );
}
