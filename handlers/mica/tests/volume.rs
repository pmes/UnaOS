// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAOSVOLUME (B427): records → volume → export, byte-identical, on the REAL repo text; the verbs on a volume.
use jobs_core as jc;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn repo_round_trips_through_the_volume() {
    let w = mica::witness(&repo()).expect("witness");
    println!("{}", w.line());
    assert!(w.identical, "export differs");
    assert!(w.census.claims >= 40 && w.census.ledger > 300 && w.census.queue > 20, "{:?}", w.census);
    assert_eq!(w.verified, w.quoted, "verify failed: {:?}", w.failed);
}

#[test]
fn verbs_on_a_volume() {
    let src = mica::Sources::read(&repo()).unwrap();
    let recs = src.records().unwrap();
    let mut fs = mica::mem_volume(mica::size_mb_for(&recs)).unwrap();
    mica::populate(&mut fs, &recs).unwrap();
    // query: the index answers the typed attributes
    let confirmed = recs.iter().filter(|r| r.kind == jc::Kind::Claim && r.get(jc::K_STATUS) == "confirmed").count();
    let hits = mica::query(&mut fs, "kind=claim status=confirmed").unwrap();
    assert_eq!(hits.len(), confirmed);
    assert!(hits.iter().all(|p| p.starts_with("/jobs/status/ST")));
    // the saved query QUERYFOLDER opens
    let q = fs.resolve_path("/jobs/queries/Open jobs").unwrap();
    assert_eq!(fs.get_attribute(q, jc::TYPE_KEY).unwrap(), Some(unafs::AttributeValue::String(jc::QUERY_TYPE.into())));
    let open = mica::query(&mut fs, "job:status == open").unwrap();
    assert!(!open.is_empty());
    // add + cite: the gate's rules hold on the volume, and the export carries the new row
    let r = mica::add_claim(&mut fs, "a new claim", "UNAOSVOLUME test", "B1").unwrap();
    assert_eq!(r.id, jc::next_claim_id(&recs));
    assert!(mica::cite_claim(&repo(), &mut fs, &r.id, "confirmed", "f24", ":: a line nobody printed ::", "t").is_err());
    let st3 = recs.iter().find(|r| r.id == "ST3").unwrap();
    let (fl, line) = (st3.get(jc::K_FLIGHT).to_string(), st3.get(jc::K_LINE).to_string());
    mica::cite_claim(&repo(), &mut fs, &r.id, "confirmed", &fl, &line, "t").unwrap();
    let back = mica::load(&mut fs).unwrap();
    let tsv = jc::export_status_tsv(&back);
    assert!(tsv.starts_with(&src.status));
    assert!(tsv.ends_with(&format!("{}\ta new claim\tconfirmed\t{}\t{}\tt\tB1\n", r.id, fl, line)));
}
