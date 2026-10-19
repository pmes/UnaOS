# BP-ring3 — programs on UnaOS: ABI, bus, ELF, slots, windows, float (read at 1719fe54)

## 1. What it is
- A ring-3 program is a static ELF from a `user-*` crate in `unaos/crates/`, staged as `/apps/<NAME>.ELF` on the root volume and launched by its bare name (R82, R94).
- The ABI crosses through `una-abi` (70 `SYS_*` numbers 1..76; packed input words; bus verb numbers) (`unaos/crates/una-abi/src/lib.rs`). Each arch's `syscall.rs` owns only the user copy and the trap.
- The bus (`unaos/crates/kernel/src/bus.rs`) is the on-UnaOS SMessage v1 codec: a fixed 52-byte header plus typed payloads over `SYS_MSEND`/`SYS_MRECV` (19/20).
- A handler that lives on UnaOS either IS a ring-3 program (Holocron, Stria, Aether, …) or is FULFILLED by the kernel through its bus verbs (Principia's prefs).
- UnaOS ring-3 code on x86 has the x87/SSE unit (hardware float, SSE/SSE2, no AVX) with a per-task FXSAVE context (B666). aarch64 ring-3 targets carry `+neon`.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-ring3-1 | `una-abi` holds only what crosses the boundary (numbers, flags, packed layouts, shared magics), is `const`-only with no dependencies, and a number means the same verb on every arch (unimplemented = `-ENOSYS`). | `unaos/crates/una-abi/src/lib.rs:4-31` | compile; `gates` row `verbs` (`unaos/scripts/verb-roots.sh`) for shell verbs |
| BP-ring3-2 | Per-arch differences in the ABI are a documented `cfg` in una-abi itself (e.g. `GETINFO_TICK_HZ` 1000 x86 / 250 aarch64; callers convert with `getinfo_ticks_to_ms`). | `unaos/crates/una-abi/src/lib.rs:36-50` | compile |
| BP-ring3-3 | The bus wire is UnaOS-native (R17), frozen by self-authored goldens; the largest allocation a hostile frame can force is `BUS_FRAME_MAX` = 4148 bytes. Both arches run the codec KATs every boot. | `unaos/crates/kernel/src/bus.rs:1-30`; `unaos/crates/kernel/src/lib.rs:244` | `bus_codec_selftest`, `bus_codec2_selftest` |
| BP-ring3-4 | Bus verb ranges: 1-25 fixed verbs (fs, menus, attrs, prefs 16-19, dialog/sheet/toast 20-22, PrefDeclare 23, DropGet 24, A11yTree 25); 127 REGISTER; ≥128 the registrable/fulfilled range (only under `busreg`); Holocron 144-151. | `unaos/crates/una-abi/src/lib.rs:645-909` `unaos/crates/una-abi/src/lib.rs:1732-1733` `unaos/crates/una-abi/src/lib.rs:1839-1843` `unaos/crates/una-abi/src/lib.rs:1941` `unaos/crates/una-abi/src/lib.rs:2384`; `unaos/crates/kernel/src/bus.rs:88` | compile |
| BP-ring3-5 | The kernel's own desktop speaks to Principia's store as a BUS CLIENT through the same `frame_parse`/`request_validate` a ring-3 frame passes. | `unaos/crates/kernel/src/prefs_client.rs:1-12` | `gates` row `prefs` |
| BP-ring3-6 | Ring 3 is not capped by a pool: per-slot sidecars are heap-grown `SlotVec`s; the only bound is the slot type and `wincap` (BP-video-7). | `unaos/crates/kernel/src/procslot.rs:1-14`; R90 | `[wm] limit … procs=` line |
| BP-ring3-7 | The ELF segment plan is ONE shared core both loaders use (`elf_core`); the dynamic loader core is `ldso_core`. | `Cargo.toml:35,37` (members comments); `unaos/crates/kernel/Cargo.toml:3941` `unaos/crates/kernel/Cargo.toml:3947` | compile |
| BP-ring3-8 | A program launched in a session runs AS the session user (`user:<name>`), stamped by the loader into the slot principal. | `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16892` `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16999`; `unaos/crates/kernel/src/arch/aarch64/syscall.rs:7767` | `tests ring3abi` / `SYS_WHOAMI` (61) (`unaos/crates/kernel/src/ring3abi.rs:1-12`) |
| BP-ring3-9 | Window ABI: `SYS_WIN_CREATE/PRESENT/MOVE/CLOSE/PRESENT_ROWS` (29-33); present returns 0 for every success on both arches; surface ≤ 288x288 on x86. | `unaos/crates/una-abi/src/lib.rs:230-266`; `unaos/crates/kernel/src/arch/x86_64/memory.rs:537-539` | compile |
| BP-ring3-10 | A UnaOS-native face is a surface, an input stream and a resize (R108); it reads its colours through palette_core roles (BP-video-1). | R108 (RULINGS); `unaos/libs/sys/palette_core/src/lib.rs:8-13` | `gates` row `appearance` (the four `theme.rs`) |
| BP-ring3-11 | x86 ring-3 targets are hardware-float (`+sse,+sse2`, AVX off, no softfloat ABI); the kernel target stays `+soft-float`, so the SYSCALL path saves nothing. Every ring-3 task (process or `SYS_THREAD_SPAWN` thread) owns a 512-byte 16-aligned FXSAVE image (`Task::fpu`): EAGER — the dispatch hook sets CR4.OSFXSR|OSXMMEXCPT (once per core) and `fxrstor64`s it, the switch-back hook `fxsave64`s it and scrubs to the initial image; no CR0.TS/#NM. A Linux-ABI task keeps its LINUXABI3 slot through the same two hooks (one mechanism, `sched::fpu`). A UnaOS program is entered at rsp ≡ 8 (mod 16), as if called. Changed by seat: wave36 FPU arc (rmbp-ledger B666; INDEX open question 1). | `unaos/x86_64-unknown-none.json:15`; `unaos/x86_64-unaos.json:8`; `unaos/crates/kernel/src/arch/x86_64/sched.rs:8130` `unaos/crates/kernel/src/arch/x86_64/sched.rs:8174-8211` `unaos/crates/kernel/src/arch/x86_64/sched.rs:6399` `unaos/crates/kernel/src/arch/x86_64/sched.rs:6488` `unaos/crates/kernel/src/arch/x86_64/sched.rs:2585` | `tests fpu` (`:: FPU: osfxsr=1 ctx_bytes=512 tasks=2 switches=<n> xmm_survived=ok mode=eager -> PASS ::`, BIG.ELF `big fpu`) |
| BP-ring3-12 | Entropy and name lookup for ring 3 have ONE kernel body each (`SYS_GETRANDOM` 56 → the DRBG; `SYS_RESOLVE` 57 → the arch's resolver). | `unaos/crates/kernel/src/netring3.rs:1-20` | `tests net` |
| BP-ring3-13 | A program declares console vs windowed in a PT_NOTE read by `midden_core::app_note_flags`; no `run`/`bg` to type (R82). | `unaos/crates/user-big/src/main.rs` (EXECNAME note); R82 | unverified gate |
| BP-ring3-14 | A ring-3 launch is timed exec→first frame (BP-bar-7, x86 `wc`): the launcher stamps the slot before the ELF load (`run_user_image_argv`, `spawn_user_image_bg_inner` — every `run`, `bg`, bare-name and `SYS_SPAWN_PATH` launch), and the slot's FIRST `wm::present` takes the stamp once and prints `[app] <name> exec->frame ms=<n>`; the sidecar is `procslot::launch`, outside WCSER. | `unaos/crates/kernel/src/procslot.rs:130` `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16892` `unaos/crates/kernel/src/arch/x86_64/syscall.rs:16999` `unaos/crates/kernel/src/video/wm.rs:1713` `unaos/crates/kernel/src/video/wm.rs:30583` | `tests applaunch` (`unaos/crates/kernel/src/tests.rs:640`) → `:: APPLAUNCH: app= n=5 p50_ms= p99_ms= bar_ms=400 -> PASS ::` |
| BP-ring3-15 | A ring-3 program ends through ONE path (x86): `sched::terminate(id, cause)` names the cause — a CPL-3 fault (`fault:<vector>:<rip>`, from `ring3_fault_kill`), an OOM (`oom:<kb>`, from `SYS_SBRK`), a kill (`reap_killed`), or the default `exit` (`SYS_EXIT` -> `exit()`) — and every end reaches ONE edge, the slot's 1->0 free in `user_space_release`, which reads the slot's wm rows and ELF-window frames, frees it (windows, handles, heap frames, slot) and prints `[task] terminate id= name= cause= freed_kb= windows=`, then one toast `<name> stopped: <cause>` for a fault or an OOM. A fault or an OOM dooms the address space (siblings follow). Each slot's heap has a ceiling armed at launch by the wincap rule (half the free kernel heap, never above the ELF window); a `SYS_SBRK` past it inside the window terminates the ASKER, never the desktop; the window cap still answers `-ENOMEM`. The kernel never halts on a CPL-3 fault. NEW by APPFAULT (rmbp-ledger B700, seat: wave36 arc appfault). | `unaos/crates/kernel/src/arch/x86_64/sched.rs:8677` `unaos/crates/kernel/src/arch/x86_64/sched.rs:8716` `unaos/crates/kernel/src/arch/x86_64/sched.rs:2896`; `unaos/crates/kernel/src/arch/x86_64/interrupts.rs:486`; `unaos/crates/kernel/src/arch/x86_64/memory.rs:4375` `unaos/crates/kernel/src/arch/x86_64/memory.rs:4730`; `unaos/crates/kernel/src/video/wm.rs:30856` | `tests appfault` (`unaos/crates/kernel/src/tests.rs:802`) → `:: APPFAULT: fault=1 oom=1 exit=1 leaked_kb=0 desktop=alive -> PASS ::` |

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
| ABIPROBE | committed fixture (`unaos/fixtures/ABIPROBE-v1.ELF`), never rebuilt by the image | no | the ABI gate (BP-ring3-15) |
| STAT, PULSE | yes | yes | instruments |
| blob / elf / blob-x86 | flat test blobs | | fixtures |

## 5. Ruled, unbuilt / open
- FPU/SSE context for UnaOS ring 3: built (B666, BP-ring3-11), unflown. TEMPLE measured fixed point 5594 ms vs the 3000 ms bar, f32 2127 ms (host, commit `d92a36b7`); the on-UnaOS f32 number is owed to a flight.
- `layout.md` §5.2 says `sys_open` takes an 8.3 leaf; `SYS_PATH_READ/WRITE` (59/60) exist at `una-abi:1294-1298`. Whether EL0 has a path namespace for every verb is `unverified`.

## 6. Read further
- `docs/dev/OS/02_KERNEL_CORE/userspace.md`, `ipc_design.md`.
- `docs/dev/USERLAND/ARCHITECTURE.md`, `SURFACE.md`.
- `docs/dev/OS/03_COMPATIBILITY_BOX/syscall_translation.md` (the Linux-ABI box).
