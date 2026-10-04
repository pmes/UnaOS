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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
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
    let img = planes_to_rgba_with(&planes, up, conv);
    std::fs::write(&args[2], png_rgba(img.w, img.h, &img.rgba)).expect("write png");
    ExitCode::SUCCESS
}
