//! CHARTER: Kernel — fs-core
//!
//! ASSOC (FILETYPE M2, B307, audit B293) — the TYPE DATABASE: type → opener, as ATTRIBUTES.
//!
//! Each type is one system object `/system/types/<mime with '/' as '.'>` (`/system/types/image.png`)
//! carrying `una:opener` (an opener id — `facet` `fileview` `textedit` `play` `launch` `linux` `none`
//! — or a ring-3 program path such as `/apps/LUMEN.ELF`), `una:icon` (a glyph name) and `una:name`
//! ("PNG image"). They are ordinary attributes: `setfattr /system/types/text.plain una:opener=fileview`
//! changes the default opener with no code. A file's own `una:preferred` wins over its type's row
//! (`BEOS:PREFERRED_APP`).
//!
//! The defaults are [`BUILTIN`] — the table the deleted Quarry if-chain implied. They are written once
//! (per boot, idempotently: an object that already exists is never touched, so an operator's edit
//! survives every later boot) when the root takes attributes. On a FAT root there is nowhere to write
//! them, so [`BUILTIN`] answers directly and the source says `builtin`. No store of its own (R79).
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::filetype as ft;
use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};

pub const OPENER_KEY: &str = "una:opener";
pub const ICON_KEY: &str = "una:icon";
pub const NAME_KEY: &str = "una:name";
pub const PREFERRED_KEY: &str = "una:preferred";
/// The type database's directory on the root volume.
pub const TYPES_DIR: &str = "/system/types";

/// `(mime, opener, icon, name)` — the defaults. `none` is "no opener in this tree yet".
pub const BUILTIN: &[(&str, &str, &str, &str)] = &[
    (ft::TEXT_PLAIN, "textedit", "doc", "Plain text"),
    (ft::IMAGE_PNG, "facet", "image", "PNG image"),
    (ft::AUDIO_WAV, "play", "sound", "WAV audio"),
    (ft::UNAOS_ELF, "launch", "app", "UnaOS program"),
    (ft::UNAOS_BIN, "launch", "app", "UnaOS flat program"),
    (ft::LINUX_ELF, "linux", "app", "Linux program"),
    (ft::DIRECTORY, "quarry", "folder", "Folder"),
    (ft::GZIP, "none", "archive", "gzip archive"),
    (ft::TAR, "none", "archive", "tar archive"),
    (ft::OCTET, "none", "file", "Binary data"),
    // QUARRY2 (B336): the two text types with their own openers (the viewer, rendered), and GIF.
    (ft::TEXT_MARKDOWN, "markdown", "doc", "Markdown"),
    (ft::APP_JSON, "json", "doc", "JSON"),
    (ft::IMAGE_GIF, "facet", "image", "GIF image"),
];

/// The builtin row for `mime`, if any. Pure.
pub fn builtin(mime: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    BUILTIN.iter().find(|r| r.0 == mime)
}

/// `/system/types/image.png` for `image/png`. Pure.
pub fn object_path(mime: &str) -> String {
    let mut s = String::from(TYPES_DIR);
    s.push('/');
    for c in mime.chars() {
        s.push(if c == '/' { '.' } else { c });
    }
    s
}

