// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The editor's track view: ONE geometry and ONE painter both faces draw (NLE6KEYS, rmbp-ledger B961).
//!
//! CHARTER: Stria — shared-core. The non-linear editor's view (BP-handlers-14): the host face (`stria::nle_face`, which
//! re-exports this module's names) and the on-UnaOS face (the kernel Player's EDL mode, `video/vplay.rs`'s view band)
//! paint the SAME pixels from the SAME [`Layout`]. Moved byte for byte from `stria::nle_face` (NLE2 SR201, NLE2c SR217,
//! NLE3d SR302, NLE3cBLACK SR323, NLE2-SPLIT SR339); its goldens (`nle2_track_paint_golden`, `nle3d_widget_paint_golden`,
//! `nle3cblack_to_black_marker_golden`, …) are this painter's.
//!
//! * [`Layout`]: where everything is on a canvas (the preview band, the ruler, one row per track), read by [`paint_in`],
//!   [`Layout::hit`], [`Layout::crossing`] and the transition widget's [`Layout::transition_rect`]; [`Layout::of_role`]
//!   cuts it for a Surface showing only the preview or only the tracks.
//! * [`paint_in`]: RGBA (or BGRA, [`Order`]) bytes over a plain [`Canvas`]; the optional [`Text`] hook names the clips
//!   and the ruler's seconds (the host's text_host; the kernel's `video::text`, NLE6KEYSLABELS rmbp-ledger B968). No font,
//!   no I/O, no_std + alloc.
//! * The colours are the editor's own table (the host's, moved unchanged); NLE6VIEW-TOKENS (video.md §5) owes them as
//!   palette_core tokens.

use alloc::vec;
use alloc::vec::Vec;

use crate::editor::EditorState;
use crate::{Kind, Timeline, Transition, TransitionKind, WipeDir, edl};

// ---------------------------------------------------------------------------------------------------------------------
// THE ONE GEOMETRY
// ---------------------------------------------------------------------------------------------------------------------

/// The ruler band's height (px).
pub const RULER_H: u32 = 16;
/// One track row's height (px).
pub const ROW_H: u32 = 40;
/// The trim handle's width (px).
pub const HANDLE_W: i64 = 4;

/// Where everything is on a `w` x `h` canvas: the preview band `0..preview_h`, the ruler, then one row per track.
/// The track area shows ticks from `t0` (the page holding the playhead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub w: u32,
    pub h: u32,
    pub preview_h: u32,
    pub ruler_y: u32,
    pub tracks_y: u32,
    pub rows: usize,
    pub zoom: u32,
    pub t0: i64,
    pub message_y: u32, // STRIAINFOREELSTRIP (SR747): the message row's top (0 with no row: [`Layout::with_message`])
    pub message_h: u32, // STRIAINFOREELSTRIP (SR747): the message row's height (0: no row)
}

/// What a point on the canvas is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Ruler(i64),
    /// `(track, clip index, tick)`.
    Clip(usize, usize, i64),
    /// An empty spot on a track: `(track, tick)`.
    Track(usize, i64),
    None,
}

impl Layout {
    pub fn of(w: u32, h: u32, st: &EditorState) -> Layout {
        let rows = st.timeline.tracks.len();
        let below = RULER_H + rows as u32 * ROW_H;
        let preview_h = h.saturating_sub(below);
        let zoom = st.zoom.max(1);
        let per_page = (w / zoom).max(1) as i64;
        let t0 = st.playhead.max(0) / per_page * per_page;
        Layout { w, h, preview_h, ruler_y: preview_h, tracks_y: preview_h + RULER_H, rows, zoom, t0, message_y: 0, message_h: 0 }
    }
    /// The x of tick `t`'s left edge.
    pub fn x_of(&self, t: i64) -> i64 {
        (t - self.t0) * self.zoom as i64
    }
    /// The tick under x.
    pub fn tick_at(&self, x: f64) -> i64 {
        self.t0 + (x.max(0.0) as i64) / self.zoom as i64
    }
    /// The y of track row `row`'s top.
    pub fn row_y(&self, row: usize) -> i64 {
        self.tracks_y as i64 + row as i64 * ROW_H as i64
    }
    /// The centre of tick `t` on track row `row` (what a click on that clip sends).
    pub fn point(&self, row: usize, t: i64) -> (f64, f64) {
        (self.x_of(t) as f64 + self.zoom as f64 / 2.0, self.row_y(row) as f64 + ROW_H as f64 / 2.0)
    }
    pub fn hit(&self, tl: &Timeline, x: f64, y: f64) -> Hit {
        if x < 0.0 || y < 0.0 || x >= self.w as f64 || y >= self.h as f64 || y < self.ruler_y as f64 {
            return Hit::None;
        }
        let t = self.tick_at(x);
        if y < self.tracks_y as f64 {
            return Hit::Ruler(t);
        }
        let row = ((y - self.tracks_y as f64) / ROW_H as f64) as usize;
        if row >= self.rows {
            return Hit::None;
        }
        match tl.clip_at(row, t) {
            Some(i) => Hit::Clip(row, i, t),
            None => Hit::Track(row, t),
        }
    }
    /// The clips whose drawn rect (`x_of(at)..x_of(end)` x its row) meets the rect with corners `(x0, y0)` and
    /// `(x1, y1)` (surface pixels, either order), sorted. (NLE2c SR217's marquee.)
    pub fn crossing(&self, tl: &Timeline, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(usize, usize)> {
        let (xa, xb) = (x0.min(x1), x0.max(x1));
        let (ya, yb) = (y0.min(y1), y0.max(y1));
        let mut out = Vec::new();
        for (row, tr) in tl.tracks.iter().enumerate().take(self.rows) {
            let (r0, r1) = (self.row_y(row) as f64, (self.row_y(row) + ROW_H as i64) as f64);
            if yb < r0 || ya >= r1 {
                continue;
            }
            for (i, c) in tr.clips.iter().enumerate() {
                let (c0, c1) = (self.x_of(c.at) as f64, self.x_of(c.end()) as f64);
                if xb >= c0 && xa < c1 {
                    out.push((row, i));
                }
            }
        }
        out
    }
}

/// Which part of the editor a Surface shows (NLE2-SPLIT SR339).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// All three bands on one Surface (the one-Surface tree).
    Whole,
    /// The preview band only.
    Preview,
    /// The ruler and the tracks only.
    Tracks,
}

