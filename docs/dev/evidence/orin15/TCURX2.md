# TCURX2 — A16 rung 2: the TCU RX mailbox is read in the serial drain and CONSUMED

Ledger row: `docs/dev/OS/orin-ledger.md` A16 (the fix), A22 (rung 1, the probe). Executor TCURX2,
seat orin 15, 2026-09-06, branch `exec-orin15-tcurx2`, base `a05c2c8e` (hw-jetson tip). Knob:
`tcurx` (`UNAOS_TCURX=1`, implies `tegra` + `orinrx` + `tcuprobe`), DEFAULT OFF. Design this rung
implements: [`../orin14/TCURX-DESIGN.md`](../orin14/TCURX-DESIGN.md) §4 (the byte protocol) and §7
row 1 (the decision this executes).

## 1. The metal fact this is built on (render6, 2026-09-06T01:28Z, orin 15 seat)

Rung 1 (`tcuprobe`, read-only, A22) flew. Burst `tste\r` into the board:

* UARTC's RBR delivered `s`, `t`, `\r` — `KEY 's'`, `KEY 't'`, `KEY 0x0d`, `[serialrx] rx=3 (+3) …
  ovrf=0`. Three of five, `ovrf=0`, exactly render4's split.
* The probe printed `[tcu] rx-mbox raw=0x82006574 full=1 nbytes=2 data=[74 65 00] flush=0 hwflush=0
  | census=35 … full-edges=1 changes=1 last-full=0x82006574` and it STAYED full with that same word
  for the rest of the boot, because rung 1 deliberately never writes.
* Decode per TCURX-DESIGN §4: bit 31 full, [25:24] = 2 bytes, byte 0 = `0x74` `'t'`, byte 1 =
  `0x65` `'e'` — **exactly the two bytes UARTC lost**.
* The paced leg (50 ms/byte, ~20 s later) delivered ZERO bytes to UARTC and the mailbox word did not
  change. That is consistent with the §4 UNKNOWN "the SPE holds/drops while the RX mailbox is FULL
  and unconsumed" — it is NOT asserted here; rung 2 is what tests it, and the paced leg of render7
  is the test.

§7 row 1 of the design doc names the next step verbatim: *"TCURX rung 2: replace `serialrx::drain`'s
LSR/RBR poll with the mailbox read + write-back (§4); expect 5/5"*. This commit is that write-back —
with one deliberate departure, below.

## 2. What changed

**R19 (failed paths stay open) — the RBR poll is NOT replaced, it is JOINED.** The design doc says
"replace"; R19 says a path that failed under conditions keeps its code. So `drain()` now has TWO
sources feeding the SAME `Event::Key` queue and the SAME `RX` counter: UARTC's RBR first (untouched),
then the mailbox. One boot therefore scores both, and render7 can still see whether UARTC's share
changes now that the mailbox is being drained — a question "replace" would have thrown away.

| file | site | what |
|---|---|---|
| `unaos/crates/kernel/src/arch/aarch64/hsp_tegra.rs` | tail, `:335-425` (all `#[cfg(feature = "tcurx")]`) | `rx_mbox_take() -> Option<u8>` (the §4 consumer), `rx_mbox_took() -> u64` (the `mbox=` source), `TOOK` / `TOOK_TAGS`, and `w32` — the one write primitive |
| `unaos/crates/kernel/src/arch/aarch64/serial.rs` | `:780` | `#[cfg(feature = "tcurx")] mbox_drain();` folded onto `drain()`'s existing `witness_once();` line |
| same | `:830` | the `mbox=` census variant folded onto `census()`'s existing `let ovrf = OVRF.load(…);` line, with `#[cfg(not(feature = "tcurx"))]` trailing that line so it gates the ORIGINAL `serial_println!` that follows — the original statement's own lines and columns are untouched |
| same | tail of `pub mod serialrx`, `:834-862` | `mbox_drain()` — the `while let Some(b) = …rx_mbox_take()` loop that pushes each byte as `Event::Key` |
| `unaos/crates/kernel/Cargo.toml` | `[features]` | `tcurx = ["tegra", "orinrx", "tcuprobe"]` |
| `unaos/arroyo` | knob map + `KERNEL_CFG_MATRIX` | `UNAOS_TCURX=1` → `tcurx,tcuprobe,orinrx,tegra`; new leg `arm-tegra-tcurx` |

`mod.rs` and `main.rs` are **untouched**, as the brief requires: the three `serialrx::drain()` call
sites in `main.rs` already exist under `orinrx`, and the mailbox read went inside `drain`.

## 3. The word protocol as implemented (TCURX-DESIGN §4, `rx_mbox_take`)

