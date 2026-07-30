# PROPOSAL-igpu-pull7-window-truth-and-panel-census

**STATUS: PROPOSAL — awaiting Peter's approval**

## M1: Fixes to the Power Instrument (smc.rs)
The `:: PWR: ::` witness will be updated to include strict window timing, total elapsed timing, and a clear inference disclaimer.

- **Elapsed Time & Boot-Cumulative:** We will track and log the actual elapsed ms of each window (`window_ms=...`). A global boot-cumulative `total_sum`, `total_samples`, and `total_ms` will be maintained, logging `(total: sum=X samples=Y time=Z)` alongside the window data so two boots can be directly compared.
- **Inference Disclaimer:** The 2012 rMBP lacks an independent AC key, requiring the `ac_derived` state to be inferred from current flow rather than a direct sensor. The witness will explicitly state this so readers do not mistake it for a measured AC state.
- **Sign Convention Bench Procedure:** 
    1. Boot the machine **on battery** and let the system reach idle (s59). This establishes the baseline draw of a normal boot at idle.
    2. Wait for **N=3** consecutive unplugged windows to print.
    3. Physically plug the wall AC in, and wait for **N=3** consecutive plugged windows to print.
    4. Physically unplug the wall AC, and wait for **N=3** consecutive unplugged windows to print.
    5. Capture the serial log.
    
    **Pre-declared Outcomes:**
    - **Outcome 1 (Correct):** `sum` is NEGATIVE during the unplugged windows and POSITIVE during the plugged windows. The inherited convention is correct.
    - **Outcome 2 (Inverted):** `sum` is POSITIVE during the unplugged windows and NEGATIVE during the plugged windows. The convention is backwards and we will invert it in Pull 8.
    - **Outcome 3 (Broken):** Any window's `min` and `max` straddle zero (containing both positive and negative samples). This means the state was mixed inside a window the flush was supposed to keep pure. This invalidates the reading (broken deadband or flush logic) and means the run says nothing.

## M2: Extended Panel Census and Reachability (igpu.rs)
No writes will be made. The read-only probe will be corrected and expanded to map out the prerequisites for a lit panel.

**1. Reachability via PCI Config Space**
Before touching any MMIO registers, we will read the GPU's PCI Configuration Space directly using its BDF (Bus/Device/Function):
- Vendor ID / Device ID (Offset `0x00`)
- COMMAND Register (Offset `0x04`, specifically checking Memory Space Enable bit)
- Power Management Capability (checking D-state)

This definitively distinguishes:
1. **Not Present:** Vendor ID reads `0xFFFF`.
2. **Present but BAR not decoding:** Vendor ID is valid, but COMMAND Memory Space Enable is 0.
3. **Present, decoding, but in D3hot:** Power Management state is D3.
If the device is not reachable and fully powered via PCI config, the MMIO census is meaningless and we will log this.

**2. Correcting the PCH Offset Families**
On Ivy Bridge (Gen7 with 7-Series Panther Point PCH), the display engine is split. `DP_A` is CPU-attached (North Display Engine). However, the Panel Power Sequencer and GMBUS families live on the PCH (South Display Engine), which adds a `0xC0000` base offset. Reading them at the pre-PCH `0x60000`/`0x5000` locations reads dead MMIO space, creating a false "all-dead" canon.
- **GMBUS:** We will read `PCH_GMBUS0..4` at `0xC5100..0xC5110` (Citation: i915 driver `intel_display_regs.h` / Intel PRM Vol 3).
- **Panel Power Sequencer:** We will read `PCH_PP_STATUS`, `PCH_PP_CONTROL`, `PCH_PP_ON_DELAYS`, `PCH_PP_OFF_DELAYS`, and `PCH_PP_DIVISOR` at `0xC7200..0xC7210` (Citation: i915 driver / Intel PRM Vol 3).

**3. FDI and eDP Architecture**
We will dump `FDI_RXA_CTL` and `FDI_TXA_CTL`, but with the citation that **eDP on Port A is directly CPU-attached on Ivy Bridge**. Therefore, the FDI link (which bridges CPU to PCH for PCH-attached display ports) is NOT on the eDP panel's path. A dead FDI link says nothing about a dark eDP panel. We read it only for complete architectural context.

**4. Additional Gaps**
- `DP_B` (0x64100), `DP_C` (0x64200), `DP_D` (0x64300) to confirm no alternative DDI routing.
- `FPA0` (0x06040) and `FPA1` (0x06044) to capture the DPLL divisors feeding the pipes.

**5. Deliverable: The Prerequisite List**
The output of this pull is this document mapping out the required states, supported by the serial dump. Once the true values are dumped, we will update this proposal with the precise "value now" vs "value needed", producing the ordered, reversible sequence of writes required for panel bring-up in subsequent pulls.
