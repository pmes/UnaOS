# Architecture Specification: AArch64 (ARM64)

## Status: PRE-ALPHA / PLANNING

## 1. Device Tree (DTB) vs ACPI
Unlike x86, ARM targets often rely on Device Trees.
* **Strategy:** unaOS for ARM will prioritize **ACPI** (ServerReady/SystemReady compliance) to unify the boot flow with x86.
* **Fallback:** For mobile devices (Pixel, etc.), a "Shim Loader" will translate the proprietary DTB into a standardized ACPI format for the kernel.

## 2. Page Size
* **Target:** 16KB Pages.
* **Reason:** Apple Silicon utilizes 16KB pages for performance. To emulate macOS apps efficiently on M-series chips later, our kernel memory manager must support non-4KB page alignment natively.
