// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The on-disk repository (feature `std`): discovery (`.git` directory, `gitdir:` files, linked
//! worktrees with `commondir`, bare repositories), configuration (global + local, includes),
//! the object database (loose objects, packs with a delta-base cache, alternates), references
//! (loose, packed, symbolic, reflog appends under lock files), `rev-parse`, unique abbreviation,
//! the index, status (HEAD↔index↔worktree, untracked with gitignore), checkout of a tree, and
//! commit creation byte-identical to `git commit`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

use crate::config::{Config, Includer};
use crate::diff::ObjectSource;
use crate::hash::{HashKind, ObjectId};
use crate::ignore::{Ignore, PatternList};
use crate::index::{self, Index};
use crate::object::{self, mode, Commit, Kind, Signature, Tag, Tree, TreeEntry};
use crate::pack::{self, EntryKind, Idx, Pack};
use crate::refs::{PackedRefs, RefValue, ReflogEntry};
use crate::{loose, Error, Result};

fn io<E: std::fmt::Display>(what: &str, p: &Path, e: E) -> Error {
    Error::Io(format!("{what} {}: {e}", p.display()))
}

struct PackFile {
    path: PathBuf,
    idx: Vec<u8>,
    data: RefCell<Option<Rc<Vec<u8>>>>,
}

impl PackFile {
    fn data(&self) -> Result<Rc<Vec<u8>>> {
        if let Some(d) = self.data.borrow().as_ref() {
            return Ok(d.clone());
        }
        let d = Rc::new(fs::read(&self.path).map_err(|e| io("read", &self.path, e))?);
        *self.data.borrow_mut() = Some(d.clone());
        Ok(d)
    }
}

/// A small LRU-ish cache of resolved pack objects (delta bases), bounded by bytes.
#[derive(Default)]
struct BaseCache {
    map: HashMap<(usize, u64), (Kind, Rc<Vec<u8>>, u64)>,
    bytes: usize,
    tick: u64,
}

const BASE_CACHE_BYTES: usize = 96 << 20;

impl BaseCache {
    fn get(&mut self, k: (usize, u64)) -> Option<(Kind, Rc<Vec<u8>>)> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|v| {
            v.2 = t;
            (v.0, v.1.clone())
        })
    }
    fn put(&mut self, k: (usize, u64), kind: Kind, d: Rc<Vec<u8>>) {
        if d.len() > BASE_CACHE_BYTES / 4 {
            return;
        }
        self.tick += 1;
        self.bytes += d.len();
        if let Some(old) = self.map.insert(k, (kind, d, self.tick)) {
            self.bytes -= old.1.len();
        }
        while self.bytes > BASE_CACHE_BYTES {
            let victim = *self.map.iter().min_by_key(|(_, v)| v.2).unwrap().0;
            let v = self.map.remove(&victim).unwrap();
            self.bytes -= v.1.len();
        }
    }
}

/// An opened repository.
pub struct Repository {
    /// The per-worktree git directory (holds HEAD and the index).
    pub git_dir: PathBuf,
    /// The shared directory (objects, refs, config); equals `git_dir` outside linked worktrees.
    pub common_dir: PathBuf,
    /// The working tree, if not bare.
    pub work_tree: Option<PathBuf>,
    /// Object format.
    pub hash: HashKind,
    /// Effective configuration.
    pub config: Config,
    packs: RefCell<Option<Rc<Vec<PackFile>>>>,
    cache: RefCell<BaseCache>,
    alternates: Vec<PathBuf>,
}

struct FsIncluder {
    git_dir: PathBuf,
}

impl Includer for FsIncluder {
    fn load(&mut self, from: &[u8], path: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let p = String::from_utf8_lossy(path).into_owned();
        let pb = if let Some(rest) = p.strip_prefix("~/") {
            PathBuf::from(std::env::var_os("HOME")?).join(rest)
        } else {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() {
                pb
            } else {
                PathBuf::from(String::from_utf8_lossy(from).into_owned()).parent()?.join(pb)
            }
        };
        let d = fs::read(&pb).ok()?;
        Some((pb.to_string_lossy().into_owned().into_bytes(), d))
    }
    fn condition(&mut self, from: &[u8], cond: &[u8]) -> bool {
        let c = String::from_utf8_lossy(cond).into_owned();
        let (icase, pat) = if let Some(p) = c.strip_prefix("gitdir/i:") {
            (true, p.to_string())
        } else if let Some(p) = c.strip_prefix("gitdir:") {
            (false, p.to_string())
        } else {
            return false; // onbranch:, hasconfig: — not evaluated (documented ceiling)
        };
        let mut pat = pat;
        if let Some(rest) = pat.strip_prefix("~/") {
            if let Some(h) = std::env::var_os("HOME") {
                pat = format!("{}/{}", PathBuf::from(h).display(), rest);
            }
        } else if let Some(rest) = pat.strip_prefix("./") {
            let base = PathBuf::from(String::from_utf8_lossy(from).into_owned());
            pat = format!("{}/{}", base.parent().map(|p| p.display().to_string()).unwrap_or_default(), rest);
        } else if !pat.starts_with('/') {
            pat = format!("**/{pat}");
        }
        if pat.ends_with('/') {
            pat.push_str("**");
        }
        let gd = self.git_dir.canonicalize().unwrap_or(self.git_dir.clone());
        let mut s = gd.to_string_lossy().into_owned();
        if !s.ends_with('/') {
            s.push('/');
        }
        let flags = crate::ignore::PATHNAME | if icase { crate::ignore::CASEFOLD } else { 0 };
        crate::ignore::wildmatch(pat.as_bytes(), s.trim_end_matches('/').as_bytes(), flags) || crate::ignore::wildmatch(pat.as_bytes(), s.as_bytes(), flags)
    }
}

fn is_git_dir(p: &Path) -> bool {
    p.join("HEAD").is_file() && (p.join("objects").is_dir() || p.join("commondir").is_file()) && (p.join("refs").is_dir() || p.join("commondir").is_file())
}

/// Status of one path (porcelain v1 letters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Index vs HEAD: b' ', b'M', b'A', b'D', b'T', b'U'.
    pub staged: u8,
    /// Worktree vs index: b' ', b'M', b'D', b'T', b'?'.
    pub unstaged: u8,
    /// Path (untracked directories end in `/`).
    pub path: Vec<u8>,
}

