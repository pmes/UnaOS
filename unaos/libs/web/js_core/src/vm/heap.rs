//! The heap: every object and environment record lives in one arena of cells addressed by `Obj`. Collection is
//! an exact mark-sweep: the VM enumerates its roots (value stack, frames, realms, job queue, host roots and the
//! native-call root stack) and every cell knows how to trace its outgoing references. WeakMap / WeakSet are
//! ephemerons; WeakRef targets and FinalizationRegistry cells are cleared / scheduled after marking.

use super::propmap::{PropMap, Slot};
use super::value::{BigInt, Obj, PropertyKey, Sym, Value};
use crate::bytecode::{Code, ScopeInfo};
use crate::string::JsStr;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;

pub type JsResult<T> = Result<T, Value>;

pub struct CallCtx {
    pub this: Value,
    pub args_base: usize,
    pub argc: usize,
    pub new_target: Value,
    pub callee: Obj,
}

pub type NativeFn = fn(&mut super::Vm, &CallCtx) -> JsResult<Value>;

pub struct NativeData {
    pub f: NativeFn,
    pub ctor: bool,
    pub slots: Vec<Value>,
    pub realm: u32,
}

#[derive(Clone)]
pub enum FieldKey {
    Prop(PropertyKey),
    Private(Sym),
}

#[derive(Clone)]
pub struct FieldDef {
    pub key: FieldKey,
    pub init: Option<Obj>,
    /// The initialiser is an anonymous function definition named at run time (computed key).
    pub anon: bool,
}

#[derive(Clone)]
pub enum PrivElem {
    Field(Value),
    Method(Obj),
    Accessor(Option<Obj>, Option<Obj>),
}

pub struct ClassData {
    pub fields: Vec<FieldDef>,
    pub private_methods: Vec<(Sym, PrivElem)>,
    /// Static fields (Some(field)) / blocks (None, block function) collected during
    /// ClassDefinitionEvaluation, run by `ClassFinish`.
    pub statics: Vec<(Option<FieldDef>, Option<Obj>)>,
}

pub struct FuncData {
    pub code: Rc<Code>,
    pub env: Option<Obj>,
    pub home: Option<Obj>,
    pub realm: u32,
    pub class: Option<Box<ClassData>>,
    /// Script / module record the function was created in (import() referrer).
    pub script: Option<Obj>,
}

pub struct BoundData {
    pub target: Obj,
    pub this: Value,
    pub args: Vec<Value>,
}

pub struct ArrayData {
    /// Dense elements (holes are `Value::Empty`); unused when `dense` is false.
    pub elems: Vec<Value>,
    pub dense: bool,
    /// The length when not dense.
    pub len: u32,
    pub len_writable: bool,
}

impl ArrayData {
    pub fn length(&self) -> u32 {
        if self.dense {
            self.elems.len() as u32
        } else {
            self.len
        }
    }
}

pub struct ArgsData {
    /// Mapped arguments: env record and the slot of each mapped parameter (None = unmapped index).
    pub env: Option<Obj>,
    pub map: Vec<Option<u32>>,
}

pub struct EnvData {
    pub parent: Option<Obj>,
    pub slots: Vec<Value>,
    pub info: Rc<ScopeInfo>,
    /// Bindings created at run time by sloppy direct eval in this (var) scope: deletable.
    pub extra: Option<Box<PropMap>>,
}

pub struct ObjEnvData {
    pub parent: Option<Obj>,
    pub object: Obj,
    pub with: bool,
}

pub struct GlobalEnvData {
    pub object: Obj,
    /// Script-level lexical declarations: Data(value or Empty), writable = mutable.
    pub lex: PropMap,
    /// VarNames: global var / function declarations created by scripts.
    pub var_names: Vec<JsStr>,
}

#[derive(Clone)]
pub struct Handler {
    pub pc: u32,
    pub sp: u32,
    pub env: Option<Obj>,
}

#[derive(Clone)]
pub struct Frame {
    pub code: Rc<Code>,
    pub pc: usize,
    pub args_base: usize,
    pub argc: usize,
    pub base: usize,
    pub func: Option<Obj>,
    pub this: Value,
    pub new_target: Value,
    pub env: Option<Obj>,
    pub handler_base: usize,
    pub realm: u32,
    /// Base-class [[Construct]]: a non-object return value yields `this`.
    pub construct: bool,
    /// The run loop that pushed this frame returns when it completes.
    pub entry: bool,
    /// Generator / async coroutine owning this frame.
    pub coroutine: Option<Obj>,
    /// Kind of the pending resumption for generators: 0 next, 1 throw, 2 return.
    pub resume_kind: u8,
    pub script: Option<Obj>,
}

