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

## AETHERWPT2 (SR135) — the rest of the platform

Runner: the same file, `handlers/aether/tests/wpt_dom.rs` — `WPT2_DIRS` (27 directories, one `wpt2_*` test
each, recursive), served by the `tests/wpt_net` stand-in, the `.any.js` window variant only. Command (the
bench's; R107 — no executor ran it):

    cargo test --release -p aether --test wpt_dom wpt2_ -- --nocapture --test-threads=4

Exclusion is infrastructure only (`wpt2_excluded`): a test that needs what the stand-in or a headless page
cannot give it is skipped with the reason; a test that needs an API Aether lacks RUNS and fails, and is named
in the gap list below. Reftests, crash tests and manual pages (no `testharness.js`) are counted per directory
and not run. Every floor is 0 at landing; the bench's first run raises each to its measured value.

The stand-in gained `fetch/api/resources/{redirect,inspect-headers,clean-stash,stash-put,stash-take,status,
method,echo-content,trickle,infinite-slow-response}.py` and `common/{slow,echo}.py` (re-implemented from their
sources at the pinned commit) and wptserve's stash. `/resources/*.js`, `/common/*`, `/xhr/resources/*`,
`/fetch/api/resources/*` (static), `/FileAPI/support/*` and `/images/*` are served as files from the checkout.

Test files: testharness `.any.js` / `.window.js` / `.html` at `da1f6d20`, counted with `find` over the sparse
checkout (resources/ and support/ left out); the bench's run gives served and skipped per file.

| directory | test files (any / window / html) | what it needs that this run cannot give (`wpt2_excluded` reasons) | floor |
|---|---|---|---|
| url | 24 / 4 / 4 | handler percent-encoding.py; `.xhtml` not collected | 0 |
| encoding | 37 / 4 / 111 | TLS (sharedarraybuffer.https); handlers single-byte-raw.py, text-plain-charset.py, json-charset.py, text-html-meta-charset.py; idlharness | 0 |
| fetch/api | 139 / 4 / 43 | cross-origin hosts (cors/, credentials/, REMOTE origins); TLS and h2 (17 `.https.`); service workers; handlers preflight.py, cache.py, authentication.py, …; window.open (keepalive); idlharness | 0 |
| streams | 73 / 6 / 9 | idlharness; TLS (3); window.open | 0 |
| FileAPI | 36 / 5 / 39 | window.open (url/); cross-origin; TLS (5); `-manual`; idlharness | 0 |
| webstorage | 0 / 30 / 24 | window.open (event tests); TLS (1) | 0 |
| IndexedDB | 210 / 4 / 15 | TLS (5, storage buckets); testdriver (1); window.open; bfcache; idlharness | 0 |
| html/browsers/history | 0 / 6 / 142 | window.open (36 files); handlers; testdriver (2); bfcache / remote contexts | 0 |
| html/browsers/the-window-object | 0 / 15 / 80 | window.open (55); cross-origin; testdriver (1); TLS (2) | 0 |
| html/browsers/browsing-the-web | 1 / 57 / 272 | remote contexts (/common/dispatcher/); bfcache; window.open (65); handlers slow.py, refresh.py, redirect.py, …; testdriver (13); TLS (11) | 0 |
| html/semantics/scripting-1 | 27 / 1 / 499 | handlers serve-custom-response.py, serve-with-content-type.py, load-error-events.py, referrer-checker.py, log.py, …; cross-origin; TLS (6) | 0 |
| html/semantics/embedded-content | 0 / 17 / 732 | `/media/` fixtures (not in the sparse checkout); layout; testdriver (25); handlers; cross-origin; TLS (9); reftests | 0 |
| html/semantics/tabular-data | 0 / 0 / 32 | layout (a few); reftests | 0 |
| html/semantics/interactive-elements | 0 / 0 / 112 | testdriver (51: dialog, popover, details input); layout; reftests | 0 |
| html/semantics/text-level-semantics | 0 / 0 / 10 | reftests; layout | 0 |
| html/semantics/grouping-content | 0 / 0 / 15 | reftests; layout | 0 |
| custom-elements | 0 / 9 / 177 | window.open; testdriver (5); layout; handlers form-echo.py, echo-content-escaped.py | 0 |
| shadow-dom | 0 / 0 / 277 | testdriver (98: focus, input, selection by input); layout; reftests | 0 |
| selection | 0 / 1 / 123 | testdriver (32); layout (caret and extent by coordinates); reftests | 0 |
| uievents (keyboard events are `uievents/keyboard`) | 0 / 1 / 71 | testdriver (52: keyboard, mouse, wheel, click order); layout; manual pages | 0 |
| pointerevents | 0 / 1 / 192 | testdriver (171 — nearly every test drives a real pointer); TLS (16); layout | 0 |
| WebCryptoAPI | 129 / 2 / 1 | idlharness; cross-origin (secure_context/); h2. `.https.` alone RUNS: it asks only for a secure context and 127.0.0.1 is potentially trustworthy (Secure Contexts §3.2) | 0 |
| compression | 20 / 0 / 0 | nothing infrastructural (its `.https.` file runs as a secure context) | 0 |
| resource-timing | 13 / 0 / 135 | handlers status-code.py, compressed-js.py, redirect-cors.py, fake_responses.py, …; cross-origin (TAO); TLS (13); window.open; layout | 0 |
| performance-timeline | 21 / 13 / 17 | bfcache / remote contexts (back-forward-cache, not-restored-reasons); window.open; idlharness | 0 |
| console | 7 / 0 / 5 | testdriver and `-manual` / `-logging` pages read by a person; idlharness | 0 |
| html/infrastructure/urls | 0 / 11 / 13 | TLS (10); legacy-charset documents through handlers resource.py, stash.py, blank.py; cross-origin | 0 |

### Gap list (APIs and behaviours the tests use that Aether lacked at fe30fa59)

Found by reading each directory's tests against `handlers/aether/src/{js,api,storage}`. **done** = implemented
in this series (spec section, the `wpt2_gaps.rs` test that pins it); **owed** = named with what it needs.

| # | directory | gap | status |
|---|---|---|---|
| G1 | url | URLSearchParams (URL §6.2): `entries()/keys()/values()` returned arrays, not live `URLSearchParams Iterator`s; `has`/`delete` ignored their `value` argument; the init union (sequence pairs must be length 2, record of own enumerable keys, USVString) unchecked; `url.search = …` / `url.href = …` replaced the query object instead of updating it, so `url.searchParams` lost its identity | done — URL §6.2 in `api/platform.rs` PRELUDE (branded state, one query object per URL, `update` steps, the WebIDL default iterator); `wpt2_gaps::g1_urlsearchparams_per_url_standard` |
| G2 | encoding | TextDecoder (Encoding §7.2): no label table (every non-UTF-8 label decoded as UTF-8 under the label's name), no `fatal`, no `ignoreBOM`, no `{stream: true}`, no UTF-16LE/BE, a UTF-8 decoder that accepted overlongs and surrogates, `replacement` labels not refused; `encodeInto` reported `read` 0 for a partial write | done — Encoding §4.2 label table (encodings.json), §7.2 decode with BOM / fatal / stream, §8.1.1 UTF-8, §14.2 UTF-16LE/BE, windows-1252 and x-user-defined, §7.4.3 `encodeInto`, in `api/platform.rs` PRELUDE; `wpt2_gaps::g2_textdecoder_per_encoding_standard` |
| G3 | streams | `CountQueuingStrategy`, `ByteLengthQueuingStrategy` (Streams §7) absent; `ReadableStream` ignored `strategy.size` (desiredSize counted chunks) | done — Streams §7.2/§7.3 strategies and §4.5 EnqueueValueWithSize / desiredSize over the queue total, in `api/fetch.rs` PRELUDE; `wpt2_gaps::g3_queuing_strategies_and_size` |
| G4 | FileAPI | Blob constructor (File API §3.1): parts converted by `Array.from` (array-likes accepted, iterables' errors lost), no `endings`, `File.lastModified` not converted as `long long` | done — File API §3.1 (sequence conversion, BlobPropertyBag `endings` / `type`, native line endings) in `api/fetch.rs`, §4.1 `long long` lastModified in `js/forms.rs`; `wpt2_gaps::g4_blob_and_file_constructors` |
| G5 | html/semantics/tabular-data | HTMLTableElement (HTML §4.9.1): `caption`/`tHead`/`tFoot` read-only, no `createCaption`/`deleteCaption`/`createTHead`/`deleteTHead`/`createTFoot`/`deleteTFoot`/`createTBody` | done — HTML §4.9.1 setters (IDL type TypeError, HierarchyRequestError, placement), create / delete, createTBody, in `js/html.rs`; `wpt2_gaps::g5_table_caption_thead_tfoot_tbody` |
| G6 | WebCryptoAPI | `crypto.getRandomValues` (WebCrypto §10.1.1): no TypeMismatchError for float / DataView, no QuotaExceededError over 65,536 bytes, BigInt64 arrays left unfilled; no `QuotaExceededError` interface (WebIDL) | done — WebCrypto §10.1.1 checks and fill through a byte view, WebIDL `QuotaExceededError` (quota / requested), in `api/platform.rs` PRELUDE; `wpt2_gaps::g6_get_random_values_checks` |
| G7 | WebCryptoAPI | `crypto.subtle` undefined | done — WebCrypto §13 CryptoKey, §14 SubtleCrypto general steps, §18.4.4 normalize: digest (SHA-1/256/384/512), HMAC, AES-GCM, AES-CTR, HKDF, PBKDF2 (`raw` format) over crypto_core, in `api/subtle.rs` (new; natives + prelude, page and worker realms); `wpt2_gaps::g7_subtle_crypto_symmetric`, `api::subtle::tests::subtle_primitives_known_answers` |
| G8 | performance-timeline | `performance.mark` / `measure` / `getEntries*` / `clearMarks` / `clearMeasures` absent; `PerformanceObserver` registered and never delivered | done — User Timing L3 §3.1 / §4 and Performance Timeline L2 §3–§4 (mark / measure / clear* / getEntries*, PerformanceObserver with `type` + `buffered` or `entryTypes`, takeRecords, disconnect, supportedEntryTypes, droppedEntriesCount) in `js/timeline.rs` (new), installed by `js/window.rs`; `wpt2_gaps::g8_performance_timeline_delivers` |
| G9 | console | label conversion swallowed a throwing `toString`; the namespace object's [[Prototype]] was %Object.prototype% (WebIDL §3.13: an empty object whose [[Prototype]] is %Object.prototype%) | done — Console §1.2/§1.4 label conversion rethrows; WebIDL §3.13 namespace [[Prototype]], in `js/window.rs`; `wpt2_gaps::g9_console_namespace_and_labels` |
| O1 | streams, encoding, compression, fetch/api | `WritableStream`, `TransformStream`, `pipeTo` / `pipeThrough`, byte streams (`ReadableByteStreamController`, BYOB reader), transferable streams — needs a Streams core (the Streams Standard's writable / transform / pipe algorithms); `TextEncoderStream` / `TextDecoderStream` and `CompressionStream` / `DecompressionStream` (plus a deflate encoder) wait on it | owed |
| O2 | encoding, html/infrastructure/urls | legacy single-byte decoders other than windows-1252 and x-user-defined, every legacy multi-byte codec (Shift_JIS, EUC-JP, ISO-2022-JP, EUC-KR, Big5, GBK / gb18030) — needs the WHATWG index tables; documents in a legacy charset (the HTML parser decodes UTF-8 only) | owed |
| O3 | FileAPI, url | `URL.createObjectURL` hands out a URL with no blob store behind it (a fetch / XHR / `<img>` of it gets nothing) — needs the blob URL store and the `blob:` scheme fetch (Fetch §4.3); `FileReaderSync` (worker-only; workers/ is not this arc's) | owed |
| O4 | custom-elements | `ElementInternals` / `attachInternals` (form-associated custom elements, `states`) | owed |
| O5 | pointerevents | `setPointerCapture` / `releasePointerCapture` / `hasPointerCapture` and the capture-target override in dispatch | owed |
| O6 | resource-timing | `PerformanceResourceTiming` entries (the script request log in `api/fetch.rs` has the requests, not the timings) | owed |
| O7 | WebCryptoAPI | ECDSA / ECDH / Ed25519 / X25519 / RSA in `crypto.subtle` (crypto_core has the primitives), `jwk` / `pkcs8` / `spki` formats, `wrapKey` / `unwrapKey` (NotSupportedError today), AES-KW; AES-CBC needs the AES inverse cipher, which crypto_core's `aes` lacks (encrypt-only) — a change in `unaos/libs/sys/crypto_core/src/aes.rs`, outside this arc's files | owed |
| O8 | html/browsers/* | auxiliary browsing contexts (`window.open` is a stub), the Navigation API (`navigation`, `NavigateEvent`), bfcache | owed |
| O9 | shadow-dom, selection | `adoptedStyleSheets` / constructable stylesheets, `Selection.getComposedRanges`, `Selection.modify` (needs layout) | owed |

### Tests and what the bench should expect

- `tests/wpt_dom.rs`: `wpt2_url`, `wpt2_encoding`, `wpt2_fetch_api`, `wpt2_streams`, `wpt2_fileapi`,
  `wpt2_webstorage`, `wpt2_indexeddb`, `wpt2_html_browsers_history`, `wpt2_html_browsers_the_window_object`,
  `wpt2_html_browsers_browsing_the_web`, `wpt2_html_semantics_scripting_1`,
  `wpt2_html_semantics_embedded_content`, `wpt2_html_semantics_tabular_data`,
  `wpt2_html_semantics_interactive_elements`, `wpt2_html_semantics_text_level_semantics`,
  `wpt2_html_semantics_grouping_content`, `wpt2_custom_elements`, `wpt2_shadow_dom`, `wpt2_selection`,
  `wpt2_uievents`, `wpt2_pointerevents`, `wpt2_webcryptoapi`, `wpt2_compression`, `wpt2_resource_timing`,
  `wpt2_performance_timeline`, `wpt2_console`, `wpt2_html_infrastructure_urls` (floor 0 each; a debug build
  skips them unless `AETHER_WPT=1`), and `wpt2_directory_table_is_complete` (runs in every build: table =
  tests = sparse lines). Each prints `wpt <dir> (served N test pages): pass/total`, the skipped pages by
  reason, and the count of pages without testharness.js; per-file results go to `target/wpt-<dir>.tsv`
  (`/` → `-`).
- `tests/wpt2_gaps.rs` (headless, no network): `g1_urlsearchparams_per_url_standard`,
  `g2_textdecoder_per_encoding_standard`, `g3_queuing_strategies_and_size`, `g4_blob_and_file_constructors`,
  `g5_table_caption_thead_tfoot_tbody`, `g6_get_random_values_checks`, `g7_subtle_crypto_symmetric`,
  `g8_performance_timeline_delivers`, `g9_console_namespace_and_labels`.
- lib: `api::subtle::tests::subtle_primitives_known_answers`.
- The existing net directories (`wpt_websockets`, `wpt_eventsource`, `wpt_xhr`) run through the same,
  generalised `run_served_dir` with their unchanged `net_excluded`; their scores should not move.

## AETHERWPT3 (SR138) — the AETHERWPT2 gap list O1–O9

The gaps AETHERWPT2 named (the table above, rows O1–O9), each implemented to its specification or refused by name
with the reason. **done** = implemented in this series (spec section, the `wpt3_gaps.rs` test that pins it);
**refused** = not built, with why. R107: no executor ran WPT or the tests; the bench's `wpt2_*` run on the fold is
the measurement (the directories each row names should rise from their AETHERWPT2 floors).

| # | directories | what landed | status |
|---|---|---|---|
| O1 | streams, encoding, compression, fetch/api | The Streams Standard rewritten as its abstract operations in `api/streams.rs` (new; installed by `js/net.rs` ahead of the fetch prelude, which keeps only Body / Blob over it): §4 ReadableStream with the default AND byte controllers (`ReadableByteStreamController`, `ReadableStreamBYOBRequest`, pull-into descriptors, `autoAllocateChunkSize`, `min`), default and BYOB readers, default and byte tee, `pipeTo` / `pipeThrough` (§4.9.1 ReadableStreamPipeTo: backpressure through the writer's ready promise, the four propagation rules, AbortSignal shutdown with its reason), WebIDL async iteration and `ReadableStream.from`; §5 WritableStream / writer / controller (erroring, in-flight requests, `signal`); §6 TransformStream / controller (backpressure, `cancel`, `terminate`); §7 the strategies (moved from fetch.rs); Encoding §5 / §6 `TextEncoderStream` (a surrogate pair split across chunks) / `TextDecoderStream`; Compression Streams §5 `DecompressionStream` (`deflate`, `deflate-raw`, `gzip` over pixel_core's inflater — the tree's one inflater, found in `unaos/libs/media/pixel_core/src/inflate.rs`; junk after the end and a truncated stream are TypeErrors) | done — `wpt3_gaps::o1_streams_writable_transform_pipes_bytes_codecs`, `api::streams::tests::inflate_from_states`. **Stopped (a file outside the arc):** `CompressionStream` — the tree's one DEFLATE encoder is `unaos/libs/vcs/git_core/src/deflate.rs` (`deflate`, `zlib_compress`; pixel_core and http_core only inflate), and `handlers/aether/Cargo.toml` has no git_core dependency; the change is the one line `git_core = { path = "../../unaos/libs/vcs/git_core" }` under `[dependencies]`, after which a `__deflate(format, bytes)` native (`git_core::deflate::deflate(b, 6)` for deflate-raw, `zlib_compress` for deflate, a 10-byte gzip header + the raw stream + `pixel_core` crc32 / ISIZE for gzip) and a CompressionStream class shaped like DecompressionStream (buffer, compress at flush) complete it. Refused: transferable streams (postMessage transfer needs the cross-realm transform over a MessagePort, Streams §8.2) |
| O2 | encoding, html/infrastructure/urls | The Encoding Standard's legacy decoders in `api/encoding/` (new): §9 single-byte (27 indexes), §10 gb18030 / GBK (four-byte ranges), §11 Big5 (the four two-code-point pointers), §12 EUC-JP (JIS X 0212), ISO-2022-JP (the escape state machine), Shift_JIS (the EUDC range), §13 EUC-KR, each the specification's byte handler with "prepend to the queue" as a push-back stack; the §4.2 label table and the §5 indexes generated by `api/encoding/gen_index.py` from the WHATWG `indexes.json` / `encodings.json` (whatwg/encoding a985b62a, sha256 b7c89610…); TextDecoder (api/platform.rs) streams every legacy encoding through `__codec_decode` with its state carried between `{stream: true}` calls and `fatal` honoured (the old UTF-8 fallback is gone); HTML §13.2.3 document decoding in `dom/charset.rs` (new): BOM, the transport `charset`, the `<meta>` prescan of the first 1024 bytes (§13.2.3.2, with the meta-content extraction and the UTF-16 → UTF-8 / x-user-defined → windows-1252 corrections), the windows-1252 default — used for nested documents (`api::fetch::fetch_subdocument`), whose `document.characterSet` is the encoding used (`DocMeta.charset`, js/dom.rs) | done — `wpt3_gaps::o2_legacy_decoders`, `api::encoding::tests::legacy_decoders_known_answers`, `dom::charset::tests::sniff_order`. **Stopped (a file outside the arc):** the TOP-LEVEL document still decodes as UTF-8 only — `handlers/aether/src/net/mod.rs` `fetch_document_final` (:170 `String::from_utf8(bytes).context("Invalid UTF-8")?`) and its siblings at :243, :261, :320, :700, :897 refuse a legacy-charset page outright; the change is to replace each with `crate::dom::charset::decode_html(&bytes, <the response's Content-Type value>).0` and carry the second element (the encoding) into the top-level `DocMeta.charset` (js/mod.rs:865, `charset` field added by this commit). Refused: the legacy ENCODERS (Encoding §9–§13 encoders): their consumers are the URL parser's query encoding and form submission in a legacy-charset document (html_core::url, not this arc's files), which a top-level legacy document (above) would need first |
| O3 | FileAPI, url | The blob URL store (File API §8.3) in `api/blobstore.rs` (new; process-wide so a worker's URLs serve its page): `blob:<origin>/<UUID v4>` from `URL.createObjectURL` (a Blob or File; MediaSource stays js/mse.rs's), `revokeObjectURL` removing exactly the URL given when the caller's origin made it, resolution without the fragment and partitioned by origin for fetches (navigations exempt); Fetch §4.3 blob scheme in `fetch()` and `XMLHttpRequest` (Content-Length / Content-Type, a single `Range` → 206 with Content-Range, GET only), the entry captured at URL parsing (a Request or an opened XHR survives a revoke); a nested navigation to a blob: URL (`fetch_subdocument`, decoded as its type) and scripts / worker scripts from one (`js::host::local_script`); `FileReaderSync` (File API §7: array buffer, binary string, text with the label / Blob charset / BOM rules, data URL) installed in dedicated workers by the one line in workers/dedicated.rs; XHR's text response now decodes with the response charset (O2's decoders) | done — `wpt3_gaps::o3_blob_url_store_fetch_and_file_reader_sync`, `api::blobstore::tests::register_resolve_revoke`. Not reached: a worker's own `fetch()` of a blob: URL (workers/realm.rs's fetch, outside this arc's files) and `<img src=blob:>` (the image loader is net/'s) |
| O4 | custom-elements | `ElementInternals` and form-associated custom elements in `js/internals.rs` (new): `attachInternals()` (HTML §4.13.7's NotSupportedError steps: customized built-in, no definition, `disabledFeatures: ["internals"]`, already attached, before the constructor), `shadowRoot`, `form`, `setFormValue` (string, File or a FormData cloned at the call; `state`), `setValidity` (the ten ValidityStateFlags, a message required with any flag, CRLF → LF, the anchor a shadow-including descendant or NotFoundError), `willValidate` / `validity` / `validationMessage` / `checkValidity` / `reportValidity`, `labels`, `states` (CustomStateSet), the ARIA mixin's stored values; a form-associated custom element is listed (`form.elements`, `fieldset.elements`), labelable, submittable (its submission value in "construct the entry list", js/forms.rs), resettable, disabled by its attribute or a fieldset, barred when disabled / readonly / in a datalist (js/controls.rs); `formAssociatedCallback`, `formDisabledCallback`, `formResetCallback` kept by the registry (dom/custom.rs: four more Callback kinds, a Bool argument) and enqueued as reactions — form owner and disabled changes observed at each reaction flush (js/custom.rs) | done — `wpt3_gaps::o4_element_internals_form_associated`. Refused: the `:state()` pseudo-class (selector matching is css_core's, not this arc's), `formStateRestoreCallback` (no session history restore or autofill to call it), the parser's form-element-pointer association (form-associated-callback.html's bogus `</form>` in a table: html_core's) |
| O5 | pointerevents | Pointer capture (Pointer Events §10) in `js/pointer.rs` (new): `Element.setPointerCapture` (NotFoundError for an inactive pointerId, InvalidStateError when disconnected, nothing outside the active buttons state) / `releasePointerCapture` / `hasPointerCapture` (the PENDING override), "process pending pointer capture" (`lostpointercapture` at the old override — at the document when it left the tree — and `gotpointercapture` at the new) before each pointer event, the implicit release after `pointerup`, implicit capture of a touch pointer at `pointerdown` (`pointer::down` with `Kind::Touch`); and the trusted pointer events themselves (§11.1, js/events.rs): the engine's mouse (pointerId 1, `mouse`, primary) fires `pointerdown` / `pointerup` / `pointermove` / `pointerover` / `pointerout` / `pointerenter` / `pointerleave` before their compatibility mouse events, both at the capture target when captured, a canceled `pointerdown` suppressing the compatibility events until release | done — `wpt3_gaps::o5_pointer_capture`. Refused: pointer lock (no Pointer Lock API here), a touch / pen input source (the shell delivers mouse events only), the `click` retargeting to the capture target (the click is routed by lib.rs, outside this arc's files) |
| O6 | resource-timing | Resource Timing L2 and Navigation Timing L2 in `js/timeline.rs`: `PerformanceResourceTiming` (every attribute, `toJSON`, `serverTiming` empty) recorded by "mark resource timing" from what each fetch observed — `fetch()` and `XMLHttpRequest` (api/fetch.rs `__rt_record`, api/xhr.rs: fetch start, the head, the end; decoded size, encoded size from Content-Length under a Content-Encoding, `transferSize` = encoded + 300; Fetch's TAO check — same-origin, or `Timing-Allow-Origin` `*` / this origin — else the detailed timings, `nextHopProtocol` and `transferSize` are 0, and an opaque response exposes no sizes or status) and external scripts (js/loader.rs → `note_resource` → `__perf_drain`); the resource timing buffer (§4.4: 250 by default, the secondary buffer, `resourcetimingbufferfull` with `onresourcetimingbufferfull`, `setResourceTimingBufferSize`, `clearResourceTimings`); `PerformanceNavigationTiming` for the document (type `navigate`, its DOM lifecycle stamped by js/loader.rs "the end": domInteractive … loadEventEnd), in `getEntries*` from the start and queued to observers once after load; `navigation` and `resource` in `supportedEntryTypes` | done — `wpt3_gaps::o6_resource_and_navigation_timing`. **Stopped (a file outside the arc):** `tests/wpt2_gaps.rs` `g8_performance_timeline_delivers` asserts the pre-O6 world — its expected string must become `…|http://127.0.0.1:8000/wpt2.html,a,m,b|…|mark,measure,navigation,resource|…` (getEntries now leads with the document's navigation entry, and supportedEntryTypes lists the two new types, both per the specifications); that test file is not in this arc's list. Not measured: DNS / connect / TLS phases (the transport reports none: domainLookup*/connect*/requestStart equal fetchStart, as for a reused connection), redirects (redirectStart/End 0), images / stylesheets / iframes (their loaders are net/'s and lib.rs's), Navigation Timing L1 (`performance.timing` / `performance.navigation`) |
| O7 | WebCryptoAPI | RSASSA-PKCS1-v1_5 / RSA-PSS / RSA-OAEP (generate, sign / verify, encrypt / decrypt) over the new `crypto_core::rsa_private` (RFC 8017: CRT private operations with a fault check, PKCS #1 v1.5 / PSS signing, OAEP, generic-hash verification, FIPS 186-5 A.1.3-shaped key generation); ECDSA / ECDH on P-256 and P-384; Ed25519; X25519; AES-CBC (SP 800-38A §6.2 + PKCS #7) and AES-KW (RFC 3394) over the new FIPS 197 §5.3 inverse cipher in `crypto_core::aes` (bitsliced, table-free); `spki` / `pkcs8` (RFC 5280, RFC 5208, RFC 5915, RFC 8410 through the strict DER of `api/der.rs`) and `jwk` (RFC 7517/7518/8037 with WebCrypto's use / key_ops / ext / alg checks) for every family; `wrapKey` / `unwrapKey` (§14.3.11/12: the op-then-encrypt normalization, jwk as UTF-8 JSON); the encapsulation KeyUsages | done — `wpt3_gaps::o7_subtle_crypto_asymmetric_and_formats`, `api::subtle::tests::subtle_cbc_and_kw_known_answers`, `api::der::tests::der_round_trips`, crypto_core `aes::tests::fips197_appendix_c_inverse`, `rsa_private::tests::generate_sign_verify_encrypt_decrypt`. Refused by name: P-521 (crypto_core has no P-521 field; NotSupportedError), Ed448 / X448 and the tentative Modern Algorithms (ML-KEM / ML-DSA / ChaCha20-Poly1305 / AES-OCB / KMAC / Argon2 / the `raw-*` formats — not registered, NotSupportedError / TypeError), multi-prime RSA JWKs (`oth`, NotSupportedError), RSA exponents above 2^64 in generateKey (OperationError) |
| O8 | html/browsers/*, navigation-api | Auxiliary browsing contexts and `window.open` in `js/frames.rs` over the AETHERFRAMES realms: HTML §7.2.2.1 the window open steps (the URL parsed against the source document — SyntaxError; the target defaulting to `_blank`; "tokenize the features argument" and "parse a boolean feature" exactly, `noreferrer` implying `noopener`; the chosen navigable navigated, or its initial about:blank kept; null under noopener) from the page and from any nested or auxiliary window; §7.3.1.7 the rules for choosing a navigable (`_self` / `_parent` / `_top`, a case-sensitive name match over the page's contexts, the sandboxed auxiliary navigation flag refusing a popup from a sandbox without `allow-popups`, the sandbox inherited); §7.3.1.4 an auxiliary context as a top-level traversable with its own realm (the creator's origin for its about:blank, an opaque one without an opener), `opener` (and `opener = null` disowning), `top` / `parent` itself, `name`, `close()` (script-closable: `closed` true at once, `beforeunload` / `pagehide` / `unload` at the queued close, its iframes first), iframes inside it scanned and served, navigations of it fetched and its scripts run. The Navigation API (HTML §7.2.6) in `js/navigation.rs` (new): `navigation` with `entries()` / `currentEntry` / `updateCurrentEntry()` / `transition` / `activation` / `canGoBack` / `canGoForward` / `navigate()` (state, info, history behavior) / `reload()` / `traverseTo()` / `back()` / `forward()` and their committed / finished promises; `NavigationHistoryEntry` (key, id, index, getState, `dispose`), `NavigationDestination`, `NavigateEvent` (the constructor's required members; trusted UA events; `intercept()` with handler / focusReset / scroll and its SecurityError / InvalidStateError checks; `scroll()`), `NavigationCurrentEntryChangeEvent`, `NavigationTransition`, `NavigationActivation`; the navigate event fired for Location navigations (fragment, cross-document, reload — js/window.rs), `pushState` / `replaceState` (after their SecurityError check; a canceled event leaves URL and state alone), and same-document traversals (js/history.rs); an intercepted navigation is the URL and history update steps alone (no fetch, no `hashchange`, classic state null), then `currententrychange`, the handlers, `navigatesuccess` / `navigateerror`; a newer navigation aborts the ongoing one (its signal, `navigateerror`, the promises reject with AbortError) | done — `wpt3_gaps::o8_auxiliary_contexts_and_navigation_api`. **Stopped (files outside the arc):** a `window.open` window as a shell TAB — the arc's "through the shell's existing tab verbs" needs `handlers/aether/src/lib.rs` (a `take_auxiliary_opens()` beside `take_pending_nav`, fed from js/frames.rs `create_aux`) and `vessels/aether-shell/src/main.rs` (`open_tab()` on it, then the tab's engine adopting the in-process context so `opener` survives); until then the auxiliary context lives in the page's engine, script-visible but not painted. Left for the fold: `tests/wpt_dom.rs`'s `window.open(` exclusion predicates (:261, :694, :995) — lifting them is a WPT measurement R107 does not let this executor make. Refused by name: bfcache (no back/forward cache: `pageshow.persisted` stays false, NotRestoredReasons absent); per-iframe `navigation` objects (a nested realm shares the top's `history` and `navigation`, as AETHERFRAMES shares the binding); the Navigation API's entries before this document's load (the shell's session history holds them) and cross-document traversal promises; `precommitHandler` (tentative); popup window features (`popup`, `width` / `left`, BarProp — no window geometry in an engine without windows); `window.open`'s user-activation gate and `document.domain`-based familiarity |
| O9 | shadow-dom, css/cssom, selection | Constructable stylesheets in `js/stylesheets.rs` (new): CSSOM §6.1 `StyleSheet` / `CSSStyleSheet` (the constructor with `baseURL` / `media` / `disabled`, `MediaList`, `cssRules` / `rules` as a live `CSSRuleList`, `insertRule` — SyntaxError, IndexSizeError, `@import` refused in a constructed sheet — `deleteRule`, `addRule` / `removeRule`, `replace()` swapping the rules in a task and resolving with the sheet while `replaceSync` / `insertRule` / `deleteRule` throw NotAllowedError, `replaceSync` dropping `@import`); §6.4 the rule objects parsed by css_core (`CSSStyleRule` with `selectorText` and its declaration block — `cssText`, `getPropertyValue`, `setProperty`, `removeProperty` — `CSSMediaRule` / `CSSSupportsRule` / `CSSContainerRule` / `CSSLayerBlockRule` as grouping rules with their own insert / delete, `CSSLayerStatementRule`, `CSSFontFaceRule`, `CSSKeyframesRule`, `CSSNamespaceRule`, `CSSImportRule`) serialized per §6.4.4; §6.1.2 `adoptedStyleSheets` on Document and ShadowRoot as an ObservableArray (assignment from any iterable, indexed set, `push` / `pop` / `length`; a non-sheet TypeError; a sheet of another document NotAllowedError); and the document's adopted sheets IN THE CASCADE after its own sheets (`css/mod.rs` `set_adopted` / `with_adopted`, taken by every cascade of that document's arena — the engine's restyle and the script-side layout flushes — re-cascaded on every change to an adopted sheet). Selection API §3 `getComposedRanges()` in `js/selection.rs` (options `{shadowRoots}` and the earlier variadic form: the selection's ends rescoped out of every shadow tree that is not one of the given roots or inside one, to before / after its host; a non-ShadowRoot TypeError), with the selection now held inside one shadow tree (`setBaseAndExtent` / `collapse` / `extend` / `selectAllChildren` take points whose tree's shadow-including root is the document); Selection API §3 `modify()` over a caret model of the RENDERED text in `js/caret.rs` (new): `move` / `extend`, `forward` / `backward` / `left` / `right`, the nine granularities, CSS Text §4.1.1 white-space collapsing (`pre` / `pre-wrap` / `pre-line` honoured), `<br>`, preserved newlines and block edges as breaks, `display: none` and replaced elements skipped, words as runs of non-whitespace (symbols included), a range collapsed toward the direction by `move` + `character` | done — `wpt3_gaps::o9_adopted_stylesheets_composed_ranges_and_modify`. **Stopped (files outside the arc):** the extended grapheme clusters of `modify('character')` — the tree's UAX #29 segmenter is `unaos/libs/text/edit_core::unicode` (`next_grapheme` / `prev_grapheme`, `word_boundaries`), not an aether dependency: `edit_core = { path = "../../unaos/libs/text/edit_core" }` in `handlers/aether/Cargo.toml` would let js/caret.rs's `combining()` approximation and its whitespace-run words become UAX #29's. Refused by name: shadow-tree SCOPING of a ShadowRoot's adopted sheets (they are kept on the ShadowRoot but not cascaded — the cascade has no tree scopes; shadow `<style>` elements are not scoped either); `document.styleSheets` and the CSSOM of `<style>` / `<link>` sheets (`sheet`, `ownerNode`); `@import` in constructed sheets (refused by the spec); soft-wrapped lines — `modify`'s `line` / `lineboundary` follow hard line breaks, since mapping a boundary point to its layout line box needs a position → fragment map over `layout_core::hit::{text_boxes, align, line_of}` that this series did not build; bidi (`left` / `right` are the left-to-right mapping); a selection spanning two trees (one end in a shadow tree, the other outside: the live Range the selection carries needs one root); `modify` with the focus inside a shadow tree (the caret model reads the document tree) |

## AETHERWPT4 (SR142) — the exclusion predicates measured, ten groups lifted

### Measurement (step 1: reading the predicates and counting, no suite run — R107)

The runner's three exclusion predicates — `excluded()` (the nine `DIRS`, where an excluded file still RUNS and only
leaves the covered subset), `net_excluded()` (websockets / eventsource / xhr) and `wpt2_excluded()` with
`run_wpt2_dir`'s pre-filter (the 27 `WPT2_DIRS`, where an excluded page is NOT run) — were ported line for line
(helper-script scan, `.any.js` window-global rule, `needs_server`, `net_files` variants and TLS variants, the
`PY_HANDLERS` / `WS_HANDLERS` lists read out of `wpt_net/mod.rs`) and applied to the pinned checkout (da1f6d20,
the same sparse set plus the directories' siblings). Counts are test pages (page × variant for the served
directories); "needs server" (`DIRS` files not run) and crash / tool / reference pages are not exclusions and are
left out. **2635 excluded before, 2197 after this arc.**

Per directory (excluded pages; the reasons with their counts):

| directory | run → run after | excluded → after | reasons before (count) |
|---|---|---|---|
| dom/nodes | 253 | 6 → 4 | XML documents / network 3, frames 2, layout 1 |
| dom/events | 175 | 96 → 88 | testdriver 82, layout 7, frames (not fully active) 4, XML / network 3 |
| dom/ranges | 67 | 10 → 10 | layout 7, testdriver 3 |
| dom/traversal, dom/collections, dom/lists | 18, 11, 5 | 0 | — |
| html/dom | 20 | 2 → 1 | testdriver 1, layout 1 |
| html/semantics/forms | 477 | 134 → 121 | testdriver 105, layout 22, frames 6, XML / network 1 |
| html/webappapis | 312 | 45 → 14 | layout 19, frames 13, XML / network 9, testdriver 2, user prompts 2 |
| websockets | 182 → 185 | 344 → 341 | TLS variants 315, bfcache 6, pywebsocket handlers 5, WebSocketStream 5, wptserve handlers 5, window.open / document.domain 3, TLS 2, cross-origin 1, idlharness 1, IDN 1 |
| eventsource | 54 | 9 → 9 | wptserve handlers 6, cross-origin 2, window.open / document.domain 1 |
| xhr | 136 → 219 | 196 → 113 | cross-origin 72, **XML / document responses 38**, wptserve handlers 39, **FormData 18**, **upload progress events 15**, **legacy charsets 8**, **blob: URLs 3**, window.open / document.domain 2, idlharness 1, HTTP auth 1, TLS 1 |
| url | 41 → 42 | 3 → 2 | window.open 1, idlharness 1, handler 1 |
| encoding | 1255 | 7 → 7 | handlers 5, idlharness 1, TLS 1 |
| fetch/api | 116 | 72 → 72 | cross-origin 42, TLS 15, handlers 9, window.open 3, h2 / service worker 3 |
| streams | 83 | 2 → 2 | idlharness 1, TLS 1 |
| FileAPI | 60 → 62 | 24 → 22 | manual 11, TLS 5, idlharness 3, cross-origin 3, window.open 2 |
| webstorage | 44 → 50 | 10 → 4 | window.open 8, TLS 1, cross-origin 1 |
| IndexedDB | 221 | 12 → 12 | cross-origin 3, TLS 3, bfcache 2, window.open 2, testdriver 1, idlharness 1 |
| html/browsers/history | 98 → 125 | 50 → 23 | window.open 27, manual 9, cross-origin 9, bfcache 2, testdriver 2, handler 1 |
| html/browsers/the-window-object | 39 → 94 | 62 → 7 | window.open 56, manual 2, TLS 2, cross-origin 1, testdriver 1 |
| html/browsers/browsing-the-web | 135 → 186 | 200 → 149 | window.open 58, bfcache 54, testdriver 44, cross-origin 17, TLS 11, layout 9, handlers 6, media 1 |
| html/semantics/scripting-1 | 378 → 393 | 149 → 134 | handlers 95, cross-origin 21, layout 15, worker-only globals 12, TLS 4, manual 2 |
| html/semantics/embedded-content | 389 → 418 | 432 → 403 | cross-origin 145, media fixtures 127, handlers 80, layout 34, testdriver 18, window.open 11, TLS 7, bfcache 6, manual 4 |
| html/semantics/tabular-data | 28 | 4 → 4 | layout 4 |
| html/semantics/interactive-elements | 36 → 47 | 76 → 65 | testdriver 51, layout 17, manual 8 |
| html/semantics/grouping-content, text-level-semantics | 14, 10 | 1, 0 | manual 1 |
| custom-elements | 169 → 177 | 17 → 9 | layout 7, testdriver 5, media 3, handler 1, window.open 1 |
| shadow-dom | 130 → 164 | 147 → 113 | testdriver 98, layout 49 |
| selection | 92 | 42 → 42 | testdriver 36, layout 4, manual 1, idlharness 1 |
| uievents | 15 | 62 → 62 | testdriver 57, manual 4, idlharness 1 |
| pointerevents | 7 → 12 | 260 → 255 | testdriver 224, TLS 24, manual 7, layout 5 |
| WebCryptoAPI, compression | 160, 17 | 3, 3 | idlharness, media |
| resource-timing | 84 → 88 | 66 → 62 | cross-origin 24, handlers 17, TLS 12, worker-only 6, window.open 4, idlharness 1, h2 1, layout 1 |
| performance-timeline | 32 | 19 → 19 | bfcache 18, idlharness 1 |
| console | 6 | 6 → 6 | testdriver 5, idlharness 1 |
| html/infrastructure/urls | 5 → 54 | 64 → 15 | window.open 49, TLS 10, cross-origin 2, handlers 2, layout 1 |

The groups (a reason, over every directory), largest first, with what each needs:

| group | excluded before | after | what it needs |
|---|---|---|---|
| testdriver (WebDriver input automation) | 735 | 736 | a testdriver backend: the stand-in serving an Aether `testdriver-vendor.js` (as it substitutes `testharnessreport.js`) over an engine automation surface (click / send_keys / action sequences / bless) |
| cross-origin | 343 | 361 | a second origin with CORS in the stand-in |
| websockets TLS variants (wss) | 315 | 315 | a wss server in the stand-in |
| un-emulated wptserve handlers | 265 | 270 | the handlers re-implemented in `wpt_net` |
| **window.open (auxiliary browsing contexts)** | 222 (+ 12 of `DIRS` frames, + 4 net) | 0 | — lifted (W1) |
| **layout / rendering** | 203 | 69 | getComputedStyle, elementFromPoint, scrollIntoView, requestAnimationFrame lifted (W2, W3, W7, W9); geometry (getBoundingClientRect, offset*, client*, scroll*) stays — EYES is its oracle |
| media fixtures (/media/ not in the sparse checkout) | 134 | 134 | `/media/` in `SPARSE` |
| TLS (https) | 96 | 96 | a TLS server |
| bfcache / remote contexts | 88 | 88 | the dispatcher (`/common/dispatcher/`) and a back/forward cache |
| manual | 49 | 49 | a person |
| **XHR XML / document responses** | 38 | 0 | — lifted (W4) |
| **XHR FormData** | 18 | 0 | — lifted (W5) |
| worker-only globals | 18 | 18 | the `.any.worker.html` variant in the stand-in |
| idlharness | 17 | 17 | `/interfaces/` in `SPARSE` |
| **XML documents / network (`DIRS`)** | 16 | 6 | the network half lifted (W6); XML documents loaded as documents stay |
| **XHR upload progress events** | 15 | 0 | — lifted (W8) |
| **XHR legacy charsets** | 8 | 0 | — lifted (W10) |
| WebSocketStream, pywebsocket handlers | 5, 5 | 5, 5 | WebSocketStream; the handlers |
| frames (not fully active) | 4 | 4 | documents of navigated-away iframes |
| **XHR blob: URLs** | 3 | 0 | — lifted (W10; the store is AETHERWPT3 O3) |
| user prompts, IDN, HTTP auth | 2, 1, 1 | 4, 1, 1 | manual; DNS; HTTP authentication |

The groups a modern page hits are the APIs, not the infrastructure: testdriver is the largest by far, but what it
needs is the harness's vendor glue (`wpt_net`, whose edits this arc may make to predicates only) — it is the first of
AETHERWPT5's ten. Spec directories outside the run set are not excluded by a predicate (they are not in `SPARSE`);
for the record, their testharness-candidate file counts at da1f6d20: content-security-policy 883, css/css-view-transitions
490, navigation-api 457, wasm 408, css/css-typed-om 370, service-workers 299, scroll-animations 266, css/cssom-view 243,
trusted-types 229, webrtc 228, html/interaction/focus 201, editing 189, web-animations 166, intersection-observer 154,
html/semantics/interactive-elements/the-dialog-element 150, webmessaging 136, css/css-highlight-api 133,
html/semantics/popovers 117, fullscreen 96, domparsing 77, event-timing 71, largest-contentful-paint 72, clipboard-apis 58,
cookiestore 56, close-watcher 49, scheduler 39, resize-observer 38, sanitizer-api 38, notifications 25, reporting 25.

### The ten groups (step 2)

| # | group (excluded pages lifted) | specification | what landed | `wpt4_gaps.rs` |
|---|---|---|---|---|
| W1 | window.open — auxiliary browsing contexts (222 + 12 in `DIRS` + 4 net) | HTML §7.2.2.1 window open steps, §7.3.1.4 auxiliary browsing contexts | the implementation is AETHERWPT3 O8's (js/frames.rs); this arc lifts `window.open(` from all three predicates as O8 left for the fold (`<frameset` / `<object` and `document.domain` stay) | `w1_window_open_auxiliary_context` |
| W2 | getComputedStyle (85 files carried the token; 77 had no other layout token) | CSSOM §9 resolved values; Cascade 5 §6 (cascade sort), §7.2 inheritance, §7.3 CSS-wide keywords | `css::author_declarations` (css/mod.rs): each element's author declarations in css_core's cascade order, its adopted sheets and style attribute included; `js/cssom.rs` `cascaded_value`: the declarations folded into a declaration block (shorthands expanded, the last winning, serialized as `element.style` serializes), `inherit` / `initial` / `unset` / `revert`, an undeclared inherited property from the parent — for every property the 20-property layout report does not compute (it still answers its own) | `w2_get_computed_style_cascade` |
| W3 | requestAnimationFrame (39 / 32 alone) | HTML §8.10 animation frames | the implementation is AETHERJS's (js/window.rs `run_raf_pass`: one timestamp per frame, cancel inside a frame); the token lifted | `w3_request_animation_frame` |
| W4 | XHR XML / document responses (38) | XHR §3.6.7 `responseType` ("document" ignored outside a Window), §3.6.8 `responseXML`, §3.6.10 document response; XML 1.0 (5th ed.) §2–§4 well-formedness, Namespaces in XML 1.0 (3rd ed.) §3–§6; HTML §8.5.1 DOMParser | `api/xml.rs` (new): a non-validating XML parser enforcing well-formedness and namespace well-formedness (XML declaration, doctype and internal-subset general entities, CDATA, comments, PIs, character / entity references, attribute-value normalization, end-of-line handling, prefix binding and the `xml` / `xmlns` rules, duplicate expanded attribute names); `__parse_xml` / `__doc_meta` / `__html_prescan` / `__xml_sniff` / `__decode_bytes` / `__get_encoding` natives (api/platform.rs); the document builder over the DOM's `createElementNS` / `setAttributeNS`; XHR's document response (HTML through html_core with the final encoding, else the §13.2.3.2 prescan, else UTF-8; XML through api/xml.rs, null when not well-formed; content type, URL and encoding set; kept as the response object); DOMParser's XML types (the parsererror document on failure, the DOMParserSupportedType TypeError, two arguments required) | `w4_xhr_document_response_and_xml_parser`, `api::xml::tests::{well_formed_and_namespaces,not_well_formed,declared_encoding_sniff}` |
| W5 | XHR FormData (18) | XHR §4.3 FormData (AETHERFORMS); Fetch §5 Body "package data" FormData — the multipart/form-data parser, the urlencoded parser | `Body.formData()` (api/fetch.rs, was a ledgered rejection): Fetch's multipart/form-data parser (boundary, `Content-Disposition: form-data; name=…; filename=…` with `%0A` / `%0D` / `%22`, `Content-Type`, a filename making a File typed `text/plain` by default), URL §5.1 for application/x-www-form-urlencoded, TypeError otherwise | `w5_form_data_body_parsers` |
| W6 | XML documents / network in `DIRS` (16 → 6) | XHR, Fetch (AETHERNET SR70); Fetch's data: URL processor | the `XMLHttpRequest` / `fetch(` half of the predicate lifted (the network APIs run since SR70); the data: URL processor fixed: the body percent-decoded to BYTES (it went through `decodeURIComponent`, so `%C2%F0` threw), `;base64` with spaces before it, a MIME type that does not parse becoming `text/plain;charset=US-ASCII` | `w6_network_apis_in_dom_tests` |
| W7 | scrollIntoView (16 alone) | CSSOM View §7 `scrollIntoView` ("scroll an element into view", ScrollIntoViewOptions, the boolean form) | `js/cssom.rs` `scroll_into_view` (was a no-op): block / inline `start` / `center` / `end` / `nearest`, every scroll-container ancestor asked through `layout::request_element_scroll` (the next frame, as `scrollTo`), the viewport offset moved at once and clamped to the document's extent, the enum TypeError | `w7_scroll_into_view` |
| W8 | XHR upload progress events (15) | XHR §3.5.6 send steps 5–11 (upload listener flag, upload complete flag, "process request end-of-body"), the request error steps step 6 | `api/xhr.rs`: the flags, `loadstart` at the upload object, `progress` / `load` / `loadend` there before the response head, the error event and `loadend` at an incomplete upload; `__net_has_listeners` (js/net.rs) | `w8_xhr_upload_progress_events` |
| W9 | elementFromPoint (9) | CSSOM View §6.1 `elementFromPoint` / `elementsFromPoint` | `js/cssom.rs` `hit_test`: boxes containing the point, positioned layers (by `z-index`) above the flow and later tree order above earlier, `pointer-events: none` / `visibility: hidden` skipped, the root element last, null / empty outside the viewport or for a document without one | `w9_element_from_point` |
| W10 | XHR legacy charsets (8) and blob: URLs (3) | XHR §3.6 "get a response MIME type", "get a final MIME type", "get a final encoding", `overrideMimeType()`; MIME Sniffing §4.4 parse / serialize; Fetch "extract a MIME type"; Encoding §6 decode; XML §4.3.3 | `api/xhr.rs`: MIME types parsed and serialized, the override's charset over the response's (a label naming no encoding → none → UTF-8), the text response's BOM-first decode, the XML declaration's encoding for an XML response, the author Content-Type's charset rewritten to UTF-8 for a string / Document body, a Document body serialized as HTML or XML, the blob response typed with the final MIME type; blob: URLs are AETHERWPT3 O3's | `w10_xhr_final_mime_type_and_encoding` |

### What the fold should expect (R107: no executor ran the suites)

- Predicates (`tests/wpt_dom.rs`): `excluded()` loses `window.open(`, `XMLHttpRequest`, `fetch(` and the
  `getComputedStyle` / `elementFromPoint` / `scrollIntoView` / `requestAnimationFrame` tokens; `net_excluded()` loses
  `window.open(` and the five xhr reasons above (HTTP authentication stays); `wpt2_excluded()` loses `window.open(` and
  the same four layout tokens. Excluded pages 2635 → 2197 (by the count above).
- `DIRS` floors: only `dom/nodes` has one (80%); its covered set gains 2 files (a `window.open` file and a
  `getComputedStyle`-only one); one frameset / object file and the three XML-document files stay excluded. Every `wpt2_*` floor is 0, so a newly served page that fails
  cannot red them; `wpt_xhr` keeps its 85% floor over 83 more served pages — W4 / W5 / W8 / W10 are what those pages
  read; `open-url-encoding.htm` (2 subtests) still fails (a windows-1252 TOP-LEVEL document: AETHERWPT3 O2's stop).
- Wall time grows: ~250 more served pages, most in html/browsers/* and html/infrastructure/urls (each bounded at
  30 s real time by `run_net_file`).
- `tests/wpt4_gaps.rs`: `w1_window_open_auxiliary_context`, `w2_get_computed_style_cascade`,
  `w3_request_animation_frame`, `w4_xhr_document_response_and_xml_parser`, `w5_form_data_body_parsers`,
  `w6_network_apis_in_dom_tests`, `w7_scroll_into_view`, `w8_xhr_upload_progress_events`, `w9_element_from_point`,
  `w10_xhr_final_mime_type_and_encoding`; lib: `api::xml::tests::{well_formed_and_namespaces, not_well_formed,
  declared_encoding_sniff}`.

Refused by name in this arc: DTD validation, external entities and attribute-list defaults (a non-validating
processor need not read them); XHR's synchronous request error steps still fire events before the NetworkError (the
spec throws first — not one of the lifted groups); ShadowRoot's `elementFromPoint` (DocumentOrShadowRoot's retargeting);
stacking contexts nested below the root's in hit testing (one level: positioned vs flow, by `z-index`); `scroll` events
for the viewport offset `scrollIntoView` moves (the viewport offset is the script-side one `window.scroll` records).

### Next ten for AETHERWPT5 (by the AFTER count)

1. testdriver — 736 pages (dom/events 82, html/semantics/forms 106, shadow-dom 98, pointerevents 224, uievents 57,
   interactive-elements 51, browsing-the-web 44, selection 36, …): `wpt_net` serving an Aether `testdriver-vendor.js`
   (as it substitutes testharnessreport.js) over an engine automation native — click / send_keys / action sequences
   / bless / set_permission — dispatched through the shell's input path.
2. cross-origin — 361: a second origin (port and host name) with CORS in the stand-in.
3. un-emulated wptserve handlers — 270 (scripting-1 95, embedded-content 82, xhr 39, resource-timing 17).
4. websockets TLS variants — 315: a wss endpoint.
5. media fixtures — 134: `/media/` in `SPARSE`.
6. TLS (https) — 96.
7. bfcache / remote contexts — 88: the dispatcher and a back/forward cache.
8. layout geometry — 69 (getBoundingClientRect / offset* / client* / scroll*), against EYES.
9. worker-only globals — 18: the `.any.worker.html` variant.
10. idlharness — 17: `/interfaces/` in `SPARSE`.

Beyond the predicates, the API directories outside the run set (the counts above) are the next arcs' run set:
popovers and dialog, IntersectionObserver / ResizeObserver, Web Animations, CSS Typed OM, the Clipboard API,
Fullscreen, close-watcher.
