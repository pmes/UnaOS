//! 3MF, the 3D Manufacturing Format (3MF Core Specification 1.3, the 3MF Consortium; the package is an OPC package,
//! ECMA-376 Part 2): a ZIP holding `[Content_Types].xml`, `_rels/.rels` and the model part (conventionally
//! `3D/3dmodel.model`): `<model unit= xml:lang=>` with `<resources>` of `<object id= type=>`, each a
//! `<mesh><vertices><vertex x= y= z=/>…</vertices><triangles><triangle v1= v2= v3=/>…</triangles></mesh>` or
//! `<components><component objectid= transform=/>…</components>`, and `<build>` of `<item objectid= transform=/>`.
//!
//! VUG3MF (LEDGER SR486). The container is zip_core's; the XML is svg_core's reader (`svg_core::xml`, the one
//! no_std XML reader in the tree). Its DOM is capped at 2^20 nodes, whitespace text included, so a model past roughly
//! half a million vertex and triangle elements refuses as [`ThreeMfError::Xml`] with `TooLarge` (VUG-3MF-XMLPULL owes
//! a streaming reader). [`read`] finds the model part through the `_rels/.rels` relationship of type
//! [`MODEL_REL`] (falling back to `3D/3dmodel.model`), reads the objects of type `model`, ignores `support`,
//! `solidsupport`, `surface` and `other` by name into [`ThreeMf::ignored`], composes components through their
//! transforms, and scales every length to millimetres by the model's `unit`. A `transform` is the spec's 12 numbers
//! `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32` acting on the row vector `[x y z 1]`; in geom_core's
//! column-vector [`Mat4`] they are the first three rows of columns 0..3, in order. Materials (`pid`, `p1`..`p3`) are
//! read past (VUG-3MF-MATERIALS). [`write`] writes millimetres with every `f64` in core::fmt's shortest round-trip
//! text, so write then read is bit-exact.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

use svg_core::xml::Document;
use zip_core::{Archive, Entry, Method, Refusal};

use crate::geom::Mat4;
use crate::{Mesh, Vec3};

/// The OPC relationship type of the 3D model part (3MF Core §2.1).
pub const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
/// The 3MF core namespace (3MF Core §3).
pub const NS_CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
/// The model part's conventional name, used when `_rels/.rels` names none.
const DEFAULT_MODEL: &str = "3D/3dmodel.model";

/// An object's `type` (3MF Core §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectType {
    Model,
    Support,
    SolidSupport,
    Surface,
    Other,
}

/// One mesh object reached from a build item: its mesh in millimetres (object space) and the composed transform
/// (item · component · …) that places it.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub object: u32,
    pub mesh: Mesh,
    pub transform: Mat4,
}

/// A read 3MF package.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreeMf {
    /// One per mesh object a build item reaches, in build order.
    pub parts: Vec<Part>,
    /// The objects of a type other than `model`, read and not built, in id order.
    pub ignored: Vec<(u32, ObjectType)>,
}

