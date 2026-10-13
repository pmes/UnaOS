//! AETHERMEDIA2 (LEDGER SR134): the media element's script extras — text tracks (HTML §4.8.11.11:
//! `TextTrackList`, `TextTrack`, `TextTrackCueList`, `TextTrackCue`, `VTTCue`, `textTracks`,
//! `addTextTrack`, `HTMLTrackElement.track` / `readyState`, `cuechange` / `enter` / `exit`) over
//! `crate::media::tracks`; `HTMLVideoElement.requestVideoFrameCallback` (WICG video-rvfc) over
//! `crate::media::frame_info`; and the refusals by name: Picture-in-Picture (`requestPictureInPicture`
//! rejects NotSupportedError, `document.pictureInPictureEnabled` is false) and
//! `HTMLMediaElement.captureStream` (throws NotSupportedError) — UnaOS has no floating media window
//! and no MediaStream.
//!
//! CHARTER: Aether — binding.

use super::idl::{arg, num, s};
use js_core::vm::*;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn num_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<f64> {
    let v = arg(vm, ctx, i);
    vm.to_number(&v)
}

fn serial(vm: &Vm, v: &Value) -> Option<u32> {
    match super::events::target_of(vm, v) {
        Some(super::TargetKey::Other(n)) => Some(n),
        _ => None,
    }
}

fn node(vm: &Vm, v: &Value) -> Option<crate::dom::NodeRef> {
    super::dom::node_of(vm, v).map(super::dom::node_ref)
}

fn array(vm: &mut Vm, v: Vec<Value>) -> Value {
    Value::Object(vm.new_array(v))
}

fn cue_rows(vm: &mut Vm, t: &crate::media::tracks::Track, active: bool) -> Value {
    let mut rows = Vec::new();
    for c in t.cues.iter().filter(|c| !active || t.active.contains(&c.key)) {
        let row = vec![num(c.key as f64), s(&c.id), num(c.start_ns as f64 / 1e9), num(c.end_ns as f64 / 1e9), s(&c.text), Value::Bool(c.pause_on_exit)];
        rows.push(array(vm, row));
    }
    array(vm, rows)
}

