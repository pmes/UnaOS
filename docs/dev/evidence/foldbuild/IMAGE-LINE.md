# IMAGE-LINE — the derivation of `unaos/scripts/image-line.env` (FOLDBUILD, rmbp-ledger B517)

**What this is.** Until FOLDBUILD the rMBP image build line existed only as a chain of deltas across
`docs/dev/OS/rmbp-queue.md` STATE paragraphs and FLIGHT headers ("image 20's unchanged" → "image 19's + …" →
"image 18's + …" → "unchanged since image 15" → "image 14's proven line − the nine recon knobs + SELFDIAG" → … →
"image 11's PROVEN line + SELFHOST LINUXABI BUSREG"). The chain never reaches a fully-spelled *proven* line in the
tree: images 3–11 are deltas against the bench's `~/unaos-bench/flash/rmbp/MANIFEST`, which is bench-local. The
**earliest fully-spelled line the bench flew that the tree holds** is flight 10's (image 2b,
`docs/dev/evidence/rmbp-0917/flight10/PLAYBOOK-flown.md:18`, 30 knobs). This table walks forward from it through
every named delta, and cross-checks each knob against flight 28's pinned wire (`f28-boot1.log`, image 21 = the
current line) where a line exists whose emitter is under that feature's cfg. **The bench confirms the result against
its shell history** — the rows marked BENCH are the ones the documents cannot settle.

## The anchor (flight 10, image 2b, 2026-09-16, hw-rmbp@fa4dcf0b)

`UNAOS_WC UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO UNAOS_KEPLER_CE UNAOS_IVB UNAOS_IVB3D UNAOS_GMUX_IGD
UNAOS_WITNESS UNAOS_WCG_PAYGO UNAOS_LOGTS UNAOS_WIFI UNAOS_WIFI2 UNAOS_BT UNAOS_BTC UNAOS_SMC UNAOS_SMCWALK
UNAOS_RTWIT UNAOS_USBDEBUG UNAOS_NOASPM UNAOS_DEADMAN UNAOS_WCDVALVE UNAOS_FTDIRX UNAOS_BEAM UNAOS_AHCI
UNAOS_BAR1WEDGE UNAOS_IVB3D_R8 UNAOS_KEPLER_KFBIND UNAOS_KEPLER_KDHEAD UNAOS_QUARRY` (each `=1`).

## The deltas, in order

