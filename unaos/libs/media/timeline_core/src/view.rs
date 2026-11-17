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
    pub squeeze: u32, // STRIAZOOMFITTHREE (SR9040): ticks per `zoom` px, the editor state's (1: one tick per `zoom` px) (file tail)
    pub t0: i64,
    pub message_y: u32, // STRIAINFOREELSTRIP (SR747): the message row's top (0 with no row: [`Layout::with_message`])
    pub message_h: u32, // STRIAINFOREELSTRIP (SR747): the message row's height (0: no row)
    pub left: u32, // STRIATRACKSTHREE (SR9038): the header column's width (0: none; [`Layout::with_headers`])
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
        let zoom = st.zoom.max(1); let squeeze = st.squeeze.max(1); // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
        let per_page = (w as i64 * squeeze as i64 / zoom as i64).max(1); // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
        let t0 = st.playhead.max(0) / per_page * per_page;
        Layout { w, h, preview_h, ruler_y: preview_h, tracks_y: preview_h + RULER_H, rows, zoom, squeeze, t0, message_y: 0, message_h: 0, left: 0 }
    }
    /// The x of tick `t`'s left edge.
    pub fn x_of(&self, t: i64) -> i64 {
        (self.left as i64 + (t - self.t0) * self.zoom as i64).div_euclid(self.squeeze.max(1) as i64) // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
    }
    /// The tick under x.
    pub fn tick_at(&self, x: f64) -> i64 {
        self.t0 + ((x - self.left as f64).max(0.0) as i64) * self.squeeze.max(1) as i64 / self.zoom as i64 // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
    }
    /// The y of track row `row`'s top.
    pub fn row_y(&self, row: usize) -> i64 {
        self.tracks_y as i64 + row as i64 * ROW_H as i64
    }
    /// The centre of tick `t` on track row `row` (what a click on that clip sends).
    pub fn point(&self, row: usize, t: i64) -> (f64, f64) {
        (self.x_of(t) as f64 + self.zoom as f64 / (2.0 * self.squeeze.max(1) as f64), self.row_y(row) as f64 + ROW_H as f64 / 2.0)
    }
    pub fn hit(&self, tl: &Timeline, x: f64, y: f64) -> Hit {
        if x < 0.0 || y < 0.0 || x >= self.w as f64 || y >= self.h as f64 || y < self.ruler_y as f64 || x < self.left as f64 {
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
    Bin, // STRIAPROJECTBIN (SR885): the source bin (`paint_bin` / `bin_hit`, its own row geometry: no band of the track view)
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
            Role::Bin => { l.preview_h = 0; l.ruler_y = h + RULER_H; l.tracks_y = h + 2 * RULER_H; } // STRIAPROJECTBIN (SR885): no band of the track view on a bin Surface
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
        self.fill_rows(x0, x1, y0, y1, c); // STRIAPERFCANVASFILL (SR919): one row filled, then copied down (file tail)
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
    paint_in(st, Layout::of(w, h, st), order, labels, preview, None)
}

/// [`paint`] in a given layout (a face's view, which a wheel zoom anchors at the pointer, or a Surface's role).
pub fn paint_in(st: &EditorState, l: Layout, order: Order, mut labels: Option<&mut dyn Text>, preview: Option<&Still>, peaks: Option<&dyn PeakSource>) -> Vec<u8> {
    let (w, h) = (l.w, l.h);
    let tl = &st.timeline;
    let mut c = Canvas::filled(w, h, order, BG); // STRIAPERFCANVASFILL (SR919): BG in one pass
    // The preview band.
    c.rect(0, 0, w as i64, l.preview_h as i64, FIELD);
    if let Some(s) = preview {
        c.blit_fit(s, 0, 0, w, l.preview_h);
    }
    // The ruler: a short mark per tick when they are 4 px apart, a long one per second.
    let fps = tl.fps().or(edl::ntsc_nominal(tl.tb)).unwrap_or(25).max(1) as i64; // STRIATCTHREE (SR9030): an NTSC base rules at its nominal (file tail)
    let (ry, ty) = (l.ruler_y as i64, l.tracks_y as i64); let mut tc = alloc::string::String::new(); // STRIAPERFTHREE (SR9037): one ruler timecode buffer (edl.rs tail)
    c.rect(0, ry, w as i64, ty, RULER);
    let t_end = if l.band_on(ry, ty) { l.tick_at(w as f64) + 1 } else { l.t0 - 1 }; // STRIAPERFCANVASFILL (SR919): an off-canvas ruler runs no tick
    for t in l.ruler_ticks(t_end, fps) { // STRIAZOOMFITTHREE (SR9040): every tick, or whole seconds when squeezed (file tail)
        let x = l.x_of(t);
        if ruler_second(tl, t, fps) { // STRIATCTHREE (SR9030): a drop-frame base marks where the label's second changes (file tail)
            c.rect(x, ry + RULER_H as i64 / 2, x + 1, ty, TICK);
            if let Some(text) = labels.as_mut().filter(|_| l.zoom as i64 * fps >= 60 * l.squeeze.max(1) as i64) {
                text.text(&mut c, ruler_tctimecode_in(tl, &mut tc, t, fps), x as f32 + 2.0, ry as f32 + 9.0, (x as i32, w as i32)); // STRIATCTHREE (SR9030): drop-frame labels on a drop-frame base (file tail)
            }
        } else if l.zoom >= 4 {
            c.rect(x, ty - 3, x + 1, ty, TICK);
        }
    }
    paint_markers(&mut c, l, tl, &mut labels); // The tracks and their clips.
    for (row, track) in tl.tracks.iter().enumerate() {
        let y0 = l.row_y(row);
        let y1 = y0 + ROW_H as i64; if !l.band_on(y0, y1) { continue; } // STRIAPERFCANVASFILL (SR919): an off-canvas row paints and shapes nothing
        c.rect(0, y0, w as i64, y1, ROW[row % 2]);
        for clip in &track.clips {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 <= 0 || x0 >= w as i64 {
                continue;
            }
            c.rect(x0 + 1, y0 + 2, x1 - 1, y1 - 2, opacity_colour(clip_colour(clip.src.0, track.kind), ROW[row % 2], clip.opacity));
            if let Some(p) = peaks.filter(|_| track.kind == Kind::Audio) { paint_peaks(&mut c, &l, (x0 + 1, y0 + 2, x1 - 1, y1 - 2), clip, tl.tb, p); } // STRIAWAVEFORM (SR827): the clip's peaks over its body (file tail)
            if let Some(p) = peaks.filter(|_| track.kind == Kind::Video) { paint_strip(&mut c, &l, (x0 + 1, y0 + 2, x1 - 1, y1 - 2), clip, p); } // STRIATHUMBNAILS (SR839): the clip's src_in frame at its body's left; STRIATHUMBNAILSMORE (SR949): a wide body's filmstrip and tail (file tail) // STRIAOPACITYFACE (SR898): the body at its opacity (file tail) pan_tick(&mut c, track.kind, clip, x0, x1, y0); // STRIAPANFACE (SR897): the audio clip's pan tick, nothing at centre (file tail) paint_crop_badge(&mut c, clip.crop, x0, x1, y0); // STRIACROPFACE (SR900): a cropped clip's badge (file tail)
            if let (Some(text), Some(src)) = (labels.as_mut(), tl.source(clip.src)) {
                let label = if src.size_policy == crate::SizePolicy::Letterbox { src.reel.clone() } else { alloc::format!("{} {}", src.reel, src.size_policy.name()) }; // NLESIZEPOLICYUI (SR402): the reel's policy as a token
                text.text(&mut c, &opacity_label(label, clip.opacity), x0 as f32 + 4.0, y0 as f32 + 15.0, ((x0 + 1) as i32, (x1 - 1) as i32)); // STRIAOPACITYMORE (SR958): the label names a non-opaque clip's percent (file tail)
            }
            paint_gain_fades(&mut c, &l, (x0 + 1, y0 + 2, x1 - 1, y1 - 2), clip); // STRIACLIPGAINFACE (SR838): the gain line and fade ramps over the body (file tail)
        }
    }
    paint_track_flags(&mut c, l, tl); paint_track_gains(&mut c, l, tl); // STRIATRACKMUTESOLO (SR837): the inactive rows dimmed, the M / S badges (file tail); STRIATRACKGAINSTEP (SR901): the track gain readout badge (file tail)
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
    paint_grade_badges(&mut c, l, st); paint_links(&mut c, l, st); paint_transitions(&mut c, l, st); paint_audio_xfades(&mut c, l, st, &mut labels); // STRIAAUDIOCROSSFADE (SR860): the explicit audio crossfades' brackets (file tail) // STRIAGRADEFACE (SR899): a graded clip's badge, nothing at identity (file tail) // STRIALINKTRIM (SR910): the chain mark on linked clips (file tail) paint_headers(&mut c, l, st, labels.as_deref_mut()); // STRIATRACKSTHREE (SR9038): the header column, nothing while `left` is 0 (file tail)
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
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
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
        let px = paint_in(st, l, Order::Rgba, Some(&mut rec as &mut dyn Text), None, None);
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
    Pan, // STRIAPANNEXT (SR994): the selected AUDIO clip's pan grip (`EditorState::set_selected_pan`; `delta` is the pan change in cq8, not ticks) (file tail) XfadeIn, XfadeOut, // STRIAXFADETHREE (SR9023): the explicit audio crossfade bracket's left / right edge (file tail)
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
    pub additive: bool, pub toggle: bool,
    /// NLE2c: the tick the delta snapped to (painted as the guide line).
    pub guide: Option<i64>,
    /// STRIARIPPLETRIM (SR850): a trim begun with shift on the primary's handle ripples (`EditorState::ripple_trim`).
    pub ripple: bool,
    pub roll: bool, // STRIAROLLMORE (SR954): the face's roll tool turned this trim into a roll of its handle's cut (`EditorState::roll_selected`; set by [`Drag::rolled`]) pub ripple_all: bool, // STRIARIPPLETHREE (SR9034): a ripple begun with the face's ripple-all tool on runs on every track (`EditorState::ripple_trim_all`, file tail)
}

impl Drag {
    /// A press at `(x, y)` with no modifier ([`Drag::begin_with`]).
    pub fn begin(st: &mut EditorState, l: Layout, x: f64, y: f64) -> Option<Drag> {
        Drag::begin_with(st, l, x, y, false, false, false)
    }
    /// A press at `(x, y)`: on the primary's drawn trim handle (the rects [`paint`] draws) a trim; on a clip body a move
    /// (a clip of the set keeps the set and becomes its primary, `shift` adds the clip, else it alone is selected); on
    /// an empty track spot a marquee; on the ruler a scrub; anywhere else no drag. NLE3ESLIPDRAG (SR760): with `slip` the
    /// same hits begin a slide (the trim handle) or a slip (the clip body) instead. STRIARIPPLETRIM (SR850): with `shift` (and not `slip`) the trim handle begins a ripple trim (`ripple`).
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
            additive: shift, toggle: false,
            guide: None,
            ripple: false, roll: false, // STRIAROLLMORE (SR954) ripple_all: false,
        };
        if let (Some((track, idx)), Some(c), false) = (st.selection, st.selected(), shift && slip) {
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
                    return Some(Drag { ripple: shift, ..mk(if slip { DragKind::Slide } else { kind }, track, idx) });
                }
            }
        }
        if let Some((t, i)) = pan_grip(st, &l, x, y, shift, slip) { return Some(mk(DragKind::Pan, t, i)); } match l.hit(&st.timeline, x, y) { // STRIAPANNEXT (SR994): the pan grip over the tick's strip (file tail)
            Hit::Clip(track, idx, _) => { let toggle = shift && st.set.contains(&(track, idx)); // STRIAMULTISELECTMORE (SR950): a shift-press on a member toggles it out if released still (file tail)
                if toggle {} else if shift || st.set.contains(&(track, idx)) { // STRIAMULTISELECTNEXT (SR998): a member's shift-press keeps the primary until the drag commits (`finish`)
                    st.select_add(track, idx).ok()?;
                } else {
                    st.select(track, idx).ok()?;
                }
                Some(Drag { toggle, ..mk(if slip { DragKind::Slip } else { DragKind::Move }, track, idx) })
            }
            Hit::Track(track, _) => Some(mk(DragKind::Marquee, track, 0)),
            Hit::Ruler(_) => Some(mk(DragKind::Scrub, 0, 0)),
            Hit::None => None,
        }
    }
    /// The pointer is at `(x, y)`: the tick delta (snapped), and for a move of one clip the row under it when that
    /// track's kind matches.
    pub fn update(&mut self, st: &EditorState, x: f64, y: f64) {
        if self.kind == DragKind::Pan { return self.pan_update(st, x, y); } self.at = (x, y); // STRIAPANNEXT (SR994): a pan drag reads the pointer as pan (file tail)
        self.delta = self.layout.tick_at(x) - self.origin_tick;
        self.guide = None;
        if self.snap {
            self.snap_delta(st, x);
        }
        if self.kind != DragKind::Move { // STRIAMULTISELECTMORE (SR950): a set follows the row too (`move_set_track`)
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
    fn finish_unnamed(&self, st: &mut EditorState) -> Result<(), EditError> {
        match self.kind {
            DragKind::Marquee => {
                let (x0, y0, x1, y1) = (self.origin.0, self.origin.1, self.at.0, self.at.1);
                st.select_many(&self.layout.crossing(&st.timeline, x0, y0, x1, y1), self.additive);
                return Ok(());
            }
            DragKind::Scrub => return Ok(()), DragKind::TransitionIn | DragKind::TransitionOut => return self.finish_transition(st), DragKind::XfadeIn | DragKind::XfadeOut => return self.finish_xfade(st),
            _ => {}
        }
        if self.toggle { st.select_add(self.track, self.idx)?; } else if st.selection != Some((self.track, self.idx)) { // STRIAMULTISELECTNEXT (SR998): a shift-pressed member becomes primary only now, the set kept
            st.select(self.track, self.idx)?;
        }
        match self.kind {
            DragKind::TrimOut if self.roll => st.roll_selected(self.delta), // STRIAROLLMORE (SR954): the roll tool's drag is ONE roll of the cut at the clip's out edge
            DragKind::TrimIn | DragKind::TrimOut if self.ripple && self.ripple_all => st.ripple_trim_all(if self.kind == DragKind::TrimIn { Edge::In } else { Edge::Out }, self.delta), DragKind::TrimIn | DragKind::TrimOut if self.ripple => st.ripple_trim(if self.kind == DragKind::TrimIn { Edge::In } else { Edge::Out }, self.delta), // STRIARIPPLETHREE (SR9034): the ripple-all tool's drag is the one all-tracks op (file tail)
            DragKind::TrimIn => st.trim_by_ticks(Edge::In, self.delta), // STRIACLIPSPEEDMORE (SR951): the timeline delta through src_at (1x: the same source delta)
            DragKind::TrimOut => st.trim_by_ticks(Edge::Out, self.delta), // STRIACLIPSPEEDMORE (SR951): the timeline delta through src_at (1x: the same source delta)
            DragKind::Slip => st.slip_selected(self.delta),
            DragKind::Slide => st.slide_selected(self.delta), DragKind::Pan => self.finish_pan(st), // STRIAPANNEXT (SR994): the one pan road (file tail)
            DragKind::Move if st.set.len() > 1 => st.move_set_track(self.to_track as isize - self.track as isize, self.delta), // STRIAMULTISELECTMORE (SR950)
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
        s.zoom = st.zoom; s.squeeze = st.squeeze; // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
        self.finish(&mut s).ok()?;
        Some(s)
    }
}

/// How near (px, at the current zoom) a moving edge must come to a clip edge or the playhead to snap to it.
pub const SNAP_PX: f64 = 6.0;
/// How far (px) the pointer must travel before a press on an empty track is a marquee rather than a click.
pub const MARQUEE_MIN: f64 = 3.0;

/// Where a moving edge may snap: every clip's `at` and `end` on any track except the `exclude`d clips', plus the
/// playhead when `playhead`; sorted, deduped; STRIASNAPDRAG (SR836): tick 0 and the set in / out marks.
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
    v.extend(fixed_targets(&st.timeline)); // STRIASNAPDRAG (SR836): tick 0 and the set in / out marks (file tail)
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
            DragKind::Marquee | DragKind::Slip => return, DragKind::TransitionIn | DragKind::TransitionOut => return self.snap_transition(st, x), DragKind::TrimIn | DragKind::TrimOut if self.ripple => return self.snap_ripple(st, x), // STRIARIPPLEMORE (SR953): a ripple snaps the edge that moves (file tail) DragKind::XfadeIn | DragKind::XfadeOut => return self.snap_xfade(st, x),
            DragKind::Scrub => {
                if let Some(t) = snap_to(&l, &snap_targets(st, &[], false), x) {
                    self.delta = t - self.origin_tick;
                    self.guide = Some(t);
                }
                return;
            }
            DragKind::TrimIn | DragKind::TrimOut => {
                let Some(c) = st.timeline.tracks.get(self.track).and_then(|tr| tr.clips.get(self.idx)) else { return };
                { slide_ex = self.trim_exclude(); (vec![if self.kind == DragKind::TrimIn { c.at } else { c.end() }], &slide_ex, true) } // STRIAROLLMORE (SR954): a roll also excludes the butted next clip
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
        let link_ex = link_exclude(st, self.kind, self.track, self.idx, exclude); let targets = snap_targets(st, &link_ex, playhead); // STRIALINKSLIDE (SR927): a trim or slide drag never snaps to its link mates' own edges (file tail)
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
        let before = paint_in(&st, l, Order::Rgba, None, None, None);
        assert_eq!(at(&before, l.x_of(25), yi), ROW[0], "committed: nothing at tick 25");
        let shown = d.shown(&st).unwrap();
        let px = paint_drag(paint_in(&shown, l, Order::Rgba, None, None, None), l, Order::Rgba, Some(&d));
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
            additive: false, toggle: false,
            guide: None,
            ripple: false, roll: false, // STRIAROLLMORE (SR954)
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
        let (a, b) = (paint_in(&st, l, Order::Rgba, None, None, None), paint_in(&st, m, Order::Rgba, None, None, None));
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

// ---- STRIAWAVEFORM (LEDGER SR827): an audio clip's peaks in its body (named tail block) ----
// An audio clip's body shows its waveform: a per-source PEAK TABLE ([`Peaks`], one u8 per bucket: max |sample| over the
// bucket, every channel folded by max) reduced by THE ONE [`peaks_of`], painted by THE ONE [`paint_peaks`] from
// [`paint_in`]'s clip-body arm through the optional [`PeakSource`] hook (the [`Text`] shape). The bars are the audible
// result: the clip gain through the hook's `gain_q16` (the mix's ONE curve, audio_core `ramp::gain_q16`, which this core
// does not link: the face answers it) and the fades through `schedule::env_q16` (the mix's envelope). No hook, no table or
// an empty table: the body exactly as before (every golden byte-identical). The table is the face's (the EDL carries no
// pixels); Stria's `nle::scan_peaks` builds it through the render's own audio decoder. Owed: NLE-WAVEFORM-STEREO,
// NLE-WAVEFORM-ZOOM-LOD, NLE-WAVEFORM-KERNEL, NLE-WAVEFORM-CACHE-FILE, NLE-WAVEFORM-ENV-SUBTICK, NLE-WAVEFORM-SCAN-THREAD.

/// The peak table's resolution: buckets per source second (a 10 ms bucket; an hour-long source is 360 000 bytes).
pub const PEAKS_PER_SEC: u32 = 100;
/// The waveform's ink: the audio clip's body colour lifted by this much per channel (saturating), the body under it.
pub const WAVE_LIFT: u8 = 0x30;

/// One source's peak table: `buckets[b]` is the max |sample| (0..=255 for 0.0..=1.0) over source time
/// `b / per_sec .. (b + 1) / per_sec` s.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Peaks {
    pub per_sec: u32,
    pub buckets: Vec<u8>, pub chans: Vec<Peaks>, pub lod: Vec<Vec<u8>>, // STRIAWAVEFORMSTEREO (SR9051): a stereo source's L / R tables (else empty) and the max pyramid `finish` builds (file tail)
}

/// The waveform hook: a source's peak table (none: the body stays flat) and the clip gain's Q16 factor by the mix's ONE
/// curve (audio_core `ramp::gain_q16`; the face that links it answers).
pub trait PeakSource {
    fn peaks(&self, src: crate::SourceId) -> Option<&Peaks>;
    fn gain_q16(&self, cdb: i32) -> i64;
    /// STRIATHUMBNAILS (SR839): the head still of `src` at source tick `src_tick` ([`head_still`]-sized; none: no head).
    fn thumb(&self, _src: crate::SourceId, _src_tick: i64) -> Option<&Still> { None }
}

/// THE ONE reduction: `interleaved` (`channels` per frame at `rate` Hz) to `ceil(frames * per_sec / rate)` buckets, bucket
/// `b` the frames `floor(b * rate / per_sec) .. floor((b + 1) * rate / per_sec)` (so whole seconds fed one after another
/// give the same bytes as one call), its value `round(min(max |x|, 1) * 255)` over every channel (NaN ignored).
/// `rate` or `per_sec` 0: no buckets.
pub fn peaks_of(rate: u32, channels: u16, interleaved: &[f32], per_sec: u32) -> Peaks {
    let ch = channels.max(1) as usize;
    let frames = interleaved.len() / ch;
    if rate == 0 || per_sec == 0 {
        return Peaks { per_sec, buckets: Vec::new(), chans: Vec::new(), lod: Vec::new() };
    }
    let (r, p) = (rate as u64, per_sec as u64);
    let n = (frames as u64 * p).div_ceil(r) as usize;
    let mut buckets = Vec::with_capacity(n);
    for b in 0..n as u64 {
        let (f0, f1) = ((b * r / p) as usize, (((b + 1) * r / p) as usize).min(frames));
        let m = interleaved[f0 * ch..f1 * ch].iter().fold(0.0f32, |m, &x| {
            let a = if x < 0.0 { -x } else { x };
            if a > m { a } else { m }
        });
        buckets.push((m.min(1.0) * 255.0 + 0.5) as u8);
    }
    Peaks { per_sec, buckets, chans: sides_of(rate, channels, interleaved, per_sec), lod: Vec::new() } // STRIAWAVEFORMSTEREO (SR9051): each channel of a stereo source through this same reduction (file tail)
}

/// THE ONE waveform painter: inside `body` (`x0, y0, x1, y1`, the clip body rect [`paint_in`] filled) a vertical bar per
/// canvas column, centred on the body's midline: the max of the buckets the column covers ([`Layout::x_of`] inverted at
/// sub-tick precision, source tick = `src_in + (t - at)`, seconds through `tb`), x the clip gain (`src.gain_q16`) x the
/// fade envelope at the column's tick (`schedule::env_q16`, the mix's), half-height `round(v * (body_h / 2) / 255)`,
/// inked in the audio clip colour + [`WAVE_LIFT`]. No table for the clip's source (or an empty one): nothing painted.
pub fn paint_peaks(c: &mut Canvas, l: &Layout, body: (i64, i64, i64, i64), clip: &crate::Clip, tb: crate::Timebase, src: &dyn PeakSource) {
    let Some(p) = src.peaks(clip.src) else { return }; if p.chans.len() == 2 { return paint_sides(c, l, body, clip, tb, src, p); } // STRIAWAVEFORMSTEREO (SR9051): a stereo table paints L over R, each in half the body (file tail)
    if p.buckets.is_empty() || p.per_sec == 0 {
        return;
    }
    let (x0, y0, x1, y1) = body;
    let half = (y1 - y0) / 2;
    if half <= 0 {
        return;
    }
    let mid = y0 + half;
    let g = src.gain_q16(clip.gain_cdb);
    let base = clip_colour(clip.src.0, Kind::Audio);
    let ink = [base[0].saturating_add(WAVE_LIFT), base[1].saturating_add(WAVE_LIFT), base[2].saturating_add(WAVE_LIFT), 255];
    let z = l.zoom.max(1) as i64;
    let at_x = l.x_of(clip.at);
    let (k, d) = (tb.num as i128 * p.per_sec as i128, tb.den.max(1) as i128 * z as i128);
    let bucket = |dx: i64| ((clip.src_in as i128 * z as i128 + dx as i128) * k).div_euclid(d);
    let len = p.buckets.len() as i128;
    for x in x0.max(0)..x1.min(c.w as i64) {
        let dx = x - at_x;
        let b0 = bucket(dx);
        let b1 = bucket(dx + 1).max(b0 + 1);
        let (b0, b1) = (b0.clamp(0, len) as usize, b1.clamp(0, len) as usize);
        if b0 >= b1 {
            continue;
        }
        let peak = p.range_max(b0, b1) as i64; // STRIAWAVEFORMSTEREO (SR9051): the pyramid's O(log n) max, the same value as the linear one (file tail)
        let e = env_sub_q16(clip, dx.max(0), z) as i64; // STRIATHUMBNAILSTHREE (SR9044): the mix's envelope at the column, below a tick (file tail)
        let v = ((((peak * g) >> 16) * e) >> 16).clamp(0, 255);
        let hh = (v * half + 127) / 255;
        if hh > 0 {
            c.rect(x, mid - hh, x + 1, mid + hh, ink);
        }
    }
}

#[cfg(test)]
mod waveform_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};

    /// A test hook: one table for every source; the gain by ramp.rs's formula (`round(10^(cdb / 2000) * 65536)`).
    struct Fixed(Peaks);
    impl PeakSource for Fixed {
        fn peaks(&self, _src: SourceId) -> Option<&Peaks> {
            Some(&self.0)
        }
        fn gain_q16(&self, cdb: i32) -> i64 {
            if cdb == 0 { 65536 } else { libm::round(libm::pow(10.0, cdb as f64 / 2000.0) * 65536.0) as i64 }
        }
    }

    /// 25 fps, one track of `kind` holding `clip` (a 2-s source), the playhead at 0.
    fn one_clip(kind: Kind, clip: Clip) -> EditorState {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(kind);
        tl.tracks[0].clips = alloc::vec![clip];
        EditorState::new(tl)
    }

    fn px(buf: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// The clip body rect of row 0 (x clipped to the canvas, the playhead column 0 left out).
    fn body(l: &Layout) -> (i64, i64, i64, i64) {
        let y0 = l.row_y(0);
        (1, y0 + 2, l.w as i64, y0 + ROW_H as i64 - 2)
    }

    fn colours(buf: &[u8], l: &Layout) -> usize {
        let (x0, y0, x1, y1) = body(l);
        let mut seen: Vec<[u8; 4]> = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let c = px(buf, l.w, x, y);
                if !seen.contains(&c) {
                    seen.push(c);
                }
            }
        }
        seen.len()
    }

    /// The inked pixels of column `x` inside the body.
    fn bar(buf: &[u8], l: &Layout, x: i64) -> i64 {
        let (_, y0, _, y1) = body(l);
        let b = clip_colour(0, Kind::Audio);
        let ink = [b[0] + WAVE_LIFT, b[1] + WAVE_LIFT, b[2] + WAVE_LIFT, 255];
        (y0..y1).filter(|&y| px(buf, l.w, x, y) == ink).count() as i64
    }

    #[test]
    fn peaks_of_sine_silence_click_stereo() {
        let sine: Vec<f32> = (0..48_000).map(|i| 0.5 * libm::sinf(2.0 * core::f32::consts::PI * 1000.0 * i as f32 / 48_000.0)).collect();
        let p = peaks_of(48_000, 1, &sine, PEAKS_PER_SEC);
        assert_eq!((p.per_sec, p.buckets.len()), (100, 100));
        assert!(p.buckets.iter().all(|&b| (127..=129).contains(&b)), "{:?}", p.buckets);
        let silence = alloc::vec![0.0f32; 48_000];
        assert_eq!(peaks_of(48_000, 1, &silence, PEAKS_PER_SEC).buckets, alloc::vec![0u8; 100]);
        let mut click = silence.clone();
        click[4810] = 1.0;
        let c = peaks_of(48_000, 1, &click, PEAKS_PER_SEC).buckets;
        assert_eq!(c[10], 255);
        assert_eq!(c.iter().filter(|&&b| b != 0).count(), 1, "one bucket holds the click");
        let stereo: Vec<f32> = (0..9_600).map(|i| if i % 2 == 0 { 0.25 } else { -0.75 }).collect();
        assert!(peaks_of(48_000, 2, &stereo, PEAKS_PER_SEC).buckets.iter().all(|&b| b == 191), "stereo folds by max");
        assert_eq!(peaks_of(48_000, 1, &silence[..4_801], PEAKS_PER_SEC).buckets.len(), 11);
        assert!(peaks_of(0, 1, &silence, PEAKS_PER_SEC).buckets.is_empty());
        // whole seconds one after another == one call (the scan's streaming)
        let two: Vec<f32> = sine.iter().chain(click.iter()).copied().collect();
        let mut fed = peaks_of(48_000, 1, &two[..48_000], PEAKS_PER_SEC).buckets;
        fed.extend(peaks_of(48_000, 1, &two[48_000..], PEAKS_PER_SEC).buckets);
        assert_eq!(fed, peaks_of(48_000, 1, &two, PEAKS_PER_SEC).buckets);
        let rate_441: Vec<f32> = (0..44_100 * 2).map(|i| (i % 7) as f32 / 7.0).collect();
        let mut fed = peaks_of(44_100, 1, &rate_441[..44_100], PEAKS_PER_SEC).buckets;
        fed.extend(peaks_of(44_100, 1, &rate_441[44_100..], PEAKS_PER_SEC).buckets);
        assert_eq!(fed, peaks_of(44_100, 1, &rate_441, PEAKS_PER_SEC).buckets);
    }

    #[test]
    fn waveform_body_not_flat_and_video_unchanged() {
        let ramp = Fixed(Peaks { per_sec: PEAKS_PER_SEC, buckets: (0..200u32).map(|i| (i * 37 % 256) as u8).collect(), chans: Vec::new(), lod: Vec::new() });
        let empty = Fixed(Peaks { per_sec: PEAKS_PER_SEC, buckets: Vec::new(), chans: Vec::new(), lod: Vec::new() });
        let st = one_clip(Kind::Audio, Clip::new(SourceId(0), 0, 50, 0));
        let l = Layout::of(320, 160, &st);
        let flat = paint_in(&st, l, Order::Rgba, None, None, None);
        let wave = paint_in(&st, l, Order::Rgba, None, None, Some(&ramp));
        let (cf, cw) = (colours(&flat, &l), colours(&wave, &l));
        assert_eq!(cf, 1, "no hook: one body colour");
        assert!(cw > 1, "the peaks paint over the body: {cw} colours");
        assert_eq!(paint_in(&st, l, Order::Rgba, None, None, Some(&empty)), flat, "an empty table paints the flat body");
        let v = one_clip(Kind::Video, Clip::new(SourceId(0), 0, 50, 0));
        assert_eq!(paint_in(&v, l, Order::Rgba, None, None, Some(&ramp)), paint_in(&v, l, Order::Rgba, None, None, None), "video bodies have no peaks");
        let bgra = paint_in(&st, l, Order::Bgra, None, None, Some(&ramp));
        assert!(wave.chunks(4).zip(bgra.chunks(4)).all(|(a, b)| a == [b[2], b[1], b[0], b[3]]), "the BGRA paint is the same picture");
        std::println!("striawaveform: golden fnv={:016x} body_colours flat={cf} wave={cw}", fnv1a(&wave));
        assert_eq!(alloc::format!("{:016x}", fnv1a(&wave)), GOLDEN_WAVE);
    }

    /// `waveform_body_not_flat_and_video_unchanged`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA
    /// paint of a 2-s audio clip under a ramp table).
    const GOLDEN_WAVE: &str = "923f68ef86d1a44d";

    #[test]
    fn waveform_gain_minus_6db_halves_the_bars() {
        let full = Fixed(Peaks { per_sec: PEAKS_PER_SEC, buckets: alloc::vec![255; 200], chans: Vec::new(), lod: Vec::new() });
        let at = |cdb: i32| {
            let st = one_clip(Kind::Audio, Clip::new(SourceId(0), 0, 50, 0).gained(cdb));
            let l = Layout::of(320, 160, &st);
            bar(&paint_in(&st, l, Order::Rgba, None, None, Some(&full)), &l, 160)
        };
        let (h0, h6, h12) = (at(0), at(-600), at(-1200));
        std::println!("striawaveform: bar_full={h0} bar_minus6db={h6} bar_minus12db={h12}");
        assert_eq!(h0, 36, "a full-scale bar fills the body");
        assert_eq!(h6, 18, "-6 dB halves");
        assert!((h12 - 9).abs() <= 1, "-12 dB quarters: {h12}");
    }

    #[test]
    fn waveform_fade_in_tapers() {
        let full = Fixed(Peaks { per_sec: PEAKS_PER_SEC, buckets: alloc::vec![255; 200], chans: Vec::new(), lod: Vec::new() });
        let st = one_clip(Kind::Audio, Clip::new(SourceId(0), 0, 50, 0).faded(25, 0));
        let l = Layout::of(320, 160, &st);
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&full));
        let z = l.zoom as i64;
        let (q1, q3) = (bar(&buf, &l, l.x_of(12) + z / 2), bar(&buf, &l, l.x_of(37) + z / 2));
        std::println!("striawaveform: fade_in=25/50 bar25={q1} bar75={q3} first={}", bar(&buf, &l, 1));
        assert!(q1 < q3, "the fade tapers the first half: {q1} vs {q3}");
        assert_eq!(q3, 36);
        assert_eq!(bar(&buf, &l, 1), 0, "silent at the clip's first tick");
    }
}

