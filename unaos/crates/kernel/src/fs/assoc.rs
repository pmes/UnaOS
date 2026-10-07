//! CHARTER: Kernel — fs-core
//!
//! FILETYPES (rmbp-ledger B423, MACPARITY §16 B3 / row 29) — THE FILETYPES REGISTRY, as ATTRIBUTES. (The type
//! database of FILETYPE M2, B307, moved and re-keyed Be's way: one store, never a second one — R79.)
//!
//! Each type is one system object `/system/filetypes/<mime with '/' as '-'>` (`/system/filetypes/image-png`)
//! carrying `una:description` ("PNG image"), `una:extensions` (`png`), `una:icon` (a glyph name) and
//! `una:preferred` — the SIGNATURE of the app that opens it (`org.unaos.facet`; Be's FileTypes "preferred
//! application"). They are ordinary attributes: `setfattr /system/filetypes/text-plain una:preferred=org.unaos.fileview`
//! (or the Settings pane's File Types tab, or `assoc text/plain fileview`) changes the default with no code. A
//! file's own `una:preferred` wins over its type's (`BEOS:PREFERRED_APP`).
//!
//! **No type→program table in code.** The programs that open a type are its REGISTRANTS
//! ([`crate::fs::appres::registrants`]): every kernel opener declares its doc types in its own resource block
//! (`unaos/res/<id>/app.res`), ring-3 programs through APPRES's `una:apps`. With no `una:preferred` the first
//! registrant opens the file; with none at all the answer is `none`, said by name. The registry is therefore NOT
//! needed to open a file (a FAT root, a card before its first login): the registrants answer from the compiled-in
//! resources and the source says `registrant`.
//!
//! **Built at LOGIN** (R93: the desktop is built at login; R80: nothing at boot): `login ok` owes [`build`], the
//! x86 device-service pass runs it ([`service`]; elsewhere inline) — every known type's object created in ONE
//! transaction, a present object's MISSING keys filled, a present key never overwritten (the user's amendment
//! sticks). Known types = [`TYPE_FACTS`] ∪ every type a registrant declares ∪ what the directory already holds.
//! Descriptions and glyphs are type FACTS (Be's MIME database shipped them); extensions are
//! `filetype::EXT_TABLE`'s, listed.
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::fs::appres::{self, Registrant};
use crate::fs::filetype as ft;
use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};

pub const DESCRIPTION_KEY: &str = una_abi::attr_keys::DESCRIPTION;
pub const EXTENSIONS_KEY: &str = una_abi::attr_keys::EXTENSIONS;
pub const ICON_KEY: &str = una_abi::attr_keys::ICON;
/// On a type object: the preferred app's signature. On a file: that file's own choice (signature, or the B307
/// opener id / program path).
pub const PREFERRED_KEY: &str = una_abi::attr_keys::PREFERRED;
/// The registry's directory on the system volume (ROOTDISK2: `/system` is UnaFS).
pub use type_core::TYPES_DIR; // SMALLFIX4 item 12 (TYPECORE fold): the one table owns the registry path

/// `(mime, icon glyph, description)` — the type FACTS. No opener column: who opens a type is its registrants'.
pub const TYPE_FACTS: &[(&str, &str, &str)] = &[
    (ft::TEXT_PLAIN, "doc", "Plain text"),
    (ft::IMAGE_PNG, "image", "PNG image"),
    (ft::AUDIO_WAV, "sound", "WAV audio"),
    (ft::UNAOS_ELF, "app", "UnaOS program"),
    (ft::UNAOS_BIN, "app", "UnaOS flat program"),
    (ft::LINUX_ELF, "app", "Linux program"),
    (ft::DIRECTORY, "folder", "Folder"),
    (ft::GZIP, "archive", "gzip archive"),
    (ft::TAR, "archive", "tar archive"),
    (ft::OCTET, "file", "Binary data"),
    (ft::TEXT_MARKDOWN, "doc", "Markdown"),
    (ft::APP_JSON, "doc", "JSON"),
    (ft::IMAGE_GIF, "image", "GIF image"),
    (ft::IMAGE_JPEG, "image", "JPEG image"),
    (ft::IMAGE_BMP, "image", "BMP image"),
    (ft::IMAGE_WEBP, "image", "WebP image"),
    (ft::IMAGE_QOI, "image", "QOI image"),
    (ft::IMAGE_SVG, "image", "SVG image"),
    (ft::AUDIO_FLAC, "sound", "FLAC audio"),
    (ft::AUDIO_OGG, "sound", "Ogg audio"),
    (ft::AUDIO_MPEG, "sound", "MP3 audio"),
    (ft::AUDIO_AAC, "sound", "AAC audio"),
    (ft::AUDIO_MP4, "sound", "MPEG-4 audio"),
    (ft::AUDIO_AIFF, "sound", "AIFF audio"),
    (ft::VIDEO_MP4, "video", "MPEG-4 video"),
    (ft::VIDEO_WEBM, "video", "WebM video"),
    (ft::VIDEO_MATROSKA, "video", "Matroska video"),
    (ft::IMAGE_ARW, "image", "Sony raw image"), // RAWCORE (B444)
    (ft::IMAGE_TIFF, "image", "TIFF image"), // RAWCORE (B444)
];

/// The facts row for `mime`, if any. Pure.
pub fn facts(mime: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    TYPE_FACTS.iter().find(|r| r.0 == mime)
}

/// `/system/filetypes/image-png` for `image/png`. Pure.
pub fn object_path(mime: &str) -> String {
    let mut s = String::from(TYPES_DIR);
    s.push('/');
    for c in mime.chars() {
        s.push(if c == '/' { '-' } else { c });
    }
    s
}

