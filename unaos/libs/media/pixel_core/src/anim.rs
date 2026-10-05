// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FACETANIM (rmbp-ledger B358) — animations one frame at a time.
//!
//! [`crate::decode`] keeps every composited canvas of a GIF, animated WebP or APNG in
//! [`crate::Image::frames`]: right for a host oracle, wrong for Ring 0, which paid for 74 canvases of
//! 297 KB to show one. This module is the streaming face of the SAME compositors (`gif::Stepper`,
//! `webp::Stepper`, `png::ApngStepper` — `decode` itself is built on them, so there is one composite
//! per format and the two faces cannot drift):
//!
//! * [`Animation`] holds ONE canvas, plus at most ONE more canvas-sized buffer while a frame that
//!   disposes to "previous" is up (GIF disposal 3, APNG `APNG_DISPOSE_OP_PREVIOUS`). A player that
//!   copies each frame out holds two frames, never N.
//! * [`decode_first_frame`] = `decode(..).rgba` without compositing (or keeping) any frame after 0.
//!
//! A still (every other format, or an animation with one frame) is an [`Animation`] of one frame.

use alloc::vec::Vec;

use crate::{Error, Format, Image};

enum Kind {
    Still,
    Gif(crate::gif::Stepper),
    WebP(crate::webp::Stepper),
    Apng(crate::png::Parsed, crate::png::ApngStepper),
}

/// One frame [`Animation::next_frame`] put on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// 0-based index in the animation.
    pub index: usize,
    /// How long it stays up, ms, as stored (0 is left to the player to clamp).
    pub delay_ms: u32,
}

/// A streaming decoder over a file's bytes — borrowed (`&[u8]`) or owned (`Vec<u8>`: the kernel keeps
/// the file and its decoder in one value). See the module doc.
pub struct Animation<B: AsRef<[u8]>> {
    bytes: B,
    kind: Kind,
    width: u32,
    height: u32,
    loop_count: Option<u16>,
    count: usize,
    /// The still's pixels (for [`Kind::Still`]).
    still: Vec<u8>,
    /// Frame 0 was composited by `new` and not yet handed out: its delay.
    primed: Option<u32>,
    next: usize,
    ended: bool,
}

