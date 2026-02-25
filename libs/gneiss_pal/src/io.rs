//! The Input/Output Abstractions of the System.
//!
//! This module defines pure-Rust contracts for memory and file handling,
//! decoupled from any specific OS implementation.

/// The MemoryMappedRegion trait.
///
/// This is a pure-Rust contract. Notice that there are no OS-specific
/// dependencies here. No `memmap2`, no `libc`, no `winapi`.
///
/// Any struct that implements this trait guarantees that it holds a
/// contiguous region of memory (likely mapped directly from disk)
/// and can safely expose it as a byte slice or a string slice.
pub trait MemoryMappedRegion {
    /// Returns the mapped memory as a raw byte slice.
    fn as_slice(&self) -> &[u8];

    /// Attempts to return the mapped memory as a UTF-8 string slice.
    /// Returns an error if the memory contains invalid UTF-8.
    fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.as_slice())
    }
}
