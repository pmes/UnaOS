// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Mica — CODEX §2's **Ledger** (structured data). UNAOSVOLUME (rmbp-ledger B427): the jobs live on the UnaOS
//! volume, one file per record under `/jobs`, their fields TYPED attributes (`job:*`, `jobs_core`), and Mica is
//! the ONLY writer (R79: one store, one writer; the kernel reads `/jobs` through its VFS). The git text —
//! `docs/dev/STATUS.tsv`, the ledgers' status cells — is the volume's EXPORT, byte-identical to what
//! `tools/status-check.py` reads, so the gate keeps running on the export until UnaOS runs it.
//!
//! The verbs (`mica jobs …`, `src/main.rs`): build, add, cite, verify, list, query, export, witness.
//! Design: `docs/dev/evidence/rmbp-1005/unaosvolume.md`.

use anyhow::{Context, Result, anyhow, bail};
use jobs_core::{self as jc, Kind, Record};
use std::path::{Path, PathBuf};
use unafs::{AttributeValue, BlockDevice, FileKind, MemDevice, UnaFS};

fn fe<E: core::fmt::Debug>(e: E) -> anyhow::Error {
    anyhow!("unafs: {e:?}")
}

/// The git text the volume is built from (and exported back to).
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub status: String,
    /// (track, repo-relative path, text)
    pub ledgers: Vec<(String, String, String)>,
    pub queues: Vec<(String, String, String)>,
}

impl Sources {
    pub fn read(repo: &Path) -> Result<Sources> {
        let rd = |rel: &str| std::fs::read_to_string(repo.join(rel)).with_context(|| format!("read {rel}"));
        let mut s = Sources { status: rd(jc::STATUS_TSV)?, ..Sources::default() };
        for (t, rel) in jc::LEDGERS {
            s.ledgers.push((t.to_string(), rel.to_string(), rd(rel)?));
        }
        for (t, rel) in jc::QUEUES {
            s.queues.push((t.to_string(), rel.to_string(), rd(rel)?));
        }
        Ok(s)
    }

    /// Every record the repo's text holds: claims, then ledger rows, then queue items.
    pub fn records(&self) -> Result<Vec<Record>> {
        let mut out = jc::parse_status_tsv(&self.status).map_err(|e| anyhow!(e))?;
        for (t, _, md) in &self.ledgers {
            out.extend(jc::parse_ledger(t, md));
        }
        for (t, _, md) in &self.queues {
            out.extend(jc::parse_queue(t, md));
        }
        Ok(out)
    }

    /// The volume's EXPORT of `records`: (repo-relative path, bytes) for STATUS.tsv and every ledger.
    pub fn export(&self, records: &[Record]) -> Vec<(String, String)> {
        let mut out = vec![(jc::STATUS_TSV.to_string(), jc::export_status_tsv(records))];
        for (t, rel, md) in &self.ledgers {
            out.push((rel.clone(), jc::export_ledger(t, md, records)));
        }
        out
    }

    /// Is the export of `records` the git text byte for byte?
    pub fn identical(&self, records: &[Record]) -> bool {
        self.export(records).iter().all(|(rel, body)| {
            if rel == jc::STATUS_TSV {
                *body == self.status
            } else {
                self.ledgers.iter().any(|(_, r, md)| r == rel && md == body)
            }
        })
    }
}

/// `mkdir -p` on the volume; returns the leaf's inode.
pub fn mkdirs<D: BlockDevice>(fs: &mut UnaFS<D>, path: &str) -> Result<u64> {
    let mut parent = fs.resolve_path("/").map_err(fe)?;
    let mut at = String::new();
    for c in path.split('/').filter(|c| !c.is_empty()) {
        at.push('/');
        at.push_str(c);
        parent = match fs.resolve_path(&at) {
            Ok(id) => id,
            Err(_) => fs.mkdir(parent, c.to_string()).map_err(fe).with_context(|| format!("mkdir {at}"))?,
        };
    }
    Ok(parent)
}

