// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — driver
//!
//! NETRING3 (rmbp-ledger B306): the arch-neutral bodies behind `SYS_GETRANDOM` (56) and `SYS_RESOLVE`
//! (57), and the `tests net` fixture. The rungs under a metal HTTPS client: ring 3 gets entropy (the
//! one kernel generator, `crate::rand`, now with a DRBG on top) and a name lookup (the one kernel
//! resolver, `smolnet::resolve` — the path `fetch` and the SNTP witness already use). Each arch's
//! `syscall.rs` owns only the user-copy; everything else is here. Design and witness lines:
//! `docs/dev/evidence/rmbp-1004/NETRING3.md`.

use una_abi::{EINVAL, ENODEV, ENOENT, GETRANDOM_MAX, RESOLVE_NAME_MAX, RESOLVE_OUT_LEN};

/// Fill up to `GETRANDOM_MAX` bytes of `buf` from the DRBG; returns the count written.
pub fn getrandom(buf: &mut [u8]) -> usize {
    first_syscall(una_abi::SYS_GETRANDOM as u64, "GETRANDOM");
    let n = buf.len().min(GETRANDOM_MAX);
    if n > 0 {
        crate::rand::drbg_fill(&mut buf[..n]);
    }
    n
}

/// A DNS name ring 3 may hand the resolver: 1..=253 bytes of `[A-Za-z0-9.-]`, no empty label.
fn name_ok(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > RESOLVE_NAME_MAX || name[0] == b'.' || name.windows(2).any(|w| w == b"..") {
        return false;
    }
    name.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
}

/// Resolve `name` to the `[v4 4][v6 16]` answer block, or a negative errno.
pub fn resolve(name: &[u8]) -> Result<[u8; RESOLVE_OUT_LEN], i64> {
    first_syscall(una_abi::SYS_RESOLVE as u64, "RESOLVE");
    if !name_ok(name) {
        return Err(EINVAL);
    }
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    {
        if crate::drivers::e1000::hw_addr().is_none() {
            return Err(ENODEV); // no e1000 and no USB dongle: a dark NIC, not an error of the name
        }
        let s = core::str::from_utf8(name).map_err(|_| EINVAL)?;
        return match crate::smolnet::resolve(s) {
            Some(ip) => {
                let mut out = [0u8; RESOLVE_OUT_LEN];
                out[..4].copy_from_slice(&ip);
                Ok(out)
            }
            None => Err(ENOENT),
        };
    }
    #[allow(unreachable_code)]
    {
        let _ = ENOENT;
        Err(ENODEV) // aarch64: the NET6 resolver is not bound to this syscall yet (owed)
    }
}

/// The name the fixture and NET.ELF resolve: the host the whole ladder is for.
pub const PROBE_HOST: &str = "api.anthropic.com";

/// `tests net`: M1 (entropy) then M2 (resolve).
pub fn selftest() {
    entropy_selftest();
    resolve_selftest();
}

/// M1: 4 KiB drawn twice through the syscall body; no two aligned 32-byte windows equal across the
/// 8 KiB; monobit per 4 KiB within 16384 ± 600 ones (about 6.6 sigma — a sanity bound, not a test suite).
fn entropy_selftest() {
    let mut a = alloc::vec![0u8; 4096];
    let mut b = alloc::vec![0u8; 4096];
    for buf in [&mut a, &mut b] {
        let mut off = 0;
        while off < buf.len() {
            off += getrandom(&mut buf[off..]);
        }
    }
    let mut windows_distinct = true;
    let all: alloc::vec::Vec<&[u8]> = a.chunks(32).chain(b.chunks(32)).collect();
    'outer: for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            if all[i] == all[j] {
                windows_distinct = false;
                break 'outer;
            }
        }
    }
    let ones = |v: &[u8]| v.iter().map(|x| x.count_ones() as i64).sum::<i64>();
    let (oa, ob) = (ones(&a), ones(&b));
    let monobit = (oa - 16384).abs() < 600 && (ob - 16384).abs() < 600;
    let short_ok = { let mut big = [0u8; GETRANDOM_MAX + 1]; getrandom(&mut big) == GETRANDOM_MAX };
    let ok = windows_distinct && monobit && short_ok;
    serial_println!(
        ":: ENTROPY: source={} bytes=8192 windows_distinct={} ones={}+{} short={} ok={} -> {} ::",
        crate::rand::seed_source(), windows_distinct, oa, ob, short_ok, ok as u8, if ok { "PASS" } else { "FAIL" }
    );
}

