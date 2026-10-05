//! Object internal methods (§10.1 ordinary, §10.4 exotic: Array, String, Arguments, TypedArray, module
//! namespace; Proxy in builtins::proxy) and the property-related abstract operations of §7.3.

use super::*;
use crate::builtins::proxy;

/// A property descriptor with optional fields (§6.2.6).
#[derive(Clone, Debug, Default)]
pub struct PropDesc {
    pub value: Option<Value>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub writable: Option<bool>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl PropDesc {
    pub fn data(v: Value, w: bool, e: bool, c: bool) -> PropDesc {
        PropDesc { value: Some(v), writable: Some(w), enumerable: Some(e), configurable: Some(c), ..Default::default() }
    }
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }
    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }
    pub fn from_prop(p: &Prop) -> PropDesc {
        match &p.slot {
            Slot::Data(v) => PropDesc::data(v.clone(), p.writable(), p.enumerable(), p.configurable()),
            Slot::Accessor(g, s) => PropDesc {
                get: Some(g.map(Value::Object).unwrap_or(Value::Undefined)),
                set: Some(s.map(Value::Object).unwrap_or(Value::Undefined)),
                enumerable: Some(p.enumerable()),
                configurable: Some(p.configurable()),
                ..Default::default()
            },
        }
    }
    /// A complete property from this descriptor (absent fields default to false / undefined).
    pub fn to_prop(&self) -> Prop {
        let mut flags = 0;
        if self.enumerable == Some(true) {
            flags |= E;
        }
        if self.configurable == Some(true) {
            flags |= C;
        }
        if self.is_accessor() {
            let g = self.get.as_ref().and_then(|v| v.as_object());
            let s = self.set.as_ref().and_then(|v| v.as_object());
            Prop { slot: Slot::Accessor(g, s), flags }
        } else {
            if self.writable == Some(true) {
                flags |= W;
            }
            Prop { slot: Slot::Data(self.value.clone().unwrap_or(Value::Undefined)), flags }
        }
    }
}

const SPARSE_GAP: usize = 1 << 16;

impl Vm {
    // ------------------------------------------------------------------------------------- [[GetPrototypeOf]] etc.

    pub fn get_prototype_of(&mut self, o: Obj) -> JsResult<Option<Obj>> {
        if let Kind::Proxy(_) = self.heap.get(o).kind {
            return proxy::get_prototype_of(self, o);
        }
        Ok(self.heap.get(o).proto)
    }

