// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-SELFUP — the OS writes its own boot files (tegra + `selfup` gated).
//
// The PERMANENT self-update core: locate a staged whole-ESP payload on the boot volume, verify it
// (SHA-256, whole-payload and per-file), stage every file beside the live set, flip the staged set
// live, consume the payload, and hand off to the warm-reboot verb. The byte SOURCE is deliberately
// scaffolding-tolerant — today the payload arrives as `UPDATE.PAK` + `UPDATE.SHA` already present in
// the boot volume's root (staged by the bench card loop / `./arroyo selfup-pak`); when a network
// transport lands, it delivers the same two artifacts to the same staging names and every line below
// this paragraph is unchanged. That is the transport seam, and it is the ONLY scaffolding-shaped
// thing in this file.
//
// ## The matched-pair rule (BOOTABI, binding)
//
// The loader and the kernel are a MATCHED PAIR — an update writes the WHOLE ESP or nothing, never a
// fresh kernel beside an old bootloader. Enforced twice:
//   * at parse (S2): a payload that does not carry BOTH `EFI/BOOT/BOOTAA64.EFI` and `KERNEL.ELF` is
//     refused before the first write — there is no file-level update verb to misuse;
//   * at flip (S4): every non-pair file flips first, then the loader, then the kernel LAST — so at
//     no instant does a fresh kernel sit beside an old loader (the rule's named forbidden state).
//
// ## State machine (each stage witnessed on serial as `[orinselfup] S<n>`)
//
//   S0 SCAN    mount the boot volume; look for UPDATE.PAK + UPDATE.SHA in the root. Absent => normal
//              boot (the common case; one witness line, nothing else).
//   S1 VERIFY  stream SHA-256 over the whole UPDATE.PAK; compare against UPDATE.SHA. This is the
//              transport-integrity gate: it catches a truncated or corrupted delivery, NOT a hostile
//              one (payload signing is future work — see the design note §7).
//   S2 PARSE   decode the UPK1 header; refuse (before any write) on: bad magic, entry count/path
//              bounds, non-8.3 path components, duplicate or reserved paths, a total size that does
//              not EXACTLY equal header + payload bytes (a short read refuses here), missing pair.
//   S3 WRITE   per file: resolve/create the parent directory, write the bytes to a staged temp name
//              (`UPD<i>.TMP`) beside the live file, hashing as they stream; then RE-READ the staged
//              file off the volume and hash again. Either mismatch aborts the whole update with the
//              live set untouched and every staged temp deleted.
//   S4 FLIP    per file: delete the live entry, rename the staged temp onto the live name — non-pair
//              files first, then BOOTAA64.EFI, then KERNEL.ELF (see above). The flip window is two
//              directory-entry RMWs per file; the pair's own window is witnessed OPEN/CLOSED.
//   S5 CLEAN   delete UPDATE.PAK + UPDATE.SHA so the next boot does not re-apply the same payload.
//   S6 REBOOT  call the warm-reboot hook (the exec-reboot arc's verb; a witnessed no-op until it
//              lands — see `reboot_hook`).
//
// ## Failure modes (the design note's §6, enforced here)
//
//   * power loss in S3: the live boot set is untouched; stale `UPD*.TMP` files are swept by the next
//     armed run before re-staging.
//   * power loss in S4: the residual window — a partially flipped set. Recovery is boot-media
//     rebuild; closing the window needs loader-side A/B, which is out of scope for this rung and
//     recorded as future work, NOT silently accepted: the window is minimized (bytes are already on
//     the volume; only directory-entry RMWs remain) and ordered so the named forbidden state
//     (fresh kernel + old loader) cannot occur.
//   * sha mismatch (S1, S3 stream, S3 read-back): refuse/abort with the live set intact.
//   * short read / truncation: the S2 exact-size equation refuses a truncated payload; a chain that
//     ends early during streaming is a short read and aborts in S3.
//
// The whole module is `#[cfg(all(feature = "tegra", feature = "selfup"))]` (declared in mod.rs) —
// with `selfup` OFF (the default), nothing here is compiled and the tegra image is byte-identical
// to baseline. Every serial token below carries the `[orinselfup]` prefix (12 bytes > the 8-byte
// LLVM immediate-encoding bound, so `strings` on the artifact proves reachability).

use crate::fs::fat::{self, FatError, FatFs};
use crate::hash::Sha256;
use alloc::string::String;
use alloc::vec::Vec;

