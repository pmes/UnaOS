// SPDX-License-Identifier: LGPL-3.0-or-later
// SYNTAXCORE (SR89): known-answer tests — one fixture + expected span list per grammar (tests/kat/<lang>.txt and
// tests/kat/<lang>.spans, `<line> <kind> |<lexeme>|` per token, `#` lines are comments), then the engine:
// incremental relex bounds, incremental == fresh under a seeded edit stream for every grammar, folds, brackets,
// the theme map.

use syntax_core::{
    language_for_id, lex_all, Fold, FoldKind, HighlightKind, Highlighter, Language, Source, StrSource, Theme,
    APPEARANCE_TOKENS,
};

fn kat_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("kat")
}

fn lines_of(text: &str) -> Vec<String> {
    let src = StrSource::new(text);
    (0..src.line_count())
        .map(|l| {
            let mut s = String::new();
            src.read_line(l, &mut s);
            s
        })
        .collect()
}

fn render(lang: &dyn Language, text: &str) -> Vec<String> {
    let lines = lines_of(text);
    let mut got = Vec::new();
    for (li, toks) in lex_all(lang, text).iter().enumerate() {
        for t in toks {
            got.push(format!("{} {} |{}|", li, t.kind.name(), &lines[li][t.start as usize..t.end as usize]));
        }
    }
    got
}

fn kat(id: &str, name: &str) {
    let text = std::fs::read_to_string(kat_dir().join(format!("{name}.txt"))).unwrap();
    let spans = std::fs::read_to_string(kat_dir().join(format!("{name}.spans"))).unwrap();
    let want: Vec<String> = spans.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
    let lang = language_for_id(id).unwrap_or_else(|| panic!("no grammar for {id}"));
    let got = render(&*lang, &text);
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(g, w, "{name}: token {i} differs\n got: {got:#?}");
    }
    assert_eq!(got.len(), want.len(), "{name}: token count\n got: {got:#?}");
}

#[test]
fn kat_rust() {
    kat("rust", "rust");
}

#[test]
fn kat_c() {
    kat("c", "c");
}

#[test]
fn kat_toml() {
    kat("toml", "toml");
}

#[test]
fn kat_markdown() {
    kat("markdown", "markdown");
}

#[test]
fn kat_shell() {
    kat("sh", "shell");
}

// ---- the engine ---------------------------------------------------------------------------------------

fn fresh(lang: &dyn Language, text: &str) -> Highlighter {
    let mut h = Highlighter::new();
    h.reset(lang, &StrSource::new(text));
    h
}

/// Replace `a..b` of `text` with `ins`; returns the new text and the edit_core-shaped line triple.
fn apply(text: &str, a: usize, b: usize, ins: &str) -> (String, usize, usize, usize) {
    let first = text[..a].matches('\n').count();
    let old_last = text[..b].matches('\n').count();
    let mut t = String::with_capacity(text.len() + ins.len());
    t.push_str(&text[..a]);
    t.push_str(ins);
    t.push_str(&text[b..]);
    let new_last = t[..a + ins.len()].matches('\n').count();
    (t, first, old_last, new_last)
}

fn same(h: &Highlighter, f: &Highlighter, ctx: &str) {
    assert_eq!(h.line_count(), f.line_count(), "{ctx}: line count");
    for l in 0..f.line_count() {
        assert_eq!(h.line_tokens(l), f.line_tokens(l), "{ctx}: tokens of line {l}");
        assert_eq!(h.line_state(l), f.line_state(l), "{ctx}: checkpoint of line {l}");
    }
}

#[test]
fn one_character_edit_relexes_one_line() {
    let rust = language_for_id("rust").unwrap();
    let text = "fn a() {}\nlet x = 1;\nlet y = 2;\nlet z = 3;\n";
    let mut h = fresh(&*rust, text);
    let before = h.lex_count();
    let at = text.find("y =").unwrap();
    let (t, f, o, n) = apply(text, at, at + 1, "w");
    h.splice(f, o, n);
    let r = h.relex(&*rust, &StrSource::new(&t));
    assert_eq!(r, 2..3, "only the edited line");
    assert_eq!(h.lex_count() - before, 1, "one line lexed");
    same(&h, &fresh(&*rust, &t), "after the one-character edit");
}

