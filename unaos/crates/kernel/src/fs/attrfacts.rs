// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — fs-core
//!
//! ATTRCOLUMNS (rmbp-ledger B402) — the Be inheritance B2: a file's sniffed FACTS as typed attributes.
//!
//! On BeOS the sniffer that typed a file also wrote what it learned (`Audio:Length`, `Media:Width`) as typed
//! attributes, and Tracker showed them as columns. Here the facts come from the cores that own the formats
//! (`pixel_core::facts_of`, `audio_core::facts_of`, `demux_core::facts::facts_of` — shared-core, both rings) and
//! are written by this module beside `una:type`:
//!
//! * `media:width`, `media:height`, `media:duration_ms` (int), `media:codec` (string), `doc:title` (string, a
//!   Markdown H1, an HTML `<title>`, an ID3/Vorbis TITLE), `image:animated` (int 0/1: the attribute surface has
//!   no bool);
//! * `una:facts-mtime` — the file mtime the facts were taken at: a later mtime refreshes them, an equal one
//!   costs one attribute read (R80: written on Quarry's open and listing pass, never at boot);
//! * `una:attrtimes` — `key=unix-seconds` lines, the time each key last changed (Get Info's column).
//!
//! Everything for one file lands in ONE transaction (`MountTable::set_attrs`). A volume with no typed attributes
//! (FAT) answers `Unsupported` and gets nothing. The edit path ([`edit_in`]) and the folder view (`una:view`,
//! [`save_view`]) are the same one-transaction write. `tests attrcolumns` is [`selftest`].
//!
//! Design: `docs/dev/evidence/rmbp-1005/attrcolumns.md`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::filetype as ft;
use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};

pub const WIDTH: &str = una_abi::attr_keys::MEDIA_WIDTH;
pub const HEIGHT: &str = una_abi::attr_keys::MEDIA_HEIGHT;
pub const DURATION: &str = una_abi::attr_keys::MEDIA_DURATION_MS;
pub const CODEC: &str = una_abi::attr_keys::MEDIA_CODEC;
pub const TITLE: &str = una_abi::attr_keys::DOC_TITLE;
pub const ANIMATED: &str = una_abi::attr_keys::IMAGE_ANIMATED;
/// The sniffed facts, in the order Get Info and the witness print them.
pub const FACT_KEYS: [&str; 6] = [WIDTH, HEIGHT, DURATION, CODEC, TITLE, ANIMATED];
pub const FACTS_MTIME: &str = una_abi::attr_keys::FACTS_MTIME;
pub const TIMES: &str = una_abi::attr_keys::ATTRTIMES;
/// The folder's chosen attribute columns, comma-separated (Be's `_trk` attribute; the seed of FOLDERVIEW B6).
pub const VIEW: &str = una_abi::attr_keys::VIEW;

/// A file at most this long is read whole; a longer one gives its head and tail (header facts, Ogg end granule).
pub const READ_CAP: u64 = 4 << 20;
const HEAD_LONG: usize = 256 << 10;
const TAIL_LONG: usize = 64 << 10;

/// Keys a person does not choose as a column (bookkeeping and the ACL).
pub fn is_internal(key: &str) -> bool {
    key == FACTS_MTIME || key == TIMES || key == VIEW || key == "owner" || key.starts_with("grants:")
}

/// A Markdown document's first ATX H1 (`# Title`), within its first 64 lines.
fn md_title(b: &[u8]) -> Option<String> {
    let s = core::str::from_utf8(&b[..b.len().min(8192)]).ok().or_else(|| core::str::from_utf8(&b[..b.len().min(8192).saturating_sub(3)]).ok())?;
    for line in s.lines().take(64) {
        if let Some(t) = line.strip_prefix("# ") {
            let t = t.trim().trim_end_matches('#').trim();
            if !t.is_empty() {
                return Some(String::from(t));
            }
        }
    }
    None
}

/// An HTML document's `<title>…</title>` (ASCII case-insensitive tags).
fn html_title(b: &[u8]) -> Option<String> {
    let h = &b[..b.len().min(16384)];
    let lower: Vec<u8> = h.iter().map(|c| c.to_ascii_lowercase()).collect();
    let find = |pat: &[u8], from: usize| lower[from..].windows(pat.len()).position(|w| w == pat).map(|p| p + from);
    let open = find(b"<title", 0)?;
    let start = open + lower[open..].iter().position(|&c| c == b'>')? + 1;
    let end = find(b"</title", start)?;
    let t = String::from(String::from_utf8_lossy(&h[start..end]).trim());
    if t.is_empty() { None } else { Some(t) }
}

