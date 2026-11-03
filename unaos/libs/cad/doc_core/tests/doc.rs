//! VUGDOC (LEDGER SR480): the design document round-trips byte for byte, replays to the same solids the cores give on
//! the direct call, names a broken statement by its line, writes every f64 so it reads back to the same bits, and
//! reads a hand-edited copy to the same Doc.

use brep_core::{Frame, extrude, properties, revolve};
use doc_core::{Cons, Doc, Fault, Failure, PRef, Stmt, What, brep_profile, f64_text, read, replay, write};
use make_core::Move;
use sketch_core::{Constraint, PointRef, Sketch, Tol, Vec2, profiles, solve};
use std::f64::consts::PI;

/// Two sketches, one extrude, one revolve, a machine with two Ops.
const DOC_A: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=0 y=0
point p2 sketch=s1 x=10 y=0
point p3 sketch=s1 x=10 y=8
point p4 sketch=s1 x=0 y=8
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
horizontal h1 sketch=s1 line=l1
horizontal h2 sketch=s1 line=l3
vertical v1 sketch=s1 line=l2
vertical v2 sketch=s1 line=l4
fix f1 sketch=s1 p=p1 at=[0 0]
distance d1 sketch=s1 a=p1 b=p2 d=10
distance d2 sketch=s1 a=p2 b=p3 d=8
sketch s2 o=[0 0 0] z=[0 -1 0] x=[1 0 0]
point q1 sketch=s2 x=2 y=0
point q2 sketch=s2 x=5 y=0
point q3 sketch=s2 x=5 y=6
point q4 sketch=s2 x=2 y=6
line m1 sketch=s2 a=q1 b=q2
line m2 sketch=s2 a=q2 b=q3
line m3 sketch=s2 a=q3 b=q4
line m4 sketch=s2 a=q4 b=q1
fix g1 sketch=s2 p=q1 at=[2 0]
fix g2 sketch=s2 p=q2 at=[5 0]
fix g3 sketch=s2 p=q3 at=[5 6]
fix g4 sketch=s2 p=q4 at=[2 6]
point qc sketch=s2 x=30 y=30
arc a1 sketch=s2 centre=qc r=2 start=0 end=1.5
radius r1 sketch=s2 circle=a1 r=2
extrude e1 sketch=s1 outer=l1 dir=[0 0 1] length=5
revolve e2 sketch=s2 outer=m1 axis_o=[0 0] axis_d=[0 1] angle=6.283185307179586
machine mill name="Shop mill \"A\"" kind=mill3 kinematics=cartesian units=mm envelope=[[-50 -50 -60] [100 100 50]] safe_z=10 power=spindle power_max=24000 ask_above=20000 loaded=1
axis axis=x min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000
axis axis=y min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000
axis axis=z min=-60 max=50 max_feed=2000 max_accel=300 rapid=4000
tool number=1 kind=flat_end dc=6 lf=40 apmx=2 nof=2 sig=0
tool number=2 kind=drill dc=3 lf=50 apmx=10 nof=2 sig=2.0943951023931953
stock shape=box size=[10 8 5] origin=[0 0 0]
fixture name="vise" keepout=[[[-40 -10 -20] [-20 20 5]]]
drill dr1 on=e1 points=[[2 2] [8 6]] top=5 depth=4 peck=1 tool=2 feed=300 plunge=100 rpm=10000
contour ct1 on=e1 profile=[[0 0] [10 0] [10 8] [0 8]] top=5 depth=5 side=outside tool=1 feed=800 plunge=200 rpm=18000
"#;

