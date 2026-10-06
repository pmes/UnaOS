//! CHARTER: Kernel — fs-core
//!
//! FILETYPE (B307, audit B293) — every file has a TYPE, and the type is an ATTRIBUTE on the file.
//!
//! The type attribute is `una:type`, a MIME string (`BEOS:TYPE`'s role). [`type_of`] answers a path's
//! type and says WHICH SOURCE decided it, in this order:
//!
//! 1. **Attribute** — `una:type` on the object (UnaFS volumes; ATTRSURF's `MountTable::get_attr`).
//! 2. **Sniffed** — the first [`SNIFF_LEN`] bytes: PNG, `RIFF....WAVE`, ELF (UnaOS vs Linux, below),
//!    gzip, `ustar`, then a UTF-8/ASCII text heuristic. Content beats the name.
//! 3. **Extension** — ONE static table ([`EXT_TABLE`]); the only place a name decides anything.
//! 4. **Unknown** — `application/octet-stream`.
//!
//! On a FAT volume every attribute call answers `-ENOTSUP`, so leg 1 is skipped there and the answer
//! comes from the sniff or the table — and the source printed says so. Nothing here keeps a store of
//! its own: the attribute IS the type (R79 — the seam is the volume, not a second table on disk).
//!
//! THE ELF SPLIT is the one the two loaders already make. A UnaOS image (`arch/*/elf.rs`) is linked at
//! vaddr 0 and biased into a slot window, so its lowest `PT_LOAD` vaddr is below the 64 KiB floor; a
//! static Linux image (`arch/x86_64/linuxabi/elf.rs`, `IMAGE_FLOOR`) is mapped at its own fixed vaddrs,
//! at or above it. The floor is the test.
//!
//! Design: `docs/dev/evidence/rmbp-1004/FILETYPE.md`.
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};

/// The type attribute's key.
pub const TYPE_KEY: &str = "una:type";

pub const TEXT_PLAIN: &str = "text/plain";
pub const IMAGE_PNG: &str = "image/png";
pub const AUDIO_WAV: &str = "audio/wav";
pub const UNAOS_ELF: &str = "application/x-unaos-elf";
pub const UNAOS_BIN: &str = "application/x-unaos-bin";
pub const LINUX_ELF: &str = "application/x-linux-elf";
pub const DIRECTORY: &str = "inode/directory";
pub const GZIP: &str = "application/gzip";
pub const TAR: &str = "application/x-tar";
pub const OCTET: &str = "application/octet-stream";
/// QUARRY2 (B336): the two text types FILETYPE owed, and the animated-image type PIXELCORE decodes.
pub const TEXT_MARKDOWN: &str = "text/markdown";
pub const APP_JSON: &str = "application/json";
pub const IMAGE_GIF: &str = "image/gif";
/// OPENERS (B379): the types the decoders already in the tree read — named by the cores that own the formats
/// (`pixel_core::mime_of`, `audio_core::mime_of`, `demux_core::mime::mime_of`), so these strings are theirs.
pub const IMAGE_JPEG: &str = "image/jpeg";
pub const IMAGE_BMP: &str = "image/bmp";
pub const IMAGE_WEBP: &str = "image/webp";
pub const IMAGE_QOI: &str = "image/qoi";
/// SMALLFIX2 (B391, R94): pixel_core's SVG type (`pixel_core::mime_of` under the `svg` feature).
pub const IMAGE_SVG: &str = "image/svg+xml";
pub const AUDIO_FLAC: &str = "audio/flac";
pub const AUDIO_OGG: &str = "audio/ogg";
pub const AUDIO_MPEG: &str = "audio/mpeg";
pub const AUDIO_AAC: &str = "audio/aac";
pub const AUDIO_AIFF: &str = "audio/aiff";
pub const AUDIO_MP4: &str = demux_core::mime::AUDIO_MP4;
pub const VIDEO_MP4: &str = demux_core::mime::VIDEO_MP4;
pub const VIDEO_WEBM: &str = demux_core::mime::VIDEO_WEBM;
pub const VIDEO_MATROSKA: &str = demux_core::mime::VIDEO_MATROSKA;

/// How many leading bytes the sniff reads (`ustar` sits at 257..262, so a tar needs 263).
pub const SNIFF_LEN: usize = 512;