impl Layout {
    /// THE geometry for a Surface of `role`: [`Layout::of`] (zoom, page, rows: one computation), with only the band fields
    /// set for the role. `Preview` puts the ruler a band below the canvas (`ruler_y = h + RULER_H`: no tick or label reaches
    /// it, and [`Layout::hit`] finds nothing on it); `Tracks` has no preview band (the ruler at 0, the rows under it).
    pub fn of_role(role: Role, w: u32, h: u32, st: &EditorState) -> Layout {
        let mut l = Layout::of(w, h, st);
        match role {
            Role::Whole => {}
            Role::Preview => {
                l.preview_h = h;
                l.ruler_y = h + RULER_H;
                l.tracks_y = h + 2 * RULER_H;
            }
            Role::Tracks => {
                l.preview_h = 0;
                l.ruler_y = 0;
                l.tracks_y = RULER_H;
            }
        }
        l
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// THE ONE PAINTER
// ---------------------------------------------------------------------------------------------------------------------

/// A preview frame: `width * height * 4` RGBA bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The byte order a canvas is painted in (the host's surfaces read either; the kernel packs RGBA into 0RGB words).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Rgba,
    Bgra,
}

pub const BG: [u8; 4] = [0x1e, 0x1e, 0x24, 255];
pub const FIELD: [u8; 4] = [0x00, 0x00, 0x00, 255];
pub const RULER: [u8; 4] = [0x2a, 0x2a, 0x33, 255];
pub const TICK: [u8; 4] = [0x88, 0x88, 0x99, 255];
pub const ROW: [[u8; 4]; 2] = [[0x26, 0x26, 0x2e, 255], [0x2c, 0x2c, 0x35, 255]];
pub const SELECT: [u8; 4] = [0xff, 0xff, 0xff, 255];
pub const HANDLE: [u8; 4] = [0xff, 0xd0, 0x40, 255];
pub const PLAYHEAD: [u8; 4] = [0xff, 0x40, 0x40, 255];
/// The labels' ink (the host's text_host colour, moved unchanged; the kernel's `video::text` labels too: NLE6KEYSLABELS B968).
pub const LABEL: [u8; 4] = [0xff, 0xff, 0xff, 255];
pub const REEL: [[u8; 3]; 6] =
    [[0x4a, 0x7d, 0xc9], [0xc9, 0x7d, 0x4a], [0x5a, 0xa8, 0x6a], [0xa8, 0x5a, 0xa0], [0xb8, 0xa8, 0x4a], [0x4a, 0xa8, 0xa8]];

/// A clip's colour: its reel's, darker on an audio track.
pub fn clip_colour(src: u32, kind: Kind) -> [u8; 4] {
    let c = REEL[src as usize % REEL.len()];
    match kind {
        Kind::Video => [c[0], c[1], c[2], 255],
        Kind::Audio => [c[0] / 4 * 3, c[1] / 4 * 3, c[2] / 4 * 3, 255],
    }
}

/// A canvas in one byte order.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub order: Order,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32, order: Order) -> Canvas {
        Canvas { w, h, order, px: vec![0; w as usize * h as usize * 4] }
    }
    pub fn put(&self, c: [u8; 4]) -> [u8; 4] {
        match self.order {
            Order::Rgba => c,
            Order::Bgra => [c[2], c[1], c[0], c[3]],
        }
    }
    /// Fill `x0..x1` x `y0..y1`, clipped to the canvas.
    pub fn rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 4]) {
        let (x0, x1) = (x0.clamp(0, self.w as i64) as usize, x1.clamp(0, self.w as i64) as usize);
        let (y0, y1) = (y0.clamp(0, self.h as i64) as usize, y1.clamp(0, self.h as i64) as usize);
        let c = self.put(c);
        for y in y0..y1 {
            let row = y * self.w as usize * 4;
            for x in x0..x1 {
                self.px[row + x * 4..row + x * 4 + 4].copy_from_slice(&c);
            }
        }
    }
    /// A 1 px outline of `x0..x1` x `y0..y1`.
    pub fn frame(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 4]) {
        self.rect(x0, y0, x1, y0 + 1, c);
        self.rect(x0, y1 - 1, x1, y1, c);
        self.rect(x0, y0, x0 + 1, y1, c);
        self.rect(x1 - 1, y0, x1, y1, c);
    }
    /// `s` nearest-scaled to fit (aspect kept) and centred in `x0..x0+bw` x `y0..y0+bh`.
    /// VIEW-SCALE-ONE (NLESIZEPOLICY, LEDGER SR394): [`Canvas::blit_policy`]'s letterbox, yuv_core::scale's integer `fit`.
    pub fn blit_fit(&mut self, s: &Still, x0: u32, y0: u32, bw: u32, bh: u32) {
        self.blit_policy(s, x0, y0, bw, bh, crate::SizePolicy::Letterbox);
    }
    /// NLESIZEPOLICY (LEDGER SR394): `s` into the box `x0..x0+bw` x `y0..y0+bh` by `fit` ([`crate::SizePolicy::place`],
    /// the ONE geometry), sampled by yuv_core::scale's ONE `nearest_rows` (the frame as a mono picture of RGBA samples;
    /// output row `r` from source row `r * h / dh`, column `c` from `c * w / dw`, inside the placed source rect), each
    /// sample through [`Canvas::put`], clipped to the canvas. The box outside the placed rect is left as it was.
    pub fn blit_policy(&mut self, s: &Still, x0: u32, y0: u32, bw: u32, bh: u32, fit: crate::SizePolicy) {
        let (sw, sh) = (s.width as usize, s.height as usize);
        if sw == 0 || sh == 0 || bw == 0 || bh == 0 || s.rgba.len() < sw * sh * 4 {
            return;
        }
        let p = fit.place(sw, sh, bw as usize, bh as usize);
        let (px, _) = s.rgba.as_chunks::<4>();
        let pic = yuv_core::Picture { w: p.src.w, h: p.src.h, y: &px[p.src.y * sw + p.src.x..], u: &[], v: &[], y_stride: sw, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
        if !yuv_core::scale::fits(&pic) {
            return;
        }
        let (cw, ch) = (self.w as usize, self.h as usize);
        let (ox, oy) = (x0 as usize + p.dst.x, y0 as usize + p.dst.y);
        let mut ry = vec![[0u8; 4]; p.dst.w];
        let pen = Canvas { w: 0, h: 0, order: self.order, px: Vec::new() }; // `put`'s byte order, while the closure holds `px`
        let dst = &mut self.px;
        yuv_core::scale::nearest_rows(&pic, p.dst.w, p.dst.h, &mut ry, &mut [], &mut [], |r, row| {
            let y = oy + r;
            if y >= ch {
                return;
            }
            for (c, &sample) in row.y.iter().enumerate() {
                let x = ox + c;
                if x >= cw {
                    break;
                }
                let di = (y * cw + x) * 4;
                dst[di..di + 4].copy_from_slice(&pen.put(sample));
            }
        });
    }
}

/// The label hook: one line of text at `x` / `baseline`, clipped to columns `clip` (the host's text_host shaper; a face
/// with no font passes none and the paint is the rects alone).
pub trait Text {
    fn text(&mut self, c: &mut Canvas, s: &str, x: f32, baseline: f32, clip: (i32, i32));
}

/// Paint the editor on a `w` x `h` canvas in `order`. `labels` names the clips (and the seconds on the ruler); `preview`
/// is the frame at the playhead.
pub fn paint(st: &EditorState, w: u32, h: u32, order: Order, labels: Option<&mut dyn Text>, preview: Option<&Still>) -> Vec<u8> {
    paint_in(st, Layout::of(w, h, st), order, labels, preview)
}

/// [`paint`] in a given layout (a face's view, which a wheel zoom anchors at the pointer, or a Surface's role).
pub fn paint_in(st: &EditorState, l: Layout, order: Order, mut labels: Option<&mut dyn Text>, preview: Option<&Still>) -> Vec<u8> {
    let (w, h) = (l.w, l.h);
    let tl = &st.timeline;
    let mut c = Canvas::new(w, h, order);
    c.rect(0, 0, w as i64, h as i64, BG);
    // The preview band.
    c.rect(0, 0, w as i64, l.preview_h as i64, FIELD);
    if let Some(s) = preview {
        c.blit_fit(s, 0, 0, w, l.preview_h);
    }
    // The ruler: a short mark per tick when they are 4 px apart, a long one per second.
    let fps = tl.fps().unwrap_or(25).max(1) as i64;
    let (ry, ty) = (l.ruler_y as i64, l.tracks_y as i64);
    c.rect(0, ry, w as i64, ty, RULER);
    let t_end = l.tick_at(w as f64) + 1;
    for t in l.t0..=t_end {
        let x = l.x_of(t);
        if t % fps == 0 {
            c.rect(x, ry + RULER_H as i64 / 2, x + 1, ty, TICK);
            if let Some(text) = labels.as_mut().filter(|_| l.zoom as i64 * fps >= 60) {
                text.text(&mut c, &edl::timecode(t, fps as u32), x as f32 + 2.0, ry as f32 + 9.0, (x as i32, w as i32));
            }
        } else if l.zoom >= 4 {
            c.rect(x, ty - 3, x + 1, ty, TICK);
        }
    }
    // The tracks and their clips.
    for (row, track) in tl.tracks.iter().enumerate() {
        let y0 = l.row_y(row);
        let y1 = y0 + ROW_H as i64;
        c.rect(0, y0, w as i64, y1, ROW[row % 2]);
        for clip in &track.clips {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 <= 0 || x0 >= w as i64 {
                continue;
            }
            c.rect(x0 + 1, y0 + 2, x1 - 1, y1 - 2, clip_colour(clip.src.0, track.kind));
            if let (Some(text), Some(src)) = (labels.as_mut(), tl.source(clip.src)) {
                let label = if src.size_policy == crate::SizePolicy::Letterbox { src.reel.clone() } else { alloc::format!("{} {}", src.reel, src.size_policy.name()) }; // NLESIZEPOLICYUI (SR402): the reel's policy as a token
                text.text(&mut c, &label, x0 as f32 + 4.0, y0 as f32 + 15.0, ((x0 + 1) as i32, (x1 - 1) as i32));
            }
        }
    }
    // The selection (NLE2c: every clip of the set outlined), and the primary's two trim handles.
    for &(row, i) in &st.set {
        if let Some(clip) = tl.tracks.get(row).and_then(|tr| tr.clips.get(i)) {
            c.frame(l.x_of(clip.at), l.row_y(row), l.x_of(clip.end()), l.row_y(row) + ROW_H as i64, SELECT);
        }
    }
    if let (Some((row, _)), Some(clip)) = (st.selection, st.selected()) {
        let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
        let (y0, y1) = (l.row_y(row), l.row_y(row) + ROW_H as i64);
        c.frame(x0, y0, x1, y1, SELECT);
        c.rect(x0, y0 + 2, x0 + HANDLE_W, y1 - 2, HANDLE);
        c.rect(x1 - HANDLE_W, y0 + 2, x1, y1 - 2, HANDLE);
    }
    paint_transitions(&mut c, l, st); paint_reverse_badges(&mut c, l, st); // STRIAREVERSEEDIT (SR9094): a reversed body's badge, nothing when none is reversed (file tail)
    // The playhead, through the ruler and the tracks.
    let px = l.x_of(st.playhead);
    c.rect(px, ry, px + 1, h as i64, PLAYHEAD);
    c.px
}

