//! RegExp (ECMA-262 §22.2): pattern parser and backtracking matcher.

pub mod matcher;
pub mod parser;

use alloc::string::String;

/// Early-error check for a RegularExpressionLiteral (§13.2.7.2).
pub fn validate(body: &[u16], flags: &[u16]) -> Result<(), String> {
    let f = parser::Flags::parse(flags).map_err(String::from)?;
    parser::parse(body, f).map(|_| ())
}

use crate::string::JsStr;
use alloc::rc::Rc;

/// A parsed and compiled pattern, shared between RegExp objects created from the same literal.
pub struct Compiled {
    pub regex: parser::Regex,
    pub prog: matcher::Program,
}

impl Compiled {
    pub fn new(pattern: &[u16], flags: parser::Flags) -> Result<Compiled, String> {
        let regex = parser::parse(pattern, flags)?;
        let prog = matcher::compile(&regex);
        Ok(Compiled { regex, prog })
    }
}

/// [[OriginalSource]], [[OriginalFlags]] and [[RegExpMatcher]] of a RegExp object.
pub struct RegExpData {
    pub source: JsStr,
    pub flags: JsStr,
    pub parsed: parser::Flags,
    pub re: Rc<Compiled>,
}
