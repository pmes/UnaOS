STATUS: BRIEF — awaiting Gemini intro proposal (new arc, new session)

# BRIEF — iGPU (Intel HD 4000) display driver, arc intro

Coordinator-authored brief (2026-07-22, post sitting #5). The assigned Gemini
specialist answers this with `PROPOSAL-igpu-pull1.md` (STATUS: PROPOSED) saved
in THIS directory (`docs/dev/GEMINI/video/iGUI/`), following the review flow
in `docs/dev/GEMINI/README.md` — **no implementation commits before
approval**.

## Why this arc exists (sitting #5 strategic redirect)

Five metal sittings established that the 2012 rMBP's internal panel is **not**
driven by the Kepler dGPU. Both Kepler scanout candidates were double-refuted
(`evo=0 crtc=0` on all 4 heads; the whole PDISPLAY engine reads idle). The
panel is owned by the **Intel HD 4000 iGPU through the gmux**, and the live GOP
framebuffer at `0x90020000` belongs to the iGPU. Kepler display takeover is
PARKED. Peter's ruling: *drive as much of the machine as possible* — the iGPU
is the shortest path to UnaOS-drawn pixels on the panel, because it already
owns it. No gmux flip is needed or wanted in this arc.

Full factual record: `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md`. Trust it;
do not re-derive its facts.

## Hardware facts (metal-confirmed, don't re-litigate)

- Machine: MacBookPro10,1 (mid-2012 Retina), Ivy Bridge, Intel HD 4000 (GT2)
  iGPU + GK107 Kepler dGPU, gmux-switched panel. Panel native 2880×1800; the
  GOP mode observed on metal is 1920-wide (`0x0780` seen in geometry reads).
- The iGPU and Kepler are independent PCI devices; neither needs the other.
- GOP framebuffer is live at `0x90020000` and survives our fb-wc retype
  (sitting #2, boot-2). It is iGPU-backed.
- The panel goes black almost instantly on every boot, including kbase builds
  with no Kepler code active, while serial stays healthy. Cause NOT established
  — per the null-hypothesis rule this may be our boot chain (fb handoff), not
  hardware. Your arc will likely be the one that explains it; treat it as an
  open question, not an assumption.

## What the intro proposal must contain

1. **Probe plan** — locate the iGPU on PCI (expected 00:02.0, VID 0x8086,
   Ivy Bridge GT2 device ID), enumerate BARs, and state which BAR is GTTMMADR
   (register MMIO + GTT) and which is the aperture, with sizes.
2. **Scanout derivation** — the register path from "GOP left the panel lit" to
   "we point the display at our own framebuffer": pipe/plane/transcoder for the
   internal panel (eDP on this machine), the plane surface-base register you
   will write, and the readback plan to confirm which pipe is live before
   touching anything. First milestone is **read-only instrumentation** (dump
   pipe config, plane base, current mode) on metal via the existing usbdebug
   instrumentation pattern; writes come only after a sitting confirms the
   decode.
3. **Citations** — every offset cited. Cleanroom source of record for this arc
   is Intel's published Ivy Bridge graphics PRMs (Intel publicly documents
   these registers). Linux i915 GPLv2 code, function names, and magic masks are
   forbidden, same as the nouveau rule on the Kepler lane. Facts-only from any
   GPL source; prefer the PRM citation every time.
4. **Scope fence** — no gmux control, no Kepler interaction, no mode-setting
   beyond reusing the mode GOP already programmed (inherit-and-repoint first;
   full modeset is a later pull).
5. **Honesty lines** — anything you cannot test locally is marked
   "NOT COMPILED HERE — Mac owed" / "metal owed". The reviewer builds and Fox
   flies it (sitting #6+).

## Lane

New kernel module under the video subsystem alongside `kepler.rs` (e.g.
`igpu.rs` / an `intel` module — propose the name); builder wiring mirrors the
existing `UNAOS_KEPLER`-style feature mapping. Do not touch Kepler files.