/// The folders' ATTRCOLUMNS view (`una:view`), so Quarry opens `/jobs/...` with the job columns shown.
fn view_for(dir: &str) -> String {
    // SMALLFIX4 item 13 (ATTRKEYS): the columns are the registered keys, joined — no `job:` literal.
    let cols: &[&str] = if dir.ends_with(jc::STATUS_DIR) {
        &[jc::K_STATUS, jc::K_FLIGHT, jc::K_SET_BY, jc::K_REFS]
    } else if dir.contains("/ledger/") {
        &[jc::K_STATUS, jc::K_OWNER, jc::K_ARC, jc::K_REFS]
    } else {
        &[jc::K_STATUS, jc::K_ARC, jc::K_REFS]
    };
    cols.join(",")
}

/// Write one record (create or replace): the body as the file's data, every `job:*` as an attribute, `job:seq`
/// an Int, `una:type` text. An attribute the record no longer carries is removed.
pub fn store<D: BlockDevice>(fs: &mut UnaFS<D>, r: &Record) -> Result<u64> {
    let dir = r.dir();
    let parent = mkdirs(fs, &dir)?;
    let path = r.path();
    let id = match fs.resolve_path(&path) {
        Ok(id) => {
            fs.truncate_data(id, 0).map_err(fe)?;
            id
        }
        Err(_) => {
            let name = path.rsplit('/').next().unwrap_or_default().to_string();
            let id = fs.create_file(parent, name).map_err(fe).with_context(|| format!("create {path}"))?;
            fs.set_attribute(id, jc::TYPE_KEY.into(), AttributeValue::String(jc::RECORD_TYPE.into())).map_err(fe)?;
            id
        }
    };
    fs.write_data(id, 0, r.body.as_bytes()).map_err(fe)?;
    let want = r.volume_attrs();
    for k in jc::STRING_KEYS {
        if !want.iter().any(|(a, _)| a == k) && fs.get_attribute(id, k).map_err(fe)?.is_some() {
            fs.remove_attribute(id, k).map_err(fe)?;
        }
    }
    for (k, v) in want {
        fs.set_attribute(id, k, AttributeValue::String(v)).map_err(fe)?;
    }
    fs.set_attribute(id, jc::K_SEQ.into(), AttributeValue::Int(r.seq)).map_err(fe)?;
    Ok(id)
}

