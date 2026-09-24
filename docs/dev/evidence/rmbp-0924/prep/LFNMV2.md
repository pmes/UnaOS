# LFNMV2 — prep

## The finding

rmbp-ledger B202 row (reopened): "**LFNMV — A RENAME TO A LONG NAME, MEASURED ON BOTH ARCHES**"
was BUILT and QEMU-green, then re-marked open at the 2026-09-24 fold: "open — RED ON THE METAL
(flight 14, bench read fac35c65): Peter's `mv hello.txt test/<~200 chars>` ran as `Host verb=mv`,
no LFNMV wire line printed, and the bench read that the long-name `mv` mutated the VFS index and
never wrote the card (no SD write after the verb; `mkdir` did write). The QEMU-green fix (fold) did
not carry: the rename's write path on the metal, and why the witness this row promised did not
print, are the next cut."

Peter's words, quoted from the row: the fix is BUILT and PASSES in QEMU but the same verb on the
rmbp's real SD card silently no-ops the disk write while claiming success, and the witness that was
supposed to catch exactly this never fired.

## Mechanism

Two separate questions, both answered by reading, not assumed:

**(1) Why no `:: LFNMV:` wire line for Peter's interactive command — answered, definitively.**
`lfnmv_witness` (`unaos/crates/kernel/src/shell.rs:8533-8600`) is `#[cfg(feature = "witness")]` and
is called from exactly one place per arch:
- `unaos/crates/kernel/src/arch/x86_64/syscall.rs:29433` inside `lfnmv_launcher()` (line
  29422-29434), itself `#[cfg(feature = "witness")]` and invoked only as the tail of
  `stor2_mv_launcher` — a synthetic ring-3 self-test ladder program (`STOR2-MV`), run only during
  the boot self-test sequence on a `witness`-featured build.
- `unaos/crates/kernel/src/arch/aarch64/syscall.rs:25653`, same shape.

`fs_mv` (`shell.rs:995-1078`, the actual interactive `mv` verb dispatched from `"mv" | "move" |
"ren" | "rename"` at `shell.rs:5528`) **never calls `lfnmv_witness`**, directly or indirectly. The
witness is wired exclusively to the boot self-test ladder's own synthetic rename
(`STOR2.BIN` -> `LongNameViaSysRename.txt` via `SYS_RENAME`), not to any operator-typed `mv`. So
"no LFNMV wire line printed" for Peter's command is not a regression to chase — it is architecturally
impossible for that line to print for a real command, on any build, witness feature or not. This is
the row's own promise ("the witness this row promised did not print") being unkeepable as wired.

**(2) The write path a long-name `mv` takes on x86 with the SD card as the store.**
`fs_mv` (`shell.rs:995-1078`) -> `MountTable::rename` (`unaos/crates/kernel/src/fs/vfs.rs:606-`,
same-volume/cross-mount-root resolution) -> `FatBackend::rename` (`vfs.rs:1411-1430`) ->
`FatFs::rename_entry` (`unaos/crates/kernel/src/fs/fat.rs:3724-3731`), which for any name
`format_83` refuses (i.e. every long name) dispatches to `rename_lfn_in_dir`
(`fat.rs:6511-6559`).