/// The lowest `PT_LOAD` vaddr a static Linux image may carry — `linuxabi::elf::IMAGE_FLOOR`.
const LINUX_VADDR_FLOOR: u64 = 0x1_0000;

/// Which leg of [`type_of`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Attribute,
    Sniffed,
    Extension,
    Unknown,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Attribute => "attribute",
            Source::Sniffed => "sniffed",
            Source::Extension => "extension",
            Source::Unknown => "unknown",
        }
    }
}

/// THE extension table — every extension the old Quarry if-chains knew, plus the ones they disagreed
/// on (`.sha/.cfg/.ini`) and the shapes this tree ships (`.lnx`, `.tgz`, `.tar`, `.toml`, `.json`).
/// Matched case-insensitively against the text after the LAST dot (a leading dot is a name, not an
/// extension). This is the ONLY name-based routing in the kernel's open path.
pub const EXT_TABLE: &[(&str, &str)] = &[
    ("elf", UNAOS_ELF),
    ("bin", UNAOS_BIN),
    ("lnx", LINUX_ELF),
    ("png", IMAGE_PNG),
    ("wav", AUDIO_WAV),
    ("txt", TEXT_PLAIN),
    ("md", TEXT_MARKDOWN),
    ("markdown", TEXT_MARKDOWN),
    ("log", TEXT_PLAIN),
    ("spec", TEXT_PLAIN),
    ("sha", TEXT_PLAIN),
    ("cfg", TEXT_PLAIN),
    ("ini", TEXT_PLAIN),
    ("toml", TEXT_PLAIN),
    ("json", APP_JSON),
    ("gif", IMAGE_GIF),
    ("tgz", GZIP),
    ("gz", GZIP),
    ("tar", TAR),
    // OPENERS (B379): the image, audio and container extensions — used only when the bytes say nothing
    // (an empty file, a head no core recognises).
    ("jpg", IMAGE_JPEG),
    ("jpeg", IMAGE_JPEG),
    ("bmp", IMAGE_BMP),
    ("webp", IMAGE_WEBP),
    ("qoi", IMAGE_QOI),
    ("svg", IMAGE_SVG), // SMALLFIX2 (B391): the bytes speak first — without the `svg` feature an .svg sniffs as its text
    ("flac", AUDIO_FLAC),
    ("ogg", AUDIO_OGG),
    ("oga", AUDIO_OGG),
    ("opus", AUDIO_OGG),
    ("mp3", AUDIO_MPEG),
    ("aac", AUDIO_AAC),
    ("m4a", AUDIO_MP4),
    ("m4b", AUDIO_MP4),
    ("aif", AUDIO_AIFF),
    ("aiff", AUDIO_AIFF),
    ("aifc", AUDIO_AIFF),
    ("mp4", VIDEO_MP4),
    ("m4v", VIDEO_MP4),
    ("webm", VIDEO_WEBM),
    ("mkv", VIDEO_MATROSKA),
];

/// The extension of `name` (without the dot), or `None`. Pure.
pub fn ext_of(name: &str) -> Option<&str> {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    match leaf.rfind('.') {
        Some(i) if i > 0 && i + 1 < leaf.len() => Some(&leaf[i + 1..]),
        _ => None,
    }
}

/// The table's answer for `name`, or `None`. Pure.
pub fn by_extension(name: &str) -> Option<&'static str> {
    let e = ext_of(name)?;
    EXT_TABLE.iter().find(|(x, _)| e.eq_ignore_ascii_case(x)).map(|(_, m)| *m)
}

fn u16le(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64)
}
fn u64le(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// UnaOS or Linux, for a buffer that starts with `\x7fELF`. The lowest `PT_LOAD` vaddr among the
/// program headers that fit in `b`; when none fit, `e_entry` stands in. Pure.
fn elf_flavour(b: &[u8]) -> &'static str {
    let phoff = u64le(b, 32).unwrap_or(0) as usize;
    let phnum = u16le(b, 56).unwrap_or(0) as usize;
    let phent = u16le(b, 54).unwrap_or(0) as usize;
    let mut min: Option<u64> = None;
    if phent == 56 {
        for i in 0..phnum.min(16) {
            let ph = phoff.saturating_add(i * 56);
            let Some(ty) = b.get(ph..ph + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) else { break };
            if ty != 1 {
                continue;
            }
            if let Some(va) = u64le(b, ph + 16) {
                min = Some(min.map_or(va, |m: u64| m.min(va)));
            }
        }
    }
    let va = min.or_else(|| u64le(b, 24)).unwrap_or(0);
    if va >= LINUX_VADDR_FLOOR { LINUX_ELF } else { UNAOS_ELF }
}