/// BUILD: every record onto the volume, the folders' views, the saved queries. One commit at the end.
pub fn populate<D: BlockDevice>(fs: &mut UnaFS<D>, records: &[Record]) -> Result<()> {
    fs.set_autocommit(false);
    let mut dirs: Vec<String> = Vec::new();
    for r in records {
        store(fs, r)?;
        let d = r.dir();
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    for d in &dirs {
        let id = fs.resolve_path(d).map_err(fe)?;
        fs.set_attribute(id, jc::VIEW_KEY.into(), AttributeValue::String(view_for(d).into())).map_err(fe)?;
    }
    let qdir = format!("{}/{}", jc::ROOT, jc::QUERIES_DIR);
    let qid = mkdirs(fs, &qdir)?;
    for (name, text) in jc::saved_queries() { // SMALLFIX4 item 13: built from the registered key
        let id = match fs.resolve_path(&format!("{qdir}/{name}")) {
            Ok(id) => {
                fs.truncate_data(id, 0).map_err(fe)?;
                id
            }
            Err(_) => fs.create_file(qid, name.to_string()).map_err(fe)?,
        };
        fs.write_data(id, 0, text.as_bytes()).map_err(fe)?;
        fs.set_attribute(id, jc::TYPE_KEY.into(), AttributeValue::String(jc::QUERY_TYPE.into())).map_err(fe)?;
    }
    fs.commit().map_err(fe)?;
    fs.set_autocommit(true);
    Ok(())
}

fn read_record<D: BlockDevice>(fs: &mut UnaFS<D>, id: u64) -> Result<Option<Record>> {
    let ino = fs.read_inode(id).map_err(fe)?;
    let mut keys: Vec<String> = ino.attributes.keys().cloned().collect();
    keys.extend(ino.large_attributes.keys().cloned());
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut seq = 0i64;
    for k in keys.iter().filter(|k| k.starts_with(jc::job_ns())) {
        match fs.get_attribute(id, k).map_err(fe)? {
            Some(AttributeValue::String(s)) => attrs.push((k.clone(), s)),
            Some(AttributeValue::Int(i)) if k == jc::K_SEQ => seq = i,
            _ => {}
        }
    }
    let body = fs.read_data(id, 0, ino.size).map_err(fe)?;
    let body = String::from_utf8(body).map_err(|_| anyhow!("inode {id}: body is not UTF-8"))?;
    Ok(jc::record_from_volume(&attrs, seq, body))
}

fn walk<D: BlockDevice>(fs: &mut UnaFS<D>, dir: &str, out: &mut Vec<Record>) -> Result<()> {
    let Ok(id) = fs.resolve_path(dir) else { return Ok(()) };
    for e in fs.ls(id).map_err(fe)? {
        match e.kind {
            FileKind::Directory => walk(fs, &format!("{dir}/{}", e.name), out)?,
            FileKind::File => {
                if let Some(r) = read_record(fs, e.inode_id)? {
                    out.push(r);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// LOAD: every record the volume holds (claims, ledger rows, queue items), in source order per kind.
pub fn load<D: BlockDevice>(fs: &mut UnaFS<D>) -> Result<Vec<Record>> {
    let mut out = Vec::new();
    for sub in [jc::STATUS_DIR, jc::LEDGER_DIR, jc::QUEUE_DIR] {
        walk(fs, &format!("{}/{}", jc::ROOT, sub), &mut out)?;
    }
    out.sort_by(|a, b| (a.kind as u8, a.track.as_str(), a.seq).cmp(&(b.kind as u8, b.track.as_str(), b.seq)));
    Ok(out)
}

/// VIEW (JOBSUI, rmbp-ledger B511) — Mica's Jobs view: the counts at the top, the queue's open items by rank below
/// (name, track, status, the flight that last touched it). The SAME `jobs_core` shape the kernel draws (the `jobs`
/// verb, Quarry's status line under `/jobs`); `track` narrows the items to one queue, `n` caps them.
pub fn view_lines(records: &[Record], track: Option<&str>, n: usize) -> Vec<String> {
    let c = jc::census(records);
    let mut out = vec![jc::counts_line(Some([c.records as u64, c.claims as u64, c.ledger as u64, c.queue as u64]))];
    let rank = |t: &str| jc::QUEUES.iter().position(|(q, _)| *q == t).unwrap_or(usize::MAX);
    let mut items: Vec<&Record> = records
        .iter()
        .filter(|r| r.kind == Kind::Queue && track.is_none_or(|t| r.track == t) && jc::shown_open(r.get(jc::K_STATUS), &r.body))
        .collect();
    items.sort_by_key(|r| (rank(&r.track), r.seq));
    for (i, r) in items.iter().take(n).enumerate() {
        let flight = jc::last_flight(r.get(jc::K_FLIGHT), &r.body);
        out.push(jc::item_row(i + 1, &jc::queue_name(&r.id).1, &r.track, r.get(jc::K_STATUS), &flight));
    }
    out
}

/// A flight's captures (status-check.py `flight_files`): `docs/dev/evidence/**/f<N>-boot*.log` and
/// `FLIGHT<a>[-<b>].md` covering N.
pub fn flight_files(repo: &Path, n: u64) -> Vec<PathBuf> {
    flight_files_of(repo, 'f', n)
}

/// STATUSORIN (B476, status-check.py `flight_files`): `kind` is `'f'` (rMBP: `f<N>-boot*.log`, `FLIGHT<a>[-<b>].md`)
/// or `'r'` (Orin render N: `orin*/**/render<N>-*.log`, `boot-render<N>-*.log`, `FLIGHT-RESULT-render<N>.md`).
pub fn flight_files_of(repo: &Path, kind: char, n: u64) -> Vec<PathBuf> {
    fn rec(dir: &Path, n: u64, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(&p, n, out);
                continue;
            }
            let b = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = b.strip_prefix('f') {
                let d: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                let tail = &rest[d.len()..];
                if !d.is_empty() && tail.starts_with("-boot") && tail.ends_with(".log") && !tail[5..].contains('/') && d.parse() == Ok(n) {
                    out.push(p);
                    continue;
                }
            }
            if let Some(rest) = b.strip_prefix("FLIGHT").and_then(|r| r.strip_suffix(".md")) {
                let (a, z) = rest.split_once('-').unwrap_or((rest, rest));
                if let (Ok(a), Ok(z)) = (a.parse::<u64>(), z.parse::<u64>()) {
                    if a <= n && n <= z && a.to_string() == rest.split('-').next().unwrap_or("") {
                        out.push(p);
                    }
                }
            }
        }
    }
    fn rec_orin(dir: &Path, n: u64, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                rec_orin(&p, n, out);
                continue;
            }
            let b = e.file_name().to_string_lossy().to_string();
            let want = format!("render{}-", n);
            let is_log = b.ends_with(".log") && (b.starts_with(&want) || b.strip_prefix("boot-").is_some_and(|r| r.starts_with(&want)));
            if is_log || b == format!("FLIGHT-RESULT-render{}.md", n) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    if kind == 'r' {
        let Ok(rd) = std::fs::read_dir(repo.join("docs/dev/evidence")) else { return out };
        for e in rd.flatten() {
            if e.path().is_dir() && e.file_name().to_string_lossy().starts_with("orin") {
                rec_orin(&e.path(), n, &mut out);
            }
        }
    } else {
        rec(&repo.join("docs/dev/evidence"), n, &mut out);
    }
    out.sort();
    out
}

/// VERIFY one claim's quoted line against its flight's captures. `None` = the claim quotes no line.
pub fn verify(repo: &Path, r: &Record) -> Option<bool> {
    let (line, fl) = (r.get(jc::K_LINE), r.get(jc::K_FLIGHT));
    if line.is_empty() {
        return None;
    }
    if !jc::flight_ok(fl) {
        return Some(false);
    }
    let n: u64 = fl[1..].parse().ok()?;
    let blobs: Vec<Vec<u8>> = flight_files_of(repo, fl.chars().next().unwrap_or('f'), n).iter().filter_map(|p| std::fs::read(p).ok()).collect();
    let refs: Vec<&[u8]> = blobs.iter().map(Vec::as_slice).collect();
    Some(jc::verify_line(line, &refs))
}

/// VERIFY every claim: (verified, quoted, the ids that failed).
pub fn verify_all(repo: &Path, records: &[Record]) -> (usize, usize, Vec<String>) {
    let (mut ok, mut n, mut bad) = (0, 0, Vec::new());
    for r in records.iter().filter(|r| r.kind == Kind::Claim) {
        match verify(repo, r) {
            Some(true) => {
                ok += 1;
                n += 1
            }
            Some(false) => {
                n += 1;
                bad.push(r.id.clone())
            }
            None => {}
        }
    }
    (ok, n, bad)
}

/// QUERY: `status=confirmed flight=f24` (or UnaFS syntax) over the volume's index → the matching records' paths.
pub fn query<D: BlockDevice>(fs: &mut UnaFS<D>, q: &str) -> Result<Vec<String>> {
    let text = jc::query_text(q).map_err(|e| anyhow!(e))?;
    let hits = fs.query(&text).map_err(fe)?;
    let mut out: Vec<String> =
        hits.into_iter().map(|h| h.path).filter(|p| p.starts_with(jc::ROOT)).collect();
    out.sort();
    Ok(out)
}

/// A fresh in-memory volume (the witness and the tests).
pub fn mem_volume(size_mb: u64) -> Result<UnaFS<MemDevice>> {
    let mut dev = MemDevice::with_blocks(size_mb * 1024 * 1024 / unafs::BLOCK_SIZE);
    unafs::format(&mut dev, &unafs::FormatParams::sized_mb(size_mb)).map_err(fe)?;
    UnaFS::mount(dev).map_err(fe)
}

/// The volume size a build of `records` asks for (bodies + an inode block each + headroom), floor 32 MiB.
pub fn size_mb_for(records: &[Record]) -> u64 {
    let bytes: u64 = records.iter().map(|r| r.body.len() as u64 + r.get(jc::K_LINE).len() as u64 + 3 * 4096).sum();
    (bytes * 3 / (1024 * 1024) + 16).max(32)
}

/// The host witness: repo → volume → export, compared byte for byte; every quoted line verified.
pub struct Witness {
    pub census: jc::Census,
    pub identical: bool,
    pub verified: usize,
    pub quoted: usize,
    pub failed: Vec<String>,
}

impl Witness {
    pub fn line(&self) -> String {
        jc::witness(&self.census, self.identical, self.verified, self.quoted)
    }
}

/// Round-trip the repo through a volume (`fs`, already holding the records or about to): load back, export, compare.
pub fn witness_on<D: BlockDevice>(repo: &Path, src: &Sources, fs: &mut UnaFS<D>) -> Result<Witness> {
    let back = load(fs)?;
    let (verified, quoted, failed) = verify_all(repo, &back);
    Ok(Witness { census: jc::census(&back), identical: src.identical(&back), verified, quoted, failed })
}

/// `mica jobs witness`: the whole round trip in memory.
pub fn witness(repo: &Path) -> Result<Witness> {
    let src = Sources::read(repo)?;
    let recs = src.records()?;
    let mut fs = mem_volume(size_mb_for(&recs))?;
    populate(&mut fs, &recs)?;
    witness_on(repo, &src, &mut fs)
}

/// ADD a claim: the next `ST<n>`, status `open` (a claim is born open; `cite` moves it).
pub fn add_claim<D: BlockDevice>(fs: &mut UnaFS<D>, claim: &str, set_by: &str, refs: &str) -> Result<Record> {
    if claim.is_empty() || set_by.is_empty() || claim.contains(['\t', '\n']) || set_by.contains(['\t', '\n']) {
        bail!("a claim and a set-by are required, without tabs or newlines");
    }
    let all = load(fs)?;
    let id = jc::next_claim_id(&all);
    let seq = all.iter().filter(|r| r.kind == Kind::Claim).map(|r| r.seq).max().unwrap_or(0) + 1;
    let mut r = Record::new(Kind::Claim, &id, "", seq, claim.to_string());
    r.set(jc::K_STATUS, "open");
    r.set(jc::K_SET_BY, set_by);
    r.set(jc::K_REFS, refs);
    store(fs, &r)?;
    fs.commit().map_err(fe)?;
    Ok(r)
}

/// CITE a claim on the volume: the gate's rules first (`jobs_core::cite`), the line verified on the flight's capture.
pub fn cite_claim<D: BlockDevice>(
    repo: &Path,
    fs: &mut UnaFS<D>,
    id: &str,
    status: &str,
    flight: &str,
    line: &str,
    set_by: &str,
) -> Result<Record> {
    let path = format!("{}/{}/{}", jc::ROOT, jc::STATUS_DIR, id);
    let ino = fs.resolve_path(&path).map_err(|_| anyhow!("{path}: no such claim"))?;
    let mut r = read_record(fs, ino)?.ok_or_else(|| anyhow!("{path}: not a job record"))?;
    let mut probe = r.clone();
    probe.set(jc::K_FLIGHT, flight);
    probe.set(jc::K_LINE, line);
    let verified = verify(repo, &probe).unwrap_or(true);
    jc::cite(&mut r, status, flight, line, verified, set_by).map_err(|e| anyhow!("{id}: {e}"))?;
    store(fs, &r)?;
    fs.commit().map_err(fe)?;
    Ok(r)
}

// ── JOBSNEXT (rmbp-ledger B506): next / cut / land / owed ──────────────────────────────────────────────────────
// Peter, 2026-10-07: "why do i have to tell you that there's more to do after every single boot". The ranking is
// `jobs_core::rank_next` (the kernel's `jobs next` calls the same function); Mica adds the repo's halves: the
// flight's §3 from `docs/dev/evidence/**/FLIGHT<n>.md`, the brief path, and the writes (`/jobs/owed`, cut, land).

/// The FLIGHT<n>.md that covers `want` (exact `FLIGHT<n>.md` first, else a `FLIGHT<a>-<b>.md` range), or with
/// `None` the newest flight the evidence folder holds: (n, path).
pub fn flight_md(repo: &Path, want: Option<u64>) -> Option<(u64, PathBuf)> {
    fn rec(dir: &Path, out: &mut Vec<(u64, u64, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(&p, out);
                continue;
            }
            let b = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = b.strip_prefix("FLIGHT").and_then(|r| r.strip_suffix(".md")) {
                let (a, z) = rest.split_once('-').unwrap_or((rest, rest));
                if let (Ok(a), Ok(z)) = (a.parse::<u64>(), z.parse::<u64>()) {
                    out.push((a, z, p));
                }
            }
        }
    }
    let mut all = Vec::new();
    rec(&repo.join("docs/dev/evidence"), &mut all);
    all.sort();
    match want {
        Some(n) => all
            .iter()
            .find(|(a, z, _)| *a == n && *z == n)
            .or_else(|| all.iter().find(|(a, z, _)| *a <= n && n <= *z))
            .map(|(_, _, p)| (n, p.clone())),
        None => all.into_iter().max_by_key(|(_, z, _)| *z).map(|(_, z, p)| (z, p)),
    }
}

/// The flight's §3 names off the repo: (`f<n>`, names).
pub fn owed_from_repo(repo: &Path, want: Option<u64>) -> Result<(String, Vec<String>)> {
    let Some((n, p)) = flight_md(repo, want) else {
        return match want {
            Some(n) => bail!("no FLIGHT{n}.md under docs/dev/evidence"),
            None => Ok((String::new(), Vec::new())),
        };
    };
    let md = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    Ok((format!("f{n}"), jc::owed_names(&md)))
}

fn owed_path() -> String {
    format!("{}/{}", jc::ROOT, jc::OWED_FILE)
}

/// Write `/jobs/owed`: the names one per line, `job:flight` = the flight, typed text.
pub fn write_owed<D: BlockDevice>(fs: &mut UnaFS<D>, flight: &str, names: &[String]) -> Result<()> {
    let root = mkdirs(fs, jc::ROOT)?;
    let path = owed_path();
    let id = match fs.resolve_path(&path) {
        Ok(id) => {
            fs.truncate_data(id, 0).map_err(fe)?;
            id
        }
        Err(_) => {
            let id = fs.create_file(root, jc::OWED_FILE.to_string()).map_err(fe)?;
            fs.set_attribute(id, jc::TYPE_KEY.into(), AttributeValue::String(jc::RECORD_TYPE.into())).map_err(fe)?;
            id
        }
    };
    let mut body = names.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    fs.write_data(id, 0, body.as_bytes()).map_err(fe)?;
    fs.set_attribute(id, jc::K_FLIGHT.into(), AttributeValue::String(flight.into())).map_err(fe)?;
    fs.commit().map_err(fe)?;
    Ok(())
}

/// Read `/jobs/owed` back: (`f<n>`, names), or `None` when the volume predates it.
pub fn read_owed<D: BlockDevice>(fs: &mut UnaFS<D>) -> Result<Option<(String, Vec<String>)>> {
    let Ok(id) = fs.resolve_path(&owed_path()) else { return Ok(None) };
    let ino = fs.read_inode(id).map_err(fe)?;
    let body = String::from_utf8(fs.read_data(id, 0, ino.size).map_err(fe)?).map_err(|_| anyhow!("/jobs/owed is not UTF-8"))?;
    let flight = match fs.get_attribute(id, jc::K_FLIGHT).map_err(fe)? {
        Some(AttributeValue::String(s)) => s,
        _ => String::new(),
    };
    Ok(Some((flight, body.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())))
}

/// The brief an arc has: `docs/dev/evidence/**/<NAME>.md` (case-insensitive stem), repo-relative, newest folder first.
pub struct Briefs(Vec<(String, String)>);

impl Briefs {
    pub fn index(repo: &Path) -> Briefs {
        fn rec(repo: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    rec(repo, &p, out);
                } else if let Some(stem) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".md")) {
                    let rel = p.strip_prefix(repo).unwrap_or(&p).to_string_lossy().to_string();
                    out.push((stem.to_ascii_uppercase(), rel));
                }
            }
        }
        let mut v = Vec::new();
        rec(repo, &repo.join("docs/dev/evidence"), &mut v);
        v.sort_by(|a, b| b.1.cmp(&a.1));
        Briefs(v)
    }
    pub fn get(&self, name: &str) -> &str {
        if name.is_empty() {
            return "";
        }
        self.0.iter().find(|(s, _)| s == &name.to_ascii_uppercase()).map(|(_, p)| p.as_str()).unwrap_or("")
    }
}

