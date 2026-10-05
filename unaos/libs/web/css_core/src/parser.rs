//! CSS Syntax Module Level 3, §5: component values, simple blocks, functions, at-rules, qualified rules,
//! declarations (with `!important`) and the error-recovery rules. Everything above the component-value
//! level works over `&[ComponentValue]`, as the specification's "list of tokens or component values".

use alloc::string::String;
use alloc::vec::Vec;

use crate::tokenizer::{Token, Tokenizer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// `( … )`
    Paren,
    /// `[ … ]`
    Bracket,
    /// `{ … }`
    Brace,
}

/// §5.3 component value: a preserved token, a function, or a simple block.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentValue {
    Token(Token),
    Function(String, Vec<ComponentValue>),
    Block(BlockKind, Vec<ComponentValue>),
}

pub type CV = ComponentValue;

impl ComponentValue {
    pub fn is_whitespace(&self) -> bool {
        matches!(self, CV::Token(Token::Whitespace))
    }
    pub fn token(&self) -> Option<&Token> {
        match self {
            CV::Token(t) => Some(t),
            _ => None,
        }
    }
    pub fn ident(&self) -> Option<&str> {
        match self {
            CV::Token(Token::Ident(s)) => Some(s),
            _ => None,
        }
    }
    pub fn is_delim(&self, c: char) -> bool {
        matches!(self, CV::Token(Token::Delim(d)) if *d == c)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Vec<ComponentValue>,
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedRule {
    pub prelude: Vec<ComponentValue>,
    pub block: Vec<ComponentValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AtRule {
    pub name: String,
    pub prelude: Vec<ComponentValue>,
    pub block: Option<Vec<ComponentValue>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Qualified(QualifiedRule),
    At(AtRule),
}

/// An item of a block's contents (§5.4.4): declarations and nested rules interleave (CSS Nesting).
#[derive(Clone, Debug, PartialEq)]
pub enum BlockItem {
    Declaration(Declaration),
    Rule(Rule),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The construct was malformed and dropped.
    Invalid,
    /// "parse a …" on input with nothing but whitespace.
    Empty,
    /// "parse a …" found more after the one construct.
    ExtraInput,
}

// ------------------------------------------------------------------------------------------------
// §5.4.7–5.4.9 component values from tokens
// ------------------------------------------------------------------------------------------------

fn consume_cv(toks: &[Token], i: &mut usize) -> CV {
    let t = toks[*i].clone();
    *i += 1;
    match t {
        Token::LeftBrace => CV::Block(BlockKind::Brace, consume_until(toks, i, Token::RightBrace)),
        Token::LeftBracket => CV::Block(BlockKind::Bracket, consume_until(toks, i, Token::RightBracket)),
        Token::LeftParen => CV::Block(BlockKind::Paren, consume_until(toks, i, Token::RightParen)),
        Token::Function(name) => CV::Function(name, consume_until(toks, i, Token::RightParen)),
        t => CV::Token(t),
    }
}

fn consume_until(toks: &[Token], i: &mut usize, end: Token) -> Vec<CV> {
    let mut out = Vec::new();
    while *i < toks.len() {
        if toks[*i] == end {
            *i += 1;
            return out;
        }
        out.push(consume_cv(toks, i));
    }
    out
}

/// "Parse a list of component values" (§5.3.10).
pub fn parse_component_values(input: &str) -> Vec<CV> {
    let mut tz = Tokenizer::new(input);
    let mut toks = Vec::new();
    while let Some((t, _, _)) = tz.next_token() {
        toks.push(t);
    }
    let mut i = 0;
    let mut out = Vec::new();
    while i < toks.len() {
        out.push(consume_cv(&toks, &mut i));
    }
    out
}

/// "Parse a component value" (§5.3.9).
pub fn parse_one_component_value(input: &str) -> Result<CV, ParseError> {
    let cvs = parse_component_values(input);
    let mut it = cvs.into_iter().filter(|c| !c.is_whitespace());
    let first = it.next().ok_or(ParseError::Empty)?;
    if it.next().is_some() {
        return Err(ParseError::ExtraInput);
    }
    Ok(first)
}

// ------------------------------------------------------------------------------------------------
// rules
// ------------------------------------------------------------------------------------------------

fn is_semicolon(c: &CV) -> bool {
    matches!(c, CV::Token(Token::Semicolon))
}

/// §5.4.2 consume an at-rule (the at-keyword already consumed). `nested`: inside a block's contents.
fn consume_at_rule(name: String, cvs: &[CV], i: &mut usize) -> AtRule {
    let mut prelude = Vec::new();
    while *i < cvs.len() {
        match &cvs[*i] {
            CV::Token(Token::Semicolon) => {
                *i += 1;
                return AtRule { name, prelude, block: None };
            }
            CV::Block(BlockKind::Brace, b) => {
                *i += 1;
                return AtRule { name, prelude, block: Some(b.clone()) };
            }
            c => {
                prelude.push(c.clone());
                *i += 1;
            }
        }
    }
    AtRule { name, prelude, block: None }
}

/// §5.4.3 consume a qualified rule. `None` (a parse error) when no `{}` block ends the prelude; when
/// `nested`, a `;` also ends it (and is consumed).
fn consume_qualified_rule(cvs: &[CV], i: &mut usize, nested: bool) -> Option<QualifiedRule> {
    let mut prelude = Vec::new();
    while *i < cvs.len() {
        match &cvs[*i] {
            CV::Token(Token::Semicolon) if nested => {
                *i += 1;
                return None;
            }
            CV::Block(BlockKind::Brace, b) => {
                *i += 1;
                return Some(QualifiedRule { prelude, block: b.clone() });
            }
            c => {
                prelude.push(c.clone());
                *i += 1;
            }
        }
    }
    None
}

/// §5.4.1 consume a list of rules. `top_level` (a stylesheet): CDO / CDC between rules are dropped.
pub fn parse_rule_list_cvs(cvs: &[CV], top_level: bool) -> Vec<Result<Rule, ParseError>> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < cvs.len()
            && (cvs[i].is_whitespace() || (top_level && matches!(cvs[i], CV::Token(Token::Cdo) | CV::Token(Token::Cdc))))
        {
            i += 1;
        }
        if i >= cvs.len() {
            return out;
        }
        if let CV::Token(Token::AtKeyword(name)) = &cvs[i] {
            i += 1;
            out.push(Ok(Rule::At(consume_at_rule(name.clone(), cvs, &mut i))));
        } else {
            match consume_qualified_rule(cvs, &mut i, false) {
                Some(r) => out.push(Ok(Rule::Qualified(r))),
                None => out.push(Err(ParseError::Invalid)),
            }
        }
    }
}

/// "Parse a stylesheet" (§5.3.3), errors kept in place.
pub fn parse_stylesheet_rules(input: &str) -> Vec<Result<Rule, ParseError>> {
    parse_rule_list_cvs(&parse_component_values(input), true)
}

/// "Parse a list of rules" (§5.3.4).
pub fn parse_rule_list(input: &str) -> Vec<Result<Rule, ParseError>> {
    parse_rule_list_cvs(&parse_component_values(input), false)
}

/// "Parse a rule" (§5.3.5).
pub fn parse_one_rule(input: &str) -> Result<Rule, ParseError> {
    let cvs = parse_component_values(input);
    let mut i = 0;
    while i < cvs.len() && cvs[i].is_whitespace() {
        i += 1;
    }
    if i >= cvs.len() {
        return Err(ParseError::Empty);
    }
    let r = if let CV::Token(Token::AtKeyword(name)) = &cvs[i] {
        i += 1;
        Rule::At(consume_at_rule(name.clone(), &cvs, &mut i))
    } else {
        Rule::Qualified(consume_qualified_rule(&cvs, &mut i, false).ok_or(ParseError::Invalid)?)
    };
    while i < cvs.len() && cvs[i].is_whitespace() {
        i += 1;
    }
    if i < cvs.len() {
        return Err(ParseError::ExtraInput);
    }
    Ok(r)
}

// ------------------------------------------------------------------------------------------------
// declarations
// ------------------------------------------------------------------------------------------------

/// §5.4.6 consume a declaration over exactly the given component values (already cut at `;`).
pub fn parse_declaration_cvs(cvs: &[CV]) -> Result<Declaration, ParseError> {
    let mut i = 0;
    while i < cvs.len() && cvs[i].is_whitespace() {
        i += 1;
    }
    let name = match cvs.get(i) {
        Some(CV::Token(Token::Ident(n))) => n.clone(),
        _ => return Err(ParseError::Invalid),
    };
    i += 1;
    while i < cvs.len() && cvs[i].is_whitespace() {
        i += 1;
    }
    if !matches!(cvs.get(i), Some(CV::Token(Token::Colon))) {
        return Err(ParseError::Invalid);
    }
    i += 1;
    let mut value: Vec<CV> = cvs[i..].to_vec();
    let mut important = false;
    // The last two non-whitespace values `!` `important` (ASCII case-insensitive) set the flag.
    let mut j = value.len();
    while j > 0 && value[j - 1].is_whitespace() {
        j -= 1;
    }
    if j > 0 && matches!(value[j - 1].ident(), Some(s) if s.eq_ignore_ascii_case("important")) {
        let mut k = j - 1;
        while k > 0 && value[k - 1].is_whitespace() {
            k -= 1;
        }
        if k > 0 && value[k - 1].is_delim('!') {
            value.truncate(k - 1);
            important = true;
        }
    }
    Ok(Declaration { name, value, important })
}

/// "Parse a declaration" (§5.3.6).
pub fn parse_one_declaration(input: &str) -> Result<Declaration, ParseError> {
    let cvs = parse_component_values(input);
    if cvs.iter().all(|c| c.is_whitespace()) {
        return Err(ParseError::Empty);
    }
    parse_declaration_cvs(&cvs)
}

/// CSS Nesting's rule for a declaration inside a block's contents: a non-custom property whose value
/// holds a top-level `{}` block beside anything else is not a declaration (it is re-tried as a rule).
fn nesting_rejects(d: &Declaration) -> bool {
    if d.name.starts_with("--") {
        return false;
    }
    let has_block = d.value.iter().any(|c| matches!(c, CV::Block(BlockKind::Brace, _)));
    has_block && d.value.iter().filter(|c| !c.is_whitespace()).count() > 1
}

/// §5.4.4 consume a block's contents: declarations, at-rules and (nested) qualified rules.
pub fn parse_block_contents(cvs: &[CV]) -> Vec<Result<BlockItem, ParseError>> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < cvs.len() && (cvs[i].is_whitespace() || is_semicolon(&cvs[i])) {
            i += 1;
        }
        if i >= cvs.len() {
            return out;
        }
        match &cvs[i] {
            CV::Token(Token::AtKeyword(name)) => {
                i += 1;
                out.push(Ok(BlockItem::Rule(Rule::At(consume_at_rule(name.clone(), cvs, &mut i)))));
            }
            CV::Token(Token::Ident(_)) => {
                let start = i;
                let mut end = i;
                while end < cvs.len() && !is_semicolon(&cvs[end]) {
                    end += 1;
                }
                match parse_declaration_cvs(&cvs[start..end]) {
                    Ok(d) if !nesting_rejects(&d) => {
                        out.push(Ok(BlockItem::Declaration(d)));
                        i = (end + 1).min(cvs.len());
                    }
                    _ => {
                        i = start;
                        match consume_qualified_rule(cvs, &mut i, true) {
                            Some(r) => out.push(Ok(BlockItem::Rule(Rule::Qualified(r)))),
                            None => out.push(Err(ParseError::Invalid)),
                        }
                    }
                }
            }
            _ => match consume_qualified_rule(cvs, &mut i, true) {
                Some(r) => out.push(Ok(BlockItem::Rule(Rule::Qualified(r)))),
                None => out.push(Err(ParseError::Invalid)),
            },
        }
    }
}

/// "Parse a list of declarations" over a string (a `style` attribute): the block's contents.
pub fn parse_declaration_list(input: &str) -> Vec<Result<BlockItem, ParseError>> {
    parse_block_contents(&parse_component_values(input))
}

/// The declarations of a block's contents, errors and nested rules dropped.
pub fn declarations_of(cvs: &[CV]) -> Vec<Declaration> {
    parse_block_contents(cvs)
        .into_iter()
        .filter_map(|r| match r {
            Ok(BlockItem::Declaration(d)) => Some(d),
            _ => None,
        })
        .collect()
}

/// Strip leading and trailing whitespace component values.
pub fn trim_ws(v: &[CV]) -> &[CV] {
    let mut a = 0;
    let mut b = v.len();
    while a < b && v[a].is_whitespace() {
        a += 1;
    }
    while b > a && v[b - 1].is_whitespace() {
        b -= 1;
    }
    &v[a..b]
}
