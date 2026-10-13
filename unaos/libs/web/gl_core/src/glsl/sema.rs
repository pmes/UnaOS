//! Semantic analysis for GLSL ES 1.00: scopes (§4.2), types without implicit conversion (§4.1.10, §5),
//! qualifiers (§4.3), precision (§4.5), constructors (§5.4), l-values (§5.8), functions and overloads (§6.1),
//! the builtin variables and constants (§7), and the Appendix A loop rule (WebGL 1.0 "Supported GLSL
//! Constructs": a WebGL shader must stay within GLSL ES Appendix A §4 — `for` loops of the constant-bounded form
//! only; while and do-while are refused).
//!
//! Names are resolved in place and every expression's type is stamped; constant expressions (§5.10) are folded
//! where the code generator and array sizes need their values.

use super::ast::*;
use super::builtins;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VK {
    Global,
    Local,
    Const,
    Attribute,
    Uniform,
    Varying,
    Param(ParamQual, bool),
    Builtin,
}

/// The builtin variables (GLSL ES §7.1–§7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BV {
    Position,
    PointSize,
    FragCoord,
    FrontFacing,
    FragColor,
    FragData,
    PointCoord,
    DepthRange,
    Constant,
}

#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
    pub ty: Ty,
    pub kind: VK,
    pub prec: Prec,
    /// A folded constant value (scalar components in layout order), for const variables.
    pub cval: Option<Vec<f32>>,
    pub invariant: bool,
    pub builtin: Option<BV>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub ret: Ty,
    pub params: Vec<u32>,
    pub pquals: Vec<ParamQual>,
    pub ptys: Vec<Ty>,
    pub body: Option<Vec<Stmt>>,
    pub line: u32,
    pub calls: Vec<u32>,
}

/// A checked translation unit.
pub struct Checked {
    pub fragment: bool,
    pub vars: Vec<Var>,
    pub funcs: Vec<Func>,
    pub structs: Vec<StructDef>,
    /// Globals initialized at run time, in declaration order (the start of main runs them).
    pub global_inits: Vec<(u32, Expr)>,
    pub main: Option<u32>,
    pub derivatives: bool,
}

pub const MAX_VERTEX_ATTRIBS_C: f32 = 16.0;

struct S {
    fragment: bool,
    derivs: bool,
    vars: Vec<Var>,
    funcs: Vec<Func>,
    structs: Vec<StructDef>,
    scopes: Vec<BTreeMap<String, u32>>,
    /// Default precisions per scope: float, int, sampler2D, samplerCube (§4.5.3).
    precs: Vec<[Prec; 4]>,
    errors: Vec<(u32, String)>,
    cur: Option<u32>,
    loops: u32,
    loop_indices: Vec<u32>,
    global_inits: Vec<(u32, Expr)>,
    fn_names: BTreeMap<String, Vec<u32>>,
    wrote_color: Option<u32>,
    wrote_data: Option<u32>,
}

pub fn check(unit: Unit, fragment: bool, derivs: bool) -> Result<Checked, Vec<(u32, String)>> {
    let mut s = S {
        fragment,
        derivs,
        vars: Vec::new(),
        funcs: Vec::new(),
        structs: unit.structs,
        scopes: vec![BTreeMap::new()],
        precs: vec![if fragment { [Prec::None, Prec::Medium, Prec::Low, Prec::Low] } else { [Prec::High, Prec::High, Prec::Low, Prec::Low] }],
        errors: Vec::new(),
        cur: None,
        loops: 0,
        loop_indices: Vec::new(),
        global_inits: Vec::new(),
        fn_names: BTreeMap::new(),
        wrote_color: None,
        wrote_data: None,
    };
    s.builtins();
    for it in unit.items {
        s.external(it);
    }
    if let (Some(_), Some(l)) = (s.wrote_color, s.wrote_data) {
        s.errors.push((l, "cannot use both gl_FragColor and gl_FragData (GLSL ES §7.2)".into()));
    }
    let main = s.fn_names.get("main").and_then(|v| v.first().copied());
    s.recursion_check();
    if !s.errors.is_empty() {
        return Err(s.errors);
    }
    Ok(Checked { fragment, vars: s.vars, funcs: s.funcs, structs: s.structs, global_inits: s.global_inits, main, derivatives: derivs })
}

/// Scalar component count of a type (layout order: struct fields in order, matrices column-major, arrays
/// element by element).
pub fn comps(structs: &[StructDef], t: &Ty) -> usize {
    let one = match t.base {
        Base::Struct(id) => structs[id as usize].fields.iter().map(|(_, ft, _)| comps(structs, ft)).sum(),
        _ => t.rows as usize * t.cols as usize,
    };
    one * (t.arr.max(1) as usize)
}

fn struct_has(structs: &[StructDef], t: &Ty, pred: &dyn Fn(&Ty) -> bool) -> bool {
    if pred(t) {
        return true;
    }
    if let Base::Struct(id) = t.base {
        return structs[id as usize].fields.iter().any(|(_, ft, _)| struct_has(structs, ft, pred));
    }
    false
}

impl S {
    fn err(&mut self, line: u32, m: impl Into<String>) {
        self.errors.push((line, m.into()));
    }

    fn add_var(&mut self, v: Var) -> u32 {
        self.vars.push(v);
        (self.vars.len() - 1) as u32
    }

    fn declare(&mut self, name: &str, v: Var, line: u32) -> u32 {
        if name.starts_with("gl_") && v.kind != VK::Builtin {
            self.err(line, format!("'{name}' : identifiers starting with gl_ are reserved (GLSL ES §3.7)"));
        }
        if self.scopes.last().unwrap().contains_key(name) {
            self.err(line, format!("'{name}' : redefinition"));
        }
        if self.scopes.len() == 1 && self.fn_names.contains_key(name) {
            self.err(line, format!("'{name}' : redefinition (a function has this name)"));
        }
        let id = self.add_var(v);
        self.scopes.last_mut().unwrap().insert(name.to_string(), id);
        id
    }

