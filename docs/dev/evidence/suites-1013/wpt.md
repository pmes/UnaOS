# SUITES 2026-10-13 — the WPT subset on 441d7e45d, the rMBP bench (Fedora 44)

The tree's subset is `handlers/aether/tests/wpt_dom.rs` (AETHERJS SR63 / AETHERWPT SR72 / AETHERNET SR70): twelve
`#[test]`s, one per directory — `dom/nodes` (floor 80 %), `dom/events`, `dom/ranges`, `dom/traversal`,
`dom/collections`, `dom/lists`, `html/dom`, `html/semantics/forms`, `html/webappapis` (floor 0 %), and
`websockets`, `eventsource`, `xhr` (floor 85 % of served subtests) — over a sparse WPT checkout pinned at
`da1f6d20caf40b4003ae288dda8c97f252dd9264`. Neither `arroyo` nor `unaos/scripts` names WPT; the file's own header
names the command, run as written:
`AETHER_WPT_VERBOSE=1 cargo test --release -p aether --test wpt_dom --no-fail-fast -- --nocapture --test-threads=4`.

## Run 1 (fresh target dir) — NOT A PASS: 12/12 "ok", eleven of them ran nothing

`test result: ok. 12 passed; 0 failed` — but eleven directories printed `wpt <dir>: skipped (no checkout, offline)`
on a host WITH network: the twelve threads raced the sparse clone into the same `target/wpt` and the losers saw a
half-made checkout. Only `websockets` ran (509/517). Filed SR110, fixed in eb3764d40 (one `OnceLock` around
`locate()`); re-checked on a FRESH clone with an `AETHER_WPT_ONLY` subset: all twelve ran, no "skipped (no
checkout" line, 12/12 ok (`wpt-race-recheck.raw.txt`, 17 s). Log: `wpt-run1-race.raw.txt` (cut at 50 KB).

## Run 2 (checkout present, code at 441d7e45d) — the measurement

**Summary: `test result: FAILED. 11 passed; 1 failed` (rc=101, 431 s).**

| test | covered (or served) subtests | floor | verdict |
|---|---|---|---|
| wpt_dom_nodes | 8927/10021 (89.1 %), all 11110/12742 | 80 % | ok |
| wpt_dom_events | 597/614 (97.2 %), all 618/812 | 0 | ok |
| wpt_dom_ranges | 37960/44586 (85.1 %), all 37960/44695 | 0 | ok |
| wpt_dom_traversal | 1600/1608 (99.5 %) | 0 | ok |
| wpt_dom_collections | 54/55 (98.2 %) | 0 | ok |
| wpt_dom_lists | 184/189 (97.4 %) | 0 | ok |
| wpt_html_dom | 63011/63139 (99.8 %), all 63022/63156 | 0 | ok |
| wpt_html_semantics_forms | 3307/3838 (86.2 %), all 3628/4873 | 0 | ok |
| wpt_html_webappapis | 570/786 (72.5 %), all 758/1143 | 0 | ok |
| wpt_websockets | served 509/517 (98.5 %), 178 pages | 85 % | ok |
| wpt_eventsource | served 63/64 (98.4 %), 41 pages | 85 % | ok |
| **wpt_xhr** | **served 274/356 (77.0 %), 125 pages** | **85 %** | **RED** |

## Red list

- **wpt_xhr** (SR117, open) — first failing assertion: `wpt_dom.rs:852` `wpt xhr: served subtests fell below 85%`.
  46 of the 125 served pages are short; the first failing subtest in file order:
  `xhr/abort-after-stop.window.js` `XMLHttpRequest: abort event should fire when stop() method is used:
  assert_equals: expected true but got false`. Largest shapes: `getallresponseheaders.htm` 0/7 and
  `getresponseheader.any.js` 0/6 (the answer carries the stand-in's own `connection`/`content-length`/`content-type`
  instead of the headers the test's handler sets — a `wpt_net` stand-in gap or an Aether one, not settled here),
  `send-data-sharedarraybuffer.any.js` 0/14, and 10 pages `harness did not complete` (8
  `xmlhttprequest-timeout-*`, `xmlhttprequest-sync-not-hang-scriptloader.html`, `abort-during-navigation.window.js`). Not a sub-twenty-line fix.

Log: `wpt.raw.txt` (cut at 200 KB of 653 KB; full log `~/unaos-bench/scratch/rmbp-0915/local1/suites/wpt.log`).
