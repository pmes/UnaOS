//! GLSL ES 1.00 (the OpenGL ES Shading Language 1.00.17): [`pp`] preprocessor and lexer, [`parse`] parser
//! to the [`ast`], [`sema`] semantic checks, [`builtins`] the §8 signatures.
//!
//! [`compile`] turns one shader's source into a [`Compiled`] unit or an info log in the
//! `ERROR: 0:<line>: <message>` shape every WebGL implementation prints.

pub mod ast;
pub mod builtins;
pub mod parse;
pub mod pp;
pub mod sema;

use alloc::format;
use alloc::string::String;

/// A compiled shader: the checked translation unit (the program link consumes it).
pub struct Compiled {
    pub fragment: bool,
    pub unit: sema::Checked,
}

/// Compile `src` as a vertex or fragment shader. `derivatives` = the context enabled
/// OES_standard_derivatives (the `#extension` may then turn it on). Returns the unit (None on error) and the
/// info log.
pub fn compile(src: &str, fragment: bool, derivatives: bool) -> (Option<Compiled>, String) {
    let mut log = String::new();
    let pre = pp::preprocess(src, fragment, derivatives);
    for (l, m) in &pre.warnings {
        log.push_str(&format!("WARNING: 0:{l}: {m}\n"));
    }
    if !pre.errors.is_empty() {
        for (l, m) in &pre.errors {
            log.push_str(&format!("ERROR: 0:{l}: {m}\n"));
        }
        return (None, log);
    }
    let unit = match parse::parse(&pre.tokens) {
        Ok(u) => u,
        Err((l, m)) => {
            log.push_str(&format!("ERROR: 0:{l}: {m}\n"));
            return (None, log);
        }
    };
    match sema::check(unit, fragment, pre.ext.standard_derivatives) {
        Ok(c) => (Some(Compiled { fragment, unit: c }), log),
        Err(errs) => {
            for (l, m) in &errs {
                log.push_str(&format!("ERROR: 0:{l}: {m}\n"));
            }
            (None, log)
        }
    }
}