```
raw = r32(RX_MBOX)                      // the address rung 1 resolved from the LIVE DTB
if raw & (1<<31) == 0        -> None    // nothing pending
n = (raw >> 24) & 0b11
if n == 0                    -> w32(RX_MBOX, 0); TOOK_TAGS += 1; None   // pure flush/hw-flush tag
b    = raw & 0xff                       // payload byte 0 is the next console byte
left = n - 1
word = 0                                    if left == 0
     = ((raw >> 8) & ((1 << 8*left) - 1))   otherwise
       | (left << 24) | (1<<31)
w32(RX_MBOX, word)                      // THE consume: this write frees the slot / advances the count
-> Some(b)
```

Worked against the render6 word, which is what render7 should reproduce:

| pass | `raw` | n | byte taken | `left` | word written |
|---|---|---|---|---|---|
| 1 | `0x82006574` | 2 | `0x74` `'t'` | 1 | `0x81000065` (byte 1 shifted to lane 0, count 1, bit 31 held) |
| 2 | `0x81000065` | 1 | `0x65` `'e'` | 0 | `0x00000000` (slot free — this is what tells the SPE) |
| 3 | `0x00000000` | — | — | — | — (bit 31 clear -> `None`, loop ends) |

Bounded by construction: every iteration either clears bit 31 or decrements the count, and a FULL
word with `n == 0` is consumed as a tag, so `mbox_drain`'s loop cannot spin on a stuck word. A tag
never injects a phantom key — it increments `tags=` instead.

