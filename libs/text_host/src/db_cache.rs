//! CHARTER: Aether (CODEX §2 Web) through text_host — AETHERFONTDBCACHE (LEDGER SR684): the persisted index of
//! [`crate::db::FontDb::discover`], so an unchanged font set is not re-indexed at every process start.
//!
//! The index lives in the caller's directory ([`crate::fontconfig::Config::cache_dir`]; Aether's data dir /fonts) as
//! one text file, [`INDEX_FILE`]: a header (magic and format version), the scanned directory list, then per walked
//! file `F path size mtime_ns` and one `A` line per face (index, weight, slant, stretch, the family count, the
//! families, the full names), closed by an `end` trailer carrying the counts; after it, AETHERFONTDBCACHECMAP
//! (SR697) appends one `C` line per face whose cmap a process read (see the named tail block). Fields are
//! tab-separated with `\\`, `\t`, `\n` and `\r` escaped. The walk stays `FontDb::discover`'s; this module only
//! reads, validates and writes.
//!
//! - [`State::Off`]: no cache dir (today's discovery, no file I/O here).
//! - [`State::Miss`]: no index, unreadable, another version or directory list, a parse error, a missing or wrong
//!   trailer (a truncation on a record boundary included), or a walked file the index does not hold.
//! - [`State::Stale`]: an indexed file gone from the walk, or present with another size or mtime_ns.
//! - [`State::Hit`]: the faces rebuilt from the index in walk order; no font byte is read.
//!
//! On a miss or stale index discovery runs as before and [`store`] rewrites the index atomically (a temp file in
//! the same directory, then a rename).

use crate::db::{FaceInfo, Slant, Style};
use crate::fontconfig::Config;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The index file's name inside the cache directory.
pub const INDEX_FILE: &str = "fontdb-index.v2";
const MAGIC: &str = "fontdb-index";
const VERSION: &str = "2";

/// What the persisted index answered for one discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Hit,
    Miss,
    Stale,
}

impl State {
    /// The `cache=` token of the `[fonts] discover` line.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Off => "off",
            State::Hit => "hit",
            State::Miss => "miss",
            State::Stale => "stale",
        }
    }
}

/// A file's (size, mtime in ns since the epoch); None when it cannot be stat'ed.
fn stamp(p: &Path) -> Option<(u64, u128)> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((m.len(), t))
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match it.next()? {
            '\\' => '\\',
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(out)
}

/// One indexed file: its stamp and its faces.
struct Entry {
    size: u64,
    mtime: u128,
    faces: Vec<FaceInfo>,
    covs: Vec<Option<Coverage>>,
}

fn slant_code(s: Slant) -> &'static str {
    match s {
        Slant::Normal => "N",
        Slant::Italic => "I",
        Slant::Oblique => "O",
    }
}

/// The index text of `files` (walk order) and their `faces` (grouped by file in the same order); None when a path
/// is not UTF-8, a file cannot be stat'ed, or the faces are not grouped in walk order.
fn render(dirs: &[PathBuf], files: &[PathBuf], faces: &[FaceInfo]) -> Option<String> {
    let mut out = format!("{MAGIC}\t{VERSION}\ndirs");
    for d in dirs {
        out.push('\t');
        out.push_str(&escape(d.to_str()?));
    }
    out.push('\n');
    let mut k = 0;
    for f in files {
        let (size, mtime) = stamp(f)?;
        out.push_str(&format!("F\t{}\t{size}\t{mtime}\n", escape(f.to_str()?)));
        while k < faces.len() && faces[k].path == *f {
            let x = &faces[k];
            out.push_str(&format!(
                "A\t{}\t{}\t{}\t{}\t{}",
                x.index,
                x.style.weight,
                slant_code(x.style.slant),
                x.style.stretch,
                x.families.len()
            ));
            for n in x.families.iter().chain(&x.full_names) {
                out.push('\t');
                out.push_str(&escape(n));
            }
            out.push('\n');
            k += 1;
        }
    }
    if k != faces.len() {
        return None;
    }
    out.push_str(&format!("end\t{}\t{}\n", files.len(), faces.len()));
    Some(out)
}

