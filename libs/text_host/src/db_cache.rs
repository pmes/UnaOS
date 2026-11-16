//! CHARTER: Aether (CODEX §2 Web) through text_host — AETHERFONTDBCACHE (LEDGER SR684): the persisted index of
//! [`crate::db::FontDb::discover`], so an unchanged font set is not re-indexed at every process start.
//!
//! The index lives in the caller's directory ([`crate::fontconfig::Config::cache_dir`]; Aether's data dir /fonts) as
//! one text file, [`INDEX_FILE`]: a header (magic and format version), the scanned directory list, then per walked
//! file `F path size mtime_ns` and one `A` line per face (index, weight, slant, stretch, the family count, the
//! families, the full names), closed by an `end` trailer carrying the counts. Fields are tab-separated with `\\`,
//! `\t`, `\n` and `\r` escaped. The walk stays `FontDb::discover`'s; this module only reads, validates and writes.
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
pub const INDEX_FILE: &str = "fontdb-index.v1";
const MAGIC: &str = "fontdb-index";
const VERSION: &str = "1";

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
    if lines.next()? != format!("{MAGIC}\t{VERSION}") {
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
            return None;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[0] {
            "F" if fields.len() == 4 => {
                let path = PathBuf::from(unescape(fields[1])?);
                let entry = Entry { size: fields[2].parse().ok()?, mtime: fields[3].parse().ok()?, faces: Vec::new() };
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
                map.get_mut(path)?.faces.push(face);
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
pub(crate) fn load(config: &Config, files: &[PathBuf]) -> (Option<Vec<FaceInfo>>, State) {
    let Some(dir) = config.cache_dir.as_deref() else { return (None, State::Off) };
    let parsed = std::fs::read_to_string(dir.join(INDEX_FILE)).ok().and_then(|t| parse(&t, &config.dirs));
    let Some((order, mut map)) = parsed else { return (None, State::Miss) };
    let live: HashSet<&PathBuf> = files.iter().collect();
    for p in &order {
        let e = &map[p];
        if !live.contains(p) || stamp(p) != Some((e.size, e.mtime)) {
            return (None, State::Stale);
        }
    }
    if files.iter().any(|f| !map.contains_key(f)) {
        return (None, State::Miss);
    }
    let mut faces = Vec::new();
    for f in files {
        faces.extend(map.remove(f).map(|e| e.faces).unwrap_or_default());
    }
    (Some(faces), State::Hit)
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
}
