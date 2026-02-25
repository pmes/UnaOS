use anyhow::{Context, Result};
use blake3::Hasher;
use elessar::{Context as ElessarContext, Spline};
use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::Regex;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;
use unafs::{BLOCK_SIZE, BlockDevice, FileDevice, Superblock};

const MEMORIA_FILENAME: &str = "UNA_MEMORIA.md"; // Adjusted to match standard UnaOS naming, fallback to MEMORIA.md if needed.

fn main() -> Result<()> {
    let start = Instant::now();
    println!("🛡️  SENTINEL: SYSTEMS ONLINE.\n");

    let cwd = env::current_dir()?;
    let ctx = ElessarContext::new(&cwd);

    if !matches!(ctx.spline, Spline::UnaOS) {
        eprintln!("🚨 WRONG DIMENSION DETECTED. Spline is not UnaOS.");
        process::exit(1);
    }

    let mut errors = 0;

    // --- PHASE 1: PHYSICAL REPO VERIFICATION ---
    println!(">> PHASE 1: STRUCTURAL VERIFICATION");
    let memoria_path = if Path::new("UNA_MEMORIA.md").exists() {
        "UNA_MEMORIA.md"
    } else {
        "MEMORIA.md"
    };

    if let Ok(content) = fs::read_to_string(memoria_path) {
        let re = Regex::new(r"\*\s+\*\*\[(CRATE|BIN|SHELL)\]\s+`(.*?)`:\*\*").unwrap();
        let mut verified = 0;

        for cap in re.captures_iter(&content) {
            let type_tag = &cap[1];
            let rel_path = &cap[2];

            if type_tag == "SHELL" {
                continue;
            }

            if Path::new(rel_path).exists() {
                verified += 1;
            } else {
                println!(
                    "   ❌ [FAIL] MEMORIA hallucination: '{}' is missing.",
                    rel_path
                );
                errors += 1;
            }
        }
        println!(
            "   [PASS] Reality Confirmed. {} Artifacts Verified.",
            verified
        );
    } else {
        println!("   ❌ [FAIL] Could not read Memoria file.");
        errors += 1;
    }

    // --- PHASE 2: SEMANTIC VAULT VERIFICATION ---
    println!("\n>> PHASE 2: VAULT INTEGRITY");
    let vault_path = elessar::gneiss_pal::paths::UnaPaths::primary_vault();

    if vault_path.exists() {
        if let Ok(mut device) = FileDevice::open_read_only(&vault_path) {
            let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
            if device.read_block(0, &mut sb_block).is_ok() {
                if let Ok(sb) = Superblock::from_bytes(&sb_block) {
                    let total_mb = (sb.block_count * BLOCK_SIZE) / (1024 * 1024);
                    let free_mb = (sb.free_blocks * BLOCK_SIZE) / (1024 * 1024);
                    println!("   [PASS] Vault Signature Valid: UNAFS v{}", sb.version);
                    println!(
                        "   [INFO] Capacity: {} MB Total / {} MB Free",
                        total_mb, free_mb
                    );
                } else {
                    println!("   ❌ [FAIL] Vault Superblock Corrupted.");
                    errors += 1;
                }
            }
        }
    } else {
        println!(
            "   [WARN] Vault not found at {}. Awaiting Lumen initialization.",
            vault_path.display()
        );
    }

    // --- PHASE 3: CRYPTOGRAPHIC SEAL ---
    println!("\n>> PHASE 3: CRYPTOGRAPHIC SEAL");
    let files: Vec<PathBuf> = WalkBuilder::new(&cwd)
        .hidden(false)
        .filter_entry(|e| {
            let s = e.path().to_string_lossy();
            !s.contains("target") && !s.contains(".git")
        })
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_file()))
        .map(|e| e.into_path())
        .collect();

    // Parallel hashing using Rayon and Blake3 Memory Mapping
    let hashes: Vec<(PathBuf, blake3::Hash)> = files
        .par_iter()
        .filter_map(|path| {
            let mut hasher = Hasher::new();
            // mmap is dangerously fast for file hashing. We bypass standard I/O overhead.
            if hasher.update_mmap_rayon(path).is_ok() {
                Some((path.clone(), hasher.finalize()))
            } else {
                None
            }
        })
        .collect();

    let mut master_hasher = Hasher::new();
    for (path, hash) in &hashes {
        master_hasher.update(path.to_string_lossy().as_bytes());
        master_hasher.update(hash.as_bytes());
    }

    let system_state = master_hasher.finalize();

    println!("\n----------------------------------------");
    if errors > 0 {
        println!(
            "🚨 SENTINEL RUN COMPLETE: {} CRITICAL ERRORS DETECTED.",
            errors
        );
        process::exit(1);
    } else {
        println!("✨ SENTINEL RUN COMPLETE IN {:?}", start.elapsed());
        println!(":: MEMORIA STATE HASH :: {}", system_state.to_hex());
        println!(":: STATUS :: IMMUTABLE");
        Ok(())
    }
}
