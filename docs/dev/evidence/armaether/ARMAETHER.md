# ARMAETHER — `tests aether` on aarch64 (orin-ledger A110)

Arc base `96bdc4de`, branch `exec-orin-armaether`. Compiled only (R107); no QEMU run, no bench run. Every wire line
below is the shape the code prints, read off the source; none is quoted from a capture.

## 1. aether.rs through the launch facade

The facade is ARMTESTS' (A109, `docs/dev/evidence/armtests/ARMTESTS.md`): `crate::arch::syscall` carries
`launch_plan`, `owner_of_launch`, `launch_push`, `user_image_cap`, `spawn_user_image_bg` and `bg_kill` on both arches.
No facade addition was needed; both syscall.rs files are untouched.

| x86 term in aether.rs (at 96bdc4de) | now |
|---|---|
| `crate::arch::x86_64::elf::validate_elf(&img, user_window_size())` (`check`, :368) | `sc::launch_plan(&img)` |
| `user_input_push_owner(slot + 1, e)` (`push`, :255) | `sc::launch_push(slot, e)` (x86: owner `slot + 1`; aarch64: the ASID) |
| `video::wm::owner_of_launch(t.1)` (:109) | `sc::owner_of_launch(t.1)` (the same function on both arches) |
| `#[cfg(target_arch = "x86_64")]` on `REPORT`, `selftest`, `net_drive`, `Drive`, `step`, `push`, `key`, `abs`, `button`, `ints`, `drive`, `chrome`, `check` | removed; the module itself is cfg'd |
| the aarch64 stub `selftest` (`reason=aarch64-image-owed`, :75-80) | removed: ARMAPPS (SR100) stages AETHER.ELF on aarch64 media |
| `net_drive`'s reason text | per arch: x86 bytes unchanged; aarch64 `(aarch64: the in-kernel HTTP loopback is kernel-side only, no ring-3 loopback)` |

