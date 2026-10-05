// SPDX-License-Identifier: LGPL-3.0-or-later
// M4: mutation fuzzing — 20 000+ mutants across packs, pack indexes, the index file and config
// files (plus deltas, loose objects, objects and protocol responses). The claim is NO PANIC: every
// mutant is either parsed or refused with a named error. Half the pack/idx/index mutants get their
// trailing checksum recomputed so the mutation reaches the parsers behind the checksum.
mod common;

use std::panic::{catch_unwind, AssertUnwindSafe};

use git_core::config::{self, Config, NoIncludes};
use git_core::index::Index;
use git_core::object::{Kind, Object};
use git_core::pack::{self, Idx, Pack, PackObject, WriteOptions};
use git_core::{delta, loose, protocol, HashKind};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(r: &mut Rng, src: &[u8], fix_tail: Option<usize>) -> Vec<u8> {
    let mut v = src.to_vec();
    for _ in 0..1 + r.below(4) {
        if v.is_empty() {
            v.push(r.next() as u8);
            continue;
        }
        let at = r.below(v.len());
        match r.below(6) {
            0 | 1 => v[at] ^= 1 << r.below(8),
            2 => v[at] = r.next() as u8,
            3 => {
                v.insert(at, r.next() as u8);
            }
            4 => {
                v.remove(at);
            }
            _ => v.truncate(at),
        }
    }
    if let Some(n) = fix_tail {
        if v.len() > n && r.below(2) == 0 {
            let end = v.len() - n;
            let d = HashKind::Sha1.digest(&v[..end]);
            v[end..].copy_from_slice(d.as_bytes());
        }
    }
    v
}