    fn lookup(&self, name: &str) -> Option<u32> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn push(&mut self) {
        self.scopes.push(BTreeMap::new());
        let p = *self.precs.last().unwrap();
        self.precs.push(p);
    }

    fn pop(&mut self) {
        self.scopes.pop();
        self.precs.pop();
    }

    fn builtins(&mut self) {
        let bv = |name: &str, ty: Ty, b: BV, prec: Prec| Var { name: name.into(), ty, kind: VK::Builtin, prec, cval: None, invariant: false, builtin: Some(b), line: 0 };
        let v4 = Ty::vec(Base::Float, 4);
        let mut list = Vec::new();
        if self.fragment {
            list.push(bv("gl_FragCoord", v4, BV::FragCoord, Prec::Medium));
            list.push(bv("gl_FrontFacing", Ty::BOOL, BV::FrontFacing, Prec::None));
            list.push(bv("gl_FragColor", v4, BV::FragColor, Prec::Medium));
            list.push(bv("gl_FragData", Ty { arr: 1, ..v4 }, BV::FragData, Prec::Medium));
            list.push(bv("gl_PointCoord", Ty::vec(Base::Float, 2), BV::PointCoord, Prec::Medium));
        } else {
            list.push(bv("gl_Position", v4, BV::Position, Prec::High));
            list.push(bv("gl_PointSize", Ty::FLOAT, BV::PointSize, Prec::Medium));
        }
        // gl_DepthRange (§7.5): uniform gl_DepthRangeParameters { highp float near, far, diff; }.
        let id = self.structs.len() as u16;
        self.structs.push(StructDef {
            name: "gl_DepthRangeParameters".into(),
            fields: vec![("near".into(), Ty::FLOAT, Prec::High), ("far".into(), Ty::FLOAT, Prec::High), ("diff".into(), Ty::FLOAT, Prec::High)],
            sizes: vec![None, None, None],
            line: 0,
        });
        list.push(bv("gl_DepthRange", Ty { base: Base::Struct(id), rows: 1, cols: 1, arr: 0 }, BV::DepthRange, Prec::High));
        // §7.4 builtin constants (values = this implementation's limits, context.rs).
        for (n, v) in [
            ("gl_MaxVertexAttribs", 16.0),
            ("gl_MaxVertexUniformVectors", crate::context::MAX_VERTEX_UNIFORM_VECTORS_V as f32),
            ("gl_MaxVaryingVectors", crate::context::MAX_VARYING_VECTORS_V as f32),
            ("gl_MaxVertexTextureImageUnits", crate::context::MAX_VERTEX_TEXTURE_UNITS_V as f32),
            ("gl_MaxCombinedTextureImageUnits", crate::context::MAX_COMBINED_UNITS as f32),
            ("gl_MaxTextureImageUnits", crate::context::MAX_TEXTURE_UNITS_V as f32),
            ("gl_MaxFragmentUniformVectors", crate::context::MAX_FRAGMENT_UNIFORM_VECTORS_V as f32),
            ("gl_MaxDrawBuffers", 1.0),
        ] {
            let mut c = bv(n, Ty::INT, BV::Constant, Prec::Medium);
            c.cval = Some(vec![v]);
            list.push(c);
        }
        for v in list {
            let n = v.name.clone();
            let id = self.add_var(v);
            self.scopes[0].insert(n, id);
        }
    }

    // -------------------------------------------------------------------------------------- types

    fn prec_slot(t: &Ty) -> Option<usize> {
        match t.base {
            Base::Float => Some(0),
            Base::Int => Some(1),
            Base::Sampler2D => Some(2),
            Base::SamplerCube => Some(3),
            _ => None,
        }
    }

    /// The effective precision of a declaration; a float type with neither an explicit precision nor a
    /// default in scope is an error in the fragment language (§4.5.3).
    fn effective_prec(&mut self, t: &Ty, p: Prec, line: u32) -> Prec {
        if p != Prec::None {
            return p;
        }
        let Some(slot) = Self::prec_slot(t) else { return Prec::None };
        let d = self.precs.last().unwrap()[slot];
        if d == Prec::None {
            self.err(line, "No precision specified for (float) (GLSL ES §4.5.3)");
        }
        d
    }

    /// Evaluate an array size (§4.1.9: an integral constant expression greater than zero).
    fn array_size(&mut self, size: Option<Option<Expr>>, line: u32) -> Option<u16> {
        match size {
            None => Some(0),
            Some(None) => {
                self.err(line, "unsized arrays are not supported in GLSL ES 1.00 (§4.1.9)");
                None
            }
            Some(Some(mut e)) => {
                self.expr(&mut e);
                if e.ty.is_err() {
                    return None;
                }
                if e.ty != Ty::INT {
                    self.err(line, "array size must be an integer constant expression");
                    return None;
                }
                match self.fold(&e) {
                    Some(v) if v[0] >= 1.0 && v[0] <= 65535.0 => Some(v[0] as u16),
                    Some(_) => {
                        self.err(line, "array size must be greater than zero");
                        None
                    }
                    None => {
                        self.err(line, "array size must be a constant integer expression");
                        None
                    }
                }
            }
        }
    }

    fn struct_def(&mut self, id: u16) {
        let sizes = core::mem::take(&mut self.structs[id as usize].sizes);
        let line = self.structs[id as usize].line;
        for (k, sz) in sizes.into_iter().enumerate() {
            let n = self.array_size(sz, line).unwrap_or(0);
            let (ft, prec) = {
                let f = &mut self.structs[id as usize].fields[k];
                f.1.arr = n;
                (f.1, f.2)
            };
            if ft.is_sampler() {
                self.err(line, "samplers are not allowed in structures here");
            }
            let p = self.effective_prec(&ft, prec, line);
            self.structs[id as usize].fields[k].2 = p;
        }
    }

