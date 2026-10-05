//! Classes (§15.7 ClassDefinitionEvaluation), private names (§7.3.26–§7.3.32), `super` (§13.3.7) and
//! method definition.

use super::*;
use super::object::PropDesc;

impl Vm {
    pub fn class_data_mut(&mut self, f: Obj) -> &mut ClassData {
        match &mut self.heap.get_mut(f).kind {
            Kind::Function(fd) => fd.class.get_or_insert_with(|| Box::new(ClassData { fields: Vec::new(), private_methods: Vec::new(), statics: Vec::new() })),
            _ => panic!("not a class constructor"),
        }
    }

    /// Create the constructor F and prototype object (heritage: Empty = none, Null, or a constructor).
    pub fn class_create(&mut self, code: Rc<Code>, heritage: Value, name: Value) -> JsResult<(Obj, Obj)> {
        let intr = self.intr();
        let (proto_parent, ctor_parent) = match &heritage {
            Value::Empty => (Some(intr.object_proto), intr.function_proto),
            Value::Null => (None, intr.function_proto),
            h => {
                if !self.is_constructor(h) {
                    return self.throw_type("Class extends value is not a constructor or null");
                }
                let ho = h.as_object().unwrap();
                let pp = self.get(ho, &PropertyKey::from_str("prototype"))?;
                let pp = match pp {
                    Value::Object(o) => Some(o),
                    Value::Null => None,
                    _ => return self.throw_type("Class extends value does not have valid prototype property"),
                };
                (pp, ho)
            }
        };
        let proto = self.new_object(proto_parent);
        let fi = self.frames.len() - 1;
        let env = self.frames[fi].env;
        let script = self.frames[fi].script;
        let realm = self.cur_realm;
        let length = code.length;
        let mut d = ObjectData::new(
            Some(ctor_parent),
            Kind::Function(Box::new(FuncData {
                code,
                env,
                home: Some(proto),
                realm,
                class: Some(Box::new(ClassData { fields: Vec::new(), private_methods: Vec::new(), statics: Vec::new() })),
                script,
            })),
        );
        d.class_ctor = true;
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(length as f64), C));
        let nm = match name {
            Value::String(s) => s,
            _ => JsStr::empty(),
        };
        d.props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(nm), C));
        d.props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(proto), 0));
        let f = self.alloc(d);
        self.heap.get_mut(proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(f), WC));
        Ok((f, proto))
    }

    fn class_home(&self, ctor: Obj, is_static: bool) -> Obj {
        if is_static {
            return ctor;
        }
        match &self.heap.get(ctor).kind {
            Kind::Function(fd) => fd.home.unwrap_or(ctor),
            _ => ctor,
        }
    }

    fn set_home(&mut self, f: Obj, home: Obj) {
        if let Kind::Function(fd) = &mut self.heap.get_mut(f).kind {
            fd.home = Some(home);
        }
    }

    pub fn class_field(&mut self, ctor: Obj, key: Value, init: Value, flags: u8) -> JsResult<()> {
        let is_static = flags & 1 != 0;
        let fkey = if flags & 2 != 0 {
            match key {
                Value::Symbol(s) => FieldKey::Private(s),
                _ => return self.throw_type("invalid private name"),
            }
        } else {
            FieldKey::Prop(self.to_property_key(&key)?)
        };
        let init = init.as_object();
        if let Some(i) = init {
            let h = self.class_home(ctor, is_static);
            self.set_home(i, h);
        }
        let fd = FieldDef { key: fkey, init, anon: flags & 4 != 0 };
        let cd = self.class_data_mut(ctor);
        if is_static {
            cd.statics.push((Some(fd), None));
        } else {
            cd.fields.push(fd);
        }
        Ok(())
    }

    pub fn class_private_method(&mut self, ctor: Obj, pn: Value, f: Obj, kind: u8) -> JsResult<()> {
        let is_static = kind & 4 != 0;
        let sym = match pn {
            Value::Symbol(s) => s,
            _ => return self.throw_type("invalid private name"),
        };
        let h = self.class_home(ctor, is_static);
        self.set_home(f, h);
        let name = sym.desc().cloned().unwrap_or_else(JsStr::empty);
        let prefix = match kind & 3 {
            1 => Some("get"),
            2 => Some("set"),
            _ => None,
        };
        let key = PropertyKey::Str(name);
        self.set_function_name(f, &key, prefix);
        let elem = match kind & 3 {
            1 => PrivElem::Accessor(Some(f), None),
            2 => PrivElem::Accessor(None, Some(f)),
            _ => PrivElem::Method(f),
        };
        if is_static {
            let d = self.heap.get_mut(ctor);
            let list = d.private.get_or_insert_with(|| Box::new(Vec::new()));
            merge_private(list, sym, elem);
        } else {
            let cd = self.class_data_mut(ctor);
            merge_private(&mut cd.private_methods, sym, elem);
        }
        Ok(())
    }

    /// Run static fields and static blocks in order with this = F.
    pub fn class_finish(&mut self, ctor: Obj) -> JsResult<()> {
        let statics = core::mem::take(&mut self.class_data_mut(ctor).statics);
        // The static elements left the class record: root their functions while they run.
        let mark = self.temp_roots.len();
        self.temp_roots.push(Value::Object(ctor));
        for (fd, block) in &statics {
            if let Some(Some(i)) = fd.as_ref().map(|f| f.init) {
                self.temp_roots.push(Value::Object(i));
            }
            if let Some(b) = block {
                self.temp_roots.push(Value::Object(*b));
            }
        }
        let mut r = Ok(());
        for (fd, block) in statics {
            r = match (fd, block) {
                (Some(fd), _) => self.define_field(ctor, &fd),
                (None, Some(b)) => self.call(&Value::Object(b), &Value::Object(ctor), &[]).map(|_| ()),
                _ => Ok(()),
            };
            if r.is_err() {
                break;
            }
        }
        self.temp_roots.truncate(mark);
        r
    }

    fn define_field(&mut self, o: Obj, fd: &FieldDef) -> JsResult<()> {
        let v = match fd.init {
            Some(i) => self.call(&Value::Object(i), &Value::Object(o), &[])?,
            None => Value::Undefined,
        };
        if fd.anon {
            if let (Value::Object(fo), FieldKey::Prop(k)) = (&v, &fd.key) {
                let is_fn = self.obj_is_callable(*fo);
                let has_own_name = self.heap.get(*fo).props.get(&PropertyKey::from_str("name")).map(|p| matches!(&p.slot, Slot::Data(Value::String(s)) if !s.is_empty())).unwrap_or(false);
                if is_fn && !has_own_name {
                    self.set_function_name(*fo, k, None);
                }
            }
        }
        match &fd.key {
            FieldKey::Private(s) => self.private_field_add(o, s.clone(), v),
            FieldKey::Prop(k) => self.create_data_property_or_throw(o, k.clone(), v),
        }
    }

    /// InitializeInstanceElements(O, constructor)
    pub fn init_fields(&mut self, o: Obj, ctor: Obj) -> JsResult<()> {
        let (methods, fields) = match &self.heap.get(ctor).kind {
            Kind::Function(fd) => match &fd.class {
                Some(c) => (c.private_methods.clone(), c.fields.clone()),
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        for (s, e) in methods {
            if self.private_find(o, &Value::Symbol(s.clone())).is_some() {
                return self.throw_type("Cannot initialize private methods twice on the same object");
            }
            let d = self.heap.get_mut(o);
            d.private.get_or_insert_with(|| Box::new(Vec::new())).push((s, e));
        }
        for fd in &fields {
            self.define_field(o, fd)?;
        }
        Ok(())
    }

    fn private_field_add(&mut self, o: Obj, s: Sym, v: Value) -> JsResult<()> {
        if let Kind::Proxy(_) = self.heap.get(o).kind {
            // Private names are attached to the proxy object itself.
        }
        if self.private_find(o, &Value::Symbol(s.clone())).is_some() {
            return self.throw_type("Cannot initialize a private field twice on the same object");
        }
        let d = self.heap.get_mut(o);
        d.private.get_or_insert_with(|| Box::new(Vec::new())).push((s, PrivElem::Field(v)));
        Ok(())
    }

    pub fn private_find(&self, o: Obj, pn: &Value) -> Option<usize> {
        let s = match pn {
            Value::Symbol(s) => s,
            _ => return None,
        };
        self.heap.get(o).private.as_ref().and_then(|l| l.iter().position(|(x, _)| x == s))
    }

    pub fn private_get(&mut self, o: &Value, pn: &Value) -> JsResult<Value> {
        let ob = match o {
            Value::Object(ob) => *ob,
            _ => return self.throw_type("Cannot read private member from a non-object"),
        };
        let i = match self.private_find(ob, pn) {
            Some(i) => i,
            None => return self.throw_type("Cannot read private member from an object whose class did not declare it"),
        };
        let e = self.heap.get(ob).private.as_ref().unwrap()[i].1.clone();
        match e {
            PrivElem::Field(v) => Ok(v),
            PrivElem::Method(m) => Ok(Value::Object(m)),
            PrivElem::Accessor(Some(g), _) => self.call(&Value::Object(g), o, &[]),
            PrivElem::Accessor(None, _) => self.throw_type("'#' accessor was defined without a getter"),
        }
    }

    pub fn private_set(&mut self, o: &Value, pn: &Value, v: Value) -> JsResult<()> {
        let ob = match o {
            Value::Object(ob) => *ob,
            _ => return self.throw_type("Cannot write private member to a non-object"),
        };
        let i = match self.private_find(ob, pn) {
            Some(i) => i,
            None => return self.throw_type("Cannot write private member to an object whose class did not declare it"),
        };
        let e = self.heap.get(ob).private.as_ref().unwrap()[i].1.clone();
        match e {
            PrivElem::Field(_) => {
                self.heap.get_mut(ob).private.as_mut().unwrap()[i].1 = PrivElem::Field(v);
                Ok(())
            }
            PrivElem::Method(_) => self.throw_type("Private method is not writable"),
            PrivElem::Accessor(_, Some(s)) => {
                self.call(&Value::Object(s), o, &[v])?;
                Ok(())
            }
            PrivElem::Accessor(_, None) => self.throw_type("'#' accessor was defined without a setter"),
        }
    }

    fn home_proto(&mut self, f: &Value) -> JsResult<Option<Obj>> {
        let home = match f {
            Value::Object(fo) => match &self.heap.get(*fo).kind {
                Kind::Function(fd) => fd.home,
                _ => None,
            },
            _ => None,
        };
        match home {
            Some(h) => self.get_prototype_of(h),
            None => self.throw_syntax("'super' keyword unexpected here"),
        }
    }

    /// GetSuperBase: the [[Prototype]] of the active function's [[HomeObject]] (null throws on use).
    pub fn super_base(&mut self, f: &Value) -> JsResult<Value> {
        Ok(match self.home_proto(f)? {
            Some(p) => Value::Object(p),
            None => Value::Null,
        })
    }

    pub fn super_get(&mut self, this: &Value, base: &Value, key: &Value) -> JsResult<Value> {
        let k = self.to_property_key(key)?;
        match base {
            Value::Object(p) => self.get_with_receiver(*p, &k, this),
            _ => self.throw_type(&alloc::format!("Cannot read properties of null (reading '{}')", k.to_js_string())),
        }
    }

    pub fn super_set(&mut self, this: &Value, base: &Value, key: &Value, v: Value) -> JsResult<()> {
        let k = self.to_property_key(key)?;
        let p = match base {
            Value::Object(p) => *p,
            _ => return self.throw_type(&alloc::format!("Cannot set properties of null (setting '{}')", k.to_js_string())),
        };
        let ok = self.set(p, k.clone(), v, this)?;
        let strict = self.frames.last().map(|f| f.code.strict).unwrap_or(true);
        if !ok && strict {
            return self.throw_type(&alloc::format!("Cannot assign to read only property '{}'", k.to_js_string()));
        }
        Ok(())
    }

    /// SuperCall: construct the parent constructor with new.target.
    pub fn super_call(&mut self, f: &Value, nt: &Value, args: &[Value]) -> JsResult<Value> {
        let fo = match f {
            Value::Object(o) => *o,
            _ => return self.throw_syntax("'super' keyword unexpected here"),
        };
        let parent = self.get_prototype_of(fo)?;
        let pv = parent.map(Value::Object).unwrap_or(Value::Null);
        if !self.is_constructor(&pv) {
            return self.throw_type("Super constructor is not a constructor");
        }
        if !nt.is_object() {
            return self.throw_syntax("'super' keyword unexpected here");
        }
        self.construct(&pv, args, Some(nt))
    }

    /// Define a method / accessor on an object literal or class (with home object and name).
    pub fn define_method(&mut self, o: Obj, k: Value, f: Obj, kind: u8) -> JsResult<()> {
        let key = self.to_property_key(&k)?;
        self.set_home(f, o);
        let prefix = match kind & 3 {
            1 => Some("get"),
            2 => Some("set"),
            _ => None,
        };
        self.set_function_name(f, &key, prefix);
        let enumerable = kind & 4 != 0;
        let desc = match kind & 3 {
            1 => PropDesc { get: Some(Value::Object(f)), enumerable: Some(enumerable), configurable: Some(true), ..Default::default() },
            2 => PropDesc { set: Some(Value::Object(f)), enumerable: Some(enumerable), configurable: Some(true), ..Default::default() },
            _ => PropDesc::data(Value::Object(f), true, enumerable, true),
        };
        self.define_property_or_throw(o, key, desc)
    }
}

fn merge_private(list: &mut Vec<(Sym, PrivElem)>, s: Sym, e: PrivElem) {
    if let Some((_, existing)) = list.iter_mut().find(|(x, _)| *x == s) {
        if let (PrivElem::Accessor(g, st), PrivElem::Accessor(ng, ns)) = (existing.clone(), &e) {
            *existing = PrivElem::Accessor(ng.or(g), ns.or(st));
            return;
        }
        *existing = e;
        return;
    }
    list.push((s, e));
}