/// The facts for a file of type `mime`: `head` is its start, `len` its length, `tail` its end (both the whole
/// file when it was read whole). Pure.
pub fn facts_for(mime: &str, name: &str, head: &[u8], len: u64, tail: &[u8]) -> Vec<(&'static str, AttrValue)> {
    let mut out: Vec<(&'static str, AttrValue)> = Vec::new();
    let whole = head.len() as u64 == len;
    let image = |out: &mut Vec<(&'static str, AttrValue)>, f: pixel_core::ImageFacts| {
        out.push((WIDTH, AttrValue::Int(f.width as i64)));
        out.push((HEIGHT, AttrValue::Int(f.height as i64)));
        out.push((ANIMATED, AttrValue::Int(f.animated as i64)));
    };
    if mime == ft::AUDIO_MP4 || mime.starts_with("video/") {
        if whole {
            if let Some(f) = demux_core::facts::facts_of(head) {
                if f.video && f.width > 0 {
                    out.push((WIDTH, AttrValue::Int(f.width as i64)));
                    out.push((HEIGHT, AttrValue::Int(f.height as i64)));
                }
                out.push((DURATION, AttrValue::Int(f.duration_ms as i64)));
                out.push((CODEC, AttrValue::Str(f.codec)));
            }
        }
    } else if mime.starts_with("audio/") {
        if let Some(f) = audio_core::facts_of(head, len, tail) {
            if let Some(d) = f.duration_ms {
                out.push((DURATION, AttrValue::Int(d as i64)));
            }
            out.push((CODEC, AttrValue::Str(String::from(f.codec))));
            if let Some(t) = f.title {
                out.push((TITLE, AttrValue::Str(t)));
            }
        }
    } else if mime.starts_with("image/") {
        if let Some(f) = pixel_core::facts_of(head) {
            image(&mut out, f);
        }
    } else if mime == ft::TEXT_MARKDOWN {
        if let Some(t) = md_title(head) {
            out.push((TITLE, AttrValue::Str(t)));
        }
    } else if mime == "text/html" {
        if let Some(t) = html_title(head) {
            out.push((TITLE, AttrValue::Str(t)));
        }
    } else if mime.starts_with("text/") && (name.to_ascii_lowercase().ends_with(".svg")) {
        // An SVG typed as text (a build without the renderer): its size is still a fact of the markup.
        if let Some(f) = pixel_core::facts::svg_facts(head) {
            image(&mut out, f);
        }
    }
    out
}

/// What [`refresh_in`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// Facts written (how many fact keys), one transaction.
    Wrote(usize),
    /// `una:facts-mtime` equals the file's mtime: nothing read, nothing written.
    Fresh,
    /// The volume takes no typed attributes (FAT): nothing.
    NoAttrs,
    /// A directory or nothing at the path.
    NotFile,
    Failed(&'static str),
}

/// `key=secs` lines -> pairs.
pub fn times_of(s: &str) -> Vec<(String, u64)> {
    s.lines().filter_map(|l| l.split_once('=')).filter_map(|(k, v)| Some((String::from(k), v.trim().parse().ok()?))).collect()
}

fn times_with(old: &[(String, u64)], changed: &[&str], now: u64) -> String {
    let mut v: Vec<(String, u64)> = old.iter().filter(|(k, _)| !changed.iter().any(|c| c == k)).cloned().collect();
    for c in changed {
        v.push((String::from(*c), now));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::new();
    for (k, t) in v {
        s.push_str(&alloc::format!("{}={}\n", k, t));
    }
    s
}

fn now_s() -> u64 {
    crate::clock::unix_now().unwrap_or(0)
}

fn old_times(mt: &MountTable, path: &str) -> Vec<(String, u64)> {
    match mt.get_attr(path, TIMES, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) => times_of(&s),
        _ => Vec::new(),
    }
}

/// Read `path` for its facts: whole up to [`READ_CAP`], else head and tail.
fn read_for_facts(mt: &MountTable, path: &str, size: u64) -> Option<(Vec<u8>, Vec<u8>)> {
    if size <= READ_CAP {
        let b = mt.read(path, 0, size as usize).ok()?;
        return Some((b.clone(), b));
    }
    let head = mt.read(path, 0, HEAD_LONG).ok()?;
    let tail = mt.read(path, size - TAIL_LONG as u64, TAIL_LONG).ok()?;
    Some((head, tail))
}

/// Write `path`'s facts if they are absent or older than its mtime (`force`: always). See the module docs.
pub fn refresh_in(mt: &MountTable, path: &str, force: bool) -> Refresh {
    let Ok(st) = mt.stat(path) else { return Refresh::NotFile };
    if !matches!(st.kind, NodeKind::File) {
        return Refresh::NotFile;
    }
    let mtime = st.mtime.unwrap_or(0) as i64;
    match mt.get_attr(path, FACTS_MTIME, KERNEL_PRINCIPAL) {
        Err(VfsError::Unsupported) => return Refresh::NoAttrs,
        Ok(AttrValue::Int(m)) if m == mtime && !force => return Refresh::Fresh,
        _ => {}
    }
    let (mime, src) = ft::type_of_in(mt, path);
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((head, tail)) = read_for_facts(mt, path, st.size) else { return Refresh::Failed("read") };
    let facts = facts_for(&mime, name, &head, st.size, &tail);
    let old: Vec<(String, AttrValue)> = mt.list_attrs(path, KERNEL_PRINCIPAL).unwrap_or_default();
    let was = |k: &str| old.iter().find(|(o, _)| o == k).map(|(_, v)| v);
    let mut kv: Vec<(String, Option<AttrValue>)> = Vec::new();
    let mut changed: Vec<&str> = Vec::new();
    for k in FACT_KEYS {
        match facts.iter().find(|(f, _)| *f == k) {
            Some((_, v)) => {
                if was(k) != Some(v) {
                    changed.push(k);
                }
                kv.push((String::from(k), Some(v.clone())));
            }
            None if was(k).is_some() => {
                changed.push(k);
                kv.push((String::from(k), None));
            }
            None => {}
        }
    }
    if src != ft::Source::Attribute && mime != ft::OCTET {
        changed.push(ft::TYPE_KEY);
        kv.push((String::from(ft::TYPE_KEY), Some(AttrValue::Str(mime.clone()))));
    }
    kv.push((String::from(FACTS_MTIME), Some(AttrValue::Int(mtime))));
    if !changed.is_empty() {
        kv.push((String::from(TIMES), Some(AttrValue::Str(times_with(&old_times(mt, path), &changed, now_s())))));
    }
    match mt.set_attrs(path, &kv, KERNEL_PRINCIPAL) {
        Ok(()) => Refresh::Wrote(facts.len()),
        Err(VfsError::Unsupported) => Refresh::NoAttrs,
        Err(_) => Refresh::Failed("set_attrs"),
    }
}

/// [`refresh_in`] on the live mount table, with one line when something was written (Quarry's open path).
pub fn refresh(path: &str) -> Refresh {
    let mt = crate::shell::vfs_mount_table();
    let r = refresh_in(&mt, path, false);
    if let Refresh::Wrote(n) = r {
        serial_println!("[attrfacts] path={} facts={} (ATTRCOLUMNS)", path, n);
    }
    r
}

/// An edit in place: `key = value` and its change time, ONE transaction, as `principal` (the ACL decides). A
/// `una:type` must look like a MIME type (`major/minor`). Returns the volume's refusal unchanged.
pub fn edit_in(mt: &MountTable, path: &str, key: &str, value: AttrValue, principal: &str) -> Result<(), VfsError> {
    if is_internal(key) {
        return Err(VfsError::Denied);
    }
    if key == ft::TYPE_KEY {
        match &value {
            AttrValue::Str(m) if m.split_once('/').map_or(false, |(a, b)| !a.is_empty() && !b.is_empty() && !m.contains(' ')) => {}
            _ => return Err(VfsError::Backend("bad-mime")),
        }
    }
    let times = times_with(&old_times(mt, path), &[key], now_s());
    let kv = [(String::from(key), Some(value)), (String::from(TIMES), Some(AttrValue::Str(times)))];
    mt.set_attrs(path, &kv, principal)
}

/// The folder's chosen attribute columns (`una:view`), in order.
pub fn view_of(mt: &MountTable, dir: &str) -> Vec<String> {
    match mt.get_attr(dir, VIEW, KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) => s.split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect(),
        _ => Vec::new(),
    }
}

