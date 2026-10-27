//! CHARTER: Amber Bytes — the UnaOS_Installer card face (vessel module; seam shared-core amber_core + amber_bytes)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERCARD (LEDGER SR414): face (a) of the UnaOS_Installer vessel — put UnaOS on a card from a Mac or a
// Linux box. The card face ENCODES NOTHING: the image is the one `./arroyo esp-x86` lays under UNAOS_UNAFS=1
// (`unaos/target/unaos-x86-card.img`, GPT p1 ESP + p2 UnaFS, built by `tools/una-card` over amber_core); the
// devices are Amber's (`amber_bytes::disks` on Linux; `diskutil -plist` read here on macOS, AMBER-PLIST owed);
// the device is opened only through Amber's write policy (`amber_bytes::policy::Policy`: the typed confirmation
// is `yes_i_mean_it`, the picked device the sole allowlist entry, never while mounted); every byte goes through
// amber_core's `Block` (`FileBlock`) in `ClonePlan::chunks` steps of 4 MiB, then `flush` (sync_all), a read-back
// of the first and last MiB, and the eject. Windows is a named refusal (`why=windows-unbuilt`).
//
// Wire:
//   [installer] card devices n=<n> writable=<n>
//   [installer] card device=<id> size=<bytes> image=<path> written_mb=<n> verified=1|0 ejected=1|0 ms=<n>
//   [installer] card refused why=<system-disk|mounted|too-small|no-image|windows-unbuilt> device=<id|->

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use amber_core::block::Block;
use amber_core::clone::ClonePlan;
use amber_core::file_block::FileBlock;
use quartzite::tetra::{ConsoleLine, ConsoleTetra, TetraNode, TextAction};

const SECTOR: u64 = 512;
/// The write step: 8192 sectors = 4 MiB.
const CHUNK_SECTORS: u64 = 8192;
/// The verify window at each end: 2048 sectors = 1 MiB.
const VERIFY_SECTORS: u64 = 2048;

/// Mount points that mean "this disk carries the running OS".
const OS_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/home", "/var", "/System/Volumes/Data", "/System/Volumes/Preboot"];

// ── devices ─────────────────────────────────────────────────────────────────────────────────────

/// One whole disk the host can see, as the card face judges it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// `disk4` (macOS) / `sdb`, `mmcblk0` (Linux).
    pub id: String,
    /// The node the write opens (`/dev/disk4`, `/dev/sdb`).
    pub node: PathBuf,
    pub bytes: u64,
    /// Removable media (an SD card in a reader, a USB stick).
    pub removable: bool,
    /// Internal / fixed / virtual: never a card.
    pub internal: bool,
    pub model: String,
    /// Mount points of the disk or any of its partitions.
    pub mounts: Vec<String>,
    /// Carries the running OS (a root / boot / system mount, or the OS flags it as its own).
    pub holds_os: bool,
}

impl Device {
    /// The confirmation the operator types: `"<GB to one decimal> GB <id>"`, e.g. `63.9 GB disk4`.
    pub fn phrase(&self) -> String {
        format!("{} GB {}", gb1(self.bytes), self.id)
    }

    /// Why this device may not take an image of `image_bytes`; `None` = writable. `mounted` is decided only
    /// after an unmount was tried (see [`write`]).
    pub fn refusal(&self, image_bytes: u64) -> Option<&'static str> {
        if self.holds_os || self.internal || !self.removable {
            return Some("system-disk");
        }
        if self.bytes < image_bytes || self.bytes == 0 {
            return Some("too-small");
        }
        None
    }

    /// One row for the face's list.
    pub fn row(&self, image_bytes: u64) -> String {
        let mut s = format!("{:<8} {:>7} GB  {}", self.id, gb1(self.bytes), if self.model.is_empty() { "-" } else { &self.model });
        if self.removable {
            s.push_str("  removable");
        }
        if !self.mounts.is_empty() {
            s.push_str(&format!("  on {}", self.mounts.join(" ")));
        }
        if let Some(why) = self.refusal(image_bytes) {
            s.push_str(&format!("  [refused {why}]"));
        }
        s
    }
}

