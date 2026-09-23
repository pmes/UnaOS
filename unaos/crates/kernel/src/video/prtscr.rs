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

//! PRTSCR — the screen capture: panel pixels to a PNG on the LOGGED-IN USER'S OWN DESKTOP
//! (SCRSHOT-DESKTOP below — `/home/<name>/Desktop` under CRISPY, the theme's own word), and
//! nowhere at all when nobody is logged in.
//!
//! Two ways in, one mechanism:
//!
//!  * the `screenshot` shell verb — arch-neutral, and the reason the mechanism is testable at all
//!    without a keyboard;
//!  * the **Print Screen key** (HID usage 0x46), which does not type a character and therefore has
//!    no representation in `pal::Event`. Its hook lives at the HID decoders' press edge and does
//!    exactly one thing: [`request`] sets a flag. The capture itself happens in [`service`], on the
//!    device-service pass, for the reason `holocron`'s call site states at length — a filesystem
//!    write issued from inside `service_ehci_hid()` would contend the xHCI storage loan *from
//!    inside the EHCI service pass* and hold the internal keyboard and trackpad hostage for its
//!    whole duration. A screenshot is seconds of work. It does not belong in an input pump.
//!
//! ## The panel is read WITHOUT the panel lock, and that is deliberate
//!
//! `video::mod`'s LOCKFIX rule forbids a bare blocking `WRITER.lock()` from anything preemptible or
//! masked. The sanctioned paint-path door is [`crate::video::panel_snapshot`], which hands back a
//! `FrameBuffer` — a `Copy` HANDLE (base address, length, geometry), not a guard. The lock is
//! released the instant the snapshot returns, and every pixel read afterwards is an ordinary
//! volatile load through that handle. So a capture that takes seconds holds nothing for any of them.
//!
//! What that costs is honest: the compositor may paint between two of our scanlines, so a capture
//! taken while the screen is moving can tear. For a screenshot that is cosmetic, and it is the right
//! trade — the alternative is a ~20 MiB frame copy taken under a lock the whole machine's paint path
//! needs, which is the WEDGE-8 shape this kernel spent three arcs eliminating.
//!
//! ## What the pixels are, and where that is decided
//!
//! [`FrameBuffer::read_pixel`](crate::video::FrameBuffer::read_pixel) is the format authority: it is
//! the documented inverse of `put_pixel` and returns `0x00RRGGBB` for `PixelFormat::Rgb` and
//! `PixelFormat::Bgr` alike, decoding from the `FrameBufferInfo` the firmware reported (UEFI GOP on
//! x86 — BGRx on the rMBP — or the VideoCore mailbox on the Pi). We do NOT assume a byte order; we
//! ask that function, and we refuse a layout it has no colour inverse for (`U8` greyscale averaging
//! is lossy and not invertible) rather than inventing one. PNG then stores plain RGB8 triples.
//!
//! ## Naming
//!
//! A Mac writes `Screenshot 2026-09-22 at 17.31.02.png`. We cannot: `fs/fat.rs:118` writes 8.3 short
//! names only, and every separator in that name is illegal here. The full argument, and the list of
//! what the FATLFN arc must add to earn it, is the block above [`clock_name`]; the rule is:
//!
//!  * **`MMDDHHMM.PNG`** — `09221731.PNG` — when the wall clock is set. Four zero-padded digit
//!    pairs, most-significant first, so a listing in name order is a listing in time order. The
//!    year and the seconds do not fit in eight characters and are dropped VISIBLY rather than
//!    packed into a cryptogram.
//!  * **`SCREEN0.PNG` .. `SCREEN99.PNG`**, first free index wins, when there is no clock this boot
//!    (the state of BOTH lanes today — measured, see that block) or when the clock name is already
//!    taken by a capture in the same minute. The wire says which, on the `-> capturing` line's
//!    `name_from=` field: `clock`, `clock-unset` or `clock-taken`.
//!
//! Both arms are **in the capture directory** (SCRSHOT-DESKTOP, next section — it was the volume
//! root until PRTSCR-HOME) and **never overwrite an existing capture**: each candidate is asked of
//! the filesystem and only a `NotFound` is taken. When all hundred ladder names are gone the verb
//! refuses and says so — it does not wrap around and clobber `SCREEN0.PNG`. Every name this module
//! mints is 8.3-clean, so none of them needs a long-name entry.
//!
//! ## SCRSHOT-DESKTOP — a capture belongs to A USER, and lands on that user's DESKTOP
//!
//! Two rulings, a week apart, and they answer two different questions. Both are live; neither
//! replaces the other.
//!
//!  * **WHOSE folder.** Peter, 2026-09-13: *"screenshots should be saved to a user's
//!    ~/Pictures/Screenshots"* (PRTSCR-HOME) — a consequence of R51, multi-user as a line. What it
//!    settled is that a capture has an OWNER, which makes the no-session case the load-bearing half
//!    and not an edge. Unchanged by what follows.
//!  * **WHICH folder.** Peter, 2026-09-22, **R60**: *"is screenshot working? mac saves to desktop,
//!    correct? we should too, on this pioneer crispy theme anyway."* The destination is
//!    `Desktop`, and — the half that is easy to miss — it is a property of the THEME, on the same
//!    argument R60 makes about key bindings in the same breath. A Windows-shaped theme is coming.
//!
//! ### The destination
//!
//! [`fs::users::whoami`](crate::fs::users::whoami) names the open session; [`home_of`] turns that
//! name into the user's home path — `crate::fs::users::home_of`, the accessor that already exists;
//! `/home/<name>`, the same path `ensure_home` creates on the EL0 FAT volume at first login. The
//! capture directory is that home plus ONE leaf, and the leaf is
//! [`crate::video::theme::CAPTURE_DIR`] — `Desktop` under CRISPY — so the resolved destination is
//! `/home/<name>/Desktop`. [`ensure_capture_dir`] creates any component that is absent, in order,
//! on the volume [`mount_capture_target`] settled on.
//!
//! **This module does not know the word `Desktop` and must not learn it.** It asks the theme table,
//! exactly as `video/wm.rs` asks that table what colour a title bar is. The second theme is one more
//! `const` there and no edit at all here; a literal in this file would be the hard-coding R60 names.
//! Every witness that reports the destination prints `theme=` beside it for the same reason — the
//! day two themes exist, `dir=/home/una/Desktop` alone does not say which table answered.
//!
//! Note which volume that is, because the two can differ and the difference is not a bug: the home
//! `ensure_home` makes lives on the EL0 volume, while a capture goes to the PRTSCR-VOL ladder's
//! answer — which on a read-only-boot bench is the operator's own USB stick. On that stick we create
//! the SAME shaped path under the SAME user name. That is the right answer for a carry-away medium
//! (it is still that user's screenshot, in that user's folder, on that user's disk) and the
//! `source=`/`serial=` fields on every verdict say which disk it was.
//!
//! ### ⚠ THE 8.3 QUESTION, ANSWERED FROM THE CODE THAT DECIDES IT — AND R60 RETIRED THE ALIAS
//!
//! **This FAT layer READS long file names and WRITES 8.3 only.** Both halves are load-bearing here:
//!
//!  * **Read** — PI-FS-3. `fs/fat.rs`'s `LfnBuf` accumulates the 0x0F-attribute VFAT component slots
//!    that precede a short entry, checksum-validates the run against the short name, and decodes it;
//!    `DirEntry::eq_name` (`fs/fat.rs:190`) then matches EITHER the long name or the 8.3 short name,
//!    ASCII-case-insensitively. So `locate_in_dir(home, "Desktop")` **does** find a folder a real
//!    VFAT driver created, spelled exactly as the user spelled it — a stick that already carries a
//!    `Desktop` made on a Mac is ADOPTED and nothing new is written.
//!  * **Write** — `fs/fat.rs:118` states it outright: *"this driver's create path writes 8.3 names
//!    only (VFAT LFN write is out of scope)"*. `format_83` (`fs/fat.rs:325`) is the decider: base
//!    `1..=8` characters, extension `0..=3`, each a legal short-name byte, or `None`. `create_dir`
//!    validates through it before it allocates anything (`fs/fat.rs:3562`), so a name it rejects
//!    comes back `FatError::Unsupported` and no cluster leaks.
//!
//! **`Desktop` is SEVEN characters, and that is the quiet gift in R60's destination.** PRTSCR-HOME
//! needed a two-spelling alias table because `"Screenshots"` is ELEVEN characters and `format_83`
//! returns `None` for it — the folder we looked up could never be the folder we created, so
//! `Screenshots` was written `SCRSHOTS` (visibly an abbreviation, never the truncation `SCREENSH`,
//! which reads as a damaged word). A seven-character leaf clears the base bound with a character to
//! spare, so:
//!
//! | what the user asked for | looked up as | created as | why |
//! |---|---|---|---|
//! | `Desktop` | `Desktop` | `DESKTOP` | 7 characters — a legal 8.3 base exactly as written. No alias, no second spelling. Stored uppercase because short names are; `format_83` upcases what it stores, and nothing in this module does. [`DIR_CAPTURE`] |
//!
//! **THE ALIAS TABLE IS GONE RATHER THAN RE-POINTED.** [`DIR_CAPTURE`] keeps the `(look up, create)`
//! pair type and puts `theme::CAPTURE_DIR` in BOTH fields, so there is exactly one place the
//! destination is written down and no second spelling that could drift from it. The 8.3 legality of
//! whatever a future theme puts there is not const-asserted — a compile error is not a red run —
//! but MEASURED on the wire every boot by [`dir_fixture`] (`legal83=`), whose go-red is to point
//! `CAPTURE_DIR` at a name `format_83` refuses. The decider's own verdict on that name reaches the
//! wire through [`dir_refused`] on any lane that has a session and a writable volume to reach it.
//!
//! **What the Mac does that we still cannot: the FILE name.** `Screenshot 2026-09-22 at 17.31.02.png`
//! needs VFAT LFN WRITE, which this driver does not have (the READ half is built — PI-FS-3 above).
//! The block above [`clock_name`] states what the FATLFN arc must add and what we do until it lands.
//!
//! ### ⚠ NO SESSION MEANS NO CAPTURE — and that is the answer, not a gap
//!
//! There is usually no logged-in user on these boards, and on an ordinary build there is not even
//! the machinery for one — `fs/users.rs` is `#[cfg(feature = "login")]` (`fs/mod.rs:105`), `login`
//! is not a default feature, and the login screen does not open at boot even when it is built
//! (SO43). Those are two different facts and the wire keeps them apart: [`WHY_NO_LOGIN`] for an
//! image with no user store, [`WHY_NO_SESSION`] for one that has it and nobody logged in.
//! Peter, 2026-09-13: *"do not hack screenshots to make it work right before multi-user is in."*
//!
//! So a capture with no session is **REFUSED** — [`Refusal::NoSession`], one bounded line naming the
//! reason, **zero bytes written**. Not the volume root, not a shared folder, not a temporary landing
//! place under a new name. The three things that refusal deliberately is NOT:
//!
//!  * it is not a fallback. A shared destination would make the feature LOOK finished on a machine
//!    where multi-user is not, and would have to be torn out and re-argued the moment sessions open;
//!  * it is not an error path. It is a first-class outcome with its own witness, its own reason
//!    token and its own fixture ([`dir_fixture`]), gone green from the boot it ships on;
//!  * it is not deferred work. The code is complete and correct as written; what it waits for is a
//!    session to exist, and nothing here changes when one does.
//!
//! **The refusal is checked FIRST**, before the panel and before the volume — see [`Job::begin`].
//! A capture that can never belong to anyone must not spin up the PRTSCR-VOL ladder, must not choose
//! a name, and must not create a directory entry, so the check that guarantees all three sits ahead
//! of all three. On a no-session boot a capture therefore costs one lock and one line.
//!
//! ## Where a capture may land — PRTSCR-VOL, the two-rung target ladder
//!
//! A capture wants "a writable FAT volume the operator can carry away", which is NOT the same
//! question `mount_program_source` answers ("the volume this system is bound to"). On a machine
//! whose boot medium is read-only by policy — the 2012 rMBP boots from the internal SD reader,
//! which SDHC-4c mounts read-only outside the reserved flight-recorder extent — the two answers
//! permanently diverge: flight-3 proved that `program_source()` under a `BM_SUBSTITUTED` verdict
//! returns the Sdhc handle on every call, and FRGUARD's `default_writable()` vetoes the global slot
//! under that same verdict, so a capture that only ever consults the program source waits for a
//! writable volume that CANNOT arrive on that bench. [`mount_capture_target`] is the fix:
//!
//!  1. **The program source**, when it admits writes — every boot whose program volume is writable
//!     (QEMU `test-fat`, a stick-booted x86 machine, the Pi's microSD) behaves exactly as before.
//!  2. **The dedicated USB mass-storage handle** (`BlockSource::Usb`), when rung 1 is read-only or
//!     absent. `publish_usb_geometry` populates it on EVERY stick arrival, boot-time or hot-plug
//!     (Boot AI-2 proved hot-plug reaches the FAT layer on metal), and its read/write paths bypass
//!     the backend selector entirely. Crucially this does NOT weaken FRGUARD: the refusal FRGUARD
//!     exists for is a write aimed at the BOOT VOLUME silently landing on whatever claimed the
//!     global slot. This rung aims at the stick BY NAME — the operator's own carry-away medium,
//!     which is exactly where a screenshot belongs — and the global slot's veto stands untouched.
//!
//! Which rung won, and WHICH DISK it landed on, is on the wire: `:: PRTSCR-VOL: … -> MOUNTED ::`
//! per mount, `:: PRTSCR-VOL: rung=none rung1=… rung2=… -> NO TARGET ::` when both decline, and
//! `source=`/`serial=` on every verdict that follows. See the PRTSCR-VOL block at this file's TAIL
//! for why the pair (handle, `BS_VolID`) and not the handle alone: since USBREG the `usb` handle is
//! REGISTRY ENTRY 0 (`USB_DISKS[0]`) — a socket a retraction empties and the next arrival refills.
//!
//! ## One capture at a time, and the wire names every state (PRTSCR2)
//!
//! A capture is seconds of work — on the Orin, 1920x1200 encodes and writes 6.9 MB over USB BOT in
//! ~7.9 s (render3b, `[pstrip] gapmax=7894ms`) — and the FAT write is crash-consistent in exactly
//! one direction: `write_grow` publishes the directory entry's size LAST (`fs/fat.rs`, "SAFE
//! ORDER"), so a boot cut mid-write leaves the entry `create_in_dir` made at its original 0 bytes.
//! That 0-byte `SCREEN<n>.PNG` is therefore not a mystery file but the interrupted-write signature,
//! and this module's job is to make sure the wire has already NAMED that file before the entry can
//! exist. Three rules follow:
//!
//!  * **`capture` announces before it commits.** The `-> capturing` line prints after the name is
//!    chosen and before a pixel is read, so every capture the wire sees ends in exactly one of
//!    `-> OK`, a `— capture skipped` refusal, or nothing after `-> capturing` — and the last one
//!    means the boot ended inside the capture, which the operator can then read off the log.
//!  * **One capture at a time.** [`IN_FLIGHT`] is taken at the door of [`capture`] and released on
//!    every exit path. The Print Screen key and the `screenshot` verb reach `capture` from different
//!    tasks (the device-service pass and the console), and `next_free_name` -> `create_in_dir`
//!    cannot de-duplicate across two concurrent callers — both would choose the same free index.
//!    A caller that finds the door taken gets [`Refusal::InFlight`], a named refusal on the wire.
//!  * **A press during a capture is deferred, not dropped.** The xHCI event ring is drained by the
//!    same `drain_event_ring_once` whether `poll_events` or the synchronous BOT pump is running, so
//!    the keyboard's press edge is decoded — and [`request`] runs — from INSIDE the storage write of
//!    the capture already in flight. [`service`] cleared [`PENDING`] before starting that capture,
//!    so the press re-arms it and the next service pass runs the second capture. Should `service`
//!    itself ever meet the door taken (a verb capture on the console task), it re-arms `PENDING`
//!    and says so once; the request is serviced when the door opens.
//!
//! ## PRTSCR-ASYNC — the capture is SLICED, so the machine is never wedged for it (SR2)
//!
//! Everything above was true and the machine still died for the duration. `service()` ran the whole
//! encode-and-write as ONE call from inside the device-service pass, and that pass is also the pass
//! that polls the keyboard and the trackpad: **70 s on the 2012 rMBP** (15.5 MB at ~220 KB/s, with
//! `[deadman] pmp=0` throughout), **6–9 s on the Orin** at 1920x1200. Peter, at the glass on
//! render7: *"3 presses in a row didn't do 3"* — and the census agreed, 8 armed / 7 OK / 1 silent.
//! The deferral above cannot fire for a press the input pump never gets to decode.
//!
//! So the capture is now a **state machine that runs in bounded slices**, and `service()` advances
//! it by one slice per pass:
//!
//!  * [`Job::begin`] does the cheap refusals, chooses the name, prints `-> capturing`, and builds
//!    the encoder. Everything that could refuse still refuses before a pixel is read — and the
//!    volume it settles on is [`mount_capture_target`]'s, the PRTSCR-VOL ladder above, NOT
//!    `mount_program_source`: slicing must not quietly re-narrow the target back to rung 1 and
//!    strand the rMBP's read-only boot medium all over again.
//!  * [`Phase::Encode`] pushes [`SLICE_ROWS`] scanlines at a time into the streaming encoder.
//!  * [`Phase::Write`] writes [`SLICE_WRITE`] bytes per `write_grow`, **in order**, from offset 0
//!    upward, so what is on the medium is always a valid PREFIX of the finished PNG.
//!  * [`Job::slice`] runs those units until [`slice_budget`] cycles have been spent, then returns.
//!    The budget is a fraction of the arch's own `hw_wait_budget()`, so it is a DURATION on every
//!    board (~31 ms on x86, ~37 ms on QEMU virt, ~43 ms on the Pi, ~75 ms on the Orin) without this
//!    module learning any board's clock rate.
//!
//! **No lock is held across a slice.** The open job lives in [`JOB`], a `spin::Mutex<Option<Job>>`
//! that is locked exactly twice per slice — once to move the job out, once to move it back — and
//! never while a pixel is read or a byte written. The FAT/BOT layer takes and releases its own loan
//! inside each `write_grow` exactly as it did for the single big write.
//!
//! **What changed for the operator, precisely.** A press that lands during an open capture prints
//! the named [`Refusal::InFlight`] line (once per episode) **and stays armed**, so it runs as the
//! next capture the moment the open one reaches its verdict: three presses in a row now make three
//! files, with the collapse — when two presses land inside one slice window — named on the wire
//! instead of silent. Progress is `:: PRTSCR: slice n=… bytes=…/… ::`, capped at
//! [`SLICE_LINES_MAX`] lines per capture so a 484-slice rMBP write does not become 484 lines.
//!
//! **The interrupted-write signature moves, and this is the one thing a reader must relearn.**
//! `write_grow` publishes the directory size last *per call*, and there are now many calls, so a
//! boot cut mid-capture no longer leaves `SCREEN<n>.PNG` at 0 bytes — it leaves it at the last
//! slice boundary the volume accepted, a truncated PNG with a valid header and no `IEND`. The
//! `-> capturing` line still states the RESERVED length, so `size < reserved` and a missing verdict
//! line are together the interrupted-write signature. A 0-byte file still means the cut landed
//! before the first slice.
//!
//! **`capture()` itself is still synchronous** — it is the same state machine driven to completion
//! in a loop, so the `screenshot` verb and PRTSCR-ST get byte-identical behaviour and the same wire.
//! Only the Print Screen path, whose driver is the input pump, is sliced.
//!
//! ## The volume may leave while the capture is still writing — PRTSCR-ASYNC/UNPLUG
//!
//! Slicing turns a hypothetical into a routine one: a capture that used to own the machine for one
//! uninterruptible write now spans seconds of passes during which the operator can pull the stick,
//! and rung 2 of the ladder above aims at exactly the medium most likely to be pulled.
//!
//! **What happened before this section existed** (the honest one-sentence answer, and it is not "it
//! faults"): nothing dangled and nothing paniced — `drivers/block.rs`'s USB-UNPLUG retraction clears
//! its USBREG entry, every block entry point re-reads the registry through `info()` / `usb_info()`
//! on EVERY call and geometry-bounds the LBA against that fresh snapshot, so the next `write_grow`
//! failed honestly with `BlockError::NotReady` and PRTSCR reported it as the GENERIC
//! `Refusal::Fat("write", …)` line, which names a FAT errno and never names the disconnection.
//!
//! Two things were wrong with that. The refusal was unreadable — an operator who pulled a stick got
//! a `write failed -EIO` and had to infer the cause — and, worse, the honest failure is only
//! guaranteed while the handle stays EMPTY. A retract followed by a replug (or a different stick on
//! a recycled xHCI slot) refills that USBREG entry with a DIFFERENT disk, and the by-value `FatFs`
//! this job parked between slices still holds the old volume's LBAs. The next `write_grow` would
//! then be geometry-bounds-checked against the new disk and pass — a write through a stale handle,
//! onto a stranger's filesystem. That is the case slicing creates and the one this refuses.
//!
//! **The probe, and which of rung 2's facts it uses.** Both of them, because they answer different
//! halves. [`Job`] records, at `begin`, whether this mount is USB-backed at all, plus
//! `block::usb_publish_gen()` — the generation `publish_usb_geometry` bumps on EVERY arrival. Before
//! each volume-touching step (`create_in_dir`, and every `write_grow`) a USB-backed job requires
//! **`block::usb_info().is_some()`** — the geometry publish is still standing, so the stick did not
//! merely leave — **and the generation to be unchanged** — so no arrival has replaced it. Either
//! test failing is [`Refusal::Vanished`], a named line carrying the byte count reached, and the
//! write is NOT issued: the stale handle is never written through. A capture that is not USB-backed
//! (the Pi's microSD, QEMU's `test-fat` image) skips the probe entirely and costs nothing.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use spin::Mutex;
use unaos_boot_info::PixelFormat;

