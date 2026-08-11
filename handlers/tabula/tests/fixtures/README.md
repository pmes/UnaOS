# Console-log fixtures

Real bytes off this bench, not synthesised. Both files are byte-for-byte
slices of captures under `~/unaos-bench/capture/`, kept so the log renderer is
tested against what the hardware actually emits — control bytes included.

- **`s73-UNAOS-slice.LOG`** — the kernel flight recorder's `UNAOS.LOG`
  (`s73-UNAOS.LOG.saved`, 262 656 bytes): its first 4096 bytes followed by its
  last 4096 bytes. The head carries the `FR-BOOT` identity line, the
  self-identifying header, and multi-byte UTF-8 (`⚡`, box drawing); the tail is
  the fixed-size reservation's NUL padding (4096 NUL bytes). The two halves are
  adjacent here but are not adjacent in the source file — this is a size-reduced
  fixture, not a whole log.
- **`squawk-ttyUSB0-head8k.log`** — the first 8192 bytes of the GR25 Boot C FTDI
  capture (`gr25-bootC/ttyUSB0.log`). Carries a squawk session-start mark and a
  **stray interior NUL** at offset 55 — the byte that makes `grep` unusable on
  these logs and the reason the renderer shows control bytes instead of passing
  them through.
