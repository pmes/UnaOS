# ARMPIPE — the ring-3 launch, pipe and kill verbs on aarch64 (orin-ledger A108)

Arc base `12393dd6`, branch `exec-orin-armpipe`. Compiled only (R107); no QEMU run, no bench run. Every wire line below
is the shape the code prints; none is quoted from a capture. One file of kernel code changed:
`unaos/crates/kernel/src/arch/aarch64/syscall.rs` (six same-line folds and the replaced tail block). No x86 file, no
una-abi, no user program touched.

## What the aarch64 syscall layer answers now (una-abi numbers)

| nr | name | before (12393dd6) | now |
|---|---|---|---|
| 68 | `SYS_SPAWN_PATH` | `-ENOSYS` (the `_ => -38` default) | served: x86's checks, `bg`'s body, SPAWN_PIPE honoured |
| 70 | `SYS_PIPE_READ` | `-ENOSYS`, `[pipe] read … -> refused=no-launch-pipe` | served over `crate::pipe` (x86's answers) |
| 71 | `SYS_PIPE_WRITE` | `-ENOSYS`, `[pipe] write … -> refused=no-launch-pipe` | served (each byte a KEY_DOWN on the child's ring) |
| 72 | `SYS_KILL` | `-ESRCH`, `[kill] … -> refused=not-my-child` always | served (parenthood-gated, reaped by `bg_kill`) |

The pipe table is the arch-neutral `crates/kernel/src/pipe.rs` unchanged. The key is the ASID (`current_asid()`), the
generation `ASID_GEN` (syscall.rs:10081), bumped first thing in `clear_handle_row` (syscall.rs:4442, line 4448) at the
slot teardown. x86 keys the same table by `(slot, SLOT_GEN)`.

## Wire lines (same text as x86; the `row=` / `by=` value is the ASID on this arch)

- `[spawnpath] path=<p> principal=<who> bytes=<n> note=<flags> pid=<pid> asid=<a> entry=<e> argc=<n> -> started`
  (x86 prints `slot=` where this prints `asid=`)
- `[spawnpath] path=<p> principal=<who> argc=<n> -> refused=<why> errno=<e>` — why: `acl-read`, `open`,
  `is-a-directory`, `empty-file`, `over-image-cap`, `apptrust`, `read`, `short-read`, `not-elf`, `not-elf64`,
  `not-little-endian`, `not-aarch64`, `pipe-table-full`, `loader`
- `[spawnpath] path=<p> loader=<the bg body's refusal, e.g. EL0-EL1CORE's>`
- `[pipe] open pid=<p> reader=<launcher asid> child=<child asid> ring=65536` (pipe.rs)
- `[pipe] close pid=<p> bytes=<b> dropped=<d>` (pipe.rs, at EOF)
- `[pipe] read pid=<p> row=<asid> -> refused=not-the-launcher`
- `[pipe] write pid=<p> row=<asid> -> refused=no-pipe|not-the-launcher|child-exited`
- `[kill] pid=<p> by=<asid> -> refused=not-my-child`
- `[pipe] kill pid=<p> by=<asid> -> ok (killed — row reaped (…))` or `-> refused=exited (…)`
- `[skill] killed pid=<p> asid=<a> confirmed=1 row=reaped (bg)` (bg_kill's own line, before the above)
- `[wc] tab -> app pid=<p> owner=<asid>` — a bare Tab while a live ring-3 program holds the keys
- `[wc] tab -> cycle cur=<c> next=<n> ring=<r>` — the shell (or a focus no running program holds) cycles, printed
  beside the unchanged `[wc-c] focus tab-cycle <c> -> <n> (ring of <r> + shell)`

## Item by item

1. **Spawn-by-path with SPAWN_PIPE.** `sys_spawn_path` (syscall.rs:26473) and `spawn_path_checked` (:26537) are x86's
   (arch/x86_64/syscall.rs:30768, :30827) with three arch terms changed: the image cap is `xwin::IMAGE_CAP`,
   `e_machine` must be 183 (EM_AARCH64), and the spawn is `spawn_user_image_bg_piped` — the `bg` body
   `spawn_user_image_bg_argv` split on its own line (:8313) so a pipe and the ring-3 flag reach the pre-run window.
   `spawn_path_bind` (:26452), folded on the `let asid` line (:8352) after `map_image_into_slot` and before
   `spawn_user_slot`, binds the reserved pipe to the child's `(asid, ASID_GEN)`, so the child's first write is
   captured. The principal: the loader stamps the image principal and, with a session open, the session user
   (`session_restamp`, :24517, the only path that may write PRIN_USER); a ring-3 caller that is NOT the live session
   user gets its child restamped to the child's image principal, so a launch never mints a session user's authority
   (x86's `slot_user_inherit` rule). `sys_write`'s Console loop feeds the pipe per chunk (:7270), the bytes still
   reaching the serial console as before.
2. **Read / write / kill.** `sys_pipe_read` (:26596), `sys_pipe_write` (:26650), `sys_kill` (:26684) are x86's
   bodies over the same table: the zero-length `pipe::read` is the parenthood probe (`pipe_parent_of`, :26625); the
   child's ASID comes from its PRUNNING PROCS row (`pipe_child_asid`, :26635); the write pushes KEY_DOWN events with
   `user_input_push` (:13431) under `PIPEW_LOCK`, -EAGAIN while the child holds the focus; the kill is `bg_kill`
   (:8433). Refusals `not-my-child` and `exited` as on x86.
3. **Bare Tab.** `wc_tab_to_app` (:26721), folded on `wc_focus_key`'s `cur` line (:13356): when the focused ASID is a
   PRUNNING PROCS row with a pid (`tab_live_pid`, :26706 — the twin of x86's `appquit_pid`), the Tab press and
   release return undelivered and `user_input_enqueue` (:13188) queues them on that ring. The shell (`cur == 0`) and
   a focus with no running program keep the cycle. pi4-regression.spec:724's REQUIRE of `[wc-c] focus tab-cycle` is
   met by TABRING (`tabring_selftest`), whose owners are the synthetic ASIDs 7 and 8 with windows but no PROCS row:
   `tab_live_pid` answers `None` for them, so every press still cycles and prints the line.
4. **Key bytes, verified by reading (no change).** `drivers/xhci/mod.rs` is compiled on both arches
   (`drivers/mod.rs:2`, `pub mod xhci;` with no cfg); aarch64 boots it (`arch/aarch64/piusb.rs:1987`, `:2484`
   `xhci::init`; the Tegra XUSB path reads the same driver's counters, `display_tegra.rs:5692`). Its keyboard decoder
   (`xhci/mod.rs:5188`, `:5208`) calls `hid_key_ascii`, whose table rows 0x4B/0x4E (`:128`, `:131`) press
   `KEY_PGUP` 0x8F / `KEY_PGDN` 0x90 and whose Ctrl branch (`:232`) presses `HID_KEY_CTRL_ENTER` = `KEY_CTRL_ENTER`
   0x8E (`:300`). aarch64's `pack_input` (syscall.rs:13164) packs any `Event::Key(b)` byte as `INPUT_EV_KEY_DOWN`
   with no filter, so the three bytes reach a ring-3 program on this arch exactly as on x86.
5. **`tests midden` / `tests una` on aarch64: NOT LIFTED — the cfg is not the blocker.** `lib.rs:339`
   (`#[cfg(all(feature = "midden", target_arch = "x86_64"))] pub mod middenapp;`) and the `unaapp` line beside it
   could be widened, but the module bodies name x86 directly and would not compile, or would compile wrong:
   * `middenapp.rs:56` `use crate::arch::x86_64::syscall::{self as sc, BgPoll};` (and `unaapp.rs:80`'s twin) — the
     x86 module path;
   * `middenapp.rs:176` `crate::arch::x86_64::elf::validate_elf(&img, sc::user_window_size())` — x86's loader
     validator and window size (aarch64: `xwin::place_elf_model` / `validate_elf` are private to its syscall.rs);
   * `sc::user_image_cap()` (`:157`) — aarch64's cap is `xwin::IMAGE_CAP`;
   * the launch handle: x86 returns the 0-based slot and the fixture types into `slot + 1`
     (`type_line`, `user_input_depth(slot + 1)`, `user_input_push_owner(slot + 1, …)`); aarch64 returns the ASID,
     already the owner (`video/wm.rs:782` `owner_of_launch`) — a widened cfg would type every key into the NEXT
     tenant's ring;
   * `user_input_push_owner` is `desktop_firmware`-gated on aarch64 (syscall.rs:25653);
   * the registration `tests.rs:175` is `#[cfg(all(feature = "midden", target_arch = "x86_64"))]` (not in this brief);
   * the programs themselves: `user-midden/src/main.rs:344` and `user-una/src/console.rs:108` build their core facts
     with `exec: cfg!(target_arch = "x86_64")`, so MIDDEN.ELF and UNA.ELF on aarch64 never call `SYS_SPAWN_PATH` —
     `run` and the IDE's Run are refused by the core before the kernel is asked.
   The exact changes owed (files outside this brief): in `user-midden/src/main.rs:344` and
   `user-una/src/console.rs:108`, `exec: cfg!(target_arch = "x86_64")` -> `exec: true` (the kernel now serves the
   verb on both arches); in middenapp.rs/unaapp.rs, route through `crate::arch::syscall` and an arch-neutral
   `owner_of_launch(handle)` for every ring push, and ask the arch for its validator and cap; then widen
   `lib.rs:339`/`:343` and `tests.rs:175` to `feature = "midden"` / `"una"` with `any(target_arch = "x86_64",
   feature = "aarch64_el0")`. Also owed: una-abi's RING3EXEC and RING3PIPE blocks still say "x86 only" /
   "aarch64 … answer -ENOSYS there by name" (`crates/una-abi/src/lib.rs`, the `arches` paragraph of the RING3PIPE
   block and the RING3EXEC header) — doc text only.

## The legs that witness it

- Compile, both polarities this code touches: `arm-virt-el0` (`virt_el0,login` — the principal rule) and
  `arm-virt-sntp6` (`desktop_firmware` — the drag cancel in `wc_tab_to_app`) in `KERNEL_CFG_MATRIX`, plus the Orin
  image line (`leg-tegra-image.txt`). Nothing new is feature-gated, so no new leg line was added.
- Runtime: `./arroyo test-arm` (QEMU virt, serial `target/serial-arm.log`) reaches the TAB path through TABRING:
  `[wc-c] focus tab-cycle … (ring of … + shell)` and `:: TABRING: TAB rotation — witness=0xff … :: PASS ::` must
  still print, each press now also printing `[wc] tab -> cycle cur=… next=… ring=…`; `kernel8-test` scores the same
  pair in pi4-regression.spec:724-725. No fixture on any aarch64 leg calls `SYS_SPAWN_PATH` until the two `exec:`
  lines above change; the spawn/pipe/kill wire lines are first seen on the Orin or the Pi when MIDDEN.ELF's `run`
  is typed after that change.
