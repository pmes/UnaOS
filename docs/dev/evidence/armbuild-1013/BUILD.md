# ARMBUILD — the Orin and Pi cards on the bench toolchain (2026-10-13, LOCAL WAVE 1)

Base: `exec-rmbp-local-armbuild` @ `441d7e45d` (hw-rmbp dev tip). Toolchain: `rustc 1.100.0-nightly (6eeff9a52
2026-09-23)`, `cargo 1.100.0-nightly (98a09e7e7 2026-09-21)`. No boot, no QEMU. Full logs (not in the repo):
`~/unaos-bench/scratch/rmbp-0915/local1/armbuild/{orin-1,orin-2,pi-1}.log`. Each line was sourced in its own
shell (`run.sh`: `cd unaos; set -a; . ./scripts/image-line-<line>.env; set +a; ./arroyo <verb>`); the issuing
shell carried no `UNAOS_*` variable (`env | grep -c ^UNAOS_` = 0).

## Verdict: BOTH RED, at the same staging check — VUGK.ELF is 360 bytes over the 16 KiB slot

| # | command | rc | wall | stopped at |
|---|---|---|---|---|
| orin-1 | `CARGO_TARGET_DIR=<scratch> ./arroyo esp-jetson` (image-line-orin.env) | 1 | 65 s | ELFHELLO objcopy (harness, see below) |
| orin-2 | `./arroyo esp-jetson` (image-line-orin.env), arroyo's own target dirs | 1 | 328 s | VUGK.ELF size check |
| pi-1 | `./arroyo kernel8` (image-line-pi.env), arroyo's own target dirs | 1 | 0 s (user crates cached from orin-2) | VUGK.ELF size check |

**No MANIFEST exists for either card.** Both verbs exit inside the EL0 user-program staging, before the kernel
is compiled and before media/MANIFEST is written (`find unaos/target -iname '*MANIFEST*'` = nothing). So
`orin-MANIFEST.txt` / `pi-MANIFEST.txt` are not in this directory; there was nothing to copy.

Artifacts that did build (in `unaos/target/`, both runs): `user_blob.bin` 51 B, `ELFHELLO.ELF` 8560 B,
`VUG.ELF` / `VUGC.ELF` / `VUGX.ELF` 12648 B each, `VUGK.ELF` 16744 B (rejected; sha256 `8e4161b7…bc64142`);
Pi also `owner_blob.bin` 128 B, `imp_blob.bin` 74 B, `midden_blob.bin` 4084 B.

### First error, verbatim (orin-2 and pi-1, identical)

```
⚡ Building EL0 KERNEL-VUG static-ELF program (crates/user-vug [kvug] → target/VUGK.ELF)...
    Finished `release` profile [optimized] target(s) in 44.47s
ERROR: VUGK.ELF 16744 bytes > USER_REGION_SIZE (16384)
```

The check is `unaos/arroyo:11002` (`[ "$ksz" -le 16384 ] || … exit 1`), shared by `esp-jetson` and `kernel8`
(both run the aarch64 user staging). It is a true check, not a staging defect: the kernel's slot window is
`USER_REGION_SIZE = 0x4000` (`crates/kernel/src/arch/aarch64/boot.rs:53`) and a larger image is BadSize at load.

### Why — measured on the rejected image (`readelf -lSW target/VUGK.ELF`)

```
[ 1] .text            PROGBITS 0000000000000000 001000 002011  AXM
[ 2] .note.unaos.app  NOTE     0000000000002014 003014 000018  AR
[ 3] .bss             NOBITS   0000000000003000 004000 0001c0  WA
```

`.text` = 0x2011, **17 bytes past the 0x2000 page cliff** documented in `crates/user-vug/src/main.rs` (the SIZE
note) and `crates/user-vug/Cargo.toml`: `.text`+note spill into a second page, `.bss` moves to 0x3000, the
stripped file jumps a page to 16744. To fit, `.text` must end at or below ~0x1fe8 (the 0x18-byte EXECNAME
note follows it), i.e. the `kvug` build must lose about **44 bytes**. The default image still fits: `VUG.ELF`
`.text` = 0x1fab (+ note) with `.bss` at 0x2000.

Not fixable in the staging: rebuilding `kvug` with `-C codegen-units=1` added to arroyo's RUSTFLAGS measured
`.text` 0x2011, unchanged (fat LTO + `opt-level="z"` already). The bytes have to come out of
`crates/user-vug/src/main.rs` under `#[cfg(feature = "kvug")]` (or out of code shared by every arch image),
which is outside this arc's lane (STAGING only) — **report, not touched.**

Likely cause (from the diff, not bisected by build): `abccbcb4a` WIREDIET M1 added `wirediet_fps_due` plus the
`VUG_SAID` atomic on BOTH arches (only M2's `wirediet_art_due` is `#[cfg(target_arch = "x86_64")]`), growing
every aarch64 vug image; VUGK had the least headroom and went over. `docs/dev/evidence/rmbp-1005/wirediet.md`
is where its size claims would be checked.

## Harness note — `CARGO_TARGET_DIR` breaks arroyo's aarch64 staging (orin-1)

The brief's `export CARGO_TARGET_DIR=…/armbuild/target` made orin-1 fail at the first objcopy:

```
llvm-objcopy: error: '…/unaos/crates/user-elf/target/aarch64-base/release/user-elf': No such file or directory
```

arroyo reads its products from hard-coded in-tree paths (`crates/<crate>/target/aarch64-base/release/…` at
:10956, :11020, :11046, :11073, :11091, :4429, :4476; `target/aarch64-base/release/unaos-kernel` at :12060;
`target/aarch64-unaos/…` at :4615/:4649). Only `user-blob` (:10909) honours `CARGO_TARGET_DIR`. So arroyo
effectively sets its own target dirs, and per the brief ("if it does, leave arroyo's") orin-2 and pi-1 ran with
it unset. No arroyo change made: a bench build never exports it, and making ~20 sites honour it is not
"minimal". (Owed, if wanted: every arroyo objcopy source as `${CARGO_TARGET_DIR:-…}`, or arroyo unsetting it.)

## What turns this green

1. Shave ≥ ~44 bytes from the `kvug` build of `crates/user-vug` (e.g. gate WIREDIET M1 to x86 the way M2 is,
   if its aarch64 value is not wanted — that alone may be enough; or a VUG-PACE-style economy under `kvug`).
2. Re-run both lines; the next red, if any, is past the user staging and was not reachable tonight.
