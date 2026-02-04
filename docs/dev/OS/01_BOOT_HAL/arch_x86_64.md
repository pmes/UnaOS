# Architecture Specification: x86_64 (Ivy Bridge +)

## 1. CPU Feature Requirements
unaOS targets the `x86_64-v2` microarchitecture level or higher.
* **Required Flags:** `SSE4.2`, `AVX`, `POPCNT`.
* **Rationale:** The target hardware (Ivy Bridge i7-3720QM) supports AVX. Using these instructions allows for vectorized memory copies, significantly speeding up the "Clean Room" emulation layer.

## 2. Interrupt Handling (APIC)
* We bypass the legacy 8259 PIC entirely.
* **x2APIC** mode is enabled immediately to handle high-frequency inter-processor interrupts (IPIs).
* This is critical for the "Pervasive Multithreading" model (BeOS style).

## 3. The "No-SMM" Policy
System Management Mode (SMM) is a security risk (ring -2).
* The unaOS kernel attempts to lock down SMM configuration registers at boot.
* We do not allow ACPI calls to enter SMM if an alternative hardware interface exists.
