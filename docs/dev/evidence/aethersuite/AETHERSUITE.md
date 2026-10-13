# AETHERSUITE — the six reds of `cargo test -p aether --lib` at 96bdc4de

Arc base 96bdc4de, branch exec-host-aethersuite. Each test was run alone before and after its fix under the
arc's R107 exception (`CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave31/target cargo test -p aether
--lib <name>`); no suite, WPT or gate was run. Every commit compiled with
`CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave31/target cargo check -p aether --tests` rc=0.

| # | test (site at 96bdc4de) | before | cause (file:line) | fix | citation | after |
|---|---|---|---|---|---|---|
| 1 | `engine_tests::tests::test_ledger_records_unimplemented_apis` (engine_tests.rs:258) | rc=101 | stale expectation: AETHERSTYLE3 (8b695a6f) implemented `filter` (layout_core css.rs:1081-1096 records only an invalid value); `display:grid` is still recorded (css.rs:3190) | fixture adds `grid-template-columns: 1fr 1fr`; asserts `property:grid-template-columns` and `display:grid` recorded and `property:filter` NOT recorded | Filter Effects 1 §5 (the implemented property); css.rs:1911-1915 (the unhandled-property arm) | rc=0 |
| 2 | `engine_tests::tests::test_generated_counters` (engine_tests.rs:3080) | rc=101 (`απ0` vs `μ0`) | stale expectation since AETHERGEN e0825aae; the generator (render/mod.rs:1073-1083) is the spec's | expectation `απ0` | css-counter-styles-3 §6.2.2 (lower-greek, alphabetic, 24 symbols), §3.1.4 (alphabetic: 40 → α, π) | rc=0 |
| 3 | `engine_tests::tests::test_noscript_iframe_hidden` (engine_tests.rs:1328) | rc=101 | the image-23 fold e49cd1e8 removed `iframe` from layout_core tree.rs `is_non_rendered` (iframe became a replaced box) without excluding its children; tree.rs:324 `replaced_media` names only `video`/`audio`, so the iframe's RAWTEXT fallback is boxed | **STOPPED — the root cause is in layout_core (not in this arc's files).** Exact change: tree.rs:322-324, `matches!(t, "video" \| "audio")` → `matches!(t, "video" \| "audio" \| "iframe")` with the comment naming iframe | HTML §4.8.5 (an iframe's children are not rendered) | not run (no edit) |
| 4 | `engine_tests::tests::click_submits_and_toggles_checkables` (engine_tests.rs:3359) | rc=101 | double activation: the trusted click dispatched at lib.rs:888 runs the label's activation behavior (js/events.rs:972-990, AETHERWPT d27d943c: `synthetic_click` on the control, toggling it), then lib.rs:946's Rust `"label"` arm called `activate` (AETHERINPUT 75af8a58), clicking the control a second time and toggling it back | with a JS engine the Rust arm focuses, repaints (`repaint_checkable`) and drains the queued submission; the old `activate` stays for no-JS | HTML §4.10.4 (label activation behavior); DOM §2.9 (one activation per dispatch) | rc=101 at :3370 (item 4b), then rc=0 |
| 4b | same test, :3370 (hidden behind :3359) | — | the expectation wanted a text/plain body for a `method=get` form posted by `formmethod=post formenctype=text/plain`; Chromium sends it urlencoded under a text/plain header (tests/data/forms_oracle/getform-postplain.expect, sha-pinned), and forms/mod.rs:509-512 reproduces that for tests/forms_oracle.rs | expectation `r=2&s=bee&act=post`, citing the vector | the measured Chromium vector (not the HTML text, which says text/plain lines) | rc=0 |
| 5 | `security::csp::tests::source_lists` (csp.rs:717) | rc=101 | csp.rs:247 compared html_core's `host_str` (hostname[:port], url/mod.rs:774-781) with the source's host, so any non-default port failed before the port check; written against the url crate (9e60f42b, hostname only) and ported by the fold 4990d1b5; same swap at :198 | both sites read `url.hostname()` | CSP3 "does url match expression in origin" (host-part and port-part matched separately) | rc=0 |
| 6 | `security::origin::tests::origins` (origin.rs:281) | rc=101 (`blob://`) | `Origin::of_url` (fold 4990d1b5's port to html_core) built the tuple from the OUTER blob: URL once `has_tuple_origin` (url/mod.rs:985) said tuple | a `blob` URL returns `Origin::of_url` of its parsed path | URL §4.7 origin (blob arm); HTML §7.1.1 | rc=0 |

AETHERWORKERS3 (d840839d) was named as the suspect for 5 and 6; its diff to `security/` adds only
`PolicyList::for_worker_response` (csp.rs:348-365 at d840839d). Both reds date from the AETHERORIGIN fold 4990d1b5.
AETHERA11Y's MouseUp hook was named for 4; it only records the focus starting point.

## Seen in passing, not fixed (outside the six)

- `handlers/aether/src/js/window.rs:1123` (`document.domain` setter) reads `u.host_str()` — host:port — so on
  a page served on a non-default port every value fails the suffix test and throws SecurityError where the
  setter should be a refused no-op. Same defect class as item 5; fix is `u.hostname()`.
- layout_core tree.rs:98-102's comment still says "iframe/object/embed: no frame support yet"; the
  engine_tests.rs:1313-1314 doc comment says "frames are unsupported empty boxes". Both stale since AETHERFRAMES.

## Other test files believed at risk (from reading; nothing run)

The seat's fold check is `cargo test -p aether` (lib + every tests/ file). These were authored under R107
("written, not run") and have never executed:

- tests/workers.rs — AETHERWORKERS3's `a_shared_worker_outlives_its_first_document_and_ends_with_its_last_port`,
  `a_worker_fetches_its_script_on_its_own_thread_and_takes_its_own_csp` (asserts a 750 ms wall bound on
  `new Worker()`, load-sensitive), `a_data_worker_has_an_opaque_origin_and_no_storage`, the rewritten
  `a_worker_creates_nested_workers_under_its_captured_context`; 25924d11's `image_bitmaps_and_offscreen_canvases_transfer`
  and `an_unreachable_port_pair_in_a_worker_is_closed_by_its_collector`; b6d8cb22's `service_worker_ready_is_one_promise_and_match_all_filters_by_type`.
- tests/devtools3.rs — all five (9bfed99b, written, not run).
- tests/wpt_dom.rs / tests/wpt_net — bc073b6f widened the served set ("Nothing run"); the runner also
  needs the WPT checkout (`git fetch` of the WPT repo), so on a box without network it cannot score.
