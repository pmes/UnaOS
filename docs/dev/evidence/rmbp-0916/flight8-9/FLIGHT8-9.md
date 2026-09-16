# Flights 8 and 9 — the score (rMBP, 2026-09-16)

Two attended boots on one capture, twelve days and 53 folds after flight 7. Flight 8 is image B
(the WC control); flight 9 is image A, the same tree plus `UNAOS_BAR1EXP=uc`. The pre-registration
is `PLAYBOOK-flown.md` beside this file, copied verbatim before any line was read.

**Capture (read-only source):** `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`, one file holding
both boots, split by `=== SQUAWK MARK …` lines. Flight 8 runs from `flight8 card written f4b2ca01`
to `flight8 DONE`; flight 9 from `flight9 card written` to the end. Flight 7's comparison capture is
`~/unaos-bench/capture/rmbp11-flight7/ttyUSB1.log` (image `bc10a469`, 2026-09-03). Every line below
is copied verbatim from those captures, read with `awk 'index($0,"<tag>")'`, never a bare `grep`.
In-boot `[ NNNNNms]` timestamps restart at each boot.

**Images.** Flight 8: `UnaOS-rmbp-esp-rmbp12flight8-20260916T0515Z-bf299a3`, `kernel.elf`
sha256 `f4b2ca01…`, built from `hw-rmbp@bf299a37`. Flight 9:
`UnaOS-rmbp-esp-rmbp12flight9-20260916T0519Z-08e1ec9`, `f8473c10…`, from `hw-rmbp@08e1ec90`. The
knob lines are in `~/unaos-bench/flash/rmbp/MANIFEST`; they are identical except for the trailing
`UNAOS_BAR1EXP=uc` on image A. **Both carry `UNAOS_WIFI=1 UNAOS_WIFI2=1`; neither carries
`UNAOS_QUARRY`** — two facts that decide findings 2 and 4 below.

---

## 1. Score card

| rung / row | verdict on the wire | status |
| --- | --- | --- |
| B89 AHCIFLY | `port=0 model="APPLE SSD SM768E" sectors=1467339812 lba48=1`, `sector0 sig=0xaa55 kind=GPT`, `selfcheck … -> PASS`, `ahci census: sata_sources=1 with_fat_volume=1 carrying_AHCIBOOT.TXT=0` | flown, PASS |
| X86BIND | `root=sdhc:/kernel.elf … by=content … -> PASS`; `/` `/boot` `/apps` on one id, `/volumes/EFI` listed not bound | flown, PASS |
| A9 FTDIRX | `first byte rx=1 byte=0x68 'h'`, then the shell answered `date` | flown, CLOSED |
| A3 RBTDRAIN | four `[pwrreboot]` lines on the cable, `ftdi flushed bytes=7503 transfers=118 exhausted=0` last | flown, CLOSED |
| A7 GMUX-2 | `gate=ACCEPT`, `restore ext=SKIPPED`; ladder `gmux=FAILED` (f8) / `gmux=MATCH` (f9) | flown, gate fixed, ladder open |
| A5 BEAMX86 | `-> ARMED`, `beam=obs` on every rollup; shell `torn=602`, storm vugs ≤ 3 | flown, NOT closed |
| KD14 kdhead | `blocks=5 separated=2 decoded=1 agree=1 disagree=0 writes=0`, head 0 `-> AGREE` | flown, PROVEN; re-opens KD3 |
| KF27 kfbind | `verdict base=NEITHER-ANSWERS … -> STILL-DARK under: …`, `writes=0` | flown, OPEN |
| R8 fb_blit | `r8 verdict=r8-refused-surface-too-large dst_bytes=1064960 max=1048576 writes=0` | flown, the rung DID NOT RUN |
| A1 BAR1WEDGE pair | both legs stalled; §11.3 outcome in §5 below | flown, A1 still open |
| A4 | reset landed in Catalina | open, as predicted |
| B45 keyboard | 40+ characters, Peter: "looks good"; the keyboard is on EHCI, not xHCI | flown |

All three of R7, KDHEAD and KFBIND read **identically on both boots**, quoted once each in §4.

---

## 2. Corrections to the seat's live reading

The seat's reading was verified line by line against the capture. Five claims were wrong and are
corrected here; the corrections are the reason this file exists.

1. **`gmux=` is NOT identical between the boots.** Flight 8:
   `:: igpu-dpy: LADDER highest=05/10 name=end ok=1 pending=2 gmux=FAILED why=none elapsed_ms=10 ::`.
   Flight 9: `:: igpu-dpy: LADDER highest=03/10 name=dpcd ok=0 pending=2 gmux=MATCH why=aux-timeout-error elapsed_ms=9 ::`.
   Same machine, same registers, one knob apart — and the boot that got FURTHER voted FAILED while
   the boot that stopped at `dpcd` voted MATCH. See finding 6.
2. **The UC arm's witness is not the line the seat named.**
   `:: x86 mmio-map: 0xc1400000..0xc1800000 uc=2 (PAT PA3) wc-kept=0 ::` prints **byte-identically on
   both boots** and discriminates nothing. The one wire line that differs is the BAR1 aperture's:
   flight 8 `:: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::`, flight 9
   `:: x86 mmio-map: 0x90000000..0xa0000000 uc=128 (PAT PA3) wc-kept=0 ::`. Eight map lines print on
   each boot; those two are the only pair that differs (measured by sorting both sets).
3. **The retype path DID run on both boots.** `crate::bootpace::record("fb-wc")` sits *inside* the
   `FB_WC_DONE` one-shot latch (`memory.rs:3649-3654`), and `fb-wc` / `fb-wc-done` are on both
   wires. What did not run is neither arm's *witness line* — see finding 3.
