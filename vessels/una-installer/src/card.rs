//! CHARTER: Amber Bytes — the UnaOS_Installer card face (vessel module; seam shared-core amber_core + amber_bytes)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERCARD (LEDGER SR414): face (a) of the UnaOS_Installer vessel — put UnaOS on a card from a Mac or a
// Linux box. The card face ENCODES NOTHING: the image is the one `./arroyo esp-x86` lays under UNAOS_UNAFS=1
// (`unaos/target/unaos-x86-card.img`, GPT p1 ESP + p2 UnaFS, built by `tools/una-card` over amber_core); the
// devices are Amber's (`amber_bytes::disks::list()` on Linux and macOS, AMBERPLIST SR417);
// the device is opened only through Amber's write policy (`amber_bytes::policy::Policy`: the typed confirmation
// is `yes_i_mean_it`, the picked device the sole allowlist entry, never while mounted); every byte goes through
// amber_core's `Block` (`FileBlock`) in `ClonePlan::chunks` steps of 4 MiB, then `flush` (sync_all), a read-back
// of the first and last MiB, and the eject. Windows is a named refusal (`why=windows-unbuilt`).
//
// Wire:
//   [installer] card devices n=<n> writable=<n>
//   [installer] card device=<id> size=<bytes> image=<path> written_mb=<n> readback=1|0 ejected=1|0 ms=<n>
//   [installer] card refused why=<system-disk|mounted|too-small|no-image|windows-unbuilt|elevation-denied|elevation-unavailable> device=<id|->
//   [installer] card elevate how=<authopen|pkexec> ok=1|0 why=<-|elevation-denied|elevation-unavailable>
//     (INSTALLERAUTHOPEN SR422: once per write, only when the plain open was refused for permission)

use std::fs::File;
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

