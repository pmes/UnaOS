# DIRNS — prep

## The finding

LEDGER SO20: "`SYS_OPEN` has no directory namespace — every EL0 file is pinned to the volume
root." Peter: prep only this round — "$41 of credits left until next Tuesday's refresh... you
will not be able to finish — you are setting things up and doing what you can to prep so we can
hit the ground running."

**CORRECTION the seat needs**: SO20's own row reads "fixed-unflown — DIRNS 2026-09-15 on
`exec-rmbp-dirns` off `5e85a7a9`", and the code backs it up — `el0_walk`/`el0_locate`/`el0_mkdir`/
`el0_rmdir`/`el0_root_leaf` are already in `fs/vfs.rs`, both `sys_open`s call through them, and an
in-tree `dirns_witness` (aarch64) already prints `:: DIRNS: abs=… nested=… escape=… acl=… root=…
-> PASS ::` on every `witness`-gated boot. **M1 is done at HEAD 4c1c1d75.** "Unflown" means: (a) no
spec pins it (`grep -rn DIRNS unaos/scripts/specs/` empty) so a regression fails silently, exactly
the hole `x86-login.spec` closed for LOGIN; (b) the walk is root-anchored only (`el0_walk`'s
`parent: u32 = 0` is fixed) — no relative-to-home resolution, so the brief's M2 is real and open;
(c) `crates/user-stat`, the brief's named M3 fixture host, is BGRUN-2's `stat.elf` window demo with
no file I/O at all.

## Mechanism

