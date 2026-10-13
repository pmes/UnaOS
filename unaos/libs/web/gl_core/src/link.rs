//! Program link (ES 2.0 §2.10.3–§2.10.4, GLSL ES §4.3.5 and §4.5.3): both stages are generated
//! ([`crate::glsl::codegen`]), varyings are matched by name and type, uniforms of both stages merged by name
//! (type and precision must agree), attribute locations assigned (bindAttribLocation first, then first fit;
//! a matrix takes one slot per column), the vec4 budgets checked, and the uniform table built.
//!
//! The uniform STORE is one vec4 row per register a uniform occupies (a mat4 element = 4 rows), shared by both
//! stages: `vs_map` / `fs_map` list (store row → that stage's register) and the draw copies them in.

use crate::consts::*;
use crate::context::{MAX_ATTRIBS, MAX_FRAGMENT_UNIFORM_VECTORS_V, MAX_VARYING_VECTORS_V, MAX_VERTEX_UNIFORM_VECTORS_V};
use crate::glsl::ast::{Base, Ty};
use crate::glsl::codegen::{self, StageCode};
use crate::glsl::Compiled;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct AttribSlot {
    pub name: String,
    pub ty: Ty,
    pub gl_type: u32,
    pub location: u32,
    pub reg: u16,
}

#[derive(Clone, Debug)]
pub struct UniformInfo {
    /// The active-info name ("a[0]" for an array).
    pub name: String,
    /// The name without the array suffix.
    pub base: String,
    pub ty: Ty,
    pub gl_type: u32,
    /// Array length (1 for a non-array).
    pub size: u32,
    pub is_array: bool,
    /// First store row and rows per element.
    pub row: u32,
    pub rows: u32,
}

pub struct Linked {
    pub vs: StageCode,
    pub fs: StageCode,
    pub attribs: Vec<AttribSlot>,
    pub uniforms: Vec<UniformInfo>,
    /// Every valid uniform location: (uniform index, array element).
    pub locations: Vec<(u32, u32)>,
    pub store_rows: usize,
    pub vs_map: Vec<(u32, u16)>,
    pub fs_map: Vec<(u32, u16)>,
    /// Interpolated slots: (vertex-stage register, fragment-stage register) per vec4.
    pub varyings: Vec<(Option<u16>, u16)>,
}

/// The GL type enum of a basic GLSL type (ES 2.0 Table 2.10).
pub fn gl_type(t: &Ty) -> u32 {
    match (t.base, t.rows, t.cols) {
        (Base::Float, 1, 1) => FLOAT,
        (Base::Float, 2, 1) => FLOAT_VEC2,
        (Base::Float, 3, 1) => FLOAT_VEC3,
        (Base::Float, 4, 1) => FLOAT_VEC4,
        (Base::Float, 2, 2) => FLOAT_MAT2,
        (Base::Float, 3, 3) => FLOAT_MAT3,
        (Base::Float, 4, 4) => FLOAT_MAT4,
        (Base::Int, 1, _) => INT,
        (Base::Int, 2, _) => INT_VEC2,
        (Base::Int, 3, _) => INT_VEC3,
        (Base::Int, 4, _) => INT_VEC4,
        (Base::Bool, 1, _) => BOOL,
        (Base::Bool, 2, _) => BOOL_VEC2,
        (Base::Bool, 3, _) => BOOL_VEC3,
        (Base::Bool, 4, _) => BOOL_VEC4,
        (Base::Sampler2D, _, _) => SAMPLER_2D,
        (Base::SamplerCube, _, _) => SAMPLER_CUBE,
        _ => 0,
    }
}

fn varying_regs(t: &Ty) -> u16 {
    t.cols as u16 * t.arr.max(1)
}

