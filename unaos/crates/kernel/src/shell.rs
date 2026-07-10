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

use alloc::vec::Vec;
use alloc::string::String;
use crate::console::Console;
use crate::fs::fat::{DirEntry, FatError, FatFs};
use crate::vug;
use crate::pal::TargetPal;

/// JD4: the shell's current working directory as a NORMALIZED, CANONICAL absolute path
/// ("/" = root, else "/DIR/SUB" in the on-disk 8.3 spelling). A path string, not a cached
/// cluster: every command re-resolves it from the root, so a swapped or remounted card can
/// never leave the shell holding a stale chain head — the worst case is an honest `-ENOENT`.
/// `None` means the root (no heap touched until the first `cd`).
static CWD: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// The current working directory as a display/join-ready absolute path.
fn cwd_path() -> String {
    CWD.lock().clone().unwrap_or_else(|| String::from("/"))
}

/// Join `arg` onto `base` and normalize lexically: absolute `arg` replaces `base`, `.` and empty
/// components collapse, `..` pops (never above the root). Purely textual — resolution against the
/// volume happens in [`resolve_path`].
fn normalize_path(base: &str, arg: &str) -> String {
    let mut comps: Vec<&str> = Vec::new();
    let prefix = if arg.starts_with('/') { "" } else { base };
    for part in prefix.split('/').chain(arg.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                comps.pop();
            }
            p => comps.push(p),
        }
    }
    if comps.is_empty() {
        return String::from("/");
    }
    let mut out = String::new();
    for c in comps {
        out.push('/');
        out.push_str(c);
    }
    out
}

/// A resolved absolute path: the root itself, or a concrete directory entry (file or subdir)
/// plus the CANONICAL absolute path it was found at (on-disk 8.3 spelling).
enum Resolved {
    Root,
    Entry(DirEntry, String),
}

/// Walk a normalized absolute path from the root, component by component, via the read-only
/// `FatFs::read_dir`. Case-insensitive 8.3 matching (short names are stored uppercase on disk).
/// Errors carry the errno-style tag the caller prints — nothing is swallowed.
fn resolve_path(fs: &FatFs, path: &str) -> Result<Resolved, String> {
    let mut cluster = 0u32; // 0 = the root (read_dir's convention)
    let mut cur: Option<(DirEntry, String)> = None;
    let mut canon = String::new();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        if let Some((de, _)) = &cur {
            if !de.is_dir {
                return Err(alloc::format!("{}: not a directory (-ENOTDIR)", canon));
            }
            cluster = de.first_cluster();
        }
        let entries = fs
            .read_dir(cluster)
            .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", canon, e))?;
        match entries.iter().find(|de| de.name().eq_ignore_ascii_case(comp)) {
            Some(de) => {
                canon.push('/');
                canon.push_str(de.name());
                cur = Some((*de, canon.clone()));
            }
            None => {
                return Err(alloc::format!("{}/{}: not found (-ENOENT)", canon, comp));
            }
        }
    }
    Ok(match cur {
        None => Resolved::Root,
        Some((de, canon)) => Resolved::Entry(de, canon),
    })
}

