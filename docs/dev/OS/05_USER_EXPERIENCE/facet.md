# FACET — UnaOS's image viewer

Status: **M1 landed (unflown)** — a kernel desktop tenant that opens a PNG by path, decodes it
streaming, box-reduces it to fit, and puts it on the glass. Knob: `UNAOS_FACET=1`, implied by
`deskcascade`, and it implies `quarry` because the file manager is its only door.

Peter's direction, 2026-09-08, verbatim: *"i was tempted to double click on one of the screenshots
... i believe we already have an image viewer in the vessels can we compile it for UnaOS?"*

The name is `docs/CODEX.md` §2's, not a coinage: **Facet — Images — "The Canvas"**.

---

## 1. The answer to the question as asked, and why it is no

The ask was literally *compile the vessel*. It cannot be compiled, and the two reasons are
independent — closing either one would leave the other standing.

**`vessels/facet` is host-native by construction.** It is a Tokio program (`libs/quartzite`), it
renders through WGPU (`libs/euclase`), and it decodes through the `png` crate (`libs/lux`). Those are
three `std` dependency trees, none of which has a build for a freestanding target. Porting it is not
a build-flag change; it is a rewrite of everything except the file format.

**And no EL0 program in this kernel can host a PNG decoder at all.** DEFLATE's history window is
32 KiB by definition — RFC 1951 §3.2.1, and `selfhost::inflate::WINDOW` is exactly `32 * 1024` —
while every EL0 task in this kernel gets the blanket 16 KiB (`sched.rs`). The decoder's *mandatory*
working set is twice the whole window it would have to live in, before a single pixel is stored.
That is a fact about the ring, not about the program: no amount of care in a userspace port gets
past it, and the ring-3 line is the right home for this the day the window grows.

So Facet is a **kernel desktop tenant** of exactly Quarry's shape — a `wm` row over a cached-RAM
ARGB8888 surface it owns, presented through the ordinary `wm::present` path, with nothing touching
the scan-out. The decode half is a pure function over bytes and moves out unchanged when the ring-3
version becomes possible.

---

## 2. What an operator does

1. The desktop comes up with Quarry on it (`UNAOS_DESKCASCADE=1` on the Orin).
2. Navigate to the card and **double-click `SCREEN6.PNG`** — or select it and press `<Enter>`.
   Double-click is Quarry's existing predicate: two presses on the same row of the same pane inside
   `DOUBLE_CLICK_MS` (400 ms), `quarry::live::is_double`.
3. The picture opens in its own window, **titled `SCREEN6.PNG`** — the DOCUMENT's name, per Peter's
   R36 (an app window carries the app's name; a document window carries the document's).
4. The close disc frees the surface. Opening a different picture replaces this one: there is one
   canvas, so a session of clicking through screenshots costs one window and one allocation.

---

## 3. Memory — the whole design

A 1920x1200 screenshot is 6.9 MB of RGB, and `prtscr` writes its PNGs with STORED deflate blocks, so
the **file** is 6.9 MB too. A naive viewer holds three of those at once (file, decoded image, window
surface) against a 48 MiB heap shared with the whole desktop. Facet holds none of them:

| what | naive | Facet | how |
|---|---|---|---|
| the file | 6.9 MB | ~256 KiB | `IdatSource` pulls the concatenated IDAT payload through the VFS in `CHUNK` reads |
| the decoded image | 6.9 MB | 2 scanlines + 1 accumulator row (~23 KB) | `RowSink` unfilters one scanline and immediately boxes it into the output |
| the window surface | 6.9 MB | 2.3 MB | the output is `out_w * out_h * 4` at target scale, `try_reserve_exact`ed before a byte is read |

Peak is therefore ~2.6 MB for a full-panel screenshot rather than ~21 MB, plus the decoder's one
mandatory 32 KiB DEFLATE window. `close()` returns the surface allocation (`*SURF.lock() = Vec::new()`,
not `clear()`, which would keep the capacity for the rest of the boot).

---

## 4. The decoder, and the one it is NOT

`selfhost::inflate` (SELFHOST-2) is already a streaming RFC 1951 decoder with a 32 KiB window. PNG's
IDAT payload is a **zlib** stream (RFC 1950) — the identical DEFLATE body between a 2-byte header and
a 4-byte big-endian Adler-32, where gzip has a 10-byte header and a CRC/ISIZE trailer. So this arc
adds no second decompressor:

* `inflate::deflate_body` is the block loop, lifted out of `gunzip` verbatim;
* `inflate::gunzip` is the RFC 1952 wrapper, unchanged in behaviour;
* `inflate::zlib_inflate` is the new RFC 1950 sibling — header checks (CM, CINFO ≤ 7 so the window
  cannot exceed ours, FCHECK, no FDICT) and an `AdlerSink` that checksums on the way through.

