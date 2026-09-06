# GA10B-HISTORY — every GPU and display-engine probe on the Orin, its verdict, and what was shut out

Seat orin 14, executor GA10BHIST, 2026-09-06. Tree at `c3cf355e` (hw-jetson TIP). Read-only compilation; no
code touched. Serves `docs/dev/RULINGS.md` R19 (Peter, 2026-09-06, not yet in the tree at `c3cf355e`; quoted
from the brief): *"i've seen it a few times when you're running through the 3D card probes where you take a
path that does not work out so you shut it out but after very many boots its discovered that path needs to
be open for later paths to succeed"* / *"the previous fails may be recorded somewhere"*. They are; this is the
record. Companion: `GA10B-LADDER.md` (executor GA10B, in parallel) — this file states no rung opinions.

Conventions. Capture line numbers are `~/unaos-bench/capture/line-acm0/orin.log` (56,018 lines at compile
time; `raw.log` carries the same `[ga10bprobe1]` lines; `unknown.log` has none). "Words" are the verdict
vocabulary as recorded at the time, verbatim. SHUT OUT = code deleted, gated to nothing, or a doc that says
never; KEPT = the knob/const is still in `unaos/crates/kernel/Cargo.toml` or the source. `od` =
`docs/dev/OS/08_VIDEO/orin-desktop.md`; `aa` = `docs/dev/OS/01_BOOT_HAL/arch_arm64.md`; `o3d` =
`unaos/docs/dev/OS/09_PLATFORM/orin-3d.md`; `crp` = `docs/MANIFESTO/CLEAN_ROOM_POLICY.md`; `dt` =
`unaos/crates/kernel/src/arch/aarch64/display_tegra.rs`; `gp` = `.../ga10b_probe.rs`; plans under
`~/.claude/plans/unaos/`.

## 1. The record, chronological — one row per attempt, rung or decision

