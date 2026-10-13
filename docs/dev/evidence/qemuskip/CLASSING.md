# QEMUSKIP — classing the QEMUIMAGE26 reds (rmbp-ledger B613)

Source run: `docs/dev/evidence/qemu-image26/RUN.md` and its `serial.log` (image-26 tree @62a3901a8, one KVM boot,
R114). Rows B558–B607. Branch `exec-rmbp-qemuskip`, parent `cb833ebb`. The witness is the
bench's next QEMU run on a UnaFS home, then the rmbp bench.

## The run's store

`[vfs] root mount / = fat boot volume source=ahci5 rw=no` and `unafs=absent` (serial.log 07:13:52Z). The root is
the builder's ESP directory, attached `fat:rw:` on `ide-hd` (`unaos/builder/src/main.rs:1895`); its block source
refuses writes. Every fixture that writes under the home therefore met the volume's refusal before its subject.

## The classes

- **metal-only**: the hardware or driver the fixture needs is not there; the fixture now asks the driver and says
  `-> SKIP reason=<…>`.
- **home-fat**: the fixture writes under the home (or `/boot`, `/apps`) and the volume is a FAT that refuses;
  the fixture now asks `fs::vfs::home_unwritable` (B556's native-root test, then `write_veto`) and says
  `-> SKIP reason=home-not-writable(fat)`.
- **builder**: the image lacked what the fixture reads.
- **real**: the driver is bound and the volume is not the cause; the FAIL is the fixture's or the code's. Each
  has a finding below (§n), filed in B613.

## Table

| row | fixture | class | basis (wire) | this arc |
|---|---|---|---|---|
| B558 | `tests list` | real: §1 | `cmd="tests list" -> Host verb=tests`, nothing after | fixed: the list prints on the wire |
| B559 | apptrust | home-fat | `copy=REFUSED … anon=ADMITTED`: the `/apps` copy probe meets the FAT's `Unsupported` (not `Denied`) | owed: fs/apptrust.rs (S4) |
| B560 | boot80 | home-fat | `types=0 … stamp=none`: a FAT root carries no type stamp | owed: fs/bootstep.rs (S4) |
| B561 | brightfloor | metal-only | `driver=gmux` with every `readback=-`; `seed readback=- … (gmux did not answer …)` | SKIP reason=no-gmux |
| B562 | brightstep | metal-only | `readback_ok=0 slider_sync=bad keys=ok` | SKIP reason=no-gmux |
| B563 | bt | real: §2 | `fixture l2cap-config … -> FAIL` (a synthetic KAT) | — |
| B564 | clock | metal-only | `[clock] source=apic … invariant=0` | SKIP reason=no-invariant-tsc |
| B565 | dimidle | real: §3 | `woke_at_ms=0 wake_key_swallowed=0` | — |
| B568 | ehci (TPDRAG) | real: §4 | `d1=-21/0 d3=6/0` (a synthetic KAT) | — |
| B569 | filetype | home-fat | `dir=/ reason=create FTPIC.DAT` | owed: fs/filetype.rs (S2) |
| B570 | filetypes | home-fat | `[filetypes] built … reason=enotsup (root takes no attributes)` | owed: fs/assoc.rs (S4) |
| B572 | gpublit | metal-only | `:: kepler: no-device ::`; `boot=no-takeover` | SKIP reason=no-kepler |
| B577 | lidsleep | metal-only | `msld=no-key … backlight_off=fail`, no gmux | SKIP reason=no-gmux |
| B580 | logwit | real: §5 | `bare=0 absent=4 ring-unreadable=0` | — |
| B581 | nethang | real: §6 | `held_locks=1 … link=1` | — |
| B583 | player | home-fat | `reason=no-TEST.WAV-and-no-writable-path` | SKIP |
| B584 | playwav | home-fat | `reason=no-writable-path` | SKIP |
| B585 | prefs | home-fat | `[prefs] set fixturens.* ok=0 why=Io` (×4), `app.sftest.level=10 ok=0 why=Io` | SKIP (both lines) |
| B586 | prefscap | home-fat | `own=fail` = `app.pcaptest.level=3 ok=0 why=Io`; the other legs held | SKIP |
| B588 | rawcore | home-fat | `could not write /home/una/RAWTEST.ARW` | SKIP |
| B590 | ringlogin | home-fat | `ring=no-holocron`: the holocron lives on a UnaFS root | owed: keyring.rs (S4) |
| B591 | selfdiag | home-fat | `stage=mkdirs err=-95 dir=/boot/SDFIX bootlog=fat-root` | SKIP |
| B592 | settings | home-fat | `loaded=0 saved=1`; `display.idle_min=5 ok=0 why=Io` | SKIP |
| B593 | settingsbus | home-fat | `set=0 get=ne`; `display.idle_min=7 ok=0 why=Io`; `direct_writers=0` is the PASS value | SKIP |
| B594 | shot | home-fat | `create /home/una/Desktop/DESKTOP.MSK: Unsupported` | SKIP |
| B595 | sinkdrain | real: §5 | `on_cable=0 … peeked=70B` | — |
| B596 | smallfix6 | home-fat | `fwvol=skipped(volume=boot)` is outside `fw_ok`'s words | owed: smallfix6.rs (S4) |
| B597 | smallfix7 | real: B605 | `verdicts=6:sdhc,DRAINCAP,SERWIT-1B,PWRDRAIN,SERWIRE,SERIALTX` | — |
| B599 | statustray | real: §7 | `net=no` on injected facts | — |
| B600 | testf | builder | `staged=0/24 missing=all … reason=no-dir` | owed: builder/src/main.rs (S1) |
| B601 | usbnet | real: §8 | `chip=ecm … frames=0 … reason=no-frame-5s` | — |
| B602 | usbnet7 | real: §8 | `rx_ok=0 frames=1 … dhcp=offer` | — |
| B603 | usbnetrx | real: §8 | `frames=0 … td=unread` | — |
| B604 | usbstor | metal-only | `boot_medium_kept=0 … reason=boot-medium-not-pinned` (the boot medium is the AHCI ESP, not the Sdhc card) | owed: fs/removable.rs + drivers/block.rs (S3) |
| B605 | verdicts | real (its own row) | `before_deferred=6` | — (SKIP is already not-red: below) |
| B607 | xhcimedia | real: §9 | `BRIGHTKEYS: … indicator=0 -> FAIL`; `XHCIMEDIA: … -> PASS` | — |

Not classed here (KVMLAT's and UIREDS26's rows): B566 B567 B571 B573 B574 B575 B576 B578 B579 B582 B587 B589 B598 B606.

Counts (36 rows classed): metal-only 6 (B561 B562 B564 B572 B577 B604); home-fat 15 (B559 B560 B569 B570 B583
B584 B585 B586 B588 B590 B591 B592 B593 B594 B596); builder 1 (B600); real 14 (B558 B563 B565 B568 B580 B581 B595
B597 B599 B601 B602 B603 B605 B607). This arc's SKIP code covers 14 rows (5 metal-only, 9 home-fat); 8 are owed in
files outside its list (B604; B559 B560 B569 B570 B590 B596; B600 — S1 to S4 below); B558 is fixed.

## What the SKIP lines read (this arc's code)

```
:: BRIGHTFLOOR: floor=1 set0_on=0 load_clamped=1 reset_ok=1 full=0 mono=1 driver=gmux -> SKIP reason=no-gmux ::
:: BRIGHTSTEP: steps=5 readback_ok=0 slider_sync=bad keys=ok driver=gmux -> SKIP reason=no-gmux ::
:: LIDSLEEP: msld=no-key raw=- flips=0 backlight_off=fail prior=767 off_rb=- restore_rb=- armed=0 -> SKIP reason=no-gmux ::
:: GPUBLIT-TEST: rerun=refused boot=no-takeover -> SKIP reason=no-kepler ::
:: CLOCKCORE: source=apic invariant=0 ref=pm -> SKIP reason=no-invariant-tsc ::
:: PLAYER: reason=no-TEST.WAV -> SKIP reason=home-not-writable(fat) ::
:: PLAYWAV: path=- -> SKIP reason=home-not-writable(fat) ::
:: RAWCORE: home=/home/una -> SKIP reason=home-not-writable(fat) ::
:: GLASSEYES: states=6 shot=desktop png=none mask=none -> SKIP reason=home-not-writable(fat) ::
:: PREFS-FIXTURE: dir=/home/una/settings -> SKIP reason=home-not-writable(fat) ::
:: SETTINGSFILES: dir=/home/una/settings -> SKIP reason=home-not-writable(fat) ::
:: PREFSCAP: dir=/home/una/settings -> SKIP reason=home-not-writable(fat) ::
:: SETTINGS: controls=11 tabs=9 store=/home/una/settings -> SKIP reason=home-not-writable(fat) ::
:: SETTINGSBUS: via=- store=/home/una/settings -> SKIP reason=home-not-writable(fat) ::
:: SELFDIAG: provider=echo dir=/boot/SDFIX bootlog=fat-root -> SKIP reason=home-not-writable(fat) ::
[tests] list <name>          <arc>                      (one per registered fixture)
:: TESTS: list n=<n> deferred=<d> at_boot=<b> ::
```

The field values are the QEMUIMAGE26 run's where the line reuses them (the gmux and clock lines print what the run
printed, with the new verdict); `/home/una` is that run's session. A SKIP line carries no `-> FAIL` and no `-> PASS`,
so the verdict tap (`selftest.rs:158-159` N_PASS/N_FAIL, `tally` at :248) never counts it, `tests` names it under
`skipped=[…]` (`tests.rs` `skip_note`), and `tests verdicts` (B605), which counts only tallied lines, reads it as
not-red. The SKIPs fire only where the probe says so: a writable FAT (`write_veto` clear) or a native root runs the
home fixtures as before, so the x86 lanes' REQUIREs (x86-witness.spec:1647 PLAYWAV, x86-wc.spec:968 SETTINGS)
see the same lines as before on a store that takes writes; a build that drives no gmux (`sim`) runs the backlight
fixtures as before.

