//! TYPECORE (rmbp-ledger B450, R79) — THE type table, both rings.
//!
//! One extension-to-MIME table ([`EXT_TABLE`]), one built-in magic sniff in one order ([`sniff_magic`]), the
//! text/JSON/Markdown heads, and the ELF split (UnaOS vs Linux) read through `elf_core::read_phdrs` — the parse
//! the two kernel loaders already share. The kernel's `fs/filetype.rs` (attributes first, then this sniff, then the
//! cores' own `mime_of`, then this table) and the host's `bandy` (`FacetCommand::image_mime_for`) both read it; the
//! FILETYPES registry's `una:extensions` lists [`extensions_of`]. Nothing here keeps a store, allocates, or reads
//! a volume. Design: `docs/dev/evidence/rmbp-1005/typecore.md`.
#![no_std]
#![forbid(unsafe_code)]

pub const TEXT_PLAIN: &str = "text/plain";
pub const TEXT_MARKDOWN: &str = "text/markdown";
pub const APP_JSON: &str = "application/json";
pub const IMAGE_PNG: &str = "image/png";
pub const IMAGE_GIF: &str = "image/gif";
pub const IMAGE_JPEG: &str = "image/jpeg";
pub const IMAGE_BMP: &str = "image/bmp";
pub const IMAGE_WEBP: &str = "image/webp";
pub const IMAGE_QOI: &str = "image/qoi";
pub const IMAGE_SVG: &str = "image/svg+xml";
pub const AUDIO_WAV: &str = "audio/wav";
pub const AUDIO_FLAC: &str = "audio/flac";
pub const AUDIO_OGG: &str = "audio/ogg";
pub const AUDIO_MPEG: &str = "audio/mpeg";
pub const AUDIO_AAC: &str = "audio/aac";
pub const AUDIO_AIFF: &str = "audio/aiff";
pub const AUDIO_MP4: &str = "audio/mp4";
pub const VIDEO_MP4: &str = "video/mp4";
pub const VIDEO_WEBM: &str = "video/webm";
pub const VIDEO_MATROSKA: &str = "video/x-matroska";
pub const UNAOS_ELF: &str = "application/x-unaos-elf";
pub const UNAOS_BIN: &str = "application/x-unaos-bin";
pub const LINUX_ELF: &str = "application/x-linux-elf";
pub const GZIP: &str = "application/gzip";
pub const TAR: &str = "application/x-tar";
pub const DIRECTORY: &str = "inode/directory";
pub const OCTET: &str = "application/octet-stream";

/// The lowest `PT_LOAD` vaddr a static Linux image may carry (`linuxabi::elf::IMAGE_FLOOR`). A UnaOS image is
/// linked at vaddr 0 and biased into a slot window, so its lowest `PT_LOAD` sits below it.
pub const LINUX_VADDR_FLOOR: u64 = 0x1_0000;

/// THE extension table. Matched case-insensitively against the text after the LAST dot (a leading dot is a name,
/// not an extension). Used only when the bytes say nothing (an empty file, a head no magic or core recognises).
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
    ("jpg", IMAGE_JPEG),
    ("jpeg", IMAGE_JPEG),
    // TYPECORE: the JPEG/BMP spellings bandy's host twin knew and the kernel did not — one table now.
    ("jpe", IMAGE_JPEG),
    ("jfif", IMAGE_JPEG),
    ("bmp", IMAGE_BMP),
    ("dib", IMAGE_BMP),
    ("webp", IMAGE_WEBP),
    ("qoi", IMAGE_QOI),
    ("svg", IMAGE_SVG),
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

/// The extension of `name` (without the dot), or `None`. Both separators, so a host path works too.
pub fn ext_of(name: &str) -> Option<&str> {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match leaf.rfind('.') {
        Some(i) if i > 0 && i + 1 < leaf.len() => Some(&leaf[i + 1..]),
        _ => None,
    }
}