4. **The tear counts.** Flight 8's shell is `torn=602`, not 0 or 1; flight 9's maximum is
   `torn=935`, not 692 (692 is the ninth of fourteen rollups on that window). Per-window table in §6.
5. **`wedge-sample` counts.** Flight 8 has **16** samples (`n=1..16`), flight 9 has **12**
   (`n=1..12`), counted with `awk 'index($0,"[pcih] wedge-sample n=")' | wc -l` — the naive pattern
   also matches the `sticky-cleared post-enum` line and both squawk marks, which is where 13 and 18
   came from.

---

## 3. Boot time — Peter's sentence, and what the kernel's own clock says

Peter, at the bench: *"both boots took much longer than they have in the past"*. His sentence stands;
the measurement below is what the wire can and cannot say about it.

`BPACE` is the kernel's own boot-pace ledger, replayed at the end of boot. Same anchors, all three
flights:

| measure | flight 7 | flight 8 | flight 9 |
| --- | --- | --- | --- |
| `BPACE: total gui=` | 25533 ms | **23286 ms** | **21387 ms** |
| `BPACE: ehci-hid-done d=` | 24516 ms | 22008 ms | 18215 ms |
| `bt-c1: C1 tally — elapsed=` | 21778 ms | 19268 ms | 15924 ms |
| first `[dock] live` | 25281 ms | 23005 ms | 20611 ms |
| bytes on the wire before it | 168945 | 161833 | 163680 |
| lines on the wire before it | 1742 | 1482 | 1538 |

Verbatim, the dominant term on each boot:

```
flight 7  [  24778ms] :: bt-c1: [1] C1 tally — elapsed=21778ms events_read=6 inquiry_responses=0 …
flight 8  [  22270ms] :: bt-c1: [1] C1 tally — elapsed=19268ms events_read=17 inquiry_responses=0 …
flight 9  [  18716ms] :: bt-c1: [1] C1 tally — elapsed=15924ms events_read=15 inquiry_responses=0 …
```

**The Bluetooth C1 pass is 85 % of the kernel's boot on every one of the three flights**, it runs
inside the EHCI-HID pass, and the internal keyboard/trackpad (`05ac:0262`, hub 3 port 2) cannot be
walked until it finishes: flight 8's `hub 3 port 2 PORT_RESET cleared` is at 22286 ms, 20.5 s after
the previous EHCI line at 1794 ms, and the only lines in between are `bt-*` and the 1 Hz
`[deadman]` tick.

**Both flights reached the desktop FASTER than flight 7 by the kernel's clock — 2247 ms and
4146 ms — on fewer wire bytes.** So whatever Peter saw is not in the interval the kernel measures.
What the kernel's clock cannot see is the pre-kernel phase: the firmware, the ⌥ picker, and the
loader reading a 3.8 MB `kernel.elf` off the SD card. Bounding it from the squawk marks (the mark's
host UTC minus the in-boot ms of the line after it) puts kernel `t=0` at 15:03:12.7Z for flight 8
and 15:13:16.8Z for flight 9, i.e. **≤ 47.7 s and ≤ 49.8 s after the card-written mark** — and those
bounds include Peter's hand at the boot picker, so they are upper bounds and nothing more. Flight 7's
capture carries no squawk marks, so the same bound cannot be taken for it and **the pre-kernel phase
is unmeasured on all three flights.** That is the open question, and it is row A12.

---

## 4. The rungs, verbatim

### A9 — FTDIRX, closed on metal

```
[ 103726ms] :: FTDIRX: first byte rx=1 byte=0x68 'h' idle=4985 ::
[ 103726ms] :: FTDIRX: rx=5 packets=4986 idle=4985 errors=0 result=OK ::
[ 106765ms] :: ui3:date: date: clock not set (date -s YYYY-MM-DD HH:MM:SS) ::
```

Typing over the wire works on this laptop for the first time, and the shell answered. Flight 9's
first byte is `byte=0x73 's'` at 65472 ms — the `storm` the seat typed.

### B89 — AHCIFLY, PASS

```
[  23256ms] :: AHCI: port=0 model="APPLE SSD SM768E" sectors=1467339812 lba48=1 ::
[  23256ms] :: AHCI: port=0 sector0 sig=0xaa55 kind=GPT ::
[  23256ms] :: AHCI: registered port=0 as registry index 0 — blocks=1467339812 (716474 MiB) READ-ONLY (global BLOCK_DEVICE untouched, installer not told) ::
[  23256ms] :: AHCI: selfcheck port=0 identify=ok sector0=GPT -> PASS ::
[  23262ms] [bootdisk] volume source=ahci port=0 vol=EFI serial=0x67e317ed blocks=1467339812 volumes_by_content=2 ::
[  23263ms] [bootdisk] ahci census: sata_sources=1 with_fat_volume=1 carrying_AHCIBOOT.TXT=0 ::
```

The controller B89 said was "enumerated, printed, and driven by nothing" is now read on its own
metal: Catalina's SSD identified, its GPT read, its ESP found by content, nothing bound to it.
`carrying_AHCIBOOT.TXT=0` is the honest answer the playbook pre-registered. Flight 9 is identical to
the sector (`[  21097ms]`, same model, same sector count, same serial `0x67e317ed`).

### X86BIND — root by content, on metal

