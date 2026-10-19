# BP-ring3 — programs on UnaOS: ABI, bus, ELF, slots, windows, float (read at 1719fe54)

## 1. What it is
- A ring-3 program is a static ELF from a `user-*` crate in `unaos/crates/`, staged as `/apps/<NAME>.ELF` on the root volume and launched by its bare name (R82, R94).
- The ABI crosses through `una-abi` (70 `SYS_*` numbers 1..76; packed input words; bus verb numbers) (`unaos/crates/una-abi/src/lib.rs`). Each arch's `syscall.rs` owns only the user copy and the trap.
- The bus (`unaos/crates/kernel/src/bus.rs`) is the on-UnaOS SMessage v1 codec: a fixed 52-byte header plus typed payloads over `SYS_MSEND`/`SYS_MRECV` (19/20).
- A handler that lives on UnaOS either IS a ring-3 program (Holocron, Stria, Aether, …) or is FULFILLED by the kernel through its bus verbs (Principia's prefs).
- UnaOS ring-3 code on x86 has NO FPU/SSE: integer and soft-float only. aarch64 ring-3 targets carry `+neon`.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-ring3-1 | `una-abi` holds only what crosses the boundary (numbers, flags, packed layouts, shared magics), is `const`-only with no dependencies, and a number means the same verb on every arch (unimplemented = `-ENOSYS`). | `unaos/crates/una-abi/src/lib.rs:4-31` | compile; `gates` row `verbs` (`unaos/scripts/verb-roots.sh`) for shell verbs |
| BP-ring3-2 | Per-arch differences in the ABI are a documented `cfg` in una-abi itself (e.g. `GETINFO_TICK_HZ` 1000 x86 / 250 aarch64; callers convert with `getinfo_ticks_to_ms`). | `unaos/crates/una-abi/src/lib.rs:36-50` | compile |
| BP-ring3-3 | The bus wire is UnaOS-native (R17), frozen by self-authored goldens; the largest allocation a hostile frame can force is `BUS_FRAME_MAX` = 4148 bytes. Both arches run the codec KATs every boot. | `unaos/crates/kernel/src/bus.rs:1-30`; `unaos/crates/kernel/src/lib.rs:244` | `bus_codec_selftest`, `bus_codec2_selftest` |
| BP-ring3-4 | Bus verb ranges: 1-24 fixed verbs (fs, menus, attrs, prefs 16-19, dialog/sheet/toast 20-22, PrefDeclare 23, DropGet 24); 127 REGISTER; ≥128 the registrable/fulfilled range (only under `busreg`); Holocron 144-151. | `unaos/crates/una-abi/src/lib.rs:645-909` `unaos/crates/una-abi/src/lib.rs:1732-1733` `unaos/crates/una-abi/src/lib.rs:1839-1843` `unaos/crates/una-abi/src/lib.rs:1941` `unaos/crates/una-abi/src/lib.rs:2384`; `unaos/crates/kernel/src/bus.rs:88` | compile |
| BP-ring3-5 | The kernel's own desktop speaks to Principia's store as a BUS CLIENT through the same `frame_parse`/`request_validate` a ring-3 frame passes. | `unaos/crates/kernel/src/prefs_client.rs:1-12` | `gates` row `prefs` |
| BP-ring3-6 | Ring 3 is not capped by a pool: per-slot sidecars are heap-grown `SlotVec`s; the only bound is the slot type and `wincap` (BP-video-7). | `unaos/crates/kernel/src/procslot.rs:1-14`; R90 | `[wm] limit … procs=` line |
| BP-ring3-7 | The ELF segment plan is ONE shared core both loaders use (`elf_core`); the dynamic loader core is `ldso_core`. | `Cargo.toml:35,37` (members comments); `unaos/crates/kernel/Cargo.toml:3941` `unaos/crates/kernel/Cargo.toml:3947` | compile |
| BP-ring3-8 | A program launched in a session runs AS the session user (`user:<name>`), stamped by the loader into the slot principal. | `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16892` `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16999`; `unaos/crates/kernel/src/arch/aarch64/syscall.rs:7767` | `tests ring3abi` / `SYS_WHOAMI` (61) (`unaos/crates/kernel/src/ring3abi.rs:1-12`) |
| BP-ring3-9 | Window ABI: `SYS_WIN_CREATE/PRESENT/MOVE/CLOSE/PRESENT_ROWS` (29-33); present returns 0 for every success on both arches; surface ≤ 288x288 on x86. | `unaos/crates/una-abi/src/lib.rs:230-266`; `unaos/crates/kernel/src/arch/x86_64/memory.rs:537-539` | compile |
| BP-ring3-10 | A UnaOS-native face is a surface, an input stream and a resize (R108); it reads its colours through palette_core roles (BP-video-1). | R108 (RULINGS); `unaos/libs/sys/palette_core/src/lib.rs:8-13` | `gates` row `appearance` (the four `theme.rs`) |
| BP-ring3-11 | x86 ring-3 targets are `+soft-float` with SSE off; CR4.OSFXSR is CLEARED whenever a UnaOS ring-3 task is dispatched, so its first SSE instruction is `#UD`. Only Linux-ABI tasks get an eager FXSAVE context. | `unaos/x86_64-unknown-none.json:15`; `unaos/crates/kernel/src/arch/x86_64/linuxabi/fpu.rs:10-25` | unheld by a gate; the `#UD` fence itself |
| BP-ring3-12 | Entropy and name lookup for ring 3 have ONE kernel body each (`SYS_GETRANDOM` 56 → the DRBG; `SYS_RESOLVE` 57 → the arch's resolver). | `unaos/crates/kernel/src/netring3.rs:1-20` | `tests net` |
| BP-ring3-13 | A program declares console vs windowed in a PT_NOTE read by `midden_core::app_note_flags`; no `run`/`bg` to type (R82). | `unaos/crates/user-big/src/main.rs` (EXECNAME note); R82 | unverified gate |

## 3. Seams
- Ring 3 may call: `una-abi` syscalls only; the bus for handler verbs; the shared `*_core` crates it links.
- The kernel fulfils: prefs (16-19, 23; 128-130 for ring-3 fulfilment), attrs (11-15), menus (7-9), dialogs (20-22). Registered ring-3 fulfillers own ≥128 (Holocron 144-151).
- Placement of ring-3 tasks: cores.md BP-cores-1, -5.

## 4. The ring-3 ELFs built (`unaos/arroyo:6161-6186` USER_CHECK_MATRIX, plus `build_user_holocron_x86` at `:4442`)
| ELF | x86 | aarch64 | owner (CHARTER/CODEX) |
|---|---|---|---|
| AETHER | yes | yes | Aether |
| STRIA | yes | yes | Stria |
| HOLOCRON | yes | yes | Holocron |
| MIDDEN | yes | yes | Midden |
| UNA | yes | yes | vessel `una` (panes = handler cores) |
| LUMEN | yes | yes | vessel `lumen` (Vein's face; `vein_ring3`) |
| VUG (+VUGX/VUGC/VUGK variants) | yes | yes | Vug |
| PREFS, NET, DIAG, BIG | yes | no aarch64 recipe found | probes/fixtures (owner `unverified`) |
| STAT, PULSE | yes | yes | instruments |
| blob / elf / blob-x86 | flat test blobs | | fixtures |

## 5. Ruled, unbuilt / open
- FPU/SSE context for UnaOS ring 3: NOT RULED. TEMPLE measured fixed point 5594 ms vs the 3000 ms bar, f32 2127 ms (host, commit `d92a36b7`): R128's game tier needs it. Open question for Peter.
- `layout.md` §5.2 says `sys_open` takes an 8.3 leaf; `SYS_PATH_READ/WRITE` (59/60) exist at `una-abi:1294-1298`. Whether EL0 has a path namespace for every verb is `unverified`.

## 6. Read further
- `docs/dev/OS/02_KERNEL_CORE/userspace.md`, `ipc_design.md`.
- `docs/dev/USERLAND/ARCHITECTURE.md`, `SURFACE.md`.
- `docs/dev/OS/03_COMPATIBILITY_BOX/syscall_translation.md` (the Linux-ABI box).
