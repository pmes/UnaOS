//! CHARTER: Kernel — fs-core
//!
//! APPRES (rmbp-ledger B398, MACPARITY §16 B4 / row 38) — the REGISTRAR: a program's resources (name,
//! signature, version, kind, icon, document types) published as ATTRIBUTES, so the dock, Quarry and the
//! About box show a program's icon and name without running it — Be's registrar role.
//!
//! **The block** is the program's own: `tools/una-res` writes it into the stripped ELF as the non-alloc
//! note section `.note.unaos.res` (format and parse: `midden_core::res_records`). This module reads it
//! through the section table only — the ELF header, the section table, the note sections — never the
//! program's loadable bytes. The kernel's own windowed apps (quarry, settings, console, shell, facet) are not
//! ELFs: their blocks are the same format, packed by `una-res pack` from `unaos/res/<app>/` and compiled in.
//!
//! **The cache** is attributes, written at a program's FIRST SIGHT (its launch, or a Quarry listing of its
//! directory) and refreshed when its stamp (mtime + size) changes: on the program's own inode when its
//! volume takes attributes, and on its SIGNATURE object in the type database,
//! `/system/filetypes/application-x-vnd.<signature>` (FILETYPES B423: the registry; Be kept an app's icons on its MIME-database signature
//! entry exactly so), with `una:app.path` / `una:app.stamp`. A later sight finds that object by the
//! attribute query `una:app.path == "<path>"` and reads the attributes, not the ELF. The RAM table here is
//! the per-boot memo of what was read (decoded pixels included), not a store: nothing in it outlives a boot.
//!
//! **Doc types**: each MIME type a program declares gets the program's path in `una:apps` on its type object;
//! `assoc::opener_for_in` reads it after the registry's `una:preferred` (FILETYPES B423: [`registrants`]).
//!
//! A program without a block gets the generic icon (`unaos/res/generic`) and its file name.
//!
//! Wire: `[appres] sight path=<p> res=<yes|no> source=<elf|attrs> attrs=<n> on=<inode,types|types|none>`,
//! `[appres] about app=<a> name=<n> version=<v> signature=<s>`, and `tests appres` →
//! `:: APPRES: programs=<n> with_res=<n> icons_drawn=<n> cached_attrs=<n> -> PASS ::`.
//! Design: `docs/dev/evidence/rmbp-1005/appres.md`.
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, ATTR_VALUE_MAX, KERNEL_PRINCIPAL};
use midden_core as mc;

const _: () = assert!(una_abi::APP_RES_NOTE_TYPE == mc::APP_RES_NOTE_TYPE);

/// The kernel's own windowed apps and their packed blocks (`una-res pack unaos/res/<app>`).
const BUILTIN: &[(&str, &[u8])] = &[
    ("quarry", include_bytes!("../../../../res/quarry/quarry.unares")),
    ("settings", include_bytes!("../../../../res/settings/settings.unares")),
    ("console", include_bytes!("../../../../res/console/console.unares")),
    ("shell", include_bytes!("../../../../res/shell/shell.unares")),
    ("facet", include_bytes!("../../../../res/facet/facet.unares")),
    ("player", include_bytes!("../../../../res/player/player.unares")), // PLAYER (B419): the audio player window
    // FILETYPES (B423): every opener the dispatch runs is a REGISTRANT with resources — its doc types are its
    // own declaration, not a row in a table. The order is the registrant order (`registrants`): for a type two
    // declare, the earlier one is the default until the registry's `una:preferred` says otherwise.
    ("markdown", include_bytes!("../../../../res/markdown/markdown.unares")),
    ("json", include_bytes!("../../../../res/json/json.unares")),
    ("textedit", include_bytes!("../../../../res/textedit/textedit.unares")),
    ("fileview", include_bytes!("../../../../res/fileview/fileview.unares")),
    ("play", include_bytes!("../../../../res/play/play.unares")),
    ("launch", include_bytes!("../../../../res/launch/launch.unares")),
    ("linux", include_bytes!("../../../../res/linux/linux.unares")),
];
/// The icon a program without a block is drawn with.
const GENERIC: &[u8] = include_bytes!("../../../../res/generic/generic.unares");

/// Cache keys beside the `RES_KEY_*` ones.
pub const KEY_APP_PATH: &str = una_abi::attr_keys::APP_PATH;
pub const KEY_APP_STAMP: &str = una_abi::attr_keys::APP_STAMP;
/// On a type object: the programs that declare they open it (newline-separated paths).
pub const KEY_APPS: &str = una_abi::attr_keys::APPS;

/// Largest note section the registrar will read (an icon set is a few KiB; this bounds a hostile header).
const NOTE_CAP: usize = 256 * 1024;
/// Most section headers it will read.
const SHNUM_CAP: usize = 512;

/// One program's published facts.
#[derive(Clone)]
pub struct App {
    /// The lookup key: a built-in's id, or the program file's lower-case stem (`lumen`).
    pub key: String,
    /// `/apps/LUMEN.ELF`, or `builtin:<id>`.
    pub path: String,
    /// `<mtime>:<size>` of the file when it was read (`builtin` for the kernel's own).
    pub stamp: String,
    pub name: String,
    pub signature: String,
    pub version: String,
    pub kind: String,
    pub doctypes: Vec<String>,
    /// DROPTYPES (B477): the MIME types the program's windows take as a drop (`type/*`, `*/*`; empty = none).
    pub droptypes: Vec<String>,
    /// `(px, PNG)` — the program's own, or the generic set when `has_res` is false.
    pub icons: Vec<(u32, Vec<u8>)>,
    pub has_res: bool,
    /// `builtin` · `elf` (read from the program) · `attrs` (read from the cache).
    pub source: &'static str,
}