/// Staged-payload names in the boot volume root — THE transport seam. Any future transport's job is
/// to make these two files exist; nothing else in this module is transport-aware.
const PAK_NAME: &str = "UPDATE.PAK";
const SHA_NAME: &str = "UPDATE.SHA";

/// UPK1 container magic (8 bytes exactly — the header decoder checks all 8).
const PAK_MAGIC: &[u8; 8] = b"UNAOSUP1";
/// Bounds: a whole-ESP payload is ~10 files today; 64 leaves headroom without inviting abuse.
const MAX_ENTRIES: usize = 64;
const MAX_PATH: usize = 128;
const MAX_HEADER: usize = 64 * 1024;
/// Streaming chunk for hash/write/read-back passes (heap-allocated per pass, freed after).
const CHUNK: usize = 32 * 1024;

/// The matched pair (BOOTABI): both MUST be present in every payload, and they flip LAST, in this
/// order (loader, then kernel — see the module header for why the kernel is last).
const PAIR_LOADER: &str = "EFI/BOOT/BOOTAA64.EFI";
const PAIR_KERNEL: &str = "KERNEL.ELF";

/// One decoded UPK1 entry: destination path, size, expected content SHA-256, and the byte offset of
/// its payload within UPDATE.PAK (computed at parse from the entry order — data is packed in order).
struct PakEntry {
    path: String,
    size: u32,
    sha: [u8; 32],
    data_off: u32,
}

/// A staged temp awaiting flip (or abort cleanup): the parent dir's first cluster (0 = root) and the
/// temp leaf name it was written under.
struct Staged {
    parent: u32,
    temp: String,
}

/// The service entry point — called from `tegra_early_stop` after the JB2b pump window (the boot
/// volume is a readable block device there) and before the JM6 EL2->EL1 drop. Every failure is a
/// witnessed refusal, never a panic: an armed image on a volume with no payload boots normally.
pub fn selfup_service() {
    serial_println!(
        ":: [orinselfup] S0 scan — service armed (UNAOS_SELFUP); probing the boot volume for a staged whole-ESP payload ::"
    );
    let fs = match fat::mount() {
        Ok(f) => f,
        Err(e) => {
            serial_println!(
                ":: [orinselfup] S0 scan — boot volume not mountable ({}) — normal boot ::",
                fat::fat_reason(e)
            );
            return;
        }
    };
    if let Some(reason) = fs.write_veto() {
        serial_println!(
            ":: [orinselfup] S0 scan — REFUSED: the mounted boot volume vetoes writes ({}) ::",
            reason
        );
        return;
    }
    let pak = match fs.find_in_root(PAK_NAME) {
        Ok(de) if !de.is_dir => de,
        Ok(_) => {
            serial_println!(
                ":: [orinselfup] S0 scan — REFUSED: UPDATE.PAK is a directory, not a payload ::"
            );
            return;
        }
        Err(_) => {
            serial_println!(
                ":: [orinselfup] S0 scan — no staged payload (UPDATE.PAK absent) — normal boot ::"
            );
            return;
        }
    };
    serial_println!(
        ":: [orinselfup] S0 scan — payload staged: UPDATE.PAK {} bytes on {} ::",
        pak.size,
        fs.source_name()
    );
    match apply_update(&fs, &pak) {
        Ok(n) => {
            serial_println!(
                ":: [orinselfup] UPDATE APPLIED — whole-ESP set of {} files verified, flipped live, payload consumed ::",
                n
            );
            reboot_hook();
        }
        Err(msg) => {
            serial_println!(":: [orinselfup] UPDATE REFUSED — {} (live boot set untouched) ::", msg);
        }
    }
}

