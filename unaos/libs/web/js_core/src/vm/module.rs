//! Source Text Module Records (§16.2.1.6): loading through the host, linking (ResolveExport, environment
//! creation), evaluation with top-level await (InnerModuleEvaluation / async module execution), module
//! namespace exotic objects (§10.4.6), import.meta and dynamic import().

use super::interp::Completion;
use super::object::PropDesc;
use super::*;

#[derive(Debug, Clone)]
pub struct ImportEntry {
    pub request: usize,
    /// None = namespace import (`import * as ns`).
    pub import_name: Option<JsStr>,
    pub local: JsStr,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleInfo {
    pub requests: Vec<(JsStr, Vec<(JsStr, JsStr)>)>,
    /// Named / default imports (index = GetImport operand) and namespace imports.
    pub imports: Vec<ImportEntry>,
    pub local_exports: Vec<(JsStr, JsStr)>,
    /// (export name, request, import name; None = `export * as ns from`)
    pub indirect_exports: Vec<(JsStr, usize, Option<JsStr>)>,
    pub star_exports: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    New,
    Unlinked,
    Linking,
    Linked,
    Evaluating,
    EvaluatingAsync,
    Evaluated,
}

#[derive(Clone)]
pub enum ImportCell {
    Unresolved,
    Slot(Obj, u32),
    Value(Value),
}

pub struct ModuleRecord {
    pub name: JsStr,
    pub status: Status,
    pub code: Option<Rc<Code>>,
    pub info: Rc<ModuleInfo>,
    pub env: Option<Obj>,
    pub coroutine: Option<Obj>,
    pub loaded: Vec<Option<Obj>>,
    pub cells: Vec<ImportCell>,
    pub namespace: Option<Obj>,
    pub meta: Option<Obj>,
    pub error: Option<Value>,
    pub dfs_index: usize,
    pub dfs_ancestor: usize,
    pub cycle_root: Option<Obj>,
    pub has_tla: bool,
    pub async_evaluation: Option<u64>,
    pub pending_deps: usize,
    pub async_parents: Vec<Obj>,
    /// Top-level capability promise (cycle roots evaluated through Evaluate()).
    pub capability: Option<Obj>,
    pub realm: u32,
    /// Synthetic modules (JSON modules) carry their default export here.
    pub synthetic_default: Option<Value>,
}

impl ModuleRecord {
    pub fn trace(&self, out: &mut Vec<Obj>) {
        let v = |x: &Value, out: &mut Vec<Obj>| {
            if let Value::Object(o) = x {
                out.push(*o);
            }
        };
        for o in [self.env, self.coroutine, self.namespace, self.meta, self.cycle_root, self.capability].into_iter().flatten() {
            out.push(o);
        }
        for l in self.loaded.iter().flatten() {
            out.push(*l);
        }
        for c in &self.cells {
            match c {
                ImportCell::Slot(e, _) => out.push(*e),
                ImportCell::Value(x) => v(x, out),
                _ => {}
            }
        }
        if let Some(e) = &self.error {
            v(e, out);
        }
        if let Some(d) = &self.synthetic_default {
            v(d, out);
        }
        out.extend_from_slice(&self.async_parents);
    }
}

enum Resolution {
    Binding(Obj, JsStr),
    Namespace(Obj),
    Ambiguous,
    NotFound,
}

impl Vm {
    fn mr(&self, m: Obj) -> &ModuleRecord {
        match &self.heap.get(m).kind {
            Kind::Module(r) => r,
            _ => panic!("not a module record"),
        }
    }
    fn mr_mut(&mut self, m: Obj) -> &mut ModuleRecord {
        match &mut self.heap.get_mut(m).kind {
            Kind::Module(r) => r,
            _ => panic!("not a module record"),
        }
    }

