//! Iteration (§7.4 iterator operations), for-in enumeration (§14.7.5.9), spread, template objects.

use super::*;

impl Vm {
    /// GetIterator(obj, sync): (iterator, next method)
    pub fn get_iterator(&mut self, v: &Value) -> JsResult<(Value, Value)> {
        let key = PropertyKey::Sym(self.wk.iterator.clone());
        let m = self.get_method(v, &key)?;
        let m = match m {
            Some(m) => m,
            None => return self.throw_type("object is not iterable"),
        };
        self.get_iterator_from_method(v, &m)
    }

    pub fn get_iterator_from_method(&mut self, v: &Value, m: &Value) -> JsResult<(Value, Value)> {
        let it = self.call(m, v, &[])?;
        if !it.is_object() {
            return self.throw_type("Result of the Symbol.iterator method is not an object");
        }
        let next = self.get_v(&it, &PropertyKey::from_str("next"))?;
        Ok((it, next))
    }

    pub fn get_async_iterator(&mut self, v: &Value) -> JsResult<(Value, Value)> {
        let key = PropertyKey::Sym(self.wk.async_iterator.clone());
        let m = self.get_method(v, &key)?;
        match m {
            Some(m) => {
                let it = self.call(&m, v, &[])?;
                if !it.is_object() {
                    return self.throw_type("Result of the Symbol.asyncIterator method is not an object");
                }
                let next = self.get_v(&it, &PropertyKey::from_str("next"))?;
                Ok((it, next))
            }
            None => {
                let (it, next) = self.get_iterator(v)?;
                crate::builtins::generator::create_async_from_sync(self, it, next)
            }
        }
    }

    /// IteratorStepValue: Some(value) or None when done.
    pub fn iterator_step_value(&mut self, it: &Value, next: &Value) -> JsResult<Option<Value>> {
        // Fast path: built-in array iterator over a dense array.
        let r = self.call(next, it, &[])?;
        if !r.is_object() {
            return self.throw_type("iterator result is not an object");
        }
        let d = self.get_v(&r, &PropertyKey::from_str("done"))?;
        if self.to_boolean(&d) {
            return Ok(None);
        }
        Ok(Some(self.get_v(&r, &PropertyKey::from_str("value"))?))
    }

    /// IteratorClose for a normal completion.
    pub fn iterator_close(&mut self, it: &Value) -> JsResult<()> {
        let m = self.get_method(it, &PropertyKey::from_str("return"))?;
        if let Some(m) = m {
            let r = self.call(&m, it, &[])?;
            if !r.is_object() {
                return self.throw_type("iterator.return() result is not an object");
            }
        }
        Ok(())
    }

    /// IterableToList
    pub fn iterable_to_list(&mut self, v: &Value) -> JsResult<Vec<Value>> {
        let (it, next) = self.get_iterator(v)?;
        let mut out = Vec::new();
        let mark = self.temp_roots.len();
        self.temp_roots.push(it.clone());
        self.temp_roots.push(next.clone());
        while let Some(x) = self.iterator_step_value(&it, &next)? {
            self.temp_roots.push(x.clone());
            out.push(x);
        }
        self.temp_roots.truncate(mark);
        // The caller now owns the values: inside a native they stay rooted for the rest of its call.
        if self.in_native {
            self.temp_roots.extend(out.iter().filter(|v| v.is_object()).cloned());
        }
        Ok(out)
    }

    /// Spread an iterable into an internal array (array literals, spread arguments).
    pub fn spread_into(&mut self, arr: Obj, v: Value) -> JsResult<()> {
        let (it, next) = self.get_iterator(&v)?;
        loop {
            match self.iterator_step_value(&it, &next)? {
                Some(x) => {
                    if let Kind::Array(a) = &mut self.heap.get_mut(arr).kind {
                        a.elems.push(x);
                    }
                }
                None => return Ok(()),
            }
        }
    }

    pub fn for_in_start(&mut self, v: &Value) -> JsResult<Obj> {
        let o = self.to_object(v)?.as_object().unwrap();
        let p = self.intr().object_proto;
        let e = self.alloc(ObjectData::new(
            Some(p),
            Kind::ForIn(Box::new(ForInData { object: Some(o), keys: Vec::new(), visited: alloc::collections::BTreeSet::new(), pos: 0, initialized: false })),
        ));
        Ok(e)
    }

    pub fn for_in_next(&mut self, e: Obj) -> JsResult<Option<JsStr>> {
        loop {
            let (obj, init, has_more) = match &self.heap.get(e).kind {
                Kind::ForIn(f) => (f.object, f.initialized, f.pos < f.keys.len()),
                _ => return Ok(None),
            };
            let o = match obj {
                Some(o) => o,
                None => return Ok(None),
            };
            if !init {
                let keys = self.own_property_keys(o)?;
                let ks: Vec<JsStr> = keys.into_iter().filter(|k| !k.is_symbol()).map(|k| k.to_js_string()).collect();
                if let Kind::ForIn(f) = &mut self.heap.get_mut(e).kind {
                    f.keys = ks;
                    f.pos = 0;
                    f.initialized = true;
                }
                continue;
            }
            if !has_more {
                let p = self.get_prototype_of(o)?;
                if let Kind::ForIn(f) = &mut self.heap.get_mut(e).kind {
                    f.object = p;
                    f.initialized = false;
                    f.keys.clear();
                }
                continue;
            }
            let k = match &mut self.heap.get_mut(e).kind {
                Kind::ForIn(f) => {
                    let k = f.keys[f.pos].clone();
                    f.pos += 1;
                    k
                }
                _ => return Ok(None),
            };
            let visited = match &self.heap.get(e).kind {
                Kind::ForIn(f) => f.visited.contains(&k),
                _ => true,
            };
            if visited {
                continue;
            }
            let d = self.get_own_property(o, &PropertyKey::from_js(k.clone()))?;
            let d = match d {
                Some(d) => d,
                None => continue,
            };
            if let Kind::ForIn(f) = &mut self.heap.get_mut(e).kind {
                f.visited.insert(k.clone());
            }
            if d.enumerable == Some(true) {
                return Ok(Some(k));
            }
        }
    }

    /// GetTemplateObject (§13.2.8.4), cached per realm and site.
    pub fn template_object(&mut self, k: u32) -> JsResult<Obj> {
        let fi = self.frames.len() - 1;
        let code = self.frames[fi].code.clone();
        let info = match &self.frames[fi].code.consts[k as usize] {
            crate::bytecode::Const::Template(t) => t.clone(),
            _ => unreachable!(),
        };
        if let Some((_, _, o)) = self.realm().template_cache.iter().find(|(c, s, _)| Rc::ptr_eq(c, &code) && *s == info.site) {
            return Ok(*o);
        }
        let raw: Vec<Value> = info.raw.iter().map(|s| Value::String(s.clone())).collect();
        let cooked: Vec<Value> = info.cooked.iter().map(|s| s.clone().map(Value::String).unwrap_or(Value::Undefined)).collect();
        let raw_arr = self.new_array(raw);
        let arr = self.new_array(cooked);
        self.set_integrity_level(raw_arr, true)?;
        self.heap.get_mut(arr).props.insert(PropertyKey::from_str("raw"), Prop::data(Value::Object(raw_arr), 0));
        self.set_integrity_level(arr, true)?;
        let r = self.cur_realm as usize;
        self.realms[r].template_cache.push((code, info.site, arr));
        Ok(arr)
    }
}