/// FNV-1a 64 of a buffer (the goldens' hash, tools/nle's, the kernel fixture's view pin).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

// ---------------------------------------------------------------------------------------------------------------------
// THE TRANSITION WIDGET (NLE3d, LEDGER SR302)
// ---------------------------------------------------------------------------------------------------------------------

/// The widget's height (px): a band at the foot of the track row.
pub const TRANS_H: i64 = 10;
/// The narrowest widget (px), so a short transition at a low zoom stays clickable.
pub const TRANS_MIN_W: i64 = 8;
/// The selected transition's thumbnail box (px), above its row.
pub const THUMB_W: i64 = 48;
pub const THUMB_H: i64 = 36;
pub const TRANS: [u8; 4] = [0xe0, 0x90, 0xff, 255];
pub const TRANS_FILL: [u8; 4] = [0x50, 0x30, 0x70, 255];
/// The kind glyphs, 7x7 (bit 6 the leftmost column): D, W001, W002, W041, W101, W201.
const GLYPHS: [[u8; 7]; 6] = [
    [0x41, 0x22, 0x14, 0x08, 0x14, 0x22, 0x41],
    [0x70, 0x70, 0x70, 0x70, 0x70, 0x70, 0x70],
    [0x7f, 0x7f, 0x7f, 0x00, 0x00, 0x00, 0x00],
    [0x7f, 0x7e, 0x7c, 0x78, 0x70, 0x60, 0x40],
    [0x7f, 0x41, 0x5d, 0x55, 0x5d, 0x41, 0x7f],
    [0x1c, 0x2a, 0x49, 0x4f, 0x41, 0x22, 0x1c],
];

fn glyph(kind: TransitionKind) -> &'static [u8; 7] {
    &GLYPHS[match kind {
        TransitionKind::Dissolve => 0,
        TransitionKind::Wipe(WipeDir::LeftToRight | WipeDir::BarnDoorVertical) => 1,
        TransitionKind::Wipe(WipeDir::TopToBottom | WipeDir::BarnDoorHorizontal) => 2,
        TransitionKind::Wipe(WipeDir::Diagonal | WipeDir::DiagonalTopRight | WipeDir::BowTieVertical | WipeDir::BowTieHorizontal | WipeDir::BarnDoorDiagonalBottomLeft | WipeDir::BarnDoorDiagonalTopLeft | WipeDir::DoubleBarnDoor) => 3,
        TransitionKind::Wipe(WipeDir::Box | WipeDir::CornerTopLeft | WipeDir::CornerTopRight | WipeDir::CornerBottomRight | WipeDir::CornerBottomLeft | WipeDir::FourBoxCornersIn | WipeDir::EdgeBoxTop | WipeDir::EdgeBoxRight | WipeDir::EdgeBoxBottom | WipeDir::EdgeBoxLeft | WipeDir::Diamond | WipeDir::Circle) => 4,
        TransitionKind::Wipe(WipeDir::Clock | WipeDir::ClockThree | WipeDir::ClockSix | WipeDir::ClockNine | WipeDir::PinWheelTwoVertical | WipeDir::PinWheelTwoHorizontal | WipeDir::PinWheelFour | WipeDir::FanCenterTop | WipeDir::FanCenterRight | WipeDir::DoubleFanVertical | WipeDir::DoubleFanHorizontal) => 5,
    }]
}

impl Layout {
    /// THE widget geometry, read by [`paint_in`] and [`Layout::transition_at`]: a bracket spanning the transition's `dur`
    /// ticks from its cut (`x_of(at)..x_of(at + dur)`, at least [`TRANS_MIN_W`] wide) in the foot band of row `row`.
    pub fn transition_rect(&self, row: usize, x: &Transition) -> (i64, i64, i64, i64) {
        let x0 = self.x_of(x.at);
        let x1 = self.x_of(x.at + x.dur).max(x0 + TRANS_MIN_W);
        let y1 = self.row_y(row) + ROW_H as i64 - 2;
        (x0, y1 - TRANS_H, x1, y1)
    }
    /// The transition whose widget is under `(x, y)`: `(track, index)` (the last drawn wins).
    pub fn transition_at(&self, tl: &Timeline, x: f64, y: f64) -> Option<(usize, usize)> {
        let mut hit = None;
        for (row, tr) in tl.tracks.iter().enumerate().take(self.rows) {
            for (i, t) in tr.transitions.iter().enumerate() {
                let (x0, y0, x1, y1) = self.transition_rect(row, t);
                if x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64 {
                    hit = Some((row, i));
                }
            }
        }
        hit
    }
    /// The thumbnail box `(x0, y0)` ([`THUMB_W`] x [`THUMB_H`]): above row `row`, at the widget's left, kept on the canvas.
    pub fn transition_thumb(&self, row: usize, x: &Transition) -> (i64, i64) {
        let x0 = self.x_of(x.at).min((self.w as i64 - THUMB_W).max(0)).max(0);
        (x0, (self.row_y(row) - THUMB_H).max(0))
    }
}

/// The tick a transition's preview shows: its midpoint, `at + dur / 2`.
pub fn transition_mid(x: &Transition) -> i64 {
    x.at + x.dur / 2
}

