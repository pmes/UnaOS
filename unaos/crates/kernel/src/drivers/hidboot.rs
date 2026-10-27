//! CHARTER: Kernel — input (HIDBOOTSHARED, rmbp-ledger B982: the ONE boot-keyboard decoder, transport-neutral — EHCI, Bluetooth and the i8042 PS/2 keyboard hand it a boot report and a `Transport` label; no CODEX §2 handler owns HID decode, seam driver; BP-input-1/19)
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
// HIDBOOTSHARED (rmbp-ledger B982) — the boot-keyboard decoder MOVED here from `drivers/ehci/mod.rs`
// (ZENBOOKI8042's owed HIDBOOT-SHARED, zenbook-ledger Z3). The text below is the EHCI module's, verbatim
// apart from the transport label: `decode_boot_keyboard` now takes the `Transport` that carried the
// report, and the decoder's own key lines name it — `EHCI-HID:` for EHCI (byte-identical to before),
// `BT-HID:` for the Bluetooth keyboard (`ehci::bthid`), `PS2-HID:` for the i8042 (`drivers::i8042`).
// The caller still owns the per-keyboard state (previous keys, modifiers, the lock-key LED byte); the
// LED change is sent through `Transport::set_leds` (HIDBOOTLEDS B987; EHCI's arm owes its caller's
// per-endpoint LED slot, HIDBOOTLEDSEHCI B991); the EHCI/BT/PS2 callers run under `ehci::EHCI_HID`, the xHCI caller under the
// xHCI loan (HIDBOOTXHCIX86 B1003): the decoder's globals are atomics (DBLSTROKE, pal's typematic tracker), so two keyboards on two controllers interleave reports on ONE tracker (one pair of hands). `ehci` re-exports
// `decode_boot_keyboard`, so `ehci::decode_boot_keyboard` paths in prose still resolve.
//
// HIDBOOTLEDS (rmbp-ledger B987): the lock LEDs leave the decoder through ONE `Transport::set_leds`
// call; `[keystat] <tag>` and the PRTSCR `on <transport>` word name the transport (EHCI byte-identical).
// HIDBOOTLEDSEHCI (B991): EHCI's arm marks the endpoint's LED slot, drained after the walk. Owed (input.md
// §5 Z3 row): the Fn keys; HIDBOOT-PUMP (the i8042 is pumped only from `ehci::service_ehci_hid`). HIDBOOT-ARM BUILT by HIDBOOTARM (B1000): declared on aarch64, the aarch64 xHCI keyboard calls `decode_noted` (file tail) with `Transport::Xhci`; HIDBOOTXHCIX86 (B1003): the x86 xHCI keyboard calls it too (its inline decode deleted). HIDBOOTLEDSTEST (B995): the slot drain is `drain_led_slots` (file tail), `tests hidboot`. HIDBOOTLEDSXHCI (B1005): xHCI's slots (`XhciController` at xhci/mod.rs's tail) drain through `drain_led_slots` too; a `held` refusal is not latched.

/// The transport that carried a boot keyboard report. Labels the decoder's wire; changes no decode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A USB boot keyboard on the EHCI controller (the rMBP's internal keyboard).
    Ehci,
    /// A Bluetooth HID keyboard (`ehci::bthid`).
    Bluetooth,
    /// The i8042 PS/2 keyboard (`drivers::i8042`), its set-1 scancodes folded into a boot report.
    Ps2,
    /// HIDBOOTARM (B1000): a USB boot keyboard on xHCI (aarch64: the Orin's XUSB, the Pi's VL805; x86 since HIDBOOTXHCIX86 B1003: an external USB keyboard on the rMBP/Zenbook).
    Xhci,
}

impl Transport {
    /// The key-line prefix: `EHCI-HID` keeps every existing EHCI line byte-identical.
    pub const fn label(self) -> &'static str {
        match self {
            Transport::Ehci => "EHCI-HID",
            Transport::Bluetooth => "BT-HID",
            Transport::Ps2 => "PS2-HID",
            Transport::Xhci => "xHCI",
        }
    }

    /// The short token for `via=` / `transport=` fields.
    pub const fn tag(self) -> &'static str {
        match self {
            Transport::Ehci => "ehci",
            Transport::Bluetooth => "bt",
            Transport::Ps2 => "ps2",
            Transport::Xhci => "xhci",
        }
    }

    /// HIDBOOTLEDS (B987) — the ONE route for a lock-LED change, called once from
    /// `decode_boot_keyboard`'s lock-key site. PS/2: the i8042's 0xED sequence (async, acked through
    /// the scancode ring, bounded). Bluetooth: bthid's existing `PENDING_LED` slot, drained as the
    /// DATA|Output report on the next pass. EHCI (HIDBOOTLEDSEHCI B991): SET_REPORT is an EP0 control
    /// transfer that needs `&mut Controller`, which the endpoint walk holds while it calls the decoder,
    /// and this unit label cannot address the endpoint — so the arm answers `true`: the caller's
    /// per-endpoint LED slot (`kbd_leds` + `kbd_led_dirty`) is owed, drained after the walk. Bluetooth and PS/2 queue themselves and answer `false`. xHCI (HIDBOOTARM B1000) answers `true` like EHCI: the slot's `keyboard_leds_dirty`.
    pub fn set_leds(self, leds: u8) -> bool {
        match self {
            Transport::Ehci => true,
            Transport::Bluetooth => { bt_queue_led(leds); false }
            Transport::Ps2 => { ps2_set_leds(leds); false }
            Transport::Xhci => true, // HIDBOOTARM (B1000): the caller owes HIDLEDDEFER's `keyboard_leds_dirty`, drained by `service_hid_leds`
        }
    }
}

