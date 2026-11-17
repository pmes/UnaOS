// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! STRIAKSEQ (LEDGER SR9065): the image-sequence reel's shared rules (`timeline_core::seq`), the ONE copy Stria's loader and
//! the kernel Player's EDL mode both call. The Player side (vplay.rs `seq_open` / `seq_frame`) is not reached by any host test.

use timeline_core::seq::{decode, dir_shape, format, number};

#[test]
fn striakseq_format_table_names_six_formats() {
    let name = |s: &str| format(s).map(|q| q.1);
    assert_eq!(name(".png"), Some("PNG"));
    assert_eq!(name(".PNG"), Some("PNG"), "case-insensitive: FAT spells it .PNG");
    assert_eq!(name(".jpg"), Some("JPEG"));
    assert_eq!(name(".JPEG"), Some("JPEG"));
    assert_eq!(name(".gif"), Some("GIF"));
    assert_eq!(name(".bmp"), Some("BMP"));
    assert_eq!(name(".qoi"), Some("QOI"));
    assert_eq!(name(".webp"), Some("WebP"));
    for s in [".tif", ".tiff", ".svg", ".mp4", ""] {
        assert_eq!(format(s), None, "{s:?} is not a sequence format");
    }
    assert_eq!(format(".png").map(|q| q.0), format(".PNG").map(|q| q.0), "one format whatever the case");
}

#[test]
fn striakseq_number_keeps_the_width_round_trip() {
    assert_eq!(number("f_", 4, ".png", "f_0001.png"), Some(1));
    assert_eq!(number("f_", 4, ".png", "f_12345.png"), Some(12345), "a number wider than the pad is itself");
    assert_eq!(number("f_", 4, ".png", "f_001.png"), None, "three digits at width 4 is no member");
    assert_eq!(number("f_", 4, ".png", "f_1.png"), None);
    assert_eq!(number("f_", 0, ".png", "f_7.png"), Some(7));
    assert_eq!(number("f_", 0, ".png", "f_07.png"), None, "a padded name under %d is no member");
    assert_eq!(number("f_", 4, ".png", "f_00a1.png"), None);
    assert_eq!(number("f_", 4, ".png", "f_.png"), None);
    assert_eq!(number("f_", 4, ".png", "g_0001.png"), None);
    assert_eq!(number("f_", 4, ".png", "f_0001.jpg"), None);
    assert_eq!(number("", 4, ".PNG", "0042.PNG"), Some(42), "the Player's stemless reel directory");
}

#[test]
fn striakseq_dir_shape_reads_the_reel_directory() {
    assert_eq!(dir_shape(["0002.PNG", "0001.PNG", "NOTES.TXT"].into_iter()), Some((4, String::from(".PNG"))));
    assert_eq!(dir_shape(["2.png", "1.png", "10.png"].into_iter()), Some((0, String::from(".png"))), "unpadded: %d");
    assert_eq!(dir_shape(["cover.png", "0001.tif", "1.2.png", "0001.jpg"].into_iter()), Some((4, String::from(".jpg"))), "only <digits><format suffix> counts");
    assert_eq!(dir_shape(["readme", ".png", "x0001.png"].into_iter()), None);
    assert_eq!(dir_shape(core::iter::empty::<&str>()), None);
    let names = ["0001.PNG", "0002.PNG", "0003.PNG", "0010.PNG", "THUMBS.DB"];
    let (w, sfx) = dir_shape(names.into_iter()).unwrap();
    let ns: Vec<u64> = names.iter().filter_map(|n| number("", w, &sfx, n)).collect();
    assert_eq!(ns, [1, 2, 3, 10], "the members are what `number` takes with no stem");
}

#[test]
fn striakseq_decode_turns_a_jpeg_upright() {
    let img = decode(include_bytes!("../../../../../tools/eyes/suites/facet/fixtures/photo-o6.jpg")).unwrap();
    assert_eq!((img.width, img.height), (32, 48), "a 48x32 stored card with EXIF 6 is 32x48 upright");
    assert_eq!(img.orientation, 1, "the orientation is applied, not reported");
    assert_eq!(img.rgba.len(), 32 * 48 * 4);
    assert!(decode(b"not an image").is_err());
}