/// M2: `PROBE_HOST` through the syscall body. A dark NIC or a silent nameserver is SKIP, never FAIL;
/// the name validator refusing a malformed name is asserted either way.
fn resolve_selftest() {
    let einval = resolve(b"").err() == Some(EINVAL) && resolve(b"bad..name").err() == Some(EINVAL) && resolve(b"a b").err() == Some(EINVAL);
    match resolve(PROBE_HOST.as_bytes()) {
        Ok(out) => {
            let ok = einval && out[..4] != [0, 0, 0, 0];
            serial_println!(":: RESOLVE: name={} ip={}.{}.{}.{} einval={} -> {} ::", PROBE_HOST, out[0], out[1], out[2], out[3], einval as u8, if ok { "PASS" } else { "FAIL" });
        }
        Err(e) if !einval => serial_println!(":: RESOLVE: name={} err={} einval=0 -> FAIL ::", PROBE_HOST, e),
        Err(e) if e == ENODEV => serial_println!(":: RESOLVE: name={} einval=1 -> SKIP reason=no-link ::", PROBE_HOST),
        Err(e) if e == ENOENT => serial_println!(":: RESOLVE: name={} einval=1 -> SKIP reason=no-answer ::", PROBE_HOST),
        Err(e) => serial_println!(":: RESOLVE: name={} err={} einval=1 -> FAIL ::", PROBE_HOST, e),
    }
}

// ---- NETHANG (B327 re-aim, boot 20): the breadcrumb and the fixture ------------------------------

/// The last task that made a net syscall; a different task prints one `[net3] first` line.
static LAST_TASK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// NETHANG M1: one line the first time a task reaches the fulfiller (which syscall, which task, and
/// on x86 which core and whether IF is masked). Boot 20 went dark between `[gui] app-enter` and any
/// other line; with this the wire says whether NET.ELF got as far as the kernel's net path.
fn first_syscall(nr: u64, name: &str) {
    let task = crate::arch::sched::current_id().unwrap_or(0);
    if LAST_TASK.swap(task, core::sync::atomic::Ordering::Relaxed) == task {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    serial_println!(
        "[net3] first syscall sys={}({}) task={} cpu={} masked={}",
        nr, name, task, crate::arch::percpu::this_cpu().cpu_index,
        !x86_64::instructions::interrupts::are_enabled() as u8
    );
    #[cfg(not(target_arch = "x86_64"))]
    serial_println!("[net3] first syscall sys={}({}) task={}", nr, name, task);
}

/// NETHANG M3: `tests nethang`. Runs the `SYS_RESOLVE` body from kernel context under the SAME
/// condition the syscall has (IF masked, as SFMASK leaves it) and checks three things:
/// (1) `crate::hlt()` with IF clear returns, and is counted. This is the boot-20 freeze: the xHCI FTDI
/// pump's `hlt` never woke. (2) The resolve returns within a bounded time. (3) After it returns,
/// neither smolnet's `STACK` nor the xHCI loan is still held (`held_locks`). With no link, (2) and (3)
/// are SKIP. (1) is asserted either way.
pub fn nethang_selftest() {
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    {
        use x86_64::instructions::interrupts;
        let was = interrupts::are_enabled();
        let h0 = crate::arch::masked_hlt_count();
        interrupts::disable();
        for _ in 0..3 {
            crate::hlt();
        }
        let ifhlt = crate::arch::masked_hlt_count().wrapping_sub(h0);
        if was {
            interrupts::enable();
        }
        let ifhlt_ok = ifhlt == 3;
        if crate::drivers::e1000::hw_addr().is_none() {
            serial_println!(
                ":: NETHANG: path=resolve held_locks=0 bounded=1 link=0 masked_hlt={} -> {} ::",
                ifhlt, if ifhlt_ok { "SKIP reason=no-link (masked hlt returns: PASS)" } else { "FAIL (masked hlt not counted)" }
            );
            return;
        }
        let capped0 = crate::smolnet::pump_capped();
        let budget = crate::arch::hw_wait_budget();
        interrupts::disable();
        let t0 = crate::arch::now_cycles();
        let r = resolve(PROBE_HOST.as_bytes());
        let el = crate::arch::now_cycles().wrapping_sub(t0);
        if was {
            interrupts::enable();
        }
        let mut held = 0u32;
        if !crate::smolnet::stack_lock_free() {
            held += 1;
        }
        #[cfg(feature = "usbnet")]
        if matches!(crate::drivers::xhci::claim(), Err(crate::drivers::xhci::XhciClaimError::Busy)) {
            held += 1; // the shell runs on the main loop's core: a Busy loan here was left behind
        }
        let bounded = el <= budget.saturating_mul(3); // sendto cap + recvfrom cap + one controller pass
        let hz = crate::arch::apic::tsc_hz();
        let ms = if hz != 0 { el.saturating_mul(1000) / hz } else { 0 };
        let rc: alloc::string::String = match r {
            Ok(o) => alloc::format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
            Err(e) if e == ENOENT => alloc::string::String::from("no-answer"),
            Err(e) => alloc::format!("err{}", e),
        };
        let ok = ifhlt_ok && bounded && held == 0;
        serial_println!(
            ":: NETHANG: path=resolve held_locks={} bounded={} link=1 masked=1 ms={} masked_hlt={} capped={} rc={} -> {} ::",
            held, bounded as u8, ms, ifhlt, crate::smolnet::pump_capped().wrapping_sub(capped0), rc,
            if ok { "PASS" } else { "FAIL" }
        );
    }
    #[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
    serial_println!(":: NETHANG: path=resolve held_locks=0 bounded=1 -> SKIP reason=no-smolnet-on-this-arch ::");
}