static REG: crate::sync::Mutex<Vec<App>> = crate::sync::Mutex::new(Vec::new());

/// A decoded, scaled icon: `argb` is `size * size` straight-alpha pixels (alpha in the top byte).
struct Pix {
    key: String,
    size: usize,
    argb: Vec<u32>,
}
static PIX: crate::sync::Mutex<Vec<Pix>> = crate::sync::Mutex::new(Vec::new());

static SIGHTS: AtomicUsize = AtomicUsize::new(0);
static DRAWN: AtomicUsize = AtomicUsize::new(0);

/// The program file's lower-case stem: `/apps/LUMEN.ELF` → `lumen`.
pub fn key_of_path(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = match base.rfind('.') {
        Some(0) | None => base,
        Some(i) => &base[..i],
    };
    stem.to_ascii_lowercase()
}

fn icons_of(block: &[u8]) -> Vec<(u32, Vec<u8>)> {
    mc::RES_ICON_SIZES.iter().filter_map(|(px, k)| mc::res_get(block, k).map(|v| (*px, v.to_vec()))).collect()
}

fn app_from_block(key: &str, path: &str, stamp: &str, block: &[u8], source: &'static str) -> App {
    let s = |k: &str| mc::res_str(block, k).map(String::from).unwrap_or_default();
    App {
        key: String::from(key),
        path: String::from(path),
        stamp: String::from(stamp),
        name: s(mc::RES_KEY_NAME),
        signature: s(mc::RES_KEY_SIGNATURE),
        version: s(mc::RES_KEY_VERSION),
        kind: s(mc::RES_KEY_KIND),
        doctypes: s(mc::RES_KEY_DOCTYPES).lines().map(String::from).filter(|l| !l.is_empty()).collect(),
        droptypes: s(mc::RES_KEY_DROPTYPES).lines().map(String::from).filter(|l| !l.is_empty()).collect(),
        icons: icons_of(block),
        has_res: true,
        source,
    }
}

/// A program with no block: the generic icon and its file name.
fn generic_app(key: &str, path: &str, stamp: &str, source: &'static str) -> App {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    App {
        key: String::from(key),
        path: String::from(path),
        stamp: String::from(stamp),
        name: String::from(leaf),
        signature: String::new(),
        version: String::new(),
        kind: String::new(),
        doctypes: Vec::new(),
        droptypes: Vec::new(),
        icons: icons_of(GENERIC),
        has_res: false,
        source,
    }
}

/// The kernel's own app `key`, from its compiled-in block. The version gets the kernel's build id.
pub fn builtin_app(key: &str) -> Option<App> {
    let (_, block) = BUILTIN.iter().find(|(k, _)| *k == key)?;
    let mut a = app_from_block(key, &alloc::format!("builtin:{}", key), "builtin", block, "builtin");
    if !a.version.contains('+') {
        if let Some(sha) = option_env!("UNAOS_GIT_SHA") {
            a.version = alloc::format!("{}+{}", a.version, sha);
        }
    }
    Some(a)
}

/// The resource block of the program at `path`, read through its section table (three small reads: the
/// header, the table, each note section). `Ok(None)` = an ELF with no block, or not an ELF.
pub fn read_block(mt: &MountTable, path: &str) -> Result<Option<Vec<u8>>, VfsError> {
    let head = mt.read(path, 0, 64)?;
    let Some((shoff, shnum)) = mc::elf_shdr_table(&head) else { return Ok(None) };
    if shnum > SHNUM_CAP {
        return Ok(None);
    }
    let table = mt.read(path, shoff as u64, shnum * 64)?;
    for (off, len) in mc::elf_note_sections(&table) {
        if len == 0 || len > NOTE_CAP {
            continue;
        }
        let area = mt.read(path, off as u64, len)?;
        if let Some(b) = mc::res_in_note_area(&area) {
            if mc::res_records(b).is_some() {
                return Ok(Some(b.to_vec()));
            }
        }
    }
    Ok(None)
}

/// The signature object in the registry: `/system/filetypes/application-x-vnd.org.unaos.lumen`.
pub fn signature_object(sig: &str) -> String {
    crate::fs::assoc::object_path(&alloc::format!("application/x-vnd.{}", sig))
}

fn get_str(mt: &MountTable, obj: &str, key: &str) -> Option<String> {
    match mt.get_attr(obj, key, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) => Some(s),
        _ => None,
    }
}

