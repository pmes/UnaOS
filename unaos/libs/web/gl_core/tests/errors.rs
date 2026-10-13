//! WEBGL1 (SR136) error-table KATs: each row makes one call in a known state and asserts the error flag
//! ES 2.0 / WebGL 1.0 assign it (getError returns it once, then NO_ERROR); the shader rows assert
//! COMPILE_STATUS false for sources GLSL ES 1.00 (and WebGL's restrictions) reject, and true for their
//! corrected twins.

mod common;
use common::*;
use gl_core::consts::*;
use gl_core::texture::Pixels;
use gl_core::{Context, Param};

type Row = (&'static str, fn(&mut Context), u32);

fn fresh() -> Context {
    ctx(4, 4)
}

fn bound_texture(g: &mut Context) {
    let t = g.create_texture();
    g.bind_texture(TEXTURE_2D, t);
}

#[test]
fn error_table() {
    let rows: &[Row] = &[
        ("bindBuffer bad target", |g| g.bind_buffer(0x1234, 0), INVALID_ENUM),
        ("bufferData no buffer bound", |g| g.buffer_data(ARRAY_BUFFER, 4, None, STATIC_DRAW), INVALID_OPERATION),
        ("bufferData bad usage", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.buffer_data(ARRAY_BUFFER, 4, None, 0x1234)
        }, INVALID_ENUM),
        ("bufferData negative size", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.buffer_data(ARRAY_BUFFER, -1, None, STATIC_DRAW)
        }, INVALID_VALUE),
        ("bufferSubData out of range", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.buffer_data(ARRAY_BUFFER, 4, None, STATIC_DRAW);
            g.buffer_sub_data(ARRAY_BUFFER, 2, &[0; 4])
        }, INVALID_VALUE),
        ("buffer rebound to the other target", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.bind_buffer(ELEMENT_ARRAY_BUFFER, b)
        }, INVALID_OPERATION),
        ("vertexAttribPointer stride 256", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.vertex_attrib_pointer(0, 4, FLOAT, false, 256, 0)
        }, INVALID_VALUE),
        ("vertexAttribPointer misaligned offset", |g| {
            let b = g.create_buffer();
            g.bind_buffer(ARRAY_BUFFER, b);
            g.vertex_attrib_pointer(0, 4, FLOAT, false, 0, 2)
        }, INVALID_OPERATION),
        ("vertexAttribPointer size 5", |g| g.vertex_attrib_pointer(0, 5, FLOAT, false, 0, 0), INVALID_VALUE),
        ("vertexAttribPointer bad type", |g| g.vertex_attrib_pointer(0, 4, INT, false, 0, 0), INVALID_ENUM),
        ("vertexAttribPointer no ARRAY_BUFFER, offset 4", |g| g.vertex_attrib_pointer(0, 4, FLOAT, false, 0, 4), INVALID_OPERATION),
        ("enableVertexAttribArray 16", |g| g.enable_vertex_attrib_array(16, true), INVALID_VALUE),
        ("drawArrays bad mode", |g| g.draw_arrays(7, 0, 3), INVALID_ENUM),
        ("drawArrays negative count", |g| g.draw_arrays(TRIANGLES, 0, -1), INVALID_VALUE),
        ("drawArrays no program", |g| g.draw_arrays(TRIANGLES, 0, 3), INVALID_OPERATION),
        ("drawElements bad type", |g| g.draw_elements(TRIANGLES, 3, FLOAT, 0), INVALID_ENUM),
        ("drawElements UNSIGNED_INT without the extension", |g| g.draw_elements(TRIANGLES, 3, UNSIGNED_INT, 0), INVALID_ENUM),
        ("drawElements no element buffer", |g| g.draw_elements(TRIANGLES, 3, UNSIGNED_SHORT, 0), INVALID_OPERATION),
        ("drawElements odd offset", |g| g.draw_elements(TRIANGLES, 3, UNSIGNED_SHORT, 1), INVALID_OPERATION),
        ("enable bad cap", |g| g.enable(0x1234, true), INVALID_ENUM),
        ("blendFunc constant colour × constant alpha", |g| g.blend_func_separate(CONSTANT_COLOR, CONSTANT_ALPHA, ONE, ZERO), INVALID_OPERATION),
        ("blendFunc SRC_ALPHA_SATURATE as destination", |g| g.blend_func_separate(ONE, SRC_ALPHA_SATURATE, ONE, ZERO), INVALID_ENUM),
        ("blendEquation bad mode", |g| g.blend_equation_separate(0x1234, FUNC_ADD), INVALID_ENUM),
        ("depthFunc bad", |g| g.depth_func(0x1234), INVALID_ENUM),
        ("depthRange near > far", |g| g.depth_range(1.0, 0.0), INVALID_OPERATION),
        ("stencilOp bad op", |g| g.stencil_op_separate(FRONT, KEEP, 0x1234, KEEP), INVALID_ENUM),
        ("stencilFuncSeparate bad face", |g| g.stencil_func_separate(0x1234, ALWAYS, 0, 0xFF), INVALID_ENUM),
        ("cullFace bad", |g| g.cull_face(0x1234), INVALID_ENUM),
        ("frontFace bad", |g| g.front_face(0x1234), INVALID_ENUM),
        ("lineWidth 0", |g| g.line_width(0.0), INVALID_VALUE),
        ("viewport negative width", |g| g.viewport(0, 0, -1, 1), INVALID_VALUE),
        ("scissor negative height", |g| g.scissor(0, 0, 1, -1), INVALID_VALUE),
        ("hint bad target", |g| g.hint(0x1234, NICEST), INVALID_ENUM),
        ("pixelStorei alignment 3", |g| g.pixel_storei(UNPACK_ALIGNMENT, 3), INVALID_VALUE),
        ("pixelStorei bad pname", |g| g.pixel_storei(0x1234, 1), INVALID_ENUM),
        ("activeTexture past the units", |g| g.active_texture(TEXTURE0 + 32), INVALID_ENUM),
        ("bindTexture target change", |g| {
            let t = g.create_texture();
            g.bind_texture(TEXTURE_2D, t);
            g.bind_texture(TEXTURE_CUBE_MAP, t)
        }, INVALID_OPERATION),
        ("texParameter no texture", |g| g.tex_parameter(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32), INVALID_OPERATION),
        ("texParameter bad value", |g| {
            bound_texture(g);
            g.tex_parameter(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR_MIPMAP_LINEAR as i32)
        }, INVALID_ENUM),
        ("texImage2D border 1", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 1, RGBA, UNSIGNED_BYTE, Pixels::None)
        }, INVALID_VALUE),
        ("texImage2D internalformat ≠ format", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGB, 1, 1, 0, RGBA, UNSIGNED_BYTE, Pixels::None)
        }, INVALID_OPERATION),
        ("texImage2D 565 with RGBA", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 0, RGBA, UNSIGNED_SHORT_5_6_5, Pixels::None)
        }, INVALID_OPERATION),
        ("texImage2D FLOAT (OES_texture_float refused)", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 0, RGBA, FLOAT, Pixels::None)
        }, INVALID_ENUM),
        ("texImage2D too little data", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 2, 2, 0, RGBA, UNSIGNED_BYTE, Pixels::Bytes(&[0; 15]))
        }, INVALID_OPERATION),
        ("texImage2D cube face not square", |g| {
            let t = g.create_texture();
            g.bind_texture(TEXTURE_CUBE_MAP, t);
            g.tex_image_2d(TEXTURE_CUBE_MAP_POSITIVE_X, 0, RGBA, 2, 1, 0, RGBA, UNSIGNED_BYTE, Pixels::None)
        }, INVALID_VALUE),
        ("texSubImage2D outside the level", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 2, 2, 0, RGBA, UNSIGNED_BYTE, Pixels::None);
            g.tex_sub_image_2d(TEXTURE_2D, 0, 1, 1, 2, 2, RGBA, UNSIGNED_BYTE, Pixels::Bytes(&[0; 16]))
        }, INVALID_VALUE),
        ("generateMipmap non-power-of-two", |g| {
            bound_texture(g);
            g.tex_image_2d(TEXTURE_2D, 0, RGBA, 3, 2, 0, RGBA, UNSIGNED_BYTE, Pixels::None);
            g.generate_mipmap(TEXTURE_2D)
        }, INVALID_OPERATION),
        ("compressedTexImage2D (no formats)", |g| g.compressed_tex_image(), INVALID_ENUM),
        ("renderbufferStorage bad format", |g| {
            let r = g.create_renderbuffer();
            g.bind_renderbuffer(RENDERBUFFER, r);
            g.renderbuffer_storage(RENDERBUFFER, RGBA, 1, 1)
        }, INVALID_ENUM),
        ("framebufferTexture2D on the default framebuffer", |g| g.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, 0, 0), INVALID_OPERATION),
        ("framebufferTexture2D level 1", |g| {
            let f = g.create_framebuffer();
            g.bind_framebuffer(FRAMEBUFFER, f);
            let t = g.create_texture();
            g.bind_texture(TEXTURE_2D, t);
            g.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 1)
        }, INVALID_VALUE),
        ("clear on an incomplete framebuffer", |g| {
            let f = g.create_framebuffer();
            g.bind_framebuffer(FRAMEBUFFER, f);
            g.clear(COLOR_BUFFER_BIT)
        }, INVALID_FRAMEBUFFER_OPERATION),
        ("clear bad bit", |g| g.clear(0x8000_0000), INVALID_VALUE),
        ("readPixels RGB/UNSIGNED_BYTE", |g| g.read_pixels(0, 0, 1, 1, RGB, UNSIGNED_BYTE, &mut [0; 4]), INVALID_OPERATION),
        ("readPixels bad format", |g| g.read_pixels(0, 0, 1, 1, 0x1234, UNSIGNED_BYTE, &mut [0; 4]), INVALID_ENUM),
        ("readPixels short destination", |g| g.read_pixels(0, 0, 2, 2, RGBA, UNSIGNED_BYTE, &mut [0; 12]), INVALID_OPERATION),
        ("useProgram unlinked", |g| {
            let p = g.create_program();
            g.use_program(p)
        }, INVALID_OPERATION),
        ("uniform1f with no program", |g| g.uniform(Some(&gl_core::UniformLoc { program: 1, link_gen: 1, index: 0 }), gl_core::program::UKind::Float, 1, &[1.0], false), INVALID_OPERATION),
        ("attachShader the same type twice", |g| {
            let p = g.create_program();
            let a = g.create_shader(VERTEX_SHADER);
            let b = g.create_shader(VERTEX_SHADER);
            g.attach_shader(p, a);
            g.attach_shader(p, b)
        }, INVALID_OPERATION),
        ("createShader bad type", |g| {
            g.create_shader(0x1234);
        }, INVALID_ENUM),
        ("bindAttribLocation gl_ prefix", |g| {
            let p = g.create_program();
            g.bind_attrib_location(p, 0, "gl_Vertex")
        }, INVALID_OPERATION),
        ("getProgramParameter bad pname", |g| {
            let p = g.create_program();
            g.get_program_parameter(p, 0x1234);
        }, INVALID_ENUM),
        ("getParameter bad pname", |g| {
            g.get_parameter(0x1234);
        }, INVALID_ENUM),
    ];
    for (name, f, want) in rows {
        let mut g = fresh();
        f(&mut g);
        assert_eq!(g.get_error(), *want, "{name}");
        assert_eq!(g.get_error(), NO_ERROR, "{name}: the flag is cleared once read");
    }
}

