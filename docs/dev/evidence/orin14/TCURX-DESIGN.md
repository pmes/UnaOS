# TCURX — the Orin console's RX belongs to the TCU mailbox, not to UARTC (design + probe)

Ledger rows: `docs/dev/OS/orin-ledger.md` A16 (fix path) and A22 (the probe; A21 is the tick1 image). Executor TCURX, seat
orin 14, 2026-09-05. Knob: `tcuprobe` (`UNAOS_TCUPROBE=1`, implies `tegra`), DEFAULT OFF, read-only.

## 1. The finding this answers

Render4 (A16, `FLIGHT-RESULT-render4.md`) determined the mechanism of the serial RX loss on UARTC
`0x0C28_0000`: `iir=0xc1 fifo=on`, `ovrf=0` on both legs, burst delivery 3/5 and PACED delivery 1/5.
Pacing made it worse, which only a **competing reader** predicts — a second consumer drains the same
RBR before the CCPLEX poll gets there. UARTC is the always-on-cluster port the SPE's Tegra Combined
UART (TCU) firmware multiplexes every processor's console onto. The competitor is the SPE.

The question this document answers: **does the CCPLEX legitimately own UARTC RX at all, and if not,
where is its console input?**

## 2. Sources — every fact carries one (no GPL driver was read: orin-ledger D3, CLEAN_ROOM_POLICY §6)

