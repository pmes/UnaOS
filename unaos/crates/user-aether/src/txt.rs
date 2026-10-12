// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66): the page's text through `font_core::ui` — the ONE engine the kernel's `video::text` fulfils
// and LUMEN.ELF draws with (KERNELFONT2, B363) — linked as a library. The faces are DATA on the volume, read at
// start from `/system/fonts/` (or `/boot/system/fonts/`, the card under a UnaFS root), Lumen's two directories:
// DejaVu Sans (required), Sans Bold, Sans Mono, Sans Mono Bold. Unlike Lumen (printable ASCII, a per-byte advance
// table) the page is UTF-8, so every run is SHAPED (`Engine::measure` / `Engine::draw_with`): what the line breaker
// measured is exactly what paints. Italic is a shear of the regular face (no oblique face is staged).
//
// No face (no file surface, no file, a parse refusal) = the font8x8 grid: every glyph 8 px wide on a 10 px line,
// sizes ignored, non-ASCII drawn as '?', and the start line says why (`font=font8x8 font_why=<w>`).

use alloc::vec::Vec;
use font_core::ui::{Engine, Role, Style as FStyle};
use font_core::Font;

use crate::style::Style;

/// Where the builder stages the faces (KERNELFONT's `video::text::DIR` / `DIR_CARD`, Lumen's `txt::DIRS`).
const DIRS: [&[u8]; 2] = [b"/system/fonts/", b"/boot/system/fonts/"];
/// (file, role, bold). The first is REQUIRED.
const FACES: [(&[u8], Role, bool); 4] = [
    (b"DejaVuSans.ttf", Role::Sans, false),
    (b"DejaVuSans-Bold.ttf", Role::Sans, true),
    (b"DejaVuSansMono.ttf", Role::Mono, false),
    (b"DejaVuSansMono-Bold.ttf", Role::Mono, true),
];
const NAMES: [&str; 4] = ["dejavu-sans", "dejavu-sans-bold", "dejavu-mono", "dejavu-mono-bold"];
/// A face larger than this is refused (DejaVu Sans is 760 KB).
const FACE_MAX: usize = 4 * 1024 * 1024;
/// The glyph cache bound (bytes): a page uses a handful of sizes x four faces.
const CACHE: usize = 1024 * 1024;

pub struct Txt {
    eng: Option<Engine<'static>>,
    mono: bool,
    pub bytes: usize,
    pub faces: usize,
}

/// Why no face is drawing (the start line's `font_why=`).
pub enum Why {
    Absent(i64),
    Parse,
}

impl Why {
    pub fn name(&self) -> &'static [u8] {
        match self {
            Why::Absent(_) => b"absent",
            Why::Parse => b"parse",
        }
    }
    pub fn code(&self) -> i64 {
        match self {
            Why::Absent(e) => *e,
            Why::Parse => 0,
        }
    }
}

/// Load the faces. `Err` = the font8x8 grid (the `Txt` it carries draws with it).
pub fn load() -> (Txt, Option<Why>) {
    let mut eng = Engine::new(CACHE);
    let mut bytes = 0usize;
    let mut faces = 0usize;
    let mut dir: Option<&[u8]> = None;
    let mut first_err = Why::Absent(0);
    for (k, &(file, role, bold)) in FACES.iter().enumerate() {
        let mut got: Option<Vec<u8>> = None;
        for d in DIRS.iter().filter(|d| dir.map_or(true, |x| x == **d)) {
            let mut p = [0u8; 64];
            let n = d.len() + file.len();
            p[..d.len()].copy_from_slice(d);
            p[d.len()..n].copy_from_slice(file);
            match crate::sys::read_file(&p[..n], FACE_MAX) {
                Ok((v, _)) if !v.is_empty() => {
                    got = Some(v);
                    dir = Some(d);
                    break;
                }
                Ok(_) => {
                    if k == 0 {
                        first_err = Why::Absent(0);
                    }
                }
                Err(e) => {
                    if k == 0 {
                        first_err = Why::Absent(e);
                    }
                }
            }
        }
        let Some(v) = got else {
            if k == 0 {
                return (Txt { eng: None, mono: false, bytes: 0, faces: 0 }, Some(first_err));
            }
            continue; // bold / mono absent: the engine's fallback stack draws them with what is there
        };
        let len = v.len();
        let data: &'static [u8] = alloc::boxed::Box::leak(v.into_boxed_slice());
        match Font::parse(data) {
            Ok(f) => {
                eng.add_face(NAMES[k], role, bold, f);
                bytes += len;
                faces += 1;
            }
            Err(_) if k == 0 => return (Txt { eng: None, mono: false, bytes: 0, faces: 0 }, Some(Why::Parse)),
            Err(_) => {}
        }
    }
    let mono = eng.has(Role::Mono);
    (Txt { eng: Some(eng), mono, bytes, faces }, None)
}