    pub fn set_prototype_of(&mut self, o: Obj, p: Option<Obj>) -> JsResult<bool> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::set_prototype_of(self, o, p),
            Kind::ModuleNamespace(_) => return Ok(self.heap.get(o).proto == p),
            _ => {}
        }
        // %Object.prototype% is an immutable prototype exotic object.
        if Some(o) == self.realms.iter().map(|r| r.intrinsics.object_proto).find(|x| *x == o) {
            return Ok(self.heap.get(o).proto == p);
        }
        let cur = self.heap.get(o).proto;
        if cur == p {
            return Ok(true);
        }
        if !self.heap.get(o).extensible {
            return Ok(false);
        }
        let mut q = p;
        while let Some(x) = q {
            if x == o {
                return Ok(false);
            }
            if let Kind::Proxy(_) = self.heap.get(x).kind {
                break;
            }
            q = self.heap.get(x).proto;
        }
        self.heap.get_mut(o).proto = p;
        Ok(true)
    }

    pub fn is_extensible(&mut self, o: Obj) -> JsResult<bool> {
        if let Kind::Proxy(_) = self.heap.get(o).kind {
            return proxy::is_extensible(self, o);
        }
        Ok(self.heap.get(o).extensible)
    }

    pub fn prevent_extensions(&mut self, o: Obj) -> JsResult<bool> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::prevent_extensions(self, o),
            Kind::TypedArray(_) => {
                // A length-tracking / resizable-backed typed array cannot be made non-extensible.
                if !crate::builtins::typedarray::is_fixed_length(self, o) {
                    return Ok(false);
                }
            }
            _ => {}
        }
        self.heap.get_mut(o).extensible = false;
        Ok(true)
    }

    // ------------------------------------------------------------------------------------- [[GetOwnProperty]]

    pub fn get_own_property(&mut self, o: Obj, key: &PropertyKey) -> JsResult<Option<PropDesc>> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::get_own_property(self, o, key),
            Kind::ModuleNamespace(_) => return module::ns_get_own_property(self, o, key),
            Kind::TypedArray(_) => {
                if let Some(idx) = crate::builtins::typedarray::canonical_index(key) {
                    return Ok(crate::builtins::typedarray::ta_get_index(self, o, idx).map(|v| PropDesc::data(v, true, true, true)));
                }
            }
            _ => {}
        }
        Ok(self.ordinary_get_own(o, key).map(|p| PropDesc::from_prop(&p)))
    }

    /// The own property of an ordinary-ish object (Array / String / Arguments handled), as a Prop.
    pub fn ordinary_get_own(&self, o: Obj, key: &PropertyKey) -> Option<Prop> {
        let d = self.heap.get(o);
        match &d.kind {
            Kind::Array(a) => match key {
                PropertyKey::Index(i) if a.dense => {
                    return match a.elems.get(*i as usize) {
                        Some(v) if !v.is_empty() => Some(Prop::data(v.clone(), WEC)),
                        _ => None,
                    };
                }
                PropertyKey::Str(s) if s.eq_str("length") => {
                    return Some(Prop::data(Value::Number(a.length() as f64), if a.len_writable { W } else { 0 }));
                }
                _ => {}
            },
            Kind::String(s) => match key {
                PropertyKey::Index(i) if (*i as usize) < s.len() => {
                    return Some(Prop::data(Value::String(s.slice(*i as usize, *i as usize + 1)), E));
                }
                _ => {}
            },
            Kind::Arguments(a) => {
                if let PropertyKey::Index(i) = key {
                    if let Some(Some(slot)) = a.map.get(*i as usize) {
                        if let Some(p) = d.props.get(key) {
                            let mut p = p.clone();
                            if let Slot::Data(_) = p.slot {
                                p.slot = Slot::Data(self.env_slot(a.env.unwrap(), *slot));
                            }
                            return Some(p);
                        }
                    }
                }
            }
            _ => {}
        }
        d.props.get(key).cloned()
    }

    // ------------------------------------------------------------------------------------- [[DefineOwnProperty]]

    pub fn define_own_property(&mut self, o: Obj, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::define_own_property(self, o, key, desc),
            Kind::Array(_) => return self.array_define_own(o, key, desc),
            Kind::ModuleNamespace(_) => return module::ns_define_own_property(self, o, key, desc),
            Kind::TypedArray(_) => {
                if let Some(idx) = crate::builtins::typedarray::canonical_index(&key) {
                    return crate::builtins::typedarray::ta_define_index(self, o, idx, desc);
                }
            }
            Kind::String(s) => {
                if let PropertyKey::Index(i) = key {
                    if (i as usize) < s.len() {
                        let cur = self.ordinary_get_own(o, &key);
                        return Ok(self.validate_and_apply(None, &key, true, &desc, cur));
                    }
                }
            }
            Kind::Arguments(_) => return self.arguments_define_own(o, key, desc),
            _ => {}
        }
        let cur = self.ordinary_get_own(o, &key);
        let ext = self.heap.get(o).extensible;
        Ok(self.validate_and_apply(Some(o), &key, ext, &desc, cur))
    }

    /// ValidateAndApplyPropertyDescriptor (§10.1.6.3) on ordinary property storage.
    pub fn validate_and_apply(&mut self, o: Option<Obj>, key: &PropertyKey, extensible: bool, desc: &PropDesc, current: Option<Prop>) -> bool {
        let cur = match current {
            None => {
                if !extensible {
                    return false;
                }
                if let Some(o) = o {
                    let p = desc.to_prop();
                    self.heap.get_mut(o).props.insert(key.clone(), p);
                }
                return true;
            }
            Some(c) => c,
        };
        if desc.is_generic() && desc.enumerable.is_none() && desc.configurable.is_none() {
            return true;
        }
        if !cur.configurable() {
            if desc.configurable == Some(true) {
                return false;
            }
            if let Some(e) = desc.enumerable {
                if e != cur.enumerable() {
                    return false;
                }
            }
            if !desc.is_generic() && desc.is_accessor() != cur.is_accessor() {
                return false;
            }
            match &cur.slot {
                Slot::Accessor(g, s) => {
                    if let Some(dg) = &desc.get {
                        if dg.as_object() != *g {
                            return false;
                        }
                    }
                    if let Some(ds) = &desc.set {
                        if ds.as_object() != *s {
                            return false;
                        }
                    }
                }
                Slot::Data(v) => {
                    if !cur.writable() {
                        if desc.writable == Some(true) {
                            return false;
                        }
                        if let Some(dv) = &desc.value {
                            if !dv.same_value(v) {
                                return false;
                            }
                        }
                    }
                }
            }
        }
        let o = match o {
            Some(o) => o,
            None => return true,
        };
        // Apply.
        let mut p = cur.clone();
        if desc.is_accessor() && !cur.is_accessor() {
            p = Prop { slot: Slot::Accessor(None, None), flags: cur.flags & (E | C) };
        } else if desc.is_data() && cur.is_accessor() {
            p = Prop { slot: Slot::Data(Value::Undefined), flags: cur.flags & (E | C) };
        }
        if let Some(v) = &desc.value {
            p.slot = Slot::Data(v.clone());
        }
        if let Some(g) = &desc.get {
            if let Slot::Accessor(gg, _) = &mut p.slot {
                *gg = g.as_object();
            }
        }
        if let Some(s) = &desc.set {
            if let Slot::Accessor(_, ss) = &mut p.slot {
                *ss = s.as_object();
            }
        }
        let set_flag = |f: &mut u8, bit: u8, v: Option<bool>| {
            if let Some(b) = v {
                if b {
                    *f |= bit
                } else {
                    *f &= !bit
                }
            }
        };
        set_flag(&mut p.flags, W, desc.writable);
        set_flag(&mut p.flags, E, desc.enumerable);
        set_flag(&mut p.flags, C, desc.configurable);
        if p.is_accessor() {
            p.flags &= !W;
        }
        self.heap.get_mut(o).props.insert(key.clone(), p);
        true
    }

    fn array_define_own(&mut self, o: Obj, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        match &key {
            PropertyKey::Str(s) if s.eq_str("length") => return self.array_set_length(o, desc),
            PropertyKey::Index(i) => {
                let i = *i;
                let (len, len_w, dense, ext) = match &self.heap.get(o).kind {
                    Kind::Array(a) => (a.length(), a.len_writable, a.dense, self.heap.get(o).extensible),
                    _ => unreachable!(),
                };
                if i >= len && !len_w {
                    return Ok(false);
                }
                let default_data = desc.value.is_some()
                    && !desc.is_accessor()
                    && desc.writable.unwrap_or(false)
                    && desc.enumerable.unwrap_or(false)
                    && desc.configurable.unwrap_or(false);
                if dense {
                    let elen = match &self.heap.get(o).kind {
                        Kind::Array(a) => a.elems.len(),
                        _ => 0,
                    };
                    let exists = (i as usize) < elen && !matches!(&self.heap.get(o).kind, Kind::Array(a) if a.elems[i as usize].is_empty());
                    // Updating an existing WEC element with a partial descriptor keeps it dense if attributes stay default.
                    let keeps_default = exists && !desc.is_accessor() && desc.writable != Some(false) && desc.enumerable != Some(false) && desc.configurable != Some(false);
                    if (default_data || keeps_default) && (exists || ext) && ((i as usize) < elen + SPARSE_GAP) {
                        if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                            if (i as usize) >= a.elems.len() {
                                a.elems.resize(i as usize + 1, Value::Empty);
                            }
                            if let Some(v) = desc.value {
                                a.elems[i as usize] = v;
                            }
                        }
                        return Ok(true);
                    }
                    if !exists && !ext {
                        return Ok(false);
                    }
                    self.array_make_sparse(o);
                }
                let cur = self.heap.get(o).props.get(&key).cloned();
                let ok = self.validate_and_apply(Some(o), &key, ext, &desc, cur);
                if !ok {
                    return Ok(false);
                }
                if i >= len {
                    if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                        a.len = i + 1;
                    }
                }
                Ok(true)
            }
            _ => {
                let cur = self.heap.get(o).props.get(&key).cloned();
                let ext = self.heap.get(o).extensible;
                Ok(self.validate_and_apply(Some(o), &key, ext, &desc, cur))
            }
        }
    }

    pub fn array_make_sparse(&mut self, o: Obj) {
        let elems = match &mut self.heap.get_mut(o).kind {
            Kind::Array(a) if a.dense => {
                a.dense = false;
                a.len = a.elems.len() as u32;
                core::mem::take(&mut a.elems)
            }
            _ => return,
        };
        // Index keys must precede other keys in property order; ordinary own-keys sorts them anyway.
        for (i, v) in elems.into_iter().enumerate() {
            if !v.is_empty() {
                self.heap.get_mut(o).props.insert(PropertyKey::Index(i as u32), Prop::data(v, WEC));
            }
        }
    }

    /// ArraySetLength (§10.4.2.4).
    fn array_set_length(&mut self, o: Obj, desc: PropDesc) -> JsResult<bool> {
        let (old_len, len_w) = match &self.heap.get(o).kind {
            Kind::Array(a) => (a.length(), a.len_writable),
            _ => unreachable!(),
        };
        let v = match &desc.value {
            None => {
                // Only attributes.
                if desc.configurable == Some(true) || desc.enumerable == Some(true) || desc.is_accessor() {
                    return Ok(false);
                }
                if desc.writable == Some(true) && !len_w {
                    return Ok(false);
                }
                if desc.writable == Some(false) {
                    if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                        a.len_writable = false;
                    }
                }
                return Ok(true);
            }
            Some(v) => v.clone(),
        };
        let new_len = self.to_uint32(&v)?;
        let num = self.to_number(&v)?;
        if new_len as f64 != num {
            return self.throw_range("Invalid array length");
        }
        // Re-read (ToNumber may have run user code).
        let (old_len, len_w) = match &self.heap.get(o).kind {
            Kind::Array(a) => (a.length(), a.len_writable),
            _ => (old_len, len_w),
        };
        if desc.configurable == Some(true) || desc.enumerable == Some(true) || desc.is_accessor() {
            return Ok(false);
        }
        if new_len == old_len {
            if desc.writable == Some(true) && !len_w {
                return Ok(false);
            }
            if desc.writable == Some(false) {
                if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                    a.len_writable = false;
                }
            }
            return Ok(true);
        }
        if !len_w {
            return Ok(false);
        }
        let dense = matches!(&self.heap.get(o).kind, Kind::Array(a) if a.dense);
        if dense {
            let elen = old_len as usize;
            if (new_len as usize) < elen {
                if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                    a.elems.truncate(new_len as usize);
                }
            } else if (new_len as usize) - elen < SPARSE_GAP {
                if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                    a.elems.resize(new_len as usize, Value::Empty);
                }
            } else {
                self.array_make_sparse(o);
                if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                    a.len = new_len;
                }
            }
        } else if new_len < old_len {
            // Delete indices >= new_len in descending order; stop at a non-configurable one.
            let mut idx: Vec<u32> = self.heap.get(o).props.keys().filter_map(|k| if let PropertyKey::Index(i) = k { if *i >= new_len { Some(*i) } else { None } } else { None }).collect();
            idx.sort_unstable_by(|a, b| b.cmp(a));
            for i in idx {
                let k = PropertyKey::Index(i);
                let conf = self.heap.get(o).props.get(&k).map(|p| p.configurable()).unwrap_or(true);
                if !conf {
                    if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                        a.len = i + 1;
                        if desc.writable == Some(false) {
                            a.len_writable = false;
                        }
                    }
                    return Ok(false);
                }
                self.heap.get_mut(o).props.remove(&k);
            }
            if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                a.len = new_len;
            }
        } else if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
            a.len = new_len;
        }
        if desc.writable == Some(false) {
            if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                a.len_writable = false;
            }
        }
        Ok(true)
    }

    fn arguments_define_own(&mut self, o: Obj, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        let mapped_slot = match (&self.heap.get(o).kind, &key) {
            (Kind::Arguments(a), PropertyKey::Index(i)) => a.map.get(*i as usize).copied().flatten(),
            _ => None,
        };
        let mut new_desc = desc.clone();
        if let Some(slot) = mapped_slot {
            if desc.is_data() && desc.value.is_none() && desc.writable == Some(false) {
                let env = match &self.heap.get(o).kind {
                    Kind::Arguments(a) => a.env.unwrap(),
                    _ => unreachable!(),
                };
                new_desc.value = Some(self.env_slot(env, slot));
            }
        }
        let cur = self.ordinary_get_own(o, &key);
        let ext = self.heap.get(o).extensible;
        if !self.validate_and_apply(Some(o), &key, ext, &new_desc, cur) {
            return Ok(false);
        }
        if let Some(slot) = mapped_slot {
            if desc.is_accessor() {
                self.unmap_arg(o, &key);
            } else {
                if let Some(v) = &desc.value {
                    let env = match &self.heap.get(o).kind {
                        Kind::Arguments(a) => a.env.unwrap(),
                        _ => unreachable!(),
                    };
                    self.set_env_slot(env, slot, v.clone());
                }
                if desc.writable == Some(false) {
                    self.unmap_arg(o, &key);
                }
            }
        }
        Ok(true)
    }

    fn unmap_arg(&mut self, o: Obj, key: &PropertyKey) {
        if let (Kind::Arguments(a), PropertyKey::Index(i)) = (&mut self.heap.get_mut(o).kind, key) {
            if let Some(m) = a.map.get_mut(*i as usize) {
                *m = None;
            }
        }
    }

    // ------------------------------------------------------------------------------------- [[HasProperty]] / [[Get]] / [[Set]] / [[Delete]]

    pub fn has_property(&mut self, o: Obj, key: &PropertyKey) -> JsResult<bool> {
        let mut cur = o;
        loop {
            match &self.heap.get(cur).kind {
                Kind::Proxy(_) => return proxy::has(self, cur, key),
                Kind::TypedArray(_) => {
                    if let Some(idx) = crate::builtins::typedarray::canonical_index(key) {
                        return Ok(crate::builtins::typedarray::ta_valid_index(self, cur, idx));
                    }
                }
                Kind::ModuleNamespace(_) => return module::ns_has(self, cur, key),
                _ => {}
            }
            if self.ordinary_get_own(cur, key).is_some() {
                return Ok(true);
            }
            match self.heap.get(cur).proto {
                Some(p) => cur = p,
                None => return Ok(false),
            }
        }
    }

    pub fn has_own_property(&mut self, o: Obj, key: &PropertyKey) -> JsResult<bool> {
        Ok(self.get_own_property(o, key)?.is_some())
    }

    pub fn get(&mut self, o: Obj, key: &PropertyKey) -> JsResult<Value> {
        self.get_with_receiver(o, key, &Value::Object(o))
    }

    /// [[Get]](P, Receiver)
    pub fn get_with_receiver(&mut self, o: Obj, key: &PropertyKey, receiver: &Value) -> JsResult<Value> {
        let mut cur = o;
        loop {
            match &self.heap.get(cur).kind {
                Kind::Proxy(_) => return proxy::get(self, cur, key, receiver),
                Kind::TypedArray(_) => {
                    if let Some(idx) = crate::builtins::typedarray::canonical_index(key) {
                        return Ok(crate::builtins::typedarray::ta_get_index(self, cur, idx).unwrap_or(Value::Undefined));
                    }
                }
                Kind::ModuleNamespace(_) => return module::ns_get(self, cur, key),
                _ => {}
            }
            if let Some(p) = self.ordinary_get_own(cur, key) {
                return match p.slot {
                    Slot::Data(v) => Ok(v),
                    Slot::Accessor(Some(g), _) => self.call(&Value::Object(g), receiver, &[]),
                    Slot::Accessor(None, _) => Ok(Value::Undefined),
                };
            }
            match self.heap.get(cur).proto {
                Some(p) => cur = p,
                None => return Ok(Value::Undefined),
            }
        }
    }

    /// [[Set]](P, V, Receiver) — OrdinarySet with exotic dispatch along the prototype chain.
    pub fn set(&mut self, o: Obj, key: PropertyKey, v: Value, receiver: &Value) -> JsResult<bool> {
        let mut cur = o;
        let own_desc: Option<Prop>;
        loop {
            match &self.heap.get(cur).kind {
                Kind::Proxy(_) => return proxy::set(self, cur, key, v, receiver),
                Kind::TypedArray(_) => {
                    if let Some(idx) = crate::builtins::typedarray::canonical_index(&key) {
                        // TypedArray [[Set]] (§10.4.5.5)
                        if matches!(receiver, Value::Object(r) if *r == cur) {
                            crate::builtins::typedarray::ta_set_index(self, cur, idx, &v)?;
                            return Ok(true);
                        }
                        if !crate::builtins::typedarray::ta_valid_index(self, cur, idx) {
                            return Ok(true);
                        }
                    }
                }
                Kind::ModuleNamespace(_) => return Ok(false),
                _ => {}
            }
            if let Some(p) = self.ordinary_get_own(cur, &key) {
                own_desc = Some(p);
                break;
            }
            match self.heap.get(cur).proto {
                Some(p) => cur = p,
                None => {
                    own_desc = None;
                    break;
                }
            }
        }
        match own_desc {
            Some(Prop { slot: Slot::Accessor(_, s), .. }) => match s {
                Some(s) => {
                    self.call(&Value::Object(s), receiver, &[v])?;
                    Ok(true)
                }
                None => Ok(false),
            },
            Some(p) if !p.writable() => Ok(false),
            _ => {
                // Data property (writable) found or absent: define on the receiver.
                let r = match receiver {
                    Value::Object(r) => *r,
                    _ => return Ok(false),
                };
                // Fast path: receiver owns a plain writable data property.
                if r == cur && own_desc_is_own_plain(self, r, &key) {
                    if let Some(pp) = self.heap.get_mut(r).props.get_mut(&key) {
                        pp.slot = Slot::Data(v);
                        return Ok(true);
                    }
                }
                match self.get_own_property(r, &key)? {
                    Some(ed) => {
                        if ed.is_accessor() || ed.writable == Some(false) {
                            return Ok(false);
                        }
                        self.define_own_property(r, key, PropDesc { value: Some(v), ..Default::default() })
                    }
                    None => self.define_own_property(r, key, PropDesc::data(v, true, true, true)),
                }
            }
        }
    }

    pub fn delete(&mut self, o: Obj, key: &PropertyKey) -> JsResult<bool> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::delete(self, o, key),
            Kind::ModuleNamespace(_) => return module::ns_delete(self, o, key),
            Kind::TypedArray(_) => {
                if let Some(idx) = crate::builtins::typedarray::canonical_index(key) {
                    return Ok(!crate::builtins::typedarray::ta_valid_index(self, o, idx));
                }
            }
            Kind::Array(a) => {
                if let PropertyKey::Index(i) = key {
                    if a.dense {
                        let i = *i as usize;
                        if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                            if i < a.elems.len() {
                                if i + 1 == a.elems.len() && false {
                                    a.elems.pop();
                                } else {
                                    a.elems[i] = Value::Empty;
                                }
                            }
                        }
                        return Ok(true);
                    }
                }
                if let PropertyKey::Str(s) = key {
                    if s.eq_str("length") {
                        return Ok(false);
                    }
                }
            }
            Kind::String(s) => {
                if let PropertyKey::Index(i) = key {
                    if (*i as usize) < s.len() {
                        return Ok(false);
                    }
                }
            }
            _ => {}
        }
        let conf = match self.heap.get(o).props.get(key) {
            None => return Ok(true),
            Some(p) => p.configurable(),
        };
        if !conf {
            return Ok(false);
        }
        self.heap.get_mut(o).props.remove(key);
        if let Kind::Arguments(_) = self.heap.get(o).kind {
            self.unmap_arg(o, key);
        }
        Ok(true)
    }

    /// [[OwnPropertyKeys]]: integer indices ascending, then strings, then symbols, in creation order.
    pub fn own_property_keys(&mut self, o: Obj) -> JsResult<Vec<PropertyKey>> {
        match &self.heap.get(o).kind {
            Kind::Proxy(_) => return proxy::own_keys(self, o),
            Kind::ModuleNamespace(_) => return module::ns_own_keys(self, o),
            Kind::TypedArray(_) => {
                let n = crate::builtins::typedarray::ta_length(self, o).unwrap_or(0);
                let mut out: Vec<PropertyKey> = (0..n as u32).map(PropertyKey::Index).collect();
                let d = self.heap.get(o);
                out.extend(d.props.keys().filter(|k| matches!(k, PropertyKey::Str(_))).cloned());
                out.extend(d.props.keys().filter(|k| matches!(k, PropertyKey::Sym(_))).cloned());
                return Ok(out);
            }
            _ => {}
        }
        Ok(self.ordinary_own_keys(o))
    }

    pub fn ordinary_own_keys(&self, o: Obj) -> Vec<PropertyKey> {
        let d = self.heap.get(o);
        let mut idx: Vec<u32> = Vec::new();
        match &d.kind {
            Kind::Array(a) if a.dense => {
                for (i, v) in a.elems.iter().enumerate() {
                    if !v.is_empty() {
                        idx.push(i as u32);
                    }
                }
            }
            Kind::String(s) => idx.extend(0..s.len() as u32),
            _ => {}
        }
        let mut extra: Vec<u32> = d.props.keys().filter_map(|k| if let PropertyKey::Index(i) = k { Some(*i) } else { None }).collect();
        if !extra.is_empty() {
            idx.append(&mut extra);
            idx.sort_unstable();
            idx.dedup();
        }
        let mut out: Vec<PropertyKey> = idx.into_iter().map(PropertyKey::Index).collect();
        if let Kind::Array(_) = &d.kind {
            out.push(PropertyKey::from_str("length"));
        }
        out.extend(d.props.keys().filter(|k| matches!(k, PropertyKey::Str(_))).cloned());
        out.extend(d.props.keys().filter(|k| matches!(k, PropertyKey::Sym(_))).cloned());
        out
    }

    // ------------------------------------------------------------------------------------- §7.3 helpers

    pub fn create_data_property(&mut self, o: Obj, key: PropertyKey, v: Value) -> JsResult<bool> {
        // Fast path: ordinary extensible object without the key.
        let d = self.heap.get(o);
        if let Kind::Ordinary = d.kind {
            if d.extensible && d.props.get(&key).is_none() {
                self.heap.get_mut(o).props.insert(key, Prop::data(v, WEC));
                return Ok(true);
            }
        }
        if let (Kind::Array(a), PropertyKey::Index(i)) = (&d.kind, &key) {
            if a.dense && d.extensible && (*i as usize) <= a.elems.len() && a.len_writable {
                if let Kind::Array(a) = &mut self.heap.get_mut(o).kind {
                    if (*i as usize) == a.elems.len() {
                        a.elems.push(v);
                    } else {
                        a.elems[*i as usize] = v;
                    }
                }
                return Ok(true);
            }
        }
        self.define_own_property(o, key, PropDesc::data(v, true, true, true))
    }

    pub fn create_data_property_or_throw(&mut self, o: Obj, key: PropertyKey, v: Value) -> JsResult<()> {
        if !self.create_data_property(o, key.clone(), v)? {
            return self.throw_type(&alloc::format!("Cannot define property '{}'", key.to_js_string()));
        }
        Ok(())
    }

    pub fn define_property_or_throw(&mut self, o: Obj, key: PropertyKey, d: PropDesc) -> JsResult<()> {
        if !self.define_own_property(o, key.clone(), d)? {
            return self.throw_type(&alloc::format!("Cannot redefine property: {}", key.to_js_string()));
        }
        Ok(())
    }

    pub fn delete_property_or_throw(&mut self, o: Obj, key: &PropertyKey) -> JsResult<()> {
        if !self.delete(o, key)? {
            return self.throw_type(&alloc::format!("Cannot delete property '{}'", key.to_js_string()));
        }
        Ok(())
    }

    /// Set(O, P, V, Throw)
    pub fn set_prop(&mut self, o: Obj, key: PropertyKey, v: Value, throw: bool) -> JsResult<()> {
        let ok = self.set(o, key.clone(), v, &Value::Object(o))?;
        if !ok && throw {
            return self.throw_type(&alloc::format!("Cannot assign to read only property '{}'", key.to_js_string()));
        }
        Ok(())
    }

    /// GetMethod(V, P): None for undefined / null.
    pub fn get_method(&mut self, v: &Value, key: &PropertyKey) -> JsResult<Option<Value>> {
        let f = self.get_v(v, key)?;
        if f.is_nullish() {
            return Ok(None);
        }
        if !self.is_callable(&f) {
            return self.throw_type(&alloc::format!("'{}' is not a function", key.to_js_string()));
        }
        Ok(Some(f))
    }

    pub fn invoke(&mut self, v: &Value, key: &PropertyKey, args: &[Value]) -> JsResult<Value> {
        let f = self.get_v(v, key)?;
        self.call(&f, v, args)
    }

    /// LengthOfArrayLike
    pub fn length_of(&mut self, o: Obj) -> JsResult<f64> {
        if let Kind::Array(a) = &self.heap.get(o).kind {
            return Ok(a.length() as f64);
        }
        let v = self.get(o, &PropertyKey::from_str("length"))?;
        self.to_length(&v)
    }

    /// CreateListFromArrayLike
    pub fn list_from_array_like(&mut self, v: &Value) -> JsResult<Vec<Value>> {
        let o = match v {
            Value::Object(o) => *o,
            _ => return self.throw_type("CreateListFromArrayLike called on non-object"),
        };
        let n = self.length_of(o)?;
        let mut out = Vec::with_capacity(n.min(1e6) as usize);
        for i in 0..n as u64 {
            out.push(self.get(o, &PropertyKey::from(i as u32))?);
        }
        Ok(out)
    }

    /// Elements of an internal array (spread arguments).
    pub fn array_to_list(&self, v: &Value) -> Vec<Value> {
        match v {
            Value::Object(o) => match &self.heap.get(*o).kind {
                Kind::Array(a) => a.elems.iter().map(|x| if x.is_empty() { Value::Undefined } else { x.clone() }).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// OrdinaryHasInstance (§7.3.21).
    pub fn ordinary_has_instance(&mut self, c: &Value, o: &Value) -> JsResult<bool> {
        if !self.is_callable(c) {
            return Ok(false);
        }
        let co = c.as_object().unwrap();
        if let Kind::Bound(b) = &self.heap.get(co).kind {
            let t = Value::Object(b.target);
            return self.instance_of(o, &t);
        }
        let mut cur = match o {
            Value::Object(x) => *x,
            _ => return Ok(false),
        };
        let p = self.get(co, &PropertyKey::from_str("prototype"))?;
        let p = match p {
            Value::Object(p) => p,
            _ => return self.throw_type("Function has non-object prototype in instanceof check"),
        };
        loop {
            match self.get_prototype_of(cur)? {
                None => return Ok(false),
                Some(x) => {
                    if x == p {
                        return Ok(true);
                    }
                    cur = x;
                }
            }
        }
    }

    /// InstanceofOperator (§13.10.2).
    pub fn instance_of(&mut self, v: &Value, target: &Value) -> JsResult<bool> {
        if !target.is_object() {
            return self.throw_type("Right-hand side of 'instanceof' is not an object");
        }
        let hi = self.wk.has_instance.clone();
        let h = self.get_method(target, &PropertyKey::Sym(hi))?;
        if let Some(h) = h {
            let r = self.call(&h, target, &[v.clone()])?;
            return Ok(self.to_boolean(&r));
        }
        if !self.is_callable(target) {
            return self.throw_type("Right-hand side of 'instanceof' is not callable");
        }
        self.ordinary_has_instance(target, v)
    }

    /// SpeciesConstructor (§7.3.22).
    pub fn species_constructor(&mut self, o: Obj, default: Obj) -> JsResult<Value> {
        let c = self.get(o, &PropertyKey::from_str("constructor"))?;
        if c.is_undefined() {
            return Ok(Value::Object(default));
        }
        if !c.is_object() {
            return self.throw_type("object.constructor is not an object");
        }
        let sp = self.wk.species.clone();
        let s = self.get_v(&c, &PropertyKey::Sym(sp))?;
        if s.is_nullish() {
            return Ok(Value::Object(default));
        }
        if self.is_constructor(&s) {
            return Ok(s);
        }
        self.throw_type("object.constructor[Symbol.species] is not a constructor")
    }

    /// SetFunctionName (§10.2.9).
    pub fn set_function_name(&mut self, f: Obj, key: &PropertyKey, prefix: Option<&str>) {
        let mut name = match key {
            PropertyKey::Sym(s) => {
                if s.0.private {
                    s.desc().cloned().unwrap_or_else(JsStr::empty)
                } else {
                    match s.desc() {
                        Some(d) => JsStr::from_str("[").concat(d).concat(&JsStr::from_str("]")),
                        None => JsStr::empty(),
                    }
                }
            }
            _ => key.to_js_string(),
        };
        if let Some(p) = prefix {
            name = JsStr::from_str(p).concat(&JsStr::from_str(" ")).concat(&name);
        }
        self.heap.get_mut(f).props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(name), C));
    }

    /// EnumerableOwnProperties keys (strings only).
    pub fn enumerable_own_keys(&mut self, o: Obj) -> JsResult<Vec<PropertyKey>> {
        let keys = self.own_property_keys(o)?;
        let mut out = Vec::new();
        for k in keys {
            if k.is_symbol() {
                continue;
            }
            if let Some(d) = self.get_own_property(o, &k)? {
                if d.enumerable == Some(true) {
                    out.push(k);
                }
            }
        }
        Ok(out)
    }

    /// CopyDataProperties (§7.3.25).
    pub fn copy_data_properties(&mut self, target: Obj, src: &Value, excluded: &[PropertyKey]) -> JsResult<()> {
        if src.is_nullish() {
            return Ok(());
        }
        let from = self.to_object(src)?.as_object().unwrap();
        let keys = self.own_property_keys(from)?;
        for k in keys {
            if excluded.contains(&k) {
                continue;
            }
            if let Some(d) = self.get_own_property(from, &k)? {
                if d.enumerable == Some(true) {
                    let v = self.get(from, &k)?;
                    self.create_data_property_or_throw(target, k, v)?;
                }
            }
        }
        Ok(())
    }

    /// TestIntegrityLevel / SetIntegrityLevel helpers.
    pub fn set_integrity_level(&mut self, o: Obj, frozen: bool) -> JsResult<bool> {
        if !self.prevent_extensions(o)? {
            return Ok(false);
        }
        let keys = self.own_property_keys(o)?;
        for k in keys {
            if frozen {
                if let Some(d) = self.get_own_property(o, &k)? {
                    let nd = if d.is_accessor() {
                        PropDesc { configurable: Some(false), ..Default::default() }
                    } else {
                        PropDesc { configurable: Some(false), writable: Some(false), ..Default::default() }
                    };
                    self.define_property_or_throw(o, k, nd)?;
                }
            } else {
                self.define_property_or_throw(o, k, PropDesc { configurable: Some(false), ..Default::default() })?;
            }
        }
        Ok(true)
    }

    pub fn test_integrity_level(&mut self, o: Obj, frozen: bool) -> JsResult<bool> {
        if self.is_extensible(o)? {
            return Ok(false);
        }
        let keys = self.own_property_keys(o)?;
        for k in keys {
            if let Some(d) = self.get_own_property(o, &k)? {
                if d.configurable == Some(true) {
                    return Ok(false);
                }
                if frozen && d.is_data() && d.writable == Some(true) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// ToPropertyDescriptor (§6.2.6.5).
    pub fn to_property_descriptor(&mut self, v: &Value) -> JsResult<PropDesc> {
        let o = match v {
            Value::Object(o) => *o,
            _ => return self.throw_type("Property description must be an object"),
        };
        let mut d = PropDesc::default();
        let field = |vm: &mut Vm, name: &str| -> JsResult<Option<Value>> {
            let k = PropertyKey::from_str(name);
            if vm.has_property(o, &k)? {
                Ok(Some(vm.get(o, &k)?))
            } else {
                Ok(None)
            }
        };
        if let Some(x) = field(self, "enumerable")? {
            d.enumerable = Some(self.to_boolean(&x));
        }
        if let Some(x) = field(self, "configurable")? {
            d.configurable = Some(self.to_boolean(&x));
        }
        if let Some(x) = field(self, "value")? {
            d.value = Some(x);
        }
        if let Some(x) = field(self, "writable")? {
            d.writable = Some(self.to_boolean(&x));
        }
        if let Some(x) = field(self, "get")? {
            if !x.is_undefined() && !self.is_callable(&x) {
                return self.throw_type("Getter must be a function");
            }
            d.get = Some(x);
        }
        if let Some(x) = field(self, "set")? {
            if !x.is_undefined() && !self.is_callable(&x) {
                return self.throw_type("Setter must be a function");
            }
            d.set = Some(x);
        }
        if (d.get.is_some() || d.set.is_some()) && (d.value.is_some() || d.writable.is_some()) {
            return self.throw_type("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute");
        }
        Ok(d)
    }

    /// FromPropertyDescriptor (§6.2.6.4).
    pub fn from_property_descriptor(&mut self, d: &PropDesc) -> Value {
        let o = self.new_plain_object();
        let put = |vm: &mut Vm, k: &str, v: Value| {
            vm.heap.get_mut(o).props.insert(PropertyKey::from_str(k), Prop::data(v, WEC));
        };
        if let Some(v) = &d.value {
            put(self, "value", v.clone());
        }
        if let Some(w) = d.writable {
            put(self, "writable", Value::Bool(w));
        }
        if let Some(g) = &d.get {
            put(self, "get", g.clone());
        }
        if let Some(s) = &d.set {
            put(self, "set", s.clone());
        }
        if let Some(e) = d.enumerable {
            put(self, "enumerable", Value::Bool(e));
        }
        if let Some(c) = d.configurable {
            put(self, "configurable", Value::Bool(c));
        }
        Value::Object(o)
    }

    /// ArrayCreate(len, proto)
    pub fn array_create(&mut self, len: f64, proto: Option<Obj>) -> JsResult<Obj> {
        if len > 4294967295.0 {
            return self.throw_range("Invalid array length");
        }
        let p = proto.unwrap_or(self.intr().array_proto);
        let n = len as usize;
        let ad = if n <= SPARSE_GAP {
            ArrayData { elems: alloc::vec![Value::Empty; n], dense: true, len: 0, len_writable: true }
        } else {
            ArrayData { elems: Vec::new(), dense: false, len: len as u32, len_writable: true }
        };
        Ok(self.alloc(ObjectData::new(Some(p), Kind::Array(ad))))
    }

    /// ArraySpeciesCreate (§10.4.2.3)
    pub fn array_species_create(&mut self, o: Obj, len: f64) -> JsResult<Obj> {
        if !self.is_array(&Value::Object(o))? {
            return self.array_create(len, None);
        }
        let mut c = self.get(o, &PropertyKey::from_str("constructor"))?;
        if self.is_constructor(&c) {
            let co = c.as_object().unwrap();
            let realm = self.function_realm(co)?;
            if realm != self.cur_realm && co == self.realms[realm as usize].intrinsics.array_ctor {
                c = Value::Undefined;
            }
        }
        if c.is_object() {
            let sp = self.wk.species.clone();
            c = self.get_v(&c, &PropertyKey::Sym(sp))?;
            if c.is_null() {
                c = Value::Undefined;
            }
        }
        if c.is_undefined() {
            return self.array_create(len, None);
        }
        if !self.is_constructor(&c) {
            return self.throw_type("species is not a constructor");
        }
        let r = self.construct(&c, &[Value::Number(len)], None)?;
        match r {
            Value::Object(o) => Ok(o),
            _ => self.throw_type("species constructor returned a non-object"),
        }
    }

    /// IsArray (§7.2.2)
    pub fn is_array(&mut self, v: &Value) -> JsResult<bool> {
        match v {
            Value::Object(o) => match &self.heap.get(*o).kind {
                Kind::Array(_) => Ok(true),
                Kind::Proxy(Some(p)) if !p.revoked => {
                    let t = Value::Object(p.target);
                    self.is_array(&t)
                }
                Kind::Proxy(_) => self.throw_type("Cannot perform 'IsArray' on a proxy that has been revoked"),
                _ => Ok(false),
            },
            _ => Ok(false),
        }
    }
}

fn own_desc_is_own_plain(vm: &Vm, o: Obj, key: &PropertyKey) -> bool {
    let d = vm.heap.get(o);
    if !matches!(d.kind, Kind::Ordinary | Kind::Function(_) | Kind::Native(_) | Kind::Error) {
        return false;
    }
    matches!(d.props.get(key), Some(p) if p.writable() && !p.is_accessor())
}
