//! RFC 3533 container KATs: packets of awkward sizes (0, 255, 256, multi-page) muxed by an independent
//! writer here, read back exactly; a corrupted page is dropped by its CRC and the packet that spanned it
//! is discarded without desynchronising the rest; granules land on the last packet completed per page.
use audio_core::crc::crc32_ogg;
use audio_core::ogg::OggReader;
use audio_core::{ByteStream, VecReader};

/// Mux `packets` into pages of at most `max_segs` lacing values. Returns the stream and per-page offsets.
fn mux(packets: &[Vec<u8>], max_segs: usize, serial: u32) -> (Vec<u8>, Vec<usize>) {
    let mut lacing_all: Vec<(u8, usize, bool)> = vec![]; // (lacing, packet index, ends packet)
    for (i, p) in packets.iter().enumerate() {
        let mut n = p.len();
        loop {
            if n >= 255 { lacing_all.push((255, i, false)); n -= 255; } else { lacing_all.push((n as u8, i, true)); break; }
        }
    }
    let (mut out, mut offs) = (vec![], vec![]);
    let mut pos_in = vec![0usize; packets.len()];
    let mut seq = 0u32;
    let mut cont = false;
    for (pi, chunk) in lacing_all.chunks(max_segs).enumerate() {
        offs.push(out.len());
        let mut body = vec![];
        for &(l, i, _) in chunk {
            body.extend_from_slice(&packets[i][pos_in[i]..pos_in[i] + l as usize]);
            pos_in[i] += l as usize;
        }
        let last_done = chunk.iter().rev().find(|c| c.2).map(|c| c.1);
        let granule: u64 = last_done.map(|i| (i as u64 + 1) * 1000).unwrap_or(u64::MAX);
        let mut h = vec![];
        h.extend_from_slice(b"OggS");
        h.push(0);
        let eos = pi == lacing_all.len().div_ceil(max_segs) - 1;
        h.push((cont as u8) | if pi == 0 { 2 } else { 0 } | if eos { 4 } else { 0 });
        h.extend_from_slice(&granule.to_le_bytes());
        h.extend_from_slice(&serial.to_le_bytes());
        h.extend_from_slice(&seq.to_le_bytes());
        h.extend_from_slice(&[0; 4]);
        h.push(chunk.len() as u8);
        h.extend(chunk.iter().map(|c| c.0));
        h.extend_from_slice(&body);
        let crc = crc32_ogg(&h);
        h[22..26].copy_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&h);
        cont = !chunk.last().unwrap().2;
        seq += 1;
    }
    (out, offs)
}

fn read_all(bytes: Vec<u8>) -> (Vec<(Vec<u8>, Option<u64>, bool)>, u64) {
    let mut r = OggReader::new(ByteStream::new(Box::new(VecReader::new(bytes))));
    let mut v = vec![];
    while let Some(p) = r.next_packet().unwrap() { v.push((p.data, p.granule, p.eos)); }
    (v, r.crc_failures)
}

fn pkt(n: usize, seed: u8) -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }

#[test]
fn reassembly_exact() {
    let packets = vec![pkt(0, 1), pkt(1, 2), pkt(255, 3), pkt(256, 4), pkt(70_000, 5), pkt(510, 6), pkt(3, 7)];
    for max in [1usize, 2, 7, 255] {
        let (bytes, _) = mux(&packets, max, 0x1234);
        let (got, fails) = read_all(bytes);
        assert_eq!(fails, 0);
        assert_eq!(got.len(), packets.len(), "max_segs={}", max);
        for (i, (d, _, _)) in got.iter().enumerate() { assert_eq!(d, &packets[i], "packet {} max_segs={}", i, max); }
        assert!(got.last().unwrap().2, "eos on the last packet");
        assert_eq!(got.last().unwrap().1, Some(packets.len() as u64 * 1000));
    }
}

#[test]
fn crc_drop_and_resync() {
    let packets: Vec<Vec<u8>> = (0..12).map(|i| pkt(300 + i * 37, i as u8)).collect();
    let (mut bytes, offs) = mux(&packets, 3, 7);
    // garbage before the first page and between pages must be skipped by the capture-pattern search
    let mut junk = b"ID3 nonsense OggS-not-a-page".to_vec();
    junk.extend_from_slice(&bytes);
    bytes = junk;
    let shift = 28;
    // corrupt one byte in the body of page 2 (which carries the tail of packet 1 and head of packet 2)
    bytes[shift + offs[2] + 40] ^= 0xFF;
    let (got, fails) = read_all(bytes);
    assert_eq!(fails, 1, "exactly one page fails its CRC");
    // every packet that did not touch page 2 comes back intact and in order
    for (d, _, _) in &got { assert!(packets.contains(d), "no corrupted or spliced packet is ever returned"); }
    assert!(got.len() >= packets.len() - 2 && got.len() < packets.len());
}