/// Boot-keyboard report decode — the same layout, scancode table, and Event delivery as the
/// xHCI keyboard path (xhci mod.rs event dispatch), so a key is a key whichever controller
/// carried it. Table shared via pub(crate) rather than duplicated.
///
/// EHCI-KEYUP — THIS PATH NOW SYNTHESISES RELEASES, WHICH IS THE HALF IT WAS MISSING.
///
/// Boot AJ (`aj-lockout-forensics.md`, Defect 2a) is what a decoder that emits presses only costs:
/// the rMBP's INTERNAL keyboard is on EHCI, so on that machine no ring-3 app had ever received a
/// single `INPUT_EV_KEY_UP`. `user-vug` clears a held bit only on a release, so the first SPACE
/// latched pause on permanently and the arrow/WASD held bits latched with it — the operator's
/// "the vug froze". The xHCI decoder has always synthesised releases; this is the same logic, and
/// it is deliberately a MIRROR rather than a variation.
///
/// WHAT "MIRROR" MEANS HERE, SCOPED — because the unqualified claim is not true and the difference
/// is one a bench reading could trip over. The mirror is over the EDGE MODEL: which keycodes produce
/// a release, when, and in what order. That half is byte-equivalent to xHCI's — same set diff, same
/// `<= 1` guard, same position-independent `contains` test, same "no `KeyUp` for a modifier", same
/// shift-at-release-time. It is NOT a mirror over ASCII MAPPING: xHCI computes
/// `eff_shift = shift ^ (caps & is_letter)` from the device's `keyboard_leds`, and this decoder has
/// no caps state at all and uses bare `shift`. So under Caps Lock a LETTER's ascii differs between
/// the two controllers — on the press as much as on the release. That is PRE-EXISTING (the press
/// loop was already caps-blind before this arc), it is internally consistent here because both loops
/// share `ascii_of` so a press and its release can never disagree about which character they are
/// about, and it is being closed by the `seat/gr21-allkeys` arc stacked on this one. It is named
/// rather than fixed here on purpose: this commit's contract is the edge model, and widening it to
/// the mapping would put two independent changes in one bisect.
///
/// THE DIFF. A USB boot report is a LEVEL, not an edge: bytes 2..8 carry the FULL set of keycodes
/// currently down (6-key rollover), so any keycode present in the previous report and absent from
/// this one was released. `prev_keys` carries that previous set per endpoint and is rewritten here.
/// The press loop reads the SAME diff in the other direction (see DBLSTROKE below): a keycode absent
/// from the previous report and present in this one is a press EDGE, and only an edge pushes `Key`.
///
/// WHY A MISSED RELEASE CANNOT STRAND A KEY HERE. `service_ehci_hid` arms one transfer per service
/// pass and is polled at frame rate, so reports the device sends between passes are simply never
/// fetched — the CLICK-3 note above is about exactly that. It cannot cost a release: USB is
/// host-polled, so a device with a pending state change holds it until the next IN token rather than
/// losing it, and the re-arm happens in the SAME service pass as this decode. All four poll-gap
/// cases are symmetric — press+release both inside a gap loses both edges; press seen then release
/// seen pairs; release+re-press inside a gap collapses to a hold with no fabricated edge. (This is
/// the property the pointer path lacks, because a button's level can go down again before the next
/// poll.) **The one ASYMMETRIC case is not a poll gap at all: it is endpoint DEATH, and it is closed
/// separately by `ehci::flush_held_releases`.**
///
/// SHORT REPORTS ARE REFUSED WHOLE, and the `< 8` is load-bearing rather than defensive dressing.
/// This guard was `< 3` and `cur_keys` zero-fills the slots a short report does not carry, so a full
/// report followed by a SHORT one emitted a `KeyUp` for everything held in the missing slots — a
/// FABRICATED release, the exact class this arc exists to remove, arriving by a different door (a
/// fabricated SPACE release clears `H_PAUSE`, the next full report re-presses it as a fresh edge,
/// and pause toggles on its own). xHCI cannot do this: it reads a fixed 8-byte staging buffer
/// UNCONDITIONALLY (`from_raw_parts(data_buf_ptr, 8)`), so a short transfer leaves the previous
/// bytes in place and reads as STILL HELD — the conservative direction. Refusing the report whole
/// takes that same direction with no staging buffer: no press, no release, and `prev_keys` is left
/// untouched, so nothing is invented and the next conforming report resolves the truth. Zero
/// behavioural cost on any conforming device — the boot protocol is fixed at 8 bytes and this
/// driver arms these endpoints at `mps=8`.
///
/// MODIFIERS GET NO KeyUp, matching xHCI exactly. `report[0]` is a bitmask, not a keycode; neither
/// decoder has ever pushed `Key` for a bare modifier, so pushing `KeyUp` for one would fabricate a
/// release edge for a press ring 3 never saw. The ascii of a release is picked by
/// `xhci::hid_key_release_ascii`, which folds on Shift and Caps only, as on xHCI. The invariant is
/// exact: **a release resolves to the same ascii its press did** — Shift+`1` that pressed `!`
/// releases `!`. The earlier gloss "consumers match case-insensitively" was true for letters and
/// FALSE for shifted symbols (`!` vs `1`), which is the shifted-symbol strand GR21 closed.
///
/// ── KEYREPEAT-X86 (Boot AL): THIS DECODER NOW FEEDS THE HOST TYPEMATIC TRACKER ────────────────
///
/// The paragraph that stood here said "no typematic interaction exists on this path", called it a
/// fact about the build rather than a judgement call, and closed with *"repeat on this arch is the
/// DEVICE's, carried by the re-reported level."* The build fact was true; the closing sentence was
/// a hypothesis, and Boot AL refuted it — Peter, at the bench: *"so far so good with keys except no
/// key repeat."* Everything else GR21 landed here passes on metal. Repeat does not, because the
/// rMBP's internal keyboard does NOT re-report a key that is held still: no SET_IDLE is sent on
/// this path (see KBDWIT), so the device runs its default report-on-change behaviour and the
/// "re-reported level" the KeyDown loop below relies on for repeat never arrives. One press report,
/// one `Event::Key`, and the shell's line editor advances exactly one character — which is the same
/// symptom, from the same cause, that UVUG-5 was written for on aarch64.
///
/// So the tracker's cfg was widened to cover `x86_64 + ehcihid` (see `pal.rs` §KEYREPEAT-X86) and
/// this decoder feeds it at the REPORT level, exactly as `drivers::xhci` does: the newest ascii
/// pressed THIS report plus the FULL currently-held ascii set, computed through the same
/// `ascii_of` fold the `Event::Key` pushes use, so the armed key and the pushed character are the
/// same byte and a release can never disagree with its press about identity.
///
/// WHY THE FEED IS HERE AND NOT AT THE EVENT QUEUE. That is the whole of UVUG-6: `EventQueue::push`
/// silently DROPS on a full 64-slot ring, so a release learned from the drained event stream can be
/// lost and the tracker would then repeat a key nobody is holding, forever (the P51 wedge). At the
/// report level a release is learned from the armed key being ABSENT from the held set — a fact the
/// queue cannot drop. The three disarm layers and the half-full backpressure guard come with the
/// shared code, already metal-cured on the Pi.
///
/// THE KeyDown/KeyUp EDGES BELOW ARE UNTOUCHED, deliberately: GR21's release synthesis is
/// metal-proven as of Boot AL (SPACE pauses and unpauses, WASD releases, `!` releases `!`,
/// modifiers get no `KeyUp`) and nothing in this arc may perturb it. The tracker is a pure OBSERVER
/// here — it pushes no event from this function; `main`'s x86 pump calls `pal::typematic_tick`.
///
/// ── DBLSTROKE (Boot AN): THE PRESS LOOP WAS LEVEL-TRIGGERED, AND ROLLOVER DOUBLED EVERY KEY ──────
///
/// Peter, at the bench on Boot AN: *"key repeat good but typing fast causes double stroke."* Held-key
/// repeat and normal-speed typing are both correct; typing FAST doubles characters. The capture names
/// the mechanism outright, with no new instrument needed — `EHCI-HID: KEY:` is pushed once per
/// `Event::Key`, and the word "help" typed quickly reads:
///
/// ```text
/// [1228135ms] KEY: 'h'                      report [h]     — press edge
/// [1228275ms] KEY: 'h'   KEY: 'e'           report [h,e]   — 'h' RESTATED, 'e' pressed
/// [1228413ms] KEY: 'e'   KEYUP: 'h'         report [e]     — 'e' RESTATED, 'h' released
/// [1228427ms] KEY: 'l'                      report [l]     — press edge
/// ```
///
/// Two `Event::Key` pushes for one physical press of `h`, and two for `e`. The cause is that the press
/// loop was a LEVEL loop: it pushed `Key(ascii)` for every keycode in every report, and a USB boot
/// report re-states the FULL held set. Fast typing is defined by OVERLAP — the next key goes down
/// before the last one comes up — so every overlapped pair produces a report that re-states the key
/// already down, and that re-statement was delivered as a second press. Type slowly enough that each
/// key is fully released first and every report carries exactly one key, and nothing ever repeats:
/// precisely the reported symptom, precisely bounded.
///
/// It is the PRODUCER, not the console or the line editor: the two pushes are two distinct lines from
/// this function, at two different report timestamps. It is not the typematic tracker either — the
/// doubles are 140 ms apart with no 400 ms delay elapsed, `[keystat] typematic hold end` shows
/// `re-arms=0` for the whole boot, and the tracker pushes nothing from here in any case.
///
/// THE FIX is to push on the press EDGE, which is the contract every consumer in the tree was already
/// written against — `vug.rs` GAME-MODE says so in as many words ("the HID path delivers a Key on the
/// PRESS edge and a KeyUp on the RELEASE edge"), and this decoder's own release loop has always been
/// an edge. The two loops now read the same `prev_keys`/`cur_keys` diff in opposite directions, so a
/// press and its release are the same fact seen twice and cannot disagree about how many there were.
///
/// WHAT PAYS FOR THE REPEAT THE LEVEL LOOP USED TO PROVIDE: the host typematic tracker, which this
/// same function already feeds and which is the ONLY source of repeat that ever reached this machine.
/// The old note here argued the level loop delivered repeat "on a device that does re-report" and so
/// should stay. On this hardware there is no such device — no `SET_IDLE` is sent on this path (KBDWIT)
/// and the internal keyboard runs report-on-change — which is why KEYREPEAT-X86 had to add the tracker
/// at all. Level-driven repeat was therefore never repeat here; it was only ever the doubling, paced
/// by the operator's other fingers rather than by any repeat rate. And a hypothetical idle-re-reporting
/// keyboard is better served by the tracker's 400 ms/40 ms than by an unthrottled poll-rate spew.
///
/// WITNESS (`[keystat] ehci press`). Two bounded counters that decide the NEXT boot either way:
/// `restated=` counts the pushes this edge gate suppressed (non-zero proves the operator typed with
/// overlap and that the old code WOULD have doubled), and `dbl=` is an independent doubled-push
/// detector that watches what this function actually pushes — the same ascii twice inside
/// `DOUBLE_WINDOW_MS`. If Peter still sees doubles with `dbl=0`, the producer is clean and the fault
/// is downstream (console echo or line editor); if `dbl>0` the producer is still doubling and this
/// diagnosis was wrong. Either way the boot is decisive, which the previous boot's instruments were
/// not.
///
/// ── ALLKEYS P1 (GR21): CAPS LOCK, THE HALF THIS DECODER WAS STILL MISSING ──────────────────────
///
/// Peter, at the bench: *"do we have working shift, caps lock etc — we want all the keys working."*
/// Shift worked here; Caps Lock did not, and on THIS machine that is the whole story, because the
/// rMBP's internal keyboard is an EHCI device — so the decoder that had never heard of caps lock
/// was the only one the operator could reach. The xHCI decoder has read a caps-lock bit since
/// HID-LED; this path did not, so the key was inert and the LED never lit.
///
/// BOTH HALVES OR NEITHER, and that is not a stylistic preference. Case logic without an LED gives
/// an operator a keyboard that types capitals with an unlit key — indistinguishable from a stuck
/// Shift. An LED without case logic gives a lit key that types lowercase. Either half alone reads
/// as a BROKEN keyboard, so this decoder toggles the state and the caller lights the key, both
/// driven from the single `kbd_leds` byte that the case fold and SET_REPORT each read.
///
/// The decoder does not itself send SET_REPORT — it cannot. It is called from inside
/// `Controller::service`'s `for e in self.int_eps.iter_mut()`, which holds an exclusive borrow of
/// `int_eps`, while a control transfer needs `&mut self`. So it RETURNS whether the transport owes
/// the caller's LED slot (HIDBOOTLEDSEHCI B991: EHCI marks `kbd_led_dirty`) and the caller drains it
/// after the loop drops that borrow — the shape the `mtraw` capture-window restore uses there.
///
/// MODIFIER POLICY is `xhci::hid_key_ascii`'s, not this function's, and deliberately so: Ctrl/Alt/
/// GUI folding was absent from BOTH decoders (Cmd-Q typed a bare `q` into the shell line), and
/// fixing it in one place while the other kept a private copy of the fold is exactly how the
/// caps-lock divergence arose. The whole decision now lives in one function both controllers call.
///
/// PER-RELEASE ASCII, and why `prev_mods` exists. A release edge fires exactly once and is never
/// re-sent, so it MUST resolve to the same ascii FAMILY its press did or it strands the key in every
/// held-state consumer (Boot AJ). The live release path has the current `report[0]` in hand; the
/// dead-endpoint flush (`ehci::flush_held_releases`) does not — the device is gone — so the modifier
/// byte of the last ACCEPTED report is carried in `prev_mods` for it to fold against. Both paths go
/// through `xhci::hid_key_release_ascii`, which depends only on Shift and Caps: Shift+`1` that
/// pressed `!` releases `!`, not `1`.
///
/// Returns `true` if this report toggled a lock key AND `Transport::set_leds` says the caller owes its LED slot (EHCI).
// ── DBLSTROKE witness state ──────────────────────────────────────────────────────────────────────
//
// Boot totals only; no per-endpoint state, because the operator has one pair of hands and the question
// ("does this decoder push the same character twice for one press?") is about the decoder, not about
// which endpoint carried it. All of it is dead weight on a boot where nobody types: every counter
// stays 0 and not one line is emitted.
//
// HIDBOOTARM (B1000): `drivers/mod.rs` declares `pub mod hidboot` on x86_64 AND aarch64 (the aarch64 xHCI
// keyboard is a caller, under the xHCI loan; x86 xHCI too since HIDBOOTXHCIX86 B1003); HIDBOOTSHARED (B982) had it x86-only. The one dependency
// that is not on every image, `pal::typematic_note_keys`, carries pal's own gate (`x86_64 + ehcihid`,
// `aarch64 + baremetal|tegra`) at its call; the i8042 and Bluetooth arms are x86-gated at their helpers.

