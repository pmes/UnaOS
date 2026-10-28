//! CHARTER: Amber Bytes — the UnaOS_Installer landing face (vessel module; seam shared-core amber_core + unafs + amber_bytes)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERLANDING (LEDGER SR415): the card comes back. Peter 2026-10-27: "the installer will install UnaOS to the
// inserted sd card, i do the boot, and if everything goes right whatever we can capture will be logged to the sd card.
// reboot to linux where the local claude will proceed with analysis". The kernel half is BOOTLOGCARD (rmbp-ledger
// B549) + ROOTDISK2 (B401): the flight recorder's snapshot is `/var/log/boot-<n>.log` on the card's UnaFS root, the
// whole file rewritten every ~4 s (4096 passes) while the capture grows, the newest 16 kept. This face READS it back
// on the host and lands it in the checkout for the local AI session. It ENCODES NOTHING: the device list is the card
// face's (`card::devices`), the medium is amber_core's read-only `FileBlock` under amber_core's `Recorder` (the
// medium is never written, even if the mount drains a reclaim queue: `amber_bytes::ops::unafs_probe`'s rule), the
// bridge is `amber_bytes::ops::Sectors`, and the filesystem is the `unafs` core's own reader (`locate_unafs`,
// `BlockAdapter::for_partition`, `UnaFS::mount` / `mount_with` for a keyed root, BP-storage-17, then `resolve_path`,
// `ls`, `stat`, `read_data`). The AI hand-off at rung 1 is a file, `PROMPT.md`, never a model call
// (INSTALLER-PROVIDER built by SR424: `provider send` posts it through Vein's provider seam).
//
// Evidence: `<checkout>/docs/dev/evidence/landing-<yyyymmdd-hhmm>/` (UTC) holding `boot<n>.log` per log found,
// `landing.md` (the device, the booted image's sha from its `:: OLDEST:` line, the boot dates, the verdict census, the
// first and last verdict lines) and `PROMPT.md` (the standing landing-report recipe, FLIGHT37.md's shape).
//
// Wire:
//   [installer] landing device=<id|image> boots=<k> log_kb=<n> verdicts=<pass>/<fail>/<skip> dir=<evidence dir>
//   [installer] landing refused why=<no-log|no-unafs|no-checkout|windows-unbuilt|keyed|unreadable> device=<id|->

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use amber_bytes::ops::Sectors;
use amber_core::block::{Block, Recorder};
use amber_core::file_block::FileBlock;
use quartzite::tetra::{ConsoleLine, ConsoleTetra, TetraNode, TextAction};
use unafs::fs::FileSystemError;
use unafs::{BlockAdapter, UnaFS, Unlock};

use crate::card::{self, Device};

/// Where BOOTLOGCARD writes (`unaos/crates/kernel/src/fs/bootlog.rs` DIR).
pub const LOG_DIR: &str = "/var/log";
/// The boot-flight report the prompt asks the local session to match.
pub const EXEMPLAR: &str = "docs/dev/evidence/rmbp-0915/flight37/FLIGHT37.md";

// ── the log ─────────────────────────────────────────────────────────────────────────────────────

/// One `/var/log/boot-<n>.log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boot {
    pub n: u32,
    pub bytes: Vec<u8>,
    /// The file's UnaFS ctime (unix seconds, the kernel's RTC; 0 = the kernel had no clock).
    pub ctime: u64,
}

/// The `::` verdict lines counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub pass: u32,
    pub fail: u32,
    pub skip: u32,
    pub first: Option<String>,
    pub last: Option<String>,
}

impl Census {
    /// Fold a later boot's census into this one.
    pub fn merge(&mut self, later: Census) {
        self.pass += later.pass;
        self.fail += later.fail;
        self.skip += later.skip;
        if self.first.is_none() {
            self.first = later.first;
        }
        if later.last.is_some() {
            self.last = later.last;
        }
    }
}

/// Everything read off one card (or image).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlightLog {
    /// The device id or the image path.
    pub source: String,
    /// Oldest first (by `<n>`).
    pub boots: Vec<Boot>,
    pub census: Census,
    /// `image=` of the newest `:: OLDEST:` line: the sha the booted kernel was built from.
    pub build: Option<String>,
    /// `board=` of the same line.
    pub board: Option<String>,
    /// The newest `:: BOOTLIST: ran= … ::` tally (the kernel's own count of the list).
    pub bootlist: Option<String>,
    /// Bytes the ring left out of the files (the FLIGHTRING markers, summed).
    pub elided: u64,
}

impl FlightLog {
    pub fn bytes(&self) -> u64 {
        self.boots.iter().map(|b| b.bytes.len() as u64).sum()
    }
    pub fn log_kb(&self) -> u64 {
        self.bytes().div_ceil(1024)
    }
}

/// The verdict of one line: it holds `:: ` and the token after its LAST `-> ` starts PASS, FAIL or SKIP.
pub fn verdict(line: &str) -> Option<&'static str> {
    if !line.contains(":: ") {
        return None;
    }
    let i = line.rfind("-> ")?;
    let t = &line[i + 3..];
    ["PASS", "FAIL", "SKIP"].into_iter().find(|v| t.starts_with(v))
}

