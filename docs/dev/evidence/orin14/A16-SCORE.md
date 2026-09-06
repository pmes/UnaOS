# A16 — RXDISCRIM scorer (render4)

Ledger row: `docs/dev/OS/orin-ledger.md` A16. Finding under test: render3b injected `tste\r` in one
write and the shell got `s`, `t`, `\r` (`rx=3` of 5). Overrun-between-polls is refuted by the census
(~325k polls/s, 3 µs/poll, vs 87 µs/byte at 115200). Two models remain: a **stall-time overrun** (the
pump's synchronous `KEY` echo holds `SERIAL_PORT` ~2.3 ms with IRQs masked, so bytes arriving during
the echo overwrite a 1-byte holding register) and a **competing reader** (UARTC `0x0C28_0000` is the
SPE/TCU combined-UART port; a second consumer draining the same RBR is a per-byte coin toss). The
mechanism is UNDETERMINED; this arc ships the three discriminators, and render4 decides.

**No FCR/IER/MCR write exists in this arc.** Both new witnesses are reads.

## What the image says (RXDISCRIM, `orinrx` knob, `serial.rs` tail mod `serialrx`)

| Wire | Where | Meaning |
|---|---|---|
| `[serialrx] lsr=0x… base=0x0c280000 iir=0x.. fifo=<on\|off\|odd(..)> -> RX-LIVE (…)` | `witness_once`, once, first poll that reached the port | `iir` = low byte of a single read of IIR (16550 register 2, `2 << 2` at the Tegra 4-byte stride). `fifo=on` = IIR[7:6] == 0b11 (FCR.FIFOE latched by whoever set the port up — firmware/SPE, never us); `off` = 0b00 (16450 mode, 1-byte holding register); `odd(01)`/`odd(10)` = a pair the 16550 does not define. |
| `[serialrx] rx=N (+d) polls=P refused=R ovrf=M lsr0=0x… -> …` | `census`, ~1 s cadence | `ovrf` = cumulative count of polls whose raw LSR word had bit 1 (OVRF) set. LSR is read on every poll and the read clears the bit, so one overrun event counts once. `rx` is cumulative bytes delivered to the PAL queue. |
| `:: tegra: JD2 — KEY 'c' ::` / `KEY 0x0d` | console pump echo, one per delivered byte | The byte the shell saw. The 5-byte payload `tste\r` should give `'t' 's' 't' 'e' 0x0d`. |

## The flight (two legs, same boot, board at RENDER-LIVE ≥ 30 s)

```sh
FIFO=$HOME/unaos-bench/capture/line-acm0/inject.fifo
LOG=$HOME/unaos-bench/capture/line-acm0/orin.log
T=$HOME/unaos-bench/tools/inject-paced.sh

# leg 1 — BURST (the render3b control, byte-for-byte: one write of 5 bytes)
L0=$(wc -l < "$LOG"); $T "$FIFO" 'tste\r' 0 | tee -a "$HOME/unaos-bench/scratch/orin14/a16/render4-inject.txt"
sleep 4; L1=$(wc -l < "$LOG")
echo "MARK render4 burst 'tste\r' at $(date -u +%FT%TZ) orin=$L0..$L1 raw=$(wc -c < "$HOME/unaos-bench/capture/line-acm0/raw.log")" >> "$HOME/unaos-bench/capture/line-acm0/marks.txt"

# leg 2 — PACED (one byte per 50 ms; the printed stamps are the true spacing, >= 50 ms)
sleep 6; L2=$(wc -l < "$LOG"); $T "$FIFO" 'tste\r' 50 | tee -a "$HOME/unaos-bench/scratch/orin14/a16/render4-inject.txt"
sleep 4; L3=$(wc -l < "$LOG")
echo "MARK render4 paced 'tste\r' 50ms at $(date -u +%FT%TZ) orin=$L2..$L3 raw=$(wc -c < "$HOME/unaos-bench/capture/line-acm0/raw.log")" >> "$HOME/unaos-bench/capture/line-acm0/marks.txt"
```

Each leg's window is `orin.log` lines `L0..L1` and `L2..L3` (write the four numbers down; the awk
below takes them as `a`/`b`). If the paced leg cannot follow the burst on one boot (the shell ate a
command and rebooted, or the echo drowned), run the paced leg first on the next boot and say so.

## Scoring (awk only — control bytes in the capture break grep)

