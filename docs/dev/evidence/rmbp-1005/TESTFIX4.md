# TESTFIX4 — the FLIGHT 20 `tests` fails that are mine (B330)

Branch `exec-rmbp-testfix4`, cut from 4e48ab03 (merge11). Source: FLIGHT20.md, `f20-boots.log`
(`:: TESTS: ran=63 pass=127 fail=9 failed=[quietboot,helpdoc,vein,windowlist,hdaboth,hda220,hda2,lumen,usbnet] ::`).
quietboot (QUIETBOOT2), lumen (LUMENCRASH), usbnet (USBNET7), hda* (AUDIO8) are not touched here.

## Design

| test | failed line | cause | fix | kind |
|---|---|---|---|---|
| helpdoc | `HELPVERB: verbs=107 documented=106 missing=[lumen]` | boot 20 still had the `lumen` verb with no DOCS row. EXECNAME (B322) removed the verb (a bare `lumen` now plans as Exec `LUMEN.ELF`), so on this tree the registry is 106 words and DOCS covers all 106 (checked by script against HOST_VERBS + CORE_VERBS, every feature on). No DOCS row names `lumen` or `vein`. | none: neither the table nor the fixture is wrong on this tree | stale (fixed by EXECNAME) |
| vein | the VEINBUS fixture | LUMENAPP (B323) retired it; no `register("vein"…)`, no spec row, no fixture survives in this tree | none | retired |
| windowlist | `rows=10 live=3 focused=5 minimised=3 show_desktop_ok=false` | `rows=10` is right (7 fixed rows + 3 live windows; the brief's "retired rows" reading does not hold: three live app windows were on the table: win 3, 4 and 5). The wire shows Show Desktop hiding all three, then NO second `[winmenu] open`: `winmenu::press_at`'s fast path declined every press when no window had menus and no window was focused (`LIVE == 0 && APP_OWNER == WIN_NONE`), which is exactly the Show Desktop state. `bar_boxes` lays the Window box in that state (the way back), but a press on it was refused before the hit test. | the fast path also requires `!winlist::desktop_hidden()` (as `bar_boxes` already does) | real (the way back from Show Desktop was unpressable) |
| (read list) | `install` → "Unknown command" | the verb is `#[cfg(feature = "installdemo")]` and the line lacked `UNAOS_INSTALLDEMO=1` | midden_core GATED-OFF answer class: `gated_off(word)` names the knob of a verb this build does not carry; `plan`, `which`, `help`/`man`/`--help` answer `install: verb present on UNAOS_INSTALLDEMO=1 builds only` | new answer class |
| (summary) | SKIP indistinguishable from FAIL/PASS in the summary | `run` counted only verdict lines | a fixture that ran and printed no PASS and no FAIL verdict is listed in `skipped=[…]` | witness shape |

Seam: GATED-OFF lives in `midden_core` (the shared `no_std` core both rings link), at the tail, with a host test;
the kernel only renders it. No new kernel file.

Milestones: M1 windowlist press fast path. M2 midden_core `gated_off` + host test + kernel wiring (plan fallback,
`which`, help). M3 `tests` summary `skipped=[…]`.

Witness (boot 21): `:: WINDOWLIST: rows=<n> live=<n> focused=<id> minimised=<n> show_desktop_ok=true -> PASS ::`
preceded by `[winlist] show-desktop restore moved=<n>`; on a build without installdemo, `install` prints
`install: verb present on UNAOS_INSTALLDEMO=1 builds only` and serial `:: [midden] gated-off verb=install knob=UNAOS_INSTALLDEMO=1 ::`;
`:: TESTS: ran= pass= fail= failed=[…] skipped=[…] ::`; `HELPVERB … missing=[] … -> PASS`.

Owed: metal `tests` on boot 21 confirms windowlist and helpdoc.

## Legs (inline, from 4e48ab03 + M1..M3)
- x86 metal shape (wc,…,installdemo,instgui,witness): `cargo +nightly check` exit 0.
- x86 metal shape WITHOUT installdemo (and without `instgui`, which implies it): exit 0.
- aarch64 `login,loginst,virt_el0` (user_blob.bin head 280080d2): exit 0.
- `cargo test -p midden_core`: exit 0 (23 passed); `--features installdemo,login`: exit 0 (23 passed).
- charter-check: exit 0.
