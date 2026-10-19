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
//! **Owed at LOGIN, built AFTER THE BAR** (R93; R80: nothing at boot; FILETYPES2 B513): `login ok` reads the stamp
//! ([`owe`]) and builds nothing — a match publishes the stored registry, a miss leaves it unpublished (the resolver
//! answers from the facts) and the `jobs-scan` worker builds it after the bar ([`build_owed`]; the device-service
//! pass [`service`] when no worker took it; inline off x86), publishing it with one store when done — every known type's object created in its OWN
//! bounded transaction (REGISTRYCHUNK B508: per object, the task yielding between), a present object's MISSING keys filled, a present key never overwritten (the user's amendment
//! sticks). Known types = [`TYPE_FACTS`] ∪ every type a registrant declares ∪ what the directory already holds.
//! Descriptions and glyphs are type FACTS (Be's MIME database shipped them); extensions are
//! `filetype::EXT_TABLE`'s, listed.
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

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

/// FILETYPES2 (B513): `key` on `mime`'s registry object — only while the registry is PUBLISHED ([`registry_readable`]);
/// `None` while the login found no current stamp and the worker's build has not swapped in (the resolver then answers
/// from the type facts, the extension table and the registrants: the empty-but-valid registry).
fn reg_attr(mt: &MountTable, mime: &str, key: &str) -> Option<String> {
    if !registry_readable() {
        return None;
    }
    str_attr(mt, &object_path(mime), key)
}

