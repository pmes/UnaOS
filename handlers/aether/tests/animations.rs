//! AETHERSTYLE3C (LEDGER SR122): the Web Animations items SR115 refused, through the page's script binding
//! (js/animations.rs over the shared timeline). Each test asserts what a page reads back: `composite: 'add'`
//! reported by the effect and composited into what `commitStyles()` writes to the inline style (opacity 0.5
//! over 0.25 → "0.75"), `iterationStart` in the computed timing (progress 0.1 of iteration 1 at 600 ms of a
//! 0.5-start animation) and in the value commitStyles writes ("10px"), a negative `iterationStart` a TypeError,
//! a new animation pending with a null `startTime`; a CSSTransition paused (`playState` "paused", pending until
//! the next frame, `ready` resolving with `pending` false after the engine's frame) and played again.

fn load(url: &str, html: &str, budget_ms: u64) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    engine.load_html(url, html, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, budget_ms);
    }
    engine
}

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    let js = engine.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|e| format!("THROW {e}"))
}

/// Web Animations 1 §5.4.4 (composite), §4.8.2 (iterationStart), §4.4.20 (commitStyles), §4.4.11 (pending).
#[test]
fn composite_iteration_start_and_commit_styles() {
    let html = r#"<!doctype html><html><body><div id="t" style="opacity: 0.25"></div><div id="u"></div></body></html>"#;
    let mut e = load("http://animations.test/1", html, 1000);
    let got = eval(
        &mut e,
        r#"(function () {
            var el = document.getElementById('t');
            var a = el.animate([{ opacity: 0.5 }, { opacity: 0.5 }], { duration: 100000, composite: 'add' });
            var out = [a.effect.composite, a.pending, a.startTime === null];
            a.pause();
            a.currentTime = 50000;
            a.commitStyles();
            out.push(el.style.opacity);
            var u = document.getElementById('u');
            var b = u.animate([{ marginLeft: '0px' }, { marginLeft: '100px' }], { duration: 1000, iterations: 2, iterationStart: 0.5, fill: 'forwards' });
            b.pause();
            b.currentTime = 600;
            var ct = b.effect.getComputedTiming();
            out.push(Math.round(ct.progress * 1000) / 1000, ct.currentIteration, b.effect.getTiming().iterationStart);
            b.commitStyles();
            out.push(u.style.marginLeft);
            try { u.animate([], { iterationStart: -1 }); out.push('no throw'); } catch (err) { out.push(err.name); }
            return JSON.stringify(out);
        })()"#,
    );
    assert_eq!(got, r#"["add",true,true,"0.75",0.1,1,0.5,"10px","TypeError"]"#);
}

/// css-transitions-2 §6 (CSSTransition is an Animation): pause() holds it, play() resumes it; Web
/// Animations 1 §4.4.11: pause-pending until the next frame, then `ready` resolves.
#[test]
fn css_transition_pause_play_and_ready() {
    let html = r#"<!doctype html><html><head><style>#t { width: 100px; transition: width 10s linear }</style></head><body><div id="t"></div></body></html>"#;
    let mut e = load("http://animations.test/2", html, 1000);
    assert_eq!(eval(&mut e, "document.getElementById('t').style.width = '200px'; 'ok'"), "ok");
    e.tick(); // the mutation relayouts: the transition starts
    let got = eval(
        &mut e,
        r#"(function () {
            var a = document.getElementById('t').getAnimations()[0];
            window.__a = a;
            window.__log = [];
            a.pause();
            a.ready.then(function (x) { __log.push('ready ' + (x === a) + ' ' + a.pending); });
            return JSON.stringify([a instanceof CSSTransition, a.transitionProperty, a.playState, a.pending]);
        })()"#,
    );
    assert_eq!(got, r#"[true,"width","paused",true]"#);
    e.tick(); // the frame resolves the pending pause
    e.tick(); // the script lane resolves `ready`
    let got = eval(&mut e, "JSON.stringify([__log, __a.pending, __a.playState, __a.currentTime === __a.currentTime])");
    assert_eq!(got, r#"[["ready true false"],false,"paused",true]"#);
    assert_eq!(eval(&mut e, "__a.play(); JSON.stringify([__a.playState, __a.pending])"), r#"["running",true]"#);
}
