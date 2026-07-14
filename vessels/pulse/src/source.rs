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

//! The sampling seam.
//!
//! `PulseSource` is the named seam between the pulse UI and wherever per-core
//! load truly comes from — the same honest-seam philosophy as the kernel's vug
//! render meter: name the seam so a real feed can replace the source someday.
//!
//! v1 ships [`HostPulseSource`], sampling the HOST's per-core CPU stats (this
//! layer is host-native). The banked vision is a UnaOS-kernel feed — serial
//! bridge or network telemetry from the metal machines — implementing the same
//! trait and firing the same `SMessage::CorePulse` beat, with zero UI change.
//!
//! Handler-extraction candidate: if a system-stats domain handler materializes
//! (principia — "The Architect", System domain — is the natural owner once it
//! becomes a real crate), this module lifts out of the vessel additively.

use sysinfo::{CpuRefreshKind, RefreshKind, System};

/// The seam: something that can report per-core CPU load.
pub trait PulseSource: Send {
    /// Sample the current per-core load, one `0.0..=1.0` entry per core.
    ///
    /// The first sample after construction may legitimately read all-zero
    /// (delta-based samplers need a baseline); callers should just keep
    /// beating.
    fn sample(&mut self) -> Vec<f32>;
}

/// v1: the host operating system's own per-core CPU statistics, via `sysinfo`.
pub struct HostPulseSource {
    sys: System,
}

impl HostPulseSource {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
        );
        Self { sys }
    }
}

impl Default for HostPulseSource {
    fn default() -> Self {
        Self::new()
    }
}

impl PulseSource for HostPulseSource {
    fn sample(&mut self) -> Vec<f32> {
        self.sys.refresh_cpu_usage();
        self.sys
            .cpus()
            .iter()
            .map(|cpu| (cpu.cpu_usage() / 100.0).clamp(0.0, 1.0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_source_reports_every_core_in_range() {
        let mut src = HostPulseSource::new();
        // Two samples: the first only establishes the measurement baseline.
        let _ = src.sample();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let loads = src.sample();

        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        assert_eq!(loads.len(), cores, "one load entry per core");
        for load in &loads {
            assert!((0.0..=1.0).contains(load), "load {load} out of range");
        }
    }
}
