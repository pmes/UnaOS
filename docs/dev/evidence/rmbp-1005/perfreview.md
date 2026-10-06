# PERFREVIEW (rmbp-ledger B443) — design

**Finding.** The wire carries timings and nothing reads them together; flights 24/25 read here say the render task is the
bottleneck: key→echo mean 237 ms (n=203, max 2960, `comp` mean 134 ms), click→shown mean 454 ms (n=112, max 3001), and 21
`render=handler` stalls over 1 s summing 64.5 s, every one beside a typed `tests` verb (the shell's dispatch runs on the
render task). Boot loader→desktop 11.6–13.2 s: 4 s of it is two bcm5974 feature-report timeouts (5/5 boots), 1.7–2.2 s
the association seed (BOOT80's FAIL), 1.0–1.3 s the root mount.

**Seam.** A reader, not a store: `perf.rs` (CHARTER Kernel — kernel-by-ruling) sums what `video/lag.rs` already measures
(the pass guard, the key's completion, the render loop's `S_CONSOLE` segment) and what `bootpace::boot_line` prints.

**Milestones.** M1 the `[perf]` line (d85a863e). F3 the per-pass session-name Strings (384db2e6). Base fix f2eedd6e
(merge17 dropped a brace). The rest are arcs: `docs/dev/review/PERF-2026-10-06.md`.

**Witness.** Once, when the session's services start (`boot::services_line`, beside `:: BOOT: phase=desktop`):
`[perf] frame_us=<mean pass> key_us=<worst key→echo> svc_idle_us=<mean service chain> boot_s=<n> passes=<n> keys=<n> svcs=<n> (B443: budgets frame 16667 key 1000)`.
`frame_us`/`key_us`/`svc_idle_us` are 0 off x86 `wc` (the lag instrument's `ON`).

**Owed.** The arcs named in the review; a second `[perf]` line at logout or every N minutes (one line at login reads the
setter/login screen only — the session's numbers wait for SHELLTASK's flight).
