//! M1 — the tokenizer (§13.2.5) against html5lib-tests `tokenizer/*.test` (fetched at test time, see vectors.txt).
//!
//! Suite `tok` (pin 9329e64, processing instructions OFF — what shipping Chromium does) and suite `tok-pi` (pin
//! c777c40, the 2026 spec's processing-instruction states ON). Output tokens are compared exactly (character
//! tokens coalesced, attributes as a map); parse-error codes/positions are not compared (not recorded).
//! Tests whose input or output holds a lone UTF-16 surrogate cannot be expressed as a Rust `&str` and are
//! counted separately as "unrepresentable".

mod common;

use common::{double_unescape, fetch, file_name, parse_json, vectors, Json};
use html_core::tokenizer::{State, Token, Tokenizer, TokenizerOpts};

fn state_of(name: &str) -> State {
    match name {
        "Data state" => State::Data,
        "PLAINTEXT state" => State::Plaintext,
        "RCDATA state" => State::Rcdata,
        "RAWTEXT state" => State::Rawtext,
        "Script data state" => State::ScriptData,
        "CDATA section state" => State::CdataSection,
        other => panic!("unknown initial state {other}"),
    }
}

/// Our tokens in the html5lib JSON-ish shape, as a comparable string.
fn render_ours(toks: &[Token]) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = String::new();
    for t in toks {
        if let Token::Character(c) = t {
            chars.push(*c);
            continue;
        }
        if !chars.is_empty() {
            out.push(format!("Character|{}", std::mem::take(&mut chars)));
        }
        match t {
            Token::Doctype(d) => out.push(format!(
                "DOCTYPE|{:?}|{:?}|{:?}|{}",
                d.name, d.public_id, d.system_id, !d.force_quirks
            )),
            Token::StartTag(tag) => {
                let mut a: Vec<String> = tag.attrs.iter().map(|a| format!("{}={}", a.name, a.value)).collect();
                a.sort();
                out.push(format!("StartTag|{}|{}|{}", tag.name, a.join("\u{1}"), tag.self_closing));
            }
            Token::EndTag(tag) => out.push(format!("EndTag|{}", tag.name)),
            Token::Comment(c) => out.push(format!("Comment|{c}")),
            Token::ProcessingInstruction { target, data } => out.push(format!("PI|{target}|{data}")),
            Token::Character(_) | Token::Eof => {}
        }
    }
    if !chars.is_empty() {
        out.push(format!("Character|{chars}"));
    }
    out
}

fn s_of(j: &Json, dbl: bool) -> Option<String> {
    let u = j.units()?;
    if dbl { String::from_utf16(&double_unescape(u)).ok() } else { String::from_utf16(u).ok() }
}

fn opt_s(j: &Json, dbl: bool) -> Option<Option<String>> {
    match j {
        Json::Null => Some(None),
        _ => s_of(j, dbl).map(Some),
    }
}

/// Expected tokens in the same shape; `None` if unrepresentable.
fn render_expected(out: &[Json], dbl: bool) -> Option<Vec<String>> {
    let mut v = Vec::new();
    let mut chars = String::new();
    for t in out {
        let a = t.arr();
        let kind = a[0].str()?;
        if kind == "Character" {
            chars.push_str(&s_of(&a[1], dbl)?);
            continue;
        }
        if !chars.is_empty() {
            v.push(format!("Character|{}", std::mem::take(&mut chars)));
        }
        match kind.as_str() {
            "DOCTYPE" => {
                let name = opt_s(&a[1], dbl)?;
                let p = opt_s(&a[2], dbl)?;
                let s = opt_s(&a[3], dbl)?;
                let ok = matches!(a[4], Json::Bool(true));
                v.push(format!("DOCTYPE|{name:?}|{p:?}|{s:?}|{ok}"));
            }
            "StartTag" => {
                let name = s_of(&a[1], dbl)?;
                let mut attrs = Vec::new();
                for k in a[2].keys() {
                    let val = s_of(a[2].get(k)?, dbl)?;
                    let k2 = if dbl { String::from_utf16(&double_unescape(&k.encode_utf16().collect::<Vec<_>>())).ok()? } else { k.clone() };
                    attrs.push(format!("{k2}={val}"));
                }
                attrs.sort();
                let sc = a.get(3).is_some_and(|x| matches!(x, Json::Bool(true)));
                v.push(format!("StartTag|{name}|{}|{sc}", attrs.join("\u{1}")));
            }
            "EndTag" => v.push(format!("EndTag|{}", s_of(&a[1], dbl)?)),
            "Comment" => v.push(format!("Comment|{}", s_of(&a[1], dbl)?)),
            "ProcessingInstruction" => {
                v.push(format!("PI|{}|{}", s_of(&a[1], dbl)?, s_of(&a[2], dbl)?))
            }
            other => panic!("unknown token kind {other}"),
        }
    }
    if !chars.is_empty() {
        v.push(format!("Character|{chars}"));
    }
    Some(v)
}