    /// Parse a module's source and create its record (status Unlinked once its requests are loaded).
    pub fn module_from_source(&mut self, name: JsStr, src: &str) -> JsResult<Obj> {
        let u: Vec<u16> = src.encode_utf16().collect();
        let prog = match crate::parser::parse_module(&u) {
            Ok(p) => p,
            Err(e) => return self.throw_syntax(&e.msg),
        };
        let code = crate::compiler::compile_module(&prog);
        let info = code.module.clone().unwrap_or_default();
        let n = info.requests.len();
        let rec = ModuleRecord {
            name: name.clone(),
            status: Status::New,
            has_tla: code.is_async,
            code: Some(code),
            cells: alloc::vec![ImportCell::Unresolved; info.imports.len()],
            info,
            env: None,
            coroutine: None,
            loaded: alloc::vec![None; n],
            namespace: None,
            meta: None,
            error: None,
            dfs_index: 0,
            dfs_ancestor: 0,
            cycle_root: None,
            async_evaluation: None,
            pending_deps: 0,
            async_parents: Vec::new(),
            capability: None,
            realm: self.cur_realm,
            synthetic_default: None,
        };
        let m = self.heap.alloc(ObjectData::new(None, Kind::Module(Box::new(rec))));
        self.modules.push((name, m));
        Ok(m)
    }

    /// A synthetic module whose default export is `v` (JSON modules).
    pub fn synthetic_module(&mut self, name: JsStr, v: Value) -> Obj {
        let rec = ModuleRecord {
            name: name.clone(),
            status: Status::Unlinked,
            has_tla: false,
            code: None,
            cells: Vec::new(),
            info: Rc::new(ModuleInfo { local_exports: alloc::vec![(JsStr::from_str("default"), JsStr::from_str("default"))], ..Default::default() }),
            env: None,
            coroutine: None,
            loaded: Vec::new(),
            namespace: None,
            meta: None,
            error: None,
            dfs_index: 0,
            dfs_ancestor: 0,
            cycle_root: None,
            async_evaluation: None,
            pending_deps: 0,
            async_parents: Vec::new(),
            capability: None,
            realm: self.cur_realm,
            synthetic_default: Some(v),
        };
        let m = self.heap.alloc(ObjectData::new(None, Kind::Module(Box::new(rec))));
        self.modules.push((name, m));
        m
    }

    /// HostLoadImportedModule (synchronous host): load `specifier` relative to `referrer`, recursively.
    pub fn load_module(&mut self, referrer: Option<&JsStr>, specifier: &JsStr, attrs: &[(JsStr, JsStr)]) -> JsResult<Obj> {
        let r = referrer.map(|r| r.to_rust());
        let (resolved, src) = match self.host.load_module(r.as_deref(), &specifier.to_rust()) {
            Ok(x) => x,
            Err(e) => return self.throw_type(&e),
        };
        let rname = JsStr::from_str(&resolved);
        let ty = attrs.iter().find(|(k, _)| k.eq_str("type")).map(|(_, v)| v.clone());
        if let Some(t) = &ty {
            if !t.eq_str("json") {
                return self.throw_type(&alloc::format!("unsupported import attribute type '{}'", t));
            }
        }
        let key = if ty.is_some() { rname.concat(&JsStr::from_str("#json")) } else { rname.clone() };
        if let Some((_, m)) = self.modules.iter().find(|(n, _)| *n == key) {
            return Ok(*m);
        }
        if ty.is_some() {
            let v = crate::builtins::json::parse_json_text(self, &JsStr::from_str(&src))?;
            return Ok(self.synthetic_module(key, v));
        }
        let m = self.module_from_source(rname, &src)?;
        self.load_requested(m)?;
        Ok(m)
    }

