//! The global object (HTML §7.2 Window) and the Window-level APIs: Location (§7.10.4), History
//! (§7.7.2), Navigator (§8.9), Screen (CSSOM View), the timers (§8.6) on Aether's page event loop,
//! `requestAnimationFrame` (§8.10), `queueMicrotask` (§8.7), `matchMedia` over css_core's media query
//! evaluator, `atob`/`btoa` (§8.3), `reportError`, the console, `performance.now()`; and the
//! Document members that belong to the page rather than the tree (`readyState`, `currentScript`,
//! `cookie`, `write`, `defaultView`, `activeElement`, …).
//!
//! Following WebIDL §3.7.2 for [Global] interfaces, Window's operations and attributes are own
//! properties of the global object; its prototype chain is Window.prototype → WindowProperties (the
//! named-properties object: `window.foo` finds `<div id=foo>`) → EventTarget.prototype.

use super::dom::{self, iface, register_iface, with_doc, wrap};
use super::idl::{self, *};
use super::{page, TargetKey};
use js_core::builtins::host::Timer;
use js_core::vm::*;

// =================================================================================================
// Timers (HTML §8.6) — the callbacks live in `vm.timers` (GC-traced); the schedule is the page clock
// =================================================================================================

fn timer_common(vm: &mut Vm, ctx: &CallCtx, repeat: bool) -> JsResult<Value> {
    let handler = arg(vm, ctx, 0);
    // A non-callable handler is compiled as script source when it fires (HTML: "timer handler").
    let callback = if vm.is_callable(&handler) { handler } else { Value::String(vm.to_string(&handler)?) };
    let t = arg(vm, ctx, 1);
    let d = if t.is_undefined() { 0.0 } else { vm.to_number(&t)? };
    let mut delay = if d.is_finite() && d > 0.0 { d.min(2_147_483_647.0) } else { 0.0 };
    if repeat {
        delay = delay.max(4.0);
    }
    let args: Vec<Value> = (2..ctx.argc).map(|i| arg(vm, ctx, i)).collect();
    vm.timer_seq = vm.timer_seq.wrapping_add(1).max(1);
    let id = vm.timer_seq;
    let now = crate::event_loop::now_ms() as f64;
    vm.timers.push(Timer { id, due: now + delay, seq: id as u64, callback, args, interval: if repeat { Some(delay) } else { None } });
    crate::event_loop::set_armed(vm.timers.len());
    Ok(num(id as f64))
}

fn set_timeout(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    timer_common(vm, ctx, false)
}

fn set_interval(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    timer_common(vm, ctx, true)
}

fn clear_timer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let id = vm.to_number(&v)?;
    if id.is_finite() && id > 0.0 {
        let id = id as u32;
        vm.timers.retain(|t| t.id != id);
        crate::event_loop::set_armed(vm.timers.len());
    }
    Ok(Value::Undefined)
}

// =================================================================================================
// requestAnimationFrame, queueMicrotask
// =================================================================================================

fn request_animation_frame(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "requestAnimationFrame")?;
    let cb = arg(vm, ctx, 0);
    if !vm.is_callable(&cb) {
        return vm.throw_type("Failed to execute 'requestAnimationFrame' on 'Window': The callback provided as parameter 1 is not a function.");
    }
    let r = root(vm, cb);
    let id = page(|p| {
        p.raf_seq += 1;
        p.raf.push((p.raf_seq, r));
        p.raf_seq
    });
    Ok(num(id as f64))
}

fn cancel_animation_frame(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = vm.to_number(&arg(vm, ctx, 0))? as u32;
    let r = page(|p| {
        let i = p.raf.iter().position(|(h, _)| *h == id)?;
        Some(p.raf.remove(i).1)
    });
    if let Some(r) = r {
        unroot(vm, r);
    }
    Ok(Value::Undefined)
}

/// Runs the animation-frame callbacks in at most `passes` passes (HTML "run the animation frame
/// callbacks"): each pass takes the callbacks registered before it began; one re-registering itself
/// every frame is an animation loop and stops when the passes run out.
pub fn drain_raf(vm: &mut Vm, passes: u32) {
    for _ in 0..passes {
        if super::engine_poisoned() {
            return;
        }
        if run_raf_pass(vm) == 0 {
            return;
        }
    }
    // What is still registered after the last pass is an animation loop: dropped.
    let left: Vec<(u32, usize)> = page(|p| std::mem::take(&mut p.raf));
    for (_, r) in left {
        unroot(vm, r);
    }
}

/// One animation frame: advances the page clock a frame and runs the callbacks registered before it.
pub fn run_raf_pass(vm: &mut Vm) -> usize {
    let batch: Vec<(u32, usize)> = page(|p| std::mem::take(&mut p.raf));
    if batch.is_empty() {
        return 0;
    }
    crate::event_loop::advance_clock(16);
    let ts = super::events::now_hr();
    let n = batch.len();
    for (_, r) in batch {
        let cb = idl::rooted(vm, r);
        unroot(vm, r);
        super::invoke(vm, "raf-callback", &cb, &Value::Undefined, &[num(ts)]);
    }
    n
}

fn microtask_trampoline(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cb = vm.native_slot(ctx.callee, 0);
    if let Err(e) = vm.call(&cb, &Value::Undefined, &[]) {
        if vm.terminated {
            return Err(e);
        }
        super::events::report_exception(vm, &e);
    }
    Ok(Value::Undefined)
}

