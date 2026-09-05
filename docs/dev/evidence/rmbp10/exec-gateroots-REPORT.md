# GATEROOTS — executor report (rmbp10)

Worktree `/home/pmes/unaos-bench/scratch/rmbp10/exec-gateroots/wt`, branch `exec-rmbp10-gateroots`, base `647f485a`.

## sha

**c85b6af3b7828c71932d075efea6565f036f65ca** — `c85b6af3 arroyo: GATE-ROOTS — a binary no check leg names is never type-checked; now every binary must be a named root`
(unpushed; the seat never pushes — Peter's push of `exec-rmbp10-gateroots` / the track branch it lands on is the one push this arc needs)

## Files (2)

- `unaos/scripts/check-roots.sh` — new, +x. Enumerates binaries by directory (crates/*/ + builder/ with src/main.rs or [[bin]]), walks the functions reachable from `check_both` in arroyo (full-line comments dropped), collects every crate a `cargo check`/`build` runs in (`cd $WORKSPACE_DIR/<crate>` + cargo, subshell form, `-p`, `--manifest-path`, and `crates/$_crate` expanded from USER_CHECK_MATRIX). Control probe: `crates/kernel` must be enumerated AND resolved as a root of check_both, matrix must parse to ≥1 crate — else exit 2, no verdict. Exit 1 names each unnamed binary.
- `unaos/arroyo` — +73 / -0, additions only, all inside `check_both`:
  - hw-jetson **a1cf4900** GATE-BOOTLOADER block adopted VERBATIM (header comment + legs 1-3: default aarch64-unknown-uefi and x86_64-unknown-uefi honouring $BOOTLOADER_FEATURES, aarch64 `bootdiag,jb8lever` cross), right after `✅ aarch64 OK`, before `# GATE-CFG:`. Verified: `diff` of that span against `ORIN-GATE-BOOTLOADER.txt` is empty.
  - **leg 4, new (rmbp)**, directly below their third leg: `x86_64-unknown-uefi --features unaos_ivb`. The crate declares `unaos_ivb` (forwards to boot-info), so the leg WAS added; x86 only, the path that ships it.
  - `# GATE-ROOTS:` block after GATE-KNOB, house idiom: host `cargo +nightly check --release` of `builder` (it was the other unnamed binary; 0.4 s) + `scripts/check-roots.sh`; `local _rrc=0`, ✅ line, verdict `if [ "$_rrc" -ne 0 ]` at the tail.

## Enumeration (9 binaries; that is all of them — kernel, bootloader, builder, six user-* crates)

```
crates/bootloader      check_both
crates/kernel          check_both,check_kernel_cfg
crates/user-blob-x86   check_user_arch
crates/user-blob       build_user_blob,check_user_arch
crates/user-elf        check_user_arch
crates/user-pulse      check_user_arch
crates/user-stat       check_user_arch
crates/user-vug        check_user_arch
builder                check_both
```

Before this commit `crates/bootloader` AND `builder` were named by no leg (two holes, not one).

## Go-red, four states (exit codes)

| state | mutation | instrument | result |
|---|---|---|---|
| (a) clean | — | `scripts/check-roots.sh` | exit **0**, `GATE-ROOTS: OK — 9 binary targets` |
| (a) clean | — | cargo-stubbed copy of arroyo, `check_both` (KNOBLEG idiom) | rc **0**, `✅ check roots` |
| (a) clean | — | REAL `./arroyo check` (cold tree) | **CHECK_EXIT=0**, tail below (`gate-a.log`) |
| (b) probe | `crates/zzz_bin_probe` (src/main.rs) added to workspace members | `scripts/check-roots.sh` | exit **1**, `crates/zzz_bin_probe   NAMED BY NO LEG` / `GATE-ROOTS FAILED — ... crates/zzz_bin_probe` |
| (b) probe | same | cargo-stubbed `check_both` | rc **1**, `❌ check FAILED — a binary target is named by no check leg, or builder did not compile` (`harness-b.log`) |
| (c) removed | probe deleted, Cargo.toml restored | `scripts/check-roots.sh` | exit **0** |
| (d) loader defect | peer's format-arity mutation at the identity-witness site, `main.rs:448` → `"UnaOS UEFI Bootloader Started {}"` | REAL `./arroyo check` | **CHECK_EXIT=101** at the first bootloader leg: `error: 1 positional argument in format string, but no arguments were given --> crates/bootloader/src/main.rs:448:47`, `could not compile bootloader (bin "bootloader")` (`gate-d.log`) |
| (d) reverted | `git checkout -- main.rs` | REAL `./arroyo check` | = the (a) real run above, CHECK_EXIT=0 |

Per the seat's update, (d) is the peer's format-arity mutation rather than commenting the leg out. (The script also treats a commented-out leg as absent by construction — full-line comments are stripped before the walk — but that state was not separately run.)

## Gate tail (REAL `./arroyo check`, clean tree, `gate-a.log`)

```
✅ x86_64 OK
✅ aarch64 OK
✅ bootloader OK
  ... 37 cfg legs, every one ✅ (x86-all, arm-pi, arm-tegra, x86-vsyncpace, 26 arm-tegra-*/arm-desk-noel0, x86-mix-0..8) ...
✅ kernel cfg coverage OK (37 legs)
  ✅ user-blob-x86 (x86_64)
  ✅ user-stat (x86_64)
  ✅ user-vug (x86_64)
  ✅ user-pulse (x86_64)
✅ userspace x86_64 OK (4 crates)
  ✅ user-blob (aarch64)
  ✅ user-elf (aarch64)
  ✅ user-stat (aarch64)
  ✅ user-vug (aarch64)
  ✅ user-pulse (aarch64)
✅ userspace aarch64 OK (5 crates)
✅ midden_core tests OK
GATE-FAMILY: OK — 8 platform-split families, none grown
  ✅ arch families (no per-platform copy added silently)
GATE-KNOB: OK — 152 features declared, 151 named by a cfg, 0 phantom, 0 dead
  ✅ knob hygiene (no phantom cfg, no knob wired to nothing)
  ✅ builder (host)
GATE-ROOTS: binary targets and the check leg(s) naming each —
GATE-ROOTS: OK — 9 binary targets, every one named by a leg of check_both
  ✅ check roots (every binary target is named by a leg of this gate)
CHECK_EXIT=0
```

Full enumeration printed by the gate itself sits in the last 14 lines of `gate-a.log`.

## Unproven / notes

- Foreground rule: the cold worktree (empty `target/`) cannot run 37 kernel legs inside a 600 s window, so both real runs were launched with `nohup` to `gate-*.log`; after the seat's correction the (a) run was waited on in the FOREGROUND until `CHECK_EXIT` and the log read to its end before anything else happened. Nothing is reported unread.
- `shellcheck` is not installed on this host; the script was exercised on all four states and `bash -n` instead.
- Leg timings, cold: bootloader x86 13 s, aarch64 12 s, `unaos_ivb` re-check 0.1 s, builder host 0.4 s. The `bootdiag,jb8lever` cross ran only inside the full runs (green).
- Reconcile expectation vs hw-jetson a1cf4900: legs 1-3 byte-identical incl. comment header → the merge folds clean on that block; leg 4 and the GATE-ROOTS block are additions and stay (leg 4 below their third leg, GATE-ROOTS after GATE-KNOB). Union, no leg dropped.
- Scratch artefacts: `gate-a.log`, `gate-d.log`, `harness-b.log`, `arroyo-stub.sh` (cargo-stubbed copy used for the wiring proof), `bl-*.err`, `builder.err`, `commit-msg.txt`.