cfgs, each on its own existing line (no line moves): `lib.rs:341` `#[cfg(feature = "aether")]` and the `tests.rs:175`
registration (col 603, before the line's first `//` at col 907) both become
`all(feature = "aether", any(target_arch = "x86_64", feature = "aarch64_el0"))` — middenapp's shape. An aarch64 build
without `aarch64_el0` has no `arch::syscall` to spawn through, so it no longer carries `tests aether` (it carried a
SKIP stub before).

The drive's pointer events are window px on both arches: aarch64's producer hands a windowed program
`INPUT_EV_MOUSE_ABS` in its window px (`ptr_abs_follow` / `ptr_abs_arrival`, aarch64 syscall.rs:26181/:26197), the
contract x86's AETHERPTR (B543) states. The ring-3 console line watch every step waits on is reached on aarch64
(`line_watch_check` on the `pipe_feed_write` line, aarch64 syscall.rs:7270), so the ARMTESTS caveat does not apply.

### What `tests aether` prints on aarch64 (from the code)

A. `./arroyo test-arm` without the app stick (no root on virt, fs/bootdisk.rs:584):
- `:: AETHERAPP: reason=no-program-volume ::` (or `reason=AETHER.ELF-not-on-the-volume` when only `/volumes/<label>` is
  bound), then
- `:: AETHERAPP: image=/apps/AETHER.ELF window=bad painted=0 lines=0 hover=0 nav=0 select=0 tab=0 url=0 back=0 find=0 -> SKIP ::`

B. The Orin, the Pi, or test-arm with the app stick (AETHER.ELF in `/apps`): x86's verdict shape —
- `[aetherapp] bytes=<n> entry=<0x…> segs=<n> stack=<n> note_flags=Some(<f>)`
- `[aetherapp] pid=<pid> slot=<ASID> entry=<0x…> sight_ms=… spawn_ms=… report_ms=… wait_ms=20000 kill=<word> tail=…`
- `[aetherapp] drive focus=… hover=… select=… clip=… nav=…`, `[aetherapp] chrome tab=… url=… find=… back=… zoom=… close=… https=…`
- `:: AETHERNET: drive=SKIP(no-server) reason=no-kernel-served-page (aarch64: the in-kernel HTTP loopback is kernel-side only, no ring-3 loopback) proof=aether-url-on-a-networked-boot ::`
- `:: AETHERNET: omnibox=SKIP(no-server) text=https://example.org routed=<url|other|fail|no-line> proof=aether-omnibox-on-a-networked-boot ::`
- `:: AETHERAPP: image=/apps/AETHER.ELF window=elf painted=<n> lines=<n> hover=1 nav=1 select=<n> tab=1 url=1 back=1 find=<n> -> PASS ::`
  (FAIL with its `reason=` otherwise). `slot=` is the ASID on aarch64, the 0-based slot on x86.

## 2. The compile leg `arm-virt-apps-min`

`unaos/arroyo` KERNEL_CFG_MATRIX, after `arm-virt-el0`:
`"arm-virt-apps-min ../../aarch64-unaos.json virt_el0,login,midden,una,lumen,aether"`.
It is the only aarch64 leg with the four app fixtures and no `desktop_firmware` (so `launch_push`'s knob-free body and
the fixtures' facade calls compile without the desktop), and it is witness-free (the polarity media builds ship).
⚠ `aether = ["selfdiag"]` (kernel Cargo.toml:3788), so this leg carries `selfdiag`: `path_write_as`'s `-ENOSYS` arm
(aarch64 syscall.rs, `#[cfg(not(feature = "selfdiag"))]`) is NOT compiled by it. Covering that arm needs a sibling
row without `aether` (`virt_el0,login,midden,una,lumen`); not added (the brief named one row).
Universe: every term is named by `x86-image` (midden, una, lumen, aether, login) or is arm-only already (`virt_el0`),
so `x86_cfg_universe` is unchanged. Precondition as every virt_el0 leg: `target/user_blob.bin` (`ensure_user_blob`).

## 3. The staging line (the ARMTESTS "Runtime, test-arm" note)

The ARMTESTS line (copy fat-sf.img, add APPS/<ELF>) is NOT sufficient, measured from the code:
- `program_source()` (fs/rootdisk.rs:253) reads `/apps` in `shell::vfs_mount_table()` (shell.rs:7444), which is
  `fs::bootdisk::bind` — `/`, `/boot` and `/apps` are bound only on the disk that carries THIS running kernel, found by
  content (a 4 KiB `.text` window at `_start` plus the build stamp, fs/bootdisk.rs module docs).
- On virt "no block device ever carries this kernel" (fs/bootdisk.rs:584): the ESP is QEMU's `fat:rw:` drive, which the
  aarch64 kernel does not enumerate (no virtio-blk driver in the tree); the stick is the only disk it sees.
- `builder/fat-sf.img` is built from the x86 ESP (`scripts/make-fat-img.sh` `stage_contents` copies
  `target/x86_64_esp/kernel.elf`), so its root `kernel.elf` is the x86 kernel and never matches. With only APPS/ added,
  `/apps` stays unbound and the four fixtures SKIP.

So the line also overwrites the stick's root `kernel.elf` with this build's aarch64 `kernel.elf`
(`$ESP_DIR_ARM/kernel.elf`, the file the loader just loaded). The walk (depth 4, 4096 entries; fat-sf.img holds ~40)
then finds it, binds the root there, and `/apps` is the stick's `APPS/`. Added in `test_aarch64`, after
`prepare_aarch64`, only under `UNAOS_FATIMG=sf` and only when `_feats` names midden, una, lumen or aether:
a COPY at `target/armapps-stick.img` (QEMU writes into its backing file — the main checkout's fat-sf.img carries a
guest-written `HOME/` and `USERS.DAT`), `kernel.elf`, `APPS/{MIDDEN,UNA,LUMEN,AETHER}.ELF` each under its own feature
(`build_user_<app>_aarch64`), `system/doc/*.html` under `aether` (stage_jetson_el0_images' shape), then `mdir` of APPS/.

Consequences to read on the first run (not measured here): on that boot the virt kernel HAS a root for the first time
(FAT, writable as `Usb`), so every fixture that skips on "no root" on virt runs instead; the stick's APPS/ still
carries the x86 STAT/VUG/PULSE images make-fat-img.sh put there. If the stick is published under two block sources,
fs/bootdisk.rs's dedupe-by-device (`admit`) is what keeps it one disk — unverified on a virt capture.

Run line: `UNAOS_FATIMG=sf UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_LUMEN=1 UNAOS_AETHER=1 ./arroyo test-arm <secs>`
(after `./arroyo fat-img`). Whether the virt boot reaches `tests-at-boot` (where `tests aether` runs) is not
established here.

## 4. Compiles (R107), each once, `CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave31/target`

All from `unaos/crates/kernel`, `cargo +nightly check --release -Z build-std=core,compiler_builtins,alloc
-Z build-std-features=compiler-builtins-mem -Z json-target-spec`, with `target/user_blob.bin` built as `build_user_blob`
does (51 B, head `280080d2`):
- `--target ../../aarch64-unaos.json --features "$(cat leg-tegra-image.txt)"` (the Orin image line) rc=0 — at ed70d64f
- `--target ../../x86_64-unaos.json --features "$(cat kfeats24.txt)"` (the x86 image line) rc=0
- `--target ../../aarch64-unaos.json --features "virt_el0,login,midden,una,lumen,aether"` (the new leg) rc=0

No warning names aether.rs, middenapp.rs or unaapp.rs in any of the three. Not run (R107): QEMU, `./arroyo check`,
knoboff, `test-arm` with the app stick.
