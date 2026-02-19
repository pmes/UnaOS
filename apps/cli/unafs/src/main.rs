use clap::{Parser, Subcommand};
use anyhow::{Context, Result};
use unafs::{FileSystem, FileDevice};
use bandy::{SMessage, BandyMember};

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
    },
    /// Inject a file from the host into the vault
    Put {
        source: String,
        destination: String,
    },
    /// Extract a file from the vault to the host
    Get {
        source: String,
        destination: String,
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
            fs.publish("system/fs/created", msg)?;
        }
        Commands::Ls { path } => {
            println!("📂 [OPERATOR] Listing '{}'...", path);
            // let fs = FileSystem::mount("unafs.img")?;
            // fs.ls(path)?;
        }
        Commands::Put { source, destination } => {
            println!("📥 [OPERATOR] Injecting '{}' -> '{}'", source, destination);
            // let fs = FileSystem::mount("unafs.img")?;
            // fs.write(source, destination)?;

            // NOTIFY THE NERVOUS SYSTEM
            // let msg = SMessage::FileEvent { path: destination.clone(), event: "Created".into() };
            // fs.publish("system/fs/change", msg)?;
        }
        Commands::Get { source, destination } => {
            println!("📤 [OPERATOR] Extracting '{}' -> '{}'", source, destination);
        }
    }

    Ok(())
}
