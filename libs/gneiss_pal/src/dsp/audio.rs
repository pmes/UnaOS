//! `dsp::audio` — the audio decoders (AUDIOCODEC, SR30): re-exports `audio_core`, the `no_std` core the
//! kernel's `play` links as well. One API for every format: `sniff`, `Decoder::open`/`open_bytes`, the
//! `AudioDecoder` trait (`info`, `next` → interleaved f32, `next_i32` → left-justified i32), `decode_all`.
pub use audio_core::*;

#[cfg(test)]
mod tests {
    #[test]
    fn reexport_decodes() {
        // a 4-frame 16-bit stereo WAV through the Gneiss face
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF\x34\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x02\x00\x44\xac\x00\x00\x10\xb1\x02\x00\x04\x00\x10\x00data\x10\x00\x00\x00");
        for s in [0i16, 16384, -16384, 32767, 1, -1, -32768, 0] { w.extend_from_slice(&s.to_le_bytes()); }
        let (info, pcm) = super::decode_all(&w).unwrap();
        assert_eq!((info.rate, info.channels, info.bits), (44100, 2, 16));
        assert_eq!(pcm, vec![0.0, 0.5, -0.5, 32767.0 / 32768.0, 1.0 / 32768.0, -1.0 / 32768.0, -1.0, 0.0]);
    }
}
