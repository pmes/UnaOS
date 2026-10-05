//! M1 — CSS Syntax Level 3 proven on css-parsing-tests (servo/rust-cssparser's maintained copy, pinned in
//! vectors.txt, fetched at test time, skipped offline). Prints pass/total per file; every file must pass
//! every vector (the per-file conventions of the test harness are documented next to `run_file`).
mod common;
use common::json::{self, Json};

use css_core::parser::*;
use css_core::{anb, urange, Token};

fn s(x: &str) -> Json {
    Json::Str(x.to_string())
}
fn arr(v: Vec<Json>) -> Json {
    Json::Arr(v)
}

fn repr(n: &css_core::Num) -> String {
    let sign = if n.has_sign {
        if n.value.is_sign_negative() { "-" } else { "+" }
    } else {
        ""
    };
    format!("{sign}{}", n.value.abs())
}

fn num(kind: &str, n: &css_core::Num) -> Vec<Json> {
    vec![s(kind), s(&repr(n)), Json::Num(n.value), s(if n.is_integer { "integer" } else { "number" })]
}

fn tok(t: &Token) -> Json {
    match t {
        Token::Ident(v) => arr(vec![s("ident"), s(v)]),
        Token::Function(v) => arr(vec![s("function"), s(v)]),
        Token::AtKeyword(v) => arr(vec![s("at-keyword"), s(v)]),
        Token::Hash { value, is_id } => arr(vec![s("hash"), s(value), s(if *is_id { "id" } else { "unrestricted" })]),
        Token::String(v) => arr(vec![s("string"), s(v)]),
        Token::BadString => arr(vec![s("error"), s("bad-string")]),
        Token::Url(v) => arr(vec![s("url"), s(v)]),
        Token::BadUrl => arr(vec![s("error"), s("bad-url")]),
        Token::Delim(c) => Json::Str(c.to_string()),
        Token::Number(n) => arr(num("number", n)),
        Token::Percentage(n) => arr(num("percentage", n)),
        Token::Dimension(n, u) => {
            let mut v = num("dimension", n);
            v.push(s(u));
            arr(v)
        }
        Token::Whitespace => s(" "),
        Token::Cdo => s("<!--"),
        Token::Cdc => s("-->"),
        Token::Colon => s(":"),
        Token::Semicolon => s(";"),
        Token::Comma => s(","),
        Token::IncludeMatch => s("~="),
        Token::DashMatch => s("|="),
        Token::PrefixMatch => s("^="),
        Token::SuffixMatch => s("$="),
        Token::SubstringMatch => s("*="),
        Token::RightBracket => arr(vec![s("error"), s("]")]),
        Token::RightParen => arr(vec![s("error"), s(")")]),
        Token::RightBrace => arr(vec![s("error"), s("}")]),
        Token::LeftBracket => s("["),
        Token::LeftParen => s("("),
        Token::LeftBrace => s("{"),
    }
}

fn cv(c: &CV) -> Json {
    match c {
        CV::Token(t) => tok(t),
        CV::Function(name, args) => {
            let mut v = vec![s("function"), s(name)];
            v.extend(args.iter().map(cv));
            arr(v)
        }
        CV::Block(k, inner) => {
            let mut v = vec![s(match k {
                BlockKind::Paren => "()",
                BlockKind::Bracket => "[]",
                BlockKind::Brace => "{}",
            })];
            v.extend(inner.iter().map(cv));
            arr(v)
        }
    }
}

fn cvs(v: &[CV]) -> Json {
    arr(v.iter().map(cv).collect())
}

fn err(e: ParseError) -> Json {
    arr(vec![
        s("error"),
        s(match e {
            ParseError::Invalid => "invalid",
            ParseError::Empty => "empty",
            ParseError::ExtraInput => "extra-input",
        }),
    ])
}

fn rule(r: &Rule) -> Json {
    match r {
        Rule::Qualified(q) => arr(vec![s("qualified rule"), cvs(&q.prelude), cvs(&q.block)]),
        Rule::At(a) => arr(vec![
            s("at-rule"),
            s(&a.name),
            cvs(&a.prelude),
            a.block.as_ref().map(|b| cvs(b)).unwrap_or(Json::Null),
        ]),
    }
}

fn decl(d: &Declaration) -> Json {
    arr(vec![s("declaration"), s(&d.name), cvs(&d.value), Json::Bool(d.important)])
}

fn is_charset(r: &Rule) -> bool {
    matches!(r, Rule::At(a) if a.name.eq_ignore_ascii_case("charset"))
}

