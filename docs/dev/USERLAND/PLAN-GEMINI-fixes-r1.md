# PLAN — Gemini fix round 1 (post-review, 2026-07-21)

**For the Gemini session.** Branch: `UnaOS-gemini` (your three branches consolidated +
junk-file cleanup). Commit only here. This plan is the ranked output of the adversarial
review of your work vs `main`. Work the phases **in order** — Phase 1 is needed at the
metal bench ASAP.

**Standing rule (Peter, 2026-07-21):** dependencies go to **latest stable** — your version
bumps were correct and are kept. Never downgrade to a pre-release (see Phase 2, unafs).

---

## Phase 1 — FIRST: debugging tools (needed at the bench now)

1. `tools/validate-manifest.py:87-88` — orphan detection only scans the MANIFEST's own
   directory (`os.listdir` + `isfile`). Recurse subdirectories (`os.walk`) so
   `base_dir/subdir/orphan.bin` is caught.
2. `tools/extract-env-knobs.py:30` — `findall` per line adds duplicate inventory entries
   when a knob appears twice on one line. Dedupe per (knob, file, line).
3. `tools/serial-analyzer.py:33` — witness detection is `'::' in line and 'witness' in
   line` substring matching; false-positives on error traces. Anchor to the actual serial
   witness format (`:: ... ::` framed lines). NOTE: serial logs contain control bytes —
   the analyzer must tolerate/strip them (this repo's law: `awk`/binary-safe, never plain
   `grep` assumptions).
4. Oracle: run each tool against a real log from `unaos/target/serial.log` (generate via
   `./arroyo test`) plus a crafted malformed input; show output in your report.

## Phase 2 — Kernel Kepler driver (two blockers + one revert)

5. **BLOCKER** `unaos/crates/kernel/src/drivers/gpu/kepler.rs:44-50` — BAR0 phys address
   is cast to a pointer and dereferenced with no mapping established ("assume identity
   mapped"). On metal, BAR0 is high MMIO outside the identity map → page fault at probe.
   Map the BAR (UC) through the kernel's paging interface before any `mmio_read`/`write`,
   or verify coverage and fail probe cleanly. No unchecked assumption.
6. **BLOCKER** `unaos/crates/kernel/src/drivers/gpu/mod.rs:5` — `pub mod ivb;` is declared
   under `feature = "intel-ivb"` but `ivb.rs` does not exist; that feature cannot build.
   Either add a minimal `ivb.rs` stub or remove the module decl + `UNAOS_IVB` plumbing
   until it exists.
7. **REVERT** `unaos/libs/fs/unafs/Cargo.toml` — `bincode` was moved *backward* from
   `2.0` to `2.0.0-rc.3` (a pre-release). Restore latest stable. (The `anyhow` bump stays.)
8. Lower priority, note in report if not fixed: BAR sizing without clearing memory-decode
   first (`gpu/detect.rs:82-116`); `VramAllocator` hardcodes 256 MB and trusts
   `vram_base` unchecked (`kepler.rs:181-206`); the 64 KB blind PDISPLAY scan heuristic.
9. Oracle: `./arroyo check` green both arches **with and without** the `nvidia-kepler`
   feature; `intel-ivb` builds or is gone.

## Phase 3 — Aether browser (largest; after 1 and 2)

Ground truth: the crate **does not compile** — your own committed `check2.log` ends with
38 errors. Nothing below counts until `cargo check -p aether` is clean and committed logs
are deleted from the tree.

10. Make it build. Dependency versions must be real, latest-stable crates (verify on
    crates.io — several pinned versions do not exist, e.g. `reqwest = "0.13"`,
    `boa = "0.21"`). Remove dead scaffolding: `check.log`, `check2.log`,
    `src/css_test.rs`, `src/css_test_mod.rs`.
11. **No fabricated success** (plan ground rule): `src/yt/mod.rs` ignores the video id and
    returns hardcoded metadata pointing at a Big Buck Bunny MP4. Delete the fake path.
    Implement the real resolver per PLAN-aether-browser.md M1, or report the blocker
    honestly. The module must be wired into `main.rs` and its tests must call functions
    that exist (`parse_response` is referenced but defined nowhere).
12. Stria delegation: add `SMessage::PlayMedia` to `libs/bandy/src/signals.rs` (flag this
    cross-lane touch in your report) and hand playback off per the plan; Aether does not
    decode media itself.
13. Security fixes, all mandatory:
    - `src/storage/mod.rs` — JS can construct `Storage` with an arbitrary path and
      `save()` writes there: arbitrary file write from page script. Confine storage to a
      fixed per-origin directory; never accept a path from JS.
    - `src/net/mod.rs:fetch_document` — no response size cap; enforce a limit. Gate or
      remove `file://` fetch.
    - `src/forms/mod.rs:submit` — percent-encode names/values (the `url` crate is already
      a dep).
    - `src/render/mod.rs:render_to_image` — guard the `f32 as u32` + add arithmetic
      against negative/overflow layout values.
14. README cites the 2026-07-20 Peter ruling for the JS direction (charter amendment
    itself is owed by the Maestro seat, not you).
15. Oracle: `cargo check` + `cargo test -p aether` green; REPORT.md written per
    PLAN-aether-browser.md's review criteria.

---

**DONE gate per phase:** commit to `UnaOS-gemini` with the oracle output in the message or
REPORT.md. Phase 1 lands on its own commit immediately — do not hold it hostage to
Phases 2–3. Adversarial re-review happens before any merge to `main`.
