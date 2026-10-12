# SMALLFIX8 (rmbp-ledger B516) — what flight 28's two reds were

Source: `docs/dev/evidence/rmbp-0915/flight28/f28-boot1.log` (read with `awk 'index($0,"<tag>")'`).

## 1. HIDSTALL — the 483 ms holder, and the dock timeout

`[12:19:00Z] [hid] stall census holder=usb-pump:flight-recorder n=12 worst_ms=483` — the `usb-pump` step
`P_FLIGHT` (main.rs:6033: `flight_recorder::service(); bootwit::service(); pump_seg(P_FLIGHT)`). On a UnaFS
build `flight_recorder::service` hands its flush to `fs::bootlog::divert` (flight_recorder.rs:442), which every
4096 pump passes (~4 s; 12 a minute matches `n=12`) rewrote the whole snapshot (up to `RING_CAP` = 256 KiB,
flight_recorder.rs:89) into `/var/log/boot-<n>.log` with `mt.write` — one IRQ-masked UnaFS transaction on the
pump core (cpu 7), which the `hid-pump` shares. Cut: the flush moves to a `bootlog-flush` task on
`smp::xhci_worker_cpu(0)` (fs/bootlog.rs `off_pump`/`flusher`), the Dock Trash read's rule (HIDSTALL2 M2).

The dock timeout did NOT coincide with that hold. Every `[lag] stall` line from 12:18:31 to 12:18:48 reads
`hid_gap_ms=5..8 holder=-` — including at_ms=48045 (12:18:46, the timeout's second) and 49059; no stall line at
all prints between 12:18:49 and 12:19:00; a 483 ms hold would read `stage=hid hid_gap_ms>=483`. The census minute
accumulates from boot to its first roll, so the 12 flight-recorder holds were in the suppressed login window
(`boot_suppressed=21`, before 12:18:31). The timed-out press reads `pend=0 lane_ms=0`: no release was decoded
within 400 ms while the lane was healthy — a click held over 400 ms, or a release the decoder never pushed. The
`483` beside it is the click latency, `[lag] click→window-shown ms=483.1` = 400 (timeout) + 83 (`first_window_ms`),
a coincidence of numbers. hidstall.rs now times that press's release when it comes (`timeout_release_ms=`).

`hid_stall_s=1`: the only desktop-phase stall line with a HID gap over the 16 ms bound before 12:19:42 is
`at_ms=33730 stage=hid hid_gap_ms=56 pump=net-hda holder=-` (12:18:31) — unnamed (a gap between 16 and 50 ms prints
no line, so it is the candidate, not proven).

## 2. JOBSCAN — the chunk cost is per TRANSACTION, not per record

| chunk | records | ms |
|---|---|---|
| 1 | 398 | 92 |
| 2 | 0 | 90 |
| 3 | 0 | 103 |
| 4 | 1 | 92 |
| 5 | 100 | 80 |
| 6 | 8 | 76 |
| 7 | 522 | 98 |
| 8 | 160 | 76 |
| 9 | 168 | 119 |
| 10 | 24 | 76 |
| 11 | 176 | 87 |
| 12 | 151 | 87 |

An EMPTY directory costs 90 and 103 ms; 522 records cost 98 ms. Fit: (98 − 90) / 522 ≈ 0.015 ms per record, a
fixed cost of 76–103 ms per chunk (`read_dir_kinds` = `resolve_path` + `ls`, one `with_unafs` transaction,
fs/vfs.rs:1787). A 50-record chunk costs ≈ 76 + 0.8 ms: no chunk size meets 50 ms, and the chunk is one directory
blob — paging it is a UnaFS change (fs/vfs.rs / the unafs crate), outside this brief. STOP — see the report.
