//! CHARTER: Matrix — shared-core
//!
//! CARDREGISTRY (rmbp-ledger B833; owed by FILETYPES3 B820, storage.md §5) — the FILETYPES registry's compiled-in
//! inputs and its generation stamp, ONE copy for both rings. The kernel's `fs/assoc.rs` (the registry build, the login's
//! stamp read) and `fs/appres.rs` (the built-in apps' resource blocks) delegate here; `tools/unafs filetypes` stages
//! `/system/filetypes` onto a card image with the same objects and the same stamp, so a fresh card's first login is a
//! stamp hit (`[filetypes] login=stamp stamp=match`).
//!
//! What lives here: the type FACTS (`(mime, glyph, description)`), the 13 built-ins' packed resource blocks
//! (`unaos/res/<k>/<k>.unares`), who declares a type, the stamp (FNV-1a 64 over the builder version, the facts,
//! `type_core::EXT_TABLE` and every built-in's key + block), and the keys a freshly created type object carries.
//! Pure: no I/O, no lock.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use midden_core as mc;
use type_core as tc;

/// The registry's directory (type_core owns the path).
pub use type_core::TYPES_DIR;

/// The stamp's key, on [`TYPES_DIR`] itself.
pub const STAMP_KEY: &str = una_abi::attr_keys::FILETYPES_STAMP;
/// The builder's version: bump when the build changes what it writes (the stamp then differs and the build runs once).
pub const BUILDER_VERSION: u32 = 1;

/// `(mime, icon glyph, description)` — the type FACTS. No opener column: who opens a type is its registrants'.
pub const TYPE_FACTS: &[(&str, &str, &str)] = &[
    (tc::TEXT_PLAIN, "doc", "Plain text"),
    (tc::IMAGE_PNG, "image", "PNG image"),
    (tc::AUDIO_WAV, "sound", "WAV audio"),
    (tc::UNAOS_ELF, "app", "UnaOS program"),
    (tc::UNAOS_BIN, "app", "UnaOS flat program"),
    (tc::LINUX_ELF, "app", "Linux program"),
    (tc::DIRECTORY, "folder", "Folder"),
    (tc::GZIP, "archive", "gzip archive"),
    (tc::TAR, "archive", "tar archive"),
    (tc::OCTET, "file", "Binary data"),
    (tc::TEXT_MARKDOWN, "doc", "Markdown"),
    (tc::APP_JSON, "doc", "JSON"),
    (tc::IMAGE_GIF, "image", "GIF image"),
    (tc::IMAGE_JPEG, "image", "JPEG image"),
    (tc::IMAGE_BMP, "image", "BMP image"),
    (tc::IMAGE_WEBP, "image", "WebP image"),
    (tc::IMAGE_QOI, "image", "QOI image"),
    (tc::IMAGE_SVG, "image", "SVG image"),
    (tc::AUDIO_FLAC, "sound", "FLAC audio"),
    (tc::AUDIO_OGG, "sound", "Ogg audio"),
    (tc::AUDIO_MPEG, "sound", "MP3 audio"),
    (tc::AUDIO_AAC, "sound", "AAC audio"),
    (tc::AUDIO_MP4, "sound", "MPEG-4 audio"),
    (tc::AUDIO_AIFF, "sound", "AIFF audio"),
    (tc::VIDEO_MP4, "video", "MPEG-4 video"),
    (tc::VIDEO_WEBM, "video", "WebM video"),
    (tc::VIDEO_MATROSKA, "video", "Matroska video"),
    (tc::IMAGE_ARW, "image", "Sony raw image"), (tc::IMAGE_CR2, "image", "Canon raw image"), (tc::IMAGE_NEF, "image", "Nikon raw image"), // RAWCORE (B444)
    (tc::IMAGE_TIFF, "image", "TIFF image"), // RAWCORE (B444)
];

/// The kernel's own windowed apps and their packed blocks (`una-res pack unaos/res/<app>`). FILETYPES (B423): every
/// opener the dispatch runs is a REGISTRANT with resources — its doc types are its own declaration. The order is the
/// registrant order: for a type two declare, the earlier one is the default until the registry's `una:preferred` says
/// otherwise.
pub const BUILTIN: &[(&str, &[u8])] = &[
    ("quarry", include_bytes!("../../../../res/quarry/quarry.unares")),
    ("settings", include_bytes!("../../../../res/settings/settings.unares")),
    ("console", include_bytes!("../../../../res/console/console.unares")),
    ("shell", include_bytes!("../../../../res/shell/shell.unares")),
    ("facet", include_bytes!("../../../../res/facet/facet.unares")),
    ("player", include_bytes!("../../../../res/player/player.unares")), // PLAYER (B419): the audio player window
    ("markdown", include_bytes!("../../../../res/markdown/markdown.unares")),
    ("json", include_bytes!("../../../../res/json/json.unares")),
    ("textedit", include_bytes!("../../../../res/textedit/textedit.unares")),
    ("fileview", include_bytes!("../../../../res/fileview/fileview.unares")),
    ("play", include_bytes!("../../../../res/play/play.unares")),
    ("launch", include_bytes!("../../../../res/launch/launch.unares")),
    ("linux", include_bytes!("../../../../res/linux/linux.unares")),
];

/// The facts row for `mime`, if any.
pub fn facts(mime: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    TYPE_FACTS.iter().find(|r| r.0 == mime)
}