// ---- STRIASNAPDRAG (LEDGER SR836): tick 0 and the in / out marks are snap targets (named tail block) ----
// NLE2c (SR217) snaps a moving edge to every other clip's edges and the playhead; NLE6FACEDRAG / NLE6DRAGSNAP (B975 / B977)
// moved that into this view so both faces share it. The ONE target list, [`snap_targets`], also takes the timeline's start
// and the export marks (`Timeline::marks`, NLE5DRANGESAVE SR723) when set, so every snapping drag (move, trims, slide,
// scrub, transition edges) on the host face and the kernel band gains them at once. Owed: NLE-SNAP-MARKERS (SR835's
// markers here once landed), NLE-SNAP-TOGGLE, NLE-SNAP-MAGNET-AUDIO.

/// The targets every drag has whatever moves: tick 0 and each set in / out mark.
fn fixed_targets(tl: &Timeline) -> impl Iterator<Item = i64> {
    [Some(0), tl.marks.0, tl.marks.1].into_iter().flatten()
}

#[cfg(test)]
mod striasnapdrag_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// V: A 4..14 and A 30..40; A: A 16..24; no clip starts at 0. The marks as given.
    fn fixture(marks: (Option<i64>, Option<i64>)) -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 4), Clip::new(a, 20, 30, 30)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 8, 16)];
        EditorState::new(tl.set_marks(marks).unwrap())
    }

    /// M3: the list carries every other clip edge once (the dragged clip's excluded), tick 0, each set mark and the
    /// playhead when asked; no marks adds only tick 0.
    #[test]
    fn striasnapdrag_targets_carry_zero_and_the_marks() {
        let mut st = fixture((Some(50), Some(70)));
        st.seek(20);
        assert_eq!(snap_targets(&st, &[(0, 0)], true), alloc::vec![0, 16, 20, 24, 30, 40, 50, 70]);
        let st = fixture((Some(50), None));
        assert_eq!(snap_targets(&st, &[(0, 0)], false), alloc::vec![0, 16, 24, 30, 40, 50]);
        let st = fixture((None, None));
        assert_eq!(snap_targets(&st, &[], false), alloc::vec![0, 4, 14, 16, 24, 30, 40]);
    }

    /// M3: a body drag of A 30..40 whose in edge lands 3 px short of the in mark at 50 (raw 19 ticks) snaps to it
    /// (delta 20, the guide at 50) and commits flush on the mark; with no mark, or begun with no-snap, the raw 19 stands.
    #[test]
    fn striasnapdrag_move_snaps_to_a_mark() {
        let st0 = fixture((Some(50), None));
        let l = Layout::of_role(Role::Tracks, 800, 96, &st0);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        let x = l.x_of(35) as f64 + 1.0;
        let to = x + 20.0 * z - 3.0;
        let mut st = st0.clone();
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.idx, d.origin_tick), (DragKind::Move, 1, 35));
        d.update(&st, to, y);
        assert_eq!(l.tick_at(to) - d.origin_tick, 19, "the raw delta");
        assert_eq!((d.delta, d.guide), (20, Some(50)));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline.tracks[0].clips[1].at, 50, "flush on the mark");
        let mut st = fixture((None, None));
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        d.update(&st, to, y);
        assert_eq!((d.delta, d.guide), (19, None), "no mark: nothing in reach");
        let mut st = st0.clone();
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap();
        d.update(&st, to, y);
        assert_eq!((d.delta, d.guide), (19, None), "no-snap");
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline.tracks[0].clips[1].at, 49);
    }

    /// M3: at zoom 1 (1 px a tick) a trim-in of the selected A 3..13 (source in 10) pressed at x 4 and dragged to x 2 puts
    /// its in edge 1 px from tick 0 (raw -2) and snaps there (delta -3, the guide at 0; the playhead sits at the timeline's end, 13 px off).
    #[test]
    fn striasnapdrag_trim_in_snaps_to_tick_zero() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 10, 20, 3)];
        let mut st = EditorState::new(tl);
        st.zoom = 1;
        st.seek(13);
        st.select(0, 0).unwrap();
        let l = Layout::of_role(Role::Tracks, 800, 96, &st);
        assert_eq!((l.zoom, l.t0), (1, 0));
        let y = l.row_y(0) as f64 + ROW_H as f64 / 2.0;
        let mut d = Drag::begin_with(&mut st, l, 4.0, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.origin_tick), (DragKind::TrimIn, 4));
        d.update(&st, 2.0, y);
        assert_eq!((d.delta, d.guide), (-3, Some(0)));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline.tracks[0].clips[0], Clip::new(a, 7, 20, 0));
    }

    /// M3: a ruler scrub pressed at tick 60 and moved 3 px short of the out mark at 70 (raw 69) snaps the playhead there.
    #[test]
    fn striasnapdrag_scrub_snaps_to_the_out_mark() {
        let mut st = fixture((Some(50), Some(70)));
        let l = Layout::of_role(Role::Tracks, 800, 96, &st);
        let y = l.ruler_y as f64 + 5.0;
        let mut d = Drag::begin_with(&mut st, l, l.x_of(60) as f64 + 1.0, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.origin_tick), (DragKind::Scrub, 60));
        let to = l.x_of(70) as f64 - 3.0;
        d.update(&st, to, y);
        assert_eq!(l.tick_at(to), 69, "the raw tick");
        assert_eq!((d.origin_tick + d.delta, d.guide), (70, Some(70)));
    }
}

// ---- STRIACLIPGAINFACE (LEDGER SR838): the gain line and the fade ramps on the clip body (named tail block) ----
// A clip that carries a gain or a fade shows it on its body: a 1-px gain line on a dB scale (0 dB at mid-height,
// `GAIN_MAX_CDB` at the top, `GAIN_MIN_CDB` pinned to the bottom, through the meters' ONE dB map [`db_frac`]) and, over
// the fade columns, a 1-px ramp at the mix's own envelope (`schedule::env_q16`). A unity clip with no fade paints
// nothing, so every golden and the kernel view pin are unchanged. Painted last in the body arm: over the peaks and the label.

/// STRIACLIPGAINFACE (SR838): the gain line's and the fade ramps' ink.
pub const GAIN_INK: [u8; 4] = [0xff, 0xf0, 0xa0, 255];

/// The ONE dB map (the meters' strip, lifted from stria `paint_meters`): `db` on a linear scale from `floor_db` (0) to
/// 0 dB (1), clamped.
pub fn db_frac(db: f64, floor_db: f64) -> f64 {
    ((db - floor_db) / -floor_db).clamp(0.0, 1.0)
}

/// The row of the gain line for `cdb` centi-dB in a body spanning rows `top..=bottom`: 0 dB at mid-height, the top at
/// `GAIN_MAX_CDB`, the bottom at `GAIN_MIN_CDB` (each half through [`db_frac`]).
pub fn gain_y(cdb: i32, top: i64, bottom: i64) -> i64 {
    let mid = (top + bottom) / 2;
    let db = cdb as f64 / 100.0;
    let (max_db, min_db) = (crate::GAIN_MAX_CDB as f64 / 100.0, crate::GAIN_MIN_CDB as f64 / 100.0);
    if cdb >= 0 {
        mid - (db_frac(db - max_db, -max_db) * (mid - top) as f64 + 0.5) as i64
    } else {
        bottom - (db_frac(db, min_db) * (bottom - mid) as f64 + 0.5) as i64
    }
}

/// THE gain / fade painter: inside `body` (`x0, y0, x1, y1`, the clip body rect [`paint_in`] filled) the gain line at
/// [`gain_y`] across the body, and over the columns whose tick lies in the fade-in or the fade-out a ramp at
/// `env_q16(clip, t)` (bottom = silence, top = full), each column joined to the last by a vertical run. Nothing for a
/// clip with no gain and no fade, or a body under 2 px.
pub fn paint_gain_fades(c: &mut Canvas, l: &Layout, body: (i64, i64, i64, i64), clip: &crate::Clip) {
    let (x0, y0, x1, y1) = body;
    if (clip.gain_cdb == 0 && clip.fade_in <= 0 && clip.fade_out <= 0) || x1 - x0 < 2 || y1 - y0 < 2 {
        return;
    }
    let (top, bottom) = (y0, y1 - 1);
    let gy = gain_y(clip.gain_cdb, top, bottom);
    c.rect(x0, gy, x1, gy + 1, GAIN_INK);
    let (xa, z) = (l.x_of(clip.at), l.zoom.max(1) as i64);
    let one = crate::schedule::ENV_ONE as i64;
    let mut prev: Option<i64> = None;
    for x in x0.max(0)..x1.min(c.w as i64) {
        let t = clip.at + (x - xa).max(0) / z;
        if !(t < clip.at + clip.fade_in || t >= clip.end() - clip.fade_out) {
            prev = None;
            continue;
        }
        let y = bottom - crate::schedule::env_q16(clip, t) as i64 * (bottom - top) / one;
        let (a, b) = match prev {
            Some(p) => (p.min(y), p.max(y)),
            None => (y, y),
        };
        c.rect(x, a, x + 1, b + 1, GAIN_INK);
        prev = Some(y);
    }
}

#[cfg(test)]
mod striaclipgainface_tests {
    use super::*;
    use crate::{Clip, Kind, Timeline};

    /// One audio track, `clip` on it, the tracks band of a 320x56 canvas (body rows 18..54: top 18, bottom 53, mid 35).
    fn one(clip: Clip) -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![clip];
        let st = EditorState::new(tl);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        (st, l)
    }

    /// The rows of `GAIN_INK` in column `x`.
    fn ink_rows(px: &[u8], x: i64) -> Vec<i64> {
        (0..56i64).filter(|&y| px[((y * 320 + x) * 4) as usize..((y * 320 + x) * 4 + 4) as usize] == GAIN_INK).collect()
    }

    fn a(at: i64) -> Clip {
        Clip::new(crate::SourceId(0), 0, 20, at)
    }

    #[test]
    fn gain_y_and_db_frac_pins() {
        assert_eq!(db_frac(-30.0, -60.0), 0.5);
        assert_eq!(db_frac(-90.0, -60.0), 0.0);
        assert_eq!(db_frac(6.0, -60.0), 1.0);
        assert_eq!(gain_y(0, 18, 53), 35);
        assert_eq!(gain_y(2400, 18, 53), 18);
        assert_eq!(gain_y(-14_400, 18, 53), 53);
        assert_eq!(gain_y(1200, 18, 53), 26);
    }

    #[test]
    fn gain_line_mid_at_unity_higher_at_plus_12_and_none_on_a_plain_clip() {
        let (st, l) = one(a(0));
        let plain = paint_in(&st, l, Order::Rgba, None, None, None);
        let xm = l.x_of(10);
        assert!(ink_rows(&plain, xm).is_empty(), "a unity clip with no fade shows no line");
        let (st, l) = one(a(0).faded(2, 0));
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
        assert_eq!(ink_rows(&px, xm), alloc::vec![35], "0 dB at mid-height");
        let (st, l) = one(a(0).gained(1200));
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
        let rows = ink_rows(&px, xm);
        assert_eq!(rows, alloc::vec![26]);
        assert!(rows[0] < 35, "+12 dB sits higher");
    }

    #[test]
    fn fade_in_of_half_the_clip_ramps_through_mid_at_a_quarter() {
        // +24 dB puts the gain line on the top row, so the ink near mid in the quarter column is the ramp.
        let (st, l) = one(a(0).gained(2400).faded(10, 0));
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
        let (x0, x1) = (l.x_of(0), l.x_of(20));
        let q = x0 + (x1 - x0) / 4;
        let rows = ink_rows(&px, q);
        assert!(rows.iter().any(|&y| (y - 35).abs() <= 1), "{rows:?}");
        assert!(ink_rows(&px, x0 + 1).contains(&53), "the ramp starts at the bottom corner");
        assert_eq!(ink_rows(&px, l.x_of(15)), alloc::vec![18], "past the fade only the line");
    }
}

// ---- STRIATRACKMUTESOLO (LEDGER SR837): the mute / solo badges and the inactive rows' dimmed clips (named tail block) ----
// After the clip arm: every row NOT in `Timeline::active_tracks` (muted, or not soloed while another track is) has each
// clip body averaged half-way with its row colour in place (the canvas has no alpha: this is the half alpha, over whatever
// the clip arm painted), then each flagged row shows its badge at the left edge: an amber `M` box, a green `S` box beside
// it. No flag on any track: nothing is touched (every golden unchanged).

/// The mute badge's colour (amber).
pub const MUTE: [u8; 4] = [0xff, 0xb0, 0x20, 255];
/// The solo badge's colour (green).
pub const SOLO: [u8; 4] = [0x40, 0xd0, 0x60, 255];
/// A badge's side (px).
pub const BADGE: i64 = 11;
/// The badge letters, 7x7 (bit 6 the leftmost column): M, S.
const BADGE_GLYPHS: [[u8; 7]; 2] = [[0x41, 0x63, 0x55, 0x49, 0x41, 0x41, 0x41], [0x3e, 0x40, 0x40, 0x3e, 0x01, 0x01, 0x7e]];

fn paint_track_flags(c: &mut Canvas, l: Layout, tl: &Timeline) {
    if !tl.tracks.iter().any(|t| t.muted || t.solo) {
        return;
    }
    let live: Vec<usize> = tl.active_tracks().collect();
    for (row, tr) in tl.tracks.iter().enumerate().take(l.rows) {
        let (y0, y1) = (l.row_y(row), l.row_y(row) + ROW_H as i64);
        if !live.contains(&row) {
            let bg = c.put(ROW[row % 2]);
            for clip in &tr.clips {
                dim(c, l.x_of(clip.at) + 1, y0 + 2, l.x_of(clip.end()) - 1, y1 - 2, bg);
            }
        }
        let mut x = 2;
        for (on, ink, g) in [(tr.muted, MUTE, 0), (tr.solo, SOLO, 1)] {
            if !on {
                continue;
            }
            c.rect(x, y0 + 2, x + BADGE, y0 + 2 + BADGE, ink);
            for (dy, bits) in BADGE_GLYPHS[g].iter().enumerate() {
                for dx in 0..7i64 {
                    if bits & (0x40 >> dx) != 0 {
                        let (px, py) = (x + 2 + dx, y0 + 4 + dy as i64);
                        c.rect(px, py, px + 1, py + 1, FIELD);
                    }
                }
            }
            x += BADGE + 2;
        }
    }
}

/// Average every pixel of `x0..x1` x `y0..y1` (clipped) half-way with `bg` (already in the canvas's byte order).
fn dim(c: &mut Canvas, x0: i64, y0: i64, x1: i64, y1: i64, bg: [u8; 4]) {
    let (x0, x1) = (x0.clamp(0, c.w as i64) as usize, x1.clamp(0, c.w as i64) as usize);
    let (y0, y1) = (y0.clamp(0, c.h as i64) as usize, y1.clamp(0, c.h as i64) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * c.w as usize + x) * 4;
            for k in 0..3 {
                c.px[i + k] = ((c.px[i + k] as u16 + bg[k] as u16) / 2) as u8;
            }
        }
    }
}

#[cfg(test)]
mod striatrackmutesolo_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// Two audio tracks, the same reel's clip on each (twins), 320x120.
    fn twins() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for _ in 0..2 {
            let t = tl.add_track(Kind::Audio);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        }
        EditorState::new(tl)
    }

    /// M3: A1 muted: its row carries amber badge pixels, A2's none; the muted clip's body colour differs from its unmuted
    /// twin's (and is the half-way blend); with both flags off the canvas is byte-equal to the unflagged paint.
    #[test]
    fn striatrackmutesolo_badge_and_dim_paint() {
        let st0 = twins();
        let l = Layout::of(320, 120, &st0);
        let px0 = paint_in(&st0, l, Order::Rgba, None, None, None);
        let mut st = st0.clone();
        st.timeline = st.timeline.set_track_flags(0, true, false).unwrap();
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
        let at = |p: &[u8], x: i64, y: i64| -> [u8; 4] {
            let i = (y as usize * 320 + x as usize) * 4;
            [p[i], p[i + 1], p[i + 2], p[i + 3]]
        };
        let amber = |row: usize| (0..BADGE + 4).flat_map(|x| (0..ROW_H as i64).map(move |y| (x, y))).filter(|&(x, y)| at(&px, x, l.row_y(row) + y) == MUTE).count();
        assert!(amber(0) > 0, "the M badge on the muted row");
        assert_eq!(amber(1), 0, "no badge on the live row");
        let (x, y0, y1) = (100, l.row_y(0) + 20, l.row_y(1) + 20);
        let live = clip_colour(0, Kind::Audio);
        assert_eq!(at(&px, x, y1), live, "the unmuted twin");
        assert_ne!(at(&px, x, y0), live, "the muted body differs");
        let bg = ROW[0];
        assert_eq!(at(&px, x, y0), [((live[0] as u16 + bg[0] as u16) / 2) as u8, ((live[1] as u16 + bg[1] as u16) / 2) as u8, ((live[2] as u16 + bg[2] as u16) / 2) as u8, 255]);
        let mut solo = st0.clone();
        solo.timeline = solo.timeline.set_track_flags(1, false, true).unwrap();
        let ps = paint_in(&solo, l, Order::Rgba, None, None, None);
        assert_ne!(at(&ps, x, y0), live, "A1 not soloed while A2 is: dimmed");
        assert_eq!(at(&ps, x, y1), live, "the soloed row stays");
        let mut off = st.clone();
        off.timeline = off.timeline.set_track_flags(0, false, false).unwrap();
        assert_eq!(paint_in(&off, l, Order::Rgba, None, None, None), px0, "no flag: the unflagged paint, byte for byte");
    }
}

// ---- STRIATHUMBNAILS (LEDGER SR839): a video clip's head frame (named tail block) ----
// A video clip's body shows its HEAD: the frame at `src_in`, scaled to the body's height in the frame's own aspect
// ([`head_still`], [`SizePolicy::Fill`](crate::SizePolicy::Fill) through THE scaler `Canvas::blit_policy`), blitted at the
// body's left edge by [`paint_head`] from [`paint_in`]'s clip-body arm through SR827's optional [`PeakSource`] hook (its
// `thumb` method, default `None`). The box is `min(still width, body width)` wide, so a clip narrower than the still shows the
// still's centre columns and no pixel leaves the body; the colour bar continues to the right; the labels paint over it. A head
// whose body starts left of the canvas paints nothing. No hook or no still: the body exactly as before (every golden and the
// kernel view band byte-identical). The stills are the face's (Stria's `nle_face::Thumbs`, decoded through `preview`).
// Owed: NLE-THUMBNAILS-FILMSTRIP, NLE-THUMBNAILS-TAIL, NLE-THUMBNAILS-BACKGROUND, NLE-THUMBNAILS-KERNEL.

/// The head still's height: the clip body's ([`paint_in`] fills `y0 + 2 .. y1 - 2` of a [`ROW_H`] row).
pub const HEAD_H: u32 = ROW_H - 4;
/// The widest head still (a 32:9 frame at [`HEAD_H`]).
pub const HEAD_MAX_W: u32 = 128;

/// A `w` x `h` frame's head size: `(round(w * HEAD_H / h)` clamped to `1..=HEAD_MAX_W`, `HEAD_H)`; `None` for an empty frame.
pub fn head_size(w: u32, h: u32) -> Option<(u32, u32)> {
    if w == 0 || h == 0 {
        return None;
    }
    let tw = ((w as u64 * HEAD_H as u64 + h as u64 / 2) / h as u64).clamp(1, HEAD_MAX_W as u64) as u32;
    Some((tw, HEAD_H))
}

/// `full` scaled to its [`head_size`] by [`Canvas::blit_policy`] with `Fill` (the frame's own aspect, no bars); `None` for
/// an empty frame.
pub fn head_still(full: &Still) -> Option<Still> {
    let (tw, th) = head_size(full.width, full.height)?;
    let mut c = Canvas::new(tw, th, Order::Rgba);
    c.blit_policy(full, 0, 0, tw, th, crate::SizePolicy::Fill);
    Some(Still { width: tw, height: th, rgba: c.px })
}

/// THE ONE head painter: `src`'s still for `(clip.src, clip.src_in)` into `body` (`x0, y0, x1, y1`, the clip body rect
/// [`paint_in`] filled) at its left edge, `min(still width, body width)` x body height, by `Fill` (1:1 at full width, the
/// still's centre columns when narrower). No still, a body starting left of the canvas, or an empty body: nothing.
pub fn paint_head(c: &mut Canvas, body: (i64, i64, i64, i64), clip: &crate::Clip, src: &dyn PeakSource) {
    let Some(s) = src.thumb(clip.src, clip.src_in) else { return };
    let (x0, y0, x1, y1) = body;
    let bw = (s.width as i64).min(x1 - x0);
    let bh = y1 - y0;
    if x0 < 0 || y0 < 0 || bw <= 0 || bh <= 0 || x0 >= c.w as i64 {
        return;
    }
    c.blit_policy(s, x0 as u32, y0 as u32, bw as u32, bh as u32, crate::SizePolicy::Fill);
}

#[cfg(test)]
mod head_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};

    /// A test hook: one head still for every key, and optionally one peak table (the waveform's ramp).
    struct Hook(Option<Still>, Option<Peaks>);
    impl PeakSource for Hook {
        fn peaks(&self, _src: SourceId) -> Option<&Peaks> {
            self.1.as_ref()
        }
        fn gain_q16(&self, cdb: i32) -> i64 {
            if cdb == 0 { 65536 } else { libm::round(libm::pow(10.0, cdb as f64 / 2000.0) * 65536.0) as i64 }
        }
        fn thumb(&self, _src: SourceId, _src_tick: i64) -> Option<&Still> {
            self.0.as_ref()
        }
    }

    /// A 48 x 36 still whose every pixel is distinct from its neighbours and from the clip colours.
    fn pattern() -> Still {
        let (w, h) = (48u32, 36u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 5) as u8, (y * 7) as u8, 0x11, 255]);
            }
        }
        Still { width: w, height: h, rgba }
    }

    fn one_clip(kind: Kind, clip: Clip) -> EditorState {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(kind);
        tl.tracks[0].clips = alloc::vec![clip];
        EditorState::new(tl)
    }

    fn px(buf: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn head_size_and_still() {
        assert_eq!(head_size(64, 48), Some((48, 36)));
        assert_eq!(head_size(1920, 1080), Some((64, 36)));
        assert_eq!(head_size(0, 48), None);
        assert_eq!(head_size(64, 0), None);
        assert_eq!(head_size(10_000, 10), Some((HEAD_MAX_W, HEAD_H)));
        let s = head_still(&pattern()).expect("a head");
        assert_eq!((s.width, s.height, s.rgba.len()), (48, 36, 48 * 36 * 4));
        assert_eq!(s, pattern(), "a still already head-sized scales 1:1");
    }

    #[test]
    fn head_box_shows_the_still_and_the_bar_continues() {
        let still = pattern();
        let hook = Hook(Some(still.clone()), None);
        let st = one_clip(Kind::Video, Clip::new(SourceId(0), 0, 11, 2)); // STRIATHUMBNAILSMORE (SR949): an 86 px body (< 2 heads): head-only, the bar right of it
        let l = Layout::of(320, 160, &st);
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&hook));
        let (bx, by) = (l.x_of(2) + 1, l.row_y(0) + 2);
        let mut matched = 0;
        for y in 0..36 {
            for x in 0..48 {
                let i = ((y * 48 + x) * 4) as usize;
                assert_eq!(px(&buf, l.w, bx + x, by + y), [still.rgba[i], still.rgba[i + 1], still.rgba[i + 2], still.rgba[i + 3]], "head pixel {x},{y}");
                matched += 1;
            }
        }
        let bar = clip_colour(0, Kind::Video);
        for y in by..by + 36 {
            assert_eq!(px(&buf, l.w, bx + 48, y), bar, "the colour bar continues right of the box");
        }
        let bgra = paint_in(&st, l, Order::Bgra, None, None, Some(&hook));
        assert!(buf.chunks(4).zip(bgra.chunks(4)).all(|(a, b)| a == [b[2], b[1], b[0], b[3]]), "the BGRA paint is the same picture");
        std::println!("striathumbnails: head box 48x36 matched={matched} fnv={:016x}", fnv1a(&buf));
    }

    #[test]
    fn head_none_and_audio_unchanged() {
        let st = one_clip(Kind::Video, Clip::new(SourceId(0), 0, 50, 0));
        let l = Layout::of(320, 160, &st);
        let none = paint_in(&st, l, Order::Rgba, None, None, None);
        assert_eq!(paint_in(&st, l, Order::Rgba, None, None, Some(&Hook(None, None))), none, "no still: the body as before");
        // the audio body under a hook holding BOTH a still and SR827's ramp table is SR827's golden: no head on audio
        let ramp = Peaks { per_sec: PEAKS_PER_SEC, buckets: (0..200u32).map(|i| (i * 37 % 256) as u8).collect(), chans: Vec::new(), lod: Vec::new() };
        let a = one_clip(Kind::Audio, Clip::new(SourceId(0), 0, 50, 0));
        let la = Layout::of(320, 160, &a);
        let wave = paint_in(&a, la, Order::Rgba, None, None, Some(&Hook(Some(pattern()), Some(ramp))));
        assert_eq!(alloc::format!("{:016x}", fnv1a(&wave)), "923f68ef86d1a44d", "the audio body equals the STRIAWAVEFORM golden");
    }

    #[test]
    fn head_narrow_clip_stays_inside_its_body() {
        let hook = Hook(Some(pattern()), None);
        // a 3-tick clip at zoom 8: a 22 px body, narrower than the 48 px still
        let st = one_clip(Kind::Video, Clip::new(SourceId(0), 0, 3, 4));
        let l = Layout::of(320, 160, &st);
        let none = paint_in(&st, l, Order::Rgba, None, None, None);
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&hook));
        let (x0, x1) = (l.x_of(4) + 1, l.x_of(7) - 1);
        let (y0, y1) = (l.row_y(0) + 2, l.row_y(0) + ROW_H as i64 - 2);
        let mut inside = 0;
        for y in 0..l.h as i64 {
            for x in 0..l.w as i64 {
                let (a, b) = (px(&buf, l.w, x, y), px(&none, l.w, x, y));
                if x >= x0 && x < x1 && y >= y0 && y < y1 {
                    inside += (a != b) as usize;
                } else {
                    assert_eq!(a, b, "a head pixel at {x},{y} outside the body");
                }
            }
        }
        assert_eq!(inside as i64, (x1 - x0) * (y1 - y0), "the whole narrow body is the head");
        // the body shows the still's centre columns (Fill's centred crop)
        let still = pattern();
        let c0 = ((48 - (x1 - x0)) / 2) as usize;
        let i = c0 * 4;
        assert_eq!(px(&buf, l.w, x0, y0), [still.rgba[i], still.rgba[i + 1], still.rgba[i + 2], still.rgba[i + 3]], "the centre crop");
        std::println!("striathumbnails: narrow body {}x{} px all head, none outside", x1 - x0, y1 - y0);
    }
}

// ---- STRIAMARKERS (LEDGER SR835): the named markers on the ruler (named tail block) ----
// Each marker in view paints from [`paint_in`]'s ruler arm: a down-pointing triangle at the ruler's top (rows `ruler_y ..
// ruler_y + MARKER_W + 1`, inside the ruler band) in the named [`MARKER`] colour, and its name through the [`Text`] hook
// (the labels' style and ink) beside it. Both faces paint it (the host face and the kernel Player's view band); a timeline
// with no marker paints byte-identical.

/// STRIAMARKERS (SR835): a marker's colour (the editor table's; NLE6VIEW-TOKENS owes palette_core).
pub const MARKER: [u8; 4] = [0xff, 0x9a, 0x30, 255];
/// STRIAMARKERS (SR835): the marker triangle's half width (px); it is `MARKER_W + 1` rows tall.
pub const MARKER_W: i64 = 4;

/// STRIAMARKERS (SR835): the markers over the ruler band of `l`.
fn paint_markers(c: &mut Canvas, l: Layout, tl: &Timeline, labels: &mut Option<&mut dyn Text>) {
    let (ry, w) = (l.ruler_y as i64, l.w as i64);
    for m in &tl.markers {
        let x = l.x_of(m.at); paint_marker_range(c, l, m, w); // STRIAMARKERJUMPKEYSNEXT (SR984): a range marker's bar, before the off-view skip (file tail)
        if x + MARKER_W < 0 || x - MARKER_W >= w {
            continue;
        }
        for k in 0..=MARKER_W {
            c.rect(x - (MARKER_W - k), ry + k, x + (MARKER_W - k) + 1, ry + k + 1, marker_rgba(m.colour)); // STRIAMARKERJUMPKEYSNEXT (SR984): the marker's colour (file tail)
        }
        if let Some(text) = labels.as_mut() {
            text.text(c, &m.name, (x + MARKER_W + 2) as f32, ry as f32 + 9.0, ((x + MARKER_W + 1) as i32, w as i32));
        }
    }
}

#[cfg(test)]
mod striamarkers_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// STRIAMARKERS M3: at 320x160 a marker at tick 5 changes pixels only inside the ruler band (`ruler_y..tracks_y`):
    /// none outside, some inside, the triangle's apex column at the ruler's top in MARKER.
    #[test]
    fn striamarkers_marker_paints_only_in_the_ruler() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(a, 0, 10, 10)];
        }
        let plain = EditorState::new(tl.clone());
        let marked = EditorState::new(tl.mark(5, "M1").unwrap());
        let (w, h) = (320u32, 160u32);
        let l = Layout::of(w, h, &marked);
        let (a, b) = (paint(&plain, w, h, Order::Rgba, None, None), paint(&marked, w, h, Order::Rgba, None, None));
        let (mut inside, mut outside) = (0usize, 0usize);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                if a[i..i + 4] != b[i..i + 4] {
                    if (l.ruler_y as usize..l.tracks_y as usize).contains(&y) { inside += 1 } else { outside += 1 }
                }
            }
        }
        assert_eq!(outside, 0, "a marker paints nothing outside the ruler");
        assert!(inside > 0, "the marker shows on the ruler");
        let (x, y) = (l.x_of(5) as usize, l.ruler_y as usize);
        assert_eq!(b[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4], MARKER);
        assert_eq!(inside as i64, (0..=MARKER_W).map(|k| 2 * (MARKER_W - k) + 1).sum::<i64>(), "the triangle alone (labels off)");
    }
}