/// The MIME type a registry leaf names (`image-png` → `image/png`; a top-level type never carries a dash). Pure.
pub fn mime_of_leaf(leaf: &str) -> String {
    match type_core::mime_leaf(leaf) { Some((t, st)) => alloc::format!("{}/{}", t, st), None => String::from(leaf) } // SMALLFIX4 item 12: type_core's leaf rule
}

/// type_core's one table's extensions for `mime`, comma-separated (`md, markdown`). Pure. SMALLFIX4 item 12: walks
/// `type_core::extensions_of` (TYPECORE fold), no second filter over the table.
pub fn extensions_of(mime: &str) -> String {
    let v: Vec<&str> = type_core::extensions_of(mime).collect();
    v.join(", ")
}

fn str_attr(mt: &MountTable, path: &str, key: &str) -> Option<String> {
    match mt.get_attr(path, key, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// The registrants of `mime` (built-ins by their resources, then APPRES's ring-3 programs).
pub fn registrants_in(mt: &MountTable, mime: &str) -> Vec<Registrant> {
    appres::registrants(mt, mime, &object_path(mime))
}

/// The opener for `path` of type `mime`, and the source that decided: `override` (the file's own
/// `una:preferred`, honoured only when it names a REGISTRANT of `mime` — OPENERTRUST B447, else ignored with a line), `db` (the registry's `una:preferred` for the type), `registrant` (no preference: the first
/// program that declares the type) or `none` (nothing declares it — a video in this tree).
pub fn opener_for_in(mt: &MountTable, path: &str, mime: &str) -> (String, &'static str) {
    let obj = object_path(mime);
    if path != obj {
        if let Some(v) = str_attr(mt, path, PREFERRED_KEY) {
            match trusted_override_in(mt, &v, mime) {
                Some(op) => return (op, "override"),
                None => refused_wire(path, &v, mime),
            }
        }
    }
    if let Some(v) = str_attr(mt, &obj, PREFERRED_KEY) {
        let op = appres::opener_of_preferred(mt, &v);
        if !crate::fs::apptrust::is_foreign(mt, &op) { return (op, "db"); } // APPTRUST (B467): the registry's choice never names a program off the system volume
    }
    match registrants_in(mt, mime).into_iter().next() {
        Some(r) => (r.opener, "registrant"),
        None => (String::from("none"), "none"),
    }
}

/// [`opener_for_in`] over a fresh mount table.
pub fn opener_for(path: &str, mime: &str) -> (String, &'static str) {
    opener_for_in(&crate::shell::vfs_mount_table(), path, mime)
}

/// The type's description (`una:description` from the registry, else the facts row, else the MIME string).
pub fn name_of_in(mt: &MountTable, mime: &str) -> String {
    str_attr(mt, &object_path(mime), DESCRIPTION_KEY)
        .or_else(|| facts(mime).map(|r| String::from(r.2)))
        .unwrap_or_else(|| String::from(mime))
}

/// The type's icon glyph name, same order as [`name_of_in`].
pub fn icon_of_in(mt: &MountTable, mime: &str) -> String {
    str_attr(mt, &object_path(mime), ICON_KEY)
        .or_else(|| facts(mime).map(|r| String::from(r.1)))
        .unwrap_or_else(|| String::from("file"))
}

/// The type's extensions (the registry's `una:extensions`, else the extension table's).
pub fn extensions_in(mt: &MountTable, mime: &str) -> String {
    str_attr(mt, &object_path(mime), EXTENSIONS_KEY).unwrap_or_else(|| extensions_of(mime))
}

/// Does the root volume take attributes (the registry can live there)?
fn root_takes_attrs(mt: &MountTable) -> bool {
    !matches!(mt.list_attrs("/", KERNEL_PRINCIPAL), Err(VfsError::Unsupported) | Err(VfsError::NoSuchVolume) | Err(VfsError::NoSuchPath))
}

/// Every type the system knows: the facts, every registrant's declared type, and the registry's own objects
/// (a type the user added) — signature objects (`application-x-vnd.*`) are APPRES's, not types.
pub fn known_types_in(mt: &MountTable) -> Vec<String> {
    let mut v: Vec<String> = TYPE_FACTS.iter().map(|r| String::from(r.0)).collect();
    for m in appres::builtin_doctypes() {
        if !v.contains(&m) {
            v.push(m);
        }
    }
    if let Ok(ents) = mt.read_dir(TYPES_DIR) {
        for e in ents {
            if e.name.starts_with("application-x-vnd.") || e.name.starts_with('.') {
                continue;
            }
            let m = mime_of_leaf(&e.name);
            if !v.contains(&m) {
                v.push(m);
            }
        }
    }
    v
}

/// The four keys a type's object carries, as built (the preferred one only when something declares the type).
fn keys_for(mt: &MountTable, mime: &str) -> Vec<(String, AttrValue)> {
    let mut kv = alloc::vec![
        (String::from(DESCRIPTION_KEY), AttrValue::Str(facts(mime).map(|r| String::from(r.2)).unwrap_or_else(|| String::from(mime)))),
        (String::from(EXTENSIONS_KEY), AttrValue::Str(extensions_of(mime))),
        (String::from(ICON_KEY), AttrValue::Str(facts(mime).map(|r| String::from(r.1)).unwrap_or_else(|| String::from("file")))),
    ];
    if let Some(r) = registrants_in(mt, mime).into_iter().next() {
        kv.push((String::from(PREFERRED_KEY), AttrValue::Str(r.preferred_value())));
    }
    kv
}

/// Build the registry: `/system`, `/system/filetypes`, every known type's object that does not exist yet (ONE
/// transaction where the volume has one — BOOT80's shape), and on a present object every MISSING key (a present
/// key is the user's or an earlier build's and is never overwritten). `Ok((created, filled))`;
/// `Err(Unsupported)` on a root that takes no attributes (the registrants answer there).
pub fn seed_in(mt: &MountTable) -> Result<(usize, usize), VfsError> {
    if !root_takes_attrs(mt) {
        return Err(VfsError::Unsupported);
    }
    let k = KERNEL_PRINCIPAL;
    for d in ["/system", TYPES_DIR] {
        match mt.stat(d) {
            Ok(s) if matches!(s.kind, NodeKind::Dir) => {}
            Ok(_) => return Err(VfsError::NotADirectory),
            Err(_) => {
                mt.create(d, NodeKind::Dir, k)?;
            }
        }
    }
    let mut missing: Vec<(String, Vec<(String, AttrValue)>)> = Vec::new();
    let mut filled = 0usize;
    for mime in seed_types_in(mt) { // SMALLFIX5 (B480): + the types only a ring-3 program's signature object declares
        let obj = object_path(&mime);
        let want = keys_for(mt, &mime);
        if mt.stat(&obj).is_err() {
            missing.push((String::from(&obj[TYPES_DIR.len() + 1..]), want));
            continue;
        }
        let have: Vec<(String, AttrValue)> = mt.list_attrs(&obj, k).unwrap_or_default();
        let fill: Vec<(String, Option<AttrValue>)> = want
            .into_iter()
            .filter(|(key, _)| !have.iter().any(|(h, v)| h == key && !matches!(v, AttrValue::Str(s) if s.is_empty())))
            .map(|(key, v)| (key, Some(v)))
            .collect();
        if !fill.is_empty() {
            filled += fill.len();
            mt.set_attrs(&obj, &fill, k)?;
        }
    }
    if missing.is_empty() {
        return Ok((0, filled));
    }
    match mt.create_files_batch(TYPES_DIR, missing.clone(), k) {
        Ok(n) => return Ok((n, filled)),
        Err(VfsError::Unsupported) => {} // no batch on this volume: the per-object path below
        Err(e) => return Err(e),
    }
    let mut made = 0usize;
    for (leaf, attrs) in missing {
        let obj = alloc::format!("{}/{}", TYPES_DIR, leaf);
        mt.create(&obj, NodeKind::File, k)?;
        for (key, v) in attrs {
            mt.set_attr(&obj, &key, v, k)?;
        }
        made += 1;
    }
    Ok((made, filled))
}

static BUILDS: AtomicUsize = AtomicUsize::new(0);
static OWED: AtomicBool = AtomicBool::new(false);

/// How many times the registry was built this boot (the witness reads it).
pub fn builds() -> usize {
    BUILDS.load(Ordering::Relaxed)
}

/// Build the registry now, one wire line. `why` = `login` · `tests` · `verb`. ASSOCSTAMP (B460): `login`/`verb` read the
/// generation stamp first and skip the walk when it matches; `tests` always builds in full.
pub fn build(why: &str) -> usize {
    build_stamped(why, why == "tests").0
}

/// `login ok` (`login::close_into_session`): the registry is owed. On x86 with the compositor the device-service
/// pass builds it ([`service`]) — VFS work never runs in the click router; elsewhere it builds inline.
pub fn owe() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    OWED.store(true, Ordering::Release);
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    build("login");
}

/// The device-service pass: one relaxed load when nothing is owed.
pub fn service() {
    if OWED.load(Ordering::Acquire) && !TAKEN.load(Ordering::Acquire) && bar_first() && OWED.swap(false, Ordering::AcqRel) { // JOBSCAN (B497): the bar first; the jobs worker builds it when it took it
        build("login");
    }
}

/// Build once per boot unless a login already did (the verbs and fixtures ask before they read).
pub fn seed_once() -> usize {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) || builds() > 0 {
        return 0;
    }
    build("verb")
}

