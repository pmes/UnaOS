# KEPLERUCODE — the GK107 ctxsw firmware from the bunker (rmbp-ledger B637, R119)

Executor: exec-rmbp-local-keplerucode, cut from 6f13e5e92 (rmbp-0915/local3), 2026-10-13. Bench-only arc. This file
holds the measurement that stopped step 1, what landed, the STOPs, and the citation table.

## 1. The source the brief named does not exist (STOP)

The brief (rmbp-queue LOCAL WAVE 3 item 1; R119) says linux-firmware ships the four files under `nvidia/gk107/gr/`
with a LICENCE. It does not. Measured 2026-10-13T17:47Z against the linux-firmware tree on git.kernel.org
(`pub/scm/linux/kernel/git/firmware/linux-firmware.git`, `main`):

| check | result |
|---|---|
| `tree/nvidia` directory listing | 35 subdirectories: ad102 ga100 ga102 ga103 ga104 ga106 ga107 gb100 gb202 gh100 **gk20a** gm200 gm204 gm206 gm20b gp100 gp102 gp104 gp106 gp107 gp108 gp10b gv100 tegra124 tegra186 tegra194 tegra210 tegra234 tegra264 tu102 tu104 tu106 tu10x tu116 tu117. No `gk107`, no `gk104`, no other desktop Kepler. |
| `tree/nvidia/gk107` | HTTP 404 |
| `WHENCE` (511800 bytes, sha256 `df07980b89c8055ff89dbac952f2a2f9d581925891b1736b17e179e139f104da`) | `gk107`: 0 lines. `fuc409` or `fuc41a`: 0 lines. 327 `File: nvidia/` lines. |
| the one Kepler in it | `nvidia/gk20a/{fecs,gpccs}_{inst,data}.bin` + four `sw_*.bin` (WHENCE:5780-5787), under `Driver: nouveau` (WHENCE:5778), `Licence: Redistributable. See LICENCE.nvidia for details` (WHENCE:6275). GK20A is the Tegra K1's GPU, not the GK107; nouveau loads it through a different path (secure/ext names `fecs_inst` etc.), not the four `fuc*` names. |

