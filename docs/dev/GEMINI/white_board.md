# WHITE BOARD — 2026-08-04

**Peter's sheet.** What I need from you, right now. Nothing else goes here —
cross-session handoff lives in the baton, per-boot status in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN

## 1. One push

```bash
git push origin UnaOS-gemini
```

`origin/UnaOS-gemini` was verified at `5a740f0d` with a fetch in the same call.
Outstanding above it: the docs commit only. Everything else you have already
pushed.

## 2. Still no metal verdict — and still nothing to act on

No removable media, and **no serial device at all** — `/dev/ttyUSB*` and
`/dev/ttyACM*` do not exist. squawk closed cleanly at 20:27Z on 08-03
(`SQUAWK MARK … session-end`), not a crash. No watchers are armed, because there
is nothing for them to watch.

This is reported state, not a request. The consequence is that FBCON-DMG's
evidence class stays `unproven`: the instrument that can answer it now exists and
is proven present in the image, but has never been booted.

---

# WHAT MOVED TODAY

- **`5a740f0d` DMG-REFUSE — executed, not merely compiled.** Syscall 33's three
  refusal arms had nothing that could reach them; the `-EINVAL` arm is the whole
  justification for the verb and had never run. 19 probes from two ring-3 slots,
  green in QEMU, and proven able to fail by flipping one expectation.
- **`ae639f5a` ARTIFACT-AUDIT.** The real `esp-x86` image was built and probed.
  Everything from both GR15 code commits is present, including the syscall-33
  client path decoded instruction-for-instruction in `VUG.ELF`.

# TWO TRAPS NOW WRITTEN DOWN — both would have inverted a reading

**`strings`, `readelf`, `objdump` and `nm` are not on this host.** A naive probe
reads zero hits for everything, which is indistinguishable from the code being
missing. `busybox strings` is what works.

**The `[wc-h]` literals do not tell you the compositor is armed.** `span=`,
`band=`, `window-band` and the rest are byte-identical in an image built with the
compositor knobs OFF — they ride on `witness` alone. The discriminators are the
symbols `wcx::activate` and `takeover_display`. And `[wc-x]`/`kepler` fall to
**1**, not 0, with the knobs off — the survivor is `[wc-x] backbuffer resync` — so
testing `> 0` gives a false positive on an unarmed image.
