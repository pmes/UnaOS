// src/brain.rs
use colored::*;

pub struct Guardian {
    // In the future, this holds the loaded Candle/Phi-3 model
    enabled: bool,
}

impl Guardian {
    pub fn init() -> Self {
        Self { enabled: true }
    }

    /// The Core Loop: Analyze a command before it runs
    pub fn assess_danger(&self, command: &[String]) -> bool {
        if command.is_empty() {
            return false;
        }

        let cmd_str = command.join(" ");

        // STATIC HEURISTICS (Fast Check)
        // Before we even wake up the AI, check for obvious nukes
        if cmd_str.contains("git reset --hard") || cmd_str.contains("rm -rf") {
            return true; // DANGER DETECTED
        }

        // AI INFERENCE (Deep Check)
        // TODO: Pass 'cmd_str' to Phi-3 Mini here
        // let sentiment = self.model.predict(cmd_str);

        false
    }

    pub fn intervene(&self, command: &[String]) {
        println!("\n{}", "🛑 MIDDEN SAFETY INTERVENTION 🛑".red().bold());
        println!("You are about to execute: '{}'", command.join(" ").yellow());
        println!(
            "{}",
            "My analysis suggests this will destroy uncommitted work.".white()
        );
        println!("Type 'OVERRIDE' to proceed, or anything else to abort:");

        // 1. Read Input
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");

        // 2. Check the Code
        if input.trim() == "OVERRIDE" {
            println!(
                "{}",
                "🔓 SECURITY OVERRIDE ACCEPTED. EXECUTING...".green().bold()
            );

            // 3. Execute the dangerous command
            let output = std::process::Command::new(&command[0])
                .args(&command[1..])
                .output()
                .expect("Failed to execute command");

            println!("{}", String::from_utf8_lossy(&output.stdout));
            println!("{}", String::from_utf8_lossy(&output.stderr));
        } else {
            println!("{}", "🛡️ Action Aborted. Stay safe.".blue());
        }
    }
}
