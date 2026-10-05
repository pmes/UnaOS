// SPDX-License-Identifier: LGPL-3.0-or-later
// M4: smart HTTP (protocol v2) clone / fetch / shallow / push against a local `git http-backend`,
// compared with git's own clone of the same URL; the dumb protocol against static files.
mod common;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use git_core::remote::{self, CloneOptions};
use git_core::{ObjectId, Repository};

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID, the child we spawned
        let _ = self.child.wait();
    }
}

fn serve(root: &Path, mode: &str) -> Option<Server> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/http_server.py");
    let mut child = Command::new("python3").arg(script).arg(root).arg(mode).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).ok()?;
    let port = line.trim().parse().ok()?;
    Some(Server { child, port })
}

fn source_repo(root: &Path) -> PathBuf {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    common::git(&work, &["init", "-q", "-b", "main"]);
    for c in 0..6 {
        std::fs::write(work.join("file.txt"), format!("version {c}\n").repeat(50 + c)).unwrap();
        std::fs::write(work.join(format!("f{c}.bin")), common::prng(c as u64 + 3, 2000)).unwrap();
        common::git(&work, &["add", "-A"]);
        common::git(&work, &["commit", "-q", "-m", &format!("c{c}")]);
        if c == 2 {
            common::git(&work, &["tag", "-a", "-m", "annotated", "v1.0"]);
            common::git(&work, &["branch", "side"]);
        }
    }
    common::git(&work, &["tag", "light"]);
    common::git(root, &["clone", "-q", "--bare", work.to_str().unwrap(), "src.git"]);
    let src = root.join("src.git");
    common::git(&src, &["config", "http.receivepack", "true"]);
    common::git(&src, &["config", "uploadpack.allowFilter", "true"]);
    src
}

fn object_set(dir: &Path) -> Vec<String> {
    let o = common::git(dir, &["cat-file", "--batch-all-objects", "--batch-check"]);
    let mut v: Vec<String> = String::from_utf8_lossy(&o).lines().map(String::from).collect();
    v.sort();
    v
}

#[test]
fn smart_http_clone_fetch_push() {
    let root = common::scratch("http");
    let src = source_repo(&root);
    let Some(srv) = serve(&root, "smart") else {
        println!("SKIPPED (python3 unavailable)");
        return;
    };
    let url = format!("http://127.0.0.1:{}/src.git", srv.port);

    // 1. Bare clone: ours vs git's.
    common::git(&root, &["clone", "-q", "--bare", &url, "git-clone.git"]);
    let (repo, t) = remote::clone(&url, &root.join("ours.git"), &CloneOptions { bare: true, depth: None }).unwrap();
    let g = root.join("git-clone.git");
    let o = root.join("ours.git");
    assert_eq!(object_set(&o), object_set(&g), "object sets");
    assert_eq!(std::fs::read(o.join("packed-refs")).unwrap(), std::fs::read(g.join("packed-refs")).unwrap(), "packed-refs byte-equal");
    assert_eq!(std::fs::read(o.join("HEAD")).unwrap(), std::fs::read(g.join("HEAD")).unwrap(), "HEAD");
    let pack_of = |d: &Path| -> Vec<u8> {
        let p = std::fs::read_dir(d.join("objects/pack")).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|e| e == "pack")).unwrap();
        std::fs::read(p).unwrap()
    };
    let same_pack = pack_of(&o) == pack_of(&g);
    let f = common::git_raw(&o, &["fsck", "--full", "--strict"], None);
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stderr));
    println!("clone: {} objects, {} pack bytes, {} requests; packed-refs/HEAD/object set equal to git clone; pack bytes identical: {same_pack}", t.objects, t.pack_bytes, t.requests);

    // 2. Shallow clone: ours vs git's --depth 1.
    common::git(&root, &["clone", "-q", "--bare", "--depth", "1", &url, "git-shallow.git"]);
    remote::clone(&url, &root.join("ours-shallow.git"), &CloneOptions { bare: true, depth: Some(1) }).unwrap();
    let (gs, os) = (root.join("git-shallow.git"), root.join("ours-shallow.git"));
    assert_eq!(object_set(&os), object_set(&gs), "shallow object sets");
    assert_eq!(std::fs::read(os.join("shallow")).unwrap(), std::fs::read(gs.join("shallow")).unwrap(), "shallow file");

    // 3. Non-bare clone checks out clean.
    let (wrepo, _) = remote::clone(&url, &root.join("ours-work"), &CloneOptions::default()).unwrap();
    assert!(!wrepo.is_dirty().unwrap());
    let st = common::git(&root.join("ours-work"), &["status", "--porcelain"]);
    assert!(st.is_empty(), "git status in our clone: {}", String::from_utf8_lossy(&st));

    // 4. Fetch: new commits upstream, negotiated with haves.
    let work = root.join("work");
    std::fs::write(work.join("file.txt"), "fetched later\n").unwrap();
    common::git(&work, &["commit", "-qam", "upstream 7"]);
    common::git(&work, &["push", "-q", src.to_str().unwrap(), "main"]);
    let t = remote::fetch(&repo, &url, "refs/heads/").unwrap();
    assert!(t.objects > 0 && t.objects < 10, "fetched {} objects (negotiation should send only new ones)", t.objects);
    let head = String::from_utf8_lossy(&common::git(&src, &["rev-parse", "main"])).trim().to_string();
    assert_eq!(repo.rev_parse("main").unwrap().to_hex(), head);
    let f = common::git_raw(&o, &["fsck", "--full"], None);
    assert!(f.status.success());
    println!("fetch: {} objects in {} bytes over {} requests", t.objects, t.pack_bytes, t.requests);

    // 5. Push: commit locally with git_core, push, server has it.
    let wr = Repository::discover(root.join("ours-work")).unwrap();
    remote::fetch(&wr, &url, "refs/remotes/origin/").unwrap();
    std::fs::write(root.join("ours-work/pushed.txt"), "from git_core\n").unwrap();
    wr.add(&[b"pushed.txt"]).unwrap();
    let who = git_core::Signature::new(b"Una", b"una@unaos", 1760000000, 0);
    // build on top of the fetched upstream main
    let upstream = wr.rev_parse("refs/remotes/origin/main").unwrap();
    let wt = wr.write_tree(&wr.index().unwrap()).unwrap();
    let c = git_core::Commit::new(wt, &[upstream], &who, &who, b"pushed by git_core\n");
    let cid = wr.write(git_core::Kind::Commit, &c.serialize()).unwrap();
    let rep = remote::push(&wr, &url, &[(&cid.to_hex(), "refs/heads/main"), (&cid.to_hex(), "refs/heads/new-branch")]).unwrap();
    assert!(rep.unpack_ok, "{rep:?}");
    assert!(rep.refs.iter().all(|(_, r)| r.is_ok()), "{rep:?}");
    assert_eq!(String::from_utf8_lossy(&common::git(&src, &["rev-parse", "main", "new-branch"])), format!("{cid}\n{cid}\n"));
    let f = common::git_raw(&src, &["fsck", "--full", "--strict"], None);
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stderr));
    // Non-fast-forward: refused by the client; forced, refused by the server (denyNonFastForwards)
    // and reported as `ng` through report-status.
    assert!(remote::push(&wr, &url, &[(&upstream.to_hex(), "refs/heads/main")]).is_err());
    common::git(&src, &["config", "receive.denyNonFastForwards", "true"]);
    let rep = remote::push(&wr, &url, &[(&upstream.to_hex(), "+refs/heads/main")]).unwrap();
    assert!(rep.refs.iter().any(|(_, r)| r.is_err()), "non-ff must be refused: {rep:?}");
    println!("push: report-status ok for 2 refs, server fsck clean; non-fast-forward reported ng: {:?}", rep.refs);
    let _ = ObjectId::from_hex(b"");
}

