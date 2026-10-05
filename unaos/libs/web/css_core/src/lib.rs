//! CSSCORE (LEDGER SR47): CSS written from the specifications for UnaOS's web stack.
//!
//! * [`tokenizer`] / [`parser`] — CSS Syntax Module Level 3 (§4 tokenization, §5 parsing, error recovery).
//! * [`anb`] / [`urange`] — the An+B (§6) and `<urange>` (§7.1) microsyntaxes.
//!
//! `no_std` + `alloc`, no dependencies, no `unsafe`.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod anb;
pub mod parser;
pub mod tokenizer;
pub mod urange;

pub use parser::{AtRule, BlockItem, BlockKind, ComponentValue, Declaration, ParseError, QualifiedRule, Rule, CV};
pub use tokenizer::{Num, Token};