| # | date | arc / session | what was tried | boot(s) and wire | verdict as recorded then | words used | SHUT OUT / KEPT | what a later rung needs from it | source |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 2026-07-06 | JX1 (orin, pre-baton) | Guarded read of the xHCI capability block @`0x0361_0000` after ExitBootServices, no BPMP ungate | one boot; first read → SError, `ESR 0xbe000011` (EC=0x2F), BL31 `Unhandled Exception in EL3` (capture `serial-orin-jx1.log`) | "the block is EL3-fatal to touch post-EBS; BPMP ungate required first" | "EL3-fatal", "the probe was removed after one boot (it would kill every boot)" | SHUT OUT then — code deleted at `c79d6ba1`. **RE-OPENED** by JB1c: PG 12/10 ON + 9 clocks, then the same address → `XUSB ALIVE: xHCI v1.20` (every boot since; capture 25771-25772 `probing XUSB host @0x3610000 (was EL3-fatal in JX1)`) | The founding R19 case on this board: the *address* was innocent, the *precondition* (rail on) was missing. Record conditions, not addresses | `aa:1267-1290`; `5cf97830`, `c79d6ba1`, `12e7ee44` |
| 2 | 2026-07-08 | JD1 (orin) | Inherit the DTB `simple-framebuffer` scanout (pure RAM walk, no display MMIO). `jd1_dc_survey` written alongside as a **default-off** register fallback behind `pub const JD1_DC_PROBE: bool = false`, window offsets from Linux `drm/tegra` `hub.c` (T186/T194) | attended, scanout `base=0x279e00000 size=0x960000 1920x1200`; the DC fallback "was NEVER needed" | METAL-CONFIRMED (scanout); DC probe never run | "default-off", "the panel is lit so it is *believed* powered", "Flip to `true` at the bench … only if the DTB handoff is absent", "NEVER powergate/reset the display (MRQ_PG DESTROYS the scanout)" | KEPT — `dt:56` const still `false`, caller `dt:101`, body `dt:250` | Nothing from its offsets (row 10 convicts them); the SMMU-IOVA/bit-39 note in its doc is unverified on this chip | `6b65e1ee`, `a9b50688`; `aa:2405-2470`; memory archive `unaos-jetson-resume.md:236,260` |
| 3 | 2026-07-17 | Research verdict (LC-orin R21) | Decision only: GPU work goes to the Pi's V3D | none | "GPU stays Pi-only (Orin GA10B = signed falcon ucode + months-scale nvgpu transcription)" | "GPU stays Pi-only" | SHUT OUT (planning); superseded by row 15 | — | memory archive `unaos-jetson-resume.md:9` |
| 4 | 2026-08-18 | orin 0 / M3 (`422c9f0a`) | `orin-3d.md`: status statement, no probe | none | "zero GPU code on this branch"; "GA10B is firmware-gated"; "Firmware is the wall, not the registers"; §4 do-not list | "firmware-gated", "The licensing decision is Peter's", "Do not probe powergated blocks", "Do not touch nvdisplay MMIO" | doc prohibition (see S4) | The evidence vocabulary it imports from GK107 (sitting id / DERIVED / EXT / UNPINNED) | `o3d:143-193, 283-298`; `review/orin-0-LANDING.md:12` |
| 5 | 2026-08-22 | orin 3 landing / orin-4 baton | Research (edk2-nvidia, L4T partitions, Linux 7.2, open-gpu-kernel-modules): no probe | none | GPU: "CLOSED — do not restart. GA10B acceleration: a wall, not a cost" (AES-encrypted PKC-signed ucode; `OPT_PRIV_SEC_EN`; MB2 loads no GPU binary on two boots; no `A_gpu-fw` partition; `grep -rni ga10b` Linux 7.2 → 0; open modules compile GA10B out). Display: "OPEN AND PROMISING — REFUTED that the DCE holds it" (`NvDisplayHw.c` raw MMIO; `grep dce` → 0) | "CLOSED — do not restart", "a wall, not a cost", "XUSB had a running falcon to inherit, GA10B has nothing", "The real blocker is documentation, not permission" | doc verdict, no code | The four firmware-free display requirements it names: BPMP, SMMU stream `TEGRA234_SID_ISO_NVDISPLAY`, TCA9539 GPIO, the DCB | `past/batons/orin-4.md:277-300`; `review/orin-3-LANDING.md:129-135` |
| 6 | 2026-08-22 | ORIN-WM1 + JD1-DC (`ab168ba2`) | The survey moved to the end of `tegra_early_stop`'s BPMP block behind `MRQ_PG GET_STATE` for every `display@` power domain; knob `jd1dc`; five `VERDICT=` arms; first touch `DISPLAY_FE_SW_SYS_CAP` +0x30000 (UEFI's own first read); FIRST-TOUCH/SURVIVED pair | none (UNFLOWN) | "prepared and unfired"; "Read-only is enforced, not intended" | "UNFLOWN", "REFUSED reason=aperture-too-small" | KEPT — `jd1dc = []` `Cargo.toml:1921`; leg `arm-tegra-jd1dc` `arroyo:2906` | The guard (PG ids from the DTB, `err=0 state=0x1` each) and the 2026-08-22 correction: `Chan::transfer(66,&[1,id,1])` CAN send SET_STATE — a convention, not a type guarantee (`dt:735-742`) | `ab168ba2`; `aa:10070-10200`; `dt:720-760` |
| 7 | 2026-08-25 | JD1-DC-MODEL (`24284e50`) | Four more reads inside the same guard: `FE_CLASSES` +0x0, `FE_HW_SYS_CAP` +0x60, `FE_HW_SYS_CAPB` +0x64, `FE_CHNCTL_CORE` +0x4e0 (open-gpu-doc gv100/tu102/ga102, identical); `MODEL-VERDICT=` axis; `CLASS_ID` compares `0xC670` not `0xC67D` | none at commit | "UNFLOWN. Every claim about what a register WILL read is a PREDICTION" | "UNDETERMINED reason=discriminator-not-read", "REFUSED reason=no-reads", "DISCRIMINATOR-TRIVIAL" | KEPT (same knob) | `FE_CHNSTATUS_CORE` +0x630 named as the next read (taken by row 12) | `24284e50`; `aa:10208-10380` |
| 8 | 2026-08-25 | boot7e run 1 (image `24284e50`-era, orin 5) | JD1-DC + MODEL flown, then the T194 window sweep | capture 8765-8794: `JD1-DC-REG entry[0] addr=0x13800000 size=0xeffff` (8766); `power-domain[0] id = 3` … `PG 3 GET_STATE … err=0 state=0x1`; `GUARD PASSED: all 1 display@ power domain(s) ON`; `FIRST READ SURVIVED: DISPLAY_FE_SW_SYS_CAP=0x00100303` (8777); `FE_CLASSES … = 0xc6700410`; `MODEL-VERDICT=NVDISPLAY-CLASS-C670` (8792); `about to read win0 WIN_OPTIONS @0x13802e00` (8793); `Unhandled Exception in EL3.` (8794); `esr_el3 = 0x00000000be000011` (8835) | the sweep is EL3-fatal; the block is NOT powergated and decodes at the FE words | "EL3-fatal", "an offset INSIDE the DTB-declared aperture", "convicted" | see row 10 | The three FE words and `CHNCTL_CORE=0x00000021` (EFI(5)=1) are metal facts from this boot | `aa:10422-10429`; `od:1043-1057` |
| 9 | 2026-08-25 | boot7e run 2 | identical image, second attempt | capture 9913-9942, same lines verbatim; `esr_el3` at 9983 | reproduced | — | — | — | `aa:10429` |
| 10 | 2026-08-25 | JX1-WINSWEEP (`04d46aae`) | The T186/T194 sweep gated to an empty slice: `for &head_off in &[] as &[u64]` (`dt:963`); block comment records the kill | none (image built for boot7f) | "the window sweep is EL3-fatal on this silicon, and boot7e's own discriminator says why" | "GATED OFF, not deleted", "It returns when its offsets are rewritten against the NVC67D class-channel model", "No rung on this ladder may reintroduce it against the T194 offsets" (`od:1056`) | SHUT OUT — code kept, unreachable; doc says never *against T194 offsets* | Its body is "exactly what a save/restore would be reconstructed from"; the per-read FIRST-TOUCH announce | `04d46aae`; `dt:926-963`; `od:3039-3047` |
| 11 | 2026-08-25 | boot7f (`boot7f-nowinsweep-20260825T2034Z-04d46aa`) | JD1-DC + MODEL, sweep gated | capture 11061-11090: FE words reproduced; `MODEL-VERDICT=NVDISPLAY-CLASS-C670` (11088); `JD1-DC CENSUS — heads=0 windows=0 … reads=5 writes=0` (11089); `VERDICT=DECODES-NOMATCH` (11090) | "the model question is answered"; "The tree's window map is Tegra186/194's and is wrong for this chip" | "DECODES-NOMATCH" — with a false cause ("no complete head-0 window bank") retracted in row 12 | — | `swept=0` on this capture is BY CONSTRUCTION, not measured | `b7900d64`; `aa:10381-10430`; `od:972-1057` |
| 12 | 2026-08-25 | JX2-NVC67D (`cc6a71c5`) | Channel census, read-only, no channel opened: `CHNSTATUS_CORE` +0x630, `CHNCTL_CORE` +0x4e0, `IP_VER` +0x18, `MISC_CONFIGA` +0x74, `HW_LOCK_PIN_CAP` +0x68, per-window `CHNCTL_WIN`/`CHNSTATUS_WIN`/`CHNSTATUS_WINIM`, per-head `CHNSTATUS_CURS`, last the +0x1c00 interrupt page. Adds `JX2-SWEEPDISABLED` and the in-verdict retraction. Sources MIT only (`ga102/dev_display_withoffset.ref.txt`, `clc67d.h`) | none at commit | "THE DESIGN CHANGE IS THAT IT IS NOT A SWEEP" — window surface state is channel methods (`NVC67D_UPDATE` 0x200), not MMIO | "UNFLOWN", "no channel is opened" | KEPT (rides `jd1dc`; `dt:1892-2100`) | `NVC67D_WINDOW_SET_CONTROL(a)_OWNER` (0x1000+a*0x80) is what binds a window to a head — cited, not coded | `cc6a71c5`; `whiteboards/orin-5-WHITEBOARD.md:83-87` |
| 13 | 2026-08-25 | boot7g (`boot7g-clickchrome-20260825T2124Z-1f2545c`) | JX2 flown | capture 12548-12659: 26/26 NEXTTOUCH/READSURVIVED; `CHNSTATUS_CORE … = 0x20070000` (12578, STATE=0x07 `EFI_OPERATION`); `CHNCTL_CORE = 0x00000021`; `IP_VER = 0x04020000`; `MISC_CONFIGA = 0x00404202`; `HW_LOCK_PIN_CAP = 0x00000020`; `CHNCTL_WIN(0) = 0x00000001`, WIN(1..3) = 0; `CHNSTATUS_WIN(0)=0x24010100` vs 1..3 `0x21000100`; first touch of the +0x1c00 page survived, `HEAD_TIMING(0) = 0x00000007` (12636); `JX2-VERDICT=EFI-OWNED-LIVE` (12656); `JX2-SWEEPDISABLED` (12657); retraction inside `DECODES-NOMATCH` (12659) | "the firmware still owns the display"; "the next display rung is a deliberate handoff … never an opportunistic MMIO poke" | "EFI-OWNED-LIVE", "Nothing about the handoff", "Nothing about window-to-head binding" | — | Window 0 is the firmware's (allocated); windows 1..3 read unallocated; the interrupt page decodes | `b710fe4c`; `aa:10478-10600`; `od:1214-1225` |
| 14 | 2026-08-25 | boot7h (`boot7h-conwin-net4-20260825T2208Z-68c4758`) | JX2 again | capture 14165-14276; `JX2-VERDICT=EFI-OWNED-LIVE` (14273), all reads survived | "a two-flight result" | — | — | — | `c2066c11`; `aa:10598-10602`; `od:1525-1527` |
| 15 | 2026-08-25 | Peter's ruling → `crp` §6 (`56749025`), facts ACK (`5b51427a`) | Quarantined Group-A fact extraction over GPL `nvgpu` L4T r36.4.3 (tarball sha `2c177804…`); 26-entry facts file; independent COI review | none | "ADJUDICATED — ADMISSIBLE under §6"; review "ACK-WITH-EDITS"; orin-5 whiteboard had recommended "Option 2 for now … GA10B stays a wall" | "Option 1", "admissible", "ACK-WITH-EDITS", "may now inform kernel code" | — (policy) | See §3 | `crp:63-120`; `whiteboards/orin-5-WHITEBOARD.md:32-45`; `whiteboards/orin-6-WHITEBOARD.md:56-60,134-145` |
| 16 | 2026-08-25 | GA10B-PROBE1 (`a8e18901` WIP, `cd568275` fix; exec shas `878d1cf7`/`3ffa56c3`) | `ga10b_probe.rs::ga10bprobe1_run`: BPMP `MRQ_PG` (66) `CMD_PG_GET_STATE` for the DTB `gpu@` power domain FIRST; then announce-before-read of 5 BAR0 words; ends `power::shutdown()` (PSCI SYSTEM_OFF); knob `ga10bprobe1 = ["tegra"]`; leg `arm-tegra-ga10bprobe1`. WIP trap: the call and `pub mod` were appended AFTER a trailing `//` — inert, caught by the strings negative control | none at commit; knob-off flat image `c3ae5a49…` byte-identical | "the first read-only GA10B iGPU probe rung" | "gates green", "reachable (30 `[ga10bprobe1]` hits)" | KEPT — `Cargo.toml:1939`; `arroyo:869,3418` | The negative-control lesson: "compiled" is not "reachable" | `a8e18901`, `cd568275`; `gp:1-60,140-319`; `main.rs:2254` |
| 17 | 2026-08-26 | O3D flight (`ga10bprobe1-20260826T0247Z-3ffa56c`, MANIFEST "COLD-BOOT, SEPARATE MEDIA BY DESIGN"; `marks.txt:8` `MARK o3d … dark=success`) | rung 1 flown | capture 25777-25794: `gpu@ node: BAR0=0x17000000 (DTB reg[0]) power-domain-id=35`; `GA10B-RAIL-POWERED reg=bpmp-mrq-pg val=0x00000001 (err=0 state=0x1)`; `GA10B-SECURE-FUSED reg=0x17820434 val=0x00000001`; `GA10B-PRIVLOCK-ENGAGED reg=0x171100f4 val=0x0001b733 (bit13=1)`; `GA10B-BROM-NEVERRAN reg=0x1711165c val=0x00000000`; `GA10B-CORE-HALTED reg=0x17111388 val=0x00000010`; `GA10B-GPC-CENSUS=2 reg=0x17022430 val=0x00000002`; `rung 1 complete — … zero MMIO writes … SYSTEM_OFF`; `[orinshutoff] PSCI SYSTEM_OFF (0x84000008)` (25794) | "Flew clean … The rail is ON and nothing has booted the GPU"; "PHASE E — THE GPU, NOW ACTUALLY OPEN" | "O3D = Peter's name for it; `ga10bprobe1` is retired vocabulary" (knob name unchanged) | KEPT | Rail ON by default at handoff; BAR0 decodes at fuse, GSP falcon, priscv and top apertures; GSP never bootstrapped | `batons/orin-9.md:134-146`; `batons/orin-10.md:845-846`; `docs/dev/evidence/orin13/ORIN-HW.md:56,61` |
| 18 | 2026-08-25/26 | JX3 — display channel handoff | Named only: "one register-step per boot (the JX3 model)"; "Display channel handoff (JX3) when its turn comes" | none | never written | "when its turn comes" | never started — no knob, no commit, no code (`grep -rn JX3` → 3 prose hits) | Everything in S6 | `unaos/docs/dev/OS/09_PLATFORM/ga10b-clean-room.md:59-61`; `past/batons/orin-6.md:129-130`; `review/orin-autodebug-plan.md:85-91` |
| 19 | 2026-08-31 | EL0JD1DC (`427580f9`, fold `92d4dad4`) | Check-matrix leg `arm-tegra-el0-jd1dc` = `arm-tegra-el0` + `jd1dc`; go-red proven two-sided | none (type check) | "THE CROSS COMPILES" — boot7e-7h flew `jd1dc` + `tegra_el0` with no row covering it (four flights, not five) | "the hole", "still open: `jd1dc` x `orindesk`/`orinclick`/`orinconwin`/`smpmark` are 0 rows" | KEPT | boot7e-7h remain a superset of no leg — a media-fingerprint question | `427580f9`; `arroyo:3769,3677-3709` |
| 20 | 2026-09-05 | ORIN-HW / orin-ledger §F (`7a587f14`, `aaaa62a0`) | Inventory, no probe | none | GPU: "RULED OUT … a licensing ruling, not an arc"; display engine: "RULED OUT (orin-desktop.md §4) for this ladder; nothing exists", "L / RULED OUT" | "RULED OUT" | doc verdict | Note the scope words "for this ladder" | `ORIN-HW.md:54,56,123-125`; `orin-ledger.md:142,144,214-215` |
| 21 | 2026-09-05 | D3 dropped (`6cc8de8c`) | Ledger reconcile | none | "the GA10B licensing question was ruled on 2026-08-25; the 'pending' line was a phantom obligation (P10)" | "dropped — ruled 2026-08-25; the arc, if ever, is a hardware question" | — | **Residual at `c3cf355e`:** `orin-ledger.md:144` (§F GPU row) still reads `Pending Peter: "GA10B licensing/bunker ruling"` — the commit fixed `ORIN-HW.md:56` and ledger D3 (`:75`) but not this cell | `6cc8de8c`; `orin-ledger.md:75,144` |
| 22 | 2026-09-06 | R19 (Peter) | — | — | the ruling this file serves | "shut it out", "needs to be open for later paths" | — | — | brief; `RULINGS.md` (to be appended by the seat) |