/// An app read back from the attributes on `obj` (the cache), when they are there and carry `stamp`.
fn from_attrs(mt: &MountTable, obj: &str, key: &str, path: &str, stamp: &str) -> Option<App> {
    if get_str(mt, obj, KEY_APP_STAMP).as_deref() != Some(stamp) || get_str(mt, obj, KEY_APP_PATH).as_deref() != Some(path) {
        return None;
    }
    let mut icons = Vec::new();
    for (px, k) in mc::RES_ICON_SIZES {
        if let Ok(AttrValue::Blob(b)) = mt.get_attr(obj, k, KERNEL_PRINCIPAL) {
            icons.push((px, b));
        }
    }
    let name = get_str(mt, obj, mc::RES_KEY_NAME)?;
    if icons.is_empty() {
        return None;
    }
    Some(App {
        key: String::from(key),
        path: String::from(path),
        stamp: String::from(stamp),
        name,
        signature: get_str(mt, obj, mc::RES_KEY_SIGNATURE).unwrap_or_default(),
        version: get_str(mt, obj, mc::RES_KEY_VERSION).unwrap_or_default(),
        kind: get_str(mt, obj, mc::RES_KEY_KIND).unwrap_or_default(),
        doctypes: get_str(mt, obj, mc::RES_KEY_DOCTYPES).unwrap_or_default().lines().map(String::from).filter(|l| !l.is_empty()).collect(),
        droptypes: get_str(mt, obj, mc::RES_KEY_DROPTYPES).unwrap_or_default().lines().map(String::from).filter(|l| !l.is_empty()).collect(),
        icons,
        has_res: true,
        source: "attrs",
    })
}

/// `app` as the attribute list the cache holds (every value inside the surface's size bound).
fn attr_list(app: &App) -> Vec<(String, AttrValue)> {
    let mut v: Vec<(String, AttrValue)> = Vec::new();
    let mut put = |k: &str, a: AttrValue| {
        if a.check().is_ok() {
            v.push((String::from(k), a));
        }
    };
    put(mc::RES_KEY_NAME, AttrValue::Str(app.name.clone()));
    put(mc::RES_KEY_SIGNATURE, AttrValue::Str(app.signature.clone()));
    put(mc::RES_KEY_VERSION, AttrValue::Str(app.version.clone()));
    put(mc::RES_KEY_KIND, AttrValue::Str(app.kind.clone()));
    if !app.doctypes.is_empty() {
        put(mc::RES_KEY_DOCTYPES, AttrValue::Str(app.doctypes.join("\n")));
    }
    if !app.droptypes.is_empty() {
        put(mc::RES_KEY_DROPTYPES, AttrValue::Str(app.droptypes.join("\n")));
    }
    for (px, png) in app.icons.iter() {
        if png.len() <= ATTR_VALUE_MAX {
            if let Some((_, k)) = mc::RES_ICON_SIZES.iter().find(|(p, _)| p == px) {
                put(k, AttrValue::Blob(png.clone()));
            }
        }
    }
    put(KEY_APP_PATH, AttrValue::Str(app.path.clone()));
    put(KEY_APP_STAMP, AttrValue::Str(app.stamp.clone()));
    v
}

/// Write `app` as attributes on the existing object `obj`, one by one (a refresh: the stamp changed).
/// Returns how many landed (`0` = the volume takes none — asked once, not per key).
fn write_attrs(mt: &MountTable, obj: &str, app: &App) -> usize {
    if matches!(mt.list_attrs(obj, KERNEL_PRINCIPAL), Err(_)) {
        return 0;
    }
    attr_list(app).into_iter().filter(|(k, v)| mt.set_attr(obj, k, v.clone(), KERNEL_PRINCIPAL).is_ok()).count()
}

/// Write the signature object: a refresh of an existing one key by key, or a NEW one created with all its
/// attributes in ONE transaction (`create_files_batch`: one root flip, BOOT80's shape). Returns how many.
fn write_signature(mt: &MountTable, obj: &str, app: &App) -> usize {
    if mt.stat(obj).is_ok() {
        return write_attrs(mt, obj, app);
    }
    let dir = crate::fs::assoc::TYPES_DIR;
    for d in ["/system", dir] {
        if mt.stat(d).is_err() && mt.create(d, NodeKind::Dir, KERNEL_PRINCIPAL).is_err() {
            return 0;
        }
    }
    let attrs = attr_list(app);
    let n = attrs.len();
    let leaf = String::from(&obj[dir.len() + 1..]);
    match mt.create_files_batch(dir, alloc::vec![(leaf, attrs)], KERNEL_PRINCIPAL) {
        Ok(1) => n,
        _ => 0,
    }
}

/// Publish `app`'s doc types: its path joins `una:apps` on each type object that exists.
fn publish_doctypes(mt: &MountTable, app: &App) {
    for mime in app.doctypes.iter() {
        let obj = crate::fs::assoc::object_path(mime);
        if mt.stat(&obj).is_err() {
            continue;
        }
        let cur = get_str(mt, &obj, KEY_APPS).unwrap_or_default();
        if cur.lines().any(|l| l == app.path) {
            continue;
        }
        let v = if cur.is_empty() { app.path.clone() } else { alloc::format!("{}\n{}", cur, app.path) };
        let _ = mt.set_attr(&obj, KEY_APPS, AttrValue::Str(v), KERNEL_PRINCIPAL);
    }
}

/// Cache `app` as attributes: the program's inode (when its volume takes them) and its signature object.
/// Returns (attributes written, where).
fn cache(mt: &MountTable, app: &App) -> (usize, &'static str) {
    let on_inode = if app.path.starts_with('/') { write_attrs(mt, &app.path, app) } else { 0 };
    let mut on_types = 0usize;
    if app.has_res && !app.signature.is_empty() {
        on_types = write_signature(mt, &signature_object(&app.signature), app);
        publish_doctypes(mt, app);
    }
    let on = match (on_inode > 0, on_types > 0) {
        (true, true) => "inode,types",
        (true, false) => "inode",
        (false, true) => "types",
        (false, false) => "none",
    };
    (on_inode + on_types, on)
}

fn remember(app: &App) {
    let mut r = REG.lock();
    r.retain(|a| a.path != app.path);
    r.push(app.clone());
    drop(r);
    PIX.lock().retain(|p| p.key != app.key);
}

