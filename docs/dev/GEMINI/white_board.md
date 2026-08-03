# WHITE BOARD — 2026-08-03

**Peter's sheet.** What I need from you, right now. Nothing else goes here —
cross-session handoff lives in the baton, per-boot status in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN

## 1. One push — now due

```
git push origin UnaOS-gemini
```

Four commits, `0a8f5048` → `e67219d0`, on top of `0965544b`. That is the whole
arc and the only push it needs. Nothing else is owed.

## 2. No metal verdict yet — and no media to give one

The arc built the instrument that can finally answer whether FBCON-DMG works. It
has not been booted, because **no removable media is present in the reader** —
verified this session, not assumed. Nothing here is a request; it is the reason
the evidence class in the docs still reads `unproven` rather than resolved.

When a card is available the discriminator is one line: a `[wc-h] rollup win=1`
carrying `scope=window-band`. Two rollups for `win=1` means the banding reaches
the panel. One rollup only is a real negative. `banded=0` on the `scope=window`
line is neither — that rollup fires at window creation, before any banded present
can exist.

---

# BENCH STATE — verified as processes, not as config

- squawk alive on `/dev/ttyUSB0`, session `rmbp-s66-cand444`, ~4h40m uptime.
- Both wakers armed **as running processes**. Mine went down mid-session — a
  waker wakes you by exiting, so its own firing disarms the bench — and was
  re-armed on a fresh anchor.
- The waker pattern was changed from `AT-RISK|torn=yes` to `rollup win=`. The old
  one could only ever go red: after a successful fix those strings vanish and it
  would never fire at all. The new one was controlled four ways — silent on the
  real log tail and on idle `[schedx86]` chatter, fires on `AT-RISK`, on
  `TEAR-FREE`, and on the crit leg.

---

# RECORDED — no action needed

The x86 record carried two errors and both are corrected in `e67219d0`: the
window-id mapping was inverted (`win=1` is the console, not `win=3`), and
FBCON-DMG was recorded as metal-proven when the instrument could not observe it
either way. U4y is unaffected — it really was confirmed on metal.

The gate itself was worse than the record. `./arroyo check` — the DONE gate named
in `CLAUDE.md` — passed with **exit 0** on a blatant type error in `video/wcg.rs`,
because that file is `witness`-gated and was never compiled. It also never
compiled any `user-*` crate. Both holes are closed in `944b853e`, and both were
proven to go red before being called fixed.
