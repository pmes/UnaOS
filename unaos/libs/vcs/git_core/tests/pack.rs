// SPDX-License-Identifier: LGPL-3.0-or-later
// M2: packfiles + idx v2 + multi-pack-index, against `git verify-pack`, `git index-pack`,
// `git multi-pack-index`, `git fsck` — and this repository's own shallow history.
mod common;

use std::path::Path;
use std::time::Instant;

use git_core::object::{Kind, Object};
use git_core::pack::{self, Idx, Midx, Pack, PackObject, WriteOptions};
use git_core::{HashKind, ObjectId};

/// Every object of a repository, read through `git cat-file --batch-all-objects --batch`.
fn all_objects(dir: &Path) -> Vec<(ObjectId, Kind, Vec<u8>)> {
    let out = common::git(dir, &["cat-file", "--batch-all-objects", "--batch"]);
    let mut v = Vec::new();
    let mut i = 0;
    while i < out.len() {
        let nl = out[i..].iter().position(|&b| b == b'\n').unwrap() + i;
        let hdr = String::from_utf8_lossy(&out[i..nl]).into_owned();
        let mut it = hdr.split(' ');
        let id = ObjectId::from_hex(it.next().unwrap().as_bytes()).unwrap();
        let kind = Kind::from_name(it.next().unwrap().as_bytes()).unwrap();
        let size: usize = it.next().unwrap().parse().unwrap();
        v.push((id, kind, out[nl + 1..nl + 1 + size].to_vec()));
        i = nl + 1 + size + 1;
    }
    v
}

fn history_repo(hk: HashKind) -> std::path::PathBuf {
    let dir = common::scratch(&format!("packsrc-{}", hk.name()));
    common::git(&dir, &["init", "-q", "-b", "main", &format!("--object-format={}", hk.name())]);
    let mut text: Vec<String> = (0..400).map(|i| format!("line {i} of a file that evolves {}\n", i * 7 % 13)).collect();
    for c in 0..12 {
        text[c * 17 % 400] = format!("changed in commit {c}\n");
        text.insert(c * 31 % 400, format!("inserted {c}\n"));
        std::fs::write(dir.join("evolving.txt"), text.concat()).unwrap();
        std::fs::write(dir.join(format!("new{c}.bin")), common::prng(c as u64 + 1, 3000 + c * 100)).unwrap();
        std::fs::create_dir_all(dir.join("d/e")).unwrap();
        std::fs::write(dir.join("d/e/copy.txt"), text[..200 + c].concat()).unwrap();
        common::git(&dir, &["add", "-A"]);
        common::git(&dir, &["commit", "-q", "-m", &format!("commit {c}")]);
        if c % 4 == 0 {
            common::git(&dir, &["tag", "-a", "-m", "t", &format!("v{c}")]);
        }
    }
    dir
}

fn verify_with_git(dir: &Path, name: &str, packbytes: &[u8], ouridx: &[u8]) -> String {
    let pdir = dir.join("check");
    std::fs::create_dir_all(&pdir).unwrap();
    let p = pdir.join(format!("{name}.pack"));
    let i = pdir.join(format!("{name}.idx"));
    std::fs::write(&p, packbytes).unwrap();
    std::fs::write(&i, ouridx).unwrap();
    let vp = common::git(dir, &["verify-pack", "-v", i.to_str().unwrap()]);
    let gi = pdir.join(format!("{name}-git.idx"));
    common::git(dir, &["index-pack", "-o", gi.to_str().unwrap(), p.to_str().unwrap()]);
    let gidx = std::fs::read(&gi).unwrap();
    assert!(gidx == ouridx, "{name}: git index-pack idx differs from ours");
    String::from_utf8_lossy(&vp).lines().filter(|l| l.starts_with("chain length") || l.starts_with("non delta")).collect::<Vec<_>>().join("; ")
}