/// The cached app for `path` from the attribute query, if its stamp is current.
fn from_cache(mt: &MountTable, key: &str, path: &str, stamp: &str) -> Option<App> {
    if let Some(a) = from_attrs(mt, path, key, path, stamp) {
        return Some(a);
    }
    let q = alloc::format!("{} == \"{}\"", KEY_APP_PATH, path);
    let hits = mt.query(&q, KERNEL_PRINCIPAL).ok()?;
    hits.iter().find_map(|(_, p)| from_attrs(mt, p, key, path, stamp))
}

/// **First sight** of the program at `path`: the memo if its stamp is unchanged, else the attribute cache,
/// else the program's own block (written to the cache). `None` when the path does not stat.
pub fn sight_in(mt: &MountTable, path: &str) -> Option<App> {
    let st = mt.stat(path).ok()?;
    if !matches!(st.kind, NodeKind::File) {
        return None;
    }
    let stamp = alloc::format!("{}:{}", st.mtime.unwrap_or(0), st.size);
    if let Some(a) = REG.lock().iter().find(|a| a.path == path && a.stamp == stamp) {
        return Some(a.clone());
    }
    let key = key_of_path(path);
    let (volume, root) = crate::fs::apptrust::trust_of(mt, path); // APPTRUST (B467): where it is decides what a sight may write
    serial_println!("[appres] sighted {} volume={} trust={}", path, volume, if root { "root" } else { "foreign" });
    if !root {
        sight_twin(mt, path, &key); // APPTRUST2 (B478): the root program of the same name is sighted first, so its key never draws the stick's icon
        let app = match read_block(mt, path) {
            Ok(Some(b)) => app_from_block(&key, path, &stamp, &b, SOURCE_FOREIGN),
            _ => generic_app(&key, path, &stamp, SOURCE_FOREIGN),
        };
        let _ = admit(mt, &app, &volume, false);
        SIGHTS.fetch_add(1, Ordering::Relaxed);
        remember(&app);
        return Some(app);
    }
    if let Some(a) = from_cache(mt, &key, path, &stamp) {
        SIGHTS.fetch_add(1, Ordering::Relaxed);
        serial_println!("[appres] sight path={} res=yes source=attrs attrs=0 on=cached sig={}", path, a.signature);
        stamp_check(mt, &a); // SMALLFIX5 (B480): a cached program outside the memo is a sighting too
        remember(&a);
        return Some(a);
    }
    let app = match read_block(mt, path) {
        Ok(Some(b)) => app_from_block(&key, path, &stamp, &b, "elf"),
        _ => generic_app(&key, path, &stamp, "elf"),
    };
    let (n, on) = admit(mt, &app, &volume, true);
    stamp_check(mt, &app); // SMALLFIX5 (B480): after `una:apps` is published on the type objects that exist
    SIGHTS.fetch_add(1, Ordering::Relaxed);
    serial_println!("[appres] sight path={} res={} source=elf attrs={} on={} sig={}", path,
        if app.has_res { "yes" } else { "no" }, n, on, if app.signature.is_empty() { "-" } else { &app.signature });
    remember(&app);
    Some(app)
}

/// [`sight_in`] over a fresh mount table (the launch hook).
pub fn sight(path: &str) {
    let _ = sight_in(&crate::shell::vfs_mount_table(), path);
}

/// The app a key names: a built-in, or a program already sighted (a root sight before a foreign one — APPTRUST2).
pub fn app(key: &str) -> Option<App> {
    builtin_app(key).or_else(|| by_key(&REG.lock(), key).cloned())
}

/// The app a sighted path names (no I/O).
pub fn app_at(path: &str) -> Option<App> {
    REG.lock().iter().find(|a| a.path == path).cloned()
}

/// The key a window title names: its first word, lower-cased, when that is a built-in or a sighted program.
pub fn key_of_title(title: &[u8]) -> Option<String> {
    let end = title.iter().position(|&c| c == b' ' || c == b':' || c == 0).unwrap_or(title.len());
    let w = core::str::from_utf8(&title[..end]).ok()?.to_ascii_lowercase();
    if w.is_empty() {
        return None;
    }
    if BUILTIN.iter().any(|(k, _)| *k == w) {
        return Some(w);
    }
    let r = REG.try_lock()?;
    r.iter().any(|a| a.key == w).then_some(w)
}

/// Area-average `src` (`sw` square, straight RGBA) down (or nearest up) to `size` square ARGB.
fn scale(rgba: &[u8], sw: usize, size: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(size * size);
    for y in 0..size {
        let (y0, y1) = (y * sw / size, ((y + 1) * sw / size).max(y * sw / size + 1).min(sw));
        for x in 0..size {
            let (x0, x1) = (x * sw / size, ((x + 1) * sw / size).max(x * sw / size + 1).min(sw));
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = &rgba[(yy * sw + xx) * 4..(yy * sw + xx) * 4 + 4];
                    let pa = p[3] as u32;
                    r += p[0] as u32 * pa;
                    g += p[1] as u32 * pa;
                    b += p[2] as u32 * pa;
                    a += pa;
                    n += 1;
                }
            }
            let px = if a == 0 { 0 } else { ((a / n) << 24) | ((r / a) << 16) | ((g / a) << 8) | (b / a) };
            out.push(px);
        }
    }
    out
}

