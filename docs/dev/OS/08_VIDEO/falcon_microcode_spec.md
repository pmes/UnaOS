# Dirty Room Specification: NVIDIA Kepler (GK107) PGRAPH Microcode

> [!WARNING]
> **CLEANROOM FIREWALL NOTICE**
> This document was generated in an isolated "Dirty Room" environment via adversarial review of proprietary macOS binaries. It is intended to be strictly reviewed by an intermediary before being passed to a "Clean Room" development team. This document contains NO proprietary code—only structural observations, extraction strategies, and hardware interface specifications.

## 1. The Falcon Microcontroller

NVIDIA GPUs from the Fermi/Kepler era utilize custom 32-bit microcontrollers called **Falcon** (FAst Logic CONtroller) to manage complex engines. The `PGRAPH` block (which handles 2D, 3D, and Compute) is managed by one such Falcon processor.

When the GPU boots, `PGRAPH` is in a reset state. It refuses to process any commands from the PFIFO pushbuffer until its embedded Falcon microcontroller boots, runs an initialization routine to set up the internal pipeline state, and signals that it is ready.

## 2. Firmware Upload Interface (Host OS)

To load firmware into the Falcon, the host OS must upload the payload into the Falcon's Instruction Memory (IMEM) and Data Memory (DMEM) using MMIO registers mapped in BAR0.

### PGRAPH Falcon Registers (Base `0x400000`)
- **IMEM Upload**:
  - `NV_PGRAPH_FALCON_IMEMC` (`0x400180`): Instruction Memory Control (set upload offset here, e.g., `(offset >> 8) | 1 << 24` for auto-increment).
  - `NV_PGRAPH_FALCON_IMEMD` (`0x400184`): Instruction Memory Data (write 32-bit firmware instructions here).
- **DMEM Upload**:
  - `NV_PGRAPH_FALCON_DMEMC` (`0x4001C0`): Data Memory Control.
  - `NV_PGRAPH_FALCON_DMEMD` (`0x4001C4`): Data Memory Data.
- **Execution Control**:
  - `NV_PGRAPH_FALCON_BOOTVEC` (`0x400104`): Boot vector (starting instruction offset).
  - `NV_PGRAPH_FALCON_CPUCTL` (`0x400100`): CPU Control (Write `2` to start execution).

## 3. What the Microcode Must Do

To successfully wake up `PGRAPH`, the open-source firmware must implement the following behaviors:

1. **Pipeline Initialization**: The microcode must write specific values to internal, undocumented `PGRAPH` state registers to clear invalid states left by a hardware reset. 
2. **Context Switching (CTXPROG)**: When the host OS switches between different GPU channels (e.g., two different applications drawing at once), the Falcon microcode must save the 3D pipeline state of the old channel to VRAM and restore the state of the new channel.
3. **Interrupt Handling**: The Falcon must handle trapping illegal 3D commands, page faults (if using virtual memory), and synchronization fences, reporting them back to the host OS via the `NV_PMC_INTR` master interrupt tree.

## 4. Development Toolchain

An open-source toolchain exists for the Falcon ISA, created by the Nouveau project:

- **Envytools (`envyas`)**: An assembler that can compile Falcon assembly (`.fuc` files) into the binary format required by IMEM.
- **Instruction Set**: The Falcon is a 32-bit architecture with 16 general-purpose registers (`$r0` - `$r15`), a stack pointer (`$sp`), and a program counter (`$pc`). It supports standard ALU operations, branching, and a specific `iowr` / `iord` instruction pair for accessing the GPU's internal registers.

## 5. Proprietary Blob Extraction Strategy

An adversarial review of macOS 10.15 proprietary drivers reveals the exact locations of the Falcon microcode required for the 2012 rMBP (`0x0fd5`).

### Target Binary
- **File**: `NVDAResman.kext/Contents/MacOS/NVDAResman`
- **Format**: Mach-O 64-bit x86_64 bundle

### Identifying the Payloads
The primary resource management binary contains explicit firmware loader functions. Look for these symbol strings:
- `RMForceGrUcodeLoad`
- `RmPmuUcodeAddrMode`
- `_acrGetBinResInfoLoadUcodeImage_STUB`
- `_acrGetBinResInfoBsiBootUcodeHdr_STUB`

The microcode payloads are embedded as static byte arrays inside the `__DATA` or `__TEXT` segments. 

### Extraction Pipeline (Linux)
Since `NVDAResman` is a Mach-O binary, a Linux-based extraction script should:
1. Parse the Mach-O header using a library like Python's `macholib`.
2. Locate the function cross-references (XREFs) to `_acrGetBinResInfoLoadUcodeImage_STUB`.
3. Follow the pointers to the static `.rodata` or `__DATA` arrays.
4. Extract the byte arrays into `.bin` files.
5. The extracted binaries will consist of an Instruction segment (IMEM) and Data segment (DMEM) which can be disassembled using `disenvy`.

## 6. Next Steps for Clean Room Team

Once the intermediary (you) has reviewed and approved this document, it can be passed across the firewall. The Clean Room team can then use this document to:
1. Write the Python extraction script based on Section 5.
2. OR, use `envyas` to attempt to write a clean, open-source replacement firmware that satisfies the requirements in Section 3.
