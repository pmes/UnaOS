# FLIGHT 39 — IMAGE 34 on the UNAFS CARD IMAGE (rmbp12flight39, claude/optimistic-ramanujan-r3qyu5@1c1c5689e), 2026-11-03, narino (the Mac)

Medium: `UnaOS-rmbp-esp-rmbp12flight39-20261103T2112Z-1c1c568/unaos-x86-card.img` sha256 818377f1…, kernel.elf 5eaf9242… (24979016 B).
Built on narino at the tip (kernel tree unchanged since the seat's 1af892778), image-line.env (52 knobs) + `UNAOS_SONDE_RECORD=12`,
rustc nightly-2026-10-26 (the tip's TOOLCHAIN-ONE pin), GNU stat/nproc/date shims, Homebrew dosfstools + mtools. Gates on narino:
banner-cert ok=60 missing=0, TRUST pin 9cc2a774 = pin, b43.pins MATCH, kepler.pins MATCH, BOOTLIST staged, strings `:: SONDEREC:`×3.
`./arroyo check` is RED on the tip (x86-all leg: `fs/bootfat.rs:221` lacks the `BlockSource::Nvme` arm; `nvme` is not on this line) and
`./arroyo gates` 18/25: prefs (`system.display.icc` undeclared, prefs.rs:111), k8reach (UNAOS_DISPLAY_ICC / NVME_4K / NVME_DISK unregistered),
locktype (`video/colour.rs:41` raw spin::RwLock), appearance (desktop_uefi.rs ×5), status (uncited ×4); deps and verbs are host-only (python 3.9,
a BSD probe). Flown on Peter's word (R133: the rows fold in the stream). Card written by Peter (dd, read-back sha matched).
**Not on this card:** the Linux self-host fixtures (Mac host, no Linux linker). **Peter's call for this boot:** no D435i, no USB 3, the
Ethernet dongle plugged AT POWER-ON — so `tests sonderec` was never readable and the boot became flight 38's open question.

Captures (pinned): `f39-boot1.log` (boot 1, mark to power-off), `f39-boot2.log`, `f39-boot3.log` (the two unlock-form boots).

## Peter, verbatim (the glass)
- boot 1: "seems locked up on the recovery key dialog. will not continue mouse cursor gone" → (after the blind click) "i got it. mouse
  cursor was hard to get unhidden" → "trackpad is barely functioning".
- boot 2: "rebooted. cannot enter password or see mouse cursor. desktop is logged in which is weird. just asking to unlock the disk".
- boot 3: "i hit enter and the cursor stopped blinking maybe? should it blink".

## 1. Boot 1 — the boot witnesses (the dongle at power-on no longer kills the console or the input: flight 38 §3 CLOSED)
- `[boot] kelf slot=A hash=ok sha=5eaf9242`; `:: USBNET-EHCI: addr=3 mac=9c:69:d3:28:6e:f4 link=up -> armed ::` at 21:24:13Z (the dongle,
  present at enumeration); `:: TPMODE: … latched=yes -> ok ::`; `:: U2.5: FTDI console up ::` at 6.0 s (`BOOTLOG: [6036 ms] ftdi:console-up`).
- First-user form: `[login] key taken by the screen`, `[users] adduser user=una … created=true`, `[keyboot] root keyed user=una … kdf=argon2id
  m=49152KiB t=3 p=4 seal_ms=217`, `[login] submit kdf_ms=3434 adduser_ms=2671 … masked_ms=153` (flight 38's 2.9 s mask is 153 ms now).
- `[login] recovery words shown (KEYBOOT: 24 words, once, on the glass only; Continue needs a click)` at 21:24:35Z. Every key withheld; the
  pointer sprite withheld (`[cursor12] … hidden=1 -> nosprite`); Peter found Continue blind at 21:27:32Z (`[login] press at=(1767,934)
  control=words-continue answered=1 swallowed=3`). **Row: a click-only modal that hides the pointer** (login.rs's own header calls this the wall).
- `[desktop] built at=login ok … pins=8 heap_kb=114835` at 21:27:33Z; `:: OLDEST: board=rmbp12 year=2012 image=1c1c5689 net=absent -> BOOTS ::`;
  `[trust] roots.pem bytes=240216 pin=9cc2a774 -> ok`; `:: USBNET: dhcp=bound ip=192.168.2.4 … gw=192.168.2.1 ::` (narino's bootpd).

## 2. Boot 1 — the list, and the pump loop that ate the pad
`:: BOOTLIST: ran=68 pass=37 fail=21 skip=10 heavy=deferred ms=437955 ::` at 21:34:51Z.
Named reds: INPUTSTALL ×4, HIDSTALL, POINTERFEEL, UNAFSLOCK (`chunked=FAIL hold_max_us=98188`), SHIMMERIDLE, VEILCOST, QUARRYHOVER,
MENUSCRAFT, A11YTREE, A11YSHADOW; GEN7 r11 `REFUSED reason=no-window`; CAMDOT `SKIP(no-device)`; kgr SKIP.
**THE ROW: the usb-pump died 42 times in 8 minutes, every time on its `hda` step.** Shape, verbatim:
```
:: PUMPWEDGE: age_ms=2001 last_step=prtscr holder=- rehome=steal(pump=139@cpu2) deaths=1 -> SEEN ::      (21:27:37Z, 4 s after the desktop)
:: PUMPWEDGE: age_ms=2001 last_step=hda holder=- rehome=wedge(pump=144@cpu3) deaths=2 -> SEEN ::
[hda] start reattach=1 walk=0 crst=0 replayed=27 ms=7733 vendor=0x10134206 … (HDASTART B869)
[pump] step=hda unparked probe=answered parked_ms=10280 skipped=208 (HDARING B811, PUMPPARK B839)
:: PUMPWEDGE: age_ms=2021 last_step=hda holder=- rehome=steal(pump=284@cpu3) deaths=4 -> FAIL ::
:: [sched] RESTART: usb-pump tid=429 on c4 in no queue the re-home could drain -> tid=430 on c3 at its loop head holds=hda.rs:2719 spare=used
```
42 `[sched] RESTART: usb-pump`, 41 `[hda] start` (max `ms=15497`), `[audio] mix sink=hda lost errno=-5` ×16, `[lag] stall … stage=hid`
1–3.7 s between deaths and `stage=queue 17633 ms` at the end. Mechanism as the wire states it: a restarted pump re-enters the hda step from
the top, HDA start re-walks the codec (7.7–15.5 s) and parks the pump ~10 s, PUMPWEDGE's 2 s bound trips, the pump is re-homed, repeat.
HID rides the pump → "trackpad barely functioning". The FTDI RX rides it too: **the serial door was dead all boot (FTDIRX=0)** — the seat's
`tests facetedit` and `tests sonderec`, typed at 115200 from narino, were never seen. After 21:36:14Z the wire went silent (last restart held
`hda.rs:2719`, `spare=used`).

## 3. Boots 2 and 3 — the unlock form: three stack overflows, every time
Boot 2 (21:38:06Z) and boot 3 (21:41:56Z) are identical:
```
[keyboot] hold OPEN root=keyed (the unlock form is up …)
[login] unlock form open (KEYBOOT: … nothing typed is printed)
[keyboot] unlock form up kdf_pending=1 on=worker (KEYBOOT3 …)
[stack] OVERFLOW task=dock-trash stack=… rip=0x77b15fa4 via=df -> task halted   / guard hit size=16384 high=19464 holding=1
[stack] OVERFLOW task=render     stack=… rip=0x77e284fa via=df -> task halted   / guard hit size=32768 high=36904 holding=1
[stack] OVERFLOW task=flightrec  stack=… rip=0x77b15fa4 via=df -> task halted   / guard hit size=16384 high=22040 holding=1   (+5 s)
```
rips resolved (f39-kernel.syms, xseg 0x7777A000 ↔ link .text 0x2d1480): **render** → `<BTreeMap<u64,SetValZST> Iter as Iterator>::next`
+0x10a (a BTreeSet<u64> walk, 4 KiB over its 32 KiB); **dock-trash and flightrec** → `arch::x86_64::syscall::xfer_row_is_clear` +0xd4
(3–6 KiB over their 16 KiB). Boot 1's first-user path never walks this; the keyed-root-unbound path does, on every boot.
Consequences on the glass: no caret, no cursor, no dots (render is dead); Peter's keys arrive (`EHCI-HID: KEY withheld` ×n) but **Enter
never reaches submit** (no `[login] press`, no `unlock submit`, no KDF) — the unlock measure (KEYBOOT2) is NOT taken. At 30 s:
`[users] stage-resolve sub=stage-witness why=no-store at_ms=30004` → `[login] installer: stage=desktop` → `[desktop] built at=stage …
battery=unpainted` — **a desktop ignited under the modal KEYBOOT hold** ("desktop is logged in which is weird").

## 4. Host findings (narino)
- `rust-toolchain.toml` now pins `nightly-2026-10-26` (TOOLCHAIN-ONE B1018); rustup auto-fetched it, rust-src + the two targets had to be added.
- arroyo still carries GNU `stat -c` ×3 and `nproc`; stage-x86.sh `stat -c`/`date -d`; una-card needs mkfs.vfat + mcopy on PATH.
- rmbp-type.sh is Linux-only (`/dev/serial/by-id`, bash `read -n1`); on the Mac the door is `/dev/cu.usbserial-*` at 115200, typed with paced writes.
- Internet Sharing was off after the Mac rebooted; Peter toggled it back (dhcp bound on boot 1).

## 5. Owed / fix briefs
1. **PUMPHDA** — the pump's hda step (HDASTART reattach walk, 7.7–15.5 s, PUMPPARK 10 s) exceeds PUMPWEDGE's 2 s bound; a re-homed pump
   re-enters it from the top → 42 deaths. Either the walk leaves the pump (its own task, off the HID path) or the restart resumes past it.
2. **UNLOCKSTACK** — three deterministic overflows on the keyed-root-unbound path at the unlock form: render (+4 KiB, BTreeSet iter),
   dock-trash and flightrec (+3–6 KiB, `syscall::xfer_row_is_clear`). Find the frame (likely a large on-stack buffer on the no-root path),
   and the per-fixture/per-task stack-high witness PUMPSTACK owed (PUMP-FIXTURE-AUDIT, STACK-POOL-ONE) is the instrument.
3. **HOLDSTAGE** — `stage-resolve why=no-store` at 30 s must not ignite a desktop while `[keyboot] hold OPEN`.
4. **WORDSPOINTER** — the recovery-words sheet hides the pointer and takes only a click; give it a key path or keep the sprite.
5. **DOORONPUMP** — the FTDI RX (the serial door) dies with the pump; the seat's verbs need a door that outlives a pump death.
6. The boot-1 reds (INPUTSTALL ×4 at 2.2 s comp, HIDSTALL, POINTERFEEL, UNAFSLOCK chunked, SHIMMERIDLE, VEILCOST, QUARRYHOVER, MENUSCRAFT,
   A11YTREE, A11YSHADOW) read in boot 1's slice; GEN7 r11 `no-window`.
7. The gate reds in the header (prefs, k8reach, locktype, appearance, status) and the x86-all `Nvme` arm — the seat's, before the next card.

Not flown: SONDEREC (no camera, by Peter's call), FACETEDIT, TEXTSCALE, the Aether dns/tls leg, KEYBOOT2's unlock measure.
