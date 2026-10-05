// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// KERNELFONT2 (rmbp-ledger B363, R85 item 10): the Lumen window's text, through `font_core::ui` — the ONE engine
// the kernel's `video::text` fulfils for the desktop (KERNELFONT, B359), linked here as a library (the R82 shape).
// The faces are DATA on the volume: DejaVu Sans / Sans Bold / Sans Mono, read at start over SYS_PATH_READ from
// `/system/fonts/` (FAT card or UnaFS root) or `/boot/system/fonts/` (UnaFS SSD root, the card at /boot) into the
// heap the raised ELF window holds (WINDOW2, B361: 64 MiB). The chat is DejaVu Sans, code DejaVu Sans Mono, bold
// the bold face. No face (no file surface, no file, a parse refusal) = the font8x8 grid this window had, and the
// start line says why (`font=font8x8 font_why=<w>`).
//
// The transcript is printable ASCII (App::add), so every advance is a per-byte table lookup built once from the
// engine, and every glyph is drawn unshaped (`draw_char_with`) at the summed advances: what wraps is exactly what
// draws, with no shaping on the paint path.

use alloc::vec::Vec;
use font_core::ui::{Engine, Role, Style};
use font_core::Font;
use vein_ring3::files;

/// Where the builder stages the faces (KERNELFONT's `video::text::DIR` / `DIR_CARD`).
const DIRS: [&[u8]; 2] = [b"/system/fonts/", b"/boot/system/fonts/"];
/// (file, role, bold). The first is REQUIRED.
const FACES: [(&[u8], Role, bool); 3] = [(b"DejaVuSans.ttf", Role::Sans, false), (b"DejaVuSans-Bold.ttf", Role::Sans, true), (b"DejaVuSansMono.ttf", Role::Mono, false)];
const NAMES: [&str; 3] = ["dejavu-sans", "dejavu-sans-bold", "dejavu-mono"];
/// A face larger than this is refused (DejaVu Sans is 760 KB).
const FACE_MAX: usize = 4 * 1024 * 1024;
/// The glyph cache bound (bytes): two sizes x three faces of the ASCII set fit many times over.
const CACHE: usize = 512 * 1024;
/// The line box with a face (px); the font8x8 grid keeps its 10.
pub const LH_TT: i32 = 15;
pub const LH_BITMAP: i32 = 10;

/// What a byte is drawn with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sans,
    Bold,
    Mono,
}

pub struct Txt {
    eng: Engine<'static>,
    sans: Style,
    mono: Style,
    /// Baseline row inside the [`LH_TT`] line box.
    base: f32,
    /// Advances of 0x20..=0x7e for [`Kind`] Sans / Bold / Mono, px.
    adv: [[f32; 95]; 3],
    pub bytes: usize,
}

/// Why no face is drawing (the start line's `font_why=`).
pub enum Why {
    NoSurface(i64),
    Absent(i64),
    Size,
    Heap,
    Parse,
}

impl Why {
    pub fn put(&self, l: &mut dyn FnMut(&[u8], Option<i64>)) {
        match self {
            Why::NoSurface(e) => l(b"no-file-surface", Some(*e)),
            Why::Absent(e) => l(b"absent", Some(*e)),
            Why::Size => l(b"size", None),
            Why::Heap => l(b"heap", None),
            Why::Parse => l(b"parse", None),
        }
    }
}

/// Read one file whole over SYS_PATH_READ.
fn read(path: &[u8]) -> Result<Vec<u8>, Why> {
    let mut v: Vec<u8> = Vec::new();
    loop {
        let n = v.len();
        if n >= FACE_MAX {
            return Err(Why::Size);
        }
        let step = files::next::PATH_IO_MAX;
        if v.try_reserve(step).is_err() {
            return Err(Why::Heap);
        }
        v.resize(n + step, 0);
        let r = files::path_read(path, n as u64, &mut v[n..]);
        if r < 0 {
            return Err(if n == 0 && r == -38 { Why::NoSurface(r) } else { Why::Absent(r) });
        }
        v.truncate(n + r as usize);
        if (r as usize) < step {
            break;
        }
    }
    if v.is_empty() {
        return Err(Why::Absent(0));
    }
    Ok(v)
}