/// Is `b` (a PREFIX of a file, possibly cut mid-character) text: no NUL, valid UTF-8 up to a cut
/// tail of at most 3 bytes, and almost no control bytes besides TAB/LF/CR/FF/ESC. Pure.
pub fn looks_text(b: &[u8]) -> bool {
    if b.is_empty() || b.contains(&0) {
        return false;
    }
    match core::str::from_utf8(b) {
        Ok(_) => {}
        Err(e) => {
            if e.error_len().is_some() || b.len() - e.valid_up_to() > 3 {
                return false;
            }
        }
    }
    let ctl = b.iter().filter(|&&c| c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    ctl * 32 <= b.len()
}

/// The sniff over a file's leading bytes. `None` = nothing recognised (the table decides). Pure.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    sniff_strength(b).map(|(m, _)| m)
}

/// OPENERS (B379) — [`sniff`] with its STRENGTH: `true` for a magic a format defines, `false` for a shape a file
/// of another kind could have by accident (the Markdown/JSON heads, a bare MPEG-audio/ADTS/FLAC frame sync, an
/// ISO box type other than `ftyp`/`moov` at offset 4). [`type_of_in`] lets a name the table types otherwise
/// overrule a weak answer. Pure.
pub fn sniff_strength(b: &[u8]) -> Option<(&'static str, bool)> {
    if let Some(m) = sniff_core(b) {
        let weak = matches!(m, TEXT_MARKDOWN | APP_JSON);
        return Some((m, !weak));
    }
    // Weak, and only for bytes that are not text: an ISO-BMFF head that opens with `free`/`mdat`/…, a bare frame sync.
    if let Some(m) = demux_core::mime::mime_of(b) {
        return Some((m, false));
    }
    audio_core::mime_of(b).map(|(m, _)| (m, false))
}

/// The pre-OPENERS sniff with the cores' strong magics inserted before the text heuristic. Pure.
fn sniff_core(b: &[u8]) -> Option<&'static str> {
    if b.len() >= 8 && b[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Some(IMAGE_PNG);
    }
    if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        return Some(AUDIO_WAV);
    }
    if b.len() >= 4 && b[..4] == [0x7f, b'E', b'L', b'F'] {
        return Some(elf_flavour(b));
    }
    if b.len() >= 2 && b[0] == 0x1f && b[1] == 0x8b {
        return Some(GZIP);
    }
    if b.len() >= 262 && &b[257..262] == b"ustar" {
        return Some(TAR);
    }
    if b.len() >= 6 && (&b[..6] == b"GIF87a" || &b[..6] == b"GIF89a") {
        return Some(IMAGE_GIF);
    }
    // OPENERS (B379): the cores' own magics — images (JPEG, BMP by its DIB header, QOI, WebP), containers (an
    // `ftyp`/`moov` at offset 4, the EBML magic), audio (`fLaC` `OggS` `ID3` `FORM…AIFF`).
    if let Some(m) = pixel_core::mime_of(b) {
        return Some(m);
    }
    if (b.len() >= 8 && matches!(&b[4..8], b"ftyp" | b"moov")) || b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        if let Some(m) = demux_core::mime::mime_of(b) {
            return Some(m);
        }
    }
    if let Some((m, true)) = audio_core::mime_of(b) {
        return Some(m);
    }
    if looks_text(b) {
        if looks_json(b) {
            return Some(APP_JSON);
        }
        if looks_markdown(b) {
            return Some(TEXT_MARKDOWN);
        }
        return Some(TEXT_PLAIN);
    }
    None
}

/// QUARRY2 (B336) — the type the text editor stamps on a save: `text/markdown` / `application/json` when
/// the name says so (a `.md` saved from the editor stays Markdown), else `text/plain`. Pure.
pub fn saved_text_type(path: &str) -> &'static str {
    match by_extension(path) {
        Some(m @ (TEXT_MARKDOWN | APP_JSON)) => m,
        _ => TEXT_PLAIN,
    }
}

