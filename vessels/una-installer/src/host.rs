// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: vessel — the installer's foreign-host probe, apps-dir rule and install plan (INSTALLERVESSEL, LEDGER SR413).
//!
//! `Host::probe` reads the machine once: OS, arch, rustup (the repo's `rust-toolchain.toml` pins a dated nightly, so
//! rustup, not a distro cargo, is the prerequisite), the toolkit a vessel's face links against (macOS: the Xcode
//! Command Line Tools, `xcode-select -p`; Linux: the GTK4 / Qt6 development files, `pkg-config --exists`), the user's
//! apps directory, and the UnaOS source checkout (elessar's `Spline::UnaOS` anchor). Everything after the probe is
//! pure: the census line, its parser, the apps-dir rule per OS, the install plan with its refusals by name, and the
//! Linux `.desktop` entry, so the host test drives every branch without touching the machine.

use crate::catalog::{Face, Vessel};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The foreign host's operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

impl Os {
    /// The OS this binary was built for.
    pub fn this() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Os::MacOs => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }
}

/// The face toolkit's development files the host carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolkit {
    AppKit,
    Gtk4,
    Qt6,
    Gtk4Qt6,
    None,
}

impl Toolkit {
    pub fn name(self) -> &'static str {
        match self {
            Toolkit::AppKit => "appkit",
            Toolkit::Gtk4 => "gtk4",
            Toolkit::Qt6 => "qt6",
            Toolkit::Gtk4Qt6 => "gtk4+qt6",
            Toolkit::None => "none",
        }
    }

    fn has_gtk4(self) -> bool {
        matches!(self, Toolkit::Gtk4 | Toolkit::Gtk4Qt6)
    }
}

/// Where an installed vessel goes: the binary's directory and, on Linux, the desktop-entry directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppsDir {
    pub bin: PathBuf,
    pub desktop: Option<PathBuf>,
}

/// The apps-dir rule per OS: macOS `~/Applications`; Linux `~/.local/bin` plus `~/.local/share/applications`;
/// Windows is refused by name until INSTALLER-WINDOWS.
pub fn apps_dir(os: Os, home: &Path) -> Result<AppsDir, &'static str> {
    match os {
        Os::MacOs => Ok(AppsDir { bin: home.join("Applications"), desktop: None }),
        Os::Linux => Ok(AppsDir {
            bin: home.join(".local/bin"),
            desktop: Some(home.join(".local/share/applications")),
        }),
        Os::Windows => Err("windows-unbuilt"),
    }
}

/// One reading of the foreign host.
#[derive(Clone, Debug)]
pub struct Host {
    pub os: Os,
    pub arch: &'static str,
    pub rustup: bool,
    pub toolkit: Toolkit,
    pub apps: Result<AppsDir, &'static str>,
    pub checkout: Option<PathBuf>,
}

