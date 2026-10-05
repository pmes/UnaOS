// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Remotes over HTTP(S) (feature `http`): clone, fetch and push through http_core's host
//! transport (SR51 — UnaOS's own HTTP/1.1 and TLS 1.3), speaking protocol v2 for fetch and the
//! receive-pack protocol (v0, the only one git defines for push) with `report-status`; and the
//! dumb HTTP protocol for servers without a git backend.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

use http_core::host::{Agent, AgentConfig, Request};
use http_core::Url;

use crate::hash::{HashKind, ObjectId};
use crate::object::{Commit, Kind, Tag, Tree};
use crate::pack::{self, PackObject, WriteOptions};
use crate::protocol::{self, Capabilities, Command, FetchArgs, RemoteRef};
use crate::refs::{PackedRef, PackedRefs};
use crate::repo::Repository;
use crate::{Error, Result};

fn herr(e: http_core::Error) -> Error {
    Error::Io(format!("http: {e:?}"))
}

/// A smart or dumb HTTP remote.
pub struct Remote {
    agent: Agent,
    base: String,
    /// Requests made (for tests and diagnostics).
    pub requests: std::cell::Cell<usize>,
}

impl Remote {
    /// A remote at `url` (`http://` or `https://`).
    pub fn new(url: &str) -> Self {
        let cfg = AgentConfig { user_agent: Some(protocol::AGENT.to_string()), ..Default::default() };
        Remote { agent: Agent::new(cfg), base: url.trim_end_matches('/').to_string(), requests: std::cell::Cell::new(0) }
    }

