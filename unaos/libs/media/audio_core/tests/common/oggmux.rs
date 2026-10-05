//! A minimal, independent Ogg page writer for the tests (RFC 3533): one packet per page, granule per page.
use audio_core::crc::crc32_ogg;

pub fn page(serial: u32, seq: u32, flags: u8, granule: u64, packet: &[u8]) -> Vec<u8> {
    let mut lacing = vec![];
    let mut n = packet.len();
    loop { if n >= 255 { lacing.push(255u8); n -= 255; } else { lacing.push(n as u8); break; } }
    assert!(lacing.len() <= 255);
    let mut h = vec![];
    h.extend_from_slice(b"OggS");
    h.push(0);
    h.push(flags);
    h.extend_from_slice(&granule.to_le_bytes());
    h.extend_from_slice(&serial.to_le_bytes());
    h.extend_from_slice(&seq.to_le_bytes());
    h.extend_from_slice(&[0; 4]);
    h.push(lacing.len() as u8);
    h.extend_from_slice(&lacing);
    h.extend_from_slice(packet);
    let crc = crc32_ogg(&h);
    h[22..26].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Wrap raw Opus packets (each with its duration in 48 kHz samples) as an Ogg Opus file (RFC 7845).
pub fn ogg_opus(channels: u8, pre_skip: u16, trim_end: u64, packets: &[(Vec<u8>, u64)]) -> Vec<u8> {
    let mut out = vec![];
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&48000u32.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    out.extend(page(7, 0, 2, 0, &head));
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&8u32.to_le_bytes());
    tags.extend_from_slice(b"UnaOS-ac");
    tags.extend_from_slice(&0u32.to_le_bytes());
    out.extend(page(7, 1, 0, 0, &tags));
    let mut g = 0u64;
    let total: u64 = packets.iter().map(|p| p.1).sum();
    for (i, (p, d)) in packets.iter().enumerate() {
        g += d;
        let last = i + 1 == packets.len();
        let gran = if last { total - trim_end } else { g };
        out.extend(page(7, 2 + i as u32, if last { 4 } else { 0 }, gran, p));
    }
    out
}
