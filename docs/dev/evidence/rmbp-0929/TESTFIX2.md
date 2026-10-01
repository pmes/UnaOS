# TESTFIX2 — boot 17 `tests` findings

## Design
1. `wm-act close=false`: `wmdirect_selftest` leg 5 minted its window under `OWNER_D`=3 (slot 2). Under the live desktop a real process holds that slot, so `wc_close_click` (arch/x86_64/syscall.rs ~6425) took the KILL arm (settle=KILLED, not NOPROC) and the leg read false (and killed a live window's process). Not a scale-1 coordinate: the press point is read back from `wm::close_box_rect`. Fix: `owner_c` = a slot with no PRUNNING Proc row.
2. `vugres pos=false`: no resize in this fixture; `pos` asserted global `VUGRES_EMITTED_POS == before+1`, which every live vug window's first-present bumps. Fix: `VUGRES_POS_FIX`, bumped only for the fixture's top slot.
3. CLOCKBAR 12:34: the synthetic model of `crystal_persist_selftest` (`persist_build`) painted through `clockbar_paint`. Not an anchor. Fix: painter returns early while `PERSIST_MODEL != 0`; the line now carries `from=sntp|verb|placeholder|none` (via new non-spinning `clock::try_source`; no RTC source exists).
4. `tests.rs`: summary carries `failed=[name,...]` (registered fixture names).

## Written
`:: TESTS: ran=N pass=P fail=F failed=[...] ::` (empty list when green); `[wm-act] direct ... close=true`; `[vugres] selftest pos=true neg=true -> PASS`; `:: CLOCKBAR: anchored=1 text=HH:MM from=sntp|verb drawn=1 -> PASS ::` only after a real anchor.
Pins: x86-wc.spec rows unchanged and still match (CLOCKBAR keeps `anchored=1 text=` adjacent; the `tests:` shell line also gains `failed=[..]`).
