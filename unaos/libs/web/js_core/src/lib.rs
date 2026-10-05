//! JSCORE (LEDGER SR55): an ECMAScript 2025 engine written from ECMA-262 — `no_std` + `alloc`, no dependencies.
//!
//! Front end: [`lexer`] → [`parser`] (ESTree-shaped [`ast`], every early error) → compiler → bytecode VM with an
//! exact tracing garbage collector and the built-in library. Unicode data is generated from UCD 17.0.0.

#![no_std]
#![forbid(unsafe_code)]
#![allow(clippy::new_without_default, clippy::too_many_arguments, clippy::type_complexity)]

extern crate alloc;

pub mod ast;
pub mod bignum;
pub mod lexer;
pub mod numconv;
pub mod parser;
pub mod regexp;
pub mod string;
pub mod unicode;