use crate::fs::fat::{BlockSource, FatError, FatFs};
use crate::video::FrameBuffer;
use crate::video::png::{PngEncoder, PngError};

/// Highest capture index. `SCREEN99.PNG` is 11 characters — still 8.3, still no long-name entry.
const MAX_CAPTURES: u32 = 100;

/// How many times a FAT operation may answer `Busy` before we give up on it.
///
/// `Busy` is the block layer refusing to WAIT for a loan it could not take instantly — under
/// WEDGE-8 that is the fix working, not a fault (`drivers/block.rs`'s note: "a NORMAL, RETRYABLE
/// outcome — not a wedge verdict"; `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §32.3). So it is
/// retried with bounded patience exactly as `fs::fat`'s own RMW wrappers retry it, and only a
/// budget that actually expires becomes `-EAGAIN` for the operator.
const BUSY_ATTEMPTS: u32 = 64;

/// PRTSCR-ASYNC — how long one slice may run, as a DIVISOR of the arch's hardware-wait budget.
///
/// `arch::hw_wait_budget()` is the one wall-clock quantity both arches already express in
/// `now_cycles()` units — an honest `tsc_hz * 2 s` on x86 once the APIC calibration lands, and a
/// CNTFRQ-derived 2.4 s / 2.78 s / 4.8 s on QEMU virt / the Pi 4 / the Orin. Dividing it is how this
/// module gets a duration on a board whose clock rate it does not know: 2 s / 64 ≈ 31 ms on x86,
/// ~37 ms on virt, ~43 ms on the Pi, ~75 ms on the Orin. All comfortably inside a human's
/// input-latency floor, and all bounded — which is the whole property SR2 asks for.
const SLICE_BUDGET_DIV: u64 = 64;

/// PRTSCR-ASYNC — scanlines encoded between two budget checks. A row is `width` volatile loads and
/// an `extend_from_slice`; 64 of them at 1920 px is ~123k loads, well under a millisecond, so the
/// granularity costs nothing and the budget check does not dominate.
const SLICE_ROWS: u32 = 64;

/// PRTSCR-ASYNC — bytes handed to `write_grow` per call, and therefore the LONGEST uninterruptible
/// span of a capture: a `write_grow` cannot be preempted from outside, so this constant IS the
/// worst-case wedge. 32 KiB is ~145 ms on the rMBP's measured 220 KB/s and ~37 ms on the Orin's
/// 870 KB/s — against 70 s and 7.9 s for the single write it replaces.
///
/// It is not smaller because each call re-walks the file's cluster chain (CHAINGROW gave that walk a
/// FAT-sector cache precisely for windowed writers like this one, so a 15.5 MB file costs ~4 sector
/// reads per call — ~1900 extra reads over the whole capture, ~1–2% of a 70 s write). Halving the
/// slice doubles that overhead to buy 70 ms; this is the knee.
const SLICE_WRITE: usize = 32 * 1024;

/// PRTSCR-ASYNC — how many `slice` progress lines one capture may print. The rMBP's 15.5 MB capture
/// is ~484 slices; the wire is evidence, not a progress bar, so it gets the opening ones (which is
/// where a capture that dies early dies) and the verdict line carries the total.
const SLICE_LINES_MAX: u32 = 8;

/// PRTSCR — a capture has been asked for and not yet performed. Set by [`request`] (from the HID
/// decoders' press edge, where nothing may block), cleared by [`service`] (on the device-service
/// pass, where I/O is legal).
///
/// A plain flag, not a counter: holding Print Screen down through a capture that takes seconds
/// should produce one file, not a queue of them. Extra presses during a capture collapse into the
/// single pending request, and [`REQUESTS`] records how many arrived so the collapse is visible
/// rather than silent.
static PENDING: AtomicBool = AtomicBool::new(false);

/// PRTSCR — total Print Screen press edges seen, whether or not each produced a file. The
/// denominator for [`CAPTURES`]: "the key was pressed and nothing appeared" and "the key never
/// arrived" are different failures and a census that cannot tell them apart is worthless on metal.
static REQUESTS: AtomicU32 = AtomicU32::new(0);

/// PRTSCR — captures that reached a written file.
static CAPTURES: AtomicU32 = AtomicU32::new(0);

/// PRTSCR — capture attempts that ended in a refusal (no volume, read-only, full, I/O).
static REFUSALS: AtomicU32 = AtomicU32::new(0);

/// PRTSCR2 — the capture door: `true` while a [`capture`] is running on ANY task. Taken by
/// compare-exchange at the top of `capture`, released on every exit path (one release site, after
/// the inner body returns). See the module note: two callers past `next_free_name` at once would
/// both take the same free index and `create_in_dir` would make two entries with one name.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// PRTSCR2 — `service` has already said "deferred: capture in flight" for the request it is
/// holding. One line per deferral episode, not one per 250 ms sweep: cleared whenever a capture
/// reaches a verdict through `service`.
static DEFERRED_SAID: AtomicBool = AtomicBool::new(false);

/// PRTSCR-ASYNC — a sliced capture is OPEN: [`JOB`] holds it (or a task is between two of its
/// slices, holding it on the stack), and [`IN_FLIGHT`] is held on its behalf until it reaches a
/// verdict. Read on the idle path so `service` can stay two relaxed loads when nothing is happening.
static SLICING: AtomicBool = AtomicBool::new(false);

/// PRTSCR-ASYNC — the open capture, parked between slices.
///
/// The lock is taken EXACTLY twice per slice — once to move the job out, once to move it back — and
/// never across a pixel read, an encode, or a write. That is deliberate and is the rule this arc was
/// briefed against: a lock held across a slice would reinvent the wedge one layer down. The moves
/// are moves of a `String`, a `Vec` and a `FatFs`; nothing is copied.
static JOB: Mutex<Option<Job>> = Mutex::new(None);

/// PRTSCR — `(requests, captures, refusals)`.
pub fn census() -> (u32, u32, u32) {
    (
        REQUESTS.load(Ordering::Relaxed),
        CAPTURES.load(Ordering::Relaxed),
        REFUSALS.load(Ordering::Relaxed),
    )
}

/// PRTSCR — **the key hook, and the whole of what runs in the input pump.**
///
/// Called from both HID decoders on the Print Screen press EDGE. One relaxed store and one relaxed
/// increment: no allocation, no lock, no I/O, no print beyond the single witness line the caller
/// emits. Everything a screenshot actually costs happens later, in [`service`].
pub fn request() {
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    PENDING.store(true, Ordering::Relaxed);
}