```
[  23372ms] :: X86BIND: root=sdhc:/kernel.elf serial=0x00000000 by=content bootinfo=0x00000000 agrees=unknown mounts=4 layout=true -> PASS ::
[  23383ms] :: [fatverb] storage settle: waited=0ms settled=found handles=global=absent sdhc=present ::
[  23398ms] :: volid: mount /volumes/EFI name=EFI id=Some(13300357605406014097) ::
[  23408ms] :: volid: mount / name=boot id=Some(18164773122540445248) ::
[  23418ms] :: volid: mount /boot name=boot id=Some(18164773122540445248) ::
[  23424ms] :: volid: mount /apps name=boot id=Some(18164773122540445248) ::
```

`/`, `/boot` and `/apps` share one volume id; Catalina's ESP is listed under `/volumes/` and is not
`/`. **`waited=0ms settled=found` is CORRECT on this machine and not a STORWAIT regression**: on the
rMBP the SD slot IS the boot medium (`bdf 3:0.1 14e4:16bc … (sdhci)`), so the predicate STORWAIT
installed — has MY disk arrived — was satisfied on pass 1. The `serial=0x00000000` and
`agrees=unknown` are the loader observation, never the verdict.

### A7 — the gmux gate

```
[  23179ms] :: igpu-dpy: pre-switch state DDC=0x02 SW_DISP=0x03 SW_EXT=0x01 SW_EXT_ST=0x21 DISP=0x03 EXT=0x21 sw_ext_state=UNACCEPTED ext_state=kepler-owned gate=ACCEPT ::
[  23179ms] :: igpu-dpy: restore ext=SKIPPED (write-target port, no state read) SW_EXT=0x01 SW_EXT_ST=0x21 ext_state=kepler-owned ::
[  23180ms] :: igpu-dpy: rung=00 name=census ok=1 bdsm=0x8BA00001 ggc=0x00000211 ggtt0=0x8BA00003 ggtt1=0x8BA01003 aux_ctl=0x014300C8 frmcnt=0x00000000 ::
[  23186ms] :: igpu: [GMUX] switch read-back mismatch (DDC is the only echo proven on this machine; DISP/EXT are advisory — see the AUX rungs for whether the mux moved): DDC=0x01 DISP=0x02 EXT=0x01 ::
[  23188ms] :: igpu-dpy: LADDER highest=05/10 name=end ok=1 pending=2 gmux=FAILED why=none elapsed_ms=10 ::
```

GMUX-2's fix is proven exactly as pre-registered: `SW_EXT=0x01` no longer fails the gate
(`gate=ACCEPT`), and the restore writes nothing it did not read (`restore ext=SKIPPED`), so the
external mux is left on IGD until power-cycle — deliberate and printed. The switch then ran and
DDC echoed `0x01`. **The ladder is a different question and it is not fixed**: `highest=05/10` with
`gmux=FAILED why=none`. Flight 9, one knob apart: `highest=03/10 name=dpcd ok=0 … gmux=MATCH
why=aux-timeout-error`. Finding 6.

### R7 / R8

```
[  22549ms] :: gen7: r7 verdict=r7-blit-verified by=mt mode=scratch-fill wake=gt-live-already engine=BCS attempts=1/3 any_ctl_enabled=1 best_ctl_readback=00000001 any_head_moved=1 any_sentinel=1 any_copy=1 best_dst_match=256/256 … ::
[  22549ms] :: gen7: r8 geometry fb=panel fb_w=2880 fb_h=1800 stride_px=4096 bpp=4 fb_base=0000000090020000 fb_len=29491200 dst_pitch_bytes=16384 dst_rows=65 dst_span_bytes=1064960 dst_bytes=1064960 dst_pages=260 src_pitch_bytes=256 src_bytes=16384 src_pages=4 rect=64x64 at_x=2816 at_y=0 corner=top-right … ::
[  22549ms] :: gen7: r8 verdict=r8-refused-surface-too-large dst_bytes=1064960 max=1048576 writes=0 note=refused-not-trimmed-a-rung-that-shrinks-its-own-experiment-reports-a-verdict-about-a-different-experiment ::
[  22549ms] :: gen7: r8 next=STOP-destination-surface-over-the-stated-ceiling-raise-R8_DST_MAX_BYTES-deliberately-or-shrink-the-rect note=no-hold-no-ring-armed ::
```

R7 passed its precondition, so R8 was allowed to run, and **R8 refused itself**. Byte-identical on
both boots. The refusal is the rung behaving correctly (R19: a rung that shrinks its own experiment
reports a verdict about a different experiment). Finding 1 is the arithmetic behind it.

### A5 — BEAMX86 armed, and the tear counter finally moves

```
[  22821ms] :: BEAMX86: head=0 vtotal=1852 samples=22025 vblank_delta=2 -> ARMED :: max_vline=1851 adv=5109 panel_h=1800 evo_size=00000000 size_hi=0 size_lo=0 sample_ms=45 census_adv=[5109,0,0,0] census_vbd=[2,0,0,0] …
[ 378648ms] [wc-h] rollup win=2 scope=window emit=8 … torn=602 stalls=0 longpres=0 … beam=obs beamobs=7437 beamwaits=7152 beamwait_us=2546969 beammaxwait_us=21834 beamgiveup=0 … maxpresent_us=11630 minpresent_us=9 presspread=50 presspop=7437 … -> AT-RISK
```

`win=2` is the Shell (`[  23305ms] [wm] alloc win=2 gen=2 owner=0xffffff02 title="Shell"`).