```sh
LOG=$HOME/unaos-bench/capture/line-acm0/orin.log
U=$HOME/unaos-bench/tools/unwrap80.sh      # rejoin 80-column firmware wraps first, always

# 1. the one-shot witness: iir and fifo (once per boot; take the LAST boot's)
$U "$LOG" | awk '/\[serialrx\] lsr=/ { for (i=1;i<=NF;i++) if ($i ~ /^(lsr|iir|fifo)=/) printf "%s ", $i; print "" }' | tail -1

# 2. rx and ovrf across a window a..b (cumulative counters: delta = last - first inside the window)
score() { $U "$LOG" | awk -v a="$1" -v b="$2" 'NR>=a && NR<=b && /\[serialrx\] rx=/ {
    for (i=1;i<=NF;i++) { if ($i ~ /^rx=/) rx=substr($i,4); if ($i ~ /^ovrf=/) ov=substr($i,6) }
    if (!seen) { rx0=rx; ov0=ov; seen=1 } rx1=rx; ov1=ov }
  END { if (!seen) print "no census line in window"; else printf "rx: %d -> %d (delta %d)   ovrf: %d -> %d (delta %d)\n", rx0, rx1, rx1-rx0, ov0, ov1, ov1-ov0 }'; }
score "$L0" "$L1"      # burst leg  -> rx delta = bytes delivered of 5, ovrf delta = overruns seen
score "$L2" "$L3"      # paced leg

# 3. the KEY echoes in each window, in order (which bytes the shell saw)
keys() { $U "$LOG" | awk -v a="$1" -v b="$2" 'NR>=a && NR<=b && /JD2 .* KEY / { sub(/.*KEY /, ""); sub(/ ::.*/, ""); printf "%s ", $0 } END { print "" }'; }
keys "$L0" "$L1"       # expect for 5/5: 't' 's' 't' 'e' 0x0d
keys "$L2" "$L3"

# 4. sanity: the poll rate did not collapse in the window (refutes "between polls" again)
$U "$LOG" | awk -v a="$L0" -v b="$L3" 'NR>=a && NR<=b && /\[serialrx\] rx=/ { for (i=1;i<=NF;i++) if ($i ~ /^polls=/) p=substr($i,7); if (p0) printf "%d polls/s\n", p-p0; p0=p }' | sort -n | sed -n '1p;$p'
```

The burst leg's `rx` delta is the number to compare with render3b's 3; `ovrf` delta must be read on
the SAME window (the counter is cumulative from boot; a nonzero absolute value with a zero delta is
an overrun that happened earlier, e.g. during boot chatter from the SPE side — record it, do not
score it).

## Decision table

`b_rx` / `b_ov` = burst-leg rx / ovrf deltas; `p_rx` / `p_ov` = paced-leg deltas. Payload = 5 bytes.

| b_rx | b_ov | p_rx | Verdict | Next |
|---|---|---|---|---|
| 5 | 0 | 5 | **not reproduced** — render3b's loss was transient (butler/USB-CDC timing) | fly the burst three more times before closing A16 as flown-clean |
| <5 | >0 | any | **OVERRUN** — the holding register/FIFO was overwritten while the pump could not poll (the `KEY` echo stall is the prime suspect: ~2.3 ms under `SERIAL_PORT`, IRQs masked). `p_rx` = 5 confirms the stall window is the loss (bytes 50 ms apart clear it); `p_rx` < 5 with `p_ov` > 0 says the stall is longer than 50 ms, measure it | fix is in the PUMP (echo off-lock or deferred), NOT an FCR write; `fifo=off` makes the FCR question worth a separate, argued arc; `fifo=on` means a 16-deep FIFO still overran — the stall is far longer than modelled |
| <5 | 0 | 5 | **TIMING-DEPENDENT, no overrun flag** — pacing rescues the bytes yet the UART never flagged an overrun. Either OVRF is not implemented/visible in this LSR layout (`lsr0=0x200` already reads oddly — bit 8 RX_FIFO_EMPTY clear with no data), or the loss is upstream of the UART (the butler's `os.write` split, USB-CDC). Check the paced stamps against the echo timestamps | reproduce with a second burst; if `b_ov` stays 0 across three bursts, the LSR.OVRF bit is not a usable witness on this port and the pump-side stall fix is still the cheapest experiment |
| <5 | 0 | <5 | **COMPETING READER (SPE/TCU)** — loss is independent of pacing and the UART never overran: a second consumer drains the RBR before the CCPLEX poll gets there. Per-byte coin toss; expect ~50-60 % delivery on both legs | do NOT write FCR (resets the co-owner's FIFOs). Next arc: a UART the CCPLEX owns (UARTA `0x0310_0000` needs clock/reset/pinmux/baud) or TCU-mailbox RX; record the `fifo=` datum for it |

`fifo=` qualifies every row: `on` = a 16-deep FIFO is in play, so an overrun implies a stall ≥ 16 ×
87 µs ≈ 1.4 ms (the echo stall fits); `off` = 1-byte holding register, any stall ≥ 87 µs loses a byte
and a burst at line rate can only survive if it is polled between every pair of bytes (it is — 3 µs —
unless the pump is echoing). `odd(..)` = record the raw `iir=` byte and do not classify.

## Files
- image: `unaos/crates/kernel/src/arch/aarch64/serial.rs` — `serialrx::note_lsr` (OVRF count),
  `serialrx::witness_once` (IIR read, `iir=`/`fifo=`), `serialrx::census` (`ovrf=`); all inside the
  `#[cfg(all(feature = "tegra", feature = "orinrx"))]` tail mod, knob-off image byte-identical.
- injector: `~/unaos-bench/tools/inject-paced.sh <fifo> <string> [ms-per-byte=50]` (0 = burst).
- this scorer; the render4 capture goes to `docs/dev/evidence/orin14/render4-boot1.log` with its
  `marks.txt` lines and the injector's stamped output.