/// DBLSTROKE — `Event::Key` pushes this decoder made, i.e. genuine press edges.
static PRESS_EDGES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — keycodes seen down in a report that the PREVIOUS report already carried. Under the old
/// level loop each of these was a second `Event::Key` for a key nobody re-pressed; it is now suppressed
/// and counted. Non-zero is the positive evidence that the operator typed with OVERLAP during the run,
/// which is what makes a `dbl=0` result meaningful rather than merely untested.
static RESTATED_PRESSES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — reports carrying two or more character keys down at once: rollover, i.e. "typing fast"
/// measured rather than inferred from the operator's description.
static ROLLOVER_REPORTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — the independent detector: the same ascii pushed twice by this decoder inside
/// [`DOUBLE_WINDOW_MS`]. Survives the fix on purpose — it watches the OUTPUT, so it stays valid however
/// the input side is rewritten, and it is what distinguishes a producer double from a consumer echo.
static DOUBLE_PUSHES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — ascii of the last `Event::Key` this decoder pushed, +1 (0 = none yet).
static LAST_PUSH_ASCII: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — `arch::ms()` of that push.
static LAST_PUSH_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DBLSTROKE — inside this window, the same character twice is a machine artefact, not typing. Human
/// double-letters ("ll" in "help", "ee" in "seen") are two presses with a release between them and are
/// paced by fingers: the Boot AN capture puts a deliberate re-tap of the same key ~450 ms apart, and
/// even the machine doubles it exposed were 24..140 ms. 50 ms sits below anything a hand does and above
/// every duplicate the level loop produced.
const DOUBLE_WINDOW_MS: u64 = 50;
/// DBLSTROKE — how many suppressed restatements name themselves individually before the rollup takes
/// over. Enough to see the shape at the bench; bounded so a stuck endpoint cannot make serial the new
/// backpressure.
const RESTATE_LOG_MAX: u32 = 3;
/// DBLSTROKE — after the individual lines, one rollup per this many suppressions. At a fast typist's
/// overlap rate this is a line every few seconds of continuous typing.
const RESTATE_ROLLUP_EVERY: u32 = 32;
/// DBLSTROKE — individually-named doubled pushes before that detector also falls back to counting.
const DOUBLE_LOG_MAX: u32 = 8;