/// Decode and scale `app`'s icon to `size` (the smallest rendering at least that big, else the largest).
fn render(app: &App, size: usize) -> Option<Vec<u32>> {
    let pick = app.icons.iter().find(|(px, _)| *px as usize >= size).or(app.icons.last())?;
    let im = pixel_core::decode_png(&pick.1).ok()?;
    if im.width != im.height || im.width == 0 || im.rgba.len() < (im.width * im.height * 4) as usize {
        return None;
    }
    Some(scale(&im.rgba, im.width as usize, size))
}

/// Blend straight-alpha `s` over `d` (XRGB).
#[inline]
fn over(s: u32, d: u32) -> u32 {
    let a = s >> 24;
    if a == 0 {
        return d;
    }
    if a == 255 {
        return s & 0x00FF_FFFF;
    }
    let ch = |sh: u32| (((s >> sh) & 0xFF) * a + ((d >> sh) & 0xFF) * (255 - a)) / 255;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Run `f` over the `size` icon of `key`, decoding it once. `None` when there is no such app, the icon will
/// not decode, or a lock is held elsewhere (a paint never waits: it draws its fallback this frame).
fn with_pix<R>(key: &str, size: usize, f: impl FnOnce(&[u32]) -> R) -> Option<R> {
    if size == 0 || size > 512 {
        return None;
    }
    {
        let p = PIX.try_lock()?;
        if let Some(px) = p.iter().find(|p| p.key == key && p.size == size) {
            return Some(f(&px.argb));
        }
    }
    let a = match builtin_app(key) {
        Some(a) => a,
        None => by_key(&REG.try_lock()?, key).cloned()?,
    };
    if a.path.starts_with('/') {
        serial_println!("[appres] icon-for name={} from={} path={} size={}", key, if a.source == SOURCE_FOREIGN { "foreign" } else { "root" }, a.path, size); // APPTRUST2 (B478): once per decode
    }
    let argb = render(&a, size)?;
    DRAWN.fetch_add(1, Ordering::Relaxed);
    let r = f(&argb);
    if let Some(mut p) = PIX.try_lock() {
        if p.len() >= 64 {
            p.remove(0);
        }
        p.push(Pix { key: String::from(key), size, argb });
    }
    Some(r)
}

/// **The dock tile's icon row.** Draws row `j` (strip-relative) of `key`'s icon, centred in the tile at
/// `(bx, by, tw, th)`, into the strip's scratch row `out`. `true` = the icon owns this tile (the caption is
/// not drawn); `false` = no icon for this tile, the caption draws as before.
pub fn dock_tile_row(out: &mut [u32], w: usize, key: &str, bx: usize, by: usize, tw: usize, th: usize, j: usize) -> bool {
    let size = (tw.min(th) * 4 / 5).max(8);
    let (x0, y0) = (bx + (tw - size.min(tw)) / 2, by + (th - size.min(th)) / 2);
    with_pix(key, size, |argb| {
        if j >= y0 && j < y0 + size {
            let row = &argb[(j - y0) * size..(j - y0 + 1) * size];
            for (i, &s) in row.iter().enumerate() {
                if let Some(d) = out.get_mut(x0 + i).filter(|_| x0 + i < w) {
                    *d = over(s, *d);
                }
            }
        }
    })
    .is_some()
}

/// The dock's entry: the key a tile names — the Facet window by its id, else the title's first word when
/// that is a built-in or a sighted program — then [`dock_tile_row`]. Allocation-free until an icon is first
/// decoded at a size; never waits on a lock.
pub fn dock_icon_row(out: &mut [u32], w: usize, id: u32, title: &[u8], bx: usize, by: usize, tw: usize, th: usize, j: usize) -> bool {
    #[cfg(feature = "facet")]
    if id != 0 && id == crate::video::facet::win() {
        return dock_tile_row(out, w, "facet", bx, by, tw, th, j);
    }
    let _ = id;
    let end = title.iter().position(|&c| c == b' ' || c == b':' || c == 0).unwrap_or(title.len()).min(16);
    let mut buf = [0u8; 16];
    for (d, s) in buf.iter_mut().zip(title[..end].iter()) {
        *d = s.to_ascii_lowercase();
    }
    let Ok(k) = core::str::from_utf8(&buf[..end]) else { return false };
    if k.is_empty() {
        return false;
    }
    let known = BUILTIN.iter().any(|(b, _)| *b == k) || REG.try_lock().map(|r| r.iter().any(|a| a.key == k)).unwrap_or(false);
    known && dock_tile_row(out, w, k, bx, by, tw, th, j)
}

/// Draw the icon of the program at `path` (sighted, else the generic one) at `(x, y)`, `size` square,
/// into a `stride`-wide surface of `h` rows. `false` when nothing was drawn.
pub fn blit_path_icon(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, size: usize, path: &str) -> bool {
    let key = match REG.try_lock() {
        Some(r) => r.iter().find(|a| a.path == path).map(|a| a.key.clone()),
        None => return false,
    };
    let Some(key) = key else { return false };
    with_pix(&key, size, |argb| {
        for r in 0..size {
            if y + r >= h {
                break;
            }
            for c in 0..size {
                if x + c >= stride {
                    break;
                }
                let d = &mut px[(y + r) * stride + x + c];
                *d = over(argb[r * size + c], *d);
            }
        }
    })
    .is_some()
}

/// **About <app>** — the FACTS only: `(name, version, signature)` for a window's app, with the
/// `[appres] about` witness. SMALLFIX4 (ARCHREVIEW F14): the registrar is fs-core and answers facts;
/// the About box itself is raised by `video::winmenu::about_box`, the app menu's own file.
pub fn about(win_name: &[u8]) -> (String, String, String) {
    let key = key_of_title(win_name).unwrap_or_else(|| String::from(core::str::from_utf8(win_name).unwrap_or("")).to_ascii_lowercase());
    let a = app(&key);
    let (name, version, sig) = match &a {
        Some(a) => (a.name.clone(), if a.version.is_empty() { String::from("-") } else { a.version.clone() }, if a.signature.is_empty() { String::from("-") } else { a.signature.clone() }),
        None => (String::from(core::str::from_utf8(win_name).unwrap_or("?")), String::from("-"), String::from("-")),
    };
    serial_println!("[appres] about app={} name={} version={} signature={} res={}", key, name, version, sig,
        if a.as_ref().map(|a| a.has_res).unwrap_or(false) { "yes" } else { "no" });
    (name, version, sig)
}

/// `tests appres` registration, once (rides `filetype::ensure_tests`).
pub fn ensure_tests() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("appres", selftest); crate::fs::apptrust::ensure_tests(); // APPTRUST (B467): `tests apptrust` rides it
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        { crate::video::launcher::ensure_tests(); crate::video::appswitch::ensure_tests(); } // APPSWITCH (B428): `tests appswitch` rides it too. LAUNCHER (B417): `tests launcher` rides this registration (no tests.rs line)
    }
}

