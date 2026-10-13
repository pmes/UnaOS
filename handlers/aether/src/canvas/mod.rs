//! CHARTER: Aether (CODEX §2, Web) — seam: handler-internal (Ring 3 host). AETHERCANVAS (LEDGER SR85).
//!
//! The `<canvas>` 2D context's engine, independent of script: [`geom`] (CTM, paths, Path2D's SVG path data,
//! arc rules), [`raster`] (coverage through font_core's exact-area rasterizer, the stroker, dashes, the
//! shadow blur), [`paint`] (colours, gradients, patterns, image sampling), [`composite`] (the Porter-Duff
//! operators and blend modes), [`filter`] (the `filter` attribute's CSS filter functions), [`text`] (the `font` shorthand, glyph outlines as paths, TextMetrics) and
//! [`context`] (the bitmap, the state stack and the drawing model). The WebIDL face is `js/canvas.rs`.
//!
//! Bitmaps live here, per page thread, keyed by a surface id: an HTMLCanvasElement's context binds its node
//! to a surface (the renderer composes it as the element's replaced content), an OffscreenCanvas holds its
//! surface id in its platform object. The store is cleared when a page's engine starts.
//!
//! WEBGL1 (LEDGER SR136): the GL-backed surface variant. A `webgl` context is a `gl_core::Context` held under
//! the SAME surface id as a 2D bitmap of the canvas's size; every read of the surface ([`with`]: toDataURL,
//! drawImage, texImage2D from the canvas; [`element_bitmap`]: the compositor) first copies a changed drawing
//! buffer in (top-down, premultiplied), the compositor's copy marks it presented (preserveDrawingBuffer:false
//! clears it before the next GL operation), and a bitmap resize (the width/height attribute steps) resizes the
//! drawing buffer.

pub mod composite;
pub mod context;
pub mod filter;
pub mod geom;
pub mod paint;
pub mod raster;
pub mod text;

pub use context::Canvas2D;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

thread_local! {
    static SURFACES: RefCell<HashMap<u32, Canvas2D>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
    /// canvas element node id → surface id (set when its 2D context is created).
    static ELEMENTS: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    /// WEBGL1: surface id → its GL context (the GL-backed variant of a surface).
    static GL: RefCell<HashMap<u32, GlSurface>> = RefCell::new(HashMap::new());
}

/// A GL context and the drawing-buffer generations last copied into / presented from its surface.
pub struct GlSurface {
    pub gl: gl_core::Context,
    synced: u64,
    presented: u64,
}

/// Forget every surface (a new page).
pub fn reset() {
    SURFACES.with(|s| s.borrow_mut().clear());
    ELEMENTS.with(|e| e.borrow_mut().clear());
    GL.with(|g| g.borrow_mut().clear());
}

/// WEBGL1: make surface `id` GL-backed.
pub fn attach_gl(id: u32, gl: gl_core::Context) {
    GL.with(|g| g.borrow_mut().insert(id, GlSurface { gl, synced: 0, presented: 0 }));
}

pub fn is_gl(id: u32) -> bool {
    GL.with(|g| g.borrow().contains_key(&id))
}

/// Run `f` on a surface's GL context. Never call script from inside `f` (the store is borrowed).
pub fn with_gl<R>(id: u32, f: impl FnOnce(&mut gl_core::Context) -> R) -> Option<R> {
    GL.with(|g| g.borrow_mut().get_mut(&id).map(|s| f(&mut s.gl)))
}

/// Copy a GL surface's drawing buffer into its bitmap when it changed since the last copy (`present`: the
/// compositor's read, which marks the frame presented).
fn sync_gl(id: u32, present: bool) {
    let frame = GL.with(|g| {
        let mut g = g.borrow_mut();
        let s = g.get_mut(&id)?;
        let g_now = s.gl.db.generation;
        if s.synced == g_now && (!present || s.presented == g_now) {
            return None;
        }
        let f = s.gl.present_premultiplied(present && s.presented != g_now);
        s.synced = g_now;
        if present {
            s.presented = g_now;
        }
        Some(f)
    });
    let Some((w, h, px)) = frame else { return };
    SURFACES.with(|s| {
        if let Some(c) = s.borrow_mut().get_mut(&id) {
            // The drawing buffer may be smaller than an oversized canvas: copy the overlap.
            let (cw, ch) = (c.w.min(w as usize), c.h.min(h as usize));
            for y in 0..ch {
                let d = y * c.w * 4;
                let s = y * w as usize * 4;
                c.px[d..d + cw * 4].copy_from_slice(&px[s..s + cw * 4]);
            }
            c.generation = c.generation.wrapping_add(1);
        }
    });
}

/// After a bitmap resize, give the GL drawing buffer the new size.
fn resize_gl(id: u32) {
    let Some((w, h)) = SURFACES.with(|s| s.borrow().get(&id).map(|c| (c.w as u32, c.h as u32))) else { return };
    GL.with(|g| {
        if let Some(s) = g.borrow_mut().get_mut(&id) {
            let want = gl_core::context::drawing_buffer_size(w, h);
            if (s.gl.db.w, s.gl.db.h) != want {
                s.gl.resize(w, h);
            }
        }
    });
}

/// A new surface with a `w × h` bitmap.
pub fn create(w: usize, h: usize, opaque: bool) -> u32 {
    let id = NEXT.with(|n| {
        let v = n.get();
        n.set(v.wrapping_add(1).max(1));
        v
    });
    SURFACES.with(|s| s.borrow_mut().insert(id, Canvas2D::new(w, h, opaque)));
    id
}

/// Run `f` on a surface. Never call script from inside `f` (the store is borrowed).
pub fn with<R>(id: u32, f: impl FnOnce(&mut Canvas2D) -> R) -> Option<R> {
    let gl = is_gl(id);
    if gl {
        sync_gl(id, false);
    }
    let r = SURFACES.with(|s| s.borrow_mut().get_mut(&id).map(f));
    if gl {
        resize_gl(id);
    }
    r
}

pub fn bind_element(node: usize, surface: u32) {
    ELEMENTS.with(|e| e.borrow_mut().insert(node, surface));
}

pub fn element_surface(node: usize) -> Option<u32> {
    ELEMENTS.with(|e| e.borrow().get(&node).copied())
}

/// The renderer's question for a `<canvas>` node of the page: its bitmap as (width, height, straight RGBA),
/// when a 2D context has drawn into it. A canvas without a context paints nothing (transparent black).
pub fn element_bitmap(node: &crate::dom::NodeRef) -> Option<(usize, usize, std::rc::Rc<Vec<u8>>)> {
    let ours = crate::js::page(|p| p.doc.as_ref().is_some_and(|d| d.same_document(node)));
    if !ours {
        return None;
    }
    let id = element_surface(node.id().0)?;
    if is_gl(id) {
        sync_gl(id, true);
    }
    with(id, |c| {
        if c.w == 0 || c.h == 0 {
            return None;
        }
        let b = c.snapshot();
        Some((b.w, b.h, b.rgba))
    })
    .flatten()
}
