# FLIGHT — orin 14 · render4 pre-flight (FLIGHTPREP4)

Prepared 2026-09-05 against `hw-jetson@6cc8de8c` (`git log --oneline -1` → `6cc8de8c docs: D3 dropped …`),
working tree clean. Read-only: nothing in the repo was built or staged by this document. The recipe
is orin 13's (`docs/dev/evidence/orin13/FLIGHT.md`, flown as render2/render3/render3b) with the
questions replaced by the four ledger rows render4 answers. Every scorer below was dry-run against
the render3b excerpt (`docs/dev/evidence/orin13/render3b-boot1.log`, 640 lines) and the quoted
"render3b control" is what it printed there — the pre-fix behaviour, proving each awk can fire.

**What render4 is:** the render3b knob line and layout (APTEXT `fef6a184` aboard) plus the three
orin 14 arcs — A16 (RX discriminators: IIR/FIFO printed once, `ovrf=` on the census, paced
injection), A17 (the second Print Screen gets a verdict), A18 (R17: the bottom status strip and the
embedded per-core lamps leave the cascaded scene, the windowed pulse returns). `SHA` below is the
`hw-jetson` tip once those land; it is set at flight time, not here.

Paths used throughout:

| name | path |
|---|---|
| repo (the orin worktree — build HERE, never in an agent worktree) | `/home/pmes/src/github.com/pmes/UnaOS-orin` |
| scratch | `~/unaos-bench/scratch/orin14/` |
| evidence (this dir) | `docs/dev/evidence/orin14/` |
| staging script | `~/unaos-bench/scratch/orin14/stage-render4.sh` (writes `NAME_RENDER4`) |
| build log | `~/unaos-bench/scratch/orin14/build-render4.log` |
| staged media | `~/unaos-bench/flash/orin/render4-<UTC>-<sha7>/` (name in `$SCR/NAME_RENDER4`) |
| card loader | `~/unaos-bench/scratch/orin11/load-card.sh` (finds the card by LABEL `UNAOS-ORIN`; harvests, copies, sha-matches 10/10, unmounts) |
| card (when in the reader) | `/run/media/pmes/UNAOS-ORIN` |
| capture (append-only, many boots) | `~/unaos-bench/capture/line-acm0/orin.log` — and `raw.log`, `unknown.log` (LEDGER P3: the anchor may land there) |
| unwrapper (takes a FILE, P2) | `~/unaos-bench/tools/unwrap80.sh` |
| butler | `~/unaos-bench/tools/line-butler.py`; pid = `flatpak-spawn --host lsof -t /dev/ttyACM0` (the authority) |
| inject FIFO | `~/unaos-bench/capture/line-acm0/inject.fifo` |
| paced injector (A16 executor) | `~/unaos-bench/tools/inject-paced.sh <fifo> <string> [ms-per-byte=50]` |
| A16 decision table | `docs/dev/evidence/orin14/A16-SCORE.md` (executor A16 — link, not duplicated here) |
| previous flight | `docs/dev/evidence/orin13/FLIGHT-RESULT-render3b.md`; card holds `render3b-20260905T1835Z-7fb1d5d` (kernel `fef6a184`, `KELF max=0x2d3488`) |

---

## A. PRE-FLIGHT — build, stage, load, hold the line

### A.1 Clean tree at the sha to fly

```sh
REPO=/home/pmes/src/github.com/pmes/UnaOS-orin; UNA=$REPO/unaos
FLASH=$HOME/unaos-bench/flash/orin; SCR=$HOME/unaos-bench/scratch/orin14
CAP=$HOME/unaos-bench/capture/line-acm0; UNWRAP=$HOME/unaos-bench/tools/unwrap80.sh
SHA=<hw-jetson tip with A16+A17+A18 landed>        # <-- the commit to fly; set at flight time
cd "$UNA"
git status --porcelain | wc -l                   # must print 0 (a dirty tree is not the sha it names)
git log --oneline -1                             # must be $SHA
GIT7=$(git rev-parse --short=7 HEAD); GITFULL=$(git rev-parse HEAD)
```