/// QUARRY2 (B336) — a Markdown head: an ATX heading `# ` (any level up to six `#`) on the first line,
/// or YAML front matter (`---` alone on the first line). Pure.
pub fn looks_markdown(b: &[u8]) -> bool {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(b);
    let line = b.split(|&c| c == b'\n').next().unwrap_or(b);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line == b"---" {
        return true;
    }
    let hashes = line.iter().take_while(|&&c| c == b'#').count();
    (1..=6).contains(&hashes) && line.get(hashes) == Some(&b' ')
}

/// QUARRY2 (B336) — a JSON head: after whitespace, `{` followed by `"` or `}`, or `[` followed by a
/// value's first byte (`"` `{` `[` `]` `-` a digit `t` `f` `n`). Valid FIRST TOKENS, not a parse: the
/// renderer validates the whole document and says so when it is not one. Pure.
pub fn looks_json(b: &[u8]) -> bool {
    let ws = |c: &u8| matches!(*c, b' ' | b'\t' | b'\r' | b'\n');
    let mut it = b.iter().skip_while(|c| ws(*c));
    let first = match it.next() {
        Some(&c) => c,
        None => return false,
    };
    let next = it.find(|c| !ws(*c)).copied();
    match (first, next) {
        (b'{', Some(b'"')) | (b'{', Some(b'}')) => true,
        (b'[', Some(c)) => matches!(c, b'"' | b'{' | b'[' | b']' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'),
        _ => false,
    }
}

/// [`type_of`] against a mount table the caller already holds (Quarry asks inside its model lock and
/// builds the table once per activation).
pub fn type_of_in(mt: &MountTable, path: &str) -> (String, Source) {
    let st = mt.stat(path);
    if let Ok(s) = &st {
        if matches!(s.kind, NodeKind::Dir) {
            return (String::from(DIRECTORY), Source::Sniffed);
        }
    }
    if let Ok(AttrValue::Str(m)) = mt.get_attr(path, TYPE_KEY, KERNEL_PRINCIPAL) {
        if !m.is_empty() {
            return (m, Source::Attribute);
        }
    }
    if let Ok(s) = &st {
        if s.size > 0 {
            let want = core::cmp::min(s.size, SNIFF_LEN as u64) as usize;
            if let Ok(head) = mt.read(path, 0, want) {
                if let Some((m, strong)) = sniff_strength(&head) {
                    // QUARRY2 (B336): the Markdown/JSON shapes are WEAK (a `# comment` config is not a
                    // document) — a name the table types otherwise keeps its table type. OPENERS (B379): so is
                    // every weak shape `sniff_strength` names (a bare frame sync, an ISO head without `ftyp`).
                    if let (false, Some(e)) = (strong, by_extension(path)) { if e != m { return (String::from(e), Source::Extension); } }
                    // OPENERS (B379): an ISO-BMFF file's `moov` may lie past the sniff window (after the `mdat`):
                    // walk the top-level boxes to it so its tracks decide audio/mp4 against video/mp4.
                    if matches!(m, AUDIO_MP4 | VIDEO_MP4) {
                        return (String::from(iso_walk(mt, path, s.size).unwrap_or(m)), Source::Sniffed);
                    }
                    return (String::from(m), Source::Sniffed);
                }
            }
        }
    }
    if let Some(m) = by_extension(path) {
        return (String::from(m), Source::Extension);
    }
    (String::from(OCTET), Source::Unknown)
}

/// The type of `path` and the leg that decided it.
pub fn type_of(path: &str) -> (String, Source) {
    type_of_in(&crate::shell::vfs_mount_table(), path)
}

/// Write `una:type = mime` on `path`. `Ok(true)` stamped, `Ok(false)` the volume takes no attributes
/// (FAT: the skip is printed once per call, never silent).
pub fn stamp_as_in(mt: &MountTable, path: &str, mime: &str) -> Result<bool, VfsError> {
    match mt.set_attr(path, TYPE_KEY, AttrValue::Str(String::from(mime)), KERNEL_PRINCIPAL) {
        Ok(()) => {
            serial_println!("[filetype] stamp path={} type={}", path, mime);
            Ok(true)
        }
        Err(VfsError::Unsupported) => {
            serial_println!("[filetype] stamp=skip reason=enotsup path={} type={}", path, mime);
            Ok(false)
        }
        Err(e) => {
            serial_println!("[filetype] stamp=fail path={} type={} ({})", path, mime, crate::fs::attrsys::refusal(&e));
            Err(e)
        }
    }
}

/// Stamp `path` with a type its WRITER knows (a screenshot is `image/png` whatever its name says).
pub fn stamp_as(path: &str, mime: &str) -> bool {
    matches!(stamp_as_in(&crate::shell::vfs_mount_table(), path, mime), Ok(true))
}

/// Stamp `path` with what the sniff/table says (an attribute already present is kept).
pub fn stamp(path: &str) -> bool {
    let mt = crate::shell::vfs_mount_table();
    let (m, src) = type_of_in(&mt, path);
    if src == Source::Attribute {
        return true;
    }
    matches!(stamp_as_in(&mt, path, &m), Ok(true))
}

/// Carry `src`'s `una:type` to `dst` (the `cp` leg; `rename` keeps attributes by inode). Copies
/// `una:preferred` too: a per-file opener choice belongs to the file. Silent when there is nothing to
/// carry or the destination takes no attributes.
pub fn carry_in(mt: &MountTable, src: &str, dst: &str) {
    for k in [TYPE_KEY, crate::fs::assoc::PREFERRED_KEY] {
        if let Ok(v) = mt.get_attr(src, k, KERNEL_PRINCIPAL) {
            match mt.set_attr(dst, k, v, KERNEL_PRINCIPAL) {
                Ok(()) => serial_println!("[filetype] carry {} {} -> {}", k, src, dst),
                Err(VfsError::Unsupported) => serial_println!("[filetype] carry=skip reason=enotsup key={} dst={}", k, dst),
                Err(_) => {}
            }
        }
    }
}

/// `file <path>` — `<path>: <type> <source>` (the Unix name, R26). `file -s <path>` also stamps it.
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    let (do_stamp, rest): (bool, Vec<&str>) = match args.first() {
        Some(&"-s") => (true, args[1..].to_vec()),
        _ => (false, args.to_vec()),
    };
    if rest.is_empty() {
        console.println("usage: file [-s] <path>...   (type and the leg that decided it; -s stamps una:type)");
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    for a in rest {
        let path = crate::shell::vfs_path(a);
        if let Err(e) = mt.stat(&path) {
            console.println(&alloc::format!("file: {}: {}", path, crate::fs::attrsys::refusal(&e)));
            continue;
        }
        let (m, src) = type_of_in(&mt, &path);
        let line = alloc::format!("{}: {} {}", path, m, src.name());
        console.println(&line);
        serial_println!("[filetype] file {}", line);
        if do_stamp && src != Source::Attribute {
            let r = match stamp_as_in(&mt, &path, &m) {
                Ok(true) => "stamped",
                Ok(false) => "skip (this volume carries no typed attributes, -ENOTSUP)",
                Err(_) => "failed",
            };
            console.println(&alloc::format!("file: {}: una:type {}", path, r));
        }
    }
}

/// `tests filetype` registration, once (the fixture decides PASS or SKIP legs itself).
pub fn ensure_tests() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("filetype", selftest);
        crate::fs::attrfacts::ensure_tests(); // ATTRCOLUMNS (B402): `tests attrcolumns` rides this registration
        // QUARRY2 (B336): `tests quarry2` rides this registration (no tests.rs line).
        #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
        crate::video::quarry::live::columns::ensure_tests();
        #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] crate::video::quarry::live::quarry3_tests(); crate::fs::appres::ensure_tests(); crate::tests::register("filetypes", crate::fs::assoc::selftest); // QUARRY3 (B413): `tests quarry3` rides this · FILETYPES (B423): `tests filetypes` too registration too. APPRES (B398): `tests appres` rides this registration (no tests.rs line).
    }
}