## 2. Shut-out register — each closed path, the exact condition it failed under, and the R19 re-open reading

**S1. XUSB @`0x0361_0000` post-EBS (row 1).** Failed under: UEFI's ExitBootServices teardown had clock/power-gated
the XUSB partition; no BPMP transaction had been issued. Not tried then: any MRQ. Shut-out kind: probe code
deleted (`c79d6ba1`). Re-opened: JB1c (PG 12 + PG 10 ON, nine `MRQ_CLK` enables) and the identical read returns
`XUSB ALIVE` on every boot since. R19 reading: the recorded verdict "EL3-fatal to touch" was true only of the
state, not the address. This is the precedent every later "EL3-fatal" line cites (`dt:724`, `gp:14`, `o3d:288`).

**S2. The Tegra186/194 nvdisplay window sweep (rows 2, 8-10).** Failed under: DISP power domain 3 ON (`GUARD
PASSED`), aperture `0x13800000` size `0xeffff` from DTB `reg[0]`, silicon identified as NVD_40 / `NVC67D`
(`FE_CLASSES=0xc6700410`), core channel EFI-owned; the first read at `head+0x2800+0x600` = `0x13802e00` took the
EL3 abort, twice. What was NOT tried: any read between +0x800 and +0x1bff or above +0x1c38 in the FE block; any
offset in the ga102 manual's window/head/SOR register ranges; anything after taking ownership of a channel.
Shut-out kind: gated to an empty slice (`dt:963`), body retained; doc: "No rung on this ladder may reintroduce
it against the T194 offsets" (`od:1056`); ORIN-HW: "display engine … RULED OUT for this ladder". R19 reading:
the conviction is of the *T194 offsets on this silicon*, not of window registers as a class — JX2 subsequently
read `FE_CHNCTL_WIN(0..3)`, `FE_CHNSTATUS_WIN(0..3)`, `FE_CHNSTATUS_WINIM(0..3)` and the +0x1c00 interrupt page
without fault (row 13). The commit itself names the re-open condition: "It returns when its offsets are
rewritten against the NVC67D class-channel model" (`04d46aae`). Bounds checking against the DTB length cannot
protect a re-opened sweep ("INSIDE the aperture and still not decodable"); only announce-before-read can.

