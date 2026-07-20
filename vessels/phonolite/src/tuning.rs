// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

#![allow(dead_code)]

//! The vessel's musical constants and the level-beat seam.
//!
//! Pure logic, no AppKit, no audio thread — everything here is unit-tested
//! host-side. The slider→Hz mapping itself lives in quartzite's tone panel
//! (`tone_panel::slider_to_hz`); this module owns what the *vessel* decides:
//! the playable range, the safe gain ceiling, and how an engine level becomes
//! a Synapse beat.

use bandy::SMessage;

/// Bottom of the frequency slider: A1. Five octaves below the top.
pub const FREQ_MIN_HZ: f64 = 55.0;
/// Top of the frequency slider: A6.
pub const FREQ_MAX_HZ: f64 = 1760.0;
/// Where the tone starts: concert A. The ear-witness reference pitch.
pub const INITIAL_FREQ_HZ: f64 = 440.0;

/// Gain slider ceiling — deliberately well below full scale; this vessel is
/// a bench instrument, not a siren.
pub const GAIN_MAX: f64 = 0.5;
/// Where the gain starts (matches `create_test_graph`'s built-in 0.1).
pub const INITIAL_GAIN: f64 = 0.1;

/// The test graph's node layout (documented on `create_test_graph`):
/// node 1 = the gain node; its param 0 = base_gain.
pub const GAIN_NODE: usize = 1;
pub const GAIN_PARAM: usize = 0;

/// One beat of the level meter: the engine's block peak, clamped to the
/// meter's `0.0..=1.0` domain, as a single-bin [`SMessage::Spectrum`].
///
/// Spectrum is the existing RESONANCE-section message whose payload is
/// magnitudes — the honest fit for an output level; nothing here fires from
/// the audio callback (the cadence task in `main` reads an atomic the
/// callback wrote).
pub fn level_beat(peak: f32) -> SMessage {
    SMessage::Spectrum {
        magnitude: vec![peak.clamp(0.0, 1.0)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_beat_is_a_single_clamped_magnitude() {
        for (input, expected) in [(0.3f32, 0.3f32), (-1.0, 0.0), (7.5, 1.0)] {
            match level_beat(input) {
                SMessage::Spectrum { magnitude } => {
                    assert_eq!(magnitude, vec![expected], "input {input}");
                }
                other => panic!("level_beat must yield Spectrum, got {other:?}"),
            }
        }
    }

    #[test]
    fn initial_values_sit_inside_their_slider_ranges() {
        assert!((FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&INITIAL_FREQ_HZ));
        assert!((0.0..=GAIN_MAX).contains(&INITIAL_GAIN));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn concert_a_round_trips_through_the_panel_mapping() {
        use quartzite::platforms::macos::tone_panel::{hz_to_slider, slider_to_hz};
        let t = hz_to_slider(INITIAL_FREQ_HZ, FREQ_MIN_HZ, FREQ_MAX_HZ);
        let hz = slider_to_hz(t, FREQ_MIN_HZ, FREQ_MAX_HZ);
        assert!((hz - INITIAL_FREQ_HZ).abs() < 1e-9);
    }
}
