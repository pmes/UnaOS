# HTTPCORE (LEDGER SR51): UnaOS's own HTTP client

Branch `exec-net-httpcore`, cut at d8788fe1, `exec-rmbp-merge13` merged first (1ae2eb7a). Commits: M1 c44dfc91,
M2 df8c8833, M3 + M4 + this doc after them (see `git log`).

## Finding

Every host HTTP(S) call went through `reqwest` 0.13 (hyper, rustls/native roots, `cookie_store`, `url`), Gneiss's
GitHub client through `octocrab` 0.54 (reqwest/hyper again), and Aether carried `tokio-tungstenite` 0.30 with no
live caller (`api/websockets.rs` is a five-line stub). Meanwhile TLSCORE + CRYPTOCORE already verified every
certificate on the metal (VEINTLS). Now the host path is UnaOS code end to end: `http_core` (URL, HTTP/1.1, h2,
cookies, codings) over its host transport (std `TcpStream` + `tls_core` TLS 1.3 + CRYPTOCORE), behind
`gneiss_pal::api::http` with reqwest's call shape. `cargo tree -e normal` on gneiss_pal, vein and aether shows
none of reqwest, octocrab, tokio-tungstenite (nor hyper, rustls, native-tls, cookie_store, h2, tungstenite).

## What was built