/// Why [`read`] refuses a package or [`write`] refuses its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreeMfError {
    /// The container (zip_core's refusal).
    Zip(Refusal),
    /// No model part in the package.
    NoModel,
    /// An XML part that does not parse (`part` names it).
    Xml { part: &'static str, err: svg_core::xml::Error },
    /// The model part's root element is not `model`.
    NotModel,
    /// A `unit` the spec does not define.
    Unit(String),
    /// A `requiredextensions` prefix (a consumer MUST refuse an extension it does not implement; this one implements
    /// none).
    RequiredExtension(String),
    /// Two objects with one id.
    DuplicateObject { id: u32 },
    /// An item or component naming an object that does not exist.
    NoSuchObject { id: u32 },
    /// A component chain that reaches its own object.
    ComponentCycle { id: u32 },
    /// A number that does not parse (`what` names the attribute; `object` is 0 outside an object).
    Number { object: u32, what: &'static str },
    /// A vertex with a NaN or infinite coordinate.
    NonFinite { object: u32, vertex: u32 },
    /// A triangle naming a vertex past the vertex list.
    IndexOutOfRange { object: u32, triangle: u32, index: u32 },
    /// A triangle naming one vertex twice (3MF Core §4.1.4: v1, v2 and v3 are distinct).
    RepeatedIndex { object: u32, triangle: u32 },
    /// A `transform` that is not 12 finite numbers.
    BadTransform,
    /// [`write`]: part `part`'s transform is not affine (bottom row not 0 0 0 1) or not finite.
    NotAffine { part: usize },
    /// No `build` element.
    NoBuild,
}

impl From<Refusal> for ThreeMfError {
    fn from(r: Refusal) -> Self {
        ThreeMfError::Zip(r)
    }
}

enum Body {
    Mesh(Mesh),
    Components(Vec<(u32, Mat4)>),
}

struct Object {
    ty: ObjectType,
    body: Body,
}

/// OPC part names compare case-insensitively (ECMA-376-2 §9.1.1.1.2); the leading `/` of a target is dropped.
fn part<'z>(z: &'z Archive<'z>, name: &str) -> Option<&'z Entry<'z>> {
    let name = name.strip_prefix('/').unwrap_or(name);
    z.entries().iter().find(|e| e.name.eq_ignore_ascii_case(name))
}

fn child(doc: &Document, n: usize, name: &str) -> Option<usize> {
    doc.elements(n).find(|&c| doc.nodes[c].name() == name)
}

fn children<'d>(doc: &'d Document, n: usize, name: &'d str) -> impl Iterator<Item = usize> + 'd {
    doc.elements(n).filter(move |&c| doc.nodes[c].name() == name)
}

fn unit_mm(u: &str) -> Option<f64> {
    Some(match u {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => return None,
    })
}

fn object_type(t: Option<&str>) -> ObjectType {
    match t {
        None | Some("model") => ObjectType::Model,
        Some("support") => ObjectType::Support,
        Some("solidsupport") => ObjectType::SolidSupport,
        Some("surface") => ObjectType::Surface,
        _ => ObjectType::Other,
    }
}

/// The 12 numbers of a `transform` (absent = identity), the translation scaled by the unit.
fn transform(t: Option<&str>, unit: f64) -> Result<Mat4, ThreeMfError> {
    let Some(t) = t else { return Ok(Mat4::IDENTITY) };
    let mut m = [0.0f64; 12];
    let mut n = 0;
    for tok in t.split_ascii_whitespace() {
        let v: f64 = tok.parse().map_err(|_| ThreeMfError::BadTransform)?;
        if n == 12 || !v.is_finite() {
            return Err(ThreeMfError::BadTransform);
        }
        m[n] = v;
        n += 1;
    }
    if n != 12 {
        return Err(ThreeMfError::BadTransform);
    }
    let s = |v: f64| if unit == 1.0 { v } else { v * unit };
    Ok(Mat4::from_cols(
        [m[0], m[1], m[2], 0.0],
        [m[3], m[4], m[5], 0.0],
        [m[6], m[7], m[8], 0.0],
        [s(m[9]), s(m[10]), s(m[11]), 1.0],
    ))
}

fn num<T: core::str::FromStr>(doc: &Document, n: usize, a: &str, object: u32, what: &'static str) -> Result<T, ThreeMfError> {
    doc.nodes[n].attr(a).and_then(|v| v.trim().parse().ok()).ok_or(ThreeMfError::Number { object, what })
}

