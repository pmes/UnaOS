use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use memchr::memmem;
use memmap2::MmapOptions;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Instant;

/// AMBER BYTES: The Preserver of State
#[derive(Parser)]
#[command(name = "Amber Bytes")]
#[command(version = "0.2.0")]
#[command(about = "Forensic data recovery and preservation suite")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Safely inspect the first 128 bytes of a target (Read-Only)
    Inspect {
        /// The target file or block device
        #[arg(required = true)]
        target: PathBuf,
    },
    /// Create a bit-perfect forensic image with hash verification
    Image {
        /// The source drive or file (Read-Only)
        #[arg(long, required = true)]
        source: PathBuf,

        /// The destination image file
        #[arg(long, required = true)]
        dest: PathBuf,

        /// Block size in bytes (Default: 1MB)
        #[arg(long, default_value_t = 1_048_576)]
        block_size: usize,
    },
    /// Locate a specific byte pattern within the target
    Search {
        /// The target file or drive
        #[arg(required = true)]
        target: PathBuf,

        /// Text string to find (e.g., "password")
        #[arg(long, conflicts_with = "hex_pattern")]
        text: Option<String>,

        /// Hex pattern to find (e.g., "CA FE BA BE")
        #[arg(long, conflicts_with = "text")]
        hex_pattern: Option<String>,

        /// Maximum number of matches to display
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Surgically extract a byte range to a file
    Extract {
        /// The target file or drive
        #[arg(required = true)]
        target: PathBuf,

        /// Start offset (Decimal or Hex '0x...')
        #[arg(long, required = true)]
        offset: String,

        /// Number of bytes to extract
        #[arg(long, required = true)]
        length: usize,

        /// Output file path
        #[arg(long, required = true)]
        out: PathBuf,
    },
    /// Destructively sanitizes a target by overwriting
    Wipe {
        /// The target file or drive
        #[arg(required = true)]
        target: PathBuf,

        /// Wipe method: 'zeros' or 'random'
        #[arg(long, default_value = "zeros")]
        method: String,

        /// Number of passes (Default: 1)
        #[arg(long, default_value_t = 1)]
        passes: usize,

        /// Safety catch: Must be present to execute
        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inspect { target } => inspect_target(target),
        Commands::Image {
            source,
            dest,
            block_size,
        } => image_drive(source, dest, block_size),
        Commands::Search {
            target,
            text,
            hex_pattern,
            limit,
        } => search_target(target, text, hex_pattern, limit),
        Commands::Extract {
            target,
            offset,
            length,
            out,
        } => extract_data(target, offset, length, out),
        Commands::Wipe {
            target,
            method,
            passes,
            force,
        } => wipe_target(target, method, passes, force),
    }
}

// --- COMMAND 1: THE LENS (Old Logic) ---
fn inspect_target(target: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("--------------------------------------------------");
    println!("AMBER LENS ACTIVATED: Inspecting {:?}", target);
    println!("--------------------------------------------------");

    // SAFETY CRITICAL: Open in Read-Only mode.
    // We never open with write permissions in the inspection phase.
    let file =
        File::open(&target).map_err(|e| format!("Failed to open target '{:?}': {}", target, e))?;

    // Map the file safely into virtual memory.
    // This allows us to handle massive files (GBs/TBs) without loading them into RAM.
    let mmap = unsafe { MmapOptions::new().map(&file)? };

    // Determine preview size (Header check)
    let total_size = mmap.len();
    let preview_len = std::cmp::min(128, total_size);
    let preview = &mmap[0..preview_len];

    println!("--- [HEADER DUMP: First {} bytes] ---", preview_len);

    // Iterate in 16-byte chunks for standard hex viewing
    for (i, chunk) in preview.chunks(16).enumerate() {
        // 1. Offset
        print!("{:08x}  ", i * 16);

        // 2. Hex Bytes
        for byte in chunk {
            print!("{:02x} ", byte);
        }

        // Padding if the chunk is incomplete
        if chunk.len() < 16 {
            for _ in 0..(16 - chunk.len()) {
                print!("   ");
            }
        }

        print!(" |");

        // 3. ASCII Representation
        for byte in chunk {
            let c = *byte as char;
            // Only print printable characters; use dot for control/binary
            if c.is_ascii_graphic() || c == ' ' {
                print!("{}", c);
            } else {
                print!(".");
            }
        }
        println!("|");
    }

    println!("--------------------------------------------------");
    println!("STATUS: Target Encased.");
    println!("TOTAL SIZE: {} bytes", total_size);
    println!("--------------------------------------------------");

    Ok(())
}