**The trailer is checked**, for `gunzip`'s reason in this container's terms: a decoder that produced
garbage would still be internally consistent, but it could not also reproduce the Adler-32 the
encoder stamped over the *original* bytes. That is what makes `inflate=OK` a claim about the file
rather than about whatever the decoder happened to emit, and it is why a corrupted IDAT surfaces as a
named refusal instead of as plausible noise on the glass.

`lib.rs` declares `selfhost::inflate` alone (an inline `mod selfhost { pub mod inflate; }` under
`all(feature = "facet", not(feature = "selfhost"))`), so `UNAOS_FACET=1` pulls in the decoder without
the FAT source walk, the tar walker or `verify_source_once`, and `UNAOS_SELFHOST=1` is unaffected in
every configuration.

---

## 5. What it accepts

Every **non-interlaced** PNG: bit depths 1/2/4/8/16, colour types 0 (greyscale), 2 (truecolour),
3 (palette), 4 (grey+alpha), 6 (RGBA), and **all five** filter types (None/Sub/Up/Average/Paeth).

`prtscr` itself only ever emits depth-8 truecolour with filter 0 (`video/png.rs::push_row`), so a
decoder tested against our own output would ship with four untested filter branches — and the card
also holds PNGs written elsewhere. Adam7 interlacing is **refused by name** rather than decoded
wrong: it needs seven passes and a second address arithmetic, and no file on this medium has it.

Alpha is dropped, not composited. Stated rather than hidden: a viewer that silently premultiplies is
lying about the pixels, and the checkerboard a real viewer draws is a later rung.

### Scaling

`fit(iw, ih, bw, bh)` picks the **integer** reduction `k = max(ceil(iw/bw), ceil(ih/bh), 1)` and the
output is `iw/k x ih/k` with the remainder dropped. Two consequences, both deliberate:

* every output pixel is the mean of exactly `k*k` source pixels, so the divisor is constant and no
  partial block has to be special-cased — that is what makes `RowSink`'s accumulator exact;
* at most `k-1` source rows and columns are discarded (one pixel at 1920x1200 → 960x600).

It is a real **box** filter, not a nearest-neighbour drop: a screenshot of 8-pixel text point-sampled
at 1/2 loses half its strokes, and the point of opening `SCREEN6.PNG` is to read what is in it.
Facet never upscales — `k = 1` for anything that already fits — because a blown-up icon is a worse
answer than the icon, and `wm`'s own compositor scale already magnifies a window when the desktop
wants that.

---

## 6. Where it is wired, and the two hooks that do not exist

Facet touches **one new file** plus three hook lines, and every one of them is inside `video/`:

| seam | site | why not the obvious place |
|---|---|---|
| the OPENER | `quarry::live::activate_row` — a `.PNG` name becomes `Act::View(path)` | the obvious place is an association registry; that is the right shape for the *second* opener and the wrong shape for the first (`quarry.md` §7) |
| the DRAIN | `quarry::live::run_act` latches, `quarry::live::service` drains | see §7 |
| POINTER routing | chained from `quarry::live::press_route` | the two files that name every other tenant's press arm are `arch/aarch64/syscall.rs` (compiled into the knob-off `kernel8.img`, whose byte-identity proof one added line breaks — PARITY.md §5.3) and the x86 half, which another lane owns |
| the FIXTURE | chained from `quarry::live::selftest` **and** `door_selftest` | the aarch64 battery reaches the first, the x86 battery reaches the second; `crystal::selftest`'s own words: a battery names one furniture fixture and the family reaches the rest |

**Two hooks that would replace the chaining, named rather than implied.** Neither is this arc's to
mint:

1. **A `wm` tenant press table.** `wm` knows every kernel-band row and its owner; a
   `wm::route_press_to_owner(x, y)` that dispatched to a registered `press_route` would delete the
   `pulsewin || quarry || facet` chain in both routers and let a new tenant register itself. It is a
   `wm.rs` change and `wm.rs` is a shared file with four other arcs live on it.
2. **A furniture service list.** Same shape for `service()`: today `main.rs` names `pulsewin::service`
   and `quarry::service` by hand, so a tenant with a latch either gets a line in `main.rs` or chains
   off someone else's, which is what Facet does.

---

## 7. Why the open is LATCHED and not called

`run_act` runs at **click-router depth** (and at key-router depth), on the input-drain band's 16 KiB
kernel stack. That is the stack **Pi boot 11 overflowed** with `quarry::open()` called from exactly
there — a directory read plus an allocation plus the window table, synchronously, under the router.
`facet::open_inner` is strictly heavier than that overflow was: a chunk walk through the VFS, a
megabyte-scale streaming read, the whole inflate, an allocation and the window table.

So the gesture calls `facet::request_open(path)`, which stores a path, and `facet::service()` opens
it from the render pass — the same place the shell's own volume reads happen, and the same law
`dock::press_at` follows. It is chained from `quarry::live::service`, which `main.rs`'s Orin render
pass and `arch/aarch64/syscall.rs`'s strip-press arm both already drain.

The visible consequence is honest and small: the path bar says `opening SCREEN6.PNG` and the verdict
(`[facet] present` or `[facet] refuse`) lands one pass later. Claiming a verdict at the press would
be claiming something that function cannot know.