/// M4 — `tests filetype`. Creates one file of each kind under `/home` (else `/`), and checks:
/// the sniff on content, the table on a contentless name, unknown on neither, the attribute after a
/// stamp (UnaFS) or the skip (FAT), the association source, an opener changed with an ordinary
/// attribute write being followed, a per-file `una:preferred` winning, and Quarry's own handler
/// function agreeing with `opener_for` on every file.
///
/// `:: FILETYPE: typed=<n> sniffed=<n> ext=<n> assoc=<src> override=<ok> quarry=<ok> -> PASS ::`
pub fn selftest() {
    use crate::fs::assoc;
    let mt = crate::shell::vfs_mount_table();
    let dir = ["/home", "/"].iter().copied().find(|d| matches!(mt.stat(d), Ok(s) if matches!(s.kind, NodeKind::Dir)));
    let Some(dir) = dir else {
        serial_println!(":: FILETYPE: typed=0 sniffed=0 ext=0 assoc=- override=- quarry=- reason=no-volume -> SKIP ::");
        return;
    };
    let p = |leaf: &str| if dir == "/" { alloc::format!("/{}", leaf) } else { alloc::format!("{}/{}", dir, leaf) };
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    let mut wav = Vec::from(&b"RIFF\x24\x00\x00\x00WAVEfmt "[..]);
    wav.extend_from_slice(&[0u8; 8]);
    let mut lnx = alloc::vec![0u8; 64 + 56];
    lnx[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    lnx[4] = 2;
    lnx[5] = 1;
    lnx[32] = 64; // e_phoff
    lnx[54] = 56; // e_phentsize
    lnx[56] = 1; // e_phnum
    lnx[64] = 1; // PT_LOAD
    lnx[64 + 16..64 + 24].copy_from_slice(&0x40_0000u64.to_le_bytes());
    let mut una = lnx.clone();
    una[64 + 16..64 + 24].copy_from_slice(&0u64.to_le_bytes());
    // (leaf, content, the type it must come out as, the leg that must decide it before any stamp)
    let cases: [(&str, &[u8], &str, Source); 8] = [
        ("FTPIC.DAT", png, IMAGE_PNG, Source::Sniffed),
        ("FTSND", &wav, AUDIO_WAV, Source::Sniffed),
        ("FTNOTE", b"hello, typed world\n", TEXT_PLAIN, Source::Sniffed),
        ("FTLNX.ELF", &lnx, LINUX_ELF, Source::Sniffed),
        ("FTUNA", &una, UNAOS_ELF, Source::Sniffed),
        ("FTCONF.CFG", b"", TEXT_PLAIN, Source::Extension),
        ("FTBLOB.BIN", b"", UNAOS_BIN, Source::Extension),
        ("FTMYSTERY", b"", OCTET, Source::Unknown),
    ];
    let k = KERNEL_PRINCIPAL;
    let (mut sniffed, mut ext, mut unknown, mut wrong) = (0u32, 0u32, 0u32, 0u32);
    for (leaf, body, want, src_want) in cases.iter() {
        let path = p(leaf);
        let _ = mt.unlink(&path, k);
        if mt.create(&path, NodeKind::File, k).is_err() || (!body.is_empty() && mt.write(&path, 0, body, k).is_err()) {
            serial_println!(":: FILETYPE: typed=0 sniffed=0 ext=0 assoc=- override=- quarry=- dir={} reason=create {} -> FAIL ::", dir, leaf);
            return;
        }
        let (m, src) = type_of_in(&mt, &path);
        if m != *want || src != *src_want {
            serial_println!("[filetype] fixture MISMATCH {} -> {} {} (want {} {})", path, m, src.name(), want, src_want.name());
            wrong += 1;
        }
        match src {
            Source::Sniffed => sniffed += 1,
            Source::Extension => ext += 1,
            Source::Unknown => unknown += 1,
            Source::Attribute => {}
        }
    }
    // The attribute leg: stamp the text file and read it back as `attribute`.
    let note = p("FTNOTE");
    let attrs = !matches!(mt.list_attrs(&note, k), Err(VfsError::Unsupported));
    let mut typed = 0u32;
    let attr_ok = if attrs {
        for (leaf, _, want, _) in cases.iter() {
            let path = p(leaf);
            if matches!(stamp_as_in(&mt, &path, want), Ok(true)) && type_of_in(&mt, &path) == (String::from(*want), Source::Attribute) {
                typed += 1;
            }
        }
        typed == cases.len() as u32
    } else {
        let skipped = matches!(stamp_as_in(&mt, &note, TEXT_PLAIN), Ok(false));
        serial_println!("[filetype] fixture attribute legs SKIP (this volume carries no typed attributes; the builtin table is exercised)");
        skipped
    };
    // The association: source, then an opener changed with an ordinary attribute write.
    let _ = assoc::seed_in(&mt);
    let (op0, asrc) = assoc::opener_for_in(&mt, &note, TEXT_PLAIN);
    let assoc_ok;
    let override_ok;
    if attrs && asrc == "db" {
        let obj = assoc::object_path(TEXT_PLAIN);
        let was = mt.get_attr(&obj, assoc::PREFERRED_KEY, k).ok();
        let changed = mt.set_attr(&obj, assoc::PREFERRED_KEY, AttrValue::Str(String::from("org.unaos.fileview")), k).is_ok()
            && assoc::opener_for_in(&mt, &note, TEXT_PLAIN).0 == "fileview"; // FILETYPES (B423): the registry names a signature
        if let Some(v) = was { let _ = mt.set_attr(&obj, assoc::PREFERRED_KEY, v, k); }
        assoc_ok = changed && op0 == "textedit";
        // Per-file override wins over the database.
        let set = mt.set_attr(&note, assoc::PREFERRED_KEY, AttrValue::Str(String::from("fileview")), k).is_ok();
        let (op1, s1) = assoc::opener_for_in(&mt, &note, TEXT_PLAIN);
        override_ok = set && op1 == "fileview" && s1 == "override";
    } else {
        assoc_ok = !attrs && asrc == "registrant" && op0 == "textedit"; // FILETYPES (B423): no registry → the first registrant
        override_ok = !attrs; // SKIP leg on FAT: there is nowhere to put a per-file choice
    }
    // Quarry's handler function agrees with `opener_for` on every case file.
    #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let quarry = {
        let mut agree = true;
        for (leaf, _, _, _) in cases.iter() {
            let path = p(leaf);
            let (m, _) = type_of_in(&mt, &path);
            let want = crate::video::quarry::live::openers::effective(&assoc::opener_for_in(&mt, &path, &m).0, &path);
            let got = crate::video::quarry::live::open_handler_path(&path);
            if got != want {
                serial_println!("[filetype] fixture QUARRY MISMATCH {} handler={} opener_for={}", path, got, want);
                agree = false;
            }
        }
        if agree { "ok" } else { "MISMATCH" }
    };
    #[cfg(not(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))))]
    let quarry = "skip(no-quarry)";
    for (leaf, _, _, _) in cases.iter() {
        let _ = mt.unlink(&p(leaf), k);
    }
    let pass = wrong == 0 && sniffed == 5 && ext == 2 && unknown == 1 && attr_ok && assoc_ok && override_ok && quarry != "MISMATCH";
    serial_println!(
        ":: FILETYPE: typed={} sniffed={} ext={} assoc={} override={} quarry={} dir={} attrs={} -> {} ::",
        typed,
        sniffed,
        ext,
        asrc,
        if !attrs { "skip" } else if override_ok { "ok" } else { "FAIL" },
        quarry,
        dir,
        if attrs { "unafs" } else { "none(fat)" },
        if pass { "PASS" } else { "FAIL" }
    );
}