/// Every transition's widget: the bracket (outlined white when selected), its kind glyph and, from black, a black
/// marker on its outgoing (left) edge; NLE3cBLACK: to black, one on its incoming (right) edge.
fn paint_transitions(c: &mut Canvas, l: Layout, st: &EditorState) {
    for (row, tr) in st.timeline.tracks.iter().enumerate().take(l.rows) {
        for (i, x) in tr.transitions.iter().enumerate() {
            let (x0, y0, x1, y1) = l.transition_rect(row, x);
            if x1 <= 0 || x0 >= l.w as i64 {
                continue;
            }
            c.rect(x0, y0, x1, y1, TRANS_FILL);
            c.frame(x0, y0, x1, y1, if st.transition == Some((row, i)) { SELECT } else { TRANS });
            let gx = if x.from_black {
                c.rect(x0 + 1, y0 + 1, (x0 + 4).min(x1 - 1), y1 - 1, FIELD);
                x0 + 5
            } else {
                x0 + 2
            };
            if x.to_black { c.rect((x1 - 4).max(x0 + 1), y0 + 1, x1 - 1, y1 - 1, FIELD); } // NLE3cBLACK: the to-black marker
            for (dy, bits) in glyph(x.kind).iter().enumerate() {
                for dx in 0..7i64 {
                    if bits & (0x40 >> dx) != 0 && gx + dx < x1 - 1 {
                        let (px, py) = (gx + dx, y0 + 2 + dy as i64);
                        c.rect(px, py, px + 1, py + 1, TRANS);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// The kernel `tests nle` keys leg's timeline after its `s` / `]` / `u` (player.rs `nle_fx::keys_leg`): NLEFXA 0..20
    /// at 0 split at 8.
    fn keys_state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        EditorState::new(tl.split(0, 8).unwrap())
    }

    /// NLE6KEYS (rmbp-ledger B961) M3: the kernel fixture's pins, host-computed through the SAME `paint_in` and
    /// `edl::write` the kernel calls: the saved EDL's text, and the view band (Role::Tracks, 320x56, the right half
    /// selected, playhead 8). `KEY_VIEW_KAT` / `KEY_EDL` in player.rs `nle_fx` are these literals.
    #[test]
    fn kernel_keys_view_pin() {
        let mut st = keys_state();
        assert_eq!(
            edl::write(&st.timeline, "NLEKEY").unwrap(),
            "TITLE: NLEKEY\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE NLEFXA 00:00:01:15\n\n001  NLEFXA   V     C        00:00:00:00 00:00:00:08 00:00:00:00 00:00:00:08\n002  NLEFXA   V     C        00:00:00:08 00:00:00:20 00:00:00:08 00:00:00:20\n"
        );
        st.select_at(0, 8).unwrap();
        st.seek(8);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        assert_eq!((l.ruler_y, l.tracks_y, l.t0), (0, RULER_H, 0));
        let px = paint_in(&st, l, Order::Rgba, None, None);
        let at = |x: usize, y: usize| &px[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        assert_eq!(at(64, 30), PLAYHEAD, "the playhead at tick 8");
        assert_eq!(at(66, 30), HANDLE, "the right half's in handle");
        assert_eq!(at(10, 30), clip_colour(0, Kind::Video), "the left half");
        assert_eq!(fnv1a(&px), 0x53f5_5489_46f1_502f);
    }

    /// The label hook as a recorder: every `(text, x, baseline, clip)` call, in order (no pixels).
    struct Rec(Vec<(alloc::string::String, f32, f32, (i32, i32))>);

    impl Text for Rec {
        fn text(&mut self, _: &mut Canvas, s: &str, x: f32, baseline: f32, clip: (i32, i32)) {
            self.0.push((alloc::string::String::from(s), x, baseline, clip));
        }
    }

    /// The kernel keys fixture's view band (Role::Tracks, 320x56, playhead 8, the right half selected) through a recorder.
    fn keys_labels(st: &EditorState) -> (Vec<(alloc::string::String, f32, f32, (i32, i32))>, u64) {
        let l = Layout::of_role(Role::Tracks, 320, 56, st);
        let mut rec = Rec(Vec::new());
        let px = paint_in(st, l, Order::Rgba, Some(&mut rec as &mut dyn Text), None);
        (rec.0, fnv1a(&px))
    }

    fn calls(v: &[(&str, f32, f32, (i32, i32))]) -> Vec<(alloc::string::String, f32, f32, (i32, i32))> {
        v.iter().map(|&(s, x, b, c)| (alloc::string::String::from(s), x, b, c)).collect()
    }

    /// NLE6KEYSLABELS (rmbp-ledger B968) M3: the label calls the kernel view band's `video::text` hook receives for the
    /// NLEKEY fixture — the ruler's timecode at each whole second (zoom 8 x 25 fps >= 60) and each clip's reel at its left
    /// edge, clipped to the clip — at the host's positions; the hook paints no rect, so the band's FNV is `KEY_VIEW_KAT`.
    #[test]
    fn kernel_keys_labels_pin() {
        let mut st = keys_state();
        st.select_at(0, 8).unwrap();
        st.seek(8);
        let (got, fnv) = keys_labels(&st);
        assert_eq!(
            got,
            calls(&[
                ("00:00:00:00", 2.0, 9.0, (0, 320)),
                ("00:00:01:00", 202.0, 9.0, (200, 320)),
                ("NLEFXA", 4.0, 31.0, (1, 63)),
                ("NLEFXA", 68.0, 31.0, (65, 159)),
            ])
        );
        assert_eq!(fnv, 0x53f5_5489_46f1_502f);
    }

    /// NLE6KEYSLABELS (rmbp-ledger B968) M3: the kernel's zoom keys (`+` / `=` -> `zoom+`, `-` -> `zoom-`, `0` -> `zoom0`)
    /// are `EditorState::verb`'s ONE zoom, and the view's `Layout` follows it: below 60 px per second the ruler names none.
    #[test]
    fn kernel_zoom_keys_pin() {
        let mut st = keys_state();
        st.select_at(0, 8).unwrap();
        st.seek(8);
        assert_eq!(st.verb("zoom+"), Some(Ok(())));
        assert_eq!(st.zoom, 16);
        assert_eq!(Layout::of_role(Role::Tracks, 320, 56, &st).zoom, 16);
        for _ in 0..3 {
            assert_eq!(st.verb("zoom-"), Some(Ok(())));
        }
        assert_eq!(st.zoom, 2);
        let (got, _) = keys_labels(&st);
        assert_eq!(got, calls(&[("NLEFXA", 4.0, 31.0, (1, 15)), ("NLEFXA", 20.0, 31.0, (17, 39))]));
        assert_eq!(st.verb("zoom0"), Some(Ok(())));
        assert_eq!(st.zoom, crate::editor::ZOOM_DEFAULT);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// NLE6FACEDRAG (rmbp-ledger B975): THE DRAG — one pointer gesture model for both faces
// ---------------------------------------------------------------------------------------------------------------------
// Moved unchanged from `stria::nle_face` (NLE2B LEDGER SR208, NLE2c SR217), as NLE6KEYS (B961) moved `Layout`: a press
// classified by the drawn trim handles (`HANDLE_W`) then `Layout::hit`, updated by moves (snapped within `SNAP_PX` unless
// the no-snap modifier), committed ONCE on release through `EditorState`'s ops (`trim`, `move_selected`, `move_set`,
// `select_many`). `stria::nle_face` re-exports every name here; the kernel Player's view band (vplay.rs `edit_press` /
// `edit_drag_move` / `edit_drag_end`) calls the same fns over the layout it painted.

use crate::EditError;
use crate::editor::Edge;

/// What a drag does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// The selected clip's in handle (`EditorState::trim(Edge::In, ..)`).
    TrimIn,
    /// The selected clip's out handle (`EditorState::trim(Edge::Out, ..)`).
    TrimOut,
    /// NLETRANSITIONFACE (SR691): the selected transition's left (outgoing) edge (`EditorState::set_transition_dur`, `dur - delta`).
    TransitionIn,
    /// NLETRANSITIONFACE (SR691): the selected transition's right (incoming) edge (`EditorState::set_transition_dur`, `dur + delta`).
    TransitionOut,
    /// A clip body (`EditorState::move_selected`; NLE2c: `EditorState::move_set` for a set of more than one).
    Move,
    /// NLE2c: a rubber band from an empty track spot (`EditorState::select_many` of the clips it crosses).
    Marquee,
    /// NLE2c: a press on the ruler; the playhead follows the pointer (a seek, not an edit).
    Scrub,
    /// NLE3ESLIPDRAG (SR760): the slip modifier on a clip body (`EditorState::slip_selected`: the source window moves).
    Slip,
    /// NLE3ESLIPDRAG (SR760): the slip modifier on the selected clip's trim handle (`EditorState::slide_selected`).
    Slide,
}

/// One pointer drag: begun by a press, updated by moves, committed ONCE on release ([`Drag::finish`]). It reads the
/// layout frozen at the press, so the pixel -> tick mapping cannot shift under it, and it edits only through
/// [`EditorState`]'s public ops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub kind: DragKind,
    pub track: usize,
    pub idx: usize,
    /// The tick under the press.
    pub origin_tick: i64,
    pub layout: Layout,
    /// Ticks moved since the press (snapped when `snap`).
    pub delta: i64,
    /// Where a move lands (a track of the clip's kind).
    pub to_track: usize,
    /// NLE2c: the press and the pointer now (surface pixels).
    pub origin: (f64, f64),
    pub at: (f64, f64),
    /// NLE2c: snap the moving edge to clip edges / the playhead (off under the no-snap modifier).
    pub snap: bool,
    /// NLE2c: begun with shift (adds to the set instead of replacing it).
    pub additive: bool,
    /// NLE2c: the tick the delta snapped to (painted as the guide line).
    pub guide: Option<i64>,
}

impl Drag {
    /// A press at `(x, y)` with no modifier ([`Drag::begin_with`]).
    pub fn begin(st: &mut EditorState, l: Layout, x: f64, y: f64) -> Option<Drag> {
        Drag::begin_with(st, l, x, y, false, false, false)
    }
    /// A press at `(x, y)`: on the primary's drawn trim handle (the rects [`paint`] draws) a trim; on a clip body a move
    /// (a clip of the set keeps the set and becomes its primary, `shift` adds the clip, else it alone is selected); on
    /// an empty track spot a marquee; on the ruler a scrub; anywhere else no drag. NLE3ESLIPDRAG (SR760): with `slip` the
    /// same hits begin a slide (the trim handle) or a slip (the clip body) instead.
    pub fn begin_with(st: &mut EditorState, l: Layout, x: f64, y: f64, shift: bool, nosnap: bool, slip: bool) -> Option<Drag> {
        let origin_tick = l.tick_at(x);
        let mk = |kind, track, idx| Drag {
            kind,
            track,
            idx,
            origin_tick,
            layout: l,
            delta: 0,
            to_track: track,
            origin: (x, y),
            at: (x, y),
            snap: !nosnap,
            additive: shift,
            guide: None,
        };
        if let (Some((track, idx)), Some(c), false) = (st.selection, st.selected(), shift) {
            let y0 = l.row_y(track) as f64;
            if y >= y0 && y < y0 + ROW_H as f64 {
                let (x0, x1) = (l.x_of(c.at) as f64, l.x_of(c.end()) as f64);
                let hw = HANDLE_W as f64;
                let kind = if x >= x0 && x < x0 + hw {
                    Some(DragKind::TrimIn)
                } else if x >= x1 - hw && x < x1 {
                    Some(DragKind::TrimOut)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    return Some(mk(if slip { DragKind::Slide } else { kind }, track, idx));
                }
            }
        }
        match l.hit(&st.timeline, x, y) {
            Hit::Clip(track, idx, _) => {
                if shift || st.set.contains(&(track, idx)) {
                    st.select_add(track, idx).ok()?;
                } else {
                    st.select(track, idx).ok()?;
                }
                Some(mk(if slip { DragKind::Slip } else { DragKind::Move }, track, idx))
            }
            Hit::Track(track, _) => Some(mk(DragKind::Marquee, track, 0)),
            Hit::Ruler(_) => Some(mk(DragKind::Scrub, 0, 0)),
            Hit::None => None,
        }
    }
    /// The pointer is at `(x, y)`: the tick delta (snapped), and for a move of one clip the row under it when that
    /// track's kind matches.
    pub fn update(&mut self, st: &EditorState, x: f64, y: f64) {
        self.at = (x, y);
        self.delta = self.layout.tick_at(x) - self.origin_tick;
        self.guide = None;
        if self.snap {
            self.snap_delta(st, x);
        }
        if self.kind != DragKind::Move || st.set.len() > 1 {
            return;
        }
        if let Hit::Clip(row, ..) | Hit::Track(row, _) = self.layout.hit(&st.timeline, x, y) {
            let from = st.timeline.tracks.get(self.track).map(|t| t.kind);
            if from.is_some() && from == st.timeline.tracks.get(row).map(|t| t.kind) {
                self.to_track = row;
            }
        }
    }
    /// The drag would change something.
    pub fn moved(&self) -> bool {
        match self.kind {
            DragKind::Marquee => {
                (self.at.0 - self.origin.0).abs() >= MARQUEE_MIN || (self.at.1 - self.origin.1).abs() >= MARQUEE_MIN
            }
            _ => self.delta != 0 || self.to_track != self.track,
        }
    }
    /// THE commit of the gesture: one `trim`, `move_selected` / `move_set` or marquee selection (a scrub commits nothing).
    pub fn finish(&self, st: &mut EditorState) -> Result<(), EditError> {
        match self.kind {
            DragKind::Marquee => {
                let (x0, y0, x1, y1) = (self.origin.0, self.origin.1, self.at.0, self.at.1);
                st.select_many(&self.layout.crossing(&st.timeline, x0, y0, x1, y1), self.additive);
                return Ok(());
            }
            DragKind::Scrub => return Ok(()), DragKind::TransitionIn | DragKind::TransitionOut => return self.finish_transition(st),
            _ => {}
        }
        if st.selection != Some((self.track, self.idx)) {
            st.select(self.track, self.idx)?;
        }
        match self.kind {
            DragKind::TrimIn => st.trim_by_ticks(Edge::In, self.delta), // STRIACLIPSPEEDMORE (SR951): the timeline delta through src_at (1x: the same source delta)
            DragKind::TrimOut => st.trim_by_ticks(Edge::Out, self.delta), // STRIACLIPSPEEDMORE (SR951): the timeline delta through src_at (1x: the same source delta)
            DragKind::Slip => st.slip_selected(self.delta),
            DragKind::Slide => st.slide_selected(self.delta),
            DragKind::Move if st.set.len() > 1 => st.move_set(self.delta),
            _ => {
                let at = st.selected().ok_or(EditError::NoClip)?.at;
                st.move_selected(self.to_track, at + self.delta)
            }
        }
    }
    /// The preview: [`Drag::finish`] on a scratch state (no undo stacks); `None` when nothing moved or the op refuses.
    pub fn shown(&self, st: &EditorState) -> Option<EditorState> {
        if !self.moved() {
            return None;
        }
        let mut s = EditorState::new(st.timeline.clone());
        s.selection = st.selection;
        s.set = st.set.clone();
        s.playhead = st.playhead;
        s.zoom = st.zoom;
        self.finish(&mut s).ok()?;
        Some(s)
    }
}

/// How near (px, at the current zoom) a moving edge must come to a clip edge or the playhead to snap to it.
pub const SNAP_PX: f64 = 6.0;
/// How far (px) the pointer must travel before a press on an empty track is a marquee rather than a click.
pub const MARQUEE_MIN: f64 = 3.0;

/// Where a moving edge may snap: every clip's `at` and `end` on any track except the `exclude`d clips', plus the
/// playhead when `playhead`; sorted, deduped.
pub fn snap_targets(st: &EditorState, exclude: &[(usize, usize)], playhead: bool) -> Vec<i64> {
    let mut v = Vec::new();
    for (t, tr) in st.timeline.tracks.iter().enumerate() {
        for (i, c) in tr.clips.iter().enumerate() {
            if !exclude.contains(&(t, i)) {
                v.push(c.at);
                v.push(c.end());
            }
        }
    }
    if playhead {
        v.push(st.playhead);
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The target nearest pixel `x` whose `x_of` lies within [`SNAP_PX`] of it (a tie: the earlier tick); `None` when none
/// is in range.
pub fn snap_to(l: &Layout, targets: &[i64], x: f64) -> Option<i64> {
    let mut best: Option<(f64, i64)> = None;
    for &t in targets {
        let d = (l.x_of(t) as f64 - x).abs();
        if d <= SNAP_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t)
}

impl Drag {
    /// Snap the delta: each moving edge sits at pixel `x_of(edge) + (x - origin.x)` (a scrub's is the pointer); the
    /// nearest (edge, target) pair within [`SNAP_PX`] sets `delta = target - edge` and the guide. A marquee never snaps.
    fn snap_delta(&mut self, st: &EditorState, x: f64) {
        let l = self.layout;
        let dx = x - self.origin.0;
        let one = [(self.track, self.idx)];
        let slide_ex: Vec<(usize, usize)>;
        let (edges, exclude, playhead): (Vec<i64>, &[(usize, usize)], bool) = match self.kind {
            DragKind::Marquee | DragKind::Slip => return, DragKind::TransitionIn | DragKind::TransitionOut => return self.snap_transition(st, x),
            DragKind::Scrub => {
                if let Some(t) = snap_to(&l, &snap_targets(st, &[], false), x) {
                    self.delta = t - self.origin_tick;
                    self.guide = Some(t);
                }
                return;
            }
            DragKind::TrimIn | DragKind::TrimOut => {
                let Some(c) = st.timeline.tracks.get(self.track).and_then(|tr| tr.clips.get(self.idx)) else { return };
                (vec![if self.kind == DragKind::TrimIn { c.at } else { c.end() }], &one, true)
            }
            DragKind::Move => {
                let set: &[(usize, usize)] = if st.set.len() > 1 { &st.set } else { &one };
                let edges = set
                    .iter()
                    .filter_map(|&(t, i)| st.timeline.tracks.get(t)?.clips.get(i))
                    .flat_map(|c| [c.at, c.end()])
                    .collect();
                (edges, set, true)
            }
            DragKind::Slide => {
                let Some(tr) = st.timeline.tracks.get(self.track) else { return };
                let Some(&c) = tr.clips.get(self.idx) else { return };
                let butted = |i: usize| tr.clips.get(i).is_some_and(|n| n.end() == c.at || n.at == c.end());
                slide_ex = [self.idx.checked_sub(1), Some(self.idx), Some(self.idx + 1)]
                    .into_iter()
                    .flatten()
                    .filter(|&i| i == self.idx || butted(i))
                    .map(|i| (self.track, i))
                    .collect();
                (alloc::vec![c.at, c.end()], &slide_ex, true)
            }
        };
        let targets = snap_targets(st, exclude, playhead);
        let mut best: Option<(f64, i64, i64)> = None;
        for &e in &edges {
            let ex = l.x_of(e) as f64 + dx;
            if let Some(t) = snap_to(&l, &targets, ex) {
                let d = (l.x_of(t) as f64 - ex).abs();
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, e, t));
                }
            }
        }
        if let Some((_, e, t)) = best {
            self.delta = t - e;
            self.guide = Some(t);
        }
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLE6FACEDRAG (rmbp-ledger B975) M3, NLE6DRAGSNAP (rmbp-ledger B977): the kernel fixture's drag leg (player.rs
    /// `nle_fx::drag_leg`) on the host: NLEFXA 0..20 at 0 and NLEFXA 20..30 at 30, the view band `Role::Tracks` 320x56,
    /// snap ON (the kernel's press). A press on the first clip's body at tick 5 dragged +3 ticks is `move_clip(0, 0, 0,
    /// 3)`; a press on its out handle (`x_of(end) - 2`) dragged -5 ticks is `trim_out(0, 0, 15)` (whole ticks at 8 px: no
    /// snap moves either); a body press at `x_of(5) + 1` dragged +94 px is a raw 11 ticks whose out edge lands 2 px short
    /// of the second clip's `at` 30 and snaps: `move_clip(0, 0, 0, 15)`; the saved text is `DRAG_EDL` in player.rs, this
    /// literal.
    #[test]
    fn kernel_drag_pin() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(a, 20, 30, 30)];
        let want_move = tl.move_clip(0, 0, 0, 3).unwrap();
        let want_trim = want_move.trim_out(0, 0, 15).unwrap();
        let want_snap = want_trim.move_clip(0, 0, 0, 15).unwrap();
        let mut st = EditorState::new(tl);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        let x = l.x_of(5) as f64 + z / 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.origin_tick), (DragKind::Move, 5));
        d.update(&st, x + 3.0 * z, y);
        assert_eq!(d.guide, None);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, want_move);
        let x = l.x_of(23) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.origin_tick), (DragKind::TrimOut, 22));
        d.update(&st, x - 5.0 * z, y);
        assert_eq!((d.origin_tick + d.delta, d.guide), (17, None));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, want_trim);
        let x = l.x_of(5) as f64 + 1.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.origin_tick), (DragKind::Move, 5));
        d.update(&st, x + 94.0, y);
        assert_eq!(l.tick_at(x + 94.0) - d.origin_tick, 11, "the raw delta");
        assert_eq!((d.delta, d.guide), (12, Some(30)), "snapped to the second clip's in edge");
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, want_snap);
        assert_eq!(st.undo_depth(), 3);
        assert_eq!(
            edl::write(&st.timeline, "NLEDRG").unwrap(),
            "TITLE: NLEDRG\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE NLEFXA 00:00:01:15\n\n001  NLEFXA   V     C        00:00:00:00 00:00:00:15 00:00:00:15 00:00:01:05\n002  NLEFXA   V     C        00:00:00:20 00:00:01:05 00:00:01:05 00:00:01:15\n"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// NLE6DRAGSNAP (rmbp-ledger B977): THE DRAG'S PAINT — the snap guide and the marquee, one fn for both faces
// ---------------------------------------------------------------------------------------------------------------------
// Moved byte for byte from `stria::nle_face` (`paint_drag`, `GUIDE`, `MARQUEE`; NLE2c LEDGER SR217). The host face paints it
// over `paint_in` of the drag's preview state (`Drag::shown`); the kernel Player's view band (vplay.rs `view_band`) does
// the same, repainted on each drag motion (vplay.rs `band_repaint`).

/// The snap guide's colour (a vertical line from the ruler down at [`Drag::guide`]).
pub const GUIDE: [u8; 4] = [0x40, 0xd0, 0xff, 255];
/// The marquee's outline.
pub const MARQUEE: [u8; 4] = [0xa0, 0xa0, 0xff, 255];

/// The live drag over the painted canvas: the snap guide (a vertical line from the ruler down) and the marquee's rect.
pub fn paint_drag(px: Vec<u8>, l: Layout, order: Order, drag: Option<&Drag>) -> Vec<u8> {
    let Some(d) = drag else { return px };
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    if let Some(t) = d.guide {
        let x = l.x_of(t);
        c.rect(x, l.ruler_y as i64, x + 1, l.h as i64, GUIDE);
    }
    if d.kind == DragKind::Marquee && d.moved() {
        let (x0, x1) = (d.origin.0.min(d.at.0) as i64, d.origin.0.max(d.at.0) as i64 + 1);
        let (y0, y1) = (d.origin.1.min(d.at.1) as i64, d.origin.1.max(d.at.1) as i64 + 1);
        c.frame(x0, y0, x1, y1, MARQUEE);
    }
    c.px
}

#[cfg(test)]
mod snap_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLE6DRAGSNAP (rmbp-ledger B977) M3: the kernel band's paint of the fixture's snap drag (`drag_tests::kernel_drag_pin`'s
    /// third drag) — the guide in [`GUIDE`] down the band at `x_of(30)`, the preview (`Drag::shown`) painting the moved clip
    /// over 15..30 while the committed state still ends it at 18.
    #[test]
    fn kernel_drag_snap_paint_pin() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 15, 3), Clip::new(a, 20, 30, 30)];
        let mut st = EditorState::new(tl);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let y = l.row_y(0) as f64 + ROW_H as f64 / 2.0;
        let x = l.x_of(5) as f64 + 1.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        d.update(&st, x + 94.0, y);
        assert_eq!(d.guide, Some(30));
        let at = |px: &[u8], x: i64, y: i64| {
            let i = (y as usize * l.w as usize + x as usize) * 4;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        let yi = y as i64;
        let before = paint_in(&st, l, Order::Rgba, None, None);
        assert_eq!(at(&before, l.x_of(25), yi), ROW[0], "committed: nothing at tick 25");
        let shown = d.shown(&st).unwrap();
        let px = paint_drag(paint_in(&shown, l, Order::Rgba, None, None), l, Order::Rgba, Some(&d));
        assert_eq!(at(&px, l.x_of(25), yi), clip_colour(a.0, Kind::Video), "the preview's moved clip");
        assert_eq!(at(&px, l.x_of(30), yi), GUIDE, "the guide line");
        assert_eq!(at(&px, l.x_of(30), l.ruler_y as i64), GUIDE, "the guide from the ruler down");
        assert_eq!(paint_drag(before.clone(), l, Order::Rgba, None), before, "no drag: unchanged");
    }
}

