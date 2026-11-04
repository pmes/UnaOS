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

/// VUGSCANTRACK (LEDGER SR493): a recording with no pose source fused through `--track` (scan_core's frame-to-model
/// tracker, SR478). The orbit steps 1° a frame with the D435i's noise, SR478's regime (tests/pose.rs `STEP_DEG`); the
/// fits, carried by frame 0's true pose into the scene, hold twice scan_core's tests/scan.rs bars as tests/pose.rs
/// does. A blank frame in the middle is skipped, counted lost and named, and the rest still fuse.
#[test]
fn a_tracked_prb_recording_fuses_without_poses() {
    let d = dir("track");
    let intr = fixture::bench_intrinsics();
    let truth = |k: u32| fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * DEG);
    let render = |k: u32| fixture::render_depth(&Scene::bench(), &intr, DepthScale::D400_DEFAULT, &truth(k), &Noise::D435I, k as u64);
    let blank = |k: u32| {
        Frame::packed(
            probe_core::StreamKind::Depth,
            probe_core::PixelFormat::Z16,
            intr.width,
            intr.height,
            k as u64 * 33_333,
            k as u64,
            vec![0u8; (intr.width * intr.height * 2) as usize],
        )
        .unwrap()
    };
    let write = |path: &PathBuf, blank_at: Option<u32>| {
        let recs: Vec<Record> = (0..12u32)
            .map(|k| {
                let f = if blank_at == Some(k) { blank(k) } else { render(k) };
                Record::new(f, "replay:d435i", 1000, Some(k4())).unwrap()
            })
            .collect();
        std::fs::write(path, recording::write(&recs).unwrap()).unwrap();
    };
    let v3 = |s: &str| -> Vec3 {
        let n: Vec<f64> = s.trim_matches(|c: char| c == '(' || c == ')').split(',').map(|t| t.parse().unwrap()).collect();
        Vec3::new(n[0], n[1], n[2])
    };
    let deg = |a: Vec3, b: Vec3| (a.dot(b).abs() / (a.mag() * b.mag())).min(1.0).acos() / DEG;
    let z = Vec3::new(0.0, 0.0, 1.0);

    // Twelve tracked frames.
    let (inp, ply) = (d.join("track.prb"), d.join("track.ply"));
    write(&inp, None);
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--track"]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    assert_eq!(field(&report, "frames=", "poses"), "track");
    assert_eq!(field(&report, "frames=", "frames"), "12");
    assert_eq!(field(&report, "track: frames=", "frames"), "12");
    assert_eq!(field(&report, "track: frames=", "tracked"), "11");
    assert_eq!(field(&report, "track: frames=", "lost"), "0");
    // VUGSCANLOOP (SR500): twelve frames over 11 degrees never come back near an old keyframe: no candidate, no closure.
    assert_eq!(field(&report, "loop:", "candidates"), "0");
    assert_eq!(field(&report, "loop:", "closures"), "0");
    assert_eq!((field(&report, "loop:", "before"), field(&report, "loop:", "after")), ("none", "none"));
    let p0 = truth(0);
    // The plane: the ground (z = 0) or the box top (z = 100), within 1° of up, 4 mm of its height, rms <= 4.
    let n = p0.apply_vector(v3(field(&report, "plane:", "normal")));
    let dw: f64 = field(&report, "plane:", "d").parse::<f64>().unwrap() + n.dot(p0.trans);
    let h = dw / n.z;
    assert!(deg(n, z) < 1.0, "plane normal {} deg off up", deg(n, z));
    assert!(h.abs().min((h - 100.0).abs()) < 4.0, "plane height {h} mm");
    let rms: f64 = field(&report, "plane:", "rms_mm").parse().unwrap();
    assert!(rms <= 4.0, "plane rms {rms}");
    // The cylinder: r 40 standing at (160, 0).
    let axis = p0.apply_vector(v3(field(&report, "cylinder:", "axis")));
    let pt = p0.apply_point(v3(field(&report, "cylinder:", "point")));
    let r: f64 = field(&report, "cylinder:", "r").parse().unwrap();
    let on = pt + axis * ((75.0 - pt.z) / axis.z);
    let off = (on.x - 160.0).hypot(on.y);
    assert!(deg(axis, z) < 2.0, "cylinder axis {} deg", deg(axis, z));
    assert!((r - 40.0).abs() < 4.0, "cylinder radius {r}");
    assert!(off < 4.0, "cylinder axis {off} mm off");

    // Frame 6 blank: skipped, counted, named; the other eleven fuse.
    let (inp6, ply6) = (d.join("blank6.prb"), d.join("blank6.ply"));
    write(&inp6, Some(6));
    let o = scan(&[inp6.to_str().unwrap(), "-o", ply6.to_str().unwrap(), "--track"]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    assert_eq!(field(&report, "track: frames=", "lost"), "1");
    assert_eq!(field(&report, "track: frames=", "tracked"), "10");
    assert_eq!(field(&report, "frames=", "frames"), "11");
    assert!(report.lines().any(|l| l == "track: frame=6 lost no_depth"), "{report}");
    // VUGSCANSHAPES (SR497): the cylinder is detect's above the floor, or none — never SR493's 27-inlier r=68.9.
    let cyl = report.lines().find(|l| l.starts_with("cylinder:")).expect("a cylinder line");
    assert!(cyl == "cylinder: none (support < floor)" || (field(&report, "cylinder:", "r").parse::<f64>().unwrap() - 40.0).abs() < 4.0, "{cyl}");

    // The three pose roads are exclusive.
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--track", "--poses", "x.txt"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("PoseFlags"));
}