// ---------------------------------------------------------------------------------------------
// JD5 — the WRITE path: create / edit / delete files from the panel shell.
//
// DESIGN NOTE (why this rides fat.rs directly, not the SYS_OPEN/WRITE syscall layer):
// The SVC syscall path is EL0/ASID-keyed and lives in the out-of-lane `arch/aarch64/syscall.rs`;
// the kernel shell runs at EL1 as ASID 0 (on the tegra post-drop core TTBR0_EL1[63:48] = 0 — it
// never switches TTBR0 to a user slot), so it cannot invoke the SVC path. The shell therefore rides
// the SAME F3-locked fat.rs PUBLIC entry points the U9/U10/U11 syscalls call — `create_in_root`,
// `find_located`, `write_grow`, `delete_located` — never editing fat.rs (call-never-edit this arc).
//
// PRINCIPAL: the shell IS ASID 0, the shared/public principal. By U6's existing rule an ASID-0
// create is PUBLIC (no owner row), so shell-created files are plain public FAT files. The shell does
// not — and cannot without an out-of-lane pub accessor — consult the U6 `OWNED_FILES` ACL; that is
// correct: the panel shell is the local trusted operator console, the same trust as the boot window.
// (A future arc that runs EL0 tasks and returns to the shell must re-establish ASID 0 before shell
// FAT ops — today the shell is cooperative EL1 and never installs a user-slot TTBR0.)
//
// SCOPE: ROOT-directory only. `create_in_root`/`find_located` are root-only and fat.rs is
// call-never-edit, so a target whose parent is a subdirectory is an honest `-ENOTSUP` (a subdir
// create needs a future `create_in_dir`). Bare names resolve against the cwd; only a root parent
// is writable.
//
// SAFETY (M3): every fat.rs write returns a `Result`; a stalled USB write surfaces as
// `FatError::Io` (the block layer's `write_block` rides the SAME JD3 wall-clock-bounded BOT pump as
// reads — a dead transfer times out, never WFI-parks the timerless EL1 core). Writes are
// WRITE-THROUGH (BOT WRITE(10) / polled SD CMD24 complete before the command returns — no write-back
// cache), and each fat.rs step is atomic under F3's FAT_MUTATION/DIR_MUTATION locks, so a mid-
// sequence failure leaves the volume consistent: a failed grow keeps the OLD (smaller) size (size is
// published last); a failed `rm`/truncate leaves lost clusters (benign, chkdsk-reclaimable), never
// an aliasing or torn volume.

/// JD5: errno tag for a `FatError`, for the write-command console messages.
fn fat_errno(e: FatError) -> &'static str {
    match e {
        FatError::NotFound => "-ENOENT",
        FatError::IsDirectory => "-EISDIR",
        FatError::NoSpace => "-ENOSPC",
        FatError::Unsupported => "-EINVAL", // not a representable 8.3 name
        FatError::NoDisk | FatError::NotFat => "-ENODEV",
        FatError::Io | FatError::BadChain => "-EIO",
    }
}

/// JD5: resolve a shell path argument to a bare ROOT-directory 8.3 name (parent must be the root).
/// `.`/`..` normalize lexically against the cwd first; a target one level below the root yields its
/// name, the root itself is rejected, and any deeper target is `-ENOTSUP` (the write path is root-
/// only this arc — see the module DESIGN NOTE). Returns `Ok(name)` or a ready-to-print error tail.
fn resolve_root_name(arg: &str) -> Result<String, String> {
    let path = normalize_path(&cwd_path(), arg);
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    match comps.as_slice() {
        [name] => Ok(String::from(*name)),
        [] => Err(String::from("/: is a directory (-EISDIR)")),
        _ => Err(alloc::format!("{}: writes are root-directory only this arc (-ENOTSUP)", path)),
    }
}

/// JD5 `touch`: ensure a 0-length ROOT file exists (create if absent; idempotent no-op if present).
fn fs_touch(console: &mut Console, arg: &str) {
    let name = match resolve_root_name(arg) {
        Ok(n) => n,
        Err(msg) => return console.println(&alloc::format!("touch: {}", msg)),
    };
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("touch: no FAT filesystem ({:?})", e)),
    };
    match fs.find_located(&name) {
        Ok((de, _, _)) => console.println(&alloc::format!("/{}", de.name())), // already exists
        Err(FatError::NotFound) => match fs.create_in_root(&name, 0x20) {
            Ok((de, _, _)) => console.println(&alloc::format!("/{}", de.name())),
            Err(e) => console.println(&alloc::format!("touch: {}: {} ({:?})", name, fat_errno(e), e)),
        },
        Err(e) => console.println(&alloc::format!("touch: {}: {} ({:?})", name, fat_errno(e), e)),
    }
}