fn queue_microtask(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "queueMicrotask")?;
    let cb = arg(vm, ctx, 0);
    if !vm.is_callable(&cb) {
        return vm.throw_type("Failed to execute 'queueMicrotask' on 'Window': The callback provided as parameter 1 is not a function.");
    }
    let fp = vm.intr().function_proto;
    let t = vm.make_native_with("", 0, microtask_trampoline, false, Some(fp), vec![cb]);
    vm.jobs.push_back(Job::Call { func: Value::Object(t), this: Value::Undefined, args: Vec::new() });
    Ok(Value::Undefined)
}

// =================================================================================================
// Console
// =================================================================================================

fn console_format(vm: &mut Vm, ctx: &CallCtx, from: usize) -> String {
    let mut out = String::new();
    let mut i = from;
    if from < ctx.argc {
        if let Value::String(f) = arg(vm, ctx, from) {
            let fmt = f.to_rust();
            if fmt.contains('%') {
                i += 1;
                let mut chars = fmt.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '%' {
                        match chars.peek().copied() {
                            Some(k @ ('s' | 'd' | 'i' | 'f' | 'o' | 'O' | 'c')) => {
                                chars.next();
                                if k == 'c' {
                                    i += 1;
                                    continue;
                                }
                                if i < ctx.argc {
                                    let a = arg(vm, ctx, i);
                                    i += 1;
                                    match k {
                                        'd' | 'i' => {
                                            let n = vm.to_number(&a).unwrap_or(f64::NAN);
                                            out.push_str(&if n.is_finite() { format!("{}", n.trunc() as i64) } else { "NaN".into() });
                                        }
                                        'f' => {
                                            let n = vm.to_number(&a).unwrap_or(f64::NAN);
                                            js_core::builtins::host::inspect(vm, &num(n), 0, &mut out);
                                        }
                                        's' => match &a {
                                            Value::String(s) => out.push_str(&s.to_rust()),
                                            other => js_core::builtins::host::inspect(vm, other, 0, &mut out),
                                        },
                                        _ => js_core::builtins::host::inspect(vm, &a, 1, &mut out),
                                    }
                                } else {
                                    out.push('%');
                                    out.push(k);
                                }
                                continue;
                            }
                            Some('%') => {
                                chars.next();
                                out.push('%');
                                continue;
                            }
                            _ => {}
                        }
                    }
                    out.push(c);
                }
            }
        }
    }
    while i < ctx.argc {
        if !out.is_empty() {
            out.push(' ');
        }
        let a = arg(vm, ctx, i);
        js_core::builtins::host::inspect(vm, &a, 0, &mut out);
        i += 1;
    }
    out
}

fn console_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let level = callee_str(vm, ctx);
    let lv = match level.as_str() {
        "warn" => 1,
        "error" => 2,
        "debug" | "trace" => 3,
        "info" => 4,
        _ => 0,
    };
    let msg = console_format(vm, ctx, 0);
    super::host::console_out(lv, &msg);
    Ok(Value::Undefined)
}

fn console_assert(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if vm.to_boolean(&arg(vm, ctx, 0)) {
        return Ok(Value::Undefined);
    }
    let rest = console_format(vm, ctx, 1);
    super::host::console_out(2, &format!("Assertion failed{}{}", if rest.is_empty() { "" } else { ": " }, rest));
    Ok(Value::Undefined)
}

fn console_noop(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Undefined)
}

// =================================================================================================
// Global attributes and operations
// =================================================================================================

fn global_self(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Object(vm.realm().global))
}

fn global_document(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let d = dom::main_doc();
    Ok(wrap(vm, d))
}

fn global_number(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    let (w, h) = page(|p| p.viewport);
    let (sx, sy) = page(|p| p.scroll);
    Ok(num(match which.as_str() {
        "innerWidth" | "outerWidth" => w as f64,
        "innerHeight" | "outerHeight" => h as f64,
        "scrollX" | "pageXOffset" => sx,
        "scrollY" | "pageYOffset" => sy,
        "devicePixelRatio" => 1.0,
        _ => 0.0,
    }))
}

fn global_null(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Null)
}

fn global_false(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn global_true(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

fn global_zero(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(0.0))
}

fn global_origin(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let u = super::page_url();
    Ok(s(&url::Url::parse(&u).map(|u| super::html::url_part(&u, "origin")).unwrap_or_else(|_| "null".into())))
}

fn dialog_stub(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    crate::ledger::record_js(&format!("window.{which}-headless"));
    Ok(match which.as_str() {
        "confirm" => Value::Bool(false),
        "prompt" | "open" => Value::Null,
        _ => Value::Undefined,
    })
}

fn window_scroll(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    let a = arg(vm, ctx, 0);
    let (mut x, mut y) = (None, None);
    if let Value::Object(o) = &a {
        let l = vm.get(*o, &PropertyKey::from_str("left"))?;
        let t = vm.get(*o, &PropertyKey::from_str("top"))?;
        if !l.is_undefined() {
            x = Some(vm.to_number(&l)?);
        }
        if !t.is_undefined() {
            y = Some(vm.to_number(&t)?);
        }
    } else if ctx.argc >= 2 {
        x = Some(vm.to_number(&a)?);
        y = Some(vm.to_number(&arg(vm, ctx, 1))?);
    }
    page(|p| {
        let (cx, cy) = p.scroll;
        let (nx, ny) = if which == "scrollBy" {
            (cx + x.unwrap_or(0.0), cy + y.unwrap_or(0.0))
        } else {
            (x.unwrap_or(cx), y.unwrap_or(cy))
        };
        p.scroll = (nx.max(0.0), ny.max(0.0));
    });
    crate::ledger::record_js("window.scroll-recorded-not-applied");
    Ok(Value::Undefined)
}

fn post_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "postMessage")?;
    let data = arg(vm, ctx, 0);
    // HTML §9.3.3: a MessageEvent at the window, queued on the posted message task source.
    let fp = vm.intr().function_proto;
    let f = vm.make_native_with("", 0, deliver_message, false, Some(fp), vec![data]);
    let r = root(vm, Value::Object(f));
    page(|p| p.tasks.push_back(super::Task::Callback(r)));
    Ok(Value::Undefined)
}

