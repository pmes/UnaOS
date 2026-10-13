// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STRIAMETAL (SR139): the transport bar — the window's bottom BAR_H rows under the picture, laid out left to right:
//
//   [ play/pause ] [ m:ss/m:ss ] [ ======o------ the track ] [ speaker + five volume steps ]
//     x 4..24        x 28..         x TRACK0..TRACK1             x VOL0..284
//
// A click on the button toggles play; on the track seeks to that fraction of the duration; on the speaker mutes;
// on a step sets the volume. `hit` answers what a window-px point is over; `over_control` drives the pointer shape
// (CURSORSHAPE B550: the hand over a control, the arrow elsewhere — the compositor draws it, this program never does).

use crate::{ui, Session, ACCENT, BAR_H, DIM, INK, MAX_W, VID_H};

/// The bar's top row.
pub const Y0: i32 = VID_H as i32;
const BAR_BG: u32 = 0x1C_1C22;
const TRACK_BG: u32 = 0x3A_3A44;
const BUTTON: (i32, i32) = (4, 24);
const TIME_X: i32 = 28;
const TRACK0: i32 = 112;
const TRACK1: i32 = 222;
const VOL0: i32 = 230;
/// The first volume step's x; steps are STEP_W apart.
const STEPS_X: i32 = VOL0 + 12;
const STEP_W: i32 = 8;
/// The volume scale: 0 (silent) ..= VOL_MAX, two levels per drawn step.
pub const VOL_MAX: i32 = 10;

/// What a point on the bar is over.
pub enum Hit {
    Play,
    Seek(u64),
    Mute,
    Vol(i32),
    Nothing,
}

fn centre() -> i32 {
    Y0 + BAR_H / 2
}

/// Paint the bar for `s` at volume `vol` (muted or not) into the back buffer.
pub fn paint(fb: &mut [u32], s: &Session, vol: i32, muted: bool) {
    let cy = centre();
    ui::fill(fb, 0, Y0, MAX_W, BAR_H, BAR_BG);
    // play / pause
    if s.playing {
        ui::fill(fb, 8, cy - 7, 4, 14, INK);
        ui::fill(fb, 15, cy - 7, 4, 14, INK);
    } else {
        for r in 0..14 {
            let w = if r < 7 { r + 1 } else { 14 - r };
            ui::fill(fb, 9, cy - 7 + r, w * 3 / 2, 1, INK);
        }
    }
    // the time
    let t = if s.dur_ms > 0 && s.dur_ms < 600_000 {
        alloc::format!("{}/{}", ui::clock_text(s.media_ms.min(s.dur_ms)), ui::clock_text(s.dur_ms))
    } else {
        ui::clock_text(s.media_ms)
    };
    ui::text(fb, TIME_X, cy - 4, &t, INK);
    // the track
    let len = TRACK1 - TRACK0;
    ui::fill(fb, TRACK0, cy - 2, len, 4, TRACK_BG);
    if s.dur_ms > 0 {
        let at = (s.media_ms.min(s.dur_ms) * len as u64 / s.dur_ms) as i32;
        ui::fill(fb, TRACK0, cy - 2, at, 4, ACCENT);
        ui::fill(fb, TRACK0 + at - 2, cy - 5, 4, 10, INK);
    }
    // the speaker, struck through when muted, and the steps
    ui::fill(fb, VOL0, cy - 2, 3, 5, INK);
    for r in 0..9 {
        let h = if r < 5 { r } else { 8 - r };
        ui::fill(fb, VOL0 + 3, cy - 4 + r, 1 + h.min(4), 1, INK);
    }
    if muted {
        for k in 0..8 {
            ui::fill(fb, VOL0 + k, cy - 4 + k, 1, 1, 0xFF_5050);
        }
    }
    for i in 0..(VOL_MAX / 2) {
        let h = 4 + i * 2;
        let x = STEPS_X + i * STEP_W;
        let lit = !muted && vol >= (i + 1) * 2;
        ui::fill(fb, x, cy + 5 - h, STEP_W - 3, h, if lit { ACCENT } else { DIM });
    }
}

/// What `(x, y)` (window px) is over, for the session `s`.
pub fn hit(x: i32, y: i32, s: &Session) -> Hit {
    if y < Y0 || y >= Y0 + BAR_H || x < 0 || x >= MAX_W {
        return Hit::Nothing;
    }
    if (BUTTON.0..BUTTON.1).contains(&x) {
        return Hit::Play;
    }
    if (TRACK0 - 3..TRACK1 + 3).contains(&x) {
        if s.dur_ms == 0 {
            return Hit::Nothing;
        }
        let at = (x - TRACK0).clamp(0, TRACK1 - TRACK0) as u64;
        return Hit::Seek(at * s.dur_ms / (TRACK1 - TRACK0) as u64);
    }
    if (VOL0..STEPS_X - 2).contains(&x) {
        return Hit::Mute;
    }
    if (STEPS_X..STEPS_X + STEP_W * (VOL_MAX / 2)).contains(&x) {
        return Hit::Vol((((x - STEPS_X) / STEP_W) + 1) * 2);
    }
    Hit::Nothing
}

/// Is `(x, y)` over something a click acts on (the pointer's hand)?
pub fn over_control(x: i32, y: i32, s: &Session) -> bool {
    !matches!(hit(x, y, s), Hit::Nothing)
}