// ---- NLETRANSITIONFACE (LEDGER SR691): the transition widget's edge drag (named tail block) ----
// NLE3d (SR302) owed dragging a transition's edges. The gesture is THE drag model's (both faces call it): a press within
// `HANDLE_W` of the DRAWN left / right edge of `Layout::transition_rect` begins `TransitionIn` / `TransitionOut` (the
// transition selected); `update` snaps the moving edge (`at` / `at + dur`) through the same `snap_to` / `snap_targets` the
// trims use; `finish` commits ONE `EditorState::set_transition_dur`. CMX pins `at` to the cut, so the right edge gives
// `dur + delta` and the left `dur - delta` (dragging it left lengthens the transition, re-anchored at the cut), at least 1.

impl Drag {
    /// A press at `(x, y)` on the edge zone of a transition's widget: the edge drag (the transition selected); `None`
    /// anywhere else (the widget's middle stays the NLE3d click). The left zone wins where a narrow widget's zones overlap.
    pub fn begin_transition_edge(st: &mut EditorState, l: Layout, x: f64, y: f64, nosnap: bool) -> Option<Drag> {
        let (row, i) = l.transition_at(&st.timeline, x, y)?;
        let tr = *st.timeline.tracks.get(row)?.transitions.get(i)?;
        let (x0, _, x1, _) = l.transition_rect(row, &tr);
        let hw = HANDLE_W as f64;
        let kind = if x < x0 as f64 + hw {
            DragKind::TransitionIn
        } else if x >= x1 as f64 - hw {
            DragKind::TransitionOut
        } else {
            return None;
        };
        st.select_transition(row, i).ok()?;
        Some(Drag {
            kind,
            track: row,
            idx: i,
            origin_tick: l.tick_at(x),
            layout: l,
            delta: 0,
            to_track: row,
            origin: (x, y),
            at: (x, y),
            snap: !nosnap,
            additive: false,
            guide: None,
        })
    }
    /// The edge drag's ONE commit: the transition `(track, idx)` selected, then `set_transition_dur` of the pointer's length.
    fn finish_transition(&self, st: &mut EditorState) -> Result<(), EditError> {
        let x = st.timeline.tracks.get(self.track).and_then(|t| t.transitions.get(self.idx)).copied().ok_or(EditError::Transition)?;
        st.select_transition(self.track, self.idx)?;
        let dur = if self.kind == DragKind::TransitionIn { x.dur - self.delta } else { x.dur + self.delta };
        st.set_transition_dur(dur.max(1))
    }
    /// Snap the moving edge (`at` for the left, `at + dur` for the right) to the nearest clip edge or the playhead within
    /// [`SNAP_PX`], as a trim snaps its edge.
    fn snap_transition(&mut self, st: &EditorState, x: f64) {
        let Some(tr) = st.timeline.tracks.get(self.track).and_then(|t| t.transitions.get(self.idx)) else { return };
        let e = if self.kind == DragKind::TransitionIn { tr.at } else { tr.at + tr.dur };
        let ex = self.layout.x_of(e) as f64 + (x - self.origin.0);
        if let Some(t) = snap_to(&self.layout, &snap_targets(st, &[], true), ex) {
            self.delta = t - e;
            self.guide = Some(t);
        }
    }
}