### A.2 Build — render3b's knob line, verbatim

The knob line is UNCHANGED from render3b. **Never add `UNAOS_ORINCONWIN`, `UNAOS_ORINDESK` or
`UNAOS_ORINTENANT`**: the cascade refuses a non-empty window table
(`[deskcascade] REFUSE reason=table-not-empty`, `main.rs`) and the whole flight scores NO-CASCADE.

```sh
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 \
  ./arroyo esp-jetson 2>&1 | tee "$SCR/build-render4.log"
echo "ESP_JETSON_EXIT=${PIPESTATUS[0]}" | tee -a "$SCR/build-render4.log"    # must be 0; stage-render4.sh greps this exact line
awk '/kernel features|effective features/' "$SCR/build-render4.log" | tee -a "$SCR/build-render4.log"
#   expected effective (render3b's, unchanged knobs):
#   witness,ehcihid,holocron,tegra,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,deskcascade
```

Banner gate: all three lines carry `witness`, `orinrender`, `deskcascade`, `orinrx`; the effective
line equals render3b's. If an orin 14 arc added a knob, the seat re-derives this line from
`unaos/arroyo` and records the difference in FLIGHT-RESULT — it is not silently absorbed.

In-artifact gate — the strings the scorers key on must exist in THESE bytes (`grep -a`, never
`strings`; a build that lacks a token cannot answer its question, and that is a build fact to record,
not a flight fact):

```sh
for t in 'u7stk' 'boot-core:post-cascade' '-> CASCADED' '[pulsewin] open' 'PRTSCR: ' '[serialrx]' \
         'ovrf=' 'iir=' 'strip=' 'pulsewin='; do
  printf '%-26s %s\n' "$t" "$(grep -a -o -F "$t" target/aarch64_esp/kernel.elf | wc -l)"
done
#   u7stk/post-cascade/CASCADED/PRTSCR/[serialrx] >=1 (all were >=1 on render3b bytes);
#   '[pulsewin] open' >=1 (A18 restores the open path — 0 means A18 is not aboard);
#   'ovrf=' and 'iir=' >=1 (A16 discriminators — 0 means A16 is not aboard);   <-- A16 TOKEN SLOT
#   'strip=' and 'pulsewin=' >=1 (A18's census fields — 0 means A18 is not aboard). <-- A18 TOKEN SLOT
grep -a -o -F 'KELF min=' target/aarch64_esp/EFI/BOOT/BOOTAA64.EFI | wc -l     # 1 (the boot anchor)
```

### A.3 Stage

```sh
bash "$SCR/stage-render4.sh"           # refuses a dirty tree or a build log without ESP_JETSON_EXIT=0
NAME=$(cat "$SCR/NAME_RENDER4"); echo "$NAME"; ls "$FLASH/$NAME"
awk '!/^#/ && !/^$/ && $1 !~ /^[0-9a-f]{64}$/ {bad++} END{print "manifest_bad_lines=" bad+0}' "$FLASH/$NAME/MANIFEST"   # 0
readelf -l -W "$FLASH/$NAME/kernel.elf" | awk '/LOAD/{v=strtonum($3)+strtonum($6); if(v>m)m=v} END{printf "render4 elf max=%#x\n",m}'
#   record this value: it is the boot's identity on the wire (§C.1). render3b's was 0x2d3488; render4's WILL differ.
```

### A.4 Load the card — after tidying the A17 artefacts off it

`load-card.sh` harvests the card and copies over it; it does NOT clear the volume root. The card
today carries `SCREEN0.PNG` (6913793 B, valid) and `SCREEN1.PNG` (0 B — the A17 artefact).
`prtscr` names captures "first free index at the volume root" (`video/prtscr.rs:294-302`), and a
0-byte entry is an entry, so an untidied card would name render4's captures `SCREEN2.PNG`/`SCREEN3.PNG`.
The scorer in §C.4 takes the names from the wire either way; tidy the card so the names read as the
ledger row does. The harvest copy is taken FIRST and verified before anything is removed:

