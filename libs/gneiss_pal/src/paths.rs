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

use std::env;
use std::fs;
use std::path::PathBuf;

/// The Spatial Truth of UnaOS.
pub struct UnaPaths;

impl UnaPaths {
    pub fn root() -> PathBuf {
        if let Ok(root) = env::var("UNA_ROOT") {
            return PathBuf::from(root);
        }

        let mut vault = PathBuf::from(
            env::var("HOME").expect("CRITICAL: HOME environment variable missing. Engine stalled."),
        );

        #[cfg(target_os = "macos")]
        {
            vault.push("Library/Application Support/unaos");
        }

        #[cfg(not(target_os = "macos"))]
        {
            // Linux / Default to XDG-ish structure
            vault.push(".local/share/unaos");
        }

        vault
    }

    /// The AI Cortex (Vein) - LLM Models and Subconscious
    pub fn cortex() -> PathBuf {
        Self::root().join("cortex")
    }

    /// The Subconscious Vault (Raw Telemetry)
    pub fn subconscious_vault() -> PathBuf {
        Self::cortex().join("subconscious.ufs")
    }

    /// The Memory Vault (UnaFS) - The Encrypted Block Storage
    pub fn vault() -> PathBuf {
        Self::root().join("vault")
    }

    /// The Primary Conscious Memory (Replaces lumen_storage)
    pub fn primary_vault() -> PathBuf {
        Self::vault().join("primary.ufs")
    }

    /// System Policy (Principia) - OS Configuration
    pub fn config() -> PathBuf {
        Self::root().join("principia")
    }

    /// Bootstraps the physical directory structure. Fails hard if the host rejects us.
    pub fn awaken() -> Result<(), String> {
        let required_nodes = [
            Self::cortex(),
            Self::vault(),
            Self::config(),
            Self::root().join("logs"),
        ];

        for node in required_nodes {
            if !node.exists() {
                fs::create_dir_all(&node).map_err(|e| {
                    format!(
                        "CRITICAL: Failed to carve spatial anchor at {}: {}",
                        node.display(),
                        e
                    )
                })?;
            }
        }

        Ok(())
    }
}
