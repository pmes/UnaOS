// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `DiskList` — READ-ONLY enumeration of the host's block devices from sysfs (`/sys/block/*`:
//! `size` in 512-byte units, `queue/logical_block_size`, `removable`, `ro`, `device/model`,
//! `device/vendor`, the partition subdirectories) and the mount table (`/proc/self/mounts`). No
//! device node is opened. The roots are parameters so the tests run against a synthetic tree.
//!
//! On macOS (AMBERPLIST, LEDGER SR417) the same `DiskInfo` comes from `diskutil list -plist external
//! physical` plus one `diskutil info -plist <id>` per whole disk, read by the XML plist subset in
//! [`plist`]; [`list_from_plists`] is the pure reader the tests drive. ONE [`list`] answers on both.

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
    /// On an internal bus / fixed (AMBERPLIST SR417). macOS: `Internal` and not removable media (an
    /// internal SD slot is a card); Linux: `!removable` (sysfs carries no bus word).
    #[serde(default)]
    pub internal: bool,
    /// The OS flags the disk as its own (macOS `OSInternalMedia` / the list's `OSInternal`); Linux
    /// `false` (the mount table answers).
    #[serde(default)]
    pub os_internal: bool,
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
        let removable = read_trim(&d.join("removable")).as_deref() == Some("1");
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
            removable,
            read_only: read_trim(&d.join("ro")).as_deref() == Some("1"),
            virtual_dev: VIRTUAL.iter().any(|v| name.starts_with(v)),
            model: read_trim(&d.join("device/model")).unwrap_or_default(),
            vendor: read_trim(&d.join("device/vendor")).unwrap_or_default(),
            partitions,
            mounts: ms,
            internal: !removable,
            os_internal: false,
            name,
        });
    }
    Ok(out)
}

/// The host's disks: Linux `/sys/block`, `/dev`, `/proc/self/mounts`; macOS `diskutil -plist`.
pub fn list() -> std::io::Result<Vec<DiskInfo>> {
    if cfg!(target_os = "macos") {
        return list_diskutil();
    }
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    list_from(Path::new("/sys/block"), Path::new("/dev"), &mounts)
}

/// macOS: `diskutil list -plist external physical` + one `diskutil info -plist <id>` per whole disk.
fn list_diskutil() -> std::io::Result<Vec<DiskInfo>> {
    let list = run_out("diskutil", &["list", "-plist", "external", "physical"])?;
    let infos: Vec<(String, String)> = whole_disks(&list)
        .into_iter()
        .filter_map(|id| run_out("diskutil", &["info", "-plist", &id]).ok().map(|t| (id, t)))
        .collect();
    Ok(list_from_plists(&list, &infos))
}