**The baseline matters and it is not what the playbook said.** Flight 7's capture contains **zero**
lines carrying `beam=` and its highest `torn=` anywhere is 4: on flight 7 every present was
unbracketed and the counter was silent while the panel tore. A5's `torn=111` is F6's number, not
flight 7's. So BEAMX86 did the one thing its row required — `arch::scanout_beam()` answers and the
counter advances — and the shell now reports 602 torn presents of 7437. **That does not close A5.
It converts it**, exactly as the row's own closing clause pre-registered.

### KD14 — flown, and it re-opens KD3

```
[  22822ms] :: KDHEAD: bracket source=beamx86-census live=[y,n,n,n] :: census_adv=[5109,0,0,0] census_vbd=[2,0,0,0] …
[  22822ms] :: KDHEAD: gop w=2880 h=1800 vram_off=00020000 pitch=16384 …
[  22830ms] :: KDHEAD: block=headstat base=0x616000 stride=0x800 heads_distinct=4/4 :: stable=3/4 readable=4/4 verdict=SEPARATES-heads …
[  22831ms] :: KDHEAD: block=headstat decode=none reason=no-cited-slicing …
[  22838ms] :: KDHEAD: block=armed100 base=0x616100 stride=0x800 heads_distinct=1/4 :: stable=3/3 readable=4/4 verdict=COLLAPSED-stride-does-not-separate-heads-here …
[  22847ms] :: KDHEAD: block=evocore base=0x610460 stride=0x300 heads_distinct=1/4 :: stable=3/3 readable=4/4 verdict=COLLAPSED-stride-does-not-separate-heads-here …
[  22855ms] :: KDHEAD: block=headval base=0x610A00 stride=0x540 heads_distinct=1/4 :: stable=3/3 readable=4/4 verdict=COLLAPSED-stride-does-not-separate-heads-here …
[  22863ms] :: KDHEAD: block=mirror base=0x640400 stride=0x300 heads_distinct=2/4 :: stable=4/4 readable=4/4 verdict=SEPARATES-heads …
[  22864ms] :: KDHEAD: head=0 live=yes geom=2880x1800 surface=0x20000 pitch=16384 vs gop=2880x1800 0x20000 16384 -> AGREE :: block=mirror mismatch=none surface_present=y pitch_present=y …
[  22867ms] :: KDHEAD: end rung=KD14 bracket=beamx86-census blocks=5 separated=2 decoded=1 agree=1 disagree=0 writes=0 …
```

The three numbers the pending entry asked for, answered:

1. **`mirror` reads `heads_distinct=2/4` with `stable=4/4`** — the core-channel method stride DOES
   separate heads on this part, and the per-head decode has a block to stand on.
2. **`armed100` reads `heads_distinct=1/4` with `stable=3/3`** — sitting #4's inference that its
   stride collapsed is now an observation, not an inference.
3. **`-> AGREE` on head 0**, the only live head. `mirror`'s slicing (geom `+0x68` lo16 × hi16,
   surface `+0x60 << 8`, pitch `+0x6C & 0xFFFF`) reproduces the GOP's 2880×1800, `0x20000`, 16384
   exactly. Per the rung's own contract this **pins the decode by observation and re-opens KD3**,
   the same way KD4 re-opened it in s11.

`headstat` also separates (`4/4`) but decodes to `decode=none reason=no-cited-slicing`, by design.
Byte-identical on flight 9 (`[  20147ms] … blocks=5 separated=2 decoded=1 agree=1 disagree=0`).

### KF27 — flown, STILL-DARK, and the CE-R1 cross-check agrees

```
[  23164ms] :: KFBIND: ptop pbdma=4 runlists=[01CD] pbdma0_base=023000 entries=64 nonzero=16 poison=0 refused_out_of_window=12 overflow=0 ctl_pre=0E7150A2 ctl_post=0E7150A2 ctl=held …
[  23164ms] :: KFBIND: pbdma[0] DERIVED pre-submit base=023000 status=BADF1100 POISON chan=BADF1100 POISON ctrl_lo=BADF1100 POISON ctrl_hi=BADF1100 POISON answers=n …
[  23165ms] :: KFBIND: pbdma[0] LEGACY pre-submit base=040000 status=00000000 ZERO chan=00000000 ZERO ctrl_lo=00000000 ZERO ctrl_hi=00000000 ZERO answers=n …
[  23172ms] :: KFBIND: userd pre-submit off=02002000 ib_get=00000000 ZERO ib_put=00000000 ZERO x090=00000000 ZERO dma_put=00000000 ZERO dma_get=00000000 ZERO …
[  23177ms] :: KFBIND: verdict base=NEITHER-ANSWERS — every dword at every base, derived and legacy, is ZERO or POISON with a HELD bracket. That is a statement about the whole PBDMA PRI space on this part and it is the strongest reading this rung can produce without a start/clock sequence it may not guess ib_get 00000000->00000000 ZERO ib_put 00000000->00000000 derived_n=4 ctl_pre=0E7150A2 ctl_post=0E7150A2 ctl=held writes=0 restored=n/a -> STILL-DARK under: PTOP-derived base comparison as scored above, FECS context microcode NOT resident (this boot runs no ctx ucode before the submit), CHAN_CUR/CHAN_NEXT NOT host-populated by this rung, PBDMA start/clock sequence NOT written (uncited, skipped above), runlist submitted via 0x2270/0x2274 and its playlist echo scored by kepler::init's own post-bind line. NOT 'ruled out' (R19): this names the conditions IB_GET did not move under, and IB_GET itself was read here for the first time in the campaign ::
[  23177ms] :: KFBIND: end rung=KF27 ptop=derived base_cmp=neither fetch=still-dark — one rollup line so a silent rung is distinguishable from a quiet pass ::
```

Three things this boot settles:

