# NEUTRAL-TABLE — every board-named token in shared kernel files, with its subsystem name (orin 14, 2026-09-05)

Source tip: `6cc8de8c` (hw-jetson). Ruling: `docs/dev/RULINGS.md` R16 / `docs/dev/LEDGER.md` S6 —
identifiers and witness tokens in arch-neutral (shared) files are named by the owning SUBSYSTEM,
never by board/arc/vendor; inside `arch/<arch>/` a board name is fine. This table is the rename
input for GATE-NEUTRAL (rmbp 11 drafts the gate; the rename waits for it — nothing here is a rename).

## 0. Method (LEDGER S17: strip comments before counting)

- **Shared file set = by location**: every `.rs` under `unaos/crates/kernel/src/` that is not under
  `arch/aarch64/` or `arch/x86_64/` — 111 files (162 total, 51 under `arch/<arch>/`). `arch/mod.rs`
  is shared. A `target_arch` cfg inside a shared file does not make it arch-local (74 shared files
  carry one).
- **Comment stripping**: `/* … */` removed; lines whose first non-blank characters are `//` (so also
  `//!` and `///`) dropped; trailing `// …` removed, except a `//` inside a string literal. Then
  every match on the remaining text is a site. Scripts and raw outputs:
  `~/unaos-bench/scratch/orin14/neutral/{census.py,pass2.py,pass3.sh,pass4.py,conflicts.sh}` →
  `{census.txt,sites.txt,pass2.txt,pass3.txt}`. The prior tree (ac27b8d2) was extracted with
  `git archive` to `scratch/orin14/neutral/prior/` and censused with the same script (`prior.txt`).
- **Patterns**: witness `\[(orin|tegra|jetson|pi|rmbp|mbp|x86)[a-z0-9-]*` followed by `]`, space,
  `:` or a digit (so `[orinrender]` and `[tegra fs-mps]` both count; `[piusb37]` is one family per
  number, as rmbp counts it); identifiers `\b(orin|tegra|jetson|pi|rmbp|mbp)_[a-z0-9_]+\b`;
  upper `[A-Z_]*(ORIN|TEGRA|JETSON|PI|RMBP|MBP)[A-Z_0-9]*`; CamelCase `(Orin|Tegra|Jetson|Pi)…`;
  colon-prefix witness `::\s*(tegra|PIUSB|…)\s*:`; `feature = "<name>"` strings for the knob column.
- **Manual pass (false positives excluded)**: `[pi]` = an index variable (`plc[pi]`, `p[pi]`,
  `polys[pi]`, `was[pi]`, `SHARDS[pi]`: 14 sites in `drivers/xhci/mod.rs`, `shell.rs`, `splash.rs`;
  also 47+49 in `arch/*/syscall.rs`); `PI` = `rast::math::PI` (3 sites, `rast_demo.rs`); `rtpi` =
  priority inheritance, not the Pi (`rtpi.rs` header: "the PRIORITY-INHERITANCE witness");
  `PIN`/`PIO`/`PIPE*`/`PITCH`/`PING*`/`PID`/`PICKS`/`PINNED`/`ACPI`/`SPINS`/`EXPIRED`/`OCCUPIED`
  (word fragments); `:: pi:` (1 regex hit) is `install::pi::` in a path. `Tegra234` / `Jetson Orin
  Nano` / `Orin` inside banner prose (`":: UnaOS aarch64 kernel — Jetson Orin Nano (Tegra234) …"`,
  main.rs ×3; "the Orin card's only writer", block.rs; etc. — 18 prose sites) name the machine in a
  sentence, not a token; listed once in §6 and not proposed for rename.

## 1. Witness families (bracketed) — shared files

