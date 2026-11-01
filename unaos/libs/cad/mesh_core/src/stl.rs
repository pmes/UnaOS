//! STL, binary and ASCII, from the format's definition (3D Systems' StereoLithography Interface Specification, 1988).
//! Binary: an 80-byte header, a little-endian u32 facet count, then 50-byte records (normal 3 x f32, three vertices
//! 3 x f32, a u16 "attribute byte count"). ASCII: `solid [name]` { `facet normal ni nj nk` `outer loop`
//! `vertex x y z` x 3 `endloop` `endfacet` } `endsolid [name]`. The file's `f32` is the on-disk format's, not a render
//! seam (BP-vug-1 names `Mesh::to_f32_buffers` as the only render conversion). [`Stl`] keeps the raw records (header,
//! stored normals, attribute words, facet order), so a binary read then write is byte-exact.
//!
//! A buffer is binary when its length is exactly `84 + 50 * count` (decided FIRST: binaries whose header starts with
//! "solid" are common), else ASCII when it starts with `solid`, else refused.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::bytes::{Tokens, le_f32, le_u16, le_u32};
use crate::{Mesh, Tol, Vec3};

/// One facet as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facet {
    pub normal: [f32; 3],
    pub v: [[f32; 3]; 3],
    /// The binary record's attribute byte count (0 from ASCII).
    pub attr: u16,
}

/// An STL file's contents.
#[derive(Clone, Debug, PartialEq)]
pub struct Stl {
    /// The binary header (zeros for an ASCII file).
    pub header: [u8; 80],
    /// The ASCII solid name (empty for a binary file).
    pub name: String,
    pub facets: Vec<Facet>,
}

/// Why [`read`] refuses a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StlError {
    /// Shorter than a binary header + count and not ASCII.
    TooShort { len: usize },
    /// A binary-sized header whose count does not match the length (and not ASCII).
    CountMismatch { count: u32, len: usize },
    /// ASCII: the keyword expected at `line` was not there (or the file ended).
    Ascii { line: u32, expected: &'static str },
    /// ASCII: a number that does not parse.
    Number { line: u32 },
    /// Starts with `solid` but is not UTF-8 text.
    NotStl,
}

/// Read a binary or ASCII STL.
pub fn read(b: &[u8]) -> Result<Stl, StlError> {
    if let Some(count) = le_u32(b, 80) {
        if 84u64 + 50 * count as u64 == b.len() as u64 {
            return Ok(read_binary(b, count));
        }
    }
    if b.len() >= 5 && b[..5].eq_ignore_ascii_case(b"solid") {
        let s = core::str::from_utf8(b).map_err(|_| StlError::NotStl)?;
        return read_ascii(s);
    }
    match le_u32(b, 80) {
        Some(count) => Err(StlError::CountMismatch { count, len: b.len() }),
        None => Err(StlError::TooShort { len: b.len() }),
    }
}

fn read_binary(b: &[u8], count: u32) -> Stl {
    let mut header = [0u8; 80];
    header.copy_from_slice(&b[..80]);
    let f3 = |i: usize| [le_f32(b, i).unwrap_or(0.0), le_f32(b, i + 4).unwrap_or(0.0), le_f32(b, i + 8).unwrap_or(0.0)];
    let facets = (0..count as usize)
        .map(|k| {
            let o = 84 + 50 * k;
            Facet { normal: f3(o), v: [f3(o + 12), f3(o + 24), f3(o + 36)], attr: le_u16(b, o + 48).unwrap_or(0) }
        })
        .collect();
    Stl { header, name: String::new(), facets }
}