/// A uniform location from an earlier link is INVALID_OPERATION after a relink; a null location is ignored;
/// several values into a non-array uniform are INVALID_OPERATION; a float setter on an int uniform too.
#[test]
fn uniform_rules() {
    let mut g = ctx(2, 2);
    let fs = "precision mediump float; uniform vec4 c; uniform int k; uniform float a[3];\nvoid main() { gl_FragColor = c * float(k) + vec4(a[2]); }\n";
    let p = program(&mut g, PASS_VS, fs);
    let c = g.get_uniform_location(p, "c");
    let k = g.get_uniform_location(p, "k");
    assert!(g.get_uniform_location(p, "a").is_some() && g.get_uniform_location(p, "a[2]").is_some() && g.get_uniform_location(p, "a[3]").is_none());
    let f = gl_core::program::UKind::Float;
    g.uniform(c.as_ref(), f, 4, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], true);
    assert_eq!(g.get_error(), INVALID_OPERATION, "two vec4s into a non-array");
    g.uniform(k.as_ref(), f, 1, &[1.0], false);
    assert_eq!(g.get_error(), INVALID_OPERATION, "uniform1f on an int");
    g.uniform(None, f, 4, &[1.0, 2.0, 3.0, 4.0], false);
    assert_eq!(g.get_error(), NO_ERROR, "null location ignored");
    g.uniform(c.as_ref(), f, 4, &[1.0, 2.0, 3.0, 4.0], false);
    assert_eq!(g.get_uniform(p, c.as_ref().unwrap()), Param::F32s(vec![1.0, 2.0, 3.0, 4.0]));
    g.uniform_matrix(c.as_ref(), 2, true, &[0.0; 4]);
    assert_eq!(g.get_error(), INVALID_VALUE, "transpose must be false");
    g.link_program(p);
    g.uniform(c.as_ref(), f, 4, &[1.0, 2.0, 3.0, 4.0], false);
    assert_eq!(g.get_error(), INVALID_OPERATION, "location from an earlier link");
}