/// `mica jobs next`: the header and the ranked rows (the same `jobs_core` lines the kernel prints, plus the brief).
pub fn next_lines(records: &[Record], flight: &str, owed: &[String], track: &str, n: usize, briefs: &Briefs) -> Vec<String> {
    let ranked = jc::rank_next(records, owed, track, n);
    let mut out = vec![jc::next_header(records, &ranked, flight, owed.len(), track)];
    for (k, t) in ranked.iter().enumerate() {
        out.push(jc::next_row(records, k + 1, t, briefs.get(&t.name)));
    }
    out
}

/// Find one record by id (`<id>` or `<track>/<id>`; a claim by `ST<n>`). Refuses an id two tracks share.
pub fn find<D: BlockDevice>(fs: &mut UnaFS<D>, want: &str, track: &str) -> Result<Record> {
    let (t, id) = want.split_once('/').unwrap_or((track, want));
    let all = load(fs)?;
    let hits: Vec<&Record> = all.iter().filter(|r| r.id == id && (t.is_empty() || r.track == t || r.kind == Kind::Claim)).collect();
    match hits.as_slice() {
        [r] => Ok((*r).clone()),
        [] => bail!("{want}: no such job (track {})", if t.is_empty() { "any" } else { t }),
        _ => bail!("{want}: {} records carry this id — name it <track>/<id>", hits.len()),
    }
}

