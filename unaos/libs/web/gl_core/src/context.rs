//! The OpenGL ES 2.0 state machine with WebGL 1.0's additions (WebGL 1.0's methods and its "Differences Between WebGL and OpenGL
//! ES 2.0"). One [`Context`] per WebGLRenderingContext: the error flags (ES 2.0 §2.5), the object stores, the
//! bindings, the per-fragment state of ES 2.0 Chapter 4 and the drawing buffer (WebGL 1.0 "The Drawing Buffer").
//!
//! Every entry point validates exactly as ES 2.0 and the WebGL deltas say and records an error with
//! [`Context::err`] instead of acting; [`Context::get_error`] hands the flags back one at a time. While the
//! context is lost (WebGL 1.0 "lose the context") the binding answers every call itself; the core only keeps `lost` so
//! that getError reports CONTEXT_LOST_WEBGL once.

use crate::consts::*;
use crate::object::*;
use crate::texture::{self, Texture};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Implementation limits (getParameter answers; ES 2.0 Table 6.18–6.20 minimums are all met).
pub const MAX_TEXTURE_SIZE_V: u32 = 4096;
pub const MAX_CUBE_SIZE_V: u32 = 4096;
pub const MAX_RENDERBUFFER_SIZE_V: u32 = 4096;
pub const MAX_VIEWPORT_V: i32 = 4096;
pub const MAX_ATTRIBS: usize = 16;
pub const MAX_VERTEX_UNIFORM_VECTORS_V: u32 = 1024;
pub const MAX_FRAGMENT_UNIFORM_VECTORS_V: u32 = 1024;
pub const MAX_VARYING_VECTORS_V: u32 = 30;
pub const MAX_TEXTURE_UNITS_V: u32 = 16;
pub const MAX_VERTEX_TEXTURE_UNITS_V: u32 = 16;
pub const MAX_COMBINED_UNITS: usize = 32;
pub const POINT_SIZE_MAX: f32 = 64.0;

/// WebGLContextAttributes (WebGL 1.0 "WebGLContextAttributes") as granted: `antialias` is always false (no multisampling).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attributes {
    pub alpha: bool,
    pub depth: bool,
    pub stencil: bool,
    pub antialias: bool,
    pub premultiplied_alpha: bool,
    pub preserve_drawing_buffer: bool,
    pub fail_if_major_performance_caveat: bool,
    /// 0 "default", 1 "low-power", 2 "high-performance".
    pub power_preference: u8,
    pub desynchronized: bool,
}

impl Default for Attributes {
    fn default() -> Self {
        Attributes {
            alpha: true,
            depth: true,
            stencil: false,
            antialias: true,
            premultiplied_alpha: true,
            preserve_drawing_buffer: false,
            fail_if_major_performance_caveat: false,
            power_preference: 0,
            desynchronized: false,
        }
    }
}

/// A getParameter-family answer, typed the way WebGL 1.0's getParameter table returns it.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    F32s(Vec<f32>),
    I32s(Vec<i32>),
    U32s(Vec<u32>),
    Bools(Vec<bool>),
    Object(ObjKind, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjKind {
    Buffer,
    Texture,
    Renderbuffer,
    Framebuffer,
    Program,
    Shader,
}

/// WebGLActiveInfo .
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveInfo {
    pub size: i32,
    pub ty: u32,
    pub name: String,
}

/// WebGLShaderPrecisionFormat .
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrecisionFormat {
    pub range_min: i32,
    pub range_max: i32,
    pub precision: i32,
}

/// A WebGLUniformLocation: the program, the link generation it came from, and its row in the link's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniformLoc {
    pub program: u32,
    pub link_gen: u32,
    pub index: u32,
}

/// What the last draw call cost (the ops-per-pixel instrument).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawStats {
    pub vertices: u64,
    pub vertex_ops: u64,
    pub fragments: u64,
    pub fragment_ops: u64,
    pub helper_invocations: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexAttrib {
    pub enabled: bool,
    pub size: i32,
    pub ty: u32,
    pub normalized: bool,
    pub stride: i32,
    pub offset: u32,
    pub buffer: u32,
    pub current: [f32; 4],
}

impl Default for VertexAttrib {
    fn default() -> Self {
        VertexAttrib { enabled: false, size: 4, ty: FLOAT, normalized: false, stride: 0, offset: 0, buffer: 0, current: [0.0, 0.0, 0.0, 1.0] }
    }
}

/// One face's stencil state (ES 2.0 §4.1.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StencilFace {
    pub func: u32,
    pub reference: i32,
    pub value_mask: u32,
    pub write_mask: u32,
    pub fail: u32,
    pub zfail: u32,
    pub zpass: u32,
}

impl Default for StencilFace {
    fn default() -> Self {
        StencilFace { func: ALWAYS, reference: 0, value_mask: 0xFFFF_FFFF, write_mask: 0xFFFF_FFFF, fail: KEEP, zfail: KEEP, zpass: KEEP }
    }
}

/// The fixed-function state of ES 2.0 Tables 6.4–6.13 (initial values per those tables).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub viewport: [i32; 4],
    pub scissor: [i32; 4],
    pub scissor_test: bool,
    pub depth_range: [f32; 2],
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub clear_stencil: i32,
    pub color_mask: [bool; 4],
    pub depth_mask: bool,
    pub front: StencilFace,
    pub back: StencilFace,
    pub stencil_test: bool,
    pub blend: bool,
    /// [rgb, alpha]
    pub blend_eq: [u32; 2],
    pub blend_src: [u32; 2],
    pub blend_dst: [u32; 2],
    pub blend_color: [f32; 4],
    pub cull: bool,
    pub cull_mode: u32,
    pub front_face: u32,
    pub depth_test: bool,
    pub depth_func: u32,
    pub polygon_offset_fill: bool,
    pub po_factor: f32,
    pub po_units: f32,
    pub dither: bool,
    pub sample_alpha_to_coverage: bool,
    pub sample_coverage: bool,
    pub sample_coverage_value: f32,
    pub sample_coverage_invert: bool,
    pub line_width: f32,
    pub mipmap_hint: u32,
    pub derivative_hint: u32,
    pub pack_alignment: i32,
    pub unpack_alignment: i32,
    pub unpack_flip_y: bool,
    pub unpack_premultiply: bool,
    pub unpack_colorspace: u32,
}

