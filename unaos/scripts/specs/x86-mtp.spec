# x86-mtp.spec — the QEMU usb-mtp leg: SendObject's Out data phase yields the controller loan to a block claimant.
#
# RUN-BY: knobleg — UNAOS_QEMU_MTP_RW=1 ./arroyo test 240
#          (the verb's `x86_spec_replay` scores target/serial.log against this file: `x86_pick_capture_spec` names it in code)
#
# XHCILOANPTPYIELDQEMUMTP (zenbook-ledger Z105, BP-cores-23 kept). XHCILOANPTPYIELDOUT (Z94) gave the MTP/PTP send
# (`ptp_txn_inner`'s Phase::Out in unaos/crates/kernel/src/drivers/xhci/ptp.rs) Z73's loan yield between two TDs, but q35
# has no PTP device, so nothing headless drove it. `UNAOS_QEMU_MTP_RW=1` (unaos/arroyo, beside the uvc map line) attaches
# QEMU's read-write `usb-mtp` on xhci.0 (QEMU is the responder; UnaOS's MTP initiator is the code under test) and arms
# `uvc`; under tests-at-boot `tests ptpout` (`mtp::out_yield_selftest`) arms Z73's block claimant INSIDE a real
# `with_controller` hold and sends 8 MiB + 64 KiB through `send_object`, so the send itself prints Z73's line with
# `who=ptp-out` and at least one window a block claimant took. Its verdict `:: PTPOUT: … -> PASS ::` is REQUIREd below
# (XHCILOANPTPYIELDQEMUMTPPICK Z112, the PASS arm of `out_yield_selftest` in drivers/xhci/mtp.rs) and its FAIL arm reds
# through the default FORBIDs; its ABSENCE (a SKIP: no device, no MTP session by test time, one CPU) reds both REQUIREs,
# since the `who=ptp-out` line is printed only by a send over 8 MiB. `tests mtpphone` SKIPs on this lane by name
# (`:: MTPPHONE: -> SKIP(device) ::`): its synthetic responder cannot be the session's phone while QEMU's is.
#
# WIRED (XHCILOANPTPYIELDQEMUMTPPICK, zenbook-ledger Z112): `x86_pick_capture_spec` routes a capture armed with
# UNAOS_QEMU_MTP_RW (and the UNAOS_UVC it exports) here, in PTRLANE's shape, so GATE-SPECROOTS resolves this file GATED.
# NOT PINNED, by measurement: the `:: PREEMPTAUDIT: … ptp_yield= …` word. `tests preemptaudit` registers on x86 only
# under ahci|nvme (tests.rs), and this leg arms neither, so the line is not on this wire (XHCILOAN-PTPYIELD-QEMUMTP-AUDIT,
# owed). Bench unknowns this leg answers: the bench QEMU's usb-mtp write support (`readonly=false`, QEMU 4.0+) and its
# 0xFFFFFFFF root parent (else `reason=send(...)`), and the session being open when tests-at-boot reaches `ptpout` (else a
# SKIP and both REQUIREs red).
REQUIRE \[xhci\] loan yield who=ptp-out n=[1-9][0-9]* taken=[1-9][0-9]* taken_poll=0
REQUIRE :: PTPOUT: bytes=8454144 handle=0x[0-9a-f]{8} yields=[1-9][0-9]* taken=[1-9][0-9]* claimant=ok deleted=[01] -> PASS ::
