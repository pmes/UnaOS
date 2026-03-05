// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// =====================================================================
// CRATE: libs/elessar/src/lib.rs
// DESCRIPTION: The Context Engine. Pure logic. Zero UI dependencies.
// =====================================================================

//! Elessar is the sensory cortex for project and spatial awareness.
//! It determines the "Spline" (the trajectory/type) of a given directory.
//! This crate is strictly pure logic. It contains NO user interface code.

// Connects to the spatial indexing logic (e.g., context/indexer.rs)
pub mod context;

use std::path::Path;

/// Represents the fundamental nature of a workspace or directory.
/// We call this the "Spline" - the mathematical curve that defines the project's trajectory.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Spline {
    /// The Monolith itself. Defined by the presence of MEMORIA.md.
    UnaOS,
    /// A Rust Crate (Cargo.toml).
    Rust,
    /// A Node/Web Project (package.json).
    Web,
    /// A Python Project (requirements.txt / pyproject.toml).
    Python,
    /// Unknown territory.
    Void,
}

/// The Context holds the spatial and structural awareness of our current environment.
pub struct Context {
    pub path: std::path::PathBuf,
    pub spline: Spline,
}

impl Context {
    /// Scans the given path to determine its Spline.
    /// This is the sensory input for Elessar's context awareness.
    pub fn new(path: &Path) -> Self {
        let spline = detect_spline(path);
        Self {
            path: path.to_path_buf(),
            spline,
        }
    }
}

/// Interrogates the directory structure to identify the project type.
/// Generously commented to leave no doubt about the engine's logic.
fn detect_spline(path: &Path) -> Spline {
    // If it contains our memory core, it is our own flesh and blood.
    if path.join("MEMORIA.md").exists() {
        return Spline::UnaOS;
    }
    // Standard Rust ecosystem detection.
    if path.join("Cargo.toml").exists() {
        return Spline::Rust;
    }
    // Standard Node/Web ecosystem detection.
    if path.join("package.json").exists() {
        return Spline::Web;
    }
    // Python environments often use either of these.
    if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() {
        return Spline::Python;
    }

    // If it matches nothing, it is the Void.
    Spline::Void
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_self_recognition() {
        // We assume we are running tests from inside libs/elessar or workspace root.
        let mut current = env::current_dir().unwrap();

        // Walk up the directory tree until we find MEMORIA.md or hit the root.
        loop {
            if current.join("MEMORIA.md").exists() {
                let ctx = Context::new(&current);
                assert_eq!(ctx.spline, Spline::UnaOS);
                return;
            }
            if !current.pop() {
                break;
            }
        }

        // Fallback for CI environments where we just check for Cargo.toml
        let ctx = Context::new(&env::current_dir().unwrap());
        assert!(matches!(ctx.spline, Spline::Rust | Spline::UnaOS));
    }
}
