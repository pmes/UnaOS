//! Opus known-answer tests: twelve bitstreams made with the reference encoder's own conformance modes
//! (`opus_demo -silk8k_test … -celt_hq_test`, sweeps, random frame sizes, restricted-lowdelay, FEC+DTX;
//! see `oracle/gen-opus.sh`), decoded at 48 kHz stereo. Every packet's final range must equal the one the
//! encoder recorded in the `.bit` file (RFC 6716 §6 / RFC 8251 testvector format), and the whole PCM output
//! must be bit-exact with the libopus 1.5.2 fixed-point decoder (`opus_demo -d 48000 2`), checked by MD5.
//! `AUDIO_CORE_OPUS_REF=<dir with tNN.dec>` reports the first differing sample instead of just the MD5.
use audio_core::md5::Md5;
use audio_core::opus::OpusDecoder;
use std::path::PathBuf;

fn data() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/opus") }

fn hex(d: &[u8]) -> String { d.iter().map(|b| format!("{:02x}", b)).collect() }

#[test]
fn opus_vectors_bit_exact() {
    let expected = std::fs::read_to_string(data().join("expected.txt")).unwrap();
    let mut fails = vec![];
    let mut total_packets = 0;
    for line in expected.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (name, md5_ref, size_ref) = (f[0], f[1], f[2].parse::<usize>().unwrap());
        let bits = std::fs::read(data().join(format!("{}.bit", name))).unwrap();
        let mut dec = OpusDecoder::new(2);
        let mut out: Vec<i16> = vec![];
        let mut pcm = vec![0i16; 48000 * 2 * 2];
        let mut p = 0usize;
        let mut pkt = 0usize;
        let mut range_bad = 0usize;
        let mut first_range_bad = None;
        while p + 8 <= bits.len() {
            let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
            let rng = u32::from_be_bytes(bits[p + 4..p + 8].try_into().unwrap());
            p += 8;
            let pk = &bits[p..p + len];
            p += len;
            let n = if len == 0 {
                let d = dec.last_packet_duration as usize;
                dec.decode(None, &mut pcm, d).unwrap()
            } else {
                match dec.decode(Some(pk), &mut pcm, 48000 * 2) {
                    Ok(n) => n,
                    Err(e) => { fails.push(format!("{} packet {}: {:?}", name, pkt, e)); break; }
                }
            };
            if len != 0 && dec.range_final != rng {
                range_bad += 1;
                if first_range_bad.is_none() { first_range_bad = Some((pkt, len, pk[0])); }
            }
            out.extend_from_slice(&pcm[..n * 2]);
            pkt += 1;
        }
        total_packets += pkt;
        let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut m = Md5::new();
        m.update(&bytes);
        let md5 = hex(&m.finish());
        let mut detail = String::new();
        if let Ok(dir) = std::env::var("AUDIO_CORE_OPUS_REF") {
            if let Ok(r) = std::fs::read(PathBuf::from(dir).join(format!("{}.dec", name))) {
                let rs: Vec<i16> = r.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                if let Some(i) = (0..rs.len().min(out.len())).find(|&i| rs[i] != out[i]) {
                    detail = format!(" first diff at sample {} (frame {}, ch {}): ours {} ref {}", i, i / 2, i % 2, out[i], rs[i]);
                }
            }
        }
        let ok = md5 == md5_ref && bytes.len() == size_ref && range_bad == 0;
        println!("{} packets={} range_mismatch={} first={:?} bytes={} (ref {}) md5={} {}{}", name, pkt, range_bad, first_range_bad, bytes.len(), size_ref, md5,
            if ok { "EXACT" } else { "DIFF" }, detail);
        if !ok { fails.push(name.to_string()); }
    }
    println!("{} packets decoded", total_packets);
    assert!(fails.is_empty(), "failing vectors: {:?}", fails);
}

/// Packet loss: the same streams decoded the way `opus_demo -d 48000 2 -lossfile loss_a.txt` does it —
/// a lost packet is concealed (PLC through SILK and CELT, both pitch-based and noise-based), and when the
/// packet after a loss carries SILK LBRR data the last lost frame is rebuilt from it (in-band FEC).
#[test]
fn opus_packet_loss_bit_exact() {
    use audio_core::opus::decoder::packet_has_lbrr;
    let expected = std::fs::read_to_string(data().join("expected_loss.txt")).unwrap();
    let mut fails = vec![];
    for line in expected.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (name, lossname, md5_ref, size_ref) = (f[0], f[1], f[2], f[3].parse::<usize>().unwrap());
        let bits = std::fs::read(data().join(format!("{}.bit", name))).unwrap();
        let losses: Vec<bool> = std::fs::read_to_string(data().join(format!("{}.txt", lossname))).unwrap()
            .split_whitespace().map(|t| t != "0").collect();
        let mut dec = OpusDecoder::new(2);
        let mut out: Vec<i16> = vec![];
        let mut pcm = vec![0i16; 48000 * 2 * 2];
        let mut p = 0usize;
        let mut pkt = 0usize;
        let mut lost_count = 0usize;
        let (mut n_plc, mut n_fec) = (0, 0);
        while p + 8 <= bits.len() {
            let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
            p += 8;
            let pk = &bits[p..p + len];
            p += len;
            let lost = len == 0 || *losses.get(pkt).unwrap_or(&false);
            pkt += 1;
            if lost { lost_count += 1; continue; }
            for fr in 0..=lost_count {
                let n = if fr + 1 == lost_count && packet_has_lbrr(pk) {
                    n_fec += 1;
                    let d = dec.last_packet_duration as usize;
                    dec.decode_ext(Some(pk), &mut pcm, d, true).unwrap()
                } else if fr < lost_count {
                    n_plc += 1;
                    let d = dec.last_packet_duration as usize;
                    dec.decode(None, &mut pcm, d).unwrap()
                } else {
                    dec.decode(Some(pk), &mut pcm, 48000 * 2).unwrap()
                };
                out.extend_from_slice(&pcm[..n * 2]);
            }
            lost_count = 0;
        }
        let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut m = Md5::new();
        m.update(&bytes);
        let md5 = hex(&m.finish());
        let mut detail = String::new();
        if let Ok(dir) = std::env::var("AUDIO_CORE_OPUS_REF") {
            if let Ok(r) = std::fs::read(PathBuf::from(dir).join(format!("{}_loss.dec", name))) {
                let rs: Vec<i16> = r.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                if let Some(i) = (0..rs.len().min(out.len())).find(|&i| rs[i] != out[i]) {
                    detail = format!(" first diff at sample {} (frame {}, ch {}): ours {} ref {}", i, i / 2, i % 2, out[i], rs[i]);
                }
            }
        }
        let ok = md5 == md5_ref && bytes.len() == size_ref;
        println!("{} +{} concealed={} fec={} bytes={} (ref {}) {}{}", name, lossname, n_plc, n_fec, bytes.len(), size_ref, if ok { "EXACT" } else { "DIFF" }, detail);
        if !ok { fails.push(name.to_string()); }
    }
    assert!(fails.is_empty(), "failing loss vectors: {:?}", fails);
}