#[test]
fn opening_a_block_comment_relexes_as_far_as_it_reaches() {
    let rust = language_for_id("rust").unwrap();
    let text = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n";
    let mut h = fresh(&*rust, text);
    let at = text.find("let b").unwrap();
    let (t, f, o, n) = apply(text, at, at, "/* ");
    h.splice(f, o, n);
    assert_eq!(h.relex(&*rust, &StrSource::new(&t)), 1..5, "every following line is now comment");
    same(&h, &fresh(&*rust, &t), "after opening");
    let at = t.find("let c = 3;").unwrap() + "let c = 3;".len();
    let (t2, f, o, n) = apply(&t, at, at, " */");
    h.splice(f, o, n);
    assert_eq!(h.relex(&*rust, &StrSource::new(&t2)), 2..5, "closing re-lexes from the closing line down");
    same(&h, &fresh(&*rust, &t2), "after closing");
    assert_eq!(h.line_tokens(3)[0].kind, HighlightKind::Keyword);
}

#[test]
fn multi_line_splice_matches_a_fresh_lex() {
    let toml = language_for_id("toml").unwrap();
    let text = "a = 1\nb = [\n  2,\n]\nc = 3\n";
    let mut h = fresh(&*toml, text);
    let at = text.find("  2,").unwrap();
    let (t, f, o, n) = apply(text, at, at, "  \"x\",\n  '''y\nz''',\n");
    h.splice(f, o, n);
    h.relex(&*toml, &StrSource::new(&t));
    same(&h, &fresh(&*toml, &t), "toml multi-line insert");
}

/// A seeded edit stream: after every edit the incremental tables equal a fresh lex, for every grammar.
#[test]
fn incremental_equals_fresh_under_a_seeded_edit_stream() {
    let corpus: [(&str, &str); 5] = [
        ("rust", include_str!("kat/rust.txt")),
        ("c", include_str!("kat/c.txt")),
        ("toml", include_str!("kat/toml.txt")),
        ("markdown", include_str!("kat/markdown.txt")),
        ("sh", include_str!("kat/shell.txt")),
    ];
    let pool = [
        "/*", "*/", "\"", "'", "\n", "x", "{", "}", "`", "```\n", "$(", ")", "<<EOF\n", "EOF\n", "\\", "#", "\"\"\"",
        "'''", "[", "]", "*", "_", "<!--", "-->", " ", "r#\"", "\"#",
    ];
    let mut seed: u64 = 0x5EED_5A1A_C0DE_0089;
    let mut rnd = |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n.max(1)
    };
    for (id, base) in corpus {
        let lang = language_for_id(id).unwrap();
        let mut text = base.to_string();
        let mut h = fresh(&*lang, &text);
        for step in 0..400 {
            // ASCII corpus and pool: every byte offset is a char boundary.
            let a = rnd(text.len() + 1);
            let b = if rnd(3) == 0 { (a + rnd(6)).min(text.len()) } else { a };
            let ins = if rnd(4) == 0 { "" } else { pool[rnd(pool.len())] };
            let (t, f, o, n) = apply(&text, a, b, ins);
            h.splice(f, o, n);
            h.relex(&*lang, &StrSource::new(&t));
            text = t;
            same(&h, &fresh(&*lang, &text), &format!("{id} step {step}"));
        }
    }
}