fn deliver_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let data = vm.native_slot(ctx.callee, 0);
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("data"), data)?;
    let origin = global_origin(vm, ctx)?;
    vm.create_data_property(init, PropertyKey::from_str("origin"), origin)?;
    vm.create_data_property(init, PropertyKey::from_str("source"), Value::Object(vm.realm().global))?;
    let e = super::events::new_event(vm, "MessageEvent", "message", false, false, Some(init));
    super::events::dispatch(vm, e, TargetKey::Window);
    Ok(Value::Undefined)
}

fn ledger_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let name = vm.to_string(&v)?.to_rust();
    crate::ledger::record_js(super::clip(&name, 64));
    Ok(Value::Undefined)
}

fn report_error(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = arg(vm, ctx, 0);
    super::events::report_exception(vm, &e);
    Ok(Value::Undefined)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn btoa(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "btoa")?;
    let v = arg(vm, ctx, 0);
    let st = vm.to_string(&v)?;
    let mut bytes = Vec::with_capacity(st.len());
    for &u in st.units() {
        if u > 0xFF {
            return throw_dom(vm, "InvalidCharacterError", "The string to be encoded contains characters outside of the Latin1 range.");
        }
        bytes.push(u as u8);
    }
    let mut out = String::new();
    for ch in bytes.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        out.push(B64[(b[0] >> 2) as usize] as char);
        out.push(B64[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if ch.len() > 1 { B64[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize] as char } else { '=' });
        out.push(if ch.len() > 2 { B64[(b[2] & 63) as usize] as char } else { '=' });
    }
    Ok(s(&out))
}

/// HTML "forgiving-base64 decode".
fn atob(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "atob")?;
    let v = arg(vm, ctx, 0);
    let st = vm.to_string(&v)?;
    let mut data: Vec<u16> = st.units().iter().copied().filter(|&c| !matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)).collect();
    if data.len() % 4 == 0 {
        if data.last() == Some(&(b'=' as u16)) {
            data.pop();
            if data.last() == Some(&(b'=' as u16)) {
                data.pop();
            }
        }
    }
    let bad = || "The string to be decoded is not correctly encoded.";
    if data.len() % 4 == 1 {
        return throw_dom(vm, "InvalidCharacterError", bad());
    }
    let mut buf: u32 = 0;
    let mut bits = 0;
    let mut out: Vec<u16> = Vec::new();
    for c in data {
        let v = match c {
            0x41..=0x5A => c - 0x41,
            0x61..=0x7A => c - 0x61 + 26,
            0x30..=0x39 => c - 0x30 + 52,
            0x2B => 62,
            0x2F => 63,
            _ => return throw_dom(vm, "InvalidCharacterError", bad()),
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u16);
        }
    }
    Ok(Value::String(js_core::string::JsStr::from_units(out)))
}

fn performance_now(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(super::events::now_hr()))
}

fn performance_time_origin(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    Ok(num(wall - super::events::now_hr()))
}

// ---- matchMedia (CSSOM View §4.2) over css_core's media queries

fn match_media(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "matchMedia")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    let p = iface("MediaQueryList").unwrap().proto;
    Ok(Value::Object(host_obj(vm, p, T_MQL, vec![s(&q)])))
}

/// Evaluates a media query list against the page viewport (the environment the cascade uses).
pub fn media_matches(q: &str) -> bool {
    let (w, h) = page(|p| p.viewport);
    let env = crate::css::media_environment(w, h);
    let cvs = css_core::parser::parse_component_values(q);
    css_core::media::parse_media_query_list(&cvs).matches(&env)
}

fn mql_matches(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_MQL) else { return vm.throw_type("Illegal invocation") };
    let q = match slot(vm, o, 0) {
        Value::String(q) => q.to_rust(),
        _ => String::new(),
    };
    Ok(Value::Bool(media_matches(&q)))
}

fn mql_media(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_MQL) else { return vm.throw_type("Illegal invocation") };
    let q = match slot(vm, o, 0) {
        Value::String(q) => q.to_rust(),
        _ => String::new(),
    };
    // CSSOM "serialize a media query list": normalized whitespace and lowercase keywords.
    let cvs = css_core::parser::parse_component_values(&q);
    let parts: Vec<String> = q.split(',').map(|x| x.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let _ = cvs;
    Ok(s(&parts.join(", ")))
}

fn mql_listener(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_tagged(vm, &ctx.this, T_MQL);
    crate::ledger::record_js("MediaQueryList-change-never-fires");
    Ok(Value::Undefined)
}

