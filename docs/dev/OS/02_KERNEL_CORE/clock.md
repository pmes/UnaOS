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

* **Log-timestamp adoption** — route serial/flight-recorder timestamps through `unix_now()` /
  `iso8601_now()` (falling back to `uptime`-relative while unsynced).
* **fs-mtime adoption** — converge `fat_stamp()`/`now()` onto the Unix anchor so a networked
  board stamps real mtimes from the SNTP sync (today the FAT anchor is operator-only). This is
  the arc that unifies the two anchors into one.
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
