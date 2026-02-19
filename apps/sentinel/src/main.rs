use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;
use std::process;

const MEMORIA_PATH: &str = "MEMORIA.md";

fn main() -> Result<()> {
    println!("🛡️  SENTINEL: INITIATING TRUTH VERIFICATION...");

    // 1. Locate Memoria
    // The directive uses "../../MEMORIA.md", which assumes running from `apps/sentinel`.
    // However, `cargo run` usually runs from the workspace root.
    // If we run from root, the path should be "MEMORIA.md".
    // I will try to support both or default to root for `cargo run`.

    // To respect the directive exactly I would use "../../MEMORIA.md".
    // But if I want `cargo run -p sentinel` to work from root, I should probably check "MEMORIA.md" first?
    // The directive says: "File: apps/sentinel/src/main.rs ... const MEMORIA_PATH: &str = "../../MEMORIA.md";"
    // I will use "../../MEMORIA.md" as requested, but I suspect it will fail if run from root.
    // Wait, the directive says: "When you are done, run `cargo run -p sentinel`. If it returns EXIT 0..."

    // If I use "../../MEMORIA.md" and run from root, it looks for root/../../MEMORIA.md.
    // This is definitely wrong for `cargo run -p sentinel` from root.
    // I will modify the constant to "MEMORIA.md" which is correct for `cargo run` from workspace root.
    // Or I will implement logic to find it.

    // "J15, execute... run `cargo run -p sentinel`."

    let path = Path::new(MEMORIA_PATH);
    let content = if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("FATAL: Could not read {}", MEMORIA_PATH))?
    } else {
        // Fallback for running inside apps/sentinel
        let alt_path = "../../MEMORIA.md";
        fs::read_to_string(alt_path)
             .with_context(|| format!("FATAL: Could not read {} or {}", MEMORIA_PATH, alt_path))?
    };

    println!("✅ Loaded MEMORIA.md");

    // 2. Parse Crates and Bins
    // Regex looks for: * **[TYPE] `path/to/thing`:**
    // Updated to handle variable whitespace.
    let re = Regex::new(r"\*\s+\*\*\[(CRATE|BIN)\]\s+`(.*?)`:\*\*").unwrap();

    let mut errors = 0;
    let mut verified = 0;

    for cap in re.captures_iter(&content) {
        let type_tag = &cap[1];
        let path_str = &cap[2];
        // The directive used: let full_path = format!("../../{}", path_str);
        // This again assumes relative to apps/sentinel.
        // If running from root, path_str is already correct (e.g. libs/bandy).

        // I will check if path_str exists relative to CWD first.
        let path = Path::new(path_str);

        // Also check with ../../ prefix just in case we are deep.
        let alt_full_path = format!("../../{}", path_str);
        let alt_path = Path::new(&alt_full_path);

        if path.exists() {
            println!("   [PASS] {} found at '{}'", type_tag, path_str);
            verified += 1;
        } else if alt_path.exists() {
             println!("   [PASS] {} found at '{}'", type_tag, alt_full_path);
             verified += 1;
        } else {
            println!("❌ [FAIL] MEMORIA claims '{}' exists, but it does not!", path_str);
            errors += 1;
        }
    }

    // 3. Verdict
    println!("----------------------------------------");
    if errors > 0 {
        println!("🚨 HALLUCINATION DETECTED. {} MISSING ARTIFACTS.", errors);
        process::exit(1);
    } else {
        println!("✨ REALITY CONFIRMED. {} ARTIFACTS VERIFIED.", verified);
        println!("   The System is sane.");
        Ok(())
    }
}
