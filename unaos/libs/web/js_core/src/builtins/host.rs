//! Host hooks for embeddings (the HTML-style host surface without a DOM): `console`, `setTimeout` /
//! `setInterval` / `clearTimeout` / `clearInterval`, `queueMicrotask`, and a built-in event loop on virtual time
//! for hosts that do not run their own (`Host::set_timer` returning None hands timers to it).

use super::*;

/// A pending timer of the built-in event loop.
pub struct Timer {
    pub id: u32,
    pub due: f64,
    /// Insertion order breaks ties between timers due at the same time.
    pub seq: u64,
    pub callback: Value,
    pub args: Vec<Value>,
    pub interval: Option<f64>,
}

/// Install the host globals into the current realm's global object.
pub fn install_host_globals(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let console = vm.new_object(Some(op));
    for (n, level) in [("log", 0u8), ("warn", 1), ("error", 2), ("debug", 3), ("info", 4), ("trace", 3)] {
        let fp = vm.intr().function_proto;
        let f = vm.make_native_with(n, 0, console_method, false, Some(fp), alloc::vec![Value::Number(level as f64)]);
        vm.heap.get_mut(console).props.insert(PropertyKey::from_str(n), Prop::data(Value::Object(f), WEC));
    }
    to_str_tag(vm, console, "console");
    global(vm, "console", Value::Object(console));
    let g = vm.realm().global;
    method(vm, g, "setTimeout", 1, set_timeout);
    method(vm, g, "setInterval", 1, set_interval);
    method(vm, g, "clearTimeout", 0, clear_timer);
    method(vm, g, "clearInterval", 0, clear_timer);
    method(vm, g, "queueMicrotask", 1, queue_microtask);
}

/// A side-effect-free rendering of a value for console output (own data properties only, depth-limited).
pub fn inspect(vm: &Vm, v: &Value, depth: u32, out: &mut String) {
    match v {
        Value::String(s) => {
            if depth == 0 {
                out.push_str(&s.to_rust());
            } else {
                out.push('\'');
                out.push_str(&s.to_rust());
                out.push('\'');
            }
        }
        Value::Number(n) => out.push_str(&crate::vm::ops::number_to_jsstr(*n).to_rust()),
        Value::BigInt(b) => {
            out.push_str(&crate::builtins::bigint::to_string_radix(b, 10));
            out.push('n');
        }
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Undefined | Value::Empty => out.push_str("undefined"),
        Value::Null => out.push_str("null"),
        Value::Symbol(s) => {
            out.push_str("Symbol(");
            if let Some(d) = s.desc() {
                out.push_str(&d.to_rust());
            }
            out.push(')');
        }
        Value::Object(o) => {
            let d = vm.heap.get(*o);
            match &d.kind {
                Kind::Function(_) | Kind::Native(_) | Kind::Bound(_) => {
                    out.push_str("[Function]");
                    return;
                }
                Kind::Error(_) => {
                    let name = d.props.get(&PropertyKey::from_str("message")).map(|p| match &p.slot {
                        Slot::Data(Value::String(s)) => s.to_rust(),
                        _ => String::new(),
                    });
                    out.push_str("Error: ");
                    out.push_str(&name.unwrap_or_default());
                    return;
                }
                _ => {}
            }
            if depth > 2 {
                out.push_str("[Object]");
                return;
            }
            if let Kind::Array(a) = &d.kind {
                out.push('[');
                let n = a.length() as usize;
                for i in 0..n.min(100) {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match vm.ordinary_get_own(*o, &PropertyKey::Index(i as u32)) {
                        Some(Prop { slot: Slot::Data(x), .. }) => inspect(vm, &x, depth + 1, out),
                        Some(_) => out.push_str("[Getter]"),
                        None => out.push_str("<empty>"),
                    }
                }
                if n > 100 {
                    out.push_str(", ...");
                }
                out.push(']');
                return;
            }
            out.push('{');
            let mut first = true;
            for (k, p) in d.props.iter() {
                if !p.enumerable() || k.is_symbol() {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                out.push(' ');
                out.push_str(&k.to_js_string().to_rust());
                out.push_str(": ");
                match &p.slot {
                    Slot::Data(x) => inspect(vm, x, depth + 1, out),
                    _ => out.push_str("[Getter/Setter]"),
                }
            }
            out.push_str(if first { "}" } else { " }" });
        }
    }
}

fn console_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let level = match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) => n as u8,
        _ => 0,
    };
    let mut out = String::new();
    for i in 0..ctx.argc {
        if i > 0 {
            out.push(' ');
        }
        let a = vm.arg(ctx, i);
        inspect(vm, &a, 0, &mut out);
    }
    vm.host.console(level, &out);
    Ok(Value::Undefined)
}

