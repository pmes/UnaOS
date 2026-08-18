# PROPOSAL-igpu-pull6-power-on-battery

**STATUS: PROPOSAL — awaiting Peter's approval**

## M1: Power Instrument
The power instrument will be implemented in `unaos/crates/kernel/src/drivers/smc.rs` (read-only; no gmux/display writes).

**Logging Strategy (`:: PWR: ::` witness rollup):**
Instead of a simple average that hides outliers, the rollup will report raw counters every 10 seconds:
- `samples`: number of valid readings in this window.
- `sum`: total accumulated `mW` in this window.
- `min`: lowest `mW` reading seen in this window.
- `max`: highest `mW` reading seen in this window.

The reader will compute the mean (`sum / samples`) if needed, but the bounds (`min`/`max`) will expose any jitter or spikes.

**State Transitions and Unknowns:**
- The accumulator will track the `ac_derived` state. If the state changes mid-window (e.g. plugged in or unplugged), the current window will be immediately flushed and printed so that charging and discharging samples are strictly separated and never averaged together.
- Samples where `ac_derived` is `AcDerived::Unknown` will be excluded from the main math and counted in a separate `unknown_samples` bin.

**Sign Convention and Assumption:**
The implementation inherits the sign convention (`negative = discharging`, `positive = charging`) as an assumption from the existing `AcDerived` code comment, rather than deriving it freshly on metal. The witness log will explicitly label it: `(sign convention: inherited assumption)`.

**Healthy-but-Idle Reading:**
When the mechanism is healthy but the machine is idling, the `:: PWR: ::` witness should look something like:
`:: PWR: state=unplugged (discharging) samples=10 unknown=0 sum=-125000 min=-13000 max=-12000 (sign convention: inherited assumption) == rollup ::`
*(The actual baseline numbers will be provided by Peter's metal run).*

## M2: Scoping the iGPU Handover
The goal is to power down the discrete Kepler (GK107) and hand the panel over to the integrated HD 4000.
**Honest Scope**: The cheap win does not exist. Switching the gmux to the HD 4000 and powering down the Kepler would leave the panel black. Based on the previous sitting canon, the iGPU is all-dead at all four trace points, with pipes and planes disabled.
To successfully light the panel with the iGPU, a full bring-up is required:
1. Initialize the iGPU display pipe and plane.
2. Configure the eDP link.
3. Bring up the panel power via the iGPU registers.

## M3: Experiment Plan
This plan involves NO gmux writes or display engine writes yet.
1. Run the approved M1 code on metal with the 2012 rMBP unplugged (on battery).
2. Capture the serial output (the panel remains driven by Kepler).
3. Observe the `:: PWR: ::` rollup logs to determine the actual `healthy-but-idle` power draw.
4. Plug the machine into wall AC, and observe the `:: PWR: ::` output to see the charging power levels.
5. Report the established baseline idle-on-battery number so all future power savings (like the eventual iGPU handover) can be measured against it.