/// DBLSTROKE — record one genuine press edge and run the doubled-push detector over it.
///
/// Called ONLY from the `push_event(Event::Key(..))` site, so what it measures is what ring 3 actually
/// received. If this ever fires after the edge gate, the edge gate is not the whole story and the next
/// boot says so on its own line rather than leaving the bench to re-describe the symptom.
fn note_press_edge(t: Transport, ascii: u8) {
    use core::sync::atomic::Ordering;
    let now = crate::arch::ms();
    PRESS_EDGES.fetch_add(1, Ordering::Relaxed);
    let prev = LAST_PUSH_ASCII.swap(ascii as u32 + 1, Ordering::Relaxed);
    let prev_ms = LAST_PUSH_MS.swap(now, Ordering::Relaxed);
    if prev == ascii as u32 + 1 && now >= prev_ms && now.wrapping_sub(prev_ms) <= DOUBLE_WINDOW_MS {
        let n = DOUBLE_PUSHES.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= DOUBLE_LOG_MAX {
            serial_println!(
                "[keystat] {} double-push — ascii={:#04x} pushed twice {}ms apart (<= {}ms); the PRODUCER is doubling, not the console (boot dbl={})",
                t.tag(),
                ascii,
                now.wrapping_sub(prev_ms),
                DOUBLE_WINDOW_MS,
                n
            );
        }
    }
}

/// DBLSTROKE — fold one report's press accounting and emit the bounded rollup.
///
/// `down` counts character keys down in the report (rollover measure); `restated` counts those the
/// previous report already carried (suppressed doubles).
fn note_press_report(t: Transport, down: u32, restated: u32) {
    use core::sync::atomic::Ordering;
    if down >= 2 {
        ROLLOVER_REPORTS.fetch_add(1, Ordering::Relaxed);
    }
    if restated == 0 {
        return;
    }
    let total = RESTATED_PRESSES.fetch_add(restated, Ordering::Relaxed) + restated;
    let first_few = total <= RESTATE_LOG_MAX;
    if first_few || total % RESTATE_ROLLUP_EVERY < restated {
        serial_println!(
            "[keystat] {} press — edges={} restated={} (+{} this report) rollover_reports={} dbl={} window={}ms",
            t.tag(),
            PRESS_EDGES.load(Ordering::Relaxed),
            total,
            restated,
            ROLLOVER_REPORTS.load(Ordering::Relaxed),
            DOUBLE_PUSHES.load(Ordering::Relaxed),
            DOUBLE_WINDOW_MS
        );
    }
}