/// VUGSCANDEVIATION (SR510): a world-posed 12-frame orbit `--against` a design document built to the fixture's box and
/// cylinder: the scan registers from the identity, the report names the deviation and every design face, and the PLY
/// carries the `deviation` scalar and the band's colours.
#[test]
fn an_against_design_prints_the_deviation_and_its_ply() {
    const DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=-230 y=-100
point p2 sketch=s1 x=70 y=-100
point p3 sketch=s1 x=70 y=100
point p4 sketch=s1 x=-230 y=100
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[-230 -100]
fix f2 sketch=s1 p=p2 at=[70 -100]
fix f3 sketch=s1 p=p3 at=[70 100]
fix f4 sketch=s1 p=p4 at=[-230 100]
sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c0 sketch=s2 x=160 y=0
circle c1 sketch=s2 centre=c0 r=40
fix g1 sketch=s2 p=c0 at=[160 0]
radius r1 sketch=s2 circle=c1 r=40
extrude body sketch=s1 outer=l1 dir=[0 0 1] length=100
extrude post sketch=s2 outer=c1 dir=[0 0 1] length=150
"#;
    let d = dir("against");
    let (inp, ply, poses, doc) = (d.join("orbit.prb"), d.join("orbit.ply"), d.join("poses.txt"), d.join("part.vugdoc"));
    let intr = fixture::bench_intrinsics();
    let truth = |k: u32| fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 30.0 * DEG);
    let recs: Vec<Record> = (0..12u32)
        .map(|k| {
            let f = fixture::render_depth(&Scene::bench(), &intr, DepthScale::D400_DEFAULT, &truth(k), &Noise::NONE, k as u64);
            Record::new(f, "replay:d435i", 1000, Some(k4())).unwrap()
        })
        .collect();
    std::fs::write(&inp, recording::write(&recs).unwrap()).unwrap();
    let mut text = String::new();
    for k in 0..12 {
        let p = truth(k);
        let (m, t) = (p.rot.to_mat3(), p.trans);
        let r = [[m.cols[0].x, m.cols[1].x, m.cols[2].x], [m.cols[0].y, m.cols[1].y, m.cols[2].y], [m.cols[0].z, m.cols[1].z, m.cols[2].z]];
        text += &format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}\n",
            r[0][0], r[0][1], r[0][2], t.x, r[1][0], r[1][1], r[1][2], t.y, r[2][0], r[2][1], r[2][2], t.z
        );
    }
    std::fs::write(&poses, text).unwrap();
    std::fs::write(&doc, DOC).unwrap();
    let o = scan(&[
        inp.to_str().unwrap(),
        "-o",
        ply.to_str().unwrap(),
        "--poses",
        poses.to_str().unwrap(),
        "--band",
        "1",
        "--against",
        doc.to_str().unwrap(),
    ]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    let rms: f64 = field(&report, "deviation: aligned", "rms").parse().unwrap();
    assert!(rms < 1.0, "deviation rms {rms}");
    let faces = field(&report, "deviation: aligned", "faces");
    let total: usize = faces.split('/').nth(1).unwrap().parse().unwrap();
    assert_eq!(report.lines().filter(|l| l.starts_with("face: ")).count(), total, "one face line per design face");
    assert!(report.lines().any(|l| l.starts_with("face: post/") && l.contains(":cylinder ")), "the post's cylinder is named");
    let verts: usize = field(&report, "vertices=", "vertices").parse().unwrap();
    let bytes = std::fs::read(&ply).unwrap();
    let mesh = mesh_core::ply::read(&bytes).unwrap();
    assert_eq!(mesh.positions.len(), verts);
    let head = String::from_utf8_lossy(&bytes[..bytes.windows(10).position(|w| w == b"end_header").unwrap()]).into_owned();
    assert!(head.contains("property float deviation") && head.contains("property uchar red"), "{head}");
}

