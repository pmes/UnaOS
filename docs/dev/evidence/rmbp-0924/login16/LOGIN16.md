# LOGIN16 — the store's mount walks the ladder its guard reads (rmbp-ledger B217), 2026-09-24

Handed over by `docs/dev/evidence/rmbp-0915/flight14/FLIGHT14.md` §1 (bench, 8c92e4ac). Branch
`claude/optimistic-ramanujan-r3qyu5`; Peter mid-turn: "forget qemu we are booting metal" — so the QEMU
lane was stopped after its first pass and the proof below is what that pass printed, not a 240 s run.

## The defect, on the flight-14 wire

- `[7121ms] :: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 … this-boot ready-by=global=0 sdhc=1
  ahci=1 -> PASS ::` — LOGIN15 (B213) opened the guard.
- `try_load()` → `fat::mount()` → `mount_source(BlockSource::Default)` → `block::info()`: the GLOBAL
  slot, unset on the rMBP. `[61354ms] [users] el0-fat volume did not mount after 4096 passes —
  last=NoDisk — store unavailable this boot`. No `[users] load`, no root row, no set-password alert
  ("no pw alert came up"), Log Out refused `reason=storage-not-up`.
- The `mount()` doc string promised "the global, else the internal card". The code did the first clause.
  B213's ledger row repeated the promise as fact; corrected in that row.

## The fix (`unaos/crates/kernel/src/fs/users.rs`, file tail)

- `StoreVia { Global, Sdhc, Ahci, None }`; `store_via_from(global, sdhc, ahci)` — the pure ladder, the
  same precedence `block::program_source` gives programs; `store_via()` reads this boot's registries.
- `store_mount()` maps the rung onto `BlockSource::{Default, Sdhc, Ahci(ahci_port_at(0))}` and
  `mount_source`s it. Every users mount goes through it: `try_load`, `flush`, `ensure_home`, the home
  witness, and the logout refusal's `load_once` — the leaf read and the leaf written are one volume.
- `[users] load volume=el0-fat(rw) via=<rung> …` names the rung.
- Fixture `:: USERSMOUNT:` (witness; inside the USERSREADY once-latch): the ladder on the rMBP shape
  (→ sdhc), the QEMU shape (→ global), none (→ none), and the OLD mount — `Default`, the global slot
  only — on the rMBP shape (→ none) as the go-red. `this-boot via=` names the rung.
- `x86-login.spec`: REQUIRE the fixture's PASS line, FORBID its FAIL, REQUIRE `[users] load
  volume=el0-fat(rw) via=(global|sdhc)`.

## Proof — one QEMU pass of the login lane (build + first boot, then stopped on Peter's word)

Lane: `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_LOGIN=1
UNAOS_LOGINST=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`, serial `unaos/target/serial.log`:

```
:: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 qemu-shape(global=1 sdhc=1 ahci=0)=1 none=0 old-guard-on-rmbp=0 this-boot ready-by=global=0 sdhc=1 ahci=0 -> PASS ::
:: USERSMOUNT: rmbp-shape=sdhc qemu-shape=global none=none old-mount-on-rmbp=none this-boot via=sdhc -> PASS ::
[users] load volume=el0-fat(rw) via=sdhc src=none users=0 (fresh store) next_uid=10735767
```

This lane's boot had NO global disk (`ready-by=global=0 sdhc=1`) — the rMBP's shape — and the store
mounted through the sdhc rung, read-write, and loaded. On the old code this same boot would have printed
the flight-14 `did not mount after 4096 passes`. The go-red is the fixture's `old-mount-on-rmbp=none`
arm: it is the pure ladder evaluated with the old resolver's inputs and it names nothing.

AHCI arm: the bench image builds with `ahci` on (banner features), so the `StoreVia::Ahci` arm was
compiled under `UNAOS_AHCI=1` separately (`login16-ahci.log`, rc recorded in the STATE line).

## Not flown

Boot 15 (image 8) flies the sdhc rung on the metal. Read, in order: `:: USERSMOUNT: … this-boot via=sdhc
-> PASS ::`, `[users] load volume=el0-fat(rw) via=sdhc …` (a `(ro)` there is the SDHC write veto — the
store loads but cannot be written; say so), `[login] root password unset … -> set-password screen` and
the alert on the glass.
