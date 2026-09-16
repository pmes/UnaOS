# rmbp12 flight 8 — the first boot of the 45-fold tree (image B), then flight 9 (image A)

Twelve days and 45 folds since flight 7. Two cards' worth of image, ONE card: flight 8 is image B,
flight 9 is image A (the same tree, one knob added). Both are ATTENDED. Board is dark, card is in
the rMBP's SD slot, FTDI on the host. Boot with ⌥ held and pick the card.

Watch on the wire, in this order, and please say what differs. Every line below is quoted from the
kernel's own strings, so `awk 'index($0,"<tag>")'` over the capture finds it — never bare `grep`.

## WATCH LIST — flight 8 (image B: UnaOS-rmbp-esp-rmbp12flight8-20260916T0515Z-bf299a3)

kernel.elf sha256 `f4b2ca01bb1956caa26af4b83fe6da514bb45a3f310f51b0e968ea51712f6d22`, 3802800 bytes, built from
hw-rmbp@bf299a37; staged at `~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp12flight8-20260916T0515Z-bf299a3/`.

1. **The banner names every knob.** `⚡ kernel features:` must carry `ftdirx beam ahci bar1wedge
   ivb3d-r8 nvidia-kepler-kfbind nvidia-kepler-kdhead` besides the standing flight-7 set. Each is
   proven in the artifact by `LC_ALL=C grep -a -o -F` (see CERT below), so a banner word with no
   wire line behind it is a finding, not a dark knob.
2. **The internal SSD is FOUND and LISTED, not bound.** `[ahci] bdf 0:31.2 8086:1e03 bar5=0x…`
   (BAR5, not the `bar0=0x3080` the census prints), then `:: AHCI: port=<p> model="APPLE SSD SM…"
   sectors=<n> lba48=1 ::`, `:: AHCI: port=<p> sector0 sig=0xaa55 kind=GPT ::`,
   `:: AHCI: selfcheck port=<p> identify=ok sector0=GPT -> PASS ::`. Then one
   `[bootdisk] volume source=ahci port=<p> vol=EFI … volumes_by_content=1 ::` (Catalina's ESP,
   read off her GPT by content) and `[bootdisk] ahci census: sata_sources=1 with_fat_volume=1
   carrying_AHCIBOOT.TXT=0 ::`. READ-ONLY by construction: the image carries no ATA write opcode.
