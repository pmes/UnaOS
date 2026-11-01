// VUGMESH (SR466): STL (binary + ASCII), OBJ and PLY read / write, round trips and named refusals. Every fixture is
// built here; nothing binary is committed.

mod common;

use common::cube;
use mesh_core::obj::{self, Group, ObjError, NONE};
use mesh_core::ply::{self, PlyError};
use mesh_core::stl::{self, Facet, Stl, StlError};
use mesh_core::{HalfEdge, Mesh, Tol, Vec3};

const T: Tol = Tol::DEFAULT;

fn odd_stl() -> Stl {
    let mut header = [0u8; 80];
    let tag = b"solid but binary: a SolidWorks-style header that starts with solid";
    header[..tag.len()].copy_from_slice(tag);
    let facets = (0..7u16)
        .map(|k| {
            let f = k as f32;
            Facet {
                normal: [0.1 * f, -0.2, 1.0 / (f + 3.0)],
                v: [[f, 1e-7 * f, -3.25], [1e30, -f, 0.333_333_34], [f * 0.5, f32::MIN_POSITIVE, 2.0]],
                attr: 0x8000 | k * 31, // a colour word some exporters put here
            }
        })
        .collect();
    Stl { header, name: String::new(), facets }
}

#[test]
fn stl_binary_round_trip_is_byte_exact() {
    let s = odd_stl();
    let bytes = stl::write_binary(&s);
    assert_eq!(bytes.len(), 84 + 50 * 7);
    let back = stl::read(&bytes).unwrap();
    assert_eq!(back, s, "a binary whose header starts with \"solid\" is still binary (length first)");
    assert_eq!(stl::write_binary(&back), bytes);
}

#[test]
fn stl_ascii_round_trip() {
    let s = Stl::from_mesh(&cube(), T);
    assert_eq!(s.facets.len(), 12);
    let text = stl::write_ascii(&s);
    assert!(text.starts_with("solid mesh_core\n") && text.ends_with("endsolid mesh_core\n"));
    let back = stl::read(text.as_bytes()).unwrap();
    assert_eq!(back, s);
    // odd values survive the text form too (f32 shortest round-trip printing)
    let mut o = odd_stl();
    o.header = [0; 80];
    o.name = String::from("odd");
    for f in &mut o.facets {
        f.attr = 0;
    }
    assert_eq!(stl::read(stl::write_ascii(&o).as_bytes()).unwrap(), o);
}

#[test]
fn stl_soup_welds_to_the_cube() {
    let s = stl::read(&stl::write_binary(&Stl::from_mesh(&cube(), T))).unwrap();
    let soup = s.to_mesh();
    assert_eq!(soup.positions.len(), 36);
    let w = soup.weld(T);
    assert_eq!(w.mesh.positions.len(), 8);
    assert!(HalfEdge::build(&w.mesh).unwrap().is_closed());
    assert!((w.mesh.volume(T).unwrap() - 1.0).abs() < 1e-12);
}

#[test]
fn stl_refusals_are_named() {
    let mut bytes = stl::write_binary(&Stl::from_mesh(&cube(), T));
    bytes.pop();
    assert_eq!(stl::read(&bytes), Err(StlError::CountMismatch { count: 12, len: 84 + 50 * 12 - 1 }));
    assert_eq!(stl::read(b"abc"), Err(StlError::TooShort { len: 3 }));
    let bad = "solid x\nfacet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 0 0\n endloop\nendfacet\nendsolid x\n";
    assert_eq!(stl::read(bad.as_bytes()), Err(StlError::Ascii { line: 6, expected: "vertex" }));
    let num = "solid x\nfacet normal 0 zero 1\n";
    assert_eq!(stl::read(num.as_bytes()), Err(StlError::Number { line: 2 }));
    let cut = "solid x\nfacet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 endloop endfacet\n";
    assert_eq!(stl::read(cut.as_bytes()), Err(StlError::Ascii { line: 3, expected: "endsolid" }));
}

const OBJ_TEXT: &str = "# a quad, groups, vt/vn, negative indices, a continuation\n\
mtllib parts.mtl\n\
v 0 0 0\n\
v 0.1 0.2 0.30000000000000004\n\
v 1e-20 -3.3333333333333335 7 1.0\n\
v 4 \\\n  5 6\n\
vt 0.5\n\
vt 0.25 0.75\n\
vn 0 0 1\n\
o first part\n\
usemtl steel\n\
f 1/1/1 2/2/1 3/1/1 4//1\n\
g second\n\
s off\n\
f -4 -3 -1\n";

#[test]
fn obj_reads_the_statements() {
    let o = obj::read(OBJ_TEXT).unwrap();
    assert_eq!(o.mesh.positions.len(), 4);
    assert_eq!(o.mesh.positions[1], Vec3::new(0.1, 0.2, 0.30000000000000004));
    assert_eq!(o.mesh.positions[3], Vec3::new(4.0, 5.0, 6.0));
    assert_eq!(o.uvs, vec![[0.5, 0.0, 0.0], [0.25, 0.75, 0.0]]);
    assert_eq!(o.normals, vec![Vec3::new(0.0, 0.0, 1.0)]);
    // the quad fans into (1,2,3) (1,3,4); the negative face is (1,2,4)
    assert_eq!(o.mesh.triangles, vec![[0, 1, 2], [0, 2, 3], [0, 1, 3]]);
    assert_eq!(o.corners[1], [[0, 0], [0, 0], [NONE, 0]]);
    assert_eq!(o.corners[2], [[NONE, NONE]; 3]);
    assert_eq!(
        o.groups,
        vec![
            Group { name: String::from("first part"), first: 0, count: 2 },
            Group { name: String::from("second"), first: 2, count: 1 }
        ]
    );
    assert_eq!(o.skipped, 3); // mtllib, usemtl, s
}

