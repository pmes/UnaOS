# PRTSCR4 (rmbp-ledger B493) — the screenshot lands on the Desktop and the glass says where

Cut from 21521a53 (merge19). Flight 26, image 19.

## Finding (from the wire, f26-boot3.log)

- The ONE capture on the wire is not a Print Screen: `PRTSCR: DESKTOP.PNG … name_from=named` is the read
  list's `shot desktop` (GLASSEYES, B343), whose leaf `shotmask::SHOTS_DIR = "Shots"` minted
  `/home/una/Shots` (`PRTSCR-DIR … dir=/home/una/Shots created=1`) and `SHOTMOUNT via=vfs
  path=/home/una/Shots/DESKTOP.PNG bytes=172846 -> PASS`.
- The Print Screen key's own destination is already the theme's `Desktop` (`PRTSCR-DIR-FIX … dir=/home/una/Desktop
  … -> PASS`, the same boot) with the Mac's name when the clock is set (`prtscr::mac_name`, FATLFN).
- What failed Peter on boot 1 is the GLASS: `prtscr::finish` toasts only a region/window capture
  (`if shot.kind != 0`); a whole-panel capture — the key Peter pressed — is SILENT, and the desktop shows
  no files (MACPARITY row 29: "Δ desktop icons"), so a capture that landed is indistinguishable from one
  that did not. Boot 1's wire died 2 s after login, so its capture's own verdict never reached the wire.

## Seam

Kernel — wm (the existing capture job, `video/prtscr.rs`; the one toast surface `dialog::notice` → `toast::post`).
No new file, no new store, no new knob.

## Milestones

- **M1** — every user capture (Print Screen, Cmd-Shift-3/4, region/window, the `screenshot` verb) ends in
  `prtscr::announce`: the wire line `[prtscr] saved path=<p> bytes=<n> -> toast` and one toast through
  `dialog::notice` (title `Screenshot`, lines `Screenshot saved to Desktop` and the path). `Shot` carries the
  mount-table path. A named state shot (`shot <state>`) and `tests prtscr` stay silent on the glass (a toast
  would land in the next state's golden).
- **M2** — `/home/<u>/Shots` is gone from the tree: `shot <state>` writes into the capture folder the theme
  names (`/home/<u>/Desktop/<STEM>.PNG` + `.MSK`); a region/window capture takes the Mac's name too
  (`Screenshot <date> at <time>.png`, the `SHOT-hhmmss.PNG` stamp retired); the docs and the EYES bench
  path follow.
- **M3** — `tests prtscr` reads the new path: PRTSCR-ST's native read-back says `path=` and scores that the
  file is in `<home>/Desktop` (`in_capture_dir=yes`), in the same PASS/FAIL verdict.

## Witness (the next flight)

Print Screen on the glass after login:
`[prtscr] saved path=/home/una/Desktop/Screenshot 2026-10-06 at 14.57.18.png bytes=<n> -> toast`
and `[notice] route title=Screenshot kind=info -> toast`; `tests prtscr` →
`PRTSCR-ST: native read-back path=/home/una/Desktop/… in_capture_dir=yes … -> PASS`; the read list's
`shot desktop` → `SHOTMOUNT: via=vfs path=/home/una/Desktop/DESKTOP.PNG … -> PASS` (no `/home/una/Shots`).

## Owed

- Desktop icons (the files in `~/Desktop` drawn on the desktop) — MACPARITY row 29's Δ; not this arc.
- A `Shots` folder already made on a flown card is not deleted (no migration runs at boot, R80).
- The toast has no thumbnail (the Mac's floating thumbnail is owed).
