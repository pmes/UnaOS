# NETRING3 — the rungs under a metal HTTPS client (rmbp-ledger B306)

Branch `exec-rmbp-netring3`, cut from b42b87cc. Knob `UNAOS_NETRING3=1` → feature `netring3`.

## Design

**Finding.** Ring 3 on the rMBP has TCP sockets (SYS_SOCKET 40 .. SYS_ACCEPT 48, x86 `smolnet`) but no
entropy syscall (ROADMAP §6 "Entropy — prerequisite for any TLS"), no way to resolve a name (the kernel
resolver `smolnet::resolve` serves only the in-kernel SNTP witness and `fetch`), and no TLS anywhere.
The kernel already has ONE entropy source, `src/rand.rs` (SECLOGIN M5: RDRAND / RNDR / jitter, 32
bytes per draw, `login`/`selfhost`-gated) — this arc extends it, it does not add a second generator.

**Seam: driver** (kernel syscalls over the existing rand and smolnet drivers) **+ a ring-3 program.**
No handler owns entropy or name resolution in CODEX §2 (Vein owns diagnosis of the link, Aether the
host browser); the kernel is the fulfiller of two primitive syscalls, and the HTTP/TLS client lives in
ring 3 where Vein will one day run. New kernel file `src/netring3.rs`: `//! CHARTER: Kernel — driver`.

**Syscalls taken (both arches minted in `una-abi`):**
- `SYS_GETRANDOM = 56` — `(buf, len) -> len / -errno`, at most `GETRANDOM_MAX` (256) bytes per call
  (a short count is a legal answer; ring 3 loops). x86 and aarch64 both dispatch it.
- `SYS_RESOLVE = 57` — `(name_ptr, name_len, out_ptr) -> 0 / -errno`, writes `RESOLVE_OUT_LEN` (20)
  bytes `[v4 4][v6 16]` (v6 zero in v1). x86 serves it through `smolnet::resolve` (DHCP-leased DNS,
  gateway fallback — the exact path `fetch` uses); aarch64 answers `-ENODEV` (no smolnet there).

**M1 ENTROPY.** `rand.rs` gains a SHA-256 hash-DRBG: key K (32 B) and a counter; output block =
SHA-256(K ‖ ctr ‖ "out"); after every request K = SHA-256(K ‖ "next") (backtracking resistance);
reseed at first use and every `RESEED_BYTES` (64 KiB): K = SHA-256(K ‖ hw32 ‖ jitter32 ‖ tsc) where
hw32 is `rdseed` if CPUID.07H:EBX[18] says so, else `rdrand` (the existing `fill`), and jitter32 is the
existing cycle-counter walk — so the TSC/jitter are always mixed in, even on an RDRAND machine. The gate
of `rand` (and of `hash`, which it builds on) widens to `netring3`. Wire: `[rand] drbg=sha256 seed=<s>
reseed=65536`. Fixture `tests net` (entropy half): 4 KiB drawn twice, no two 32-byte windows equal
across the 8 KiB, monobit count within 4096±256 per 8192 bits sanity bound per 4 KiB — witness
`:: ENTROPY: source=<s> bytes=8192 ok=1 -> PASS ::`.

**M2 DNS.** `SYS_RESOLVE` as above. Fixture (resolve half of `tests net`): with a link, resolve
`api.anthropic.com` through the syscall body and print `:: RESOLVE: name=api.anthropic.com ip=<ip> ->
PASS ::`; with no NIC or no answer, `-> SKIP reason=<no-link|no-answer>` — never FAIL on a dark NIC.
(The SINKHOLE-1 slirp leg is a QEMU fixture and R78 retires QEMU; the canned-parser leg is
`dns_x86_gate`, unchanged.)

**M3 NET.BIN** (`crates/user-net` → `APPS/NET.BIN`, x86 static ELF in the 16 KiB ring-3 window). No
argv exists for ring-3 programs yet, so v1 runs a fixed script: SYS_GETRANDOM(32) → SYS_RESOLVE
`api.anthropic.com` → SYS_SOCKET(TCP) → SYS_CONNECT :80 (poll with SYS_SLEEP_MS) → `GET / HTTP/1.0` →
print the status line → the TLS leg. The TLS leg is the blocking `Read`/`Write` adapter over
SYS_SEND/SYS_SOCK_RECV (EAGAIN → sleep and retry; 0 → end of stream) with the exact shape
`embedded-io` 0.7 asks for, and reports `tls=skip` with its reason (below).