// ---- STRIAINFOREELSTRIP (LEDGER SR747): the editor's message row — a band of its own between the preview and the ruler (named tail block) ----
// `Layout::of` has no labels input (the labels are the painter's `Text` hook and the face's), so the row is a builder the face
// calls once on the layout it paints: [`Layout::with_message`] takes [`MESSAGE_H`] px from the foot of the preview band. The ruler,
// the tracks and every hit stay where they were (a click on the row is `Hit::None`, as anything above `ruler_y`); `paint_in`'s
// BG fill already paints the row, so the painter is unchanged. Owed: NLE6-MESSAGE-KERNEL (the kernel Player paints Role::Tracks),
// STRIA-INFO-REEL-STRIP-CLICK (a click on the row shows the full summary when clipped).

/// STRIAINFOREELSTRIP (SR747): the message row's height (px); a preview band shorter than this gets no row.
pub const MESSAGE_H: u32 = 14;

impl Layout {
    /// STRIAINFOREELSTRIP (SR747): this layout with the message row when `on` and the preview band holds it: the band shrinks by
    /// [`MESSAGE_H`] and the row is `message_y..message_y + message_h` under it. Off, a band under [`MESSAGE_H`] (Role::Tracks has
    /// none) or a row already cut: unchanged.
    pub fn with_message(mut self, on: bool) -> Layout {
        if on && self.message_h == 0 && self.preview_h >= MESSAGE_H {
            self.preview_h -= MESSAGE_H;
            self.message_y = self.preview_h;
            self.message_h = MESSAGE_H;
        }
        self
    }
}

