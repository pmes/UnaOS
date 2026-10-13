//! URLCORE (SR87) — `html_core::url` against the WPT URL suite at da1f6d20caf40b4003ae288dda8c97f252dd9264:
//! `url/resources/urltestdata.json` (+ `URL.canParse`), `setters_tests.json`, `toascii.json` and
//! `IdnaTestV2.json`, driven the way the suite's `.window.js` files drive them. The files are read from
//! `$UNAOS_WPT_DIR/url/resources/` (a sparse checkout of WPT's `url/`); the tests skip when it is unset.

mod common;

use common::{parse_json, Json};
use html_core::url::Url;
use std::path::PathBuf;

fn load(name: &str) -> Option<Json> {
    let dir = std::env::var_os("UNAOS_WPT_DIR")?;
    let p = PathBuf::from(dir).join("url/resources").join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    Some(parse_json(&text))
}

/// A JS string reaching a USVString argument: a lone surrogate becomes U+FFFD.
fn s(j: &Json) -> String {
    String::from_utf16_lossy(j.units().unwrap_or(&[]))
}

fn getter(u: &Url, k: &str) -> String {
    match k {
        "href" => u.href(),
        "origin" => u.origin(),
        "protocol" => u.protocol(),
        "username" => u.username().to_string(),
        "password" => u.password().to_string(),
        "host" => u.host_str(),
        "hostname" => u.hostname(),
        "port" => u.port_str(),
        "pathname" => u.pathname(),
        "search" => u.search(),
        "hash" => u.hash(),
        other => panic!("no getter {other}"),
    }
}

fn setter(u: &mut Url, k: &str, v: &str) {
    match k {
        "href" => {
            let _ = u.set_href(v);
        }
        "protocol" => u.set_protocol(v),
        "username" => u.set_username(v),
        "password" => u.set_password(v),
        "host" => u.set_host(v),
        "hostname" => u.set_hostname(v),
        "port" => u.set_port(v),
        "pathname" => u.set_pathname(v),
        "search" => u.set_search(v),
        "hash" => u.set_hash(v),
        other => panic!("no setter {other}"),
    }
}

fn report(name: &str, pass: usize, total: usize, fails: &[String]) {
    for f in fails.iter().take(300) {
        println!("FAIL {f}");
    }
    println!("WPT {name}: {pass}/{total} pass");
}

/// url-constructor.any.js + url-statics-canparse.any.js over urltestdata.json.
#[test]
fn wpt_urltestdata() {
    let Some(data) = load("urltestdata.json") else { return };
    let (mut pass, mut total, mut can_pass) = (0, 0, 0);
    let mut fails = Vec::new();
    for case in data.arr() {
        let Some(input) = case.get("input") else { continue };
        total += 1;
        let input = s(input);
        let base = match case.get("base") {
            Some(Json::Null) | None => None,
            Some(b) => Some(s(b)),
        };
        let failure = case.get("failure") == Some(&Json::Bool(true));
        let got = Url::parse_str_with_base(&input, base.as_deref());
        if Url::can_parse(&input, base.as_deref()) == !failure {
            can_pass += 1;
        }
        let ok = match (&got, failure) {
            (Err(_), true) => true,
            (Ok(u), true) => {
                fails.push(format!("{input:?} base={base:?}: expected failure, got {}", u.href()));
                false
            }
            (Err(e), false) => {
                fails.push(format!("{input:?} base={base:?}: failed ({e}), want {:?}", case.get("href").map(s)));
                false
            }
            (Ok(u), false) => {
                let mut good = true;
                for k in ["href", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash", "origin"] {
                    if let Some(want) = case.get(k) {
                        let v = getter(u, k);
                        if s(want) != v {
                            fails.push(format!("{input:?} base={base:?}: {k} = {v:?}, want {:?}", s(want)));
                            good = false;
                            break;
                        }
                    }
                }
                good
            }
        };
        if ok {
            pass += 1;
        }
    }
    report("urltestdata", pass, total, &fails);
    println!("WPT URL.canParse: {can_pass}/{total} pass");
    assert!(total > 800);
    assert_eq!(pass, total, "urltestdata");
    assert_eq!(can_pass, total, "URL.canParse");
}