**The entire write surface this knob adds is that one 32-bit store to the RX mailbox word.** No other
HSP register, no UART register (no FCR/IER/MCR — REVIEW3's warning stands), no doorbell, no IRQ
enable. If rung 1's DTB resolution failed, `ARMED` is false and `rx_mbox_take` returns `None` without
touching anything.

## 4. What the rung-1 sampler does now that a consumer exists

**It keeps sampling, READ-ONLY, unchanged. It never writes; consumption happens only in
`rx_mbox_take`.** The two race on one address, and the worst outcome of that race is that the sampler
prints a word the consumer has already replaced — it cannot lose a byte, because it does not consume
one. Expect its census line to read `full=0` most of the time with `tcurx` on: that is the fix
working, not an absence of input. `full-edges` / `changes` should still climb as the SPE refills.
**Do not score render7's RX by the sampler's `full=`** — `[serialrx] … mbox=` is the count that
answers "did the mailbox deliver".

## 5. On the wire

Per byte taken (off the `SERIAL_PORT` lock, so it is safe to print from `drain`):

```
[tcurx] took=0x74 't' left=1 word=0x81000065 <- raw=0x82006574 @ 0x3c10000 n=2 took-total=1 tags=0
[tcurx] took=0x65 'e' left=0 word=0x00000000 <- raw=0x81000065 @ 0x3c10000 n=1 took-total=2 tags=0
```

The census gains `mbox=` — appended after the existing fields, none of which moved or was renamed
(render4/6 scorers key on `rx=` and `ovrf=`):

```
[serialrx] rx=5 (+5) polls=… refused=… ovrf=0 lsr0=0x00000200 mbox=2 -> RX-LIVE (…)
```

`rx=` totals BOTH sources; `mbox=` is how many of those came from the mailbox. Knob-off the original
census line prints byte-for-byte as before.

## 6. Gate table (this tree, 2026-09-06)

| # | gate | command | result |
|---|---|---|---|
| 1 | check, both arches | `cd unaos && ./arroyo check` | **exit 0**. `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ kernel cfg coverage OK (49 legs)` with `✅ arm-tegra-tcurx` among them. `GATE-KNOB: OK — 158 features declared, 157 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg`. `✅ knob→leg coverage OK`. `GATE-LEDGER: OK — 78 rows in 2 ledger file(s) + RULINGS`. The one `warning: unreachable statement` in the log is pre-existing (`arch/aarch64/v3d.rs:1152`, from the `return` at `:1145`) and is not this arc's |
| 2 | aarch64 QEMU suite | `./arroyo test-arm 60` | **exit 0** — `✅ aarch64 test complete` |
| 3 | armed jetson media | render6 line + `UNAOS_TCURX=1 ./arroyo esp-jetson` | **exit 0**; banner `⚡ kernel features (jetson): …,orinrx,tcuprobe,tcurx,deskcascade`. In the ARTIFACT (not the diff): `grep -a -c '\[tcurx\] took' target/aarch64_esp/kernel.elf` = **1**, `grep -a -c 'mbox=' …` = **2**. kernel.elf sha256 `7589982f339ef93fd8c6499c49024ee581105ee04bde781634d62299f2cbc814` |
| 4a | knob-off jetson byte identity | same line WITHOUT `UNAOS_TCURX=1`, at `a05c2c8e` and with every edit applied; `llvm-objcopy -O binary kernel.elf` | **IDENTICAL** — loadable image sha256 `fd2fb251790ce9eb66804f776cacfc971ee22703fe2a8281079e278557ee6aa0` both before and after |
| 4b | the whole-file ELF delta, explained | `llvm-readelf -S` before vs after | before `bec22fd0280a1e85c660c9747ef57a088dd01f144e78fd51cdd0573425a00387`, after `e35fb4518a1aaeef54acce3274d1036a3e2abb3f91250b9a02cf46312d42d26c`. The ONLY section header that differs is `[19] .strtab` (`0x073d02` → `0x073cb1`); every ALLOC section keeps its address, size and offset, and the section-header table start moves by exactly that delta. This is the effect the `orinrx` knob comment in `Cargo.toml` already measured and documented — any byte changed in this LIB-crate file, comments included, renames ThinLTO `.llvm.<hash>` symbol suffixes in `.symtab`/`.strtab`. The loaded image is what boots, and 4a shows it unchanged |
| 4c | Pi byte identity | `./arroyo kernel8` at `a05c2c8e` and after | **IDENTICAL** — `target/pi_baremetal/kernel8.img` sha256 `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` both times (the same value the orin 14 TCURX commit recorded). `serial.rs` IS compiled on the Pi; the edit lives inside the `all(tegra, orinrx)` tail module and on two same-line folds, so no Pi-lexed line moved |

Line-neutrality, measured rather than asserted: `pub fn drain()` is still `serial.rs:775` and
`let ovrf = OVRF.load(…)` is still `serial.rs:830`, their pre-edit line numbers. Both folds sit
BEFORE their line's first `//` (LEDGER P7) — neither line has one.

Build logs (unversioned): `~/unaos-bench/scratch/orin15/tcurx2/`.

## 7. The flight question for render7

Boot with the render6 knob line **plus `UNAOS_TCURX=1`**:

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 ./arroyo esp-jetson
```

Inject `tste\r` twice with `~/unaos-bench/tools/inject-paced.sh` exactly as render4/6 did — BURST
(0 ms) and then PACED (50 ms/byte).

**Pass = 5 `KEY` lines on EACH leg** (`KEY 't'`, `KEY 's'`, `KEY 't'`, `KEY 'e'`, `KEY 0x0d` — the
shell sees `tste` and the CR runs it), `[tcurx] took=` lines accounting for whatever UARTC did not
deliver, `mbox=` climbing across the census lines, and `[tcu] rx-mbox … full=0` at rest instead of
render6's stuck `full=1 raw=0x82006574`.

| what render7 shows | reading |
|---|---|
| 5/5 on both legs, `mbox=` climbing, sampler at `full=0` | A16 is FIXED. The CCPLEX console input is the TCU RX mailbox; consuming it is all that was missing. The paced leg's render6 zero was the SPE holding while the slot was full |
| 5/5 burst, paced still short, `mbox=` climbing on burst only | the mailbox is the right source but the SPE's paced behaviour is a second effect — capture the `[tcu] rx-mbox` census across the paced window before theorising |
| `[tcurx] took=` never prints, `mbox=0`, sampler back to `full=1` stuck | the consumer never ran: check `[tcu] hsp …` resolved and `[tcu] sampler task spawned`, then that `serialrx::drain` is being called at all (`polls=` on the census) |
| bytes duplicated (a `KEY` twice) | UARTC and the mailbox are both delivering the same byte — the SPE mirrors rather than reroutes. Not seen on render6, and it would be a finding, not a regression: `mbox=` vs `rx=` separates the two sources |
| `tags=` climbing with `took-total=0` | the SPE is sending flush tags only — no console bytes are being routed to the CCPLEX channel; TCURX-DESIGN §7 row 3's enable/handshake question comes back |

## 8. Files
- `unaos/crates/kernel/src/arch/aarch64/hsp_tegra.rs` (tail block), `unaos/crates/kernel/src/arch/aarch64/serial.rs` (`:780`, `:830`, `serialrx` tail).
- `unaos/crates/kernel/Cargo.toml` (`tcurx`), `unaos/arroyo` (`UNAOS_TCURX` map + `arm-tegra-tcurx` leg).
- Facts come from the DTB, the public DT bindings and BSD-licensed edk2-nvidia only — no GPL NVIDIA
  driver was read (orin-ledger D3, CLEAN_ROOM_POLICY §6). Sources and licences: TCURX-DESIGN §2.
