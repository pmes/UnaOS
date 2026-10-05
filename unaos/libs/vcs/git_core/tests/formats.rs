// SPDX-License-Identifier: LGPL-3.0-or-later
// M1: refs / packed-refs / reflog / config / gitignore / gitattributes against the git CLI.
mod common;

use std::path::{Path, PathBuf};

use git_core::config::{self, Config, Includer};
use git_core::ignore::{AttrFile, AttrValue, Attributes, Ignore, PatternList};
use git_core::refs::{self, PackedRefs, ReflogEntry};
use git_core::HashKind;

#[test]
fn packed_refs_and_reflog_roundtrip() {
    let dir = common::scratch("refs");
    common::git(&dir, &["init", "-q", "-b", "main"]);
    for i in 0..5 {
        std::fs::write(dir.join("f"), format!("{i}\n")).unwrap();
        common::git(&dir, &["add", "f"]);
        common::git(&dir, &["commit", "-q", "-m", &format!("c{i}\n\nbody line")]);
        common::git(&dir, &["branch", &format!("b{i}")]);
        common::git(&dir, &["tag", "-a", "-m", "annotated", &format!("v{i}")]);
        common::git(&dir, &["tag", &format!("light{i}")]);
    }
    common::git(&dir, &["pack-refs", "--all"]);
    let raw = std::fs::read(dir.join(".git/packed-refs")).unwrap();
    let p = PackedRefs::parse(HashKind::Sha1, &raw).unwrap();
    assert_eq!(p.serialize(), raw, "packed-refs byte-identical round trip");
    assert_eq!(p.refs.len(), 16);
    assert_eq!(p.refs.iter().filter(|r| r.peeled.is_some()).count(), 5);
    let v3 = p.find(b"refs/tags/v3").unwrap();
    let want = common::git(&dir, &["rev-parse", "v3^{commit}"]);
    assert_eq!(v3.peeled.unwrap().to_hex(), String::from_utf8_lossy(&want).trim());
    // reflog
    let log = std::fs::read(dir.join(".git/logs/HEAD")).unwrap();
    let entries = ReflogEntry::parse_all(HashKind::Sha1, &log).unwrap();
    assert_eq!(entries.len(), 5);
    let back: Vec<u8> = entries.iter().flat_map(|e| e.serialize()).collect();
    assert_eq!(back, log, "reflog byte-identical round trip");
    // HEAD is symbolic
    let head = std::fs::read(dir.join(".git/HEAD")).unwrap();
    assert_eq!(refs::RefValue::parse(HashKind::Sha1, &head).unwrap(), refs::RefValue::Symbolic(b"refs/heads/main".to_vec()));
    // check-ref-format agreement
    let names = [
        "refs/heads/main", "refs/heads/a..b", "refs/heads/.hidden", "refs/heads/x.lock", "refs/heads/sp ace", "refs//double",
        "refs/heads/at@{x", "refs/heads/ok@x", "refs/heads/end.", "refs/heads/tilde~", "refs/heads/caret^", "refs/heads/colon:",
        "refs/heads/q?", "refs/heads/star*", "refs/heads/br[", "refs/heads/back\\slash", "refs/heads/trail/", "/refs/lead", "onelevel",
        "refs/heads/ünï", "refs/heads/-dash", "refs/tags/v1.0.0", "refs/heads/a/.b", "refs/heads/a.lock/b", "@",
    ];
    for n in names {
        let ok = common::git_raw(&dir, &["check-ref-format", n], None).status.success();
        assert_eq!(refs::is_valid_name(n.as_bytes(), false), ok, "check-ref-format {n}");
    }
}

struct FsIncluder;
impl Includer for FsIncluder {
    fn load(&mut self, from: &[u8], path: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let from = PathBuf::from(String::from_utf8_lossy(from).into_owned());
        let p = from.parent().unwrap().join(String::from_utf8_lossy(path).into_owned());
        let d = std::fs::read(&p).ok()?;
        Some((p.to_string_lossy().into_owned().into_bytes(), d))
    }
    fn condition(&mut self, _: &[u8], _: &[u8]) -> bool {
        false
    }
}

