# SERIAL2 — the 1136 merged lines were never the line lock

## Finding (boot 18, `rmbp-0915/flight18/f18-boot1.log`)
Census `merged_fixed=189 trunc=514 bypass=29`; `serial-merged.sh` = 1136. ALL 1136 are VUGART lines (first-head histogram: `1136 VUGART:`). Every one is cut at EXACTLY 204 bytes = 12-byte `logts` prefix + 192:
`[ 281277ms] :: VUGART: frames=1216 … waits_us=16835000 -> PASS ::` + `:: VUGART: frames=1216 …` (no `\n`, no timestamp between).
Cause: `user-vug/src/main.rs` `BUF_CAP = 192` on x86 (comment says TEAR+STRAND grew the line) — the verdict line with large counters is >192, `Buf::put` clips it and the final ` -> PASS ::\n` (incl. the `\n`) is lost; the next vug's line follows on the same wire line (`-> PA:: VUGART`). The kernel lock never saw it: one `SYS_WRITE` = one complete (clipped) write.
Classes: (a) no — witness lines are one `serial_println!`; (b) bypass=29 is real but 29 << 1136; (c) yes, ring-3 `>>>` writes class: user write path; FTDI ring is 1 MiB and drains whole (not the cause); (d) `trunc=514` is real (kernel lines >512) but those carry the `…` marker and are not the 1136.

## Mechanism / built
- M1 `serial_line.rs`: census `by_source=[emit:E,user:U,raw:R,ring:G]` (raw = SUBMITTED − emit − user; ring = FTDI tap dropped+torn). `serial-merged.sh` prints the first 10 merged lines with head1/head2/cut offset.
- M2 `LINE_MAX` 512 -> 2048 (x86 task stack 16 KiB, IST 8 KiB; panic path bypasses the buffer).
- M3 `emit_user(row, text)` (`serial_line.rs`): ring-3 `sys_write` (`arch/x86_64/syscall.rs`) buffers per process slot until `\n` (bound 1536) and emits each complete line as one `emit`. ROOT FIX: `BUF_CAP` 192 -> 256 in user-vug, and a clipped verdict still ends in `\n`.

## Written
Boot 19: `:: SERIAL: lines=… trunc=0 … by_source=[emit:…,user:…,raw:…,ring:…] -> PASS ::`; `serial-merged.sh` = 0. Pin: x86-default.spec SERIAL REQUIRE (trunc=0, by_source).
Not done: aarch64 `sys_write` still `serial_print!`; user-vug restage must be rebuilt (the vug binary embeds BUF_CAP).
