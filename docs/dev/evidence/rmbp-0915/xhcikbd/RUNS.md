# XHCIKBD / XHCINTD — the runs (exec-rmbp-xhcikbd, 2026-09-15)

Every run below is one `./arroyo test 90` on this worktree (`~/unaos-bench/scratch/rmbp-0915/xhcikbd`, branch
`exec-rmbp-xhcikbd`); the full arroyo log and serial capture of each stay in the arc's scratch
`xhcikbd-logs/` (the seat folds what it wants), this file carries the verdict lines VERBATIM and the exit
status of each, read from the log file (never a pipe). The fixture and its knob: usb_xhci.md §7i.

| run | command / tree | exit | notes |
|---|---|---|---|
| 00 | `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90` at 27716175 (the default leg, before the arc) | `rc=0` | The B45 fact re-measured: `[hidkeys]` lines = 0, `EHCI-HID` lines = 61, `usb-kbd` on the EHCI bus. |
| 02 | `UNAOS_XHCIKBD=1 UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90`, first fixture cut (atomic 20-event `input-send-event` bursts, 3 s after the theme line, 0.5 s apart), driver = 27716175 | `rc=1` | Two bursts fell into one 1.06 s pump stall (the boot battery's window launches). Finding about the battery's pump cadence, not about the driver. |
| 03 | same, atomic bursts after the battery (`:: zeolite: resolver bound :53` marker), driver = 27716175 | `rc=1` | Exactly 17 of 20 per burst: one delivered at injection, 16 queued by QEMU's usb-kbd, 3 dropped. |
| 05 | same atomic bursts, driver = N=4 (M2 applied) | `rc=1` | ALSO 17 of 20 per burst with `outstanding=4/4`: QEMU delivers one report per 8 ms polling interval however many TRBs wait. The atomic burst cannot tell one TRB from four — the fixture was reworked into the stall-key stream. |
| 06 | `UNAOS_XHCIKBD=1 UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90`, FINAL fixture (d863eab2 = 27716175 + M1: F11 stall key, 600 ms spin in the completion branch, 19 events 20 ms apart per burst), driver = one TRB | `rc=1` | THE RED LOG. Endpoint dark for the whole stall (`armgap_us=602801`): 16 of 19 survive per burst, 3 lost, a lost release restates the next press. |
| 07 | same command, driver = M2 (four TDs, re-arm before decode, LED SET_REPORT deferred) | `rc=0` | THE GREEN LOG. All 80 arrive, `restated=0 outstanding=4/4 skipped=0`; the pump gap is still 600 ms (the fixture's own stall) but the endpoint is armed through it. |
| 08 | same command, M2 reverted onto d863eab2 (`git apply -R` of M2's diff, then re-applied) | `rc=1` | GO-RED: the leg reds again with the repair alone removed. |

## The verdict lines, verbatim (`awk 'index($0,":: XHCIKBD:")'` on each serial capture)

run 00 (rc=0):
```
(no XHCIKBD line — the fixture was not armed on this run)
```

run 02 (rc=1):
```
:: XHCIKBD: reports=50 restated=2 lost=30 expected=80 armgap_us=1063003 gapmax_us=1063003 rearm=51 discard=0 dup=0 nobuf=0 -> FAIL ::
```

run 03 (rc=1):
```
:: XHCIKBD: reports=68 restated=3 lost=12 expected=80 armgap_us=13889 gapmax_us=433043 rearm=69 discard=0 dup=0 nobuf=0 -> FAIL ::
```

run 05 (rc=1):
```
:: XHCIKBD: reports=68 restated=3 lost=12 expected=80 armgap_us=38978 gapmax_us=918723 rearm=73 discard=0 dup=0 nobuf=0 outstanding=4/4 skipped=0 leddefer=0 -> FAIL ::
```

run 06 (rc=1):
```
:: XHCIKBD: reports=68 restated=3 lost=12 expected=80 stalls=4 armgap_us=602801 gapmax_us=1875134 rearm=69 discard=0 dup=0 nobuf=0 -> FAIL ::
```

run 07 (rc=0):
```
:: XHCIKBD: reports=80 restated=0 lost=0 expected=80 stalls=4 armgap_us=601568 gapmax_us=735083 rearm=85 discard=0 dup=0 nobuf=0 outstanding=4/4 skipped=0 leddefer=0 -> PASS ::
```

run 08 (rc=1):
```
:: XHCIKBD: reports=68 restated=3 lost=12 expected=80 stalls=4 armgap_us=30944 gapmax_us=778066 rearm=69 discard=0 dup=0 nobuf=0 -> FAIL ::
```

## The typist transcripts of the three defining runs (target/xhcikbd-typist.log, printed by arroyo beside the capture)

run 06:
```
[qmp] waiting up to 75s for '[hidkeys] set-idle ok' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[hidkeys] set-idle ok' seen at +4.0s
[qmp] waiting up to 71s for '[crispy] theme=' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[crispy] theme=' seen at +14.3s
[qmp] waiting up to 61s for ':: zeolite: resolver bound :53' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker ':: zeolite: resolver bound :53' seen at +29.0s
[qmp] boot 5.0s, then type '' (enter=False)
[qmp] burst 1/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 2/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 3/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 4/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] bursts done: 80 events total = expected reports if none were lost
[qmp] sentinel 'f12' pressed after 1.0s quiet
```

run 07:
```
[qmp] waiting up to 75s for '[hidkeys] set-idle ok' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[hidkeys] set-idle ok' seen at +6.3s
[qmp] waiting up to 69s for '[crispy] theme=' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[crispy] theme=' seen at +16.3s
[qmp] waiting up to 59s for ':: zeolite: resolver bound :53' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker ':: zeolite: resolver bound :53' seen at +30.0s
[qmp] boot 5.0s, then type '' (enter=False)
[qmp] burst 1/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 2/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 3/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 4/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] bursts done: 80 events total = expected reports if none were lost
[qmp] sentinel 'f12' pressed after 1.0s quiet
```

run 08:
```
[qmp] waiting up to 75s for '[hidkeys] set-idle ok' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[hidkeys] set-idle ok' seen at +5.8s
[qmp] waiting up to 69s for '[crispy] theme=' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker '[crispy] theme=' seen at +16.0s
[qmp] waiting up to 59s for ':: zeolite: resolver bound :53' in /home/pmes/unaos-bench/scratch/rmbp-0915/xhcikbd/unaos/target/serial.log
[qmp] marker ':: zeolite: resolver bound :53' seen at +30.8s
[qmp] boot 5.0s, then type '' (enter=False)
[qmp] burst 1/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 2/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 3/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] burst 4/4: 20 events (stall key 'f11' + 9 pairs over ['a', 'b']), one command each, 20 ms apart, 400 ms total
[qmp] bursts done: 80 events total = expected reports if none were lost
[qmp] sentinel 'f12' pressed after 1.0s quiet
```

## Boot anchors of the three defining captures (so each capture is identifiable)

run 06:
```
[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x12fe795 pg=4863
:: WXN-x86: ehdr=0x3C87D000 img=[0x3C87D000,0x3DB7B795) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
```

run 07:
```
[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x12ff795 pg=4864
:: WXN-x86: ehdr=0x3C87C000 img=[0x3C87C000,0x3DB7B795) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
```

run 08:
```
[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x12fe795 pg=4863
:: WXN-x86: ehdr=0x3C87D000 img=[0x3C87D000,0x3DB7B795) gib_img=0 gib_tramp=0 spare_n=1 pdpt_seen=1024 nx_set=1023 huge_leaf_nx=0 skip_spare=1 skip_user=0 skip_pml4_user=0 skip_selfmap=0 already_nx=0 skip_fb_lock=0 skip_fb_base=0 skip_fb_walk=0 residue_leaves=4089 (1g=0 2m=505 4k=3584 pt=7) pge=0 flush=cr3-reload wp=1 -> SWEPT ::
```