fn run_out(cmd: &str, args: &[&str]) -> std::io::Result<String> {
    let o = std::process::Command::new(cmd).args(args).output()?;
    if !o.status.success() {
        return Err(std::io::Error::other(format!("{cmd} {} failed", args.join(" "))));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The whole-disk ids (`WholeDisks`) of a `diskutil list -plist` text.
pub fn whole_disks(list: &str) -> Vec<String> {
    use plist::Pv;
    plist::parse_plist(list)
        .and_then(|r| r.get("WholeDisks").map(|w| w.items().iter().filter_map(Pv::str).map(str::to_string).collect()))
        .unwrap_or_default()
}

/// macOS: `diskutil list -plist external physical` + one `diskutil info -plist <id>` per whole disk
/// (`infos` maps the id to that text). A disk with no info is judged internal, never guessed removable.
pub fn list_from_plists(list: &str, infos: &[(String, String)]) -> Vec<DiskInfo> {
    use plist::{Pv, parse_plist};
    let Some(root) = parse_plist(list) else { return Vec::new() };
    let mut out = Vec::new();
    for whole in root.get("AllDisksAndPartitions").map(Pv::items).unwrap_or_default() {
        let Some(id) = whole.get("DeviceIdentifier").and_then(Pv::str) else { continue };
        let mut mounts = Vec::new();
        let mut partitions = Vec::new();
        if let Some(m) = whole.get("MountPoint").and_then(Pv::str).filter(|m| !m.is_empty()) {
            mounts.push(m.to_string());
        }
        for key in ["Partitions", "APFSVolumes"] {
            for p in whole.get(key).map(Pv::items).unwrap_or_default() {
                if let Some(m) = p.get("MountPoint").and_then(Pv::str).filter(|m| !m.is_empty()) {
                    mounts.push(m.to_string());
                }
                if let Some(pid) = p.get("DeviceIdentifier").and_then(Pv::str) {
                    partitions.push(pid.to_string());
                }
            }
        }
        let info = infos.iter().find(|(k, _)| k == id).and_then(|(_, t)| parse_plist(t));
        let flag = |k: &str| info.as_ref().and_then(|i| i.get(k)).and_then(Pv::bool);
        let internal = flag("Internal").unwrap_or(true);
        let removable = flag("RemovableMedia").or(flag("Removable")).unwrap_or(false) || (!internal && flag("Ejectable").unwrap_or(false));
        let os_internal = flag("OSInternalMedia").unwrap_or(false) || whole.get("OSInternal").and_then(Pv::bool).unwrap_or(false);
        let bytes = info
            .as_ref()
            .and_then(|i| i.get("TotalSize").or(i.get("Size")))
            .and_then(Pv::int)
            .or(whole.get("Size").and_then(Pv::int))
            .unwrap_or(0);
        let dev = info.as_ref().and_then(|i| i.get("DeviceNode")).and_then(Pv::str).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/dev/{id}")));
        let model = info.as_ref().and_then(|i| i.get("MediaName")).and_then(Pv::str).unwrap_or("").trim().to_string();
        let lbs = info.as_ref().and_then(|i| i.get("DeviceBlockSize")).and_then(Pv::int).and_then(|b| u32::try_from(b).ok()).unwrap_or(512);
        let read_only = flag("WritableMedia").or(flag("Writable")).map(|w| !w).unwrap_or(false);
        let virtual_dev = info.as_ref().and_then(|i| i.get("VirtualOrPhysical")).and_then(Pv::str) == Some("Virtual");
        out.push(DiskInfo {
            name: id.to_string(),
            dev,
            sectors: bytes / 512,
            logical_block_size: lbs,
            removable,
            read_only,
            virtual_dev,
            model,
            vendor: String::new(),
            partitions,
            mounts,
            // An internal SD slot (RemovableMedia on an Internal bus) is a card; an internal fixed disk is not.
            internal: internal && !removable,
            os_internal,
        });
    }
    out
}

/// The XML property-list subset `diskutil -plist` speaks (dict / array / string / integer / true / false).
pub mod plist {
    /// A plist value; `Other` is any element the reader never reads (`real`, `data`, `date`).
    #[derive(Clone, Debug, PartialEq)]
    pub enum Pv {
        Dict(Vec<(String, Pv)>),
        Array(Vec<Pv>),
        Str(String),
        Int(i64),
        Bool(bool),
        Other,
    }

    impl Pv {
        pub fn get(&self, key: &str) -> Option<&Pv> {
            match self {
                Pv::Dict(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }
        pub fn items(&self) -> &[Pv] {
            match self {
                Pv::Array(v) => v,
                _ => &[],
            }
        }
        pub fn str(&self) -> Option<&str> {
            match self {
                Pv::Str(s) => Some(s),
                _ => None,
            }
        }
        pub fn int(&self) -> Option<u64> {
            match self {
                Pv::Int(i) => u64::try_from(*i).ok(),
                _ => None,
            }
        }
        pub fn bool(&self) -> Option<bool> {
            match self {
                Pv::Bool(b) => Some(*b),
                _ => None,
            }
        }
    }

    /// Parse an XML property list's root value.
    pub fn parse_plist(text: &str) -> Option<Pv> {
        let start = text.find("<plist")?;
        let mut p = Px { s: text, i: start };
        p.tag()?; // <plist version="1.0">
        p.value()
    }

    struct Px<'a> {
        s: &'a str,
        i: usize,
    }

    impl<'a> Px<'a> {
        /// The next tag's inside (`dict`, `/dict`, `true/`), skipping text, comments and declarations.
        fn tag(&mut self) -> Option<&'a str> {
            loop {
                let open = self.s[self.i..].find('<')? + self.i;
                if self.s[open..].starts_with("<!--") {
                    self.i = self.s[open..].find("-->")? + open + 3;
                    continue;
                }
                let close = self.s[open..].find('>')? + open;
                self.i = close + 1;
                let inner = &self.s[open + 1..close];
                if inner.starts_with('?') || inner.starts_with('!') {
                    continue;
                }
                return Some(inner.trim());
            }
        }
        /// Text up to `</name>`, consumed with the closing tag.
        fn text(&mut self, name: &str) -> Option<String> {
            let end = format!("</{name}>");
            let at = self.s[self.i..].find(&end)? + self.i;
            let raw = &self.s[self.i..at];
            self.i = at + end.len();
            Some(unescape(raw))
        }
        fn value(&mut self) -> Option<Pv> {
            let t = self.tag()?;
            self.value_from(t)
        }
        fn value_from(&mut self, t: &str) -> Option<Pv> {
            let name = t.split_whitespace().next().unwrap_or("");
            let empty = t.ends_with('/');
            let name = name.trim_end_matches('/');
            match name {
                "true" => Some(Pv::Bool(true)),
                "false" => Some(Pv::Bool(false)),
                "dict" if empty => Some(Pv::Dict(Vec::new())),
                "array" if empty => Some(Pv::Array(Vec::new())),
                "string" if empty => Some(Pv::Str(String::new())),
                "string" => self.text("string").map(Pv::Str),
                "integer" => self.text("integer").and_then(|s| s.trim().parse().ok()).map(Pv::Int),
                "dict" => {
                    let mut kv = Vec::new();
                    loop {
                        let t = self.tag()?;
                        if t == "/dict" {
                            return Some(Pv::Dict(kv));
                        }
                        if t != "key" {
                            return None;
                        }
                        let k = self.text("key")?;
                        let v = self.value()?;
                        kv.push((k, v));
                    }
                }
                "array" => {
                    let mut v = Vec::new();
                    loop {
                        let t = self.tag()?;
                        if t == "/array" {
                            return Some(Pv::Array(v));
                        }
                        v.push(self.value_from(t)?);
                    }
                }
                _ if empty => Some(Pv::Other),
                "" => None,
                other => {
                    self.text(other)?;
                    Some(Pv::Other)
                }
            }
        }
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
    }
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
        assert!(sda.internal && !l[1].internal && !sda.os_internal);
    }

    const LIST: &str = include_str!("../tests/fixtures/diskutil-list-external.plist");
    const DISK4: &str = include_str!("../tests/fixtures/diskutil-info-disk4.plist");
    const DISK0: &str = include_str!("../tests/fixtures/diskutil-info-disk0.plist");

    #[test]
    fn plist_parses_diskutil_shapes() {
        use plist::{Pv, parse_plist};
        let r = parse_plist(LIST).unwrap();
        let whole: Vec<&str> = r.get("WholeDisks").unwrap().items().iter().filter_map(Pv::str).collect();
        assert_eq!(whole, ["disk0", "disk4"]);
        assert_eq!(whole_disks(LIST), ["disk0", "disk4"]);
        let i = parse_plist(DISK4).unwrap();
        assert_eq!(i.get("MediaName").and_then(Pv::str), Some("SD Card Reader"));
        assert_eq!(i.get("Size").and_then(Pv::int), Some(63_864_569_856));
        assert_eq!(i.get("MountPoint").and_then(Pv::str), Some(""));
        assert_eq!(i.get("Internal").and_then(Pv::bool), Some(false));
        assert_eq!(parse_plist("<plist><string>a &amp; b</string></plist>"), Some(Pv::Str("a & b".into())));
        assert_eq!(parse_plist("not a plist"), None);
        assert!(whole_disks("not a plist").is_empty());
    }

    #[test]
    fn macos_disks_from_plists() {
        let infos = vec![("disk0".to_string(), DISK0.to_string()), ("disk4".to_string(), DISK4.to_string())];
        let d = list_from_plists(LIST, &infos);
        assert_eq!(d.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["disk0", "disk4"]);
        let (ssd, sd) = (&d[0], &d[1]);
        assert!(ssd.os_internal && ssd.internal && !ssd.removable);
        assert_eq!((ssd.logical_block_size, ssd.bytes()), (4096, 500_277_790_720));
        assert_eq!(ssd.mounts, ["/"]);
        assert_eq!(sd.dev, PathBuf::from("/dev/disk4"));
        assert_eq!(sd.model, "SD Card Reader");
        assert_eq!(sd.mounts, ["/Volumes/UNAOS"]);
        assert_eq!(sd.partitions, ["disk4s1", "disk4s2"]);
        assert!(sd.removable && !sd.internal && !sd.os_internal && !sd.read_only && !sd.virtual_dev);
        assert_eq!((sd.logical_block_size, sd.bytes()), (512, 63_864_569_856));
        // No info for a disk: judged internal, never guessed removable.
        let blind = list_from_plists(LIST, &[]);
        assert_eq!(blind.len(), 2);
        assert!(blind.iter().all(|d| d.internal && !d.removable));
        assert!(list_from_plists("not a plist", &infos).is_empty());
    }
}
