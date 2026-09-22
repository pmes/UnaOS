# The composite blit bracket — what `[comp2] blit_us` actually measures

## BLITWIRE (SO45) — `blit_us` is not a blit rate

SO45 is the disagreement between SO31's prediction and render14 metal: DRAINCAP capped one serial
drain at `DRAIN_BYTE_BUDGET` = 192 B and predicted the drag-stall band would fall from 377.8 ms to
22.6 ms, and `[comp2] max_us` did not move (361 130 / 359 500 / 348 593 us on boots 1/2/5 and
367 166 us on boot 3). `serial_transport.md` §SERWIRE settles the transport half by arithmetic: the
cap IS on the metal path at every hop, one capped drain pays at most 115.1 ms even at render14's
widest line, and 361 130 us is 4 161 bytes — 1.05 whole staging rings. **One capped drain cannot
produce it.** This section takes the other half: what the compositor is doing for 361 ms.

The lead was that on that rollup `blit_us` reads 94 713 us of a 119 516 us pass — 79 % of it —
moving 3 053 700 B at **32 B/us** where the same boot's healthy rollups sustain **109**. The answer
is that those are not the same measurement.

### `blit_us` is the whole loop, and four witness sites live inside it

`comp2_emit` prints `blit_us` as `loop_cyc - cache_cyc` (`video/wm.rs`, the sixth argument of the
`[comp2] rollup` format), and `C2_LOOP_CYC` is charged with `now_cycles() - c2_loop0` at the CLOSE of
the whole per-window loop (`video/wm.rs:6413`). Between the open and that charge sit four sites that
move no pixel at all and exist only because the image was built with `witness`:

| hop | file:line | what it does | has its own field? |
| --- | --- | --- | --- |
| 1 | `video/wm.rs:6362` | `wcg::end` — reads the scan-out back, prints `[wc-g] win=` and `[wc-g] prof` | no |
| 2 | `video/wm.rs:6378` | `wcg::stage_flush` — prints `[wc-h]` / `[wc-k]` | no |
| 3 | `video/wm.rs:6382` | `band_flush` — prints `[wc-b]` | no |
| 4 | `video/wm.rs:6386` | `chromeband_fixture` — x86 only | no |
| 5 | `video/wm.rs:6398` | `verify_window` — reads the scan-out back, prints `[wc-d]` | **yes**, `wcd_us` |

So `bytes_pp / blit_us` is a copy rate ONLY on a span where none of them fired. The loop's own
comment at the `C2_LOOP_CYC` charge says they "never [perturb] the rollup average this line is read
for" because they are per-window-id one-shots. **That is false on exactly the rollup SO45 is about,
because that rollup is the boot's FIRST and every one-shot is still unfired.**

### The partition, over all 38 rollups of render14 boot 1

```text
  rollups carrying a WC-G sample or wcd_us > 0 :  r1  r2  r3  r4   (and r33, r38 later in the boot)
  bytes_pp / blit_us on r1..r4                 :  32.2  40.2  51.8  71.7   <- the four slowest of the boot
  bytes_pp / blit_us on the 32 witness-free    :  68.7 .. 163.0, mean 109
```

Measured with `awk 'index($0,"[comp2] rollup")'` over
`docs/dev/evidence/orin28/render14-boot1-desktop-menubar.log`, counting `[wc-g] win=` lines between
consecutive rollups. There is no rollup carrying a witness read-back in the healthy band and none
without one in the stall band, and the recovery is monotonic as the one-shots retire.

**It reproduces.** `render14-boot3-shutdown-rung4e.log` rollup 1 reads
`passes=4 max_us=361880 blit_us=94910 bytes_pp=3053700 wcd_us=41937` against boot 1's
`passes=4 max_us=361130 blit_us=94713 bytes_pp=3053700 wcd_us=41944` — the same 32.2 B/us to three
significant figures, on a different boot. Contention for the memory port and a mapping-attribute
fault do not repeat like that; a boot-phase structure does. Boot 3's OTHER peak, `max_us=367166` at
its rollup 5, also carries three WC-G samples.

### The arithmetic on the stall rollup, from the boot's own instruments

r1 is `passes=4 blit_us=94713`, so the loop bracket's total for the span is `4 x 94 713` =
**378 852 us**, and `max_us=361130` is **95.3 %** of it: one pass holds essentially the whole span's
bracket. Charged INSIDE that bracket and measured on the same wire:

```text
  [wc-g] prof win=1 seq=0   cks_blit 7339 + civac 7682 + cks_after 7345 + readback 5274 =  27 640
  [wc-g] prof win=1 seq=1             7335 +      7470 +           7338 +        5253 =  27 396
                                                            WC-G inside this span  =   55 036 us
  [comp2] wcd_us=                                           WC-D inside this span  =   41 944 us
                                                                             total =   96 980 us
```

`[wc-g] rollup win=1 ... wit_us=109867` is the four-phase sum over all FOUR of win=1's samples
(`wcg.rs:387`, accumulated at `wcg.rs:3901`), which is the independent check that 55 036 is the two
samples this span took. That is **25.6 %** of the bracket and **26.9 %** of `max_us`, and not one
byte of it is a pixel on glass.