/// PRTSCR — advance the capture by one bounded slice, opening one first if a request is armed.
/// Call from a device-service pass: task context, interrupts enabled, no driver lock held.
///
/// **PRTSCR-ASYNC: this call is bounded** ([`slice_budget`], ~31–75 ms depending on the board) and
/// returns so its caller can poll the keyboard again. It used to run the whole encode-and-write —
/// 6–9 s on the Orin, 70 s on the rMBP — from inside the pass that services input, which is SR2.
///
/// Costs THREE relaxed loads per call when idle (two until PRTSCR-HOME added [`dir_fixture`]'s
/// one-shot latch below), which is why it can still sit unconditionally beside `fat::probe_once()`
/// at every storage-ready pass this kernel carries.
pub fn service() {
    // PRTSCR-HOME: the destination fixture, once per boot. It is here rather than behind a knob
    // because the no-session refusal it measures is what EVERY board does today — a gate that only
    // runs when someone remembers a knob would never have measured the ordinary case even once.
    // One relaxed load per pass after the first; see `dir_fixture` for both arms and their go-reds.
    dir_fixture();
    if !PENDING.load(Ordering::Relaxed) && !SLICING.load(Ordering::Relaxed) {
        return;
    }

    // (a) A capture is already open. Advance it by one slice and hand the machine back.
    if SLICING.load(Ordering::Relaxed) {
        // PRTSCR-ASYNC: a press that landed inside this capture. It is NAMED — once per episode,
        // not once per pass — and left armed, so it becomes the next capture rather than a silent
        // collapse. This is the line that was structurally unreachable before slicing: the input
        // pump could not decode the press at all while the write owned the pass.
        if PENDING.load(Ordering::Relaxed) && !DEFERRED_SAID.swap(true, Ordering::Relaxed) {
            Refusal::InFlight.report();
        }
        let open = { JOB.lock().take() };
        let mut job = match open {
            Some(job) => job,
            // Another task is between two slices of this same capture, holding it on its stack.
            None => return,
        };
        match job.slice() {
            Ok(None) => *JOB.lock() = Some(job),
            Ok(Some(shot)) => finish(Ok(shot)),
            Err(why) => finish(Err(why)),
        }
        return;
    }

    // (b) Nothing open and a request is armed: open one.
    //
    // Clear BEFORE the work, not after: a press that lands mid-capture should arm the NEXT one
    // rather than be swallowed by our own clear. (On the Orin that press is decoded from INSIDE
    // this capture's own storage write — see the module note — and this clear-first order is
    // what makes it a second capture instead of a lost one.)
    PENDING.store(false, Ordering::Relaxed);
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        // PRTSCR2: the door is held by another task's synchronous capture (the `screenshot` verb).
        // Not a refusal of the request — it is re-armed and opens on the first pass after the door
        // opens. Said once per episode so a 7 s verb capture does not print 28 copies of the line.
        PENDING.store(true, Ordering::Relaxed);
        if !DEFERRED_SAID.swap(true, Ordering::Relaxed) {
            Refusal::InFlight.report();
        }
        return;
    }
    match Job::begin() {
        Ok(job) => {
            *JOB.lock() = Some(job);
            SLICING.store(true, Ordering::Release);
        }
        // Every cheap refusal (no panel, no volume, read-only, all names taken, allocator) still
        // arrives here, before a pixel is read — `begin` kept that order, and kept the PRTSCR-VOL
        // ladder that decides which volume is being refused about.
        Err(why) => finish(Err(why)),
    }
}

/// PRTSCR-ASYNC — release the door and print the one verdict line a `-> capturing` is owed.
///
/// The single exit for the sliced path, so `SLICING` and `IN_FLIGHT` cannot be left set by a branch
/// that forgot them — the PRTSCR2 "released on every exit path" rule, now that there are more exits.
fn finish(verdict: Result<Shot, Refusal>) {
    SLICING.store(false, Ordering::Relaxed);
    IN_FLIGHT.store(false, Ordering::Release);
    DEFERRED_SAID.store(false, Ordering::Relaxed);
    match verdict {
        Ok(shot) => {
            CAPTURES.fetch_add(1, Ordering::Relaxed);
            shot.report_ok();
        }
        Err(why) => {
            REFUSALS.fetch_add(1, Ordering::Relaxed);
            why.report();
        }
    }
}

/// A capture that landed: what was written, where, and how big.
pub struct Shot {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    /// PRTSCR-VOL — WHICH volume the file landed on. See [`VolId`] at this file's tail.
    pub vol: VolId,
    /// PRTSCR-HOME — the capture directory as it is spelled ON DISK (e.g.
    /// `HOME/UNA/PICTURES/SCRSHOTS`), for the verdict line's `dir=` field.
    pub dir: String,
    /// PRTSCR-HOME — that directory's first cluster, so a reader (PRTSCR-ST) can find the file
    /// again without re-walking the path.
    pub dir_cluster: u32,
}

