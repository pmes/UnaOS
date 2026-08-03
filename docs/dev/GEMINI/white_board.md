# WHITE BOARD — 2026-08-03

**Peter's sheet.** What I need from you, right now. Nothing else goes here —
cross-session handoff lives in the baton, per-boot status in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN

## 1. One push, at arc close — not yet

```
git push origin UnaOS-gemini
```

Nothing is owed before it. `origin/UnaOS-gemini` was verified at `0965544b` this session with a
`git fetch` in the same call; the tree was clean and nothing was unpushed. This is the only push
GR15 will need unless the arc splits onto a second branch, which will be named here the moment it
is decided rather than when it is reached.

## 2. A metal boot will be needed for a verdict — media not yet built

GR15 is not asking for one yet. When the media exists this line will say so, name the commit it
carries and the knobs it was built with, and report what was verified by reading back off the card.
Until then there is nothing to act on.

---

# RECORDED THIS SESSION — no action needed, but the record was wrong

Two things were written into the x86 docs as established fact and are not.

**The window-id mapping was inverted.** `win=1` is the panel console (box 1314x750), `win=2` is the
WC-X demo (96x64 at scale 8), `win=3` is the MOVE-VACATE probe (8x8, witness builds only). The
docs had `win=3` as the banded console. Verified off the s69 wire, and the box arithmetic
corroborates independently.

**FBCON-DMG is `unproven`, not metal-proven.** The console's four `[wc-h]` samples are all
whole-box — `bytes=3942000` is exactly 1314 × 750 × 4 — which is the conviction case named in the
s69 playbook's own watch list. But it is not evidence the feature is broken: `SAMPLES = 4` in
`video/wcg.rs:145` hard-caps and never reopens, so the budget was spent on window-creation and
first-paint presents. In that boot the samples land at capture lines 4752–4763 and the log runs to
5744 — roughly 980 lines of console output after the rollup closed, none of it observable. The
instrument stopped before its subject ran.

U4y is unaffected. It really was confirmed on metal.

Also worth keeping: that capture file holds **four** boots, not one — `console-route first-paint`
announces at lines 468, 1779, 3289 and 4759.