fn gb1(bytes: u64) -> String {
    // One decimal, rounded half up, without floats drifting: tenths of a GB (1e9).
    let tenths = (bytes + 50_000_000) / 100_000_000;
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

/// Does `typed` match the device's confirmation phrase (case and runs of spaces ignored)?
pub fn phrase_ok(dev: &Device, typed: &str) -> bool {
    !typed.trim().is_empty() && norm(typed) == norm(&dev.phrase())
}

/// Linux: Amber's sysfs enumeration, judged.
pub fn devices_from_disks(disks: Vec<amber_bytes::disks::DiskInfo>) -> Vec<Device> {
    disks
        .into_iter()
        .map(|d| {
            let holds_os = d.mounts.iter().any(|m| OS_MOUNTS.contains(&m.as_str()));
            let bytes = d.bytes();
            Device {
                internal: d.virtual_dev || !d.removable,
                model: format!("{} {}", d.vendor, d.model).trim().to_string(),
                removable: d.removable && !d.virtual_dev,
                id: d.name,
                node: d.dev,
                bytes,
                mounts: d.mounts,
                holds_os,
            }
        })
        .collect()
}

/// macOS: `diskutil list -plist external physical` + one `diskutil info -plist <id>` per whole disk
/// (`infos` maps the id to that text). A disk with no info is judged internal (refused), never guessed writable.
pub fn devices_from_plists(list: &str, infos: &[(String, String)]) -> Vec<Device> {
    let Some(root) = parse_plist(list) else { return Vec::new() };
    let mut out = Vec::new();
    for whole in root.get("AllDisksAndPartitions").map(Pv::items).unwrap_or_default() {
        let Some(id) = whole.get("DeviceIdentifier").and_then(Pv::str) else { continue };
        let mut mounts = Vec::new();
        let mut take = |v: &Pv| {
            if let Some(m) = v.get("MountPoint").and_then(Pv::str).filter(|m| !m.is_empty()) {
                mounts.push(m.to_string());
            }
        };
        take(whole);
        for key in ["Partitions", "APFSVolumes"] {
            for p in whole.get(key).map(Pv::items).unwrap_or_default() {
                take(p);
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
        let node = info.as_ref().and_then(|i| i.get("DeviceNode")).and_then(Pv::str).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/dev/{id}")));
        let model = info.as_ref().and_then(|i| i.get("MediaName")).and_then(Pv::str).unwrap_or("").trim().to_string();
        let holds_os = os_internal || mounts.iter().any(|m| OS_MOUNTS.contains(&m.as_str()) || m.starts_with("/System"));
        // An internal SD slot (RemovableMedia on an Internal bus) is a card; an internal fixed disk is not.
        let internal = internal && !removable;
        out.push(Device { id: id.to_string(), node, bytes, removable, internal, model, mounts, holds_os });
    }
    out
}

/// The host's candidate disks, every one judged (refused ones are listed with their reason).
pub fn devices() -> Vec<Device> {
    let devs = host_devices();
    let writable = devs.iter().filter(|d| d.refusal(0).is_none()).count();
    println!("[installer] card devices n={} writable={}", devs.len(), writable);
    devs
}

#[cfg(target_os = "linux")]
fn host_devices() -> Vec<Device> {
    amber_bytes::disks::list().map(devices_from_disks).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn host_devices() -> Vec<Device> {
    let Some(list) = run_out("diskutil", &["list", "-plist", "external", "physical"]) else { return Vec::new() };
    let ids: Vec<String> = parse_plist(&list)
        .and_then(|r| r.get("WholeDisks").map(|w| w.items().iter().filter_map(Pv::str).map(str::to_string).collect()))
        .unwrap_or_default();
    let infos: Vec<(String, String)> = ids.iter().filter_map(|id| run_out("diskutil", &["info", "-plist", id]).map(|t| (id.clone(), t))).collect();
    devices_from_plists(&list, &infos)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_devices() -> Vec<Device> {
    println!("{}", refused_line("windows-unbuilt", None));
    Vec::new()
}

#[cfg(target_os = "macos")]
fn run_out(cmd: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn run_ok(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── the plist subset (diskutil's XML: dict / array / string / integer / true / false) ───────────

/// A plist value; `Other` is any element the card face never reads (`real`, `data`, `date`).
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

// ── image, write, verify, eject ─────────────────────────────────────────────────────────────────

/// The image `./arroyo esp-x86` lays under UNAOS_UNAFS=1, in the checkout this vessel was built from.
pub fn default_image() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unaos/target/unaos-x86-card.img")
}

/// A whole-disk image: a regular file, whole sectors, with the (protective) MBR signature at LBA 0.
/// `Err("no-image")` otherwise (a two-tree esp-x86 build without UNAOS_UNAFS=1 has no image: INSTALLER-TREES).
pub fn image_check(path: &Path) -> Result<u64, &'static str> {
    let meta = std::fs::metadata(path).map_err(|_| "no-image")?;
    let bytes = meta.len();
    if !meta.is_file() || bytes < SECTOR || bytes % SECTOR != 0 {
        return Err("no-image");
    }
    let mut src = FileBlock::open(path).map_err(|_| "no-image")?;
    let mut lba0 = [0u8; SECTOR as usize];
    src.read(0, &mut lba0).map_err(|_| "no-image")?;
    if lba0[510..512] != [0x55, 0xAA] {
        return Err("no-image");
    }
    Ok(bytes)
}

/// What a finished write did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub device: String,
    pub size: u64,
    pub image: PathBuf,
    pub written_mb: u64,
    pub verified: bool,
    pub ejected: bool,
    pub ms: u64,
}

impl Report {
    pub fn line(&self) -> String {
        format!(
            "[installer] card device={} size={} image={} written_mb={} verified={} ejected={} ms={}",
            self.device,
            self.size,
            self.image.display(),
            self.written_mb,
            self.verified as u8,
            self.ejected as u8,
            self.ms
        )
    }
}

/// `[installer] card refused why=<why> device=<id|->`.
pub fn refused_line(why: &str, device: Option<&str>) -> String {
    format!("[installer] card refused why={why} device={}", device.unwrap_or("-"))
}

fn refuse(why: &'static str, device: Option<&str>) -> String {
    let l = refused_line(why, device);
    println!("{l}");
    l
}

/// Copy the first `sectors` of `src` onto `dst` in 4 MiB steps (`ClonePlan::chunks`), then `flush`.
/// `progress(done_sectors, total_sectors)` after each step. The ONE writer: the device write and the
/// tests' temp-file write are this fn.
pub fn write_to(src: &mut dyn Block, dst: &mut dyn Block, sectors: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    if sectors == 0 || sectors > src.sectors() {
        return Err(format!("image span {sectors} sectors is not inside the image ({})", src.sectors()));
    }
    if sectors > dst.sectors() {
        return Err(format!("image {sectors} sectors does not fit the device ({})", dst.sectors()));
    }
    let plan = ClonePlan { src_first: 0, dst_first: 0, sectors };
    let mut buf = vec![0u8; (CHUNK_SECTORS * SECTOR) as usize];
    let mut done = 0u64;
    for (s, d, n) in plan.chunks(CHUNK_SECTORS) {
        let b = &mut buf[..(n * SECTOR) as usize];
        src.read(s, b).map_err(|e| format!("read image lba {s}: {e}"))?;
        dst.write(d, b).map_err(|e| format!("write device lba {d}: {e}"))?;
        done += n;
        progress(done, sectors);
    }
    dst.flush().map_err(|e| format!("flush: {e}"))
}

/// Read back the first and the last MiB of the image span on `dst` and compare with `src`.
pub fn verify(src: &mut dyn Block, dst: &mut dyn Block, sectors: u64) -> bool {
    let n = VERIFY_SECTORS.min(sectors);
    if n == 0 || sectors > src.sectors() || sectors > dst.sectors() {
        return false;
    }
    fn same(src: &mut dyn Block, dst: &mut dyn Block, lba: u64, n: u64) -> bool {
        let mut a = vec![0u8; (n * SECTOR) as usize];
        let mut b = vec![0u8; (n * SECTOR) as usize];
        src.read(lba, &mut a).is_ok() && dst.read(lba, &mut b).is_ok() && a == b
    }
    same(&mut *src, &mut *dst, 0, n) && same(&mut *src, &mut *dst, sectors - n, n)
}

/// Unmount every volume of the disk (macOS `diskutil unmountDisk`; Linux `umount` per mount point).
pub fn unmount(dev: &Device) -> bool {
    if dev.mounts.is_empty() {
        return true;
    }
    if cfg!(target_os = "macos") {
        run_ok("diskutil", &["unmountDisk", &dev.node.to_string_lossy()])
    } else {
        let mut ok = true;
        for m in &dev.mounts {
            ok &= run_ok("umount", &[m]);
        }
        ok
    }
}

/// Eject the card (macOS `diskutil eject`; Linux `eject`, else `udisksctl power-off -b`).
pub fn eject(dev: &Device) -> bool {
    let node = dev.node.to_string_lossy();
    if cfg!(target_os = "macos") {
        run_ok("diskutil", &["eject", &node])
    } else {
        run_ok("eject", &[&node]) || run_ok("udisksctl", &["power-off", "-b", &node])
    }
}

/// Open the picked device for writing: Amber's policy decides (block device, both keys, not mounted).
fn open_device(dev: &Device) -> Result<FileBlock, String> {
    let pol = amber_bytes::policy::Policy { yes_i_mean_it: true, allow: vec![dev.node.clone()], mounts: None };
    let (_, canon) = pol.check_write(&dev.node)?;
    let fb = if cfg!(target_os = "macos") {
        // `seek(End)` answers 0 on a macOS disk node: open at diskutil's size (the grant, amber_core INSTALLERCARD).
        FileBlock::open_rw_sectors(&canon, dev.bytes / SECTOR)
    } else {
        FileBlock::open_rw(&canon)
    };
    fb.map_err(|e| format!("{}: {e}", canon.display()))
}

/// Write `image` onto `dev` after the operator typed its phrase: refuse by name, unmount, write, verify, eject.
pub fn write(image: &Path, dev: &Device, typed: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<Report, String> {
    if cfg!(not(any(target_os = "linux", target_os = "macos"))) {
        return Err(refuse("windows-unbuilt", Some(&dev.id)));
    }
    let image_bytes = image_check(image).map_err(|why| refuse(why, Some(&dev.id)))?;
    if let Some(why) = dev.refusal(image_bytes) {
        return Err(refuse(why, Some(&dev.id)));
    }
    if !phrase_ok(dev, typed) {
        return Err(format!("confirmation does not match: type `{}`", dev.phrase()));
    }
    unmount(dev);
    let now = host_devices().into_iter().find(|d| d.id == dev.id).ok_or_else(|| format!("{} is gone", dev.id))?;
    if !now.mounts.is_empty() {
        return Err(refuse("mounted", Some(&dev.id)));
    }
    if let Some(why) = now.refusal(image_bytes) {
        return Err(refuse(why, Some(&dev.id)));
    }
    let t0 = Instant::now();
    let sectors = image_bytes / SECTOR;
    let mut src = FileBlock::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let mut dst = open_device(&now)?;
    write_to(&mut src, &mut dst, sectors, progress)?;
    let verified = verify(&mut src, &mut dst, sectors);
    drop(dst);
    let ejected = verified && eject(&now);
    let r = Report {
        device: now.id.clone(),
        size: now.bytes,
        image: image.to_path_buf(),
        written_mb: image_bytes >> 20,
        verified,
        ejected,
        ms: t0.elapsed().as_millis() as u64,
    };
    println!("{}", r.line());
    Ok(r)
}

// ── the face ────────────────────────────────────────────────────────────────────────────────────

enum Msg {
    Progress(u64, u64),
    Done(Result<Report, String>),
}

/// The card face's state. ONE owner: the vessel's serve task; the write runs on its own thread, which owns
/// both FileBlocks and reports over a channel this struct drains (no shared lock).
pub struct Card {
    pub devices: Vec<Device>,
    pub picked: Option<String>,
    pub image: PathBuf,
    /// The image's bytes, or the refusal (`no-image`).
    pub image_bytes: Result<u64, &'static str>,
    pub typed: String,
    /// (done, total) sectors of the running or last write.
    pub progress: (u64, u64),
    pub verdict: String,
    rx: Option<Receiver<Msg>>,
}

impl Card {
    /// Enumerate the host's disks and find the checkout's card image.
    pub fn probe() -> Self {
        let image = default_image();
        let image_bytes = image_check(&image);
        let verdict = match image_bytes {
            Ok(_) => String::from("pick a card, type its size and name, then Write"),
            Err(why) => format!("{} (build it: UNAOS_UNAFS=1 ./arroyo esp-x86)", refused_line(why, None)),
        };
        Card { devices: devices(), picked: None, image, image_bytes, typed: String::new(), progress: (0, 0), verdict, rx: None }
    }

    pub fn picked_device(&self) -> Option<&Device> {
        self.picked.as_ref().and_then(|id| self.devices.iter().find(|d| &d.id == id))
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Drain the write thread's reports; true when anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        let mut done = false;
        while let Ok(m) = rx.try_recv() {
            changed = true;
            match m {
                Msg::Progress(d, t) => self.progress = (d, t),
                Msg::Done(r) => {
                    self.verdict = match r {
                        Ok(rep) => rep.line(),
                        Err(e) => e,
                    };
                    done = true;
                }
            }
        }
        if done {
            self.rx = None;
            self.typed.clear();
        }
        changed
    }

    /// Start the write of the picked device on its own thread.
    pub fn start_write(&mut self) {
        if self.busy() {
            return;
        }
        let Some(dev) = self.picked_device().cloned() else {
            self.verdict = String::from("pick a card first");
            return;
        };
        if let Err(why) = self.image_bytes {
            self.verdict = refuse(why, Some(&dev.id));
            return;
        }
        if !phrase_ok(&dev, &self.typed) {
            self.verdict = format!("type `{}` to confirm", dev.phrase());
            return;
        }
        let (tx, rx) = channel();
        let (image, typed) = (self.image.clone(), self.typed.clone());
        self.progress = (0, self.image_bytes.unwrap_or(0) / SECTOR);
        self.verdict = format!("writing {} ...", dev.id);
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let r = write(&image, &dev, &typed, &mut |d, t| {
                let _ = ptx.send(Msg::Progress(d, t));
            });
            let _ = tx.send(Msg::Done(r));
        });
    }
}

/// Drain the write thread's progress and Report into `card` (the serve task calls it each loop pass and on a
/// 100 ms tick while a write runs). Idle cost: one `Option` check.
pub fn poll(card: &mut Card) {
    card.poll();
}

/// The pane INSTALLERVESSEL's window shows (a fresh probe).
pub fn face() -> TetraNode {
    face_of(&Card::probe())
}

/// The pane for a given state: the image, the device list, the pick buttons, the typed confirmation, Write,
/// the progress and the verdict. Every control speaks `SMessage::ConsoleInput`, routed to [`on_input`].
pub fn face_of(card: &Card) -> TetraNode {
    let line = |level: &str, text: String| ConsoleLine { level: level.to_string(), source: String::from("card"), text };
    let ib = card.image_bytes.unwrap_or(0);
    let mut lines = vec![line(
        "info",
        match card.image_bytes {
            Ok(b) => format!("image {} ({} MiB)", card.image.display(), b >> 20),
            Err(why) => format!("image {} [refused {why}]", card.image.display()),
        },
    )];
    if card.devices.is_empty() {
        lines.push(line("warn", String::from("no removable disk found — insert a card")));
    }
    for d in &card.devices {
        let mark = if card.picked.as_deref() == Some(d.id.as_str()) { "> " } else { "  " };
        lines.push(line(if d.refusal(ib).is_some() { "warn" } else { "info" }, format!("{mark}{}", d.row(ib))));
    }
    let (done, total) = card.progress;
    if total > 0 {
        lines.push(line("info", format!("written {} / {} MiB", (done * SECTOR) >> 20, (total * SECTOR) >> 20)));
    }
    lines.push(line("info", card.verdict.clone()));

    let picks: Vec<TetraNode> = card
        .devices
        .iter()
        .filter(|d| d.refusal(ib).is_none())
        .map(|d| TetraNode::Button {
            id: format!("card.pick.{}", d.id),
            label: format!("{} ({} GB)", d.id, gb1(d.bytes)),
            action: bandy::SMessage::ConsoleInput(format!("card pick {}", d.id)),
        })
        .collect();
    let placeholder = card.picked_device().map(Device::phrase).unwrap_or_else(|| String::from("pick a card"));
    TetraNode::VStack(vec![
        TetraNode::Console(ConsoleTetra { lines, ..Default::default() }),
        TetraNode::HStack(picks),
        TetraNode::TextField { id: String::from("card.confirm"), placeholder, action: TextAction::ConsoleInput },
        TetraNode::Button { id: String::from("card.write"), label: String::from("Write"), action: bandy::SMessage::ConsoleInput(String::from("card write")) },
    ])
}

/// Route one `SMessage::ConsoleInput` line: `card pick <id>`, `card write`, `card eject`, `card image <path>`,
/// `card rescan`, or — with a card picked — its typed confirmation phrase. False = not the card face's line.
pub fn on_input(card: &mut Card, line: &str) -> bool {
    card.poll();
    let l = line.trim();
    let Some(rest) = l.strip_prefix("card ").map(str::trim) else {
        if let Some(dev) = card.picked_device() {
            if !card.busy() && phrase_ok(dev, l) {
                card.typed = l.to_string();
                card.verdict = String::from("confirmed — press Write");
                return true;
            }
        }
        return false;
    };
    let (verb, arg) = rest.split_once(' ').map(|(v, a)| (v, a.trim())).unwrap_or((rest, ""));
    match verb {
        "pick" if !card.busy() => {
            let ib = card.image_bytes.unwrap_or(0);
            match card.devices.iter().find(|d| d.id == arg) {
                Some(d) => match d.refusal(ib) {
                    Some(why) => card.verdict = refuse(why, Some(&d.id)),
                    None => {
                        card.verdict = format!("type `{}` to confirm", d.phrase());
                        card.picked = Some(d.id.clone());
                        card.typed.clear();
                    }
                },
                None => card.verdict = format!("no device {arg}"),
            }
        }
        "write" => card.start_write(),
        "eject" if !card.busy() => {
            if let Some(d) = card.picked_device().cloned() {
                card.verdict = format!("eject {}: {}", d.id, if eject(&d) { "ok" } else { "failed" });
            }
        }
        "image" if !card.busy() => {
            card.image = PathBuf::from(arg);
            card.image_bytes = image_check(&card.image);
            if let Err(why) = card.image_bytes {
                card.verdict = refuse(why, None);
            }
        }
        "rescan" if !card.busy() => {
            card.devices = devices();
            if card.picked_device().is_none() {
                card.picked = None;
            }
        }
        _ if card.busy() => card.verdict = String::from("busy writing — wait for the verdict"),
        _ => card.verdict = format!("unknown card verb `{verb}` (pick / write / eject / image / rescan)"),
    }
    true
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LIST: &str = include_str!("../tests/fixtures/diskutil-list-external.plist");
    const DISK4: &str = include_str!("../tests/fixtures/diskutil-info-disk4.plist");
    const DISK0: &str = include_str!("../tests/fixtures/diskutil-info-disk0.plist");
    const IMG4G: u64 = 4_500 << 20;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("installercard-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plist_parses_diskutil_shapes() {
        let r = parse_plist(LIST).unwrap();
        let whole: Vec<&str> = r.get("WholeDisks").unwrap().items().iter().filter_map(Pv::str).collect();
        assert_eq!(whole, ["disk0", "disk4"]);
        let i = parse_plist(DISK4).unwrap();
        assert_eq!(i.get("MediaName").and_then(Pv::str), Some("SD Card Reader"));
        assert_eq!(i.get("Size").and_then(Pv::int), Some(63_864_569_856));
        assert_eq!(i.get("MountPoint").and_then(Pv::str), Some(""));
        assert_eq!(i.get("Internal").and_then(Pv::bool), Some(false));
        assert_eq!(parse_plist("<plist><string>a &amp; b</string></plist>"), Some(Pv::Str("a & b".into())));
        assert_eq!(parse_plist("not a plist"), None);
    }

    #[test]
    fn macos_devices_judged() {
        let infos = vec![("disk0".to_string(), DISK0.to_string()), ("disk4".to_string(), DISK4.to_string())];
        let d = devices_from_plists(LIST, &infos);
        assert_eq!(d.len(), 2);
        let (ssd, sd) = (&d[0], &d[1]);
        assert_eq!(ssd.id, "disk0");
        assert!(ssd.holds_os && ssd.internal && !ssd.removable);
        assert_eq!(ssd.refusal(IMG4G), Some("system-disk"));
        assert_eq!(sd.node, PathBuf::from("/dev/disk4"));
        assert_eq!(sd.model, "SD Card Reader");
        assert_eq!(sd.mounts, ["/Volumes/UNAOS"]);
        assert!(sd.removable && !sd.internal && !sd.holds_os);
        assert_eq!(sd.refusal(IMG4G), None);
        assert_eq!(sd.phrase(), "63.9 GB disk4");
        // No info for a disk: judged internal, never guessed writable.
        let blind = devices_from_plists(LIST, &[]);
        assert!(blind.iter().all(|d| d.refusal(0) == Some("system-disk")));
    }

    #[test]
    fn linux_devices_judged_through_amber() {
        let root = scratch("sysfs");
        let sb = root.join("block");
        let put = |p: PathBuf, s: &str| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, s).unwrap();
        };
        put(sb.join("sda/size"), "1953525168\n");
        put(sb.join("sda/removable"), "0\n");
        put(sb.join("sda/device/model"), "Samsung SSD 870\n");
        put(sb.join("sda/sda2/partition"), "2\n");
        put(sb.join("mmcblk0/size"), "124735488\n");
        put(sb.join("mmcblk0/removable"), "1\n");
        put(sb.join("mmcblk0/mmcblk0p1/partition"), "1\n");
        put(sb.join("loop0/size"), "8388608\n");
        put(sb.join("loop0/removable"), "1\n");
        let mounts = "/dev/sda2 / ext4 rw 0 0\n/dev/mmcblk0p1 /media/u/UNAOS vfat rw 0 0\n";
        let disks = amber_bytes::disks::list_from(&sb, Path::new("/dev"), mounts).unwrap();
        let _ = fs::remove_dir_all(&root);
        let d = devices_from_disks(disks);
        let by = |id: &str| d.iter().find(|x| x.id == id).unwrap().clone();
        assert_eq!(by("sda").refusal(IMG4G), Some("system-disk"));
        assert!(by("sda").holds_os);
        assert_eq!(by("loop0").refusal(IMG4G), Some("system-disk"), "virtual");
        let sd = by("mmcblk0");
        assert_eq!(sd.refusal(IMG4G), None);
        assert_eq!(sd.mounts, ["/media/u/UNAOS"]);
        assert_eq!(sd.phrase(), "63.9 GB mmcblk0");
        assert_eq!(sd.refusal(64_000_000_000), Some("too-small"));
    }

    #[test]
    fn phrase_rules() {
        let d = Device { id: "disk4".into(), node: "/dev/disk4".into(), bytes: 63_864_569_856, removable: true, internal: false, model: String::new(), mounts: vec![], holds_os: false };
        assert!(phrase_ok(&d, "63.9 GB disk4"));
        assert!(phrase_ok(&d, "  63.9   gb  DISK4 "));
        assert!(!phrase_ok(&d, "64 GB disk4"));
        assert!(!phrase_ok(&d, "63.9 GB disk5"));
        assert!(!phrase_ok(&d, ""));
        assert_eq!(gb1(15_931_539_456), "15.9");
        assert_eq!(gb1(950_000_000), "1.0");
    }

    #[test]
    fn image_check_rules() {
        let dir = scratch("img");
        assert_eq!(image_check(&dir.join("absent.img")), Err("no-image"));
        assert_eq!(image_check(&dir), Err("no-image"), "a tree is not an image");
        let mut b = vec![0u8; 4096];
        fs::write(dir.join("blank.img"), &b).unwrap();
        assert_eq!(image_check(&dir.join("blank.img")), Err("no-image"), "no 55AA");
        fs::write(dir.join("odd.img"), &b[..1000]).unwrap();
        assert_eq!(image_check(&dir.join("odd.img")), Err("no-image"), "not whole sectors");
        b[510] = 0x55;
        b[511] = 0xAA;
        fs::write(dir.join("ok.img"), &b).unwrap();
        assert_eq!(image_check(&dir.join("ok.img")), Ok(4096));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunk_writer_round_trip_and_verify() {
        let dir = scratch("write");
        // 9 MiB + 1 sector: two whole 4 MiB steps, a partial third, a one-sector tail.
        let sectors = (9 << 20) / SECTOR + 1;
        let img: Vec<u8> = (0..sectors * SECTOR).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let (ip, dp) = (dir.join("card.img"), dir.join("device.bin"));
        fs::write(&ip, &img).unwrap();
        fs::File::create(&dp).unwrap().set_len(16 << 20).unwrap();
        let mut src = FileBlock::open(&ip).unwrap();
        let mut dst = FileBlock::open_rw(&dp).unwrap();
        let mut steps = Vec::new();
        write_to(&mut src, &mut dst, sectors, &mut |d, t| steps.push((d, t))).unwrap();
        assert_eq!(steps, [(8192, sectors), (16384, sectors), (sectors, sectors)]);
        assert!(verify(&mut src, &mut dst, sectors));
        drop(dst);
        let got = fs::read(&dp).unwrap();
        assert_eq!(&got[..img.len()], &img[..]);
        assert!(got[img.len()..].iter().all(|b| *b == 0));
        // The granted sized open sees the same bytes.
        let mut sized = FileBlock::open_rw_sectors(&dp, (16 << 20) / SECTOR).unwrap();
        assert_eq!(sized.sectors(), (16 << 20) / SECTOR);
        assert!(verify(&mut src, &mut sized, sectors));
        // Corrupt the device's last MiB of the span: verify reads it back and says 0.
        sized.write(sectors - 1, &[0xEEu8; 512]).unwrap();
        assert!(!verify(&mut src, &mut sized, sectors));
        // A device smaller than the image is refused before a byte moves.
        fs::File::create(dir.join("tiny.bin")).unwrap().set_len(1 << 20).unwrap();
        let mut tiny = FileBlock::open_rw(&dir.join("tiny.bin")).unwrap();
        assert!(write_to(&mut src, &mut tiny, sectors, &mut |_, _| {}).unwrap_err().contains("does not fit"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wire_lines() {
        let r = Report { device: "disk4".into(), size: 63_864_569_856, image: "/u/unaos/target/unaos-x86-card.img".into(), written_mb: 4704, verified: true, ejected: false, ms: 812345 };
        assert_eq!(
            r.line(),
            "[installer] card device=disk4 size=63864569856 image=/u/unaos/target/unaos-x86-card.img written_mb=4704 verified=1 ejected=0 ms=812345"
        );
        assert_eq!(refused_line("too-small", Some("sdb")), "[installer] card refused why=too-small device=sdb");
        assert_eq!(refused_line("windows-unbuilt", None), "[installer] card refused why=windows-unbuilt device=-");
    }

    #[test]
    fn face_and_input_route() {
        let sd = Device { id: "disk4".into(), node: "/dev/disk4".into(), bytes: 63_864_569_856, removable: true, internal: false, model: "SD Card Reader".into(), mounts: vec![], holds_os: false };
        let ssd = Device { id: "disk0".into(), node: "/dev/disk0".into(), bytes: 500_277_790_720, removable: false, internal: true, model: "APPLE SSD".into(), mounts: vec!["/".into()], holds_os: true };
        let mut c = Card { devices: vec![ssd, sd], picked: None, image: "/nonexistent/card.img".into(), image_bytes: Ok(IMG4G), typed: String::new(), progress: (0, 0), verdict: String::new(), rx: None };
        assert!(!on_input(&mut c, "ls /"), "not the card face's line");
        assert!(on_input(&mut c, "card pick disk0"));
        assert_eq!(c.picked, None);
        assert!(c.verdict.contains("why=system-disk"));
        assert!(on_input(&mut c, "card pick disk4"));
        assert_eq!(c.picked.as_deref(), Some("disk4"));
        assert!(!on_input(&mut c, "64 GB disk4"), "a wrong phrase is not consumed");
        assert!(on_input(&mut c, "63.9 GB disk4"));
        assert_eq!(c.typed, "63.9 GB disk4");
        let TetraNode::VStack(kids) = face_of(&c) else { panic!("face is a VStack") };
        assert_eq!(kids.len(), 4);
        let TetraNode::HStack(picks) = &kids[1] else { panic!("picks row") };
        assert_eq!(picks.len(), 1, "only the writable disk gets a pick button");
        let TetraNode::TextField { placeholder, .. } = &kids[2] else { panic!("confirm field") };
        assert_eq!(placeholder, "63.9 GB disk4");
        let TetraNode::Console(con) = &kids[0] else { panic!("list") };
        assert!(con.lines.iter().any(|l| l.text.contains("disk0") && l.text.contains("[refused system-disk]")));
        assert!(con.lines.iter().any(|l| l.text.starts_with("> disk4")));
    }
}