/// Why a capture did not happen. Every variant carries what it inspected, not just what was
/// missing — the WINX-8 refusal discipline.
pub enum Refusal {
    /// PRTSCR-HOME — **there is no user to save this capture for.** The reason token says which of
    /// the three shapes it is: `no-session` (nobody is logged in — the ordinary case on every board
    /// until SO43's login screen opens at boot), `unknown-user` (a session names a user the store
    /// has no row for), `bad-home` (the row's home path is not one this FAT layer can walk).
    ///
    /// A first-class outcome, not an error path: nothing is written, no name is chosen, no volume is
    /// touched, and the wire says so in one line. See the module note's NO SESSION section for why
    /// this is a refusal and not a shared-folder fallback.
    NoSession(&'static str),
    /// No framebuffer attached, or the panel lock was contended while masked.
    NoPanel,
    /// The panel's pixel layout has no colour inverse (`U8` greyscale, or an unknown format).
    NoFormat(PixelFormat),
    /// Nothing mounted on the program-source handle NOR the dedicated USB handle.
    NoVolume(FatError),
    /// The program-source volume mounted and refuses writes — `(target, label, reason)` — and the
    /// USB rung of the ladder had no writable volume to offer either.
    ReadOnly(VolId, String, &'static str),
    /// `SCREEN0.PNG` .. `SCREEN99.PNG` are all taken **on that volume**. We do not overwrite.
    AllTaken(VolId),
    /// The PNG encoder declined, with the geometry it declined for. The only post-mount refusal
    /// that carries no [`VolId`], and deliberately: it is a fact about the allocator and the panel
    /// geometry, and no volume was touched to reach it.
    Encode(PngError, u32, u32, usize),
    /// A FAT operation failed: `(target, what we were doing, the error)`.
    Fat(VolId, &'static str, FatError),
    /// The write was accepted but short: `(target, name, written, wanted)`.
    Short(VolId, String, usize, usize),
    /// PRTSCR2 — another task's capture holds the door ([`IN_FLIGHT`]). The verb reports it and
    /// stops; [`service`] re-arms the request and runs it once the door opens.
    InFlight,
    /// PRTSCR-ASYNC/UNPLUG — the USB volume this capture was writing left (or was replaced by a
    /// different disk on a recycled xHCI slot) between two slices: `(target, name, bytes reached,
    /// wanted)` — and the target is the one the capture OPENED, which is the whole point of naming
    /// it here: the disk now under the handle may be a different one.
    /// The write that would have gone through the now-stale handle was NOT issued.
    Vanished(VolId, String, usize, usize),
}

impl Refusal {
    /// One honest serial line naming the reason AND what was inspected. Mirrors the WINX-8 skip
    /// lines: a guard with a `return`, never a panic, never silence.
    pub fn report(&self) {
        match self {
            // PRTSCR-HOME: the ONE line a no-session capture costs. It names the reason token, says
            // plainly that nothing was written, and names what would end the refusal — so an
            // operator reading a boot log is never left to wonder where the file went.
            Refusal::NoSession(why) => serial_println!(
                ":: PRTSCR: no user session (reason={}) — a capture belongs to a user's own {} folder (theme={}) and there is none; NOTHING WRITTEN (no name chosen, no volume touched) — capture skipped ::",
                why,
                DIR_CAPTURE.0,
                crate::video::theme::NAME
            ),
            Refusal::NoPanel => serial_println!(
                ":: PRTSCR: no panel attached (or the panel lock was contended while masked) — capture skipped ::"
            ),
            Refusal::NoFormat(f) => serial_println!(
                ":: PRTSCR: panel layout {:?} has no RGB inverse — capture skipped ::", f
            ),
            Refusal::NoVolume(e) => serial_println!(
                ":: PRTSCR: no FAT volume on the program-source or USB handles ({:?}; handles={}) — capture skipped ::",
                e,
                crate::drivers::block::source_census()
            ),
            Refusal::ReadOnly(v, label, why) => serial_println!(
                ":: PRTSCR: REFUSED READ-ONLY (source={} serial=0x{:08X} label={} reason={}) — no writable USB volume attached either — capture skipped ::",
                v.source,
                v.serial,
                if label.is_empty() { "-" } else { label.as_str() },
                why
            ),
            Refusal::AllTaken(v) => serial_println!(
                ":: PRTSCR: SCREEN0.PNG..SCREEN{}.PNG all present in the capture directory — capture skipped (nothing overwritten) :: source={} serial=0x{:08X} ::",
                MAX_CAPTURES - 1,
                v.source,
                v.serial
            ),
            Refusal::Encode(e, w, h, need) => serial_println!(
                ":: PRTSCR: encoder declined ({:?}) for {}x{} needing {} bytes — capture skipped ::",
                e, w, h, need
            ),
            Refusal::Fat(v, what, e) => serial_println!(
                ":: PRTSCR: {} failed {} ({:?}; handles={}) — capture skipped :: source={} serial=0x{:08X} ::",
                what,
                fat_errno(*e),
                e,
                crate::drivers::block::source_census(),
                v.source,
                v.serial
            ),
            Refusal::Short(v, name, written, wanted) => serial_println!(
                ":: PRTSCR: {} short write {} of {} bytes — capture INCOMPLETE :: source={} serial=0x{:08X} ::",
                name, written, wanted, v.source, v.serial
            ),
            Refusal::InFlight => serial_println!(
                ":: PRTSCR: refused — capture in flight (another task holds the capture door; a key request is re-armed and runs after it) ::"
            ),
            Refusal::Vanished(v, name, done, total) => serial_println!(
                ":: PRTSCR: {} — volume vanished mid-capture at {}/{} bytes (usb geometry retracted or a newer publish replaced it; handles={}) — capture ABANDONED, nothing written through the stale handle :: source={} serial=0x{:08X} ::",
                name,
                done,
                total,
                crate::drivers::block::source_census(),
                v.source,
                v.serial
            ),
        }
    }

    /// The one-sentence form for a console. The serial line above carries the forensics; the panel
    /// clips at 128-180 columns, so the operator gets the verdict and the capture gets the census
    /// (FATVERB's two-sinks-two-lengths rule).
    pub fn sentence(&self) -> String {
        match self {
            Refusal::NoSession(why) => alloc::format!(
                "screenshot: refused ({}) — a screenshot is saved to a user's own {} and nobody is logged in",
                why, DIR_CAPTURE.0
            ),
            Refusal::NoPanel => String::from("screenshot: no panel attached"),
            Refusal::NoFormat(f) => alloc::format!("screenshot: panel layout {:?} has no RGB inverse", f),
            Refusal::NoVolume(e) => alloc::format!("screenshot: no FAT filesystem ({:?})", e),
            Refusal::ReadOnly(v, _, _) => {
                alloc::format!("screenshot: REFUSED READ-ONLY ({}); plug a writable USB FAT volume", v.source)
            }
            Refusal::AllTaken(v) => alloc::format!(
                "screenshot: SCREEN0..SCREEN{}.PNG all present on {} — delete one", MAX_CAPTURES - 1, v.source
            ),
            Refusal::Encode(e, w, h, need) => {
                alloc::format!("screenshot: encoder declined ({:?}) for {}x{} ({} bytes)", e, w, h, need)
            }
            Refusal::Fat(v, what, e) => {
                alloc::format!("screenshot: {}: {} ({:?}) on {}", what, fat_errno(*e), e, v.source)
            }
            Refusal::Short(_, name, written, wanted) => {
                alloc::format!("screenshot: {}: short write {} of {} bytes", name, written, wanted)
            }
            Refusal::InFlight => {
                String::from("screenshot: a capture is already in flight — retry after its verdict")
            }
            Refusal::Vanished(_, name, done, total) => alloc::format!(
                "screenshot: {}: volume vanished mid-capture at {}/{} bytes", name, done, total
            ),
        }
    }
}

/// The errno spelling the shell's FAT verbs use, so a PRTSCR line and an `ls` line name the same
/// failure the same way. (`shell::fat_errno` is private to that file; this is the same mapping over
/// the same public enum.)
fn fat_errno(e: FatError) -> &'static str {
    match e {
        FatError::NoDisk => "-ENODEV",
        FatError::Io => "-EIO",
        FatError::NotFat => "-ENOTSUP",
        FatError::Unsupported => "-EINVAL",
        FatError::NotFound => "-ENOENT",
        FatError::IsDirectory => "-EISDIR",
        FatError::BadChain => "-EIO",
        FatError::NoSpace => "-ENOSPC",
        FatError::OutOfVolume => "-EIO",
        FatError::Busy => "-EAGAIN",
    }
}

/// Run a FAT operation, retrying a `Busy` answer with bounded patience.
///
/// `Busy` means the block device was on loan and this call declined to wait for it — a healthy slow
/// transaction, not a failure. `fs::fat`'s own RMW wrappers retry it exactly this way; the budget is
/// the same hardware-handshake budget, so an operation that never gets the loan still terminates and
/// still tells the truth (`-EAGAIN`) instead of spinning.
///
/// `hlt` only while unmasked: halting with interrupts off is the WEDGE-8 death, and the block layer
/// makes the same distinction at its own claim site.
fn busy_retry<R>(mut op: impl FnMut() -> Result<R, FatError>) -> Result<R, FatError> {
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    for _ in 0..BUSY_ATTEMPTS {
        match op() {
            Err(FatError::Busy) => {}
            other => return other,
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            break;
        }
        if !crate::arch::irqs_masked() {
            crate::hlt();
        }
    }
    Err(FatError::Busy)
}

/// PRTSCR-VOL — mount the volume a capture may write, by the two-rung ladder the module note
/// states: the program source when it admits writes, else the dedicated USB mass-storage handle.
///
/// The refusal returned when BOTH rungs decline describes rung 1 — the more informative failure:
/// `ReadOnly` names the vetoing source (and its report adds that no writable USB volume was
/// attached either), `NoVolume` carries the mount error and the handle census. Rung 2 is consulted
/// fresh on every call, which is what makes a LATER-ARRIVING stick reachable: `usb_info()` re-reads
/// the registry, so the pass after `publish_usb_geometry` runs sees the new volume with no cache to
/// invalidate.
fn mount_capture_target() -> Result<FatFs, Refusal> {
    let (primary, r1) = match crate::fs::fat::mount_program_source() {
        Ok(fs) => match fs.write_veto() {
            None => {
                vol_mounted(1, &fs);
                return Ok(fs);
            }
            Some(why) => (Refusal::ReadOnly(vol_id(&fs), fs.label(), why), VOL_R_READONLY),
        },
        Err(e) => (Refusal::NoVolume(e), VOL_R_NOVOLUME),
    };
    // Rung 2: the stick under its OWN handle — never the ambient global, whose FRGUARD veto is not
    // ours to bypass. Gated on the registry so an absent stick costs one lock, not a mount attempt.
    // `r2` is advanced past each test it survives, so the decline witness names the LAST thing that
    // was true rather than a single undifferentiated "no".
    let mut r2 = VOL_R_ABSENT;
    if crate::drivers::block::usb_info().is_some() {
        r2 = VOL_R_MOUNTFAIL;
        if let Ok(fs) = crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb) {
            r2 = VOL_R_VETOED;
            // `Usb`'s write_veto is `None` today; asked anyway so this ladder keeps telling the
            // truth if that arm ever grows a refusal.
            if fs.write_veto().is_none() {
                vol_mounted(2, &fs);
                return Ok(fs);
            }
        }
    }
    vol_declined(r1, r2);
    Err(primary)
}

/// PRTSCR-ASYNC/UNPLUG — does this mount ride the USB stick, and is therefore hot-unpluggable
/// underneath an open sliced capture?
///
/// Two ways it can, and both must be caught, because [`mount_capture_target`]'s two rungs reach the
/// same disk by different names:
///
///  * rung 2 mounted it explicitly — `source_name()` is `BlockSource::Usb`'s;
///  * rung 1 mounted the PROGRAM SOURCE and on x86 that IS the stick, because
///    `publish_usb_geometry` claims the global slot as well as the dedicated one on any target
///    without the aarch64 backend selector. The two handles are the same disk exactly when the
///    ENUMERATOR says so, which is what [`crate::fs::fat::same_device`] asks.
///
/// CLONEALIAS (orin 24, rmbp-ledger B98): this used to compare `slot_id` here, inline. It is now
/// the one predicate `fs::bootdisk::admit` also uses — deliberately, and deliberately TIGHTER than
/// what it replaced: `same_device` adds the `num_blocks` check (a freed slot handed to some other,
/// differently sized device no longer reads as the same disk) and makes the slot-0 refusal an
/// EXPLICIT guard instead of a property of the sentinel. Two same-device tests that can drift apart
/// is the defect class this closes; there is one test, and it has two callers.
///
///  * **Pi** — the microSD holds the global with `slot_id: 0` while a stick holds the USB handle.
///    The `slot_id != 0` clause fails FIRST, so the answer is `false` by the guard rather than
///    incidentally by a slot mismatch, and a capture to the card is NOT probed: pulling an
///    unrelated stick must not refuse it. This is pi 7's caveat on the old comparison — "safe ONLY
///    because slot 0 is never a real xHCI device (`xhci/mod.rs:2890`; the SD sentinel is
///    `emmc2.rs:632`), and if that invariant moves the Pi's card is misclassified USB-backed and
///    `volume_alive()` probes a generation that never advances → permanent refusal" — turned from
///    a comment into code.
///  * **x86** — one card under both handles: same live slot, same `num_blocks`, so `true`,
///    unchanged.
fn usb_backed(fs: &FatFs) -> bool {
    let name = fs.source_name();
    if name == BlockSource::Usb.name() {
        return true;
    }
    if name == BlockSource::Default.name() {
        return match (crate::drivers::block::info(), crate::drivers::block::usb_info()) {
            (Some(global), Some(usb)) => crate::fs::fat::same_device(&global, &usb),
            _ => false,
        };
    }
    false
}

/// The first `SCREEN<n>.PNG` the CAPTURE DIRECTORY does not already hold.
///
/// Asks the filesystem per candidate rather than scanning a directory listing, because
/// `locate_in_dir` matches on BOTH the 8.3 short name and any long name — a file whose long name
/// differs from its short name would slip past a listing scan and then be duplicated by
/// `create_in_dir`, which does not de-duplicate.
///
/// PRTSCR-HOME: `dir` is [`ensure_capture_dir`]'s answer — the user's own capture folder
/// (SCRSHOT-DESKTOP: `Desktop` under CRISPY) — where it used to be a hardcoded `0` (the volume
/// root). The index therefore counts PER USER, which is what an operator expects: two users each
/// get their own `SCREEN0.PNG`, and neither can exhaust the other's hundred names.
///
/// SCRSHOT-DESKTOP (R60) demoted this from THE naming rule to the FALLBACK one — see
/// [`choose_name`], which prefers a clock stamp and lands here when there is no clock to stamp
/// with. Unchanged in every other respect, deliberately: a boot with no wall clock is the ordinary
/// case on both lanes today, so this is not a legacy corner but the path most captures take.
fn next_free_name(fs: &FatFs, dir: u32) -> Result<String, Refusal> {
    for n in 0..MAX_CAPTURES {
        let name = alloc::format!("SCREEN{}.PNG", n);
        match busy_retry(|| match fs.locate_in_dir(dir, &name) {
            Ok(hit) => Ok(Some(hit)),
            Err(FatError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }) {
            Ok(None) => return Ok(name),
            Ok(Some(_)) => continue,
            Err(e) => return Err(Refusal::Fat(vol_id(fs), "capture directory lookup", e)),
        }
    }
    Err(Refusal::AllTaken(vol_id(fs)))
}

// ─────────── SCRSHOT-DESKTOP (R60) — THE NAME, AND WHAT 8.3 COSTS US TO SAY IT ────────────
//
// **WHAT A MAC WRITES:** `Screenshot 2026-09-22 at 17.31.02.png`. Thirty-six characters, a space,
// two of them, and three dots. **WHAT THIS FILESYSTEM CAN WRITE:** an 8.3 short name and nothing
// else — `fs/fat.rs:118` says it outright, *"this driver's create path writes 8.3 names only (VFAT
// LFN write is out of scope)"*, and `format_83` (`fs/fat.rs:325`) enforces base `1..=8`, extension
// `0..=3`, one dot, legal short-name bytes. The Mac name is not merely long: every one of its
// separators is illegal here. So there is no clever encoding that gets us Peter's name; there is a
// MISSING FEATURE, and the honest thing is to name it rather than to pack a cryptogram.
//
// **THE MISSING FEATURE IS LFN WRITE, AND THE READ HALF IS ALREADY BUILT.** `fs/fat.rs`'s `LfnBuf`
// decodes the 0x0F-attribute VFAT component slots that precede a short entry and checksum-validates
// the run against that short name (PI-FS-3); `DirEntry::eq_name` (`fs/fat.rs:190`) then matches
// either spelling. What the FATLFN arc must add is the inverse of exactly that, and only that:
// EMIT the component slots (13 UTF-16 units each, in reverse order, `LAST_LONG_ENTRY` on the first
// written), compute the SAME one-byte checksum over the 11-byte short field the read path already
// verifies, allocate the contiguous run of slots plus the short entry in one directory extend, and
// mint a non-colliding `NAME~1` style alias for the short field. Not started here — this arc is the
// DESTINATION (R60), and a write path through the directory allocator is its own arc with its own
// crash-order argument to make. `docs/dev/OS/08_VIDEO/screenshot.md` §13 carries the same list.
//
// **WHAT WE DO IN THE MEANTIME, AND WHY IT IS NOT A CRYPTOGRAM.** Eight characters, all digits,
// `MMDDHHMM.PNG` — month, day, hour, minute, each zero-padded to two. `09221731.PNG` is the capture
// Peter's Mac would have called `Screenshot 2026-09-22 at 17.31.02.png`. Three properties, in the
// order they were weighed:
//
//  * **It sorts chronologically.** Most-significant field first, fixed width, so a directory listed
//    in name order is listed in time order — the single property a screenshot folder is actually
//    used through, and the one `SCREEN<n>` also has but only by accident of creation order.
//  * **It is READ, not decoded.** Four digit pairs in the order a human says a date is four digit
//    pairs; nobody needs this comment to know what `09221731` is. A packing like `S0922173` (base-36
//    minutes, a leading letter to dodge a rule) buys a field and costs every future reader, which is
//    the trade this tree refuses on principle.
//  * **What it drops, it drops VISIBLY.** The YEAR and the SECONDS do not fit, full stop — twelve
//    digits into eight. A name that silently dropped them while looking complete would be worse than
//    one that is obviously a truncation of a date. Two captures inside the same minute therefore
//    collide, and that is handled by FALLING BACK to the `SCREEN<n>` ladder with the reason on the
//    wire ([`choose_name`]), never by overwriting and never by lying about the minute.
//
// **AND TODAY IT IS THE FALLBACK THAT RUNS, ON BOTH LANES — MEASURED, NOT ASSUMED.** `clock::now()`
// (`clock.rs:201`) answers `None` until something seeds the anchor this boot, and nothing does:
// QEMU's hermetic slirp gateway answers no NTP (`:: SMOLNET: [sntp] 10.0.2.2 no reply — clock
// unsynced ::`) and the rMBP's flight-11 capture reads `clock=unsynced` on its own menu-bar
// witness at 43 s. There is no RTC read on either arch's boot path. So every capture today is named
// by the ladder and says `name_from=clock-unset` for it, and the clock-stamped arm lights up for
// free the day `date -s`, SNTP on a real network, or an RTC seeds the anchor.

/// SCRSHOT-DESKTOP (R60) — `MMDDHHMM.PNG` for the current wall-clock moment, or `None` when the
/// clock has never been set this boot.
///
/// `None` is not an error and is not rare: see the block above — it is what BOTH lanes read today.
/// The caller turns it into the `SCREEN<n>` ladder and a reason token, so the wire always says
/// which naming rule produced the file rather than leaving it to be inferred from the shape.
fn clock_name() -> Option<String> {
    let t = crate::clock::now()?;
    // `WallTime::is_valid` is the same predicate `clock.rs` gates its own consumers on; a moment it
    // rejects (the year-2107 saturation tail) must not become a file name whose digits are a lie.
    if !t.is_valid() {
        return None;
    }
    Some(alloc::format!(
        "{:02}{:02}{:02}{:02}.PNG",
        t.month, t.day, t.hour, t.min
    ))
}

/// SCRSHOT-DESKTOP (R60) — **the capture's file name, and the token that says which rule made it.**
///
/// Preference order, and each step's fallback is a REPORTED fact rather than a silent one:
///
///  1. `MMDDHHMM.PNG` from [`clock_name`], when the clock is set AND that name is free -> `clock`.
///  2. The clock is set but the name is taken (two captures inside one minute, which the Orin's
///     ~8 s capture can reach and the rMBP's ~70 s one cannot) -> the ladder, `clock-taken`.
///  3. No clock this boot, the ordinary state of every board today -> the ladder, `clock-unset`.
///
/// **Never overwrites**, in any branch: step 1 asks the filesystem for the candidate and takes it
/// only on `NotFound`, exactly as [`next_free_name`] does per index, and for the same reason
/// (`locate_in_dir` matches long names too, so a listing scan would miss an alias and
/// `create_in_dir` does not de-duplicate).
fn choose_name(fs: &FatFs, dir: u32) -> Result<(String, &'static str), Refusal> {
    if let Some(name) = clock_name() {
        match busy_retry(|| match fs.locate_in_dir(dir, &name) {
            Ok(hit) => Ok(Some(hit)),
            Err(FatError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }) {
            Ok(None) => return Ok((name, "clock")),
            Ok(Some(_)) => return Ok((next_free_name(fs, dir)?, "clock-taken")),
            Err(e) => return Err(Refusal::Fat(vol_id(fs), "capture directory lookup", e)),
        }
    }
    Ok((next_free_name(fs, dir)?, "clock-unset"))
}

/// PRTSCR — **capture the panel and write it to the volume root as a PNG.** Task context only.
///
/// PRTSCR2: the door. One capture at a time on the whole machine — the body is [`capture_inner`],
/// and [`IN_FLIGHT`] is released HERE, after it returns, whichever of its exits it took. A second
/// caller is told [`Refusal::InFlight`] and is never let past `next_free_name`, which is the only
/// point at which two captures could choose one name.
pub fn capture() -> Result<Shot, Refusal> {
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(Refusal::InFlight);
    }
    let verdict = capture_inner();
    IN_FLIGHT.store(false, Ordering::Release);
    verdict
}

/// The capture proper, under the door [`capture`] holds — the SAME state machine [`service`]
/// slices, driven straight to completion here.
///
/// PRTSCR-ASYNC: the synchronous form is kept, and kept as a driver of the sliced machine rather
/// than a second copy of the work, because two callers genuinely want a verdict in hand — the
/// `screenshot` shell verb (which prints a sentence to the console it was typed at) and
/// [`selftest_once`] (which reads the file back and scores it). What they lose is nothing: the wire
/// is identical, and the slice boundaries they run through are the same ones the key path parks at.
fn capture_inner() -> Result<Shot, Refusal> {
    let mut job = Job::begin()?;
    loop {
        if let Some(shot) = job.slice()? {
            return Ok(shot);
        }
    }
}

/// PRTSCR-ASYNC — one slice's worth of budget, in `arch::now_cycles()` units. See
/// [`SLICE_BUDGET_DIV`] for why the arch's hardware-wait budget is the right thing to divide.
fn slice_budget() -> u64 {
    let b = crate::arch::hw_wait_budget() / SLICE_BUDGET_DIV;
    // A calibration that has not happened yet must not produce a zero-length slice that makes no
    // progress per pass: one unit of work always runs, so the floor only bounds the LOOP.
    if b == 0 { 1 } else { b }
}

/// PRTSCR-ASYNC — a capture in progress: everything the next slice needs and nothing it does not.
///
/// `FatFs` is a handful of scalars and `FrameBuffer` is a `Copy` handle whose base is a `usize`
/// precisely so it can live in a static (`framebuffer.rs`'s `unsafe impl Send`), so parking this
/// between passes introduces no new sharing claim: the panel is read exactly as the one-shot capture
/// read it — without the panel lock, through the handle, tearing accepted (see the module note).
struct Job {
    fs: FatFs,
    panel: FrameBuffer,
    /// `SCREEN<n>.PNG`. Moved out into the [`Shot`] at the verdict.
    name: String,
    /// PRTSCR-HOME — the resolved capture directory's first cluster (the user's
    /// `Pictures/Screenshots`), and its on-disk spelling for the witness. Settled in [`Job::begin`],
    /// before a pixel is read, so a sliced capture cannot change its mind about where it is going.
    dir_cluster: u32,
    dir: String,
    width: u32,
    height: u32,
    /// The reserved length the `-> capturing` line published. Denominator of the slice witness.
    need: usize,
    /// Slices spent so far — the witness's `n`, and what [`SLICE_LINES_MAX`] caps.
    slices: u32,
    /// PRTSCR-ASYNC/UNPLUG — this mount rides the hot-unpluggable USB stick ([`usb_backed`]), so
    /// the liveness probe applies to it. `false` for the Pi's microSD and QEMU's `test-fat` image,
    /// where the probe would cost a lock per unit and can never fire.
    usb_backed: bool,
    /// PRTSCR-ASYNC/UNPLUG — `block::usb_publish_gen()` as it stood when this job opened. A DIFFERENT
    /// value means an arrival has republished the handle since — a replug, or another disk on a
    /// recycled xHCI slot — and this job's parked `FatFs` addresses a volume that is no longer there.
    vol_gen: u64,
    phase: Phase,
}

/// PRTSCR-ASYNC — which half of the capture the next unit of work belongs to. Strictly sequential:
/// the PNG cannot be written before `finish` patches the IDAT length and appends `IEND`, so the
/// whole encode precedes the whole write. That is also what keeps the on-medium bytes a valid
/// PREFIX of the final file at every slice boundary.
enum Phase {
    /// Reading the panel into the streaming encoder, `y` rows done.
    Encode { enc: PngEncoder, row: Vec<u8>, y: u32 },
    /// Writing the finished bytes to the volume in order, `done` bytes published.
    Write {
        bytes: Vec<u8>,
        done: usize,
        first: u32,
        size: u32,
        dir_lba: u64,
        dir_off: usize,
    },
    /// Transient placeholder while a unit of work owns the phase by value. Never observed by a
    /// caller: every path that takes the phase out puts one back or returns a verdict.
    Spent,
}

/// What one unit of work produced.
enum Step {
    More(Phase),
    Done(Shot),
}

impl Job {
    /// Everything that can refuse, and nothing that takes time — the order [`capture_inner`] used
    /// to run inline. The panel and the volume are settled, the name is chosen and announced, and
    /// the output buffer is reserved, all before a single pixel is read.
    fn begin() -> Result<Job, Refusal> {
        // 0. PRTSCR-HOME — **WHOSE capture is this?** First of all the refusals, ahead of the panel
        //    and ahead of the volume, because a capture that can belong to nobody must not spin up
        //    the PRTSCR-VOL ladder, must not choose a name, and must not create a directory entry.
        //    Putting the check here is what makes "zero bytes written" a structural property rather
        //    than a claim: on a no-session boot this returns before any of the three can happen.
        let plan = live_plan()?;

        // 1. The panel — through the sanctioned door, and only for the HANDLE. See the module note.
        let panel = crate::video::panel_snapshot().ok_or(Refusal::NoPanel)?;
        if !panel.is_ready() {
            return Err(Refusal::NoPanel);
        }
        let info = panel.info();
        if !matches!(info.pixel_format, PixelFormat::Rgb | PixelFormat::Bgr) {
            return Err(Refusal::NoFormat(info.pixel_format));
        }
        let (width, height) = (info.width as u32, info.height as u32);

        // 2. The volume, by the PRTSCR-VOL ladder (module note), before anything is built. This is
        //    `mount_capture_target`, NOT `mount_program_source`: rung 2 is the whole reason a
        //    read-only-boot-medium bench can capture at all, and a sliced capture must inherit it.
        let fs = mount_capture_target()?;
        // PRTSCR-ASYNC/UNPLUG: the two facts the liveness probe compares against, taken now, while
        // the volume is known good. `usb_publish_gen` is read AFTER the mount so a publish that
        // raced the mount is already reflected — a stale-low generation would refuse a live disk.
        let usb_backed = usb_backed(&fs);
        let vol_gen = crate::drivers::block::usb_publish_gen();

        // 3. PRTSCR-HOME — the user's own folder, created if absent, on THIS volume. One witness
        //    line per capture (`ensure_capture_dir` prints it), never one per slice.
        let (dir_cluster, dir) = ensure_capture_dir(&fs, &plan)?;

        // 4. A name nothing else owns, IN THAT DIRECTORY.
        // SCRSHOT-DESKTOP (R60): `choose_name`, not `next_free_name` — the clock stamp when there is
        // a clock, the ladder when there is not, and a token saying which. Same never-overwrite
        // contract in both arms; see the naming block above `clock_name`.
        let (name, name_from) = choose_name(&fs, dir_cluster)?;

        // PRTSCR2: name it on the wire BEFORE it can exist on the medium. From here every exit is
        // one of `-> OK`, a `— capture skipped` refusal, or a boot that ended inside this capture.
        // PRTSCR-ASYNC moved what that last one leaves behind: no longer always a 0-byte entry but
        // an entry SHORTER than the reserved length, because the size is published per slice.
        let need = PngEncoder::encoded_len(width, height).unwrap_or(0);
        serial_println!(
            ":: PRTSCR: {} {}x{} name_from={} -> capturing ({} bytes reserved; the verdict line follows — a boot cut before it leaves the entry short of that) ::",
            name, width, height, name_from, need
        );

        // 5. The encoder. `PngEncoder::new` reserves the whole output up front, so an allocator
        //    refusal arrives here — before any pixel is read — rather than halfway down the screen.
        let enc =
            PngEncoder::new(width, height).map_err(|e| Refusal::Encode(e, width, height, need))?;
        let mut row: Vec<u8> = Vec::new();
        if row.try_reserve_exact(width as usize * 3).is_err() {
            return Err(Refusal::Encode(PngError::OutOfMemory, width, height, need));
        }

        Ok(Job {
            fs,
            panel,
            name,
            dir_cluster,
            dir,
            width,
            height,
            need,
            slices: 0,
            usb_backed,
            vol_gen,
            phase: Phase::Encode { enc, row, y: 0 },
        })
    }

    /// PRTSCR-ASYNC/UNPLUG — is the volume this job opened still the volume it would be writing?
    ///
    /// Both of rung 2's facts, because they answer different halves of the question: the geometry
    /// publish still standing (`usb_info().is_some()` — the stick did not simply leave) AND the
    /// publish generation unchanged (no arrival has replaced it with a different disk on the same
    /// or a recycled slot). See the module note's UNPLUG section for why presence alone is not
    /// enough: the block layer's own bounds check would pass a stale LBA against a NEW disk.
    ///
    /// Cheap: two atomic loads and, for the first, one uncontended spin lock — per volume-touching
    /// step, of which a capture has a few hundred, against the ~1900 sector reads the same capture
    /// already spends on chain walks.
    fn volume_alive(&self) -> bool {
        if !self.usb_backed {
            return true;
        }
        crate::drivers::block::usb_info().is_some()
            && crate::drivers::block::usb_publish_gen() == self.vol_gen
    }

    /// PRTSCR-ASYNC — run units of work until the slice budget is spent, then hand the machine
    /// back. `Ok(None)` means "more to do"; `Ok(Some(shot))` is the finished capture.
    ///
    /// The budget is checked AFTER a unit, never before: one unit always runs, so a caller whose
    /// clock is not yet calibrated still makes progress and cannot livelock.
    fn slice(&mut self) -> Result<Option<Shot>, Refusal> {
        let start = crate::arch::now_cycles();
        let budget = slice_budget();
        loop {
            let phase = core::mem::replace(&mut self.phase, Phase::Spent);
            match self.unit(phase)? {
                Step::Done(shot) => return Ok(Some(shot)),
                Step::More(next) => self.phase = next,
            }
            if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                self.slices += 1;
                if self.slices <= SLICE_LINES_MAX {
                    let done = match &self.phase {
                        Phase::Write { done, .. } => *done,
                        _ => 0,
                    };
                    serial_println!(
                        ":: PRTSCR: slice n={} bytes={}/{} ::",
                        self.slices,
                        done,
                        self.need
                    );
                }
                return Ok(None);
            }
        }
    }

    /// One unit of work: [`SLICE_ROWS`] scanlines, or [`SLICE_WRITE`] bytes.
    fn unit(&mut self, phase: Phase) -> Result<Step, Refusal> {
        match phase {
            Phase::Encode { mut enc, mut row, mut y } => {
                let end = core::cmp::min(y.saturating_add(SLICE_ROWS), self.height);
                while y < end {
                    row.clear();
                    for x in 0..self.width as usize {
                        // `read_pixel` is the format authority (see the module note). A pixel it
                        // cannot decode cannot happen here — the layout was checked in `begin` —
                        // but an out-of-length tail row on a firmware whose reported height
                        // overruns its own buffer would answer `None`, and black is the honest
                        // answer for "this pixel is not in the framebuffer".
                        let rgb = self.panel.read_pixel(x, y as usize).unwrap_or(0);
                        row.push(((rgb >> 16) & 0xFF) as u8);
                        row.push(((rgb >> 8) & 0xFF) as u8);
                        row.push((rgb & 0xFF) as u8);
                    }
                    enc.push_row(&row)
                        .map_err(|e| Refusal::Encode(e, self.width, self.height, self.need))?;
                    y += 1;
                }
                if y < self.height {
                    return Ok(Step::More(Phase::Encode { enc, row, y }));
                }
                let bytes = enc
                    .finish()
                    .map_err(|e| Refusal::Encode(e, self.width, self.height, self.need))?;
                // PRTSCR-ASYNC/UNPLUG: the encode spent seconds of passes during which the stick
                // could have gone. This is the first volume-touching step since `begin` verified
                // the mount, so it is probed like every write below — an entry created on a disk
                // that left, or on a stranger's, is exactly the stale-handle write this refuses.
                if !self.volume_alive() {
                    return Err(Refusal::Vanished(
                        vol_id(&self.fs),
                        core::mem::take(&mut self.name),
                        0,
                        bytes.len(),
                    ));
                }
                // The entry is created only now, with the pixels already in hand: the same
                // four-step recipe `shell::fs_write` uses, minus the truncate branch, which cannot
                // apply — `next_free_name` only ever returns a name that directory does not hold.
                // PRTSCR-HOME: into `dir_cluster`, the user's own folder, where this was `0`.
                let dc = self.dir_cluster;
                let (dir_lba, dir_off) = match busy_retry(|| self.fs.create_in_dir(dc, &self.name, 0x20)) {
                    Ok((_, lba, off)) => (lba, off),
                    Err(e) => return Err(Refusal::Fat(vol_id(&self.fs), "create", e)),
                };
                Ok(Step::More(Phase::Write {
                    bytes,
                    done: 0,
                    first: 0,
                    size: 0,
                    dir_lba,
                    dir_off,
                }))
            }
            Phase::Write { bytes, mut done, mut first, mut size, dir_lba, dir_off } => {
                // PRTSCR-ASYNC/UNPLUG: probe BEFORE the write, never after — the whole point is that
                // the write is not issued. `done` is the byte count the wire reports, and it is the
                // count the medium actually holds, because `write_grow` published each slice's size
                // as it went.
                if !self.volume_alive() {
                    return Err(Refusal::Vanished(
                        vol_id(&self.fs),
                        core::mem::take(&mut self.name),
                        done,
                        bytes.len(),
                    ));
                }
                // In order, from `done` upward. `start == size` on every call after the first, so
                // no hole is ever asked for, and each call publishes the grown size + chain head —
                // which is what makes the partial file on the medium a valid PNG PREFIX rather than
                // a size that claims bytes the data does not back.
                let take = core::cmp::min(SLICE_WRITE, bytes.len() - done);
                let at = done;
                let chunk = &bytes[at..at + take];
                let (wrote, new_size, new_first) =
                    match busy_retry(|| self.fs.write_grow(first, size, dir_lba, dir_off, at as u32, chunk)) {
                        Ok(t) => t,
                        Err(e) => return Err(Refusal::Fat(vol_id(&self.fs), "write", e)),
                    };
                if wrote != take {
                    return Err(Refusal::Short(
                        vol_id(&self.fs),
                        core::mem::take(&mut self.name),
                        at + wrote,
                        bytes.len(),
                    ));
                }
                done += wrote;
                size = new_size;
                first = new_first;
                if done < bytes.len() {
                    Ok(Step::More(Phase::Write { bytes, done, first, size, dir_lba, dir_off }))
                } else {
                    Ok(Step::Done(Shot {
                        name: core::mem::take(&mut self.name),
                        width: self.width,
                        height: self.height,
                        bytes: done,
                        vol: vol_id(&self.fs),
                        dir: core::mem::take(&mut self.dir),
                        dir_cluster: self.dir_cluster,
                    }))
                }
            }
            // Unreachable: `slice` is the only caller and it always hands back a live phase.
            // Answered rather than panicked, per this module's guard-with-a-return discipline.
            Phase::Spent => Err(Refusal::Fat(vol_id(&self.fs), "slice", FatError::Io)),
        }
    }
}

// ================================ PRTSCR-ST — THE BOOT-TIME WITNESS ================================
//
// A capture is not provable from a `check` and not provable from a plain `./arroyo test`, because
// the verb needs an operator at a prompt and the key needs a finger. This drives the REAL
// [`capture`] — the same function the verb and the Print Screen edge call, never a transcription of
// it — once, at boot, and then reads back what landed ON THE MEDIUM through the block layer.
//
// **Its own knob, default OFF** (`UNAOS_PRTSCRST=1`), by the rule that gave `hcronst` a knob apart
// from `holocron` and `sdw` one apart from `sdhcblk`: *a boot that did not ask to WRITE the boot
// medium must be incapable of doing so.* Off the knob this function and its call sites vanish
// entirely, so the gate run (`UNAOS_WC=1 ./arroyo test`) and every shipped image are byte-alike.
//
// It does NOT clean up after itself, and that is deliberate — `btbond::selftest_once` sets the same
// precedent. The written file is the deliverable: `./arroyo test-fat sf` leaves a real `SCREEN0.PNG`
// in `builder/fat-sf.img`, which a host can extract with `mcopy` and decode with a real zlib. A
// kernel that says PASS is evidence; a PNG a foreign decoder opens is proof.
//
// Re-running it on an image that already holds captures is safe and is itself a demonstration: the
// free-name search takes the next index, and a hundredth run refuses rather than overwriting.

/// PRTSCR-ST — drive one real capture at boot and verify what reached the medium.
///
/// One-shot, and **the latch is taken only on a pass that reached a WRITABLE volume.** Both of the
/// states that precede one are transient and neither is a verdict:
///
///  * *No volume at all* — storage enumerates asynchronously, so the early passes have none.
///  * *A volume that vetoes writes* — on a machine whose boot medium is read-only by policy (the
///    rMBP's internal SD reader under SDHC-4c) this state is PERMANENT for the program source:
///    flight-3 proved the `BM_SUBSTITUTED` verdict pins `program_source()` to the Sdhc handle and
///    FRGUARD vetoes the global, so no amount of waiting on THAT mount ever ends. The wait is real
///    anyway because [`mount_capture_target`]'s second rung re-reads the USB registry every pass —
///    a FAT stick hot-plugged minutes after boot reaches `publish_usb_geometry`, the next
///    storage-ready pass mounts it under its own handle, and the deferred selftest runs THEN.
///
/// So both states are announced ONCE, for the log's sake, and then waited through — and the moment
/// a writable volume ends a wait, the arrival is announced too, so the log shows the deferred run
/// firing rather than a PASS appearing out of nowhere. A boot that never gets a writable volume
/// (a plain `./arroyo test`, which attaches no FAT-bearing device) leaves exactly one honest line
/// and never a false FAIL.
#[cfg(feature = "prtscrst")]
pub fn selftest_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    static SAID_NO_VOLUME: AtomicBool = AtomicBool::new(false);
    static SAID_READ_ONLY: AtomicBool = AtomicBool::new(false);
    static SAID_NO_SESSION: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    // PRTSCR-HOME: a capture belongs to a user, so this selftest cannot run before one exists. That
    // is a WAIT of exactly the shape the two below already are — announced once, never latched, and
    // it ends the moment a session opens (SO43's login screen at boot, or the `login` verb). It is
    // deliberately NOT a FAIL: on every board today there is no session at boot, and a permanent red
    // that means "the feature is correct and nothing has exercised it" is a broken instrument.
    //
    // It is also FIRST, ahead of the mount, so a no-session boot leaves one line and never churns
    // the PRTSCR-VOL ladder's decline witness on every storage-ready pass.
    if let Err(why) = live_plan() {
        if !SAID_NO_SESSION.swap(true, Ordering::Relaxed) {
            why.report();
            serial_println!(
                ":: PRTSCR-ST: SKIPPED — no user session yet, so there is no {} to capture into (theme={}); still waiting, and this selftest runs on the first pass after a login ::",
                DIR_CAPTURE.0,
                crate::video::theme::NAME
            );
        }
        return;
    }
    let fs = match mount_capture_target() {
        Ok(fs) => fs,
        Err(Refusal::NoVolume(e)) => {
            if !SAID_NO_VOLUME.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: PRTSCR-ST: no FAT volume on the program-source or USB handles ({:?}; handles={}) — still waiting; a boot that never gets one leaves the capture selftest SKIPPED ::",
                    e,
                    crate::drivers::block::source_census()
                );
            }
            return;
        }
        Err(Refusal::ReadOnly(v, _, why)) => {
            if !SAID_READ_ONLY.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: PRTSCR-ST: program source is {} (serial=0x{:08X}) and vetoes writes ({}) — still waiting for a writable volume; a FAT USB volume plugged in NOW will be adopted on arrival ::",
                    v.source,
                    v.serial,
                    why
                );
            }
            return;
        }
        // `mount_capture_target` returns only the two refusals above; anything else would be a
        // future variant, and waiting on it silently would be the dead-loop shape this selftest
        // exists to disprove — so it reports (once, like its siblings) and keeps polling.
        Err(other) => {
            static SAID_OTHER: AtomicBool = AtomicBool::new(false);
            if !SAID_OTHER.swap(true, Ordering::Relaxed) {
                other.report();
            }
            return;
        }
    };
    DONE.store(true, Ordering::Relaxed);
    if SAID_NO_VOLUME.load(Ordering::Relaxed) || SAID_READ_ONLY.load(Ordering::Relaxed) {
        let label = fs.label();
        serial_println!(
            ":: PRTSCR-ST: writable volume arrived (source={} serial=0x{:08X} label={}) — running the deferred capture selftest ::",
            fs.source_name(),
            fs.volume_fingerprint().0,
            if label.is_empty() { "-" } else { label.as_str() }
        );
    }

    let shot = match capture() {
        Ok(shot) => shot,
        Err(why) => {
            why.report();
            serial_println!(":: PRTSCR-ST: FAIL — the capture itself refused (line above) ::");
            return;
        }
    };
    shot.report_ok();

    // Read back through the block layer — the directory entry the volume actually holds, and the
    // file's own first and last bytes. Head and tail rather than the whole file: at 2880x1800 the
    // whole file is 15.5 MiB, and the three facts that matter are structural. A truncated write
    // cannot pass all three, because the size is the directory's own and the IEND is at the end.
    // PRTSCR-HOME: read back from the CAPTURE DIRECTORY the shot names, not the root — which is
    // itself part of what this selftest now proves. A file that landed anywhere else fails here.
    let (de, _, _) = match fs.locate_in_dir(shot.dir_cluster, &shot.name) {
        Ok(hit) => hit,
        Err(e) => {
            serial_println!(
                ":: PRTSCR-ST: FAIL — {} is not in {} after the write ({:?}) ::",
                shot.name, shot.dir, e
            );
            return;
        }
    };
    if de.size as usize != shot.bytes {
        serial_println!(
            ":: PRTSCR-ST: FAIL — {} is {} bytes on disk, {} were written ::",
            shot.name, de.size, shot.bytes
        );
        return;
    }
    let mut head: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    if fs.read_at(de.first_cluster(), de.size, 0, &mut head, 33).is_err()
        || fs.read_at(de.first_cluster(), de.size, de.size - 12, &mut tail, 12).is_err()
    {
        serial_println!(":: PRTSCR-ST: FAIL — {} could not be read back ::", shot.name);
        return;
    }
    let sig_ok = head.len() >= 33 && head[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let ihdr_ok = sig_ok && &head[12..16] == b"IHDR";
    let w = if ihdr_ok { u32::from_be_bytes([head[16], head[17], head[18], head[19]]) } else { 0 };
    let h = if ihdr_ok { u32::from_be_bytes([head[20], head[21], head[22], head[23]]) } else { 0 };
    let colour_ok = ihdr_ok && head[24] == 8 && head[25] == 2 && head[26] == 0 && head[28] == 0;
    let iend_ok = tail.len() == 12 && &tail[4..8] == b"IEND";
    let dims_ok = w == shot.width && h == shot.height;

    if sig_ok && ihdr_ok && colour_ok && dims_ok && iend_ok {
        serial_println!(
            ":: PRTSCR-ST: {}/{} on the medium — {} bytes, PNG signature OK, IHDR {}x{} depth 8 colour 2 non-interlaced, IEND OK -> PASS ::",
            shot.dir, shot.name, de.size, w, h
        );
    } else {
        serial_println!(
            ":: PRTSCR-ST: FAIL — {} sig={} ihdr={} colour={} dims={}x{} (want {}x{}) iend={} ::",
            shot.name, sig_ok, ihdr_ok, colour_ok, w, h, shot.width, shot.height, iend_ok
        );
    }
}