pub unsafe fn decode_boot_keyboard(
    t: Transport,
    report: &[u8],
    prev_keys: &mut [u8; 6],
    prev_mods: &mut u8,
    leds: &mut u8,
) -> bool {
    if report.len() < 8 {
        return false; // short/partial boot report — refused whole (keyup F1); see SHORT REPORTS above
    }
    let modifiers = report[0]; crate::video::keymap::note_mods(modifiers); // BRIGHTFLOOR M2 — the latest modifier byte (Shift = safe mode at Log In), as the xHCI decoder already records it. SAME-LINE fold.
    // ALLKEYS P1: the live caps-lock bit feeds the case fold, so the lit key and the typed case
    // are the same fact read twice rather than two states that can drift apart.
    let caps = *leds & 0x02 != 0;
    // The ascii for one keycode under the modifier state in force. One place, so a press and its
    // release can never disagree about which character they are about.
    let ascii_of = |keycode: u8| -> u8 { crate::drivers::xhci::hid_key_ascii(keycode, modifiers, caps) };
    // ALLKEYS: and the same for a RELEASE, which may never resolve to "no event" for a key that has
    // a character identity — see `hid_key_release_ascii`. A release edge fires once and is never
    // re-sent, so suppressing one strands the key in every consumer that tracks held state, which is
    // the precise failure Boot AJ cost this path.
    let release_ascii_of =
        |keycode: u8| -> u8 { crate::drivers::xhci::hid_key_release_ascii(keycode, modifiers, caps) };
    // This report's six keycode slots. The `< 8` guard above is what makes this a COMPLETE picture of
    // what is down — every slot comes from the wire, none is invented — which is the precondition the
    // release diff below needs to be sound. `report[2 + i]` cannot panic for the same reason.
    let mut cur_keys = [0u8; 6];
    for (i, slot) in cur_keys.iter_mut().enumerate() {
        *slot = report[2 + i];
    }

    // BT-RETRY — THE PAIRING CHORD. Ctrl+Alt+B, edge-triggered: keycode 0x05 ('b') present in THIS
    // report and absent from the previous one (the same press-edge test the character loop below
    // uses), while both a Ctrl bit (LeftCtrl 0x01 / RightCtrl 0x10) and an Alt bit (LeftAlt 0x04 /
    // RightAlt 0x40) are held. On that edge, request a re-run of the Bluetooth bring-up chain — the
    // one input affordance a spatial OS with no menu bar has today. It only RECORDS the request; the
    // drain in `service_ehci_hid` runs the chain under the lock. Ctrl+Alt+b resolves to no printable
    // character, so `hid_key_ascii` returns 0 for it and the loop below emits nothing for this key —
    // the chord types nothing. Gated on `bt`: a non-Bluetooth build has no chord and no request path.
    #[cfg(all(target_arch = "x86_64", feature = "bt"))]
    {
        const CHORD_KEY_B: u8 = 0x05;
        let ctrl = modifiers & 0x11 != 0;
        let alt = modifiers & 0x44 != 0;
        let b_down_now = cur_keys.contains(&CHORD_KEY_B);
        let b_was_down = prev_keys.contains(&CHORD_KEY_B);
        if ctrl && alt && b_down_now && !b_was_down {
            serial_println!(
                "{}: BT pairing chord (Ctrl+Alt+B) — requesting Bluetooth re-trigger == witness ::", t.label()
            );
            crate::drivers::ehci::bt_request_retrigger(1);
        }
    }

    // DBLSTROKE: PRESS EDGES, NOT LEVELS. `prev_keys` is still the PREVIOUS report here (it is
    // rewritten at the end of this function), so `!prev_keys.contains(&keycode)` is exactly "this
    // keycode went down in THIS report". A keycode the previous report already carried is a RESTATED
    // level, not a new press, and pushing `Event::Key` for it is what doubled characters on metal.
    let mut restated_this_report = 0u32;
    let mut down_this_report = 0u32; { let mut lev = [crate::pal::Event::None; 12]; let ln = crate::drivers::xhci::hid_layout_push(&cur_keys, prev_keys, modifiers, caps, t.label(), true, &mut lev); for ev in lev[..ln].iter() { if let crate::pal::Event::Key(b) = *ev { note_press_edge(t, b); } } } // KEYCHARS (B718): the report's ONE layout call (stamped for `[login] key latency`, DBLSTROKE-counted); its claimed usages are skipped below. LINE-NEUTRAL, code first.
    for &keycode in cur_keys.iter() {
        if keycode <= 1 {
            continue; // no key / ErrorRollOver
        }
        let ascii = if crate::drivers::xhci::hid_layout_claim(keycode, modifiers, caps).is_some() { 0 } else { ascii_of(keycode) }; // KEYCHARS (B718): a claimed usage typed through `hid_layout_push`
        if ascii == 0 {
            continue;
        }
        down_this_report += 1;
        if prev_keys.contains(&keycode) {
            // Still held from the previous report. Repeat for a held key is the host typematic
            // tracker's job on this path (KEYREPEAT-X86, fed below) — at 400 ms / 40 ms, once — and
            // never the report level's, which is paced by the operator's OTHER fingers.
            restated_this_report += 1;
            continue;
        }
        if !crate::pwwire::withhold() { serial_println!("{}: KEY: class={}", t.label(), crate::pwwire::key_class(ascii)); let _ = keycode; } else { serial_println!("{}: KEY withheld (a secret is being typed)", t.label()); } // CONSOLEFIX M3 (B365, R65): flight 22 printed the login password AND `holocron init`'s here, byte by byte
        crate::boot::key_stamp(); // INSTALLBARE M2: the decode's stamp, read back by `login::consume_key` as `[login] key latency ms=`
        crate::pal::push_event(crate::pal::Event::Key(ascii));
        note_press_edge(t, ascii);
    }
    note_press_report(t, down_this_report, restated_this_report);

    // KEYREPEAT-X86: feed the host-side typematic tracker at the REPORT LEVEL — BEFORE the release
    // edges below and before anything else this report will push, so a `KeyUp` the 64-slot ring may
    // later DROP can never strand a held key (UVUG-6's root cause). `newest_press` is the ascii of a
    // keycode that is down NOW and was NOT down in the previous report; `held` is every ascii down
    // now. Both resolve through `ascii_of`, the same fold the `Event::Key` pushes above used, so the
    // tracker arms the exact byte the consumers received. Non-ascii usages (F-keys, the lock keys)
    // are absent from both, exactly as on the xHCI feed — `IDLE_RUN_TO_LATCH` covers the residue.
    // Mirrors `drivers::xhci`'s call site one-for-one; the tracker itself is shared code.
    {
        let mut held: [u32; 6] = [0; 6];
        let mut hn = 0usize;
        let mut newest_press: u32 = 0;
        for &keycode in cur_keys.iter() {
            if keycode <= 1 {
                continue;
            }
            let ascii = crate::drivers::xhci::hid_repeat_key(keycode, modifiers, caps, ascii_of(keycode)); // CHARREPEAT (SR156): the layout's TOKEN — the byte it typed, or a char token so a held `ü` repeats `ü` (never the US `[`); KEYCHARS (B718) handed the claim byte, 0 for `ü`
            if ascii != 0 {
                held[hn] = ascii;
                hn += 1;
                if !prev_keys.contains(&keycode) {
                    newest_press = ascii;
                }
            }
        }
        #[cfg(any(all(target_arch = "x86_64", feature = "ehcihid"), all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra"))))] { crate::pal::typematic_note_keys(newest_press, &held[..hn]); } for &kc in cur_keys.iter() { if kc > 1 && !prev_keys.contains(&kc) { #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] let _ = crate::video::fnrow::fn_row_usage(if t == Transport::Xhci { crate::video::fnrow::PATH_XHCI } else { crate::video::fnrow::PATH_EHCI }, kc, modifiers); } } // XHCIMEDIA (B438): through the one fn-row router. VOLKEYS: F10/F11/F12 press edges -> mute/down/up (codec amp + bar indicator)
    }

    // The release edges. Bounded by six per report, and a human's key releases are human-rate, so
    // the serial line is unconditional like its `KEY:` twin rather than hidden behind `usbdebug` —
    // it is the wire evidence that this path emits releases at all, which is the one thing Boot AJ
    // could not show.
    for &keycode in prev_keys.iter() {
        if keycode <= 1 {
            continue;
        }
        if cur_keys.contains(&keycode) {
            continue; // still held
        }
        let ascii = release_ascii_of(keycode);
        if ascii != 0 {
            if !crate::pwwire::withhold() { serial_println!("{}: KEYUP: class={}", t.label(), crate::pwwire::key_class(ascii)); } // CONSOLEFIX M3
            crate::pal::push_event(crate::pal::Event::KeyUp(ascii));
        }
    }

    // ALLKEYS P1: lock-key PRESS edges — a lock usage present now and absent last report. Edge and
    // not level, because a boot report re-states the full held set: a caps key held down for half a
    // second is re-reported every poll, and a level test would toggle the state on every one of
    // those reports, flipping caps tens of times per press and landing on whichever parity the
    // release happened to fall on. The edge fires exactly once per physical press.
    //
    // Note these usages produce NO `Key`/`KeyUp` event and never did — `HID_SCANCODE_TO_ASCII` maps
    // all three to (0,0), so the loops above skip them. A lock key changes the MEANING of later
    // keys; it is not itself a character. Mirrors the xHCI toggle loop, sharing its (usage, bit) table.
    let mut changed = false; let mut owed = false; // HIDBOOTLEDSEHCI (B991): `owed` = the transport's answer, returned below
    for &(usage, bit) in crate::drivers::xhci::HID_LOCK_KEYS.iter() {
        if cur_keys.contains(&usage) && !prev_keys.contains(&usage) {
            *leds ^= bit;
            changed = true;
        }
    }
    if changed {
        owed = t.set_leds(*leds); // HIDBOOTLEDS (B987): the ONE LED route; HIDBOOTLEDSEHCI (B991): EHCI's arm owes the caller's LED slot
    }

    // PRTSCR: the Print Screen press edge — the SAME shared predicate the xHCI decoder asks, for the
    // same reason the lock keys share a table: a key is a key whichever controller carried it. This
    // is the path that matters on the bench, because the rMBP's INTERNAL keyboard is on EHCI.
    //
    // Asked here, BEFORE `*prev_keys = cur_keys` below, so the diff is against the previous report.
    // We are inside `service()` holding `EHCI_HID`, so this does exactly one thing — set a flag. The
    // capture runs on the device-service pass; a filesystem write from in here would contend the
    // xHCI storage loan from inside the EHCI pass and hold the keyboard and trackpad hostage for
    // seconds, which is the argument `holocron`'s deferred write already makes at its call site.
    if let Some(act) = crate::drivers::xhci::hid_print_screen_action_edge(&cur_keys, prev_keys, modifiers) {
        // KEYMAP (R60): the 0x46 EDGE is detected as it always was; what the usage MEANS comes from
        // the theme's binding table now, so a theme that drops the row disarms the key here too.
        serial_println!(":: PRTSCR: PrintScreen (HID 0x46) down on {} -> capture armed action={} ::", t.label().trim_end_matches("-HID"), act.name()); // HIDBOOT-TAGS (B987): EHCI byte-identical
        if act.is_capture() { crate::video::prtscr::request(); }
    } else if let Some((act, chord)) =
        crate::drivers::xhci::hid_screenshot_chord_edge(&cur_keys, prev_keys, modifiers)
    {
        // KEYMAP (R60) — the path that matters on the bench: the rMBP's internal keyboard has no Print
        // Screen key and so never puts 0x46 on this wire. The chord is no longer TESTED here or in the
        // shared predicate — `video::keymap::resolve` judges it against the theme's table and this
        // decoder only asks. Same request, same deferral; `else if` so a report carrying both 0x46 and
        // a chord arms exactly once. The chord types nothing: `ascii_of` above folds any GUI-held (and
        // any Alt-held) usage to 0, which covers a PC-shaped table with no new rule.
        if act.is_capture() { serial_println!(
            ":: PRTSCR: [prtscr] chord={} (GUI+Shift+digit) down on {} -> capture armed action={} ::",
            chord, t.label().trim_end_matches("-HID"), act.name()); crate::video::prtscr::request(); } else { serial_println!("[clip] chord={} action={} via={} -> delivered", chord, act.name(), t.tag()); crate::pal::push_event(crate::pal::Event::Action(act)); } } // APPCLIP (R61) — **THE NON-CAPTURE ARM, WHICH UNTIL NOW WENT NOWHERE.** KEYMAP resolved ⌘C/⌘V/⌘X/⌘A here and STOPPED (keymap.md §6: "no consumer and no delivery path"), so the chord was judged and then dropped on the floor. It is now pushed onto the SAME ring the keystrokes from this decoder travel, as `pal::Event::Action` — a third kind of event, neither key nor pointer — and the first thing that acts on it is `video::clipboard::terminal_action`. The capture arm above is byte-for-byte what it was: `is_capture()` still owns `prtscr::request()`, so the PRTSCR wire and flight 11’s two proven chords are untouched, and this `else` can only be reached by an action `is_capture()` refused. No modifier, focus or window test is added here and none may be: R61’s point is that the TABLE decides, which is also why `Ctrl-C` cannot reach this line — no shipped row claims it, so `resolve` returns `None` and `hid_key_ascii` still hands the shell 0x03. ⚠ FOLDED onto the existing statement and given no line of its own — this file is 17k/19k lines of `panic::Location` records that embed line numbers (B94, LEDGER P7); CODE FIRST, then the prose (A10FIX).

    *prev_keys = cur_keys;
    // ALLKEYS: remember this accepted report's modifier byte, so a later endpoint-death flush can
    // resolve each stranded key to the same shifted ascii its press produced. Updated ONLY here —
    // past the `< 8` guard — so a refused short report never overwrites the last true modifier state.
    *prev_mods = modifiers;
    owed
}