const CONFIG_FIXTURE: &str = "\u{feff}# leading comment\n\
[core]\n\
\trepositoryformatversion = 0\n\
\tfilemode = true\r\n\
\tbare\n\
\tEmpty =\n\
\tspaced   =   a   b\t\tc   \n\
\tquoted = \"  lead\" mid \" trail  \" # comment\n\
\tescapes = tab\\there\\nnl \\\"q\\\" back\\\\slash\n\
\tcont = one \\\n  two\\\n three\n\
\tsemi = \"a;b#c\" ; real comment\n\
[remote \"origin\"]\n\
\turl = https://example.com/x.git\n\
\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
[Remote \"Origin\"] url = second\n\
[branch.Main]\n\
\tremote = origin\n\
[sec \"sub \\\"q\\\" \\\\ x\"]\n\
\tKey-Name = v\n\
[include]\n\
\tpath = inc/one.cfg\n\
[core]\n\
\tafter = include\n";

#[test]
fn config_matches_git_list() {
    let dir = common::scratch("config");
    let f = dir.join("main.cfg");
    std::fs::write(&f, CONFIG_FIXTURE).unwrap();
    std::fs::create_dir_all(dir.join("inc")).unwrap();
    std::fs::write(dir.join("inc/one.cfg"), "[inc]\n\tlevel = 1\n[include]\n\tpath = two.cfg\n").unwrap();
    std::fs::write(dir.join("inc/two.cfg"), "[inc]\n\tlevel = 2\n\tsize = 3k\n").unwrap();
    let want = common::git(&dir, &["config", "--file", f.to_str().unwrap(), "--includes", "--list"]);
    let mut c = Config::new();
    c.load(f.to_str().unwrap().as_bytes(), CONFIG_FIXTURE.as_bytes(), &mut FsIncluder).unwrap();
    assert_eq!(String::from_utf8_lossy(&c.list()), String::from_utf8_lossy(&want));
    // typed values against git --type
    for (k, ty) in [("core.bare", "bool"), ("core.filemode", "bool"), ("core.empty", "bool"), ("inc.size", "int")] {
        let g = common::git(&dir, &["config", "--file", f.to_str().unwrap(), "--includes", &format!("--type={ty}"), "--get", k]);
        let g = String::from_utf8_lossy(&g).trim().to_string();
        let ours = match ty {
            "bool" => c.get_bool(k.as_bytes()).unwrap().unwrap().to_string(),
            _ => c.get_int(k.as_bytes()).unwrap().unwrap().to_string(),
        };
        assert_eq!(ours, g, "{k}");
    }
    // Last-one-wins agrees with git --get on a multi-valued key.
    let g = common::git(&dir, &["config", "--file", f.to_str().unwrap(), "--get", "remote.origin.url"]);
    assert_eq!(c.get(b"remote.origin.url").unwrap(), String::from_utf8_lossy(&g).trim().as_bytes());
    // Edits: git reads back what we wrote; untouched bytes stay untouched.
    let mut text = CONFIG_FIXTURE.as_bytes().to_vec();
    text = config::set(&text, b"core.filemode", b"false").unwrap();
    text = config::set(&text, b"remote.origin.pushurl", b" spaced; value\t").unwrap();
    text = config::set(&text, b"brand.new \"sub\".key", b"x").unwrap_or(text);
    text = config::set(&text, b"newsec.sub.key", b"line1\nline2").unwrap();
    text = config::unset_all(&text, b"core.cont").unwrap();
    let f2 = dir.join("edited.cfg");
    std::fs::write(&f2, &text).unwrap();
    let get = |k: &str| {
        let o = common::git_raw(&dir, &["config", "--file", f2.to_str().unwrap(), "--get", k], None);
        (o.status.code(), o.stdout)
    };
    assert_eq!(get("core.filemode").1, b"false\n");
    assert_eq!(get("remote.origin.pushurl").1, b" spaced; value\t\n");
    assert_eq!(get("newsec.sub.key").1, b"line1\nline2\n");
    assert_eq!(get("core.cont").0, Some(1));
    assert_eq!(get("core.spaced").1, b"a   b  c\n"); // git keeps the run length, tabs become spaces
    let all = common::git(&dir, &["config", "--file", f2.to_str().unwrap(), "--list"]);
    let mut c2 = Config::new();
    c2.load(b"edited", &text, &mut config::NoIncludes).unwrap();
    assert_eq!(String::from_utf8_lossy(&c2.list()), String::from_utf8_lossy(&all));
}

fn write(root: &Path, rel: &str, data: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

fn walk(root: &Path, rel: &str, out: &mut Vec<String>) {
    for e in std::fs::read_dir(root.join(rel)).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if rel.is_empty() && name == ".git" {
            continue;
        }
        let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if e.file_type().unwrap().is_dir() {
            walk(root, &r, out);
        } else {
            out.push(r);
        }
    }
}

#[test]
fn gitignore_matches_git() {
    let dir = common::scratch("ignore");
    common::git(&dir, &["init", "-q"]);
    write(&dir, ".gitignore", "*.log\n!keep.log\n/build/\ndoc/*.html\n**/tmp\nfoo/**/bar\n\\#hash\ntrail\\ \n*.[oa]\ndironly/\n# comment\n\n!/build/keep\nlit[!x]y\n*~\n.*.swp\nnest/*/deep\n");
    write(&dir, "sub/.gitignore", "!*.log\nx?.txt\n/anch\n[[:upper:]]*.up\n");
    write(&dir, ".git/info/exclude", "excluded-by-info\n");
    let files = [
        "a.log", "keep.log", "sub/b.log", "sub/deeper/c.log", "build/out.bin", "build/keep", "sub/build/x", "doc/a.html",
        "doc/sub/b.html", "tmp/t", "a/b/tmp/t", "c/tmp", "foo/bar", "foo/x/y/bar", "foo/x/bar/z", "#hash", "trail ", "trail",
        "x.o", "x.a", "x.c", "dironly/f", "sub/dironly/f", "dironly2", "litay", "litxy", "file~", ".a.swp", "nest/a/deep",
        "nest/a/b/deep", "sub/x1.txt", "sub/x12.txt", "x1.txt", "sub/anch", "sub/q/anch", "anch", "sub/Abc.up", "sub/abc.up",
        "excluded-by-info", "sub/excluded-by-info", "plain.txt",
    ];
    for f in files {
        write(&dir, f, "x");
    }
    let want = common::git(&dir, &["ls-files", "-o", "-i", "--exclude-standard"]);
    let mut want: Vec<String> = String::from_utf8_lossy(&want).lines().map(|s| s.to_string()).collect();
    want.sort();
    // Ours: info/exclude, root .gitignore, sub/.gitignore.
    let mut ig = Ignore::default();
    ig.lists.push(PatternList::parse(&std::fs::read(dir.join(".git/info/exclude")).unwrap(), b""));
    ig.lists.push(PatternList::parse(&std::fs::read(dir.join(".gitignore")).unwrap(), b""));
    ig.lists.push(PatternList::parse(&std::fs::read(dir.join("sub/.gitignore")).unwrap(), b"sub/"));
    // git ranks info/exclude BELOW every .gitignore — our stack order mirrors that.
    let mut all = Vec::new();
    walk(&dir, "", &mut all);
    let mut ours: Vec<String> = all.into_iter().filter(|p| ig.is_excluded_with_parents(p.as_bytes(), false)).collect();
    ours.sort();
    assert_eq!(ours, want);
    println!("gitignore: {} of {} paths ignored, identical to git", ours.len(), files.len());
}

#[test]
fn gitattributes_match_git() {
    let dir = common::scratch("attrs");
    common::git(&dir, &["init", "-q"]);
    write(&dir, ".gitattributes", "[attr]mymacro text eol=lf -diff\n*.txt text\n*.bin binary\n*.c diff=cpp whitespace=trailing\ndocs/** linguist-documentation\n*.sh mymacro\n/root.only foo=bar\nx/*.y !text\n");
    write(&dir, "x/.gitattributes", "*.y special -text\n*.txt -text eol=crlf\n");
    let paths = ["a.txt", "a.bin", "m.c", "docs/a/b.md", "run.sh", "root.only", "x/root.only", "x/q.y", "x/a.txt", "x/deep/a.txt", "none"];
    let input: String = paths.iter().map(|p| format!("{p}\n")).collect();
    let out = common::git_raw(&dir, &["check-attr", "-a", "--stdin"], Some(input.as_bytes()));
    assert!(out.status.success());
    let mut want: std::collections::BTreeMap<String, Vec<(String, String)>> = Default::default();
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = l.splitn(3, ": ");
        let (p, a, v) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        want.entry(p.to_string()).or_default().push((a.to_string(), v.to_string()));
    }
    let mut at = Attributes::default();
    at.files.push(AttrFile::parse(&std::fs::read(dir.join(".gitattributes")).unwrap(), b"", true));
    at.files.push(AttrFile::parse(&std::fs::read(dir.join("x/.gitattributes")).unwrap(), b"x/", false));
    for p in paths {
        let mut w = want.remove(p).unwrap_or_default();
        w.sort();
        let ours: Vec<(String, String)> = at
            .check_all(p.as_bytes())
            .into_iter()
            .map(|(n, v)| {
                let v = match v {
                    AttrValue::Set => "set".to_string(),
                    AttrValue::Unset => "unset".to_string(),
                    AttrValue::Value(x) => String::from_utf8_lossy(&x).into_owned(),
                    AttrValue::Unspecified => "unspecified".to_string(),
                };
                (String::from_utf8_lossy(&n).into_owned(), v)
            })
            .collect();
        assert_eq!(ours, w, "{p}");
    }
}
