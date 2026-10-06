// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRY3 (rmbp-ledger B413, MACPARITY row 27) — QUICK LOOK. Space on Quarry's selection opens a floating panel
//! that shows the file WITHOUT opening an app; Space or Esc closes it; the arrows move the selection and the
//! panel follows. Quarry is the Finder by ruling (R50).
//!
//! NO SECOND RENDERER. The panel is drawn by the viewer the file's type opens with (`fs::assoc::opener_for` +
//! `openers::effective`, the one dispatch's decision):
//!
//! | opener | the panel's body |
//! | :--- | :--- |
//! | `facet` | `video::facet::decode_file` — Facet's decoder (PNG, and its foreign formats) scaled into the panel |
//! | `fileview` `textedit` | `video::fileview::quicklook_body` — the viewer's read, sanitise and painter |
//! | `markdown` `json` | the same, with the viewer's renderer (`richtext`) |
//! | `play` | the player's own codec sniff (`audio_core::sniff`): a card naming the format; Return plays it |
//! | anything else | a card: the icon (APPRES for a program), the name, the type, the size |
//!
//! THE PANEL is DIALOG's family's chromeless overlay row (`wm::overlay_open`, the toast's and the shortcuts
//! overlay's shape): it never takes focus, so the keyboard stays with Quarry and the arrows keep moving its
//! selection. aarch64 has no overlay row yet: there it is a titled native row (owner [`OWNER`]).
//!
//! The decode is I/O, so the key router only LATCHES a request; the panel renders on Quarry's service pass.
//! Wire, per preview: `[quarry] quicklook path=<p> type=<mime> via=<viewer> ms=<n>`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{theme, toolbar, wm, NodeKind, Pane, MODEL};

/// The panel's owner on aarch64 (a titled row there). In the kernel band, distinct from Quarry's.
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x47;
const _: () = assert!(OWNER != super::OWNER);

enum Req {
    Show(String),
    Close,
}

struct Panel {
    win: wm::WinId,
    surf: Vec<u32>,
    w: usize,
    h: usize,
    /// Panel position (outer), for the press guard.
    x: usize,
    y: usize,
    path: String,
}

static REQ: spin::Mutex<Option<Req>> = spin::Mutex::new(None);
static PANEL: spin::Mutex<Option<Panel>> = spin::Mutex::new(None);
static OPEN: AtomicBool = AtomicBool::new(false);
static SHOWN: AtomicU32 = AtomicU32::new(0);

pub(super) fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

fn request(r: Req) {
    *REQ.lock() = Some(r);
}

/// The selected list row's path (list focus only), or `None`.
fn selected() -> Option<String> {
    let g = MODEL.lock();
    let m = g.as_ref()?;
    if m.focus != Pane::List {
        return None;
    }
    let e = m.list.get(m.list_sel)?;
    Some(super::join(&m.cwd, &e.name))
}

/// Quarry's keys, asked first (after the rename field and the search field). `true` = consumed.
pub(super) fn key(c: u8) -> bool {
    let open = is_open();
    match c {
        b' ' => {
            if open {
                request(Req::Close);
                return true;
            }
            match selected() {
                Some(p) => {
                    request(Req::Show(p));
                    true
                }
                None => false,
            }
        }
        0x1b if open => {
            request(Req::Close);
            true
        }
        0x1C..=0x1F if open => {
            let icons = toolbar::view() == toolbar::View::Icons;
            if icons {
                if !super::iconview::key(c) {
                    return true;
                }
            } else {
                if c == 0x1C || c == 0x1D {
                    return true; // left/right do not leave the list while the panel shows it
                }
                let mut g = MODEL.lock();
                let Some(m) = g.as_mut() else { return true };
                if c == 0x1F {
                    m.list_sel = m.list_sel.saturating_sub(1);
                } else if m.list_sel + 1 < m.list.len() {
                    m.list_sel += 1;
                }
                m.settle();
            }
            if let Some(p) = selected() {
                request(Req::Show(p));
            }
            true
        }
        _ => false,
    }
}

