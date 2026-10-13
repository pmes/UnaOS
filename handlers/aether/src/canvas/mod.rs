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
}

/// Forget every surface (a new page).
pub fn reset() {
    SURFACES.with(|s| s.borrow_mut().clear());
    ELEMENTS.with(|e| e.borrow_mut().clear());
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
    SURFACES.with(|s| s.borrow_mut().get_mut(&id).map(f))
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
    with(id, |c| {
        if c.w == 0 || c.h == 0 {
            return None;
        }
        let b = c.snapshot();
        Some((b.w, b.h, b.rgba))
    })
    .flatten()
}