impl Repository {
    /// Find the repository containing `start` (walking up), like `git rev-parse --git-dir`.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = start.as_ref();
        let mut dir = start.canonicalize().map_err(|e| io("open", start, e))?;
        loop {
            let dotgit = dir.join(".git");
            if dotgit.is_dir() && is_git_dir(&dotgit) {
                return Self::open_with(dotgit, Some(dir));
            }
            if dotgit.is_file() {
                let s = fs::read_to_string(&dotgit).map_err(|e| io("read", &dotgit, e))?;
                let g = s.trim().strip_prefix("gitdir:").ok_or(Error::Corrupt(".git file without gitdir:"))?.trim();
                let gd = if Path::new(g).is_absolute() { PathBuf::from(g) } else { dir.join(g) };
                return Self::open_with(gd, Some(dir));
            }
            if is_git_dir(&dir) {
                return Self::open_with(dir, None);
            }
            if !dir.pop() {
                return Err(Error::Io(format!("not a git repository (or any parent): {}", start.display())));
            }
        }
    }

    /// Open a git directory directly (bare or not); the worktree is inferred from `core.worktree`
    /// or the parent of a `.git` directory.
    pub fn open(git_dir: impl AsRef<Path>) -> Result<Self> {
        let gd = git_dir.as_ref().to_path_buf();
        let wt = if gd.file_name().is_some_and(|n| n == ".git") { gd.parent().map(Path::to_path_buf) } else { None };
        Self::open_with(gd, wt)
    }

    fn open_with(git_dir: PathBuf, work_tree: Option<PathBuf>) -> Result<Self> {
        let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
            Ok(s) => {
                let c = PathBuf::from(s.trim());
                if c.is_absolute() { c } else { git_dir.join(c) }
            }
            Err(_) => git_dir.clone(),
        };
        let mut config = Config::new();
        let mut inc = FsIncluder { git_dir: git_dir.clone() };
        let mut globals: Vec<PathBuf> = Vec::new();
        if std::env::var_os("GIT_CONFIG_NOSYSTEM").is_none() {
            globals.push(PathBuf::from("/etc/gitconfig"));
        }
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(x) if !x.is_empty() => globals.push(PathBuf::from(x).join("git/config")),
            _ => {
                if let Some(h) = std::env::var_os("HOME") {
                    globals.push(PathBuf::from(h).join(".config/git/config"));
                }
            }
        }
        if let Some(h) = std::env::var_os("HOME") {
            globals.push(PathBuf::from(h).join(".gitconfig"));
        }
        globals.push(common_dir.join("config"));
        for p in &globals {
            if let Ok(d) = fs::read(p) {
                config.load(p.to_string_lossy().as_bytes(), &d, &mut inc)?;
            }
        }
        let hash = match config.get(b"extensions.objectformat") {
            Some(v) => HashKind::from_name(&String::from_utf8_lossy(v).to_ascii_lowercase()).ok_or(Error::Unsupported("extensions.objectFormat"))?,
            None => HashKind::Sha1,
        };
        let bare = config.get_bool(b"core.bare").and_then(|r| r.ok()).unwrap_or(false);
        let work_tree = match config.get(b"core.worktree") {
            Some(w) => {
                let p = PathBuf::from(String::from_utf8_lossy(w).into_owned());
                Some(if p.is_absolute() { p } else { git_dir.join(p) })
            }
            None if bare && git_dir.join("commondir").is_file() == false => None,
            None => work_tree,
        };
        let mut alternates = Vec::new();
        if let Ok(s) = fs::read_to_string(common_dir.join("objects/info/alternates")) {
            for l in s.lines() {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let p = PathBuf::from(l);
                alternates.push(if p.is_absolute() { p } else { common_dir.join("objects").join(p) });
            }
        }
        Ok(Repository { git_dir, common_dir, work_tree, hash, config, packs: RefCell::new(None), cache: RefCell::new(BaseCache::default()), alternates })
    }

    /// Create a new repository (`git init`): `.git` under `dir` (or `dir` itself when bare).
    pub fn init(dir: impl AsRef<Path>, bare: bool, hk: HashKind, initial_branch: &str) -> Result<Self> {
        let dir = dir.as_ref();
        let gd = if bare { dir.to_path_buf() } else { dir.join(".git") };
        for sub in ["objects/info", "objects/pack", "refs/heads", "refs/tags", "info", "hooks"] {
            fs::create_dir_all(gd.join(sub)).map_err(|e| io("mkdir", &gd, e))?;
        }
        fs::write(gd.join("HEAD"), format!("ref: refs/heads/{initial_branch}\n")).map_err(|e| io("write", &gd, e))?;
        let mut cfg = String::from("[core]\n");
        cfg.push_str(&format!("\trepositoryformatversion = {}\n", if hk == HashKind::Sha1 { 0 } else { 1 }));
        cfg.push_str("\tfilemode = true\n");
        cfg.push_str(&format!("\tbare = {bare}\n"));
        if !bare {
            cfg.push_str("\tlogallrefupdates = true\n");
        }
        if hk != HashKind::Sha1 {
            cfg.push_str(&format!("[extensions]\n\tobjectformat = {}\n", hk.name()));
        }
        fs::write(gd.join("config"), cfg).map_err(|e| io("write", &gd, e))?;
        fs::write(gd.join("description"), "Unnamed repository; edit this file 'description' to name the repository.\n").map_err(|e| io("write", &gd, e))?;
        fs::write(gd.join("info/exclude"), "# git ls-files --others --exclude-from=.git/info/exclude\n").map_err(|e| io("write", &gd, e))?;
        if bare { Self::open_with(gd, None) } else { Self::open_with(gd, Some(dir.to_path_buf())) }
    }

    fn objects_dir(&self) -> PathBuf {
        self.common_dir.join("objects")
    }

    fn packs(&self) -> Result<Rc<Vec<PackFile>>> {
        if let Some(p) = self.packs.borrow().as_ref() {
            return Ok(p.clone());
        }
        let mut v = Vec::new();
        let mut dirs = vec![self.objects_dir().join("pack")];
        for a in &self.alternates {
            dirs.push(a.join("pack"));
        }
        for d in dirs {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            let mut names: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "idx")).collect();
            names.sort();
            for ip in names {
                let pp = ip.with_extension("pack");
                if !pp.is_file() {
                    continue;
                }
                let idx = fs::read(&ip).map_err(|e| io("read", &ip, e))?;
                Idx::parse(&idx, self.hash, false)?;
                v.push(PackFile { path: pp, idx, data: RefCell::new(None) });
            }
        }
        let rc = Rc::new(v);
        *self.packs.borrow_mut() = Some(rc.clone());
        Ok(rc)
    }

    /// Forget cached pack lists (after writing a pack).
    pub fn refresh(&self) {
        *self.packs.borrow_mut() = None;
    }

    fn loose_path(&self, id: &ObjectId) -> PathBuf {
        self.objects_dir().join(loose::path(id))
    }

    /// Read an object (kind, payload).
    pub fn read(&self, id: &ObjectId) -> Result<(Kind, Vec<u8>)> {
        let (k, d) = self.read_rc(id)?;
        Ok((k, Rc::try_unwrap(d).unwrap_or_else(|rc| (*rc).clone())))
    }

    fn read_rc(&self, id: &ObjectId) -> Result<(Kind, Rc<Vec<u8>>)> {
        let packs = self.packs()?;
        for (pi, p) in packs.iter().enumerate() {
            let ix = Idx::parse(&p.idx, self.hash, false)?;
            if let Some(off) = ix.lookup(id) {
                return self.read_packed(&packs, pi, off);
            }
        }
        let mut cands = vec![self.loose_path(id)];
        for a in &self.alternates {
            cands.push(a.join(loose::path(id)));
        }
        for c in cands {
            if let Ok(f) = fs::read(&c) {
                let (k, d) = loose::decode(&f, usize::MAX)?;
                return Ok((k, Rc::new(d)));
            }
        }
        Err(Error::Missing(*id))
    }

    fn read_packed(&self, packs: &[PackFile], pi: usize, offset: u64) -> Result<(Kind, Rc<Vec<u8>>)> {
        if let Some(hit) = self.cache.borrow_mut().get((pi, offset)) {
            return Ok(hit);
        }
        let data = packs[pi].data()?;
        let pk = Pack::parse(&data, self.hash, false)?;
        let h = pack::parse_entry_header(&data, offset as usize, self.hash)?;
        let (kind, out) = match h.kind {
            EntryKind::Base(k) => (k, Rc::new(pk.inflate_entry(&h)?.0)),
            EntryKind::OfsDelta(b) => {
                let (k, base) = self.read_packed(packs, pi, b)?;
                let d = pk.inflate_entry(&h)?.0;
                (k, Rc::new(crate::delta::apply(&base, &d)?))
            }
            EntryKind::RefDelta(bid) => {
                let ix = Idx::parse(&packs[pi].idx, self.hash, false)?;
                let (k, base) = match ix.lookup(&bid) {
                    Some(o) => self.read_packed(packs, pi, o)?,
                    None => self.read_rc(&bid)?,
                };
                let d = pk.inflate_entry(&h)?.0;
                (k, Rc::new(crate::delta::apply(&base, &d)?))
            }
        };
        self.cache.borrow_mut().put((pi, offset), kind, out.clone());
        Ok((kind, out))
    }

    /// Does the object exist?
    pub fn has(&self, id: &ObjectId) -> bool {
        if let Ok(packs) = self.packs() {
            for p in packs.iter() {
                if Idx::parse(&p.idx, self.hash, false).ok().and_then(|ix| ix.find(id)).is_some() {
                    return true;
                }
            }
        }
        self.loose_path(id).is_file() || self.alternates.iter().any(|a| a.join(loose::path(id)).is_file())
    }

    /// Write a loose object (`core.looseCompression`, default 1 as git), returning its id.
    pub fn write(&self, kind: Kind, payload: &[u8]) -> Result<ObjectId> {
        let id = object::hash_object(self.hash, kind, payload);
        if self.has(&id) {
            return Ok(id);
        }
        let level = self.config.get_int(b"core.loosecompression").and_then(|r| r.ok()).or_else(|| self.config.get_int(b"core.compression").and_then(|r| r.ok())).map(|l| if l < 0 { 1 } else { l.min(9) as u8 }).unwrap_or(1);
        let (_, bytes) = loose::encode(self.hash, kind, payload, level);
        let p = self.loose_path(&id);
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| io("mkdir", &p, e))?;
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&tmp, &bytes).map_err(|e| io("write", &tmp, e))?;
        let mut perm = fs::metadata(&tmp).map_err(|e| io("stat", &tmp, e))?.permissions();
        perm.set_readonly(true);
        let _ = fs::set_permissions(&tmp, perm);
        fs::rename(&tmp, &p).map_err(|e| io("rename", &p, e))?;
        Ok(id)
    }

    /// Store a received pack (and its idx) under `objects/pack`.
    pub fn install_pack(&self, packbytes: &[u8], idx: &[u8]) -> Result<PathBuf> {
        let ck = ObjectId::from_bytes(self.hash, &packbytes[packbytes.len() - self.hash.len()..]);
        let d = self.objects_dir().join("pack");
        fs::create_dir_all(&d).map_err(|e| io("mkdir", &d, e))?;
        let base = d.join(format!("pack-{ck}"));
        fs::write(base.with_extension("pack"), packbytes).map_err(|e| io("write", &base, e))?;
        fs::write(base.with_extension("idx"), idx).map_err(|e| io("write", &base, e))?;
        self.refresh();
        Ok(base.with_extension("pack"))
    }

    // -----------------------------------------------------------------------------------------
    // refs
    // -----------------------------------------------------------------------------------------

    fn ref_dir(&self, name: &str) -> &Path {
        if name == "HEAD" || name.starts_with("refs/worktree/") || name.starts_with("refs/bisect/") || !name.contains('/') {
            &self.git_dir
        } else {
            &self.common_dir
        }
    }

    /// The packed-refs file.
    pub fn packed_refs(&self) -> Result<PackedRefs> {
        match fs::read(self.common_dir.join("packed-refs")) {
            Ok(d) => PackedRefs::parse(self.hash, &d),
            Err(_) => Ok(PackedRefs::default()),
        }
    }

    /// Read one ref without following it.
    pub fn read_ref(&self, name: &str) -> Result<Option<RefValue>> {
        let p = self.ref_dir(name).join(name);
        if let Ok(d) = fs::read(&p) {
            return RefValue::parse(self.hash, &d).map(Some);
        }
        Ok(self.packed_refs()?.find(name.as_bytes()).map(|r| RefValue::Direct(r.id)))
    }

    /// Follow symbolic refs to the final (name, id) — id `None` for an unborn branch.
    pub fn resolve_ref(&self, name: &str) -> Result<Option<(String, Option<ObjectId>)>> {
        let mut n = name.to_string();
        for _ in 0..10 {
            match self.read_ref(&n)? {
                None => return Ok(if n == name { None } else { Some((n, None)) }),
                Some(RefValue::Direct(id)) => return Ok(Some((n, Some(id)))),
                Some(RefValue::Symbolic(t)) => n = String::from_utf8_lossy(&t).into_owned(),
            }
        }
        Err(Error::Corrupt("ref: symbolic ref loop"))
    }

    /// HEAD: (the branch it points at, if symbolic; its commit, if born).
    pub fn head(&self) -> Result<(Option<String>, Option<ObjectId>)> {
        match self.read_ref("HEAD")? {
            None => Err(Error::Corrupt("no HEAD")),
            Some(RefValue::Direct(id)) => Ok((None, Some(id))),
            Some(RefValue::Symbolic(_)) => {
                let (n, id) = self.resolve_ref("HEAD")?.unwrap();
                Ok((Some(n), id))
            }
        }
    }

    /// All refs under `prefix` (loose shadowing packed), sorted by name, peeled where known.
    pub fn refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>> {
        let mut m: BTreeMap<String, ObjectId> = BTreeMap::new();
        for r in self.packed_refs()?.refs {
            let n = String::from_utf8_lossy(&r.name).into_owned();
            if n.starts_with(prefix) {
                m.insert(n, r.id);
            }
        }
        fn walk(base: &Path, rel: &str, prefix: &str, hk: HashKind, m: &mut BTreeMap<String, ObjectId>) {
            let Ok(rd) = fs::read_dir(base.join(rel)) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let r = format!("{rel}/{name}");
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    walk(base, &r, prefix, hk, m);
                } else if r.starts_with(prefix) && !name.ends_with(".lock") {
                    if let Ok(d) = fs::read(e.path()) {
                        if let Ok(RefValue::Direct(id)) = RefValue::parse(hk, &d) {
                            m.insert(r, id);
                        }
                    }
                }
            }
        }
        walk(&self.common_dir, "refs", prefix, self.hash, &mut m);
        Ok(m.into_iter().collect())
    }

    /// Update `name` to `new` (optionally checking `old`), with a reflog line when the log
    /// exists or `core.logAllRefUpdates` applies; written through `<ref>.lock` as git does.
    pub fn update_ref(&self, name: &str, new: ObjectId, old: Option<ObjectId>, who: &Signature, msg: &str) -> Result<()> {
        let current = self.resolve_ref(name)?.and_then(|(_, id)| id);
        if let Some(o) = old {
            if current != Some(o) && !(o.is_null() && current.is_none()) {
                return Err(Error::Corrupt("ref: old value mismatch"));
            }
        }
        let p = self.ref_dir(name).join(name);
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| io("mkdir", &p, e))?;
        let lock = PathBuf::from(format!("{}.lock", p.display()));
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&lock).map_err(|e| io("lock", &lock, e))?;
        f.write_all(&RefValue::Direct(new).serialize()).map_err(|e| io("write", &lock, e))?;
        drop(f);
        fs::rename(&lock, &p).map_err(|e| io("rename", &p, e))?;
        let prev = current.unwrap_or(self.hash.null());
        self.append_reflog(name, prev, new, who, msg)?;
        Ok(())
    }

    fn append_reflog(&self, name: &str, old: ObjectId, new: ObjectId, who: &Signature, msg: &str) -> Result<()> {
        let log = self.ref_dir(name).join("logs").join(name);
        let all = self.config.get_bool(b"core.logallrefupdates").and_then(|r| r.ok()).unwrap_or(self.work_tree.is_some());
        let wanted = log.is_file() || (all && (name == "HEAD" || name.starts_with("refs/heads/") || name.starts_with("refs/remotes/") || name.starts_with("refs/notes/")));
        if !wanted {
            return Ok(());
        }
        fs::create_dir_all(log.parent().unwrap()).map_err(|e| io("mkdir", &log, e))?;
        let e = ReflogEntry { old, new, who: who.clone(), message: msg.as_bytes().to_vec() };
        let mut f = fs::OpenOptions::new().append(true).create(true).open(&log).map_err(|e| io("open", &log, e))?;
        f.write_all(&e.serialize()).map_err(|e| io("write", &log, e))?;
        Ok(())
    }

    /// Point a symbolic ref (e.g. HEAD) at `target`.
    pub fn set_symbolic_ref(&self, name: &str, target: &str) -> Result<()> {
        let p = self.ref_dir(name).join(name);
        fs::write(&p, RefValue::Symbolic(target.as_bytes().to_vec()).serialize()).map_err(|e| io("write", &p, e))
    }

    // -----------------------------------------------------------------------------------------
    // rev-parse
    // -----------------------------------------------------------------------------------------

    /// Every object id with hex prefix `p` (packs + loose).
    pub fn prefix_matches(&self, p: &[u8]) -> Result<Vec<ObjectId>> {
        let mut v = Vec::new();
        for pk in self.packs()?.iter() {
            Idx::parse(&pk.idx, self.hash, false)?.prefix_matches(p, &mut v);
        }
        if p.len() >= 2 {
            let d = self.objects_dir().join(String::from_utf8_lossy(&p[..2]).to_ascii_lowercase());
            if let Ok(rd) = fs::read_dir(&d) {
                for e in rd.flatten() {
                    let mut hex = p[..2].to_ascii_lowercase();
                    hex.extend_from_slice(e.file_name().to_string_lossy().as_bytes());
                    if let Some(id) = ObjectId::from_hex_kind(self.hash, &hex) {
                        if id.starts_with_hex(p) {
                            v.push(id);
                        }
                    }
                }
            }
        }
        v.sort();
        v.dedup();
        Ok(v)
    }

    fn peel(&self, mut id: ObjectId, want: Option<Kind>) -> Result<ObjectId> {
        loop {
            let (k, d) = self.read(&id)?;
            if want == Some(k) || (want.is_none() && k != Kind::Tag) {
                return Ok(id);
            }
            match k {
                Kind::Tag => id = Tag::parse(self.hash, &d)?.target().ok_or(Error::Corrupt("tag target"))?,
                Kind::Commit if want == Some(Kind::Tree) => return Commit::parse(self.hash, &d)?.tree().ok_or(Error::Corrupt("commit tree")),
                _ => return Err(Error::Corrupt("rev-parse: cannot peel to the requested kind")),
            }
        }
    }

    /// Resolve a revision: full/short hex, ref names (dwim), `HEAD`/`@`, with `~n`, `^n`,
    /// `^{tree}`, `^{commit}`, `^{}` suffixes and `<rev>:<path>`.
    pub fn rev_parse(&self, spec: &str) -> Result<ObjectId> {
        if let Some((rev, path)) = spec.split_once(':') {
            let tree = self.peel(self.rev_parse(if rev.is_empty() { "HEAD" } else { rev })?, Some(Kind::Tree))?;
            return self.lookup_path(&tree, path.as_bytes())?.map(|(_, id)| id).ok_or(Error::Corrupt("rev-parse: path not in tree"));
        }
        // split base and suffixes
        let b = spec.as_bytes();
        let mut end = b.len();
        for (i, &c) in b.iter().enumerate() {
            if c == b'~' || c == b'^' {
                end = i;
                break;
            }
        }
        let base = &spec[..end];
        let mut id = self.resolve_base(if base.is_empty() || base == "@" { "HEAD" } else { base })?;
        let mut i = end;
        while i < b.len() {
            let c = b[i];
            i += 1;
            if c == b'^' && b.get(i) == Some(&b'{') {
                let close = spec[i..].find('}').ok_or(Error::Corrupt("rev-parse: unterminated ^{"))? + i;
                let what = &spec[i + 1..close];
                id = match what {
                    "" => self.peel(id, None)?,
                    "commit" => self.peel(id, Some(Kind::Commit))?,
                    "tree" => self.peel(id, Some(Kind::Tree))?,
                    "blob" => self.peel(id, Some(Kind::Blob))?,
                    "tag" => id,
                    _ => return Err(Error::Unsupported("rev-parse ^{...} form")),
                };
                i = close + 1;
                continue;
            }
            let mut n: usize = 0;
            let mut digits = false;
            while i < b.len() && b[i].is_ascii_digit() {
                n = n * 10 + (b[i] - b'0') as usize;
                i += 1;
                digits = true;
            }
            if !digits {
                n = 1;
            }
            let c_id = self.peel(id, Some(Kind::Commit))?;
            if c == b'~' {
                id = c_id;
                for _ in 0..n {
                    let (_, d) = self.read(&id)?;
                    id = *Commit::parse(self.hash, &d)?.parents().first().ok_or(Error::Corrupt("rev-parse: no parent"))?;
                }
            } else if n == 0 {
                id = c_id;
            } else {
                let (_, d) = self.read(&c_id)?;
                id = *Commit::parse(self.hash, &d)?.parents().get(n - 1).ok_or(Error::Corrupt("rev-parse: no such parent"))?;
            }
        }
        Ok(id)
    }

    fn resolve_base(&self, s: &str) -> Result<ObjectId> {
        if let Some(id) = ObjectId::from_hex_kind(self.hash, s.as_bytes()) {
            return Ok(id);
        }
        for rule in crate::refs::RESOLVE_RULES {
            let name = rule.replace("%s", s);
            if let Some((_, Some(id))) = self.resolve_ref(&name)? {
                return Ok(id);
            }
        }
        if s.len() >= 4 && s.bytes().all(|c| c.is_ascii_hexdigit()) {
            let m = self.prefix_matches(s.as_bytes())?;
            match m.len() {
                1 => return Ok(m[0]),
                0 => {}
                _ => return Err(Error::Corrupt("rev-parse: ambiguous short id")),
            }
        }
        Err(Error::Io(format!("unknown revision {s}")))
    }

    /// Find `path` inside a tree: (mode, id).
    pub fn lookup_path(&self, tree: &ObjectId, path: &[u8]) -> Result<Option<(u32, ObjectId)>> {
        let mut cur = *tree;
        let mut mode = mode::TREE;
        for comp in path.split(|&c| c == b'/').filter(|c| !c.is_empty()) {
            let (k, d) = self.read(&cur)?;
            if k != Kind::Tree {
                return Ok(None);
            }
            let t = Tree::parse(self.hash, &d)?;
            match t.find(comp) {
                Some(e) => {
                    cur = e.id;
                    mode = e.mode;
                }
                None => return Ok(None),
            }
        }
        Ok(Some((mode, cur)))
    }

    /// Flatten a tree into path → (mode, id).
    pub fn flatten_tree(&self, tree: &ObjectId) -> Result<BTreeMap<Vec<u8>, (u32, ObjectId)>> {
        let mut m = BTreeMap::new();
        let mut stack = vec![(Vec::new(), *tree)];
        while let Some((base, id)) = stack.pop() {
            let (_, d) = self.read(&id)?;
            for e in Tree::parse(self.hash, &d)?.entries {
                let mut p = base.clone();
                if !p.is_empty() {
                    p.push(b'/');
                }
                p.extend_from_slice(&e.name);
                if e.is_tree() {
                    stack.push((p, e.id));
                } else {
                    m.insert(p, (e.mode, e.id));
                }
            }
        }
        Ok(m)
    }

    // -----------------------------------------------------------------------------------------
    // index, status, checkout, commit
    // -----------------------------------------------------------------------------------------

    /// Read the index (empty when absent).
    pub fn index(&self) -> Result<Index> {
        match fs::read(self.git_dir.join("index")) {
            Ok(d) => Index::parse(self.hash, &d),
            Err(_) => Ok(Index::new(self.hash)),
        }
    }

    /// Write the index through `index.lock`.
    pub fn write_index(&self, idx: &Index) -> Result<()> {
        let p = self.git_dir.join("index");
        let lock = self.git_dir.join("index.lock");
        fs::write(&lock, idx.serialize()).map_err(|e| io("write", &lock, e))?;
        fs::rename(&lock, &p).map_err(|e| io("rename", &p, e))
    }

    fn worktree(&self) -> Result<&Path> {
        self.work_tree.as_deref().ok_or(Error::Unsupported("bare repository has no worktree"))
    }

    /// Hash a worktree file as a blob (symlinks hash their target).
    pub fn hash_worktree_file(&self, rel: &[u8]) -> Result<(u32, ObjectId)> {
        let p = self.worktree()?.join(String::from_utf8_lossy(rel).into_owned());
        let md = fs::symlink_metadata(&p).map_err(|e| io("stat", &p, e))?;
        if md.file_type().is_symlink() {
            let t = fs::read_link(&p).map_err(|e| io("readlink", &p, e))?;
            let tb = t.to_string_lossy().into_owned().into_bytes();
            return Ok((mode::LINK, object::hash_object(self.hash, Kind::Blob, &tb)));
        }
        let d = fs::read(&p).map_err(|e| io("read", &p, e))?;
        Ok((file_mode(&md), object::hash_object(self.hash, Kind::Blob, &d)))
    }

    /// Porcelain-v1 status (no rename detection), sorted by path; untracked directories
    /// collapse to `dir/` as git's default `--untracked-files=normal`.
    pub fn status(&self, include_untracked: bool) -> Result<Vec<StatusEntry>> {
        let wt = self.worktree()?.to_path_buf();
        let idx = self.index()?;
        let head_tree = match self.head()?.1 {
            Some(c) => {
                let (_, d) = self.read(&c)?;
                Some(Commit::parse(self.hash, &d)?.tree().ok_or(Error::Corrupt("commit tree"))?)
            }
            None => None,
        };
        let head = match head_tree {
            Some(t) => self.flatten_tree(&t)?,
            None => BTreeMap::new(),
        };
        let index_mtime = fs::metadata(self.git_dir.join("index")).ok().map(|m| mtime(&m));
        let filemode = self.config.get_bool(b"core.filemode").and_then(|r| r.ok()).unwrap_or(true);
        let mut out: BTreeMap<Vec<u8>, (u8, u8)> = BTreeMap::new();
        let mut seen_paths: BTreeMap<Vec<u8>, ()> = BTreeMap::new();
        for e in &idx.entries {
            seen_paths.insert(e.path.clone(), ());
            if e.stage() != 0 {
                out.insert(e.path.clone(), (b'U', b'U'));
                continue;
            }
            let staged = match head.get(&e.path) {
                None => b'A',
                Some((m, id)) => {
                    if (m & mode::TYPE_MASK) != (e.mode & mode::TYPE_MASK) {
                        b'T'
                    } else if *id != e.oid() || *m != e.mode {
                        b'M'
                    } else {
                        b' '
                    }
                }
            };
            let unstaged = if e.ext_flags & index::flags::SKIP_WORKTREE != 0 || e.mode == mode::COMMIT {
                b' '
            } else {
                let p = wt.join(String::from_utf8_lossy(&e.path).into_owned());
                match fs::symlink_metadata(&p) {
                    Err(_) => b'D',
                    Ok(md) => {
                        let wmode = if md.file_type().is_symlink() { mode::LINK } else if md.is_dir() { 0 } else { file_mode(&md) };
                        if wmode == 0 {
                            b'D'
                        } else if (wmode & mode::TYPE_MASK) != (e.mode & mode::TYPE_MASK) {
                            b'T'
                        } else {
                            let racy = index_mtime.is_none_or(|im| mtime(&md) >= im);
                            let stat_ok = md_size(&md) as u32 == e.size && mtime(&md) == (e.mtime_s as i64, e.mtime_ns as i64) && !racy;
                            let mode_changed = filemode && wmode != e.mode && e.mode != mode::LINK;
                            if stat_ok && !mode_changed {
                                b' '
                            } else {
                                let (_, id) = self.hash_worktree_file(&e.path)?;
                                if id != e.oid() || mode_changed { b'M' } else { b' ' }
                            }
                        }
                    }
                }
            };
            if staged != b' ' || unstaged != b' ' {
                out.insert(e.path.clone(), (staged, unstaged));
            }
        }
        for p in head.keys() {
            if !seen_paths.contains_key(p) {
                out.insert(p.clone(), (b'D', b' '));
            }
        }
        let mut entries: Vec<StatusEntry> = out.into_iter().map(|(path, (s, u))| StatusEntry { staged: s, unstaged: u, path }).collect();
        if include_untracked {
            let tracked: Vec<Vec<u8>> = idx.entries.iter().map(|e| e.path.clone()).collect();
            let mut ig = Ignore::default();
            if let Some(x) = self.config.get(b"core.excludesfile") {
                if let Ok(d) = fs::read(expand_home(&String::from_utf8_lossy(x))) {
                    ig.lists.push(PatternList::parse(&d, b""));
                }
            } else if let Some(h) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))) {
                if let Ok(d) = fs::read(h.join("git/ignore")) {
                    ig.lists.push(PatternList::parse(&d, b""));
                }
            }
            if let Ok(d) = fs::read(self.common_dir.join("info/exclude")) {
                ig.lists.push(PatternList::parse(&d, b""));
            }
            let mut un = Vec::new();
            self.untracked(&wt, b"", &tracked, &mut ig, &mut un)?;
            // git lists tracked changes first, then untracked paths, each in path order.
            un.sort();
            for u in un {
                entries.push(StatusEntry { staged: b'?', unstaged: b'?', path: u });
            }
        }
        Ok(entries)
    }

    fn untracked(&self, wt: &Path, rel: &[u8], tracked: &[Vec<u8>], ig: &mut Ignore, out: &mut Vec<Vec<u8>>) -> Result<()> {
        let dir = wt.join(String::from_utf8_lossy(rel).into_owned());
        let pushed = match fs::read(dir.join(".gitignore")) {
            Ok(d) => {
                let mut base = rel.to_vec();
                if !base.is_empty() {
                    base.push(b'/');
                }
                ig.lists.push(PatternList::parse(&d, &base));
                true
            }
            Err(_) => false,
        };
        let mut names: Vec<(Vec<u8>, bool)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned().into_bytes();
                if rel.is_empty() && n == b".git" {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                names.push((n, is_dir));
            }
        }
        names.sort();
        for (n, is_dir) in names {
            let mut p = rel.to_vec();
            if !p.is_empty() {
                p.push(b'/');
            }
            p.extend_from_slice(&n);
            if ig.is_excluded(&p, is_dir) {
                continue;
            }
            if is_dir {
                if wt.join(String::from_utf8_lossy(&p).into_owned()).join(".git").exists() {
                    if !tracked.iter().any(|t| *t == p) {
                        let mut d = p.clone();
                        d.push(b'/');
                        out.push(d);
                    }
                    continue;
                }
                let mut prefix = p.clone();
                prefix.push(b'/');
                let has_tracked = tracked.iter().any(|t| t.starts_with(&prefix));
                if has_tracked {
                    self.untracked(wt, &p, tracked, ig, out)?;
                } else {
                    let mut sub = Vec::new();
                    self.untracked(wt, &p, tracked, ig, &mut sub)?;
                    if !sub.is_empty() {
                        out.push(prefix);
                    }
                }
            } else if !tracked.iter().any(|t| *t == p) {
                out.push(p);
            }
        }
        if pushed {
            ig.lists.pop();
        }
        Ok(())
    }

    /// Any tracked change (staged or unstaged)? Untracked files do not count — `git status
    /// --porcelain --untracked-files=no` is non-empty.
    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.status(false)?.is_empty())
    }

    /// Check out `tree` into the (empty or matching) worktree and write a fresh index for it.
    pub fn checkout_tree(&self, tree: &ObjectId) -> Result<Index> {
        let wt = self.worktree()?.to_path_buf();
        let files = self.flatten_tree(tree)?;
        let mut idx = Index::new(self.hash);
        for (path, (m, id)) in &files {
            let p = wt.join(String::from_utf8_lossy(path).into_owned());
            if let Some(par) = p.parent() {
                fs::create_dir_all(par).map_err(|e| io("mkdir", par, e))?;
            }
            if *m == mode::COMMIT {
                fs::create_dir_all(&p).map_err(|e| io("mkdir", &p, e))?;
                let mut e = index::Entry { mode: *m, id: Some(*id), path: path.clone(), ..Default::default() };
                e.fix_flags();
                idx.entries.push(e);
                continue;
            }
            let (_, data) = self.read(id)?;
            let _ = fs::remove_file(&p);
            if *m == mode::LINK {
                #[cfg(unix)]
                std::os::unix::fs::symlink(String::from_utf8_lossy(&data).into_owned(), &p).map_err(|e| io("symlink", &p, e))?;
            } else {
                fs::write(&p, &data).map_err(|e| io("write", &p, e))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perm = fs::Permissions::from_mode(if *m == mode::BLOB_EXEC { 0o755 } else { 0o644 });
                    fs::set_permissions(&p, perm).map_err(|e| io("chmod", &p, e))?;
                }
            }
            let md = fs::symlink_metadata(&p).map_err(|e| io("stat", &p, e))?;
            let mut e = stat_entry(&md);
            e.mode = *m;
            e.id = Some(*id);
            e.path = path.clone();
            e.fix_flags();
            idx.entries.push(e);
        }
        idx.sort();
        self.write_index(&idx)?;
        Ok(idx)
    }

    /// Stage worktree paths (`git add <paths>`; a missing file is removed from the index).
    pub fn add(&self, paths: &[&[u8]]) -> Result<()> {
        let wt = self.worktree()?.to_path_buf();
        let mut idx = self.index()?;
        for &rel in paths {
            let p = wt.join(String::from_utf8_lossy(rel).into_owned());
            idx.entries.retain(|e| e.path != rel);
            if let Ok(md) = fs::symlink_metadata(&p) {
                let (m, data) = if md.file_type().is_symlink() {
                    (mode::LINK, fs::read_link(&p).map_err(|e| io("readlink", &p, e))?.to_string_lossy().into_owned().into_bytes())
                } else {
                    (file_mode(&md), fs::read(&p).map_err(|e| io("read", &p, e))?)
                };
                let id = self.write(Kind::Blob, &data)?;
                let mut e = stat_entry(&md);
                e.mode = m;
                e.id = Some(id);
                e.path = rel.to_vec();
                e.fix_flags();
                idx.entries.push(e);
            }
        }
        idx.sort();
        idx.invalidate_derived();
        self.write_index(&idx)
    }

    /// Write the index's stage-0 entries as trees (`git write-tree`).
    pub fn write_tree(&self, idx: &Index) -> Result<ObjectId> {
        let ents: Vec<(&[u8], u32, ObjectId)> = idx.entries.iter().filter(|e| e.stage() == 0 && e.ext_flags & index::flags::INTENT_TO_ADD == 0).map(|e| (e.path.as_slice(), e.mode, e.oid())).collect();
        self.build_tree(&ents, 0)
    }

    fn build_tree(&self, ents: &[(&[u8], u32, ObjectId)], depth: usize) -> Result<ObjectId> {
        let mut t = Tree::default();
        let mut i = 0;
        while i < ents.len() {
            let rest = &ents[i].0[depth..];
            match rest.iter().position(|&c| c == b'/') {
                None => {
                    t.entries.push(TreeEntry { mode: ents[i].1, name: rest.to_vec(), id: ents[i].2 });
                    i += 1;
                }
                Some(s) => {
                    let dir = &rest[..s];
                    let mut j = i;
                    while j < ents.len() && ents[j].0.len() > depth + s && &ents[j].0[depth..depth + s] == dir && ents[j].0[depth + s] == b'/' {
                        j += 1;
                    }
                    let id = self.build_tree(&ents[i..j], depth + s + 1)?;
                    t.entries.push(TreeEntry { mode: mode::TREE, name: dir.to_vec(), id });
                    i = j;
                }
            }
        }
        t.sort();
        self.write(Kind::Tree, &t.serialize())
    }

    /// The identity for `who` ("AUTHOR" / "COMMITTER"): `GIT_<WHO>_NAME/EMAIL/DATE`, else
    /// `user.name`/`user.email` and the current time (UTC offset from `GIT_<WHO>_DATE` only).
    pub fn ident(&self, who: &str) -> Result<Signature> {
        let env = |k: &str| std::env::var(format!("GIT_{who}_{k}")).ok();
        let name = env("NAME").or_else(|| self.config.get(b"user.name").map(|v| String::from_utf8_lossy(v).into_owned())).ok_or(Error::Corrupt("ident: no name (user.name)"))?;
        let email = env("EMAIL").or_else(|| self.config.get(b"user.email").map(|v| String::from_utf8_lossy(v).into_owned())).ok_or(Error::Corrupt("ident: no email (user.email)"))?;
        let (time, off) = match env("DATE") {
            Some(d) => parse_raw_date(&d).ok_or(Error::Unsupported("date format (raw `<secs> <+hhmm>` or `@<secs>` only)"))?,
            None => (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0), 0),
        };
        Ok(Signature::new(name.trim().as_bytes(), email.trim().as_bytes(), time, off))
    }

    /// `git commit`: write the index as a tree, create the commit with HEAD as parent, advance
    /// the branch HEAD names (reflog `commit: <subject>` / `commit (initial): <subject>`).
    pub fn commit(&self, message: &str) -> Result<ObjectId> {
        let author = self.ident("AUTHOR")?;
        let committer = self.ident("COMMITTER")?;
        self.commit_as(message, &author, &committer)
    }

    /// [`Repository::commit`] with explicit identities.
    pub fn commit_as(&self, message: &str, author: &Signature, committer: &Signature) -> Result<ObjectId> {
        let idx = self.index()?;
        let tree = self.write_tree(&idx)?;
        let (branch, parent) = self.head()?;
        let mut msg = message.to_string();
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
        let parents: Vec<ObjectId> = parent.into_iter().collect();
        let c = Commit::new(tree, &parents, author, committer, msg.as_bytes());
        let id = self.write(Kind::Commit, &c.serialize())?;
        let subject = c.summary();
        let reflog = format!("commit{}: {}", if parents.is_empty() { " (initial)" } else { "" }, String::from_utf8_lossy(subject));
        match branch {
            Some(b) => {
                self.update_ref(&b, id, parent.or(Some(self.hash.null())), committer, &reflog)?;
                let prev = parent.unwrap_or(self.hash.null());
                self.append_reflog("HEAD", prev, id, committer, &reflog)?;
            }
            None => self.update_ref("HEAD", id, parent, committer, &reflog)?,
        }
        Ok(id)
    }
}

