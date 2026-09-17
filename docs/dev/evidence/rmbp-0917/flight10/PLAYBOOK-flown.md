# rmbp12 flight 10 — image 2b: the tree after today's 16 folds (freeze shortened, W5 instruments, pointer on the producer)

ONE image, ONE card, ATTENDED. Board dark, card in the rMBP's SD slot, FTDI on the host. Boot with ⌥ held and pick the card.
Every line below is quoted from the kernel's own strings and proven in the artifact by `LC_ALL=C grep -a -o -F`;
find it in the capture with `awk 'index($0,"<tag>")'`, never bare `grep`.

## THE RULE THAT CHANGED SINCE FLIGHT 9
**Nothing printed before ~350 ms was ever evidence on this laptop** (the boot-capture ring, 256 KiB drop-oldest on flights
8/9, was at capacity both times; every "absent" early line was evicted, not unreached — PHASE31WIT, B114). Ring is 1 MiB now
and says so late: `:: FTDI-CAP: replayed=<n> cap=<n> lost=<n> head_cut=<y|n> ::` then `:: FTDI-CAP: early-boot capture INTACT -- 0 byte(s) dropped (…)`
or `… TRUNCATED -- …`. Quote it first when scoring anything early.

## IMAGE — UnaOS-rmbp-esp-rmbp12flight10-20260916T2132Z-fa4dcf0
kernel.elf sha256 `2b0db1e530ea115ad15d8f2809681a11d5338b33cb21f174f134c14b2fd25549`, 3966448 bytes, built from hw-rmbp@fa4dcf0b; staged at `~/unaos-bench/flash/rmbp/UnaOS-rmbp-esp-rmbp12flight10-20260916T2132Z-fa4dcf0/`.
Replaces the earlier flight-10 tree (11ca67f1), never written to a card. Knob line (gate 7's image-2 line; `UNAOS_QUARRY=1`
is the addition since flight 9; the UC arm is NOT on this line):

    UNAOS_WC=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_KEPLER_CE=1 UNAOS_IVB=1 UNAOS_IVB3D=1 UNAOS_GMUX_IGD=1 UNAOS_WITNESS=1 UNAOS_WCG_PAYGO=1 UNAOS_LOGTS=1 UNAOS_WIFI=1 UNAOS_WIFI2=1 UNAOS_BT=1 UNAOS_BTC=1 UNAOS_SMC=1 UNAOS_SMCWALK=1 UNAOS_RTWIT=1 UNAOS_USBDEBUG=1 UNAOS_NOASPM=1 UNAOS_DEADMAN=1 UNAOS_WCDVALVE=1 UNAOS_FTDIRX=1 UNAOS_BEAM=1 UNAOS_AHCI=1 UNAOS_BAR1WEDGE=1 UNAOS_IVB3D_R8=1 UNAOS_KEPLER_KFBIND=1 UNAOS_KEPLER_KDHEAD=1 UNAOS_QUARRY=1 ./arroyo esp-x86

Gate 7 on fa4dcf0b: check plain strict rc=0, check tegra strict rc=0, test 120 rc=0, test-fat sf 300 rc=0, AHCI+installdemo test 120 rc=0, test-arm rc=0, test-usb2 sf 180 rc=0, esp-x86 rc=0 with banner-cert `ok=36 missing/leak=0 unverifiable=0 noverdict=0`; knoboff witness vs f8f8ce8c rc=1 = default code moved by design (STEALMS, PTRINSTALL2, RENDSTACK; control fires both arches); kernel8-test 124/126 = the Pi test's OWN flake (the same ERET-SCRUB pair is missing at the gate-6-green base 11ca67f1 on a quiet box: `[smpbal] steal 'eret-verdict' c2->c0`), not this tree

## WHAT IS NEW SINCE THE 11ca67f1 TREE (16 folds), each with its wire line
- **Freeze length (STEALMS).** A render core stuck inside a store into the Kepler window is stolen from after 1.5 s, not 4 s:
  `[wcser] GATE STOLEN from c<n> by c<m> after 15xx ms …` — never `after 4xxx ms`. The core is still lost (`REHOMED the render role`).
- **W5 instruments (W5I1 + W5I2), right beside every GATE STOLEN:**
  `:: W5: site=post-steal from=c<stealer> dead=c<n> held=<ms> aim=<hex> pmc_intr=<8x> pfifo_intr=<8x> flush=<8x> ep_pcists=<4x|none> pbus_intr=<8x> pri_fault=<8x>/<8x> fault_mask=<8x> bar1_fault=<8x>/<8x>/<8x>/<8x> ::`
  (or `… held=<ms> bar0=unmapped ::` on a probe-abort boot), then
  `:: W5: nmi core=c<n> taken=<y|n> rip=<hex> in_blit=<y|n|?> cs=… memcpy=<lo>..<hi> icr=ok ::`.
  **`taken=n` = the dead core is hardware-parked** (the store is in flight forever; only a reset path recovers it).
  `taken=y in_blit=y` = the store completes but the loop does not (software). `taken=y in_blit=n` = the core is elsewhere.
  Self-test lines at boot: `:: W5: nmi selftest spin … -> PASS ::` and `… idle … -> PASS`.