impl Txt {
    fn fstyle(&self, st: &Style) -> FStyle {
        let role = if st.mono && self.mono { Role::Mono } else { Role::Sans };
        FStyle { role, bold: st.bold, size: st.font_size }
    }

    /// Advance width of `s` in `st`, px.
    pub fn measure(&mut self, s: &str, st: &Style) -> f32 {
        let fs = self.fstyle(st);
        match self.eng.as_mut() {
            Some(e) => e.measure(s.as_bytes(), fs),
            None => 8.0 * s.chars().count() as f32,
        }
    }

    /// (ascent, descent) in px for `st` — browser line metrics of the primary face.
    pub fn metrics(&self, st: &Style) -> (f32, f32) {
        let fs = self.fstyle(st);
        match self.eng.as_ref().and_then(|e| e.line_metrics(fs.role, fs.size)) {
            Some((a, d, _)) => (a, d),
            None => (8.0, 2.0),
        }
    }

    /// The `normal` line height for `st`, px (ascent + descent + line gap, the face's own; 10 on the grid).
    pub fn normal_lh(&self, st: &Style) -> f32 {
        let fs = self.fstyle(st);
        match self.eng.as_ref().and_then(|e| e.line_metrics(fs.role, fs.size)) {
            Some((_, _, lh)) => lh,
            None => 10.0,
        }
    }

    /// Draw `s` with its pen origin at (`x`, `baseline`); covered pixels go to `put(x, y, coverage)`.
    pub fn draw(&mut self, x: f32, baseline: f32, s: &str, st: &Style, put: &mut dyn FnMut(i32, i32, u8)) {
        let fs = self.fstyle(st);
        let italic = st.italic;
        let ink = st.color & 0x00FF_FFFF;
        match self.eng.as_mut() {
            Some(e) => {
                let bl = baseline as i32;
                e.draw_with(s.as_bytes(), fs, x, baseline, None, None, ink, &mut |gx, gy, cov| {
                    let sh = if italic && gy < bl { ((bl - gy) as f32 * 0.22) as i32 } else { 0 };
                    put(gx + sh, gy, cov)
                });
            }
            None => {
                let mut pen = x as i32;
                let top = baseline as i32 - 8;
                for ch in s.chars() {
                    let c = if (' '..='~').contains(&ch) { ch as usize } else { b'?' as usize };
                    let rows = font8x8::legacy::BASIC_LEGACY[c];
                    for (ry, row) in rows.iter().enumerate() {
                        for cx in 0..8 {
                            if row & (1 << cx) != 0 {
                                let sh = if italic { (7 - ry as i32) / 3 } else { 0 };
                                put(pen + cx + sh, top + ry as i32, 255);
                                if st.bold {
                                    put(pen + cx + sh + 1, top + ry as i32, 255);
                                }
                            }
                        }
                    }
                    pen += 8;
                }
            }
        }
    }

    pub fn name(&self) -> &'static [u8] {
        if self.eng.is_some() { b"dejavu-sans" } else { b"font8x8" }
    }
}
