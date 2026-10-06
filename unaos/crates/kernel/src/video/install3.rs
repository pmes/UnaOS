// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (the installer window's five SSD screens; every verdict is selfinstall/selfguard's, every plan amber_core's, shared-core; the one write path is `install ssd --write`'s body)
//!
//! INSTALL3 (rmbp-ledger B342) — the installer's glass for the two-partition SSD install AHCIROOT
//! (B332) made real. A child module of [`super`] (`video/instgui.rs`): it paints into the dialog's
//! surface with the dialog's own helpers and takes the dialog's keys while it is up. Key `i` on the
//! chooser opens it; Esc on the census (or leaving the result) gives the chooser back.
//!
//! * **CENSUS** — every disk the block layer publishes (each AHCI port, the global and USB rows), its
//!   GPT (`install::gpt::read_table`, which is `amber_core::gpt::read_table_with`) and the verdict in
//!   words: empty / ours / foreign (the OS named) / unknown, plus the boot disk and the running root.
//!   Only SATA rows are selectable: this installer writes the internal SSD.
//! * **LAYOUT** — ESP size and UnaFS partition size (default: the rest; the last 64 sectors are the
//!   scratch tail), echoed as `amber_core::Plan::lines()` — the dry run's own sector list.
//! * **CONFIRM** — one Enter for an empty or ours disk; for a disk carrying somebody's OS (or content
//!   nobody can name) the operator TYPES the disk's name, which becomes the shell's confirmation token
//!   (`selfinstall::stranger_token`) and enters the same check `--erase-stranger` reaches. A disk the
//!   guard refuses shows the refusal in words; Enter still goes through the judgment, which refuses.
//! * **PROGRESS** — the write body's `[install] stage=` lines, painted as they arrive.
//! * **RESULT** — the verdict, and `r` = reboot.
//!
//! Nothing here mints, holds or writes: the confirm calls `selfinstall::write_ssd_glass`, i.e. the
//! `install ssd --write` body. `tests instgui` drives all five screens with synthetic keys on its
//! DRY-RUN leg (the grant is minted from the verdict and never held; no stage past it touches a disk).

use super::{button, fill, rect, text, theme, CELL_H, H, W};
use crate::drivers::block::{self, BlockHandle};
use crate::install::selfinstall;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Census,
    Layout,
    Confirm,
    Progress,
    Result,
}

impl Screen {
    fn bit(self) -> u8 {
        1 << (self as u8)
    }
    fn tag(self) -> &'static str {
        match self {
            Screen::Census => "census",
            Screen::Layout => "layout",
            Screen::Confirm => "confirm",
            Screen::Progress => "progress",
            Screen::Result => "result",
        }
    }
}

const ALL: [Screen; 5] = [Screen::Census, Screen::Layout, Screen::Confirm, Screen::Progress, Screen::Result];

/// What the census concluded about one disk — the words on glass, and what the confirm screen asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Nothing to lose (zero head, or a GPT with no partitions). One Enter.
    Empty,
    /// Every partition ours (UnaOS ESP + UnaFS). One Enter; an existing UnaFS volume is replaced.
    Ours,
    /// Somebody's OS lives here, and the census can name it. Typed confirmation.
    Foreign,
    /// Content nobody can name. UNKNOWN IS NOT EMPTY: typed confirmation.
    Unknown,
    /// selfguard: the disk this system booted from. Refused.
    Boot,
    /// AHCIROOT: this disk's UnaFS is the running root (the boot's root grant names it). Refused.
    LiveRoot,
    /// The probe could not read it. Refused.
    Unreadable,
    /// Not a SATA disk — listed for the census, never selectable here.
    NotSata,
}

impl Kind {
    fn typed(self) -> bool {
        matches!(self, Kind::Foreign | Kind::Unknown)
    }
    fn refused(self) -> bool {
        matches!(self, Kind::Boot | Kind::LiveRoot | Kind::Unreadable)
    }
    fn tag(self) -> &'static str {
        match self {
            Kind::Empty => "empty",
            Kind::Ours => "ours",
            Kind::Foreign => "foreign",
            Kind::Unknown => "unknown",
            Kind::Boot => "boot",
            Kind::LiveRoot => "live-root",
            Kind::Unreadable => "unreadable",
            Kind::NotSata => "not-sata",
        }
    }
}

