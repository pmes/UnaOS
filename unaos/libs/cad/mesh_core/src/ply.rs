//! PLY, the Stanford polygon format (Greg Turk's PLY definition): the header `ply`, `format ascii | binary_little_endian
//! | binary_big_endian 1.0`, `comment` / `obj_info` lines, `element <name> <count>`, `property <type> <name>`,
//! `property list <count-type> <item-type> <name>`, `end_header`; the scalar types char/int8, uchar/uint8, short/int16,
//! ushort/uint16, int/int32, uint/uint32, float/float32, double/float64. The `vertex` element's x, y, z (any scalar type)
//! are required; the `face` element's `vertex_indices` (or `vertex_index`) list is fan-triangulated; every other element
//! and property is read for its size and skipped. All three formats are read; [`write`] writes binary_little_endian with
//! double x / y / z (value-exact) and `list uchar uint vertex_indices`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::bytes::{Tokens, take};
use crate::{Mesh, Vec3};

/// Why [`read`] refuses a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlyError {
    /// The first line is not `ply`.
    NotPly,
    /// A header line that does not parse (or no `end_header`).
    Header { line: u32 },
    /// A `format` this reader does not know.
    Format(String),
    /// A property type this reader does not know.
    Type(String),
    /// No `vertex` element with x, y and z.
    NoVertexXyz,
    /// A `face` element without a `vertex_indices` / `vertex_index` list.
    NoFaceList,
    /// The body ended inside `element` record `index`.
    Truncated { element: String, index: u64 },
    /// ASCII body: a token that is not a number.
    Number { line: u32 },
    /// A face naming a vertex that does not exist (or a negative / non-integer index).
    IndexOutOfRange { face: u64, index: i64 },
    /// A face with fewer than three corners.
    ShortFace { face: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Result<Ty, PlyError> {
        Ok(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return Err(PlyError::Type(String::from(s))),
        })
    }
    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }
}

#[derive(Clone, Debug)]
struct Prop {
    name: String,
    ty: Ty,
    /// `Some(count type)` for a list property (then `ty` is the item type).
    list: Option<Ty>,
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    count: u64,
    props: Vec<Prop>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fmt {
    Ascii,
    Le,
    Be,
}

/// Read a PLY file into a mesh.
pub fn read(b: &[u8]) -> Result<Mesh, PlyError> {
    let (fmt, elements, body, body_line) = header(b)?;
    let mut positions: Vec<Vec3> = Vec::new();
    let mut faces: Vec<Vec<f64>> = Vec::new();
    let mut src = Body { fmt, b: body, pos: 0, tk: None };
    if fmt == Fmt::Ascii {
        let s = core::str::from_utf8(body).map_err(|_| PlyError::Number { line: body_line })?;
        src.tk = Some(Tokens::new(s, body_line));
    }
    let mut saw_vertex = false;
    for el in &elements {
        let is_vertex = el.name == "vertex";
        let is_face = el.name == "face";
        let (mut xi, mut yi, mut zi) = (None, None, None);
        let mut fi = None;
        for (k, p) in el.props.iter().enumerate() {
            if p.list.is_none() {
                match p.name.as_str() {
                    "x" => xi = Some(k),
                    "y" => yi = Some(k),
                    "z" => zi = Some(k),
                    _ => {}
                }
            } else if p.name == "vertex_indices" || p.name == "vertex_index" {
                fi = Some(k);
            }
        }
        let xyz = match (is_vertex, xi, yi, zi) {
            (true, Some(x), Some(y), Some(z)) => Some((x, y, z)),
            (true, ..) => return Err(PlyError::NoVertexXyz),
            _ => None,
        };
        if is_vertex {
            saw_vertex = true;
        }
        if is_face && fi.is_none() {
            return Err(PlyError::NoFaceList);
        }
        for r in 0..el.count {
            let mut p = Vec3::ZERO;
            for (k, prop) in el.props.iter().enumerate() {
                let trunc = || PlyError::Truncated { element: el.name.clone(), index: r };
                match prop.list {
                    None => {
                        let v = src.scalar(prop.ty)?.ok_or_else(trunc)?;
                        if let Some((x, y, z)) = xyz {
                            if k == x {
                                p.x = v;
                            } else if k == y {
                                p.y = v;
                            } else if k == z {
                                p.z = v;
                            }
                        }
                    }
                    Some(ct) => {
                        let n = src.scalar(ct)?.ok_or_else(trunc)?;
                        if !(n >= 0.0 && n <= u32::MAX as f64) {
                            return Err(trunc());
                        }
                        let mut items = Vec::new();
                        for _ in 0..n as u64 {
                            items.push(src.scalar(prop.ty)?.ok_or_else(trunc)?);
                        }
                        if is_face && Some(k) == fi {
                            faces.push(items);
                        }
                    }
                }
            }
            if xyz.is_some() {
                positions.push(p);
            }
        }
    }
    if !saw_vertex {
        return Err(PlyError::NoVertexXyz);
    }
    let nv = positions.len() as f64;
    let mut triangles = Vec::new();
    for (f, items) in faces.iter().enumerate() {
        let f = f as u64;
        if items.len() < 3 {
            return Err(PlyError::ShortFace { face: f });
        }
        let mut ix = Vec::with_capacity(items.len());
        for &v in items {
            if !(v >= 0.0 && v < nv && v == libm::floor(v)) {
                return Err(PlyError::IndexOutOfRange { face: f, index: v as i64 });
            }
            ix.push(v as u32);
        }
        for k in 1..ix.len() - 1 {
            triangles.push([ix[0], ix[k], ix[k + 1]]);
        }
    }
    Ok(Mesh { positions, triangles })
}

type Header<'a> = (Fmt, Vec<Element>, &'a [u8], u32);

