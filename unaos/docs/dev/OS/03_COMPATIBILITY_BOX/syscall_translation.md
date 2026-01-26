# Syscall Translation: The "Universal Translator"

## 1. The Persona System
unaOS uses "Personas" to mimic different environments.
* **binfmt_misc:** The kernel detects the file type (PE/EXE, Mach-O, ELF) and automatically loads the correct Persona interpreter.

## 2. Dynamic Translation (JIT)
When a foreign app executes a system call (e.g., `NtCreateFile` on Windows):
1.  **Trap:** The CPU triggers an exception because that syscall ID doesn't exist in unaOS.
2.  **Translate:** The Compatibility Layer catches the trap.
3.  **Thunk:** We map the parameters to the unaOS equivalent (`jules::fs::File::open`).
4.  **Return:** We reconstruct the result into the format the Windows app expects (e.g., a Windows `HANDLE`).

## 3. Architecture Bridging (Rosetta-style)
If running x86_64 apps on ARM64 hardware (e.g., Pixel 10):
* We use **Ahead-of-Time (AOT)** binary translation during installation to convert x86 instructions to optimized ARM64 code.
* This ensures "Old Software" runs at native speed on "New Hardware."
