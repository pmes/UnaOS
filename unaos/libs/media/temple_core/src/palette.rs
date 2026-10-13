// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): the scene's colours, by NAME. The beam and the spectrum are palette_core's existing SPLASH_* rows
// (the boot splash's beam and refracted spectrum — the same picture's ancestors). The dark-matter and star rows are
// the ones R128's two tiers share with STARFIELD (B661); they are held HERE, in this one module, until the fold moves
// them into `libs/sys/palette_core/src/lib.rs` beside STARFIELD's rows (palette_core is not this arc's file — the
// exact rows are in docs/dev/evidence/temple/TEMPLE.md §Palette). Packed 0x00RRGGBB display values; the
// renderer linearises them once (gamma 2, the inverse of its output curve).

pub use palette_core::painter::{SPLASH_BEAM, SPLASH_SPECTRUM};

/// `scene.ground` — ULTRA BLACK: the void is zero light, not a dark grey (the panel's black is the frame).
pub const SCENE_ULTRA_BLACK: u32 = 0x0000_0000;
/// `flow.violet` — the dark-matter strokes where the mass is thin.
pub const FLOW_VIOLET: u32 = 0x005A_2DB4;
/// `flow.cyan` — the strokes where the mass gathers (the filaments' cores).
pub const FLOW_CYAN: u32 = 0x0032_A8C8;
/// `flow.haze` — the diffuse glow of the mass itself.
pub const FLOW_HAZE: u32 = 0x0030_1C5A;
/// `star.warm` / `star.cool` — the two ends of the stars' colour temperature.
pub const STAR_WARM: u32 = 0x00FF_D6A8;
pub const STAR_COOL: u32 = 0x00B4_CCFF;
/// `zircon.transmit` — what one world unit of the gem lets through, per channel (a blue zircon: red absorbed most).
pub const ZIRCON_TRANSMIT: u32 = 0x009C_D8F6;
/// `zircon.fire` — the beam's light trapped inside the gem (the internal glow a ray picks up after two bounces).
pub const ZIRCON_FIRE: u32 = 0x0078_B4FF;
