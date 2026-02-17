use std::fs;
use std::path::Path;
use std::process::Command;

const SHARDS: &[&str] = &[
    "unaos",
    "stria",
    "gneiss_pal",
    "midden",
    "amber_bytes",
    "vug",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    println!(":: VAIRË HAMMER INITIALIZED :: Mode: [{}]", command);

    match command {
        "status" => execute_on_all("git", &["status", "-s", "-b"]),
        "sync" => {
            execute_on_all("git", &["fetch", "--all"]);
            execute_on_all("git", &["pull"]);
        }
        "snap" => println!(":: Vairë :: Snapshot logic pending..."),
        _ => println!(":: Vairë :: Unknown directive. Usage: vairë [status|sync|snap]"),
    }
}

fn execute_on_all(cmd: &str, args: &[&str]) {
    let root = std::env::current_dir().unwrap();

    for shard in SHARDS {
        let shard_path = root.join(shard);
        if shard_path.exists() {
            println!("\n--- [ {} ] ---", shard.to_uppercase());

            // Check if it's actually a git repo before hammering
            if !shard_path.join(".git").exists() {
                println!("(Not a git repository, skipping)");
                continue;
            }

            let output = Command::new(cmd)
                .args(args)
                .current_dir(&shard_path)
                .output()
                .expect("Failed to execute command");

            if !output.stdout.is_empty() {
                println!("{}", String::from_utf8_lossy(&output.stdout).trim());
            }
            if !output.stderr.is_empty() {
                println!("ERR: {}", String::from_utf8_lossy(&output.stderr).trim());
            }
        } else {
            println!("\n--- [ {} ] MISSING ---", shard.to_uppercase());
        }
    }
}