/// JD5 `write`: create-or-TRUNCATE a ROOT file and store `data` (the exact given bytes). Truncate of
/// an existing file = free its chain + a fresh 0-length entry, then grow-write — the only create-or-
/// truncate reachable through fat.rs's PUBLIC API (there is no in-place shrink primitive, and the
/// directory-field publisher is private). A directory target is refused (`-EISDIR`).
fn fs_write(console: &mut Console, arg: &str, data: &[u8]) {
    let name = match resolve_root_name(arg) {
        Ok(n) => n,
        Err(msg) => return console.println(&alloc::format!("write: {}", msg)),
    };
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("write: no FAT filesystem ({:?})", e)),
    };
    // Locate (root only). Present-as-file -> TRUNCATE (delete then recreate); directory -> refuse;
    // absent -> create fresh. The result is the fresh entry's on-disk (dir_lba, dir_off).
    let (dir_lba, dir_off) = match fs.find_located(&name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return console.println(&alloc::format!("write: /{}: is a directory (-EISDIR)", de.name()));
            }
            if let Err(e) = fs.delete_located(dl, doff, de.first_cluster()) {
                return console.println(&alloc::format!(
                    "write: /{}: truncate failed {} ({:?})", name, fat_errno(e), e));
            }
            match fs.create_in_root(&name, 0x20) {
                Ok((_, dl2, doff2)) => (dl2, doff2),
                Err(e) => return console.println(&alloc::format!(
                    "write: /{}: recreate failed {} ({:?}) — old file removed", name, fat_errno(e), e)),
            }
        }
        Err(FatError::NotFound) => match fs.create_in_root(&name, 0x20) {
            Ok((_, dl, doff)) => (dl, doff),
            Err(e) => return console.println(&alloc::format!("write: {}: {} ({:?})", name, fat_errno(e), e)),
        },
        Err(e) => return console.println(&alloc::format!("write: {}: {} ({:?})", name, fat_errno(e), e)),
    };
    if data.is_empty() {
        return console.println(&alloc::format!("wrote 0 bytes to /{}", name));
    }
    // The entry is a fresh 0-length file (first_cluster = 0, size = 0): grow from offset 0.
    match fs.write_grow(0, 0, dir_lba, dir_off, 0, data) {
        Ok((written, new_size, _)) =>
            console.println(&alloc::format!("wrote {} bytes to /{} ({} bytes)", written, name, new_size)),
        Err(e) => console.println(&alloc::format!("write: /{}: {} ({:?})", name, fat_errno(e), e)),
    }
}

/// JD5 `append`: append `data` at the end of a ROOT file, creating it if absent (like `>>`). The
/// append grows the file from its current EOF via `write_grow` (allocate + zero-fill + chain new
/// clusters, directory `size` published LAST). A directory target is refused (`-EISDIR`).
fn fs_append(console: &mut Console, arg: &str, data: &[u8]) {
    let name = match resolve_root_name(arg) {
        Ok(n) => n,
        Err(msg) => return console.println(&alloc::format!("append: {}", msg)),
    };
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("append: no FAT filesystem ({:?})", e)),
    };
    let (first_cluster, size, dir_lba, dir_off) = match fs.find_located(&name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return console.println(&alloc::format!("append: /{}: is a directory (-EISDIR)", de.name()));
            }
            (de.first_cluster(), de.size, dl, doff)
        }
        Err(FatError::NotFound) => match fs.create_in_root(&name, 0x20) {
            Ok((de, dl, doff)) => (de.first_cluster(), de.size, dl, doff), // fresh: 0, 0
            Err(e) => return console.println(&alloc::format!("append: {}: {} ({:?})", name, fat_errno(e), e)),
        },
        Err(e) => return console.println(&alloc::format!("append: {}: {} ({:?})", name, fat_errno(e), e)),
    };
    if data.is_empty() {
        return console.println(&alloc::format!("appended 0 bytes to /{} ({} bytes)", name, size));
    }
    // Seek to EOF (`start = size`) and grow: write_grow appends the new bytes past the current end.
    match fs.write_grow(first_cluster, size, dir_lba, dir_off, size, data) {
        Ok((written, new_size, _)) =>
            console.println(&alloc::format!("appended {} bytes to /{} ({} bytes)", written, name, new_size)),
        Err(e) => console.println(&alloc::format!("append: /{}: {} ({:?})", name, fat_errno(e), e)),
    }
}

