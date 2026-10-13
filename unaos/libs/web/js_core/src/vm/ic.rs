//! AETHERPERF (SR118) inline caches for named property get / set sites (`Op::GetProp`, `Op::SetProp`).
//!
//! `Code` is immutable behind an `Rc` (and built by literal outside vm/), so the per-site cache slots live in the
//! VM: a direct-mapped table indexed by (code address, pc). An entry remembers its site and the site's key
//! constant (the very `JsStr`, compared by pointer), and one monomorphic case:
//!   * `Length`   — `.length` of a primitive string or an Array (the synthesized length);
//!   * `GetOwn`   — receiver of shape S has the key as a data property at slot `off`;
//!   * `GetProto` — receiver of shape S lacks the key, and the prototype chain is exactly the recorded
//!                  (object, shape) links, the last holding the key at `off` (or, `ABSENT`, ending at null);
//!   * `SetOwn`   — receiver of shape S has the key as a writable data property at `off`: overwrite the value;
//!   * `SetAdd`   — receiver (ordinary, extensible) of shape S lacks the key and so does every recorded link up
//!                  to null: append the property under S's transition `next` (OrdinarySet → CreateDataProperty).
//! Every hit re-checks the structure it depends on (shape identity, each proto link, each object's kind), so a
//! stale entry, a reused heap index or a reused code address can only miss — never answer wrongly; shapes are
//! immutable, and the entry's `Rc<Shape>` keeps the compared shape alive. Accessors, dictionary-mode maps,
//! integer-index keys and index/length-exotic objects (Proxy, TypedArray, module namespace, Array "length",
//! Arguments for stores) are never cached. A site that misses `MEGA_MISSES` times turns MEGAMORPHIC and goes
//! straight to the old slow path, which is also every miss's fallback.

use super::propmap::Shape;
use super::*;

const IC_SIZE: usize = 1024;
const MEGA_MISSES: u8 = 8;
const MAX_CHAIN: usize = 4;
const ABSENT: u32 = u32::MAX;

#[derive(Clone)]
enum IcKind {
    Empty,
    Mega,
    Length,
    GetOwn { shape: Rc<Shape>, off: u32 },
    GetProto { shape: Rc<Shape>, chain: Vec<(Obj, Rc<Shape>)>, off: u32 },
    SetOwn { shape: Rc<Shape>, off: u32 },
    SetAdd { shape: Rc<Shape>, next: Rc<Shape>, chain: Vec<(Obj, Rc<Shape>)> },
}

struct IcEntry {
    code: usize,
    pc: u32,
    misses: u8,
    is_length: bool,
    key: Option<JsStr>,
    kind: IcKind,
}

impl IcEntry {
    fn empty() -> IcEntry {
        IcEntry { code: 0, pc: 0, misses: 0, is_length: false, key: None, kind: IcKind::Empty }
    }
}

/// The VM's inline-cache slots (allocated on first use).
#[derive(Default)]
pub struct IcTable {
    entries: Vec<IcEntry>,
}

#[inline]
fn slot_of(code: usize, pc: usize) -> usize {
    let h = ((code >> 4) as u32).wrapping_mul(0x9E37_79B1) ^ (pc as u32).wrapping_mul(0x85EB_CA77);
    (h ^ (h >> 15)) as usize & (IC_SIZE - 1)
}

/// Objects whose [[Get]] / [[GetOwnProperty]] for a non-index string key is the ordinary one on `props`.
#[inline]
fn kind_ok(k: &Kind, is_length: bool) -> bool {
    match k {
        Kind::Proxy(_) | Kind::TypedArray(_) | Kind::ModuleNamespace(_) => false,
        Kind::Array(_) => !is_length,
        _ => true,
    }
}

/// Objects whose [[DefineOwnProperty]] for a non-index string key is the ordinary one on `props`.
#[inline]
fn kind_ok_set(k: &Kind, is_length: bool) -> bool {
    !matches!(k, Kind::Arguments(_)) && kind_ok(k, is_length)
}

#[inline]
fn data_at(d: &ObjectData, off: u32) -> Option<&Value> {
    match &d.props.at(off as usize).slot {
        Slot::Data(v) => Some(v),
        _ => None,
    }
}

/// Verify a recorded prototype chain starting at `first`; returns the last link's data and its proto.
fn walk_chain<'h>(heap: &'h Heap, first: Option<Obj>, chain: &[(Obj, Rc<Shape>)], is_length: bool) -> Option<(Option<&'h ObjectData>, Option<Obj>)> {
    let mut cur = first;
    let mut last = None;
    for (p, ps) in chain {
        if cur != Some(*p) {
            return None;
        }
        let pd = heap.get(*p);
        if !kind_ok(&pd.kind, is_length) || !Rc::ptr_eq(pd.props.shape()?, ps) {
            return None;
        }
        cur = pd.proto;
        last = Some(pd);
    }
    Some((last, cur))
}

