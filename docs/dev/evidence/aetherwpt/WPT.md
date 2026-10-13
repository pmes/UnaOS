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
| O5 | pointerevents | owed in this series | open |
| O6 | resource-timing | owed in this series | open |
| O7 | WebCryptoAPI | RSASSA-PKCS1-v1_5 / RSA-PSS / RSA-OAEP (generate, sign / verify, encrypt / decrypt) over the new `crypto_core::rsa_private` (RFC 8017: CRT private operations with a fault check, PKCS #1 v1.5 / PSS signing, OAEP, generic-hash verification, FIPS 186-5 A.1.3-shaped key generation); ECDSA / ECDH on P-256 and P-384; Ed25519; X25519; AES-CBC (SP 800-38A §6.2 + PKCS #7) and AES-KW (RFC 3394) over the new FIPS 197 §5.3 inverse cipher in `crypto_core::aes` (bitsliced, table-free); `spki` / `pkcs8` (RFC 5280, RFC 5208, RFC 5915, RFC 8410 through the strict DER of `api/der.rs`) and `jwk` (RFC 7517/7518/8037 with WebCrypto's use / key_ops / ext / alg checks) for every family; `wrapKey` / `unwrapKey` (§14.3.11/12: the op-then-encrypt normalization, jwk as UTF-8 JSON); the encapsulation KeyUsages | done — `wpt3_gaps::o7_subtle_crypto_asymmetric_and_formats`, `api::subtle::tests::subtle_cbc_and_kw_known_answers`, `api::der::tests::der_round_trips`, crypto_core `aes::tests::fips197_appendix_c_inverse`, `rsa_private::tests::generate_sign_verify_encrypt_decrypt`. Refused by name: P-521 (crypto_core has no P-521 field; NotSupportedError), Ed448 / X448 and the tentative Modern Algorithms (ML-KEM / ML-DSA / ChaCha20-Poly1305 / AES-OCB / KMAC / Argon2 / the `raw-*` formats — not registered, NotSupportedError / TypeError), multi-prime RSA JWKs (`oth`, NotSupportedError), RSA exponents above 2^64 in generateKey (OperationError) |
| O8 | html/browsers/* | owed in this series | open |
| O9 | shadow-dom, selection | owed in this series | open |