fn str_attr(mt: &MountTable, path: &str, key: &str) -> Option<String> {
    match mt.get_attr(path, key, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// The opener for `path` of type `mime`, and the source that decided: `override` (the file's own
/// `una:preferred`), `db` (`/system/types/<mime>`'s `una:opener`) or `builtin` ([`BUILTIN`]; also the
/// answer on a FAT root, and for a type the database has no row for).
pub fn opener_for_in(mt: &MountTable, path: &str, mime: &str) -> (String, &'static str) {
    if let Some(o) = str_attr(mt, path, PREFERRED_KEY) {
        return (o, "override");
    }
    if let Some(o) = str_attr(mt, &object_path(mime), OPENER_KEY) {
        return (o, "db");
    }
    match builtin(mime) {
        Some(r) => (String::from(r.1), "builtin"),
        None => (String::from("none"), "builtin"),
    }
}

/// [`opener_for_in`] over a fresh mount table.
pub fn opener_for(path: &str, mime: &str) -> (String, &'static str) {
    opener_for_in(&crate::shell::vfs_mount_table(), path, mime)
}

/// The type's short name (`una:name` from the database, else the builtin row, else the MIME string).
pub fn name_of_in(mt: &MountTable, mime: &str) -> String {
    str_attr(mt, &object_path(mime), NAME_KEY)
        .or_else(|| builtin(mime).map(|r| String::from(r.3)))
        .unwrap_or_else(|| String::from(mime))
}

/// The type's icon glyph name, same order as [`name_of_in`].
pub fn icon_of_in(mt: &MountTable, mime: &str) -> String {
    str_attr(mt, &object_path(mime), ICON_KEY)
        .or_else(|| builtin(mime).map(|r| String::from(r.2)))
        .unwrap_or_else(|| String::from("file"))
}

/// Does the root volume take attributes (the database can live there)?
fn root_takes_attrs(mt: &MountTable) -> bool {
    !matches!(mt.list_attrs("/", KERNEL_PRINCIPAL), Err(VfsError::Unsupported) | Err(VfsError::NoSuchVolume) | Err(VfsError::NoSuchPath))
}

/// Write the defaults: `/system`, `/system/types`, and every [`BUILTIN`] object that does not exist
/// yet. `Ok(n)` = objects created this call (0 on every boot after the first); `Err(Unsupported)` on
/// a root that takes no attributes (the builtin table answers there).
pub fn seed_in(mt: &MountTable) -> Result<usize, VfsError> {
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
    let mut made = 0usize;
    for (mime, opener, icon, name) in BUILTIN.iter() {
        let obj = object_path(mime);
        if mt.stat(&obj).is_ok() {
            continue;
        }
        mt.create(&obj, NodeKind::File, k)?;
        mt.set_attr(&obj, OPENER_KEY, AttrValue::Str(String::from(*opener)), k)?;
        mt.set_attr(&obj, ICON_KEY, AttrValue::Str(String::from(*icon)), k)?;
        mt.set_attr(&obj, NAME_KEY, AttrValue::Str(String::from(*name)), k)?;
        made += 1;
    }
    Ok(made)
}

/// Seed once per boot, after the root volume answers (called from the users service's ready arm and
/// from the verbs/fixture, whichever runs first). One wire line either way.
pub fn seed_once() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    match seed_in(&mt) {
        Ok(n) => serial_println!("[assoc] seed dir={} created={} types={} source=db", TYPES_DIR, n, BUILTIN.len()),
        Err(VfsError::Unsupported) => serial_println!("[assoc] seed=skip reason=enotsup (root takes no attributes) source=builtin types={}", BUILTIN.len()),
        Err(e) => serial_println!("[assoc] seed=fail ({}) source=builtin", crate::fs::attrsys::refusal(&e)),
    }
}

/// `assoc` — every known type, its opener and the source. `assoc <mime>` — that one row.
/// `assoc <mime> <opener>` — set the type's opener in the database (an ordinary attribute write).
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    seed_once();
    let mt = crate::shell::vfs_mount_table();
    let row = |mime: &str| -> String {
        let (op, src) = opener_for_in(&mt, &object_path(mime), mime);
        // `object_path` as the "file" argument: the database object carries no `una:preferred`, so
        // this reads db-or-builtin, never an override.
        alloc::format!("{:<26} {:<12} {:<8} {} ({})", mime, op, src, name_of_in(&mt, mime), icon_of_in(&mt, mime))
    };
    match args {
        [] => {
            let mut mimes: Vec<String> = BUILTIN.iter().map(|r| String::from(r.0)).collect();
            if let Ok(ents) = mt.read_dir(TYPES_DIR) {
                for e in ents {
                    let m = e.name.replacen('.', "/", 1);
                    if !mimes.contains(&m) {
                        mimes.push(m);
                    }
                }
            }
            for m in &mimes {
                console.println(&row(m));
            }
            console.println(&alloc::format!("assoc: {} type(s)", mimes.len()));
        }
        [mime] => console.println(&row(mime)),
        [mime, opener, ..] => {
            let obj = object_path(mime);
            if !root_takes_attrs(&mt) {
                console.println(&alloc::format!("assoc: {}: the root carries no typed attributes, the builtin table is read-only (-ENOTSUP)", mime));
                return;
            }
            if mt.stat(&obj).is_err() {
                if let Err(e) = mt.create(&obj, NodeKind::File, KERNEL_PRINCIPAL) {
                    console.println(&alloc::format!("assoc: {}: {}", obj, crate::fs::attrsys::refusal(&e)));
                    return;
                }
            }
            match mt.set_attr(&obj, OPENER_KEY, AttrValue::Str(String::from(*opener)), KERNEL_PRINCIPAL) {
                Ok(()) => {
                    serial_println!("[assoc] set type={} opener={} obj={}", mime, opener, obj);
                    console.println(&row(mime));
                }
                Err(e) => console.println(&alloc::format!("assoc: {}: {}", obj, crate::fs::attrsys::refusal(&e))),
            }
        }
    }
}