/// Framebuffer completeness (ES 2.0 §4.4.5 / WebGL): missing attachment, mismatched dimensions, a non-
/// renderable texture format, depth + depth-stencil together; a complete one renders and reads back.
#[test]
fn framebuffer_completeness() {
    let mut g = ctx(4, 4);
    let f = g.create_framebuffer();
    g.bind_framebuffer(FRAMEBUFFER, f);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT);
    let t = g.create_texture();
    g.bind_texture(TEXTURE_2D, t);
    g.tex_image_2d(TEXTURE_2D, 0, LUMINANCE, 4, 4, 0, LUMINANCE, UNSIGNED_BYTE, Pixels::None);
    g.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 0);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_INCOMPLETE_ATTACHMENT, "LUMINANCE is not colour-renderable");
    g.tex_image_2d(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, Pixels::None);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_COMPLETE);
    let r = g.create_renderbuffer();
    g.bind_renderbuffer(RENDERBUFFER, r);
    g.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 2, 2);
    g.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, r);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_INCOMPLETE_DIMENSIONS);
    g.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 4, 4);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_COMPLETE);
    let ds = g.create_renderbuffer();
    g.bind_renderbuffer(RENDERBUFFER, ds);
    g.renderbuffer_storage(RENDERBUFFER, DEPTH_STENCIL, 4, 4);
    g.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_STENCIL_ATTACHMENT, RENDERBUFFER, ds);
    assert_eq!(g.check_framebuffer_status(FRAMEBUFFER), FRAMEBUFFER_UNSUPPORTED);
    g.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_STENCIL_ATTACHMENT, RENDERBUFFER, 0);
    g.clear_color([0.0, 1.0, 0.0, 1.0]);
    g.clear(COLOR_BUFFER_BIT);
    assert_eq!(read(&mut g, 3, 3), [0, 255, 0, 255]);
    assert_eq!(g.get_error(), NO_ERROR);
}

