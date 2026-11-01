//! Wavefront OBJ (the geometry statements of the Wavefront Advanced Visualizer's Appendix B1 "Object Files"):
//! `v x y z [w]`, `vt u [v [w]]`, `vn i j k`, `f` with corners `v`, `v/vt`, `v//vn` or `v/vt/vn`; indices are 1-based,
//! a negative index counts back from the elements defined so far, 0 is refused; `g` and `o` start a named group; `#`
//! starts a comment; a line ending in `\` continues on the next. Polygons are fan-triangulated (v0, vi, vi+1).
//! Every other statement (`usemtl`, `mtllib`, `s`, `l`, `p`, ...) is counted in `skipped`, never an error. Numbers are
//! written with f64's shortest round-trip form, so a write then read gives the same values back.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::{Mesh, Vec3};

/// "No index" for a corner without a texture coordinate or a normal.
pub const NONE: u32 = u32::MAX;

/// A named run of triangles `first .. first + count`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub first: u32,
    pub count: u32,
}

/// An OBJ file's geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obj {
    /// Positions (`v`) and the fan-triangulated faces.
    pub mesh: Mesh,
    /// `vn` in file order.
    pub normals: Vec<Vec3>,
    /// `vt` in file order (absent components are 0).
    pub uvs: Vec<[f64; 3]>,
    /// Per triangle, per corner: `[vt, vn]` indices (0-based) or [`NONE`].
    pub corners: Vec<[[u32; 2]; 3]>,
    pub groups: Vec<Group>,
    /// Statements read past (materials, smoothing, lines, points, unknown keywords).
    pub skipped: u32,
}

/// Why [`read`] refuses a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjError {
    /// A missing or unparsable number on `line`.
    Number { line: u32 },
    /// An index of 0 (OBJ is 1-based).
    ZeroIndex { line: u32 },
    /// An index naming an element not yet defined.
    IndexOutOfRange { line: u32, index: i64 },
    /// A face with fewer than three corners.
    ShortFace { line: u32, corners: u32 },
    /// The bytes are not UTF-8 ([`read_bytes`]).
    Utf8,
}

/// Read OBJ bytes (UTF-8).
pub fn read_bytes(b: &[u8]) -> Result<Obj, ObjError> {
    read(core::str::from_utf8(b).map_err(|_| ObjError::Utf8)?)
}

/// Read OBJ text.
pub fn read(s: &str) -> Result<Obj, ObjError> {
    let mut o = Obj::default();
    let mut pending = String::new();
    let mut pending_line = 0u32;
    for (i, raw) in s.lines().enumerate() {
        let ln = i as u32 + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if pending.is_empty() {
            pending_line = ln;
        }
        if let Some(head) = raw.strip_suffix('\\') {
            pending.push_str(head);
            pending.push(' ');
            continue;
        }
        pending.push_str(raw);
        statement(&mut o, &pending, pending_line)?;
        pending.clear();
    }
    if !pending.is_empty() {
        statement(&mut o, &pending, pending_line)?;
    }
    let total = o.mesh.triangles.len() as u32;
    for k in 0..o.groups.len() {
        let end = o.groups.get(k + 1).map_or(total, |g| g.first);
        o.groups[k].count = end - o.groups[k].first;
    }
    Ok(o)
}

