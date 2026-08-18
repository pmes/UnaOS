# PLAN — Kepler pull 3 (for the Gemini session): metal bug fixes + EVO flip

**Context:** pull 2 passed review and its first metal sitting (rMBP GT 650M,
fox-metal-r23s1f, 2026-07-21) ran 4/4 boots clean: probe completed all phases on real
silicon, EVO decode aborted read-only as designed, and the FIFO run executed the full
channel setup (instance block, GPFIFO, USERD, runlist, doorbell) before an honest
fence-timeout. Two silicon bugs came back; they gate everything else. Serial evidence:
`~/unaos-bench/capture/rmbp-r23s1f/` (boot marks K1..K4).

**Standing gates unchanged:** per-phase commits on `UnaOS-gemini`; `./arroyo check` +
`UNAOS_KEPLER=1 ./arroyo check` green both arches; `./arroyo test` zero FAIL/panic and
byte-quiet without knobs; flag out-of-lane touches; cite envytools facts per register.

## Phase 1 — the two silicon bugs (blockers; Fox's read: same MMIO/decode family, and
a bad base could equally misplace USERD/the semaphore — fix these, the fence may follow)

1. **BUG #1 — VRAM size decode wrong on hardware.** PFB reported 2989 MB on a card
   that is 512M/1G. QEMU never exercised this read. Re-derive the register/field from
   envytools PFB facts for GK107 (`NV_PFB_RAM_AMOUNT` semantics differ across
   generations — verify the offset AND the unit/shift for GKxxx; document the fact
   source). The 16MB..32GB sanity window let 2989 MB through — after the fix, tighten
   the check to also require power-of-two-ish sizes (n or 3n/4 for asymmetric configs).
2. **BUG #2 — GOP framebuffer base read from the wrong source.** The driver logged
   "video::WRITER has no base address" and aborted `no-gop` while the framebuffer was
   demonstrably live in the same boot (fb-wc retype over 0x90020000 + splash drew
   BEFORE kepler init). Find where the live boot framebuffer base actually lives
   (the fb-wc path knows it — follow what `set_framebuffer_wc` was called with) and
   read THAT, not the stale/never-set WRITER field you used. If this needs a getter
   outside `drivers/gpu/**`, it is an out-of-lane touch: flag it, keep it read-only.

## Phase 2 — witness + plumbing corrections (from review + sitting)
3. `:: kepler: no-device ::` is emitted on probe-abort paths too. Split:
   `:: kepler: no-device ::` only when the PCI scan finds no Kepler class;
   `:: kepler: probe-abort <reason> ::` for BAR/size/mapping failures. Update the
   QEMU gate expectations accordingly.
4. Wire `UNAOS_KEPLER_TAKEOVER` / `UNAOS_KEPLER_FIFO` through arroyo's knob plumbing
   (same pattern as Fox's UNAOS_KEPLER builder mapping at c84688f1) instead of bare
   `option_env!`, so staged binaries are string-verifiable per knob. Keep the
   option_env! reads if you like, but arroyo must know the knobs exist.

## Phase 3 — EVO flip (the descoped half of K-GPU-2)
5. With BUG #2 fixed, `takeover-abort no-gop` should become a real head-state
   correlation. Then implement the minimal EVO core-channel push: allocate its
   pushbuffer, submit surface-address methods + UPDATE, verify via readback that the
   head latched (envytools disp method IDs; cite each). Still behind
   `UNAOS_KEPLER_TAKEOVER=1`; still: never reprogram on a guess — the bounds/fallback
   discipline from pull 2 stands.
6. Copy GOP contents to the new surface before the flip (double-buffer rule) so a
   successful flip is visually a no-op; kernel text keeps flowing via the new base.

## Phase 4 — fence re-run readiness
7. Re-audit USERD/semaphore VRAM offsets against the fixed base/size understanding
   from Phase 1 (Fox's hypothesis). Add one diagnostic witness before the poll:
   `:: kepler: fifo-layout userd=<off> fence=<off> gp=<put>/<get> ::`, and after
   timeout read back GP_GET + channel status raws into the abort witness so the next
   sitting can localize where submission stalled.

**Oracle:** QEMU = quiet baseline + clean `probe-abort`/`no-device` split + MISSION
gates green. Metal (next rMBP sitting, not yours): correct VRAM size printed, GOP base
correlated, flip visually seamless, and the k3-fifo re-run — fence or a localizing
abort witness. End-of-pull REPORT: per-phase sections, out-of-lane list, updated
"what metal must verify" checklist.