/// The lost-context path of the core: getError reports CONTEXT_LOST_WEBGL once, a restore starts over.
#[test]
fn lost_context() {
    let mut g = ctx(2, 2);
    let b = g.create_buffer();
    g.lose();
    assert!(g.lost);
    assert_eq!(g.get_error(), CONTEXT_LOST_WEBGL);
    assert_eq!(g.get_error(), NO_ERROR);
    g.restore();
    assert!(!g.lost);
    assert!(!g.is_object(gl_core::ObjKind::Buffer, b), "objects do not survive a restore");
}

/// Shaders that must not compile (GLSL ES 1.00 and WebGL's rules), each with a twin that must.
#[test]
fn compile_errors() {
    let rows: &[(&str, u32, &str, &str)] = &[
        ("no default float precision in the fragment language", FRAGMENT_SHADER, "void main() { float x = 1.0; gl_FragColor = vec4(x); }", "precision mediump float; void main() { float x = 1.0; gl_FragColor = vec4(x); }"),
        ("while loops (Appendix A)", VERTEX_SHADER, "void main() { int i = 0; while (i < 2) { i++; } gl_Position = vec4(0.0); }", "void main() { for (int i = 0; i < 2; i++) {} gl_Position = vec4(0.0); }"),
        ("recursion", VERTEX_SHADER, "float f(float x); float f(float x) { return f(x); } void main() { gl_Position = vec4(f(1.0)); }", "float f(float x) { return x; } void main() { gl_Position = vec4(f(1.0)); }"),
        ("undeclared identifier", VERTEX_SHADER, "void main() { gl_Position = vec4(q); }", "void main() { float q = 1.0; gl_Position = vec4(q); }"),
        ("no implicit int → float", VERTEX_SHADER, "void main() { float x = 1; gl_Position = vec4(x); }", "void main() { float x = 1.0; gl_Position = vec4(x); }"),
        ("gl_ identifiers are reserved", VERTEX_SHADER, "void main() { float gl_x = 1.0; gl_Position = vec4(gl_x); }", "void main() { float g_x = 1.0; gl_Position = vec4(g_x); }"),
        ("webgl_ identifiers are reserved", VERTEX_SHADER, "void main() { float webgl_x = 1.0; gl_Position = vec4(webgl_x); }", "void main() { float web_x = 1.0; gl_Position = vec4(web_x); }"),
        ("reserved operator %", VERTEX_SHADER, "void main() { int a = 5 % 2; gl_Position = vec4(0.0); }", "void main() { int a = 5 / 2; gl_Position = vec4(0.0); }"),
        ("assigning a uniform", VERTEX_SHADER, "uniform float u; void main() { u = 1.0; gl_Position = vec4(u); }", "uniform float u; void main() { gl_Position = vec4(u); }"),
        ("#version 300 es", VERTEX_SHADER, "#version 300 es\nvoid main() { gl_Position = vec4(0.0); }", "#version 100\nvoid main() { gl_Position = vec4(0.0); }"),
        ("texture2DLod in a fragment shader", FRAGMENT_SHADER, "precision mediump float; uniform sampler2D s; void main() { gl_FragColor = texture2DLod(s, vec2(0.0), 0.0); }", "precision mediump float; uniform sampler2D s; void main() { gl_FragColor = texture2D(s, vec2(0.0)); }"),
        ("dFdx without the extension", FRAGMENT_SHADER, "precision mediump float; void main() { gl_FragColor = vec4(dFdx(1.0)); }", "precision mediump float; void main() { gl_FragColor = vec4(1.0); }"),
        ("swizzle out of range", VERTEX_SHADER, "void main() { vec2 v = vec2(1.0); gl_Position = vec4(v.z); }", "void main() { vec2 v = vec2(1.0); gl_Position = vec4(v.y); }"),
        ("constant index out of range", VERTEX_SHADER, "void main() { float a[2]; a[2] = 1.0; gl_Position = vec4(0.0); }", "void main() { float a[2]; a[1] = 1.0; gl_Position = vec4(a[1]); }"),
        ("undefined macro in #if", VERTEX_SHADER, "#if FOO\n#endif\nvoid main() { gl_Position = vec4(0.0); }", "#if defined(FOO)\n#endif\nvoid main() { gl_Position = vec4(0.0); }"),
        ("both gl_FragColor and gl_FragData", FRAGMENT_SHADER, "precision mediump float; void main() { gl_FragColor = vec4(1.0); gl_FragData[0] = vec4(1.0); }", "precision mediump float; void main() { gl_FragData[0] = vec4(1.0); }"),
    ];
    for (name, kind, bad, good) in rows {
        let mut g = ctx(1, 1);
        let (_, ok, log) = shader(&mut g, *kind, bad);
        assert!(!ok, "{name}: must not compile");
        assert!(log.starts_with("ERROR: 0:"), "{name}: log shape, got {log:?}");
        let (_, ok, log) = shader(&mut g, *kind, good);
        assert!(ok, "{name}: the corrected twin must compile, log {log}");
    }
}

/// The macro preprocessor: function-like macros, #ifdef/#else, __LINE__, GL_ES.
#[test]
fn preprocessor() {
    let mut g = ctx(1, 1);
    let src = "#define SQ(x) ((x) * (x))\n#ifdef GL_ES\n#define K 3.0\n#else\n#define K 0.0\n#endif\nvoid main() { float l = float(__LINE__); gl_Position = vec4(SQ(K), l, 0.0, 1.0); }\n";
    let (_, ok, log) = shader(&mut g, VERTEX_SHADER, src);
    assert!(ok, "{log}");
}
