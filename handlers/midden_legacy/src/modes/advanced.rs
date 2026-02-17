// src/modes/advanced.rs
use crate::brain::Guardian;
use anyhow::Result;
use std::process::Command;

pub fn launch(guardian: Guardian, args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        // Start Interactive Shell Loop (REPL)
        println!("(Entering Advanced REPL... type 'exit' to quit)");
        return Ok(());
    }

    // 1. Check with the Guardian
    if guardian.assess_danger(&args) {
        guardian.intervene(&args);
        return Ok(()); // Stop execution for demo purposes
    }

    // 2. If safe, pass through to the OS
    // This makes Midden transparent!
    let output = Command::new(&args[0]).args(&args[1..]).output()?;

    println!("{}", String::from_utf8_lossy(&output.stdout));

    Ok(())
}