/// Remember the folder's attribute columns (an empty set removes `una:view`). The system's bookkeeping about the
/// folder, written as the kernel (Tracker wrote `_trk` the same way).
pub fn save_view(mt: &MountTable, dir: &str, cols: &[String]) -> Result<(), VfsError> {
    let v = if cols.is_empty() { None } else { Some(AttrValue::Str(cols.join(","))) };
    mt.set_attrs(dir, &[(String::from(VIEW), v)], KERNEL_PRINCIPAL)
}

/// `m:ss` (or `h:mm:ss`) for a length in milliseconds.
pub fn fmt_duration(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        alloc::format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        alloc::format!("{}:{:02}", s / 60, s % 60)
    }
}

/// An attribute value as a person reads it (a duration key as `m:ss`).
pub fn fmt_value(key: &str, v: &AttrValue) -> String {
    match v {
        AttrValue::Int(i) if key == DURATION => fmt_duration(*i),
        AttrValue::Int(i) if key == ANIMATED => String::from(if *i != 0 { "yes" } else { "no" }),
        AttrValue::Int(i) => alloc::format!("{}", i),
        AttrValue::Float(f) => alloc::format!("{:.3}", f),
        AttrValue::Str(s) => s.replace('\n', " "),
        AttrValue::Blob(b) => alloc::format!("<{} bytes>", b.len()),
        AttrValue::Vector(v) => alloc::format!("<{} floats>", v.len()),
    }
}