/// url-setters.any.js over setters_tests.json.
#[test]
fn wpt_setters() {
    let Some(data) = load("setters_tests.json") else { return };
    let Json::Obj(map, keys) = &data else { panic!("setters_tests.json: not an object") };
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for key in keys {
        if key == "comment" {
            continue;
        }
        for case in map[key].arr() {
            let (Some(href), Some(nv), Some(exp)) = (case.get("href"), case.get("new_value"), case.get("expected")) else {
                continue;
            };
            total += 1;
            let href = s(href);
            let nv = s(nv);
            let Ok(mut u) = Url::parse(&href) else {
                fails.push(format!("{key} {href:?}: href did not parse"));
                continue;
            };
            setter(&mut u, key, &nv);
            let mut good = true;
            for k in exp.keys() {
                let want = s(exp.get(k).unwrap_or(&Json::Null));
                let v = getter(&u, k);
                if v != want {
                    fails.push(format!("{key} {href:?} <- {nv:?}: {k} = {v:?}, want {want:?}"));
                    good = false;
                    break;
                }
            }
            if good {
                pass += 1;
            }
        }
    }
    report("setters_tests", pass, total, &fails);
    assert!(total > 200);
    assert_eq!(pass, total, "setters_tests");
}

/// toascii.window.js: `new URL("https://{input}/x")`, then the `host` and `hostname` setters on `https://x/x`.
#[test]
fn wpt_toascii() {
    let Some(data) = load("toascii.json") else { return };
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for case in data.arr() {
        let Some(input) = case.get("input") else { continue };
        let input = s(input);
        let want = match case.get("output") {
            Some(Json::Null) | None => None,
            Some(o) => Some(s(o)),
        };
        total += 1;
        let got = Url::parse(&format!("https://{input}/x"));
        let ok = match (&want, &got) {
            (None, Err(_)) => true,
            (Some(w), Ok(u)) => {
                u.host_str() == *w && u.hostname() == *w && u.pathname() == "/x" && u.href() == format!("https://{w}/x")
            }
            _ => false,
        };
        if ok {
            pass += 1;
        } else {
            fails.push(format!("{input:?} (URL): got {:?}, want {want:?}", got.as_ref().map(|u| u.host_str())));
        }
        for k in ["host", "hostname"] {
            total += 1;
            let mut u = Url::parse("https://x/x").unwrap();
            setter(&mut u, k, &input);
            let v = getter(&u, k);
            let w = want.clone().unwrap_or_else(|| "x".into());
            if v == w {
                pass += 1;
            } else {
                fails.push(format!("{input:?} (URL.{k}): {v:?}, want {w:?}"));
            }
        }
    }
    report("toascii", pass, total, &fails);
    assert!(total > 100);
    assert_eq!(pass, total, "toascii");
}

/// IdnaTestV2.window.js: `new URL("https://{input}/x")` — host, hostname, pathname and href, or a failure.
#[test]
fn wpt_idna_v2() {
    let Some(data) = load("IdnaTestV2.json") else { return };
    let (mut pass, mut total) = (0, 0);
    let mut fails = Vec::new();
    for case in data.arr() {
        let Some(input) = case.get("input") else { continue };
        let input = s(input);
        if input.is_empty() {
            continue;
        }
        total += 1;
        let got = Url::parse(&format!("https://{input}/x"));
        let ok = match (case.get("output"), &got) {
            (Some(Json::Null), Err(_)) => true,
            (Some(want), Ok(u)) if *want != Json::Null => {
                let w = s(want);
                u.host_str() == w && u.hostname() == w && u.pathname() == "/x" && u.href() == format!("https://{w}/x")
            }
            _ => false,
        };
        if ok {
            pass += 1;
        } else {
            fails.push(format!("{input:?}: got {:?}, want {:?}", got.as_ref().map(|u| u.host_str()), case.get("output").map(s)));
        }
    }
    report("IdnaTestV2", pass, total, &fails);
    assert!(total > 1000);
    assert_eq!(pass, total, "IdnaTestV2");
}
