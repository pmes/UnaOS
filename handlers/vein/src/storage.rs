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

// use crate::model::DispatchRecord; // <-- EXCISED
use gneiss_pal::paths::UnaPaths;
use std::fs;
use std::path::PathBuf;

pub struct CortexStorage {
    base_dir: PathBuf,
}

impl CortexStorage {
    /// Initializes the Cortex Storage.
    /// It inherently trusts the Plexus Abstraction Layer.
    pub fn new() -> Self {
        let base_dir = UnaPaths::cortex();

        // Ensure our specific lobes exist.
        fs::create_dir_all(base_dir.join("models")).expect("Failed to form model lobe");
        fs::create_dir_all(base_dir.join("memories")).expect("Failed to form memory lobe");

        Self { base_dir }
    }

    #[inline]
    pub fn model_path(&self, model_name: &str) -> PathBuf {
        self.base_dir.join("models").join(model_name)
    }

    #[inline]
    pub fn memory_db(&self) -> PathBuf {
        self.base_dir.join("memories").join("vector.db")
    }
}