* **IB_GET (USERD `+0x88`) has now been read.** It reads `00000000` before and after the submit. The
  campaign's ten sittings had been reading `0x8C`/`0x90` (IB_PUT and an unnamed word).
* **Neither base answers.** The derived bases (`023000`/`027000`/`02B000`/`02F000`) return
  `BADF1100` POISON; the legacy bases (`040000`/`042000`/`044000`) return clean `00000000`. The
  rung refused to decode CHID/ACTIVE at an unproven base — the ten-sitting error it was built to stop.
* **Two writes were skipped for citation**, and both reasons are on the wire:
  `skipped write=pbdma_chan_bind reason=uncited` and `skipped write=pbdma_start_clock reason=uncited`.

**CE-R1 cross-check, measured rather than asserted.** `kepler: ce-ptop row i=…` and
`KFBIND: ptop-row i=…` were compared field by field on both boots: **64 of 64 entries identical,
8 of 8 rows identical, on each boot**, e.g.

```
[  23098ms] :: kepler: ce-ptop row i=00..07 [8006183E 00000003 90828C3E 00000007 94A30E3E 0000000B 9C03AA3E 0000000F] ::
[  23163ms] :: KFBIND:  ptop-row i=00..07 [8006183E 00000003 90828C3E 00000007 94A30E3E 0000000B 9C03AA3E 0000000F] ::
```

The two independent readers of the PTOP table agree dword for dword.

### A3 — RBTDRAIN, closed, and the playbook's KNOWN list was wrong

```
[ 394356ms] [pwrreboot] reboot verb invoked — dispatching the platform mechanism
[ 394356ms] [pwrreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)
[ 394356ms] [pwrreboot] ring drained lines=0 bytes=0 mirror=ok
[ 395002ms] [pwrreboot] ftdi flushed bytes=7503 transfers=118 exhausted=0
```

Those are the last four lines of flight 8 and the last of them is the last byte before the reset.
**`[pwrreboot] ring drained …` DID reach the cable** — `mirror=ok` on it is SINKDRAIN's fix, and the
playbook's KNOWN-AND-EXPECTED list ("`[pwrreboot] ring drained …` … never reach the cable") is
corrected by this capture. Flight 9's tail is the same shape with `bytes=436 transfers=24`.

### B45 — the keyboard, and which controller it is actually on

Peter typed 40+ characters and said "looks good". The device is:

```
[  22299ms] :: EHCI-HID: [1] M1 hub-downstream device addr=8 05ac:0262 class=0x00 speed=FS depth=2 (parent hub 3 port 2) tt=(hub 3 port 2) == witness ::
[  22301ms] :: EHCI-HID: [1] M2 armed keyboard addr=8 ep=IN3 mps=10 interval=8 (boot protocol) == witness ::
[  22302ms] :: EHCI-HID: [1] M1 armed vendor-multitouch addr=8 ep=IN1 mps=64 interval=2 id=0x44 body=4088b …
```

**The internal keyboard and trackpad are one device on EHCI controller [1], hub 3 port 2 — not on
xHCI at all.** So `XHCIKBD`'s N-outstanding-TD repair was **not in the path of this flight**, and
B45's own conditions are unchanged by it: only an xHCI keyboard exercises that fix, and this laptop
has none. What flight 8 does prove is the EHCI path, which halted twice at boot and recovered:

```
[  23299ms] :: KBDWIT: [1] ep=IN1 addr=5 kind=kbd SILENCE-CUT-BY-HALT class=xact-err-burn tok=0x00088141 … halted=1 … ::
[  23536ms] :: KBDWIT: [1] ep=IN1 addr=5 kind=kbd NO-COMPLETIONS class=never-completed … dead=1 == witness ::
[  35710ms] :: KBDWIT: [1] ep=IN1 addr=8 SILENCE-BROKE tok=0x00388d00 halted=0 … polls=1977 walks=1977 … == witness ::
[ 207408ms] :: KBDWIT: [1] ep=IN3 addr=8 SILENCE-BROKE tok=0x80028d00 halted=0 … polls=144614 … == witness ::
```

addrs 5 and 6 are the external dock's boot keyboard and mouse and they died at boot; addr 8 — the
internal one — broke silence and served the whole flight.

---

## 5. A1 — the BAR1WEDGE pair, and the §11.3 outcome

### What each boot printed

```
f8 [  22549ms] :: BAR1WEDGE: rung=first-stall armed=UNAOS_BAR1WEDGE aperture=wc rp=0:1.0 capver=2 v2=1 baseline=lnksta=d081(2.5GT/s x8) lnkctl=0040(aspm=off) devsta=0000 secsta=2000 devctl2=0000 aer=n ::
f9 [  18995ms] :: BAR1WEDGE: rung=first-stall armed=UNAOS_BAR1WEDGE aperture=uc rp=0:1.0 capver=2 v2=1 baseline=lnksta=d081(2.5GT/s x8) lnkctl=0040(aspm=off) devsta=0000 secsta=2000 devctl2=0000 aer=n ::

f8 [  22549ms] [pcih] bar1wedge cto rp devcap2=00000000 ranges=0 cto_dis_sup=0 devctl2=0000 value=50us-50ms(default) dis=0 — …
f8 [  22549ms] [pcih] bar1wedge sticky-cleared at-arm lnksta d081->1081 devsta 0000->0000 secsta 2000->0000 (w1c written c000/0000/2000) — …
f8 [  23263ms] [pcih] bar1wedge sticky-cleared post-enum rp=0:1.0 secsta=2000->0000 lnksta=1081->1081 relatch=secsta:2000 lnksta:0000 at-arm=secsta:0000 lnksta:1081 (w1c written 2000/0000) — …
```