/// A press on the panel: swallowed (it is a preview, not a target), and the keyboard stays with Quarry.
pub fn press_route(x: i32, y: i32) -> bool {
    if !is_open() || x < 0 || y < 0 {
        return false;
    }
    let (x, y) = (x as usize, y as usize);
    let inside = PANEL.try_lock().and_then(|p| p.as_ref().map(|p| x >= p.x && y >= p.y && x < p.x + p.w && y < p.y + p.h)).unwrap_or(false);
    if inside {
        wm::focus_changed(super::OWNER);
    }
    inside
}

/// The panel's size for a `pw x ph` screen.
fn panel_size(pw: usize, ph: usize) -> (usize, usize) {
    ((pw * 9 / 20).max(crate::ui::px(320)).min(pw), (ph * 11 / 20).max(crate::ui::px(240)).min(ph))
}

fn fill(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, w: usize, hh: usize, c: u32) {
    for r in y..(y + hh).min(h) {
        for col in x..(x + w).min(stride) {
            px[r * stride + col] = c;
        }
    }
}

fn face() -> crate::video::text::Face {
    crate::video::metrics::native_face(crate::video::text::Face::Chrome)
}

fn line(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, s: &[u8], fg: u32, bold: bool) {
    crate::video::text::draw_text(px, stride, stride.saturating_sub(crate::ui::px(8)), h, x, y, s, fg, bold, face());
}

fn size_text(n: u64) -> String {
    if n >= 10 << 20 {
        alloc::format!("{} MB ({} bytes)", n >> 20, n)
    } else if n >= 10 << 10 {
        alloc::format!("{} KB ({} bytes)", n >> 10, n)
    } else {
        alloc::format!("{} bytes", n)
    }
}

/// A card: the icon, the name, then `facts`, one per line.
fn card(px: &mut [u32], w: usize, h: usize, top: usize, path: &str, dir: bool, facts: &[String]) {
    let s = crate::ui::px(128).min(h.saturating_sub(top) / 2);
    let x = crate::ui::px(24);
    let y = top + crate::ui::px(24);
    super::iconview::draw_icon(px, w, h, x, y, s, path, dir, face());
    let tx = x + s + crate::ui::px(24);
    let lh = face().cell_h() + crate::ui::px(4);
    let leaf = path.rsplit('/').next().unwrap_or(path);
    line(px, w, h, tx, y, leaf.as_bytes(), theme::content_text(), true);
    for (i, f) in facts.iter().enumerate() {
        line(px, w, h, tx, y + (i + 1) * lh + crate::ui::px(4), f.as_bytes(), theme::title_text_inactive(), false);
    }
}

/// Copy a `bw x bh` body into the panel at `(x, y)`.
fn blit(px: &mut [u32], w: usize, h: usize, src: &[u32], bw: usize, bh: usize, x: usize, y: usize) {
    for r in 0..bh {
        if y + r >= h {
            break;
        }
        let n = bw.min(w.saturating_sub(x));
        px[(y + r) * w + x..(y + r) * w + x + n].copy_from_slice(&src[r * bw..r * bw + n]);
    }
}