/// The table's answer for `name`, or `None`.
pub fn by_extension(name: &str) -> Option<&'static str> {
    let e = ext_of(name)?;
    EXT_TABLE.iter().find(|(x, _)| e.eq_ignore_ascii_case(x)).map(|(_, m)| *m)
}

/// The table's extensions for `mime`, in table order (FILETYPES' `una:extensions`).
pub fn extensions_of(mime: &str) -> impl Iterator<Item = &'static str> + '_ {
    EXT_TABLE.iter().filter(move |(_, m)| *m == mime).map(|(e, _)| *e)
}

/// Does a `una:extensions` value (`md, markdown`; commas and/or spaces, an optional leading dot) name `ext`
/// (case-insensitive)? The B423 leg: a user-added extension on a FILETYPES object.
pub fn ext_in_list(list: &str, ext: &str) -> bool {
    !ext.is_empty()
        && list.split([',', ' ', '\t']).map(|e| e.strip_prefix('.').unwrap_or(e)).any(|e| !e.is_empty() && e.eq_ignore_ascii_case(ext))
}

/// The FILETYPES registry (B423): one object per type under this directory, named by [`mime_leaf`]'s inverse
/// (`image/png` → `image-png`), its user-amendable extension list under [`EXTENSIONS_KEY`].
pub const TYPES_DIR: &str = "/system/filetypes";
pub const EXTENSIONS_KEY: &str = "una:extensions";

/// The `(type, subtype)` a registry leaf names (`image-png` → `("image", "png")`; a top-level type never carries a
/// dash, a subtype may: `video-x-matroska`). `None` for a leaf with no dash or an empty half.
pub fn mime_leaf(leaf: &str) -> Option<(&str, &str)> {
    leaf.split_once('-').filter(|(t, s)| !t.is_empty() && !s.is_empty())
}

/// UnaOS or Linux, for a buffer that starts with the ELF magic: the lowest `PT_LOAD` vaddr through
/// `elf_core::read_phdrs` (the loaders' parse) against [`LINUX_VADDR_FLOOR`]; when the table yields no `PT_LOAD`
/// (cut by the sniff window, or malformed) `e_entry` stands in.
pub fn elf_flavour(b: &[u8]) -> &'static str {
    let mut segs = [elf_core::Seg { flags: 0, vaddr: 0, filesz: 0, memsz: 0 }; elf_core::MAX_SEGS];
    let min = match elf_core::read_phdrs(b, &mut segs) {
        Some((n, _)) => segs[..n].iter().map(|s| s.vaddr).min(),
        None => None,
    };
    let entry = b.get(24..32).map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]));
    let va = min.or(entry).unwrap_or(0);
    if va >= LINUX_VADDR_FLOOR { LINUX_ELF } else { UNAOS_ELF }
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
/// The two ISO-BMFF top-level box types the type of a file is read from.
pub const ISO_FTYP: [u8; 4] = *b"ftyp";
pub const ISO_MOOV: [u8; 4] = *b"moov";

/// The built-in magics, in order: PNG, `RIFF....WAVE`, ELF (split by [`elf_flavour`]), gzip, `ustar` at 257, GIF.
/// `None` = none of these (the cores' own `mime_of`, then the text heads, decide next).
pub fn sniff_magic(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&PNG_MAGIC) {
        return Some(IMAGE_PNG);
    }
    if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        return Some(AUDIO_WAV);
    }
    if b.starts_with(&ELF_MAGIC) {
        return Some(elf_flavour(b));
    }
    if b.starts_with(&[0x1f, 0x8b]) {
        return Some(GZIP);
    }
    if b.len() >= 262 && &b[257..262] == b"ustar" {
        return Some(TAR);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(IMAGE_GIF);
    }
    None
}

/// A container head the container core should judge: an `ftyp`/`moov` box at offset 4, or the EBML magic.
pub fn container_head(b: &[u8]) -> bool {
    (b.len() >= 8 && (b[4..8] == ISO_FTYP || b[4..8] == ISO_MOOV)) || b.starts_with(&EBML_MAGIC)
}