/// S1..S5. Returns the number of files applied, or a refusal reason. The live boot set is mutated
/// ONLY inside `flip_live` (S4); every earlier stage touches staged temp names alone.
fn apply_update(fs: &FatFs, pak: &fat::DirEntry) -> Result<usize, String> {
    // ── S1 VERIFY: whole-payload sha256 against UPDATE.SHA. ─────────────────────────────────────
    let want = read_expected_sha(fs)?;
    let got = sha_of_extent(fs, pak.first_cluster(), pak.size, 0, pak.size)
        .map_err(|m| alloc::format!("S1 verify — payload stream failed ({})", m))?;
    if got != want {
        return Err(alloc::format!(
            "S1 verify — payload sha256 MISMATCH (UPDATE.SHA says {}…, UPDATE.PAK hashes {}…) — refusing before any write",
            hex8(&want),
            hex8(&got)
        ));
    }
    serial_println!(
        ":: [orinselfup] S1 verify — payload sha256 MATCH ({}…) over {} bytes ::",
        hex8(&got),
        pak.size
    );

    // ── S2 PARSE: decode + refuse everything refusable BEFORE the first write. ──────────────────
    let entries = parse_header(fs, pak)?;
    serial_println!(
        ":: [orinselfup] S2 parse — {} entries decoded; matched pair present (EFI/BOOT/BOOTAA64.EFI + KERNEL.ELF); sizes account for the payload exactly ::",
        entries.len()
    );

    // ── S3 WRITE: stage every file beside the live set, verifying twice. ────────────────────────
    let mut staged: Vec<Staged> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match stage_one(fs, pak, e, i) {
            Ok(s) => {
                serial_println!(
                    ":: [orinselfup] S3 write — {} ({} bytes) staged as {} + read-back sha verified ::",
                    e.path,
                    e.size,
                    s.temp
                );
                staged.push(s);
            }
            Err(msg) => {
                // Abort: delete every temp staged so far (best effort — a leftover is swept by the
                // next armed run), live set untouched.
                for s in &staged {
                    let _ = delete_if_present(fs, s.parent, &s.temp);
                }
                return Err(alloc::format!("S3 write — {} — staging aborted, temps swept", msg));
            }
        }
    }

    // ── S4 FLIP: non-pair first, then loader, then kernel LAST (matched-pair ordering). ─────────
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| pair_rank(&entries[i].path));
    let pair_start = order
        .iter()
        .position(|&i| pair_rank(&entries[i].path) > 0)
        .unwrap_or(order.len());
    for (k, &i) in order.iter().enumerate() {
        if k == pair_start {
            serial_println!(
                ":: [orinselfup] S4 flip — matched-pair flip window OPEN (loader, then kernel) ::"
            );
        }
        let e = &entries[i];
        let s = &staged[i];
        flip_live(fs, s, e).map_err(|m| {
            alloc::format!(
                "S4 flip — {} failed ({}) — VOLUME MAY HOLD A PARTIAL FLIP; rebuild boot media before trusting the next boot",
                e.path,
                m
            )
        })?;
        serial_println!(":: [orinselfup] S4 flip — {} is live ::", e.path);
    }
    serial_println!(":: [orinselfup] S4 flip — matched-pair flip window CLOSED ::");

    // ── S5 CLEAN: consume the payload so the next boot does not re-apply it. ────────────────────
    let _ = delete_if_present(fs, 0, PAK_NAME);
    let _ = delete_if_present(fs, 0, SHA_NAME);
    serial_println!(":: [orinselfup] S5 clean — staged payload consumed (UPDATE.PAK + UPDATE.SHA deleted) ::");
    Ok(entries.len())
}

/// S6 — THE REBOOT SEAM. The warm-reboot verb is the exec-reboot arc's deliverable, built in
/// parallel with this one; this hook is its single agreed call site. When that verb lands, this
/// body becomes one call into it and the witness line below is replaced by the verb's own.
fn reboot_hook() {
    serial_println!(
        ":: [orinselfup] S6 reboot — warm-reboot verb NOT WIRED yet (exec-reboot arc owns it); continuing this boot on the in-RAM kernel — the updated ESP takes effect at the next power cycle ::"
    );
}

/// Read UPDATE.SHA and decode its leading 64 hex chars (the `sha256sum` line shape the SOURCE-ALONG
/// artifacts already use — hash first, anything after it ignored).
fn read_expected_sha(fs: &FatFs) -> Result<[u8; 32], String> {
    let de = match fs.find_in_root(SHA_NAME) {
        Ok(d) if !d.is_dir => d,
        _ => {
            return Err(String::from(
                "S1 verify — UPDATE.PAK is staged but UPDATE.SHA is absent — refusing an unverifiable payload",
            ))
        }
    };
    let mut buf: Vec<u8> = Vec::new();
    fs.read_file(&de, &mut buf, 4096)
        .map_err(|e| alloc::format!("S1 verify — UPDATE.SHA unreadable ({})", fat::fat_reason(e)))?;
    if buf.len() < 64 {
        return Err(String::from("S1 verify — UPDATE.SHA too short for a sha256 hex digest"));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_val(buf[2 * i]);
        let lo = hex_val(buf[2 * i + 1]);
        match (hi, lo) {
            (Some(h), Some(l)) => out[i] = (h << 4) | l,
            _ => return Err(String::from("S1 verify — UPDATE.SHA is not valid hex")),
        }
    }
    Ok(out)
}

