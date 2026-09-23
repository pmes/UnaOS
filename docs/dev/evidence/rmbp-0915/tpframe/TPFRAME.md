# TPFRAME: flight 12's vendor-multitouch frames, decoded and routed

Executor TPFRAME, branch `exec-rmbp-tpframe`, parent `978ca84e` (hw-rmbp), 2026-09-23. Ledgered as
rmbp-ledger **B197**, which follows up B139 (TRACKPAD) and B190 (MOUSEHALT). Mechanism and format table:
`docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §39.7. Flight-13 line list: §39.8.

## Corpus

`~/unaos-bench/scratch/rmbp-0915/logs/foldgate/f12-boot1.log` (6899 lines, image 4 =
`hw-rmbp@6d8d3d2d`, sha256 `2fb8f055…b49d`). The executor read a copy of it and did not modify it.
`awk 'index($0,"raw report #")'` finds 4 lines. One is the 2-byte `60 02` runt at 8345 ms. The other
three are the 58-byte frames at 234250, 234262 and 234267 ms. Those three are the whole corpus. The
fixture carries them verbatim.

## Derivation B (the corpus alone)

`derive.py` in this directory reads only those three frames and scans every le16 offset. Its output
is `derive.out`. It finds two meaningful relations:

- `le16@36 == 10 x delta(le16@32)` and `le16@38 == 10 x delta(le16@34)`, each on 2/2 consecutive
  pairs. These are rel_x/rel_y against abs_x/abs_y.
- `le16@8 == delta(le16@0)` and `le16@16 == 2 x delta(le16@4)` are coincidences of two constants
  (256 and 16), and are recorded as such.

Derivation A is the wsp.c TYPE2 table that the tree already carried. A and B agree at every offset B
reaches.

## Gates (all at the load this box had on 2026-09-23, 11–60)

| gate | result |
|---|---|
| `./arroyo check` at M1, M2, M3 | 84 cfg legs, 0 compile errors. Red only on GATE-BRANCH: this unregistered tip, plus foreign `origin/exec-rmbp-kvblank3` (report-not-touch) |
| `./arroyo test` (20 s wall) | TRUNCATED, void. The fixture line printed PASS, but that proves nothing about the run |
| `./arroyo test 180` at M2 | rc=0, `completion=complete` (line 1678), x86-default.spec green, `:: TPFRAME: … -> PASS ::` |
| go-red, M2 | abs_x/abs_y read big-endian in `decode_wellspring_type2`: `deltas_ok=false … d=-128/-128,-128/-128 … relx10=0/2 … -> FAIL ::`, rc=1. Source sha256 `f1ac05f8…2039` before the mutation and again after the restore |
| `./arroyo test 180` at M3 (pins in) | rc=0, `completion=complete`, both TPFRAME pins ✅ |
| REQUIRE go-red | TPFRAME line deleted from the M2 capture: rc=1. `mismatch_no=false -> PASS` substituted: rc=1 |
| FORBID go-red | a `-> FAIL ::` copy added after the PASS line: rc=1 on `FORBID :: TPFRAME: .* -> FAIL ::` alone (the REQUIRE stays ✅) |
| artifact | `LC_ALL=C grep -a -o -F` on `target/x86_64_esp/kernel.elf` finds each of these tokens once: `:: TPFRAME: frames=`, `] [tp] mt route=`, `] [tp] mt fingers=`, `] [tp] mode-mismatch readback=`, `(the wire beats the register)`. Present control `] [tp] mode wrote=` = 1. Absent control `[tp] mt route=vendor latched=yes` = 0, because it is composed at runtime |

## What only the glass proves

- The arrow follows the finger, fluidly, with no jump on re-touch.
- The Y direction is right.
- The 1:1 gain is usable.
- `ibt`@15 is the click.

The corpus has no click, no lift and no multi-finger frame.