**S3. `JD1_DC_PROBE = false` (row 2).** Never flown as such; its reason ("if the display block were powergated
the first register read would be EL3-fatal") was measured false for the FE block on 2026-08-25 (PG 3 ON, five
then twenty-six reads survived). Shut-out kind: default-off const, comment says flip only at the bench with the
DTB handoff absent. R19 reading: nothing to re-open — the same body is reached via `jd1dc` at the guarded site;
the const-guarded caller at `dt:101` is a dead twin that predates the guard and should not be flipped.

**S4. "Do not touch nvdisplay MMIO" (rows 4, 20).** Stated 2026-08-18 before any nvdisplay register had been
read; updated 2026-08-25 with the boot7e conviction (`od:3039-3047`). Measured since: 5 + 26 + 26 read-only
touches at MIT-documented `NV_PDISP` offsets, zero faults, three flights. Shut-out kind: standing prohibition
in `o3d` §4.2 and `od` §4; ORIN-HW row 54 "RULED OUT (orin-desktop.md §4) for this ladder". R19 reading: the
prohibition's measured content is (a) no writes while the core channel is EFI-owned and feeding the panel, and
(b) no T194 offsets; the doc words are broader than the evidence. "For this ladder" is a scope qualifier — the
desktop ladder needs no engine (`od:3018-3019`); it is not a measurement about the engine.

