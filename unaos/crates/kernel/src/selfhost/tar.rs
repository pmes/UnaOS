// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFHOST-2 — a streaming tar (POSIX ustar / GNU) member walker, no_std + alloc.
//
// This is a WALKER, not an extractor: it consumes the decompressed stream 512 bytes at a time and
// keeps a census, never the contents. That is deliberate for this rung. The claim being proven is
// "the shard can read its own source tree" — enumerate it, count it, and check the shape of it. A
// write path (extracting members onto a volume) is a separate rung with a separate blast radius, and
// nothing here can touch media.
//
// FORMAT NOTES, because `git archive` is the only producer we care about:
//   * Members are ustar (`magic[257..262] == "ustar"`), with the long-path `prefix` field at 345.
//   * Long paths that still do not fit produce a PAX extended header (typeflag 'x') whose payload we
//     SKIP — the member it describes follows immediately with a truncated name we count anyway. GNU
//     long-name records ('L') are handled the same way. Neither appears in a normal UnaOS archive;
//     they are handled so a future deeper tree does not silently break the count.
//   * The archive ends with two consecutive all-zero blocks.

use alloc::string::String;

use super::inflate::Sink;

const BLOCK: usize = 512;

/// A running census of the archive.
#[derive(Debug, Clone, Copy, Default)]
pub struct TarCensus {
    /// Regular files (typeflag '0' or NUL).
    pub files: u32,
    /// Directory members (typeflag '5').
    pub dirs: u32,
    /// Anything else that carried a name (symlinks, PAX/GNU metadata records).
    pub other: u32,
    /// Summed `size` field of every regular file — the tree's real byte count.
    pub bytes: u64,
    /// Members whose path starts with `target/`. MUST be 0: build output is never archived, and
    /// `source_along.md` names this exact check as the way to tell a source payload from a dump.
    pub target_members: u32,
    /// The two-zero-block end-of-archive marker was reached.
    pub saw_end: bool,
}

/// Why a walk gave up. A tar stream that does not parse is a decoder bug or a corrupt payload; either
/// way the witness must FAIL rather than report a partial census as a success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarError {
    /// A header block that is neither all-zero nor a valid ustar header.
    BadHeader,
    /// The header's own octal checksum does not match its bytes.
    BadChecksum,
    /// A size field that is not octal.
    BadSize,
    /// Bytes arrived after the end-of-archive marker.
    TrailingData,
}

/// The walker's state machine. It is a `Sink`, so it plugs straight into `inflate::gunzip` and the
/// tar stream is never materialised.
pub struct TarWalk {
    block: [u8; BLOCK],
    filled: usize,
    /// Remaining 512-byte blocks of the current member's payload to skip.
    skip_blocks: u64,
    zero_run: u32,
    census: TarCensus,
    error: Option<TarError>,
    /// The first regular-file path seen, kept as a cheap "this is really our tree" sample for the
    /// witness line. `git archive` emits members in tree order, so this is stable.
    first_name: Option<String>,
}

impl TarWalk {
    pub fn new() -> Self {
        Self {
            block: [0u8; BLOCK],
            filled: 0,
            skip_blocks: 0,
            zero_run: 0,
            census: TarCensus::default(),
            error: None,
            first_name: None,
        }
    }

    pub fn census(&self) -> TarCensus {
        self.census
    }

    pub fn error(&self) -> Option<TarError> {
        self.error
    }

    pub fn first_name(&self) -> Option<&str> {
        self.first_name.as_deref()
    }

    /// Finish the walk. A stream that stopped mid-member, or never reached the two-zero-block
    /// marker, is not a complete archive.
    pub fn finish(self) -> Result<(TarCensus, Option<String>), TarError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if !self.census.saw_end {
            return Err(TarError::BadHeader);
        }
        Ok((self.census, self.first_name))
    }

    fn on_block(&mut self) -> Result<(), TarError> {
        // Payload blocks are skipped without inspection.
        if self.skip_blocks > 0 {
            self.skip_blocks -= 1;
            return Ok(());
        }

        if self.block.iter().all(|&b| b == 0) {
            self.zero_run += 1;
            if self.zero_run >= 2 {
                self.census.saw_end = true;
            }
            return Ok(());
        }
        if self.census.saw_end {
            return Err(TarError::TrailingData);
        }
        self.zero_run = 0;

        // ustar magic at 257..262. `git archive` always writes it.
        if &self.block[257..262] != b"ustar" {
            return Err(TarError::BadHeader);
        }
        verify_checksum(&self.block)?;

        let size = octal(&self.block[124..136]).ok_or(TarError::BadSize)?;
        let typeflag = self.block[156];
        let name = full_path(&self.block);

        self.skip_blocks = size.div_ceil(BLOCK as u64);

        if name.starts_with("target/") {
            self.census.target_members += 1;
        }
        match typeflag {
            b'0' | 0 => {
                self.census.files += 1;
                self.census.bytes += size;
                if self.first_name.is_none() {
                    self.first_name = Some(name);
                }
            }
            b'5' => self.census.dirs += 1,
            _ => self.census.other += 1,
        }
        Ok(())
    }
}

impl Default for TarWalk {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for TarWalk {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.error.is_some() {
            return Err(());
        }
        self.block[self.filled] = byte;
        self.filled += 1;
        if self.filled == BLOCK {
            self.filled = 0;
            if let Err(e) = self.on_block() {
                self.error = Some(e);
                return Err(());
            }
        }
        Ok(())
    }
}

/// `prefix` + `/` + `name`, both NUL-padded (POSIX.1-1988 ustar).
fn full_path(block: &[u8; BLOCK]) -> String {
    let name = cstr(&block[0..100]);
    let prefix = cstr(&block[345..500]);
    let mut s = String::new();
    if !prefix.is_empty() {
        s.push_str(prefix);
        s.push('/');
    }
    s.push_str(name);
    s
}

fn cstr(field: &[u8]) -> &str {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).unwrap_or("")
}

/// A NUL/space-padded octal field. Leading and trailing spaces are legal padding in the wild.
fn octal(field: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    let mut any = false;
    for &b in field {
        match b {
            b'0'..=b'7' => {
                v = v.checked_mul(8)?.checked_add((b - b'0') as u64)?;
                any = true;
            }
            b' ' | 0 => {
                if any {
                    break;
                }
            }
            _ => return None,
        }
    }
    if any { Some(v) } else { None }
}

/// The header checksum: the sum of all 512 header bytes with the checksum field itself read as eight
/// spaces. Both the unsigned and the (historical) signed interpretation are accepted.
fn verify_checksum(block: &[u8; BLOCK]) -> Result<(), TarError> {
    let want = octal(&block[148..156]).ok_or(TarError::BadChecksum)?;
    let mut unsigned: u64 = 0;
    let mut signed: i64 = 0;
    for (i, &b) in block.iter().enumerate() {
        let v = if (148..156).contains(&i) { b' ' } else { b };
        unsigned += v as u64;
        signed += v as i8 as i64;
    }
    if want == unsigned || want as i64 == signed {
        Ok(())
    } else {
        Err(TarError::BadChecksum)
    }
}

/// A human-readable reason for a FAIL line.
pub fn tar_reason(e: TarError) -> &'static str {
    match e {
        TarError::BadHeader => "malformed or truncated tar header",
        TarError::BadChecksum => "tar header checksum mismatch",
        TarError::BadSize => "non-octal tar size field",
        TarError::TrailingData => "data after end-of-archive marker",
    }
}
