use clap::{Parser, Subcommand};
use anyhow::{Context, Result};
use unafs::{FileSystem, FileDevice, parse_value};
use bandy::{SMessage, BandyMember};
use std::path::Path;

#[derive(Parser)]
#[command(name = "unafs")]
#[command(about = "The Operator Tool for the UnaOS Virtual Filesystem")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new unafs.img vault
    Init {
        #[arg(short, long, default_value = "unafs.img")]
        path: String,
        #[arg(short, long, default_value = "1024")]
        size_mb: u64,
    },
    /// List files inside the vault
    Ls {
        #[arg(short, long, default_value = "/")]
        path: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Inject a file from the host into the vault (destination must be a directory)
    Put {
        source: String,
        destination: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Extract a file from the vault to the host
    Get {
        source: String,
        destination: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Set a semantic attribute
    AttrSet {
        path: String,
        key: String,
        value: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Get a semantic attribute
    AttrGet {
        path: String,
        key: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Execute a semantic query
    Query {
        query: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { path, size_mb } => {
            println!("⚡ [OPERATOR] Initializing Vault at '{}' ({} MB)...", path, size_mb);

            // Pre-allocate file
            let file = std::fs::File::create(path).context("Failed to create file")?;
            file.set_len(size_mb * 1024 * 1024).context("Failed to set file size")?;

            // Open as block device
            let device = FileDevice::open(path).context("Failed to open device")?;
            let fs = FileSystem::format(device, *size_mb).context("Failed to format filesystem")?;

            // Notify
            let msg = SMessage::FileEvent { path: path.clone(), event: "Created".into() };
            // Since publish is fire-and-forget, we ignore errors or print warnings
            if let Err(e) = fs.publish("system/fs/created", msg) {
                eprintln!("Warning: Failed to publish event: {}", e);
            }
        }
        Commands::Ls { path, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            let entries = fs.ls(id).context("Failed to list directory")?;

            println!("Listing '{}':", path);
            for entry in entries {
                println!("  {:10} {}", format!("({:?})", entry.kind), entry.name);
            }
        }
        Commands::Put { source, destination, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let parent_id = fs.resolve_path(destination).context("Destination directory not found")?;

            let src_path = Path::new(source);
            let file_name = src_path.file_name().context("Invalid source filename")?.to_string_lossy().to_string();
            let data = std::fs::read(source).context("Failed to read source file")?;

            let file_id = fs.create_file(parent_id, file_name.clone()).context("Failed to create file")?;
            fs.write_data(file_id, 0, &data).context("Failed to write data")?;

            println!("✅ [OPERATOR] Wrote '{}' to '{}/{}' (ID: {})", source, destination, file_name, file_id);
        }
        Commands::Get { source, destination, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(source).context("Source file not found")?;
            let inode = fs.read_inode(id).context("Failed to read inode")?;

            let data = fs.read_data(id, 0, inode.size).context("Failed to read data")?;
            std::fs::write(destination, data).context("Failed to write destination file")?;

            println!("✅ [OPERATOR] Extracted '{}' to '{}'", source, destination);
        }
        Commands::AttrSet { path, key, value, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            let val = parse_value(value).map_err(|e| anyhow::anyhow!(e))?;

            fs.set_attribute(id, key.clone(), val).context("Failed to set attribute")?;
            println!("✅ [OPERATOR] Set attribute '{}' on '{}'", key, path);
        }
        Commands::AttrGet { path, key, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            if let Some(val) = fs.get_attribute(id, key).context("Failed to get attribute")? {
                println!("{:?}", val);
            } else {
                println!("(Attribute not found)");
            }
        }
        Commands::Query { query, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let results = fs.query(query).map_err(|e| anyhow::anyhow!(e))?;

            println!("Found {} results:", results.len());
            for inode in results {
                println!("  Inode {} (Size: {} bytes)", inode.id, inode.size);
            }
        }
    }

    Ok(())
}
