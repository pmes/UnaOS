# unaOS Clean Room Design Policy

## 1. Objective
To implement compatibility with proprietary executables and hardware interfaces without infringing on copyright or intellectual property.

## 2. The Two-Team Rule (The "Chinese Wall")
To ensure legal immunity, contributors must self-identify into one of two groups for any specific feature implementation:

### Group A: The Reverse Engineers (White Box)
* **Role:** Analyze proprietary binaries, observe hardware behavior, and inspect input/output signals.
* **Output:** Detailed documentation and specifications (e.g., "When register 0x1 is set to 5, the GPU clears the screen").
* **RESTRICTION:** Group A members MAY NOT write code for the unaOS implementation of that feature.

### Group B: The Implementers (Black Box)
* **Role:** Write the actual code for unaOS.
* **Input:** Only the documentation provided by Group A.
* **RESTRICTION:** Group B members MAY NOT disassemble, decompile, or view leaked source code of the proprietary target.

## 3. Contributor Declaration
By submitting a Pull Request to unaOS, you certify that:
1.  You have not viewed stolen or leaked source code related to the component you are implementing.
2.  Your implementation is derived solely from public documentation or "Clean Room" specifications.
3.  You have not included any proprietary binaries, firmware blobs, or assets (images, fonts, sounds) owned by third parties.

## 4. Proprietary Assets
unaOS does not distribute proprietary software. All proprietary assets (BIOS files, ROMs, Drivers) must be provided by the user at runtime.
