//! M3 — known-answer tests for the cascade helpers: Media Queries 4, the math functions, `var()`,
//! stylesheet data (`@import` `@font-face` `@keyframes` `@namespace` `@layer`, nesting), the cascade sort,
//! and colors (css-parsing-tests' color files, fetched at test time).
mod common;
use common::dom::Dom;
use common::json;

use css_core::cascade::{cascade, Conditions, Origin, RuleSet};
use css_core::color::{parse_color, Color};
use css_core::media::{parse_media_query_list, Environment};
use css_core::parser::{parse_component_values, Declaration};
use css_core::selectors::HTML_NS;
use css_core::serialize::to_css;
use css_core::stylesheet::*;
use css_core::values::*;

fn mq(s: &str, env: &Environment) -> bool {
    parse_media_query_list(&parse_component_values(s)).matches(env)
}

#[test]
fn m3_media_queries() {
    let d = Environment::default(); // 800x600 screen
    let n = Environment::viewport(375.0, 812.0);
    let cases: &[(&str, bool, bool)] = &[
        ("", true, true),
        ("all", true, true),
        ("screen", true, true),
        ("print", false, false),
        ("not print", true, true),
        ("only screen", true, true),
        ("tv", false, false),
        ("screen and (min-width: 600px)", true, false),
        ("(max-width: 600px)", false, true),
        ("(width >= 600px)", true, false),
        ("(600px <= width)", true, false),
        ("(400px <= width <= 1000px)", true, false),
        ("(300px < width < 400px)", false, true),
        ("(min-width: 30em)", true, false),
        ("(min-width: 50rem)", true, false),
        ("(width = 800px)", true, false),
        ("(width > 800px)", false, false),
        ("(orientation: landscape)", true, false),
        ("(orientation: portrait)", false, true),
        ("(min-aspect-ratio: 4/3)", true, false),
        ("(aspect-ratio: 4/3)", true, false),
        ("(aspect-ratio > 1)", true, false),
        ("(color)", true, true),
        ("(monochrome)", false, false),
        ("(color-index)", false, false),
        ("(min-color: 8)", true, true),
        ("(hover: hover) and (pointer: fine)", true, true),
        ("(any-pointer: coarse)", false, false),
        ("(prefers-color-scheme: dark)", false, false),
        ("(prefers-color-scheme: light)", true, true),
        ("(prefers-reduced-motion)", false, false),
        ("(resolution: 1dppx)", true, true),
        ("(min-resolution: 2x)", false, false),
        ("(min-resolution: 96dpi)", true, true),
        ("(scripting: enabled)", true, true),
        ("(width)", true, true),
        ("(grid)", false, false),
        ("(max-width: 600px), print", false, true),
        ("print, (orientation: portrait)", false, true),
        ("not all and (monochrome)", true, true),
        ("not screen and (color)", false, false),
        ("(min-width: 500px) and (max-width: 900px)", true, false),
        ("(min-width: 900px) or (orientation: portrait)", false, true),
        ("not (min-width: 600px)", false, true),
        ("((min-width: 600px) and (color))", true, false),
        // unknown / malformed
        ("(foo-bar: 1)", false, false),
        ("not (foo-bar: 1)", false, false),
        ("(unknown)", false, false),
        ("screen and (color) or (monochrome)", false, false),
        ("screen and", false, false),
        ("and", false, false),
        ("(min-width: 600)", false, false),
        ("(width >= 600px) and", false, false),
        ("unknown-type, screen", true, true),
        ("(example, all,), speech", false, false),
        ("all and (color), garbage {}", true, true),
    ];
    for (q, wd, wn) in cases {
        assert_eq!(mq(q, &d), *wd, "{q:?} at 800x600");
        assert_eq!(mq(q, &n), *wn, "{q:?} at 375x812");
    }
    println!("media queries: {} KATs x 2 viewports", cases.len());
}