// ---- STRIARIPPLETRIM (LEDGER SR850): the ripple variant of the edge drag (named tail block) ----
// A press with shift (and not the slip modifier) on the primary's drawn trim handle begins the same TrimIn / TrimOut drag
// with `ripple` set; `Drag::finish` commits it as ONE `EditorState::ripple_trim` (the later clips of that track follow; one
// undo step) and `Drag::shown` previews it (finish on a scratch state). Snapping is the plain trim's (`snap_delta` unchanged).

#[cfg(test)]
mod ripple_drag_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLEFXA 0..10 at 0, 10..20 at 10, 20..30 at 20 (three butted clips), a 40-tick source, the view band `Role::Tracks`
    /// 320x56; clip `sel` selected.
    fn ripple_fixture(sel: usize) -> (Timeline, EditorState, Layout, f64, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10), Clip::new(a, 20, 30, 20)];
        let mut st = EditorState::new(tl.clone());
        st.select(0, sel).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        (tl, st, l, z, y)
    }

    /// A twin editor's `rtrim` word on clip `sel` of `tl` (THE ripple op by its verb).
    fn rtrim_twin(tl: &Timeline, sel: usize, word: &str) -> (Result<(), EditError>, Timeline) {
        let mut twin = EditorState::new(tl.clone());
        twin.select(0, sel).unwrap();
        let r = twin.verb(word).unwrap();
        (r, twin.timeline)
    }

    /// STRIARIPPLETRIM (SR850) M3: shift on clip 0's out handle dragged -3 ticks is a ripple TrimOut: the preview
    /// (`Drag::shown`) equals the committed timeline, clip 0 ends at 7 and clips 1 and 2 each move -3 (to 7 and 17), the
    /// twin of `rtrim out -3`; ONE undo restores all three clips.
    #[test]
    fn shift_on_the_out_handle_ripples() {
        let (tl, mut st, l, z, y) = ripple_fixture(0);
        let x = l.x_of(10) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert_eq!((d.kind, d.idx, d.ripple), (DragKind::TrimOut, 0, true));
        d.update(&st, x - 3.0 * z, y);
        assert_eq!((d.delta, d.guide), (-3, None));
        let want = tl.ripple_trim_out(0, 0, 7).unwrap();
        assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()), "the preview is the ripple");
        assert_eq!(st.timeline, tl, "nothing committed before the release");
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, want);
        let ats: Vec<i64> = st.timeline.tracks[0].clips.iter().map(|c| c.at).collect();
        assert_eq!((st.timeline.tracks[0].clips[0].end(), ats), (7, alloc::vec![0, 7, 17]));
        assert_eq!(rtrim_twin(&tl, 0, "rtrim out -3"), (Ok(()), want));
        assert_eq!(st.undo_depth(), 1);
        assert!(st.undo());
        assert_eq!(st.timeline, tl, "one undo restores all three");
    }

    /// STRIARIPPLETRIM (SR850) M3: the same drag without shift is the plain TrimOut: clips 1 and 2 stay (a gap opens).
    #[test]
    fn plain_out_handle_leaves_the_gap() {
        let (tl, mut st, l, z, y) = ripple_fixture(0);
        let x = l.x_of(10) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.ripple), (DragKind::TrimOut, false));
        d.update(&st, x - 3.0 * z, y);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.trim_out(0, 0, 7).unwrap());
        let ats: Vec<i64> = st.timeline.tracks[0].clips.iter().map(|c| c.at).collect();
        assert_eq!(ats, alloc::vec![0, 10, 20], "the neighbours stay");
    }

    /// STRIARIPPLETRIM (SR850) M3: shift on clip 1's in handle dragged +3 ticks is a ripple TrimIn: clip 1 KEEPS its head
    /// (at 10) with its source in at 13, its out edge and clip 2 move -3 (clip 2 to 17), clip 0 is untouched; `rtrim in 3`.
    #[test]
    fn shift_on_the_in_handle_ripples_keeping_the_head() {
        let (tl, mut st, l, z, y) = ripple_fixture(1);
        let x = l.x_of(10) as f64 + 1.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert_eq!((d.kind, d.idx, d.ripple), (DragKind::TrimIn, 1, true));
        d.update(&st, x + 3.0 * z, y);
        assert_eq!(d.delta, 3);
        let shown = d.shown(&st).map(|s| s.timeline);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(shown, Some(st.timeline.clone()), "the preview equals the commit");
        let c = &st.timeline.tracks[0].clips;
        assert_eq!((c[0], c[1].at, c[1].src_in, c[1].end(), c[2].at), (tl.tracks[0].clips[0], 10, 13, 17, 17));
        assert_eq!(rtrim_twin(&tl, 1, "rtrim in 3"), (Ok(()), st.timeline.clone()));
        assert_eq!(st.undo_depth(), 1);
    }

    /// STRIARIPPLETRIM (SR850) M3: the moving edge snaps exactly as a plain trim's: the playhead at 6, the out edge dragged
    /// to 3 px right of `x_of(6)` snaps (delta -4, the guide at 6); with the no-snap modifier the raw delta (-3) stands.
    #[test]
    fn ripple_snaps_and_alt_unsnaps() {
        let (tl, mut st, l, _z, y) = ripple_fixture(0);
        st.seek(6);
        let x = l.x_of(10) as f64 - 2.0;
        let to = x - 29.0;
        let mut d = Drag::begin_with(&mut st.clone(), l, x, y, true, false, false).unwrap();
        d.update(&st, to, y);
        assert_eq!((d.ripple, d.delta, d.guide), (true, -4, Some(6)));
        let mut n = Drag::begin_with(&mut st.clone(), l, x, y, true, true, false).unwrap();
        n.update(&st, to, y);
        assert_eq!((n.ripple, n.snap, n.delta, n.guide), (true, false, -3, None));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.ripple_trim_out(0, 0, 6).unwrap());
    }

    /// STRIARIPPLETRIM (SR850) M3: a ripple past the source's end (clip 2's out 30 dragged +11 on a 40-tick source) is
    /// refused by name (`OutOfSource`): no preview, and the timeline and undo depth do not move.
    #[test]
    fn ripple_past_the_source_is_refused() {
        let (tl, mut st, l, z, y) = ripple_fixture(2);
        let x = l.x_of(30) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, true, true, false).unwrap();
        d.update(&st, x + 11.0 * z, y);
        assert_eq!((d.kind, d.ripple, d.delta), (DragKind::TrimOut, true, 11));
        assert!(d.shown(&st).is_none(), "a refused ripple previews nothing");
        assert_eq!(d.finish(&mut st), Err(EditError::OutOfSource));
        assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
    }

    /// STRIARIPPLETRIM (SR850) M3: a dissolve on the 0/1 cut: the ripple drag of clip 0's out by -3 does exactly what the
    /// `rtrim out -3` word does (the ripple op shifts the transition with the cut: it sits on the moved cut at 7).
    #[test]
    fn ripple_at_a_transitioned_cut_is_the_rtrim() {
        let (tl0, _, l, z, y) = ripple_fixture(0);
        let tl = tl0.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
        let mut st = EditorState::new(tl.clone());
        st.select(0, 0).unwrap();
        let x = l.x_of(10) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, true, true, false).unwrap();
        d.update(&st, x - 3.0 * z, y);
        assert_eq!(d.delta, -3);
        let r = d.finish(&mut st);
        let (tr, twin) = rtrim_twin(&tl, 0, "rtrim out -3");
        assert_eq!((r, &st.timeline), (tr, &twin), "the drag is the word");
        assert_eq!(r, Ok(()));
        assert_eq!(st.timeline.tracks[0].transitions.iter().map(|t| t.at).collect::<Vec<_>>(), alloc::vec![7], "on the moved cut");
    }

    /// STRIARIPPLETRIM (SR850) M3: shift with the slip modifier on the handle keeps today's meaning (the edge block is skipped:
    /// the body's slip, additive); never a ripple.
    #[test]
    fn shift_slip_on_the_handle_keeps_the_slip() {
        let (_tl, mut st, l, _z, y) = ripple_fixture(0);
        let x = l.x_of(10) as f64 - 2.0;
        let d = Drag::begin_with(&mut st, l, x, y, true, false, true).unwrap();
        assert_eq!((d.kind, d.ripple, d.additive), (DragKind::Slip, false, true));
    }
}

// ---- STRIAAUDIOCROSSFADE (LEDGER SR860): the explicit audio crossfade's bracket (named tail block) ----
// Every explicit audio crossfade (`Timeline::audio_xfades`, NLEXFADEPOWER's record) paints as a bracket in its audio row's
// foot band, on THE transition widget's geometry (`Layout::transition_rect`), with the two weights of THE curve
// (`schedule::xfade_weight`: rising and falling) plotted inside it, so equal-power and linear read apart. Only explicit
// records paint: a crossfade a video transition implies is already the video widget, and every existing golden holds no
// explicit record (byte-identical).

/// The brackets of every explicit audio crossfade on a visible row.
fn paint_audio_xfades(c: &mut Canvas, l: Layout, st: &EditorState, labels: &mut Option<&mut dyn Text>) { // STRIAXFADEMORE (SR980): and the painter's labels, for the curve's name
    use crate::schedule::{ENV_ONE, xfade_weight};
    for x in st.timeline.audio_xfades.iter().filter(|x| x.track < l.rows) {
        let (x0, y0, x1, y1) = l.xfade_rect(x); // STRIAXFADETHREE (SR9023): the bracket's one geometry, the hit's too (file tail)
        if x1 <= 0 || x0 >= l.w as i64 {
            continue;
        }
        c.rect(x0, y0, x1, y1, TRANS_FILL);
        c.frame(x0, y0, x1, y1, TRANS); paint_xfade_name(c, labels, x.curve, (x0, y0, x1)); // STRIAXFADEMORE (SR980): the bracket names its curve (file tail)
        let (span, h) = ((x1 - x0 - 2).max(1), y1 - y0 - 3);
        for px in x0 + 1..x1 - 1 {
            let p = ((px - x0 - 1) * ENV_ONE as i64 / span) as u32;
            for w in [xfade_weight(x.curve, p), xfade_weight(x.curve, ENV_ONE - p)] {
                let py = y1 - 2 - w as i64 * h / ENV_ONE as i64;
                c.rect(px, py, px + 1, py + 1, TRANS);
            }
        }
    }
}

// ---- STRIAOPACITYFACE (LEDGER SR898): the clip body painted at its opacity (named tail block) ----
// A clip below [`crate::OPAQUE`] paints its body blended from the row colour toward its reel colour by its percent (0 % is
// the row itself); an opaque clip paints its reel colour unchanged, so every golden stays byte-identical. Both faces (the
// host's `nle_face` and the kernel Player's view band) paint through `paint_in`, so both show it.

/// STRIAOPACITYFACE: `c` over `row` at `pct` percent; `c` itself at [`crate::OPAQUE`] or above.
pub fn opacity_colour(c: [u8; 4], row: [u8; 4], pct: u8) -> [u8; 4] {
    if pct >= crate::OPAQUE {
        return c;
    }
    let mix = |a: u8, b: u8| -> u8 { (b as i32 + ((a as i32 - b as i32) * pct as i32 * 2 + crate::OPAQUE as i32).div_euclid(2 * crate::OPAQUE as i32)) as u8 };
    [mix(c[0], row[0]), mix(c[1], row[1]), mix(c[2], row[2]), 255]
}

// ---- STRIAGRADEFACE (LEDGER SR899): a graded clip's badge on its body (named tail block) ----
// Every clip whose SR869 grade is not identity carries a small badge: three 2 x 6 px stripes (R, G, B) at the body's top
// right, inside the end trim handle. A clip too narrow for it, or off the canvas, shows none; an identity grade paints no
// pixel, so every golden is byte-identical. The kernel Player grades a graded timeline too (vplay.rs `edl_grade`, SR992).

/// The badge's three stripes, left to right.
pub const GRADE_BADGE: [[u8; 4]; 3] = [[0xff, 0x50, 0x50, 255], [0x50, 0xe0, 0x50, 255], [0x50, 0x90, 0xff, 255]];

/// The badge's left edge and top for a clip body `x0..x1` on row top `y0`; `None` when the body is too narrow.
fn grade_badge_at(x0: i64, x1: i64, y0: i64) -> Option<(i64, i64)> {
    let bx = x1 - 1 - HANDLE_W - 7;
    if bx < x0 + 1 + HANDLE_W {
        return None;
    }
    Some((bx, y0 + 3))
}

/// [`paint_in`]'s badges: one per non-identity-graded clip on the layout's rows.
fn paint_grade_badges(c: &mut Canvas, l: Layout, st: &EditorState) {
    for (row, tr) in st.timeline.tracks.iter().enumerate().take(l.rows) {
        for clip in tr.clips.iter().filter(|c| c.grade != crate::Grade::IDENTITY) {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 <= 0 || x0 >= l.w as i64 {
                continue;
            }
            let Some((bx, by)) = grade_badge_at(x0, x1, l.row_y(row)) else { continue };
            for (i, k) in GRADE_BADGE.iter().enumerate() {
                let x = bx + 2 * i as i64;
                c.rect(x, by, x + 2, by + 6, *k);
            }
        }
    }
}

#[cfg(test)]
mod striagradeface_tests {
    use super::*;
    use crate::{Clip, Grade, Kind, Timeline};

    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10)];
        EditorState::new(tl)
    }

    /// The badge paints only on a graded clip: graded-then-cleared paints byte-identical to never graded; graded, the
    /// canvas differs ONLY inside the badge's 6 x 6 rect, and each stripe carries its colour.
    #[test]
    fn striagradeface_badge_only_when_graded() {
        let (w, h) = (320u32, 160u32);
        let st = state();
        let plain = paint(&st, w, h, Order::Rgba, None, None);
        let g = Grade { brightness: 50, ..Grade::IDENTITY };
        let graded = EditorState::new(st.timeline.set_grade(0, 1, g).unwrap());
        let cleared = EditorState::new(graded.timeline.set_grade(0, 1, Grade::IDENTITY).unwrap());
        assert_eq!(paint(&cleared, w, h, Order::Rgba, None, None), plain);
        let px = paint(&graded, w, h, Order::Rgba, None, None);
        let l = Layout::of(w, h, &graded);
        let clip = graded.timeline.tracks[0].clips[1];
        let (bx, by) = grade_badge_at(l.x_of(clip.at), l.x_of(clip.end()), l.row_y(0)).expect("room for the badge");
        let at = |x: i64, y: i64| (y as usize * w as usize + x as usize) * 4;
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let i = at(x, y);
                if (bx..bx + 6).contains(&x) && (by..by + 6).contains(&y) {
                    assert_eq!(&px[i..i + 4], &GRADE_BADGE[((x - bx) / 2) as usize][..], "({x},{y})");
                } else {
                    assert_eq!(&px[i..i + 4], &plain[i..i + 4], "({x},{y})");
                }
            }
        }
    }
}

// ---- STRIAPANFACE (LEDGER SR897): the pan tick on an audio clip's body (named tail block) ----
// A 2 px x 4 px mark in [`TICK`] at the body's top edge, at the clip's pan along the body between its trim handles (centre
// at the body's middle, hard left / right a handle's width inside the ends). Painted only on an audio track and only when
// `pan_cq8 != 0`, so a centred clip paints exactly as before (every golden unchanged). The kernel Player paints it through
// this same painter: it shows the document's pan, though the kernel plays pan centred (NLE-PAN-KERNEL).
fn pan_tick(c: &mut Canvas, kind: Kind, clip: &crate::Clip, x0: i64, x1: i64, y0: i64) {
    if !matches!(kind, Kind::Audio) || clip.pan_cq8 == 0 {
        return;
    }
    let half = pan_half(x0, x1); // STRIAPANNEXT (SR994): the one tick geometry, shared with the pan grip (file tail)
    let xm = (x0 + x1) / 2 + half * (clip.pan_cq8 as i64).clamp(-127, 127) / 127;
    c.rect(xm - 1, y0 + 2, xm + 1, y0 + 6, TICK);
}

// ---- STRIACROPFACE (LEDGER SR900): the crop badge on a cropped clip's body (named tail block) ----
/// STRIACROPFACE (SR900): the crop badge's glyph colour.
pub const CROP_BADGE: [u8; 4] = [0xff, 0xa0, 0x20, 255];
/// The crop glyph, 7x7 (bit 6 the leftmost column): two interlocking corners.
const CROP_GLYPH: [u8; 7] = [0x20, 0x7c, 0x24, 0x24, 0x24, 0x3f, 0x04];

/// A clip body `x0..x1` on the row at `y0` carrying `crop`: a 9x9 [`FIELD`] box at the body's top right (`x1 - 11 ..
/// x1 - 2`, `y0 + 3 .. y0 + 12`) holding [`CROP_GLYPH`] in [`CROP_BADGE`]. No crop, or a body under 16 px: nothing painted
/// (every golden stays byte-identical).
fn paint_crop_badge(c: &mut Canvas, crop: crate::Crop, x0: i64, x1: i64, y0: i64) {
    if crop == crate::Crop::NONE || x1 - x0 < 16 {
        return;
    }
    let (bx, by) = (x1 - 11, y0 + 3);
    c.rect(bx, by, bx + 9, by + 9, FIELD);
    for (dy, bits) in CROP_GLYPH.iter().enumerate() {
        for dx in 0..7i64 {
            if bits & (0x40 >> dx) != 0 {
                let (px, py) = (bx + 1 + dx, by + 1 + dy as i64);
                c.rect(px, py, px + 1, py + 1, CROP_BADGE);
            }
        }
    }
}

// ---- STRIAPROJECTBIN (LEDGER SR885): the source bin — rows, geometry, paint, hit, the insert word (named tail block) ----
// The editor's source bin, painted by THIS painter so the kernel face can show it later (NLE-BIN-KERNEL owed): one row per
// `Timeline::sources` entry (reel, duration as `edl::timecode`, the kind read from the tracks its clips sit on), each row
// [`BIN_ROW_H`] px from the top of its own Surface ([`Role::Bin`]). A click is NOT an edit of its own: [`bin_word`] names
// SR852's `insert <reel> 0 <duration>`, which the face sends through its one verb road (`EditorState::verb` ->
// `insert_reel` -> `Timeline::place` -> the ONE `commit`). Rows past the canvas are not shown (NLE-BIN-SCROLL owed).

/// One bin row's height (px).
pub const BIN_ROW_H: u32 = 18;
/// The row's left pad and the reel swatch's width (px).
pub const BIN_PAD: i64 = 4;
pub const BIN_SWATCH: i64 = 12;

/// What a source feeds, read from the tracks its clips sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinKind {
    Video,
    Audio,
    /// On a video and an audio track.
    Both,
    /// On no track.
    Unused,
}

impl BinKind {
    /// The bin's word: `V`, `A`, `AV`, `-`.
    pub fn name(self) -> &'static str {
        match self {
            BinKind::Video => "V",
            BinKind::Audio => "A",
            BinKind::Both => "AV",
            BinKind::Unused => "-",
        }
    }
}

/// One bin row: a source of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinRow {
    pub src: crate::SourceId,
    pub reel: alloc::string::String,
    pub duration: i64,
    pub kind: BinKind,
}

impl BinRow {
    /// The row's swatch: the reel's clip colour (darker for an audio-only source), [`TICK`] grey when unused.
    pub fn swatch(&self) -> [u8; 4] {
        match self.kind {
            BinKind::Video | BinKind::Both => clip_colour(self.src.0, Kind::Video),
            BinKind::Audio => clip_colour(self.src.0, Kind::Audio),
            BinKind::Unused => TICK,
        }
    }
}

/// THE bin's rows, in `Timeline::sources` order (read by [`paint_bin`], [`bin_hit`]'s index, the witness's JSON).
pub fn bin_rows(tl: &Timeline) -> Vec<BinRow> {
    tl.sources
        .iter()
        .map(|s| {
            let on = |k: Kind| tl.tracks.iter().any(|t| t.kind == k && t.clips.iter().any(|c| c.src == s.id));
            let kind = match (on(Kind::Video), on(Kind::Audio)) {
                (true, true) => BinKind::Both,
                (true, false) => BinKind::Video,
                (false, true) => BinKind::Audio,
                (false, false) => BinKind::Unused,
            };
            BinRow { src: s.id, reel: s.reel.clone(), duration: s.duration, kind }
        })
        .collect()
}

/// The y of bin row `i`'s top.
pub fn bin_row_y(i: usize) -> i64 {
    i as i64 * BIN_ROW_H as i64
}

/// A point inside bin row `i` (what a click on that row sends).
pub fn bin_point(i: usize) -> (f64, f64) {
    (BIN_PAD as f64 + 1.0, bin_row_y(i) as f64 + BIN_ROW_H as f64 / 2.0)
}

/// The source row under `(x, y)` on a `w` x `h` bin canvas: `Some(index into tl.sources)`, `None` off the canvas or
/// under the last row.
pub fn bin_hit(tl: &Timeline, w: u32, h: u32, x: f64, y: f64) -> Option<usize> {
    if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
        return None;
    }
    let i = (y / BIN_ROW_H as f64) as usize;
    (i < tl.sources.len()).then_some(i)
}

/// The word a click on bin row `i` sends: SR852's `insert <reel> 0 <duration>` (the reel's whole range at the playhead).
pub fn bin_word(tl: &Timeline, i: usize) -> Option<alloc::string::String> {
    bin_range_word(tl, i, None) // STRIABINTHREE (SR9025): the whole reel is the range word's default (file tail)
}

/// Paint the bin on a `w` x `h` canvas in `order`: [`BG`], one [`ROW`]-striped row per source with its swatch and, through
/// `labels`, `"<reel>  <HH:MM:SS:FF>  <kind>"`.
pub fn paint_bin(tl: &Timeline, w: u32, h: u32, order: Order, mut labels: Option<&mut dyn Text>) -> Vec<u8> {
    let mut c = Canvas::new(w, h, order);
    c.rect(0, 0, w as i64, h as i64, BG);
    let fps = tl.fps().unwrap_or(25).max(1);
    let lx = BIN_PAD * 2 + BIN_SWATCH;
    for (i, r) in bin_rows(tl).iter().enumerate() {
        let y0 = bin_row_y(i);
        if y0 >= h as i64 {
            break;
        }
        let y1 = y0 + BIN_ROW_H as i64;
        c.rect(0, y0, w as i64, y1, ROW[i % 2]);
        c.rect(BIN_PAD, y0 + 3, BIN_PAD + BIN_SWATCH, y1 - 3, r.swatch());
        if let Some(text) = labels.as_mut() {
            let s = alloc::format!("{}  {}  {}", r.reel, edl::timecode(r.duration, fps), r.kind.name());
            text.text(&mut c, &s, lx as f32, y0 as f32 + 13.0, (lx as i32, w as i32));
        }
    }
    c.px
}

#[cfg(test)]
mod striaprojectbin_tests {
    use super::*;
    use crate::{Clip, SourceId, Timeline};

    /// A (on V1 and A1), B (V1 only), C (no track), D (A1 only) at 25 fps.
    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_source("C", 25);
        let d = tl.add_source("D", 30);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(d, 0, 10, 10)];
        tl
    }

    fn px(c: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [c[i], c[i + 1], c[i + 2], c[i + 3]]
    }

    #[test]
    fn striaprojectbin_rows_kind_duration_and_word() {
        let tl = tl();
        let rows = bin_rows(&tl);
        let got: Vec<(&str, i64, &str)> = rows.iter().map(|r| (r.reel.as_str(), r.duration, r.kind.name())).collect();
        assert_eq!(got, alloc::vec![("A", 50, "AV"), ("B", 50, "V"), ("C", 25, "-"), ("D", 30, "A")]);
        assert_eq!(rows[3].src, SourceId(3));
        assert_eq!(edl::timecode(rows[0].duration, 25), "00:00:02:00");
        assert_eq!(bin_word(&tl, 1).as_deref(), Some("insert B 0 50"));
        assert_eq!(bin_word(&tl, 4), None);
        assert!(bin_rows(&Timeline::new(25)).is_empty());
    }

    #[test]
    fn striaprojectbin_hit_maps_rows_and_refuses_off_canvas() {
        let tl = tl();
        assert_eq!(bin_hit(&tl, 120, 100, 5.0, 0.0), Some(0));
        assert_eq!(bin_hit(&tl, 120, 100, 5.0, 18.0), Some(1));
        assert_eq!(bin_hit(&tl, 120, 100, 119.0, 71.9), Some(3));
        assert_eq!(bin_hit(&tl, 120, 100, 5.0, 72.0), None, "under the last row");
        assert_eq!(bin_hit(&tl, 120, 100, -1.0, 5.0), None);
        assert_eq!(bin_hit(&tl, 120, 100, 120.0, 5.0), None);
        assert_eq!(bin_hit(&tl, 120, 30, 5.0, 40.0), None, "past the canvas");
        for i in 0..4 {
            let (x, y) = bin_point(i);
            assert_eq!(bin_hit(&tl, 120, 100, x, y), Some(i));
        }
        let st = EditorState::new(tl.clone());
        let l = Layout::of_role(Role::Bin, 120, 100, &st);
        assert_eq!(l.preview_h, 0);
        assert_eq!(l.hit(&tl, 5.0, 20.0), Hit::None, "the track view finds nothing on a bin Surface");
    }

    #[test]
    fn striaprojectbin_paint_rows_and_swatches() {
        let tl = tl();
        let (w, h) = (120u32, 100u32);
        let c = paint_bin(&tl, w, h, Order::Rgba, None);
        assert_eq!(c.len(), (w * h * 4) as usize);
        let mid = |i: usize| bin_row_y(i) + BIN_ROW_H as i64 / 2;
        assert_eq!(px(&c, w, BIN_PAD + 1, mid(0)), clip_colour(0, Kind::Video));
        assert_eq!(px(&c, w, BIN_PAD + 1, mid(1)), clip_colour(1, Kind::Video));
        assert_eq!(px(&c, w, BIN_PAD + 1, mid(2)), TICK, "an unused source's swatch is grey");
        assert_eq!(px(&c, w, BIN_PAD + 1, mid(3)), clip_colour(3, Kind::Audio));
        assert_eq!(px(&c, w, 100, mid(0)), ROW[0]);
        assert_eq!(px(&c, w, 100, mid(1)), ROW[1]);
        assert_eq!(px(&c, w, 100, 90), BG, "under the rows");
        let b = paint_bin(&tl, w, h, Order::Bgra, None);
        let s = clip_colour(0, Kind::Video);
        assert_eq!(px(&b, w, BIN_PAD + 1, mid(0)), [s[2], s[1], s[0], s[3]]);
        let short = paint_bin(&tl, w, 20, Order::Rgba, None);
        assert_eq!(short.len(), (w * 20 * 4) as usize, "rows past the canvas are clipped");
    }
}

// ---- STRIATRACKGAINSTEP (LEDGER SR901): the track gain readout badge beside the M / S badges (named tail block) ----
// A track whose gain is not unity shows it at its row's left edge in SR837's badge geometry: a blue box right of the
// M / S badges it carries, the gain as [`db_text`] (`-6.0`, `+2.5`) in 3x5 pixel glyphs (no text hook is guaranteed:
// the kernel and the headless faces pass none). Every track at 0 dB: nothing is touched (every golden unchanged).

/// The track gain badge's colour (blue).
pub const TGAIN: [u8; 4] = [0x50, 0x90, 0xff, 255];
/// The readout glyphs, 3x5 (bit 2 the leftmost column), in the order of [`TGAIN_CHARS`].
const TGAIN_GLYPHS: [[u8; 5]; 13] = [
    [7, 5, 5, 5, 7], [2, 6, 2, 2, 7], [7, 1, 7, 4, 7], [7, 1, 7, 1, 7], [5, 5, 7, 1, 1], [7, 4, 7, 1, 7], [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1], [7, 5, 7, 5, 7], [7, 5, 7, 1, 7], [0, 0, 7, 0, 0], [0, 2, 7, 2, 0], [0, 0, 0, 0, 2],
];
const TGAIN_CHARS: &[u8; 13] = b"0123456789-+.";

/// A gain in centi-dB as its number of dB to the tenth: `+3.0`, `-3.5`, `0.0` (the tenth rounded half up on the
/// magnitude). THE rule: the host face's `db_words` adds the unit to it.
pub fn db_text(cdb: i32) -> alloc::string::String {
    let tenths = (cdb.unsigned_abs() + 5) / 10;
    let sign = if tenths == 0 { "" } else if cdb > 0 { "+" } else { "-" };
    alloc::format!("{sign}{}.{}", tenths / 10, tenths % 10)
}