/// Amber's enumeration (sysfs on Linux, `diskutil -plist` on macOS), judged: the ONE mapping for both OSes.
pub fn devices_from_disks(disks: Vec<amber_bytes::disks::DiskInfo>) -> Vec<Device> {
    disks
        .into_iter()
        .map(|d| {
            let holds_os = d.os_internal || d.mounts.iter().any(|m| OS_MOUNTS.contains(&m.as_str()) || m.starts_with("/System"));
            let bytes = d.bytes();
            Device {
                internal: d.virtual_dev || d.internal,
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

/// The host's candidate disks, every one judged (refused ones are listed with their reason).
pub fn devices() -> Vec<Device> {
    let devs = host_devices();
    let writable = devs.iter().filter(|d| d.refusal(0).is_none()).count();
    println!("[installer] card devices n={} writable={}", devs.len(), writable);
    devs
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn host_devices() -> Vec<Device> {
    amber_bytes::disks::list().map(devices_from_disks).unwrap_or_default()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_devices() -> Vec<Device> {
    println!("{}", refused_line("windows-unbuilt", None));
    Vec::new()
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
            "[installer] card device={} size={} image={} written_mb={} readback={} ejected={} ms={}",
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
    match fb {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => elevated_block(dev, &canon),
        fb => fb.map_err(|e| format!("{}: {e}", canon.display())),
    }
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

// ── elevation (INSTALLERAUTHOPEN SR422) ─────────────────────────────────────────────────────────
//
// A removable disk node opens only as root (macOS `/dev/diskN`; Linux `/dev/sdX` without the disk group). The
// elevated process opens ONE fd and hands it back over a socketpair end that is its STDOUT, as one SCM_RIGHTS
// message: macOS `/usr/libexec/authopen -stdoutpipe -o 2 <node>` (O_RDWR: `verify` reads back through the same fd),
// Linux `pkexec <self> --open-device <node>` (pkexec marks every fd >= 3 close-on-exec, so stdout is the only
// channel that survives it). The helper re-runs Amber's policy and judgment, opens, sends and exits; nothing else
// runs as root, and every byte is written by the unprivileged write thread through the received fd.

/// `[installer] card elevate how=<authopen|pkexec> ok=1|0 why=<why>`.
pub fn elevate_line(how: &str, ok: bool, why: &str) -> String {
    format!("[installer] card elevate how={how} ok={} why={why}", ok as u8)
}

/// The elevated command for `node`: authopen on macOS, `pkexec <exe> --open-device <node>` elsewhere.
pub fn elevation_argv(macos: bool, node: &Path, exe: &Path) -> (String, Vec<String>) {
    let node = node.to_string_lossy().into_owned();
    if macos {
        (String::from("/usr/libexec/authopen"), vec!["-stdoutpipe".into(), "-o".into(), "2".into(), node])
    } else {
        (String::from("pkexec"), vec![exe.to_string_lossy().into_owned(), "--open-device".into(), node])
    }
}

/// The plain open was refused for permission: elevate, print the elevate line once, and take the fd into a
/// `FileBlock` (macOS at diskutil's size, as `open_device` sizes it; Linux at the measured length).
fn elevated_block(dev: &Device, canon: &Path) -> Result<FileBlock, String> {
    let how = if cfg!(target_os = "macos") { "authopen" } else { "pkexec" };
    match elevated_open(canon) {
        Ok(file) => {
            println!("{}", elevate_line(how, true, "-"));
            let sized = cfg!(target_os = "macos").then_some(dev.bytes / SECTOR);
            FileBlock::from_file(file, sized).map_err(|e| format!("{}: {e}", canon.display()))
        }
        Err(why) => {
            println!("{}", elevate_line(how, false, why));
            Err(refuse(why, Some(&dev.id)))
        }
    }
}

/// Run the elevated opener on `canon` and receive its fd. `elevation-unavailable` = the tool is not here;
/// `elevation-denied` = dismissed, not authorized, or no fd came back.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn elevated_open(canon: &Path) -> Result<File, &'static str> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().map_err(|_| "elevation-unavailable")?;
    let (prog, args) = elevation_argv(cfg!(target_os = "macos"), canon, &exe);
    let (ours, theirs) = UnixStream::pair().map_err(|_| "elevation-unavailable")?;
    // The temporary Command (and its copy of `theirs`) drops at the end of this statement: the child then holds
    // the only other end, so a child that exits without sending reads as end-of-file below.
    let mut child = Command::new(&prog)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(theirs)))
        .spawn()
        .map_err(|_| "elevation-unavailable")?;
    let fd = recv_fd(&ours);
    let _ = child.wait();
    fd.ok_or("elevation-denied")
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn elevated_open(_canon: &Path) -> Result<File, &'static str> {
    Err("windows-unbuilt")
}

const SCM_RIGHTS: i32 = 1;

/// The `cmsghdr` layout of one OS: Linux `{ size_t len; int level; int type; }` aligned to `size_t`, SOL_SOCKET 1;
/// macOS `{ socklen_t len; int level; int type; }` aligned to 4, SOL_SOCKET 0xffff. The parser takes it as data so
/// a canned macOS message is tested on Linux.
#[derive(Clone, Copy, Debug)]
pub struct CmsgLayout {
    pub len_bytes: usize,
    pub align: usize,
    pub sol_socket: i32,
}

impl CmsgLayout {
    pub const LINUX: Self = Self { len_bytes: 8, align: 8, sol_socket: 1 };
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub const MACOS: Self = Self { len_bytes: 4, align: 4, sol_socket: 0xffff };
    #[cfg(target_os = "macos")]
    pub const NATIVE: Self = Self::MACOS;
    #[cfg(not(target_os = "macos"))]
    pub const NATIVE: Self = Self { len_bytes: std::mem::size_of::<usize>(), align: std::mem::size_of::<usize>(), sol_socket: 1 };

    fn up(&self, n: usize) -> usize {
        (n + self.align - 1) & !(self.align - 1)
    }
    /// The header's aligned size: where the data starts (`CMSG_DATA`).
    fn hdr(&self) -> usize {
        self.up(self.len_bytes + 8)
    }
}

/// The first SCM_RIGHTS fd in a received control buffer; `None` when there is none or the buffer is malformed.
pub fn fd_from_control(buf: &[u8], l: CmsgLayout) -> Option<i32> {
    let int = |at: usize| -> Option<i32> { Some(i32::from_ne_bytes(buf.get(at..at + 4)?.try_into().ok()?)) };
    let mut off = 0usize;
    while off + l.hdr() <= buf.len() {
        let len = if l.len_bytes == 4 {
            u32::from_ne_bytes(buf.get(off..off + 4)?.try_into().ok()?) as usize
        } else {
            u64::from_ne_bytes(buf.get(off..off + 8)?.try_into().ok()?) as usize
        };
        if len < l.hdr() || len > buf.len() - off {
            return None;
        }
        let (level, ty) = (int(off + l.len_bytes)?, int(off + l.len_bytes + 4)?);
        if level == l.sol_socket && ty == SCM_RIGHTS && len >= l.hdr() + 4 {
            return int(off + l.hdr());
        }
        off += l.up(len);
    }
    None
}

/// One SCM_RIGHTS control message carrying `fd` (`CMSG_SPACE(4)` bytes, `cmsg_len = CMSG_LEN(4)`).
pub fn control_for_fd(fd: i32, l: CmsgLayout) -> Vec<u8> {
    let mut v = vec![0u8; l.hdr() + l.up(4)];
    let len = l.hdr() + 4;
    if l.len_bytes == 4 {
        v[..4].copy_from_slice(&(len as u32).to_ne_bytes());
    } else {
        v[..8].copy_from_slice(&(len as u64).to_ne_bytes());
    }
    v[l.len_bytes..l.len_bytes + 4].copy_from_slice(&l.sol_socket.to_ne_bytes());
    v[l.len_bytes + 4..l.len_bytes + 8].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
    v[l.hdr()..l.hdr() + 4].copy_from_slice(&fd.to_ne_bytes());
    v
}

/// Receive one fd sent as SCM_RIGHTS (authopen's `-stdoutpipe` message, or the helper's [`send_fd`]).
#[cfg(unix)]
pub fn recv_fd(sock: &std::os::unix::net::UnixStream) -> Option<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut byte = [0u8; 1];
    let mut ctrl = [0u64; 8];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    // SAFETY: an all-zero msghdr is valid (null name, no flags); the pointers set below outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&ctrl) as _;
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let n = loop {
        // SAFETY: `msg` points at live buffers of the stated lengths.
        let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, flags) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        break n;
    };
    if n < 0 {
        return None;
    }
    let clen = (msg.msg_controllen as usize).min(std::mem::size_of_val(&ctrl));
    // SAFETY: the first `clen` bytes of `ctrl` are initialised (it was zeroed and the kernel wrote into it).
    let bytes = unsafe { std::slice::from_raw_parts(ctrl.as_ptr().cast::<u8>(), clen) };
    let fd = fd_from_control(bytes, CmsgLayout::NATIVE)?;
    // SAFETY: the kernel installed `fd` in this process for us; nothing else owns it.
    (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) })
}