/// Set `mime`'s preferred app in the registry (`v` = a signature, an opener id or a program path; a registrant's
/// opener id is stored as its signature). An ordinary attribute write; the object is created when absent.
pub fn set_preferred_in(mt: &MountTable, mime: &str, v: &str) -> Result<String, VfsError> {
    if !root_takes_attrs(mt) {
        return Err(VfsError::Unsupported);
    }
    let obj = object_path(mime);
    if mt.stat(&obj).is_err() {
        let _ = seed_in(mt);
        if mt.stat(&obj).is_err() {
            mt.create(&obj, NodeKind::File, KERNEL_PRINCIPAL)?;
        }
    }
    let stored = registrants_in(mt, mime).into_iter().find(|r| r.opener == v || r.signature == v).map(|r| r.preferred_value()).unwrap_or_else(|| String::from(v));
    mt.set_attr(&obj, PREFERRED_KEY, AttrValue::Str(stored.clone()), KERNEL_PRINCIPAL)?;
    serial_println!("[filetypes] preferred type={} app={} obj={}", mime, stored, obj);
    Ok(stored)
}

/// `assoc` — every known type, its opener and the source. `assoc <mime>` — that one row with its registrants.
/// `assoc <mime> <app>` — set the type's preferred app in the registry (an ordinary attribute write).
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    seed_once();
    let mt = crate::shell::vfs_mount_table();
    let row = |mime: &str| -> String {
        // `object_path` as the "file" argument: the registry object is skipped as an override, so this reads
        // db-or-registrant.
        let (op, src) = opener_for_in(&mt, &object_path(mime), mime);
        alloc::format!("{:<26} {:<12} {:<10} {} ({}) [{}]", mime, op, src, name_of_in(&mt, mime), icon_of_in(&mt, mime), extensions_in(&mt, mime))
    };
    match args {
        [] => {
            let mimes = known_types_in(&mt);
            for m in &mimes {
                console.println(&row(m));
            }
            console.println(&alloc::format!("assoc: {} type(s) in {}", mimes.len(), TYPES_DIR));
        }
        [mime] => {
            console.println(&row(mime));
            for r in registrants_in(&mt, mime) {
                console.println(&alloc::format!("  open with: {} ({}) {}", r.name, r.opener, if r.signature.is_empty() { "-" } else { &r.signature }));
            }
        }
        [mime, app, ..] => match set_preferred_in(&mt, mime, app) {
            Ok(_) => console.println(&row(mime)),
            Err(VfsError::Unsupported) => console.println(&alloc::format!("assoc: {}: the root carries no typed attributes, the registrants decide (-ENOTSUP)", mime)),
            Err(e) => console.println(&alloc::format!("assoc: {}: {}", object_path(mime), crate::fs::attrsys::refusal(&e))),
        },
    }
}