/// Parses an index written for `dirs`: the files in index order and their entries; None on any mismatch.
fn parse(text: &str, dirs: &[PathBuf]) -> Option<(Vec<PathBuf>, HashMap<PathBuf, Entry>)> {
    let mut lines = text.lines();
    if !text.ends_with('\n') || lines.next()? != format!("{MAGIC}\t{VERSION}") {
        return None;
    }
    let mut head = lines.next()?.split('\t');
    if head.next()? != "dirs" {
        return None;
    }
    let listed: Vec<PathBuf> = head.map(|d| unescape(d).map(PathBuf::from)).collect::<Option<_>>()?;
    if listed != dirs {
        return None;
    }
    let mut order: Vec<PathBuf> = Vec::new();
    let mut map: HashMap<PathBuf, Entry> = HashMap::new();
    let mut n_faces = 0usize;
    let mut ended = false;
    for line in lines {
        if ended {
            c_record(line, &mut map)?;
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[0] {
            "F" if fields.len() == 4 => {
                let path = PathBuf::from(unescape(fields[1])?);
                let entry = Entry { size: fields[2].parse().ok()?, mtime: fields[3].parse().ok()?, faces: Vec::new(), covs: Vec::new() };
                if map.insert(path.clone(), entry).is_some() {
                    return None;
                }
                order.push(path);
            }
            "A" if fields.len() >= 6 => {
                let path = order.last()?;
                let n_fam: usize = fields[5].parse().ok()?;
                if fields.len() < 6 + n_fam {
                    return None;
                }
                let names: Vec<String> = fields[6..].iter().map(|s| unescape(s)).collect::<Option<_>>()?;
                let slant = match fields[3] {
                    "N" => Slant::Normal,
                    "I" => Slant::Italic,
                    "O" => Slant::Oblique,
                    _ => return None,
                };
                let face = FaceInfo {
                    path: path.clone(),
                    index: fields[1].parse().ok()?,
                    families: names[..n_fam].to_vec(),
                    full_names: names[n_fam..].to_vec(),
                    style: Style { weight: fields[2].parse().ok()?, slant, stretch: fields[4].parse().ok()? },
                };
                let e = map.get_mut(path)?;
                e.faces.push(face);
                e.covs.push(None);
                n_faces += 1;
            }
            "end" if fields.len() == 3 => {
                if fields[1].parse::<usize>().ok()? != order.len() || fields[2].parse::<usize>().ok()? != n_faces {
                    return None;
                }
                ended = true;
            }
            _ => return None,
        }
    }
    ended.then_some((order, map))
}

/// The index's answer for the walked `files` of `config` (see the module comment).
pub(crate) fn load(config: &Config, files: &[PathBuf]) -> (Option<Vec<FaceInfo>>, Vec<Option<Coverage>>, State) {
    let Some(dir) = config.cache_dir.as_deref() else { return (None, Vec::new(), State::Off) };
    let parsed = std::fs::read_to_string(dir.join(INDEX_FILE)).ok().and_then(|t| parse(&t, &config.dirs));
    let Some((order, mut map)) = parsed else { return (None, Vec::new(), State::Miss) };
    let live: HashSet<&PathBuf> = files.iter().collect();
    for p in &order {
        let e = &map[p];
        if !live.contains(p) || stamp(p) != Some((e.size, e.mtime)) {
            return (None, Vec::new(), State::Stale);
        }
    }
    if files.iter().any(|f| !map.contains_key(f)) {
        return (None, Vec::new(), State::Miss);
    }
    let (mut faces, mut seeds) = (Vec::new(), Vec::new());
    for f in files {
        let e = map.remove(f).map(|e| (e.faces, e.covs)).unwrap_or_default();
        faces.extend(e.0);
        seeds.extend(e.1);
    }
    (Some(faces), seeds, State::Hit)
}

/// Rewrites the index after a miss or a stale answer (a no-op for Off and Hit); a failure leaves no index change.
pub(crate) fn store(config: &Config, files: &[PathBuf], faces: &[FaceInfo], state: State) {
    if !matches!(state, State::Miss | State::Stale) {
        return;
    }
    let Some(dir) = config.cache_dir.as_deref() else { return };
    let Some(text) = render(&config.dirs, files, faces) else { return };
    let _ = write_atomic(dir, &text);
}

/// Writes the index through a per-process temp file in the same directory and a rename.
fn write_atomic(dir: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{INDEX_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    let r = std::fs::rename(&tmp, dir.join(INDEX_FILE));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::FontDb;
    use std::fs;

    const INTER: &str = "inter/InterVariable.ttf";
    const JB: &str = "jetbrains-mono/JetBrainsMono-Variable.ttf";
    const NOTO: &str = "noto-sans/NotoSans-Variable.ttf";

    /// A per-test directory under the temp dir (`fonts/` and `cache/`), removed on drop.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(case: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("text_host_fontdb_cache_{}_{case}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(p.join("fonts")).unwrap();
            fs::create_dir_all(p.join("cache")).unwrap();
            Tmp(p)
        }
        fn cfg(&self) -> Config {
            Config { dirs: vec![self.0.join("fonts")], cache_dir: Some(self.0.join("cache")), ..Config::default() }
        }
        fn off(&self) -> Config {
            Config { cache_dir: None, ..self.cfg() }
        }
        fn index(&self) -> PathBuf {
            self.0.join("cache").join(INDEX_FILE)
        }
        fn copy_in(&self, rel: &str) -> PathBuf {
            let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts").join(rel);
            let dst = self.0.join("fonts").join(src.file_name().unwrap());
            fs::copy(&src, &dst).expect("fixture font");
            dst
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn same(a: &[FaceInfo], b: &[FaceInfo]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.path == y.path
                    && x.index == y.index
                    && x.families == y.families
                    && x.full_names == y.full_names
                    && x.style == y.style
            })
    }

    /// Discover twice: the first indexes and writes, the second is a hit that reads no font byte and yields the same
    /// records field for field; no temp file is left behind.
    #[test]
    fn fontdb_cache_hit_equal_kat() {
        let t = Tmp::new("hit");
        t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        assert_eq!(a.discover_cache(), "miss");
        assert!(a.discover_bytes() > 0);
        assert!(a.faces.len() >= 2, "{:?}", a.faces);
        assert!(t.index().is_file(), "the index is written");
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "hit");
        assert_eq!(b.discover_bytes(), 0, "a hit reads no font byte");
        assert!(same(&a.faces, &b.faces), "{:?} vs {:?}", a.faces, b.faces);
        let left: Vec<_> = fs::read_dir(t.0.join("cache")).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from(INDEX_FILE)], "only the index remains");
    }

    /// A changed mtime makes the index stale: re-discovered, the same records, then a hit again.
    #[test]
    fn fontdb_cache_stale_on_mtime_kat() {
        let t = Tmp::new("stale");
        let p = t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        assert_eq!(a.discover_cache(), "miss");
        let m = fs::metadata(&p).unwrap().modified().unwrap();
        fs::File::options().write(true).open(&p).unwrap().set_modified(m + std::time::Duration::from_secs(2)).unwrap();
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "stale");
        assert!(b.discover_bytes() > 0, "stale re-discovers");
        assert!(same(&a.faces, &b.faces));
        let c = FontDb::discover(t.cfg());
        assert_eq!(c.discover_cache(), "hit", "the rewritten index answers");
        assert!(same(&a.faces, &c.faces));
    }

    /// A file the index does not hold is a miss; the new index then answers with all three files' faces.
    #[test]
    fn fontdb_cache_miss_on_new_file_kat() {
        let t = Tmp::new("newfile");
        t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        assert_eq!(FontDb::discover(t.cfg()).discover_cache(), "hit");
        t.copy_in(NOTO);
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "miss");
        assert!(b.faces.len() > a.faces.len());
        let fresh = FontDb::discover(t.off());
        assert!(same(&b.faces, &fresh.faces));
        let c = FontDb::discover(t.cfg());
        assert_eq!(c.discover_cache(), "hit");
        assert!(same(&c.faces, &fresh.faces));
    }

    /// A truncated index is a miss, mid-line and on a record boundary (the trailer gone); a removed file is stale.
    #[test]
    fn fontdb_cache_miss_on_truncated_index_kat() {
        let t = Tmp::new("trunc");
        let p = t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        let text = fs::read_to_string(t.index()).unwrap();
        fs::write(t.index(), &text.as_bytes()[..text.len() / 2]).unwrap();
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "miss", "half an index");
        assert!(same(&a.faces, &b.faces));
        let text = fs::read_to_string(t.index()).unwrap();
        let body = text.trim_end_matches('\n');
        let cut = &body[..body.rfind('\n').unwrap() + 1];
        assert!(!cut.contains("\nend\t"));
        fs::write(t.index(), cut).unwrap();
        assert_eq!(FontDb::discover(t.cfg()).discover_cache(), "miss", "every record, no trailer");
        assert_eq!(FontDb::discover(t.cfg()).discover_cache(), "hit");
        fs::remove_file(&p).unwrap();
        let c = FontDb::discover(t.cfg());
        assert_eq!(c.discover_cache(), "stale", "an indexed file removed");
        assert!(same(&c.faces, &FontDb::discover(t.off()).faces));
    }

    /// No cache dir: off, nothing written, the same records as a cached discovery of the same files.
    #[test]
    fn fontdb_cache_off_kat() {
        let t = Tmp::new("off");
        t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.off());
        assert_eq!(a.discover_cache(), "off");
        assert!(a.discover_bytes() > 0);
        assert_eq!(fs::read_dir(t.0.join("cache")).unwrap().count(), 0, "off writes nothing");
        assert!(same(&a.faces, &FontDb::discover(t.cfg()).faces));
    }

    /// Tabs, newlines, carriage returns and backslashes in a name survive the index; a bad escape does not parse.
    #[test]
    fn fontdb_cache_escape_round_trip() {
        let t = Tmp::new("escape");
        let file = t.0.join("fonts").join("x.ttf");
        fs::write(&file, b"abc").unwrap();
        let face = FaceInfo {
            path: file.clone(),
            index: 3,
            families: vec!["Tab\tFam".into(), "New\nLine".into()],
            full_names: vec!["Back\\slash\r".into(), String::new()],
            style: Style { weight: 350.0, slant: Slant::Oblique, stretch: 62.5 },
        };
        let dirs = vec![t.0.join("fonts")];
        let text = render(&dirs, std::slice::from_ref(&file), std::slice::from_ref(&face)).unwrap();
        let (order, map) = parse(&text, &dirs).expect("parses");
        assert_eq!(order, vec![file.clone()]);
        assert!(same(&map[&file].faces, &[face]));
        assert_eq!(unescape("a\\x"), None);
        assert_eq!(unescape("a\\"), None);
        assert!(parse(&text, &[t.0.clone()]).is_none(), "another directory list");
    }

    /// AETHERFONTDBCACHECMAP (SR697): the number of `C` lines in the index.
    fn c_lines(t: &Tmp) -> usize {
        fs::read_to_string(t.index()).unwrap().lines().filter(|l| l.starts_with("C\t")).count()
    }

    const CHARS: [char; 8] = ['a', 'Z', '\u{e9}', '\u{416}', '\u{4e2d}', '\u{2603}', '\u{1f600}', '\u{f041}'];

    /// A discovery writes no `C` line; a `covers` on every face appends one per face read, once.
    #[test]
    fn fontdb_cache_cmap_written_kat() {
        let t = Tmp::new("cmapwrite");
        t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        assert_eq!(a.discover_cache(), "miss");
        assert_eq!(c_lines(&t), 0, "discovery reads no cmap");
        for i in 0..a.faces.len() {
            assert!(a.covers(i, 'a').is_some());
        }
        assert!(a.scan_bytes() > 0);
        assert_eq!(c_lines(&t), a.faces.len(), "one C line per face read");
        for i in 0..a.faces.len() {
            a.covers(i, 'Z');
        }
        assert_eq!(c_lines(&t), a.faces.len(), "at most once per face per process");
    }

    /// A fresh database from the same dir is a hit whose `covers` reads no byte, answers as the first did, keeps the
    /// fallback order and the face picked per char, and appends nothing.
    #[test]
    fn fontdb_cache_cmap_hit_kat() {
        let t = Tmp::new("cmaphit");
        t.copy_in(INTER);
        t.copy_in(JB);
        t.copy_in(NOTO);
        let a = FontDb::discover(t.cfg());
        let order_a = a.fallback_order().to_vec();
        let want: Vec<Vec<Option<bool>>> =
            (0..a.faces.len()).map(|i| CHARS.iter().map(|&c| a.covers(i, c)).collect()).collect();
        let len = fs::metadata(t.index()).unwrap().len();
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "hit");
        assert_eq!(b.fallback_order(), &order_a[..], "the same fallback order");
        for (i, row) in want.iter().enumerate() {
            for (k, &c) in CHARS.iter().enumerate() {
                assert_eq!(b.covers(i, c), row[k], "face {i} U+{:04X}", c as u32);
            }
        }
        for c in CHARS {
            let pick = |d: &FontDb| d.fallback_order().iter().copied().find(|&i| d.covers(i, c) == Some(true));
            assert_eq!(pick(&b), pick(&a), "the face picked for U+{:04X}", c as u32);
        }
        assert_eq!(b.scan_bytes(), 0, "a cmap hit reads no font byte");
        assert_eq!(fs::metadata(t.index()).unwrap().len(), len, "a hit appends nothing");
    }

    /// A touched file is stale: the rewritten index has no `C` line and `covers` reads again. A `C` line whose stamp
    /// is not its F record's is ignored: that face reads, the others do not.
    #[test]
    fn fontdb_cache_cmap_stale_kat() {
        let t = Tmp::new("cmapstale");
        let p = t.copy_in(INTER);
        t.copy_in(JB);
        let a = FontDb::discover(t.cfg());
        for i in 0..a.faces.len() {
            a.covers(i, 'a');
        }
        let text = fs::read_to_string(t.index()).unwrap();
        let jb = a.faces.iter().position(|f| f.path != p).unwrap();
        let jb_path = a.faces[jb].path.to_str().unwrap().to_string();
        let edited: String = text
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                if f[0] == "C" && f[1] == escape(&jb_path) {
                    let mut g: Vec<String> = f.iter().map(|s| s.to_string()).collect();
                    g[4] = "1".into();
                    g.join("\t")
                } else {
                    l.to_string()
                }
            })
            .map(|l| l + "\n")
            .collect();
        fs::write(t.index(), &edited).unwrap();
        let b = FontDb::discover(t.cfg());
        assert_eq!(b.discover_cache(), "hit");
        for i in (0..b.faces.len()).filter(|&i| b.faces[i].path == p) {
            b.covers(i, 'a');
        }
        assert_eq!(b.scan_bytes(), 0, "the matching C lines seed their faces");
        b.covers(jb, 'a');
        assert!(b.scan_bytes() > 0, "a C line with another stamp is ignored");
        let m = fs::metadata(&p).unwrap().modified().unwrap();
        fs::File::options().write(true).open(&p).unwrap().set_modified(m + std::time::Duration::from_secs(2)).unwrap();
        let c = FontDb::discover(t.cfg());
        assert_eq!(c.discover_cache(), "stale");
        assert_eq!(c_lines(&t), 0, "the stale rewrite drops every C line");
        c.covers(0, 'a');
        assert!(c.scan_bytes() > 0, "stale: covers reads again");
    }

    /// A v1 header is a miss; the index file is the v2 name.
    #[test]
    fn fontdb_cache_v1_is_miss_kat() {
        assert!(INDEX_FILE.ends_with(".v2"));
        let t = Tmp::new("v1");
        t.copy_in(INTER);
        FontDb::discover(t.cfg());
        let text = fs::read_to_string(t.index()).unwrap();
        let v1 = text.replacen(&format!("{MAGIC}\t{VERSION}\n"), &format!("{MAGIC}\t1\n"), 1);
        assert_ne!(v1, text);
        fs::write(t.index(), v1).unwrap();
        assert_eq!(FontDb::discover(t.cfg()).discover_cache(), "miss", "a v1 index");
        assert_eq!(FontDb::discover(t.cfg()).discover_cache(), "hit");
    }

    /// The coverage of each fixture face equals the glyph rule over every code point; a `C` line round-trips exactly;
    /// malformed `C` lines (and an index without its final newline) do not parse.
    #[test]
    fn fontdb_cache_cmap_round_trip() {
        let t = Tmp::new("cmaprt");
        for rel in [INTER, JB, NOTO] {
            t.copy_in(rel);
        }
        let db = FontDb::discover(t.off());
        for f in &db.faces {
            let (cmap, _) = crate::db::face_cmap(&f.path, f.index).unwrap();
            let cov = coverage(&cmap);
            let st = font_core::cmap::Cmap::parse(&cmap).and_then(|t| t.best()).unwrap();
            for cp in 0..=0x10FFFFu32 {
                let Some(c) = char::from_u32(cp) else { continue };
                assert_eq!(covered(&cov, c), st.unicode_glyph(c) != 0, "{}#{} U+{cp:04X}", f.path.display(), f.index);
            }
        }
        let file = t.0.join("fonts").join("x.ttf");
        fs::write(&file, b"abc").unwrap();
        let (size, mtime) = stamp(&file).unwrap();
        let face = FaceInfo {
            path: file.clone(),
            index: 0,
            families: vec!["X".into()],
            full_names: vec![],
            style: Style { weight: 400.0, slant: Slant::Normal, stretch: 100.0 },
        };
        let dirs = vec![t.0.join("fonts")];
        let body = render(&dirs, std::slice::from_ref(&file), std::slice::from_ref(&face)).unwrap();
        let cov: Coverage = vec![(0x20, 0x5f), (0xa0, 1), (0x10fff0, 0x10)].into_boxed_slice();
        let line = c_line(file.to_str().unwrap(), 0, size, mtime, &cov);
        let (_, map) = parse(&(body.clone() + &line), &dirs).expect("parses");
        assert_eq!(map[&file].covs, vec![Some(cov.clone())]);
        let p = escape(file.to_str().unwrap());
        for bad in [
            format!("C\t{p}\t0\t{size}\t{mtime}\t2\t20\t5f\n"),
            format!("C\t{p}\t0\t{size}\t{mtime}\t1\t20\t0\n"),
            format!("C\t{p}\t0\t{size}\t{mtime}\t2\t40\t1\t20\t1\n"),
            format!("C\t{p}\t0\t{size}\t{mtime}\t1\t10ffff\t2\n"),
            format!("C\t{p}\t0\t{size}\t{mtime}\t1\tzz\t1\n"),
        ] {
            assert!(parse(&(body.clone() + &bad), &dirs).is_none(), "{bad:?}");
        }
        assert!(parse(&(body.clone() + line.trim_end_matches('\n')), &dirs).is_none(), "no final newline");
        let other = format!("C\t{p}\t0\t{size}\t1\t1\t20\t1\nC\t{p}\t7\t{size}\t{mtime}\t1\t20\t1\n");
        let (_, map) = parse(&(body + &other), &dirs).expect("an unmatched stamp or face is ignored");
        assert_eq!(map[&file].covs, vec![None]);
    }
}