// HIDBOOTLEDS (B987): `Transport::set_leds`'s Bluetooth arm — bthid's `PENDING_LED` slot where the
// Bluetooth HID host is linked (`btc`), nothing where it is not.
#[cfg(all(target_arch = "x86_64", feature = "btc"))]
fn bt_queue_led(leds: u8) {
    crate::drivers::ehci::bthid::queue_led(leds);
}
#[cfg(not(all(target_arch = "x86_64", feature = "btc")))]
fn bt_queue_led(_leds: u8) {}

// HIDBOOTLEDSEHCI (rmbp-ledger B991): the `[hidboot] leds` line for a transport whose LED change its
// caller sent (EHCI's slot drain, after the endpoint walk: `ehci::Controller::service`). `r` is the
// control fn's own answer; "timeout" names itself and every other refusal (stall, hse, ...) reads
// `ctrl-fail` (the EHCI-HID ALLKEYS witness beside it keeps the exact cause). Human-rate: one line per
// lock press, and a refusing keyboard's latch makes its refused line once per endpoint.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))] // HIDBOOTLEDSXHCI (B1005): the module's own gate, so the xHCI slots drain on an x86 image without `ehcihid`
pub fn note_leds_sent(t: Transport, leds: u8, addr: u8, intf: u8, r: Result<(), &'static str>) {
    let caps = (leds >> 1) & 1;
    match r {
        Ok(()) => {
            serial_println!("[hidboot] leds via={} leds={:#04x} caps={} addr={} intf={} -> sent", t.tag(), leds, caps, addr, intf);
        }
        Err(e) => {
            let why = if e == "timeout" || e == "held" { e } else { "ctrl-fail" }; // HIDBOOTLEDSXHCI (B1005): `held` names itself
            serial_println!("[hidboot] leds via={} leds={:#04x} caps={} addr={} intf={} -> refused why={}", t.tag(), leds, caps, addr, intf, why);
        }
    }
}