**M4 the metal shape.** `tests net` runs M1 and M2 in the kernel. NET.BIN prints the metal pin:
`:: NETRING3: resolve=<ip|err> connect=<0|err> http=<status|err> tls=<handshake|skip|err> -> PASS|SKIP ::`
— SKIP (with `reason=`) whenever the resolve or connect fails for want of a link; FAIL only for a
broken answer on a live link.

**Witness lines a metal boot should print** (`UNAOS_NETRING3=1`, dongle up, `tests net` then
`bg /apps/NET.BIN`):
```
[rand] drbg=sha256 seed=rdrand reseed=65536
:: ENTROPY: source=rdrand bytes=8192 ok=1 -> PASS ::
:: RESOLVE: name=api.anthropic.com ip=<a.b.c.d> -> PASS ::
:: NETRING3: resolve=<a.b.c.d> connect=0 http=301 tls=skip reason=window -> PASS ::
```

**Stays owed.** The TLS handshake on the metal (blocked on the ring-3 window, below); argv for ring-3
programs; v6 in the resolve slot; aarch64 SYS_RESOLVE (needs the NET6 resolver bound to the same
syscall); certificate verification (the trust store, below).

## TLS crate decision — evidence

`embedded-tls` 0.19.0, `default-features = false` (drops `std`, `log`, `tokio`), plus `rand_core` 0.6
and `embedded-io` 0.7, **builds** for `unaos/x86_64-unknown-none.json` (the target the x86 user crates
use: soft-float, no SSE) with `-Z build-std=core,compiler_builtins`: no `alloc`, atomics through
`portable-atomic`, no feature trouble. A spike that drives `TlsConnection::open` with
`UnsecureProvider::<Aes128GcmSha256>` (verify=NONE) over a socket adapter links, at `opt-level="s"`,
`lto`, `panic=abort`, to: `.text` 78025 B, `.rodata` 1850 B, `.bss` 20736 B (the 16640 B read record
buffer + a 4096 B write buffer); stripped ELF 80824 B. **The x86 ring-3 program window is 16 KiB
in total** — `memory.rs` `U3_WINDOW_PAGES = 4` (code, data, two stack pages), and arroyo's build recipes
assert `<= 16384` B. The handshake needs ~100 KiB of code+buffers before its stack. So the crate is the
right crate and cannot run in today's window: the deliverable is the adapter (in NET.BIN) and the spike
(`crates/user-net/tls-spike/`, NOT in the check matrix — it pulls crates.io deps; build it with the
command in its Cargo.toml). The rung that unblocks the handshake is a larger ring-3 window (a heap or
an N-page program window — a `memory.rs` STOP tripwire, so its own arc); after it, NET.BIN links
embedded-tls and the `tls=` cell becomes `handshake`.

## Trust store (the next rung after the handshake)

v1 prints the server certificate's subject and `verify=NONE`. The trust store is a root bundle as a
FILE on the system volume — `/system/trust/roots.pem` (PEM, the Mozilla CA set, the same set the host
Lumen's `rustls` uses through `webpki-roots`) — loaded BY NAME through the ordinary SYS_OPEN/SYS_READ
path, never compiled into a binary, so a root rotation is a file update, not a rebuild. Its owner is
Holocron (secrets and identity: what is trusted) with Principia owning the policy knobs (whether a
user-added root is honoured, pinning for `api.anthropic.com`); the kernel only serves the bytes. On the
UnaFS volume each root carries attributes (`una:trust-subject`, `una:trust-sha256`, `una:trust-origin`)
so `query` lists them, and NET.BIN's verifier (`embedded-tls` `webpki`/`rustpki` feature, ECDSA-P256 and
RSA roots) matches the chain against the bundle and prints `verify=OK|FAIL <reason>`.