fn calc(s: &str, cx: &LengthContext) -> Option<Resolved> {
    let cvs = parse_component_values(s);
    let c = parse_math(cvs.iter().find(|c| !c.is_whitespace())?)?;
    c.resolve(cx)
}

#[test]
fn m3_math_functions() {
    let cx = LengthContext { font_size: 20.0, root_font_size: 16.0, viewport_width: 800.0, viewport_height: 600.0, percent_basis: None };
    let px = |s: &str| calc(s, &cx).and_then(|r| r.px());
    let numv = |s: &str| match calc(s, &cx) {
        Some(Resolved::Number(n)) => Some(n),
        _ => None,
    };
    assert_eq!(px("calc(10px + 2em)"), Some(50.0));
    assert_eq!(px("calc(1rem * 2)"), Some(32.0));
    assert_eq!(px("calc((10px + 2vw) * 2 - 1rem)"), Some(36.0));
    assert_eq!(px("calc(100px / 4)"), Some(25.0));
    assert_eq!(px("calc(1in - 1cm)").map(|v| (v * 1000.0).round()), Some(((96.0 - 96.0 / 2.54) * 1000.0f64).round()));
    assert_eq!(px("min(10vw, 50px, 3em)"), Some(50.0));
    assert_eq!(px("max(1px, min(5px, 2vmin))"), Some(5.0));
    assert_eq!(px("clamp(12px, 1rem + 1vw, 40px)"), Some(24.0));
    assert_eq!(px("clamp(30px, 10px, 40px)"), Some(30.0));
    assert_eq!(px("clamp(none, 50px, 40px)"), Some(40.0));
    assert_eq!(px("calc(-1 * (3px - 5px))"), Some(2.0));
    assert_eq!(px("abs(calc(2px - 7px))"), Some(5.0));
    assert_eq!(numv("sign(-3px)"), Some(-1.0));
    assert_eq!(px("round(17px, 5px)"), Some(15.0));
    assert_eq!(px("round(up, 11px, 5px)"), Some(15.0));
    assert_eq!(px("round(down, 14px, 5px)"), Some(10.0));
    assert_eq!(px("round(to-zero, -14px, 5px)"), Some(-10.0));
    assert_eq!(px("mod(-18px, 5px)"), Some(2.0));
    assert_eq!(px("rem(-18px, 5px)"), Some(-3.0));
    assert_eq!(numv("calc(2 * pi)").map(|v| (v * 1e6).round()), Some((2.0 * std::f64::consts::PI * 1e6).round()));
    assert_eq!(numv("calc(1 / 4 + 0.75)"), Some(1.0));
    // percentages kept without a basis, resolved with one
    assert_eq!(calc("calc(100% - 40px)", &cx), Some(Resolved::LengthPercentage { px: -40.0, pct: 100.0, has_pct: true, has_len: true }));
    let with = LengthContext { percent_basis: Some(400.0), ..cx };
    assert_eq!(calc("calc(100% - 40px)", &with).and_then(|r| r.px()), Some(360.0));
    assert_eq!(calc("min(300px, 80%)", &with).and_then(|r| r.px()), Some(300.0));
    assert_eq!(calc("min(300px, 80%)", &cx), None, "needs the basis");
    // other dimensions
    assert_eq!(calc("calc(90deg + 0.5turn)", &cx), Some(Resolved::Other(Dim::Angle, 270.0)));
    assert_eq!(calc("calc(1s - 250ms)", &cx), Some(Resolved::Other(Dim::Time, 0.75)));
    // syntax and type errors
    for bad in ["calc(1px+2px)", "calc(1px -2px)", "calc(1px * 2px)", "calc(1px + 2)", "calc(2 / 1px)", "calc()", "calc(1px 2px)", "min()", "clamp(1px, 2px)", "calc(1foo)"] {
        assert_eq!(calc(bad, &cx), None, "{bad}");
    }
    println!("math functions: 40 KATs");
}

