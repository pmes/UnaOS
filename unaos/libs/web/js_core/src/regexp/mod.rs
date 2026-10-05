//! RegExp (ECMA-262 §22.2): pattern parser and backtracking matcher.

pub mod parser;

use alloc::string::String;

/// Early-error check for a RegularExpressionLiteral (§13.2.7.2).
pub fn validate(body: &[u16], flags: &[u16]) -> Result<(), String> {
    let f = parser::Flags::parse(flags).map_err(String::from)?;
    parser::parse(body, f).map(|_| ())
}

use crate::string::JsStr;
use crate::vm::Value;
use alloc::rc::Rc;

/// [[RegExpMatcher]] state of a RegExp object.
pub struct RegExpData {
    pub source: JsStr,
    pub flags: JsStr,
    pub regex: Option<Rc<parser::Regex>>,
    pub last_index_cache: Value,
}