    // -------------------------------------------------------------------------------------- declarations

    fn external(&mut self, it: Ext) {
        match it {
            Ext::Precision(p, t, line) => self.precision(p, t, line),
            Ext::Invariant(names, line) => {
                for n in names {
                    match self.lookup(&n) {
                        Some(id) => {
                            let v = &self.vars[id as usize];
                            let ok = v.kind == VK::Varying || matches!(v.builtin, Some(BV::Position | BV::PointSize | BV::FragCoord | BV::PointCoord | BV::FrontFacing));
                            if ok {
                                self.vars[id as usize].invariant = true;
                            } else {
                                self.err(line, format!("'{n}' : only outputs (varyings, gl_Position, gl_PointSize) can be invariant"));
                            }
                        }
                        None => self.err(line, format!("'{n}' : undeclared identifier")),
                    }
                }
            }
            Ext::Struct(id) => self.struct_def(id),
            Ext::Decl(g) => {
                self.decl_group(g, true);
            }
            Ext::Func(f) => self.function(f),
        }
    }

    fn precision(&mut self, p: Prec, t: Ty, line: u32) {
        match Self::prec_slot(&t) {
            Some(slot) if t.is_scalar() || t.is_sampler() => self.precs.last_mut().unwrap()[slot] = p,
            _ => self.err(line, "precision statements apply to float, int and sampler types only (§4.5.3)"),
        }
    }

    fn decl_group(&mut self, mut g: DeclGroup, global: bool) -> DeclGroup {
        let decls = core::mem::take(&mut g.decls);
        let mut out = Vec::new();
        for mut d in decls {
            let line = d.line;
            let size = d.size.take();
            let had_size = size.is_some();
            let Some(n) = self.array_size(size, line) else {
                out.push(d);
                continue;
            };
            let mut ty = d.ty;
            ty.arr = n;
            d.ty = ty;
            if ty.base == Base::Void {
                self.err(line, format!("'{}' : illegal use of type 'void'", d.name));
            }
            let simple_float = ty.base == Base::Float && ty.arr == 0;
            match g.qual {
                Qual::Attribute => {
                    if self.fragment {
                        self.err(line, "'attribute' : not supported in fragment shaders");
                    }
                    if !simple_float {
                        self.err(line, format!("'{}' : attributes must be float, vec or mat (§4.3.3)", d.name));
                    }
                }
                Qual::Varying => {
                    if ty.base != Base::Float {
                        self.err(line, format!("'{}' : varyings must be float, vec, mat or arrays of them (§4.3.5)", d.name));
                    }
                }
                Qual::Const => {
                    if had_size {
                        self.err(line, "arrays may not be declared constant (they cannot be initialized, §4.1.9)");
                    }
                    if d.init.is_none() {
                        self.err(line, format!("'{}' : variables with qualifier 'const' must be initialized", d.name));
                    }
                }
                _ => {}
            }
            if matches!(g.qual, Qual::Attribute | Qual::Varying | Qual::Uniform) && d.init.is_some() {
                self.err(line, format!("'{}' : cannot initialize this type of qualifier", d.name));
            }
            if ty.is_sampler() || struct_has(&self.structs, &ty, &|t: &Ty| t.is_sampler()) {
                if g.qual != Qual::Uniform {
                    self.err(line, format!("'{}' : samplers must be uniform (§4.1.7)", d.name));
                }
            }
            if g.invariant && g.qual != Qual::Varying {
                self.err(line, "'invariant' applies to varyings only");
            }
            let prec = self.effective_prec(&ty, g.prec, line);
            let mut cval = None;
            if let Some(init) = d.init.as_mut() {
                self.expr(init);
                if !init.ty.is_err() && init.ty != ty {
                    self.err(line, format!("'{}' : initializer type does not match the declaration", d.name));
                }
                if g.qual == Qual::Const {
                    cval = self.fold(init);
                }
            }
            let kind = match g.qual {
                Qual::Const => VK::Const,
                Qual::Attribute => VK::Attribute,
                Qual::Uniform => VK::Uniform,
                Qual::Varying => VK::Varying,
                Qual::None => {
                    if global {
                        VK::Global
                    } else {
                        VK::Local
                    }
                }
            };
            let v = Var { name: d.name.clone(), ty, kind, prec, cval: cval.clone(), invariant: g.invariant, builtin: None, line };
            let id = self.declare(&d.name.clone(), v, line);
            d.id = id;
            if global {
                if let Some(init) = d.init.take() {
                    if cval.is_none() {
                        self.global_inits.push((id, init));
                    }
                }
            }
            out.push(d);
        }
        g.decls = out;
        g
    }

