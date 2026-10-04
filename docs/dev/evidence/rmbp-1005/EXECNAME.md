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

Every survivor of the five program names, and every other `.BIN` in `unaos/`, is one of:

1. **Genuine flat blobs** (`llvm-objcopy -O binary`, no ELF header): `HELLO.BIN` (the Pi and x86 U2 flat
   program), `MIDDEN.BIN`, `K2OWN.BIN`, `K2IMP.BIN` (the Pi EL0 fixture blobs, `-O binary
   --only-section=.text`), `target/user_blob.bin`. They keep the name; it is true.
2. **Data files, not programs**: the storage / ownership / write-path fixture names the kernel creates
   at run time (`SCRATCH.BIN`, `GROW.BIN`, `S8W.BIN`, `OWNED.BIN`, `DEFER*.BIN`, `STOR*.BIN`, `K2PRIV.BIN`,
   `K3PAT.BIN`, `FRESH.BIN`, `DELME.BIN`, `BUSPRIV.BIN`, `OTHER.BIN`, `COPY.BIN`, `A11/B11.BIN`,
   `UNALOG.BIN`, the `FM*`/`S2MV*`/`K6*`/`K9*` rows and the rest). Bytes on a volume; `.BIN` is right.
3. **The generic `.ELF/.BIN` opener rule** in Quarry (`video/quarry/live.rs`, `openers.rs`) and the
   `is_elf_image` doc in `arch/x86_64/elf.rs`: they name the flat format, which still exists.
4. **VEIN, left for LUMENAPP** (B323 deletes them this wave; editing a file another arc deletes is a
   modify/delete conflict at the fold): `crates/user-vein/*`, `kernel/src/vein_bus.rs`,
   `libs/sys/vein_core/*` comments, `handlers/vein/*` comments, and `crates/user-lumen/{Cargo.toml,
   src/main.rs}` comments plus its `NO_PROVIDER` string (LUMENAPP rewrites that crate; this arc adds only
   its note static at the tail). VEIN's FUNCTIONAL names — the builder's staging, arroyo, the
   `tests lumen` load in `lumen.rs`, help's `vein` row — were renamed so an unfolded tree is consistent.
5. **History**: ledger and queue rows, `docs/MILESTONES.md`, `docs/dev/evidence/**` captures, and the
   two EXECNAME comments that quote the old name on purpose (`midden_core` test, the `shell.rs` arm note).

## Witness — what the metal boot prints

`tests exec` on the boot-21 image (program source mounted, all five staged):

```
:: EXECNAME: prefs -> /apps/PREFS.ELF flags=2 note=yes launch=detach ::
:: EXECNAME: vein -> /apps/VEIN.ELF flags=0 note=no launch=foreground ::
:: EXECNAME: lumen -> /apps/LUMEN.ELF flags=1 note=yes launch=detach ::
:: EXECNAME: net -> /apps/NET.ELF flags=0 note=yes launch=foreground ::
:: EXECNAME: big -> /apps/BIG.ELF flags=0 note=yes launch=foreground ::
:: EXECNAME: resolved=5/5 windowed=1 console=4 -> PASS ::
```

After LUMENAPP's fold the `vein` row leaves `STAGED` (`kernel/src/execname.rs`) and the line is
`resolved=4/4 windowed=1 console=3`. Operator checks at the prompt: `which lumen` →
`lumen: program /apps/LUMEN.ELF`; typing `lumen` → serial `:: BAREXEC: /apps/LUMEN.ELF (typed 'lumen') —
note flags=1 -> detach ::` then the existing `… DETACHED, left RUNNING ::`; typing `net` → `note flags=0
-> foreground ::` then `:: EXEC: run /apps/NET.ELF — … exit=<n> ::`. The dock's lumen pin prints
`[dock] dockpin verb /apps/LUMEN.ELF` followed by the same two BAREXEC lines.

Host proof: `cargo test -p midden_core` (the note parser by program header and by section header, the
malformed-note `None` cases, the launch rule, `.ELF` resolution and the `.BIN` miss, `lumen` planning as
Exec); the real staged images were parsed by the same function (LUMEN 1, PREFS 2, NET 0, BIG 0; VUG,
PULSE, STAT 1 on both arches).