fn roundtrip(hk: HashKind) {
    let dir = history_repo(hk);
    let objs = all_objects(&dir);
    // 1. Our writer: verify-pack accepts, git's idx == ours.
    let po: Vec<PackObject> = objs.iter().map(|(_, k, d)| PackObject { kind: *k, data: d.clone(), path: None }).collect();
    let w = pack::write_pack(hk, &po, WriteOptions::default());
    assert!(w.deltas > 10, "deltas {}", w.deltas);
    let idx = w.idx(hk);
    let stats = verify_with_git(&dir, "ours", &w.pack, &idx);
    // 2. Our index-pack on our pack reproduces the same idx.
    let r = pack::index_pack(&w.pack, hk, &mut |_| None, &mut |_, _, _| Ok(())).unwrap();
    assert_eq!(r.idx(hk), idx);
    // 3. Our index-pack on git's pack (aggressive repack: deep chains) = git's idx.
    common::git(&dir, &["repack", "-adf", "--depth=50", "--window=50", "-q"]);
    let packdir = dir.join(".git/objects/pack");
    let gp = std::fs::read_dir(&packdir).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|e| e == "pack")).unwrap();
    let gpack = std::fs::read(&gp).unwrap();
    let gidx = std::fs::read(gp.with_extension("idx")).unwrap();
    let mut n = 0;
    let r = pack::index_pack(&gpack, hk, &mut |_| None, &mut |id, k, d| {
        // byte-identical parse/serialize for every object
        let o = Object::parse(hk, k, d)?;
        assert_eq!(o.serialize(), d, "{id}");
        n += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(r.idx(hk), gidx, "idx of git's pack");
    // 4. Random access through the idx.
    let ix = Idx::parse(&gidx, hk, true).unwrap();
    let pk = Pack::parse(&gpack, hk, true).unwrap();
    for (id, k, d) in &objs {
        let off = ix.lookup(id).unwrap();
        let (k2, d2) = pk.object_at(off, &|i| ix.lookup(i), &mut |_| None).unwrap();
        assert_eq!((&k2, &d2), (k, d));
    }
    println!("{}: {} objects; our pack {} bytes ({} deltas) [{stats}]; git pack {} bytes, depth max {}", hk.name(), objs.len(), w.pack.len(), w.deltas, gpack.len(), r.objects.iter().map(|o| o.depth).max().unwrap());
    // 5. A repository whose only pack is ours passes fsck.
    let fresh = common::scratch(&format!("packfsck-{}", hk.name()));
    common::git(&fresh, &["init", "-q", "--bare", &format!("--object-format={}", hk.name())]);
    let name = format!("pack-{}", ObjectId::from_bytes(hk, &w.pack[w.pack.len() - hk.len()..]));
    std::fs::write(fresh.join("objects/pack").join(format!("{name}.pack")), &w.pack).unwrap();
    std::fs::write(fresh.join("objects/pack").join(format!("{name}.idx")), &idx).unwrap();
    let head = common::git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(fresh.join("refs/heads/main"), head).unwrap();
    std::fs::write(fresh.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let f = common::git_raw(&fresh, &["fsck", "--full", "--strict"], None);
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stderr));
}

#[test]
fn pack_roundtrip_sha1() {
    roundtrip(HashKind::Sha1);
}

#[test]
fn pack_roundtrip_sha256() {
    roundtrip(HashKind::Sha256);
}

#[test]
fn multi_pack_index() {
    let dir = common::scratch("midx");
    common::git(&dir, &["init", "-q", "-b", "main"]);
    for c in 0..4 {
        std::fs::write(dir.join(format!("f{c}")), format!("{c}\n").repeat(100)).unwrap();
        common::git(&dir, &["add", "-A"]);
        common::git(&dir, &["commit", "-q", "-m", &format!("c{c}")]);
        common::git(&dir, &["repack", "-q"]); // one new pack per commit
    }
    common::git(&dir, &["multi-pack-index", "write"]);
    let pd = dir.join(".git/objects/pack");
    let m = std::fs::read(pd.join("multi-pack-index")).unwrap();
    let midx = Midx::parse(&m, true).unwrap();
    assert_eq!(midx.packs.len(), 4);
    let mut checked = 0;
    for (pi, name) in midx.packs.iter().enumerate() {
        let idxb = std::fs::read(pd.join(String::from_utf8_lossy(name).into_owned())).unwrap();
        let ix = Idx::parse(&idxb, HashKind::Sha1, true).unwrap();
        for i in 0..ix.count as usize {
            let id = ix.oid(i);
            let (p, off) = midx.lookup(&id).unwrap();
            // git picks one pack per object; when it is this one, the offset must agree.
            if p as usize == pi {
                assert_eq!(off, ix.offset(i));
                checked += 1;
            }
        }
    }
    assert_eq!(checked, midx.count as usize);
    println!("midx: {} objects across {} packs resolved", midx.count, midx.packs.len());
}

