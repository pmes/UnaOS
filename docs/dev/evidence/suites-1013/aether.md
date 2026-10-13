# SUITES 2026-10-13 — `cargo test -p aether` on 441d7e45d (hw-rmbp dev tip), the rMBP bench (Fedora 44)

Command: `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/local1/suites/target cargo test -p aether --no-fail-fast`
(debug profile, as the brief names it). Full log: `aether.raw.txt` (47 KB, uncut). rc=101, wall 417 s.

## Summary line

**230 passed, 10 failed, 0 ignored across 13 targets** (lib 195/6 · main 0/0 · dom_oracle 0/1 · forms_oracle 1/0 ·
innertext 1/0 · net 7/0 · script_oracle 1/0 · service_worker 8/2 · style_oracle 1/0 · style_time 3/0 ·
text_oracle 1/1 · wpt_dom 12/0 · doc 0/0). `error: 4 targets failed: --lib, --test dom_oracle, --test service_worker, --test text_oracle`.

Of the 230 passes, 13 ran nothing: the 12 `wpt_dom` tests SKIP in a debug build by design ("skipped in a debug
build (run with --release, or set AETHER_WPT=1)"; the subset is run separately, `wpt.md`), and
`text_rasters_and_widths_vs_chromium_live` SKIPs with no Chromium/node on this host.

## Red list (first failing assertion verbatim; line numbers at 441d7e45d)

| test | first failing assertion | ledger | disposition |
|---|---|---|---|
| `security::origin::tests::origins` | `origin.rs:281` `assertion left == right failed` left: `"blob://"` right: `"https://a.example"` | SR106 | FIXED 0d331273c |
| `security::csp::tests::source_lists` | `csp.rs:699` `assertion failed: url_matches_list(&l("b.example:8443"), "https://b.example:8443/", &so, false)` | SR107 | FIXED 0d331273c |
| `engine_tests::tests::test_generated_counters` | `engine_tests.rs:3080` left: `[…, "απ0"]` right: `[…, "μ0"]` | SR108 | FIXED 0d331273c (the expectation was wrong) |
| `eyes_corpus_dom_equals_html_core_and_chromium` (dom_oracle) | `dom_oracle.rs:28` `the 22 EYES pages (17-21: AETHERINLINE's cases, 22: AETHERFONT's)` left: 25 right: 22 | SR109 | FIXED 0d331273c |
| `engine_tests::tests::aethersee_text_is_measured_with_the_face_it_is_painted_with` | `engine_tests.rs:2463` `bold run measured with the bold face (194 vs 194)` | SR111 | open |
| `engine_tests::tests::click_submits_and_toggles_checkables` | `engine_tests.rs:3359` `a label click toggles its control` | SR112 | open |
| `engine_tests::tests::test_noscript_iframe_hidden` | `engine_tests.rs:1328` `iframe content must produce no box` | SR113 | open |
| `offline_the_worker_answers_from_its_cache` (service_worker) | `service_worker.rs:480` `the cached offline page: "Service worker network error: The service worker answered 'http://127.0.0.1:46873/app/live' with a network error."` | SR114 | open |
| `the_worker_reads_and_writes_the_cache_api` (service_worker) | `service_worker.rs:422` `addAll fetched the asset once` left: 0 right: 1 (preceded by `[aether-sw] error: Uncaught` from the worker's install handler) | SR115 | open |
| `text_widths_vs_chromium_frozen` (text_oracle) | `text_oracle.rs:179` `widths off by more than 0.5 px:` first row `12px #0 "The quick brown fox jumps over the lazy dog": aether 251.160 chromium 237.453` | SR116 | open |

## Re-runs

- The four fixed tests, alone, after 0d331273c: `aether-rerun-fixed.raw.txt` — lib 3/3 ok, dom_oracle 1/1 ok.
- The six open reds re-run once on the fix tip (nothing of theirs touched): `aether-rerun-open.raw.txt` — all six
  red again with the same assertions (deterministic, not flakes). `click_submits…` reads line 3361 there only because
  the counters fix added two comment lines above it.

## Notes on the open reds (not fixed: none is a sub-twenty-line change in a file this arc may touch)

- SR111 and SR116 are font-environment shaped: this host resolves `sans-serif` to Noto Sans (`fc-match`), and the
  frozen Chromium widths in `tests/data/text_oracle_widths.tsv` were produced on a host with a different face set
  (137 rows exceed the 0.5 px gate, Aether both wider and narrower, -401 px to +128 px). Not proven here which
  face Aether selected; the hypothesis is the host's font set, not the measurer.
- SR113 sits in layout (an `<iframe>`'s fallback text gets a box); layout is AETHERSTYLE3's live lane (cloud wave26),
  so it is filed, not touched.