**S5. Display power-domain SET_STATE.** Never tried, by rule: cycling PG 3 tears down the inherited scanout
("unrecoverable for this boot", `dt:844`; "MRQ_PG DESTROYS the scanout", memory archive :260). Not a shut-out
but a live hazard, and only a convention: `Chan::transfer` is `pub` and can send it (`dt:735-742`).

**S6. JX3 — the channel handoff (rows 13, 18).** Never started. Established preconditions: `EFI-OWNED-LIVE`;
2 heads, 2 SORs, 4 windows; `CHNCTL_WIN(0)=1` (firmware's), WIN(1..3)=0; `NVC67D_WINDOW_SET_CONTROL(a)_OWNER`
cited as the binding method; interrupt page decodes. UNKNOWN: whether a window channel can be allocated while
the core channel stays EFI-owned; the push-buffer/method-FIFO geometry for this class on Tegra; the DCB; the
SMMU stream for a CPU-fed surface. The autodebug plan called this "where auto-debug would pay first" (no
licensing problem); not acted on. No knob exists.

**S7. GA10B GPU-core acceleration (rows 3-5, 17, 20).** Recorded as "CLOSED — do not restart" (2026-08-22),
"RULED OUT" (2026-09-05) and "NOW ACTUALLY OPEN" (orin-9, after O3D) — three verdicts, one of which postdates
the only measurement. Measured condition (row 17): rail ON at handoff without any MRQ; `opt_priv_sec_en=1`;
GSP BR priv-lockdown engaged (`hwcfg2=0x0001b733`); BR never reached a verdict; GSP RISC-V halted; 2 GPCs.
What was NOT tried: any write (BCR config, `priscv_cpuctl` startcpu, `pgsp_falcon_engine` reset — all recorded
in the facts file and marked "probe omits"); the other fuses the facts file carries (`opt_sec_debug_en`
0x821040, `opt_wpr_enabled` 0x8205ec, `opt_vpr_enabled` 0x82067c); `top_device_info_cfg`/`device_info2`;
`mc_enable`/`mc_elpg_enable`; the PMU falcon2 at 0x10b000; `MRQ_STRAP`; any GR/CE/2D engine register; any
GPU clock MRQ. Shut-out kind: doc verdicts only — the knob is KEPT. R19 reading: the "wall" statements are about
signed GSP ucode; whether that wall gates every engine on the die or only the GSP-managed paths is unmeasured,
and orin-9 named exactly that as rung 2's question ("what can be read or driven from the CCPLEX with the GSP
halted").

**S8. "GPU stays Pi-only" (row 3)** — superseded by row 15; no code. **S9. The licensing "pending" phantom
(row 21)** — closed by `6cc8de8c` except the residual cell at `orin-ledger.md:144`.

## 3. The facts file — what is imported, what the ack required, what is still unknown

File: `unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md`, 26 entries, ACKED under §6
2026-08-25 (`5b51427a`); source of record L4T r36.4.3 `nvgpu` in `~/unaos-bench/scratch/quarantine/nvgpu/`.

Imported, by name (BAR0-relative unless noted): aperture framing — BAR0 base is a DTB `gpu@` fact (EXT); GSP
falcon base `0x110000`; GSP falcon2/priscv base `0x111000`; PMU falcon2 `0x10b000`. (a) BPMP — no BAR0
register reports rail state (EXT: BPMP/DTB power domain); `MRQ_STRAP` sequence `{STRAP_SET, id, value}`;
Tegra234 strap ids OPT_GPC=1, OPT_FBP=2, OPT_TPC_GPC0=3, OPT_TPC_GPC1=4; BPMP reply semantics (EINVAL hard,
ENODEV/EACCES proceed); PG straps are BPMP-applied on silicon. (b) core selection FCD/DCS ⇒ RISC-V path;
legacy falcon `irqmask` 0x018, `irqdest` 0x01c, `idlestate` 0x04c, `cpuctl` 0x100 (startcpu bit1, halt_intr
bit4), `hwcfg` 0x108, `bootvec` 0x104, `dmactl` 0x10c, `hwcfg2` 0x0f4 (bit13); priscv `cpuctl` 0x388,
`br_retcode` 0x65c (PASS 0x3 / FAIL 0x2), `bcr_ctrl` 0x668, `bcr_dmacfg` 0x66c, BCR DMA addrs 0x670-0x684,
`riscv_irqmask` 0x528, `riscv_irqdest` 0x52c, `boot_vector` 0x380/0x384; the seven-step boot-ROM handshake
ordering (steps 1-4 are writes, "probe omits"); `pgsp_falcon_engine` reset `0x1103c0` with the 10 µs delay;
fuses `opt_priv_sec_en` 0x820434, `opt_sec_debug_en` 0x821040, `opt_wpr_enabled` 0x8205ec, `opt_vpr_enabled`
0x82067c; `top_num_gpcs` 0x022430, `top_device_info_cfg` 0x0224fc, `device_info2`; `mc_enable` 0x000200,
`mc_elpg_enable` 0x00020c, `mc_device_enable(i)`. Used by code so far: five (`gp:218-300`) plus the EXT rail.

The ack required (applied by a non-extractor editor): P1 recast as a silicon fact (no BAR0 register reports
rail state); P5 stated as a programming requirement, not a driver predicate; B7's "never bootstrapped" clause
downgraded to a marked inference with the `falcon.c:206-220` poll-loop pointer. Non-blocking: framing and step-5
pointers, B13 range widened with the delay.

Still UNKNOWN / not extracted: the Tegra234 GPU power-domain id's source of record (35 was resolved from the
DTB at runtime, never from a document); which `MRQ_CLK` ids and resets the GPU needs; the SMMU stream id for
the GPU; anything about GR, CE, FECS/GPCCS, ACR or memory apertures beyond `mc_*`; what a *completed* boot's
register state looks like is recorded but unexercised. Group boundary: the exec-ga10b seat that extracted is
Group A for GA10B and may not implement (`crp` §6, `ga10b-facts/README.md:17-19`). A second extraction has not
been commissioned.