/// The registrants of `mime` (built-ins by their resources, then APPRES's ring-3 programs).
pub fn registrants_in(mt: &MountTable, mime: &str) -> Vec<Registrant> {
    let regs = appres::registrants(mt, mime, &object_path(mime)); #[cfg(feature = "stria")] let regs = stria_first(regs); regs // STRIAMETAL (LEDGER SR139): under `stria`, a sighted STRIA.ELF opens the media types it declares ahead of the built-in Player (fn at the file tail). LINE-NEUTRAL fold.
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
    if let Some(v) = reg_attr(mt, mime, PREFERRED_KEY) { // FILETYPES2 (B513): the published registry only
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
    reg_attr(mt, mime, DESCRIPTION_KEY)
        .or_else(|| facts(mime).map(|r| String::from(r.2)))
        .unwrap_or_else(|| String::from(mime))
}

/// The type's icon glyph name, same order as [`name_of_in`].
pub fn icon_of_in(mt: &MountTable, mime: &str) -> String {
    reg_attr(mt, mime, ICON_KEY)
        .or_else(|| facts(mime).map(|r| String::from(r.1)))
        .unwrap_or_else(|| String::from("file"))
}

/// The type's extensions (the registry's `una:extensions`, else the extension table's).
pub fn extensions_in(mt: &MountTable, mime: &str) -> String {
    reg_attr(mt, mime, EXTENSIONS_KEY).unwrap_or_else(|| extensions_of(mime))
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
    if let Ok(ents) = mt.read_dir_kinds(TYPES_DIR) { // JOBSCAN2 (B551): names only — `read_dir` read every row's inode inside ONE masked transaction
        for (name, _) in ents {
            if name.starts_with("application-x-vnd.") || name.starts_with('.') {
                continue;
            }
            let m = mime_of_leaf(&name);
            if !v.contains(&m) {
                v.push(m);
            }
        }
    }
    v
}

/// The four keys a type's object carries, as built (the preferred one only when something declares the type).
/// `exists = false` (REGISTRYCHUNK B508: the object is not there yet) reads no attribute — the
/// built-in registrants only, which is all an absent object could name.
///
/// FILETYPES2 (B513): a type with no extension in the table (`inode/directory`, `application/octet-stream`,
/// `audio/opus`) carries NO `una:extensions` key — an empty one read as missing to the fill below, so every full build
/// re-wrote those three objects (flight 28: `built at=tests … created=0 filled=3`). A type only a ring-3 program
/// declares names that program (`sigs`: its signature object's signature and path) — the built-ins alone named
/// nobody, so the object stayed without `una:preferred` and SMALLFIX5's sighting invalidated the stamp on every boot.
fn keys_for_obj(mt: &MountTable, mime: &str, exists: bool, sigs: &[SigDecl]) -> Vec<(String, AttrValue)> {
    let mut kv = alloc::vec![
        (String::from(DESCRIPTION_KEY), AttrValue::Str(facts(mime).map(|r| String::from(r.2)).unwrap_or_else(|| String::from(mime)))),
        (String::from(ICON_KEY), AttrValue::Str(facts(mime).map(|r| String::from(r.1)).unwrap_or_else(|| String::from("file")))),
    ];
    let ext = extensions_of(mime);
    if !ext.is_empty() {
        kv.push((String::from(EXTENSIONS_KEY), AttrValue::Str(ext)));
    }
    let regs = if exists { registrants_in(mt, mime) } else { appres::builtin_registrants(mime) };
    if let Some(r) = regs.into_iter().next() {
        kv.push((String::from(PREFERRED_KEY), AttrValue::Str(r.preferred_value())));
    } else if let Some(d) = sigs.iter().find(|d| d.types.iter().any(|t| t == mime)) {
        kv.push((String::from(PREFERRED_KEY), AttrValue::Str(d.sig.clone())));
        if !d.path.is_empty() {
            kv.push((String::from(appres::KEY_APPS), AttrValue::Str(d.path.clone())));
        }
    }
    kv
}

/// A ring-3 program's signature object in [`TYPES_DIR`]: its signature, its root path (empty when foreign or
/// unrecorded) and the doc types it declares that the stamp does not cover.
pub struct SigDecl {
    pub sig: String,
    pub path: String,
    pub types: Vec<String>,
}

/// Every signature object's uncovered declared types (one listing, one or two attribute reads per signature object;
/// full builds only).
fn sig_decls(mt: &MountTable) -> Vec<SigDecl> {
    let mut out = Vec::new();
    if let Ok(ents) = mt.read_dir_kinds(TYPES_DIR) { // JOBSCAN2 (B551): names only — `read_dir` read every row's inode inside ONE masked transaction
        for (name, _) in ents.iter().filter(|(n, _)| n.starts_with("application-x-vnd.")) {
            let obj = alloc::format!("{}/{}", TYPES_DIR, name);
            let decl: Vec<String> = str_attr(mt, &obj, midden_core::RES_KEY_DOCTYPES).unwrap_or_default().lines().map(String::from).collect();
            let types = uncovered_types(&decl);
            if types.is_empty() {
                continue;
            }
            let path = str_attr(mt, &obj, appres::KEY_APP_PATH).filter(|p| !crate::fs::apptrust::is_foreign(mt, p)).unwrap_or_default();
            out.push(SigDecl { sig: String::from(&name["application-x-vnd.".len()..]), path, types });
        }
    }
    out
}

/// Build the registry: `/system`, `/system/filetypes`, every known type's object that does not exist yet (one
/// bounded transaction PER OBJECT — REGISTRYCHUNK B508, was one for all), and on a present object every MISSING key (a present
/// key is the user's or an earlier build's and is never overwritten). `Ok((created, filled))`;
/// `Err(Unsupported)` on a root that takes no attributes (the registrants answer there).
pub fn seed_in(mt: &MountTable) -> Result<(usize, usize), VfsError> {
    if !root_takes_attrs(mt) {
        return Err(VfsError::Unsupported);
    }
    let k = KERNEL_PRINCIPAL;
    ensure_dirs(mt)?;
    // REGISTRYCHUNK (B508): PER OBJECT. Presence from ONE listing of the directory (no `stat` miss per type); each
    // missing type is its own one-object batch (one bounded transaction, one root flip), each present type its own
    // `set_attrs` — no 30-object masked hold. On x86 the caller's task yields between objects (interrupts and the
    // HID pump run between them). One line per object: `[filetypes] type=<mime> made= filled= blocks= cmds= ms= masked_ms=`.
    // JOBSCAN2 (B551): presence from the names alone (`read_dir_kinds`: the directory blob, no inode read per row —
    // `read_dir` read ~35 inodes inside one masked transaction before the first object).
    let present: Vec<String> = mt.read_dir_kinds(TYPES_DIR).map(|v| v.into_iter().map(|(n, _)| n).collect()).unwrap_or_default();
    let mut group = 1usize; // keys per stamping transaction, sized by the last one's masked span (`stamp_grouped`)
    let scoped = crate::fs::bootstep::scope_word() != "global";
    let sio = || if scoped { crate::fs::bootstep::scope_io() } else { crate::fs::bootstep::io() };
    let (mut made, mut filled) = (0usize, 0usize);
    let sigs = sig_decls(mt); // FILETYPES2 (B513): who declares the uncovered types (their objects name that program)
    for mime in seed_types_in(mt) { // SMALLFIX5 (B480): + the types only a ring-3 program's signature object declares
        let (t0, io0) = (crate::arch::ms(), sio());
        let obj = object_path(&mime);
        let leaf = String::from(&obj[TYPES_DIR.len() + 1..]);
        let exists = present.iter().any(|n| *n == leaf);
        let want = keys_for_obj(mt, &mime, exists, &sigs);
        let (mut m1, mut f1, mut txs, mut mm) = (0usize, 0usize, 0u32, 0u64);
        // FIRSTLOGIN (rmbp-ledger B636): flight 34 boot 2 `type=text/html made=1 blocks=344 masked_ms=183` — the boot's first
        // write to the volume read its cold directory and index blocks INSIDE the create's IRQ-masked UnaFS transaction
        // (every UnaFS op is one masked `MOUNT` hold, fs/unafs.rs `with_unafs_attempt`). The object's path is read first in
        // short read-only transactions of their own (a yield between), so the create and the stamp find those blocks
        // cached; each transaction kind's masked span is said on the per-type row (`masked_pre/create/stamp_max`).
        let (mut m_pre, mut m_create, mut m_stamp) = (crate::hidstall::scope_masked_take(), 0u64, 0u64); // keys_for_obj's reads count as pre
        if !exists {
            let _ = mt.stat(TYPES_DIR);
            breathe_between();
            let _ = mt.stat(&obj); // the miss walks the same lookup the create walks
            breathe_between();
            m_pre = m_pre.max(crate::hidstall::scope_masked_take());
            mm = mm.max(m_pre);
            // FILETYPES (JOBSCAN2, B551): the object is created BARE (one small transaction: the inode, the directory
            // rewrite, its name facts), then its keys are stamped in groups (`stamp_grouped`). Flight 32's 275 ms
            // masked hold was the one-object create carrying every key (each key two index B-tree inserts, cmds=62 in
            // one IRQ-masked transaction). The stamp is written only after every object, so a build cut between the
            // create and its keys is finished by the next build's fill arm below.
            match mt.create_files_batch(TYPES_DIR, alloc::vec![(leaf, Vec::new())], k) {
                Ok(n) => {
                    m1 = n;
                    txs += 1;
                    m_create = crate::hidstall::scope_masked_take();
                    mm = mm.max(m_create);
                    breathe_between();
                    let _ = mt.list_attrs(&obj, k); // FIRSTLOGIN (B636): the new inode's attribute blocks, read before the stamp
                    breathe_between();
                    m_pre = m_pre.max(crate::hidstall::scope_masked_take());
                    mm = mm.max(m_pre);
                    let keys: Vec<(String, Option<AttrValue>)> = want.into_iter().map(|(key, v)| (key, Some(v))).collect();
                    txs += stamp_grouped(mt, &obj, &keys, k, &mut group, &mut m_stamp)?;
                    mm = mm.max(m_stamp);
                }
                Err(VfsError::Unsupported) => { // no batch on this volume: the plain create, then its keys
                    mt.create(&obj, NodeKind::File, k)?;
                    for (key, v) in want {
                        mt.set_attr(&obj, &key, v, k)?;
                    }
                    m1 = 1;
                }
                Err(e) => return Err(e),
            }
        } else {
            let have: Vec<(String, AttrValue)> = mt.list_attrs(&obj, k).unwrap_or_default();
            breathe_between(); // FIRSTLOGIN (B636): the read is its own transaction; the stamp's groups follow
            m_pre = m_pre.max(crate::hidstall::scope_masked_take());
            mm = mm.max(m_pre);
            let fill: Vec<(String, Option<AttrValue>)> = want
                .into_iter()
                .filter(|(key, _)| !have.iter().any(|(h, v)| h == key && !matches!(v, AttrValue::Str(s) if s.is_empty())))
                .map(|(key, v)| (key, Some(v)))
                .collect();
            if !fill.is_empty() {
                f1 = fill.len();
                txs += stamp_grouped(mt, &obj, &fill, k, &mut group, &mut m_stamp)?; // JOBSCAN2 (B551): grouped like a new object's keys
                mm = mm.max(m_stamp);
            }
        }
        made += m1;
        filled += f1;
        let mm = mm.max(crate::hidstall::scope_masked_take()); // every object's span, said or not
        PER_TYPE_MASKED.fetch_max(mm, Ordering::Relaxed);
        if m1 + f1 > 0 {
            let d = sio().since(io0);
            serial_println!("[filetypes] type={} made={} filled={} blocks={} cmds={} ms={} masked_ms={} txs={} group={} masked_pre={} masked_create={} masked_stamp_max={}", mime, m1, f1, d.blocks_read(), d.cmds(),
                crate::arch::ms().saturating_sub(t0), mm, txs, group, m_pre, m_create, m_stamp); // FIRSTLOGIN (B636): each transaction kind's masked span
        }
        breathe_between(); // between objects, never inside a hold: the HID pump and the render lane run (JOBSCAN's worker runs this build)
    }
    Ok((made, filled))
}

/// JOBSCAN2 (B551): the most keys one stamping transaction carries.
const STAMP_GROUP_MAX: usize = 4;

/// Between two registry transactions: on x86, with interrupts on, the task yields (the HID pump and the render lane
/// run); never inside a hold.
fn breathe_between() {
    #[cfg(target_arch = "x86_64")]
    if !crate::arch::irqs_masked() {
        crate::arch::sched::yield_now();
    }
}

/// JOBSCAN2 (B551): stamp `keys` on `obj` in GROUPS — each group one `set_attrs` (one masked transaction, one
/// commit), a yield between groups. The group is sized by the last transaction's masked span: back to one key past
/// two thirds of [`MASKED_MS_BOUND`], one more key (to [`STAMP_GROUP_MAX`]) under a third of it. `mm` takes every
/// group's span; returns the transactions taken.
fn stamp_grouped(mt: &MountTable, obj: &str, keys: &[(String, Option<AttrValue>)], k: &str, group: &mut usize, mm: &mut u64) -> Result<u32, VfsError> {
    let mut rest = keys;
    let mut txs = 0u32;
    while !rest.is_empty() {
        let n = (*group).clamp(1, STAMP_GROUP_MAX).min(rest.len());
        *mm = (*mm).max(crate::hidstall::scope_masked_take()); // what came before this group stays the object's
        mt.set_attrs(obj, &rest[..n], k)?;
        let m = crate::hidstall::scope_masked_take();
        *mm = (*mm).max(m);
        txs += 1;
        if m * 3 > MASKED_MS_BOUND * 2 {
            *group = 1;
        } else if m * 3 <= MASKED_MS_BOUND && n == *group {
            *group = (*group + 1).min(STAMP_GROUP_MAX);
        }
        rest = &rest[n..];
        breathe_between();
    }
    Ok(txs)
}

/// `/system` and [`TYPES_DIR`], created when absent.
fn ensure_dirs(mt: &MountTable) -> Result<(), VfsError> {
    for d in ["/system", TYPES_DIR] {
        match mt.stat(d) {
            Ok(s) if matches!(s.kind, NodeKind::Dir) => {}
            Ok(_) => return Err(VfsError::NotADirectory),
            Err(_) => {
                mt.create(d, NodeKind::Dir, KERNEL_PRINCIPAL)?;
            }
        }
    }
    Ok(())
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

/// `login ok` (`login::close_into_session`, video/login.rs:761). FILETYPES2 (B513): the login BUILDS NOTHING — it
/// reads the registry's stamp (ONE attribute read on [`TYPES_DIR`]). A match publishes the stored registry
/// (`login=stamp`); a miss (absent, stale or invalidated) leaves the registry UNPUBLISHED — the resolver answers
/// from the facts, the extension table and the registrants — and owes the full build: on x86 with the compositor
/// the `jobs-scan` worker runs it after the bar ([`build_owed`]; the device-service pass when no worker took it,
/// still after the bar); elsewhere it builds inline (said `login=full`). One line:
/// `[filetypes] login=<stamp|none|full> stamp=<match|stale|absent> blocks_read= ms= owed=<no|after-bar|inline>`.
pub fn owe() {
    let (state, blocks, ms) = login_read(&crate::shell::vfs_mount_table());
    if state == "match" {
        REG.store(REG_LIVE, Ordering::Release);
        note_login(LOGIN_STAMP, ms, blocks);
        serial_println!("[filetypes] login=stamp stamp=match blocks_read={} ms={} owed=no", blocks, ms);
        return;
    }
    REG.store(REG_NONE, Ordering::Release);
    BUILD_STATE.store(BUILD_PENDING, Ordering::Release);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        note_login(LOGIN_NONE, ms, blocks);
        serial_println!("[filetypes] login=none stamp={} blocks_read={} ms={} owed=after-bar", state, blocks, ms);
        OWED.store(true, Ordering::Release);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        serial_println!("[filetypes] login=full stamp={} blocks_read={} ms={} owed=inline", state, blocks, ms);
        build_stamped("login", true);
    }
}

/// The login's read of the stamp: `(match|stale|absent, blocks_read, ms)` — ONE attribute read, nothing published.
/// Why THIS key: the stamp hashes everything `seed_in` derives an object from (the type facts, the extension table,
/// every built-in's resource block), and the one input outside the hash (a ring-3 program's own doc type) replaces
/// it when sighted unfilled (SMALLFIX5). The volume's commit generation moves on every commit (the login itself
/// writes the seat store and the home), so keyed on it the registry would rebuild on every boot; the apps folder's
/// count/mtime moves on every program install whether or not it declares a type, and SMALLFIX5 already covers the
/// install that does.
pub fn login_read(mt: &MountTable) -> (&'static str, u64, u64) {
    let (t0, io0) = (crate::arch::ms(), crate::fs::bootstep::io());
    let have = stamp_on_disk(mt);
    let want = stamp_now();
    let state = match &have { Some(h) if *h == want => "match", Some(_) => "stale", None => "absent" };
    (state, crate::fs::bootstep::io().since(io0).blocks_read(), crate::arch::ms().saturating_sub(t0))
}

/// The device-service pass: one relaxed load when nothing is owed. Builds only when the `jobs-scan` worker did
/// not take the build, and only after the bar.
pub fn service() {
    if OWED.load(Ordering::Acquire) && !TAKEN.load(Ordering::Acquire) && bar_first() && OWED.swap(false, Ordering::AcqRel) { // JOBSCAN (B497): the bar first; the jobs worker builds it when it took it
        build_stamped("service", false);
    }
}

/// Build once per boot unless a login already did (the verbs and fixtures ask before they read). FILETYPES2
/// (B513): once the login has read the stamp nothing here builds — the registry is published, or the worker's
/// build is owed (a verb never runs a second, concurrent build).
pub fn seed_once() -> usize {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) || builds() > 0 || REG.load(Ordering::Acquire) != REG_UNKNOWN {
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
        // FILETYPES2 (B513): ONE object, never the whole build (a click on the File Types pane is not the registry build).
        ensure_dirs(mt)?;
        let leaf = String::from(&obj[TYPES_DIR.len() + 1..]);
        if !matches!(mt.create_files_batch(TYPES_DIR, alloc::vec![(leaf, keys_for_obj(mt, mime, false, &[]))], KERNEL_PRINCIPAL), Ok(1)) && mt.stat(&obj).is_err() {
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
            let preferred = reg_attr(mt, &mime, PREFERRED_KEY).or_else(|| regs.first().map(|r| r.preferred_value())).unwrap_or_default();
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
/// `:: FILETYPES: types=<n> build_ms=<n> blocks=<n> masked_max_ms=<n> -> PASS|FAIL :: login=<stamp|none|full|-> worker=<none|pending|built|failed>
/// on=<worker|service|login|-> login_ms=<n> login_blocks=<n> bound_ms=20000 masked_bound_ms=50 bar_ms=<n|none> preferred=<n> resolved=<ok|why>
/// registry=<dir|none> objects=<n> source=<db|registrants> sticky=<ok|skip|why> amend=<ok|skip|why> openwith=<n> builds=<n>`
/// FILETYPES2 (B513): the head is the OWED build's cost (the worker's, after the bar; zeros on a stamp hit), read
/// BEFORE this test's own forced build (which is not recorded); [`verdict`] decides.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let k = KERNEL_PRINCIPAL;
    let attrs = root_takes_attrs(&mt);
    let (login, login_ms, login_blocks) = login_cost(); // FILETYPES2 (B513): the boot's record, before this test builds
    let (wstate, won, build_ms, blocks, masked) = build_cost();
    let cost_ok = verdict(login, wstate, won, build_ms, masked);
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
    let pass = why.is_none() && matches!(amend, "ok" | "skip") && matches!(sticky, "ok" | "skip") && openwith >= 2 && cost_ok; let word = if pass { "PASS" } else if !attrs && why.is_none() && openwith >= 2 { "SKIP reason=no-attr-root" } else { "FAIL" }; // QEMUCLASS1 (B570): a root that takes no attributes builds no registry (`build_stamped` :934 notes `failed` on enotsup), so the cost legs have no subject; the resolution legs still decide (ASSOCSTAMP's word)
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    let bar_ms = crate::video::desktopbuild::last_bar_ms();
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let bar_ms: Option<u64> = None;
    let bar = bar_ms.map_or(String::from("none"), |b| alloc::format!("{}", b));
    serial_println!(
        ":: FILETYPES: types={} build_ms={} blocks={} masked_max_ms={} -> {} :: login={} worker={} on={} login_ms={} login_blocks={} bound_ms={} masked_bound_ms={} bar_ms={} preferred={} resolved={} registry={} objects={} source={} sticky={} amend={} openwith={} builds={}",
        types.len(), build_ms, blocks, masked, word, login, wstate, won, login_ms, login_blocks, BUILD_MS_BOUND, MASKED_MS_BOUND, bar,
        preferred, resolved, if attrs { TYPES_DIR } else { "none" }, objects, if attrs { "db" } else { "registrants" }, sticky, amend, openwith, builds()
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
    let scoped = crate::fs::bootstep::scope_arm(); // REGISTRYCHUNK (B508): this core's I/O, not the card's (flight 27's 20776 was the jobs scan's too)
    let iow = if scoped { crate::fs::bootstep::scope_word() } else { String::from("global") };
    let sio = || if scoped { crate::fs::bootstep::scope_io() } else { crate::fs::bootstep::io() };
    let io0 = sio();
    let _ = crate::hidstall::scope_masked_take();
    PER_TYPE_MASKED.store(0, Ordering::Relaxed); // a `set_preferred_in` seed's leftover is not this build's
    let mut masked_max = 0u64;
    let mt = crate::shell::vfs_mount_table();
    BUILDS.fetch_add(1, Ordering::Relaxed);
    let want = stamp_now();
    let done = |masked_max: u64| { if scoped { crate::fs::bootstep::scope_disarm(); } masked_max.max(crate::hidstall::scope_masked_take()) };
    if !force && stamp_on_disk(&mt).as_deref() == Some(want.as_str()) {
        let d = sio().since(io0);
        let mm = done(masked_max);
        let ms = crate::arch::ms().saturating_sub(t0);
        note_cost(why, ms, d.blocks_read(), mm, false);
        if REG.swap(REG_LIVE, Ordering::AcqRel) == REG_NONE { // FILETYPES2 (B513): the stored registry is current — published
            serial_println!("[filetypes] swap registry=live on={} build_ms=0 skipped=stamp", why);
        }
        serial_println!("[filetypes] built at={} dir={} created=0 filled=0 types={} skipped=stamp stamp=v{} blocks_read={} cmds={} ms={} io={} masked_max_ms={}",
            why, TYPES_DIR, builtin_type_count(), BUILDER_VERSION, d.blocks_read(), d.cmds(), ms, iow, mm);
        return (0, true);
    }
    let r = seed_in(&mt);
    masked_max = masked_max.max(crate::hidstall::scope_masked_take());
    match r {
        Ok((n, f)) => {
            let _ = crate::fs::rootacl::stamp(&mt); // ROOTACL (B456): the system trees take the `system` owner once the registry is built
            let st = match mt.set_attr(TYPES_DIR, STAMP_KEY, AttrValue::Str(want), KERNEL_PRINCIPAL) {
                Ok(()) => "written",
                Err(_) => "refused",
            };
            let d = sio().since(io0);
            let mm = done(masked_max.max(PER_TYPE_MASKED.swap(0, Ordering::Relaxed)));
            let ms = crate::arch::ms().saturating_sub(t0);
            note_cost(why, ms, d.blocks_read(), mm, true);
            crate::fs::bootstep::note_span("filetypes-build", t0, crate::arch::ms()); // SMALLFIX6 (B495): the full build's span, beside the boot steps
            serial_println!("[filetypes] built at={} dir={} created={} filled={} types={} stamp={} blocks_read={} cmds={} ms={} io={} masked_max_ms={}", why, TYPES_DIR, n, f,
                known_types_in(&mt).len(), st, d.blocks_read(), d.cmds(), ms, iow, mm);
            if st == "written" {
                // FILETYPES2 (B513): THE SWAP — every object and the stamp are on the volume; ONE Release store publishes
                // the registry. Readers Acquire it in `reg_attr`: before it they answer from the facts, after it from the
                // registry — old or new, never a half-built directory.
                let was = REG.swap(REG_LIVE, Ordering::AcqRel);
                if was == REG_NONE {
                    serial_println!("[filetypes] swap registry=live on={} build_ms={} blocks_read={} masked_max_ms={}", why, ms, d.blocks_read(), mm);
                }
            }
            (n, false)
        }
        Err(VfsError::Unsupported) => { done(0); note_failed(why); serial_println!("[filetypes] built at={} dir=none reason=enotsup (root takes no attributes) source=registrants", why); (0, false) }
        Err(e) => { done(0); note_failed(why); serial_println!("[filetypes] built at={} FAILED ({}) source=registrants", why, crate::fs::attrsys::refusal(&e)); (0, false) }
    }
}

// ── REGISTRYCHUNK (rmbp-ledger B508) — the login build's measured cost, and the build off the critical path ─────
//
// Flight 27: `[filetypes] built at=login … blocks_read=20776 cmds=3599 ms=31154` then `[desktop] built … bar_ms=31205`.
// The build ran INLINE in the device-service pass that also builds the desktop (the bar waited), and its counters
// were the card's (the jobs scan shared the window). The build is now per object (`seed_in`: one bounded
// transaction per type, one `[filetypes] type=` line each) and counted on its own core. WHERE it runs is JOBSCAN's
// (B497: the bar first, then the `jobs-scan` worker's `build_owed`); this arc makes it cheap. `tests filetypes` reads
// the cost recorded here.

// FILETYPES2 (rmbp-ledger B513) — flight 28: `:: FILETYPES: types=30 build_ms=16134 … -> FAIL :: login=full`. The build
// had ALREADY left the login path (it ran on the jobs-scan worker after the bar: `[desktop] built … bar_ms=71` at
// 12:18:31, the worker's per-type lines 12:18:33–12:18:48) but `build_owed` called it `login` and `note_cost` filed
// every `login` build as the LOGIN's full build, held to REGISTRYCHUNK's 1000 ms bound. The card was a FIRST boot
// (`[users] seat … src=fresh users=0`, the installer form): no stamp, so the build was owed — not a stamp miss.
// Now the login's own act (a stamp read) and the owed build are two records.

const LOGIN_UNSET: usize = 0;
const LOGIN_FULL: usize = 1;
const LOGIN_STAMP: usize = 2;
const LOGIN_NONE: usize = 3;
static LOGIN_KIND: AtomicUsize = AtomicUsize::new(LOGIN_UNSET);
static LOGIN_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LOGIN_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The longest per-object masked span `seed_in` saw (taken by the summary line).
static PER_TYPE_MASKED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The registry's publication: UNKNOWN before any login read the stamp (the disk is read as it stands), NONE while
/// the login found no current stamp and the owed build has not swapped in, LIVE once the stamp matched or a full
/// build wrote it.
const REG_UNKNOWN: u8 = 0;
const REG_NONE: u8 = 1;
const REG_LIVE: u8 = 2;
static REG: AtomicU8 = AtomicU8::new(REG_UNKNOWN);

/// May readers take the registry's objects? `false` only between a stamp miss at login and the build's swap.
pub fn registry_readable() -> bool {
    REG.load(Ordering::Acquire) != REG_NONE
}

/// The owed build's state: none owed, pending, built, failed.
const BUILD_NONE: u8 = 0;
const BUILD_PENDING: u8 = 1;
const BUILD_DONE: u8 = 2;
const BUILD_FAILED: u8 = 3;
static BUILD_STATE: AtomicU8 = AtomicU8::new(BUILD_NONE);
/// Where the owed build ran: 0 none, 1 worker, 2 service, 3 login (inline).
static BUILD_ON: AtomicU8 = AtomicU8::new(0);
static BUILD_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static BUILD_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static BUILD_MASKED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn on_code(why: &str) -> u8 {
    match why { "worker" => 1, "service" => 2, "login" => 3, _ => 0 }
}

fn note_login(kind: usize, ms: u64, blocks: u64) {
    LOGIN_MS.store(ms, Ordering::Relaxed);
    LOGIN_BLOCKS.store(blocks, Ordering::Relaxed);
    LOGIN_KIND.store(kind, Ordering::Release);
}

/// Record the owed build's cost (`why` = `worker` · `service` · `login`; the `tests` and `verb` builds are not the
/// boot's). `full` = a full build ran (a stamp hit records nothing but the state). An inline `login` build is the
/// login's own full build.
fn note_cost(why: &str, ms: u64, blocks: u64, masked: u64, full: bool) {
    let on = on_code(why);
    if on == 0 {
        return;
    }
    if on == 3 && full {
        note_login(LOGIN_FULL, ms, blocks);
    }
    if full {
        BUILD_MS.store(ms, Ordering::Relaxed);
        BUILD_BLOCKS.store(blocks, Ordering::Relaxed);
        BUILD_MASKED.store(masked, Ordering::Relaxed);
    }
    BUILD_ON.store(on, Ordering::Relaxed);
    BUILD_STATE.store(BUILD_DONE, Ordering::Release);
}

fn note_failed(why: &str) {
    let on = on_code(why);
    if on != 0 {
        BUILD_ON.store(on, Ordering::Relaxed);
        BUILD_STATE.store(BUILD_FAILED, Ordering::Release);
    }
}

/// What the login did: `(full|stamp|none|-, ms, blocks)` (`-` = no login read the stamp this boot).
pub fn login_cost() -> (&'static str, u64, u64) {
    let k = match LOGIN_KIND.load(Ordering::Acquire) { LOGIN_FULL => "full", LOGIN_STAMP => "stamp", LOGIN_NONE => "none", _ => "-" };
    (k, LOGIN_MS.load(Ordering::Relaxed), LOGIN_BLOCKS.load(Ordering::Relaxed))
}

/// The owed build: `(none|pending|built|failed, on, ms, blocks, masked_max_ms)`.
pub fn build_cost() -> (&'static str, &'static str, u64, u64, u64) {
    let st = match BUILD_STATE.load(Ordering::Acquire) { BUILD_PENDING => "pending", BUILD_DONE => "built", BUILD_FAILED => "failed", _ => "none" };
    let on = match BUILD_ON.load(Ordering::Relaxed) { 1 => "worker", 2 => "service", 3 => "login", _ => "-" };
    (st, on, BUILD_MS.load(Ordering::Relaxed), BUILD_BLOCKS.load(Ordering::Relaxed), BUILD_MASKED.load(Ordering::Relaxed))
}

/// The arc's bounds: the owed build finishes under 20 s with no interrupts-off span over 50 ms.
pub const BUILD_MS_BOUND: u64 = 20_000;
pub const MASKED_MS_BOUND: u64 = 50;

/// FILETYPES2 (B513): PASS iff the login did no full build, and the owed build (when one was owed) finished off the
/// login under [`BUILD_MS_BOUND`] with masked_max_ms ≤ [`MASKED_MS_BOUND`]. A stamp hit owes nothing and passes.
pub fn verdict(login: &str, build: &str, on: &str, ms: u64, masked: u64) -> bool {
    login != "full"
        && match build {
            "none" => true,
            "built" => on != "login" && ms < BUILD_MS_BOUND && masked <= MASKED_MS_BOUND,
            _ => false,
        }
}

/// Does the recorded boot cost pass?
pub fn login_cost_ok() -> bool {
    let (login, _, _) = login_cost();
    let (b, on, ms, _, masked) = build_cost();
    verdict(login, b, on, ms, masked)
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
    // FILETYPES2 (B513): the LOGIN PATH over this volume, the boot's record untouched — a current stamp reads `match`
    // (the login publishes the stored registry and owes nothing); a stale one reads `stale`, the registry goes
    // unpublished (the resolver answers `registrant`, the empty-but-valid registry) until a build's one store swaps it
    // in (`db`), and the next login reads `match` again: the steady state is a stamp hit on every boot.
    let saved = REG.load(Ordering::Acquire);
    let (hit, hit_blocks, hit_ms) = login_read(&mt);
    let _ = mt.set_attr(TYPES_DIR, STAMP_KEY, AttrValue::Str(String::from("v0 h=stale n=0")), KERNEL_PRINCIPAL);
    let (miss, _, _) = login_read(&mt);
    REG.store(REG_NONE, Ordering::Release);
    let probe = object_path(ft::TEXT_PLAIN);
    let none_src = opener_for_in(&mt, &probe, ft::TEXT_PLAIN).1;
    let swapped = !build_stamped("tests", false).1 && registry_readable();
    let live_src = opener_for_in(&mt, &probe, ft::TEXT_PLAIN).1;
    let (again, _, _) = login_read(&mt);
    REG.store(saved, Ordering::Release); // the boot's publication as it was (a pending worker build then hits the stamp this wrote and publishes)
    let login_path = if hit != "match" {
        alloc::format!("hit:{}", hit)
    } else if miss != "stale" {
        alloc::format!("miss:{}", miss)
    } else if none_src != "registrant" {
        alloc::format!("unpublished:{}", none_src)
    } else if !swapped {
        String::from("no-swap")
    } else if live_src != "db" {
        alloc::format!("live:{}", live_src)
    } else if again != "match" {
        alloc::format!("next-login:{}", again)
    } else {
        String::from("ok")
    };
    let pass = state == "match" && stale == "rebuilt" && rehit == "ok" && ms <= 10 && login_path == "ok";
    serial_println!(":: ASSOCSTAMP: stamp={} blocks_read={} ms={} -> {} :: cmds={} stale={} rehit={} bound_ms=10 stamp=v{} n={} login_path={} login_hit_blocks={} login_hit_ms={} unpublished_src={} live_src={}",
        state, d.blocks_read(), ms, if pass { "PASS" } else { "FAIL" }, d.cmds(), stale, rehit, BUILDER_VERSION, builtin_type_count(),
        login_path, hit_blocks, hit_ms, none_src, live_src);
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
    if let Ok(ents) = mt.read_dir_kinds(TYPES_DIR) { // JOBSCAN2 (B551): names only — `read_dir` read every row's inode inside ONE masked transaction
        for (name, _) in ents.iter().filter(|(n, _)| n.starts_with("application-x-vnd.")) {
            let obj = alloc::format!("{}/{}", TYPES_DIR, name);
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
        build_stamped("worker", false); // FILETYPES2 (B513): the worker's build, recorded as the worker's (never the login's)
    }
    TAKEN.store(false, Ordering::Release);
}

// =================================================================================================
// STRIAMETAL (LEDGER SR139) — Stria first for its own media. CODEX §2 gives moving pictures and sound to Stria, and
// APPS/STRIA.ELF is Stria's own program (crates/user-stria); the kernel Player (`unaos/res/player`, video/player.rs)
// is its FULFILLER (vplay.rs CHARTER). Both declare the media types (STRIA.ELF in its APPRES block, res/app.res:
// video/webm, video/mp4, video/x-matroska, audio/wav, audio/mpeg, audio/flac, audio/ogg, audio/aac, audio/mp4,
// audio/aiff), and the registrant order puts every built-in first (`appres::registrants`), so a double-click on a
// .webm went to the Player. Under `stria` the program is moved to the front of the registrants of exactly the types
// it declares — once APPRES has sighted it (its launch, or a Quarry listing of /apps) — so the `registrant` answer of
// `opener_for_in` and the `una:preferred` a registry object is BUILT or FILLED with name it. A preference already
// stored on a type object is the operator's (or an earlier build's) and is never overwritten here: `assoc video/webm
// org.unaos.stria`, or the Settings File Types row, moves such a type.
// =================================================================================================

/// STRIAMETAL: STRIA.ELF's signature (crates/user-stria/res/app.res).
#[cfg(feature = "stria")]
const STRIA_SIG: &str = "org.unaos.stria";

/// STRIAMETAL: `regs` with Stria's registrant (when it declares this type) moved to the front, the rest in order.
#[cfg(feature = "stria")]
fn stria_first(mut regs: Vec<Registrant>) -> Vec<Registrant> {
    if let Some(i) = regs.iter().position(|r| r.signature == STRIA_SIG).filter(|&i| i > 0) {
        let r = regs.remove(i);
        regs.insert(0, r);
    }
    regs
}
