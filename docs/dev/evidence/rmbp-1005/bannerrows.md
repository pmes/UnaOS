# BANNERROWS (rmbp-ledger B481) — design and measurement

**Finding.** GATEPATH's `banner-cert.sh --registry <the seat's x86 metal line>` names ten features with no row
(prefs_reset installdemo instgui ahciroot kvblank_trace lidsleep videoplayer ahci-write holocron svg): an `esp-x86`
whose banner names any of them stops at NO VERDICT before media exists. And `arroyo check` runs its own copies of
seven of the gates `./arroyo gates` runs (knob, k8reach, verbs, charter, arch, attrkeys, status) and none of the
other four (prefs, appearance, deps, banner).

**Seam.** Host gates only: `unaos/scripts/banner-cert.sh` (the table) and `unaos/arroyo` (`check_both` calls
`gates`). No kernel code, no knob, no new file under the charter scope.

**Milestones.**
- M1 — ten rows, each MEASURED on one x86 kernel ELF built with the seat's metal line verbatim (cargo +nightly
  build --release, `x86_64-unaos.json`, the arroyo `KERNEL_FP_RUSTFLAGS`), each token read with
  `LC_ALL=C grep -a -o -F`, each token found in exactly one place in `crates/` + `libs/` and that place under the
  feature's cfg (or, for svg, in svg_core, which only `pixel_core/svg` links). prefs_reset has NO gated literal
  (`cfg!(feature = "prefs_reset")` at `video/settings.rs::safe_mode_check` only selects the 4-byte `knob` word) and
  takes a NOWITNESS row like `wedge2`, certified from its serial line.
- M2 — `check_both` calls `gates` once in place of its seven copies; its longer legs (ledger-check, fc2, lba32,
  test-roots, spec-roots, fixture-reachable, check-roots, verb-alias closure) stay.
- M3 — GATEFIX added no new script (its fixes live inside verb-roots, charter-check, arch-check, status-check), so
  GATES_SET stays at eleven and `_gates_plant` needs no new case.

**Measured (M1), metal-line ELF, 16,596,688 bytes, built from c191d8d7.**

| feature | token | hits | cond |
|---|---|---|---|
| installdemo | `:: INSTALLVERB: census disk=` | 4 | - |
| instgui | `[wc-x] instgui DECLINE reason=create-failed` | 1 | - |
| ahciroot | `:: AHCIROOT: grant=` | 9 | - |
| kvblank_trace | `kvblank8-trace` | 1 | - |
| lidsleep | `[smc] lid=no-key key=MSLD` | 1 | x86_64 only |
| videoplayer | `:: VIDEOPLAYER: path=` | 1 | wc, x86_64 only |
| ahci-write | `:: AHCI: write path ARMED` | 1 | - |
| holocron | `[hcron] /system/` | 1 | - |
| svg | `more than one root element` | 1 | facet and (wc or desktop_firmware) |
| prefs_reset | none (NOWITNESS) | - | serial `[prefs] display reset=1 reason=knob` |

**Witness (host).** `bash unaos/scripts/banner-cert.sh --registry <metal line>` → `findings=0 -> PASS`, and
`banner-cert.sh <that ELF> <metal line>` exits 0 with every row OK or NOWITNESS. `./arroyo gates` →
`GATES: 11 run 11 green -> PASS` on the seat's tree (this worktree's docs lag the seat's: arch is green with
`--docs /home/user/UnaOS/docs/dev`, status reds on the tip's own STATUS/ledger rows).

**Owed.** The OFF polarity of each token is read from source (cfg), not measured on a knob-off ELF (one build per
cause); lidsleep's is the weakest — its literal is ungated text that dies only when `read_msld` folds to `Unbuilt`.
