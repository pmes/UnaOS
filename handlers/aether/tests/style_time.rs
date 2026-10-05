//! AETHERSTYLE (SR54) M4: style time — Aether's whole cascade (`css::apply_stylesheets`: parse the
//! sheets, match every element, cascade, feed the computed-value step, fold into the box tree), with the
//! re-layout that follows it reported separately — on EYES case 01 and on a saved Wikipedia article.
//!
//! Run: `cargo test --release -p aether --test style_time -- --nocapture`. The article and the big sheet
//! are fetched at test time (tests/vectors.txt: URL + sha256), and the Wikipedia case is SKIPPED offline
//! or with `AETHER_OFFLINE=1`. `STYLE_TIME_RUNS` (default 5) sets the repetitions; the median is printed.

use std::path::{Path, PathBuf};
use std::time::Duration;

const VECTORS_TXT: &str = include_str!("vectors.txt");

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A pinned file's text, from the cache or downloaded and verified; `Err(reason)` offline.
fn fetch(name: &str) -> Result<String, String> {
    let line = VECTORS_TXT
        .lines()
        .find(|l| {
            let mut it = l.split_whitespace();
            it.next() == Some("FETCH") && it.next() == Some(name)
        })
        .ok_or_else(|| format!("{name}: not in vectors.txt"))?;
    let f: Vec<&str> = line.split_whitespace().collect();
    let (want, url) = (f[2], f[3]);
    let dir = std::env::var_os("AETHER_VECTORS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("unaos-aether-vectors"));
    let path = dir.join(name);
    if let Ok(b) = std::fs::read(&path) {
        if hex(&crypto_core::sha2::sha256(&b)) == want {
            return Ok(String::from_utf8_lossy(&b).into_owned());
        }
    }
    if std::env::var_os("AETHER_OFFLINE").is_some() {
        return Err(format!("{name}: AETHER_OFFLINE set"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{name}: {e}"))?;
    let tmp = dir.join(format!("{name}.part{}", std::process::id()));
    let st = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "120", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("{name}: curl: {e}"))?;
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: offline (curl {st})"));
    }
    let b = std::fs::read(&tmp).map_err(|e| format!("{name}: {e}"))?;
    let got = hex(&crypto_core::sha2::sha256(&b));
    if got != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: sha256 {got} != pinned {want}"));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{name}: {e}"))?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

fn runs() -> usize {
    std::env::var("STYLE_TIME_RUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(5)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// (median style time, median re-layout time, elements, sheets bytes)
fn time_page(html: &str, extra: &[String], w: f32, h: f32) -> (Duration, Duration, usize, usize) {
    let doc = aether::dom::parse_html(html);
    let mut sheets: Vec<String> =
        doc.select("style").unwrap().map(|s| s.as_node().text_contents()).collect();
    sheets.extend(extra.iter().cloned());
    let elements = doc.select("*").unwrap().count();
    let bytes = sheets.iter().map(|s| s.len()).sum();
    let (mut style, mut layout) = (Vec::new(), Vec::new());
    for _ in 0..runs() {
        let mut tree = aether::layout::build_tree(&doc, w, h);
        aether::css::apply_stylesheets(&mut tree, &sheets);
        let t = aether::css::last_style_timing();
        style.push(t.0);
        layout.push(t.1);
    }
    (median(style), median(layout), elements, bytes)
}

fn report(name: &str, r: (Duration, Duration, usize, usize)) {
    println!(
        "style-time {name:28} style {:9.2} ms   re-layout {:9.2} ms   {:5} elements  {:7} CSS bytes",
        r.0.as_secs_f64() * 1e3,
        r.1.as_secs_f64() * 1e3,
        r.2,
        r.3
    );
}

#[test]
fn style_time_eyes_01() {
    let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/eyes/suites/aether/corpus/01-blog-article.html");
    let html = std::fs::read_to_string(page).unwrap();
    report("eyes/01-blog-article", time_page(&html, &[], 800.0, 600.0));
}

#[test]
fn style_time_wikipedia() {
    let (html, big) = match (fetch("wikipedia-2.html"), fetch("bootstrap-5.3.3.css")) {
        (Ok(h), Ok(c)) => (h, c),
        (a, b) => {
            println!("style-time wikipedia: SKIPPED ({:?} {:?})", a.err(), b.err());
            return;
        }
    };
    report("wikipedia-2 (inline styles)", time_page(&html, &[], 800.0, 600.0));
    report("wikipedia-2 + bootstrap 5.3.3", time_page(&html, &[big], 800.0, 600.0));
}

/// The rule hash + ancestor filter are exact on the Wikipedia article under the 2.7k-rule sheet: for
/// every element, the same applied declarations in the same cascade order as the full scan.
#[test]
fn rule_index_exact_on_wikipedia() {
    use css_core::cascade::{cascade, cascade_filtered, AncestorFilter, Conditions, Origin, RuleSet};
    let (html, big) = match (fetch("wikipedia-2.html"), fetch("bootstrap-5.3.3.css")) {
        (Ok(h), Ok(c)) => (h, c),
        (a, b) => {
            println!("rule-index wikipedia: SKIPPED ({:?} {:?})", a.err(), b.err());
            return;
        }
    };
    let sheet = css_core::stylesheet::parse_stylesheet(&big);
    let env = css_core::media::Environment::viewport(800.0, 600.0);
    let supports = |_: &css_core::parser::Declaration| true;
    let import = |_: &str| None;
    let cond = Conditions { env: &env, supports: &supports, import: &import };
    let mut full = RuleSet::new();
    full.add_sheet(&sheet, Origin::Author, &cond);
    let mut fast = RuleSet::new();
    fast.add_sheet(&sheet, Origin::Author, &cond);
    fast.build_index();
    let st = fast.index().unwrap().stats();
    let doc = aether::dom::parse_html(&html);
    let (mut n, mut applied, mut matched_any) = (0usize, 0usize, 0usize);
    doc.with_doc(|d, _| {
        let mut filters = std::collections::HashMap::new();
        for id in d.descendants(html_core::Document::ROOT) {
            if d.element(id).is_none() {
                continue;
            }
            let el = aether::dom::El::new(d, id);
            let f: AncestorFilter = d.parent_element(id).and_then(|p| filters.get(&p)).copied().unwrap_or_default();
            filters.insert(id, f.with_element(&el));
            let key = |x: &css_core::cascade::Applied| (x.declaration as *const _ as usize, x.order, x.index, x.specificity);
            let a: Vec<_> = cascade(&full, &el, &[], None).iter().map(key).collect();
            let b: Vec<_> = cascade_filtered(&fast, &el, &[], None, Some(&f)).iter().map(key).collect();
            assert_eq!(a, b, "element {}", id.0);
            n += 1;
            applied += a.len();
            matched_any += (!a.is_empty()) as usize;
        }
    });
    println!(
        "rule-index wikipedia: exact on {n} elements ({matched_any} styled, {applied} applied declarations); {} entries — buckets id {:?} class {:?} type {:?} attr {:?} universal {:?}",
        full.entries.len(), st[0], st[1], st[2], st[3], st[4]
    );
}
