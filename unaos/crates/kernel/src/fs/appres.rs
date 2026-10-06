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
//! `/system/types/application.x-vnd.<signature>` (Be kept an app's icons on its MIME-database signature
//! entry exactly so), with `una:app.path` / `una:app.stamp`. A later sight finds that object by the
//! attribute query `una:app.path == "<path>"` and reads the attributes, not the ELF. The RAM table here is
//! the per-boot memo of what was read (decoded pixels included), not a store: nothing in it outlives a boot.
//!
//! **Doc types**: each MIME type a program declares gets the program's path in `una:apps` on its type object;
//! `assoc::opener_for_in` reads it after the database's own `una:opener`.
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
];
/// The icon a program without a block is drawn with.
const GENERIC: &[u8] = include_bytes!("../../../../res/generic/generic.unares");

/// Cache keys beside the `RES_KEY_*` ones.
pub const KEY_APP_PATH: &str = "una:app.path";
pub const KEY_APP_STAMP: &str = "una:app.stamp";
/// On a type object: the programs that declare they open it (newline-separated paths).
pub const KEY_APPS: &str = "una:apps";

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
    /// `(px, PNG)` — the program's own, or the generic set when `has_res` is false.
    pub icons: Vec<(u32, Vec<u8>)>,
    pub has_res: bool,
    /// `builtin` · `elf` (read from the program) · `attrs` (read from the cache).
    pub source: &'static str,
}

static REG: spin::Mutex<Vec<App>> = spin::Mutex::new(Vec::new());

/// A decoded, scaled icon: `argb` is `size * size` straight-alpha pixels (alpha in the top byte).
struct Pix {
    key: String,
    size: usize,
    argb: Vec<u32>,
}
static PIX: spin::Mutex<Vec<Pix>> = spin::Mutex::new(Vec::new());

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

/// The signature object in the type database: `/system/types/application.x-vnd.org.unaos.lumen`.
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
    if let Some(a) = from_cache(mt, &key, path, &stamp) {
        SIGHTS.fetch_add(1, Ordering::Relaxed);
        serial_println!("[appres] sight path={} res=yes source=attrs attrs=0 on=cached sig={}", path, a.signature);
        remember(&a);
        return Some(a);
    }
    let app = match read_block(mt, path) {
        Ok(Some(b)) => app_from_block(&key, path, &stamp, &b, "elf"),
        _ => generic_app(&key, path, &stamp, "elf"),
    };
    let (n, on) = cache(mt, &app);
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

/// The app a key names: a built-in, or a program already sighted.
pub fn app(key: &str) -> Option<App> {
    builtin_app(key).or_else(|| REG.lock().iter().find(|a| a.key == key).cloned())
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
        None => REG.try_lock()?.iter().find(|a| a.key == key).cloned()?,
    };
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

/// **About <app>** — the app's name, version and signature on the wire and in a notice.
pub fn about(win_name: &[u8]) {
    let key = key_of_title(win_name).unwrap_or_else(|| String::from(core::str::from_utf8(win_name).unwrap_or("")).to_ascii_lowercase());
    let a = app(&key);
    let (name, version, sig) = match &a {
        Some(a) => (a.name.clone(), if a.version.is_empty() { String::from("-") } else { a.version.clone() }, if a.signature.is_empty() { String::from("-") } else { a.signature.clone() }),
        None => (String::from(core::str::from_utf8(win_name).unwrap_or("?")), String::from("-"), String::from("-")),
    };
    serial_println!("[appres] about app={} name={} version={} signature={} res={}", key, name, version, sig,
        if a.as_ref().map(|a| a.has_res).unwrap_or(false) { "yes" } else { "no" });
    #[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let title = alloc::format!("About {}", name);
        let text = alloc::format!("Version {}\n{}", version, sig);
        crate::video::crystal::login::notice_show(title.as_bytes(), text.as_bytes());
    }
}

/// `tests appres` registration, once (rides `filetype::ensure_tests`).
pub fn ensure_tests() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("appres", selftest);
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        crate::video::launcher::ensure_tests(); // LAUNCHER (B417): `tests launcher` rides this registration (no tests.rs line)
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


/// LAUNCHER (B417): draw the icon of the app `key` (a built-in or a sighted program) at `(x, y)`, `size`
/// square, into a `stride`-wide surface of `h` rows. `false` when nothing was drawn (never waits on a lock).
pub fn blit_key_icon(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, size: usize, key: &str) -> bool {
    with_pix(key, size, |argb| {
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
