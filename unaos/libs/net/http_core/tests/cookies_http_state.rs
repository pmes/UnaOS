//! The http-state suite (abarth/http-state, the RFC 6265 user-agent tests) as KATs for the cookie jar. Each
//! test is what its testserver does: a response to `http://home.example.org:8888/cookie-parser?<name>`
//! carrying the test file's fields (Set-Cookie lines, maybe a Location), then a request to the Location
//! (default `/cookie-parser-result?<name>`), whose Cookie header must equal the `-expected` file's
//! `Cookie:` value (or be absent when the file has none). `disabled-*` tests are skipped, as the suite does;
//! `optional-*` are counted separately.

mod common;

use std::process::Command;

use common::{cache_dir, crate_dir, vectors};
use http_core::cookie::CookieJar;
use http_core::url::Url;

fn fetch_suite() -> Option<std::path::PathBuf> {
    let (base, sha) = vectors("http-state").into_iter().next()?;
    let names: Vec<String> = std::fs::read_to_string(crate_dir().join("tests/http-state.list")).ok()?.lines().map(String::from).collect();
    let dir = cache_dir().join("http-state-155e45c6");
    std::fs::create_dir_all(&dir).ok()?;
    let missing: Vec<&String> = names.iter().filter(|n| !dir.join(n).exists()).collect();
    if !missing.is_empty() {
        let mut cfg = String::new();
        for n in &missing {
            cfg.push_str(&format!("url = \"{base}{n}\"\noutput = \"{}\"\n", dir.join(n).display()));
        }
        let cfgp = dir.join("curl.cfg");
        std::fs::write(&cfgp, cfg).ok()?;
        let st = Command::new("curl").args(["-sSfL", "--parallel", "--parallel-max", "16", "--max-time", "120", "-K"]).arg(&cfgp).status();
        if !matches!(st, Ok(s) if s.success()) {
            eprintln!("SKIP (offline?): could not fetch the http-state suite");
            return None;
        }
    }
    let mut cat = Vec::new();
    for n in &names {
        cat.extend_from_slice(n.as_bytes());
        cat.push(b'\n');
        cat.extend_from_slice(&std::fs::read(dir.join(n)).ok()?);
    }
    let catp = dir.join("all.cat");
    std::fs::write(&catp, &cat).ok()?;
    let out = Command::new("sha256sum").arg(&catp).output().ok()?;
    let got = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string();
    if got != sha {
        eprintln!("SKIP: http-state aggregate sha256 {got} != {sha}");
        return None;
    }
    Some(dir)
}

/// The testserver's `(\S+):\s*(.*)` over each line.
fn fields(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let colon = l.find(':')?;
            let name = l[..colon].trim_start();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            Some((name.to_string(), l[colon + 1..].trim_start().to_string()))
        })
        .collect()
}

#[test]
fn http_state_parser_suite() {
    let Some(dir) = fetch_suite() else { return };
    let names: Vec<String> = std::fs::read_to_string(crate_dir().join("tests/http-state.list")).unwrap().lines().map(String::from).collect();
    let now = 1_300_000_000; // 2011-03-13: the suite's own era (its "future" Expires are 2019, its past ones 2007).
    let (mut pass, mut total, mut opt_pass, mut opt_total) = (0, 0, 0, 0);
    for n in names.iter().filter(|n| n.ends_with("-test")) {
        let test = n.trim_end_matches("-test");
        if test.starts_with("disabled-") {
            continue;
        }
        let input = std::fs::read(dir.join(n)).unwrap();
        let input = String::from_utf8_lossy(&input).into_owned();
        let expected = String::from_utf8_lossy(&std::fs::read(dir.join(format!("{test}-expected"))).unwrap()).into_owned();
        let want: Option<String> = {
            let v: Vec<&str> = expected.lines().filter_map(|l| l.strip_prefix("Cookie:")).collect();
            if v.len() == 1 { Some(v[0].trim_start().to_string()) } else { None }
        };
        let req = Url::parse(&format!("http://home.example.org:8888/cookie-parser?{test}")).unwrap();
        let mut jar = CookieJar::new();
        let mut location = format!("/cookie-parser-result?{test}");
        for (k, v) in fields(&input) {
            if k.eq_ignore_ascii_case("set-cookie") {
                // The value reaches the client as an HTTP field: OWS around it is not part of it.
                jar.set_cookie(&req, v.trim_matches(|c| c == ' ' || c == '\t'), now, true);
            } else if k.eq_ignore_ascii_case("location") {
                location = v.trim().to_string();
            }
        }
        let next = req.join(&location).unwrap();
        let got = jar.cookie_header(&next, now, true);
        let ok = got == want;
        if test.starts_with("optional-") {
            opt_total += 1;
            opt_pass += ok as usize;
        } else {
            total += 1;
            pass += ok as usize;
        }
        if !ok {
            println!("FAIL {test}: got {got:?}, want {want:?}\n  {}", input.replace('\n', "\n  "));
        }
    }
    println!("http-state: {pass}/{total} pass (optional: {opt_pass}/{opt_total})");
    assert!(total >= 200);
    assert!(pass >= FLOOR, "http-state regressed: {pass}/{total}");
}

const FLOOR: usize = 214;
