# EXECNAME — a program is named by its format and launched by its name (B322, R82)

## Design (written before the code)

**Finding (B322).** The five x86 ring-3 programs (`crates/user-{prefs,vein,lumen,net,big}`) are linked
static ELF64 images; arroyo's `build_user_*_x86` run `llvm-objcopy --strip-all` (the output keeps its ELF
header — not `-O binary`) and the builder staged them as `APPS/*.BIN`. The loader sniffs the magic and
never reads the extension, so `.BIN` was a name copied from the Pi's genuinely flat `HELLO.BIN`. The
resolver (`midden_core::resolve_exec`, `EXEC_EXTS = ["elf"]`) therefore could not find them by name, and
the shell grew a `lumen` verb arm hard-wired to `bg /apps/LUMEN.BIN` to get round its own resolver.

**Seam.** Midden — `shared-core`. The launch decision lives in `unaos/libs/sys/midden_core` (the `no_std`
shell core both rings link, host-tested by `cargo test -p midden_core`): `app_note_flags(bytes)` reads
the program's own declaration, and `launch_mode(flags)` turns it into Detach / Foreground. The program
declares itself with an ELF note emitted by its own link (constants in `una-abi`, shared by the ring-3
crates and the kernel). The kernel adds no second implementation: `bare_exec` (the existing Plan::Exec
arm, both arches) asks the core and then calls the existing `bg`/`run` bodies.

**The note.** Section `.note.unaos.app` (SHT_NOTE, allocated), name `"UnaOS"` (namesz 6), type 1
(`APP_NOTE_TYPE`), desc one little-endian u32 of flags: bit0 `APP_FLAG_WINDOWED` (creates a window),
bit1 `APP_FLAG_RESIDENT` (stays running to serve — the PREFS fulfiller owns Principia's bus verbs and
must outlive the 5 s foreground bound). Each x86 link script KEEPs the section in its own output section
ahead of the `/DISCARD/ *(.note*)` line and gives it a `PT_NOTE` header; the parser walks `PT_NOTE`
headers first and section headers second, every read bounds-checked. A missing or malformed note is
flags 0 (foreground) — the fallback, never a refusal.

**Rule.** windowed or resident → detach (the `bg` path: spawn, window title via `wm::app_name_arm`, a
`BG_JOBS` row, prompt back); otherwise → foreground (the `run` path, 5 s bound, exit status printed).
`run <path>` and `bg <path>` stay as explicit overrides.

| Image | note flags | bare-name launch |
| :--- | :--- | :--- |
| LUMEN.ELF | WINDOWED | detach |
| PREFS.ELF | RESIDENT | detach |
| NET.ELF | 0 | foreground |
| BIG.ELF | 0 | foreground |
| VEIN.ELF | (no note — LUMENAPP retires it this wave) | foreground fallback |

**Milestones.** M1 NAME — stage `.ELF`; every reference follows. M2 LAUNCH — the note, the parser in
midden_core, `elf.rs::app_flags`, `bare_exec` decides. M3 NO SHORTCUTS — the `lumen` arm and its
HOST_VERBS row go (GATE-VERBS stays equal); the dock pin's launch line is `/apps/LUMEN.ELF`, dispatched
through `shell::dispatch_command` exactly as typed, so the pin and the prompt are one path. M4 WITNESS —
`tests exec`.

**Witness.** `:: EXECNAME: resolved=<k>/<n> windowed=<w> console=<c> -> PASS ::` (n = the names the
builder stages; one `:: EXECNAME: <name> -> <path> flags=<f> launch=<detach|foreground> ::` line per
name before it), or `:: EXECNAME: SKIP (<reason>) ::` when no program source is mounted.

**Owed.** The aarch64 images (the Pi stages none of these five yet). A Quarry double-click on an `.ELF`
still always detaches (`video/quarry/openers.rs`) — moving it onto `launch_mode` is a one-line follow-up
outside this arc's files. Programs have no argv yet (`net tls <host>` is still owed by NETRING3).

## Survivors of `grep -rn '\.BIN'` (justified)