impl<B: AsRef<[u8]>> Animation<B> {
    /// Parse `bytes` and composite frame 0 (so a file whose first frame fails is refused here, the way
    /// [`crate::decode`] refuses it, and an APNG whose animation is unusable becomes its still).
    pub fn new(owner: B) -> Result<Self, Error> {
        let bytes = owner.as_ref();
        let (kind, w, h, lp, n, d, still) = match crate::sniff(bytes).ok_or(Error::UnknownFormat)? {
            Format::Gif => {
                let (n, lp) = crate::gif::scan(bytes);
                let mut s = crate::gif::Stepper::new(bytes)?;
                let d = match s.step(bytes) {
                    Some(r) => r?,
                    None => return Err(Error::Malformed("gif without an image")),
                };
                let (w, h) = (s.cw, s.ch);
                (Kind::Gif(s), w, h, lp, n, d, None)
            }
            Format::WebP => match crate::webp::animation_start(bytes) {
                Some((p, end, w, h)) => {
                    let mut s = crate::webp::Stepper::new(bytes, p, end, w, h)?;
                    let d = s.step(bytes).ok_or(Error::Malformed("webp animation without an ANMF frame"))??;
                    let (n, lp) = (s.listed(), s.loop_count());
                    (Kind::WebP(s), w, h, lp, n, d, None)
                }
                None => still(crate::decode_webp(bytes)?),
            },
            Format::Png => {
                let p = crate::png::parse(bytes)?;
                match crate::png::ApngStepper::new(&p)? {
                    Some(mut s) => match s.step(&p) {
                        Some(r) => {
                            let d = r?;
                            let (w, h, n, lp) = (p.width(), p.height(), crate::png::ApngStepper::listed(&p), s.loop_count());
                            (Kind::Apng(p, s), w, h, lp, n, d, None)
                        }
                        None => still(p.still()?),
                    },
                    None => still(p.still()?),
                }
            }
            _ => still(crate::decode(bytes)?),
        };
        let mut a = Animation {
            bytes: owner,
            kind,
            width: w,
            height: h,
            loop_count: lp,
            count: n.max(1),
            still: still.unwrap_or_default(),
            primed: Some(d),
            next: 0,
            ended: false,
        };
        if a.count <= 1 {
            // One frame is a still: `decode` reports no frames and no loop count for it.
            a.count = 1;
            a.loop_count = None;
        }
        Ok(a)
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// [`Image::loop_count`]'s meaning: `Some(0)` forever, `Some(n)` n extra plays, `None` play once.
    pub fn loop_count(&self) -> Option<u16> {
        self.loop_count
    }
    /// Frames in the animation: the container's count until a frame fails, then the frames that
    /// played (a frame whose data is broken ends the animation, as [`crate::decode`] ends it).
    pub fn frame_count(&self) -> usize {
        self.count
    }
    /// More than one frame.
    pub fn is_animated(&self) -> bool {
        self.count > 1
    }

    /// The canvas: `width * height * 4` straight RGBA, the frame [`Animation::next_frame`] last put up.
    pub fn canvas(&self) -> &[u8] {
        match &self.kind {
            Kind::Still => &self.still,
            Kind::Gif(s) => &s.canvas,
            Kind::WebP(s) => &s.canvas,
            Kind::Apng(_, s) => &s.canvas,
        }
    }

    /// Canvas-sized buffers this decoder holds right now (1, or 2 while a dispose-previous frame is
    /// up) — the FACETANIM witness's `frames_held` counts this plus the player's own frame.
    pub fn buffers_held(&self) -> usize {
        1 + match &self.kind {
            Kind::Gif(s) => s.holds_saved() as usize,
            Kind::Apng(_, s) => s.holds_saved() as usize,
            _ => 0,
        }
    }

    /// Composite the next frame onto the canvas. `None` at the end of the animation (call
    /// [`Animation::rewind`] to loop); `Some(Err)` = the file broke there (GIF: a structural error
    /// [`crate::decode`] refuses the whole file for).
    pub fn next_frame(&mut self) -> Option<Result<FrameInfo, Error>> {
        if self.ended {
            return None;
        }
        let index = self.next;
        let r = match self.primed.take() {
            Some(d) => Some(Ok(d)),
            None => match &mut self.kind {
                Kind::Still => None,
                Kind::Gif(s) => s.step(self.bytes.as_ref()),
                Kind::WebP(s) => s.step(self.bytes.as_ref()),
                Kind::Apng(p, s) => s.step(p),
            },
        };
        match r {
            Some(Ok(delay_ms)) => {
                self.next += 1;
                if self.next > self.count {
                    self.count = self.next;
                }
                Some(Ok(FrameInfo { index, delay_ms }))
            }
            Some(Err(e)) => {
                self.ended = true;
                Some(Err(e))
            }
            None => {
                self.ended = true;
                // Fewer frames than the container listed: the count is what played.
                self.count = self.next.max(1);
                None
            }
        }
    }

    /// Back to before frame 0; the next [`Animation::next_frame`] composites frame 0 again.
    pub fn rewind(&mut self) {
        self.next = 0;
        self.ended = false;
        match &mut self.kind {
            Kind::Still => self.primed = Some(0),
            Kind::Gif(s) => {
                let _ = s.reset();
            }
            Kind::WebP(s) => s.reset(),
            Kind::Apng(_, s) => s.reset(),
        }
    }
}

/// The arms of [`Animation::new`] for a still: `(kind, w, h, loop, count, delay, pixels)`.
#[allow(clippy::type_complexity)]
fn still(img: Image) -> (Kind, u32, u32, Option<u16>, usize, u32, Option<Vec<u8>>) {
    (Kind::Still, img.width, img.height, None, 1, 0, Some(img.rgba))
}

/// The file's bytes.
impl<B: AsRef<[u8]>> Animation<B> {
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

/// [`crate::decode`]`(bytes)` with only frame 0 composited: `rgba` equal to `decode(bytes).rgba`,
/// `frames: None`, `loop_count: None`. For the paths that show one picture of a file (wallpaper,
/// Quarry thumbnails) — they never pay for frames 1..N. Non-animated formats are `decode` itself.
pub fn decode_first_frame(bytes: &[u8]) -> Result<Image, Error> {
    match crate::sniff(bytes) {
        Some(Format::Gif | Format::WebP | Format::Png) => {
            let a = Animation::new(bytes)?;
            let (w, h) = (a.width, a.height);
            let rgba = match a.kind {
                Kind::Still => a.still,
                Kind::Gif(s) => s.canvas,
                Kind::WebP(s) => s.canvas,
                Kind::Apng(_, s) => s.canvas,
            };
            Ok(Image::still(w, h, rgba))
        }
        _ => {
            let mut img = crate::decode(bytes)?;
            img.frames = None;
            img.loop_count = None;
            Ok(img)
        }
    }
}