/// Every statement kind: a circle, the nineteen constraints, the six Ops (the refused ones as data), a cylinder stock,
/// a scan with a poses file, strings needing every escape.
const DOC_ALL: &str = r#"vug-doc version=1 lin=0.000001 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point a sketch=s x=0 y=0
point b sketch=s x=10 y=0
point c sketch=s x=3 y=4
line l sketch=s a=a b=b
line k sketch=s a=b b=c
circle o sketch=s centre=c r=2.5
arc w sketch=s centre=a r=1 start=0.1 end=3
coincident c01 sketch=s a=l.start b=w.end
on_line c02 sketch=s p=c line=l
on_circle c03 sketch=s p=b circle=o
horizontal c04 sketch=s line=l
vertical c05 sketch=s line=k
parallel c06 sketch=s a=l b=k
perpendicular c07 sketch=s a=l b=k
tangent_line_circle c08 sketch=s line=k circle=o
tangent_circles c09 sketch=s a=o b=w
equal_length c10 sketch=s a=l b=k
equal_radius c11 sketch=s a=o b=w
distance c12 sketch=s a=a b=w.centre d=12.5
distance_point_line c13 sketch=s p=c line=l d=-3e-7
angle c14 sketch=s a=l b=k angle=1.5707963267948966
radius c15 sketch=s circle=o r=2.5
diameter c16 sketch=s circle=w d=2
fix c17 sketch=s p=a at=[0 0]
midpoint c18 sketch=s p=c line=l
symmetric c19 sketch=s a=a b=k.end line=l
extrude f sketch=s outer=l dir=[0 0 1] length=1e-300
machine m name="tab\there \\ back \"q\" ü \u{1}" kind=fdm3 kinematics=cartesian units=inch envelope=[[0 0 0] [10 10 10]] safe_z=5 power=extruder power_max=260 ask_above=240
axis axis=c min=-1 max=1 max_feed=1 max_accel=1 rapid=1
tool number=7 kind=nozzle dc=0.4 lf=10 apmx=0.2 nof=1 sig=0
stock shape=cylinder radius=3 height=4 origin=[1 2 3]
fixture name="" keepout=[]
drill o1 on=f points=[] top=0 depth=1 peck=0 tool=7 feed=1 plunge=1 rpm=1
contour o2 on=f profile=[[0 0] [1 0] [1 1]] top=0 depth=1 side=inside tool=7 feed=1 plunge=1 rpm=1
pocket o3 on=f outline=[[0 0] [4 0] [4 4]] islands=[[[1 1] [2 1] [2 2]] []] top=0 depth=1 stepover=0.5 chord=0.01 climb=yes tool=7 feed=1 plunge=1 rpm=1
face o4 on=f lo=[0 0] hi=[4 4] top=0 depth=0.25 stepover=2 along=x tool=7 feed=1 plunge=1 rpm=1
slice o7 on=f triangles=[[[0 0 0] [1 0 0] [0 1 0]] [[0 0 0] [0 1 0] [0 0 1]]] tool=7 layer_h=0.2 first_layer=0.3 width=0.45 perimeters=2 density=0.2 top_layers=3 bottom_layers=3 filament=1.75 print_feed=3000 first_layer_feed=1200 retract=1 retract_feed=2400 retract_min_travel=2 z_hop=0.2 hotend_c=210 bed_c=60
print_layers o5 on=f slices=[[0.2 [[[0 0] [1 0] [1 1]]]] [0.4 []]] tool=7
laser_cut o6 on=f polylines=[[[0 0] [1 1]] [[2 2] [3 3] [4 4]]] passes=3
scan sc path="scans/part 1.prb" pose=file poses="scans/poses.txt" confidence=0.75
scan sd path="x" pose=icp confidence=1
"#;

fn round_trip(text: &str) -> Doc {
    let d = read(text).unwrap_or_else(|r| panic!("read: {r:?}"));
    let s1 = write(&d);
    let d2 = read(&s1).unwrap_or_else(|r| panic!("re-read: {r:?}\n{s1}"));
    assert_eq!(d2, d, "read(write(d)) == d");
    let s2 = write(&d2);
    assert_eq!(s1, s2, "write -> read -> write byte-equal");
    d
}

