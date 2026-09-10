# QEMUFAST AMENDMENT 01 (22:15Z; pi 9 retraction + Peter's no-hardcode ruling) — apply if read; else seat follow-on
The run stamp must be EXCLUDED FROM THE MATCHER BY CONSTRUCTION, not by wording. The serial log stays PURE GUEST
BYTES (the tree's own principle at test_kernel8: "the guest's bytes still land in exactly $logf and mbench still
replays exactly that file"). The harness writes its run metadata to a SIDECAR next to the log — `<logfile>.run`
(mode=fast|full, completion_at=+N.Ns or none, grace=Gs, wall=W.Ws, cap=Cs, spec=…, sha=…) — and mbench's verdict
line reads it and prints the mode (`MBENCH PASS … [fast: completion +11.0s grace 20s wall 31.4s]` / `[full wall 300s]`).
Distinguishability after the fact = the sidecar travels with the capture (capture conventions already carry
manifests). No trailer line inside the log; no spec-vocabulary check needed because no harness text can reach a
directive. Delete the "avoid the word COMPLETE" reasoning wherever it appears.
ADDENDUM (pi 9, 22:25Z): the sidecar read is THREE-valued — fast | full | UNKNOWN (absent, unreadable, or STALE) —
and "unknown" must be sayable in the verdict line (`[mode unknown: no run sidecar]`), never collapsed into either
mode (the `run_verdict` lesson: two-valued was the shape of the problem). Staleness is the sharp edge: the sidecar
carries the log's identity (size + sha256 of the log at the moment of writing, or the log's inode+mtime), and a
sidecar whose identity does not match the log it sits beside is reported as `stale` — confidently wrong beats
obviously absent, so the identity check is mandatory, not optional.
ORDERING (pi 9, 22:45Z): the sidecar's identity is computed over the log AS THE VERDICT WILL READ IT — i.e. AFTER
QEMU has exited and `wait`ed (no further bytes can land), immediately before mbench replays; the mode/grace/
completion values are decided earlier and carried in variables, but the FILE is written last. A sidecar written at
the exit decision would mismatch on every run (late flush, teardown) and turn `stale` into a systematic false
result — a check that fires every time gets deleted immediately. RED-first: append one byte to the log after the
sidecar is written → verdict reads `stale`; normal run → mode read correctly on every run (3-of-3 consecutive).
CONSUMERS (rmbp 16, 22:50Z): every reader of the sidecar treats `unknown`/`stale` as NOT-FULL and REFUSES to certify
"tail clean" — written into the reader (mbench's verdict line; scorer11's ARMING/mode leg), not a convention. A
reader that infers "not fast, therefore full" collapses the third value in the unsafe direction.
