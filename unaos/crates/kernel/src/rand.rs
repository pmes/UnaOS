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

//! RAND — the kernel's entropy source (SECLOGIN M5, `multiuser.md` §4 gap 6).
//!
//! Three sources, tried in this order, and the one that answered is SAID on the wire once per boot:
//!
//!  * **`rdrand`** (x86_64): CPUID.01H:ECX[30] says the instruction exists; each 64-bit draw is retried
//!    up to ten times on CF=0, which is the discipline Intel's DRNG Software Implementation Guide
//!    documents (a DRNG that fails ten consecutive retries is treated as failed for the boot). Ivy Bridge
//!    — the 2012 rMBP — is the first generation to carry it.
//!  * **`rndr`** (aarch64): `ID_AA64ISAR0_EL1[63:60]` (RNDR) non-zero says the register exists; a draw
//!    that sets NZCV.Z is retried ten times, then the source is treated as failed. ARMv8.5. Cortex-A72
//!    (the Pi 4, QEMU `virt -cpu cortex-a72`) and Cortex-A78AE (the Orin) do not have it.
//!  * **`jitter`** (both): the documented fallback. Two hundred and fifty-six samples of the free-running
//!    cycle counter (`arch::now_cycles`) around a data-dependent memory walk, each sample's low byte
//!    folded through SHA-256 with a running counter. Its entropy is NOT quantified here and is not
//!    claimed to be: it is a source that differs from draw to draw and boot to boot (the `LOGIN-RAND`
//!    fixture asserts exactly that), never a certified generator. A machine on this path says so.
//!
//! Every consumer gets 32 bytes per call and the source name; the salt in `fs/users.rs` is the first.
//! No knob: an entropy source is not optional.

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Which source answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Rdrand,
    Rndr,
    Jitter,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Rdrand => "rdrand",
            Source::Rndr => "rndr",
            Source::Jitter => "jitter",
        }
    }
}

/// Retries per hardware draw before the source is treated as failed (Intel DRNG guide §5.2.1).
const HW_RETRIES: u32 = 10;
/// Jitter samples per 32-byte output.
const JITTER_SAMPLES: usize = 256;

/// 0 = not yet probed, 1 = hardware present and healthy, 2 = hardware absent or failed.
static HW_STATE: AtomicU8 = AtomicU8::new(0);
/// Draws served, for the wire and for the jitter fold's running counter.
static DRAWS: AtomicU64 = AtomicU64::new(0);
/// Said once: the first draw prints its source and what the CPU said.
static SAID: AtomicU8 = AtomicU8::new(0);

/// What the CPU says about a hardware source, as a wire fragment.
fn probe() -> (bool, &'static str) {
    #[cfg(target_arch = "x86_64")]
    {
        let r = unsafe { core::arch::x86_64::__cpuid(1) };
        let has = (r.ecx >> 30) & 1 == 1;
        return (has, if has { "cpuid.01h.ecx.30=1" } else { "cpuid.01h.ecx.30=0" });
    }
    #[cfg(target_arch = "aarch64")]
    {
        let isar0: u64;
        unsafe { core::arch::asm!("mrs {}, ID_AA64ISAR0_EL1", out(reg) isar0, options(nomem, nostack, preserves_flags)); }
        let has = (isar0 >> 60) & 0xF != 0;
        return (has, if has { "id_aa64isar0_el1.rndr!=0" } else { "id_aa64isar0_el1.rndr=0" });
    }
    #[allow(unreachable_code)]
    (false, "no-hw-probe")
}

/// One 64-bit hardware draw, `None` after `HW_RETRIES` failures.
fn hw_draw() -> Option<u64> {
    for _ in 0..HW_RETRIES {
        #[cfg(target_arch = "x86_64")]
        {
            let v: u64;
            let ok: u8;
            unsafe { core::arch::asm!("rdrand {v}", "setc {ok}", v = out(reg) v, ok = out(reg_byte) ok, options(nomem, nostack)); }
            if ok == 1 {
                return Some(v);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            let v: u64;
            let nzcv: u64;
            // RNDR is S3_3_C2_C4_0; a failed draw sets Z.
            unsafe { core::arch::asm!("mrs {v}, S3_3_C2_C4_0", "mrs {n}, NZCV", v = out(reg) v, n = out(reg) nzcv, options(nomem, nostack)); }
            if (nzcv >> 30) & 1 == 0 {
                return Some(v);
            }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            return None;
        }
    }
    None
}

/// The jitter fallback: 32 bytes from the cycle counter's low bits around a data-dependent walk.
fn jitter_fill(out: &mut [u8; 32]) {
    let mut h = crate::hash::Sha256::new();
    let mut walk = [0u8; 256];
    let mut idx = (crate::arch::now_cycles() & 0xFF) as usize;
    let seed = DRAWS.load(Ordering::Relaxed);
    h.update(&seed.to_le_bytes());
    for i in 0..JITTER_SAMPLES {
        let t0 = crate::arch::now_cycles();
        // a data-dependent step so the walk's latency, not just the counter's period, lands in the sample
        walk[idx] = walk[idx].wrapping_add((t0 & 0xFF) as u8).wrapping_add(i as u8);
        idx = (idx.wrapping_mul(31).wrapping_add(walk[idx] as usize)) & 0xFF;
        let t1 = crate::arch::now_cycles();
        h.update(&(t1.wrapping_sub(t0)).to_le_bytes());
        h.update(&[(t1 & 0xFF) as u8]);
    }
    *out = h.finalize();
}

/// Fill `out` with 32 fresh bytes; returns the source that answered. The first call per boot prints
/// `[rand] source=<s> probe=<what the CPU said> bits=256`.
pub fn fill(out: &mut [u8; 32]) -> Source {
    let state = match HW_STATE.load(Ordering::Acquire) {
        0 => {
            let (has, _) = probe();
            let s = if has && hw_draw().is_some() { 1 } else { 2 };
            HW_STATE.store(s, Ordering::Release);
            s
        }
        s => s,
    };
    let mut src = Source::Jitter;
    if state == 1 {
        let mut ok = true;
        let mut words = [0u64; 4];
        for w in words.iter_mut() {
            match hw_draw() {
                Some(v) => *w = v,
                None => { ok = false; break; }
            }
        }
        if ok {
            for (i, w) in words.iter().enumerate() {
                out[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
            }
            src = if cfg!(target_arch = "x86_64") { Source::Rdrand } else { Source::Rndr };
        } else {
            // the hardware failed ten retries mid-boot: say so once, and fall to jitter for the boot
            HW_STATE.store(2, Ordering::Release);
        }
    }
    if src == Source::Jitter {
        jitter_fill(out);
    }
    DRAWS.fetch_add(1, Ordering::Relaxed);
    if SAID.swap(1, Ordering::AcqRel) == 0 {
        let (_, said) = probe();
        serial_println!("[rand] source={} probe={} bits=256", src.name(), said);
    }
    src
}

/// The source the NEXT draw will use, without drawing (for a fixture's own line).
pub fn source() -> Source {
    match HW_STATE.load(Ordering::Acquire) {
        1 => if cfg!(target_arch = "x86_64") { Source::Rdrand } else { Source::Rndr },
        _ => Source::Jitter,
    }
}

/// Draws served so far.
pub fn draws() -> u64 {
    DRAWS.load(Ordering::Relaxed)
}
