# PROPOSAL — iGPU Pull 3: Point-0 and the gmux

STATUS: LANDED 2a4a92ff + 80ede042 (2026-07-22 — Point-0 at true bootloader
entry (before helpers::init), dual-style gmux dump per A1 (indexed 0x7D0/0x7C2
+ classic 0x710/0x750, byte sentinels), PP_STATUS/PP_CONTROL/DPLL_A added,
11-reg traces through boot-info per A2. Land-review caught one regression: the
intel-ivb+x86_64 gate on the kernel handoff was dropped AGAIN (same aarch64
break as pull 2) — restored in 80ede042 with a keep-this comment. Gates green,
strings-proven both artifacts. Metal owed: sitting #8.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — Point-0 design, PP_STATUS
(0x61200) / PP_CONTROL / DPLL_A (0x6014) additions, and sentinel discipline
all accepted; those PRM offsets check out. Amendments:
A1 — gmux protocol variant is UNPROVEN for this exact board: the indexed
scheme described (index→0x7D0, value←0x7C2) is one of two known gmux access
styles; the other is classic direct reads at 0x700+reg. Dump BOTH styles as
separate labeled rows (classic read of 0x710/0x750 alongside the indexed
reads of reg 0x10/0x50), read-only, sentinels on both — one boot then tells
us which style this gmux speaks instead of guessing.
A2 — boot-info grows a third array: same shared-ABI law as pull 2 (initializer
fields + BOTH builder sides arm the feature — the pull-2 land-review lists the
exact holes; re-verify each, then strings-prove kernel.elf AND BOOTX64.EFI).
Metal owed: sitting #8, rides with kepler pull 9.)

## The Paradox
In sitting #7, the iGPU trace read fully dead at all three points (including Point-1, immediately before ExitBootServices), yet the GOP text console was visibly on the panel during boot. This implies one of two scenarios:
(a) The GOP firmware tears down scanout *during* our bootloader's setup phase, before Point-1 is reached (i.e. the bootloader runs longer than assumed before hitting EBS).
(b) Scanout state is driven by a different display block/engine, or the Apple gmux routed the panel away from the iGPU entirely.

## 1. Point-0 Trace
To test hypothesis (a), we will add a **Point-0** trace to capture the iGPU state at the absolute earliest moment of our code's execution.
- **Location:** At the very top of `unaos/crates/bootloader/src/main.rs`, before *any* UEFI protocol calls or logging initialization.
- **Method:** We will use the existing `read_igpu_trace()` PCI Port I/O helper.
- **Storage:** We will add `igpu_trace_0: [u32; 8]` to `unaos-boot-info` (under the `unaos_ivb` feature gate) and have `igpu.rs` print it alongside the others. If Point-0 is live, we know the teardown happens inside our bootloader.

## 2. Apple gmux Readback
To test hypothesis (b), we will probe the Apple gmux microcontroller to definitively answer which GPU currently owns the panel.
- **Location:** We will read the gmux state at Point-0 (in the bootloader) and at Point-3 (in `igpu.rs`).
- **Method (Indexed I/O):** The 2012 rMBP uses an Indexed I/O scheme in the `0x7xx` port range. 
  - `GMUX_PORT_READ` = `0x7D0` (Index Port)
  - `GMUX_PORT_VALUE` = `0x7C2` (Data Port)
  - Register `0x10` (`GMUX_PORT_SWITCH_DISPLAY`): Controls DDC/Display routing (typically 1 = discrete, 2 = integrated, though raw value dump is sufficient).
  - Register `0x50` (`GMUX_PORT_DISCRETE_POWER`): dGPU power state.
- **Execution:** We will write the target register index (e.g. `0x10`) to `0x7D0` using `outb`, then read the result from `0x7C2` using `inb`. We will output these raw bytes in the boot trace and kernel probe.
- **Failed-Read Sentinel:** If `inb(0x7C2)` returns `0xFF` or `0x00` in a suspicious manner (or if we implement a timeout on an acknowledgment port and it fails), we will emit `0xBAD0BA20` (or `0xBA` for bytes) to distinguish a failed read from a valid zero.

## 3. Pipe-Adjacent Evidence (PP_STATUS / DPLL)
We will expand the MMIO trace to include the Panel Power and PLL registers, as an engine cannot scan with the panel power off. 
- **New Registers (Ivy Bridge PRM offsets):**
  - `PP_STATUS` (`0x61200`): Bit 31 is `PP_ON`, Bit 30 is `PP_READY`.
  - `PP_CONTROL` (`0x61204`): Panel power sequencing control.
  - `DPLL_A_CTRL` (`0x06014`): Display PLL A Control (Bit 31 is VCO Enable).
- **Execution:** We will add these three registers to our `read_igpu_trace()` array (expanding the trace size to 11, or keeping it separate). `igpu.rs` will print these alongside the pipe config.

## Standing Rules Compliance
- **Read-Only:** Zero state modifications. No writes to the GPU MMIO, and only read-intent index writes to the gmux index port (`0x7D0`).
- **Cleanroom:** All Intel register offsets (`0x61200`, `0x06014`) are cited from the Ivy Bridge PRM Volume 3. gmux ports are public hardware facts. No GPLv2 code used.
- **Bench Filter Law:** Every new print line will be prefixed with `:: igpu: `.
- **Gate:** Everything will remain behind the `UNAOS_IVB` feature gate to ensure a zero-delta default build.