| token | kind | file | sites | owning subsystem | proposed name | mechanical? | conflicts |
|---|---|---|---:|---|---|---|---|
| `[orinfurn]` | witness-family | `main.rs` | 9 | desk (menubar furniture on the aarch64 desk seam, `tegra_desk_furn`) | `[deskfurn]` | sed-safe as a token; the emitting fn is under `#[cfg(feature = "orinfurn")]` (knob row §4, arroyo:1251) | none (`[deskfurn` 0 hits; `[deskseam]`/`[deskcascade]` are siblings, distinct) |
| `[orinrender]` | witness-family | `main.rs` | 8 | render (the aarch64 render-service pass, `orin_render_service`) | `[render]` or `[renderpass]` | sed-safe token; fn under `#[cfg(feature = "orinrender")]` (arroyo:1261) | `[render` 0 hits; `"render"` is a wm StealPick label + task name on x86 (main.rs:1442/1555, wm.rs:9000) — a `[render]` family would not collide with those strings but the x86 render service has no bracket family of its own yet; GATE-NEUTRAL picks one family for both |
| `[orinstkdepth]` | witness-family | `main.rs` | 2 | sched/stack (boot-core stack depth probe, `tegra_stk_anchor`) | `[stkdepth]` | sed-safe; emitted inside the `orinfurn` seam (same cfg) | none (`[stk` 0 hits) |
| `[orinface]` | witness-family | `video/fbcon.rs` | 4 | console (fbcon anti-aliased face arm) | `[conface]` | sed-safe; fn under `#[cfg(all(target_arch = "aarch64", feature = "orinface"))]` (arroyo:1317) | none (`CONFACE` 0, `[conface` 0) |
| `[orindefer]` | witness-family | `video/fbcon.rs` | 1 | console (deferred layout census under the interrupt mask) | `[condefer]` | sed-safe; 25 `feature = "orindefer"` cfg sites in fbcon.rs (arroyo:1350) | none |
| `[orinreboot]` | witness-family | `power.rs` | 5 | power | `[pwrreboot]` (rmbp's PWRNAME name) | sed-safe token; the 5 sites straddle `#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]` / `feature = "pi"` / x86 arms (power.rs:59–155) — the cfgs stay, only the token moves | none (`[pwr` 0 hits). NOTE `arch/aarch64/wdt_tegra.rs` also prints `[orinreboot]` ×2 — exempt (arch), but the family then splits across two names unless the wdt keeps `[orinwdt]` |
| `[orinshutoff]` | witness-family | `power.rs` | 5 | power | `[pwrshutoff]` | as above (power.rs:47–155) | none |
| `[pidesk]` | witness-family | `video/desktop_firmware.rs` ×14, `main.rs` ×1 | 15 | desk (firmware-panel desktop activate — pi lane; listed, proposal is "desk" only) | `[desk…]` — `[deskfw]` (0 hits) or fold into `[deskseam]` | sed-safe token; module is `feature = "desktop_firmware"` whose arroyo knob is `UNAOS_PIDESK` (arroyo:1000) — the knob name is board-named, the feature is not | `[desk]` 0 hits; siblings `[deskseam]`, `[deskcascade]` exist |
| `[piusb24]` | witness-family | `main.rs` | 2 | usb/hid (pointer report) — pi lane | `[usbhid…]` (list only) | sed-safe; emitted by `fn piusb24_pointer_witness` (main.rs:4676) under `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]` — the cfg is already neutral | `[usbhid` 0; `[hid…]` families exist (9 hits: `[hidkeys]`, `[hidled]`) |
| `[piusb26]` | witness-family | `main.rs` | 1 | usb (pump cadence) — pi lane | `[usb…]` (list only) | sed-safe | — |
| `[piusb25]` `[piusb34]` `[piusb35]` `[piusb36]` `[piusb37]` `[piusb38]` `[piusb39]` `[piusb40]` `[piusb41]` | witness-family ×9 | `drivers/xhci/mod.rs` | 3+1+1+13+21+18+5+4+9 = **75** | usb (BOT/storage ladder in the shared xHCI driver) — **rmbp's lane; list only, "usb"** | `[usb NN]` / `[usbstor NN]` | sed-safe per family; the emitting fns are `feature = "piusb"`-free (xhci gates on `baremetal`/`tegra`) — verify per site before sed | `[usbstor` 0 hits. Matches rmbp's count of 75 |
| `[tegra fs-mps]` | witness-family | `drivers/xhci/mod.rs` | 7 | usb (full-speed MPS0 learning, `#[cfg(feature = "tegra")]` block at mod.rs:14322–14426) | `[xhci fs-mps]` or `[usb fs-mps]` | sed-safe token; the cfg is a seam (`tegra` knob, arroyo:758) — token rename does not need the cfg to move | none (`[fsmps`/`[usbfsmps` 0). **NEW vs prior census** (prior counted `[orin…]` only) |

Bracket totals (shared, stripped): **20 families / 134 sites** — `[orin…]` 7 / 34 (main.rs 3 / 19,
video/ 2 / 5, power.rs 2 / 10); `[pidesk]` 1 / 15; `[piusb NN]` 11 / 78 (main.rs 2 / 3, xhci 9 / 75);
`[tegra …]` 1 / 7.

## 2. Witness families (colon-prefix `:: NAME:` form) — shared files

Same category as §1 (a board/vendor name at the head of a witness line); the prior census did not
look for this form.

| token | kind | file | sites | owning subsystem | proposed name | mechanical? | conflicts |
|---|---|---|---:|---|---|---|---|
| `:: tegra:` (JM/JB/JD/XCARVE narrative) | witness-prefix | `main.rs` | 36 | boot (the Tegra terminus `tegra_early_stop` + JD2 console pump) | seam-decision, not a rename: these lines ARE the Tegra bring-up narrative; the honest fix is relocating `tegra_early_stop` (main.rs:2029) and the JD2 pump into `arch/aarch64/` where the name is exempt. If they stay in main.rs: `:: boot:` | seam | `:: boot:` 0 hits |
| `:: TEGRA-SD:` | witness-prefix | `drivers/block.rs` | 3 | block/sdmmc | `:: SDMMC:` | sed-safe; block is `#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]` | `"sdmmc"` is the feature name (49 cfg sites) — no witness prefix uses it |
| `:: TEGRA-UNAFS:` | witness-prefix | `main.rs` | 5 | fs/unafs (mount census on the card) | `:: UNAFS:` | sed-safe | none (`:: UNAFS:` 0 hits in fs/unafs.rs and main.rs) |
| `:: TEGRA-EL0:` | witness-prefix | `main.rs` | 2 | user/el0 (`tegra_el0_start_maybe`) | `:: EL0:` | sed-safe; fn under `feature = "tegra_el0"` | `[el0in]` family exists (distinct form) |
| `:: PIUSB:` | witness-prefix | `drivers/xhci/mod.rs` | 77 | usb — rmbp's lane, list only | `:: USB:` | sed-safe | — |
| `:: PI-RAST:` | witness-prefix | `main.rs` | 3 | rast (`pi_rast_demo_maybe`) — pi lane | `:: RAST:` | sed-safe | `:: RAST:` already used by the tegra twin (main.rs:6968/6974) — that is the target, not a conflict |
| `:: PI-DESK:` | witness-prefix | `video/desktop_firmware.rs` | 1 | desk — pi lane | `:: DESK:` | sed-safe | — |
| `:: PIINSTALL:` (`const PS`) + `INSTALL-PI` | witness-prefix | `install/pi.rs` | 1 + 1 | install — pi lane (file is board-named too: `install/pi.rs`) | `:: INSTALL:` | sed-safe; module is `feature = "piinstall"` | — |
| `ORIN-DESKFURN` / `ORIN-RENDER` (inside message text) | witness prose | `main.rs` | 2 | desk / render | follow the family rename | sed-safe | — |
| `UNAOS_ORINDESK/ORINCONWIN/ORINTENANT` (inside message text) | knob names quoted in a witness | `main.rs` | 2 (main.rs:7391, 8532) | desk | follow the knob rename (§4) | sed-safe | — |

Colon-prefix totals: **9 families / 131 sites** (127 of them are `:: tegra:` ×36 + `:: PIUSB:` ×77 + rest).

## 3. Identifiers — fn / const / static / field / enum variant / task name

### 3a. Defined in a shared file (the rename target)

| token | kind | file (def) | sites (all shared) | owning subsystem | proposed name | mechanical? | conflicts |
|---|---|---|---:|---|---|---|---|
| `orin_render_service` | fn | `main.rs:8222` | 2 | render | `render_pass_service` | sed-safe; body under `feature = "orinrender"` | **`fn render_service`** exists at main.rs:5273 (the x86 render service — rmbp's lane), so the bare name is taken; GATE-NEUTRAL decides merge vs. second name |
| `"orin-render"` | task-name | `main.rs:8200` | 1 | render/sched | `"render-service"` | sed-safe | `"render"` is the x86 task name (main.rs:1442/1555) — same merge question |
| `"orin-render:pass1"` / `":pass2"` | stk_probe label | `main.rs:8340` | 2 | render/sched | `"render:pass1"` … | sed-safe | none |
| `ORINRENDER_ARMED` | static | `main.rs:8107` | 2 | render | `RENDER_ARMED` | sed-safe | none (only the prefixed form exists) |
| `tegra_render_arm` | fn | `main.rs:8115` | 2 | render | `render_arm` | sed-safe | none |
| `tegra_desk_furn` | fn | `main.rs:7868` | 3 | desk | `desk_furn` | sed-safe | none |
| `ORINFURN_ENTERED` | static | `main.rs:7853` | 2 | desk | `DESKFURN_ENTERED` | sed-safe | none (`DESKFURN` 2 hits = the prose sites above) |
| `tegra_desk_arm` | fn | `main.rs:7305` | 2 | desk | `desk_arm` | sed-safe | none |
| `tegra_desk_cascade` | fn | `main.rs:8496` | 2 | desk | `desk_cascade` | sed-safe; under `feature = "deskcascade"` (already subsystem-named) | none |
| `TEGRADESK_ENTERED` / `TEGRADESK_CLICK_ROUTED` / `TEGRADESK_CASCADE_OK` | static ×3 | `main.rs:7254/7269/7291` | 2 + 4 + 4 | desk | `DESK_ENTERED` / `DESK_CLICK_ROUTED` / `DESK_CASCADE_OK` | sed-safe; under `feature = "tegradesk"` | none in shared; `arch/aarch64/display_tegra.rs` has `ORINCONWIN_CLICK_ROUTED` (exempt, distinct) |
| `tegra_conwin_live` | fn | `main.rs:7479, 7489` (cfg pair) | 5 | desk/console-window | `conwin_live` | sed-safe | none |
| `tegra_cascade_stk_pre` / `_post` | fn ×2 | `main.rs:8442/8470` | 2 + 2 | sched/stack | `cascade_stk_pre/_post` | sed-safe | none |
| `tegra_stk_anchor` | fn | `main.rs:8088` | 2 | sched/stack | `stk_anchor` | sed-safe; `#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]` | none |
| `ORINSTK_ANCHOR_SP` | static | `main.rs:8080` | 3 | sched/stack | `STK_ANCHOR_SP` | sed-safe | none |
| `tegra_darkwin_witness` | fn | `main.rs:7053` | 2 | video/boot (dark-window guard) | `darkwin_witness` | sed-safe | none |
| `tegra_early_stop` | fn | `main.rs:2029` | 3 | boot (the Tegra platform terminus) | **seam-decision**: relocate into `arch/aarch64/` (see §2 `:: tegra:`) rather than rename; else `platform_early_stop` | seam | `platform_early_stop`/`early_stop` 0 hits |
| `tegra_el0_start_maybe` | fn | `main.rs:7010, 7040` (cfg pair) | 3 | user/el0 | `el0_start_maybe` | sed-safe; `feature = "tegra_el0"` | none |
| `"tegra-el0-verdict"` | task-name | `main.rs:7024` | 1 | user/el0 | `"el0-verdict"` | sed-safe | none |
| `tegra_rast_demo_maybe` | fn | `main.rs:6964, 6983` (cfg pair) | 3 | rast | `rast_demo_maybe` | sed-safe | **`pi_rast_demo_maybe`** (below) wants the same name — the two are cfg-exclusive twins (`tegra` vs `pi`); GATE-NEUTRAL can merge them under one name with the cfg inside |
| `pi_rast_demo_maybe` | fn | `main.rs:7093, 7138` (cfg pair) | 3 | rast — pi lane | `rast_demo_maybe` | sed-safe | see above |
| `PI_RAST_FRAMES` | const | `main.rs:7131` | 3 | rast — pi lane | `RAST_FRAMES` | sed-safe | none |
| `PIUSB24_LAST_LOG_MS` / `PIUSB26_LAST_LOG_MS` / `PIUSB28_ARMED` | static ×3 | `main.rs:3357/3362/3397` | 3 + 3 + 2 | usb/hid — pi lane | `HIDPTR_LAST_LOG_MS` / `USBPUMP_LAST_LOG_MS` / `USB28_ARMED` (list only) | sed-safe | none |
| `orin_face_arm` | fn | `video/fbcon.rs:2665` | 2 | console | `con_face_arm` | sed-safe; `#[cfg(all(target_arch = "aarch64", feature = "orinface"))]` | none |
| `ORINFACE_ARMED` / `ORINFACE_RUNS` | static ×2 | `video/fbcon.rs:2661/2656` | 5 + 2 | console | `CONFACE_ARMED` / `CONFACE_RUNS` | sed-safe | none |
| `BlockHandle::TegraSd` / `BlockSource::TegraSd` | enum variant ×2 | `drivers/block.rs`, `fs/fat.rs` | 39 across `block.rs` 10, `fat.rs` 13, `unafs.rs` 7, `install/mod.rs` 2, `main.rs` 5, `wifi/firmware.rs` 2 | block/sdmmc | `SdMmc` (sibling of the existing x86 `Sdhc`) | sed-safe (`TegraSd` appears only as itself); variant under `#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]` | `SdMmc` 0 hits |
| `"tegra-sd"` (handle name string) | name string | `block.rs:2062`, `fat.rs:628`, `unafs.rs:587` | 3 | block/sdmmc | `"sdmmc"` | sed-safe | none as a display string |
| `b"TEGRA-SD"` (vendor field) | const bytes | `block.rs:1692` | 1 | block/sdmmc | `b"SDMMC   "` (8 bytes) | sed-safe (width-sensitive: 8-byte field) | — |
| `tegra_sd` | struct field | `block.rs:473` (+ locals 508/510) | 7 | block/sdmmc | `sdmmc` | sed-safe | none |
| `tegra_sd_info` | fn | `block.rs:1658` | 10 (`block.rs` 6, `fat.rs` 2, `unafs.rs` 1, `main.rs` 1) | block/sdmmc | `sdmmc_info` | sed-safe | none |
| `TEGRA_SD_BLOCK_DEVICE` / `TEGRA_SD_PUBLISHED` / `TEGRA_SD_WRITE_REFUSED` | static ×3 | `block.rs:1654/1666/1739` | 3 + 2 + 2 | block/sdmmc | `SDMMC_BLOCK_DEVICE` / `SDMMC_PUBLISHED` / `SDMMC_WRITE_REFUSED` | sed-safe | none |
| `TEGRA_SD_VETO` | const | `fs/fat.rs:698` | 2 | fs/fat (write veto on the card) | `SDMMC_VETO` | sed-safe | none |
| `TEGRA_DRAM_TOP` | const | `vugras.rs:61` | 2 | mem/vugras (heap sweep span) | `DRAM_TOP` | sed-safe; `feature = "tegra"` | none in shared (`arch/aarch64/xusb_tegra.rs::JBXC_DRAM_TOP` is exempt and distinct) |
| `pi_ls` / `pi_ls_witness` | fn ×2 | `shell.rs:1706/1761` | 2 + 2 | shell (ls) — pi lane | `ls_cmd` / `ls_witness` | sed-safe | none |
| `pi_usb_ls_witness` | fn | `shell.rs:1773` | 2 (`fat.rs`, `shell.rs`) | shell/usb — pi lane | `usb_ls_witness` | sed-safe | none |
| `PIUSB36_STATIC_BUF` | static | `drivers/xhci/mod.rs:39` | 2 | usb — rmbp's lane, list only | `USB36_STATIC_BUF` | sed-safe | — |
| `piusb24_pointer_witness` | fn | `main.rs:4676` | 4 | usb/hid — pi lane | `hid_pointer_witness` (list only) | sed-safe; `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]` | `fn hid_` 4 hits — check the names before choosing |
| `piusb27_service` / `piusb27_mount_witness` / `piusb27_walk_subtree` | fn ×3 | `fs/fat.rs:4337/4350/4409` | 4 (main.rs 3, fat.rs 1) + 2 + 3 | fs/usb (USB-storage mount census) — pi lane | `usbmount_service` / `usbmount_witness` / `usbmount_walk_subtree` (list only) | sed-safe | none |
| `piusb36_delay_ms` / `piusb36_matrix` / `piusb36_read10_two_trb` / `piusb36_report` / `piusb37_dump16` / `piusb37_matrix` / `piusb38_matrix` / `piusb39_witness` | fn ×8 | `drivers/xhci/mod.rs:8778/8795/8683/8760/8896/8915/9125/14730` | 2+2+2+6+5+2+2+4 = 25 | usb — rmbp's lane, list only | `usb36_…` … (list only) | sed-safe | — |

(These twelve glued-prefix `piusbNN_…` fns were found by a second identifier pass — the plain
`\b(pi)_` pattern cannot see them; `pass4.py` in scratch is the pattern that does.)

3a totals: **51 shared-defined symbols** (fn 31, static/const 16, field 1, enum variant 2 [+ 1 name
string, 1 vendor bytes], task-name 2, probe-label 1) — orin/tegra: 31; pi (pi + rmbp lanes): 20.

### 3b. Defined under `arch/aarch64/` (exempt home) but referenced from a shared file

The symbol's home is exempt; the shared-file reference carries the board name only through the
`unaos_kernel::arch::display_tegra::…` path, which is itself the arch seam. No rename is proposed
here — if GATE-NEUTRAL wants the reference neutral too, the fix is at the definition (arch lane).

| token | def | shared refs | subsystem |
|---|---|---:|---|
| `orin_wm1` | `arch/aarch64/display_tegra.rs:377` | main.rs 1 | desk |
| `orin_click` / `orin_click_census` | `display_tegra.rs:1309/1441` | main.rs 2 + 2 | desk/input |
| `orin_conwin` | `display_tegra.rs:2572` | main.rs 1 | desk |
| `orin_tenant_arm` / `orin_tenant_census` | `display_tegra.rs:2858/3124` | main.rs 1 + 1 | desk |
| `orin_ladder_arm` / `orin_ladder_census` | `display_tegra.rs:4129/4238` | main.rs 1 + 1 | desk |
| `orin_rast_census` / `orin_rast_console_owns` / `orin_rast_glass_post` | `display_tegra.rs:4961/5030/4925` | main.rs 3 + 2 + 1 | rast |
| `tegra_el0_verdict` | `arch/aarch64/syscall.rs:23791` | main.rs 1 | user/el0 |
| `tegra_sd_read_block_512` / `tegra_sd_read_blocks_512` | `arch/aarch64/sdmmc_tegra.rs:821/844` | block.rs 1 + 1 | block/sdmmc |

3b totals: **15 arch-homed symbols / 20 shared references**.

## 4. Feature knobs (Cargo.toml feature names + arroyo env mapping) — `knob`, not symbols

A feature NAME is a knob. Listed so the rename can be scoped; each row is a seam-decision by
definition (every cfg site is a compile-time gate; the arroyo line is the operator's name for it).

| feature | Cargo.toml def | arroyo mapping | shared cfg sites (stripped) | subsystem | note |
|---|---|---|---:|---|---|
| `orindesk` | `orindesk = []` | `UNAOS_ORINDESK` arroyo:830 | main.rs 3 | desk | knob |
| `orinclick` | `["tegra_el0"]` | `UNAOS_ORINCLICK` :909 | main.rs 7 | desk/input | knob |
| `orinconwin` | `["desktop_firmware","tegra_el0"]` | `UNAOS_ORINCONWIN` :1086 | main.rs 6 | desk | knob |
| `orintenant` | `["tegra_el0"]` | `UNAOS_ORINTENANT` :1126 | main.rs 4 | desk | knob |
| `orinladder` | `["orinconwin","orinclick","orindesk"]` | `UNAOS_ORINLADDER` :1207, :1281 | main.rs 2 | desk | knob |
| `orinfurn` | `["desktop_firmware","orinclick"]` | `UNAOS_ORINFURN` :1251 | main.rs 8 | desk | knob |
| `orinrender` | `["desktop_firmware","tegra_el0"]` | `UNAOS_ORINRENDER` :1261 | main.rs 5 | render | knob |
| `orinrx` | `orinrx = []` | `UNAOS_ORINRX` :1270 | main.rs 6 | serial (UART RX drain) | knob |
| `orinwdt` | `["tegra"]` | `UNAOS_ORINWDT` :976 | main.rs 2 | power/wdt | knob (the `[orinwdt]` witness itself lives in arch — exempt) |
| `orinvpar` | `["desktop_firmware"]` | `UNAOS_ORINVPAR` :931 | video/screen.rs 11 | video (present parity) | knob |
| `orinface` | `orinface = []` | `UNAOS_ORINFACE` :1317 | video/fbcon.rs 5 | console | knob |
| `orindefer` | `orindefer = []` | `UNAOS_ORINDEFER` :1350 | video/fbcon.rs 25 | console | knob |
| `orinel1ap` / `orininput` | `["tegrasmp","tegra_el0"]` / `["tegra_el0"]` | :1163 / :1181 | 0 shared (arch-only) | smp / input | knob, no shared site |
| `tegra` | `tegra = []` | `UNAOS_TEGRA` :758 (+ esp-jetson forces it, :4616) | 107 (main.rs 23, block.rs 21, rast_demo.rs 22, fat.rs 12, vugras.rs 12, unafs.rs 7, xhci 7, install 2, wifi 1) | platform | knob — the platform selector itself; the name IS the platform, like `pi` |
| `tegrasmp` | `["tegra"]` | `UNAOS_TEGRASMP` :963/:964 | main.rs 1 | smp | knob |
| `tegra_el0` | `["tegra","aarch64_el0"]` | `UNAOS_TEGRA_EL0` :954 | 13 (main.rs 3, shell.rs 2, dock.rs 4, quarry/live.rs 4) | user/el0 | knob |
| `tegradesk` | `["desktop_firmware","tegra_el0"]` | `UNAOS_TEGRADESK` :1049 | main.rs 6 | desk | knob |
| `pi` | `pi = []` | `UNAOS_PI` :201 | 13 (main.rs 6, power.rs 7) | platform | knob — platform selector (pi lane) |
| `piusb` | `["baremetal"]` | (kernel8 path) | main.rs 2 | usb | knob (pi lane) |
| `pirast` | `["rast"]` | — | main.rs 3 | rast | knob (pi lane) |
| `piinstall` / `_arm` / `_confirm` | chain on `baremetal` | `UNAOS_PIINSTALL*` (quoted in install/pi.rs:174/199/286) | 6 / 5 / 4 | install | knob (pi lane) |
| `desktop_firmware` | `desktop_firmware = []` | **`UNAOS_PIDESK`** :1000 | 127 | desk | feature is neutral; the ENV KNOB is board-named |
| `deskcascade` | `["desktop_firmware","tegra_el0"]` | `UNAOS_DESKCASCADE` :1280 | main.rs 8 | desk | already subsystem-named (listed per brief) |
| `rtpi` | `rtpi = []` | `UNAOS_RTPI` :725 | 4 | sched (priority inheritance) | NOT board-named — false positive, listed to close it |

Knob totals: **26 board-named feature names** (14 `orin*`, 4 `tegra*`, 6 `pi*` incl. the 3-step
install chain, + `UNAOS_PIDESK` as a board-named env name over a neutral feature) plus 2 non-board
rows for the record.

## 5. Exempt — board-named tokens correctly inside `arch/<arch>/` (do not re-audit)

Bracket witness families under `arch/aarch64/` + `arch/x86_64/`, comment-stripped, `[pi]` index
variable excluded (47 + 49 sites in the two `syscall.rs`):

| file | families (sites) |
|---|---|
| `arch/aarch64/display_tegra.rs` | `[orinwm1]` 7, `[orinchrome]` 4, `[orinclick]` 3, `[orinconwin]` 8, `[orintenant]` 10, `[oringlass]` 5, `[orindock]` 5, `[orinrast]` 4 |
| `arch/aarch64/selfup_tegra.rs` | `[orinselfup]` 16 |
| `arch/aarch64/wdt_tegra.rs` | `[orinreboot]` 2 (see §1 note — same family name as power.rs) |
| `arch/aarch64/timer.rs` / `sched.rs` / `xusb_tegra.rs` | `[orinbsptick]` 2 / `[orinbsprun]` 1 / `[orininput]` 1 |
| `arch/aarch64/piusb.rs` | `[piusb40]` 3, `[piusb32]` 3, `[piusb43]` 14 |
| `arch/aarch64/genet.rs` | `[pigenet4]` 3, `[piusb27]` 7 |

Exempt totals: **17 families / 98 sites** (19 / 194 before removing `[pi]`), plus **33 distinct
`orin_`/`tegra_` identifiers / 72 references** homed in `arch/aarch64/`. Seven `[orin…]` families
that the prior census attributed to main.rs+video (`[orinchrome]`, `[orinclick]`, `[orinconwin]`,
`[orindock]`, `[oringlass]`, `[orintenant]`, `[orinwm1]`) appear there ONLY in comments; their code
homes are in this table.

## 6. Prose mentions (not tokens; no rename)

`Jetson Orin Nano (Tegra234)` banner ×3 + `Tegra234` ×1 (main.rs), `Orin` in sentences ×10
(main.rs 7, block.rs 1, fat.rs 1, fbcon.rs 1), `Tegra Device-nGnRE` ×1, `rmbp lane's` / `rmbp1-boot1` /
`pi lane's` ×4 (power.rs, ehci, wifi). 18 sites total; they describe the machine, they do not name a
symbol.

## 7. Totals and the diff against the prior census (ac27b8d2)

| kind | this table (6cc8de8c, stripped) | prior census (ac27b8d2, as recorded) | what changed |
|---|---:|---:|---|
| `[orin…]` witness families in main.rs+video/ | 5 / 24 sites (7 / 34 with power.rs) | 12 / 43 | **the prior count was a naive grep (comments included)** — re-run naively on ac27b8d2 it reproduces 12 / 43 exactly, and on 6cc8de8c gives 12 / 45. Stripped, ac27b8d2 has the same 7 / 34 as today: **no family gained or lost in code**; +1 `[orinrender]` site and +1 `[orinclick]` mention exist only in comments |
| other bracket families in shared files | `[pidesk]` 1/15, `[piusb NN]` 11/78, `[tegra fs-mps]` 1/7 | `PIUSB` family noted; rmbp: 75 in xhci | `[pidesk]` +1 site (main.rs:8557, new in `deskcascade`); `[tegra fs-mps]` is NEW to the census; xhci 75 confirmed |
| colon-prefix families | 9 / 131 | not counted | NEW category (`:: tegra:` ×36 is the largest single exposure in main.rs) |
| `orin_`/`tegra_` symbols in main.rs (naive, distinct) | 27 | 24 (recorded as "13 fns/consts") | +3: `tegra_cascade_stk_pre`, `tegra_cascade_stk_post`, `tegra_desk_cascade` (orin 13's `deskcascade` arc); stripped, main.rs references 12 `orin_*` (11 arch-homed) + 14 `tegra_*` |
| shared-defined symbols needing a subsystem name | 51 (31 orin/tegra, 20 pi) | — | first full count |
| arch-homed symbols referenced from shared | 15 / 20 refs | — | first count |
| board-named feature knobs | 26 (+`UNAOS_PIDESK`) | — | first count |
| exempt (arch) | 17 families / 98 sites; 33 idents / 72 refs | — | first count |
| mechanical vs seam | every §1–§3a row is sed-safe as a token (its cfg stays where it is) except two seam-decisions (`tegra_early_stop` and its `:: tegra:` narrative → relocate into `arch/aarch64/`, not rename); three rows carry a merge question for the gate (`render_service` ×2 names, the `rast_demo_maybe` twins, `[orinreboot]` shared with `wdt_tegra.rs`) | — | — |

Sum for the S6 row: **29 witness families (20 bracket incl. the 11 `[piusb NN]` + 9 colon-prefix)
in 7 shared files; 265 witness sites (134 bracket + 131 colon-prefix); 51 shared-defined symbols +
15 arch-homed symbols referenced from shared files; 26 board-named knobs.**