- `fs/vfs.rs:2922` `el0_root_leaf` — collapses a 1-component path to its leaf (x86's ACL seam).
- `fs/vfs.rs:2941-2975` `el0_walk` — walks `/`-components from `parent=0` via
  `fs.locate_in_dir(parent, c)`; `..` refused (`Invalid`), never normalised; bare leaf -> `(0, leaf)`.
- `fs/vfs.rs:2993-3033` `el0_locate` — resolve + optional create (`create_in_dir`); `#[cfg(feature =
  "login")]` blocks `fs::users::kernel_owned_leaf`.
- `fs/vfs.rs:3035-3057` `el0_mkdir`/`el0_rmdir` — same walk, idempotent.
- `arch/aarch64/syscall.rs:9027` `sys_open` calls the resolver in-process, namespace lock held
  across it; `:24744` `open_locate` maps errors; `MAX_NAME`=40 (`:8995`, sized for
  `/home/<8.3 user>/<8.3 leaf>`).
- `arch/x86_64/syscall.rs:13331` `sys_open` collapses to bare leaf via `el0_root_leaf` *before* the
  owner-ACL lookup (`:13360`, `OWNED_FILES` is a static name-string table, not a disk slot);
  `:12991` `sys_open_dynamic` walks a real path on the storage SERVICE TASK (x86's IF-masked
  handler can't mount/walk FAT itself).
- Fixture: `arch/aarch64/syscall.rs:24867` `dirns_witness` — 5 legs (abs/nested/escape/acl/root),
  scrubs before/after (`DIRNS_DIR="DIRNSD"`, `DIRNS_SUB="SUB"`), final print `:24951`. Called
  dead-last in the U7 chain (`:16572`, `#[cfg(feature = "witness")]`) — QUEUE.md line 72: that
  placement is MEASURED (earlier placement cost 3 other witnesses across two runs).
- `fs/users.rs:866` `ensure_home` already creates `/home/<user>` (serial `:892`) — but `el0_walk`
  never reads a session's home; every path resolves from the volume root today. No
  `session_home`/cwd role exists in `fs::users` (0 grep hits).
- Ring-0 shell's own `cwd_path()` (`shell.rs:52`) is a shell-internal string-join for exec/ls
  resolution, unrelated to the EL0-syscall resolver — `sys_open` takes no cwd/home parameter on
  either arch.

## Plan

**M1 — spec pins for what's already built (no code change).** Add `REQUIRE`/`FORBID` to
`unaos/scripts/specs/x86-login.spec` (and confirm/create an aarch64-side host — see Open
questions). Witness exists: `:: DIRNS: abs=ok nested=ok escape=refused acl=refused root=ok … ->
PASS ::`. Go-red: pin `el0_walk`'s `parent` to `0` unconditionally — QUEUE.md line 72 records this
exact mutation once passed `nested=ok` falsely until the `root_alias` control leg existed, so the
go-red check must confirm `root_alias=true` flips the verdict, not just that `nested` degrades.

**M2 — relative-path-to-home resolution (net new).** Give `el0_walk` a per-caller base cluster
instead of the fixed `0`. Needs: (a) the calling process's home cluster/path, threaded from
`fs::users`/`ensure_home`'s state, not re-derived per call; (b) `el0_walk_from(base, path)` used
only when `path` doesn't start with `/` (absolute paths keep resolving from root, so every existing
fixture is unaffected by construction). Files: `fs/vfs.rs` (base param), `fs/users.rs` (expose the
per-session home), both `arch/*/syscall.rs::sys_open` (pass it through). Witness: extend
`dirns_witness` with leg 6 `relhome` — open a relative leaf as a non-root user, assert it lands
under `/home/<user>/`. Line becomes `:: DIRNS: abs=… nested=… escape=… acl=… root=… relhome=… ->
PASS ::`. Go-red: hardcode the new branch's base to `0` (silent fallback to root) — `relhome` must
flip to FAIL because the file lands in the wrong directory, not because it errors.

**M3 — a real fixture, not `crates/user-stat`.** That crate (`user-stat/src/main.rs`, 494 lines) is
BGRUN-2's `stat.elf`, a windowed non-exiting demo, zero `SYS_OPEN` calls. Recommend: fold M3 into
M2's `relhome` leg inside `dirns_witness` rather than adding a new EL0 program — it reuses the
already-measured-safe placement (dead-last, `witness`-gated, self-scrubbing) instead of a new
EL0-boot-time open with its own placement cost to re-measure. If an EL0-side proof is specifically
wanted, giving `user-stat` a startup `SYS_OPEN` probe is a separate, later session's work.

## Spec pins

```
REQUIRE :: DIRNS: abs=ok nested=ok escape=refused acl=refused root=ok .* -> PASS ::
FORBID :: DIRNS: .* -> FAIL
```

After M2, add rather than edit (keeps the M1 pin valid the moment M1 alone ships):

```
REQUIRE :: DIRNS: .*relhome=ok.* -> PASS ::
```

Plain `.*` glob matches only, no look-around — matches this repo's mbench engine
(`x86-login.spec`'s LOGIN-RAND/LOGIN-KOWN lines are the precedent).

## Open questions

- Is "the STAT program... opens `/HOME/UNA/X.TXT`" a request for a *new* EL0 program, or a
  misnaming of `crates/user-stat` (unrelated code)? Needs a person's call before M3 is scheduled.
- Does `arm-login.spec` (or any aarch64-run spec) execute a `witness`-gated boot today? If not, M1's
  pin needs a different or new host file — confirm before picking one.
- Is there already a per-process/per-task home-cluster field on the scheduler, or must one be
  added? `fs/users.rs` wasn't read past `ensure_home` this pass (budget) — check before designing
  M2's plumbing; if a field exists, M2 shrinks to "read it."

## Next-session start

1. `grep -n "struct.*Task\|home_cluster\|uid" unaos/crates/kernel/src/arch/aarch64/sched.rs unaos/crates/kernel/src/fs/users.rs | head -40` — settle the M2 field question first.
2. Read `fs/vfs.rs:2941-3057` and `arch/aarch64/syscall.rs:24800-24970` (full `dirns_witness`) to draft the `relhome` leg diff.
3. Add the M1 spec pin to `x86-login.spec`; confirm its gate command carries `UNAOS_WITNESS` (transitively or explicit) — `dirns_witness` is `#[cfg(feature = "witness")]`-gated, and QUEUE.md line 70's DIRNS lesson is exactly a gate that didn't compile the path it named.

## Draft code (unbuilt)

```rust
// fs/vfs.rs, after el0_walk (~line 2975) — SKELETON, unbuilt. Existing callers unaffected: they
// keep calling el0_walk/el0_locate, which become thin wrappers passing base=0.
fn el0_walk_from<'p>(fs: &crate::fs::fat::FatFs, base: u32, path: &'p str)
    -> Result<(u32, &'p str), El0LocateError> {
    let start = if path.starts_with('/') { 0 } else { base };
    todo!("el0_walk's loop verbatim, seeded with `parent: u32 = start;`")
}
```

```rust
// arch/aarch64/syscall.rs, inside dirns_witness, after "leg 5: root" (~line 24946), before
// dirns_scrub(&fs) — SKELETON, unbuilt, depends on M2's home-cluster plumbing existing first.
// let home = fs::users::home_cluster_of(A_OWNER); // needs to exist — see Open questions
// let rel = open_locate_from(&fs, home, "RELHOME.TXT", O_CREAT, &mut created_rel);
// let relhome_ok = matches!(rel, Ok(_)) && created_rel && !fs.locate_in_dir(0, "RELHOME.TXT").is_ok();
```
