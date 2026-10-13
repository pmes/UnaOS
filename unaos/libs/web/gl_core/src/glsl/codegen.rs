//! Code generation: the checked GLSL tree → the register bytecode of [`crate::vm`].
//!
//! Registers are allocated statically: every variable gets a block on its first reference (so a variable no
//! emitted code touches has none — that is "static use", the active-variable rule of ES 2.0 §2.10.4), every
//! expression node a fresh temporary, every literal a deduplicated constant register preloaded once per draw.
//! Layout: a matrix is its column registers, a struct its fields in order, an array its elements in order.
//! main is emitted first, then each function on its first call (reachability), each with static parameter
//! and local registers — GLSL ES §6.1 forbids recursion, so no function is ever live twice and a call is a
//! Call/Ret pair with argument copies through call-site temporaries.
//!
//! Writes go through an l-value PATH (static register offsets, dynamic array/matrix steps, a final swizzle or
//! dynamic lane): a dynamic step loads the element into a temporary (LdIdx), writes the rest of the path into
//! it, and stores it back (StIdx).

use super::ast::*;
use super::sema::{self, Checked, BV, VK};
use crate::vm::*;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A uniform leaf (a basic-typed uniform, or a basic field of a struct uniform under its WebGL name).
#[derive(Clone, Debug)]
pub struct Leaf {
    pub name: String,
    /// Basic type; `arr` > 0 for an array leaf.
    pub ty: Ty,
    pub reg: u16,
    pub prec: Prec,
    pub used: bool,
}

#[derive(Clone, Debug)]
pub struct VaryingInfo {
    pub name: String,
    pub ty: Ty,
    pub reg: Option<u16>,
    pub invariant: bool,
}

#[derive(Clone, Debug)]
pub struct AttribInfo {
    pub name: String,
    pub ty: Ty,
    pub reg: u16,
}

/// Registers of the builtin variables a stage uses.
#[derive(Clone, Debug, Default)]
pub struct Builtins {
    pub position: Option<u16>,
    pub point_size: Option<u16>,
    pub frag_coord: Option<u16>,
    pub front_facing: Option<u16>,
    pub frag_color: Option<u16>,
    pub frag_data: Option<u16>,
    pub point_coord: Option<u16>,
    pub depth_range: Option<u16>,
}

/// One stage's executable.
#[derive(Clone, Debug)]
pub struct StageCode {
    pub ops: Vec<Op>,
    pub nregs: usize,
    pub consts: Vec<(u16, [f32; 4])>,
    /// Every declared uniform, flattened; `used` = statically used.
    pub uniforms: Vec<Leaf>,
    pub attributes: Vec<AttribInfo>,
    /// Every declared varying (reg None = never referenced in this stage).
    pub varyings: Vec<VaryingInfo>,
    pub builtins: Builtins,
    /// The FS samples with implicit LOD or takes derivatives: helper invocations can serve it.
    pub implicit_lod: bool,
    pub derivatives: bool,
    pub discards: bool,
}

#[derive(Clone, Copy, Debug)]
struct V {
    r: u16,
    sw: u8,
}

impl V {
    fn src(&self) -> Src {
        Src { r: self.r, sw: self.sw }
    }
    fn reg(r: u16) -> V {
        V { r, sw: IDENT }
    }
}

#[derive(Clone, Debug)]
enum Step {
    Off(u16),
    Dyn { idx: Src, stride: u16, len: u16, n: u16 },
    Swz([u8; 4], u8),
    Lane(Src),
}

struct Gen<'a> {
    u: &'a Checked,
    ops: Vec<Op>,
    next: u32,
    var_reg: Vec<Option<u16>>,
    consts: Vec<(u16, [f32; 4])>,
    const_map: BTreeMap<[u32; 4], u16>,
    entry: Vec<Option<u32>>,
    ret_reg: Vec<Option<u16>>,
    calls: Vec<(usize, u32)>,
    queue: Vec<u32>,
    loops: Vec<(Vec<usize>, Vec<usize>)>,
    cur_fn: Option<u32>,
    errors: Vec<String>,
    implicit_lod: bool,
    derivatives: bool,
    discards: bool,
    builtins: Builtins,
}

