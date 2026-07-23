# Video and the Framebuffer

The video subsystem (`unaos/crates/kernel/src/video/`) owns everything that
reaches the screen. It is built on a single pixel-format-aware framebuffer
surface and layers a double-buffered, damage-tracked GUI surface and a boot/panic
text console on top of it.

> **Branch note.** The `video` module lives on the video track and the combined
> integration branch. It is also the substrate the future native `unaos`
> Quartzite backend will render through (see the
> [userspace architecture](../../USERLAND/ARCHITECTURE.md)).

---

## 1. Components

`video/mod.rs` defines the module and the shared surface:

```rust
pub static WRITER: Mutex<FrameBuffer>;   // the primary display surface
pub use framebuffer::FrameBuffer;
pub use screen::Screen;
pub mod fbcon;
```

- **`FrameBuffer`** (`framebuffer.rs`) — the one pixel-format-aware drawing
  surface; every pixel on screen goes through it.
- **`Screen`** (`screen.rs`) — a double-buffered surface with damage tracking,
  built on two `FrameBuffer`s (the live framebuffer + a cached-RAM back buffer).
  The steady-state GUI renderer.
- **`fbcon`** (`fbcon.rs`) — the boot/panic text console: a log sink for hardware
  with no serial port.

`WRITER` and `fbcon` are handles to the *same* physical framebuffer, used at
different times (fbcon during boot, the GUI `Screen` after a successful boot), each
serialised by its own lock.

---

## 2. Mode selection (bootloader)

Mode selection happens in the bootloader (`crates/bootloader/src/main.rs`) over
UEFI GOP. It enumerates the available modes, and selects the **EDID-native** mode
when an EDID is readable, falling back to the firmware-current mode otherwise. The
resulting geometry (width, height, stride, pixel format) is passed to the kernel
in `BootInfo`.

Boot evidence: `GOP: 30 modes (firmware current 1280x800)` … `GOP mode: 1280x800
stride=1280 fmt=Bgr`.

On real hardware, EDID may be absent (e.g. Apple EFI exposes no EDID protocol), in
which case the firmware-current mode is used. The framebuffer stride can exceed
the visible width (Apple panels pad the scanline), which the `FrameBuffer`
honours via the `stride` field — drawing by stride, not width, avoids shear.

---

## 3. `FrameBuffer` — the drawing surface

`FrameBuffer` is pixel-format-aware (`Rgb` / `Bgr` / `U8`) and addresses the
framebuffer by physical address (stored as `usize`, so the type is `Send` and
`Copy`):

- `init(base, len, info)`, `is_ready()`, `width()`, `height()`, `info()`
- `put_pixel(x, y, color)`, `fill_rect(...)`, `fill_rows(...)`, `fill_screen(...)`
- `blit(byte_offset, src)`, `scroll_up(dy, fill)`

---

## 4. `Screen` — double-buffered GUI surface

`Screen::new(front)` takes a `FrameBuffer` handle and builds a cached-RAM back
buffer. All GUI drawing (`put_pixel`, `fill_rect`, `scroll_up`, …) goes to the
back buffer; `flush()` copies **only the damaged region(s)** to the (slow)
framebuffer. The `kernel_main` loop calls the equivalent of `flush()` once per
frame, so the idle path stays cheap when nothing changed.

Damage is tracked as a small **set** of up to `MAX_DAMAGE_RECTS` (16) independent
rectangles, not a single bounding box (VUG-FPS). A new dirty rect merges into any
rect it overlaps (cascading); on overflow it folds into the least-growth
neighbour, so the set is always a correct **superset** of the true damage — never
dropping a dirty pixel, only rarely reflushing a clean one. `flush()` blits each
rect as its own run of bulk row copies. This matters for animated content with
**disjoint** dirty regions: a full-height rotating crystal plus two corner meters
plus a moving cursor would, under a single bounding box, collapse to a
panel-spanning box that reflushed most of the framebuffer every frame — the metal
`vug` 8–9 fps bottleneck. As separate rects each blits tightly. `Screen`'s
`last_flush_bytes()` reports the bytes the last flush copied (the `[vugfps]`
bandwidth witness); `last_flush_rects()` and `last_union_dims()` report the
merged rect count and the union bounding-box `(w, h)` of that flush's damage
(VUG-FPS-2), so a metal capture can see whether the disjoint regions really
stay separate or the merge cascade collapses them to a near-panel box.

The `[vugfps]` line (emitted ~1×/s by the `vug` loop) reads:

```
[vugfps] F.f fps  N bytes/frame flushed  bands=B  rects=R union=WxH  raster=Xus flush=Yus (n frames / m ms)
```

where `raster` and `flush` are the per-frame cycle split (VUG-FPS-2) — the draw
span (input drain + all back-buffer rasterisation) vs the present span
(`pal.render()`: the row blits + one cache-clean sweep). It names whether a slow
frame is raster-bound or flush(bandwidth)-bound; µs are derived from the aarch64
generic-timer rate (raw cycles on x86, where this witness is not the target). The
copy path itself is already a per-row `copy_nonoverlapping` (a word-wide memcpy),
not a per-pixel re-encode — so the split, not the copy loop, is where a flush
regression would show.

The `vug` crystal demo drives this with dirty-rect rendering: instead of clearing
the whole panel each frame it background-fills only the union of the crystal's
previous + current projected-vertex bbox (plus the cursor's old footprint), and
the HUD / corner meters clear their own small blocks. Painter's back-to-front
face ordering is unchanged.

This is what the console / `pal::TargetPal` draws through in steady state.

---

## 5. `fbcon` — boot and panic console

`fbcon` mirrors `serial_println!` output onto the framebuffer so boot diagnostics
and panics are visible on hardware with no serial port (most laptops):

- `init(fb_addr, fb_len, info)` — set up the console (runs pre-heap, so it owns
  its own `FrameBuffer` handle, no back buffer).
- `_print(args)` — draw a line (self-guarding: `try_lock` + interrupts off, so a
  panic mid-print cannot deadlock).
- `detach()` — once the GUI `Screen` takes over the display, fbcon stops mirroring
  so post-boot diagnostics don't scribble over the GUI.
- `panic_screen()` — paint a red panic backdrop and re-enable mirroring so the
  panic message is shown even after `detach()`.

---

## 6. Real-hardware status and build knobs

The software stack (FrameBuffer + Screen + fbcon + EDID-native mode select) builds
and runs on both arches under QEMU. On metal, the x86 path has been validated on a
2012 MacBook Pro Retina at its native 2880×1800 (with the Apple padded stride
handled). The aarch64 Raspberry Pi 4 path is blocked on a firmware SD-card driver
limitation (the fix is to boot the OS from USB).

Composable build knobs (env → Cargo features):

| Env var | Effect |
| --- | --- |
| `UNAOS_SKIP_XHCI=1` | Video only; skip USB/xHCI bring-up (firmware may still own the controller on metal). |
| `UNAOS_BOOTLOG=1` | Hold the fbcon boot log on screen + print the EDID/mode-selection readout, then halt (for photographing serial-less hardware). |
| `UNAOS_PI=1` | aarch64: don't touch the QEMU-address UART (it is RAM on a real Pi). |

---

## See also
- `unaos/crates/kernel/src/video/` and `crates/bootloader/src/main.rs` — the implementation.
- [userspace architecture](../../USERLAND/ARCHITECTURE.md) — the planned native `unaos` Quartzite backend that will render through this surface.
