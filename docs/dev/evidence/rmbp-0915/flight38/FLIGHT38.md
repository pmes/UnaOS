# FLIGHT 38 — IMAGE 32 on the UNAFS CARD IMAGE (rmbp12flight38n, hw-rmbp@197971840), 2026-10-27, narino (the late-2019 MacBook) after the Zenbook screen died

Medium: `UnaOS-rmbp-esp-rmbp12flight38n-20261027T2035Z-1979718/unaos-x86-card.img` sha256 7fb5cbe7…, kernel.elf 210bf120…
(23569832 B). **Rebuilt on the Mac**: the Zenbook's staged flight-38 tree never reached the transfer card (only its MANIFEST
line did). Same commit, same 58-knob line (image-line.env + AETHER/MIDDEN/UNA/SONDE + FACETEDIT + the two bunker FW paths),
host rustc nightly-2026-09-01 (1.100.0-nightly 0dfb098f3 2026-08-31). Gates on narino: banner-cert ok=64 missing=0, TRUST
pins = pin, b43.pins MATCH, kepler.pins MATCH, BOOTLIST staged, strings-proof `:: FACETEDIT:`×3 + `:: FACET: gesture=` +
`[facetedit] fixture` + `:: OLDEST:`. Card written by Peter (dd, read-back sha matched). Serial: cu.usbserial-ABAFUJCO,
squawk session rmbp12-flight8. Net: narino Internet Sharing, bridge100 192.168.2.1, bootpd.

**Known gap of this medium vs the Zenbook build:** the Linux self-host fixtures are NOT on the card — SYSKAT1..5.LNX, TCC.LNX,
PROBE.LNX, LIB (musl), rust-probe, ldrun — a Mac host has no Linux linker (`cc` is Apple clang; `strip --strip-debug` is GNU).
Their BOOTLIST rows SKIP/FAIL by construction on this card; that is the host, not the kernel.

Capture (pinned): `f38-boot1.log` (boot 1, from its mark to the sitting close). Boot 2 produced NO WIRE (§3).

## Peter, verbatim (the glass)
- boot 1: "machine seems locked up" (≈20:46Z, ~2 min after the desktop).
- boot 2 (dongle at power-on): "restarted and it's stuck at unlock. no input working".

## 1. Boot 1 — the boot witnesses
- `[boot] kelf slot=A hash=ok sha=210bf120` — the narino build is what flew.
- `:: UNAFSX86: root=unafs disk=sdhc blocks=1048576 gen=29 home=/home -> mounted ::`
- `[users] adduser user=una … created=true` → `[keyboot] root keyed user=una … kdf=argon2id m=49152KiB t=3 p=4 seal_ms=216` →
  `[login] submit kdf_ms=2980 adduser_ms=2006 on=worker input_blocked_ms=0 kind=create` (the first-user form, R134 keyed root).
- `[desktop] built at=login ok bar_ms=46 … pins=11 heap_kb=95965` at 20:44:59Z; `:: OLDEST: board=rmbp12 year=2012 image=19797184-d15d66f net=absent -> BOOTS ::`;
  `[trust] roots.pem bytes=240216 pin=9cc2a774 -> ok` (TRUSTSTAGE flown).
