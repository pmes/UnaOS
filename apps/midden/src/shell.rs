use crate::pile::Pile;
use crate::indexer::Indexer;
use std::env;
use std::fs::File;
use std::io::{self, Write, BufWriter};

// CHANGE 1: Accept mutable reference
pub fn run(pile: &mut Pile) {
    println!("Welcome to Midden (Host Mode).");
    if pile.entries.is_empty() {
        println!("(Tip: Type 'index' to build the Knowledge Pile)");
    } else {
        println!("Knowledge Pile: {} entries loaded.", pile.entries.len());
    }

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        // Safe EOF handling: read_line returns Ok(0) on EOF
        if io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
            break;
        }
        let command = input.trim();

        match command {
            "exit" => break,
            "help" => println!("Commands: index, list, find <name>, tags, filter <tag>, exit"),

            // CHANGE 2: The Index Command
            "index" => {
                println!("Indexing current strata...");
                match env::current_dir() {
                    Ok(cwd) => {
                         match Indexer::scan(&cwd) {
                             Ok(new_pile) => {
                                 // Update Memory
                                 *pile = new_pile;
                                 println!("Knowledge Pile updated: {} entries.", pile.entries.len());

                                 // Persist to Disk
                                 match File::create("midden_pile.json") {
                                     Ok(file) => {
                                         let writer = BufWriter::new(file);
                                         if let Err(e) = serde_json::to_writer_pretty(writer, &pile) {
                                             eprintln!("[ERROR] Failed to save memory to disk: {}", e);
                                         } else {
                                             println!("Memory saved to 'midden_pile.json'.");
                                         }
                                     },
                                     Err(e) => eprintln!("[ERROR] Failed to open file for writing: {}", e),
                                 }
                             },
                             Err(e) => eprintln!("[ERROR] Indexing failed: {}", e),
                         }
                    },
                    Err(e) => eprintln!("[ERROR] Could not determine current directory: {}", e),
                }
            },

            // Existing commands...
            "list" => {
                for entry in pile.entries.iter().take(5) {
                    println!(" - {:?}", entry.path);
                }
            },
            cmd if cmd.starts_with("find ") => {
                let term = cmd.strip_prefix("find ").unwrap();
                let mut found = false;
                for entry in pile.entries.iter() {
                    if entry.path.to_string_lossy().contains(term) {
                        println!("Found: {:?}", entry.path);
                        found = true;
                    }
                }
                if !found { println!("The Pile is silent on that matter."); }
            },
            "tags" => {
                // List all unique tags (simple aggregation)
                let mut all_tags: Vec<String> = pile.entries.iter()
                    .flat_map(|e| e.tags.clone())
                    .collect();
                all_tags.sort();
                all_tags.dedup();
                println!("Known Tags: {:?}", all_tags);
            },
            cmd if cmd.starts_with("filter ") => {
                let tag = cmd.strip_prefix("filter ").unwrap();
                let mut count = 0;
                for entry in pile.entries.iter() {
                    if entry.tags.iter().any(|t| t == tag) {
                        println!(" - {:?}", entry.path);
                        count += 1;
                    }
                }
                if count == 0 {
                    println!("No entries found with tag '{}'.", tag);
                } else {
                    println!("Found {} items.", count);
                }
            },
            "" => {}, // Ignore empty enter keys
            _ => println!("Unknown command."),
        }
    }
}
