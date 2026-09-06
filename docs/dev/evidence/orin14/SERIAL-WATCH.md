# SERIAL-WATCH — how the seat watches the Orin's wire during a boot (orin 14, 2026-09-06)

Peter's instruction at the close of orin 14: the next seat must know how to watch the serial line as
well as this one did. This is the exact method, with the commands as run on render4.

## 1. The instrument
- The board's UART (UARTC, the SPE/TCU combined port) reaches the host as `/dev/ttyACM0` through the
  debug probe. A **butler** (`line-butler.py`, started on the host; its stdout is
  `~/unaos-bench/capture/line-acm0/butler.out`) holds the port and appends every line to
  `~/unaos-bench/capture/line-acm0/orin.log`; lines it cannot attribute to a board go to `raw.log`
  and `unknown.log` (LEDGER P3: the boot's loader anchor may land in any of the three).
- **The authority for "is the butler alive" is the port, never the process list:**
  `flatpak-spawn --host lsof -t /dev/ttyACM0` prints its pid (34809 on 2026-09-05/06). Empty output =
  nothing holds the port = restart the butler BEFORE power-on (it RELEASES when the cable is unplugged;
  `pgrep -f 'line-butler'` matches its own shell and lies). Restart command: see `butler-start.out`.
- Injection INTO the board: `printf 'tste\r' > ~/unaos-bench/capture/line-acm0/inject.fifo` (burst) or
  `~/unaos-bench/tools/inject-paced.sh <fifo> 'tste\r' 50` (one byte per 50 ms; the A16 legs).
- Marks: every operator action gets a line in `~/unaos-bench/capture/line-acm0/marks.txt` with the
  UTC time and the capture offsets (`raw=$(wc -c < raw.log) orin=$(wc -l < orin.log)`), so a scorer
  can cut the window later. Shape: `MARK render4 518dca3 q-burst 'tste\r' injected at <UTC> raw=… orin=…`.

## 2. Watching live (the Monitor pattern)
The seat runs ONE background monitor over all three capture files with a SELECTIVE filter and reads
its events; a noisy filter is auto-suppressed ("output rate too high"), so the census lines that
print once a second are excluded once the boot is up. The command that watched render4's boot:
```sh
tail -n 0 -F ~/unaos-bench/capture/line-acm0/orin.log ~/unaos-bench/capture/line-acm0/raw.log \
     ~/unaos-bench/capture/line-acm0/unknown.log 2>/dev/null \
 | tr -d '\000' \
 | grep --line-buffered -a -E 'KELF min=|CPU_ON AP [0-9] ->|enumerated core 5|syndrome=0x82000010|Powering off core|RAS Uncorrectable|deskcascade\] (-> CASCADED|REFUSE)|\[pulsewin\] open|\[realdesk\]|serialrx\] (lsr|iir)|PRTSCR:|u7stk\] at=boot-core:post|panicked at|PANIC|line-butler released|\[redzone\]' \
 | sed -u 's/^\(.\{0,220\}\).*/\1/'
```
- `tr -d '\000'` first: the capture carries NUL bytes (the butler writes one when the probe drops);
  grep without `-a` and awk without the strip will misbehave on them. **Use `awk`, never bare `grep`, for
  anything scored** (CLAUDE.md); `grep -a` is acceptable only in a live filter.
- `-F` (capital) follows a rotated/recreated file; `-n 0` starts at the end so the previous boot's lines
  do not replay.
- Every terminal state is in the alternation: the A15 death signature (`syndrome=0x82000010`,
  `Powering off core`, `RAS Uncorrectable`), panics, the butler releasing the port. **Silence is not
  success** — if the filter would print nothing on a crash, widen it.
- Then narrow as the boot advances: once RENDER-LIVE holds, restart the monitor with only
  `PRTSCR:|panicked at|PANIC|line-butler released|\[redzone\]|syndrome=|KEY ` so the operator's presses are
  the only events. (`KEY ` echoes every keystroke Peter types at the USB keyboard — drop it once you have
  seen the keys arrive, or it floods.)
- The 80-column wrap: F11-menu boots inherit an 80-col console and the loader wraps long lines (D1);
  scoring goes through `~/unaos-bench/tools/unwrap80.sh <FILE>` (a FILE argument, never stdin — P2).

## 3. Reading back a window without the monitor
```sh
CAP=~/unaos-bench/capture/line-acm0
tail -c 400000 $CAP/orin.log | tr -d '\000' | awk '/CPU_ON AP [0-9]/' | tail -6          # last boot's APs
sed -n '54819,$p' $CAP/orin.log | tr -d '\000' | awk '/KEY |serialrx\] rx=[0-9]+ \(\+[1-9]/'  # from a mark's orin= offset
```

## 4. What the render4 boot looked like, in order (the shape to expect on render5)
`KELF min=0x0 max=<elf max> pg=…` (the anchor; pin the boot by THIS value, §C.1 of FLIGHT.md) →
`ORIN-SMP-3 enumerated core 5` → `CPU_ON AP 1..5 -> SUCCESS` → `[u7stk] at=boot-core:post-cascade …
hw=15584` → `[deskcascade] -> CASCADED windows=1 bar=1` → `[serialrx] lsr=… iir=0xc1 fifo=on -> RX-LIVE`
→ `[realdesk] backdrop=desktop-scene retired=pulse-band,status-line` → `[pulsewin] open win=2 … at (10,914)`
→ `[orinrender] census … strip=retired pulsewin=2 -> RENDER-LIVE` once a second, `[serialrx] rx=N (+d) …
ovrf=M` once a second → (inject) `:: tegra: JD2 — KEY 's' ::` … → (Print Screen) `capture armed` →
`SCREEN0.PNG 1920x1200 -> capturing (…)` → `SCREEN0.PNG 1920x1200 6913793 bytes -> OK` → (second press)
the same for SCREEN1 → power off → `line-butler released` may or may not print (the probe stays up).
render5 adds `[orinrender] arm … click=1`, `[orinclick] arm … -> ARMED`, and on a click
`[clickroute] press … -> CONSUMED` + `[orinclick] edge=…`.

## 5. After power-off: score, do not eyeball
FLIGHT.md §C: pin by the anchor's `max=` in whichever file carries it, extract the excerpt (first line =
the anchor), prove purity (`orin_marks>0 pi_marks=0`), then run the scorers verbatim
(`~/unaos-bench/scratch/orin14/scorers.run.sh` is the render4 instance; make the render5 one from
FLIGHT.md's code blocks the same way). Tick the ledger in the same commit as the excerpt.