#[test]
fn two_sketches_extrude_revolve_machine_two_ops_round_trip_byte_equal() {
    let d = round_trip(DOC_A);
    assert_eq!(d.stmts.len(), 44);
    assert_eq!(d.line(0), 2);
    let ops = d.stmts.iter().filter(|s| matches!(s, Stmt::Op { .. })).count();
    assert_eq!(ops, 2);
}

#[test]
fn every_statement_kind_round_trips() {
    let d = round_trip(DOC_ALL);
    let n = d.stmts.iter().filter(|s| matches!(s, Stmt::Constraint { .. })).count();
    assert_eq!(n, 19, "all nineteen constraints");
    let name = d.stmts.iter().find_map(|s| if let Stmt::Machine(m) = s { Some(m.name.clone()) } else { None });
    assert_eq!(name.as_deref(), Some("tab\there \\ back \"q\" ü \u{1}"));
    assert!(d.stmts.contains(&Stmt::Constraint {
        id: "c01".into(),
        sketch: "s".into(),
        c: Cons::Coincident(PRef::Start("l".into()), PRef::End("w".into())),
    }));
    // A Doc built in code writes and reads back; its lines are where `write` puts them.
    let mut built = Doc::new(Tol { lin: 1e-6, ang: 1e-9 });
    built.stmts = d.stmts.clone();
    let again = read(&write(&built)).unwrap();
    assert_eq!(again, d);
    assert_eq!(again.lines, (0..d.stmts.len() as u32).map(|i| i + 2).collect::<Vec<_>>());
    assert_eq!(built.line(3), 5);
}

/// The direct call: the same sketch through sketch_core, profiles, brep_core.
fn direct_s1() -> f64 {
    let mut sk = Sketch::new();
    let p: Vec<_> = [(0.0, 0.0), (10.0, 0.0), (10.0, 8.0), (0.0, 8.0)].iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    let l: Vec<_> = (0..4).map(|i| sk.add_line(p[i], p[(i + 1) % 4]).unwrap()).collect();
    for c in [
        Constraint::Horizontal(l[0]),
        Constraint::Horizontal(l[2]),
        Constraint::Vertical(l[1]),
        Constraint::Vertical(l[3]),
        Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0)),
        Constraint::Distance(PointRef::Point(p[0]), PointRef::Point(p[1]), 10.0),
        Constraint::Distance(PointRef::Point(p[1]), PointRef::Point(p[2]), 8.0),
    ] {
        sk.add(c).unwrap();
    }
    solve(&mut sk, Tol { lin: 1e-6, ang: 1e-9 }).unwrap();
    let pr = profiles(&sk, Tol { lin: 1e-6, ang: 1e-9 });
    let pr = pr.iter().find(|q| q.outer.edges.iter().any(|e| e.src() == l[0])).unwrap();
    let bt = brep_core::dep::Tol { lin: 1e-6, ang: 1e-9 };
    let fr = Frame::new(brep_core::dep::Vec3::ZERO, brep_core::dep::Vec3::Z, brep_core::dep::Vec3::X).unwrap();
    let b = extrude(&brep_profile(pr), &fr, brep_core::dep::Vec3::Z, 5.0, bt).unwrap();
    properties(&b, bt).unwrap().volume
}

