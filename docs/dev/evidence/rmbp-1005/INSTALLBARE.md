# INSTALLBARE — nothing runs but the setter (rmbp-ledger B364, R86 under R77/R80)

Branch `exec-rmbp-installbare`, cut from the boot-24 integration `ed1cc57f`. x86 metal shape.

## 1. The evidence (flight 22, `docs/dev/evidence/rmbp-0915/flight22/f22-boots.log`, read with awk)

### Boot 1: under the password setter, `[login] set-password screen open` (04:54:26, line 384) to the first key (04:54:53, line 715)

The tags in that window, with how many lines each printed (332 lines in all):

| group | tags (lines) |
|---|---|
| **network** | `[net] poll` ×27 (sendto/recvfrom pairs, `masked=1` on sid 1: each recvfrom sits 2 s); `SOCK-1` (icmp 0/4), `SOCK-2` ×2 (udp dns, ring-3 udp), `SOCK-3`, `SOCK-5` (dhcpv4 no offer), `SOCK-6`, `SOCK-7`; `SMOLNET [dns]` ×1; `[sntp] target=0.0.0.0` ×1 |
| **usbnet** | `USBNET: candidate` + `USBNET: up -> PASS` (2), `[usbnet]` ×18 (the register walk, rx_arm, link up at 04:54:53) |
| **bluetooth** | `bt-sched` ×3 (campaign start, FIRING, COMPLETE), `bt-retry` ×2, `bthid: HCI …` ×10, `BTHID` ×2 (bring-up, up) |
| **wifi** | `:: wifi:` ×8 (net function skipped, radio SELECTED, three firmware ABSENT, HELD 30 s, second-handle wait n=1) |
| **ring-3 fixtures** | `U5x` ×2 + `u5x` ×2, `U7x` ×2 + `u7x` ×1, `U8x` ×2 + `u8x` ×2, `U9x` ×2 (5 s storage wait), `U2` ×1, `SYSCALL` ×1, `WXAUDIT-SLOT` ×1 |
| **furniture** | `[dock] dockpin login` + `[dock] quit app=shell by=x86_render_service win=3 gen=1 -> TORN-DOWN` (a shell window minted, then swept), `DOCKPIN` ×1, `[login] installer: furniture swept n=2` (console + shell had been minted at the takeover), `[kfont] load` ×1 |
| **status / power** | `[status] poll` ×1, `SMC-BATT` ×1, `PWR` ×1, `PSRC` ×1, `DIMIDLE` ×1 |
| **compositor** | `[wc-d] valve CLOSED` at 04:54:24 (line 236, before the screen opened), `[wc-d] valve OPEN resumed suspended=15 after 29027ms closed` at 04:54:53 (line 718, three lines after the first key), `[wc-d] paygo` ×1, `[wc-h] rollup` ×2 |
| **census / prose** | `BPACE` ×128 (the boot-phase ledger re-dumped twice), `BOOTLOG` ×15, `SDHCBLK` ×16, `SERWIT-2` ×5 + `SERWIT-1B`, `DRAINCAP`, `PWRDRAIN`, `S5DRAIN`, `SERWIRE`, `SERIALTX`, `CLOCK-X1`, `SDHC4C-ROOT`, `TESTS: deferred=67`, `[vfs]` ×3, `[boot] step` ×3, `FIRSTBOOT` + `BOOT` |
| **USB enumeration** | `xHCI` ×13 (three ENUM RECOVERY), `>>> NEW HARDWARE` ×8 (FTDI and the AX88179), `FTDI` ×2, `U2.5` ×1, `EHCI-HID STOP-NOTE` ×2 (the BT HID proxies) |
| **the keyboard** | `EHCI-HID: KEY:` ×6 + `KEYUP` ×6, **all six in the same second, 04:54:53**, and each one **printed the typed character** ('q','w','e','r','t','y': Peter's password, on the wire, R65), then `USB-DEBUG: KEY withheld` ×1 and `[login] key taken by the screen` |

What it shows: the six keys left the EHCI decode together, in the second the 2 s `[net] poll … masked=1` recvfrom
loop and the U9x storage wait finished and the valve reopened. The setter was up and painted; the input path behind
it was waiting on boot work that had no business running.

### Boot 2: the login screen, `FIRSTBOOT: stage=login-screen` (05:57:39, line 19361) to the end of the capture (06:01:53, line 20279)

Peter did not log in on boot 2 in this capture (the log ends at the login screen with the machine left up), so the window
runs 4 min 14 s to the end. 918 lines:

| group | tags (lines) |
|---|---|
| **the stray stage line** | `[login] installer: stage=desktop (R77: the desktop ignites)` at 05:57:39, right after `FIRSTBOOT: stage=login-screen` |
| **furniture / programs** | `[wc-x] desktop-app HOLD-NONE name=/STAT.ELF`, `STAT: start pid=12` + `STAT: alive` (2), `[dock] dockpin login`, `[dock] quit app=shell … -> TORN-DOWN`, `DOCKPIN`, `[login] installer: furniture swept n=2` |
| **audio** | `[hda]` ×37 (audit walk/end, deferred start), `HDA` ×1 |
| **network** | `[net]` ×128, `SOCK-5` ×7 (dhcpv4 retries), `SOCK-1/2/3/4` (ring-3 SOCK-2/3/4 FAIL), `SOCK-6/7` |
| **usbnet** | `[usbnet]` ×18, `USBNET` ×2 |
| **wifi** | `:: wifi` ×13, `wifi2` ×31, `wifi4` ×3 |
| **bluetooth** | `bthid` ×10, `BTHID` ×2, `bt-sched` ×3, `bt-retry` ×2 |
| **status / power** | `[status] poll` ×4, `PWR` ×22, `SMC-BATT`, `PSRC`, `DIMIDLE` |
| **compositor** | `[wc-d] valve CLOSED` at 05:57:37 (line 19211, never reopened in the window), `[wc-d] paygo`, `[wpace]` ×142, `[wc-h]` ×127, `[beam]` ×9 + `BEAMHOLD` ×9, `[wedge*]` ×15, cursor/pointer probes ~40 |
| **census** | `BPACE` ×128, `BOOTLOG` ×15, `SDHCBLK` ×17, `SERWIT-2` ×5, `USB-DEBUG` ×28, `zeolite` ×4, `STORMFAULT`, `SHOTMENU`, `SCHEDPLACE-X86` |

"stat and glass are open along with the login dialog": STAT.ELF launched because `desktop_allowed()` answered **true** at
the login screen. Boot 2 published `BootStage::Desktop` (why=`store-has-users`) and the screen was a separate flag
(`BOOT2`) that no starter read. The taskbar shadow both times was the bar `desktop_uefi::activate` enabled at the takeover,
together with the console window it minted. `furniture_held()` answered `false` while the stage was unresolved ("the
first ~400 ms of a metal boot paint as before"), so they painted until the store resolved and the sweep took them down.

## 2. The seam

`Kernel — kernel-by-ruling (R86)`. A new module `unaos/crates/kernel/src/boot.rs` holds ONE gate:

```
boot::phase() -> Phase::{Setter, LoginScreen, Desktop}     // pure: atomics only, safe under any lock
boot::services_up() -> bool                                 // phase()==Desktop AND (furniture ready OR 1.5 s bound)
boot::services_gate(who) -> bool                            // services_up + the ONE `[boot] services up` line, + held-census
```

- **Setter**: the store is not read yet, or it says Installer / CreateUser.
- **LoginScreen**: the store has users (boot 2) and no session has opened since boot.
- **Desktop**: anything else, latched by the first session open (`login::close_into_session`) or by the
  installer's own `user-created` advance, so a later Log Out does not stop the services.

`users::desktop_allowed()` now answers `phase()==Desktop` (it keeps its 30 s no-store fallback), so every
tenant that already asked it (STAT.ELF, the witness chain, winx, `tests`, the shell verb, hda) now also
waits at the login screen. Boot 2's `BOOT2` flag is published BEFORE the stage, so no reader ever sees
"Desktop without BOOT2". Non-`login` builds answer Desktop from the first instruction (no behaviour change).

## 3. Milestones

- **M1, the gate, read by every starter**:
  - furniture: `desktop_uefi::activate` neither mints the console window nor enables the menu bar unless the phase is
    Desktop (`fbcon::detach()` instead, and `login::furniture_owed()` + `users::bar_owed()` so the first Desktop
    advance re-mints console + shell and turns the bar on); `x86_render_service` does not mint the shell window
    (same latch); `furniture_held()` now holds while the stage is unresolved;
  - programs: STAT.ELF (through `desktop_allowed`);
  - services: `wifi::service`, `status::poll`, `e1000::service_net`'s smolnet witnesses + idle poll,
    `net_tick::service_tick` (dhcp + sntp), `usbnet::bringup_pending` (the class bring-up; enumeration still runs),
    the bt boot campaign, the ring-3 U2/U4x/U5x/U6x/U6bx probes (the SOCK-2/3/4 chain), hda (`services_up`);
  - compositor: `wm::verify_reference` returns before the WC-D valve unless services are up, so no valve
    episode exists under a setter or the login screen;
  - the stray line: boot 2's publish now says `[login] installer: stage=login-screen (R86: the login dialog only;
    the desktop ignites at the first login)`.
- **M2, the keyboard path**: `boot::key_stamp()` at the EHCI decode (only while a secret screen is up) and
  `boot::key_taken()` in `login::consume_key` give `[login] key latency ms=<n> max=<m> n=<k> hid_gap_max_ms=<g>`
  per key (`hid_gap` = the longest gap between two `service_ehci_hid` passes while not at the Desktop: the
  starvation the queue latency cannot see). The EHCI decode line no longer prints the character while a
  secret screen is up (`EHCI-HID: KEY: withheld`). `tests installbare` reports what the boot recorded
  before its Desktop:
  `:: INSTALLBARE: phase=<setter|login-screen> services=<n> windows=<n> valve=<none|n> first_key_ms=<n|none> key_max_ms=<n|none> hid_gap_max_ms=<n> held=<n> -> PASS|FAIL ::`
  PASS when services = windows = valve = 0 and key_max ≤ 50 ms (no keys → `none`, still PASS). SKIP on a boot that had
  no pre-Desktop phase.
- **M3, the first-login ignition order**: session open → `[boot] phase=desktop from=<setter|login-screen> at=<ms>` →
  furniture re-mint (console + shell posted by `close_into_session` / `installer_release`) → the shell's
  `dock::app_launched` marks furniture ready → services gate opens (or the 1.5 s bound) →
  `:: BOOT: login->desktop=<ms> services_after_ms=<ms> why=<furniture-ready|bound> ::`. The boot's own `loader->desktop`
  line (`bootpace::boot_line`, printed at the first stage) is unchanged and loses work, it gains none.
- **M4, GLASSEYES**: `shot setter` and `shot login` compose a BARE scene from the desktop: every window minimised
  (restored afterwards), bar and dock held, and the real form (`Set password` for root / `Log in`) opened over the empty
  desktop. Captured as `SETTER.PNG` / `LOGIN.PNG` with the password field masked. The goldens are blessed by the
  first flown boot.

## 4. Witness (what a metal boot should print)

Boot 1 (installer), between `set-password screen open` and the first key: NO `[net]`, `[usbnet]`, `bthid`, `wifi`,
`[status]`, `U5x/U7x/U8x`, `SOCK-`, `[dock] quit`, `[wc-d] valve`, `furniture swept n=2`; per key
`[login] key latency ms=<≤50> …`; after create-user:
`[boot] phase=desktop from=setter …`, `[boot] services up first=<who> …`, `:: BOOT: login->desktop=<ms> … ::`.
Boot 2: `FIRSTBOOT: stage=login-screen`, `[login] installer: stage=login-screen (R86 …)`, no STAT, no hda, no net
until `[login] session open user=…`, then `[boot] phase=desktop from=login-screen`.
From the desktop: `tests installbare` → `:: INSTALLBARE: phase=setter services=0 windows=0 valve=none first_key_ms=<n> … -> PASS ::`.

## 5. Stays owed

- The console's boot-log text: with no console window before the Desktop, its first mint is `from=none`. The
  boot log is still on the wire and in the flight recorder.
- The xHCI/EHCI enumeration itself (FTDI, the dongle's descriptors, BT radio claim) still runs at boot. It is
  device discovery, and the serial door needs it. Only the class bring-ups wait.
- Census and prose lines (BPACE ×128, SERWIT, DRAINCAP, …) are QUIETBOOT's, not this arc's.
- `kepler_vblank` rung 3 is not gated (it reads one BAR0 word and needs no compositor, by its own design note).
- Metal is the proof (R78): the keyboard latency bound and the empty wire under the setter are read on the next flight.
