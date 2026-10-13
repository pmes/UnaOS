# STRIAMETAL — Stria plays natively (LEDGER SR139, rmbp-ledger B618)

Peter, 2026-10-14: "which is the base handler that takes care of playing video? throw getting it up to speed
natively into the mix." CODEX §2: Stria owns A/V (Amendment II: Aether renders, Stria plays). Until this arc the
image carried no Stria: the host handler (handlers/stria) plays through gneiss_pal's dsp arm, and the kernel's Player
(video/player.rs + video/vplay.rs, "CHARTER: Stria — fulfiller") played video in a kernel window. This arc puts Stria
itself on the image as one ring-3 program, `APPS/STRIA.ELF`, over the same no_std cores, and gives ring 3 a sound
output to clock it by.

Branch `exec-rmbp-striametal`, base `0d88e0c9`. Commits: the ABI and kernel arms; the program's core; the transport,
keys, pointer and association; the fixture; the knobs, staging and docs (this file).

## 1. The ABI (una-abi STRIAMETAL block, crates/una-abi/src/lib.rs tail)

| nr | verb | arguments | answer |
|----|------|-----------|--------|
| 73 | `SYS_AUDIO_OPEN` | rate (8000..=48000 Hz), channels (1, 2), bits (8 unsigned, 16 signed LE) | handle > 0; -EINVAL format; -EEXIST the caller holds one; -EBUSY another program's stream or the kernel player sounding; -ENODEV no sink (refused by name on the wire) |
| 74 | `SYS_AUDIO_WRITE` | handle, buf, len (<= AUDIO_WRITE_MAX 16 KiB, whole frames) | bytes taken; -EAGAIN ring full or still arming (offer them again); `len == 0` = end of stream (0); -EBADF; -EIO the stream died |
| 75 | `SYS_AUDIO_CLOSE` | handle | 0 (stopped now, the output free); -EBADF |
| 76 | `SYS_AUDIO_POS` | handle, what: AUDIO_POS_PLAYED 0 / AUDIO_POS_UNDERRUNS 1 | source frames PLAYED since the stream ran, capped at the frames written (an underrun holds the clock), or the underrun count; -EBADF, -EINVAL, -EIO |

New constants: `AUDIO_POS_PLAYED`, `AUDIO_POS_UNDERRUNS`, `AUDIO_RATE_MIN`/`MAX`, `AUDIO_WRITE_MAX`, `EBUSY` (-16).
The const-assert chain continues from SHELLABI's `SYS_KILL` (72): 73..=76 consecutive.

One stream per program and one system-wide (the HDA output stream `drivers/hda_play.rs` drives is one ring).

### Arches

- **x86** (`arch/x86_64/syscall.rs` tail, dispatch folded onto the `SYS_KILL` line): over `drivers::hda::play`'s
  ring-3 stream (`hda_play.rs` tail, `r3_open` / `r3_write` / `r3_close` / `r3_pos` / `r3_tick`), compiled with
  `hda-tone` (UNAOS_HDATONE). The open is LATCHED and brought up on the device-service tick (`service`, folded at the
  top of `hda::probe_after_root`): `play::start` re-runs the controller probe, which does not belong on a ring-3
  syscall's 16 KiB kernel stack. The key is RING3PIPE's (B609): the caller's slot and `SLOT_GEN`; the tick reaps a
  stream whose owner's generation moved. `start` bumps an epoch (one LINE-NEUTRAL fold), so the kernel player
  re-arming the output ends the ring-3 stream (`reason=superseded`) instead of being fed by it. Without `hda-tone`:
  `refused=no-hda-stream`, -ENODEV.
- **aarch64** (`arch/aarch64/syscall.rs` tail, dispatch folded onto the `SYS_SPAWN_PATH` line): refused by name —
  `[audio] open … -> refused=no-aarch64-sink (pi4: hdmi+pwm audio, no driver; orin: tegra hda behind the ape, no
  driver; virt: no audio device)`, -ENODEV; the other three verbs -EBADF.

| board | sink | today |
|-------|------|-------|
| rMBP (x86) | Intel HDA, Cirrus codec (the PLAYWAV ring) | plays (UNAOS_HDA + UNAOS_HDATONE on the image line) |
| x86 QEMU | `-device intel-hda -device hda-duplex -audiodev none` (attached under UNAOS_HDA) | plays to the null audiodev; LPIB walks |
| Pi 4 | HDMI audio (VC4) and the 3.5 mm PWM | no driver: refused by name, wall clock |
| Jetson Orin | Tegra HDA behind the APE (AHUB) | no driver: refused by name, wall clock |
| QEMU virt | none | refused by name, wall clock |

### Wire