// ── the File Types pane's model (Settings, B423 M4) — read and changed on the service pass, never in the router ───

/// One row of the pane: a type and what the registry says of it.
#[derive(Clone)]
pub struct TypeRow {
    pub mime: String,
    pub description: String,
    pub extensions: String,
    pub icon: String,
    /// The registry's `una:preferred` (a signature), empty when nothing declares the type.
    pub preferred: String,
    /// The preferred registrant's name (the stored value when it names no registrant).
    pub preferred_name: String,
    pub registrants: usize,
}

static VIEW: spin::Mutex<Vec<TypeRow>> = spin::Mutex::new(Vec::new());
static VIEW_OWED: AtomicBool = AtomicBool::new(false);
/// Row index + 1 whose preferred app the pane asked to advance (0 = none).
static CYCLE_OWED: AtomicUsize = AtomicUsize::new(0);

/// The pane's rows (a snapshot; no I/O).
pub fn view() -> Vec<TypeRow> {
    VIEW.lock().clone()
}

/// Ask the service pass to (re)read the rows.
pub fn owe_view() {
    VIEW_OWED.store(true, Ordering::Release);
}

/// Ask the service pass to hand row `i`'s type to its next registrant.
pub fn owe_cycle(i: usize) {
    CYCLE_OWED.store(i + 1, Ordering::Release);
}

fn rows_in(mt: &MountTable) -> Vec<TypeRow> {
    let mut v: Vec<TypeRow> = known_types_in(mt)
        .into_iter()
        .map(|mime| {
            let regs = registrants_in(mt, &mime);
            let preferred = str_attr(mt, &object_path(&mime), PREFERRED_KEY).or_else(|| regs.first().map(|r| r.preferred_value())).unwrap_or_default();
            let op = appres::opener_of_preferred(mt, &preferred);
            let preferred_name = regs.iter().find(|r| r.opener == op).map(|r| r.name.clone()).unwrap_or_else(|| preferred.clone());
            TypeRow {
                description: name_of_in(mt, &mime),
                extensions: extensions_in(mt, &mime),
                icon: icon_of_in(mt, &mime),
                preferred,
                preferred_name,
                registrants: regs.len(),
                mime,
            }
        })
        .collect();
    v.sort_by(|a, b| a.mime.cmp(&b.mime));
    v
}

/// Advance `mime`'s preferred app to the registrant after the current one (wrapping). The new opener, or why not.
pub fn cycle_preferred_in(mt: &MountTable, mime: &str) -> Result<String, &'static str> {
    let regs = registrants_in(mt, mime);
    if regs.is_empty() {
        return Err("no-registrant");
    }
    let (cur, _) = opener_for_in(mt, &object_path(mime), mime);
    let i = regs.iter().position(|r| r.opener == cur).map(|i| (i + 1) % regs.len()).unwrap_or(0);
    set_preferred_in(mt, mime, &regs[i].opener).map(|_| regs[i].opener.clone()).map_err(|_| "write-refused")
}

/// The service pass (Settings' `service`): a latched cycle, then a re-read when owed. `true` = the rows changed.
pub fn view_service() -> bool {
    let c = CYCLE_OWED.swap(0, Ordering::AcqRel);
    let owed = VIEW_OWED.swap(false, Ordering::AcqRel);
    if c == 0 && !owed {
        return false;
    }
    let mt = crate::shell::vfs_mount_table();
    if c != 0 {
        let mime = VIEW.lock().get(c - 1).map(|r| r.mime.clone());
        if let Some(m) = mime {
            let r = cycle_preferred_in(&mt, &m);
            serial_println!("[filetypes] pane type={} preferred={} via=settings", m, match &r { Ok(o) => o.as_str(), Err(w) => w });
        }
    }
    let rows = rows_in(&mt);
    *VIEW.lock() = rows;
    true
}

// ── `tests filetypes` (B423 M5; R80: run only when asked) ────────────────────────────────────────────────────────