/// This repository's own history, from a shallow clone of itself: git's pack indexed here
/// (idx byte-identical), every object round-tripped byte-identical, re-packed by git_core, and the
/// re-packed repository passes `git fsck --full --strict` and `git verify-pack`.
#[test]
fn self_history_shallow() {
    let depth = std::env::var("GITCORE_SELF_DEPTH").unwrap_or_else(|_| "8".into());
    let src = common::self_repo();
    let dir = common::scratch("selfclone");
    let url = format!("file://{}", src.display());
    let out = common::git_raw(&dir, &["clone", "-q", "--bare", "--no-local", "--single-branch", "--depth", &depth, &url, "c.git"], None);
    if !out.status.success() {
        println!("SKIPPED (shallow clone of self failed: {})", String::from_utf8_lossy(&out.stderr));
        return;
    }
    let repo = dir.join("c.git");
    let pd = repo.join("objects/pack");
    let gp = std::fs::read_dir(&pd).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|e| e == "pack")).unwrap();
    let gpack = std::fs::read(&gp).unwrap();
    let gidx = std::fs::read(gp.with_extension("idx")).unwrap();
    let t0 = Instant::now();
    let mut objs: Vec<PackObject> = Vec::new();
    let mut counts = [0usize; 5];
    let r = pack::index_pack(&gpack, HashKind::Sha1, &mut |_| None, &mut |id, k, d| {
        let o = Object::parse(HashKind::Sha1, k, d)?;
        assert_eq!(o.serialize(), d, "round trip {id}");
        counts[k as usize] += 1;
        objs.push(PackObject { kind: k, data: d.to_vec(), path: None });
        Ok(())
    })
    .unwrap();
    let t_index = t0.elapsed();
    assert_eq!(r.idx(HashKind::Sha1), gidx, "self pack: idx byte-identical to git's");
    let t1 = Instant::now();
    let w = pack::write_pack(HashKind::Sha1, &objs, WriteOptions::default());
    let t_write = t1.elapsed();
    let idx = w.idx(HashKind::Sha1);
    // Replace git's pack with ours.
    std::fs::remove_file(&gp).unwrap();
    std::fs::remove_file(gp.with_extension("idx")).unwrap();
    let name = format!("pack-{}", ObjectId::from_bytes(HashKind::Sha1, &w.pack[w.pack.len() - 20..]));
    std::fs::write(pd.join(format!("{name}.pack")), &w.pack).unwrap();
    std::fs::write(pd.join(format!("{name}.idx")), &idx).unwrap();
    let vp = common::git_raw(&repo, &["verify-pack", pd.join(format!("{name}.idx")).to_str().unwrap()], None);
    assert!(vp.status.success(), "verify-pack: {}", String::from_utf8_lossy(&vp.stderr));
    let f = common::git_raw(&repo, &["fsck", "--full", "--strict"], None);
    assert!(f.status.success(), "fsck: {}", String::from_utf8_lossy(&f.stderr));
    println!(
        "self history depth {depth}: {} objects (commits {}, trees {}, blobs {}, tags {}), git pack {} B -> ours {} B ({} deltas); index {:?}, write {:?}; idx identical, verify-pack + fsck clean",
        r.objects.len(), counts[1], counts[2], counts[3], counts[4], gpack.len(), w.pack.len(), w.deltas, t_index, t_write
    );
}
