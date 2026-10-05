// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `FileBlock` — a [`Block`] over a host file or device node (the `std` feature only; the kernel
//! builds this crate with `default-features = false` and never sees this module). Opened READ-ONLY
//! unless the caller asks for writes; the size is taken once at open (`seek(End)`, which is also how
//! a Linux block device reports its length) and every access is bounded by it.

extern crate std;

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::block::{check, Block, BlockError};
use crate::gpt::SECTOR;

pub struct FileBlock {
    file: File,
    sectors: u64,
    writable: bool,
}

impl FileBlock {
    /// Open read-only.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Self::with(OpenOptions::new().read(true).open(path)?, false)
    }
    /// Open read-write (the caller has already applied its safety policy).
    pub fn open_rw(path: &Path) -> std::io::Result<Self> {
        Self::with(OpenOptions::new().read(true).write(true).open(path)?, true)
    }
    fn with(mut file: File, writable: bool) -> std::io::Result<Self> {
        let len = file.seek(SeekFrom::End(0))?;
        Ok(Self { file, sectors: len / SECTOR as u64, writable })
    }
    pub fn writable(&self) -> bool {
        self.writable
    }
}

impl Block for FileBlock {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check(self.sectors, lba, buf.len())?;
        self.file.seek(SeekFrom::Start(lba * SECTOR as u64)).map_err(|_| BlockError::Io)?;
        self.file.read_exact(buf).map_err(|_| BlockError::Io)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if !self.writable {
            return Err(BlockError::ReadOnly);
        }
        check(self.sectors, lba, buf.len())?;
        self.file.seek(SeekFrom::Start(lba * SECTOR as u64)).map_err(|_| BlockError::Io)?;
        self.file.write_all(buf).map_err(|_| BlockError::Io)
    }
    fn flush(&mut self) -> Result<(), BlockError> {
        if self.writable {
            self.file.sync_all().map_err(|_| BlockError::Io)?;
        }
        Ok(())
    }
}