/// `tests appres` — every program in `/apps` and every built-in: its block, its icon drawn into a scratch
/// tile, and its attributes cached where the root takes them.
///
/// PASS = the five built-ins and LUMEN.ELF (when staged) carry a block; every program's icon (its own or
/// the generic one) decodes and draws; and on a root that takes attributes, every program with a block has
/// its attributes on its signature object, stamp current.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let (mut programs, mut with_res, mut drawn, mut cached) = (0usize, 0usize, 0usize, 0usize);
    let mut ok = true;
    let root_attrs = mt.list_attrs("/", KERNEL_PRINCIPAL).is_ok();
    let mut apps: Vec<App> = BUILTIN.iter().filter_map(|(k, _)| builtin_app(k)).collect();
    let mut lumen_seen = false;
    if let Ok(rows) = mt.read_dir("/apps") {
        for r in rows {
            if !matches!(r.kind, NodeKind::File) || !r.name.to_ascii_uppercase().ends_with(".ELF") {
                continue;
            }
            let path = alloc::format!("/apps/{}", r.name);
            if let Some(a) = sight_in(&mt, &path) {
                if a.key == "lumen" {
                    lumen_seen = true;
                    ok &= a.has_res;
                }
                apps.push(a);
            }
        }
    }
    for a in apps.iter() {
        programs += 1;
        if a.has_res {
            with_res += 1;
        }
        // Draw into a scratch tile (64 px, the dock's size class) — the same path the dock paints with.
        let mut tile = alloc::vec![0x0030_3030u32; 64];
        let mut px_ok = false;
        for j in 0..64 {
            px_ok |= dock_tile_row(&mut tile, 64, &a.key, 0, 0, 64, 64, j) && tile.iter().any(|&p| p != 0x0030_3030);
        }
        if px_ok {
            drawn += 1;
        } else {
            ok = false;
        }
        let mut c = "-";
        if a.has_res && a.path.starts_with('/') && root_attrs {
            let obj = signature_object(&a.signature);
            if get_str(&mt, &obj, KEY_APP_STAMP).as_deref() == Some(a.stamp.as_str()) {
                cached += 1;
                c = "types";
            } else {
                ok = false;
                c = "MISSING";
            }
        }
        let icon = a.icons.first().map(|(px, _)| *px).unwrap_or(0);
        serial_println!(":: APPRES: {} sig={} version={} kind={} icon={} source={} drawn={} cache={} ::", a.key,
            if a.signature.is_empty() { "-" } else { &a.signature }, if a.version.is_empty() { "-" } else { &a.version },
            if a.kind.is_empty() { "-" } else { &a.kind }, if a.has_res { alloc::format!("{}px+", icon) } else { String::from("generic") },
            a.source, px_ok, c);
    }
    let builtin_ok = BUILTIN.iter().all(|(k, _)| builtin_app(k).map(|a| a.has_res && !a.icons.is_empty()).unwrap_or(false));
    ok &= builtin_ok;
    serial_println!(":: APPRES: programs={} with_res={} icons_drawn={} cached_attrs={} -> {} :: lumen={} root_attrs={} sights={} decodes={}",
        programs, with_res, drawn, cached, if ok { "PASS" } else { "FAIL" },
        if lumen_seen { "staged" } else { "absent" }, root_attrs, SIGHTS.load(Ordering::Relaxed), DRAWN.load(Ordering::Relaxed));
}