/// `__tt(op, …)`: the text-track family's one native.
fn tt_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::media::tracks as t;
    let op = str_arg(vm, ctx, 0)?;
    Ok(match op.as_str() {
        "list" => {
            let Some(m) = node(vm, &arg(vm, ctx, 1)) else { return Ok(array(vm, Vec::new())) };
            t::scan(&m, &super::page_url());
            let keys = t::list(&m).into_iter().map(|k| num(k as f64)).collect();
            array(vm, keys)
        }
        "info" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            match t::get(key) {
                Some(x) => {
                    let row = vec![s(&x.kind), s(&x.label), s(&x.language), s(&x.id), s(x.mode.as_str()), num(x.ready as f64)];
                    array(vm, row)
                }
                None => Value::Null,
            }
        }
        "bind" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            if let Some(sr) = serial(vm, &arg(vm, ctx, 2)) {
                t::bind(key, sr);
            }
            Value::Undefined
        }
        "mode" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            let m = match str_arg(vm, ctx, 2)?.as_str() {
                "showing" => t::Mode::Showing,
                "hidden" => t::Mode::Hidden,
                _ => t::Mode::Disabled,
            };
            t::set_mode(key, m);
            Value::Undefined
        }
        "add" => {
            let Some(m) = node(vm, &arg(vm, ctx, 1)) else { return Ok(num(-1.0)) };
            let (k, l, g) = (str_arg(vm, ctx, 2)?, str_arg(vm, ctx, 3)?, str_arg(vm, ctx, 4)?);
            num(t::add(&m, &k, &l, &g) as f64)
        }
        "cues" | "active" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            match t::get(key) {
                Some(x) => cue_rows(vm, &x, op == "active"),
                None => array(vm, Vec::new()),
            }
        }
        "addCue" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            let ck = num_arg(vm, ctx, 2)?;
            let id = str_arg(vm, ctx, 3)?;
            let (a, b) = (num_arg(vm, ctx, 4)?, num_arg(vm, ctx, 5)?);
            let text = str_arg(vm, ctx, 6)?;
            let pause = super::idl::boolean(vm, &arg(vm, ctx, 7));
            let target = serial(vm, &arg(vm, ctx, 8));
            num(t::add_cue(key, (ck >= 0.0).then_some(ck as u32), &id, a, b, &text, pause, target) as f64)
        }
        "removeCue" => {
            let key = num_arg(vm, ctx, 1)? as u32;
            let ck = num_arg(vm, ctx, 2)? as u32;
            Value::Bool(t::remove_cue(key, ck))
        }
        "setCue" => {
            let ck = num_arg(vm, ctx, 1)? as u32;
            let field = str_arg(vm, ctx, 2)?;
            let n = num_arg(vm, ctx, 3)?;
            let text = str_arg(vm, ctx, 4)?;
            t::set_cue(ck, &field, n, &text);
            Value::Undefined
        }
        "bindCue" => {
            let ck = num_arg(vm, ctx, 1)? as u32;
            if let Some(sr) = serial(vm, &arg(vm, ctx, 2)) {
                t::bind_cue(ck, sr);
            }
            Value::Undefined
        }
        "forElement" => {
            let Some(el) = node(vm, &arg(vm, ctx, 1)) else { return Ok(num(-1.0)) };
            if let Some(m) = el.parent().filter(|p| p.as_element().is_some_and(|e| matches!(e.name.local.as_ref(), "video" | "audio"))) {
                t::scan(&m, &super::page_url());
            }
            num(t::for_element(&el).map(|k| k as f64).unwrap_or(-1.0))
        }
        "frame" => {
            let Some(el) = node(vm, &arg(vm, ctx, 1)) else { return Ok(Value::Null) };
            match crate::media::frame_info(&el) {
                Some((n, t, w, h)) => {
                    let row = vec![num(n as f64), num(t), num(w as f64), num(h as f64)];
                    array(vm, row)
                }
                None => {
                    let row = vec![num(0.0), num(0.0), num(0.0), num(0.0)];
                    array(vm, row)
                }
            }
        }
        "refuse" => {
            let what = str_arg(vm, ctx, 1)?;
            crate::ledger::record_js(&format!("media-refused:{}", super::clip(&what, 48)));
            Value::Undefined
        }
        _ => return vm.throw_type("__tt: unknown operation"),
    })
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__tt", 4, tt_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__tt"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("tracks-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] tracks prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var TOKEN = {};
    var T = __tt;
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var H = new WeakMap();
    var handlers = function (proto, names) {
        names.forEach(function (n) {
            Object.defineProperty(proto, 'on' + n, {
                configurable: true, enumerable: true,
                get: function () { var h = H.get(this); return (h && h[n]) || null; },
                set: function (f) {
                    var h = H.get(this);
                    if (!h) { h = {}; H.set(this, h); }
                    var first = !(n in h);
                    h[n] = (typeof f === 'function') ? f : null;
                    if (first) {
                        var self = this;
                        this.addEventListener(n, function (e) { var f = h[n]; if (typeof f === 'function') { return f.call(self, e); } });
                    }
                },
            });
        });
    };
    var KINDS = ['subtitles', 'captions', 'descriptions', 'chapters', 'metadata'];

    // ---- cues
    var C = new WeakMap();      // cue -> { key, track, id, start, end, text, pause }
    var CUES = new Map();       // cue key -> cue object
    var sync = function (cue, field, n, t) { var st = C.get(cue); if (st && st.key >= 0) { T('setCue', st.key, field, n, t); } };
    class TextTrackCue extends EventTarget {
        constructor(tok) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
        }
        get track() { var st = C.get(this); return st ? st.track : null; }
        get id() { return C.get(this).id; }
        set id(v) { C.get(this).id = String(v); sync(this, 'id', 0, String(v)); }
        get startTime() { return C.get(this).start; }
        set startTime(v) { v = Number(v); C.get(this).start = v; sync(this, 'startTime', v, ''); }
        get endTime() { return C.get(this).end; }
        set endTime(v) { v = Number(v); C.get(this).end = v; sync(this, 'endTime', v, ''); }
        get pauseOnExit() { return C.get(this).pause; }
        set pauseOnExit(v) { v = !!v; C.get(this).pause = v; sync(this, 'pauseOnExit', v ? 1 : 0, ''); }
    }
    handlers(TextTrackCue.prototype, ['enter', 'exit']);
    class VTTCue extends TextTrackCue {
        constructor(startTime, endTime, text) {
            if (arguments.length < 3) { throw new TypeError("Failed to construct 'VTTCue': 3 arguments required, but only " + arguments.length + ' present.'); }
            super(TOKEN);
            C.set(this, { key: -1, track: null, id: '', start: Number(startTime), end: Number(endTime), text: String(text), pause: false,
                          vertical: '', snapToLines: true, line: 'auto', lineAlign: 'start', position: 'auto', positionAlign: 'auto', size: 100, align: 'center', region: null });
        }
        get text() { return C.get(this).text; }
        set text(v) { C.get(this).text = String(v); sync(this, 'text', 0, String(v)); }
        getCueAsHTML() {
            var f = document.createDocumentFragment();
            f.appendChild(document.createTextNode(C.get(this).text.replace(/<[^>]*>/g, '')));
            return f;
        }
    }
    ['vertical', 'snapToLines', 'line', 'lineAlign', 'position', 'positionAlign', 'size', 'align', 'region'].forEach(function (p) {
        Object.defineProperty(VTTCue.prototype, p, {
            configurable: true, enumerable: true,
            get: function () { return C.get(this)[p]; },
            set: function (v) { C.get(this)[p] = v; },
        });
    });
    var cueFor = function (row, track) {
        var key = row[0];
        var c = CUES.get(key);
        if (!c) {
            c = new VTTCue(row[2], row[3], row[4]);
            var st = C.get(c);
            st.key = key; st.id = row[1]; st.pause = row[5];
            CUES.set(key, c);
            T('bindCue', key, c);
        }
        C.get(c).track = track;
        return c;
    };
    class TextTrackCueList {
        constructor(tok, list) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            Object.defineProperty(this, '_l', { value: list, enumerable: false });
            for (var i = 0; i < list.length; i++) { Object.defineProperty(this, i, { value: list[i], enumerable: true }); }
        }
        get length() { return this._l.length; }
        getCueById(id) { id = String(id); for (var i = 0; i < this._l.length; i++) { if (this._l[i].id === id && id !== '') { return this._l[i]; } } return null; }
        [Symbol.iterator]() { return this._l[Symbol.iterator](); }
    }

    // ---- tracks
    var TR = new Map();         // track key -> TextTrack
    var K = new WeakMap();      // TextTrack -> key
    class TextTrack extends EventTarget {
        constructor(tok, key) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            K.set(this, key);
            T('bind', key, this);
        }
        get kind() { return T('info', K.get(this))[0]; }
        get label() { return T('info', K.get(this))[1]; }
        get language() { return T('info', K.get(this))[2]; }
        get id() { return T('info', K.get(this))[3]; }
        get inBandMetadataTrackDispatchType() { return ''; }
        get mode() { return T('info', K.get(this))[4]; }
        set mode(v) {
            v = String(v);
            if (v !== 'disabled' && v !== 'hidden' && v !== 'showing') { return; }
            T('mode', K.get(this), v);
        }
        get cues() {
            if (this.mode === 'disabled') { return null; }
            var self = this;
            return new TextTrackCueList(TOKEN, T('cues', K.get(this)).map(function (r) { return cueFor(r, self); }));
        }
        get activeCues() {
            if (this.mode === 'disabled') { return null; }
            var self = this;
            return new TextTrackCueList(TOKEN, T('active', K.get(this)).map(function (r) { return cueFor(r, self); }));
        }
        addCue(cue) {
            if (!(cue instanceof TextTrackCue)) { throw new TypeError("Failed to execute 'addCue' on 'TextTrack': parameter 1 is not of type 'TextTrackCue'."); }
            var st = C.get(cue);
            var key = T('addCue', K.get(this), st.key, st.id, st.start, st.end, st.text || '', st.pause, cue);
            st.key = key; st.track = this;
            CUES.set(key, cue);
        }
        removeCue(cue) {
            var st = C.get(cue);
            if (!st || st.key < 0 || !T('removeCue', K.get(this), st.key)) {
                throw new DOMException("Failed to execute 'removeCue' on 'TextTrack': The specified cue is not listed in the TextTrack's list of cues.", 'NotFoundError');
            }
            st.track = null;
        }
    }
    handlers(TextTrack.prototype, ['cuechange']);
    var trackFor = function (key) {
        var t = TR.get(key);
        if (!t) { t = new TextTrack(TOKEN, key); TR.set(key, t); }
        return t;
    };
    class TextTrackList extends EventTarget {
        constructor(tok) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            Object.defineProperty(this, '_l', { value: [], enumerable: false });
        }
        get length() { return this._l.length; }
        getTrackById(id) { id = String(id); for (var i = 0; i < this._l.length; i++) { if (this._l[i].id === id) { return this._l[i]; } } return null; }
        [Symbol.iterator]() { return this._l[Symbol.iterator](); }
    }
    handlers(TextTrackList.prototype, ['change', 'addtrack', 'removetrack']);
    var LISTS = new WeakMap();
    var listFor = function (el) {
        var l = LISTS.get(el);
        if (!l) { l = new TextTrackList(TOKEN); LISTS.set(el, l); }
        var keys = T('list', el);
        for (var k = 0; k < l._l.length + 1; k++) { delete l[k]; }
        l._l.length = 0;
        keys.forEach(function (key, i) {
            var t = trackFor(key);
            l._l.push(t);
            Object.defineProperty(l, i, { value: t, configurable: true, enumerable: true });
        });
        return l;
    };
    hide('TextTrack', TextTrack);
    hide('TextTrackList', TextTrackList);
    hide('TextTrackCue', TextTrackCue);
    hide('TextTrackCueList', TextTrackCueList);
    hide('VTTCue', VTTCue);

    if (typeof HTMLMediaElement === 'function') {
        var mp = HTMLMediaElement.prototype;
        Object.defineProperty(mp, 'textTracks', { configurable: true, enumerable: true, get: function () { return listFor(this); } });
        mp.addTextTrack = function (kind, label, language) {
            kind = String(kind);
            if (KINDS.indexOf(kind) < 0) { throw new TypeError("Failed to execute 'addTextTrack' on 'HTMLMediaElement': The provided value '" + kind + "' is not a valid enum value of type TextTrackKind."); }
            var key = T('add', this, kind, label === undefined ? '' : String(label), language === undefined ? '' : String(language));
            var l = listFor(this);
            __mse('queue', l, 'addtrack');
            return trackFor(key);
        };
        // captureStream: no MediaStream in UnaOS — refused by name
        mp.captureStream = function () {
            T('refuse', 'captureStream');
            throw new DOMException("Failed to execute 'captureStream' on 'HTMLMediaElement': HTMLMediaElement.captureStream is not supported (no MediaStream).", 'NotSupportedError');
        };
    }
    if (typeof HTMLTrackElement === 'function') {
        var tp = HTMLTrackElement.prototype;
        [['NONE', 0], ['LOADING', 1], ['LOADED', 2], ['ERROR', 3]].forEach(function (c) {
            Object.defineProperty(HTMLTrackElement, c[0], { value: c[1], enumerable: true });
            Object.defineProperty(tp, c[0], { value: c[1], enumerable: true });
        });
        Object.defineProperty(tp, 'track', { configurable: true, enumerable: true, get: function () {
            var k = T('forElement', this);
            return k < 0 ? null : trackFor(k);
        } });
        Object.defineProperty(tp, 'readyState', { configurable: true, enumerable: true, get: function () {
            var k = T('forElement', this);
            return k < 0 ? 0 : T('info', k)[5];
        } });
    }

    if (typeof HTMLVideoElement === 'function') {
        var vp = HTMLVideoElement.prototype;
        // requestVideoFrameCallback (WICG): the callback runs in the first animation frame after a new
        // picture was presented, with that picture's metadata.
        var RVFC = new Map(), rvfcSeq = 0;
        vp.requestVideoFrameCallback = function (callback) {
            if (typeof callback !== 'function') { throw new TypeError("Failed to execute 'requestVideoFrameCallback' on 'HTMLVideoElement': The callback provided as parameter 1 is not a function."); }
            var id = ++rvfcSeq, el = this, start = T('frame', el)[0];
            RVFC.set(id, true);
            var step = function (now) {
                if (!RVFC.has(id)) { return; }
                var f = T('frame', el);
                if (f[0] > start) {
                    RVFC.delete(id);
                    callback.call(el, now, {
                        presentationTime: now, expectedDisplayTime: now + 1000 / 60,
                        width: f[2], height: f[3], mediaTime: f[1], presentedFrames: f[0], processingDuration: 0,
                    });
                } else {
                    requestAnimationFrame(step);
                }
            };
            requestAnimationFrame(step);
            return id;
        };
        vp.cancelVideoFrameCallback = function (handle) { RVFC.delete(Number(handle)); };
        // Picture-in-Picture: UnaOS has no floating media window — refused by name
        vp.requestPictureInPicture = function () {
            T('refuse', 'requestPictureInPicture');
            return Promise.reject(new DOMException("Failed to execute 'requestPictureInPicture' on 'HTMLVideoElement': Picture-in-Picture is not supported.", 'NotSupportedError'));
        };
        Object.defineProperty(vp, 'disablePictureInPicture', { configurable: true, enumerable: true,
            get: function () { return this.hasAttribute('disablepictureinpicture'); },
            set: function (v) { if (v) { this.setAttribute('disablepictureinpicture', ''); } else { this.removeAttribute('disablepictureinpicture'); } } });
        handlers(vp, ['enterpictureinpicture', 'leavepictureinpicture']);
    }
    if (typeof Document === 'function') {
        Object.defineProperty(Document.prototype, 'pictureInPictureEnabled', { configurable: true, enumerable: true, get: function () { return false; } });
        Object.defineProperty(Document.prototype, 'pictureInPictureElement', { configurable: true, enumerable: true, get: function () { return null; } });
        Document.prototype.exitPictureInPicture = function () {
            return Promise.reject(new DOMException("Failed to execute 'exitPictureInPicture' on 'Document': There is no Picture-in-Picture element in this document.", 'InvalidStateError'));
        };
    }
})();
"#;
