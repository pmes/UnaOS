# HELPVERB (R75) — the verbs document themselves

## Design
- Finding: `midden_core::HOST_VERBS` (unaos/libs/sys/midden_core/src/lib.rs:257) registers ~100 verbs; the only in-OS learning surface was
  midden_core's `help(facts)` name dump. No summaries, no usage, no examples.
- Also found: `trash`, `shortcuts`, `play`, `linux`, `shot` have match arms in shell.rs (~5574, ~5996) but NO HOST_VERBS row, so `plan` never
  routed them. Registered `Always` with `man` on the `reboot` line (cfg-gated-off arms refuse by name via the `other =>` net).
- Mechanism: `unaos/crates/kernel/src/help.rs` — `DOCS: &[VerbDoc]` (name, group, summary, usage, examples) written by reading each arm
  (shell.rs:5154-6370, fs/users.rs:1041, fs/trash.rs:300, drivers/hda_play.rs:356, arch/x86_64/linuxabi/mod.rs:543, shell.rs shell_fetch/shell_src/view_verb/edit_verb).
- M1: table + `tests helpdoc` (`help::selftest`, registered by `help::ensure()` from `tests::ensure_shellux`): every CORE_VERBS and HOST_VERBS name must have a doc whose
  summary does not start `(undocumented`; a missing/undocumented name is listed and FAILs.
- M2: `help` (grouped: files, windows, users, network, system, audio, tests, self-host), `help <verb>` (usage, summary, examples), `<verb> --help`: ONE check,
  `help::intercept`, folded on the `history_record(cmd_line);` line at the head of `dispatch_command` (shell.rs, line-neutral).
- M3: `man <verb>` -> `video::fileview::open_text(title, &str)` (new thin seam over `open_bytes`), x86 wc / aarch64 desktop_firmware; elsewhere prints to console.
- Witness: `:: HELPVERB: verbs=<n> documented=<n> missing=[..] groups=<n> -> PASS|FAIL ::`. Pin: x86-wc.spec REQUIRE `missing=\[\]` PASS, FORBID FAIL.
- No knob.

## Written
Boot 17 (`tests helpdoc`, or lane default tests-at-boot): `:: HELPVERB: verbs=<~108> documented=<same> missing=[] groups=8 -> PASS ::`.
Notes: aliases (dir, type, del, md, rd, copy, move, ren, rename, off, version, quit, selftest) carry their own entries "alias of X". `echo --help` is left to echo.
