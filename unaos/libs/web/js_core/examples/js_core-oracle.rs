//! `js_core-oracle`: run the oracle pages (tests/oracle/page*.html) on js_core's tiny DOM host and compare the
//! serialized DOM with Chromium's `--dump-dom` of the same page.
//!
//! cargo run --release -p js_core --example js_core-oracle -- [--chromium <headless_shell>] [--record]
//! `--record` stores Chromium's dumps as tests/oracle/page*.chromium.html (the `oracle` test compares against
//! them, so the gate needs no browser).

#[path = "../tests/oracle/host.rs"]
mod host;

use std::path::{Path, PathBuf};
use std::process::Command;

fn chromium_dump(bin: &Path, page: &Path, profile: &Path) -> Option<String> {
    let out = Command::new(bin)
        .args(["--no-sandbox", "--disable-gpu", "--virtual-time-budget=10000", "--dump-dom"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("file://{}", page.display()))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut chromium = PathBuf::from("/opt/pw-browsers/chromium_headless_shell-1194/chrome-linux/headless_shell");
    let mut record = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--chromium" => {
                chromium = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--record" => record = true,
            _ => {}
        }
        i += 1;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle");
    let profile = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../target/oracle-chromium-profile");
    let _ = std::fs::create_dir_all(&profile);
    let mut agree = 0;
    let mut total = 0;
    for n in 1..=3 {
        let page = dir.join(format!("page{}.html", n));
        let html = std::fs::read_to_string(&page).expect("page");
        let (ours, console) = host::run_page(&html);
        if !console.is_empty() {
            println!("page{} console:\n{}", n, console);
        }
        let reference = if chromium.exists() {
            let d = chromium_dump(&chromium, &page, &profile);
            if record {
                if let Some(d) = &d {
                    std::fs::write(dir.join(format!("page{}.chromium.html", n)), format!("{}\n", d)).expect("write");
                }
            }
            d
        } else {
            std::fs::read_to_string(dir.join(format!("page{}.chromium.html", n))).ok().map(|s| s.trim().to_string())
        };
        total += 1;
        match reference {
            Some(r) if r == ours.trim() => {
                agree += 1;
                println!("page{}: identical DOM ({} bytes)", n, ours.len());
            }
            Some(r) => {
                println!("page{}: DIFFERENT", n);
                let (a, b): (Vec<&str>, Vec<&str>) = (r.split('\n').collect(), ours.split('\n').collect());
                for k in 0..a.len().max(b.len()) {
                    let (x, y) = (a.get(k).copied().unwrap_or(""), b.get(k).copied().unwrap_or(""));
                    if x != y {
                        println!("  line {}:\n    chromium: {}\n    js_core:  {}", k + 1, x, y);
                    }
                }
            }
            None => println!("page{}: no Chromium reference available", n),
        }
    }
    println!("oracle: {}/{} pages identical", agree, total);
    if agree != total {
        std::process::exit(1);
    }
}