/// Send `file`'s fd as one SCM_RIGHTS message (the helper's half of the handoff).
#[cfg(unix)]
pub fn send_fd(sock: &std::os::unix::net::UnixStream, file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut byte = [0u8; 1];
    let mut ctrl = control_for_fd(file.as_raw_fd(), CmsgLayout::NATIVE);
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    // SAFETY: as in `recv_fd`.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.as_mut_ptr().cast();
    msg.msg_controllen = ctrl.len() as _;
    // SAFETY: `msg` points at live buffers of the stated lengths.
    let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
    if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// The helper's open, as root: the node must be a block device Amber's policy admits (not mounted) AND a disk
/// Amber lists and judges a writable card. A regular file, a system disk or an unlisted node is refused.
#[cfg(unix)]
pub fn helper_open(node: &Path) -> Result<File, String> {
    use amber_bytes::policy::{Policy, TargetKind};
    let canon = node.canonicalize().map_err(|e| format!("{}: {e}", node.display()))?;
    let pol = Policy { yes_i_mean_it: true, allow: vec![canon.clone()], mounts: None };
    let (kind, canon) = pol.check_write(&canon)?;
    if kind != TargetKind::BlockDevice {
        return Err(format!("{}: not a block device — refused", canon.display()));
    }
    let dev = host_devices()
        .into_iter()
        .find(|d| d.node.canonicalize().map(|c| c == canon).unwrap_or(false))
        .ok_or_else(|| format!("{}: not a disk Amber lists — refused", canon.display()))?;
    if let Some(why) = dev.refusal(0) {
        return Err(format!("{}: not a card ({why}) — refused", canon.display()));
    }
    let mut oo = std::fs::OpenOptions::new();
    oo.read(true).write(true);
    #[cfg(target_os = "linux")]
    {
        // O_EXCL on a block device: EBUSY while anything holds or mounts it.
        use std::os::unix::fs::OpenOptionsExt;
        oo.custom_flags(libc::O_EXCL);
    }
    oo.open(&canon).map_err(|e| format!("{}: {e}", canon.display()))
}

/// The hidden helper verb `--open-device <node>` (what `elevated_open` runs under pkexec): `None` for any other
/// argv; else open through [`helper_open`], send the fd on stdout, and answer the exit code (0 sent, 1 refused).
#[cfg(unix)]
pub fn open_device_verb(args: impl Iterator<Item = String>) -> Option<i32> {
    use std::os::fd::AsFd;
    let a: Vec<String> = args.collect();
    if a.len() != 2 || a[0] != "--open-device" {
        return None;
    }
    let served = helper_open(Path::new(&a[1])).and_then(|file| {
        let out = std::io::stdout().as_fd().try_clone_to_owned().map_err(|e| format!("stdout: {e}"))?;
        send_fd(&std::os::unix::net::UnixStream::from(out), &file).map_err(|e| format!("send fd: {e}"))
    });
    Some(match served {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("una-installer --open-device: {e}");
            1
        }
    })
}