fn paint_track_gains(c: &mut Canvas, l: Layout, tl: &Timeline) {
    if tl.tracks.iter().all(|t| t.gain_cdb == 0) {
        return;
    }
    for (row, tr) in tl.tracks.iter().enumerate().take(l.rows) {
        if tr.gain_cdb == 0 {
            continue;
        }
        let y0 = l.row_y(row);
        let x = 2 + (BADGE + 2) * (tr.muted as i64 + tr.solo as i64);
        let text = db_text(tr.gain_cdb);
        c.rect(x, y0 + 2, x + 3 + 4 * text.len() as i64, y0 + 2 + BADGE, TGAIN);
        for (i, ch) in text.bytes().enumerate() {
            let Some(g) = TGAIN_CHARS.iter().position(|&k| k == ch) else { continue };
            for (dy, bits) in TGAIN_GLYPHS[g].iter().enumerate() {
                for dx in 0..3i64 {
                    if bits & (4 >> dx) != 0 {
                        let (px, py) = (x + 2 + 4 * i as i64 + dx, y0 + 5 + dy as i64);
                        c.rect(px, py, px + 1, py + 1, FIELD);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod striatrackgainstep_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// Two audio tracks, the same reel's clip on each, 320x120.
    fn twins() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for _ in 0..2 {
            let t = tl.add_track(Kind::Audio);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        }
        EditorState::new(tl)
    }

    #[test]
    fn striatrackgainstep_db_text_pins() {
        assert_eq!(db_text(-600), "-6.0");
        assert_eq!(db_text(250), "+2.5");
        assert_eq!(db_text(-350), "-3.5");
        assert_eq!(db_text(4), "0.0");
        assert_eq!(db_text(-14_400), "-144.0");
    }

    /// M3: A2 at -6 dB carries blue badge pixels on its row only, starting at x 2; with A2 also muted the box starts right
    /// of the M badge; back at 0 dB the canvas is byte-equal to the unity paint.
    #[test]
    fn striatrackgainstep_badge_paint() {
        let st0 = twins();
        let l = Layout::of(320, 120, &st0);
        let px0 = paint_in(&st0, l, Order::Rgba, None, None, None);
        let mut st = st0.clone();
        st.timeline = st.timeline.set_track_gain(1, -600).unwrap();
        let px = paint_in(&st, l, Order::Rgba, None, None, None);
        let at = |p: &[u8], x: i64, y: i64| -> [u8; 4] {
            let i = (y as usize * 320 + x as usize) * 4;
            [p[i], p[i + 1], p[i + 2], p[i + 3]]
        };
        let blue = |p: &[u8], row: usize| (0..60i64).flat_map(|x| (0..ROW_H as i64).map(move |y| (x, y))).filter(|&(x, y)| at(p, x, l.row_y(row) + y) == TGAIN).count();
        assert!(blue(&px, 1) > 0, "the gain badge on A2");
        assert_eq!(blue(&px, 0), 0, "no badge on the unity row");
        assert_eq!(at(&px, 2, l.row_y(1) + 2), TGAIN, "the box at the left edge");
        let mut muted = st.clone();
        muted.timeline = muted.timeline.set_track_flags(1, true, false).unwrap();
        let pm = paint_in(&muted, l, Order::Rgba, None, None, None);
        assert_eq!(at(&pm, 2, l.row_y(1) + 2), MUTE, "the M badge first");
        assert_eq!(at(&pm, 2 + BADGE + 2, l.row_y(1) + 2), TGAIN, "the gain box right of it");
        let mut off = st.clone();
        off.timeline = off.timeline.set_track_gain(1, 0).unwrap();
        assert_eq!(paint_in(&off, l, Order::Rgba, None, None, None), px0, "0 dB: the unity paint, byte for byte");
    }
}

// ---- STRIALINKTRIM (LEDGER SR910): the chain mark on a linked clip's body (named tail block) ----
// Every clip with a nonzero `group` whose body is at least 16 px wide carries two overlapping 1 px rings (5x4 px each,
// 3 px apart) at its top-right, in [`LINK`]; an unlinked clip paints nothing here, so every golden is unchanged. The
// host face and the kernel view band both reach it through [`paint_in`].
/// The chain mark's colour.
pub const LINK: [u8; 4] = [0x9a, 0xe6, 0xff, 255];

/// The chain mark of every linked clip in view (see the block comment).
fn paint_links(c: &mut Canvas, l: Layout, st: &EditorState) {
    for (row, track) in st.timeline.tracks.iter().enumerate() {
        let y0 = l.row_y(row);
        for clip in track.clips.iter().filter(|k| k.group != 0) {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 - x0 < 16 || x1 <= 0 || x0 >= l.w as i64 {
                continue;
            }
            let (xa, ya) = (x1 - 12, y0 + 4);
            c.frame(xa, ya, xa + 5, ya + 4, LINK);
            c.frame(xa + 3, ya, xa + 8, ya + 4, LINK);
        }
    }
}

// ---- STRIAPERFCANVASFILL (LEDGER SR919): one-pass canvas fill and the off-canvas band cull (named tail block) ----
// NLE-PERF-CANVAS-FILL: `paint_in` starts its canvas from BG in one pass ([`Canvas::filled`]) instead of a zero fill then
// a BG rect, and [`Canvas::rect`] fills its first row then copies it down ([`Canvas::fill_rows`]): the same bytes for every
// input. NLE-PERF-ROLE-CULL: a ruler or a track row whose band is wholly off the canvas ([`Layout::band_on`] false) runs no
// tick and shapes no label (the split Preview Surface puts both a band below the canvas). Byte identity of the cull rests
// on the label glyphs' rise: no glyph rises more than 9 px (ruler, baseline `ry + 9`) or 15 px (rows, baseline `y0 + 15`)
// above its baseline (the host's 11 px sans; the kernel's 7x16 cell rises 9, vplay.rs `LABEL_RISE`), so a label of a band
// starting at or below `h` painted nothing before. The kernel's `[nle] view labels=` then counts only the labels on its
// band. no_std + alloc only.

impl Canvas {
    /// A `w` x `h` canvas in `order` every pixel of which is `c` (one pass: no zero fill first).
    pub fn filled(w: u32, h: u32, order: Order, c: [u8; 4]) -> Canvas {
        let pen = Canvas { w: 0, h: 0, order, px: Vec::new() };
        let p = pen.put(c);
        Canvas { w, h, order, px: p.repeat(w as usize * h as usize) }
    }

    /// Fill columns `x0..x1` of rows `y0..y1` (already clamped to the canvas) with the in-order pixel `c`: the first row's
    /// span by one pixel and doubling copies, then that span copied to each later row.
    fn fill_rows(&mut self, x0: usize, x1: usize, y0: usize, y1: usize, c: [u8; 4]) {
        let span = x1.saturating_sub(x0) * 4;
        if span == 0 || y0 >= y1 {
            return;
        }
        let stride = self.w as usize * 4;
        let first = y0 * stride + x0 * 4;
        self.px[first..first + 4].copy_from_slice(&c);
        let mut done = 4;
        while done < span {
            let n = done.min(span - done);
            self.px.copy_within(first..first + n, first + done);
            done += n;
        }
        for y in y0 + 1..y1 {
            self.px.copy_within(first..first + span, y * stride + x0 * 4);
        }
    }
}

impl Layout {
    /// Whether the band `y0..y1` reaches any row of the canvas.
    pub fn band_on(&self, y0: i64, y1: i64) -> bool {
        y1 > 0 && y0 < self.h as i64
    }
}

#[cfg(test)]
mod striaperfcanvasfill_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// The rect loop as it was before SR919: one 4-byte write per pixel.
    fn rect_per_pixel(c: &mut Canvas, x0: i64, y0: i64, x1: i64, y1: i64, col: [u8; 4]) {
        let (x0, x1) = (x0.clamp(0, c.w as i64) as usize, x1.clamp(0, c.w as i64) as usize);
        let (y0, y1) = (y0.clamp(0, c.h as i64) as usize, y1.clamp(0, c.h as i64) as usize);
        let col = c.put(col);
        for y in y0..y1 {
            let row = y * c.w as usize * 4;
            for x in x0..x1 {
                c.px[row + x * 4..row + x * 4 + 4].copy_from_slice(&col);
            }
        }
    }

    #[test]
    fn striaperfcanvasfill_rect_matches_the_per_pixel_loop() {
        let rects: [(i64, i64, i64, i64); 10] = [
            (3, 4, 20, 9),
            (-5, -7, 6, 5),
            (30, 18, 60, 40),
            (12, 10, 4, 15),
            (8, 12, 8, 20),
            (5, 5, 6, 6),
            (0, 0, 37, 23),
            (-100, 2, 100, 3),
            (36, 0, 37, 23),
            (10, 22, 30, 50),
        ];
        for order in [Order::Rgba, Order::Bgra] {
            let mut a = Canvas::new(37, 23, order);
            let mut b = Canvas::new(37, 23, order);
            for (i, &(x0, y0, x1, y1)) in rects.iter().enumerate() {
                let col = [i as u8 * 20, 255 - i as u8 * 7, i as u8 * 3 + 1, 200 + i as u8];
                a.rect(x0, y0, x1, y1, col);
                rect_per_pixel(&mut b, x0, y0, x1, y1, col);
                assert_eq!(a.px, b.px, "rect {i} {:?} in {order:?}", (x0, y0, x1, y1));
            }
            a.frame(2, 3, 30, 20, SELECT);
            rect_per_pixel(&mut b, 2, 3, 30, 4, SELECT);
            rect_per_pixel(&mut b, 2, 19, 30, 20, SELECT);
            rect_per_pixel(&mut b, 2, 3, 3, 20, SELECT);
            rect_per_pixel(&mut b, 29, 3, 30, 20, SELECT);
            assert_eq!(a.px, b.px, "frame in {order:?}");
        }
    }

    #[test]
    fn striaperfcanvasfill_filled_is_new_then_rect() {
        for order in [Order::Rgba, Order::Bgra] {
            for (w, h) in [(37u32, 23u32), (1, 1), (0, 5), (5, 0)] {
                let mut want = Canvas::new(w, h, order);
                want.rect(0, 0, w as i64, h as i64, BG);
                let got = Canvas::filled(w, h, order, BG);
                assert_eq!((got.w, got.h, got.order), (w, h, order));
                assert_eq!(got.px, want.px, "{w}x{h} {order:?}");
            }
        }
    }

    struct Rec(Vec<alloc::string::String>);

    impl Text for Rec {
        fn text(&mut self, _: &mut Canvas, s: &str, _: f32, _: f32, _: (i32, i32)) {
            self.0.push(alloc::string::String::from(s));
        }
    }

    /// `tracks` video tracks, each one clip of source A (0..20) and one of B (20..40).
    fn state(tracks: usize) -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        let b = tl.add_source("B", 100);
        for row in 0..tracks {
            tl.add_track(Kind::Video);
            tl.tracks[row].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 20, 40, 20)];
        }
        EditorState::new(tl)
    }

    fn labels(st: &EditorState, l: Layout) -> (Vec<alloc::string::String>, Vec<u8>) {
        let mut rec = Rec(Vec::new());
        let px = paint_in(st, l, Order::Rgba, Some(&mut rec as &mut dyn Text), None, None);
        (rec.0, px)
    }

    #[test]
    fn striaperfcanvasfill_preview_role_shapes_no_label() {
        let st = state(2);
        let (got, px) = labels(&st, Layout::of_role(Role::Preview, 320, 90, &st));
        assert!(got.is_empty(), "the Preview Surface's ruler and rows are below the canvas: {got:?}");
        assert_eq!(px.len(), 320 * 90 * 4);
        let (whole, _) = labels(&st, Layout::of_role(Role::Whole, 320, 200, &st));
        assert!(whole.iter().any(|s| s == "00:00:00:00"), "{whole:?}");
        assert_eq!(whole.iter().filter(|s| *s == "A" || *s == "B").count(), 4, "{whole:?}");
    }

    #[test]
    fn striaperfcanvasfill_short_tracks_canvas_is_the_tall_ones_top() {
        let st = state(3);
        let (short, sp) = labels(&st, Layout::of_role(Role::Tracks, 320, 56, &st));
        let (tall, tp) = labels(&st, Layout::of_role(Role::Tracks, 320, 200, &st));
        assert_eq!(sp[..], tp[..320 * 56 * 4], "the short canvas is the tall one's first 56 rows");
        assert_eq!(short.iter().filter(|s| *s == "A" || *s == "B").count(), 2, "row 0 only: {short:?}");
        assert_eq!(tall.iter().filter(|s| *s == "A" || *s == "B").count(), 6, "{tall:?}");
        let ruler = |v: &[alloc::string::String]| v.iter().filter(|s| s.contains(':')).count();
        assert_eq!(ruler(&short), ruler(&tall));
    }
}

// ---- STRIAUNDOHISTORYDRAG (LEDGER SR912): a drag's undo entry named by its kind (named tail block) ----
// `Drag::finish` was the one commit road outside any verb, so its undo entry read `(unnamed)` in the history line. The
// public [`Drag::finish`] arms its kind's word through SR876's `EditorState::arm_word` (the ONE name store; an outer
// armed word wins) around the unchanged body (`finish_unnamed`), and disarms after. A marquee or a scrub commits
// nothing and has no word. Both faces (the host face's release, the kernel Player's drag) call this one fn.

impl DragKind {
    /// The word a drag of this kind names its undo entry with; `None` for the kinds that commit nothing.
    pub fn word(self) -> Option<&'static str> {
        match self {
            DragKind::TrimIn => Some("drag trim in"),
            DragKind::TrimOut => Some("drag trim out"),
            DragKind::TransitionIn => Some("drag transition in"),
            DragKind::TransitionOut => Some("drag transition out"),
            DragKind::Move => Some("drag move"),
            DragKind::Slip => Some("drag slip"),
            DragKind::Slide => Some("drag slide"),
            DragKind::Marquee | DragKind::Scrub => None,
        }
    }
}

impl Drag {
    /// THE commit of the gesture: one `trim`, `move_selected` / `move_set` or marquee selection (a scrub commits nothing),
    /// its undo entry named by the drag's kind ([`DragKind::word`]).
    pub fn finish(&self, st: &mut EditorState) -> Result<(), EditError> {
        let armed = self.kind.word().map_or(false, |w| st.arm_word(w));
        let r = self.finish_unnamed(st);
        st.disarm_word(armed);
        r
    }
}

// ---- STRIALINKSLIDE (LEDGER SR927): a linked trim or slide drag excludes its mates' own edges from the snap (named tail block) ----
// A trim or slide of a linked clip carries every mate (`Timeline::linked`), so the mates' edges move with the drag just as
// the primary's do: `snap_delta` excludes them as it excludes the primary (and, for a slide, each mate's butted
// neighbours, as the primary's are excluded); otherwise a small drag snaps back to its mate's still edge (delta 0). A
// move of a linked set already excludes the whole set; group 0 has no mates, so the exclusion is unchanged there.
/// `exclude` grown, for a trim or slide drag of clip `idx` of `track`, by that clip's other link mates (a slide: and each
/// mate's butted neighbours on its own track); `exclude` unchanged for every other drag and for group 0.
fn link_exclude(st: &EditorState, kind: DragKind, track: usize, idx: usize, exclude: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut v = exclude.to_vec();
    if !matches!(kind, DragKind::TrimIn | DragKind::TrimOut | DragKind::Slide) {
        return v;
    }
    let tl = &st.timeline;
    for (k, j) in tl.mates(track, idx) {
        if (k, j) == (track, idx) {
            continue;
        }
        v.push((k, j));
        let Some(tr) = tl.tracks.get(k) else { continue };
        let Some(&m) = tr.clips.get(j) else { continue };
        if kind != DragKind::Slide {
            continue;
        }
        if let Some(p) = j.checked_sub(1).filter(|&i| tr.clips.get(i).is_some_and(|p| p.end() == m.at)) {
            v.push((k, p));
        }
        if tr.clips.get(j + 1).is_some_and(|n| n.at == m.end()) {
            v.push((k, j + 1));
        }
    }
    v
}
// ---- STRIAPERFDRAGSHOWN (LEDGER SR936): the drag preview kept until the drag or the state it reads changes (named tail block) ----

/// STRIAPERFDRAGSHOWN (SR936): [`Drag::shown`] kept across paints. The key is the drag and the five fields of the state
/// `shown` reads (`timeline`, `selection`, `set`, `playhead`, `zoom`); while they are equal the kept preview is handed out
/// (a refcount bump), else `Drag::shown` replays ONCE and the result (a refusal's `None` too) is kept. `replays` counts
/// the replays (the tests' witness, not a wall time).
#[derive(Debug, Default)]
pub struct DragShown {
    key: Option<(Drag, Timeline, Option<(usize, usize)>, Vec<(usize, usize)>, i64, u32)>,
    shown: Option<alloc::sync::Arc<EditorState>>,
    pub replays: u64,
}

impl DragShown {
    /// The preview of `d` over `st` (`None`: no drag, nothing moved, or the op refuses). No drag drops what was kept.
    pub fn get(&mut self, d: Option<&Drag>, st: &EditorState) -> Option<alloc::sync::Arc<EditorState>> {
        let Some(d) = d else {
            self.clear();
            return None;
        };
        if let Some((kd, tl, sel, set, ph, zoom)) = &self.key {
            if kd == d && *sel == st.selection && *ph == st.playhead && *zoom == st.zoom && *set == st.set && *tl == st.timeline {
                return self.shown.clone();
            }
        }
        self.replays += 1;
        self.shown = d.shown(st).map(alloc::sync::Arc::new);
        self.key = Some((*d, st.timeline.clone(), st.selection, st.set.clone(), st.playhead, st.zoom));
        self.shown.clone()
    }
    /// Forget the kept preview (the next [`DragShown::get`] replays); `replays` is kept.
    pub fn clear(&mut self) {
        self.key = None;
        self.shown = None;
    }
}

#[cfg(test)]
mod striaperfdragshown_tests {
    use super::*;
    use crate::Clip;

    /// NLEFXA 0..20 at 0 and NLEFXA 20..30 at 20 (butted), a 40-tick source, the view band `Role::Tracks` 320x56.
    fn fixture() -> (EditorState, Layout, f64, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(a, 20, 30, 20)];
        let st = EditorState::new(tl);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        (st, l, z, y)
    }

    /// STRIAPERFDRAGSHOWN (SR936) M3: one replay per (drag, state) value; a moved pointer, a moved playhead and a fresh
    /// drag after none each replay once; the kept preview is `Drag::shown`'s.
    #[test]
    fn striaperfdragshown_get_replays_once_per_drag_value() {
        let (mut st, l, z, y) = fixture();
        let x = l.x_of(25) as f64 + z / 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap();
        assert_eq!(d.kind, DragKind::Move);
        d.update(&st, x + 3.0 * z, y);
        let mut c = DragShown::default();
        let want = d.shown(&st).expect("the move previews");
        for _ in 0..3 {
            assert_eq!(c.get(Some(&d), &st).as_deref(), Some(&want));
        }
        assert_eq!(c.replays, 1, "three paints, one replay");
        d.update(&st, x + 4.0 * z, y);
        assert_eq!(c.get(Some(&d), &st).as_deref(), d.shown(&st).as_ref());
        assert_eq!(c.replays, 2, "the pointer moved");
        st.playhead = 5;
        let got = c.get(Some(&d), &st).expect("previews");
        assert_eq!((got.playhead, c.replays), (d.shown(&st).unwrap().playhead, 3), "the playhead is a key field");
        assert!(c.get(None, &st).is_none());
        assert_eq!(c.replays, 3, "no drag, no replay");
        assert!(c.get(Some(&d), &st).is_some());
        assert_eq!(c.replays, 4, "a released drag left nothing kept");
    }

    /// STRIAPERFDRAGSHOWN (SR936) M3: a refused slip (the first clip's source window past 0) is kept as `None`.
    #[test]
    fn striaperfdragshown_refused_op_is_cached() {
        let (mut st, l, z, y) = fixture();
        let x = l.x_of(5) as f64 + z / 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, true).unwrap();
        assert_eq!(d.kind, DragKind::Slip);
        d.update(&st, x - 3.0 * z, y);
        assert_eq!(d.delta, -3);
        let mut c = DragShown::default();
        assert!(c.get(Some(&d), &st).is_none());
        assert!(c.get(Some(&d), &st).is_none());
        assert_eq!(c.replays, 1, "the refusal is kept");
    }
}

// ---- STRIABINSCROLL (LEDGER SR939): the bin's row offset — clamp, offset hit, offset paint (named tail block) ----
// The bin's reach: rows past the canvas are reached by a first-shown row `top` (the face's wheel moves it). ONE shared
// offset, [`bin_offset`], feeds both [`bin_hit_from`] and [`paint_bin_from`], and both call STRIAPROJECTBIN's unchanged
// [`bin_hit`] / [`paint_bin`] on a canvas `off` px taller (the paint crops those rows off). `top` is clamped to the list
// ([`bin_clamp`]), so the last row sits at the canvas's bottom at most; `top` 0 paints `paint_bin`'s exact bytes.

/// The largest first-shown row on a bin canvas `h` px tall: the sources past the rows that fit whole (at least one).
pub fn bin_top_max(tl: &Timeline, h: u32) -> usize {
    tl.sources.len().saturating_sub((h / BIN_ROW_H).max(1) as usize)
}

/// `top` clamped to `0..=bin_top_max(tl, h)`.
pub fn bin_clamp(tl: &Timeline, h: u32, top: usize) -> usize {
    top.min(bin_top_max(tl, h))
}

/// THE bin's row offset (px) for a first-shown row `top`: what the paint and the hit both shift by.
pub fn bin_offset(top: usize) -> i64 {
    bin_row_y(top)
}

/// [`bin_hit`] with the rows shifted up by `top` (clamped): the source row under canvas point `(x, y)`.
pub fn bin_hit_from(tl: &Timeline, w: u32, h: u32, top: usize, x: f64, y: f64) -> Option<usize> {
    if y < 0.0 {
        return None;
    }
    let off = bin_offset(bin_clamp(tl, h, top));
    bin_hit(tl, w, h.saturating_add(off as u32), x, y + off as f64)
}

/// [`paint_bin`] with the rows shifted up by `top` (clamped): painted `off` px taller, the first `off` rows dropped.
pub fn paint_bin_from(tl: &Timeline, w: u32, h: u32, top: usize, order: Order, labels: Option<&mut dyn Text>) -> Vec<u8> {
    let off = bin_offset(bin_clamp(tl, h, top)) as u32;
    if off == 0 {
        return paint_bin(tl, w, h, order, labels);
    }
    let mut px = paint_bin(tl, w, h.saturating_add(off), order, labels);
    px.drain(..off as usize * w as usize * 4);
    px
}

#[cfg(test)]
mod striabinscroll_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// A (on V1), B, C, D (on no track) at 25 fps: four rows.
    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_source("B", 50);
        tl.add_source("C", 25);
        tl.add_source("D", 30);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0)];
        tl
    }

    #[test]
    fn striabinscroll_top_clamps_to_the_list() {
        let tl = tl();
        let h = 2 * BIN_ROW_H;
        assert_eq!(bin_top_max(&tl, h), 2);
        assert_eq!(bin_clamp(&tl, h, 1), 1);
        assert_eq!(bin_clamp(&tl, h, 9), 2, "the last row sits at the bottom at most");
        assert_eq!(bin_top_max(&tl, 4 * BIN_ROW_H), 0, "every row fits: no scroll");
        assert_eq!(bin_top_max(&tl, 5), 3, "a canvas under one row still shows one");
        assert_eq!(bin_top_max(&Timeline::new(25), h), 0);
        assert_eq!(bin_offset(2), 2 * BIN_ROW_H as i64);
    }

    #[test]
    fn striabinscroll_hit_shifts_by_the_offset() {
        let tl = tl();
        let (w, h) = (120u32, 2 * BIN_ROW_H);
        for top in 0..3 {
            for i in 0..2 {
                let (x, y) = bin_point(i);
                assert_eq!(bin_hit_from(&tl, w, h, top, x, y), Some(i + top));
            }
        }
        assert_eq!(bin_hit_from(&tl, w, h, 0, 5.0, 5.0), bin_hit(&tl, w, h, 5.0, 5.0), "top 0 is bin_hit");
        assert_eq!(bin_hit_from(&tl, w, h, 9, 5.0, 5.0), Some(2), "top clamped");
        assert_eq!(bin_hit_from(&tl, w, h, 1, 5.0, -1.0), None);
        assert_eq!(bin_hit_from(&tl, w, h, 1, 5.0, h as f64), None, "past the canvas");
        assert_eq!(bin_hit_from(&tl, w, h, 1, w as f64, 5.0), None);
    }

    #[test]
    fn striabinscroll_paint_crops_the_shifted_rows() {
        let tl = tl();
        let (w, h) = (120u32, 2 * BIN_ROW_H);
        assert_eq!(paint_bin_from(&tl, w, h, 0, Order::Rgba, None), paint_bin(&tl, w, h, Order::Rgba, None), "top 0: byte-identical");
        for top in 1..3 {
            let got = paint_bin_from(&tl, w, h, top, Order::Rgba, None);
            assert_eq!(got.len(), (w * h * 4) as usize);
            let tall = paint_bin(&tl, w, h + top as u32 * BIN_ROW_H, Order::Rgba, None);
            assert_eq!(got.as_slice(), &tall[top * BIN_ROW_H as usize * w as usize * 4..]);
        }
        assert_eq!(paint_bin_from(&tl, w, h, 9, Order::Bgra, None), paint_bin_from(&tl, w, h, 2, Order::Bgra, None));
    }
}

// ---- STRIAOPACITYMORE (LEDGER SR958): the clip label names its opacity (named tail block) ----
// A clip below [`crate::OPAQUE`] is labelled `<label> <pct>%`; an opaque clip's label is unchanged, so every labelled
// golden (all opaque) records the same strings. Both faces label through `paint_in`'s `Text` hook.

/// STRIAOPACITYMORE: `label` with ` <pct>%` appended below [`crate::OPAQUE`]; `label` itself at or above it.
pub fn opacity_label(label: alloc::string::String, pct: u8) -> alloc::string::String {
    if pct >= crate::OPAQUE {
        return label;
    }
    alloc::format!("{label} {pct}%")
}

// ---- STRIATHUMBNAILSMORE (LEDGER SR949): a wide video clip's filmstrip and tail (named tail block) ----
// A video clip body at least two heads wide shows more than its head: [`head_tiles`] (THE tile rule, read by BOTH this
// painter and the face's fill, Stria's `Thumbs::fill_strip`) lays the head (`src_in`) at the body's left, the TAIL
// (`src_out - 1`) right-aligned, and between them a FILMSTRIP: a still every head-width px, each the frame under the tile's
// left column (anchored to the clip, so a scroll keeps the keys; a zoom changes them). The gap left of the tail stays the
// colour bar. A narrower body is SR839's head exactly. [`paint_strip`] blits every tile through SR839's unchanged
// [`paint_head`] (Fill, the crop, the off-canvas refusal per tile) via the same `thumb(src, src_tick)` hook; a tile whose
// still is not decoded yet keeps the bar. No hook or no head still: nothing (every golden and the kernel band unchanged).
// Owed: NLE-THUMBNAILS-RATE (with STRIACLIPSPEED SR829 the middle tick becomes `clip.src_at(clip.at + k * sw / zoom)`, the
// one line in `head_tiles`), NLE-THUMBNAILS-STRIP-CACHE (more visible tiles than the face's THUMB_CACHE re-decode each paint).

/// THE tile rule: the stills `(x, w, src_tick)` of a video clip body `x0..x1` whose head still is `sw` px wide at `zoom` px
/// per tick. `sw <= 0` or an empty body: none. A body narrower than `2 * sw`: the head alone, `(x0, min(sw, bw), src_in)`.
/// Otherwise the head `(x0, sw, src_in)`, middles `(x0 + k * sw, sw, src_in + min(k * sw / zoom, len - 1))` for k = 1.. while
/// the tile ends at or before the tail's left, and the tail `(x1 - sw, sw, src_out - 1)`.
pub fn head_tiles(x0: i64, x1: i64, sw: i64, zoom: u32, clip: &crate::Clip) -> Vec<(i64, i64, i64)> {
    let bw = x1 - x0;
    if sw <= 0 || bw <= 0 {
        return Vec::new();
    }
    if bw < 2 * sw {
        return vec![(x0, sw.min(bw), clip.src_in)];
    }
    let (z, last) = (zoom.max(1) as i64, (clip.len() - 1).max(0));
    let mut tiles = vec![(x0, sw, clip.src_in)];
    let mut k = 1i64;
    while x0 + (k + 1) * sw <= x1 - sw {
        tiles.push((x0 + k * sw, sw, clip.src_in + (k * sw / z).min(last)));
        k += 1;
    }
    tiles.push((x1 - sw, sw, clip.src_out - 1));
    tiles
}

/// The video body's stills: the head still's width sets the tiles ([`head_tiles`] at the layout's zoom), each tile blitted by
/// [`paint_head`] into its own `x..x + w` slice of `body` with the clip's `src_in` set to the tile's tick. No head still: nothing.
pub fn paint_strip(c: &mut Canvas, l: &Layout, body: (i64, i64, i64, i64), clip: &crate::Clip, src: &dyn PeakSource) {
    let Some(sw) = src.thumb(clip.src, clip.src_in).map(|s| s.width as i64) else { return };
    let (x0, y0, x1, y1) = body;
    for (x, w, tick) in head_tiles(x0, x1, sw, l.zoom, clip) {
        paint_head(c, (x, y0, x + w, y1), &crate::Clip { src_in: tick, ..*clip }, src);
    }
}

#[cfg(test)]
mod strip_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};
    use alloc::collections::BTreeMap;

    /// A test hook: one distinct still per source tick (none for a tick it does not hold).
    struct PerTick(BTreeMap<i64, Still>);
    impl PeakSource for PerTick {
        fn peaks(&self, _src: SourceId) -> Option<&Peaks> {
            None
        }
        fn gain_q16(&self, _cdb: i32) -> i64 {
            65536
        }
        fn thumb(&self, _src: SourceId, src_tick: i64) -> Option<&Still> {
            self.0.get(&src_tick)
        }
    }

    /// A 48 x 36 still marked by `tick` in its blue channel.
    fn marked(tick: i64) -> Still {
        let (w, h) = (48u32, 36u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 5) as u8, (y * 7) as u8, (0x40 + tick) as u8, 255]);
            }
        }
        Still { width: w, height: h, rgba }
    }

    fn one_clip(clip: Clip) -> EditorState {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![clip];
        EditorState::new(tl)
    }

    fn px(buf: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn box_is(buf: &[u8], w: u32, bx: i64, by: i64, s: &Still) -> bool {
        (0..s.height as i64).all(|y| (0..s.width as i64).all(|x| {
            let i = ((y * s.width as i64 + x) * 4) as usize;
            px(buf, w, bx + x, by + y) == [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2], s.rgba[i + 3]]
        }))
    }

    #[test]
    fn strip_tiles_narrow_one_head_and_filmstrip() {
        let c = Clip::new(SourceId(0), 5, 55, 0);
        assert_eq!(head_tiles(10, 30, 48, 8, &c), alloc::vec![(10, 20, 5)], "narrower than the head: the cropped head");
        assert_eq!(head_tiles(10, 100, 48, 8, &c), alloc::vec![(10, 48, 5)], "under two heads: the head alone");
        assert!(head_tiles(10, 100, 0, 8, &c).is_empty() && head_tiles(10, 10, 48, 8, &c).is_empty());
        let mut want = alloc::vec![(17, 48, 5)];
        for k in 1..=6 {
            want.push((17 + 48 * k, 48, 5 + 6 * k));
        }
        want.push((367, 48, 54));
        assert_eq!(head_tiles(17, 415, 48, 8, &c), want, "head, six middles every 48 px (6 ticks), the tail at src_out - 1");
        // a 3-tick clip at zoom 1 over 300 px: the middle ticks clamp to the clip's last frame
        let short = Clip::new(SourceId(0), 0, 3, 0);
        let t = head_tiles(0, 300, 48, 1, &short);
        assert_eq!(t.len(), 6);
        assert!(t[1..].iter().all(|e| e.2 == 2), "middles and tail clamp to src_out - 1");
    }

    #[test]
    fn strip_paints_head_middles_tail_inside_the_body() {
        let mut m = BTreeMap::new();
        for t in [0i64, 6, 12, 18, 24, 30, 36, 49] {
            m.insert(t, marked(t));
        }
        let hook = PerTick(m);
        // a 50-tick clip at tick 2, zoom 8, on a 480 px canvas: body 17..415
        let st = one_clip(Clip::new(SourceId(0), 0, 50, 2));
        let l = Layout::of(480, 160, &st);
        let none = paint_in(&st, l, Order::Rgba, None, None, None);
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&hook));
        let (x0, x1) = (l.x_of(2) + 1, l.x_of(52) - 1);
        let (y0, y1) = (l.row_y(0) + 2, l.row_y(0) + ROW_H as i64 - 2);
        assert_eq!((x0, x1), (17, 415));
        assert!(box_is(&buf, l.w, x0, y0, &marked(0)), "the head is src_in's still");
        assert!(box_is(&buf, l.w, x0 + 48, y0, &marked(6)), "the first middle is the frame 6 ticks in");
        assert!(box_is(&buf, l.w, x0 + 6 * 48, y0, &marked(36)), "the sixth middle");
        assert!(box_is(&buf, l.w, x1 - 48, y0, &marked(49)), "the tail is src_out - 1");
        let bar = clip_colour(0, Kind::Video);
        for y in y0..y1 {
            assert_eq!(px(&buf, l.w, x1 - 49, y), bar, "the gap left of the tail is the colour bar");
        }
        for y in 0..l.h as i64 {
            for x in 0..l.w as i64 {
                if !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                    assert_eq!(px(&buf, l.w, x, y), px(&none, l.w, x, y), "a strip pixel at {x},{y} outside the body");
                }
            }
        }
        std::println!("striathumbnailsmore: body {x0}..{x1} head + 6 middles + tail, fnv={:016x}", fnv1a(&buf));
    }

    #[test]
    fn strip_missing_tiles_keep_the_bar_and_no_head_paints_nothing() {
        let st = one_clip(Clip::new(SourceId(0), 0, 50, 2));
        let l = Layout::of(480, 160, &st);
        let none = paint_in(&st, l, Order::Rgba, None, None, None);
        let mut m = BTreeMap::new();
        m.insert(49i64, marked(49));
        assert_eq!(paint_in(&st, l, Order::Rgba, None, None, Some(&PerTick(m))), none, "no head still: nothing, the tail neither");
        let mut m = BTreeMap::new();
        m.insert(0i64, marked(0));
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&PerTick(m)));
        let (x0, x1, y0) = (l.x_of(2) + 1, l.x_of(52) - 1, l.row_y(0) + 2);
        assert!(box_is(&buf, l.w, x0, y0, &marked(0)));
        let bar = clip_colour(0, Kind::Video);
        assert!((x0 + 48..x1).all(|x| px(&buf, l.w, x, y0 + 10) == bar), "undecoded middles and tail keep the bar");
    }
}