/// **Render `path` into a `w x h` panel surface** with the viewer its type opens with. Returns
/// `(via, mime)`; `via` is the viewer that drew the body (`card` = no viewer for this type in this build, or the
/// viewer refused the file — the reason is on the card and the wire). Prints the per-preview wire line.
pub(super) fn render_into(path: &str, px: &mut [u32], w: usize, h: usize) -> (&'static str, String) {
    let t0 = crate::arch::ms();
    for p in px.iter_mut() {
        *p = theme::content_fill();
    }
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).ok();
    let dir = st.as_ref().map(|s| matches!(s.kind, NodeKind::Dir)).unwrap_or(false);
    let size = st.as_ref().map(|s| s.size).unwrap_or(0);
    let mime = if dir { String::from("inode/directory") } else { crate::fs::filetype::type_of(path).0 };
    // The title strip: the name and the type.
    let pad = crate::ui::px(8);
    let strip = face().cell_h() + 2 * pad;
    fill(px, w, h, 0, 0, w, strip, theme::chrome_face());
    fill(px, w, h, 0, strip - 1, w, 1, theme::frame_line());
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let head = alloc::format!("{}   {}", leaf, mime);
    line(px, w, h, pad, pad, head.as_bytes(), theme::title_text_active(), true);
    let (bx, by) = (pad, strip + pad);
    let (bw, bh) = (w.saturating_sub(2 * pad), h.saturating_sub(strip + 2 * pad));
    let opener = if dir { String::from("none") } else { crate::fs::assoc::opener_for(path, &mime).0 };
    let eff = super::openers::effective(&opener, path);
    let mut why: Option<String> = None;
    let via: &'static str = match eff.as_str() {
        #[cfg(feature = "facet")]
        "facet" => match crate::video::facet::decode_file(path, u64::MAX, bw, bh) {
            Ok((img, ow, oh, sw, sh)) => {
                blit(px, w, h, &img, ow, oh, bx + bw.saturating_sub(ow) / 2, by + bh.saturating_sub(oh) / 2);
                let _ = (sw, sh);
                "facet"
            }
            Err(e) => {
                why = Some(alloc::format!("Facet: {}", e.reason()));
                "card"
            }
        },
        "fileview" | "textedit" | "markdown" | "json" => {
            let kind = if eff == "markdown" || eff == "json" { eff.as_str() } else { "" };
            match crate::video::fileview::quicklook_body(path, kind, bw, bh) {
                Ok((body, _rows)) => {
                    blit(px, w, h, &body, bw, bh, bx, by);
                    match eff.as_str() {
                        "markdown" => "markdown",
                        "json" => "json",
                        _ => "fileview",
                    }
                }
                Err(e) => {
                    why = Some(alloc::format!("viewer: {}", e));
                    "card"
                }
            }
        }
        "play" => {
            let head = mt.read(path, 0, 64).unwrap_or_default();
            let fmt = audio_core::sniff(&head).name();
            card(px, w, h, strip, path, false, &[alloc::format!("Audio: {}", fmt), size_text(size), String::from("Return plays it")]);
            "play"
        }
        _ => "card",
    };
    if via == "card" {
        let mut facts: Vec<String> = Vec::new();
        if dir {
            let n = super::collect(path).map(|(_, r)| r.len()).unwrap_or(0);
            facts.push(alloc::format!("Folder: {} items", n));
        } else {
            facts.push(alloc::format!("Kind: {}", mime));
            facts.push(size_text(size));
            facts.push(match &why {
                Some(w) => w.clone(),
                None => alloc::format!("No viewer for this type in this build (opener {})", opener),
            });
        }
        card(px, w, h, strip, path, dir, &facts);
    }
    serial_println!(
        "[quarry] quicklook path={} type={} via={} ms={}{}",
        path,
        mime,
        via,
        crate::arch::ms().saturating_sub(t0),
        why.map(|w| alloc::format!(" why={}", w.replace(' ', "_"))).unwrap_or_default()
    );
    (via, mime)
}

fn close(by: &str) {
    let p = PANEL.lock().take();
    OPEN.store(false, Ordering::Relaxed);
    if let Some(p) = p {
        if p.win != wm::WIN_NONE {
            wm::close(p.win);
        }
        serial_println!("[quarry3] quicklook closed by={} path={} shown={}", by, p.path, SHOWN.load(Ordering::Relaxed));
        drop(p.surf); // after the row stopped naming it
    }
}