| where | what |
|---|---|
| `unaos/libs/net/http_core` (new, `no_std` + alloc, `forbid(unsafe_code)`, zero third-party deps) | `url` — the WHATWG URL Standard: the basic URL parser state machine (§4.4, every state), host parser with IPv4 (hex/octal/short forms) and IPv6 (compression, embedded IPv4) and opaque hosts (§3.5), "domain to ASCII" with the ASCII fast path exact and a partial UTS #46 mapping + RFC 3492 Punycode for non-ASCII labels, all percent-encode sets (§1.3), serializers, getters, origins (§4.7), `join`. `headers` — RFC 9110 §5 ordered case-insensitive fields, token / value grammar enforced (no CR/LF/NUL injection). `h1` — RFC 9112: request head (Host first, Content-Length), status line + fields (bare-LF accepted §2.2, whitespace-before-colon refused §5.1, obs-fold replaced with SP as §5.2 requires of a user agent), §6.3 body length (HEAD/1xx/204/304/CONNECT, TE beats CL, differing CL lists refused, non-chunked TE refused), §7.1 chunked with extensions and trailers (incremental, split anywhere, size overflow refused), §9.3 persistence. `conn` — one exchange over any `Transport` (1xx skipped, leftover bytes kept for keep-alive, truncation is an error), a streaming `BodySource`. `redirect` — RFC 9110 §15.4 + Fetch method rules (301/302 POST→GET, 303→GET but HEAD, 307/308 keep), Location fragment inheritance, cross-origin credential stripping (Authorization, Proxy-Authorization, Cookie, x-api-key, x-goog-api-key), http/https only, 20 hops. `cookie` — RFC 6265 §5: Set-Cookie parser, §5.1.1 date parser, domain/path match, default-path, the §5.3 storage model (host-only, Max-Age over Expires, HttpOnly vs non-HTTP APIs, replacement keeps creation time), §5.4 Cookie header ordering; SameSite parsed and kept. `encoding` — gzip / deflate (zlib, bare-DEFLATE fallback) through pixel_core's inflater, streamed; br refused by name and never advertised. `multipart` — RFC 7578. `h2` (feature) — RFC 9113 framing with every §6 rule, RFC 7541 HPACK (static + dynamic table, Huffman, size updates, never-indexed credentials), a client connection (preface/SETTINGS, HEADERS+CONTINUATION, both flow-control windows, WINDOW_UPDATE credit, PING, GOAWAY, RST_STREAM, push refused, §8.3 response checks). `host` (feature, std) — below. |
| `unaos/libs/media/pixel_core` | `inflate::inflate_raw` — a third sibling over the ONE `deflate_body` (for servers that send bare DEFLATE as `deflate`). |
| `http_core::host` | Connection = thread (tls_core's provider is not `Sync`, so a TLS session cannot change threads): connect (`resolve` overrides like curl's `--resolve`), CONNECT tunnel through `HTTPS_PROXY`/`HTTP_PROXY` minus `NO_PROXY` (names, suffixes, `*.`, IPv4 CIDR), `tls_core` handshake with `WebPkiVerifier` at the system clock, ALPN `http/1.1` (or `h2` when offered and the `h2` feature is on), then serve exchanges from a job channel and park in the `Agent`'s pool (keep-alive) until idle; a parked connection the server closed is retried once on a fresh one. Events (head, chunks, end) go to an `EventSink` (std channel for the blocking `Agent::send`, tokio channel for gneiss_pal). gzip/deflate decoded on that thread as the body arrives. Cookies from an optional shared jar. Trust: config, else `$SSL_CERT_FILE`, else `system/trust/roots.pem`, else the distribution bundle; parsed once per process. |
| `gneiss_pal::api::http` (new) | reqwest's shape over `http_core::host`: `Client`/`ClientBuilder` (timeout, user_agent, cookie_jar, max_redirects, trust_pem_file, no_proxy), `RequestBuilder` (header, bearer_auth, body, json, multipart, try_clone, `send().await`), `Response` (status → `StatusCode` with reason phrase, headers, url, content_length, chunk/bytes/text/json, `bytes_stream()`), `multipart::{Form, Part}`, `blocking::{Client, ClientBuilder, RequestBuilder, Response}`, re-exports `CookieJar`, `Url`, `Headers`. Futures are `Send` and executor-agnostic. |
| gneiss_pal call sites | `retry.rs` (send_with_backoff/send_classified, retry-after), `sse.rs` (`sse_stream` over `bytes_stream`), `claude.rs`, `gemini.rs` (bearer for ADC): only the import lines changed. `forge.rs`: octocrab replaced by the three REST calls Gneiss uses (`GET /user`, `GET /user/repos`, `GET /repos/{o}/{r}/contents/{path}?ref=`) with GitHub's headers; the file content is now actually base64-decoded (RFC 4648; the octocrab version returned the raw base64 because the decoder had been removed). |
| vein | `synapse.rs` (`SynapticRetry` over `api::http`), the S9 upload (`api::http::multipart`). `reqwest` removed from Cargo.toml. |
| aether | `net/mod.rs`: the shared jar is `http_core::cookie::CookieJar` (document.cookie uses it as the non-HTTP API, so HttpOnly cookies are now invisible to scripts and cannot be set by them — RFC 6265 §5.3 step 10 / §5.4; reqwest's jar did not make that distinction), one pooled `Client`, `blocking_client_builder` for the JS fetch/XHR thread. `reqwest` and `tokio-tungstenite` removed. |

## Oracles and KATs

All tests run on the PRODUCT crypto provider (CRYPTOCORE through tls_core `cryptocore-std`).

| proof | result |
|---|---|
| WPT `url/resources/urltestdata.json` (wpt 564b9b1e) | **896/896** (floor-gated) |
| WPT `IdnaTestV2.json` through the URL parser (the IDNA ceiling, measured) | **1353/2676** (floor-gated; the rest needs the UTS #46 table, NFC, Bidi/ContextJ) |
| http-state (abarth/http-state 155e45c6, the RFC 6265 UA suite, its testserver's semantics) | **214/214**, optional 4/4 (disabled-* skipped as the suite does) |
| RFC 9112/9110 KATs (`tests/h1_kats.rs`, 9 tests): status line/fields at every prefix, refusals (5 status lines, 6 field lines, oversize head), §6.3 table (13 cases), chunked split at every byte with extensions + trailer, 5 malformed chunk streams, request head golden + injection refusals, scripted-transport exchange at 5 read sizes (1xx, keep-alive second response, truncation, close-delimited), redirect rules (10 cases + header stripping), gzip/zlib/raw-deflate against Python's output + CRC tamper + br refusal, multipart golden | 9/9 |
| RFC 3492 Punycode samples, RFC 6265 date/path units, RFC 4648 base64 | green |
| Python `http.server` + `ssl` (OpenSSL 3, TLS 1.3 only, per-run CA → intermediate → P-256 leaf) | verified handshake, ALPN `http/1.1`, TLSv1.3; Content-Length, chunked (ext + trailer, odd sizes), gzip and deflate streamed and decoded, 302 + HttpOnly Set-Cookie followed with the cookie returned, 307 keeping a POST body, 1 MiB PUT, HEAD, a paced SSE answer whose first chunk arrived at ~43 ms of a ~1.25 s stream, **12 requests over 1 TCP connection** (keep-alive; a connection is parked before its End event); refusals: foreign root → `cert-unknown-issuer`, wrong name (`wrong.test` pinned to 127.0.0.1) → `cert-name-mismatch`, the right name pinned → 200; plain HTTP suite identical (1 connection) |
| real `api.anthropic.com`, no key | Mozilla bundle (certifi 2026.7.22, 121 roots, sha256 9cc2a774…): **REFUSED `cert-unknown-issuer`** (this container's egress gateway re-terminates TLS — the verifier is right); egress CAs `/root/.ccr/ca-bundle.crt` (reported separately, never in place of Mozilla): **VERIFIED, 401** `"x-api-key header is required"` |
| CONNECT through the session proxy (`HTTPS_PROXY`) | 200, the 229 610-byte WPT file byte-exact |
| **Chromium oracle** (Playwright's Chromium navigates; `response.body()` vs http_core's body, local server; Chromium sent `gzip, deflate, br, zstd`, the server answered gzip) | **5/5 byte-equal**: `/page` 12 813 B (gzip+chunked HTML), `/gzip` 112 000 B, `/deflate` 112 000 B, `/chunked` 112 000 B, `/hello` 18 B |
| http2jp/hpack-test-case (8a1406e7; nghttp2, go-hpack, nghttp2-change-table-size, swift-nio-hpack-plain-text, python-hpack: 159 stories) | **16 803/16 803 header blocks decoded exactly**; our encoder round-trips all 16 803 at 27.8 % of the raw field bytes |
| RFC 7541 §C.1, §C.4, §C.6 (integers, Huffman requests, Huffman responses with eviction at 256 B) + Huffman table completeness (Kraft = 1, 256 internal nodes) + padding/EOS refusals | green |
| RFC 9113 framing KATs | exact octets for HEADERS/SETTINGS/DATA(padded)/HEADERS(priority)/PING/GOAWAY/WINDOW_UPDATE/unknown; 16 §6 refusals with the right error code and scope |
| h2 interop: Node's `http2` (nghttp2, OpenSSL, TLS 1.3, ALPN h2 only, 64 KiB initial windows) | GET, 1 MiB body through our WINDOW_UPDATEs, gzip decoded, a 300 KB POST waiting on the server's windows (sha256 equal), 302 followed, trailers; **8 streams on 1 session** |
| gneiss_pal (82 tests: provider KATs against the mock server incl. Claude SSE, Gemini, retry/backoff, claudecode fake CLI, forge mock) | 82/82 |

Vectors too large to commit are fetched at test time (curl, pinned commit + sha256 in
`unaos/libs/net/http_core/vectors.txt`; per-file suites pinned by an aggregate sha256 over `tests/*.list`); a test
whose vectors cannot be fetched SKIPS and says so.

## Honest ceiling

* **TLS 1.3 only** (TLSCORE's ceiling): a TLS-1.2-only server is refused. Aether loses such sites until TLSCORE
  grows 1.2; Anthropic, Google APIs and GitHub all speak 1.3. No session resumption, no OCSP/CT.
* **IDNA**: 1353/2676 of IdnaTestV2 — the UTS #46 mapping table, NFC and the Bidi/ContextJ rules are not in.
* **Cookies**: no public suffix list (only single-label suffixes like `Domain=org` are refused); SameSite recorded,
  not enforced (the caller owns site-for-cookies); jar is in memory (persistence is the caller's, every field public).
* **Content codings**: br and zstd are not decoded (never advertised); stacked codings stream only one layer
  (the whole-body `decode` handles stacks); transfer codings other than chunked are refused.
* **h2**: behind the `h2` feature and opt-in via `AgentConfig::alpn` — gneiss_pal does not enable it yet. One stream
  at a time per connection (the pool gives concurrency across connections); no PRIORITY scheduling; the encoder caps its
  dynamic table at 4096 octets.
* **WebSocket (RFC 6455)**: no live caller existed, so tokio-tungstenite was deleted and no client was written.
* Timeouts are per socket read/write (idle), not a whole-exchange deadline.
* `url` (the crate) stays in Aether for its own parsing (the row says "later"); `tools/foreman` still uses reqwest
  (outside gneiss_pal/vein/aether).
* Proxies: HTTP CONNECT only (no https:// or SOCKS proxies, no proxy auth).

## Owed

1. Flip gneiss_pal to offer `h2` (one-line `alpn` + feature) once multiplexed streams are scheduled concurrently.
2. UTS #46 data tables (generated from IdnaMappingTable.txt, as HTMLCORE generated its entity table) + NFC.
3. Public suffix list as generated data; SameSite enforcement in Aether's navigation.
4. Brotli (RFC 7932) as its own `no_std` decoder; then advertise `br`.
5. foreman → `gneiss_pal::api::http`; Aether's remaining `url` crate uses → `http_core::url`.
6. RFC 6455 client when a caller appears.
7. TLS 1.2 in TLSCORE (for Aether's long tail).

## How to continue

`cargo test --release -p http_core --features host,h2` runs everything in the crate (needs python3, openssl, node +
the pre-installed Playwright for the oracles; each skips cleanly without them). The host suite for a new site
problem: add a route to `tests/oracle/http_server.py` and a leg to `tests/host_e2e.rs`; for h2,
`tests/oracle/h2_server.js` + `tests/h2_e2e.rs`. `system/trust/roots.pem` (gitignored) is staged by
`tools/trust-bundle` for the Mozilla leg.

## Third-party crates this arc leans on

None in http_core (dependencies: pixel_core; with `host`: tls_core + crypto_core — all UnaOS). Dev-dependencies:
none (curl/sha256sum/python3/openssl/node are test-time tools). gneiss_pal's api::http uses tokio (`sync`, utility:
the channel) and futures-core/futures-util (utility: the Stream trait) — both already in the tree. Removed: reqwest
0.13 (chicken wire: HTTP+TLS), octocrab 0.54 (chicken wire: GitHub client over reqwest/hyper), tokio-tungstenite
0.30 (chicken wire, dead). No crate was added.
