# x86-mtp.spec — the QEMU usb-mtp leg: SendObject's Out data phase yields the controller loan to a block claimant.
#
# RUN-BY: knobleg — UNAOS_QEMU_MTP_RW=1 ./arroyo test 240 ; then
#          python3 scripts/mbench.py --replay target/serial.log --spec scripts/specs/x86-mtp.spec --platform x86
#
# XHCILOANPTPYIELDQEMUMTP (zenbook-ledger Z105, BP-cores-23 kept). XHCILOANPTPYIELDOUT (Z94) gave the MTP/PTP send
# (`ptp_txn_inner`'s Phase::Out in unaos/crates/kernel/src/drivers/xhci/ptp.rs) Z73's loan yield between two TDs, but q35
# has no PTP device, so nothing headless drove it. `UNAOS_QEMU_MTP_RW=1` (unaos/arroyo, beside the uvc map line) attaches
# QEMU's read-write `usb-mtp` on xhci.0 (QEMU is the responder; UnaOS's MTP initiator is the code under test) and arms
# `uvc`; under tests-at-boot `tests ptpout` (`mtp::out_yield_selftest`) arms Z73's block claimant INSIDE a real
# `with_controller` hold and sends 8 MiB + 64 KiB through `send_object`, so the send itself prints Z73's line with
# `who=ptp-out` and at least one window a block claimant took. Its verdict `:: PTPOUT: … -> PASS ::` is scored by the
# default FORBIDs on its FAIL arm; its ABSENCE (a SKIP: no device, no MTP session by test time, one CPU) is caught by the
# REQUIRE below, since the line is printed only by a send over 8 MiB. `tests mtpphone` SKIPs on this lane by name
# (`:: MTPPHONE: -> SKIP(device) ::`): its synthetic responder cannot be the session's phone while QEMU's is.
#
# WHY HAND-RUN AND NOT WIRED: `x86_pick_capture_spec` routes a no-medium capture to x86-default.spec; routing this knob
# here is XHCILOAN-PTPYIELD-QEMUMTP-PICK (owed, zenbook-ledger Z105). Bench unknowns this leg answers: the bench QEMU's
# usb-mtp write support (`readonly=false`, QEMU 4.0+) and its 0xFFFFFFFF root parent (else `reason=send(...)`), and the
# session being open when tests-at-boot reaches `ptpout` (else a SKIP and this REQUIRE reds).
REQUIRE \[xhci\] loan yield who=ptp-out n=[1-9][0-9]* taken=[1-9][0-9]* taken_poll=0