/// OPENERS (B379) — the ISO-BMFF type of `path` from its `ftyp` and `moov` wherever they lie: one 16-byte read
/// per top-level box (at most [`ISO_WALK_BOXES`]), then the `moov` payload whole if it is at most
/// [`ISO_MOOV_CAP`]; `demux_core::mime::iso_mime` decides. `None` (the head's answer stands) when the walk
/// breaks. The media (`mdat`) is never read.
fn iso_walk(mt: &MountTable, path: &str, size: u64) -> Option<&'static str> {
    let (mut ftyp, mut moov): (Option<Vec<u8>>, Option<Vec<u8>>) = (None, None);
    let mut off = 0u64;
    for _ in 0..ISO_WALK_BOXES {
        if off + 8 > size {
            break;
        }
        let hdr = mt.read(path, off, core::cmp::min(16, size - off) as usize).ok()?;
        let (ty, bsize, hl) = demux_core::mime::iso_box_header(&hdr)?;
        let end = if bsize == 0 { size } else { off.checked_add(bsize)? };
        let body_len = end.min(size).saturating_sub(off + hl as u64);
        match &ty {
            b"ftyp" if body_len <= 1024 => ftyp = mt.read(path, off + hl as u64, body_len as usize).ok(),
            b"moov" if body_len <= ISO_MOOV_CAP => {
                moov = mt.read(path, off + hl as u64, body_len as usize).ok();
                break;
            }
            b"moov" => break,
            _ => {}
        }
        off = end;
    }
    if ftyp.is_none() && moov.is_none() {
        return None;
    }
    Some(demux_core::mime::iso_mime(ftyp.as_deref(), moov.as_deref()))
}