impl ObjectSource for Repository {
    fn read_object(&self, id: &ObjectId) -> Result<(Kind, Vec<u8>)> {
        self.read(id)
    }

    fn unique_abbrev(&self, id: &ObjectId, min: usize) -> usize {
        let hexlen = id.kind().hex_len();
        let mut need = min;
        let common_hex = |a: &ObjectId, b: &ObjectId| -> usize {
            let (x, y) = (a.as_bytes(), b.as_bytes());
            let mut n = 0;
            for k in 0..x.len() {
                if x[k] == y[k] {
                    n += 2;
                    continue;
                }
                if x[k] >> 4 == y[k] >> 4 {
                    n += 1;
                }
                break;
            }
            n
        };
        if let Ok(packs) = self.packs() {
            for p in packs.iter() {
                let Ok(ix) = Idx::parse(&p.idx, self.hash, false) else { continue };
                let c = ix.count as usize;
                // first position >= id
                let (mut lo, mut hi) = (0usize, c);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if ix.oid(mid).as_bytes() < id.as_bytes() { lo = mid + 1 } else { hi = mid }
                }
                let mut check = |k: usize| {
                    let o = ix.oid(k);
                    if o != *id {
                        need = need.max(common_hex(&o, id) + 1);
                    }
                };
                if lo < c {
                    check(lo);
                    if ix.oid(lo) == *id && lo + 1 < c {
                        check(lo + 1);
                    }
                }
                if lo > 0 {
                    check(lo - 1);
                }
            }
        }
        let hex = id.to_hex();
        let d = self.objects_dir().join(&hex[..2]);
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                let full = format!("{}{}", &hex[..2], e.file_name().to_string_lossy());
                if let Some(o) = ObjectId::from_hex_kind(self.hash, full.as_bytes()) {
                    if o != *id {
                        need = need.max(common_hex(&o, id) + 1);
                    }
                }
            }
        }
        need.min(hexlen)
    }

    fn default_abbrev(&self) -> usize {
        if let Some(v) = self.config.get(b"core.abbrev") {
            let s = String::from_utf8_lossy(v).to_ascii_lowercase();
            if s == "no" {
                return self.hash.hex_len();
            }
            if let Ok(n) = s.parse::<usize>() {
                return n.clamp(4, self.hash.hex_len());
            }
        }
        let mut count: u64 = 0;
        if let Ok(packs) = self.packs() {
            for p in packs.iter() {
                if let Ok(ix) = Idx::parse(&p.idx, self.hash, false) {
                    count += ix.count as u64;
                }
            }
        }
        let bits = if count == 0 { 1 } else { 64 - count.leading_zeros() as usize };
        bits.div_ceil(2).max(7)
    }
}