/// Load the faces. `Err` = the font8x8 grid.
pub fn load() -> Result<Txt, Why> {
    let mut eng = Engine::new(CACHE);
    let mut bytes = 0usize;
    let mut dir: Option<&[u8]> = None;
    let mut first_err = Why::Absent(0);
    for (k, &(file, role, bold)) in FACES.iter().enumerate() {
        let mut got = None;
        for d in DIRS.iter().filter(|d| dir.map_or(true, |x| x == **d)) {
            let mut p = [0u8; 64];
            let n = d.len() + file.len();
            p[..d.len()].copy_from_slice(d);
            p[d.len()..n].copy_from_slice(file);
            match read(&p[..n]) {
                Ok(v) => {
                    got = Some(v);
                    dir = Some(d);
                    break;
                }
                Err(e) => {
                    if k == 0 {
                        first_err = e;
                    }
                }
            }
        }
        let Some(v) = got else {
            if k == 0 {
                return Err(first_err);
            }
            continue; // bold / mono absent: the sans face draws them
        };
        let len = v.len();
        let data: &'static [u8] = alloc::boxed::Box::leak(v.into_boxed_slice());
        match Font::parse(data) {
            Ok(f) => {
                eng.add_face(NAMES[k], role, bold, f);
                bytes += len;
            }
            Err(_) if k == 0 => return Err(Why::Parse),
            Err(_) => {}
        }
    }
    let size = eng.fit_size(Role::Sans, None, LH_TT as f32).unwrap_or(12.0);
    let mrole = if eng.has(Role::Mono) { Role::Mono } else { Role::Sans };
    let msize = eng.fit_size(mrole, None, LH_TT as f32).unwrap_or(12.0);
    let sans = Style { role: Role::Sans, bold: false, size };
    let mono = Style { role: mrole, bold: false, size: msize };
    let base = eng.baseline_in_cell(Role::Sans, size, LH_TT as f32);
    let mut t = Txt { eng, sans, mono, base, adv: [[0.0; 95]; 3], bytes };
    for (k, st) in [sans, Style { bold: true, ..sans }, mono].into_iter().enumerate() {
        for c in 0x20u8..=0x7e {
            t.adv[k][(c - 0x20) as usize] = t.eng.measure(&[c], st);
        }
    }
    Ok(t)
}

impl Txt {
    pub fn name(&self) -> &'static str {
        NAMES[0]
    }
    #[inline]
    pub fn adv(&self, c: u8, k: Kind) -> f32 {
        let i = if (0x20..=0x7e).contains(&c) { (c - 0x20) as usize } else { (b'?' - 0x20) as usize };
        self.adv[k as usize][i]
    }
    /// Draw `s` with its line box's top at `y`, pen at `x`; each covered pixel goes to `put(x, y, coverage)`.
    /// Italic shears the upper half of the glyph one pixel right. Returns the pen after the last byte.
    pub fn draw(&mut self, x: f32, y: i32, s: &[u8], k: Kind, italic: bool, ink: u32, put: &mut dyn FnMut(i32, i32, u8)) -> f32 {
        let st = match k {
            Kind::Sans => self.sans,
            Kind::Bold => Style { bold: true, ..self.sans },
            Kind::Mono => self.mono,
        };
        let bl = y as f32 + self.base;
        let shear_above = (bl - st.size * 0.35) as i32;
        let mut pen = x;
        for &c in s {
            let ch = if (0x20..=0x7e).contains(&c) { c as char } else { '?' };
            let a = self.adv(c, k);
            if ch != ' ' {
                self.eng.draw_char_with(ch, st, pen, bl, ink & 0x00FF_FFFF, &mut |gx, gy, cov| {
                    let sh = if italic && gy < shear_above { 1 } else { 0 };
                    put(gx + sh, gy, cov)
                });
            }
            pen += a;
        }
        pen
    }
}