fn show(path: &str) {
    let mut g = PANEL.lock();
    if let Some(p) = g.as_mut() {
        if p.path == path {
            return;
        }
        let (w, h) = (p.w, p.h);
        render_into(path, &mut p.surf, w, h);
        p.path = String::from(path);
        let win = p.win;
        drop(g);
        SHOWN.fetch_add(1, Ordering::Relaxed);
        let _ = wm::present(win);
        return;
    }
    let Some(pi) = crate::video::panel_info_nonblocking() else { return };
    let (pw, ph) = (pi.width, pi.height);
    let (w, h) = panel_size(pw, ph);
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(w * h).is_err() {
        serial_println!("[quarry3] quicklook DECLINE reason=oom bytes={}", w * h * 4);
        return;
    }
    surf.resize(w * h, theme::content_fill());
    render_into(path, &mut surf, w, h);
    let (x, y) = (pw.saturating_sub(w) / 2, ph.saturating_sub(h) / 3);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    let win = wm::overlay_open(surf.as_ptr() as usize, w * h * 4, w, h, x, y);
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    let win = wm::create_at_native(OWNER, surf.as_ptr() as usize, w * h * 4, w as u32, h as u32, (w * 4) as u32, b"Quick Look", x + wm::BORDER(), y + wm::TITLE_H() + wm::BORDER());
    if win == wm::WIN_NONE {
        serial_println!("[quarry3] quicklook DECLINE reason=no-row");
        return;
    }
    // The Vec's heap buffer does not move when the Vec itself moves into the state.
    *g = Some(Panel { win, surf, w, h, x, y, path: String::from(path) });
    drop(g);
    OPEN.store(true, Ordering::Relaxed);
    SHOWN.fetch_add(1, Ordering::Relaxed);
    let _ = wm::present(win);
    wm::focus_changed(super::OWNER);
    serial_println!("[quarry3] quicklook open win={} panel={}x{} at=({},{})", win, w, h, x, y);
}

/// Quarry's service pass: drain the latched request (the render is I/O, never in the router), and close the
/// panel when Quarry has gone.
pub(super) fn service() {
    if is_open() && !super::is_open() {
        close("quarry-closed");
        return;
    }
    let r = REQ.try_lock().and_then(|mut g| g.take());
    // The panel FOLLOWS the selection however it moved (an arrow, a press, a wheel, a search keystroke).
    let r = match r {
        Some(r) => r,
        None if is_open() => {
            let sel = MODEL.try_lock().and_then(|g| g.as_ref().and_then(|m| m.list.get(m.list_sel).map(|e| super::join(&m.cwd, &e.name))));
            let cur = PANEL.try_lock().and_then(|p| p.as_ref().map(|p| p.path.clone()));
            match (sel, cur) {
                (Some(s), Some(c)) if s != c => Req::Show(s),
                _ => return,
            }
        }
        None => return,
    };
    match r {
        Req::Close => close("key"),
        Req::Show(p) => show(&p),
    }
}

/// The fixture: preview every sample in `dir` named in `names` into an off-glass surface. Returns
/// `(by a viewer, routed to a viewer this build carries, owed names)`.
pub(super) fn preview_set(dir: &str, names: &[&str]) -> (usize, usize, Vec<String>) {
    let (w, h) = (crate::ui::px(560), crate::ui::px(420));
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(w * h).is_err() {
        return (0, names.len(), Vec::new());
    }
    surf.resize(w * h, 0);
    let (mut viewer, mut routed) = (0usize, 0usize);
    let mut owed: Vec<String> = Vec::new();
    for n in names.iter() {
        let p = alloc::format!("{}/{}", dir, n);
        let mime = crate::fs::filetype::type_of(&p).0;
        let eff = super::openers::effective(&crate::fs::assoc::opener_for(&p, &mime).0, &p);
        let has_route = matches!(eff.as_str(), "facet" | "fileview" | "textedit" | "markdown" | "json" | "play");
        if has_route {
            routed += 1;
        }
        let (via, _) = render_into(&p, &mut surf, w, h);
        if via != "card" {
            viewer += 1;
        } else {
            owed.push(String::from(*n));
        }
    }
    (viewer, routed, owed)
}
