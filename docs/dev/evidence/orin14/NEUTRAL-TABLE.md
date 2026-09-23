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
| `pidesk=` (field key) | census field inside other families' lines | `arch/aarch64/display_tegra.rs` ×5 (`[orinclick] arm` :1474, `[orintenant] arm` :2878, `[orintenant] census` :3180, `[orindock] arm` :4187, `[orinrast] census` :5009) | 5 | desk (the field reports `desktop_firmware::armed()`; the knob it names no longer exists on either board — pi 7, S28, 2026-09-06) | `desk=` | sed-safe on the literal `pidesk={}`; a different population from the `[pidesk]` bracket family above — the batch is scoped by this table's file list, so without this row the five keys survive the rename (measured by pi 7 at 8131cd2d, verified by orin 15 at a05c2c8e: `grep -rn 'pidesk={}' --include=*.rs unaos/` → display_tegra.rs only) | none; renames with the `[orin…]` families that carry it (same file, same batch) |
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


## Addendum (PANEL4 L1, 2026-09-06)
| token | kind | file | sites | subsystem | proposed name | mechanical? | note |
|---|---|---|---|---|---|---|---|
| `orin_desk_scene_up` | fn | `main.rs` (orinrender region, DESKSCENE `8cbfaadf`) | 3 (:8121 def, :8149, :8281) | render/desk | `render_desk_scene_up` | yes (sed-safe, cfg unchanged) | rename with the S6 batch once GATE-NEUTRAL exists; not renamed alone (memory: no lone rename commit) |
| `[orinrender]` | witness family | `main.rs` | +1 site (:8285 census, DESKSCENE) | render | `[render]` (with the family) | yes | already tabled; count updated |

## Addendum (NEUTRAL M1, 2026-09-22) — the bracket families are RENAMED, and four corrections to this table