/// Count the verdict lines of one log.
pub fn census(text: &str) -> Census {
    let mut c = Census::default();
    for line in text.lines() {
        let Some(v) = verdict(line) else { continue };
        match v {
            "PASS" => c.pass += 1,
            "FAIL" => c.fail += 1,
            _ => c.skip += 1,
        }
        let l = line.trim_end().to_string();
        if c.first.is_none() {
            c.first = Some(l.clone());
        }
        c.last = Some(l);
    }
    c
}

fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let i = s.find(key)?;
    s[i + key.len()..].split_whitespace().next()
}

/// The census and the identity lines of a set of boots (oldest first).
pub fn flight_log(source: &str, boots: Vec<Boot>) -> FlightLog {
    let mut log = FlightLog { source: source.to_string(), boots: Vec::new(), census: Census::default(), build: None, board: None, bootlist: None, elided: 0 };
    for b in &boots {
        let text = String::from_utf8_lossy(&b.bytes);
        log.census.merge(census(&text));
        for line in text.lines() {
            if let Some(i) = line.find(":: OLDEST: ") {
                let rest = &line[i..];
                log.build = field(rest, "image=").map(str::to_string).or(log.build.take());
                log.board = field(rest, "board=").map(str::to_string).or(log.board.take());
            }
            if let Some(i) = line.find(":: BOOTLIST: ran=") {
                log.bootlist = Some(line[i..].trim_end().to_string());
            }
            if let Some(i) = line.find(" byte(s) of the ring left out") {
                if line.contains(":: FLIGHTRING: ") {
                    log.elided += line[..i].split_whitespace().last().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
                }
            }
        }
    }
    log.boots = boots;
    log
}

fn boot_num(name: &str) -> Option<u32> {
    name.strip_prefix("boot-")?.strip_suffix(".log")?.parse().ok()
}

/// Read every `/var/log/boot-<n>.log` off a whole medium (GPT/MBR + a UnaFS partition found by its magic).
/// The medium is never written: every write the mount might issue lands in the `Recorder`.
fn read_block(dev: &mut dyn Block, source: &str, key: Option<&str>) -> Result<FlightLog, &'static str> {
    if dev.sectors() == 0 {
        return Err("unreadable");
    }
    let mut rec = Recorder::new(dev);
    let mut s = Sectors(&mut rec);
    let span = unafs::locate_unafs(&mut s).ok().flatten().ok_or("no-unafs")?;
    let adapter = BlockAdapter::for_partition(&mut s, &span);
    let mounted = match key {
        None => UnaFS::mount(adapter),
        Some(k) => UnaFS::mount_with(adapter, &Unlock::Password(k.as_bytes())),
    };
    let mut fs = mounted.map_err(|e| match e {
        FileSystemError::KeyRequired | FileSystemError::KeySlot(_) => "keyed",
        _ => "no-unafs",
    })?;
    let dir = fs.resolve_path(LOG_DIR).map_err(|_| "no-log")?;
    let mut found: Vec<(u32, u64)> = fs.ls(dir).map_err(|_| "no-log")?.into_iter().filter_map(|e| boot_num(&e.name).map(|n| (n, e.inode_id))).collect();
    if found.is_empty() {
        return Err("no-log");
    }
    found.sort_unstable();
    let mut boots = Vec::with_capacity(found.len());
    for (n, id) in found {
        let st = fs.stat(id).map_err(|_| "no-log")?;
        let bytes = fs.read_data(id, 0, st.size).map_err(|_| "no-log")?;
        boots.push(Boot { n, bytes, ctime: st.ctime });
    }
    Ok(flight_log(source, boots))
}

/// Read a card image file (a `dd` of the card, or the image itself).
pub fn read_image(path: &Path, key: Option<&str>) -> Result<FlightLog, &'static str> {
    let mut fb = FileBlock::open(path).map_err(|_| "unreadable")?;
    read_block(&mut fb, &path.display().to_string(), key)
}

/// Read the picked card. A macOS disk node sizes to 0 here (`unreadable`, LANDING-MACOS owed); a node the user may
/// not open is `unreadable` too (INSTALLER-AUTHOPEN owed).
pub fn read_device(dev: &Device, key: Option<&str>) -> Result<FlightLog, &'static str> {
    if cfg!(not(any(target_os = "linux", target_os = "macos"))) {
        return Err("windows-unbuilt");
    }
    let mut fb = FileBlock::open(&dev.node).map_err(|_| "unreadable")?;
    read_block(&mut fb, &dev.id, key)
}

/// The card face's devices that can be read: removable, not internal, not the OS disk, non-zero size.
pub fn devices() -> Vec<Device> {
    card::devices().into_iter().filter(|d| d.refusal(0).is_none()).collect()
}

// ── the landing ─────────────────────────────────────────────────────────────────────────────────

/// UTC (year, month, day, hour, minute) of unix seconds (days-from-civil inverse, proleptic Gregorian).
pub fn civil(secs: u64) -> (i64, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}

