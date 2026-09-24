# FLIGHT 13 — image 6 (rmbp12flight13b, hw-rmbp@63009d35), 2026-09-24, a cafe

Capture: `f13-boot1.log` beside this file (6163 lines, 0 → 205.6 s; the seat's slice of
`~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` from byte 6313953). Playbook: the image-6
`PLAYBOOK-rmbp.md` (copy at the seat's `PLAYBOOK-f13-image6.md`). Image: gate13 + gate13b
(`docs/dev/OS/rmbp-queue.md` STATE 2026-09-24), banner-cert ok=48, nightly 2026-09-23.

## Peter, verbatim (the glass)

- "i sure did not miss that test "tone" sounded like i was dragging the laptop across sandpaper"
- "mouse is hypersensitive to the point of unusable so i shut the machine off after trying the screenshot key combo"

The sitting ended there, at about 205 s: no `adduser`, no Log Out, no login screen, no first login.
The root set-password alert never appeared (§1) — Peter did not report it because there was nothing
to report.

## 1. THE FINDING THAT VOIDED THE SITTING — the users store never loads on the rMBP, so LOGIN14 never ran (mechanism established)

Wire: ONE `[login]` line in 205 s — `[login] boot session=root desktop=true screen=closed` at 7108 ms —
and ZERO `[users]` lines, ZERO `[rand]` lines. Under QEMU (gate13 `test-login`, 53/53) the same image
prints `[rand] source=jitter …`, `[users] load volume=el0-fat(rw) src=none users=0 (fresh store)`,
`[login] root password unset row=created -> set-password screen`, `[login] set-password screen open
user=root …` in that order.

Mechanism, read in the source at 63009d35:

- `fs/users.rs::service()` (the LOGIN M1 call, three sites in `main.rs`, every service pass) returns at
  its second guard: `if crate::drivers::block::info().is_none() { return; }`. Everything LOGIN14 does at
  boot — `try_load()`, then `root_credential_ignition()` (users.rs:1240) — is below that guard.
- `block::info()` reads the GLOBAL `BLOCK_DEVICE`. On the rMBP nothing sets it: the wire says so twice,
  `[7074ms] :: SDHCBLK: registered internal SD card as block handle Sdhc — … (global BLOCK_DEVICE
  untouched) ::` and `[7075ms] :: AHCI: registered port=0 as registry index 0 — … READ-ONLY (global
  BLOCK_DEVICE untouched, installer not told) ::`. Under QEMU the test disk sets the global, so the guard
  opens and the lane is green. The guard measures the QEMU disk's shape, not "a store can be mounted".
- `fs::fat::mount()` (what `try_load` uses) already falls back to the internal card on
  `x86_64 + sdhcblk` ("registered, else the card in the machine's internal SD slot"). So the load would
  succeed if asked; it is never asked. The login SCREEN's own path (`video/login.rs:930`
  `users::load_once()`) mounts directly, which is why flight 12's screen could open with a store while
  the boot-time chain never ran — flight 12's wire (`f12-boot1.log`) also has ZERO `[users]` lines.
- `[rand]` is absent for the same reason: its first use is inside the load path.

Consequences: no root row, no root password, no alert, no `[rand]`; LOGINORDER's battery hold read
`held 0ms … settled=true` trivially (`loginst` is not armed on the metal and no chain ran). LOGINFONT
is unscored (nothing painted). The five-step sitting of `multiuser.md` §9.2 was unreachable on this
image by construction; it was never a keyboard, trackpad or Peter problem.

The fix is one guard: readiness for the users service is "the store's volume mounts"
(`fat::mount().is_ok()`, or `block::info().or(block::sdhc_info())` plus the AHCI registry), not the global
slot. The go-red is the guard as it stands with the global left `None`; the QEMU lanes cannot show it
(their disk sets the global) — a control lane that registers the disk WITHOUT the global, or a unit on
the guard, is owed with the fix. Owed arc: **LOGIN15** (below).

## 2. Trackpad: the frames route (TPFRAME proven) and the scale is 1:1 sensor units (unusable) — mechanism known

