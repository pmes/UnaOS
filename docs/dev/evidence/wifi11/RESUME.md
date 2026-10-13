# WIFI11 (B653) — RESUME, written at the seat's stop 2026-10-14 (wrong model; the arc resumes on Opus)

Worktree `/home/user/exec/wifi11`, branch `exec-rmbp-wifi11`, cut from `8da376cb`. The wip commit `820db2e7`
(`wip: WIFI11 — stopped by the seat (model), resumes on Opus`) carries everything below; this file is the commit after it.

## DONE (all under the existing `wifi10` cfg region; no new knob; image-line.env untouched)

Design (R106) is written in `docs/dev/evidence/wifi11/WIFI11.md` (the Design section + 30-row citation table + wire
shapes) and in `docs/dev/OS/06_NETWORK_STACK/wifi_bcma.md` (§"S7 (WIFI11, B653)" + §"S8 — … the design"). Ledger row
B653 is at `docs/dev/OS/rmbp-ledger.md:681` (7 cells, status `fixed-unflown — …`, no claim words), inserted after B645.

1. `unaos/crates/kernel/src/wifi/bringup/ident.rs` (178 lines; was 97)
   - `:52` the store folded onto the existing `*ID.lock() = Some(id);` line (ident.rs has no LINE-NEUTRAL rule; the
     append is before the line's `//`): `#[cfg(feature = "wifi10")] { *PWR.lock() = Some(pwr_decode(&words)); }`.
   - tail (`:99-178`): `CorePwr {itssi_2g, maxpwr_2g, pa_2g: [u16;3]}`, `PWR_INFO_CORE = [0xC0, 0xE0, 0x100]`,
     `pwr_decode(words)` (word = byte/2, sprom.c:183; +0 low=maxpwr, high=itssi, +2/+4/+6 = pa_2g; ssb_regs.h:537-548,
     sprom.c:287-294), `pwr_blank(&CorePwr)` (PA words uniform 0xFFFF/0, or maxpwr 0/0xFF — OUR gate, b43 has none),
     `pwr_2g() -> Result<(rev, [CorePwr;3]), &str>` (gated on S2r verdict Plausible = `sprom=ok`, the gate `mac()` uses),
     `pwr_line(rev, &p)` (the `[wifi11] sprom rev=… core0@0x0c0 maxpwr_2g=… itssi_2g=… pa_2g=[…]` line).
   - Decision: cores 0..2 only (phy_ht.c:632-637 reads three); the 0x120 block and pa_2g[3] are never read (sprom.c:292-294).
   - Decision: one layout for every revision bcma accepts (8..11): sprom.c:167-173 accepts, :643 extracts all via extract_r8.
2. `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs` (1044 lines; was 732; the WIFI11 block is the tail `:733-1044`)
   - consts from phy_ht.h (each cited on its line): IQLOCAL_CMDGCTL 0x0C2, SAMP_* 0x0C3..0x0C7, TSSIMODE 0x122/EN,
     CMD INIT 0x7F, PCTLEN 0x8000, TXPCTL_N 0x1E8 (+masks), IDLE_TSSI 0x1E9 (+masks, BINF), TARG_PWR 0x1EA,
     TX_PCTL_STATUS [0x1ED, 0x1EE, extg(0x169)], RSSI [0x219..0x21B], CA_OVER 0x1, IDLE_TSSI2 extg(0x165),
     TARG_PWR2 extg(0x166), HT_IDLE_TSSI_BASE [0x840,0x860,0x880], HT_POLL_RSSI_SAVE (12 regs), sample table 0x11, len 160.
   - `PwrIn {target, a1, b0, b1}`, `TxPctl {on, idle, status, cmd, targ, idle_regs, tone_ok, samp_stat, c}`.
   - `load_samples` (:359-372; per-element cursor, 160 x 32-bit zero), `run_samples` (:374-413), `stop_playback`
     (:335-357), `rssi_select_tssi_2g` (:427-465), `poll_rssi_tssi_2g` (:467-509; 6-bit sign-extend), `idle_tssi`
     (:573-607), `tx_power_ctl_setup` (:621-729, 2.4 GHz arm; the 64-entry HTTAB16(26+c) estimate tables with b43's
     integer arithmetic), `tssi_setup` (:609-619), `tx_power_ctl_on` (:533-571 enable arm; no CMD index written because
     b43's saved indices start at INIT+1, :862-864), `pub(super) fn tx_pwr_ctl(h, &[PwrIn;3]) -> TxPctl` (the tail in
     :998-1004 order + readback; prints `[wifi11] idle-tssi …` and `[wifi11] txpctl-setup … -> ON|NOT-ON`).
   - Decision: the tone's `run_samples` timeout is tolerated as b43 tolerates it (:409-410): setup proceeds, `tone=TIMEOUT`.
   - Decision: b43's fallback coefficients (52,-424,5612,-1393; :660-665) are NOT used — a blank block is refused upstream.
3. `unaos/crates/kernel/src/wifi/bringup.rs` `wifi10()` (`:2649-2690`): between `tx_prep` and `rx::chan_cookie`,
   `ident::pwr_2g()` -> `pwr_line` -> blank check -> `htphyinit::tx_pwr_ctl(&mut h, &pin)` -> the line
   `:: wifi11: txpctl on=1 target_idx=0x… status=[…] targ_pwr=[…]dBm/4 idle_tssi=[…] tone=ran sprom=ok(rev=N) -> SET ::`
   (refusals: `on=0 target_idx=- sprom=<verdict|blank(coreN rev=R)> -> NOT-SET reason=…`). End line now prints
   `tx_pwr_ctl=on|off` and `wrote-core-regs`/`wrote-wrapper` from `h.c` (cumulative) instead of `tp.c`. All edits are
   at the file's tail (wifi10() is the last fn).

Compile (the arc's ONE command, run once, rc=0, log `~/unaos-bench/scratch/cloud/wave34/wifi11/check1.log`, a real
check: one `Checking unaos-kernel` line, `Finished … in 2m 00s`):
`cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"` → rc=0.
The kernel's 101 warnings are pre-existing; the two in htphyinit.rs (`:614:9`, `:690:9`) are in WIFI9/WIFI10 code
(below the old 732-line tail), none in the WIFI11 block, ident.rs or bringup.rs.

## NOT DONE — the exact next step

1. The pre-commit leg per the coordinator's correction (wave head rule 12 withdrawn in favour of the PLAYBOOK's):
   from the worktree `cd unaos && ./arroyo gates > gates.log 2>&1; echo rc=$?` and paste `GATES: 17 run 17 green -> PASS`
   in the REPORT. NOT run (the stop order forbade gates). The three linters (`bash unaos/scripts/charter-check.sh`,
   `python3 tools/status-check.py`, `python3 unaos/scripts/appearance-check.py unaos`) were also NOT run. No new kernel
   file was created (no CHARTER line needed); no colour literal; the B653 cell has no claim word.