// =================================================================================================
// Location (HTML §7.10.4)
// =================================================================================================

fn location_obj(vm: &mut Vm) -> Value {
    let doc = dom::main_doc();
    dom::cached(vm, doc, SO_LOCATION, |vm| {
        let p = iface("Location").unwrap().proto;
        host_obj(vm, p, T_LOCATION, vec![])
    })
}

const SO_LOCATION: u8 = 40;
const SO_HISTORY: u8 = 41;
const SO_NAVIGATOR: u8 = 42;
const SO_SCREEN: u8 = 43;

fn location_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(location_obj(vm))
}

fn location_put(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    // [PutForwards=href]
    let v = string(vm, &arg(vm, ctx, 0))?;
    navigate(vm, &v, false)?;
    Ok(Value::Undefined)
}

fn current_url() -> Option<url::Url> {
    url::Url::parse(&super::page_url()).ok()
}

/// Location-object navigate: a fragment-only change stays in the document (hashchange); anything else
/// is staged for the shell, which performs navigations.
fn navigate(vm: &mut Vm, target: &str, _replace: bool) -> JsResult<()> {
    let base = super::page_url();
    let resolved = match url::Url::parse(&base).and_then(|b| b.join(target)) {
        Ok(u) => u,
        Err(_) => match url::Url::parse(target) {
            Ok(u) => u,
            Err(_) => return throw_dom(vm, "SyntaxError", &format!("'{target}' is not a valid URL.")),
        },
    };
    if let Some(cur) = current_url() {
        let mut a = cur.clone();
        a.set_fragment(None);
        let mut b = resolved.clone();
        b.set_fragment(None);
        if a == b && resolved.fragment().is_some() {
            let old = cur.to_string();
            let new = resolved.to_string();
            if old != new {
                page(|p| p.url = new.clone());
                super::Engine::set_doc_url(&new);
                let init = vm.new_plain_object();
                vm.create_data_property(init, PropertyKey::from_str("oldURL"), s(&old))?;
                vm.create_data_property(init, PropertyKey::from_str("newURL"), s(&new))?;
                let fp = vm.intr().function_proto;
                let f = vm.make_native_with("", 0, fire_hashchange, false, Some(fp), vec![Value::Object(init)]);
                let r = root(vm, Value::Object(f));
                page(|p| p.tasks.push_back(super::Task::Callback(r)));
            }
            return Ok(());
        }
    }
    page(|p| p.pending_nav = Some(resolved.to_string()));
    crate::ledger::record_dom("location-navigation-staged");
    Ok(())
}

fn fire_hashchange(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let init = vm.native_slot(ctx.callee, 0).as_object();
    let e = super::events::new_event(vm, "HashChangeEvent", "hashchange", true, false, init);
    super::events::dispatch(vm, e, TargetKey::Window);
    Ok(Value::Undefined)
}

fn loc_part_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let part = callee_str(vm, ctx);
    let Some(u) = current_url() else { return Ok(s(if part == "href" { "about:blank" } else { "" })) };
    Ok(s(&super::html::url_part(&u, &part)))
}

fn loc_part_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let part = callee_str(vm, ctx);
    let v = string(vm, &arg(vm, ctx, 0))?;
    if part == "href" {
        navigate(vm, &v, false)?;
        return Ok(Value::Undefined);
    }
    let Some(mut u) = current_url() else { return Ok(Value::Undefined) };
    super::html::set_url_part(&mut u, &part, &v);
    navigate(vm, u.as_str(), false)?;
    Ok(Value::Undefined)
}

fn loc_assign(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Location", "assign")?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    navigate(vm, &v, false)?;
    Ok(Value::Undefined)
}

fn loc_replace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Location", "replace")?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    navigate(vm, &v, true)?;
    Ok(Value::Undefined)
}

fn loc_reload(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let u = super::page_url();
    page(|p| p.pending_nav = Some(u));
    Ok(Value::Undefined)
}

// =================================================================================================
// History (HTML §7.7.2) — same-document state only
// =================================================================================================

fn history_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let doc = dom::main_doc();
    Ok(dom::cached(vm, doc, SO_HISTORY, |vm| {
        let p = iface("History").unwrap().proto;
        host_obj(vm, p, T_HISTORY, vec![Value::Null, num(1.0)])
    }))
}

fn this_history(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_HISTORY) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn history_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_history(vm, ctx)?;
    Ok(slot(vm, o, 1))
}

fn history_state(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_history(vm, ctx)?;
    Ok(slot(vm, o, 0))
}

