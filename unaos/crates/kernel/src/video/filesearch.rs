//! CHARTER: Kernel — wm
//!
//! FILESEARCH (rmbp-ledger B741; BP-bar-14, BP-video-36, BP-storage-27; research C §8: finding a file is one
//! keystroke away, by name and by what it is, offline, instant) — **the Launcher's Files section.**
//!
//! * THE SOURCE — the volume's own name index (`UnaFS::find_names`, NAMEINDEX B432: ONE ordered-tree range scan,
//!   written with the name, NAMEINDEXTX B680) asked once per keystroke for [`ASK`] candidates, inside ONE
//!   `with_unafs` hold. A FAT root (no index) ranks the Launcher's open-time walk with the same rule.
//! * THE RANK — exact name (the leaf, or the leaf without its extension, equals the first word), then a leaf
//!   prefix, then a word prefix; every further word must name the leaf too. Within a rank, recently opened
//!   first ([`RECENT_MAX`] paths, newest first, kept by the ONE opener dispatch through [`note_opened`] — RAM,
//!   per session home), then the index's own order. At most [`ROWS_MAX`] rows.
//! * THE FILTERS — `kind:<photo|pdf|video|audio|text|folder>` (a prefix of the word is enough) and
//!   `taken:<yyyy>` read the facts already stored on the file (`una:type`, `media:taken`, ATTRCOLUMNS B402 /
//!   RAWCORE B444); a name hit is kept only when its facts agree. A query of filters alone asks the attribute
//!   catalog (`UnaFS::query`, the same store, R79) — no second index anywhere.
//! * THE ROW — Quarry's own type icon ([`icon`] -> `quarry::live::iconview::draw_icon`, FILESEARCH2 B745: ONE
//!   painter for a file's icon, the program's sighted icon else the folder / the page with its type tag), the
//!   leaf, its folder. Return opens it (the opener dispatch; a folder opens Quarry AT it), Shift-Return reveals
//!   it in Quarry (`quarry::live::open_at_select`: Quarry opened at its folder WITH the file selected).
//!
//! Wire: `[search] q=<n chars> hits=<n> ms=<n> us=<n> src=<una-name-index|attr-query|walk|none> cand=<n>` per
//! keystroke; `[search] reveal path=<p> folder=<f> -> shown=<0|1> selected=<0|1>`; `[search] open folder=<p> ->
//! shown=<0|1>`. `tests filesearch` (tests.rs) →
//! `:: FILESEARCH: index=10000 hits_ok=3 recent=ok kind=ok ms=<n> under=1 -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::search;

/// Rows the Files section shows at most.
pub const ROWS_MAX: usize = 20;
/// Paths the opener's recent list keeps (newest first).
pub const RECENT_MAX: usize = 32;
/// Candidates one keystroke asks of the name index (the rank cuts them to [`ROWS_MAX`]).
pub const ASK: usize = 64;
/// The per-keystroke budget the wire is read against (ms, on a 10k-name index).
pub const BUDGET_MS: u64 = 2;

const TYPE_KEY: &str = una_abi::attr_keys::TYPE;
const TAKEN_KEY: &str = una_abi::attr_keys::MEDIA_TAKEN;
const PDF: &str = "application/pdf";

// ── The query ───────────────────────────────────────────────────────────────────────────────────

/// What a file is, for `kind:`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Photo,
    Pdf,
    Video,
    Audio,
    Text,
    Folder,
    Other,
}

/// The `kind:` words, in the order a prefix is tried (`kind:p` is photo, `kind:pd` is pdf).
const KIND_WORDS: [(&str, FileKind); 12] = [
    ("photo", FileKind::Photo),
    ("pdf", FileKind::Pdf),
    ("picture", FileKind::Photo),
    ("image", FileKind::Photo),
    ("video", FileKind::Video),
    ("movie", FileKind::Video),
    ("audio", FileKind::Audio),
    ("music", FileKind::Audio),
    ("song", FileKind::Audio),
    ("text", FileKind::Text),
    ("folder", FileKind::Folder),
    ("directory", FileKind::Folder),
];

/// A typed query: the name words and the fact filters.
#[derive(Debug)]
pub struct Parsed<'a> {
    pub words: Vec<&'a str>,
    /// `kind:` — `Some(Other)` = a word no kind starts with (matches nothing, said on the wire as cand=0).
    pub kind: Option<FileKind>,
    /// `taken:<yyyy>` — only once four digits are typed.
    pub year: Option<i64>,
}