    fn function(&mut self, mut f: FuncDef) {
        let line = f.line;
        if builtins::NAMES.contains(&f.name.as_str()) {
            self.err(line, format!("'{}' : cannot redefine or overload a built-in function (§6.1)", f.name));
            return;
        }
        if self.scopes[0].contains_key(&f.name) {
            self.err(line, format!("'{}' : redefinition (a variable has this name)", f.name));
            return;
        }
        if f.ret.is_sampler() {
            self.err(line, "functions cannot return samplers");
        }
        let ret_prec = self.effective_prec(&f.ret, f.ret_prec, line);
        let _ = ret_prec;
        let mut ptys = Vec::new();
        for p in f.params.iter_mut() {
            let size = p.size.take();
            let n = self.array_size(size, p.line).unwrap_or(0);
            p.ty.arr = n;
            if p.ty.base == Base::Void {
                self.err(p.line, "illegal use of type 'void' in a parameter");
            }
            if p.ty.is_sampler() && p.qual != ParamQual::In {
                self.err(p.line, "samplers can only be 'in' parameters");
            }
            ptys.push(p.ty);
        }
        if f.name == "main" && (f.ret != Ty::VOID || !f.params.is_empty()) {
            self.err(line, "'main' must be 'void main()' (§6.1)");
        }
        // Find the overload this declares or defines.
        let existing = self.fn_names.get(&f.name).and_then(|ids| ids.iter().copied().find(|id| self.funcs[*id as usize].ptys == ptys));
        let fid = match existing {
            Some(id) => {
                let (oret, oq, obody) = {
                    let old = &self.funcs[id as usize];
                    (old.ret, old.pquals.clone(), old.body.is_some())
                };
                if oret != f.ret {
                    self.err(line, format!("'{}' : overloaded functions must have the same return type", f.name));
                }
                if oq != f.params.iter().map(|p| p.qual).collect::<Vec<_>>() {
                    self.err(line, format!("'{}' : parameter qualifiers differ from the prototype", f.name));
                }
                if obody && f.body.is_some() {
                    self.err(line, format!("'{}' : function already has a body", f.name));
                    return;
                }
                id
            }
            None => {
                if f.name == "main" && self.fn_names.contains_key("main") {
                    self.err(line, "'main' cannot be overloaded");
                }
                self.funcs.push(Func {
                    name: f.name.clone(),
                    ret: f.ret,
                    params: Vec::new(),
                    pquals: f.params.iter().map(|p| p.qual).collect(),
                    ptys: ptys.clone(),
                    body: None,
                    line,
                    calls: Vec::new(),
                });
                let id = (self.funcs.len() - 1) as u32;
                self.fn_names.entry(f.name.clone()).or_default().push(id);
                id
            }
        };
        let Some(mut body) = f.body.take() else { return };
        // Parameters and the body's top level share one scope (§6.1).
        self.push();
        let mut pids = Vec::new();
        for p in &f.params {
            let prec = self.effective_prec(&p.ty, p.prec, p.line);
            let v = Var {
                name: p.name.clone().unwrap_or_default(),
                ty: p.ty,
                kind: VK::Param(p.qual, p.cnst),
                prec,
                cval: None,
                invariant: false,
                builtin: None,
                line: p.line,
            };
            let id = match &p.name {
                Some(n) => self.declare(n, v, p.line),
                None => self.add_var(v),
            };
            pids.push(id);
        }
        self.funcs[fid as usize].params = pids;
        self.cur = Some(fid);
        for st in body.iter_mut() {
            self.stmt(st);
        }
        self.cur = None;
        self.pop();
        self.funcs[fid as usize].body = Some(body);
    }

    fn recursion_check(&mut self) {
        // §6.1: recursion (direct or indirect) is not allowed — a DFS over the static call graph.
        let n = self.funcs.len();
        let mut state = vec![0u8; n];
        fn dfs(f: usize, funcs: &[Func], state: &mut [u8]) -> bool {
            state[f] = 1;
            for &c in &funcs[f].calls {
                let c = c as usize;
                if state[c] == 1 || (state[c] == 0 && dfs(c, funcs, state)) {
                    return true;
                }
            }
            state[f] = 2;
            false
        }
        for f in 0..n {
            if state[f] == 0 && dfs(f, &self.funcs, &mut state) {
                let l = self.funcs[f].line;
                let name = self.funcs[f].name.clone();
                self.err(l, format!("'{name}' : recursion is not allowed (GLSL ES §6.1)"));
                return;
            }
        }
    }

    // -------------------------------------------------------------------------------------- statements

