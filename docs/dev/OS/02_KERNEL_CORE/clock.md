# The kernel clock (`crate::clock`)

UnaOS has no RTC it reads. Time comes from two honest sources — a free-running architectural
counter (always available) and, on a networked board, an SNTP sync (learns *civil* time). The
`clock` module turns those into two small, arch-agnostic services behind one monotonic seam.

## The arch monotonic seam

`monotonic() -> Option<(ticks, freq)>` is the single point where architecture enters:

* **aarch64** — `CNTPCT_EL0` / `CNTFRQ_EL0` (EL-independent, free-running, never stops — the
  same JD3 mechanism the BOT pump and screen-on-boot deadline ride).
* **x86_64** (CLOCK-X1) — the invariant TSC (`rdtsc`), calibrated once at boot against the ACPI
  PM timer (`apic::tsc_hz`), gated on CPUID's invariant-TSC bit (`apic::tsc_invariant`). A
  non-invariant or uncalibrated TSC returns `None` — the kernel never serves an untrustworthy
  counter as a clock.

Everything below reads time through this one function, so both clocks work on both arches with
no new plumbing.

## Two anchors, one seam

### FAT wall clock (JD17) — `set` / `now` / `fat_stamp` / `uptime_secs`

Operator-seeded (`setdate`), whole seconds since the **FAT epoch (1980-01-01)**, range
1980..=2107, 2-second `fat_stamp` resolution. Serves the FAT filesystem's mtime fields. `UNSET`
is first-class: `now()` is `None` and `fat_stamp()` is `(0, 0)` — the all-zero on-disk value
`ls -l` renders as dashes. The kernel never fabricates a reading. `uptime_secs()` reads the raw
counter independently of any anchor.

### FAT mtimes ride the unified clock (CLOCK-3)

