# BOOTVERDICTS (B472) — nothing wears a verdict before `TESTS: deferred` (R80)

Cut from 5da57a2d (merge19). Branch `exec-rmbp-bootverdicts`. No knob.

## Finding (the wire, f25-boots.log boot 1, lines 1..539)
32 lines / 26 tags carry `-> PASS` or `-> FAIL` before `:: TESTS: deferred=75 … ::` (line 539).
Flight 24's login-screen boot (f24-boots.log 5301..5830) adds three more arming lines in the same dress:
WINDOWCAP, USBNET (`up`), HDA. Printers and verdicts:

| tag | printer | kind | fix |
|---|---|---|---|
| U7x U8x U9x U10 U10c U10d U11x CFU2-WGATE U11m2 U6gx LFNMV BUSX86-EQ BANDY-ATTR APPMENU (14, 22 lines) | the one-task U7x ladder: `u6bx_probe_once` -> `u7x_probe_once` (arch/x86_64/syscall.rs) — it sat ABOVE the INSTALLBARE services gate, so it fired under the setter | TEST | moved: `tests ladder` (`syscall::ladder_selftest`) |
| LOGWIT-1 | `logts::logwit1` (main.rs) | TEST | moved: `tests logwit` |
| SERWIT-2 | `serial_ring::mirror_verdict_once` (first mirror poll) | TEST (an accounting audit) | moved: `tests serwit2` |
| DIMIDLE | `video::dimidle::fixture` — blanks the panel at 12 s and injects a key, under the setter | TEST | moved: `tests dimidle` (arms the state machine) |
| USBNET-EHCI | `ehci::usbnet_ehci_front` | arming | `-> armed` / `-> declined` |
| SPLASH | `splash.rs` hold surface | arming | `-> painted` |
| AHCI | `ahci.rs` attach | arming | `-> armed` / `-> declined reason=…` |
| UNAFSX86 | `fs/bootdisk.rs` root pass | arming (the mount) | `-> mounted` / `-> skipped reason=fat-root` |
| PREFS | `prefs.rs` load | arming | `-> loaded` / `-> refused` |
| FIRSTBOOT | `fs/users.rs` stage line | the stage | `-> coherent` / `-> incoherent` |
| DOCKPIN | `video/dock.rs` live table line | state | `-> held` / `-> short` |
| KFONTPPI | `video/edidsrc.rs` | arming | `-> armed` / `-> declined reason=…` / `-> skipped reason=no-edid` |
| XHCIHUB | `xhci::xhcihub_score` | census | `-> armed` / `-> declined` (the spec-shape clause) |
| WINDOWCAP / USBNET up / HDA (f24 boot 4) | `wincap.rs`, `usbnet::set_up`, `hda.rs` | arming | `-> armed` / `-> declined` |

moved = 17 tags, reworded = 12 tags (9 from f25 + 3 from f24 boot 4).

## Seam
`tests::register` (R77 M3 / R80) is the only home of a test; each moved fixture registers at its OLD call
site, so `tests-at-boot` (QEMU lanes) still runs it there. The count is the verdict tap's own:
`tests::tally` counts every `-> PASS|FAIL` line printed before the `TESTS: deferred` announce.

## Milestones
- M1 the host gate `unaos/scripts/verdictwords.py` (GATE-VERDICTWORDS), `--selftest`, run on f25/f24.
- M2 the moves (ladder, logwit, serwit2, dimidle) + `tests verdicts`.
- M3 the rewords + the spec lines that quoted them.

## Witness (the next metal boot)
`python3 unaos/scripts/verdictwords.py <f26 wire>` exits 0 (`before_deferred=0` on every boot), and
`tests verdicts` prints `:: VERDICTS: before_deferred=0 moved=17 reworded=12 -> PASS ::`.

## Owed
aarch64's U7 ladder and its own boot lines are not on this wire and are untouched; the QEMU specs that
REQUIRE the reworded lines are updated by pattern only (R78: not run).

## M1 — the gate, run on the historical wires (the fixtures that prove it counts)
- `python3 unaos/scripts/verdictwords.py --selftest` -> `boots=2 counts=[2, 0] tags=['PLANT', 'PLANT2'] -> ok`, exit 0.
- f25-boots.log: `boot=1 from=1 until=deferred@539 before_deferred=32 tags=26 [AHCI APPMENU BANDY-ATTR BUSX86-EQ CFU2-WGATE DIMIDLE DOCKPIN FIRSTBOOT KFONTPPI LFNMV LOGWIT-1 PREFS SERWIT-2 SPLASH U10 U10c U10d U11m2 U11x U6gx U7x U8x U9x UNAFSX86 USBNET-EHCI XHCIHUB]`, `-> REFUSED`, exit 1.
- f24-boots.log: four boots, before_deferred=14/13/14/17 (tags 13/13/13/16; boot 4, the login-screen boot, adds HDA, USBNET, WINDOWCAP), exit 1.

## M2/M3 — legs (inline, sequential, target removed after each)
- x86 metal shape (the seat's gate line, verbatim): `cargo +nightly check` exit 0.
- aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet` (user_blob head 280080d2): exit 0.
- aarch64 `tegra,login,loginst,virt_el0`: exit 0.
- charter-check exit 0; attrkeys-check exit 0 (new=0); arch-check exit 0 (none new).

## STATUS.tsv rows whose `line` quotes a changed line (the seat edits)
ST66 (AHCI selfcheck: `-> armed`), ST86 (SERWIT-2: now `tests serwit2`), ST114 (DOCKPIN: `-> held`),
ST133 and ST159 (UNAFSX86: `-> mounted`), ST188 (KFONTPPI: `-> armed`), ST240 (FIRSTBOOT: `-> coherent`).
The quoted historical lines stay true of their flights; a re-quote from flight 26 carries the new word.