/// `yyyymmdd-hhmm` (the evidence dir's name).
pub fn stamp(secs: u64) -> String {
    let (y, m, d, h, mi) = civil(secs);
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}")
}

/// `yyyy-mm-dd hh:mmZ`; 0 is said as unknown.
pub fn date(secs: u64) -> String {
    if secs == 0 {
        return String::from("unknown (the kernel had no clock)");
    }
    let (y, m, d, h, mi) = civil(secs);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}Z")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `landing.md`: what was read, by whom, and what it says.
pub fn landing_md(log: &FlightLog, dir_rel: &str, now: u64) -> String {
    let mut s = format!("# Landing — {}, {}\n\n", log.source, date(now));
    s.push_str("INSTALLERLANDING (SR415): the flight recorder's boot logs (`/var/log/boot-<n>.log`, BOOTLOGCARD B549) read off the card's UnaFS volume by the UnaOS_Installer, read-only.\n\n");
    s.push_str(&format!("- device: `{}`\n", log.source));
    s.push_str(&format!("- image: `{}` (the booted kernel's `:: OLDEST: … image=` sha)\n", log.build.as_deref().unwrap_or("unstamped")));
    s.push_str(&format!("- board: `{}`\n", log.board.as_deref().unwrap_or("-")));
    s.push_str(&format!("- boots: {}\n", log.boots.len()));
    for b in &log.boots {
        s.push_str(&format!("  - `{}/boot{}.log`: {} bytes, boot date {}\n", dir_rel, b.n, b.bytes.len(), date(b.ctime)));
    }
    s.push_str(&format!(
        "- verdicts: {} PASS / {} FAIL / {} SKIP (the `::` lines whose last `->` says PASS, FAIL or SKIP)\n",
        log.census.pass, log.census.fail, log.census.skip
    ));
    s.push_str(&format!("- the kernel's list tally: {}\n", log.bootlist.as_deref().map(|l| format!("`{l}`")).unwrap_or_else(|| String::from("none in the files"))));
    s.push_str(&format!("- left out by the ring: {} byte(s) (the FLIGHTRING cut; the census counts only what the files hold)\n", log.elided));
    s.push_str(&format!("- first verdict: {}\n", log.census.first.as_deref().map(|l| format!("`{l}`")).unwrap_or_else(|| String::from("none"))));
    s.push_str(&format!("- last verdict: {}\n", log.census.last.as_deref().map(|l| format!("`{l}`")).unwrap_or_else(|| String::from("none"))));
    s.push_str("\nHonest limits: the kernel rewrites the log every ~4 s (4096 passes) while the capture grows, so the last seconds before power-off may be missing; a long boot's middle is left out of the file by the ring's 256 KiB budget.\n");
    s
}

/// `PROMPT.md`: the standing landing-report recipe for the local Claude / Gemini session, paths filled in.
pub fn prompt_md(log: &FlightLog, checkout: &Path, dir_rel: &str) -> String {
    let mut s = format!("# Landing prompt — {dir_rel}\n\n");
    s.push_str(&format!(
        "You are the local Claude (or Gemini) session in the UnaOS checkout `{}`. The UnaOS_Installer read a boot of UnaOS back off the card (`{}`); this directory is the evidence. Follow the standing landing-report recipe:\n\n",
        checkout.display(),
        log.source
    ));
    s.push_str(&format!("1. Read `CLAUDE.md` and `docs/dev/LAWS.md` first. Then read `{dir_rel}/landing.md`, then every log whole:\n"));
    for b in &log.boots {
        s.push_str(&format!("   - `{dir_rel}/boot{}.log`\n", b.n));
    }
    s.push_str("   Read serial logs with `awk` (bracketed tags: `awk 'index($0,\"[tag]\")'`), never bare `grep`.\n");
    s.push_str("2. Score every `fixed-unflown` row in `docs/dev/LEDGER.md` and `docs/dev/OS/*-ledger.md` whose wire these logs witness: quote the line (file:line) and say flown-PASS, flown-FAIL or not reached. Do not edit the ledgers; the seat folds.\n");
    s.push_str(&format!(
        "3. Write `{dir_rel}/LANDING.md` in the shape of `{EXEMPLAR}`: the title (machine `{}`, image `{}`, the boot date), the `Medium:` line, `## Peter, verbatim` (ask the operator what the glass showed), one `## <k>. Boot <k>` section per log quoting the wire, the list (the census reads {} PASS / {} FAIL / {} SKIP), the findings (PROVEN on metal / open), and the owed rows.\n",
        log.board.as_deref().unwrap_or("-"),
        log.build.as_deref().unwrap_or("unstamped"),
        log.census.pass,
        log.census.fail,
        log.census.skip
    ));
    s.push_str("4. Name the next arcs: one line each, a NAME, the subsystem, and the wire that will show it fixed, in priority order.\n\n");
    s.push_str("Nothing here calls a model on its own: the installer's `provider send` (INSTALLERPROVIDER SR424) posts this file and landing.md to the AI the person chose, and its reply lands as REPLY.md here.\n");
    s
}