pub struct SavedFrame {
    pub frame: Frame,
    pub stack: Vec<Value>,
    pub handlers: Vec<Handler>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoroKind {
    Generator,
    Async,
    AsyncGenerator,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoroState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    AwaitingReturn,
    Completed,
}

pub struct AsyncGenRequest {
    pub kind: u8,
    pub value: Value,
    pub promise: Obj,
    pub resolve: Value,
    pub reject: Value,
}

pub struct CoroData {
    pub kind: CoroKind,
    pub state: CoroState,
    pub frame: Option<Box<SavedFrame>>,
    /// Async functions: the promise and its resolving functions.
    pub promise: Option<Obj>,
    pub resolve: Value,
    pub reject: Value,
    pub queue: VecDeque<AsyncGenRequest>,
    pub realm: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

pub struct Reaction {
    pub capability: Option<(Obj, Value, Value)>,
    pub fulfill: bool,
    pub handler: Value,
}

pub struct PromiseData {
    pub state: PromiseState,
    pub result: Value,
    pub fulfill_reactions: Vec<Reaction>,
    pub reject_reactions: Vec<Reaction>,
    pub handled: bool,
}

/// Insertion-ordered hash map keyed by SameValueZero, with tombstones so live iterators observe mutations.
#[derive(Default)]
pub struct MapData {
    pub entries: Vec<Option<(Value, Value)>>,
    pub table: Vec<u32>,
    pub live: usize,
    /// Absolute index of entries[0] (leading tombstones are dropped; iterators hold absolute indices).
    pub offset: usize,
}

pub struct ForInData {
    pub object: Option<Obj>,
    pub keys: Vec<JsStr>,
    pub visited: alloc::collections::BTreeSet<JsStr>,
    pub pos: usize,
    pub initialized: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IterKind {
    Keys,
    Values,
    Entries,
}

pub enum IterData {
    Array { target: Option<Value>, index: u64, kind: IterKind },
    Map { target: Option<Obj>, index: usize, kind: IterKind },
    Set { target: Option<Obj>, index: usize, kind: IterKind },
    String { s: JsStr, pos: usize, done: bool },
    RegExpString { regexp: Obj, s: JsStr, global: bool, unicode: bool, done: bool },
    /// %WrapForValidIteratorPrototype% / iterator helpers / async-from-sync: (iterator, next method).
    Wrap { iter: Value, next: Value },
    Helper(Box<HelperData>),
    AsyncFromSync { iter: Value, next: Value, done: bool },
}

pub struct HelperData {
    pub kind: u8,
    pub iter: Value,
    pub next: Value,
    pub func: Value,
    pub counter: f64,
    pub remaining: f64,
    pub inner: Option<(Value, Value)>,
    pub state: u8,
}

pub struct BufferData {
    pub data: Vec<u8>,
    pub detached: bool,
    pub shared: bool,
    pub max_len: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TAKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float16,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TAKind {
    pub fn size(self) -> usize {
        match self {
            TAKind::Int8 | TAKind::Uint8 | TAKind::Uint8Clamped => 1,
            TAKind::Int16 | TAKind::Uint16 | TAKind::Float16 => 2,
            TAKind::Int32 | TAKind::Uint32 | TAKind::Float32 => 4,
            TAKind::Float64 | TAKind::BigInt64 | TAKind::BigUint64 => 8,
        }
    }
    pub fn is_bigint(self) -> bool {
        matches!(self, TAKind::BigInt64 | TAKind::BigUint64)
    }
}

pub struct TypedArrayData {
    pub kind: TAKind,
    pub buffer: Obj,
    pub byte_offset: usize,
    /// None = length-tracking (auto) view on a resizable buffer.
    pub length: Option<usize>,
}

pub struct DataViewData {
    pub buffer: Obj,
    pub byte_offset: usize,
    pub byte_length: Option<usize>,
}

pub struct ProxyData {
    pub target: Obj,
    pub handler: Obj,
    pub callable: bool,
    pub ctor: bool,
    pub revoked: bool,
}

pub struct WeakRefData {
    pub target: Value,
}

pub struct FinRegData {
    pub cleanup: Value,
    pub cells: Vec<(Value /*target*/, Value /*held*/, Value /*unregister token*/)>,
}

pub struct ModuleNsData {
    pub module: Obj,
    pub exports: Vec<JsStr>,
}

pub enum Kind {
    Ordinary,
    Array(ArrayData),
    Function(Box<FuncData>),
    Native(Box<NativeData>),
    Bound(Box<BoundData>),
    Error,
    Boolean(bool),
    Number(f64),
    String(JsStr),
    Symbol(Sym),
    BigInt(Rc<BigInt>),
    Arguments(Box<ArgsData>),
    Date(f64),
    RegExp(Box<crate::regexp::RegExpData>),
    Map(Box<MapData>),
    Set(Box<MapData>),
    WeakMap(Box<MapData>),
    WeakSet(Box<MapData>),
    WeakRef(Box<WeakRefData>),
    FinReg(Box<FinRegData>),
    Promise(Box<PromiseData>),
    Proxy(Option<Box<ProxyData>>),
    ArrayBuffer(Box<BufferData>),
    TypedArray(Box<TypedArrayData>),
    DataView(Box<DataViewData>),
    Coroutine(Box<CoroData>),
    ForIn(Box<ForInData>),
    Iterator(Box<IterData>),
    ModuleNamespace(Box<ModuleNsData>),
    Module(Box<crate::vm::module::ModuleRecord>),
    Env(Box<EnvData>),
    ObjEnv(Box<ObjEnvData>),
    GlobalEnv(Box<GlobalEnvData>),
    /// A revoked-proxy holder / generic internal record.
    Internal(Vec<Value>),
}

pub struct ObjectData {
    pub proto: Option<Obj>,
    pub extensible: bool,
    pub props: PropMap,
    pub kind: Kind,
    pub private: Option<Box<Vec<(Sym, PrivElem)>>>,
    /// Class constructor (functions only): calling without `new` throws.
    pub class_ctor: bool,
}

impl ObjectData {
    pub fn new(proto: Option<Obj>, kind: Kind) -> ObjectData {
        ObjectData { proto, extensible: true, props: PropMap::new(), kind, private: None, class_ctor: false }
    }