2. The final commit: squash/replace the wip commit's message with the arc's `wifi: WIFI11 — …` subject and the design
   body (the Design section of WIFI11.md is the body), the two attribution lines of the model that does it (Opus).
   The tree content is complete as far as this executor knows: nothing in the code is half-written.
3. Not reviewed by a second pass: `poll_rssi_tssi_2g`'s sign-extension closure
   `(((v & 0x3F) << 2) as u8 as i8 >> 2) as i32` mirrors b43's `((s8)((tmp & 0x3F) << 2)) >> 2` — compiled, not tested on host.

## Learned, not in the arc file

- Line numbers: WIFI10's `phy_ht.c:589-640` is `:621-729` in the 2026-10-13 fetch (1137 lines); every WIFI11 citation
  uses the fetched copy's numbers, named in WIFI11.md's header.
- `wifi_core::ieee80211` (LGPL, host-tested) ALREADY carries `build_auth_open`, `parse_auth`, `build_assoc_req`,
  `parse_assoc_resp`, `ethernet_to_data`/`data_to_ethernet` (WIFI1 M3/M4) — S8 needs no new frame codec; the §S8
  design cites them and names the hardware deltas only (MACFILTER_BSSID slot 3, MACCTL.INFRA, retry limits into SHM
  scratch, the ACK bit on unicast, two receiver subtypes).
- bcm4331.md's ladder labels S8 "WPA2-PSK" and folds auth/assoc into S7; the arc's S8 (open auth+assoc) is written as
  the remainder of bcm4331.md's S7 observables with that naming difference stated in §S8's first paragraph.
- `drivers/bcma/sprom.c`'s header reads "Licensed under the GNU/GPL" (no SPDX); read for offsets only (R110).
- The shared wave34 target dir was empty at start; the check populated it (never rm'd, per the head).
