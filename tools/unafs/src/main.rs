// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Context, Result};
use bandy::{BandyMember, SMessage};
use clap::{Parser, Subcommand};
use std::path::Path;
use unafs::{FileDevice, FileSystem, parse_value};

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
    /// Remove a file from the vault (frees its blocks and scrubs its catalog entries)
    Rm {
        path: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Rename or move a file/directory. If DESTINATION is an existing
    /// directory, SOURCE moves into it keeping its name; otherwise
    /// DESTINATION names the new parent + new name. Refuses to overwrite.
    Mv {
        source: String,
        destination: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Remove a semantic attribute (and its query-index entries)
    Rmattr {
        path: String,
        key: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Scan for crash-window residue — leaked blocks and query-orphaned inodes.
    /// With --repair, reclaim them and clear a dirty journal (dirty-mount
    /// recovery).
    Fsck {
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
        /// Actually reclaim leaks, heal orphans, and reset a dirty journal.
        #[arg(long)]
        repair: bool,
    },
}

/// Split a vault path into (parent path, entry name). "/a/b/c" -> ("/a/b", "c").
fn split_parent(path: &str) -> Result<(&str, &str)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("'{}' has no parent (is it the root?)", path);
    }
    match trimmed.rfind('/') {
        Some(idx) => Ok((&trimmed[..idx.max(1)], &trimmed[idx + 1..])),
        None => Ok(("/", trimmed)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { path, size_mb } => {
            println!(
                "⚡ [OPERATOR] Initializing Vault at '{}' ({} MB)...",
                path, size_mb
            );

            // Pre-allocate file
            let file = std::fs::File::create(path).context("Failed to create file")?;
            file.set_len(size_mb * 1024 * 1024)
                .context("Failed to set file size")?;

            // Open as block device
            let device = FileDevice::open(path).context("Failed to open device")?;
            let fs = FileSystem::format(device, *size_mb).context("Failed to format filesystem")?;

            // Notify
            let msg = SMessage::FileEvent {
                path: path.clone(),
                event: "Created".into(),
            };
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
        Commands::Put {
            source,
            destination,
            img,
        } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let parent_id = fs
                .resolve_path(destination)
                .context("Destination directory not found")?;

            let src_path = Path::new(source);
            let file_name = src_path
                .file_name()
                .context("Invalid source filename")?
                .to_string_lossy()
                .to_string();
            let data = std::fs::read(source).context("Failed to read source file")?;

            let file_id = fs
                .create_file(parent_id, file_name.clone())
                .context("Failed to create file")?;
            fs.write_data(file_id, 0, &data)
                .context("Failed to write data")?;

            println!(
                "✅ [OPERATOR] Wrote '{}' to '{}/{}' (ID: {})",
                source, destination, file_name, file_id
            );
        }
        Commands::Get {
            source,
            destination,
            img,
        } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(source).context("Source file not found")?;
            let inode = fs.read_inode(id).context("Failed to read inode")?;

            let data = fs
                .read_data(id, 0, inode.size)
                .context("Failed to read data")?;
            std::fs::write(destination, data).context("Failed to write destination file")?;

            println!("✅ [OPERATOR] Extracted '{}' to '{}'", source, destination);
        }
        Commands::AttrSet {
            path,
            key,
            value,
            img,
        } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            let val = parse_value(value).map_err(|e| anyhow::anyhow!(e))?;

            fs.set_attribute(id, key.clone(), val)
                .context("Failed to set attribute")?;
            println!("✅ [OPERATOR] Set attribute '{}' on '{}'", key, path);
        }
        Commands::AttrGet { path, key, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            if let Some(val) = fs
                .get_attribute(id, key)
                .context("Failed to get attribute")?
            {
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
            for (inode, score) in results {
                println!(
                    "  Inode {} (Size: {} bytes) [Score: {:.4}]",
                    inode.id, inode.size, score
                );
            }
        }
        Commands::Rm { path, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let (parent, name) = split_parent(path)?;
            let parent_id = fs
                .resolve_path(parent)
                .context("Parent directory not found")?;
            let inode_id = fs
                .unlink(parent_id, name)
                .map_err(|e| anyhow::anyhow!("Failed to remove '{}': {:?}", path, e))?;

            println!("✅ [OPERATOR] Removed '{}' (was inode {})", path, inode_id);
        }
        Commands::Mv {
            source,
            destination,
            img,
        } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let (src_parent, src_name) = split_parent(source)?;
            let src_parent_id = fs
                .resolve_path(src_parent)
                .context("Source parent directory not found")?;

            // If the destination resolves to an existing directory, move the
            // source into it under its own name; otherwise treat the
            // destination as parent + new name.
            let dst_dir_id = fs.resolve_path(destination).ok().filter(|&id| {
                matches!(fs.read_inode(id), Ok(i) if i.kind == unafs::FileKind::Directory)
            });
            let (dst_parent_id, dst_name) = match dst_dir_id {
                Some(id) => (id, src_name.to_string()),
                None => {
                    let (dst_parent, dst_name) = split_parent(destination)?;
                    let dst_parent_id = fs
                        .resolve_path(dst_parent)
                        .context("Destination parent directory not found")?;
                    (dst_parent_id, dst_name.to_string())
                }
            };

            fs.rename(src_parent_id, src_name, dst_parent_id, &dst_name)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to move '{}' -> '{}': {:?}", source, destination, e)
                })?;

            println!("✅ [OPERATOR] Moved '{}' -> '{}'", source, destination);
        }
        Commands::Rmattr { path, key, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let id = fs.resolve_path(path).context("Path not found")?;
            fs.remove_attribute(id, key).map_err(|e| {
                anyhow::anyhow!("Failed to remove attribute '{}' on '{}': {:?}", key, path, e)
            })?;

            println!("✅ [OPERATOR] Removed attribute '{}' from '{}'", key, path);
        }
        Commands::Fsck { img, repair } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            if *repair {
                let report = fs
                    .recover()
                    .map_err(|e| anyhow::anyhow!("fsck --repair failed: {:?}", e))?;
                println!("🔧 [FSCK] recovery complete on '{}'", img);
                println!("  dirty journal        : {}", report.dirty_journal);
                println!("  query-orphans healed : {}", report.orphan_inodes.len());
                println!("  catalog entries scrubbed: {}", report.scrubbed_catalog_entries);
                println!("  blocks reclaimed     : {}", report.reclaimed_blocks);
            } else {
                let dirty = fs
                    .is_dirty()
                    .map_err(|e| anyhow::anyhow!("fsck failed: {:?}", e))?;
                let report = fs
                    .fsck(false)
                    .map_err(|e| anyhow::anyhow!("fsck failed: {:?}", e))?;
                println!(
                    "🔍 [FSCK] scan of '{}' (dry run — pass --repair to heal)",
                    img
                );
                println!("  dirty journal        : {}", dirty);
                println!("  blocks in use        : {}", report.blocks_in_use);
                println!("  reachable blocks     : {}", report.reachable_blocks);
                println!("  leaked blocks        : {}", report.leaked_blocks.len());
                println!("  query-orphaned inodes: {}", report.orphan_inodes.len());
                if report.leaked_blocks.is_empty() && report.orphan_inodes.is_empty() && !dirty {
                    println!("  ✅ volume is clean");
                }
            }
        }
    }

    Ok(())
}
