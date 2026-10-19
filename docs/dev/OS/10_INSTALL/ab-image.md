# ABIMAGE — two kernel slots, the user's restart, the fallback (rmbp-ledger B683)

**Law (BP-bar-8, BP-boot-15):** the image is the unit of change and no restart is forced. An update stages a second
image; the loader switches only after a restart the user asked for, and falls back to the current image if the new one
never prints `DESKTOPBUILT` PASS. x86 (the rMBP) only: every piece is `x86_64` + `installdemo` (on the image line).

## The ESP

| file | written by | meaning |
|---|---|---|
| `KERNEL.ELF` | the build; the promotion | the current image (the loader has always opened `kernel.elf`) |
| `KERNEL.NEW` | `install::abimage::stage_image` | the staged image |
| `STAGE.REC` | the stage (LAST, after the re-read); removed by the promotion and the loader's fallback | `stage=1 kelf=0x<max_vaddr> bytes=<n> crc=0x<crc32>` |
| `SHUTDOWN.REC` | the stage (`staged`), `power::prelude` (`user`), the LOADER (`trial`, `fallback`), the promotion (`promoted`) | `why=<…>` |

The records and the decision are ONE file, `unaos/crates/kernel/src/install/abslot.rs`; the loader compiles it by
`#[path]` (`unaos/crates/bootloader/src/main.rs`, file tail), so `tests abimage` asserts the function the loader runs.

## The loader's decision (`abslot::decide`, pure over the two records)

| STAGE.REC | SHUTDOWN.REC | slot | `why=` | the loader writes before the jump |
|---|---|---|---|---|
| absent / not `stage=1` | any | ELF | `normal` | nothing |
| staged | `why=user` | NEW | `user-restart` | `why=trial`; if that write fails it stays on ELF (an unmarked trial could repeat forever) |
| staged | `why=trial` | ELF | `fallback` | `why=fallback`, `STAGE.REC` removed; prints `[loader] fallback from=NEW reason=no-desktopbuilt` |
| staged | anything else | ELF | `normal` | nothing — a crash, a panic-screen reset or firmware restart never switches |

Every boot prints `[loader] slot=<ELF|NEW> why=<user-restart|fallback|normal>` (`slots::open`, before `KELF`). If the
chosen file is missing the loader opens the other slot and says `[loader] slot=<X> missing: opening slot=<Y>`.

## The kernel's half (`install/abimage.rs`; every ESP write through `bootfat::fat_unlock(Writer::Updater, …)`)

1. **Stage** `stage_image(path)`: heal a torn promotion, remove `STAGE.REC` and `KERNEL.OLD`, copy the image to
   `KERNEL.NEW` in 1 MiB chunks, re-read it against its CRC-32, `SHUTDOWN.REC := why=staged` (a stale `why=user`
   never arms this stage), write `STAGE.REC`. `[update] staged kelf=<max_vaddr> bytes= crc= from=`. `kelf` is the
   loader's `KELF max=` computed over the PT_LOAD headers by `elf_core::read_phdrs` (at most 8 PT_LOADs).
2. **The user's restart / shut down** (`power::prelude`, the funnel of the shell verbs and the crystal menu; the panic
   screen's reset bypasses it): when staged, `SHUTDOWN.REC := why=user` and `[update] shutdown record why=user act=`.
3. **Promote** `desktop_built(pass)` at the boot's DESKTOPBUILT verdict: only on a TRIAL boot (staged and `why=trial`,
   which only the loader writes when it jumped to NEW) and only on PASS — rename `KERNEL.ELF`->`KERNEL.OLD`,
   `KERNEL.NEW`->`KERNEL.ELF`, unlink `KERNEL.OLD`, remove `STAGE.REC`, `why=promoted`. `[update] promoted NEW -> ELF`.
   A cut between the two renames leaves `KERNEL.NEW` alone; the loader opens it (the missing-slot rule) and the next
   stage or unstage renames it home first (`heal`).

**OWED (outside this arc's files):** the call of `desktop_built` from the DESKTOPBUILT verdict in
`unaos/crates/kernel/src/video/desktopbuild.rs` (`say`, the `);` closing the `:: DESKTOPBUILT:` print) — until it
lands a trial boot is never promoted and the next boot falls back. No operator verb calls `stage_image` yet.

## Wire, in order (a stage, a user restart, a good NEW)

`[update] staged kelf=0x…` → `[update] shutdown record why=user act=reboot` → `[loader] slot=NEW why=user-restart` →
`KELF … max=<the staged kelf>` → `:: DESKTOPBUILT: … ::` → `[update] promoted NEW -> ELF kelf=0x…` → next boot
`[loader] slot=ELF why=normal` with the same `KELF max=`. A NEW that dies: the next boot prints
`[loader] fallback from=NEW reason=no-desktopbuilt` + `[loader] slot=ELF why=fallback` and the old `KELF max=`.

`tests abimage`: `[update] abimage case=<unstaged|user|trial|crash> slot= why= -> ok` ×4, then
`:: ABIMAGE: decide=4/4 stage=staged kelf=0x… record=ok unstaged=1 -> PASS ::` (it stages a copy of `/boot/KERNEL.ELF`
and removes it).
