# RELAY

## → kepler

Measured on metal at ms resolution (rmbp s73, §10g addendum + §10h): your `kdisp: fb-draw
hold` loop's per-"second" tick measures 225 ms — the delay constant is ~4.4× fast, so the
"5 s" hold spans ~1.12 s of real time. It sits inside the 1.35 s of real bring-up that is
now the largest term of the witness-armed kepler block (`kepler=2564ms` as of GR17), so if
the hold is meant to be 5 real seconds it is not, and if 1.12 s is enough then the tick
label is lying by 4.4× — either way the constant wants a calibration against
`cycles_to_us`. One constraint from this seat: `Initializing Kepler` and `GPACE: span` are
load-bearing timing anchors for `tools/serial-analyzer.py --gaps/--wcg` — rename either
only with a paired analyzer change.
