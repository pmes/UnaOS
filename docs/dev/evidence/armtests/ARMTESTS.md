# ARMTESTS — `tests midden` / `tests una` / `tests lumen` on aarch64 (orin-ledger A109)

Arc base `4f3a436e`, branch `exec-orin-armtests`. Compiled only (R107); no QEMU run, no bench run. Every wire line
below is the shape the code prints, read off the source; none is quoted from a capture. The spec is
`docs/dev/evidence/armpipe/ARMPIPE.md` item 5.

## The launch facade (`crate::arch::syscall`, both arches, appended at each syscall.rs tail)

| term | x86_64 | aarch64 |
|---|---|---|
| `owner_of_launch(h)` | `video::wm::owner_of_launch` = `h + 1` (h = 0-based slot) | the same fn = `h` (h = ASID) |
| `launch_plan(img)` -> `LaunchPlan {model_elf, entry, nsegs, stack}` | `elf::validate_elf(img, user_window_size())` | `validate_elf` over `xwin::XWIN_BYTES`; ELF model iff lowest PT_LOAD >= `USER_XWIN_VA_ARM`; stack by `xwin::place`'s rule; a fixed-model image re-validated over the slot window |
| `user_window_size()` | existing (syscall.rs:268) | NEW: `uslots::user_region().1` |
| `user_image_cap()` | existing (syscall.rs:29948) | NEW: `xwin::IMAGE_CAP` |
| `launch_push(h, ev)` | `user_input_push_owner(owner_of_launch(h), ev)` | owner range check + `user_input_push` (`user_input_push_owner` is `desktop_firmware`-only there) |
| `launch_depth(h)` | `user_input_depth(owner_of_launch(h))` | `user_input_depth(owner_of_launch(h))` |
| `path_write_as(...)` | existing (syscall.rs:30899) | NEW: `selfdiag::path_fulfil(SYS_PATH_WRITE, …)` as `who`; -ENOSYS without `selfdiag` |

`xwin::place_elf_model` (the aarch64 ELF-model validator) allocates the slot it validates into, so it cannot run
before the spawn; the spawn stays the placing check and its refusal is the fixtures' `reason=spawn-refused (…)`.

## Per fixture: what was x86-only, what is now

| fixture | x86 terms removed | cfg | lane still x86-only |
|---|---|---|---|
| `tests midden` (middenapp.rs) | `arch::x86_64::syscall`, `x86_64::elf::validate_elf`, `slot + 1` (`type_line`) | lib.rs:339, tests.rs:175 widened to `any(target_arch = "x86_64", feature = "aarch64_el0")` | the line watch (below) |
| `tests una` (unaapp.rs) | the same three, plus `path_write_as` (now on both) | lib.rs:343, tests.rs:175 widened the same way | the line watch; SYS_PATH_WRITE (the save legs) is `selfdiag`-only on aarch64 |
| `tests lumen` (lumen.rs) | `x86_64::elf::validate_elf`; the x86-only `selftest`/`check`/`spawn_step`/`spawn_run`/`lumenfast` | module already `feature = "lumen"`; bodies now `any(x86_64, aarch64_el0)`, a `no-el0-loader` SKIP stub otherwise | the line watch; the fault's vector (aarch64 reads `bg_poll`'s `Faulted`) |

## The lane that is still x86-only: the ring-3 console line watch (NOT FIXED — file outside the brief)

Every drive in the three fixtures waits on `serial_line::line_watch_hit()`. The latch is `line_watch_check`
(`serial_line.rs:378`, private), called only from `emit_user` (`serial_line.rs:247`), which only x86's `sys_write`
calls (`arch/x86_64/syscall.rs:9629`). aarch64's `sys_write` Console loop writes to the port and the pipe
(`arch/aarch64/syscall.rs:7270`) and never reaches the watch. The exact owed change:

1. `unaos/crates/kernel/src/serial_line.rs:378`: `fn line_watch_check(text: &str) {` -> `pub fn line_watch_check(text: &str) {`
2. `unaos/crates/kernel/src/arch/aarch64/syscall.rs:7270`, before the line's first `//`, after `off += n;`:
   `if let Ok(t) = core::str::from_utf8(&chunk[..n]) { crate::serial_line::line_watch_check(t); }` (LINE-NEUTRAL fold;
   one Acquire load per write when no watch is armed, as on x86).

## What each fixture prints on aarch64 (from the code)