```sh
MP=/run/media/pmes/UNAOS-ORIN; H=$SCR/render3b-card-harvest; mkdir -p "$H"
cp "$MP"/SCREEN*.PNG "$MP"/UPD0.TMP "$H"/ 2>/dev/null; ls -la "$H"
sha256sum "$MP/SCREEN0.PNG" "$H/SCREEN0.PNG" "$HOME/unaos-bench/scratch/orin13/render3b-SCREEN0.png"   # all three equal
# only after the three shas agree:
rm "$MP"/SCREEN*.PNG "$MP"/UPD0.TMP; flatpak-spawn --host sync; ls "$MP"      # no SCREEN*.PNG remains
"$HOME/unaos-bench/scratch/orin11/load-card.sh" "$NAME"            # dry run: prints CARD: /dev/… and SRC:
"$HOME/unaos-bench/scratch/orin11/load-card.sh" "$NAME" --write    # copies, sha-matches 10/10, unmounts
# expected tail:  "=== 10/10 match. Card carries render4-…. ===" then "UNMOUNTED /dev/… — safe to pull."
KSHA=$(sha256sum "$FLASH/$NAME/kernel.elf" | cut -c1-16)
echo "$NAME WRITTEN TO CARD (UNAOS-ORIN) $(date -u +%Y-%m-%dT%H:%MZ) by orin 14, hand load — sha-verified $KSHA, all 10 files matched against the staged MANIFEST, unmounted clean." >> "$FLASH/MANIFEST"
```