fn run(name: &str, n: usize, seed: u64, src: &[u8], fix_tail: Option<usize>, f: &dyn Fn(&[u8])) -> (usize, usize) {
    let scale: usize = std::env::var("GITCORE_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let n = n * scale;
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut panics = 0;
    for _ in 0..n {
        let m = mutate(&mut r, src, fix_tail);
        if catch_unwind(AssertUnwindSafe(|| f(&m))).is_err() {
            panics += 1;
            if panics == 1 {
                std::fs::write(std::env::temp_dir().join(format!("gitcore-fuzz-{name}.bin")), &m).ok();
            }
        }
    }
    println!("{name:<10} {n} mutants, {panics} panics");
    (n, panics)
}

#[test]
fn twenty_thousand_mutants_no_panic() {
    std::panic::set_hook(Box::new(|_| {}));
    // Seeds: a small pack with deltas, its idx, a v2 and a v4 index from git, a config file.
    let objs: Vec<PackObject> = (0..12)
        .map(|i| PackObject { kind: Kind::Blob, data: format!("{}\n", "shared line of text ".repeat(20 + i)).into_bytes(), path: None })
        .chain(std::iter::once(PackObject { kind: Kind::Tree, data: Vec::new(), path: None }))
        .collect();
    let w = pack::write_pack(HashKind::Sha1, &objs, WriteOptions::default());
    let idx = w.idx(HashKind::Sha1);
    let dir = common::scratch("fuzz");
    common::git(&dir, &["init", "-q"]);
    for i in 0..6 {
        let p = dir.join(format!("d{}/f{i}.txt", i % 2));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{i}\n")).unwrap();
    }
    common::git(&dir, &["add", "-A"]);
    common::git(&dir, &["commit", "-qm", "x"]);
    let index_v2 = std::fs::read(dir.join(".git/index")).unwrap();
    common::git(&dir, &["update-index", "--index-version", "4"]);
    let index_v4 = std::fs::read(dir.join(".git/index")).unwrap();
    let cfg = b"[core]\n\tbare = false\n\tx = \"a\\tb\" ; c\n[remote \"o\\\"x\"]\n\turl = u\\\n v\n[a.B]k\n[include]\npath = p\n".to_vec();
    let d = delta::encode(&objs[0].data, &objs[5].data);
    let (_, loose_bytes) = loose::encode(HashKind::Sha1, Kind::Blob, b"hello world\n", 6);
    let commit = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nparent 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A <a> 1 +0000\ncommitter C <c> 2 -0130\ngpgsig x\n y\n\nmsg\n".to_vec();
    let mut fetch_resp = Vec::new();
    protocol::pkt_line(&mut fetch_resp, "shallow-info");
    protocol::pkt_line(&mut fetch_resp, "shallow 4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    protocol::delim(&mut fetch_resp);
    protocol::pkt_line(&mut fetch_resp, "packfile");
    let mut band = vec![1u8];
    band.extend_from_slice(&w.pack[..200]);
    protocol::pkt(&mut fetch_resp, &band);
    protocol::flush(&mut fetch_resp);

    let mut total = 0;
    let mut panics = 0;
    let mut tally = |(n, p): (usize, usize)| {
        total += n;
        panics += p;
    };
    tally(run("pack", 5000, 1, &w.pack, Some(20), &|m| {
        let _ = pack::index_pack(m, HashKind::Sha1, &mut |_| None, &mut |_, _, _| Ok(()));
        if let Ok(p) = Pack::parse(m, HashKind::Sha1, false) {
            let ix = Idx::parse(&idx, HashKind::Sha1, false).unwrap();
            for i in 0..ix.count as usize {
                let _ = p.object_at(ix.offset(i), &|id| ix.lookup(id), &mut |_| None);
            }
        }
    }));
    tally(run("idx", 3000, 2, &idx, Some(20), &|m| {
        if let Ok(ix) = Idx::parse(m, HashKind::Sha1, true) {
            for i in 0..ix.count as usize {
                let id = ix.oid(i);
                let _ = (ix.lookup(&id), ix.crc(i), ix.offset(i));
            }
            let mut v = Vec::new();
            ix.prefix_matches(b"ab", &mut v);
        }
        if let Ok(ix) = Idx::parse(m, HashKind::Sha1, false) {
            for i in 0..(ix.count as usize).min(64) {
                let _ = ix.offset(i);
            }
        }
    }));
    for (name, seed, src) in [("index-v2", 3u64, &index_v2), ("index-v4", 4, &index_v4)] {
        tally(run(name, 2500, seed, src, Some(20), &|m| {
            if let Ok(i) = Index::parse(HashKind::Sha1, m) {
                let _ = i.serialize();
                let _ = i.cache_tree();
            }
        }));
    }
    tally(run("config", 4000, 5, &cfg, None, &|m| {
        let mut c = Config::new();
        let _ = c.load(b"x", m, &mut NoIncludes);
        let _ = c.list();
        let _ = config::set(m, b"core.new", b"v ; x");
        let _ = config::unset_all(m, b"core.bare");
    }));
    tally(run("delta", 1500, 6, &d, None, &|m| {
        let _ = delta::apply(&objs[0].data, m);
    }));
    tally(run("loose", 1000, 7, &loose_bytes, None, &|m| {
        let _ = loose::decode(m, 1 << 20);
    }));
    tally(run("commit", 1000, 8, &commit, None, &|m| {
        if let Ok(o) = Object::parse(HashKind::Sha1, Kind::Commit, m) {
            let _ = o.serialize();
        }
        let _ = Object::parse(HashKind::Sha1, Kind::Tree, m);
        let _ = Object::parse(HashKind::Sha1, Kind::Tag, m);
    }));
    tally(run("protocol", 1000, 9, &fetch_resp, None, &|m| {
        let _ = protocol::parse_fetch_response(HashKind::Sha1, m);
        let _ = protocol::parse_ls_refs(HashKind::Sha1, m);
        let _ = protocol::parse_v0_advert(HashKind::Sha1, m);
        let _ = protocol::parse_report_status(m, true);
        let _ = protocol::Capabilities::parse(m);
    }));
    let _ = std::panic::take_hook();
    println!("TOTAL {total} mutants, {panics} panics");
    assert!(total >= 20_000);
    assert_eq!(panics, 0, "panicking inputs saved under the temp dir as gitcore-fuzz-*.bin");
}
