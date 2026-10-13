//! AETHERAPIS (LEDGER SR145): HTML §6.4 user activation — the page's sticky and transient activation and its
//! history-action activation, as the APIs that gate on a user gesture read them (Fullscreen §4 `requestFullscreen`,
//! the Clipboard API's write, CloseWatcher's "allowed number of groups", `navigator.userActivation`).
//!
//! CHARTER: Aether — binding.
//!
//! `note()` is the "activation notification steps" (HTML §6.4.3): a trusted click or a keydown that is not Escape or
//! a modifier (`js/events.rs`). The transient activation duration is 5 s of page time (`events::now_hr`, the clock
//! WPT's virtual time advances); `consume()` is "consume user activation". The history-action activation is set with
//! every activation and consumed by a close request (`js/closewatcher.rs`). `crate::media`'s sticky flag (autoplay)
//! is set from the same notification, so the two never disagree.

use super::idl::arg;
use js_core::vm::*;
use std::cell::Cell;

/// HTML §6.4's transient activation duration, as browsers set it.
const TRANSIENT_MS: f64 = 5000.0;

thread_local! {
    static LAST: Cell<Option<f64>> = const { Cell::new(None) };
    static STICKY: Cell<bool> = const { Cell::new(false) };
    static HISTORY: Cell<bool> = const { Cell::new(false) };
}

/// The activation notification steps: a user-interaction event the page received.
pub fn note() {
    LAST.with(|l| l.set(Some(super::events::now_hr())));
    STICKY.with(|s| s.set(true));
    HISTORY.with(|h| h.set(true));
    crate::media::note_user_activation();
}

/// The document has sticky activation (an activation ever happened).
pub fn has_sticky() -> bool {
    STICKY.with(Cell::get)
}

/// The document has transient activation (an activation within the last 5 s that was not consumed).
pub fn has_transient() -> bool {
    LAST.with(|l| l.get().is_some_and(|t| super::events::now_hr() - t <= TRANSIENT_MS))
}

/// "Consume user activation": true when there was transient activation to consume.
pub fn consume() -> bool {
    let had = has_transient();
    LAST.with(|l| l.set(None));
    had
}

/// The document has history-action activation (HTML §6.4.4).
pub fn has_history_action() -> bool {
    HISTORY.with(Cell::get) && has_transient()
}

/// "Consume history-action user activation".
pub fn consume_history_action() {
    HISTORY.with(|h| h.set(false));
}

pub fn reset() {
    LAST.with(|l| l.set(None));
    STICKY.with(|s| s.set(false));
    HISTORY.with(|h| h.set(false));
}

/// `__activation(op)`: `sticky`, `transient`, `consume`, `history`, `consume-history`.
fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    Ok(Value::Bool(match op.as_str() {
        "sticky" => has_sticky(),
        "transient" => has_transient(),
        "consume" => consume(),
        "history" => has_history_action(),
        "consume-history" => {
            consume_history_action();
            true
        }
        _ => false,
    }))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__activation", 1, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__activation"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("activation-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] activation prelude failed: {m}");
    }
}

/// HTML §6.4.5 `navigator.userActivation` (`UserActivation`: `hasBeenActive`, `isActive`).
const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    class UserActivation {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        get hasBeenActive() { return __activation('sticky'); }
        get isActive() { return __activation('transient'); }
    }
    Object.defineProperty(UserActivation.prototype, Symbol.toStringTag, { value: 'UserActivation', configurable: true });
    hide(globalThis, 'UserActivation', UserActivation);
    var ua = null;
    if (typeof Navigator === 'function') {
        Object.defineProperty(Navigator.prototype, 'userActivation', {
            get: function () { if (!ua) { ua = new UserActivation(TOKEN); } return ua; },
            configurable: true, enumerable: true
        });
    }
})();
"#;
