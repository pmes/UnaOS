//! WPT `url/resources/urltestdata.json` as known-answer tests for the WHATWG URL parser: every entry with an
//! `input` is parsed (against `base` when given) and either must fail (`"failure": true`) or must reproduce
//! href, protocol, username, password, host, hostname, port, pathname, search, hash (and origin when listed).
//! Pass/total is printed; the floor below is the count this tree reached, so a regression goes red.

mod common;

use common::{fetch, parse_json, vectors, Json};
use http_core::url::Url;

fn s(j: &Json) -> String {
    // JS strings are UTF-16; a lone surrogate becomes U+FFFD when it reaches the URL parser (USVString).
    String::from_utf16_lossy(j.units().unwrap_or(&[]))
}

#[test]
fn wpt_urltestdata() {
    let Some((url, sha)) = vectors("wpt").into_iter().next() else { panic!("vectors.txt: wpt") };
    let Some(text) = fetch(&url, &sha) else { return };
    let data = parse_json(&text);
    let (mut pass, mut total, mut shown) = (0, 0, 0);
    let mut fails = Vec::new();
    for case in data.arr() {
        let Some(input) = case.get("input") else { continue };
        total += 1;
        let input = s(input);
        let base = match case.get("base") {
            Some(Json::Null) | None => None,
            Some(b) => Some(s(b)),
        };
        let base_url = match &base {
            Some(b) => match Url::parse(b) {
                Ok(u) => Some(u),
                Err(_) => {
                    if case.get("failure") == Some(&Json::Bool(true)) {
                        pass += 1;
                    } else {
                        fails.push(format!("base failed: {b:?}"));
                    }
                    continue;
                }
            },
            None => None,
        };
        let got = Url::parse_with_base(&input, base_url.as_ref());
        let ok = if case.get("failure") == Some(&Json::Bool(true)) {
            match &got {
                Err(_) => true,
                Ok(u) => {
                    fails.push(format!("{input:?} base={base:?}: expected failure, got {}", u.href()));
                    false
                }
            }
        } else {
            match &got {
                Err(e) => {
                    fails.push(format!("{input:?} base={base:?}: failed ({e}), want {:?}", case.get("href").map(s)));
                    false
                }
                Ok(u) => {
                    let fields: [(&str, String); 11] = [
                        ("href", u.href()),
                        ("protocol", u.protocol()),
                        ("username", u.username().to_string()),
                        ("password", u.password().to_string()),
                        ("host", u.host_str()),
                        ("hostname", u.hostname()),
                        ("port", u.port().map(|p| p.to_string()).unwrap_or_default()),
                        ("pathname", u.pathname()),
                        ("search", u.search()),
                        ("hash", u.hash()),
                        ("origin", u.origin()),
                    ];
                    let mut good = true;
                    for (k, v) in fields {
                        if let Some(want) = case.get(k) {
                            if s(want) != v {
                                fails.push(format!("{input:?} base={base:?}: {k} = {v:?}, want {:?}", s(want)));
                                good = false;
                                break;
                            }
                        }
                    }
                    good
                }
            }
        };
        if ok {
            pass += 1;
        }
    }
    for f in &fails {
        if shown < 400 {
            println!("FAIL {f}");
            shown += 1;
        }
    }
    println!("WPT urltestdata: {pass}/{total} pass");
    assert!(total > 800);
    assert!(pass >= FLOOR, "WPT urltestdata regressed: {pass}/{total} < {FLOOR}");
}

const FLOOR: usize = 896;

/// WPT's IdnaTestV2 through the URL parser exactly as url/IdnaTestV2.window.js drives it: `https://{input}/x`,
/// the host must equal `output`, or the parse must fail when `output` is null. A measure of the IDNA ceiling.
#[test]
fn wpt_idna_v2() {
    let Some((url, sha)) = vectors("idna").into_iter().next() else { panic!("vectors.txt: idna") };
    let Some(text) = fetch(&url, &sha) else { return };
    let data = parse_json(&text);
    let (mut pass, mut total) = (0, 0);
    for case in data.arr() {
        let Some(input) = case.get("input") else { continue };
        let input = s(input);
        // The window.js test skips inputs that would be parsed as something other than a host.
        if input.contains(|c| matches!(c, '/' | '\\' | '?' | '#' | ':' | '@' | '[' | ']')) {
            continue;
        }
        total += 1;
        let got = Url::parse(&format!("https://{input}/x"));
        let ok = match (case.get("output"), &got) {
            (Some(Json::Null), Err(_)) => true,
            (Some(want), Ok(u)) if *want != Json::Null => u.host_str() == s(want),
            _ => false,
        };
        if ok {
            pass += 1;
        }
    }
    println!("WPT IdnaTestV2: {pass}/{total} pass");
    assert!(pass >= IDNA_FLOOR, "IDNA regressed: {pass}/{total}");
}

const IDNA_FLOOR: usize = 1353;
