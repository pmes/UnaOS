# SEEKTABLE2 (rmbp-ledger B469) — Ogg and ADTS seek from their own data; Opus inside WebM

CHARTER: Stria — shared-core (`unaos/libs/media/audio_core`, the decoders both rings link; R79). The kernel stays the
fulfiller (`video/vplay.rs`, `drivers/hda_play.rs`, `video/player.rs`: existing files). No new kernel file, no knob.

**Finding.** SEEKTABLE (B433) left `Source::seek` at its default `Ok(None)` for `OggOpus`, `OggVorbis`, `OggFlac` and
`AdtsStream`: `play-dec` falls back to `seek_skip` (`[play] seek … method=decode-skip table=none`). `vplay::container_audio`
refuses every Matroska sound but Vorbis (`[vplay] sound codec=opus -> none`), although audio_core carries the Opus
decoder (`audio_core::opus`, RFC 6716, bit-exact KATs) — the decoder exists; only the packet door was missing.

**The seam.** All in audio_core (one implementation, both rings):
- `ogg::OggReader::bisect(target_granule)`: a bisection over byte offsets — at each probe the next CRC-checked page of
  the locked serial that completes a packet (granule ≠ −1) — then a forward page walk; returns the last page Q with
  `granule(Q) ≤ target`. `OggReader::resume(Q)` re-reads Q and drops the packets completing on it, so the next
  packet starts at sample `granule(Q)` exactly (RFC 3533 §6: a page's granule is the end of its last completed packet).
- **Opus** (RFC 7845 §4.6): target granule − 80 ms (3840) pre-roll; the decoder restarts fresh at `granule(Q)`; the
  sample is known exactly (`table=ogg exact=1`), the PCM converges within the pre-roll.
- **Vorbis**: the last packet completing on Q is decoded as the one-block pre-roll (its output is the overlap, none),
  so the next packet's output starts at `granule(Q)` — bit-exact with the full decode.
- **Ogg FLAC**: frames are independent; restart at `granule(Q)`, bit-exact.
- **ADTS** (ISO/IEC 13818-7 / 14496-3 §1.A.2): no timestamps, so the index is a header walk (no decode): every
  `syncword`+`frame_length` from the first frame, every 16th frame kept as (byte, sample), built once on the first
  seek; a seek walks ≤ 16 headers from the stride point to frame k, restarts the decoder fresh at frame k−1 (one AU
  pre-roll for the MDCT overlap) → `table=adts exact=1`.
- **Opus in Matroska/WebM**: `opus::OpusPackets` — a container-free Opus source over a `Packets` trait (OpusHead =
  CodecPrivate, pre-skip = CodecDelay when the track gives it, else OpusHead's), the same decode/gain/pre-skip rules
  as Ogg Opus. The kernel's `vplay::container_audio` feeds it demux_core's track packets (one adapter, no decoder).

**Milestones.** M1 Ogg bisection + Opus/Vorbis/FLAC-in-Ogg seek. M2 ADTS header-walk index. M3 `OpusPackets` + the
kernel's Matroska Opus arm. M4 host KATs (TEST.OPUS, TEST.OGG, TEST.AAC, a WebM remuxed from TEST.OPUS's packets by
demux_core's writer). M5 kernel: `tests player` probes the staged TEST.OGG/TEST.OPUS/TEST.AAC through the core's
table (open + seek, over the kernel's `VfsSrc`), and the hda_play/player docs drop the decode-skip for Ogg/ADTS.

**Witness (the wire a metal boot prints).** A scrub from the Player on an Ogg or ADTS file:
`[play] seek to_ms=<n> landed_ms=<n> method=table table=<ogg|adts> exact=1 byte=<n> sample=<n> jid=<n>`;
`tests player` → `[player] coded seek path=/system/test-f/TEST.OGG table=ogg exact=1 to_ms=200 landed_ms=200 byte=<n> sample=<n>`
(and TEST.OPUS, TEST.AAC) then `:: PLAYER: open=ok transport=ok seek=ok method=table … coded=ogg:table,opus:table,adts:table -> PASS ::`;
a WebM with an Opus track: `[vplay] sound container=matroska codec=opus rate=48000 ch=<n> … -> play-dec`.

**Owed.** Chained Ogg (a second BOS) still ends at the chain boundary; Ogg Opus multistream (mapping family 1/255);
Matroska DiscardPadding (the Opus end trim inside WebM: the tail plays its padding, ≤ one packet); seeking the
Matroska sound itself (the video seek is VIDEOPLAYER's, with demux_core's `seek_track`); AAC/MP3 beside a video track
(MP4ONE B464 takes the MP4 container); the first metal line (DECSTALL/DECJOBHANG must fly first).
