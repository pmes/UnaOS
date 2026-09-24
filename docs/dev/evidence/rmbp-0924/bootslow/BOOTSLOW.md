# BOOTSLOW: the root pass runs before the probes that do not serve it (B201)

Folded from the WIP tip the seat committed on 2026-09-23 (branch `exec-rmbp-bootslow`), 2026-09-24, cloud
session. Mechanism: rmbp-ledger B201; spec block: `x86-wc.spec` (BOOTSLOW). The executor stopped before
writing this record; the gates below are the fold session's.

## Gates

Recorded as they report: the x86 `wc` lane (`root-pass BOUND`, `root-bind d=`, `fixture n=1 … verdict=bound`)
and the go-red (`root_pass_open` returning `true` unconditionally).

| gate | result |
|---|---|
| `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test 240` on f5efe21a, then `mbench --spec x86-wc.spec` | mbench PASS 36/36 required, 0 forbidden (the first run, at 8de29731, was 36/36 with ONE forbidden hit: the healthy `verdict=bound` line's explanatory tail spelled `verdict=pending` — the wire text was changed in f5efe21a, the rule was not). The verb's own rc=1 is `:: SOCK-3:` (see FOLD.md), not this arc. |
| GO-RED: `root_pass_open` returning `true` unconditionally, same line | mbench FAIL 34/36, 2 forbidden hits: the fixture ran ahead of the root twice (`verdict=pending`, n=1 and n=2), `root-bind d=1306ms` fell outside `\d{1,3}ms`; file restored, `git status` clean |

## The wire (green)

```text
[vfs] root-pass HOLD probe=witness-fixture at=2685ms :: BOOTSLOW: this probe does not serve the root and waits for the root pass's verdict ::
[vfs] root-pass BOUND source=global match=/KERNEL.ELF at=3024ms source_seen_at=2686ms wait_ms=338ms pass=27 held=witness-fixture :: BOOTSLOW: the root is bound on its own pass; the probes named in hel
:: BPACE: root-bind t=3387ms d=35ms ::
[vfs] root-pass fixture n=1 held_ms=1200 at=4833ms verdict=bound :: BOOTSLOW: a synthetic probe that does not serve the root; bound is the ordering; a pending v
```

## The wire (go-red)

```text
[vfs] root-pass fixture n=1 held_ms=1200 at=2751ms verdict=pending :: BOOTSLOW: a synthetic probe that does not serve the root; bound is the ordering; a pending
:: BPACE: root-bind t=5782ms d=1306ms ::
```

q35 has no radio and no HDA tone on this lane, so `held=witness-fixture` is the only holder here; the metal's
`held=bt-campaign,hda` and the BOUND line's `wait_ms=` against flight 12's 25 s are flight 13's reading.