Flight 9's `at-arm` and `post-enum` lines are byte-identical in every field.

Both storms crossed the tripwire and the first sample is the same on both legs:

```
f8 [ 331272ms] [pcih] wedge-sample n=1 first=1 aperture=wc lnksta=1081 d_lnksta=0000 (2.5GT/s x8 training=0 bwmgmt=0 autobw=0) lnkctl=0040 lnkctl0=0040 aspm=off lnkdis=0 retrain=0 devsta=0000 d_devsta=0000 secsta=2000 d_secsta=2000 devctl2=0000 cto=50us-50ms(default) dis=0 aer=n uesta=00000000 cesta=00000000
f9 [  95288ms] [pcih] wedge-sample n=1 first=1 aperture=uc lnksta=1081 d_lnksta=0000 (2.5GT/s x8 training=0 bwmgmt=0 autobw=0) lnkctl=0040 lnkctl0=0040 aspm=off lnkdis=0 retrain=0 devsta=0000 d_devsta=0000 secsta=2000 d_secsta=2000 devctl2=0000 cto=50us-50ms(default) dis=0 aer=n uesta=00000000 cesta=00000000
```

`[wcser] PASS OVERDUE holder=c1 … blit_inflight=1` fired **16 times on flight 8 and 12 times on
flight 9**, one per second, and `d_lnksta=0000 d_devsta=0000 secsta=2000 d_secsta=2000` on all 28.
`[pcih] rp-at-wedge` printed 17 / 13 times and `[pcih] rp-boot` once on each: §11.3's presence
controls are met.

### The outcomes §11.3 pre-registered, and which fired

| §11.3 row | fired? | reading |
| --- | --- | --- |
| `aperture=uc` and **no** PASS OVERDUE in a full storm → W4 CONVICTED | **no** | the tripwire fired 12 times under UC |
| **`aperture=uc` and the wedge happens anyway → W4 EXONERATED** | **YES** | the aperture's memory type leaves the ladder; W5 (credits / the GPU window path) becomes the head of the ladder with nothing above it |
| `lnkdis=1` at any crossing → STOP EVERYTHING | no | `lnkdis=0` on 28 of 28 samples |
| `d_lnksta=c000` at `n=1` → W2 re-opens | no | `d_lnksta=0000` |
| **`d_lnksta=0000` across every crossing → W2's shut-out confirmed** | **YES** | on both legs, on an instrument that can tell the instant from the boot |
| **`relatch=secsta:2000` → enumeration DOES latch Received Master Abort** | **YES** | on both legs. §1.3's premise is measured for the first time; boots 8/9/11's `secsta=2000` is explained without the wedge and **W7's reading dies on evidence** |
| `relatch=secsta:0000` | no | — |
| `relatch=lnksta:c000` | no | `relatch=… lnksta:0000` |
| `sticky-cleared post-enum` absent with the arm line present | no | present on both |
| `secsta=…->2000` after value nonzero | no | `2000->0000` on both |
| `d_secsta=0000` at `n=1` → W7 DEAD | no | — |
| **`d_secsta=2000` at `n=1` → the latch moved after `pci::init`** | **YES** | see finding 4: the named alternative is LIVE on these boots, so this does not yet reach the wedge |
| `dis=1` in the `cto` line → W6 convicted | no | `dis=0` |
| **`dis=0` with a `value=` in class A or B** | **partly** | `value=50us-50ms(default)` — the spec default, `devcap2=00000000 ranges=0`, so the port advertises no optional class; the prober is bounded at 50 ms, which is §3.2's cheap case |
| `v2=0` / `cto rp UNREADABLE` | no | `capver=2 v2=1` — W6 is answerable on this machine |
| `:: BAR1WEDGE:` absent with `bar1wedge` in the banner | no | present on both |

**The pre-registered absence controls both FAILED to discriminate, and that is finding 3.**
`:: x86 bar1exp: UC arm ARMED` is absent on the UC leg (0 hits) and `:: x86 fb-wc:` is absent on the
WC leg (0 hits) — neither alternative reached the wire on either boot. `fb-wc` "present" is the
BPACE stage stamp `:: BPACE: fb-wc t=0ms d=0ms ::`, which prints on flight 7 as well and is not the
framebuffer's memory type. The arm is nonetheless proven, by the line in correction 2.

### The machine survived, and the UC boot cost what the ladder predicted

Both storms ran to completion, presents continued, no `PANIC`, no `REHOMED`, no `DEAD c<n>`. On the
UC leg the price is measurable on the present path rather than on PCIe:

| | flight 8 (WC) | flight 9 (UC) |
| --- | --- | --- |
| Shell `maxpresent_us` | 11630 | **69120** |
| Shell `longpres` | 0 | **20** |
| `[schedx86] depth … inflight>0` | **0 of 65 samples** | **9 of 21 samples, peak `inflight=59`** |

```
f9 [  91667ms] [schedx86] depth sent=447 recv=388 inflight=59 (render core 1) fold=61
f9 [ 139235ms] [schedx86] depth sent=935 recv=926 inflight=9 (render core 4) fold=376
f9 [ 121636ms] [wc-h] rollup win=2 scope=window emit=8 … torn=196 stalls=0 longpres=20 … maxpresent_us=69120 … -> AT-RISK
```

Peter on flight 9: *"it seemed even more unstable than the last boot. i noted a few of the vugs had
locked up and the machine kept freezing"*. **The wire agrees.** 59 render jobs sent and not returned
at one sample, 20 presents over the stall bound on the shell, and a worst-case present of 69 ms —
four frames — against 11.6 ms on the WC control. Flight 8 never had a single outstanding render job
across 65 samples. This is the UC arm's cost and it is row A13.

