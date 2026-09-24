# VFSWIT — prep

## The finding

Flight line (QUEUE.md:66, CARDROOT, hw-rmbp 2026-09-17): "every per-mount `[vfs]` witness on x86
has been dead since X86BIND — `bind`'s announce latch is consumed by the FIRST mount table (serial
~line 156, before any disk enumerates), so `[vfs] root mount` / `[vfs] volume mounted` / `[vfs] data
mount` read 0 on boots where all four prefixes bound; the fact rides `:: volid: mount …` inside a
fixture instead."

Peter's words (via the exec brief): the cloud has ~$41 of credits left until Tuesday's refresh;
"you will not be able to finish — you are setting things up and doing what you can to prep so we
can hit the ground running."

**Correction to the brief's premise, found this session**: the fix is not outstanding. QUEUE.md:67
already carries a `✓ FIXED IN TREE VFSWIT` row (exec-rmbp-vfswit; LEDGER SR18), and the code in the
tree at `/home/user/UnaOS` matches it exactly. M1 is done. What is still open is narrower than the
brief describes: spec pins on the two boot specs the brief named, `x86-default.spec` and
`arm-login.spec`, which do not carry the `[vfs]` REQUIRE block that `x86-fat.spec` already has.

## Mechanism

- `unaos/crates/kernel/src/fs/bootdisk.rs:1349` `pub fn bind(mt: &mut MountTable)` — rebuilt PER
  VERB (`bootdisk.rs:1353-1372`: the old unconditional `swap` spent the one-shot latch on the first
  table built, which on x86 has nothing bound yet).
- `bootdisk.rs:1377-1378` — the fix, already live:
  ```rust
  let announce =
      s.root.is_some() && !MOUNTS_ANNOUNCED.swap(true, core::sync::atomic::Ordering::Relaxed);
  ```
  Short-circuit on `s.root.is_some()` means a rootless table never touches, let alone spends, the
  latch (`bootdisk.rs:1376`). `MOUNTS_ANNOUNCED` is the static one-shot flag at `bootdisk.rs:1649`.
- `announce` is threaded through: `bind_root(mt, ..., announce)` at `bootdisk.rs:1433`,
  `bind_data(mt, ..., announce)` at `bootdisk.rs:1445`.
- `bind_root` (`bootdisk.rs:1478-1548`): prints `[vfs] root mount / = fat boot volume source={}
  rw={} ::` (x86 / non-native aarch64, `:1517-1521`) or, on aarch64 with a native volume, either the
  inline arm at `:1500-1512` (no `sdwrite`) or `native_root_mount` (`:2165-2178`, `sdwrite` build)
  printing `[vfs] root mount / = native unafs volume source={} rw={} ::`. Same function also mounts
  and (if `announce`) announces `/boot` (`:1536-1541`) and `/apps` (`:1542-1546`).
- `bind_data` (`bootdisk.rs:1598-1633`) mounts `/volumes/data` only when the boot volume's `DATA/`
  dir exists (`DATA_PRESENT` cache), announcing `[vfs] data mount /volumes/data = fat boot volume
  source={} rooted={} rw={} ::` at `:1629` — legitimately 0 lines on an image with no `DATA/` (e.g.
  the `sf` fixture).
- Callers: `shell.rs:7451` (`bootdisk::bind(&mut mt)`, aarch64 arm of `vfs_mount_table`); x86 reaches
  it via the same function's x86 arm (`bootdisk.rs:2468`, `video/quarry/live.rs:569`).
- Fix already measured on the ledger (QUEUE.md:67): `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_QEMU_FULL=1
  ./arroyo test-fat sf 200` rc=0; root/boot/apps/volume-mounted = 1 each (0 before), data mount = 0
  (expected, no `DATA/`). Go-red recorded: latch back on first table → `MBENCH FAIL — 36/40`, rc=1,
  `FIRST-SHORTFALL x86-fat.spec:339`.
- Spec pin already in tree: `x86-fat.spec:341-345` (header `:323-339`). Confirmed **absent** from
  `x86-default.spec` and `arm-login.spec` (grepped this session — zero hits both). `x86-default.spec:56`
  shows its own boot already binds a root and mounts (`X86BIND: ... mounts=[0-9]+ layout=true ->
  PASS`), so the same four lines are reachable there, just unpinned. `arm-login.spec` boots through
  the native-root arm, so its root line uses `native unafs volume`, not `fat boot volume`.

## Plan

**M1 — latch on first root-bound table.** Already done (`bootdisk.rs:1377-1378`, above). No work.

