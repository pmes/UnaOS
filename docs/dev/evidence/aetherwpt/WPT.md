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

## AETHERWPT5 (SR144) — testdriver, a second origin, the handlers

### Measurement (step 1: the predicates read over the pinned checkout, no suite run — R107)

The same port of the three predicates as AETHERWPT4's (`excluded`, `net_excluded`, `wpt2_excluded` with
`run_wpt2_dir`'s pre-filter, the helper-script scan, the variants, `py_refs` / `wpt2_py_refs`, `PY_HANDLERS`) applied to
da1f6d20 reproduces SR142's after count exactly — **2197 excluded pages, testdriver 736, cross-origin 361** — and is
the yardstick for this arc's lifts: **2197 before, 1082 after** (the script is a 300-line Python port kept in the
seat's scratch; its per-directory output is the table below). The testdriver group by the WebDriver command the
excluded pages used: `test_driver.Actions` 410, `send_keys` 137, `click` 83, `bless` 38, `set_test_context` 10,
`get_computed_label` 6, `bidi` 5, `get_computed_role` 2, and one each of `set_window_rect`, `get_window_rect`,
`create_window`, `navigate`, `minimize_window`. The handler group, by the pages naming each `.py`: two were not
handlers at all — `html/canvas/tools/gentest.py` (56) and `moving-between-documents/tools/generate.py` (52) are
generators a generated page names in its comment — then `delayed-broken-video.py` 14, `serve-with-content-type.py` 12,
`log.py` 8, `download_stash.py` 6, `echo-headers.py` / `infinite-redirects.py` / `fake_responses.py` 4, nine at 3 and
2, and a tail of singles; lifting cross-origin exposed the CORS handlers behind it (`fetch/api/resources/preflight.py`
15, `reset-token.py` / `access-control-preflight-denied.py` 6, `access-control-*.py` 3 and 2 each, `multi_redirect.py`
3).

| directory | run → after | excluded → after | what stays (count) |
|---|---|---|---|
| dom/events | 175 | 88 → 42 | layout 38, documents not fully active 4 |
| dom/ranges | 67 | 10 → 7 | layout 7 |
| html/semantics/forms | 477 | 121 → 29 | layout 23, frames 5, XML documents 1 |
| html/webappapis | 312 | 14 → 13 | user prompts 4, layout 3, frames 3, XML 2, testdriver beyond 1 |
| websockets | 185 → 191 | 341 → 335 | wss variants 315, bfcache 6, WebSocketStream 5, TLS 2, idlharness 1, IDN 1, four pywebsocket handlers, `check.py`'s sibling `redirect_response` served |
| eventsource | 54 → 58 | 9 → 5 | `eventsource/message.py` (a path that does not exist) 2, worker `message.py` 2, document.domain 1 |
| xhr | 219 → 294 | 113 → 38 | HTTP authentication (`auth*/`, `access-control-auth-basic.py`) 17, preflight-cache handlers 5, `gzip.py` / `zlib.py` 2, `xhr/content.py` and other paths that do not exist 7, idlharness 1, TLS 1, document.domain 1 |
| url, encoding | 42 → 43, 1255 → 1260 | 2 → 1, 7 → 2 | idlharness; TLS |
| fetch/api | 116 → 165 | 72 → 23 | TLS 15, h2 / service worker 3, `network-partition-key.py` (not in the checkout) 2, three handler singles |
| FileAPI, webstorage, IndexedDB | 62 → 65, 50 → 53, 221 → 227 | 22 → 19, 4 → 1, 12 → 6 | manual, TLS, idlharness, bfcache |
| html/browsers/history | 125 → 136 | 23 → 12 | manual 9, bfcache 2, `delay.py` (not in the checkout) 1 |
| html/browsers/the-window-object | 94 → 96 | 7 → 5 | manual 2, TLS 2, testdriver beyond 1 |
| html/browsers/browsing-the-web | 186 → 254 | 149 → 81 | bfcache 54, TLS 11, get-host-info's HTTPS origins 6, layout 6, media 1, three handler singles |
| html/semantics/scripting-1 | 393 → 492 | 134 → 35 | worker-only globals 12, `cookies/resources/set-cookie.py` (not in the checkout) 4, TLS 4, `echo-referrer.py` (a path that does not exist) 3, manual 2, handler singles |
| html/semantics/embedded-content | 418 → 644 | 403 → 177 | media fixtures 130, layout 19, bfcache 16, TLS 7, manual 4, `timeout_on_seek.py` 1 |
| html/semantics/interactive-elements | 47 → 86 | 65 → 26 | layout 17, manual 8, media 1 |
| custom-elements | 177 → 179 | 9 → 7 | testdriver beyond (`get_computed_label`) 3, media 3, `form-echo.py` 1 |
| shadow-dom | 164 → 257 | 113 → 20 | layout 17, testdriver beyond (`get_computed_label`) 3 |
| selection | 92 → 121 | 42 → 13 | layout 11, manual 1, idlharness 1 |
| uievents | 15 → 63 | 62 → 14 | layout 9, manual 4, idlharness 1 |
| pointerevents | 12 → 178 | 255 → 89 | layout 58, TLS 24, manual 7 |
| resource-timing | 88 → 122 | 62 → 28 | TLS 12, worker-only 6, `content-type.py` (not in the checkout) and `multi_redirect` siblings, idlharness 1 |
| console | 6 → 11 | 6 → 1 | idlharness 1 |
| html/infrastructure/urls | 54 → 58 | 15 → 11 | TLS 10, `stash.py` 1 |

The groups after this arc, largest first: wss variants 315; **layout / rendering 212** (69 before — the testdriver
pages that also read `getBoundingClientRect` / `offset*` / `client*` / `scroll*` now reach that predicate, which
`testdriver` had hidden; EYES stays its oracle); media fixtures 139; bfcache 98; TLS 96; manual 49; worker-only
globals 18; idlharness 17; testdriver commands beyond the vendor 15; frames 9 + 4; TLS for get-host-info's HTTPS
origins 7; XML documents 6; WebSocketStream 5; user prompts 4; handlers 31 pages over 24 files (17 of them HTTP
authentication, 7 naming a path that does not exist in the checkout, `gzip.py` / `zlib.py` needing a deflate
compressor the tree does not have); document.domain 2; IDN 1; HTTP authentication 1.

### The three (step 2)