fn run_suite(suite: &str, pi: bool) -> Option<(usize, usize, usize)> {
    let files = vectors(suite);
    let (mut pass, mut total, mut unrep) = (0, 0, 0);
    for (url, sha) in &files {
        let Some(text) = fetch(url, sha) else { return None };
        let j = parse_json(&text);
        let tests = j.get("tests").or_else(|| j.get("xmlViolationTests")).map(|t| t.arr().to_vec()).unwrap_or_default();
        let fname = file_name(url);
        let is_xml_violation = j.get("xmlViolationTests").is_some();
        let (mut fp, mut ft, mut fu) = (0, 0, 0);
        let mut fails = Vec::new();
        for t in &tests {
            let dbl = matches!(t.get("doubleEscaped"), Some(Json::Bool(true)));
            let desc = t.get("description").and_then(|d| d.str()).unwrap_or_default();
            let states: Vec<String> = match t.get("initialStates") {
                Some(s) => s.arr().iter().filter_map(|x| x.str()).collect(),
                None => vec!["Data state".to_string()],
            };
            let last = t.get("lastStartTag").and_then(|x| x.str());
            let input = t.get("input").and_then(|x| s_of(x, dbl));
            let expected = render_expected(t.get("output").map(|o| o.arr()).unwrap_or(&[]), dbl);
            for st in &states {
                if is_xml_violation {
                    // These expect the §13.2.7 "coercing into an infoset" tweaks, which a browser parser does not
                    // apply; counted as unrepresentable, not failures.
                    fu += 1;
                    continue;
                }
                let (Some(input), Some(expected)) = (input.clone(), expected.clone()) else {
                    fu += 1;
                    continue;
                };
                ft += 1;
                let mut tz = Tokenizer::from_str(&input, TokenizerOpts { processing_instructions: pi });
                tz.state = state_of(st);
                tz.last_start_tag = last.clone();
                let mut toks = Vec::new();
                loop {
                    let tk = tz.next_token();
                    if tk == Token::Eof {
                        break;
                    }
                    toks.push(tk);
                }
                let ours = render_ours(&toks);
                if ours == expected {
                    fp += 1;
                } else if fails.len() < 5 {
                    fails.push(format!("    FAIL {desc:?} [{st}]\n      input {input:?}\n      want  {expected:?}\n      got   {ours:?}"));
                }
            }
        }
        println!("{suite:7} {fname:32} {fp:5}/{ft:<5}{}", if fu > 0 { format!(" ({fu} unrepresentable)") } else { String::new() });
        for f in fails {
            println!("{f}");
        }
        pass += fp;
        total += ft;
        unrep += fu;
    }
    println!("{suite:7} TOTAL {pass}/{total} ({unrep} unrepresentable)");
    Some((pass, total, unrep))
}

#[test]
fn html5lib_tokenizer_pi_off() {
    let Some((pass, total, _)) = run_suite("tok", false) else { return };
    assert_eq!(pass, total, "tokenizer vectors (PI off) must all pass");
}

#[test]
fn html5lib_tokenizer_pi_on() {
    let Some((pass, total, _)) = run_suite("tok-pi", true) else { return };
    assert_eq!(pass, total, "tokenizer vectors (PI on, 2026 spec) must all pass");
}

#[test]
fn named_reference_longest_match() {
    // §13.2.5.73 examples.
    let toks = |s: &str| {
        let mut tz = Tokenizer::from_str(s, TokenizerOpts::default());
        let mut out = String::new();
        loop {
            match tz.next_token() {
                Token::Character(c) => out.push(c),
                Token::Eof => break,
                _ => {}
            }
        }
        out
    };
    assert_eq!(toks("I'm &notit; I tell you"), "I'm \u{ac}it; I tell you");
    assert_eq!(toks("I'm &notin; I tell you"), "I'm \u{2209} I tell you");
    assert_eq!(toks("&amp&lt;&#x41;&#65;&#0;&#x80;"), "&<AA\u{FFFD}\u{20AC}");
}