fn history_push(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_history(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let state = arg(vm, ctx, 0);
    // StructuredSerializeForStorage, approximated by a JSON round trip.
    let json = vm.realm().intrinsics.json;
    let stringify = vm.get(json, &PropertyKey::from_str("stringify"))?;
    let parse = vm.get(json, &PropertyKey::from_str("parse"))?;
    let text = vm.call(&stringify, &Value::Object(json), &[state])?;
    let cloned = if text.is_undefined() { Value::Null } else { vm.call(&parse, &Value::Object(json), &[text])? };
    set_slot(vm, o, 0, cloned);
    let u = arg(vm, ctx, 2);
    if !u.is_nullish() {
        let u = string(vm, &u)?;
        let base = super::page_url();
        if let Ok(nu) = url::Url::parse(&base).and_then(|b| b.join(&u)) {
            let same_origin = current_url().is_some_and(|c| c.origin() == nu.origin());
            if !same_origin {
                return throw_dom(vm, "SecurityError", "A history state object with URL cannot be created in a document with a different origin.");
            }
            page(|p| p.url = nu.to_string());
            super::Engine::set_doc_url(nu.as_str());
        }
    }
    if which == "pushState" {
        let n = slot_num(vm, o, 1);
        set_slot(vm, o, 1, num(n + 1.0));
    }
    Ok(Value::Undefined)
}

fn history_nav(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_history(vm, ctx)?;
    crate::ledger::record_js("history-traversal-not-supported");
    Ok(Value::Undefined)
}

fn history_scroll_restoration(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_history(vm, ctx)?;
    Ok(s("auto"))
}

// =================================================================================================
// Navigator, Screen
// =================================================================================================

pub const USER_AGENT: &str = "UnaOS Aether/0.1.0";

fn navigator_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let doc = dom::main_doc();
    Ok(dom::cached(vm, doc, SO_NAVIGATOR, |vm| {
        let p = iface("Navigator").unwrap().proto;
        host_obj(vm, p, T_NAVIGATOR, vec![])
    }))
}

fn nav_str(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    Ok(s(match which.as_str() {
        "userAgent" => USER_AGENT,
        "appVersion" => "0.1.0",
        "appName" => "Netscape",
        "appCodeName" => "Mozilla",
        "platform" => "UnaOS",
        "product" => "Gecko",
        "productSub" => "20030107",
        "vendor" => "UnaOS",
        "language" => "en-US",
        _ => "",
    }))
}

fn nav_languages(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.new_array(vec![s("en-US"), s("en")]);
    Ok(Value::Object(a))
}

fn nav_cores(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(num(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64))
}

fn nav_send_beacon(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let _ = vm;
    crate::ledger::record_js("navigator.sendBeacon-not-sent");
    Ok(Value::Bool(true))
}

fn screen_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let doc = dom::main_doc();
    Ok(dom::cached(vm, doc, SO_SCREEN, |vm| {
        let p = iface("Screen").unwrap().proto;
        host_obj(vm, p, T_SCREEN, vec![])
    }))
}

fn screen_num(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    let (w, h) = page(|p| p.viewport);
    Ok(num(match which.as_str() {
        "width" | "availWidth" => w as f64,
        "height" | "availHeight" => h as f64,
        "colorDepth" | "pixelDepth" => 24.0,
        _ => 0.0,
    }))
}

// =================================================================================================
// Document members that belong to the page
// =================================================================================================

fn this_doc(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    match dom::node_of(vm, &ctx.this) {
        Some(n) if dom::kind(n) == dom::NK::Document => Ok(n),
        _ => vm.throw_type("Illegal invocation"),
    }
}

fn ready_state(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if d != dom::main_doc() {
        return Ok(s("complete"));
    }
    Ok(s(page(|p| p.ready_state)))
}

fn current_script_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if d != dom::main_doc() {
        return Ok(Value::Null);
    }
    let c = page(|p| p.current_script);
    Ok(dom::wrap_opt(vm, c))
}

fn default_view(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    Ok(if d == dom::main_doc() { Value::Object(vm.realm().global) } else { Value::Null })
}

fn cookie_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if d != dom::main_doc() {
        return Ok(s(""));
    }
    crate::ledger::record_dom("document.cookie:get");
    Ok(s(&crate::net::cookies_for(&super::page_url())))
}

fn cookie_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    if d == dom::main_doc() && !v.trim().is_empty() {
        crate::net::set_cookie(&super::page_url(), &v);
        crate::ledger::record_dom("document.cookie:set");
    }
    Ok(Value::Undefined)
}

fn doc_str(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    let which = callee_str(vm, ctx);
    Ok(s(&match which.as_str() {
        "referrer" => String::new(),
        "domain" => current_url().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default(),
        "visibilityState" => "visible".into(),
        "designMode" => "off".into(),
        "lastModified" => {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let days = secs / 86400;
            let (y, m, dd) = civil(days as i64);
            let t = secs % 86400;
            format!("{:02}/{:02}/{:04} {:02}:{:02}:{:02}", m, dd, y, t / 3600, (t / 60) % 60, t % 60)
        }
        _ => String::new(),
    }))
}

/// Days since 1970-01-01 → (year, month, day) (proleptic Gregorian).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn doc_hidden(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    Ok(Value::Bool(false))
}

fn doc_has_focus(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    Ok(Value::Bool(true))
}

fn active_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if d != dom::main_doc() {
        return Ok(Value::Null);
    }
    let f = page(|p| p.focused).filter(|&f| dom::is_connected(f)).or_else(|| dom::body_of(d)).or_else(|| dom::document_element_of(d));
    Ok(dom::wrap_opt(vm, f))
}

fn doc_location(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if d != dom::main_doc() {
        return Ok(Value::Null);
    }
    Ok(location_obj(vm))
}

fn doc_location_put(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    location_put(vm, ctx)
}

fn doc_dir_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let v = dom::document_element_of(d).and_then(|h| dom::attr_value(h, "dir")).unwrap_or_default().to_ascii_lowercase();
    Ok(s(if matches!(v.as_str(), "ltr" | "rtl" | "auto") { &v } else { "" }))
}