---

## 6. The tear table (A5)

Last `scope=window` rollup per window, both boots. `birth` is the rollup's own
timestamp minus its `age_ms`; the storm was typed at 324427 ms (f8) and 65472 ms (f9), so a window
born after that is one of the six vugs `STORM: launched 6/6 vugs` started.

| flight 8 | birth ms | presents | torn | | flight 9 | birth ms | presents | torn |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| win=1 Console | 22899 | 57 | 0 | | win=1 Console | 20483 | 487 | 94 |
| **win=2 Shell** | 23317 | 7437 | **602** | | **win=2 Shell** | 21439 | 879 | **196** |
| win=3 STAT | 38316 | 6199 | 2 | | win=3 STAT | 36426 | 990 | **935** |
| win=5 vug | 324447 | 438 | 0 | | win=6 vug | 65572 | 230 | 0 |
| win=6 vug | 324490 | 442 | 0 | | win=7 vug | 65655 | 331 | 6 |
| win=7 vug | 324509 | 635 | 3 | | win=8 vug | 65665 | 144 | **127** |
| win=8 vug | 324513 | 682 | 0 | | win=9 vug | 65680 | 483 | 9 |
| win=10 vug | 324535 | 4 | 0 | | win=10 vug | 65696 | 245 | 2 |

Peter on flight 8: *"there was tearing in vug and the mouse movement was super choppy"*.

**The counter and the eye disagree on the vug windows of flight 8**: 3 torn presents in 2201 vug
presents, while Peter watched them tear. On the shell the counter and the eye agree (602 of 7437).
On flight 9 the vugs do tear on the counter (127 on win=8) and `STAT` tears on 935 of 990 presents —
94 %, which is what a UC aperture at ~6.8× does to a bracketed present. **Three readings are open
and this flight does not choose between them:** (a) "vug" in Peter's sentence names the desktop
region rather than the six storm windows; (b) the bracket is right and those windows genuinely did
not tear on flight 8; (c) the tear class the eye sees on a vug window is not the class
`beamcross` counts. A5 stays open with that named.

---

## 7. The four new findings

### Finding 1 — R8 is unrunnable on this panel by exactly one row, and it is the CAP, not the engine

```
dst_pitch_bytes=16384 dst_rows=65 dst_span_bytes=1064960 dst_bytes=1064960  max=1048576
```

`gen7.rs` allocates `dst_rows = R8_RECT_H + 1 = 65` (one deliberate slack row, so an inclusive
`X2/Y2` lands inside the surface and is counted as spill) at the panel's own pitch, and caps the
destination at `R8_DST_MAX_BYTES = 1024 * 1024`. On this panel `stride_px=4096` and `bpp=4`, so the
pitch is **16384 B**, 65 rows is **1064960 B**, and the surface is over the cap by **16384 B —
exactly the one slack row**. A 64-row surface would be 1048576 B, the cap to the byte.

The register's R8 row states the panel's pitch as **7 680 B**; the metal says 16384
(`KDHEAD: gop … pitch=16384`, and `r8 geometry … dst_pitch_bytes=16384`). The cap was sized against
the wrong pitch, so **R8 could never have run on this machine** and the flight bought a refusal
instead of the experiment. The wire already names the repair:
`next=STOP-destination-surface-over-the-stated-ceiling-raise-R8_DST_MAX_BYTES-deliberately-or-shrink-the-rect`.
Row B111.

### Finding 2 — Quarry was never compiled into either flight image

Peter: *"there is no quarry"*.

```
[  38665ms] :: APPQUIT: app=quarry not compiled (UNAOS_QUARRY unset) — no window to quit :: SKIP ::
[ 288465ms] :: [midden] cmd="quarry" -> TerminalError len=44 ::
```

`UNAOS_QUARRY` is absent from both MANIFEST knob lines. The kernel says so at 38 s of boot, on the
wire, in one line. **This is not a board fact and not a reachability fact**: QUARRYX86 and SMALLFIX3
both landed the x86 Finder this session, and the flight line simply did not arm it, so playbook item
9 pre-registered an observation the build could not produce. Row A10. The class is the one the
queue's own STATE line names three times — a knob is wired in three places and the flight line is a
fourth — and the flight-line review is what has no gate.

### Finding 3 — the PHASE31ROOT witness cannot reach this laptop's cable, on either arm

`set_framebuffer_wc` ends in exactly one of two `serial_println!`s: `:: x86 fb-wc: retyped N leaf(s)
WC (PAT PA4) …` on the default build, `:: x86 bar1exp: UC arm ARMED — retyped N leaf(s) UC (PAT PA3)
…` on the `bar1exp-uc` build. Measured over the three captures:

| | flight 7 | flight 8 | flight 9 |
| --- | --- | --- | --- |
| `:: x86 fb-wc:` | **1** | 0 | 0 |
| `:: x86 bar1exp:` | 0 | 0 | 0 |
| `:: BPACE: fb-wc ` | 5 | 2 | 2 |

The BPACE stamps sit *inside* the same `FB_WC_DONE` one-shot latch as the two witness lines
(`memory.rs:3649-3654`), so their presence proves the function ran and retyped on both boots. What
changed between flight 7 and flight 8 is the CALL SITE: the retype moved into `main.rs:112`, ahead
of `fbcon::init`, and `main.rs`'s own comment says the line "reaches serial/FTDI". **On this laptop
there is no 16550 and the FTDI mirror is not up at `main.rs:112`** (`BPACE: ftdi-up t=23567ms`,
against a retype at `t≈0ms`), so the line is emitted into nothing and neither arm's witness exists on
any rMBP wire.

