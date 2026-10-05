//! AETHERFONT (SR61): the `name` table against fontTools. tests/data/name_kat.tsv (oracle/gen_name_kat.py) holds
//! every record fontTools decodes for name IDs 1, 2, 4, 6, 16, 17 of the container's outline fonts, each face of a
//! collection; font_core must decode exactly the same records. A font whose file is absent or differs is skipped
//! by name; the test fails when no font could be checked.

mod common;

use font_core::name::{self, NameRecord};
use font_core::Font;
use std::collections::BTreeMap;

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn name_records_match_fonttools() {
    let tsv = include_str!("data/name_kat.tsv");
    let mut by_face: BTreeMap<(String, u32, String), Vec<NameRecord>> = BTreeMap::new();
    for line in tsv.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let f: Vec<&str> = line.splitn(8, '\t').collect();
        let rec = NameRecord {
            platform: f[3].parse().unwrap(),
            encoding: f[4].parse().unwrap(),
            language: f[5].parse().unwrap(),
            name_id: f[6].parse().unwrap(),
            value: unescape(f[7]),
        };
        by_face.entry((f[0].to_string(), f[1].parse().unwrap(), f[2].to_string())).or_default().push(rec);
    }
    let (mut faces, mut recs) = (0, 0);
    let mut data_cache: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    for ((path, face, sha), want) in &by_face {
        let data = data_cache.entry(path.clone()).or_insert_with(|| common::load(path, Some(sha)));
        let Some(data) = data else { continue };
        let font = Font::parse_face(data, *face).unwrap_or_else(|e| panic!("{path}#{face}: {e:?}"));
        let got: Vec<NameRecord> =
            name::records(&font).into_iter().filter(|r| matches!(r.name_id, 1 | 2 | 4 | 6 | 16 | 17)).collect();
        assert_eq!(&got, want, "{path}#{face}");
        faces += 1;
        recs += want.len();
    }
    println!("name: {faces} faces, {recs} records identical to fontTools");
    assert!(faces > 0, "no font of the KAT set is present");
}

#[test]
fn family_names_order() {
    let Some(d) = common::load("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", None) else { return };
    let f = Font::parse(&d).unwrap();
    assert_eq!(name::family(&f).as_deref(), Some("DejaVu Sans"));
    assert_eq!(name::values(&f, name::SUBFAMILY).first().map(String::as_str), Some("Bold"));
    assert_eq!(name::values(&f, name::POSTSCRIPT).first().map(String::as_str), Some("DejaVuSans-Bold"));
}
