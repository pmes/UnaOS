# Screen capture — PRTSCR

Print Screen, and the `screenshot` verb, write the panel to a PNG **on the logged-in user's own
Desktop** — `/home/<name>/Desktop` under the CRISPY theme — and refuse, writing nothing, when nobody
is logged in (§12, SCRSHOT-DESKTOP/R60; the folder was `Pictures/Screenshots` from 2026-09-13 and the
volume root before that). This document states what the mechanism is, where each piece runs, and what
it refuses.

Source: [`video/png.rs`](../../../../unaos/crates/kernel/src/video/png.rs) (the encoder),
[`video/prtscr.rs`](../../../../unaos/crates/kernel/src/video/prtscr.rs) (capture, naming, write,
key flag), the `screenshot` arm in
[`shell.rs`](../../../../unaos/crates/kernel/src/shell.rs), the verb's table entry in
[`midden_core`](../../../../unaos/libs/sys/midden_core/src/lib.rs), and the two HID decoders'
press-edge hooks in [`drivers/xhci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/xhci/mod.rs)
and [`drivers/ehci/mod.rs`](../../../../unaos/crates/kernel/src/drivers/ehci/mod.rs).

## 1. Two ways in, one mechanism

| Entry | Where it runs | What it does |
|---|---|---|
| `screenshot` verb | the shell task (`dispatch_command`), interrupts enabled | calls `prtscr::capture()` directly and prints the outcome on both sinks |
| Print Screen key | the HID decoders' press edge, inside a driver lock | calls `prtscr::request()` — **one atomic store and one counter**, nothing else |
| ⌘⇧3 / ⌘⇧4 (PRTSCRCHORD) | the same press edge, one line below the 0x46 test | the same `prtscr::request()`; the witness names which chord fired |
| — | the device-service pass, beside `fat::probe_once()` | `prtscr::service()` sees the flag and performs the capture |

**PRTSCRCHORD (2026-09-02).** The rMBP's internal Apple keyboard has no Print Screen key and never
puts usage 0x46 on the wire, so the metal-proven capture was bound to a key the bench machine cannot
press. The Apple chords ⌘⇧3 (GUI+Shift+3) and ⌘⇧4 (GUI+Shift+4) are bound to the same whole-screen
capture through `xhci::hid_screenshot_chord_edge(cur_keys, prev_keys, modifiers)`, asked by both
decoders as an `else if` directly after `hid_print_screen_edge` (so a report carrying both arms
exactly once). The judgement has to live at the decoder: a chord is a modifier byte plus a usage,
and the boot report is the only place both are in one hand — `pal::Event::Key` carries no modifier.
Left and right GUI (`HID_MOD_GUI` = 0x88) and Shift (`HID_MOD_SHIFT` = 0x22) count alike; the digit
is diffed against the previous report exactly as 0x46 and the lock keys are, so a held chord arms one
capture, not one per restated report. **The chord types nothing:** `hid_key_ascii` already folds
every GUI-held usage to 0, so no `Key('3')`/`Key('#')` is delivered — the same suppression 0x46 gets
from its `(0, 0)` table entry, reached through the modifier instead. The release fold ignores GUI on
purpose and emits a lone `KeyUp('#')`/`KeyUp('$')` when the digit lifts — the documented
"spurious release, safe" case. ⌘⇧4 is region-select on macOS; region-select is reserved here (no
pointer-driven selector exists) and the chord is honoured as a whole-screen capture rather than
ignored.

The split is not stylistic. The Print Screen edge is decoded inside `service_ehci_hid()` while
`EHCI_HID` is held (or inside the xHCI event pass while its loan is held), and the writable FAT
volume rides USB mass storage serviced from that same pass. A filesystem write issued from in there
would contend the storage loan *from inside the input pass* and hold the internal keyboard and
trackpad hostage for the whole multi-second duration of an encode. This is the identical argument
`holocron`'s call site makes for its own deferred write, and `prtscr::service()` sits at the same
three storage-ready passes for the same reason: which pass a given build reaches depends on its
knobs. Idle cost is one relaxed atomic load per pass.

## 2. Why the key had to be hooked at the decoder, not in the routing chain

`pal::Event::Key` carries a single `u8` and no modifier field. HID usage `0x46` (PrintScreen)
produces no character, so `HID_SCANCODE_TO_ASCII` maps it to `(0, 0)` and both decoders skip a zero
fold — the key never becomes an event and is invisible above the driver. There were exactly two
seams:

1. **Give `0x46` a byte in the fold.** Rejected. The table's own doc block explains at length that
   the byte space is full of collisions a consumer cannot disambiguate (that is why the Ctrl-folds
   at `0x08/0x09/0x0A/0x0D` and the arrows at `0x1C..0x1F` were carved out), and a screenshot key
   that occasionally forges a Tab or a Backspace is worse than no screenshot key.
2. **The decoders' usage-level press edge**, beside `HID_LOCK_KEYS` and the Ctrl+Alt+B pairing
   chord. Taken. It is the established precedent for "a non-character key triggers a kernel action",
   and — decisively — **it is the only seam that reaches the rMBP's internal keyboard**, which rides
   EHCI rather than xHCI.

Both decoders are hooked, through one shared edge predicate (`xhci::hid_print_screen_edge`), because
the shared-table invariant is "a key is a key whichever controller carried it". There is no PS/2
path in this kernel to hook — no i8042 code, no `0x60`/`0x64` access, no E0-prefix decoding — so the
PS/2 `E0 37` scancode has no site here.

## 3. Reading the panel

`prtscr::capture()` takes the panel through `video::panel_snapshot()`, the sanctioned paint-path
door. That returns a `FrameBuffer` — a `Copy` **handle** (base, length, geometry), not a guard — so
the lock is released the moment the snapshot returns and **no panel lock is held for any of the
millions of pixel reads that follow**. Holding `WRITER` across a multi-second encode would be the
WEDGE-8 shape this kernel spent three arcs eliminating.

The cost is stated honestly: the compositor may paint between two of our scanlines, so a capture
taken while the screen is moving can tear. For a screenshot that is cosmetic.

Pixels come from `FrameBuffer::read_pixel`, which is the format authority — the documented inverse
of `put_pixel`, decoding `PixelFormat::Rgb` and `PixelFormat::Bgr` from the `FrameBufferInfo` the
firmware reported (UEFI GOP on x86, VideoCore mailbox on the Pi). Nothing here assumes a byte order.
A layout with no colour inverse (`U8` greyscale averaging is lossy) is **refused**, not guessed at.

## 4. The PNG

Stored (BTYPE=00) deflate blocks inside a real zlib stream: `RFC 1951` §3.2.4 makes a 5-byte header
plus literal bytes a legal deflate block, and a zlib stream made entirely of them is a legal zlib
stream every decoder accepts. So there is no compression dependency and no compressor working set —
only the two checksums the containers demand (CRC-32 per PNG chunk, Adler-32 over the zlib payload).
Colour type 2 (truecolour RGB8), bit depth 8, filter 0 (None) on every scanline: filters buy
compression ratio, and with stored blocks there is nothing to buy.

Size is `1 + width*3` per scanline plus 5 bytes per 65535 — 3,073,098 bytes at 1280x800 and
15,555,053 at 2880x1800. Compression is explicitly out of scope; it would drop in behind the same
streaming API without the capture path noticing.

**The encoder owns exactly one buffer, `try_reserve_exact`ed to its final size before any pixel is
read.** Reading the whole frame and then encoding it would hold a ~20 MiB frame copy *and* a ~15 MiB
output at once. Instead the caller pushes one scanline at a time, so the frame copy shrinks to one
row, there is one allocation and no doubling spike, and an out-of-memory answer arrives *before* the
first pixel rather than halfway down the screen. Every stored block's length is known before its
header is written because the total raw size is fixed at construction — which is what makes
single-buffer streaming possible at all.

`video/png.rs` has **no kernel dependencies**, only `alloc`. That is deliberate: it makes the
encoder a pure function of its inputs, so a host harness can `#[path]`-include the same source file
the kernel compiles and decode the result with a real zlib.

## 5. Naming, and the no-overwrite rule

A Mac writes `Screenshot 2026-09-22 at 17.31.02.png`. **We cannot** — `fs/fat.rs:118` writes 8.3
short names only and every separator in that name is illegal here. §13 states what the FATLFN arc
must add to earn it. Until then `prtscr::choose_name` picks, in order:

| rule | name | when | `name_from=` |
|---|---|---|---|
| clock stamp | `MMDDHHMM.PNG` — e.g. `09221731.PNG` | the wall clock is set **and** that name is free | `clock` |
| ladder | `SCREEN0.PNG` .. `SCREEN99.PNG`, first free index | the clock name is already taken (two captures inside one minute) | `clock-taken` |
| ladder | the same | `clock::now()` is `None` — no wall clock this boot | `clock-unset` |

**`MMDDHHMM` and not a packing.** Four zero-padded digit pairs, most-significant first, so a
directory listed in name order is listed in **time order** — the one property a screenshot folder is
actually used through. Nobody needs a comment to read `09221731`. The year and the seconds do not fit
(twelve digits into eight) and are dropped **visibly**, rather than smuggled into a base-36
cryptogram that buys one field and costs every future reader.

**Today the ladder is what runs, on both lanes.** Measured, not assumed: `clock::now()`
(`clock.rs:201`) answers `None` until something seeds the anchor, and nothing does — QEMU's hermetic
slirp gateway answers no NTP (`:: SMOLNET: [sntp] 10.0.2.2 no reply — clock unsynced ::`) and the
rMBP's flight-11 capture reads `clock=unsynced` on its own menu-bar witness at 43 s. There is no RTC
read on either arch's boot path. The clock arm lights up for free the day `date -s`, SNTP on a real
network, or an RTC seeds it.

Both rules land **in the capture directory** (§12 — `Pictures/Screenshots` until 2026-09-22, the
volume root until 2026-09-13), so the ladder index counts **per user**: two users each get their own
`SCREEN0.PNG`, and neither can exhaust the other's hundred names. **An existing capture is never
overwritten** in either arm: the search asks `locate_in_dir(dir, name)` per candidate and takes the
first `NotFound`; when all hundred ladder names are present the capture refuses and says so rather
than wrapping around onto `SCREEN0.PNG`. The lookup goes through the filesystem rather than a
directory listing because `locate_in_dir` matches on both the 8.3 short name and any long name, and
`create_in_dir` does not de-duplicate. Every name this module mints is 8.3-clean, so none of them
needs a long-name entry.

## 6. Writing, and `Busy`

The write is the same four-step recipe `shell::fs_write` uses, minus the truncate branch (which
cannot apply — the name is known absent): `mount_capture_target` (§6.1) → `create_in_dir`
→ `write_grow(0, 0, dir_lba, dir_off, 0, bytes)`.

### 6.1 Where a capture may land — PRTSCR-VOL, the two-rung target ladder

A capture wants "a writable FAT volume the operator can carry away", which is **not** the question
`mount_program_source` answers ("the volume this system is bound to"). `prtscr::mount_capture_target`
tries, in order:

1. **The program source**, when `write_veto()` is `None`. Every boot whose program volume is
   writable — QEMU `test-fat`, a stick-booted x86 machine, the Pi's microSD — behaves exactly as
   before; rung 2 is never consulted.
2. **The dedicated USB mass-storage handle** (`BlockSource::Usb`), when rung 1 is read-only or
   absent. `publish_usb_geometry` populates that handle on *every* stick arrival — boot-time or
   hot-plug (Boot AI-2 proved on metal that a hot-plugged card reaches the FAT layer) — and the
   handle's read/write paths (`read_block_usb`/`write_block_usb` and their multi-sector twins)
   bypass the backend selector entirely. The ladder re-reads the registry on every call, so there is
   no cache to invalidate: the first storage-ready pass after the stick enumerates sees it.

Rung 2 does **not** weaken FRGUARD. The hazard FRGUARD closes is a write aimed at the *boot volume*
silently landing on whatever claimed the global slot (`default_writable()`'s `BM_SUBSTITUTED`
refusal, born of Boot AI-2's misdirected `/UNAOS.LOG`). Rung 2 aims at the stick *by name*, under
its own handle — the operator's carry-away medium, which is exactly where a screenshot belongs —
and the global slot's veto stands untouched, as does SDHC-4c's read-only policy on the internal
reader and the reserved flight-recorder extent (which no file verb can name, PNGs included).

Only when *both* rungs decline does the capture refuse, and the refusal describes rung 1 — the more
informative failure — while stating that no writable USB volume was attached either (§7).

`FatError::Busy` is **not a failure**. It is the block layer refusing to *wait* for a loan it could
not take instantly — under WEDGE-8 that is the fix working (`drivers/block.rs`: "a NORMAL,
RETRYABLE outcome — not a wedge verdict"; `07_USB_STORAGE/usb_xhci.md` §32.3). Every FAT call here
goes through `busy_retry`, which retries up to 64 times inside the hardware-handshake budget,
`hlt`ing between attempts **only while unmasked** (halting with interrupts off is the WEDGE-8 death,
and the block layer's own claim site makes the same distinction). Only an expired budget becomes
`-EAGAIN` for the operator.

## 7. Witness lines

Success, one line:

```
:: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::
```

Refusals, one honest line each, in the WINX-8 discipline — a guard with a `return`, never a panic,
never silence, and each names *what was inspected* rather than only what was missing:

```
:: PRTSCR: no panel attached (or the panel lock was contended while masked) — capture skipped ::
:: PRTSCR: panel layout U8 has no RGB inverse — capture skipped ::
:: PRTSCR: no FAT volume on the program-source or USB handles (NoDisk; handles=...) — capture skipped ::
:: PRTSCR: REFUSED READ-ONLY (source=... label=... reason=...) — no writable USB volume attached either — capture skipped ::
:: PRTSCR: SCREEN0.PNG..SCREEN99.PNG all present at the volume root — capture skipped (nothing overwritten) ::
:: PRTSCR: encoder declined (OutOfMemory) for 2880x1800 needing 15555053 bytes — capture skipped ::
:: PRTSCR: create failed -EAGAIN (Busy; handles=...) — capture skipped ::
:: PRTSCR: SCREEN0.PNG short write 512 of 3073098 bytes — capture INCOMPLETE ::
```

The key edge announces itself before the deferral, so a metal capture can tell "the key never
arrived" from "the key arrived and the capture refused":

```
:: PRTSCR: PrintScreen (HID 0x46) down on <controller> -> capture armed ::
:: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on <controller> -> capture armed ::
:: PRTSCR: [prtscr] chord=cmd-shift-4 (GUI+Shift+digit) down on <controller> -> capture armed ::
```

`chord=cmd-shift-3` / `chord=cmd-shift-4` is the PRTSCRCHORD token — `awk '/chord=cmd-shift/'` on
a serial capture answers "did the chord arrive" independently of whether the capture then succeeded.

`prtscr::census()` returns `(requests, captures, refusals)` for the same reason — a key press that
produced no file and a key press that never happened are different failures.

Two sinks, two lengths (FATVERB's rule): the console gets one sentence, serial gets the census.
The panel clips at 128-180 columns and the census tail is the whole diagnostic.

## 8. The boot-time witness — `UNAOS_PRTSCRST=1`

Nothing on a headless x86 boot can drive a shell verb: there is no serial RX on x86, no autoexec, and
no `UNAOS_K8_SCRIPT` analogue (the aarch64 `kernel8-test` typist has no x86 twin). So the capture
gets a witness in the shape this tree already uses for unattended writes.

`prtscr::selftest_once()` drives the **real** `capture()` — the same function the verb and the key
call, never a transcription — once at boot, then reads back what landed on the medium through the
block layer and checks the directory size against what was written, the PNG signature, the IHDR
geometry and colour type, and the trailing IEND. Head and tail rather than the whole file: at
2880x1800 the file is 15.5 MiB and the three facts that matter are structural, and a truncated write
cannot pass all three.

**Its own knob, default OFF**, by the rule that gave `hcronst` one apart from `holocron` and `sdw`
one apart from `sdhcblk`: *a boot that did not ask to WRITE the boot medium must be incapable of
doing so.* Off the knob the function and its call sites vanish, so the gate run and every shipped
image are byte-alike. The capture mechanism itself — the verb, the key — is ungated and always
present; only this unattended write is behind the knob. It does not clean up after itself
(`btbond::selftest_once`'s precedent): the written file **is** the deliverable.

It latches only on a pass that reached a **writable** volume. Two states precede one and neither is
a verdict: no volume at all (storage enumerates asynchronously), and a volume that vetoes writes.
Both are announced once and waited through — the wait polls `mount_capture_target` (§6.1) on every
storage-ready pass, so a writable volume that arrives *late* is adopted, however late — and the
arrival itself is announced, so the log shows the deferred run firing rather than a PASS appearing
out of nowhere:

```
:: PRTSCR-ST: program source is sdhc and vetoes writes (...) — still waiting for a writable volume; a FAT USB volume plugged in NOW will be adopted on arrival ::
:: PRTSCR-ST: writable volume arrived (source=usb label=...) — running the deferred capture selftest ::
```

Latching on the veto is exactly what the first run of this witness did, and it gave up about a
second before the writable volume arrived.

## 8.1 Metal — the rMBP's boot medium can never take a capture, and that is policy

Flight-3 (`UNAOS_PRTSCRST=1`, 2026-08) settled the metal state of this bench:

- **The boot SD is read-only, permanently.** The 2012 rMBP boots from the internal SD reader, which
  SDHC-4c mounts read-only — only the reserved flight-recorder extent admits a write, no file verb
  can name it, and PNGs are explicitly out of its scope. Neither the boot self-test PNG nor a
  keypress PNG can *ever* land on this machine's boot medium. That policy is deliberate and stands.
- **Waiting on the program source alone was a dead loop by construction — the flight-3 bug.** On
  this bench FRGUARD's boot-medium verdict is `BM_SUBSTITUTED` (boot volume positively located on
  the Sdhc handle), which (a) pins `program_source()` to the read-only Sdhc handle on every call,
  and (b) makes `default_writable()` veto the global slot — so even a stick that claimed the global
  was unwritable through it. The single veto line at 26 414 ms followed by 19 minutes of silence was
  that loop: re-polling a mount whose answer could not change.
- **Capture on this bench requires a second, writable volume — a FAT USB stick — and hot-plug is
  enough.** With PRTSCR-VOL (§6.1) the stick is reached under its own `Usb` handle the pass after
  `publish_usb_geometry` runs, whether it was present at boot or plugged minutes later. On arrival:
  a pending `PRTSCR-ST` prints the `writable volume arrived` line and runs its deferred self-test
  against the stick; the `screenshot` verb and the Print Screen key write `SCREEN<n>.PNG` to the
  stick's root. Unplugging the stick retracts the handle (USB-UNPLUG), and the refusal lines return.
- **What QEMU proves and what only metal can.** QEMU (`test-fat sf`) proves the deferred sequence —
  veto announced, writable volume arrives later, deferred selftest runs and PASSes — but there the
  writable volume arrives as the *program source* (rung 1: the boot stick claims the global with a
  `BM_MATCH` verdict). The `BM_SUBSTITUTED` + hot-plug case — rung 2 adopting a stick the FRGUARD
  verdict keeps out of the program source — cannot be staged in QEMU (the emulated internal card is
  a raw pattern image, so the verdict can never be SUBSTITUTED there) and is next flight's bench
  protocol: boot to the veto line, plug a FAT stick in the other port, expect `writable volume
  arrived (source=usb ...)` then the PASS line.

## 9. Verification status

- **Encoder, host-side.** A harness `#[path]`-includes `video/png.rs` itself — the same source the
  kernel compiles. CRC-32 and Adler-32 match their published check values; python's real `zlib`
  decompresses the output; every chunk CRC validates; decoded pixels are byte-identical to the
  source rows, in a single-block case and in an eleven-stored-block case; the refusal paths
  (`EmptyImage`, `BadRowLength`, `RowCountMismatch`) answer as specified; `encoded_len` is exact.

- **Whole chain, in QEMU — a real PNG on a real FAT volume, through the deferred wait.**
  `UNAOS_WC=1 UNAOS_PRTSCRST=1 ./arroyo test-fat sf 240` prints (2026-08-27, PRTSCR-VOL):

  ```
  :: PRTSCR-ST: program source is sdhc and vetoes writes (...) — still waiting for a writable volume; a FAT USB volume plugged in NOW will be adopted on arrival ::
  :: PRTSCR-ST: writable volume arrived (source=global label=UNAOS) — running the deferred capture selftest ::
  :: PRTSCR-ST: program source is sdhc and vetoes writes (...) — still waiting for a writable volume ::
  :: PRTSCR: SCREEN0.PNG 1280x800 -> capturing (3073098 bytes reserved; the verdict line follows — a boot cut before it leaves the entry at 0 bytes) ::
  :: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::
  :: PRTSCR-ST: SCREEN0.PNG on the medium — 3073098 bytes, PNG signature OK, IHDR 1280x800 depth 8 colour 2 non-interlaced, IEND OK -> PASS ::
  ```

  The first two lines ARE the deferred sequence: the early storage-ready passes see only the
  emulated internal card (read-only), the veto is announced once, and the selftest keeps polling
  until the stick's later enumeration ends the wait — then announces the arrival and runs. In QEMU
  the arriving volume is rung 1 (`source=global`, `BM_MATCH`); on the rMBP it will be rung 2
  (`source=usb`), per §8.1.

  `mcopy -i builder/fat-sf.img ::SCREEN0.PNG` pulls the file off the image host-side. `file` reports
  `PNG image data, 1280 x 800, 8-bit/color RGB, non-interlaced`; python's `zlib` decompresses the
  IDAT (which validates all 47 stored blocks and the Adler-32); every chunk CRC checks; the raw
  stream is exactly `800 * (1 + 1280*3)` bytes with a zero filter byte on every scanline. 3,073,098
  is byte-for-byte what the host harness's `encoded_len(1280, 800)` predicts.

  **The pixels are the real screen, decoded in the right channel order.** The dominant colour is
  `(45, 43, 85)` — `wm::DESKTOP_BG`, `0x2D2B55` — and `(30, 30, 30)` is `video::PANEL_BG`,
  `0x1E1E1E`. A swapped decode would have rendered the desktop as `0x552B2D`.

- **The KEY, in QEMU — through the real HID path, with the witness knob OFF.** QEMU's `send-key`
  delivers the `print` qcode to the emulated `usb-kbd`, which the builder attaches to `ehci.0` — so
  the report is decoded by `decode_boot_keyboard`, the same function that decodes the rMBP's
  internal keyboard. Two presses, 30 s apart, over QMP on a `test-fat sf` run carrying **no**
  `prtscrst` (so the key was the only possible trigger). Since PRTSCR2 every capture is NAMED on
  the wire before a pixel is read — the `-> capturing` line — so the sequence reads (the PRTSCRGATE
  run, `UNAOS_WC=1 ./arroyo test-fat sf 200`, two chords 45 s apart):

  ```
  :: PRTSCR: PrintScreen (HID 0x46) down on EHCI -> capture armed ::
  :: PRTSCR: SCREEN0.PNG 1280x800 -> capturing (3073098 bytes reserved; the verdict line follows — a boot cut before it leaves the entry at 0 bytes) ::
  :: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::
  :: PRTSCR: PrintScreen (HID 0x46) down on EHCI -> capture armed ::
  :: PRTSCR: SCREEN1.PNG 1280x800 -> capturing (3073098 bytes reserved; the verdict line follows — a boot cut before it leaves the entry at 0 bytes) ::
  :: PRTSCR: SCREEN1.PNG 1280x800 3073098 bytes -> OK ::
  ```

  How to read the wire: count `-> capturing` lines against their verdicts (`-> OK`, a
  `— capture skipped` refusal, or `refused — capture in flight`). **A `capturing` line with no
  partner verdict = the boot ended inside the write; the card's 0-byte file carries that name**
  (`write_grow` publishes the directory size last, so the entry `create_in_dir` made stays at 0
  bytes — the interrupted-write signature, not a mystery file). A second press that lands while a
  capture is in flight is decoded from inside that capture's own storage drain and re-arms the
  request; it runs as the next capture, never as a second concurrent one (the `IN_FLIGHT` door).

  Two files, two indices — the no-overwrite rule doing its job. Both extract from the image and
  decode cleanly (real zlib, all chunk CRCs, `800 * (1 + 1280*3)` raw bytes each). This exercises
  the whole chain the metal will: HID report → press-edge diff → `request()` → the flag → the
  device-service pass → `capture()` → the FAT write.

- **Refusal path, in QEMU.** `UNAOS_PRTSCRST=1 ./arroyo test` attaches no FAT-bearing device and
  prints the honest lines, once each, naming the handle census it inspected — first the read-only
  internal SD reader the ladder falls back to, then `NotFat` on the raw pattern image.

- **Gate.** `UNAOS_WC=1 ./arroyo test` green with `wc` in the `⚡ kernel features:` banner;
  `./arroyo check` green on both arches, with `prtscrst` added to the `x86-all` and `arm-pi`
  cfg-coverage legs so the knob-on build is type-checked too.

- **The CHORDS, in QEMU — PRTSCRCHORD, through the same real HID path (2026-09-02).** QMP
  `send-key` with `meta_l`, `shift` and `3` (then `4`) in ONE command, so the emulated `usb-kbd` on
  `ehci.0` emits a boot report with modifiers `0x0A` (LGUI|LShift) and usage `0x20` (then `0x21`) —
  decoded by `decode_boot_keyboard`, the rMBP's internal-keyboard decoder. Two chords 45 s apart on
  `UNAOS_WC=1 ./arroyo test-fat sf 200` with no `prtscrst` (the chord was the only possible trigger):

  ```
  :: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::
  :: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::
  :: PRTSCR: [prtscr] chord=cmd-shift-4 (GUI+Shift+digit) down on EHCI -> capture armed ::
  :: PRTSCR: SCREEN1.PNG 1280x800 3073098 bytes -> OK ::
  ```

  The only `EHCI-HID:` key lines in the whole log are `KEYUP: '#' (scancode 0x20)` and
  `KEYUP: '$' (scancode 0x21)` — no `KEY:` press was delivered for either digit, which is the
  no-keystroke property, and the two releases are the documented safe spurious `KeyUp`. Both files
  pulled off `builder/fat-sf.img` with `mcopy`: `PNG image data, 1280 x 800, 8-bit/color RGB`,
  every chunk CRC valid, IEND last, IDAT inflating to exactly `800 * (1 + 1280*3)` bytes. Gate
  battery for the arc: `./arroyo check` green both arches; `UNAOS_WC=1 ./arroyo test 150` exit 0
  with `wc` in the banner and no fault text; `./arroyo test-arm 60` exit 0.

- **Print Screen on metal — flight 5 proved the capture, and exposed the binding.** `SCREEN2.PNG`
  2880x1800 with IHDR and IEND verified landed on the machine, so the panel read, the encode and the
  stick write are metal-proven; but the trigger was bound to usage 0x46 alone, and the rMBP's own
  internal keyboard has no Print Screen key, so from the laptop's own keys the capture was
  unreachable — which is what PRTSCRCHORD answers. What QEMU still cannot prove is that the internal keyboard's
  report carries the chord as `GUI|Shift` in byte 0 plus usage 0x20/0x21 (Apple keyboards could in
  principle route ⌘ through a vendor page; the boot-protocol modifier byte says they do not). The
  metal procedure: boot, wait for the veto line, plug the FAT stick, press ⌘⇧3 — expect the
  `chord=cmd-shift-3` witness on the wire, then the `SCREEN<n>.PNG ... -> OK` line, then the file
  at the stick's root. If the witness never appears, the chord did not decode (the census says
  `requests` unchanged); if it appears and no `OK` follows, the refusal line names why.

- **The CHORDS on metal — flight 6 (2012 rMBP, `f751cb78`, 2026-09-03): CONFIRMED.** Knob line was
  flight 5's minus `UNAOS_PRTSCRST=1`, so no selftest could fire the capture (the staged kernel carries
  zero `prtscrst`/`PRTSCR-ST` strings, and the wire printed none across three boots). FAT stick in at
  370.9 s; then, on the internal keyboard:

  ```
  [ 384457ms] :: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed ::
  [ 455094ms] :: PRTSCR: SCREEN3.PNG 2880x1800 15555053 bytes -> OK ::
  [ 455101ms] EHCI-HID: KEYUP: '#' (scancode 0x20)
  [ 455140ms] :: PRTSCR: [prtscr] chord=cmd-shift-4 (GUI+Shift+digit) down on EHCI -> capture armed ::
  [ 525624ms] :: PRTSCR: SCREEN4.PNG 2880x1800 15555053 bytes -> OK ::
  [ 525631ms] EHCI-HID: KEYUP: '$' (scancode 0x21)
  ```

  So the Apple internal keyboard does report ⌘ as the boot-protocol GUI modifier bit — the one fact
  QEMU could not supply. No `KEY:` press line for either digit (the no-keystroke property holds on
  metal); indices 3 and 4 because the stick still carried flight 5's SCREEN0..2 (the no-overwrite rule).
  Both files verified host-side: PNG signature, IHDR 2880x1800 depth 8 colour 2, IHDR/IDAT/IEND with
  every CRC valid, IDAT inflating to exactly `1800 * (1 + 2880*3)` bytes.

  **Measured cost: 70.6 s and 70.5 s from chord to `OK`** for the 15,555,053-byte file — ~220 KB/s to
  the stick — during which the USB pump made zero passes (`[deadman] pmp=0` for the whole window), so
  keyboard and mouse were dead until the write finished. The second chord, pressed during the first
  write, was decoded 46 ms after the first `OK`: chords are deferred by the freeze, not lost. The
  duration is the capture running inside the device-service pass; making it incremental is a separate
  arc, recorded here as the number to beat.

## 10. PRTSCR-ASYNC — the capture runs in bounded slices

> **Where this lives.** The behaviour below is on `hw-jetson` at `6128706f` (the state machine) and
> `9905ddd7` (the Orin service cadence), granted by the rmbp seat 2026-09-06 and reaching this branch
> at the rmbp landing. It is documented here, in the subsystem's own file, because `screenshot.md` is
> this lane's and the alternative was two seats writing half a section each.

§6 describes a capture as one pass: reserve the entry, encode, write. That is what makes Print Screen
cost **70 s on the rMBP with the keyboard and mouse dead** (`A2`, promoted to `SR2` once Peter
reproduced it on the Orin) — the whole 15.5 MB write happens inside the device-service pass, at
~220 KB/s, with `[deadman] pmp=0` for its entire duration.

`capture_inner`'s straight-line body is now a `Job` state machine parked in a `spin::Mutex<Option<Job>>`:

| phase | unit of work | slice bound |
|---|---|---|
| `Phase::Encode` | scanlines pushed into the streaming encoder | `SLICE_ROWS` = 64 |
| `Phase::Write` | bytes per `write_grow`, in order from offset 0 | `SLICE_WRITE` = 32 KiB |

`Job::slice` runs units until `arch::hw_wait_budget()/64` cycles are spent — about 31 ms on x86 and
75 ms on the Orin — so `service()` advances **one bounded slice per device-service pass** and returns
to input polling. **No lock is held across a slice**: the job is taken out of the mutex, worked on
unlocked, and stored back, so the mutex is acquired exactly twice per slice and never across the work.

Two consequences worth stating because they are what a reader will check:

- **A press during an open capture is now visible.** It yields the named `Refusal::InFlight` line *and*
  the existing re-arm, so three fast presses produce three files and the collapse Peter reported ("3
  presses did not yield 3 captures") is named on the wire rather than silent.
- **`capture()` stays synchronous**, driving the same machine to completion. The `screenshot` verb and
  `UNAOS_PRTSCRST=1` therefore keep their wire byte for byte — §8's boot-time witness is unchanged.

**Service cadence is per-body, and that matters on the Orin.** `prtscr::service()` has five call sites:
`main.rs:1206` (the `usbdebug` loop), `:1696` (the `kernel_main` tail), `:3021` (the ~250 ms tegra
sweep), `:5957` (`x86_usb_pump`), and `:8358` in `orin_render_service`. The last two of those are
`holocron`-gated — that knob is the repo's arming switch for "this boot may WRITE its boot medium", so
an `orinrender` image that did not ask for a writer does not gain one. Without the `:8358` site the
Orin advanced a capture only on the 250 ms sweep, which keeps it responsive but stretches a capture
roughly fourfold.

## 11. The volume that leaves mid-capture — `Refusal::Vanished`

A sliced capture spans many service passes, and §8.1's rung 2 aims deliberately at *the operator's
carry-away stick* — the medium most likely to be pulled while the capture is still running.

**What happened before this existed, stated because it is what an old log will show:** nothing dangled
and nothing panicked. `USB-UNPLUG`'s retraction clears the USB block device, and every block entry
point re-reads the registry per call and bounds the LBA against that fresh snapshot, so the next
`write_grow` failed as `BlockError::NotReady` and the capture reported the **generic**
`Refusal::Fat("write", …)` line — a FAT errno that never names the disconnection. The failure was
safe and unreadable.

It is now named, and probed before `create_in_dir` and before **every** `write_grow`:

```
:: PRTSCR: SCREEN0.PNG — volume vanished mid-capture at 98304/3073098 bytes (usb geometry retracted or a newer publish replaced it; handles=…) — capture ABANDONED, nothing written through the stale handle ::
```

**The probe tests two facts, and the second is the load-bearing one.** It requires
`usb_info().is_some()` **and** that `usb_publish_gen()` (`drivers/block.rs:761`) is unchanged since
`begin`. Presence alone is not enough: a retract-then-replug refills the handle with a **different
disk**, and the parked `FatFs`'s stale LBAs would pass that disk's bounds checks cleanly. A presence
check would have been a check that cannot fire — it returns "alive" in exactly the case that most
needs catching — and the generation counter is what distinguishes "still there" from "something is
there".

The encode step is probed for the same reason: it spends seconds of passes before the first
volume-touching call, so an entry created on a disk that has left, or on a stranger's, is precisely
the stale-handle write this refuses.

## 12. SCRSHOT-DESKTOP — a capture belongs to a user, and lands on that user's Desktop

**Two rulings, a week apart, answering two different questions. Both are live; neither replaces the
other, and reading them as one is the mistake this section exists to prevent.**

| ruling | the question it answers | what it says |
|---|---|---|
| PRTSCR-HOME, 2026-09-13 | **whose** folder | *"screenshots should be saved to a user's ~/Pictures/Screenshots"*, and on the no-session half, *"do not hack screenshots to make it work right before multi-user is in."* A consequence of R51 — multi-user as a line: a human logs in and gets a home folder. Settling that a capture has an **owner** is what makes the no-session case the load-bearing half, not an edge. |
| **R60**, 2026-09-22 | **which** folder | *"is screenshot working? mac saves to desktop, correct? we should too, on this pioneer crispy theme anyway. we will be implementing a windows-esque them at some point so key-bindings shouldn't be hard coded."* The destination is `Desktop` — **and it is a property of the THEME**, on the same argument the ruling makes about key bindings in the same breath. |

### 12.1 The destination

`fs::users::whoami` names the open session; `fs::users::home_of` turns that name into the user's home
path (`/home/<name>` — the same path `ensure_home` creates at first login). The capture directory is
that home plus **one** leaf, and the leaf is `video::theme::CAPTURE_DIR` — `Desktop` under CRISPY —
so the resolved destination is `/home/<name>/Desktop`. `prtscr::ensure_capture_dir` walks it and
creates what is absent, component by component, on the volume the PRTSCR-VOL ladder (§6) settled on.
**This module reads `fs/users.rs` and writes nothing there**: `whoami` and `home_of` were already
public and are the whole of the interface.

**`video/prtscr.rs` does not know the word `Desktop` and must not learn it.** It asks the theme
table, exactly as `video/wm.rs` asks that table what colour a title bar is. R60's second sentence is
about key bindings, but its argument is about both: a Windows-shaped theme is coming, and the folder
a screenshot lands in is as much a fact about the desktop the user is looking at as the chord that
takes it. The second theme is one more `const` in `video/theme.rs` and **no edit at all** in
`prtscr`; a string literal in `prtscr` would be exactly the hard-coding the ruling names. For the
same reason every witness that reports the destination prints `theme=` beside it — the day two
themes exist, `dir=/home/una/Desktop` alone does not say which table answered.

Note which volume that is, because the two can differ and the difference is not a defect: the home
`ensure_home` makes lives on the EL0 volume, while a capture goes to the ladder's answer — which on a
read-only-boot bench is the operator's own USB stick. On that stick the same shaped path is created
under the same user name. That is right for a carry-away medium — still that user's screenshot, in
that user's folder, on that user's disk — and the `source=`/`serial=` fields on every verdict say
which disk it was.

Directory creation needs no new crash-consistency machinery. `create_dir` (`fs/fat.rs:3558`)
zero-fills and `.`/`..`-initialises the child **before** linking the parent, and publishes the child
cluster into the parent entry **last** — the same shape as `write_grow`'s SAFE ORDER. A boot cut
inside it leaves either no entry or a valid empty directory, never an entry pointing at an
uninitialised cluster.

### 12.2 The 8.3 question, answered from the code that decides it — and R60 retired the alias

**This FAT layer reads long file names and writes 8.3 only.** Both halves matter here:

| half | what the code does | where it is decided |
|---|---|---|
| **read** | VFAT long names ARE parsed (PI-FS-3): `LfnBuf` accumulates the 0x0F-attribute component slots preceding a short entry and checksum-validates the run; `DirEntry::eq_name` then matches **either** the long name or the 8.3 short name, ASCII-case-insensitively | `fs/fat.rs:190`, `fs/fat.rs:365`–`460` |
| **write** | 8.3 ONLY — *"this driver's create path writes 8.3 names only (VFAT LFN write is out of scope)"*. `format_83` is the decider: base `1..=8`, extension `0..=3`, each a legal short-name byte, else `None`. `create_dir` validates through it before allocating anything, so a rejected name returns `FatError::Unsupported` and leaks no cluster | `fs/fat.rs:118`, `fs/fat.rs:325`, `fs/fat.rs:3562` |

**`Desktop` is SEVEN characters, and that is the quiet gift in R60's destination.** PRTSCR-HOME
needed a two-spelling alias table: `"Screenshots"` is **eleven** characters, `format_83` returns
`None` for it, and we could not create a directory by that name on this filesystem at all — so the
rule was *look up long, create short* (`Screenshots` → `SCRSHOTS`, visibly an abbreviation and never
the truncation `SCREENSH`, which reads as a damaged word). A seven-character leaf clears the base
bound with a character to spare:

| the user asked for | looked up as | created as | why |
|---|---|---|---|
| `Desktop` | `Desktop` | `DESKTOP` | 7 characters — a legal 8.3 base exactly as written. **No alias, no second spelling.** Uppercase because `format_83` upcases what it stores, not because anything in the kernel does. |

**The alias table is gone rather than re-pointed.** `DIR_CAPTURE` in `video/prtscr.rs` keeps the
`(look up, create)` pair type and puts `theme::CAPTURE_DIR` in **both** fields, so there is exactly
one place the destination is written down and no second spelling that could drift from it.
`path_for_home` renders the medium's upcase through the same fold it already applies to the home's
own components, for the same reason.

The lookup runs before the create, so a volume that **already** carries a `Desktop` — a stick
formatted and filled on a Mac — is adopted verbatim, with its own spelling, and nothing new is made.
Only a genuinely absent folder is created, and then in 8.3. So: `/home/<name>/DESKTOP` on a volume we
created it on, `/home/<name>/Desktop` on one where it already existed, and the mapping is on the wire
for every capture rather than something a reader has to infer.

**The 8.3 legality of a future theme's word is measured, not const-asserted.** A compile error is not
a red run, and the go-red for this whole section is to point `theme::CAPTURE_DIR` at a name
`format_83` refuses. `dir_fixture` therefore checks the **necessary** condition on the wire every
boot (`legal83=` — one component, no dot, base `1..=8`), stated in `prtscr` because `format_83` is
private to `fs/fat.rs` and this arc does not touch that file. The **sufficient** proof is the
decider's own answer, which reaches the wire through `dir_refused`'s `-> REFUSED (unsupported name)`
line on any lane that has a session and a writable volume to reach it.

### 12.3 No session means no capture — and that is the answer, not a gap

There is usually no logged-in user on these boards, and on an ordinary build there is not even the
machinery for one: `fs/users.rs` is `#[cfg(feature = "login")]` (`fs/mod.rs:105`), `login` is not a
default feature, and the login screen does not open at boot even when it is built (SO43, in flight on
a parallel branch).

A capture with no session is **REFUSED** — `Refusal::NoSession`, one bounded line naming the reason,
**zero bytes written**. Not the volume root, not a shared folder, not a temporary landing place under
a new name. Three things that refusal deliberately is **not**:

* **not a fallback.** A shared destination would make the feature *look* finished on a machine where
  multi-user is not, and would have to be torn out and re-argued the moment sessions open.
* **not an error path.** It is a first-class outcome with its own witness, its own reason token and
  its own fixture, green from the boot it ships on.
* **not deferred work.** The code is complete as written; what it waits for is a session to exist,
  and nothing here changes when one does.

Two reason tokens, because they are different facts cured by different things:

| token | means | cured by |
|---|---|---|
| `no-login-built` | this image has no user store at all — `whoami`/`home_of` do not exist to call | building with `UNAOS_LOGIN=1` |
| `no-session` | the store is built and nobody is logged in | logging in (SO43's boot login screen, or the `login` verb) |
| `unknown-user` | a session names a user the store has no row for | — (a store defect; refused rather than invented around) |
| `bad-home` | the row's home path is empty or deeper than the walk's bound | — |

**The check is first**, ahead of the panel and ahead of the volume (`Job::begin` step 0). A capture
that can never belong to anyone must not spin up the PRTSCR-VOL ladder, must not choose a name, and
must not create a directory entry — so the check that guarantees all three sits ahead of all three.
That is what makes "zero bytes written" a structural property rather than a claim. On a no-session
boot a capture costs one lock and one line.

`PRTSCR-ST` (§8) treats it the same way its two existing waits are treated: announced once, **never
latched**, not a FAIL. A permanent red meaning "the feature is correct and nothing has exercised it"
is a broken instrument. It runs on the first pass after a login.

### 12.4 The wire

```
:: PRTSCR-DIR: theme=crispy user=una home=/home/una dir=/home/una/Desktop path=HOME/UNA/DESKTOP created=1 reason=session -> RESOLVED ::
:: PRTSCR: SCREEN0.PNG 1920x1200 name_from=clock-unset -> capturing (6912345 bytes reserved; …) ::
:: PRTSCR: SCREEN0.PNG 1920x1200 6912345 bytes -> OK :: source=usb serial=0x1A2B3C4D dir=HOME/UNA/DESKTOP ::
```

`theme=` and `dir=` are R60's two new fields on the RESOLVED line and no new line was added for them
— a capture's witness is bounded evidence, not a feed. `dir=` is the destination as the **user**
would say it (their own home, the theme's word); `path=` stays what the **medium** spells (upcased,
or an adopted long name), and the two differing is the 8.3 mapping being visible rather than
inferred. `created=` counts components this walk made, so `0` means every one of them was adopted.
`name_from=` on the `-> capturing` line is §5's naming rule, named rather than inferred from shape.

and, on every board today:

```
:: PRTSCR: no user session (reason=no-login-built) — a capture belongs to a user's own Desktop folder (theme=crispy) and there is none; NOTHING WRITTEN (no name chosen, no volume touched) — capture skipped ::
```

`dir=` is appended to the verdict line's **second** `::`-delimited segment, never folded into the
first: `scorers-render9.sh` keys on `bytes -> OK ::` being contiguous (A17 at :320 and :888, A36 at
:702), and a field inserted before that `::` would silently zero three census counters — a check that
cannot fire, produced by a witness change. The failure line names the component the walk stopped at:

```
:: PRTSCR-DIR: theme=crispy user=una home=/home/una path=HOME/UNA at=Desktop -> REFUSED (-ENOSPC) — nothing written ::
```

This is also where `format_83`'s **own verdict** on the theme's word reaches the wire: a
`CAPTURE_DIR` the short-name decider refuses comes back from `create_dir` as `FatError::Unsupported`
and prints here with nothing written and no cluster leaked (`create_dir` validates before it
allocates, `fs/fat.rs:3562`).

The resolved path is clipped at `DIR_PATH_MAX` (120 bytes) for printing only — an adopted long name
can be up to `LNAME_MAX` (768) bytes, and two of those would put ~1.5 KB on one serial line against
this module's standing rule that a capture's witness is bounded. The walk itself is never truncated;
components are appended whole, so a clipped path ends at a component boundary.

### 12.5 The fixture, and its go-reds

`prtscr::dir_fixture()` runs once per boot from `service()` — not behind a knob, because the
no-session refusal is what **every** board does today and a gate that runs only when someone
remembers a knob would not have measured the ordinary case once. It costs one relaxed load per
service pass after the first.

| arm | asserts | go red by |
|---|---|---|
| A — the destination R60 named, and the 8.3 mapping it goes through | three claims, because they fail for three different reasons: `path_for_home("/home/una") == "HOME/UNA/DESKTOP"` (the want is a **literal** — a want derived from `theme::CAPTURE_DIR` would assert nothing, and R60 named this folder by name); the theme is on the wire (`theme=crispy dir=/home/una/Desktop`); and `legal83=true` for the theme's word. Volume-free, so it runs on a board with no filesystem — which is exactly the board `./arroyo test` gives us, and the reason this is the arm the wc lane can score | pointing `theme::CAPTURE_DIR` at a name `format_83` refuses. Restoring the pre-R60 `"Screenshots"` does both halves at once — the rendered path becomes `HOME/UNA/SCREENSHOTS`, which is not the want, and `legal83=false` names **why** that folder could never have been created. Dropping the upcase in `path_for_home` reds it the other way, on the path alone |
| B — no session refuses, writing nothing | `plan_dir(None)` is `Err(NoSession)`, **and** the real `capture()` refuses with that variant while the `CAPTURES` census does not move | giving `plan_dir` a fallback destination for `None` — the shared-folder hack — which turns the `Err` false and the capture into an attempted write |

Both arms are pinned in `scripts/specs/x86-wc.spec` (SCRSHOT-DESKTOP block). The live
`:: PRTSCR-DIR: … -> RESOLVED ::` line is **not** pinned there and must not be: it needs a session
and a writable volume, and no `./arroyo test` lane has either (§12.6). It is a PENDING in the same
block, for the boot that can print it.

Arm B drives the real `capture()` only when the machine genuinely has no session. If one is open, a
boot-time fixture must not help itself to the operator's panel and write a file nobody asked for, so
the live leg is skipped and **says** it was skipped; arm B's pure assertion still holds the refusal
contract.

### 12.6 What is exercisable today

Honestly: **nothing on the happy path.** With `login` off by default and no boot login screen, every
capture on every current build takes the refusal. Arm A proves the destination and the 8.3 mapping
and arm B proves the refusal and its silence, and those are real gates that run on every boot — but
the resolved-home write is proven by construction and by `UNAOS_LOGIN=1` compilation, not by a booted
capture. Measured at R60's fold, so the next reader does not have to re-derive it:

* **The wc lane has no session.** `UNAOS_LOGIN=1 … ./arroyo test` builds the store and nobody logs
  in; the `loginst` fixtures open a session and **close it again** (`fs/users.rs:918`'s chain puts
  the boot back where it found it — row closed, screen down, no session), so even the armed lane
  reads `live=no-session` on arm B.
* **The wc lane has no writable volume for a capture.** The plain `test` medium is read-only to the
  PRTSCR-VOL ladder's rung 1 and attaches no USB FAT for rung 2, so `PRTSCR-ST` (§8) stays in its
  announced wait. `UNAOS_PRTSCRST=1 ./arroyo test-fat sf` is the lane that has one.
* **Metal says the same thing, out loud.** Flight 11 pressed ⌘⇧3 for real at 798 s and got
  `:: PRTSCR-VOL: rung=none rung1=read-only rung2=absent -> NO TARGET ::` — the rMBP's internal SD
  reader is read-only by policy (§8.1) and no stick was in.

All three are cured by work already in flight rather than by anything here: LOGINFLOW lands the
session, R59 lands the boot volume read-write, and on flight 12 a ⌘⇧3 after logging in should print
the RESOLVED line above and leave the file on Peter's own card.

## 13. What the Mac name needs — the FATLFN arc, NOT started here

A Mac writes `Screenshot 2026-09-22 at 17.31.02.png`. Thirty-six characters, two spaces and three
dots: not merely too long, but illegal in every separator. `fs/fat.rs:118` states the constraint
outright — *"this driver's create path writes 8.3 names only (VFAT LFN write is out of scope)"* — so
this is a **missing feature**, and §5's `MMDDHHMM.PNG` is what we do until it lands, not a substitute
for it.

**The read half is already built and is the specification for the write half.** PI-FS-3's `LfnBuf`
accumulates the 0x0F-attribute VFAT component slots preceding a short entry, checksum-validates the
run against that short name, and decodes it; `DirEntry::eq_name` (`fs/fat.rs:190`) then matches
either spelling. What FATLFN must add is the inverse of exactly that, and only that:

1. **Emit the component slots** — 13 UTF-16 code units each, written in reverse order, with
   `LAST_LONG_ENTRY` (0x40) or'd into the sequence number of the first one written.
2. **Compute the one-byte checksum** over the 11-byte short-name field, the same checksum the read
   path already verifies, and stamp it into every slot of the run.
3. **Allocate the run and the short entry contiguously** in one directory extend, with a crash order
   as deliberate as `create_dir`'s and `write_grow`'s: a boot cut must leave either no entry or a
   complete run, never a short entry whose long slots are half-written.
4. **Mint a non-colliding short alias** for the short field — the `NAME~1` form a real VFAT driver
   writes, with the `~n` bumped against what the directory already holds.

It is its own arc because item 3 is its own argument. This arc (SCRSHOT-DESKTOP) is the
**destination**; the name waits on that one.