/// NOTIFY (B418): draw the icon of app `key` (a built-in or a sighted program) at `(x, y)`, `size` square, into a
/// `stride`-wide surface of `h` rows — the GENERIC icon when the key names no known program (a notification always
/// carries an icon). Never waits on a lock; `false` when nothing was drawn (a lock was held: the caller's fallback).
pub fn blit_key_icon(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, size: usize, key: &str) -> bool {
    let blit = |argb: &[u32], px: &mut [u32]| {
        for r in 0..size.min(h.saturating_sub(y)) {
            for c in 0..size.min(stride.saturating_sub(x)) {
                let d = &mut px[(y + r) * stride + x + c];
                *d = over(argb[r * size + c], *d);
            }
        }
    };
    if with_pix(key, size, |argb| blit(argb, px)).is_some() {
        return true;
    }
    if let Some(p) = PIX.try_lock() {
        if let Some(g) = p.iter().find(|p| p.key == "generic" && p.size == size) {
            blit(&g.argb, px);
            return true;
        }
    }
    let g = app_from_block("generic", "builtin:generic", "builtin", GENERIC, "builtin");
    let Some(argb) = render(&g, size) else { return false };
    blit(&argb, px);
    if let Some(mut p) = PIX.try_lock() {
        if p.len() >= 64 {
            p.remove(0);
        }
        p.push(Pix { key: String::from("generic"), size, argb });
    }
    true
}

// ── FILETYPES (rmbp-ledger B423) — the registrants of a type ─────────────────────────────────────────────────────────

/// One program that declares it opens a type: `opener` is what the dispatch runs (a built-in's key, or a ring-3
/// program's path), `signature` what the registry's `una:preferred` names (empty for a program without a block —
/// its path stands in), `name` what a menu shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Registrant {
    pub opener: String,
    pub signature: String,
    pub name: String,
}

impl Registrant {
    /// The value `una:preferred` stores for this registrant: its signature, else its path.
    pub fn preferred_value(&self) -> String {
        if self.signature.is_empty() { self.opener.clone() } else { self.signature.clone() }
    }
}

fn builtin_registrant(key: &str, block: &[u8]) -> Registrant {
    let s = |k: &str| mc::res_str(block, k).map(String::from).unwrap_or_default();
    Registrant { opener: String::from(key), signature: s(mc::RES_KEY_SIGNATURE), name: s(mc::RES_KEY_NAME) }
}

fn declares(block: &[u8], mime: &str) -> bool {
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

/// The registrants of `mime`: the built-ins that declare it (compiled-in resources, `BUILTIN` order), then the
/// ring-3 programs APPRES published on the type object (`una:apps` on `type_obj`, sighted order). No I/O beyond
/// that one attribute read; answers on a root that takes no attributes (the built-ins alone).
pub fn registrants(mt: &MountTable, mime: &str, type_obj: &str) -> Vec<Registrant> {
    let mut out: Vec<Registrant> = BUILTIN.iter().filter(|(_, b)| declares(b, mime)).map(|(k, b)| builtin_registrant(k, b)).collect();
    for p in get_str(mt, type_obj, KEY_APPS).unwrap_or_default().lines().filter(|l| !l.is_empty()) {
        if out.iter().any(|r| r.opener == p) || crate::fs::apptrust::is_foreign(mt, p) { // APPTRUST (B467): a foreign path is never a registrant, whoever wrote the line
            continue;
        }
        let (signature, name) = match app_at(p) {
            Some(a) => (a.signature, a.name),
            None => {
                let leaf = p.rsplit('/').next().unwrap_or(p);
                (String::new(), String::from(leaf))
            }
        };
        out.push(Registrant { opener: String::from(p), signature, name });
    }
    out
}

/// The opener a `una:preferred` value names: a built-in's signature → its key; a ring-3 program's signature → the
/// program's path (the memo, else its signature object's `una:app.path`); anything else (an opener id or a path, the
/// B307 per-file form) is returned as it is.
pub fn opener_of_preferred(mt: &MountTable, v: &str) -> String {
    if let Some((k, _)) = BUILTIN.iter().find(|(_, b)| mc::res_str(b, mc::RES_KEY_SIGNATURE) == Some(v)) {
        return String::from(*k);
    }
    if v.contains('.') && !v.starts_with('/') {
        if let Some(a) = REG.lock().iter().find(|a| a.signature == v && !crate::fs::apptrust::sighted_foreign(&a.path)) { // APPTRUST (B467): a foreign program's signature never names an opener
            return a.path.clone();
        }
        if let Some(p) = get_str(mt, &signature_object(v), KEY_APP_PATH) {
            return p;
        }
    }
    String::from(v)
}
/// LOGINWINDOW (B430): draw `key`'s icon (a built-in or sighted program) at `(x, y)`, `size` square, into a
/// `stride`-wide surface of `h` rows — ONLY when APPRES knows the key (no generic fallback: the caller draws its own,
/// the user's initials). Never waits on a lock. `false` = nothing drawn.
pub fn blit_icon_known(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, size: usize, key: &str) -> bool {
    with_pix(key, size, |argb| {
        for r in 0..size.min(h.saturating_sub(y)) {
            for c in 0..size.min(stride.saturating_sub(x)) {
                let d = &mut px[(y + r) * stride + x + c];
                *d = over(argb[r * size + c], *d);
            }
        }
    })
    .is_some()
}

/// LOGINWINDOW (B430): does APPRES know `key` (a built-in or a sighted program)? `false` when the registry is busy.
pub fn knows(key: &str) -> bool {
    builtin_app(key).is_some() || REG.try_lock().map(|r| r.iter().any(|a| a.key == key)).unwrap_or(false)
}

/// ASSOCSTAMP (rmbp-ledger B460): fold every built-in's key and packed resource block into the FNV-1a 64 state `h`
/// (the FILETYPES registry's generation stamp: a changed doc type, signature or name changes the hash). Pure.
pub fn builtin_hash(mut h: u64) -> u64 {
    for (k, b) in BUILTIN.iter() {
        for &x in k.as_bytes().iter().chain([0u8].iter()).chain(b.iter()) {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}
// ── APPTRUST (rmbp-ledger B467) — a sight from a foreign volume writes nothing and registers nothing ──────────────────

/// The one admission a sight makes: a ROOT program is cached (its inode, its signature object) and published in
/// `una:apps` on its doc types' objects — `(attributes written, where)`, as [`cache`]; a FOREIGN program writes
/// NOTHING (not on the stick, not in the registry) and joins `apptrust`'s RAM list — `(0, "foreign")`.
pub fn admit(mt: &MountTable, app: &App, volume: &str, root: bool) -> (usize, &'static str) {
    if !root {
        crate::fs::apptrust::note_foreign(&app.path, volume, &app.name, &app.signature, &app.doctypes);
        return (0, "foreign");
    }
    cache(mt, app)
}

/// `tests apptrust`'s probe: a program at `path` with a block declaring `mime` under `signature` (no icons).
pub fn probe_app(path: &str, signature: &str, mime: &str) -> App {
    App {
        key: key_of_path(path),
        path: String::from(path),
        stamp: String::from("probe"),
        name: String::from("AppTrust Probe"),
        signature: String::from(signature),
        version: String::from("1"),
        kind: String::from("app"),
        doctypes: alloc::vec![String::from(mime)],
        droptypes: Vec::new(),
        icons: Vec::new(),
        has_res: true,
        source: "elf",
    }
}

/// Drop `path` from the per-boot memo (the test's probe).
pub fn forget(path: &str) {
    REG.lock().retain(|a| a.path != path);
}

/// SMALLFIX4 item 11 (PREFSCAP fold) — the NAME a program launched by `path` answers to, from APPRES: its
/// signature's last dotted segment (`org.unaos.lumen` → `lumen`), else its declared name when that is one
/// token, else `None` (the launcher falls back to `wm::program_name(path)`). A FACT, no UI — the arming is
/// `wm::app_name_arm_launch`'s.
pub fn launch_name(path: &str) -> Option<String> {
    let a = app_at(path)?;
    let seg = a.signature.rsplit('.').next().unwrap_or("").trim();
    if !seg.is_empty() && seg.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
        return Some(seg.to_ascii_lowercase());
    }
    let n = a.name.trim();
    (!n.is_empty() && !n.contains(' ') && !n.contains('.')).then(|| n.to_ascii_lowercase())
}

// ── APPTRUST2 (rmbp-ledger B478) — a foreign sight never lends its icon to a root program's key ──────────────────────

/// `App.source` of a program sighted on a FOREIGN volume (`apptrust::trust_of`): listed, never cached or published.
pub const SOURCE_FOREIGN: &str = "foreign";

/// THE by-key pick: a root (or cached) sight of `key` before a foreign one, whatever order they were sighted in.
fn by_key<'a>(r: &'a [App], key: &str) -> Option<&'a App> {
    r.iter().find(|a| a.key == key && a.source != SOURCE_FOREIGN).or_else(|| r.iter().find(|a| a.key == key))
}

