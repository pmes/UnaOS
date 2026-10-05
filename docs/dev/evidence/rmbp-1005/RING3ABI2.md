# RING3ABI2 — the ring-3 surface's loose ends, in one arc (rmbp-ledger B333)

Branch `exec-rmbp-ring3abi2`, cut from 8c750d43 (the merge11 tip). No knob: every change is core ABI.

## Design

**Finding (B333).** Six loose ends left by NETRING3 (B306), RING3WIN (B316), EXECNAME (B322) and LUMENAPP
(B323): `SYS_GETRANDOM` sat behind `netring3`; programs got no argv (a bare `net example.com` could not pass
the host); ring 3 could not ask who it runs as (LUMENAPP needed a preference for the key path); NET.ELF
still sat in the 16 KiB fixed window; aarch64 had neither `SYS_SBRK` nor the ELF window (user-lumen's
aarch64 link failed `data+bss past 0x3800`); Quarry's double-click on an `.ELF` always detached.

**Seams.** *Kernel — the ring-3 ABI* (`una-abi` declares it once; both kernels fulfil it) and
*shared-core* for the launch rule (`midden_core::launch_mode`, which the shell and now Quarry both call;
`midden_core::Plan::Exec` now carries the typed words, so the core — not each ring — splits the line).
The args-page layout, its builder and its parser live in `una-abi` (safe code, host-tested) so the kernel
writes and ring 3 reads the SAME layout; `una_abi::args()` is the one ring-3 reader. The `SYS_WHOAMI`
record likewise (`whoami_build` / `whoami_parse`); its kernel body reads the users store
(`fs::users::whoami` / `home_of` / `id_of`), never a literal `/home/<name>`.

**The args page.** One RO page at a FIXED VA in every address space a loader places:
x86 `USER_BASE + 0x1FF000` (the last page of the slot's first 2 MiB — above the FB hole, below the ELF
window; the same VA in the fixed, flat and elf models); aarch64 `481 GiB + 0x1FF000`. Layout:
`[magic "ARGS"][argc u32][window_base u64][argv[] u64 absolute VAs][0][0 (envp)][strings, NUL-joined]`,
≤ 4 KiB, ≤ 32 words. `window_base` is the classic window base: an elf-model program cannot derive the FB
landmarks (`+0x4000` info page, `+0x5000` surface) from its own `_start`. It is a legal syscall INPUT
range (so `argv[1]` goes straight to `SYS_RESOLVE`), never an output.

**aarch64 window VA (M5) — the RING3WIN reasoning, applied.** x86 put its ELF window at `USER_BASE + 2 MiB`:
a FIXED VA (so an image links at its run address and needs no relocation), clear of the classic window
and the FB hole, wired from static per-slot tables with frames from the kernel heap. On the Pi the classic
aarch64 window has no fixed VA (it is the identity PA of a `.bss` anchor) and the rest of its 2 MiB block
is live kernel `.bss`, so the window cannot sit beside it. It goes in its own **extension GiB**: L1 entry
481 of every slot's own TTBR0 table (VA 481 GiB) — above every identity window any aarch64 board maps (Pi
RAM GiB 0..=3, Orin DRAM and PCIe below ~200 GiB, the Orin's own classic window at 480 GiB) and below the
512 GiB ceiling of the 39-bit VA. Inside it the x86 offsets are reused verbatim: args page at `+0x1FF000`,
ELF window at `+2 MiB` for 4 MiB (`una_abi::USER_XWIN_VA_ARM` = 0x7840200000). One static L2 and three
static L3s per slot (`arch/aarch64/xwin.rs`), frames from the heap, freed at the slot's last teardown.
`SYS_SBRK` (58) on aarch64 has the x86 semantics (fixed-model programs get the whole window as heap).

**Milestones.**
- M1 GETRANDOM — `SYS_GETRANDOM` dispatched unconditionally on both arches; `rand`, `hash` and the DRBG
  un-gated (a knob-off build has entropy).
- M2 ARGS — una-abi layout; x86 and aarch64 args pages; `Plan::Exec { typed, name, args }`; `bare_exec`,
  `run` and `bg` all pass their words through ONE launcher (`run_image` / the bg body take `argv`);
  NET.ELF takes its host from `argv[1]`.
- M3 WHOAMI — `SYS_WHOAMI` (59) on both arches; user-lumen's key path defaults to
  `<home>/.config/unaos/vein.key` from it (the preference still overrides).