fn direct_s2() -> f64 {
    let t = Tol { lin: 1e-6, ang: 1e-9 };
    let mut sk = Sketch::new();
    let q: Vec<_> = [(2.0, 0.0), (5.0, 0.0), (5.0, 6.0), (2.0, 6.0)].iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    let m: Vec<_> = (0..4).map(|i| sk.add_line(q[i], q[(i + 1) % 4]).unwrap()).collect();
    for (i, at) in [(2.0, 0.0), (5.0, 0.0), (5.0, 6.0), (2.0, 6.0)].iter().enumerate() {
        sk.add(Constraint::Fix(PointRef::Point(q[i]), Vec2::new(at.0, at.1))).unwrap();
    }
    let qc = sk.add_point(30.0, 30.0);
    let a1 = sk.add_arc(qc, 2.0, 0.0, 1.5).unwrap();
    sk.add(Constraint::Radius(a1, 2.0)).unwrap();
    solve(&mut sk, t).unwrap();
    let pr = profiles(&sk, t);
    let pr = pr.iter().find(|p| p.outer.edges.iter().any(|e| e.src() == m[0])).unwrap();
    let bt = brep_core::dep::Tol { lin: 1e-6, ang: 1e-9 };
    let fr = Frame::new(brep_core::dep::Vec3::ZERO, -brep_core::dep::Vec3::Y, brep_core::dep::Vec3::X).unwrap();
    let b = revolve(
        &brep_profile(pr),
        &fr,
        brep_core::dep::Vec2::new(0.0, 0.0),
        brep_core::dep::Vec2::new(0.0, 1.0),
        6.283185307179586,
        bt,
    )
    .unwrap();
    properties(&b, bt).unwrap().volume
}

#[test]
fn replay_gives_the_volume_the_direct_call_gives() {
    let d = read(DOC_A).unwrap();
    let r = replay(&d, d.tol);
    assert_eq!(r.failures, Vec::<Failure>::new(), "no failures");
    assert_eq!(r.sketches.len(), 2);
    let ids: Vec<&str> = r.solids.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["e1", "e2"]);
    let bt = brep_core::dep::Tol { lin: 1e-6, ang: 1e-9 };
    let v1 = properties(&r.solids[0].brep, bt).unwrap().volume;
    let v2 = properties(&r.solids[1].brep, bt).unwrap().volume;
    assert_eq!(v1, direct_s1(), "extrude: replay == direct, bit for bit");
    assert_eq!(v2, direct_s2(), "revolve: replay == direct, bit for bit");
    assert!((v1 - 400.0).abs() < 1e-6, "10 x 8 x 5: {v1}");
    assert!((v2 - 126.0 * PI).abs() < 1e-6, "tube pi (25 - 4) 6: {v2}");
}

#[test]
fn the_program_carries_the_tool_across_ops() {
    let d = read(DOC_A).unwrap();
    let r = replay(&d, d.tol);
    let p = r.program.expect("both Ops generate");
    let changes: Vec<u32> =
        p.moves.iter().filter_map(|m| if let Move::ToolChange { tool } = m { Some(*tool) } else { None }).collect();
    // Loaded T1; the drill changes to T2; the contour must change BACK to T1 (op 2's prelude sees T2 loaded).
    assert_eq!(changes, [2, 1]);
}

fn line_of(text: &str, prefix: &str) -> u32 {
    text.lines().position(|l| l.starts_with(prefix)).unwrap() as u32 + 1
}

#[test]
fn a_broken_constraint_still_reads_and_replay_names_its_line() {
    let text = DOC_A
        .replace(
            "distance d2 sketch=s1 a=p2 b=p3 d=8\n",
            "distance d2 sketch=s1 a=p2 b=p3 d=8\nvertical bad sketch=s1 line=l1\nline lx sketch=s1 a=l1 b=p2\n",
        );
    let d = read(&text).expect("a broken constraint is geometry, not syntax: the document reads");
    let r = replay(&d, d.tol);
    let bad = line_of(&text, "vertical bad");
    let lx = line_of(&text, "line lx");
    let e1 = line_of(&text, "extrude e1");
    assert!(
        r.failures.contains(&Failure { line: bad, id: "bad".into(), fault: Fault::Conflicting }),
        "the conflicting constraint named by its line: {:?}",
        r.failures
    );
    assert!(r.failures.contains(&Failure { line: lx, id: "lx".into(), fault: Fault::BadRef("l1".into()) }));
    assert!(r.failures.contains(&Failure { line: e1, id: "e1".into(), fault: Fault::Upstream("s1".into()) }));
    assert!(r.failures.iter().any(|f| f.id == "dr1" && f.fault == Fault::Upstream("e1".into())));
    assert!(r.program.is_none(), "a Program missing an Op is not the design's Make");
    assert_eq!(r.solids.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["e2"], "the other sketch still builds");
    // Never dropped: the document still holds and writes every statement.
    assert!(write(&d).contains("vertical bad sketch=s1 line=l1\n"));
    assert!(r.failures.windows(2).all(|w| w[0].line <= w[1].line));
}

