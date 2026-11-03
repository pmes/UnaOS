// VUG3MF (SR486): 3MF read / write over zip_core — a hand-written package (the unit cube in the shape of the 3MF Core
// Specification's example model) reads, the fixture meshes round-trip bit-exactly, components compose through their
// transforms, stored and deflated packages both read, and every refusal is reached by name. Every package is built
// here with zip_core's Writer; nothing binary is committed.

mod common;

use common::{cube, sphere};
use mesh_core::threemf::{self, ObjectType, ThreeMfError};
use mesh_core::{Mat4, Mesh, Tol, Vec3};
use zip_core::{Method, Refusal, Writer};

const T: Tol = Tol::DEFAULT;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#;

fn rels(target: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="{target}" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#
    )
}

/// The cube's mesh element, 8 vertices and 12 outward triangles (common::cube's numbering).
const CUBE_MESH: &str = r#"<mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
          <vertex x="1" y="1" z="0"/>
          <vertex x="0" y="0" z="1"/>
          <vertex x="1" y="0" z="1"/>
          <vertex x="0" y="1" z="1"/>
          <vertex x="1" y="1" z="1"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="3"/>
          <triangle v1="0" v2="3" v3="1"/>
          <triangle v1="4" v2="5" v3="7"/>
          <triangle v1="4" v2="7" v3="6"/>
          <triangle v1="0" v2="1" v3="5"/>
          <triangle v1="0" v2="5" v3="4"/>
          <triangle v1="2" v2="6" v3="7"/>
          <triangle v1="2" v2="7" v3="3"/>
          <triangle v1="0" v2="4" v3="6"/>
          <triangle v1="0" v2="6" v3="2"/>
          <triangle v1="1" v2="3" v3="7"/>
          <triangle v1="1" v2="7" v3="5"/>
        </triangles>
      </mesh>"#;

/// A model part: `attrs` on `<model>`, `resources` and `build` inner XML.
fn model(attrs: &str, resources: &str, build: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model {attrs} xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    {resources}
  </resources>
  <build>
    {build}
  </build>
</model>"#
    )
}