/// Land `log` in `<checkout>/docs/dev/evidence/landing-<stamp>/` (now).
pub fn land(log: &FlightLog, checkout: &Path) -> Result<PathBuf, &'static str> {
    land_at(log, checkout, now_secs())
}

/// Land `log` at the time `now`: the dir (a `-2`, `-3` … suffix when the minute is taken), `boot<n>.log`,
/// `landing.md`, `PROMPT.md`.
pub fn land_at(log: &FlightLog, checkout: &Path, now: u64) -> Result<PathBuf, &'static str> {
    let evidence = checkout.join("docs/dev/evidence");
    if !evidence.is_dir() {
        return Err("no-checkout");
    }
    let base = format!("landing-{}", stamp(now));
    let mut name = base.clone();
    let mut k = 2;
    while evidence.join(&name).exists() {
        name = format!("{base}-{k}");
        k += 1;
    }
    let dir = evidence.join(&name);
    let dir_rel = format!("docs/dev/evidence/{name}");
    std::fs::create_dir_all(&dir).map_err(|_| "no-checkout")?;
    for b in &log.boots {
        std::fs::write(dir.join(format!("boot{}.log", b.n)), &b.bytes).map_err(|_| "no-checkout")?;
    }
    std::fs::write(dir.join("landing.md"), landing_md(log, &dir_rel, now)).map_err(|_| "no-checkout")?;
    std::fs::write(dir.join("PROMPT.md"), prompt_md(log, checkout, &dir_rel)).map_err(|_| "no-checkout")?;
    Ok(dir)
}

/// `[installer] landing device= boots= log_kb= verdicts=<p>/<f>/<s> dir=`.
pub fn line(log: &FlightLog, dir: &Path) -> String {
    format!(
        "[installer] landing device={} boots={} log_kb={} verdicts={}/{}/{} dir={}",
        log.source,
        log.boots.len(),
        log.log_kb(),
        log.census.pass,
        log.census.fail,
        log.census.skip,
        dir.display()
    )
}

/// `[installer] landing refused why=<why> device=<id|->`.
pub fn refused_line(why: &str, device: Option<&str>) -> String {
    format!("[installer] landing refused why={why} device={}", device.unwrap_or("-"))
}

fn refuse(why: &'static str, device: Option<&str>) -> String {
    let l = refused_line(why, device);
    println!("{l}");
    l
}

/// The checkout this vessel was built from (card.rs's `default_image` anchor).
pub fn default_checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ── the face ────────────────────────────────────────────────────────────────────────────────────

enum Source {
    Device(Device),
    Image(PathBuf),
}

impl Source {
    fn label(&self) -> String {
        match self {
            Source::Device(d) => d.id.clone(),
            Source::Image(p) => p.display().to_string(),
        }
    }
}

enum Msg {
    Done(Result<(String, PathBuf), (&'static str, String)>),
}

/// Read, then land, on the read thread; the refusal or the landing line, said once.
fn job(src: Source, key: Option<String>, checkout: PathBuf) -> Result<(String, PathBuf), (&'static str, String)> {
    let label = src.label();
    let fail = |why: &'static str| (why, refuse(why, Some(&label)));
    if !checkout.join("docs/dev/evidence").is_dir() {
        return Err(fail("no-checkout"));
    }
    let read = match &src {
        Source::Device(d) => read_device(d, key.as_deref()),
        Source::Image(p) => read_image(p, key.as_deref()),
    };
    drop(key);
    let log = read.map_err(fail)?;
    let dir = land(&log, &checkout).map_err(fail)?;
    let l = line(&log, &dir);
    println!("{l}");
    Ok((l, dir))
}

/// The landing face's state. ONE owner: the vessel's serve task; the read runs on its own thread, which owns the
/// medium and the mount and reports over a channel this struct drains (no shared lock). The password is held only
/// until the next read takes it, and is never printed or written.
pub struct Landing {
    pub devices: Vec<Device>,
    pub picked: Option<String>,
    /// An image file to read instead of a device (`landing read <path>`).
    pub image: Option<PathBuf>,
    pub checkout: PathBuf,
    /// A keyed refusal is pending: the next raw line no other face took is the password.
    pub need_key: bool,
    /// The last landing's evidence dir.
    pub dir: Option<PathBuf>,
    pub verdict: String,
    key: Option<String>,
    rx: Option<Receiver<Msg>>,
}

impl Landing {
    /// The card face's readable devices and the checkout this vessel was built from.
    pub fn probe() -> Self {
        Self::probe_in(None)
    }

    /// As [`probe`](Self::probe), landing under `checkout` when given (INSTALLERVESSEL's `host::find_checkout`).
    pub fn probe_in(checkout: Option<PathBuf>) -> Self {
        Landing {
            devices: devices(),
            picked: None,
            image: None,
            checkout: checkout.unwrap_or_else(default_checkout),
            need_key: false,
            dir: None,
            verdict: String::from("after the boot, put the card back: pick it, then Read"),
            key: None,
            rx: None,
        }
    }

    pub fn picked_device(&self) -> Option<&Device> {
        self.picked.as_ref().and_then(|id| self.devices.iter().find(|d| &d.id == id))
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Drain the read thread's verdict; true when anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let Ok(Msg::Done(r)) = rx.try_recv() else { return false };
        self.rx = None;
        match r {
            Ok((l, dir)) => {
                self.verdict = l;
                self.dir = Some(dir);
                self.need_key = false;
            }
            Err(("keyed", l)) => {
                self.verdict = format!("{l} — the card's root is keyed: type its password");
                self.need_key = true;
            }
            Err((_, l)) => {
                self.verdict = l;
                self.need_key = false;
            }
        }
        true
    }

    /// Start the read of the image (if one was named) or the picked card on its own thread.
    pub fn start_read(&mut self) {
        if self.busy() {
            return;
        }
        let src = match (&self.image, self.picked_device()) {
            (Some(p), _) => Source::Image(p.clone()),
            (None, Some(d)) => Source::Device(d.clone()),
            (None, None) => {
                self.verdict = String::from("pick a card (landing pick <id>) or name an image (landing read <path>)");
                return;
            }
        };
        let (tx, rx) = channel();
        let (key, checkout) = (self.key.take(), self.checkout.clone());
        self.verdict = format!("reading {} ...", src.label());
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Done(job(src, key, checkout)));
        });
    }

    /// Open the last landing's evidence dir with the host's opener.
    fn open(&mut self) {
        let Some(dir) = self.dir.clone() else {
            self.verdict = String::from("nothing landed yet");
            return;
        };
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        self.verdict = match std::process::Command::new(opener).arg(&dir).spawn() {
            Ok(_) => format!("opened {}", dir.display()),
            Err(e) => format!("{opener} {}: {e}", dir.display()),
        };
    }
}