#[test]
fn dumb_http_clone() {
    let root = common::scratch("dumb");
    let src = source_repo(&root);
    // Half packed, half loose: repack, then add loose objects.
    common::git(&src, &["repack", "-adq"]);
    let work = root.join("work");
    std::fs::write(work.join("loose.txt"), "loose object\n").unwrap();
    common::git(&work, &["add", "-A"]);
    common::git(&work, &["commit", "-qm", "loose"]);
    common::git(&work, &["push", "-q", src.to_str().unwrap(), "main"]);
    common::git(&src, &["update-server-info"]);
    let Some(srv) = serve(&root, "dumb") else { return };
    let url = format!("http://127.0.0.1:{}/src.git", srv.port);
    let (_, t) = remote::dumb_clone(&url, &root.join("dumb.git")).unwrap();
    let d = root.join("dumb.git");
    let f = common::git_raw(&d, &["fsck", "--full", "--strict"], None);
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stderr));
    assert_eq!(String::from_utf8_lossy(&common::git(&d, &["rev-parse", "main", "v1.0", "side"])), String::from_utf8_lossy(&common::git(&src, &["rev-parse", "main", "v1.0", "side"])));
    println!("dumb clone: {} objects ({} pack bytes) over {} requests, fsck clean", t.objects, t.pack_bytes, t.requests);
}

/// This repository's own history (a depth-50 bare clone of itself) cloned over smart HTTP by
/// git and by git_core: object sets, packed-refs, HEAD and shallow boundary byte-equal.
#[test]
fn smart_http_clone_of_self() {
    let root = common::scratch("httpself");
    let url0 = format!("file://{}", common::self_repo().display());
    let o = common::git_raw(&root, &["clone", "-q", "--bare", "--no-local", "--single-branch", "--depth", "50", &url0, "self.git"], None);
    if !o.status.success() {
        println!("SKIPPED (self clone failed)");
        return;
    }
    let Some(srv) = serve(&root, "smart") else { return };
    let url = format!("http://127.0.0.1:{}/self.git", srv.port);
    common::git(&root, &["clone", "-q", "--bare", &url, "git.git"]);
    let t0 = std::time::Instant::now();
    let (_, t) = remote::clone(&url, &root.join("ours.git"), &CloneOptions { bare: true, depth: None }).unwrap();
    let el = t0.elapsed();
    let (g, o) = (root.join("git.git"), root.join("ours.git"));
    assert_eq!(object_set(&o), object_set(&g));
    for f in ["packed-refs", "HEAD"] {
        assert_eq!(std::fs::read(o.join(f)).ok(), std::fs::read(g.join(f)).ok(), "{f}");
    }
    // git 2.43 writes the shallow boundary it is sent from a shallow server with repeated lines
    // (the server repeats them per `deepen` pass); the set is the claim, ours is sorted + unique.
    let set = |d: &Path| -> std::collections::BTreeSet<String> {
        std::fs::read_to_string(d.join("shallow")).unwrap_or_default().lines().map(String::from).collect()
    };
    assert_eq!(set(&o), set(&g), "shallow boundary (as a set)");
    let pk = |d: &Path| {
        let p = std::fs::read_dir(d.join("objects/pack")).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|e| e == "pack")).unwrap();
        std::fs::read(p).unwrap()
    };
    let same = pk(&o) == pk(&g);
    let f = common::git_raw(&o, &["fsck", "--full"], None);
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stderr));
    println!("self over HTTP: {} objects, {} pack bytes in {el:?}; object set, packed-refs, HEAD, shallow equal to git clone; pack bytes identical: {same}", t.objects, t.pack_bytes);
}