#[cfg(test)]
mod message_row_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Timeline};

    fn two_tracks() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("ROWA", 40);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        }
        EditorState::new(tl)
    }

    /// STRIAINFOREELSTRIP (SR747) M3: the row comes out of the preview band only (ruler, tracks, page unchanged); off, on the
    /// tracks role and on a band under MESSAGE_H the layout is today's, field for field; Preview's row is at its foot; idempotent.
    #[test]
    fn message_row_is_taken_from_the_preview_band() {
        let st = two_tracks();
        let l = Layout::of(320, 160, &st);
        assert_eq!((l.preview_h, l.ruler_y, l.tracks_y, l.message_y, l.message_h), (64, 64, 80, 0, 0));
        let m = l.with_message(true);
        assert_eq!((m.preview_h, m.message_y, m.message_h), (50, 50, MESSAGE_H));
        assert_eq!((m.w, m.h, m.ruler_y, m.tracks_y, m.rows, m.zoom, m.t0), (l.w, l.h, l.ruler_y, l.tracks_y, l.rows, l.zoom, l.t0));
        assert_eq!(m.with_message(true), m, "idempotent");
        assert_eq!(l.with_message(false), l, "labels off: today's layout");
        let t = Layout::of_role(Role::Tracks, 320, 96, &st);
        assert_eq!(t.with_message(true), t, "the tracks role has no preview band");
        assert_eq!((t.preview_h, t.ruler_y, t.tracks_y, t.message_y, t.message_h), (0, 0, RULER_H, 0, 0));
        let small = Layout::of(320, 100, &st);
        assert_eq!(small.with_message(true), small, "a band under MESSAGE_H");
        let p = Layout::of_role(Role::Preview, 320, 90, &st).with_message(true);
        assert_eq!((p.preview_h, p.message_y, p.message_h, p.ruler_y), (76, 76, MESSAGE_H, 90 + RULER_H));
        std::println!("striainforeelstrip: layout whole+labels preview_h={} message={}+{} ruler_y={}", m.preview_h, m.message_y, m.message_h, m.ruler_y);
    }

    /// STRIAINFOREELSTRIP (SR747) M3: a click on the row is nothing; the painted row is the view's BG, the preview above it FIELD,
    /// and everything from the ruler down is byte-identical to the row-less paint.
    #[test]
    fn message_row_paints_bg_and_hits_nothing() {
        let st = two_tracks();
        let l = Layout::of(320, 160, &st);
        let m = l.with_message(true);
        assert_eq!(m.hit(&st.timeline, 10.0, 55.0), Hit::None);
        assert_eq!(m.hit(&st.timeline, 10.0, 70.0), l.hit(&st.timeline, 10.0, 70.0));
        assert_eq!(m.hit(&st.timeline, 10.0, 100.0), l.hit(&st.timeline, 10.0, 100.0));
        let (a, b) = (paint_in(&st, l, Order::Rgba, None, None), paint_in(&st, m, Order::Rgba, None, None));
        let at = |px: &[u8], x: usize, y: usize| [px[(y * 320 + x) * 4], px[(y * 320 + x) * 4 + 1], px[(y * 320 + x) * 4 + 2], px[(y * 320 + x) * 4 + 3]];
        assert_eq!(at(&b, 10, 49), FIELD);
        assert_eq!(at(&b, 10, 50), BG);
        assert_eq!(at(&b, 300, 63), BG);
        assert_eq!(at(&a, 10, 50), FIELD, "row-less: the preview reaches the ruler");
        let r = m.ruler_y as usize * 320 * 4;
        assert_eq!(&a[r..], &b[r..], "the ruler and the tracks are unchanged");
        assert_eq!(&a[..m.message_y as usize * 320 * 4], &b[..m.message_y as usize * 320 * 4]);
    }
}