fn expand_home(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(r) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(r),
        None => PathBuf::from(s),
    }
}

fn parse_raw_date(s: &str) -> Option<(i64, i32)> {
    let s = s.trim();
    let mut it = s.split_whitespace();
    let t = it.next()?;
    let t: i64 = t.strip_prefix('@').unwrap_or(t).parse().ok()?;
    let off = match it.next() {
        Some(tz) if tz.len() == 5 && (tz.starts_with('+') || tz.starts_with('-')) => {
            let h: i32 = tz[1..3].parse().ok()?;
            let m: i32 = tz[3..5].parse().ok()?;
            let o = h * 60 + m;
            if tz.starts_with('-') { -o } else { o }
        }
        Some(_) => return None,
        None => 0,
    };
    Some((t, off))
}

#[cfg(unix)]
fn file_mode(md: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if md.permissions().mode() & 0o100 != 0 { mode::BLOB_EXEC } else { mode::BLOB }
}
#[cfg(not(unix))]
fn file_mode(_: &fs::Metadata) -> u32 {
    mode::BLOB
}

#[cfg(unix)]
fn mtime(md: &fs::Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (md.mtime() as u32 as i64, md.mtime_nsec() as u32 as i64)
}
#[cfg(not(unix))]
fn mtime(md: &fs::Metadata) -> (i64, i64) {
    let t = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    (t.as_secs() as i64, t.subsec_nanos() as i64)
}

fn md_size(md: &fs::Metadata) -> u64 {
    md.len()
}

#[cfg(unix)]
fn stat_entry(md: &fs::Metadata) -> index::Entry {
    use std::os::unix::fs::MetadataExt;
    index::Entry {
        ctime_s: md.ctime() as u32,
        ctime_ns: md.ctime_nsec() as u32,
        mtime_s: md.mtime() as u32,
        mtime_ns: md.mtime_nsec() as u32,
        dev: md.dev() as u32,
        ino: md.ino() as u32,
        uid: md.uid(),
        gid: md.gid(),
        size: md.size() as u32,
        ..Default::default()
    }
}
#[cfg(not(unix))]
fn stat_entry(md: &fs::Metadata) -> index::Entry {
    let (s, ns) = mtime(md);
    index::Entry { mtime_s: s as u32, mtime_ns: ns as u32, size: md.len() as u32, ..Default::default() }
}
