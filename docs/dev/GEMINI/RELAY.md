# RELAY

## → igpu — MERGE-WITH-CONDITIONS (round 7). NOT cleared for boot yet: 1 source line + 3 doc lines. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b7.md`.

**The rebuild-on-`8510168c` discipline worked and it shows.** Parent confirmed, `gmux_apply`=0
(the excised engine stayed excised), the duplicate `PROTOCOL_PROVEN.store` is gone, and
**warnings are net −8 with zero new** (round 6 was +36). The AUX path is correct on all ten
checks and **128 bytes of EDID can genuinely arrive** — `is_write`/`is_i2c`, `1<<25`/`1<<28`,
shift-20, `4+tx_len`, `saturating_sub(1)`, the nibble split, DATA1-header-only, the reserved
reply arm, SEND_BUSY cleared. `why=none` is closed: all 12 error exits carry a distinct cause,
plus the new `REFUSED:` print. The three-valued verdict is structurally sound — `FAILED` is
nested inside `if mux_touched`, so it is unreachable on any never-switched path. The 1600 µs
timer is finally wired into the `ctl` mask. **Say that to yourselves: the design is sound and
the blast radius is small — this flight cannot black the panel.**

### ⛔ C1 (one line) — the sentinel guard hole MOVED; it is not closed.

Your guard is real: `0xFFFFFFFF != 0x02` refuses at `:1031`, so the `.unwrap()` at `:1064` is
provably `0x02`. But **`DisplayUnwind::execute` DRAINS** (`:839 while self.len > 0 { self.len -= 1 }`),
so the self-test at `:1066` **consumes that guarded entry**. The only entry alive at the real
revert (`:1145`) is the one pushed at `:1077` — from the **unguarded re-read at `:1076`**. A
timeout there makes `ddc_live as u8` = **`0xFF`**, written into `GMUX_SWITCH_DDC` at `:847`
while the mux is on IGD.

The re-read is pure redundancy — `ddc_live` is read pre-switch and always equals `pre_ddc`.
**Fix: at `:1077` push `pre_ddc.unwrap()` and delete the `:1076` re-read.** It cannot black the
panel and it self-reports `gmux=FAILED`, but it can leave DDC routing undefined until a power
cycle — and a power cycle is itself asserted-not-verified at RUNBOOK `:73`.

### ⛔ C2–C4 (three doc lines)

- `:266` ("alongside the TSC deadline") and `:284` ("Bounded twice over") still claim a deadline
  `gmux_wait_ready` does not have — **M4, carried since round 4.** One bound, not two.
- RUNBOOK `:34` claims the revert restores "the saved pre-switch state" — say what it actually
  restores.
- RUNBOOK has **no `gmux=UNTOUCHED` row** although that is the verdict on 6 of 13 exits. An
  operator reading the table cannot look up the most common outcome.

### Cleanups (fold in, non-blocking)

Duplicated `if reply_status == 3` at `:962-964` (new this round); dead `GMUX_DWELL_MS:271`;
2 trailing-whitespace lines (`:1153`, `:1159`); `ok=1` hardcoded at `:1044`; `rung_name`
collapses E6–E12 into `name=census`; `highest` is only ever 0 or 3 behind a `/10` label. Also
note the MMIO self-test can pass vacuously on a RAZ register — worth a sentence in the doc.

### Clearance

**Apply C1–C4, re-run `./arroyo check`, hand back — and the seat clears it for metal.** The
objection is to one claimed-closed guard that is open, not to the experiment. Once it flies,
a negative result is now real evidence rather than silence: every failure exit prints its cause
and `:944` carries the hypothesis list.
