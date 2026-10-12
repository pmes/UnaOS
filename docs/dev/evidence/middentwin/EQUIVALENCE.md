# MIDDENTWIN (LEDGER SR75) — the equivalence B171 wanted

Ten lines planned by `midden_core::plan` through the kernel shell's `Facts` and through MIDDEN.ELF's `Facts`,
over ONE fixture `Volume` (`NameList(["VUG.ELF", "STAT.ELF", "README.TXT"])`). The KAT is
`unaos/libs/sys/midden_core/src/lib.rs`, `middentwin_tests::ten_lines_plan_the_same_where_both_rings_can_and_refuse_in_the_cores_words_where_one_cannot`.

Measured with: `cargo test -p midden_core middentwin -- --nocapture` (from the repo root): `3 passed; 0 failed`.

* Kernel facts: `shell.rs` `midden_facts()` on x86, field for field: `{ aarch64: false, x86: true, v3d: false,
  proc_verbs: true, proc_rows: 16, exec: true }` (`proc_rows` is `proc_table_rows()` at run time; it reaches only
  the `storm` help line).
* Ring-3 facts: `Facts::ring3(true)` = `{ aarch64: false, x86: true, v3d: false, proc_verbs: false, proc_rows: 0,
  exec: false }`. `proc_verbs`/`exec` are false because the ring-3 ABI has no spawn-by-path: `SYS_SPAWN` (8)
  loads only the fixed staged program (`arch/x86_64/syscall.rs` `sys_spawn`, "the pre-staged program (HELLO.BIN)").
* Outcome rule: the kernel shell fulfils every verb its facts register, so its outcome is the `Plan` debug text.
  MIDDEN.ELF fulfils the `Plan::Host` verbs in `midden_core::ring3::SERVED` (clear history pwd cd ls dir cat type
  stat); for any other Host verb it prints `midden_core::ring3::refusal(verb)`.

| # | line | kernel (Plan debug) | MIDDEN.ELF | same? |
|---|------|---------------------|------------|-------|
| 1 | `""` | `Nothing` | `Nothing` | same |
| 2 | `help` | `Say(TerminalOutput(..4116 bytes))` | `Say(TerminalOutput(..3657 bytes))` | DIFF (4 lines, below) |
| 3 | `echo hello twin` | `Say(TerminalOutput("hello twin"))` | identical | same |
| 4 | `exit` | `Say(TerminalOutput("this console IS the shell — …"))` | identical plan (the ring then closes the window) | same |
| 5 | `LS -l` | `Host { verb: "ls", rest: "-l" }` | identical, served | same |
| 6 | `cat README.TXT` | `Host { verb: "cat", rest: "README.TXT" }` | identical, served | same |
| 7 | `cd /apps` | `Host { verb: "cd", rest: "/apps" }` | identical, served | same |
| 8 | `vug` | `Exec { typed: "vug", name: "VUG.ELF", args: [] }` | `Say(TerminalError("Unknown command. Type 'help' for assistance."))` | DIFF |
| 9 | `run /apps/VUG.ELF` | `Host { verb: "run", rest: "/apps/VUG.ELF" }` | `Say(TerminalError("Unknown command. Type 'help' for assistance."))` | DIFF |
| 10 | `ping 10.0.2.2` | `Host { verb: "ping", rest: "10.0.2.2" }` | same plan; REFUSED `TerminalError("ping: not available on this build (the verb exists; the ring-3 ABI does not carry it)")` | DIFF |

## The difference set (asserted exactly by the KAT)

* **`help`** — both rings answer in full from `midden_core::help`; ring 3's text is a strict subset of the
  kernel's, missing exactly the four lines keyed on the two facts ring 3 does not have: `PROC: bg <path> / run
  <path> …`, `<name> (just type it: …)`, `the .elf is optional …`, `storm [n] (… n>16 …)`.
* **`vug`, `run /apps/VUG.ELF`** — only the kernel can fulfil (bare-name launch and `run` need spawn-by-path).
  Ring 3's answer is the core's own: on `Facts::ring3` the word is not a verb and `exec` is off, so `plan`
  returns its "Unknown command" refusal. No stub.
* **`ping 10.0.2.2`** — the PLAN is identical on both rings (`ping` is `Avail::Always`); only the kernel carries
  a NIC verb, and MIDDEN.ELF prints the core's refusal text (`midden_core::ring3::refusal`), which is the kernel
  drift net's sentence (`shell.rs` `other =>`) with "the ring-3 ABI" for "this kernel".

Where both rings can fulfil (lines 1, 3–7) the `Plan` debug text is identical, byte for byte.

## What closes the remaining differences

A ring-3 spawn-by-path syscall (the `run` the kernel shell performs, `spawn_user_image_bg_argv` reached from
ring 3) would let `Facts::ring3` carry `proc_verbs`/`exec` and collapse lines 2, 8 and 9 into "same". It needs a
new syscall in `arch/x86_64/syscall.rs` and a una-abi number — outside this arc's files; reported, not made.