#[test]
fn folds_and_brackets() {
    let rust = language_for_id("rust").unwrap();
    let text = include_str!("kat/rust.txt");
    let h = fresh(&*rust, text);
    assert_eq!(
        h.folds(),
        vec![
            Fold { start_line: 4, end_line: 10, kind: FoldKind::Bracket },
            Fold { start_line: 11, end_line: 12, kind: FoldKind::Block },
        ]
    );
    assert_eq!(h.matching_bracket(4, 10), Some((10, 0)), "fn main's brace");
    assert_eq!(h.matching_bracket(10, 0), Some((4, 10)), "and back");
    assert_eq!(h.matching_bracket(4, 3), None, "not a bracket");

    let md = language_for_id("markdown").unwrap();
    let h = fresh(&*md, include_str!("kat/markdown.txt"));
    assert_eq!(
        h.folds(),
        vec![
            Fold { start_line: 0, end_line: 17, kind: FoldKind::Heading },
            Fold { start_line: 4, end_line: 14, kind: FoldKind::Heading },
            Fold { start_line: 8, end_line: 10, kind: FoldKind::Block },
            Fold { start_line: 12, end_line: 13, kind: FoldKind::Block },
            Fold { start_line: 15, end_line: 17, kind: FoldKind::Heading },
        ]
    );

    let sh = language_for_id("sh").unwrap();
    let h = fresh(&*sh, include_str!("kat/shell.txt"));
    assert_eq!(
        h.folds(),
        vec![
            Fold { start_line: 5, end_line: 7, kind: FoldKind::Block },
            Fold { start_line: 8, end_line: 10, kind: FoldKind::Block },
        ]
    );

    let c = language_for_id("c").unwrap();
    let h = fresh(&*c, include_str!("kat/c.txt"));
    assert_eq!(
        h.folds(),
        vec![
            Fold { start_line: 1, end_line: 2, kind: FoldKind::Block },
            Fold { start_line: 3, end_line: 4, kind: FoldKind::Block },
            Fold { start_line: 6, end_line: 11, kind: FoldKind::Bracket },
            Fold { start_line: 7, end_line: 8, kind: FoldKind::Block },
        ]
    );

    let toml = language_for_id("toml").unwrap();
    let h = fresh(&*toml, include_str!("kat/toml.txt"));
    assert_eq!(
        h.folds(),
        vec![
            Fold { start_line: 6, end_line: 7, kind: FoldKind::Bracket },
            Fold { start_line: 10, end_line: 12, kind: FoldKind::Block },
        ]
    );
}

#[test]
fn highlights_are_absolute_and_crlf_is_a_terminator() {
    let rust = language_for_id("rust").unwrap();
    let text = "let a;\r\nfn b() {}\r\n";
    let src = StrSource::new(text);
    let mut h = Highlighter::new();
    h.reset(&*rust, &src);
    let spans = h.highlights(&src, 1..2);
    assert_eq!(&text[spans[0].range.clone()], "fn");
    assert_eq!(spans[0].kind, HighlightKind::Keyword);
    assert_eq!(&text[spans[1].range.clone()], "b");
    assert_eq!(spans[1].kind, HighlightKind::Function);
    assert!(h.line_tokens(0).iter().all(|t| (t.end as usize) <= "let a;".len()), "the CR is not lexed");
}

#[test]
fn grammar_change_lexes_afresh() {
    let rust = language_for_id("rust").unwrap();
    let sh = language_for_id("sh").unwrap();
    let text = "echo # hi\n";
    let mut h = fresh(&*rust, text);
    assert_eq!(h.language(), "rust");
    assert_eq!(h.relex(&*sh, &StrSource::new(text)), 0..2);
    assert_eq!(h.language(), "sh");
    assert_eq!(h.line_tokens(0)[1].kind, HighlightKind::Comment);
}

#[test]
fn language_ids() {
    for (id, want) in [("rust", "rust"), ("c", "c"), ("toml", "toml"), ("markdown", "markdown"), ("sh", "sh"), ("bash", "sh")] {
        assert_eq!(language_for_id(id).map(|l| l.id()), Some(want), "{id}");
    }
    assert!(language_for_id("txt").is_none());
    assert!(language_for_id("python").is_none());
}

#[test]
fn theme_map_names_tokens_and_takes_preferences() {
    let t = Theme::default();
    for k in HighlightKind::ALL {
        assert!(APPEARANCE_TOKENS.contains(&t.token(k)), "{} -> {}", k.name(), t.token(k));
        assert_eq!(HighlightKind::from_name(k.name()), Some(k));
    }
    assert_eq!(t.token(HighlightKind::Error), "ErrorText");
    assert_eq!(Theme::pref_key(HighlightKind::DocComment), "theme.doc-comment");
    let t = Theme::from_prefs(&|k: &str| match k {
        "theme.keyword" => Some("SyntaxNum".to_string()),
        "theme.string" => Some("NotAToken".to_string()),
        _ => None,
    });
    assert_eq!(t.token(HighlightKind::Keyword), "SyntaxNum");
    assert_eq!(t.token(HighlightKind::String), "SyntaxLit", "an unknown token name is ignored");
}