    fn stmt(&mut self, st: &mut Stmt) {
        match st {
            Stmt::Decl(g) => {
                let taken = core::mem::replace(g, DeclGroup { qual: Qual::None, invariant: false, prec: Prec::None, decls: Vec::new(), line: 0 });
                *g = self.decl_group(taken, false);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::Block(b) => {
                self.push();
                for s in b.iter_mut() {
                    self.stmt(s);
                }
                self.pop();
            }
            Stmt::If(c, a, b) => {
                self.expr(c);
                if !c.ty.is_err() && c.ty != Ty::BOOL {
                    let l = c.line;
                    self.err(l, "'if' : boolean expression expected");
                }
                self.stmt(a);
                if let Some(b) = b {
                    self.stmt(b);
                }
            }
            Stmt::For(init, cond, cdecl, step, body) => {
                self.push();
                self.stmt(init);
                if let Some(c) = cond.as_mut() {
                    self.expr(c);
                    if !c.ty.is_err() && c.ty != Ty::BOOL {
                        let l = c.line;
                        self.err(l, "'for' : boolean expression expected");
                    }
                }
                if let Some(d) = cdecl.take() {
                    let d = self.decl_group(d, false);
                    *cdecl = Some(d);
                }
                if let Some(s) = step.as_mut() {
                    self.expr(s);
                }
                let idx = self.appendix_a_for(init, cond.as_ref(), cdecl.is_some(), step.as_ref());
                if let Some(i) = idx {
                    self.loop_indices.push(i);
                }
                self.loops += 1;
                self.stmt(body);
                self.loops -= 1;
                if idx.is_some() {
                    self.loop_indices.pop();
                }
                self.pop();
            }
            Stmt::While(_, _, _) | Stmt::DoWhile(_, _) => {
                let line = match st {
                    Stmt::While(Some(c), _, _) => c.line,
                    Stmt::DoWhile(_, c) => c.line,
                    _ => 0,
                };
                self.err(line, "'while' : this type of loop is not allowed in WebGL (GLSL ES Appendix A §4)");
            }
            Stmt::Return(e, line) => {
                let line = *line;
                let ret = self.cur.map(|f| self.funcs[f as usize].ret).unwrap_or(Ty::VOID);
                match e {
                    Some(e) => {
                        self.expr(e);
                        if ret == Ty::VOID {
                            self.err(line, "'return' : void function cannot return a value");
                        } else if !e.ty.is_err() && e.ty != ret {
                            self.err(line, "'return' : function return is not matching type");
                        }
                    }
                    None => {
                        if ret != Ty::VOID {
                            self.err(line, "'return' : non-void function must return a value");
                        }
                    }
                }
            }
            Stmt::Break(l) | Stmt::Continue(l) => {
                if self.loops == 0 {
                    let l = *l;
                    self.err(l, "'break' / 'continue' : only allowed in loops");
                }
            }
            Stmt::Discard(l) => {
                if !self.fragment {
                    let l = *l;
                    self.err(l, "'discard' : only allowed in fragment shaders");
                }
            }
            Stmt::Precision(p, t, l) => {
                let (p, t, l) = (*p, *t, *l);
                self.precision(p, t, l);
            }
            Stmt::Struct(id) => {
                let id = *id;
                self.struct_def(id);
            }
            Stmt::Empty => {}
        }
    }

    /// GLSL ES Appendix A §4 (WebGL 1.0 requires it): `for (type i = const; i relop const; i++|i--|++i|--i|
    /// i += const|i -= const)`. Returns the loop index on success.
    fn appendix_a_for(&mut self, init: &Stmt, cond: Option<&Expr>, cond_decl: bool, step: Option<&Expr>) -> Option<u32> {
        let line = cond.map(|c| c.line).unwrap_or(0);
        let idx = match init {
            Stmt::Decl(g) if g.decls.len() == 1 && g.qual == Qual::None => {
                let d = &g.decls[0];
                let ok_ty = d.ty == Ty::INT || d.ty == Ty::FLOAT;
                let ok_init = d.init.as_ref().is_some_and(|e| self.fold(e).is_some());
                if !ok_ty || !ok_init {
                    self.err(d.line, "Invalid init declaration in for loop (GLSL ES Appendix A §4)");
                    return None;
                }
                d.id
            }
            _ => {
                self.err(line, "Invalid init declaration in for loop (GLSL ES Appendix A §4)");
                return None;
            }
        };
        let is_idx = |e: &Expr| matches!(e.k, EK::Var(v) if v == idx);
        let cond_ok = !cond_decl
            && cond.is_some_and(|c| match &c.k {
                EK::Bin(op, a, b) => matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne) && is_idx(a) && self.fold(b).is_some(),
                _ => false,
            });
        if !cond_ok {
            self.err(line, "Invalid condition in for loop (GLSL ES Appendix A §4)");
            return None;
        }
        let step_ok = step.is_some_and(|s| match &s.k {
            EK::Un(UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec, a) => is_idx(a),
            EK::Assign(AsgOp::Add | AsgOp::Sub, a, b) => is_idx(a) && self.fold(b).is_some(),
            _ => false,
        });
        if !step_ok {
            self.err(line, "Invalid expression in for loop (GLSL ES Appendix A §4)");
            return None;
        }
        Some(idx)
    }

    // -------------------------------------------------------------------------------------- expressions

    fn writable(&mut self, e: &Expr, line: u32) -> bool {
        match &e.k {
            EK::Var(id) => {
                let v = &self.vars[*id as usize];
                let ok = match v.kind {
                    VK::Const | VK::Uniform | VK::Attribute => false,
                    VK::Varying => !self.fragment,
                    VK::Param(_, c) => !c,
                    VK::Builtin => matches!(v.builtin, Some(BV::Position | BV::PointSize | BV::FragColor | BV::FragData)),
                    _ => true,
                };
                if !ok {
                    let n = v.name.clone();
                    self.err(line, format!("'{n}' : l-value required (cannot modify a constant, uniform, attribute, input or read-only variable)"));
                    return false;
                }
                if self.loop_indices.contains(id) {
                    let n = v.name.clone();
                    self.err(line, format!("'{n}' : loop index cannot be modified (GLSL ES Appendix A §4)"));
                    return false;
                }
                match v.builtin {
                    Some(BV::FragColor) => self.wrote_color = Some(line),
                    Some(BV::FragData) => self.wrote_data = Some(line),
                    _ => {}
                }
                true
            }
            EK::Member(a, _) | EK::Index(a, _) => self.writable(a, line),
            EK::Swizzle(a, sel, n) => {
                for i in 0..*n as usize {
                    for j in 0..i {
                        if sel[i] == sel[j] {
                            self.err(line, "l-value of swizzle cannot have duplicate components (§5.8)");
                            return false;
                        }
                    }
                }
                self.writable(a, line)
            }
            _ => {
                self.err(line, "l-value required");
                false
            }
        }
    }