fn get_hit(heap: &Heap, e: &IcEntry, recv: &Value) -> Option<Value> {
    match &e.kind {
        IcKind::Length => match recv {
            Value::String(s) => Some(Value::Number(s.len() as f64)),
            Value::Object(o) => match &heap.get(*o).kind {
                Kind::Array(a) => Some(Value::Number(a.length() as f64)),
                _ => None,
            },
            _ => None,
        },
        IcKind::GetOwn { shape, off } => {
            let o = recv.as_object()?;
            let d = heap.get(o);
            if !Rc::ptr_eq(d.props.shape()?, shape) || !kind_ok(&d.kind, e.is_length) {
                return None;
            }
            data_at(d, *off).cloned()
        }
        IcKind::GetProto { shape, chain, off } => {
            let o = recv.as_object()?;
            let d = heap.get(o);
            if !Rc::ptr_eq(d.props.shape()?, shape) || !kind_ok(&d.kind, e.is_length) {
                return None;
            }
            let (last, end) = walk_chain(heap, d.proto, chain, e.is_length)?;
            if *off == ABSENT {
                return if end.is_none() { Some(Value::Undefined) } else { None };
            }
            data_at(last?, *off).cloned()
        }
        _ => None,
    }
}

/// Structural lookup that both answers the get and describes the cache entry for it (no user code runs).
fn get_fill(heap: &Heap, recv: &Value, key: &PropertyKey, is_length: bool) -> Option<(IcKind, Value)> {
    match recv {
        Value::String(s) if is_length => Some((IcKind::Length, Value::Number(s.len() as f64))),
        Value::Object(o) => {
            let d = heap.get(*o);
            if is_length {
                if let Kind::Array(a) = &d.kind {
                    return Some((IcKind::Length, Value::Number(a.length() as f64)));
                }
            }
            if !kind_ok(&d.kind, is_length) {
                return None;
            }
            let shape = d.props.shape()?.clone();
            if let Some(off) = shape.lookup(key) {
                let v = data_at(d, off as u32)?.clone();
                return Some((IcKind::GetOwn { shape, off: off as u32 }, v));
            }
            let mut chain: Vec<(Obj, Rc<Shape>)> = Vec::new();
            let mut cur = d.proto;
            loop {
                let p = match cur {
                    None => return Some((IcKind::GetProto { shape, chain, off: ABSENT }, Value::Undefined)),
                    Some(p) => p,
                };
                if chain.len() >= MAX_CHAIN {
                    return None;
                }
                let pd = heap.get(p);
                if !kind_ok(&pd.kind, is_length) {
                    return None;
                }
                let ps = pd.props.shape()?.clone();
                let found = ps.lookup(key);
                chain.push((p, ps));
                if let Some(off) = found {
                    let v = data_at(pd, off as u32)?.clone();
                    return Some((IcKind::GetProto { shape, chain, off: off as u32 }, v));
                }
                cur = pd.proto;
            }
        }
        _ => None,
    }
}

impl Vm {
    /// The entry for site (frame `fi`, `pc`, name constant `k`), reset if it belonged to another site.
    fn ic_site(&mut self, fi: usize, pc: usize, k: u32) -> usize {
        if self.ic.entries.is_empty() {
            self.ic.entries = (0..IC_SIZE).map(|_| IcEntry::empty()).collect();
        }
        let code = &self.frames[fi].code;
        let cp = Rc::as_ptr(code) as usize;
        let name = code.str(k);
        let si = slot_of(cp, pc);
        let e = &mut self.ic.entries[si];
        let same = e.code == cp && e.pc == pc as u32 && e.key.as_ref().map(|x| JsStr::ptr_eq(x, name)).unwrap_or(false);
        if !same {
            *e = IcEntry { code: cp, pc: pc as u32, misses: 0, is_length: name.eq_str("length"), key: Some(name.clone()), kind: IcKind::Empty };
        }
        si
    }

    fn ic_miss(&mut self, si: usize) {
        let e = &mut self.ic.entries[si];
        e.misses = e.misses.saturating_add(1);
        if e.misses >= MEGA_MISSES {
            e.kind = IcKind::Mega;
        }
    }