/// SHA-256 over `[start, start+len)` of a file's chain, streamed in CHUNK pieces. A short read
/// (chain ends before the span) is an error — never a silently-shorter hash.
fn sha_of_extent(
    fs: &FatFs,
    first_cluster: u32,
    file_size: u32,
    start: u32,
    len: u32,
) -> Result<[u8; 32], String> {
    let mut h = Sha256::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut off: u32 = 0;
    while off < len {
        let take = core::cmp::min(CHUNK as u32, len - off);
        fs.read_at(first_cluster, file_size, start + off, &mut buf, take as usize)
            .map_err(|e| alloc::format!("read failed at +{} ({})", start + off, fat::fat_reason(e)))?;
        if buf.len() != take as usize {
            return Err(alloc::format!(
                "SHORT READ at +{} (wanted {}, got {})",
                start + off,
                take,
                buf.len()
            ));
        }
        h.update(&buf);
        off += take;
    }
    Ok(h.finalize())
}

/// S2 — decode the UPK1 header and refuse every malformation before the first write.
fn parse_header(fs: &FatFs, pak: &fat::DirEntry) -> Result<Vec<PakEntry>, String> {
    let refuse = |m: String| Err(alloc::format!("S2 parse — {} — refusing before any write", m));
    if (pak.size as usize) < 16 {
        return refuse(String::from("payload smaller than the fixed header"));
    }
    let mut head: Vec<u8> = Vec::new();
    fs.read_at(pak.first_cluster(), pak.size, 0, &mut head, 16)
        .map_err(|e| alloc::format!("S2 parse — header unreadable ({})", fat::fat_reason(e)))?;
    if head.len() != 16 {
        return refuse(String::from("SHORT READ on the fixed header"));
    }
    if &head[0..8] != PAK_MAGIC {
        return refuse(String::from("bad magic (not an UNAOSUP1 payload)"));
    }
    let count = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) as usize;
    let header_len = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
    if count == 0 || count > MAX_ENTRIES {
        return refuse(alloc::format!("entry count {} outside 1..={}", count, MAX_ENTRIES));
    }
    if header_len < 16 || header_len > MAX_HEADER || header_len > pak.size as usize {
        return refuse(alloc::format!("header length {} out of bounds", header_len));
    }
    fs.read_at(pak.first_cluster(), pak.size, 0, &mut head, header_len)
        .map_err(|e| alloc::format!("S2 parse — header unreadable ({})", fat::fat_reason(e)))?;
    if head.len() != header_len {
        return refuse(String::from("SHORT READ on the entry table"));
    }

    let mut entries: Vec<PakEntry> = Vec::new();
    let mut pos: usize = 16;
    let mut data_off: u64 = header_len as u64;
    for i in 0..count {
        if pos + 2 > header_len {
            return refuse(alloc::format!("entry {} truncated (path length)", i));
        }
        let plen = u16::from_le_bytes([head[pos], head[pos + 1]]) as usize;
        pos += 2;
        if plen == 0 || plen > MAX_PATH {
            return refuse(alloc::format!("entry {} path length {} outside 1..={}", i, plen, MAX_PATH));
        }
        if pos + plen + 4 + 32 > header_len {
            return refuse(alloc::format!("entry {} truncated (path/size/sha)", i));
        }
        let path = match core::str::from_utf8(&head[pos..pos + plen]) {
            Ok(s) => String::from(s),
            Err(_) => return refuse(alloc::format!("entry {} path is not UTF-8", i)),
        };
        pos += plen;
        let size = u32::from_le_bytes([head[pos], head[pos + 1], head[pos + 2], head[pos + 3]]);
        pos += 4;
        let mut sha = [0u8; 32];
        sha.copy_from_slice(&head[pos..pos + 32]);
        pos += 32;

        if let Some(why) = path_defect(&path) {
            return refuse(alloc::format!("entry {} path {:?} refused ({})", i, path, why));
        }
        if entries.iter().any(|e| e.path.eq_ignore_ascii_case(&path)) {
            return refuse(alloc::format!("duplicate path {:?}", path));
        }
        if data_off + size as u64 > pak.size as u64 {
            return refuse(alloc::format!("entry {} data runs past the payload end", i));
        }
        entries.push(PakEntry { path, size, sha, data_off: data_off as u32 });
        data_off += size as u64;
    }
    if pos != header_len {
        return refuse(String::from("entry table does not fill the declared header length"));
    }
    // THE EXACT-CONSUMPTION EQUATION: header + every declared byte == the file. A truncated or
    // padded delivery fails here even after the S1 whole-file sha passed (belt and braces — S1
    // guards the transport, this guards the container's own arithmetic).
    if data_off != pak.size as u64 {
        return refuse(alloc::format!(
            "sizes account for {} bytes but the payload is {} — truncated or padded container",
            data_off,
            pak.size
        ));
    }
    // THE MATCHED-PAIR GATE (BOOTABI): whole ESP or nothing.
    let has_loader = entries.iter().any(|e| e.path.eq_ignore_ascii_case(PAIR_LOADER));
    let has_kernel = entries.iter().any(|e| e.path.eq_ignore_ascii_case(PAIR_KERNEL));
    if !has_loader || !has_kernel {
        return refuse(alloc::format!(
            "matched-pair rule: payload must carry the WHOLE ESP incl. BOTH {} and {} (loader present={}, kernel present={})",
            PAIR_LOADER,
            PAIR_KERNEL,
            has_loader,
            has_kernel
        ));
    }
    Ok(entries)
}