    /// Push every outgoing reference onto `out`.
    pub fn trace(&self, out: &mut Vec<Obj>) {
        fn v(x: &Value, out: &mut Vec<Obj>) {
            if let Value::Object(o) = x {
                out.push(*o);
            }
        }
        if let Some(p) = self.proto {
            out.push(p);
        }
        for (_, p) in self.props.iter() {
            match &p.slot {
                Slot::Data(x) => v(x, out),
                Slot::Accessor(g, s) => {
                    if let Some(g) = g {
                        out.push(*g);
                    }
                    if let Some(s) = s {
                        out.push(*s);
                    }
                }
            }
        }
        if let Some(pv) = &self.private {
            for (_, e) in pv.iter() {
                trace_priv(e, out);
            }
        }
        match &self.kind {
            Kind::Array(a) => a.elems.iter().for_each(|x| v(x, out)),
            Kind::Function(f) => {
                if let Some(e) = f.env {
                    out.push(e);
                }
                if let Some(h) = f.home {
                    out.push(h);
                }
                if let Some(s) = f.script {
                    out.push(s);
                }
                if let Some(c) = &f.class {
                    for fd in &c.fields {
                        if let Some(i) = fd.init {
                            out.push(i);
                        }
                    }
                    for (_, e) in &c.private_methods {
                        trace_priv(e, out);
                    }
                    for (fd, o) in &c.statics {
                        if let Some(o) = o {
                            out.push(*o);
                        }
                        if let Some(FieldDef { init: Some(i), .. }) = fd {
                            out.push(*i);
                        }
                    }
                }
                trace_code_consts(&f.code, out);
            }
            Kind::Native(n) => n.slots.iter().for_each(|x| v(x, out)),
            Kind::Bound(b) => {
                out.push(b.target);
                v(&b.this, out);
                b.args.iter().for_each(|x| v(x, out));
            }
            Kind::Arguments(a) => {
                if let Some(e) = a.env {
                    out.push(e);
                }
            }
            Kind::RegExp(r) => v(&r.last_index_cache, out),
            Kind::Map(m) | Kind::Set(m) => {
                for (k, x) in m.entries.iter().flatten() {
                    v(k, out);
                    v(x, out);
                }
            }
            Kind::WeakMap(_) | Kind::WeakSet(_) | Kind::WeakRef(_) => {} // ephemerons: handled by the collector
            Kind::FinReg(f) => {
                v(&f.cleanup, out);
                for (_, held, tok) in &f.cells {
                    v(held, out);
                    let _ = tok;
                }
            }
            Kind::Promise(p) => {
                v(&p.result, out);
                for r in p.fulfill_reactions.iter().chain(p.reject_reactions.iter()) {
                    v(&r.handler, out);
                    if let Some((o, a, b)) = &r.capability {
                        out.push(*o);
                        v(a, out);
                        v(b, out);
                    }
                }
            }
            Kind::Proxy(Some(p)) => {
                out.push(p.target);
                out.push(p.handler);
            }
            Kind::TypedArray(t) => out.push(t.buffer),
            Kind::DataView(d) => out.push(d.buffer),
            Kind::Coroutine(c) => {
                if let Some(f) = &c.frame {
                    trace_saved(f, out);
                }
                if let Some(p) = c.promise {
                    out.push(p);
                }
                v(&c.resolve, out);
                v(&c.reject, out);
                for r in &c.queue {
                    v(&r.value, out);
                    out.push(r.promise);
                    v(&r.resolve, out);
                    v(&r.reject, out);
                }
            }
            Kind::ForIn(f) => {
                if let Some(o) = f.object {
                    out.push(o);
                }
            }
            Kind::Iterator(it) => match &**it {
                IterData::Array { target: Some(t), .. } => v(t, out),
                IterData::Map { target: Some(t), .. } | IterData::Set { target: Some(t), .. } => out.push(*t),
                IterData::RegExpString { regexp, .. } => out.push(*regexp),
                IterData::Wrap { iter, next } | IterData::AsyncFromSync { iter, next, .. } => {
                    v(iter, out);
                    v(next, out);
                }
                IterData::Helper(h) => {
                    v(&h.iter, out);
                    v(&h.next, out);
                    v(&h.func, out);
                    if let Some((a, b)) = &h.inner {
                        v(a, out);
                        v(b, out);
                    }
                }
                _ => {}
            },
            Kind::ModuleNamespace(n) => out.push(n.module),
            Kind::Module(m) => m.trace(out),
            Kind::Env(e) => {
                if let Some(p) = e.parent {
                    out.push(p);
                }
                e.slots.iter().for_each(|x| v(x, out));
                if let Some(x) = &e.extra {
                    for (_, p) in x.iter() {
                        if let Slot::Data(d) = &p.slot {
                            v(d, out);
                        }
                    }
                }
            }
            Kind::ObjEnv(e) => {
                if let Some(p) = e.parent {
                    out.push(p);
                }
                out.push(e.object);
            }
            Kind::GlobalEnv(g) => {
                out.push(g.object);
                for (_, p) in g.lex.iter() {
                    if let Slot::Data(d) = &p.slot {
                        v(d, out);
                    }
                }
            }
            Kind::Internal(vs) => vs.iter().for_each(|x| v(x, out)),
            _ => {}
        }
    }
}

fn trace_priv(e: &PrivElem, out: &mut Vec<Obj>) {
    match e {
        PrivElem::Field(Value::Object(o)) => out.push(*o),
        PrivElem::Method(m) => out.push(*m),
        PrivElem::Accessor(g, s) => {
            if let Some(g) = g {
                out.push(*g);
            }
            if let Some(s) = s {
                out.push(*s);
            }
        }
        _ => {}
    }
}

fn trace_code_consts(_c: &Code, _out: &mut Vec<Obj>) {}

pub fn trace_saved(f: &SavedFrame, out: &mut Vec<Obj>) {
    trace_frame(&f.frame, out);
    for x in &f.stack {
        if let Value::Object(o) = x {
            out.push(*o);
        }
    }
    for h in &f.handlers {
        if let Some(e) = h.env {
            out.push(e);
        }
    }
}

pub fn trace_frame(f: &Frame, out: &mut Vec<Obj>) {
    if let Some(o) = f.func {
        out.push(o);
    }
    if let Value::Object(o) = &f.this {
        out.push(*o);
    }
    if let Value::Object(o) = &f.new_target {
        out.push(*o);
    }
    if let Some(e) = f.env {
        out.push(e);
    }
    if let Some(c) = f.coroutine {
        out.push(c);
    }
    if let Some(s) = f.script {
        out.push(s);
    }
}

pub struct Heap {
    pub cells: Vec<Option<ObjectData>>,
    free: Vec<u32>,
    marks: Vec<bool>,
    pub allocated: usize,
    pub threshold: usize,
    pub live: usize,
}

impl Heap {
    pub fn new() -> Heap {
        Heap { cells: Vec::new(), free: Vec::new(), marks: Vec::new(), allocated: 0, threshold: 20000, live: 0 }
    }