## 4. The knobs that exist today for GPU/display, and which are dead

`grep -n 'ga10b\|jd1\|jx1\|jx2\|dc\|display' unaos/crates/kernel/Cargo.toml | grep '= \['` at `c3cf355e`:

| knob | Cargo.toml | arroyo env / leg | last touched (`git log -S`) | status |
|---|---|---|---|---|
| `jd1dc = []` | :1921 | `UNAOS_JD1DC` (:851); legs `arm-tegra-jd1dc` (:2906), `arm-tegra-el0-jd1dc` (:3769; comment :3677-3709) | `aaaa62a0` 2026-09-05, `7a587f14` 2026-09-05 (docs); code `427580f9` 2026-08-31 | LIVE — flown boot7e/7f/7g/7h; carries JD1-DC + MODEL + JX2 |
| `ga10bprobe1 = ["tegra"]` | :1939 | `UNAOS_GA10B_PROBE1` (:869); leg `arm-tegra-ga10bprobe1` (:3418) | `aaaa62a0`, `7a587f14` (docs); code `a8e18901` 2026-08-25 | LIVE — flown once (O3D, 2026-08-26); Peter's name for it is O3D |
| `v3d81_display`, `v3d_armedclose`, `v3d_bincontent` | :1317, :1473, :1513 | Pi V3D family | — | Pi, out of scope (the grep matches `display`/`dc`) |
| `JD1_DC_PROBE` (const, not a feature) | `dt:56` | none | `24284e50` 2026-08-25 (doc text only) | dead twin of the guarded path (S3) |

No knob, leg, or source file exists for JX3 (display channel handoff), for a GA10B rung 2, or for any write
path on either block. No GPU/display knob has been removed from `Cargo.toml` in the history searched; the only
code-level shut-outs are the JX1 XUSB probe deletion (`c79d6ba1`, re-opened) and the empty-slice gate (`dt:963`).
