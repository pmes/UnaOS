# AETHERWPT (SR72) — Aether's DOM binding against web-platform-tests

Runner: `handlers/aether/tests/wpt_dom.rs` (one `#[test]` per directory), WPT pinned at
`da1f6d20caf40b4003ae288dda8c97f252dd9264`, fetched sparse into `target/wpt` at test time, skipped
offline, release-only. Command:

    cargo test --release -p aether --test wpt_dom -- --nocapture --test-threads=4

Counts are testharness subtests. **covered** = the files run minus those excluded for a surface outside
this arc (the runner's `excluded()` names each reason; counts per reason below). **all** = every file
run. **needs server** = files not run because they need wptserve itself (`.py` handlers, `.sub.`
substitution, `.https.`/`.h2.`/`.www.` hosts, `{{…}}` templates, `get-host-info`, the dispatcher).
A file whose harness never completed counts as one failing subtest. `html/dom` is its top-level
reflection suite; `.any.js` files run in their window global only.

## BEFORE (binding at 39e56a33, runner extended only)

Measured 2026-10-12 on the shared bench (4 cores, seven executors), `--test-threads=4`; wall seconds per
directory as the runner printed them (the directories run concurrently).

| directory | covered pass/total | covered % | all pass/total | needs server (files) | wall s |
|---|---|---|---|---|---|
| dom/nodes | 5832/6040 | 96.6 | 7517/8776 | 3 | 1619 |
| dom/events | 407/601 | 67.7 | 417/801 | 6 | 686 |
| dom/ranges | 1/30573 | 0.0 | 1/30749 | 0 | 121 |
| dom/traversal | 1/839 | 0.1 | 1/847 | 0 | 5 |
| dom/collections | 54/55 | 98.2 | 54/55 | 0 | 11 |
| dom/lists | 184/189 | 97.4 | 184/189 | 0 | 12 |
| html/dom (reflection) | 53451/63112 | 84.7 | 53467/63156 | 3 | 169 |
| html/semantics/forms | 708/3431 | 20.6 | 896/4603 | 33 | 111 |
| html/webappapis | 488/626 | 78.0 | 519/974 | 31 | 58 |

Excluded from covered (files, by reason) in the BEFORE run:

- dom/nodes: MutationObserver 16, frames/browsing contexts 26, shadow DOM 8, custom elements 1.
- dom/events: testdriver (WebDriver input) 74, frames 11, layout 8, shadow DOM 5, custom elements 1.
- dom/ranges: shadow DOM 12, frames 7 (the content-operation suites compare against a reference
  implementation running in an iframe), layout 6, testdriver 3.
- dom/traversal: frames 3.
- html/dom: shadow DOM 2, layout 1.
- html/semantics/forms: testdriver 103, frames 44, layout 21, shadow DOM 5, MutationObserver 4.
- html/webappapis: frames 106, layout 13, workers/messaging 7, MutationObserver 5, shadow DOM 3,
  custom elements 2, testdriver 2, network (SR70) 1, user prompts 1.

Notes on the BEFORE wall time: four `dom/nodes/NodeList-static-length-getter-tampered*` files run a
JS-heavy loop and take 140–400 s each (js_core throughput, not a hang); `dom/events/Event-timestamp-safe-
resolution.html` spun 649 s to the task budget because the page clock is frozen in virtual time and
event timestamps never advanced within a task.

## Interim development runs (not the AFTER table)

R107 stopped executor-side WPT runs before a full AFTER pass; the AFTER table is the fold's. For the
seat's expectation only, the targeted development runs (same runner, `--test-threads=4`) on the
binding before the last edits (options collection, selectedOptions, `input.list`, option IDL, pattern
validity, CSS color sanitization, label/radio activation fixes) read, covered pass/total:

| directory | BEFORE covered | dev-run covered |
|---|---|---|
| dom/events | 407/601 (67.7%) | 552/601 (91.8%) |
| dom/ranges | 1/30573 (0.0%) | 37361/40270 (92.8%) — the total grows: suites that errored before now run |
| dom/traversal | 1/839 (0.1%) | 1598/1600 (99.9%) |
| dom/collections | 54/55 | 54/55 (unchanged) |
| dom/lists | 184/189 | 184/189 (unchanged) |
| html/dom (reflection) | 53451/63112 (84.7%) | 62988/63112 (99.8%) |
| html/semantics/forms | 708/3431 (20.6%) | 3064/3483 (88.0%) |
| html/webappapis | 488/626 (78.0%) | 488/626 (no webappapis-specific change landed) |
| dom/nodes | 5832/6040 (96.6%) | not re-run |

Known misses at hand-off: html/webappapis below 90% (promise-rejection events, event-handler
compilation details, document.write cases); html/semantics/forms tests that need navigation
(`form-double-submit-*`, `input-type-change-submit`), image load (`image-width-height`), FormData
(SR70's XHR surface). No js_core defect was isolated.
