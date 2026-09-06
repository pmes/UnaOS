# Screen capture — PRTSCR

Print Screen, and the `screenshot` verb, write the panel to `SCREEN<n>.PNG` at the root of the FAT
volume. This document states what the mechanism is, where each piece runs, and what it refuses.

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

`SCREEN0.PNG` .. `SCREEN99.PNG` at the volume root, first free index wins. **An existing capture is
never overwritten**: the search asks `locate_in_dir(0, name)` per candidate and takes the first
`NotFound`; when all hundred are present the capture refuses and says so rather than wrapping around
onto `SCREEN0.PNG`. The lookup goes through the filesystem rather than a directory listing because
`locate_in_dir` matches on both the 8.3 short name and any long name, and `create_in_dir` does not
de-duplicate. The names are 8.3-clean, so they need no long-name entry.

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