A 10/10 match proves the WRITE. It does not prove what BOOTS — the firmware can still pick another
volume (orin13's dark boots of `0xabfbdefa`). §C.1 scores the loaded image by its `max=`, first.

### A.5 Hold the line — the butler, checked immediately before power-on

The butler RELEASES the port on cable unplug (it exited that way on 2026-08-26). `lsof` is the
authority, not `butler.pid` and not `bench-state.sh`'s ledger line. Run this in the same minute as
the power-on, and again if the cable was touched:

```sh
flatpak-spawn --host lsof -t /dev/ttyACM0        # a pid (34809 at 19:09Z on 2026-09-05) — or EMPTY = unheld, the boot would be lost
# if EMPTY, restart ON THE HOST (in-sandbox the device reads nobody:nogroup and the open fails silently):
nohup flatpak-spawn --host python3 "$HOME/unaos-bench/tools/line-butler.py" /dev/ttyACM0 "$CAP" \
   > "$CAP/butler-start.out" 2>&1 &
sleep 3; flatpak-spawn --host lsof -t /dev/ttyACM0; tail -1 "$CAP/butler-witness.log"   # "=== line-butler holds /dev/ttyACM0 @ …"
# capture floor for the boot — the three files, so a P3 anchor in raw/unknown is still addressable:
for f in orin.log raw.log unknown.log; do printf '%s=%s ' "$f" "$(wc -l < "$CAP/$f")"; done; echo
echo "MARK render4 $GIT7 boot1 power-on at $(date -u +%Y-%m-%dT%H:%M:%SZ) raw=$(wc -c < "$CAP/raw.log") orin=$(wc -l < "$CAP/orin.log") unknown=$(wc -l < "$CAP/unknown.log")" >> "$CAP/marks.txt"
```

---

## B. THE BOOT — one sequence, in order

1. Card in the Orin; §A.5 butler check (pid printed) in the same minute.
2. Power on. Watch `tail -f "$CAP/orin.log"` for `ORIN-SMP-3 CPU_ON AP 5 … -> SUCCESS` then
   `[deskcascade] -> CASCADED` then `[pulsewin] open win=` then `-> RENDER-LIVE`. If the wire shows
   `Exception reason=1 syndrome=0x82000010` + `Powering off core` after `enumerated core 5`, that is
   the A15 signature: power-cycle and count it (§C.3 scores every boot; the excerpt is the LAST one).
3. **Eyes on the panel** (A18 has no wire proxy for pixels): is the bottom status strip GONE? are the
   embedded per-core lamps GONE? is the pulse WINDOW present at (10,874)-ish? Note it in FLIGHT-RESULT.
4. Sit at RENDER-LIVE ≥ 30 s (at least two `[serialrx] rx=` census lines).
5. **A16 burst leg** (render3b's shape): `L0=$(wc -l < "$CAP/orin.log"); printf 'tste\r' > "$CAP/inject.fifo"`;
   `echo "MARK render4 $GIT7 q-burst 'tste\r' injected at $(date -u +%FT%TZ) raw=$(wc -c < "$CAP/raw.log") orin=$L0" >> "$CAP/marks.txt"`. Wait 20 s.
6. **A16 paced leg**: `L1=$(wc -l < "$CAP/orin.log"); "$HOME/unaos-bench/tools/inject-paced.sh" "$CAP/inject.fifo" 'tste\r' 50 | tee "$SCR/render4-paced-inject.out"`;
   `echo "MARK render4 $GIT7 q-paced 'tste\r' 50ms at $(date -u +%FT%TZ) raw=$(wc -c < "$CAP/raw.log") orin=$L1" >> "$CAP/marks.txt"`. Wait 20 s.
   (The injector prints one stamped line per byte and a `raw=` summary — keep its output; it is the
   window the A16-SCORE.md table reads.)
7. **A17 press #1**: Print Screen at the USB keyboard. Wait for `:: PRTSCR: SCREEN0.PNG … -> OK ::`
   (the 6.9 MB write over USB BOT takes seconds — wait for the verdict line, not the armed line).
8. **A17 press #2**: Print Screen again. Wait ≥ 30 s for `:: PRTSCR: SCREEN1.PNG … -> OK ::` OR a
   named refusal (`… — capture skipped ::` / `… — capture INCOMPLETE ::`). The render3b failure was
   silence here.
9. ≥ 60 s more of census, then power off. Card to the reader. §C.

One line: **card in → `lsof -t /dev/ttyACM0` → marks → power on → CPU_ON×5 → CASCADED → `[pulsewin] open` → RENDER-LIVE ≥30 s → burst `tste\r` → 20 s → `inject-paced.sh … 50` → 20 s → PrtSc #1 → `-> OK` → PrtSc #2 → verdict → ≥60 s → off → card to reader.**

---

## C. THE SCORING — pin, prove purity, extract, then one scorer per question

### C.1 Pin the boot by the LOADED image's identity (P3: in whichever file carries the anchor)

The anchor is the loader identity line `KELF min=0x0 max=<hex> pg=<n>` (`crates/bootloader/src/main.rs@792`);
its `max=` must equal render4's staged `kernel.elf` max vaddr (§A.3). Pre-orin-12 captures use the
`Kernel ELF: min_vaddr=` wording, so the regex takes both. Always through `unwrap80.sh` (a FILE).

```sh
NAME=$(cat "$SCR/NAME_RENDER4")
MAXV=$(readelf -l -W "$FLASH/$NAME/kernel.elf" | awk '/LOAD/{v=strtonum($3)+strtonum($6); if(v>m)m=v} END{printf "%#x",m}'); echo "render4 elf max=$MAXV"
for f in orin.log raw.log unknown.log; do
  "$UNWRAP" "$CAP/$f" | awk -v f="$f" -v want="max=$MAXV " '/KELF min=|Kernel ELF: min_vaddr=/{n=NR; l=$0; if(index($0,want)) h=NR}
       END{printf "%-12s lines=%d last_anchor=%d last_render4_anchor=%d :: %s\n", f, NR, n, h, substr(l,1,72)}'
done
```

PASS predicate: at least one file has `last_render4_anchor > 0`, and in that file it is the LAST
anchor (`last_anchor == last_render4_anchor`); a later, different anchor means the board went on to
boot something else and the render4 boot is the excerpt BETWEEN them (the extraction below stops at
the next anchor regardless). If no file has a render4 anchor, the board booted something else
(stale volume, the firmware's other choice) — STOP; nothing below is about render4.

Where the anchor sits today (2026-09-05, before render4): `orin.log` 51791 and `raw.log` 107884
both carry render3b's `max=0x2d3488`; `unknown.log` 15857 carries render2's `max=0x2d92a8`. So the
render3b boot was routed into `orin.log` normally, but the P3 case (render2's anchor in
`unknown.log`) is one boot old — check all three.

### C.2 Extract the excerpt (first line = the anchor) and prove board purity

```sh
SRC=$CAP/orin.log; n=<last_render4_anchor in $SRC>             # from C.1
"$UNWRAP" "$SRC" | awk -v n="$n" 'NR>n && /KELF min=|Kernel ELF: min_vaddr=/{exit} NR>=n{gsub(/\x1b\[[0-9;]*[A-Za-z]/,""); print}' \
   > "$REPO/docs/dev/evidence/orin14/render4-boot1.log"
B=$REPO/docs/dev/evidence/orin14/render4-boot1.log
head -1 "$B"; wc -l "$B"          # line 1 = `[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=$MAXV pg=…`
# board purity — the butler's own marker sets (line-butler.py ORIN_MARKS / PI_MARKS), counted over the excerpt:
awk '/tegra|Tegra|TEGRA|Jetson|ga10b|GA10B|\[orin/{o++} /BCM2711|bcm2711|VideoCore|mailbox-fb|kernel8|rpi-4-b|start4\.elf|raspberrypi|\[v3d/{p++; if(p<=3) print "PI-LINE " NR ": " substr($0,1,100)}
     END{printf "orin_marks=%d pi_marks=%d lines=%d -> %s\n",o,p,NR,(o&&!p)?"PURE":(p?"MIXED — a Pi line is in the excerpt; cut it or do not score":"NO MARKS")}' "$B"
```
PASS predicate: `PURE`. render3b control: `orin_marks=165 pi_marks=0 lines=640 -> PURE`. If the
excerpt came from `raw.log` (P3), MIXED is possible when the probe moved mid-boot — name the lines.
(`PIUSB` is the shared USB-storage witness family and is NOT a Pi marker — the butler rejected it
for exactly that reason.)

### C.3 Scorer — A15, `CPU_ON AP n -> SUCCESS` ×5 (pass count; 1 pass so far)

Ledger row: **A15** (`fixed-unflown — 1 pass` → `2 passes` on PASS; "closes as flown after
several passes" is the row's own rule — this flight ticks the count, it does not close the row).

```sh
awk '/ORIN-SMP-3 CPU_ON AP [0-9]+ .*-> SUCCESS/{s++} /CPU_ON AP [0-9]+ .*-> ERROR/{e++} /[0-9]+\/[0-9]+ secondaries online via PSCI CPU_ON/{o++} /Exception reason=1 syndrome=0x82000010/{x++} /Powering off core/{p++}
     END{printf "cpu_on_success=%d cpu_on_error=%d el3_abort=%d poweroff=%d online_line=%d -> %s\n",s,e,x,p,o,(s==5&&!e&&!x&&!p&&o)?"PASS (A15 pass +1)":(x||p)?"FAIL A15-signature (AP died in its MMU-off window)":"NO-VERDICT"}' "$B"
```
PASS predicate: `cpu_on_success=5`, zero ERROR, zero `0x82000010` aborts, zero `Powering off core`,
and the `5/5 secondaries online via PSCI CPU_ON` line present.
render3b control: `cpu_on_success=5 cpu_on_error=0 el3_abort=0 poweroff=0 online_line=1 -> PASS`.
Every power-on this session is a sample: if the boot had to be retried, score each attempt's excerpt
and record `passes/attempts` in FLIGHT-RESULT — A15 is a rate.

### C.4 Scorer — A1/§5.2, the boot-core stack through the cascade, again

Ledger row: **A1** (already flown; this is the second sample of the §5.2 number — record it beside
render3b's `hw=15472 headroom=17296` in FLIGHT-RESULT; the row's status does not change).

```sh
awk '/\[u7stk\] at=boot-core:post-cascade/{n++; for(i=1;i<=NF;i++){if($i~/^hw=/){sub(/hw=/,"",$i);hw=$i+0} if($i~/^headroom=/){sub(/headroom=/,"",$i);hr=$i+0} if($i~/^len=/){sub(/len=/,"",$i);len=$i+0}}} /\[deskcascade\] arming/{a++}
     END{printf "arming=%d post_cascade=%d len=%d hw=%d headroom=%d -> %s\n",a,n,len,hw,hr,n?(hr>0?"PASS (unsaturated)":"SATURATED — widen the boot-core window"):(a?"NO post-cascade probe after arming (overflow: the §5.2 stop-line case)":"NO CASCADE ARMED")}' "$B"
awk '/\[u7stk\] at=boot-core:(pre|post)-cascade/' "$B" | cut -c1-160
```
PASS predicate: exactly one `post-cascade` probe with `headroom > 0`. An `arming` line with NO
`post-cascade` probe is the overflow signature (§5.2). A18 changes what the cascade paints, so this
number is expected to MOVE — that is why it is re-measured.
render3b control: `arming=1 post_cascade=1 len=32768 hw=15472 headroom=17296 -> PASS`.

### C.5 Scorer — A18, the strip and the embedded pulse are gone, the windowed pulse is back

Ledger row: **A18** (R17). The wire can prove the pulse WINDOW opened and (via the A18 census
field) that the strip retired; the PANEL is the eye's — step B.3 records what Peter saw.

```sh
awk '/\[deskcascade\] -> CASCADED/{c++} /\[deskcascade\] REFUSE/{r++; rr=$0} /\[pulsewin\] open win=/{pw++; pl=$0} /\[pulsewin\] open DECLINE/{pd++; pl=$0} /\[orinrender\] strip=kept/{sk++}
     /\[orinrender\] census/{if(match($0,/strip=[a-z]+/))st=substr($0,RSTART+6,RLENGTH-6); if(match($0,/pulsewin=[0-9]+/))pv=substr($0,RSTART+9,RLENGTH-9)}   # <-- A18 CENSUS TOKEN SLOT: `strip={retired|live} pulsewin=<win>` as written by executor A18 at 13:17Z; re-check the token before scoring
     END{printf "cascaded=%d refuse=%d pulsewin_open=%d pulsewin_decline=%d strip_kept=%d census_strip=%s census_pulsewin=%s -> %s\n",c,r,pw,pd,sk,(st?st:"n/a"),(pv?pv:"n/a"),
     (c&&pw&&!pd&&!sk&&st=="retired")?"PASS":(c&&pd)?"FAIL pulsewin DECLINE":(c&&!pw)?"FAIL no [pulsewin] open on the cascaded scene":(sk||st=="live")?"FAIL strip still live":(r?"NO-CASCADE: "substr(rr,1,80):"NO-VERDICT")}' "$B"
awk '/\[pulsewin\] open|\[orinrender\] strip=|\[deskcascade\] ->/' "$B" | cut -c1-160
```
PASS predicate: `-> CASCADED` present, ≥1 `[pulsewin] open win=N …` (the form on render2 was
`[pulsewin] open win=3 panel=1920x1200 surf=1280x168 box=1290x212 at (10,874) view=…`), no
`[pulsewin] open DECLINE`, no `[orinrender] strip=kept`, and the census field reads
`strip=retired` — AND Peter's eyes (B.3): no bottom strip, no embedded lamps, the pulse window on
the desktop. If the census token differs from the slot above, the field reads `n/a` and the awk
verdict is `NO-VERDICT` — fix the slot, re-run; do not score `n/a` as anything.
render3b control: `cascaded=1 refuse=0 pulsewin_open=0 pulsewin_decline=0 strip_kept=0 census_strip=n/a census_pulsewin=n/a -> FAIL no [pulsewin] open on the cascaded scene`
(correct for render3b bytes: the windowed pulse was retired, the strip was live — the R17 finding).

### C.6 Scorer — A17, two Print Screen presses, two verdicts, and a valid second PNG on the card

Ledger row: **A17**. Two halves: the wire (two `-> OK` lines, or one OK and a NAMED refusal) and the
card (the second file named on the wire is a PNG with signature + IHDR and size > 0).

```sh
awk '/PRTSCR: PrintScreen \(HID 0x46\) down/{armed++} /:: PRTSCR: SCREEN[0-9]+\.PNG [0-9]+x[0-9]+ [0-9]+ bytes -> OK ::/{ok++; match($0,/SCREEN[0-9]+\.PNG/); names=names" "substr($0,RSTART,RLENGTH)} /:: PRTSCR: .*capture (skipped|INCOMPLETE)/{ref++; refs=refs" | "substr($0,1,90)}
     END{printf "armed=%d ok=%d refusals=%d names=[%s ] -> %s\n",armed,ok,ref,names,(armed>=2&&ok==2)?"PASS (two files; now verify the second on the card)":(armed>=2&&ok==1&&ref>=1)?"PASS-BY-REFUSAL (named):"refs:(armed>=2&&ok==1&&!ref)?"FAIL A17 (second press: no verdict — the render3b signature)":(armed<2)?"INCOMPLETE (<2 presses seen)":"NO-VERDICT"}' "$B"
awk '/:: PRTSCR:/' "$B" | cut -c1-140
# the card half (card back in the reader, mounted at $MP): every name the wire printed `-> OK` for
MP=/run/media/pmes/UNAOS-ORIN
python3 - "$MP"/SCREEN*.PNG <<'EOF'
import os,struct,sys
for p in sys.argv[1:]:
    b=open(p,'rb').read(33); n=os.path.getsize(p)
    ok=len(b)>=33 and b[:8]==b'\x89PNG\r\n\x1a\n' and b[12:16]==b'IHDR'
    w,h=struct.unpack('>II',b[16:24]) if ok else (0,0)
    print("%s size=%d sig+IHDR=%s %dx%d -> %s"%(p,n,ok,w,h,"VALID" if ok and n>0 else "INVALID"))
EOF
cp "$MP"/SCREEN*.PNG "$SCR"/      # keep them beside the flight (6.9 MB each — scratch, not the repo)
```
PASS predicate: `armed>=2` and either `ok=2` with BOTH names `VALID` on the card, or `ok=1` plus a
named refusal for the second press (a refusal is a verdict; silence is the defect).
Control (orin.log from render3b's anchor 51791 — the PRTSCR lines fell after the committed
excerpt): `armed=2 ok=1 refusals=0 names=[ SCREEN0.PNG ] -> FAIL A17 (second press: no verdict)`;
card today: `SCREEN0.PNG size=6913793 sig+IHDR=True 1920x1200 -> VALID`, `SCREEN1.PNG size=0 … -> INVALID`.

### C.7 Scorer — A16, the RX discriminators are aboard and both legs were measured

Ledger row: **A16**. This flight's PASS is that the MEASUREMENT happened: the IIR/FIFO line printed
once, `ovrf=` is on every census, and both injection legs (burst, paced) have a window in the
capture. The MECHANISM verdict (which of REVIEW3 M2's hypotheses the numbers pick) is the decision
table in `docs/dev/evidence/orin14/A16-SCORE.md` — read the numbers below against it; do not
re-derive it here.

```sh
awk '/\[serialrx\] lsr=/{l++} /\[serialrx\] .*iir=/{ii++; il=$0} /\[serialrx\] rx=/{n++; for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0;ovs=1}}} /:: tegra: JD2 — KEY /{k++}   # <-- A16 TOKEN SLOT: `iir=`/`fifo=` once, `ovrf=N` on the census, per the A16 brief; re-check against serial.rs at $SHA
     END{printf "lsr_lines=%d iir_lines=%d census=%d rx_final=%d ovrf_final=%s keys=%d -> %s\n",l,ii,n,rx,(ovs?ov:"ABSENT"),k,(ii==1&&ovs)?"DISCRIMINATORS PRESENT — verdict per A16-SCORE.md":"DISCRIMINATORS ABSENT (A16 bytes not aboard, or the token changed — fix the slot)"; if(il) print substr(il,1,160)}' "$B"
# per-leg windows (L0 = orin.log lines before the burst, L1 = before the paced leg; from B.5/B.6 or marks.txt):
for leg in "burst $L0 $L1" "paced $L1 999999999"; do set -- $leg
  "$UNWRAP" "$CAP/orin.log" | awk -v a="$2" -v b="$3" -v leg="$1" 'NR>a && NR<=b && /:: tegra: JD2 — KEY /{k++; ks=ks" "$0} NR>a && NR<=b && /\[serialrx\] rx=/{for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0}}}
       END{printf "%s: keys=%d rx_after=%d ovrf_after=%d\n",leg,k,rx,ov; gsub(/:: tegra: JD2 — /,"",ks); print "   " ks}'
done
```
PASS predicate (this flight): `iir_lines=1`, `ovrf_final` not ABSENT, and both legs report
`keys`/`rx_after`/`ovrf_after`. Expected shapes, for the record: 5 bytes per leg (`t s t e \r`); the
render3b burst delivered 3 (`KEY 's'`, `KEY 't'`, `KEY 0x0d`, `rx=3`). A paced leg that delivers 5
where the burst delivers 3 is the between-polls hypothesis A16-SCORE.md already refutes for the
census rate — so it is the `ovrf` count and the IIR/FIFO line that decide, not the key count alone.
render3b control: `lsr_lines=1 iir_lines=0 census=22 rx_final=3 ovrf_final=ABSENT keys=3 -> DISCRIMINATORS ABSENT`
(correct: render3b bytes carry no discriminator).

### C.8 Liveness — terminus, heartbeat, no redzone, no exception (the frame every scorer sits in)

```sh
awk '/AARCH64: timer heartbeat live/{h++} /JM6b — EL1 landing: CurrentEL=1/{e++} /\[orinrender\] arm conwin=/{t++} /RENDER-ARMED/{ra++} /RENDER-LIVE/{rl++} /\[redzone\] .*LOW-REDZONE/{rz++} /Exception reason=|panicked at|PANIC:/{x++}
     END{printf "heartbeat=%d el1=%d arm=%d armed=%d live=%d redzone=%d exceptions=%d -> %s\n",h,e,t,ra,rl,rz,x,(h&&e&&t&&ra&&rl&&!rz&&!x)?"PASS":"INCOMPLETE"}' "$B"
```
render3b control: `heartbeat=1 el1=1 arm=1 armed=1 live=22 redzone=0 exceptions=0 -> PASS`.
(`[wc-x] console-window panic-fallback armed` is a witness that the fallback is armed, not a panic —
the pattern above does not match it.)

---

## D. What this flight ticks, and where the results go

| question | ledger row | tick on PASS | evidence file |
|---|---|---|---|
| C.3 CPU_ON ×5 | A15 | `fixed-unflown — 2 passes` (count; the row closes "after several passes") | `render4-boot1.log` |
| C.4 boot-stack post-cascade | A1 (already flown) | second §5.2 sample recorded in FLIGHT-RESULT; status unchanged | `render4-boot1.log` |
| C.5 strip gone, windowed pulse back | A18 | `flown` — with B.3's eyes-on note | `render4-boot1.log` + Peter's panel observation |
| C.6 second Print Screen verdict + valid PNG | A17 | `flown` | `render4-boot1.log`; `$SCR/SCREEN1.PNG` (scratch) |
| C.7 RX discriminators + paced leg | A16 | status per `A16-SCORE.md`'s table (`flown` if the mechanism is named; else `open` with the numbers) | `render4-boot1.log`; `$SCR/render4-paced-inject.out`; `A16-SCORE.md` |

The results file is `docs/dev/evidence/orin14/FLIGHT-RESULT-render4.md`, in the shape of
`docs/dev/evidence/orin13/FLIGHT-RESULT-render3b.md` (pinned anchor + file, one row per question,
excerpt line numbers as evidence). The ledger is ticked in the same commit (orin-ledger rule).