    /// `Op::GetProp(k)` through the inline cache. Ok(Some(v)): done. Ok(None): take the slow path.
    pub(crate) fn ic_get_prop(&mut self, fi: usize, pc: usize, k: u32, recv: &Value) -> JsResult<Option<Value>> {
        let si = self.ic_site(fi, pc, k);
        let e = &self.ic.entries[si];
        match &e.kind {
            IcKind::Mega => return Ok(None),
            IcKind::Empty => {}
            _ => {
                if let Some(v) = get_hit(&self.heap, e, recv) {
                    // The slow path charges the budget once per object [[Get]] (vm/object.rs get_with_receiver).
                    if recv.is_object() {
                        self.tick()?;
                    }
                    return Ok(Some(v));
                }
                self.ic_miss(si);
                if matches!(self.ic.entries[si].kind, IcKind::Mega) {
                    return Ok(None);
                }
            }
        }
        let e = &self.ic.entries[si];
        let key = PropertyKey::from_js(e.key.clone().unwrap_or_else(JsStr::empty));
        if matches!(key, PropertyKey::Index(_)) {
            self.ic.entries[si].kind = IcKind::Mega;
            return Ok(None);
        }
        match get_fill(&self.heap, recv, &key, e.is_length) {
            Some((kind, v)) => {
                self.ic.entries[si].kind = kind;
                if recv.is_object() {
                    self.tick()?;
                }
                Ok(Some(v))
            }
            None => {
                self.ic_miss(si);
                Ok(None)
            }
        }
    }

    /// `Op::SetProp(k)` through the inline cache: true when the store was done (the old [[Set]] outcome).
    pub(crate) fn ic_set_prop(&mut self, fi: usize, pc: usize, k: u32, recv: &Value, v: &Value) -> bool {
        let o = match recv {
            Value::Object(o) => *o,
            _ => return false,
        };
        let si = self.ic_site(fi, pc, k);
        let e = &self.ic.entries[si];
        let is_length = e.is_length;
        match &e.kind {
            IcKind::Mega => return false,
            IcKind::Empty => {}
            IcKind::SetOwn { shape, off } => {
                let off = *off as usize;
                let d = self.heap.get(o);
                let ok = matches!(d.props.shape(), Some(s) if Rc::ptr_eq(s, shape)) && kind_ok_set(&d.kind, is_length) && {
                    let p = d.props.at(off);
                    p.writable() && !p.is_accessor()
                };
                if ok {
                    self.heap.get_mut(o).props.at_mut(off).slot = Slot::Data(v.clone());
                    return true;
                }
                self.ic_miss(si);
            }
            IcKind::SetAdd { shape, next, chain } => {
                let d = self.heap.get(o);
                let ok = matches!(d.kind, Kind::Ordinary)
                    && d.extensible
                    && matches!(d.props.shape(), Some(s) if Rc::ptr_eq(s, shape))
                    && matches!(walk_chain(&self.heap, d.proto, chain, is_length), Some((_, None)));
                if ok {
                    let next = next.clone();
                    let key = PropertyKey::Str(e.key.clone().unwrap_or_else(JsStr::empty));
                    self.heap.get_mut(o).props.push_shaped(next, key, Prop::data(v.clone(), WEC));
                    return true;
                }
                self.ic_miss(si);
            }
            _ => self.ic_miss(si),
        }
        if matches!(self.ic.entries[si].kind, IcKind::Mega) {
            return false;
        }
        let key = PropertyKey::from_js(self.ic.entries[si].key.clone().unwrap_or_else(JsStr::empty));
        if matches!(key, PropertyKey::Index(_)) {
            self.ic.entries[si].kind = IcKind::Mega;
            return false;
        }
        match self.set_fill(o, &key, v, is_length) {
            Some(kind) => {
                if let Some(kind) = kind {
                    self.ic.entries[si].kind = kind;
                }
                true
            }
            None => {
                self.ic_miss(si);
                false
            }
        }
    }

    /// Structural store: Some(entry-or-none) when done here, None to take the slow path (nothing changed).
    fn set_fill(&mut self, o: Obj, key: &PropertyKey, v: &Value, is_length: bool) -> Option<Option<IcKind>> {
        let d = self.heap.get(o);
        if !kind_ok_set(&d.kind, is_length) {
            return None;
        }
        let shape = d.props.shape()?.clone();
        if let Some(off) = shape.lookup(key) {
            let p = d.props.at(off);
            if !p.writable() || p.is_accessor() {
                return None;
            }
            self.heap.get_mut(o).props.at_mut(off).slot = Slot::Data(v.clone());
            return Some(Some(IcKind::SetOwn { shape, off: off as u32 }));
        }
        if !matches!(d.kind, Kind::Ordinary) || !d.extensible {
            return None;
        }
        let mut chain: Vec<(Obj, Rc<Shape>)> = Vec::new();
        let mut cur = d.proto;
        while let Some(p) = cur {
            if chain.len() >= MAX_CHAIN {
                return None;
            }
            let pd = self.heap.get(p);
            if !kind_ok(&pd.kind, is_length) {
                return None;
            }
            let ps = pd.props.shape()?.clone();
            if ps.lookup(key).is_some() {
                return None;
            }
            chain.push((p, ps));
            cur = pd.proto;
        }
        // OrdinarySet reached null with no property: CreateDataProperty on the extensible ordinary receiver.
        let props = &mut self.heap.get_mut(o).props;
        props.insert(key.clone(), Prop::data(v.clone(), WEC));
        Some(props.shape().map(|next| IcKind::SetAdd { shape, next: next.clone(), chain }))
    }
}
