# PROPOSAL — iGPU Pull 4: gmux Indexed Protocol & Decode

STATUS: LANDED 6ee2eaf8 (2026-07-22 — full handshake with bounded waits both
sides, version self-test as a decode GATE (raw-only on unproven protocol per
A1), 6-reg ABI carried per A2, arch gate intact. Gates green, strings-proven
both artifacts. Metal owed: sitting #9.)
Prior: APPROVED WITH AMENDMENTS (2026-07-22 — handshake sequence, version
self-test gate, register set, decode table, and timeout fallback all accepted
as specified; port/index/protocol facts and decode values are consistent with
the known apple-gmux hardware facts. Amendments:
A1 — the version self-test is a GATE, not a row: if version reads implausible
(0x00/0xFF/identical bytes), print the raw bytes with a "PROTOCOL UNPROVEN"
marker and do NOT print decoded meanings for switch/power — raw only. No
decode output from an unproven protocol.
A2 — ABI law as listed, plus: the main.rs handoff keeps its
intel-ivb+x86_64 arch gate (dropped twice; the keep-this comment is in the
file), and the handshake timeouts are iteration-bounded like every poll on
this branch. Metal owed: sitting #9.)

## The Hardware Facts
Sitting #8 proved that the Apple gmux on this board ignores classic PIO reads but responds to the Indexed I/O ports. However, reading the `VALUE` port (`0x7C2`) immediately after writing to the `INDEX` port (`0x7D0`) yielded identical garbage values (`0x39` at boot, `0x03` in the kernel) across completely different registers (like `0x10` and `0x50`). 

This indicates the microcontroller requires a handshake to acknowledge the index and signal that the data is ready on the value port.

According to the open-source Linux `apple-gmux` driver facts, the indexed protocol utilizes a third port:
- **`GMUX_PORT_READ`** = `0x7D0` (Index Port)
- **`GMUX_PORT_VALUE`** = `0x7C2` (Data Port)
- **`GMUX_PORT_WRITE`** = `0x7D4` (Status/Handshake Port)

### The Indexed Read Sequence
A robust, read-only fetch of a gmux register involves two wait loops against the `0x7D4` status port:
1. **Wait Ready**: Poll `inb(0x7D4)`. While bit 0 is `1`, perform a dummy `inb(0x7D0)` to clear the pipeline, and delay slightly.
2. **Write Index**: `outb(0x7D0, REG_INDEX)`
3. **Wait Complete**: Poll `inb(0x7D4)`. While bit 0 is `0`, delay slightly. Once bit 0 flips to `1`, perform a dummy `inb(0x7D0)` to acknowledge.
4. **Read Data**: `val = inb(0x7C2)`

## Protocol Self-Test
Before we trust any display-switch decoding, we must prove the protocol works. We will read the gmux Version tuple:
- `VERSION_MAJOR` (`0x04`)
- `VERSION_MINOR` (`0x05`)
- `VERSION_RELEASE` (`0x06`)

If these three registers return plausible bytes (e.g. `[3, 2, 1]`), the protocol is proven. If they return `0x00`, `0xFF`, or identical garbage, the protocol failed.

## Registers to Read
We will read the following 6 registers at Point-0 (Bootloader) and Point-3 (Kernel Probe):
1. `0x04` - Version Major
2. `0x05` - Version Minor
3. `0x06` - Version Release
4. `0x10` - `SWITCH_DISPLAY` (Panel ownership)
5. `0x28` - `SWITCH_DDC` (DDC ownership)
6. `0x50` - `DISCRETE_POWER` (dGPU power state)

*Note: The `0x39` and `0x03` values observed in sitting #8 were the raw un-handshaked bytes lingering on `0x7C2`, reflecting a state change in the microcontroller but not the actual register contents.*

## Decode Table
Based on the `apple-gmux` driver definitions, the values for the switch registers decode as follows:

| Register | Value | Meaning |
| :--- | :--- | :--- |
| `SWITCH_DISPLAY` (`0x10`) | `2` | Integrated GPU (IGD) owns the panel |
| `SWITCH_DISPLAY` (`0x10`) | `3` | Discrete GPU (DIS) owns the panel |
| `SWITCH_DDC` (`0x28`) | `1` | Integrated GPU (IGD) owns DDC |
| `SWITCH_DDC` (`0x28`) | `2` | Discrete GPU (DIS) owns DDC |

## Implementation Plan (ABI Law)
1. **`unaos-boot-info`**: Replace `gmux_trace_0: [u32; 4]` with `gmux_trace_0: [u32; 6]`.
2. **`bootloader/src/main.rs`**: Implement the proper `gmux_index_read8()` handshake with a timeout fallback (to prevent an infinite hang if the microcontroller dies). Output the 6 registers at Point-0.
3. **`kernel/src/drivers/gpu/igpu.rs`**: 
   - Update `set_boot_traces` to accept the 6-element gmux array.
   - Implement the identical `gmux_index_read8()` handshake for the Point-3 kernel read.
   - Print the Version, Switch, and Power registers side-by-side (Point-0 vs Point-3), prefixed with `:: igpu: `.
4. **Verification**: Build with `UNAOS_IVB=1` and string-prove both artifacts.
