//! CSSCORE (LEDGER SR47): CSS written from the specifications for UnaOS's web stack.
//!
//! * [`tokenizer`] / [`parser`] — CSS Syntax Module Level 3 (§4 tokenization, §5 parsing, error recovery).
//! * [`anb`] / [`urange`] — the An+B (§6) and `<urange>` (§7.1) microsyntaxes.
//! * [`selectors`] — Selectors Level 4 grammar and specificity; [`matching`] — the matcher over the
//!   abstract [`matching::Element`] trait.
//! * [`stylesheet`] — rules as data (`@media` `@supports` `@layer` `@import` `@namespace` `@font-face`
//!   `@keyframes`, CSS Nesting); [`media`] — Media Queries 4; [`cascade`] — the cascade sort.
//! * [`values`] — lengths, `calc()` / `min()` / `max()` / `clamp()` trees, `var()` substitution;
//!   [`color`] — CSS Color 4 sRGB colors; [`serialize`] — CSSOM serialization.
//!
//! `no_std` + `alloc`, no dependencies, no `unsafe`.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod anb;
pub mod cascade;
pub mod color;
pub mod matching;
pub mod media;
pub mod parser;
pub mod selectors;
pub mod serialize;
pub mod stylesheet;
pub mod tokenizer;
pub mod urange;
pub mod values;

pub use parser::{AtRule, BlockItem, BlockKind, ComponentValue, Declaration, ParseError, QualifiedRule, Rule, CV};
pub use tokenizer::{Num, Token};