// ---- NLE3ESLIPDRAG (LEDGER SR760): the slip modifier turns the body hit into a slip, the trim-handle hit into a slide (named tail block) ----

#[cfg(test)]
mod slipdrag_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLEFXA 0..20 at 0 and NLEFXA 20..30 at 20 (butted), a 40-tick source, the view band `Role::Tracks` 320x56.
    fn slipdrag_fixture() -> (Timeline, EditorState, Layout, f64, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(a, 20, 30, 20)];
        let st = EditorState::new(tl.clone());
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        (tl, st, l, z, y)
    }

    /// NLE3ESLIPDRAG (SR760) M3: a slip press on the second clip's body dragged +3 ticks is `Timeline::slip(0, 1, 3)`, the
    /// twin of the editor's `slip 3` word (one commit); a slip never snaps (no guide); the preview (`Drag::shown`) is the
    /// slipped timeline while the committed one is unchanged. The same press without the modifier is the Move it was.
    #[test]
    fn slip_press_on_the_body_is_the_slip_word() {
        let (tl, mut st, l, z, y) = slipdrag_fixture();
        let x = l.x_of(25) as f64 + z / 2.0;
        let mut plain = st.clone();
        assert_eq!(Drag::begin_with(&mut plain, l, x, y, false, false, false).map(|d| d.kind), Some(DragKind::Move));
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, true).unwrap();
        assert_eq!((d.kind, d.track, d.idx, d.origin_tick), (DragKind::Slip, 0, 1, 25));
        d.update(&st, x + 3.0 * z, y);
        assert_eq!((d.delta, d.guide), (3, None));
        let want = tl.slip(0, 1, 3).unwrap();
        assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()), "the preview slips");
        assert_eq!(st.timeline, tl, "nothing committed before the release");
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, want);
        let mut twin = EditorState::new(tl.clone());
        twin.select(0, 1).unwrap();
        assert_eq!(twin.verb("slip 3"), Some(Ok(())));
        assert_eq!(st.timeline, twin.timeline);
        assert_eq!(st.undo_depth(), 1);
    }

    /// NLE3ESLIPDRAG (SR760) M3: with the second clip selected, a slip press on its in handle dragged -5 ticks is
    /// `Timeline::slide(0, 1, -5)` (the butted first clip's out edge follows), the twin of `slide -5`; snapping is on and
    /// finds no target (the clip and its butted neighbour are excluded). Without the modifier the same press is TrimIn.
    #[test]
    fn slip_press_on_the_handle_is_the_slide_word() {
        let (tl, mut st, l, z, y) = slipdrag_fixture();
        st.select(0, 1).unwrap();
        let x = l.x_of(20) as f64 + 1.0;
        let mut plain = st.clone();
        assert_eq!(Drag::begin_with(&mut plain, l, x, y, false, false, false).map(|d| d.kind), Some(DragKind::TrimIn));
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, true).unwrap();
        assert_eq!((d.kind, d.idx, d.origin_tick, d.snap), (DragKind::Slide, 1, 20, true));
        d.update(&st, x - 5.0 * z, y);
        assert_eq!((d.delta, d.guide), (-5, None));
        assert_eq!(d.finish(&mut st), Ok(()));
        let want = tl.slide(0, 1, -5).unwrap();
        assert_eq!(st.timeline, want);
        assert_eq!(st.timeline.tracks[0].clips[0].end(), 15, "the neighbour follows");
        let mut twin = EditorState::new(tl.clone());
        twin.select(0, 1).unwrap();
        assert_eq!(twin.verb("slide -5"), Some(Ok(())));
        assert_eq!(st.timeline, twin.timeline);
    }

    /// NLE3ESLIPDRAG (SR760) M3: a slip past the source's end is refused by name (`OutOfSource`) and changes nothing; the
    /// preview of a refused slip is None (the committed state shows).
    #[test]
    fn slip_drag_past_the_source_is_refused() {
        let (tl, mut st, l, z, y) = slipdrag_fixture();
        let x = l.x_of(25) as f64 + z / 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, true).unwrap();
        d.update(&st, x + 11.0 * z, y);
        assert_eq!(d.delta, 11);
        assert!(d.shown(&st).is_none());
        assert_eq!(d.finish(&mut st), Err(EditError::OutOfSource));
        assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
    }
}

// ---- STRIAREVERSEEDIT (LEDGER SR9094): a reversed clip body carries a small badge in the band (named tail block) ----
// A "<<" (8x7, LABEL ink) at the body's BOTTOM-RIGHT corner, inside the out handle (HANDLE_W + 2 px in, 4 px up), so the
// label (baseline y0 + 15), the selection handles and the marks / curves siblings paint on the body stay clear. Painted
// after the transitions; a body narrower than the badge plus both handles gets none. No reversed clip: nothing painted, so
// every golden stays byte-identical. The kernel Player paints its band through THE `paint_in`, so it shows the same badge.

/// STRIAREVERSEEDIT: the badge's rows, bit 7 the leftmost pixel.
const REVERSE_BADGE: [u8; 7] = [0x11, 0x33, 0x77, 0xff, 0x77, 0x33, 0x11];

/// STRIAREVERSEEDIT: the badge box `(x0, y0, x1, y1)` of a clip body spanning `x0..x1` on a row starting at `y0`, or
/// `None` when the body is too narrow.
fn reverse_badge_box(x0: i64, x1: i64, y0: i64) -> Option<(i64, i64, i64, i64)> {
    if x1 - x0 < 8 + 2 * HANDLE_W + 6 {
        return None;
    }
    let (bx1, by1) = (x1 - 1 - HANDLE_W - 2, y0 + ROW_H as i64 - 4);
    Some((bx1 - 8, by1 - 7, bx1, by1))
}

/// STRIAREVERSEEDIT: paint the badge on every visible reversed clip body (none reversed: no pixel touched).
fn paint_reverse_badges(c: &mut Canvas, l: Layout, st: &EditorState) {
    for (row, tr) in st.timeline.tracks.iter().enumerate().take(l.rows) {
        for clip in tr.clips.iter().filter(|k| k.reverse) {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 <= 0 || x0 >= l.w as i64 {
                continue;
            }
            let Some((bx0, by0, _, _)) = reverse_badge_box(x0, x1, l.row_y(row)) else { continue };
            for (dy, bits) in REVERSE_BADGE.iter().enumerate() {
                for dx in 0..8i64 {
                    if bits & (0x80 >> dx) != 0 {
                        let (px, py) = (bx0 + dx, by0 + dy as i64);
                        c.rect(px, py, px + 1, py + 1, LABEL);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod striareverseedit_tests {
    use super::*;
    use crate::Clip;

    fn state(reverse: bool) -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 10, 20, 0).reversed(reverse)];
        EditorState::new(tl)
    }

    #[test]
    fn striareverseedit_badge_only_on_a_reversed_body_inside_its_box() {
        let (w, h) = (320, 120);
        let fwd = paint(&state(false), w, h, Order::Rgba, None, None);
        assert_eq!(fwd, paint(&state(false), w, h, Order::Rgba, None, None));
        let st = state(true);
        let rev = paint(&st, w, h, Order::Rgba, None, None);
        let l = Layout::of(w, h, &st);
        let c = st.timeline.tracks[0].clips[0];
        let (bx0, by0, bx1, by1) = reverse_badge_box(l.x_of(c.at), l.x_of(c.end()), l.row_y(0)).unwrap();
        let mut changed = 0;
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let i = ((y * w as i64 + x) * 4) as usize;
                if fwd[i..i + 4] != rev[i..i + 4] {
                    assert!(bx0 <= x && x < bx1 && by0 <= y && y < by1, "pixel ({x}, {y}) outside the badge box");
                    assert_eq!(&rev[i..i + 4], &LABEL[..]);
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, REVERSE_BADGE.iter().map(|b| b.count_ones()).sum::<u32>() as i32, "the whole badge, nothing else");
        assert_eq!(reverse_badge_box(0, 8, 0), None, "a narrow body gets none");
        extern crate std;
        std::println!("striareverseedit: badge box {:?}, {changed} px", (bx0, by0, bx1, by1));
    }
}
