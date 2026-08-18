# QUARRY — UnaOS's file manager

Status: **M2 landed** — M1's window, volume tree, detailed list and scrolling, plus the four
corrections the bench asked for: a cached listing path, one row per volume, a working double-click
launch, and a landing rule that opens where the content is. Knob: `UNAOS_QUARRY=1` (pair it with
`UNAOS_PIDESK=1`).

Peter's direction, 2026-08-17, verbatim: *"Tree on left and start with detailed list view on right.
I'm not sure we have scrolling yet."* — and, on placement: *"pinned to the left side of the
taskbar/dock so it opens like Mac's Finder"*. The name is his, the same day.

He was right about the scrolling. There was none. §4 is the audit.

---

## 0. M2 — the four bench complaints, and what each one actually was

M1 shipped and was driven on the bench the same night. Four things came back, verbatim, and none of
them was a matter of taste. Each had a mechanism; §11 writes each mechanism and its fix down in full,
and this table is the map.

| Peter said | it actually was | the fix |
| --- | --- | --- |
| *"FAT contents VERY SLOW to come up"* | one listing = **three full volume probes**, and every navigation asked for the same directory **twice** | a path-keyed directory cache — §11.1 |
| *"/fat is LISTED TWICE"* | every mount prefix was a tree ROOT, **and** `/`'s listing carries the same mount points as child rows | the duplicate-root rule — §11.2 |
| *"double-click on VUG should open the app"* | M1 deliberately minted no launch path at all | double-click / `Enter` → `spawn_user_image_bg`, the shell's own `bg` seam — §11.3 |
| *"where is vug, where is the kernel — nothing about it is normal"* | it opened on the **emptiest volume in the namespace** (`/` carries two files; the card is `/fat`) | the landing rule — §11.4 |

