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
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use unafs::{AttributeValue, BatchFile, FileDevice, FileSystem, parse_value};

mod dump;

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
    /// Make a directory and every missing parent (`mkdir -p`); an existing directory is not an error.
    /// VOLUMES (B366): the builder stages `/system/test-f` onto the card's UnaFS root with it.
    Mkdir {
        path: String,
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
    /// Show an object's id, kind, size, parent and timestamps (v6; a pre-v6
    /// volume reports 0 for what it cannot store)
    Stat {
        path: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Execute a semantic query; prints `id path score` per hit.
    ///
    /// Grammar: `k == v`, `k != v`, `k > v`, `k < v`, `k >= v`, `k <= v`,
    /// `k BETWEEN a AND b`, `a <= k <= b`, `similarity(k, [..]) > t`, combined
    /// with AND / OR and parentheses. Strings are quoted; ordering compares
    /// Int/Float numerically. Example: 'size >= 10 AND (kind == "doc" OR kind == "memo")'
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
    /// Print the volume's on-disk records field by field, walked from the
    /// byte-level spec (docs/dev/OS/09_FILESYSTEM/unafs-records.md)
    Dump {
        /// Every record: superblock, root slots, inodes + trailers, overflow
        /// lists, large attribute values, directory lists, catalog, snapshot
        /// index, reclaim queue.
        #[arg(long)]
        records: bool,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Check refcount consistency (K8 CoW: recompute reachability and diff
    /// against the persisted refcount map). With --repair, rebuild the map
    /// and scrub stale catalog entries.
    Fsck {
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
        /// Actually rebuild the refcount map and scrub stale index entries.
        #[arg(long)]
        repair: bool,
    },
    /// Grow the volume in place to BLOCKS 4 KiB blocks (UNAFSGROW): the image
    /// file is extended first, then the refcount map, then block 0 (the
    /// superblock, last). Refuses a shrink and a size the map cannot address.
    Grow {
        img: String,
        blocks: u64,
    },
    /// UNAFSMAP (B354) map bench: for each size, a SPARSE image under
    /// OUT_DIR is formatted (`--version`: 7 = paged maps, 6 = legacy), given a
    /// file, remounted (timed; peak heap of the mount), overwritten by one
    /// block (blocks that commit wrote), and fsck'd (timed). `--files N` also
    /// creates N empty files (batched) and times their fsck. Prints one table
    /// row per size; the images are deleted.
    BenchMap {
        /// Directory for the throwaway sparse images.
        #[arg(long, default_value = ".")]
        out_dir: String,
        /// Volume sizes in MiB, comma-separated.
        #[arg(long, default_value = "1024,16384,1048576")]
        sizes: String,
        /// Format version (7 paged, 6 legacy whole-map).
        #[arg(long, default_value = "7")]
        version: u32,
        /// Empty files to create before the fsck (0 = none).
        #[arg(long, default_value = "0")]
        files: u64,
    },
    /// List retained snapshots (the on-disk snapshot index).
    Snaps {
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Retain the current committed tree as a snapshot (a retained root). The
    /// snapshot shares blocks with the live tree until they diverge.
    Snap {
        /// A human name for the snapshot (a K6 typed attribute on the entry).
        name: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Drop a retained snapshot by its generation stamp (see `snaps`). Frees
    /// only blocks no live/retained root still reaches (eager reclamation).
    Snapdrop {
        /// The snapshot's generation stamp.
        generation: u64,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// Read a file from a retained snapshot (K8c) under the LIVE object's
    /// CURRENT ACL — the host mirror of the kernel `snap cat` verb. The bytes
    /// are served AS OF the snapshot, but authority is re-evaluated against the
    /// live object of the same logical id: a principal that cannot read it live
    /// cannot read the snapshot, and a live-DELETED object fails closed.
    Snapcat {
        /// The snapshot's generation stamp (see `snaps`).
        generation: u64,
        /// The path within the snapshot (absolute, e.g. `/notes.txt`).
        path: String,
        /// The reading principal. `kernel` is authority (reads any LIVE object);
        /// otherwise the live object's `owner` / `grants:<principal>` decide.
        #[arg(long, default_value = "operator")]
        as_principal: String,
        #[arg(short, long, default_value = "unafs.img")]
        img: String,
    },
    /// UNAFS-BATCH before/after: sync a real directory tree TWO ways into
    /// fresh throwaway v3 images — the per-op regime (vaire's control: one
    /// root flip per file) and the bulk create+write path
    /// (`create_files_batch`, one flip for the whole tree) — and print the
    /// per-phase / flip-count / wall table for each, cold then warm. Isolates
    /// the single variable (per-op vs batch) on identical hardware and load.
    /// The two images are throwaway and use caller-supplied paths (never a
    /// shared/fixed bench image).
    BenchBatch {
        /// The source directory tree to sync (a real mixed-file load).
        source: String,
        /// Directory to write the two throwaway bench images into.
        #[arg(long, default_value = ".")]
        out_dir: String,
        /// Size of each throwaway image, MB.
        #[arg(long, default_value = "512")]
        size_mb: u64,
    },
    /// One-way migration of a pre-K8 (version 2) or a K8 v3–v5 volume into
    /// the current format (v6: B+tree catalog, parent pointers, timestamps):
    /// walks the old tree read-only and replays it (names, data, attributes)
    /// into a freshly formatted image.
    Migrate {
        /// The source image: pre-K8 (v2) or a K8 v3–v5 volume (opened
        /// read-only, never written) — replayed into a fresh current-format
        /// (v6, indexed catalog) image.
        from: String,
        /// The K8 target image (created/overwritten, freshly formatted).
        to: String,
        /// Target size in MB (default: sized like the source volume).
        #[arg(short, long)]
        size_mb: Option<u64>,
    },
}

// =============================================================================
// UNAFS-BATCH before/after harness (bench-batch)
// =============================================================================

/// One file to sync: its name, bytes, and the size/mtime the incremental
/// (warm) path keys on.
struct FilePlan {
    name: String,
    data: Vec<u8>,
    size: i64,
    mtime: i64,
    src: String,
}

/// One directory in the plan: its path components relative to the sync root
/// (empty == the root itself) and the files directly inside it. Parent-first
/// order is guaranteed by the scan so a directory's parent always exists first.
struct DirPlan {
    path: Vec<String>,
    files: Vec<FilePlan>,
}

/// Coarse per-phase wall (ms) for one sync run, plus the flip/block ledger.
#[derive(Default)]
struct RunReport {
    files: usize,
    dirs: usize,
    scan_ms: f64,
    build_ms: f64,
    commit_ms: f64,
    commits: u64,
    blocks: u64,
    wall_ms: f64,
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Walk `root` into a parent-first `Vec<DirPlan>`. Symlinks are not followed
/// (mirrors the vaire penumbra rule); this is a benchmark load, not a security
/// boundary, so no exclusion floor — the caller points it at a clean tree.
fn scan_tree(root: &Path) -> Result<Vec<DirPlan>> {
    let mut out: Vec<DirPlan> = Vec::new();
    // BFS keeps parents strictly before children.
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, Vec<String>)> =
        std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), Vec::new()));

    while let Some((host_dir, rel)) = queue.pop_front() {
        let mut files = Vec::new();
        let mut subdirs = Vec::new();
        for entry in std::fs::read_dir(&host_dir)
            .with_context(|| format!("read_dir {}", host_dir.display()))?
        {
            let entry = entry?;
            let ft = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if ft.is_symlink() {
                continue; // never followed
            }
            let path = entry.path();
            if ft.is_dir() {
                let mut child_rel = rel.clone();
                child_rel.push(name);
                subdirs.push((path, child_rel));
            } else if ft.is_file() {
                let meta = std::fs::metadata(&path)?;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let data = std::fs::read(&path)?;
                files.push(FilePlan {
                    name,
                    size: data.len() as i64,
                    mtime,
                    src: path.to_string_lossy().to_string(),
                    data,
                });
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        out.push(DirPlan {
            path: rel,
            files,
        });
        subdirs.sort_by(|a, b| a.1.cmp(&b.1));
        for s in subdirs {
            queue.push_back(s);
        }
    }
    Ok(out)
}

/// The four K6 typed attrs vaire attaches per file (the per-op cost the batch
/// path folds into one inode write).
fn file_attrs(f: &FilePlan, stamp: &str) -> Vec<(String, AttributeValue)> {
    vec![
        ("vaire.size".to_string(), AttributeValue::Int(f.size)),
        ("vaire.mtime".to_string(), AttributeValue::Int(f.mtime)),
        (
            "vaire.src".to_string(),
            AttributeValue::String(f.src.clone()),
        ),
        (
            "vaire.sync".to_string(),
            AttributeValue::String(stamp.to_string()),
        ),
    ]
}

/// Create the throwaway image file and format a fresh v3 filesystem on it.
fn fresh_image(path: &Path, size_mb: u64) -> Result<FileSystem> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("create bench image {}", path.display()))?;
    file.set_len(size_mb * 1024 * 1024)?;
    let device = FileDevice::open(path).context("open bench device")?;
    let fs = FileSystem::format(device, size_mb).context("format bench image")?;
    Ok(fs)
}

/// Resolve a directory plan's parent id from a path→id map (parent-first order
/// guarantees the parent is present).
fn dir_ids_for<'a>(plan: &'a DirPlan, ids: &BTreeMap<Vec<String>, u64>) -> (u64, &'a str) {
    if plan.path.is_empty() {
        return (0, ""); // handled specially by the caller (root)
    }
    let parent = &plan.path[..plan.path.len() - 1];
    let parent_id = *ids.get(parent).expect("parent created first");
    (parent_id, plan.path.last().unwrap())
}

/// COLD sync via the per-op regime (the control): every directory is its own
/// commit, and every file is create+write+4×set_attribute in ONE commit
/// (autocommit off + one explicit commit per file) — vaire's current 242-flip
/// regime.
fn sync_cold_perop(fs: &mut FileSystem, plan: &[DirPlan], stamp: &str) -> Result<RunReport> {
    let mut r = RunReport::default();
    let root_id = fs.superblock.root_inode;
    let mut ids: BTreeMap<Vec<String>, u64> = BTreeMap::new();
    ids.insert(Vec::new(), root_id);
    fs.set_autocommit(false);
    let wall = Instant::now();

    for dir in plan {
        let dir_id = if dir.path.is_empty() {
            root_id
        } else {
            let (parent_id, name) = dir_ids_for(dir, &ids);
            let t = Instant::now();
            let id = fs.mkdir(parent_id, name.to_string())?;
            r.build_ms += ms(t.elapsed());
            let t = Instant::now();
            fs.commit()?;
            r.commit_ms += ms(t.elapsed());
            r.dirs += 1;
            id
        };
        ids.insert(dir.path.clone(), dir_id);

        for f in &dir.files {
            let t = Instant::now();
            let fid = fs.create_file(dir_id, f.name.clone())?;
            if !f.data.is_empty() {
                fs.write_data(fid, 0, &f.data)?;
            }
            for (k, v) in file_attrs(f, stamp) {
                fs.set_attribute(fid, k, v)?;
            }
            r.build_ms += ms(t.elapsed());
            let t = Instant::now();
            fs.commit()?;
            r.commit_ms += ms(t.elapsed());
            r.files += 1;
        }
    }

    fs.set_autocommit(true);
    r.wall_ms = ms(wall.elapsed());
    let cs = fs.commit_stats();
    r.commits = cs.commits;
    r.blocks = cs.blocks_written;
    Ok(r)
}

/// COLD sync via the bulk create+write path: the whole tree is staged
/// (autocommit off — every mkdir and `create_files_batch` stages only) and
/// committed ONCE at the end. One root flip for the entire tree.
fn sync_cold_batch(fs: &mut FileSystem, plan: &[DirPlan], stamp: &str) -> Result<RunReport> {
    let mut r = RunReport::default();
    let root_id = fs.superblock.root_inode;
    let mut ids: BTreeMap<Vec<String>, u64> = BTreeMap::new();
    ids.insert(Vec::new(), root_id);
    fs.set_autocommit(false);
    let wall = Instant::now();

    for dir in plan {
        let dir_id = if dir.path.is_empty() {
            root_id
        } else {
            let (parent_id, name) = dir_ids_for(dir, &ids);
            let t = Instant::now();
            let id = fs.mkdir(parent_id, name.to_string())?;
            r.build_ms += ms(t.elapsed());
            r.dirs += 1;
            id
        };
        ids.insert(dir.path.clone(), dir_id);

        if dir.files.is_empty() {
            continue;
        }
        let batch: Vec<BatchFile> = dir
            .files
            .iter()
            .map(|f| BatchFile {
                name: f.name.clone(),
                data: f.data.clone(),
                attributes: file_attrs(f, stamp).into_iter().collect(),
            })
            .collect();
        r.files += batch.len();
        let t = Instant::now();
        fs.create_files_batch(dir_id, batch)?;
        r.build_ms += ms(t.elapsed());
    }

    // The single whole-tree flip.
    let t = Instant::now();
    fs.commit()?;
    r.commit_ms += ms(t.elapsed());
    fs.set_autocommit(true);
    r.wall_ms = ms(wall.elapsed());
    let cs = fs.commit_stats();
    r.commits = cs.commits;
    r.blocks = cs.blocks_written;
    Ok(r)
}

/// WARM (incremental) re-sync: re-mount the just-built image and re-walk the
/// plan, skipping every file whose stored `vaire.size` + `vaire.mtime` still
/// match the live file (the all-skip case that dominates a real warm run).
/// Measures the lookup-bound incremental cost + one final commit.
fn sync_warm(fs: &mut FileSystem, plan: &[DirPlan]) -> Result<(RunReport, usize)> {
    let mut r = RunReport::default();
    let mut skipped = 0usize;
    fs.set_autocommit(false);
    let wall = Instant::now();

    for dir in plan {
        let prefix = if dir.path.is_empty() {
            String::from("/")
        } else {
            format!("/{}", dir.path.join("/"))
        };
        for f in &dir.files {
            let vault_path = if prefix == "/" {
                format!("/{}", f.name)
            } else {
                format!("{}/{}", prefix, f.name)
            };
            let t = Instant::now();
            let id = fs.resolve_path(&vault_path)?;
            let size = fs.get_attribute(id, "vaire.size")?;
            let mtime = fs.get_attribute(id, "vaire.mtime")?;
            r.build_ms += ms(t.elapsed());
            if size == Some(AttributeValue::Int(f.size))
                && mtime == Some(AttributeValue::Int(f.mtime))
            {
                skipped += 1;
            }
            r.files += 1;
        }
    }
    let t = Instant::now();
    fs.commit()?;
    r.commit_ms += ms(t.elapsed());
    fs.set_autocommit(true);
    r.wall_ms = ms(wall.elapsed());
    let cs = fs.commit_stats();
    r.commits = cs.commits;
    r.blocks = cs.blocks_written;
    Ok((r, skipped))
}

fn print_report(label: &str, r: &RunReport) {
    println!(
        "  {:<22} files={:<5} dirs={:<4} scan={:>8.2} build={:>9.2} commit={:>10.2} \
         flips={:<6} blocks={:<7} wall={:>10.2}",
        label, r.files, r.dirs, r.scan_ms, r.build_ms, r.commit_ms, r.commits, r.blocks, r.wall_ms
    );
}

fn run_bench_batch(source: &str, out_dir: &str, size_mb: u64) -> Result<()> {
    let root = Path::new(source);
    anyhow::ensure!(root.is_dir(), "source '{}' is not a directory", source);
    let out = Path::new(out_dir);
    std::fs::create_dir_all(out).context("create out_dir")?;
    let perop_img = out.join("bench-batch-perop.img");
    let batch_img = out.join("bench-batch-batch.img");
    let stamp = "bench-batch-run";

    // Shared scan (one walk of the tree; both modes sync the same plan).
    let t = Instant::now();
    let plan = scan_tree(root)?;
    let scan_ms = ms(t.elapsed());
    let n_files: usize = plan.iter().map(|d| d.files.len()).sum();
    let n_dirs = plan.iter().filter(|d| !d.path.is_empty()).count();
    let n_bytes: usize = plan.iter().flat_map(|d| &d.files).map(|f| f.data.len()).sum();

    println!("UNAFS-BATCH before/after — source '{}'", source);
    println!(
        "  load: {} files, {} dirs, {} bytes; images {} MB each (throwaway)",
        n_files, n_dirs, n_bytes, size_mb
    );
    println!("  scan (shared): {:.2} ms\n", scan_ms);

    // --- COLD, per-op control ---
    let mut fp = fresh_image(&perop_img, size_mb)?;
    let mut rp = sync_cold_perop(&mut fp, &plan, stamp)?;
    rp.scan_ms = scan_ms;
    anyhow::ensure!(fp.fsck(false)?.is_clean(), "per-op cold image not fsck-clean");

    // --- COLD, batch ---
    let mut fb = fresh_image(&batch_img, size_mb)?;
    let mut rb = sync_cold_batch(&mut fb, &plan, stamp)?;
    rb.scan_ms = scan_ms;
    anyhow::ensure!(fb.fsck(false)?.is_clean(), "batch cold image not fsck-clean");

    println!("COLD (fresh format):");
    print_report("per-op (control)", &rp);
    print_report("batch", &rb);

    // --- WARM (incremental all-skip) on each just-built image ---
    drop(fp);
    drop(fb);
    let mut fp2 = FileSystem::mount(FileDevice::open(&perop_img)?)?;
    let (rp_warm, sp) = sync_warm(&mut fp2, &plan)?;
    anyhow::ensure!(fp2.fsck(false)?.is_clean(), "per-op warm image not fsck-clean");
    let mut fb2 = FileSystem::mount(FileDevice::open(&batch_img)?)?;
    let (rb_warm, sb) = sync_warm(&mut fb2, &plan)?;
    anyhow::ensure!(fb2.fsck(false)?.is_clean(), "batch warm image not fsck-clean");

    println!("\nWARM (incremental, all-skip):");
    print_report(&format!("per-op ({sp} skip)"), &rp_warm);
    print_report(&format!("batch ({sb} skip)"), &rb_warm);

    println!(
        "\nHeadline: cold commit-phase {:.0} ms across {} flips (per-op) -> {:.0} ms across {} flips (batch).",
        rp.commit_ms, rp.commits, rb.commit_ms, rb.commits
    );
    println!("Images left at:\n  {}\n  {}", perop_img.display(), batch_img.display());
    Ok(())
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

            // Format through the one UnaFS format entry (AMBER1: `unafs::format`, shared with
            // Amber Bytes and the installer), then mount the fresh volume to announce it.
            let mut device = FileDevice::open(path).context("Failed to open device")?;
            let made = unafs::format(&mut device, &unafs::FormatParams::sized_mb(*size_mb))
                .context("Failed to format filesystem")?;
            println!(
                "   v{} · {} blocks · generation {} · {} free",
                made.version, made.blocks, made.generation, made.free_blocks
            );
            let fs = FileSystem::mount(device).context("Failed to mount the fresh volume")?;

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
        Commands::Mkdir { path, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;
            let mut at = String::new();
            let mut parent = fs.resolve_path("/").context("No root")?;
            let mut made = 0usize;
            for c in path.split('/').filter(|c| !c.is_empty()) {
                at.push('/');
                at.push_str(c);
                parent = match fs.resolve_path(&at) {
                    Ok(id) => id,
                    Err(_) => {
                        made += 1;
                        fs.mkdir(parent, c.to_string()).with_context(|| format!("mkdir {at}"))?
                    }
                };
            }
            println!("✅ [OPERATOR] mkdir '{}' ({} created)", path, made);
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
        Commands::Stat { path, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;
            let id = fs.resolve_path(path).context("Path not found")?;
            let st = fs.stat(id).map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "id {} kind {:?} size {} parent {} ctime {} mtime {} atime {} path {}",
                st.inode_id,
                st.kind,
                st.size,
                st.parent,
                st.ctime,
                st.mtime,
                st.atime,
                fs.path_of(id).map_err(|e| anyhow::anyhow!(e))?
            );
        }
        Commands::Query { query, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            let results = fs.query(query).map_err(|e| anyhow::anyhow!(e))?;

            // One hit per line: `id path score` (B302 — the path comes from
            // the v6 parent pointers; an unnamed object prints `-`).
            println!("Found {} results:", results.len());
            for hit in results {
                let path = if hit.path.is_empty() { "-" } else { hit.path.as_str() };
                println!("{} {} {:.4}", hit.inode_id, path, hit.score);
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
        Commands::Grow { img, blocks } => {
            let want = blocks
                .checked_mul(unafs::BLOCK_SIZE)
                .context("block count overflows a byte size")?;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(img)
                .context("Failed to open image")?;
            let len = file.metadata().context("Failed to stat image")?.len();
            if want > len {
                file.set_len(want).context("Failed to extend image")?;
            }
            drop(file);
            let device = FileDevice::open(img).context("Failed to open device")?;
            let (mut fs, r) = unafs::grow(device, *blocks)
                .map_err(|e| anyhow::anyhow!("grow failed: {}", e))?;
            let rep = fs
                .fsck(false)
                .map_err(|e| anyhow::anyhow!("fsck after grow failed: {:?}", e))?;
            println!(
                "grow '{}': from={} to={} free {} -> {} fsck={}",
                img,
                r.from,
                r.to,
                r.free_before,
                r.free_after,
                if rep.is_clean() { "ok" } else { "NOT-CLEAN" }
            );
            if !rep.is_clean() {
                anyhow::bail!("the grown volume is not clean");
            }
        }
        Commands::BenchMap { out_dir, sizes, version, files } => {
            run_bench_map(out_dir, sizes, *version, *files)?;
        }
        Commands::Dump { records, img } => {
            anyhow::ensure!(*records, "dump: pass --records (the only view today)");
            print!("{}", dump::dump_records(img)?);
        }
        Commands::Fsck { img, repair } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            if *repair {
                let report = fs
                    .recover()
                    .map_err(|e| anyhow::anyhow!("fsck --repair failed: {:?}", e))?;
                println!("🔧 [FSCK] refcount rebuild complete on '{}'", img);
                println!("  stale index inodes   : {}", report.orphan_inodes.len());
                println!("  catalog entries scrubbed: {}", report.scrubbed_catalog_entries);
                println!("  blocks reclaimed     : {}", report.reclaimed_blocks);
                println!("  parent links restamped: {}", report.bad_parent_links.len());
            } else {
                let report = fs
                    .fsck(false)
                    .map_err(|e| anyhow::anyhow!("fsck failed: {:?}", e))?;
                println!(
                    "🔍 [FSCK] scan of '{}' (dry run — pass --repair to heal)",
                    img
                );
                println!("  root generation      : {}", fs.root_generation());
                println!("  blocks in use        : {}", report.blocks_in_use);
                println!("  reachable blocks     : {}", report.reachable_blocks);
                println!("  leaked blocks        : {}", report.leaked_blocks.len());
                println!("  stale index inodes   : {}", report.orphan_inodes.len());
                println!("  bad parent links     : {}", report.bad_parent_links.len());
                if report.is_clean() {
                    println!("  ✅ volume is clean");
                }
            }
        }
        Commands::Snaps { img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;
            let snaps = fs
                .snapshot_index()
                .map_err(|e| anyhow::anyhow!("Failed to read snapshot index: {:?}", e))?;
            if snaps.is_empty() {
                println!("📷 [OPERATOR] no retained snapshots on '{}'", img);
            } else {
                println!("📷 [OPERATOR] {} retained snapshot(s) on '{}':", snaps.len(), img);
                println!("  {:>10}  {:<20}  {:<16}  gen", "created@", "name", "creator");
                for s in &snaps {
                    println!(
                        "  {:>10}  {:<20}  {:<16}  {}",
                        s.timestamp, s.name, s.creator, s.generation
                    );
                }
            }
        }
        Commands::Snap { name, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let generation = fs
                .snapshot_create(name.clone(), "operator".into(), ts)
                .map_err(|e| anyhow::anyhow!("Failed to create snapshot '{}': {:?}", name, e))?;
            println!(
                "✅ [OPERATOR] retained snapshot '{}' (generation {}) on '{}'",
                name, generation, img
            );
        }
        Commands::Snapdrop { generation, img } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;
            fs.snapshot_drop(*generation)
                .map_err(|e| anyhow::anyhow!("Failed to drop snapshot {}: {:?}", generation, e))?;
            println!(
                "✅ [OPERATOR] dropped snapshot generation {} (blocks reclaimed) on '{}'",
                generation, img
            );
        }
        Commands::Snapcat {
            generation,
            path,
            as_principal,
            img,
        } => {
            let device = FileDevice::open(img).context("Failed to open device")?;
            let mut fs = FileSystem::mount(device).context("Failed to mount filesystem")?;

            // Phase 1: resolve the object's logical id in the SNAPSHOT.
            let sid = {
                let mut view = fs.open_snapshot(*generation).map_err(|e| {
                    anyhow::anyhow!("no such snapshot generation {} ({:?})", generation, e)
                })?;
                view.resolve_path(path)
                    .map_err(|_| anyhow::anyhow!("{}: not in snapshot gen {}", path, generation))?
            };

            // Phase 2: CURRENT-ACL — authorize against the LIVE object of that id.
            // This mirrors the kernel `read_authz` faithfully (lens A fold): a live
            // object gone from the tree fails closed (no current ACL row); kernel
            // authority permits; a public object (no owner) permits; the owner
            // permits; a grant permits ONLY if its rights value carries the READ
            // right — the K6 `rw`/`r`/`w` encoding, the same mapping the kernel's
            // `rights_from_native` decodes and tests against CAP_READ (a write-only
            // `w` grant is refused, matching the live syscall path).
            match fs.read_inode(sid) {
                Err(_) => {
                    anyhow::bail!(
                        "{}: refused — object deleted from live tree (no current ACL; fail-closed)",
                        path
                    );
                }
                Ok(live) => {
                    if as_principal != "kernel" {
                        if let Some(unafs::AttributeValue::String(owner)) =
                            live.attributes.get("owner")
                        {
                            // Rights-aware: only `rw`/`r` carry the read right
                            // (rights_from_native: rw->R|W, r->R, w->W, else 0).
                            let grant_reads = matches!(
                                live.attributes.get(&format!("grants:{}", as_principal)),
                                Some(unafs::AttributeValue::String(r)) if r == "rw" || r == "r"
                            );
                            if as_principal != owner && !grant_reads {
                                anyhow::bail!(
                                    "{}: refused — current ACL denies principal '{}' (no read right)",
                                    path,
                                    as_principal
                                );
                            }
                        }
                        // No `owner` attribute → public live object → permitted.
                    }
                }
            }

            // Phase 3: permitted — hand back the retained bytes.
            let mut view = fs
                .open_snapshot(*generation)
                .map_err(|e| anyhow::anyhow!("reopen snapshot {} failed: {:?}", generation, e))?;
            let size = view
                .read_inode(sid)
                .map_err(|e| anyhow::anyhow!("read snapshot inode failed: {:?}", e))?
                .size;
            let bytes = view
                .read_data(sid, 0, size)
                .map_err(|e| anyhow::anyhow!("read snapshot data failed: {:?}", e))?;
            match std::str::from_utf8(&bytes) {
                Ok(s) => print!("{}", s),
                Err(_) => eprintln!(
                    "📷 [OPERATOR] gen {} {} — {} bytes (binary, not printed)",
                    generation,
                    path,
                    bytes.len()
                ),
            }
        }
        Commands::BenchBatch {
            source,
            out_dir,
            size_mb,
        } => {
            run_bench_batch(source, out_dir, *size_mb)?;
        }
        Commands::Migrate { from, to, size_mb } => {
            // Which format is the source? v2 → the legacy walker; v3–v5 → a
            // live (read-only device) mount replayed into v6 (B302).
            let k8_source = unafs::legacy::LegacyVolume::open(
                FileDevice::open_read_only(from).context("Failed to open source image read-only")?,
            )
            .is_err();

            let make_target = |src_blocks: u64| -> Result<FileSystem> {
                let src_mb = (src_blocks * unafs::BLOCK_SIZE).div_ceil(1024 * 1024);
                let target_mb = size_mb.unwrap_or(src_mb.max(1));
                let file = std::fs::File::create(to).context("Failed to create target file")?;
                file.set_len(target_mb * 1024 * 1024)
                    .context("Failed to size target file")?;
                let new_dev = FileDevice::open(to).context("Failed to open target device")?;
                FileSystem::format(new_dev, target_mb)
                    .map_err(|e| anyhow::anyhow!("failed to format target: {:?}", e))
            };

            let (report, mut new) = if k8_source {
                let old_dev = FileDevice::open_read_only(from)
                    .context("Failed to open source image read-only")?;
                let mut old = FileSystem::mount(old_dev)
                    .map_err(|e| anyhow::anyhow!("source is neither v2 nor a mountable K8 volume: {:?}", e))?;
                anyhow::ensure!(
                    !old.superblock.indexed(),
                    "source is already v{} (indexed catalog) — nothing to migrate",
                    old.superblock.version
                );
                println!(
                    "⚡ [OPERATOR] Migrating v{} vault '{}' → v{} '{}'...",
                    old.superblock.version,
                    from,
                    unafs::superblock::VERSION,
                    to
                );
                let snaps = old.snapshot_index().map(|v| v.len()).unwrap_or(0);
                if snaps > 0 {
                    println!("   (note: {snaps} retained snapshot(s) are not carried over — they name the old volume's history)");
                }
                let mut new = make_target(old.superblock.block_count)?;
                let report = unafs::legacy::migrate_k8_into(&mut old, &mut new)
                    .map_err(|e| anyhow::anyhow!("migration failed: {:?}", e))?;
                (report, new)
            } else {
                println!("⚡ [OPERATOR] Migrating pre-K8 vault '{}' → v{} '{}'...", from, unafs::superblock::VERSION, to);
                let old_dev = FileDevice::open_read_only(from)
                    .context("Failed to open source image read-only")?;
                let mut old = unafs::legacy::LegacyVolume::open(old_dev)
                    .map_err(|e| anyhow::anyhow!("source is not a pre-K8 (v2) volume: {:?}", e))?;
                let mut new = make_target(old.superblock.block_count)?;
                let report = unafs::legacy::migrate_into(&mut old, &mut new)
                    .map_err(|e| anyhow::anyhow!("migration failed: {:?}", e))?;
                (report, new)
            };

            // Belt-and-braces: the migrated volume must be consistent.
            let check = new
                .fsck(false)
                .map_err(|e| anyhow::anyhow!("post-migration fsck failed: {:?}", e))?;
            anyhow::ensure!(check.is_clean(), "post-migration fsck not clean: {check:?}");

            println!(
                "✅ [OPERATOR] Migrated {} files, {} directories, {} bytes → '{}' (v{}, gen {}) — fsck clean",
                report.files,
                report.directories,
                report.bytes,
                to,
                new.superblock.version,
                new.root_generation()
            );
        }
    }

    Ok(())
}

// =============================================================================
// UNAFSMAP (B354) map bench (bench-map)
// =============================================================================

/// Live/peak heap counters for `bench-map` (a counting wrapper over the system
/// allocator; costs two atomics per allocation for every verb).
struct CountingAlloc;
static HEAP_CUR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HEAP_PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        use std::sync::atomic::Ordering::Relaxed;
        let p = unsafe { std::alloc::System.alloc(l) };
        if !p.is_null() {
            let now = HEAP_CUR.fetch_add(l.size(), Relaxed) + l.size();
            HEAP_PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(p, l) };
        HEAP_CUR.fetch_sub(l.size(), std::sync::atomic::Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn heap_mark() -> usize {
    use std::sync::atomic::Ordering::Relaxed;
    let c = HEAP_CUR.load(Relaxed);
    HEAP_PEAK.store(c, Relaxed);
    c
}
fn heap_peak_since(mark: usize) -> usize {
    HEAP_PEAK.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(mark)
}

fn run_bench_map(out_dir: &str, sizes: &str, version: u32, files: u64) -> Result<()> {
    println!("| v | volume | blocks | format s | mount s | mount peak heap | 1-block write commit blocks | map blocks | files | fsck s | fsck peak heap |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for size in sizes.split(',') {
        let mib: u64 = size.trim().parse().context("size (MiB)")?;
        let path = Path::new(out_dir).join(format!("bench-map-v{version}-{mib}.img"));
        struct Rm(std::path::PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _rm = Rm(path.clone());
        std::fs::File::create(&path)?.set_len(mib * 1024 * 1024)?;
        let t = Instant::now();
        {
            let device = FileDevice::open(&path)?;
            let mut fs = FileSystem::format_with_version(device, mib, version)
                .map_err(|e| anyhow::anyhow!("format: {e}"))?;
            let root = fs.superblock.root_inode;
            let id = fs.create_file(root, "f".into()).map_err(|e| anyhow::anyhow!("{e}"))?;
            fs.write_data(id, 0, &[7u8; 4096]).map_err(|e| anyhow::anyhow!("{e}"))?;
            if files > 0 {
                // 1,000 files per directory (a directory's entry list is one
                // decoded object with a bounded budget).
                let top = fs.mkdir(root, "many".into()).map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut made = 0u64;
                while made < files {
                    let n = (files - made).min(1_000);
                    let dir = fs
                        .mkdir(top, format!("d{}", made / 1_000))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let batch: Vec<BatchFile> = (0..n)
                        .map(|i| BatchFile {
                            name: format!("f{}", made + i),
                            data: Vec::new(),
                            attributes: BTreeMap::new(),
                        })
                        .collect();
                    fs.create_files_batch(dir, batch).map_err(|e| anyhow::anyhow!("batch: {e}"))?;
                    made += n;
                }
            }
        }
        let format_s = t.elapsed().as_secs_f64();
        let mark = heap_mark();
        let t = Instant::now();
        let mut fs = FileSystem::mount(FileDevice::open(&path)?).map_err(|e| anyhow::anyhow!("mount: {e}"))?;
        let mount_s = t.elapsed().as_secs_f64();
        let mount_heap = heap_peak_since(mark);
        let id = fs.resolve_path("/f").map_err(|e| anyhow::anyhow!("{e}"))?;
        fs.write_data(id, 0, &[9u8; 4096]).map_err(|e| anyhow::anyhow!("{e}"))?;
        let commit_blocks = fs.commit_stats().last_commit_blocks;
        let mark = heap_mark();
        let t = Instant::now();
        let rep = fs.fsck(false).map_err(|e| anyhow::anyhow!("fsck: {e}"))?;
        let fsck_s = t.elapsed().as_secs_f64();
        let fsck_heap = heap_peak_since(mark);
        anyhow::ensure!(rep.is_clean(), "fsck not clean: {rep:?}");
        println!(
            "| {} | {} MiB | {} | {:.3} | {:.3} | {} KiB | {} | {} | {} | {:.3} | {} KiB |",
            fs.superblock.version,
            mib,
            fs.superblock.block_count,
            format_s,
            mount_s,
            mount_heap / 1024,
            commit_blocks,
            fs.map_block_total(),
            files,
            fsck_s,
            fsck_heap / 1024
        );
    }
    Ok(())
}