/// `YYYY-MM-DD HH:MM` (UTC) for unix seconds.
pub fn fmt_when(secs: u64) -> String {
    let (y, mo, d, h, mi, _) = crate::clock::civil_from_unix(secs);
    alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, mi)
}

/// One Get Info row: the key, its type word, the value as read, and when it last changed (when recorded).
#[derive(Clone, Debug)]
pub struct InfoRow {
    pub key: String,
    pub ty: &'static str,
    pub val: String,
    pub when: Option<u64>,
}

/// Every attribute of `path` (but the change-time ledger itself), sorted by key, with its change time.
pub fn info_rows(mt: &MountTable, path: &str) -> Result<Vec<InfoRow>, VfsError> {
    let all = mt.list_attrs(path, KERNEL_PRINCIPAL)?;
    let times = all.iter().find(|(k, _)| k == TIMES).and_then(|(_, v)| if let AttrValue::Str(s) = v { Some(times_of(s)) } else { None }).unwrap_or_default();
    Ok(all
        .into_iter()
        .filter(|(k, _)| k != TIMES)
        .map(|(k, v)| {
            let when = times.iter().find(|(t, _)| *t == k).map(|(_, s)| *s);
            InfoRow { ty: v.type_name(), val: fmt_value(&k, &v), when, key: k }
        })
        .collect())
}

// ── `tests attrcolumns` ─────────────────────────────────────────────────────────────────────────────

/// Registration, once (rides `filetype::ensure_tests`; no tests.rs line).
pub fn ensure_tests() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("attrcolumns", selftest);
    }
}

/// The test-f samples whose format carries no fact (plain text names no title).
const NO_FACTS: [&str; 3] = ["TEST.TXT", "TEST.JSON", "TEST.CSV"];

fn restore(mt: &MountTable, path: &str, key: &str, old: Option<AttrValue>) {
    let _ = mt.set_attrs(path, &[(String::from(key), old)], KERNEL_PRINCIPAL);
}

