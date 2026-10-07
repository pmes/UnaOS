// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JOBSNEXT (B506): `mica jobs next/cut/land/owed` over a FIXTURE volume (a fixture repo, built the way `build`
// builds), and the ranking over the REAL repo's latest flight.
use jobs_core as jc;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mica-next-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let w = |rel: &str, s: &str| {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    w(jc::STATUS_TSV, "id\tclaim\tstatus\tflight\tline\tset-by\trow-refs\nST1\ta claim\topen\t\t\tt\tB1\n");
    for (_, rel) in jc::LEDGERS {
        w(rel, "# L\n\n| id | job | track | status |\n|---|---|---|---|\n| B1 | **CEFLY** the Kepler CE | rmbp | open |\n| B2 | **JOBSCAN** off the render handler | rmbp | open — JOBSCAN on branch exec-rmbp-jobscan |\n");
    }
    for (_, rel) in jc::QUEUES {
        w(rel, "# Q\n## METAL\n✓ DONE  GMUX-1  a Kepler rung\n· NEW  CEFLY  first Kepler CE flight (ledger row **B1**)\n· NEW  OLDTHING  a job\n· NEW  GEN7BLIT  the gen7 blitter on the card\n· NEW  PRTSCR  capture race\n");
    }
    w("docs/dev/evidence/x/flight26/FLIGHT26.md", "## 3. Owed\nOLDTHING.\n");
    w("docs/dev/evidence/x/flight27/FLIGHT27.md", "# F27\n## 3. Owed (the cloud's, in Peter's order)\nFINEMOTION + PRTSCR (the race), JOBSCAN (17 s). Then Cmd-Tab.\n## 4. x\n");
    w("docs/dev/evidence/x/PRTSCR.md", "brief\n");
    d
}

fn rows(lines: &[String]) -> Vec<String> {
    lines[1..].iter().map(|l| l.split('\t').nth(1).unwrap().to_string() + "/" + l.split('\t').nth(2).unwrap()).collect()
}

#[test]
fn next_cut_land_on_a_fixture_volume() {
    let repo = fixture();
    let src = mica::Sources::read(&repo).unwrap();
    let recs = src.records().unwrap();
    let mut fs = mica::mem_volume(32).unwrap();
    mica::populate(&mut fs, &recs).unwrap();
    // build's half: the latest §3 onto the volume
    let (fl, owed) = mica::owed_from_repo(&repo, None).unwrap();
    assert_eq!((fl.as_str(), owed.clone()), ("f27", vec!["FINEMOTION".to_string(), "PRTSCR".into(), "JOBSCAN".into()]));
    mica::write_owed(&mut fs, &fl, &owed).unwrap();
    assert_eq!(mica::read_owed(&mut fs).unwrap(), Some((fl.clone(), owed.clone())));
    assert_eq!(mica::owed_from_repo(&repo, Some(26)).unwrap(), ("f26".to_string(), vec!["OLDTHING".to_string()]));
    let briefs = mica::Briefs::index(&repo);
    let back = mica::load(&mut fs).unwrap();
    let l = mica::next_lines(&back, &fl, &owed, "rmbp", 13, &briefs);
    // §3 first (JOBSCAN is running — skipped), then the GPU slots (CEFLY is the queue's, not the ledger's), then seq
    assert_eq!(l[0], "[jobs] next flight=f27 track=rmbp owed=3 open=5 ranked=5 gpu=kepler:002-CEFLY,intel:004-GEN7BLIT");
    assert_eq!(rows(&l), vec!["-/FINEMOTION", "005-PRTSCR/PRTSCR", "002-CEFLY/CEFLY [gpu:kepler]", "004-GEN7BLIT/GEN7BLIT [gpu:intel]", "003-OLDTHING/OLDTHING"]);
    assert!(l[2].ends_with("\topen\t-\tdocs/dev/evidence/x/PRTSCR.md"), "{}", l[2]);
    assert!(l[3].contains("\tB1\t"));
    // cut: the item leaves `next`; an ambiguous id is refused
    assert!(mica::cut(&mut fs, "005-PRTSCR", "", "PRTSCR", "exec-rmbp-prtscr").is_err());
    let r = mica::cut(&mut fs, "005-PRTSCR", "rmbp", "", "exec-rmbp-prtscr").unwrap();
    assert_eq!((r.get(jc::K_STATUS), r.get(jc::K_BRANCH)), ("open", "exec-rmbp-prtscr"));
    let back = mica::load(&mut fs).unwrap();
    let l = mica::next_lines(&back, &fl, &owed, "rmbp", 13, &briefs);
    assert_eq!(rows(&l), vec!["-/FINEMOTION", "002-CEFLY/CEFLY [gpu:kepler]", "004-GEN7BLIT/GEN7BLIT [gpu:intel]", "003-OLDTHING/OLDTHING"]);
    // land: the closed set, a sha; the row's text (so the export) untouched
    assert!(mica::land(&mut fs, "rmbp/005-PRTSCR", "", "green", "a4ad6780").is_err());
    assert!(mica::land(&mut fs, "rmbp/005-PRTSCR", "", "fixed-unflown", "zz").is_err());
    let r = mica::land(&mut fs, "rmbp/005-PRTSCR", "", "fixed-unflown", "a4ad6780").unwrap();
    assert_eq!((r.get(jc::K_STATUS), r.get(jc::K_TIP)), ("fixed-unflown", "a4ad6780"));
    let r = mica::land(&mut fs, "rmbp/B1", "", "fixed-unflown", "a4ad6780").unwrap();
    assert_eq!(r.kind, jc::Kind::Ledger);
    let back = mica::load(&mut fs).unwrap();
    assert!(src.identical(&back), "cut/land changed the export");
    let l = mica::next_lines(&back, &fl, &owed, "rmbp", 2, &briefs);
    assert_eq!(rows(&l), vec!["002-CEFLY/CEFLY [gpu:kepler]", "004-GEN7BLIT/GEN7BLIT [gpu:intel]"]);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_real_repo_ranks_flight_27() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = mica::Sources::read(&repo).unwrap();
    let recs = src.records().unwrap();
    let (fl, owed) = mica::owed_from_repo(&repo, Some(27)).unwrap();
    assert_eq!(fl, "f27");
    assert_eq!(owed.first().map(String::as_str), Some("FINEMOTION"));
    let l = mica::next_lines(&recs, &fl, &owed, "rmbp", 13, &mica::Briefs::index(Path::new(&repo)));
    for x in &l {
        println!("{x}");
    }
    assert!(l[0].contains(" ranked=13 ") && !l[0].contains("kepler:-") && !l[0].contains("intel:-"), "{}", l[0]);
}