/// JD5 `rm`: delete a ROOT file — `delete_located` marks the directory slot `0xE5` FIRST, then frees
/// the cluster chain (all FAT copies), the crash-safe order fat.rs guarantees. A directory target is
/// refused (`-EISDIR` — directory removal is out of scope this arc); an absent name is `-ENOENT`.
fn fs_rm(console: &mut Console, arg: &str) {
    let name = match resolve_root_name(arg) {
        Ok(n) => n,
        Err(msg) => return console.println(&alloc::format!("rm: {}", msg)),
    };
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
    match fs.find_located(&name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return console.println(&alloc::format!("rm: /{}: is a directory (-EISDIR)", de.name()));
            }
            match fs.delete_located(dl, doff, de.first_cluster()) {
                Ok(freed) => console.println(&alloc::format!(
                    "removed /{} ({} cluster(s) freed)", de.name(), freed.len())),
                Err(e) => console.println(&alloc::format!("rm: /{}: {} ({:?})", name, fat_errno(e), e)),
            }
        }
        Err(FatError::NotFound) => console.println(&alloc::format!("rm: {}: not found (-ENOENT)", name)),
        Err(e) => console.println(&alloc::format!("rm: {}: {} ({:?})", name, fat_errno(e), e)),
    }
}

/// Print one directory's entries in the `ls` table format, with the file/dir tally.
fn print_dir_listing(console: &mut Console, entries: &[DirEntry]) {
    let (mut files, mut dirs) = (0u32, 0u32);
    for de in entries {
        if de.is_dir {
            dirs += 1;
            console.println(&alloc::format!("  <DIR>         {}", de.name()));
        } else {
            files += 1;
            console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
        }
    }
    console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
}

pub struct History {
    entries: Vec<String>,
    position: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
        }
    }

    pub fn push(&mut self, cmd: String) {
        if !cmd.trim().is_empty() {
            self.entries.push(cmd);
            self.position = self.entries.len();
        }
    }
}

