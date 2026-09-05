# FADTRESET — executor report (rmbp seat, x86_64)

**Branch** `exec-rmbp10-fadtreset` (worktree `/home/pmes/unaos-bench/scratch/rmbp10/exec-fadtreset/wt`)
**Base** `647f485a` (baseline: ancestor exit 0, toplevel + branch as required)
**Commit** `a5021ac4f0be336951fa4bd32d0d67173625a41d`
`x86/power: FADTRESET — wire the x86 reboot verb to the FADT RESET_REG, 8042 pulse behind it`
Not pushed (the seat never pushes). No stash, no /tmp.

## Files

| File | Lines | What |
|---|---|---|
| `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs` | 58–72 | new FADT constants: `FADT_FLAGS` (112), `FADT_RESET_REG` (116), `FADT_RESET_VALUE` (128), `FADT_FLAG_RESET_REG_SUP` (bit 10), `SDT_REVISION` (8), `GAS_SPACE_SYSTEM_MEMORY` (0) |
| same | 411–598 | FADTRESET section: `ResetReg`, `gas_space_name`, `table_checksum_ok`, `discover_reset`, `reset_settle`, `raw_witness`, `pub fn reboot() -> !` |
| `unaos/crates/kernel/src/power.rs` | 26–30 | header bullet for the x86 reboot mechanism (was "deliberately UNWIRED") |
| same | 141–148 | the x86 `platform_reboot` arm: witness + `crate::arch::acpi_power::reboot()` (mirrors the shutdown arm) |

aarch64 arms and `power::reboot()` / `power::shutdown()` are byte-identical (diff touches only the x86
bullet, the x86 arm and its doc). `shell.rs` untouched: `reboot` (shell.rs:4095) reaches the new arm
through the unchanged `crate::power::reboot()`.

## ACPI access (step 1)

Existing walker reused; none written. The UEFI bootloader passes the RSDP in `BootInfo::rsdp_addr`
(boot-info/src/lib.rs:98, ABI-locked offset 80); `kernel_main` hands it to `arch::acpi::init`
(main.rs:770), which retains it as `acpi::rsdp_addr()`. `acpi::root_sdt` decodes RSDP → XSDT/RSDT and
`acpi::find_table(.., b"FACP")` walks the entries — the exact path `acpi_power::discover` uses for S5.
Tables are identity-mapped; every access is `read_unaligned`. The existing walker does not checksum,
so `discover_reset` adds `table_checksum_ok` on the FADT (all bytes mod 256 == 0, length ≤ 64 KiB).

Facts required before the write, each refusal named in the witness: FADT present · checksum clean ·
header revision ≥ 2 · length ≥ 129 · Flags bit 10 set · GAS SystemIO (addr ≠ 0, ≤ 0xFFFF) or
SystemMemory (addr ≠ 0). Any other space is refused.

## The ladder (steps 2–3), witnesses in order

1. `[orinreboot] reboot verb invoked — dispatching the platform mechanism` (unchanged)
2. `[orinreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)`
3. either `[orinreboot] FADT RESET_REG space=<SystemIO|SystemMemory> addr=<hex> value=<hex> — writing`
   → `out` (SystemIO) / volatile byte store (SystemMemory), bounded spin; if still running:
   `[orinreboot] FADT RESET_REG write RETURNED — platform did not reset; trying 8042 pulse`
   or `[orinreboot] FADT has no RESET_REG — trying 8042 pulse (why: <reason>)`
4. 8042: bounded wait for input-buffer-empty, `out 0x64, 0xFE`, bounded spin
5. `[orinreboot] no reboot mechanism took on this platform (x86: FADT RESET_REG and the 8042 pulse both returned) — parking in hlt` → `hlt_loop`

LOCKFIX: past the arm's first line no lock is taken. Witnesses 3–5 use `serial::raw_write_str` /
`RawUart` (the audited lock-free 16550 primitive), not `serial_println!` — `_print` is `try_lock` +
deferral, and a witness deferred into the staging ring on a path whose next instruction resets the
machine is lost. The ring is drained first; interrupts are masked. Consequence: lines 3–5 are
serial-only (no fbcon / flight-recorder / FTDI mirror).