/// `tests filetypes` — the registry built (every known type an object on an attribute root), every type's opener
/// resolved through the registry to a REGISTRANT (or `none` where nothing declares it), a user's amendment surviving
/// a rebuild, the sniff never overwriting a set `una:type` (ATTRCOLUMNS's refresh), and Open With's list for text.
///
/// `:: FILETYPES: types=<n> preferred=<n> resolved=<ok|why> -> PASS|FAIL :: registry=<dir|none> source=<db|registrants>
/// sticky=<ok|skip|why> amend=<ok|skip|why> openwith=<n> builds=<n>`.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let k = KERNEL_PRINCIPAL;
    let attrs = root_takes_attrs(&mt);
    build("tests");
    let types = known_types_in(&mt);
    let mut objects = 0usize;
    let mut preferred = 0usize;
    let mut why: Option<String> = None;
    for m in types.iter() {
        let obj = object_path(m);
        if mt.stat(&obj).is_ok() {
            objects += 1;
        } else if attrs && why.is_none() {
            why = Some(alloc::format!("missing:{}", m));
        }
        if str_attr(&mt, &obj, PREFERRED_KEY).is_some() {
            preferred += 1;
        }
        let regs = registrants_in(&mt, m);
        let (op, src) = opener_for_in(&mt, &obj, m);
        let ok = if regs.is_empty() { op == "none" } else { regs.iter().any(|r| r.opener == op) && (!attrs || src == "db") };
        if !ok && why.is_none() {
            why = Some(alloc::format!("{}:{}({})", m, op, src));
        }
    }
    // The user's amendment survives a rebuild: text/plain → the viewer, rebuild, still the viewer, restored.
    let amend = if attrs {
        let obj = object_path(ft::TEXT_PLAIN);
        let was = mt.get_attr(&obj, PREFERRED_KEY, k).ok();
        let set = set_preferred_in(&mt, ft::TEXT_PLAIN, "fileview").is_ok();
        let _ = seed_in(&mt);
        let held = opener_for_in(&mt, &obj, ft::TEXT_PLAIN) == (String::from("fileview"), "db");
        match was {
            Some(v) => { let _ = mt.set_attr(&obj, PREFERRED_KEY, v, k); }
            None => { let _ = mt.set_attrs(&obj, &[(String::from(PREFERRED_KEY), None)], k); }
        }
        if set && held { "ok" } else if !set { "set-refused" } else { "overwritten" }
    } else {
        "skip"
    };
    // The sniff never overwrites a set type: a text file typed by hand as Markdown keeps it through a forced refresh.
    let sticky = if attrs {
        let probe = "/system/FTPROBE.TXT";
        let _ = mt.unlink(probe, k);
        let made = mt.create(probe, NodeKind::File, k).is_ok() && mt.write(probe, 0, b"plain words\n", k).is_ok();
        let r = if !made {
            "probe-refused"
        } else if crate::fs::attrfacts::edit_in(&mt, probe, ft::TYPE_KEY, AttrValue::Str(String::from(ft::TEXT_MARKDOWN)), k).is_err() {
            "edit-refused"
        } else {
            let _ = crate::fs::attrfacts::refresh_in(&mt, probe, true);
            match mt.get_attr(probe, ft::TYPE_KEY, k) {
                Ok(AttrValue::Str(s)) if s == ft::TEXT_MARKDOWN => "ok",
                _ => "overwritten",
            }
        };
        let _ = mt.unlink(probe, k);
        r
    } else {
        "skip"
    };
    let openwith = registrants_in(&mt, ft::TEXT_PLAIN).len();
    let resolved = why.clone().unwrap_or_else(|| String::from("ok"));
    let pass = why.is_none() && matches!(amend, "ok" | "skip") && matches!(sticky, "ok" | "skip") && openwith >= 2;
    serial_println!(
        ":: FILETYPES: types={} preferred={} resolved={} -> {} :: registry={} objects={} source={} sticky={} amend={} openwith={} builds={}",
        types.len(), preferred, resolved, if pass { "PASS" } else { "FAIL" },
        if attrs { TYPES_DIR } else { "none" }, objects, if attrs { "db" } else { "registrants" }, sticky, amend, openwith, builds()
    );
}

// ── OPENERTRUST (B447, ARCHREVIEW F3) ────────────────────────────────────────────────────────────────────────────
//
// A FILE's `una:preferred` is the document's wish, not the user's act on a PROGRAM: it is honoured only when it
// names a REGISTRANT of the file's type (a built-in that declares the type, or a ring-3 program APPRES sighted and
// published in the type object's `una:apps`). Anything else — an absolute path to an arbitrary ELF, an opener id
// that does not declare the type — is ignored and the type's own default opens the file. Be's PREFERRED_APP named
// a signature the registrar knew, never a path.

/// The opener a file's `una:preferred` value `v` names, when that opener is a registrant of `mime`; `None` = refused.
pub fn trusted_override_in(mt: &MountTable, v: &str, mime: &str) -> Option<String> {
    let op = appres::opener_of_preferred(mt, v);
    if registrants_in(mt, mime).iter().any(|r| r.opener == op) { Some(op) } else { None }
}

/// The last path whose override was refused (the resolver runs per row and per frame; one line per file).
static REFUSED_LAST: spin::Mutex<String> = spin::Mutex::new(String::new());

fn refused_wire(path: &str, v: &str, mime: &str) {
    let mut last = REFUSED_LAST.lock();
    if last.as_str() != path {
        last.clear();
        last.push_str(path);
        drop(last);
        serial_println!("[openers] preferred={} refused=not-a-registrant path={} type={}", v, path, mime);
    }
}