/// Run one command. Returns `true` if the command took over the whole screen with its own
/// graphics (e.g. `vug`), so the caller should NOT repaint the console over it.
pub fn dispatch_command(cmd_line: &str, console: &mut Console, pal: &mut TargetPal) -> bool {
    // Split command and args (simple whitespace split)
    let mut parts = cmd_line.trim().split_whitespace();
    let command = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();

    // The `vug` command paints a full-screen demo; everything else leaves the console visible.
    let took_screen = command == "vug";

    match command {
        "ver" | "version" => {
            console.println("unaOS v0.1.0 (Kernel: Jules 1 / Cortex: Jules 6)");
        },
        "help" => {
            console.println("COMMANDS: ver, help, clear, echo, panic, gneiss");
            console.println("STORAGE:  diskinfo, usbinfo, read <lba>, write <lba> <byte>");
            console.println("FILES:    fatinfo (FAT geometry), ls [dir], cd [dir], pwd, cat <path>");
            console.println("WRITE:    touch <path>, write <path> <text>, append <path> <text>, rm <path>");
            console.println("          (root dir; create/edit/delete files)");
            console.println("SMP:      sched (per-CPU run queues)");
            console.println("TEST:     tste (in-OS self-test suite: boot-replay + live checks)");
            console.println("NETWORK:  netinfo, ping <ip> [count], arp <ip>");
            console.println("          connect <ip> <port> [message], udpsend <ip> <port> [message]");
            console.println("          get <ip> [port] [path]  (HTTP/1.0 GET)");
        },
        "clear" => {
            // Clear both the screen and the console buffer?
            // Usually 'clear' clears the visible screen.
            // For now, we will rely on console.draw() to repaint.
            // To effectively clear, we might want to clear the lines in console?
            // Or just clear screen. But draw() repaints lines.
            // Let's implement a 'clear' on console if needed, or just let draw handle it.
            // If the user wants a blank slate, we should probably clear the history buffer.
            // BUT, the prompt said "Reset cursor logic here".
            // Let's implement a clear method on Console.
            console.clear();
        },
        "echo" => {
            let content = args.join(" ");
            console.println(&content);
        },
        "panic" => {
            // Test the Exception Handler
            panic!("Manual Panic Requested by Architect!");
        },
        "gneiss" => {
             console.println("Gneiss is Home.");
        },
        "usbinfo" => {
            for line in crate::drivers::xhci::usb_summary() {
                console.println(&line);
            }
        },
        "fatinfo" => {
            match crate::fs::fat::mount() {
                Ok(fs) => console.println(&fs.describe()),
                Err(e) => console.println(&alloc::format!("fatinfo: no FAT filesystem ({:?})", e)),
            }
        },
        "ls" | "dir" => {
            // JD4: `ls` lists the cwd; `ls <dir>` any path (absolute or cwd-relative). An `ls` of
            // a plain file prints its one table line (the DOS idiom), not an error.
            let path = normalize_path(&cwd_path(), args.first().copied().unwrap_or("."));
            match crate::fs::fat::mount() {
                Ok(fs) => match resolve_path(&fs, &path) {
                    Ok(Resolved::Root) => match fs.read_dir(0) {
                        Ok(entries) => print_dir_listing(console, &entries),
                        Err(e) => console.println(&alloc::format!("ls: /: read failed ({:?}, -EIO)", e)),
                    },
                    Ok(Resolved::Entry(de, canon)) => {
                        if de.is_dir {
                            match fs.read_dir(de.first_cluster()) {
                                Ok(entries) => print_dir_listing(console, &entries),
                                Err(e) => console.println(&alloc::format!(
                                    "ls: {}: read failed ({:?}, -EIO)", canon, e)),
                            }
                        } else {
                            console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
                        }
                    }
                    Err(msg) => console.println(&alloc::format!("ls: {}", msg)),
                },
                Err(e) => console.println(&alloc::format!("ls: no FAT filesystem ({:?})", e)),
            }
        },
        "cd" => {
            // JD4: change the shell's working directory. No argument (or `/`) returns to the
            // root. The stored cwd is the CANONICAL on-disk spelling of the resolved path.
            let path = normalize_path(&cwd_path(), args.first().copied().unwrap_or("/"));
            match crate::fs::fat::mount() {
                Ok(fs) => match resolve_path(&fs, &path) {
                    Ok(Resolved::Root) => {
                        *CWD.lock() = None;
                        console.println("/");
                    }
                    Ok(Resolved::Entry(de, canon)) => {
                        if de.is_dir {
                            console.println(&canon);
                            *CWD.lock() = Some(canon);
                        } else {
                            console.println(&alloc::format!(
                                "cd: {}: not a directory (-ENOTDIR)", canon));
                        }
                    }
                    Err(msg) => console.println(&alloc::format!("cd: {}", msg)),
                },
                Err(e) => console.println(&alloc::format!("cd: no FAT filesystem ({:?})", e)),
            }
        },
        "pwd" => {
            console.println(&cwd_path());
        },
        "cat" | "type" => {
            // JD4: `cat` takes a path (absolute or cwd-relative), e.g. `cat DOCS/README.TXT`.
            match args.first() {
                None => console.println("usage: cat <path>"),
                Some(name) => match crate::fs::fat::mount() {
                    Ok(fs) => match resolve_path(&fs, &normalize_path(&cwd_path(), name)) {
                        Ok(Resolved::Root) =>
                            console.println("cat: /: is a directory (-EISDIR)"),
                        Ok(Resolved::Entry(de, canon)) => {
                            // Bound the read so a huge file (e.g. kernel.elf) can't flood the console.
                            const CAP: usize = 8192;
                            let mut data: Vec<u8> = Vec::new();
                            match fs.read_file(&de, &mut data, CAP) {
                                Ok(()) => {
                                    // Render printable ASCII; keep LF, drop CR, others -> '.'.
                                    let text: String = data.iter().filter_map(|&b| match b {
                                        b'\n' => Some('\n'),
                                        b'\r' => None,
                                        0x20..=0x7e => Some(b as char),
                                        _ => Some('.'),
                                    }).collect();
                                    for line in text.split('\n') {
                                        console.println(line);
                                    }
                                    if (de.size as usize) > data.len() {
                                        console.println(&alloc::format!(
                                            "[... {} of {} bytes shown]", data.len(), de.size));
                                    }
                                }
                                Err(FatError::IsDirectory) => console.println(&alloc::format!(
                                    "cat: {}: is a directory (-EISDIR)", canon)),
                                Err(e) => console.println(&alloc::format!("cat: {}: {:?}", canon, e)),
                            }
                        }
                        Err(msg) => console.println(&alloc::format!("cat: {}", msg)),
                    },
                    Err(e) => console.println(&alloc::format!("cat: no FAT filesystem ({:?})", e)),
                },
            }
        },
        "touch" => {
            // JD5: create a 0-length ROOT file if absent (idempotent). `touch <path>`.
            match args.first() {
                None => console.println("usage: touch <path>"),
                Some(name) => fs_touch(console, name),
            }
        },
        "append" => {
            // JD5: append text at EOF, creating the file if absent (like `>>`). `append <path> <text>`.
            match args.first() {
                None => console.println("usage: append <path> <text>"),
                Some(name) => fs_append(console, name, args[1..].join(" ").as_bytes()),
            }
        },
        "rm" | "del" => {
            // JD5: delete a ROOT file. `rm <path>`.
            match args.first() {
                None => console.println("usage: rm <path>"),
                Some(name) => fs_rm(console, name),
            }
        },
        "diskinfo" => {
            match crate::drivers::block::info() {
                Some(d) => {
                    let vendor = core::str::from_utf8(&d.vendor).unwrap_or("?");
                    let product = core::str::from_utf8(&d.product).unwrap_or("?");
                    let cap_mib = (d.num_blocks * d.block_size as u64) / (1024 * 1024);
                    console.println(&alloc::format!("Disk: {} {}", vendor.trim_end(), product.trim_end()));
                    console.println(&alloc::format!("Block size: {}  Blocks: {}  Capacity: {} MiB",
                        d.block_size, d.num_blocks, cap_mib));
                }
                None => {
                    console.println("No block device ready.");
                    // Surface how far USB mass-storage enumeration/bring-up got (metal diagnosis).
                    console.println(&crate::drivers::xhci::storage_diag());
                }
            }
        },
        "read" => {
            match args.first().and_then(|s| s.parse::<u64>().ok()) {
                Some(lba) => {
                    let mut buf = [0u8; 512];
                    match crate::drivers::block::read_block(lba, &mut buf) {
                        Ok(_) => {
                            console.println(&alloc::format!("LBA {}:", lba));
                            hexdump(console, &buf[0..128]);
                        }
                        Err(e) => console.println(&alloc::format!("read error: {:?}", e)),
                    }
                }
                None => console.println("usage: read <lba>"),
            }
        },
        "write" => {
            // JD5 overload: `write <lba> <byte>` (raw block write) IFF exactly two args parse as a
            // <u64 lba> <byte 0..=255> pair — byte-identical to the pre-JD5 behaviour. Any other
            // shape is a FILE write `write <path> <text...>` (create-or-truncate; text = the rest of
            // the line, whitespace-collapsed like `echo`). A numeric filename can still be reached as
            // `/NAME` (an absolute path never parses as an LBA).
            let raw = if args.len() == 2 {
                match (args[0].parse::<u64>().ok(), parse_byte(args[1])) {
                    (Some(lba), Some(b)) => Some((lba, b)),
                    _ => None,
                }
            } else {
                None
            };
            match raw {
                Some((lba, b)) => {
                    let buf = [b; 512];
                    match crate::drivers::block::write_block(lba, &buf) {
                        Ok(()) => console.println(&alloc::format!("wrote LBA {} (0x{:02x} x512)", lba, b)),
                        Err(e) => console.println(&alloc::format!("write error: {:?}", e)),
                    }
                }
                None if args.is_empty() =>
                    console.println("usage: write <path> <text>  |  write <lba> <byte>"),
                None => fs_write(console, args[0], args[1..].join(" ").as_bytes()),
            }
        },
        "netinfo" => {
            match crate::drivers::e1000::info() {
                Some(n) => {
                    console.println(&alloc::format!(
                        "NIC: MAC {}  link {}",
                        crate::drivers::e1000::fmt_mac(&n.mac),
                        if n.link_up { "UP" } else { "DOWN" }
                    ));
                    console.println(&alloc::format!(
                        "BAR0 {:#x}  RX frames: {}  TX frames: {}  IRQs: {}",
                        n.mmio_base, n.rx_count, n.tx_count, n.irq_count
                    ));
                    console.println(&alloc::format!("TCP listener (:7) active conns: {}", n.tcp_conns));
                }
                None => console.println("No network device ready."),
            }
        },
        "ping" => {
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => {
                    let count = args.get(1)
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(4)
                        .clamp(1, 16);
                    console.println(&alloc::format!(
                        "PING {}.{}.{}.{} ({} requests)", ip[0], ip[1], ip[2], ip[3], count));
                    // Blocks while it ARP-resolves the target and waits for each reply.
                    match crate::drivers::e1000::ping(ip, count) {
                        Some(o) if o.resolved => {
                            let peer = o.mac
                                .map(|m| crate::drivers::e1000::fmt_mac(&m))
                                .unwrap_or_default();
                            console.println(&alloc::format!(
                                "{}/{} replies received (peer {})", o.received, o.sent, peer));
                        }
                        Some(_) => console.println("host unreachable (no ARP reply)"),
                        None => console.println("No network device ready."),
                    }
                }
                None => console.println("usage: ping <a.b.c.d> [count]"),
            }
        },
        "arp" => {
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => match crate::drivers::e1000::arp_resolve(ip) {
                    Some(mac) => console.println(&alloc::format!(
                        "{}.{}.{}.{} is-at {}",
                        ip[0], ip[1], ip[2], ip[3], crate::drivers::e1000::fmt_mac(&mac))),
                    None => console.println("no ARP reply (host unreachable / no NIC)"),
                },
                None => console.println("usage: arp <a.b.c.d>"),
            }
        },
        "connect" => {
            let ip = args.first().and_then(|s| parse_ipv4(s));
            let port = args.get(1).and_then(|s| s.parse::<u16>().ok());
            match (ip, port) {
                (Some(ip), Some(port)) => {
                    // Optional message; if omitted, just open and immediately close.
                    let msg = if args.len() > 2 { args[2..].join(" ") } else { String::new() };
                    console.println(&alloc::format!(
                        "CONNECT {}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port));
                    // Blocks while it ARP-resolves, handshakes, exchanges, and closes.
                    match crate::drivers::e1000::connect(ip, port, msg.as_bytes()) {
                        Some(o) if o.established => {
                            console.println(&alloc::format!(
                                "established; {} bytes received; closed={}", o.rx_len, o.closed));
                        }
                        Some(o) if !o.resolved => console.println("host unreachable (no ARP reply)"),
                        Some(_) => console.println("connection refused / no response"),
                        None => console.println("No network device ready."),
                    }
                }
                _ => console.println("usage: connect <a.b.c.d> <port> [message]"),
            }
        },
        "udpsend" => {
            let ip = args.first().and_then(|s| parse_ipv4(s));
            let port = args.get(1).and_then(|s| s.parse::<u16>().ok());
            match (ip, port) {
                (Some(ip), Some(port)) => {
                    let msg = if args.len() > 2 { args[2..].join(" ") } else { String::from("unaos-udp") };
                    console.println(&alloc::format!(
                        "UDP {}.{}.{}.{}:{} <- {:?}", ip[0], ip[1], ip[2], ip[3], port, msg));
                    match crate::drivers::e1000::udp_send(ip, port, msg.as_bytes()) {
                        Some(o) if o.sent => {
                            if o.replied {
                                console.println(&alloc::format!("reply: {} bytes", o.rx_len));
                            } else {
                                console.println("sent; no reply (UDP is best-effort)");
                            }
                        }
                        Some(_) => console.println("host unreachable (no ARP reply)"),
                        None => console.println("No network device ready."),
                    }
                }
                _ => console.println("usage: udpsend <a.b.c.d> <port> [message]"),
            }
        },
        "get" => {
            // Minimal HTTP/1.0 GET over the streaming TCP client: connect, send the request,
            // read the whole response until the server closes, and print it.
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => {
                    let port = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(80);
                    let path = if args.len() > 2 { String::from(args[2]) } else { String::from("/") };
                    let req = alloc::format!(
                        "GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nConnection: close\r\n\r\n",
                        path, ip[0], ip[1], ip[2], ip[3]);
                    console.println(&alloc::format!(
                        "GET http://{}.{}.{}.{}:{}{}", ip[0], ip[1], ip[2], ip[3], port, path));
                    match crate::drivers::e1000::fetch(ip, port, req.as_bytes()) {
                        Some((o, body)) if o.established => {
                            console.println(&alloc::format!(
                                "--- {} bytes received; closed={} ---", o.rx_len, o.closed));
                            // Render printable ASCII; drop CR, keep LF as line breaks.
                            let text: String = body.iter().filter_map(|&b| match b {
                                b'\n' => Some('\n'),
                                b'\r' => None,
                                0x20..=0x7e => Some(b as char),
                                _ => Some('.'),
                            }).collect();
                            for line in text.split('\n') {
                                console.println(line);
                            }
                        }
                        Some((o, _)) if !o.resolved => console.println("host unreachable (no ARP reply)"),
                        Some(_) => console.println("connection refused / no response"),
                        None => console.println("No network device ready."),
                    }
                }
                None => console.println("usage: get <a.b.c.d> [port] [path]"),
            }
        },
        "vug" => {
             match args.first().copied() {
                 Some("bebox") => {
                     console.println("Vug: BeBox tribute (press any key)...");
                     vug::run_bebox_mode(pal);
                     // Tribute screen stays up; `took_screen` keeps the console off it.
                 }
                 Some("wire") => {
                     console.println("Vug: sculpting the quartz (wireframe)...");
                     vug::run_crystal(pal, vug::Mode::Wire);
                     console.draw(pal); // clean exit: restore the shell over the demo
                 }
                 _ => {
                     console.println("Vug: sculpting the quartz (solid)...");
                     vug::run_crystal(pal, vug::Mode::Solid);
                     console.draw(pal); // clean exit: restore the shell over the demo
                 }
             }
        },
        "tste" | "selftest" => {
            // The in-OS self-test suite (TSTE-1). Prints a three-section PASS/FAIL/SKIP table in the
            // console (like `ps` — it does NOT take the screen) and mirrors every line to serial.
            crate::selftest::run(console, pal);
        },
        "sched" | "ps" => {
            #[cfg(target_arch = "x86_64")]
            {
                let count = core::cmp::min(
                    crate::arch::acpi::cpu_count().max(1),
                    crate::arch::gdt::MAX_CPUS,
                );
                console.println("CPU  role  current  run-queue");
                for cpu in 0..count {
                    let role = if cpu == 0 { "bsp" } else { "ap " };
                    let cur = match crate::arch::sched::current_task_id(cpu) {
                        Some(id) => alloc::format!("tid {}", id),
                        None => "-".into(),
                    };
                    console.println(&alloc::format!(
                        "{:>3}  {}   {:<8} {}",
                        cpu, role, cur, crate::arch::sched::run_queue_len(cpu)
                    ));
                }
                console.println(&alloc::format!(
                    "demo tasks finished: {}", crate::arch::sched::demo_done()));
            }
            #[cfg(not(target_arch = "x86_64"))]
            console.println("sched: x86_64 only");
        },
        "shutdown" | "off" => {
             // TODO: Create arch::shutdown()
             serial_println!("Shutdown requested");
             crate::hlt_loop();
             crate::hlt_loop();
        },
        "" => {}, // Ignore empty enter
        _ => {
            console.println("Unknown command. Type 'help' for assistance.");
        }
    }

    took_screen
}

/// Print `data` as a classic hex dump (offset, 16 hex bytes, ASCII gutter) to the console.
fn hexdump(console: &mut Console, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let mut line = alloc::format!("{:04x}: ", i * 16);
        for b in chunk {
            line.push_str(&alloc::format!("{:02x} ", b));
        }
        line.push_str(" |");
        for b in chunk {
            let c = if *b >= 32 && *b < 127 { *b as char } else { '.' };
            line.push(c);
        }
        line.push('|');
        console.println(&line);
    }
}

/// Parse a dotted-quad IPv4 address (`a.b.c.d`) into 4 octets. Rejects anything that
/// isn't exactly four decimal octets in 0..=255.
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in s.split('.') {
        if count >= 4 {
            return None;
        }
        octets[count] = part.parse::<u8>().ok()?;
        count += 1;
    }
    if count == 4 {
        Some(octets)
    } else {
        None
    }
}

/// Parse a byte literal in decimal or `0x..` hex form.
fn parse_byte(s: &str) -> Option<u8> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u8>().ok()
    }
}