It is a LOWER BOUND, because it excludes the loop's own UART. The span carries 1 686 B of
`[wc-g]` / `[wc-h]` / `[wc-b]` / `[wc-d]`, all printed from inside the bracket, which at 115200 8N1
(86.805 us/B, the rate §SERWIRE pins) is up to 146 353 us more.

### What is NOT proven, and why this ships as an instrument rather than a fix

```text
  witness subtracted, MEASURED terms only :  378 852 - 96 980  = 281 872 us -> 12 214 800 B = 43 B/us
  witness subtracted, WHOLE bracket modelled: 378 852 - 243 333 = 135 519 us ->             = 90 B/us
```

43 B/us is still 2.5x under the boot's own healthy 109 — the copy would still be slow. 90 B/us is
inside the healthy band — there would be no slow copy at all, only a wide bracket. **The wire cannot
separate those two today, because nothing charges the print sites.** Anything built on either
reading would be a guess, and the surviving candidates (destination mapping attributes, an alignment
or width fallback, memory-port contention with scan-out) all live on the far side of that gap.

### The instrument: charge the bracket, print one latched line

`C2_WIT_CYC` brackets the witness-only region of the loop — opened one statement after `draw_window`
returns (`video/wm.rs:6361`, folded onto `comp_mark(rw.id, 4)`) and closed on `wcn_note_drawn`
(`video/wm.rs:6406`), a straight-line region with no `continue`, `break`, `return` or `?` between
them. It is charged on EVERY drawn window, not only when a witness fires, because a counter charged
only when it is large cannot be read as a share; `C2_WIT_N` counts the windows so the total can never
be misread as a per-pass mean. Both are drained in `comp2_emit`'s existing sweep, on every rollup
whether the latch has spoken or not, which is what keeps the odometer a SPAN and stops it silently
becoming a boot total — the same ordering rule §SERWIRE's `wire_take()` follows.

### The constants, each with its reason

| constant | value | why |
| --- | --- | --- |
| `BLITWIRE_ARM_US` | `100_000` us | render14 boot 1's 32 witness-FREE rollups read `max_us` 43 218..85 871 and its four witness-carrying rollups read 91 082..361 130. 100 ms is above every witness-free `max_us` the boot recorded, is 6x the 16.667 ms frame, and is cleared by the stall band (348 593..367 166) by 3.5x. Lower and the latch spends itself on a rollup with no stall in it. |
| `BLITWIRE_FLOOR_BUS` | `64` B/us | the rate the witness-SUBTRACTED loop must sustain to acquit the copy. Boot 1's witness-free rollups sustain 68.7..163.0 and boot 3's 158.6..398.8; 64 is below every one of them and exactly twice the stall rollup's 32.2, so the verdict cannot read WITNESS-BOUND on a copy still running at the stall rate. |

Both rows are pinned by `const _: () = assert!(...)` in `video/wm.rs` and, like `serial_ring.rs`'s
truth tables and for the same reason, they are **not** `witness`-gated: they emit no code, and a
compile-time go-red that only fires in the configuration nobody ships is the polarity trap LAWS §5
names.

### What it costs a flight boot

The line prints **at most once per boot** — `BLITWIRE_SAID` is a one-shot latch — and is bounded at
**345 B**: 112 B of format literal and newline, 11 fields that cannot exceed 20 decimal digits each,
and a 13 B verdict word. On the render14 numbers it was shaped against it measures **166 B**.
`[comp2]` is not widened by one byte, and there is no per-pass and no per-rollup field: at ~140
rollups on a 700 s boot a per-rollup field is SO30 one layer up. The hot path pays two `now_cycles()`
reads and two relaxed atomics per DRAWN WINDOW, in a `witness` build only, and zero bytes of UART.

`[comp2] rollup` and `comp2_emit` have always been `#[cfg(feature = "witness")]` and the flight card
carries `UNAOS_WITNESS=1`, so the line reaches the glass boot; a witness-FREE `esp-jetson` build
correctly contains none of it, and both polarities are certified on the built ELF.

### Reading the next Orin boot

```text
[comp2] rollup passes=4 pass_us=119516 max_us=361130 ... blit_us=94713 ... bytes_pp=3053700 ...
[blitwire] arm max_us=361130 passes=4 loop_us=378852 wit_us=U wit_n=N wcd_us=41944 wit_pct=P bytes=12214800 raw_bus=32 net_bus=B floor_bus=64 -> WITNESS-BOUND | COPY-BOUND
```

* `-> WITNESS-BOUND` (expected `wit_pct` ~64, `net_bus` ~90): the bracket's excess IS the instrument.
  SO45 closes — there is no slow blit, only a wide bracket — and the follow-on is to give `blit_us` a
  companion field so no future reader divides bytes by it again.
* `-> COPY-BOUND` with a small `wit_pct`: the copy really is slow with the witness subtracted, and
  the mapping-attribute / alignment / memory-port candidates survive into the next arc with a
  measured budget (`net_us`) to explain instead of an unmeasured one.
* `wit_us` close to `loop_us` with `wit_n` near zero would convict the bracket itself — a
  desynchronised open and close — and is the one reading that means the instrument, not the
  compositor.

`serwire_selftest` and `blitwire_selftest` are independent: SERWIRE asks whether the ring drain can
account for `max_us`, BLITWIRE asks what the compositor spent it on. A boot that prints
`-> NOT-DRAIN` and `-> WITNESS-BOUND` has closed SO45 from both sides.
