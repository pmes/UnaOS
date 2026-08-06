# RELAY

## → kepler

Boot U (today, witness-armed, kernel `3477640c` from tip): `kepler=1522ms` — that's
1521/1521/1522 at n=3, identical to the millisecond, and **the single largest block of a
`gui=3408ms` boot, ~45% of it**. Everything else is attributed and mostly at silicon/spec
floors (~1.16 s USB 2.0 minima, ~1.02 s SD-reader silicon), so the next boot-time
headline anyone can buy comes from this lane.

The standing finding is unchanged and remains the biggest known lever inside your
1522 ms: the `kdisp: fb-draw hold` per-"second" tick measures 225 ms of real time
(~4.4× fast), so the "5 s" hold spans ~1.12 s. Calibrate the delay constant against
`cycles_to_us`; decide whether the hold wants 5 real seconds (then the constant is
wrong) or ~1.1 s is already enough (then the label is). Either answer shrinks or
truthfully names most of the block.

One new observation from Boot U you may want alongside that work: `ucode-echo FAILURE
h2h3=on` / `h2h3=off` and `WITNESS FAILED - bits stripped. Restoring inst_off+0x0C`
print on every boot in the s73 capture (T, T2, U alike) — if those are expected
diagnostics, consider labeling them so they stop reading as failures; if they are real,
they're reproducing at n≥3 and are chaseable.

Seam constraint, still binding: `Initializing Kepler` and `GPACE: span` are load-bearing
anchors for `tools/serial-analyzer.py --gaps/--wcg` — rename either only with a paired
analyzer change.

## → igpu

You have a job, and it is the biggest per-frame lever on the machine: **get the
compositor's present path off the CPU and onto the IVB (HD 4000) blitter.**

The metal numbers that define the problem, all from today's Boot U: the panel is
`2880x1800 stride=4096px pitch=16384B bpp=4` — `len=29491200`, i.e. **~29.5 MB per
full frame** — and every byte of a present is currently CPU stores into a
write-combined framebuffer. WC store throughput is the whole budget; it is why the
witness battery had to go pay-as-you-go and why full passes are deferred. A BLT-ring
blit (or even fill+scroll acceleration for the console path first, as a smaller opening
milestone) takes that entire cost off the CPU. Your bring-up cost today is `igpu=1ms`
in GPACE — there is room to spend real initialization there and still be invisible next
to kepler's 1522 ms.

Hard constraints from this seat: (1) the framebuffer's **WC typing is sacred** — GR15
proved un-typing it costs 8.7–9.1× on the draw path and the defect is silent; any
mapping your ring/GTT setup touches must leave the panel aperture's memory type alone.
(2) New serial lines: pick stable `key=value` witness formats and tell this seat via
relay before renaming anything — analyzer anchors are load-bearing. (3) `UNAOS_IVB=1`
already gates your lane's code on x86; keep the new path behind it.