// ---- AETHERFONTDBCACHECMAP (LEDGER SR697): the persisted cmap coverage (named tail block) ----
// SR425's per-face cell holds the face's COVERAGE (sorted code-point ranges, start and length) derived once from its
// cmap table through font_core's one glyph rule, so `FontDb::covers` is a binary search. A process that reads a face's
// cmap APPENDS one line after the index's `end` trailer (never a rewrite: the readers ask face by face):
// `C path face_index size mtime_ns n start len ...` (ranges in hex), stamped before the read. `load` seeds the cell
// from a C line whose stamp equals its F record's; a C line naming no indexed face, or another stamp, is ignored; a
// malformed one is a miss. A miss or stale `store` rewrites the body with no C line.

/// One face's coverage: sorted, non-overlapping (start, len) code-point ranges.
pub(crate) type Coverage = Box<[(u32, u32)]>;

/// The code points face's `cmap` table maps (`unicode_glyph(c) != 0` on its best subtable), as ranges. Candidates are
/// the subtable's own mapped code points, U+0000..U+00FF (the Symbol rule) and U+FFFF; the glyph rule keeps each.
pub(crate) fn coverage(cmap: &[u8]) -> Coverage {
    let Some(st) = font_core::cmap::Cmap::parse(cmap).and_then(|t| t.best()) else { return Box::new([]) };
    let mut cps: Vec<u32> = Vec::new();
    st.for_each(|cp, _| cps.push(cp));
    cps.extend(0..0x100);
    cps.push(0xFFFF);
    cps.sort_unstable();
    cps.dedup();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for cp in cps.into_iter().filter(|&cp| char::from_u32(cp).is_some_and(|c| st.unicode_glyph(c) != 0)) {
        let k = out.len();
        if k > 0 && out[k - 1].0 + out[k - 1].1 == cp {
            out[k - 1].1 += 1;
        } else {
            out.push((cp, 1));
        }
    }
    out.into_boxed_slice()
}