**On x86 the latch has no drain, and that is unreachable rather than broken.** `quarry::service` is
called from `main.rs`'s Orin render pass and `arch/aarch64/syscall.rs`'s strip-press arm — both
aarch64 — so an x86 build would latch and never open. It cannot arise: `quarry::live::collect` is
`#[cfg(target_arch = "aarch64")]`, so an x86 Quarry lists nothing and no `.PNG` row exists to click.
The day the x86 VFS adoption lands (`vfs.md` §12.4) and that shim collapses, `facet::service()` needs
a drain in `x86_render_service` — recorded here rather than discovered then.

---

## 8. Witnesses

Every path prints exactly one line; there is no silent failure.

```
[facet] open path=/boot/SCREEN6.PNG ihdr=1920x1200 depth=8 colour=2 bytes=6913257 idat-chunks=1 -> DECODING
[facet] decoded rows=1200 inflate=OK ms=…
[facet] present win=3 scale=1/2 src=1920x1200 shown=960x600 title=SCREEN6.PNG
[facet] refuse path=… reason=<not-png|bad-ihdr(…)|inflate-…|too-large(…)|interlaced-adam7-unsupported|bad-idat(…)|bad-palette(…)|bad-filter(N)|row-count(a of b)|alloc(N bytes)|no-window(…)|vfs-…>
[facet] press close win=3 at (x,y)
[facet] closed win=3 paints=1
[quarry] open VIEW path=… -> facet (latched for the render pass)
[pidesk] facet ARMED — …
```

`reason=` tokens are stable, lower-case and hyphenated, so a spec or an `awk` can match on them.

### FACETPNG — the fixture

`facet::selftest`, under `witness`. It **builds** its image rather than reading one off the volume,
and that is the whole point: a fixture that read `SCREEN6.PNG` would prove the plumbing on a machine
that happened to have taken a screenshot and would be vacuous in QEMU, which is exactly where it
runs.

* **Legs 1-5** — a 5x5 truecolour-8 image whose rows are filtered with types 0, 1, 2, 3 and 4
  respectively (legal PNG: the filter is per scanline), wrapped in a real zlib stream with a real
  Adler-32 and inflated through `zlib_inflate` — the SAME entry the file path takes. The verdict is a
  **checksum over the decoded pixels**, not a row count: a decoder that unfiltered wrong would still
  produce the right number of rows.
* **Leg B** — `fit`: 4x4 into a 2x2 box gives `k=2`, and an image that already fits is not upscaled.
* **Leg C, the negative** — one byte of the compressed payload is flipped and the decode must FAIL
  with a named reason. Without it a green board could not tell a working Adler check from an absent
  one, and "never a silent failure" is the property this module is built around.
* **Leg D** — `parse_ihdr` refuses a zero dimension and an Adam7 image by name, and `is_png_name`
  routes `.PNG`/`.png` and nothing else.

```
:: FACETPNG: filters=0,1,2,3,4 zlib=selfhost::inflate::zlib_inflate checksum=0x……… expected=0x……… corrupt-idat-refused=adler :: PASS ::
```

---

## 9. Bounds

Every one is a loop bound or an allocation bound over attacker-shaped input — the file is whatever is
on the medium, and its IHDR is four bytes that *claim* a size. Nothing trusts a claimed dimension
before it has been multiplied out and checked.

| bound | value | why |
|---|---|---|
| `MAX_FILE` | 64 MiB | ~9x a full-panel capture; the file streams, so this bounds WORK and the IDAT index, not a buffer |
| `MAX_DIM` | 16384 | PNG allows 2^31-1; at depth-16 RGBA one such scanline is 16 GB and `RowSink` allocates two |
| `MAX_IDAT` | 4096 chunks | the index vector's bound |
| `CHUNK` | 256 KiB | the FAT backend re-collects the cluster chain per `read`, so FAT traffic goes as the square of the chunk count (`selfhost::CHUNK`'s model) |
| `CEIL_W/H` | 1200x800 | smaller than the panel on purpose: a viewer that covered the screen would hide the window it was opened from |
| `MAX_OUTPUT` | 512 MiB | `selfhost::inflate`'s, inherited — a compression bomb stops and says so |

---

## 10. Owed

1. **Alpha.** Composite over a checkerboard rather than dropping the channel.
2. **Adam7.** Seven passes and a second address arithmetic; refused by name today.
3. **Pan and zoom.** The window shows a fitted whole image and binds no keys. A 1:1 view of a region
   needs a scroll model, which Quarry already has one of (`quarry.md` §5) and which should be lifted
   rather than copied.
4. **The two `wm` hooks in §6.** Until they exist, Facet's routing chains off Quarry's, which is
   truthful about the dependency but is not where a second document viewer should have to live.
5. **An association registry** — `quarry.md` §7 item 7, now one opener less hypothetical.
6. **JPEG, and everything else.** `is_png_name` is the whole routing test today.
