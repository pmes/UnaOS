//! The transcript sink (design §3.4).
//!
//! Every iteration appends: the assembled context, the exact request (minus
//! credentials), the response, the parsed action, and the resulting verdict
//! table. The transcript is the artifact a bench sitting keeps.
//!
//! Credentials, tokens, and any Holocron-supplied material are NEVER written
//! here. Nothing in this module ever receives a credential: the connector holds
//! it privately and the neutral `Request` type has no field for one.
//!
//! It is a SINK the CLI passes in, so the `UnaOS_Installer` vessel can route
//! transcripts to its own storage without changing the modules above it.

use std::io::Write;
use std::path::{Path, PathBuf};

pub trait Transcript {
    /// Append one titled section. Must be durable before the next call returns
    /// for the "written BEFORE any send" guarantee to mean anything.
    fn section(&mut self, title: &str, body: &str) -> std::io::Result<()>;
}

/// Appends to a file, flushing each section.
pub struct FileTranscript {
    path: PathBuf,
    file: std::fs::File,
}

impl FileTranscript {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(FileTranscript { path: path.to_path_buf(), file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Transcript for FileTranscript {
    fn section(&mut self, title: &str, body: &str) -> std::io::Result<()> {
        writeln!(self.file, "\n───── {title} ─────")?;
        writeln!(self.file, "{body}")?;
        self.file.flush()?;
        self.file.sync_data()
    }
}

/// Discards everything. Used when `--transcript` is not given, so `main` never
/// branches on `Option<Transcript>`.
pub struct NullTranscript;

impl Transcript for NullTranscript {
    fn section(&mut self, _title: &str, _body: &str) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_transcript_appends_and_is_durable_per_section() {
        let dir = std::env::temp_dir().join(format!("foreman-t-{}", std::process::id()));
        let path = dir.join("t.txt");
        let _ = std::fs::remove_file(&path);
        {
            let mut t = FileTranscript::open(&path).unwrap();
            t.section("ONE", "alpha").unwrap();
            // Readable BEFORE the second section is written — this is the
            // "written before any send" guarantee.
            let mid = std::fs::read_to_string(&path).unwrap();
            assert!(mid.contains("alpha"));
            assert!(!mid.contains("beta"));
            t.section("TWO", "beta").unwrap();
        }
        let all = std::fs::read_to_string(&path).unwrap();
        assert!(all.contains("ONE") && all.contains("TWO") && all.contains("beta"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn null_transcript_is_a_no_op() {
        assert!(NullTranscript.section("x", "y").is_ok());
    }
}