    pub fn alloc(&mut self, d: ObjectData) -> Obj {
        self.allocated += 1;
        self.live += 1;
        if let Some(i) = self.free.pop() {
            self.cells[i as usize] = Some(d);
            Obj(i)
        } else {
            self.cells.push(Some(d));
            Obj(self.cells.len() as u32 - 1)
        }
    }

    #[inline]
    pub fn get(&self, o: Obj) -> &ObjectData {
        self.cells[o.0 as usize].as_ref().expect("dangling object handle")
    }
    #[inline]
    pub fn get_mut(&mut self, o: Obj) -> &mut ObjectData {
        self.cells[o.0 as usize].as_mut().expect("dangling object handle")
    }

    pub fn should_collect(&self) -> bool {
        self.allocated >= self.threshold
    }

    /// Mark from `roots`, resolve ephemerons, clear weak references and sweep. Returns the FinalizationRegistry
    /// cleanup work: (registry, held value) pairs whose targets died.
    pub fn collect(&mut self, roots: &mut Vec<Obj>, weak_syms_alive: &dyn Fn(&Sym) -> bool) -> Vec<(Obj, Value)> {
        let n = self.cells.len();
        self.marks.clear();
        self.marks.resize(n, false);
        let mut stack: Vec<Obj> = core::mem::take(roots);
        let mut weak_containers: Vec<Obj> = Vec::new();
        let mut scratch: Vec<Obj> = Vec::new();
        loop {
            while let Some(o) = stack.pop() {
                let i = o.0 as usize;
                if i >= n || self.marks[i] {
                    continue;
                }
                let cell = match &self.cells[i] {
                    Some(c) => c,
                    None => continue,
                };
                self.marks[i] = true;
                if matches!(cell.kind, Kind::WeakMap(_) | Kind::WeakSet(_)) {
                    weak_containers.push(o);
                }
                cell.trace(&mut stack);
            }
            // Ephemerons: a WeakMap value is live if its key is live.
            let mut progressed = false;
            for &w in &weak_containers {
                if let Some(Kind::WeakMap(m)) = self.cells[w.0 as usize].as_ref().map(|c| &c.kind) {
                    for (k, x) in m.entries.iter().flatten() {
                        let key_live = match k {
                            Value::Object(ko) => self.marks[ko.0 as usize],
                            Value::Symbol(s) => weak_syms_alive(s),
                            _ => true,
                        };
                        if key_live {
                            if let Value::Object(xo) = x {
                                if !self.marks[xo.0 as usize] {
                                    scratch.push(*xo);
                                }
                            }
                        }
                    }
                }
            }
            if !scratch.is_empty() {
                stack.append(&mut scratch);
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        // Clear dead weak entries and collect finalization work.
        let mut cleanup = Vec::new();
        for i in 0..n {
            if !self.marks[i] {
                continue;
            }
            let marks = &self.marks;
            let dead = |x: &Value| matches!(x, Value::Object(o) if !marks[o.0 as usize]);
            let cell = self.cells[i].as_mut().unwrap();
            match &mut cell.kind {
                Kind::WeakMap(m) | Kind::WeakSet(m) => {
                    let mut changed = false;
                    for e in m.entries.iter_mut() {
                        if let Some((k, _)) = e {
                            if dead(k) {
                                *e = None;
                                m.live -= 1;
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        super::collections::rebuild_map(m);
                    }
                }
                Kind::WeakRef(w) => {
                    if dead(&w.target) {
                        w.target = Value::Undefined;
                    }
                }
                Kind::FinReg(f) => {
                    let mut k = 0;
                    while k < f.cells.len() {
                        if dead(&f.cells[k].0) {
                            let (_, held, _) = f.cells.remove(k);
                            cleanup.push((Obj(i as u32), held));
                        } else {
                            if dead(&f.cells[k].2) {
                                f.cells[k].2 = Value::Undefined;
                            }
                            k += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        // Sweep.
        let mut live = 0;
        for i in 0..n {
            if self.cells[i].is_some() {
                if self.marks[i] {
                    live += 1;
                } else {
                    self.cells[i] = None;
                    self.free.push(i as u32);
                }
            }
        }
        self.live = live;
        self.allocated = 0;
        self.threshold = (live * 2).max(20000);
        cleanup
    }
}
