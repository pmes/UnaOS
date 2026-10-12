// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MIDDENTWIN (LEDGER SR75): the shell window's text through `font_core::ui` — LUMEN.ELF's txt.rs (KERNELFONT2,
// rmbp-ledger B363) cut to what a terminal draws: ONE monospace face, DejaVu Sans Mono, read at start over
// SYS_PATH_READ from `/system/fonts/` (FAT card or UnaFS root) or `/boot/system/fonts/` (UnaFS SSD root, the
// card at /boot) into the heap the ELF window holds. No face (no file surface, no file, a parse refusal) = the
// font8x8 grid, and the start line says why (`font=font8x8 font_why=<w>`).
//
// The scrollback is printable ASCII, so every cell is one advance wide and every glyph is drawn unshaped
// (`draw_char_with`): what wraps is exactly what draws.

use alloc::vec::Vec;
use font_core::ui::{Engine, Role, Style};
use font_core::Font;
use vein_ring3::files;

const DIRS: [&[u8]; 2] = [b"/system/fonts/", b"/boot/system/fonts/"];
/// The terminal face, then the proportional face as a last resort (drawn on the same fixed cell).
const FACES: [(&[u8], Role); 2] = [(b"DejaVuSansMono.ttf", Role::Mono), (b"DejaVuSans.ttf", Role::Sans)];
const NAMES: [&str; 2] = ["dejavu-mono", "dejavu-sans"];
const FACE_MAX: usize = 4 * 1024 * 1024;
const CACHE: usize = 256 * 1024;
/// The line box with a face (px); the font8x8 grid's is 10.
pub const LH_TT: i32 = 14;
pub const LH_BITMAP: i32 = 10;

pub struct Txt {
    eng: Engine<'static>,
    st: Style,
    base: f32,
    /// The cell advance (px), the face's `M`.
    pub cell: f32,
    pub name: &'static str,
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
    pub fn word(&self) -> (&'static [u8], Option<i64>) {
        match self {
            Why::NoSurface(e) => (b"no-file-surface", Some(*e)),
            Why::Absent(e) => (b"absent", Some(*e)),
            Why::Size => (b"size", None),
            Why::Heap => (b"heap", None),
            Why::Parse => (b"parse", None),
        }
    }
}

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
            return Err(if n == 0 && r == una_abi::ENOSYS { Why::NoSurface(r) } else { Why::Absent(r) });
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

/// Load the face. `Err` = the font8x8 grid.
pub fn load() -> Result<Txt, Why> {
    let mut first_err = Why::Absent(0);
    for (k, &(file, role)) in FACES.iter().enumerate() {
        for d in DIRS.iter() {
            let mut p = [0u8; 64];
            let n = d.len() + file.len();
            p[..d.len()].copy_from_slice(d);
            p[d.len()..n].copy_from_slice(file);
            let v = match read(&p[..n]) {
                Ok(v) => v,
                Err(e) => {
                    if k == 0 && d == &DIRS[0] {
                        first_err = e;
                    }
                    continue;
                }
            };
            let len = v.len();
            let data: &'static [u8] = alloc::boxed::Box::leak(v.into_boxed_slice());
            let Ok(f) = Font::parse(data) else {
                first_err = Why::Parse;
                continue;
            };
            let mut eng = Engine::new(CACHE);
            eng.add_face(NAMES[k], role, false, f);
            let size = eng.fit_size(role, None, LH_TT as f32).unwrap_or(12.0);
            let st = Style { role, bold: false, size };
            let base = eng.baseline_in_cell(role, size, LH_TT as f32);
            let cell = eng.measure(b"M", st).max(1.0);
            return Ok(Txt { eng, st, base, cell, name: NAMES[k], bytes: len });
        }
    }
    Err(first_err)
}

impl Txt {
    /// Draw `s` (printable ASCII) on the fixed cell with its line box's top at `y`, pen at `x`; each covered
    /// pixel goes to `put(x, y, coverage)`.
    pub fn draw(&mut self, x: f32, y: i32, s: &[u8], ink: u32, put: &mut dyn FnMut(i32, i32, u8)) {
        let bl = y as f32 + self.base;
        let mut pen = x;
        for &c in s {
            let ch = if (0x21..=0x7e).contains(&c) { c as char } else { ' ' };
            if ch != ' ' {
                self.eng.draw_char_with(ch, self.st, pen, bl, ink & 0x00FF_FFFF, put);
            }
            pen += self.cell;
        }
    }
}