#[test]
fn a_hand_edited_document_reads_to_the_same_doc() {
    let canonical = "vug-doc version=1 lin=1e-6 ang=1e-9\n\
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n\
point p1 sketch=s1 x=0 y=0\n\
point p2 sketch=s1 x=10 y=0\n\
point p3 sketch=s1 x=10 y=8\n\
line l1 sketch=s1 a=p1 b=p2\n\
line l2 sketch=s1 a=p2 b=p3\n\
line l3 sketch=s1 a=p3 b=p1\n\
extrude e1 sketch=s1 outer=l1 dir=[0 0 1] length=2.5\n\
machine m name=\"Shop \\\"A\\\"\" kind=mill3 kinematics=cartesian units=mm envelope=[[0 0 -10] [10 10 10]] safe_z=5 power=spindle power_max=100 ask_above=90\n";
    let hand = "# a hand-edited Vug document\n\
\n\
vug-doc   ang=0.000000001 version=1 lin=0.000001   # keys reordered, numbers spelled long\n\
sketch s1 x=[1 0 0] o=[0  0 0] z=[ 0 0 1 ]\r\n\
point p1 y=0 x=0.0 sketch=s1  # the origin\n\
point p2 sketch=s1 y=0 x=1e1\n\
\t# an indented comment\n\
point p3 x=10 y=8 sketch=s1\n\
line l1 b=p2 a=p1 sketch=s1\n\
line l2 sketch=s1 b=p3 a=p2\n\
line l3 sketch=s1 a=p3 b=p1 # closes the triangle\n\
extrude e1 length=2.50 dir=[0 0 1] outer=l1 sketch=s1\n\
machine m safe_z=5 name=\"Shop \\\"A\\\"\" units=mm kind=mill3 kinematics=cartesian power=spindle ask_above=90 power_max=100 envelope=[[0 0 -10] [10 10 10]]\n";
    let a = read(canonical).unwrap();
    let b = read(hand).unwrap();
    assert_eq!(a, b);
    assert_ne!(a.lines, b.lines, "the lines differ; the Doc does not");
    assert_eq!(write(&b), canonical);
}

#[test]
fn f64_round_trip_a_thousand_values_with_subnormals() {
    let mut v: Vec<f64> = vec![
        0.0,
        -0.0,
        0.1,
        1.0 / 3.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        f64::EPSILON,
        5e-324,
        -5e-324,
        1e-5,
        9.999999999999999e-6,
        1e16,
        9999999999999998.0,
        123456.789,
        -2.5e-310,
        2.2250738585072009e-308,
    ];
    for k in 1..=100u64 {
        let bits = (k.wrapping_mul(0x0000_9e37_79b9_7f4b)) & 0x000f_ffff_ffff_ffff;
        v.push(f64::from_bits(bits));
        v.push(-f64::from_bits(bits | 1));
    }
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    while v.len() < 1000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let x = f64::from_bits(s);
        if x.is_finite() {
            v.push(x);
        }
    }
    assert_eq!(v.len(), 1000);
    let subnormals = v.iter().filter(|x| **x != 0.0 && x.abs() < f64::MIN_POSITIVE).count();
    assert!(subnormals >= 200, "{subnormals} subnormals");
    for x in &v {
        let t = f64_text(*x);
        let back: f64 = t.parse().unwrap();
        assert_eq!(back.to_bits(), x.to_bits(), "{x:e} -> {t}");
    }
    // Through the document too: every value as a point coordinate.
    let mut text = String::from("vug-doc version=1 lin=1e-6 ang=1e-9\nsketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]\n");
    for (i, x) in v.iter().enumerate() {
        text.push_str(&format!("point p{i} sketch=s x={} y=0\n", f64_text(*x)));
    }
    let d = read(&text).unwrap();
    for (i, x) in v.iter().enumerate() {
        let Stmt::Point { at, .. } = &d.stmts[i + 1] else { panic!() };
        assert_eq!(at[0].to_bits(), x.to_bits());
    }
    assert_eq!(write(&d), text);
    // The forms: shortest digits, plain in [1e-5, 1e16), exponent outside.
    assert_eq!(f64_text(0.1), "0.1");
    assert_eq!(f64_text(1.0), "1");
    assert_eq!(f64_text(-0.0), "-0");
    assert_eq!(f64_text(123456.0), "123456");
    assert_eq!(f64_text(1e16), "1e16");
    assert_eq!(f64_text(1e-300), "1e-300");
    assert_eq!(f64_text(5e-324), "5e-324");
    assert_eq!(f64_text(f64::NAN), "nan");
}

