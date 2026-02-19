use anyhow::{Context, Result};
use elessar::{Context as ElessarContext, Spline}; // Import the Compass
use regex::Regex;
use std::env;
use std::fs;
use std::path::Path;
use std::process;

const MEMORIA_FILENAME: &str = "MEMORIA.md";

fn main() -> Result<()> {
    println!("🛡️  SENTINEL: SYSTEMS ONLINE.");

    // 1. ORIENTATION (Using Elessar)
    let cwd = env::current_dir()?;
    let ctx = ElessarContext::new(&cwd);

    match ctx.spline {
        Spline::UnaOS => {
            println!("✨ CONTEXT CONFIRMED: SPLINE [UnaOS]");
            println!("   The Monolith is present.");
        }
        _ => {
            eprintln!("🚨 WRONG DIMENSION DETECTED.");
            eprintln!("   Current Spline: {:?}", ctx.spline);
            eprintln!("   The Sentinel must act from the Monolith Root.");
            process::exit(1);
        }
    }

    // 2. Locate Memoria (We know it exists because Elessar found it)
    let content = fs::read_to_string(MEMORIA_FILENAME)
        .with_context(|| format!("Could not read {}", MEMORIA_FILENAME))?;

    // 3. Parse Crates and Bins
    let re = Regex::new(r"\*\s+\*\*\[(CRATE|BIN)\]\s+`(.*?)`:\*\*").unwrap();

    let mut errors = 0;
    let mut verified = 0;

    for cap in re.captures_iter(&content) {
        let type_tag = &cap[1];
        let rel_path = &cap[2];
        let path = Path::new(rel_path);

        if path.exists() {
            println!("   [PASS] {} found at '{}'", type_tag, rel_path);
            verified += 1;
        } else {
            println!("❌ [FAIL] MEMORIA claims '{}' exists, but it does not!", rel_path);
            errors += 1;
        }
    }

    // 4. Verdict
    println!("----------------------------------------");
    if errors > 0 {
        println!("🚨 HALLUCINATION DETECTED. {} MISSING ARTIFACTS.", errors);
        process::exit(1);
    } else {
        println!("✨ REALITY CONFIRMED. {} ARTIFACTS VERIFIED.", verified);
        Ok(())
    }
}