```
[audio] open pid=<p> rate=<r> ch=<c> bits=<b> -> ok handle=<h>
[audio] open pid=<p> rate=<r> ch=<c> bits=<b> -> refused=<why> errno=<e>          (x86)
[audio] open pid=<p> rate=<r> ch=<c> bits=<b> -> refused=no-aarch64-sink (…)      (aarch64)
[audio] arm handle=<h> eff_rate=<e> resampled=<0|1> -> ok
[audio] arm handle=<h> eff_rate=0 resampled=0 -> refused=start-failed (<why>)
[audio] close handle=<h> pid=<p> reason=<close|owner-exited|superseded> frames=<f> underruns=<u>
```

## 2. The program (crates/user-stria)

- **Shape**: user-aether's (SR66): own `[workspace]`, build-std, linked in the ELF window (user-stria-x86.ld at
  0x10000200000, user-stria.ld at 0x7840200000), 256 KiB stack, EXECNAME's windowed app note, APPRES block
  (res/app.res: Stria, org.unaos.stria, the media doctypes; res/icon.svg, UnaOS's own design).
- **Pipeline**: the file (argv[1], else a Quarry BUS_VERB_NOTICE, else the staged TEST.WEBM) read whole after a
  SYS_STAT (one allocation, FILE_CAP 24 MiB) -> `demux_core::probe` -> `Demuxer` (MP4, Matroska, WebM) or a bare
  sound file -> `VDec` (vp8_core, vp9_core, av1_core, h264_core) and `Sound` (audio_core, the container's track through
  `container::open_demuxed(d.share())`) -> pictures converted straight into the 288 x 260 video rect (BT.601 limited,
  integer, nearest, letterboxed) -> SYS_WIN_PRESENT.
- **Clock**: the sound's (`base + SYS_AUDIO_POS * 1000 / rate`), held at the first picture until the stream runs
  (<= 2 s), the wall otherwise (a refused, dead or never-running stream; after the sound has played out).
- **Drops**: a picture whose slot the clock has passed is decoded (references) and counted, never converted.
- **Transport**: play/pause, `m:ss/m:ss`, the track (click seeks), mute and five volume steps; Space, Left/Right
  (-/+5 s), Up/Down (volume), `m` (mute), Esc; the pointer per AETHERPTR B543 and its shape per CURSORSHAPE B550.
- **Heap budget**: file + w * h * per-pixel (VP8 6, VP9 30, AV1 28, H.264 26 bytes) + 4 MiB over 52 MiB is refused
  by name (`:: STRIA: open path=… -> FAIL reason=frame-over-heap size=WxH need_kib=… heap_kib=… ::`). A 1920x1080
  VP8 file under 8 MiB fits (~20 MiB); VP9/AV1 1080p (~60 MiB) is refused; 720p of every codec fits.

### What decodes on the image

| | codecs |
|---|---|
| containers | MP4 (progressive, fragmented), Matroska, WebM (demux_core) |
| video | VP8; VP9 profiles 0 and 2 (8/10/12-bit 4:2:0); AV1 (av1_core); H.264 Baseline/Main/High 8-bit 4:2:0, avcC or Annex B |
| sound in a container | Opus and Vorbis in Matroska/WebM; AAC and MP3 in MP4 (`audio_core::container::plays`) |
| bare sound files | WAV, AIFF, FLAC, Ogg (Opus, Vorbis), MP3, AAC (ADTS) |
| not | HEVC (no core), FLAC/PCM tracks inside Matroska and MP4 (named on the window, the pictures still play) |

### ELF sizes (stripped, `llvm-objcopy --strip-all`, all four video cores linked)

| arch | build | bytes |
|------|-------|-------|
| x86_64 | user-stria-x86.ld, x86_64-unknown-none.json | 1,023,224 |
| aarch64 | user-stria.ld, aarch64-base.json | 909,744 |

AETHER.ELF sits at ~3.2 MB; the aarch64 loader's file cap is 12 MiB (arroyo's fit check), x86's is the 64 MiB
window. x86 program headers: R+X 0x10000200000 (761,062 B at the core commit), R, RW; PT_GNU_STACK 262,144;
entry 0x10000200000.

## 3. The fixture — `tests stria` (crates/kernel/src/striaapp.rs)