/// True when `program args` spawns and exits 0 (its output discarded).
fn runs(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The UnaOS checkout: `explicit` when it is one, else the first ancestor of the working directory, then of this
/// binary, whose elessar `Context` reads as `Spline::UnaOS`.
pub fn find_checkout(explicit: Option<&Path>) -> Option<PathBuf> {
    let is_unaos = |d: &Path| elessar::Context::new(d).spline == elessar::Spline::UnaOS;
    if let Some(dir) = explicit {
        return is_unaos(dir).then(|| dir.to_path_buf());
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    starts
        .iter()
        .flatten()
        .flat_map(|p| p.ancestors())
        .find(|d| is_unaos(*d))
        .map(Path::to_path_buf)
}

impl Host {
    /// Read the machine once.
    pub fn probe(checkout: Option<&Path>) -> Host {
        let os = Os::this();
        let toolkit = match os {
            Os::MacOs if runs("xcode-select", &["-p"]) => Toolkit::AppKit,
            Os::Linux => match (runs("pkg-config", &["--exists", "gtk4"]), runs("pkg-config", &["--exists", "Qt6Widgets"])) {
                (true, true) => Toolkit::Gtk4Qt6,
                (true, false) => Toolkit::Gtk4,
                (false, true) => Toolkit::Qt6,
                (false, false) => Toolkit::None,
            },
            _ => Toolkit::None,
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let apps = match home {
            Some(h) => apps_dir(os, &h),
            None if os == Os::Windows => Err("windows-unbuilt"),
            None => Err("no-home"),
        };
        Host {
            os,
            arch: std::env::consts::ARCH,
            rustup: runs("rustup", &["--version"]),
            toolkit,
            apps,
            checkout: find_checkout(checkout),
        }
    }

    /// THE census line: `[installer] host os= arch= rustup= toolkit= apps_dir= checkout=`.
    pub fn census(&self) -> String {
        let apps = match &self.apps {
            Ok(a) => a.bin.display().to_string(),
            Err(_) => "none".to_string(),
        };
        let checkout = self.checkout.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".to_string());
        format!(
            "[installer] host os={} arch={} rustup={} toolkit={} apps_dir={} checkout={}",
            self.os.name(),
            self.arch,
            self.rustup as u8,
            self.toolkit.name(),
            apps,
            checkout
        )
    }
}

/// The census keys, in the order the line prints them.
#[cfg_attr(not(test), allow(dead_code))] // read back by the host test (and by any tool that parses the wire)
pub const CENSUS_KEYS: [&str; 6] = ["os", "arch", "rustup", "toolkit", "apps_dir", "checkout"];

/// Parse a census line back into its six `(key, value)` pairs. The keys are found in their fixed order, so a value
/// may carry spaces (a macOS home path); None when the prefix or any key is missing.
#[cfg_attr(not(test), allow(dead_code))] // read back by the host test (and by any tool that parses the wire)
pub fn parse_census(line: &str) -> Option<Vec<(&'static str, &str)>> {
    let mut rest = line.strip_prefix("[installer] host ")?;
    let mut out = Vec::with_capacity(CENSUS_KEYS.len());
    for (i, key) in CENSUS_KEYS.iter().enumerate() {
        rest = rest.strip_prefix(*key)?.strip_prefix('=')?;
        let end = match CENSUS_KEYS.get(i + 1) {
            Some(next) => rest.find(&format!(" {next}="))?,
            None => rest.len(),
        };
        out.push((*key, &rest[..end]));
        rest = rest[end..].trim_start_matches(' ');
    }
    Some(out)
}

/// What `install` will do for one vessel on this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The cargo arguments to build with, or None to copy the prebuilt binary as it stands.
    pub build: Option<Vec<String>>,
    /// Why the build is skipped (a toolchain refusal name), when `build` is None.
    pub why: Option<&'static str>,
    /// The checkout's release binary, the copy's source.
    pub from: PathBuf,
    /// The installed binary's path.
    pub dest: PathBuf,
    /// The Linux desktop entry's path.
    pub desktop: Option<PathBuf>,
}

/// The install plan for `v` on `host`, `prebuilt` saying whether the checkout already holds its release binary.
/// Refusals by name: `windows-unbuilt`, `no-home`, `no-face`, `no-checkout`, and the toolchain's `no-rustup` /
/// `no-xcode-clt` / `no-gtk4-dev` when there is no prebuilt binary to copy instead.
pub fn plan(host: &Host, v: &Vessel, prebuilt: bool) -> Result<Plan, &'static str> {
    if host.os == Os::Windows {
        return Err("windows-unbuilt");
    }
    let apps = host.apps.clone()?;
    let args = v.build_args(host.os).ok_or("no-face")?;
    let checkout = host.checkout.as_ref().ok_or("no-checkout")?;
    let toolchain = if !host.rustup {
        Err("no-rustup")
    } else {
        match (host.os, v.face(host.os)) {
            (Os::MacOs, _) if host.toolkit != Toolkit::AppKit => Err("no-xcode-clt"),
            (Os::Linux, Face::Feature("gtk")) if !host.toolkit.has_gtk4() => Err("no-gtk4-dev"),
            _ => Ok(()),
        }
    };
    let (build, why) = match toolchain {
        Ok(()) => (Some(args), None),
        Err(why) if prebuilt => (None, Some(why)),
        Err(why) => return Err(why),
    };
    Ok(Plan {
        build,
        why,
        from: checkout.join("target/release").join(v.krate),
        dest: apps.bin.join(v.krate),
        desktop: apps.desktop.map(|d| d.join(format!("{}.desktop", v.krate))),
    })
}

/// The Linux desktop entry for an installed vessel.
pub fn desktop_entry(v: &Vessel, bin: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nComment=UnaOS {}\nExec={}\nTerminal=false\nCategories=Utility;\n",
        v.name,
        v.about,
        bin.display()
    )
}