    fn exchange(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Result<(u16, Vec<u8>)> {
        self.requests.set(self.requests.get() + 1);
        let url = Url::parse(&format!("{}/{}", self.base, path)).map_err(|e| Error::Io(format!("url: {e:?}")))?;
        let mut req = Request::new(method, url);
        for (k, v) in headers {
            req.headers.set(k, v).map_err(|e| Error::Io(format!("header: {e:?}")))?;
        }
        if method == "POST" {
            req.headers.set("Content-Length", &body.len().to_string()).ok();
        }
        req.body = std::sync::Arc::new(body);
        let resp = self.agent.send(req).map_err(herr)?;
        let st = resp.status();
        let b = resp.bytes().map_err(herr)?;
        Ok((st, b))
    }

    fn get_ok(&self, path: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>> {
        let (st, b) = self.exchange("GET", path, headers, Vec::new())?;
        if st != 200 {
            return Err(Error::Io(format!("GET {path}: HTTP {st}")));
        }
        Ok(b)
    }

    /// The protocol-v2 capability advertisement of `git-upload-pack`.
    pub fn capabilities(&self) -> Result<Capabilities> {
        let b = self.get_ok("info/refs?service=git-upload-pack", &[("Git-Protocol", "version=2")])?;
        Capabilities::parse(&b)
    }

    fn post_upload(&self, body: Vec<u8>) -> Result<Vec<u8>> {
        let (st, b) = self.exchange(
            "POST",
            "git-upload-pack",
            &[("Git-Protocol", "version=2"), ("Content-Type", "application/x-git-upload-pack-request"), ("Accept", "application/x-git-upload-pack-result")],
            body,
        )?;
        if st != 200 {
            return Err(Error::Io(format!("POST git-upload-pack: HTTP {st}: {}", String::from_utf8_lossy(&b))));
        }
        Ok(b)
    }

    /// `ls-refs`.
    pub fn ls_refs(&self, caps: &Capabilities, prefixes: &[&str]) -> Result<Vec<RemoteRef>> {
        let hk = caps.object_format();
        let b = self.post_upload(protocol::ls_refs_request(hk, caps, prefixes))?;
        protocol::parse_ls_refs(hk, &b)
    }

    /// One `fetch` exchange.
    pub fn fetch_raw(&self, caps: &Capabilities, args: &FetchArgs) -> Result<protocol::FetchResponse> {
        let hk = caps.object_format();
        let b = self.post_upload(protocol::fetch_request(hk, caps, args))?;
        protocol::parse_fetch_response(hk, &b)
    }
}

/// Clone options.
#[derive(Debug, Clone, Default)]
pub struct CloneOptions {
    /// `--bare`: refs land under `refs/heads`, no worktree.
    pub bare: bool,
    /// `--depth`.
    pub depth: Option<u32>,
}

/// What a clone or fetch brought.
#[derive(Debug, Clone, Default)]
pub struct Transfer {
    /// Objects received.
    pub objects: usize,
    /// Pack bytes received.
    pub pack_bytes: usize,
    /// Refs written.
    pub refs: Vec<(String, ObjectId)>,
    /// HTTP requests made.
    pub requests: usize,
}

fn receive_pack(repo: &Repository, packbytes: &[u8]) -> Result<usize> {
    if packbytes.is_empty() {
        return Ok(0);
    }
    let res = pack::index_pack(packbytes, repo.hash, &mut |id| repo.read(id).ok(), &mut |_, _, _| Ok(()))?;
    if !res.external_bases.is_empty() {
        return Err(Error::Unsupported("thin pack received (not requested)"));
    }
    repo.install_pack(packbytes, &res.idx(repo.hash))?;
    Ok(res.objects.len())
}

fn write_shallow(repo: &Repository, add: &[ObjectId], remove: &[ObjectId]) -> Result<()> {
    let p = repo.common_dir.join("shallow");
    let mut set: BTreeSet<ObjectId> = BTreeSet::new();
    if let Ok(s) = fs::read_to_string(&p) {
        for l in s.lines() {
            if let Some(id) = ObjectId::from_hex_kind(repo.hash, l.trim().as_bytes()) {
                set.insert(id);
            }
        }
    }
    set.extend(add.iter().copied());
    for r in remove {
        set.remove(r);
    }
    if set.is_empty() {
        let _ = fs::remove_file(&p);
        return Ok(());
    }
    let s: String = set.iter().map(|i| format!("{i}\n")).collect();
    fs::write(&p, s).map_err(|e| Error::Io(format!("write shallow: {e}")))
}

/// `git clone <url> <dir>` over smart HTTP (protocol v2).
pub fn clone(url: &str, dir: &Path, opts: &CloneOptions) -> Result<(Repository, Transfer)> {
    let remote = Remote::new(url);
    let caps = remote.capabilities()?;
    let hk = caps.object_format();
    let refs = remote.ls_refs(&caps, &["HEAD", "refs/heads/", "refs/tags/"])?;
    let head_target = refs.iter().find(|r| r.name == "HEAD").and_then(|r| r.symref_target.clone()).unwrap_or_else(|| "refs/heads/main".into());
    let branch = head_target.strip_prefix("refs/heads/").unwrap_or("main").to_string();
    let repo = Repository::init(dir, opts.bare, hk, &branch)?;
    // `--depth` implies `--single-branch` (git-clone(1)): only HEAD's branch is wanted; tags come
    // along through include-tag when they point into what was fetched.
    let single = opts.depth.is_some();
    let mut wants: Vec<ObjectId> = Vec::new();
    for r in &refs {
        if r.name == "HEAD" || !(r.name.starts_with("refs/heads/") || r.name.starts_with("refs/tags/")) {
            continue;
        }
        if single && r.name != head_target {
            continue;
        }
        if let Some(id) = r.id {
            if !wants.contains(&id) {
                wants.push(id);
            }
        }
    }
    let mut t = Transfer::default();
    if !wants.is_empty() {
        let args = FetchArgs { wants, done: true, depth: opts.depth, no_progress: true, include_tag: true, ..Default::default() };
        let resp = remote.fetch_raw(&caps, &args)?;
        t.pack_bytes = resp.pack.len();
        t.objects = receive_pack(&repo, &resp.pack)?;
        if !resp.shallow.is_empty() || !resp.unshallow.is_empty() {
            write_shallow(&repo, &resp.shallow, &resp.unshallow)?;
        }
    }
    // Refs: bare → refs/heads + refs/tags in packed-refs (as git's initial transaction writes
    // them); non-bare → refs/remotes/origin/*, refs/tags/*, and the local branch.
    let mut packed = PackedRefs { traits: Some(PackedRefs::standard_traits()), refs: Vec::new() };
    for r in &refs {
        let Some(id) = r.id else { continue };
        if single && r.name != head_target && !(r.name.starts_with("refs/tags/") && repo.has(&id)) {
            continue;
        }
        let name = if r.name.starts_with("refs/tags/") {
            r.name.clone()
        } else if let Some(b) = r.name.strip_prefix("refs/heads/") {
            if opts.bare { r.name.clone() } else { format!("refs/remotes/origin/{b}") }
        } else {
            continue;
        };
        let peeled = if r.peeled.is_some() { r.peeled } else { None };
        packed.upsert(PackedRef { name: name.clone().into_bytes(), id, peeled });
        t.refs.push((name, id));
    }
    fs::write(repo.common_dir.join("packed-refs"), packed.serialize()).map_err(|e| Error::Io(format!("packed-refs: {e}")))?;
    let mut cfg = fs::read(repo.common_dir.join("config")).unwrap_or_default();
    cfg = crate::config::set(&cfg, b"remote.origin.url", url.as_bytes())?;
    if opts.bare {
    } else {
        cfg = crate::config::set(&cfg, b"remote.origin.fetch", b"+refs/heads/*:refs/remotes/origin/*")?;
        cfg = crate::config::set(&cfg, format!("branch.{branch}.remote").as_bytes(), b"origin")?;
        cfg = crate::config::set(&cfg, format!("branch.{branch}.merge").as_bytes(), head_target.as_bytes())?;
    }
    fs::write(repo.common_dir.join("config"), cfg).map_err(|e| Error::Io(format!("config: {e}")))?;
    let repo = Repository::open(&repo.git_dir)?;
    if !opts.bare {
        if let Some(id) = refs.iter().find(|r| r.name == head_target).and_then(|r| r.id) {
            let who = crate::object::Signature::new(b"git_core", b"git_core@unaos", 0, 0);
            repo.update_ref(&head_target, id, None, &who, &format!("clone: from {url}"))?;
            let (_, d) = repo.read(&id)?;
            let tree = Commit::parse(hk, &d)?.tree().ok_or(Error::Corrupt("commit tree"))?;
            repo.checkout_tree(&tree)?;
            fs::write(repo.common_dir.join("refs/remotes/origin/HEAD"), format!("ref: refs/remotes/origin/{branch}\n")).ok();
        }
    }
    t.requests = remote.requests.get();
    Ok((repo, t))
}

/// Up to `limit` commits reachable from `tips`, newest first (date order by walk).
fn recent_commits(repo: &Repository, tips: &[ObjectId], limit: usize) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue: Vec<ObjectId> = tips.to_vec();
    while let Some(id) = queue.pop() {
        if out.len() >= limit || !seen.insert(id) {
            continue;
        }
        let Ok((Kind::Commit, d)) = repo.read(&id) else { continue };
        out.push(id);
        if let Ok(c) = Commit::parse(repo.hash, &d) {
            queue.extend(c.parents());
        }
    }
    out
}

/// `git fetch <url>`: refs/heads/* land under `dst_prefix` (e.g. `refs/remotes/origin/` or
/// `refs/heads/` for a bare mirror), tags under refs/tags/. Negotiates with `have` lines (one
/// round without `done`, then the final request with `done`).
pub fn fetch(repo: &Repository, url: &str, dst_prefix: &str) -> Result<Transfer> {
    let remote = Remote::new(url);
    let caps = remote.capabilities()?;
    if caps.object_format() != repo.hash {
        return Err(Error::Unsupported("remote object format differs"));
    }
    let refs = remote.ls_refs(&caps, &["refs/heads/", "refs/tags/"])?;
    let wants: Vec<ObjectId> = {
        let mut v = Vec::new();
        for r in &refs {
            if let Some(id) = r.id {
                if !repo.has(&id) && !v.contains(&id) {
                    v.push(id);
                }
            }
        }
        v
    };
    let mut t = Transfer::default();
    if !wants.is_empty() {
        let local: Vec<ObjectId> = repo.refs("refs/")?.into_iter().map(|(_, id)| id).collect();
        let haves = recent_commits(repo, &local, 256);
        let shallow: Vec<ObjectId> = fs::read_to_string(repo.common_dir.join("shallow")).map(|s| s.lines().filter_map(|l| ObjectId::from_hex_kind(repo.hash, l.trim().as_bytes())).collect()).unwrap_or_default();
        let mut common = Vec::new();
        if !haves.is_empty() {
            let probe = FetchArgs { wants: wants.clone(), haves: haves.clone(), done: false, no_progress: true, shallow: shallow.clone(), ..Default::default() };
            let r = remote.fetch_raw(&caps, &probe)?;
            common = r.acks.clone();
            if !r.pack.is_empty() {
                // the server decided it was ready and sent the pack already
                t.pack_bytes = r.pack.len();
                t.objects = receive_pack(repo, &r.pack)?;
            }
        }
        if t.pack_bytes == 0 {
            let args = FetchArgs { wants, haves: if common.is_empty() { haves } else { common }, done: true, no_progress: true, include_tag: true, shallow, ..Default::default() };
            let resp = remote.fetch_raw(&caps, &args)?;
            t.pack_bytes = resp.pack.len();
            t.objects = receive_pack(repo, &resp.pack)?;
        }
    }
    let who = crate::object::Signature::new(b"git_core", b"git_core@unaos", 0, 0);
    for r in &refs {
        let Some(id) = r.id else { continue };
        let name = match r.name.strip_prefix("refs/heads/") {
            Some(b) => format!("{dst_prefix}{b}"),
            None => r.name.clone(),
        };
        if repo.resolve_ref(&name)?.and_then(|(_, i)| i) != Some(id) {
            repo.update_ref(&name, id, None, &who, &format!("fetch: {url}"))?;
            t.refs.push((name, id));
        }
    }
    t.requests = remote.requests.get();
    Ok(t)
}

fn reachable(repo: &Repository, tips: &[ObjectId], stop_commits: &BTreeSet<ObjectId>, stop_objects: &BTreeSet<ObjectId>) -> Result<Vec<(ObjectId, Kind, Vec<u8>, Option<Vec<u8>>)>> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    let mut commits = tips.to_vec();
    let mut trees: Vec<(ObjectId, Vec<u8>)> = Vec::new();
    while let Some(id) = commits.pop() {
        if stop_commits.contains(&id) || !seen.insert(id) {
            continue;
        }
        let (k, d) = repo.read(&id)?;
        match k {
            Kind::Commit => {
                let c = Commit::parse(repo.hash, &d)?;
                commits.extend(c.parents());
                trees.push((c.tree().ok_or(Error::Corrupt("commit tree"))?, Vec::new()));
            }
            Kind::Tag => commits.push(Tag::parse(repo.hash, &d)?.target().ok_or(Error::Corrupt("tag target"))?),
            _ => {}
        }
        out.push((id, k, d, None));
    }
    while let Some((id, path)) = trees.pop() {
        if stop_objects.contains(&id) || !seen.insert(id) {
            continue;
        }
        let (k, d) = repo.read(&id)?;
        if k == Kind::Tree {
            for e in Tree::parse(repo.hash, &d)?.entries {
                if e.is_gitlink() {
                    continue;
                }
                let mut p = path.clone();
                if !p.is_empty() {
                    p.push(b'/');
                }
                p.extend_from_slice(&e.name);
                trees.push((e.id, p));
            }
        }
        out.push((id, k, d, Some(path)));
    }
    Ok(out)
}