/// A foreign sight of `path`: when `/apps/<leaf>` exists and no root sight of `key` is in the memo, sight it first
/// (a root sight), so the key names the root program from the stick's first appearance.
fn sight_twin(mt: &MountTable, path: &str, key: &str) {
    let leafn = path.rsplit('/').next().unwrap_or(path);
    let twin = alloc::format!("{}/{}", crate::fs::apptrust::APPS_DIR, leafn);
    if twin == path || REG.lock().iter().any(|a| a.key == key && a.source != SOURCE_FOREIGN) {
        return;
    }
    if mt.stat(&twin).is_ok() && crate::fs::apptrust::is_root(mt, &twin) {
        let _ = sight_in(mt, &twin);
    }
}

/// `tests apptrust`'s icon leg: a foreign and a root probe of one key, remembered in both orders — `app(key)` must
/// name the root one each time. `"root"` or `"FOREIGN"`; leaves nothing in the memo.
pub fn icon_probe() -> &'static str {
    let (fp, rp) = ("/volumes/apptrust-probe/APTICON.ELF", "/apps/APTICON.ELF");
    let mut f = probe_app(fp, "org.unaos.apptrust-icon", "application/x-apptrust-icon");
    f.source = SOURCE_FOREIGN;
    let r = probe_app(rp, "org.unaos.apptrust-icon", "application/x-apptrust-icon");
    let key = f.key.clone();
    remember(&f);
    remember(&r);
    let a = app(&key).map(|a| a.path == rp).unwrap_or(false);
    forget(fp);
    forget(rp);
    remember(&r);
    remember(&f);
    let b = app(&key).map(|a| a.path == rp).unwrap_or(false);
    forget(fp);
    forget(rp);
    PIX.lock().retain(|p| p.key != key);
    if a && b { "root" } else { "FOREIGN" }
}

/// SMALLFIX5 (rmbp-ledger B480) item 1 — a ROOT program outside the per-boot memo whose block declares a type the
/// registry's stamp does not cover: `assoc::stamp_invalidate_for` decides (only a program `cache` publishes — a
/// resource block and a signature — can ever be filled, so no other sighting can invalidate every boot).
fn stamp_check(mt: &MountTable, app: &App) {
    if app.has_res && !app.signature.is_empty() && !app.doctypes.is_empty() {
        let _ = crate::fs::assoc::stamp_invalidate_for(mt, &app.path, &app.doctypes);
    }
}
