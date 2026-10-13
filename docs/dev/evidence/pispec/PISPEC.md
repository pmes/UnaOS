# PISPEC — the Pi line's remaining reds after PIWM: which side is right, row by row (pi-ledger PI23)

Arc base `37adc602`, branch `exec-pi-pispec`. Source capture: `docs/dev/evidence/pikernel8test-1013/serial-pi.log`
(PIKERNEL8TEST2, QEMU raspi4b 640x480, evidence base `35af76a3e`); every `NNN:` below is a line of that file, read with
`LC_ALL=C awk 'index($0,"…")'`. Reading and compiling only (R107): no QEMU run was made for this record. The wires in
§9 are what the fold's `./arroyo kernel8-test` should show. The spec file is `unaos/scripts/specs/pi4-regression.spec`
(the brief's `unaos/specs/` path does not exist; this is the one file `kernel8-test` replays).

| row | verdict | cause, with the citing commit | made here |
|---|---|---|---|
| PI22 cause C | code moved | RASTWIN's 3D row opened from the Pi handoff line over the live cascade (`28bcd764` parks it) | yes — `main.rs` RASTWIN-ARMED |
| stage never published | the Pi never calls `users::service` | the three call sites (`main.rs:1214/1704/5965`) are the usbdebug loop, the shared BSP loop and the x86 `usb_pump`; the Pi baremetal BSP joins the scheduler at `main.rs:1494` and never reaches the shared loop (WITSWEEP's own note, `main.rs:4416-4418`) | no — `main.rs` beyond the RASTWIN move; exact change §3 |
| PI15 BGRUN-ST capacity | spec stale | WINDOWCAP3 M5 `9a9d7930` moved the aarch64 table to the heap and the line to `= {} rows (…; EL0 slots=heap, R90)`; the value is `video::wincap::proc_limit()`, memory-derived | yes — spec `:789` and `:1983` |
| PI16 knurl `box=` | spec stale | UIMETRICS M1 `c6ec712b`: `theme::CONTROL_BOX()` = `ui::base::CONTROL_BOX` (12) x the dpi scale; 12 at 640x480 | yes — spec `:2180` |
| PI17 WMCTRL `floor=` | spec stale | the same UIMETRICS move: floor = `2*FRAME + GAP + 3*CONTROL_BOX + 4*GAP + TITLE_CELL_W` = 10+12+36+48+9 = 115 at scale 1 (151 was the 24-px box) | yes — spec `:2302` |
| PI21 QUARRYVOL | fixture stale | `rows >= mounts` stopped holding when ROOTDISK (B390) nested `/volumes/UnaOS` and VOLUMES (B366 M2) withdrew `boot` from `/`; no mount is missing | yes — `video/quarry/live.rs` QUARRYVOL2 |
| PI14 BANDY-CODEC/2 | fixture stale | bit4's "invalid verb" probes use verb 9 (CODEC) and verb 7 (CODEC2); APPMENU `38d1c70b` assigned 7/8/9 to the menu verbs and merge9 `51e98fa7` made them valid in `verb_valid` | no — `bus.rs` is not on this arc's list (no `bandy*.rs` exists); exact change §6 |
| PI13 K3-mount | fixture or a leaking fixture (not named by the capture) | bit5 (`0x20`) = "the native root holds only the two fixtures and `acl-*` rows"; some other root entry exists at 2399 | no — `fs/unafs.rs` not on the list; probe + change §7 |
| PI12 SERWIT-2 | ring capacity, deterministic | the `tste` ring is `selftest::RING_CAP = 64`; 74 verdict lines precede the first `mirror_service` poll (64 absorbed + 10 dropped exactly) | no — `selftest.rs` not on the list; exact change §8 |

## 1. Cause C: RASTWIN moved to the armed seam (main.rs)

Per `docs/dev/evidence/piwm/PIWM.md` §4. Before: `main.rs:1412` (the Pi GUI handoff, the CONSWIN-PI detach line) called
`rast_demo_maybe()`; the 3D row (owner `0xffffff61`, `931:`) interleaved with the cascade's window fixtures and, parked
since `28bcd764`, sat over the desktop for the boot (`1153: [wc-iso] … forced=0x101018/false -> FAIL`, `0x101018` =
`rast_demo::RW_CLEAR`).

Mechanism (all line-neutral ahead of the file tail; `main.rs` 10652 -> 10692 lines (`wc -l`), the +40 at the tail):

- `main.rs:1412`: the call becomes `#[cfg(not(all(feature = "pirast", feature = "desktop_firmware")))] rast_demo_maybe();`
  — a Pi build without the desktop keeps the handoff call (and the knob-off inline no-op), so no other image moves.
- `main.rs:5598` (the SHELLWIN-PI mint arm in `render_pass`, inside `if desktop && … && desktop_firmware::armed()`):
  `let info = front_fb.info(); rast_demo_armed_once();` — the first render pass where `armed()` reads true.
- File tail: `rast_demo_armed_once` latches a static `AtomicBool` (the mint arm re-fires on a shell quit + relaunch; the
  demo must not) and spawns `rast_demo_maybe` on a `CPU_AUTO` kernel task (`"rastwin"`) with a sized 32 KiB stack —
  never inline, because 90 paced frames (~3 s) would stall the render pass, and never on the blanket 16 KiB, because the
  demo creates and composites a `wm` row from its own stack (the Orin measured `hw=23232` on the same chain,
  `TEGRA_JD2_STACK_SIZE`'s note). Every other `desktop_firmware` build gets an `#[inline(always)]` empty stub.
- Inside `rast_demo_maybe` (line-neutral): the pi arm builds its `Screen` on an EMPTY `FrameBuffer::new()` and prints
  the panel size from `front_fb.info()`. `rast_demo::run(_screen)` never reads the `Screen` (it renders into its own
  row since RASTWIN), and the old placement's stated invariant — the demo's full-panel back buffer (~9 MiB at
  1920x1200) never coexists with the render task's shadow on the 48 MiB metal heap — would otherwise break now that the
  demo runs after `render_service` exists. The block comment at `main.rs:7226-7233` is rewritten in place.
- The tegra call on `tegra_early_stop`'s terminus line (`main.rs:2742`) is unchanged.

## 2. Why the old placement was wrong on its own terms

`main.rs:7227-7230` (before this arc) said the handoff call ran "BEFORE any service task exists, so it can neither race
bring-up nor fight the compositor". The capture contradicts it: the u7-launch cascade was already compositing on the
APs (`600: [wm] alloc win=1`, `1004-1222` the fixture windows), and the demo's 90 frames ran across them (`931-1300`).

## 3. The stage that never published (STOPPED — `main.rs` beyond the RASTWIN move)

No `[users] stage-resolve`, `:: FIRSTBOOT:`, `[boot] step=` or `[users]` line appears in 6500 lines (0 hits for each
with `LC_ALL=C awk 'index($0,"[users]")'` etc.). The store resolution lives in `fs::users::service()`
(`fs/users.rs:1236`): load the store once the disk answers, then `resolve_tail` -> `stage_resolve` -> `stage_publish`.
Its callers, `grep -n "users::service()" unaos/crates/kernel/src/main.rs`:

- `:1214` — the `usbdebug` terminal loop;
- `:1704` — the shared main loop that ends `kernel_main` (virt GICv2, the x86 no-handoff path);
- `:2940` — the tegra cascade;
- `:5965` — the x86 `usb_pump`.

The Pi baremetal BSP joins the scheduler (`main.rs:1494`, `run_bsp(0)`) and never reaches `:1704` — the WITSWEEP note at
`main.rs:4416-4418` says so for `mirror_service`, which was moved into `pump_usb_into_gui` for exactly this reason. The
no-store fallback (`fs/users.rs:2990-2996`, `STAGE_WAIT_MS = 30_000`) only fires when something asks
`desktop_allowed()`/`boot::services_gate()`; nothing on the Pi path asks before the 60 s wall (no `[boot] services up`).
So `fs/users.rs` is not where it breaks: the Pi has no caller. **Exact change (not made):** `main.rs:4425`, inside
`pump_usb_into_gui` (aarch64 baremetal; runs from `usb_pump` on metal and the `input_service` poll-nap in QEMU), fold
`#[cfg(feature = "login")] unaos_kernel::fs::users::service();` onto the `unaos_kernel::serial_ring::mirror_service();`
line, before its `//` (line-neutral; IRQs unmasked, no lock held — the same contract that line states). This needs a run
before it lands: it turns on the whole R77 stage chain on the Pi (store load, `stage_publish`, `desktopbuild::build`,
the splash release and, with no users on the QEMU card, the create-user form), which is the SHARD-PRESS `painted=false`
decision PIWM §6 left open.

## 4. PI15 — BGRUN-ST capacity (spec stale)

`1881: :: BGRUN-ST: process table capacity = 12 rows (bg programs alive at once; EL0 slots=heap, R90) ::`. Emitter
`arch/aarch64/syscall.rs:10656`, value `proc_table_rows()` = `video::wincap::proc_limit()` (`:3968`), i.e.
`min(mem, windows)` from half the heap free at arming (`256: [wm] limit windows=37 procs=12 from=mem:47`). The text
changed at `9a9d7930` (WINDOWCAP3 M5, `git log -S "EL0 slots=heap, R90"`). Spec `:789` pinned `= 6 rows … EL0 slots 8`
and `:1983` FORBADE any value but 6 — a literal R90 retired ("no hardcoding"). Re-keyed: `:789` REQUIREs a non-zero
count with the heap text; `:1983` FORBIDs the fixed-pool text (`EL0 slots [0-9]`) coming back. BGRUN-SCAV still drives
`proc_table_rows() + 2` launches, so the leg's teeth do not depend on the number.

## 5. PI16 / PI17 — the UIMETRICS scale (spec stale)

`622: [knurl] … box=12 …` and `1289: :: WMCTRL: controls-declined — floor=115 under bw=114 …`. Since UIMETRICS M1
(`c6ec712b`, `git log -S "fn CONTROL_BOX"`) every furniture metric is `ui::px(base)` = `(base * s2 + 1) / 2`:
`CONTROL_BOX` 12, `GAP` 12, `FRAME` 5 at scale 1 (QEMU raspi4b 640x480), and the old 24-px box was the 2x value. The
WMCTRL floor is `2*BORDER + GAP + CTRL_RESERVE + TITLE_CELL_W` (`video/wm.rs:105,160,123`) = 10 + 12 + (36 + 48) + 9 =
115 — every term accounted for, the same expression the spec's own FONT-METRIC note derives 151 from at the 24-px box.
The spec is also replayed against metal captures (`~/pi-serial.log`, the bench Pi at 1920x1200, a different scale), so
both pins become scale-free (`box=[0-9]+`, `floor=[0-9]+`) and keep every other field pinned (the knurl hash, the
WMCTRL legs).

## 6. PI14 — BANDY-CODEC / CODEC2 bit 4 (STOPPED — `bus.rs` not on the list)

`2721: :: BANDY-CODEC: FAIL — w=0x2f/0x3f ::`, `2724: … CODEC2 … w=0x2f/0x3f`. Bit 4 (`0x10`) is the fail-closed leg in
both. CODEC's (`bus.rs:400-424`) sets `m[6] = 9` and requires `frame_parse` to answer `Malformed`; CODEC2's
(`bus.rs:642`) sets `f[6] = 7` ("verb 7 is not valid"). Since APPMENU M1 (`38d1c70b`) verbs 7/8/9 are
`BUS_VERB_MENU_PUBLISH/CLEAR/GET` (`una-abi/src/lib.rs:743-748`) and merge9 (`51e98fa7`) listed them in `verb_valid`
(`bus.rs:74`), so both probes now name VALID verbs and `frame_parse` accepts them. Code right, fixture stale.
**Exact change (not made):** `bus.rs:418` `m[6] = 9; // verb` -> `m[6] = 0; // verb 0 is never assigned`; and the CODEC2
`f[6] = 7;` -> `f[6] = 0;`. Verb 0 is in no `verb_valid` arm and below `BUS_VERB_REGISTER`/`FULFIL_MIN` under `busreg`,
so it stays invalid as verbs are added. Expected: `BANDY-CODEC: … PASS [w=0x3f]` and `BANDY-CODEC2: … PASS [w=0x3f]`.

## 7. PI13 — K3-mount bit 5 (STOPPED — `fs/unafs.rs` not on the list; the leaking entry is not named by the capture)

`2399: :: K3-mount: … ls/cat byte-verified FAIL [w=0x1df] ::`. `0x1ff ^ 0x1df = 0x20`: bit 5 (`fs/unafs.rs:1575-1601`),
"`ls /` of the native volume finds both fixtures and every other root entry starts `acl-`". So the volume mounts, both
files byte-verify (bits 6, 7) and one non-`acl-` entry sits in the native root at 2399. The arroyo image stages only
`K3HELLO.TXT` and `K3PAT.BIN` (`arroyo:12251-12253`). The capture never lists the raw native root after the shell
fixtures (`255`/`353` are VFS listings with the mount points synthesized), and every native-root write it shows before
2399 is cleaned on the wire (`296-327`: `/0`, `RELIC1-3`, `VFSR1`; `XVOL` scrubbed by `mt.unlink`). Not the boot log:
`fs::bootlog::divert` returns at once without `unafs` (`fs/bootlog.rs:70`), and the Pi line has no `unafs` feature.
**Probe (not made):** in bit 5's `else`, print the offenders once —
`serial_println!("[k3] root unexpected={:?}", entries.iter().filter(|e| …not a fixture, not acl-…).map(|e| &e.name).collect::<Vec<_>>())`
— and the fold's run names the fixture that leaks (or, if it is a by-design directory, the fixture's allow-list gains
it the way `acl-` was added).

## 8. PI12 — SERWIT-2 `evidence_lost=10` (STOPPED — `selftest.rs` not on the list)

`1314: :: SERWIT-2 tap tste: submitted=1289 absorbed=64 staged=0 dropped=10 …`. The `tste` tap is `selftest::capture`
(`selftest.rs:230`): a lock-free ring of `RING_CAP = 64` slots (`selftest.rs:68`, unchanged since `d094a2d6`) that
records every `-> PASS`/`-> FAIL` verdict line; `idx >= RING_CAP` is `drop_line()`. `LC_ALL=C awk` counts 74 non-TSTE
verdict lines up to 1317 (134 in the whole capture): 64 absorbed + 10 dropped, exactly. Deterministic, not load: the
image-24 Pi line simply prints more boot verdicts than the ring holds before SERWIT-2's one-shot verdict (emitted on
the first `mirror_service` poll, which on the Pi rides `pump_usb_into_gui`). Not a UART FIFO and not
`serial_ring::SLOTS`; the aarch64 arms of `serial_ring.rs` are not involved. **Exact change (not made):**
`selftest.rs:68` `const RING_CAP: usize = 64;` -> `256` (42 B a slot: ~10.5 KiB static; 134 verdicts in this capture,
and every arc adds verdicts). Expected: `SERWIT-2 tap tste: … dropped=0`, `:: SERWIT-2: mirror taps … -> PASS ::`, and no
`[mirror] tste: N line(s) dropped` (`1667:`).

## 9. Expected wires on the fold's `./arroyo kernel8-test`

| spec row | before | expected after this arc |
|---|---|---|
| FORBID `-> FAIL` (wc-iso) | `1153: [wc-iso] … forced=0x101018/false … -> FAIL` | `[wc-iso] close-iso … forced=…/true … -> PASS`; `[rastwin] spin open … owner=0xffffff61` and `:: RAST: BCM2711 mailbox panel 640x480 …` appear only AFTER `:: DESK: desktop armed` and the `[shellwin-pi] backdrop=crispy-scene` line |
| `:789` / `:1983` | REQUIRE miss, FORBID hit on `= 12 rows` | REQUIRE hits `BGRUN-ST: process table capacity = 12 rows (… EL0 slots=heap, R90)`; FORBID quiet |
| `:2180` | REQUIRE miss on `box=12` | hit (`box=12` at 640x480; the bench Pi's scale prints its own) |
| `:2302` | REQUIRE miss on `floor=115` | hit |
| default FORBIDs (QUARRYVOL) | `rows=4 … rows_ge_mounts=false … -> FAIL` | `:: QUARRYVOL: mounts=6 table=6 rows=4 … reach=6/6 … match=true -> PASS ::` |
| `:399`, `:470`/`:477`, `:265` | K3 `w=0x1df`, BANDY `w=0x2f`, SERWIT-2 `evidence_lost=10` | unchanged until §7, §6, §8 land |
| — | no `[users] stage-resolve` | unchanged until §3 lands |

## 10. Commits and compile legs (R107: compiled once per code commit; every QEMU/gate leg is the fold's)

| commit | row | compile |
|---|---|---|
| `ccb56889` | PI22 cause C — RASTWIN-ARMED (`main.rs`) | Pi kernel8-test line rc=0 |
| `1bbb08af` | PI15 — spec `:789`, `:1983` | spec-only |
| `688dbb03` | PI16 — spec `:2180` | spec-only |
| `e7cf274e` | PI17 — spec `:2302` | spec-only |
| `7033b823` | PI21 — QUARRYVOL2 (`video/quarry/live.rs`) | Pi kernel8-test line rc=0 |
| this record's commit | PI12/PI13/PI14 + the stage: diagnoses, exact changes | x86 kfeats24 line rc=0 at `7033b823` |

Pi line: `cd unaos/crates/kernel && cargo +nightly check --target ../../aarch64-unaos.json -Z build-std=core,compiler_builtins,alloc
-Z build-std-features=compiler-builtins-mem -Z json-target-spec --features "$(cat k8feats.txt)"` with PIWM's
`k8feats.txt` (`baremetal,skip_xhci,witness,tests-at-boot,census,usbnet,smp7,v3d,vugpar,wedge2,piusb,genet,nettest,genet,pirast,desktop_firmware,quarry,login,lumen,selfdiag,holocron,midden,una,sonde,aether,uvc`),
`unaos/target/user_blob.bin` built once the way `build_user_blob` does. x86 line: the same with
`--target ../../x86_64-unaos.json` and wave 32's `kfeats24.txt`.