// --- COMMAND 2: THE REPLICATOR (New Logic) ---
fn image_drive(
    source: PathBuf,
    dest: PathBuf,
    block_size: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("--- AMBER REPLICATOR INITIATED ---");
    println!("SOURCE: {:?}", source);
    println!("DEST:   {:?}", dest);

    // 1. Open Source (Read-Only)
    let input_file = File::open(&source)?;
    let file_size = input_file.metadata()?.len();
    let mut reader = BufReader::new(input_file);

    // 2. Open Destination (Create/Write)
    let output_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true) // Be careful: This overwrites!
        .open(&dest)?;
    let mut writer = BufWriter::new(output_file);

    // 3. Setup Progress Bar
    let pb = ProgressBar::new(file_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
        .progress_chars("#>-"));

    // 4. Setup Hasher (SHA-256)
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; block_size];
    let start_time = Instant::now();

    // 5. The Copy Loop
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        } // EOF

        // A. Feed the Hasher
        hasher.update(&buffer[0..bytes_read]);

        // B. Write to Disk
        writer.write_all(&buffer[0..bytes_read])?;

        // C. Update UI
        pb.inc(bytes_read as u64);
    }

    writer.flush()?;
    pb.finish_with_message("Replication Complete");

    // 6. Final Report
    let result = hasher.finalize();
    let duration = start_time.elapsed();

    println!("\n--- FORENSIC REPORT ---");
    println!("Time Elapsed: {:.2?}", duration);
    println!("Bytes Copied: {}", file_size);
    println!("SHA-256 Hash: {:x}", result);
    println!("-----------------------");

    Ok(())
}

// --- COMMAND 3: THE SEEKER (New Logic) ---
fn search_target(
    target: PathBuf,
    text: Option<String>,
    hex: Option<String>,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("--- AMBER SEEKER ACTIVATED ---");

    // 1. Prepare the Needle
    let needle = if let Some(t) = text {
        t.into_bytes()
    } else if let Some(h) = hex {
        // Remove spaces and decode hex
        let clean_hex = h.replace(" ", "");
        hex::decode(&clean_hex).map_err(|_| "Invalid hex string")?
    } else {
        return Err("Must provide either --text or --hex-pattern".into());
    };

    println!("Target: {:?}", target);
    println!("Seeking Pattern ({} bytes): {:02x?}", needle.len(), needle);

    // 2. Map the Haystack
    let file = File::open(&target)?;
    let mmap = unsafe { MmapOptions::new().map(&file)? };

    // 3. The High-Speed Scan (Memchr)
    let finder = memmem::Finder::new(&needle);
    let mut match_count = 0;

    println!("--------------------------------------------------");

    // Iterate over every occurrence
    for index in finder.find_iter(&mmap) {
        match_count += 1;

        // Calculate Context Window (32 bytes before/after)
        let start = index.saturating_sub(32);
        let end = std::cmp::min(index + needle.len() + 32, mmap.len());
        let context_slice = &mmap[start..end];

        // Print Match Report
        println!("MATCH #{} at Offset: 0x{:08x}", match_count, index);

        // Visual Context Dump
        print!("   CONTEXT: ");
        for (i, byte) in context_slice.iter().enumerate() {
            // Highlight the needle in the output
            let rel_pos = start + i;
            if rel_pos >= index && rel_pos < index + needle.len() {
                // Colorize or Bracket the match
                print!("[{:02x}] ", byte);
            } else {
                print!("{:02x} ", byte);
            }
        }
        println!("\n");

        if match_count >= limit {
            println!("--- Limit reached ({} matches) ---", limit);
            break;
        }
    }

    if match_count == 0 {
        println!("No matches found.");
    }

    Ok(())
}