**M2 — pin `x86-default.spec`.** Add the four-line REQUIRE block (see Spec pins below) near the
existing `X86BIND`/`layout=true` block (line 56-59), mirroring `x86-fat.spec:341-345`. `data mount`
left unpinned, same as `x86-fat.spec` (confirm this verb's image has no `DATA/` before hardening).
Witness: the four lines, one each. Go-red: revert `bootdisk.rs:1378` to the unconditional
`MOUNTS_ANNOUNCED.swap(true, ...)` (drop `s.root.is_some() &&`) — spec then reads 0 on all four.

**M3 — pin `arm-login.spec`.** Add the three-line REQUIRE block (Spec pins below) alongside the
existing `[users]`/`LOGIN` block (bare `\[tag\] ...` convention, lines 41-72, no leading `::`).
Confirm first: whether the Pi/virt image has `DATA/` (add a fourth `data mount` REQUIRE if so), and
whether this leg builds with `feature = "sdwrite"` (picks `bootdisk.rs:1500-1512` vs
`native_root_mount` at `:2165` — both emit identical wire text, but verify against a real capture,
not just this session's source read). Witness: the three (or four) lines. Go-red: same latch
reversion as M2 — note QUEUE.md:67 says the Pi/Orin bind root on table 1 already, so this go-red
needs confirming on that lane's own boot order rather than assumed from the x86 case.

## Spec pins

`unaos/scripts/specs/x86-default.spec` (new block, format matches `x86-fat.spec`):
```
REQUIRE \[vfs\] root mount / = fat boot volume source=[a-z-]+ rw=(yes|no) ::
REQUIRE \[vfs\] boot mount /boot = fat boot volume source=[a-z-]+ rw=(yes|no) ::
REQUIRE \[vfs\] apps mount /apps = fat boot volume source=[a-z-]+ rooted=[A-Z0-9]+ ::
REQUIRE \[vfs\] volume mounted /volumes/.+ source=[a-z-]+ rw=(yes|no) ::
```

`unaos/scripts/specs/arm-login.spec` (new block, format matches this file's bare-tag convention):
```
REQUIRE \[vfs\] root mount / = native unafs volume source=[a-z-]+ rw=(yes|no) ::
REQUIRE \[vfs\] boot mount /boot = fat boot volume source=[a-z-]+ rw=(yes|no) ::
REQUIRE \[vfs\] apps mount /apps = fat boot volume source=[a-z-]+ rooted=[A-Z0-9]+ ::
```

No look-around used in either block; both are plain literal-plus-class regexes, same shape as the
already-landed `x86-fat.spec` block.

## Open questions

1. Does the image booted by `x86-default.spec` / `arm-login.spec` carry a `DATA/` directory?
   Decides whether `[vfs] data mount` belongs in M2/M3 as REQUIRE. Needs a real capture, not source
   reading — this session ran no build per the credit limit.
2. Does the `arm-login.spec` lane build with `feature = "sdwrite"`? Decides which aarch64 root-mount
   code path fires (both print identical wire text per this session's read, but unconfirmed live).
3. Is a per-rebind `[vfs] table n=… roots=…` progression line (the brief's own "M2" idea) still
   wanted on top of the already-landed fix? Nothing in QUEUE.md:67 asks for it; only Peter can say.
4. QUEUE.md:66 also owed an x86 lane driving the STOR-1 witnesses (`S7`/`DIRNS` 0 lines on
   `test-fat`) beside VFSWIT — is that this ARC's scope for Tuesday, or separate? Not touched here.

## Next-session start

1. `grep -n "REQUIRE\|FORBID" unaos/scripts/specs/x86-default.spec unaos/scripts/specs/arm-login.spec`
   to re-locate the right insertion point (near the existing root/mount-related REQUIRE lines) before
   editing either file.
2. Boot a real capture for each lane (`./arroyo test` / `./arroyo test-arm` or `kernel8-test`,
   per whichever verb each spec actually replays) and `awk` for `[vfs] data mount` and the exact
   root-mount wording, to close open questions 1 and 2 before hardening the REQUIRE lines.
3. Land the M2 and M3 REQUIRE blocks above into the two spec files, then run each spec's go-red
   (revert `bootdisk.rs:1378`'s `s.root.is_some() &&` clause) to confirm both specs actually fail
   without the fix, the same way `x86-fat.spec`'s go-red is already recorded on the ledger.

## Draft code (unbuilt)

None — M1 (the only kernel-side change) is already in the tree; this round's remaining work is
spec-file text (M2, M3 above), not code.
