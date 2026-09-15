# SHUTOUT REGISTER — every rMBP GPU ladder rung, and the conditions it failed under

**The ruling this file exists to serve — `docs/dev/RULINGS.md` R19 (Peter, 2026-09-06, orin 14),
verbatim:**

> "i've seen it a few times when you're running through the 3D card probes where you take a path
> that does not work out so you shut it out but after very many boots its discovered that path
> needs to be open for later paths to succeed"

Its enforcement, from the same row: *a probe rung that FAILS is recorded as "failed under
&lt;conditions&gt;" with its knob and code KEPT (never-trash), never "ruled out"; every ladder rung
names the earlier rungs it depends on being OPEN; a later rung may re-open an earlier failure as
its first step.* Carried into law at `docs/dev/LAWS.md:343`.

Ledger row: `docs/dev/OS/rmbp-ledger.md` B10. This file is the one home for the metal verdicts of
every rMBP GPU ladder; the per-ladder design documents keep the design and point here for the
verdicts.

## How to read a row

| column | meaning |
| --- | --- |
| **rung** | the ladder step, by the name its own code and log use |
| **knob** | the env knob (and Cargo feature) that arms it. `—` = unconditional on a Kepler boot |
| **witness token** | a literal substring of a `serial_println!` in the tree, verbatim, with `file:line` and `n` = `grep -rn -F '<token>' unaos/crates/kernel/src/drivers/gpu \| wc -l`. `awk` a capture for the token and the rung either spoke or did not |
| **last metal verdict** | the newest sitting or flight that ran it, with the verdict line quoted |
| **failed under** | every condition the capture states: firmware state, gmux owner, knobs, ASPM, which earlier rungs were open or closed. **Never "ruled out"** |
| **depends on** | the earlier rung that must be OPEN for this one to be able to pass |
| **status** | `proven` (its own falsifier was met on metal) · `open` (flown, no verdict claimed, still live) · `shut-out` (flown, failed, the ladder stopped using it — conditions named) · `never-run` (code and knob exist, never flown) |

Paths in the witness column are relative to `unaos/crates/kernel/src/drivers/gpu/`, except where a
row says otherwise.