// ---- STRIAMULTISELECTMORE (LEDGER SR950): the set toggle at a still release, the set dragged to another row (named tail block) ----
// `Drag.toggle` is set by `begin_with` when a shift-press lands on a clip already in the set; the face's still release
// calls [`Drag::release_still`], which takes that clip out (`EditorState::select_remove`: view-only, no commit). A
// shift-drag that moves keeps moving the whole set. A set's Move drag follows the row under the pointer (a track of the
// pressed clip's kind) and commits through `EditorState::move_set_track` (ONE commit, refused whole).
impl Drag {
    /// The press came up where it went down: a shift-press on a set member takes it out of the set. `true` when the set
    /// changed.
    pub fn release_still(&self, st: &mut EditorState) -> bool {
        self.toggle && !self.moved() && st.select_remove(self.track, self.idx)
    }
}

#[cfg(test)]
mod striamultiselectmore_tests {
    use super::*;
    use crate::Clip;

    /// V1: A 0..10 at 0, B 0..10 at 10; A1: A 0..20 at 0; V2 (row 2) empty; the set both V1 clips, primary V1's B; the
    /// tracks band of a 320x136 canvas (three rows).
    fn state() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.select_add(0, 1).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 136, &st);
        (st, l)
    }

    #[test]
    fn striamultiselectmore_drag_set_to_another_row() {
        let (mut st, l) = state();
        let tl0 = st.timeline.clone();
        let (x, y) = l.point(0, 15);
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!((d.kind, d.toggle, st.set.len()), (DragKind::Move, false, 2), "a press on a member keeps the set");
        let (_, y1) = l.point(1, 15);
        d.update(&st, x + 40.0, y1);
        assert_eq!((d.delta, d.to_track), (5, 0), "an audio row is not a video set's row");
        let (_, y2) = l.point(2, 15);
        d.update(&st, x + 40.0, y2);
        assert_eq!((d.delta, d.to_track, d.guide), (5, 2, None));
        let want = tl0.move_many_track(&[(0, 0), (0, 1)], 2, 5).unwrap();
        assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()), "the preview");
        assert_eq!(st.timeline, tl0, "the preview commits nothing");
        assert!(!d.release_still(&mut st));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!((&st.timeline, st.undo_depth()), (&want, 1));
        assert_eq!(st.set, alloc::vec![(2, 0), (2, 1)]);
    }

    #[test]
    fn striamultiselectmore_shift_still_press_on_member_toggles() {
        let (mut st, l) = state();
        let (x, y) = l.point(0, 5);
        let d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert_eq!((d.kind, d.toggle, d.additive), (DragKind::Move, true, true));
        assert!(d.release_still(&mut st));
        assert_eq!((st.selection, st.set.clone()), (Some((0, 1)), alloc::vec![(0, 1)]));
        assert_eq!(st.undo_depth(), 0);
        let (mut st, l) = state();
        let mut d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        d.update(&st, x + 40.0, y);
        assert!(d.moved());
        assert!(!d.release_still(&mut st), "a shift-drag from a member moves the set, it does not toggle");
        assert_eq!(st.set, alloc::vec![(0, 0), (0, 1)]);
        let (mut st, l) = state();
        st.select(0, 1).unwrap();
        let d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert!(!d.toggle, "a shift-press on a non-member adds it");
        assert!(!d.release_still(&mut st));
        assert_eq!(st.set, alloc::vec![(0, 0), (0, 1)]);
    }
}

// ---- STRIAROLLMORE (LEDGER SR954): the roll tool's drag (named tail block) ----
// No modifier and no hit region is free on the selected clip's edge zone, so a roll drag is a FACE TOOL: the host face
// passes its tool flag to [`Drag::rolled`] right after `Drag::begin_with` (whose signature the kernel calls, unchanged).
// A trim-handle drag whose cut has a butted neighbour becomes a roll of that cut: the out handle rolls the cut at the
// clip's out edge, the in handle rolls the previous clip's out cut (the drag is re-aimed at that clip, and `finish`'s
// re-select selects it). `Drag::finish` commits it as ONE `EditorState::roll_selected` (SR851's op), `Drag::shown`
// previews it, and the snap excludes both clips of the cut. No butted neighbour, a ripple or a slide: unchanged.
impl Drag {
    /// The drag as the roll tool sees it (`on`: the tool is on); unchanged when off or when the handle has no butted
    /// neighbour.
    pub fn rolled(self, st: &EditorState, on: bool) -> Drag {
        if !on || self.ripple || !matches!(self.kind, DragKind::TrimIn | DragKind::TrimOut) {
            return self;
        }
        let Some(tr) = st.timeline.tracks.get(self.track) else { return self };
        let Some(&c) = tr.clips.get(self.idx) else { return self };
        if self.kind == DragKind::TrimOut {
            return match tr.clips.get(self.idx + 1) {
                Some(b) if b.at == c.end() => Drag { roll: true, ..self },
                _ => self,
            };
        }
        match self.idx.checked_sub(1).and_then(|i| tr.clips.get(i).map(|a| (i, *a))) {
            Some((i, a)) if a.end() == c.at => Drag { kind: DragKind::TrimOut, idx: i, roll: true, ..self },
            _ => self,
        }
    }
    /// The clips a trim's moving edge does not snap to: the trimmed clip, and for a roll the butted next clip too.
    fn trim_exclude(&self) -> Vec<(usize, usize)> {
        let mut v = vec![(self.track, self.idx)];
        if self.roll {
            v.push((self.track, self.idx + 1));
        }
        v
    }
}

#[cfg(test)]
mod striarollmore_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLEFXA 0..10 at 0, 10..20 at 10, 20..30 at 20 (three butted clips), a 40-tick source, the view band `Role::Tracks`
    /// 320x56; clip `sel` selected.
    fn roll_fixture(sel: usize) -> (Timeline, EditorState, Layout, f64, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10), Clip::new(a, 20, 30, 20)];
        let mut st = EditorState::new(tl.clone());
        st.select(0, sel).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        (tl, st, l, z, y)
    }

    /// STRIAROLLMORE (SR954) M3: the tool on, clip 0's out handle dragged +3 ticks (no snap) is a roll: the preview equals
    /// the commit, which equals `Timeline::roll(0, 0, 3)`; ONE undo restores. The tool off, the same drag is the plain trim.
    #[test]
    fn roll_tool_on_the_out_handle_rolls_the_cut() {
        let (tl, mut st, l, z, y) = roll_fixture(0);
        let x = l.x_of(10) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.idx, d.roll), (DragKind::TrimOut, 0, true));
        d.update(&st, x + 3.0 * z, y);
        assert_eq!(d.delta, 3);
        let shown = d.shown(&st).unwrap().timeline;
        d.finish(&mut st).unwrap();
        assert_eq!(st.timeline, tl.roll(0, 0, 3).unwrap());
        assert_eq!(shown, st.timeline);
        assert_eq!(st.undo_depth(), 1);
        assert!(st.undo());
        assert_eq!(st.timeline, tl);
        let (_, mut st, ..) = roll_fixture(0);
        let d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap().rolled(&st, false);
        assert_eq!((d.kind, d.roll), (DragKind::TrimOut, false));
    }

    /// STRIAROLLMORE (SR954) M3: the tool on, clip 1's IN handle is the previous clip's out cut: the drag is re-aimed at
    /// clip 0 (TrimOut, roll) and its -2 commit is `Timeline::roll(0, 0, -2)` with clip 0 selected.
    #[test]
    fn roll_tool_on_the_in_handle_rolls_the_previous_cut() {
        let (tl, mut st, l, z, y) = roll_fixture(1);
        let x = l.x_of(10) as f64 + 1.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.idx, d.roll), (DragKind::TrimOut, 0, true));
        d.update(&st, x - 2.0 * z, y);
        d.finish(&mut st).unwrap();
        assert_eq!(st.timeline, tl.roll(0, 0, -2).unwrap());
        assert_eq!(st.selection, Some((0, 0)));
    }

    /// STRIAROLLMORE (SR954) M3: the tool leaves alone a handle with no butted neighbour (clip 0's in, clip 2's out), a
    /// ripple (shift) and a slide (the slip modifier).
    #[test]
    fn roll_tool_leaves_gaps_ripple_and_slide() {
        let (_, mut st, l, _, y) = roll_fixture(0);
        let d = Drag::begin_with(&mut st, l, l.x_of(0) as f64 + 1.0, y, false, true, false).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.idx, d.roll), (DragKind::TrimIn, 0, false));
        let d = Drag::begin_with(&mut st, l, l.x_of(10) as f64 - 2.0, y, true, true, false).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.ripple, d.roll), (DragKind::TrimOut, true, false));
        let d = Drag::begin_with(&mut st, l, l.x_of(10) as f64 - 2.0, y, false, true, true).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.roll), (DragKind::Slide, false));
        let (_, mut st, l, _, y) = roll_fixture(2);
        let d = Drag::begin_with(&mut st, l, l.x_of(30) as f64 - 2.0, y, false, true, false).unwrap().rolled(&st, true);
        assert_eq!((d.kind, d.idx, d.roll), (DragKind::TrimOut, 2, false));
    }

    /// STRIAROLLMORE (SR954) M3: a roll's moving cut does not snap to the butted next clip's head (it is the cut itself)
    /// and still snaps to the playhead: the playhead at 13, the pointer 26 px (3.25 ticks, unsnapped +4) past the cut at
    /// 10 puts the edge 2 px from the playhead, so the delta snaps to +3.
    #[test]
    fn roll_snap_skips_its_own_cut_and_takes_the_playhead() {
        let (_, mut st, _, _, _) = roll_fixture(0);
        st.playhead = 13;
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        let x = l.x_of(10) as f64 - 2.0;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap().rolled(&st, true);
        assert_eq!(d.trim_exclude(), alloc::vec![(0, 0), (0, 1)]);
        d.update(&st, x + 3.25 * z, y);
        assert_eq!((d.delta, d.guide), (3, Some(13)));
    }
}

// ---- STRIARIPPLEMORE (LEDGER SR953): a ripple drag snaps the edge that moves (named tail block) ----
// A ripple trim keeps the clip's start, so the edge that MOVES on the timeline is its out edge: by `+delta` for a ripple
// TrimOut and by `-delta` for a ripple TrimIn (the in point eats the clip from inside). `snap_ripple` snaps that edge
// (the pointer's edit point of a ripple trim-in does not move on the timeline, SR850 snapped it), and the later clips of
// the track ride along with the edit, so their old edges are no snap targets; the other tracks and the playhead are.
// This supersedes SR850's "snapping is the plain trim's"; a plain trim snaps as before.

impl Drag {
    /// STRIARIPPLEMORE: snap a ripple trim's moving edge (`c.end()`, moving `s * dx` with `s` = +1 out / -1 in); a hit
    /// on target `t` sets `delta = s * (t - end)` and the guide at `t`.
    fn snap_ripple(&mut self, st: &EditorState, x: f64) {
        let l = self.layout;
        let Some(tr) = st.timeline.tracks.get(self.track) else { return };
        let Some(&c) = tr.clips.get(self.idx) else { return };
        let s: i64 = if self.kind == DragKind::TrimIn { -1 } else { 1 };
        let e = c.end();
        let riders: Vec<(usize, usize)> =
            tr.clips.iter().enumerate().filter(|&(i, n)| i == self.idx || n.at >= e).map(|(i, _)| (self.track, i)).collect();
        let ex = l.x_of(e) as f64 + s as f64 * (x - self.origin.0);
        if let Some(t) = snap_to(&l, &snap_targets(st, &self.all_riders(st, riders, c), true), ex) { // STRIARIPPLETHREE (SR9034): every track's riders under the ripple-all tool (file tail)
            self.delta = s * (t - e);
            self.guide = Some(t);
        }
    }
}

#[cfg(test)]
mod ripple_more_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// A 40-tick source, V1 0..10 at 0, 10..20 at 10, 20..30 at 20 (butted), the view band `Role::Tracks` 320x56 (8 px
    /// per tick, t0 0); clip `sel` selected; the y of row 0's centre.
    fn fixture(sel: usize) -> (Timeline, EditorState, Layout, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10), Clip::new(a, 20, 30, 20)];
        let mut st = EditorState::new(tl.clone());
        st.select(0, sel).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        let y = l.row_y(0) as f64 + ROW_H as f64 / 2.0;
        (tl, st, l, y)
    }

    /// STRIARIPPLEMORE (SR953) M3: shift on clip 1's in handle (x 81, tick 10) dragged +29 px (raw +3): the moving edge is
    /// the OUT edge 20 going left to x 131, 3 px from the playhead at 16, so it snaps there: delta +4 (the in point to 14,
    /// the end to 16), the guide at 16; the release is `ripple_trim_in(0, 1, 14)`.
    #[test]
    fn ripple_in_snaps_the_out_edge() {
        let (tl, mut st, l, y) = fixture(1);
        st.seek(16);
        let mut d = Drag::begin_with(&mut st.clone(), l, 81.0, y, true, false, false).unwrap();
        assert_eq!((d.kind, d.ripple), (DragKind::TrimIn, true));
        d.update(&st, 110.0, y);
        assert_eq!((d.delta, d.guide), (4, Some(16)));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.ripple_trim_in(0, 1, 14).unwrap());
        assert_eq!((st.timeline.tracks[0].clips[1].end(), st.timeline.tracks[0].clips[2].at), (16, 16));
    }

    /// STRIARIPPLEMORE (SR953) M3: clip 0's out handle (x 78, tick 9) dragged +75 px (raw +10): the plain trim snaps the
    /// edge at x 155 to clip 1's end 20 (the guide at 20); the ripple does not (clip 1 and 2 ride along: no stale target),
    /// so its delta is the raw +10 with no guide, and the release is `ripple_trim_out(0, 0, 20)`.
    #[test]
    fn ripple_out_ignores_the_riders_edges() {
        let (tl, mut st, l, y) = fixture(0);
        let mut p = Drag::begin_with(&mut st.clone(), l, 78.0, y, false, false, false).unwrap();
        p.update(&st, 153.0, y);
        assert_eq!((p.kind, p.ripple, p.delta, p.guide), (DragKind::TrimOut, false, 10, Some(20)));
        let mut d = Drag::begin_with(&mut st.clone(), l, 78.0, y, true, false, false).unwrap();
        d.update(&st, 153.0, y);
        assert_eq!((d.kind, d.ripple, d.delta, d.guide), (DragKind::TrimOut, true, 10, None));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.ripple_trim_out(0, 0, 20).unwrap());
    }

    /// STRIARIPPLEMORE (SR953) M3: the no-snap modifier still unsnaps a ripple: the first test's drag reads the raw +3
    /// with no guide, and the release is `ripple_trim_in(0, 1, 13)`.
    #[test]
    fn ripple_in_alt_unsnaps() {
        let (tl, mut st, l, y) = fixture(1);
        st.seek(16);
        let mut d = Drag::begin_with(&mut st.clone(), l, 81.0, y, true, true, false).unwrap();
        d.update(&st, 110.0, y);
        assert_eq!((d.ripple, d.snap, d.delta, d.guide), (true, false, 3, None));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.ripple_trim_in(0, 1, 13).unwrap());
    }
}

// ---- STRIAZOOMFITMORE (LEDGER SR965): the view anchor rule (named tail block) ----
// THE rule both faces apply to a stored view origin (the host face's `Face.view`, the kernel Player's `Edit.view`: a
// wheel zoom's or a fit's `(t0, the playhead it was anchored at)`): the layout keeps that `t0` while the playhead is where
// it was anchored or inside the anchored view, else [`Layout::of`]'s page holding the playhead. `None`: unchanged.
impl Layout {
    /// This layout with the view `view` anchored, by the rule above.
    pub fn anchor(mut self, view: Option<(i64, i64)>, playhead: i64) -> Layout {
        if let Some((t0, at)) = view {
            let per = self.page(); // STRIAZOOMFITTHREE (SR9040): the squeeze, ticks per `zoom` px (file tail)
            if playhead == at || (t0..t0 + per).contains(&playhead) {
                self.t0 = t0;
            }
        }
        self
    }
}

// ---- STRIACROPMORE (LEDGER SR960): the crop drag on the preview (named tail block) ----
// NLE-CROP-DRAG: a press in the preview band within [`CROP_GRAB`] px of an edge of the picture rect `q` (the still letterboxed
// into the band: `Canvas::blit_fit`'s ONE geometry, `SizePolicy::Letterbox.place`; a corner takes two sides) begins a
// [`CropDrag`] when the primary is a video clip at the playhead that the schedule shows (`schedule::top`). A crop side is
// 1/1000 of the output frame (stria's `cropped` crops the frame placed at the output size), so the pointer maps through `q`
// exactly: each dragged side moves by its delta in 1/1000 of `q` (l / t with +x / +y, r / b with -x / -y), clamped 0..=999.
// While live the face paints [`CropDrag::shown`] (the clip uncropped) and [`paint_crop_drag`] (the dragged rect in
// [`CROP_BADGE`]); the release commits ONCE through `EditorState::crop_set` (every selected video clip's dragged sides by the
// same delta: SR900's step words with a free count). The kernel Player never builds one; the kernel Player crops it (SR9010).

/// STRIACROPMORE (SR960): how near (px) a press must come to an edge of the preview picture to grab it.
pub const CROP_GRAB: f64 = 6.0;

/// STRIACROPMORE (SR960): one crop drag on the preview picture (begun by a press, updated by moves, committed ONCE).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropDrag {
    pub track: usize,
    pub idx: usize,
    /// The picture rect in the preview band at the press (canvas px).
    pub q: yuv_core::scale::Rect,
    /// The sides grabbed: 1 left, 2 top, 4 right, 8 bottom.
    pub sides: u8,
    /// The press and the pointer now (canvas px).
    pub origin: (f64, f64),
    pub at: (f64, f64),
    /// The primary's crop at the press.
    pub crop0: crate::Crop,
}

impl CropDrag {
    /// A press at `(x, y)` on layout `l` whose preview still is `sw x sh`: a crop drag when the press is in the preview band
    /// within [`CROP_GRAB`] of the picture's edge and the primary is the video clip shown at the playhead; else `None`.
    pub fn begin(st: &EditorState, l: Layout, sw: u32, sh: u32, x: f64, y: f64) -> Option<CropDrag> {
        let (track, idx) = st.selection?;
        let clip = st.selected()?;
        if sw == 0 || sh == 0 || l.w == 0 || l.preview_h == 0 || x < 0.0 || y < 0.0 || x >= l.w as f64 || y >= l.preview_h as f64 {
            return None;
        }
        if st.timeline.tracks.get(track)?.kind != Kind::Video || !clip.covers(st.playhead) {
            return None;
        }
        if crate::schedule::top(&st.timeline, Kind::Video, st.playhead) != Some((track, idx)) {
            return None;
        }
        let q = crate::SizePolicy::Letterbox.place(sw as usize, sh as usize, l.w as usize, l.preview_h as usize).dst;
        let (x0, y0, x1, y1) = (q.x as f64, q.y as f64, (q.x + q.w) as f64, (q.y + q.h) as f64);
        if x < x0 - CROP_GRAB || x >= x1 + CROP_GRAB || y < y0 - CROP_GRAB || y >= y1 + CROP_GRAB {
            return None;
        }
        let near = |p: f64, e: f64| (p - e).abs() < CROP_GRAB;
        let mut sides = near(x, x0) as u8 | (near(y, y0) as u8) << 1 | (near(x, x1) as u8) << 2 | (near(y, y1) as u8) << 3;
        if sides & 5 == 5 {
            sides &= if (x - x0).abs() <= (x1 - x).abs() { !4 } else { !1 };
        }
        if sides & 10 == 10 {
            sides &= if (y - y0).abs() <= (y1 - y).abs() { !8 } else { !2 };
        }
        let sides = if sides == 0 { crop_move_sides(x, y, q, clip.crop) } else { sides }; if sides == 0 { // STRIACROPTHREE (SR9032): a press inside a cropped rect moves it (file tail)
            return None;
        }
        Some(CropDrag { track, idx, q, sides, origin: (x, y), at: (x, y), crop0: clip.crop })
    }
    /// The pointer is at `(x, y)`.
    pub fn update(&mut self, x: f64, y: f64) {
        self.at = (x, y);
    }
    /// Each side's change since the press, 1/1000 of the picture: `[l, t, r, b]` (0 for a side not grabbed).
    pub fn deltas(&self) -> [i32; 4] {
        let (dx, dy) = (self.at.0 - self.origin.0, self.at.1 - self.origin.1);
        let fx = (dx * 1000.0 / self.q.w.max(1) as f64).round() as i32;
        let fy = (dy * 1000.0 / self.q.h.max(1) as f64).round() as i32;
        let (fx, fy) = self.move_clamp(fx, fy); let on = |b: u8| self.sides & b != 0; // STRIACROPTHREE (SR9032): a move keeps the rect's size (file tail)
        [if on(1) { fx } else { 0 }, if on(2) { fy } else { 0 }, if on(4) { -fx } else { 0 }, if on(8) { -fy } else { 0 }]
    }
    /// The drag would change a side.
    pub fn moved(&self) -> bool {
        self.deltas() != [0; 4]
    }
    /// `c` with the dragged sides moved by [`CropDrag::deltas`], each clamped to 0..=999.
    pub fn apply(&self, c: crate::Crop) -> crate::Crop {
        let d = self.deltas();
        let s = |v: u16, d: i32| (v as i32 + d).clamp(0, 999) as u16;
        crate::Crop { l: s(c.l, d[0]), t: s(c.t, d[1]), r: s(c.r, d[2]), b: s(c.b, d[3]) }
    }
    /// The primary's crop as the drag stands.
    pub fn crop(&self) -> crate::Crop {
        self.apply(self.crop0)
    }
    /// The dragged crop's rect over the picture `q`: `(x0, y0, x1, y1)` canvas px.
    pub fn rect(&self) -> (i64, i64, i64, i64) {
        let c = self.crop();
        let (qx, qy, qw, qh) = (self.q.x as i64, self.q.y as i64, self.q.w as i64, self.q.h as i64);
        (qx + qw * c.l as i64 / 1000, qy + qh * c.t as i64 / 1000, qx + qw - qw * c.r as i64 / 1000, qy + qh - qh * c.b as i64 / 1000)
    }
    /// THE commit of the gesture: ONE `EditorState::crop_set` (nothing when nothing moved); `NoClip` when the primary changed.
    pub fn finish(&self, st: &mut EditorState) -> Result<(), EditError> {
        if !self.moved() {
            return Ok(());
        }
        if st.selection != Some((self.track, self.idx)) {
            return Err(EditError::NoClip);
        }
        st.crop_set(|c| self.apply(c))
    }
    /// The state the face paints while the drag is live: a scratch state (no undo stacks) with the clip uncropped.
    pub fn shown(&self, st: &EditorState) -> Option<EditorState> {
        let mut s = EditorState::new(st.timeline.set_crop(self.track, self.idx, crate::Crop::NONE).ok()?);
        s.selection = st.selection;
        s.set = st.set.clone();
        s.playhead = st.playhead;
        s.zoom = st.zoom;
        Some(s)
    }
}

/// STRIACROPMORE (SR960): the live crop drag's rect as a 1 px [`CROP_BADGE`] outline over the painted canvas; `None` (or a
/// canvas of another size) comes back untouched.
pub fn paint_crop_drag(px: Vec<u8>, l: Layout, order: Order, drag: Option<&CropDrag>) -> Vec<u8> {
    let Some(d) = drag else { return px };
    if px.len() != l.w as usize * l.h as usize * 4 {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    let (x0, y0, x1, y1) = d.rect();
    c.frame(x0, y0, x1.max(x0 + 1), y1.max(y0 + 1), CROP_BADGE);
    c.px
}

#[cfg(test)]
mod striacropmore_tests {
    use super::*;
    use crate::{Clip, Crop, Timeline};

    /// V1: A 0..20 at 0; A1: A 0..20 at 0; V1's clip selected, playhead 5; the preview band 320 x 90 over a 160 x 90 still,
    /// so the picture `q` is 160 x 90 at (80, 0).
    fn fixture() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.playhead = 5;
        let l = Layout::of_role(Role::Preview, 320, 90, &st);
        (st, l)
    }

    #[test]
    fn striacropmore_left_edge_drag_commits_once() {
        let (mut st, l) = fixture();
        let mut d = CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).expect("the left edge");
        assert_eq!((d.q, d.sides), (yuv_core::scale::Rect { w: 160, h: 90, x: 80, y: 0 }, 1));
        d.update(96.0, 45.0);
        assert_eq!(d.crop(), Crop { l: 100, t: 0, r: 0, b: 0 });
        assert_eq!(d.rect(), (96, 0, 240, 90));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!((st.timeline.tracks[0].clips[0].crop, st.undo_depth()), (Crop { l: 100, t: 0, r: 0, b: 0 }, 1));
        assert!(st.undo());
        assert_eq!(st.timeline.tracks[0].clips[0].crop, Crop::NONE);
    }

    #[test]
    fn striacropmore_corner_right_bottom_and_clamp() {
        let (mut st, l) = fixture();
        let mut d = CropDrag::begin(&st, l, 160, 90, 238.0, 88.0).expect("the bottom-right corner");
        assert_eq!(d.sides, 4 | 8);
        d.update(222.0, 79.0);
        assert_eq!(d.crop(), Crop { l: 0, t: 0, r: 100, b: 100 });
        d.update(260.0, 100.0);
        assert_eq!(d.crop(), Crop::NONE, "clamped at 0");
        assert!(d.moved());
        assert_eq!((d.finish(&mut st), st.undo_depth()), (Ok(()), 0), "no change commits nothing");
    }

    #[test]
    fn striacropmore_begin_refusals() {
        let (mut st, l) = fixture();
        assert!(CropDrag::begin(&st, l, 160, 90, 160.0, 45.0).is_none(), "off the edges");
        assert!(CropDrag::begin(&st, l, 160, 90, 20.0, 45.0).is_none(), "outside the picture");
        assert!(CropDrag::begin(&st, l, 160, 90, 80.0, 95.0).is_none(), "below the band");
        st.playhead = 30;
        assert!(CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).is_none(), "the clip not at the playhead");
        st.playhead = 5;
        st.select(1, 0).unwrap();
        assert!(CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).is_none(), "an audio primary");
        st.select(0, 0).unwrap();
        let v2 = st.timeline.add_track(Kind::Video);
        st.timeline.tracks[v2].clips = alloc::vec![Clip::new(crate::SourceId(0), 0, 20, 0)];
        assert!(CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).is_none(), "a higher video track is shown");
        st.deselect();
        assert!(CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).is_none(), "no selection");
    }

    #[test]
    fn striacropmore_paint_none_untouched_and_the_frame_inset() {
        let (st, l) = fixture();
        let px = paint_in(&st, l, Order::Rgba, None, None);
        assert_eq!(paint_crop_drag(px.clone(), l, Order::Rgba, None), px, "no drag: byte-identical");
        let mut d = CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).unwrap();
        d.update(96.0, 45.0);
        let shown = d.shown(&st).unwrap();
        assert_eq!((shown.undo_depth(), shown.selection), (0, st.selection));
        let out = paint_crop_drag(px.clone(), l, Order::Rgba, Some(&d));
        let at = |x: usize, y: usize| out[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4].to_vec();
        assert_eq!(at(96, 45), CROP_BADGE.to_vec());
        assert_eq!(at(239, 45), CROP_BADGE.to_vec());
        assert_eq!(at(150, 0), CROP_BADGE.to_vec());
        assert_eq!(at(150, 45), px[(45 * 320 + 150) * 4..(45 * 320 + 150) * 4 + 4].to_vec(), "inside untouched");
    }
}

// ---- STRIABINMORE (LEDGER SR976): the bin's scroll thumb (named tail block) ----
// When the sources overflow the bin canvas, a [`TICK`] thumb [`BIN_THUMB_W`] px wide at the canvas's right edge shows the
// shown rows' share of the list and where they sit ([`bin_thumb`], `top` clamped by SR939's `bin_clamp`). Paint only: a
// click on its column is the row's click (NLE-BIN-SCROLLBAR-DRAG owed). Every row fitting paints `paint_bin_from`'s exact
// bytes, so the default bin (the face sizes it to every row) is unchanged.

/// The scroll thumb's width (px) at the bin canvas's right edge.
pub const BIN_THUMB_W: i64 = 4;
/// The thumb's least height (px), so a long list still shows one.
pub const BIN_THUMB_MIN: i64 = 6;

/// The thumb's `(y0, y1)` on a bin canvas `h` px tall with first-shown row `top`: `None` when every row fits.
pub fn bin_thumb(tl: &Timeline, h: u32, top: usize) -> Option<(i64, i64)> {
    if bin_top_max(tl, h) == 0 {
        return None;
    }
    let (n, h) = (tl.sources.len() as i64, h as i64);
    let shown = (h / BIN_ROW_H as i64).max(1);
    let top = bin_clamp(tl, h as u32, top) as i64;
    let y0 = h * top / n;
    let y1 = (h * (top + shown) / n).max(y0 + BIN_THUMB_MIN).min(h);
    Some((y0.min(h - BIN_THUMB_MIN).max(0), y1))
}

/// [`paint_bin_from`] with the scroll thumb painted over the right edge when the rows overflow.
pub fn paint_bin_thumb(tl: &Timeline, w: u32, h: u32, top: usize, order: Order, labels: Option<&mut dyn Text>) -> Vec<u8> {
    let px = paint_bin_from(tl, w, h, top, order, labels);
    let Some((y0, y1)) = bin_thumb(tl, h, top) else { return px };
    let mut c = Canvas { w, h, order, px };
    c.rect(w as i64 - BIN_THUMB_W, y0, w as i64, y1, TICK);
    c.px
}

#[cfg(test)]
mod striabinmore_tests {
    use super::*;
    use crate::Timeline;

