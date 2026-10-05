//! av1-check — decode an AVIF (or a raw low-overhead AV1 OBU stream, `.obu`) with `av1_core` and
//! write an 8-bit RGBA PNG, plus the frame header facts and which coding tools were exercised.
//!
//! usage: av1-check <in.avif|in.obu> <out.png> [--no-deblock] [--no-cdef] [--no-lr] [--nearest]
//!        [--libyuv] (oracle-only: Chromium/libyuv coefficient quantisation)
//!        [--yuv <out.yuv>]   (raw planes, 16-bit LE per sample, Y then U then V)

use av1_core::image::{decode_avif_planes, decode_obus, planes_to_rgba_with, Conversion, Filters, Upsampling};
use std::process::ExitCode;

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// PNG with a zlib stream of stored (uncompressed) deflate blocks — RFC 1950/1951/PNG spec.
fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    let mut z = vec![0x78u8, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |typ: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut td = typ.to_vec();
        td.extend_from_slice(data);
        out.extend_from_slice(&td);
        out.extend_from_slice(&crc32(&td).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

/// MD5 (RFC 1321), for comparing decoded frames with the libaom test-vector .md5 files.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
        11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = (0..16).map(|i| u32::from_le_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (mut f, g);
            if i < 16 {
                f = (b & c) | (!b & d);
                g = i;
            } else if i < 32 {
                f = (d & b) | (!d & c);
                g = (5 * i + 1) % 16;
            } else if i < 48 {
                f = b ^ c ^ d;
                g = (3 * i + 5) % 16;
            } else {
                f = c ^ (b | !d);
                g = (7 * i) % 16;
            }
            f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The frame bytes libaom's test harness hashes: Y, U, V rows (8-bit samples as bytes, deeper
/// samples as 16-bit little endian), monochrome = Y only.
fn frame_bytes(p: &av1_core::image::Planes) -> Vec<u8> {
    let mut raw = Vec::new();
    let push = |raw: &mut Vec<u8>, v: &[u16]| {
        for &s in v {
            if p.bit_depth == 8 {
                raw.push(s as u8);
            } else {
                raw.extend_from_slice(&s.to_le_bytes());
            }
        }
    };
    push(&mut raw, &p.y);
    if !p.mono {
        push(&mut raw, &p.u);
        push(&mut raw, &p.v);
    } else {
        // libaom's decoder output for 4:0:0 carries mid-grey 4:2:0 chroma planes
        let gray = vec![1u16 << (p.bit_depth - 1); (p.chroma_width() * p.chroma_height()) as usize];
        push(&mut raw, &gray);
        push(&mut raw, &gray);
    }
    raw
}

/// Split an IVF file into frame payloads (32-byte file header, 12-byte frame headers).
pub fn ivf_frames(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    if data.len() < 32 || &data[0..4] != b"DKIF" {
        return out;
    }
    let hdr_len = u16::from_le_bytes([data[6], data[7]]) as usize;
    let mut off = hdr_len;
    while off + 12 <= data.len() {
        let sz = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
        off += 12;
        if off + sz > data.len() {
            break;
        }
        out.push(&data[off..off + sz]);
        off += sz;
    }
    out
}

/// ISO BMFF boxes of `data`: (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= data.len() {
        let mut size = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        let typ: [u8; 4] = data[off + 4..off + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 && off + 16 <= data.len() {
            size = u64::from_be_bytes(data[off + 8..off + 16].try_into().unwrap()) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - off;
        }
        if size < hdr || off + size > data.len() {
            break;
        }
        out.push((typ, &data[off + hdr..off + size]));
        off += size;
    }
    out
}

fn child<'a>(data: &'a [u8], typ: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(t, _)| t == typ).map(|(_, b)| b)
}

