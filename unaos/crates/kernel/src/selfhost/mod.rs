// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFHOST-2 — the source tree is READABLE ON THE SHARD.
//
// SOURCE-ALONG (rung 1, `docs/dev/OS/10_INSTALL/source_along.md`) put the code on the machine: every
// boot medium carries `SRC.TGZ` (`git archive HEAD | gzip -9`) and `SRC.SHA` (its sha256 plus the
// commit/branch/describe provenance) at the volume root. That rung made a claim UnaOS itself could
// not check — the verification recipe in that document is a list of commands you run on a HOST.
//
// This is rung 2, and it is the smallest honest step: UnaOS opens its own source payload, verifies it
// against the stamp beside it, decompresses it, and enumerates the tree — all on the shard, with no
// host in the loop. It is what stands between "the source is on the machine" and any future in-tree
// build tool: a compiler that cannot be handed a verified, enumerable source tree has nothing to
// compile.
//
// SCOPE, deliberately narrow:
//   * READ-ONLY. Nothing here writes to media. The FAT volume is mounted through the ordinary
//     `fs::fat` reader and only `read_at` is ever called. Extraction — materialising members onto a
//     volume — is a later rung with a genuinely different blast radius.
//   * NOTHING IS BUFFERED WHOLE. `SRC.TGZ` is ~5.6 MiB and expands to tens of MiB against a 48 MiB
//     kernel heap, and the tree only grows. The compressed file is pulled from FAT in 64 KiB chunks
//     that feed the SHA-256 and the inflater in one pass; the decompressed tar is walked 512 bytes at
//     a time and discarded. Peak cost is the 32 KiB DEFLATE window plus the chunk buffer.
//   * ARCH-NEUTRAL. It drives only `fs::fat` + `hash`, both arch-neutral, so it COMPILES on x86_64
//     and aarch64 under the `selfhost` feature. The witness call site is x86_64-only this arc,
//     because the packaged fixture (`./arroyo test-selfhost`) is the x86 usb-storage image — the same
//     split the installer engine has carried since INSTALL-CORE.
//
// FOUR INDEPENDENT CLAIMS, so a PASS is not one number vouching for itself:
//   1. sha256(SRC.TGZ) equals the digest in SRC.SHA          — the payload is the one that was packed
//   2. the gzip trailer's CRC-32 and ISIZE match what we produced — the DECODER is right, not just
//      internally consistent (a broken inflater cannot forge the packer's CRC over its own garbage)
//   3. the tar walk reaches the two-zero-block end-of-archive marker — the tree is COMPLETE
//   4. zero members under `target/`                          — this is source, not build output
//      (`source_along.md` names this exact check)
//
// Knob: `UNAOS_SELFHOST=1` ⇒ the `selfhost` cargo feature. DEFAULT OFF ⇒ this module and its call
// site vanish and every image is byte-identical to baseline.

pub mod inflate;
pub mod tar;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::fat::{self, FatError, FatFs};
use crate::hash::Sha256;

/// Read `SRC.TGZ` in chunks this big.
///
/// The size is chosen against `read_at`'s cost model, not plucked: `read_at` re-collects the cluster
/// chain from the FIRST cluster on every call, so the FAT-sector reads over a whole file go as the
/// SQUARE of the chunk count. On the fixture volumes (512-byte clusters) a ~9 MiB payload is ~18k
/// clusters, so 64 KiB chunks would cost roughly eight times the FAT traffic of 512 KiB ones for the
/// same 9 MiB of data. Half a megabyte is still noise against the 48 MiB heap.
const CHUNK: usize = 512 * 1024;

/// `SRC.SHA` is four short lines; anything larger is not the file we mean.
const SHA_FILE_MAX: usize = 4096;

static DONE: AtomicBool = AtomicBool::new(false);

/// A pull reader over a FAT file that hashes every byte it hands out.
///
/// The digest and the decompressor consume the SAME pass: the file is read off the volume exactly
/// once, which is the whole reason this is a `ByteSource` and not a `Vec<u8>`.
struct FatSource<'a> {
    fs: &'a FatFs,
    first_cluster: u32,
    size: u32,
    /// Next file offset to read from.
    pos: u32,
    buf: Vec<u8>,
    off: usize,
    sha: Sha256,
    /// Set if a FAT read failed; surfaces as `TruncatedInput` and is reported by name.
    io_error: Option<FatError>,
}

