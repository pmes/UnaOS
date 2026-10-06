# LOADERSTALL (B490) — design

**Finding (read from the code, not re-derived).** The loader never paints the rMBP panel and never drives
a wire: its only output is `log::info!` to ConOut, which Apple's firmware does not show on the glass. The
word on the glass is the KERNEL's: `fs/users.rs` opens `bootstep::begin("stage-resolve", "Starting")`,
whose label `splash::step_label` paints on the held splash. All three flight-26 wires reach
`[boot] step=stage-resolve ms=300` at about 13 s, AFTER `firmware->loader=12821..13171ms` and after
`BPACE pci-usb` (the FTDI's enumeration). So "stuck at starting" says the kernel reached stage-resolve and
the glass was never released from the held splash; "nothing on the wire" then says the FTDI never carried
the ring (the warm reboot after USBNETKILLSFTDI's xHCI kill is the candidate) — no captured wire exists for
the two stalled boots, so neither is proven. This arc makes the loader say its own phase, so the next stall
is attributable on the glass with no wire at all: the loader's lines stay on the panel until the kernel's
splash covers them; a stall with the loader lines visible and no splash is the loader's, a stall with
the splash and "Starting" is the kernel's.

**Seam.** `crates/boot-info` is the shared core both sides compile (the loader↔kernel hand-off, ABI-LOCK):
a `LoaderStages` record (stage ids, end stamps in raw TSC, last status per stage, volumes, kernel bytes,
the UART the loader found, the timeout count, a magic) and the stage-name table, x86_64 only, appended to
the common prefix. The loader writes it, the kernel reads it; no second store.

**Milestones.**
- M1 — boot-info: `LoaderStages` + `LOADER_STAGE_NAMES` + ABI-LOCK offsets re-measured.
- M2 — loader (`crates/bootloader/src/stages.rs`, x86 only, font8x8 x86-only dep): TSC rate by a 10 ms
  firmware stall; the UART (EFI SerialIo if the firmware publishes one, else the chipset 16550 at COM1 if
  its scratch register answers, else none — said on the panel); one panel line per stage
  (`firmware ok · gop · volumes <n> · kernel <bytes> · elf · discover · jumping`, each with elapsed ms);
  a 1 s firmware timer watchdog with a budget per stage that prints `loader: TIMEOUT stage=<s> ms=<n>
  last=<status>` on panel + UART every time it fires past budget; the kernel read in 1 MiB chunks (last
  status = bytes so far), a failed chunk retried 3 times before `boot_fail`.
- M3 — kernel (`loaderstage.rs`): `:: LOADER: firmware->loader=<ms> loader->kernel=<ms> stages=<s:ms,...>
  volumes=<n> kernel_bytes=<n> uart=<u> timeouts=<n> ::` beside BOOTCLOCK; `tests loader` →
  `:: LOADER: stages=<n> slowest=<stage>:<ms> timeouts=0 -> PASS ::` (FAIL on timeouts>0 or no record:
  `stages=0 ... record=absent -> FAIL`).

**Witness (next flight).** Panel bottom-left during the loader: `loader wire=<efi-serial|com1|none>` then
`firmware ok <ms>ms` … `jumping <ms>ms`; wire: `:: LOADER: firmware->loader=… loader->kernel=… stages=… ::`
after `:: BOOTCLOCK:`; `tests loader` → `:: LOADER: stages=7 slowest=kernel:<ms> timeouts=0 -> PASS ::`.

**Owed.** A firmware call that never returns cannot be pre-empted from inside it; the watchdog names it
(on panel/UART, while the firmware's timer still runs) but cannot give up on it. The kernel-side "Starting"
stall (the held splash never released at stage-resolve) is not this arc's fix; it is named for the seat.
