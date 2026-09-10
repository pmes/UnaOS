# orin 19 — close report (2026-09-07)

## What flew
**render9 `render9-20260907T1157Z-a62188c` FLEW.** Image identity verified positively, not assumed:
board reported `KELF min=0x0 max=0x33f480 pg=832`; the staged `kernel.elf` computes `max_vaddr=0x33f480,
pages=832`. Preflight all PASS (JB1b MRQ_PING, JB0 fan, JB1c XUSB, JB2b keyboard, TEGRA-EL0 round-trip),
`-> CASCADED windows=2 bar=1 route=ROUTED`, `[bsprun] -> HOSTING`, zero panics, zero exceptions.

**The interactive battery (steps 9-14) never ran.** Peter shut the board down from the crystal menu
(`crystal_pick verb=ShutDown action=real` -> `PSCI SYSTEM_OFF`). ShutDown is confirmed; **Restart (A34)
is still untested.** Step 12b — the four keystrokes that are the only stimulus for VFSROUTE,
SHELLRELICS and FSLAYOUT — did not happen.

Scored anyway, off primary lines rather than counters: **A17 + SR2/A36 InFlight refusal PASS** (fired
twice, by name); **PRTSCLOST CLEAN, 6/6 captures completed** where render8 lost two of three; XHCINTD
correctly absent; ARMGAP-WIDE the expected instrument PASS.

## The number of the round
**With `a62188c9`'s two `authorize_write` lines deleted and nothing else, `kernel8-test` reported
`MBENCH PASS — 119/119`.** Every witness in the tree stayed green while the fix it existed to protect
was gone. Gates 12, 13 and 14 had nothing to say about that change. That is why the witness arc existed,
and it is why the floor moved **119 -> 125 in one day** — every new REQUIRE guards something previously
invisible.

## Gates
18/19/20 all green on the folded tree: **MBENCH PASS 125/125 forbid=0**, check 0, test-arm 0,
test-x86-wc 0 (`ptrdead=0`), `aclsym-arm=PASS aclsym-skips=0 xvol=1`.
⚠ **GATE 17 IS VOID** — it measured a tree with conflict markers in it (`scratch/orin19/gate17/VOID`).
The x86 leg reddened twice under a four-executor load and was settled by a CONTROLLED PAIR at the same
load (patch reversed -> green; re-applied, sha256 identical -> green), not by precedent.

## The day's shape — four instances and one COUNTER-EXAMPLE
1. `[kbdpoll] prtscr refused=` read 0 across a whole boot while the PRTSCR path logged two refusals by
   name; its sibling `ok=` counter worked. The flight plan points the operator at the broken field.
2. A storage fault (`read_inode` miss) rendered as "permission denied (-EACCES)" in TWO renderers.
3. The taskbar has no tear scope — `torn=` is `scope=window` only, so Peter's tearing is unscoreable.
4. Four `[cursor*]` instruments name the x86 front-buffer sprite; aarch64 does not use it for its
   visible arrow (`SPRITE_OWNS_PAINT = cfg!(target_arch = "x86_64")`).
**COUNTER-EXAMPLE: foreman's SPECFLIGHT preflight (`457ed7c5`) — built not to fail silently, and it
worked.** This seat read `parse_spec_bytes` in isolation, correctly found the abort, and published
"green by vacuum"; rmbp refuted it and the retraction was verified here before it reached a ledger.
**Four failures plus one defended near-miss teaches something different from five failures.**

## The finding that defines orin 20
`sdmmc_tegra.rs:3495` prints "all were dead: unafs has no volume here" as a **hardcoded string literal**,
three lines after the probe reports the native UnaFS volume MOUNTED. No check exists. A mount decision
rests on a sentence. Peter found it by reading this seat's own quoted output back at it.

## Owed
`git push origin hw-jetson` (origin `06ffdaf8`, local `42eb2736`). PANICLOC remains Peter's, with a
third option now on the table (normalize the comparison rather than degrade the panic output).
MENUOWN / TEARSCOPE / TICKDEFAULT were still running at close; harvest, gate, fold.