/// How many top-level boxes [`iso_walk`] reads past before giving up.
const ISO_WALK_BOXES: usize = 64;
/// The largest `moov` payload [`iso_walk`] reads (sample tables of a long film run to a few MiB).
const ISO_MOOV_CAP: u64 = 4 << 20;

/// OPENERS (B379) — the witness `tests testf` prints after its TESTF line: for every claimed sample in `dir`,
/// its type (and the leg that decided), its opener (and that opener's source), the handler that would run in
/// THIS build, and whether the core that handler decodes with recognises the bytes. One line per sample, then
///
/// `:: OPENERS: test_f=<present> typed=<n> unhandled=<n> -> PASS :: opened=<n> cores=<ok>/<n> owed=<list> ::`
///
/// `unhandled` = a file with no type (`application/octet-stream`), or whose opener is not compiled into this
/// build. A type the database itself gives `none` (video: no player in this tree) is OWED, named on the line,
/// and does not count as unhandled — it is typed, and the gap is the tree's, said by name. R80: a test, run
/// only when asked.
pub fn openers_witness(dir: &str, names: &[&str]) {
    let mt = crate::shell::vfs_mount_table();
    let (mut present, mut typed, mut unhandled, mut opened, mut cores_ok, mut cores_n) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut owed: Vec<String> = Vec::new();
    for n in names {
        let path = alloc::format!("{}/{}", dir, n);
        let Ok(st) = mt.stat(&path) else { continue };
        present += 1;
        let (m, src) = type_of_in(&mt, &path);
        let (op, osrc) = crate::fs::assoc::opener_for_in(&mt, &path, &m);
        #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
        let handler = crate::video::quarry::live::openers::effective(&op, &path);
        #[cfg(not(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))))]
        let handler = op.clone();
        let is_typed = m != OCTET;
        typed += is_typed as u32;
        let is_owed = is_typed && op == "none";
        let is_unhandled = !is_typed || (!is_owed && handler == "none");
        unhandled += is_unhandled as u32;
        if is_owed {
            owed.push(alloc::format!("{}({})", n, m));
        }
        // The decoding core the handler uses must recognise the bytes (the type and the decoder agree).
        let head = mt.read(&path, 0, core::cmp::min(st.size, SNIFF_LEN as u64) as usize).unwrap_or_default();
        let core: Option<(&str, bool)> = match handler.as_str() {
            "facet" => Some(("pixel_core", pixel_core::sniff(&head).is_some())),
            "play" => Some(("audio_core", audio_core::sniff(&head) != audio_core::Format::Unknown)),
            "textedit" | "fileview" | "markdown" | "json" => Some(("text", looks_text(&head))),
            _ => None,
        };
        if handler != "none" {
            opened += 1;
        }
        if let Some((_, ok)) = core {
            cores_n += 1;
            cores_ok += ok as u32;
        }
        serial_println!(
            "[openers] {} type={} src={} opener={}({}) handler={} core={}{}",
            n,
            m,
            src.name(),
            op,
            osrc,
            handler,
            core.map_or("-", |c| c.0),
            match core { Some((_, true)) => "=ok", Some((_, false)) => "=REFUSED", None => "" }
        );
    }
    let pass = present as usize == names.len() && typed == present && unhandled == 0 && cores_ok == cores_n;
    serial_println!(
        ":: OPENERS: test_f={} typed={} unhandled={} -> {} :: opened={} cores={}/{} owed={}{} dir={} ::",
        present,
        typed,
        unhandled,
        if pass { "PASS" } else { "FAIL" },
        opened,
        cores_ok,
        cores_n,
        if owed.is_empty() { String::from("-") } else { owed.join(",") },
        if owed.is_empty() { "" } else { " reason=no-opener-in-this-tree(video: Stria's player, SR26)" },
        dir
    );
}