    /// Four sources A..D at 25 fps, none on a track.
    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        for r in ["A", "B", "C", "D"] {
            tl.add_source(r, 25);
        }
        tl
    }

    fn px(c: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [c[i], c[i + 1], c[i + 2], c[i + 3]]
    }

    #[test]
    fn striabinmore_thumb_geometry() {
        let tl = tl();
        let h = 2 * BIN_ROW_H;
        assert_eq!(bin_thumb(&tl, 4 * BIN_ROW_H, 0), None, "every row fits: no thumb");
        assert_eq!(bin_thumb(&Timeline::new(25), h, 0), None);
        assert_eq!(bin_thumb(&tl, h, 0), Some((0, 18)), "two of four rows: half the canvas, at the top");
        assert_eq!(bin_thumb(&tl, h, 2), Some((18, 36)), "the last rows: at the bottom");
        assert_eq!(bin_thumb(&tl, h, 9), bin_thumb(&tl, h, 2), "top clamped");
        let mut long = Timeline::new(25);
        for i in 0..100 {
            long.add_source(&alloc::format!("R{i}"), 25);
        }
        let (y0, y1) = bin_thumb(&long, h, 0).unwrap();
        assert_eq!((y0, y1), (0, BIN_THUMB_MIN), "a long list keeps the least thumb");
        let (y0, y1) = bin_thumb(&long, h, 99).unwrap();
        assert_eq!((y0, y1), (h as i64 - BIN_THUMB_MIN, h as i64), "the least thumb stays on the canvas");
    }

    #[test]
    fn striabinmore_thumb_paints_only_on_overflow() {
        let tl = tl();
        let w = 120u32;
        let fit = 4 * BIN_ROW_H;
        assert_eq!(paint_bin_thumb(&tl, w, fit, 0, Order::Rgba, None), paint_bin_from(&tl, w, fit, 0, Order::Rgba, None), "no overflow: byte-identical");
        let h = 2 * BIN_ROW_H;
        for top in 0..3 {
            let base = paint_bin_from(&tl, w, h, top, Order::Rgba, None);
            let got = paint_bin_thumb(&tl, w, h, top, Order::Rgba, None);
            assert_eq!(got.len(), base.len());
            let (y0, y1) = bin_thumb(&tl, h, top).unwrap();
            for y in 0..h as i64 {
                let on = y >= y0 && y < y1;
                assert_eq!(px(&got, w, w as i64 - 1, y), if on { TICK } else { px(&base, w, w as i64 - 1, y) }, "top {top} y {y}");
                assert_eq!(px(&got, w, w as i64 - BIN_THUMB_W - 1, y), px(&base, w, w as i64 - BIN_THUMB_W - 1, y), "left of the thumb unchanged");
            }
        }
    }
}

// ---- STRIAXFADEMORE (LEDGER SR980): the crossfade bracket names its curve (named tail block) ----
// SR925 owed NLE-XFADE-CURVE-PAINT: the bracket plots its curve but never names it. With labels on, each explicit audio
// crossfade's bracket (drawn by `paint_audio_xfades`, THE `transition_rect`) names its curve (`Curve::word`: `equal` /
// `linear`) through the painter's `Text` hook, just above the bracket and clipped to its width. Labels off, or no explicit
// record: nothing is drawn, so every labels-off golden and every record-free paint is byte-identical.

/// The curve's word over a bracket `x0..x1` whose top is `y0`; nothing without labels.
fn paint_xfade_name(c: &mut Canvas, labels: &mut Option<&mut dyn Text>, curve: crate::Curve, (x0, y0, x1): (i64, i64, i64)) {
    if let Some(text) = labels.as_mut() {
        text.text(c, curve.word(), x0 as f32 + 2.0, y0 as f32 - 1.0, (x0 as i32, x1 as i32));
    }
}

// ---- STRIAROLLNEXT (LEDGER SR999): the roll tool's cue on the cuts a press would roll (named tail block) ----
// SR954's roll tool had no cue on the band (NLE-ROLL-TOOL-PAINT). The cue asks the press road itself: for the primary's
// drawn in and out handles it runs `Drag::begin_with` (THE press classification) on a probe state holding the same
// Timeline and selection, then SR954's `Drag::rolled` (THE roll predicate), and keeps the cut of every drag that rolls.
// So the cue marks exactly the cuts a press would roll: no second hit test, no second butted-neighbour rule, no Drag
// literal. The host face paints it only while its tool is on and no drag is live; off, the canvas is returned untouched,
// so every golden is byte-identical. The kernel band has no tool and never calls it.

/// STRIAROLLNEXT (SR999): the roll tool's cue over a cut a press would roll.
pub const ROLL_CUE: [u8; 4] = [0x40, 0xff, 0x90, 255];

/// STRIAROLLNEXT (SR999): the cuts `(track, tick)` a press on the primary's in / out handle would roll with the roll tool
/// on (none without a selection or a butted neighbour).
pub fn roll_cuts(st: &EditorState, l: Layout) -> Vec<(usize, i64)> {
    let (Some((track, idx)), Some(c)) = (st.selection, st.selected()) else { return Vec::new() };
    let mut probe = EditorState::new(st.timeline.clone());
    if probe.select(track, idx).is_err() {
        return Vec::new();
    }
    let y = (l.row_y(track) + ROW_H as i64 / 2) as f64;
    let mut v: Vec<(usize, i64)> = Vec::new();
    for x in [l.x_of(c.at) as f64, (l.x_of(c.end()) - 1) as f64] {
        let d = Drag::begin_with(&mut probe, l, x, y, false, true, false).map(|d| d.rolled(&probe, true));
        if let Some(d) = d.filter(|d| d.roll) {
            if let Some(a) = probe.timeline.tracks.get(d.track).and_then(|tr| tr.clips.get(d.idx)) {
                if !v.contains(&(d.track, a.end())) {
                    v.push((d.track, a.end()));
                }
            }
        }
    }
    v
}

/// STRIAROLLNEXT (SR999): the roll cue over a painted canvas: with `on`, a 4 px [`ROLL_CUE`] column straddling each of
/// [`roll_cuts`]' cuts over its row (the two clip edges that move together); off, `px` untouched.
pub fn paint_roll_cue(px: Vec<u8>, l: Layout, order: Order, st: &EditorState, on: bool) -> Vec<u8> {
    if !on {
        return px;
    }
    let cuts = roll_cuts(st, l);
    if cuts.is_empty() {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    for (track, t) in cuts {
        let (x, y0) = (l.x_of(t), l.row_y(track));
        c.rect(x - 2, y0 + 2, x + 2, y0 + ROW_H as i64 - 2, ROLL_CUE);
    }
    c.px
}

#[cfg(test)]
mod striarollnext_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLEFXA 0..10 at 0, 10..20 at 10, 20..30 at 20 (three butted clips) and 32..36 at 32 (a gap before it), the view
    /// band `Role::Tracks` 320x56; clip `sel` selected.
    fn cue_fixture(sel: usize) -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10), Clip::new(a, 20, 30, 20), Clip::new(a, 32, 36, 32)];
        let mut st = EditorState::new(tl);
        st.select(0, sel).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        (st, l)
    }

    /// STRIAROLLNEXT (SR999) M3: the first clip rolls only its out cut, the middle clip both cuts, the gapped last clip
    /// none; no selection, none.
    #[test]
    fn roll_cuts_are_the_cuts_a_press_rolls() {
        let (st, l) = cue_fixture(0);
        assert_eq!(roll_cuts(&st, l), alloc::vec![(0, 10)]);
        let (st, l) = cue_fixture(1);
        assert_eq!(roll_cuts(&st, l), alloc::vec![(0, 10), (0, 20)]);
        let (st, l) = cue_fixture(3);
        assert_eq!(roll_cuts(&st, l), Vec::<(usize, i64)>::new());
        let (mut st, l) = cue_fixture(0);
        st.selection = None;
        assert_eq!(roll_cuts(&st, l), Vec::<(usize, i64)>::new());
    }

    /// STRIAROLLNEXT (SR999) M3: off, the paint is `paint_in`'s byte for byte; on, ROLL_CUE straddles the cut at tick 10.
    #[test]
    fn roll_cue_paints_only_when_on() {
        let (st, l) = cue_fixture(0);
        let base = paint_in(&st, l, Order::Rgba, None, None);
        assert_eq!(paint_roll_cue(base.clone(), l, Order::Rgba, &st, false), base);
        let px = paint_roll_cue(base.clone(), l, Order::Rgba, &st, true);
        let at = |x: i64, y: i64| px[((y * 320 + x) * 4) as usize..((y * 320 + x) * 4 + 4) as usize].to_vec();
        let (x, y) = (l.x_of(10), l.row_y(0) + ROW_H as i64 / 2);
        assert_eq!((at(x - 2, y), at(x + 1, y)), (ROLL_CUE.to_vec(), ROLL_CUE.to_vec()));
        assert_ne!(px, base);
    }

    /// STRIAROLLNEXT (SR999) M3: the probe leaves the caller's state alone (selection, set, undo depth).
    #[test]
    fn roll_cuts_leave_the_state_alone() {
        let (st, l) = cue_fixture(1);
        let before = (st.selection, st.set.clone(), st.undo_depth(), st.timeline.clone());
        let _ = roll_cuts(&st, l);
        assert_eq!((st.selection, st.set.clone(), st.undo_depth(), st.timeline.clone()), before);
    }
}

// ---- STRIAMULTISELECTNEXT (LEDGER SR998): a shift-press on a set member keeps the primary (named tail block) ----
// `begin_with` no longer makes a shift-pressed member primary at the press; `finish` does it (`select_add`, the set kept)
// when the drag commits, so a still release (`release_still`) takes the member out with the user's primary intact.
#[cfg(test)]
mod striamultiselectnext_tests {
    use super::*;
    use crate::Clip;

    /// V1: A 0..10 at 0, B 0..10 at 10, A 20..30 at 20; the set all three, primary the last.
    fn state() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 20, 30, 20)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.select_add(0, 1).unwrap();
        st.select_add(0, 2).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 136, &st);
        (st, l)
    }

    #[test]
    fn striamultiselectnext_toggle_keeps_the_primary() {
        let (mut st, l) = state();
        assert_eq!(st.selection, Some((0, 2)));
        let (x, y) = l.point(0, 15);
        let d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert!(d.toggle);
        assert_eq!((st.selection, st.set.len()), (Some((0, 2)), 3), "the press changes nothing");
        assert!(d.release_still(&mut st));
        assert_eq!((st.selection, st.set.clone()), (Some((0, 2)), alloc::vec![(0, 0), (0, 2)]), "the primary stays");
        assert_eq!(st.undo_depth(), 0);
        let (mut st, l) = state();
        let tl0 = st.timeline.clone();
        let mut d = Drag::begin_with(&mut st, l, x, y, true, true, false).unwrap();
        let (x2, _) = l.point(0, 20);
        d.update(&st, x2, y);
        assert_eq!((d.delta, d.to_track), (5, 0));
        assert!(!d.release_still(&mut st));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl0.move_many(&[(0, 0), (0, 1), (0, 2)], 5).unwrap(), "a shift-drag of a member moves the set");
        assert_eq!((st.selection, st.set.len(), st.undo_depth()), (Some((0, 1)), 3, 1), "the pressed clip is primary after");
    }
}

// ---- STRIAPANNEXT (LEDGER SR994): the pan drag on the pan tick's strip (named tail block) ----
// A press (no shift, no slip modifier) on the selected AUDIO clip's top [`PAN_GRIP_H`] px between its trim handles (the strip
// SR897's [`pan_tick`] paints in) begins a [`DragKind::Pan`]: the pointer's x travel from the press, over [`pan_half`] px per
// full swing (the tick's own geometry), steps the pan from the clip's pan at the press, clamped to -127..=127; the release
// commits ONCE through `EditorState::set_selected_pan` (one undo step; a set of more than one pans its audio clips, SR959).
// The kernel Player does not begin it (its sound road plays pan centred: NLE-PAN-DRAG-KERNEL).

/// STRIAPANNEXT (SR994): the pan grip's height (px) from the clip row's top.
pub const PAN_GRIP_H: i64 = 6;

/// STRIAPANNEXT (SR994): THE pan geometry: the tick's travel (px) from the body's middle to a hard side on a body `x0..x1`.
pub fn pan_half(x0: i64, x1: i64) -> i64 {
    ((x1 - x0) / 2 - HANDLE_W - 1).max(0)
}

/// STRIAPANNEXT (SR994): the selected clip under a press on its pan grip: no shift, no slip, an AUDIO track, y in the row's top
/// [`PAN_GRIP_H`] px, x strictly between the trim handles; `None` otherwise.
fn pan_grip(st: &EditorState, l: &Layout, x: f64, y: f64, shift: bool, slip: bool) -> Option<(usize, usize)> {
    if shift || slip {
        return None;
    }
    let (track, idx) = st.selection?;
    let c = st.selected()?;
    if !matches!(st.timeline.tracks.get(track)?.kind, Kind::Audio) {
        return None;
    }
    let y0 = l.row_y(track) as f64;
    let (x0, x1) = (l.x_of(c.at), l.x_of(c.end()));
    let hw = HANDLE_W as f64;
    (y >= y0 && y < y0 + PAN_GRIP_H as f64 && x >= x0 as f64 + hw && x < x1 as f64 - hw).then_some((track, idx))
}

impl Drag {
    /// STRIAPANNEXT (SR994): the grip's clip pan at the press and its swing (px; at least 1); `None` when the clip is gone.
    fn pan_origin(&self, st: &EditorState) -> Option<(i64, i64)> {
        let c = st.timeline.tracks.get(self.track)?.clips.get(self.idx)?;
        Some((c.pan_cq8 as i64, pan_half(self.layout.x_of(c.at), self.layout.x_of(c.end())).max(1)))
    }
    /// STRIAPANNEXT (SR994): a pan drag's pointer: `delta` = the pan the x travel reaches (rounded, clamped) - the pan at the
    /// press; no snap, no guide, no track change.
    fn pan_update(&mut self, st: &EditorState, x: f64, y: f64) {
        self.at = (x, y);
        self.guide = None;
        let Some((p0, half)) = self.pan_origin(st) else { return };
        let v = (x - self.origin.0) * 127.0 / half as f64;
        let steps = if v >= 0.0 { (v + 0.5) as i64 } else { (v - 0.5) as i64 };
        self.delta = (p0 + steps).clamp(-127, 127) - p0;
    }
    /// STRIAPANNEXT (SR994): a pan drag's ONE commit: the pan at the press + `delta` through `EditorState::set_selected_pan`.
    fn finish_pan(&self, st: &mut EditorState) -> Result<(), EditError> {
        let (p0, _) = self.pan_origin(st).ok_or(EditError::NoClip)?;
        st.set_selected_pan((p0 + self.delta).clamp(-127, 127) as i8)
    }
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): a marker's colour and range bar on the ruler (named tail block) ----
// [`paint_markers`] paints each triangle in [`marker_rgba`] of the marker's colour (index 0 is SR835's [`MARKER`], so a
// default marker paints byte-identical) and, for a range marker (`len > 0`), a [`MARKER_BAR_H`]-row bar along the ruler's
// top from the marker's tick to its end, in the same colour, under the triangle; a point marker paints no bar. Both faces
// paint it through [`paint_in`].

/// STRIAMARKERJUMPKEYSNEXT (SR984): the marker colours' pixels, in `crate::MARKER_COLOURS` order ([0] == [`MARKER`]).
pub const MARKER_RGBA: [[u8; 4]; 8] = [
    MARKER,
    [0xe8, 0x3a, 0x3a, 255],
    [0xf2, 0xd4, 0x2c, 255],
    [0x4c, 0xc2, 0x5a, 255],
    [0x3c, 0xc8, 0xd8, 255],
    [0x46, 0x7c, 0xf0, 255],
    [0xa0, 0x5c, 0xe0, 255],
    [0xf0, 0x6c, 0xb8, 255],
];
/// STRIAMARKERJUMPKEYSNEXT (SR984): a range marker's bar height (px), from the ruler's top.
pub const MARKER_BAR_H: i64 = 2;

/// STRIAMARKERJUMPKEYSNEXT (SR984): the pixel of marker colour `c` (an index past the table paints [`MARKER`]).
pub fn marker_rgba(c: u8) -> [u8; 4] {
    MARKER_RGBA.get(usize::from(c)).copied().unwrap_or(MARKER)
}

/// STRIAMARKERJUMPKEYSNEXT (SR984): the bar of a range marker `m` over the ruler of `l` (canvas width `w`), clipped to
/// `0..w`; nothing for a point marker or a range narrower than a pixel.
fn paint_marker_range(c: &mut Canvas, l: Layout, m: &crate::Marker, w: i64) {
    if m.len <= 0 {
        return;
    }
    let (x0, x1) = (l.x_of(m.at).max(0), l.x_of(m.at + m.len).min(w));
    if x1 > x0 {
        let ry = l.ruler_y as i64;
        c.rect(x0, ry, x1, ry + MARKER_BAR_H, marker_rgba(m.colour));
    }
}

#[cfg(test)]
mod striamarkerjumpkeysnext_tests {
    use super::*;
    use crate::{Clip, Timeline};

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 30, 0)];
        tl.mark(5, "M1").unwrap()
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: an orange point marker paints exactly as before (its colour is index 0); a red one
    /// paints its apex red; a range marker adds a bar only inside the ruler band, ending at the range's end.
    #[test]
    fn striamarkerjumpkeysnext_colour_and_range_paint() {
        assert_eq!((MARKER_RGBA[0], MARKER_RGBA.len()), (MARKER, crate::MARKER_COLOURS.len()));
        let (w, h) = (320u32, 160u32);
        let point = EditorState::new(tl());
        let l = Layout::of(w, h, &point);
        let px = |st: &EditorState| paint(st, w, h, Order::Rgba, None, None);
        let at = |x: i64, y: i64| ((y as usize) * w as usize + x as usize) * 4;
        let base = px(&point);
        let red = px(&EditorState::new(tl().colour_marker(5, 1).unwrap()));
        let (x, ry) = (l.x_of(5), l.ruler_y as i64);
        assert_eq!(red[at(x, ry)..at(x, ry) + 4], MARKER_RGBA[1]);
        let ranged = px(&EditorState::new(tl().range_marker(5, 15).unwrap()));
        let (mut inside, mut outside) = (0usize, 0usize);
        for y in 0..h as usize {
            for xx in 0..w as usize {
                let i = (y * w as usize + xx) * 4;
                if base[i..i + 4] != ranged[i..i + 4] {
                    if (l.ruler_y as usize..l.tracks_y as usize).contains(&y) { inside += 1 } else { outside += 1 }
                }
            }
        }
        assert_eq!(outside, 0, "a range paints nothing outside the ruler");
        assert!(inside > 0, "the bar shows");
        let x1 = l.x_of(20);
        assert_eq!(ranged[at(x1 - 1, ry)..at(x1 - 1, ry) + 4], MARKER, "the bar reaches the range's end");
        assert_eq!(ranged[at(x1, ry)..at(x1, ry) + 4], base[at(x1, ry)..at(x1, ry) + 4], "and stops there");
    }
}

// ---- STRIAKMULTISEL (LEDGER SR9020): the shift marquee the kernel Player now reaches (named tail block) ----
// The Player's press passes Shift into `Drag::begin_with` (vplay.rs `edit_press`), so a shift-press on an empty track
// spot is an ADDITIVE marquee (`finish` -> `EditorState::select_many(.., additive)`), the host face's rule.
#[cfg(test)]
mod striakmultisel_tests {
    use super::*;
    use crate::Clip;

    /// V1: A 0..10 at 0, B 0..10 at 10; A1: A 0..20 at 0; V2 (row 2) empty; the set both V1 clips, primary V1's B; the
    /// tracks band of a 320x136 canvas (three rows).
    fn state() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.select_add(0, 1).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 136, &st);
        (st, l)
    }

    #[test]
    fn striakmultisel_shift_marquee_adds_to_the_set() {
        let (mut st, l) = state();
        let (x, y) = l.point(2, 5);
        let (x1, y1) = l.point(1, 15);
        let mut d = Drag::begin_with(&mut st, l, x, y, true, false, false).unwrap();
        assert_eq!((d.kind, d.additive, d.toggle), (DragKind::Marquee, true, false));
        assert_eq!(st.set, alloc::vec![(0, 0), (0, 1)], "the press selects nothing");
        d.update(&st, x1, y1);
        assert!(d.moved());
        assert!(!d.release_still(&mut st), "a marquee never toggles");
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.set, alloc::vec![(0, 0), (0, 1), (1, 0)], "the audio clip joins the set");
        assert_eq!((st.selection, st.undo_depth()), (Some((0, 1)), 0), "the primary is kept; a selection commits nothing");
        let (mut st, l) = state();
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert!(!d.additive);
        d.update(&st, x1, y1);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!((st.selection, st.set.clone()), (Some((1, 0)), alloc::vec![(1, 0)]), "a plain marquee replaces the set");
    }
}

// ---- STRIAKTC (LEDGER SR9018): the playhead readout text, lifted from nle_face (named tail block) ----
// SR872's `TcUnit` / `tc_text` and SR964's `tc_text_tb` moved byte for byte from `stria::nle_face` (which re-exports them), so
// the host face's message-row readout and the kernel Player's view band (vplay.rs `band_tc`) read ONE text: [`tc_readout`] is
// the face's rate rule (`Timeline::fps` else 25) over `tc_text_tb` (an NTSC base drop-frame through `edl::timecode_drop`).

/// STRIATIMECODEDISPLAY (SR872): the playhead readout's unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcUnit {
    Timecode,
    Frames,
    Seconds,
}

impl TcUnit {
    /// The unit's word (the witness's `"tc"` value).
    pub fn word(self) -> &'static str {
        match self {
            TcUnit::Timecode => "timecode",
            TcUnit::Frames => "frames",
            TcUnit::Seconds => "seconds",
        }
    }
}

/// STRIATIMECODEDISPLAY (SR872): tick `t` at `fps` in `unit`: `HH:MM:SS:FF` ([`edl::timecode`]), the tick count, or `S.CC s`
/// (integer hundredths); a negative tick reads as 0.
pub fn tc_text(t: i64, fps: u32, unit: TcUnit) -> alloc::string::String {
    let (t, fps) = (t.max(0), fps.max(1) as i64);
    match unit {
        TcUnit::Timecode => edl::timecode(t, fps as u32),
        TcUnit::Frames => alloc::format!("{}", t),
        TcUnit::Seconds => {
            let cs = t * 100 / fps;
            alloc::format!("{}.{:02} s", cs / 100, cs % 100)
        }
    }
}

/// STRIATCMORE (SR964): tick `t` in `unit` at the time base `tb`; `fps` is the rate for any base that is not NTSC.
pub fn tc_text_tb(t: i64, tb: crate::Timebase, fps: u32, unit: TcUnit) -> alloc::string::String {
    let k = tb.den / 1000;
    if tb.num != 1001 || tb.den % 1000 != 0 || k == 0 || k > u32::MAX as u64 {
        return tc_text(t, fps, unit);
    }
    let t = t.max(0);
    match unit {
        TcUnit::Timecode if k == 30 || k == 60 => edl::timecode_drop(t, k as u32),
        TcUnit::Timecode => edl::timecode(t, k as u32),
        TcUnit::Frames => alloc::format!("{}", t),
        TcUnit::Seconds => {
            let cs = (t as i128 * 1001 * 100 / tb.den as i128) as i64;
            alloc::format!("{}.{:02} s", cs / 100, cs % 100)
        }
    }
}

/// STRIAKTC (SR9018): THE playhead readout text both faces show: tick `t` of `tl` in `unit`, at `tl`'s rate
/// (`Timeline::fps` else 25; an NTSC base at its real rate).
pub fn tc_readout(tl: &Timeline, t: i64, unit: TcUnit) -> alloc::string::String {
    tc_text_tb(t, tl.tb, tl.fps().unwrap_or(25).max(1), unit)
}

// ---- STRIABINTHREE (LEDGER SR9025): the bin's row range and the thumb's inverse (named tail block) ----
// A drag along a bin row picks a source range: the row's strip ([`bin_strip`], right of the swatch, left of SR976's thumb
// column) maps x to the reel's ticks ([`bin_range`]), and the lay is the SAME word road, [`bin_range_word`] =
// `insert <reel> <in> <out>` (SR852), whose `None` range is the whole reel ([`bin_word`]). A press on SR976's thumb drags the
// first shown row through [`bin_thumb_top`], the inverse of [`bin_thumb`], clamped by SR939's `bin_clamp`. While a range
// drag shows, [`paint_bin_range`] paints a [`HANDLE`] bar under the row; with no drag nothing is painted.

/// The least drag (px) along a row that picks a range; a shorter one is the row's click (the whole reel).
pub const BIN_RANGE_MIN: i64 = 4;

/// The x span `(x0, x1)` of a bin row's range strip on a canvas `w` px wide: the label's x to the thumb column.
pub fn bin_strip(w: u32) -> (i64, i64) {
    (BIN_PAD * 2 + BIN_SWATCH, w as i64 - BIN_THUMB_W)
}

/// The source range `(in, out)` a drag from `x0` to `x1` along bin row `i` picks: each x clamped to [`bin_strip`] and
/// mapped to the reel's ticks (rounded), sorted; `None` off the rows, on a strip of no width, or for an empty range.
pub fn bin_range(tl: &Timeline, w: u32, i: usize, x0: f64, x1: f64) -> Option<(i64, i64)> {
    let dur = tl.sources.get(i)?.duration;
    let (lx, rx) = bin_strip(w);
    let sw = rx - lx;
    if sw <= 0 || dur <= 0 {
        return None;
    }
    let tick = |x: f64| (((x as i64).clamp(lx, rx) - lx) * dur * 2 + sw) / (2 * sw);
    let (a, b) = (tick(x0), tick(x1));
    let (a, b) = (a.min(b), a.max(b));
    (b > a).then_some((a, b))
}

/// The word laying bin row `i`'s range: `insert <reel> <in> <out>`, the whole reel (`0 <duration>`) for `None`.
pub fn bin_range_word(tl: &Timeline, i: usize, range: Option<(i64, i64)>) -> Option<alloc::string::String> {
    tl.sources.get(i).map(|s| {
        let (a, b) = range.unwrap_or((0, s.duration));
        alloc::format!("insert {} {} {}", s.reel, a, b)
    })
}

/// The first shown row whose thumb top sits nearest `y0` on a bin canvas `h` px tall (the inverse of [`bin_thumb`]'s
/// `h * top / n`), clamped by [`bin_clamp`].
pub fn bin_thumb_top(tl: &Timeline, h: u32, y0: f64) -> usize {
    let (n, hh) = (tl.sources.len() as i64, h as i64);
    if hh <= 0 {
        return 0;
    }
    let y = (y0 as i64).max(0);
    bin_clamp(tl, h, ((y * n * 2 + hh) / (2 * hh)) as usize)
}

/// `px` (a bin paint of `w` x `h` from first shown row `top`) with a 3 px [`HANDLE`] bar at the bottom of row `row`
/// between `x0` and `x1`, clamped to [`bin_strip`]: the range a drag picks.
pub fn paint_bin_range(tl: &Timeline, px: Vec<u8>, (w, h): (u32, u32), top: usize, order: Order, row: usize, (x0, x1): (i64, i64)) -> Vec<u8> {
    let (lx, rx) = bin_strip(w);
    let (a, b) = (x0.min(x1).clamp(lx, rx), x0.max(x1).clamp(lx, rx));
    let y1 = bin_row_y(row) - bin_offset(bin_clamp(tl, h, top)) + BIN_ROW_H as i64;
    let mut c = Canvas { w, h, order, px };
    c.rect(a, y1 - 3, b, y1, HANDLE);
    c.px
}

#[cfg(test)]
mod striabinthree_tests {
    use super::*;
    use crate::Timeline;

    /// Four sources A..D (48 ticks each) at 25 fps, none on a track.
    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        for r in ["A", "B", "C", "D"] {
            tl.add_source(r, 48);
        }
        tl
    }

    fn px(c: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [c[i], c[i + 1], c[i + 2], c[i + 3]]
    }

    #[test]
    fn striabinthree_range_maps_the_strip_to_ticks() {
        let tl = tl();
        assert_eq!(bin_strip(120), (20, 116));
        assert_eq!(bin_range(&tl, 120, 1, 20.0, 68.0), Some((0, 24)));
        assert_eq!(bin_range(&tl, 120, 1, 116.0, 68.0), Some((24, 48)), "sorted");
        assert_eq!(bin_range(&tl, 120, 1, 0.0, 500.0), Some((0, 48)), "clamped to the strip");
        assert_eq!(bin_range(&tl, 120, 1, 22.0, 22.0), None, "empty");
        assert_eq!(bin_range(&tl, 120, 4, 20.0, 68.0), None, "off the rows");
        assert_eq!(bin_range(&tl, 24, 0, 20.0, 68.0), None, "a strip of no width");
        assert_eq!(bin_range_word(&tl, 1, Some((3, 9))).as_deref(), Some("insert B 3 9"));
        assert_eq!(bin_range_word(&tl, 1, None), bin_word(&tl, 1));
        assert_eq!(bin_word(&tl, 1).as_deref(), Some("insert B 0 48"));
        assert_eq!(bin_range_word(&tl, 4, None), None);
    }

    #[test]
    fn striabinthree_thumb_top_inverts_the_thumb() {
        let tl = tl();
        let h = 2 * BIN_ROW_H;
        for top in 0..3 {
            let (y0, _) = bin_thumb(&tl, h, top).unwrap();
            assert_eq!(bin_thumb_top(&tl, h, y0 as f64), top, "top {top}");
        }
        assert_eq!(bin_thumb_top(&tl, h, -5.0), 0);
        assert_eq!(bin_thumb_top(&tl, h, 500.0), 2, "clamped");
        assert_eq!(bin_thumb_top(&tl, 0, 9.0), 0);
    }

    #[test]
    fn striabinthree_range_bar_paints_only_its_band() {
        let tl = tl();
        let (w, h) = (120u32, 2 * BIN_ROW_H);
        let base = paint_bin_from(&tl, w, h, 1, Order::Rgba, None);
        let got = paint_bin_range(&tl, base.clone(), (w, h), 1, Order::Rgba, 2, (68, 30));
        let y1 = BIN_ROW_H as i64 * 2;
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let on = (30..68).contains(&x) && (y1 - 3..y1).contains(&y);
                assert_eq!(px(&got, w, x, y), if on { HANDLE } else { px(&base, w, x, y) }, "x {x} y {y}");
            }
        }
    }
}

// ---- STRIACROPTHREE (LEDGER SR9032): the crop drag moves the whole rect (named tail block) ----
// NLE-CROP-DRAG-MOVE: a press inside an already cropped clip's rect over the picture `q` (and away from `q`'s edges, which
// `CropDrag::begin` grabs first) begins a move: `sides` = 15, a value no edge or corner press yields (begin strips the
// opposite pairs). `deltas` then moves l / r by +fx / -fx and t / b by +fy / -fy, clamped by [`CropDrag::move_clamp`] so the
// rect keeps its size inside the frame; the release commits ONCE through `crop_set` as every crop drag does. An uncropped
// clip has nothing to move (its rect is the whole picture): no drag.

/// STRIACROPTHREE (SR9032): 15 (all four sides: a move) when `c` crops and `(x, y)` lies inside `c`'s rect over `q`
/// ([`CropDrag::rect`]'s math), else 0.
fn crop_move_sides(x: f64, y: f64, q: yuv_core::scale::Rect, c: crate::Crop) -> u8 {
    if c == crate::Crop::NONE {
        return 0;
    }
    let (qx, qy, qw, qh) = (q.x as i64, q.y as i64, q.w as i64, q.h as i64);
    let (x0, y0) = ((qx + qw * c.l as i64 / 1000) as f64, (qy + qh * c.t as i64 / 1000) as f64);
    let (x1, y1) = ((qx + qw - qw * c.r as i64 / 1000) as f64, (qy + qh - qh * c.b as i64 / 1000) as f64);
    if x >= x0 && x < x1 && y >= y0 && y < y1 { 15 } else { 0 }
}

impl CropDrag {
    /// STRIACROPTHREE (SR9032): a move's (`sides` 15) deltas clamped so the rect stays in the frame at its size: `fx` in
    /// `-crop0.l ..= crop0.r`, `fy` in `-crop0.t ..= crop0.b`; any other drag's deltas as they came.
    fn move_clamp(&self, fx: i32, fy: i32) -> (i32, i32) {
        if self.sides != 15 {
            return (fx, fy);
        }
        let c = self.crop0;
        (fx.clamp(-(c.l as i32), c.r as i32), fy.clamp(-(c.t as i32), c.b as i32))
    }
}

