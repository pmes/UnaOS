use anyhow::{Context, Result};
use directories::BaseDirs;
use elessar::{Context as ElessarContext, Spline};
use regex::Regex;
use std::env;
use std::fs;
use std::path::Path;
use std::process;
use unafs::{BLOCK_SIZE, BlockDevice, FileDevice, Superblock};

const MEMORIA_FILENAME: &str = "MEMORIA.md";

fn main() -> Result<()> {
    println!("🛡️  SENTINEL: SYSTEMS ONLINE.\n");

    // --- PHASE 1: PHYSICAL REPO VERIFICATION ---
    println!(">> INITIATING PHASE 1: PHYSICAL REPO VERIFICATION");
    let cwd = env::current_dir()?;
    let ctx = ElessarContext::new(&cwd);

    match ctx.spline {
        Spline::UnaOS => {
            println!("   [PASS] Context Confirmed: Spline [UnaOS]");
        }
        _ => {
            eprintln!("🚨 WRONG DIMENSION DETECTED.");
            process::exit(1);
        }
    }

    let content = fs::read_to_string(MEMORIA_FILENAME)
        .with_context(|| format!("Could not read {}", MEMORIA_FILENAME))?;

    // Updated Regex to catch SHELL (Design-only) tags
    let re = Regex::new(r"\*\s+\*\*\[(CRATE|BIN|SHELL)\]\s+`(.*?)`:\*\*").unwrap();
    let mut errors = 0;
    let mut verified = 0;

    for cap in re.captures_iter(&content) {
        let type_tag = &cap[1];
        let rel_path = &cap[2];
        let path = Path::new(rel_path);

        if type_tag == "SHELL" {
            continue; // Skip shells, they are theoretical/design-only
        }

        if path.exists() {
            verified += 1;
        } else {
            println!(
                "   ❌ [FAIL] MEMORIA claims '{}' exists, but it does not!",
                rel_path
            );
            errors += 1;
        }
    }

    if errors > 0 {
        println!("🚨 HALLUCINATION DETECTED. {} MISSING ARTIFACTS.", errors);
        process::exit(1);
    } else {
        println!(
            "   [PASS] Reality Confirmed. {} Artifacts Verified.\n",
            verified
        );
    }

    // --- PHASE 2: SEMANTIC VAULT VERIFICATION ---
    println!(">> INITIATING PHASE 2: SEMANTIC VAULT VERIFICATION");

    // Ask the Plexus for the absolute truth
    let vault_path = elessar::gneiss_pal::paths::UnaPaths::lumen_storage();

    if vault_path.exists() {
        // Open in Read-Only mode so we don't corrupt Lumen's active session
        match FileDevice::open_read_only(&vault_path) {
            Ok(mut device) => {
                let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
                if device.read_block(0, &mut sb_block).is_ok() {
                    match Superblock::from_bytes(&sb_block) {
                        Ok(sb) => {
                            println!("   [PASS] Vault Located: {}", vault_path.display());
                            println!("   [PASS] Magic Signature Valid: UNAFS v{}", sb.version);

                            let total_mb = (sb.block_count * BLOCK_SIZE) / (1024 * 1024);
                            let free_mb = (sb.free_blocks * BLOCK_SIZE) / (1024 * 1024);

                            println!(
                                "   [INFO] Capacity: {} MB Total / {} MB Free",
                                total_mb, free_mb
                            );
                            println!(
                                "   [INFO] Root Inode: {} | Catalog Inode: {}",
                                sb.root_inode, sb.catalog_inode
                            );
                        }
                        Err(e) => {
                            println!("   ❌ [FAIL] Vault Superblock Corrupted: {}", e);
                            errors += 1;
                        }
                    }
                } else {
                    println!("   ❌ [FAIL] Could not read Block 0 of Vault.");
                    errors += 1;
                }
            }
            Err(e) => {
                println!("   ❌ [FAIL] Could not open Vault: {}", e);
                errors += 1;
            }
        }
    } else {
        println!(
            "   [WARN] Vault not found at {}. Lumen has not initialized it yet.",
            vault_path.display()
        );
    }

    println!("\n----------------------------------------");
    if errors > 0 {
        println!("🚨 SENTINEL RUN COMPLETE: CRITICAL ERRORS DETECTED.");
        process::exit(1);
    } else {
        println!("✨ SENTINEL RUN COMPLETE: ALL SYSTEMS NOMINAL.");
        Ok(())
    }
}