`rename_lfn_in_dir` computes a slot run (`free_run_or_grow`, `fat.rs:5712-5734`, which may call
`grow_dir_chain` — a FAT-table write plus a fresh cluster), builds the payload
(`write_lfn_run`, `fat.rs:5765-` onward — component slots then the short entry, in the
crash-safe order documented at `fat.rs:6218-6243`), and retires the old entry
(`lfn_tombstone_slots` in place, or `mark_dir_deleted` when the run moved). Every actual byte
reaches the medium only through `wr_sector`/`wr_sectors` (`fat.rs:2179-2206`, extent-checked
wrappers) -> `write_sector`/`write_sectors` (`fat.rs:819-870, 920-960`). For `BlockSource::Sdhc`
(the internal card, `x86_64` + `sdhcblk`), `write_sector` gates on `permit_sdhc_write` ->
`sdhc4c::permit_write` -> `permit_span` (`unaos/crates/kernel/src/fs/sdhc4c.rs:198-230`): under
R59/B166 (SDHCRW) a `sdw` rw-postured boot admits every span (`sdhcpost_admits`, line 214), so on
a normal (non-`sdw-ro`) boot this gate should not be the refusal — a refusal there also logs a
`:: SDHC4C: permit REFUSED …` serial line (`sdhc4c.rs:227-`) and propagates a `FatError`, which
`fs_mv` would surface as an explicit `mv: … (-E…)` line via `vfs_fail`, not as `moved …`. Bench read
says the verb printed as if it succeeded ("mutated the VFS index") while nothing hit the card —
that is inconsistent with an ordinary propagated write error, and points instead at one of:
  a. a build/feature mismatch — the bench boot may not carry `sdhcblk`/`sdw`, putting the mount on
     `BlockSource::Default` (a different `write_block` path, `drivers::block`, not read this round);
  b. a call in the `rename_lfn_in_dir` chain that treats a swallowed write error as `Ok` (candidates
     needing a read next session: `write_lfn_run`'s per-chunk loop past line 5805, and
     `free_run_or_grow`'s `grow_dir_chain` error handling);
  c. the "in place" branch (`fat.rs:6540-6542`, `keep`/`old_run` arithmetic) taking a path that
     never calls `wr_sector` at all for some slot-count transition, so `locate_in_dir` afterwards
     reads the NEW name back out of an in-memory read of a sector it never wrote — i.e. `rd_sector`
     re-reading a buffer FAT semantics assume was just written, without a device round trip actually
     having occurred. This needs a citation-backed trace next session (see M2), not another guess.

No caching layer exists between `wr_sector` and the device (`fat.rs:2179-2206` calls `read_sector`/
`write_sector` directly, no dirty-buffer/flush indirection) — so "mutated the VFS index" is not a
buffered-write problem in this file; it is either (a) a build running a different `BlockSource`, or
(b)/(c) a control-flow path that returns `Ok` without ever reaching `wr_sector`. Distinguishing them
needs an on-device counter (M2), not more static reading.

## Plan

**M1 — make a witness fire on the REAL operator verb, not only the synthetic boot ladder.**
Files: `unaos/crates/kernel/src/shell.rs`, `fs_mv` (after the `mt.rename(&spath, &dpath, …)` call,
~line 1065-1072). Add a cheap, non-`witness`-gated trace that fires whenever `dpath`'s leaf has no
8.3 form (mirror `format_83(new_leaf).is_none()`), printing what actually happened.
Witness line: `:: LFNMV-OP: dst=<leaf> ok=<bool> wr_sector_calls=<n before>..<n after> -> PASS|FAIL ::`
Go-red mutation: on a card where the M2 counter shows 0 sector writes but `fs_mv` printed
`moved …`, this line must read `wr_sector_calls=N..N` (no delta) with `ok=true` — i.e. today's bug,
made visible instead of silent, is the fixture's red state until the actual mutator is fixed.

**M2 — an atomic sector-write counter, always compiled, read before/after the verb.**
Files: `unaos/crates/kernel/src/fs/fat.rs`, `write_sector` (~819-840) and `write_sectors`
(~920-960). Add `static SECTOR_WRITES: AtomicU64` incremented on every `Ok` return (mirroring the
existing `sdhc4c::PERMITS`/`SECTORS` counters' shape, `sdhc4c.rs:198-203`, but device-agnostic —
not only the `Sdhc` arm). Expose a `pub(crate) fn sector_write_count() -> u64` for M1 and for a
QEMU fixture to read before/after `mv`.
Witness line (folded into M1's, or standalone): `:: FATWR: before=<n> after=<n> delta=<n> -> PASS|FAIL ::`
Go-red mutation: stub `wr_sector` in a test cfg to return `Ok(())` without calling `write_sector`
(simulating hypothesis (b)/(c)) and confirm the counter — unlike `fs_mv`'s "moved" line — reports
`delta=0`, proving the counter catches what the operator-visible success line cannot.

**M3 — QEMU go-red reproduction of the bench symptom.**
Files: a new fixture beside `LFN2` (`fat.rs:6559-` area) or a `selftest.rs` case, staged in `/boot`
or a scratch FAT mount, exercising `fs_mv` with the SAME shape as Peter's command: an 8.3-eligible
source, a ~200-char destination leaf (over `LFN_MAX_SLOTS` boundary — confirm against `MAX_NAME`/
`LFN_MAX_SLOTS`, not read this round; if 200 chars exceeds the cap the correct behaviour is a
refusal, and the bench symptom would then be a DIFFERENT bug: `fs_mv` accepting an over-long name
it should reject before touching the medium — check `lfn_units`'s length gate next session first).
Witness line: `:: LFNMV-GOREAD: leaf_len=<n> slots=<n> cap=<LFN_MAX_SLOTS> wr_delta=<n> readback_after_cold_mount=<ok|stale> -> PASS|FAIL ::`
Go-red mutation: force `permit_sdhc_write` to `Err` (simulate an unarmed reserve, `sdhc4c.rs`
`ST_UNATTEMPTED`/`ST_UNARMED` states) and confirm `fs_mv` now prints an explicit `(-E…)` refusal
line instead of `moved …` — establishing the CORRECT behaviour the bench's silent-success is
violating, as a spec-pinnable contrast.

**M4 — resolve the length question before touching `rename_lfn_in_dir` itself.**
Files: `fat.rs`, `lfn_units`/`lfn_slot_count`/`LFN_MAX_SLOTS` (grep next session, not read this
round — the LFN2 comment at `fat.rs:6217` and `LFNMV_SYS_LONG`'s own comment at `shell.rs:8483-8484`
cite `MAX_NAME` of 40; a ~200-char leaf is 5x that). If `mv`'s destination in Peter's flight 14
command is genuinely ~200 characters, `fs_mv`'s failure mode may be "accepted an over-length name
and did SOMETHING wrong with it" rather than "silently no-op'd a valid long-name rename" — these are
different bugs with different fixes, and M1-M3's counters will disambiguate them the moment they
run, but the length cap itself should be read and cited before writing new code against it.

## Spec pins

Target files: `unaos/scripts/specs/x86-default.spec`, `unaos/scripts/specs/pi4-regression.spec`
(existing LFNMV row's home per the ledger).

```
REQUIRE :: LFNMV-OP: .* ok=true wr_sector_calls=(\d+)\.\.(\2) .*
```
(no look-around available — express "no delta" as the SAME captured number on both sides, not a
regex assertion; `mbench.py`'s matcher is a plain substring/field check per the wire-line
convention here, so pin the LITERAL field spelling `wr_sector_calls=N..N` only when N is equal,
i.e. add the go-red fixture's own equality check in Rust, and let the spec `FORBID` the FAIL
spelling below.)

```
FORBID :: LFNMV-OP: .* -> FAIL ::
FORBID :: FATWR: .* delta=0 .* -> FAIL ::
REQUIRE :: FATWR: before=(\d+) after=(\d+) delta=(\d+) -> PASS ::
REQUIRE :: LFNMV-GOREAD: .* -> PASS ::
```

## Open questions

- What features does the rmbp bench boot actually carry (`sdhcblk`, `sdw`, `sdw-ro`, `witness`)?
  This decides which `BlockSource` arm `mv` used and whether SDHC-4c's reserved-extent veto is even
  in play. Needs the bench's own boot banner / serial log from flight 14, not more static reading.
- Was Peter's destination leaf actually ~200 characters, or is "~200 chars" his description of
  `test/<something>` where `test/` is a real subdirectory and the leaf is shorter? The exact string
  decides whether M4's length-cap question is live.
- Is `bus_mv`'s aarch64 same-slot promise (the LFN2/LFNMV comment block, `fat.rs:6218-`) meant to
  extend to `fs_mv`'s x86 `rename_lfn_in_dir` path, or are they intentionally different contracts
  (shell trusted vs ring-3 job-only)? R48's open question in the ledger row is adjacent but not
  identical to this one — a person should say whether M1-M3 should also re-litigate R48.

## Next-session start

1. `sed -n '1,60p' unaos/scripts/specs/x86-default.spec` and the `pi4-regression.spec` twin — find
   the exact existing LFNMV pin text to edit in place rather than duplicate.
2. `grep -n "LFN_MAX_SLOTS\|fn lfn_units\|MAX_NAME" unaos/crates/kernel/src/fs/fat.rs` — resolve
   M4's length question before writing the M1/M2 counters against the wrong hypothesis.
3. Ask Peter (or read flight 14's own bench serial log, not just the ledger's summary of it) for the
   bench's boot feature flags and the LITERAL `mv` command text, to close the two Open Questions
   above before M1's edit is written for real.

## Draft code (unbuilt)

```rust
// unaos/crates/kernel/src/fs/fat.rs — after write_sector's closing brace (~line 843)
#[cfg(feature = "witness")]
static SECTOR_WRITES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// LFNMV2: total successful `write_sector`/`write_sectors` calls this boot. Read before/after a
/// verb to answer "did anything actually reach the medium" without trusting the verb's own success
/// line — see rmbp-ledger B202's re-open.
#[cfg(feature = "witness")]
pub(crate) fn sector_write_count() -> u64 {
    SECTOR_WRITES.load(core::sync::atomic::Ordering::Relaxed)
}
```

```rust
// unaos/crates/kernel/src/shell.rs — inside fs_mv, replacing the final match block (~line 1065)
    #[cfg(feature = "witness")]
    let before = crate::fs::fat::sector_write_count();
    let long_dst = crate::fs::fat::format_83(vfs_leaf(&dpath)).is_none();
    let r = mt.rename(&spath, &dpath, SHELL_PRINCIPAL);
    #[cfg(feature = "witness")]
    if long_dst {
        let after = crate::fs::fat::sector_write_count();
        serial_println!(
            ":: LFNMV-OP: dst={} ok={} wr_sector_calls={}..{} -> {} ::",
            vfs_leaf(&dpath), r.is_ok(), before, after,
            if r.is_ok() { after } else { before } != before { "PASS" } else { "FAIL" }
        );
    }
    match r {
        Ok(()) => vfs_say(console, &alloc::format!("moved {} -> {}", spath, dpath)),
        Err(VfsError::Backend("exists")) =>
            console.println(&alloc::format!("mv: {}: file exists (-EEXIST)", dpath)),
        Err(e) => vfs_fail(console, "mv", &dpath, e),
    }
```
(unbuilt — the trailing PASS/FAIL boolean expression above needs cleanup before this compiles; left
literal so next session sees the intent, not a hidden fix.)