#[cfg(test)]
mod striacropthree_tests {
    use super::*;
    use crate::{Clip, Crop, Timeline};

    /// V1: A 0..20 at 0, cropped 100 on every side, selected, playhead 5; the preview band 320 x 90 over a 160 x 90 still:
    /// the picture `q` is 160 x 90 at (80, 0), the crop rect (96, 9)..(224, 81).
    fn fixture() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let tl = tl.set_crop(0, 0, Crop { l: 100, t: 100, r: 100, b: 100 }).unwrap();
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.playhead = 5;
        let l = Layout::of_role(Role::Preview, 320, 90, &st);
        (st, l)
    }

    #[test]
    fn striacropthree_move_whole_rect() {
        let (mut st, l) = fixture();
        let mut d = CropDrag::begin(&st, l, 160, 90, 160.0, 45.0).expect("inside the crop rect");
        assert_eq!(d.sides, 15);
        d.update(176.0, 45.0);
        assert_eq!(d.crop(), Crop { l: 200, t: 100, r: 0, b: 100 });
        d.update(200.0, 36.0);
        assert_eq!(d.crop(), Crop { l: 200, t: 0, r: 0, b: 200 }, "clamped: the rect keeps its size inside the frame");
        assert_eq!(d.rect(), (112, 0, 240, 72));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!((st.timeline.tracks[0].clips[0].crop, st.undo_depth()), (Crop { l: 200, t: 0, r: 0, b: 200 }, 1));
        assert!(st.undo());
        assert_eq!(st.timeline.tracks[0].clips[0].crop, Crop { l: 100, t: 100, r: 100, b: 100 });
    }

    #[test]
    fn striacropthree_move_refusals() {
        let (mut st, l) = fixture();
        assert!(CropDrag::begin(&st, l, 160, 90, 90.0, 45.0).is_none(), "inside the picture, outside the crop rect");
        assert_eq!(CropDrag::begin(&st, l, 160, 90, 80.0, 45.0).map(|d| d.sides), Some(1), "the edge still grabs one side");
        st.timeline = st.timeline.set_crop(0, 0, Crop::NONE).unwrap();
        assert!(CropDrag::begin(&st, l, 160, 90, 160.0, 45.0).is_none(), "an uncropped clip: nothing to move");
    }
}

// ---- STRIAXFADETHREE (LEDGER SR9023): the audio crossfade bracket's edge drag (named tail block) ----
// SR980 owed NLE-XFADE-WIDGET: the explicit audio crossfade's bracket (SR860's `paint_audio_xfades`) had its geometry inline
// in the painter, so nothing could hit it. `Layout::xfade_rect` is now THE bracket geometry (the painter's and the hit's,
// on THE transition widget's `transition_rect`), `Layout::xfade_at` the hit. A press within `HANDLE_W` of the drawn left /
// right edge begins `DragKind::XfadeIn` / `XfadeOut` (the incoming clip, whose head cut it sits on, selected), the sibling
// of NLETRANSITIONFACE's edge drag: the moving edge (`at` / `at + dur`) snaps through the same `snap_to` / `snap_targets`,
// and the release is ONE `EditorState::toggle_audio_xfade(Some(dur), Some(curve))` (the curve kept): the right edge gives
// `dur + delta`, the left `dur - delta` (the record stays anchored at the cut), at least 1; past the incoming clip or the
// outgoing's handle the model refuses (`Handle`) and nothing changes. The middle of the bracket stays a clip press. The
// kernel Player names the kinds (vplay.rs `drag_word`) but its press begins only `begin_with` (owed NLE-XFADE-WIDGET-KERNEL).

impl Layout {
    /// STRIAXFADETHREE (SR9023): THE bracket of an explicit audio crossfade, read by `paint_audio_xfades` and
    /// [`Layout::xfade_at`]: the transition widget's rect for a dissolve of the record's `dur` on its cut.
    pub fn xfade_rect(&self, x: &crate::AudioXfade) -> (i64, i64, i64, i64) {
        let bracket = Transition { at: x.at, kind: TransitionKind::Dissolve, dur: x.dur, from_black: false, to_black: false, ease: crate::Ease::Linear, want: None };
        self.transition_rect(x.track, &bracket)
    }
    /// STRIAXFADETHREE (SR9023): the explicit audio crossfade whose bracket is under `(x, y)`: its index in
    /// `Timeline::audio_xfades` (a visible row only; the last drawn wins).
    pub fn xfade_at(&self, tl: &Timeline, x: f64, y: f64) -> Option<usize> {
        let mut hit = None;
        for (i, xf) in tl.audio_xfades.iter().enumerate().filter(|(_, xf)| xf.track < self.rows) {
            let (x0, y0, x1, y1) = self.xfade_rect(xf);
            if x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64 {
                hit = Some(i);
            }
        }
        hit
    }
}

impl Drag {
    /// STRIAXFADETHREE (SR9023): a press at `(x, y)` on the edge zone of an explicit audio crossfade's bracket: the edge
    /// drag (the incoming clip selected; `idx` is that clip); `None` anywhere else (the middle stays a clip press). The left
    /// zone wins where a narrow bracket's zones overlap.
    pub fn begin_xfade_edge(st: &mut EditorState, l: Layout, x: f64, y: f64, nosnap: bool) -> Option<Drag> {
        let xf = *st.timeline.audio_xfades.get(l.xfade_at(&st.timeline, x, y)?)?;
        let (x0, _, x1, _) = l.xfade_rect(&xf);
        let hw = HANDLE_W as f64;
        let kind = if x < x0 as f64 + hw {
            DragKind::XfadeIn
        } else if x >= x1 as f64 - hw {
            DragKind::XfadeOut
        } else {
            return None;
        };
        let ci = st.timeline.tracks.get(xf.track)?.clips.iter().position(|c| c.at == xf.at)?;
        st.select(xf.track, ci).ok()?;
        Some(Drag {
            kind,
            track: xf.track,
            idx: ci,
            origin_tick: l.tick_at(x),
            layout: l,
            delta: 0,
            to_track: xf.track,
            origin: (x, y),
            at: (x, y),
            snap: !nosnap,
            additive: false,
            guide: None,
        })
    }
    /// The record on the head cut of clip `(track, idx)`.
    fn xfade_of(&self, st: &EditorState) -> Option<crate::AudioXfade> {
        let at = st.timeline.tracks.get(self.track)?.clips.get(self.idx)?.at;
        st.timeline.audio_xfades.iter().find(|x| x.track == self.track && x.at == at).copied()
    }
    /// The edge drag's ONE commit: the incoming clip selected, then `toggle_audio_xfade` of the pointer's length, curve kept.
    fn finish_xfade(&self, st: &mut EditorState) -> Result<(), EditError> {
        let x = self.xfade_of(st).ok_or(EditError::Transition)?;
        st.select(self.track, self.idx)?;
        let dur = if self.kind == DragKind::XfadeIn { x.dur - self.delta } else { x.dur + self.delta };
        st.toggle_audio_xfade(Some(dur.max(1)), Some(x.curve))
    }
    /// Snap the moving edge (`at` for the left, `at + dur` for the right) to the nearest clip edge or the playhead within
    /// [`SNAP_PX`], as the transition edge snaps.
    fn snap_xfade(&mut self, st: &EditorState, x: f64) {
        let Some(xf) = self.xfade_of(st) else { return };
        let e = if self.kind == DragKind::XfadeIn { xf.at } else { xf.at + xf.dur };
        let ex = self.layout.x_of(e) as f64 + (x - self.origin.0);
        if let Some(t) = snap_to(&self.layout, &snap_targets(st, &[], true), ex) {
            self.delta = t - e;
            self.guide = Some(t);
        }
    }
}

// ---- STRIAKTHUMBS (LEDGER SR9019): the thumbnail want rule (named tail block) ----
// WHICH still a face decodes next for the stills [`paint_strip`] paints: lifted from Stria's `Thumbs::fill` / `fill_strip`
// so the host face and the kernel Player (vplay.rs `thumbs_wait`) read ONE rule. Pass 1, the HEADS: video rows on the
// canvas, a clip whose body left `x_of(at) + 1` lies in `0..w` and whose body is at least [`THUMB_MIN_W`] px wide, its
// `(src, src_in)` not asked yet. Pass 2, once every such head is asked, the STRIP: a body whose visible part is at least
// [`THUMB_MIN_W`] px; a body starting left of the canvas asks its head first (the head still's width sets the tiles); a
// head asked with no picture asks nothing more; then the [`head_tiles`] after the head whose left column is on the canvas,
// not asked yet. The face owns the cache, the decode and the budget; this fn only names the next key.

/// A clip body narrower than this (px) asks for no head (Stria's `nle_face::THUMB_MIN_W` is this).
pub const THUMB_MIN_W: i64 = 8;

/// THE want rule (the block's head): the next `(src, src_tick)` to decode for layout `l` of `tl`, or `None` when every
/// wanted still is asked. `have(src, tick)`: `None` not asked yet, `Some(None)` asked with no picture, `Some(Some(w))` a
/// still `w` px wide.
pub fn thumb_want(tl: &Timeline, l: &Layout, have: &dyn Fn(crate::SourceId, i64) -> Option<Option<i64>>) -> Option<(crate::SourceId, i64)> {
    let w = l.w as i64;
    let rows = move || {
        tl.tracks.iter().enumerate().filter(|(_, tr)| tr.kind == Kind::Video).filter(move |(row, _)| {
            let y = l.row_y(*row);
            y < l.h as i64 && y + ROW_H as i64 > 0
        })
    };
    for (_, tr) in rows() {
        for c in &tr.clips {
            let (x0, x1) = (l.x_of(c.at) + 1, l.x_of(c.end()) - 1);
            if x0 < 0 || x0 >= w || x1 - x0 < THUMB_MIN_W || have(c.src, c.src_in).is_some() {
                continue;
            }
            return Some((c.src, c.src_in));
        }
    }
    for (_, tr) in rows() {
        for c in &tr.clips {
            let (x0, x1) = (l.x_of(c.at) + 1, l.x_of(c.end()) - 1);
            if x1 <= 0 || x0 >= w || x1.min(w) - x0.max(0) < THUMB_MIN_W {
                continue;
            }
            let sw = match have(c.src, c.src_in) {
                None if x0 >= 0 => continue, // the head pass left it: no strip either
                None => return Some((c.src, c.src_in)),
                Some(None) => continue,
                Some(Some(sw)) => sw,
            };
            for (x, _, tick) in head_tiles(x0, x1, sw, l.zoom, c).into_iter().skip(1) {
                if x < 0 || x >= w || have(c.src, tick).is_some() {
                    continue;
                }
                return Some((c.src, tick));
            }
        }
    }
    None
}

#[cfg(test)]
mod striakthumbs_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    /// Two sources 60 ticks long; track 0 audio, track 1 video holding `clips`.
    fn state(clips: Vec<Clip>) -> EditorState {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 60);
        tl.add_source("B", 60);
        tl.add_track(Kind::Audio);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(SourceId(0), 0, 50, 0)];
        tl.tracks[1].clips = clips;
        EditorState::new(tl)
    }

    /// Every key `thumb_want` names, in order, each answered by a `sw`-wide still (or no picture when `sw` is 0).
    fn drain(tl: &Timeline, l: &Layout, sw: i64) -> Vec<(SourceId, i64)> {
        let held: RefCell<BTreeMap<(SourceId, i64), Option<i64>>> = RefCell::new(BTreeMap::new());
        let mut order = Vec::new();
        while let Some(k) = { let h = held.borrow(); thumb_want(tl, l, &|s, t| h.get(&(s, t)).copied()) } {
            assert!(order.len() < 200, "the want rule must run dry");
            order.push(k);
            held.borrow_mut().insert(k, if sw > 0 { Some(sw) } else { None });
        }
        order
    }

    #[test]
    fn striakthumbs_want_heads_before_tiles() {
        // at zoom 8 on 480 px: A 0..50 at tick 0 (body 1..399), B 5..25 at tick 52 (body 417..575, its head on the canvas)
        let st = state(alloc::vec![Clip::new(SourceId(0), 0, 50, 0), Clip::new(SourceId(1), 5, 25, 52)]);
        let l = Layout::of(480, 200, &st);
        let order = drain(&st.timeline, &l, 48);
        assert_eq!(order[0], (SourceId(0), 0), "A's head first");
        assert_eq!(order[1], (SourceId(1), 5), "then B's head, before any tile");
        let (x0, x1) = (l.x_of(0) + 1, l.x_of(50) - 1);
        let tiles: Vec<(SourceId, i64)> = head_tiles(x0, x1, 48, l.zoom, &st.timeline.tracks[1].clips[0]).into_iter().skip(1).map(|t| (SourceId(0), t.2)).collect();
        assert_eq!(&order[2..2 + tiles.len()], &tiles[..], "then A's tiles in the tile rule's order");
        assert!(drain(&state(Vec::new()).timeline, &l, 48).is_empty(), "an audio row's clip asks nothing");
        std::println!("striakthumbs: {} keys, heads first", order.len());
    }

    #[test]
    fn striakthumbs_want_offcanvas_head_then_tiles() {
        let st = state(alloc::vec![Clip::new(SourceId(0), 0, 50, 0)]);
        let l = Layout { t0: 10, ..Layout::of(480, 200, &st) };
        let order = drain(&st.timeline, &l, 48);
        assert_eq!(order[0], (SourceId(0), 0), "a body starting left of the canvas asks its head first");
        let (x0, x1) = (l.x_of(0) + 1, l.x_of(50) - 1);
        assert!(x0 < 0);
        let want: Vec<(SourceId, i64)> = head_tiles(x0, x1, 48, l.zoom, &st.timeline.tracks[1].clips[0]).into_iter().skip(1).filter(|t| t.0 >= 0 && t.0 < l.w as i64).map(|t| (SourceId(0), t.2)).collect();
        assert_eq!(&order[1..], &want[..], "then only its on-canvas tiles");
    }

    #[test]
    fn striakthumbs_want_skips_narrow_and_nopicture() {
        // a 0-tick-wide body would be refused by Clip; a 1-tick clip at zoom 1 is a body under THUMB_MIN_W
        let mut st = state(alloc::vec![Clip::new(SourceId(0), 0, 1, 0)]);
        st.zoom = 1;
        let l = Layout::of(480, 200, &st);
        assert_eq!(thumb_want(&st.timeline, &l, &|_, _| None), None, "a narrow body asks nothing");
        let st = state(alloc::vec![Clip::new(SourceId(0), 0, 50, 0)]);
        let l = Layout::of(480, 200, &st);
        assert_eq!(drain(&st.timeline, &l, 0), alloc::vec![(SourceId(0), 0)], "a head with no picture: no tiles");
    }

    #[test]
    fn striakthumbs_want_none_when_all_held() {
        let st = state(alloc::vec![Clip::new(SourceId(0), 0, 50, 0), Clip::new(SourceId(1), 0, 10, 50)]);
        let l = Layout::of(480, 200, &st);
        assert_eq!(thumb_want(&st.timeline, &l, &|_, _| Some(Some(48))), None);
        assert_eq!(thumb_want(&st.timeline, &l, &|_, _| Some(None)), None);
    }
}

// ---- STRIARIPPLETHREE (LEDGER SR9034): the sync lock — a ripple drag across every track (named tail block) ----
// Every modifier on the selected clip's edge zone is taken (SR850), so the all-tracks ripple drag is a face TOOL (the roll
// tool's shape, SR954): with the tool on, the face maps the drag a press begins through `Drag::synced`, which marks a
// RIPPLE trim (Shift on the handle) `ripple_all`; `Drag::finish` commits it as SR953's ONE `EditorState::ripple_trim_all`
// (one undo step) and `Drag::shown` previews it. Every other drag is unchanged. Under the tool the other tracks' twins and
// later clips move with the edit too, so `all_riders` leaves them out of the snap targets as SR953 left out the primary
// track's. The kernel never sets the bool: byte-identical there.

impl Drag {
    /// STRIARIPPLETHREE: this drag under the face's ripple-all tool `on`: a ripple trim becomes all-tracks; anything else
    /// (a plain trim, a move, a slip, a slide, a roll) is returned unchanged.
    pub fn synced(self, on: bool) -> Drag {
        let trim = matches!(self.kind, DragKind::TrimIn | DragKind::TrimOut);
        Drag { ripple_all: on && self.ripple && trim, ..self }
    }
    /// STRIARIPPLETHREE: `snap_ripple`'s riders, plus, under `ripple_all`, every other track's twin of `c` (its exact span)
    /// and every clip at or past `c`'s end: all of them move with the edit, so none is a snap target.
    fn all_riders(&self, st: &EditorState, mut riders: Vec<(usize, usize)>, c: crate::Clip) -> Vec<(usize, usize)> {
        if !self.ripple_all {
            return riders;
        }
        let e = c.end();
        for (k, t) in st.timeline.tracks.iter().enumerate().filter(|&(k, _)| k != self.track) {
            riders.extend(t.clips.iter().enumerate().filter(|&(_, n)| n.at >= e || (n.at == c.at && n.end() == e)).map(|(i, _)| (k, i)));
        }
        riders
    }
}

#[cfg(test)]
mod striaripplethree_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// A 40-tick source; V1 0..10 at 0, 10..20 at 10; A1 the same two clips (V1's twins); the view band `Role::Tracks`
    /// 320x96 (8 px per tick, t0 0, row 0 at y 16, row 1 at y 56); V1's clip 0 selected; the y of row 0's centre.
    fn fixture() -> (Timeline, EditorState, Layout, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10)];
        tl.tracks[1].clips = tl.tracks[0].clips.clone();
        let mut st = EditorState::new(tl.clone());
        st.select(0, 0).unwrap();
        let l = Layout::of_role(Role::Tracks, 320, 96, &st);
        let y = l.row_y(0) as f64 + ROW_H as f64 / 2.0;
        (tl, st, l, y)
    }

    /// STRIARIPPLETHREE (SR9034) M3: shift on V1 clip 0's out handle (x 78) under the tool, dragged -24 px (-3 ticks): the
    /// drag is a ripple_all TrimOut, its preview and its release are `ripple_all_out(0, 0, 7)` (A1's twin and both
    /// tracks' later clips follow), ONE undo step; untouched by the tool the same drag ripples V1 only.
    #[test]
    fn striaripplethree_synced_ripple_moves_every_track() {
        let (tl, mut st, l, y) = fixture();
        let mut d = Drag::begin_with(&mut st.clone(), l, 78.0, y, true, true, false).unwrap().synced(true);
        d.update(&st, 54.0, y);
        assert_eq!((d.kind, d.ripple, d.ripple_all, d.delta), (DragKind::TrimOut, true, true, -3));
        let want = tl.ripple_all_out(0, 0, 7).unwrap();
        assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()));
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!((&st.timeline, st.undo_depth()), (&want, 1));
        assert_eq!(want.tracks[1].clips, alloc::vec![Clip::new(want.tracks[1].clips[0].src, 0, 7, 0), Clip::new(want.tracks[1].clips[0].src, 10, 20, 7)]);
        let (tl, mut st, l, y) = fixture();
        let mut d = Drag::begin_with(&mut st.clone(), l, 78.0, y, true, true, false).unwrap().synced(false);
        d.update(&st, 54.0, y);
        assert!(!d.ripple_all);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline, tl.ripple_trim_out(0, 0, 7).unwrap());
    }

    /// STRIARIPPLETHREE (SR9034) M3: the tool leaves every non-ripple drag alone: a plain trim (no shift) and a body move
    /// stay as begun (`synced(true)` changes nothing), and the plain trim's release is the plain `trim`.
    #[test]
    fn striaripplethree_synced_leaves_other_drags() {
        let (tl, mut st, l, y) = fixture();
        let d0 = Drag::begin_with(&mut st.clone(), l, 78.0, y, false, true, false).unwrap();
        assert_eq!(d0.synced(true), d0);
        let mut d = d0.synced(true);
        d.update(&st, 54.0, y);
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.timeline.tracks[0].clips[1], tl.tracks[0].clips[1], "a plain trim moves no later clip");
        assert_eq!(st.timeline.tracks[1], tl.tracks[1]);
        let (_, mut st, l, y) = fixture();
        let m = Drag::begin_with(&mut st, l, 40.0, y, false, false, false).unwrap();
        assert_eq!(m.kind, DragKind::Move);
        assert_eq!(m.synced(true), m);
    }

    /// STRIARIPPLETHREE (SR9034) M3: under the tool the other track's riders are no snap targets: A1's twin and its later
    /// clip are excluded beside V1's own; off the tool A1's clips stay targets (SR953's riders, V1 only).
    #[test]
    fn striaripplethree_all_riders_cover_every_track() {
        let (_, st, l, y) = fixture();
        let d = Drag::begin_with(&mut st.clone(), l, 78.0, y, true, false, false).unwrap();
        let c = st.timeline.tracks[0].clips[0];
        let own = alloc::vec![(0, 0), (0, 1)];
        assert_eq!(d.all_riders(&st, own.clone(), c), own);
        assert_eq!(d.synced(true).all_riders(&st, own.clone(), c), alloc::vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
    }
}

// ---- STRIATCTHREE (LEDGER SR9030): the ruler reads a drop-frame base (named tail block) ----
// `paint_in` rules a 30000/1001 or 60000/1001 timeline at its nominal: a long mark (and label) where the drop-frame label's
// whole second changes (`edl::drop_number`), so the marks sit on `;00` (or `;02` / `;04` past a minute) labels, the text
// through `edl::timecode_drop`. A `1/n` base takes today's expressions (`t % fps == 0`, `edl::timecode`): every golden holds.

/// STRIATCTHREE (SR9030): whether tick `t` takes the ruler's long mark (`fps` the rate `paint_in` rules at).
fn ruler_second(tl: &Timeline, t: i64, fps: i64) -> bool {
    match edl::ntsc_nominal(tl.tb).filter(|k| tl.fps().is_none() && (*k == 30 || *k == 60)) {
        Some(k) => t == 0 || edl::drop_number(t, k) / k as i64 != edl::drop_number(t - 1, k) / k as i64,
        None => t % fps == 0,
    }
}

/// STRIATCTHREE (SR9030): the ruler's label at tick `t`: drop-frame on a drop-frame base, else `edl::timecode`.
fn ruler_tc(tl: &Timeline, t: i64, fps: i64) -> alloc::string::String {
    match edl::ntsc_nominal(tl.tb).filter(|k| tl.fps().is_none() && (*k == 30 || *k == 60)) {
        Some(k) => edl::timecode_drop(t, k),
        None => edl::timecode(t, fps as u32),
    }
}

#[cfg(test)]
mod striatcthree_tests {
    use super::*;
    use crate::{Clip, Kind, Timebase};

    struct Rec(Vec<(alloc::string::String, f32)>);
    impl Text for Rec {
        fn text(&mut self, _: &mut Canvas, s: &str, x: f32, _: f32, _: (i32, i32)) {
            self.0.push((alloc::string::String::from(s), x));
        }
    }

    /// STRIATCTHREE (SR9030) M3: a 30000/1001 timeline's ruler labels read drop-frame, and the long mark past the first minute
    /// is tick 1800 labelled `00:01:00;02` (no mark on the skipped `;00` / `;01`); the same ticks on 1/30 read non-drop.
    #[test]
    fn striatcthree_ruler_reads_drop_frame() {
        let mut tl = Timeline::new(30);
        let a = tl.add_source("A", 4000);
        let v = tl.add_track(Kind::Video);
        tl.tracks[v].clips.push(Clip::new(a, 0, 4000, 0));
        let mut st = EditorState::new(tl.clone());
        let mut l = Layout::of(640, 200, &st);
        l.zoom = 4;
        l.t0 = 1770;
        let labels = |st: &EditorState| {
            let mut r = Rec(Vec::new());
            paint_in(st, l, Order::Rgba, Some(&mut r as &mut dyn Text), None);
            r.0.into_iter().map(|x| x.0).filter(|s| s.len() == 11 && s.as_bytes()[2] == b':').collect::<Vec<_>>()
        };
        let ndf = labels(&st);
        assert!(ndf.contains(&alloc::string::String::from("00:01:00:00")), "{ndf:?}");
        st.timeline.tb = Timebase { num: 1001, den: 30000 };
        let df = labels(&st);
        assert_eq!(df.first().map(|s| s.as_str()), Some("00:00:59;00"), "{df:?}");
        assert!(df.contains(&alloc::string::String::from("00:01:00;02")), "{df:?}");
        assert!(df.iter().all(|s| s.as_bytes()[8] == b';'), "{df:?}");
        assert!(ruler_second(&st.timeline, 1800, 30) && !ruler_second(&st.timeline, 1799, 30) && !ruler_second(&st.timeline, 1801, 30));
        assert!(ruler_second(&st.timeline, 1770, 30), "00:00:59;00 is a whole second");
    }
}

// ---- STRIAZOOMFITTHREE (LEDGER SR9040): the squeezed Layout's page and ruler (named tail block) ----
// `Layout.squeeze` (the editor state's ticks per `zoom` px) is read by every geometry (`x_of`, `tick_at`, `point`, the page
// `Layout::of` and `anchor` hold); at squeeze 1 each expression equals the unsqueezed one, so every golden is unchanged.
// The ruler visits every tick at squeeze 1; squeezed, only the multiples of the smallest `fps * 2^k` ticks whose marks
// stand at least [`RULER_STEP_MIN`] px apart (never a loop over every tick of a long timeline at a far zoom).

/// The closest two squeezed ruler marks may stand (px).
pub const RULER_STEP_MIN: i64 = 8;

impl Layout {
    /// The ticks one window width shows (at least 1).
    pub fn page(&self) -> i64 {
        (self.w as i64 * self.squeeze.max(1) as i64 / self.zoom.max(1) as i64).max(1)
    }
    /// The ticks the ruler visits from `t0` through `t_end`: every one at squeeze 1, else whole seconds (`fps` ticks) in
    /// a power-of-two stride wide enough on screen.
    pub fn ruler_ticks(&self, t_end: i64, fps: i64) -> core::iter::StepBy<core::ops::RangeInclusive<i64>> {
        let sq = self.squeeze.max(1) as i64;
        if sq == 1 {
            return (self.t0..=t_end).step_by(1);
        }
        let mut s = fps.max(1);
        while s * (self.zoom.max(1) as i64) < RULER_STEP_MIN * sq {
            s *= 2;
        }
        let start = -((-self.t0).div_euclid(s)) * s;
        (start..=t_end).step_by(s as usize)
    }
}

// ---- STRIATRACKSTHREE (LEDGER SR9038): the track header column (named tail block) ----
// NLE-TRACK-HEADER, opt-in: a face that asks for it ([`Layout::with_headers`]) gets a column [`HEADER_W`] px wide at the
// left of the ruler and the rows; `left` moves every clip's x right by that width through THE two maps (`x_of` /
// `tick_at`), so the ruler, clips, selection, transitions, drags, snap and the playhead all carry it. `hit` finds nothing
// in the column; [`Layout::header_at`] is the header press (a separate pure query: `Hit` keeps its arms). [`paint_headers`]
// names each row by its channel word (`Timeline::track_word`) through the Text hook. `left` is 0 by default (`Layout::of`),
// the kernel never sets it, and then nothing here paints: every golden is byte-identical.

/// The header column's width (px): room for `V9` / `A9` at the label size.
pub const HEADER_W: u32 = 32;

impl Layout {
    /// STRIATRACKSTHREE (SR9038): this layout with the header column when `on` and the canvas is wider than two columns:
    /// `left` = [`HEADER_W`] and the page holding the playhead re-cut for the narrower track area (`Layout::of`'s rule).
    /// Off, too narrow, or a column already cut: unchanged.
    pub fn with_headers(mut self, on: bool, st: &EditorState) -> Layout {
        if on && self.left == 0 && self.w > 2 * HEADER_W {
            self.left = HEADER_W;
            let per = ((self.w - self.left) / self.zoom.max(1)).max(1) as i64;
            self.t0 = st.playhead.max(0) / per * per;
        }
        self
    }
    /// STRIATRACKSTHREE (SR9038): the track row whose header is under `(x, y)`; `None` with no column or off the column.
    pub fn header_at(&self, x: f64, y: f64) -> Option<usize> {
        if self.left == 0 || x < 0.0 || x >= self.left as f64 || y < self.tracks_y as f64 || y >= self.h as f64 {
            return None;
        }
        let row = ((y - self.tracks_y as f64) / ROW_H as f64) as usize;
        (row < self.rows).then_some(row)
    }
}

/// STRIATRACKSTHREE (SR9038): the header column over whatever ran left of the page: the ruler's corner, one header per row
/// with a one-pixel edge, and the row's channel word. Nothing at all while `l.left` is 0.
fn paint_headers(c: &mut Canvas, l: Layout, st: &EditorState, mut labels: Option<&mut dyn Text>) {
    if l.left == 0 {
        return;
    }
    let left = l.left as i64;
    c.rect(0, l.ruler_y as i64, left, l.tracks_y as i64, RULER);
    for row in 0..st.timeline.tracks.len().min(l.rows) {
        let (y0, y1) = (l.row_y(row), l.row_y(row) + ROW_H as i64);
        c.rect(0, y0, left, y1, RULER);
        c.rect(left - 1, y0, left, y1, TICK);
        if let (Some(text), Some(word)) = (labels.as_mut(), st.timeline.track_word(row)) {
            text.text(c, &word, 4.0, y0 as f32 + 15.0, (0, (left - 1) as i32));
        }
    }
}

// ---- STRIATHUMBNAILSTHREE (LEDGER SR9044): the fade envelope below a tick (named tail block) ----
// [`paint_peaks`] sampled the fade envelope once per tick, so at zoom z > 1 px/tick a fade drew z-column stairs. The mix
// ramps each audio run LINEARLY from `env[0]` to `env[1]` per sample (Stria nle.rs `mix_into`), and runs split at the fade
// edges, which are whole ticks; `schedule::env_q16` is piecewise linear with integer breakpoints. So the envelope at
// `t + r / z` IS `env(t) + (env(t + 1) - env(t)) * r / z`: [`env_sub_q16`] is that, read through the mix's own `env_q16`.
// At one pixel per tick `r` is always 0: the bytes are the old ones.

/// STRIATHUMBNAILSTHREE (SR9044): clip `clip`'s fade envelope (Q16) at the column `dx` px right of its start at `z` px per
/// tick: `env_q16` at the tick under the column, linearly toward the next tick by the column's offset `dx % z` inside it.
fn env_sub_q16(clip: &crate::Clip, dx: i64, z: i64) -> u32 {
    let z = z.max(1);
    let (t, r) = (clip.at + dx.div_euclid(z), dx.rem_euclid(z));
    let e0 = crate::schedule::env_q16(clip, t) as i64;
    if r == 0 {
        return e0 as u32;
    }
    let e1 = crate::schedule::env_q16(clip, t + 1) as i64;
    (e0 + (e1 - e0) * r / z) as u32
}

