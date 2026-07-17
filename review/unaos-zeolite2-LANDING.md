# ZEOLITE-2 — landing report

**Arc:** ZEOLITE-2 (R20, Maestro-spawned, Peter-picked 2026-07-17). **Branch:** `us-zeolite2`
(off main tip `534c55b`, the BATMON-1 merge). **Lane:** the ring-3 zeolite resolver blob + launcher in
`unaos/crates/kernel/src/arch/x86_64/syscall.rs`, `unaos/scripts/make-fat-img.sh` (`BLOCK.TXT`),
`unaos/docs/dev/OS/08_NET/networking.md`, `docs/SECURITY.md`, `docs/MILESTONES.md`. **No kernel-net
change, no new syscall (next free stays 28), no new ring-3 surface, zero aarch64.**

## Audit — slice 1 (SINKHOLE-1) vs the pitch

SINKHOLE-1 (merge `f33e340`, commit `e0169b6`) landed the ring-3 resolver: binds UDP `:53`, blocks
names with `0.0.0.0`, forwards the rest to `10.0.2.3:53`, hardened hostile-packet parser, blocklist
read from FAT via the STOR-1 S7 dynamic-open path. Against the pitch
(`plans/unaos/future/unaos-dns-sinkhole.md`), its blocklist was the one honest gap that lives
**entirely in-lane** and needs no kernel-net work: a toy format (one bare `UPPERCASE` name per line,
**exact whole-name compare**), with **no subdomain matching** and **no exposed counters**. Real
sinkhole lists (Steven Black hosts, AdAway) ship in **hosts-file format**, and a real sinkhole matches
**subdomains** — SINKHOLE-1 did neither, and the pitch's stats/admin VIEW had no data source. The
other gaps (aarch64/GENET NIC, cache/request-table, DHCP/kit) are out of lane (kernel-net / other
executors / future). So ZEOLITE-2 = the blocklist-ingest slice, which is also the best fit for the
arc's security lens (a blocklist file is hostile input).

## Scope delivered (3 milestones + gate)

- **M1 — hosts-format blocklist ingest** (`9bb6bed`). The `FILEBUF` walk in `zdns_parse_and_match`
  parses real hosts-file format: skips the leading IP field (`0.0.0.0`/`127.0.0.1`/any first token) to
  the DOMAIN (field-2 if a second whitespace-delimited field exists, else field-1 bare-name), drops
  `#`/`;` comment lines + trailing comments + blank lines, compares case-insensitively. `make-fat-img.sh`
  plants a realistic hosts-format `BLOCK.TXT`; the builtin fallback adopts the same format.
- **M2 — label-boundary suffix (subdomain) matching** (`5265345`). Blocks `www.ads.example` from a
  listed `ads.example` but NOT `notads.example` (tail compare; the byte before the match must be `.`).
  Two inline self-tests witness both directions (bits 8/9).
- **M3 — resolver metrics** (`23334a9`). Counts queries seen / blocked / forwarded, prints
  `:: zeolite: metrics — 4 queries seen, 2 blocked (sinkholed), 1 forwarded upstream ::` — the honest
  source a future stats view reads. No new syscall: counts saturate at 63 and pack into the witness
  word's spare high bits (`[10:28]`), clear of the `bit0..9` decision flags.
- **M4 — gate + docs + lens + this report** (this commit).

## Gates (verbatim)

- `./arroyo check` — both arches, knob on AND off: **green**, no new warnings.
  `Finished \`release\` profile` x2 (x86_64 + aarch64).
- Knob-off `./arroyo test 40`: **MISSION** — 19 demo PASS, 0 FAIL, **0 zeolite lines** (all code
  `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]`, byte-identical off/aarch64).
- Knob-off `./arroyo test-arm 22`: **MISSION** — `aarch64 test complete`, 0 FAIL/panic, 0 zeolite
  lines.
- Hermetic `UNAOS_SMOLNET=1 ./arroyo test 90` (builtin hosts-format list), 0 FAIL:
  `:: zeolite: hosts-format blocklist from builtin list (no FAT), blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), subdomain WWW.ADS.EXAMPLE sinkholed + NOTADS.EXAMPLE not over-blocked, forwarded una.os -> 10.0.2.3:53 real answer relayed — witness OK ::`
  `:: zeolite: metrics — 4 queries seen, 2 blocked (sinkholed), 1 forwarded upstream ::`
  `:: zeolite: resolver bound :53 — awaiting an inbound query (UNAOS_NET=socket net-inject dns) — witness PENDING ::`