fn read_mesh(doc: &Document, m: usize, id: u32, unit: f64) -> Result<Mesh, ThreeMfError> {
    let mut mesh = Mesh::default();
    if let Some(vs) = child(doc, m, "vertices") {
        for v in children(doc, vs, "vertex") {
            let x: f64 = num(doc, v, "x", id, "vertex x")?;
            let y: f64 = num(doc, v, "y", id, "vertex y")?;
            let z: f64 = num(doc, v, "z", id, "vertex z")?;
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(ThreeMfError::NonFinite { object: id, vertex: mesh.positions.len() as u32 });
            }
            let p = if unit == 1.0 { Vec3::new(x, y, z) } else { Vec3::new(x * unit, y * unit, z * unit) };
            mesh.positions.push(p);
        }
    }
    let n = mesh.positions.len() as u64;
    if let Some(ts) = child(doc, m, "triangles") {
        for t in children(doc, ts, "triangle") {
            let triangle = mesh.triangles.len() as u32;
            let v = [
                num::<u32>(doc, t, "v1", id, "triangle v1")?,
                num::<u32>(doc, t, "v2", id, "triangle v2")?,
                num::<u32>(doc, t, "v3", id, "triangle v3")?,
            ];
            if let Some(&index) = v.iter().find(|&&i| i as u64 >= n) {
                return Err(ThreeMfError::IndexOutOfRange { object: id, triangle, index });
            }
            if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
                return Err(ThreeMfError::RepeatedIndex { object: id, triangle });
            }
            mesh.triangles.push(v);
        }
    }
    Ok(mesh)
}

/// The model part's name: the `_rels/.rels` relationship of type [`MODEL_REL`], else `3D/3dmodel.model`.
fn model_name(z: &Archive) -> Result<String, ThreeMfError> {
    if let Some(rels) = part(z, "_rels/.rels") {
        let doc = Document::parse(&rels.data()?).map_err(|err| ThreeMfError::Xml { part: "_rels/.rels", err })?;
        if let Some(root) = doc.root_element() {
            for r in children(&doc, root, "Relationship") {
                let node = &doc.nodes[r];
                if node.attr("Type") == Some(MODEL_REL) {
                    if let Some(t) = node.attr("Target") {
                        return Ok(t.to_string());
                    }
                }
            }
        }
    }
    Ok(DEFAULT_MODEL.to_string())
}

fn expand(
    objects: &BTreeMap<u32, Object>,
    id: u32,
    m: Mat4,
    stack: &mut Vec<u32>,
    parts: &mut Vec<Part>,
) -> Result<(), ThreeMfError> {
    let obj = objects.get(&id).ok_or(ThreeMfError::NoSuchObject { id })?;
    if obj.ty != ObjectType::Model {
        return Ok(()); // listed in `ignored` by name
    }
    if stack.contains(&id) {
        return Err(ThreeMfError::ComponentCycle { id });
    }
    match &obj.body {
        Body::Mesh(mesh) => parts.push(Part { object: id, mesh: mesh.clone(), transform: m }),
        Body::Components(cs) => {
            stack.push(id);
            for &(cid, cm) in cs {
                expand(objects, cid, m * cm, stack, parts)?;
            }
            stack.pop();
        }
    }
    Ok(())
}

/// Read a 3MF package: every mesh object its build reaches, placed, in millimetres; refusals by name.
pub fn read(b: &[u8]) -> Result<ThreeMf, ThreeMfError> {
    let z = zip_core::read(b)?;
    let name = model_name(&z)?;
    let model = part(&z, &name).ok_or(ThreeMfError::NoModel)?;
    let doc = Document::parse(&model.data()?).map_err(|err| ThreeMfError::Xml { part: "3D model", err })?;
    let root = doc.root_element().filter(|&r| doc.nodes[r].name() == "model").ok_or(ThreeMfError::NotModel)?;
    if let Some(ext) = doc.nodes[root].attr("requiredextensions").and_then(|r| r.split_ascii_whitespace().next()) {
        return Err(ThreeMfError::RequiredExtension(ext.to_string()));
    }
    let unit = match doc.nodes[root].attr("unit") {
        None => 1.0,
        Some(u) => unit_mm(u).ok_or_else(|| ThreeMfError::Unit(u.to_string()))?,
    };

    let mut objects: BTreeMap<u32, Object> = BTreeMap::new();
    if let Some(res) = child(&doc, root, "resources") {
        for o in children(&doc, res, "object") {
            let id: u32 = num(&doc, o, "id", 0, "object id")?;
            let ty = object_type(doc.nodes[o].attr("type"));
            let body = if let Some(m) = child(&doc, o, "mesh") {
                Body::Mesh(read_mesh(&doc, m, id, unit)?)
            } else if let Some(cs) = child(&doc, o, "components") {
                let mut v = Vec::new();
                for c in children(&doc, cs, "component") {
                    let cid: u32 = num(&doc, c, "objectid", id, "component objectid")?;
                    v.push((cid, transform(doc.nodes[c].attr("transform"), unit)?));
                }
                Body::Components(v)
            } else {
                Body::Mesh(Mesh::default())
            };
            if objects.insert(id, Object { ty, body }).is_some() {
                return Err(ThreeMfError::DuplicateObject { id });
            }
        }
    }
    let ignored = objects.iter().filter(|(_, o)| o.ty != ObjectType::Model).map(|(&id, o)| (id, o.ty)).collect();

    let build = child(&doc, root, "build").ok_or(ThreeMfError::NoBuild)?;
    let mut parts = Vec::new();
    let mut stack = Vec::new();
    for item in children(&doc, build, "item") {
        let id: u32 = num(&doc, item, "objectid", 0, "item objectid")?;
        let m = transform(doc.nodes[item].attr("transform"), unit)?;
        expand(&objects, id, m, &mut stack, &mut parts)?;
    }
    Ok(ThreeMf { parts, ignored })
}