pub fn link(vs: &Compiled, fs: &Compiled, bindings: &BTreeMap<String, u32>) -> Result<Linked, String> {
    let vcode = codegen::generate(&vs.unit).map_err(|e| format!("Vertex shader: {e}"))?;
    let fcode = codegen::generate(&fs.unit).map_err(|e| format!("Fragment shader: {e}"))?;

    // Varyings (GLSL ES §4.3.5).
    let mut varyings = Vec::new();
    let mut vary_vectors = 0u32;
    for fv in &fcode.varyings {
        match vcode.varyings.iter().find(|v| v.name == fv.name) {
            None => {
                if fv.reg.is_some() {
                    return Err(format!("Varying '{}' is used in the fragment shader but not declared in the vertex shader", fv.name));
                }
            }
            Some(vv) => {
                if vv.ty != fv.ty {
                    return Err(format!("Types of varying '{}' differ between the shaders", fv.name));
                }
                if vv.invariant != fv.invariant {
                    return Err(format!("Invariance of varying '{}' differs between the shaders (GLSL ES §4.6.4)", fv.name));
                }
                if let Some(fr) = fv.reg {
                    let n = varying_regs(&fv.ty);
                    vary_vectors += n as u32;
                    for k in 0..n {
                        varyings.push((vv.reg.map(|r| r + k), fr + k));
                    }
                }
            }
        }
    }
    if vary_vectors > MAX_VARYING_VECTORS_V {
        return Err(format!("Too many varyings ({vary_vectors} vectors > {MAX_VARYING_VECTORS_V})"));
    }

    // Uniforms: merged by name; both declarations must agree (GLSL ES §4.5.3).
    let mut uniforms: Vec<UniformInfo> = Vec::new();
    let mut vs_map = Vec::new();
    let mut fs_map = Vec::new();
    let mut row = 0u32;
    let mut order: Vec<String> = Vec::new();
    for l in vcode.uniforms.iter().chain(fcode.uniforms.iter()) {
        if !order.contains(&l.name) {
            order.push(l.name.clone());
        }
    }
    for name in order {
        let a = vcode.uniforms.iter().find(|l| l.name == name);
        let b = fcode.uniforms.iter().find(|l| l.name == name);
        if let (Some(a), Some(b)) = (a, b) {
            if a.ty != b.ty {
                return Err(format!("Types of uniform '{name}' differ between the shaders"));
            }
            if a.prec != b.prec && !a.ty.is_sampler() {
                return Err(format!("Precisions of uniform '{name}' differ between the shaders (GLSL ES §4.5.3)"));
            }
        }
        let used = a.is_some_and(|l| l.used) || b.is_some_and(|l| l.used);
        if !used {
            continue;
        }
        let leaf = a.or(b).unwrap();
        let size = leaf.ty.arr.max(1) as u32;
        let rows = leaf.ty.cols as u32;
        let elem = Ty { arr: 0, ..leaf.ty };
        let info = UniformInfo {
            name: if leaf.ty.arr > 0 { format!("{name}[0]") } else { name.clone() },
            base: name.clone(),
            ty: elem,
            gl_type: gl_type(&elem),
            size,
            is_array: leaf.ty.arr > 0,
            row,
            rows,
        };
        for (l, map) in [(a, &mut vs_map), (b, &mut fs_map)] {
            if let Some(l) = l.filter(|l| l.used) {
                for k in 0..size * rows {
                    map.push((row + k, l.reg + k as u16));
                }
            }
        }
        row += size * rows;
        uniforms.push(info);
    }
    let rows_of = |code: &StageCode| -> u32 { code.uniforms.iter().filter(|l| l.used).map(|l| l.ty.cols as u32 * l.ty.arr.max(1) as u32).sum() };
    if rows_of(&vcode) > MAX_VERTEX_UNIFORM_VECTORS_V {
        return Err("Too many vertex shader uniforms".into());
    }
    if rows_of(&fcode) > MAX_FRAGMENT_UNIFORM_VECTORS_V {
        return Err("Too many fragment shader uniforms".into());
    }
    let mut locations = Vec::new();
    for (i, u) in uniforms.iter().enumerate() {
        for e in 0..u.size {
            locations.push((i as u32, e));
        }
    }

    // Attribute locations (ES 2.0 §2.10.4).
    let mut taken = [false; MAX_ATTRIBS];
    let mut attribs = Vec::new();
    let mut pending = Vec::new();
    for a in &vcode.attributes {
        let slots = a.ty.cols as usize;
        match bindings.get(&a.name) {
            Some(&loc) => {
                let loc = loc as usize;
                if loc + slots > MAX_ATTRIBS {
                    return Err(format!("Attribute '{}' bound to location {loc} does not fit", a.name));
                }
                for s in loc..loc + slots {
                    if taken[s] {
                        return Err(format!("Attribute '{}' aliases another attribute at location {s}", a.name));
                    }
                    taken[s] = true;
                }
                attribs.push(AttribSlot { name: a.name.clone(), ty: a.ty, gl_type: gl_type(&a.ty), location: loc as u32, reg: a.reg });
            }
            None => pending.push(a),
        }
    }
    for a in pending {
        let slots = a.ty.cols as usize;
        let Some(loc) = (0..=MAX_ATTRIBS - slots).find(|&l| (l..l + slots).all(|s| !taken[s])) else {
            return Err(format!("Too many vertex attributes (no room for '{}')", a.name));
        };
        for s in loc..loc + slots {
            taken[s] = true;
        }
        attribs.push(AttribSlot { name: a.name.clone(), ty: a.ty, gl_type: gl_type(&a.ty), location: loc as u32, reg: a.reg });
    }

    Ok(Linked { vs: vcode, fs: fcode, attribs, uniforms, locations, store_rows: row as usize, vs_map, fs_map, varyings })
}

impl Linked {
    /// glGetUniformLocation's name rules (ES 2.0 §2.10.4, WebGL 1.0 getUniformLocation): "a", "a[0]" and
    /// "a[k]" for an array; the exact name otherwise.
    pub fn location(&self, name: &str) -> Option<u32> {
        let (base, elem) = match name.strip_suffix(']').and_then(|s| s.rfind('[').map(|i| (&s[..i], &s[i + 1..]))) {
            Some((b, digits)) => {
                if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || (digits.len() > 1 && digits.starts_with('0')) {
                    return None;
                }
                (b.to_string(), Some(digits.parse::<u32>().ok()?))
            }
            None => (name.to_string(), None),
        };
        let (ui, u) = self.uniforms.iter().enumerate().find(|(_, u)| u.base == base)?;
        let e = match elem {
            Some(e) => {
                if !u.is_array || e >= u.size {
                    return None;
                }
                e
            }
            None => 0,
        };
        self.locations.iter().position(|&(i, k)| i == ui as u32 && k == e).map(|p| p as u32)
    }
}
