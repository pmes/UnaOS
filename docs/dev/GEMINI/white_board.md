# WHITE BOARD — 2026-08-04

**Peter's sheet.** What I need from you, right now. Nothing else goes here —
cross-session handoff lives in the baton, per-boot status in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — nothing

No pushes owed. `origin/UnaOS-gemini` is at `d5a1bf0a`, same as local, verified with a fetch. Tree
clean. No decisions waiting on you.

---

# WHERE THE BOOT TIME ACTUALLY GOES

Last boot was **27.9 seconds**. Kepler is **17.1 of them** — 61 %, in one block:

```
kepler=17138ms   sdhc=117ms   nic=12ms   detect=5ms   igpu=1ms   xtail=0ms   resid=2ms
```

Everything that is not Kepler adds up to under a second. That is the whole remaining boot problem.

It cannot be worked on yet, because the log reports it as **one number with nothing inside it**. The
next arc's job is to break it down so a boot names which part is slow — and `drivers/gpu/kepler*.rs`
is Gemini's lane, so that has to be agreed before anything is written.

---

# WHAT LANDED TODAY — ten commits, `7091c23a` → `d5a1bf0a`

The one that mattered: **the panel had been running uncached since 2026-07-21.** An MMIO remap was
silently un-typing the framebuffer's page-table entries from write-combining back to uncached, and
nothing said so. Fixing it made the panel **8.7–9.1× faster** on large writes and took every window
from AT-RISK to tear-free. The boot's GUI phase dropped ~7.1 seconds.

Every "the compositor is tearing" conclusion from the last two weeks was reading that, not the
compositor.

Three instruments turned out to be hiding real defects rather than merely being noisy:

- **SERWIT-1** had been red in every boot since gr7 — it could not pass on this machine — and was
  concealing two genuine accounting bugs the whole time. It passes now, for the first time.
- **WC-H's tear counter** stopped counting after eight presents, so a window that tore a thousand
  times still reported "tear-free". The Pi track's gate rested on that number too.
- **WC-D** was reading its own log output back off the screen and reporting the difference as
  corruption.

Also: the console holds 4× more text, the compositor's own chatter no longer paints itself onto the
glass, and the first ~29 seconds of boot are no longer lost from the capture.