| tag | source | licence | what it gives |
|---|---|---|---|
| **LIVE** | the DTB the kernel parses (`JB1a — DTB @0x25f501000 size=0xcc90c`, `render4-boot1.log`) and the JB1b readout of the HSP block it names | n/a (board data) | the HSP top0 base, its dimensioning, the doorbell derivation that MRQ_PING proved |
| **BIND-HSP** | `Documentation/devicetree/bindings/mailbox/nvidia,tegra186-hsp.yaml` (public DT binding; fetched 2026-09-05 from kernel.org) | GPL-2.0-only OR BSD-2-Clause | `#mbox-cells = 2`; type cell bits [7:0] = class, bits [15:8] = shared-mailbox data-size flags; index cell bit 31 = direction (1 = producer/TX, 0 = consumer/RX), bits [23:0] = index; compatibles `nvidia,tegra234-hsp` (fallback `tegra194-hsp`) |
| **BIND-TCU** | `Documentation/devicetree/bindings/serial/nvidia,tegra194-tcu.yaml` (public DT binding) | documentation of the binding | `compatible = "nvidia,tegra234-tcu", "nvidia,tegra194-tcu"`; `mbox-names = "rx", "tx"`; example `mboxes = <&hsp_top0 SM 0>, <&hsp_aon SM 1>` — rx on top0, tx on aon |
| **EDK2** | edk2-nvidia (NVIDIA's own UEFI, the firmware that boots this board): `Silicon/NVIDIA/Library/TegraCombinedSerialPort/TegraCombinedSerialPortLib.c`, `Silicon/NVIDIA/Drivers/BpmpIpc/HspDoorbellPrivate.h`, `Platform/NVIDIA/Kconfig`, `Platform/NVIDIA/NVIDIA.common.dsc.inc`; fetched 2026-09-05 from `raw.githubusercontent.com/NVIDIA/edk2-nvidia/main/`, copies + sha256 in `~/unaos-bench/scratch/orin14/tcurx/` | **BSD-2-Clause-Patent** (SPDX line 9 of each .c) — permissive, not GPL | the HSP shared-mailbox stride, the TCU mailbox word format, the RX consumption protocol, and the addresses UEFI itself uses |
| **TREE** | `arch/aarch64/bpmp_tegra.rs`, `arch/aarch64/serial.rs` | this repo | the metal-verified doorbell derivation; the UARTC driver's own header note about `0x0C16_8000` |
| **NVDOC** | Jetson Linux Developer Guide r36.4.3, "Tegra Combined UART" (`AT/JetsonLinuxDevelopmentTools/TegraCombinedUART.html`) | public documentation | "The multiplexing is accomplished in the Sensor Processing Engine (SPE)"; the CCPLEX console is one multiplexed stream; nothing about input |

Not available to this seat: the Tegra234 TRM (login-gated) — every register fact below therefore
comes from EDK2/TREE and is marked so. UNKNOWN is used where no admissible source speaks.

## 3. HSP block layout (what is KNOWN, from which source)

| fact | value | source |
|---|---|---|
| HSP top0 block base | `0x0000_0000_03c0_0000` | LIVE: `/bpmp mboxes[0x125 …]` → phandle 0x125 `reg` = `0x3c00000` (`JB1b — geom: … hsp=0x3c00000`) |
| top0 dimensioning (`HSP_INT_DIMENSIONING`, block + 0x380) | `0x8a228` → nSM=8, nSS=2, nAS=2 (fields [3:0], [7:4], [11:8]) | LIVE readout; register offset + fields: EDK2 `HspDoorbellPrivate.h` (`HSP_DIMENSIONING 0x380`, bit-fields 4/4/4) and TREE (`bpmp_tegra.rs`) |
| common region size | 64 KiB | EDK2 `HSP_COMMON_REGION_SIZE SIZE_64KB` |
| shared-mailbox stride | 32 KiB (`1 << 15`) | EDK2 `HSP_MAILBOX_SHIFT_SIZE 15` |
| **shared mailbox i** | **block + 0x1_0000 + (i << 15)** | EDK2 (the two constants above). Cross-check: TREE's metal-verified doorbell region `block + (1 + nSM/2 + nSS + nAS) * 0x10000` = `0x3c90000` — the `nSM/2` is exactly "two 32 KiB mailboxes per 64 KiB step", and MRQ_PING answered over that doorbell on every Orin boot since 2026-07-06 |
| top0 SM0..SM7 word addresses | `0x3c10000, 0x3c18000, … 0x3c48000` | derived from the two rows above |
| doorbell registers | +0x0 trigger, +0x4 enable, +0x8 raw, +0xc pending; 0x100 apart | EDK2 `HSP_DB_REG_*`; TREE |
| HSP AON block base | `0x0c15_0000` — **derived, to be CONFIRMED by the probe's `[tcu] hsp aon=` line** | EDK2 Kconfig: TCU TX mailbox default `0x0C168000`; with BIND-TCU's tx = aon SM1 and the stride rows, `0x0C168000 − 0x10000 − (1 << 15) = 0x0C150000`. TREE's `serial.rs` header already names "~0x0C16_8000" as the TCU mailbox (it calls it a doorbell; it is the TX shared mailbox word) |
| registers inside one SM window other than the word at +0x0 (full/empty interrupt enables, the 128-bit data extension `nvidia,tegra234-hsp` adds) | **UNKNOWN** to this seat (TRM-only). The probe never needs them: EDK2 reads and writes the +0x0 word only | — |
| the `shared0..7` interrupt lines | present in BIND-HSP (`interrupt-names`), numbers UNKNOWN here; unused — the probe polls | — |

The question the brief posed — "shared mailbox N at base + 0x10000*(1+N)?" — is answered **no**: the
stride is 0x8000, so SM N is at base + 0x10000 + N*0x8000 (EDK2; consistent with the tree's
metal-proven nSM/2). For N = 0 the two candidates coincide, so the RX mailbox address is robust to
this either way; for the TX mailbox (N = 1) only the 0x8000 stride gives UEFI's `0x0C168000`.

## 4. The TCU byte protocol over the mailbox (EDK2, BSD)

The mailbox WORD (32 bits) carries up to three bytes:

| bits | meaning |
|---|---|
| [7:0], [15:8], [23:16] | payload byte 0, 1, 2 (byte 0 first) |
| [25:24] | number of valid payload bytes (0..3) |
| 26 | flush |
| 27 | hw-flush |
| [30:28] | reserved |
| 31 | data present — the mailbox FULL / interrupt bit |

**TX (CCPLEX → SPE)**: wait until bit 31 of the TX word is clear, write the word (payload + count +
bit 31); the SPE consumes and clears it. **RX (SPE → CCPLEX)**: poll bit 31 of the RX word; take
byte 0; **consume by writing the word back** with the count decremented and the remaining bytes
shifted down (or 0 when none remain). The consumer's write is what tells the SPE the slot is free.
UEFI's `SerialPortPoll` is exactly "bit 31 of the RX mailbox word".

What stays UNKNOWN (no admissible source): whether the SPE requires any enable/handshake before it
forwards keystrokes to the RX mailbox; whether the SPE **buffers** further bytes while the RX
mailbox is FULL and unconsumed, or drops them, or falls back to leaving them in UARTC's RBR (this is
the render5 question — §7); the on-the-wire multiplexing tags the SPE emits toward the host
demuxer (`nv_tcu_demuxer`, NVDOC) — irrelevant to the CCPLEX side.