fn doc_dir_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    if let Some(h) = dom::document_element_of(d) {
        dom::set_attr(vm, h, "dir", &v);
    }
    Ok(Value::Undefined)
}

fn scrolling_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let e = dom::document_element_of(d);
    Ok(dom::wrap_opt(vm, e))
}

fn doc_write(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let mut text = String::new();
    for i in 0..ctx.argc {
        text.push_str(&string(vm, &arg(vm, ctx, i))?);
    }
    if callee_str(vm, ctx) == "writeln" {
        text.push('\n');
    }
    super::loader::document_write(vm, d, &text)?;
    Ok(Value::Undefined)
}

fn doc_open(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    super::loader::document_open(vm, d)?;
    Ok(Value::Object(ctx.this.as_object().unwrap()))
}

fn doc_close(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    super::loader::document_close(vm, d)?;
    Ok(Value::Undefined)
}

fn get_selection(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    crate::ledger::record_dom("getSelection-stub");
    let o = vm.new_plain_object();
    vm.create_data_property(o, PropertyKey::from_str("rangeCount"), num(0.0))?;
    vm.create_data_property(o, PropertyKey::from_str("isCollapsed"), Value::Bool(true))?;
    vm.create_data_property(o, PropertyKey::from_str("type"), s("None"))?;
    Ok(Value::Object(o))
}

// =================================================================================================
// WindowProperties (named access on the Window, HTML §7.2.4)
// =================================================================================================

thread_local! {
    static NAMED_HANDLER: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

/// The element `window[name]` names: the first element with that id, or an `embed`/`form`/`img`/
/// `object` with that name.
fn named_element(name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let doc = dom::main_doc();
    with_doc(|d| {
        d.query_first(html_core::NodeId(doc), |d, i| {
            d.element(i).is_some_and(|e| {
                e.attr("id") == Some(name)
                    || (e.ns == html_core::Namespace::Html
                        && matches!(e.local.as_str(), "embed" | "form" | "img" | "object")
                        && e.attr("name") == Some(name))
            })
        })
        .map(|i| i.0)
    })
}

fn wp_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Option<String>> {
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    Ok(match key {
        PropertyKey::Str(s) => Some(s.to_rust()),
        _ => None,
    })
}

fn wp_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0).as_object().unwrap();
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let recv = vm.arg(ctx, 2);
    if vm.has_property(t, &key)? {
        return vm.get_with_receiver(t, &key, &recv);
    }
    if let Some(n) = wp_name(vm, ctx)? {
        if let Some(el) = named_element(&n) {
            return Ok(wrap(vm, el));
        }
    }
    Ok(Value::Undefined)
}

fn wp_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0).as_object().unwrap();
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if vm.has_property(t, &key)? {
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(wp_name(vm, ctx)?.and_then(|n| named_element(&n)).is_some()))
}

fn wp_gopd(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0).as_object().unwrap();
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(d) = vm.get_own_property(t, &key)? {
        return Ok(vm.from_property_descriptor(&d));
    }
    if let Some(el) = wp_name(vm, ctx)?.and_then(|n| named_element(&n)) {
        let v = wrap(vm, el);
        let o = vm.new_plain_object();
        vm.create_data_property(o, PropertyKey::from_str("value"), v)?;
        vm.create_data_property(o, PropertyKey::from_str("writable"), Value::Bool(true))?;
        vm.create_data_property(o, PropertyKey::from_str("enumerable"), Value::Bool(false))?;
        vm.create_data_property(o, PropertyKey::from_str("configurable"), Value::Bool(true))?;
        return Ok(Value::Object(o));
    }
    Ok(Value::Undefined)
}

fn wp_define(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let _ = vm;
    Ok(Value::Bool(false))
}

fn wp_delete(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

// =================================================================================================
// Installation
// =================================================================================================

fn global_accessor(vm: &mut Vm, name: &str, get: NativeFn, set: Option<NativeFn>, data: Option<&str>, unforgeable: bool) {
    let g = vm.realm().global;
    let fp = vm.intr().function_proto;
    let slots = data.map(|d| vec![s(d)]).unwrap_or_default();
    let gf = vm.make_native_with(&format!("get {name}"), 0, get, false, Some(fp), slots.clone());
    let sf = set.map(|f| vm.make_native_with(&format!("set {name}"), 1, f, false, Some(fp), slots));
    let flags = if unforgeable { E } else { E | C };
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(gf), sf), flags });
}

fn global_op(vm: &mut Vm, name: &str, len: u32, f: NativeFn, data: Option<&str>) {
    let g = vm.realm().global;
    let fp = vm.intr().function_proto;
    let slots = data.map(|d| vec![s(d)]).unwrap_or_default();
    let fo = vm.make_native_with(name, len, f, false, Some(fp), slots);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
}

