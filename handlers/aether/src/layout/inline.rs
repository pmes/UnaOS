//! The inline formatting context (CSS 2.2 §9.4.2, css-inline-3 §2-4). LAYOUTCORE (SR76): the formatter is
//! `layout_core::inline` (unaos/libs/web/layout_core/src/inline.rs), shared with APPS/AETHER.ELF; the
//! fragment types and the painter's atomic test are re-exported here under their old paths.

pub use layout_core::inline::{is_atomic_pub, Frag, InlineLayout};