## 5. The ownership question — answered

On Tegra234 with the SPE's TCU firmware running (it is: every capture line this track has ever read
came through it), **the CCPLEX does not own UARTC RX**. UARTC is the SPE's port; the CCPLEX's
console — output *and input* — is the pair of HSP shared mailboxes the DTB's `tcu` node names.
The firmware that boots this board reads its own keystrokes from `0x03C10000` (top0 SM0) and writes
its output to `0x0C168000` (aon SM1); it never touches UARTC (EDK2 Kconfig defaults +
`TegraCombinedSerialPortLib.c`).

Consequences:
- The A16 loss is not a bug in `serialrx::drain` and no UARTC register write (FCR/IER/MCR) can fix
  it; two readers on one RBR is a coin toss by construction. The FCR write REVIEW3 warned against
  would additionally reset the co-owner's FIFOs.
- The fix is to **stop reading UARTC's RBR and read the TCU RX mailbox instead**, consuming by the
  write-back in §4. One new write class (the RX mailbox word, CCPLEX-owned by the protocol), no
  UART register touched, no IRQ needed (poll bit 31 on the pump's existing cadence, exactly where
  `serialrx::drain` polls LSR.DR today).
- Output is the same story one rung later: our polled THR writes to UARTC interleave with the SPE's
  own TCU stream (they have worked so far because the SPE is quiet between our lines). Moving TX to
  the aon SM1 mailbox is the same protocol in the other direction — a follow-on arc, not this one;
  it changes what the bench demuxer sees (the CCPLEX stream becomes a tagged TCU channel), so the
  capture tooling must be checked before that flies.

**The alternative** — UARTA `0x0310_0000` / UARTE `0x0314_0000` (`serial.rs:45` fallback bases),
CCPLEX-owned 16550s with no competing reader: needs the debug cable moved off the button-header
TTL to the 40-pin header pins, `status = "disabled"` in the DTB, and clock/reset/pinmux/baud set up
through the BPMP. It trades a protocol port for a bench rewiring plus a BPMP bring-up. **Peter's
decision**; the mailbox path needs neither a cable move nor a BPMP call, and it is what the board's
own firmware does, so this arc designs and probes that path first.

## 6. The probe — `tcuprobe` (this commit), read-only

Files: `arch/aarch64/hsp_tegra.rs` (new, `#[cfg(all(feature = "tegra", feature = "tcuprobe"))]`,
declared on the `mod.rs` line that already hosts `selfup_tegra`/`ga10b_probe`), one call appended to
the console pump's spawn line in `main.rs::tegra_early_stop` (before the line's first `//`, P7),
`tcuprobe = ["tegra"]`, `UNAOS_TCUPROBE=1`, matrix leg `arm-tegra-tcuprobe`.

What it does, in order, at the pump's arm (EL2 boot core, DTB and RAM-GiB mask in scope):
1. Walk the live DTB for the first node whose `compatible` names `tcu`; read its `mboxes` (two
   `<phandle type index>` triples) and `mbox-names` (which entry is `rx` — read, not assumed);
   resolve both phandles to their HSP nodes' `reg` and `#mbox-cells`.
2. Print `[tcu] hsp top0=<rx HSP base> aon=<tx HSP base> tx-mbox=<i> rx-mbox=<i> | node=… rx=<path>
   cells=[…] tx=<path> cells=[…] #mbox-cells=… dir(rx)=rx dir(tx)=tx -> rx-word=0x… tx-word=0x…`.
   The raw cells are printed so the decode can be re-checked from the capture.
3. `[hsp] touching <base+0x380>` then read the RX block's dimensioning (the register `bpmp_tegra`
   reads on top0 every boot) and refuse (`[tcu] STOP`) if the RX index is outside nSM — the read of
   the mailbox window is bounded by the block's own census, not by trust in the type cell.
4. `[hsp] dim=… rx sm<i> @ <pa> — touching` then ONE read of the RX word: `[tcu] rx-mbox raw=0x…
   full=<0|1> nbytes=… data=[…] flush=… hwflush=… (arm sample)`.