/// VUGINTENTSCANWIRE (SR515): a plate with a boss from scan_core's fixture (the ground rectangle ±200 at z = 0 is the
/// plate's top, an r 40 boss 500 tall stands over the origin lifted 30 mm off it: a touching boss's inside corner is
/// rounded by the TSDF into a skirt detect reads as a cone, VUG-INTENT-SCAN-UNMATCHED), a world-posed 12-frame orbit
/// whose eye (z 446) sits under the boss top (z 530) and over its bottom so neither cap is seen: one plane and one
/// cylinder. `--intent <doc>` prints doc_core's measurement
/// table, then suggest's proposals each disposed by verify_all; `--intent` alone prints the table only.
#[test]
fn an_intent_scan_prints_the_shapes_and_verified_proposals() {
    const DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 -10] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=-200 y=-200
point p2 sketch=s x=200 y=-200
point p3 sketch=s x=200 y=200
point p4 sketch=s x=-200 y=200
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
extrude plate sketch=s outer=l1 dir=[0 0 1] length=10
sketch b o=[0 0 30] z=[0 0 1] x=[1 0 0]
point c0 sketch=b x=0 y=0
circle c1 sketch=b centre=c0 r=40
extrude boss sketch=b outer=c1 dir=[0 0 1] length=500
"#;
    let scene = Scene {
        ground_centre: Vec3::ZERO,
        ground: [200.0, 200.0],
        boxes: Vec::new(),
        cylinders: vec![fixture::CylPrim { base: Vec3::new(0.0, 0.0, 30.0), radius: 40.0, height: 500.0 }],
    };
    let d = dir("intent");
    let (inp, ply, poses, doc) = (d.join("boss.prb"), d.join("boss.ply"), d.join("poses.txt"), d.join("part.vug"));
    let intr = fixture::bench_intrinsics();
    let truth = |k: u32| fixture::orbit(Vec3::new(0.0, 0.0, 60.0), 600.0, 40.0 * DEG, k as f64 * 30.0 * DEG);
    let recs: Vec<Record> = (0..12u32)
        .map(|k| {
            let f = fixture::render_depth(&scene, &intr, DepthScale::D400_DEFAULT, &truth(k), &Noise::NONE, k as u64);
            Record::new(f, "replay:d435i", 1000, Some(k4())).unwrap()
        })
        .collect();
    std::fs::write(&inp, recording::write(&recs).unwrap()).unwrap();
    let mut text = String::new();
    for k in 0..12 {
        let p = truth(k);
        let (m, t) = (p.rot.to_mat3(), p.trans);
        let r = [[m.cols[0].x, m.cols[1].x, m.cols[2].x], [m.cols[0].y, m.cols[1].y, m.cols[2].y], [m.cols[0].z, m.cols[1].z, m.cols[2].z]];
        text += &format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}\n",
            r[0][0], r[0][1], r[0][2], t.x, r[1][0], r[1][1], r[1][2], t.y, r[2][0], r[2][1], r[2][2], t.z
        );
    }
    std::fs::write(&poses, text).unwrap();
    std::fs::write(&doc, DOC).unwrap();
    let (inp, ply, poses, doc) = (inp.to_str().unwrap(), ply.to_str().unwrap(), poses.to_str().unwrap(), doc.to_str().unwrap());
    let count = |report: &str, head: &str| report.lines().filter(|l| l.starts_with(head)).count();

    let o = scan(&[inp, "-o", ply, "--poses", poses, "--intent", doc]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    assert_eq!(count(&report, "intent: plane "), 1, "one plane in the table\n{report}");
    assert_eq!(count(&report, "intent: cylinder "), 1, "one cylinder in the table\n{report}");
    let accepted: usize = field(&report, "intent: proposed=", "accepted").parse().unwrap();
    assert!(accepted >= 1 && count(&report, "accept ") == accepted, "an accepted proposal\n{report}");

    // `--intent` with no document: the table alone.
    let o = scan(&[inp, "-o", ply, "--poses", poses, "--intent"]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert!(count(&report, "intent: shapes: n=") == 1 && count(&report, "intent: cylinder ") == 1, "{report}");
    assert_eq!(count(&report, "intent: proposed="), 0, "{report}");
}

/// VUGSCANSHAPESPREAD (SR526): every `shape:` line carries its measured `spread_deg=`, and `--intent` hands doc_core
/// the largest as sigma_deg (doc_core prints it to 2 places and the shape line to 3: within 0.0055), no longer the 0.5
/// default.
#[test]
fn the_intent_sigma_deg_is_the_largest_shape_spread() {
    let d = dir("spread");
    let (inp, ply) = (d.join("one.prb"), d.join("one.ply"));
    prb(&inp, 1, true);
    let o = scan(&[inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--intent"]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    let spreads: Vec<f64> = report
        .lines()
        .filter(|l| l.starts_with("shape: "))
        .map(|l| field(l, "shape: ", "spread_deg").parse::<f64>().unwrap())
        .collect();
    assert!(!spreads.is_empty(), "a shape\n{report}");
    assert!(spreads.iter().all(|s| s.is_finite() && *s >= 0.0), "{spreads:?}");
    let largest = spreads.iter().cloned().fold(0.0, f64::max);
    let sigma: f64 = field(&report, "intent: shapes:", "sigma_deg").parse().unwrap();
    assert!((sigma - largest).abs() <= 0.0055 + 1e-9, "sigma_deg {sigma} vs the largest spread {largest}\n{report}");
}

/// VUGSCANGDT (SR542): the SR510 world-posed orbit `--against` its design document (its own copy of that test's
/// setup), read as ISO 1101 tolerances: every design face gets a `gdt:` line and no verdict when no tolerance is
/// named; the box top's flatness and the post's position named with `--gdt` both read PASS; an unknown face is refused
/// `GdtFace` (exit 1) and a malformed callout is usage (exit 2).
#[test]
fn a_gdt_against_design_reads_each_face_and_the_named_verdicts() {
    const DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=-230 y=-100
point p2 sketch=s1 x=70 y=-100
point p3 sketch=s1 x=70 y=100
point p4 sketch=s1 x=-230 y=100
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[-230 -100]
fix f2 sketch=s1 p=p2 at=[70 -100]
fix f3 sketch=s1 p=p3 at=[70 100]
fix f4 sketch=s1 p=p4 at=[-230 100]
sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c0 sketch=s2 x=160 y=0
circle c1 sketch=s2 centre=c0 r=40
fix g1 sketch=s2 p=c0 at=[160 0]
radius r1 sketch=s2 circle=c1 r=40
extrude body sketch=s1 outer=l1 dir=[0 0 1] length=100
extrude post sketch=s2 outer=c1 dir=[0 0 1] length=150
"#;
    let d = dir("gdt");
    let (inp, ply, poses, doc) = (d.join("orbit.prb"), d.join("orbit.ply"), d.join("poses.txt"), d.join("part.vugdoc"));
    let intr = fixture::bench_intrinsics();
    let truth = |k: u32| fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 30.0 * DEG);
    let recs: Vec<Record> = (0..12u32)
        .map(|k| {
            let f = fixture::render_depth(&Scene::bench(), &intr, DepthScale::D400_DEFAULT, &truth(k), &Noise::NONE, k as u64);
            Record::new(f, "replay:d435i", 1000, Some(k4())).unwrap()
        })
        .collect();
    std::fs::write(&inp, recording::write(&recs).unwrap()).unwrap();
    let mut text = String::new();
    for k in 0..12 {
        let p = truth(k);
        let (m, t) = (p.rot.to_mat3(), p.trans);
        let r = [[m.cols[0].x, m.cols[1].x, m.cols[2].x], [m.cols[0].y, m.cols[1].y, m.cols[2].y], [m.cols[0].z, m.cols[1].z, m.cols[2].z]];
        text += &format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}\n",
            r[0][0], r[0][1], r[0][2], t.x, r[1][0], r[1][1], r[1][2], t.y, r[2][0], r[2][1], r[2][2], t.z
        );
    }
    std::fs::write(&poses, text).unwrap();
    std::fs::write(&doc, DOC).unwrap();
    let base = [inp.to_str().unwrap(), "-o", ply.to_str().unwrap(), "--poses", poses.to_str().unwrap(), "--band", "1", "--against", doc.to_str().unwrap()];
    let run = |extra: &[&str]| scan(&[&base[..], extra].concat());

    // No tolerance named: one gdt line per face line, no verdict.
    let o = run(&[]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    let gdt: Vec<&str> = report.lines().filter(|l| l.starts_with("gdt: ") && !l.starts_with("gdt: callouts=")).collect();
    assert_eq!(gdt.len(), report.lines().filter(|l| l.starts_with("face: ")).count(), "one gdt line per design face");
    assert!(gdt.iter().all(|l| !l.contains("verdict=") && l.contains(" n=")), "no verdict without a tolerance");
    assert!(report.lines().any(|l| l == "gdt: callouts=0 pass=0 fail=0"), "{report}");
    let name = |l: &str| l["gdt: ".len()..l.find(" surface=").unwrap()].to_string();
    let num = |l: &str, key: &str| -> f64 {
        let at = l.find(&format!(" {key}=")).unwrap_or_else(|| panic!("no {key} in {l}")) + key.len() + 2;
        l[at..].split_whitespace().next().unwrap().parse().unwrap_or_else(|_| panic!("{key} in {l}"))
    };
    let top = gdt
        .iter()
        .copied()
        .find(|l| {
            let l: &str = l;
            l.contains(" surface=plane ") && (num(l, "d") - 100.0).abs() < 0.01 && l.contains(" normal=(") && {
                let n = &l[l.find(" normal=(").unwrap() + 9..];
                n[..n.find(')').unwrap()].split(',').nth(2).unwrap().parse::<f64>().unwrap() > 0.99
            }
        })
        .unwrap_or_else(|| panic!("no box top in\n{report}"));
    let post = gdt.iter().copied().find(|l| l.contains(" surface=cylinder ")).unwrap_or_else(|| panic!("no post side in\n{report}"));
    assert!(num(top, "flatness") < 1.0 && num(top, "band") >= 0.0, "{top}");
    assert!(num(post, "cylindricity") < 1.0 && num(post, "position") < 1.0 && (num(post, "r") - 40.0).abs() < 0.05, "{post}");

    // Two named tolerances: both read, both PASS.
    let (top_spec, post_spec) = (format!("{}=flatness:1", name(top)), format!("{}=position:1", name(post)));
    let o = run(&["--gdt", &top_spec, "--gdt", &post_spec]);
    let report = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    println!("{report}");
    assert!(report.lines().any(|l| l.starts_with(&format!("gdt: {} ", name(top))) && l.ends_with(" verdict=flatness:1 PASS")), "{report}");
    assert!(report.lines().any(|l| l.starts_with(&format!("gdt: {} ", name(post))) && l.ends_with(" verdict=position:1 PASS")), "{report}");
    assert!(report.lines().any(|l| l == "gdt: callouts=2 pass=2 fail=0"), "{report}");

    // An unknown face is refused by name; a malformed callout is usage.
    let o = run(&["--gdt", "nosuch=flatness:1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("GdtFace"), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(&["--gdt", "nosuch=roundness:1"]);
    assert_eq!(o.status.code(), Some(2));
}
