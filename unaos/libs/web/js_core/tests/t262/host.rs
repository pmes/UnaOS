//! The test262 host: console capture, `print`, `$262` (createRealm, evalScript, detachArrayBuffer, gc,
//! global, agent stubs) and a file-system module loader rooted at the test's directory.

use js_core::string::JsStr;
use js_core::vm::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct TestHost {
    pub out: Rc<RefCell<String>>,
    pub base: PathBuf,
}

impl Host for TestHost {
    fn console(&mut self, _level: u8, msg: &str) {
        let mut o = self.out.borrow_mut();
        o.push_str(msg);
        o.push('\n');
    }
    fn load_module(&mut self, referrer: Option<&str>, specifier: &str) -> Result<(String, String), String> {
        let dir = match referrer {
            Some(r) => Path::new(r).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.base.clone()),
            None => self.base.clone(),
        };
        let p = dir.join(specifier);
        let p = normalize(&p);
        match std::fs::read_to_string(&p) {
            Ok(s) => Ok((p.to_string_lossy().to_string(), s)),
            Err(e) => Err(format!("Cannot load module {}: {}", p.display(), e)),
        }
    }
    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn print(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let s = vm.to_string(&v)?.to_rust();
    vm.host.console(0, &s);
    Ok(Value::Undefined)
}

fn create_realm(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let r = vm.create_realm();
    let saved = vm.cur_realm;
    vm.cur_realm = r;
    let o = install(vm);
    vm.cur_realm = saved;
    Ok(Value::Object(o))
}

fn eval_script(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let src = vm.arg(ctx, 0);
    let s = vm.to_string(&src)?;
    // Run in the realm of this $262 object.
    let realm = match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) => n as u32,
        _ => vm.cur_realm,
    };
    let saved = vm.cur_realm;
    vm.cur_realm = realm;
    let r = vm.run_script(s.units());
    vm.cur_realm = saved;
    r
}

fn gc(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.collect_garbage();
    Ok(Value::Undefined)
}

fn detach(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = vm.arg(ctx, 0);
    js_core::builtins::typedarray::detach_array_buffer(vm, &b, &Value::Undefined)?;
    Ok(Value::Null)
}

/// Install `print` and `$262` into the current realm; returns the $262 object.
pub fn install(vm: &mut Vm) -> Obj {
    let realm = vm.cur_realm;
    let g = vm.realm().global;
    let pf = vm.make_native("print", 1, print, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("print"), Prop::data(Value::Object(pf), WC));
    let o = vm.new_plain_object();
    let fp = vm.intr().function_proto;
    let cr = vm.make_native("createRealm", 0, create_realm, false);
    let es = vm.make_native_with("evalScript", 1, eval_script, false, Some(fp), vec![Value::Number(realm as f64)]);
    let gcf = vm.make_native("gc", 0, gc, false);
    let det = vm.make_native("detachArrayBuffer", 1, detach, false);
    let agent = vm.new_plain_object();
    for (k, v) in [
        ("createRealm", Value::Object(cr)),
        ("evalScript", Value::Object(es)),
        ("gc", Value::Object(gcf)),
        ("detachArrayBuffer", Value::Object(det)),
        ("global", Value::Object(g)),
        ("agent", Value::Object(agent)),
    ] {
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(k), Prop::data(v, WC));
    }
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("$262"), Prop::data(Value::Object(o), WC));
    vm.realms[realm as usize].host_defined = Value::Object(o);
    o
}

pub fn js(s: &str) -> JsStr {
    JsStr::from_str(s)
}
