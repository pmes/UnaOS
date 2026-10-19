# BP-audio — sinks, HDA, USB Audio Class 1 (read at 96ed4711 + UAC1, rmbp-ledger B706)

## 1. What it is
- ONE seam: `unaos/crates/kernel/src/audio/mod.rs` (cfg `uvc` or `hda-tone`). `audio::sink(name)` chooses where sound goes, `audio::tone_on(sink, hz, ms)` plays to it; the choice is Principia's `system.audio.output` (`hda` | `usb`).
- HDA (x86, `hda` + `hda-tone`): the Intel HDA controller walk and the output stream (`unaos/crates/kernel/src/drivers/hda.rs`, `hda_stream.rs`, `hda_play.rs`, `hda_amp.rs`); the tone is a test (R77), `tests hda`.
- USB Audio Class 1 playback (cfg `uvc`): `unaos/crates/kernel/src/drivers/usb/uac.rs` over the host-neutral `uvc::IsoHost`, carried by xHCI isochronous OUT in `unaos/crates/kernel/src/drivers/xhci/isoch.rs` (the XHCIISO engine, B534, now both directions).
- Settings > General, the Sound box's Mute row: the output picker `HDA internal | USB <vid:pid>` (`unaos/crates/kernel/src/video/settings.rs:952` paint, `:1718` press).
- The decoders (WAV/FLAC/Ogg/MP3/AAC) are `audio_core` (shared, no_std); the Player and Stria feed the HDA stream (`drivers/hda_play.rs`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-audio-1 | One sink seam: a sound the kernel plays names a `Sink` and goes through `audio::tone_on`; HDA is reached from it by ONE call (`hda_tone_test_default`), USB by the class driver's ring. No second router. | `unaos/crates/kernel/src/audio/mod.rs:126` `unaos/crates/kernel/src/audio/mod.rs:135` `unaos/crates/kernel/src/drivers/hda.rs:2464` | `tests uac1` |
| BP-audio-2 | The sink choice is a PREFERENCE: Principia's `system.audio.output`, set only by `audio::sink`; a USB choice holds only while a USB sink is registered, else the internal sink plays. | `unaos/crates/kernel/src/audio/mod.rs:30` `unaos/crates/kernel/src/audio/mod.rs:70` `unaos/crates/kernel/src/audio/mod.rs:95` | `[audio] sink=… chosen` line |
| BP-audio-3 | Nothing makes a sound at boot: the HDA tone runs from `tests hda` (R77); a USB stream starts silent and carries samples only while a writer holds the ring. | `unaos/crates/kernel/src/drivers/hda.rs:2464`; `unaos/crates/kernel/src/drivers/usb/uac.rs:298` | `[hda] tone deferred` line; `underruns=` stays 0 at idle |
| BP-audio-4 | The USB ring is single-producer single-consumer on atomics: the consumer is the isochronous OUT TD fill, reached from the transfer-event claim, and takes no lock of the class driver. | `unaos/crates/kernel/src/drivers/usb/uac.rs:298` `unaos/crates/kernel/src/drivers/usb/uac.rs:337`; `unaos/crates/kernel/src/drivers/xhci/isoch.rs:375` | unheld |
| BP-audio-5 | Isochronous OUT is the XHCIISO engine's other direction: EP Type 1 at DCI ep*2, its own pipe and armed word (a camera and a speaker stream side by side), TD = the bytes one interval plays, frame-ID scheduled with the same SIA fallback. | `unaos/crates/kernel/src/drivers/xhci/isoch.rs:64` `unaos/crates/kernel/src/drivers/xhci/isoch.rs:476` `unaos/crates/kernel/src/drivers/xhci/isoch.rs:690` | `[usb-iso] cfg-ep … type=isoch-out` line |
| BP-audio-6 | The class driver configures the host before the device: Configure Endpoint, then SET_INTERFACE(alt); a refusal leaves the device at alt 0. UAC2 (bcdADC 2.00) is refused by name. | `unaos/crates/kernel/src/drivers/usb/uac.rs:398` | `[uac] abort … reason=` line |
| BP-audio-7 | One format is played: PCM, Type I, 2 x 16-bit, 48 kHz (192 bytes per full-speed frame); an alternate that does not carry it is not picked. | `unaos/crates/kernel/src/drivers/usb/uac.rs:258` `unaos/crates/kernel/src/drivers/usb/uac.rs:269` | `tests uac1` fixture (mono refused) |
| BP-audio-8 | An audio device is a SINK, never an input decoder: it reaches neither `pal::push_event` nor the HID path; a root-port audio device with no HID interface releases the port itself. | `unaos/crates/kernel/src/drivers/xhci/isoch.rs:156`; input.md BP-input-15 | `[usb-iso] audio candidate … queued` line |

## 3. Seams
- `audio` may call: `drivers::hda` (one call), `drivers::usb::uac` (begin/write/finish/queued/counters), `prefs` (the one key).
- `uac` may call: `uvc::IsoHost` (its host), `audio::usb_arrived` / `usb_gone`. `isoch` calls `uac::pull` / `out_td_len` / `cfg_has_audio` / `probe`.
- Settings calls `audio::picker` and `audio::sink` only.

## 4. Wire
`[usb-iso] audio candidate slot= path= queued` · `[uac] dev=<vid:pid> … alts=` · `[uac] alt … ours=` · `[usb-iso] cfg-ep … type=isoch-out` · `[uac] stream … -> streaming` · `[audio] usb vid=<x> pid=<x> fmt=pcm16/2/48000 isoch_pkt=<n> underruns=<n>` · `[audio] sink=<label> chosen` · `:: UAC1: enumerated=<0|1> stream=<ok|…> frames=<n> underruns=<n> -> PASS|SKIP|FAIL ::` · `:: HDA-TONE: … ::`.

## 5. Ruled, unbuilt / open
- Feedback (asynchronous) endpoints: a 48-frame-per-ms stream is sent unmetered; an async device's clock drift is unhandled. UAC2 (high-speed, clock entities): refused.
- The Player, Stria and the alert sound still write the HDA stream directly; moving them onto `audio::sink` is owed. Volume/mute reach HDA only (no feature-unit SET_CUR on USB).
- The picker has no keyboard control index yet (click only); its own `kernel feature` is `uvc` (no `uac` knob in Cargo.toml).
- QEMU: `UNAOS_UVC=1 UNAOS_USBAUDIO=1` attaches `usb-audio` (`unaos/arroyo`); the rmbp bench flight with a USB speaker is unflown.

## 6. Read further
- `docs/dev/OS/11_AUDIO/hda.md` (the HDA walk and tone), `docs/dev/OS/07_USB_STORAGE/uvc.md` (XHCIISO, the IsoHost shape).
- bar.md BP-bar-25 (the user's hardware is the user's: the audio class).