impl State {
    fn new(w: i32, h: i32) -> State {
        State {
            viewport: [0, 0, w, h],
            scissor: [0, 0, w, h],
            scissor_test: false,
            depth_range: [0.0, 1.0],
            clear_color: [0.0; 4],
            clear_depth: 1.0,
            clear_stencil: 0,
            color_mask: [true; 4],
            depth_mask: true,
            front: StencilFace::default(),
            back: StencilFace::default(),
            stencil_test: false,
            blend: false,
            blend_eq: [FUNC_ADD, FUNC_ADD],
            blend_src: [ONE, ONE],
            blend_dst: [ZERO, ZERO],
            blend_color: [0.0; 4],
            cull: false,
            cull_mode: BACK,
            front_face: CCW,
            depth_test: false,
            depth_func: LESS,
            polygon_offset_fill: false,
            po_factor: 0.0,
            po_units: 0.0,
            dither: true,
            sample_alpha_to_coverage: false,
            sample_coverage: false,
            sample_coverage_value: 1.0,
            sample_coverage_invert: false,
            line_width: 1.0,
            mipmap_hint: DONT_CARE,
            derivative_hint: DONT_CARE,
            pack_alignment: 4,
            unpack_alignment: 4,
            unpack_flip_y: false,
            unpack_premultiply: false,
            unpack_colorspace: BROWSER_DEFAULT_WEBGL,
        }
    }
}

/// The default framebuffer (WebGL 1.0 "The Drawing Buffer"): RGBA8 colour, optional 24-bit depth and
/// 8-bit stencil, rows bottom-up (row 0 = window y 0, ES 2.0 §2.12's window coordinates).
pub struct DrawingBuffer {
    pub w: u32,
    pub h: u32,
    pub color: Vec<u8>,
    pub depth: Vec<u32>,
    pub stencil: Vec<u8>,
    /// Bumped on every change to the drawing buffer's pixels (the compositor's cache key).
    pub generation: u64,
    /// `preserveDrawingBuffer: false`: the buffer was presented and must read as cleared before the next
    /// operation that touches it (WebGL 1.0 "The Drawing Buffer").
    pub clear_pending: bool,
}

impl DrawingBuffer {
    fn new(w: u32, h: u32, a: &Attributes) -> DrawingBuffer {
        let n = (w as usize) * (h as usize);
        DrawingBuffer {
            w,
            h,
            color: vec![0; n * 4],
            depth: if a.depth { vec![0x00FF_FFFF; n] } else { Vec::new() },
            stencil: if a.stencil { vec![0; n] } else { Vec::new() },
            generation: 1,
            clear_pending: false,
        }
    }
}

/// Enabled WebGL extensions the core must know about (the binding owns the extension objects).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extensions {
    pub standard_derivatives: bool,
    pub element_index_uint: bool,
}

pub struct Context {
    pub attrs: Attributes,
    pub(crate) errors: Vec<u32>,
    pub lost: bool,
    lost_reported: bool,
    pub ext: Extensions,
    pub db: DrawingBuffer,
    pub buffers: Store<Buffer>,
    pub textures: Store<Texture>,
    pub renderbuffers: Store<Renderbuffer>,
    pub framebuffers: Store<Framebuffer>,
    pub shaders: Store<Shader>,
    pub programs: Store<Program>,
    pub array_buffer: u32,
    pub element_buffer: u32,
    pub attribs: Vec<VertexAttrib>,
    pub current_program: u32,
    pub active_unit: u32,
    /// Per texture unit: (TEXTURE_2D binding, TEXTURE_CUBE_MAP binding).
    pub units: Vec<(u32, u32)>,
    pub framebuffer: u32,
    pub renderbuffer: u32,
    pub st: State,
    pub stats: DrawStats,
}

/// The largest drawing buffer we allocate; a bigger canvas gets a smaller drawing buffer (WebGL 1.0 "The Drawing Buffer"
/// permits it; drawingBufferWidth/Height report the truth).
pub fn drawing_buffer_size(w: u32, h: u32) -> (u32, u32) {
    (w.min(MAX_VIEWPORT_V as u32), h.min(MAX_VIEWPORT_V as u32))
}

impl Context {
    /// A new context for a `w × h` canvas (WebGL 1.0 "Context Creation").
    pub fn new(w: u32, h: u32, mut attrs: Attributes) -> Context {
        attrs.antialias = false;
        let (w, h) = drawing_buffer_size(w, h);
        Context {
            db: DrawingBuffer::new(w, h, &attrs),
            attrs,
            errors: Vec::new(),
            lost: false,
            lost_reported: false,
            ext: Extensions::default(),
            buffers: Store::default(),
            textures: Store::default(),
            renderbuffers: Store::default(),
            framebuffers: Store::default(),
            shaders: Store::default(),
            programs: Store::default(),
            array_buffer: 0,
            element_buffer: 0,
            attribs: vec![VertexAttrib::default(); MAX_ATTRIBS],
            current_program: 0,
            active_unit: 0,
            units: vec![(0, 0); MAX_COMBINED_UNITS],
            framebuffer: 0,
            renderbuffer: 0,
            st: State::new(w as i32, h as i32),
            stats: DrawStats::default(),
        }
    }

    // ------------------------------------------------------------------------------------------ errors

    /// Record an error flag (ES 2.0 §2.5: a flag already set is not set again).
    pub fn err(&mut self, e: u32) {
        if !self.errors.contains(&e) {
            self.errors.push(e);
        }
    }

    /// ES 2.0 §2.5 / WebGL 1.0 getError: one flag per call, cleared as returned. A lost context
    /// answers CONTEXT_LOST_WEBGL once, then NO_ERROR (WebGL 1.0 "lose the context").
    pub fn get_error(&mut self) -> u32 {
        if self.lost {
            if !self.lost_reported {
                self.lost_reported = true;
                return CONTEXT_LOST_WEBGL;
            }
            return NO_ERROR;
        }
        if self.errors.is_empty() { NO_ERROR } else { self.errors.remove(0) }
    }

    /// WebGL 1.0 "lose the context" "lose the context": every object is invalidated and the drawing buffer dropped.
    pub fn lose(&mut self) {
        let (w, h) = (self.db.w, self.db.h);
        let attrs = self.attrs;
        *self = Context::new(w, h, attrs);
        self.attrs = attrs;
        self.lost = true;
        self.lost_reported = false;
    }

    /// WebGL 1.0 "restore the context": a fresh context in its initial state.
    pub fn restore(&mut self) {
        let (w, h) = (self.db.w, self.db.h);
        let attrs = self.attrs;
        *self = Context::new(w, h, attrs);
        self.attrs = attrs;
    }

