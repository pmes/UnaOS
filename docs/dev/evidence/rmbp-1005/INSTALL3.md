# INSTALL3 — the installer's glass: census, layout, confirm, progress, result (B342)

Branch `exec-rmbp-install3`, cut from 522ab055, AHCIROOT (B332, `exec-rmbp-ahciroot`) merged first
(d81b1f78, clean). No new knob: the screens exist under `UNAOS_INSTGUI=1` + `UNAOS_AHCIROOT=1`
(Cargo `instgui` + `ahciroot`); nothing runs at boot (R80).

## Design (written before the code)

**Finding.** AHCIROOT put the SSD write grant behind an operator confirmation that exists only as a
shell token (`install ssd --write --erase-stranger ERASE-<port>-<sectors>`). The installer window
(`video/instgui.rs`) shows the one-partition PARTINSTALL flow and, on key `i`, SELFINSTALL2's plan as
text; the disk census in words, a layout the operator chooses, the confirmation that issues the grant,
a progress list and a result screen do not exist. `install ssd --write` also fixes the layout (ESP
512 MiB, p2 = the running volume or 4 GiB, plus the 64-sector scratch).

**Seam.** `Kernel — wm` for the screens (a kernel-owned compositor window, the instgui surface); every
judgment stays where it is: `selfinstall::probe` + `selfguard` for the verdict, `partition::census` for
the content, `amber_core::Plan` for the layout (shared-core), and ONE write path — the glass calls the
same `write_ssd` body the shell verb runs (refactored to `write_ssd_inner`, which takes an optional
`Glass { port, layout, dry_run, stage }`). The typed confirmation becomes the shell's token
(`stranger_token`) and enters the same check; nothing on the glass mints a grant by itself.

**Milestones.**
- M1 CENSUS — every disk the block layer publishes (each AHCI port, the global and USB rows): its GPT
  (`install::gpt::read_table` = `amber_core::gpt::read_table_with`), its partitions, and the verdict in
  words — `empty`, `ours`, `foreign: macOS (APFS)` / `Linux` / `Windows` / `a FAT volume` (named from the
  census content + `partition::foreign_type`), `unknown`, plus `boot disk` (selfguard) and `not SATA`.
  Only AHCI rows are selectable.
- M2 LAYOUT — ESP size (256 / 512 / 1024 MiB, `e`), UnaFS partition size (default: the rest; `w`/`s`
  step rest, 3/4, 1/2, 1/4, minimum) whose last 64 sectors are the scratch tail; the plan text is
  `amber_core::Plan::lines()` (the dry run's sector list). The volume inside p2 keeps its size (mirror
  or 4 GiB fresh): growing it to the partition stays owed (AHCIROOT).
- M3 CONFIRM — empty/ours: one Enter. Foreign/unknown: the operator types the disk's name (its model
  string, case-insensitive); a match becomes `stranger_token(port, sectors)` passed to the same check
  `--erase-stranger` reaches. Boot disk / live root / unreadable: the refusal in words; Enter still goes
  through the judgment, which refuses (nothing minted).
- M4 PROGRESS + RESULT — `write_ssd_inner` emits `[install] stage=<name> <state>` lines (probe, guard,
  grant, snapshot, gpt, esp, unafs, fsck, done) and the glass's stage callback paints them as a list;
  the result screen names the verdict and offers `r` = reboot (`power::reboot`). Every screen: Enter,
  Esc back, w/s (arrows), q halt outside the typed field.
- M5 `tests instgui` — drives the five screens with synthetic keys on the DRY-RUN path (`dry_run`: the
  judgment runs and the grant is minted, never held; no stage past `grant` touches the disk). A machine
  with no SATA disk drives a synthetic blank row whose judgment refuses (no such port).

**Witness.** `:: INSTALL3: screens=census,layout,confirm,progress,result grant=<issued|refused> dry_run=1 -> PASS ::`
(preceded by one `[install3]` detail line naming the disk, its verdict and the confirmation kind).

**Owed.** The volume is not grown to the chosen partition; mouse/click (the screens are keyboard
only — "one click" is one Enter); the whole-disk path still writes only the FIRST-listed layout of
partitions (ESP + UnaFS); unflown (R78).
