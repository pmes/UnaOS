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

//! WEDGE-2 — LAST-WORDS BREADCRUMBS FOR THE TAB→focus-raise CHAIN.
//!
//! ### What this is for
//! Three silicon lockups (P66, P67v2, P68) and one x86 reproduction (s44) share a death shape: the
//! wire's last line is pointer/click traffic followed by `[wc-fv] focus raise asid=…`, and then
//! nothing at all. No panic handler runs, the x86 capture stopped MID-WORD, and WEDGE-1's
//! `DRAIN STALLED` tripwire stayed silent every time — so the drain barrier is exonerated and the
//! dying core stops with IRQs masked somewhere in the arch-neutral focus chain.
//!
//! A buffered or deferred print cannot name that step: whatever the dying core had queued dies with
//! it. So this module writes a **fixed four-byte token straight at the UART data register, one token
//! BEFORE each phase**, so that on a torn wire the LAST token names the phase that died (or, read the
//! other way, the phase whose successor never started).
//!
//! ### The tokens
//! `<F1>`..`<F9>` mark the phases of the chain on the core that owns the focus change; `<f7>`/`<f8>`
//! mark a CONCURRENT composite entering the same two phases on some OTHER core while the focus chain
//! is live. The table lives in `docs/dev/OS/08_VIDEO/engine.md` §WEDGE-2 and is the authority; the
//! call sites carry the same numbers in their comments.
//!
//! ### The write primitive, and its lock analysis
//! [`mark`] calls `crate::arch::serial::wedge2_raw_byte`, which is a **lock-free, bounded** poll of
//! the UART's TX-ready flag followed by one volatile store to its data register. It acquires NOTHING:
//! not `SERIAL_PORT`/`SERIAL1`, not `FBCON`, not `WRITER`, not `TABLE`, not `SPRITE`, not the
//! allocator. That is the whole point — every one of those locks is reachable from the focus chain
//! itself, and a breadcrumb that could block on a lock the chain holds would be an instrument that
//! disappears in exactly the runs it exists for. It is also why the tokens do NOT go through
//! `serial_println!`: that path masks interrupts and takes the serial lock.
//!
//! **Accepted cost, stated plainly.** Because it takes no lock, a token CAN land in the middle of
//! another core's `serial_println!` line, splitting it. For a last-words instrument that is the right
//! trade: an interleaved token is still legible on the wire (`<F` + digit + `>` is unmistakable inside
//! any other text, and no other line in the tree emits that shape), whereas a token serialised behind
//! a lock is a token the wedge eats. Knob-gated (`UNAOS_WEDGE2=1`), so no shipped image pays it.
//!
//! ### Arch seam
//! The only arch-specific piece is `wedge2_raw_byte` (aarch64: PL011 FR/DR; x86_64: 16550 LSR/THR at
//! 0x3F8). Everything else — this module and every call site — is arch-neutral, so the x86/gemini tree
//! inherits the instrumentation by porting one function.
//!
//! ### Knob-off state
//! With the `wedge2` feature off, [`mark`] is an empty `#[inline(always)]` function and its argument
//! is a dead constant: the call sites vanish and no `<F` token string exists in the image. The QEMU
//! regression gate runs with the knob OFF and must be unchanged-green.

/// Emit one breadcrumb token, raw and unbuffered, BEFORE the phase it names begins.
///
/// `tok` must be a short fixed string literal (the `<F1>`..`<F9>` / `<f7>`/`<f8>` set). It is written
/// byte-at-a-time through the arch's lock-free UART primitive; see the module docs for why no lock is
/// taken and what that costs.
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn mark(tok: &str) {
    for b in tok.as_bytes() {
        crate::arch::serial::wedge2_raw_byte(*b);
    }
}

/// Knob-off: byte-inert. The literal is dead and the call site compiles to nothing.
#[cfg(not(feature = "wedge2"))]
#[inline(always)]
pub fn mark(_tok: &str) {}

/// Which core (1-based, 0 = none) is currently inside the WEDGE-2 focus chain.
///
/// Set at `<F4>` (entry to `wm::focus_changed`) and cleared at `<F9>`. Its ONLY consumer is the
/// composite pass, which uses it to decide between the owner tokens `<F7>`/`<F8>` and the
/// concurrent-core tokens `<f7>`/`<f8>` — i.e. to distinguish "the focus chain reached the blit
/// region" from "another core walked into the same region while the focus chain was open". A plain
/// relaxed atomic: it gates a diagnostic print and orders nothing.
#[cfg(feature = "wedge2")]
pub static CHAIN_CORE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Claim the chain for this core (called at `<F4>`).
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn chain_enter() {
    CHAIN_CORE.store(
        crate::arch::sched::meter_current_cpu().wrapping_add(1),
        core::sync::atomic::Ordering::Relaxed,
    );
}

#[cfg(not(feature = "wedge2"))]
#[inline(always)]
pub fn chain_enter() {}

/// Release the chain (called at `<F9>`).
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn chain_exit() {
    CHAIN_CORE.store(0, core::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(feature = "wedge2"))]
#[inline(always)]
pub fn chain_exit() {}

/// Composite-pass breadcrumb: `owner` picks `<F7>`/`<F8>`, a concurrent core picks `<f7>`/`<f8>`, and
/// a pass running while NO focus chain is open stays silent (the steady-state present rate would
/// otherwise bury the chain in tokens).
///
/// `owner` / `concurrent` are the two literals for this phase, passed in so the whole token set stays
/// visible at the call site.
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn mark_composite(owner: &str, concurrent: &str) {
    let claim = CHAIN_CORE.load(core::sync::atomic::Ordering::Relaxed);
    if claim == 0 {
        return; // no focus change in flight — an ordinary present, not our business
    }
    if claim == crate::arch::sched::meter_current_cpu().wrapping_add(1) {
        mark(owner);
    } else {
        mark(concurrent);
    }
}

#[cfg(not(feature = "wedge2"))]
#[inline(always)]
pub fn mark_composite(_owner: &str, _concurrent: &str) {}