aether.rs's shape over the ARMTESTS launch facade, both arches. The clip is builder/testf.list's existing
`TEST.WEBM` (Chromium's bear-320x240.webm: VP8 + Vorbis, BSD-3-Clause, sha256 834dbada…) — no clip had to be
generated. The arm media stage it from the builder's fetch (`target/testf-cache/TEST.WEBM`) when present; otherwise
the fixture SKIPs `no-clip` by name.

Expected wire on x86 QEMU (UNAOS_STRIA=1 with the image line's HDA knobs):

```
[striaapp] bytes=<~1.0 MB> entry=0x10000200000 segs=3 stack=262144 note_flags=Some(1)
[appres] sight path=/apps/STRIA.ELF res=yes source=elf …
:: STRIA: open path=/system/test-f/TEST.WEBM tracks=v:vp8 a:vorbis dur_ms=<~2700> size=320x240 bytes=<n> ::
[audio] open pid=<p> rate=44100 ch=2 bits=16 -> ok handle=1
:: STRIA: audio handle=1 rate=44100 underruns=0 state=open ::
[audio] arm handle=1 eff_rate=<44100|48000> resampled=<0|1> -> ok
:: STRIA: frame n=30 pts_ms=<~1000> late_ms=<n> ::
:: STRIA: frame n=60 pts_ms=<~2000> late_ms=<n> ::
:: STRIA: audio handle=1 rate=44100 underruns=<u> state=end ::
:: STRIA: end -> PASS frames=<n> audio=ok drift_ms=<n> late_max_ms=<n> dropped=<n> clock=audio ::
[audio] close handle=1 pid=<p> reason=close frames=<f> underruns=<u>
[striaapp] pid=… slot=… entry=0x10000200000 clip=/system/test-f/TEST.WEBM … tail=Some("PASS frames=…")
:: STRIAAPP: image=/apps/STRIA.ELF window=elf frames=<n> audio=ok drift_ms=<n> -> PASS ::
```

On aarch64: `[audio] open … -> refused=no-aarch64-sink (…)`, `:: STRIA: audio handle=-19 … state=refused ::`,
`clock=wall`, `audio=none`, still PASS on frames > 0.

## 4. Knobs (LAWS: "a knob lives in FIVE places")

| place | change |
|-------|--------|
| unaos/arroyo | `[ -n "${UNAOS_STRIA:-}" ] && _feats="${_feats}stria,"` after aether; the K8_FEATS arm after aether; build_user_stria_x86 / _aarch64; the x86 build beside every build_user_aether_x86; stage_jetson_el0_images, kernel8 and the virt app stick stage STRIA.ELF (+ TEST.WEBM when fetched); USER_CHECK_MATRIX rows; `stria` on x86-image, arm-tegra-image, arm-pi-image, arm-virt-apps-min |
| crates/kernel/Cargo.toml | `stria = ["selfdiag"]` |
| builder/src/main.rs | `UNAOS_STRIA` -> `stria`; APPS/STRIA.ELF on the ESP and the DATA volume |
| scripts/banner-cert.sh | `stria\|:: STRIAAPP: image=\|-\|unmeasured-here` (the literal is striaapp.rs's verdict format's head, 19 bytes, compiled wherever `stria` is: x86, and aarch64 through `baremetal`/`tegra_el0` -> `aarch64_el0`) |
| scripts/k8-reach.registry | no row: the knob HAS a K8_FEATS arm (a row would be `k8-reach CONTRADICTION`) |
| image lines | `UNAOS_STRIA=1` in image-line.env (after LUMEN, arroyo's map order), image-line-orin.env (after AETHER), image-line-pi.env (after AETHER, K8_FEATS order) |

## 5. Owed and stopped

- **STOP (file outside the brief): the dock pin.** `system.dock.pins`' doc names `stria` (prefs_core schema +
  PREFS-SCHEMA.md), but the pin itself is a `DpSpec` row in `crates/kernel/src/video/dock.rs:3238` and a bit in
  `DP_ALL` (:3243). The exact change: append `DpSpec { name: "stria", initial: b'S', kind: DpKind::Ring3Program,
  verb: "/apps/STRIA.ELF", id: wm::WinId::MAX - 12 },` after the `una` row on :3238 (before its `/*`), and
  `& !(if cfg!(feature = "stria") { 0 } else { 1 << 10 })` on :3243 (before its `/*`) — index 10, the next after
  una's 9; `WinId::MAX - 12` is free at 0d88e0c9 (`grep -rn 'MAX - 12' crates/kernel/src/video/`: 0 hits; MAX - 8 is
  TRASH_PIN_ID, MAX - 9/10/11 are aether/midden/una).
- The association moves a type only where no `una:preferred` is STORED (fs/assoc.rs tail): a registry built before
  STRIA.ELF's first sight names the Player; `assoc video/webm org.unaos.stria` (or the Settings File Types row)
  moves it. A boot-time sight of /apps/STRIA.ELF before the registry build would close it (fs/appres.rs, not this
  arc's).
- The fixture's run, the knoboff measurement and the gates are the seat's at the fold (R107).