impl Parsed<'_> {
    pub fn filtered(&self) -> bool {
        self.kind.is_some() || self.year.is_some()
    }
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && !self.filtered()
    }
}

fn strip_ci<'a>(tok: &'a str, pre: &str) -> Option<&'a str> {
    (tok.len() >= pre.len() && tok.as_bytes()[..pre.len()].eq_ignore_ascii_case(pre.as_bytes())).then(|| &tok[pre.len()..])
}

/// Split `q` into name words and `kind:` / `taken:` filters. A filter still being typed (`kind:`, `taken:20`)
/// filters nothing yet.
pub fn parse(q: &str) -> Parsed<'_> {
    let mut p = Parsed { words: Vec::new(), kind: None, year: None };
    for tok in q.split_ascii_whitespace() {
        if let Some(v) = strip_ci(tok, "kind:") {
            if !v.is_empty() {
                let k = KIND_WORDS.iter().find(|(w, _)| strip_ci(w, v).is_some()).map(|(_, k)| *k);
                p.kind = Some(k.unwrap_or(FileKind::Other));
            }
        } else if let Some(v) = strip_ci(tok, "taken:") {
            if v.len() == 4 && v.bytes().all(|c| c.is_ascii_digit()) {
                p.year = v.parse::<i64>().ok();
            }
        } else {
            p.words.push(tok);
        }
    }
    p
}

/// The kind of a file from its type (the stored `una:type`, else the extension table) and its name.
pub fn kind_of(mime: &str, leaf: &str, dir: bool) -> FileKind {
    if dir {
        return FileKind::Folder;
    }
    if mime == PDF || type_core::ext_of(leaf).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return FileKind::Pdf;
    }
    match mime.split('/').next().unwrap_or("") {
        "image" => FileKind::Photo,
        "video" => FileKind::Video,
        "audio" => FileKind::Audio,
        "text" => FileKind::Text,
        _ => FileKind::Other,
    }
}

/// Days from 1970-01-01 to the civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// The first second of `year` (UTC, unix seconds).
pub fn year_start(year: i64) -> i64 {
    days_from_civil(year, 1, 1) * 86400
}

/// The year a unix second falls in (UTC).
pub fn year_of(secs: i64) -> i64 {
    let mut y = 1970 + secs.div_euclid(31_556_952);
    while year_start(y) > secs {
        y -= 1;
    }
    while year_start(y + 1) <= secs {
        y += 1;
    }
    y
}

/// The rank of `leaf` for `words`: 0 = exact (the leaf or its stem equals the first word), 1 = a leaf prefix,
/// 2 = a word prefix; `None` when a word does not name it. The first word decides; the rest must also match.
pub fn tier(leaf: &str, words: &[&str]) -> Option<u8> {
    let first = words.first()?;
    if words[1..].iter().any(|w| search::name_match(leaf, w).is_none()) {
        return None;
    }
    let stem = match leaf.rfind('.') {
        Some(i) if i > 0 => &leaf[..i],
        _ => leaf,
    };
    if leaf.eq_ignore_ascii_case(first) || stem.eq_ignore_ascii_case(first) {
        return Some(0);
    }
    match search::name_match(leaf, first)? {
        2 => Some(1),
        _ => Some(2),
    }
}

// ── The rows ────────────────────────────────────────────────────────────────────────────────────

/// One row of the Files section.
#[derive(Clone, Debug)]
pub struct Row {
    pub path: String,
    pub dir: bool,
    /// `kind:`'s input: on a filtered query the stored `una:type`, else the extension table's, else empty; on a
    /// name-only query the extension table's (FILESEARCH2: the icon reads the path, so no per-row attribute read).
    pub mime: String,
    /// 0 exact · 1 prefix · 2 word · 3 filters only.
    pub tier: u8,
}

impl Row {
    pub fn leaf(&self) -> &str {
        search::leaf(&self.path)
    }
    /// The folder the row lives in (`/` for a top-level name).
    pub fn folder(&self) -> &str {
        match self.path.rfind('/') {
            Some(0) | None => "/",
            Some(i) => &self.path[..i],
        }
    }
}