/// The text heads, after the cores: JSON, then Markdown, then plain text; `None` when `b` is not text.
pub fn sniff_text(b: &[u8]) -> Option<&'static str> {
    if !looks_text(b) {
        return None;
    }
    Some(if looks_json(b) {
        APP_JSON
    } else if looks_markdown(b) {
        TEXT_MARKDOWN
    } else {
        TEXT_PLAIN
    })
}

/// Is `b` (a PREFIX of a file, possibly cut mid-character) text: no NUL, valid UTF-8 up to a cut tail of at most
/// 3 bytes, and almost no control bytes besides TAB/LF/CR/FF/ESC.
pub fn looks_text(b: &[u8]) -> bool {
    if b.is_empty() || b.contains(&0) {
        return false;
    }
    if let Err(e) = core::str::from_utf8(b) {
        if e.error_len().is_some() || b.len() - e.valid_up_to() > 3 {
            return false;
        }
    }
    let ctl = b.iter().filter(|&&c| c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    ctl * 32 <= b.len()
}

/// A Markdown head: an ATX heading `# ` (up to six `#`) on the first line, or YAML front matter (`---`).
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

/// A JSON head: after whitespace, `{` followed by `"` or `}`, or `[` followed by a value's first byte. Valid
/// FIRST TOKENS, not a parse.
pub fn looks_json(b: &[u8]) -> bool {
    let ws = |c: &u8| matches!(*c, b' ' | b'\t' | b'\r' | b'\n');
    let mut it = b.iter().skip_while(|c| ws(c));
    let first = match it.next() {
        Some(&c) => c,
        None => return false,
    };
    let next = it.find(|c| !ws(c)).copied();
    match (first, next) {
        (b'{', Some(b'"')) | (b'{', Some(b'}')) => true,
        (b'[', Some(c)) => matches!(c, b'"' | b'{' | b'[' | b']' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'),
        _ => false,
    }
}

/// The fixture heads the kernel's `tests filetype` writes (and the KATs below read): the smallest bytes each
/// built-in magic answers for. No allocation; the kernel copies them.
pub mod fixture {
    /// A PNG signature and the IHDR length word.
    pub const PNG: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    /// `RIFF` .. `WAVE` `fmt ` and eight zero bytes.
    pub const WAV: [u8; 24] = *b"RIFF\x24\x00\x00\x00WAVEfmt \0\0\0\0\0\0\0\0";
    /// An ELF64 LE header with one `PT_LOAD` at `vaddr` (64 + 56 bytes).
    pub const fn elf(vaddr: u64) -> [u8; 120] {
        let mut b = [0u8; 120];
        b[0] = 0x7f;
        b[1] = b'E';
        b[2] = b'L';
        b[3] = b'F';
        b[4] = 2;
        b[5] = 1;
        b[32] = 64; // e_phoff
        b[54] = 56; // e_phentsize
        b[56] = 1; // e_phnum
        b[64] = 1; // PT_LOAD
        let v = vaddr.to_le_bytes();
        let mut i = 0;
        while i < 8 {
            b[80 + i] = v[i];
            i += 1;
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_answers() {
        assert_eq!(by_extension("/home/a/Photo.JPG"), Some(IMAGE_JPEG));
        assert_eq!(by_extension("C:\\x\\scan.dib"), Some(IMAGE_BMP));
        assert_eq!(by_extension("notes.md"), Some(TEXT_MARKDOWN));
        assert_eq!(by_extension(".profile"), None);
        assert_eq!(by_extension("trailing."), None);
        assert_eq!(by_extension("noext"), None);
        assert_eq!(ext_of("a.tar.gz"), Some("gz"));
    }

    #[test]
    fn table_is_a_function() {
        for (i, (e, _)) in EXT_TABLE.iter().enumerate() {
            assert!(e.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "{e}");
            assert!(!EXT_TABLE[i + 1..].iter().any(|(x, _)| x == e), "duplicate {e}");
        }
    }

    #[test]
    fn extensions_listed_and_user_lists() {
        let v: [&str; 2] = {
            let mut it = extensions_of(TEXT_MARKDOWN);
            [it.next().unwrap(), it.next().unwrap()]
        };
        assert_eq!(v, ["md", "markdown"]);
        assert_eq!(extensions_of("x/none").count(), 0);
        assert!(ext_in_list("txt, foo", "FOO"));
        assert!(ext_in_list(".foo .bar", "bar"));
        assert!(!ext_in_list("txt,foobar", "foo"));
        assert!(!ext_in_list("txt,,", ""));
        assert_eq!(mime_leaf("video-x-matroska"), Some(("video", "x-matroska")));
        assert_eq!(mime_leaf("image"), None);
    }

    #[test]
    fn magics_in_order() {
        assert_eq!(sniff_magic(&fixture::PNG), Some(IMAGE_PNG));
        assert_eq!(sniff_magic(&fixture::WAV), Some(AUDIO_WAV));
        assert_eq!(sniff_magic(&fixture::elf(0x40_0000)), Some(LINUX_ELF));
        assert_eq!(sniff_magic(&fixture::elf(0)), Some(UNAOS_ELF));
        assert_eq!(sniff_magic(&[0x1f, 0x8b, 8, 0]), Some(GZIP));
        let mut tar = [0u8; 300];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(sniff_magic(&tar), Some(TAR));
        assert_eq!(sniff_magic(b"GIF89a\x01\x00"), Some(IMAGE_GIF));
        assert_eq!(sniff_magic(b"hello"), None);
        assert!(container_head(b"\0\0\0\x18ftypisom"));
        assert!(!container_head(b"\0\0\0\x18mdat...."));
    }

    #[test]
    fn elf_split_reads_the_loaders_parse() {
        // A truncated table (phnum past the window) falls back to e_entry.
        let mut b = fixture::elf(0);
        b[56] = 9;
        b[24..32].copy_from_slice(&0x40_1000u64.to_le_bytes());
        assert_eq!(elf_flavour(&b), LINUX_ELF);
        // The lowest PT_LOAD decides, not the first.
        let mut two = [0u8; 64 + 112];
        two[..120].copy_from_slice(&fixture::elf(0x40_0000));
        two[56] = 2;
        two[120] = 1;
        assert_eq!(elf_flavour(&two), UNAOS_ELF);
    }

    #[test]
    fn text_heads() {
        assert_eq!(sniff_text(b"{\"a\":1}"), Some(APP_JSON));
        assert_eq!(sniff_text(b"# Title\n"), Some(TEXT_MARKDOWN));
        assert_eq!(sniff_text(b"#comment\n"), Some(TEXT_PLAIN));
        assert_eq!(sniff_text(b"a\0b"), None);
        assert!(looks_text(&[b'a', 0xE2, 0x82])); // cut mid-character
    }

    #[test]
    fn strings_are_the_cores_own() {
        let cores = [
            pixel_core::mime_of(&fixture::PNG),
            pixel_core::mime_of(b"GIF89a\x01\x00\x01\x00\x00\x00\x00"),
            pixel_core::mime_of(b"qoif\0\0\0\x01\0\0\0\x01\x04\x00"),
        ];
        assert_eq!(cores, [Some(IMAGE_PNG), Some(IMAGE_GIF), Some(IMAGE_QOI)]);
        assert_eq!(audio_core::mime_of(&fixture::WAV).map(|m| m.0), Some(AUDIO_WAV));
        assert_eq!(audio_core::mime_of(b"fLaC\0\0\0\x22").map(|m| m.0), Some(AUDIO_FLAC));
        assert_eq!(audio_core::mime_of(b"OggS\0\x02").map(|m| m.0), Some(AUDIO_OGG));
        assert_eq!(
            [demux_core::mime::AUDIO_MP4, demux_core::mime::VIDEO_MP4, demux_core::mime::VIDEO_WEBM, demux_core::mime::VIDEO_MATROSKA],
            [AUDIO_MP4, VIDEO_MP4, VIDEO_WEBM, VIDEO_MATROSKA]
        );
    }
}