fn cube_model(attrs: &str) -> String {
    model(attrs, &format!(r#"<object id="1" type="model">{CUBE_MESH}</object>"#), r#"<item objectid="1"/>"#)
}

/// An OPC package: content types, `_rels/.rels` naming `/3D/3dmodel.model`, and the model part.
fn package(model: &str, method: Method) -> Vec<u8> {
    package_at("3D/3dmodel.model", model, method)
}

fn package_at(name: &str, model: &str, method: Method) -> Vec<u8> {
    let mut w = Writer::new();
    w.add("[Content_Types].xml", CONTENT_TYPES.as_bytes(), Method::Stored).unwrap();
    w.add("_rels/.rels", rels(&format!("/{name}")).as_bytes(), Method::Stored).unwrap();
    w.add(name, model.as_bytes(), method).unwrap();
    w.finish()
}

fn same_bits(a: &Mesh, b: &Mesh) -> bool {
    a.triangles == b.triangles
        && a.positions.len() == b.positions.len()
        && a.positions.iter().zip(&b.positions).all(|(p, q)| {
            p.x.to_bits() == q.x.to_bits() && p.y.to_bits() == q.y.to_bits() && p.z.to_bits() == q.z.to_bits()
        })
}

fn mat_bits(a: &Mat4, b: &Mat4) -> bool {
    a.cols.iter().flatten().zip(b.cols.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn hand_written_cube_reads() {
    let m = threemf::read(&package(&cube_model(r#"unit="millimeter""#), Method::Deflated)).unwrap();
    assert_eq!(m.parts.len(), 1);
    let p = &m.parts[0];
    assert_eq!(p.object, 1);
    assert_eq!(p.transform, Mat4::IDENTITY);
    assert_eq!(p.mesh.positions.len(), 8);
    assert_eq!(p.mesh.triangles.len(), 12);
    assert_eq!(p.mesh, cube());
    assert!((p.mesh.volume(T).unwrap() - 1.0).abs() < 1e-12);
    assert!(m.ignored.is_empty());
}

#[test]
fn stored_and_deflated_packages_read_alike() {
    let xml = cube_model("");
    let a = threemf::read(&package(&xml, Method::Stored)).unwrap();
    let b = threemf::read(&package(&xml, Method::Deflated)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.parts[0].mesh, cube());
}

#[test]
fn rels_name_the_model_part() {
    let m = threemf::read(&package_at("3D/Other.model", &cube_model(""), Method::Deflated)).unwrap();
    assert_eq!(m.parts[0].mesh, cube());
}

#[test]
fn write_read_is_bit_exact() {
    let c = cube();
    let s = sphere(12.345, 9, 17);
    let mut place = Mat4::from_cols([0.6, 0.8, 0.0, 0.0], [-0.8, 0.6, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [1.0 / 3.0, -7.25, 0.1, 1.0]);
    place.cols[2][2] = 2.0_f64.sqrt();
    let bytes = threemf::write(&[(&c, Mat4::IDENTITY), (&s, place)]).unwrap();
    assert_eq!(bytes, threemf::write(&[(&c, Mat4::IDENTITY), (&s, place)]).unwrap(), "byte-deterministic");
    let m = threemf::read(&bytes).unwrap();
    assert_eq!(m.parts.len(), 2);
    assert!(same_bits(&m.parts[0].mesh, &c));
    assert!(mat_bits(&m.parts[0].transform, &Mat4::IDENTITY));
    assert!(same_bits(&m.parts[1].mesh, &s));
    assert!(mat_bits(&m.parts[1].transform, &place));
    assert_eq!(mesh_core::read_3mf(&mesh_core::write_3mf(&[(&s, place)]).unwrap()).unwrap().parts[0].mesh, s);
}

#[test]
fn components_compose_through_transforms() {
    // Object 2 places object 1 at +10 x; its item turns it 90 degrees about z and lifts it 5 in z.
    let xml = model(
        "",
        &format!(
            r#"<object id="1" type="model">{CUBE_MESH}</object>
    <object id="2" type="model"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 10 0 0"/></components></object>"#
        ),
        r#"<item objectid="1"/>
    <item objectid="2" transform="0 1 0 -1 0 0 0 0 1 0 0 5"/>"#,
    );
    let m = threemf::read(&package(&xml, Method::Deflated)).unwrap();
    assert_eq!(m.parts.len(), 2);
    assert_eq!((m.parts[0].object, m.parts[0].transform), (1, Mat4::IDENTITY));
    let p = &m.parts[1];
    assert_eq!(p.object, 1);
    let item = Mat4::from_cols([0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 5.0, 1.0]);
    let comp = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
    assert_eq!(p.transform, item * comp);
    // (1, 0, 0) -> (11, 0, 0) -> turned (0, 11, 0) -> lifted (0, 11, 5).
    assert_eq!(p.transform.transform_point3(Vec3::new(1.0, 0.0, 0.0)), Vec3::new(0.0, 11.0, 5.0));
    assert_eq!(p.transform.transform_point3(Vec3::new(0.0, 1.0, 1.0)), Vec3::new(-1.0, 10.0, 6.0));
}

#[test]
fn unit_scales_to_millimetres() {
    let xml = model(
        r#"unit="inch""#,
        &format!(r#"<object id="1" type="model">{CUBE_MESH}</object>"#),
        r#"<item objectid="1" transform="1 0 0 0 1 0 0 0 1 2 0 0"/>"#,
    );
    let m = threemf::read(&package(&xml, Method::Deflated)).unwrap();
    let p = &m.parts[0];
    assert_eq!(p.mesh.positions[7], Vec3::new(25.4, 25.4, 25.4));
    assert_eq!(p.transform.cols[3], [50.8, 0.0, 0.0, 1.0]);
    assert_eq!(
        threemf::read(&package(&cube_model(r#"unit="parsec""#), Method::Stored)),
        Err(ThreeMfError::Unit("parsec".into()))
    );
}

#[test]
fn support_objects_are_ignored_by_name() {
    let xml = model(
        "",
        &format!(
            r#"<object id="1" type="model">{CUBE_MESH}</object>
    <object id="3" type="support">{CUBE_MESH}</object>
    <object id="4" type="other">{CUBE_MESH}</object>"#
        ),
        r#"<item objectid="1"/><item objectid="3"/>"#,
    );
    let m = threemf::read(&package(&xml, Method::Deflated)).unwrap();
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.ignored, vec![(3, ObjectType::Support), (4, ObjectType::Other)]);
}

#[test]
fn refusals_by_name() {
    assert_eq!(threemf::read(b"solid cube\nendsolid cube\n"), Err(ThreeMfError::Zip(Refusal::NotZip)));

    let mut w = Writer::new();
    w.add("[Content_Types].xml", CONTENT_TYPES.as_bytes(), Method::Stored).unwrap();
    assert_eq!(threemf::read(&w.finish()), Err(ThreeMfError::NoModel));

    let bad_index = cube_model("").replace(r#"<triangle v1="1" v2="7" v3="5"/>"#, r#"<triangle v1="1" v2="7" v3="8"/>"#);
    assert_eq!(
        threemf::read(&package(&bad_index, Method::Deflated)),
        Err(ThreeMfError::IndexOutOfRange { object: 1, triangle: 11, index: 8 })
    );

    let repeated = cube_model("").replace(r#"<triangle v1="1" v2="7" v3="5"/>"#, r#"<triangle v1="1" v2="7" v3="7"/>"#);
    assert_eq!(
        threemf::read(&package(&repeated, Method::Deflated)),
        Err(ThreeMfError::RepeatedIndex { object: 1, triangle: 11 })
    );

    let inf = cube_model("").replace(r#"<vertex x="1" y="1" z="1"/>"#, r#"<vertex x="inf" y="1" z="1"/>"#);
    assert_eq!(threemf::read(&package(&inf, Method::Deflated)), Err(ThreeMfError::NonFinite { object: 1, vertex: 7 }));

    assert_eq!(
        threemf::read(&package(&cube_model(r#"requiredextensions="p""#), Method::Deflated)),
        Err(ThreeMfError::RequiredExtension("p".into()))
    );

    let missing = model("", &format!(r#"<object id="1" type="model">{CUBE_MESH}</object>"#), r#"<item objectid="9"/>"#);
    assert_eq!(threemf::read(&package(&missing, Method::Deflated)), Err(ThreeMfError::NoSuchObject { id: 9 }));

    let cycle = model(
        "",
        r#"<object id="2" type="model"><components><component objectid="3"/></components></object>
    <object id="3" type="model"><components><component objectid="2"/></components></object>"#,
        r#"<item objectid="2"/>"#,
    );
    assert_eq!(threemf::read(&package(&cycle, Method::Deflated)), Err(ThreeMfError::ComponentCycle { id: 2 }));

    let bad_transform = cube_model("").replace(r#"<item objectid="1"/>"#, r#"<item objectid="1" transform="1 0 0"/>"#);
    assert_eq!(threemf::read(&package(&bad_transform, Method::Deflated)), Err(ThreeMfError::BadTransform));

    // A model part compressed by a method zip_core lacks (12, bzip2): the central record names it.
    let mut z = package(&cube_model(""), Method::Deflated);
    let e = z.len() - 22;
    let mut c = u32::from_le_bytes(z[e + 16..e + 20].try_into().unwrap()) as usize;
    for _ in 0..2 {
        let len = |i: usize| u16::from_le_bytes([z[i], z[i + 1]]) as usize;
        c += 46 + len(c + 28) + len(c + 30) + len(c + 32);
    }
    z[c + 10..c + 12].copy_from_slice(&12u16.to_le_bytes());
    assert_eq!(
        threemf::read(&z),
        Err(ThreeMfError::Zip(Refusal::Method { name: "3D/3dmodel.model".into(), method: 12 }))
    );
}

#[test]
fn write_refuses_what_read_would() {
    let mut c = cube();
    let mut skew = Mat4::IDENTITY;
    skew.cols[0][3] = 0.5;
    assert_eq!(threemf::write(&[(&c, skew)]), Err(ThreeMfError::NotAffine { part: 0 }));
    c.positions[3].y = f64::NAN;
    assert_eq!(threemf::write(&[(&c, Mat4::IDENTITY)]), Err(ThreeMfError::NonFinite { object: 1, vertex: 3 }));
    let mut d = cube();
    d.triangles[2] = [4, 5, 9];
    assert_eq!(threemf::write(&[(&d, Mat4::IDENTITY)]), Err(ThreeMfError::IndexOutOfRange { object: 1, triangle: 2, index: 9 }));
}
