//! M3 — the Chromium oracle on EYES's 18 Aether cases (fixtures: tests/oracle/*.json, made by
//! tools/oracle.mjs). For every element of Chromium's own serialized DOM, css_core's cascade (UA sheet +
//! the page's <style> sheets + style attributes, `@media` at the case's viewport) must give the same
//! value as Chromium's getComputedStyle for 20 named properties. Prints per-page agreement.
mod common;
use common::dom::Dom;
use common::json;
use common::style::{agree, sheets, Styler, PROPS};
use css_core::media::Environment;

/// Per page: (cells agreeing, cells, elements agreeing on all 20, elements).
fn run_case(path: &std::path::Path, verbose: bool) -> (String, usize, usize, usize, usize) {
    let fx = json::parse(&std::fs::read_to_string(path).unwrap());
    let name = fx.get("case").unwrap().str().to_string();
    let props: Vec<String> = fx.get("props").unwrap().arr().iter().map(|p| p.str().to_string()).collect();
    assert_eq!(props, PROPS.to_vec(), "{name}: fixture property list");
    let styles: Vec<String> = fx.get("styles").unwrap().arr().iter().map(|s| s.str().to_string()).collect();
    let dom_json = fx.get("dom").unwrap();
    let dom = Dom::from_json(dom_json);
    // Chromium's values, in the same pre-order as Dom::preorder(0)
    let mut want: Vec<Vec<String>> = vec![Vec::new(); dom.nodes.len()];
    fn collect(j: &json::Json, out: &mut Vec<Vec<String>>, next: &mut usize) {
        out[*next] = j.get("s").unwrap().arr().iter().map(|v| v.str().to_string()).collect();
        *next += 1;
        for c in j.get("c").unwrap().arr() {
            collect(c, out, next);
        }
    }
    let mut k = 0;
    collect(dom_json, &mut want, &mut k);
    let env = Environment::viewport(fx.get("width").unwrap().num(), fx.get("height").unwrap().num());
    let (ua, author) = sheets(&styles);
    let styler = Styler::new(&ua, &author, env);
    let got = styler.compute_all(&dom);
    let (mut cells_ok, mut cells, mut els_ok) = (0, 0, 0);
    for i in 0..dom.nodes.len() {
        let ours = got[i].serialize();
        let mut all = true;
        for (p, prop) in PROPS.iter().enumerate() {
            cells += 1;
            if agree(&want[i][p], &ours[p]) {
                cells_ok += 1;
            } else {
                all = false;
                if verbose {
                    let id = dom.id_of(i).map(|s| format!("#{s}")).unwrap_or_default();
                    let cls = dom.el(i).attr_class();
                    let cls = format!("{cls} {:?}", dom.nodes[i].attrs.iter().map(|a| format!("{}={}", a.1, a.2)).collect::<Vec<_>>());
                    println!("    {name}: <{}{id}{cls}> {prop}: chromium {:?} ours {:?}", dom.nodes[i].name, want[i][p], ours[p]);
                }
            }
        }
        if all {
            els_ok += 1;
        }
    }
    (name, cells_ok, cells, els_ok, dom.nodes.len())
}

trait ClassAttr {
    fn attr_class(&self) -> String;
}
impl ClassAttr for common::dom::El<'_> {
    fn attr_class(&self) -> String {
        use css_core::matching::Element;
        self.attr("class").map(|c| format!(".{}", c.replace(' ', "."))).unwrap_or_default()
    }
}

fn run_dir(sub: &str, expect_files: usize) -> (usize, usize, usize, usize) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(sub);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().map(|e| e == "json").unwrap_or(false)).collect();
    files.sort();
    assert_eq!(files.len(), expect_files, "{sub}: fixture count");
    let verbose = std::env::var_os("ORACLE_VERBOSE").is_some();
    let (mut c_ok, mut c_all, mut e_ok, mut e_all) = (0, 0, 0, 0);
    println!("{sub}: page                  cells agreeing        elements fully agreeing");
    for f in &files {
        let (name, a, b, c, d) = run_case(f, verbose);
        println!("  {name:30} {a:4}/{b:4} ({:5.1}%)   {c:3}/{d:3}", 100.0 * a as f64 / b as f64);
        c_ok += a;
        c_all += b;
        e_ok += c;
        e_all += d;
    }
    println!("  TOTAL                          {c_ok:4}/{c_all:4} ({:5.1}%)   {e_ok:3}/{e_all:3}", 100.0 * c_ok as f64 / c_all as f64);
    (c_ok, c_all, e_ok, e_all)
}

/// EYES's 18 Aether cases. Floor = the agreement measured when this milestone landed (a regression gate).
#[test]
fn m3_chromium_oracle_eyes() {
    let (c_ok, c_all, _, _) = run_dir("tests/oracle", 18);
    assert!(c_ok * 1000 >= c_all * 1000, "EYES oracle regressed: {c_ok}/{c_all}");
}

/// The stress set: CSSCORE's own pages + WPT's style-based selector tests, same oracle.
#[test]
fn m3_chromium_oracle_stress() {
    let (c_ok, c_all, _, _) = run_dir("tests/oracle-stress", 9);
    assert!(c_ok * 1000 >= c_all * 1000, "stress oracle regressed: {c_ok}/{c_all}");
}