fn read_ascii(s: &str) -> Result<Stl, StlError> {
    let (first, rest) = match s.find('\n') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    let name = String::from(first.trim()[5..].trim());
    let mut tk = Tokens::new(rest, 2);
    let mut facets = Vec::new();
    loop {
        let Some((w, line)) = tk.next() else {
            return Err(StlError::Ascii { line: tk.line(), expected: "endsolid" });
        };
        if w.eq_ignore_ascii_case("endsolid") {
            break;
        }
        if !w.eq_ignore_ascii_case("facet") {
            return Err(StlError::Ascii { line, expected: "facet or endsolid" });
        }
        word(&mut tk, "normal")?;
        let normal = three(&mut tk)?;
        word(&mut tk, "outer")?;
        word(&mut tk, "loop")?;
        let mut v = [[0f32; 3]; 3];
        for c in &mut v {
            word(&mut tk, "vertex")?;
            *c = three(&mut tk)?;
        }
        word(&mut tk, "endloop")?;
        word(&mut tk, "endfacet")?;
        facets.push(Facet { normal, v, attr: 0 });
    }
    Ok(Stl { header: [0u8; 80], name, facets })
}

fn word(tk: &mut Tokens, expected: &'static str) -> Result<(), StlError> {
    match tk.next() {
        Some((w, _)) if w.eq_ignore_ascii_case(expected) => Ok(()),
        Some((_, line)) => Err(StlError::Ascii { line, expected }),
        None => Err(StlError::Ascii { line: tk.line(), expected }),
    }
}

fn three(tk: &mut Tokens) -> Result<[f32; 3], StlError> {
    let mut o = [0f32; 3];
    for x in &mut o {
        let (w, line) = tk.next().ok_or(StlError::Number { line: tk.line() })?;
        *x = w.parse::<f32>().map_err(|_| StlError::Number { line })?;
    }
    Ok(o)
}

/// The binary form: header, count, the 50-byte records.
pub fn write_binary(stl: &Stl) -> Vec<u8> {
    let mut o = Vec::with_capacity(84 + 50 * stl.facets.len());
    o.extend_from_slice(&stl.header);
    o.extend_from_slice(&(stl.facets.len() as u32).to_le_bytes());
    for f in &stl.facets {
        for x in f.normal.iter().chain(f.v.iter().flatten()) {
            o.extend_from_slice(&x.to_le_bytes());
        }
        o.extend_from_slice(&f.attr.to_le_bytes());
    }
    o
}

/// The ASCII form (f32 printed shortest round-trip, so a read gives the same values back).
pub fn write_ascii(stl: &Stl) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "solid {}", stl.name);
    for f in &stl.facets {
        let _ = writeln!(s, "  facet normal {} {} {}", f.normal[0], f.normal[1], f.normal[2]);
        let _ = writeln!(s, "    outer loop");
        for v in &f.v {
            let _ = writeln!(s, "      vertex {} {} {}", v[0], v[1], v[2]);
        }
        let _ = writeln!(s, "    endloop");
        let _ = writeln!(s, "  endfacet");
    }
    let _ = writeln!(s, "endsolid {}", stl.name);
    s
}

impl Stl {
    /// The triangle soup: three vertices per facet in facet order (weld with [`Mesh::weld`]).
    pub fn to_mesh(&self) -> Mesh {
        let mut positions = Vec::with_capacity(self.facets.len() * 3);
        let mut triangles = Vec::with_capacity(self.facets.len());
        for (k, f) in self.facets.iter().enumerate() {
            for v in &f.v {
                positions.push(Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64));
            }
            let b = 3 * k as u32;
            triangles.push([b, b + 1, b + 2]);
        }
        Mesh { positions, triangles }
    }

    /// One facet per in-range triangle with its unit face normal (zero for a degenerate one), a zero header and the
    /// name `mesh_core`.
    pub fn from_mesh(mesh: &Mesh, tol: Tol) -> Stl {
        let normals = mesh.normals_face(tol);
        let f = |p: Vec3| [p.x as f32, p.y as f32, p.z as f32];
        let facets = (0..mesh.triangles.len())
            .filter_map(|t| {
                let [a, b, c] = mesh.corners(t)?;
                Some(Facet { normal: f(normals[t]), v: [f(a), f(b), f(c)], attr: 0 })
            })
            .collect();
        Stl { header: [0u8; 80], name: String::from("mesh_core"), facets }
    }
}
