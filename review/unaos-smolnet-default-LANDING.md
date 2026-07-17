# SMOLNET-DEFAULT — landing report

**Arc:** SMOLNET-DEFAULT (R20, Maestro-spawned, Peter's ruling 2026-07-17). **Branch:** `hw-rmbp`
(off `main` tip `e536a37`, the SINKHOLE-2/zeolite merge). **Lane:** x86 kernel net-stack default flip —
`unaos/arroyo`, `unaos/builder/src/main.rs`, the `smolnet` feature comments in
`unaos/crates/kernel/{Cargo.toml, src/smolnet.rs, src/arch/x86_64/syscall.rs}`, and the net docs
(`unaos/docs/dev/OS/08_NET/networking.md`, `docs/dev/OS/06_NETWORK_STACK/network_stack.md`,
`unaos/crates/net/README.md`). **Zero kernel .rs code change (comment-only), zero new syscall, zero
aarch64 perturbation.**

**Peter's ruling (verbatim intent):** "make smoltcp the default, retire the hand-rolled line but don't
shut out the possibility we resume hand-rolling our own."

## Chosen knob shape + justification

**Default-ON positive cargo feature `smolnet` + `UNAOS_NOSMOLNET` opt-out ENV, plus an aarch64
smolnet-strip so the flip is byte-identical on aarch64.**

- Kept the POSITIVE cargo feature `smolnet = ["dep:smoltcp"]` (cargo features are additive; "feature
  present = smoltcp compiled" stays semantically correct). Flipped only the ENV surface from opt-IN
  `UNAOS_SMOLNET` to opt-OUT `UNAOS_NOSMOLNET`, mirroring the established PORTSW-1/EHCI-4 default-ON +
  negative-knob idiom (precedent `arroyo:73` ehcihid). No legacy `UNAOS_SMOLNET` alias kept — do-it-right:
  our own doc/reproduce lines migrate; only EXTERNAL standards bind, not our own env names.
- **DISCOVERY (cost a bisect):** pushing `smolnet` to the aarch64 cargo invocation is NOT hash-identical,
  even though the aarch64 compiler emits ZERO smolnet bytes (x86-only dep + module; grep-verified zero
  ungated `feature="smolnet"` cfg). cargo hashes the ENABLED-FEATURE SET into each crate's `-Cmetadata`
  fingerprint (→ symbol manglings), so an aarch64 `unaos-kernel` built with `ehcihid,smolnet` (82331003…)
  differs BYTE-WISE from one with `ehcihid` (f77b5c64…) — same code, same size, same behavior, different
  bytes. The SOCK docs' "byte-identical knob-on aarch64" claim was CODE-identity, not hash-identity.
- **To honor "the flip must not perturb aarch64,"** `arroyo` gained an `arm_features` helper that strips
  `smolnet` from the two shared aarch64 kernel compiles (`build_kernel_aarch64`, `check_both`'s aarch64
  leg). x86 keeps the full feature set (smolnet default-on). `esp_jetson` is untouched (its tegra features
  flow through the strip → `ehcihid,tegra`); pi `kernel8` builds from its own curated `K8_FEATS` (never
  carried smolnet). Result, PROVEN end-to-end: aarch64 kernel default == opt-out == pre-flip `ehcihid`
  baseline (all `f77b5c64…`). `build_kernel_aarch64` also now echoes the effective (stripped) aarch64
  feature set so an operator doesn't misread `smolnet` off the x86 banner for an aarch64 build.
- The ehcihid/smolnet asymmetry (ehcihid remains in the aarch64 fingerprint) is deliberate: ehcihid was
  the PRE-FLIP baseline (out of this arc's lane); we only avoid ADDING a new perturbation.

## What happened to the hand-rolled line (never-trash-code)

**Retired as the default, NOT trashed and NOT removed.** Key finding: the hand-rolled `crates/net` crate
is an UNCONDITIONAL live dependency regardless of the knob — even smolnet-default, the driver's
`service_net()` poll, the boot connectivity self-test, DHCP, the TCP echo listener, the shell's
`connect`/`fetch`/`udpsend` (smoltcp has no shell equivalent yet — SOCK-8+), and `net::arp::learn` (reused
by smolnet's `Device` for MAC surfacing) all run through it. The `smolnet` feature is purely additive. So
"retire" here is a **default + status** change, not a code removal (removing it would break the default
build and regress those surfaces).

- **Kept in tree, compiling, live.** Catalogued in `unaos/crates/net/README.md` (new "Status: RETIRED AS
  THE DEFAULT — live, and available for resumption" section): disposition "available for reuse/resumption",
  still the whole opt-out stack under `UNAOS_NOSMOLNET=1`.
- `docs/dev/OS/06_NETWORK_STACK/network_stack.md` gets a retired-line banner → points at `08_NET` as the
  doc of record and at `crates/net` as the live, resumable opt-out stack.
- `unaos/docs/dev/OS/08_NET/networking.md` is the doc of record: rewrote the intro/two-stack/knob sections
  (smoltcp default, `UNAOS_NOSMOLNET` opt-out, arm_features strip + byte-identity), added a "retired
  hand-rolled line (resumable)" section, migrated every reproduce command off `UNAOS_SMOLNET` (0 stale refs).
- **Resume hand-rolling our own** = build `UNAOS_NOSMOLNET=1`. Proven to work (opt-out gate below).

## Commits (hw-rmbp)

- `096229c` — M1: the knob flip (arroyo + builder + comment-only kernel touches) + arm_features.
- `8e561b0` — M2: docs of record (08_NET) + retired-line banner (06) + hand-rolled catalog (crates/net README).
- M3 (this commit) — the effective-aarch64-features echo, the FAT-composition fresh-image doc note, and this
  landing report.

## Gate results (verbatim)

- **`./arroyo check`, both arches, both knob states:**
  - default → `⚡ kernel features: ehcihid,smolnet` → `✅ x86_64 OK` / `✅ aarch64 OK`
  - `UNAOS_NOSMOLNET=1` → `⚡ kernel features: ehcihid` → `✅ x86_64 OK` / `✅ aarch64 OK`
- **aarch64 byte-identity (the flip must not perturb aarch64):** aarch64 kernel via `./arroyo esp-arm`,
  default `= f77b5c64683b0a2a4c72a139224ec59eec28e9cd08b4964978be729248ec7525`, opt-out `= f77b5c64…`,
  == pre-flip `ehcihid` baseline `f77b5c64…`. Effective-features echo shows `aarch64 effective features:
  ehcihid` under the default. IDENTICAL — proven.
- **Default `./arroyo test 40` (smoltcp default path):** `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET
  ACQUIRED. <<<`. Witness the default IS smoltcp (all with NO `UNAOS_SMOLNET`): `:: SOCK-1: … witness OK ::`,
  `:: SOCK-2: … witness OK ::` + ring-3 udp round-trip PASS, `:: SOCK-3: …` + ring-3 tcp round-trip PASS,
  `:: SOCK-4: … PASS ::`, `:: SOCK-5: smoltcp dhcpv4 lease 10.0.2.20/24 …`, SOCK-6/7 PENDING (hermetic),
  `:: zeolite: … witness OK ::` + `:: zeolite: metrics — 4 queries seen, 2 blocked, 1 forwarded ::`.
- **Full x86 net regression under the new default** (`UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test
  200`, fresh fat-sf.img): **0 `-> FAIL`**, 28 `-> PASS ::`, S4 grow/create/delete PASS, U10 growth PASS,
  `:: zeolite: hosts-format blocklist from BLOCK.TXT via S7 dynamic-open … witness OK ::` (STOR feeds NET),
  full SOCK witness set. Matches the ZEOLITE-2 landing's "sf 200 STOR chain 0 FAIL, BLOCK.TXT via S7".
- **Opt-out `UNAOS_NOSMOLNET=1 ./arroyo test 40`:** `⚡ kernel features: ehcihid` → `MISSION SUCCESS`,
  **0** SOCK/zeolite lines (feature genuinely gated off), hand-rolled stack live (`[dhcp] bound: IP
  10.0.2.20`, `[selftest] gateway 10.0.2.2 reachable — ICMP echo reply`, TCP/UDP selftest).
- **`./arroyo test-arm 22`:** `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<` (serial-arm.log
  line 136), no FAIL/panic.

## Review lens (one lens — focus per brief)

PASS, 0 MUST-FIX. Focus areas:

1. **Default flip doesn't regress the SOCK line** — VERIFIED. test 40 default: every SOCK-1/2/3/4/5 witness
   OK + both ring-3 round-trips PASS + zeolite OK, firing by default. sf 200: 0 FAIL, zeolite BLOCK.TXT-via-S7
   composition OK. The composition is byte-for-byte the same kernel that landed at e536a37 (kernel .rs diff is
   comment-only — the flip changes only *which feature set is default*, not any compiled code).
2. **Opt-out path still builds + works** — VERIFIED. `UNAOS_NOSMOLNET=1` check green both arches; test 40
   MISSION on the hand-rolled path with 0 SOCK/zeolite lines and the hand-rolled DHCP + selftest live.
3. **Hand-rolled archive genuinely resumable, not orphaned** — VERIFIED. It is not merely archived; it is a
   LIVE dependency (backs connect/fetch/udpsend, TCP echo, DHCP, arp::learn, and the whole opt-out stack),
   catalogued (crates/net README), doc-pointed (06 banner → 08_NET; 08_NET → crates/net), and resumable via a
   proven `UNAOS_NOSMOLNET=1` build.

Other lens checks: no protection weakened (smoltcp + hand-rolled checksums untouched; flip is feature
plumbing + comments only); zeolite ring-3 blob in syscall.rs undisturbed (only a comment touched; its
witnesses fired correctly); `arm_features` strip verified on all feature-set shapes (incl. `ehcihid,smolnet,
tegra` → `ehcihid,tegra`, and it does NOT false-match `tegrasmp`).

**Fold (proactive clarity, not a MUST-FIX):** added the `build_kernel_aarch64` effective-features echo so
the aarch64 strip is visible in build output, not just documented.

## Flagged (for the integrator / next runner)

- **Shared docs carry stale `UNAOS_SMOLNET` prose (OUT of the named doc lane — integrator reconciles):**
  `docs/ROADMAP.md`, `docs/SECURITY.md`, `docs/MILESTONES.md`, and `review/unaos-zeolite2-LANDING.md` still
  mention `UNAOS_SMOLNET`. These are descriptive (not broken reproduce commands), but should migrate to the
  new default/opt-out framing when those shared docs are next touched. Left untouched to respect the lane.
- **Stateful-FAT trap (documented, caught):** `./arroyo test` with `UNAOS_FATIMG=sf` reuses `builder/fat-sf.img`
  as-is; a stale one (GROW.BIN pre-grown) trips U10/S4 with `grew_ok=false` (NOT a regression — the hazards-memo
  stateful-fixture signature). Rebuild the FAT (`bash scripts/make-fat-img.sh sf` / `./arroyo test-fat sf`)
  before the composition run. Noted inline in the 08_NET FAT-composition reproduce line.
- **Metal:** no NIC on the 2012 rMBP, so QEMU is the honest gate throughout (inherited from the SOCK line —
  not this arc's concern). The flip carries no new metal debt.
- **SOCK-8+ (future, out of lane):** fully retiring the hand-rolled SHELL surface (`connect`/`fetch`/`udpsend`)
  and the driver DHCP onto smoltcp — the remaining reason `crates/net` is still a live dependency, not just an
  archive.
