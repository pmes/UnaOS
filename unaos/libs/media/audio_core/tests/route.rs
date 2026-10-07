//! SOUNDOPENERS (rmbp-ledger B500): `audio_core::route` — a sound goes to its own decoder, a container to demux_core.
use audio_core::{route, Format, Route};

#[test]
fn routes() {
    let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    wav.resize(64, 0);
    assert_eq!(route(&wav), Some(Route::AudioCore(Format::Wav)));
    assert_eq!(route(b"fLaC\0\0\0\x22"), Some(Route::AudioCore(Format::Flac)));
    assert_eq!(route(b"OggS\0\x02\0\0\0\0\0\0"), Some(Route::AudioCore(Format::Ogg)));
    assert_eq!(route(b"ID3\x04\0\0\0\0\0\0"), Some(Route::AudioCore(Format::Mp3)));
    assert_eq!(route(&[0xFF, 0xF1, 0x50, 0x80, 0, 0x1F, 0xFC]), Some(Route::AudioCore(Format::Adts)));
    let mut m4a = b"\0\0\0\x20ftypM4A \0\0\0\0M4A mp42isom".to_vec();
    m4a.resize(64, 0);
    assert_eq!(route(&m4a), Some(Route::Demux));
    assert_eq!(route(&m4a).map(Route::via), Some("demux"));
    assert_eq!(route(&wav).map(Route::via), Some("audiocore"));
    assert_eq!(route(b"hello, world"), None);
}
