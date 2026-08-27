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
| — | the device-service pass, beside `fat::probe_once()` | `prtscr::service()` sees the flag and performs the capture |

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
cannot apply — the name is known absent): `mount_program_source` → `write_veto` → `create_in_dir`
→ `write_grow(0, 0, dir_lba, dir_off, 0, bytes)`.

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
:: PRTSCR: no FAT volume on any program-source handle (NoDisk; handles=...) — capture skipped ::
:: PRTSCR: REFUSED READ-ONLY (source=... label=... reason=...) — capture skipped ::
:: PRTSCR: SCREEN0.PNG..SCREEN99.PNG all present at the volume root — capture skipped (nothing overwritten) ::
:: PRTSCR: encoder declined (OutOfMemory) for 2880x1800 needing 15555053 bytes — capture skipped ::
:: PRTSCR: create failed -EAGAIN (Busy; handles=...) — capture skipped ::
:: PRTSCR: SCREEN0.PNG short write 512 of 3073098 bytes — capture INCOMPLETE ::
```

The key edge announces itself before the deferral, so a metal capture can tell "the key never
arrived" from "the key arrived and the capture refused":

```
:: PRTSCR: PrintScreen (HID 0x46) down on <controller> -> capture armed ::
```

`prtscr::census()` returns `(requests, captures, refusals)` for the same reason — a key press that
produced no file and a key press that never happened are different failures.

Two sinks, two lengths (FATVERB's rule): the console gets one sentence, serial gets the census.
The panel clips at 128-180 columns and the census tail is the whole diagnostic.

## 8. Verification status

- **Encoder**: proven as a pure function on the host. A harness `#[path]`-includes `video/png.rs`
  itself, checks CRC-32 and Adler-32 against their published check values, and writes two images —
  one single-block, one spanning eleven stored blocks. Python's real `zlib` decompresses both, every
  chunk CRC validates, and the decoded pixels are byte-identical to the source rows. `file` reports
  `PNG image data, ..., 8-bit/color RGB, non-interlaced`.
- **Kernel path**: `UNAOS_WC=1 ./arroyo test` green with `wc` in the feature banner; the plain test
  leg attaches no block device, so the honest no-volume refusal is what it prints.
- **Print Screen on metal**: the seat's proof, at an arc boundary, on the rMBP.