/// Drain the read thread's verdict into `landing` (the serve task calls it beside `card::poll`).
pub fn poll(landing: &mut Landing) {
    landing.poll();
}

/// The pane INSTALLERVESSEL's window shows (a fresh probe).
pub fn face() -> TetraNode {
    face_of(&Landing::probe())
}

/// The pane for a given state: the checkout, the readable cards, the pick buttons, the password field, Read, Open
/// and the verdict. Every control speaks `SMessage::ConsoleInput`, routed to [`on_input`].
pub fn face_of(l: &Landing) -> TetraNode {
    let line = |level: &str, text: String| ConsoleLine { level: level.to_string(), source: String::from("landing"), text };
    let mut lines = vec![line("info", format!("lands in {}/docs/dev/evidence", l.checkout.display()))];
    if l.devices.is_empty() {
        lines.push(line("warn", String::from("no card found — insert the booted card")));
    }
    for d in &l.devices {
        let mark = if l.picked.as_deref() == Some(d.id.as_str()) { "> " } else { "  " };
        lines.push(line("info", format!("{mark}{}", d.row(0))));
    }
    if let Some(p) = &l.image {
        lines.push(line("info", format!("> image {}", p.display())));
    }
    if l.need_key {
        lines.push(line("warn", String::from("the card's root is keyed — type its password (never shown in the log, never written)")));
    }
    lines.push(line("info", l.verdict.clone()));
    let picks: Vec<TetraNode> = l
        .devices
        .iter()
        .map(|d| TetraNode::Button {
            id: format!("landing.pick.{}", d.id),
            label: d.phrase(),
            action: bandy::SMessage::ConsoleInput(format!("landing pick {}", d.id)),
        })
        .collect();
    let button = |id: &str, label: &str, cmd: &str| TetraNode::Button { id: id.into(), label: label.into(), action: bandy::SMessage::ConsoleInput(cmd.into()) };
    TetraNode::VStack(vec![
        TetraNode::Console(ConsoleTetra { lines, ..Default::default() }),
        TetraNode::HStack(picks),
        TetraNode::TextField { id: String::from("landing.key"), placeholder: String::from("password (a keyed card root)"), action: TextAction::ConsoleInput },
        TetraNode::HStack(vec![button("landing.read", "Read", "landing read"), button("landing.open", "Open", "landing open")]),
    ])
}