- TPFRAME (B197): `[tp] mode … try=hid1.11-intf … latched=no` then `… try=legacy-index0 set=true
  readback_ok=true latched=yes`, `[tp] mt route=vendor latched=yes` at 5836 ms; 23 `[tp] mt fingers=1 …`
  witnesses from 93800 ms (Peter's first touch) with non-zero `dx`/`dy`; `[tp] ids=02:0,44:0,other:572
  sizes=2/58`. The pad is alive; flight 12's dead pad is fixed. No `mode-mismatch` line.
- The scale: `mt_step` (ehci/mod.rs:17481) emits `dx = x - px` in RAW sensor units clamped to
  `TP_MT_MAX_STEP` (max seen |dx|=88, |dy|=60 per witness frame, ~128 frames/s), and the router takes them
  as pointer pixels. The witnesses read `x0` from -3503 to 2051 and `y0` from 414 to 4923 over one finger's
  travel — the Wellspring absolute range on this model is about ten thousand units across the pad's
  width, against 1440 logical pixels (2880 at scale 2). A one-centimetre stroke is ~1000 units = ~1000
  px. That is "hypersensitive to the point of unusable". `usb_xhci.md` §39.8 said it in advance: "1:1
  sensor units is the retired sub-knob's scale, not a measurement."
- Owed arc: **TPSCALE** — a divisor on the vendor deltas (start at 1/8; ~125 units per logical pixel is
  a first guess, measured on the glass), applied in `mt_step` or at the route, with the witness line
  carrying both raw and scaled deltas; Y sign still unverified (Peter did not say inverted; he said
  unusable). A knob is not needed; a constant with the witness is.

## 3. Sound: the louder sine sounds like sandpaper — mechanism NOT established

- Amplitude 12288 (32eb7d5d, this image). `:: HDA-TONE: lpib_advanced=1 walked=1 wraps=1 bcis=0
  tag_ok=1 fifo_ready=1 run_ms=1200 members=2 -> PASS ::` at 8558 ms; `[hda] tone stream=0 lpib=0 ->
  38400 … consumed=230400 rate_bps=192000` — the DMA ran at exactly 48 kHz × 16-bit × 2. `[hda] amp
  member=0 dac=0x04 raw=0x0000 mute=0 gain=0 pin=0x0b … out_amp=0 pinctl=0x40 out_en=1 hp_en=0
  fmt_conv=0x0011 fmt_want=0x0011 fmt_match=1` (member 1: dac 0x03, pin 0x0a, same) — no codec
  output-amp steps on this path (`moderate_gain` returned 0 because `steps==0`), so nothing in the codec
  attenuates or clips the sample.
- What "sandpaper" (broadband noise, not a tone) can be, none measured: (a) the speaker path's class-D
  amplifier (HDAAMP defect 2: a GPIO, not an EAPD pin) driven past its input range by a sample three
  times louder than every prior flight — flight 12 heard the 4096 tone as PASS-with-silence or faint,
  so distortion at that level was inaudible; (b) a sample-layout error (mono vs stereo interleave,
  endianness) that a sine at -18 dBFS hid; (c) the two-entry BDL seam (each entry 24000 frames = 220
  whole cycles of 440 Hz, so the seam is phase-continuous — unlikely). The witness cannot separate them.
- Owed arc: **HDATONE2** — a knob `UNAOS_HDA_AMP=<n>` (default 4096) so a flight can sweep 4096/8192/12288
  without a rebuild, a unit test that checks the PCM buffer's first period against a reference
  (`sin_q15` and the interleave), and a wire line naming the amplitude used. DECISION for Peter: boot 14
  at 4096 (the level every prior flight flew) or 12288 with the sweep.

## 4. Screenshot as root refused — DECISION for Peter (ROOTSHOT)

`[197592ms] :: PRTSCR: [prtscr] chord=cmd-shift-3 (GUI+Shift+digit) down on EHCI -> capture armed
action=screenshot ::` then `:: PRTSCR: no user session (reason=no-session) — a capture belongs to a
user's own Desktop folder (theme=crispy) and there is none; NOTHING WRITTEN`. The chord routes (KEYMAP
proven on EHCI); the refusal is R54's rule written before R63 named root. Question: does root get a
Desktop (`/home/root/Desktop`, created like a user's) or does the refusal stand for root? Not a bug
until Peter says which.

## 5. KVBLANK3 (B192): the rung-1 falsifier fired; the vector path works once — a KVBLANK4 read is owed

- Rung 1 at `kepler::init`: `:: kepler: vblank pmc-arm bit=26 reg=INTR_MASK_HOST en_host=00000000
  mask_entry=FFBFB3F1 mask_armed=FFBFB3F1 intr_entry=00000000 intr_or=00000000 line_or=00000001
  pdisplay_seen=0/43358 window_ms=50 deliver=none reason=source-probe-only restored=FFBFB3F1
  verdict=clean ::`. Read against `KEPLER-METAL-LOG.md`: bit 26 was ALREADY set at entry (the write
  changed nothing — `mask_entry == mask_armed`), `pdisplay_seen=0` (the head enable, not the PMC mask,
  gates the source), and `line_or=00000001` is the doc's own falsifier: "a non-zero value means the line
  asserted with `INTR_ENABLE_HOST` at 0, and then this doc's reading of `0x140` is wrong."
- R3: `:: kepler: vblank-intr vector close head=0 irq=1 vbl_delta=62 rearms=1 wire=1 vector=0x44 storm=0
  … verdict=clean mode=irq deliver=msi` — the function sent ONE message and did not send again: the
  documented result (per-message MSI re-arm NOT-IN-TREE).
- Period: `:: kepler: vblank head=0 count=11740 period_us=16669 jitter_us=547 raster_at_irq=1064 vt=1852
  mode=irq vbwaits=220 … seen=5496 tight=979 period_src=tight ::` — WORKING by the doc's window
  (16600-16700, jitter under 2000, `period_src=tight`); `seen/count` = 0.47 (the doc predicted ~0.4).
- Owed arc: **KVBLANK4** — re-read `0x140` vs `0x640` against `line_or=1 at en_host=0`, and the
  per-message MSI re-arm (irq=1 per ~62 vblanks is the number to move).

## 6. IOAPIC2 (B191): the PIRQ router's first metal reading — the queue's question answered

`[5837ms] [ioapic] pirq bdf=0:31.0 id=8086:1e57 family=pch7 rcba=0xfed1c000 pirqa=0x80 pirqb=0x80
pirqc=0x80 pirqd=0x80 pirqe=0x80 pirqf=0x80 pirqg=0x80 pirqh=0x80 fn=0:29.0 pin=INTA d29ir=0x3236 ->
pirq=G REFUSED reason=pirq-disabled` and `[ioapic] route bdf=0:29.0 pin=INTA line=0 -> REFUSED
reason=pirq-disabled`. The LPC bridge is in `pch7`'s range (predicted); Apple's firmware leaves EVERY
PIRQ_ROUT at 0x80 (IRQEN clear) and D29IR routes the EHCI's INTA to PIRQG. The refusal names its reason
(M1 proven on the metal). Next rung, **IOAPIC3**, WRITES a chipset register (clear bit 7 of PIRQG_ROUT
and choose the IRQ, or route I/O APIC input 16+6 directly) — Peter's go is required for a chipset
write on the rMBP.

## 7. Flown green (record; the arcs' rows move to flown)

- **GLASSFIX3 (B203)**: `:: GLASSFIX2: … cascade overlaps=0 worst=win0-over-win0:0rows minted=4/4
  pinned=3/3 n=10 control=1 expect_overlaps=0 tb=3 …` — flights 11 and 12 read overlaps=8 and 14 FAIL.
- **BOOTSLOW (B201)**: `[7118ms] SDHCBLK: FAT mounted` → `[7175ms] [vfs] root-pass BOUND source=sdhc` →
  `[7185ms] X86BIND: root=sdhc:/kernel.elf … -> PASS` — 57 ms where flight 12 measured 25 s; the HOLD
  lines name the probes deferred (`bt-campaign`, `witness-fixture`, HDA `deferred-start at=7213ms`).
- **TPFRAME (B197)**: §2 — the frames route; the scale is TPSCALE.
- **PTRLEAK (B193)**: `[cursor] armed x=961 y=617` at 25055 ms is preceded by `[7129ms] [deadman] up=7
  hid=1 …` (a HID report existed); `[ptrdead] backlog whole=true nodrop=true order=true … fpop12=0
  fpop3=0 … -> PASS`. Not flight 12's leak shape.
- **The FAIL census**: 3 lines (`SHOTMENU`, `APPQUIT`, `APPPIN` — voided/known on flights 11 and 12)
  against flight 12's 14. `MENUBATT` did NOT fail this boot (flight 12: FAIL); `[menubar] first-paint
  at=6699 after_enable_ms=0 … crystal=drawn` once.
- **LOGINORDER (B206)**: `[clickroute] battery held 0ms for the loginst chain settled=true` — true but
  vacuous on this image (`loginst` off, no chain; §1).
- **R59**: `SDHCPOST … sdhc=rw reason=none`, FAT mounted READ-WRITE.
- **BOOTCLOCK**: `firmware->loader=13254ms loader-read=773ms loader-jump=20ms`.

## 8. No reading (the fixture did not fire, or the sitting did not reach it)

MENULOCK (B200: no `holder=` line — no refused paint this boot), WCDMEM (B199: no rollup line seen),
LFNMV (B202), TERMSEL2 (B196), LOGINFONT (B204), `adduser`/Log Out/first login (§9.2 lines 7-21),
DOCKVAC, VUGART, `storm`, the close box and `screenshot` verb (B212). USBNET: not armed, dongle at home.
`MENUBATT`'s absence from the FAIL census is a reading; its PASS line was not checked.

## 9. Owed to the queue from this flight

LOGIN15 (§1, the image-voiding bug, first), TPSCALE (§2), HDATONE2 (§3) + Peter's amplitude decision,
ROOTSHOT (§4, decision), KVBLANK4 (§5), IOAPIC3 (§6, Peter's go). Flown-green rows: GLASSFIX3, BOOTSLOW,
TPFRAME, PTRLEAK (§7). SOCK-2/3 host-resolver flake registered before the flight (FIXTURE_FLAKES).