/// `tests openertrust` (B447; R80: run only when asked) — a probe text file on the attribute root:
/// `registrant` = its `una:preferred=org.unaos.fileview` (a registrant of text) opens the viewer as `override`;
/// `foreign_path` = `una:preferred=/apps/EVIL.ELF` and `=launch` (not registrants of text) are both refused and the
/// type's default opens it; `carried` = a copy from ANOTHER attribute-taking volume (the first writable mount that is
/// not the root's storage) arrives without `una:preferred`, and a copy within the root keeps it.
///
/// `:: OPENERTRUST: registrant=<ok|why> foreign_path=<refused|why> carried=<stripped|skip|why> -> PASS|FAIL :: kept=<ok|why> foreign=<mount|none>`.
pub fn openertrust_selftest() {
    let mt = crate::shell::vfs_mount_table();
    let k = KERNEL_PRINCIPAL;
    if !root_takes_attrs(&mt) {
        serial_println!(":: OPENERTRUST: registrant=skip foreign_path=skip carried=skip -> PASS :: kept=skip foreign=none (the root takes no attributes: no file can carry una:preferred)");
        return;
    }
    let mk = |p: &str| {
        let _ = mt.unlink(p, k);
        mt.create(p, NodeKind::File, k).is_ok() && mt.write(p, 0, b"opener trust probe\n", k).is_ok()
    };
    let pref = |p: &str, v: &str| mt.set_attr(p, PREFERRED_KEY, AttrValue::Str(String::from(v)), k).is_ok();
    let has_pref = |p: &str| mt.get_attr(p, PREFERRED_KEY, k).is_ok();
    let _ = seed_in(&mt); // `/system` exists on an attribute root (ROOTDISK2); idempotent
    let probe = "/system/OTPROBE.TXT";
    let default = opener_for_in(&mt, &object_path(ft::TEXT_PLAIN), ft::TEXT_PLAIN).0;
    let registrant = if !mk(probe) {
        String::from("probe-refused")
    } else if !pref(probe, "org.unaos.fileview") {
        String::from("set-refused")
    } else {
        match opener_for_in(&mt, probe, ft::TEXT_PLAIN) {
            (op, "override") if op == "fileview" => String::from("ok"),
            (op, src) => alloc::format!("{}({})", op, src),
        }
    };
    let mut foreign_path = String::from("refused");
    for v in ["/apps/EVIL.ELF", "launch"] {
        let r = if pref(probe, v) { opener_for_in(&mt, probe, ft::TEXT_PLAIN) } else { (String::from("set-refused"), "") };
        if r.1 == "override" || r.0 == v || r.0 != default {
            foreign_path = alloc::format!("{}:{}({})", v, r.0, r.1);
            break;
        }
    }
    // A copy within the root keeps the choice; one from another volume does not.
    let copy = "/system/OTCOPY.TXT";
    let kept = if pref(probe, "org.unaos.fileview") && mk(copy) {
        crate::fs::filetype::carry_in(&mt, probe, copy);
        if has_pref(copy) { "ok" } else { "dropped" }
    } else {
        "probe-refused"
    };
    let _ = mt.unlink(copy, k);
    let mut foreign = String::from("none");
    let mut carried = String::from("skip");
    let prefixes: Vec<String> = mt.prefixes().into_iter().map(String::from).collect();
    for pfx in prefixes.iter() {
        if pfx == "/" || matches!(mt.same_volume(pfx, probe), Ok(true)) || !matches!(mt.write_veto(pfx), Ok(None)) {
            continue;
        }
        let src = alloc::format!("{}/OTPROBE.TXT", pfx.trim_end_matches('/'));
        if !mk(&src) {
            continue;
        }
        if !pref(&src, "org.unaos.fileview") {
            let _ = mt.unlink(&src, k);
            continue; // this volume takes no attributes: nothing to carry
        }
        foreign = pfx.clone();
        let made = mk(copy);
        crate::fs::filetype::carry_in(&mt, &src, copy);
        carried = String::from(if !made { "copy-refused" } else if has_pref(copy) { "CARRIED" } else { "stripped" });
        let _ = mt.unlink(&src, k);
        let _ = mt.unlink(copy, k);
        break;
    }
    let _ = mt.unlink(probe, k);
    let pass = registrant == "ok" && foreign_path == "refused" && kept == "ok" && matches!(carried.as_str(), "stripped" | "skip");
    serial_println!(
        ":: OPENERTRUST: registrant={} foreign_path={} carried={} -> {} :: kept={} foreign={} default={}",
        registrant, foreign_path, carried, if pass { "PASS" } else { "FAIL" }, kept, foreign, default
    );
}

// ── ASSOCSTAMP (rmbp-ledger B460): the registry's generation stamp ───────────────────────────────────────────────
//
// PERFREVIEW F4: a build with nothing to create re-walked every type (flight 24: `blocks_read=1191 … created=0
// ms=1662`). The registry directory carries ONE attribute naming what it was built from; when the compiled-in type
// set is unchanged the build is a single attribute read.

/// The stamp's key, on [`TYPES_DIR`] itself.
pub const STAMP_KEY: &str = una_abi::attr_keys::FILETYPES_STAMP;
/// The builder's version: bump when `seed_in` changes what it writes (the stamp then differs and the build runs once).
pub const BUILDER_VERSION: u32 = 1;