    /// The canvas was resized: a new drawing buffer, cleared (HTML §4.12.5 / WebGL 1.0 "Changing the Size of the Canvas"). The viewport
    /// and scissor are NOT changed (WebGL 1.0 "Changing the Size of the Canvas").
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = drawing_buffer_size(w, h);
        if (w, h) == (self.db.w, self.db.h) {
            return;
        }
        let g = self.db.generation;
        self.db = DrawingBuffer::new(w, h, &self.attrs);
        self.db.generation = g + 1;
    }

    // ------------------------------------------------------------------------------------------ objects

    pub fn create_buffer(&mut self) -> u32 {
        self.buffers.insert(Buffer { target: 0, data: Vec::new(), usage: STATIC_DRAW, deleted: false })
    }

    pub fn create_texture(&mut self) -> u32 {
        self.textures.insert(Texture::default())
    }

    pub fn create_renderbuffer(&mut self) -> u32 {
        self.renderbuffers.insert(Renderbuffer {
            format: RGBA4,
            w: 0,
            h: 0,
            color: Vec::new(),
            depth: Vec::new(),
            stencil: Vec::new(),
            deleted: false,
            bound: false,
        })
    }

    pub fn create_framebuffer(&mut self) -> u32 {
        self.framebuffers.insert(Framebuffer::default())
    }

    pub fn create_shader(&mut self, kind: u32) -> u32 {
        if kind != VERTEX_SHADER && kind != FRAGMENT_SHADER {
            self.err(INVALID_ENUM);
            return 0;
        }
        self.shaders.insert(Shader {
            kind,
            source: String::new(),
            compiled: false,
            log: String::new(),
            deleted: false,
            attached: 0,
            code: None,
        })
    }

    pub fn create_program(&mut self) -> u32 {
        self.programs.insert(Program {
            vs: 0,
            fs: 0,
            deleted: false,
            linked: false,
            log: String::new(),
            validated: false,
            link_gen: 0,
            bindings: Default::default(),
            linked_data: None,
            store: Vec::new(),
        })
    }

    /// Whether `name` of `kind` exists, is not deleted and (for the bindable kinds) has been bound once — the
    /// is* queries (ES 2.0 §6.1; WebGL 1.0: false for a deleted object).
    pub fn is_object(&self, kind: ObjKind, name: u32) -> bool {
        match kind {
            ObjKind::Buffer => self.buffers.get(name).is_some_and(|b| !b.deleted && b.target != 0),
            ObjKind::Texture => self.textures.get(name).is_some_and(|t| !t.deleted && t.target != 0),
            ObjKind::Renderbuffer => self.renderbuffers.get(name).is_some_and(|r| !r.deleted && r.bound),
            ObjKind::Framebuffer => self.framebuffers.get(name).is_some_and(|f| !f.deleted && f.bound),
            ObjKind::Shader => self.shaders.get(name).is_some_and(|s| !s.deleted || s.attached > 0),
            ObjKind::Program => self.programs.get(name).is_some_and(|p| !p.deleted || self.current_program == name),
        }
    }

    /// Whether the object was deleted by the application (WebGL's "invalidated" flag).
    pub fn is_deleted(&self, kind: ObjKind, name: u32) -> bool {
        match kind {
            ObjKind::Buffer => self.buffers.get(name).is_none_or(|b| b.deleted),
            ObjKind::Texture => self.textures.get(name).is_none_or(|t| t.deleted),
            ObjKind::Renderbuffer => self.renderbuffers.get(name).is_none_or(|r| r.deleted),
            ObjKind::Framebuffer => self.framebuffers.get(name).is_none_or(|f| f.deleted),
            ObjKind::Shader => self.shaders.get(name).is_none_or(|s| s.deleted),
            ObjKind::Program => self.programs.get(name).is_none_or(|p| p.deleted),
        }
    }

    pub fn delete_buffer(&mut self, name: u32) {
        let Some(b) = self.buffers.get_mut(name) else { return };
        if b.deleted {
            return;
        }
        b.deleted = true;
        // ES 2.0 §2.9: deleting a bound buffer unbinds it from the context's binding points; vertex attribute
        // arrays keep their reference (the buffer lives on until they let go).
        if self.array_buffer == name {
            self.array_buffer = 0;
        }
        if self.element_buffer == name {
            self.element_buffer = 0;
        }
        self.sweep();
    }

    pub fn delete_texture(&mut self, name: u32) {
        let Some(t) = self.textures.get_mut(name) else { return };
        if t.deleted {
            return;
        }
        t.deleted = true;
        for u in self.units.iter_mut() {
            if u.0 == name {
                u.0 = 0;
            }
            if u.1 == name {
                u.1 = 0;
            }
        }
        // ES 2.0 §4.4.5: detached from the currently bound framebuffer only.
        let fb = self.framebuffer;
        if let Some(f) = self.framebuffers.get_mut(fb) {
            for a in [&mut f.color, &mut f.depth, &mut f.stencil, &mut f.depth_stencil] {
                if matches!(*a, Attach::Tex(n, _, _) if n == name) {
                    *a = Attach::None;
                }
            }
        }
        self.sweep();
    }

    pub fn delete_renderbuffer(&mut self, name: u32) {
        let Some(r) = self.renderbuffers.get_mut(name) else { return };
        if r.deleted {
            return;
        }
        r.deleted = true;
        if self.renderbuffer == name {
            self.renderbuffer = 0;
        }
        let fb = self.framebuffer;
        if let Some(f) = self.framebuffers.get_mut(fb) {
            for a in [&mut f.color, &mut f.depth, &mut f.stencil, &mut f.depth_stencil] {
                if *a == Attach::Rb(name) {
                    *a = Attach::None;
                }
            }
        }
        self.sweep();
    }

    pub fn delete_framebuffer(&mut self, name: u32) {
        let Some(f) = self.framebuffers.get_mut(name) else { return };
        if f.deleted {
            return;
        }
        f.deleted = true;
        f.color = Attach::None;
        f.depth = Attach::None;
        f.stencil = Attach::None;
        f.depth_stencil = Attach::None;
        if self.framebuffer == name {
            self.framebuffer = 0;
        }
        self.sweep();
    }

    pub fn delete_shader(&mut self, name: u32) {
        if let Some(s) = self.shaders.get_mut(name) {
            s.deleted = true;
        }
        self.sweep();
    }

    pub fn delete_program(&mut self, name: u32) {
        let Some(p) = self.programs.get_mut(name) else { return };
        if p.deleted {
            return;
        }
        p.deleted = true;
        self.sweep();
    }

    /// Release the storage of deleted objects nothing references any more (ES 2.0 §2.9 / §2.10.3 / §4.4: a
    /// deleted object lives while it is attached or in use). Names stay retired.
    pub fn sweep(&mut self) {
        let mut buf_refs: Vec<u32> = self.attribs.iter().map(|a| a.buffer).collect();
        buf_refs.push(self.array_buffer);
        buf_refs.push(self.element_buffer);
        for (i, b) in self.buffers.items.iter_mut().enumerate() {
            let n = i as u32 + 1;
            if b.as_ref().is_some_and(|b| b.deleted) && !buf_refs.contains(&n) {
                *b = None;
            }
        }
        let mut tex_refs: Vec<u32> = Vec::new();
        let mut rb_refs: Vec<u32> = Vec::new();
        for f in self.framebuffers.items.iter().flatten() {
            for a in f.points() {
                match a {
                    Attach::Tex(n, _, _) => tex_refs.push(n),
                    Attach::Rb(n) => rb_refs.push(n),
                    Attach::None => {}
                }
            }
        }
        for u in &self.units {
            tex_refs.push(u.0);
            tex_refs.push(u.1);
        }
        rb_refs.push(self.renderbuffer);
        for (i, t) in self.textures.items.iter_mut().enumerate() {
            if t.as_ref().is_some_and(|t| t.deleted) && !tex_refs.contains(&(i as u32 + 1)) {
                *t = None;
            }
        }
        for (i, r) in self.renderbuffers.items.iter_mut().enumerate() {
            if r.as_ref().is_some_and(|r| r.deleted) && !rb_refs.contains(&(i as u32 + 1)) {
                *r = None;
            }
        }
        for (i, f) in self.framebuffers.items.iter_mut().enumerate() {
            if f.as_ref().is_some_and(|f| f.deleted) && self.framebuffer != i as u32 + 1 {
                *f = None;
            }
        }
        let cur = self.current_program;
        let mut detach: Vec<u32> = Vec::new();
        for (i, p) in self.programs.items.iter_mut().enumerate() {
            if p.as_ref().is_some_and(|p| p.deleted) && cur != i as u32 + 1 {
                if let Some(p) = p.as_ref() {
                    detach.push(p.vs);
                    detach.push(p.fs);
                }
                *p = None;
            }
        }
        for s in detach {
            if let Some(sh) = self.shaders.get_mut(s) {
                sh.attached = sh.attached.saturating_sub(1);
            }
        }
        for s in self.shaders.items.iter_mut() {
            if s.as_ref().is_some_and(|s| s.deleted && s.attached == 0) {
                *s = None;
            }
        }
    }

    // ------------------------------------------------------------------------------------------ bindings

    pub fn bind_buffer(&mut self, target: u32, name: u32) {
        if target != ARRAY_BUFFER && target != ELEMENT_ARRAY_BUFFER {
            return self.err(INVALID_ENUM);
        }
        if name != 0 {
            let Some(b) = self.buffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
            if b.deleted {
                return self.err(INVALID_OPERATION);
            }
            if b.target != 0 && b.target != target {
                return self.err(INVALID_OPERATION);
            }
            b.target = target;
        }
        if target == ARRAY_BUFFER {
            self.array_buffer = name;
        } else {
            self.element_buffer = name;
        }
        self.sweep();
    }

    pub fn bind_texture(&mut self, target: u32, name: u32) {
        if target != TEXTURE_2D && target != TEXTURE_CUBE_MAP {
            return self.err(INVALID_ENUM);
        }
        if name != 0 {
            let Some(t) = self.textures.get_mut(name) else { return self.err(INVALID_OPERATION) };
            if t.deleted || (t.target != 0 && t.target != target) {
                return self.err(INVALID_OPERATION);
            }
            t.target = target;
            t.ensure_faces();
        }
        let u = &mut self.units[self.active_unit as usize];
        if target == TEXTURE_2D {
            u.0 = name;
        } else {
            u.1 = name;
        }
        self.sweep();
    }

    pub fn bind_renderbuffer(&mut self, target: u32, name: u32) {
        if target != RENDERBUFFER {
            return self.err(INVALID_ENUM);
        }
        if name != 0 {
            let Some(r) = self.renderbuffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
            if r.deleted {
                return self.err(INVALID_OPERATION);
            }
            r.bound = true;
        }
        self.renderbuffer = name;
        self.sweep();
    }

    pub fn bind_framebuffer(&mut self, target: u32, name: u32) {
        if target != FRAMEBUFFER {
            return self.err(INVALID_ENUM);
        }
        if name != 0 {
            let Some(f) = self.framebuffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
            if f.deleted {
                return self.err(INVALID_OPERATION);
            }
            f.bound = true;
        }
        self.framebuffer = name;
        self.sweep();
    }

    pub fn active_texture(&mut self, unit: u32) {
        if unit < TEXTURE0 || (unit - TEXTURE0) as usize >= MAX_COMBINED_UNITS {
            return self.err(INVALID_ENUM);
        }
        self.active_unit = unit - TEXTURE0;
    }

    /// The texture bound to `target` (TEXTURE_2D / TEXTURE_CUBE_MAP) on the active unit.
    pub fn bound_texture(&self, target: u32) -> u32 {
        let u = self.units[self.active_unit as usize];
        if target == TEXTURE_2D { u.0 } else { u.1 }
    }

    // ------------------------------------------------------------------------------------------ buffers

    fn bound_buffer(&self, target: u32) -> Option<u32> {
        match target {
            ARRAY_BUFFER => Some(self.array_buffer),
            ELEMENT_ARRAY_BUFFER => Some(self.element_buffer),
            _ => None,
        }
    }

    /// ES 2.0 §2.9.2 bufferData: `data` None = `size` zero bytes (WebGL 1.0 bufferData: a size-only store is zero-filled).
    pub fn buffer_data(&mut self, target: u32, size: i64, data: Option<&[u8]>, usage: u32) {
        let Some(name) = self.bound_buffer(target) else { return self.err(INVALID_ENUM) };
        if !matches!(usage, STREAM_DRAW | STATIC_DRAW | DYNAMIC_DRAW) {
            return self.err(INVALID_ENUM);
        }
        if data.is_none() && size < 0 {
            return self.err(INVALID_VALUE);
        }
        if name == 0 {
            return self.err(INVALID_OPERATION);
        }
        if data.is_none() && size > (1 << 30) {
            return self.err(OUT_OF_MEMORY);
        }
        let Some(b) = self.buffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
        b.data = match data {
            Some(d) => d.to_vec(),
            None => vec![0; size as usize],
        };
        b.usage = usage;
    }

    /// ES 2.0 §2.9.2 bufferSubData (WebGL 1.0 bufferSubData: out of range is INVALID_VALUE).
    pub fn buffer_sub_data(&mut self, target: u32, offset: i64, data: &[u8]) {
        let Some(name) = self.bound_buffer(target) else { return self.err(INVALID_ENUM) };
        if offset < 0 {
            return self.err(INVALID_VALUE);
        }
        if name == 0 {
            return self.err(INVALID_OPERATION);
        }
        let Some(b) = self.buffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
        let off = offset as usize;
        if off.checked_add(data.len()).is_none_or(|e| e > b.data.len()) {
            return self.err(INVALID_VALUE);
        }
        b.data[off..off + data.len()].copy_from_slice(data);
    }

    pub fn get_buffer_parameter(&mut self, target: u32, pname: u32) -> Param {
        let Some(name) = self.bound_buffer(target) else {
            self.err(INVALID_ENUM);
            return Param::Null;
        };
        if pname != BUFFER_SIZE && pname != BUFFER_USAGE {
            self.err(INVALID_ENUM);
            return Param::Null;
        }
        let Some(b) = self.buffers.get(name) else {
            self.err(INVALID_OPERATION);
            return Param::Null;
        };
        if pname == BUFFER_SIZE { Param::Int(b.data.len() as i64) } else { Param::Int(b.usage as i64) }
    }

    // ------------------------------------------------------------------------------------------ vertex attribs

    pub fn enable_vertex_attrib_array(&mut self, index: u32, on: bool) {
        if index as usize >= MAX_ATTRIBS {
            return self.err(INVALID_VALUE);
        }
        self.attribs[index as usize].enabled = on;
    }

    /// ES 2.0 §2.8 VertexAttribPointer with WebGL 1.0 "Vertex Attribute Data Stride" (stride ≤ 255), "Buffer Offset and
    /// Stride Requirements" (multiples of the type size) and the no-ARRAY_BUFFER rule with a non-zero offset.
    #[allow(clippy::too_many_arguments)]
    pub fn vertex_attrib_pointer(&mut self, index: u32, size: i32, ty: u32, normalized: bool, stride: i32, offset: i64) {
        if index as usize >= MAX_ATTRIBS {
            return self.err(INVALID_VALUE);
        }
        if !(1..=4).contains(&size) {
            return self.err(INVALID_VALUE);
        }
        let tsize = match ty {
            BYTE | UNSIGNED_BYTE => 1,
            SHORT | UNSIGNED_SHORT => 2,
            FLOAT => 4,
            _ => return self.err(INVALID_ENUM),
        };
        if !(0..=255).contains(&stride) || offset < 0 {
            return self.err(INVALID_VALUE);
        }
        if stride % tsize != 0 || offset % tsize as i64 != 0 {
            return self.err(INVALID_OPERATION);
        }
        if self.array_buffer == 0 && offset != 0 {
            return self.err(INVALID_OPERATION);
        }
        let a = &mut self.attribs[index as usize];
        a.size = size;
        a.ty = ty;
        a.normalized = normalized;
        a.stride = stride;
        a.offset = offset as u32;
        a.buffer = self.array_buffer;
        self.sweep();
    }

    /// vertexAttrib{1,2,3,4}f[v]: the missing components are (0, 0, 1) (ES 2.0 §2.7).
    pub fn vertex_attrib(&mut self, index: u32, v: &[f32]) {
        if index as usize >= MAX_ATTRIBS {
            return self.err(INVALID_VALUE);
        }
        let mut c = [0.0, 0.0, 0.0, 1.0];
        for (i, x) in v.iter().take(4).enumerate() {
            c[i] = *x;
        }
        self.attribs[index as usize].current = c;
    }

    pub fn get_vertex_attrib(&mut self, index: u32, pname: u32) -> Param {
        if index as usize >= MAX_ATTRIBS {
            self.err(INVALID_VALUE);
            return Param::Null;
        }
        let a = self.attribs[index as usize];
        match pname {
            VERTEX_ATTRIB_ARRAY_BUFFER_BINDING => {
                if a.buffer == 0 { Param::Null } else { Param::Object(ObjKind::Buffer, a.buffer) }
            }
            VERTEX_ATTRIB_ARRAY_ENABLED => Param::Bool(a.enabled),
            VERTEX_ATTRIB_ARRAY_SIZE => Param::Int(a.size as i64),
            VERTEX_ATTRIB_ARRAY_STRIDE => Param::Int(a.stride as i64),
            VERTEX_ATTRIB_ARRAY_TYPE => Param::Int(a.ty as i64),
            VERTEX_ATTRIB_ARRAY_NORMALIZED => Param::Bool(a.normalized),
            CURRENT_VERTEX_ATTRIB => Param::F32s(a.current.to_vec()),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    pub fn get_vertex_attrib_offset(&mut self, index: u32, pname: u32) -> i64 {
        if index as usize >= MAX_ATTRIBS {
            self.err(INVALID_VALUE);
            return 0;
        }
        if pname != VERTEX_ATTRIB_ARRAY_POINTER {
            self.err(INVALID_ENUM);
            return 0;
        }
        self.attribs[index as usize].offset as i64
    }

    // ------------------------------------------------------------------------------------------ fixed state

    fn cap(&mut self, cap: u32) -> Option<&mut bool> {
        let s = &mut self.st;
        Some(match cap {
            BLEND => &mut s.blend,
            CULL_FACE => &mut s.cull,
            DEPTH_TEST => &mut s.depth_test,
            DITHER => &mut s.dither,
            POLYGON_OFFSET_FILL => &mut s.polygon_offset_fill,
            SAMPLE_ALPHA_TO_COVERAGE => &mut s.sample_alpha_to_coverage,
            SAMPLE_COVERAGE => &mut s.sample_coverage,
            SCISSOR_TEST => &mut s.scissor_test,
            STENCIL_TEST => &mut s.stencil_test,
            _ => return None,
        })
    }

    pub fn enable(&mut self, cap: u32, on: bool) {
        match self.cap(cap) {
            Some(b) => *b = on,
            None => self.err(INVALID_ENUM),
        }
    }

    pub fn is_enabled(&mut self, cap: u32) -> bool {
        match self.cap(cap) {
            Some(b) => *b,
            None => {
                self.err(INVALID_ENUM);
                false
            }
        }
    }

    fn valid_factor(f: u32, src: bool) -> bool {
        matches!(
            f,
            ZERO | ONE
                | SRC_COLOR
                | ONE_MINUS_SRC_COLOR
                | DST_COLOR
                | ONE_MINUS_DST_COLOR
                | SRC_ALPHA
                | ONE_MINUS_SRC_ALPHA
                | DST_ALPHA
                | ONE_MINUS_DST_ALPHA
                | CONSTANT_COLOR
                | ONE_MINUS_CONSTANT_COLOR
                | CONSTANT_ALPHA
                | ONE_MINUS_CONSTANT_ALPHA
        ) || (src && f == SRC_ALPHA_SATURATE)
    }

    /// WebGL 1.0 "Blending With Constant Color": a constant-colour factor and a constant-alpha factor may not be paired.
    fn constant_conflict(a: u32, b: u32) -> bool {
        let cc = |f| f == CONSTANT_COLOR || f == ONE_MINUS_CONSTANT_COLOR;
        let ca = |f| f == CONSTANT_ALPHA || f == ONE_MINUS_CONSTANT_ALPHA;
        (cc(a) && ca(b)) || (ca(a) && cc(b))
    }

    pub fn blend_func_separate(&mut self, src_rgb: u32, dst_rgb: u32, src_a: u32, dst_a: u32) {
        if !Self::valid_factor(src_rgb, true) || !Self::valid_factor(dst_rgb, false) || !Self::valid_factor(src_a, true) || !Self::valid_factor(dst_a, false) {
            return self.err(INVALID_ENUM);
        }
        if Self::constant_conflict(src_rgb, dst_rgb) {
            return self.err(INVALID_OPERATION);
        }
        self.st.blend_src = [src_rgb, src_a];
        self.st.blend_dst = [dst_rgb, dst_a];
    }

    pub fn blend_equation_separate(&mut self, rgb: u32, alpha: u32) {
        let ok = |e| matches!(e, FUNC_ADD | FUNC_SUBTRACT | FUNC_REVERSE_SUBTRACT);
        if !ok(rgb) || !ok(alpha) {
            return self.err(INVALID_ENUM);
        }
        self.st.blend_eq = [rgb, alpha];
    }

    pub fn blend_color(&mut self, c: [f32; 4]) {
        self.st.blend_color = c.map(|v| v.clamp(0.0, 1.0));
    }

    fn valid_func(f: u32) -> bool {
        (NEVER..=ALWAYS).contains(&f)
    }

    fn valid_stencil_op(o: u32) -> bool {
        matches!(o, KEEP | ZERO | REPLACE | INCR | DECR | INVERT | INCR_WRAP | DECR_WRAP)
    }

    pub fn depth_func(&mut self, f: u32) {
        if !Self::valid_func(f) {
            return self.err(INVALID_ENUM);
        }
        self.st.depth_func = f;
    }

    fn faces(&mut self, face: u32) -> Option<(bool, bool)> {
        match face {
            FRONT => Some((true, false)),
            BACK => Some((false, true)),
            FRONT_AND_BACK => Some((true, true)),
            _ => {
                self.err(INVALID_ENUM);
                None
            }
        }
    }

    pub fn stencil_func_separate(&mut self, face: u32, func: u32, reference: i32, mask: u32) {
        let Some((f, b)) = self.faces(face) else { return };
        if !Self::valid_func(func) {
            return self.err(INVALID_ENUM);
        }
        for (on, s) in [(f, &mut self.st.front), (b, &mut self.st.back)] {
            if on {
                s.func = func;
                s.reference = reference;
                s.value_mask = mask;
            }
        }
    }

    pub fn stencil_op_separate(&mut self, face: u32, fail: u32, zfail: u32, zpass: u32) {
        let Some((f, b)) = self.faces(face) else { return };
        if !Self::valid_stencil_op(fail) || !Self::valid_stencil_op(zfail) || !Self::valid_stencil_op(zpass) {
            return self.err(INVALID_ENUM);
        }
        for (on, s) in [(f, &mut self.st.front), (b, &mut self.st.back)] {
            if on {
                s.fail = fail;
                s.zfail = zfail;
                s.zpass = zpass;
            }
        }
    }

    pub fn stencil_mask_separate(&mut self, face: u32, mask: u32) {
        let Some((f, b)) = self.faces(face) else { return };
        if f {
            self.st.front.write_mask = mask;
        }
        if b {
            self.st.back.write_mask = mask;
        }
    }

    pub fn cull_face(&mut self, mode: u32) {
        if !matches!(mode, FRONT | BACK | FRONT_AND_BACK) {
            return self.err(INVALID_ENUM);
        }
        self.st.cull_mode = mode;
    }

    pub fn front_face(&mut self, mode: u32) {
        if mode != CW && mode != CCW {
            return self.err(INVALID_ENUM);
        }
        self.st.front_face = mode;
    }

    pub fn line_width(&mut self, w: f32) {
        if w.is_nan() || w <= 0.0 {
            return self.err(INVALID_VALUE);
        }
        self.st.line_width = w;
    }

    /// ES 2.0 §2.12.1 DepthRangef, WebGL 1.0 "Viewport Depth Range": zNear > zFar is INVALID_OPERATION.
    pub fn depth_range(&mut self, n: f32, f: f32) {
        if n > f {
            return self.err(INVALID_OPERATION);
        }
        self.st.depth_range = [n.clamp(0.0, 1.0), f.clamp(0.0, 1.0)];
    }

    pub fn viewport(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w < 0 || h < 0 {
            return self.err(INVALID_VALUE);
        }
        self.st.viewport = [x, y, w.min(MAX_VIEWPORT_V), h.min(MAX_VIEWPORT_V)];
    }

    pub fn scissor(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w < 0 || h < 0 {
            return self.err(INVALID_VALUE);
        }
        self.st.scissor = [x, y, w, h];
    }

    pub fn clear_color(&mut self, c: [f32; 4]) {
        self.st.clear_color = c.map(|v| v.clamp(0.0, 1.0));
    }

    pub fn clear_depth(&mut self, d: f32) {
        self.st.clear_depth = d.clamp(0.0, 1.0);
    }

    pub fn clear_stencil(&mut self, s: i32) {
        self.st.clear_stencil = s;
    }

    pub fn color_mask(&mut self, m: [bool; 4]) {
        self.st.color_mask = m;
    }

    pub fn depth_mask(&mut self, m: bool) {
        self.st.depth_mask = m;
    }

    pub fn polygon_offset(&mut self, factor: f32, units: f32) {
        self.st.po_factor = factor;
        self.st.po_units = units;
    }

    pub fn sample_coverage(&mut self, value: f32, invert: bool) {
        self.st.sample_coverage_value = value.clamp(0.0, 1.0);
        self.st.sample_coverage_invert = invert;
    }

    pub fn hint(&mut self, target: u32, mode: u32) {
        if !matches!(mode, DONT_CARE | FASTEST | NICEST) {
            return self.err(INVALID_ENUM);
        }
        match target {
            GENERATE_MIPMAP_HINT => self.st.mipmap_hint = mode,
            FRAGMENT_SHADER_DERIVATIVE_HINT_OES if self.ext.standard_derivatives => self.st.derivative_hint = mode,
            _ => self.err(INVALID_ENUM),
        }
    }

    /// ES 2.0 §3.6.1 PixelStorei with WebGL 1.0's three WebGL parameters ("Pixel Storage Parameters").
    pub fn pixel_storei(&mut self, pname: u32, v: i32) {
        match pname {
            PACK_ALIGNMENT | UNPACK_ALIGNMENT => {
                if !matches!(v, 1 | 2 | 4 | 8) {
                    return self.err(INVALID_VALUE);
                }
                if pname == PACK_ALIGNMENT {
                    self.st.pack_alignment = v;
                } else {
                    self.st.unpack_alignment = v;
                }
            }
            UNPACK_FLIP_Y_WEBGL => self.st.unpack_flip_y = v != 0,
            UNPACK_PREMULTIPLY_ALPHA_WEBGL => self.st.unpack_premultiply = v != 0,
            UNPACK_COLORSPACE_CONVERSION_WEBGL => {
                if v as u32 != BROWSER_DEFAULT_WEBGL && v as u32 != NONE {
                    return self.err(INVALID_ENUM);
                }
                self.st.unpack_colorspace = v as u32;
            }
            _ => self.err(INVALID_ENUM),
        }
    }

    // ------------------------------------------------------------------------------------------ textures

    /// The texture bound to `target` on the active unit, or the error ES 2.0 §3.7.4 names.
    pub(crate) fn tex_for(&mut self, target: u32) -> Option<u32> {
        if target != TEXTURE_2D && target != TEXTURE_CUBE_MAP {
            self.err(INVALID_ENUM);
            return None;
        }
        let n = self.bound_texture(target);
        if n == 0 {
            self.err(INVALID_OPERATION);
            return None;
        }
        Some(n)
    }

    /// ES 2.0 §3.7.4 TexParameter (int and float forms land here; the values are enums).
    pub fn tex_parameter(&mut self, target: u32, pname: u32, param: i32) {
        let valid_target = target == TEXTURE_2D || target == TEXTURE_CUBE_MAP;
        if !valid_target {
            return self.err(INVALID_ENUM);
        }
        let p = param as u32;
        let ok = match pname {
            TEXTURE_MIN_FILTER => matches!(p, NEAREST | LINEAR | NEAREST_MIPMAP_NEAREST | LINEAR_MIPMAP_NEAREST | NEAREST_MIPMAP_LINEAR | LINEAR_MIPMAP_LINEAR),
            TEXTURE_MAG_FILTER => matches!(p, NEAREST | LINEAR),
            TEXTURE_WRAP_S | TEXTURE_WRAP_T => matches!(p, REPEAT | CLAMP_TO_EDGE | MIRRORED_REPEAT),
            _ => return self.err(INVALID_ENUM),
        };
        if !ok {
            return self.err(INVALID_ENUM);
        }
        let Some(n) = self.tex_for(target) else { return };
        let t = self.textures.get_mut(n).unwrap();
        match pname {
            TEXTURE_MIN_FILTER => t.min_filter = p,
            TEXTURE_MAG_FILTER => t.mag_filter = p,
            TEXTURE_WRAP_S => t.wrap_s = p,
            _ => t.wrap_t = p,
        }
    }

    pub fn get_tex_parameter(&mut self, target: u32, pname: u32) -> Param {
        if target != TEXTURE_2D && target != TEXTURE_CUBE_MAP {
            self.err(INVALID_ENUM);
            return Param::Null;
        }
        if !matches!(pname, TEXTURE_MIN_FILTER | TEXTURE_MAG_FILTER | TEXTURE_WRAP_S | TEXTURE_WRAP_T) {
            self.err(INVALID_ENUM);
            return Param::Null;
        }
        let Some(n) = self.tex_for(target) else { return Param::Null };
        let t = self.textures.get(n).unwrap();
        Param::Int(match pname {
            TEXTURE_MIN_FILTER => t.min_filter,
            TEXTURE_MAG_FILTER => t.mag_filter,
            TEXTURE_WRAP_S => t.wrap_s,
            _ => t.wrap_t,
        } as i64)
    }

    // ------------------------------------------------------------------------------------------ queries

    fn bits(&self) -> [i64; 6] {
        // (red, green, blue, alpha, depth, stencil) of the current draw framebuffer (ES 2.0 Table 6.21).
        if self.framebuffer == 0 {
            return [8, 8, 8, if self.attrs.alpha { 8 } else { 0 }, if self.attrs.depth { 24 } else { 0 }, if self.attrs.stencil { 8 } else { 0 }];
        }
        let f = self.framebuffers.get(self.framebuffer).unwrap();
        let mut out = [0i64; 6];
        let rb_bits = |fmt: u32| -> [i64; 6] {
            match fmt {
                RGBA4 => [4, 4, 4, 4, 0, 0],
                RGB565 => [5, 6, 5, 0, 0, 0],
                RGB5_A1 => [5, 5, 5, 1, 0, 0],
                DEPTH_COMPONENT16 => [0, 0, 0, 0, 16, 0],
                STENCIL_INDEX8 => [0, 0, 0, 0, 0, 8],
                DEPTH_STENCIL => [0, 0, 0, 0, 24, 8],
                _ => [0; 6],
            }
        };
        for a in f.points() {
            let b = match a {
                Attach::Rb(r) => self.renderbuffers.get(r).map(|r| rb_bits(r.format)).unwrap_or([0; 6]),
                Attach::Tex(t, target, level) => self
                    .textures
                    .get(t)
                    .and_then(|t| t.level(texture::face_index(target).unwrap_or(0), level as usize))
                    .map(|l| match (l.format, l.ty) {
                        (RGBA, UNSIGNED_BYTE) => [8, 8, 8, 8, 0, 0],
                        (RGB, UNSIGNED_BYTE) => [8, 8, 8, 0, 0, 0],
                        (_, UNSIGNED_SHORT_4_4_4_4) => [4, 4, 4, 4, 0, 0],
                        (_, UNSIGNED_SHORT_5_5_5_1) => [5, 5, 5, 1, 0, 0],
                        (_, UNSIGNED_SHORT_5_6_5) => [5, 6, 5, 0, 0, 0],
                        _ => [0; 6],
                    })
                    .unwrap_or([0; 6]),
                Attach::None => [0; 6],
            };
            for i in 0..6 {
                out[i] = out[i].max(b[i]);
            }
        }
        out
    }

    fn obj(kind: ObjKind, n: u32) -> Param {
        if n == 0 { Param::Null } else { Param::Object(kind, n) }
    }

    /// WebGL 1.0 getParameter: every pname of the table there, typed as it says.
    pub fn get_parameter(&mut self, pname: u32) -> Param {
        let s = self.st.clone();
        let f2 = |a: f32, b: f32| Param::F32s(vec![a, b]);
        let face = |f: &StencilFace, which: u32| -> Param {
            match which {
                0 => Param::Int(f.func as i64),
                1 => Param::Int(f.fail as i64),
                2 => Param::Int(f.zfail as i64),
                3 => Param::Int(f.zpass as i64),
                4 => Param::Int(f.reference as i64),
                5 => Param::Int(f.value_mask as i64),
                _ => Param::Int(f.write_mask as i64),
            }
        };
        let bits = self.bits();
        match pname {
            ACTIVE_TEXTURE => Param::Int((TEXTURE0 + self.active_unit) as i64),
            ALIASED_LINE_WIDTH_RANGE => f2(1.0, 1.0),
            ALIASED_POINT_SIZE_RANGE => f2(1.0, POINT_SIZE_MAX),
            RED_BITS => Param::Int(bits[0]),
            GREEN_BITS => Param::Int(bits[1]),
            BLUE_BITS => Param::Int(bits[2]),
            ALPHA_BITS => Param::Int(bits[3]),
            DEPTH_BITS => Param::Int(bits[4]),
            STENCIL_BITS => Param::Int(bits[5]),
            ARRAY_BUFFER_BINDING => Self::obj(ObjKind::Buffer, self.array_buffer),
            ELEMENT_ARRAY_BUFFER_BINDING => Self::obj(ObjKind::Buffer, self.element_buffer),
            BLEND => Param::Bool(s.blend),
            BLEND_COLOR => Param::F32s(s.blend_color.to_vec()),
            BLEND_DST_ALPHA => Param::Int(s.blend_dst[1] as i64),
            BLEND_DST_RGB => Param::Int(s.blend_dst[0] as i64),
            BLEND_SRC_ALPHA => Param::Int(s.blend_src[1] as i64),
            BLEND_SRC_RGB => Param::Int(s.blend_src[0] as i64),
            BLEND_EQUATION_RGB => Param::Int(s.blend_eq[0] as i64),
            BLEND_EQUATION_ALPHA => Param::Int(s.blend_eq[1] as i64),
            COLOR_CLEAR_VALUE => Param::F32s(s.clear_color.to_vec()),
            COLOR_WRITEMASK => Param::Bools(s.color_mask.to_vec()),
            COMPRESSED_TEXTURE_FORMATS => Param::U32s(Vec::new()),
            CULL_FACE => Param::Bool(s.cull),
            CULL_FACE_MODE => Param::Int(s.cull_mode as i64),
            CURRENT_PROGRAM => Self::obj(ObjKind::Program, self.current_program),
            DEPTH_CLEAR_VALUE => Param::Float(s.clear_depth as f64),
            DEPTH_FUNC => Param::Int(s.depth_func as i64),
            DEPTH_RANGE => f2(s.depth_range[0], s.depth_range[1]),
            DEPTH_TEST => Param::Bool(s.depth_test),
            DEPTH_WRITEMASK => Param::Bool(s.depth_mask),
            DITHER => Param::Bool(s.dither),
            FRAMEBUFFER_BINDING => Self::obj(ObjKind::Framebuffer, self.framebuffer),
            FRONT_FACE => Param::Int(s.front_face as i64),
            GENERATE_MIPMAP_HINT => Param::Int(s.mipmap_hint as i64),
            FRAGMENT_SHADER_DERIVATIVE_HINT_OES if self.ext.standard_derivatives => Param::Int(s.derivative_hint as i64),
            IMPLEMENTATION_COLOR_READ_FORMAT => Param::Int(RGBA as i64),
            IMPLEMENTATION_COLOR_READ_TYPE => Param::Int(UNSIGNED_BYTE as i64),
            LINE_WIDTH => Param::Float(s.line_width as f64),
            MAX_COMBINED_TEXTURE_IMAGE_UNITS => Param::Int(MAX_COMBINED_UNITS as i64),
            MAX_CUBE_MAP_TEXTURE_SIZE => Param::Int(MAX_CUBE_SIZE_V as i64),
            MAX_FRAGMENT_UNIFORM_VECTORS => Param::Int(MAX_FRAGMENT_UNIFORM_VECTORS_V as i64),
            MAX_RENDERBUFFER_SIZE => Param::Int(MAX_RENDERBUFFER_SIZE_V as i64),
            MAX_TEXTURE_IMAGE_UNITS => Param::Int(MAX_TEXTURE_UNITS_V as i64),
            MAX_TEXTURE_SIZE => Param::Int(MAX_TEXTURE_SIZE_V as i64),
            MAX_VARYING_VECTORS => Param::Int(MAX_VARYING_VECTORS_V as i64),
            MAX_VERTEX_ATTRIBS => Param::Int(MAX_ATTRIBS as i64),
            MAX_VERTEX_TEXTURE_IMAGE_UNITS => Param::Int(MAX_VERTEX_TEXTURE_UNITS_V as i64),
            MAX_VERTEX_UNIFORM_VECTORS => Param::Int(MAX_VERTEX_UNIFORM_VECTORS_V as i64),
            MAX_VIEWPORT_DIMS => Param::I32s(vec![MAX_VIEWPORT_V, MAX_VIEWPORT_V]),
            PACK_ALIGNMENT => Param::Int(s.pack_alignment as i64),
            POLYGON_OFFSET_FACTOR => Param::Float(s.po_factor as f64),
            POLYGON_OFFSET_FILL => Param::Bool(s.polygon_offset_fill),
            POLYGON_OFFSET_UNITS => Param::Float(s.po_units as f64),
            RENDERBUFFER_BINDING => Self::obj(ObjKind::Renderbuffer, self.renderbuffer),
            RENDERER => Param::Str("UnaOS gl_core (software)".into()),
            SAMPLE_ALPHA_TO_COVERAGE => Param::Bool(s.sample_alpha_to_coverage),
            SAMPLE_BUFFERS => Param::Int(0),
            SAMPLE_COVERAGE => Param::Bool(s.sample_coverage),
            SAMPLE_COVERAGE_INVERT => Param::Bool(s.sample_coverage_invert),
            SAMPLE_COVERAGE_VALUE => Param::Float(s.sample_coverage_value as f64),
            SAMPLES => Param::Int(0),
            SCISSOR_BOX => Param::I32s(s.scissor.to_vec()),
            SCISSOR_TEST => Param::Bool(s.scissor_test),
            SHADING_LANGUAGE_VERSION => Param::Str("WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 UnaOS gl_core)".into()),
            STENCIL_FUNC => face(&s.front, 0),
            STENCIL_FAIL => face(&s.front, 1),
            STENCIL_PASS_DEPTH_FAIL => face(&s.front, 2),
            STENCIL_PASS_DEPTH_PASS => face(&s.front, 3),
            STENCIL_REF => face(&s.front, 4),
            STENCIL_VALUE_MASK => face(&s.front, 5),
            STENCIL_WRITEMASK => face(&s.front, 6),
            STENCIL_BACK_FUNC => face(&s.back, 0),
            STENCIL_BACK_FAIL => face(&s.back, 1),
            STENCIL_BACK_PASS_DEPTH_FAIL => face(&s.back, 2),
            STENCIL_BACK_PASS_DEPTH_PASS => face(&s.back, 3),
            STENCIL_BACK_REF => face(&s.back, 4),
            STENCIL_BACK_VALUE_MASK => face(&s.back, 5),
            STENCIL_BACK_WRITEMASK => face(&s.back, 6),
            STENCIL_CLEAR_VALUE => Param::Int(s.clear_stencil as i64),
            STENCIL_TEST => Param::Bool(s.stencil_test),
            SUBPIXEL_BITS => Param::Int(8),
            TEXTURE_BINDING_2D => Self::obj(ObjKind::Texture, self.units[self.active_unit as usize].0),
            TEXTURE_BINDING_CUBE_MAP => Self::obj(ObjKind::Texture, self.units[self.active_unit as usize].1),
            UNPACK_ALIGNMENT => Param::Int(s.unpack_alignment as i64),
            UNPACK_COLORSPACE_CONVERSION_WEBGL => Param::Int(s.unpack_colorspace as i64),
            UNPACK_FLIP_Y_WEBGL => Param::Bool(s.unpack_flip_y),
            UNPACK_PREMULTIPLY_ALPHA_WEBGL => Param::Bool(s.unpack_premultiply),
            VENDOR => Param::Str("UnaOS".into()),
            VERSION => Param::Str("WebGL 1.0 (OpenGL ES 2.0 UnaOS gl_core)".into()),
            VIEWPORT => Param::I32s(s.viewport.to_vec()),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    /// WebGL 1.0 getShaderPrecisionFormat: every type is evaluated at IEEE single precision (the
    /// interpreter's lanes are f32), so all three precisions report highp's (127, 127, 23) and the ints (24, 24, 0) —
    /// 24-bit integers are what an f32 lane holds exactly.
    pub fn get_shader_precision_format(&mut self, shader: u32, prec: u32) -> Option<PrecisionFormat> {
        if shader != VERTEX_SHADER && shader != FRAGMENT_SHADER {
            self.err(INVALID_ENUM);
            return None;
        }
        match prec {
            LOW_FLOAT | MEDIUM_FLOAT | HIGH_FLOAT => Some(PrecisionFormat { range_min: 127, range_max: 127, precision: 23 }),
            LOW_INT | MEDIUM_INT | HIGH_INT => Some(PrecisionFormat { range_min: 24, range_max: 24, precision: 0 }),
            _ => {
                self.err(INVALID_ENUM);
                None
            }
        }
    }
}