/// [Replaceable] attributes (`self`, `innerWidth`, …): a setter that defines an own data property.
fn replaceable_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = callee_str(vm, ctx);
    let v = arg(vm, ctx, 0);
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(&name), Prop::data(v, WEC));
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let et = iface("EventTarget").unwrap();
    // WindowProperties: a proxy (named access) between Window.prototype and EventTarget.prototype.
    let target = vm.new_object(Some(et.proto));
    let handler = vm.new_plain_object();
    for (name, f, len) in [
        ("get", wp_get as NativeFn, 3),
        ("has", wp_has, 2),
        ("getOwnPropertyDescriptor", wp_gopd, 2),
        ("defineProperty", wp_define, 3),
        ("deleteProperty", wp_delete, 2),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(handler).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    }
    let wp = vm.alloc(ObjectData::new(
        None,
        Kind::Proxy(Some(Box::new(ProxyData { target, handler, callable: false, ctor: false, revoked: false }))),
    ));
    root(vm, Value::Object(wp));
    NAMED_HANDLER.with(|h| h.set(Some(handler)));
    let window = interface(vm, "Window", Some(et), None);
    vm.heap.get_mut(window.proto).proto = Some(wp);
    register_iface("Window", window);
    let g = vm.realm().global;
    vm.heap.get_mut(g).proto = Some(window.proto);

    // [LegacyUnforgeable] window, document, location, top.
    global_accessor(vm, "window", global_self, None, None, true);
    global_accessor(vm, "document", global_document, None, None, true);
    global_accessor(vm, "location", location_get, Some(location_put), None, true);
    global_accessor(vm, "top", global_self, None, None, true);
    for name in ["self", "frames", "parent"] {
        global_accessor(vm, name, global_self, Some(replaceable_set), Some(name), false);
    }
    for name in ["innerWidth", "innerHeight", "outerWidth", "outerHeight", "scrollX", "scrollY", "pageXOffset", "pageYOffset", "devicePixelRatio", "screenX", "screenY", "screenLeft", "screenTop"] {
        global_accessor(vm, name, global_number, Some(replaceable_set), Some(name), false);
    }
    global_accessor(vm, "length", global_zero, Some(replaceable_set), Some("length"), false);
    global_accessor(vm, "opener", global_null, Some(replaceable_set), Some("opener"), false);
    global_accessor(vm, "frameElement", global_null, None, None, false);
    global_accessor(vm, "closed", global_false, None, None, false);
    global_accessor(vm, "isSecureContext", global_true, None, None, false);
    global_accessor(vm, "origin", global_origin, Some(replaceable_set), Some("origin"), false);
    global_accessor(vm, "history", history_get, None, None, false);
    global_accessor(vm, "navigator", navigator_get, None, None, false);
    global_accessor(vm, "screen", screen_get, Some(replaceable_set), Some("screen"), false);
    data(vm, g, "name", s(""), WEC);
    data(vm, g, "status", s(""), WEC);

    global_op(vm, "setTimeout", 1, set_timeout, None);
    global_op(vm, "setInterval", 1, set_interval, None);
    global_op(vm, "clearTimeout", 0, clear_timer, None);
    global_op(vm, "clearInterval", 0, clear_timer, None);
    global_op(vm, "requestAnimationFrame", 1, request_animation_frame, None);
    global_op(vm, "cancelAnimationFrame", 1, cancel_animation_frame, None);
    global_op(vm, "queueMicrotask", 1, queue_microtask, None);
    global_op(vm, "getComputedStyle", 1, super::cssom::get_computed_style, None);
    global_op(vm, "matchMedia", 1, match_media, None);
    global_op(vm, "btoa", 1, btoa, None);
    global_op(vm, "atob", 1, atob, None);
    global_op(vm, "reportError", 1, report_error, None);
    global_op(vm, "postMessage", 1, post_message, None);
    global_op(vm, "getSelection", 0, get_selection, None);
    for name in ["alert", "confirm", "prompt", "open", "print", "stop", "focus", "blur", "close"] {
        global_op(vm, name, 0, dialog_stub, Some(name));
    }
    for name in ["scroll", "scrollTo", "scrollBy"] {
        global_op(vm, name, 0, window_scroll, Some(name));
    }
    super::events::install_handlers(vm, g, super::events::GLOBAL_HANDLERS);
    super::events::install_handlers(vm, g, super::events::WINDOW_HANDLERS);

    // console
    let op_ = vm.intr().object_proto;
    let console = vm.new_object(Some(op_));
    let fp = vm.intr().function_proto;
    for name in ["log", "info", "warn", "error", "debug", "trace", "dir", "dirxml", "table"] {
        let f = vm.make_native_with(name, 0, console_method, false, Some(fp), vec![s(name)]);
        vm.heap.get_mut(console).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(f), WEC));
    }
    let f = vm.make_native("assert", 0, console_assert, false);
    vm.heap.get_mut(console).props.insert(PropertyKey::from_str("assert"), Prop::data(Value::Object(f), WEC));
    for name in ["group", "groupCollapsed", "groupEnd", "time", "timeEnd", "timeLog", "count", "countReset", "clear", "profile", "profileEnd", "timeStamp"] {
        let f = vm.make_native(name, 0, console_noop, false);
        vm.heap.get_mut(console).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(f), WEC));
    }
    let tag = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    vm.heap.get_mut(console).props.insert(tag, Prop::data(s("console"), C));
    data(vm, g, "console", Value::Object(console), WC);

    // performance
    let perf_i = interface(vm, "Performance", Some(et), None);
    register_iface("Performance", perf_i);
    op(vm, perf_i.proto, "now", 0, performance_now);
    attr(vm, perf_i.proto, "timeOrigin", performance_time_origin, None);
    let perf = vm.new_object(Some(perf_i.proto));
    data(vm, g, "performance", Value::Object(perf), WEC);

    // MediaQueryList
    let mql = interface(vm, "MediaQueryList", Some(et), None);
    register_iface("MediaQueryList", mql);
    attr(vm, mql.proto, "matches", mql_matches, None);
    attr(vm, mql.proto, "media", mql_media, None);
    op(vm, mql.proto, "addListener", 1, mql_listener);
    op(vm, mql.proto, "removeListener", 1, mql_listener);

    // Location
    let loc = interface(vm, "Location", None, None);
    register_iface("Location", loc);
    for part in ["href", "origin", "protocol", "host", "hostname", "port", "pathname", "search", "hash"] {
        let setter = if part == "origin" { None } else { Some(loc_part_set as NativeFn) };
        attr_with(vm, loc.proto, part, loc_part_get, setter, s(part));
    }
    op(vm, loc.proto, "assign", 1, loc_assign);
    op(vm, loc.proto, "replace", 1, loc_replace);
    op(vm, loc.proto, "reload", 0, loc_reload);
    let ts = vm.make_native_with("toString", 0, loc_part_get, false, Some(fp), vec![s("href")]);
    vm.heap.get_mut(loc.proto).props.insert(PropertyKey::from_str("toString"), Prop::data(Value::Object(ts), WEC));

    // History
    let hist = interface(vm, "History", None, None);
    register_iface("History", hist);
    attr(vm, hist.proto, "length", history_length, None);
    attr(vm, hist.proto, "state", history_state, None);
    attr(vm, hist.proto, "scrollRestoration", history_scroll_restoration, None);
    idl::op_with(vm, hist.proto, "pushState", 2, history_push, s("pushState"));
    idl::op_with(vm, hist.proto, "replaceState", 2, history_push, s("replaceState"));
    for name in ["back", "forward", "go"] {
        op(vm, hist.proto, name, 0, history_nav);
    }

    // Navigator, Screen
    let nav = interface(vm, "Navigator", None, None);
    register_iface("Navigator", nav);
    for name in ["userAgent", "appVersion", "appName", "appCodeName", "platform", "product", "productSub", "vendor", "language"] {
        attr_with(vm, nav.proto, name, nav_str, None, s(name));
    }
    attr(vm, nav.proto, "languages", nav_languages, None);
    attr(vm, nav.proto, "hardwareConcurrency", nav_cores, None);
    attr(vm, nav.proto, "onLine", global_true, None);
    attr(vm, nav.proto, "cookieEnabled", global_true, None);
    attr(vm, nav.proto, "webdriver", global_false, None);
    attr(vm, nav.proto, "pdfViewerEnabled", global_false, None);
    attr(vm, nav.proto, "maxTouchPoints", global_zero, None);
    attr(vm, nav.proto, "doNotTrack", global_null, None);
    op(vm, nav.proto, "javaEnabled", 0, global_false);
    op(vm, nav.proto, "sendBeacon", 1, nav_send_beacon);
    let scr = interface(vm, "Screen", None, None);
    register_iface("Screen", scr);
    for name in ["width", "height", "availWidth", "availHeight", "colorDepth", "pixelDepth"] {
        attr_with(vm, scr.proto, name, screen_num, None, s(name));
    }

    // Document members that are the page's.
    let dp = iface("Document").unwrap().proto;
    attr(vm, dp, "readyState", ready_state, None);
    attr(vm, dp, "currentScript", current_script_get, None);
    attr(vm, dp, "defaultView", default_view, None);
    attr(vm, dp, "cookie", cookie_get, Some(cookie_set));
    for name in ["referrer", "domain", "lastModified", "visibilityState", "designMode"] {
        attr_with(vm, dp, name, doc_str, None, s(name));
    }
    attr(vm, dp, "hidden", doc_hidden, None);
    op(vm, dp, "hasFocus", 0, doc_has_focus);
    attr(vm, dp, "activeElement", active_element, None);
    attr(vm, dp, "location", doc_location, Some(doc_location_put));
    attr(vm, dp, "dir", doc_dir_get, Some(doc_dir_set));
    attr(vm, dp, "scrollingElement", scrolling_element, None);
    attr(vm, dp, "fullscreenElement", global_null, None);
    attr(vm, dp, "pointerLockElement", global_null, None);
    idl::op_with(vm, dp, "write", 0, doc_write, s("write"));
    idl::op_with(vm, dp, "writeln", 0, doc_write, s("writeln"));
    op(vm, dp, "open", 0, doc_open);
    op(vm, dp, "close", 0, doc_close);
    op(vm, dp, "createEvent", 1, super::events::create_event);
    op(vm, dp, "getSelection", 0, get_selection);
    super::events::install_handlers(vm, dp, super::events::GLOBAL_HANDLERS);
    super::events::install_handlers(vm, dp, &["readystatechange", "visibilitychange", "DOMContentLoaded"]);

    // `__ledger(name)`: lets the JS preludes record their own honest gaps in the page's ledger.
    global_op(vm, "__ledger", 1, ledger_native, None);
    let lk = PropertyKey::from_str("__ledger");
    if let Some(p) = vm.heap.get_mut(g).props.get_mut(&lk) {
        p.flags = WC;
    }

    // The platform preludes: URL/URLSearchParams, TextEncoder/Decoder, AbortController, DOMParser,
    // crypto, fetch/XMLHttpRequest, storage, MessageChannel, the observer stubs, structuredClone.
    crate::api::fetch::init(vm);
    crate::api::platform::init(vm);
}