fn add_timer(vm: &mut Vm, ctx: &CallCtx, repeat: bool) -> JsResult<Value> {
    let cb = vm.arg(ctx, 0);
    if !vm.is_callable(&cb) {
        // HTML compiles a string handler; this host does not evaluate strings.
        return vm.throw_type("timer handler must be a function");
    }
    let d = vm.arg(ctx, 1);
    let delay = vm.to_number(&d)?;
    let delay = if delay.is_nan() || delay < 0.0 { 0.0 } else { delay };
    let args: Vec<Value> = if ctx.argc > 2 { vm.stack[ctx.args_base + 2..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    if let Some(id) = vm.host.set_timer(cb.clone(), delay, repeat) {
        return Ok(Value::Number(id as f64));
    }
    vm.timer_seq += 1;
    let id = vm.timer_seq;
    let seq = id as u64;
    let due = vm.loop_now + delay;
    vm.timers.push(Timer { id, due, seq, callback: cb, args, interval: if repeat { Some(delay.max(1.0)) } else { None } });
    Ok(Value::Number(id as f64))
}

fn set_timeout(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_timer(vm, ctx, false)
}
fn set_interval(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    add_timer(vm, ctx, true)
}
fn clear_timer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let id = vm.to_number(&a)?;
    if id.is_finite() && id >= 0.0 {
        let id = id as u32;
        let before = vm.timers.len();
        vm.timers.retain(|t| t.id != id);
        if vm.timers.len() == before {
            vm.host.clear_timer(id);
        }
    }
    Ok(Value::Undefined)
}

fn queue_microtask(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cb = vm.arg(ctx, 0);
    if !vm.is_callable(&cb) {
        return vm.throw_type("queueMicrotask requires a function");
    }
    vm.jobs.push_back(Job::Call { func: cb, this: Value::Undefined, args: Vec::new() });
    Ok(Value::Undefined)
}

impl Vm {
    /// Run the built-in event loop: a microtask checkpoint, then due timers in order (virtual time jumps to the
    /// next due timer), each followed by a checkpoint, until no timers remain or `max_tasks` tasks ran.
    /// Uncaught exceptions from tasks are reported to the host console and the loop continues (HTML "report
    /// the exception"); budget exhaustion stops it.
    pub fn run_event_loop(&mut self, max_tasks: usize) -> JsResult<()> {
        self.checkpoint()?;
        let mut ran = 0;
        while ran < max_tasks && !self.timers.is_empty() {
            let mut best = 0;
            for (i, t) in self.timers.iter().enumerate() {
                let b = &self.timers[best];
                if t.due < b.due || (t.due == b.due && t.seq < b.seq) {
                    best = i;
                }
            }
            let t = self.timers.remove(best);
            if t.due > self.loop_now {
                self.loop_now = t.due;
            }
            if let Some(iv) = t.interval {
                self.timer_seq += 1;
                let seq = self.timer_seq as u64 + (1u64 << 32);
                self.timers.push(Timer { id: t.id, due: self.loop_now + iv, seq, callback: t.callback.clone(), args: t.args.clone(), interval: t.interval });
            }
            let r = self.call(&t.callback, &Value::Undefined, &t.args);
            if let Err(e) = r {
                if self.terminated {
                    return Err(e);
                }
                let msg = self.error_string(&e);
                self.host.console(2, &alloc::format!("Uncaught {}", msg));
            }
            self.checkpoint()?;
            ran += 1;
        }
        Ok(())
    }

    /// A microtask checkpoint that reports (rather than propagates) exceptions thrown by queued callbacks.
    pub fn checkpoint(&mut self) -> JsResult<()> {
        loop {
            match self.run_jobs() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if self.terminated {
                        return Err(e);
                    }
                    let msg = self.error_string(&e);
                    self.host.console(2, &alloc::format!("Uncaught {}", msg));
                }
            }
        }
    }
}

use alloc::string::String;
