// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAOSVOLUME (B427): the core's KATs on the REAL files of this tree — every source parses, and the export of the
// parsed records is the file byte for byte (the volume's export is what status-check.py reads).
use jobs_core::*;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn status_tsv_round_trips_byte_identical() {
    let text = read(STATUS_TSV);
    let recs = parse_status_tsv(&text).expect("parse");
    assert!(recs.len() >= 40, "claims={}", recs.len());
    assert_eq!(export_status_tsv(&recs), text);
    for r in &recs {
        assert!(CLAIM_STATUSES.contains(&r.get(K_STATUS)), "{} {}", r.id, r.get(K_STATUS));
        assert!(r.get(K_FLIGHT).is_empty() || flight_ok(r.get(K_FLIGHT)));
    }
}

#[test]
fn ledgers_round_trip_byte_identical() {
    for (track, rel) in LEDGERS {
        let md = read(rel);
        let recs = parse_ledger(track, &md);
        assert!(!recs.is_empty(), "{rel}: no rows");
        assert_eq!(export_ledger(track, &md, &recs), md, "{rel}");
        let mut ids: Vec<&str> = recs.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), recs.len(), "{rel}: ids unique");
        for r in &recs {
            assert!(status_ok(r.get(K_STATUS)), "{rel} {}: {}", r.id, r.get(K_STATUS));
        }
    }
}

#[test]
fn queues_parse() {
    let mut total = 0;
    for (track, rel) in QUEUES {
        let recs = parse_queue(track, &read(rel));
        total += recs.len();
        for r in &recs {
            assert!(status_ok(r.get(K_STATUS)));
            assert!(!r.id.contains('/'));
        }
    }
    assert!(total > 20, "queue items={total}");
}

#[test]
fn ledger_cell_edit_exports_only_that_cell() {
    let md = "x\n| id | item | owner | status |\n|---|---|---|---|\n| B1 | a | rmbp | open — x |\n| B2 | b | rmbp | flown — y |\ntail";
    let mut recs = parse_ledger("rmbp", md);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[1].get(K_STATUS), "flown");
    let head = split_row("| id | item | owner | status |");
    recs[0].body = set_ledger_cell(&head, &recs[0].body, "landed — ST1").unwrap();
    let out = export_ledger("rmbp", md, &recs);
    assert_eq!(out, "x\n| id | item | owner | status |\n|---|---|---|---|\n| B1 | a | rmbp | landed — ST1 |\n| B2 | b | rmbp | flown — y |\ntail");
}

#[test]
fn cite_follows_the_gate() {
    let mut r = Record::new(Kind::Claim, "ST9", "", 9, "c".into());
    assert!(cite(&mut r, "confirmed", "", "", true, "s").is_err());
    assert!(cite(&mut r, "confirmed", "f7", ":: x ::", false, "s").is_err());
    assert!(cite(&mut r, "unflown", "f7", "", true, "s").is_err());
    assert!(cite(&mut r, "settled", "", "", true, "s").is_err());
    assert!(cite(&mut r, "confirmed", "24", ":: x ::", true, "s").is_err());
    cite(&mut r, "confirmed", "f7", ":: x ::", true, "s").unwrap();
    assert_eq!((r.get(K_STATUS), r.get(K_FLIGHT), r.get(K_LINE)), ("confirmed", "f7", ":: x ::"));
    assert!(verify_line(":: x ::", &[b"a\n:: x ::\n".as_slice()]));
    assert!(!verify_line(":: y ::", &[b"a\n:: x ::\n".as_slice()]));
}

#[test]
fn query_translates() {
    // SMALLFIX4 item 13 (ATTRKEYS): the expected texts are built from the registered keys.
    assert_eq!(query_text("status=confirmed flight=f24").unwrap(), format!("{} == \"confirmed\" AND {} == \"f24\"", K_STATUS, K_FLIGHT));
    assert_eq!(query_text("set-by=x").unwrap(), format!("{} == \"x\"", K_SET_BY));
    let open = format!("{} == open", K_STATUS);
    assert_eq!(query_text(&open).unwrap(), open);
    assert!(query_text("status").is_err());
    assert_eq!(cited("cites ST1 and ST12, not XST3 or ST4a"), "ST1,ST12");
    assert_eq!(ledger_head("**open** — x"), "open");
    assert_eq!(ledger_head("fixed-unflown — x"), "fixed-unflown");
    assert_eq!(ledger_head("opened"), "-");
}