impl<'a> FatSource<'a> {
    fn new(fs: &'a FatFs, de: &fat::DirEntry) -> Self {
        Self {
            fs,
            first_cluster: de.first_cluster(),
            size: de.size,
            pos: 0,
            buf: Vec::new(),
            off: 0,
            sha: Sha256::new(),
            io_error: None,
        }
    }

    /// Refill `buf` from the volume, hashing what arrives. Returns false at EOF or on error.
    fn refill(&mut self) -> bool {
        if self.io_error.is_some() || self.pos >= self.size {
            return false;
        }
        let want = core::cmp::min(CHUNK as u32, self.size - self.pos) as usize;
        let mut out = Vec::new();
        // FATREAD-1: `read_at` REPLACES `out` (it clears first) — never pre-size this buffer.
        if let Err(e) = self.fs.read_at(self.first_cluster, self.size, self.pos, &mut out, want) {
            self.io_error = Some(e);
            return false;
        }
        if out.is_empty() {
            return false;
        }
        self.sha.update(&out);
        self.pos += out.len() as u32;
        self.buf = out;
        self.off = 0;
        true
    }

    /// Consume every remaining byte so the digest covers the WHOLE file, not just the prefix the
    /// gzip member happened to occupy. Without this a payload with trailing bytes would hash short.
    fn drain(&mut self) {
        while self.off < self.buf.len() || self.refill() {
            self.off = self.buf.len();
        }
    }
}

impl inflate::ByteSource for FatSource<'_> {
    #[inline]
    fn next(&mut self) -> Option<u8> {
        if self.off >= self.buf.len() && !self.refill() {
            return None;
        }
        let b = self.buf[self.off];
        self.off += 1;
        Some(b)
    }
}

/// What `SRC.SHA` says: the expected digest plus the provenance stamp beside it.
struct SrcStamp {
    sha: [u8; 32],
    commit: String,
    describe: String,
}

/// Parse `SRC.SHA`. Line 1 is `<64 hex>  SRC.TGZ`; the rest are `key value` provenance lines written
/// by `arroyo`'s `pack_source_along`.
fn parse_stamp(text: &str) -> Option<SrcStamp> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let hex = first.split_whitespace().next()?;
    if hex.len() != 64 {
        return None;
    }
    let mut sha = [0u8; 32];
    let bytes = hex.as_bytes();
    for i in 0..32 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        sha[i] = (hi << 4) | lo;
    }
    let mut commit = String::new();
    let mut describe = String::new();
    for line in lines {
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some("commit"), Some(v)) => commit = String::from(v),
            (Some("describe"), Some(v)) => describe = String::from(v),
            _ => {}
        }
    }
    Some(SrcStamp { sha, commit, describe })
}

/// A short human reason for a [`FatError`]. `fs::fat::fat_reason` says the same things but is
/// `#[cfg(target_arch = "aarch64")]` (it was written for the PIUSB-27 mount witness), and this module
/// is arch-neutral — so it carries its own rather than widening that one's gate from outside its lane.
fn fat_reason(e: FatError) -> &'static str {
    match e {
        FatError::NoDisk => "no block device",
        FatError::Unsupported => "unsupported sector geometry",
        FatError::Io => "block I/O error",
        FatError::NotFat => "no FAT partition/BPB found",
        FatError::BadChain => "corrupt FAT chain",
        FatError::NotFound => "entry not found",
        _ => "unreadable",
    }
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in d {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('?'));
        s.push(char::from_digit((b & 0xF) as u32, 16).unwrap_or('?'));
    }
    s
}

/// Verify + enumerate the source payload on the program-source volume, once per boot.
///
/// Called from the boot loop after storage is up, beside `fs::fat::probe_once()`.
pub fn verify_source_once() {
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    run();
}