/// THE recent list's one rule: `path` to the front, once, at most [`RECENT_MAX`].
pub fn push_recent(list: &mut Vec<String>, path: &str) {
    list.retain(|p| p != path);
    list.insert(0, String::from(path));
    list.truncate(RECENT_MAX);
}

/// Order candidates `(row, index order)`: rank, then recently opened, then the source's own order; cut to
/// [`ROWS_MAX`].
pub fn order(mut c: Vec<(Row, usize)>, recent: &[String]) -> Vec<Row> {
    let pos = |p: &str| recent.iter().position(|r| r == p).unwrap_or(usize::MAX);
    c.sort_by(|a, b| a.0.tier.cmp(&b.0.tier).then(pos(&a.0.path).cmp(&pos(&b.0.path))).then(a.1.cmp(&b.1)));
    c.truncate(ROWS_MAX);
    c.into_iter().map(|x| x.0).collect()
}

/// What one keystroke's engine found.
pub struct Answer {
    pub rows: Vec<Row>,
    /// Candidates the source gave before the filters and the cut.
    pub cand: usize,
    pub src: &'static str,
}

#[cfg(feature = "unafs")]
/// The core attribute query a filters-only query asks (`None`: a kind the stored type cannot name).
fn attr_expr(p: &Parsed) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let range = |major: &str| alloc::format!("{k} >= \"{m}/\" AND {k} < \"{m}0\"", k = TYPE_KEY, m = major);
    match p.kind {
        Some(FileKind::Photo) => parts.push(range("image")),
        Some(FileKind::Video) => parts.push(range("video")),
        Some(FileKind::Audio) => parts.push(range("audio")),
        Some(FileKind::Text) => parts.push(range("text")),
        Some(FileKind::Pdf) => parts.push(alloc::format!("{} == \"{}\"", TYPE_KEY, PDF)),
        Some(FileKind::Folder) | Some(FileKind::Other) => return None,
        None => {}
    }
    if let Some(y) = p.year {
        parts.push(alloc::format!("{} >= {} AND {} < {}", TAKEN_KEY, year_start(y), TAKEN_KEY, year_start(y + 1)));
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// THE ENGINE, over any UnaFS volume (the root's in the Launcher, a RAM volume in `tests filesearch`): the name
/// index's one range scan, the filters on the stored facts, the rank. `Ok(None)` = the volume has no ready name
/// index (the caller walks).
#[cfg(feature = "unafs")]
pub fn answer<D: ::unafs::BlockDevice>(fs: &mut ::unafs::UnaFS<D>, p: &Parsed, recent: &[String]) -> Result<Option<Answer>, ::unafs::fs::FileSystemError> {
    use ::unafs::AttributeValue as V;
    let facts = |fs: &mut ::unafs::UnaFS<D>, id: u64, want_taken: bool| -> Result<(String, Option<i64>), ::unafs::fs::FileSystemError> {
        let mime = match fs.get_attribute(id, TYPE_KEY)? {
            Some(V::String(s)) => s,
            _ => String::new(),
        };
        let taken = if want_taken {
            match fs.get_attribute(id, TAKEN_KEY)? {
                Some(V::Int(t)) => Some(t),
                _ => None,
            }
        } else {
            None
        };
        Ok((mime, taken))
    };
    let mut c: Vec<(Row, usize)> = Vec::new();
    if p.words.is_empty() {
        if !p.filtered() {
            return Ok(Some(Answer { rows: Vec::new(), cand: 0, src: "none" }));
        }
        let Some(expr) = attr_expr(p) else {
            return Ok(Some(Answer { rows: Vec::new(), cand: 0, src: "attr-query" }));
        };
        let hits = match fs.query(&expr) {
            Ok(h) => h,
            Err(::unafs::fs::FileSystemError::Query(_)) => Vec::new(),
            Err(e) => return Err(e),
        };
        let cand = hits.len();
        // Recent first, then creation order: only the recent ones and the first ROWS_MAX others can show.
        let mut keep: Vec<(usize, ::unafs::QueryHit)> = Vec::new();
        for (i, h) in hits.into_iter().enumerate() {
            if h.path.is_empty() {
                continue;
            }
            if keep.len() < ROWS_MAX || recent.iter().any(|r| *r == h.path) {
                keep.push((i, h));
            }
        }
        for (i, h) in keep {
            let (mime, _) = facts(fs, h.inode_id, false)?;
            c.push((Row { path: h.path, dir: false, mime, tier: 3 }, i));
        }
        return Ok(Some(Answer { rows: order(c, recent), cand, src: "attr-query" }));
    }
    let f = fs.find_names(p.words[0], ASK)?;
    if !f.indexed {
        return Ok(None);
    }
    let cand = f.hits.len();
    for (i, h) in f.hits.into_iter().enumerate() {
        let Some(t) = tier(search::leaf(&h.path), &p.words) else { continue };
        let mut row = Row { path: h.path, dir: h.dir, mime: String::new(), tier: t };
        if p.filtered() {
            let id = fs.resolve_path(&row.path)?;
            let (mime, taken) = facts(fs, id, p.year.is_some())?;
            row.mime = mime;
            if let Some(k) = p.kind {
                let m = if row.mime.is_empty() { type_core::by_extension(row.leaf()).unwrap_or("") } else { row.mime.as_str() };
                if kind_of(m, row.leaf(), row.dir) != k {
                    continue;
                }
            }
            if let Some(y) = p.year {
                if taken.map(year_of) != Some(y) {
                    continue;
                }
            }
        }
        c.push((row, i));
    }
    // FILESEARCH2 (B745): no per-row attribute read on a name-only query — the row's icon is Quarry's painter, which
    // reads the PATH (the program's icon, else the page and its type tag); the keystroke costs the one range scan.
    let mut rows = order(c, recent);
    for r in rows.iter_mut().filter(|r| !r.dir && r.mime.is_empty()) {
        r.mime = String::from(type_core::by_extension(r.leaf()).unwrap_or(""));
    }
    Ok(Some(Answer { rows, cand, src: ::unafs::NAME_SRC }))
}

/// The same rank over the Launcher's walk (a root with no name index): names only, `kind:` by the extension
/// table, `taken:` unanswerable (no stored facts on such a volume: it filters everything out, honestly).
pub fn answer_walk(hits: &[search::Hit], p: &Parsed, recent: &[String]) -> Answer {
    let mut c: Vec<(Row, usize)> = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let leaf = search::leaf(&h.path);
        let t = if p.words.is_empty() { Some(3) } else { tier(leaf, &p.words) };
        let Some(t) = t else { continue };
        let mime = if h.dir { "" } else { type_core::by_extension(leaf).unwrap_or("") };
        if p.kind.is_some_and(|k| kind_of(mime, leaf, h.dir) != k) || p.year.is_some() {
            continue;
        }
        c.push((Row { path: h.path.clone(), dir: h.dir, mime: String::from(mime), tier: t }, i));
    }
    Answer { rows: order(c, recent), cand: hits.len(), src: "walk" }
}