/// Route one `SMessage::ConsoleInput` line: `landing pick <id>`, `landing read [<image path>]`, `landing key
/// <password>`, `landing open`, `landing rescan`, or — with a keyed refusal pending — the password itself (a raw line
/// no other face took). False = not the landing face's line.
pub fn on_input(l: &mut Landing, line: &str) -> bool {
    l.poll();
    let t = line.trim();
    let Some(rest) = t.strip_prefix("landing ").map(str::trim) else {
        if l.need_key && !l.busy() && !t.is_empty() {
            l.key = Some(t.to_string());
            l.start_read();
            return true;
        }
        return false;
    };
    let (verb, arg) = rest.split_once(' ').map(|(v, a)| (v, a.trim())).unwrap_or((rest, ""));
    match verb {
        "pick" if !l.busy() => match l.devices.iter().find(|d| d.id == arg) {
            Some(d) => {
                l.verdict = format!("{} picked — press Read", d.id);
                l.picked = Some(d.id.clone());
                l.image = None;
                l.need_key = false;
                l.key = None;
            }
            None => l.verdict = format!("no readable card {arg}"),
        },
        "read" if !l.busy() => {
            if !arg.is_empty() {
                l.image = Some(PathBuf::from(arg));
                l.picked = None;
            }
            l.start_read();
        }
        "key" if !l.busy() => {
            l.key = Some(arg.to_string());
            l.start_read();
        }
        "open" => l.open(),
        "rescan" if !l.busy() => {
            l.devices = devices();
            if l.picked_device().is_none() {
                l.picked = None;
            }
        }
        _ if l.busy() => l.verdict = String::from("busy reading — wait for the verdict"),
        _ => l.verdict = format!("unknown landing verb `{verb}` (pick / read / key / open / rescan)"),
    }
    true
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use amber_core::plan::{PartKind, PartReq, Plan, Size};
    use std::fs;

    const BOOT1: &[u8] = include_bytes!("../tests/fixtures/landing-boot1.log");
    const BOOT2: &[u8] = include_bytes!("../tests/fixtures/landing-boot2.log");
    /// The ctime every fixture inode carries (2026-10-27 23:40 UTC).
    const CTIME: u64 = 1_793_144_400;

    fn fixed_clock() -> u64 {
        CTIME
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("installerlanding-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Lay a card image the way the real one is laid: amber_core's GPT (p1 ESP, p2 UnaFS), `unafs::format_partition`
    /// on p2 (skipped when `with_unafs` is false), then each `(path, bytes)` written through the unafs core; keyed with
    /// `key` at the KDF floor when given.
    fn lay(path: &Path, files: &[(&str, &[u8])], with_unafs: bool, key: Option<&str>) {
        let plan = Plan::for_image(
            b"LANDING-TEST",
            &[
                PartReq { kind: PartKind::Esp, size: Size::Sectors(2048), name: "UNAOS-ESP", seed: b"LANDING-ESP" },
                PartReq { kind: PartKind::UnaFS, size: Size::Sectors(16384), name: "UNAOS-UNAFS", seed: b"LANDING-UFS" },
            ],
        )
        .unwrap();
        fs::File::create(path).unwrap().set_len(plan.disk_sectors * 512).unwrap();
        let mut blk = FileBlock::open_rw(path).unwrap();
        amber_core::plan_apply::apply(&plan, &mut blk, amber_core::plan_apply::Mode::Write).unwrap();
        if !with_unafs {
            return;
        }
        let p2 = &plan.parts[1];
        unafs::clock::set_clock_hook(fixed_clock);
        let mut s = Sectors(&mut blk);
        let made = unafs::format_partition(&mut s, p2.first, p2.sectors(), &unafs::FormatParams::default()).unwrap();
        let mut fs = UnaFS::mount(BlockAdapter::new(&mut s, p2.first, made.blocks)).unwrap();
        for (p, bytes) in files {
            let mut parent = fs.resolve_path("/").unwrap();
            let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
            let (dirs, name) = parts.split_at(parts.len() - 1);
            let mut at = String::new();
            for d in dirs {
                at.push('/');
                at.push_str(d);
                parent = match fs.resolve_path(&at) {
                    Ok(id) => id,
                    Err(_) => fs.mkdir(parent, d.to_string()).unwrap(),
                };
            }
            let id = fs.create_file(parent, name[0].to_string()).unwrap();
            fs.write_data(id, 0, bytes).unwrap();
        }
        fs.commit().unwrap();
        if let Some(k) = key {
            let mut rng = |b: &mut [u8]| {
                for (i, x) in b.iter_mut().enumerate() {
                    *x = (i as u8).wrapping_mul(37).wrapping_add(11);
                }
            };
            let (slot, _words) = unafs::KeySlot::seal(k.as_bytes(), unafs::KdfParams::FLOOR, &mut rng).unwrap();
            fs.set_key_slot(slot).unwrap();
        }
    }

    #[test]
    fn landing_reads_flight_logs_off_a_card_image() {
        let dir = scratch("read");
        let img = dir.join("card.img");
        lay(&img, &[("/var/log/boot-2.log", BOOT2), ("/var/log/boot-1.log", BOOT1), ("/var/log/notes.txt", b"not a boot log")], true, None);
        let before = fs::read(&img).unwrap();
        let log = read_image(&img, None).unwrap();
        assert_eq!(fs::read(&img).unwrap(), before, "the read never writes the medium");
        assert_eq!(log.boots.iter().map(|b| b.n).collect::<Vec<_>>(), [1, 2], "oldest first; notes.txt is not a boot log");
        assert_eq!(log.boots[0].bytes, BOOT1);
        assert_eq!(log.boots[1].bytes, BOOT2);
        assert!(log.boots.iter().all(|b| b.ctime == CTIME));
        assert_eq!((log.census.pass, log.census.fail, log.census.skip), (4, 1, 2));
        assert!(log.census.first.as_deref().unwrap().starts_with(":: QUIETBOOT:"));
        assert!(log.census.last.as_deref().unwrap().contains(":: NAMEINDEXTX: -> SKIP(budget 60s) ::"));
        assert_eq!(log.build.as_deref(), Some("c1d61903"));
        assert_eq!(log.board.as_deref(), Some("zenbook"));
        assert_eq!(log.bootlist.as_deref(), Some(":: BOOTLIST: ran=3 pass=1 fail=1 skip=1 heavy=0 ms=51002 ::"));
        assert_eq!(log.elided, 51234);
        assert_eq!(log.bytes(), (BOOT1.len() + BOOT2.len()) as u64);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn census_rules() {
        assert_eq!(verdict(":: MENUBAR: rows=7 -> PASS ::"), Some("PASS"));
        assert_eq!(verdict("[00:00:01.000] :: DOCKPINS: pins=9 -> PASS ::"), Some("PASS"), "the logts prefix");
        assert_eq!(verdict(":: X: -> SKIP(budget 60s) ::"), Some("SKIP"));
        assert_eq!(verdict(":: X: tx NOT ATTEMPTED -> SKIP reason=no-mac ::"), Some("SKIP"));
        assert_eq!(verdict("PUMPWEDGE: age_ms=2171 -> FAIL :: pump=9@cpu7"), Some("FAIL"), "-> FAIL with `::` after it");
        assert_eq!(verdict(":: OLDEST: board=b image=s -> BOOTS ::"), None);
        assert_eq!(verdict("[gen7] r10 draw=3dprimitive -> PASS"), None, "not a `::` line");
        assert_eq!(verdict("[boot] log path=/var/log/boot-1.log -> written ::"), None);
        assert_eq!(verdict(":: A: x -> PASS then -> FAIL ::"), Some("FAIL"), "the LAST arrow decides");
        let c = census(std::str::from_utf8(BOOT1).unwrap());
        assert_eq!((c.pass, c.fail, c.skip), (3, 0, 1));
        let mut all = c.clone();
        all.merge(census(std::str::from_utf8(BOOT2).unwrap()));
        assert_eq!((all.pass, all.fail, all.skip), (4, 1, 2));
        assert_eq!(all.first, c.first);
        all.merge(Census::default());
        assert!(all.last.is_some(), "an empty later boot keeps the last verdict");
    }

    #[test]
    fn refusals_by_name() {
        let dir = scratch("refuse");
        assert_eq!(read_image(&dir.join("absent.img"), None), Err("unreadable"));
        fs::write(dir.join("empty.img"), b"").unwrap();
        assert_eq!(read_image(&dir.join("empty.img"), None), Err("unreadable"), "0 sectors (a macOS disk node reads so)");
        let bare = dir.join("bare.img");
        lay(&bare, &[], false, None);
        assert_eq!(read_image(&bare, None), Err("no-unafs"));
        let nolog = dir.join("nolog.img");
        lay(&nolog, &[("/home/u/readme", b"x")], true, None);
        assert_eq!(read_image(&nolog, None), Err("no-log"));
        let other = dir.join("other.img");
        lay(&other, &[("/var/log/kern.txt", b"x")], true, None);
        assert_eq!(read_image(&other, None), Err("no-log"), "a /var/log without boot-<n>.log");
        let log = flight_log("x", vec![Boot { n: 1, bytes: BOOT1.to_vec(), ctime: 0 }]);
        assert_eq!(land_at(&log, &dir.join("not-a-checkout"), CTIME), Err("no-checkout"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keyed_root_opens_only_with_its_password() {
        let dir = scratch("keyed");
        let img = dir.join("keyed.img");
        lay(&img, &[("/var/log/boot-1.log", BOOT1)], true, Some("correct horse"));
        assert_eq!(read_image(&img, None), Err("keyed"));
        assert_eq!(read_image(&img, Some("wrong horse")), Err("keyed"));
        let log = read_image(&img, Some("correct horse")).unwrap();
        assert_eq!(log.boots.len(), 1);
        assert_eq!(log.boots[0].bytes, BOOT1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn land_writes_the_evidence_dir() {
        let root = scratch("land");
        fs::create_dir_all(root.join("docs/dev/evidence")).unwrap();
        let log = flight_log("mmcblk0", vec![Boot { n: 1, bytes: BOOT1.to_vec(), ctime: CTIME }, Boot { n: 2, bytes: BOOT2.to_vec(), ctime: 0 }]);
        let dir = land_at(&log, &root, CTIME).unwrap();
        assert_eq!(dir, root.join("docs/dev/evidence/landing-20261027-2340"));
        assert_eq!(fs::read(dir.join("boot1.log")).unwrap(), BOOT1);
        assert_eq!(fs::read(dir.join("boot2.log")).unwrap(), BOOT2);
        let md = fs::read_to_string(dir.join("landing.md")).unwrap();
        assert!(md.starts_with("# Landing — mmcblk0, 2026-10-27 23:40Z\n"));
        assert!(md.contains("- device: `mmcblk0`"));
        assert!(md.contains("- image: `c1d61903`"));
        assert!(md.contains("- board: `zenbook`"));
        assert!(md.contains("`docs/dev/evidence/landing-20261027-2340/boot1.log`: "));
        assert!(md.contains("boot date 2026-10-27 23:40Z"));
        assert!(md.contains("boot date unknown (the kernel had no clock)"));
        assert!(md.contains("- verdicts: 4 PASS / 1 FAIL / 2 SKIP"));
        assert!(md.contains("- left out by the ring: 51234 byte(s)"));
        assert!(md.contains("- first verdict: `:: QUIETBOOT: lines=212 bound=400 census=0 -> PASS ::`"));
        assert!(md.contains("- last verdict: `[00:12:30.000] :: NAMEINDEXTX: -> SKIP(budget 60s) ::`"));
        let p = fs::read_to_string(dir.join("PROMPT.md")).unwrap();
        assert!(p.starts_with("# Landing prompt — docs/dev/evidence/landing-20261027-2340\n"));
        assert!(p.contains(&format!("UnaOS checkout `{}`", root.display())));
        assert!(p.contains("`docs/dev/evidence/landing-20261027-2340/boot2.log`"));
        assert!(p.contains("`docs/dev/evidence/landing-20261027-2340/landing.md`"));
        assert!(p.contains("`fixed-unflown`"));
        assert!(p.contains(EXEMPLAR));
        assert!(p.contains("awk 'index($0,\"[tag]\")'"));
        assert!(p.contains("machine `zenbook`, image `c1d61903`"));
        assert_eq!(land_at(&log, &root, CTIME).unwrap(), root.join("docs/dev/evidence/landing-20261027-2340-2"));
        assert_eq!(land_at(&log, &root, CTIME).unwrap(), root.join("docs/dev/evidence/landing-20261027-2340-3"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn wire_lines() {
        let log = flight_log("mmcblk0", vec![Boot { n: 1, bytes: BOOT1.to_vec(), ctime: 0 }, Boot { n: 2, bytes: BOOT2.to_vec(), ctime: 0 }]);
        let kb = ((BOOT1.len() + BOOT2.len()) as u64).div_ceil(1024);
        assert_eq!(
            line(&log, Path::new("/u/UnaOS/docs/dev/evidence/landing-20261027-2340")),
            format!("[installer] landing device=mmcblk0 boots=2 log_kb={kb} verdicts=4/1/2 dir=/u/UnaOS/docs/dev/evidence/landing-20261027-2340")
        );
        assert_eq!(refused_line("keyed", Some("sdb")), "[installer] landing refused why=keyed device=sdb");
        assert_eq!(refused_line("no-checkout", None), "[installer] landing refused why=no-checkout device=-");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0));
        assert_eq!(civil(CTIME), (2026, 10, 27, 23, 40));
        assert_eq!(civil(4_107_542_399), (2100, 2, 28, 23, 59));
        assert_eq!(stamp(CTIME), "20261027-2340");
        assert_eq!(date(CTIME), "2026-10-27 23:40Z");
        assert_eq!(date(0), "unknown (the kernel had no clock)");
    }

    #[test]
    fn face_and_input_route() {
        let sd = Device { id: "mmcblk0".into(), node: "/nonexistent/mmcblk0".into(), bytes: 63_864_569_856, removable: true, internal: false, model: "SD".into(), mounts: vec![], holds_os: false };
        let mut l = Landing {
            devices: vec![sd],
            picked: None,
            image: None,
            checkout: "/nonexistent/checkout".into(),
            need_key: false,
            dir: None,
            verdict: String::new(),
            key: None,
            rx: None,
        };
        assert!(!on_input(&mut l, "ls /"), "not the landing face's line");
        assert!(on_input(&mut l, "landing read"));
        assert!(l.verdict.starts_with("pick a card"), "nothing picked: nothing read");
        assert!(on_input(&mut l, "landing pick sda"));
        assert_eq!(l.picked, None);
        assert!(on_input(&mut l, "landing pick mmcblk0"));
        assert_eq!(l.picked.as_deref(), Some("mmcblk0"));
        assert!(on_input(&mut l, "landing open"));
        assert_eq!(l.verdict, "nothing landed yet");
        assert!(on_input(&mut l, "landing frob"));
        assert!(l.verdict.contains("unknown landing verb `frob`"));
        // A keyed refusal pending: a raw line is the password, consumed and moved into the read (not kept, not shown).
        l.need_key = true;
        assert!(on_input(&mut l, "hunter2"));
        assert!(l.busy());
        assert!(l.key.is_none(), "the read took the password");
        assert!(!l.verdict.contains("hunter2"));
        while l.busy() {
            l.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(l.verdict, "[installer] landing refused why=no-checkout device=mmcblk0");
        assert!(!l.need_key);
        assert!(!on_input(&mut l, "hunter2"), "no key pending: a raw line is not the landing face's");
        let TetraNode::VStack(kids) = face_of(&l) else { panic!("face is a VStack") };
        assert_eq!(kids.len(), 4);
        let TetraNode::HStack(picks) = &kids[1] else { panic!("picks row") };
        assert_eq!(picks.len(), 1);
        let TetraNode::Console(con) = &kids[0] else { panic!("list") };
        assert!(con.lines.iter().any(|x| x.text.starts_with("> mmcblk0")));
        assert!(con.lines.iter().all(|x| !x.text.contains("hunter2")));
    }
}
