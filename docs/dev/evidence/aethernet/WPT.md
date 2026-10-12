# AETHERNET (SR70) — WPT and local-server evidence (executor run, before R107 stopped testing)

Runner: `handlers/aether/tests/wpt_dom.rs` (`wpt_websockets`, `wpt_eventsource`, `wpt_xhr`) over the
wptserve stand-in `handlers/aether/tests/wpt_net/mod.rs`, WPT pinned at da1f6d20. Release build.
Per-file rows: `wpt-websockets.tsv`, `wpt-eventsource.tsv` (this directory; the skip column names each
test the stand-in cannot serve and why).

| dir | served pass/total | % | note |
|---|---|---|---|
| websockets (`?default` + .html) | 484/510 | 94.9 | run 1, before the fixes below; 145 wss/h2 variants + 36 pages skipped by name |
| eventsource | 62/64 | 96.9 | 22 pages skipped by name (cors.py, cors-cookie.py, workers, frames) |
| xhr | not measured | — | run interrupted by R107 (stop all test runs); owed to the fold |

Fixes made after websockets run 1 (unmeasured, owed to the fold): option bag + targetAddressSpace,
readyState CLOSED when `error` fires on failure, EventHandler objects never get `handleEvent`, per-host
CONNECTING serialization, handshake Set-Cookie applied with the `open` task, FileReader, the stand-in's
echo_close_data / `trickle(dN)` pipe / html `<meta name=variant>`; eventsource: the last-event-ID
*string* vs *buffer* split (an undispatched `id:` does not survive a reconnect).

Known misses not in this arc's files: `document.createEvent('UIEvents').initUIEvent` (js/events.rs),
`window.stop()` aborting an EventSource (js/window.rs), keeping-connection-open (20 s wait > harness 10 s).

Local tokio server (`handlers/aether/tests/net.rs`): 7/7 passed, 5 consecutive runs (ws echo +
fragmented message with a ping between fragments + ping during a 1 MiB send; ws close-code matrix incl.
1005/1006/handshake failure; EventSource drop + Last-Event-ID reconnect with retry:100; chunked fetch —
first read() resolved with ≤3 of 4 chunks sent; fetch mixins/abort; XHR states/headers/sync/abort/
timeout/error; storage event across two engines on two threads, a third origin hears nothing).
gneiss_pal: RFC 6455 §5.7 KATs byte-for-byte, SHA-1/accept KAT, close/violation matrix, handshake checks,
socket e2e; sse EventStreamDecoder HTML semantics. `cargo test -p aether -p gneiss_pal --release --lib`:
aether 144 passed, gneiss_pal 88 passed (before the last sse/ws edits; owed to the fold).