fn refused(text: &str) -> (u32, u32, What) {
    let r = read(text).expect_err("refused");
    (r.line, r.col, r.what)
}

#[test]
fn every_read_refusal_names_line_and_column() {
    let h = "vug-doc version=1 lin=1e-6 ang=1e-9\n";
    let sk = "sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n";
    assert_eq!(refused(&format!("{h}sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0] zz=1\n")), (2, 41, What::UnknownKey("zz".into())));
    assert_eq!(refused(&format!("{h}scan sc path=\"a\\qb\" pose=identity confidence=1\n")), (2, 16, What::BadEscape));
    assert_eq!(
        refused(&format!("{h}{sk}line l1 sketch=s1 a=p1 b=p2\npoint p1 sketch=s1 x=0 y=0\n")),
        (3, 21, What::UnknownRef("p1".into()))
    );
    assert_eq!(refused(&format!("{h}{sk}point p1 sketch=s1 x=nan y=0\n")), (3, 22, What::NonFinite));
    assert_eq!(refused(&format!("{h}{sk}point p1 sketch=s1 x=1.2.3 y=0\n")), (3, 22, What::BadNumber));
    assert_eq!(refused(&format!("{h}{sk}point p1 sketch=s1 x=1 x=2 y=0\n")), (3, 24, What::DuplicateKey("x".into())));
    assert_eq!(refused(&format!("{h}{sk}point p1 sketch=s1 x=1\n")), (3, 1, What::MissingKey("y")));
    assert_eq!(refused(&format!("{h}{sk}point s1 sketch=s1 x=1 y=1\n")), (3, 7, What::DuplicateId("s1".into())));
    assert_eq!(refused(&format!("{h}{sk}point p1 sketch=p0 x=1 y=1\n")), (3, 17, What::UnknownRef("p0".into())));
    assert_eq!(refused(&format!("{h}{sk}frob p1 a=1\n")), (3, 1, What::UnknownVerb("frob".into())));
    assert_eq!(refused(&format!("{h}scan sc path=\"open pose=identity confidence=1\n")), (2, 14, What::Unterminated));
    assert_eq!(refused(&format!("{h}axis axis=x min=0 max=1 max_feed=1 max_accel=1 rapid=1\n")), (2, 1, What::Orphan));
    assert_eq!(refused("sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n"), (1, 1, What::NoHeader));
    assert_eq!(refused("# only a comment\n"), (1, 1, What::NoHeader));
    assert_eq!(refused("vug-doc version=2 lin=1e-6 ang=1e-9\n"), (1, 17, What::Version(2)));
    let two = format!("{h}{sk}sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]\npoint a sketch=s1 x=0 y=0\nhorizontal hz sketch=s2 line=a\n");
    assert_eq!(refused(&two), (5, 30, What::CrossSketch("a".into())));
    assert_eq!(
        refused(&format!("{h}{sk}extrude e sketch=s1 outer=s1 dir=[0 0 1] length=1\n")),
        (3, 27, What::WrongTarget("s1".into()))
    );
}