- **Pointer on the producer (PTRINSTALL + PTRINSTALL2).** The arrow is installed by the input service now, so it keeps tracking
  the pad while the render core is stuck. `[ptrinstall] installs=N reports=N lag_max_ms=<~1> coalesced=C drains=D folds=F`
  every 5 s beside `[schedx86] depth …`; `installs == reports` every sample; late `:: PTRINSTALL: installs=… ::`.
  The chop under storm should be GONE; the freeze (1.5 s) is not.
- **Stacks (RENDSTACK + U7XSTACK).** `:: STACK: render high=<n> of 32768 ::` (was 95 % of 16384); NO
  `:: STACK: task=u7x-launch overflow guard hit …` line (that one is a red now); a `TRAVERSED` is a named panic.
- **Cursor rollups on x86 (CURSOREMIT).** `[cursor11] compose-through … flicker_frames=<n>` (must be 0), `[wc-i] rollup … intrusions=<n> -> CLEAN|INTRUDED`
  (intermittent 0/2 on QEMU, B118), `[flick2] …`; a `-> FLICKER` verdict is forbidden.
- **WC arm witness late (FBWCWIT).** `:: x86 fb-wc: ARM=wc leaves=<n> range=<lo>..<hi> ::` after the console is up.
- **WINMENU (WINMENUFLAKE).** `:: WINMENU: … owner=<n> published=y waited=<ms> … app_box=true … :: PASS ::`; a `-> SKIP reason=menu-unpublished` is a finding.
- **Boot clock (BOOTCLOCK).** `:: BOOTCLOCK: firmware->loader=<ms> …` — the pre-kernel phase measured for the first time (A12).
- **Storage (STORWAIT2 + STORSLOT).** `:: [fatverb] storage settle: waited=<n>ms settled=<found|usb|ceiling> …`, `:: STORSLOT: claim slot=…`, `:: X86BIND: root=… by=content … -> PASS ::`.
- **Quarry (QUARRYDOCK + QUARRYX86-2).** `[dock] pins=…` names quarry; press the tile → `:: QUARRYDOOR: win=…`; `[quarry] DECLINE reason=create-failed` is a red.
- **gen7 R8 (R8CAP).** `:: gen7: r8 begin rung=R8 wake=…` and a verdict, not a refusal.
- **Wifi sweep (WIFISWEEP).** `… site=post-wifi walks_since_enum=<n>`; wifi arcs as flight 8 (radio MATCH, 3/3 STAGED, d11 FOUND, no upload).

## WATCH LIST, in order
1. Banner `⚡ kernel features:` carries `quarry` plus flight 8's set. 2. FTDI-CAP verdict. 3. BOOTCLOCK. 4. storage settle + X86BIND.
5. STACK render high. 6. W5 self-test PASS x2. 7. Quarry tile press. 8. Typing over the wire:
       flatpak-spawn --host bash -c "printf 'help\r' > /dev/ttyUSB0"
   then `date`, then after 60 s of quiet `storm`. 9. Under storm: count `GATE STOLEN` (each ~15xx ms) and read the two W5 lines beside each;
   watch whether the arrow keeps moving during a freeze (`[ptrinstall] installs==reports`); say when vugs lock and the machine freezes.
10. **Do NOT type `install`** (no INSTALLDEMO / no AHCI write on this line; SSD read-only by construction; sitting is NO-GO, SITTING-1.md).

## KNOWN / EXPECTED (not findings)
KF27 dark; KD14 separated (KD3 re-opened); gmux ladder varies; BAR1WEDGE relatch at enum; keyboard on EHCI hub3; the freeze itself
(1.5 s, core lost) until W5 answers; `[wc-i]` intrusions intermittent.

## CARD
Capture under squawk session `rmbp12-flight8` (host), appended after a `=== SQUAWK MARK`; waker re-armed with A_ANCHOR at the log's size
and the flight-10 tags. Card: the seat runs `card-write.sh <staged> --write` (host, flat copy incl. data/ and B43/), sha-match,
then `x86-preflight.sh rmbp12-flight8 <staged>` must print READY before "staged" is said.

## CERT
`scripts/banner-cert.sh` inside `esp-x86`: `ok=36 missing/leak=0 registered-divergences=0 unverifiable=0 nowitness=0 noverdict=0`. Token hits in kernel.elf by `LC_ALL=C grep -a -o -F`: `:: FTDI-CAP:` 4, `:: BOOTCLOCK: firmware->loader=` 1, `[dock] pins=` 1, `:: STORSLOT: claim slot=` 2, `site=post-wifi` 2, `flicker_frames=` 1, `:: gen7: r8 begin rung=R8 wake=` 1, `:: QUARRYDOOR: win=` 1, `storage settle` 1, `:: W5: site=post-steal` 2, `:: W5: nmi core=c` 4, `:: STACK: render high=` 0, `:: PTRINSTALL: installs=` 1, `[ptrinstall] installs=` 1, `:: x86 fb-wc: ARM=wc` 1, `[wc-i] rollup` 1, `:: WINMENU:` 5, `GATE STOLEN` 1