#[test]
fn m3_custom_properties() {
    use std::collections::BTreeMap;
    let v = |s: &str| parse_component_values(s);
    let spec = |pairs: &[(&str, &str)]| -> Vec<(String, Vec<css_core::CV>)> { pairs.iter().map(|(n, s)| (n.to_string(), v(s))).collect() };
    let root = compute_custom_properties(
        &spec(&[("--main", " rgb(1, 2, 3) "), ("--pad", "6px"), ("--twice", "calc(var(--pad) * 2)"), ("--a", "var(--b)"), ("--b", "var(--a)"), ("--c", "var(--a, 9px)"), ("--d", "var(--zzz)"), ("--e", "var(--zzz, var(--pad))"), ("--empty", "")]),
        &BTreeMap::new(),
    );
    assert_eq!(to_css(&root["--main"]), "rgb(1, 2, 3)");
    assert_eq!(to_css(&root["--twice"]), "calc(6px * 2)");
    assert!(!root.contains_key("--a") && !root.contains_key("--b"), "a cycle is guaranteed-invalid");
    assert_eq!(to_css(&root["--c"]), "9px", "a cycle member referenced with a fallback");
    assert!(!root.contains_key("--d"), "no value, no fallback");
    assert_eq!(to_css(&root["--e"]), "6px");
    assert_eq!(to_css(&root["--empty"]), "");
    // inheritance, inherit / initial
    let child = compute_custom_properties(&spec(&[("--pad", "10px"), ("--main", "initial"), ("--x", "inherit")]), &root);
    assert_eq!(to_css(&child["--pad"]), "10px");
    assert!(!child.contains_key("--main"));
    assert_eq!(to_css(&child["--twice"]), "calc(6px * 2)", "inherited as computed, not re-resolved");
    // substitution into an ordinary property
    let look = |n: &str| root.get(n).cloned();
    assert_eq!(to_css(&substitute_vars(&v("1px solid var(--main)"), &look).unwrap()), "1px solid rgb(1, 2, 3)");
    assert_eq!(to_css(&substitute_vars(&v("var(--nope, var(--pad))"), &look).unwrap()), "6px");
    assert_eq!(to_css(&substitute_vars(&v("var(--nope,)"), &look).unwrap()), "");
    assert!(substitute_vars(&v("var(--nope)"), &look).is_err());
    assert!(substitute_vars(&v("var(nope)"), &look).is_err(), "not a custom property name");
    println!("custom properties: 15 KATs");
}