## Findings (real), filed in B613

1. **B558, `tests list` on the door.** `tests::shell_verb`'s `list` arm wrote each row and the tail with
   `console.println` only; a verb routed from the serial door (`[midden] cmd="tests list" -> Host verb=tests`) has
   no console the door can read. Fixed: each row is also `[tests] list <name> <arc>` and the tail
   `:: TESTS: list n= deferred= at_boot= ::` on the wire. The door's own Enter arm not counting lines
   (`DOOR: … lines=0`) is B567's (UIREDS26).
2. **B563, bt.** `parse_cfg_opts` (drivers/ehci/bthid.rs:1916) matches on `t & 0x7F`, so the KAT's second option
   `0x81 0x02 0x00 0x00` (type 1 with the hint bit) sets `mtu=Some(0)`; the KAT wants it skipped ("hint-bit option
   skipped"). The parser and the KAT disagree on a synthetic frame; which one the L2CAP spec supports decides the
   fix. Hardware-independent: it reds on every machine.
3. **B565, dimidle.** The blank happened (`blanked_at_ms=271968`), the wake did not (`woke_at_ms=0`,
   `wake_key_swallowed=0`). State 2 waits 2000 ms for the swallow (video/dimidle.rs:144); the same run's
   `HIDSTALL … key_queue_max_ms=2542` exceeds that window. Hypothesis, unmeasured: KVMLAT's key latency. The
   x86-wc lane REQUIREs this PASS.
4. **B568, ehci TPDRAG.** A synthetic KAT: `want` is built from `tp_scale`, while `mt_step` applies the trackpad
   speed gain after the curve (drivers/ehci/mod.rs:19584, "speed is one gain after it (tpgest)"). `d1=-21` against
   `tp_scale(-60) = -15` says the gain was not 1 when it ran; the same run printed `set system.trackpad.speed=5
   ok=0 why=Io` earlier. The KAT is not speed-neutral (or an earlier fixture left the speed). Unmeasured which.
5. **B580 logwit, B595 sinkdrain.** The FTDI is bound — the door typed every `tests` line through it — and its ring
   was readable (`ring-unreadable=0`); the markers were not in the peek window (`absent=4`), and SINKDRAIN read
   `peeked=70B on_cable=0`. The x86-witness lane REQUIREs LOGWIT's PASS. Not a missing device; cause unread.
6. **B581, nethang.** `link=1`, the usbnet lease held (`[usbnet] census … leased=1`); the red leg is
   `held_locks=1`: after the resolve either the stack lock was not free or the xHCI loan answered Busy
   (netring3.rs `held` count). `rc=no-answer ms=1999` is the resolve's own bound, which held.
7. **B599, statustray.** The `net` leg reads facts the fixture INJECTED (`status::tray_inject`, a USB link with a
   lease of 10.0.1.2) and read `no` on a boot whose real usbnet was leased. Cause unread.
8. **B601–B603, usbnet family.** QEMU's ECM function is bound (`chip=ecm`), the link is up and leased
   (`leased=1`, `arp=2 ipv4=3 dhcp_replies=2`). USBNET6 waits 5 s for a frame no peer sends on a quiet network;
   USBNET7 `rx_ok=0 frames=1 … dhcp=offer`; USBNETRX `frames=0 td=unread`. The fixtures assume traffic; whether a
   bound, leased, silent link is SKIP (`no-traffic`) or FAIL is a design question, not answered here.
9. **B607, xhcimedia.** XHCIMEDIA's own verdict is PASS. The red is the BRIGHTKEYS line its injected
   brightness-up key produced: `brightkeys::apply` prints on the desktop service pass, after the fixture's volume
   keys re-armed the bezel (`[bezel] show kind=volume`), so `indicator() == K_BRIGHT` is false (`indicator=0`).
   `gmux_written=0` is a PASS shape (video/brightkeys.rs:85). Not a gmux question.

## Owed outside this arc's files (STOP — the exact change)

- **S1, B600 testf (builder).** The QEMU root is the ESP directory (`fat:rw:` on ide-hd); `stage_testf` runs only
  for the DATA tree (`builder/src/main.rs:1540`, `&data_dir`), which rides `builder/usb-boot.img`, and arroyo's
  `esp_x86_unafs_card` puts `$data_t/system` onto the card's UnaFS `/system` — so metal has test-f and the QEMU
  ESP does not. arroyo cannot stage it: the builder `remove_dir_all`s the ESP tree (:1143) and launches QEMU in the
  same process. The one line, `unaos/builder/src/main.rs`, after :1439
  (`let _ = stage_fonts(&kfont_dirs_env(), &esp_dir);`):
  `let _ = stage_testf(&workspace_dir.join("builder/testf.list"), &target_dir.join("testf-cache"), &esp_dir); // QEMUSKIP (B600): the ESP is the root a FAT-only boot reads /system/test-f from`
  (`< 1 MB`, builder/testf.list's header; `una-card --skip system` keeps it off a card's p1.)
- **S2, B569 filetype.** `unaos/crates/kernel/src/fs/filetype.rs` `selftest`, folded onto the
  `let p = |leaf: &str| …` line, before it:
  `if let Some(k) = crate::fs::vfs::home_unwritable(&mt, dir) { serial_println!(":: FILETYPE: typed=0 sniffed=0 ext=0 assoc=- override=- quarry=- dir={} -> SKIP reason=home-not-writable({}) ::", dir, k); return; }`
- **S3, B604 usbstor.** `drivers/block.rs`: `pub fn boot_medium_not_sdhc() -> bool` = `BOOT_PIN == 2` under the
  same cfg as `boot_medium_kept` (`false` otherwise); `fs/removable.rs` `witness()`: when `!kept &&
  boot_medium_not_sdhc()` the verdict word is `SKIP reason=boot-medium-not-sdhc` instead of `FAIL` (the line is
  also the removable-change event line, so the word changes there too).
- **S4, the other home-fat fixtures** (not named by this arc): apptrust (`fs/apptrust.rs` `copy_probe` target
  `/apps`), boot80 (`fs/bootstep.rs`), filetypes (`fs/assoc.rs`), ringlogin (`keyring.rs`), smallfix6
  (`smallfix6.rs` `fw_ok`): the same predicate at each fixture's head, `-> SKIP reason=home-not-writable(fat)`.