fn statement(o: &mut Obj, line_text: &str, line: u32) -> Result<(), ObjError> {
    let text = match line_text.find('#') {
        Some(i) => &line_text[..i],
        None => line_text,
    };
    let mut w = text.split_ascii_whitespace();
    let Some(kw) = w.next() else { return Ok(()) };
    match kw {
        "v" => {
            let p = nums::<3>(&mut w, 3, line)?;
            o.mesh.positions.push(Vec3::new(p[0], p[1], p[2]));
        }
        "vn" => {
            let p = nums::<3>(&mut w, 3, line)?;
            o.normals.push(Vec3::new(p[0], p[1], p[2]));
        }
        "vt" => o.uvs.push(nums::<3>(&mut w, 1, line)?),
        "f" => {
            let mut c: Vec<[u32; 3]> = Vec::new();
            for tok in w {
                let mut parts = tok.split('/');
                let v = index(parts.next().unwrap_or(""), o.mesh.positions.len(), line)?;
                let vt = match parts.next() {
                    Some(t) if !t.is_empty() => index(t, o.uvs.len(), line)?,
                    _ => NONE,
                };
                let vn = match parts.next() {
                    Some(t) if !t.is_empty() => index(t, o.normals.len(), line)?,
                    _ => NONE,
                };
                c.push([v, vt, vn]);
            }
            if c.len() < 3 {
                return Err(ObjError::ShortFace { line, corners: c.len() as u32 });
            }
            for k in 1..c.len() - 1 {
                let (a, b, d) = (c[0], c[k], c[k + 1]);
                o.mesh.triangles.push([a[0], b[0], d[0]]);
                o.corners.push([[a[1], a[2]], [b[1], b[2]], [d[1], d[2]]]);
            }
        }
        "g" | "o" => {
            let mut name = String::new();
            for (k, part) in w.enumerate() {
                if k > 0 {
                    name.push(' ');
                }
                name.push_str(part);
            }
            o.groups.push(Group { name, first: o.mesh.triangles.len() as u32, count: 0 });
        }
        _ => o.skipped += 1,
    }
    Ok(())
}

/// `N` numbers, the first `need` required, the rest 0 when absent (a `w` beyond N is ignored).
fn nums<'a, const N: usize>(w: &mut impl Iterator<Item = &'a str>, need: usize, line: u32) -> Result<[f64; N], ObjError> {
    let mut o = [0f64; N];
    for (k, x) in o.iter_mut().enumerate() {
        match w.next() {
            Some(t) => *x = t.parse::<f64>().map_err(|_| ObjError::Number { line })?,
            None if k < need => return Err(ObjError::Number { line }),
            None => break,
        }
    }
    Ok(o)
}

/// A 1-based (or negative, relative) index against `count` elements defined so far, to 0-based.
fn index(t: &str, count: usize, line: u32) -> Result<u32, ObjError> {
    let i: i64 = t.parse().map_err(|_| ObjError::Number { line })?;
    let z = match i {
        0 => return Err(ObjError::ZeroIndex { line }),
        i if i > 0 => i - 1,
        i => count as i64 + i,
    };
    if z < 0 || z >= count as i64 || z >= NONE as i64 {
        return Err(ObjError::IndexOutOfRange { line, index: i });
    }
    Ok(z as u32)
}

/// Write the geometry: `v`, `vt` (three components), `vn`, then the faces as triangles, each group's run after its
/// `g` line (triangles before the first group first).
pub fn write(o: &Obj) -> String {
    let mut s = String::new();
    for p in &o.mesh.positions {
        let _ = writeln!(s, "v {} {} {}", p.x, p.y, p.z);
    }
    for t in &o.uvs {
        let _ = writeln!(s, "vt {} {} {}", t[0], t[1], t[2]);
    }
    for n in &o.normals {
        let _ = writeln!(s, "vn {} {} {}", n.x, n.y, n.z);
    }
    let total = o.mesh.triangles.len();
    let head = o.groups.first().map_or(total, |g| (g.first as usize).min(total));
    faces(&mut s, o, 0, head);
    for g in &o.groups {
        let _ = writeln!(s, "g {}", g.name);
        let a = (g.first as usize).min(total);
        let b = (g.first as usize).saturating_add(g.count as usize).min(total);
        faces(&mut s, o, a, b);
    }
    s
}

fn faces(s: &mut String, o: &Obj, a: usize, b: usize) {
    for t in a..b {
        let tri = o.mesh.triangles[t];
        let cn = o.corners.get(t).copied().unwrap_or([[NONE; 2]; 3]);
        s.push('f');
        for k in 0..3 {
            let _ = write!(s, " {}", tri[k] as u64 + 1);
            match (cn[k][0], cn[k][1]) {
                (NONE, NONE) => {}
                (vt, NONE) => {
                    let _ = write!(s, "/{}", vt as u64 + 1);
                }
                (NONE, vn) => {
                    let _ = write!(s, "//{}", vn as u64 + 1);
                }
                (vt, vn) => {
                    let _ = write!(s, "/{}/{}", vt as u64 + 1, vn as u64 + 1);
                }
            }
        }
        s.push('\n');
    }
}
