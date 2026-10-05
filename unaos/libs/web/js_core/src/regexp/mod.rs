//! RegExp (ECMA-262 §22.2): pattern parser and backtracking matcher.

pub mod parser;

use alloc::string::String;

/// Early-error check for a RegularExpressionLiteral (§13.2.7.2).
pub fn validate(body: &[u16], flags: &[u16]) -> Result<(), String> {
    let f = parser::Flags::parse(flags).map_err(String::from)?;
    parser::parse(body, f).map(|_| ())
}