- M4 BIGAPPS — NET.ELF relinked at the ELF window (BIG.ELF already is): `user-net-x86.ld` takes the
  user-big shape, arroyo's NET build gains `-z stack-size` and `user_elf_window_check`.
- M5 ARM — `arch/aarch64/xwin.rs`: the extension GiB, the elf model in the aarch64 loader, `SYS_SBRK`,
  the args page; user-lumen's aarch64 link moves to the aarch64 ELF window (`user-lumen.ld`).
- M6 QUARRY — an `.ELF` double-click reads the note: `midden_core::launch_mode` → Detach (windowed or
  resident) or a console window (the shell window runs `run <path>`, exactly as typed).
- M7 WITNESS — `tests ring3abi`.

**Witness.** `:: RING3ABI2: getrandom=1 args=<n> whoami=<name|none> net_window=elf big_window=elf
arm_sbrk=<1|skip> -> PASS ::` (x86 prints `arm_sbrk=skip`; the aarch64 build prints `arm_sbrk=1` from its own
in-kernel probe of the extension GiB).

**Stays owed.** The metal boot (R78). One `crate::elf` validator for x86, aarch64 and linuxabi (the aarch64
elf-model validator is a third copy, flagged). TLS on aarch64 (embedded-tls is an x86-only dep of
vein_ring3; the aarch64 LUMEN image links with the echo provider). envp is reserved (empty) in the layout.

## Results (compile legs; R78 — no QEMU, the metal boot is the seat's)

Commits: M1 `087496dc` · M2+M3+M5 kernel `ac454415` · M4 + NET argv `32db44da` · M3/M5 user halves
`681aa9d2` · M6 `c284a295`.

- x86 metal shape (`wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,
  beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,
  installdemo,instgui,witness`): exit 0.
- x86 base without netring3 (`wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,
  facet,beam,sdw,selfhost,linuxabi,ahci,unafs,busreg`): exit 0.
- aarch64 `login,loginst,virt_el0,lumen,netring3,busreg`: exit 0. aarch64 `login,loginst,baremetal,lumen,
  netring3,busreg` (the Pi slot backend, `boot.rs`): exit 0.
- User crates via their arroyo functions: `build_user_net_x86` exit 0 (`NET-X86.ELF: 9048 B file,
  model=elf span=4588 stack=65536`, `:: ELFENTRY: NET-X86.ELF entry=0x10000200000 … -> PASS ::`, PT_NOTE
  owner UnaOS kept); `build_user_big_x86` exit 0 (`model=elf`, ELFENTRY PASS); `build_user_lumen_x86`
  exit 0 (`model=elf span=257264 stack=262144`, ELFENTRY PASS). aarch64 user-lumen `cargo build` with the
  USER_CHECK_MATRIX flags: exit 0 — it LINKS: entry 0x7840200000, PT_LOADs R+X / R / R+W at the aarch64
  ELF window, PT_GNU_STACK 256 KiB, PT_NOTE kept.
- `cargo test -p midden_core -p una-abi`: exit 0 (`:: RING3ABI2-ABI: … -> PASS ::`).
- `charter-check.sh`: exit 0.

**Wire a metal boot should print** (`tests ring3abi` with a session open, NET.ELF and BIG.ELF staged):
`:: RING3ABI2: getrandom=1 args=2 whoami=<user> net_window=elf big_window=elf arm_sbrk=skip -> PASS ::`
(aarch64: `arm_sbrk=1`, windows `skip`). `net example.com` prints
`:: NETRING3: host=example.com argc=2 rand=32 resolve=<ip> connect=0 … ::`; a bare-name detach prints
`:: BAREXEC: … argc=<n> DETACHED … ::`; a Quarry double-click on a console program prints
`:: QUARRY-LAUNCH: <path> — note flags=0 -> foreground: the shell window runs `run <path>` ::` then the
`run` lines in the shell.

**Not finished / owed.** The metal boot. The shell-line seam is drained only by the x86 render body
(`dock::LINE_LAUNCH_DRAINED`): aarch64 Quarry keeps detaching console programs. A console program whose
path holds a space is refused by Quarry (the `run` line splits on whitespace). TLS on aarch64 (the
aarch64 LUMEN links with the echo provider only). One shared ELF validator (xwin.rs is the third copy).
envp reserved, empty.
