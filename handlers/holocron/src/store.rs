// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The host directory store: `<root>/.ring` and `<root>/<ns>/<name>`, `<root>` = `$HOLOCRON_HOME` or
//! `$HOME/.holocron` (on UnaOS: `/home/<u>/.holocron`).
//!
//! * The root and every namespace directory are created `0700`; files `0600`.
//! * The store refuses to operate on a root that is not a directory owned by this uid with no
//!   group/other permission bits, and never follows a symlink inside it.
//! * Writes are atomic: a hidden temp file in the same directory (`.<name>.tmp`, a name the
//!   [`name::valid`](holocron_core::name::valid) alphabet can never produce), `fsync`, `rename`,
//!   `fsync` of the directory.
//! * The host filesystem has no typed attributes; the metadata lives in the file's authenticated
//!   header (which is what `SecretList` reads on every store). The UnaFS store
//!   ([`crate::unafs_store`]) additionally sets them as typed attributes for `query`.

use holocron_core::format::Meta;
use holocron_core::name;
use holocron_core::service::{Store, StoreError};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The ring file's name inside the root (`holocron_core::root::RING`).
pub const RING_FILE: &str = holocron_core::root::RING;

/// The default root: `$HOLOCRON_HOME`, else `$HOME/.holocron` — THE root `holocron_core::root` names for both
/// rings (HOLOCRONROOT, B448).
pub fn default_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HOLOCRON_HOME") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(holocron_core::root::DIR))
}

/// The effective uid of this process (the owner of `/proc/self`), without libc.
pub fn my_uid() -> u32 {
    fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX)
}

/// The directory store.
#[derive(Debug, Clone)]
pub struct DirStore {
    root: PathBuf,
    uid: u32,
}

fn log(what: &str, p: &Path, e: &dyn std::fmt::Display) {
    eprintln!(":: HOLOCRON: store {what} {} : {e} ::", p.display());
}

impl DirStore {
    /// A store rooted at `root` (created on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirStore { root: root.into(), uid: my_uid() }
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check (and with `create`, make) a private directory.
    fn private_dir(&self, p: &Path, create: bool) -> Result<bool, StoreError> {
        match fs::symlink_metadata(p) {
            Ok(m) => {
                if !m.file_type().is_dir() || m.uid() != self.uid || m.mode() & 0o077 != 0 {
                    log("refused (not a 0700 directory owned by this uid)", p, &format!("mode={:o} uid={}", m.mode() & 0o7777, m.uid()));
                    return Err(StoreError);
                }
                Ok(true)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if !create {
                    return Ok(false);
                }
                fs::DirBuilder::new().mode(0o700).recursive(false).create(p).map_err(|e| {
                    log("mkdir", p, &e);
                    StoreError
                })?;
                // The umask may have narrowed it further; make it exactly 0700.
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).map_err(|_| StoreError)?;
                Ok(true)
            }
            Err(e) => {
                log("stat", p, &e);
                Err(StoreError)
            }
        }
    }

    fn ensure_root(&self, create: bool) -> Result<bool, StoreError> {
        if create {
            if let Some(parent) = self.root.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    fs::create_dir_all(parent).map_err(|_| StoreError)?;
                }
            }
        }
        self.private_dir(&self.root, create)
    }

    fn ns_dir(&self, ns: &str, create: bool) -> Result<Option<PathBuf>, StoreError> {
        if !name::valid(ns) {
            return Err(StoreError);
        }
        if !self.ensure_root(create)? {
            return Ok(None);
        }
        let d = self.root.join(ns);
        Ok(self.private_dir(&d, create)?.then_some(d))
    }

    fn read_file(&self, p: &Path) -> Result<Option<Vec<u8>>, StoreError> {
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => {
                log("refused (not a regular file)", p, &"symlink or special");
                return Err(StoreError);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                log("stat", p, &e);
                return Err(StoreError);
            }
        }
        let mut v = Vec::new();
        File::open(p).and_then(|mut f| f.read_to_end(&mut v)).map_err(|e| {
            log("read", p, &e);
            StoreError
        })?;
        Ok(Some(v))
    }

    fn write_atomic(&self, dir: &Path, leaf: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let tmp = dir.join(format!(".{leaf}.tmp"));
        let dst = dir.join(leaf);
        let _ = fs::remove_file(&tmp);
        let r = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, &dst)?;
            File::open(dir)?.sync_all()
        })();
        r.map_err(|e| {
            let _ = fs::remove_file(&tmp);
            log("write", &dst, &e);
            StoreError
        })
    }
}

impl Store for DirStore {
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        if !self.ensure_root(false)? {
            return Ok(None);
        }
        self.read_file(&self.root.join(RING_FILE))
    }

    fn write_ring(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        self.ensure_root(true)?;
        self.write_atomic(&self.root.clone(), RING_FILE, bytes)
    }

    fn read(&mut self, ns: &str, name_: &str) -> Result<Option<Vec<u8>>, StoreError> {
        if !name::valid(name_) {
            return Err(StoreError);
        }
        match self.ns_dir(ns, false)? {
            Some(d) => self.read_file(&d.join(name_)),
            None => Ok(None),
        }
    }

    fn write(&mut self, ns: &str, name_: &str, file: &[u8], _meta: &Meta) -> Result<(), StoreError> {
        if !name::valid(name_) {
            return Err(StoreError);
        }
        let d = self.ns_dir(ns, true)?.ok_or(StoreError)?;
        self.write_atomic(&d, name_, file)
    }

    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError> {
        let Some(d) = self.ns_dir(ns, false)? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for e in fs::read_dir(&d).map_err(|_| StoreError)? {
            let e = e.map_err(|_| StoreError)?;
            if let Some(n) = e.file_name().to_str() {
                if name::valid(n) && e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.push(n.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn remove(&mut self, ns: &str, name_: &str) -> Result<bool, StoreError> {
        if !name::valid(name_) {
            return Err(StoreError);
        }
        let Some(d) = self.ns_dir(ns, false)? else { return Ok(false) };
        match fs::remove_file(d.join(name_)) {
            Ok(()) => {
                let _ = File::open(&d).and_then(|f| f.sync_all());
                Ok(true)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => {
                log("remove", &d.join(name_), &e);
                Err(StoreError)
            }
        }
    }
}
