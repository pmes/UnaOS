use std::fs::File;
use std::path::Path;
use memmap2::Mmap;
use gneiss_pal::io::MemoryMappedRegion;

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
