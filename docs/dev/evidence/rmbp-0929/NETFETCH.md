# NETFETCH — shell `fetch <http-url> [<dest-path>]`

**Finding.** The shell had `curl` (hand-rolled e1000 `fetch`, IP literal, prints only) but no way to land a
download on disk; the smoltcp TCP client (`smolnet::stack_open_tcp/connect/send/recv`, smolnet.rs:995-1112) and
resolver (`smolnet::resolve`, smolnet.rs:1830) were reachable only from ring 3 syscalls.

**Mechanism.** `shell_fetch` (shell.rs tail) = parse (`net_fetch::parse_url`) -> IP literal or `smolnet::resolve` ->
`stack_open_tcp(usize::MAX)` -> `stack_connect` re-driven -> `stack_send(build_request)` -> `stack_recv` 1400 B chunks;
head split by `net_fetch::parse_response_head`; on 200 the body streams through `vfs_mount_table()` `mt.create` +
`mt.write` at a running offset (default `/home/<user>/Desktop/<basename>`). 4 MB cap, 10 s wall (`clock::logts_now`),
progress per 64 KB. Arm folded onto the `"curl"` line (shell.rs, cfg smolnet+x86_64); `fetch` added to `HOST_VERBS`.

**Milestones.** M1 pure half + parse gate + verb + file write. M2 `fetch - <url>` prints the body to the console.

**Witness.** `:: NETFETCH: url=<u> status=<c> bytes=<n> saved=<0|1> -> PASS|FAIL ::` (live only);
`:: NETFETCH-PARSE: url=.. basename=.. request=.. head=.. -> PASS ::` (every boot, main.rs beside `dns_x86_gate`).
**Spec pins.** x86-default.spec (REQUIRE parse PASS; FORBID both FAILs). No knob.

## Written
M1+M2 in one commit: net_fetch.rs (new), lib.rs tail mod, main.rs gate call (same line as dns_x86_gate), shell.rs
(arm + `shell_fetch`/`fetch_body` at tail), midden_core HOST_VERBS, spec. Not compiled (R76). No fixture server: live
lane unexercised on QEMU. 301/redirects and chunked encoding unsupported (HTTP/1.0).