    /// Load the requested modules of `m` (depth-first; cycles hit the module map).
    pub fn load_requested(&mut self, m: Obj) -> JsResult<()> {
        if self.mr(m).status != Status::New {
            return Ok(());
        }
        self.mr_mut(m).status = Status::Unlinked;
        let reqs = self.mr(m).info.requests.clone();
        let name = self.mr(m).name.clone();
        for (i, (spec, attrs)) in reqs.iter().enumerate() {
            let dep = self.load_module(Some(&name), spec, attrs)?;
            self.mr_mut(m).loaded[i] = Some(dep);
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------- linking

    fn get_exported_names(&mut self, m: Obj, set: &mut Vec<Obj>) -> Vec<JsStr> {
        if set.contains(&m) {
            return Vec::new();
        }
        set.push(m);
        let info = self.mr(m).info.clone();
        let mut names: Vec<JsStr> = info.local_exports.iter().map(|(e, _)| e.clone()).collect();
        names.extend(info.indirect_exports.iter().map(|(e, _, _)| e.clone()));
        for &si in &info.star_exports {
            if let Some(dep) = self.mr(m).loaded[si] {
                for n in self.get_exported_names(dep, set) {
                    if !n.eq_str("default") && !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
        names
    }

    fn resolve_export(&mut self, m: Obj, name: &JsStr, set: &mut Vec<(Obj, JsStr)>) -> Resolution {
        if set.iter().any(|(mm, n)| *mm == m && n == name) {
            return Resolution::NotFound;
        }
        set.push((m, name.clone()));
        let info = self.mr(m).info.clone();
        for (e, l) in &info.local_exports {
            if e == name {
                return Resolution::Binding(m, l.clone());
            }
        }
        for (e, req, imp) in &info.indirect_exports {
            if e == name {
                let dep = match self.mr(m).loaded[*req] {
                    Some(d) => d,
                    None => return Resolution::NotFound,
                };
                return match imp {
                    None => Resolution::Namespace(dep),
                    Some(i) => self.resolve_export(dep, i, set),
                };
            }
        }
        if name.eq_str("default") {
            return Resolution::NotFound;
        }
        let mut star: Option<Resolution> = None;
        for &si in &info.star_exports {
            let dep = match self.mr(m).loaded[si] {
                Some(d) => d,
                None => continue,
            };
            let r = self.resolve_export(dep, name, set);
            match r {
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::NotFound => {}
                r => match &star {
                    None => star = Some(r),
                    Some(s) => {
                        let same = match (s, &r) {
                            (Resolution::Binding(a, an), Resolution::Binding(b, bn)) => a == b && an == bn,
                            (Resolution::Namespace(a), Resolution::Namespace(b)) => a == b,
                            _ => false,
                        };
                        if !same {
                            return Resolution::Ambiguous;
                        }
                    }
                },
            }
        }
        star.unwrap_or(Resolution::NotFound)
    }

    /// Link(): InnerModuleLinking over the graph rooted at m.
    pub fn link_module(&mut self, m: Obj) -> JsResult<()> {
        let mut stack = Vec::new();
        let r = self.inner_link(m, &mut stack, 0);
        if let Err(e) = r {
            for x in stack {
                self.mr_mut(x).status = Status::Unlinked;
            }
            return Err(e);
        }
        Ok(())
    }

    fn inner_link(&mut self, m: Obj, stack: &mut Vec<Obj>, index: usize) -> JsResult<usize> {
        let st = self.mr(m).status;
        if !matches!(st, Status::Unlinked) {
            return Ok(index);
        }
        let mut index = index;
        {
            let r = self.mr_mut(m);
            r.status = Status::Linking;
            r.dfs_index = index;
            r.dfs_ancestor = index;
        }
        index += 1;
        stack.push(m);
        let deps: Vec<Option<Obj>> = self.mr(m).loaded.clone();
        for d in deps.into_iter().flatten() {
            index = self.inner_link(d, stack, index)?;
            if self.mr(d).status == Status::Linking {
                let a = self.mr(m).dfs_ancestor.min(self.mr(d).dfs_ancestor);
                self.mr_mut(m).dfs_ancestor = a;
            }
        }
        self.initialize_environment(m)?;
        if self.mr(m).dfs_ancestor == self.mr(m).dfs_index {
            loop {
                let x = stack.pop().unwrap();
                self.mr_mut(x).status = Status::Linked;
                if x == m {
                    break;
                }
            }
        }
        Ok(index)
    }

    /// InitializeEnvironment: check exports resolve, run the module's instantiation part (environment,
    /// hoisted functions) up to its GenStart, resolve import cells, bind namespace imports.
    fn initialize_environment(&mut self, m: Obj) -> JsResult<()> {
        let info = self.mr(m).info.clone();
        for (e, _, _) in &info.indirect_exports {
            let mut set = Vec::new();
            match self.resolve_export(m, e, &mut set) {
                Resolution::Binding(..) | Resolution::Namespace(_) => {}
                _ => return self.throw_syntax(&alloc::format!("export '{}' cannot be resolved", e)),
            }
        }
        if self.mr(m).synthetic_default.is_some() {
            return Ok(());
        }
        // Instantiate: run module code until GenStart.
        let code = self.mr(m).code.clone().unwrap();
        let realm = self.mr(m).realm;
        let global_env = self.realms[realm as usize].global_env;
        let base = self.stack.len();
        self.stack.push(Value::Undefined);
        self.stack.push(Value::Undefined);
        let nlocals = code.nlocals as usize;
        self.stack.resize(base + 2 + nlocals, Value::Undefined);
        let hb = self.handlers.len();
        self.frames.push(Frame {
            code,
            pc: 0,
            args_base: base + 2,
            argc: 0,
            base: base + 2,
            func: None,
            this: Value::Undefined,
            new_target: Value::Undefined,
            env: Some(global_env),
            handler_base: hb,
            realm,
            construct: false,
            entry: true,
            coroutine: None,
            resume_kind: 0,
            script: Some(m),
        });
        let saved = self.cur_realm;
        self.cur_realm = realm;
        let r = self.run();
        self.cur_realm = saved;
        self.stack.truncate(base);
        let co = match r? {
            Completion::Return(Value::Object(c)) => c,
            _ => return self.throw_type("module instantiation failed"),
        };
        let env = match &self.heap.get(co).kind {
            Kind::Coroutine(c) => c.frame.as_ref().and_then(|f| f.frame.env),
            _ => None,
        };
        {
            let r = self.mr_mut(m);
            r.coroutine = Some(co);
            r.env = env;
        }
        // Imports.
        for (i, ie) in info.imports.iter().enumerate() {
            let dep = match self.mr(m).loaded[ie.request] {
                Some(d) => d,
                None => return self.throw_syntax("unresolved module request"),
            };
            match &ie.import_name {
                None => {
                    let ns = self.get_module_namespace(dep);
                    self.module_env_init(m, &ie.local, Value::Object(ns));
                }
                Some(name) => {
                    let mut set = Vec::new();
                    match self.resolve_export(dep, name, &mut set) {
                        Resolution::Binding(tm, local) => {
                            let cell = self.binding_cell(tm, &local);
                            self.mr_mut(m).cells[i] = cell;
                        }
                        Resolution::Namespace(tm) => {
                            let ns = self.get_module_namespace(tm);
                            self.mr_mut(m).cells[i] = ImportCell::Value(Value::Object(ns));
                        }
                        Resolution::Ambiguous => return self.throw_syntax(&alloc::format!("ambiguous import '{}'", name)),
                        Resolution::NotFound => return self.throw_syntax(&alloc::format!("module does not provide an export named '{}'", name)),
                    }
                }
            }
        }
        Ok(())
    }

    fn binding_cell(&mut self, m: Obj, local: &JsStr) -> ImportCell {
        if let Some(v) = &self.mr(m).synthetic_default {
            return ImportCell::Value(v.clone());
        }
        match self.mr(m).env {
            Some(env) => match &self.heap.get(env).kind {
                Kind::Env(d) => match d.info.find(local) {
                    Some(i) => ImportCell::Slot(env, i as u32),
                    None => ImportCell::Unresolved,
                },
                _ => ImportCell::Unresolved,
            },
            // Not yet instantiated (a cycle): resolved lazily by name on access.
            None => ImportCell::Unresolved,
        }
    }

    fn module_env_init(&mut self, m: Obj, name: &JsStr, v: Value) {
        if let Some(env) = self.mr(m).env {
            if let Kind::Env(d) = &mut self.heap.get_mut(env).kind {
                if let Some(i) = d.info.find(name) {
                    d.slots[i] = v;
                }
            }
        }
    }

    /// GetImport(k) from the current frame's module.
    pub fn get_import(&mut self, k: u32) -> JsResult<Value> {
        let m = match self.frames.last().and_then(|f| f.script) {
            Some(m) => m,
            None => return self.throw_ref("import binding outside a module"),
        };
        let cell = self.mr(m).cells.get(k as usize).cloned().unwrap_or(ImportCell::Unresolved);
        let cell = match cell {
            ImportCell::Unresolved => {
                // Resolve now (cyclic graphs instantiate in dependency order).
                let ie = self.mr(m).info.imports[k as usize].clone();
                let dep = self.mr(m).loaded[ie.request];
                let mut c = ImportCell::Unresolved;
                if let (Some(dep), Some(name)) = (dep, &ie.import_name) {
                    let mut set = Vec::new();
                    if let Resolution::Binding(tm, local) = self.resolve_export(dep, name, &mut set) {
                        c = self.binding_cell(tm, &local);
                    }
                }
                if let ImportCell::Slot(..) = c {
                    self.mr_mut(m).cells[k as usize] = c.clone();
                }
                c
            }
            c => c,
        };
        match cell {
            ImportCell::Slot(e, i) => {
                let v = self.env_slot(e, i);
                if v.is_empty() {
                    let name = match &self.heap.get(e).kind {
                        Kind::Env(d) => d.info.names[i as usize].to_rust(),
                        _ => alloc::string::String::new(),
                    };
                    return Err(self.tdz_error(&name));
                }
                Ok(v)
            }
            ImportCell::Value(v) => Ok(v),
            ImportCell::Unresolved => self.throw_ref("import binding is not initialized"),
        }
    }

    // ------------------------------------------------------------------------------------- evaluation

    /// Evaluate() (§16.2.1.5.3): returns the top-level capability promise.
    pub fn evaluate_module(&mut self, m: Obj) -> JsResult<Obj> {
        let mut m = m;
        let st = self.mr(m).status;
        if matches!(st, Status::EvaluatingAsync | Status::Evaluated) {
            if let Some(r) = self.mr(m).cycle_root {
                m = r;
            }
        }
        if let Some(p) = self.mr(m).capability {
            return Ok(p);
        }
        let p = crate::builtins::promise::new_promise(self);
        self.mr_mut(m).capability = Some(p);
        let mut stack = Vec::new();
        let mut counter = 0u64;
        match self.inner_evaluate(m, &mut stack, 0, &mut counter) {
            Ok(_) => {
                if self.mr(m).async_evaluation.is_none() {
                    let _ = crate::builtins::promise::resolve_promise(self, p, Value::Undefined);
                }
            }
            Err(e) => {
                if self.terminated {
                    return Err(e);
                }
                for x in stack {
                    let r = self.mr_mut(x);
                    r.status = Status::Evaluated;
                    r.error = Some(e.clone());
                }
                crate::builtins::promise::reject_promise(self, p, e);
            }
        }
        Ok(p)
    }

    fn inner_evaluate(&mut self, m: Obj, stack: &mut Vec<Obj>, index: usize, counter: &mut u64) -> JsResult<usize> {
        let st = self.mr(m).status;
        match st {
            Status::EvaluatingAsync | Status::Evaluated => {
                return match self.mr(m).error.clone() {
                    Some(e) => Err(e),
                    None => Ok(index),
                };
            }
            Status::Evaluating => return Ok(index),
            _ => {}
        }
        let mut index = index;
        {
            let r = self.mr_mut(m);
            r.status = Status::Evaluating;
            r.dfs_index = index;
            r.dfs_ancestor = index;
            r.pending_deps = 0;
        }
        index += 1;
        stack.push(m);
        let deps: Vec<Option<Obj>> = self.mr(m).loaded.clone();
        for d in deps.into_iter().flatten() {
            index = self.inner_evaluate(d, stack, index, counter)?;
            let mut req = d;
            if self.mr(d).status == Status::Evaluating {
                let a = self.mr(m).dfs_ancestor.min(self.mr(d).dfs_ancestor);
                self.mr_mut(m).dfs_ancestor = a;
            } else {
                req = self.mr(d).cycle_root.unwrap_or(d);
                if let Some(e) = self.mr(req).error.clone() {
                    return Err(e);
                }
            }
            if self.mr(req).async_evaluation.is_some() {
                self.mr_mut(m).pending_deps += 1;
                self.mr_mut(req).async_parents.push(m);
            }
        }
        if self.mr(m).pending_deps > 0 || self.mr(m).has_tla {
            *counter += 1;
            let order = self.next_async_order();
            self.mr_mut(m).async_evaluation = Some(order);
            if self.mr(m).pending_deps == 0 {
                self.execute_async_module(m);
            }
        } else {
            self.execute_module_sync(m)?;
        }
        if self.mr(m).dfs_ancestor == self.mr(m).dfs_index {
            loop {
                let x = stack.pop().unwrap();
                {
                    let r = self.mr_mut(x);
                    r.status = if r.async_evaluation.is_none() { Status::Evaluated } else { Status::EvaluatingAsync };
                    r.cycle_root = Some(m);
                }
                if x == m {
                    break;
                }
            }
        }
        Ok(index)
    }

    fn next_async_order(&mut self) -> u64 {
        self.async_counter += 1;
        self.async_counter
    }

    fn execute_module_sync(&mut self, m: Obj) -> JsResult<()> {
        if self.mr(m).synthetic_default.is_some() {
            return Ok(());
        }
        let co = match self.mr(m).coroutine {
            Some(c) => c,
            None => return Ok(()),
        };
        self.resume_coroutine(co, 0, Value::Undefined)?;
        Ok(())
    }

    /// ExecuteAsyncModule: run the TLA body; its completion settles the module.
    fn execute_async_module(&mut self, m: Obj) {
        let co = match self.mr(m).coroutine {
            Some(c) => c,
            None => {
                self.async_module_fulfilled(m);
                return;
            }
        };
        // Make the coroutine an async function with its own promise.
        let p = crate::builtins::promise::new_promise(self);
        if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
            c.kind = CoroKind::Async;
            c.promise = Some(p);
        }
        let on_ok = self.make_native_closure("", 1, async_module_ok, alloc::vec![Value::Object(m)]);
        let on_err = self.make_native_closure("", 1, async_module_err, alloc::vec![Value::Object(m)]);
        crate::builtins::promise::perform_then(self, p, Value::Object(on_ok), Value::Object(on_err), None);
        let _ = self.resume_coroutine(co, 0, Value::Undefined);
    }

    fn async_module_fulfilled(&mut self, m: Obj) {
        if self.mr(m).status == Status::Evaluated {
            return;
        }
        {
            let r = self.mr_mut(m);
            r.async_evaluation = None;
            r.status = Status::Evaluated;
        }
        if let Some(p) = self.mr(m).capability {
            let _ = crate::builtins::promise::resolve_promise(self, p, Value::Undefined);
        }
        // GatherAvailableAncestors, sorted by async evaluation order.
        let mut exec: Vec<Obj> = Vec::new();
        self.gather_ancestors(m, &mut exec);
        exec.sort_by_key(|x| self.mr(*x).async_evaluation.unwrap_or(0));
        for x in exec {
            if self.mr(x).error.is_some() {
                continue;
            }
            if self.mr(x).has_tla {
                self.execute_async_module(x);
            } else {
                match self.execute_module_sync(x) {
                    Ok(()) => {
                        {
                            let r = self.mr_mut(x);
                            r.async_evaluation = None;
                            r.status = Status::Evaluated;
                        }
                        if let Some(p) = self.mr(x).capability {
                            let _ = crate::builtins::promise::resolve_promise(self, p, Value::Undefined);
                        }
                    }
                    Err(e) => self.async_module_rejected(x, e),
                }
            }
        }
    }

    fn gather_ancestors(&mut self, m: Obj, exec: &mut Vec<Obj>) {
        let parents = self.mr(m).async_parents.clone();
        for p in parents {
            if exec.contains(&p) {
                continue;
            }
            let root = self.mr(p).cycle_root.unwrap_or(p);
            if self.mr(root).error.is_some() {
                continue;
            }
            let r = self.mr_mut(p);
            if r.pending_deps > 0 {
                r.pending_deps -= 1;
            }
            if r.pending_deps == 0 {
                exec.push(p);
                if !self.mr(p).has_tla {
                    self.gather_ancestors(p, exec);
                }
            }
        }
    }

    fn async_module_rejected(&mut self, m: Obj, e: Value) {
        if self.mr(m).status == Status::Evaluated && self.mr(m).error.is_some() {
            return;
        }
        {
            let r = self.mr_mut(m);
            r.error = Some(e.clone());
            r.status = Status::Evaluated;
            r.async_evaluation = None;
        }
        let parents = self.mr(m).async_parents.clone();
        for p in parents {
            self.async_module_rejected(p, e.clone());
        }
        if let Some(p) = self.mr(m).capability {
            crate::builtins::promise::reject_promise(self, p, e);
        }
    }

    // ------------------------------------------------------------------------------------- namespaces

    pub fn get_module_namespace(&mut self, m: Obj) -> Obj {
        if let Some(ns) = self.mr(m).namespace {
            return ns;
        }
        let mut set = Vec::new();
        let names = self.get_exported_names(m, &mut set);
        let mut exports = Vec::new();
        for n in names {
            let mut rs = Vec::new();
            match self.resolve_export(m, &n, &mut rs) {
                Resolution::Binding(..) | Resolution::Namespace(_) => exports.push(n),
                _ => {}
            }
        }
        exports.sort();
        let mut d = ObjectData::new(None, Kind::ModuleNamespace(Box::new(ModuleNsData { module: m, exports })));
        d.extensible = false;
        d.props.insert(PropertyKey::Sym(self.wk.to_string_tag.clone()), Prop::data(Value::str("Module"), 0));
        let ns = self.heap.alloc(d);
        self.mr_mut(m).namespace = Some(ns);
        ns
    }

    fn ns_parts(&self, o: Obj) -> (Obj, Vec<JsStr>) {
        match &self.heap.get(o).kind {
            Kind::ModuleNamespace(n) => (n.module, n.exports.clone()),
            _ => unreachable!(),
        }
    }

    fn ns_value(&mut self, m: Obj, name: &JsStr) -> JsResult<Value> {
        let mut set = Vec::new();
        match self.resolve_export(m, name, &mut set) {
            Resolution::Binding(tm, local) => {
                if let Some(v) = &self.mr(tm).synthetic_default {
                    return Ok(v.clone());
                }
                match self.binding_cell(tm, &local) {
                    ImportCell::Slot(e, i) => {
                        let v = self.env_slot(e, i);
                        if v.is_empty() {
                            return Err(self.tdz_error(&local.to_rust()));
                        }
                        Ok(v)
                    }
                    _ => Err(self.tdz_error(&local.to_rust())),
                }
            }
            Resolution::Namespace(tm) => Ok(Value::Object(self.get_module_namespace(tm))),
            _ => self.throw_ref("unresolvable export"),
        }
    }

    // ------------------------------------------------------------------------------------- import() / import.meta

    pub fn import_call(&mut self, spec: Value, opts: Value) -> JsResult<Value> {
        let p = crate::builtins::promise::new_promise(self);
        let referrer = self.frames.last().and_then(|f| f.script).map(|m| self.mr(m).name.clone());
        let r = (|| -> JsResult<Obj> {
            let s = self.to_string(&spec)?;
            let mut attrs = Vec::new();
            if !opts.is_undefined() {
                let o = match opts {
                    Value::Object(o) => o,
                    _ => return self.throw_type("The second argument of import() must be an object"),
                };
                let w = self.get(o, &PropertyKey::from_str("with"))?;
                if !w.is_undefined() {
                    let wo = match w {
                        Value::Object(o) => o,
                        _ => return self.throw_type("The 'with' option must be an object"),
                    };
                    let keys = self.enumerable_own_keys(wo)?;
                    for k in keys {
                        let v = self.get(wo, &k)?;
                        match v {
                            Value::String(sv) => attrs.push((k.to_js_string(), sv)),
                            _ => return self.throw_type("import attribute values must be strings"),
                        }
                    }
                }
            }
            let m = self.load_module(referrer.as_ref(), &s, &attrs)?;
            self.link_module(m)?;
            Ok(m)
        })();
        match r {
            Ok(m) => {
                let ep = self.evaluate_module(m)?;
                let on_ok = self.make_native_closure("", 0, import_fulfilled, alloc::vec![Value::Object(m), Value::Object(p)]);
                let on_err = self.make_native_closure("", 1, import_rejected, alloc::vec![Value::Object(p)]);
                crate::builtins::promise::perform_then(self, ep, Value::Object(on_ok), Value::Object(on_err), None);
            }
            Err(e) => {
                if self.terminated {
                    return Err(e);
                }
                crate::builtins::promise::reject_promise(self, p, e);
            }
        }
        Ok(Value::Object(p))
    }

    pub fn import_meta(&mut self) -> JsResult<Value> {
        let m = match self.frames.last().and_then(|f| f.script) {
            Some(m) => m,
            None => return self.throw_syntax("import.meta outside a module"),
        };
        if let Some(o) = self.mr(m).meta {
            return Ok(Value::Object(o));
        }
        let o = self.heap.alloc(ObjectData::new(None, Kind::Ordinary));
        let name = self.mr(m).name.clone();
        self.heap.get_mut(o).props.insert(PropertyKey::from_str("url"), Prop::data(Value::String(name), WEC));
        self.mr_mut(m).meta = Some(o);
        Ok(Value::Object(o))
    }

    /// Run a module (load + link + evaluate) and drain jobs; used by hosts.
    pub fn run_module(&mut self, name: &str, src: &str) -> JsResult<Obj> {
        let m = self.module_from_source(JsStr::from_str(name), src)?;
        self.load_requested(m)?;
        self.link_module(m)?;
        self.evaluate_module(m)
    }
}

fn async_module_ok(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    vm.async_module_fulfilled(m);
    Ok(Value::Undefined)
}
fn async_module_err(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = vm.arg(ctx, 0);
    vm.async_module_rejected(m, e);
    Ok(Value::Undefined)
}
fn import_fulfilled(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let p = vm.native_slot(ctx.callee, 1).as_object().unwrap();
    let ns = vm.get_module_namespace(m);
    let _ = crate::builtins::promise::resolve_promise(vm, p, Value::Object(ns));
    Ok(Value::Undefined)
}
fn import_rejected(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = vm.arg(ctx, 0);
    crate::builtins::promise::reject_promise(vm, p, e);
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------- namespace internal methods

pub fn ns_get_own_property(vm: &mut Vm, o: Obj, key: &PropertyKey) -> JsResult<Option<PropDesc>> {
    if let PropertyKey::Sym(_) = key {
        return Ok(vm.heap.get(o).props.get(key).map(PropDesc::from_prop));
    }
    let (m, exports) = vm.ns_parts(o);
    let name = key.to_js_string();
    if !exports.contains(&name) {
        return Ok(None);
    }
    let v = vm.ns_value(m, &name)?;
    Ok(Some(PropDesc::data(v, true, true, false)))
}

pub fn ns_define_own_property(vm: &mut Vm, o: Obj, key: PropertyKey, d: PropDesc) -> JsResult<bool> {
    if let PropertyKey::Sym(_) = key {
        let cur = vm.heap.get(o).props.get(&key).cloned();
        return Ok(vm.validate_and_apply(Some(o), &key, false, &d, cur));
    }
    let cur = match ns_get_own_property(vm, o, &key)? {
        None => return Ok(false),
        Some(c) => c,
    };
    if d.configurable == Some(true) || d.enumerable == Some(false) || d.is_accessor() || d.writable == Some(false) {
        return Ok(false);
    }
    if let Some(v) = &d.value {
        return Ok(v.same_value(cur.value.as_ref().unwrap()));
    }
    Ok(true)
}

pub fn ns_has(vm: &mut Vm, o: Obj, key: &PropertyKey) -> JsResult<bool> {
    if let PropertyKey::Sym(_) = key {
        return Ok(vm.heap.get(o).props.get(key).is_some());
    }
    let (_, exports) = vm.ns_parts(o);
    Ok(exports.contains(&key.to_js_string()))
}

pub fn ns_get(vm: &mut Vm, o: Obj, key: &PropertyKey) -> JsResult<Value> {
    if let PropertyKey::Sym(_) = key {
        return Ok(match vm.heap.get(o).props.get(key) {
            Some(Prop { slot: Slot::Data(v), .. }) => v.clone(),
            _ => Value::Undefined,
        });
    }
    let (m, exports) = vm.ns_parts(o);
    let name = key.to_js_string();
    if !exports.contains(&name) {
        return Ok(Value::Undefined);
    }
    vm.ns_value(m, &name)
}

pub fn ns_delete(vm: &mut Vm, o: Obj, key: &PropertyKey) -> JsResult<bool> {
    if let PropertyKey::Sym(_) = key {
        return vm.delete_ordinary_sym(o, key);
    }
    let (_, exports) = vm.ns_parts(o);
    Ok(!exports.contains(&key.to_js_string()))
}

pub fn ns_own_keys(vm: &mut Vm, o: Obj) -> JsResult<Vec<PropertyKey>> {
    let (_, exports) = vm.ns_parts(o);
    let mut out: Vec<PropertyKey> = exports.into_iter().map(PropertyKey::from_js).collect();
    out.extend(vm.heap.get(o).props.keys().filter(|k| k.is_symbol()).cloned());
    Ok(out)
}

impl Vm {
    pub fn delete_ordinary_sym(&mut self, o: Obj, key: &PropertyKey) -> JsResult<bool> {
        match self.heap.get(o).props.get(key) {
            None => Ok(true),
            Some(p) if p.configurable() => {
                self.heap.get_mut(o).props.remove(key);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