fn header(b: &[u8]) -> Result<Header<'_>, PlyError> {
    let mut pos = 0usize;
    let mut line = 0u32;
    let mut fmt = None;
    let mut elements: Vec<Element> = Vec::new();
    loop {
        let rest = b.get(pos..).unwrap_or(&[]);
        let Some(nl) = rest.iter().position(|&c| c == b'\n') else {
            return Err(if line == 0 { PlyError::NotPly } else { PlyError::Header { line: line + 1 } });
        };
        line += 1;
        let raw = &rest[..nl];
        pos += nl + 1;
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let text = core::str::from_utf8(raw).map_err(|_| PlyError::Header { line })?;
        if line == 1 {
            if text.trim() != "ply" {
                return Err(PlyError::NotPly);
            }
            continue;
        }
        let mut w = text.split_ascii_whitespace();
        let bad = PlyError::Header { line };
        match w.next() {
            None | Some("comment") | Some("obj_info") => {}
            Some("format") => {
                let f = w.next().ok_or(bad.clone())?;
                let ver = w.next().ok_or(bad.clone())?;
                if ver != "1.0" {
                    return Err(PlyError::Format(String::from(text.trim())));
                }
                fmt = Some(match f {
                    "ascii" => Fmt::Ascii,
                    "binary_little_endian" => Fmt::Le,
                    "binary_big_endian" => Fmt::Be,
                    _ => return Err(PlyError::Format(String::from(f))),
                });
            }
            Some("element") => {
                let name = w.next().ok_or(bad.clone())?;
                let count: u64 = w.next().and_then(|c| c.parse().ok()).ok_or(bad.clone())?;
                elements.push(Element { name: String::from(name), count, props: Vec::new() });
            }
            Some("property") => {
                let el = elements.last_mut().ok_or(bad.clone())?;
                let t = w.next().ok_or(bad.clone())?;
                let prop = if t == "list" {
                    let ct = Ty::parse(w.next().ok_or(bad.clone())?)?;
                    let it = Ty::parse(w.next().ok_or(bad.clone())?)?;
                    let name = w.next().ok_or(bad.clone())?;
                    Prop { name: String::from(name), ty: it, list: Some(ct) }
                } else {
                    let ty = Ty::parse(t)?;
                    let name = w.next().ok_or(bad.clone())?;
                    Prop { name: String::from(name), ty, list: None }
                };
                el.props.push(prop);
            }
            Some("end_header") => {
                let fmt = fmt.ok_or(bad)?;
                return Ok((fmt, elements, b.get(pos..).unwrap_or(&[]), line + 1));
            }
            Some(_) => return Err(bad),
        }
    }
}

struct Body<'a> {
    fmt: Fmt,
    b: &'a [u8],
    pos: usize,
    tk: Option<Tokens<'a>>,
}

impl Body<'_> {
    /// The next scalar of type `ty` as f64 (`Ok(None)` at the end of the body).
    fn scalar(&mut self, ty: Ty) -> Result<Option<f64>, PlyError> {
        if let Some(tk) = self.tk.as_mut() {
            let Some((w, line)) = tk.next() else { return Ok(None) };
            return w.parse::<f64>().map(Some).map_err(|_| PlyError::Number { line });
        }
        let big = self.fmt == Fmt::Be;
        let (b, i) = (self.b, self.pos);
        let v = match ty {
            Ty::I8 => take::<1>(b, i, big).map(|x| i8::from_le_bytes(x) as f64),
            Ty::U8 => take::<1>(b, i, big).map(|x| x[0] as f64),
            Ty::I16 => take::<2>(b, i, big).map(|x| i16::from_le_bytes(x) as f64),
            Ty::U16 => take::<2>(b, i, big).map(|x| u16::from_le_bytes(x) as f64),
            Ty::I32 => take::<4>(b, i, big).map(|x| i32::from_le_bytes(x) as f64),
            Ty::U32 => take::<4>(b, i, big).map(|x| u32::from_le_bytes(x) as f64),
            Ty::F32 => take::<4>(b, i, big).map(|x| f32::from_le_bytes(x) as f64),
            Ty::F64 => take::<8>(b, i, big).map(f64::from_le_bytes),
        };
        if v.is_some() {
            self.pos += ty.size();
        }
        Ok(v)
    }
}

/// Write binary_little_endian: `element vertex` with double x / y / z, `element face` with `list uchar uint
/// vertex_indices` (every face a triangle).
pub fn write(mesh: &Mesh) -> Vec<u8> {
    let mut o = Vec::new();
    let head = alloc::format!(
        "ply\nformat binary_little_endian 1.0\ncomment mesh_core (VUGMESH SR466)\nelement vertex {}\nproperty double x\nproperty double y\nproperty double z\nelement face {}\nproperty list uchar uint vertex_indices\nend_header\n",
        mesh.positions.len(),
        mesh.triangles.len()
    );
    o.extend_from_slice(head.as_bytes());
    for p in &mesh.positions {
        for x in [p.x, p.y, p.z] {
            o.extend_from_slice(&x.to_le_bytes());
        }
    }
    for t in &mesh.triangles {
        o.push(3);
        for &i in t {
            o.extend_from_slice(&i.to_le_bytes());
        }
    }
    o
}