Base `4d8d047d` (branch `exec-rmbp-neutral`; the branch tip is the sha — the seat fills it at the
fold). Census re-derived at that base before any edit, per §0 and LAWS §5 ("a measurement is scoped
to its base exactly as a claim is scoped to its check"): script and raw output in
`docs/dev/evidence/rmbp-0915/neutral/{census.py,census-BEFORE-4d8d047d.txt,census-AFTER-m1.txt}`.

### M1 — DONE (bracket witness families in shared files)

| old | new | sites moved | files |
|---|---|---:|---|
| `[orinrender]` | `[render]` | 9 | `main.rs` |
| `[orinfurn]` | `[deskfurn]` | 9 | `main.rs` |
| `[orinstkdepth]` | `[stkdepth]` | 2 | `main.rs` |
| `[orinface]` | `[conface]` | 4 | `video/fbcon.rs` |
| `[orindefer]` | `[condefer]` | 1 | `video/fbcon.rs` |
| `[pidesk]` | `[deskfw]` | 15 | `video/desktop_firmware.rs`, `main.rs` |
| `[piusb24 25 26 34 35 36 37 38 39 40 41]` | `[usb NN]` | 78 | `drivers/xhci/mod.rs` 75, `main.rs` 3, `video/wm.rs` 1 |
| `[tegra fs-mps]` | `[usb fs-mps]` | 7 | `drivers/xhci/mod.rs` |

The rename is a pure token substitution and is MEASURED as one: 185 changed lines under
`crates/kernel/src/` and 88 outside it, across the 22 files the rename edits — **273 total,
insertions == deletions, and every single line is reproduced exactly by applying the substitution
table (plain AND regex-escaped spellings) to the old line** (`m1-substitution-proof.txt` in this
arc's evidence dir). No emission site, no cadence, no cfg and no line number moved — the LAWS §5
rename contract ("a tag's TEXT changes, its emission site and cadence do not") asserted rather than
argued.

Feature NAMES are untouched (§4: a knob is not a symbol) — `orinrender`, `orinfurn`, `orinface`,
`orindefer`, `desktop_firmware`, `piusb` all still spell themselves in `Cargo.toml` and `arroyo`, and
`UNAOS_PIDESK` is still the env knob over the neutral `desktop_firmware` feature.

### OWED — sites a family lost to an excluded or exempt home (NOT half-renamed)

| token | where | why owed |
|---|---|---|
| `[piusb40]` ×8, `[piusb25]` ×1 | `arch/aarch64/piusb.rs` | exempt home (§5). **These two families now SPLIT across two names** — the same shape §1's NOTE predicted for `[orinreboot]`, and PWRNAME resolved by letting the wdt sibling keep `[orinwdt]`. Same call is owed here |
| `[piusb26]` ×1 | `drivers/ehci/mod.rs` | EHCIDARK holds `drivers/ehci/*` |
| `[orinrender]` ×2 (comments) | `arch/aarch64/sched.rs`, `arch/aarch64/serial.rs` | exempt home; both are prose naming the shared family and now name a token that no longer exists |
| `pidesk={}` field key ×5 | `arch/aarch64/display_tegra.rs` | exempt home — LEDGER S28's adjudicated row, unchanged |
| `[orinfurn] arm` ×1 (comment on `main.rs:88`), `[piusb26]` ×1 (comment on `main.rs:1432`) | `main.rs`, above `bootpace::record("gui")` | SPLASHGATE's region. Both are COMMENT-ONLY and provably cannot move the image (the code bytes and the line count on :88 are identical), but :88 is a `bootpace::record("entry")` line SPLASHGATE is likely to be editing, and the exclusion exists to prevent exactly that fold conflict. Reverted deliberately; two comments now name tokens that no longer exist |

### Four corrections to this table, each measured at `4d8d047d`

1. **§0's file set** — 122 shared `.rs` / 53 under `arch/` / 175 total, not 111 / 51 / 162.
2. **§0's comment-stripping method is wrong and loses sites.** It removes `/* … */` FIRST, so a `/*`
   inside a `//` line opens a bogus block comment and swallows the rest of the file. Re-run naively it
   drops `[tegra fs-mps]` and four `[piusbNN]` families entirely. A single-pass lexer that handles
   line comments, NESTED block comments and string/char/raw-string literals reproduces §1 exactly.
3. **§2's `:: TEGRA-SD:` is 5 sites, not 3** (`drivers/block.rs`).
4. **§5 says "17 families / 98 sites" but its own list enumerates 18.** Sites agree exactly (98);
   the family count is an arithmetic slip. At this base the arch set is 18 families / 98 sites, with
   `wdt_tegra.rs` carrying `[orinwdt]` ×2 rather than `[orinreboot]` ×2.

### Already landed since this table was cut, so no longer owed by S6

- **§1's `[orinreboot]` / `[orinshutoff]` rows (power.rs, 2 families / 10 sites) are DONE** —
  `bc10a469 power: PWRNAME` renamed them `[pwrreboot]` / `[pwrshutoff]` and gave the
  `arch/aarch64/wdt_tegra.rs` sibling `[orinwdt]`, exactly as §1's NOTE proposed. Zero
  `orinreboot|orinshutoff` anywhere in kernel source.
- **§3a's `rast_demo_maybe` twins are DONE** — ONEOS `3dab65ac` (recorded in LEDGER S6).
- That reconciles §1's bracket total to this base: 20 families / 134 sites − 2 / 10 (PWRNAME)
  + 1 site (`[orinrender]`, the PANEL4 addendum) = **18 families / 125 sites**, which is what the
  re-run census returns.

### Missing from this table entirely — drift since `6cc8de8c` (~60 sites, all M3 work)

A whole `tegra_shell_*` family in `main.rs`: `tegra_shell_pick` 6, `_pal` 5, `_present` 5,
`_window_open` 4, `_id` 3, `_mark` 3, `TegraShellWin` 13, `TEGRA_SHELL_PRESENTED` 2 (≈41 sites);
plus `tegra_boot_focus` 3 / `TEGRA_BOOT_FOCUS_DONE` 2, `tegra_quarry_seat` 2, `orin_drag_steer` 2,
`TEGRA_JD2_STACK_SIZE` 2, `tegra_opened` 4 (`video/login.rs`), and the SDHCWRITE-era
`tegra_sd_write*` / `NATIVE_TEGRA_SD_VETO`. New holder files not in §3a: `fs/bootdisk.rs`,
`install/partition.rs`, `video/login.rs`, `video/quarry/live.rs`.
**Every one of the 39 `tegra_shell_*`/`TegraShellWin` sites is outside EHCIDARK's and SPLASHGATE's
regions** (measured: all sit at `main.rs` 2899–10435; SPLASHGATE ends at the `bootpace::record("gui")`
at :1638, the three `service_ehci_hid()` call sites are :1181/:1674/:5911, and `x86_usb_pump` spans
:5880–6058), so the family is in scope for M3.

### §1's `[render]` conflict column is stale, and the tree already agrees with the proposal

`[render]` has ZERO live emission sites at this base. Its only occurrence was `main.rs:5404`, a
comment that ALREADY calls the family `[render] census` — the neutral name this table proposes. Three
further comments (`video/fbcon.rs`, `video/screen.rs`, `arch/aarch64/serial.rs`) spelled it
`[orinrender] census`. No shared file composes a witness tag through a `{}` hole (only
`arch/aarch64/ga10b_ignite.rs` does, for a different family), so an artifact grep on these tokens is
sound — the LAWS §5 runtime-composition hazard does not apply here.

## Addendum (NEUTRAL M2, 2026-09-22) — the colon families are RENAMED, and §2 was wrong in five places

Base `acd102d7` (branch `exec-rmbp-neutral2`; the branch tip is the sha — the seat fills it at the
fold). Census re-derived at that base before any edit, per §0 and LAWS §5 ("a measurement is scoped
to its base exactly as a claim is scoped to its check"): script and raw output in
`docs/dev/evidence/rmbp-0915/neutral/{census.py,census-at-acd102d7.txt,census-AFTER-m2.txt}`. The
substitution table IS the script — `m2-rename.py`, one file with `--apply`, `--prove` and `--count`.

### M2 — DONE (colon witness families in shared files)

Counts are RAW occurrences in the file (what a `sed` moves), not the comment-stripped census figure;
where they differ the census number is in parentheses.

| old | new | sites moved | files |
|---|---|---:|---|
| `:: PIUSB:` | `:: USB:` | 79 (77) | `drivers/xhci/mod.rs` |
| `:: PINSTALL:` | `:: INSTALL:` | 63 | `install/partition.rs` |
| `:: PIINSTALL:` (`const PS`) | `:: INSTALL:` | 1 | `install/pi.rs` |
| `:: TEGRA-SD:` | `:: SDMMC:` | 5 | `drivers/block.rs` |
| `:: TEGRA-UNAFS:` | `:: UNAFS:` | 5 | `main.rs` |
| `:: PI-RAST:` | `:: RAST:` | 4 (3) | `main.rs` |
| `:: PI-DESK:` | `:: DESK:` | 2 (1) | `video/desktop_firmware.rs` 1, `main.rs` 1 |
| `:: piusb27:` | `:: usb27:` | 6 | `fs/fat.rs` |
| `:: piusb28:` | `:: usb28:` | 2 | `main.rs` |
| `ORIN-DESKFURN` / `ORIN-RENDER` **inside witness message text only** | `DESKFURN` / `RENDER` | 2 | `main.rs:8420`, `main.rs:8682` |

169 kernel sites + 100 sites in the pins outside it. **269 changed lines, insertions == deletions,
and every line reproduced exactly by applying the table to the old line** (`m2-substitution-proof.txt`).

### THE SECOND SPELLING IS NOT REGEX-ESCAPING HERE — IT IS THE DROPPED `:: ` PREFIX

M1's defect was that specs write a bracket witness regex-escaped (`\[orinstkdepth\]`) and a plain sed
left three rules behind. M2's families contain no regex metacharacter, so that spelling does not
exist — and the lesson still applied, in a new form. **`jetson-sync1.spec` writes three LIVE rules
with the `:: ` prefix dropped**, and two subsystem docs hand the operator the same short form:

| file | rule | why a plain sed misses it silently |
|---|---|---|
| `jetson-sync1.spec:409` | `FORBID TEGRA-SD: REFUSED to publish` | a FORBID that can no longer match still reads green |
| `jetson-sync1.spec:619` | `FORBID TEGRA-UNAFS: mount on TegraSd FAILED` | same |
| `jetson-sync1.spec:593` | `PENDING TEGRA-UNAFS: native unafs volume MOUNTED…` | a PENDING was never required; it moves `pending N/M` only |
| `partition-install.md:247,397` | `awk 'index($0,"PINSTALL:")'` | an operator command, not a gate — it just returns nothing |
| `SITTING-1.md:99,239` | `awk 'index($0,"PINSTALL")'`, `grep -a -o -F 'PINSTALL: census part='` | the second is an ARTIFACT CERTIFICATION command |
| `orin-specscore.py:292` | `` `TEGRA-SD: REFUSED to publish` `` | comment quoting the family |

`m2-rename.py`'s `PASS_B` enumerates these per file **with an expected count, and a miscount is
fatal** — the table refuses to write anything rather than move eight of ten spellings.

### THE RULE THAT DECIDED WHAT DOES *NOT* MOVE

The rename moves **wire tokens** — what a capture carries and what a gate greps. It does not move
**arc names** or **knob names** (LAWS §4). So `PI-DESK` the 2026-08-12 arc (`engine.md:13262`'s
section heading and eleven references to it), `PI-RAST` the arc, `# PI-DESK:`/`# PI-RAST:` as
knob-doc headings in `arroyo:10332,10347`, `# ORIN-DESKFURN:` at `arroyo:1970,6291` and
`Cargo.toml:3074`, and every `UNAOS_PIUSB` / `piinstall` / `sdmmc` / `pirast` stay. `ORIN-DESKFURN`
and `ORIN-RENDER` moved at exactly the two places they sit INSIDE a `serial_println!` argument.

### FOUR TARGETS WERE ALREADY OWNED — and all four are the SAME subsystem, so each is a merge

Checked before renaming, at this base. None is a foreign emitter, so no target was re-chosen:

| target | already emitted by | verdict |
|---|---|---|
| `:: INSTALL:` ×35 | `install/mod.rs` 32, `install/pi.rs` 2, `main.rs` 1 — the installer engine | MERGE. Count-safe: the engine spells its refusals `refusal — `, the renamed family spells them `refusal target=`, so `x86-install.spec:87 COUNT 5 :: INSTALL: refusal target=` cannot see the engine's. `:: INSTALL: census ` was 0 before the merge. |
| `:: SDMMC:` ×2 | `arch/aarch64/sdmmc_tegra.rs:74` `const PS` — the Tegra SD controller | MERGE with the layer below it. `:: SDMMC: WRITE admitted`, the string `arroyo`'s `BANNER_ARTIFACT_MAP` greps, is still emitted from exactly ONE site (`drivers/block.rs:2522`). |
| `:: RAST:` ×10 | `main.rs` 5, `rast_demo.rs` 4, `display_tegra.rs` 1 | MERGE — §2 already named this the target, not a conflict. |
| `:: USB:`, `:: UNAFS:`, `:: DESK:`, `:: usb27:`, `:: usb28:` | 0 hits | clean. |

### PARKED — and why, said out loud

| family | count at this tip | why it did not move |
|---|---:|---|
| `:: tegra:` | 42 raw / 36 live, all `main.rs` (plus 393 in `arch/`) | §2's own seam decision, and the brief's exclusion: these lines ARE the Tegra bring-up narrative around `tegra_early_stop`, and the honest fix relocates the terminus into `arch/aarch64/`. **It is now the ONLY colon family left naming a board in a shared file.** |
| `:: TEGRA-EL0:` | 2 `main.rs`, 2 `arch/aarch64/syscall.rs` | **§2's row is wrong twice over, and the rename would have made the tree worse.** (1) `:: EL0:` is NOT free — `arch/aarch64/syscall.rs` emits it 26 times for a DIFFERENT narrative (the EL0 threads/input self-tests), so by the collision rule this target is owned. (2) The board name here is DELIBERATE and documented: `arch/aarch64/mmu_tegra_el0.rs:115-117` routes six witnesses through `const EL0TAG = "TEGRA-EL0"` / `"VIRT-EL0"` precisely so a QEMU capture and an Orin capture are told apart by the same `awk` (`arch_arm64.md:10052-10069`). (3) `main.rs:7146`'s FAIL is the twin of `syscall.rs:23838`'s FAIL and `:23835`'s PASS; moving only the shared half splits one PASS/FAIL family across two spellings — the exact defect M1 recorded as OWED for `[piusb40]`. **STOP question for Peter / the seat: rename all four (arch included) to a free spelling such as `:: EL0HELLO:`, or leave the family board-tagged by design.** Note `jetson-sync1.spec:675` is RED against its own green capture at this base and was red at M1's base too — pre-existing, jetson's to answer, untouched here. |
| `INSTALL-PI` | 13 kernel + 5 doc/script files | Not a `:: NAME:` family at all — an ARC name, and a doc SECTION ANCHOR (`install/pi.rs:35` reads "See `installer_engine.md` §INSTALL-PI"). Renaming it to `INSTALL` would collide with the generic installer-engine narrative it exists to distinguish, and its pins reach five files this brief does not name (`unaos/docs/dev/OS/10_INSTALL/{installer_engine,orin-unafs-root,vein-smart-installer}.md`, `unaos/scripts/make-pi-install-src.sh`, `review/unaos-install-pi-LANDING.md`). Reported, not half-renamed. |

### OWED — sites a family lost to an excluded or unnamed home (NOT half-renamed)

| token | where | why owed |
|---|---|---|
| `:: PIUSB:` ×3 | `arch/aarch64/piusb.rs:50` (`const P`), `:199`, `:2047` | exempt home — the `:: PIUSB:`/`:: USB:` family now SPLITS across two names, the shape M1 met with `[piusb40]`. The Pi's own PCIe/VL805 bring-up keeps the board name; the shared xHCI driver no longer does. |
| `:: TEGRA-SD:` ×1 | `unaos/crates/kernel/Cargo.toml:1439` | a LIVE witness-family pin (`Witness families: :: TEGRA-SD: WRITE admitted`) that now disagrees with `arroyo`'s `BANNER_ARTIFACT_MAP` row. One-token change; Cargo.toml is not a file this brief names. |
| `:: TEGRA-SD:` ×1, `TEGRA-SD:` ×1 | `docs/dev/OS/orin-queue.md:21,267` | another track's queue. |
| `:: PIINSTALL:` ×2 | `docs/env-knobs.md:4874,4905` | not a file this brief names. |
| `:: PIUSB:` ×30 | `unaos/scripts/pi-usb1-bench.md` | the PI-USB-1 bench procedure's expected wire; not a file this brief names. |
| `PIUSB:`, `piusb27:`, `TEGRA-UNAFS:` | `docs/dev/OS/orin-ledger.md:141,221,223,224,227` | another track's ledger — and `:141` is row **B5**, the GATE-NEUTRAL census item this milestone answers. |

### Five corrections to §2, each measured at `acd102d7`

1. **`:: PINSTALL:` ×63 in `install/partition.rs` is absent from the table entirely** — the largest
   colon family after `:: PIUSB:`, and the one with twelve live spec rules behind it
   (`x86-install.spec`). **The cause is NOT M1's stripper bug, and that matters:** §0's COLON pattern
   was an ALLOWLIST of family names that listed `PIINSTALL` and never `PINSTALL`, so the instrument
   was never told to look. Measured at `acd102d7`: all 63 sites are live `serial_println!` code and
   none is inside a comment, so the census's zero was a fact about the PATTERN, not the data
   (LAWS §5). `census.py`'s COLON rule now matches the BOARD PREFIX and leaves the family name open,
   the shape its BRACKET and IDENT rules already used; the space in `:: NAME:` is kept because
   dropping it re-admits four Rust-module-path false positives (`::PixelFormat:` ×12, `::PinnedApp:`
   ×7, `::piusb:` ×2, `::pi:` ×1). **An allowlist census cannot report what it was never told to
   look for** — and three of M2's nine families (`:: PINSTALL:`, `:: piusb27:`, `:: piusb28:`) were
   invisible to it for exactly that reason.
2. `:: TEGRA-SD:` is **5** sites, not 3.
3. `:: PI-RAST:` is **4** raw / 3 live, not 3; `:: PI-DESK:` is **2** (a second site at `main.rs:7401`), not 1.
4. **Two colon families are missing from §2 altogether**: `:: piusb27:` ×6 (`fs/fat.rs`) and
   `:: piusb28:` ×2 (`main.rs`) — the colon twins of M1's `[usb27]`/`[usb28]` bracket families.
5. §2's total ("9 families / 131 sites") is wrong on both halves. Re-censused at `acd102d7` with
   the corrected pattern: **11 families / 200 live sites** in shared files, going to **2 families /
   38 sites** after M2 — and both remainders are the declared parked families (`:: tegra:` 36,
   `:: TEGRA-EL0:` 2). Raw occurrences, which is what a `sed` moves, are 169 in the kernel.

### M3 (identifiers) NOT started

Unchanged by this milestone: `TegraSd` 51 sites, `tegra_shell_*` 39, `tegra_el0` 19, `tegra_sd_info`
15 — the §3a population, plus the drift the M1 addendum recorded above.

## Addendum (NEUTRAL M3, 2026-09-23) — the identifiers B160 counted are RENAMED, and one of the three was a knob

Base `98fd8e66` (branch `exec-rmbp-neutral3`; the branch tip is the sha — the seat fills it at the
fold). Census re-derived at that base before any edit (`census-at-98fd8e66.txt`): identical to
`census-AFTER-m2.txt` in every token row — SDHCRW (B166) moved `TegraSd` lines inside `drivers/block.rs`
but not its count, which is still **51** live shared sites (80 raw). The substitution table IS the script —
`m3-rename.py`, one file with `--apply`, `--prove`, `--count`, `--preflight` and the go-red's `--half`.

### M3 — DONE (the three counted rows of B160's status cell)

The naming rule is this table's own (§3a): strip the board prefix; where the stripped name is already bound,
qualify it by the subsystem the code already uses (§3a's `orin_render_service` -> `render_pass_service`).

| old | new | kernel occurrences (raw / live) | files | why this name |
|---|---|---:|---|---|
| `TegraSd` (`BlockHandle::` + `BlockSource::` variants) | `SdMmc` | 80 / 51 | `drivers/block.rs` 17, `fs/fat.rs` 19, `fs/unafs.rs` 20, `main.rs` 7, `fs/bootdisk.rs` 6, `install/mod.rs` 3, `fs/vfs.rs` 2, `install/partition.rs` 2, `shell.rs` 2, `wifi/firmware.rs` 2 | §3a's proposal ("sibling of the existing x86 `Sdhc`"); what the handle IS — the card behind the SoC's SD/MMC host (`sdmmc` feature, and M2's `:: SDMMC:` family). 0 prior hits outside §3a |
| `TegraShellWin` | `ShellWin` | 15 / 13 | `main.rs` | stripped name, free |
| `tegra_shell_window_open` / `_mark` / `_id` / `_pal` / `_present` / `_pick` | `shellwin_window_open` / `_mark` / `_id` / `_pal` / `_present` / `_pick` | 37 / 26 (+2 comments in `video/dock.rs`) | `main.rs`, `video/dock.rs` | stripping gives `shell_*`, and `shell_id` (51 hits — `let shell_id = tegra_shell_id(&shellwin)` at the call site itself) and `shell_pal` (20, the x86/Pi pump's `TargetPal`) are TAKEN. The qualifier is the family's own: its unrenamed siblings are `shellwin_row`, `_absent`, `_launch_owed`, `_note_mint`, `_quit_note`, `_scene_live`, `_service_rearm`, `SHELLWIN_ROW` |
| `TEGRA_SHELL_PRESENTED` | `SHELLWIN_PRESENTED` | 2 / 2 | `main.rs` | same family (the M1 addendum's "≈41"), sibling `SHELLWIN_ROW` |

136 kernel occurrences + 27 in pins = **163 occurrences, 141 changed lines over 22 files, insertions ==
deletions, every line reproduced exactly by applying the table to the old line** (`m3-substitution-proof.txt`).
Census after (`census-AFTER-m3.txt`): IDENT 48/152 -> 42/126, UPPER 31/72 -> 30/70, CAMEL 6/83 -> 4/19 —
92 live shared sites to 0, and nothing else moved. Pins moved with the identifiers: `unaos/arroyo` ×4
(comments), `Cargo.toml` ×2 (comments), `k8-reach.registry` ×1 (evidence prose), `banner-cert.sh` ×1,
`jetson-sync1.spec` ×3, and six subsystem docs (`arch_arm64.md` 4, `dock.md` 3, `sdhc.md` 2, `layout.md` 3,
`partitions.md` 2, `vfs.md` 2). The substitution is WORD-BOUNDED (identifiers, not strings), and the
retired `tegra_shell_note` / `_live_id` / `_remint` / `TEGRA_SHELL_BASE/…` in the APPPIN history notes
(`main.rs:9567-9579`, `dock.md:225-226`) are NOT in the table: they name deleted functions, and history
keeps the names it had.

### THE SECOND SPELLING IS THE IDENTIFIER ON THE WIRE

M1's was regex-escaping; M2's the dropped `:: ` prefix. M3's: `TegraSd` is spelled inside FOUR `:: UNAFS:`
witness MESSAGES in `main.rs` (`probe SKIPPED — no TegraSd block backend`, `MOUNTED read-only on TegraSd`,
`partition scan on TegraSd FAILED`, `mount on TegraSd FAILED`), and three live gates pin that text:
`banner-cert.sh:312` (the `sdmmc` artifact row — left behind, a hard red on every sdmmc-armed media
build), `jetson-sync1.spec:593` (PENDING) and `:619` (FORBID — left behind, a rule that can never match
and reads green, M2's own finding). All three are in `m3-rename.py`'s EXPECT table with their counts and
move in the same pass; the wire now reads `… MOUNTED read-only on SdMmc …`. Nothing in `pi4-regression.spec`,
`x86-default.spec` or `test-arm`'s capture carries any of the nine tokens, because the variant exists only
under `aarch64 + tegra + sdmmc` and the shell family only under `tegra`: no QEMU leg can witness the wire
half, and the jetson spec is scored only on metal.

### GO-RED (`m3-gored.txt`)

`--half fs/bootdisk.rs TegraSd` (the table applied everywhere except `bootdisk.rs`): the `arm-tegra-render`
leg is **rc=101, `error[E0599]: no variant, associated function, or constant named TegraSd found for enum
BlockSource` at `fs/bootdisk.rs:672:31`**; the SAME half tree on the x86 default feature set is rc=0 — the
x86 leg is blind to this rename, and only the tegra+sdmmc legs falsify it. Reverted, re-applied whole.
**A FIRST go-red did not fire, and it is a finding:** `--half wifi/firmware.rs TegraSd` is rc=0 on the tegra
leg, because `wifi/firmware.rs:772-773`'s `TegraSd` arm is `aarch64 + tegra + sdmmc`-gated inside a module
`lib.rs:65` gates `wifi + x86_64`. No configuration compiles that arm — it is dead in every build, so a
half-rename there passes every compile gate; only `--prove` covers it.

### NOT MOVED — and why

| token | count at `98fd8e66` | why |
|---|---:|---|
| `tegra_el0` (B160's "19") | 19 live shared | **Not an identifier.** All 19 are the FEATURE NAME — `#[cfg(feature = "tegra_el0")]` sites and prose naming that feature (`video/quarry/live.rs` 9, `main.rs` 4, `video/dock.rs` 4, `shell.rs` 2). A feature name is a knob (§4; M2: "a rename batch keeps knobs"; LAWS §4), and renaming it is a five-place knob change, not a token substitution. B160's cell counted it with the identifiers because the census's IDENT pattern cannot tell a cfg string from a symbol. |
| `tegra_el0_start_maybe` 3, `"tegra-el0-verdict"` 1, `tegra_el0_verdict` 1 (arch-homed) | 5 | the emitters of `:: TEGRA-EL0:` — Peter's open question (rename-all-four vs machine-tag-by-design, B160 (2)); they move with his answer |
| `:: tegra:` | 42 raw / 36 live, `main.rs` | excluded — Peter's seam question (B160 (1)); `tegra_early_stop` is its terminus |
| `:: TEGRA-EL0:` | 2 `main.rs` + 2 `arch/aarch64/syscall.rs` | excluded — Peter's question (B160 (2)) |
| `:: PIUSB:` | 3, `arch/aarch64/piusb.rs` | excluded — Peter's question; board-named file |
| `TegraSd` ×1 | `arch/aarch64/sdmmc_tegra.rs:1866` (comment) | arch home AND a board-named file; the comment now names `handle_write(TegraSd)`, an arm spelled `SdMmc` — owed with the arch lane, the M1 `[orinrender]`-comment shape |
| `TegraSd` ×3 | `unaos/docs/dev/OS/10_INSTALL/orin-unafs-root.md:122,135,147` | a file whose NAME carries the board (the brief's rule) |
| records: `LAWS.md` `TegraSd` 3; `LEDGER.md` `tegra_shell_window_open` 2; `orin-ledger.md` `TegraSd` 16 + `tegra_shell_present` 1; `rmbp-ledger.md` `TegraSd` 10 + `TegraShellWin` 1 + `TEGRA_SHELL_PRESENTED` 1; `orin-queue.md` `TegraSd` 2; `rmbp-queue.md` `TegraSd` 4; `docs/dev/evidence/**` | 41 + evidence | records, not code: a ledger row and a rule quote the names the tree had when they were written (LAWS §3's orin 19-21 incident is spelled `locate_on(TegraSd)` because that is what shipped) |

### OWED — the rest of §3a, NOT in B160's M3 cell (measured at `98fd8e66`, live shared sites)

Same subsystems, different identifiers — left whole rather than half-renamed: the SD handle's siblings
`tegra_sd_info` 15, `tegra_sd` (field) 7, `tegra_sd_writes_admitted` 4, `tegra_sd_write_through` 3,
`tegra_sd_state` 2, `tegra_sd_write_blocks_through` 2, `TEGRA_SD_BLOCK_DEVICE` 3, `TEGRA_SD_WRITE_REFUSED` 3,
`TEGRA_SD_PUBLISHED` 2, `TEGRA_SD_VETO` 2, `NATIVE_TEGRA_SD_VETO` 2, the `"tegra-sd"` handle-name string (a
WIRE token, `handle=tegra-sd` / `source=tegra-sd`, with scorer pins in orin evidence) and `b"TEGRA-SD"`;
the shell window's `tegra_boot_focus` 3 / `TEGRA_BOOT_FOCUS_DONE` 2; the §3a desk/render/stack/console rows
(`tegra_desk_*`, `tegra_render_arm`, `orin_render_service`, `orin_desk_scene_up`, `tegra_conwin_live`,
`tegra_cascade_stk_*`, `tegra_stk_anchor`, `ORINSTK_ANCHOR_SP`, `TEGRADESK_*`, `ORINFURN_ENTERED`,
`ORINRENDER_ARMED`, `orin_face_arm`, `ORINFACE_*`, `tegra_darkwin_witness`, `tegra_quarry_seat`,
`TEGRA_JD2_STACK_SIZE`, `TEGRA_DRAM_TOP`, `tegra_opened`); the pi/usb rows (`pi_ls_witness`,
`pi_usb_ls_witness`, `PI_RAST_FRAMES`, `PIUSB24/26_LAST_LOG_MS`, `PIUSB28_ARMED`, `PIUSB36_STATIC_BUF`, the
twelve glued `piusbNN_*` fns); and §3b's arch-homed `orin_*` symbols (renamed at their definition in
`arch/aarch64/display_tegra.rs`, a board-named file). Full list with counts: `census-AFTER-m3.txt`.
