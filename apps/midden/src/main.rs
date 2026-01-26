use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

// THE MIDDEN CRYSTAL PALETTE
const CRYSTAL_GREEN: &str = "\x1b[32m●\x1b[0m"; // Stable
const CRYSTAL_RED: &str = "\x1b[31m●\x1b[0m"; // Error
const CRYSTAL_AMBER: &str = "\x1b[33m●\x1b[0m"; // Warning
const CRYSTAL_BLUE: &str = "\x1b[34m●\x1b[0m"; // Moonstone/AI

fn main() {
    // 1. STATUS CHECK (The Crystal)
    // In a real shell, this runs after every command.
    // For now, we simulate the prompt.
    let last_exit_code = 0; // Simulate success
    let crystal = match last_exit_code {
        0 => CRYSTAL_GREEN,
        _ => CRYSTAL_RED,
    };

    print!("{} midden > ", crystal);

    // ... Input handling would go here ...

    // 2. CASE INSENSITIVITY LOGIC
    // Example: User types "cd unaos" but folder is "UnaOS" (if we rename root) or "stria" vs "Stria"
    let target = "UnaOS"; // Hypothetical user input
    if let Some(fixed_path) = resolve_path_insensitive(target) {
        println!("Midden: Correction applied -> {:?}", fixed_path);
    }
}

fn resolve_path_insensitive(input: &str) -> Option<PathBuf> {
    let path = Path::new(input);
    if path.exists() {
        return Some(path.to_path_buf());
    }

    // Smart Recovery: Scan current dir for case-insensitive match
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.eq_ignore_ascii_case(input) {
                return Some(entry.path());
            }
        }
    }
    None
}