// ======================== PRTSCR-VOL — WHICH DISK, ON THE WIRE (orin 23) ========================
//
// **The gap this closes.** `mount_capture_target` above picks the capture target by two rungs, and
// until this block the wire never said which one won or what it landed on. That is fatal to the one
// experiment the ladder exists for: before USBREG (SO33) `drivers/block.rs`'s `USB_BLOCK_DEVICE` was
// ONE `Option<BlockDeviceInfo>` that every `publish_usb_geometry` OVERWROTE, so with two USB disks
// attached rung 2 mounted whichever enumerated LAST — and the verdict line
// `:: PRTSCR: SCREEN0.PNG 1920x1200 6912345 bytes -> OK ::` named no device at all. A FRIEND-DIFF
// positive control (a friend's stick attached AND the root refusing writes) could therefore produce
// a green capture that nobody could attribute to a disk. An experiment whose result cannot be read
// is not an experiment.
//
// **Two identity fields, and why these two.** `source` is the REGISTRY HANDLE the volume was reached
// through, spelled exactly as `BlockSource::name` and `block::source_census` spell it, so a
// PRTSCR line and a `handles=` census read as one vocabulary. `serial` is `BS_VolID` — the volume
// serial fixed at FORMAT time, `FatFs::volume_fingerprint`'s first field, which is the same identity
// the aarch64 UNAFS.ATR store binds its ACL rows to. The handle answers "which slot"; the serial
// answers "which disk", and only the pair survives the overwrite described above: two sticks share
// the `usb` handle and cannot share a serial.
//
// **Nothing here changes the ladder.** Not a rung, not an order, not a refusal. Every function below
// either reads a mounted `FatFs` or prints; the capture behaves exactly as it did at 600887c2.
//
// **Appended at the file TAIL on purpose.** A definition inserted higher up shifts every
// `panic::Location` below it and so changes bytes that have nothing to do with this arc; the format
// strings are the deliverable and they live at their call sites, which are edits in place.

