# APPTITLE — prep

## The finding

QUEUE.md (2026-09-12, render13 on the Orin, Peter on the glass — shared code, every board), item
(a): "launched programs are titled 'Application' in the taskbar and menubar —
`[winmenu] app-menu owner=N name=Application kind=default` for every QUARRY-LAUNCH'd ELF; only
Shell is `from=declared`... the launch path knows the ELF path and the app_name registry
(wintitle) is not fed for EL0 launches." R36 (`docs/dev/RULINGS.md:67`): "An app window's title is
the app's NAME (declared, else its program name), never a generated label."

**Correction, found while reading the code (see Mechanism): this is already fixed on trunk.**
`LEDGER.md:80` (SO28), branch `exec-orin27-appname`, status `fixed-unflown`: the code is in, green
under `UNAOS_WC=1 ./arroyo test` on x86 QEMU, but unconfirmed on real hw-rmbp metal, and the
x86-wc.spec pin covers only the older SO22 fixture (dock label), not SO28's race-fix witness. The
remaining rmbp work is verification + a spec pin, not new code.

## Mechanism

- `unaos/crates/kernel/src/video/wm.rs:849-889` — `app_name_arm(owner, path)`: records
  `program_name(path)` (the ELF basename, extension dropped, `wm.rs:816-833`) against the
  compositor `owner` in `APP_NAMES` (leaf mutex, `wm.rs:770`), then calls `app_name_adopt`.
- `wm.rs:889-916` — `app_name_adopt(owner, name)`: **the race fix.** `spawn_user_image_bg` makes
  the child runnable before it returns, so the child can reach `SYS_WIN_CREATE` before the
  launcher's `app_name_arm` call lands (measured on render13 boot 1: 4/6 windows read
  `Application`, `docs/dev/evidence/orin27/render13-boot1-comp.log`). `app_name_adopt` re-titles
  any of that owner's *already-open* rows still at `TitleSource::Unnamed`, so the arm is
  retroactive regardless of who reaches `SYS_WIN_CREATE` first.
- `wm.rs:661-724` — `TitleSource` enum (`Declared`/`Program`/`Document`/`Unnamed`) +
  `as_from_str()` — prints `from=declared|program|document|unnamed` as one grep-able literal.
- `wm.rs:934-966` — `TITLE_SRC`: one lock-free atomic cell per window id, written wherever the
  caption is written, read by `winmenu::set_app_window` (`winmenu.rs:519-546`) which runs inside
  `strip::compose_all` under a no-lock contract — this is why provenance is a side atomic array
  and not a table field.
- `winmenu.rs:544` — the witness line itself:
  `"[winmenu] app-menu owner={} name={} {} kind={}"`, args `id`, caption, `from=...`, `custom|default`.
- Launch-path callers of `app_name_arm(owner_of_launch(handle), path)`, every EL0 launcher already
  wired: `video/quarry/live.rs:1537` (QUARRY-LAUNCH/double-click, the ARC's named path — right
  after `spawn_user_image_bg` returns at `:1522-1537`, before the `:: QUARRY-LAUNCH: ... ::`
  witness at `:1540`), `shell.rs:6925` (`bg` verb), `shell.rs:7346` (bare-name launch),
  `arch/x86_64/syscall.rs:19402`/`:19538` (hardcoded `VUG.ELF`/`PULSE.ELF` spawns),
  `video/desktop_uefi.rs:925` (UEFI desktop launcher).
- `wm.rs:826-833` — `owner_of_launch(handle)`: corrects the per-arch off-by-one between what
  `spawn_user_image_bg` returns on x86 (`mapped.slot`, 0-based) vs. aarch64 (`ttbr0 >> 48` =
  `slot + 1`) before it is used as the `APP_NAMES` key — every launcher must go through this, not
  the raw handle.
- Fixture: `wm.rs:26754-26904` `wintitle_selftest()`. Legs 1-7 assert the derivation, the
  generated-label noun (`ANON_APP = b"Application"`, `wm.rs:747`, digit-free per R36), the
  declared/document clauses, and the wired create→row readback. **Leg 8 (`wm.rs:26838-26867`) is
  the SO28 race leg**: drives the *losing* order deliberately — create the window first (asserts
  `Unnamed`/`Application` beforehand), *then* arm — and requires the row end up `VUG`/`Program`
  anyway. Run-once (`DONE` swap, `:26756`), called from the same tail battery as
  `closemin_selftest` at `wm.rs:25240` (`UNAOS_WC=1 ./arroyo test` x86, `desktop_firmware` aarch64).
- Witness line printed (`wm.rs:26884-26903`):
  `:: WINTITLE: program_name=1 document=1 label=1 seam="el0 win " program=1 no-seq=1 declared=1 row=1 forget=1 late-arm=1 PASS ::`
  (`row=` and `late-arm=` can print `skip` instead of `0`/`1` when the window table is full —
  `wm.rs:26816-26825`, `26855-26866` — a SKIP is honest mid-slice, never a silent PASS).