/// `tests attrcolumns` over `system/test-f` (24 files on the UnaFS root):
///
/// `:: ATTRCOLUMNS: typed_facts=<n>/24 columns_added=<n> inline_edit=ok view_saved=1 getinfo=ok -> PASS :: …`
///
/// 1 every sample's facts written (forced) and read back; 2 Quarry's attribute columns over the folder (offered,
/// typed cells, sorted by duration) — `skip` without Quarry; 3 `doc:title` edited and read back, `una:type`
/// edited and the opener re-routed, both restored; 4 `una:view` saved, read back, restored; 5 Get Info's rows for
/// TEST.FLAC carry `una:type` and `media:duration_ms` with a change time. R80: run only when asked.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let names = &crate::fs::volumes::TESTF_CLAIMED;
    let Some(dir) = crate::fs::volumes::TESTF_DIRS.iter().find(|d| mt.stat(d).is_ok()) else {
        serial_println!(":: ATTRCOLUMNS: typed_facts=0/{} -> SKIP :: reason=no-test-f ::", names.len());
        return;
    };
    if matches!(mt.list_attrs(dir, KERNEL_PRINCIPAL), Err(VfsError::Unsupported)) {
        serial_println!(":: ATTRCOLUMNS: typed_facts=0/{} -> SKIP :: reason=volume-has-no-attributes(FAT) dir={} ::", names.len(), dir);
        return;
    }
    let p = |n: &str| alloc::format!("{}/{}", dir, n);
    // 1 — facts.
    let (mut with, mut want, mut wrote) = (0u32, 0u32, 0u32);
    let mut missing: Vec<&str> = Vec::new();
    for n in names.iter() {
        let path = p(n);
        let r = refresh_in(&mt, &path, true);
        wrote += matches!(r, Refresh::Wrote(_)) as u32;
        let attrs = mt.list_attrs(&path, KERNEL_PRINCIPAL).unwrap_or_default();
        let mut line = String::new();
        for (k, v) in attrs.iter().filter(|(k, _)| FACT_KEYS.contains(&k.as_str())) {
            if !line.is_empty() {
                line.push(',');
            }
            line.push_str(&alloc::format!("{}={}", k, fmt_value(k, v)));
        }
        let expected = !NO_FACTS.contains(n);
        want += expected as u32;
        if !line.is_empty() {
            with += 1;
        } else if expected {
            missing.push(n);
        }
        serial_println!("[attrcolumns] {} r={:?} facts={}", n, r, if line.is_empty() { "-" } else { line.as_str() });
    }
    // 2 — Quarry's columns.
    #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let cols: Result<usize, String> = crate::video::quarry::live::attrcols::selftest_columns(&mt, dir);
    #[cfg(not(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))))]
    let cols: Result<usize, String> = Err(String::from("skip(no quarry in this build)"));
    // 3 — edit in place: doc:title, then una:type re-routing the opener.
    let md = p("TEST.MD");
    let old_title = mt.get_attr(&md, TITLE, KERNEL_PRINCIPAL).ok();
    let t_ok = edit_in(&mt, &md, TITLE, AttrValue::Str(String::from("ATTRCOLUMNS edit")), KERNEL_PRINCIPAL).is_ok()
        && mt.get_attr(&md, TITLE, KERNEL_PRINCIPAL).ok() == Some(AttrValue::Str(String::from("ATTRCOLUMNS edit")));
    restore(&mt, &md, TITLE, old_title);
    let txt = p("TEST.TXT");
    let old_type = mt.get_attr(&txt, ft::TYPE_KEY, KERNEL_PRINCIPAL).ok();
    let op0 = crate::fs::assoc::opener_for_in(&mt, &txt, &ft::type_of_in(&mt, &txt).0).0;
    let ty_set = edit_in(&mt, &txt, ft::TYPE_KEY, AttrValue::Str(String::from(ft::TEXT_MARKDOWN)), KERNEL_PRINCIPAL).is_ok();
    let op1 = crate::fs::assoc::opener_for_in(&mt, &txt, &ft::type_of_in(&mt, &txt).0).0;
    restore(&mt, &txt, ft::TYPE_KEY, old_type);
    let ty_ok = ty_set && op0 != op1;
    serial_println!("[attrcolumns] edit doc:title={} una:type={} opener {} -> {}", t_ok as u8, ty_set as u8, op0, op1);
    let edit_ok = t_ok && ty_ok;
    // 4 — the folder view.
    let old_view = mt.get_attr(dir, VIEW, KERNEL_PRINCIPAL).ok();
    let want_view = [String::from(DURATION), String::from(TITLE)];
    let view_ok = save_view(&mt, dir, &want_view).is_ok() && view_of(&mt, dir) == want_view;
    restore(&mt, dir, VIEW, old_view);
    // 5 — Get Info.
    let rows = info_rows(&mt, &p("TEST.FLAC")).unwrap_or_default();
    let gi = rows.iter().any(|r| r.key == ft::TYPE_KEY) && rows.iter().any(|r| r.key == DURATION && r.when.is_some());
    for r in rows.iter() {
        serial_println!("[attrcolumns] getinfo TEST.FLAC {} ({}) = {} changed={}", r.key, r.ty, r.val, r.when.map(fmt_when).unwrap_or_else(|| String::from("-")));
    }
    let facts_ok = missing.is_empty() && with == want;
    let cols_ok = matches!(cols, Ok(n) if n >= 2) || matches!(&cols, Err(e) if e.starts_with("skip"));
    let pass = facts_ok && cols_ok && edit_ok && view_ok && gi;
    serial_println!(
        ":: ATTRCOLUMNS: typed_facts={}/{} columns_added={} inline_edit={} view_saved={} getinfo={} -> {} :: wrote={} nofacts={}(plain text names no fact) missing={} dir={} ::",
        with,
        names.len(),
        match &cols { Ok(n) => alloc::format!("{}", n), Err(e) => e.clone() },
        if edit_ok { "ok" } else { "FAIL" },
        view_ok as u8,
        if gi { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" },
        wrote,
        NO_FACTS.join(","),
        if missing.is_empty() { String::from("-") } else { missing.join(",") },
        dir
    );
}
