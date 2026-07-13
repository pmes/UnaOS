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

use gneiss_pal::io::MemoryMappedRegion;
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

/// UnaFS's implementation of a memory-mapped file.
///
/// This is where the actual OS-level magic happens. We wrap `memmap2::Mmap`
/// to handle the heavy lifting of zero-copy file reading.
pub struct MappedFile {
    // The underlying memory map provided by the host OS.
    inner: Mmap,
}

impl MappedFile {
    /// Opens a file and maps it entirely into virtual memory.
    ///
    /// # Safety
    /// Memory mapping is inherently unsafe if another process modifies
    /// the file while we are reading it. For UnaOS, we treat these
    /// mappings as immutable snapshots.
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: We assume the file is not being concurrently truncated
        // by an external host process while UnaOS is analyzing it.
        // We use map() instead of map_copy() for zero-copy efficiency,
        // trusting the immutable snapshot assumption.
        // We use unsafe block because memory mapping is fundamentally unsafe
        // regarding external modifications.
        let inner = unsafe { Mmap::map(&file)? };

        Ok(Self { inner })
    }
}

// Here, we fulfill the contract defined by `gneiss_pal`.
// Now, `elessar` can consume `MappedFile` without ever knowing
// that `memmap2` or `unafs` exists!
impl MemoryMappedRegion for MappedFile {
    fn as_slice(&self) -> &[u8] {
        &self.inner
    }
}
