// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `DiskList` — READ-ONLY enumeration of the host's block devices from sysfs (`/sys/block/*`:
//! `size` in 512-byte units, `queue/logical_block_size`, `removable`, `ro`, `device/model`,
//! `device/vendor`, the partition subdirectories) and the mount table (`/proc/self/mounts`). No
//! device node is opened. The roots are parameters so the tests run against a synthetic tree.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One block device as sysfs describes it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Kernel name (`sda`, `nvme0n1`, `mmcblk0`).
    pub name: String,
    /// The node under the device root (`/dev/sda`).
    pub dev: PathBuf,
    /// 512-byte sectors (sysfs `size` is always in 512-byte units).
    pub sectors: u64,
    pub logical_block_size: u32,
    pub removable: bool,
    pub read_only: bool,
    /// `loop`, `ram`, `zram`, `dm-`, `md`, `sr` — not a disk an installer partitions.
    pub virtual_dev: bool,
    pub model: String,
    pub vendor: String,
    /// Partition names (`sda1`, `nvme0n1p2`).
    pub partitions: Vec<String>,
    /// Mount points of the disk or any of its partitions.
    pub mounts: Vec<String>,
}

impl DiskInfo {
    pub fn bytes(&self) -> u64 {
        self.sectors * 512
    }
    /// One line for `amber list`.
    pub fn line(&self) -> String {
        let mut flags = Vec::new();
        if self.removable {
            flags.push("removable");
        }
        if self.read_only {
            flags.push("ro");
        }
        if self.virtual_dev {
            flags.push("virtual");
        }
        if !self.mounts.is_empty() {
            flags.push("MOUNTED");
        }
        format!(
            "{:<10} {:>12} sectors {:>8} MiB lbs {:<4} {:<24} parts [{}] {}{}",
            self.dev.display(),
            self.sectors,
            self.bytes() >> 20,
            self.logical_block_size,
            format!("{} {}", self.vendor, self.model).trim(),
            self.partitions.join(" "),
            flags.join(","),
            if self.mounts.is_empty() { String::new() } else { format!(" on {}", self.mounts.join(" ")) }
        )
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

const VIRTUAL: &[&str] = &["loop", "ram", "zram", "dm-", "md", "sr", "nbd"];

/// Enumerate `sys_block` (normally `/sys/block`), naming nodes under `dev_root` (`/dev`) and taking
/// mounts from `mounts` (the text of `/proc/self/mounts`).
pub fn list_from(sys_block: &Path, dev_root: &Path, mounts: &str) -> std::io::Result<Vec<DiskInfo>> {
    let mut out = Vec::new();
    let mut names: Vec<String> = fs::read_dir(sys_block)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let d = sys_block.join(&name);
        let sectors = read_trim(&d.join("size")).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut partitions: Vec<String> = fs::read_dir(&d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().join("partition").is_file())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        partitions.sort();
        let dev = dev_root.join(&name);
        let mut ms = Vec::new();
        for l in mounts.lines() {
            let mut w = l.split_whitespace();
            let (Some(src), Some(at)) = (w.next(), w.next()) else { continue };
            let src = Path::new(src);
            if src == dev || partitions.iter().any(|p| src == dev_root.join(p)) {
                ms.push(at.to_string());
            }
        }
        out.push(DiskInfo {
            dev,
            sectors,
            logical_block_size: read_trim(&d.join("queue/logical_block_size")).and_then(|s| s.parse().ok()).unwrap_or(512),
            removable: read_trim(&d.join("removable")).as_deref() == Some("1"),
            read_only: read_trim(&d.join("ro")).as_deref() == Some("1"),
            virtual_dev: VIRTUAL.iter().any(|v| name.starts_with(v)),
            model: read_trim(&d.join("device/model")).unwrap_or_default(),
            vendor: read_trim(&d.join("device/vendor")).unwrap_or_default(),
            partitions,
            mounts: ms,
            name,
        });
    }
    Ok(out)
}

/// The host's disks (`/sys/block`, `/dev`, `/proc/self/mounts`).
pub fn list() -> std::io::Result<Vec<DiskInfo>> {
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    list_from(Path::new("/sys/block"), Path::new("/dev"), &mounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn synthetic_sysfs() {
        let root = std::env::temp_dir().join(format!("amber-sysfs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let sb = root.join("block");
        put(&sb.join("sda/size"), "1953525168\n");
        put(&sb.join("sda/queue/logical_block_size"), "512\n");
        put(&sb.join("sda/removable"), "0\n");
        put(&sb.join("sda/ro"), "0\n");
        put(&sb.join("sda/device/model"), "Samsung SSD 870 \n");
        put(&sb.join("sda/device/vendor"), "ATA\n");
        put(&sb.join("sda/sda1/partition"), "1\n");
        put(&sb.join("sda/sda2/partition"), "2\n");
        put(&sb.join("mmcblk0/size"), "62333952\n");
        put(&sb.join("mmcblk0/removable"), "1\n");
        put(&sb.join("loop0/size"), "0\n");
        let mounts = "/dev/sda2 / ext4 rw 0 0\nproc /proc proc rw 0 0\n";
        let l = list_from(&sb, Path::new("/dev"), mounts).unwrap();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(l.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["loop0", "mmcblk0", "sda"]);
        let sda = &l[2];
        assert_eq!((sda.sectors, sda.logical_block_size, sda.removable), (1_953_525_168, 512, false));
        assert_eq!(sda.partitions, ["sda1", "sda2"]);
        assert_eq!(sda.mounts, ["/"]);
        assert_eq!(sda.model, "Samsung SSD 870");
        assert!(l[0].virtual_dev && l[1].removable && !l[1].virtual_dev);
        assert!(sda.line().contains("MOUNTED"));
    }
}