A. `./arroyo test-arm` today (no ELF staged; the default virt stick `builder/usb.img` is the raw pattern image,
arroyo:3305-3308, so no program volume is expected — unverified on a capture):
- `:: MIDDENAPP: reason=no-program-volume ::` (or `MIDDEN.ELF-not-on-the-volume`), then
  `:: MIDDENAPP: image=/apps/MIDDEN.ELF window=bad lines=0 plan=none run=unrun drives=unrun -> SKIP ::`
- `:: UNAAPP: reason=… ::`, then `:: UNAAPP: image=/apps/UNA.ELF window=bad panes=0 lines=0 saved=skip highlight=0 ide=skip run=skip -> SKIP ::`
- `:: LUMENAPP: reason=… ::`, `:: LUMENAPP: image=/apps/LUMEN.ELF window=bad provider=<p> key=<k> -> SKIP ::`,
  `:: LUMENCRASH: spawned=0 first_line=none -> SKIP reason=not-spawned ::`, then LUMENUX / LUMEN2 as before.

B. The Orin or the Pi (ELFs staged under `midden`/`una`/`lumen`), BEFORE the line-watch change: the validator
takes each image (`window=elf`), the spawn runs, and the first watched line never latches:
- `:: MIDDENAPP: reason=timeout ::`, `:: MIDDENAPP: image=/apps/MIDDEN.ELF window=elf lines=0 plan=none run=unrun drives=unrun -> FAIL ::`
- `:: UNAAPP: reason=timeout ::`, `:: UNAAPP: image=/apps/UNA.ELF window=elf panes=0 lines=0 saved=<0|skip> highlight=0 ide=<unrun|skip> run=<unrun|skip> -> FAIL ::`
- `:: LUMENAPP: … window=elf … -> PASS ::`, `:: LUMENCRASH: spawned=1 first_line=timeout -> FAIL :: waited_ms=… bound_ms=20000`

C. After the line-watch change: the x86 verdict shapes — `:: MIDDENAPP: image=/apps/MIDDEN.ELF window=elf lines=<n>
plan=wc run=ok drives=cd:ok,…,pipe:ok -> PASS ::`, `:: UNAAPP: image=/apps/UNA.ELF window=elf panes=3 … -> PASS ::`
(the save/IDE legs need `selfdiag` on aarch64, which the Orin image line carries), `:: LUMENCRASH: spawned=1
first_line=<ms> -> PASS ::`. The `[middenapp]`/`[unaapp]`/`[lumencrash]` lines print the launch handle under `slot=`,
which on aarch64 is the ASID.

## The leg

- Compile: the Orin image line (`leg-tegra-image.txt`: `tegra_el0`, `desktop_firmware`, `selfdiag`, `midden`, `una`,
  `lumen`) and x86 `kfeats24.txt` — both rc=0 at this arc's tip. Not compiled here: an aarch64 build with
  `midden`/`una`/`lumen` and `virt_el0` WITHOUT `selfdiag`/`desktop_firmware` (the facade's `-ENOSYS` arm and the
  knob-free push are the paths that build alone).
- Runtime: `./arroyo test-arm` has no program volume carrying the ELFs. The arroyo line that would add them (NOT
  ADDED — it needs a FAT stick, and which volume virt roots on is unverified): in `test_aarch64` after
  `prepare_aarch64`, under `UNAOS_FATIMG=sf` (a superfloppy mtools can write at offset 0),
  `case ",${_feats}" in *",midden,"*|*",una,"*|*",lumen,"*) _apps="${WORKSPACE_DIR}/target/armapps-stick.img"; cp "$STICK_IMG" "$_apps"; mmd -i "$_apps" ::/APPS 2>/dev/null; for _p in midden:MIDDEN una:UNA lumen:LUMEN; do case ",${_feats}" in *",${_p%%:*},"*) "build_user_${_p%%:*}_aarch64"; mcopy -o -i "$_apps" "${WORKSPACE_DIR}/target/${_p##*:}.ELF" ::/APPS/ ;; esac; done; STICK_IMG="$_apps" ;; esac`.
  The bench leg that carries the ELFs today is the media verbs: `esp-jetson` (`stage_jetson_el0_images`,
  arroyo:10142-10143, `lumen` at :10134) and `kernel8` (arroyo:12185-12194), then `tests midden` / `tests una` /
  `tests lumen` typed at the shell — expected shape B until the line-watch change lands, then C.