Two consequences, and the second is the general one:

* **§11.3's absence controls for this flight were unscorable by construction**, and the
  `bar1exp-uc` banner-cert row seeded on `:: x86 bar1exp: UC arm ARMED` (queue STATE: "1 lean /
  1 flight / 0 on image B; banner-cert 36/36") certified the **string in the ELF** and proved
  nothing about a wire. That is exactly the pairing LAWS §5 keeps apart: an instrument's presence is
  proven in the artifact, and an absence is evidence only if the producing path ran. Here the path
  ran and the line was still lost.
* The arm IS provable, on a line nobody pre-registered:
  `:: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::` (WC) versus
  `uc=128 … wc-kept=0` (UC). That is the code's own documented UC signature — under `bar1exp-uc`
  the PAT bit ends up clear, so `FB_WC_LO/HI` records no hull, `leaf_is_fb_wc` is constant-false and
  `map_mmio_window` keeps nothing WC. **The BAR1 aperture was UC on flight 9 and WC on flight 8, and
  this is the measurement that says so.** `bw_aperture()` — the `aperture=uc|wc` field on every
  BAR1WEDGE and wedge-sample line — is `cfg!(feature = "bar1exp-uc")`, a compile-time constant: it
  reports which build flew, never what the page tables carry, and must not be cited as the latter.

Row B112.

### Finding 4 — the wifi bus sweep runs AFTER the post-enum sticky clear, so §11.3's named alternative is live

§11.3 dismisses one alternative for a `d_secsta=2000` at `n=1` — "a `wifi`-armed boot's own census,
`wifi/bus.rs:125` — a knob no A1 flight row asks for, `grep -c UNAOS_WIFI docs/dev/OS/rmbp-queue.md
docs/dev/OS/rmbp-ledger.md` = 0/0".

**Both flight images carry `UNAOS_WIFI=1 UNAOS_WIFI2=1`** (MANIFEST), `wifi::bus::census()` at
`wifi/bus.rs:125` is a full `for bus in 0u16..256` × 32-slot config sweep, and on flight 8 it ran at
**23516 ms — 253 ms AFTER the post-enum sticky clear at 23263 ms**:

```
[  23263ms] [pcih] bar1wedge sticky-cleared post-enum rp=0:1.0 secsta=2000->0000 …
[  23516ms] :: wifi: brcm net function 03:00.0 device=0x16a3 subclass=0x00 (Ethernet) — NOT the radio, skipped ::
[  23516ms] :: wifi: radio 04:00.0 device=0x4331 expected=0x4331 MATCH …
```

A config read to an absent function on bus 1 — the bus below the root port under test
(`[pcih] ep bdf=1:0.0`) — master-aborts and sets that bridge's Secondary Status. So the alternative
§11.3 dismissed is **not excluded on these two boots**, and `d_secsta=2000` at `n=1` cannot be
attributed to the wedge from this flight. The EHCI walk, the alternative §11.4 was written for, IS
excluded here by time: EHCI-HID finished at 22303 ms, before the post-enum clear.

The dismissal's shape is the error, not its conclusion: it asked "does a queue file name this knob"
when the decision needed "does the flight line arm it". The knob line in the MANIFEST is the
population; two doc files are not. Row B113.

---

## 8. What the operator saw, measured

Peter, flight 8 and flight 9: *"the mouse cursor was flickering a lot too in both boot"*, and
*"the mouse movement was super choppy"* under the storm.

The pointer is **not** starved at the driver. `[deadman] hid=` (HID reports in the last second)
during flight 8's storm averages **53.4 reports/s over the 14 one-second windows in which the
pointer was moving**, peak 115 — higher than the 8.9/s pre-storm average, because he was moving it
more. Nothing is lost there. What moves is the repaint:

```
[ 202145ms] [cursor6] rollup scope=live present_over=3  masked=243  repaired=64   desktop_over=0 mismatch=0 uncover_lost=1/175 -> REPAIRED
[ 394355ms] [cursor6] rollup scope=live present_over=12 masked=1105 repaired=2634 desktop_over=0 mismatch=0 uncover_lost=4/730 -> REPAIRED
```

**2570 cursor repairs during the storm** (64 → 2634), `present_over` 3 → 12, and
**`uncover_lost` 1/175 → 4/730 — four uncovers the compositor never repaired**, i.e. four cursor
trails left on the glass. That is the flicker, and the chop is the same window's
`maxpresent_us=11630` with `presspread=50` on the shell. S30 does not cover it: S30 is the xHCI
pointer path's missing press accounting, and this pointer is on EHCI, which HAS that accounting
(`:: PTR: [1] press seen=1 delivered=1 recovered=0 (down edge) == witness ::`). Row A11.

---

## 9. Reading commands

```sh
L=~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log
awk 'index($0,"=== SQUAWK MARK")' "$L"                       # the boot boundaries
awk 'NR>=10 && NR<=6654' "$L" > f8.log ; awk 'NR>6654' "$L" > f9.log
awk 'index($0,"[pcih] wedge-sample n=")' f8.log | wc -l      # 16 — the naive pattern over-counts by 2
awk 'index($0,"[wc-h] rollup")' f9.log                       # the tear table
awk 'index($0,"BPACE:")' f8.log                              # the boot-pace ledger
awk 'index($0,"mmio-map")' f8.log | sed 's/^\[[^]]*\] //' | sort   # diff against f9's for the UC arm
```