pub fn generate(u: &Checked) -> Result<StageCode, String> {
    let mut g = Gen {
        u,
        ops: Vec::new(),
        next: 0,
        var_reg: vec![None; u.vars.len()],
        consts: Vec::new(),
        const_map: BTreeMap::new(),
        entry: vec![None; u.funcs.len()],
        ret_reg: vec![None; u.funcs.len()],
        calls: Vec::new(),
        queue: Vec::new(),
        loops: Vec::new(),
        cur_fn: None,
        errors: Vec::new(),
        implicit_lod: false,
        derivatives: false,
        discards: false,
        builtins: Builtins::default(),
    };
    let Some(main) = u.main else { return Err("missing main() function".into()) };
    let Some(body) = u.funcs[main as usize].body.as_ref() else { return Err("missing main() function body".into()) };
    for (id, init) in &u.global_inits {
        let ty = u.vars[*id as usize].ty;
        let v = g.ex(init);
        let r = g.var_reg(*id);
        g.copy_val(r, v, &ty);
    }
    for s in body {
        g.stmt(s);
    }
    g.emit(Op::Ret);
    while let Some(fid) = g.queue.pop() {
        if g.entry[fid as usize].is_some() {
            continue;
        }
        let f = &u.funcs[fid as usize];
        let Some(body) = f.body.as_ref() else {
            g.errors.push(format!("function '{}' is called but never defined", f.name));
            continue;
        };
        g.entry[fid as usize] = Some(g.ops.len() as u32);
        g.cur_fn = Some(fid);
        for s in body {
            g.stmt(s);
        }
        g.emit(Op::Ret);
    }
    for (at, fid) in core::mem::take(&mut g.calls) {
        if let Some(e) = g.entry[fid as usize] {
            g.ops[at] = Op::Call(e);
        }
    }
    if g.next > 60000 {
        g.errors.push("shader too large (register file exhausted)".into());
    }
    if !g.errors.is_empty() {
        return Err(g.errors.join("\n"));
    }
    // Interface tables.
    let mut uniforms = Vec::new();
    let mut attributes = Vec::new();
    let mut varyings = Vec::new();
    for (id, v) in u.vars.iter().enumerate() {
        let reg = g.var_reg[id];
        match v.kind {
            VK::Uniform => g.flatten(&v.name, &v.ty, reg.unwrap_or(u16::MAX), v.prec, reg.is_some(), &mut uniforms),
            VK::Attribute => {
                if let Some(r) = reg {
                    attributes.push(AttribInfo { name: v.name.clone(), ty: v.ty, reg: r });
                }
            }
            VK::Varying => varyings.push(VaryingInfo { name: v.name.clone(), ty: v.ty, reg, invariant: v.invariant }),
            _ => {}
        }
    }
    Ok(StageCode {
        ops: g.ops,
        nregs: g.next as usize,
        consts: g.consts,
        uniforms,
        attributes,
        varyings,
        builtins: g.builtins,
        implicit_lod: g.implicit_lod,
        derivatives: g.derivatives,
        discards: g.discards,
    })
}