| step | delta | source |
|---|---|---|
| flight-11 era | + `WIFI3 WIFI4` (Peter's go 2026-09-17), + `KEPLER_CTRL_ADDR KEPLER_CTRLBIND`, + `HDA HDATONE` | rmbp-queue.md STATE (gate7 flight line, "the flight line takes UNAOS_HDA=1 UNAOS_HDATONE=1 now") |
| flight-11 era | + `LOGIN` (NEVER `LOGINST` on media) | rmbp-queue.md ("knob line plus `UNAOS_LOGIN=1`") |
| image 3 | flight 8's MANIFEST line − "the four probe knobs" + `AHCI_WRITE INSTALLDEMO INSTGUI QUARRY` | rmbp-queue.md "THE KNOB LINE (image 3)" — the four are NOT named in the tree (BENCH) |
| images 4–8 | "knob line = flight 12's" | the staging records in rmbp-queue.md |
| image 9 | + `FACET` | "knob line = image 8's + UNAOS_FACET=1" |
| image 10 | + `USBNET` | "Knob line = image 9's + UNAOS_USBNET=1 (Peter's line)"; FLIGHT17.md |
| image 11 | unchanged | "knob line = image 10's (nothing new armed; UNAOS_BEAM=1 confirmed on the line since flight 8)" |
| image 12 | + `SELFHOST LINUXABI BUSREG`; the seat's pasted line lacked `KEPLER KEPLER_TAKEOVER IVB GMUX_IGD` and carried `LOGINST` — the bench flew image 11's PROVEN line + the three | STATE BOOT 19 READ ("BUILD LINE — CORRECTED"); FLIGHT19.md; `KEPLER_VBLANK` "stays as flown on boot 19" |
| image 13 | + `VEIN LUMEN NETRING3` | FLIGHT20.md; STATE MERGE10 |
| image 14 | − `VEIN` (the knob no longer exists) + `UNAFS` (R82); `KVBLANK_TRACE` optional | STATE MERGE11 FOLDED; FLIGHT21.md |
| image 15 | − the nine recon knobs `KEPLER_KFBIND KEPLER_KDHEAD KEPLER_CTRLBIND KEPLER_CTRL_ADDR IVB3D IVB3D_R8 BAR1WEDGE SMCWALK RTWIT` + `SELFDIAG` (+ `BTC`, already on); `AHCIROOT` optional (install sitting only) | STATE BOOT 21 READ / MERGE12; FLIGHT22.md ("image 14's proven line − the nine recon knobs + `UNAOS_SELFDIAG=1`") |
| images 16–18 | unchanged (`UNAOS_LLD_LNX` optional, `UNAOS_SELFBUILD6_RUSTC` default-on under R87 — staging, no knob on the line) | STATE MERGE14/MERGE16; FLIGHT23/24/25.md ("unchanged since image 15") |
| image 19 | + `IVB3D IVB3D_R8` (GEN7) + `WIFI_FW_PATH` (+ `WIFI WIFI2 WIFI3 WIFI4`, already on) | STATE MERGE17 FOLDED; FLIGHT26.md |
| image 20 | + `IVB3D_BLIT LIDSLEEP VIDEO`; − `USBDEBUG` (WIREDIET) − `KEPLER_FIFO`; `WIFI5` OFF | STATE MERGE18/MERGE19 FOLDED; FLIGHT27.md |
| image 21 | unchanged (no `IVB3D_R9`) | STATE MERGE20 FOLDED; FLIGHT28.md ("Knob line = image 20's, unchanged") |

## The result, knob by knob (the file's order = arroyo's knob-map order)

`wire` = a flight-28 line whose emitter is compiled only under that feature (the cfg read at this tree), so the
knob was on image 21. `—` = nothing on that wire decides it (QUIETBOOT keeps most features silent until `tests`).

| knob | feature (arroyo → builder) | added by | wire (f28-boot1.log) | status |
|---|---|---|---|---|
| UNAOS_WITNESS | witness | flight 10 | `held=bt-campaign,witness-fixture` | wire |
| UNAOS_SELFHOST | selfhost | image 12 | `:: SELFHOST:` | wire |
| UNAOS_WCG_PAYGO | wcg-paygo | flight 10 | `[wc-d] paygo win=1 …` (wm.rs:9259, cfg witness+wcg-paygo) | wire |
| UNAOS_WCDVALVE | wcdvalve | flight 10 | `[wc-d] valve CLOSED util~` (wm.rs:7771, cfg witness+wcdvalve) | wire |
| UNAOS_LOGTS | logts | flight 10 | the kernel-clock stamps FLIGHT22/23.md name | derived |
| UNAOS_SMC | smc | flight 10 | `[smc] lid=open …` (LIDSLEEP implies smc too) | wire |
| UNAOS_BT | bt | flight 10 | `probe=bt-campaign` | wire |
| UNAOS_BTC | btc | flight 10; named again image 15 | — | derived |
| UNAOS_LOGIN | login | flight-11 era | `[login]` ×31; `[holocron] door kdf=kernel` (keyring.rs: lumen+login) | wire |
| UNAOS_INSTALLDEMO | installdemo | image 3 | — (implied by INSTGUI in Cargo) | derived |
| UNAOS_WIFI | wifi | flight 10 | `WIFI5 READY …` | wire |
| UNAOS_WIFI2 | wifi2 | flight 10 | (implied by wifi3) | derived |
| UNAOS_WIFI3 | wifi3 | flight-11 era | `:: wifi2: ucode upload words=` (banner-cert's wifi3 token) | wire |
| UNAOS_WIFI4 | wifi4 | flight-11 era | — | derived |
| UNAOS_KEPLER | nvidia-kepler | flight 10; restored image 12 | `:: GPUBLIT: selftest=ok … -> blitter=gpu` (gpu/mod.rs:104, cfg nvidia-kepler+takeover) | wire |
| UNAOS_KEPLER_TAKEOVER | nvidia-kepler-takeover | flight 10; restored image 12 | same line | wire |
| UNAOS_NOASPM | noaspm | flight 10 | `[pcih] aspm cleared rp` (pcihealth.rs, cfg noaspm) | wire |
| UNAOS_WC | wc | flight 10 | `[wc-x] activate bare` | wire |
| UNAOS_KEPLER_VBLANK | nvidia-kepler-vblank | image 12 ("stays as flown on boot 19") | `:: kepler: vblank-intr window open` | wire |
| UNAOS_DEADMAN | deadman | flight 10, never named dropped | — (census-gated since QUIETBOOT) | **BENCH** |
| UNAOS_INSTGUI | instgui | image 3 | — | derived |
| UNAOS_LINUXABI | linuxabi | image 12 | — | derived |
| UNAOS_BUSREG | busreg | image 12 | — | derived |
| UNAOS_LUMEN | lumen | image 13 | `:: LUMEN:` | wire |
| UNAOS_NETRING3 | netring3 | image 13 | — | derived |
| UNAOS_SELFDIAG | selfdiag | image 15 | — | derived |
| UNAOS_LIDSLEEP | lidsleep | image 20 | `[smc] lid=open raw=0 … key=MSLD` | wire |
| UNAOS_VIDEO | videoplayer | image 20 | — | derived |
| UNAOS_IVB | intel-ivb, unaos_ivb (+ the loader's unaos_ivb) | flight 10; restored image 12 | `igpu:` ×21 | wire |
| UNAOS_IVB3D | gen7 | flight 10; − image 15; + image 19 | `GEN7LADDER …` | wire |
| UNAOS_IVB3D_R8 | gen7, gen7r8 | flight 10; − image 15; + image 19 | `:: gen7: r8 begin` | wire |
| UNAOS_IVB3D_BLIT | gen7, gen7blit | image 20 | `:: GEN7BLIT2: impl=bcs` | wire |
| UNAOS_GMUX_IGD | gmux_igd | flight 10; restored image 12 | `[GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD` | wire |
| UNAOS_BEAM | beam | flight 10 ("since flight 8") | `:: BEAMX86: head=` | wire |
| UNAOS_QUARRY | quarry | flight 10; image 3 | `[quarry]` ×8 | wire |
| UNAOS_FACET | facet | image 9 | — | derived |
| UNAOS_FTDIRX | ftdirx | flight 10 | `:: FTDIRX: first byte rx=` | wire |
| UNAOS_USBNET | usbnet | image 10 | — (the dongle was out) | derived |
| UNAOS_AHCI | ahci | flight 10 | `:: AHCI: selfcheck port=0 …` | wire |
| UNAOS_AHCI_WRITE | ahci-write | image 3, never named dropped | — (`write path ARMED` prints at the first grant only) | **BENCH** |
| UNAOS_UNAFS | unafs | image 14 | `:: UNAFSX86: root=` | wire |
| UNAOS_HDA | hda | flight-11 era | `[hda] census` | wire |
| UNAOS_HDATONE | hda-tone | flight-11 era | `:: HDA-TONE:` | wire |
| UNAOS_IOAPIC | ioapic | **no STATE names it** (IOAPIC B147, the flight-13 ISRARM era) | `[ioapic] census ioapics=1 …` (ioapic.rs, `#[cfg(feature = "ioapic")] pub mod ioapic`) | wire, **BENCH** (which STATE) |
| UNAOS_WIFI_FW_PATH | — [media]: arroyo:9359 stages the b43 set onto the card's UnaFS /FIRMWARE/; builder/src/main.rs and the kernel never read it | image 19 | `WIFI5 READY … fw=pinned sha=match` | wire |

## Named and left OFF the line

| knob | why | source |
|---|---|---|
| UNAOS_KEPLER_FIFO | dropped at image 20 | STATE MERGE18/19 FOLDED |
| UNAOS_USBDEBUG | dropped at image 20 (WIREDIET; 2634 of 17915 lines) | same |
| UNAOS_KEPLER_KFBIND, KEPLER_KDHEAD, KEPLER_CTRLBIND, KEPLER_CTRL_ADDR, BAR1WEDGE, SMCWALK, RTWIT | the recon knobs, dropped at image 15 (QUIETBOOT, R80) | STATE BOOT 21 READ |
| UNAOS_KEPLER_CE | a recon knob QUIETBOOT keeps off — `drivers/gpu/mod.rs:103`: GPUBLIT "is NOT behind the recon knob `nvidia-kepler-ce` (QUIETBOOT keeps that off)"; not in the image-15 list of nine, so presumably one of image 3's unnamed four | **BENCH** |
| UNAOS_LOGINST | NEVER on media (the fixture credential) | STATE BOOT 19 READ |
| UNAOS_CENSUS | QUIETBOOT: `tests quietboot` needs census OFF | STATE MERGE11 |
| UNAOS_VEIN | the knob no longer exists (image 14) | STATE MERGE11 |
| UNAOS_SDW | default-on since SDHCRW (R59): "`UNAOS_SDW=1` decides nothing" (arroyo:401); in arroyo no `-n` map row, so not a knob of the line | arroyo |
| UNAOS_WIFI5, UNAOS_IVB3D_R9 | OFF (WIFI5 until C1 reads NON-DISTURBING; R9 only on Peter's word) | STATE MERGE17/MERGE20 |
| UNAOS_AHCIROOT, UNAOS_KVBLANK_TRACE | optional for one sitting each, never the standing line | STATE MERGE12 / MERGE11 |
| UNAOS_HOLOCRON, UNAOS_SVG, UNAOS_PREFS_RESET | in the seat's merge-gate "x86 metal shape" (boot-leg.sh METAL) but named by no STATE as an image knob; PREFS_RESET would reset the display prefs at every boot | boot-leg.sh, BRIGHTFLOOR |

**Not the line: the seat's "x86 metal shape".** `scripts/boot-leg.sh`'s METAL list (the merge gate's compile shape)
carries `loginst census witness usbdebug holocron svg prefs_reset ahciroot kvblank_trace nvidia-kepler-ce` and lacks
`wcg-paygo logts noaspm deadman ioapic wifi2 wifi3 wifi4 bt` — it is a compile superset/subset of convenience, which
is exactly B517's finding: the image's set was no leg's set.

## For the bench to confirm (against the shell history of the image-21 build)

1. `UNAOS_DEADMAN=1` and `UNAOS_AHCI_WRITE=1` — on the line? (carried since flight 10 / image 3, never named dropped)
2. `UNAOS_IOAPIC=1` — on (the wire says so); which flight added it?
3. `UNAOS_KEPLER_CE` — off? (the source says QUIETBOOT keeps it off; no STATE drops it by name)
4. Anything the image-21 build armed that is not in the file — the four probe knobs image 3 removed are unnamed.

A correction is one commit to `unaos/scripts/image-line.env` (the gate row `image-line` checks the shape); from then
on a STATE cites `image-line.env @<sha>`.
