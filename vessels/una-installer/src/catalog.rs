// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: vessel — the installer's vessel catalog as data (INSTALLERVESSEL, LEDGER SR413); it builds nothing itself.
//!
//! One row per UnaOS vessel a person can adopt on a foreign host: its name, its crate and directory in the root
//! workspace, the Cargo feature that selects its face on each host OS (measured from each vessel's Cargo.toml
//! `[features]` and its `main.rs` cfgs), and a one-line description. The host test reads the root `Cargo.toml` and
//! each row's own manifest, so a row that names a crate, a directory or a feature the tree does not have goes red.

use crate::host::Os;

/// How a vessel's face is selected on one host OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// The face is chosen by `target_os` alone: build with no feature.
    Plain,
    /// Build with `--features <name>`.
    Feature(&'static str),
    /// The vessel has no face on this OS.
    None,
}

/// One installable vessel.
#[derive(Clone, Copy, Debug)]
pub struct Vessel {
    pub name: &'static str,
    pub krate: &'static str,
    pub dir: &'static str,
    pub macos: Face,
    pub linux: Face,
    pub about: &'static str,
}

/// The vessels, in the order the window lists them.
pub static CATALOG: [Vessel; 7] = [
    Vessel { name: "Aether", krate: "aether-shell", dir: "vessels/aether-shell", macos: Face::Feature("macos"), linux: Face::Feature("gtk"), about: "the web browser" },
    Vessel { name: "Una", krate: "una", dir: "vessels/una", macos: Face::Plain, linux: Face::Feature("gtk"), about: "the code workspace" },
    Vessel { name: "Lumen", krate: "lumen", dir: "vessels/lumen", macos: Face::Feature("macos"), linux: Face::Feature("gtk"), about: "the companion" },
    Vessel { name: "Stria", krate: "stria-view", dir: "vessels/stria-view", macos: Face::Feature("macos"), linux: Face::Feature("gtk"), about: "the video editor" },
    Vessel { name: "Facet", krate: "facet-view", dir: "vessels/facet-view", macos: Face::Plain, linux: Face::None, about: "the image viewer" },
    Vessel { name: "Pulse", krate: "pulse", dir: "vessels/pulse", macos: Face::Plain, linux: Face::None, about: "the system monitor" },
    Vessel { name: "Phonolite", krate: "phonolite", dir: "vessels/phonolite", macos: Face::Plain, linux: Face::None, about: "the tone generator" },
];

/// The catalog row for a crate name.
pub fn find(krate: &str) -> Option<&'static Vessel> {
    CATALOG.iter().find(|v| v.krate == krate)
}

impl Vessel {
    /// This vessel's face on `os` (Windows: none yet, INSTALLER-WINDOWS).
    pub fn face(&self, os: Os) -> Face {
        match os {
            Os::MacOs => self.macos,
            Os::Linux => self.linux,
            Os::Windows => Face::None,
        }
    }

    /// The cargo arguments that build this vessel's face on `os`, or None when it has no face there.
    pub fn build_args(&self, os: Os) -> Option<Vec<String>> {
        let mut args = vec!["build".to_string(), "-p".to_string(), self.krate.to_string()];
        match self.face(os) {
            Face::None => return None,
            Face::Plain => {}
            Face::Feature(f) => {
                args.push("--features".to_string());
                args.push(f.to_string());
            }
        }
        args.push("--release".to_string());
        Some(args)
    }

    /// The developer's run line for this vessel on `os` (what the README tells a person with a checkout).
    pub fn run_line(&self, os: Os) -> Option<String> {
        self.build_args(os).map(|mut a| {
            a[0] = "run".to_string();
            format!("cargo {}", a.join(" "))
        })
    }
}
