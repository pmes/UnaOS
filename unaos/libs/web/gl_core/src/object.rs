//! The OpenGL ES 2.0 objects (ES 2.0 §2.9 buffers, §2.10.3 programs and shaders, §4.4 framebuffers and
//! renderbuffers; textures live in [`crate::texture`]). Names are never reused (WebGL 1.0 "WebGLObject": a deleted
//! WebGLObject must stay deleted, and a recycled name would make a stale wrapper alias a new object), so each
//! store is `Vec<Option<T>>` indexed by `name - 1`.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

/// A name-indexed object store.
pub struct Store<T> {
    pub items: Vec<Option<T>>,
}

impl<T> Default for Store<T> {
    fn default() -> Self {
        Store { items: Vec::new() }
    }
}

impl<T> Store<T> {
    pub fn insert(&mut self, t: T) -> u32 {
        self.items.push(Some(t));
        self.items.len() as u32
    }
    pub fn get(&self, name: u32) -> Option<&T> {
        if name == 0 {
            return None;
        }
        self.items.get(name as usize - 1).and_then(|o| o.as_ref())
    }
    pub fn get_mut(&mut self, name: u32) -> Option<&mut T> {
        if name == 0 {
            return None;
        }
        self.items.get_mut(name as usize - 1).and_then(|o| o.as_mut())
    }
}

/// ES 2.0 §2.9: a buffer object. `target` is fixed by the first bind (WebGL 1.0 "Buffer Object Binding": a buffer bound to
/// ARRAY_BUFFER can never be bound to ELEMENT_ARRAY_BUFFER and vice versa).
pub struct Buffer {
    pub target: u32,
    pub data: Vec<u8>,
    pub usage: u32,
    pub deleted: bool,
}

/// ES 2.0 §2.10.1: a shader object.
pub struct Shader {
    pub kind: u32,
    pub source: String,
    pub compiled: bool,
    pub log: String,
    pub deleted: bool,
    pub attached: u32,
    pub code: Option<Rc<crate::glsl::Compiled>>,
}

/// ES 2.0 §2.10.3: a program object. `bindings` are the bindAttribLocation requests, applied at the next link.
pub struct Program {
    pub vs: u32,
    pub fs: u32,
    pub deleted: bool,
    pub linked: bool,
    pub log: String,
    pub validated: bool,
    /// Bumped on every link attempt: a WebGLUniformLocation from an older link is invalid (WebGL 1.0 "Uniforms and attributes").
    pub link_gen: u32,
    pub bindings: BTreeMap<String, u32>,
    pub linked_data: Option<Rc<crate::link::Linked>>,
    /// The uniform store (one vec4 row per uniform register; a mat4 element is 4 rows), indexed by the link's
    /// row offsets; zero after every successful link (ES 2.0 §2.10.4: uniforms start at 0).
    pub store: Vec<[f32; 4]>,
}

/// A renderbuffer's storage (ES 2.0 §4.4.2; WebGL 1.0 "Framebuffer Object Attachments" adds DEPTH_STENCIL).
pub struct Renderbuffer {
    pub format: u32,
    pub w: u32,
    pub h: u32,
    /// Colour as RGBA8 (quantized to the format's bits on write), or empty.
    pub color: Vec<u8>,
    /// Depth as fixed point at the format's precision (16 bits for DEPTH_COMPONENT16, 24 inside DEPTH_STENCIL).
    pub depth: Vec<u32>,
    pub stencil: Vec<u8>,
    pub deleted: bool,
    pub bound: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Attach {
    #[default]
    None,
    Rb(u32),
    /// texture name, the image target (TEXTURE_2D or a cube face), level.
    Tex(u32, u32, i32),
}

/// ES 2.0 §4.4.1: a framebuffer object. WebGL 1.0 "Framebuffer Object Attachments" keeps DEPTH_STENCIL_ATTACHMENT as its own point.
#[derive(Default)]
pub struct Framebuffer {
    pub color: Attach,
    pub depth: Attach,
    pub stencil: Attach,
    pub depth_stencil: Attach,
    pub deleted: bool,
    pub bound: bool,
}

impl Framebuffer {
    pub fn points(&self) -> [Attach; 4] {
        [self.color, self.depth, self.stencil, self.depth_stencil]
    }
}