#[cfg(test)]
mod striathumbnailsthree_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};

    /// A full-scale table and the unity gain (the gain curve is not under test here).
    struct Full(Peaks);
    impl PeakSource for Full {
        fn peaks(&self, _src: SourceId) -> Option<&Peaks> {
            Some(&self.0)
        }
        fn gain_q16(&self, _cdb: i32) -> i64 {
            65536
        }
    }

    #[test]
    fn striathumbnailsthree_env_sub_is_the_tick_at_zoom_one_and_lerps_between() {
        let c = Clip::new(SourceId(0), 0, 50, 10).faded(4, 4);
        for dx in 0..60 {
            assert_eq!(env_sub_q16(&c, dx, 1), crate::schedule::env_q16(&c, c.at + dx), "zoom 1 is the tick: dx {dx}");
            assert_eq!(env_sub_q16(&c, dx * 8, 8), crate::schedule::env_q16(&c, c.at + dx), "a tick's first column is the tick");
        }
        let (e1, e2) = (crate::schedule::env_q16(&c, 11) as i64, crate::schedule::env_q16(&c, 12) as i64);
        assert_eq!(env_sub_q16(&c, 8 + 4, 8) as i64, e1 + (e2 - e1) * 4 / 8, "half way inside tick 1");
        let rising: Vec<u32> = (0..32).map(|dx| env_sub_q16(&c, dx, 8)).collect();
        assert!(rising.windows(2).all(|w| w[0] < w[1]), "the fade-in rises every column: {rising:?}");
        let (fo0, fo1) = (env_sub_q16(&c, 46 * 8 + 3, 8), env_sub_q16(&c, 46 * 8 + 4, 8));
        assert!(fo1 < fo0, "the fade-out falls inside a tick: {fo0} {fo1}");
        std::println!("striathumbnailsthree: env tick1={e1} mid={} tick2={e2}", env_sub_q16(&c, 12, 8));
    }

    #[test]
    fn striathumbnailsthree_fade_in_rises_inside_a_tick() {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(SourceId(0), 0, 50, 0).faded(4, 0)];
        let mut st = EditorState::new(tl);
        st.zoom = 8;
        let l = Layout::of(400, 160, &st);
        let full = Full(Peaks { per_sec: PEAKS_PER_SEC, buckets: alloc::vec![255; 200] });
        let buf = paint_in(&st, l, Order::Rgba, None, None, Some(&full));
        let b = clip_colour(0, Kind::Audio);
        let ink = [b[0] + WAVE_LIFT, b[1] + WAVE_LIFT, b[2] + WAVE_LIFT, 255];
        let (y0, y1) = (l.row_y(0) + 2, l.row_y(0) + ROW_H as i64 - 2);
        let bar = |x: i64| {
            (y0..y1)
                .filter(|&y| {
                    let i = (y as usize * l.w as usize + x as usize) * 4;
                    buf[i..i + 4] == ink
                })
                .count() as i64
        };
        let x = l.x_of(1);
        let bars: Vec<i64> = (x..x + 8).map(bar).collect();
        std::println!("striathumbnailsthree: fade_in=4 zoom=8 tick1 bars={bars:?}");
        assert!(bars.windows(2).all(|w| w[0] <= w[1]), "nondecreasing inside the tick: {bars:?}");
        assert!(bars[7] > bars[0], "the bar grows inside one tick: {bars:?}");
    }
}

// ---- STRIAXFORMTHREE (LEDGER SR9033): the placement drag on the preview (named tail block) ----
// NLE-XFORM-DRAG: a press in the preview band on the rect of a PLACED primary (`Clip::xform` not [`crate::Xform::NONE`]),
// the video clip at the playhead the schedule shows (`schedule::top`), begins an [`XformDrag`]: within [`XFORM_GRAB`] px of
// a corner of the rect it scales the rect with the opposite corner fixed, elsewhere inside it moves the rect. The picture
// rect `q` is the still letterboxed into the band (`SizePolicy::Letterbox.place`, the geometry SR960's crop drag reads),
// so a pointer delta maps to 1/1000 of the frame through `q`; every result passes SR975's ONE clamp
// ([`crate::Xform::clamped`]). While live the face paints [`XformDrag::shown`] (the clip at the dragged placement) and
// [`paint_xform_drag`] (the dragged rect in [`HANDLE`]); the release commits ONCE through `EditorState::set_xform`. A
// press on an unplaced clip, or outside the rect, is not this drag's (SR960's crop drag owns the picture's edges of an
// unplaced clip). The kernel Player never builds one (it refuses a placed timeline).

/// STRIAXFORMTHREE (SR9033): how near (px) a press must come to a corner of the placed rect to scale it.
pub const XFORM_GRAB: f64 = 6.0;

/// STRIAXFORMTHREE (SR9033): one placement drag on the preview picture (begun by a press, updated by moves, committed ONCE).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XformDrag {
    pub track: usize,
    pub idx: usize,
    /// The picture rect in the preview band at the press (canvas px).
    pub q: yuv_core::scale::Rect,
    /// The corner grabbed (bit 1 right, bit 2 bottom: 0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right); `None` moves.
    pub corner: Option<u8>,
    /// The press and the pointer now (canvas px).
    pub origin: (f64, f64),
    pub at: (f64, f64),
    /// The primary's placement at the press.
    pub xf0: crate::Xform,
}

impl XformDrag {
    /// A press at `(x, y)` on layout `l` whose preview still is `sw x sh`: a placement drag when the primary is a placed
    /// video clip shown at the playhead and the press is on its rect (a corner within [`XFORM_GRAB`]); else `None`.
    pub fn begin(st: &EditorState, l: Layout, sw: u32, sh: u32, x: f64, y: f64) -> Option<XformDrag> {
        let (track, idx) = st.selection?;
        let clip = st.selected()?;
        if sw == 0 || sh == 0 || l.w == 0 || l.preview_h == 0 || x < 0.0 || y < 0.0 || x >= l.w as f64 || y >= l.preview_h as f64 {
            return None;
        }
        if st.timeline.tracks.get(track)?.kind != Kind::Video || clip.xform == crate::Xform::NONE || !clip.covers(st.playhead) {
            return None;
        }
        if crate::schedule::top(&st.timeline, Kind::Video, st.playhead) != Some((track, idx)) {
            return None;
        }
        let q = crate::SizePolicy::Letterbox.place(sw as usize, sh as usize, l.w as usize, l.preview_h as usize).dst;
        let d = XformDrag { track, idx, q, corner: None, origin: (x, y), at: (x, y), xf0: clip.xform };
        let (x0, y0, x1, y1) = d.rect();
        let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let mut corner = None;
        let mut best = f64::MAX;
        for (c, cx, cy) in [(0u8, x0, y0), (1, x1, y0), (2, x0, y1), (3, x1, y1)] {
            let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
            if dx < XFORM_GRAB && dy < XFORM_GRAB && dx + dy < best {
                best = dx + dy;
                corner = Some(c);
            }
        }
        if corner.is_none() && (x < x0 || x >= x1 || y < y0 || y >= y1) {
            return None;
        }
        Some(XformDrag { corner, ..d })
    }
    /// The pointer is at `(x, y)`.
    pub fn update(&mut self, x: f64, y: f64) {
        self.at = (x, y);
    }
    /// The placement as the drag stands: the move, or the corner's scale with the opposite corner fixed, through the ONE
    /// clamp ([`crate::Xform::clamped`]).
    pub fn xform(&self) -> crate::Xform {
        let fx = ((self.at.0 - self.origin.0) * 1000.0 / self.q.w.max(1) as f64) as i32;
        let fy = ((self.at.1 - self.origin.1) * 1000.0 / self.q.h.max(1) as f64) as i32;
        let (s, x, y) = (self.xf0.s as i32, self.xf0.x as i32, self.xf0.y as i32);
        let Some(c) = self.corner else { return crate::Xform::clamped(s, x + fx, y + fy) };
        let (right, bottom) = (c & 1 != 0, c & 2 != 0);
        let d = ((if right { fx } else { -fx }) + (if bottom { fy } else { -fy })) / 2;
        let (ax, ay) = (if right { x } else { x + s }, if bottom { y } else { y + s });
        let room = (if right { 1000 - ax } else { ax }).min(if bottom { 1000 - ay } else { ay }).max(1);
        let t = (s + d).clamp(1, room);
        crate::Xform::clamped(t, if right { ax } else { ax - t }, if bottom { ay } else { ay - t })
    }
    /// The drag would change the placement.
    pub fn moved(&self) -> bool {
        self.xform() != self.xf0
    }
    /// The dragged placement's rect over the picture `q`: `(x0, y0, x1, y1)` canvas px.
    pub fn rect(&self) -> (i64, i64, i64, i64) {
        let xf = self.xform();
        let (qx, qy, qw, qh) = (self.q.x as i64, self.q.y as i64, self.q.w as i64, self.q.h as i64);
        let (x0, y0) = (qx + qw * xf.x as i64 / 1000, qy + qh * xf.y as i64 / 1000);
        (x0, y0, x0 + (qw * xf.s as i64 / 1000).max(1), y0 + (qh * xf.s as i64 / 1000).max(1))
    }
    /// THE commit of the gesture: ONE `EditorState::set_xform` (nothing when nothing moved); `NoClip` when the primary
    /// changed.
    pub fn finish(&self, st: &mut EditorState) -> Result<(), crate::EditError> {
        if !self.moved() {
            return Ok(());
        }
        if st.selection != Some((self.track, self.idx)) {
            return Err(crate::EditError::NoClip);
        }
        st.set_xform(self.xform())
    }
    /// The state the face paints while the drag is live: a scratch state (no undo stacks) with the clip at the dragged
    /// placement.
    pub fn shown(&self, st: &EditorState) -> Option<EditorState> {
        let mut s = EditorState::new(st.timeline.set_xform(self.track, self.idx, self.xform()).ok()?);
        s.selection = st.selection;
        s.set = st.set.clone();
        s.playhead = st.playhead;
        s.zoom = st.zoom;
        Some(s)
    }
}

/// STRIAXFORMTHREE (SR9033): the live placement drag's rect as a 1 px [`HANDLE`] outline over the painted canvas; `None` (or
/// a canvas of another size) comes back untouched.
pub fn paint_xform_drag(px: Vec<u8>, l: Layout, order: Order, drag: Option<&XformDrag>) -> Vec<u8> {
    let Some(d) = drag else { return px };
    if px.len() != l.w as usize * l.h as usize * 4 {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    let (x0, y0, x1, y1) = d.rect();
    c.frame(x0, y0, x1, y1, HANDLE);
    c.px
}

#[cfg(test)]
mod striaxformthree_tests {
    use super::*;
    use crate::{Clip, Timeline, Xform};

    /// V1: A 0..20 at 0, placed at [`Xform::PIP`]; A1: A 0..20 at 0; V1's clip selected, playhead 5; the preview band
    /// 320 x 90 over a 160 x 90 still, so the picture `q` is 160 x 90 at (80, 0) and the PIP rect is (196, 65)..(236, 87).
    fn fixture() -> (EditorState, Layout) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let tl = tl.set_xform(0, 0, Xform::PIP).unwrap();
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.playhead = 5;
        let l = Layout::of_role(Role::Preview, 320, 90, &st);
        (st, l)
    }

    /// Inside the rect moves it (16 px of a 160 px picture is 100 / 1000), stopped at the frame's edge; a corner scales it
    /// with the opposite corner fixed; the release is ONE undo step; an unplaced clip, a press off the rect, a clip not at
    /// the playhead begin nothing.
    #[test]
    fn striaxformthree_drag_moves_and_scales() {
        let (mut st, l) = fixture();
        let mut d = XformDrag::begin(&st, l, 160, 90, 216.0, 76.0).expect("inside the rect");
        assert_eq!(d.corner, None);
        assert_eq!(d.rect(), (196, 65, 236, 87));
        d.update(200.0, 76.0);
        assert_eq!(d.xform(), Xform { x: 625, y: 725, s: 250 });
        d.update(232.0, 76.0);
        assert_eq!(d.xform(), Xform { x: 750, y: 725, s: 250 }, "stopped at the right edge");
        d.update(200.0, 76.0);
        d.finish(&mut st).unwrap();
        assert_eq!(st.selected().map(|c| c.xform), Some(Xform { x: 625, y: 725, s: 250 }));
        assert_eq!(st.undo_depth(), 1);

        let (st, l) = fixture();
        let mut br = XformDrag::begin(&st, l, 160, 90, 235.0, 86.0).expect("the bottom-right corner");
        assert_eq!(br.corner, Some(3));
        br.update(219.0, 77.0);
        assert_eq!(br.xform(), Xform { x: 725, y: 725, s: 150 }, "the top-left kept");
        br.update(300.0, 200.0);
        assert_eq!(br.xform(), Xform { x: 725, y: 725, s: 275 }, "grown to the frame's edge");
        let mut tl = XformDrag::begin(&st, l, 160, 90, 197.0, 66.0).expect("the top-left corner");
        assert_eq!(tl.corner, Some(0));
        tl.update(181.0, 57.0);
        assert_eq!(tl.xform(), Xform { x: 625, y: 625, s: 350 }, "the bottom-right kept at 975");
        assert!(!XformDrag::begin(&st, l, 160, 90, 216.0, 76.0).unwrap().moved());

        assert!(XformDrag::begin(&st, l, 160, 90, 120.0, 30.0).is_none(), "off the rect");
        let mut off = st.clone();
        off.playhead = 30;
        assert!(XformDrag::begin(&off, l, 160, 90, 216.0, 76.0).is_none(), "the clip not at the playhead");
        let mut plain = st.clone();
        plain.timeline = plain.timeline.set_xform(0, 0, Xform::NONE).unwrap();
        assert!(XformDrag::begin(&plain, l, 160, 90, 160.0, 45.0).is_none(), "an unplaced clip");
    }

    /// No drag paints nothing (byte-identical); a live drag outlines the dragged rect in HANDLE and shows the clip there.
    #[test]
    fn striaxformthree_paint_none_is_identical() {
        let (st, l) = fixture();
        let px = alloc::vec![0u8; l.w as usize * l.h as usize * 4];
        assert_eq!(paint_xform_drag(px.clone(), l, Order::Rgba, None), px, "no drag: byte-identical");
        let mut d = XformDrag::begin(&st, l, 160, 90, 216.0, 76.0).unwrap();
        d.update(200.0, 76.0);
        let out = paint_xform_drag(px.clone(), l, Order::Rgba, Some(&d));
        let (x0, y0, _, _) = d.rect();
        let o = (y0 as usize * l.w as usize + x0 as usize) * 4;
        assert_eq!(&out[o..o + 4], &HANDLE);
        assert_eq!(d.shown(&st).and_then(|s| s.selected()).map(|c| c.xform), Some(Xform { x: 625, y: 725, s: 250 }));
    }
}

// ---- STRIAWAVEFORMSTEREO (LEDGER SR9051): a stereo clip's two envelopes and the peak pyramid (named tail block) ----
// STEREO: a 2-channel source's table keeps one MONO table per channel in `Peaks.chans` ([L, R], each built by THE ONE
// [`peaks_of`] over its de-interleaved channel; `buckets` stays the folded max), and [`paint_peaks`] hands such a table to
// [`paint_sides`], which calls THE SAME [`paint_peaks`] twice through the [`Side`] adapter: L in the body's upper half, R in
// its lower half (same ink, gain, fade envelope, column walk). Every other table has `chans` empty and paints as before.
// LOD: [`Peaks::finish`] (called once the table is whole: Stria's `nle::scan_peaks`) builds `Peaks.lod`, the max pyramid
// (`lod[k - 1][i]` = max of `buckets[i << k .. (i + 1) << k]`, up to one byte), and [`Peaks::range_max`] answers a column's
// bucket range in O(log n) through it: the SAME value as the linear max, so the bars are byte-identical at every zoom, and a
// squeezed zoom (many ticks per pixel) reads a bounded number of bytes per column. No pyramid (a literal, a bare
// `peaks_of`): the linear max. Owed: NLE-WAVEFORM-MULTICHANNEL (more than two channels stay folded).

/// STRIAWAVEFORMSTEREO (SR9051): the per-channel tables of a 2-channel `interleaved` (empty for any other count), each
/// through THE ONE [`peaks_of`] (`channels` 1, so its own `chans` is empty).
fn sides_of(rate: u32, channels: u16, interleaved: &[f32], per_sec: u32) -> Vec<Peaks> {
    if channels != 2 {
        return Vec::new();
    }
    let (mut lt, mut rt) = (Vec::with_capacity(interleaved.len() / 2), Vec::with_capacity(interleaved.len() / 2));
    for f in interleaved.chunks_exact(2) {
        lt.push(f[0]);
        rt.push(f[1]);
    }
    vec![peaks_of(rate, 1, &lt, per_sec), peaks_of(rate, 1, &rt, per_sec)]
}

impl Peaks {
    /// STRIAWAVEFORMSTEREO (SR9051): `more` (the next stretch of the same source at the same `per_sec`) appended: the
    /// buckets, and each channel's table (taken whole on the first stretch). A pyramid built before is dropped (stale).
    pub fn append(&mut self, more: Peaks) {
        if self.buckets.is_empty() && self.chans.is_empty() {
            self.chans = more.chans;
        } else {
            for (a, b) in self.chans.iter_mut().zip(more.chans) {
                a.append(b);
            }
        }
        self.buckets.extend(more.buckets);
        self.lod.clear();
    }

    /// STRIAWAVEFORMSTEREO (SR9051): build the max pyramid of the whole table (and of each channel's): level `k` halves
    /// level `k - 1` by max (a ragged last pair keeps its one byte) until a level holds one byte.
    pub fn finish(&mut self) {
        let mut lod: Vec<Vec<u8>> = Vec::new();
        let mut len = self.buckets.len();
        while len > 1 {
            let up: Vec<u8> = lod.last().unwrap_or(&self.buckets).chunks(2).map(|c| c.iter().copied().max().unwrap_or(0)).collect();
            len = up.len();
            lod.push(up);
        }
        self.lod = lod;
        for c in &mut self.chans {
            c.finish();
        }
    }

    /// STRIAWAVEFORMSTEREO (SR9051): true when `lod` is the pyramid of `buckets` (each level half the one below, rounded
    /// up, down to one byte).
    fn lod_ok(&self) -> bool {
        let mut len = self.buckets.len();
        for t in &self.lod {
            if len <= 1 || t.len() != len.div_ceil(2) {
                return false;
            }
            len = t.len();
        }
        !self.lod.is_empty() && len == 1
    }

    /// STRIAWAVEFORMSTEREO (SR9051): the max of `buckets[b0..b1]` (0 when empty): through the pyramid in O(log n) when
    /// `finish` built it, else the linear max. Both give the same byte.
    pub fn range_max(&self, b0: usize, b1: usize) -> u8 {
        let b1 = b1.min(self.buckets.len());
        if b0 >= b1 {
            return 0;
        }
        if b1 - b0 <= 8 || !self.lod_ok() {
            return self.buckets[b0..b1].iter().copied().max().unwrap_or(0);
        }
        let (mut lo, mut hi, mut k, mut m) = (b0, b1, 0usize, 0u8);
        while lo < hi {
            let t: &[u8] = if k == 0 { &self.buckets } else { &self.lod[k - 1] };
            if lo & 1 == 1 {
                m = m.max(t[lo]);
                lo += 1;
            }
            if hi & 1 == 1 {
                hi -= 1;
                m = m.max(t[hi]);
            }
            lo >>= 1;
            hi >>= 1;
            k += 1;
        }
        m
    }
}

/// STRIAWAVEFORMSTEREO (SR9051): one channel's table presented as a [`PeakSource`] (the face's ONE gain curve kept).
struct Side<'a>(&'a dyn PeakSource, &'a Peaks);

impl PeakSource for Side<'_> {
    fn peaks(&self, _src: crate::SourceId) -> Option<&Peaks> {
        Some(self.1)
    }
    fn gain_q16(&self, cdb: i32) -> i64 {
        self.0.gain_q16(cdb)
    }
}

/// STRIAWAVEFORMSTEREO (SR9051): a stereo table's two envelopes: `body` split at `y0 + (y1 - y0) / 2`, L painted by THE
/// ONE [`paint_peaks`] in the upper half, R in the lower half.
fn paint_sides(c: &mut Canvas, l: &Layout, body: (i64, i64, i64, i64), clip: &crate::Clip, tb: crate::Timebase, src: &dyn PeakSource, p: &Peaks) {
    let (x0, y0, x1, y1) = body;
    let ym = y0 + (y1 - y0) / 2;
    paint_peaks(c, l, (x0, y0, x1, ym), clip, tb, &Side(src, &p.chans[0]));
    paint_peaks(c, l, (x0, ym, x1, y1), clip, tb, &Side(src, &p.chans[1]));
}

#[cfg(test)]
mod striawaveformstereo_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, SourceId, Timeline};

    struct Fixed(Peaks);
    impl PeakSource for Fixed {
        fn peaks(&self, _src: SourceId) -> Option<&Peaks> {
            Some(&self.0)
        }
        fn gain_q16(&self, cdb: i32) -> i64 {
            if cdb == 0 { 65536 } else { libm::round(libm::pow(10.0, cdb as f64 / 2000.0) * 65536.0) as i64 }
        }
    }

    fn table(buckets: Vec<u8>) -> Peaks {
        Peaks { per_sec: PEAKS_PER_SEC, buckets, chans: Vec::new(), lod: Vec::new() }
    }

    fn stereo(lt: u8, rt: u8) -> Peaks {
        Peaks { chans: alloc::vec![table(alloc::vec![lt; 200]), table(alloc::vec![rt; 200])], ..table(alloc::vec![lt.max(rt); 200]) }
    }

    fn one_clip(clip: Clip) -> EditorState {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![clip];
        EditorState::new(tl)
    }

    /// The inked pixels of column `x` in rows `ya..yb`.
    fn inked(buf: &[u8], l: &Layout, x: i64, ya: i64, yb: i64) -> i64 {
        let b = clip_colour(0, Kind::Audio);
        let ink = [b[0] + WAVE_LIFT, b[1] + WAVE_LIFT, b[2] + WAVE_LIFT, 255];
        (ya..yb)
            .filter(|&y| {
                let i = (y as usize * l.w as usize + x as usize) * 4;
                buf[i..i + 4] == ink
            })
            .count() as i64
    }

    #[test]
    fn striawaveformstereo_peaks_of_keeps_each_channel() {
        let lr: Vec<f32> = (0..9_600).map(|i| if i % 2 == 0 { 0.25 } else { -0.75 }).collect();
        let p = peaks_of(48_000, 2, &lr, PEAKS_PER_SEC);
        assert!(p.buckets.iter().all(|&b| b == 191), "the folded table unchanged");
        assert_eq!(p.chans.len(), 2);
        assert!(p.chans[0].buckets.iter().all(|&b| b == 64) && p.chans[1].buckets.iter().all(|&b| b == 191));
        assert_eq!((p.chans[0].buckets.len(), p.chans[1].buckets.len()), (p.buckets.len(), p.buckets.len()));
        assert!(p.chans.iter().all(|c| c.chans.is_empty()));
        let mono = alloc::vec![0.5f32; 4_800];
        assert!(peaks_of(48_000, 1, &mono, PEAKS_PER_SEC).chans.is_empty(), "mono: no channel tables");
        assert!(peaks_of(48_000, 6, &mono, PEAKS_PER_SEC).chans.is_empty(), "more than two channels stay folded");
        // two one-second stretches appended == one call
        let two: Vec<f32> = (0..2 * 2 * 48_000).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 * if i % 2 == 0 { 1.0 } else { -0.5 }).collect();
        let mut fed = peaks_of(48_000, 2, &two[..2 * 48_000], PEAKS_PER_SEC);
        fed.append(peaks_of(48_000, 2, &two[2 * 48_000..], PEAKS_PER_SEC));
        assert_eq!(fed, peaks_of(48_000, 2, &two, PEAKS_PER_SEC));
        let mut mfed = table(Vec::new());
        mfed.append(peaks_of(48_000, 1, &mono, PEAKS_PER_SEC));
        assert_eq!(mfed, peaks_of(48_000, 1, &mono, PEAKS_PER_SEC), "a mono stretch appends its buckets only");
    }

    #[test]
    fn striawaveformstereo_two_half_height_envelopes() {
        let st = one_clip(Clip::new(SourceId(0), 0, 50, 0));
        let l = Layout::of(320, 160, &st);
        let y0 = l.row_y(0) + 2;
        let y1 = l.row_y(0) + ROW_H as i64 - 2;
        let ym = y0 + (y1 - y0) / 2;
        let paint = |p: Peaks| paint_in(&st, l, Order::Rgba, None, None, Some(&Fixed(p)));
        let lfull = paint(stereo(255, 0));
        let (up, down) = (inked(&lfull, &l, 160, y0, ym), inked(&lfull, &l, 160, ym, y1));
        std::println!("striawaveformstereo: L full / R silent upper={up} lower={down} body={}", y1 - y0);
        assert_eq!((up, down), ((y1 - y0) / 2, 0), "L fills the upper half, R silent");
        let rfull = paint(stereo(0, 255));
        assert_eq!((inked(&rfull, &l, 160, y0, ym), inked(&rfull, &l, 160, ym, y1)), (0, (y1 - y0) / 2), "R fills the lower half");
        let both = paint(stereo(255, 255));
        assert_ne!(both, paint(table(alloc::vec![255; 200])), "a stereo table paints two envelopes, not the folded bar");
        let ramp: Vec<u8> = (0..200u32).map(|i| (i * 37 % 256) as u8).collect();
        let mut fin = table(ramp.clone());
        fin.finish();
        assert_eq!(paint(fin), paint(table(ramp)), "the pyramid paints the same bytes as the linear max");
        let g = one_clip(Clip::new(SourceId(0), 0, 50, 0).gained(-600));
        let half = paint_in(&g, l, Order::Rgba, None, None, Some(&Fixed(stereo(255, 0))));
        let h6 = inked(&half, &l, 160, y0, ym);
        assert!((h6 - up / 2).abs() <= 1, "-6 dB halves each envelope: {h6} of {up}");
    }

    #[test]
    fn striawaveformstereo_lod_range_max_is_exact() {
        for n in [0usize, 1, 2, 3, 7, 8, 9, 17, 100, 1000, 1023, 1025] {
            let mut s = 12_345u32;
            let mut p = table(
                (0..n)
                    .map(|_| {
                        s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        (s >> 16) as u8
                    })
                    .collect(),
            );
            p.finish();
            assert_eq!(p.lod.is_empty(), n <= 1);
            if n > 1 {
                assert_eq!(p.lod.last().map(|t| t.len()), Some(1));
                assert_eq!(p.lod.last().unwrap()[0], p.buckets.iter().copied().max().unwrap());
            }
            let step = (n / 37).max(1);
            for b0 in (0..=n).step_by(step) {
                for b1 in (b0..=n + 2).step_by(step) {
                    let lin = p.buckets[b0..b1.min(n)].iter().copied().max().unwrap_or(0);
                    assert_eq!(p.range_max(b0, b1), lin, "n={n} {b0}..{b1}");
                }
            }
        }
        let mut st = stereo(10, 200);
        st.finish();
        assert!(st.chans.iter().all(|c| c.lod.last().map(|t| t[0]) == c.buckets.iter().copied().max()), "each channel gets its pyramid");
        let mut stale = table(alloc::vec![1, 2, 3, 4]);
        stale.finish();
        stale.append(table(alloc::vec![9]));
        assert!(stale.lod.is_empty(), "append drops a pyramid built before");
        assert_eq!(stale.range_max(0, 5), 9);
    }
}

// ---- STRIAKSNAP (LEDGER SR9063): the press's snap rule, one fn both faces call (named tail block) ----
// STRIASNAPTOGGLE (SR892) gave the host face a snap default (`snap_off`, the `snap` word, `n`) and Alt / Option at the
// press inverts it for that drag; the rule was written twice in nle_face.rs and nowhere the kernel Player could reach.
// [`press_nosnap`] is that rule, once: the host's body press and transition-edge press and the Player's band press
// (vplay.rs `edit_press`, its own `snap_off` flipped by `n`, Option read from the kernel's modifier state) all call it.

/// The `nosnap` a press hands [`Drag::begin_with`] / [`Drag::begin_transition_edge`]: Alt / Option held inverts the face's
/// snap default (`snap_off`) for this press only.
pub fn press_nosnap(alt: bool, snap_off: bool) -> bool {
    alt != snap_off
}

#[cfg(test)]
mod striaksnap_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// M3: snap on by default, Alt turns it off; with the default off, Alt turns it back on.
    #[test]
    fn striaksnap_press_nosnap_is_alt_against_the_default() {
        assert!(!press_nosnap(false, false));
        assert!(press_nosnap(true, false));
        assert!(press_nosnap(false, true));
        assert!(!press_nosnap(true, true));
    }

    /// M3: V: A 4..14 and A 30..40, the in mark at 50. A body drag of A 30..40 landing 3 px short of the mark snaps
    /// (guide at 50) when pressed with Alt while the default is off, and moves raw without Alt.
    #[test]
    fn striaksnap_alt_with_default_off_snaps() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 4), Clip::new(a, 20, 30, 30)];
        let st0 = EditorState::new(tl.set_marks((Some(50), None)).unwrap());
        let l = Layout::of_role(Role::Tracks, 800, 96, &st0);
        let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        let x = l.x_of(35) as f64 + 1.0;
        let to = x + 20.0 * z - 3.0;
        let mut st = st0.clone();
        let mut d = Drag::begin_with(&mut st, l, x, y, false, press_nosnap(true, true), false).unwrap();
        d.update(&st, to, y);
        assert_eq!((d.delta, d.guide), (20, Some(50)), "Alt with the default off snaps");
        let mut st = st0.clone();
        let mut d = Drag::begin_with(&mut st, l, x, y, false, press_nosnap(false, true), false).unwrap();
        d.update(&st, to, y);
        assert_eq!((d.delta, d.guide), (19, None), "the default off moves raw");
    }
}

// ---- STRIAKSCRUB (LEDGER SR9060): the scrub step's tick (named tail block) ----

impl Drag {
    /// STRIAKSCRUB (SR9060): THE tick a ruler scrub step shows (the pointer's tick: the press plus the snapped delta);
    /// `None` for any other drag. The host face's `hover` and the kernel Player's per-step seek both read it.
    pub fn scrub_tick(&self) -> Option<i64> {
        (self.kind == DragKind::Scrub).then(|| self.origin_tick + self.delta)
    }
}

#[cfg(test)]
mod kscrub_tests {
    use super::*;
    use crate::{Clip, Timeline};

    /// NLEFXA 0..20 at 0 and NLEFXA 20..30 at 20, the view band `Role::Tracks` 320x56 (the slip fixture's shape).
    fn kscrub_fixture() -> (EditorState, Layout, f64) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("NLEFXA", 40);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(a, 20, 30, 20)];
        let st = EditorState::new(tl);
        let l = Layout::of_role(Role::Tracks, 320, 56, &st);
        (st, l, l.zoom as f64)
    }

    /// STRIAKSCRUB M3: a ruler press (no snap) at tick 5 moved five ticks right is the scrub step at tick 10.
    #[test]
    fn striakscrub_scrub_tick_is_the_pointer() {
        let (mut st, l, z) = kscrub_fixture();
        let (x, y) = (l.x_of(5) as f64 + z / 2.0, l.ruler_y as f64 + RULER_H as f64 / 2.0);
        let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap();
        assert_eq!(d.kind, DragKind::Scrub);
        assert_eq!(d.scrub_tick(), Some(d.origin_tick));
        d.update(&st, x + 5.0 * z, y);
        assert_eq!(d.scrub_tick(), Some(d.origin_tick + 5));
        assert_eq!(d.origin_tick, 5);
    }

    /// STRIAKSCRUB M3: a press on a clip body (a Move) names no scrub tick.
    #[test]
    fn striakscrub_scrub_tick_none_off_ruler() {
        let (mut st, l, z) = kscrub_fixture();
        let (x, y) = (l.x_of(25) as f64 + z / 2.0, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
        let d = Drag::begin_with(&mut st, l, x, y, false, false, false).unwrap();
        assert_eq!(d.kind, DragKind::Move);
        assert_eq!(d.scrub_tick(), None);
    }
}