/// Does this resource block declare `mime` among its doc types?
pub fn declares(block: &[u8], mime: &str) -> bool {
    mc::res_str(block, mc::RES_KEY_DOCTYPES).map(|d| d.lines().any(|l| l.trim() == mime)).unwrap_or(false)
}

/// Every MIME type a built-in declares, in registrant order, each once.
pub fn builtin_doctypes() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, b) in BUILTIN.iter() {
        for l in mc::res_str(b, mc::RES_KEY_DOCTYPES).unwrap_or("").lines() {
            let l = l.trim();
            if !l.is_empty() && !out.iter().any(|m| m == l) {
                out.push(String::from(l));
            }
        }
    }
    out
}

fn fnv(mut h: u64, b: &[u8]) -> u64 {
    for &x in b.iter().chain([0u8].iter()) {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// ASSOCSTAMP (rmbp-ledger B460): fold every built-in's key and packed resource block into the FNV-1a 64 state `h`
/// (a changed doc type, signature or name changes the hash).
pub fn builtin_hash(mut h: u64) -> u64 {
    for (k, b) in BUILTIN.iter() {
        for &x in k.as_bytes().iter().chain([0u8].iter()).chain(b.iter()) {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// The compiled-in type count: [`TYPE_FACTS`] ∪ every built-in's declared doc type.
pub fn builtin_type_count() -> usize {
    let mut n = TYPE_FACTS.len();
    for m in builtin_doctypes() {
        if facts(&m).is_none() {
            n += 1;
        }
    }
    n
}

/// The stamp a build writes: `v<version> h=<FNV-1a 64 of TYPE_FACTS, EXT_TABLE and BUILTIN> n=<types>` — everything a
/// type object is derived from that is compiled in.
pub fn stamp() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = fnv(h, &BUILDER_VERSION.to_le_bytes());
    for (m, g, d) in TYPE_FACTS.iter() {
        h = fnv(fnv(fnv(h, m.as_bytes()), g.as_bytes()), d.as_bytes());
    }
    for (e, m) in tc::EXT_TABLE.iter() {
        h = fnv(fnv(h, e.as_bytes()), m.as_bytes());
    }
    h = builtin_hash(h);
    alloc::format!("v{} h={:016x} n={}", BUILDER_VERSION, h, builtin_type_count())
}

/// A type's registry leaf: `image/png` -> `image-png`.
pub fn leaf(mime: &str) -> String {
    mime.chars().map(|c| if c == '/' { '-' } else { c }).collect()
}

/// The type's description: its facts row, else the MIME type itself.
pub fn description(mime: &str) -> String {
    facts(mime).map(|r| String::from(r.2)).unwrap_or_else(|| String::from(mime))
}

/// The type's glyph name: its facts row, else `file`.
pub fn glyph(mime: &str) -> String {
    facts(mime).map(|r| String::from(r.1)).unwrap_or_else(|| String::from("file"))
}

/// type_core's one table's extensions for `mime`, comma-separated (`md, markdown`); empty when it has none.
pub fn extensions_list(mime: &str) -> String {
    let v: Vec<&str> = tc::extensions_of(mime).collect();
    v.join(", ")
}

/// The compiled-in keys every type object carries: description, icon, and the extensions when the table has any
/// (FILETYPES2 B513: an empty `una:extensions` read as missing, so it is omitted).
pub fn base_keys(mime: &str) -> Vec<(&'static str, String)> {
    let mut kv = alloc::vec![(una_abi::attr_keys::DESCRIPTION, description(mime)), (una_abi::attr_keys::ICON, glyph(mime))];
    let ext = extensions_list(mime);
    if !ext.is_empty() {
        kv.push((una_abi::attr_keys::EXTENSIONS, ext));
    }
    kv
}

/// The `una:preferred` a fresh object takes from the built-ins: the first declaring built-in's signature, else its
/// key (`Registrant::preferred_value`); `None` when no built-in declares `mime`.
pub fn builtin_preferred(mime: &str) -> Option<String> {
    let (k, b) = BUILTIN.iter().find(|(_, b)| declares(b, mime))?;
    match mc::res_str(b, mc::RES_KEY_SIGNATURE) {
        Some(s) if !s.is_empty() => Some(String::from(s)),
        _ => Some(String::from(*k)),
    }
}

/// Every key a freshly created type object carries, in the build's order: [`base_keys`] then `una:preferred` when a
/// built-in declares the type.
pub fn object_keys(mime: &str) -> Vec<(&'static str, String)> {
    let mut kv = base_keys(mime);
    if let Some(p) = builtin_preferred(mime) {
        kv.push((una_abi::attr_keys::PREFERRED, p));
    }
    kv
}

/// The compiled-in types: [`TYPE_FACTS`] in order, then every built-in's declared type the facts lack.
pub fn builtin_types() -> Vec<String> {
    let mut v: Vec<String> = TYPE_FACTS.iter().map(|r| String::from(r.0)).collect();
    for m in builtin_doctypes() {
        if !v.contains(&m) {
            v.push(m);
        }
    }
    v
}

/// Every compiled-in type's object as a fresh volume's build creates it: `(leaf, keys)`.
pub fn fresh_objects() -> Vec<(String, Vec<(&'static str, String)>)> {
    builtin_types().into_iter().map(|m| (leaf(&m), object_keys(&m))).collect()
}
