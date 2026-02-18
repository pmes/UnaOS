use clap::{Parser, Subcommand};
use gneiss_pal::paths;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use unafs::{UnaFS, FileDevice, FileKind};

#[derive(Parser)]
#[command(name = "unafs")]
#[command(about = "UnaFS CLI Tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new 10GB sparse filesystem
    Init,
    /// List files in the image
    Ls {
        #[arg(default_value = "/")]
        path: String,
    },
    /// Copy file from Host to UnaFS
    Put {
        source: String,
        dest: String,
    },
    /// Copy file from UnaFS to Host
    Get {
        source: String,
        dest: String,
    },
}

fn get_image_path() -> PathBuf {
    let mut path = paths::data_dir();
    path.push("file_systems");
    fs::create_dir_all(&path).expect("Failed to create data directory");
    path.push("unafs.img");
    path
}

fn resolve_path(unafs: &UnaFS<FileDevice>, root_id: u64, path: &str) -> Option<u64> {
    if path == "/" || path.is_empty() {
        return Some(root_id);
    }

    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current_id = root_id;

    for part in parts {
        let entries = unafs.ls(current_id).ok()?;
        if let Some(entry) = entries.iter().find(|e| e.name == part) {
            current_id = entry.inode_id;
        } else {
            return None;
        }
    }

    Some(current_id)
}

fn main() {
    let cli = Cli::parse();
    let image_path = get_image_path();

    match cli.command {
        Commands::Init => {
            println!("Initializing UnaFS at {:?}", image_path);
            if image_path.exists() {
                println!("Image already exists. Aborting to prevent overwrite.");
                return;
            }
            let file = File::create(&image_path).expect("Failed to create file");
            file.set_len(10 * 1024 * 1024 * 1024).expect("Failed to set file length"); // 10GB

            let device = FileDevice::open(&image_path).expect("Failed to open device");
            match UnaFS::format(device) {
                Ok(_) => println!("Successfully formatted UnaFS."),
                Err(e) => eprintln!("Failed to format: {:?}", e),
            }
        }
        Commands::Ls { path } => {
            let device = FileDevice::open(&image_path).expect("Failed to open device");
            let unafs = UnaFS::mount(device).expect("Failed to mount UnaFS");

            let root_id = unafs.superblock.root_inode;
            let target_id = resolve_path(&unafs, root_id, &path).expect("Path not found");

            match unafs.ls(target_id) {
                Ok(entries) => {
                    if entries.is_empty() {
                        println!("(empty)");
                    } else {
                        for entry in entries {
                            let kind_str = match entry.kind {
                                FileKind::File => "FILE",
                                FileKind::Directory => "DIR ",
                                FileKind::Symlink => "LINK",
                            };
                            println!("{}  {}", kind_str, entry.name);
                        }
                    }
                }
                Err(e) => eprintln!("Error listing directory: {:?}", e),
            }
        }
        Commands::Put { source, dest } => {
             let device = FileDevice::open(&image_path).expect("Failed to open device");
             let mut unafs = UnaFS::mount(device).expect("Failed to mount UnaFS");

             let root_id = unafs.superblock.root_inode;

             // 1. Read source file
             let mut src_file = File::open(&source).expect("Failed to open source file");
             let mut data = Vec::new();
             src_file.read_to_end(&mut data).expect("Failed to read source file");

             // 2. Resolve destination parent and filename
             let dest_path = Path::new(&dest);
             let file_name = dest_path.file_name().expect("Invalid destination").to_string_lossy().to_string();
             let parent_path_str = dest_path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or("/".to_string());

             // For now, assume parent exists or is root
             let parent_id = resolve_path(&unafs, root_id, &parent_path_str).expect("Destination parent path not found");

             // 3. Create file or Overwrite
             let inode_id = match unafs.create_file(parent_id, file_name.clone()) {
                 Ok(id) => id,
                 Err(unafs::fs::FileSystemError::FileExists) => {
                     // Resolve the existing file
                     let mut path_to_file = parent_path_str.clone();
                     if !path_to_file.ends_with('/') {
                         path_to_file.push('/');
                     }
                     path_to_file.push_str(&file_name);

                     match resolve_path(&unafs, root_id, &path_to_file) {
                         Some(id) => {
                             println!("File exists. Overwriting...");
                             id
                         },
                         None => {
                             eprintln!("File reported as existing but could not be resolved.");
                             return;
                         }
                     }
                 }
                 Err(e) => {
                     eprintln!("Failed to create file: {:?}", e);
                     return;
                 }
             };

             // 4. Write data
             if let Err(e) = unafs.write_data(inode_id, 0, &data) {
                 eprintln!("Failed to write data: {:?}", e);
             } else {
                 println!("Wrote {} bytes to {}", data.len(), dest);
             }
        }
        Commands::Get { source, dest } => {
            let device = FileDevice::open(&image_path).expect("Failed to open device");
            let unafs = UnaFS::mount(device).expect("Failed to mount UnaFS");

            let root_id = unafs.superblock.root_inode;
            let inode_id = resolve_path(&unafs, root_id, &source).expect("Source file not found");

            let inode = unafs.read_inode(inode_id).expect("Failed to read inode");
            let data = unafs.read_data(inode_id, 0, inode.size).expect("Failed to read data");

            let mut dest_file = File::create(&dest).expect("Failed to create destination file");
            dest_file.write_all(&data).expect("Failed to write destination file");
            println!("Extracted {} bytes to {}", data.len(), dest);
        }
    }
}