## Gates

    ./arroyo check                exit=0  ✅ x86_64 OK ✅ aarch64 OK ✅ kernel cfg coverage OK (37 legs)
                                          ✅ userspace x86_64 (4) ✅ userspace aarch64 (5) ✅ midden_core
                                          ✅ arch families ✅ knob hygiene
    UNAOS_WC=1 ./arroyo test 60   exit=0  banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`;
                                          no [orinreboot] line in a default boot; ✅ Test run complete.

Outputs: `check.out`, `test-default.out`, `test-selfinvoke.out`, `serial-default.log`,
`serial-selfinvoke.log` in this directory.

## QEMU proof

The x86 headless harness cannot type: the builder attaches COM1 as `-serial file:` (write-only) and
the kernel has no serial-RX→shell pump on x86 (main.rs:1809's `poll_input` loop is aarch64-only;
`arch/x86_64/serial.rs` has no read path; the pi's `UNAOS_K8_SCRIPT` typist has no x86 twin). So the
proof used the brief's sanctioned fallback: a TEMPORARY `option_env!("UNAOS_FADTRESET_SELFTEST")`-gated
call to `power::reboot()` right after `acpi::init`, default OFF, removed before the commit
(`git show --stat HEAD` lists only the two files above).

`UNAOS_WC=1 UNAOS_FADTRESET_SELFTEST=1 ./arroyo test 60` → exit 0. No `-no-reboot` in the builder's
argv, so a real reset re-boots OVMF: **23 full reboot cycles in 60 s** (46 `FADT RESET_REG` lines;
witness at log lines 75, 150, 225, …; the OVMF banner repeating at 1, 76, 151, 226, …). Zero
`RETURNED` / `8042` / `parking` lines. One cycle (awk-extracted from `serial-selfinvoke.log`):

    ACPI: 6 CPU(s) discovered, local APIC @ 0xfee00000, apic ids [0, 1, 2, 3, 4, 5]
    [orinreboot] reboot verb invoked — dispatching the platform mechanism
    [orinreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)
    [orinreboot] FADT RESET_REG space=SystemIO addr=0xcf9 value=0xf — writing
    ...BdsDxe: loading Boot0002 "UEFI QEMU HARDDISK QM00011 " from PciRoot(0x0)/Pci(0x1F,0x2)/Sata(0x5,0xFFFF,0x0)
    BdsDxe: starting Boot0002 "UEFI QEMU HARDDISK QM00011 " from PciRoot(0x0)/Pci(0x1F,0x2)/Sata(0x5,0xFFFF,0x0)
    [ INFO]: crates/bootloader/src/main.rs@448: UnaOS UEFI Bootloader Started

Reachability: `strings -a target/x86_64-unaos/release/unaos-kernel` carries every `[orinreboot]`
piece; `grep -a -c 'RESET_REG space='` = 1.

## Metal flight tonight (rMBP) — what to look for on the wire, in order

Type `reboot` at the shell. Expect:

1. `rebooting: invoking the platform firmware mechanism...` (console; also on the panel)
2. `[orinreboot] reboot verb invoked — dispatching the platform mechanism`
3. `[orinreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)`
4. `[orinreboot] FADT RESET_REG space=SystemIO addr=0xcf9 value=0x6 — writing`
   (Apple's Series-7 FADT normally publishes 0xCF9 / 0x06; QEMU used 0x0F.) **PASS = this is the
   last line before the firmware's own boot output / the next `UnaOS UEFI Bootloader Started`.**
5. If the capture continues with `[orinreboot] FADT RESET_REG write RETURNED — platform did not
   reset; trying 8042 pulse`, the PCH ignored ACPI; a reset right after that line is the 8042 rung
   working — still a warm reset; record which rung fired.
6. If line 4 never appears: `[orinreboot] FADT has no RESET_REG — trying 8042 pulse (why: ...)` —
   the reason names the missing FADT fact (checksum / revision / bit 10 / address space). Record it
   verbatim; the 8042 rung then runs as in 5.
7. Worst case: `[orinreboot] no reboot mechanism took on this platform (x86: FADT RESET_REG and the
   8042 pulse both returned) — parking in hlt` — machine stays on; 5-second power-button hold.
   STOP finding; record.

Lines 4–7 are serial-only (LOCKFIX): not on the panel. `awk '/orinreboot/' <capture>` — never grep.

## Flagged

- No x86 typed-input harness exists; a permanent gate for the `reboot` verb needs an x86 twin of
  `UNAOS_K8_SCRIPT` (socket chardev on COM1 + a serial-RX pump into `shell_inbox`). Out of scope.
- The brief named no doc file; nothing under `docs/` was edited.