// HIDBOOTLEDSTEST (rmbp-ledger B995) — TAIL-APPENDED: B991's LED SLOT DRAIN, split out of `ehci::Controller::service` so
// a KAT can drive it without a live controller (B991's owed HIDBOOT-LEDS-TEST, input.md §5 Z3 row). The drain cannot
// take `&mut [IntEp]` plus a `send` closure: `IntEp` is private to `ehci`, and the closure would need the controller's
// `&mut self` (EP0's `set_hid_leds`) while the slice already borrows `self.int_eps`. So the slot OWNER implements
// `LedSlots` (EHCI's `Controller` at ehci/mod.rs's tail, the KAT's fake below) and the ONE drain below holds the whole
// rule: skip a clean slot, a latched-off LED and a retired endpoint; one send per dirty slot carrying its LATEST byte
// (toggles in one pass coalesce); the `[hidboot] leds` line; a refusal latches the slot off (one refused line per
// endpoint). Best-effort: a refusal costs a dark key, never a keystroke (the case fold already moved with the byte).

/// HIDBOOTLEDSTEST (B995): a transport's per-endpoint LED slots, as the one drain sees them.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))] // HIDBOOTLEDSXHCI (B1005): the module's own gate, so the xHCI slots drain on an x86 image without `ehcihid`
pub trait LedSlots {
    /// How many slots (EHCI: one per interrupt endpoint).
    fn count(&self) -> usize;
    /// Slot `i` as `(dirty, ok, dead, leds, addr, intf)`, its dirty flag CLEARED by the read.
    fn take(&mut self, i: usize) -> (bool, bool, bool, u8, u8, u8);
    /// Ship `leds` to slot `i`'s device (EHCI: SET_REPORT on EP0); the control fn's own answer.
    fn send(&mut self, i: usize, leds: u8) -> Result<(), &'static str>;
    /// Slot `i` refused: never send to it again this boot.
    fn latch_off(&mut self, i: usize);
}

/// HIDBOOTLEDSTEST (B995): the ONE LED slot drain (B991's loop body, moved verbatim in logic). Returns `(sent, refused)`.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))] // HIDBOOTLEDSXHCI (B1005): the module's own gate, so the xHCI slots drain on an x86 image without `ehcihid`
pub fn drain_led_slots<S: LedSlots>(s: &mut S, t: Transport) -> (u32, u32) {
    let (mut sent, mut refused) = (0u32, 0u32);
    for i in 0..s.count() {
        let (dirty, ok, dead, leds, addr, intf) = s.take(i);
        if !dirty || !ok || dead {
            continue; // nothing owed, a latched-off LED, or a retired endpoint
        }
        let r = s.send(i, leds);
        note_leds_sent(t, leds, addr, intf, r);
        if r.is_err() {
            if r != Err("held") { s.latch_off(i); } // the case fold is untouched; HIDBOOTLEDSXHCI (B1005): a `held` refusal is a deferral the transport owes to its resume (xHCI's `ep0_replay`), counted refused, never latched
            refused += 1;
        } else {
            sent += 1;
        }
    }
    (sent, refused)
}

/// HIDBOOTLEDSTEST (B995): the KAT's slot owner — two endpoints on address 0 (no enumerated USB device holds it).
#[cfg(any(feature = "ehcihid", target_arch = "aarch64"))]
struct KatSlots {
    dirty: [bool; 2],
    ok: [bool; 2],
    dead: [bool; 2],
    leds: [u8; 2],
    refuse: [bool; 2],
    sends: [u32; 2],
    last: [u8; 2],
}

#[cfg(any(feature = "ehcihid", target_arch = "aarch64"))]
impl LedSlots for KatSlots {
    fn count(&self) -> usize {
        2
    }
    fn take(&mut self, i: usize) -> (bool, bool, bool, u8, u8, u8) {
        let d = self.dirty[i];
        self.dirty[i] = false;
        (d, self.ok[i], self.dead[i], self.leds[i], 0, i as u8)
    }
    fn send(&mut self, i: usize, leds: u8) -> Result<(), &'static str> {
        self.sends[i] += 1;
        self.last[i] = leds;
        if self.refuse[i] { Err("kat-refuse") } else { Ok(()) }
    }
    fn latch_off(&mut self, i: usize) {
        self.ok[i] = false;
    }
}