/// Per-path refusals: shape, 8.3 representability, reserved names. `None` = acceptable.
fn path_defect(path: &str) -> Option<&'static str> {
    if path.starts_with('/') || path.ends_with('/') {
        return Some("leading/trailing slash");
    }
    let mut depth = 0usize;
    for comp in path.split('/') {
        depth += 1;
        if depth > 8 {
            return Some("deeper than 8 components");
        }
        if comp.is_empty() || comp == "." || comp == ".." {
            return Some("empty or dot component");
        }
        if !leaf_83_ok(comp) {
            return Some("component not 8.3-representable");
        }
    }
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if leaf.eq_ignore_ascii_case(PAK_NAME) || leaf.eq_ignore_ascii_case(SHA_NAME) {
        return Some("collides with the staging names");
    }
    let upper_leaf_is_temp = {
        let b = leaf.as_bytes();
        b.len() >= 7
            && (b[0] | 0x20) == b'u'
            && (b[1] | 0x20) == b'p'
            && (b[2] | 0x20) == b'd'
            && leaf[leaf.len() - 4..].eq_ignore_ascii_case(".TMP")
    };
    if upper_leaf_is_temp {
        return Some("collides with the staged-temp namespace (UPD*.TMP)");
    }
    None
}

/// Conservative 8.3 check, run at parse so a bad name refuses BEFORE any write (the FAT layer's own
/// `format_83` is the authority at write time; this predicate only accepts a subset of what it does).
fn leaf_83_ok(leaf: &str) -> bool {
    let (base, ext) = match leaf.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (leaf, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || base.contains('.') {
        return false;
    }
    base.bytes()
        .chain(ext.bytes())
        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'~')
}

/// Flip ordering: 0 = ordinary file, 1 = the loader, 2 = the kernel (LAST).
fn pair_rank(path: &str) -> u8 {
    if path.eq_ignore_ascii_case(PAIR_KERNEL) {
        2
    } else if path.eq_ignore_ascii_case(PAIR_LOADER) {
        1
    } else {
        0
    }
}

/// Walk (creating as needed) the directory components of `path`; return the parent dir's first
/// cluster (0 = root) and the leaf name.
fn resolve_parent<'a>(fs: &FatFs, path: &'a str) -> Result<(u32, &'a str), String> {
    let mut parent: u32 = 0;
    let mut comps: Vec<&str> = path.split('/').collect();
    let leaf = comps.pop().expect("path_defect guarantees a leaf");
    for comp in comps {
        match fs.locate_in_dir(parent, comp) {
            Ok((de, _, _)) if de.is_dir => parent = de.first_cluster(),
            Ok(_) => {
                return Err(alloc::format!(
                    "path component {:?} exists as a FILE where a directory is needed",
                    comp
                ))
            }
            Err(FatError::NotFound) => {
                fs.create_dir(parent, comp)
                    .map_err(|e| alloc::format!("mkdir {:?} failed ({})", comp, fat::fat_reason(e)))?;
                // Re-locate rather than trusting the create's returned snapshot for the chain head.
                let (de, _, _) = fs
                    .locate_in_dir(parent, comp)
                    .map_err(|e| alloc::format!("mkdir {:?} vanished ({})", comp, fat::fat_reason(e)))?;
                parent = de.first_cluster();
            }
            Err(e) => return Err(alloc::format!("dir walk failed at {:?} ({})", comp, fat::fat_reason(e))),
        }
    }
    Ok((parent, leaf))
}

