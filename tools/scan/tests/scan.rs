// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANTOOL (LEDGER SR471): the `scan` binary run on scan_core's fixture written as the files a D435i road
//! produces — a `.prb` recording, an rs-convert metres CSV — and on the ways those files are refused.

use std::path::PathBuf;
use std::process::{Command, Output};

use probe_core::recording::{self, Record};
use scan_core::fixture::{self, Noise, Scene};
use scan_core::{DepthScale, Frame, Transform, Vec3};

const DEG: f64 = std::f64::consts::PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scan-tool").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pose(k: u32) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 0.2)
}

fn frame(k: u32) -> Frame {
    let intr = fixture::bench_intrinsics();
    fixture::render_depth(&Scene::bench(), &intr, DepthScale::D400_DEFAULT, &pose(k), &Noise::NONE, k as u64)
}

fn k4() -> [f32; 4] {
    let i = fixture::bench_intrinsics();
    [i.fx, i.fy, i.ppx, i.ppy]
}

fn prb(path: &PathBuf, frames: u32, intrinsics: bool) {
    let recs: Vec<Record> = (0..frames)
        .map(|k| Record::new(frame(k), "replay:d435i", 1000, intrinsics.then(k4)).unwrap())
        .collect();
    std::fs::write(path, recording::write(&recs).unwrap()).unwrap();
}

fn scan(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scan")).args(args).output().unwrap()
}

fn field<'a>(report: &'a str, line: &str, key: &str) -> &'a str {
    let l = report.lines().find(|l| l.starts_with(line)).unwrap_or_else(|| panic!("no {line} in\n{report}"));
    let at = l.find(&format!("{key}=")).unwrap_or_else(|| panic!("no {key} in {l}")) + key.len() + 1;
    l[at..].split_whitespace().next().unwrap()
}

#[test]
fn one_prb_frame_meshes_and_fits_its_plane() {
    let d = dir("prb1");
    let (inp, ply, rep) = (d.join("one.prb"), d.join("one.ply"), d.join("one.txt"));
    prb(&inp, 1, true);
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--report", rep.to_str().unwrap()]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    assert_eq!(std::fs::read_to_string(&rep).unwrap(), report);
    assert_eq!(field(&report, "intrinsics=", "intrinsics"), "record");
    assert_eq!(field(&report, "frames=", "frames"), "1");
    assert_eq!(field(&report, "confidence:", "min"), "1.000");
    let verts: usize = field(&report, "vertices=", "vertices").parse().unwrap();
    assert!(verts > 1000, "{verts}");
    let mesh = mesh_core::ply::read(&std::fs::read(&ply).unwrap()).unwrap();
    assert_eq!(mesh.positions.len(), verts);
    let rms: f64 = field(&report, "plane:", "rms_mm").parse().unwrap();
    assert!(rms <= 2.0, "plane rms {rms}");
    let loops: usize = field(&report, "boundary_loops=", "boundary_loops").parse().expect("rim named as loops (VUGSCANRIM SR474)");
    assert!(loops >= 1, "an open scan has a rim");
    assert!(report.lines().any(|l| l.starts_with("scan: frames=1 ")));
}

#[test]
fn a_posed_prb_recording_fuses_every_frame() {
    let d = dir("poses");
    let (inp, ply, poses) = (d.join("three.prb"), d.join("three.ply"), d.join("poses.txt"));
    prb(&inp, 3, true);
    let mut text = String::from("# camera -> world, row-major [R | t], mm\n");
    for k in 0..3 {
        let p = pose(k);
        let (m, t) = (p.rot.to_mat3(), p.trans); let r = [[m.cols[0].x, m.cols[1].x, m.cols[2].x], [m.cols[0].y, m.cols[1].y, m.cols[2].y], [m.cols[0].z, m.cols[1].z, m.cols[2].z]];
        text += &format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}\n",
            r[0][0], r[0][1], r[0][2], t.x, r[1][0], r[1][1], r[1][2], t.y, r[2][0], r[2][1], r[2][2], t.z
        );
    }
    std::fs::write(&poses, text).unwrap();
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--poses", poses.to_str().unwrap()]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(field(&report, "frames=", "frames"), "3");
    assert_eq!(field(&report, "frames=", "poses"), "file");
    let rms: f64 = field(&report, "plane:", "rms_mm").parse().unwrap();
    assert!(rms <= 2.0, "plane rms {rms}");
}

#[test]
fn an_rs_convert_csv_meshes_with_the_intrinsics_flags() {
    let d = dir("csv");
    let (inp, ply) = (d.join("depth.csv"), d.join("depth.ply"));
    let f = frame(0);
    // converter-csv.cpp:83-90: one line per row, `get_distance` metres comma-separated, 0 for a hole.
    let mut text = String::new();
    for y in 0..f.height {
        let row: Vec<String> =
            (0..f.width).map(|x| format!("{}", f.u16_at(x, y).unwrap() as f64 * 0.001)).collect();
        text += &row.join(",");
        text.push('\n');
    }
    std::fs::write(&inp, text).unwrap();
    let k = k4().map(|v| v.to_string());
    let o = scan(&[
        inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--fx", &k[0], "--fy", &k[1], "--ppx", &k[2], "--ppy", &k[3],
    ]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(field(&report, "intrinsics=", "intrinsics"), "flags");
    let rms: f64 = field(&report, "plane:", "rms_mm").parse().unwrap();
    assert!(rms <= 2.0, "plane rms {rms}");
    // Without the flags the CSV has no intrinsics.
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("NoIntrinsics"));
}

#[test]
fn refusals_are_named() {
    let d = dir("refuse");
    let ply = d.join("x.ply");
    let ply = ply.to_str().unwrap();
    let stderr = |o: &Output| String::from_utf8_lossy(&o.stderr).into_owned();
    // Several frames, no pose source.
    let many = d.join("three.prb");
    prb(&many, 3, true);
    let o = scan(&[many.to_str().unwrap(), "-o", ply]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("ManyFramesNoPose"), "{}", stderr(&o));
    // ... unless one is picked.
    assert!(scan(&[many.to_str().unwrap(), "-o", ply, "--frame", "2"]).status.success());
    // A truncated recording.
    let bytes = std::fs::read(&many).unwrap();
    let cut = d.join("cut.prb");
    std::fs::write(&cut, &bytes[..bytes.len() - 7]).unwrap();
    let o = scan(&[cut.to_str().unwrap(), "-o", ply]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Truncated"), "{}", stderr(&o));
    // A kernel-shaped record (no intrinsics) and no flags; a partial flag set.
    let bare = d.join("bare.prb");
    prb(&bare, 1, false);
    let o = scan(&[bare.to_str().unwrap(), "-o", ply]);
    assert!(stderr(&o).contains("NoIntrinsics"), "{}", stderr(&o));
    let o = scan(&[bare.to_str().unwrap(), "-o", ply, "--fx", "212"]);
    assert!(stderr(&o).contains("PartialIntrinsics"), "{}", stderr(&o));
    // A turntable without its radius.
    let o = scan(&[many.to_str().unwrap(), "-o", ply, "--turntable", "10"]);
    assert!(stderr(&o).contains("TurntableNoRadius"), "{}", stderr(&o));
}
