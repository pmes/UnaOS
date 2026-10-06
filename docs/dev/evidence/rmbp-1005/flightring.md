# FLIGHTRING (rmbp-ledger B400, R88) — the console reads a ROLLING ring of the boot, on both arches

Cut from exec-rmbp-merge17 05debd26. Wire: `flight24/f24-boots.log`, `flight25/f25-boots.log` (`[console]` lines).

## Finding (from the wire)
- Card 3's console opened at 13:04:49 reads `[console] prefill win=5 lines=16 … ring_bytes=262144 ring_full=1`:
  the x86 `flight_recorder` ring is a FIXED 256 KiB that keeps the FIRST 256 KiB (`RING_CAP`, append stops when
  full), so the prefill painted the ring's END — text from early in the boot — not the live tail. The other three
  opens (`ring_bytes=192244/111540/198732 ring_full=0`) were early enough to be right by luck.
- aarch64 has no recorder at all: `console_prefill` prints `ring=none` (the `cfg(not(x86_64))` arm).
- The console window has no scroll-back: its wheel falls to the shell's SCROLLBACK (main.rs) and its Shift+PgUp
  `Action`s reach nobody while the console holds focus.

## Seam
Kernel — shared-core. ONE ring, fed from each arch's one serial print seam (R79: no second store):
`boot_ring.rs` (new, arch-neutral) owns the ring, its staging and its readers; `flight_recorder.rs` (x86) keeps
only the `UNAOS.LOG` flush and reads the ring through `boot_ring::file_image`; the console prefill
(`loginfurn.rs`) and the scroll-back (`flightring.rs`, new) read `boot_ring::tail`.

## Milestones
- M1 — the ring rolls: a 64 KiB PINNED head (the boot's first bytes, never evicted) + a ROLLING ring (newest N KiB,
  always present). A 192 KiB static bootstrap rolls from the first byte; at the first main-loop service (heap up,
  `serial_ring::mirror_service`, both arches) the rolling part moves to a heap buffer sized from memory
  (heap free ÷ 64, floor 1 MiB, ceiling 4 MiB) — one line `[flight] ring rolling_kib=<n> pinned_head_kib=64 …`.
  `UNAOS.LOG` keeps its fixed reservation (no new FAT mutation): head + a gap marker + the newest bytes that fit;
  past the reservation the re-flush is time-throttled (30 s) instead of stopping.
- M2 — the prefill shows the LIVE tail; where the head and the rolling part do not join, a marker row
  `:: FLIGHTRING: ---- pinned head ends; <g> bytes rolled out; rolling ring begins ---- ::` sits between them.
  `[console] prefill … ring_bytes=<total> wrapped=<0|1> tail_live=1 …`.
- M3 — scroll-back: the wheel over the console window and Shift+PgUp/PgDn, Cmd/Ctrl+Home/End while the console
  holds focus re-render the window from the ring (clear + replay under one present), through the whole ring;
  back at the bottom it is the live tail again. `[console] scroll back=<lines> top=<0|1>`.
- M4 — aarch64: `arch/aarch64/serial.rs::_print` feeds the same ring (same-line fold); the Pi's prefill reads it.
- M5 — `tests flightring`: a heap ring of the LIVE shape (64 KiB head, N KiB rolling) is written past N (the
  live boot log is not overwritten by filler), its tail and marker checked; a token printed through
  `serial_println!` must be the live ring's last line.

## Witness (the next flight reads)
`[flight] ring rolling_kib=<n> pinned_head_kib=64` once per boot (both arches);
`[console] prefill … wrapped=… tail_live=1` when the console opens late; `[console] scroll back=<n>` on a wheel;
`tests flightring` -> `:: FLIGHTRING: rolling_kib=<n> pinned_kib=64 tail_live=1 prefill_lines=<n> wrapped=1 -> PASS ::`.

## Owed
- A filter field (`[usbnet]`) is not built (M4-if-cheap in the brief; the console window has no text field).
- While scrolled back, a live line still lands at the window's bottom row (the next scroll re-renders cleanly);
  a frozen view is owed. PageUp/PageDown bare have no decoded byte on either HID path: the Shift/Cmd chords are the keys.
- The GUI_ACTIVE gate across logout/login is verified by reading (`BARE_DETACHED` is never cleared; the gate
  re-opens on the next console mint) — no new code.