3. **Root binds BY CONTENT to the card.** `:: X86BIND: root=<vol>:/kernel.elf serial=0x<8hex>
   by=content … -> PASS ::` and `/` `/boot` `/apps` on one volume id; Catalina's EFI volume under
   `/volumes/`, listed, never `/`. `root=- … -> FAIL` is the red this tree just fixed (STORWAIT) —
   quote the `:: [fatverb] storage settle: waited=<n>ms settled=<found|usb|ceiling> handles=… ::` line
   with it (it follows the `storage witness` line; `settled=found` = the kernel's volume was found).
4. **TYPING OVER THE WIRE — the first time on this laptop.** From the host, once the shell prompt
   is up:
       printf 'help\r' > ~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.in
   Expect `:: FTDIRX: first byte rx=1 byte=0x68 'h' idle=0 ::`, then the shell's help text. Then
   `date`, then `storm` (item 8). The built-in keyboard keeps working alongside.
5. **The keyboard does not die after N keys.** Type a long line on the laptop's own keyboard
   (40+ characters). Every character must echo; `:: KBDWIT:` lines saying `NO-COMPLETIONS` or
   `SILENCE-CUT-BY-HALT` are the old defect (flight 7 lost the keyboard) and are a red here.
6. **The gmux gate.** `:: igpu-dpy: pre-switch state DDC=0x02 … SW_EXT=0x01 SW_EXT_ST=0x21
   DISP=0x03 EXT=0x21 … gate=ACCEPT ::` — `SW_EXT=0x01` with `gate=ACCEPT` is the fix, not a
   contradiction. Then `:: igpu-dpy: rung=00 name=census ok=1` and the ladder, or the honest
   `:: igpu: [GMUX] REFUSED: pre-switch-not-accepted` → `gmux=UNTOUCHED`. No `igpu-dpy` lines at
   all = G3 failing upstream (say so).
7. **Ivy Bridge R8 — a blit into a CPU-readable surface.** Read `:: gen7: r7 verdict=` FIRST.
   Only `r7-blit-verified` lets R8 run; otherwise `r8-gated-on-r7` and R8 says nothing (correct).
   Pass: `:: gen7: r8 verdict=r8-fb-blit-verified … rect_match=4096/4096 spill=0 sentinel_hit=1
   settled=1`. `r8-fb-blit-verified-spill` = engine works, geometry wrong. `preblit` must show
   `rect_match=0/4096 seed_ok=1`.
8. **Kepler, four rungs on one boot, zero writes on three of them.**
   - `:: BEAMX86: head=<h> vtotal=<n> samples=<n> vblank_delta=<n> -> ARMED ::` (A5's beam
     source). `-> NONE` means no live head was found and everything below withholds.
   - `:: KDHEAD: bracket source=beamx86-census live=[…] ::`, then per block
     `heads_distinct=<k>/4`, `:: KDHEAD: gop w= h= …`, and the `end rung=KD14 … separated=<n>`
     line. `-> AGREE` on a live head pins a stride; `-> DISAGREE` names `mismatch=`;
     `separated=0` confirms s4. `bracket source=absent` means BEAM did not arm (item above).
   - `:: KFBIND: … verdict base=<DERIVED-WINS|LEGACY-WINS|BOTH-ANSWER|NEITHER-ANSWERS|
     LEGACY-ONLY|VOID-BRACKET> ib_get <pre>-><post> … -> <FETCHED|STILL-DARK under …> ::`.
     `-> FETCHED` would be the first movement of K-GPU-3's wall since July. CE-R1 sweeps the
     same PTOP words on this boot and must agree dword-for-dword.
   - BAR1WEDGE at kepler init: `:: BAR1WEDGE: rung=first-stall …`, `[pcih] bar1wedge cto rp …`,
     `[pcih] bar1wedge sticky-cleared at-arm …`; at the tail of pci init: `[pcih] bar1wedge
     sticky-cleared post-enum rp=… secsta=…->… relatch=secsta:<0000|2000> …`. SCORE `relatch=`
     alone: `2000` = a bus walk latches Master Abort and flights 8/9/11's secsta is explained
     without the wedge; `0000` = nothing but the wedge is left to blame.
   Then type `storm` at the shell. This boot is the CONTROL: it is EXPECTED TO WEDGE. Read
   `[pcih] wedge-sample n=1` (the first stall) and `[wc-h] win=2 torn=<n>` (A5: was 111 on
   flight 7). A core dying mid-blit here is the result, not a failure — flight 9 is the experiment.
9. **The Finder lists volumes on x86.** Open Quarry: it must list `/`, `/boot`, `/apps` and the
   volumes (it listed NOTHING on x86 before). Double-click launches.
10. **Shut down from the shell.** Type `reboot`. The LAST THREE lines on the cable, in order:
    `[pwrreboot] reboot verb invoked — dispatching the platform mechanism`, `[pwrreboot] x86
    mechanism: FADT RESET_REG ladder (acpi_power::reboot)`, `[pwrreboot] ftdi flushed bytes=B
    transfers=T exhausted=0`. If it comes back up, `shutdown` (or Crystal → Shut Down): the last
    line before dark must be `[pwrshutoff] ftdi flushed bytes=B transfers=T exhausted=0`. A dark
    board with no such line is a red, not a known gap.

## WATCH LIST — flight 9 (image A: UnaOS-rmbp-esp-rmbp12flight9-20260916T0519Z-08e1ec9) — the same tree plus `UNAOS_BAR1EXP=uc`

kernel.elf sha256 `f8473c104fb7d0890e806802da0aee97b6b37e06b2bd8fcebb19160c853a018a`, 3802408 bytes, built from hw-rmbp@08e1ec90 (= bf299a37 + one
cert row, scripts only); staged at `~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp12flight9-20260916T0519Z-08e1ec9/`.

Same list; the differences: `bar1exp-uc` in the banner, `:: x86 bar1exp: UC arm ARMED` present
and `fb-wc` ABSENT. Type `storm`. If flight 8 wedged and this boot does NOT, A1's theory stands.
If flight 8 did not wedge, flight 9 proves only that the storm was weak. Everything is ~6.8x
slower under UC; that is expected.

## CARD LINES (the seat runs these; you plug in)

1. Cable is in and the capture is UP (seat, 05:21Z): squawk holds /dev/ttyUSB0 for session
   `rmbp12-flight8`, log `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log`; waker and media watch armed.
   If it ever has to be restarted, it is a HOST process:

       flatpak-spawn --host python3 ~/unaos-bench/tools/squawk-bench/squawk_bench.py watch --session rmbp12-flight8 --no-disks

2. Card: put the UNAOS-X86 card in the reader; the seat runs the flat copy of the staged tree (ESP files AND the data/ files at the card root, B43/ too),
   then sha-match of every file; `x86-preflight.sh rmbp12-flight8 <staged>` must print READY.
   Flight 9 = the same copy from the image-A tree over the same card after flight 8's capture is
   marked.

## CERT — how each knob was proven in the artifact (filled from the build)

Every knob on the line was proven in the artifact by `LC_ALL=C grep -a -o -F` (never `strings`), by
`scripts/banner-cert.sh` inside the `esp-x86` verb: `ok=35 missing/leak=0 unverifiable=0 noverdict=0`.
The seven knobs new since flight 7, each also measured on a build arming ONLY that knob (and 0 hits
on a build without it):

| knob | banner word | token in kernel.elf | hits (flight / lean) |
|---|---|---|---|
| UNAOS_FTDIRX=1 | ftdirx | `:: FTDIRX: first byte rx=` | 1 / 1 |
| UNAOS_AHCI=1 | ahci | `:: AHCI: port=` | 3 / 1 |
| UNAOS_BEAM=1 | beam | `:: BEAMX86: head=` | 2 / 2 |
| UNAOS_BAR1WEDGE=1 | bar1wedge | `:: BAR1WEDGE: rung=first-stall` | 1 / 1 |
| UNAOS_IVB3D_R8=1 | gen7r8 | `:: gen7: r8 begin rung=R8 wake=` | 1 / 1 |
| UNAOS_KEPLER_KFBIND=1 | nvidia-kepler-kfbind | `:: KFBIND: pbdma[` | 1 / 1 |
| UNAOS_KEPLER_KDHEAD=1 | nvidia-kepler-kdhead | `:: KDHEAD: end rung=KD14` | 1 / 1 |

Found while measuring: the standing `nvidia-kepler` row had been certified only on the rich flight
line; on a kepler-only build its token was absent. Re-seeded on the init path (kepler.rs:1340).

## KNOWN AND EXPECTED — not defects

* `hello.txt` exists in BOTH the ESP tree and the carried-forward data volume with different text; the
  card is flat so one copy wins (the ESP root's by convention). `cat hello.txt` shows which; note it.
* The card carries `B43/` (bcm4331 firmware, carried forward from flight 7, not build-produced) — the
  WIFI-armed boot needs it. `readme.txt`, `GROW.BIN`, `S8W.BIN`, `SCRATCH.BIN` are the data volume's
  fixtures from earlier flights, harmless.

* No `:: AHCIBOOT: … -> PASS ::` on metal: the marker file is a QEMU fixture; `carrying_AHCIBOOT.TXT=0`
  is the honest answer.
* `[pwrreboot] ring drained …` and the FADT `raw_witness` lines never reach the cable: they go to
  the 16550 this laptop lacks (that fact IS A3; BOOTFADT's boot-time line already carries it).
* `battery_moved=0/17`, `fw_evidence=blind`, `reclaim=held reason=no-invalidation-evidence` on the
  gen7 lines are deliberate blind instruments.
* `:: igpu-dpy: restore ext=SKIPPED (write-target port, no state read) ::` and EXTERNAL abstaining
  from the `gmux=` vote: the external mux may be LEFT ON IGD until power-cycle on the first boot
  that reaches G7.
* `-> STILL-DARK` on KFBIND is a result: it is the first capture that contains IB_GET at all.
* The storm wedging a core on flight 8 is the control's job.
* `storage settle: waited=<n>ms` may be several seconds on this laptop: the wait is real for the
  first time (the SD reader used to satisfy it at 0 ms); `settled=ceiling` with root still bound is fine.
* NOT on these images, by design: the seven SHUTRESTORE display/FIFO write rungs (one per flight),
  KFCTXBIND (after KF27 answers), KFUNWEDGE (sacrificial, flown alone), `UNAOS_AHCI_WRITE` (the
  installer's sitting; nothing on this image can write the SSD).
