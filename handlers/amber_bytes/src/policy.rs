// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The WRITE POLICY — who may open what for writing. Reads are always allowed (a `FileBlock` opened
//! read-only). A write is allowed to:
//!
//! * a REGULAR FILE (a disk image) — always;
//! * a BLOCK DEVICE — only when BOTH keys turn: the operator said `--yes-i-mean-it`
//!   ([`Policy::yes_i_mean_it`]) AND the device's canonical path is on the allowlist
//!   ([`Policy::allow`]: `--allow <dev>` or `AMBER_ALLOW=/dev/sdX:/dev/sdY`) — and never while the
//!   device or one of its partitions is mounted.
//!
//! Anything else (a directory, a character device, a FIFO, a missing path) is refused. The policy
//! decides on the canonical path, so a symlink cannot smuggle `/dev/sda` past an allowlist entry
//! for `/dev/sdb`.

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use amber_core::file_block::FileBlock;

#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub yes_i_mean_it: bool,
    pub allow: Vec<PathBuf>,
    /// The mount table text (`/proc/self/mounts`); empty = read it at decision time.
    pub mounts: Option<String>,
}

/// What the target is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetKind {
    Image,
    BlockDevice,
}

impl Policy {
    /// `AMBER_ALLOW` (colon separated) plus `extra`.
    pub fn from_env(yes_i_mean_it: bool, extra: &[PathBuf]) -> Self {
        let mut allow: Vec<PathBuf> = std::env::var_os("AMBER_ALLOW")
            .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        allow.extend(extra.iter().cloned());
        Self { yes_i_mean_it, allow, mounts: None }
    }

    /// Decide whether `path` may be written; `Ok` names what it is.
    pub fn check_write(&self, path: &Path) -> Result<(TargetKind, PathBuf), String> {
        let canon = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
        let ft = std::fs::metadata(&canon).map_err(|e| format!("{}: {e}", canon.display()))?.file_type();
        if ft.is_file() {
            return Ok((TargetKind::Image, canon));
        }
        if !ft.is_block_device() {
            return Err(format!("{}: neither a disk image nor a block device — refused", canon.display()));
        }
        if !self.yes_i_mean_it {
            return Err(format!("{} is a real disk: writing it needs --yes-i-mean-it AND an allowlist entry", canon.display()));
        }
        let listed = self.allow.iter().any(|a| a.canonicalize().map(|c| c == canon).unwrap_or(false));
        if !listed {
            return Err(format!("{} is not on the allowlist (--allow or AMBER_ALLOW) — refused", canon.display()));
        }
        let mounts = self.mounts.clone().unwrap_or_else(|| std::fs::read_to_string("/proc/self/mounts").unwrap_or_default());
        let me = canon.to_string_lossy().into_owned();
        for l in mounts.lines() {
            if let Some(src) = l.split_whitespace().next() {
                // `/dev/sda` matches `/dev/sda`, `/dev/sda1`, `/dev/nvme0n1p2` for `/dev/nvme0n1`.
                if src == me || (src.starts_with(&me) && src[me.len()..].trim_start_matches('p').bytes().all(|b| b.is_ascii_digit())) {
                    return Err(format!("{} (or a partition of it) is mounted ({src}) — refused", canon.display()));
                }
            }
        }
        Ok((TargetKind::BlockDevice, canon))
    }

    /// Open `path` for writing under the policy.
    pub fn open_rw(&self, path: &Path) -> Result<(TargetKind, FileBlock), String> {
        let (k, canon) = self.check_write(path)?;
        let fb = FileBlock::open_rw(&canon).map_err(|e| format!("{}: {e}", canon.display()))?;
        Ok((k, fb))
    }
}

/// Open `path` read-only (always allowed).
pub fn open_ro(path: &Path) -> Result<FileBlock, String> {
    FileBlock::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_yes_everything_else_no() {
        let p = std::env::temp_dir().join(format!("amber-policy-{}.img", std::process::id()));
        std::fs::write(&p, [0u8; 1024]).unwrap();
        let pol = Policy::default();
        assert_eq!(pol.check_write(&p).unwrap().0, TargetKind::Image);
        assert!(pol.check_write(&std::env::temp_dir()).is_err(), "a directory");
        assert!(pol.check_write(Path::new("/dev/null")).is_err(), "a character device");
        assert!(pol.check_write(Path::new("/nonexistent/amber")).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// A block device, when the container has one: refused without the flag, refused unlisted,
    /// refused mounted. (Never opened for writing by this test.)
    #[test]
    fn block_devices_need_both_keys() {
        let Some(dev) = ["/dev/vda", "/dev/sda", "/dev/nvme0n1", "/dev/loop0"]
            .iter()
            .map(Path::new)
            .find(|p| std::fs::metadata(p).map(|m| m.file_type().is_block_device()).unwrap_or(false))
        else {
            eprintln!("SKIP: no block device node in this container");
            return;
        };
        let canon = dev.canonicalize().unwrap();
        let no_flag = Policy { yes_i_mean_it: false, allow: vec![canon.clone()], mounts: Some(String::new()) };
        assert!(no_flag.check_write(dev).unwrap_err().contains("--yes-i-mean-it"));
        let unlisted = Policy { yes_i_mean_it: true, allow: vec![], mounts: Some(String::new()) };
        assert!(unlisted.check_write(dev).unwrap_err().contains("allowlist"));
        let mounted = Policy {
            yes_i_mean_it: true,
            allow: vec![canon.clone()],
            mounts: Some(format!("{}1 / ext4 rw 0 0\n", canon.display())),
        };
        assert!(mounted.check_write(dev).unwrap_err().contains("mounted"));
        let both = Policy { yes_i_mean_it: true, allow: vec![canon.clone()], mounts: Some(String::new()) };
        assert_eq!(both.check_write(dev).unwrap().0, TargetKind::BlockDevice);
    }
}
