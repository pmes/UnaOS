// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The keyring on a UnaFS volume: `/home/<u>/.holocron/.ring` and `/home/<u>/.holocron/<ns>/<name>`,
//! one secret per file, with the metadata ALSO set as typed UnaFS attributes on the file's inode:
//!
//! | key | type | value |
//! |---|---|---|
//! | `created` | Int | unix seconds |
//! | `kind` | String | `api-key`, `password`, `ssh-ed25519`, … |
//! | `label` | String | the human label |
//!
//! so `query kind == "ssh-ed25519"` finds the agent's keys without opening a sealed body. The attributes
//! are a mirror for search; the authority is the authenticated header (a tampered attribute changes
//! nothing Holocron returns). This is the host twin of what HOLOCRON.ELF does on the metal through
//! SYS_OPEN/SYS_ATTR_SET (ATTRSURF): the same crate (`unafs`), the same attribute engine.

use holocron_core::format::Meta;
use holocron_core::name;
use holocron_core::service::{Store, StoreError};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use unafs::fs::FileSystemError;
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, FileKind, UnaFS};

/// The keyring store on a UnaFS volume.
pub struct UnaFsStore<D: BlockDevice> {
    fs: UnaFS<D>,
    base: String,
}

fn e(_: FileSystemError) -> StoreError {
    StoreError
}

impl<D: BlockDevice> UnaFsStore<D> {
    /// The store for user `user` on `fs` (`/home/<user>/.holocron`).
    pub fn new(fs: UnaFS<D>, user: &str) -> Result<Self, StoreError> {
        if !name::valid(user) {
            return Err(StoreError);
        }
        Ok(UnaFsStore { fs, base: holocron_core::root::root(&format!("/home/{user}")) }) // B448: the one root
    }

    /// The volume (attribute checks, `query`).
    pub fn fs(&mut self) -> &mut UnaFS<D> {
        &mut self.fs
    }

    /// The base directory.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn dir(&mut self, path: &str, create: bool) -> Result<Option<u64>, StoreError> {
        let mut cur = self.fs.resolve_path("/").map_err(e)?;
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            let found = self.fs.ls(cur).map_err(e)?.into_iter().find(|d| d.name == comp);
            cur = match found {
                Some(d) if d.kind == FileKind::Directory => d.inode_id,
                Some(_) => return Err(StoreError),
                None if create => self.fs.mkdir(cur, comp.to_string()).map_err(e)?,
                None => return Ok(None),
            };
        }
        Ok(Some(cur))
    }

    fn read_in(&mut self, dir: u64, leaf: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let Some(ent) = self.fs.ls(dir).map_err(e)?.into_iter().find(|d| d.name == leaf) else {
            return Ok(None);
        };
        if ent.kind != FileKind::File {
            return Err(StoreError);
        }
        let size = self.fs.read_inode(ent.inode_id).map_err(e)?.size;
        self.fs.read_data(ent.inode_id, 0, size).map(Some).map_err(e)
    }

    /// Replace `leaf` in `dir` with `bytes` (UnaFS `write_data` is grow-only, so a replace is a fresh
    /// inode). Returns the new inode id.
    fn replace_in(&mut self, dir: u64, leaf: &str, bytes: &[u8]) -> Result<u64, StoreError> {
        if self.fs.ls(dir).map_err(e)?.iter().any(|d| d.name == leaf) {
            self.fs.unlink(dir, leaf).map_err(e)?;
        }
        let id = self.fs.create_file(dir, leaf.to_string()).map_err(e)?;
        self.fs.write_data(id, 0, bytes).map_err(e)?;
        Ok(id)
    }
}

impl<D: BlockDevice> Store for UnaFsStore<D> {
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        let base = self.base.clone();
        match self.dir(&base, false)? {
            Some(d) => self.read_in(d, crate::store::RING_FILE),
            None => Ok(None),
        }
    }

    fn write_ring(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        let base = self.base.clone();
        let d = self.dir(&base, true)?.ok_or(StoreError)?;
        self.replace_in(d, crate::store::RING_FILE, bytes).map(|_| ())
    }

    fn read(&mut self, ns: &str, name_: &str) -> Result<Option<Vec<u8>>, StoreError> {
        if !name::valid(ns) || !name::valid(name_) {
            return Err(StoreError);
        }
        let p = format!("{}/{ns}", self.base);
        match self.dir(&p, false)? {
            Some(d) => self.read_in(d, name_),
            None => Ok(None),
        }
    }

    fn write(&mut self, ns: &str, name_: &str, file: &[u8], meta: &Meta) -> Result<(), StoreError> {
        if !name::valid(ns) || !name::valid(name_) {
            return Err(StoreError);
        }
        let p = format!("{}/{ns}", self.base);
        let d = self.dir(&p, true)?.ok_or(StoreError)?;
        let id = self.replace_in(d, name_, file)?;
        self.fs.set_attribute(id, "created".into(), AttributeValue::Int(meta.created)).map_err(e)?;
        self.fs.set_attribute(id, "kind".into(), AttributeValue::String(meta.kind.clone())).map_err(e)?;
        self.fs.set_attribute(id, "label".into(), AttributeValue::String(meta.label.clone())).map_err(e)?;
        Ok(())
    }

    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError> {
        if !name::valid(ns) {
            return Err(StoreError);
        }
        let p = format!("{}/{ns}", self.base);
        let Some(d) = self.dir(&p, false)? else { return Ok(Vec::new()) };
        Ok(self
            .fs
            .ls(d)
            .map_err(e)?
            .into_iter()
            .filter(|x| x.kind == FileKind::File && name::valid(&x.name))
            .map(|x| x.name)
            .collect())
    }

    fn remove(&mut self, ns: &str, name_: &str) -> Result<bool, StoreError> {
        if !name::valid(ns) || !name::valid(name_) {
            return Err(StoreError);
        }
        let p = format!("{}/{ns}", self.base);
        let Some(d) = self.dir(&p, false)? else { return Ok(false) };
        match self.fs.unlink(d, name_) {
            Ok(_) => Ok(true),
            Err(FileSystemError::NotFound) => Ok(false),
            Err(x) => Err(e(x)),
        }
    }
}

/// A UnaFS block device over a host image file (the `unafs` crate's own `FileDevice` lives behind its
/// `std` feature, which would pull gneiss_pal's network stack into the keyring).
pub struct ImageFile {
    file: File,
    blocks: u64,
}

impl ImageFile {
    /// Open an existing image read-write.
    pub fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let blocks = file.metadata()?.len() / BLOCK_SIZE;
        Ok(ImageFile { file, blocks })
    }
}

impl BlockDevice for ImageFile {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), unafs::storage::Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(unafs::storage::Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }
        if id >= self.blocks {
            return Err(unafs::storage::Error::OutOfBounds(id));
        }
        self.file
            .seek(SeekFrom::Start(id * BLOCK_SIZE))
            .and_then(|_| self.file.read_exact(buf))
            .map_err(|x| unafs::storage::Error::Io(x.to_string()))
    }
    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), unafs::storage::Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(unafs::storage::Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }
        if id >= self.blocks {
            return Err(unafs::storage::Error::OutOfBounds(id));
        }
        self.file
            .seek(SeekFrom::Start(id * BLOCK_SIZE))
            .and_then(|_| self.file.write_all(buf))
            .map_err(|x| unafs::storage::Error::Io(x.to_string()))
    }
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn flush(&mut self) -> Result<(), unafs::storage::Error> {
        self.file.sync_all().map_err(|x| unafs::storage::Error::Io(x.to_string()))
    }
}