fn all_tree_objects(repo: &Repository, commit: &ObjectId, set: &mut BTreeSet<ObjectId>) -> Result<()> {
    let (_, d) = repo.read(commit)?;
    let mut stack = vec![Commit::parse(repo.hash, &d)?.tree().ok_or(Error::Corrupt("commit tree"))?];
    while let Some(id) = stack.pop() {
        if !set.insert(id) {
            continue;
        }
        let (k, d) = repo.read(&id)?;
        if k == Kind::Tree {
            for e in Tree::parse(repo.hash, &d)?.entries {
                if !e.is_gitlink() {
                    stack.push(e.id);
                }
            }
        }
    }
    Ok(())
}

/// `git push <url> <local>:<remote>` for each (local ref or rev, remote ref) pair; returns the
/// server's report. As git's client does, a non-fast-forward update is refused locally unless the
/// remote ref is written `+refs/...` (force).
pub fn push(repo: &Repository, url: &str, specs: &[(&str, &str)]) -> Result<protocol::Report> {
    let remote = Remote::new(url);
    let b = remote.get_ok("info/refs?service=git-receive-pack", &[])?;
    let adv = protocol::parse_v0_advert(repo.hash, &b)?;
    let remote_refs: BTreeMap<String, ObjectId> = adv.refs.iter().cloned().collect();
    let mut cmds = Vec::new();
    let mut tips = Vec::new();
    for (local, dst) in specs {
        let (force, dst) = match dst.strip_prefix('+') {
            Some(d) => (true, d),
            None => (false, *dst),
        };
        let new = repo.rev_parse(local)?;
        let old = remote_refs.get(dst).copied().unwrap_or(repo.hash.null());
        if old == new {
            continue;
        }
        if !force && !old.is_null() && !recent_commits(repo, &[new], usize::MAX).contains(&old) {
            return Err(Error::Io(format!("push to {dst} rejected: non-fast-forward (use +{dst} to force)")));
        }
        cmds.push(Command { old, new, name: dst.to_string() });
        tips.push(new);
    }
    if cmds.is_empty() {
        return Ok(protocol::Report { unpack_ok: true, unpack: "ok".into(), refs: Vec::new() });
    }
    // What the remote already has (and we can see): stop there.
    let mut stop_c = BTreeSet::new();
    let mut stop_o = BTreeSet::new();
    for id in remote_refs.values() {
        if repo.has(id) {
            if let Ok((Kind::Commit, _)) = repo.read(id) {
                for c in recent_commits(repo, &[*id], usize::MAX) {
                    stop_c.insert(c);
                }
                all_tree_objects(repo, id, &mut stop_o)?;
            }
        }
    }
    let objs = reachable(repo, &tips, &stop_c, &stop_o)?;
    let po: Vec<PackObject> = objs.into_iter().map(|(_, k, d, p)| PackObject { kind: k, data: d, path: p }).collect();
    let written = pack::write_pack(repo.hash, &po, WriteOptions { window: if adv.caps.iter().any(|c| c == "ofs-delta") { 10 } else { 0 }, ..Default::default() });
    let mut caps: Vec<String> = vec!["report-status".into()];
    let sideband = adv.caps.iter().any(|c| c == "side-band-64k");
    if sideband {
        caps.push("side-band-64k".into());
    }
    if adv.caps.iter().any(|c| c.starts_with("agent=")) {
        caps.push(format!("agent={}", protocol::AGENT));
    }
    if repo.hash != HashKind::Sha1 {
        caps.push(format!("object-format={}", repo.hash.name()));
    }
    let capr: Vec<&str> = caps.iter().map(String::as_str).collect();
    let body = protocol::push_request(&cmds, &capr, &written.pack);
    let (st, resp) = remote.exchange(
        "POST",
        "git-receive-pack",
        &[("Content-Type", "application/x-git-receive-pack-request"), ("Accept", "application/x-git-receive-pack-result")],
        body,
    )?;
    if st != 200 {
        return Err(Error::Io(format!("POST git-receive-pack: HTTP {st}: {}", String::from_utf8_lossy(&resp))));
    }
    protocol::parse_report_status(&resp, sideband)
}

