//! Shader and program objects' entry points (ES 2.0 §2.10.1–§2.10.3, WebGL 1.0 shaderSource / compileShader /
//! getShaderParameter / getShaderInfoLog / getShaderSource).

use crate::consts::*;
use crate::context::{ActiveInfo, Context, Param, UniformLoc};
use crate::glsl;
use alloc::rc::Rc;
use alloc::string::String;

impl Context {
    pub fn shader_source(&mut self, shader: u32, src: &str) {
        let Some(s) = self.shaders.get_mut(shader) else { return self.err(INVALID_VALUE) };
        s.source = src.into();
    }

    /// ES 2.0 §2.10.1 CompileShader: COMPILE_STATUS and the info log are replaced; a previously compiled
    /// unit stays attached to programs already linked (they keep their own copy).
    pub fn compile_shader(&mut self, shader: u32) {
        let derivs = self.ext.standard_derivatives;
        let Some(s) = self.shaders.get_mut(shader) else { return self.err(INVALID_VALUE) };
        let (unit, log) = glsl::compile(&s.source, s.kind == FRAGMENT_SHADER, derivs);
        s.compiled = unit.is_some();
        s.code = unit.map(Rc::new);
        s.log = log;
    }

    pub fn get_shader_parameter(&mut self, shader: u32, pname: u32) -> Param {
        let Some(s) = self.shaders.get(shader) else {
            self.err(INVALID_VALUE);
            return Param::Null;
        };
        match pname {
            SHADER_TYPE => Param::Int(s.kind as i64),
            DELETE_STATUS => Param::Bool(s.deleted),
            COMPILE_STATUS => Param::Bool(s.compiled),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    pub fn get_shader_info_log(&mut self, shader: u32) -> Option<String> {
        match self.shaders.get(shader) {
            Some(s) => Some(s.log.clone()),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }

    pub fn get_shader_source(&mut self, shader: u32) -> Option<String> {
        match self.shaders.get(shader) {
            Some(s) => Some(s.source.clone()),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }
}

/// The data kind of a uniform* call: uniform{1,2,3,4}f[v] or uniform{1,2,3,4}i[v].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UKind {
    Float,
    Int,
}

impl Context {
    fn program_ok(&mut self, p: u32) -> bool {
        if self.programs.get(p).is_none() {
            self.err(INVALID_VALUE);
            return false;
        }
        true
    }

    /// ES 2.0 §2.10.3 AttachShader: one shader of each type; the same shader twice is INVALID_OPERATION.
    pub fn attach_shader(&mut self, p: u32, s: u32) {
        if !self.program_ok(p) {
            return;
        }
        let Some(kind) = self.shaders.get(s).map(|x| x.kind) else { return self.err(INVALID_VALUE) };
        let prog = self.programs.get_mut(p).unwrap();
        let slot = if kind == VERTEX_SHADER { &mut prog.vs } else { &mut prog.fs };
        if *slot != 0 {
            return self.err(INVALID_OPERATION);
        }
        *slot = s;
        self.shaders.get_mut(s).unwrap().attached += 1;
    }

    pub fn detach_shader(&mut self, p: u32, s: u32) {
        if !self.program_ok(p) {
            return;
        }
        if self.shaders.get(s).is_none() {
            return self.err(INVALID_VALUE);
        }
        let prog = self.programs.get_mut(p).unwrap();
        if prog.vs == s {
            prog.vs = 0;
        } else if prog.fs == s {
            prog.fs = 0;
        } else {
            return self.err(INVALID_OPERATION);
        }
        if let Some(sh) = self.shaders.get_mut(s) {
            sh.attached = sh.attached.saturating_sub(1);
        }
        self.sweep();
    }

    pub fn get_attached_shaders(&mut self, p: u32) -> Option<alloc::vec::Vec<u32>> {
        if !self.program_ok(p) {
            return None;
        }
        let prog = self.programs.get(p).unwrap();
        Some([prog.vs, prog.fs].into_iter().filter(|s| *s != 0).collect())
    }

    /// ES 2.0 §2.10.4 BindAttribLocation (applies at the next link); `gl_` names are INVALID_OPERATION.
    pub fn bind_attrib_location(&mut self, p: u32, index: u32, name: &str) {
        if index as usize >= crate::context::MAX_ATTRIBS {
            return self.err(INVALID_VALUE);
        }
        if name.starts_with("gl_") || name.starts_with("webgl_") || name.starts_with("_webgl_") {
            return self.err(INVALID_OPERATION);
        }
        if !self.program_ok(p) {
            return;
        }
        self.programs.get_mut(p).unwrap().bindings.insert(name.into(), index);
    }

    /// ES 2.0 §2.10.3 LinkProgram.
    pub fn link_program(&mut self, p: u32) {
        if !self.program_ok(p) {
            return;
        }
        let (vs, fs) = {
            let prog = self.programs.get(p).unwrap();
            (prog.vs, prog.fs)
        };
        let vcode = self.shaders.get(vs).and_then(|s| s.code.clone());
        let fcode = self.shaders.get(fs).and_then(|s| s.code.clone());
        let result = match (vcode, fcode) {
            (Some(v), Some(f)) => {
                let b = self.programs.get(p).unwrap().bindings.clone();
                crate::link::link(&v, &f, &b)
            }
            _ => Err(String::from("Attached vertex and fragment shaders must both be successfully compiled")),
        };
        let prog = self.programs.get_mut(p).unwrap();
        prog.link_gen = prog.link_gen.wrapping_add(1);
        prog.validated = false;
        match result {
            Ok(l) => {
                prog.store = alloc::vec![[0.0; 4]; l.store_rows];
                prog.linked_data = Some(Rc::new(l));
                prog.linked = true;
                prog.log = String::new();
            }
            Err(e) => {
                prog.linked = false;
                prog.linked_data = None;
                prog.store.clear();
                prog.log = e;
            }
        }
    }

    /// ES 2.0 §2.10.3 UseProgram: an unlinked program is INVALID_OPERATION.
    pub fn use_program(&mut self, p: u32) {
        if p != 0 {
            let Some(prog) = self.programs.get(p) else { return self.err(INVALID_VALUE) };
            if !prog.linked || prog.deleted {
                return self.err(INVALID_OPERATION);
            }
        }
        self.current_program = p;
        self.sweep();
    }

    /// The sampler units of a linked program (texture type per unit), or the conflict ES 2.0 §2.10.4 names:
    /// two samplers of different types on one unit.
    pub(crate) fn sampler_units(&self, p: u32) -> Result<alloc::vec::Vec<(usize, bool)>, ()> {
        let Some(prog) = self.programs.get(p) else { return Ok(alloc::vec::Vec::new()) };
        let Some(l) = prog.linked_data.as_ref() else { return Ok(alloc::vec::Vec::new()) };
        let mut out: alloc::vec::Vec<(usize, bool)> = alloc::vec::Vec::new();
        for u in &l.uniforms {
            let cube = match u.ty.base {
                crate::glsl::ast::Base::Sampler2D => false,
                crate::glsl::ast::Base::SamplerCube => true,
                _ => continue,
            };
            for e in 0..u.size {
                let unit = prog.store[(u.row + e) as usize][0] as usize;
                if let Some(x) = out.iter().find(|(un, _)| *un == unit) {
                    if x.1 != cube {
                        return Err(());
                    }
                } else {
                    out.push((unit, cube));
                }
            }
        }
        Ok(out)
    }

    pub fn validate_program(&mut self, p: u32) {
        if !self.program_ok(p) {
            return;
        }
        let ok = self.programs.get(p).unwrap().linked && self.sampler_units(p).is_ok();
        let prog = self.programs.get_mut(p).unwrap();
        prog.validated = ok;
        if !ok && prog.linked {
            prog.log = String::from("Samplers of different types use the same texture image unit");
        }
    }

    pub fn get_program_parameter(&mut self, p: u32, pname: u32) -> Param {
        if !self.program_ok(p) {
            return Param::Null;
        }
        let prog = self.programs.get(p).unwrap();
        let l = prog.linked_data.as_ref();
        match pname {
            DELETE_STATUS => Param::Bool(prog.deleted),
            LINK_STATUS => Param::Bool(prog.linked),
            VALIDATE_STATUS => Param::Bool(prog.validated),
            ATTACHED_SHADERS => Param::Int((prog.vs != 0) as i64 + (prog.fs != 0) as i64),
            ACTIVE_ATTRIBUTES => Param::Int(l.map(|l| l.attribs.len()).unwrap_or(0) as i64),
            ACTIVE_UNIFORMS => Param::Int(l.map(|l| l.uniforms.len()).unwrap_or(0) as i64),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    pub fn get_program_info_log(&mut self, p: u32) -> Option<String> {
        if !self.program_ok(p) {
            return None;
        }
        Some(self.programs.get(p).unwrap().log.clone())
    }

    fn linked(&mut self, p: u32) -> Option<Rc<crate::link::Linked>> {
        if !self.program_ok(p) {
            return None;
        }
        let prog = self.programs.get(p).unwrap();
        if !prog.linked {
            self.err(INVALID_OPERATION);
            return None;
        }
        prog.linked_data.clone()
    }

    /// ES 2.0 §2.10.4 GetAttribLocation: -1 when not an active attribute.
    pub fn get_attrib_location(&mut self, p: u32, name: &str) -> i32 {
        let Some(l) = self.linked(p) else { return -1 };
        l.attribs.iter().find(|a| a.name == name).map(|a| a.location as i32).unwrap_or(-1)
    }

    pub fn get_uniform_location(&mut self, p: u32, name: &str) -> Option<UniformLoc> {
        let l = self.linked(p)?;
        let index = l.location(name)?;
        Some(UniformLoc { program: p, link_gen: self.programs.get(p).unwrap().link_gen, index })
    }

    pub fn get_active_attrib(&mut self, p: u32, index: u32) -> Option<ActiveInfo> {
        if !self.program_ok(p) {
            return None;
        }
        let l = self.programs.get(p).unwrap().linked_data.clone();
        match l.as_ref().and_then(|l| l.attribs.get(index as usize)) {
            Some(a) => Some(ActiveInfo { size: 1, ty: a.gl_type, name: a.name.clone() }),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }

    pub fn get_active_uniform(&mut self, p: u32, index: u32) -> Option<ActiveInfo> {
        if !self.program_ok(p) {
            return None;
        }
        let l = self.programs.get(p).unwrap().linked_data.clone();
        match l.as_ref().and_then(|l| l.uniforms.get(index as usize)) {
            Some(u) => Some(ActiveInfo { size: u.size as i32, ty: u.gl_type, name: u.name.clone() }),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }

    /// The uniform a location names, checked against `program` and its link generation.
    fn resolve_loc(&mut self, program: u32, loc: &UniformLoc) -> Option<(Rc<crate::link::Linked>, u32, u32)> {
        if loc.program != program {
            self.err(INVALID_OPERATION);
            return None;
        }
        let Some(prog) = self.programs.get(program) else {
            self.err(INVALID_OPERATION);
            return None;
        };
        if prog.link_gen != loc.link_gen || !prog.linked {
            self.err(INVALID_OPERATION);
            return None;
        }
        let l = prog.linked_data.clone()?;
        let (ui, e) = *l.locations.get(loc.index as usize)?;
        Some((l, ui, e))
    }

    /// ES 2.0 §2.10.4 GetUniform, typed per WebGL 1.0 getUniform.
    pub fn get_uniform(&mut self, p: u32, loc: &UniformLoc) -> Param {
        if !self.program_ok(p) {
            return Param::Null;
        }
        let Some((l, ui, e)) = self.resolve_loc(p, loc) else { return Param::Null };
        let u = &l.uniforms[ui as usize];
        let store = &self.programs.get(p).unwrap().store;
        let row = (u.row + e * u.rows) as usize;
        let n = u.ty.rows as usize;
        let v = store[row];
        match u.gl_type {
            FLOAT => Param::Float(v[0] as f64),
            FLOAT_VEC2 | FLOAT_VEC3 | FLOAT_VEC4 => Param::F32s(v[..n].to_vec()),
            INT | SAMPLER_2D | SAMPLER_CUBE => Param::Int(v[0] as i64),
            INT_VEC2 | INT_VEC3 | INT_VEC4 => Param::I32s(v[..n].iter().map(|x| *x as i32).collect()),
            BOOL => Param::Bool(v[0] != 0.0),
            BOOL_VEC2 | BOOL_VEC3 | BOOL_VEC4 => Param::Bools(v[..n].iter().map(|x| *x != 0.0).collect()),
            _ => {
                let mut out = alloc::vec::Vec::new();
                for c in 0..u.rows as usize {
                    out.extend_from_slice(&store[row + c][..n]);
                }
                Param::F32s(out)
            }
        }
    }

    /// uniform{1,2,3,4}{f,i}[v] (ES 2.0 §2.10.4 with WebGL 1.0's checks). `count_form` = the *v form (the data
    /// may hold several elements); a null location is ignored silently.
    pub fn uniform(&mut self, loc: Option<&UniformLoc>, kind: UKind, comps: u8, data: &[f64], count_form: bool) {
        let cur = self.current_program;
        if cur == 0 {
            return self.err(INVALID_OPERATION);
        }
        let Some(loc) = loc else { return };
        let Some((l, ui, e)) = self.resolve_loc(cur, loc) else { return };
        let u = &l.uniforms[ui as usize];
        let base = u.ty.base;
        use crate::glsl::ast::Base;
        let type_ok = match base {
            Base::Float => kind == UKind::Float,
            Base::Int | Base::Sampler2D | Base::SamplerCube => kind == UKind::Int,
            Base::Bool => true,
            _ => false,
        };
        let shape_ok = !u.ty.is_matrix() && (if u.ty.is_sampler() { comps == 1 } else { u.ty.rows == comps });
        if !type_ok || !shape_ok {
            return self.err(INVALID_OPERATION);
        }
        let c = comps as usize;
        if data.is_empty() || data.len() % c != 0 {
            return self.err(INVALID_VALUE);
        }
        let count = data.len() / c;
        if count_form && count > 1 && !u.is_array {
            return self.err(INVALID_OPERATION);
        }
        if u.ty.is_sampler() && data.iter().any(|v| *v < 0.0 || *v >= crate::context::MAX_COMBINED_UNITS as f64) {
            return self.err(INVALID_VALUE);
        }
        let n = count.min((u.size - e) as usize);
        let (row, rows) = (u.row, u.rows);
        let prog = self.programs.get_mut(cur).unwrap();
        for k in 0..n {
            let r = &mut prog.store[(row + (e + k as u32) * rows) as usize];
            for i in 0..c {
                let v = data[k * c + i];
                r[i] = if base == Base::Bool { (v != 0.0) as u8 as f32 } else { v as f32 };
            }
        }
    }

    /// uniformMatrix{2,3,4}fv (WebGL 1.0: `transpose` must be false).
    pub fn uniform_matrix(&mut self, loc: Option<&UniformLoc>, n: u8, transpose: bool, data: &[f32]) {
        let cur = self.current_program;
        if cur == 0 {
            return self.err(INVALID_OPERATION);
        }
        let Some(loc) = loc else { return };
        if transpose {
            return self.err(INVALID_VALUE);
        }
        let Some((l, ui, e)) = self.resolve_loc(cur, loc) else { return };
        let u = &l.uniforms[ui as usize];
        if !(u.ty.is_matrix() && u.ty.cols == n) {
            return self.err(INVALID_OPERATION);
        }
        let per = (n as usize) * (n as usize);
        if data.is_empty() || data.len() % per != 0 {
            return self.err(INVALID_VALUE);
        }
        let count = data.len() / per;
        if count > 1 && !u.is_array {
            return self.err(INVALID_OPERATION);
        }
        let k_max = count.min((u.size - e) as usize);
        let (row, rows) = (u.row, u.rows);
        let prog = self.programs.get_mut(cur).unwrap();
        for k in 0..k_max {
            for c in 0..n as usize {
                let r = &mut prog.store[(row + (e + k as u32) * rows) as usize + c];
                for i in 0..n as usize {
                    r[i] = data[k * per + c * n as usize + i];
                }
            }
        }
    }
}