/// Whether `cov` holds `c`.
pub(crate) fn covered(cov: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    let k = cov.partition_point(|&(s, _)| s <= cp);
    k > 0 && cp - cov[k - 1].0 < cov[k - 1].1
}

/// The `C` line of one face's coverage (newline-terminated).
fn c_line(path: &str, index: u32, size: u64, mtime: u128, cov: &[(u32, u32)]) -> String {
    let mut out = format!("C\t{}\t{index}\t{size}\t{mtime}\t{}", escape(path), cov.len());
    for (s, n) in cov {
        out.push_str(&format!("\t{s:x}\t{n:x}"));
    }
    out.push('\n');
    out
}

/// Applies one `C` line to the parsed entries; None when it is malformed (the index is then a miss).
fn c_record(line: &str, map: &mut HashMap<PathBuf, Entry>) -> Option<()> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 6 || f[0] != "C" {
        return None;
    }
    let path = PathBuf::from(unescape(f[1])?);
    let index: u32 = f[2].parse().ok()?;
    let (size, mtime): (u64, u128) = (f[3].parse().ok()?, f[4].parse().ok()?);
    let n: usize = f[5].parse().ok()?;
    if f.len() != 6 + 2 * n {
        return None;
    }
    let mut cov = Vec::with_capacity(n);
    let mut end = 0u32;
    for k in 0..n {
        let s = u32::from_str_radix(f[6 + 2 * k], 16).ok()?;
        let len = u32::from_str_radix(f[7 + 2 * k], 16).ok()?;
        if len == 0 || s < end || s.checked_add(len)? > 0x11_0000 {
            return None;
        }
        end = s + len;
        cov.push((s, len));
    }
    let Some(e) = map.get_mut(&path) else { return Some(()) };
    if (e.size, e.mtime) != (size, mtime) {
        return Some(());
    }
    if let Some(k) = e.faces.iter().position(|x| x.index == index) {
        e.covs[k] = Some(cov.into_boxed_slice());
    }
    Some(())
}