    fn bin_ty(&self, op: BinOp, a: Ty, b: Ty) -> Option<Ty> {
        let arith_base = |t: &Ty| t.arr == 0 && matches!(t.base, Base::Float | Base::Int);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                if !arith_base(&a) || !arith_base(&b) || a.base != b.base {
                    return None;
                }
                if a == b {
                    return Some(a);
                }
                if a.is_scalar() {
                    return Some(b);
                }
                if b.is_scalar() {
                    return Some(a);
                }
                if op == BinOp::Mul && a.base == Base::Float {
                    if a.is_matrix() && b.is_vector() && a.cols == b.rows {
                        return Some(Ty::vec(Base::Float, a.rows));
                    }
                    if a.is_vector() && b.is_matrix() && a.rows == b.rows {
                        return Some(Ty::vec(Base::Float, b.cols));
                    }
                }
                None
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                if a.is_scalar() && a == b && a.base != Base::Bool { Some(Ty::BOOL) } else { None }
            }
            BinOp::Eq | BinOp::Ne => {
                let bad = |t: &Ty| t.is_array() || t.is_sampler();
                if a == b && !struct_has(&self.structs, &a, &bad) && a.base != Base::Void { Some(Ty::BOOL) } else { None }
            }
            BinOp::And | BinOp::Or | BinOp::Xor => {
                if a == Ty::BOOL && b == Ty::BOOL { Some(Ty::BOOL) } else { None }
            }
        }
    }

    pub fn expr(&mut self, e: &mut Expr) {
        let line = e.line;
        let k = core::mem::replace(&mut e.k, EK::Bool(false));
        let (k, ty) = self.expr_kind(k, line);
        e.k = k;
        e.ty = ty;
    }

    fn expr_kind(&mut self, k: EK, line: u32) -> (EK, Ty) {
        match k {
            EK::Int(v) => (EK::Int(v), Ty::INT),
            EK::Float(v) => (EK::Float(v), Ty::FLOAT),
            EK::Bool(v) => (EK::Bool(v), Ty::BOOL),
            EK::Name(n) => match self.lookup(&n) {
                Some(id) => {
                    let ty = self.vars[id as usize].ty;
                    (EK::Var(id), ty)
                }
                None => {
                    if self.fn_names.contains_key(&n) {
                        self.err(line, format!("'{n}' : function name used as a variable"));
                    } else {
                        self.err(line, format!("'{n}' : undeclared identifier"));
                    }
                    (EK::Name(n), Ty::ERR)
                }
            },
            EK::Var(id) => {
                let ty = self.vars[id as usize].ty;
                (EK::Var(id), ty)
            }
            EK::Bin(op, mut a, mut b) => {
                self.expr(&mut a);
                self.expr(&mut b);
                if a.ty.is_err() || b.ty.is_err() {
                    return (EK::Bin(op, a, b), Ty::ERR);
                }
                match self.bin_ty(op, a.ty, b.ty) {
                    Some(t) => (EK::Bin(op, a, b), t),
                    None => {
                        self.err(line, format!("'{op:?}' : wrong operand types — no operation exists for these operand types (§5.9)"));
                        (EK::Bin(op, a, b), Ty::ERR)
                    }
                }
            }
            EK::Un(op, mut a) => {
                self.expr(&mut a);
                if a.ty.is_err() {
                    return (EK::Un(op, a), Ty::ERR);
                }
                let t = a.ty;
                let ok = match op {
                    UnOp::Neg | UnOp::Pos => t.is_numeric() && !matches!(t.base, Base::Struct(_)),
                    UnOp::Not => t == Ty::BOOL,
                    _ => {
                        let num = t.is_numeric();
                        num && self.writable(&a, line)
                    }
                };
                if !ok {
                    if matches!(op, UnOp::Neg | UnOp::Pos | UnOp::Not) || !t.is_numeric() {
                        self.err(line, format!("'{op:?}' : wrong operand type"));
                    }
                    return (EK::Un(op, a), Ty::ERR);
                }
                (EK::Un(op, a), t)
            }
            EK::Assign(op, mut a, mut b) => {
                self.expr(&mut a);
                self.expr(&mut b);
                if a.ty.is_err() || b.ty.is_err() {
                    return (EK::Assign(op, a, b), Ty::ERR);
                }
                if !self.writable(&a, line) {
                    return (EK::Assign(op, a, b), Ty::ERR);
                }
                let ok = match op {
                    AsgOp::Set => a.ty == b.ty && !a.ty.is_array() && !struct_has(&self.structs, &a.ty, &|t: &Ty| t.is_array() || t.is_sampler()),
                    AsgOp::Add => self.bin_ty(BinOp::Add, a.ty, b.ty) == Some(a.ty),
                    AsgOp::Sub => self.bin_ty(BinOp::Sub, a.ty, b.ty) == Some(a.ty),
                    AsgOp::Mul => self.bin_ty(BinOp::Mul, a.ty, b.ty) == Some(a.ty),
                    AsgOp::Div => self.bin_ty(BinOp::Div, a.ty, b.ty) == Some(a.ty),
                };
                if !ok {
                    self.err(line, "'assign' : cannot convert between these types (§5.8)");
                    return (EK::Assign(op, a, b), Ty::ERR);
                }
                let t = a.ty;
                (EK::Assign(op, a, b), t)
            }
            EK::Cond(mut c, mut a, mut b) => {
                self.expr(&mut c);
                self.expr(&mut a);
                self.expr(&mut b);
                if c.ty.is_err() || a.ty.is_err() || b.ty.is_err() {
                    return (EK::Cond(c, a, b), Ty::ERR);
                }
                if c.ty != Ty::BOOL {
                    self.err(line, "'?:' : boolean expression expected");
                    return (EK::Cond(c, a, b), Ty::ERR);
                }
                if a.ty != b.ty || a.ty.is_array() {
                    self.err(line, "'?:' : the second and third expressions must have the same type");
                    return (EK::Cond(c, a, b), Ty::ERR);
                }
                let t = a.ty;
                (EK::Cond(c, a, b), t)
            }
            EK::Comma(mut a, mut b) => {
                self.expr(&mut a);
                self.expr(&mut b);
                let t = b.ty;
                (EK::Comma(a, b), t)
            }
            EK::Index(mut a, mut i) => {
                self.expr(&mut a);
                self.expr(&mut i);
                if a.ty.is_err() || i.ty.is_err() {
                    return (EK::Index(a, i), Ty::ERR);
                }
                if i.ty != Ty::INT {
                    self.err(line, "'[]' : integer expression required");
                    return (EK::Index(a, i), Ty::ERR);
                }
                let t = a.ty;
                let (rt, size) = if t.is_array() {
                    (t.elem(), t.arr as i64)
                } else if t.is_matrix() {
                    (Ty::vec(Base::Float, t.rows), t.cols as i64)
                } else if t.is_vector() {
                    (Ty::vec(t.base, 1), t.rows as i64)
                } else {
                    self.err(line, "'[]' : left of '[' is not of type array, matrix, or vector");
                    return (EK::Index(a, i), Ty::ERR);
                };
                if let Some(v) = self.fold(&i) {
                    let ix = v[0] as i64;
                    if ix < 0 || ix >= size {
                        self.err(line, format!("'[]' : index out of range '{ix}'"));
                        return (EK::Index(a, i), Ty::ERR);
                    }
                }
                (EK::Index(a, i), rt)
            }
            EK::Field(mut a, name) => {
                self.expr(&mut a);
                if a.ty.is_err() {
                    return (EK::Field(a, name), Ty::ERR);
                }
                let t = a.ty;
                if let (Base::Struct(id), 0) = (t.base, t.arr) {
                    let sd = &self.structs[id as usize];
                    return match sd.fields.iter().position(|(n, _, _)| *n == name) {
                        Some(ix) => {
                            let ft = sd.fields[ix].1;
                            (EK::Member(a, ix as u16), ft)
                        }
                        None => {
                            self.err(line, format!("'{name}' : no such field in structure"));
                            (EK::Field(a, name), Ty::ERR)
                        }
                    };
                }
                if (t.is_vector() || t.is_scalar()) && t.cols == 1 && !name.is_empty() && name.len() <= 4 {
                    let sets = ["xyzw", "rgba", "stpq"];
                    let chars: Vec<char> = name.chars().collect();
                    for set in sets {
                        if chars.iter().all(|c| set.contains(*c)) {
                            let mut sel = [0u8; 4];
                            let mut ok = true;
                            for (i, c) in chars.iter().enumerate() {
                                let k = set.find(*c).unwrap() as u8;
                                if k >= t.rows {
                                    ok = false;
                                }
                                sel[i] = k;
                            }
                            if !ok || t.is_scalar() {
                                self.err(line, format!("'{name}' : vector field selection out of range"));
                                return (EK::Field(a, name), Ty::ERR);
                            }
                            let n = chars.len() as u8;
                            return (EK::Swizzle(a, sel, n), Ty::vec(t.base, n));
                        }
                    }
                }
                self.err(line, format!("'{name}' : field selection requires structure or vector on left hand side"));
                (EK::Field(a, name), Ty::ERR)
            }
            EK::Call(name, mut args) => {
                for a in args.iter_mut() {
                    self.expr(a);
                }
                if args.iter().any(|a| a.ty.is_err()) {
                    return (EK::Call(name, args), Ty::ERR);
                }
                let tys: Vec<Ty> = args.iter().map(|a| a.ty).collect();
                if let Some(ids) = self.fn_names.get(&name) {
                    if let Some(fid) = ids.iter().copied().find(|id| self.funcs[*id as usize].ptys == tys) {
                        let quals = self.funcs[fid as usize].pquals.clone();
                        for (a, q) in args.iter().zip(quals.iter()) {
                            if *q != ParamQual::In && !self.writable(a, line) {
                                return (EK::Call(name, args), Ty::ERR);
                            }
                        }
                        if let Some(cur) = self.cur {
                            self.funcs[cur as usize].calls.push(fid);
                        }
                        let ret = self.funcs[fid as usize].ret;
                        return (EK::UserCall(fid, args), ret);
                    }
                    self.err(line, format!("'{name}' : no matching overloaded function found"));
                    return (EK::Call(name, args), Ty::ERR);
                }
                match builtins::resolve(&name, &tys, self.fragment, self.derivs) {
                    Some(Ok((bf, t))) => (EK::Builtin(bf, args), t),
                    Some(Err(m)) => {
                        self.err(line, m);
                        (EK::Call(name, args), Ty::ERR)
                    }
                    None => {
                        if self.lookup(&name).is_some() {
                            self.err(line, format!("'{name}' : not a function"));
                        } else {
                            self.err(line, format!("'{name}' : no matching overloaded function found"));
                        }
                        (EK::Call(name, args), Ty::ERR)
                    }
                }
            }
            EK::Ctor(t, mut args) => {
                for a in args.iter_mut() {
                    self.expr(a);
                }
                if args.iter().any(|a| a.ty.is_err()) {
                    return (EK::Ctor(t, args), Ty::ERR);
                }
                match self.ctor_check(t, &args) {
                    Ok(()) => (EK::Ctor(t, args), t),
                    Err(m) => {
                        self.err(line, m);
                        (EK::Ctor(t, args), Ty::ERR)
                    }
                }
            }
            other => {
                // Already-resolved forms (re-check): keep their stamped types.
                (other, Ty::ERR)
            }
        }
    }

    /// Constructor rules (§5.4.1–§5.4.3).
    fn ctor_check(&self, t: Ty, args: &[Expr]) -> Result<(), String> {
        if t.is_array() {
            return Err("array constructors are not supported in GLSL ES 1.00".into());
        }
        if t.is_sampler() || t.base == Base::Void {
            return Err("cannot construct this type".into());
        }
        if args.is_empty() {
            return Err("constructor does not have any arguments".into());
        }
        if let Base::Struct(id) = t.base {
            let sd = &self.structs[id as usize];
            if args.len() != sd.fields.len() {
                return Err(format!("'{}' : number of constructor parameters does not match the number of structure fields", sd.name));
            }
            for (a, (_, ft, _)) in args.iter().zip(sd.fields.iter()) {
                if a.ty != *ft {
                    return Err(format!("'{}' : structure constructor arguments do not match structure fields", sd.name));
                }
            }
            return Ok(());
        }
        for a in args {
            let at = a.ty;
            if at.is_array() || at.is_sampler() || matches!(at.base, Base::Struct(_) | Base::Void) {
                return Err("cannot convert a structure, array or sampler in a constructor".into());
            }
        }
        let need = t.components() as usize;
        if t.is_scalar() {
            if args.len() > 1 {
                return Err("too many arguments to constructor".into());
            }
            return Ok(());
        }
        if args.len() == 1 && args[0].ty.is_scalar() {
            return Ok(());
        }
        if t.is_matrix() && args.iter().any(|a| a.ty.is_matrix()) {
            return Err("constructing a matrix from a matrix is not allowed in GLSL ES 1.00 (§5.4.2)".into());
        }
        let mut have = 0usize;
        for a in args {
            if have >= need {
                return Err("too many arguments to constructor".into());
            }
            have += a.ty.components() as usize;
        }
        if have < need {
            return Err("not enough data provided for construction".into());
        }
        Ok(())
    }

    // -------------------------------------------------------------------------------------- constant folding

    /// Fold a typed constant expression (§5.10) into its scalar components; None when it is not constant (or
    /// uses a builtin function call or a matrix product, which run at run time instead).
    pub fn fold(&self, e: &Expr) -> Option<Vec<f32>> {
        fold(&self.vars, &self.structs, e)
    }
}