impl Gen<'_> {
    fn emit(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    fn here(&self) -> u32 {
        self.ops.len() as u32
    }

    fn patch(&mut self, at: usize, to: u32) {
        self.ops[at] = match self.ops[at] {
            Op::Jmp(_) => Op::Jmp(to),
            Op::Jz(c, _) => Op::Jz(c, to),
            Op::Jnz(c, _) => Op::Jnz(c, to),
            o => o,
        };
    }

    fn alloc(&mut self, n: u16) -> u16 {
        let r = self.next;
        self.next += n.max(1) as u32;
        if self.next > 65000 {
            self.next = 65000;
            return 0;
        }
        r as u16
    }

    fn elem_regs(&self, t: &Ty) -> u16 {
        match t.base {
            Base::Struct(id) => self.u.structs[id as usize].fields.iter().map(|(_, ft, _)| self.regs_of(ft)).sum(),
            _ => t.cols as u16,
        }
    }

    fn regs_of(&self, t: &Ty) -> u16 {
        self.elem_regs(t) * t.arr.max(1)
    }

    fn field_off(&self, sid: u16, k: u16) -> u16 {
        self.u.structs[sid as usize].fields[..k as usize].iter().map(|(_, ft, _)| self.regs_of(ft)).sum()
    }

    /// Lanes held by each register of a type, in layout order.
    fn lanes(&self, t: &Ty, out: &mut Vec<u8>) {
        for _ in 0..t.arr.max(1) {
            match t.base {
                Base::Struct(id) => {
                    for (_, ft, _) in &self.u.structs[id as usize].fields {
                        self.lanes(ft, out);
                    }
                }
                _ => {
                    for _ in 0..t.cols {
                        out.push(t.rows);
                    }
                }
            }
        }
    }

    fn flatten(&self, name: &str, t: &Ty, reg: u16, prec: Prec, used: bool, out: &mut Vec<Leaf>) {
        match t.base {
            Base::Struct(id) => {
                let n = t.arr.max(1);
                let er = self.elem_regs(t);
                for i in 0..n {
                    let nm = if t.arr > 0 { format!("{name}[{i}]") } else { String::from(name) };
                    let base = reg.wrapping_add(i * er);
                    for (k, (fname, ft, fp)) in self.u.structs[id as usize].fields.iter().enumerate() {
                        let off = self.field_off(id, k as u16);
                        self.flatten(&format!("{nm}.{fname}"), ft, base.wrapping_add(off), *fp, used, out);
                    }
                }
            }
            _ => out.push(Leaf { name: String::from(name), ty: *t, reg, prec, used }),
        }
    }

    fn var_reg(&mut self, id: u32) -> u16 {
        if let Some(r) = self.var_reg[id as usize] {
            return r;
        }
        let v = &self.u.vars[id as usize];
        let n = self.regs_of(&v.ty);
        let builtin = v.builtin;
        let r = self.alloc(n);
        self.var_reg[id as usize] = Some(r);
        let b = &mut self.builtins;
        match builtin {
            Some(BV::Position) => b.position = Some(r),
            Some(BV::PointSize) => b.point_size = Some(r),
            Some(BV::FragCoord) => b.frag_coord = Some(r),
            Some(BV::FrontFacing) => b.front_facing = Some(r),
            Some(BV::FragColor) => b.frag_color = Some(r),
            Some(BV::FragData) => b.frag_data = Some(r),
            Some(BV::PointCoord) => b.point_coord = Some(r),
            Some(BV::DepthRange) => b.depth_range = Some(r),
            _ => {}
        }
        r
    }

    fn konst(&mut self, v: [f32; 4]) -> u16 {
        let key = v.map(|x| x.to_bits());
        if let Some(r) = self.const_map.get(&key) {
            return *r;
        }
        let r = self.alloc(1);
        self.consts.push((r, v));
        self.const_map.insert(key, r);
        r
    }

    fn scalar_const(&mut self, x: f32) -> V {
        V::reg(self.konst([x; 4]))
    }

    /// A constant of any type laid out in fresh registers from its scalar components.
    fn const_value(&mut self, t: &Ty, comps: &[f32]) -> V {
        let mut lanes = Vec::new();
        self.lanes(t, &mut lanes);
        if lanes.len() == 1 {
            let mut v = [0.0; 4];
            for i in 0..lanes[0] as usize {
                v[i] = comps.get(i).copied().unwrap_or(0.0);
            }
            if lanes[0] == 1 {
                v = [v[0]; 4];
            }
            return V::reg(self.konst(v));
        }
        let base = self.alloc(lanes.len() as u16);
        let mut k = 0;
        for (i, n) in lanes.iter().enumerate() {
            let mut v = [0.0; 4];
            for l in 0..*n as usize {
                v[l] = comps.get(k).copied().unwrap_or(0.0);
                k += 1;
            }
            self.consts.push((base + i as u16, v));
        }
        V::reg(base)
    }

    fn copy_val(&mut self, dst: u16, v: V, t: &Ty) {
        let n = self.regs_of(t);
        if n <= 1 {
            self.emit(Op::Mov(Dst::all(dst), v.src()));
        } else {
            for k in 0..n {
                self.emit(Op::Mov(Dst::all(dst + k), Src::reg(v.r + k)));
            }
        }
    }

    /// Copy a value into fresh registers (so later writes to its source cannot change it).
    fn fresh(&mut self, v: V, t: &Ty) -> V {
        let n = self.regs_of(t);
        let r = self.alloc(n);
        self.copy_val(r, v, t);
        V::reg(r)
    }

    /// The operand of a value of type `t` used where a value of type `want` is expected: a scalar is broadcast.
    fn opnd(&self, v: V, t: &Ty, want: &Ty) -> Src {
        if t.is_scalar() && !want.is_scalar() { v.src().bcast(0) } else { v.src() }
    }

    // ------------------------------------------------------------------------------------ l-values

    fn lpath(&mut self, e: &Expr) -> (u16, Vec<Step>) {
        match &e.k {
            EK::Var(id) => (self.var_reg(*id), Vec::new()),
            EK::Member(a, k) => {
                let (b, mut st) = self.lpath(a);
                let Base::Struct(sid) = a.ty.base else { return (b, st) };
                let off = self.field_off(sid, *k);
                st.push(Step::Off(off));
                (b, st)
            }
            EK::Swizzle(a, sel, n) => {
                let (b, mut st) = self.lpath(a);
                if let Some(Step::Swz(prev, _)) = st.last().cloned() {
                    let mut c = [0u8; 4];
                    for i in 0..*n as usize {
                        c[i] = prev[sel[i] as usize];
                    }
                    st.pop();
                    st.push(Step::Swz(c, *n));
                } else {
                    st.push(Step::Swz(*sel, *n));
                }
                (b, st)
            }
            EK::Index(a, i) => {
                let (b, mut st) = self.lpath(a);
                let at = a.ty;
                let k = sema::fold(&self.u.vars, &self.u.structs, i).map(|v| v[0] as i64);
                if let Some(k) = k {
                    let k = k.max(0) as u16;
                    if at.is_array() {
                        let er = self.elem_regs(&at);
                        st.push(Step::Off(k * er));
                    } else if at.is_matrix() {
                        st.push(Step::Off(k));
                    } else if let Some(Step::Swz(prev, _)) = st.last().cloned() {
                        st.pop();
                        st.push(Step::Swz([prev[k as usize], 0, 0, 0], 1));
                    } else {
                        st.push(Step::Swz([k as u8, 0, 0, 0], 1));
                    }
                } else {
                    let vi = self.ex(i);
                    let vi = self.fresh(vi, &Ty::INT);
                    let idx = vi.src().bcast(0);
                    if at.is_array() {
                        let er = self.elem_regs(&at);
                        st.push(Step::Dyn { idx, stride: er, len: at.arr, n: er });
                    } else if at.is_matrix() {
                        st.push(Step::Dyn { idx, stride: 1, len: at.cols as u16, n: 1 });
                    } else {
                        st.push(Step::Lane(idx));
                    }
                }
                (b, st)
            }
            _ => {
                // Not an l-value (sema refused it); write into a scratch register.
                (self.alloc(1), Vec::new())
            }
        }
    }

    fn store(&mut self, base: u16, steps: &[Step], v: V, t: &Ty) {
        let Some((first, rest)) = steps.split_first() else {
            self.copy_val(base, v, t);
            return;
        };
        match first {
            Step::Off(k) => self.store(base + k, rest, v, t),
            Step::Swz(sel, n) => {
                let mut sw = [0u8; 4];
                let mut mask = 0u8;
                for i in 0..*n as usize {
                    sw[sel[i] as usize] = v.src().lane(i);
                    mask |= 1 << sel[i];
                }
                self.emit(Op::Mov(Dst { r: base, m: mask }, Src { r: v.r, sw: swizzle(sw) }));
            }
            Step::Lane(idx) => {
                self.emit(Op::InsDyn { d: base, idx: *idx, val: v.src().bcast(0) });
            }
            Step::Dyn { idx, stride, len, n } => {
                let tmp = self.alloc(*n);
                self.emit(Op::LdIdx { d: tmp, base, idx: *idx, stride: *stride, len: *len, n: *n });
                self.store(tmp, rest, v, t);
                self.emit(Op::StIdx { base, idx: *idx, stride: *stride, len: *len, s: tmp, n: *n });
            }
        }
    }

    fn load(&mut self, base: u16, steps: &[Step]) -> V {
        let Some((first, rest)) = steps.split_first() else { return V::reg(base) };
        match first {
            Step::Off(k) => self.load(base + k, rest),
            Step::Swz(sel, n) => {
                let mut sw = [0u8; 4];
                for i in 0..4 {
                    sw[i] = sel[i.min(*n as usize - 1)];
                }
                V { r: base, sw: swizzle(sw) }
            }
            Step::Lane(idx) => {
                let t = self.alloc(1);
                self.emit(Op::ExtDyn(Dst::all(t), Src::reg(base), *idx));
                V::reg(t)
            }
            Step::Dyn { idx, stride, len, n } => {
                let tmp = self.alloc(*n);
                self.emit(Op::LdIdx { d: tmp, base, idx: *idx, stride: *stride, len: *len, n: *n });
                self.load(tmp, rest)
            }
        }
    }

    // ------------------------------------------------------------------------------------ arithmetic

    fn arith(&mut self, op: Bin, a: V, at: &Ty, b: V, bt: &Ty, rt: &Ty) -> V {
        if op == Bin::Mul && at.is_matrix() && bt.is_matrix() {
            let n = rt.cols as u16;
            let t = self.alloc(n);
            for c in 0..n {
                self.mat_vec(t + c, a.r, at.cols, Src::reg(b.r + c));
            }
            return V::reg(t);
        }
        if op == Bin::Mul && at.is_matrix() && bt.is_vector() {
            let t = self.alloc(1);
            self.mat_vec(t, a.r, at.cols, b.src());
            return V::reg(t);
        }
        if op == Bin::Mul && at.is_vector() && bt.is_matrix() {
            let t = self.alloc(1);
            for c in 0..bt.cols {
                self.emit(Op::Dot(at.rows, Dst::lane(t, c), a.src(), Src::reg(b.r + c as u16)));
            }
            return V::reg(t);
        }
        if rt.is_matrix() {
            let n = rt.cols as u16;
            let t = self.alloc(n);
            for c in 0..n {
                let sa = if at.is_matrix() { Src::reg(a.r + c) } else { a.src().bcast(0) };
                let sb = if bt.is_matrix() { Src::reg(b.r + c) } else { b.src().bcast(0) };
                self.emit(Op::Bin(op, Dst::all(t + c), sa, sb));
            }
            return V::reg(t);
        }
        let t = self.alloc(1);
        let sa = self.opnd(a, at, rt);
        let sb = self.opnd(b, bt, rt);
        self.emit(Op::Bin(op, Dst::all(t), sa, sb));
        if op == Bin::Div && rt.base == Base::Int {
            // §5.9: integer division truncates (rounding of negative quotients is undefined; we truncate).
            self.emit(Op::Un(Un::Trunc, Dst::all(t), Src::reg(t)));
        }
        V::reg(t)
    }

    /// dst = M · v for an n-column matrix at `m`: Σ col_c · v[c] (Mul, then Mad per column).
    fn mat_vec(&mut self, dst: u16, m: u16, n: u8, v: Src) {
        self.emit(Op::Bin(Bin::Mul, Dst::all(dst), Src::reg(m), v.bcast(0)));
        for c in 1..n as usize {
            self.emit(Op::Tri(Tri::Mad, Dst::all(dst), Src::reg(m + c as u16), v.bcast(c), Src::reg(dst)));
        }
    }

    fn equal(&mut self, a: V, b: V, t: &Ty) -> V {
        let mut lanes = Vec::new();
        self.lanes(t, &mut lanes);
        let r = self.alloc(1);
        if lanes.len() == 1 {
            self.emit(Op::EqAll(lanes[0], Dst::all(r), a.src(), b.src()));
            return V::reg(r);
        }
        for (k, n) in lanes.iter().enumerate() {
            let k = k as u16;
            if k == 0 {
                self.emit(Op::EqAll(*n, Dst::all(r), Src::reg(a.r), Src::reg(b.r)));
            } else {
                let tmp = self.alloc(1);
                self.emit(Op::EqAll(*n, Dst::all(tmp), Src::reg(a.r + k), Src::reg(b.r + k)));
                self.emit(Op::Bin(Bin::And, Dst::all(r), Src::reg(r), Src::reg(tmp)));
            }
        }
        V::reg(r)
    }

    fn bin_of(op: BinOp) -> Bin {
        match op {
            BinOp::Add => Bin::Add,
            BinOp::Sub => Bin::Sub,
            BinOp::Mul => Bin::Mul,
            BinOp::Div => Bin::Div,
            BinOp::Lt => Bin::Lt,
            BinOp::Gt => Bin::Gt,
            BinOp::Le => Bin::Le,
            BinOp::Ge => Bin::Ge,
            BinOp::Eq => Bin::Eq,
            BinOp::Ne => Bin::Ne,
            BinOp::And => Bin::And,
            BinOp::Or => Bin::Or,
            BinOp::Xor => Bin::Ne,
        }
    }

    // ------------------------------------------------------------------------------------ expressions

    fn ex(&mut self, e: &Expr) -> V {
        match &e.k {
            EK::Int(v) => self.scalar_const(*v as f32),
            EK::Float(v) => self.scalar_const(*v),
            EK::Bool(v) => self.scalar_const(*v as u8 as f32),
            EK::Var(id) => {
                let var = &self.u.vars[*id as usize];
                if let Some(c) = var.cval.clone() {
                    let t = var.ty;
                    return self.const_value(&t, &c);
                }
                V::reg(self.var_reg(*id))
            }
            EK::Name(_) | EK::Call(_, _) | EK::Field(_, _) => self.scalar_const(0.0),
            EK::Comma(a, b) => {
                self.ex(a);
                self.ex(b)
            }
            EK::Bin(op, a, b) => match op {
                BinOp::And | BinOp::Or => {
                    let r = self.alloc(1);
                    let va = self.ex(a);
                    self.emit(Op::Mov(Dst::all(r), va.src().bcast(0)));
                    let j = if *op == BinOp::And { self.emit(Op::Jz(Src::reg(r), 0)) } else { self.emit(Op::Jnz(Src::reg(r), 0)) };
                    let vb = self.ex(b);
                    self.emit(Op::Mov(Dst::all(r), vb.src().bcast(0)));
                    let h = self.here();
                    self.patch(j, h);
                    V::reg(r)
                }
                BinOp::Eq | BinOp::Ne => {
                    let (va, vb) = (self.ex(a), self.ex(b));
                    let r = self.equal(va, vb, &a.ty);
                    if *op == BinOp::Ne {
                        self.emit(Op::Un(Un::Not, Dst::all(r.r), Src::reg(r.r)));
                    }
                    r
                }
                _ => {
                    let (va, vb) = (self.ex(a), self.ex(b));
                    self.arith(Self::bin_of(*op), va, &a.ty, vb, &b.ty, &e.ty)
                }
            },
            EK::Un(op, a) => match op {
                UnOp::Pos => self.ex(a),
                UnOp::Neg | UnOp::Not => {
                    let va = self.ex(a);
                    let f = if *op == UnOp::Neg { Un::Neg } else { Un::Not };
                    let n = self.regs_of(&a.ty);
                    let t = self.alloc(n);
                    if n == 1 {
                        self.emit(Op::Un(f, Dst::all(t), va.src()));
                    } else {
                        for k in 0..n {
                            self.emit(Op::Un(f, Dst::all(t + k), Src::reg(va.r + k)));
                        }
                    }
                    V::reg(t)
                }
                UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec => {
                    let ty = a.ty;
                    let (base, steps) = self.lpath(a);
                    let cur = self.load(base, &steps);
                    let old = self.fresh(cur, &ty);
                    let one = self.scalar_const(1.0);
                    let f = if matches!(op, UnOp::PreInc | UnOp::PostInc) { Bin::Add } else { Bin::Sub };
                    let n = self.regs_of(&ty);
                    let nv = self.alloc(n);
                    for k in 0..n {
                        self.emit(Op::Bin(f, Dst::all(nv + k), Src::reg(old.r + k), one.src()));
                    }
                    self.store(base, &steps, V::reg(nv), &ty);
                    if matches!(op, UnOp::PreInc | UnOp::PreDec) { V::reg(nv) } else { old }
                }
            },
            EK::Assign(op, a, b) => {
                let ty = a.ty;
                let vb = self.ex(b);
                let (base, steps) = self.lpath(a);
                let val = match op {
                    AsgOp::Set => vb,
                    _ => {
                        let cur = self.load(base, &steps);
                        let f = match op {
                            AsgOp::Add => Bin::Add,
                            AsgOp::Sub => Bin::Sub,
                            AsgOp::Mul => Bin::Mul,
                            _ => Bin::Div,
                        };
                        self.arith(f, cur, &ty, vb, &b.ty, &ty)
                    }
                };
                self.store(base, &steps, val, &ty);
                val
            }
            EK::Cond(c, a, b) => {
                let n = self.regs_of(&e.ty);
                let r = self.alloc(n);
                let vc = self.ex(c);
                let j1 = self.emit(Op::Jz(vc.src().bcast(0), 0));
                let va = self.ex(a);
                self.copy_val(r, va, &e.ty);
                let j2 = self.emit(Op::Jmp(0));
                let h = self.here();
                self.patch(j1, h);
                let vb = self.ex(b);
                self.copy_val(r, vb, &e.ty);
                let h = self.here();
                self.patch(j2, h);
                V::reg(r)
            }
            EK::Index(a, i) => {
                let at = a.ty;
                let va = self.ex(a);
                let k = sema::fold(&self.u.vars, &self.u.structs, i).map(|v| v[0] as i64);
                match k {
                    Some(k) => {
                        let k = k.max(0) as u16;
                        if at.is_array() {
                            let er = self.elem_regs(&at);
                            V::reg(va.r + k * er)
                        } else if at.is_matrix() {
                            V::reg(va.r + k)
                        } else {
                            V { r: va.r, sw: va.src().bcast(k as usize).sw }
                        }
                    }
                    None => {
                        let vi = self.ex(i);
                        let idx = vi.src().bcast(0);
                        if at.is_array() {
                            let er = self.elem_regs(&at);
                            let t = self.alloc(er);
                            self.emit(Op::LdIdx { d: t, base: va.r, idx, stride: er, len: at.arr, n: er });
                            V::reg(t)
                        } else if at.is_matrix() {
                            let t = self.alloc(1);
                            self.emit(Op::LdIdx { d: t, base: va.r, idx, stride: 1, len: at.cols as u16, n: 1 });
                            V::reg(t)
                        } else {
                            let t = self.alloc(1);
                            self.emit(Op::ExtDyn(Dst::all(t), va.src(), idx));
                            V::reg(t)
                        }
                    }
                }
            }
            EK::Member(a, k) => {
                let va = self.ex(a);
                let Base::Struct(sid) = a.ty.base else { return va };
                let off = self.field_off(sid, *k);
                V::reg(va.r + off)
            }
            EK::Swizzle(a, sel, n) => {
                let va = self.ex(a);
                let mut sw = [0u8; 4];
                for i in 0..4 {
                    sw[i] = va.src().lane(sel[i.min(*n as usize - 1)] as usize);
                }
                V { r: va.r, sw: swizzle(sw) }
            }
            EK::Ctor(t, args) => self.ctor(t, args),
            EK::Builtin(bf, args) => self.builtin(*bf, args, &e.ty),
            EK::UserCall(fid, args) => self.call(*fid, args),
        }
    }

    fn ctor(&mut self, t: &Ty, args: &[Expr]) -> V {
        if let Base::Struct(sid) = t.base {
            let n = self.regs_of(t);
            let r = self.alloc(n);
            for (k, a) in args.iter().enumerate() {
                let va = self.ex(a);
                let off = self.field_off(sid, k as u16);
                self.copy_val(r + off, va, &a.ty);
            }
            return V::reg(r);
        }
        // The component stream: one broadcast source per scalar component, in order.
        let mut stream: Vec<Src> = Vec::new();
        for a in args {
            let va = self.ex(a);
            let at = a.ty;
            if at.is_matrix() {
                for c in 0..at.cols as u16 {
                    for l in 0..at.rows as usize {
                        stream.push(Src::reg(va.r + c).bcast(l));
                    }
                }
            } else {
                for l in 0..at.rows as usize {
                    stream.push(va.src().bcast(l));
                }
            }
        }
        let n = t.cols as u16;
        let r = self.alloc(n);
        if t.is_matrix() {
            if args.len() == 1 && args[0].ty.is_scalar() {
                let z = self.konst([0.0; 4]);
                for c in 0..n {
                    self.emit(Op::Mov(Dst::all(r + c), Src::reg(z)));
                    self.emit(Op::Mov(Dst::lane(r + c, c as u8), stream[0]));
                }
            } else {
                let mut k = 0;
                for c in 0..n {
                    for l in 0..t.rows {
                        self.emit(Op::Mov(Dst::lane(r + c, l), stream[k]));
                        k += 1;
                    }
                }
            }
        } else if args.len() == 1 && args[0].ty.is_scalar() {
            self.emit(Op::Mov(Dst::all(r), stream[0]));
        } else {
            for l in 0..t.rows {
                self.emit(Op::Mov(Dst::lane(r, l), stream[l as usize]));
            }
        }
        let conv = match t.base {
            Base::Int if args.iter().any(|a| a.ty.base != Base::Int) => Some(Un::Trunc),
            Base::Bool if args.iter().any(|a| a.ty.base != Base::Bool) => Some(Un::ToBool),
            _ => None,
        };
        if let Some(f) = conv {
            for c in 0..n {
                self.emit(Op::Un(f, Dst::all(r + c), Src::reg(r + c)));
            }
        }
        V::reg(r)
    }

    fn builtin(&mut self, bf: Bf, args: &[Expr], rt: &Ty) -> V {
        let vals: Vec<V> = args.iter().map(|a| self.ex(a)).collect();
        let tys: Vec<Ty> = args.iter().map(|a| a.ty).collect();
        let s = |i: usize| -> Src { if tys[i].is_scalar() && !rt.is_scalar() { vals[i].src().bcast(0) } else { vals[i].src() } };
        let n = tys[0].rows;
        let t = self.alloc(self.regs_of(rt));
        let d = Dst::all(t);
        let un = |f: Un| Op::Un(f, d, s(0));
        let op = match bf {
            Bf::Radians => un(Un::Radians),
            Bf::Degrees => un(Un::Degrees),
            Bf::Sin => un(Un::Sin),
            Bf::Cos => un(Un::Cos),
            Bf::Tan => un(Un::Tan),
            Bf::Asin => un(Un::Asin),
            Bf::Acos => un(Un::Acos),
            Bf::Atan => un(Un::Atan),
            Bf::Exp => un(Un::Exp),
            Bf::Log => un(Un::Log),
            Bf::Exp2 => un(Un::Exp2),
            Bf::Log2 => un(Un::Log2),
            Bf::Sqrt => un(Un::Sqrt),
            Bf::InverseSqrt => un(Un::InvSqrt),
            Bf::Abs => un(Un::Abs),
            Bf::Sign => un(Un::Sign),
            Bf::Floor => un(Un::Floor),
            Bf::Ceil => un(Un::Ceil),
            Bf::Fract => un(Un::Fract),
            Bf::Not => un(Un::Not),
            Bf::Atan2 => Op::Bin(Bin::Atan2, d, s(0), s(1)),
            Bf::Pow => Op::Bin(Bin::Pow, d, s(0), s(1)),
            Bf::Mod => Op::Bin(Bin::Mod, d, s(0), s(1)),
            Bf::Min => Op::Bin(Bin::Min, d, s(0), s(1)),
            Bf::Max => Op::Bin(Bin::Max, d, s(0), s(1)),
            Bf::Step => Op::Bin(Bin::Step, d, s(0), s(1)),
            Bf::Clamp => Op::Tri(Tri::Clamp, d, s(0), s(1), s(2)),
            Bf::Mix => Op::Tri(Tri::Mix, d, s(0), s(1), s(2)),
            Bf::Smoothstep => Op::Tri(Tri::Smoothstep, d, s(0), s(1), s(2)),
            Bf::Length => Op::Len(n, d, s(0)),
            Bf::Distance => Op::Dist(n, d, s(0), s(1)),
            Bf::Dot => Op::Dot(n, d, s(0), s(1)),
            Bf::Normalize => Op::Norm(n, d, s(0)),
            Bf::Cross => Op::Cross(d, s(0), s(1)),
            Bf::FaceForward => Op::FaceFwd(n, d, s(0), s(1), s(2)),
            Bf::Reflect => Op::Reflect(n, d, s(0), s(1)),
            Bf::Refract => Op::Refract(n, d, s(0), s(1), vals[2].src().bcast(0)),
            Bf::LessThan => Op::Bin(Bin::Lt, d, s(0), s(1)),
            Bf::LessThanEqual => Op::Bin(Bin::Le, d, s(0), s(1)),
            Bf::GreaterThan => Op::Bin(Bin::Gt, d, s(0), s(1)),
            Bf::GreaterThanEqual => Op::Bin(Bin::Ge, d, s(0), s(1)),
            Bf::Equal => Op::Bin(Bin::Eq, d, s(0), s(1)),
            Bf::NotEqual => Op::Bin(Bin::Ne, d, s(0), s(1)),
            Bf::Any => Op::Any(n, d, s(0)),
            Bf::All => Op::All(n, d, s(0)),
            Bf::MatrixCompMult => {
                for c in 0..rt.cols as u16 {
                    self.emit(Op::Bin(Bin::Mul, Dst::all(t + c), Src::reg(vals[0].r + c), Src::reg(vals[1].r + c)));
                }
                return V::reg(t);
            }
            Bf::Texture(kind, extra, lod) => {
                if !lod && self.u.fragment {
                    self.implicit_lod = true;
                }
                Op::Tex { kind, d, coord: vals[1].src(), sampler: vals[0].src().bcast(0), extra: if extra { Some(vals[2].src().bcast(0)) } else { None }, lod }
            }
            Bf::DFdx | Bf::DFdy | Bf::Fwidth => {
                self.derivatives = true;
                let f = match bf {
                    Bf::DFdx => Deriv::Dx,
                    Bf::DFdy => Deriv::Dy,
                    _ => Deriv::Fwidth,
                };
                Op::Deriv(f, d, s(0))
            }
        };
        self.emit(op);
        V::reg(t)
    }

    fn call(&mut self, fid: u32, args: &[Expr]) -> V {
        let f = &self.u.funcs[fid as usize];
        let (pquals, ptys, params, ret) = (f.pquals.clone(), f.ptys.clone(), f.params.clone(), f.ret);
        let mut temps = Vec::new();
        let mut paths = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let q = pquals[i];
            if q != ParamQual::Out {
                let va = self.ex(a);
                temps.push(Some(self.fresh(va, &ptys[i])));
            } else {
                temps.push(None);
            }
            paths.push(if q != ParamQual::In { Some(self.lpath(a)) } else { None });
        }
        if self.entry[fid as usize].is_none() && !self.queue.contains(&fid) {
            self.queue.push(fid);
        }
        let mut pregs = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let pr = self.var_reg(*p);
            pregs.push(pr);
            if let Some(t) = temps[i] {
                self.copy_val(pr, t, &ptys[i]);
            }
        }
        let at = self.emit(Op::Call(0));
        self.calls.push((at, fid));
        for (i, p) in paths.into_iter().enumerate() {
            if let Some((base, steps)) = p {
                self.store(base, &steps, V::reg(pregs[i]), &ptys[i]);
            }
        }
        if ret == Ty::VOID {
            return self.scalar_const(0.0);
        }
        let rr = self.ret_reg(fid);
        let out = V::reg(rr);
        self.fresh(out, &ret)
    }

    fn ret_reg(&mut self, fid: u32) -> u16 {
        if let Some(r) = self.ret_reg[fid as usize] {
            return r;
        }
        let rt = self.u.funcs[fid as usize].ret;
        let n = self.regs_of(&rt);
        let r = self.alloc(n);
        self.ret_reg[fid as usize] = Some(r);
        r
    }

    // ------------------------------------------------------------------------------------ statements

    fn decls(&mut self, g: &DeclGroup) {
        for d in &g.decls {
            let var = &self.u.vars[d.id as usize];
            if var.cval.is_some() {
                continue;
            }
            let ty = var.ty;
            if let Some(init) = &d.init {
                let v = self.ex(init);
                let r = self.var_reg(d.id);
                self.copy_val(r, v, &ty);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Decl(g) => self.decls(g),
            Stmt::Expr(e) => {
                self.ex(e);
            }
            Stmt::Block(b) => {
                for x in b {
                    self.stmt(x);
                }
            }
            Stmt::If(c, a, b) => {
                let vc = self.ex(c);
                let j = self.emit(Op::Jz(vc.src().bcast(0), 0));
                self.stmt(a);
                match b {
                    Some(b) => {
                        let j2 = self.emit(Op::Jmp(0));
                        let h = self.here();
                        self.patch(j, h);
                        self.stmt(b);
                        let h = self.here();
                        self.patch(j2, h);
                    }
                    None => {
                        let h = self.here();
                        self.patch(j, h);
                    }
                }
            }
            Stmt::For(init, cond, cdecl, step, body) => {
                self.stmt(init);
                let top = self.here();
                let mut jend = None;
                if let Some(c) = cond {
                    let vc = self.ex(c);
                    jend = Some(self.emit(Op::Jz(vc.src().bcast(0), 0)));
                } else if let Some(g) = cdecl {
                    self.decls(g);
                    if let Some(d) = g.decls.first() {
                        let r = self.var_reg(d.id);
                        jend = Some(self.emit(Op::Jz(Src::reg(r).bcast(0), 0)));
                    }
                }
                self.loops.push((Vec::new(), Vec::new()));
                self.stmt(body);
                let cont = self.here();
                if let Some(st) = step {
                    self.ex(st);
                }
                self.emit(Op::Jmp(top));
                let end = self.here();
                if let Some(j) = jend {
                    self.patch(j, end);
                }
                let (br, co) = self.loops.pop().unwrap();
                for b in br {
                    self.patch(b, end);
                }
                for c in co {
                    self.patch(c, cont);
                }
            }
            Stmt::While(cond, cdecl, body) => {
                let top = self.here();
                let mut jend = None;
                if let Some(c) = cond {
                    let vc = self.ex(c);
                    jend = Some(self.emit(Op::Jz(vc.src().bcast(0), 0)));
                } else if let Some(g) = cdecl {
                    self.decls(g);
                }
                self.loops.push((Vec::new(), Vec::new()));
                self.stmt(body);
                self.emit(Op::Jmp(top));
                let end = self.here();
                if let Some(j) = jend {
                    self.patch(j, end);
                }
                let (br, co) = self.loops.pop().unwrap();
                for b in br {
                    self.patch(b, end);
                }
                for c in co {
                    self.patch(c, top);
                }
            }
            Stmt::DoWhile(body, c) => {
                let top = self.here();
                self.loops.push((Vec::new(), Vec::new()));
                self.stmt(body);
                let cont = self.here();
                let vc = self.ex(c);
                self.emit(Op::Jnz(vc.src().bcast(0), top));
                let end = self.here();
                let (br, co) = self.loops.pop().unwrap();
                for b in br {
                    self.patch(b, end);
                }
                for c in co {
                    self.patch(c, cont);
                }
            }
            Stmt::Return(e, _) => {
                if let (Some(e), Some(f)) = (e, self.cur_fn) {
                    let v = self.ex(e);
                    let r = self.ret_reg(f);
                    let ty = e.ty;
                    self.copy_val(r, v, &ty);
                } else if let Some(e) = e {
                    self.ex(e);
                }
                self.emit(Op::Ret);
            }
            Stmt::Break(_) => {
                let j = self.emit(Op::Jmp(0));
                if let Some(l) = self.loops.last_mut() {
                    l.0.push(j);
                }
            }
            Stmt::Continue(_) => {
                let j = self.emit(Op::Jmp(0));
                if let Some(l) = self.loops.last_mut() {
                    l.1.push(j);
                }
            }
            Stmt::Discard(_) => {
                self.discards = true;
                self.emit(Op::Discard);
            }
            Stmt::Precision(..) | Stmt::Struct(_) | Stmt::Empty => {}
        }
    }
}