Nothing in M2 is FAT-specific. ORIN lays out UnaFS in days, and every rule below is written against
the namespace (mount prefixes, directory entries, the block layer's hot-plug epoch) rather than
against a filesystem — §11.5 states that constraint and where each fix satisfies it.

---

## 1. What it is

A compositor window carrying two panes and a path bar.

| region | contents |
| --- | --- |
| path bar | the absolute VFS path the list pane is showing, a truncation notice when the medium had more entries than the model holds, and (M2) the **last activation's result** — `started pid 7`, `8 live jobs — kill one first`, `no opener for CONFIG.TXT`. This window has no console, so a launch's only feedback is here |
| left pane | the **directory tree**. M2: one root per **unclaimed** mount prefix (§11.2) — on this machine that is `/` alone, with `fat/` and `usb/` under it — expandable to `MAX_DEPTH` (8) levels, with a disclosure triangle per row |
| right pane | the **detailed list** of the selected directory — `NAME` · `SIZE` · `MODIFIED`, header pinned above the scrolled band. M2 marks rows `ls -F` style: `/` for a directory, `*` for a file a double-click will RUN |
| both panes | a scrollbar in `theme::SCROLL_TRACK`/`SCROLL_THUMB` whenever the content is taller than the pane |

Files: `unaos/crates/kernel/src/video/quarry.rs` (the knob seam) and
`unaos/crates/kernel/src/video/quarry/live.rs` (the implementation).

### Gestures

| gesture | effect |
| --- | --- |
| `Up` / `Down` | move the selection in the focused pane; the viewport follows it |
| `Left` | in the list, focus the tree; in the tree, collapse — or step to the parent row when already closed |
| `Right` | in the tree, expand — or, on an already-open row, cross into the list |
| `Enter` | tree: show this directory. list: **open the row** — descend into a directory (revealing it on the left), or RUN a `.ELF`/`.BIN`. The keyboard twin of the double-click, deliberately the same function: a gesture that exists only on the pointer is one an operator at a serial console cannot reach |
| `Backspace` | up one level |
| `r` | re-read — **including the mount table**, and (M2) dropping the directory cache, so a stick plugged after Quarry opened appears and a card written to from the shell re-reads |
| `Esc` | close (the keyboard goes straight back to the console) |

The keyboard contract is `instgui`'s: while Quarry's window is **on the glass** it takes first refusal
on the keys above, and consumes nothing else, so the console keeps working underneath. `Esc` hands the
keyboard straight back. "On the glass" and not merely "open" is load-bearing — `wm` expresses
*minimised* as a position (`z` below `SHELL_Z`), so a parked Quarry that kept first refusal would be
eating arrows for a window the operator cannot see. The pointer needs no equivalent guard:
`wm::hit_test` does not report a row that is not compositing.

| pointer gesture | effect |
| --- | --- |
| click a tree row | select and show it; clicking the **disclosure triangle** toggles instead |
| click a list row | select it |
| **double-click a list row** | (M2) open it — same rule as `Enter`. Two presses on the **same row of the same pane** inside `DOUBLE_CLICK_MS` (400 ms). §11.3 derives the constant; nothing in this tree had one |
| click a scrollbar **track** | page towards the press — see §4 for why the track, and not a wheel |
| title bar / borders | drag, minimise, zoom — the ordinary `wm` arms, deliberately not claimed |
| dock's `quarry` tile | reopen |

---

## 2. Architecture: why this is a kernel window and not an EL0 program

The ring-3 line (`user-vug`, `user-stat`, `user-pulse`) is where a new app should live, and the
question was asked seriously before it was answered the other way. Two facts in the tree make the EL0
version **impossible today**, not merely harder. Both are checkable rather than matters of taste.

### 2.1 An EL0 window is hard-capped at 128 x 128 pixels, on both arches

`arch/aarch64/boot.rs:89` and `arch/x86_64/memory.rs:511` both read `FB_WIN_MAX_W: u32 = 128`, and the
x86 side carries the assertion that ties it to the window region slot:

```rust
const _: () = assert!((FB_WIN_MAX_W * FB_WIN_MAX_H * 4) as usize == FB_WIN_SLOT_SIZE);
```

`sys_win_create` refuses anything larger with `-EINVAL` on both arches
(`arch/aarch64/syscall.rs:12119`, `arch/x86_64/syscall.rs:3567`). At the 8-px `font8x8` cell that is
**16 columns** — four short of a single FAT 8.3 name, before any thought of a size column beside a
date column beside a tree. The deliverable is not expressible in that surface, and raising the cap is
a window-region memory-map arc on **both** arches (the x86 half is the rmbp lane), not a file-manager
arc.

### 2.2 There is no directory syscall, and the one listing EL0 *can* do bypasses the VFS

`una-abi` is the frozen ABI. Its filesystem verbs are `SYS_OPEN(11)`, `SYS_READ(12)`, `SYS_SEEK(15)`,
`SYS_UNLINK(16)`, `SYS_CLOSE(17)`. There is **no** `readdir`, no `getdents`, no `stat`; numbers 34–39
are unallocated. `SYS_OPEN` itself does not route through the mount table — it binds `fat::mount()`
directly and caps the name at 12 bytes, so `/fat/DOCS/X.TXT` is not even expressible from ring 3
(`vfs.md` §12.4 names this as the outstanding adoption).

The one directory listing an EL0 program can perform today is the midden bus's `BUS_VERB_LS`
(`arch/aarch64/syscall.rs:21351` `bus_ls`), and it reads the **FAT root** through `fat::mount()`. That
is a raw backend. Routing Quarry through it would reproduce, in a new program, precisely the
"`ls` and `cat` name different files" defect VFS-1 (adoption) was written to delete.

### 2.3 What was built instead

The idiom this tree actually uses for a windowed tool that needs room and kernel data:
`video/instgui.rs` (the installer dialog), `fbcon::panel_console_window_open` (the console window),
`main.rs`'s `open_shell_window` (the shell window). All three are **kernel-owned `wm` rows over a
cached-RAM ARGB8888 surface**, presented through the ordinary `wm::present` path. Quarry is the
fourth.

It is written so the ring-3 version is a **port, not a rewrite**: the geometry accessor, the scroll
arithmetic, the tree splice, the list model and the whole painter are pure functions over a `DirEnt`
slice and a `&mut [u32]`. §7 lists what has to land first, in order.

**Front-buffer discipline.** Every pixel lands in the heap surface; presentation is `wm`'s. This
module never touches the framebuffer and never holds `WRITER` across a directory read.

---

## 3. The VFS seam

Every directory read goes through **`shell::vfs_ls_collect`** — the one collector VFS-1 (adoption)
left behind when it deleted the per-volume ones. Quarry therefore inherits, by construction:

* longest-prefix mount resolution at a component boundary (`/usb` never claims `/usbfoo`);
* the VFS-4 `-ENODEV` guard, so a reserved-but-unbound volume reports *the volume* as missing rather
  than a bare `-ENOENT` off the native root;
* the synthesized mount-point rows (`fat/`, `usb/`) immediately below a listed path, present only for
  bound prefixes — the honest hot-plug posture of `vfs.md` §6;
* the `.`/`..` filter;
* `DirEnt::size` and `DirEnt::mtime`, which `vfs.md` §12.3 added for exactly this shape of consumer.

The tree's **roots** are `MountTable::prefixes()` off the same table the collector builds. Quarry
names no backend, calls no `fat::mount()`, and calls no `unafs::with_unafs()`.

Two visibility widenings were needed and nothing else: `shell::vfs_ls_collect` and
`shell::vfs_mount_table` became `pub(crate)`. Both edits are one token; neither changes a line count.

### 3.1 The one `target_arch`, and why it is not Quarry's

`fs/vfs.rs` gates `NativeBackend` and `FatBackend`'s impls to aarch64 — `vfs.md` §12.4: *"x86 is
unchanged by design … that arch has no mount table to route through"*. So on x86 the collector and the
mount table **do not exist to be called**, and Quarry's two shims (`collect`, `roots`) carry the only
`cfg(target_arch)` in the module.

It is not a hardware decision and it is not a new one — it mirrors a gate already in the tree. Quarry
compiles, lays out, scrolls, hit-tests and paints identically on both arches; on x86 it opens on an
empty volume list and says `no VFS mount table on this arch yet (vfs.md 12.4)` in the list pane rather
than pretending. The day the x86 VFS adoption lands, the two shims collapse into one and x86 gets a
working file manager with no further work here.

---

## 4. Scrolling — the audit, the verdict, and what was built

**Verdict: nothing in the window/present/input stack could scroll a list, at any of the four layers a
scrolling list needs.** Quarry is the first scrolling anything in this tree.

| layer | what exists | usable? |
| --- | --- | --- |
| input | ~~`drivers/xhci/mod.rs` decoded the HID boot mouse as `byte0=buttons, byte1=dx, byte2=dy` and ignored byte 3; `pal::Event` had no wheel variant and `una-abi` no `INPUT_EV_WHEEL`.~~ **Closed by the WHEEL arc:** the decoder reads byte 3 as a signed `i8` whenever the Transfer Event residual says the report actually carried four bytes, `pal::Event::Wheel(i8)` carries it, `INPUT_EV_WHEEL` packs it, and the router delivers it to the focused EL0 window (`[wheel1]` census). No CONSUMER exists yet — the channel is live, nothing scrolls on it. The HID keymap still drops PageUp/PageDown (`0x4B`/`0x4E` → `(0,0)`). | **wheel: yes; PageUp/Dn: no** |
| compositor | `wm.rs` is 18 373 lines and `WindowInfo` has no scroll, content-offset or viewport field. There is no clip-to-content-rect. | **no** |
| blit | `FrameBuffer::scroll_up(dy, fill)` (`framebuffer.rs:552`) is a **whole-surface** memmove — no rect, no clip, kernel-private, behind no syscall. `video/vperf.rs` instruments its cost; it is a benchmark subject, not an API. | **no** |
| widget | `theme::SCROLL_TRACK` and `theme::SCROLL_THUMB` have existed since the theme table landed and **no scrollbar has ever consumed them** (their only uses were a row highlight in `instgui` and the dim minimised pip in `dock`). `theme::SCROLLBAR_WIDTH` had no consumer at all. | **no** |

Console *scrollback* exists (`console.rs`, a 256-line ring) but is tail-anchored with no position
variable, so it cannot show older lines either; `selftest.rs`'s `Pager` is forward-only. Everything
else a `grep -i scroll` finds is host-native ring-3 code (`libs/quartzite`'s AppKit/GTK views,
`handlers/aether`'s engine) that does not run on UnaOS at all.

### What Quarry implements

The **minimal honest version**: an integer row offset per pane, a clamp, and a full pane redraw at the
new offset.

```rust
fn scroll_max(len, visible) -> usize;                       // len - visible, saturating
fn scroll_follow(scroll, sel, len, visible) -> usize;       // clamp + keep the selection on screen
fn thumb(track_h, len, visible, scroll) -> Option<(y, h)>;  // None when everything fits
```

Two invariants, both asserted by the witness over a swept input space rather than three hand-picked
cases: the offset never exceeds `scroll_max` (so the viewport never shows blank rows under a short
list), and the selection is always inside the viewport (so the keyboard cannot drive a cursor the
operator cannot see).

### Its cost, stated rather than hidden

A scroll step costs **one full surface repaint plus one `wm::present`** — `w * h` word stores into
cached RAM, bounded by the `CEIL_W x CEIL_H` (1152 x 720) surface cap, then the compositor's ordinary
staged copy-out. It does **not** cost a per-row damage rectangle, and that is deliberate:
`wm::present_rows` is reached from the x86 syscall arm only (`SYS_WIN_PRESENT_ROWS` is documented
x86-only in `una-abi`, with a mandatory whole-box `-ENOSYS` fallback in `user-vug`). A row-band present
that one arch honours and the other does not is not a mechanism, it is a fork — so Quarry presents
whole boxes on both arches and the row-band optimisation waits for the arch to catch up.

The surface cap is what makes that affordable: at 1152 x 720 a repaint is ~830 k word stores, and a
repaint happens only on an actual gesture — there is no animation and no per-frame paint. A settled
Quarry costs nothing at all.

### Scroll GESTURES, given no wheel CONSUMER

The scrollbar **track** is the coarse gesture: a press above the thumb pages back, below pages
forward. That was originally the consequence of the wheel byte being discarded in the xHCI decoder
before the ABI ever saw it; the WHEEL arc has since landed the byte, the event and the routing (§4's
table), so the track gesture is now a CHOICE rather than a workaround — and it stays the coarse
gesture regardless, since it is the only one a wheel-less mouse has. Wiring Quarry's list to
`INPUT_EV_WHEEL` is a scoped follow-up, not a blocked one. Thumb **dragging** is not implemented —
`wm`'s drag machinery is title-bar-scoped and a content-drag protocol is its own arc. Keyboard
`Up`/`Down` is the fine gesture and auto-follows.

---

## 5. Geometry and cost

One accessor, `geometry(pw, ph) -> Option<Geom>`, and both the painter and the router read it — the
crispywire law `dock::Layout` states, so there is no second copy of the arithmetic to drift.

* Text scale follows the **panel**, and it is the one number here that is not a proportion: `ts = 1`
  below 1280 px wide, `ts = 2` at or above. QEMU raspi4b is 640 x 480 and the bench Pi is 1920 x 1200,
  and a legibility decision cannot be a fixed fraction of a surface that differs by 7.5x in area.
* Content surface: 3/5 of the panel, floored at `320 x 200`, ceilinged at `1152 x 720`, rounded down
  to a whole text cell, and checked against the panel minus `wm`'s chrome. Below the floor `open()`
  DECLINEs with `[quarry] DECLINE reason=panel-below-floor` — an unreadable window is worse than no
  window, which is `pidesk`'s own CONSOLEWIN reasoning applied to a second tenant.
* Resolved: **640 x 480 → 384 x 288 at 1x** (48 cols x 26 list rows), **1920 x 1200 → 1152 x 720 at
  2x** (72 cols x 33 list rows).
* Panes tile the surface exactly: tree = 5/16 of the width (floored at ten columns, capped at half),
  one pixel of divider, list takes the rest. The witness asserts the tiling rather than trusting it.
* Columns **degrade rather than overlap**: below 34 columns the date goes, below 22 the size goes too.
  A narrow pane shows names, not three columns of ellipsis.

The surface is a heap `Vec<u8>` sized from the live panel — `try_reserve_exact` with a
`[quarry] DECLINE reason=alloc` arm, the `panel_console_window_open` idiom — rather than a
`[u32; W * H]` static, because a static large enough for the bench panel is `.bss` that QEMU pays for
and never uses.

### 5.1 CONSOLEWIN, applied to a second tenant

`open()` carries `wcx`'s CONSOLEWIN law, unchanged in substance and unchanged in reason: if
`dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph)` is `None`, Quarry DECLINEs.

Quarry is an ordinary `wm` row, so the kernel draws it the ordinary control cluster, minimise disc
included. Minimise is a POSITION — the row drops below `SHELL_Z`, stops compositing, and the only
gesture that brings it back is the dock. Its own pinned tile is no escape from this: that tile is *in*
the strip that will not fit. A control that hides a window with no way back is worse than no window.
`MAX_WINDOWS` and not the live count, because the check must hold for every table state the boot can
reach.

**This is also what keeps the armed QEMU gate honest, and that is said here rather than discovered
later.** 640 x 480 cannot host a twelve-tile dock, so on QEMU raspi4b Quarry declines exactly where the
console window declines, and the video witness battery — which asserts exact panel pixels and knows
nothing of a file manager — is unperturbed. The bench panel hosts the strip, so the bench is where the
window opens, and the armed **bench-geometry** run (§10) is where its effect on that battery is
measured rather than assumed.

---

## 6. Wiring, and why every seam is where it is

| seam | file | note |
| --- | --- | --- |
| module declaration | `video/mod.rs`, **at the file tail** | below `pidesk`, for `pidesk`'s reason: a `mod` line inserted higher renumbers every panic `Location` below it and moves the knob-off `kernel8.img` hash. Nothing is below this, so nothing moves. |
| knob seam | `video/quarry.rs` | the module is compiled under the furniture gate (`any(all(x86_64, wc), all(aarch64, pidesk))`) so **both** arches type-check it; `feature = "quarry"` decides whether the name resolves to the implementation or to `#[inline(always)] false`. See below for why the knob is here and not on the call sites. |
| open | `video/pidesk.rs` step 6 | the Pi's DESKTOP-READY seam, **last** in the sequence: Quarry reads directories and every step before it is pure geometry or a flag, so a slow or declined volume cannot delay the menu bar's paint. This mints no second launcher — Quarry is not a program, it is kernel furniture, the same class as the console window `panel_console_window_open` mints thirty lines above. |
| keyboard | `arch/aarch64/syscall.rs`, `user_input_enqueue` | folded onto the existing `crystal::key_escape` one-liner, **after** it: an open SHARD menu is modal and its `Esc` beats Quarry's. Before `wc_focus_key`, and Quarry does not bind TAB, so the compositor's key is never hostage. |
| pointer | `arch/aarch64/syscall.rs`, `wc_click_route` press edge | folded onto the existing `strip::press_route` one-liner, **after** it (the strips composite on top of the window layer). Quarry asks `wm::hit_test` itself and acts only when the top-most window at the point is its own, so folding it in this early can never let it claim a press that landed on a window above it. |
| dock tile | `video/dock.rs`, `pin_quarry` | see §6.2. |

### 6.1 The line-neutrality constraint, and what it forced

`arch/aarch64/syscall.rs` is compiled into the knob-off `kernel8.img`. `PARITY.md` §5.3 records the
measurement that makes this binding: a **`cfg`-only** change to `wm.rs` still moved the knob-off hash
(`42355ca2…` → `1143ecc5…`), because panic `Location` records embed file line numbers and eleven added
*comment* lines shifted every location below them.

So both of Quarry's input hooks are folded onto lines that were already there — the diff in that file
is **line-neutral, 22 504 lines before and after**. And a folded call cannot carry a second `cfg` of
its own without becoming a second line, which is the whole reason the knob lives inside
`video/quarry.rs` as a stub/implementation split rather than on the two call sites where it would
otherwise belong. With `UNAOS_QUARRY` off, `quarry/live.rs` is not compiled, the stubs inline to
nothing, and the two edited lines are the same length they were.

### 6.2 The dock's pinned tile — and why this does not make the dock a launcher

The dock's stated ruling (`dock.rs`, Peter's white-board Q10) is that it is *"a window switcher, and
nothing else … NOT an app launcher"*, because there is exactly one launch path in this kernel (the
shell's program source / `bg`). That ruling stands and is not being quietly reversed.

`pin_quarry` adds no launch path, because **Quarry is not a program**. It is a kernel-owned window, and
its tile is a *reopen* route for exactly the reason `pin_shell` is the shell's: a window an operator
can close and cannot call back is a window they have lost. While Quarry is LIVE its real row is
dock-addressable (kernel owners are), so the model is unchanged and no pin is added — the pin exists
only while it is closed, precisely as the shell's does.

Peter's placement — *"pinned to the left side of the taskbar/dock so it opens like Mac's Finder"* — is
why this one **prepends** where `pin_shell` appends. The settled strip reads
`[quarry] [live windows…] [shell]`, which is the macOS order.

Applied by all three readers of the model (`compose`, `press_at`, `strip_rect`), so painter, router and
the occlusion registry cannot disagree about the tile count.

**The press LATCHES rather than opens.** `press_at` runs inside a click router and Quarry's open reads
directories, so the tile sets a flag and `quarry::service()` drains it from the arch's input-drain
task — after `pump_usb_into_gui` has dropped its xHCI loan, which is the same context the shell's own
`ls` reads a volume from. The drain call is folded into the same aarch64 router line as the rest, so
**x86 has no drain yet**: when the x86 wiring lands it must place `service()` off the render core, for
the reason `wcx::desktop_app_service` documents (the render core holds the xHCI lock).

---

## 7. The ring-3 port — what has to land, in order

Quarry moves to EL0 the day these are true. Nothing here is Quarry's own work.

1. **A window larger than 128 x 128.** Raise `FB_WIN_MAX_W`/`H` and the window-region slot size, on
   both arches, or add a multi-slot surface. This is the blocking one and it is a memory-map arc.
2. **`SYS_READDIR` (and `SYS_STAT`), routed through `MountTable`.** Numbers 34–39 are free. The
   authorization is already composed — `MountTable::open_read` authorizes then stats, per `vfs.md` §2.
3. **`SYS_OPEN` off `fat::mount()` and onto the mount table**, so ring 3 and the shell agree about what
   a path names. `vfs.md` §12.4 already names this as outstanding.
4. **The x86 VFS adoption**, which deletes §3.1's two shims.
5. **A userspace drawing library.** Every EL0 crate today depends on `una-abi` alone, which is
   `const`-only, and there are four separately hand-rolled bitmap fonts across four programs (none
   covering full ASCII). Quarry needs ~96 glyphs, a `fill_rect`, a clip and a blit.
6. ~~*(optional, for cost)* **A wheel event in the ABI** and un-ignoring `data_data[3]` in the xHCI HID
   decoder~~ — **landed** (WHEEL arc); what remains of this item is `SYS_WIN_PRESENT_ROWS` on both
   arches, so a scroll costs a row band rather than a box.

M2 adds two, both of which are about what a double-click on a NON-program should do:

7. **An opener registry**, and something to open a document WITH. There is none of either today: no
   association table, no viewer, no editor. A double-click on `CONFIG.TXT` therefore does nothing and
   says so on the wire (`[quarry] open UNHANDLED path=… — no opener exists in this tree`), which is
   the honest posture — "broken" and "not built yet" must be tellable apart.
8. **`SYS_EXEC` with an argv**, so an opener could be *handed* the path it is meant to open.
   `spawn_user_image_bg` takes an image and nothing else; a program launched from Quarry today is
   launched exactly as `bg` launches it, with no argument, which is why the launch path is limited to
   programs that need none.

A third item is a bookkeeping debt rather than a capability:

9. **A launch registry the shell and Quarry share.** `bg`'s job table (`BG_JOBS`) is `shell.rs`-private,
   and `shell.rs` is compiled into the knob-off `kernel8.img`, where an added line breaks the
   byte-identity proof (PARITY.md §5.3). So Quarry keeps its own small table and runs the *same*
   reaper (`bg_poll(pid, reap = true)`) on every gesture and every `service()` pass. The consequence
   is named rather than hidden: a Quarry-launched program does not appear in the shell's `jobs`, and
   `MAX_JOBS` (8) is the ceiling on how many it can have outstanding. The 9th launch reaps first and
   then declines out loud in the path bar.

---

## 8. The witness

`:: QUARRY: … :: PASS ::`, **ten** legs (M1's five plus M2's five), `witness`-gated, run from
`pidesk::activate` step 6 **before** `open()` — its legs are pure functions over synthetic input (no
panel, no volume, no window table), so a DECLINE in the thing it proves the arithmetic of must not be
able to skip it.

M2 also arms it. `pi4-regression.spec` carries

```
FORBID :: QUARRY: .* :: FAIL ::
```

so a regression in any leg reds **both** batteries (knob-off and armed alike — the fixture is
`witness`-gated, so it is present on both) instead of printing a FAIL nobody greps for. It is a
FORBID and not a REQUIRE for the standing arithmetic reason recorded beside `SHARD-PRESS` and
`SERIAL-FOCUS` in that spec: a REQUIRE would move the count this arc's DONE gate is stated against,
and would red a knob-off build for not carrying a knob it was never given. A FORBID costs nothing at
0 hits and still catches the regression. M1 shipped this fixture **unguarded**; that is the gap M2
closes.

| leg | claim |
| --- | --- |
| geometry | a 200 x 200 panel is DECLINED; 640 x 480 and 1920 x 1200 both resolve, at `ts` 1 and 2; neither exceeds its panel or the ceiling; and on both, the two panes **tile the surface exactly** — one pixel of divider, nothing over, both filling below the path bar |
| `scroll_follow` | clamped to the top for a leading selection, follows to the tail, never exceeds `scroll_max`, and never scrolls a list shorter than the viewport — then the same two invariants asserted over a **swept** `len x visible x sel` space, not three hand-picked cases |
| `thumb` | absent when everything fits; at the floor at scroll 0; **exactly flush with the end of the track at max scroll**; never overruns mid-range |
| tree splice | `subtree_len` stops at the sibling, and `collapse` is an exact inverse of the splice — the property a hand-rolled index walk gets wrong when a sibling follows the expanded row. Then the SELECTION's three cases: a highlight on a later sibling stays on it (a bare `sel > i` test drags it back onto the closing row — the defect this leg was written against), and a highlight inside the subtree lands on the row that closed over it |
| press-to-row | the router's row arithmetic **inverts the painter's**, in both panes, with the list's body correctly clearing its pinned header |
| **duplicate roots** (M2) | `prefix_claims` honours the resolver's boundary rule (`/usb` claims `/usb/a`, never `/usbfoo`); `root_prefixes` reduces the **live** table `["/", "/fat", "/usb"]` — the exact one that produced the bench's double `/fat` — to `["/"]`; it is **idempotent** and **order-independent**; and it does NOT hide a volume on a table with no root mount, nor drop a sibling that only shares a name prefix. That last pair is the leg that rejects the lazy fix ("just keep `/`") |
| **name dedupe** (M2) | `dedupe_by_name` keeps exactly the first of each name, in order |
| **launchability** (M2) | `is_executable` accepts `VUG.ELF` / `vug.elf` / `Vug.Elf` / `STAT.ELF` / `HELLO.BIN` / `midden.bin`, and refuses `KERNEL8.IMG`, `CONFIG.TXT`, `SRC.TGZ`, `START4.ELF.BAK`, a bare `.ELF`, a bare `.BIN`, `ELF`, and the empty name |
| **double-click** (M2) | `is_double` fires exactly at the window and not one ms past it; never pairs different rows or different panes; never fires on a **zero clock** (the guard that stops the first press of a boot launching on a board whose `CNTFRQ_EL0` reads 0); and rejects a backwards clock |
| **the cache** (M2) | the directory cache never exceeds `MAX_CACHE`, and evicts **oldest-first** |
| **the launch gesture, end to end** (M2) | two presses at a real pixel, through the **same `content_press` the click router calls**, produce `Act::Launch("/fat/VUG.ELF")`; a third rapid press does NOT re-activate (the stamp reset); presses on two different rows open nothing; and a double-click on `CONFIG.TXT` produces `Act::NoOpener` naming it. Both clock branches are asserted, so the leg is a real claim on a board whose `CNTFRQ_EL0` reads 0 rather than a claim on some boards |

Leg 11 deliberately stops at the **decision**: everything downstream of the `Act` is `bg`'s own
machinery, exercised on every boot by the BGRUN fixtures, and a fixture that actually spawned a
program would perturb the very window table the rest of this battery asserts exact pixels of.

It is arch-neutral and disk-free by construction, so it is honest on QEMU raspi4b (no stick) and on
x86 (no mount table at all) — the two surfaces where a volume-dependent leg would be vacuous. The
cache leg is deliberately driven against the model's own `Vec` rather than through `collect_cached`,
for the same reason: reaching the seam would make it a test of the machine's storage.

Runtime evidence, on the wire, from the seams rather than from a fixture. M2 adds three lines at open
and again on every `r`, and each one answers one bench complaint **in the terms it was made in** — so
a headless capture settles all four without a photograph:

```
[quarry] open win=<id> surf=<w>x<h> ts=<n> box=<w>x<h> at (<x>,<y>) volumes=<n> tree-rows=<n> list-rows=<n> cwd=<path>
[quarry] open volumes mounts=["/", "/fat"] roots=["/"] tree-rows=<n>          <- "/fat listed twice"
[quarry] open cost reads=<n> hits=<n> cycles=<n> cache=<n>/16 gen=<n>         <- "VERY SLOW"
[quarry] open census cwd=/fat entries=<n> dirs=<n> files=<n> names: KERNEL8.IMG(…) VUG.ELF*(12568) …
                                                                             <- "where is vug, where is the kernel"
:: QUARRY-LAUNCH: /fat/VUG.ELF — 12568 bytes, entry 0x…, pid=<n> asid=<n> DETACHED ::   <- "double-click should open the app"
[quarry] open UNHANDLED path=/fat/CONFIG.TXT — no opener exists in this tree
[quarry] reaped pid=<n> asid=<n> name=<path> — exited status=<n>
[quarry] press close win=<id> at (<x>,<y>)
[quarry] closed win=<id> paints=<n>
[dock]   press at (<x>,<y>) tile=<t>/<n> quarry=pin -> open requested
[pidesk] quarry open=<bool> — the file manager is a desktop tenant
```

---

## 9. Bounds

Every model is capped, because both inputs are shaped by something other than this module: a
directory's entry count is whatever the medium says, and the tree's depth is whatever the operator
clicks. The caps are the loop bounds.

| cap | value | behaviour at the cap |
| --- | --- | --- |
| `MAX_TREE` | 192 rows | enforced against the **whole** tree, not per level, so six expanded 200-entry directories cannot grow it without bound; further children are simply not spliced |
| `MAX_LIST` | 2048 entries | the model is a prefix and the path bar SAYS `(list truncated)` — a silently short listing is a lie about the medium |
| `MAX_DEPTH` | 8 | an expand at the ceiling marks the row open with nothing under it |
| `PATH_MAX` | 160 chars | a longer child is skipped rather than built |
| surface | 1152 x 720 | the repaint cost §4 states is bounded by this |
| `MAX_CACHE` (M2) | 16 listings | FIFO — the oldest path is evicted, never the model's growth |
| `MAX_JOBS` (M2) | 8 programs | the 9th launch reaps first, then declines in the path bar |

---

## 10. Gate results (2026-08-17)

| gate | result |
| --- | --- |
| `./arroyo check` | green, both arches. The `arm-pi` **and** `x86-all` legs both carry `quarry`, so the implementation is type-checked on both arches, not merely the one that runs it |
| `UNAOS_WC=1 ./arroyo check` | green, both arches |
| `./arroyo kernel8` knob-off | `kernel8.img` sha256 `c3000ff5bd87ed0e…` — **byte-identical to the pre-arc baseline** |
| `./arroyo kernel8-test 210` knob-off | **PASS 108/108**, 0 forbidden, 36967 lines (no other aarch64 QEMU on the host) |
| `UNAOS_QUARRY=1 UNAOS_PIDESK=1 ./arroyo kernel8-test 300` | **PASS 108/108**, 0 forbidden, 48850 lines. `:: QUARRY: … :: PASS ::`, and `[quarry] DECLINE reason=dock-cannot-host-full-strip panel=640x480` — §5.1's law, the same decline the console window takes on this panel |

**Host-load caveat, stated because it produced two misleading intermediate runs.** This host carries
other sessions' QEMUs. Two knob-off runs taken while three to five were live returned `108/108 with 4
forbidden` (a lone `[wc-h] torn=1 -> AT-RISK`) and `107/108 with 14 forbidden, 3666 lines scanned` — a
visibly starved capture. Both were taken against a `kernel8.img` that is **byte-identical to the
pre-arc baseline**, so neither can be this arc's: same bytes, different verdict, is the definition of a
host term. The numbers in the table are from runs with no other aarch64 QEMU on the machine. (Note
`pgrep -c qemu` is not the quiet test here — it counts the seat's own x86 sandbox VM, which never
exits; `pgrep -f qemu-system-aarch64` is.)

### The bench-geometry run, controlled

`UNAOS_FBW=1920 UNAOS_FBH=1200`, both runs back to back on the same (non-quiet) host, so the delta is
attributable to the knob and not to load:

| run | verdict | forbidden | lines |
| --- | --- | --- | --- |
| control — `UNAOS_PIDESK=1` (Quarry OFF) | FAIL 104/108 | 26 | 18106 |
| armed — `+ UNAOS_QUARRY=1` | FAIL 104/108 | 37 | 18228 |

**The battery already fails at bench geometry without Quarry**, at the identical 104/108 with the
identical missing REQUIREs. That is the standing conflict CONSWIN-PI M2 recorded (`engine.md`
§CONSWIN-PI: 104/108 with 13 forbidden on a quiet host) — the arm-pi witness battery asserts exact
panel pixels and was written against a Pi desktop with no extra tenant on it.

Quarry's contribution is +11 forbidden hits, all of the **already-diagnosed** class:

* `[wc-d] verify win=3 … first=(307,158) got=0x2d2b55 want=0xc3c3c3` — `0x2d2b55` is `wm::DESKTOP_BG`,
  and `(307,158)` is the *same pixel* CONSWIN-PI M2's commit records verbatim for the console row. The
  fixture window's id also moved `2 → 3`, because Quarry took a table row ahead of it. Occlusion, with
  the surface itself intact.
* `[wc-h] torn=3` / `[wc-k] torn=2` against the control's `torn=1` / absent — one more compositor
  client on a loaded host, which is what the CONSWIN-PI measurement predicted.
* `[dragperf] admitted=4` against the control's `admitted=2` — the same FORBID the control already hits.

The armed run also **passed two legs the control failed** (`[wc-c] side-by-side drawn=2`, `[wc-g]
RACE-BLIT`), which is the honest read on how load-dependent these numbers are: the control's own 26
forbidden against the 13 the quiet-host arc recorded is the size of the host term.

No spec REQUIRE or FORBID was touched or re-worded. Teaching `pi4-regression.spec` about a quarry row —
the way x86's spec was taught "win=1 is the console window on this bench" — is the integrator's
reconciliation, not this arc's.

---

## 11. M2 — the four fixes, in full

### 11.1 SLOW — the listing path, measured

**The measurement.** One `shell::vfs_ls_collect` is not one volume access. It is three, and none of
them is a directory read:

1. `vfs_ls_collect` opens with `vfs_mount_table()`, which calls
   `fat::mount_source(BlockSource::Usb)` to decide whether to bind `/usb` at all (the honest
   hot-plug posture, `vfs.md` §6). That is a full probe of the stick.
2. `MountTable::stat(path)` resolves to `FatBackend::stat`, whose **first line** is
   `fat::mount_source(self.source)`.
3. `MountTable::read_dir(path)` resolves to `FatBackend::read_dir`, whose first line is
   `fat::mount_source(self.source)` **again**.

And `mount_source` is not cheap. It reads LBA 0, runs `mbr_census`, attempts a superfloppy `parse_bpb`,
then `scan_gpt` (LBA 1 plus a partition-entry sector), then walks the accepted MBR entries parsing a
BPB sector for each — every one of those a real transaction on the SD or USB path, before a single
directory sector is touched. Both the `stat` and the `read_dir` then resolve the path from the volume
root independently, so the directory walk happens twice as well.

On top of that, M1's **model asked for the same directory twice per navigation**: `Model::expand`
collects a row's children for the tree, and `Model::show` immediately collects the identical path for
the list. Landing on `/fat` cost roughly **4 mount probes and 2 root-directory walks** where one of
each would do. That is the "VERY SLOW", and it is a per-CALL cost, not a per-ENTRY one.

**The fix: `Model::collect_cached`.** A path-keyed cache of the seam's own answer — `(is_dir, rows)`,
nothing derived — bounded at `MAX_CACHE` (16) entries with FIFO eviction. It caches *successes only*:
a `-ENODEV` volume is a state that ends when the operator plugs the stick in, and caching it would
need a fourth invalidation event to forget it.

Invalidated on exactly three events:

| event | why |
| --- | --- |
| **window open** | a fresh `Model` has an empty cache — this is the brief's "per open" |
| **the `r` key** | the refresh gesture that already existed and already meant "re-read the world" |
| **a volume generation change** | `drivers::block::usb_publish_gen()`, the block layer's own hot-plug epoch, advanced by every geometry publish and every retraction (PA35's storage race). It is one `Acquire` load of an `AtomicU64`, which is what makes it safe to ask on **every** access — asking `vfs_mount_table()` instead would cost the very USB probe the cache exists to stop paying |

`Model::mounts` holds the prefix list from the last `reload_roots`, so the landing rule and the tree
read it without a second `vfs_mount_table()` — and therefore without a second USB probe.

**Why not "bound and page it", the brief's other option.** Because it divides the wrong term. The cost
here is per-CALL setup (three volume probes and two path resolutions); a 12-entry FAT root and a
2000-entry one pay almost the same mount-scan toll. Paging would shrink a term that is already small,
leave the dominant one untouched, and put a page cursor into a model ORIN's UnaFS is about to re-back.
`MAX_LIST` already bounds the entry term and says `(list truncated)` on the glass when it bites.

**It is measured, not asserted.** The model counts its own misses, hits and cycles, and prints them:

```
[quarry] open cost reads=<n> hits=<n> cycles=<n> cache=<n>/16 gen=<n>
```

`reads` is seam calls actually made; `hits` is calls the cache answered. The line appears at open and
on every refresh, so "it is faster now" is a number in a capture rather than a claim.

### 11.2 TWICE — the duplicate-root rule

**The mechanism.** M1 made every mount prefix a depth-0 tree row (`mt.prefixes()` = `/`, `/fat`,
`/usb`) and then expanded `/` at open. But `vfs_ls_collect`'s "mount points immediately below `path`"
arm **synthesizes those same mount points as child rows** of `/`. So `/fat` was a depth-0 root AND a
depth-1 child of the root — one path, two rows, both real, both correct on their own terms. That is
what the bench saw, and it was a model fact rather than a rendering bug.

**Which row is wrong.** The ROOT row. The child row is where the path actually lives in the one
namespace, and it is what a person means by "inside my machine". So the rule removes roots, and it is
stated about namespaces rather than about this machine's three volumes:

> **A mount point claimed by another mount point is not a root; it is reached through its parent.**

`root_prefixes` implements it over `prefix_claims`, which restates the resolver's own boundary rule
(`/usb` claims `/usb` and `/usb/…` but never `/usbfoo`; the bare root claims everything) as a pure,
witnessed function — `MountTable`'s copy is private and this has to hold on both arches.

On a table carrying `/` it leaves exactly `["/"]`, which is also the honest shape of a single
namespace: one root, volumes hanging under it. On a table with **no** root mount — an arch that has
not adopted the VFS root, or a namespace assembled from peers — it leaves every unclaimed prefix, so
the tree still has roots and nothing is hidden. That last property is what rejects the lazy fix
("just keep `/`"), and the witness asserts it.

A second, independent guard sits at the splice: `Model::expand` refuses to insert a child whose path
the tree already carries. `root_prefixes` removed the way this happened; the splice guard means no
future source of tree rows can reintroduce a duplicate path without tripping over it. `dedupe_by_name`
does the same job on the list pane's rows.

### 11.3 LAUNCH — double-click, and the constant that did not exist

**The gesture.** Two presses on the **same row of the same pane** inside `DOUBLE_CLICK_MS`. The row
test is what makes it a gesture rather than a timer: a rapid press on row 3 then row 4 is two
selections, which is what an operator scanning a list is doing, and it must never run anything. A
consumed double-click **resets** the stamp rather than re-arming it, so three fast presses are one
double-click and one fresh single — never two overlapping activations. A press in the tree stamps the
click too, so a tree press and a list press can never combine.

**The constant, honestly.** *Nothing in this tree had one.* There is no double-click anywhere in the
kernel, no `DOUBLE_CLICK`/`DBLCLK` constant in any driver or compositor file, and the xHCI HID
boot-mouse decoder publishes button transitions with no timing attached at all. So `DOUBLE_CLICK_MS`
is minted here, and three facts fixed it at **400 ms**:

* the CLOCK is `arch::ms()`, which on aarch64 is `CNTVCT_EL0 / (CNTFRQ_EL0/1000)` — derived from the
  free-running counter, **not** from `ticks()`. That matters: on QEMU raspi4b the periodic timer IRQ
  is never delivered and `ticks()` stays frozen at 0 (UVUG-7's measurement), so a tick-derived clock
  would have made every gate here vacuous on the QEMU battery;
* the mouse arrives over USB HID at boot-protocol rates, so two deliberate clicks land tens of ms
  apart at best — a window under ~200 ms would drop real double-clicks on a busy compositor pass;
* 400 ms sits between the classic desktop defaults (macOS ~450 ms, Windows 500 ms) and the low end,
  and is deliberately on the short side because Quarry's single click is **not inert** — it selects —
  so a too-long window makes a slow re-select feel like an accidental launch.

The predicate itself, `is_double`, is pure and witnessed, so the number above is the only part of the
gesture that is a judgement call. Its zero-clock guard is not pedantry: `arch::ms()` legitimately
answers 0 before the timebase is up, and forever on any board whose `CNTFRQ_EL0` reads 0. Without the
guard, the **first click of a boot** on such a machine would launch whatever it landed on.

**The launch, through the shell's own seam.** `launch()` reads the image through the VFS mount table
(never `fat::mount()` — this arc's standing law) and hands it to
`arch::syscall::spawn_user_image_bg` — the exact call the shell's `bg` verb makes, with the same
bounds, the same console-cap endowment and the same DETACHED posture. Quarry mints no loader and no
policy: the extension test is a **routing** hint, and every real check (ELF magic, `EI_CLASS`,
`e_machine`, segment bounds, the 16 KiB user window) is the loader's. A `.ELF` that is not one is
refused in the loader's own words.

Two bounds, stated rather than discovered:

* it does **not** call `shell::read_el0_image`, though the gates below are that function's in the same
  order and vocabulary. That function takes a `&mut Console` and prints into it, and it lives in
  `shell.rs` — compiled into the knob-off `kernel8.img`, where splitting out a console-free core would
  ADD lines and break the byte-identity proof (PARITY.md §5.3). The shared thing is the seam that
  matters — the mount table for the read, `spawn_user_image_bg` for the spawn — not the printing;
* `Act` carries the decision **out of the `MODEL` lock** before it is acted on. `spawn_user_image_bg`
  reserves a `Proc` row, maps an address-space slot and calls `spawn_user_slot`; doing that with a
  repaint-path spinlock held would put Quarry's model underneath the scheduler for no reason.

**A non-executable double-click does nothing, and says so.** There are no openers in this tree: no
association registry, no viewer, no `SYS_EXEC`-with-argv to hand a program a path with. So the census
line is the deliverable —

```
[quarry] open UNHANDLED path=/fat/CONFIG.TXT — no opener exists in this tree (launchable = .ELF/.BIN
via spawn_user_image_bg; a document needs the opener registry named in quarry.md 7)
```

— because an operator who double-clicks `CONFIG.TXT` and sees nothing must be able to tell "broken"
from "not built yet". The path bar says `no opener for CONFIG.TXT` on the glass at the same time.

**Reaping.** §7 item 9 has the bookkeeping debt in full: `bg`'s `BG_JOBS` is `shell.rs`-private, so
Quarry keeps its own `MAX_JOBS`-bounded table and runs the **same** reaper — `bg_poll(pid, reap = true)`
— on every gesture and every `service()` pass. A Quarry-launched program therefore does not show up in
the shell's `jobs`, and that is named, not hidden.

### 11.4 TRUTH — the landing rule

**The measurement.** `arroyo`'s own staging step builds the native UnaFS volume with exactly two files
on it:

```
✅ [OPERATOR] Wrote '…/K3HELLO.TXT' to '//K3HELLO.TXT'
✅ [OPERATOR] Wrote '…/K3PAT.BIN'   to '//K3PAT.BIN'
```

…while `/fat` is the boot card: `KERNEL8.IMG`, `VUG.ELF`, `CONFIG.TXT`, `SRC.TGZ` and the firmware.
M1 opened on `/` (the first sorted prefix) and showed two files. It was **not** hiding the kernel's
own files from the owner and it was not truncating or filtering anything — the collector returns every
entry the medium has, and the only filter anywhere in the path is FAT's `.`/`..` on-disk artifacts,
which are not names in this namespace. M1 had simply landed on the emptiest volume there was, which is
exactly as useless as being wrong.

**The rule (`Model::landing`).**

> Open on the root; unless one of the root's **immediate mount points** carries strictly more plain
> FILES than the root does, in which case open on the richest of them.

It names no volume, no filesystem and no extension. When the native volume is the one carrying the
system — which is where ORIN is taking this — the same rule lands on `/` and the behaviour inverts
without a line changing. It is bounded by the mount count (three today) and one level deep by
construction, and every probe goes through `collect_cached`, so the listing it chooses is already in
hand for `show` and for the tree: the rule's marginal cost on this machine is **one** directory read,
of a volume the operator is about to be looking at.

"Strictly more" is doing real work: it makes the root the default and the descent the exception, so a
machine whose namespace has content at the top is never dragged into a volume by a coin flip. A tie
keeps the root.

The listing itself is then put on the wire, by name and size, so "where is vug, where is the kernel"
is answerable from a headless capture:

```
[quarry] open census cwd=/fat entries=<n> dirs=<n> files=<n> truncated=false names: KERNEL8.IMG(…) VUG.ELF*(12568) …
```

And on the glass, the `*` mark tells the operator which of those rows a double-click will run — a
window that starts programs must show which rows start programs, rather than making that discoverable
only by double-clicking.

### 11.5 The namespace-agnostic constraint

Peter's coordination note: ORIN lays out UnaFS in days, so M2 must build nothing FAT-specific beyond
the VFS seam. Where each fix satisfies that:

| fix | what it is written against | what would have been wrong |
| --- | --- | --- |
| the cache | absolute paths, and `usb_publish_gen()` — the **block layer's** hot-plug epoch, which means "the set of block devices changed" on any filesystem | a FAT mount cache, or a cache keyed on a cluster number |
| the duplicate-root rule | mount **prefixes** and the resolver's boundary rule | special-casing `/fat`, or hardcoding "the root is `/`" |
| the launch | `NodeKind`, `Stat::size`, `MountTable::read`, and the arch's own loader | reading the image through `fat::mount()` — the raw-backend path this arc is forbidden to take |
| the landing rule | "plain files per immediate mount point" | "open on `/fat`", which would be wrong the day the native volume carries the system |

The one `target_arch` gate is §3.1's, unchanged in substance: `fs/vfs.rs` gates both backends to
aarch64, so on x86 `collect` and `launch` are shims that SAY so. Every pure part of M2 —
`prefix_claims`, `root_prefixes`, `dedupe_by_name`, `is_executable`, `is_double`, the cache bound —
compiles and is witnessed on both arches.

---

## 12. M2 gate results (2026-08-17)

| gate | result |
| --- | --- |
| `./arroyo check` | **green**, both arches (`x86-all` and `arm-pi` both carry `quarry`, so M2 is type-checked on the arch that cannot run it too) |
| `UNAOS_WC=1 ./arroyo check` | **green**, both arches; `✅ kernel cfg coverage OK (12 legs)` |
| knob-off `kernel8.img` byte-identity | **proven, in place.** `2d9f9ab347106102ce2b4a26eca71c0e54970e875f5600de38b0e7264c81557d` with M2's `live.rs` and with the pre-arc one, built in the **same directory** from the same command. M2 touches exactly one source file and it is behind `#[cfg(feature = "quarry")]` |
| `./arroyo kernel8-test 210` knob-off | **PASS 111/111, 0 forbidden**, 21151 lines |
| `UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo kernel8-test 300` | FAIL 106/111, 15 forbidden — **and its paired control is worse** (see below) |
| `:: QUARRY: … :: PASS ::` | on every armed run, eleven legs, `dbl=400ms cache=16` |
| `FORBID :: QUARRY: .* :: FAIL ::` | **0 hits on every battery**, knob-off and armed |

**Attribution of the armed battery's deficit — Quarry's delta is zero.** The armed run was paired
against a control (`UNAOS_PIDESK=1` alone, Quarry not compiled) on the same host, back to back:

| run | verdict | forbidden | quarry lines in log |
| --- | --- | --- | --- |
| control — `UNAOS_PIDESK=1` (7 aarch64 QEMUs on host) | FAIL 105/111 | 16 | none |
| armed — `+ UNAOS_QUARRY=1` (7 QEMUs) | FAIL 105/111 | 16 | present |
| control — `UNAOS_PIDESK=1` (4 QEMUs) | FAIL 105/111 | 16 | none |
| armed — `+ UNAOS_QUARRY=1` (4 QEMUs) | FAIL **106**/111 | **15** | present |

Same verdict, same 16 hits, and the **identical six missing REQUIREs** in every case —
`[wc-c] side-by-side drawn=2`, `[wc-f] twin`, three `[wc-j]` legs, `[clickroute] hit-test`. That is
the standing `UNAOS_PIDESK` conflict §10 already records: the `arm-pi` witness battery asserts exact
window ids and exact panel pixels, and was written against a Pi desktop with no extra tenant on it.
The knob-off battery on the same host is **111/111 with 0 forbidden**, which is what makes the
attribution clean rather than a guess.

### The bench-geometry run, controlled

`UNAOS_FBW=1920 UNAOS_FBH=1200` — the panel Peter actually has:

| run | verdict | forbidden |
| --- | --- | --- |
| control — `UNAOS_PIDESK=1` | FAIL 107/111 | 233 |
| armed — `+ UNAOS_QUARRY=1` | FAIL **108**/111 | **160** |

The armed run is strictly better than its own control, on both terms. Bench geometry already fails
without Quarry, exactly as §10 recorded for M1, and the size of the host term (233 vs 160 forbidden
between two runs differing only by a knob that adds a window) is the honest measure of how
load-dependent these numbers are.

### What the wire actually said

Both armed runs, 640 x 480 and 1920 x 1200, landed the same way — the four complaints, answered in
one capture each:

```
:: QUARRY: geometry+scroll+tree+hit+dedupe+exec+dblclick+cache+launch — 640x480 surf_px=110592
           1920x1200 surf_px=829440 dbl=400ms cache=16 :: PASS ::
[quarry] open win=2 surf=1152x720 ts=2 box=1162x764 at (379,183) volumes=1 tree-rows=3
         list-rows=21 cwd=/fat
[quarry] open volumes mounts=["/", "/fat"] roots=["/"] tree-rows=3
[quarry] open cost reads=2 hits=3 cycles=2963625 cache=2/16 gen=0
[quarry] open census cwd=/fat entries=21 dirs=1 files=20 truncated=false names: OVERLAYS/
         B11.BIN*(16) CONFIG.TXT(842) DEFER2.BIN*(16) ELFHELLO.ELF*(8560) FIXUP4.DAT(5499)
         FRESH.BIN*(16) GROW.BIN*(528) HELLO.BIN*(51) K2IMP.BIN*(74) K2OWN.BIN*(128)
         KERNEL8.IMG(2028640) ...
```

Read against §0's table, line by line:

* `roots=["/"]` against `mounts=["/", "/fat"]` — **one** row per volume. The duplicate is gone, and
  the line proves it without a photograph.
* `reads=2 hits=3` — five collect calls, two of which touched a volume. The three the cache answered
  are the ones M1 paid for twice. **Landing on `/fat` under M1 cost four seam calls** (two to open on
  `/`, two more when the operator clicked into `/fat`, and every one of them three volume probes);
  M2 lands there in two, and every navigation after that is free until something changes. The
  `cycles` term is the honest scale of a miss: ~2.96 M CNTVCT ticks for two reads at the Pi's 54 MHz
  counter is **~27 ms per directory read** on QEMU, and the SD path on metal is not faster.
* `cwd=/fat` with `KERNEL8.IMG`, `CONFIG.TXT` and a screen of `*`-marked programs — the landing rule
  chose the card over the two-file native root, without being told what a card is.

---

## 13. FATFIX — the `/fat` duplicate re-checked, and the cost put on the wire (2026-08-18)

Three items came into this arc from the PA42/PA44 metal sittings, by way of baton `pi-1` §8. They did
not all turn out to be the same kind of thing, and saying so is the arc's first deliverable.

| item | verdict |
| --- | --- |
| 8b — *"/fat is LISTED TWICE in the tree"* | **already fixed.** §11.2's duplicate-root rule landed in `54ab30b9` (Quarry v2) and holds. Re-measured this arc on the wire; no code changed. §13.1 |
| 8a — *"FAT contents VERY SLOW"*, plus the double-click launch delay | **instrumented, not fixed.** The cost is now a measurement rather than an argument: `[fatperf]`. §13.2 |
| 8d — *"the USB reader shows NO FILES"* | **not a wiring gap.** The path from an enumerated disk to a Quarry row is complete and each of its two gates is honest. §13.3 |

### 13.1 The duplicate is gone, and this is how that was checked rather than assumed

Baton item 8b was written against **late boot5**, which was Quarry **v1**. §11.2's rule shipped after
it. So the honest first move was to re-run the complaint before repairing it, and an armed QEMU run
(`UNAOS_QUARRY=1 UNAOS_PIDESK=1`, bench geometry) answers it in one line:

```
[quarry] open volumes mounts=["/", "/fat"] roots=["/"] tree-rows=3 (a mount claimed by another mount is not a root — that is the duplicate-/fat rule)
```

Two mount prefixes, **one** root, three tree rows — `/`, `fat`, and `OVERLAYS` under it. `/fat`
appears once. The property behind that line is not a QEMU accident either: `selftest` leg 6 asserts
`root_prefixes(["/", "/fat", "/usb"]) == ["/"]` — *exactly* the mount table of a Pi with a stick in
it, the shape the bench actually had — plus idempotence, order-independence, the rootless-namespace
case and the `/usb` vs `/usbfoo` boundary. Leg 7 asserts the list pane's `dedupe_by_name` beside it.

**No code was changed for this item.** A second dedupe layered over a working one would have been a
change that could only ever hide a future regression from the witness that is meant to catch it.

### 13.2 `[fatperf]` — the listing and launch cost, in raw words

Peter's two speed complaints are about different code and were being answered with the same shrug.
The launch half is already half-convicted: this round's wire showed `spawn`→first-present is fast,
which leaves the **read**. So this arc builds the instrument, and deliberately stops there.

```
[fatperf] op=list path=/fat sectors=21 us=9772
[fatperf] op=read path=/fat/VUG.ELF sectors=33 us=12525
```

Four raw words, one line per operation, no derived value:

* `sectors` — 512-byte sector reads the block layer actually performed, counted at the FAT driver's
  **two read funnels** (`fat::read_sector` and `fat::read_sectors`), so a chunked multi-block
  transfer counts the sectors it moved rather than counting as one call. It is a delta of a global
  census across the operation, which means an unrelated FAT read landing inside the window is
  attributed here; on this kernel's serialised boot that is rare, and it is stated rather than
  papered over.
* `us` — wall time across the whole VFS operation, which is what an operator waits through. It
  therefore **includes** `FatBackend::read_dir`/`read`'s first line, a full `fat::mount_source`
  volume probe (LBA 0, the MBR census, the superfloppy BPB attempt, the GPT scan, a BPB sector per
  accepted partition). That probe is the per-CALL term §11.1 convicted, and it belongs inside the
  bracket for exactly that reason.

The clock is `CNTVCT_EL0` via `arch::now_cycles()` and `timer::cntfrq()`, **not** `ticks()`: on QEMU
raspi4b the periodic timer IRQ is never delivered and `ticks()` stays frozen at 0 (UVUG-7), so a
tick-derived elapsed time would print `us=0` for every operation on the entire QEMU battery — the
instrument would be vacuous precisely where it is first exercised. This is the same reasoning, and
the same pair of registers, that `DOUBLE_CLICK_MS` needed in §11.3.

Files: `unaos/crates/kernel/src/fs/fatperf.rs` (the whole instrument), `fs/mod.rs` (the wrapper),
and four call sites — two funnels in `fs/fat.rs`, two brackets in `fs/vfs.rs`.

**It is an instrument, not a fix, and that boundary is deliberate.** No cache, no refusal, no
behavioural change. The repair — cluster-chain caching, or migration onto ORIN's UnaFS — is a later
arc because the baton forbids building on the FAT namespace twice, and a repair chosen before the
cost is measured is a guess. §11.1's `[quarry] cost` line counts Quarry's *calls*; `[fatperf]` counts
what one call costs. The fix arc needs both terms and now has them separately.

**Byte-identity, and the part of it that had to be measured.** The knob is `UNAOS_FATPERF=1` (cargo
feature `fatperf`); OFF, `fs/fatperf.rs` is not compiled and `kernel8.img` is byte-identical to
baseline (`3a280f9d…` both ways, 1301840 bytes). Getting there cost two builds and taught something
worth writing down. The first form of the sector counter was an `#[inline(always)]` shim,
`fs::perf_note_sectors(n)`, called from the two funnels. Knob-off it inlines to nothing, and the two
call sites were **line-neutral** edits to lines that already existed — the discipline `arroyo`'s
PI-DESK block states, because panic `Location` records embed line numbers. The image moved anyway:
`3a280f9d…` → `08535f64…`, same length, **11997 bytes different**. An empty `#[inline(always)]`
function is still a *call* in MIR, and `read_sector` is small and inlined through most of the FAT
driver, so one extra MIR statement moved the inliner's cost decision and the drift cascaded. The fix
is that the call must not exist knob-off at all: `fat.rs` carries the `#[cfg]` on the **statement**,
which is gone before MIR. So the discipline is longer than it was written:

> A knob-off-compiled file must be line-neutral **and** MIR-neutral — a shim that inlines to nothing
> is not the same as a call that was never there — and the identity is re-measured, never reasoned
> about.

The `vfs.rs` bracket keeps the wrapper form, because measurement showed it costs nothing there:
`MountTable::read_dir`/`read` are not the inline candidates `read_sector` is.

### 13.3 `/usb` — the path is complete; both of its gates are honest

Baton item 8d asked whether an enumerated disk ever reaches the Quarry tree. Structurally it does,
and the chain has no missing link:

1. `drivers/xhci/mod.rs:10954` — a completed SCSI bring-up calls
   `drivers::block::publish_usb_geometry(dev_info)`.
2. `drivers/block.rs:639` — that records the stick under `USB_BLOCK_DEVICE` (deliberately *beside*
   the global `BLOCK_DEVICE`, which the microSD owns on the Pi), advances `USB_PUBLISH_GEN`, and
   raises the storage-ready edge. `read_block_usb` reads through that handle.
3. `shell.rs:5113` — `vfs_mount_table()` binds `/usb` **iff**
   `fat::mount_source(BlockSource::Usb).is_ok()`.
4. `shell.rs:vfs_ls_collect` — listing `/` synthesizes every bound mount prefix immediately below it
   as a child row, so `usb` is a row of `/`.
5. `quarry/live.rs` — `Model::expand` splices that row into the tree; `root_prefixes` keeps it a
   child of `/` rather than a second root (§11.2).

So the absence on late boot5 is one of exactly two honest refusals, and the wire distinguishes them:

* **step 1 never happened** — the wire showed no enumeration lines at all (no `[piusb25]`, no
  `BOT: PARKED`), so no geometry was ever published and there was nothing to mount. This is the
  xHCI/BOT bring-up's business, an off-limits seam for this arc, and it is reported rather than
  touched.
* **step 3 refused** — the baton's own note is that the card's content reads as raw zeros plus a
  `0x55AA` signature, which is not a FAT. `mount_source` then honestly finds no volume and `/usb` is
  not bound, which is the hot-plug posture `vfs.md` §6 asks for. `vfs_ls_collect` reports
  `volume /usb not mounted (-ENODEV)` for a direct `ls /usb` (VFS-4), and Quarry's `r` gesture
  re-reads the mount table so a later plug appears without a reopen.

**No fix was made**, because there is no gap in `fs/vfs.rs` or the collector to fix. When the stick
next enumerates on the bench, `[fatperf] op=list path=/usb …` will appear beside the `/fat` lines and
say what the stick's own listing costs.

### 13.4 Gate

* `./arroyo check` and `UNAOS_WC=1 ./arroyo check` — green, both arches. The `arm-pi` leg carries
  `fatperf`, so the knob's ARMED polarity is type-checked and not merely its absence.
* Knob-off `./arroyo kernel8` — `3a280f9dcbb32145…`, 1301840 bytes, **byte-identical to the
  pre-arc baseline**, measured before and after rather than argued.
* Knob-off `./arroyo kernel8-test 210` — `✅ MBENCH PASS — 117/117 required witnesses, 0 forbidden
  hit(s), 17807 lines scanned`.
* Armed `UNAOS_QUARRY=1 UNAOS_PIDESK=1 UNAOS_FATPERF=1` at bench geometry
  (`UNAOS_FBW=1920 UNAOS_FBH=1200`) — the `[quarry]` and `[fatperf]` lines quoted above. **This
  configuration is red on the required count, and it was red before this arc too** (baseline, same
  host, same geometry, no changes applied: 116/117 and 102 forbidden hits; after: 115/117 and 106).
  Every residual line is in the compositor lane — `[wc-g] … COHER`, `[wc-c] side-by-side drawn=1`,
  `[wc-h]`/`[wc-k] AT-RISK`, `[dragperf] … coalesced=0` — the documented-known classes the PA44
  ledger already names, and the host was running four concurrent QEMU sessions throughout. Nothing
  filesystem-side appears in it, and the knob-off byte-identity is the stronger statement anyway:
  the image an operator boots without this knob is the same bytes it was.