#[test]
fn m3_stylesheet_data() {
    let s = parse_stylesheet(
        r#"@charset "utf-8";
        @import url("a.css") layer(base) supports(display: grid) screen and (min-width: 10px);
        @import 'b.css' print;
        @namespace svg url(http://www.w3.org/2000/svg);
        @import "late.css";
        @layer reset, base.inner;
        @font-face { font-family: "My Font"; src: local(Foo Bar), url(f.woff2) format("woff2"), url('g.ttf'); font-weight: 400 700; }
        @keyframes spin { from { transform: rotate(0) } 50%, 75% { opacity: .5 } to { transform: rotate(1turn); color: red !important } }
        @keyframes "quoted" { 0% {} }
        svg|rect { fill: red }
        .a { color: red; & .b { color: blue } color: green; @media (min-width: 1px) { color: pink } }
        @page { margin: 1in }
        "#,
    );
    let imports: Vec<&ImportRule> = s.rules.iter().filter_map(|r| if let CssRule::Import(i) = r { Some(i) } else { None }).collect();
    assert_eq!(imports.len(), 2, "@import after @namespace is ignored");
    assert_eq!(imports[0].url, "a.css");
    assert_eq!(imports[0].layer, Some(Some("base".into())));
    assert!(imports[0].supports.is_some());
    assert!(imports[0].media.matches(&Environment::default()));
    assert_eq!(imports[1].url, "b.css");
    assert!(!imports[1].media.matches(&Environment::default()));
    assert_eq!(s.namespaces.resolve("svg"), Some("http://www.w3.org/2000/svg"));
    assert!(s.rules.iter().any(|r| matches!(r, CssRule::LayerStatement(n) if n == &["reset".to_string(), "base.inner".to_string()])));
    let ff = s.rules.iter().find_map(|r| if let CssRule::FontFace(f) = r { Some(f) } else { None }).unwrap();
    assert_eq!(ff.family().as_deref(), Some("My Font"));
    assert_eq!(
        ff.sources(),
        vec![
            FontSource::Local("Foo Bar".into()),
            FontSource::Url { url: "f.woff2".into(), format: Some("woff2".into()) },
            FontSource::Url { url: "g.ttf".into(), format: None }
        ]
    );
    let kfs: Vec<&Keyframes> = s.rules.iter().filter_map(|r| if let CssRule::Keyframes(k) = r { Some(k) } else { None }).collect();
    assert_eq!(kfs[0].name, "spin");
    assert_eq!(kfs[0].frames.iter().map(|f| f.offsets.clone()).collect::<Vec<_>>(), vec![vec![0.0], vec![0.5, 0.75], vec![1.0]]);
    assert_eq!(kfs[0].frames[2].declarations.len(), 1, "!important is ignored in keyframes");
    assert_eq!(kfs[1].name, "quoted");
    // nesting: .a { color: red; & .b {…} color: green; @media {…} }
    let a = s.rules.iter().find_map(|r| if let CssRule::Style(st) = r { if st.declarations.iter().any(|d| d.name == "color") { Some(st) } else { None } } else { None }).unwrap();
    assert_eq!(a.declarations.len(), 1);
    assert_eq!(a.children.len(), 3, "nested rule, nested declarations, nested @media");
    if let CssRule::Style(b) = &a.children[0] {
        assert_eq!((b.selectors.0[0].specificity.a, b.selectors.0[0].specificity.b), (0, 2));
    } else {
        panic!("nested style rule");
    }
    assert!(s.rules.iter().any(|r| matches!(r, CssRule::Other(o) if o.name == "page")));
    println!("stylesheet data: 22 KATs");
}

/// A test DOM: html > body > div#t.c > span
fn tiny() -> Dom {
    let mut d = Dom { in_document: true, ..Default::default() };
    let at = |k: &str, v: &str| (String::new(), k.to_string(), v.to_string());
    let html = d.push(None, "html", HTML_NS, vec![]);
    let body = d.push(Some(html), "body", HTML_NS, vec![]);
    let div = d.push(Some(body), "div", HTML_NS, vec![at("id", "t"), at("class", "c")]);
    d.push(Some(div), "span", HTML_NS, vec![]);
    d
}

fn winner(rules: &RuleSet, d: &Dom, i: usize, inline: &[Declaration], prop: &str) -> Option<String> {
    let a = cascade(rules, &d.el(i), inline, None);
    a.iter().rev().find(|x| x.declaration.name == prop).map(|x| to_css(css_core::parser::trim_ws(&x.declaration.value)))
}

#[test]
fn m3_cascade_order() {
    let d = tiny();
    let ua = parse_stylesheet("div { color: ua; background: ua !important; } span { color: ua-span }");
    let author = parse_stylesheet(
        r#"
        @layer one, two;
        @layer two { #t { color: two-id } div { margin: two !important } }
        @layer one { #t { color: one-id } div { margin: one !important } }
        div.c { color: unlayered-low; }
        div { background: author !important; padding: a; padding: b; }
        .c { border: x } div { border: y }
        @media print { div { color: print } }
        @supports (foo: bar) { div { outline: s1 } }
        @supports not (foo: bar) { div { outline: s2 } }
        @layer three { div { color: three !important } }
        div { color: unlayered-important !important }
        "#,
    );
    let env = Environment::default();
    let supports = |d: &Declaration| d.name != "foo";
    let import = |_: &str| None;
    let cond = Conditions { env: &env, supports: &supports, import: &import };
    let mut rs = RuleSet::new();
    rs.add_sheet(&ua, Origin::UserAgent, &cond);
    rs.add_sheet(&author, Origin::Author, &cond);
    let div = 2;
    // important: layer three (declared last) loses to … unlayered important loses to layered important
    assert_eq!(winner(&rs, &d, div, &[], "color").as_deref(), Some("three"));
    // important across layers: the earlier layer wins
    assert_eq!(winner(&rs, &d, div, &[], "margin").as_deref(), Some("one"));
    // UA !important beats author !important
    assert_eq!(winner(&rs, &d, div, &[], "background").as_deref(), Some("ua"));
    // order of appearance, then specificity
    assert_eq!(winner(&rs, &d, div, &[], "padding").as_deref(), Some("b"));
    assert_eq!(winner(&rs, &d, div, &[], "border").as_deref(), Some("x"));
    assert_eq!(winner(&rs, &d, div, &[], "outline").as_deref(), Some("s2"));
    // normal declarations: unlayered beats layered regardless of specificity
    let author2 = parse_stylesheet("@layer l { #t { color: layered-id } } div { color: unlayered-type }");
    let mut rs2 = RuleSet::new();
    rs2.add_sheet(&author2, Origin::Author, &cond);
    assert_eq!(winner(&rs2, &d, div, &[], "color").as_deref(), Some("unlayered-type"));
    // style attribute: beats any selector of the same origin and importance, loses to !important rules
    let inline = parse_style_attribute("color: inline; padding: inline !important");
    assert_eq!(winner(&rs2, &d, div, &inline, "color").as_deref(), Some("inline"));
    let author3 = parse_stylesheet("#t#t#t { padding: rule !important; color: rule !important }");
    let mut rs3 = RuleSet::new();
    rs3.add_sheet(&author3, Origin::Author, &cond);
    assert_eq!(winner(&rs3, &d, div, &inline, "padding").as_deref(), Some("inline"));
    assert_eq!(winner(&rs3, &d, div, &inline, "color").as_deref(), Some("rule"));
    // @import resolution with a layer
    let imported = parse_stylesheet("div { color: imported }");
    let main = parse_stylesheet("@import 'x.css' layer(lib); div { color: main }");
    let import2 = |u: &str| if u == "x.css" { Some(&imported) } else { None };
    let cond2 = Conditions { env: &env, supports: &supports, import: &import2 };
    let mut rs4 = RuleSet::new();
    rs4.add_sheet(&main, Origin::Author, &cond2);
    assert_eq!(rs4.entries.len(), 2);
    assert_eq!(winner(&rs4, &d, div, &[], "color").as_deref(), Some("main"));
    println!("cascade order: 12 KATs");
}

/// css-parsing-tests color files: the input parsed as a `<color>` and serialized (Chromium's format).
#[test]
fn m3_color_kats() {
    let mut total = (0, 0);
    for f in ["color3.json", "color3_keywords.json", "color3_hsl.json", "color4_hwb.json"] {
        let text = match common::fetch(f) {
            Ok(t) => t,
            Err(e) => {
                println!("{f}: SKIPPED ({e})");
                continue;
            }
        };
        let v = json::parse(&text);
        let (mut p, mut t) = (0, 0);
        let mut shown = 0;
        for pair in v.arr().chunks(2) {
            t += 1;
            let got = match parse_color(&parse_component_values(pair[0].str())) {
                Some(Color::CurrentColor) => Some("currentcolor".to_string()),
                Some(Color::Rgba(c)) => Some(c.serialize()),
                None => None,
            };
            let want = match &pair[1] {
                json::Json::Str(s) => Some(s.clone()),
                _ => None,
            };
            if got == want {
                p += 1;
            } else if shown < 8 {
                shown += 1;
                println!("  FAIL {f} {:?}: want {want:?} got {got:?}", pair[0].str());
            }
        }
        println!("{f}: {p}/{t}");
        total = (total.0 + p, total.1 + t);
    }
    println!("color KATs: {}/{}", total.0, total.1);
    assert_eq!(total.0, total.1);
}