struct Disk {
    /// `ahci0`, `usb`, `global`, or `synthetic` (the test's stand-in on a machine with no SATA disk).
    name: String,
    model: String,
    sectors: u64,
    /// The SATA port; `None` for a non-SATA row (never selectable).
    port: Option<u8>,
    gpt: String,
    parts: Vec<String>,
    kind: Kind,
    words: String,
    unafs_present: bool,
}

impl Disk {
    /// The name the operator types to erase a foreign disk: its model string, or `ahci<port>`.
    fn confirm_name(&self) -> &str {
        if self.model.trim().is_empty() {
            self.name.as_str()
        } else {
            self.model.trim()
        }
    }
}

/// ESP sizes the layout screen offers (MiB); 512 is the verb's own.
const ESP_MIB: [u64; 3] = [256, 512, 1024];
/// The UnaFS partition steps: the rest, 3/4, 1/2, 1/4 of it, the minimum.
const UFS_STEPS: [&str; 5] = ["the rest", "3/4 of the rest", "1/2 of the rest", "1/4 of the rest", "the minimum"];

struct Model {
    screen: Screen,
    disks: Vec<Disk>,
    sel: usize,
    esp_ix: usize,
    ufs_ix: usize,
    /// The running UnaFS volume's sectors when there is one to mirror.
    mirror: Option<u64>,
    typed: String,
    note: Option<&'static str>,
    stages: Vec<(String, String)>,
    /// `(grant issued, pass, reason)` once the confirm ran.
    outcome: Option<(bool, bool, String)>,
    last_out: String,
    dry: bool,
    visited: u8,
    painted: u8,
    /// The result screen offered `r` = reboot (painted).
    offered_reboot: bool,
    /// The typed confirmation was refused at least once (the negative leg of the test).
    mismatch_seen: bool,
}

static M: crate::sync::Mutex<Option<Model>> = crate::sync::Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set while `tests instgui` drives the screens: `r` never reboots, and the confirm is always DRY.
static TEST: AtomicBool = AtomicBool::new(false);
static ESP_DEFAULT: AtomicU8 = AtomicU8::new(1);

// ------------------------------------------------------------------- census --

fn trim_ident(s: &[u8]) -> String {
    let end = s.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |p| p + 1);
    s[..end].iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect()
}

/// The foreign OS a census row names, from its CONTENT first (what is there) and its TYPE GUID second
/// (what somebody declared).
fn os_name(r: &crate::install::partition::CensusRow) -> Option<&'static str> {
    use crate::install::partition::{foreign_type, Content};
    match r.content {
        Content::Apfs => return Some("macOS (APFS)"),
        Content::HfsPlus => return Some("macOS (HFS+)"),
        _ => {}
    }
    match foreign_type(&r.entry.type_guid) {
        Some("apple-apfs") => Some("macOS (APFS)"),
        Some("apple-hfs+") => Some("macOS (HFS+)"),
        Some(n) if n.starts_with("apple") => Some("macOS"),
        Some(n) if n.starts_with("linux") => Some("Linux"),
        Some(_) => Some("Windows"),
        None if r.content == Content::Fat && !r.entry.is_esp() => Some("a FAT volume"),
        None => None,
    }
}