Sources read for this compile: `KEPLER-METAL-LOG.md` (sittings #1–#43), `gen7.md` (R1–R7),
`PLAN-kepler-continuation.md` (K-GPU-1..4), `PCIE-RP-RECOVERY.md`, `phase31-root.md`,
`docs/dev/evidence/rmbp8/FLIGHT4-POSTMORTEM.md`, `docs/dev/evidence/rmbp9/FLIGHT5-POSTMORTEM.md`,
`docs/dev/OS/rmbp-ledger.md` (A1 A5 A7 B10 E1 E3), and the code itself
(`kepler.rs` `kepler_display.rs` `kepler_ce.rs` `gen7.rs` `igpu.rs` `pcihealth.rs`) plus the
`UNAOS_KEPLER*` / `UNAOS_IVB*` / `UNAOS_GMUX_IGD` / `UNAOS_NOASPM` knob comments in `unaos/arroyo`.

**⚠ Two documents this register contradicts, and the contradiction is the point.** `gen7.md` §2
records R6 and R7 as *pending metal* and §2.7 records R7 as *held dark*. Both flew, and both
**passed**, on flight 4 (2026-08-28, `docs/dev/evidence/rmbp8/FLIGHT4-POSTMORTEM.md` §3.1) — the
exact R19 shape: three rungs marked failed, then a fourth changed one condition and the ladder
walked straight through them. Where this register and a design document disagree, the capture
quoted here is the record.

---

## 1. Kepler display ladder — `kepler_display.rs`

Knobs: `UNAOS_KEPLER` (`nvidia-kepler`) + `UNAOS_KEPLER_TAKEOVER` (`nvidia-kepler-takeover`);
`UNAOS_KDISP_HOLD` (`nvidia-kepler-kdisp-hold`) adds the camera hold; `UNAOS_WC` (`wc`) arms the
compositor activation at the end of the seam.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| KD1 `caps` — display class probe | `UNAOS_KEPLER` | `:: kdisp: caps version=` (`kepler_display.rs:58`, n=1) | s11, 2026-07-22: `caps version=0210 class=917D` | — | — | **proven** |
| KD2 `gop` — firmware framebuffer located | `UNAOS_KEPLER` | `:: kdisp: gop phys=` (`kepler_display.rs:86`, n=1) | s11: `gop phys=0x90020000 vram_off=0x20000` | — | KD1 | **proven** |
| KD3 `head-raw` — EVO/CRTC per-head candidate decode | `UNAOS_KEPLER` | `:: kdisp: head[` (`kepler_display.rs:123`, n=6) | s5, 2026-07-22: `head-raw head=N evo=00000000 crtc=00000000` → `bad-read head N no valid candidates` ×4 | the 0x616100 ARMED-block offsets and the `head*0x800` stride of pull 4/6, with **no HEAD_STAT reading to bracket them**; all four heads read byte-identical, which is itself the stride collapsing. s10 formally reversed the "engine idle" reading this produced | KD1 | **shut-out** |
| KD4 `head stat` — raster counters | `UNAOS_KEPLER` | `stat underflow=` (`kepler_display.rs:140`, n=1) | s11: `head[0] stat underflow=0 vert=0x0493048A horz=0x0000068C`, heads 1–3 zero | — | KD1 | **proven** — and it is the rung that **re-opened KD3**: the engine was never idle, so every s5–s10 "all heads dead" reading was wrong-address decode |
| KD5 `evo-scan` — known-value sweep of 0x610000–0x613FFC | `UNAOS_KEPLER` | `:: kdisp: evo-scan hit off=` (`kepler_display.rs:232`, n=1) | s14, 2026-07-23: one hit in 16 KB, `0x6101E0 = 0x00000200`; s16: 18 hits incl. `0x640460 = 0x00000200` | — | KD4 | **proven** (as a locator) |
| KD6 `repoint` — write the armed pointer 0x6101E0 | `UNAOS_KEPLER_TAKEOVER` + `UNAOS_KEPLER_REPOINT` | `:: kdisp: repoint pre 6101E0=` / `repoint wrote=` / `repoint restored rb=` (`kepler_display.rs`, `repoint_surface`) — **code: restored 2026-09-15 (SHUTRESTORE) behind `nvidia-kepler-repoint`, default OFF** | s15 boot 2, 2026-07-23: `repoint wrote=00016000 rb=00000200` … `verdict rb-stuck=no` | 0x6101E0 is a read-only armed-state readout; the write never took. Boot 1 of the same sitting never reached the code at all (`takeover-abort no-match` — the refuted EVO-mirror head-match gate sat upstream) | KD5 | **shut-out; code RESTORED** |
| KD7 `latch` — EVO assembly (0x640460) + UPDATE (0x640080) | `UNAOS_KEPLER_TAKEOVER` + `UNAOS_KEPLER_LATCH_ARM` | `:: kdisp: latch pre asm=` (`kepler_display.rs:285`, n=1) for the pre-state, plus `:: kdisp: latch verdict asm-stuck=` / `armed-followed=` and the `pm-step` dumps (`kepler_display.rs`, `latch_arm_update`) — **code: the arm + UPDATE writes restored 2026-09-15 (SHUTRESTORE) behind `nvidia-kepler-latcharm`, default OFF; `update_reg` is live again under that knob** | s28, 2026-07-25: `gop-overlap=YES-RESULT-VOID surf2=01600000+01C20000 gop=00020000+01C20000`; `armed=00000200 shadow=00000200` at t=1 and t=5, i.e. `0x200 << 8` = the GOP framebuffer, unmoved | the scratch surface at VRAM `0x1600000` sat **inside** the firmware's own framebuffer, so s17's "first UnaOS pixels" were direct painting into the GOP FB, not a latch. Peter watched the graphic appear *before* `pm-step fill done` — a full latch cycle before the latch. Arming a Kepler head plausibly needs a core-channel **method**, not a bare MMIO poke, and no pushbuffer existed | KD5; a working pushbuffer (→ §2 KF-wall) | **shut-out** |
| KD8 `mirror-sp` — EVO core-channel method-mirror decode | `UNAOS_KEPLER_TAKEOVER`, plus the hard-coded `run_recon` at `kepler_display.rs:302` | `:: kdisp: mirror-sp cand off=` (`kepler_display.rs:339`, n=1) | s25, 2026-07-25: `0x46C = 01004000` → `SET_STORAGE bit24 LAYOUT=1 = PITCH (LINEAR)`, `pitch = 0x4000 = 16384 B/row`; `0x468 = 07080B40` = h1800 w2880 | — | KD7's window | **proven** — it retired the whole block-linear campaign (s19–s24) in one read |
| KD9 `fb-draw` — full-panel paint at the GOP origin | `UNAOS_KEPLER_TAKEOVER` | `:: kdisp: fb-draw cover=` (`kepler_display.rs:424`, n=1) | s29, 2026-07-25: `cover=exact`; Peter's photo "edge to edge, everything predicted, nothing missing" | — | KD8 (the pitch), KD2 (the base) | **proven** |
| KD10 `fbcon-vs-hw` — console stride reconcile | `UNAOS_KEPLER_TAKEOVER` | `:: kdisp: fbcon-vs-hw row_bytes=` (`kepler_display.rs:264`, n=1) | s30, 2026-07-25: `row_bytes=16384 hw_pitch=16384 match=true` | — | KD9 | **proven** — it **refuted** the standing "fbcon is mis-strided" hypothesis |
| KD11 `fbcon-probe` — 8-row glyph-block visibility test | `UNAOS_KEPLER_TAKEOVER` | `:: kdisp: fbcon-probe drawn rows=8` (`kepler_display.rs:490`, n=1) | s30: serial says `drawn rows=8`; Peter: "NO graphic visible — only the main calibration draw" | the probe was **under-sized**: three 8×8-px blocks at 220 ppi are ~0.7 mm dots laid over the calibration colour bands. Recorded at the sitting as INCONCLUSIVE, not as a measured null | KD9 | **open** (inconclusive, superseded by KD12) |
| KD12 `console-repaint` — kernel console on the panel | `UNAOS_KEPLER_TAKEOVER` | `:: kdisp: console-repaint rows=` (`kepler_display.rs:501`, n=1) | s33 boot 2, 2026-07-25: `console-repaint rows=4` with `fbcon: glyphs-active base=90020000 pitch=16384 cell=48x48`; Peter: the console "prints text very well" | — | KD10 | **proven** |
| KD13 `wcx_activate` — compositor takes the seam | `UNAOS_WC` | `:: kdisp: inner phase wcx_activate` (`kepler_display.rs:519`, n=1) | s40, 2026-07-26: `[wc-x] console-window win=1 panel=2880x1800` … `[wc-x] present win=2 rows=1104..1630 ok=true` | s39 failed first: `[wc-x] activate DECLINE reason=fb-not-ready` — `video::WRITER` was seeded at main.rs step 3, **after** the Kepler takeover where activation runs. Fixed by seeding WRITER beside `fbcon::init` (5701b9a8); the rung passed on the next boot unchanged | KD12 | **proven** |

**What would change the verdict**

- **KD3** — re-run the per-head decode with KD4's HEAD_STAT as the bracket and the per-head stride
  re-derived for the 917D class. The rung failed because it had no control read, not because the
  heads are dead; KD4 proves head 0 scans.
- **KD6** — nothing re-opens a read-only register *as a pointer*. The rung's code was gone, which
  R19 forbids; it is **restored** (SHUTRESTORE, 2026-09-15) behind `UNAOS_KEPLER_REPOINT`, so a
  later arc can re-read 0x6101E0 as the armed-state *witness* it turned out to be, with the one
  piece of code that ever wrote it and put it back present again.
- **KD7** — the condition to change is the mechanism, not the address: arm the head through a
  core-channel pushbuffer method with the scratch surface relocated clear of the GOP window
  (s28 named `0x4000000` as clear of both BAR1's 256 MB and the allocator's 32 MB floor). That
  needs §2's channel to run, so KD7 is **blocked on the FIFO wall, not refuted by it**.
- **KD11** — draw at human scale. KD12 already did, and passed; KD11 stays on the books only so
  its "invisible" reading is never cited as a mapping refutation.

---

## 2. Kepler FIFO / PBDMA ladder — `kepler.rs`

Knob: `UNAOS_KEPLER_FIFO` (`nvidia-kepler-fifo`), on top of `UNAOS_KEPLER`. This is the ladder that
has never passed: the channel is configured, the runlist is read, and PFIFO strips VALID/POLL. Ten
eliminations across sittings #8–#43; the wall has never moved a byte.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| KF1 `pbdma-count` | `UNAOS_KEPLER_FIFO` | `:: kepler: pbdma-count` (`kepler.rs:1516`, n=1) | s5/s6, 2026-07-22: `pbdma-count 3` | — | — | **proven** (a structural datum; "3 vs an expected 1 on GK107" is still undecoded) |
| KF2 `pbdma-eng-mask` | `UNAOS_KEPLER_FIFO` | `:: kepler: pbdma-eng-mask set ::` (`kepler.rs:1522`, n=1) | s6: masks `pbdma0=0x01 pbdma1=0x6E pbdma2=0x10`, writes took | — | KF1 | **proven** |
| KF3 `inst-raw` — instance-block field order | `UNAOS_KEPLER_FIFO` | `:: kepler: inst-raw 08=` (`kepler.rs:1594`, n=1) | s7, 2026-07-22: `inst-raw 4C=0x00090000` (was `0x01FF0000`; `log2(512)=9` took) | the ORDER defect was **real and is fixed on silicon** — and it is **refuted as the bind wall**: post-fix, `playlist_rd=0x2013 len=0x100001` yet all three PBDMAs still `ch=0 ACTIVE=0`, `gp_get=0`, fence-timeout | KF2 | **shut-out as the wall; proven as a fix** |
| KF4 `sched-status` — CHAN_TABLE_ERROR / SCHED_STATUS read | `UNAOS_KEPLER_FIFO` | `:: kepler: sched-status post-init err=` (`kepler.rs:2453`, n=1); also `post-restore` (`:2403`) and `post-submit` (`:3391`) | s9, 2026-07-22: `pre-init err=0 stat=0` → `post-init err=0x00000002` → `post-submit err=0x00000002 stat=0x00000005` | — | KF3 | **proven** (the instrument works; the chip names its own refusal) |
| KF5 `WITNESS STRIPPED` — the wall itself | `UNAOS_KEPLER_FIFO` | `:: kepler: PFIFO_CHAN[1] pre-submit:` (`kepler.rs:2465`, n=1) · `:: kepler: WITNESS STRIPPED. Restoring inst_off+0x0C ::` (`kepler.rs:2482`, n=1) · `:: kepler: witness-rematch end err=` (`kepler.rs:3514`, n=1) | s43 (GR25 Boot A): `witness-rematch end err=00000002 stat=00000005 valid=00002000` — byte-identical to the s25 baseline, **tenth** confirmation | not a rung that fails; the rung that **measures** every other rung's failure | — | **open** — the standing wall |
| KF6 `USERD_SNOOP` write (candidate A) | `UNAOS_KEPLER_FIFO` + `UNAOS_KEPLER_USERD_SNOOP` | `:: kepler: USERD_SNOOP (0x2a1c) orig=` and the witness-conditional `Restoring USERD_SNOOP=` (`kepler.rs`) — **code: restored 2026-09-15 (SHUTRESTORE) behind `nvidia-kepler-userdsnoop`, default OFF** | s10, 2026-07-22: `USERD_SNOOP orig=0` → write 1 → witness FAILED, snoop restored, `err=2 stat=5`, discriminators 0, RAMFC untouched | writes to `0x2a1c` read back as zero on GK107. The sitting refused to call it "config writes don't stick" — `SUBFIFO_ENG_MASK` and `PLAYLIST_WR/LEN` demonstrably stick; this register is either absent on this part, write-gated, or not a boolean | KF4 | **shut-out; code RESTORED** |
| KF7 `USERD_HI` bit31 | `UNAOS_KEPLER_FIFO` | folded into KF5's rewrite; the bit-31 arm has no surviving token | s11, 2026-07-22: witness FAILED, `err=2` unchanged, clean evidenced restore. New precision: `inst-raw 0C=80000000` — **the bit-31 write PERSISTS in instance memory** | the strip has always been on the PFIFO_CHAN **MMIO** word, never on instance bytes. BAR1 readback proves BAR1 self-coherence, not scheduler-side visibility | KF4 | **shut-out** — and it opened the live question KF8 answered |
| KF8 `PFIFO_FLUSH` between inst writes and validate | `UNAOS_KEPLER_FIFO` + `UNAOS_KEPLER_PFIFO_FLUSH` | `:: kepler: flush-executed 0x70000 pre=` and `PFLUSH (0x70000) ABSENT/POISON pre=` (`kepler.rs`) — **code: restored 2026-09-15 (SHUTRESTORE) behind `nvidia-kepler-pfifoflush`, default OFF** | s12, 2026-07-23: `flush-executed 0x70000 pre=00000000 post=00000000 iters=1` → `WITNESS FAILED - bits stripped` | engine-side stale-view-of-BAR1-writes **in its flushable form** is refuted. A non-flushable coherence gap is not | KF7 | **shut-out; code RESTORED** |
| KF9 `CTRL_ADDR TARGET` ×12 | `UNAOS_KEPLER_FIFO` + `UNAOS_KEPLER_CTRL_ADDR` | `:: kepler: ctrladdr pbdma{} pre=` / `try target=` / `RO?` / `restored rb=` (`kepler.rs`) — **code: the audit restored 2026-09-15 (SHUTRESTORE) behind `nvidia-kepler-ctrladdr`, default OFF; the per-target channel-bringup re-run is NOT restored (rmbp-queue `SHUTRESTORE-CTRLADDR-BRINGUP`)** | s13, 2026-07-23: every TARGET value 0..3 on every PBDMA `wrote=rb`, and the witness ladder never latched once — `WITNESS FAILED` on all 12 steps | all three PBDMAs read `pre=00000000 hi=00000000` at entry, one PBDMA at a time, clean evidenced restores between steps | KF4 | **shut-out; code RESTORED** |
| KF10 `poll-control valid-only` | `UNAOS_KEPLER_FIFO` | `:: kepler: poll-control valid-only chan=` (`kepler.rs:2531`, n=1) | s37, 2026-07-26: `poll-control valid-only chan=00002000 err=00000002 stat=00000000` — VALID written **without** POLL_ENABLE and the refusal is byte-identical | **the chip's own error name is a red herring.** `err=2` is documented NO_POLL ("validated a channel with POLL_ENABLE, but poll area is disabled") — and POLL_ENABLE was never the subject of the complaint. Twenty-eight sittings honoured a reason name that does not describe its own precondition | KF5 | **proven** (it settles the name) — it **shuts out the poll-area lead** (pulls 11/12) permanently |
| KF11 `pgraph-pulse` — PMC_ENABLE bit 12 | `UNAOS_KEPLER_FIFO` | `:: kepler: pgraph-pulse pre=` (`kepler.rs:1974`, n=1) | s22, 2026-07-25: `pre=E011216D → wrote=E011316D rb=E011316D` (bit 12 stuck); s23: witness rematch reproduces the strip **exactly** | s21 found PGRAPH **powered off at PMC** and read the whole falcon block as `0xBADF1200`. Enabling it changed the error class to `BADF1000` + real zeros but did **not** move the wall: **pgraph power-gating is refuted as the fence wall** (refutation #7) | KF5 | **shut-out as the wall; proven as a prerequisite** — every later falcon rung needs bit 12 set |
| KF12 `fal-base` — where the GR falcons live | `UNAOS_KEPLER_FIFO` | `:: kepler: fal-base b=` (`kepler.rs:2019`, n=1) | s26, 2026-07-25: `fal-base b=409000 verdict cpuctl=00000010 imemc=00000000 dmemc=00000000`, and the same at `0x41A000` | the spec's `0x400180`/`0x4001C0` (falcon_microcode_spec §2) read `BADF1000` on **every** access including control readbacks at s24/s25 — the nonexistent-PRI-register signature, not a gate. Those addresses are **shut out for GK107**; FECS 0x409000 / GPCCS 0x41A000 are the real ones | KF11 | **proven** |
| KF13 `fal-port` — IMEM/DMEM sentinel probe | `UNAOS_KEPLER_FIFO` | `:: kepler: fal-port b=` (`kepler.rs:2024`, n=4) | s27, 2026-07-25: all sixteen sentinels returned exactly (`DEADBEEF/CAFEF00D/12345678/A5A55A5A`, imem **and** dmem, FECS **and** GPCCS) | s24 had reported the ports "still gated" (`BADF1000` ×8 with PMC bit 12 set) — **at the wrong base**. The gate was an address error, and KF12 dissolved it | KF12 | **proven** — K-GPU-4 milestone 1 |
| KF14 `ucode` — first UnaOS code on GPU silicon | `UNAOS_KEPLER_FIFO` | `:: kepler: ucode EXECUTED img=` (`kepler.rs:2166`, n=1) | s29, 2026-07-25: `ucode EXECUTED img=A mailbox0=F00DFACE`, `ucode-post off=040 val=F00DFACE SENTINEL` | s28 aborted at exactly the same code: `cpuctl 00000010 → 00000012`, mailbox never left the `A5A50000` seed. The post-sweep named the blocker in the same boot — **`DMACTL (base+0x10C) = 0x00000001`, REQUIRE_CTX SET**. One masked write cleared it and the image ran on the first attempt | KF13 | **proven** — K-GPU-4 milestone 2. The indexed IO scheme `(X & 0xffc) << 6` is settled empirically; image B (flat ports) never needed to run |
| KF15 `hb` — bounded heartbeat, engine kept RUNNING | `UNAOS_KEPLER_FIFO` | `:: kepler: hb pre-witness mb1=` (`kepler.rs:2324`, n=1) | s30, 2026-07-25: `mb1 0x4 → 0x5750 → 0x5AA5 → 0x34328`, `cpuctl=00000000` throughout, and the strip signature **byte-identical** | **refutation #8, the cleanest: the wall is not engine liveness.** PFIFO stripped the channel while FECS was demonstrably alive and executing. s33 boot 2 supplied the other side — a **halted** FECS gives the same wall | KF14 | **shut-out as the wall; proven as a capability** |
| KF16 `ucode-echo` — host↔FECS command loop | `UNAOS_KEPLER_FIFO` | `:: kepler: ucode-echo SUCCESS h2h3=` (`kepler.rs:2270`, n=1) | s37, 2026-07-26: `ucode-echo host-cmd CC_SCRATCH[0]=00000001` → `host-ack CC_SCRATCH[1]=00000001 iters=0` → `SUCCESS img=A` | image **A** (derived indexed ports `I[0x20000]`/`I[0x20100]`) acked on the first poll; image B never ran. The proposal had shipped **host** offsets 0x800/0x804 as falcon port indices — the A/B fallback caught it in one boot | KF14 | **proven** |
| KF17 `ctx-echo` / `FENCE ladder` | `UNAOS_KEPLER_FIFO` | `:: kepler: ctx-echo h2h3=` (`kepler.rs:2259`, n=1) · `:: kepler: FENCE ladder VERDICT` (`kepler.rs:2924`, n=1) | s41/s43: `ctx-echo img=A ack=1 mb0=1 phase=4`; s43: `FENCE ladder VERDICT MAPPED` | the ladder proved the `(off & 0xffc) << 6` port rule **reaches host 0xB00 from inside the falcon**, so the port derivation was never the fault — **ENGINE_STATUS is simply not falcon-writable**. That extends refutation 7 to the falcon side. The `err=2` verdict itself stays **VOID** (the treatment was never applied) | KF16 | **open** |
| KF18 `bind` — host writes CHAN_CUR / CHAN_NEXT | `UNAOS_KEPLER_FIFO` | `:: kepler: bind CHAN_CUR=` (`kepler.rs:3287`, n=1) · `:: kepler: bind-post ENGINE_STATUS=` (`kepler.rs:3297`, n=1) | s35, 2026-07-25: `bind CHAN_CUR=00002000` and `bind CHAN_NEXT=00002000` (both writes TOOK, no fault, no poison) → `bind-post ENGINE_STATUS=00000000` — **CHAN_VALID NOT asserted** | a **bare MMIO bind** does not build CTXCTL state. Per the study, the FECS context microcode itself must run to accept a context. Nothing about the register surface is refuted — it is host-writable and holds a channel id | KF12, KF11 | **shut-out under "no FECS ctx ucode running"** |
| KF19 `witness post-bind` — strip test with a populated CHAN_CUR | `UNAOS_KEPLER_FIFO` | `:: kepler: witness post-bind PFIFO_CHAN[1]=` (`kepler.rs:3307`, n=1) | s36, 2026-07-26: `witness pre-rewrite PFIFO_CHAN[1]=00002000` → `witness post-bind PFIFO_CHAN[1]=00002000` — **not** `C0002000`; the strip persists with CHAN_CUR bound (tenth elimination) | s35's first attempt at this observation was **VOID**: the amendment aimed the post-bind witness at `inst_off+0x0C`, plain VRAM, where a readback trivially returns what was written. The historic strip lives in the PFIFO channel-table **register** at `0x800008` | KF18 | **shut-out** — and its own first run is a logged amendment error, not a datum |
| KF20 `recon` — the CTXCTL host-interface offsets | `UNAOS_KEPLER_FIFO` | `:: kepler: recon-pre cpuctl=` (`kepler.rs:3268`, n=1) | s34, 2026-07-25: `recon CC_SCRATCH[1] (0x804)=00000000`, `CHAN_CUR (0xB00)=00000000`, `CHAN_NEXT (0xB04)=00000000`, `ENGINE_STATUS (0xC00)=00000000`, `ENGINE_TRIGGER (0xC08)=00000000`, control bracket `cpuctl=00000010` both ends | **⚠ silicon law, s31:** a bad `0x409xxx` read **poisons the whole FECS unit for the rest of the boot**. s32 proved it by its own control frame (`recon-pre cpuctl=00000000` real, `recon-post cpuctl=BADF1000`, same register microseconds apart). Exactly ONE offset faults — `0x409504` WRCMD_CMD — and s34 convicted it by elimination once the block was relocated after `hb final` | KF12 | **proven** (six of seven offsets exist and read zero at rest) |
| KF21 `terminal-poke` 0x409504 | `UNAOS_KEPLER_FIFO` | `:: kepler: terminal-poke 0x409504 wr=0` (`kepler.rs:3836`, n=1) | s41, 2026-07-26: `terminal-poke 0x409504 wr=0` → `[NVIDIA] Initialization complete` and the boot sailed on | **the poison register is writable without consequence to the boot.** Reading it first is what poisons; writing it last is harmless. The un-wedge experiment ("does a PRING clear recover the unit?") remains **UNEXERCISED** — nothing has ever wedged on a boot that went looking | KF20 | **proven** |
| KF22 `ucode-poke` | `UNAOS_KEPLER_FIFO` | `:: kepler: ucode-poke SUCCESS img=POKE` (`kepler.rs:3704`, n=1) · `ucode-poke heartbeat` (`kepler.rs:3649`, n=1) | s43 era; the POKE ucode deliberately poisons the unit, which is why the CE ladder is called **above** it | any verdict collected after the POKE ucode or after the terminal `0x409504` write is **void** | KF16 | **open** |
| KF23 `runlist-scan` — the sibling-runlist sweep | `UNAOS_KEPLER_FIFO` | `:: kepler: runlist-scan verdict occupied_mask=` (`kepler.rs:3487`, n=1) | s43: `runlist-scan i=1 base_off=2278 base=BAD0011F POISON`, `i=2 base_off=2280 base=00002013 len=00100003 OCCUPIED`, `verdict occupied_mask=4 alias_i2_base=match alias_i2_len=match` | read strictly this **refutes the array-stride assumption at `0x2270`** — element 1 of that array does not exist. It did not find "no CE runlist"; it found "not there". And it ran **after** our own submit, so the one occupied slot it could see was necessarily ours | KF5 | **open** — superseded pre-submit by CE-R3 (§3) |
| KF24 `bar1-identity` | `UNAOS_KEPLER_CE` | `:: kepler: bar1-identity VERDICT` (`kepler.rs:3791`, n=1) | s43 (GR25 Boot A): `bar1-identity scratch_off=02015000 magic=CEA50BA5 bar1_rb=CEA50BA5 … pramin_read=CEA50BA5 win_restored=Y` → `VERDICT IDENTITY — … BAR1 offsets ARE physical VRAM addresses on this part; every FIFO pointer in this driver is fine and the paged-BAR1 root cause is CLOSED as a false alarm` | — | KF12 | **proven** — the highest-risk open question in the study, closed as a **false alarm**, which retroactively **keeps** the fence arc's ten eliminations valid |
| KF25 `mirror-hdr` / `beacon` / `latch-delta` — the 0x640000 window | `UNAOS_KEPLER_FIFO` | `:: kepler: mirror-hdr pass0 off=` (`kepler.rs:1756`, n=1) · `:: kepler: beacon none-seen ::` (`kepler.rs:1948`, n=1) · `:: kepler: latch-delta none ::` (`kepler.rs:1765`, n=1) | s20, 2026-07-24: **triple-refuted** — beacons none-seen twice, `latch-delta none`, and the pre-dump was all-zero this boot vs 158 non-zero rows at s19 | contents are **boot-dependent residue**, not live state we can steer; the window is engine-private memory, not a channel mirror. Window parked | KF5 | **shut-out** |
| KF26 `DISCRIMINATOR pbdma` | `UNAOS_KEPLER_FIFO` | `:: kepler: DISCRIMINATOR pbdma` (`kepler.rs:3509`, n=1) | s43: `DISCRIMINATOR pbdma{0,1,2} ch=00000000 (CHID=0 ACTIVE=0)` — unchanged since s6 | raw, bit31-valid and bit0-valid runlist entry encodings are **all refuted as sufficient** (s8); the channel is never scheduled onto any PBDMA | KF5 | **open** — the downstream symptom of the wall |

**Where the FIFO ladder actually stands.** The complete elimination, ten sittings of it: runlist
encodings, USERD variants, flushes, CTRL_ADDR, a powered engine, a reset-pulsed engine, a **live
running** engine, a **halted** engine, and a host-populated CHAN_CUR/CHAN_NEXT — the strip
signature never moved once. Meanwhile every constructive fact points one way: the submit path
provably works (`playlist_rd` echoes our runlist), the falcon executes our code, the CTXCTL
register surface is mapped and writable, and `ENGINE_STATUS.CHAN_VALID` — the bit PFIFO's
validation plausibly keys on — is set by nothing we can reach from the host. **The remaining actor
is the FECS context-switch microcode itself.** K-GPU-4 pivoted from probing the wall to building
the gatekeeper.

**What would change the verdict**

- **KF6 / KF8 / KF9** — each was refuted *as a sufficient cause of the strip*, under a host that
  had never run FECS ucode. Every one of them is worth re-flying **as the first step of a boot
  where a FECS context machine is being brought up** — that is R19's exact scenario. Their code is
  back (§7): `UNAOS_KEPLER_USERD_SNOOP=1`, `UNAOS_KEPLER_PFIFO_FLUSH=1`,
  `UNAOS_KEPLER_CTRL_ADDR=1`, each on top of `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1`. Nothing is
  owed here but the flight.
- **KF7** — the open question it created is still open: **does the engine see our instance bytes
  at validate time?** BAR1 self-coherence is proven (KF24); scheduler-side visibility is not.
- **KF18 / KF19** — re-run the bind **with FECS ctx microcode resident and running**, not as a
  bare poke. That is the one condition never yet varied.
- **KF21** — the un-wedge experiment needs a boot where the poison **deliberately** fires, then a
  PRING observe/clear and a `cpuctl` re-read in the same boot.
- **KF25** — the window is parked, not dead; a boot that correlates it against a *working* channel
  would be reading a different machine than s19/s20 read.
- **KF11 / KF12 / KF13 / KF14** — these are the register's clearest R19 lesson already paid for:
  four rungs recorded as failures (`BADF1200` everywhere, ports gated, ucode won't start) whose
  conditions were, in order, *PMC bit 12 clear*, *the wrong base*, *the wrong base again*, and
  *DMACTL REQUIRE_CTX set*. Not one of them was a property of the silicon.

---

## 3. Kepler copy-engine ladder (CE-LADDER) — `kepler_ce.rs`

Knob: `UNAOS_KEPLER_CE` (`nvidia-kepler-ce`); implies `nvidia-kepler`, **independent of**
`UNAOS_KEPLER_FIFO`. Called from `kepler::init` **above** the fifo leg, which puts it upstream of
every FECS access (the POKE ucode poisons the unit and the terminal `0x409504` write closes the
boot) and lets R3 read the runlist array **pre-submit**, where a populated slot is unambiguously
the firmware's. Every rung is bracketed by a control read of `NV_PMC_BOOT_0`, so "the space is
dead" is never reported as "the space is empty".

**The whole ladder is `never-run`.** It is armed and unflown as of sitting #43 / GR26.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| CE-R1 `ce-ptop` — PTOP device-info table at 0x022700 | `UNAOS_KEPLER_CE` | `:: kepler: ce-ptop VERDICT` (`kepler_ce.rs:386`, n=1) | — never flown | — | KF24 (BAR1 identity) | **never-run** |
| CE-R2 `ce-probe` — are 0x104000/0x105000/0x106000 alive, and falcons? | `UNAOS_KEPLER_CE` | `:: kepler: ce-probe base=` (`kepler_ce.rs:496`, n=2) | — never flown | — | CE-R1 | **never-run** |
| CE-R2b `ce-r2` — the campaign's FIRST copy-engine WRITE | `UNAOS_KEPLER_CE` (no separate knob; gated at runtime on a **same-boot** FALCON-REST verdict) | `:: kepler: ce-r2 base=` (`kepler_ce.rs:610`, n=3) | — never flown; the expected first-flight outcome is `ce-r2 SKIPPED` and **nothing is written** | — | **CE-R2 naming a FALCON-REST base in the same boot** (`cpuctl=0x10` **and** `dmactl=0x01`, re-verified with a fresh read at write time) | **never-run** |
| CE-R3 `ce-rlscan` — the runlist array at 0x2280 + i*8, pre-submit | `UNAOS_KEPLER_CE` | `:: kepler: ce-rlscan VERDICT bit20=` (`kepler_ce.rs:776`, n=1) | — never flown | — | CE-R1 for the stride; **must run before the fifo submit** | **never-run** |
| CE-R5 `ce-inst` — walk the firmware's instance block through PRAMIN | `UNAOS_KEPLER_CE` | `:: kepler: ce-inst verdict regs=` (`kepler_ce.rs:1018`, n=1) | — never flown | — | KF24 (the PRAMIN window base is metal-proven by Boot A) | **never-run** — this is the clean-room-legal RAMFC route; it is how the standing UNAUDITED-constants debt gets discharged |
| CE rollup | `UNAOS_KEPLER_CE` | `:: kepler: CE-LADDER end r1_ptop=` (`kepler_ce.rs:1083`, n=1) | — never flown | — | all of the above | **never-run** — prints all five outcomes on one line, so a silent ladder is distinguishable from a quiet pass |

**Deliberately not implemented** (consumers of answers this ladder does not yet have): draft R6 (a
CE channel) and draft R7 (a genuine VRAM→VRAM copy). They need a CE runlist id from R1/R3, an
audited instance-block layout from R5, or the CE falcon's internal datapath map.

**What would change the verdict** — nothing here has a verdict yet. The next flight's falsification
story is already written: with `UNAOS_KEPLER_CE=1`, if any base reports `FALCON-REST` then
`ce-r2 base=… VERDICT` must read `ARMED` or `REJECTED` — decisive either way; if no base is a
falcon, `ce-r2 SKIPPED` prints and the boot is as read-only as the recon-only ladder. A witness
reporting `ARMED` without a `landed=Y` transition, or on a `SKIPPED` boot, would be a defect.
Confirm `nvidia-kepler-ce` appears in the `⚡ kernel features:` banner **and** in the artifact
(s42's INSTGUI lesson: a knob added only to `arroyo` is invisible to media).

---

## 4. gen7 — Ivy Bridge GT2 render-engine ladder — `gen7.rs`

Knobs: `UNAOS_IVB3D` (`gen7`, implies `intel-ivb`) for R1-R7; `UNAOS_IVB3D_R8` (`gen7r8`, implies `gen7`)
for R8. Design of record `gen7.md`.

**This is the register's headline.** R2, R3 and R5 are all recorded as failures. R6 and R7 — which
`gen7.md` still calls *pending metal* and *held dark* — flew on **flight 4, 2026-08-28, both boots**
and **passed**. The condition that changed was one line of discipline: **keep the forcewake hold
across the arm** instead of releasing it inside the acquiring rung.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| R1 `recon` (read-only census) | `UNAOS_IVB3D` | `:: gen7: recon verdict=` (`gen7.rs:873`, n=2) · `:: gen7: gt blk=` (`gen7.rs:485`, n=2) | Boot D: `HYP_GTFIFOCTL (0x120008)` reads `0x0000003F`, stable — **the only structured register in the whole 25-register probe** | — | — | **proven** — it establishes that BAR0 reaches the `0x12xxxx` block, so a zero at `0x1300xx` is a statement about that register, not the mapping. Every rung since uses `GTFIFOCTL` as its delta control |
| R2 `wake` (INSTPM / RCS_WAKE Sync-Flush) | `UNAOS_IVB3D` | `:: gen7: r2 verdict=` (`gen7.rs:1194`, n=4) · verdict string `r2-unscorable-until-behavioural-witness` (`gen7.rs:1192`, n=3) · the battery reading `gt-still-dark`, kept verbatim in `battery=` (`gen7.rs:1188`, n=20) | Boot D: `r2 verdict=gt-still-dark trans_all=0/17 trans_untouched=0/14 struct=0 varies=0` | **three conditions, and all three are now known to be wrong or blind.** (1) `poll_ack=1 poll_iters=0` was *not* an ack — `0x22AC` read zero on the first look and `== 0` was the pass condition, so a power-gated window passed on iteration zero. (2) The Sync-Flush sequence (`IVB-V1P3 §1.1.10.9`) is a **VT-d workaround**, not a forcewake protocol — draining a command streamer is not powering one. (3) **The 17-register battery it was scored on is not a liveness witness on this part**: flight 4 read `battery_moved=0/17` through a *verified* 1 KiB engine DMA | R1 | **shut-out — and GEN7R2 CLOSED it as far as it can be closed, 2026-09-15.** The rung now prints `r2-unscorable-until-behavioural-witness` and carries the old reading in `battery=`, with `instrument_blind=1` and the flight-4 citation on the same line. The re-score the row asked for is NOT done here and the reason is structural: a ring arm needs a proven-unowned GGTT window (R4b) and a HELD acquire (R6), and R2 runs before R3 has returned a `GtWake` at all — so the behavioural experiment lives where the wake is held, and R6 (`r6-sentinel-hit`) and R7 (`r7-blit-verified`) ARE that experiment, already flown. ⚠ The blindness reaches the NEGATIVE arm only: `gt-woke` is still a verdict |
| R3 `forcewake` (request/ack handshake) | `UNAOS_IVB3D` | `:: gen7: r3 verdict=` (`gen7.rs:2237`, n=3) · classifications `fw-ack-transition` (`gen7.rs:1649`, n=2), `fw-req-decodes-no-ack` (`gen7.rs:1650`, n=3), `fw-no-decode` (`gen7.rs:1651`, n=3) · `gt-live-already` (`gen7.rs:1395`, n=9) | flight 4: R3's verdict was **`gt-live-already` by=none (`pre_live=1`)**, i.e. no ack was ever decoded — and R6/R7 then executed real engine work under the `mt` hold | Intel **never published** the Gen7 GT power/forcewake register block; the full sixteen-volume IVB PRM set was searched and the only hits are register-less prose. Both candidates are pinned on later silicon: `mt` = `0x0A188`/`0x130044[15:0]` `[BDW-ONLY]`, `renfw` = `0x1300B0`/`0x1300B4` `[CHV-ONLY]`. All four dwords read `0x00000000`, so `restored=` is a `0 == 0` compare — the rung prints `restore_evidence=blind` rather than claim it. Flight 4 confirmed it end to end: `fw_evidence=blind`, `classification=fw-no-decode` on R7, **while the BCS demonstrably executed** | R1 | **shut-out as an evidence channel; the hold itself is proven by behaviour.** **R8 must not gate on a forcewake ack decode** — the hold-across-the-arm discipline plus behavioural witnesses is the only currency this part honours |
| R3's `preheld` guard | `UNAOS_IVB3D` | retired with `Acq::SkippedPreheld`; see `gen7.md` §2.6 | it **skipped the only [PINNED]-adjacent candidate the ladder has** and the boot was spent: `0x0A188` read `0x00010000` at candidate entry while reading `0x00000000` in the frame census milliseconds earlier | the defect was the **inference**, not the threshold: a non-zero read cannot distinguish "another owner holds forcewake" from "this offset is not a forcewake register here" (on Cherryview the same offset is `SCRATCH1`) from a gated-window decode artefact — and `0x00010000` is the **mask-form release pattern the rung itself writes**, which a healthy MT register cannot read back at all | R3 | **shut-out — retired, correctly** |
| R4 / R4b `claim` (GGTT PTE round-trip) | `UNAOS_IVB3D` | `:: gen7: r4 ggtt-claim first=` (`gen7.rs:2507`, n=1) | Boot Ab: **the GGTT PTE round-trip is proven on metal.** "Unowned" has two proven shapes — every pre-image zero, or every pre-image the firmware **scratch-fill** (one identical valid PTE whose frame is the BDSM stolen-memory base read from the host bridge *this boot*, uniform across window, both neighbours and six distant probes) | — | R3 (a *reachable* wake for the census; a **confirmed** wake for the write) | **proven** — the GT **fabric** answers even while the engine block refuses. That split is what the rest of the ladder is about |
| R5 `execute` (RCS ring, `MI_STORE_DATA_IMM`) | `UNAOS_IVB3D` | `:: gen7: r5 verdict=` (`gen7.rs:3679`, n=16) | flight 4, both boots: `r5 verdict=enable-void … ctl_wrote=00000001 ctl_readback=00000000` | **no forcewake hold was in force.** R3 releases its acquire *inside its own rung*, by design — R3's job was to measure the acquire, not to keep it — so by the time R5 wrote `RING_CTL`, nothing was held. R5's own `next=` named it: `STOP-RING_CTL-enable-did-not-latch-likely-forcewake-released-R6-must-hold-forcewake-and-rearm` | R4 | **shut-out — and R6 re-opened it by changing exactly that one condition** |
| R6 `rearm` (the same ring, under a **held** wake) | `UNAOS_IVB3D` | `:: gen7: r6 verdict=` (`gen7.rs:4537`, n=13) · `r6-sentinel-hit` (`gen7.rs:4339`, n=10) · the decisive negative `r6-enable-void-under-every-hold` (`gen7.rs:4353`, n=3) | **flight 4, both boots: `r6 verdict=r6-sentinel-hit by=mt … attempts=1/3`** — head 0→0x20, sentinel `5EED1234` landed | — | **R3's acquire kept open across the whole arm / submit / drain / disable / restore**; R4's proven-unowned window | **proven** — `gen7.md` §2's "pending metal" is superseded |
| R6's TLB-invalidation sub-rung (`GFX_FLSH_CNTL` 0x101008) | `UNAOS_IVB3D` | `:: gen7: r6 tlb verdict=` (`gen7.rs:4461`, n=1) · `tlb-flush-write-silent` (`gen7.rs:4403`, n=3) | flight 4: `tlb=tlb-flush-write-silent` + `reclaim=leaked pages=3 bytes=12288 reason=no-invalidation-evidence` | the register read `0` before **and** after the write — §2.6's "a self-clearing flush register and a non-decoding offset are indistinguishable here" case, exactly. The address and the write-any-value semantic are `[PINNED]` (IVB PRM Vol1 Part3 §1.2.21 p.191); the PRM gives **no** name, format or read decode. The reclaim invariant then correctly refuses the free: *an unpinned register is never the sole reason a page goes back to the heap* | R6 | **shut-out — and GEN7TLB DECIDED IT, 2026-09-15: HOLD.** `reclaim=freed` is struck from the falsifier. An alternative witness cannot be pinned (the PRM names one mechanism and gives it no read decode), and the tempting substitute — free under R7's idle proof — was refused: `engine_quiesced` is a statement about the COMMAND STREAMER, a GGTT translation is cached in the SYSTEM AGENT, and substituting one for the other is LAWS §5's error shape. The bounded retention is documented-accepted (`gen7.md` §2.6) and is now VISIBLE AS A DECISION: `reclaim=` is three-valued — `freed` / **`held`** (quiesced, every write restored, no invalidation witness — the healthy armed state) / `leaked` (SAFETY refusal only). `reason=` unchanged so flight-4 captures still compare. Cost: r5 (2) + r6 (2) + r7 (3) = **7 pages / 28 672 B per armed boot**, plus R8's 128 pages / 524 288 B when `gen7r8` is armed |
| R7 `blit` (BCS, `XY_SRC_COPY_BLT` 16×16×32bpp) | `UNAOS_IVB3D` | `:: gen7: r7 verdict=` (`gen7.rs:5604`, n=15) · `r7-blit-verified` (`gen7.rs:5377`, n=16) | **flight 4, both boots: `r7 verdict=r7-blit-verified … attempts=1/3 … best_dst_match=256/256`** at `[15744ms]` boot 1 (L1565) and `[12199ms]` boot 2 (L10467). `ctl_readback=00000001 head_post=00000040 … sentinel_post=0B75C0DE sentinel_hit=1 dst_match=256/256 dst_crc=D7994E3D src_crc=D7994E3D … iters=3 cyc=7396`. `ring_idle=1 drain_iters=0 drain_cyc=28 … settled=1` — `col=exec == col=drain = 256`, so the G1 STOP condition (drain below exec) is **excluded by measurement**, both boots | — | R6 (the held wake and the arm discipline); R4b (the proven-unowned window, extended by one slot) | **proven** — the pre-registered falsifier is satisfied field-for-field, and none of the four failure tripwires fired. `gen7.md` §2.7's "held dark" is superseded. The bytes-not-DWords pitch ruling is now **metal-proven** |
| R8 `fb_blit` (BCS, `XY_SRC_COPY_BLT` 64×64×32bpp at the panel's pitch) | `UNAOS_IVB3D_R8` (`gen7r8`, implies `gen7`) | `:: gen7: r8 verdict=` (`gen7.rs:6780`, n=16) · `r8-fb-blit-verified` (`gen7.rs:6567`, n=9) · `r8-fb-blit-verified-spill` (`gen7.rs:6568`, n=3) · `r8-gated-on-r7` (`gen7.rs:5826`, n=2) · `:: gen7: r8 begin` (`gen7.rs:5806`, n=1) | **never run** — built 2026-09-15 (GEN7NEXT), pending metal | — | **R7's verdict on THIS boot must be `r7-blit-verified`** (R19; enforced in code, the call is at the tail of `blit()` and takes R7's own local verdict) · R6's held-wake discipline · R4b's proven-unowned window, widened to `1 + 4 + dst_pages` slots | **pending metal.** R7 proved the BCS blits in the smallest possible case — 16×16, origin (0,0), 64-byte pitch, ONE destination page. R8 removes all four degeneracies at once: the panel's real pitch (7 680 B), a NON-ZERO origin (the top-right corner), and a multi-page GGTT surface on both the read and the write side. It adds the one witness R7 structurally could not have — `spill`, the dwords OUTSIDE the rectangle — which is what tells a correct blit from a blit that landed the right pixels in the wrong place, and which ANSWERS `gen7.md` §2.7's unresolved `X2/Y2` inclusive-or-exclusive residual by arithmetic (`spill ≈ pitch/4` = a pitch-unit error; `spill ≈ 64` = an inclusive X2). ⚠ It does **not** write the panel: §5 G4 proves the Kepler owns it and G7 is never-run, so a blit into the live scanout would prove nothing visible AND would be a write into another device's VRAM aperture — the destination is CPU-readable scratch CARRYING the panel's geometry, and the visible copy is deferred to the rung after the gmux switch persists. It does **not** gate on a forcewake ack decode, as R3's row requires |

**What would change the verdict**

- **R2** — ~~re-score it~~ **DONE as far as this rung can do it (GEN7R2, 2026-09-15).** The verdict
  is now `r2-unscorable-until-behavioural-witness`, the battery reading is kept verbatim in
  `battery=gt-still-dark`, and `instrument_blind=1` plus the flight-4 citation ride the same line.
  The behavioural re-score is NOT added to R2, and the reason is structural rather than a deferral:
  a ring arm needs a proven-unowned GGTT window (R4b) and a HELD acquire (R6's discipline), and R2
  runs before R3 has returned a `GtWake` at all — neither input exists at its call site. The
  experiment lives where the wake is held and **has already been run there**: `r6-sentinel-hit` and
  `r7-blit-verified`. What would change THIS row now is only a boot that prints the new line.
- **R3** — the ack pairing stays unpinned and probably unpinnable; Intel never published the block.
  What would change the verdict is a *third* candidate (`gtforceawake 0x130090` is coded and has
  never been reached, because `mt` succeeds first) or a positively-decoding ack found by
  observation. **Nothing downstream should wait for it.**
- **R5** — already re-opened by R6. Its `enable-void` is `failed under: no hold in force`, full
  stop; the identical write latches when a hold is kept.
- **R6 TLB** — ~~pin an alternative witness, or promote the leak~~ **DECIDED (GEN7TLB, 2026-09-15):
  HOLD.** No alternative witness is pinnable — the PRM names one mechanism (`0x101008`) and gives it
  no read decode. Freeing under R7's idle proof was considered and REFUSED: `engine_quiesced` is a
  statement about the command streamer and the transfer it was handed, a GGTT translation is cached
  in the system agent, and substituting one for the other is exactly LAWS §5's "the check measures a
  different question than the decision needs". The bounded retention is documented-accepted
  (`gen7.md` §2.6) and is now three-valued on the wire — `freed` / `held` (the healthy armed state)
  / `leaked` (safety refusal only) — so a decision and an alarm stop wearing the same word.
  `reason=` is unchanged, so flight-4 captures still compare.
- **R7** — nothing. It passed. What it handed R8 is in its own words:
  `r7 next=DONE-the-BCS-copies-pixels-under-a-held-wake-wire-bring_up_blt_ring-to-the-held-wake-and-fix-blitter_copy_rect-DW0-client-field`.
- **R8** — **built 2026-09-15, and only a boot can move it.** Its falsifier is pre-registered in
  `gen7.md` §2.8: with a wake held and R7 verified on the same boot, `ctl_readback==0x1`,
  `head_moved=1`, `rect_match=4096/4096`, `spill=0`, `sentinel_hit=1`, `settled=1`. Three named
  alternatives, each pointing at a different suspect: `r8-fb-blit-verified-spill` (the engine works,
  the geometry does not — read `spill` against `pitch/4` and against 64), `r8-fb-blit-partial` with
  `col=exec` short and `col=drain` full (we looked too early), and
  `r8-enable-void-under-every-hold` AFTER R7 latched on the same boot (the delta is R8's own ring
  programming, not the engine domain — do not re-open the forcewake question). Expected and NOT
  defects on a healthy armed boot: `battery_moved=0/17`, `fw_evidence=blind`, and
  `reclaim=held reason=no-invalidation-evidence`.
- ⚠ **The rung R8 does NOT attempt, named so it is not mistaken for an oversight:** a blit into the
  LIVE framebuffer. Its condition is §5's G7, not anything in this section.

---

## 5. GMUX / iGPU ladder — `igpu.rs`

Knobs: `UNAOS_IVB` (`intel-ivb,unaos_ivb`) for the probes; `UNAOS_GMUX_IGD` (`gmux_igd`) for the
mux switch. ⛔ `UNAOS_GMUX_IGD` is **single-use test media** — the knob is baked into the image and
nothing guards it across boots, so every boot from that stick switches the mux again.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| G1 `probe-complete` — iGPU pipes/planes census | `UNAOS_IVB` | `:: igpu: probe-complete ::` (`igpu.rs:699`, n=1) | s6 boot 1b, 2026-07-22, reconfirmed every boot since: Pipes A/B/C `CONF=0`; Planes A/B/C `CNTR=0 SURF=0 STRIDE=0 LINOFF=0 TILEOFF=0`; `DP_A=0x1C` | — | `UNAOS_IVB` publishing BAR0 | **proven** — every iGPU display plane is off |
| G2 `TEARDOWN HUNT TRACE` — four-point firmware-teardown hunt | `UNAOS_IVB` | `:: igpu: TEARDOWN HUNT TRACE ::` (`igpu.rs:459`, n=1) | s8 boot 1, 2026-07-22: **Point-0 is ALL-DEAD too** — pipes/planes/PP_STATUS/PP_CONTROL/DPLL_A read `0x00000000` at all four points, first-instruction-adjacent bootloader entry included; `DP_A=0x1C` constant | the "firmware tears iGPU scanout down during our bootloader window" theory is **dead**: panel power and the PLL were never on at any observable instant. The CF8-failed-read caveat was itself refuted — a failed read would have zeroed `DP_A` too | G1 | **shut-out (the teardown theory); proven (the census)** |
| G3 `PROTOCOL PROVEN` — gmux indexed handshake | `UNAOS_GMUX_IGD` | `:: igpu: PROTOCOL PROVEN (version plausible)` (`igpu.rs:499`, n=1) | s10 boot 1, 2026-07-22: **version 3.2.19** via the 32-bit indexed read; `MAX_BRIGHTNESS=0x3FF` as a second proof. Gate PASSED | s9's attempt **failed under the 3×8-bit read variant** — the version self-test returned implausible tuples and the gate correctly held, printing raw bytes only. s8 had already flagged the shape: `idx_SWITCH` and `idx_POWER` returned identical bytes twice, the signature of a missing ready-wait between index write and value read. **A read-width error, not a protocol absence** | G1 | **proven** |
| G4 `SW_DISPLAY` — who owns the panel | `UNAOS_GMUX_IGD` | `:: igpu: SW_DISPLAY` (`igpu.rs:509`, n=1) | s10: **`SW_DISPLAY=0x03 (DISCRETE)`, `SW_DDC=0x02 (DISCRETE)`, `DISC_POWER=0x03 (ON)`**, stable at Boot **and** Kernel | — | G3 | **proven** — the Kepler dGPU owns the panel at every observed instant. This **formally reversed** sitting #5's gmux/iGPU redirect and made "iGPU-all-dead" the **expected** state rather than a paradox |
| G5 `igpu-dpy rung=00 census` | `UNAOS_GMUX_IGD` | `:: igpu-dpy: rung=00 name=census` (`igpu.rs:1285`, n=1) | flight 5, 2026-08-28: **never reached** | G6 refused upstream of it | G6 | **never-run** |
| G6 pre-switch gate | `UNAOS_GMUX_IGD` | `:: igpu-dpy: pre-switch state DDC=` (`igpu.rs:1231`, n=1) · `pre-switch-not-accepted` (`igpu.rs:1254`, n=1) | flight 5: `:: igpu: [GMUX] REFUSED: pre-switch-not-accepted (status: 0x00000000) ::` → `:: igpu-dpy: LADDER highest=00/10 name=harness ok=0 pending=0 gmux=UNTOUCHED why=pre-switch-not-accepted elapsed_ms=1 ::` | the gate demands `DDC == GMUX_DDC_DIS` **and** `READ_DISPLAY == GMUX_DISPLAY_DIS` **and both** EXTERNAL registers ∈ {`GMUX_EXTERNAL_DIS`, `GMUX_EXTERNAL_KEPLER_OWNED` (0x21)}. At least one read on that boot fell outside. **The capture already contains the answer** — `igpu-dpy: pre-switch state` prints at `igpu.rs:1231`, *before* the gate returns at `:1254`, so the offending register is named on the wire and has never been read back | G3 | fixed-unflown — GMUX-1 read the datum (`docs/dev/evidence/rmbp-0915/GMUX-1-PRESWITCH.md`: SW_EXT=0x01 was 0x40, the write target); GMUX-2 scores 0x41 (`gmux_preswitch_decode`, igpu.rs tail) and restores nothing it did not read as state; flies next |
| G7 gmux switch to IGD | `UNAOS_GMUX_IGD` | `:: igpu: [GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD` (`igpu.rs:1441`, n=1) | **never reached on metal** | G6 | G6 | **never-run** — and the inherited claim that it "switches and restores on the same call stack" is **false**: it does not switch at all (flight 5 §3.1). Ledger A7's `GMUX_SWITCH_EXTERNAL=0x01` blocker is therefore a statement about a rung that has never run |
| G8 ladder rollup | `UNAOS_GMUX_IGD` | `:: igpu-dpy: LADDER highest=` (`igpu.rs:1529`, n=1) | flight 5: `highest=00/10` | — | all | **open** (the instrument works; it reports 0 of 10) |
| G9 iGPU BLT ring (console acceleration) | `UNAOS_IVB` | `:: igpu-blt: ring=absent why=no-active-surface` (`igpu.rs:892`, n=1) | flight 5 and every flight: `ring=absent why=no-active-surface — every iGPU display plane is off (gmux routes the panel elsewhere); CPU path carries the console` | it needs an **active iGPU display plane** to prove scanout extent, and G4 proves there is none while the Kepler owns the panel | G7 opening | **shut-out under "the Kepler owns the panel"** |

**What would change the verdict**

- **G6** — ~~one `awk` over the flight-5 capture~~ **READ, and FIXED; what would change the verdict
  now is one armed boot.** GMUX-1 (`docs/dev/evidence/rmbp-0915/GMUX-1-PRESWITCH.md`) read the datum
  the flight had already paid for: the single failing term of the four was `SW_EXT=0x01`, the read of
  `GMUX_SWITCH_EXTERNAL` (**0x40, the WRITE-TARGET port**) scored against the STATUS encodings, while
  every register that *reports* mux state — `DDC`=0x02, `READ_DISPLAY`=0x03, `READ_EXTERNAL`=0x21 —
  read its accepted value. A wrong read, ours. **GMUX-2 (this register's own next rung; ledger A7,
  `fixed-unflown`)** put the gate on the status ports only and stopped the unwind writing a value it
  never read as state. The next verdict is therefore scored at the glass, on these lines:
  1. `:: igpu-dpy: pre-switch state … SW_EXT=0x… SW_EXT_ST=0x… … gate=… ::` (`igpu.rs:1231`) — the
     census now prints **both** halves of the 0x40/0x41 pair and the verdict token. `gate=ACCEPT`
     means the three status reads were all in their accepted sets; `gate=REFUSE:<port>@<idx>` names
     which status port reports a state this rung was not written for; `gate=UNREADABLE:<port>@<idx>`
     says the gmux did not answer at all (0x00 / 0xFF / the 0xFFFFFFFF timeout sentinel) — that is
     **not** a mux-state finding, and it is the one outcome that sends this row back to G3.
  2. Then either `:: igpu-dpy: rung=00 name=census ok=1 …` (`igpu.rs:1285`) — G6 ACCEPTS and G5
     opens — or the same `pre-switch-not-accepted` (`igpu.rs:1254`; the `why` token is deliberately
     unchanged so old captures still compare) with `gate=` naming the port.
  A `gate=ACCEPT` whose `SW_EXT` is still `0x01` is the *expected* shape, not a contradiction: 0x40
  is printed and no longer gated, exactly as `SW_DISP` already was.
- **G7 / G5 / A7** — unreachable until G6 accepts, and GMUX-2 is the attempt to make it accept.
  Residency ("make the switch stay") still cannot be designed before a boot reaches G7. One shape to
  score when it does: the unwind no longer restores `SWITCH_EXTERNAL` from the 0x40 read, so on a
  Kepler-owned machine (`READ_EXTERNAL=0x21`, for which this tree has no cited status→write map) the
  wire carries `:: igpu-dpy: restore ext=SKIPPED (write-target port, no state read) ::`
  (`igpu.rs:1262`) and `:: igpu: [GMUX] EXTERNAL restore=SKIPPED …` at the revert, and EXTERNAL does
  **not** vote in the `gmux=MATCH|FAILED` verdict. That is deliberate — scoring a register we chose
  not to restore would report `FAILED` for a healthy flight — but it means **G7's first metal run
  leaves the external mux on IGD until power-cycle.** If that costs anything at the glass, the fix is
  a cited status→write encoding for 0x21, not a re-armed blind write-back.
- **G9** — its condition is G7's success, not its own code. It is correct to decline.
- **G2** — the teardown theory is shut out under *the four points we can observe*. If a future
  bootchain moves Point-1 earlier than the firmware's own handoff, the question re-opens; s7 named
  that alternative explicitly ("Point-1 is later in the boot than assumed").

---

## 6. PCIe / ASPM ladder — `pcihealth.rs` (+ `arch/x86_64/memory.rs`)

Knobs: `UNAOS_NOASPM` (`noaspm`) arms the ASPM clear; the census and the wedge sampler ride **every**
kepler boot regardless. `UNAOS_BAR1EXP=uc` (`bar1exp-uc`) arms the PHASE31ROOT UC-retype arm.
`UNAOS_BAR1WEDGE` (`bar1wedge`) arms the FIRST-STALL register block — P6 below, the falsifier P5
needs to be scorable. The full wedge-theory ladder behind these rungs, and the field-by-field decode
of every register named in this section, are `PCIE-RP-RECOVERY.md` §11.

| rung | knob | witness token (file:line, n) | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| P1 boot link census | — (unconditional) | `[pcih] rp-boot bdf=` (`pcihealth.rs:438`, n=1) | every kepler boot | — | — | **proven** |
| P2 ECAM trust guard | — | `[pcih] ecam-mismatch` (`pcihealth.rs:283`, n=1) | — | — | P1 | **open** (a guard; it fires or it does not) |
| P3 ASPM kill switch | `UNAOS_NOASPM` | `[pcih] aspm cleared rp ` (`pcihealth.rs:456`, n=1) | boot 11: `aspm cleared rp 0043->0040 ep 0043->0040` — **the clear took, and the machine wedged anyway at 118 s**. Flight 4 repeated it: ASPM cleared at init, three strikes regardless | **ASPM L0s/L1 is off the table as the cause of the BAR1 wedge (ledger A1)** — under these conditions: WC-typed BAR1 aperture (PAT PA4), sustained compositor paint bursts, Kepler FIFO and CE engines present *and* (rmbp-5 boot 17) equally absent. The **clear itself is proven to work**; only its curative hypothesis is shut out | P1 | **shut-out as a cure; proven as a mechanism** |
| P4 wedge sampler | — (arms when the root port has a verified ECAM page) | `[pcih] rp-at-wedge lnksta=` (`pcihealth.rs:584`, n=2) | boot 11 and every flight-4 strike: `rp-at-wedge lnksta=d081 devsta=0000 secsta=2000 aer=n` | **link training is exonerated**: boot 9 read `lnksta=d881` with the Link Training bit SET, boot 11 `d081` with it CLEAR — same wedge either way. The link is clean at the wedge, with no AER | P1 | **proven** (the instrument); the link-fault hypothesis is **shut out — with the caveat P6's decode found**: LNKSTA[15:14] are RW1C latches this kernel had never cleared and BOTH are set in `d081`, so "clean link" was read off a field that was partly reporting the whole boot. `PCIE-RP-RECOVERY.md` §11.2 |
| P5 BAR1 UC retype (PHASE31ROOT) | `UNAOS_BAR1EXP=uc` | `:: x86 bar1exp: UC arm ARMED` (`arch/x86_64/memory.rs:3729`, n=1 over `unaos/crates/kernel/src`) | **never flown.** Flight 5 deliberately did not arm it: *"UC is ~6.8x slower and would corrupt the power numbers"* | — | P3, P4 (the discriminator only means something with ASPM already excluded), **and now P6 — the flight needs both knobs or it is unscorable** | **never-run** — the M1-vs-{M2,M3} discriminator for the store-buffer-backpressure reading |
| P6 BAR1WEDGE first-stall block | `UNAOS_BAR1WEDGE` | `:: BAR1WEDGE: rung=first-stall` (`pcihealth.rs:787`, n=1) · `[pcih] wedge-sample n=` (`pcihealth.rs:906`, n=1) · `[pcih] bar1wedge sticky-cleared` (`pcihealth.rs:866`, n=1) · `[pcih] bar1wedge cto rp` (`pcihealth.rs:812`, n=2 — the armed and the UNREADABLE arm) | **never flown** — landed this arc, default OFF | — | P1 (the census resolves the root port and the ECAM page), P4 (it rides the same tripwire crossing) | **never-run** — the instrument that makes P5 scorable: it prints LNKCTL (which P4 reads and discards), the completion-timeout configuration (never read by anything, ever), and the deltas against an arm-time W1C clear of the three sticky latches |

**What would change the verdict**

- **P3** — nothing re-opens ASPM *for the wedge*; two independent flights cleared it and wedged.
  But the rung stays armed because a *different* failure (link retrain, L1 substate entry under a
  quiescent desktop) would need the same switch, and R19's whole point is that the knob outlives
  its first verdict.
- **P5** — fly it on a boot whose success criterion is **wedge/no-wedge**, not throughput. The
  6.8x slowdown that disqualified it from flight 5's power measurement is irrelevant to the
  question it answers. This is the single named, coded, never-flown experiment for ledger A1.
  **⚠ AND IT NEEDS P6 ON THE SAME LINE, which is why P6 exists.** A UC boot on its own yields one
  bit, and the 6.8x slowdown explains a quiet boot as readily as the theory does; the pair
  `UNAOS_BAR1EXP=uc UNAOS_BAR1WEDGE=1` is the experiment, and a WC boot with `UNAOS_BAR1WEDGE=1`
  alone is its control. Score card: `PCIE-RP-RECOVERY.md` §11.3.
- **P6** — it has never flown, so every cell above is a prediction. Three of its outcomes end an
  argument on their own, without P5: `dis=1` in the `cto` line convicts the completion-timeout
  theory (`PCIE-RP-RECOVERY.md` §11.1 W6) and takes §3.2's sacrificial probe off the table;
  `d_secsta=0000` at `n=1` kills the master-abort reading three boots have leaned on (W7); and
  `d_lnksta` carrying bits [15:14] RE-OPENS P4's shut-out as bandwidth renegotiation under burst,
  a claim nothing has ever tested separately. R19's shape exactly: the rung that re-opens an
  earlier failure is a rung, not a retraction.

---

## 7. R19 restoration — the seven rungs whose code was deleted, and is back

**This section used to be a list of absences. Since SHUTRESTORE (2026-09-15) it is the record of
the restore.** R19's enforcement says the code and the knob are **KEPT**. Seven refuted rungs' code
had been deleted — verified as `grep -rn -F '<token>' unaos/crates/kernel/src` returning **0** at
`27716175`. All seven are now back, restored from OUR OWN git history (never anything external),
each behind a **default-OFF** feature of its own inside the existing `nvidia-kepler` cfg region, so
an unarmed image links not one byte of any of them and no rung reaches the boot path unasked.

| rung | knob (feature) | restored from | restored where | state |
| --- | --- | --- | --- | --- |
| KF6 `USERD_SNOOP` | `UNAOS_KEPLER_USERD_SNOOP` (`nvidia-kepler-userdsnoop`) | `7124e4e1`, deleted by `384449d7` ("kepler-fence pull 14") | `kepler.rs`, pre-init arm + the witness-conditional restore beside `PFIFO_CHAN[1] pre-submit` | **restored, whole** |
| KF8 `PFIFO_FLUSH` / `flush-executed` | `UNAOS_KEPLER_PFIFO_FLUSH` (`nvidia-kepler-pfifoflush`) | `7420e06f` (introduced `d63a1495`), deleted by `384449d7` | `kepler.rs`, pre-init | **restored, whole** |
| KF9 `CTRL_ADDR TARGET` | `UNAOS_KEPLER_CTRL_ADDR` (`nvidia-kepler-ctrladdr`) | `3620c7d5`, deleted by `51b98bab` ("kepler-fence pull 15") | `kepler.rs`, pre-init | **restored in part** — the 3×4 reversible TARGET audit is back; the s13 original re-ran the whole channel bringup *inside* the target loop and that surrounding code no longer exists in this shape (rmbp-queue `· NEW SHUTRESTORE-CTRLADDR-BRINGUP`) |
| KD6 `repoint` 0x6101E0 | `UNAOS_KEPLER_REPOINT` (`nvidia-kepler-repoint`) | `896faee0`, deleted by `9ff1a9c2` ("kepler-display pull 7") | `kepler_display.rs`, `repoint_surface` | **restored, whole** — and the hard-coded surface pointer is now derived from the live GOP offset instead of the s15 literal `0x00016000` |
| KD7 latch arm + UPDATE | `UNAOS_KEPLER_LATCH_ARM` (`nvidia-kepler-latcharm`) | `bfeedd94`, deleted by `104fed6e` ("pull 19 relocate decisive") | `kepler_display.rs`, `latch_arm_update` | **restored in part** — the write half, the `pm-step` dumps and both `asm-stuck` / `armed-followed` verdicts are back; the original's `return None` out of `takeover_display` is deliberately NOT restored, because a knob-gated rung must not change the takeover's contract |
| `lin-step` / `bwpg` display parameter ladders | `UNAOS_KEPLER_PITCH_LADDER` (`nvidia-kepler-pitchladder`) | `eee60395` (lin-step, deleted by `b9f3d9bf`) and `04b494be` (bwpg, deleted by `f6dd1961`) | `kepler_display.rs`, `pitch_ladders` | **restored, whole** — both ladders in one function, the four `(bw, pg)` cycles intact |
| `gop-overlap` detector | `UNAOS_KEPLER_GOP_OVERLAP` (`nvidia-kepler-gopoverlap`) | `bfeedd94`, deleted by `2d1da2e3` ("replace the now-tautological overlap test with an exact-cover check") | `kepler_display.rs`, `gop_overlap_probe`, called beside the `fb-draw cover=` line | **restored, whole** |

**Why each deleting commit does not excuse itself.** Five of the six deleting commits carry a bare
subject line ("implement kepler-fence pull 14 [none]") and no body — there is no recorded reason at
all, so R19's default applies and the code comes back. The sixth, `2d1da2e3`, DOES give a reason:
the overlap test had become tautological once we drew at the GOP base by design, and the `cover=`
check replaced it. That reason is sound for the *call site* and unsound for the *deletion*: cover
answers extent, intersection answers validity, and the moment a rung paints somewhere other than
the GOP base — which `nvidia-kepler-pitchladder` does, at `0x1600000` — `gop-overlap` is the only
line that can say the photo is void. So it is restored as a detector, with the `cover=` line left
exactly where it was.

**Still knobless hard-codes, NOT part of this restore** (code kept, knob absent — neither can be
re-armed from a media build without an edit): `run_recon` (`kepler_display.rs`) is a literal
`false` and `do_takeover` a literal `true`. They were never deleted, so they are not §7's subject;
they are noted here so the next reader does not mistake the restore for having covered them.

**How to arm a restored rung.** Each knob is default OFF and independent, and each needs its parent
to REACH its call site: the three FIFO rungs need `UNAOS_KEPLER=1 UNAOS_KEPLER_FIFO=1`, the four
display rungs need `UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1`. Arming a rung without its parent is
not an error — it is a rung that never runs, which the absent witness token says plainly. All seven
are mapped in `unaos/arroyo`'s knob block AND read by `unaos/builder/src/main.rs`, so a metal boot
carries what the banner claims (the s42/INSTGUI and rastmc lesson); `./arroyo check`'s KNOB→BUILDER
WIRING CHECK reds the build if either half is dropped, and the seven are named on the literal
`x86-all` leg so their ARMED bodies are type-checked alongside `nvidia-kepler`.

**This section is a finding, not an accusation**: nothing here was trashed maliciously, and every
deletion happened before R19 existed (2026-09-06). It is kept, now that the job is done, so no
later reader mistakes a knob that is OFF for a rung that never ran.

---

## 8. Peter's two questions, answered from the tables

### "Where are we with the GPU?"

**We own the panel and we cannot yet command an engine on the Kepler — but we now command one on
the Intel.** On the Kepler the display half is finished and proven: KD4 found head 0 alive, KD8
read the firmware's own storage parameters (linear, pitch 16384), KD9 painted the full panel
`cover=exact`, KD10 proved fbcon was never mis-strided, KD12 put the kernel console on the glass and
KD13 put it in a compositor window — thirteen sittings from first pixel to a working desktop, and
every flight since `94b0ed0c` has rendered on Kepler. The compute half is a ten-deep elimination
against one unmoved wall: KF5's `err=00000002 stat=00000005 valid=00002000` has been byte-identical
across runlist encodings, USERD variants, a flush, CTRL_ADDR, a powered engine (KF11), a live
running engine (KF15), a halted engine, and a host-populated CHAN_CUR/CHAN_NEXT (KF18/KF19) —
while every constructive rung passed: KF12 found the real falcons, KF13 proved the memory ports,
KF14 executed our own microcode (`mailbox0=F00DFACE`), KF16 completed a host↔FECS command loop and
KF24 closed the paged-BAR1 alarm as a false alarm, which keeps all ten eliminations valid. The
remaining actor is named and is not a mystery: **the FECS context-switch microcode**, which nothing
we can reach from the host will substitute for — KF10 even took the chip's own error name away as
evidence. The copy-engine ladder (§3) is built, gated, reversible and **never flown**; one boot
with `UNAOS_KEPLER_CE=1` decides five rungs at once. And on the Intel side the ladder that was
supposed to be the long shot is the one that worked: **R7 copied pixels on metal on flight 4, both
boots, `best_dst_match=256/256`, `dst_crc==src_crc`** — after R2, R3 and R5 had each been recorded
as a failure. That is R19 paying for itself in one table: R5's `enable-void` was *failed under: no
forcewake hold in force*, and R6 changed that single condition and walked through.

### "What about iGPU and power management?"

**The iGPU question is not "is the Intel GPU usable" — it is answered, it is — it is "can we take
the panel", and that has never actually been tried.** G1 and G2 proved every iGPU pipe and plane is
dark at all four observable points, including first-instruction bootloader entry, which killed the
firmware-teardown theory. G3 then proved the gmux protocol (version 3.2.19, after s9's failure
under a 3×8-bit read width) and G4 read the answer: `SW_DISPLAY=0x03 DISCRETE`, `SW_DDC=0x02`,
`DISC_POWER=0x03` at boot **and** at kernel — **the Kepler owns the panel at every observed
instant**, so the iGPU being dark is the expected state, not a paradox, and G9's `ring=absent
why=no-active-surface` is the correct refusal rather than a defect. The switch itself, G7, has
**never run on metal**: flight 5 stopped at G6's pre-switch gate with `LADDER highest=00/10
gmux=UNTOUCHED why=pre-switch-not-accepted`, which falsified the inherited claim that the mux
"switches and restores on the same call stack" — it does not switch at all, so ledger A7's
`GMUX_SWITCH_EXTERNAL=0x01` residency blocker describes a rung that has never executed. The
cheapest item in this entire register is the fix for that: the gate prints `igpu-dpy: pre-switch
state DDC=… SW_DISP=… SW_EXT=… DISP=… EXT=…` at `igpu.rs:1214` **before** it refuses at `:1245`, so
the flight-5 capture already named the offending register; GMUX-1 read it back on 2026-09-15 (a wrong READ of the write-target port 0x40) and GMUX-2 fixed the gate the same day — no boot
required. On power management, the picture is narrower and cleaner: P3 proved the ASPM clear works
on the wire (`aspm cleared rp 0043->0040 ep 0043->0040`) and proved it is **not** the cure for the
BAR1 wedge (ledger A1) — boot 11 and flight 4 both wedged with ASPM off — while P4 exonerated link
training (`lnksta=d881` SET vs `d081` CLEAR, same wedge) and shows a clean link with no AER at every
strike. What is left is the one coded, knob-gated, never-flown experiment in the whole power story:
**P5, `UNAOS_BAR1EXP=uc`**, the UC-vs-WC discriminator for the store-buffer-backpressure reading —
skipped on flight 5 only because UC is ~6.8x slower and would have corrupted that flight's power
numbers, which is no reason at all on a boot scored wedge/no-wedge. Separately, SMC has no `AC-W`
key on this machine (a clean negative from flight 5), so any wattage baseline must be built from
`WCPW`/`SMBW`/`MSDW`/`MSQW`/`HDSW`.

---

## 9. Headline

| ladder | rungs | open | shut-out | never-run | proven |
| --- | --- | --- | --- | --- | --- |
| §1 Kepler display | 13 | 1 | 3 | 0 | 9 |
| §2 Kepler FIFO / PBDMA | 26 | 5 | 10 | 0 | 11 |
| §3 Kepler CE | 6 | 0 | 0 | 6 | 0 |
| §4 gen7 R1–R7 | 9 | 0 | 5 | 0 | 4 |
| §5 GMUX / iGPU | 9 | 1 | 3 | 2 | 3 |
| §6 PCIe / ASPM | 5 | 1 | 1 | 1 | 2 |
| **total** | **68** | **8** | **22** | **9** | **29** |

A rung whose status cell reads "shut-out … ; proven as …" is counted **shut-out**: the hypothesis
it was flown to test is the thing that failed, and that is what this register is for. **Nothing
anywhere in this file is "ruled out."**

Rungs whose recorded failure a **later rung's conditions changed** — R19's own scenario, observed
twenty-one times in this tree: KD3 (re-opened by KD4), KD6, KD7 (blocked on §2, not refuted),
KF3, KF6, KF7, KF8, KF9, KF11, KF12, KF13, KF14, KF18, KF19, KF25, G2, G3, G6, R2, R3, R5.
Of those, **four had their condition changed and then passed** — KF12→KF13, KF13→KF14, R5→R6,
R6→R7 — and the rest have a named condition nobody has varied yet. Eleven `· NEW` rows in
`docs/dev/OS/rmbp-queue.md` (section "GPU LADDERS") carry them. Of the three jobs that needed no
flight at all, all three are now done: GMUX-1 (read a line already on disk), SHUTRESTORE (§7) and
GEN7DOC (§4).

The seven rungs of §7 could not be re-flown at all while their code was deleted. **Their code is
back** (SHUTRESTORE, 2026-09-15), each behind a default-OFF knob of its own, so what each of them
now needs is a flight and nothing else — the knobs and their parents are tabulated in §7 and the
flight lines in [`KEPLER-METAL-LOG.md`](KEPLER-METAL-LOG.md). Two carve-outs stay open as
`docs/dev/OS/rmbp-queue.md` rows: KF9's per-TARGET channel-bringup re-run needs a seam carved out
of `kepler::init` (`SHUTRESTORE-CTRLADDR-BRINGUP`), and the two knobless hard-codes `run_recon` /
`do_takeover` still cannot be re-armed from a media build (`SHUTRESTORE-KNOBLESS`).