`fat_stamp()` is the single stamping seam every fat.rs writer calls (create, grow, rename/move,
and a subdir's `.`/`..`). As of CLOCK-3 it derives from the **Civil (Unix) anchor** when one exists:
`unix_now()` → `civil_from_unix` → FAT packing. So an **SNTP-synced boot stamps REAL last-write
times with zero operator action** — the visible payoff of unifying the two anchors. It falls back to
the legacy JD17 FAT anchor (`now()`) only when no Unix anchor is set, and to `(0, 0)` when neither is
(the honest UNSET → dashed placeholder). Because `setdate` plants BOTH anchors from the same
monotonic tick, the derivation round-trips (`setdate D` → `fat_stamp()` yields `D`).

* **Timezone.** FAT stores wall-clock time with no offset; the civil clock is UTC, so CLOCK-3 stamps
  **UTC** (`civil_from_unix` is UTC). A boot synced under a non-UTC operator shows UTC in `ls -l`.
* **Range / edge honesty.** FAT cannot represent pre-1980 or post-2107. `fat_stamp()` **clamps**
  (never panics/wraps): a pre-1980 anchor pins to the epoch floor `1980-01-01 00:00:00`, a post-2107
  anchor to the last representable tick `2107-12-31 23:59:58`.
* **Granularity.** FAT time has **2-second** resolution (the low second bit is not stored) — the
  packed `sec/2` field, unchanged from JD17.
* **Witness.** `:: CLOCK3-fat: … PASS [w=0x07] ::` (aarch64 storage-chain, uncounted): after a
  deterministic Manual anchor (2020-06-15 12:34:56 UTC) a created file's dir-entry date reads
  2020-06-15; the civil clock is then restored to exactly as found (`clear_anchor` when it was unset).

### Civil clock (CLOCK-1) — `set_anchor` / `unix_now` / `iso8601_now` / `source`

The arch-agnostic civil clock the rest of the kernel wants. A single **Unix-second** anchor
paired with a monotonic reading and a `ClockSource`:

```
set_anchor(unix_secs, mono_now, source)   // the only writer; mono_now from mono_ticks()
unix_now() -> Option<u64>                  // extrapolated UTC Unix seconds, None while unset
iso8601_now(&mut buf) -> Option<usize>     // "YYYY-MM-DDTHH:MM:SSZ", None while unset
source() -> ClockSource                    // Unset | Manual | Sntp { stratum }
raw_anchor() -> Option<(u64, ClockSource)> // deterministic, non-extrapolated (gate use)
render_iso8601(unix, &mut buf) -> usize    // pure Hinnant civil + ISO renderer
```

`unix_now()` = `base_unix + (monotonic elapsed since the anchor)` — monotonic, non-hanging,
frozen at `base_unix` where no counter is available. Civil rendering uses Howard Hinnant's
days→civil algorithm (float-free, proven exact), moved here from PI-NET-16.

**Writers.** On the pi, the genet **SNTP client** (PI-NET-16, RFC 4330) anchors it as
`Sntp{stratum}` on each sync via forwarders (`wall_set` → `set_anchor`). On both arches, the
`setdate` verb also plants a `Manual` anchor (it does **not** touch the FAT anchor, so `date`
and `fat_stamp` are unchanged). x86 has no SNTP client yet, so `time` there reads `unsynced`
until a manual `setdate` — the seam is what CLOCK-1 delivers; x86 SNTP is a future rmbp arc.

## Shell

* `date` / `setdate YYYY-MM-DD HH:MM[:SS]` — the FAT wall clock (seeds mtime stamps).
* `uptime` — seconds since boot from the counter; appends the FAT wall clock when set.
* `time` — the civil clock: ISO-8601 UTC + source, e.g. `2026-07-22T15:30:45Z (sntp, stratum 2)`
  or `(manual)`; `unsynced` until an SNTP sync or `setdate`.

## Follow-ups (named, out of CLOCK-1 scope)

* **Log-timestamp adoption** — ✅ LANDED (CLOCK-2): an opt-in timestamp prefix on the serial log,
  monotonic-relative while unsynced and flipping to UTC once anchored. See "Opt-in serial log
  timestamps" below.
* **fs-mtime adoption** — ✅ LANDED (CLOCK-3): `fat_stamp()` derives from the Unix anchor when set
  (SNTP or `setdate` Manual), falling back to the JD17 FAT anchor, so a networked board stamps real
  mtimes from the SNTP sync. See "FAT mtimes ride the unified clock" above.
* **x86 SNTP client** (SNTP-X86, ✅ landed) — the x86 smolnet stack now has its own SNTP client
  (`crate::smolnet::sntp_sync_once` / `witness_tick_sntp`), so the x86 civil clock gets a network
  writer exactly as the pi/genet client does. One RFC 4330 client-mode request over the persistent
  UDP stack, parsed by the shared, hostile-input-hardened `crate::net_sntp` parser (ported from
  PI-NET-16; genet migrates onto it in a later fold), then `set_anchor(unix, mono, Sntp{stratum})`.
  Since smolnet has no DNS client yet (SOCK-8+) it targets the live default gateway (the DHCP-leased
  router, or slirp's 10.0.2.2). Success emits `:: SMOLNET: [sntp] <server> -> <ISO> (stratum N) ::`;
  the honest one-liner otherwise. The deterministic `sntp_x86_gate` (`witness` battery) proves the
  parser + anchor path with canned datagrams in any environment — `:: SNTP-X86-GATE: ... PASS
  [w=0x1f] ::`. On real rMBP hardware behind an NTP-answering router, `time` then shows synced time.

## Opt-in serial log timestamps (CLOCK-2)

The serial log can prefix every line with a compact, fixed-width timestamp so a captured log is
self-dating. It is gated entirely by the **`logts` cargo feature** (env **`UNAOS_LOGTS=1`**, wired in
`arroyo` *and* the builder — `arroyo` also arms it in the curated `K8_FEATS` for the kernel8 image).
**DEFAULT OFF**: with the feature absent the prefix path is not compiled and the serial byte-stream is
**identical** to a plain build, so the witness batteries and mbench specs (which parse serial lines)
are unaffected.

**Format (12 columns, aligned across the flip).**

* pre-sync (no civil anchor yet): `[  12345ms] ` — monotonic milliseconds since boot, right-justified
  in 7 columns. Live from the first print on aarch64 (CNTPCT always runs); `[      0ms] ` on x86 until
  the invariant TSC is calibrated (honest frozen zero, never a panic).
* post-sync (a civil anchor exists — SNTP, or a `setdate` Manual): `[15:04:07Z] ` — UTC wall time
  `HH:MM:SS`. The prefix flips the instant `set_anchor` plants the anchor.

**Where it hooks.** The prefix is inserted in `crate::logts::PrefixWriter`, a `fmt::Write` adapter the
two arch `_print`s wrap around the UART writer (aarch64 `SERIAL_PORT`, x86 `SERIAL1`) under the
feature. Only the **UART byte-stream** is prefixed — the fbcon console and every capture ring (`tste`
selftest, flight-recorder → `UNAOS.LOG`, FTDI) still receive the raw `args`, so those consumers are
unchanged. A per-stream line-start flag (a `Relaxed` `AtomicBool`, mutated only under the existing UART
lock — **no new lock**) gives a `serial_print!`-built line exactly one prefix regardless of how many
fragments compose it.

**Safety.** The prefix reads `clock::logts_now()`, which is lock-free for the monotonic part and
`try_lock`-only for the civil anchor (yields the monotonic form for that one line if momentarily
contended). So it never blocks and never panics — safe from early boot (before clock init), from
IRQ-masked handlers, and on every core.

**Witnesses need no exemption.** All serial matching is line-*unanchored* — mbench specs use
`re.search`, the `arroyo` gates use `awk '/…/'` — so a leading prefix does not break any witness. The
`::`-delimited verdict lines keep their prefix and still match. Verified: the `UNAOS_LOGTS=1`
`kernel8-test` battery is 23/23 PASS (CAPSTONE COMPLETE), and the x86 `UNAOS_LOGTS=1` `test` battery is
9/9 PASS with the prefix present on every line. The x86 battery also witnesses the flip at the exact
anchoring line: `[      0ms] :: [sntp-x86] reject …` → `[15:30:45Z] :: [sntp-x86] canned reply sets
clock => 2026-07-22T15:30:45Z PASS ::`.
