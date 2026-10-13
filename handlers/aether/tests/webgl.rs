//! WEBGL1 (LEDGER SR136): WebGL 1.0 in a page — the three.js-style smoke (two shaders compiled, a buffer
//! uploaded, a triangle drawn, readPixels asserting it), `getContext` returning one object, the
//! WEBGL_lose_context round trip with its events, and the canvas surface hand-off (toDataURL / drawImage of a
//! GL canvas).
//!
//! Run: `cargo test -p aether --test webgl`.

use aether::AetherEngine;

const URL: &str = "http://webgl.test/";

fn page(script: &str) -> AetherEngine {
    let html = format!("<!doctype html><html><body><canvas id=c width=64 height=64></canvas><script>{script}</script></body></html>");
    let mut e = AetherEngine::new();
    e.load_html(URL, &html, true);
    if let Some(js) = e.js_engine.as_mut() {
        aether::event_loop::settle(js, 200);
    }
    e
}

fn eval(e: &mut AetherEngine, src: &str) -> String {
    let js = e.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|err| format!("THROW {err}"))
}

fn settle(e: &mut AetherEngine) {
    if let Some(js) = e.js_engine.as_mut() {
        aether::event_loop::settle(js, 200);
    }
}

const DRAW: &str = r#"
var c = document.getElementById('c');
var gl = c.getContext('webgl');
function sh(type, src) { var s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) { throw new Error(gl.getShaderInfoLog(s)); } return s; }
var vs = sh(gl.VERTEX_SHADER, 'attribute vec2 position; uniform vec2 offset; void main() { gl_Position = vec4(position + offset, 0.0, 1.0); }');
var fs = sh(gl.FRAGMENT_SHADER, 'precision mediump float; uniform vec4 color; void main() { gl_FragColor = color; }');
var prog = gl.createProgram(); gl.attachShader(prog, vs); gl.attachShader(prog, fs); gl.linkProgram(prog);
var linked = gl.getProgramParameter(prog, gl.LINK_STATUS);
gl.useProgram(prog);
var buf = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, buf);
gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1]), gl.STATIC_DRAW);
var loc = gl.getAttribLocation(prog, 'position');
gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
gl.uniform2f(gl.getUniformLocation(prog, 'offset'), 0, 0);
gl.uniform4fv(gl.getUniformLocation(prog, 'color'), [1, 0, 0, 1]);
gl.clearColor(0, 0, 1, 1); gl.clear(gl.COLOR_BUFFER_BIT);
gl.drawArrays(gl.TRIANGLES, 0, 3);
function px(x, y) { var o = new Uint8Array(4); gl.readPixels(x, y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, o); return Array.from(o).join(','); }
"#;

/// The three.js-style smoke: compile, link, upload, draw; readPixels sees the triangle and the clear colour,
/// and the WebGL objects are the ones the queries return.
#[test]
fn smoke_draws_a_triangle() {
    let mut e = page(DRAW);
    let r = eval(
        &mut e,
        "[linked, gl.getError(), px(5, 5), px(60, 60), gl.getParameter(gl.ARRAY_BUFFER_BINDING) === buf, gl.getParameter(gl.CURRENT_PROGRAM) === prog,
          gl.getParameter(gl.VIEWPORT).join(' '), gl.drawingBufferWidth, gl.getActiveAttrib(prog, 0).name, gl.getActiveUniform(prog, 1).type === gl.FLOAT_VEC4,
          gl.getParameter(gl.VERSION).indexOf('WebGL 1.0') === 0, gl.getSupportedExtensions().join(' ')].join('|')",
    );
    assert_eq!(r, "true|0|255,0,0,255|0,0,255,255|true|true|0 0 64 64|64|position|true|true|OES_element_index_uint OES_standard_derivatives WEBGL_lose_context");
}

/// getContext('webgl') returns the same object every time (and for 'experimental-webgl'); a WebGL canvas
/// answers null for '2d' and a 2D canvas null for 'webgl'; the constants are on the interface.
#[test]
fn get_context_same_object() {
    let mut e = page("");
    let r = eval(
        &mut e,
        "var c = document.getElementById('c'); var a = c.getContext('webgl'); var b = c.getContext('webgl', { alpha: false });
         var d = document.createElement('canvas'); d.getContext('2d');
         [a === b, a === c.getContext('experimental-webgl'), a instanceof WebGLRenderingContext, a.canvas === c, c.getContext('2d') === null,
          d.getContext('webgl') === null, WebGLRenderingContext.COLOR_BUFFER_BIT === 0x4000, a.TRIANGLES === 4, a.getContextAttributes().alpha,
          a.getContextAttributes().antialias].join('|')",
    );
    assert_eq!(r, "true|true|true|true|true|true|true|true|true|false");
}

/// WEBGL_lose_context: loseContext makes the context lost at once (getError reports CONTEXT_LOST_WEBGL once,
/// methods answer null), webglcontextlost fires as a task; preventDefault allows restoreContext, whose
/// webglcontextrestored fires as a task and leaves a working context.
#[test]
fn lose_and_restore_context() {
    let mut e = page(DRAW);
    let r = eval(
        &mut e,
        "var events = []; c.addEventListener('webglcontextlost', function (ev) { events.push(ev.type + ':' + (ev instanceof WebGLContextEvent)); ev.preventDefault(); });
         c.addEventListener('webglcontextrestored', function (ev) { events.push(ev.type); });
         var ext = gl.getExtension('WEBGL_lose_context'); var same = ext === gl.getExtension('webgl_lose_context');
         ext.loseContext();
         [same, gl.isContextLost(), gl.getError() === gl.CONTEXT_LOST_WEBGL, gl.getError(), gl.createBuffer(), events.length].join('|')",
    );
    assert_eq!(r, "true|true|true|0||0");
    settle(&mut e);
    let r = eval(&mut e, "ext.restoreContext(); events.join(' ')");
    assert_eq!(r, "webglcontextlost:true");
    settle(&mut e);
    let r = eval(&mut e, "[events.join(' '), gl.isContextLost(), gl.createBuffer() instanceof WebGLBuffer, gl.getError()].join('|')");
    assert_eq!(r, "webglcontextlost:true webglcontextrestored|false|true|0");
}

/// The surface hand-off: a GL canvas's toDataURL is its drawn frame (not the blank canvas), drawImage of it
/// into a 2D canvas copies the red triangle (top-down: GL's bottom-left pixel is the 2D canvas's bottom
/// row), and texImage2D from an ImageData uploads (readable back through a framebuffer).
#[test]
fn surface_hand_off() {
    let mut e = page(DRAW);
    let r = eval(
        &mut e,
        "var blank = document.createElement('canvas'); blank.width = 64; blank.height = 64;
         var d = document.createElement('canvas'); d.width = 64; d.height = 64; var x = d.getContext('2d'); x.drawImage(c, 0, 0);
         var bl = Array.from(x.getImageData(5, 58, 1, 1).data).join(','), tr = Array.from(x.getImageData(60, 3, 1, 1).data).join(',');
         var t = gl.createTexture(); gl.bindTexture(gl.TEXTURE_2D, t);
         gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, new ImageData(new Uint8ClampedArray([10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 1, 2, 3, 255]), 2, 2));
         var fb = gl.createFramebuffer(); gl.bindFramebuffer(gl.FRAMEBUFFER, fb); gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, t, 0);
         var st = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
         var tex = px(1, 0);
         [c.toDataURL() !== blank.toDataURL(), bl, tr, st, tex, gl.getError()].join('|')",
    );
    assert_eq!(r, "true|255,0,0,255|0,0,255,255|true|40,50,60,255|0");
}
