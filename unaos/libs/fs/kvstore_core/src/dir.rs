//! [`DirBacking`] — a [`Backing`] over one host directory (feature `std`): append + fsync, replace through a
//! temp file + fsync + rename (POSIX rename is atomic within a directory), list skipping the temp files.

use crate::{Backing, Error};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Suffix of a replacement being written (never listed, removed on the next replace).
const TMP: &str = ".kvtmp";

/// Named files in one directory.
#[derive(Debug, Clone)]
pub struct DirBacking {
    dir: PathBuf,
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

impl DirBacking {
    /// The directory (created when absent).
    pub fn new(dir: &Path) -> Result<Self, Error> {
        fs::create_dir_all(dir).map_err(io)?;
        Ok(DirBacking { dir: dir.to_path_buf() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf, Error> {
        // A name is one path component: no separators, no parent references.
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(Error::Io(String::from("invalid file name")));
        }
        Ok(self.dir.join(name))
    }

    /// Bytes on disk of every file the backing holds (a quota estimate's usage).
    pub fn usage(&self) -> u64 {
        fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }
}

impl Backing for DirBacking {
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        match fs::read(self.path(name)?) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let mut f = OpenOptions::new().create(true).append(true).open(self.path(name)?).map_err(io)?;
        f.write_all(bytes).map_err(io)?;
        f.sync_data().map_err(io)
    }

    fn replace(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let dst = self.path(name)?;
        let tmp = self.path(&alloc::format!("{name}{TMP}"))?;
        {
            let mut f = fs::File::create(&tmp).map_err(io)?;
            f.write_all(bytes).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        fs::rename(&tmp, &dst).map_err(io)?;
        // Make the rename itself durable (best effort: not every platform opens directories).
        if let Ok(d) = fs::File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    fn remove(&mut self, name: &str) -> Result<(), Error> {
        match fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn list(&mut self) -> Result<Vec<String>, Error> {
        let mut out = Vec::new();
        for e in fs::read_dir(&self.dir).map_err(io)?.flatten() {
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if let Some(n) = e.file_name().to_str() {
                if !n.ends_with(TMP) {
                    out.push(n.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}
