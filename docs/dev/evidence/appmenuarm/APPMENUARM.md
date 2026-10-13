# APPMENUARM (orin-ledger A112): the APPMENU proof on aarch64

APPMENU M1–M4 (R73, QUEUE.md §1 "BUILT 2026-09-29") gave every board the menu verbs (`BUS_VERB_MENU_PUBLISH` /
`CLEAR` / `GET` = 7/8/9), the pick event (`INPUT_EV_MENU_PICK` = 8), the 40-byte wire item and the owner-keyed
registry `video/appmenu.rs`. The queue line carried "aarch64 arms owed". This arc measured what aarch64 lacked at
`e57d1b0e` and built that.

CHARTER: Kernel, fulfiller (the kernel fulfils the app's bus verbs; the registry is `video/appmenu.rs`). No new
kernel file.

## 1. The measurement at e57d1b0e

`grep -n -i appmenu unaos/crates/kernel/src/arch/aarch64/syscall.rs` (4 hits, not 2) and the call graph from each:

| piece | aarch64 at e57d1b0e | where |
|---|---|---|
| the three verb arms | present | `sys_msend_for`'s dispatch, `arch/aarch64/syscall.rs:23112` (ARMROUTER): `#[cfg(feature = "desktop_firmware")] una_abi::BUS_VERB_MENU_PUBLISH => crate::video::appmenu::verb_publish(asid as usize, body)`, the same for CLEAR and GET (`verb_get` with `&mut text`). The owner is the ASID (`video/appmenu.rs:66` `owner_of_row`, the aarch64 arm). |
| the pick delivery | present | `user_input_push_owner` (`:25654`, `desktop_firmware`), called by `video/appmenu.rs:193` `pick_slot` under `all(target_arch = "aarch64", any(baremetal, tegra_el0))`; the bar's pick sink `pick_bar` and the fixture's `deliver_pick` both go through it. |
| the reap on close | present | `clear_handle_row` calls `crate::video::wm::close_owner(asid)` (`:4551`); `video/wm.rs:2736` calls `super::appmenu::reap(owner_asid)` under `any(all(x86_64, wc), all(aarch64, desktop_firmware))`. |
| the fixture | ABSENT | x86's `appmenu_fixture` (`arch/x86_64/syscall.rs:29467`) is fired from the head of `busx86_stamp_check` (`:25108`) under `witness` + `wc`, x86-only. `tests appmenu` is APPMENU2's `sysmenu::selftest` on both arches (`tests.rs:191`), not this fixture. |

So the arms, the seam and the reap were ARMROUTER's; the owed piece was the proof that drives them.

## 2. What landed

One file of kernel code, `unaos/crates/kernel/src/arch/aarch64/syscall.rs`:

- **FILE TAIL**: `appmenu_call(asid, agen, verb, corr, body, stamp)` — one bus round trip as a scratch ASID through
  the PRODUCTION `sys_msend_for` body (the `bandy_stamp_check` idiom, `:23182`), reply popped with `bus_mbox_pop`;
  `stamp` nonzero plants a caller principal byte at frame offset 16, which `request_validate` refuses. And
  `arm_appmenu_selftest`, the fixture (§3). Both under `all(witness, desktop_firmware)`.
- **`:15488`** (`wcb_launcher`): `crate::tests::register("appmenubus", arm_appmenu_selftest)` folded onto the
  `winresize` registration line, before the line's first `//` (column 296 against 332, measured with `awk index`).
  `git diff -U0` shows one `-15488 +15488` hunk and one tail append; the file keeps its line count up to the tail.
- Knob-off: every new statement is `#[cfg(all(feature = "witness", feature = "desktop_firmware"))]`-erased.

`video/appmenu.rs`, `tests.rs`, `main.rs`, `display_tegra.rs` and the x86 tree are untouched.

## 3. The fixture: `tests appmenubus`

The legs are x86's, in x86's order, and the four verdict lines are x86's byte for byte, so one spec row scores
both arches:

1. a kernel-minted owner publishes the 3-item tree (`Tools` > `Run`, `Stop`) and reads it back through GET,
   byte-identical;
2. a 65-item tree (`items-cap`, reply status -22) and a caller-stamped principal (`sys_msend_for` -> -EINVAL) are
   refused and GET still returns the published tree;
3. `appmenu::deliver_pick(owner, 10)` lands exactly one `input_ev_pack(INPUT_EV_MENU_PICK, 10)` in the OWNER's
   ring and no other ring's tail moves (`USER_INPUT_TAIL` rows read with `peek`, never allocated by the scan);
4. `appmenu::reap(owner)` (the `close_owner` path's call) empties the registry and GET answers empty.

The scratch owner: on aarch64 the wm owner IS the ASID, so the fixture's bus row and its owner are one number.
It takes ASID 7 (the K-line bus fixtures' scratch row) unless a tenant is stamped there (`SLOT_PPID` not NONE),
then the first unstamped ASID above it; with none free it prints `:: APPMENU: -> SKIP (no unstamped ASID in
7..=127) ::`. x86's fixture uses row 6 / owner 7, so the normal wire reads `owner=7` on both arches.

Without `baremetal` or `tegra_el0`, `video/appmenu.rs:72`'s `owner_of_row` names no owner and every verb would
answer -EPERM, so the fixture prints one line and returns:
`:: APPMENU: -> SKIP (video::appmenu::owner_of_row names no owner without baremetal/tegra_el0) ::`. In practice
`desktop_firmware` on aarch64 already needs one of the two (`unaos/arroyo:1792`), so this arm is a compile-time
guard, not an expected wire.

Expected on QEMU raspi4b, the Orin and the bench Pi (`witness` + `desktop_firmware`; `tests appmenubus`, or the
whole registry under `UNAOS_TESTS_AT_BOOT=1`), ASID 7 unstamped:

```
:: TESTS: run appmenubus ::
:: APPMENU: verb=publish owner=7 items=3 depth=2 bar=0/0 -> PASS ::
:: APPMENU: owner=7 items=3 published=1 -> PASS ::
:: APPMENU: readback owner=7 items=3 match=true -> PASS ::
:: APPMENU: verb=publish owner=7 reason=items-cap -> REFUSED ::
:: APPMENU: refuse items=65 principal=caller registry_unchanged=true -> PASS ::
[menubar] pick owner=7 item=10 delivered=true
:: APPMENU: owner=7 items=3 pick_to=owner others_quiet=true -> PASS ::
:: APPMENU: owner=7 closed reaped=true -> PASS ::
:: APPMENU: owner=7 reaped empty=true -> PASS ::
```

`bar=0/0`: the scratch owner has no windows, so no bar tenant takes the tree (x86's fixture reads the same).
`[menubar] pick …` is a `bootlog_println!` line (`video/appmenu.rs:198`), present when the boot log is on the wire.

## 4. A cfg the arc did not widen (report, not change)

`video/appmenu.rs:66` and `:193` gate the real aarch64 owner model on `any(feature = "baremetal", feature =
"tegra_el0")`, with a comment saying "no syscall module" otherwise. `arch/aarch64/mod.rs:46` gates `pub mod
syscall` on `aarch64_el0`, which `virt_el0` also implies (`Cargo.toml:1197`). A `virt_el0` + `desktop_firmware`
build would therefore compile the verbs and the seam but refuse every owner. The brief allows `appmenu.rs` only
for widening an x86 cfg, so this is reported: the exact change is both predicates to `feature = "aarch64_el0"`
(and `:72`'s to `not(feature = "aarch64_el0")`). Until then the fixture's SKIP arm names it.

## 5. Proof owed to the fold

The only proof is one compile (R107): the Orin line, `cd unaos/crates/kernel && CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave33/target cargo +nightly check -Z
build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target
../../aarch64-base.json --features "$(cat leg-tegra-image.txt)"`, rc=0 (the ledger row quotes it). The
`include_bytes!` input `unaos/target/user_blob.bin` was absent in this worktree; it was copied from a sibling
worktree (`/home/user/exec/armresize`) whose `unaos/crates/user-blob` source is byte-identical to e57d1b0e's
(`git diff --stat d4e06198 e57d1b0e -- unaos/crates/user-blob` empty; the two sibling blobs share sha256
`5545d8b8…`). The seat's fold owes:

- the QEMU raspi4b / virt run of `tests appmenubus` (the wire in §3);
- the Pi `kernel8.img` knob-off byte-identity check;
- the Orin and the bench Pi: an app publishing a menu, a pick landing in its ring, the menu gone on quit.
