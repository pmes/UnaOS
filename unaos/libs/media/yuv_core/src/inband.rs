// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Stria (CODEX §2 A/V), seam shared-core — the ONE in-band colour / size switch rule every decoder face (AV1, VP9; host and kernel) reports by.
//! `yuv_core::inband` — the ONE in-band switch rule (AV1INBANDSWITCH SR373, AV1INBANDSIZE SR379), lifted here from av1_core's
//! `image.rs` by VP9INBANDSWITCH (LEDGER SR400) byte for byte, so VP9 reports by it too (av1_core re-exports [`InBand`] and
//! [`switch_codes`]; `StreamDecoder::switch` / `size_switch` delegate, signatures kept). Plain values: the caller hands in the
//! code points the current picture states (`now`) and the open's colour; demux_core's `Colour` maps them on each face.
//!
//! A frame converts by the open's colour until the stated code points differ from the first stated ones (the open's bitstream
//! reading); then every field the open took FROM the bitstream (open == base) follows the new statement and a field the
//! container stated differently keeps the container's. Back at the base codes = the open's colour again, byte-identical.

/// AV1INBANDSWITCH (SR373): a decoder's switch state — the first stated code points (`base`) and the colour the last frame
/// switched to (`last`, `None` = the open's).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct InBand {
    pub base: Option<(u8, u8, u8, bool)>,
    pub last: Option<(u8, u8, u8, Option<bool>)>,
    pub size: Option<(u32, u32)>, // AV1INBANDSIZE (SR379): the last shown picture's size (`size_switch`)
}

impl InBand {
    /// AV1INBANDSWITCH (SR373): after a decode — `now` the code points the current picture states as `(primaries, transfer,
    /// matrix, full_range)` (`None`: it states none, the last answer holds), `open` the open's colour. `.0` is `None` while
    /// `now` equals the first stated codes (the open's colour applies), else the colour to convert by ([`switch_codes`]);
    /// `.1` is `Some((matrix before, matrix now))` once per change of `.0`.
    pub fn switch(&mut self, now: Option<(u8, u8, u8, bool)>, open: Option<(u8, u8, u8, Option<bool>)>) -> (Option<(u8, u8, u8, Option<bool>)>, Option<(u8, u8)>) {
        let Some(now) = now else { return (self.last, None) };
        let base = *self.base.get_or_insert(now);
        let next = if now == base { None } else { Some(switch_codes(open, base, now)) };
        let open_m = open.map_or(base.2, |o| o.2);
        let changed = (next != self.last).then(|| (self.last.map_or(open_m, |l| l.2), next.map_or(open_m, |n| n.2)));
        self.last = next;
        (next, changed)
    }

    /// AV1INBANDSIZE (SR379): after a decode — the shown picture's `w` x `h` against the last shown one: `Some((before, now))`
    /// once per change, `None` for the first picture and while the size holds. A seek keeps it.
    pub fn size_switch(&mut self, w: u32, h: u32) -> Option<((u32, u32), (u32, u32))> {
        let before = self.size.replace((w, h))?;
        (before != (w, h)).then_some((before, (w, h)))
    }
}

/// AV1INBANDSWITCH (SR373): the container-wins merge of a header switch — each field the open took from the bitstream (equal to
/// `base`, the open's in-band reading) takes `now`'s value; a field the container stated differently keeps the open's. `open`
/// `None` (no colour at open) reads as `base`.
pub fn switch_codes(open: Option<(u8, u8, u8, Option<bool>)>, base: (u8, u8, u8, bool), now: (u8, u8, u8, bool)) -> (u8, u8, u8, Option<bool>) {
    let o = open.unwrap_or((base.0, base.1, base.2, Some(base.3)));
    let f = |o: u8, b: u8, n: u8| if o == b { n } else { o };
    (f(o.0, base.0, now.0), f(o.1, base.1, now.1), f(o.2, base.2, now.2), if o.3 == Some(base.3) { Some(now.3) } else { o.3 })
}
