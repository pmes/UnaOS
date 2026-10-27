#!/bin/bash
# typist.sh — CHARTER: build tooling (no handler domain): the ONE QMP typist the QEMU lanes source. Lifted verbatim from
# twoboot.sh by ZENBOOKLANETYPED (zenbook-ledger Z16); KEYBOOT4 (rmbp-ledger B864) made it phases. Sourced by
# scripts/twoboot.sh (boots 1, 3, 4) and arroyo's `test_zenbook` (the first-user form, then six typed `tests <verb>` lines).
# The caller sets TYPIST_PORT (the QMP port it put in UNAOS_QEMU_EXTRA) and TYPIST_LOG (the serial capture the markers are
# read from); `typist_start` leaves the typist's pid in $typist. Its transcript goes to <out log>, BESIDE the capture.
TYPIST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # qmp_type.py's directory (this file's own)

# --- the typist (KEYBOOT4 B864: one function over PHASES; boot 1's phase is the TWOBOOT typist's keys, unchanged) ------
# qmp_type.py's Qmp/connect/wait_for_marker/qcode, imported by path (the module's main() is guarded), plus the keys its
# --text cannot spell (Tab, Enter). Its connect waits out the build; each phase's marker wait is the boot's cap. A phase is
# `<marker>|<label>|<keys>`: keys are space-separated, `t:<text>` one key per character, anything else a QMP qcode.
# ZENBOOKLANETYPED (Z16): an optional 4th field `|<done>` — after the keys the typist waits (up to the cap) for <done>
# instead of the screen's key-taken line, and stops the chain if it never comes; three-field phases are unchanged.
typist_start() { # <out log> <cap> <phase>...
    local out="$1" cap="$2"; shift 2
    python3 - "$TYPIST_DIR" "${TYPIST_PORT:?typist_start: TYPIST_PORT unset}" "${TYPIST_LOG:?typist_start: TYPIST_LOG unset}" "$cap" "$@" > "$out" 2>&1 <<'PY' &
import sys, time
sys.path.insert(0, sys.argv[1])
import qmp_type as q
port, log, cap = int(sys.argv[2]), sys.argv[3], float(sys.argv[4])
phases = [(a.split("|", 3) + [""])[:4] for a in sys.argv[5:]]  # ZENBOOKLANETYPED (Z16): an optional 4th field, the done marker
qmp = q.Qmp(q.connect("127.0.0.1", port, 7200))
qmp._read_obj()
r = qmp.execute("qmp_capabilities")
if "error" in r:
    raise SystemExit(f"[typist] qmp_capabilities refused: {r['error']} — nothing typed")
t0 = time.time()
for n, (mark, label, spec, done) in enumerate(phases):
    lead = f"connected :{port}; " if n == 0 else ""
    print(f"[typist] {lead}waiting up to {cap:.0f}s for {mark!r}", flush=True)
    if not q.wait_for_marker(log, mark, cap):
        raise SystemExit(f"[typist] {mark!r} not seen in {cap:.0f}s — nothing typed")
    print(f"[typist] form open at +{time.time() - t0:.1f}s; settling 4s", flush=True)
    time.sleep(4)
    keys = []
    for tok in spec.split():
        keys += [q.qcode(c) for c in tok[2:]] if tok.startswith("t:") else [tok]
    for k in keys:
        r = qmp.execute("send-key", {"keys": [{"type": "qcode", "data": k}]})
        if "error" in r:
            raise SystemExit(f"[typist] send-key {k!r} refused: {r['error']}")
        time.sleep(0.12)
    print(f"[typist] typed {label} ({len(keys)} keys) at +{time.time() - t0:.1f}s", flush=True)
    if done:   # ZENBOOKLANETYPED (Z16): a typed verb's phase holds on the line its command causes, up to the cap
        ok = q.wait_for_marker(log, done, cap)
        print(f"[typist] {label}: {'saw' if ok else 'did NOT see'} {done!r} at +{time.time() - t0:.1f}s", flush=True)
        if not ok:
            raise SystemExit(f"[typist] {done!r} not seen in {cap:.0f}s after {label} — the chain stops here")
        continue
    ok = q.wait_for_marker(log, "[login] key taken by the screen", 10)
    print(f"[typist] the screen {'took' if ok else 'did NOT report taking'} a key", flush=True)
PY
    typist=$!
}
FIRSTUSER_PHASE="[login] installer: first-user form open|name + Tab + password + Tab + retype + Enter|t:una tab t:correct-horse tab t:correct-horse ret"