/// CUT: the executor's cut — `job:status=open`, `job:arc`, `job:branch` (a cut item leaves `next`).
pub fn cut<D: BlockDevice>(fs: &mut UnaFS<D>, want: &str, track: &str, arc: &str, branch: &str) -> Result<Record> {
    if branch.is_empty() || branch.contains(['\t', '\n']) || arc.contains(['\t', '\n']) {
        bail!("cut needs --branch <executor branch> (and an --arc without tabs)");
    }
    let mut r = find(fs, want, track)?;
    if r.kind == Kind::Claim {
        bail!("{}: a claim is cited, not cut", r.id);
    }
    r.set(jc::K_STATUS, "open");
    if !arc.is_empty() {
        r.set(jc::K_ARC, arc);
    }
    r.set(jc::K_BRANCH, branch);
    store(fs, &r)?;
    fs.commit().map_err(fe)?;
    Ok(r)
}

/// LAND: the hand-back — `job:status` (the closed set) and `job:tip`. The row's TEXT is untouched, so the ledger
/// cell and STATUS.tsv still come from `export` (the gate reads the export).
pub fn land<D: BlockDevice>(fs: &mut UnaFS<D>, want: &str, track: &str, status: &str, tip: &str) -> Result<Record> {
    if !jc::status_ok(status) || status == "-" {
        bail!("status '{status}' not in {}", jc::STATUSES[..jc::STATUSES.len() - 1].join("/"));
    }
    if tip.len() < 7 || !tip.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--tip '{tip}' is not a commit sha");
    }
    let mut r = find(fs, want, track)?;
    if r.kind == Kind::Claim {
        bail!("{}: a claim is cited (`mica jobs cite`), not landed", r.id);
    }
    r.set(jc::K_STATUS, status);
    r.set(jc::K_TIP, tip);
    store(fs, &r)?;
    fs.commit().map_err(fe)?;
    Ok(r)
}