/// Clone over the DUMB protocol (static files: `info/refs`, `objects/info/packs`, packs, loose
/// objects) into a bare repository.
pub fn dumb_clone(url: &str, dir: &Path) -> Result<(Repository, Transfer)> {
    let remote = Remote::new(url);
    let refs = protocol::parse_dumb_info_refs(HashKind::Sha1, &remote.get_ok("info/refs", &[])?)?;
    let head = remote.get_ok("HEAD", &[]).ok().map(|h| String::from_utf8_lossy(&h).trim().to_string()).unwrap_or_default();
    let branch = head.strip_prefix("ref: refs/heads/").unwrap_or("main").to_string();
    let repo = Repository::init(dir, true, HashKind::Sha1, &branch)?;
    let mut t = Transfer::default();
    if let Ok(packs) = remote.get_ok("objects/info/packs", &[]) {
        for p in protocol::parse_info_packs(&packs) {
            let pb = remote.get_ok(&format!("objects/pack/{p}"), &[])?;
            t.pack_bytes += pb.len();
            t.objects += receive_pack(&repo, &pb)?;
        }
    }
    // Walk from the refs, fetching loose objects the packs did not hold.
    let mut stack: Vec<ObjectId> = refs.iter().map(|(_, id)| *id).collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if !repo.has(&id) {
            let h = id.to_hex();
            let f = remote.get_ok(&format!("objects/{}/{}", &h[..2], &h[2..]), &[])?;
            let (k, d) = crate::loose::decode_verified(&id, &f, usize::MAX)?;
            repo.write(k, &d)?;
            t.objects += 1;
        }
        let (k, d) = repo.read(&id)?;
        match k {
            Kind::Commit => {
                let c = Commit::parse(repo.hash, &d)?;
                stack.extend(c.parents());
                stack.push(c.tree().ok_or(Error::Corrupt("commit tree"))?);
            }
            Kind::Tree => stack.extend(Tree::parse(repo.hash, &d)?.entries.iter().filter(|e| !e.is_gitlink()).map(|e| e.id)),
            Kind::Tag => stack.push(Tag::parse(repo.hash, &d)?.target().ok_or(Error::Corrupt("tag target"))?),
            Kind::Blob => {}
        }
    }
    let mut packed = PackedRefs { traits: Some(PackedRefs::standard_traits()), refs: Vec::new() };
    for (n, id) in &refs {
        if !(n.starts_with("refs/heads/") || n.starts_with("refs/tags/")) {
            continue;
        }
        let (k, d) = repo.read(id)?;
        let peeled = if k == Kind::Tag {
            let mut x = Tag::parse(repo.hash, &d)?.target();
            while let Some(i) = x {
                match repo.read(&i)? {
                    (Kind::Tag, dd) => x = Tag::parse(repo.hash, &dd)?.target(),
                    _ => break,
                }
            }
            x
        } else {
            None
        };
        packed.upsert(PackedRef { name: n.clone().into_bytes(), id: *id, peeled });
        t.refs.push((n.clone(), *id));
    }
    fs::write(repo.common_dir.join("packed-refs"), packed.serialize()).map_err(|e| Error::Io(format!("packed-refs: {e}")))?;
    t.requests = remote.requests.get();
    Ok((Repository::open(&repo.git_dir)?, t))
}
