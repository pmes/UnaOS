// SPDX-License-Identifier: LGPL-3.0-or-later
//! M1/M2 KATs: the RFC 6386 / libvpx VP8 test vectors must decode BIT-EXACT — every shown
//! frame's cropped I420 MD5 equal to the reference decoder's (`.ivf.md5`). The comprehensive set
//! (001..018) runs one test per vector; the rest of libvpx's VP8 set (intra, inter, segmentation,
//! partitions, sharpness, small size) runs in `libvpx_vp8_set`.

mod common;

fn run(n: u32) -> Option<(usize, usize)> {
    run_named(&format!("vp80-00-comprehensive-{n:03}.ivf"))
}

fn run_named(name: &str) -> Option<(usize, usize)> {
    let ivf = common::load(&format!("vp8/{name}"))?;
    let md5s = common::load(&format!("vp8/{name}.md5"))?;
    let want: Vec<&str> = std::str::from_utf8(&md5s).unwrap().lines().filter_map(|l| l.split_whitespace().next()).collect();
    let mut dec = vp8_core::Decoder::new();
    let mut got = 0usize;
    let mut first_bad = None;
    for (i, f) in common::ivf_frames(&ivf).into_iter().enumerate() {
        match dec.decode(f) {
            Ok(Some(p)) => {
                let h = common::md5_hex(&p.to_i420());
                if want.get(got) != Some(&h.as_str()) && first_bad.is_none() {
                    first_bad = Some((i, got));
                }
                got += 1;
            }
            Ok(None) => {}
            Err(e) => panic!("{name} frame {i}: {e}"),
        }
    }
    if let Some((i, k)) = first_bad {
        panic!("{name}: shown frame {k} (packet {i}) MD5 differs");
    }
    assert_eq!(got, want.len(), "{name}: shown frame count");
    Some((got, want.len()))
}

macro_rules! vectors {
    ($($t:ident = $n:expr),* $(,)?) => {$(
        #[test]
        fn $t() {
            match run($n) {
                Some((g, _)) => eprintln!("vp80-00-comprehensive-{:03}: {g} frames bit-exact", $n),
                None => eprintln!("SKIP vp80-00-comprehensive-{:03}", $n),
            }
        }
    )*};
}

vectors!(
    comprehensive_001 = 1, comprehensive_002 = 2, comprehensive_003 = 3, comprehensive_004 = 4,
    comprehensive_005 = 5, comprehensive_006 = 6, comprehensive_007 = 7, comprehensive_008 = 8,
    comprehensive_009 = 9, comprehensive_010 = 10, comprehensive_011 = 11, comprehensive_012 = 12,
    comprehensive_013 = 13, comprehensive_014 = 14, comprehensive_015 = 15, comprehensive_016 = 16,
    comprehensive_017 = 17, comprehensive_018 = 18,
);

#[test]
fn libvpx_vp8_set() {
    let names: Vec<&str> = include_str!("vectors.txt")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("vp8/"))
        .filter(|n| n.ends_with(".ivf") && !n.starts_with("vp80-00-"))
        .collect();
    let (mut files, mut frames) = (0, 0);
    for n in &names {
        if let Some((g, _)) = run_named(n) {
            files += 1;
            frames += g;
        }
    }
    eprintln!("libvpx VP8 set: {files}/{} vectors, {frames} frames bit-exact", names.len());
}