/// PRTSCR-VOL — the identity of the volume a capture settled on.
///
/// `Copy` and two words wide, so every post-mount refusal can carry it without a clone and without
/// borrowing the `FatFs` a `Refusal` outlives.
#[derive(Clone, Copy)]
pub struct VolId {
    /// The registry handle, spelled as [`BlockSource::name`] spells it: `global`, `usb`, `sdhc`,
    /// `tegra-sd`.
    pub source: &'static str,
    /// `BS_VolID`, the serial fixed at format time — [`FatFs::volume_fingerprint`]'s first field.
    pub serial: u32,
}

/// PRTSCR-VOL — read a mounted volume's identity. Two field reads off the already-parsed BPB; no
/// I/O, so it is safe at every refusal site including the ones reached from a failing block layer.
fn vol_id(fs: &FatFs) -> VolId {
    VolId { source: fs.source_name(), serial: fs.volume_fingerprint().0 }
}

impl Shot {
    /// PRTSCR-VOL — **the capture verdict line, in one place.**
    ///
    /// The identity is appended as a SECOND `::`-delimited segment rather than folded into the
    /// first, because the first segment is what every scorer in the corpus keys on:
    /// `scorers-render9.sh` matches `/:: PRTSCR: SCREEN[0-9]+\.PNG [0-9]+x[0-9]+ [0-9]+ bytes -> OK
    /// ::/` (A17, :320 and :888) and `/:: PRTSCR: .* bytes -> OK ::/` (A36, :702), and both need
    /// `bytes -> OK ::` CONTIGUOUS. A field inserted before that `::` would have silently zeroed
    /// three census counters — a check that cannot fire, produced by a witness change.
    /// PRTSCR-HOME appends `dir=` to that SECOND segment for exactly the reason the paragraph above
    /// gives: the destination is now the interesting half of the verdict, and it still may not sit
    /// between `bytes` and `-> OK ::`. A reader gets the full on-disk path — the user's own folder,
    /// in the 8.3 spelling the medium actually holds — beside the disk it landed on.
    pub fn report_ok(&self) {
        serial_println!(
            ":: PRTSCR: {} {}x{} {} bytes -> OK :: source={} serial=0x{:08X} dir={} ::",
            self.name,
            self.width,
            self.height,
            self.bytes,
            self.vol.source,
            self.vol.serial,
            self.dir
        );
    }
}