fn sata_row(port: u8, info: block::BlockDeviceInfo) -> Disk {
    use crate::install::selfinstall::Verdict;
    let id = info.id(BlockHandle::Ahci { port });
    let mut d = Disk {
        name: alloc::format!("ahci{}", port),
        model: alloc::format!("{} {}", trim_ident(&info.vendor), trim_ident(&info.product)).trim().into(),
        sectors: info.num_blocks,
        port: Some(port),
        gpt: String::from("-"),
        parts: Vec::new(),
        kind: Kind::Unreadable,
        words: String::from("unknown — the disk cannot be read"),
        unafs_present: false,
    };
    let p = match selfinstall::probe(id, port) {
        Ok(p) => p,
        Err(_) => return d,
    };
    d.model = p.model.clone();
    d.sectors = p.sectors;
    d.unafs_present = p.unafs_present;
    d.gpt = match p.gpt {
        "valid" => alloc::format!("GPT, {} partitions", p.parts),
        "none" => String::from("no partition table"),
        other => alloc::format!("GPT {}", other),
    };
    let mut names: Vec<&'static str> = Vec::new();
    if let Ok(t) = crate::install::BlockTarget::bind_id(id) {
        if let Ok(c) = crate::install::partition::census(&t) {
            for r in &c.rows {
                d.parts.push(alloc::format!("p{} {} MiB {}", r.entry.index, r.entry.sectors() / 2048, r.content.tag()));
                if let Some(n) = os_name(r) {
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
    }
    let live_root = matches!(block::ahci_live_grant(), Some((gp, _, _, block::GrantKind::Root)) if gp == port);
    (d.kind, d.words) = if crate::install::selfguard::refuses(id) {
        (Kind::Boot, String::from("the boot disk — UnaOS never erases it"))
    } else if live_root {
        (Kind::LiveRoot, String::from("ours — its UnaFS is the running root"))
    } else {
        match &p.verdict {
            Verdict::Blank => (Kind::Empty, String::from("empty — nothing to lose")),
            Verdict::Ours if p.unafs_present => (Kind::Ours, String::from("ours (UnaOS) — its UnaFS is replaced")),
            Verdict::Ours => (Kind::Ours, String::from("ours (UnaOS)")),
            _ if !names.is_empty() => (Kind::Foreign, alloc::format!("foreign: {}", names.join(", "))),
            _ => (Kind::Unknown, String::from("unknown — content nobody can name")),
        }
    };
    d
}

fn other_row(name: &str, handle: BlockHandle, info: block::BlockDeviceInfo) -> Disk {
    let id = info.id(handle);
    let gpt = match crate::install::BlockTarget::bind_id(id).map(|t| crate::install::gpt::read_table(&t)) {
        Ok(Ok(tb)) => alloc::format!("GPT, {} partitions", tb.entries.len()),
        Ok(Err(_)) => String::from("no readable GPT"),
        Err(_) => String::from("-"),
    };
    let boot = crate::install::selfguard::refuses(id);
    Disk {
        name: String::from(name),
        model: alloc::format!("{} {}", trim_ident(&info.vendor), trim_ident(&info.product)).trim().into(),
        sectors: info.num_blocks,
        port: None,
        gpt,
        parts: Vec::new(),
        kind: Kind::NotSata,
        words: String::from(if boot { "the boot disk (not SATA)" } else { "not SATA — this installer writes the SSD" }),
        unafs_present: false,
    }
}

/// Every disk the block layer publishes, read-only.
fn take_census() -> Vec<Disk> {
    let mut v = Vec::new();
    for ix in 0..block::MAX_AHCI_DISKS {
        if let Some(d) = block::ahci_disk(ix) {
            v.push(sata_row(d.port, d.info));
        }
    }
    let g = block::info();
    if let Some(i) = g {
        v.push(other_row("global", BlockHandle::Global, i));
    }
    if let Some(u) = block::usb_info() {
        if g.map(|i| i.slot_id) != Some(u.slot_id) {
            v.push(other_row("usb", BlockHandle::Usb, u));
        }
    }
    #[cfg(feature = "sdhcblk")] if let Some(sd) = block::sdhc_info() { let mut row = other_row("sdhc", BlockHandle::Sdhc, sd); row.words = String::from(if row.words.starts_with("the boot disk") { "the boot SD card (read-only here)" } else { "the SD card (read-only here)" }); v.push(row); } // UNAFSGROW (B347) M3: the card reads (BlockTarget Sdhc), so it is a census row; never selectable
    v
}

/// `tests instgui` on a machine with no SATA disk: a blank 500 GB stand-in on a port nothing answers,
/// so the judgment refuses it (no AHCI disk on that port) and the refusal path is the one driven.
fn synthetic() -> Disk {
    Disk {
        name: String::from("synthetic"),
        model: String::from("SYNTHETIC SSD"),
        sectors: 976_773_168,
        port: Some(0xFF),
        gpt: String::from("no partition table"),
        parts: Vec::new(),
        kind: Kind::Empty,
        words: String::from("empty (synthetic: tests only)"),
        unafs_present: false,
    }
}

// ------------------------------------------------------------------- layout --

impl Model {
    fn disk(&self) -> Option<&Disk> {
        self.disks.get(self.sel)
    }
    fn esp_sectors(&self) -> u64 {
        ESP_MIB[self.esp_ix] * 2048
    }
    /// The p2 request for the current step: `None` = the rest.
    fn p2(&self) -> Option<u64> {
        let d = self.disk()?;
        let min = selfinstall::p2_min_sectors(self.mirror);
        if self.ufs_ix == 0 {
            return None;
        }
        let rest = selfinstall::ssd_plan_layout(d.sectors, self.esp_sectors(), None, self.mirror)
            .ok()
            .and_then(|p| p.part(amber_core::PartKind::UnaFS).map(|u| u.sectors()))
            .unwrap_or(min);
        let n = match self.ufs_ix {
            1 => rest / 4 * 3,
            2 => rest / 2,
            3 => rest / 4,
            _ => min,
        };
        Some((n / 2048 * 2048).max(min))
    }
    fn layout(&self) -> (u64, Option<u64>) {
        (self.esp_sectors(), self.p2())
    }
    fn plan(&self) -> Result<amber_core::Plan, amber_core::PlanError> {
        let sectors = self.disk().map_or(0, |d| d.sectors);
        let (e, p2) = self.layout();
        selfinstall::ssd_plan_layout(sectors, e, p2, self.mirror)
    }
    fn selectable(&self, i: usize) -> bool {
        self.disks.get(i).is_some_and(|d| d.port.is_some())
    }
    fn step(&mut self, dir: isize) {
        let mut i = self.sel as isize;
        loop {
            i += dir;
            if i < 0 || i >= self.disks.len() as isize {
                return;
            }
            if self.selectable(i as usize) {
                self.sel = i as usize;
                return;
            }
        }
    }
    fn go(&mut self, s: Screen) {
        self.screen = s;
        self.visited |= s.bit();
        crate::census_println!("[install3] screen={}", s.tag());
    }
}

// ------------------------------------------------------------------ entries --

/// Open the five screens on a fresh census (key `i` on the chooser, or the test).
fn enter(dry: bool) {
    let mut disks = take_census();
    if TEST.load(Ordering::Relaxed) && !disks.iter().any(|d| d.port.is_some()) {
        disks.insert(0, synthetic());
    }
    let sel = disks.iter().position(|d| d.port.is_some()).unwrap_or(0);
    for d in &disks {
        serial_println!("[install3] census {} model={} sectors={} gpt={} verdict={} ({})", d.name, d.model, d.sectors, d.gpt, d.kind.tag(), d.words);
    }
    let mut m = Model {
        screen: Screen::Census,
        disks,
        sel,
        esp_ix: ESP_DEFAULT.load(Ordering::Relaxed) as usize,
        ufs_ix: 0,
        mirror: selfinstall::mirror_source_sectors(),
        typed: String::new(),
        note: None,
        stages: Vec::new(),
        outcome: None,
        last_out: String::new(),
        dry,
        visited: 0,
        painted: 0,
        offered_reboot: false,
        mismatch_seen: false,
    };
    m.go(Screen::Census);
    *M.lock() = Some(m);
    ACTIVE.store(true, Ordering::Release);
}

/// Back to the chooser (the dialog stays open).
fn leave() {
    ACTIVE.store(false, Ordering::Release);
    *M.lock() = None;
}

/// The dialog closed under us: drop the screens without a repaint.
pub(super) fn leave_silent() {
    leave();
}

/// The confirm: through `selfinstall::write_ssd_glass` — the `install ssd --write` body — with the
/// token the typed name became (foreign disks), the layout, and the dry-run choice. Runs with the model
/// UNLOCKED: the stage callback takes the lock to append, then repaints.
fn confirm() {
    let (port, token, layout, dry) = {
        let mut g = M.lock();
        let Some(m) = g.as_mut() else { return };
        let Some(d) = m.disk() else { return };
        let Some(port) = d.port else { return };
        let token = d.kind.typed().then(|| selfinstall::stranger_token(port, d.sectors));
        let layout = m.layout();
        let dry = m.dry || TEST.load(Ordering::Relaxed);
        m.stages.clear();
        m.outcome = None;
        m.go(Screen::Progress);
        (port, token, layout, dry)
    };
    super::repaint();
    serial_println!(
        "[install3] confirm ahci:{} esp={} MiB p2={} confirmation={} dry_run={}",
        port,
        layout.0 / 2048,
        layout.1.map_or(String::from("rest"), |n| alloc::format!("{} MiB", n / 2048)),
        if token.is_some() { "typed-name" } else { "one-press" },
        dry as u8
    );
    let mut on_stage = |name: &str, what: &str| {
        if let Some(m) = M.lock().as_mut() {
            m.stages.push((String::from(name), String::from(what)));
        }
        super::repaint();
    };
    let mut last = String::new();
    let mut out = |l: &str| {
        last = String::from(l);
    };
    let o = selfinstall::write_ssd_glass(
        &mut out,
        token.as_deref(),
        selfinstall::Glass { port, layout: Some(layout), dry_run: dry, stage: &mut on_stage },
    );
    if let Some(m) = M.lock().as_mut() {
        m.outcome = Some((o.grant, o.pass, o.reason));
        m.last_out = last;
        m.go(Screen::Result);
    }
    super::repaint();
}

/// Keys. `st` is the dialog's own state; `i` on the chooser opens the screens. Returns true when the
/// key was ours; `q` outside the typed field is left to the dialog (halt).
pub(super) fn key(st: super::State, c: u8) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        if st == super::State::Choose && c == b'i' {
            enter(false);
            super::repaint();
            return true;
        }
        return false;
    }
    let screen = match M.lock().as_ref() {
        Some(m) => m.screen,
        None => {
            leave();
            return false;
        }
    };
    let typing = screen == Screen::Confirm && M.lock().as_ref().and_then(|m| m.disk().map(|d| d.kind.typed())).unwrap_or(false);
    if c == b'q' && !typing && screen != Screen::Progress {
        leave();
        return false; // the dialog's own `q`: halt
    }
    let mut run_confirm = false;
    {
        let mut g = M.lock();
        let Some(m) = g.as_mut() else { return false };
        match (screen, c) {
            (Screen::Census, b'w') | (Screen::Census, b'A') => m.step(-1),
            (Screen::Census, b's') | (Screen::Census, b'B') => m.step(1),
            (Screen::Census, b'\r') | (Screen::Census, b'\n') => {
                if m.selectable(m.sel) {
                    m.ufs_ix = 0;
                    m.go(Screen::Layout);
                }
            }
            (Screen::Census, 0x1b) => {
                drop(g);
                leave();
                super::repaint();
                return true;
            }
            (Screen::Layout, b'e') => m.esp_ix = (m.esp_ix + 1) % ESP_MIB.len(),
            (Screen::Layout, b's') | (Screen::Layout, b'B') => m.ufs_ix = (m.ufs_ix + 1).min(UFS_STEPS.len() - 1),
            (Screen::Layout, b'w') | (Screen::Layout, b'A') => m.ufs_ix = m.ufs_ix.saturating_sub(1),
            (Screen::Layout, b'\r') | (Screen::Layout, b'\n') => {
                if m.plan().is_ok() {
                    m.typed.clear();
                    m.note = None;
                    m.go(Screen::Confirm);
                } else {
                    m.note = Some("the disk cannot hold this layout");
                }
            }
            (Screen::Layout, 0x1b) => m.go(Screen::Census),
            (Screen::Confirm, 0x1b) => {
                m.note = None;
                m.go(Screen::Layout);
            }
            (Screen::Confirm, b'\r') | (Screen::Confirm, b'\n') => {
                let ok = match m.disk() {
                    Some(d) if d.kind.typed() => m.typed.trim().eq_ignore_ascii_case(d.confirm_name()),
                    Some(_) => true,
                    None => false,
                };
                if ok {
                    run_confirm = true;
                } else {
                    m.note = Some("the name does not match — nothing was issued");
                    m.mismatch_seen = true;
                    serial_println!("[install3] typed confirmation does not match the disk's name — no grant, nothing written");
                }
            }
            (Screen::Confirm, 0x08) | (Screen::Confirm, 0x7f) if typing => {
                m.typed.pop();
            }
            (Screen::Confirm, ch) if typing && (0x20..0x7f).contains(&ch) => {
                if m.typed.len() < 48 {
                    m.typed.push(ch as char);
                }
            }
            (Screen::Result, b'r') => {
                if TEST.load(Ordering::Relaxed) {
                    serial_println!("[install3] reboot refused: tests instgui never reboots");
                } else {
                    serial_println!("[install3] reboot requested on the result screen");
                    drop(g);
                    crate::power::reboot();
                }
            }
            (Screen::Result, 0x1b) | (Screen::Result, b'\r') | (Screen::Result, b'\n') => {
                drop(g);
                leave();
                super::repaint();
                return true;
            }
            _ => {}
        }
    }
    if run_confirm {
        confirm();
    } else {
        super::repaint();
    }
    true
}

// ----------------------------------------------------------------- painting --

const LX: usize = 24;
const STEP: usize = CELL_H + 4;
const COLS: usize = (W - 2 * LX) / super::CELL_W;

fn line(px: &mut [u32], y: usize, s: &str, fg: u32) {
    let b = s.as_bytes();
    text(px, LX, y, &b[..b.len().min(COLS)], fg);
}

fn title(px: &mut [u32], s: &str) {
    line(px, 20, s, theme::CONTENT_TEXT);
    fill(px, LX, 42, W - 2 * LX, 2, theme::FRAME_LINE);
}

fn hint(px: &mut [u32], s: &str) {
    line(px, H - 40, s, theme::TITLE_TEXT_INACTIVE);
}

fn mib(sectors: u64) -> u64 {
    sectors / 2048
}

/// Paint the current screen if the screens are up; false = not up, the dialog paints as usual.
pub(super) fn paint(px: &mut [u32]) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let mut g = M.lock();
    let Some(m) = g.as_mut() else { return false };
    m.painted |= m.screen.bit();
    let dim = theme::TITLE_TEXT_INACTIVE;
    let ink = theme::CONTENT_TEXT;
    match m.screen {
        Screen::Census => {
            title(px, "Install UnaOS on the SSD - what is on each disk");
            let mut y = 54;
            if m.disks.is_empty() {
                line(px, y, "No disk is published yet. Esc back.", ink);
            }
            for (i, d) in m.disks.iter().enumerate() {
                if y + 2 * STEP > H - 60 {
                    break;
                }
                let sel = i == m.sel && d.port.is_some();
                fill(px, LX, y - 3, W - 2 * LX, 2 * STEP + 2, if sel { theme::SCROLL_THUMB } else { theme::CONTENT_FILL });
                if sel {
                    rect(px, LX, y - 3, W - 2 * LX, 2 * STEP + 2, theme::ACCENT);
                }
                let fg = if d.port.is_some() { ink } else { dim };
                line(px, y, &alloc::format!("{:<9} {} MiB  {}  {}", d.name, mib(d.sectors), d.model, d.gpt), fg);
                line(px, y + STEP, &alloc::format!("          {}", d.words), fg);
                y += 2 * STEP + 6;
                if sel && !d.parts.is_empty() {
                    let ps = d.parts.join("  ");
                    line(px, y, &alloc::format!("          {}", ps), dim);
                    y += STEP + 2;
                }
            }
            hint(px, "w/s pick   Enter layout   Esc back   q halt");
        }
        Screen::Layout => {
            title(px, "Layout - ESP + UnaFS");
            let (name, words) = m.disk().map_or((String::new(), String::new()), |d| (d.name.clone(), d.words.clone()));
            line(px, 54, &alloc::format!("{}: {}", name, words), ink);
            let p2 = m.p2();
            line(px, 54 + STEP, &alloc::format!("ESP    {} MiB            (e: 256 / 512 / 1024)", ESP_MIB[m.esp_ix]), ink);
            let vol = match m.mirror {
                Some(n) => alloc::format!("the running volume mirrored, {} MiB", mib(n)),
                None => alloc::format!("a fresh volume, {} MiB", mib(selfinstall::p2_min_sectors(None)) ),
            };
            line(px, 54 + 2 * STEP, &alloc::format!("UnaFS  {}   (w/s: {})", p2.map_or(String::from("the rest"), |n| alloc::format!("{} MiB", mib(n))), UFS_STEPS[m.ufs_ix]), ink);
            line(px, 54 + 3 * STEP, &alloc::format!("       last 64 sectors = scratch; volume: {}", vol), dim);
            let mut y = 54 + 4 * STEP + 6;
            match m.plan() {
                Ok(p) => {
                    for l in p.lines() {
                        line(px, y, &l, ink);
                        y += STEP;
                    }
                }
                Err(e) => line(px, y, &alloc::format!("plan: none ({})", e), ink),
            }
            if let Some(n) = m.note {
                line(px, H - 64, n, ink);
            }
            hint(px, "e ESP   w/s UnaFS   Enter confirm   Esc back");
        }
        Screen::Confirm => {
            title(px, if m.dry { "Confirm (dry run - nothing is written)" } else { "Confirm - this erases the disk" });
            fill(px, LX, 54, W - 2 * LX, 7 * STEP, theme::BUTTON_FACE_PRESSED);
            rect(px, LX + 2, 56, W - 2 * LX - 4, 7 * STEP - 4, theme::ACCENT);
            if let Some(d) = m.disk() {
                line(px, 62, &alloc::format!("  {}  {}  {} MiB", d.name, d.model, mib(d.sectors)), ink);
                line(px, 62 + STEP, &alloc::format!("  {}", d.words), ink);
                if d.kind.refused() {
                    line(px, 62 + 2 * STEP, "  UnaOS will not erase this disk.", ink);
                    line(px, 62 + 3 * STEP, "  Enter asks the guard again; it refuses.", dim);
                    hint(px, "Enter  Esc back");
                } else if d.kind.typed() {
                    line(px, 62 + 2 * STEP, "  EVERYTHING ON IT IS LOST. To erase it, type", ink);
                    line(px, 62 + 3 * STEP, &alloc::format!("  the disk's name:  {}", d.confirm_name()), ink);
                    fill(px, LX + 12, 62 + 4 * STEP + 2, W - 2 * LX - 24, CELL_H + 6, theme::CONTENT_FILL);
                    rect(px, LX + 12, 62 + 4 * STEP + 2, W - 2 * LX - 24, CELL_H + 6, theme::ACCENT);
                    let shown = alloc::format!("{}_", m.typed);
                    text(px, LX + 18, 62 + 4 * STEP + 5, shown.as_bytes(), ink);
                    hint(px, "type the name   Enter erase   Esc back");
                } else {
                    line(px, 62 + 2 * STEP, "  One press lays ESP + UnaFS on it.", ink);
                    hint(px, "Enter install   Esc back   q halt");
                    button(px, W - 190, H - 92, 160, b"Install", true);
                }
            }
            if let Some(n) = m.note {
                line(px, 62 + 6 * STEP, n, ink);
            }
        }
        Screen::Progress | Screen::Result => {
            let done = m.screen == Screen::Result;
            match (&m.outcome, done) {
                (Some((_, true, _)), true) if m.dry => title(px, "Dry run passed - nothing was written"),
                (Some((_, true, _)), true) => title(px, "Installed - the SSD carries UnaOS"),
                (Some(_), true) => title(px, "Not installed"),
                _ => title(px, "Installing..."),
            }
            let mut y = 54;
            for s in ["probe", "guard", "grant", "snapshot", "gpt", "esp", "unafs", "fsck", "grow", "done"] {
                let got = m.stages.iter().rev().find(|(n, _)| n == s);
                let (mark, what) = match got {
                    Some((_, w)) if w.starts_with("ok") || w.starts_with("issued") || w.starts_with("PASS") || w.ends_with(" ok") => ("[x]", w.as_str()),
                    Some((_, w)) if w.starts_with("dry") => ("[-]", w.as_str()),
                    Some((_, w)) => ("[!]", w.as_str()),
                    None => ("[ ]", ""),
                };
                line(px, y, &alloc::format!("{} {:<9} {}", mark, s, what), if got.is_some() { ink } else { dim });
                y += STEP;
            }
            if done {
                if let Some((grant, _, why)) = &m.outcome {
                    line(px, y + 4, &alloc::format!("grant {} - {}", if *grant { "issued" } else { "refused" }, why), ink);
                }
                if !m.last_out.is_empty() {
                    line(px, y + 4 + STEP, &m.last_out, dim);
                }
                m.offered_reboot = true;
                hint(px, "r reboot   Esc/Enter back to the chooser");
                button(px, W - 190, H - 92, 160, b"Reboot", true);
            }
        }
    }
    true
}

