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
pub mod tz;
pub mod unicode;
pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod vm;

/// x * 2^k with correct handling of overflow and subnormal results.
pub fn numconv_ldexp(x: f64, k: i32) -> f64 {
    let mut x = x;
    let mut k = k;
    while k > 1000 {
        x *= bignum::pow2(1000);
        k -= 1000;
    }
    while k < -1000 {
        x *= bignum::pow2(-1000);
        k += 1000;
    }
    if k < -1022 {
        // two steps to avoid double rounding into subnormals as far as possible
        x * bignum::pow2(k + 600) * bignum::pow2(-600)
    } else {
        x * bignum::pow2(k)
    }
}