5. Spawn `tcu-probe` (boot core, cooperative like `jd2-console`): every pass one read of the RX word,
   latching FULL rising edges, value changes and the last FULL word; once a second:
   `[tcu] rx-mbox raw=0x… full=… nbytes=… data=[…] | census=<n> polls=<p> full-edges=<e> changes=<c>
   last-full=0x… -> FULL-NEVER | FULL-SEEN | FULL-NOW`.

**No write anywhere**: not to HSP, not to UARTC, no IRQ enable. The consumer write-back of §4 is
deliberately absent — a byte the SPE parks in the mailbox stays parked, so the probe measures the
SPE's behaviour, not its own.

Knob-off: the jetson image is byte-identical (same-line appends only); the Pi never compiles `tegra`
and no Pi-lexed line moved — `kernel8.img` sha before/after in the commit body.

## 7. What render5/6 will say (the decision table)

Inject with `~/unaos-bench/tools/inject-paced.sh` as render4 did (burst, then paced), with
`UNAOS_ORINRX=1` still armed so UARTC delivery is scored on the same boot.

| `[tcu] rx-mbox` after injection | UARTC (`[serialrx] rx=`) | reading | next |
|---|---|---|---|
| `full=1`, `nbytes≥1`, `data[0]` = the first injected byte, `FULL-NOW` and it stays | fewer bytes than render4 or the same | **the SPE forwards RX into the mailbox** and parks it until consumed. The CCPLEX console input is there. | TCURX rung 2: replace `serialrx::drain`'s LSR/RBR poll with the mailbox read + write-back (§4); expect 5/5 |
| `full-edges` > 0 but `full=0` at census (`FULL-SEEN`) | any | something consumed the mailbox after the SPE filled it (a resident firmware reader? a UEFI runtime leftover?) | log the `last-full` word (it carries the byte); rung 2 still applies — but find the other consumer first |
| `FULL-NEVER`, `changes=0` | 3/5-ish as render4 | the SPE does NOT forward RX to the mailbox unprompted — an enable/handshake is needed (UNKNOWN, §4) or RX simply is not routed to the CCPLEX channel | flight question for render6: does a TX over the aon mailbox (one write) turn RX forwarding on? If not, the UARTA/UARTE cable move (§5) is the remaining path — Peter's call |
| `[tcu] STOP` at arm | — | the DTB's `tcu`/`hsp` shape differs from the binding — the STOP line names the field | read the printed cells; adjust the walker |

## 8. Files
- `unaos/crates/kernel/src/arch/aarch64/hsp_tegra.rs` — the probe (facts + provenance in its header).
- `unaos/crates/kernel/src/arch/aarch64/mod.rs:110`, `unaos/crates/kernel/src/main.rs:2495` — the
  two same-line appends. `unaos/crates/kernel/Cargo.toml` (`tcuprobe`), `unaos/arroyo` (mapping +
  `arm-tegra-tcuprobe`).
- Bench scratch (unversioned): `~/unaos-bench/scratch/orin14/tcurx/` — the fetched EDK2 sources
  with sha256, `fetch*.py`, build/check logs, `PROGRESS.md`.

## 9. Gate (this tree, 2026-09-05)
- `cd unaos && ./arroyo check` exit 0, both arches; leg `✅ arm-tegra-tcuprobe`; `GATE-KNOB: OK — 155
  features declared, 154 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg`; knob→leg
  coverage OK; ledgers OK.
- `./arroyo test-arm 60` exit 0 (`✅ aarch64 test complete`).
- `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1
  UNAOS_HOLOCRON=1 UNAOS_TCUPROBE=1 ./arroyo esp-jetson` exit 0; banner
  `kernel features: …,orinrx,tcuprobe,deskcascade`; `grep -a -c '\[tcu\]' target/aarch64_esp/kernel.elf`
  = 5, `'\[hsp\]'` = 2, task name `tcu-probe` present; kernel.elf sha256 `2e6695bee123aadf…`.
- Pi knob-off byte identity: `./arroyo kernel8` → `target/pi_baremetal/kernel8.img` sha256
  `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` at TIP `2a04fb4a` and again
  with every TCURX edit applied — identical (main.rs stays 8626 lines, mod.rs 505).