fn fnv(mut h: u64, b: &[u8]) -> u64 {
    for &x in b.iter().chain([0u8].iter()) {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The compiled-in type count: [`TYPE_FACTS`] ∪ every built-in's declared doc type. Pure.
pub fn builtin_type_count() -> usize {
    let mut n = TYPE_FACTS.len();
    for m in appres::builtin_doctypes() {
        if facts(&m).is_none() {
            n += 1;
        }
    }
    n
}

/// The stamp this build would write: `v<version> h=<FNV-1a 64 of TYPE_FACTS, EXT_TABLE and APPRES's BUILTIN> n=<types>`.
/// Pure (no I/O): everything `seed_in` derives an object from that is compiled in.
pub fn stamp_now() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = fnv(h, &BUILDER_VERSION.to_le_bytes());
    for (m, g, d) in TYPE_FACTS.iter() {
        h = fnv(fnv(fnv(h, m.as_bytes()), g.as_bytes()), d.as_bytes());
    }
    for (e, m) in ft::EXT_TABLE.iter() {
        h = fnv(fnv(h, e.as_bytes()), m.as_bytes());
    }
    h = appres::builtin_hash(h);
    alloc::format!("v{} h={:016x} n={}", BUILDER_VERSION, h, builtin_type_count())
}

/// The stamp the registry carries (ONE attribute read on one inode), `None` when absent or unreadable.
pub fn stamp_on_disk(mt: &MountTable) -> Option<String> {
    str_attr(mt, TYPES_DIR, STAMP_KEY)
}

/// Build unless the stamp matches (`force` builds regardless). `(created, skipped)`; one wire line either way.
pub fn build_stamped(why: &str, force: bool) -> (usize, bool) {
    let t0 = crate::arch::ms();
    let io0 = crate::fs::bootstep::io();
    let mt = crate::shell::vfs_mount_table();
    BUILDS.fetch_add(1, Ordering::Relaxed);
    let want = stamp_now();
    if !force && stamp_on_disk(&mt).as_deref() == Some(want.as_str()) {
        let d = crate::fs::bootstep::io().since(io0);
        serial_println!("[filetypes] built at={} dir={} created=0 filled=0 types={} skipped=stamp stamp=v{} blocks_read={} cmds={} ms={}",
            why, TYPES_DIR, builtin_type_count(), BUILDER_VERSION, d.blocks_read(), d.cmds(), crate::arch::ms().saturating_sub(t0));
        return (0, true);
    }
    match seed_in(&mt) {
        Ok((n, f)) => {
            let _ = crate::fs::rootacl::stamp(&mt); // ROOTACL (B456): the system trees take the `system` owner once the registry is built
            let st = match mt.set_attr(TYPES_DIR, STAMP_KEY, AttrValue::Str(want), KERNEL_PRINCIPAL) {
                Ok(()) => "written",
                Err(_) => "refused",
            };
            let d = crate::fs::bootstep::io().since(io0);
            crate::fs::bootstep::note_span("filetypes-build", t0, crate::arch::ms()); // SMALLFIX6 (B495): the full build's span, beside the boot steps
            serial_println!("[filetypes] built at={} dir={} created={} filled={} types={} stamp={} blocks_read={} cmds={} ms={}", why, TYPES_DIR, n, f,
                known_types_in(&mt).len(), st, d.blocks_read(), d.cmds(), crate::arch::ms().saturating_sub(t0));
            (n, false)
        }
        Err(VfsError::Unsupported) => { serial_println!("[filetypes] built at={} dir=none reason=enotsup (root takes no attributes) source=registrants", why); (0, false) }
        Err(e) => { serial_println!("[filetypes] built at={} FAILED ({}) source=registrants", why, crate::fs::attrsys::refusal(&e)); (0, false) }
    }
}

/// `tests assocstamp` (R80: run only when asked) — the registry built in full (the stamp written), the stamp check
/// MEASURED (one attribute read: `blocks_read`, `cmds`, `ms`), then a stale stamp forcing exactly one full build that
/// re-writes it, then a hit again.
///
/// `:: ASSOCSTAMP: stamp=<match|miss|none> blocks_read=<n> ms=<n> -> PASS|FAIL|SKIP reason= :: cmds=<n> stale=<rebuilt|why> rehit=<ok|why> bound_ms=10 stamp=<v> n=<n>`
pub fn stamp_selftest() {
    let mt = crate::shell::vfs_mount_table();
    if !root_takes_attrs(&mt) {
        serial_println!(":: ASSOCSTAMP: stamp=none blocks_read=0 ms=0 -> SKIP reason=no-attr-root ::");
        return;
    }
    let _ = build_stamped("tests", true);
    let want = stamp_now();
    let t0 = crate::arch::ms();
    let io0 = crate::fs::bootstep::io();
    let have = stamp_on_disk(&mt);
    let d = crate::fs::bootstep::io().since(io0);
    let ms = crate::arch::ms().saturating_sub(t0);
    let state = match &have { Some(h) if *h == want => "match", Some(_) => "miss", None => "none" };
    // A stale stamp (an arc changed the type set): the next build must NOT skip, and must leave the stamp current.
    let stale = if mt.set_attr(TYPES_DIR, STAMP_KEY, AttrValue::Str(String::from("v0 h=stale n=0")), KERNEL_PRINCIPAL).is_err() {
        "set-refused"
    } else {
        match build_stamped("tests", false) {
            (_, true) => "skipped-stale",
            (_, false) if stamp_on_disk(&mt).as_deref() == Some(want.as_str()) => "rebuilt",
            _ => "not-rewritten",
        }
    };
    let rehit = if build_stamped("tests", false).1 { "ok" } else { "built-again" };
    let pass = state == "match" && stale == "rebuilt" && rehit == "ok" && ms <= 10;
    serial_println!(":: ASSOCSTAMP: stamp={} blocks_read={} ms={} -> {} :: cmds={} stale={} rehit={} bound_ms=10 stamp=v{} n={}",
        state, d.blocks_read(), ms, if pass { "PASS" } else { "FAIL" }, d.cmds(), stale, rehit, BUILDER_VERSION, builtin_type_count());
}

/// REFUSALUI (B468): the wire's words when `path`'s own `una:preferred` is refused for type `mime`
/// (`preferred=<v> refused=not-a-registrant`), `None` when it has none or it is honoured. Read on the OPEN only
/// (the person's act), never by the per-row resolver a listing runs.
pub fn override_refusal(path: &str, mime: &str) -> Option<String> {
    let mt = crate::shell::vfs_mount_table();
    if path == object_path(mime) {
        return None;
    }
    let v = str_attr(&mt, path, PREFERRED_KEY)?;
    match trusted_override_in(&mt, &v, mime) {
        Some(_) => None,
        None => Some(alloc::format!("preferred={} refused=not-a-registrant", v)),
    }
}

// ── SMALLFIX5 (rmbp-ledger B480) item 1 — a ring-3 program's own doc type and the stamp ─────────────────────────
//
// The stamp hashes what is compiled in. A type only a ring-3 program declares (its APPRES block, cached on its
// signature object `application-x-vnd.*` in this directory) is outside the hash, so a stamp match never fills its
// `una:preferred`. A ROOT sighting outside the per-boot memo that declares such a type, with the type's object
// missing or carrying no preferred app, replaces the stamp (ONE attribute write); the next login build then runs
// `seed_in` in full, which creates the object from the signature object's declared types and fills the missing keys.
// A filled type never invalidates, so the steady state stays the one-attribute read.

/// The stamp an invalidation writes (never equal to [`stamp_now`]: version 0).
pub const STAMP_INVALID: &str = "v0 h=invalidated n=0";

/// `doctypes` the stamp's hash does not cover: not in [`TYPE_FACTS`], not a built-in's declared doc type. Pure.
pub fn uncovered_types(doctypes: &[String]) -> Vec<String> {
    let built = appres::builtin_doctypes();
    doctypes.iter().filter(|m| !m.is_empty() && facts(m).is_none() && !built.iter().any(|b| b == *m)).cloned().collect()
}

/// [`known_types_in`] plus every uncovered type a ring-3 program's signature object in [`TYPES_DIR`] declares
/// (`seed_in`'s set: a full build creates their objects). One attribute read per signature object, full builds only.
pub fn seed_types_in(mt: &MountTable) -> Vec<String> {
    let mut v = known_types_in(mt);
    if let Ok(ents) = mt.read_dir(TYPES_DIR) {
        for e in ents.iter().filter(|e| e.name.starts_with("application-x-vnd.")) {
            let obj = alloc::format!("{}/{}", TYPES_DIR, e.name);
            let decl: Vec<String> = str_attr(mt, &obj, midden_core::RES_KEY_DOCTYPES).unwrap_or_default().lines().map(String::from).collect();
            for m in uncovered_types(&decl) {
                if !v.contains(&m) {
                    v.push(m);
                }
            }
        }
    }
    v
}

/// Should a sighting declaring `doctypes` invalidate the stamp? The first uncovered type whose object is missing or
/// has no `una:preferred`, `None` when every one is filled (or none is uncovered).
pub fn stamp_owed_type(mt: &MountTable, doctypes: &[String]) -> Option<String> {
    uncovered_types(doctypes).into_iter().find(|m| str_attr(mt, &object_path(m), PREFERRED_KEY).is_none())
}

static INVALIDATIONS: AtomicUsize = AtomicUsize::new(0);

/// How many stamp invalidations this boot wrote (`tests smallfix5`).
pub fn invalidations() -> usize {
    INVALIDATIONS.load(Ordering::Relaxed)
}

/// APPRES's root sighting of `program` (not in the per-boot memo) declaring `doctypes`: replace the stamp when an
/// uncovered type is unfilled. One line when it writes: `[filetypes] stamp invalidated by=<program> type=<mime> written=<0|1>`.
/// `true` when the stamp was replaced.
pub fn stamp_invalidate_for(mt: &MountTable, program: &str, doctypes: &[String]) -> bool {
    let Some(m) = stamp_owed_type(mt, doctypes) else { return false };
    match stamp_on_disk(mt) {
        None => return false,                             // no stamp: the next build runs in full anyway
        Some(s) if s == STAMP_INVALID => return false,    // already invalidated this boot or an earlier one
        Some(_) => {}
    }
    let ok = mt.set_attr(TYPES_DIR, STAMP_KEY, AttrValue::Str(String::from(STAMP_INVALID)), KERNEL_PRINCIPAL).is_ok();
    if ok {
        INVALIDATIONS.fetch_add(1, Ordering::Relaxed);
    }
    serial_println!("[filetypes] stamp invalidated by={} type={} written={}", program, m, ok as u8);
    ok
}

// JOBSCAN (B497) — flight 27: the first-boot registry build (`blocks_read=20776 … ms=31154`) ran on the device-service
// pass BEFORE the bar's build got its next turn (`bar_ms=31205`), and held the pass that pumps hid 11 s. The bar
// paints first; on x86 the `jobs-scan` worker takes the owed build and runs it after its scan.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// The device-service pass waits for the bar (the desktop built and painted) before it builds the owed registry.
fn bar_first() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        crate::video::desktopbuild::settled()
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        true
    }
}

/// The `jobs-scan` worker takes the owed build (the device-service pass leaves it).
pub fn take_to_worker() {
    TAKEN.store(true, Ordering::Release);
}

/// The worker's turn: build the owed registry, if one is owed (one relaxed load otherwise), and hand the duty back.
pub fn build_owed() {
    if OWED.load(Ordering::Acquire) && OWED.swap(false, Ordering::AcqRel) {
        build("login");
    }
    TAKEN.store(false, Ordering::Release);
}