/// The AV1 track of an MP4: (av1C record, sample payloads, timescale, sample durations).
pub fn mp4_av1_samples(data: &[u8]) -> Option<(Vec<u8>, Vec<&[u8]>, u32, Vec<u32>)> {
    let moov = child(data, b"moov")?;
    for (t, trak) in boxes(moov) {
        if &t != b"trak" {
            continue;
        }
        let mdia = child(trak, b"mdia")?;
        let mdhd = child(mdia, b"mdhd")?;
        let timescale = if mdhd[0] == 1 { u32::from_be_bytes(mdhd[20..24].try_into().unwrap()) } else { u32::from_be_bytes(mdhd[12..16].try_into().unwrap()) };
        let stbl = child(child(mdia, b"minf")?, b"stbl")?;
        let stsd = child(stbl, b"stsd")?;
        // stsd: version/flags (4), entry_count (4), then sample entries
        let entries = boxes(&stsd[8..]);
        let Some((_, av01)) = entries.iter().find(|(t, _)| t == b"av01") else { continue };
        // VisualSampleEntry: 78 bytes before the child boxes
        let av1c = child(&av01[78..], b"av1C")?.to_vec();
        let stsz = child(stbl, b"stsz")?;
        let fixed = u32::from_be_bytes(stsz[4..8].try_into().unwrap());
        let count = u32::from_be_bytes(stsz[8..12].try_into().unwrap()) as usize;
        let sizes: Vec<usize> = (0..count).map(|i| if fixed != 0 { fixed as usize } else { u32::from_be_bytes(stsz[12 + 4 * i..16 + 4 * i].try_into().unwrap()) as usize }).collect();
        let offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
            let n = u32::from_be_bytes(stco[4..8].try_into().unwrap()) as usize;
            (0..n).map(|i| u32::from_be_bytes(stco[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as u64).collect()
        } else {
            let co64 = child(stbl, b"co64")?;
            let n = u32::from_be_bytes(co64[4..8].try_into().unwrap()) as usize;
            (0..n).map(|i| u64::from_be_bytes(co64[8 + 8 * i..16 + 8 * i].try_into().unwrap())).collect()
        };
        let stsc = child(stbl, b"stsc")?;
        let n = u32::from_be_bytes(stsc[4..8].try_into().unwrap()) as usize;
        let runs: Vec<(usize, usize)> = (0..n)
            .map(|i| {
                let e = &stsc[8 + 12 * i..20 + 12 * i];
                (u32::from_be_bytes(e[0..4].try_into().unwrap()) as usize, u32::from_be_bytes(e[4..8].try_into().unwrap()) as usize)
            })
            .collect();
        let mut samples = Vec::new();
        let mut s = 0usize;
        for (ci, &off) in offsets.iter().enumerate() {
            let chunk = ci + 1;
            let per = runs.iter().rev().find(|(first, _)| *first <= chunk).map(|r| r.1).unwrap_or(0);
            let mut o = off as usize;
            for _ in 0..per {
                if s >= sizes.len() {
                    break;
                }
                samples.push(&data[o..o + sizes[s]]);
                o += sizes[s];
                s += 1;
            }
        }
        let stts = child(stbl, b"stts")?;
        let n = u32::from_be_bytes(stts[4..8].try_into().unwrap()) as usize;
        let mut durs = Vec::new();
        for i in 0..n {
            let e = &stts[8 + 8 * i..16 + 8 * i];
            let c = u32::from_be_bytes(e[0..4].try_into().unwrap());
            let d = u32::from_be_bytes(e[4..8].try_into().unwrap());
            for _ in 0..c {
                durs.push(d);
            }
        }
        // fragmented MP4: moof / traf / tfhd + trun
        let mut moof_off;
        let mut off = 0usize;
        while off + 8 <= data.len() {
            let size = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            let typ = &data[off + 4..off + 8];
            if size < 8 {
                break;
            }
            if typ == b"moof" {
                moof_off = off;
                let moof = &data[off + 8..off + size];
                for (t, traf) in boxes(moof) {
                    if &t != b"traf" {
                        continue;
                    }
                    let tfhd = child(traf, b"tfhd")?;
                    let flags = u32::from_be_bytes([0, tfhd[1], tfhd[2], tfhd[3]]);
                    let mut q = 8;
                    let mut base = moof_off as u64;
                    if flags & 1 != 0 {
                        base = u64::from_be_bytes(tfhd[q..q + 8].try_into().unwrap());
                        q += 8;
                    }
                    if flags & 2 != 0 {
                        q += 4;
                    }
                    let mut def_dur = 0u32;
                    let mut def_size = 0u32;
                    if flags & 8 != 0 {
                        def_dur = u32::from_be_bytes(tfhd[q..q + 4].try_into().unwrap());
                        q += 4;
                    }
                    if flags & 0x10 != 0 {
                        def_size = u32::from_be_bytes(tfhd[q..q + 4].try_into().unwrap());
                    }
                    for (t2, trun) in boxes(traf) {
                        if &t2 != b"trun" {
                            continue;
                        }
                        let tf = u32::from_be_bytes([0, trun[1], trun[2], trun[3]]);
                        let cnt = u32::from_be_bytes(trun[4..8].try_into().unwrap()) as usize;
                        let mut q = 8;
                        let mut data_off = base as i64;
                        if tf & 1 != 0 {
                            data_off += i32::from_be_bytes(trun[q..q + 4].try_into().unwrap()) as i64;
                            q += 4;
                        }
                        if tf & 4 != 0 {
                            q += 4;
                        }
                        let mut o = data_off as usize;
                        for _ in 0..cnt {
                            let mut d = def_dur;
                            let mut sz = def_size;
                            if tf & 0x100 != 0 {
                                d = u32::from_be_bytes(trun[q..q + 4].try_into().unwrap());
                                q += 4;
                            }
                            if tf & 0x200 != 0 {
                                sz = u32::from_be_bytes(trun[q..q + 4].try_into().unwrap());
                                q += 4;
                            }
                            if tf & 0x400 != 0 {
                                q += 4;
                            }
                            if tf & 0x800 != 0 {
                                q += 4;
                            }
                            samples.push(&data[o..o + sz as usize]);
                            durs.push(d);
                            o += sz as usize;
                        }
                    }
                }
            }
            off += size;
        }
        return Some((av1c, samples, timescale, durs));
    }
    None
}

/// Matroska / WebM: read an EBML variable-length integer (ID keeps its marker bit).
fn ebml_vint(d: &[u8], off: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let b = *d.get(off)?;
    let len = b.leading_zeros() as usize + 1;
    if len > 8 || off + len > d.len() {
        return None;
    }
    let mut v = if keep_marker { b as u64 } else { (b as u64) & ((1u64 << (8 - len)) - 1) };
    for i in 1..len {
        v = (v << 8) | d[off + i] as u64;
    }
    Some((v, len))
}

/// The frames of the first video track of a Matroska/WebM file: (CodecPrivate, frames).
/// Walks Segment > Cluster > SimpleBlock / BlockGroup > Block (no lacing).
pub fn mkv_frames(data: &[u8]) -> Option<(Vec<u8>, Vec<&[u8]>)> {
    const SEGMENT: u64 = 0x18538067;
    const CLUSTER: u64 = 0x1F43B675;
    const SIMPLEBLOCK: u64 = 0xA3;
    const BLOCKGROUP: u64 = 0xA0;
    const BLOCK: u64 = 0xA1;
    const TRACKS: u64 = 0x1654AE6B;
    const TRACKENTRY: u64 = 0xAE;
    const TRACKNUMBER: u64 = 0xD7;
    const TRACKTYPE: u64 = 0x83;
    const CODECPRIVATE: u64 = 0x63A2;
    let mut frames = Vec::new();
    let mut codec_private = Vec::new();
    let mut video_track = 0u64;
    fn walk<'a>(d: &'a [u8], mut off: usize, end: usize, f: &mut dyn FnMut(u64, &'a [u8]) -> bool) {
        while off < end {
            let Some((id, il)) = ebml_vint(d, off, true) else { return };
            let Some((size, sl)) = ebml_vint(d, off + il, false) else { return };
            let start = off + il + sl;
            let unknown = size == (1u64 << (7 * sl)) - 1;
            let stop = if unknown { end } else { (start + size as usize).min(end) };
            if f(id, &d[start..stop]) {
                walk(d, start, stop, f);
            }
            off = stop;
        }
    }
    let mut blocks: Vec<&[u8]> = Vec::new();
    walk(data, 0, data.len(), &mut |id, body| match id {
        SEGMENT | CLUSTER | BLOCKGROUP | TRACKS => true,
        TRACKENTRY => {
            let mut num = 0u64;
            let mut typ = 0u64;
            let mut cp: &[u8] = &[];
            walk(body, 0, body.len(), &mut |id2, b2| {
                match id2 {
                    TRACKNUMBER => num = b2.iter().fold(0, |a, &x| (a << 8) | x as u64),
                    TRACKTYPE => typ = b2.iter().fold(0, |a, &x| (a << 8) | x as u64),
                    CODECPRIVATE => cp = b2,
                    _ => {}
                }
                false
            });
            if typ == 1 && video_track == 0 {
                video_track = num;
                codec_private = cp.to_vec();
            }
            false
        }
        SIMPLEBLOCK | BLOCK => {
            blocks.push(body);
            false
        }
        _ => false,
    });
    for b in blocks {
        let Some((track, tl)) = ebml_vint(b, 0, false) else { continue };
        if track != video_track || b.len() < tl + 3 {
            continue;
        }
        let flags = b[tl + 2];
        if flags & 0x06 != 0 {
            continue; // laced blocks are not used for video
        }
        frames.push(&b[tl + 3..]);
    }
    Some((codec_private, frames))
}

/// `.mp4` mode: decode the AV1 track; print one line per frame with its presentation time;
/// `--png <prefix>` writes every frame (or `--frames a,b,c` only those).
fn mp4_mode(args: &[String]) -> ExitCode {
    let input = std::fs::read(&args[1]).expect("read input");
    let parsed = if args[1].ends_with(".mkv") || args[1].ends_with(".webm") {
        mkv_frames(&input).map(|(cp, f)| {
            let n = f.len();
            (cp, f, 1000u32, vec![0u32; n])
        })
    } else {
        mp4_av1_samples(&input)
    };
    let Some((av1c, samples, timescale, durs)) = parsed else {
        eprintln!("no AV1 track");
        return ExitCode::from(1);
    };
    let mut md5ref: Vec<String> = Vec::new();
    let mut matched = 0usize;
    let mut png_prefix: Option<String> = None;
    let mut only: Option<Vec<usize>> = None;
    let mut verbose = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--png" => {
                i += 1;
                png_prefix = Some(args[i].clone());
            }
            "--frames" => {
                i += 1;
                only = Some(args[i].split(',').map(|x| x.parse().unwrap()).collect());
            }
            "-v" => verbose = true,
            "--md5" => {
                i += 1;
                let t = std::fs::read_to_string(&args[i]).expect("read md5 file");
                md5ref = t.lines().filter_map(|l| l.split_whitespace().next().map(|s| s.to_string())).collect();
            }
            a => {
                eprintln!("unknown option {a}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let mut dec = av1_core::image::StreamDecoder::new(if av1c.len() >= 4 { &av1c } else { &[] }).expect("av1C");
    let mut pts = 0u64;
    for (n, s) in samples.iter().enumerate() {
        let p = match dec.decode_temporal_unit(s) {
            Ok(p) => p,
            Err(e) => {
                println!("sample {n}: decode failed: {e}");
                return ExitCode::from(1);
            }
        };
        let d = hex(&md5(&frame_bytes(&p)));
        let verdict = match md5ref.get(n) {
            Some(w) if *w == d => {
                matched += 1;
                "MATCH"
            }
            Some(_) => "DIFF",
            None => "",
        };
        println!(
            "frame {n:3} pts {:.6}s {}x{} type {} show_existing {} matrix {} range {} {} {}",
            pts as f64 / timescale as f64,
            p.width,
            p.height,
            p.frame.frame_type,
            p.frame.show_existing_frame,
            p.matrix_coefficients,
            if p.full_range { "full" } else { "limited" },
            d,
            verdict
        );
        if verbose {
            println!("   tools: {:?}", p.stats);
        }
        if let Some(pre) = &png_prefix {
            if only.as_ref().map(|o| o.contains(&n)).unwrap_or(true) {
                let img = planes_to_rgba_with(&p, Upsampling::Bilinear, Conversion::Exact);
                std::fs::write(format!("{pre}{n:03}.png"), png_rgba(img.w, img.h, &img.rgba)).expect("write png");
            }
        }
        pts += *durs.get(n).unwrap_or(&0) as u64;
    }
    if !md5ref.is_empty() {
        println!("{}/{} md5 match", matched, md5ref.len());
        if matched != md5ref.len() {
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// `.ivf` mode: decode every temporal unit, print one line per shown frame (md5, size, tools),
/// compare with a libaom `.md5` file, optionally write PNGs.
fn ivf_mode(args: &[String]) -> ExitCode {
    let input = std::fs::read(&args[1]).expect("read input");
    let mut md5ref: Vec<String> = Vec::new();
    let mut png_prefix: Option<String> = None;
    let mut yuv_prefix: Option<String> = None;
    let mut filters = Filters::default();
    let mut verbose = false;
    let mut keep_going = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--md5" => {
                i += 1;
                let t = std::fs::read_to_string(&args[i]).expect("read md5 file");
                md5ref = t.lines().filter_map(|l| l.split_whitespace().next().map(|s| s.to_string())).collect();
            }
            "--png" => {
                i += 1;
                png_prefix = Some(args[i].clone());
            }
            "--yuv" => {
                i += 1;
                yuv_prefix = Some(args[i].clone());
            }
            "--no-deblock" => filters.deblock = false,
            "--no-cdef" => filters.cdef = false,
            "--no-lr" => filters.restoration = false,
            "-v" => verbose = true,
            "--keep-going" => keep_going = true,
            a => {
                eprintln!("unknown option {a}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let mut dec = av1_core::image::StreamDecoder::new(&[]).unwrap();
    dec.filters = filters;
    let mut n = 0usize;
    let mut matched = 0usize;
    let mut failed = false;
    let t0 = std::time::Instant::now();
    for (tu_idx, tu) in ivf_frames(&input).iter().enumerate() {
        let frames = match dec.decode_temporal_unit_all(tu) {
            Ok(f) => f,
            Err(e) => {
                println!("TU {tu_idx}: decode failed: {e}");
                failed = true;
                if keep_going {
                    continue;
                }
                break;
            }
        };
        // libaom outputs one frame per temporal unit: the last shown one (highest spatial layer)
        if let Some(p) = frames.last() {
            let d = hex(&md5(&frame_bytes(p)));
            let want = md5ref.get(n).cloned().unwrap_or_default();
            let ok = !want.is_empty() && want == d;
            matched += ok as usize;
            let f = &p.frame;
            println!(
                "frame {n:3} {}x{} type {} show_existing {} q {} {} {}",
                p.width,
                p.height,
                f.frame_type,
                f.show_existing_frame,
                f.base_q_idx,
                d,
                if want.is_empty() { "(no ref)" } else if ok { "MATCH" } else { "DIFF" }
            );
            if verbose {
                println!("   tools: {:?}", p.stats);
            }
            if let Some(pre) = &png_prefix {
                let img = planes_to_rgba_with(p, Upsampling::Bilinear, Conversion::Exact);
                std::fs::write(format!("{pre}{n:03}.png"), png_rgba(img.w, img.h, &img.rgba)).expect("write png");
            }
            if let Some(pre) = &yuv_prefix {
                std::fs::write(format!("{pre}{n:03}.yuv"), frame_bytes(p)).expect("write yuv");
            }
            n += 1;
        }
    }
    println!(
        "{} frames decoded, {}/{} md5 match{} ({:.1} ms)",
        n,
        matched,
        md5ref.len().min(n.max(md5ref.len())),
        if failed { ", DECODE STOPPED" } else { "" },
        t0.elapsed().as_secs_f64() * 1000.0
    );
    if failed || (!md5ref.is_empty() && matched != md5ref.len()) { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

fn p3_to_srgb(rgba: &mut [u8]) {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let enc = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    const M: [[f64; 3]; 3] = [[1.2249401, -0.2249404, 0.0], [-0.0420569, 1.0420571, 0.0], [-0.0196376, -0.0786361, 1.0982735]];
    for px in rgba.chunks_mut(4) {
        let r = lin(px[0] as f64 / 255.0);
        let g = lin(px[1] as f64 / 255.0);
        let b = lin(px[2] as f64 / 255.0);
        for k in 0..3 {
            let v = M[k][0] * r + M[k][1] * g + M[k][2] * b;
            px[k] = (enc(v) * 255.0 + 0.5) as u8;
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1].ends_with(".ivf") {
        return ivf_mode(&args);
    }
    if args.len() >= 2 && (args[1].ends_with(".mp4") || args[1].ends_with(".mkv") || args[1].ends_with(".webm")) {
        return mp4_mode(&args);
    }
    if args.len() < 3 {
        eprintln!("usage: av1-check <in.avif|in.obu> <out.png> [--no-deblock] [--no-cdef] [--no-lr] [--nearest] [--libyuv] [--yuv out.yuv]");
        return ExitCode::from(2);
    }
    let input = std::fs::read(&args[1]).expect("read input");
    let mut filters = Filters::default();
    let mut up = Upsampling::Bilinear;
    let mut conv = Conversion::Exact;
    let mut yuv_out: Option<String> = None;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--no-deblock" => filters.deblock = false,
            "--no-cdef" => filters.cdef = false,
            "--no-lr" => filters.restoration = false,
            "--nearest" => up = Upsampling::Nearest,
            "--libyuv" => conv = Conversion::Libyuv,
            "--yuv" => {
                i += 1;
                yuv_out = Some(args[i].clone());
            }
            a => {
                eprintln!("unknown option {a}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let t0 = std::time::Instant::now();
    let planes = if args[1].ends_with(".obu") { decode_obus(&input, &[], filters) } else { decode_avif_planes(&input, filters) };
    let planes = match planes {
        Ok(p) => p,
        Err(e) => {
            eprintln!("decode failed: {e}");
            return ExitCode::from(1);
        }
    };
    let dt = t0.elapsed();
    let f = &planes.frame;
    let s = &planes.seq;
    println!(
        "{}x{} {}-bit {} profile {} sb{} | q {} lossless {} | tiles {}x{} | deblock [{},{},{},{}] sharp {} | cdef bits {} damp {} | lr {:?} | tx_mode {} reduced_tx_set {} | screen_content {} filter_intra {} edge_filter {} | qm {} | seg {} dq {} dlf {} | decode {:.1} ms",
        planes.width,
        planes.height,
        planes.bit_depth,
        if planes.mono { "4:0:0".to_string() } else { format!("4:{}:{}", if planes.ss_x == 1 { 2 } else { 4 }, if planes.ss_y == 1 { 0 } else if planes.ss_x == 1 { 2 } else { 4 }) },
        s.seq_profile,
        if s.use_128x128_superblock { 128 } else { 64 },
        f.base_q_idx,
        f.coded_lossless,
        f.tile_info.tile_cols,
        f.tile_info.tile_rows,
        f.loop_filter_level[0],
        f.loop_filter_level[1],
        f.loop_filter_level[2],
        f.loop_filter_level[3],
        f.loop_filter_sharpness,
        f.cdef_bits,
        f.cdef_damping,
        f.frame_restoration_type,
        f.tx_mode,
        f.reduced_tx_set,
        f.allow_screen_content_tools,
        s.enable_filter_intra,
        s.enable_intra_edge_filter,
        f.using_qmatrix,
        f.segmentation_enabled,
        f.delta_q_present,
        f.delta_lf_present,
        dt.as_secs_f64() * 1000.0
    );
    println!(
        "colour: primaries {} transfer {} matrix {} full_range {}",
        planes.color_primaries, planes.transfer_characteristics, planes.matrix_coefficients, planes.full_range
    );
    println!("tools: {:?}", planes.stats);
    if let Some(path) = yuv_out {
        let mut raw = Vec::new();
        for v in planes.y.iter().chain(planes.u.iter()).chain(planes.v.iter()) {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, raw).expect("write yuv");
    }
    let mut img = planes_to_rgba_with(&planes, up, conv);
    if planes.color_primaries == 12 && planes.transfer_characteristics == 13 {
        // ORACLE ONLY: Chromium colour-manages Display P3 content into the sRGB output profile;
        // do the same (sRGB EOTF, P3-D65 -> sRGB linear matrix, sRGB OETF) so the comparison
        // measures the decoder, not the gamut.
        p3_to_srgb(&mut img.rgba);
        println!("oracle: Display P3 -> sRGB gamut conversion applied to the PNG");
    }
    std::fs::write(&args[2], png_rgba(img.w, img.h, &img.rgba)).expect("write png");
    ExitCode::SUCCESS
}