fn run() {
    let fs = match fat::mount_program_source() {
        Ok(fs) => fs,
        Err(e) => {
            // No volume is not a verification failure — there is nothing to verify. Say so plainly;
            // this line must never read as a fault to the suite's scanner.
            serial_println!(
                ":: SELFHOST: no program-source volume ({}) — no source payload to verify ::",
                fat_reason(e)
            );
            return;
        }
    };

    let tgz = fs.find_in_root("SRC.TGZ");
    let shafile = fs.find_in_root("SRC.SHA");

    let (tgz, shafile) = match (tgz, shafile) {
        (Ok(t), Ok(s)) => (t, s),
        (Err(_), Err(_)) => {
            // A UNAOS_NOSRC medium (the FAT fixtures, vm-image, the Pi card) legitimately carries
            // neither file. Honest, not a fault.
            serial_println!(
                ":: SELFHOST: no SRC.TGZ/SRC.SHA on {} — SOURCE-ALONG not packed on this medium ::",
                fs.source_name()
            );
            return;
        }
        (Ok(_), Err(_)) => {
            serial_println!(
                ":: SELFHOST: SRC.TGZ present but SRC.SHA is MISSING — an unverifiable source payload -> FAIL ::"
            );
            return;
        }
        (Err(_), Ok(_)) => {
            serial_println!(
                ":: SELFHOST: SRC.SHA present but SRC.TGZ is MISSING — a stamp with nothing to stamp -> FAIL ::"
            );
            return;
        }
    };

    // --- the stamp ---
    let mut raw = Vec::new();
    if let Err(e) = fs.read_file(&shafile, &mut raw, SHA_FILE_MAX) {
        serial_println!(":: SELFHOST: SRC.SHA unreadable ({}) -> FAIL ::", fat_reason(e));
        return;
    }
    let text = match core::str::from_utf8(&raw) {
        Ok(t) => t,
        Err(_) => {
            serial_println!(":: SELFHOST: SRC.SHA is not UTF-8 -> FAIL ::");
            return;
        }
    };
    let stamp = match parse_stamp(text) {
        Some(s) => s,
        None => {
            serial_println!(":: SELFHOST: SRC.SHA carries no 64-hex sha256 on line 1 -> FAIL ::");
            return;
        }
    };

    // --- one pass: read + hash + inflate + walk ---
    let mut src = FatSource::new(&fs, &tgz);
    let mut walk = tar::TarWalk::new();
    let gz = inflate::gunzip(&mut src, &mut walk);

    // Whatever happened above, finish reading so the digest covers the whole file.
    src.drain();
    let got_sha = src.sha.finalize();
    let sha_ok = got_sha == stamp.sha;

    let gz = match gz {
        Ok(g) => g,
        Err(e) => {
            // Name the tar error if that is what aborted the inflate — "sink rejected" alone would
            // hide which of the two decoders actually disagreed with the payload.
            let detail = match (e, walk.error()) {
                (inflate::InflateError::SinkRejected, Some(te)) => tar::tar_reason(te),
                _ => inflate::inflate_reason(e),
            };
            serial_println!(
                ":: SELFHOST: SRC.TGZ walk aborted ({}) after files={} sha_match={} -> FAIL ::",
                detail,
                walk.census().files,
                sha_ok as u8
            );
            return;
        }
    };

    let (census, first_name) = match walk.finish() {
        Ok(v) => v,
        Err(e) => {
            serial_println!(
                ":: SELFHOST: SRC.TGZ is not a complete tar archive ({}) -> FAIL ::",
                tar::tar_reason(e)
            );
            return;
        }
    };

    let describe = if stamp.describe.is_empty() { "?" } else { stamp.describe.as_str() };
    let commit7 = stamp.commit.get(..7).unwrap_or("?");
    serial_println!(
        ":: SELFHOST: SRC.TGZ on {} tgz={} raw={} crc=0x{:08x} commit={} describe={} first={} ::",
        fs.source_name(),
        gz.compressed,
        gz.uncompressed,
        gz.crc,
        commit7,
        describe,
        first_name.as_deref().unwrap_or("-")
    );

    if !sha_ok {
        serial_println!(
            ":: SELFHOST: src sha256 MISMATCH want={} got={} -> FAIL ::",
            hex32(&stamp.sha),
            hex32(&got_sha)
        );
        return;
    }
    if census.target_members != 0 {
        // source_along.md: `tar -tzf SRC.TGZ | awk '/^target\//' | wc -l` must be 0.
        serial_println!(
            ":: SELFHOST: src payload carries {} member(s) under target/ — build output was archived -> FAIL ::",
            census.target_members
        );
        return;
    }
    if census.files == 0 {
        serial_println!(":: SELFHOST: src payload holds no regular files -> FAIL ::");
        return;
    }

    serial_println!(
        ":: SELFHOST: src verified sha={} files={} dirs={} bytes={} target=0 crc=ok -> PASS ::",
        hex32(&got_sha),
        census.files,
        census.dirs,
        census.bytes
    );
}
