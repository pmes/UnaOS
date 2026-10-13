//! Shared helpers for gl_core's pixel and error KATs.
#![allow(dead_code)]

use gl_core::consts::*;
use gl_core::{Attributes, Context, Param};

pub fn ctx(w: u32, h: u32) -> Context {
    Context::new(w, h, Attributes { preserve_drawing_buffer: true, ..Attributes::default() })
}

pub fn shader(g: &mut Context, kind: u32, src: &str) -> (u32, bool, String) {
    let s = g.create_shader(kind);
    g.shader_source(s, src);
    g.compile_shader(s);
    let ok = g.get_shader_parameter(s, COMPILE_STATUS) == Param::Bool(true);
    (s, ok, g.get_shader_info_log(s).unwrap_or_default())
}

/// Compile, link and use a program; panics with the logs on failure.
pub fn program(g: &mut Context, vs: &str, fs: &str) -> u32 {
    let (v, vok, vlog) = shader(g, VERTEX_SHADER, vs);
    assert!(vok, "vertex shader failed:\n{vlog}");
    let (f, fok, flog) = shader(g, FRAGMENT_SHADER, fs);
    assert!(fok, "fragment shader failed:\n{flog}");
    let p = g.create_program();
    g.attach_shader(p, v);
    g.attach_shader(p, f);
    g.link_program(p);
    assert_eq!(g.get_program_parameter(p, LINK_STATUS), Param::Bool(true), "link failed: {:?}", g.get_program_info_log(p));
    g.use_program(p);
    assert_eq!(g.get_error(), NO_ERROR);
    p
}

/// A float vertex buffer bound to `attrib` of `p` with `size` components.
pub fn attrib(g: &mut Context, p: u32, name: &str, size: i32, data: &[f32]) {
    let b = g.create_buffer();
    g.bind_buffer(ARRAY_BUFFER, b);
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    g.buffer_data(ARRAY_BUFFER, bytes.len() as i64, Some(&bytes), STATIC_DRAW);
    let loc = g.get_attrib_location(p, name);
    assert!(loc >= 0, "attribute {name} not active");
    g.enable_vertex_attrib_array(loc as u32, true);
    g.vertex_attrib_pointer(loc as u32, size, FLOAT, false, 0, 0);
}

pub fn read(g: &mut Context, x: i32, y: i32) -> [u8; 4] {
    let mut o = [0u8; 4];
    g.read_pixels(x, y, 1, 1, RGBA, UNSIGNED_BYTE, &mut o);
    o
}

pub fn close(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    (0..4).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

pub const PASS_VS: &str = "attribute vec2 pos;\nvoid main() { gl_Position = vec4(pos, 0.0, 1.0); }\n";

/// Two triangles covering clip space.
pub const QUAD: [f32; 12] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0];