// ── The recent list (the opener's) ──────────────────────────────────────────────────────────────

/// `(session home, paths newest first)` — a new home starts an empty list.
static RECENT: crate::sync::Mutex<(String, Vec<String>)> = crate::sync::Mutex::new((String::new(), Vec::new()));

fn home_now() -> String {
    crate::prefs::home().unwrap_or_default()
}

/// **The opener's one call** (`quarry::live::openers::open`, every file the dispatch opens): `path` becomes the
/// newest of the session's [`RECENT_MAX`] recently opened.
pub fn note_opened(path: &str) {
    if !path.starts_with('/') || path.contains('\n') {
        return;
    }
    let home = home_now();
    let mut g = RECENT.lock();
    if g.0 != home {
        g.0 = home;
        g.1.clear();
    }
    push_recent(&mut g.1, path);
}

/// The session's recent list, newest first.
pub fn recent() -> Vec<String> {
    let home = home_now();
    let g = RECENT.lock();
    if g.0 != home { Vec::new() } else { g.1.clone() }
}

// ── The live section (the Launcher's pass) ──────────────────────────────────────────────────────

#[cfg(feature = "unafs")]
fn native_root() -> bool {
    crate::shell::vfs_mount_table().volume_name("/").map(|n| n == "native").unwrap_or(false)
}