- Full composition `UNAOS_SMOLNET=1 UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200` (hosts-format
  `BLOCK.TXT` from FAT via S7), STOR chain 0 FAIL:
  `:: zeolite: hosts-format blocklist from BLOCK.TXT via S7 dynamic-open, blocked ADS.EXAMPLE -> 0.0.0.0 (answer built), subdomain WWW.ADS.EXAMPLE sinkholed + NOTADS.EXAMPLE not over-blocked, forwarded una.os -> 10.0.2.3:53 real answer relayed — witness OK ::`
  + the same metrics line. Resolver blob still fits one code page (spawned; `assert!(blen <= PAGE_SIZE)`
  did not fire).

The over-the-wire SERVE leg is unchanged from SINKHOLE-1 (injector-driven; PENDING hermetically —
smoltcp has no loopback). The SINKHOLE-1 regression (knob-off no-line + hermetic forward OK + FAT
composition) plus the new subdomain/near-miss/metrics witnesses all hold.

## Lens — parsing / ingest hardening (security-relevant surface): **PASS, 0 must-fix**

Focus per brief: the new hostile **blocklist-file** parser (M1), the suffix-match boundary (M2), and
the (unchanged) hostile-**packet** parser re-exercised.

- **Every FILEBUF read is bounds-checked against `fend` (`r9 = FILEBUF + file_len`) before the access**
  — verified at each of: `skip_ws1`, `f1_scan`, `skip_ws2`, `f2_scan`, `dom_cmp` (loop guard
  `cmp rdx,rcx; jae`, with `rcx ≤ r9`), `toeol`, `eol_adv`. No unbounded read past `file_len` exists.
- **`file_len` is bounded ≤ 2048** (SYS_READ length cap; `> 0` checked before store) and FILEBUF is a
  2048-byte region ending exactly at RECVBUF — `fend` never crosses out of FILEBUF.
- **NAMEBUF reads are bounded by `r11 ≤ 250 < 256`**: `dom_cmp` reads `[r10+rdi]` for `rdi ∈ [offset,
  r11)`; the M2 boundary guard `[r10+rdi-1]` is reached only when `offset = r11−L ≥ 1` (`L ≤ r11`
  enforced by `cmp rax,r11; ja`, `L ≥ 1` by `test rax,rax; jz`), so the index is in `[0, r11−2]`.
- **Adversarial `BLOCK.TXT` traced, all fail-safe (skip, no crash, no read past buffer):** truncated /
  no-domain line, giant (2048-byte no-newline) line, all-comment file, CRLF-only file, embedded
  NUL/control bytes, a line ending exactly at `file_len`, leading-whitespace + comment, trailing `#`
  comment. A line the parser cannot resolve to a matching domain simply matches nothing — the failure
  mode is **under-block, never over-block or crash**.
- **M2 adds no new OOB surface**: exact-match (offset 0) skips the boundary read; the suffix path's one
  extra read (`NAMEBUF[offset−1]`) is provably in-bounds. Over-block guarded by the near-miss self-test
  (`NOTADS.EXAMPLE` must NOT block).
- **Packet parser + `0.0.0.0` response builder unchanged** from SINKHOLE-1 (compression-pointer
  refusal, label ≤ 63, name ≤ 250, fixed 16-byte answer) — re-exercised by the 3-label subdomain
  self-test. **Metrics packing** carries no attacker-controlled value (the resolver's own tallies,
  saturated at 63) and stays clear of the flag bits.

No MUST-FIX. Two benign observations, no fold: a blocklist entry with a **leading dot** (`.ads.example`)
or **trailing dot** (`ads.example.`) under-blocks (won't match the dotless wire name) — real hosts
files carry neither; the failure mode is safe (under-block).

## Flagged / residual (ledgered, honest)

- SERVE-over-the-wire remains injector-only (PENDING hermetically) — inherited from SINKHOLE-1, not
  this arc's scope; the guest-side sinkhole and the metrics serve-loop counters are deterministic.
- Still future (the appliance arcs, out of this lane): cache, query log, multi-in-flight forwarding,
  the stats VIEW itself (M3 now gives it a data source), dedupe/fetch-path, aarch64/GENET NIC, the kit.
- `copy_from_user` for socket/name buffers: the standing SOCK-2..7 deferred hardening, unchanged.
- **Metal-pending** — no wired NIC on any current board; QEMU slirp is the honest gate.

## Handoff

- Commits: `9bb6bed` (M1) · `5265345` (M2) · `23334a9` (M3) · this (M4 docs + landing) — all on
  `us-zeolite2`. Not merged, not pushed (integrator merges after review).
- Brief written: `~/.claude/plans/unaos/queue/unaos-zeolite2.md`. Future-doc status updated
  (`plans/unaos/future/unaos-dns-sinkhole.md` — second slice marked LANDED).