/// HIDBOOTLEDSTEST (B995) — `tests hidboot` (R80: registered, never at boot). The live decoder and the live drain over
/// a fake slot owner: `owed` = `decode_boot_keyboard` under `Transport::Ehci` answers true on a lock-key PRESS EDGE only
/// (restated, released and short reports answer false); `coalesce` = four owed toggles in one pass ship ONE send with
/// the latest byte, a dead endpoint's dirty slot is skipped and cleared, a clean pass sends nothing; `latch` = a
/// refusing endpoint is sent to once, latched off, and never again. The decoder runs under `ehci::EHCI_HID`, as every
/// caller does (the DBLSTROKE atomics and the typematic feed). Lock usages carry no character: no `pal::push_event`.
/// `:: HIDBOOT: owed=ok|bad coalesce=ok|bad latch=ok|bad -> PASS|FAIL ::`
#[cfg(any(feature = "ehcihid", target_arch = "aarch64"))]
pub fn selftest() {
    const CAPS: u8 = 0x39;
    const NUM: u8 = 0x53;
    const KT: Transport = if cfg!(target_arch = "aarch64") { Transport::Xhci } else { Transport::Ehci }; // HIDBOOTARM (B1000): the aarch64 KAT drives the xHCI arm (same `true` answer)
    let rep = |kc: u8| -> [u8; 8] { [0, 0, kc, 0, 0, 0, 0, 0] };
    let up = [0u8; 8];
    let mut fake = KatSlots {
        dirty: [false; 2],
        ok: [true; 2],
        dead: [false, true],
        leds: [0; 2],
        refuse: [false; 2],
        sends: [0; 2],
        last: [0; 2],
    };
    let (mut prev, mut pm) = ([0u8; 6], 0u8);
    let (owed_ok, coalesce_ok, latch_ok) = {
        #[cfg(feature = "ehcihid")] let _g = crate::drivers::ehci::EHCI_HID.lock(); // HIDBOOTARM (B1000): aarch64 has no EHCI; its KAT runs unserialised (lock keys push no event), HIDBOOT-ARM-KATLOCK owed
        // owed: a press edge owes the slot; a restated level, a release and a short report do not.
        let mut leds = 0u8;
        let (mut p0, mut m0) = ([0u8; 6], 0u8);
        let o1 = unsafe { decode_boot_keyboard(KT, &rep(CAPS), &mut p0, &mut m0, &mut leds) };
        let l1 = leds;
        let o2 = unsafe { decode_boot_keyboard(KT, &rep(CAPS), &mut p0, &mut m0, &mut leds) };
        let o3 = unsafe { decode_boot_keyboard(KT, &up, &mut p0, &mut m0, &mut leds) };
        let o4 = unsafe { decode_boot_keyboard(KT, &rep(CAPS)[..4], &mut p0, &mut m0, &mut leds) };
        let owed_ok = o1 && l1 == 0x02 && !o2 && !o3 && !o4 && leds == 0x02;
        // coalesce: slot 0's byte through the decoder, marked as the walk marks it; slot 1 dirty but dead.
        for r in [rep(CAPS), up, rep(NUM), up] {
            if unsafe { decode_boot_keyboard(KT, &r, &mut prev, &mut pm, &mut fake.leds[0]) } {
                fake.dirty[0] = true;
            }
        }
        fake.dirty[1] = true;
        let d1 = drain_led_slots(&mut fake, KT);
        let d2 = drain_led_slots(&mut fake, KT);
        let coalesce_ok = d1 == (1, 0) && fake.sends == [1, 0] && fake.last[0] == 0x03 && !fake.dirty[1] && d2 == (0, 0);
        // latch: a refusing endpoint is sent to once and latched off; the next owed toggle sends nothing.
        fake.refuse[0] = true;
        for r in [rep(CAPS), up] {
            if unsafe { decode_boot_keyboard(KT, &r, &mut prev, &mut pm, &mut fake.leds[0]) } {
                fake.dirty[0] = true;
            }
        }
        let d3 = drain_led_slots(&mut fake, KT);
        for r in [rep(CAPS), up] {
            if unsafe { decode_boot_keyboard(KT, &r, &mut prev, &mut pm, &mut fake.leds[0]) } {
                fake.dirty[0] = true;
            }
        }
        let d4 = drain_led_slots(&mut fake, KT);
        let latch_ok = d3 == (0, 1) && !fake.ok[0] && fake.last[0] == 0x01 && d4 == (0, 0) && fake.sends == [2, 0];
        (owed_ok, coalesce_ok, latch_ok)
    };
    let w = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        ":: HIDBOOT: owed={} coalesce={} latch={} -> {} ::",
        w(owed_ok), w(coalesce_ok), w(latch_ok),
        if owed_ok && coalesce_ok && latch_ok { "PASS" } else { "FAIL" }
    );
}

// HIDBOOTARM (rmbp-ledger B1000) — TAIL-APPENDED. The i8042 arm of `Transport::set_leds` behind its arch (the
// `bt_queue_led` shape: the i8042 module is x86_64-only), and the xHCI keyboard's entry (both arches since HIDBOOTXHCIX86 B1003): `decode_noted` is a
// WITNESS WRAPPER over the ONE decoder, not a second decoder — it calls `decode_boot_keyboard` and, on the first report
// that pushed a press edge, prints `[hidboot] via=<tag> arch=aarch64|x86_64 keys=<edges this report>` once per boot.
#[cfg(target_arch = "x86_64")]
fn ps2_set_leds(leds: u8) {
    crate::drivers::i8042::set_leds(leds);
}
#[cfg(not(target_arch = "x86_64"))]
fn ps2_set_leds(_leds: u8) {}

/// HIDBOOTARM (B1000): set once the first-key line has printed (HIDBOOTXHCIX86 B1003: x86 too).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
static ARM_FIRST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// HIDBOOTARM (B1000): the xHCI keyboard completion's call on both arches (HIDBOOTXHCIX86 B1003; `drivers::xhci` `poll_events`, under the xHCI loan,
/// which owns the slot state passed here). Returns `decode_boot_keyboard`'s answer (true = the caller owes its LED slot).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub fn decode_noted(t: Transport, report: &[u8], prev_keys: &mut [u8; 6], prev_mods: &mut u8, leds: &mut u8) -> bool {
    use core::sync::atomic::Ordering;
    let before = PRESS_EDGES.load(Ordering::Relaxed);
    let owed = unsafe { decode_boot_keyboard(t, report, prev_keys, prev_mods, leds) };
    let keys = PRESS_EDGES.load(Ordering::Relaxed).wrapping_sub(before);
    if keys > 0 && !ARM_FIRST.swap(true, Ordering::Relaxed) {
        serial_println!("[hidboot] via={} arch={} keys={}", t.tag(), if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" }, keys);
    }
    owed
}
