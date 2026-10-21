# FLIGHT 37 — IMAGE 30 on the UNAFS CARD IMAGE (rmbp12flight37, hw-rmbp@3340e13c), 2026-10-21, main bench — IN PROGRESS

Medium: `UnaOS-rmbp-esp-rmbp12flight37-20261021T1848Z-3340e13` (.card.img sha256 e1a3e3ac…). Tree = the cloud tip 3340e13c ("green on both legs, boot that"); hw-rmbp fast-forwarded 381 commits past image 28. Line = image 28's 56 knobs + UNAOS_A2DP=1 = 57. banner-cert ok=62 missing=0. No battery before the boot (Peter: "i do not want to wait for the gating lets boot it while the gates are run").

## Peter, verbatim (the glass)
- "desktop. splash and shadows good"
- "tap to click not working but i did see a wedge notice earlier."
- "notification pop-up getting leak through flickers of the background."
- "window shadow fails to update all windows on accessibility color inversion."
- "text overwriting other text when increasing text size and largest size has rendered the system unusable."
- "window resizing not possible i'm not sure if it's that fine mouse movements are nearly impossible. when i try to click the cursor makes big jumps."
- "plugged in dongle says network but google not found"

Capture: `f37-boot1.log` (pinned at the end of the boot).

## 1. Boot 1 — the boot witnesses (all green)
- `:: SPLASH: arch=x86_64 WxH=2880x1800 ms=175 scene=1 wall_ms=150 -> painted ::` (STARTSCENE: the scene from the first frame; `:: STARTSCENE: gen_count=1 splash_scene=1 gop_stars=ok hold_ticks=2 login_backdrop=picture -> PASS`).
- `[clock] source=tsc … civil=tsc` (CLOCKX86); `:: CLOCKCORE: … cores_ticking=8/8 -> PASS`.
- `[desktop] built at=login ok … pins=11 heap_kb=96608` at 18:56:28Z; `:: OLDEST: board=rmbp12 year=2012 image=3340e13c net=absent -> BOOTS ::`.
- `[wifi6] sprom mac=14:10:9f:d7:05:67 src=sprom rev=9 plausible=1 oui=apple board=0x00ef … -> PLAUSIBLE` (WIFIMAC: the S2r read; WIFI10's no-station-mac is over).
- `[wcser] pump step=store-services gate=released declined=2 redriven=1` (GATESTEAL2's long-step rule, at 18:55:24Z, before login).

## 2. The big reads (typed over the door)
- **GATESTEAL2 PASS.** The steal came as before (`dead=c7` on the prtscr step), and the pump RESTARTED: `:: [sched] RESTART: usb-pump tid=9 on c7 in no queue the re-home could drain -> tid=110 on c2 at its loop head`, then a second death `tid=110 on c2 -> tid=113 on c3`. `:: GATESTEAL2: death=c7 current=9 moved_current=1 rehome=steal pump_alive_ms=1 gate_released_steps=1 -> PASS :: … door=alive revenant=exited … deaths=2`. `:: PUMPWEDGE: age_ms=2000 last_step=prtscr holder=- rehome=steal(pump=110@cpu2) deaths=1 -> SEEN` — this is the "wedge notice" Peter saw on the glass. The door stayed alive through both.
- **UNAFSLOCK PASS** (HIDSTALL2): `ops=53 held_max_ms=1 io_under_lock=0 … path=/apps/AETHER.ELF bytes=3276896 read_ms=1253 read_held_us=71` — the 1.1 s hold is a 71 µs hold.
- **HIDSTALL FAIL** stays: `halted_at_login=0 recovered=0 proxy_retired=2 hid_stall_s=72 key_queue_max_ms=0 masked_max_ms=479 -> FAIL`. Not the unafs hold any more (`[hid] stall census holders=0 worst_ms=0 -> CLEAR`); it is the trackpad read below.
- **FPU PASS**: `osfxsr=1 ctx_bytes=512 tasks=2 switches=127 xmm_survived=ok mode=eager`.
- **GEN7**: `:: GEN7: r11 draw=quad src=3 rt=1840x1120 sampled=ok mismatch=0 landed=1 -> PASS` (R11's window sampled through the 3D pipe), R8/R9/ladder PASS, `GEN7BLIT … -> candidate`, `KCOMP … fastest=cpu`.
- **KGR DECLINED** as on image 28: `fw=MATCH ext-start=ok golden=refused(topology tpc_nr[0]=00040002) … fecs=running ucode=running` (KEPLERGOLD still owed).
- **WIFI1 PASS** `ucode=ok d11=up scan=0`; WIFI10's tx line still to read.
- **Screenshots**: the door's `screenshot` → `:: PRTSCR: SCREEN1.PNG 2880x1800 1253235 bytes -> OK` (SHOTZIP 12.4x, SHOTMOUNT via vfs); Peter's Cmd-Shift-3 → SCREEN0.PNG 1446669 bytes OK, IMGVIEW opened it. Both on the card.
- STARTSCENE, CONTRAST (`min_ratio=7.2`), WINKEY, SETTINGSMAP (`keys=47 rows=45 orphans=0`), TOASTCAP (`posted=12 shown_max=3 queued=9 dropped=0`) PASS. SETTINGS `tabs=16`.
- **Network**: dongle after login → `:: USBNET: dhcp=bound ip=10.42.0.160 mask=255.255.255.0 gw=10.42.0.1 dns=10.42.0.1 lease=3600 ::`, `:: SNTP: link=usbnet synced=1 offset_ms=1000 -> PASS` (the clock set from 159.203.158.197 over the dongle: the first SNTP sync on this machine).

## 3. Peter's six reports — what the wire says (TAKEN AS GIVEN)
1. **Tap to click / fine movement / big jumps — ONE ROOT: the vendor multitouch parse.** `[tp] mt route=vendor ep=1 … ids=02:1,44:0,other:31699` — of 31,700 trackpad frames, ONE carried report id 0x02 and every other frame is `other` (unparsed id). While Peter has one finger down the line says `fingers=3`, `fingers=4`, `fingers=7`, `mover=0 dx=0 dy=0`; the only taps decoded were `[tp] gesture kind=tap fingers=3 … button=2` (three times: a one-finger tap read as a three-finger tap = button 2, never a left click). `kind=tap fingers=1` occurred ONCE in the boot. The `[ptr] fine` line has `moved=21214` frozen from 19:16:10Z while `n` and `clamped` keep climbing (clamped 86→168 in 20 s): the reports arrive, the movement is clamped away, and the occasional accepted frame is a jump (`rebase` 66→81). HIDSTALL's `hid_stall_s=72` is this. Row: TRACKPADMT — the vendor report ids on the 2012 rMBP (05ac:0252 family) are not the ones the parser keys on; decode the actual id in the stream (the `other` id), then fingers/tap/fine motion follow.
2. **Notification pop-up leaks flickers of the background.** `[cursor11] compose-through … flicker_frames=0` says the compositor's own meter saw none — so the toast's paint path bypasses the meter: the toast rect is presented between the backdrop pass and the toast pass (TOASTCAP posted=12 shown_max=3). Row: TOASTFLICKER — the toast composes in one bracketed pass or not at all; a fixture that reads back the toast rect mid-present.
3. **Window shadows not updated on colour inversion.** `[a11y] filter=invert pass_us=~9000 motion=full presents=6-9 px=27-35M why=tick` — the invert pass runs on the ticked presents only; the shadow tiles (WINEDGE2: laid on the desktop layer's base) are cached and never re-laid under the filter, so windows that did not present keep un-inverted shadows. Row: A11YSHADOW — epoch-bump the shadow cache on a filter change (CONTRAST already does `epoch_bumps=2` for its looks).
4. **Text size: overwriting, largest unusable.** `[settings] accessibility.text_size=largest (26 px) applied=1` → `[kfont] restyle font=sans font_size=26 ppi=221 chrome=inter-32.75 ui=inter-52.75 console=jetbrains-mono-46.50`: the UI face goes to 52.75 px while the rows, bars and controls keep their px geometry, so glyphs overwrite neighbours. Row: TEXTSCALE — text size must reflow the layout (row heights, bar height, control boxes scale with `ui`), and the largest step is capped to what the 2880x1800 panel holds; a fixture that measures overlap at each step.
5. **Window resizing not possible.** The edge zones ARE found (`[cursor] zone=ne shape=resize-nesw win=3`, `zone=se`, `zone=nw`, `zone=e`, `zone=n`) — the cursor changes, the grab never lands, because the press arrives through the broken MT parse (item 1: a press is read as a 3-finger gesture, or the fine motion is clamped so the drag never crosses the threshold). Re-read after TRACKPADMT; no separate row yet.
6. **Dongle says network, google not found.** DHCP is bound with `dns=10.42.0.1`, SNTP reached the Internet. Aether: `:: AETHER: net url=https://google.com/ status=0 bytes=0 ms=469 tls=off proto=h1 hops=0 conn=new -> FAIL`, `nav -> FAIL reason=no-page code=-5`. NO `[dns]` line follows the nav anywhere on the wire, and `tls=off` on an https URL: Aether's fetch has no resolver call and no TLS leg on this build. Row: AETHERNET — resolve through the DHCP-given DNS (smoltcp has the socket), then TLS for https (rustls is in the host lock; the kernel-side client is the arc).

7. **"shell font size did not increase." / "aether input font size did not increase."** (Peter, after the text-size step.) `[kfont] restyle font=sans font_size=26 … console=jetbrains-mono-46.50` names a console size, but the repaint that follows lists `console_rows=0 windows=quarry,settings,activity,fileview,textedit,login` — the shell and Aether are not in the repaint set, and `[prefsbus] subscribe … prefix="system.appearance." / "aether.tabs."` are Aether's only subscriptions: it never hears `system.display.font_size` or `system.accessibility.text_size`. Row: TEXTSCALE2 — the shell's console face and Aether's omnibox/input faces follow the display font size (the shell from kfont's console size; Aether by subscribing to `system.display.` on the prefsbus); the fixture reads both faces after a set.

(Section 4: the rest of the list, boot 2 — to follow.)