| # | group (pages lifted) | specification | what landed |
|---|---|---|---|
| 1 | testdriver (736 → 15) | WebDriver §12.4.1 Element Click, §12.5.1 Element Send Keys, §15.5 Perform Actions (§15.4.3 origins, §17.2 normalised key values), Permissions §3.2 Set Permission, §16 cookie commands; testdriver.js's `test_driver_internal` contract (`in_automation`) | `tests/wpt_net/mod.rs` `TESTDRIVER_VENDOR_JS`, served at `/resources/testdriver-vendor.js` by both servers (`wpt_dom.rs` `serve`, `wpt_net` `http_conn`): `click` (the in-view centre testdriver.js computed, moved into the top-level viewport through the frame chain), `send_keys` (the focusing steps, then the keys by node id), `action_sequence` (element origins by node id), `set_permission`, `get_all_cookies` / `get_named_cookie` / `delete_all_cookies`, `set_test_context`; `in_automation` true so the rest rejects at once. `src/js/testdriver.rs` (new): `__td_queue` / `__td_node`, the thread-local queue, the WebDriver key table, the permission store. `src/lib.rs` `AetherEngine::pump_automation`: each command through the shell's input path — click = `MouseMove` + `MouseDown` + `MouseUp` (`pointerdown` / `mousedown`, `pointerup` / `mouseup`, `click` with activation); send keys = focus (caret at the end of a text control), per code point `keydown`, the default action, `Text` for an unmodified printable key, `keyup`, modifiers held for the sequence; actions tick by tick (`pointerMove` by viewport / pointer / element-centre origin through `pointer_moved`, `pointerDown` / `pointerUp`, `keyDown` / `keyUp`, `scroll` = a `wheel` (`js/events.rs` `user_wheel`, new) then `Scroll`); then `__td_settle(ticket, error, result)`. The runners (`run_file`, `run_net_file`) settle in 200 ms virtual slices and pump between them. Predicates: `testdriver_beyond` names the commands the vendor does not implement. |
| 2 | cross-origin (361 → 0; 7 moved to TLS) | HTML §7.5 origins (a tuple origin differs by host and by port), Fetch §3.2 CORS protocol (the engine's `security/cors.rs`, since AETHERNET), wptserve's localhost configuration (`get-host-info.sub.js`: REMOTE_HOST and NOTSAMESITE_HOST are `127.0.0.1` when the host is `localhost`) | `wpt_net::HOST` = `localhost`, `run_net_file` loads `http://localhost:<port>/…`; `start` binds `{{ports[http][0]}}` and `{{ports[http][1]}}` on `127.0.0.1`, `127.0.0.2`–`127.0.0.6` and `[::1]`; `subst` maps `{{host}}` → `localhost`, `www` / `www1` → `127.0.0.1`, `www2` → `127.0.0.2`, `alt` → `127.0.0.3`, `alt.www*` → `127.0.0.4`–`6`, `nonexistent` → `nonexistent.invalid`, `location[host]` / `[port]` from the Host header, `header_or_default(...)`; `requri.py` answers `request.url` from the Host header. |
| 3 | un-emulated handlers (270 → 31) | each handler's `.py` at da1f6d20, cited by path in `PY_HANDLERS`; wptserve's `request.POST` (urlencoded and multipart/form-data), `response.set_cookie` (http.cookies' Morsel), the stash | 94 handlers in `wpt_net` `py()`: the scripting-1 set (`serve-with-content-type`, `log`, `serve-custom-response` with its stashed context, the four `load-error-events`, `delayed-modulescript`, `referrer-checker`, `cross-origin`, `script-not-found-not-executed`, `serve-text-then-js`, the two `http-accept-header-checker`, `record-fetch`), embedded-content (`delayed-broken-video`, `download_stash`, the four img handlers with their PNGs read from the checkout), html/browsers (`redirect`, `refresh` with its raw 0x80 0xFF header bytes, `slow`, `no-cache-single-redirect`, `unset_context_name_stash`), html/infrastructure/urls (`blank`, `echo-referrer-text`), xhr (`echo-headers`, `infinite-redirects`, `parse-headers`, `json-with-bom`, `win-1252-xml`, `invalid-utf8`, `echo-content-type`, `get-set-cookie`, `bad-chunk-encoding`, `upload`, `chunked` with its trailer, `last-modified`, `accept`, `conditional`, `form`, and 22 `access-control-*` / CORS handlers), resource-timing (`fake_responses`, `empty`, `TAOResponse`, `status-code`, `redirect-without-location`, `resource-timing-content-length`, `redirect-cors`, `cacheable-and-validated`, `multi_redirect`), encoding (`single-byte-raw`, `json-charset`, `text-plain-charset`, `text-html-meta-charset`), url (`percent-encoding`), fetch/api (`preflight` with its stashed dictionary, `bad-chunk-encoding`, `script-with-header`, `cache`, `redirect-empty-location`, `stashed-chunk`), eventsource (`cors` delegating to the handlers it runs, `cors-cookie`), FileAPI (`echo-content-escaped`), websockets (`set-cookie`, `redirect_response`, `check`), common (`slow-redirect`). `py_refs` ignores a `.py` under `tools/` (108 pages). |

### What the fold should expect (R107: no executor ran the suites)

- Predicates: `excluded()` and `wpt2_excluded()` lose `testdriver (WebDriver input automation)` and gain
  `testdriver (WebDriver commands beyond input automation)` (15 pages); `net_excluded()` and `wpt2_excluded()` lose
  `cross-origin (no second origin with CORS in the stand-in)`; `wpt2_excluded()` gains `needs a TLS server (the HTTPS
  origins of get-host-info)` (7). Excluded pages 2197 → 1082 by the count above; the `layout / rendering` count rises
  69 → 212 as explained.
- Served pages grow by ~900 (pointerevents 12 → 178, embedded-content 418 → 644, shadow-dom 164 → 257, scripting-1
  393 → 492, xhr 219 → 294 …); each is bounded at 30 s real time by `run_net_file`, so wall time grows with them —
  the pointerevents directory alone is 166 more pages, most of which drive the one mouse pointer the engine has.
- The DIRS runner (`run_file`) now settles in slices: a page that completes early returns at the same virtual time
  as before; a page that never completes still ends at 15 s virtual.
- `wpt_xhr` keeps its 85% floor over 75 more served pages: the CORS pages now run against the engine's own
  `security/cors.rs` (the preflight, the CORS check, the response-header filter) — a floor movement there is a CORS
  finding, not a stand-in one. Every `wpt2_*` floor is 0.
- The first page of a run that reaches `localhost` resolves it through the system resolver; the stand-in listens on
  `[::1]` as well as `127.0.0.1`, so either address order connects at once. A bench whose loopback refuses
  `127.0.0.2`–`127.0.0.6` prints `wpt_net: cannot listen on …` and the pages naming those hosts fail; it never panics.
- Wire shape of a testdriver page: `click` → `pointerdown` / `mousedown` / `pointerup` / `mouseup` / `click` at the
  element's centre; `send_keys("ab")` on an input → `keydown a`, `input`, `keyup a`, `keydown b`, `input`, `keyup b`
  with the value `ab`; a `bless` resolves through its injected button's click.

Refused by name in this arc: a second pointer (the engine has one, the mouse — js/pointer.rs — so a `pointerType`
of `touch` or `pen` drives it as the mouse, `pointerId` 1; the pointerevents pages that assert on the type fail and
count), buttons other than the primary (a `pointerDown` with `button` 2 presses the primary; no `contextmenu`),
`get_computed_label` / `get_computed_role` (the a11y name computation wants the rendering; 9 pages stay excluded),
the BiDi modules and the window-placement, navigation, virtual-authenticator, sensor, FedCM and reporting commands
(the `testdriver_beyond` list), HTTP authentication handlers (`auth*/auth.py`, `authentication.py`,
`access-control-auth-basic.py`: the engine has no credential prompt — the standing `HTTP authentication` reason),
`gzip.py` / `zlib.py` (a compressor), `timeout_on_seek.py` and the preflight-cache handlers (`access-control-basic-
preflight-cache*.py`, `no-custom-header-on-preflight.py`: a stash-counted preflight cache; next), and the pages naming
a `.py` path that is not in the checkout (`xhr/content.py`, `eventsource/message.py`, `module/echo-referrer.py`,
`cookies/resources/set-cookie.py`, `fetch/connection-pool/…`, `fetch/content-type/…`: the predicate names them
truthfully).

### Next for AETHERWPT6 (by the AFTER count)

1. websockets TLS variants — 315: a wss endpoint (a TLS server in the stand-in; with it the 96 `.https.` pages and
   get-host-info's HTTPS origins, 7).
2. layout geometry — 212 (`getBoundingClientRect` / `offset*` / `client*` / `scroll*`), against EYES; the largest
   share is now pointerevents (58) and dom/events (38), pages that read a rect to aim the pointer.
3. media fixtures — 139: `/media/` in `SPARSE`.
4. bfcache / remote contexts — 98: the dispatcher and a back/forward cache.
5. manual — 49 (a person).
6. worker-only globals — 18: the `.any.worker.html` variant.
7. idlharness — 17: `/interfaces/` in `SPARSE`.
8. testdriver beyond the vendor — 15: `get_computed_label` / `get_computed_role` over the a11y tree (9), window
   placement, BiDi.
9. the handler tail — 31: HTTP authentication (17), the preflight cache (5), a compressor (2).
10. a second pointer type for the pointerevents pages that run but assert `pointerType` (not an exclusion; a score).
## AETHERAPIS (SR145) — the API directories outside the run set

The last paragraph of AETHERWPT4's "Next ten" names the API directories a modern page reaches for that no runner
serves: popovers and dialog, IntersectionObserver / ResizeObserver, Web Animations, CSS Typed OM, the Clipboard API,
Fullscreen, close-watcher. This arc measures them, implements them as far as one arc reaches, and gives them their own
runner (`handlers/aether/tests/wpt_apis.rs`). R107: no executor ran WPT, a test or QEMU; the fold's run is the
measurement. The commit carrying this section is the one on `exec-host-aetherapis` (parent e57d1b0e).

### Measurement (step 1: the run set and the pinned checkout, 2026-10-13)

**Which of the seven the runner served today.** `tests/wpt_dom.rs` runs `DIRS` (9), `NET_DIRS` (3) and `WPT2_DIRS`
(27); its `SPARSE` carries no `css/css-typed-om`, `web-animations`, `intersection-observer`, `html/semantics/popovers`,
`fullscreen`, `clipboard-apis`, `close-watcher` or `resize-observer` — none of those is in the run set. The one
exception is the dialog element: `html/semantics/interactive-elements/the-dialog-element` is a subdirectory of
`html/semantics/interactive-elements`, which `wpt2_html_semantics_interactive_elements` runs recursively (floor 0), so
its 86 testharness pages already ran and failed on the stubs (the AETHERWPT4 table counts them under
`html/semantics/interactive-elements`: 47 run, 65 excluded, "testdriver (dialog, popover, details input)"). This runner
lists it too, so the dialog's score is read beside the popovers' (the two share the top layer); the fold may drop one
of the two runs if the wall time matters.

**Page counts at da1f6d20** (a sparse checkout of the nine directories, counted with the runner's own rules: an
`.html` / `.htm` that loads testharness.js, a `.window.js`, an `.any.js` whose `META: global` names window;
`resources/`, `support/`, `tools/`, `crashtests/` and `reference/` left out; "other" = reftests, crash tests, support
pages without testharness.js). "testdriver" = pages that load testdriver.js directly (the runner also scans their
helper scripts, so the served count at the fold is a little lower):

| directory | testharness pages | testdriver | https | manual | other | pages this runner serves (no testdriver, no TLS, no second origin, no handler) |
|---|---|---|---|---|---|---|
| css/css-typed-om | 359 (the-stylepropertymap 288, of them properties/ 246; stylevalue-subclasses 46; serialization 11; normalization 5; objects 4) | 0 | 0 | 0 | 13 | 359 |
| web-animations | 144 (animation-model 41, interfaces 47, timing-model 42, responsive 53 minus reftests, animation-trigger 3) | 3 | 0 | 0 | 40 crash tests | 140 (one page fetches `resources/xhr-doc.py`) |
| intersection-observer | 150 | 0 | 0 | 0 | 5 | ~140 (the `.sub.html` cross-origin iframe pages are excluded) |
| html/semantics/interactive-elements/the-dialog-element | 86 | 46 | 0 | 0 | 97 | 40 |
| html/semantics/popovers | 85 | 53 | 0 | 1 | 45 | 32 |
| fullscreen | 90 | 75 | 4 | 0 | 11 | 9 (six `.sub.html` allowfullscreen pages and the idlharness page excluded) |
| clipboard-apis | 61 | 51 | 53 | 0 | 0 | 5 (the Clipboard interface is [SecureContext]; the http pages are the ClipboardEvent / DataTransfer ones) |
| close-watcher | 49 | 47 | 0 | 0 | 0 | 2 |
| resize-observer | 34 | 0 | 0 | 0 | 7 | ~33 |

By served pages, the order of work was: Typed OM (359), IntersectionObserver (~140), Web Animations (140, already
implemented), the dialog (40), ResizeObserver (~33), popovers (32), Fullscreen (9), Clipboard (5), CloseWatcher (2).
The testdriver pages (275 of the 1058) are AETHERWPT5's first group: when its `testdriver-vendor.js` lands, this runner's
predicate drops the `testdriver` reason and the popover, dialog, fullscreen, clipboard and close-watcher pages that
drive a real click or an Escape join the served set unchanged.

**What Aether had at e57d1b0e** (grep of `handlers/aether/src`):

- popover: nothing but the `popoverTargetAction` reflection rows (`js/reflect_table.rs:443-444`); no `popover` IDL
  attribute, no `showPopover`, no top layer, no ToggleEvent for popovers.
- dialog: `show()` / `showModal()` set the `open` attribute and `close()` removes it and fires `close` synchronously
  (`js/html.rs:1779-1790`); no modal flag, no top layer, no toggle / cancel events, no `returnValue`, no
  `requestClose`, no focusing steps; no UA `display: none` for a closed dialog (layout_core's hidden-tag list
  `tree.rs:122` has no `dialog`; `css/mod.rs` had no UA sheet).
- IntersectionObserver / ResizeObserver: the COMPAT shim of `api/platform.rs:1167-1182` — `observe()` ledgers
  `observer-never-delivers:<kind>` and nothing is ever delivered.
- Web Animations: AETHERSTYLE3B/3C's `js/animations.rs` (SR115 / SR122): `Element.animate`, `Animation`,
  `KeyframeEffect`, `AnimationEffect`, `DocumentTimeline` / `document.timeline`, `getAnimations`, `commitStyles`,
  `ready` / `finished`, CSSAnimation / CSSTransition — over `layout_core::anim::Timeline`.
- CSS Typed OM: nothing (no `attributeStyleMap`, `computedStyleMap`, `CSSUnitValue`, `CSS.px`).
- Clipboard: the shell seam only — `AetherEngine::copy_selection` (`lib.rs:497`) answers `BrowserCopy` with
  `BrowserClipboardSet(text)` and `paste(text)` (`lib.rs:1222`) inserts the shell's text into the focused control;
  no `navigator.clipboard`, no `ClipboardItem`, no `ClipboardEvent` constructor.
- Fullscreen: `document.fullscreenElement` as a null getter (`js/window.rs:1626`); no `requestFullscreen`,
  `exitFullscreen`, `fullscreenEnabled`, no events.
- CloseWatcher: nothing. User activation: `crate::media`'s sticky flag (SR134), set by a trusted click; no transient
  activation anywhere.

### What landed (step 2), per specification

| feature | specification | what landed (new files under `handlers/aether/src/js/`) |
|---|---|---|
| CSS Typed OM | CSS Typed OM Level 1: §3 StylePropertyMapReadOnly / StylePropertyMap, §4 CSSStyleValue / CSSKeywordValue / CSSUnparsedValue / CSSVariableReferenceValue, §5 CSSNumericValue, CSSUnitValue, CSSMathValue family, §5.6 the type algebra, §6 CSSTransformValue and its components, §6.7 serialization, §7 reification | `typed_om.rs` (a prelude over `element.style`, `getComputedStyle`, `CSSStyleRule.style`): `Element.attributeStyleMap`, `Element.computedStyleMap()`, `CSSStyleRule.styleMap`; `get` / `getAll` / `has` / `size` / iteration; `set` / `append` / `delete` / `clear` validated against a property grammar table (`PROPS`: which numeric base types a property takes, its keywords, list-valued, non-negative, integer-valued) — the wrong numeric type, a keyword outside the grammar, an unsupported value bound to another property, a list for a non-list property: TypeError; an out-of-range number is written as `calc()` (so `width: -3.14em` reads back as `CSSMathSum`, as the tests expect); custom properties read and write `CSSUnparsedValue`; `CSSStyleValue.parse` / `parseAll`, `CSSNumericValue.parse`; every unit of §5.1 with the `CSS.px()`… factories (the `CSS` namespace created when absent); `add` / `sub` / `mul` / `div` / `min` / `max` / `equals` / `to` / `toSum` / `type` with "add two types" / "multiply two types" / "invert a type" and the percent hint; `calc()` / `min()` / `max()` / `clamp()` reified as the math tree (sum of products with negate / invert), `calc()` serialization with nested parentheses; `CSSTranslate` / `CSSRotate` / `CSSScale` / `CSSSkew` / `CSSSkewX` / `CSSSkewY` / `CSSPerspective` / `CSSMatrixComponent` with `is2D` and `toMatrix()` (4×4 products into a `DOMMatrix`), a transform list reified from `translate()` … `matrix3d()`; `CSSImageValue` for `url()` / gradients; computed values with relative lengths resolved to px (em / rem / ex / ch / vw / vh / …), times to s, angles to deg, same-unit sums folded, non-negative properties clamped, integer properties rounded |
| IntersectionObserver | Intersection Observer §2 (constructor options and validation, observe / unobserve / disconnect / takeRecords, IntersectionObserverEntry), §3.2.x "run the update intersection observations steps", "queue an intersection observer task", "notify intersection observers" | `observers.rs`: the implicit root is the viewport (`innerWidth` × `innerHeight`), an element root its border box; `rootMargin` / `scrollMargin` parsed (px or %, SyntaxError otherwise); thresholds validated (RangeError) and sorted; the threshold index and `isIntersecting` compared with the previous ones (edge-adjacent rects intersect), entries with `time` (`performance.now()`), `rootBounds`, `boundingClientRect`, `intersectionRect`, `intersectionRatio`, `target`, `isVisible`; the entries delivered in a task (`__obs('queue')` → `Task::Callback`). The update runs after every frame's animation-frame callbacks: `Engine::drain_raf_once` (HTML §8.1.7.3 steps 14 / 16), which `event_loop::settle` calls each turn |
| ResizeObserver | Resize Observer §3 (ResizeObserver with `box` options, ResizeObserverEntry, ResizeObserverSize), §4 the processing model: gather active observations at depth, broadcast, the depth limit, the loop error | `observers.rs`: the content box from `getBoundingClientRect()` less the computed paddings and borders, `borderBoxSize` / `contentBoxSize` / `devicePixelContentBoxSize`, `contentRect` at (padding-left, padding-top); observations whose size changed since the last report are delivered in the same rendering update, shallowest first, deeper targets changed by a callback in the next loop iteration, a shallower one skipped with the window `error` event "ResizeObserver loop completed with undelivered notifications." |
| the dialog element | HTML §4.11.4: `show()`, `showModal()`, `close()`, `requestClose()`, `returnValue`, `closedBy`, the dialog focusing steps, the `open` attribute change steps, the removing steps, `beforetoggle` / `toggle` ToggleEvents with the toggle task tracker, the `cancel` / `close` events, the close watcher a modal dialog establishes | `toplayer.rs`: the InvalidStateError cases (open modal vs modeless, not connected, open as a popover), `beforetoggle` cancelable before `show` / `showModal`, the modal flag and the top layer list, `closedBy` (`any` / `closerequest` / `none`, auto = closerequest when modal), the close watcher whose cancel action fires `cancel` (cancelable when the page has history-action activation or the request came from `requestClose()`), the focusing steps (autofocus delegate, first focusable descendant, else the dialog), focus restored to the previously focused element on close, `close` queued as a task, a dialog whose `open` attribute is removed by script closes (modal flag, top layer, watcher) and queues `close`; `css/mod.rs` gains a UA sheet at `Origin::UserAgent` with `dialog:not([open]) { display: none }` |
| popovers | HTML §6.12: the `popover` attribute (auto / manual / hint), `showPopover()` / `hidePopover()` / `togglePopover()`, "check popover validity", "show popover", "hide popover", "hide all popovers until", "topmost popover ancestor", the auto and hint stacks, §6.12.2 light dismiss, §6.12.3 the `popovertarget` / `popovertargetaction` activation behavior, `popoverTargetElement`, the popover focusing steps, the removing and attribute change steps | `toplayer.rs` + `reflect_table.rs` (the `popover` IDL attribute: empty = auto, invalid = manual, missing = null): NotSupportedError / InvalidStateError per the validity check, `beforetoggle` (cancelable) and `toggle` (a task, the tracker collapsing a show+hide into one event with the first old state), nested auto popovers hidden from the top down to the new popover's ancestor (flat tree and invoker chain), hint popovers above autos, the `source` option, the invoker element, the previously focused element restored on hide, a close watcher per auto / hint popover (Escape hides the topmost), light dismiss from a trusted click (`events::user_click`: the popovers above the one clicked, or its invoker, are hidden; `closedby=any` dialogs clicked outside get a close request), `popoverTargetElement` (the explicitly set element or the id in the tree scope) on button and input, the activation behavior called from `js/events.rs`'s button and input arms (a submit button with a form owner does nothing), a popover removed from the document is hidden without events, a showing popover whose `popover` attribute changes is hidden |
| CloseWatcher | HTML §6.6.5: the close watcher manager (groups, allowed number of groups raised by user activation, "next user interaction allows a new group"), establish / request to close / close / destroy / "process close watchers", the `CloseWatcher` interface with its `signal`, `requestClose()`, `close()`, `destroy()`, `cancel` / `close` events | `closewatcher.rs`: the manager in the prelude, the dialog and popover watchers through the hidden `__cw_make`, `close_request()` from an uncanceled Escape keydown (`events::user_key`), `note_user_activation()` from every activation; the `cancel` event cancelable only with history-action activation (consumed when a cancel prevents the close) or from script |
| user activation | HTML §6.4: sticky, transient (5 s), history-action activation; §6.4.5 `navigator.userActivation` | `activation.rs`: `note()` from a trusted click and a keydown that is not Escape or a modifier (`js/events.rs`), `consume()`, `has_transient()`; `crate::media`'s sticky flag is set from the same notification |
| Fullscreen | Fullscreen API §4 `requestFullscreen()` (the error conditions, `fullscreenerror`, `fullscreenchange`, the promise), §5 `fullscreenEnabled`, `fullscreenElement`, `fullscreen`, `exitFullscreen()`, the event handler attributes on Document and Element | `fullscreen.rs`: rejected (TypeError, `fullscreenerror` queued) when the element is not connected / not HTML or SVG / a dialog / a showing popover, when fullscreen is not enabled (a nested document without `allowfullscreen`) or without transient activation; else the activation is consumed, the element becomes the fullscreen element (`fullscreen::element()`, the shell's seam) and `fullscreenchange` is queued at it (and at the previous one); `exitFullscreen()` rejects without a fullscreen element; an element removed from the document leaves fullscreen with `fullscreenchange` at the document |
| Clipboard | Clipboard API and events §4 ClipboardEvent (its constructor's `clipboardData`), §5 `navigator.clipboard`: `read()`, `readText()`, `write()`, `writeText()`, §5.2 ClipboardItem (`types`, `getType()`, `presentationStyle`, `supports()`) | `clipboard.rs` + `events.rs` (ClipboardEvent registered with `clipboardData`): a write needs transient activation (NotAllowedError otherwise) and lands in the page's clipboard store (`clipboard::take_written()` for the shell to send as `BrowserClipboardSet`); a read needs it too (standing in for the `clipboard-read` permission) and answers the shell's text (`clipboard::set_platform_text`); rich items (`text/html`, `image/png`) stay in the page and read back as written; a type outside the mandatory ones is NotAllowedError on write |
| Web Animations | Web Animations Level 1 | already AETHERSTYLE3B/3C's (`js/animations.rs`); nothing changed in this arc — its directory joins the run set and its gaps are measured by the fold's run (below) |

Hooks into existing files (every one a no-op while nothing is showing): `js/mod.rs` (the eight installs after
`tracks::install`, the resets, `drain_raf_once` running `observers::update`), `js/dom.rs` (`remove` → the removing
steps; `attribute_changed` → the `popover` / dialog `open` steps), `js/events.rs` (button / input activation →
`toplayer::invoker_activation`; a trusted click → `activation::note`, the close watcher manager's notice, light
dismiss; keydown → activation or, for Escape, a close request; `ClipboardEvent`), `js/reflect_table.rs` (`popover`),
`css/mod.rs` (the UA sheet).

### The runner (`tests/wpt_apis.rs`) and its exclusion predicate

One `wpt_apis_<dir>` test per directory (`API_DIRS`, 9 entries; `apis_directory_table_is_complete` pins the table and
`apis_exclusions_are_infrastructure_only` the predicate), served by the same `wpt_net` stand-in through a copy of
`wpt_dom.rs`'s page loop (`run_page`: 200 ms settles, 30 s real / 16 s page time), with its own checkout locator whose
`SPARSE` is `wpt_dom.rs`'s plus the eight new directories (one `target/wpt` serves both runners; whichever runs first
widens it). `wpt_dom.rs` and `tests/wpt_net/**` are untouched (AETHERWPT5's). Floors are 0 at landing.

`apis_excluded(root, rel, src)` names, in this order: crash / tool / reference trees; a non-window `.any.js` global;
`.h2.` / `.serviceworker.` / `.sharedworker.`; `.https.` (a TLS server); `-manual.`; testdriver (`testdriver.js` or
`test_driver` in the page or any helper script it loads — the helper scan is `helper_sources`, so the popover and
dialog pages whose `clickOn` / `sendEscape` live in `popover-utils.js` / `common.js` are named); idlharness; bfcache /
remote contexts; cross-origin (`.sub.` pages, REMOTE / OTHER origins, `get-host-info`, `cross-origin` / `cross-site`
names); an un-emulated `.py` handler (outside `wpt_net::PY_HANDLERS`); `/media/` fixtures. **Not a reason here, unlike
`wpt2_excluded`: the layout tokens** (`getBoundingClientRect`, `offset*`, `client*`, `scroll*`): the intersection and
resize observers are geometry, their pages run against the engine's own layout (AETHERINLINE's boxes, taffy's sizes)
and fail honestly where it differs — those failures are the layout's measurement, not the observers'.

### What the fold should expect (R107: no executor ran the suites)

- `cargo test --release -p aether --test wpt_apis -- --nocapture --test-threads=4`: nine `wpt_apis_*` tests, each
  printing `wpt <dir> (served N test pages): P/T subtests (x%); wall S s` and its skipped pages by reason, plus the two
  table tests; all floors 0, so nothing reds on a score. Expected served counts near the table's last column; the
  skipped counts near its `testdriver` + `https` + cross-origin columns.
- The scores to read first: `css/css-typed-om` (the property pages are the bulk; each runs CSS-wide keywords, `var()`,
  the property's syntaxes and the rejection of the others), `intersection-observer` (the viewport-root pages; the
  element-root and clipped pages will show where no ancestor clipping is modelled), `resize-observer`, the dialog and
  popover pages that need no testdriver (`dialog-open`, `dialog-close-event*`, `dialog-showModal*`, `toggle-events`,
  `dialog-return-value`, `dialog-requestclose*`, `popover-events`, `togglePopover`, `toggleevent-interface`,
  `popover-types`, `popover-removal*`, `popovertarget-reflection`, `imperative-invokers`, `popover-stacking`).
- The `wpt2_html_semantics_interactive_elements` score (`wpt_dom.rs`) should rise with the dialog pages; nothing else
  in `wpt_dom` should move (the preludes install after everything else and redefine only the dialog operations, the
  `fullscreenElement` getter and the observer shims).
- `cargo test -p aether` (the lib tests): `engine_tests::test_ledger_records_unimplemented_apis` will red — see the
  fold note below.

### Gap list (dated 2026-10-13; what one arc did not reach)

1. **`:popover-open`, `:modal`, `:fullscreen` and `::backdrop`** are not parsed by css_core (`selectors.rs:637` knows
   `open` but none of these), so `el.matches(':popover-open')` throws SyntaxError and no UA rule can hide a hidden
   popover: `[popover]:not(:popover-open) { display: none }` cannot be written. `popover-utils.js`'s
   `assertPopoverVisibility` reads `getComputedStyle(popover).display` and `matches(':popover-open')`, so every page
   using it fails on those asserts even where the DOM half is right. The exact change (outside this arc's files):
   `unaos/libs/web/css_core/src/selectors.rs` — `ElementState::{PopoverOpen, Modal, Fullscreen}` and the three arms
   next to `"open" => P::State(S::Open)`; `handlers/aether/src/dom/select.rs` `state()` — arms answering from
   `js::toplayer` (the prelude's `__tl_is_popover_showing` / `__tl_is_modal` need a Rust mirror: a thread-local set
   fed by `__tl('track')`) and `js::fullscreen::element()`; then `css/mod.rs`'s `UA_SHEET` gains the popover rule,
   `dialog:modal` and `::backdrop`.
2. **The top layer is not painted** above the document (no `::backdrop`, no "blocked by a modal dialog" inertness in
   layout or hit testing, no centering of a modal dialog); `fullscreen::element()` is a seam the shell does not yet
   read (the viewport is not resized to it). The dialog layout pages (`centering`, `top-layer-position`,
   `abspos-dialog-layout`, `modal-dialog-scroll-height`, `inert-*`) measure this.
3. **Events the preludes dispatch are untrusted** (`isTrusted` false): the ToggleEvents, `cancel`, `close`,
   `fullscreenchange` / `fullscreenerror`, the CloseWatcher's events. A trusted path needs a native `__tl('fire')`
   over `events::new_event` / `dispatch` (the shape `queue_details_toggle` uses).
4. **IntersectionObserver**: no clipping by `overflow` ancestors other than the root, no `scrollMargin` effect, no
   `delay` / `trackVisibility` (`isVisible` is always false), no cross-document roots (iframes); entries for a root
   that is a Document other than the page's are not computed.
5. **ResizeObserver**: fragments (multi-column) report one box; `devicePixelContentBoxSize` rounds at
   `devicePixelRatio`; SVG targets use their border box.
6. **Typed OM**: `CSSColorValue` and its subclasses (`cssColorValue.html`, `cssRGB.html`, `cssHSL.html`, …) are not
   reified (a color reads as a base `CSSStyleValue`); `<position>` likewise; shorthands read as base values (per
   spec); `calc()` is simplified only for same-unit sums; the grammar table (`PROPS`) is the properties the tests
   exercise plus the common ones — a property outside it is "not supported" (TypeError on `get` / `set`), and a
   keyword list may miss a rare value. `CSSStyleValue.parse` validates with the same table.
7. **Web Animations**: untouched; the fold's `wpt_apis_web_animations` run is the first measurement of SR115 / SR122
   against WPT. Known from `animations.rs`'s own notes: per-keyframe `composite`, `iterationComposite`,
   `ScrollTimeline` / `ViewTimeline`, `new Animation(effect, timeline)` with a timeline other than the document's,
   `DocumentTimeline({ originTime })`, `Animation.timeline` setter, `Animation.replaceState` / `persist()`.
8. **Clipboard**: no permission model (transient activation stands in for `clipboard-read` / `clipboard-write`); no
   `copy` / `cut` / `paste` events from the shell's verbs (lib.rs's `paste` inserts text without a `paste` event);
   `read()` returns the shell's text as one `text/plain` item unless the page wrote rich items itself.
9. **Popovers**: `popover=hint` nesting follows the auto rules (no "hint stack above auto stack" invariants beyond
   "hints are hidden before autos"); no `interesttarget` (not in the pinned WPT); the `popovertarget` activation
   behavior runs for `input` types button / reset / submit / image only.
10. **CloseWatcher**: a `signal` that is already aborted makes an inactive watcher without throwing (per spec); the
    "allowed number of groups" is raised by every activation, not only the ones after the manager was notified.

### Notes for the fold (changes outside this arc's files, named exactly)

- `handlers/aether/src/engine_tests.rs` `test_ledger_records_unimplemented_apis` (lines 250 and 267-269): the fixture's
  `new IntersectionObserver(function () {}).observe(document.body);` no longer ledgers
  `observer-never-delivers:IntersectionObserver` (the observer delivers). The COMPAT shim in `api/platform.rs` still
  installs the four `mkObserver` constructors, but every one is replaced by a real implementation now (MutationObserver
  SR82, PerformanceObserver SR135, IntersectionObserver / ResizeObserver here), so no shim is left to stand in: delete
  that fixture line and the assertion (keep the CSS assertions), and the comment above it says the JS-side "shim
  reports itself" contract has no live example.
- `handlers/aether/src/lib.rs`: two lines wire the Clipboard API to the shell — in `paste(&mut self, text)` add
  `crate::js::clipboard::set_platform_text(text);` before the insertion, and a `pub fn clipboard_written(&self) ->
  Option<bandy::SMessage>` returning `crate::js::clipboard::take_written().map(bandy::SMessage::BrowserClipboardSet)`
  for the shell's frame loop to poll beside `copy_selection`.
- `unaos/libs/web/css_core/src/selectors.rs` and `handlers/aether/src/dom/select.rs`: gap 1 above.

## AETHERTOP (SR146) — the top layer: pseudo-classes, painting, inertness, trusted events, observer clipping

Branch `exec-host-aethertop`, cut from 8da376cb (AETHERAPIS folded at c369c236). The arc closes AETHERAPIS's gaps
1-4 above in that order; gap 6's `CSSColorValue` is not reached (typed_om.rs is not in the arc's files). No suite was
run (R107): the fold's `wpt_apis` run is the measurement.

### What landed, per spec section

1. **`:popover-open`, `:modal` (HTML §4.16.3) and `:fullscreen` (Fullscreen §6).** css_core parses them as host
   states (`unaos/libs/web/css_core/src/selectors.rs` `ElementState::{PopoverOpen, Modal, Fullscreen}` and the three
   arms beside `"open"`; css_core's own `html_state` answers false for them and `invalidation.rs` treats them as
   host-driven, untouched). Aether answers them in `handlers/aether/src/dom/select.rs` `state()` from a Rust MIRROR
   of the prelude's top layer (`handlers/aether/src/js/toplayer.rs` `TOP`, fed by `__tl('track', n, elements,
   kinds)` on every change: 1 a showing popover, 2 a modal dialog) and from `js::fullscreen::element()`, only in the
   page's own document (`is_page_doc`, the arena-identity check `focused_in` uses). A top-layer change bumps a
   generation (`toplayer::bump`, also from `fullscreen.rs` `__fs('set')`) that marks the DOM touched and makes
   `css/mod.rs` `restyle_plan` re-cascade (`StyleCache.toplayer_gen`): the states flip with no DOM mutation.
2. **The UA sheet** (`css/mod.rs` `UA_SHEET`; HTML §15.3 / §15.5 rendering, Fullscreen §7): a hidden popover is
   `display: none`; a showing popover and a modal dialog are `position: fixed` boxes of the viewport placed at its
   top-left (NOT the spec's `inset: 0; margin: auto` centering, which needs `width: fit-content` — layout_core has no
   `fit-content`; with both insets an auto width would stretch to the viewport); the popover's border, padding,
   `canvas` / `canvastext` colours; `:not(:root):fullscreen` fills the viewport; `::backdrop` is a fixed viewport
   box, a modal dialog's `rgba(0, 0, 0, 0.1)`.
3. **Painting the top layer** (HTML §15.5 "the top layer", CSS Position 4 §top-layer): `render/mod.rs` `frame_walk`
   skips each top-layer element's box where its parent would paint it (`hoisted_to_top_layer`, after the
   `display: none` check) and paints the top layer after the document in order (`paint_top_layer`, called after the
   root `draw_node` of both the index walk and the paint walk): per element, its `::backdrop` colour blended over the
   damage rects, then the element as its own stacking context from its parent's origin with the paint state inherited
   down its ancestor chain, clipped to the viewport. The backdrop colour is the winning `background-color` /
   `background` of the `::backdrop` rules matching the element (`css/mod.rs` `top_layer_backdrops`: Cascade 5 order
   by importance, origin, specificity, order). A top-layer generation change paints the frame whole
   (`relayout_damage`, `FrameSnap.top_gen`): a backdrop is not a box the damage snapshot sees.
4. **Inertness** (HTML §6.3 inert, §4.11.4 "blocked by a modal dialog"): `toplayer::is_inert(id)` — while a modal
   dialog is shown every node outside the topmost one's subtree (and outside the top-layer elements after it) is
   inert. `js/events.rs` `click_with_activation`: a trusted click on an inert node targets the modal dialog (the
   backdrop's originating element) and runs no activation behavior there; light dismiss still sees where it landed,
   so a `closedby=any` dialog closes on a backdrop click (§6.12.2). `user_key`: a key whose target is inert goes to
   the modal dialog.
5. **Trusted events** (DOM §2.2 `isTrusted`; the UA fires them in HTML §6.12 / §4.11.4 and Fullscreen §4): the
   prelude's `beforetoggle` / `toggle` (popover and dialog), `cancel`, `close` and the fullscreen prelude's
   `fullscreenchange` / `fullscreenerror` dispatch through the native `__tl('fire', target, event)` →
   `events::dispatch_trusted`.
6. **IntersectionObserver clipping** (Intersection Observer §3.2.6 "compute the intersection" step 3, §3.2.5 "root
   intersection rectangle", §2.2 scroll margin): `js/observers.rs` `clipByAncestors` clips the target's rect by the
   content clip of each flat-tree ancestor up to the root (an axis whose computed overflow is not `visible` is
   clipped to the padding box; a scroll container's clip grows by `scrollMargin`; for the implicit root the document
   element's and a propagated body's overflow are the viewport's); an element root with a content clip intersects
   at its padding area.

### What the fold should expect

- `wpt_apis_popovers` rises most: every page using `popover-utils.js`'s `assertPopoverVisibility` (`matches(
  ':popover-open')` no longer throws; a hidden popover computes `display: none`) — `popover-attribute-basic`,
  `popover-light-dismiss`, `popover-hidden-display`, `popover-types`, `popover-removal*`, `popover-stacking`,
  `popover-focus*` where no testdriver input is needed; plus every page reading `isTrusted` on a ToggleEvent.
- `wpt_apis_dialog` and `wpt2_html_semantics_interactive_elements` (`wpt_dom.rs`): `:modal` pages
  (`dialog-showModal`, `modal-dialog-*` matching asserts), `closedby-*` and `dialog-close-event*` reading
  `isTrusted`; `inert-*` partly (the script-visible half; pages that click through testdriver go through the hit
  test, which does not know the top layer — gap 2 below).
- `wpt_apis_fullscreen`: the `:fullscreen` pseudo-class pages and the `isTrusted` asserts on `fullscreenchange`.
- `wpt_apis_intersection_observer`: the clipped-target and element-root pages (`clip-path`-free ones such as
  `containing-block`, `client-rect`, `root-margin-root-element`, `scroll-margin*`, `isIntersecting-change-events`).
- Stay red: `centering`, `top-layer-position`, `abspos-dialog-layout`, `modal-dialog-scroll-height` (no
  `fit-content`, gap 1 below); Typed OM's colour pages (gap 7).
- Nothing in `wpt_dom` outside the dialog pages should move; a page with no popover, dialog or fullscreen element has
  an empty top layer, paints as before, and its restyle plan is unchanged (generation steady).

### Gap list (dated 2026-10-13; what this arc did not reach — each change outside its files, named exactly)

1. DONE (AETHERFIT SR189, below). **No `fit-content`** in layout_core (`unaos/libs/web/layout_core/src/css.rs`'s `width` arm has no keyword for it;
   grep finds none in layout_core): the spec's UA rules for popovers and modal dialogs (`inset: 0; margin: auto;
   width: fit-content; height: fit-content`) cannot be written, so both sit at the viewport's top-left instead of
   centred. Change: a `fit-content` width / height keyword (shrink-to-fit) in layout_core, then the UA sheet takes the
   spec's rules.
2. DONE (AETHERFIT SR189, below). **The hit test does not hoist the top layer** (`handlers/aether/src/lib.rs` `hit_origin` / `hit_context`, lines
   ~2085-2310, mirror render's paint order without the top layer): a click over a top-layer element can hit the
   document box beneath it. `events.rs` retargets a click on an inert node to the modal dialog, so a modal dialog's
   own controls are reachable only where they do not overlap document content. Change: test the top layer's boxes
   first, in reverse order (`js::toplayer::top_layer`), and skip inert boxes.
3. DONE (AETHERFIT2 SR191, below: `js/html.rs focus_op` refuses an inert element; AETHERFIT SR189 measured it a different rule: focus REFUSES, it does not retarget). **The focusing steps do not refuse inert elements** (`focus()` on an element outside a modal dialog still focuses
   it; HTML §6.6.3 "focusable area" excludes inert nodes): `handlers/aether/src/js/dom.rs`'s focus path needs
   `toplayer::is_inert`.
4. **The CloseWatcher interface's own events** (`cancel` / `close` on a page's `CloseWatcher`) are dispatched
   untrusted by `handlers/aether/src/js/closewatcher.rs` (not in this arc's files): route them through
   `events::dispatch_trusted` as toplayer.rs does.
5. DONE (AETHERFIT SR189, below). **Pointer, mouse and wheel events** (`events.rs` `pointer_event` / `user_mouse` / `user_button` / `user_wheel`) still
   reach inert nodes; only the click and keys are retargeted.
6. **`::backdrop` under `@media` / `@container`**: `top_layer_backdrops` reads the rule set without evaluating the
   conditions of the rule's ancestors; a backdrop's only painted property is its background colour (no image,
   gradient, border, filter or opacity).
7. **`CSSColorValue`** and its subclasses (AETHERAPIS gap 6) — typed_om.rs is not in this arc's files.
8. **The fullscreen element's place in the top layer** is approximated as last (Fullscreen §4 adds it where the
   request fell); the shell still does not size the viewport to it (AETHERAPIS gap 2).
9. **IntersectionObserver**: the flat-tree parent chain stands in for the containing-block chain (an absolutely
   positioned target is clipped by a non-positioned overflow ancestor it escapes); `clip-path` is not a clip; no
   `delay` / `trackVisibility` (AETHERAPIS gap 4's rest).

## AETHERFIT (SR189) — popovers and modal dialogs centred; the top layer hit first; one inert retarget

Closes AETHERTOP's gap list items 1, 2 and 5; item 3 (focus) is a different rule and stays owed
(`docs/dev/blueprint/handlers.md` §5). Design measured at df909c747, `MEASURE: PASS` from the seat.

### What landed, per spec section
- **CSS Sizing 3 §2.1 / §5.2 `fit-content`** (layout_core `css.rs` `parse_dimension_str`): the keyword becomes taffy
  0.15.1's own `Dimension::fit_content()`; taffy's sizing-keyword routine does the shrink-to-fit
  (min(max-content, max(min-content, available))) and its abspos auto-margin routine the centring. No shrink-to-fit
  code is written. In the block axis it is the automatic size (CSS Sizing 3 §3.1): `tree.rs passes`' IFC height
  checks read it as `auto` (`block_auto`). A `fit-content` width is content-sized (`has_width` false).
- **CSS 2.2 §10.1.3 / Position 3 §2.1, the viewport is a fixed box's containing block** (layout_core `tree.rs`
  `fixed_viewport`, `record_fixed`): for the run each fixed box's definite insets are rewritten as lengths from its
  parent's out-of-flow area, so taffy stretches / fit-contents / centres it against the viewport. The original
  insets are restored with the pass's other rewrites, so every style is byte-identical after the run (the KAT lays
  the same tree out twice). The viewport origin goes in ONE map, `LayoutTree::fixed_at`, which render
  (`placed_origin`), the hit test (`hit_origin`) and `scroll::box_origin` all read. It replaces the two copied
  placement closures (which ignored margins) and the third reading.
- **HTML §15.3.x / §15.5.x UA rules** (`css/mod.rs` `UA_SHEET`): `[popover]` and `dialog:modal` take the spec's
  `inset: 0; width: fit-content; height: fit-content; margin: auto`, so both are centred in the viewport.
- **The top layer is hit first** (`lib.rs hit_test_at`): `js::toplayer::top_layer()` in reverse, inert boxes
  skipped, each box found through render's factored `top_layer_box` and hit as its own context from its parent's
  `scroll::box_origin`. The document walk skips those boxes (`hit_collect`), the same rule render uses.
- **HTML §6.3 / §4.11.4, one retarget rule** (`toplayer::retarget`): an inert node's user interaction event goes
  to the topmost modal dialog. Key, click, `user_mouse`, `user_button`, `user_wheel` and the hover path
  (`pointer_moved`) all use it; the two inline copies are gone.

### Counts (host, measured on exec-rmbp-aetherfit; before = df909c747)
| runner | before | after |
|---|---|---|
| `cargo test -p layout_core` | 31 passed / 0 failed | 34 passed / 0 failed (+`tests/fit_content.rs`, 3) |
| `cargo test --release -p aether --lib` | 210 passed / 1 failed | 211 passed / 1 failed (+`test_hit_test_hoists_the_top_layer`; the failure is the foreign `js::idb::tests::transaction_ordering_kat` both times) |
| `wpt_apis` the-dialog-element (40 served) | 60/170 | 60/170 |
| `wpt_apis` popovers (28 served) | 97/136 | 98/136 (`popover-anchor-multicol-display.html` 0/2 -> 1/2) |
| `wpt_dom` interactive-elements (86 served) | 150/352 | 166/352 (testdriver clicks now reach the centred top layer: `dialog-popover-closedby-complex` 0/40 -> 10/40, `dialog-popover-closedby-simple` 0/6 -> 3/6, `dialog-cancel-preventDefault`, `dialog-light-dismiss-touch`, `simulated-click-inert` 0/1 -> 1/1 each) |

No page went down (per-page diff of both runs). `test_z_index_and_fixed_kat` now paints at the size it was laid
out at (RULING A: the engine relays out on Resize, so the layout viewport always equals the paint size).

### Why the centring pages did not move (each reads geometry somewhere this arc did not change)
- `centering.html` 0/23: it sizes nested iframes' viewports and reads their dialogs' rects.
- `multiple-centered-dialogs.html` 0/1: it reads `getBoundingClientRect()` after `window.scroll`, and
  `js/cssom.rs element_boxes` gives a fixed box's client rect without taking the scroll off (owed).
- `abspos-dialog-layout.html` 3/10, `non-modal-dialog-layout.html` 0/1: non-modal dialogs. The spec's
  `dialog { position: absolute; inset-inline: 0; width: fit-content; margin: auto }` is not written (owed).
- `modal-dialog-scroll-height.html` 0/1: a fixed box still adds to the document's `scrollHeight`.
- `top-layer-position.html` 0/1: it reads the computed `position` of a modal dialog whose inline style says
  `static` / `relative`. That is cascade priority on a UA rule, not layout.

## AETHERFIT2 (SR191) — focus refuses inert; MouseUp defaults retargeted; CSSOM fixed rects and the top layer; fixed % sizes; the sizing keywords

Closes AETHERFIT's owed list (a)-(e) (`docs/dev/blueprint/handlers.md` §5) and AETHERTOP gap 3. Design measured at
2879aed46, `MEASURE: PASS` from the seat; grants `js/html.rs` (the focus path is there at the cut, not in
`js/dom.rs`) and, after the first after-run, `js/toplayer.rs` (below).

### What landed, per spec section
- **HTML §6.6.3, an inert node is not a focusable area** (`js/html.rs focus_op`): `focus()` on an element
  `toplayer::is_inert` names REFUSES; it is never retargeted. The dialog focusing steps reach it through `el.focus()`.
  The first after-run took two pages DOWN (`dialog-focus-previous-outside.html` 4/6 -> 2/6, `dialog-closedby-start-
  open.html` 1/2 -> 0/2): `toplayer.rs closeDialog` restored focus BEFORE syncing the Rust top-layer mirror
  (`track()`), so the restore target was still inert to `is_inert` and was refused. HTML's close steps take the
  dialog out of the top layer before the restore; `closeDialog` and `hidePopover` now call `track()` at that
  point (the trailing one stays). Both pages are back at baseline.
- **HTML §6.3, the shell's MouseUp defaults** (`lib.rs`): the focus starting point, media toggle and the link /
  control / label walk act on the RETARGETED hit through ONE tail fn `retargeted`, which `pointer_moved` also uses.
  The JS dispatch keeps the raw hit (`user_button` / `click_with_activation` retarget themselves; light dismiss needs
  to know the click fell outside the dialog).
- **CSS 2.2 §9.6.1 / CSSOM View §6.1, §7** (`js/cssom.rs`): `element_boxes` takes a fixed box's origin from
  `LayoutTree::fixed_at` per pinned axis and marks its subtree (`Flushed.pinned`). ONE `client_rect` subtracts the
  scroll only on unpinned axes, and both `client_rects` and `hit_test` read it. `hit_test` ranks a hit in a
  top-layer element above every document layer (later entries above earlier), through the same `top_layer()` +
  `render::top_layer_box` path. `document_extent` leaves pinned boxes out (css-overflow-3 §2.2). The root element's
  clientWidth/Height are the viewport's, and its scrollWidth/Height = max(extent, viewport).
- **CSS 2.2 §10.1.3, a fixed box's % sizes** (layout_core `tree.rs fixed_viewport`): its % `width` / `height` /
  `min-*` / `max-*` (and `fit-content(%)`) resolve against the viewport in the same rewrite-then-restore step as its
  insets (one `FixedSaved` record per box). That restore now runs first among the pass restores, so another pass's
  own restore keeps the last word on its field. KAT: a second layout leaves every style identical.
- **CSS Sizing 3 §2.1 / §3.1 / §3.2** (layout_core `css.rs parse_dimension_str`): `min-content`, `max-content` and
  `fit-content(<length-percentage>)` map onto taffy 0.15.1's own keywords (no sizing code); `has_width` is false for
  any sizing keyword, and `block_auto` reads every sizing keyword but `stretch` as auto. On `min-*` / `max-*` they fold
  to auto (taffy's min/max sizes cannot hold a keyword).
- **HTML §15.3.x UA rules** (`css/mod.rs UA_SHEET`): the spec's non-modal `dialog { position: absolute; inset-inline:
  0; width / height: fit-content; margin: auto }` (written as `left: 0; right: 0`: layout_core has no logical insets).
  `dialog:modal`'s `max-width` / `max-height` are `calc(100vw - 6px - 2em)` / `calc(100vh - 6px - 2em)`, the spec's
  `100%`, which is right ONLY because a top-layer box's containing block is the viewport.

### Counts (host, measured in the same worktree; before = 2879aed46)
| runner | before | after |
|---|---|---|
| `cargo test -p layout_core` | 34 passed / 0 failed | 36 passed / 0 failed (+`fixed_percent_size_resolves_against_the_viewport`, +`sizing_keywords_parse_onto_taffy`) |
| `cargo test --release -p aether --lib` | 211 passed / 1 failed | 214 passed / 1 failed (+`test_focus_refuses_an_inert_element`, +`test_mouseup_defaults_take_the_retargeted_hit`, +`test_cssom_fixed_rects_hit_the_top_layer`; the failure is the foreign `js::idb::tests::transaction_ordering_kat` both times) |
| `wpt_apis` the-dialog-element (40 served) | 60/170 | 69/170 (`abspos-dialog-layout` 3/10 -> 10/10, `modal-dialog-scroll-height` 0/1 -> 1/1, `multiple-centered-dialogs` 0/1 -> 1/1) |
| `wpt_apis` popovers (28 served) | 98/136 | 98/136 (no page changed) |
| `wpt_dom` interactive-elements (86 served) | 166/352 | 168/352 (`modal-dialog-scroll-height` 0/1 -> 1/1, `remove-dialog-should-unblock-document` 0/1 -> 1/1) |

No page went down (per-page diff of both runners after the `toplayer.rs` fix).

### Why the remaining named pages did not move
- `centering.html` 0/23: nested iframes' viewports (owed).
- `top-layer-position.html` 0/1: a top-layer element's computed `position` (static / relative becoming absolute) is
  cascade priority on a UA rule, not layout (owed).
- `non-modal-dialog-layout.html` 0/1: one subtest that switches an open dialog between `absolute`, `static` and
  `relative` against a relative container by script; its first failing assert was not chased in this arc.
- Measured while building and owed (handlers.md §5 (c)): layout_core's IFC-root measure lays a content-sized box at
  its max-content capped at the viewport, whatever the definite available width, so `fit-content(<lp>)` (and the
  bare `fit-content`) do not clamp a text wider than the limit, and `max-content` stops at the viewport.


MEASURE: PASS — the top-layer fit is built on the ONE existing top-layer list (toplayer.rs track() right after each removal, before the focus restore, per the seat's ruling); the dialog's absolute positioning and the IFC-root measure clamp capped at the viewport are the measurement's corrections; the two dropped pages returned to baseline with no page below it; both grants (js/html.rs, js/toplayer.rs) named.
