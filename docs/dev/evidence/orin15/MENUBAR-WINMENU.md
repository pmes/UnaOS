# MENUBAR-WINMENU — the focused window's menus live in the MENU BAR (R21)

Orin 15, executors `exec-orin15-menubar` (drafted, killed by the session limit at the gate step) and
`exec-orin15-menubar2` (gated, measured, committed). Base sha **`191823c2`** (hw-jetson tip at the
time of writing). Patch: [`MENUBAR-WINMENU.patch`](MENUBAR-WINMENU.patch) — `git apply --check` at
`191823c2` → exit 0.

**This is a PATCH deliverable, not a landing.** The files it touches are rmbp's lane
(`video/menubar.rs`, `video/pulsewin.rs`, `video/wm.rs`, `video/strip.rs`, and by extension the
rest of `video/` and the two `syscall.rs` routers). The orin seat built and gated it in its own
worktree and then **reverted every `video/` edit before committing**, so this branch is docs-only and
merging it lands no kernel change; the rmbp seat applies the patch. §6 answers rmbp 12's grant
conditions A–J one at a time, each with its measurement; §7 names the two things found while
answering them that this patch deliberately does not fix.

**Three results a reviewer should not have to hunt for, two of them negative.** (1) The knob-off Pi
`kernel8.img` is **byte-identical** before and after — but only after a mid-file `video/mod.rs`
insertion found during the gate pass was moved to the file's tail (§3). (2) The x86 leg is green and
**does not exercise the bar path at all** — the publish/open/pick half is linked away as unreachable
on x86, and the banner cannot tell you that (§4). (3) `[wc-w] amp` moved against the stated
prediction, and a second baseline run shows the counter is not stable enough on this leg to carry the
claim either way (§4).

---

## §1 — The ruling, and the defect it names

Peter, [`docs/dev/RULINGS.md`](../../RULINGS.md) R21 (2026-09-06), on seeing the pulse window's
`View` menu drawn as the first row of the window's own content:

> WHO PUT THE GOD DAMN MENU IN THE WINDOW IT GOES IN THE ----- GOD DAMN MENU BAR

Menus belong in the menu bar, never inside a window. ONE-OS: arch-neutral, the same on x86 under
`wc`, on the Pi and on the Orin.

**The defect's own header said why it was built that way, and it is worth quoting because the
PREMISE was correct.** `video/pulsewin.rs` §"The menu, stated plainly" (PULSEWIN `27922509`):

> This kernel has exactly one menu framework — `crystal`'s SHARD dropdown — and it is hard-wired to
> the menu bar's brand mark … It is not a per-window menu and generalising it into one is a
> different arc from this one. So the pulse window carries its own menu strip as the first row of
> its own content.

The premise was a true statement about the code. The conclusion drew the wrong thing from it: one
menu framework hard-wired to one publisher is an argument for giving the framework a **second
publisher**, not for building a private one inside a window. This patch is that generalisation.

Two ledger rows close on it:

* **A25** — this row. `docs/dev/OS/orin-ledger.md`.
* **A10** — *"`<Esc>` cannot dismiss the pulse window's menu"*. The cause is now visible: before
  this patch `pulsewin::key_escape` existed and had **no caller anywhere in the tree** (the aarch64
  router asked `crystal::key_escape` and the x86 router asked `crystal::key_escape`; neither ever
  asked pulsewin's). It was a function that could not fire. It is deleted here, and Escape is wired
  through a shared seam that both routers ask — see §2.5.

---

## §2 — The design

### 2.1 A new module, `video/winmenu.rs` — the registry

```rust
pub fn publish(owner: wm::WinId, titles: &'static [MenuTitle], on_pick: fn(u32)) -> bool
pub fn clear(owner: wm::WinId) -> bool
```

* **Static trees only.** `MenuTitle { label: &'static str, items: &'static [MenuItem] }`,
  `MenuItem { id: u32, label: &'static str, flags: u32 }`. No allocation anywhere on this path.
* **The caps are the protocol's**, taken verbatim from `menubar.rs`'s design ledger so a tree legal
  in this registry is legal in the eventual wire form: `MENU_LABEL_MAX = 24`, `MENU_DEPTH_MAX = 2`
  (enforced *structurally* — a title holds items, an item holds nothing — so the kernel needs no
  walk and no parser), `MENU_ITEMS_MAX = 64`. Plus one cap that is the BAR's rather than the wire's:
  `MENU_TITLES_MAX = 4`, which is what a 34 px strip carrying a caption and a clock has room for.
* **The caps are REFUSALS, not truncations** — the protocol ledger's own falsification leg 3. Every
  refusal prints one named line: `reason=no-window | titles | title-label | item-label | items |
  registry-full | registry-contended`.
* **`FLAG_CHECKED` moves without mutation.** A publisher that wants the mark elsewhere re-publishes
  the OTHER `const` tree; the registry replaces in place, keyed by window id. That is how every
  tree stays `&'static` and the paint path never reads a tree a second core is halfway through
  writing.
* **Locks.** The slot table is a `spin::Mutex`, and *every* acquisition on the compose and input
  paths is a `try_lock` whose failure prints a refusal and declines the pass — LOCKFIX `7847ceea`'s
  rule and `strip::paint`'s decline-and-retry shape. Before that, `LIVE` (a relaxed count of
  published trees) short-circuits: a boot in which nothing ever publishes touches no lock at all.
  `has_tree()`, which the bar asks once per window per compose from inside `wm::dock_scan`'s
  existing scan, is lock-free by construction (`WINMENU_MAX` relaxed loads of an `AtomicU32` array)
  — a lock there would be nested under `wm`'s table.

### 2.2 The bar composes the focused window's titles

`menubar::compose` already runs one `wm::dock_scan` for the caption. The same scan now yields a
second reduction, and `Model` carries both.