/// Equality with the numeric tolerance of an f32-based reference, and a number's representation string
/// compared as (value, explicit-sign) — the exact digits are rust-cssparser's own re-serialization.
fn same(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Num(x), Json::Num(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0),
        (Json::Arr(x), Json::Arr(y)) => {
            if x.len() != y.len() {
                return false;
            }
            let numeric = matches!(x.first(), Some(Json::Str(k)) if k == "number" || k == "percentage" || k == "dimension")
                && x.len() >= 4;
            x.iter().zip(y).enumerate().all(|(i, (p, q))| {
                if numeric && i == 1 {
                    let (p, q) = (p.str(), q.str());
                    let pv: f64 = p.parse().unwrap_or(f64::NAN);
                    let qv: f64 = q.parse().unwrap_or(f64::NAN);
                    (pv - qv).abs() <= 1e-6 * pv.abs().max(1.0) && p.starts_with('+') == q.starts_with('+') && p.starts_with('-') == q.starts_with('-')
                } else {
                    same(p, q)
                }
            })
        }
        _ => a == b,
    }
}

/// One file: `f(input) -> Json` per vector. Conventions of the reference harness reproduced here:
/// the reference's rule parser rejects `@charset` (an "invalid" error), except that a stylesheet's first
/// rule being `@charset` is skipped silently.
fn run_file(name: &str, f: &dyn Fn(&str) -> Json) -> (usize, usize) {
    let text = match common::fetch(name) {
        Ok(t) => t,
        Err(e) => {
            println!("{name}: SKIPPED ({e})");
            return (0, 0);
        }
    };
    let v = json::parse(&text);
    let items = v.arr();
    let mut pass = 0;
    let mut total = 0;
    for pair in items.chunks(2) {
        total += 1;
        let got = f(pair[0].str());
        if same(&got, &pair[1]) {
            pass += 1;
        } else {
            println!("  FAIL {name} input {:?}\n    want {:?}\n    got  {:?}", pair[0].str(), pair[1], got);
        }
    }
    println!("{name}: {pass}/{total}");
    (pass, total)
}

#[test]
fn m1_css_parsing_tests() {
    // The comparator is not vacuous.
    assert!(!same(&json::parse(r#"["ident","a"]"#), &json::parse(r#"["ident","b"]"#)));
    assert!(!same(&json::parse(r#"["number","+1",1,"integer"]"#), &json::parse(r#"["number","1",1,"integer"]"#)));
    let mut results = Vec::new();
    results.push(run_file("component_value_list.json", &|i| cvs(&parse_component_values(i))));
    results.push(run_file("one_component_value.json", &|i| match parse_one_component_value(i) {
        Ok(c) => cv(&c),
        Err(e) => err(e),
    }));
    results.push(run_file("declaration_list.json", &|i| {
        arr(parse_declaration_list(i)
            .iter()
            .map(|r| match r {
                Ok(BlockItem::Declaration(d)) => decl(d),
                Ok(BlockItem::Rule(r)) if is_charset(r) => err(ParseError::Invalid),
                Ok(BlockItem::Rule(r)) => rule(r),
                Err(e) => err(*e),
            })
            .collect())
    }));
    results.push(run_file("one_declaration.json", &|i| match parse_one_declaration(i) {
        Ok(d) => decl(&d),
        Err(e) => err(e),
    }));
    results.push(run_file("rule_list.json", &|i| {
        arr(parse_rule_list(i)
            .iter()
            .map(|r| match r {
                Ok(r) if is_charset(r) => err(ParseError::Invalid),
                Ok(r) => rule(r),
                Err(e) => err(*e),
            })
            .collect())
    }));
    results.push(run_file("one_rule.json", &|i| match parse_one_rule(i) {
        Ok(r) => rule(&r),
        Err(e) => err(e),
    }));
    results.push(run_file("stylesheet.json", &|i| {
        arr(parse_stylesheet_rules(i)
            .iter()
            .enumerate()
            .filter_map(|(k, r)| match r {
                Ok(r) if is_charset(r) && k == 0 => None,
                Ok(r) if is_charset(r) => Some(err(ParseError::Invalid)),
                Ok(r) => Some(rule(r)),
                Err(e) => Some(err(*e)),
            })
            .collect())
    }));
    results.push(run_file("An+B.json", &|i| match anb::parse_anb(&parse_component_values(i)) {
        Some((a, b)) => arr(vec![Json::Num(a as f64), Json::Num(b as f64)]),
        None => Json::Null,
    }));
    results.push(run_file("urange.json", &|i| {
        arr(urange::parse_urange_list(i)
            .iter()
            .map(|r| match r {
                Some((a, b)) => arr(vec![Json::Num(*a as f64), Json::Num(*b as f64)]),
                None => Json::Null,
            })
            .collect())
    }));
    let (p, t) = results.iter().fold((0, 0), |(a, b), (p, t)| (a + p, b + t));
    println!("css-parsing-tests total: {p}/{t}");
    assert_eq!(p, t, "css-parsing-tests: {p}/{t}");
}