fn affine(m: &Mat4) -> bool {
    m.cols.iter().all(|c| c.iter().all(|v| v.is_finite()))
        && m.cols[0][3] == 0.0
        && m.cols[1][3] == 0.0
        && m.cols[2][3] == 0.0
        && m.cols[3][3] == 1.0
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\
</Types>";

const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" \
Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\
</Relationships>";

/// Write a 3MF package: one object (id `i + 1`, type `model`) and one build item per part, unit millimeter, the
/// transform omitted when it is the identity. No whitespace between elements (svg_core's DOM then holds one node per
/// element on the way back). Refuses what [`read`] would refuse: a non-finite position, an index past the vertex
/// list or repeated in a triangle, a transform that is not finite and affine; and the container's ZIP64 limits.
pub fn write(parts: &[(&Mesh, Mat4)]) -> Result<Vec<u8>, ThreeMfError> {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = write!(x, "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{}\"><resources>", NS_CORE);
    for (i, (mesh, m)) in parts.iter().enumerate() {
        let object = i as u32 + 1;
        if !affine(m) {
            return Err(ThreeMfError::NotAffine { part: i });
        }
        let _ = write!(x, "<object id=\"{}\" type=\"model\"><mesh><vertices>", object);
        for (v, p) in mesh.positions.iter().enumerate() {
            if !p.is_finite() {
                return Err(ThreeMfError::NonFinite { object, vertex: v as u32 });
            }
            let _ = write!(x, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p.x, p.y, p.z);
        }
        x.push_str("</vertices><triangles>");
        let n = mesh.positions.len() as u64;
        for (t, tri) in mesh.triangles.iter().enumerate() {
            let triangle = t as u32;
            if let Some(&index) = tri.iter().find(|&&ix| ix as u64 >= n) {
                return Err(ThreeMfError::IndexOutOfRange { object, triangle, index });
            }
            if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                return Err(ThreeMfError::RepeatedIndex { object, triangle });
            }
            let _ = write!(x, "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", tri[0], tri[1], tri[2]);
        }
        x.push_str("</triangles></mesh></object>");
    }
    x.push_str("</resources><build>");
    for (i, (_, m)) in parts.iter().enumerate() {
        let _ = write!(x, "<item objectid=\"{}\"", i + 1);
        if *m != Mat4::IDENTITY {
            x.push_str(" transform=\"");
            for c in 0..4 {
                for r in 0..3 {
                    if c + r > 0 {
                        x.push(' ');
                    }
                    let _ = write!(x, "{}", m.cols[c][r]);
                }
            }
            x.push('"');
        }
        x.push_str("/>");
    }
    x.push_str("</build></model>");

    let mut w = zip_core::Writer::new();
    w.add("[Content_Types].xml", CONTENT_TYPES.as_bytes(), Method::Stored)?;
    w.add("_rels/.rels", RELS.as_bytes(), Method::Stored)?;
    w.add(DEFAULT_MODEL, x.as_bytes(), Method::Deflated)?;
    Ok(w.finish())
}