pub fn fold(vars: &[Var], structs: &[StructDef], e: &Expr) -> Option<Vec<f32>> {
    let r = |x: &Expr| fold(vars, structs, x);
    let int = e.ty.base == Base::Int;
    match &e.k {
        EK::Int(v) => Some(vec![*v as f32]),
        EK::Float(v) => Some(vec![*v]),
        EK::Bool(v) => Some(vec![*v as u8 as f32]),
        EK::Var(id) => vars.get(*id as usize).and_then(|v| v.cval.clone()),
        EK::Comma(_, b) => r(b),
        EK::Un(op, a) => {
            let v = r(a)?;
            match op {
                UnOp::Neg => Some(v.iter().map(|x| -x).collect()),
                UnOp::Pos => Some(v),
                UnOp::Not => Some(v.iter().map(|x| (*x == 0.0) as u8 as f32).collect()),
                _ => None,
            }
        }
        EK::Bin(op, a, b) => {
            let (x, y) = (r(a)?, r(b)?);
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                    if *op == BinOp::Mul && (a.ty.is_matrix() || b.ty.is_matrix()) && !a.ty.is_scalar() && !b.ty.is_scalar() {
                        return None;
                    }
                    let n = x.len().max(y.len());
                    let g = |v: &Vec<f32>, i: usize| if v.len() == 1 { v[0] } else { v[i] };
                    let mut out = Vec::with_capacity(n);
                    for i in 0..n {
                        let (p, q) = (g(&x, i), g(&y, i));
                        out.push(match op {
                            BinOp::Add => p + q,
                            BinOp::Sub => p - q,
                            BinOp::Mul => p * q,
                            _ => {
                                if int {
                                    if q == 0.0 {
                                        return None;
                                    }
                                    crate::fmath::trunc((p / q) as f64) as f32
                                } else {
                                    p / q
                                }
                            }
                        });
                    }
                    Some(out)
                }
                BinOp::Lt => Some(vec![(x[0] < y[0]) as u8 as f32]),
                BinOp::Gt => Some(vec![(x[0] > y[0]) as u8 as f32]),
                BinOp::Le => Some(vec![(x[0] <= y[0]) as u8 as f32]),
                BinOp::Ge => Some(vec![(x[0] >= y[0]) as u8 as f32]),
                BinOp::Eq => Some(vec![(x == y) as u8 as f32]),
                BinOp::Ne => Some(vec![(x != y) as u8 as f32]),
                BinOp::And => Some(vec![(x[0] != 0.0 && y[0] != 0.0) as u8 as f32]),
                BinOp::Or => Some(vec![(x[0] != 0.0 || y[0] != 0.0) as u8 as f32]),
                BinOp::Xor => Some(vec![((x[0] != 0.0) != (y[0] != 0.0)) as u8 as f32]),
            }
        }
        EK::Cond(c, a, b) => {
            let c = r(c)?;
            if c[0] != 0.0 { r(a) } else { r(b) }
        }
        EK::Swizzle(a, sel, n) => {
            let v = r(a)?;
            Some((0..*n as usize).map(|i| v[sel[i] as usize]).collect())
        }
        EK::Member(a, ix) => {
            let v = r(a)?;
            let Base::Struct(id) = a.ty.base else { return None };
            let sd = &structs[id as usize];
            let off: usize = sd.fields[..*ix as usize].iter().map(|(_, t, _)| comps(structs, t)).sum();
            let n = comps(structs, &sd.fields[*ix as usize].1);
            Some(v[off..off + n].to_vec())
        }
        EK::Index(a, i) => {
            let v = r(a)?;
            let ix = r(i)?[0] as usize;
            let elem = if a.ty.is_array() {
                comps(structs, &a.ty.elem())
            } else if a.ty.is_matrix() {
                a.ty.rows as usize
            } else {
                1
            };
            v.get(ix * elem..ix * elem + elem).map(|s| s.to_vec())
        }
        EK::Ctor(t, args) => {
            let mut flat = Vec::new();
            for a in args {
                flat.extend(r(a)?);
            }
            let conv = |x: f32| match t.base {
                Base::Int => crate::fmath::trunc(x as f64) as f32,
                Base::Bool => (x != 0.0) as u8 as f32,
                _ => x,
            };
            if let Base::Struct(_) = t.base {
                return Some(flat);
            }
            let need = t.components() as usize;
            if t.is_matrix() && args.len() == 1 && args[0].ty.is_scalar() {
                let n = t.rows as usize;
                let mut m = vec![0.0; n * n];
                for k in 0..n {
                    m[k * n + k] = flat[0];
                }
                return Some(m);
            }
            if args.len() == 1 && args[0].ty.is_scalar() {
                return Some(vec![conv(flat[0]); need]);
            }
            if flat.len() < need {
                return None;
            }
            Some(flat[..need].iter().map(|x| conv(*x)).collect())
        }
        _ => None,
    }
}