/// Reads face `f`'s cmap through [`crate::db::face_cmap`] (the one bounded reader) and derives its coverage; with
/// the index on (`state` not Off and a cache dir), appends its `C` line, stamped before the read. Returns the
/// coverage and the bytes read; None when the file or the face's directory is unreadable.
pub(crate) fn read_coverage(config: &Config, state: State, f: &FaceInfo) -> Option<(Coverage, u64)> {
    let before = stamp(&f.path);
    let (t, n) = crate::db::face_cmap(&f.path, f.index)?;
    let cov = coverage(&t);
    if let (false, Some(dir), Some((size, mtime)), Some(p)) =
        (state == State::Off, config.cache_dir.as_deref(), before, f.path.to_str())
    {
        let _ = append(dir, &c_line(p, f.index, size, mtime, &cov));
    }
    Some((cov, n))
}

/// Appends `line` to an existing index in one write; never creates the file.
fn append(dir: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(dir.join(INDEX_FILE))?.write_all(line.as_bytes())
}

/// The per-face coverage cells of a discovery: pre-filled from `seeds` (aligned to the faces), else empty.
pub(crate) fn seed(n: usize, mut seeds: Vec<Option<Coverage>>) -> Vec<std::sync::OnceLock<Option<Coverage>>> {
    (0..n)
        .map(|k| match seeds.get_mut(k).and_then(Option::take) {
            Some(c) => std::sync::OnceLock::from(Some(c)),
            None => std::sync::OnceLock::new(),
        })
        .collect()
}
