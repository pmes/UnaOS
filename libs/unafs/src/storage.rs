use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use thiserror::Error;

/// The fundamental atomic unit of the file system.
/// All reads and writes must be aligned to this size.
pub const BLOCK_SIZE: u64 = 4096;

/// Error types related to block storage operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Buffer size does not match BLOCK_SIZE.
    #[error("Buffer size {0} does not match block size {1}")]
    BadBlockSize(usize, u64),
    /// Generic IO error.
    #[error("IO Error: {0}")]
    Io(String),
    /// Attempted to access a block outside the device boundaries.
    #[error("Block out of bounds: {0}")]
    OutOfBounds(u64),
}

/// A trait representing a block storage device.
///
/// This trait abstracts over physical disks, memory buffers, or network stores.
/// Implementations must enforce `BLOCK_SIZE` alignment.
pub trait BlockDevice {
    /// Read a block from the device.
    ///
    /// # Arguments
    /// * `id` - The block ID to read.
    /// * `buf` - The buffer to read into. Must be exactly `BLOCK_SIZE` bytes.
    fn read_block(&self, id: u64, buf: &mut [u8]) -> Result<(), Error>;

    /// Write a block to the device.
    ///
    /// # Arguments
    /// * `id` - The block ID to write to.
    /// * `buf` - The buffer to write. Must be exactly `BLOCK_SIZE` bytes.
    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error>;

    /// Return the total number of blocks in the device.
    fn block_count(&self) -> u64;
}

/// A block device backed by a file on the host OS.
pub struct FileDevice {
    file: File,
}

impl FileDevice {
    /// Open a file as a block device.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        Ok(Self { file })
    }
}

impl BlockDevice for FileDevice {
    fn read_block(&self, id: u64, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        // We need mutable access to the file to seek/read, but the trait takes &self.
        // `File` has internal mutability for I/O operations in some contexts, but `Read` requires `&mut self`.
        // However, `&File` implements `Read` and `Write` in recent Rust versions?
        // No, `&File` implements `Read` and `Write` via system calls which are thread-safe (mostly),
        // BUT `Seek` on `&File` is tricky.
        // Actually, looking at std::fs::File, `Read` is implemented for `&File`.
        // Wait, `impl Read for &File` exists.
        // So we can use `(&self.file).seek(...)`? No, seek modifies the file pointer.
        // If we have multiple threads reading, seeking would race.
        // But here we are single-threaded for now or need to be careful.
        // The trait definition `fn read_block(&self, ...)` implies concurrent reads might be possible.
        // `File` in Rust:
        // `impl Read for &File`
        // `impl Write for &File`
        // `impl Seek for &File`
        // Yes, all implemented for `&File`. This allows concurrent access if the OS supports it (e.g. pread/pwrite).
        // However, `seek` + `read` is not atomic.
        // If we use `seek` then `read`, another thread could `seek` in between.
        // For `FileDevice`, we should ideally use `std::os::unix::fs::FileExt` for `read_at` / `write_at` to avoid seek races,
        // but that's Unix-specific.
        // The instructions say "Wrapper around `std::fs::File`... Use `file.seek(...)` then `read_exact`".
        // If the trait signature is `&self` for read, and we use `seek`, we have a race condition if shared.
        // But for this task, we can assume single ownership or use a Mutex if needed.
        // But `File` methods for `&File` use the shared file descriptor.
        // Given the instructions, I will use `seek` and `read_exact` on `&self.file`.
        // Note: `seek` takes `&mut self` on `File`, but `&File` implements `Seek`?
        // Let's check `std::fs::File`.
        // `impl Seek for &File` exists.
        // So we can do `(&self.file).seek(...)`.

        let mut file = &self.file;
        // seek to offset
        file.seek(SeekFrom::Start(id * BLOCK_SIZE))
            .map_err(|e| Error::Io(e.to_string()))?;

        // read exact
        file.read_exact(buf)
            .map_err(|e| Error::Io(e.to_string()))?;

        Ok(())
    }

    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let mut file = &self.file;
        file.seek(SeekFrom::Start(id * BLOCK_SIZE))
            .map_err(|e| Error::Io(e.to_string()))?;

        file.write_all(buf)
            .map_err(|e| Error::Io(e.to_string()))?;

        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.file.metadata()
            .map(|m| m.len() / BLOCK_SIZE)
            .unwrap_or(0)
    }
}

/// An in-memory block device backed by a `Vec<u8>`.
///
/// Useful for testing and temporary filesystems.
pub struct MemDevice {
    data: Vec<u8>,
}

impl Default for MemDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl MemDevice {
    /// Create a new empty `MemDevice`.
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }
}

impl BlockDevice for MemDevice {
    fn read_block(&self, id: u64, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let start = (id * BLOCK_SIZE) as usize;
        let end = start + BLOCK_SIZE as usize;

        if start >= self.data.len() {
            // Reading uninitialized memory - return zeros? Or error?
            // "We define our own reality."
            // Standard behavior for unwritten blocks is usually zeros or error.
            // Let's return zeros for simplicity in a "sparse" like feel,
            // but strict implementation might error.
            // Given "MemDevice (RAM Disk)", usually it has a size.
            // If I try to read beyond the end of the "disk", it should probably be an error or zeros.
            // Let's assume explicit resizing is not in the trait, so write expands, read beyond end returns zeros?
            // Or should read fail if not written?
            // "The file system never hangs."
            // Let's go with: if it's out of bounds of the current vector, we treat it as zeros (sparse)
            // OR we return OutOfBounds.
            // Let's return OutOfBounds for now to be safe, unless we want auto-growth on read (which is weird).
            // Actually, if we want to support "Resizing", maybe we should just error if out of bounds.
            return Err(Error::OutOfBounds(id));
        }

        // partial read handling if data len is not multiple of block size (shouldn't happen if we only write blocks)
        if end > self.data.len() {
             return Err(Error::OutOfBounds(id));
        }

        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let start = (id * BLOCK_SIZE) as usize;
        let end = start + BLOCK_SIZE as usize;

        if end > self.data.len() {
            // Resize to accommodate new block
            self.data.resize(end, 0);
        }

        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn block_count(&self) -> u64 {
        (self.data.len() as u64) / BLOCK_SIZE
    }
}