**⚠ The two reductions are NOT the same, and the difference is a fact about this kernel rather than
a taste call.** The caption's `focused` flag is an **owner-ASID** match, and the click router hands
**shell focus (asid 0)** to a press on kernel furniture (`wm::is_kernel_owner`). So the first
publisher this arc has — `pulsewin`, whose `OWNER` is `KERNEL_OWNER_BASE + 0x60` — can *never* be
`focused` by that test, and an owner-keyed menu selection would show nothing for the one window it
exists to serve. The menu owner is therefore **the frontmost VISIBLE window that has published a
tree** (max `z` over `dock_scan`'s rows where `visible && winmenu::has_tree(id)`), which is what an
operator means by *the window in front*. It is stated on the wire (`[winmenu] publish owner=`,
`[winmenu] rollup … bar_owner=`) so the two readings can be compared rather than confused.

**Layout.** `menubar::menus_x0()` = `TITLE_X0 + (wm::MAX_TITLE + 1) * CELL_W` — a **fixed** slot for
the caption, not the caption's rendered width. If the titles began after the rendered caption they
would slide left and right every time the focused window changed, and a press would then be judged
against a layout the operator was not looking at when they aimed. macOS's bar has the same property
for the same reason; the difference is only that this kernel's caption is bounded, so the slot can
be a `const`. Titles run left to right, each `label.len() * CELL_W + 2 * TPAD` wide (`TPAD =
strip::PAD / 2`), by the bar's full height. A title that would reach `menubar::menus_right_limit()`
— the clock's left edge less one `PAD`, derived from the same two terms `compose_row` draws the
clock at — is **DROPPED, not squeezed** (the strip constructors' decline rule).

**Paint.** `winmenu::draw_bar_row` is called from `menubar::compose_row` with that function's own
scratch row, so the titles are part of the bar's single paint rather than a second surface stacked
on it. It runs BEFORE the text-band early-return, because an open title's box is filled for the
bar's whole height (bar its keyline) so the dropdown reads as hanging from a lit title. The text
band (`ty0`) is handed in rather than recomputed, so a title's baseline cannot drift from the
caption's.

**Damage.** `BarSnapshot::signature()` folds into `Model::signature()`. Without it, a title
appearing, moving, being relabelled or OPENING would leave a stale bar on the glass for as long as
the caption and the clock happened not to move — minutes, on a quiet desktop.

**Cost.** `BarSnapshot` is a `Copy` struct taken ONCE per compose and handed to the row painter, so
a 34-row paint costs one registry read rather than thirty-four. It is also what `press_at`
hit-tests and what `open_rect` anchors to: one function, three readers — `crystal_box_abs`'s rule.

### 2.3 The dropdown — the SHARD primitive, a second surface

`winmenu::compose` is `crystal::compose`'s shape: `strip::paint` / `strip::erase_rect`, a
`strip::Slot` damage slot, a `strip::Ledger`, the CLOBBER-REPAIR `dock_scan` condition, the
CRYSTAL-DISMISS erase-before-clear ordering, and `repaint_vacated` (`wm::damage_intersecting` +
`screen::request_full_present`). It runs from `strip::compose_all` at the furniture tail, **after**
the SHARD menu, so a window menu is the topmost surface while it is down.

The row METRICS are not restated — `crystal.rs` now re-exports them (`DROP_BORDER`, `DROP_ITEM_H`,
`DROP_SEP_H`, `DROP_PADX`, `DROP_CELL_W`, `DROP_CELL_H`, `DROP_FACE`) and `winmenu` imports them
under a `const _` assert that fails the BUILD if the import ever stops agreeing. A second copy of
"how tall is a menu row" is two things that can drift, and they drift SILENTLY — the only symptom
is a pick landing one row off the item the operator pressed. Deliberately NOT re-exported:
`ROWS`, `menu_rect`, `item_at`, `OPEN` — those are the SHARD menu's own model and modal state, and
sharing them would make the two surfaces one surface with two names.

A two-glyph check column precedes every label so a menu with one marked item still lines its labels
up; `FLAG_CHECKED` draws `>` there (`pulsewin`'s own argument, kept: the chrome face is the only
type this surface has, and a second glyph source for one checkmark would be a font for a
checkmark). `FLAG_DISABLED` draws in `TITLE_TEXT_INACTIVE` and is not pickable.
`FLAG_SEPARATOR` draws crystal's inset keyline.

### 2.4 ⚠ ONE dropdown at a time — the load-bearing invariant

`wm::MENU_OCC_MAX == 1`. That budget is the capacity `occ_clip` and `erase_clip` reserve for "the
open transient dropdown", and `OCC_MAX`/`OCC_CLIP_MAX` are `const`-asserted against it. A second
modal surface that could be open *at the same time* as the SHARD menu would need that widened in
`wm.rs`. **This patch does not widen it, and does not need to**, because the two are mutually
exclusive by construction:

1. `winmenu::press_at` runs **FIRST** in `strip::press_route`, ahead of the crystal. While a window
   menu is down it consumes every press on the panel (pick / switch title / dismiss), so the
   crystal's CLOSED-corner arm is unreachable and cannot mint a second dropdown.
2. `winmenu::press_at`'s own CLOSED arm declines every point while `crystal::is_open()`, so the
   press falls through to the crystal's dismiss-outside arm.

So at most one of the two can answer `Some`, and the four sites that asked
`crystal::open_rect` by name now ask ONE accessor, `menubar::open_dropdown_rect`
= `crystal::open_rect(..).or_else(|| winmenu::open_rect(..))`:

| site | file:line (post-patch) | what it is |
|---|---|---|
| desktop present | `video/screen.rs:1603` | MENUFIT — the shell's `clear_screen` must not flush the menu |
| sprite overlay arm | `video/wm.rs:5120` | the save-under must not be taken from the layer under an open menu |
| window blit clip | `video/wm.rs:16871` | MENU-OCC — a crossing blit withholds the menu's columns |
| erase clip | `video/wm.rs:17220` | CRYSTAL-DISMISS — the deferred `DESKTOP_BG` fill must not publish over it |

**If a future arc ever wants both open at once, `MENU_OCC_MAX` must go to 2 first.** That is the
one thing a reviewer should not let slide.

### 2.5 Escape — a shared KEY seam

`strip::key_escape(ev)` = `crystal::key_escape(ev) || winmenu::key_escape(ev)`, the twin of
`strip::press_route` and extracted for its reason: both arch routers asked `crystal::key_escape` by
name, and a second name at each of two call sites in two files edited by two lanes is exactly the
drift `press_route`'s own header was written about. Both routers now ask one thing.

* `arch/x86_64/syscall.rs:6739` (`wc_route_event`) — **line-neutral** (four comment lines in, four
  out; x86-only, so `kernel8.img`'s panic-`Location` proof is untouched either way, but the idiom is
  the tree's).
* `arch/aarch64/syscall.rs:13211` — **line-neutral**, edited on the existing folded line per
  PARITY.md §5.3.

`pulsewin::key_escape` — the function with no caller, A10's real cause — is deleted.

### 2.6 `pulsewin` becomes the first publisher

* On `open()`, after `WIN` is stored (the registry is keyed by window id and a tree published
  against `WIN_NONE` is refused) and before the `[pulsewin] open` witness:
  `winmenu::publish(id, tree_for(view()), on_menu_pick)`. A refused publish is **not fatal** — a
  pulse window without menus is still a pulse window, `open`'s own decline rule one level down.
* On `close()`, `winmenu::clear(id)` runs **BEFORE** `wm::close(id)`: `clear` dismisses this
  window's dropdown if it is down and drives the composite that erases it, and that erase must
  happen while the window is still a legible member of the table.
* `on_menu_pick(id)` sets `VIEW`, counts `SWITCHES`, and re-publishes the other `const` tree so the
  mark follows the live view.
* **The in-window strip is DELETED**: `draw_menu`, `title_box`, `menu_box`, `item_h`,
  `max_option_glyphs`, `OPTIONS`, `MENU_OPEN`, `Hit`, `hit_at`, `key_escape`, the five `MENU_*`
  colours, and the two menu arms of `press_route`. (The never-trash rule is about hardware paths;
  this is a UI defect Peter ruled on.)
* **The first content row goes back to the instrument.** `View::Lamps` now gets
  `draw_panel_at(&mut p, Some((0, 0, cw, ch)))` — it was `(0, menu_h, cw, ch - menu_h)`.
* **The surface is the same size.** `content_extent` still budgets `menu_h(&m)`, so the window's
  outer box and its `surf=`/`box=` witness are byte-for-byte what they were and the operator's
  window does not change size because a menu moved. `menu_h`'s doc comment now names that row as
  known slack; reclaiming the pixels is a geometry change for whoever next moves this box.
* `press_route` now claims **exactly one** region, the close disc, and a content press is
  unconditionally not consumed (before, that was true only while the in-window menu was closed —
  one fewer state an operator has to be in to raise their own window).
* The `opens=` counter is **removed** from `[pulsewin] close` and `[pulsewin] rollup` rather than
  left reading zero: the menu is no longer this window's, so "how many times was it opened" is a
  question `[winmenu]`'s ledger answers and this one cannot. A field that could only ever print `0`
  is an instrument asserting that a working menu was never opened. `switches=` stays; `menu=bar`
  replaces `menu=<bool>` on the rollup.

### 2.7 What the ring-3 bus step adds later, and why it is not here

`menubar.rs`'s design ledger (§THE MENU PROTOCOL) specifies the other half: `BUS_VERB_MENU_PUBLISH`
/ `_CLEAR` / `_GET` over the already-principal-stamped `SYS_MSEND` frame (BANDY-STAMP,
`docs/SECURITY.md:325`), `INPUT_EV_MENU_PICK`, the fixed-width wire item, and — the design's
sharpest edge — picks addressed to the **TREE'S OWNER** rather than to whoever holds focus.

None of it is built here, and the module is shaped so that arc is **additive**: an app's tree
arrives as bytes, is decoded into the same caps, and lands in the same registry keyed by
`owner_asid` instead of by window id; the `on_pick: fn(u32)` field becomes an input-ring enqueue.
The RENDERER — `winmenu.rs` — does not change. Building the bus half now would have meant landing
a wire format, three verb tags, an event type and a cross-principal delivery rule inside a UI-defect
fix, and the protocol's own falsification law says the first protocol arc must land with **no
renderer at all**. Doing the renderer first and the protocol second keeps both halves provable on
their own terms.

---

## §3 — Every file the patch touches

Base `191823c2`. `git diff --numstat`:

| file | + | − | what |
|---|---|---|---|
| `unaos/crates/kernel/src/video/winmenu.rs` | 1040 | 0 | **NEW.** The whole module. |
| `unaos/crates/kernel/src/video/pulsewin.rs` | 140 | 250 | header rewrite; the two `const` trees + `tree_for`; `on_menu_pick`; publish/clear; strip DELETED |
| `unaos/crates/kernel/src/video/menubar.rs` | 111 | 3 | `MENUS_X0`; `Model.menu_owner`/`.menus`; the second reduction; signature fold; `set_bar_owner`+snapshot in `compose`; `menus_x0`/`menus_right_limit`/`open_dropdown_rect`; row overlay |
| `unaos/crates/kernel/src/video/crystal.rs` | 39 | 0 | seven `DROP_*` metric re-exports + `is_open()` |
| `unaos/crates/kernel/src/video/strip.rs` | 33 | 2 | `compose_all` `| winmenu::compose()`; `press_route` winmenu FIRST; NEW `key_escape` seam |
| `unaos/crates/kernel/src/video/mod.rs` | 30 | 1 | `pub mod winmenu;` on the furniture gate + the FC-2 note — **at the file's TAIL**, plus a 1↔1 pointer folded onto the `crystal` comment. See the byte-identity block below: this is the one hunk whose PLACEMENT is load-bearing |
| `unaos/crates/kernel/src/video/wm.rs` | 3 | 3 | three `open_rect` sites → `open_dropdown_rect` |
| `unaos/crates/kernel/src/video/screen.rs` | 1 | 1 | one `open_rect` site → `open_dropdown_rect` |
| `unaos/crates/kernel/src/arch/x86_64/syscall.rs` | 5 | 5 | Esc seam → `strip::key_escape` (line-neutral) |
| `unaos/crates/kernel/src/arch/aarch64/syscall.rs` | 1 | 1 | Esc seam → `strip::key_escape` (line-neutral) |
| **total** | **1403** | **266** | |

Hunk anchors (post-patch line numbers), for the reviewer:

* `video/mod.rs:126` (the 1↔1 pointer fold onto the `crystal` comment) and `:875-903` (the
  declaration and the FC-2 paragraph, at the file's tail).
* `video/menubar.rs:203-213` (`MENUS_X0`), `:652-667` (`Model` fields), `:699-712` (the second
  reduction), `:748-753` (signature fold), `:846-855` (`set_bar_owner` + snapshot),
  `:951-986` (`menus_x0` / `menus_right_limit` / `open_dropdown_rect`), `:1071-1082` (row overlay).
* `video/crystal.rs:242-280`.
* `video/strip.rs:695-699` (`compose_all`), `:747-776` (`press_route` header + arm, `key_escape`).
* `video/wm.rs:5120`, `:16871`, `:17220`. `video/screen.rs:1603`.
* `arch/x86_64/syscall.rs:6734-6739`. `arch/aarch64/syscall.rs:13211`.

### FC-2 — the new file's gate, stated

`winmenu.rs` is **arch-neutral**: it contains no `target_arch` anywhere (measured: `grep -c
target_arch video/winmenu.rs` → **0**). Its declaration rides `any(all(target_arch = "x86_64",
feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))` — the identical dual
gate `strip`, `dock`, `menubar` and `crystal` ride, with no arch-only arm. It is not written bare
because the module is a CLIENT of `strip`, `menubar` and `crystal`, all three of which ride exactly
that gate; an unconditional declaration would fail to resolve `super::strip` on a build without the
desktop. **The gate is a DEPENDENCY fact, not a policy one**, and it moves the day the furniture
family's does — together, in one place.

**GATE-FAMILY holds (condition F).** One bar, one menu host, no platform sibling: `grep -rn 'pub mod
winmenu'` over `crates/kernel/src` returns exactly **one** declaration, on that one gate. There is no
`winmenu_pi`, no `orin_winmenu`, and no second host.

### ⚠ Byte identity — measured, and the knob-off Pi image did NOT move

R21 is a BEHAVIOUR change and ONE-OS, so **no image that carries the desktop is byte-identical after
this patch**, and no such claim is made for those. The claim that matters is the Pi's knob-off
baseline, and it is now a measurement rather than an argument:

```
sha256(target/pi_baremetal/kernel8.img)  BEFORE (tree == 191823c2)
  d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0
sha256(target/pi_baremetal/kernel8.img)  AFTER  (patch applied)
  d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0
cmp -> IDENTICAL byte-for-byte
```

Both built with a bare `./arroyo kernel8` in this worktree, `K8_EXIT=0` each; the AFTER image was
copied aside and `cmp`'d against the BEFORE build, so the equality is a byte compare and not just a
digest match. `kernel8.img`, not `kernel.elf` — pi 7's rule.

Why it holds, per file. `K8_FEATS` is `baremetal,skip_xhci` on a bare `kernel8`; `desktop_firmware`
is added only under a knob. So `video/winmenu.rs`, `video/menubar.rs`, `video/crystal.rs`,
`video/strip.rs` and `video/pulsewin.rs` — the five files carrying **23 of the patch's 33 hunks, and
every one of its mid-file additions** — are not compiled into that image at all. The four files that
ARE in it were held line-neutral:

| file in `kernel8.img` | hunks | shape |
|---|---|---|
| `arch/aarch64/syscall.rs` | 1 | `-13208,7 +13208,7` — **1↔1 fold**, and the folded statement is itself inside `#[cfg(feature = "desktop_firmware")]`, so knob-off it is not even compiled |
| `video/mod.rs` | 2 | `-126,7 +126,7` (**1↔1** pointer fold) + `-872,3 +872,32` — a **TAIL** addition past the old file's last line (874) |
| `video/screen.rs` | 1 | `-1600,7 +1600,7` — **1↔1** |
| `video/wm.rs` | 3 | `-5117,7`, `-16868,7`, `-17217,7`, all `+N,7` — **1↔1 ×3** |

**`video/mod.rs` is the hunk a reviewer should look at twice, because the first draft of this patch
got it wrong.** The declaration was originally placed beside `crystal`'s at `:131`, which inserted
**16 lines mid-file**. That file is compiled into every image — `panel_owner`,
`publish_panel_owner`, `panel_snapshot`, `panel_info_nonblocking` and `note_panel_write_refused` all
live below the declaration block — and `core::panic::Location` embeds the source LINE, so the Pi's
knob-off baseline would have moved for a module that build does not contain. The declaration was
moved to the file's tail with a 1↔1 pointer folded onto the `crystal` comment above it. The
byte-identical `kernel8.img` above is the proof that the move was both necessary and sufficient;
before it, the same measurement is what would have caught the defect.

* **x86 (`wc`) and the Pi/Orin desktop images DO change**, by design: they gain the module, the
  registry, the fourth furniture compose and the third press arm.

---

### The hunk table, old → new (condition C)

Generated by `hunktable.py` (kept beside the logs at
`~/unaos-bench/scratch/orin15/menubar/hunktable-r2.txt`). "TAIL" means the hunk begins past the old
file's last line; "MID-FILE" means lines were inserted above existing code and everything below
renumbers.

| file | old | new | Δ | shape |
|---|---|---|---|---|
| `arch/aarch64/syscall.rs` | 13208,7 | 13208,7 | 0 | LINE-NEUTRAL |
| `arch/x86_64/syscall.rs` | 6731,12 | 6731,12 | 0 | LINE-NEUTRAL |
| `video/mod.rs` | 126,7 | 126,7 | 0 | LINE-NEUTRAL |
| `video/mod.rs` | 872,3 | 872,32 | +29 | TAIL (old file ends at 874) |
| `video/screen.rs` | 1600,7 | 1600,7 | 0 | LINE-NEUTRAL |
| `video/wm.rs` | 5117,7 | 5117,7 | 0 | LINE-NEUTRAL |
| `video/wm.rs` | 16868,7 | 16868,7 | 0 | LINE-NEUTRAL |
| `video/wm.rs` | 17217,7 | 17217,7 | 0 | LINE-NEUTRAL |
| `video/winmenu.rs` | 0,0 | 1,1040 | +1040 | NEW FILE |
| `video/crystal.rs` | 239,6 | 239,45 | +39 | mid-file — furniture-gated, not in `kernel8.img` |
| `video/menubar.rs` | 200,6 · 638,11 · 663,17 · 701,6 · 788,11 · 888,6 · 972,6 | 200,17 · 649,33 · 696,28 · 745,12 · 838,21 · 948,42 · 1068,18 | +11 +22 +11 +6 +10 +36 +12 | mid-file ×7 — furniture-gated |
| `video/pulsewin.rs` | 43,37 · 82,7 · 100,15 · 153,24 · 266,7 · 277,45 · 474,9 · 503,18 · 529,7 · 558,13 · 613,47 · 766,75 · 843,70 · 914,12 | 43,52 · 97,7 · 115,6 · 159,51 · 299,14 · 317,6 · 475,15 · 510,21 · 539,10 · 571,12 · 625,33 · 764,33 · 799,6 · 806,10 | +15 0 −9 +27 +7 −39 +6 +3 +3 −1 −14 −42 −64 −2 | mid-file ×13 (one 1↔1) — furniture-gated |
| `video/strip.rs` | 692,7 · 740,9 | 692,11 · 744,36 | +4 +27 | mid-file ×2 — furniture-gated |

**33 hunks. 23 are mid-file non-neutral, and every one of the 23 is in one of the four
furniture-gated files** (`crystal`, `menubar`, `pulsewin`, `strip`) that a knob-off `kernel8` does
not compile. Every hunk in a file that IS in `kernel8.img` is either 1↔1 or a tail addition. That is
the shape condition C asks for, and the byte-identical image above is the shape holding.

### P7 — the fold position proof (condition D)

`p7check.py` (beside the logs) walks every `+` line in the patch, finds the first `//` that is not
inside a string or char literal, and asserts the statement closes before it:

```
folded added lines (code before the first //): 3
  arch/aarch64/syscall.rs   first // at col 158   code before = '#[cfg(feature = "desktop_firmware")] if crate::video::strip::key_escap…'
  video/winmenu.rs          first // at col 19    code before = 'break;'
  video/winmenu.rs          first // at col 30    code before = 'let x0 = s.x[k] - bx;'
P7 OK — every folded added line closes its statement before the first //   (exit 0)
```

The `video/mod.rs` fold is comment-only (no statement), so P7 is vacuous there and it is not counted.

### Names (condition E)

No board token appears in any identifier, symbol or witness string the patch adds. `grep -iE
'orin|jetson|rmbp|pi4|tegra'` over the patch's added lines returns **one** hit, and it is prose in a
doc comment — `//! code on x86 under \`wc\`, on the Pi and on the Orin — so nothing in this file is
arch-conditional` — naming the boards to state the ONE-OS claim, not to name a thing. Every symbol is
subsystem-scoped: the module is `winmenu`, the witness family is `[winmenu]`, the bar accessors are
`menubar::menus_x0` / `menus_right_limit` / `open_dropdown_rect`.

---

## §4 — The gate table

Run in this executor worktree, on the base `191823c2`, with the patch applied. Logs in
`~/unaos-bench/scratch/orin15/menubar/` (`*-r2` = this run; the un-suffixed logs in that directory
are the killed executor's, from a different worktree at an unrecorded sha, and are NOT evidence).

| gate | command | exit | result |
|---|---|---|---|
| type-check, both arches + every cfg leg | `./arroyo check` | **0** | `x86_64 OK`, `aarch64 OK`, `bootloader OK`; **48 kernel cfg legs** green — `x86-all`, `arm-pi`, `arm-tegra`, `arm-tegra-render`, `arm-tegra-desk`, `arm-tegra-furn`, `arm-desk-noel0`, `x86-mix-0…8`, … ; knob→leg coverage OK; knob→builder wiring OK; userspace x86_64 OK (4 crates) + aarch64 OK (5 crates); `midden_core` tests OK; **GATE-KNOB OK** (157 declared / 156 named / 0 phantom / 0 dead / 0 trailing-comment cfg); **GATE-LEDGER OK** (80 rows) |
| x86 compositor QEMU | `UNAOS_WC=1 ./arroyo test 150` | **0** | banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` — carries **`wc`**. H counters below; reachability below (it is a NEGATIVE) |
| aarch64 QEMU | `./arroyo test-arm 60` | **0** | complete, no new forbidden hits |
| Pi bare-metal, BENCH GEOMETRY | `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | **0** | **MBENCH PASS — 119/119 required witnesses, 0 forbidden hits, 32294 lines scanned.** Denominator 119, as stated for hw-jetson |
| knob-off Pi image | `./arroyo kernel8` ×2 (base, then patched) | **0**, **0** | `kernel8.img` **byte-identical**, `d73a8981…69fb0` (see the byte-identity block in §3) |
| armed jetson image, render6's knob line | `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 ./arroyo esp-jetson` | **0** | effective features `witness,ehcihid,holocron,tegra,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,deskcascade` — render6's line, unchanged. Token counts below |

### Witness tokens in the BUILT artifacts (condition B)

`grep -a -c -F` — the `-F` is required: a bare `[token]` is a regex character class under `ugrep`,
which is how a real token silently scores zero. Every run carries **known-absent controls**; a
control that scored would indict the method, not the build.

| token | armed jetson `kernel.elf` | x86 `wc` `kernel.elf` |
|---|---|---|
| `[winmenu] publish` | **7** | 0 |
| `[winmenu] open title=` | **1** | 0 |
| `[winmenu] pick owner=` | **1** | 0 |
| `[winmenu] dismiss reason=` | **1** | **1** |
| `[winmenu] clear owner=` | **1** | 0 |
| `[winmenu] REFUSE` | **1** | **1** |
| `[pulsewin] open win=` | **1** | 0 |
| _control_ `[winmenu] unpublish owner=` | 0 | 0 |
| _control_ `[winmenu] teleport` | 0 | 0 |
| _control_ `[pulsewin] menu open` | 0 | 0 |
| _control_ `[pulsewin] menu dismiss` | 0 | 0 |
| _control_ `PULSEWIN-MENU: title_press` | 0 | 0 |

The last three controls are the tokens this patch DELETES, and they score 0 in both artifacts —
which is the deletion proved in the binary rather than in the diff.

### ⚠ x86 REACHABILITY — stated separately, and it is a negative

**The banner proves the knob; it does not prove the path, and on x86 the path is not reached.** The
x86 artifact carries **27 `winmenu` strings and symbols** — `compose`, `bar_boxes`, `open_rect`,
`draw_bar_row`, `compose_row`, `BarSnapshot::signature`, the `OWNERS`/`TREES`/`LIVE`/`BAR_OWNER`/
`PUBLISHES`/`PICKS`/`DISMISSES` statics, and the `strip::paint::<winmenu::compose::{closure}>`
instantiation — so the module is compiled AND linked into the compositor's compose path. But the
publish/open/pick/clear/rollup half was **eliminated as unreachable**: its format-string literals are
absent from the binary. Only `[winmenu] REFUSE site=` and `[winmenu] dismiss reason=` survive, which
are the two paths the x86 router does ask — `strip::key_escape` and `strip::press_route`.

The reason is that **nothing publishes a tree on x86**: `pulsewin` is armed only from
`desktop_firmware::activate`, which is aarch64's. The same build proves the point about itself —
`[pulsewin] open win=` and every `[menubar]` and `[crystal] …` string except one is likewise absent
from this x86 kernel, exactly as they already were before this patch. So `winmenu` behaves on x86
precisely like the family it joined.

**What the x86 leg therefore proves, and what it does not.** It proves the code is present, links,
compiles under every cfg leg, and is HARMLESS (§H below: nothing moved). It does **not** exercise the
bar path, and it cannot: on x86 the compositor's ignition is the Kepler takeover, and there is no
publisher behind it. **The behavioural proof of R21 is aarch64's, on the armed jetson image and on
the Pi desktop.** Any reviewer who reads "x86 green" as "the menu bar was exercised" is reading more
than the artifact says.

### H — tearing / decline / amplification counters, before and after (condition H)

**The expected direction was written down before the runs** (recorded in
`~/unaos-bench/scratch/orin15/menubar/PROGRESS.md` at resume time): since nothing publishes on x86,
`LIVE == 0`, the compose fast path is two relaxed atomics, and **all three families should not
move**.

Same leg each time — `UNAOS_WC=1 ./arroyo test 150`, exit 0 — in this worktree:

| counter | BASE run 1 | BASE run 2 | PATCHED |
|---|---|---|---|
| `[wc-h]` lines | 57 | 55 | 55 |
| `[wc-h]` `torn=` non-zero | **0** | **0** | **0** |
| `[wc-h]` verdicts | all `TEAR-FREE` | all `TEAR-FREE` | all `TEAR-FREE` |
| `[wc-h]` `banded=1` rollups | 1 (`win=1 window-band`) | 1 | 1 (`win=1 window-band`) |
| `[wcser] declined_pct` | **0** (`entered=3 declined=0`, SOLO) | **0** (SOLO) | **0** (`entered=3 declined=0`, SOLO) |
| `[wc-w]` final `amp` | 3.41x | 3.27x | **3.24x** |
| `[wc-w]` `presents` | 558 | 569 | 566 |
| `[wc-w]` `requested_px` | 1519292 | 1250548 | 1263420 |
| `[wc-w]` `full_presents` / `bracketq_met` | 4 / 1 | 3 / 0 | 3 / 0 |

**Tearing and serialisation: the prediction held exactly.** `torn` is 0 on every one of the 55–57
`[wc-h]` lines in all three captures, every verdict is `TEAR-FREE`, the banded rollup is the same
single `win=1 window-band` in each, and `declined_pct` is 0 with `entered=3 declined=0 -> SOLO` in
each.

**Amplification: the prediction was WRONG AS STATED, and the second baseline run is why it is
reported this way rather than explained away.** `amp` moved 3.41x → 3.24x. But a second run of the
*unpatched* base moved it 3.41x → 3.27x — a 0.14x baseline-to-baseline spread against a 0.03x
patched-to-baseline-2 delta. **`amp` is not a stable predicate on this leg**: it is a ratio over a
wall-clock-bounded capture on a host that had other executors' builds running, and `full_presents`
and `bracketq_met` are single-event counters that moved between the two baselines too. The patched
run sits closer to baseline 2 than the two baselines sit to each other. The correct statement is
*within the leg's own run-to-run spread*, and the mechanism agrees: with no publisher on x86 the only
live winmenu code in that capture is `compose()`'s two-atomic decline. **If rmbp wants `amp` to be a
gate, it needs a repeat count and a spread first** — one run against one run cannot carry it, and this
table is the evidence for that claim as much as for the patch.

### Specs

No spec counts a token this patch removes. Checked: `jetson-sync1.spec`, `pi4-regression.spec`,
`pi4-barename.spec`, `x86-wc.spec`, `x86-witness.spec`, `x86-fat.spec`, `rmbp-boot.spec`,
`round6-rmbp.spec`, `jetson-jd5.spec`.

* The only `pulsewin` mention anywhere in `unaos/scripts/specs/` is a **comment** at
  `jetson-sync1.spec:654` quoting a `[pulsewin] open …` line; `win=`, `panel=`, `surf=`, `box=`,
  `at (…)` and `view=` are all unchanged. Only the trailing parenthetical changed, from
  `(menu: click \`View\` for the two faces …)` to `(menu: \`View\` is in the MENU BAR …)` — the old
  text would now be a lie.
* `x86-witness.spec:1371` REQUIREs `:: MENUBAR: … press=crystal …` — the `menubar::selftest` line
  and the `[menubar]` ledger tail are **untouched**; `winmenu` carries its own `[winmenu]` ledger
  line rather than adding a field to the bar's.
* `pi4-regression.spec:2248` FORBIDs `[menubar] … press=inert` — still not emitted.
* No spec references `PULSEWIN-MENU`, `title_press`, or `[pulsewin] menu dismiss`.

### ⚠ One SCORER does misread after this patch — the render6 menu scorer

Not a spec, but it is what scores the Orin flights, so it is named here rather than discovered on the
bench. `~/unaos-bench/scratch/orin15/scorers-render6.sh` line 45 counts menu OPENS with

```
/SHARD-MENU.*open|pulsewin.*menu.*open|\[pulsewin\] menu/{o++; ol=$0}
```

and dismissals with `/dismiss reason=esc|…/` and `/dismiss reason=/`. After R21 the dismiss terms
still fire — from `[winmenu]` — but **every open term is dead**, so the scorer would report
dismissals against zero opens and read the menu as never opened. The open alternation needs
`\[winmenu\] open title=` added. The scorer was left unedited (it is shared scratch tooling, and
silently changing a scorer between flights is how a score stops meaning what the last one meant); the
change is one alternation and belongs to whoever runs the next render flight.

The `[pulsewin] open win=` term the PASS/FAIL verdict keys on (lines 6 and 9 of the same scorer) is
**unaffected**: the line's fields through `view={}` are byte-for-byte what they were, and only the
trailing parenthetical changed — `(menu: click \`View\` for the two faces …)` →
`(menu: \`View\` is in the MENU BAR …)`, the old text now being false.

---

## §5 — The wire, boot by boot

### Orin (render7 — render6's knob line, unchanged)

On the cascaded desktop, once `pulsewin::service`'s open arm fires:

```
[winmenu] publish owner=2 titles=1 items=2 slot=0 replaced=false
[pulsewin] open win=2 panel=1920x1200 surf=… box=1290x212 at (10,914) view=Pi LED lamps (menu: `View` is in the MENU BAR — first option is the Pi's)
```

The bar draws `View` right of the caption slot. A press on it:

```
[winmenu] open title=View items=2 at (227,34) owner=2
```

A press on `x86 segments`:

```
[winmenu] dismiss reason=pick owner=2
[winmenu] pick owner=2 id=1 label=x86 segments
[winmenu] publish owner=2 titles=1 items=2 slot=0 replaced=true
:: PULSEWIN-MENU: pick id=1 view=x86 segments was=Pi LED lamps switched=true ::
```

and the window's face changes on `service`'s next paced pass (≤ 250 ms). A press outside, and
`<Esc>` — **A10's test**:

```
[winmenu] dismiss reason=outside owner=2
[winmenu] dismiss reason=esc owner=2
```

Closing the window:

```
[winmenu] clear owner=2
[pulsewin] close win=2 switches=1 (surface freed; menu cleared from the bar; the desktop LED band is untouched)
```

Ledger line, per composite while open:

```
[winmenu] … state=open owner=2 publishes=2 clears=0 opens=1 dismisses=1 picks=1 refusals=0
```

**A boot that never opens a window menu prints `[winmenu] publish` and nothing else** — the ledger
line is emitted from the open path only, and the compose fast path is two relaxed atomics.

**⚠ `winmenu::rollup` has NO CALLER, and neither do the other three.** Measured, not assumed:
`grep -rn 'winmenu::rollup'` over `crates/kernel/src` returns nothing, and `[winmenu] rollup` scores
**0** in the armed jetson artifact — the function is eliminated. This is not a hazard this patch
introduces; it is the furniture family's existing shape. `crystal::rollup`, `dock::rollup` and
`menubar::rollup` are all `pub fn rollup(scope: &str)` with **zero callers** in this tree, and
`[crystal] rollup` / `[menubar] …` are likewise absent from the built artifacts. `winmenu::rollup` was
written to match its siblings and it matches them in this too.

It is still worth saying out loud, because it is **the same shape as A10** — the defect this patch
closes. `pulsewin::key_escape` was a function that could not fire; four furniture rollups are
functions that do not fire. One uncalled function is a bug; four in one family, all written to the
same template, is a missing call site in whatever was supposed to drive them. It is filed as **S31**
rather than fixed here: driving them is a decision about the furniture family's ledger cadence, it
belongs to the lane that owns `strip`, and inventing a fifth caller in a patch that is already asking
for a grant would be the wrong place to make it. No line of this patch depends on the rollup firing.

### Pi (`kernel8`, `UNAOS_PIDESK=1` desktop)

Identical lines, same module, same code. The Pi's menu bar is still DEFAULT OFF
(`menubar::ENABLED` starts `false`), so on a boot where nothing enables the bar `bar_boxes` returns
the empty snapshot after `menubar::strip_rect` answers `None`, and `[winmenu] publish` is the only
line. **A publisher with no bar is not an error and does not print one.**

### x86 (`UNAOS_WC=1`)

Same code; the module compiles and links under `wc`, and its compose arm runs on every composite —
`compose`, `bar_boxes`, `open_rect`, `draw_bar_row` and `compose_row` are all in the x86 binary.
Nothing publishes on x86 today (`pulsewin` is armed only by `desktop_firmware::activate`, on
aarch64), so `LIVE == 0`, every pass declines in two relaxed atomics, and the publish/open/pick/clear
half is linked away as unreachable. §4's reachability block has the measurement, including which
tokens survive and which do not. That is the ONE-OS claim in its falsifiable form: **the x86 leg
proves the code is present and harmless — it does NOT prove the bar path was exercised, and on this
kernel it cannot.**

---

## §6 — rmbp 12's grant conditions, each with its measurement

rmbp 12 granted the patch-for-review with binding conditions. They are reproduced here in their own
letters, each followed by what was measured rather than by an assurance that it was considered.

### A — the patch reaches rmbp before any `video/` commit

**Met by construction.** This branch is **DOCS ONLY**: `git diff 191823c2 --stat` on the committed
tree is one file, this document, plus the patch and the two ledger ticks. The `video/` and
`arch/*/syscall.rs` edits lived in the executor worktree for building and gating and were reverted
(`git checkout 191823c2 -- unaos`) before the commit, so the branch carries **no `video/` change** and
merging it cannot land one.

`git apply --check docs/dev/evidence/orin15/MENUBAR-WINMENU.patch` at `191823c2` → **exit 0**
(re-run against the reverted tree, which is the same tree a reviewer will have). If trunk has moved
under `video/menubar.rs` or `video/wm.rs`, re-derive rather than force — the four `open_rect` sites
are the ones most likely to have moved.

### B — x86 proof, with the banner AND the artifact, and reachability stated separately

`UNAOS_WC=1 ./arroyo test 150` → **exit 0**; banner `⚡ kernel features:
witness,ehcihid,kbdwit,sdhcblk,smolnet,wc` carries **`wc`**.

And the condition's own point — the banner only proves it compiled — is honoured with a negative
answer, not a positive one. §4's token table and reachability block are the measurement: the module
is linked into the x86 compose path (27 strings/symbols) but **the bar path is not reached on x86**,
because no publisher exists behind the Kepler ignition; publish/open/pick/clear/rollup literals are
eliminated from the binary, and so are `[pulsewin]`'s and `[menubar]`'s, as they already were.
Known-absent controls score 0 in both artifacts. `grep -a -c` is used with **`-F`** throughout.

### C — byte-identity shape, the hunk table, and the Pi's knob-off baseline

* **Hunk-by-hunk old→new table**: §3, all 33 hunks.
* **Every mid-file hunk in a file compiled into `kernel8.img` is N→N**; the one addition to such a
  file (`video/mod.rs`) is at the **TAIL**. The 23 mid-file non-neutral hunks are confined to the
  four furniture-gated files that image does not compile.
* **`kernel8.img` measured before and after**: `d73a8981…69fb0` both times, `cmp` → identical
  byte-for-byte. The Pi's knob-off baseline **does not move**.
* This required a fix during the gate pass, and the fix is the interesting part of the row: the
  first draft declared `winmenu` beside `crystal` at `video/mod.rs:131`, **+16 lines mid-file**, in a
  file every image compiles and whose `panel_*` functions live below that point. It was moved to the
  tail. Without the measurement the draft would have shipped a moved baseline while claiming an
  unmoved one.
* **pi 7's ack is asked on this table**, per pi 7's rule (`sha256(kernel8.img)`, not `kernel.elf`;
  if it moves, say so). It did not move.

### D — P7, every folded statement precedes the line's first `//`

`p7check.py`, a position proof rather than an eyeball: 3 folded added lines, all closing their
statement before the first non-literal `//`; **exit 0**. Output quoted in §3.

### E — names subsystem-scoped, never a board token

Measured over the patch's added lines: one hit for `orin|jetson|rmbp|pi4|tegra`, and it is prose in a
doc comment stating the ONE-OS claim. Module `winmenu`, family `[winmenu]`, accessors `menubar::…`.
Detail in §3.

### F — GATE-FAMILY stays: one bar, one menu host, no platform sibling

`grep -rn 'pub mod winmenu'` → exactly one declaration, on the shared furniture gate. `grep -c
target_arch video/winmenu.rs` → **0**. No `winmenu_pi`, no `orin_winmenu`, no second host, and the
bar remains `menubar`'s alone.

### G — the DRAIN of the dismiss/pick latch on all three arches

**There is no latch, and that is the answer.** S4's `SHELL_REOPEN` is a flag one path sets and
another path drains, which is why it can be drained on x86 only. `winmenu` has no such flag. Every
state change — open, dismiss, pick, clear — calls `drive()` on **the caller's own thread**:

```rust
fn drive() { wm::composite(); if paint_owed() { wm::composite(); } }
```

That is `crystal`'s MENU-DRIVE rule verbatim: the gesture pays for its own paint, from task context,
with one verified retry for a `strip::paint` declined on a contended scratch. `paint_owed()` has **no
caller outside `winmenu.rs`** (measured), so there is nothing for any arch's pump to drain and
nothing that *could* be drained on one arch only. The S4 shape is structurally absent on all three.

**And what dismisses the menu where nothing pumps the bar?** Where nothing pumps the bar, the menu
cannot be open in the first place: the only opener is a press through `strip::press_route` and the
only key path is `strip::key_escape`, both of them arch-router entries. An arch with no router feed
has `is_open()` permanently false and `LIVE == 0`, so the module declines in two relaxed atomics
forever. On top of that, `pulsewin::close` calls `winmenu::clear(id)` **before** `wm::close(id)`, and
`clear` dismisses unconditionally and drives the erase while the window is still a legible member of
the table — so even a window torn down by something other than a press leaves no dropdown behind.

**⚠ The one asymmetry found while answering this, NAMED not fixed** — see §7.

### H — tearing counters, not pass/fail, with the direction stated first

Full table in §4, and it contains a miss reported as a miss. Prediction (written before the runs):
no movement in any of the three families. `torn` (0 everywhere, all `TEAR-FREE`) and
`[wcser] declined_pct` (0, `SOLO`) held exactly. **`[wc-w] amp` moved, 3.41x → 3.24x — and a second
run of the unpatched base moved it to 3.27x**, a baseline-to-baseline spread five times the
patched-to-baseline delta. `amp` is not a stable predicate on a wall-clock-bounded leg on a loaded
host; the honest statement is *within the leg's own spread*, and the second baseline run is why that
sentence is a measurement rather than an excuse.

### I — the ledger rows

`docs/dev/LEDGER.md` **S31** (the four uncalled furniture rollups) and `docs/dev/OS/orin-ledger.md`
**A25** ticked, in this commit. R21 is already verbatim in `docs/dev/RULINGS.md` at `191823c2` and is
**not** re-added.

### J — `wm.rs` is not granted blind

The patch touches `video/wm.rs` at **3 lines** and `video/screen.rs` at **1**, all four the same
one-line accessor swap `crystal::open_rect(..)` → `menubar::open_dropdown_rect(..)`, each hunk 1↔1
and each listed separately in §3's table. No focus plumbing was needed: the bar's menu owner is
derived inside `menubar::compose` from the `wm::dock_scan` rows it already walks (§2.2), so `wm.rs`
gains no new accessor, no new field and no new call.

The one further `wm.rs` change this analysis *found* was deliberately **not taken** — §7.

If the seat prefers to keep `crystal::open_rect` at those four sites, the alternative is widening
`MENU_OCC_MAX` to 2 and pushing both rects — strictly more code in the hottest loop in the
compositor, which is why it was not chosen.

---

## §7 — Two findings the patch deliberately does NOT fix

Both are outside the grant, both are named with the exact site and the exact change, and both are
one line. Neither blocks the patch.

### 7.1 `wm::menu_paint_owed` knows only the crystal

`video/wm.rs`, `fn menu_paint_owed()` (x86-only, `wc`-gated) answers
`super::crystal::paint_owed()` and nothing else. It is the standing backstop the crystal arc added
for a **masked gate holder**: a `COMP_PENDING` whose only freight is a menu has no other taker on a
quiet desktop, so without the term an open-in-state dropdown can be stranded with nothing committed
to painting it. Its three callers are `composite_once`'s idle path and two sites in the gate holder's
re-run loop.

A window menu is now a second surface with exactly that hazard, and the term does not cover it. The
change is one line — `super::crystal::paint_owed() || super::winmenu::paint_owed()` — and it is
**not in this patch** because `wm.rs` is not granted blind and this is not one of the four swaps the
grant contemplated.

**It is also not reachable today**, which is why deferring it is safe rather than merely cautious:
`menu_paint_owed` is `#[cfg(target_arch = "x86_64")]`, and on x86 nothing publishes, so
`winmenu::is_open()` is permanently false and the added term would be permanently false too. It
closes the hazard for the day an x86 publisher exists. On aarch64 the function does not exist at all,
so the Orin and the Pi rely on `drive()`'s in-line composite exactly as the crystal already does
there — a pre-existing asymmetry this patch neither creates nor widens.

### 7.2 Four furniture rollups with no callers — LEDGER **S31**

`crystal::rollup`, `dock::rollup`, `menubar::rollup` and (as written to match them) `winmenu::rollup`
are all `pub fn rollup(scope: &str)` with **zero call sites** in the tree; their format strings are
eliminated from the built artifacts. Detail and the reasoning for filing rather than fixing are in
§5. Owner: the lane that owns `strip`.