/// S3 for one entry: sweep a stale temp, create the temp, stream payload bytes into it (hashing as
/// they pass), then re-read the staged file off the volume and hash again. Both hashes must equal
/// the entry's declared sha.
fn stage_one(fs: &FatFs, pak: &fat::DirEntry, e: &PakEntry, idx: usize) -> Result<Staged, String> {
    let (parent, leaf) = resolve_parent(fs, &e.path)?;
    // The final name may not exist as a directory — refuse rather than flip onto it later.
    if let Ok((de, _, _)) = fs.locate_in_dir(parent, leaf) {
        if de.is_dir {
            return Err(alloc::format!("{} exists as a DIRECTORY on the live volume", e.path));
        }
    }
    let temp = alloc::format!("UPD{}.TMP", idx);
    // Sweep a stale temp from an earlier power-lost run (S3's own failure mode).
    let _ = delete_if_present(fs, parent, &temp);
    let (_de, dlba, doff) = fs
        .create_in_dir(parent, &temp, 0x20)
        .map_err(|er| alloc::format!("create {} failed ({})", temp, fat::fat_reason(er)))?;

    // Stream: pak[data_off .. data_off+size) -> temp file, hashing in flight.
    let mut h = Sha256::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut first_cluster: u32 = 0;
    let mut cur_size: u32 = 0;
    let mut off: u32 = 0;
    while off < e.size {
        let take = core::cmp::min(CHUNK as u32, e.size - off);
        fs.read_at(pak.first_cluster(), pak.size, e.data_off + off, &mut buf, take as usize)
            .map_err(|er| alloc::format!("payload read at +{} failed ({})", off, fat::fat_reason(er)))?;
        if buf.len() != take as usize {
            return Err(alloc::format!(
                "SHORT READ in payload at +{} (wanted {}, got {})",
                off,
                take,
                buf.len()
            ));
        }
        h.update(&buf);
        let (wrote, nsize, nfirst) = fs
            .write_grow(first_cluster, cur_size, dlba, doff, off, &buf)
            .map_err(|er| alloc::format!("write {} at +{} failed ({})", temp, off, fat::fat_reason(er)))?;
        if wrote != buf.len() {
            return Err(alloc::format!("SHORT WRITE on {} at +{}", temp, off));
        }
        cur_size = nsize;
        first_cluster = nfirst;
        off += take;
    }
    if h.finalize() != e.sha {
        return Err(alloc::format!("{} payload sha MISMATCH while streaming", e.path));
    }
    // Read-back verify: hash what the VOLUME now holds, not what we think we wrote.
    let back = sha_of_extent(fs, first_cluster, cur_size, 0, e.size)
        .map_err(|m| alloc::format!("read-back of {} failed ({})", temp, m))?;
    if back != e.sha {
        return Err(alloc::format!("{} read-back sha MISMATCH (media wrote different bytes)", e.path));
    }
    Ok(Staged { parent, temp: String::from(temp.as_str()) })
}

/// S4 for one entry: delete the live name if present, then rename the staged temp onto it.
fn flip_live(fs: &FatFs, s: &Staged, e: &PakEntry) -> Result<(), String> {
    let leaf = e.path.rsplit('/').next().unwrap_or(&e.path);
    delete_if_present(fs, s.parent, leaf)?;
    fs.rename_entry(s.parent, &s.temp, leaf)
        .map_err(|er| alloc::format!("rename {} -> {} failed ({})", s.temp, leaf, fat::fat_reason(er)))?;
    Ok(())
}

/// Delete `leaf` in the directory at `parent` if it exists (NotFound is success).
fn delete_if_present(fs: &FatFs, parent: u32, leaf: &str) -> Result<(), String> {
    match fs.locate_in_dir(parent, leaf) {
        Ok((de, lba, off)) => {
            if de.is_dir {
                return Err(alloc::format!("{} is a directory — not deleting a tree", leaf));
            }
            fs.delete_located(lba, off, de.first_cluster())
                .map_err(|e| alloc::format!("delete {} failed ({})", leaf, fat::fat_reason(e)))?;
            Ok(())
        }
        Err(FatError::NotFound) => Ok(()),
        Err(e) => Err(alloc::format!("locate {} failed ({})", leaf, fat::fat_reason(e))),
    }
}

/// First 8 hex chars of a digest — enough to compare on serial without drowning the log.
fn hex8(d: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in &d[..4] {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b));
    }
    s
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