/// **The Files section** for query `q` — asked by the Launcher's rank on its service pass, never the router.
/// `walk` is the open-time walk (empty on an indexed root). Prints the `[search]` line.
pub fn section(q: &str, walk: &[search::Hit]) -> Vec<Row> {
    let p = parse(q);
    if p.is_empty() {
        return Vec::new();
    }
    let t0 = crate::hidstall::now_us();
    let recent = recent();
    #[allow(unused_mut)]
    let mut got: Option<Answer> = None;
    #[cfg(feature = "unafs")]
    if native_root() {
        if let Ok(Ok(Some(a))) = crate::fs::unafs::with_unafs(|fs| answer(fs, &p, &recent)) {
            got = Some(a);
        }
    }
    let a = got.unwrap_or_else(|| answer_walk(walk, &p, &recent));
    let us = crate::hidstall::now_us().saturating_sub(t0);
    serial_println!("[search] q={} hits={} ms={} us={} src={} cand={}", q.len(), a.rows.len(), us / 1000, us, a.src, a.cand);
    a.rows
}

// ── Return / Shift-Return ───────────────────────────────────────────────────────────────────────

/// Set by the Launcher's key door on Return: Shift was held (reveal, not open). Read once by [`pick`].
static REVEAL: AtomicBool = AtomicBool::new(false);

/// The key door's latch: this Return is a Shift-Return.
pub fn latch_reveal(on: bool) {
    REVEAL.store(on, Ordering::Release);
}

/// Act on a Files row (the Launcher's pass): Shift-Return reveals it in Quarry (Quarry opened at its folder, the
/// file selected — FILESEARCH2 B745); a
/// folder opens Quarry AT it. `None` = an ordinary open: the caller hands the file to the ONE opener dispatch.
pub fn pick(path: &str, dir: bool) -> Option<String> {
    let reveal = REVEAL.swap(false, Ordering::AcqRel);
    let folder = match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    };
    if reveal {
        let (shown, selected) = quarry_reveal(folder, search::leaf(path));
        serial_println!("[search] reveal path={} folder={} -> shown={} selected={}", path, folder, shown as u8, selected as u8);
        return Some(alloc::format!("reveal folder={} shown={} selected={}", folder, shown as u8, selected as u8));
    }
    if dir {
        let shown = quarry_at(path);
        if shown {
            note_opened(path);
        }
        serial_println!("[search] open folder={} -> shown={}", path, shown as u8);
        return Some(alloc::format!("quarry at={} shown={}", path, shown as u8));
    }
    None
}

#[cfg(feature = "quarry")]
fn quarry_at(dir: &str) -> bool {
    crate::video::quarry::live::open_at(dir)
}

#[cfg(not(feature = "quarry"))]
fn quarry_at(_dir: &str) -> bool {
    false
}

/// FILESEARCH2 (B745): the reveal — Quarry at `dir` with `leaf` selected. `(shown, selected)`.
#[cfg(feature = "quarry")]
fn quarry_reveal(dir: &str, leaf: &str) -> (bool, bool) {
    crate::video::quarry::live::open_at_select(dir, leaf)
}

#[cfg(not(feature = "quarry"))]
fn quarry_reveal(_dir: &str, _leaf: &str) -> (bool, bool) {
    (false, false)
}

// ── The icon: Quarry's painter ──────────────────────────────────────────────────────────────────

/// Draw the type icon for a row (`path`, `dir`) `s` px square at `(x, y)` into a `w`-wide surface of `h` rows —
/// Quarry's own painter (`quarry::live::iconview::draw_icon`, FILESEARCH2 B745: one painter for a file's icon, not
/// a second one here). `false` = nothing drawn (a build without Quarry, or no room): the Launcher draws its mark.
#[cfg(feature = "quarry")]
pub fn icon(buf: &mut [u32], w: usize, h: usize, x: usize, y: usize, s: usize, path: &str, dir: bool) -> bool {
    if s < 8 || buf.len() < w * h || x + s > w || y + s > h {
        return false;
    }
    crate::video::quarry::live::iconview::draw_icon(&mut buf[..w * h], w, h, x, y, s, path, dir, crate::video::text::Face::Ui);
    true
}

#[cfg(not(feature = "quarry"))]
pub fn icon(_buf: &mut [u32], _w: usize, _h: usize, _x: usize, _y: usize, _s: usize, _path: &str, _dir: bool) -> bool {
    false
}