**What the four names actually are** (facts, nouveau gr/gf100.c): `gf100_gr_load` (:2596-2611) requests `fuc409c`,
`fuc409d`, `fuc41ac`, `fuc41ad` (:2603-2606) as `nouveau/nv<chipset>_<name>` and then `nouveau/<name>`
(:2578-2582) — for the GK107 (chipset E7) `nouveau/nve7_fuc409c` etc. — and only when the `NvGrUseFW` option is set
(:2600). No linux-firmware file carries those names. They are produced on the user's machine by cutting NVIDIA's
proprietary Linux driver (the nouveau project's firmware-extraction script run against an `NVIDIA-Linux-x86*.run`
package) — *recalled, not verified this session; no such package or script was downloaded*. That is the b43 shape
(b43-fwcutter against Broadcom's wl tarball, bunker PROVENANCE `drivers/broadcom-b43/`), but it is a download of a
proprietary driver and a licence the brief did not name, so it is Peter's (R52: "any download of it are separate
asks"). **No redistribution terms are recorded because no GK107 file exists to carry them.**

Consequence: nothing was downloaded into `~/unaos-bench/scratch/rmbp-0915/local3/keplerucode/dl/` (it stays empty);
the bunker gets no `drivers/nvidia-gk107/` and no commit; `unaos/firmware/kepler.pins` ships with NO ROWS, exactly as
FWPIN's `b43.pins` first shipped (B455).

## 2. What landed (source-independent: it serves any bunker set carrying the four names)

* `unaos/firmware/kepler.pins` — the b43.pins grammar, roles `fecs_code` (fuc409c) `fecs_data` (fuc409d)
  `gpccs_code` (fuc41ac) `gpccs_data` (fuc41ad); no rows.
* `unaos/arroyo` KEPLER-FW block (beside WIFI-FW in `esp_x86_unafs_card`): `UNAOS_KEPLER_FW_PATH=<abs bunker dir>`
  stages the four into `target/kepler-fw` and onto the ESP tree at `FW/KEPLER/` (p1, which una-card writes), prints
  each `sha256=… bytes=… pin-row: <role> <hex>` and `firmware/kepler.pins vs the staged set -> MATCH|UNPINNED|MISMATCH`.
  Unset: nothing staged, image unchanged. Relative path or a missing file: the build fails naming it. The block was
  lifted out of arroyo and run on four files of RANDOM BYTES (not firmware) — §2.1. The builder (`unaos/builder`) is
  not touched: the copy onto the ESP tree is arroyo's, the WIFI-FW (FWVOL) shape.
* `unaos/crates/kernel/src/drivers/gpu/kepler_ucode.rs` (new, `kepler_fifo::ucode`, kgr's gate) — at `tests kgr` only
  (R80), after the PFIFO preamble raises PGRAPH and before any channel is bound: read `FW/KEPLER/` from the
  program-source FAT, SHA-256 each against the pins, refuse `absent` / `unpinned` / `pin-mismatch` / `size` /
  `unreadable` with nothing written; check sizes against each falcon's HWCFG; upload FECS then GPCCS (DMEM, then IMEM
  with a tag per 256 bytes, zero-padded); start both by nouveau's EXTERNAL-firmware sequence; wait 0x409800 bit 0
  (2 s); then the watchdog method (0x21) and the main context image size method (0x10) — `ctx_size=` is the first
  fact the golden-context generator needs. 0x409504 is touched only after the firmware reported ready.
* `kepler_fifo.rs` `kgr`: calls it, carries `ucode=<running ctx_size=…|why>` in KGR4's `fecs=` word, and GATES the
  SET_OBJECT whenever FECS reads running (`ucode::GOLDEN_CONTEXT = false`): `set_object=gated` with
  `DECLINED reason=no-golden-context` (our firmware running) or `reason=fecs-running-without-our-ucode`. Halted after
  our firmware reported ready reads `DECLINED reason=fecs-halted-after-start`. Halted with no firmware is KEPLERGR4's
  `PASS (ucode-gated)`, unchanged.

### 2.1 arroyo KEPLER-FW block, exercised (random bytes, NOT firmware; harness lifted the block verbatim)

```
== case unset          rc=0 esp=0
== case relative       ❌ KEPLER-FW: UNAOS_KEPLER_FW_PATH must be absolute: rel            rc=1
== case unpinned       ⚡ KEPLER-FW: GK107 ctxsw set (4 files) from …/bunker onto the ESP at FW/KEPLER/ (bench-only, never in-tree)
                       ⚡ KEPLER-FW: fuc409c sha256=2432b5d3…aa95 bytes=1000 pin-row: fecs_code 2432b5d3…aa95   (x4)
                       ⚡ KEPLER-FW: firmware/kepler.pins vs the staged set -> UNPINNED (…)   rc=0 esp=fuc409c fuc409d fuc41ac fuc41ad
== case match          ⚡ KEPLER-FW: firmware/kepler.pins vs the staged set -> MATCH (…)      rc=0
== case mismatch       ⚡ KEPLER-FW: firmware/kepler.pins vs the staged set -> MISMATCH (…)   rc=0
== case missing        ❌ KEPLER-FW: …/bunker/fuc41ad missing or unreadable — no card image   rc=1
```

A MATCH line from a REAL build is owed: it needs a bunker set (§1) and its pins.

## 3. STOPs

1. **Source (§1).** The four files have no linux-firmware source. DECISION for Peter, in R52's shape:
   (1) the bench downloads NVIDIA's proprietary Linux driver package and cuts `fuc409c/409d/41ac/41ad` for chipset E7
   with nouveau's extraction script into the bunker (`drivers/nvidia-gk107/` + PROVENANCE with the NVIDIA licence);
   (2) route (a), nouveau's MIT GK104 hub/gpc ucode (keplergr4.md §3), which needs a different start (below) and no
   bunker; (3) route (b), our own.
2. **The brief's hub-start facts belong to a different firmware.** "0x409100 = 2, wait 0x409800 bit 31"
   (gf100.c:1867-1871) is `gf100_gr_init_ctxctl_int` (:1832-1880): nouveau's OWN ucode, which also loads the register
   lists into DMEM (`gf100_gr_init_csdata`, :1861-1865). NVIDIA's four files take `gf100_gr_init_ctxctl_ext`
   (:1751-1830; chosen at :1883-1893 by `gr->firmware`, set at :2610): both falcons started (:1790-1791), ready =
   0x409800 **bit 0** (:1793-1797), then the method interface. The kernel implements the ext facts; the int facts are
   cited, not used.
3. **The golden context is a rung of its own and is not written.** With firmware, `gf100_grctx_generate`
   (ctxgf100.c:1436-1558) forces FE power on (0x404170, :1446-1451), RESETS FECS (:1457), inits SCC RAM (0x40802c,
   :1470), allocates `CB_RESERVED + gr->size` (:1473-1474 — `gr->size` is the `ctx_size` this arc now reads), writes
   the instance block's 0x210/0x214 context pointer (:1488-1489), binds it through FECS (:1495), runs `grctx->main`
   (:1515; `gf100_grctx_generate_main` :1342), has FECS save the golden image (:1531), clears 0x409b00 bit 31 (:1535)
   and keeps the image (:1538-1542). For the GK107 `main` replays ctxgk104.c's lists — icmd (`gk104_grctx_pack_icmd`
   :277), KEPLER_A method defaults (`gk104_grctx_init_a097_0` :283-583, pack :584), hub (`gk104_grctx_pack_hub` :702),
   gpc (:743, :753), tpc (:812), ppc (:836) — plus floorsweep/tile/alpha-beta tables (`gk104_grctx_generate_*`
   :849-966, func :968). It also presumes `gk104_gr_init` (GR's own MMIO init, gr/gk104.c) has run, which this tree has
   never done. Two bodies of work, both needing the firmware live on metal to verify, so the generator was not written
   blind; `ucode::GOLDEN_CONTEXT` gates KEPLERGR4's push until it exists. **The brief's expected wire
   `:: KGR4: … set_object=ok … landed=1 -> PASS ::` is therefore not reachable from this arc.**
4. **No unwind for the falcons.** A started FECS/GPCCS is left running when the rung declines; nouveau has no "stop"
   either (it resets FECS inside the generator, :1457). One run per boot (`kgr`'s RAN latch) bounds it.

## 4. Citation table (facts only, R95 §2; nouveau at torvalds/linux master, read 2026-10-13)

| fact | value | source file:line |
|---|---|---|
| firmware names, opt-in | fuc409c fuc409d fuc41ac fuc41ad, `NvGrUseFW` | gr/gf100.c:2596-2611 (:2600, :2603-2606) |
| request path | `nouveau/nv%02x_<name>`, then `nouveau/<name>` | gf100.c:2578-2582 |
| ext vs int | `gr->firmware ? _ext : _int` | gf100.c:1883-1893 |
| falcon bases | FECS 0x409000, GPCCS 0x41a000 | gf100.c:2641-2642, :2648-2649 |
| falcon loaders | v1 load_imem / load_dmem / start | gf100.c:2163-2166 |
| ext load order | PMC 0x260 = 0; FECS DMEM, IMEM; GPCCS DMEM, IMEM; 0x260 = 1 | gf100.c:1759, :1764, :1771, :1783; `gf100_gr_init_fw` :1700-1705 |
| PMC unk260 | write 0x000260 | subdev/mc/gf100.c:62-65; gk104 uses it, mc/gk104.c:59 |
| DMEM upload | +0x1c0 = start \| BIT(24); words to +0x1c4; tail masked | falcon/v1.c:68-89 |
| IMEM upload | +0x180 = start \| BIT(24); tag to +0x188 every 64 words; words to +0x184; pad to 64 words | falcon/v1.c:29-65 |
| falcon start | CPUCTL(+0x100) bit 6 ? +0x130 = 2 : +0x100 = 2 | falcon/v1.c:93-101 |
| memory limits | +0x108: IMEM = (r & 0x1ff) << 8, DMEM = (r & 0x3fe00) >> 1 | falcon/base.c:273-275 |
| ext start | 0x409800 = 0, 0x41a10c = 0, 0x40910c = 0, GPCCS then FECS | gf100.c:1786-1791 |
| ext ready | 0x409800 & 1 within 2000 ms | gf100.c:1793-1797 |
| watchdog | 0x409800 = 0, 0x409500 = timeout, 0x409504 = 0x21; timeout 0x7fffffff | gf100.c:973-980, :1799 |
| ctx image size | 0x409800 = 0, 0x409500 = 0, 0x409504 = 0x10; answer = non-zero 0x409800 within 2000 ms | gf100.c:957-970, :1802 |
| ctxctl is GR init's last step | `return gf100_gr_init_ctxctl(gr)` | gf100.c:2463 |
| int path (NOT used) | csdata lists; 0x40910c = 0, 0x409100 = 2; wait 0x409800 bit 31; size = 0x409804 | gf100.c:1832-1880 |
| golden context | `gf100_grctx_generate` | ctxgf100.c:1436-1558 |
| GK104 grctx lists | icmd, a097, hub, gpc, tpc, ppc; generators; func | ctxgk104.c:277, :283-584, :702, :743, :753, :812, :836, :849-966, :968 |

## 5. Expected wire after `tests kgr` on the rMBP

With this tree and no bunker set (the card as it can be built today):
```
[kgr] ucode dir=/FW/KEPLER/ -> absent (no /FW/ on the ESP; arroyo's KEPLER-FW block stages it from UNAOS_KEPLER_FW_PATH)
[kgr] ucode set=FW/KEPLER pins=0/4 -> absent (nothing written; FECS stays as found)
[kfifo] preamble … -> ok
…
[kgr] fecs chipset=E7 class=A097 cpuctl=00000010 chan_cur=00000000 ctx210=00000000 -> halted
:: KGR4: class=0xA097 set_object=gated method=i2m bytes=256 landed=- preamble=ok fecs=halted cpuctl=00000010 ucode=absent -> PASS (ucode-gated) ::
```
With a pinned bunker set on the card (after §3.1 is ruled and the rows pasted), the firmware starting:
```
[kgr] ucode fuc409c role=fecs_code bytes=<n> sha256=<hex> -> pinned        (x4)
[kgr] ucode limits fecs hwcfg=<…> imem=<…> dmem=<…> gpccs hwcfg=<…> imem=<…> dmem=<…> set <…> -> fits
[kgr] ucode start fecs cpuctl=00000010-><…> gpccs cpuctl=<…>-><…> mailbox=<…> ready_us=<…> watchdog=7FFFFFFF ctx_size=0x<…> size_us=<…> -> running
[kgr] fecs chipset=E7 class=A097 cpuctl=<…> chan_cur=<…> ctx210=00000000 -> running
[kgr] setobj class=A097 subch=0 -> gated(no-golden-context; ctx210=00000000)
:: KGR4: class=0xA097 set_object=gated method=i2m bytes=256 landed=- preamble=ok fecs=running ucode=running ctx_size=0x<…> -> DECLINED reason=no-golden-context ::
```
Other reads: `-> refused reason=unpinned` (first boot with an unpinned set: paste the KEPLER-FW rows), `pin-mismatch`,
`-> ready-wall` (the firmware never set 0x409800 bit 0 — GR's own init, `gk104_gr_init`, has not run; the likely first
metal answer), `-> size-wall`, `refused reason=hwcfg`.
