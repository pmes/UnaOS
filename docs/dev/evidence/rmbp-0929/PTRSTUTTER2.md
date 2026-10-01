# PTRSTUTTER2 — the ring drops before the desktop, and what "dark" counted

## Finding (f18-boot1.log)
`:: PTRSTUTTER: ring=64 dropped=273 dark_ms=9903 ...` at 35.5 s, `dropped=316` at 43.0 s, constant after.
`ISRARM ... irq=376 depth_max=64 dropped=273` at 35.5 s. Deadman: `pmp=0` every second from 13 s to 36 s
(`pmp=1` at 37 s, `pmp=11 hid=66` at 43 s): the polled pass (`service_ehci_hid`, the ring's only consumer, called from
the `usb_pump` task main.rs:5911 / `pump_and_poll` pal.rs:2064) was not entered for ~33 s, while the installer
stage (`FIRSTBOOT stage=installer` 10042 ms) and the SD/FAT bring-up (`SDHCBLK` 10101 ms) ran. The ISR (armed 7007 ms)
kept filling the ring with the pad's reports; nothing took them.

`dark_ms` is NOT "reports expected but missing": `dark_gap_ms` (ehci/mod.rs ~13979) is the whole gap since the ISR/pass last
touched the endpoint, charged whenever a report ends it. A still pad's first report after 10 s of silence
= `dark=9903ms max=9903ms`. Idle was counted as dark.

## Milestones
- M1 ISR coalesces a report byte-identical to the newest UNCONSUMED report (`ptr2_isr_idle_drop`, counted in
  `dropped_idle=`, never ringed); the first consumer take prints `[ehci] consumer attached at= ring_backlog=`.
  NOT done: making the consumer run during the installer — the blocker is the pump task sitting in a long
  synchronous chain (pmp=0), which needs a boot-19 stack/trace read (`[usb26]`/service_storage timing), not a guess.
- M2 EHCIDARK windows count only in a touch session (previous non-idle report within 1700 ms and gap <= 1500 ms);
  idle `dark_ms` reads 0. `touch_sessions=` `dark_in_touch_ms=` on PTRSTUTTER.
- M3 `dropped_installer= dropped_desktop=` (drops attributed by the boot stage current at each witness).

## Written
Boot 19 should show `:: PTRSTUTTER: ring=64 dropped=N dropped_idle=M dropped_installer=I dropped_desktop=D dark_ms=0..
touch_sessions=S dark_in_touch_ms=T dark_max_ms=... -> PASS ::` (idle desktop: dark_ms=0) and one
`:: EHCI-HID: [ehci] consumer attached at=...ms ... ring_backlog=B ... == witness ::`. If `dropped_installer` is still
large with `pmp=0` in the deadman, the pump is blocked in the installer-time chain: that is the next arc.
Pins: none added; x86-witness/default EHCI rows (ISRARM self-test, `[N] ISRARM armed via=`) untouched.
Compiler notes: `ptr2_*` helpers are at ehci/mod.rs tail; stage split is `#[cfg(feature="login")]`.