#[cfg(not(unix))]
pub fn open_device_verb(_args: impl Iterator<Item = String>) -> Option<i32> {
    None
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

    // The diskutil fixtures live with the reader in Amber Bytes (AMBERPLIST SR417).
    const LIST: &str = include_str!("../../../handlers/amber_bytes/tests/fixtures/diskutil-list-external.plist");
    const DISK4: &str = include_str!("../../../handlers/amber_bytes/tests/fixtures/diskutil-info-disk4.plist");
    const DISK0: &str = include_str!("../../../handlers/amber_bytes/tests/fixtures/diskutil-info-disk0.plist");
    const IMG4G: u64 = 4_500 << 20;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("installercard-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn macos_devices_judged() {
        let infos = vec![("disk0".to_string(), DISK0.to_string()), ("disk4".to_string(), DISK4.to_string())];
        let d = devices_from_disks(amber_bytes::disks::list_from_plists(LIST, &infos));
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
        let blind = devices_from_disks(amber_bytes::disks::list_from_plists(LIST, &[]));
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
            "[installer] card device=disk4 size=63864569856 image=/u/unaos/target/unaos-x86-card.img written_mb=4704 readback=1 ejected=0 ms=812345"
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

    // INSTALLERAUTHOPEN (SR422): the helper's half sends a real fd over a real socketpair (on a thread: a bin crate's
    // unit test has no CARGO_BIN_EXE to spawn), the receiver takes it into a FileBlock, and the ONE writer runs.
    #[test]
    fn elevate_fd_handoff_over_socketpair() {
        use std::os::unix::net::UnixStream;
        let dir = scratch("elevate");
        let sectors = (3 << 20) / SECTOR + 7;
        let img: Vec<u8> = (0..sectors * SECTOR).map(|i| (i.wrapping_mul(2246822519) >> 11) as u8).collect();
        let (ip, dp) = (dir.join("card.img"), dir.join("device.bin"));
        fs::write(&ip, &img).unwrap();
        fs::File::create(&dp).unwrap().set_len(16 << 20).unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let dev_file = fs::OpenOptions::new().read(true).write(true).open(&dp).unwrap();
        let sender = std::thread::spawn(move || send_fd(&a, &dev_file));
        let got = recv_fd(&b).expect("an fd over the socketpair");
        sender.join().unwrap().unwrap();
        let mut dst = FileBlock::from_file(got, None).unwrap();
        assert_eq!(dst.sectors(), (16 << 20) / SECTOR);
        assert!(dst.writable());
        let mut src = FileBlock::open(&ip).unwrap();
        write_to(&mut src, &mut dst, sectors, &mut |_, _| {}).unwrap();
        assert!(verify(&mut src, &mut dst, sectors));
        drop(dst);
        assert_eq!(&fs::read(&dp).unwrap()[..img.len()], &img[..]);
        // The sized take (macOS: diskutil's size, seek(End) answers 0 on a disk node).
        let (a, b) = UnixStream::pair().unwrap();
        send_fd(&a, &fs::OpenOptions::new().read(true).write(true).open(&dp).unwrap()).unwrap();
        let sized = FileBlock::from_file(recv_fd(&b).unwrap(), Some(4096)).unwrap();
        assert_eq!(sized.sectors(), 4096);
        // Denied: the elevated end closes without sending (dismissed, not authorized): no fd.
        let (a, b) = UnixStream::pair().unwrap();
        drop(a);
        assert!(recv_fd(&b).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_refuses_a_regular_file() {
        let dir = scratch("helper");
        let f = dir.join("passwd");
        fs::write(&f, b"root:x:0:0\n").unwrap();
        let e = helper_open(&f).unwrap_err();
        assert!(e.contains("not a block device") && e.contains("refused"), "{e}");
        assert!(helper_open(&dir.join("absent")).is_err());
        // The verb: any other argv is not the helper's; a refused node answers 1 before stdout is touched.
        assert_eq!(open_device_verb(["census".to_string()].into_iter()), None);
        assert_eq!(open_device_verb(["--open-device".to_string()].into_iter()), None);
        assert_eq!(open_device_verb(["--open-device", "/dev/sdb", "x"].map(String::from).into_iter()), None);
        assert_eq!(open_device_verb(["--open-device".to_string(), f.to_string_lossy().into_owned()].into_iter()), Some(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmsg_parser_canned() {
        // authopen's message on macOS: cmsg_len (u32) 16, SOL_SOCKET 0xffff, SCM_RIGHTS 1, fd 7 at offset 12.
        let mut mac = Vec::new();
        mac.extend(16u32.to_ne_bytes());
        mac.extend(0xffffi32.to_ne_bytes());
        mac.extend(1i32.to_ne_bytes());
        mac.extend(7i32.to_ne_bytes());
        assert_eq!(fd_from_control(&mac, CmsgLayout::MACOS), Some(7));
        assert_eq!(fd_from_control(&mac, CmsgLayout::LINUX), None, "the wrong layout reads a bogus length");
        assert_eq!(fd_from_control(&mac[..10], CmsgLayout::MACOS), None, "truncated");
        let mut lvl = mac.clone();
        lvl[4..8].copy_from_slice(&1i32.to_ne_bytes());
        assert_eq!(fd_from_control(&lvl, CmsgLayout::MACOS), None, "not SOL_SOCKET on macOS");
        // Linux: cmsg_len (u64) 20, SOL_SOCKET 1, SCM_RIGHTS 1, fd 9 at offset 16, padded to 24.
        let mut lin = Vec::new();
        lin.extend(20u64.to_ne_bytes());
        lin.extend(1i32.to_ne_bytes());
        lin.extend(1i32.to_ne_bytes());
        lin.extend(9i32.to_ne_bytes());
        lin.extend([0u8; 4]);
        assert_eq!(fd_from_control(&lin, CmsgLayout::LINUX), Some(9));
        let mut cred = lin.clone();
        cred[12..16].copy_from_slice(&2i32.to_ne_bytes());
        assert_eq!(fd_from_control(&cred, CmsgLayout::LINUX), None, "SCM_CREDENTIALS is not rights");
        assert_eq!(fd_from_control(&[], CmsgLayout::LINUX), None);
        assert_eq!(control_for_fd(7, CmsgLayout::MACOS), mac);
        assert_eq!(control_for_fd(9, CmsgLayout::LINUX), lin);
        for l in [CmsgLayout::LINUX, CmsgLayout::MACOS, CmsgLayout::NATIVE] {
            assert_eq!(fd_from_control(&control_for_fd(5, l), l), Some(5));
        }
    }

    #[test]
    fn elevation_argv_and_wire() {
        let (p, a) = elevation_argv(true, Path::new("/dev/disk4"), Path::new("/Users/u/Applications/una-installer"));
        assert_eq!(p, "/usr/libexec/authopen");
        assert_eq!(a, ["-stdoutpipe", "-o", "2", "/dev/disk4"]);
        let (p, a) = elevation_argv(false, Path::new("/dev/sdb"), Path::new("/home/u/.local/bin/una-installer"));
        assert_eq!(p, "pkexec");
        assert_eq!(a, ["/home/u/.local/bin/una-installer", "--open-device", "/dev/sdb"]);
        assert_eq!(elevate_line("authopen", false, "elevation-denied"), "[installer] card elevate how=authopen ok=0 why=elevation-denied");
        assert_eq!(elevate_line("pkexec", true, "-"), "[installer] card elevate how=pkexec ok=1 why=-");
        assert_eq!(refused_line("elevation-unavailable", Some("sdb")), "[installer] card refused why=elevation-unavailable device=sdb");
    }
}