/// PRTSCR-VOL — a rung's decline reason: `(shape code, wire token)`. The code exists only so
/// [`vol_declined`] can tell "the same decline again" from "a DIFFERENT decline" without allocating
/// or comparing strings; the token is what the operator reads.
type VolReason = (u32, &'static str);

/// Rung 1 mounted and vetoed writes (the rMBP's SDHC-4c boot medium, and the FRIEND-DIFF control).
const VOL_R_READONLY: VolReason = (1, "read-only");
/// Rung 1 did not mount at all — storage has not enumerated yet, or there is no FAT volume.
const VOL_R_NOVOLUME: VolReason = (2, "no-volume");
/// Rung 2: no USB geometry is published, so there is no stick to try.
const VOL_R_ABSENT: VolReason = (3, "absent");
/// Rung 2: geometry is published but the FAT mount through the USB handle failed.
const VOL_R_MOUNTFAIL: VolReason = (4, "mount-failed");
/// Rung 2: it mounted and then vetoed writes. Unreachable today (`BlockSource::Usb`'s `write_veto`
/// is `None`) and kept because the ladder asks the live predicate rather than assuming it.
const VOL_R_VETOED: VolReason = (5, "write-vetoed");

/// PRTSCR-VOL — the last decline SHAPE announced, `(rung1 code << 8) | rung2 code`; `0` means
/// nothing is outstanding. Reset by [`vol_mounted`], so a target that arrives and later goes away
/// announces the new decline instead of being swallowed by a latch from the previous episode.
static VOL_DECLINED: AtomicU32 = AtomicU32::new(0);

/// PRTSCR-VOL — **the witness the ladder owed: which rung took the capture, and which disk.**
///
/// Printed once per successful mount, which is once per capture (`Job::begin`) plus once per
/// `selftest_once` that reaches a writable volume — not per slice and not per service pass.
///
/// `label` comes from `FatFs::label_raw` and `fs::bootdisk::sanitize_label`, the SAME pair the home
/// soil mount uses, so `/volumes/<name>` and this line cannot come to disagree about what a disk is
/// called; an unnamed volume reads `Untitled` rather than blank. `label_raw` prefers the root
/// directory's `ATTR_VOLUME_ID` entry, so this costs a short root walk — a handful of sector reads
/// against the several hundred a capture already spends — and a read error falls back to the BPB
/// field rather than failing the mount.
///
/// `rw=` is read from the live `write_veto()` and not asserted: it reads `yes` on this line by
/// construction today, because both rungs return only after that predicate answered `None`. It is
/// printed anyway so that the day a rung grows a writable-but-restricted arm, the wire says so
/// instead of the line quietly continuing to mean something else.
fn vol_mounted(rung: u8, fs: &FatFs) {
    VOL_DECLINED.store(0, Ordering::Relaxed);
    let id = vol_id(fs);
    let (label, _altered) = crate::fs::bootdisk::sanitize_label(&fs.label_raw());
    serial_println!(
        ":: PRTSCR-VOL: rung={} source={} serial=0x{:08X} label={} rw={} -> MOUNTED ::",
        rung,
        id.source,
        id.serial,
        label,
        if fs.write_veto().is_none() { "yes" } else { "no" }
    );
}

/// PRTSCR-VOL — both rungs declined: name what each one said, beside the refusal the caller reports.
///
/// Said once per DECLINE SHAPE, not once per call, and that distinction is the whole reason the
/// codes exist: `selftest_once` calls `mount_capture_target` on every storage-ready pass while it
/// waits, so a per-call line would bury the log — while a plain "said once" latch would hide the
/// transition that matters most on the bench, `no-volume/absent` (nothing plugged in yet) becoming
/// `read-only/absent` (the boot medium is up and refuses) becoming `read-only/mount-failed` (a stick
/// arrived and its filesystem would not mount). Each of those speaks; a repeat of the same one does
/// not.
fn vol_declined(r1: VolReason, r2: VolReason) {
    let shape = (r1.0 << 8) | r2.0;
    if VOL_DECLINED.swap(shape, Ordering::Relaxed) == shape {
        return;
    }
    serial_println!(
        ":: PRTSCR-VOL: rung=none rung1={} rung2={} -> NO TARGET ::",
        r1.1,
        r2.1
    );
}

// ======================= PRTSCR-HOME — A CAPTURE BELONGS TO A USER (SCRSHOT) =======================
//
// Peter, 2026-09-13: "screenshots should be saved to a user's ~/Pictures/Screenshots", and — on the
// no-session half — "do not hack screenshots to make it work right before multi-user is in."
//
// The module note at this file's head carries the argument in full: the 8.3 table, why `Screenshots`
// is written `SCRSHOTS`, and why a capture with no session is REFUSED rather than given a shared
// folder. This block is the mechanism, appended at the TAIL for the reason PRTSCR-VOL states — a
// definition inserted higher up shifts every `panic::Location` below it.
//
// ⚠ THIS BLOCK READS `fs::users` AND WRITES NOTHING THERE. `whoami` and `home_of` are already public
// and are the whole of the interface; the user store, the session record and `/home/<name>`'s
// creation at first login all stay `fs/users.rs`'s business.

/// PRTSCR-HOME — a directory this capture path needs, in BOTH spellings the FAT layer requires:
/// `(what we LOOK UP, what we CREATE)`.
///
/// The lookup spelling is what a human wrote, and `DirEntry::eq_name` (`fs/fat.rs:190`) matches it
/// against a VFAT long name as readily as against an 8.3 short name — so a folder made on another
/// machine is found and adopted with its own spelling intact. The create spelling is only ever used
/// when the lookup came back `NotFound`, and it must satisfy `format_83` (`fs/fat.rs:325`) or
/// `create_dir` refuses it with `FatError::Unsupported`.
///
/// SCRSHOT-DESKTOP (R60) left exactly one value of this type and its two fields are now EQUAL —
/// see [`DIR_CAPTURE`], which is the whole reason the pair survives at all.
type DirName = (&'static str, &'static str);

/// SCRSHOT-DESKTOP (R60) — **the capture's leaf directory, and it is the THEME's word, not this
/// module's.** `crate::video::theme::CAPTURE_DIR` = `Desktop` under CRISPY; a Windows-shaped theme
/// will answer differently and nothing here changes when it does.
///
/// **BOTH FIELDS ARE THE SAME STRING, and that is the point of the move.** `Pictures`/`Screenshots`
/// needed a two-spelling table because `Screenshots` is eleven characters and `format_83` returns
/// `None` for it, so the folder we LOOKED UP could never be the folder we CREATED (`SCRSHOTS`).
/// `Desktop` is SEVEN characters — a legal 8.3 base exactly as written — so the lookup spelling and
/// the create spelling are one word and the alias table is GONE rather than re-pointed. What lands
/// on the medium is `DESKTOP` because `format_83` upcases what it stores, not because anything here
/// upcases it; [`path_for_home`] renders that upcase for the witness through the same fold it
/// already applies to the home's own components, so there is no second spelling to keep in sync.
///
/// The 8.3 legality of the theme's word is NOT const-asserted (a compile error is not a red run):
/// [`dir_fixture`] measures it on the wire every boot, and pointing `theme::CAPTURE_DIR` at a name
/// `format_83` refuses is that fixture's go-red.
const DIR_CAPTURE: DirName =
    (crate::video::theme::CAPTURE_DIR, crate::video::theme::CAPTURE_DIR);

/// PRTSCR-HOME — nobody is logged in. The ordinary state of every board today: the login screen does
/// not open at boot (SO43).
const WHY_NO_SESSION: &str = "no-session";
/// PRTSCR-HOME — **this image has no user store at all.** `fs/users.rs` is `#[cfg(feature = "login")]`
/// (`fs/mod.rs:105`) and `login` is not a default feature, so on an ordinary build there is not only
/// no session — there is no machinery that could ever open one, and `whoami`/`home_of` do not exist
/// to be called. A separate token from `no-session` because they are genuinely different facts and
/// an operator reading the wire deserves to know which one they have: `no-session` is answered by
/// logging in, `no-login-built` is answered by building with `UNAOS_LOGIN=1`.
const WHY_NO_LOGIN: &str = "no-login-built";
/// PRTSCR-HOME — a session names a user the store has no row for. `home_of` answers `None`; we
/// refuse rather than invent a home for a principal the store does not know.
const WHY_UNKNOWN_USER: &str = "unknown-user";
/// PRTSCR-HOME — the row's home path is not one this FAT layer can walk: empty, or deeper than
/// [`DIR_DEPTH_MAX`] leaves room for. Refused for the same reason as the other two — a destination
/// we cannot state exactly is one we must not write to.
const WHY_BAD_HOME: &str = "bad-home";

/// PRTSCR-HOME — the most directory components a capture path may have, home and the two leaves
/// together. A home is `/home/<name>` (two components) on every row this store can hold, so eight is
/// slack, not a limit anyone meets; it exists so the walk is bounded by construction rather than by
/// the shape of data read off a disk.
const DIR_DEPTH_MAX: usize = 8;

/// PRTSCR-HOME — how many bytes of the resolved path the witness line prints.
///
/// The walk itself is never truncated; only the RENDERING is. An adopted long name can be up to
/// `LNAME_MAX` (768) bytes (`fs/fat.rs:114`), and two of those would put ~1.5 KB on one serial line —
/// against this module's standing rule that a capture's witness is bounded (the SLICE_LINES_MAX
/// discipline: evidence, not a progress bar). Components are appended whole, so a clipped path
/// always ends at a component boundary and never mid-character.
const DIR_PATH_MAX: usize = 120;

/// PRTSCR-HOME — the destination a capture has been resolved to, before any volume is touched.
///
/// Holds the session's user name and that user's home path by VALUE (inline arrays, no allocation),
/// so the `fs::users` locks are taken once, at the top of [`Job::begin`], and never again for the
/// seconds a sliced capture runs. A logout mid-capture therefore cannot move a capture already in
/// flight — it lands in the folder it was opened for, which is the only answer that is not a race.
struct DirPlan {
    user: [u8; USER_NAME_MAX],
    user_len: usize,
    home: [u8; USER_HOME_MAX],
    home_len: usize,
}

/// PRTSCR-HOME — `fs::users::NAME_MAX` and `HOME_MAX`, MIRRORED, and the mirror is not a choice.
///
/// `fs/users.rs` is `#[cfg(feature = "login")]` (`fs/mod.rs:105`), so on an ordinary build the
/// module does not exist and its constants cannot be named — while [`DirPlan`] has to have a size in
/// every build. Mirroring is the only way to write the type down.
///
/// A mirror is a drift hazard, so it carries its own enforcer: the `const` block below fails the
/// BUILD — not a test, not a gate someone has to remember — the moment a `login` image's real
/// constants stop matching these. That is the whole cost of the mirror paid at compile time, on
/// exactly the configuration where the two are both visible.
const USER_NAME_MAX: usize = 8;
const USER_HOME_MAX: usize = 32;

#[cfg(feature = "login")]
const _: () = {
    assert!(USER_NAME_MAX == crate::fs::users::NAME_MAX);
    assert!(USER_HOME_MAX == crate::fs::users::HOME_MAX);
};

/// PRTSCR-HOME — the open session's user name, or `None`. The `not(login)` arm is not a stub that
/// fakes an answer: an image with no user store HAS no session, and saying `None` is the truth.
#[cfg(feature = "login")]
fn session_user(out: &mut [u8; USER_NAME_MAX]) -> Option<usize> {
    crate::fs::users::whoami(out)
}
#[cfg(not(feature = "login"))]
fn session_user(_out: &mut [u8; USER_NAME_MAX]) -> Option<usize> {
    None
}

/// PRTSCR-HOME — that user's home path from the store, or the reason there is not one.
#[cfg(feature = "login")]
fn home_lookup(name: &[u8], out: &mut [u8; USER_HOME_MAX]) -> Result<usize, Refusal> {
    crate::fs::users::home_of(name, out).ok_or(Refusal::NoSession(WHY_UNKNOWN_USER))
}
#[cfg(not(feature = "login"))]
fn home_lookup(_name: &[u8], _out: &mut [u8; USER_HOME_MAX]) -> Result<usize, Refusal> {
    Err(Refusal::NoSession(WHY_NO_LOGIN))
}

impl DirPlan {
    /// The session user's name. Always valid UTF-8: `users::name_ok` admits only `[a-z0-9_-]`.
    fn user_str(&self) -> &str {
        core::str::from_utf8(&self.user[..self.user_len]).unwrap_or("?")
    }

    /// The user's home path as the store spells it (`/home/<name>`).
    fn home_str(&self) -> &str {
        core::str::from_utf8(&self.home[..self.home_len]).unwrap_or("/")
    }
}

/// PRTSCR-HOME — **decide the destination, touching nothing.**
///
/// Pure with respect to the machine: no volume, no panel, no I/O, no globals beyond the user store
/// this reads through its own accessor. That is deliberate and it is what lets [`dir_fixture`] prove
/// both arms on a board with no filesystem attached at all.
///
/// `None` is the ordinary case and it is a REFUSAL, not a default. See the module note.
fn plan_dir(user: Option<&[u8]>) -> Result<DirPlan, Refusal> {
    let name = user.ok_or(Refusal::NoSession(WHY_NO_SESSION))?;
    if name.is_empty() || name.len() > USER_NAME_MAX {
        return Err(Refusal::NoSession(WHY_UNKNOWN_USER));
    }
    let mut home = [0u8; USER_HOME_MAX];
    // `home_of` is the store's own accessor for this and it is already public — the brief's
    // instruction and the right seam: the home a capture uses is the home the login created, read
    // from the row rather than re-derived here from a convention that could drift.
    let home_len = home_lookup(name, &mut home)?;
    if home_len == 0 {
        return Err(Refusal::NoSession(WHY_BAD_HOME));
    }
    let mut user = [0u8; USER_NAME_MAX];
    user[..name.len()].copy_from_slice(name);
    Ok(DirPlan { user, user_len: name.len(), home, home_len })
}

/// PRTSCR-HOME — [`plan_dir`] for the session that is actually open right now.
///
/// An image built WITHOUT the user store answers [`WHY_NO_LOGIN`] rather than [`WHY_NO_SESSION`] —
/// the same refusal, a different and more useful sentence, because the two are cured by different
/// things (logging in, versus building with `UNAOS_LOGIN=1`).
fn live_plan() -> Result<DirPlan, Refusal> {
    if !cfg!(feature = "login") {
        return Err(Refusal::NoSession(WHY_NO_LOGIN));
    }
    let mut nb = [0u8; USER_NAME_MAX];
    match session_user(&mut nb) {
        Some(n) => plan_dir(Some(&nb[..n])),
        None => plan_dir(None),
    }
}

/// PRTSCR-HOME — the on-disk path a home resolves to, in the spelling the CREATE path would use.
///
/// SCRSHOT-DESKTOP (R60): `/home/una` -> `HOME/UNA/DESKTOP`. Uppercase because `format_83` upcases
/// every byte it stores, so this is what a reader of the medium sees — not a presentation choice
/// made here. The LEAF goes through the SAME upcase fold as the home's own components rather than
/// being a pre-uppercased literal, which is what keeps `theme::CAPTURE_DIR` the single place the
/// destination is written down: there is no second spelling that could drift from it.
///
/// Pure and volume-free, which is the point: it is the 8.3 mapping stated as a function, so the
/// fixture can assert the mapping itself rather than assert that a directory appeared somewhere.
fn path_for_home(home: &str) -> String {
    let mut out = String::new();
    for c in home.split('/').chain(core::iter::once(DIR_CAPTURE.1)) {
        if c.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        for b in c.bytes() {
            out.push(b.to_ascii_uppercase() as char);
        }
    }
    out
}

/// PRTSCR-HOME — append one resolved component to the witness path, respecting [`DIR_PATH_MAX`].
///
/// Returns nothing and can fail at nothing: a path too long to print is still a path we walked
/// correctly, so the ONLY consequence is an elision mark on the witness line.
fn path_push(path: &mut String, comp: &str) {
    // The elision mark is its own terminator: once it is there the path is final, so a deeper
    // component cannot append after it and the mark is printed exactly once.
    if path.ends_with('…') {
        return;
    }
    if path.len() + 1 + comp.len() > DIR_PATH_MAX {
        path.push_str("/…");
        return;
    }
    if !path.is_empty() {
        path.push('/');
    }
    path.push_str(comp);
}

/// PRTSCR-HOME — **walk to the user's `Pictures/Screenshots` on `fs`, creating what is absent.**
///
/// Returns the leaf directory's first cluster (what [`next_free_name`] and `create_in_dir` take) and
/// its path as the medium actually spells it.
///
/// Three properties this is built for, in the order they matter:
///
///  * **Look up before you create, every component.** The lookup uses the LONG spelling, and
///    `eq_name` matches a VFAT long name as happily as an 8.3 short name — so a stick that already
///    carries a real `Pictures/Screenshots` is ADOPTED, with its own spelling, and nothing new is
///    written. Only a genuinely absent component is created, and then in 8.3.
///  * **`create_dir`'s crash order is the one we want and we do not second-guess it.** It zero-fills
///    and `.`/`..`-initialises the child BEFORE linking the parent, and publishes the child cluster
///    into the parent entry LAST (`fs/fat.rs:3558`) — the same shape as `write_grow`'s SAFE ORDER.
///    A boot cut inside this leaves either no entry or a valid empty directory, never an entry
///    pointing at an uninitialised cluster. That is why creating directories here needs no new
///    crash-consistency machinery: the FAT layer already owns it.
///  * **A non-directory in the way is a refusal, not a surprise.** A FILE called `Pictures` makes the
///    path unwalkable; we say so and write nothing rather than picking somewhere else to put the
///    capture. Same for a directory entry whose first cluster is 0 — a malformed or root-like entry
///    the walk cannot descend into.
///
/// Cost: one `locate_in_dir` per component on the common path (three or four bounded directory
/// walks) and nothing at all after the first capture, once the folders exist.
fn ensure_capture_dir(fs: &FatFs, plan: &DirPlan) -> Result<(u32, String), Refusal> {
    // The components, home first and the two fixed leaves last. Built into a fixed array rather than
    // chained iterators so the depth bound is visible and enforced before the walk starts.
    // The element type is `(&str, &str)` and NOT [`DirName`] on purpose: the home components are
    // borrowed out of `plan`, which outlives this walk, while the two leaves are `'static`. Letting
    // the array take the SHORTER lifetime is what lets both kinds sit in it with no cast at all.
    let mut comps: [(&str, &str); DIR_DEPTH_MAX] = [("", ""); DIR_DEPTH_MAX];
    let mut n = 0usize;
    for c in plan.home_str().split('/') {
        if c.is_empty() {
            continue;
        }
        if n + 1 >= DIR_DEPTH_MAX {
            dir_refused(plan, "", c, "home too deep");
            return Err(Refusal::NoSession(WHY_BAD_HOME));
        }
        comps[n] = (c, c);
        n += 1;
    }
    if n == 0 {
        dir_refused(plan, "", "", "empty home path");
        return Err(Refusal::NoSession(WHY_BAD_HOME));
    }
    // SCRSHOT-DESKTOP (R60): ONE leaf where PRTSCR-HOME had two. The depth guard above moved from
    // `n + 2` to `n + 1` with it — a bound that still described a two-leaf walk would have left one
    // component of slack nothing uses, which is the kind of stale arithmetic a later reader has to
    // re-derive to trust.
    comps[n] = DIR_CAPTURE;
    n += 1;

    let mut cluster = 0u32; // the volume root, on every FAT kind here
    let mut created = 0u32;
    let mut path = String::new();
    for i in 0..n {
        let (look, make) = comps[i];
        let found = busy_retry(|| match fs.locate_in_dir(cluster, look) {
            Ok(hit) => Ok(Some(hit)),
            Err(FatError::NotFound) => Ok(None),
            Err(e) => Err(e),
        });
        match found {
            Ok(Some((de, _, _))) => {
                if !de.is_dir {
                    dir_refused(plan, &path, look, "a file is in the way");
                    return Err(Refusal::Fat(
                        vol_id(fs),
                        "capture directory (a file of that name is in the way)",
                        FatError::Unsupported,
                    ));
                }
                if de.first_cluster() == 0 {
                    dir_refused(plan, &path, look, "0-cluster directory entry");
                    return Err(Refusal::Fat(
                        vol_id(fs),
                        "capture directory (malformed 0-cluster entry)",
                        FatError::BadChain,
                    ));
                }
                cluster = de.first_cluster();
                // The spelling the MEDIUM holds — the long name when there is one, else the 8.3
                // short name. So the witness reports what an operator will actually see on the disk.
                path_push(&mut path, de.name());
            }
            Ok(None) => {
                match busy_retry(|| fs.create_dir(cluster, make)) {
                    Ok((de, _, _)) => {
                        cluster = de.first_cluster();
                        created += 1;
                        path_push(&mut path, make);
                    }
                    Err(e) => {
                        dir_refused(plan, &path, make, fat_errno(e));
                        return Err(Refusal::Fat(vol_id(fs), "capture directory create", e));
                    }
                }
            }
            Err(e) => {
                dir_refused(plan, &path, look, fat_errno(e));
                return Err(Refusal::Fat(vol_id(fs), "capture directory lookup", e));
            }
        }
    }

    // THE witness: one bounded line per capture, naming the destination and why it was chosen.
    // Printed from `Job::begin`'s call, so it is once per capture and never once per slice.
    //
    // SCRSHOT-DESKTOP (R60) added `theme=` and `dir=` and did NOT add a second line, for the reason
    // this module states everywhere: a capture's witness is bounded evidence, not a feed. `theme=`
    // is the load-bearing new field — `dir=/home/una/Desktop` alone cannot be told from a
    // Windows-shaped theme that happens to agree, and the reader's question the day a second theme
    // lands is WHICH TABLE ANSWERED. `dir=` is the destination as the USER would say it (their own
    // home, the theme's word); `path=` stays what the MEDIUM spells (upcased, or an adopted long
    // name), and the two differing is the 8.3 mapping being visible rather than inferred.
    // `created=` counts components this walk made: `0` means every one of them was adopted.
    serial_println!(
        ":: PRTSCR-DIR: theme={} user={} home={} dir={}/{} path={} created={} reason=session -> RESOLVED ::",
        crate::video::theme::NAME,
        plan.user_str(),
        plan.home_str(),
        plan.home_str(),
        DIR_CAPTURE.0,
        path,
        created
    );
    Ok((cluster, path))
}

/// PRTSCR-HOME — the witness line for a destination that could NOT be resolved, naming the component
/// the walk stopped at. The caller still returns a [`Refusal`], whose own line carries the errno and
/// the volume identity; this one carries the PATH, which no `Refusal` variant has room for.
/// SCRSHOT-DESKTOP (R60): this is where `format_83`'s OWN VERDICT on the theme's word reaches the
/// wire. A `CAPTURE_DIR` the short-name decider refuses comes back from `create_dir` as
/// `FatError::Unsupported` and lands here as `at=<the word> (unsupported)` with nothing written —
/// no cluster leaked, `create_dir` validates before it allocates (`fs/fat.rs:3562`). It needs a
/// session and a writable volume to be reached, which no `./arroyo test` lane has; the volume-free
/// half of the same claim is [`dir_fixture`]'s, and it runs on every board.
fn dir_refused(plan: &DirPlan, path: &str, at: &str, why: &str) {
    serial_println!(
        ":: PRTSCR-DIR: theme={} user={} home={} path={} at={} -> REFUSED ({}) — nothing written ::",
        crate::video::theme::NAME,
        plan.user_str(),
        plan.home_str(),
        if path.is_empty() { "-" } else { path },
        if at.is_empty() { "-" } else { at },
        why
    );
}

/// PRTSCR-HOME — **the fixture, and it proves BOTH paths.**
///
/// One-shot, from [`service`], on a pass where nothing else is happening. It costs one relaxed load
/// per service pass once it has run, which is why it sits beside `PENDING`'s own load rather than
/// behind a knob: the no-session refusal is the behaviour EVERY board has today, and a gate that
/// only runs when someone remembers a knob would not have measured it once.
///
/// **Arm A — a resolved user home, and SCRSHOT-DESKTOP's destination (R60).** Three claims on one
/// line, because they fail for three different reasons and a reader must be able to tell which:
///
///  * **the DESTINATION is the one Peter ruled** — `/home/una` resolves to `HOME/UNA/DESKTOP`.
///    `FIX_WANT` is a LITERAL and is deliberately not derived from `theme::CAPTURE_DIR`: a want
///    computed from the value under test asserts nothing, and R60 named this folder by name;
///  * **the theme is named on the wire** — `theme=crispy dir=/home/una/Desktop`, so the day a
///    Windows-shaped theme lands, a log says which table answered rather than leaving the
///    destination to be matched against a guess;
///  * **the theme's word is a legal 8.3 SHORT NAME** — `legal83=`, the NECESSARY condition
///    `format_83` (`fs/fat.rs:325`) imposes: one component, no dot, base `1..=8`. It is stated here
///    and not delegated because `format_83` is private to `fs/fat.rs` and this arc does not touch
///    that file; the SUFFICIENT proof is the decider's own answer, which rides [`dir_refused`]'s
///    `-> REFUSED (unsupported name)` line on any lane that has a session and a writable volume.
///
/// Volume-free, so it runs on a board with no filesystem — which is exactly the board `./arroyo
/// test` gives us, and the reason this is the arm the wc lane can actually score.
///
/// **GO RED — point [`crate::video::theme::CAPTURE_DIR`] at a name `format_83` refuses.** Restoring
/// the pre-R60 `"Screenshots"` does both halves at once: the rendered path becomes
/// `HOME/UNA/SCREENSHOTS`, which is not `FIX_WANT`, and `legal83=false` names WHY that folder could
/// never have been created — the same eleven characters PRTSCR-HOME's alias table existed for.
/// Dropping the upcase in [`path_for_home`] reds it the other way, on the path alone.
///
/// **Arm B — no session refuses, and writes nothing.** Two assertions, because the line and the
/// silence are different claims: [`plan_dir`]`(None)` must be `Err(NoSession)` with the `no-session`
/// token, AND — when this machine genuinely has no session, the case today — the REAL [`capture`]
/// must refuse with that same variant while the capture census does not move. The second is what
/// makes "zero bytes written" measured rather than argued: `CAPTURES` is incremented only on a
/// written file, and the refusal is returned from `Job::begin`'s first statement, ahead of the name,
/// the volume and any directory entry. GO RED by giving `plan_dir` a fallback destination for
/// `None` — the shared-folder hack Peter refused — which turns the `Err` assertion false and the
/// capture into an attempted write; that is the mutation this arm exists to catch.
pub fn dir_fixture() {
    static DONE: AtomicBool = AtomicBool::new(false);
    // A relaxed load in steady state; the RMW happens exactly once, on the first pass.
    if DONE.load(Ordering::Relaxed) || DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // --- Arm A: the destination R60 named, and the 8.3 mapping it goes through -----------------
    const FIX_HOME: &str = "/home/una";
    // SCRSHOT-DESKTOP (R60): a LITERAL, not `path_for_home`'s own inputs. See the doc block.
    const FIX_WANT: &str = "HOME/UNA/DESKTOP";
    let leaf = DIR_CAPTURE.0;
    let got = path_for_home(FIX_HOME);
    // The necessary condition `format_83` imposes on a directory name, restated because that
    // function is private to `fs/fat.rs` and this arc leaves that file alone. Every clause is one
    // the decider's own body enforces: a base of 1..=8 bytes, no dot (a dotted name is a base plus
    // an extension and a DIRECTORY has neither), and no path separator (one component, or the walk
    // in `ensure_capture_dir` would be creating a folder whose name contains a `/`).
    let legal83 = !leaf.is_empty()
        && leaf.len() <= 8
        && !leaf.contains('.')
        && !leaf.contains('/')
        && !leaf.contains('\\');
    serial_println!(
        ":: PRTSCR-DIR-FIX: theme={} dir={}/{} home={} -> {} (want {}; \"{}\" is {} chars and the FAT create path is 8.3 ONLY, so legal83 is the necessary condition format_83 imposes and the decider's own verdict rides the PRTSCR-DIR REFUSED line) legal83={} -> {} ::",
        crate::video::theme::NAME,
        FIX_HOME,
        leaf,
        FIX_HOME,
        got,
        FIX_WANT,
        leaf,
        leaf.len(),
        legal83,
        if got == FIX_WANT && legal83 { "PASS" } else { "FAIL" }
    );

    // --- Arm B: no session refuses, and nothing is written ------------------------------------
    let planned_none = match plan_dir(None) {
        Err(Refusal::NoSession(w)) => w == WHY_NO_SESSION,
        _ => false,
    };
    let (_, before, _) = census();
    // Only drive a REAL capture when this machine has no session. If one is open — LOGINBOOT's
    // screen has landed and someone logged in — a boot-time fixture must not help itself to the
    // operator's panel and write a file nobody asked for, so the live leg is skipped and said to be
    // skipped. The pure assertion above still holds the refusal contract in that case.
    let live = live_plan();
    let (live_refused, live_token) = match live {
        Err(Refusal::NoSession(why)) => match capture() {
            Err(Refusal::NoSession(w2)) => (true, w2),
            Err(_) => (false, why),
            Ok(_) => (false, why),
        },
        Err(_) => (false, "other-refusal"),
        Ok(_) => (true, "session-open-live-leg-skipped"),
    };
    let (_, after, _) = census();
    let wrote_nothing = after == before;
    serial_println!(
        ":: PRTSCR-DIR-FIX: no session -> REFUSED reason={} plan_none={} live={} captures {}->{} bytes=0 (no name chosen, no volume touched, no directory entry made) -> {} ::",
        WHY_NO_SESSION,
        planned_none,
        live_token,
        before,
        after,
        if planned_none && live_refused && wrote_nothing { "PASS" } else { "FAIL" }
    );
}