#[test]
fn obj_round_trip_is_value_exact() {
    let o = obj::read(OBJ_TEXT).unwrap();
    let text = obj::write(&o);
    let mut back = obj::read(&text).unwrap();
    assert_eq!(back.skipped, 0);
    back.skipped = o.skipped;
    assert_eq!(back, o);
    assert_eq!(obj::write(&back), text);
}

#[test]
fn obj_refusals_are_named() {
    let base = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
    assert_eq!(obj::read(&format!("{base}f 0 1 2\n")), Err(ObjError::ZeroIndex { line: 4 }));
    assert_eq!(obj::read(&format!("{base}f 1 2 9\n")), Err(ObjError::IndexOutOfRange { line: 4, index: 9 }));
    assert_eq!(obj::read(&format!("{base}f 1 2 -4\n")), Err(ObjError::IndexOutOfRange { line: 4, index: -4 }));
    assert_eq!(obj::read(&format!("{base}f 1 2\n")), Err(ObjError::ShortFace { line: 4, corners: 2 }));
    assert_eq!(obj::read("v 0 zero 0\n"), Err(ObjError::Number { line: 1 }));
    assert_eq!(obj::read_bytes(&[b'v', b' ', 0xff]), Err(ObjError::Utf8));
}

fn odd_mesh() -> Mesh {
    let mut m = cube();
    for (k, p) in m.positions.iter_mut().enumerate() {
        *p = *p * 0.1 + Vec3::new(1e-17 * k as f64, -1.0 / 3.0, 1e300);
    }
    m
}

#[test]
fn ply_binary_round_trip() {
    let m = odd_mesh();
    let bytes = ply::write(&m);
    assert!(bytes.starts_with(b"ply\nformat binary_little_endian 1.0\n"));
    let back = ply::read(&bytes).unwrap();
    assert_eq!(back, m);
    assert_eq!(ply::write(&back), bytes);
}

#[test]
fn ply_ascii_with_a_quad_and_skipped_data() {
    let text = "ply\r\nformat ascii 1.0\r\ncomment from a scanner\r\nobj_info nothing\r\n\
element vertex 4\r\nproperty float x\r\nproperty float y\r\nproperty uchar red\r\nproperty float z\r\n\
element face 1\r\nproperty list uchar int vertex_index\r\nproperty uchar flags\r\n\
element edge 1\r\nproperty int vertex1\r\nproperty int vertex2\r\nend_header\r\n\
0 0 255 0\r\n1 0 128 0\r\n1 1 0 0.5\r\n0 1 7 0\r\n4 0 1 2 3 9\r\n0 1\r\n";
    let m = ply::read(text.as_bytes()).unwrap();
    assert_eq!(m.positions.len(), 4);
    assert_eq!(m.positions[2], Vec3::new(1.0, 1.0, 0.5));
    assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]]);
}

#[test]
fn ply_big_endian_read() {
    let mut b = b"ply\nformat binary_big_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n".to_vec();
    for v in [[0f32, 0.0, 0.0], [2.5, 0.0, 0.0], [0.0, -1.25, 3.0]] {
        for x in v {
            b.extend_from_slice(&x.to_be_bytes());
        }
    }
    b.push(3);
    for i in [0i32, 1, 2] {
        b.extend_from_slice(&i.to_be_bytes());
    }
    let m = ply::read(&b).unwrap();
    assert_eq!(m.positions[2], Vec3::new(0.0, -1.25, 3.0));
    assert_eq!(m.triangles, vec![[0, 1, 2]]);
}

#[test]
fn ply_refusals_are_named() {
    assert_eq!(ply::read(b"plx\n"), Err(PlyError::NotPly));
    assert_eq!(ply::read(b"ply\nformat binary_middle_endian 1.0\nend_header\n"), Err(PlyError::Format(String::from("binary_middle_endian"))));
    assert_eq!(ply::read(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty quad x\nend_header\n"), Err(PlyError::Type(String::from("quad"))));
    assert_eq!(ply::read(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n0\n"), Err(PlyError::NoVertexXyz));
    let head = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\n";
    assert_eq!(ply::read(format!("{head}property int n\nend_header\n0 0 0 1 0 0 0 1 0 5\n").as_bytes()), Err(PlyError::NoFaceList));
    let lst = format!("{head}property list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n");
    assert_eq!(ply::read(format!("{lst}3 0 1 7\n").as_bytes()), Err(PlyError::IndexOutOfRange { face: 0, index: 7 }));
    assert_eq!(ply::read(format!("{lst}2 0 1\n").as_bytes()), Err(PlyError::ShortFace { face: 0 }));
    assert_eq!(ply::read(format!("{lst}3 0 1\n").as_bytes()), Err(PlyError::Truncated { element: String::from("face"), index: 0 }));
    assert_eq!(ply::read(format!("{lst}3 0 one 2\n").as_bytes()), Err(PlyError::Number { line: 13 }));
    let mut bin = ply::write(&cube());
    bin.truncate(bin.len() - 2);
    assert_eq!(ply::read(&bin), Err(PlyError::Truncated { element: String::from("face"), index: 11 }));
}