- `:: USBNET: dhcp=bound ip=192.168.2.4 mask=255.255.255.0 gw=192.168.2.1 dns=192.168.2.1 lease=3600 ::` — the Mac's bootpd answered
  (AETHERNET2's first leg). `:: WIFIDHCP: dhcp=NOT ATTEMPTED reason=no-station-link -> ABSENT ::` as designed.
- `:: TPMODE: requests=3 timeouts=0 … latched=yes -> ok ::`; `[tp] mt route=vendor … layout=type2` streaming (TRACKPADMT's parser live).

## 2. Boot 1 — the list, then the wedge
Tally before the wedge: 44 PASS, 5 FAIL, 1 SKIP (kgr, no-verdict), 2 WEDGED.
- FAIL `:: INPUTSTALL: key_queue_max_ms=1 comp_max_ms=832 hid_gap_max_ms=354 strand_pct=0 bound=50 -> FAIL` and
  `:: HIDSTALL: … hid_stall_s=5 … masked_max_ms=2940 -> FAIL` — both measured across the login burst (`masked_max_ms≈2.9 s`, CLOCKCORE agrees).
- FAIL `:: MEMLINE: heap_kb=124467 … logout_delta_pct=29 -> FAIL :: src=rebuild builds=2 first_high_kb=95971` — the heap grew 29% across a desktop rebuild.
- FAIL `:: GLYPHS: … cjk=font8x8 … notdef=9 -> FAIL` — the CJK face fell back to font8x8 on this card (fonts pins MATCHed at build; the face is on the UnaFS volume).
- PASS of note: GATESTEAL2 (`death=c7 … rehome=steal … -> PASS`), UNAFSLOCK, FPU, CLOCKCORE, GEN7 r11, GEN7LADDER, SNTP, SHOTZIP
  (`raw=15553800 deflated=1294779 ratio=12.0x enc_ms=12097 … -> PASS`).

**The wedge (20:46:14Z), verbatim:**
```
[lock] released-on-death name=unafs at=lockowner by=66@cpu? why=guard poisoned=1
[lock] released-on-death name=unafs-ticket at=lockowner by=66@cpu? why=guard poisoned=0
[stack] OVERFLOW task=usb-pump stack=0x2057c508..0x20580508 fault=0x2057bfd8 rip=0x77e5f7c1 via=df -> task halted
[stack] guard hit task=usb-pump size=16384 high=17712 holding=2
[unafs] mount poisoned -> rebind dropped=0 (POISON2 B846 …)
[lock] spin-wait name=allocator.rs:51 at=allocator.rs:64 waiter=12@cpu1 on=untracked ms=22
:: PUMPWEDGE: age_ms=2158 last_step=prtscr holder=- rehome=wedge(pump=66@cpu2) deaths=3 -> FAIL :: pump=66@cpu2 passes=38200 wedges=3 bound_ms=2000
```
Then `[lock] spin-wait name=allocator.rs:51 … ms=124523` (two minutes), `[lock] held-too-long name=text.rs:449 … ms=122440`,
`[tests] watch verb=idlenet … holders=allocator.rs:0@cpu0 state=spin`, `[pump] cable rescue n=40 … outcome=drained` — the kernel
ticked, the pump was dead, the glass was frozen.

**rip resolved** (f38n-kernel.syms, llvm-nm of the staged kernel.elf; runtime xseg start 0x779EA000 ↔ link .text 0x2b00b0):
`0x77e5f7c1 = link 0x725871 → unaos_kernel::drivers::xhci::uvc::fixture +0x581`. The `uvccam` row is on this card's
TESTS.LIST (line 29). PrtScr was only the pump's *previous* step (SHOTZIP had PASSed); the overflow is the UVC fixture.

**Source facts for the fix:** `main.rs:7681 const PUMP_PATH_STACK_SIZE: usize = 32 * 1024;` is what `spawn_prio_stack("usb-pump", …)`
asks for at main.rs:1436, yet the guard reports `size=16384` (0x2057c508..0x20580508 is exactly 16 KiB). Either the request is not
honoured on this path or the pool slot is 16 KiB (sched.rs:95). Two bugs, one wedge: (a) the pump stack is half what the source
says; (b) `uvc::fixture` needs >17.7 KiB of stack and runs on the pump.

## 3. Boot 2 (dongle at power-on) — no wire
After power-key, Peter booted with the Ethernet dongle plugged at power-on (the KEYBOOT2 measure: the unlock form). The capture
holds **nothing** from this boot: no loader line, no kelf line. The Mac's side was intact throughout (FT232R enumerated, squawk
PID 28442 holding the port, port never lost), so the rMBP's kernel never brought its FTDI console up; the Mac's share link
(en6) to the rMBP's dongle stayed inactive; the unlock form took no keyboard/trackpad input.
Hypothesis from boot 1's wire (unmeasured on boot 2): `:: EHCI-HID: [1] STOP-NOTE static int-EP pool exhausted (6) — addr 1
intf 0 ep=IN1 NOT armed ::` / `[usb] ehci=1 addr=1 kind=hub … -> pool-full` was already hit with the dongle plugged *late*; with
the dongle present at enumeration the FTDI console and the HID endpoints are the next things not to get a slot.
Decisive next boot: dongle OUT at power-on → does the wire come up and the unlock form take Enter? Then hot-plug.

## 4. Host findings (the move to the Mac) — tooling, not kernel
- `unaos/arroyo` grew GNU-only `stat -c %s` (3 card-path sites: 9851, 9882, 9941) and `nproc` (4003); `tools/stage-x86.sh` uses
  `stat -c`, `date -d`. media-writer.sh and make-pi-img.sh already carry the `stat -f || stat -c` form — port the rest.
- `tools/una-card` needs mkfs.vfat + mcopy on the host (Homebrew dosfstools + mtools on the Mac).
- The repo's recent flights were built on nightly ≥ 2026-08-31; the Mac had a March nightly. `rust-toolchain.toml` pins `nightly`
  undated — consider a dated pin.
- squawk `mark` without `--session` dies on a stray file in capture/ (FileExistsError); `--session` works.
- waker.conf patterns: awk `-v` eats backslashes, so `\[x\]` must be written `[[]x[]]` (a false-fire loop cost three re-arms).

## 5. Owed / fix briefs (for the remote executors)
1. **PUMPSTACK**: make the pump's stack the 32 KiB the source already asks for (find why `spawn_prio_stack` yields 16 KiB), and
   move `uvc::fixture` (and audit the other pump-hosted fixtures) off the pump or under its budget; a stack-high witness per fixture.
2. **DONGLEBOOT2**: reproduce the dongle-at-power-on boot in QEMU with an extra interrupt-endpoint device at enumeration; the
   int-EP pool (6) must not starve the FTDI console or the internal HID; a witness that names who got no slot.
3. **INPUTSTALL/HIDSTALL at login**: masked_max 2.9 s across the first-user KDF (argon2id 48 MiB, kdf_ms=2980 on=worker) — the
   form's input is blocked while the KDF runs; measure, then move the hold off the HID path.
4. **MEMLINE** 29% rebuild growth; **GLYPHS** cjk=font8x8 on the card image (the face is staged to /system/fonts on p2 — check the lookup).
5. **Host portability** (§4) so a Mac bench can build the flight image without shims.
6. **Linux fixtures on a Mac host**: a cross link (lld-based or a static x86_64-linux gcc) so a Mac-built card carries the LNX rows.

Not flown on this card: FACETEDIT (`tests facetedit`), TEXTSCALE, the Aether dns/tls leg, KEYBOOT2's unlock measure — all behind the
boot-1 wedge and the boot-2 silence.
