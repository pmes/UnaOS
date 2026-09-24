# CLOSESCOPE + SHOTVERB (B212, 2026-09-24)

## Before — the x86 wc lane (`UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 ./arroyo test`, closescope-before)
```
[wc-a] close_owner asid=0x3 closed=1 ids=[1] refused=0
[dock] tile remove win=1 gen=16 owner=0x3 reason=close
[wm] close-scope win=0 owner=0x3 next_focus=0x0 shell_z=64 visible_after=[2] hidden_after=[]     ← the close-box path, id lost
[wm] close-scope win=1 owner=0xc30 next_focus=0xc32 …                                            ← the closemin fixture, id carried
```
Go-red of the new pins on this capture:
```
  ❌ COUNT>=2   \[wm\] close-scope win=[1-9][0-9]* owner=0x[0-9a-f]+ next_focus=      1 hit(s) — SHORT of 2
  ❌ FORBID     \[wm\] close-scope win=0                                             1 hit(s) @ line 2065: [wm] close-scope win=0 owner=0x3 …
```
## After — the same lane, `./arroyo test 90` (a 20 s cap cut the first rerun at 782 lines on a slow TCG boot; no fault, the harness's cap)
```
[wm] close-scope win=1 owner=0xc30 next_focus=0xc32 shell_z=0 visible_after=[2, 3] hidden_after=[]   ← closemin fixture, as before
[wm] close-scope win=1 owner=0x3 next_focus=0x0 shell_z=64 visible_after=[2] hidden_after=[]        ← the close-box path, now naming window 1
  ✅ COUNT>=2   \[wm\] close-scope win=[1-9][0-9]* owner=0x[0-9a-f]+ next_focus=        2 hit(s), first @ line 1474
  ✅ FORBID     \[wm\] close-scope win=0                                               0 hits
  ❌ MBENCH FAIL — 41/42 required witnesses, 1 forbidden hit(s), 2609 lines scanned    (SERIALDOOR: the lane ran without UNAOS_QUARRY/UNAOS_FTDIRX — the spec's RUN-BY knobs; not this arc's)
```
`cargo check` both arches with the edited files' features: rc=0. `git diff --numstat`: syscall.rs x86 4/4, aarch64 3/3, shell.rs 4/4, wm.rs 1/1 — line-neutral.
The `screenshot` verb has no QEMU lane that types it; `Shot::report_ok`'s first segment is the one the verb printed, byte for byte, so the
`bytes -> OK ::` scorers are untouched, and the second segment (`source= serial= dir=`) is the fact the verb now carries. Next stick boot reads it.