// -------------------------------------------------------------- the fixture --

/// `tests instgui` — the five screens, synthetic keys, the DRY-RUN path:
/// `:: INSTALL3: screens=census,layout,confirm,progress,result grant=<issued|refused> dry_run=1 -> PASS ::`
pub fn selftest() {
    let was = ACTIVE.load(Ordering::Acquire);
    if was {
        serial_println!(":: INSTALL3: screens=none grant=none dry_run=1 -> SKIP (the operator has the SSD screens open) ::");
        return;
    }
    TEST.store(true, Ordering::Release);
    let st = super::State::Choose;
    // `i` on the chooser opens the census (the test opens it DRY directly: same entry, dry set).
    enter(true);
    super::repaint();
    let feed = |c: u8| {
        let _ = key(st, c);
    };
    let snap = || M.lock().as_ref().map(|m| (m.screen, m.disk().map(|d| (d.kind, d.port, String::from(d.confirm_name()), d.name.clone()))));
    let census_ok = matches!(snap(), Some((Screen::Census, Some((_, Some(_), _, _)))));
    // CENSUS -> LAYOUT
    feed(b'\r');
    let layout_ok = matches!(snap(), Some((Screen::Layout, _))) && M.lock().as_ref().is_some_and(|m| m.plan().is_ok());
    // every layout key, then back to the defaults: e cycles the ESP sizes, s/w step the UnaFS size
    for c in [b'e', b'e', b'e', b's', b's', b'w', b'w'] {
        feed(c);
    }
    let defaults = M.lock().as_ref().is_some_and(|m| m.esp_ix == ESP_DEFAULT.load(Ordering::Relaxed) as usize && m.ufs_ix == 0);
    // LAYOUT -> CONFIRM
    feed(b'\r');
    let (kind, name, disk) = match snap() {
        Some((Screen::Confirm, Some((k, _, n, d)))) => (Some(k), n, d),
        _ => (None, String::new(), String::new()),
    };
    let mut negative_ok = true;
    if kind.is_some_and(|k| k.typed()) {
        // the negative leg: a wrong name issues nothing and stays on CONFIRM
        for &c in b"not-this-disk" {
            feed(c);
        }
        feed(b'\r');
        negative_ok = matches!(snap(), Some((Screen::Confirm, _))) && M.lock().as_ref().is_some_and(|m| m.mismatch_seen && m.outcome.is_none());
        for _ in 0..13 {
            feed(0x7f);
        }
        for c in name.bytes() {
            feed(c.to_ascii_lowercase());
        }
    }
    // CONFIRM -> PROGRESS -> RESULT (the judgment, DRY)
    feed(b'\r');
    let (result_ok, grant, pass, reason, stages_dry, offered) = match M.lock().as_ref() {
        Some(m) => (
            m.screen == Screen::Result,
            m.outcome.as_ref().is_some_and(|o| o.0),
            m.outcome.as_ref().is_some_and(|o| o.1),
            m.outcome.as_ref().map_or(String::new(), |o| o.2.clone()),
            // nothing past the grant ran: every later stage that was reported is `dry`
            m.stages.iter().filter(|(n, _)| matches!(n.as_str(), "snapshot" | "gpt" | "esp" | "unafs" | "fsck" | "grow")).all(|(_, w)| w.starts_with("dry") || w.starts_with("fail (plan")),
            m.offered_reboot,
        ),
        None => (false, false, false, String::new(), false, false),
    };
    // `r` on the result screen is refused under the fixture; Esc gives the chooser back.
    feed(b'r');
    let (visited, painted) = M.lock().as_ref().map_or((0, 0), |m| (m.visited, m.painted));
    feed(0x1b);
    let left = !ACTIVE.load(Ordering::Acquire);
    TEST.store(false, Ordering::Release);
    super::repaint();
    // The expectation is the verdict's: empty / ours / typed-foreign are issued; boot, live root,
    // unreadable and the synthetic stand-in (no disk on its port) are refused.
    let expect_issued = kind.is_some_and(|k| !k.refused()) && disk != "synthetic";
    let screens: Vec<&str> = ALL.iter().filter(|s| visited & painted & s.bit() != 0).map(|s| s.tag()).collect();
    let all5 = screens.len() == 5;
    let pass_all = census_ok && layout_ok && defaults && negative_ok && result_ok && offered && left && stages_dry && all5 && grant == expect_issued && (!grant || pass);
    serial_println!(
        "[install3] disk={} verdict={} confirmation={} negative={} layout_defaults={} stages_dry={} reboot_offered={} outcome={} ({})",
        disk,
        kind.map_or("none", |k| k.tag()),
        if kind.is_some_and(|k| k.typed()) { "typed-name" } else { "one-press" },
        if kind.is_some_and(|k| k.typed()) { if negative_ok { "refused" } else { "ADMITTED" } } else { "n/a" },
        defaults as u8,
        stages_dry as u8,
        offered as u8,
        if pass { "pass" } else { "no" },
        reason
    );
    serial_println!(
        ":: INSTALL3: screens={} grant={} dry_run=1 -> {} ::",
        if screens.is_empty() { alloc::string::String::from("none") } else { screens.join(",") },
        if grant { "issued" } else { "refused" },
        if pass_all { "PASS" } else { "FAIL" }
    );
}