- Spec coverage today: `unaos/scripts/specs/x86-wc.spec:141-147` pins **SO22** only — the older
  dock-label fixture (`WINX-8`), not `WINTITLE`/`late-arm`. `grep -n WINTITLE` on that file has no
  hit — the SO28 fixture's PASS line is unpinned, so a regression that deletes the
  `app_name_adopt` call (SO28's own go-red) would not red `./arroyo mbench` on this lane, only a
  human diff.

## Plan

Code is done (SO28); what's owed for rmbp is verification plumbing, not new logic.

- **M1 — pin `WINTITLE` in `x86-wc.spec`.** File: `unaos/scripts/specs/x86-wc.spec`, appended
  after the SO22 block (currently ends `x86-wc.spec:147`, same tail-append convention that block's
  own comment cites). Add:
  ```
  REQUIRE :: WINTITLE: program_name=1 document=1 label=1 seam="el0 win " program=1 no-seq=1 declared=1 row=\S+ forget=1 late-arm=\S+ PASS ::
  FORBID :: WINTITLE: .* FAIL ::
  ```
  `row=` and `late-arm=` as `\S+` (not literal `1`) because both legs SKIP under a full window
  table — pinning them literal would red a boot that is merely slice-loaded, the exact inversion
  GATE-TESTTRUNC (cited in this same spec file) exists to prevent. Witness printed by
  `wintitle_selftest`, `wm.rs:26884`. Go-red: comment out the `app_name_adopt(owner, ...)` call at
  `wm.rs:915` (SO28's own stated go-red) — leg 8's `armed_late` still true but `named` false, so
  `late_ok = Some(false)`, overall `ok=false`, line reads `late-arm=0 ... FAIL`; rebuild, rerun
  `UNAOS_WC=1 ./arroyo test`, confirm `FORBID` line fires (`FAIL` present) and `mbench` exits
  non-zero; then revert.
- **M2 — flight the fix on hw-rmbp metal**, closing SO28's "fixed-unflown" half for this board.
  No file changes: a QUARRY-LAUNCH capture on the bench rMBP (double-click an ELF from Quarry, not
  `bg`), then `awk 'index($0,"[winmenu] app-menu")'` / `awk 'index($0,"QUARRY-LAUNCH")'` over the
  serial capture, confirming `name=<PROGRAM> from=program` (never `name=Application` unless a
  build genuinely declares it) and `:: WINTITLE: ... late-arm=1 ... PASS ::` in the same boot's
  tail battery. Evidence to `docs/dev/evidence/rmbp-0924/<capture>.log`, cited into `LEDGER.md`
  SO28's board/evidence cells by the seat.
- **M3 (only if M2 reproduces `name=Application` on a QUARRY-LAUNCH'd ELF on real metal)** —
  means SO28 has a gap this reading did not find (an unlisted launch path, or a timing window
  `app_name_adopt` doesn't close at the rMBP's core count). Capture the failing boot, `awk` the
  `QUARRY-LAUNCH`/`app-menu` lines, diff against `render13-boot1-comp.log`'s shape before writing
  new code — do not re-open `app_name_arm`/`app_name_adopt` without a fresh flight line naming it.

## Draft code (unbuilt)

None — M1 is a spec-file addition (verbatim above, not a kernel edit); M2/M3 are capture-and-measure.

## Spec pins

`unaos/scripts/specs/x86-wc.spec` (append after line 147, tail-append convention, own commit
alongside no kernel change since the line already exists):
```
REQUIRE :: WINTITLE: program_name=1 document=1 label=1 seam="el0 win " program=1 no-seq=1 declared=1 row=\S+ forget=1 late-arm=\S+ PASS ::
FORBID :: WINTITLE: .* FAIL ::
```
No look-around used; both lines are plain literal/`\S+`/`.*` per this repo's spec grammar.

## Open questions

- Does M2's metal capture close SO28 outright, or does Peter want a second, rmbp-specific ledger
  row? A person decides.
- QUEUE.md is dated 2026-09-12 and still reads open on 2026-09-24, twelve days after SO28's branch
  cut — is the queue entry itself stale and due to be struck by the seat?

## Next-session start

1. `sed -n '140,148p' unaos/scripts/specs/x86-wc.spec` — confirm the SO22 block's exact tail
   before appending M1's two lines after it.
2. Append the M1 `REQUIRE`/`FORBID` pair, then `UNAOS_WC=1 ./arroyo test` followed by
   `./arroyo mbench --replay target/serial.log --spec unaos/scripts/specs/x86-wc.spec --platform x86`
   to confirm it scores PASS on the current (already-fixed) tree.
3. Do the go-red in M1 (comment `wm.rs:915`'s `app_name_adopt` call, rebuild, rerun, confirm
   `mbench` reds), revert, then move to M2's metal capture on hw-rmbp.
