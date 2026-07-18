# K3-BIT5 — landing report

**Arc:** identify and fix the K3-mount assertion that drops (bit 5) when the full
BANDY/write-side fixture chain runs to completion before K3-mount.
**Branch:** `us-k3bit5` (off `main` 05eafaf). Lane: unafs/K-fixture witness + named docs.
**Date:** 2026-07-18. Model: Claude Opus 4.8.

## Bit-5 identity (M1)

- **Emitter:** `k3_mount_selftest`, `unaos/crates/kernel/src/fs/unafs.rs`.
- **Bit 5 (pre-fix), file:line `unaos/crates/kernel/src/fs/unafs.rs:817-827`:**
  the root-`ls` assertion `entries.len() == 2 && hello.is_some() && pat.is_some()`
  — "`ls /` sees exactly the two staged fixture files (`K3HELLO.TXT`, `K3PAT.BIN`)."
- **Silicon signature `[w=0x1df]`:** exactly bit 5 clear, every other assertion set.
  Because bit 6 (read `K3HELLO.TXT`) and bit 7 (read `K3PAT.BIN`) PASS, both fixtures
  resolve and read byte-exact — so the volume holds a THIRD root entry and only the
  `== 2` count fails. `0x1ff & ~(1<<5) = 0x1df`, arithmetically exact.

### QEMU reachability — metal-only ordering (stated, per brief)

- Standard `UNAOS_WITNESS=1 ./arroyo kernel8-test 35` reports `K3-mount PASS [w=0x1ff]`:
  the QEMU card image is rebuilt fresh each run with an empty `UNAFS.ATR`, so the
  boot-time migration installs zero rows and no `acl-*` file is created.
- A two-boot run on the SAME image (boot 1 completes the full write-side chain; boot 2
  reads the resulting image) also stays `[w=0x1ff]` — proving the completed chain
  self-cleans its OWN scratch fixtures; the drop is NOT a scratch leak.
- **Reproduced deliberately:** injecting a single `acl-40-20` file into the K3 fixture
  image root (host `unafs` CLI) yields `K3-mount FAIL [w=0x1df]` on the unmodified
  kernel — the exact metal signature. This is metal-only ordering: on silicon the
  card carries a real committed owner row whose boot migration creates the `acl-*`
  file; QEMU's fresh empty sidecar never does.

## Adjudication (M2): (b) — assertion over-strict, NOT a fixture leak

The native ACL store (K6, `docs/SECURITY.md`) persists one unafs-root file per owned
FAT file, named `acl-<dir_lba>-<dir_off>` (`acl_file_name`, `fs/unafs.rs:302`). Boot-time
`native_migrate_from_sidecar` runs at the head of `u7_launcher`
(`arch/aarch64/syscall.rs:8474`), BEFORE `k3_mount_selftest` (`:8549`), and materialises
each committed `IMAGE_SHA256` sidecar row into such a file. That file is durable,
by-design security state — cleaning it would destroy real ACL data. So the residue is
LEGITIMATE, and `entries.len() == 2` was over-strict against a volume that carries any
live owner row. Adjudication (b).

Evidence it is (b) and not (a): the completed write-side chain (`k4_write`, `k8a`, `k8b`,
`k8c`, `k6_migrate`) self-cleans in QEMU (two-boot run clean); every scratch fixture
unlinks its own file and force-remounts; the only root resident a real card legitimately
adds is the `acl-*` native ACL file.

## Fix (M2): do-it-right, protection preserved

`fs/unafs.rs` bit 5 now requires: both staged fixtures present as readable FILES **and**
every OTHER root entry is an `acl-`-prefixed native ACL file:

```
hello.is_some() && pat.is_some()
  && entries.iter().all(|e|
       e.name == "K3HELLO.TXT" || e.name == "K3PAT.BIN" || e.name.starts_with("acl-"))
```

This is NOT a mask-widen: a genuine scratch leak (`K4TEST` / `K8CUT` / `K8BSNAP` /
`K8CSNAP` / `K8BCHURN*`, none `acl-`-prefixed) STILL fails bit 5, so the fixture
self-clean discipline the bit protects stays genuinely enforced. Only the native store,
which by design lives in that root, is tolerated. No protection was weakened.

## Gate results (verbatim)

- `./arroyo check` — `✅ aarch64 OK` (x86 + aarch64 both green).
- `UNAOS_WITNESS=1 ./arroyo kernel8-test 35` (clean image) —
  `:: K3-mount: … byte-verified PASS [w=0x1ff] ::`, 0 `FAIL`, 191 serial lines, CAPSTONE 6/6.
- Reproduction / fix validation (QEMU, injected `acl-40-20`):
  - unmodified kernel + acl residue → `K3-mount FAIL [w=0x1df]`
  - fixed kernel + acl residue → `K3-mount PASS [w=0x1ff]`
  - fixed kernel + clean image → `K3-mount PASS [w=0x1ff]`
- `./arroyo test-arm 22` — exit 0, no `:: … FAIL`, no panic (virt path; unregressed).
- `./arroyo test 22` — exit 0, no `:: … FAIL`, tests PASS through SOCK-4 (unregressed).
- `UNAOS_WITNESS=1 ./arroyo test 22` — exit 0, no `:: … FAIL` (unregressed).
- Knob-off `kernel8-test` 0 FAIL: trivially satisfied — the K3-mount change is
  witness-gated and the clean witness battery already showed 0 FAIL.

**MBENCH:** the aggregate `MBENCH n/m` line is metal-only (never emitted in QEMU). The
metal `45/46` (K3-mount bit 5 the single miss) → `46/46` accrues on the next Pi sitting;
QEMU cannot reach the metal ordering, as stated in M1.

## Lane / flags

- Files: `unaos/crates/kernel/src/fs/unafs.rs` (bit-5 predicate + doc comment),
  `docs/SECURITY.md` (K3-BIT5 ledger entry), this report.
- `libs/fs/unafs` untouched; zero x86 caller of `k3_mount_selftest`; no protection weakened.
- No lane-boundary crossings; no STOP tripwires hit.