// --- COMMAND 4: THE SCALPEL (New Logic) ---
fn extract_data(
    target: PathBuf,
    offset_str: String,
    length: usize,
    out_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("--- AMBER SCALPEL ACTIVATED ---");

    // 1. Parse the Offset
    let offset = parse_offset(&offset_str).map_err(|_| "Invalid offset format")?;

    println!("Target:   {:?}", target);
    println!("Offset:   0x{:x} ({})", offset, offset);
    println!("Length:   {} bytes", length);
    println!("Output:   {:?}", out_path);

    // 2. Open Source (Read-Only)
    let mut source = File::open(&target)?;
    let file_len = source.metadata()?.len();

    // 3. Safety Check
    if offset + (length as u64) > file_len {
        return Err(format!("Extraction range exceeds file size (Size: {})", file_len).into());
    }

    // 4. Perform the Surgery
    source.seek(SeekFrom::Start(offset))?;

    let mut buffer = vec![0u8; length];
    source.read_exact(&mut buffer)?;

    // 5. Save the Evidence
    let mut dest = File::create(&out_path)?;
    dest.write_all(&buffer)?;

    println!("--------------------------------------------------");
    println!("STATUS: Extraction Complete.");
    println!("EVIDENCE SECURED: {:?}", out_path);
    println!("--------------------------------------------------");

    Ok(())
}

fn parse_offset(s: &str) -> Result<u64, std::num::ParseIntError> {
    if s.trim().to_lowercase().starts_with("0x") {
        u64::from_str_radix(s.trim().trim_start_matches("0x"), 16)
    } else {
        s.parse::<u64>()
    }
}

// --- COMMAND 5: THE ERASER (New Logic) ---
fn wipe_target(
    target: PathBuf,
    method: String,
    passes: usize,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("--- AMBER ERASER INITIATED ---");

    // 1. Safety Check
    if !force {
        return Err("SAFETY INTERLOCK ACTIVE. You must use --force to destroy data.".into());
    }

    println!("Target: {:?}", target);
    println!("Method: {} ({} passes)", method, passes);

    // 2. Open Target (Write Mode)
    let mut file = OpenOptions::new().write(true).open(&target)?;
    let file_len = file.metadata()?.len();

    // 3. Prepare Progress Bar
    let pb = ProgressBar::new(file_len * passes as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.red} [{elapsed_precise}] [{bar:40.red/black}] {bytes}/{total_bytes} ({eta})")?
        .progress_chars("#>-"));

    // 4. The Loop
    let chunk_size = 1_048_576; // 1MB chunks
    let mut buffer = vec![0u8; chunk_size];

    for _pass in 1..=passes {
        // Reset to start
        file.seek(SeekFrom::Start(0))?;
        let mut bytes_written = 0u64;

        while bytes_written < file_len {
            // Fill Buffer
            if method == "random" {
                rand::thread_rng().fill(&mut buffer[..]);
            } else {
                // "zeros" - buffer is already zeroed, but ensure clean if reused
                buffer.fill(0);
            }

            // Calculate remaining bytes (don't overshoot file size)
            let remaining = file_len - bytes_written;
            let to_write = std::cmp::min(remaining, chunk_size as u64) as usize;

            // Write
            file.write_all(&buffer[0..to_write])?;
            bytes_written += to_write as u64;
            pb.inc(to_write as u64);
        }

        // Ensure data hits the platter
        file.sync_all()?;
    }

    pb.finish_with_message("Sanitization Complete");
    println!("--------------------------------------------------");
    println!("STATUS: Target Neutralized.");
    println!("--------------------------------------------------");

    Ok(())
}
