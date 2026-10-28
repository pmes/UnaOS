//! CHARTER: Vairë — the UnaOS_Installer contribute face (vessel module; seam shared-core git_core, the typed-command redaction midden_core::redact)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERCONTRIBUTE (LEDGER SR428): the landing goes back to the project. Peter 2026-10-27: "we want to make it easy
// for people to get UnaOS to work with their computer by contributing ... share their work back with us, submit bug
// reports". The landing face (INSTALLERLANDING SR415) leaves `docs/dev/evidence/landing-<stamp>/` untracked in the
// person's checkout; this face turns it into ONE local git commit on `contrib/landing-<stamp>`, after ONE redaction
// pass. It NEVER pushes and never touches the network: after a commit it prints the three commands (review, push, PR)
// the person runs themselves (INSTALLER-CONTRIBUTE-PUSH owed).
//
// The commit is built IN THE OBJECT STORE ONLY, through the repo's own git (git_core, Vairë's shared core; R83): the
// redacted blobs and HEAD's flattened tree go into an in-memory `Index`, `Repository::write_tree` writes the trees (never
// the index file), `Commit::new` takes HEAD as its parent, and `Repository::update_ref` creates the branch with a null
// old value (an existing branch refuses). The person's HEAD, index and working tree are never touched: a dirty tree is
// neither refused nor at risk, the redacted copies live only in the commit, and the originals on disk stay byte for byte.
// No git binary is needed to commit; the git CLI is only the person's tool to push.
//
// The redaction (one byte-level pass per line; NUL padding and non-ASCII bytes kept verbatim; a VALUE IS NEVER PRINTED):
//   mac     six two-hex groups joined by `:` or `-`                       -> xx:xx:xx:xx:xx:xx
//   ssid    the value after `ssid=` up to the next ` k=`, ` ->`, ` ::`    -> <ssid>  (the PCI `ssid=hhhh:hhhh` is kept)
//   serial  the value after `serial=`, `psn=`, `serial:`                  -> <serial>
//   user    `user=` values; the segment after /home/, /Users/, /media/, /run/media/, /Volumes/, \Users\; the literal
//           checkout path -> <checkout> and the literal $HOME -> ~
//   ip      a dotted quad outside the private, loopback and link-local ranges -> <ip>
//   cmd     a `cmd="…"` span `midden_core::redact` changes (the R65 table both rings run; idempotent on a CONSOLEFIX log)
// Unredactable (the contribution REFUSES and names file:line:class): a key's shape (`sk-ant-`, `AIza`, `ghp_`, `gho_`,
// `github_pat_`, `xoxb-`, `xoxp-` with 16+ key characters after the prefix; `PRIVATE KEY-----`) or a `password=`,
// `passphrase=`, `psk=` field with a value. The kernel knows no host hostname: no file class of a landing carries one.
//
// Wire:
//   [installer] contribute landing=<stamp|-> branch=<contrib/landing-…|-> files=<n> redacted=<n> ok=<1|0> why=<-|review-pending|no-git|no-landing|branch-exists|unredactable|git-error>
//   [installer] contribute next: <command>                                  (three, after ok=1)
//   [installer] contribute redact-check landing=<stamp> files=<n> redacted=<n> unredactable=<n>
//   [installer] contribute redact file=<name> line=<n> class=<class>        (first 20, then `redact more=<n>`)
//   [installer] contribute bugreport landing=<stamp|-> fails=<n> redacted=<n> ok=<1|0> why=<-|no-landing|unredactable|write-error>
//   [installer] contribute next: (cd <landing dir> && gh issue create --repo <owner/repo> --title '<title>' --body-file BUGREPORT.md)
//
// INSTALLERBUGREPORT (LEDGER SR433): `contribute bugreport [<dir>]` writes BUGREPORT.md beside the landing (on disk, for
// the person to read before filing) from the SAME redacted plan: the title (board, image, the first FAIL verdict's
// name), the environment table, the verdict counts, the failing verdict lines quoted (redacted, first 20), the landing
// files; an unredactable landing refuses and writes nothing. It prints the `gh issue create` command and never calls
// the network (INSTALLER-BUGREPORT-GH owed). The repo slug is git_core's `remote.origin.url` on github.com, else the
// placeholder `<owner>/<repo>`; the URL itself is never printed.
//
// CONTRIBUTEREVIEW (LEDGER SR445): the commit waits for a yes. The first `contribute [<dir>]` plans, redacts and HOLDS
// the plan in memory (the scratch copy: the redacted bytes exist only there and then in the commit, never on disk),
// prints the wire line with `ok=0 why=review-pending` (files= the blobs the confirm would commit) and shows the review
// in the pane: per file its name, bytes and redacted count with its first redacted lines, and the confirm phrase. The
// confirm commits exactly the reviewed plan: the Confirm button (`contribute confirm`) or the phrase
// `contribute yes <stamp>` typed in the field (the card's confirm door: a TextField whose placeholder is the phrase,
// routed back through `on_input`) or inline. `contribute cancel` drops it; `contribute --yes [<dir>]` is the one-shot
// path for scripts. A landing whose branch exists, or that lies outside the repo, refuses before any review.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use bandy::SMessage;
use git_core::Repository;
use git_core::index::{Entry, Index};
use git_core::object::{Commit, Kind, Signature, mode};
use quartzite::tetra::{ConsoleLine, ConsoleTetra, TetraNode, TextAction};

use crate::landing::verdict_name; // LANDINGSCORE (SR437): the verdict-name rule moved beside landing::verdict, body unchanged

/// The identity a commit takes when the person's git names none.
pub const FALLBACK_NAME: &str = "UnaOS Contributor";
pub const FALLBACK_EMAIL: &str = "contrib@unaos.invalid";
/// The generated census, committed beside the logs (never written on disk).
pub const CONTRIBUTION: &str = "CONTRIBUTION.md";
/// The generated bug report, written beside the landing (never read back by `plan`).
pub const BUGREPORT: &str = "BUGREPORT.md";
/// How many failing verdict lines a bug report quotes, and how long a quoted line may be.
const QUOTED: usize = 20;
const QUOTE_CHARS: usize = 240;
/// How many redaction hits a check prints before `more=`.
const SHOWN: usize = 20;
/// CONTRIBUTEREVIEW (SR445): how many redacted lines per file the review quotes.
const REVIEWED: usize = 3;

// ── the redaction ───────────────────────────────────────────────────────────────────────────────

/// What a redaction hit was. `Key` and `SecretField` are unredactable: they refuse the contribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Mac,
    Ssid,
    Serial,
    User,
    Ip,
    Cmd,
    Key,
    SecretField,
}

/// Every class, in the order CONTRIBUTION.md lists them.
pub const CLASSES: [Class; 8] = [Class::Mac, Class::Ssid, Class::Serial, Class::User, Class::Ip, Class::Cmd, Class::Key, Class::SecretField];

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Mac => "mac",
            Class::Ssid => "ssid",
            Class::Serial => "serial",
            Class::User => "user",
            Class::Ip => "ip",
            Class::Cmd => "cmd",
            Class::Key => "key",
            Class::SecretField => "secret-field",
        }
    }
    pub fn unredactable(self) -> bool {
        matches!(self, Class::Key | Class::SecretField)
    }
}

/// The literal strings of THIS host that identify the person (the checkout path, $HOME), longest first.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    literals: Vec<(Vec<u8>, &'static str)>,
}

impl Redactor {
    /// `checkout` becomes `<checkout>`, `home` becomes `~` (a root-ish or empty path is ignored).
    pub fn new(checkout: Option<&str>, home: Option<&str>) -> Self {
        let mut literals = Vec::new();
        for (lit, with) in [(checkout, "<checkout>"), (home, "~")] {
            let Some(l) = lit.map(|l| l.trim_end_matches('/')) else { continue };
            if l.len() >= 2 {
                literals.push((l.as_bytes().to_vec(), with));
            }
        }
        literals.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Redactor { literals }
    }

    /// Redact `bytes`: the redacted copy and every hit as (1-based line, class).
    pub fn redact(&self, bytes: &[u8]) -> (Vec<u8>, Vec<(usize, Class)>) {
        let mut out = Vec::with_capacity(bytes.len());
        let mut hits = Vec::new();
        for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            let mut classes = Vec::new();
            out.extend_from_slice(&self.redact_line(line, &mut classes));
            hits.extend(classes.into_iter().map(|c| (i + 1, c)));
        }
        (out, hits)
    }

    /// One line through every pass, in order.
    pub fn redact_line(&self, line: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
        if !line.iter().any(|b| b.is_ascii_graphic()) {
            return line.to_vec();
        }
        let mut l = line.to_vec();
        for (lit, with) in &self.literals {
            l = replace_spans(&l, find_all(&l, lit).into_iter().map(|i| (i, i + lit.len(), with.as_bytes().to_vec())).collect(), Class::User, hits);
        }
        unredactable(&l, hits);
        l = pass_cmd(&l, hits);
        l = pass_ssid(&l, hits);
        for key in ["serial=", "psn=", "serial:"] {
            l = pass_value(&l, key, "<serial>", Class::Serial, hits);
        }
        l = pass_value(&l, "user=", "<user>", Class::User, hits);
        l = pass_paths(&l, hits);
        l = pass_mac(&l, hits);
        pass_ip(&l, hits)
    }
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return v;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            v.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    v
}

/// Rebuild `l` with each (start, end, with) span replaced (spans ascending, non-overlapping); one hit per span.
fn replace_spans(l: &[u8], spans: Vec<(usize, usize, Vec<u8>)>, class: Class, hits: &mut Vec<Class>) -> Vec<u8> {
    if spans.is_empty() {
        return l.to_vec();
    }
    let mut out = Vec::with_capacity(l.len());
    let mut at = 0;
    for (s, e, with) in spans {
        if s < at {
            continue;
        }
        out.extend_from_slice(&l[at..s]);
        out.extend_from_slice(&with);
        at = e;
        hits.push(class);
    }
    out.extend_from_slice(&l[at..]);
    out
}

/// A key's start: the byte before is not a word byte.
fn word_start(l: &[u8], i: usize) -> bool {
    i == 0 || !(l[i - 1].is_ascii_alphanumeric() || l[i - 1] == b'_')
}

fn key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Already a placeholder (`<ssid>`, `<user>` …).
fn placeholder(v: &[u8]) -> bool {
    v.len() >= 2 && v[0] == b'<' && v[v.len() - 1] == b'>'
}

/// The unredactable classes: noted, never replaced (the contribution refuses).
fn unredactable(l: &[u8], hits: &mut Vec<Class>) {
    for p in ["sk-ant-", "AIza", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-"] {
        for i in find_all(l, p.as_bytes()) {
            let tail = l[i + p.len()..].iter().take_while(|&&b| key_byte(b)).count();
            if word_start(l, i) && tail >= 16 {
                hits.push(Class::Key);
            }
        }
    }
    if !find_all(l, b"PRIVATE KEY-----").is_empty() {
        hits.push(Class::Key);
    }
    for k in ["password=", "passphrase=", "psk="] {
        for i in find_all(l, k.as_bytes()) {
            let v: Vec<u8> = l[i + k.len()..].iter().copied().take_while(|b| !b.is_ascii_whitespace()).collect();
            let empty = v.is_empty() || v.iter().all(|&b| b == b'-' || b == b'*') || placeholder(&v);
            if word_start(l, i) && !empty {
                hits.push(Class::SecretField);
            }
        }
    }
}

/// `cmd="…"` spans through Midden's own redaction table.
fn pass_cmd(l: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
    let mut spans = Vec::new();
    for i in find_all(l, b"cmd=\"") {
        let s = i + 5;
        let Some(len) = l[s..].iter().position(|&b| b == b'"') else { continue };
        let Ok(text) = std::str::from_utf8(&l[s..s + len]) else { continue };
        if let Some(r) = midden_core::redact(text) {
            if r != text {
                spans.push((s, s + len, r.into_bytes()));
            }
        }
    }
    replace_spans(l, spans, Class::Cmd, hits)
}

/// The end of an `ssid=` value: the next ` <word>=`, ` ->`, ` ::`, or the line's end.
fn ssid_end(l: &[u8], s: usize) -> usize {
    if l.get(s) == Some(&b'"') {
        return l[s + 1..].iter().position(|&b| b == b'"').map(|p| s + p + 2).unwrap_or(l.len());
    }
    let mut j = s;
    while j < l.len() {
        if l[j] == b' ' {
            let r = &l[j + 1..];
            if r.starts_with(b"->") || r.starts_with(b"::") {
                return j;
            }
            let w = r.iter().take_while(|&&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_').count();
            if w > 0 && r.get(w) == Some(&b'=') {
                return j;
            }
        }
        j += 1;
    }
    l.len()
}

/// The PCI subsystem id shape `hhhh:hhhh` (pci.rs), which is not a network name.
fn pci_shape(v: &[u8]) -> bool {
    v.len() == 9 && v[4] == b':' && v.iter().enumerate().all(|(i, b)| i == 4 || b.is_ascii_hexdigit())
}

fn pass_ssid(l: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
    let mut spans = Vec::new();
    for i in find_all(l, b"ssid=") {
        if !word_start(l, i) {
            continue;
        }
        let s = i + 5;
        let e = ssid_end(l, s);
        let v = &l[s..e];
        if v.is_empty() || placeholder(v) || pci_shape(v) || v == b"-" {
            continue;
        }
        spans.push((s, e, b"<ssid>".to_vec()));
    }
    replace_spans(l, spans, Class::Ssid, hits)
}

/// A `key` value up to whitespace, `,`, `;` or `)`.
fn pass_value(l: &[u8], key: &str, with: &str, class: Class, hits: &mut Vec<Class>) -> Vec<u8> {
    let mut spans = Vec::new();
    for i in find_all(l, key.as_bytes()) {
        if !word_start(l, i) {
            continue;
        }
        let s = i + key.len();
        let n = l[s..].iter().take_while(|&&b| !(b.is_ascii_whitespace() || b == b',' || b == b';' || b == b')')).count();
        let v = &l[s..s + n];
        if v.is_empty() || v == b"-" || placeholder(v) {
            continue;
        }
        spans.push((s, s + n, with.as_bytes().to_vec()));
    }
    replace_spans(l, spans, class, hits)
}

/// The per-person path segment after a home or removable-media prefix.
fn pass_paths(l: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
    let mut l = l.to_vec();
    for (prefix, with) in [("/run/media/", "<user>"), ("/home/", "<user>"), ("/Users/", "<user>"), ("/media/", "<user>"), ("/Volumes/", "<volume>"), ("\\Users\\", "<user>")] {
        let mut spans = Vec::new();
        for i in find_all(&l, prefix.as_bytes()) {
            let s = i + prefix.len();
            let n = l[s..].iter().take_while(|&&b| !(b == b'/' || b == b'\\' || b.is_ascii_whitespace() || b"`\"')(,:;".contains(&b))).count();
            let v = &l[s..s + n];
            if v.is_empty() || placeholder(v) {
                continue;
            }
            spans.push((s, s + n, with.as_bytes().to_vec()));
        }
        l = replace_spans(&l, spans, Class::User, hits);
    }
    l
}

fn pass_mac(l: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 17 <= l.len() {
        let w = &l[i..i + 17];
        let sep = w[2];
        let shaped = (sep == b':' || sep == b'-')
            && (0..6).all(|g| w[g * 3].is_ascii_hexdigit() && w[g * 3 + 1].is_ascii_hexdigit() && (g == 5 || w[g * 3 + 2] == sep));
        let before = i == 0 || !(l[i - 1].is_ascii_alphanumeric() || l[i - 1] == sep);
        let after = l.get(i + 17).is_none_or(|&b| !(b.is_ascii_alphanumeric() || b == sep));
        if shaped && before && after {
            spans.push((i, i + 17, b"xx:xx:xx:xx:xx:xx".to_vec()));
            i += 17;
        } else {
            i += 1;
        }
    }
    replace_spans(l, spans, Class::Mac, hits)
}

/// Private, loopback, link-local, unspecified and broadcast addresses say nothing about who you are.
fn private_ip(o: [u32; 4]) -> bool {
    o[0] == 10
        || o[0] == 127
        || (o[0] == 172 && (16..32).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
        || o == [0, 0, 0, 0]
        || o == [255, 255, 255, 255]
}

fn pass_ip(l: &[u8], hits: &mut Vec<Class>) -> Vec<u8> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < l.len() {
        let start_ok = l[i].is_ascii_digit() && (i == 0 || !(l[i - 1].is_ascii_alphanumeric() || l[i - 1] == b'.'));
        if !start_ok {
            i += 1;
            continue;
        }
        let mut o = [0u32; 4];
        let mut j = i;
        let mut ok = true;
        for (g, slot) in o.iter_mut().enumerate() {
            let n = l[j..].iter().take_while(|b| b.is_ascii_digit()).count();
            if n == 0 || n > 3 {
                ok = false;
                break;
            }
            *slot = std::str::from_utf8(&l[j..j + n]).ok().and_then(|s| s.parse().ok()).unwrap_or(999);
            j += n;
            if g < 3 {
                if l.get(j) != Some(&b'.') {
                    ok = false;
                    break;
                }
                j += 1;
            }
        }
        let tail_ok = ok && l.get(j).is_none_or(|&b| !(b.is_ascii_alphanumeric() || (b == b'.' && l.get(j + 1).is_some_and(|c| c.is_ascii_digit()))));
        if tail_ok && o.iter().all(|&x| x <= 255) {
            if !private_ip(o) {
                spans.push((i, j, b"<ip>".to_vec()));
            }
            i = j;
        } else {
            i += 1;
            while i < l.len() && l[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    replace_spans(l, spans, Class::Ip, hits)
}

// ── the plan ────────────────────────────────────────────────────────────────────────────────────

/// One landing file, redacted.
#[derive(Clone, Debug)]
pub struct FileOut {
    pub name: String,
    pub bytes: Vec<u8>,
    pub hits: Vec<(usize, Class)>,
}

/// A landing dir read and redacted (nothing written).
#[derive(Clone, Debug)]
pub struct Plan {
    pub dir: PathBuf,
    /// `landing-<stamp>`.
    pub name: String,
    pub files: Vec<FileOut>,
}

impl Plan {
    /// The `<stamp>` of `landing-<stamp>`.
    pub fn stamp(&self) -> &str {
        self.name.strip_prefix("landing-").unwrap_or(&self.name)
    }
    pub fn branch(&self) -> String {
        format!("contrib/{}", self.name)
    }
    fn hits(&self) -> impl Iterator<Item = (&str, usize, Class)> {
        self.files.iter().flat_map(|f| f.hits.iter().map(move |&(n, c)| (f.name.as_str(), n, c)))
    }
    pub fn redacted(&self) -> usize {
        self.hits().filter(|h| !h.2.unredactable()).count()
    }
    pub fn unredactable(&self) -> usize {
        self.hits().filter(|h| h.2.unredactable()).count()
    }
    pub fn count(&self, c: Class) -> usize {
        self.hits().filter(|h| h.2 == c).count()
    }
    /// The redacted landing.md (empty when the landing has none).
    pub fn landing_md(&self) -> String {
        self.files.iter().find(|f| f.name == "landing.md").map(|f| String::from_utf8_lossy(&f.bytes).into_owned()).unwrap_or_default()
    }
    /// The hit lines: `[installer] contribute redact file= line= class=` (only `unredactable` ones when asked), capped.
    pub fn hit_lines(&self, only_unredactable: bool) -> Vec<String> {
        let all: Vec<_> = self.hits().filter(|h| !only_unredactable || h.2.unredactable()).collect();
        let mut v: Vec<String> =
            all.iter().take(SHOWN).map(|(f, n, c)| format!("[installer] contribute redact file={f} line={n} class={}", c.name())).collect();
        if all.len() > SHOWN {
            v.push(format!("[installer] contribute redact more={}", all.len() - SHOWN));
        }
        v
    }
}

/// Read every regular, non-dot file directly in `dir` (sorted; an on-disk CONTRIBUTION.md is the commit's own and
/// BUGREPORT.md the bug report's own, both skipped) and redact it.
pub fn plan(dir: &Path, r: &Redactor) -> Result<Plan, &'static str> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no-landing")?;
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|_| "no-landing")?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != CONTRIBUTION && n != BUGREPORT)
        .collect();
    names.sort();
    if names.is_empty() {
        return Err("no-landing");
    }
    let mut files = Vec::with_capacity(names.len());
    for n in names {
        let raw = std::fs::read(dir.join(&n)).map_err(|_| "no-landing")?;
        let (bytes, hits) = r.redact(&raw);
        files.push(FileOut { name: n, bytes, hits });
    }
    Ok(Plan { dir: dir.to_path_buf(), name, files })
}

/// A `- <key>: <value>` line of landing.md, backticks trimmed; a value that opens with a backtick is its first
/// backticked span (landing.md's `- image: `<sha>` (the booted kernel's … sha)` is the bare sha).
fn landing_field(md: &str, key: &str) -> Option<String> {
    let p = format!("- {key}: ");
    md.lines().find_map(|l| l.strip_prefix(&p)).map(|v| {
        let v = v.trim();
        match v.strip_prefix('`').and_then(|r| r.split_once('`')) {
            Some((span, _)) => span.to_string(),
            None => v.trim_matches('`').to_string(),
        }
    })
}

/// `<pass>/<fail>/<skip>` from landing.md's verdicts line.
fn verdicts(md: &str) -> String {
    let Some(v) = landing_field(md, "verdicts") else { return String::from("-") };
    let n: Vec<String> = v.split(" (").next().unwrap_or("").split_whitespace().filter(|w| w.parse::<u32>().is_ok()).map(str::to_string).collect();
    if n.len() == 3 { n.join("/") } else { String::from("-") }
}

/// The census of the landing: (board, image, verdicts, device id).
fn census_of(md: &str) -> (String, String, String, String) {
    let f = |k: &str| landing_field(md, k).unwrap_or_else(|| String::from("-"));
    (f("board"), f("image"), verdicts(md), f("device"))
}

/// CONTRIBUTION.md: the machine census and what was redacted, by class (never a value).
pub fn contribution_md(p: &Plan, os: &str, arch: &str, card_model: Option<&str>) -> String {
    let md = p.landing_md();
    let (board, image, v, _) = census_of(&md);
    let mut s = format!("# Contribution — {}\n\n", p.name);
    s.push_str("INSTALLER-CONTRIBUTE (SR428): the UnaOS_Installer committed this landing on a local `contrib/` branch after one redaction pass. Nothing was pushed; the person pushes it.\n\n");
    s.push_str("## Machine\n\n");
    s.push_str(&format!("- board: `{board}`\n- image: `{image}`\n- boots: {}\n- verdicts: {v} (PASS/FAIL/SKIP)\n- score: {}\n", landing_field(&md, "boots").unwrap_or_else(|| String::from("-")), landing_field(&md, "score").unwrap_or_else(|| String::from("-")))); // LANDINGSCORE (SR437): the grade line, read from landing.md like the census
    s.push_str(&format!("- host: {os} {arch}\n- card: {}\n\n", card_model.filter(|m| !m.trim().is_empty()).unwrap_or("-")));
    s.push_str("## Files\n\n");
    for f in &p.files {
        s.push_str(&format!("- `{}`: {} bytes, {} value(s) redacted\n", f.name, f.bytes.len(), f.hits.len()));
    }
    s.push_str("\n## Redacted, by class\n\n");
    for c in CLASSES.iter().filter(|c| !c.unredactable()) {
        s.push_str(&format!("- {}: {}\n", c.name(), p.count(*c)));
    }
    s.push_str("\nNever in this commit: a serial, a MAC, a Wi-Fi name, a username or home path, a public IP, a typed secret, a key (a key or a password field refuses the whole contribution). Limits: a username that appears bare in prose (not in a path or a `user=` field) is not recognised; the kernel knows no host hostname, so none is looked for.\n");
    s
}

// ── the commit ──────────────────────────────────────────────────────────────────────────────────

fn git(_: git_core::Error) -> &'static str {
    "git-error"
}

/// A landed commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landed {
    pub branch: String,
    pub commit: String,
    /// The repository's working tree (the push commands run there).
    pub root: PathBuf,
    /// Blobs committed under the landing dir (CONTRIBUTION.md included).
    pub files: usize,
}

/// Where `p` lands in `repo`: (the canonical working tree, the landing dir's path prefix in the tree, the branch's ref).
/// A landing outside the working tree refuses `no-git`, an existing branch `branch-exists` (CONTRIBUTEREVIEW SR445: the
/// review asks this before it shows anything; `commit` asks it again before it writes).
fn target(repo: &Repository, p: &Plan) -> Result<(PathBuf, String, String), &'static str> {
    let wt = repo.work_tree.clone().ok_or("no-git")?;
    let wt = wt.canonicalize().unwrap_or(wt);
    let dir = p.dir.canonicalize().map_err(|_| "no-landing")?;
    let rel = dir.strip_prefix(&wt).map_err(|_| "no-git")?;
    let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if rel.is_empty() {
        return Err("no-git");
    }
    let prefix = format!("{}/", rel.join("/"));
    let refname = format!("refs/heads/{}", p.branch());
    if matches!(repo.resolve_ref(&refname), Ok(Some((_, Some(_))))) {
        return Err("branch-exists");
    }
    Ok((wt, prefix, refname))
}

/// Commit `p` (redacted) + `contribution` on `contrib/<landing dir>` in the repository holding `checkout`, in the object
/// store only: HEAD, the index and the working tree are never touched.
pub fn commit(checkout: &Path, p: &Plan, contribution: &str, now: i64) -> Result<Landed, &'static str> {
    if p.unredactable() > 0 {
        return Err("unredactable");
    }
    let repo = Repository::discover(checkout).map_err(|_| "no-git")?;
    let (wt, prefix, refname) = target(&repo, p)?;
    let branch = p.branch();
    let (_, head) = repo.head().map_err(git)?;
    let mut tree: BTreeMap<Vec<u8>, (u32, git_core::ObjectId)> = match head {
        Some(h) => {
            let (k, d) = repo.read(&h).map_err(git)?;
            if k != Kind::Commit {
                return Err("git-error");
            }
            let t = Commit::parse(repo.hash, &d).map_err(git)?.tree().ok_or("git-error")?;
            repo.flatten_tree(&t).map_err(git)?
        }
        None => BTreeMap::new(),
    };
    tree.retain(|path, _| !path.starts_with(prefix.as_bytes()));
    let mut files = 0;
    for (name, bytes) in p.files.iter().map(|f| (f.name.as_str(), f.bytes.as_slice())).chain(std::iter::once((CONTRIBUTION, contribution.as_bytes()))) {
        let id = repo.write(Kind::Blob, bytes).map_err(git)?;
        tree.insert(format!("{prefix}{name}").into_bytes(), (mode::BLOB, id));
        files += 1;
    }
    let mut idx = Index::new(repo.hash);
    for (path, (m, id)) in tree {
        let mut e = Entry { mode: m, id: Some(id), path, ..Default::default() };
        e.fix_flags();
        idx.entries.push(e);
    }
    idx.sort();
    let root_tree = repo.write_tree(&idx).map_err(git)?;
    let fallback = Signature::new(FALLBACK_NAME.as_bytes(), FALLBACK_EMAIL.as_bytes(), now, 0);
    let author = repo.ident("AUTHOR").unwrap_or_else(|_| fallback.clone());
    let committer = repo.ident("COMMITTER").unwrap_or_else(|_| author.clone());
    let md = p.landing_md();
    let (board, image, v, _) = census_of(&md);
    let msg = format!(
        "contrib: {} — board={board} image={image} verdicts={v}\n\nINSTALLER-CONTRIBUTE (SR428): the UnaOS_Installer's Contribute verb committed this landing after one redaction pass (files={files} redacted={}); {CONTRIBUTION} says what was redacted, by class.\n",
        p.name,
        p.redacted()
    );
    let parents: Vec<git_core::ObjectId> = head.into_iter().collect();
    let c = Commit::new(root_tree, &parents, &author, &committer, msg.as_bytes());
    let id = repo.write(Kind::Commit, &c.serialize()).map_err(git)?;
    repo.update_ref(&refname, id, Some(repo.hash.null()), &committer, &format!("contribute: {}", p.name)).map_err(git)?;
    Ok(Landed { branch, commit: id.to_hex(), root: wt, files })
}

/// `[installer] contribute landing= branch= files= redacted= ok= why=`.
pub fn line(landing: &str, branch: &str, files: usize, redacted: usize, ok: bool, why: &str) -> String {
    format!("[installer] contribute landing={landing} branch={branch} files={files} redacted={redacted} ok={} why={why}", ok as u8)
}

/// The three commands the person runs: review, push, open the PR (never run here).
pub fn next_steps(root: &Path, branch: &str) -> [String; 3] {
    let r = root.display();
    [
        format!("[installer] contribute next: git -C {r} show --stat {branch}"),
        format!("[installer] contribute next: git -C {r} push origin {branch}"),
        format!("[installer] contribute next: (cd {r} && gh pr create --head {branch} --fill)"),
    ]
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The whole verb, synchronous (the face runs it on its own thread): the review then the commit, no yes asked (the
/// one-shot `contribute --yes` path). Returns the printed lines: the wire line first, then the next steps or the hit lines.
pub fn contribute_at(checkout: &Path, dir: Option<PathBuf>, r: &Redactor, card_model: &dyn Fn(&str) -> Option<String>, now: i64) -> Vec<String> {
    match review_at(checkout, dir, r) {
        Ok(p) => commit_plan(checkout, &p, card_model, now),
        Err(lines) => lines,
    }
}

/// CONTRIBUTEREVIEW (SR445): the first half of the verb, nothing written: the landing (`dir`, else the newest), the plan
/// (redacted), the unredactable refusal, and where it would land (`target`: no-git / branch-exists refuse here). Err is
/// the printed refusal: the wire line first, then the hit lines.
pub fn review_at(checkout: &Path, dir: Option<PathBuf>, r: &Redactor) -> Result<Plan, Vec<String>> {
    let Some(dir) = dir.or_else(|| crate::provider::newest_landing(checkout)).filter(|d| d.is_dir()) else {
        return Err(vec![line("-", "-", 0, 0, false, "no-landing")]);
    };
    let p = plan(&dir, r).map_err(|why| vec![line("-", "-", 0, 0, false, why)])?;
    if p.unredactable() > 0 {
        let mut v = vec![line(p.stamp(), "-", 0, p.redacted(), false, "unredactable")];
        v.extend(p.hit_lines(true));
        return Err(v);
    }
    let landing = Repository::discover(checkout).map_err(|_| "no-git").and_then(|repo| target(&repo, &p).map(|_| ()));
    if let Err(why) = landing {
        let branch = if why == "branch-exists" { p.branch() } else { String::from("-") };
        return Err(vec![line(p.stamp(), &branch, 0, p.redacted(), false, why)]);
    }
    Ok(p)
}

/// CONTRIBUTEREVIEW (SR445): the review the face shows before a commit (face rows, not wire lines), every byte from the
/// redacted plan: per file its name, bytes and redacted count, then its first [`REVIEWED`] redacted lines as
/// `  <file>:<line>: <line>` (trailing NULs and spaces trimmed, capped at [`QUOTE_CHARS`]); the census note; the phrase.
pub fn review_lines(p: &Plan) -> Vec<String> {
    let mut v = Vec::new();
    for f in &p.files {
        v.push(format!("{}: {} bytes, {} redacted", f.name, f.bytes.len(), f.hits.len()));
        let lines: Vec<&[u8]> = f.bytes.split(|&b| b == b'\n').collect();
        let mut shown: Vec<usize> = Vec::new();
        for &(n, _) in &f.hits {
            if shown.len() == REVIEWED {
                break;
            }
            if shown.contains(&n) {
                continue;
            }
            shown.push(n);
            let text = lines.get(n.wrapping_sub(1)).map(|l| String::from_utf8_lossy(l).into_owned()).unwrap_or_default();
            let l = text.trim_end_matches(|c: char| c == '\0' || c.is_whitespace());
            let cut: String = l.chars().take(QUOTE_CHARS).collect();
            let more = if l.chars().count() > QUOTE_CHARS { " …" } else { "" };
            v.push(format!("  {}:{n}: {cut}{more}", f.name));
        }
    }
    v.push(format!("{CONTRIBUTION}: generated at the commit (the census and the counts by class, never a value)"));
    v.push(format!("type `{}` or press Confirm to commit on {}; Cancel drops it", phrase(p), p.branch()));
    v
}

/// CONTRIBUTEREVIEW (SR445): the confirm phrase of a reviewed plan, `contribute yes <stamp>`.
pub fn phrase(p: &Plan) -> String {
    format!("contribute yes {}", p.stamp())
}

/// The second half of the verb: the census (the card model through `card_model`), CONTRIBUTION.md, the commit. Returns
/// the printed lines: the wire line first, then the next steps.
pub fn commit_plan(checkout: &Path, p: &Plan, card_model: &dyn Fn(&str) -> Option<String>, now: i64) -> Vec<String> {
    let (_, _, _, device) = census_of(&p.landing_md());
    let model = card_model(&device);
    let md = contribution_md(p, crate::host::Os::this().name(), std::env::consts::ARCH, model.as_deref());
    match commit(checkout, p, &md, now) {
        Ok(l) => {
            let mut v = vec![line(p.stamp(), &l.branch, l.files, p.redacted(), true, "-")];
            v.extend(next_steps(&l.root, &l.branch));
            v
        }
        Err(why) => {
            let branch = if why == "branch-exists" { p.branch() } else { String::from("-") };
            vec![line(p.stamp(), &branch, 0, p.redacted(), false, why)]
        }
    }
}

/// `contribute redact-check`: what would be redacted, by file:line:class (never the value).
pub fn redact_check(checkout: &Path, dir: Option<PathBuf>, r: &Redactor) -> Vec<String> {
    let Some(dir) = dir.or_else(|| crate::provider::newest_landing(checkout)).filter(|d| d.is_dir()) else {
        return vec![line("-", "-", 0, 0, false, "no-landing")];
    };
    match plan(&dir, r) {
        Ok(p) => {
            let mut v = vec![format!(
                "[installer] contribute redact-check landing={} files={} redacted={} unredactable={}",
                p.stamp(),
                p.files.len(),
                p.redacted(),
                p.unredactable()
            )];
            v.extend(p.hit_lines(false));
            v
        }
        Err(why) => vec![line("-", "-", 0, 0, false, why)],
    }
}

/// The git CLI on PATH (the push needs it; the commit does not). Nothing is run.
pub fn git_cli() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("git").is_file() || d.join("git.exe").is_file()))
}

// ── the bug report ──────────────────────────────────────────────────────────────────────────────

/// The boot number of `boot<n>.log` (so boot10 sorts after boot2); other names sort after every boot.
fn boot_order(name: &str) -> (u32, String) {
    let n = name.strip_prefix("boot").and_then(|r| r.strip_suffix(".log")).and_then(|d| d.parse().ok()).unwrap_or(u32::MAX);
    (n, name.to_string())
}

/// Every FAIL verdict (landing.rs `verdict`, the one rule) in the plan's redacted `*.log` files, by boot: the total and
/// the first [`QUOTED`] as `<file>:<line>: <line>` (trailing NULs and spaces trimmed, capped at [`QUOTE_CHARS`]).
pub fn failing(p: &Plan) -> (usize, Vec<String>) {
    let mut logs: Vec<&FileOut> = p.files.iter().filter(|f| f.name.ends_with(".log")).collect();
    logs.sort_by_key(|f| boot_order(&f.name));
    let mut total = 0;
    let mut quoted = Vec::new();
    for f in logs {
        for (i, raw) in f.bytes.split(|&b| b == b'\n').enumerate() {
            let text = String::from_utf8_lossy(raw);
            let l = text.trim_end_matches(|c: char| c == '\0' || c.is_whitespace());
            if crate::landing::verdict(l) != Some("FAIL") {
                continue;
            }
            total += 1;
            if quoted.len() < QUOTED {
                let cut: String = l.chars().take(QUOTE_CHARS).collect();
                let more = if l.chars().count() > QUOTE_CHARS { " …" } else { "" };
                quoted.push(format!("{}:{}: {cut}{more}", f.name, i + 1));
            }
        }
    }
    (total, quoted)
}

/// `owner/repo` of a github.com remote URL (`https://[userinfo@]github.com/o/r[.git]`, `ssh://git@github.com/o/r.git`,
/// `git@github.com:o/r.git`); anything else is None.
pub fn slug_of(url: &str) -> Option<String> {
    let u = url.trim();
    let (host, path) = if let Some((_, rest)) = u.split_once("://") {
        let (auth, path) = rest.split_once('/')?;
        (auth.rsplit('@').next().unwrap_or(auth), path)
    } else {
        let (auth, path) = u.split_once(':')?;
        if auth.contains('/') || !auth.contains('@') {
            return None;
        }
        (auth.rsplit('@').next().unwrap_or(auth), path)
    };
    let host = host.split(':').next().unwrap_or(host);
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() != 2 {
        return None;
    }
    let repo = segs[1].strip_suffix(".git").unwrap_or(segs[1]);
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    (ok(segs[0]) && ok(repo)).then(|| format!("{}/{repo}", segs[0]))
}

/// The repo slug for `gh issue create --repo`: git_core's `remote.origin.url` on github.com, else `<owner>/<repo>`.
/// The URL is never printed (a token in it never leaves).
pub fn repo_slug(checkout: &Path) -> String {
    Repository::discover(checkout)
        .ok()
        .and_then(|r| r.config.get(b"remote.origin.url").map(|u| String::from_utf8_lossy(u).into_owned()))
        .and_then(|u| slug_of(&u))
        .unwrap_or_else(|| String::from("<owner>/<repo>"))
}

/// The checkout's HEAD commit, 9 hex digits, else `-`.
fn checkout_head(checkout: &Path) -> String {
    Repository::discover(checkout)
        .ok()
        .and_then(|r| r.head().ok())
        .and_then(|(_, id)| id)
        .map(|id| id.to_hex().chars().take(9).collect())
        .unwrap_or_else(|| String::from("-"))
}

/// One shell word in single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The bug report: (title, BUGREPORT.md), every byte from the redacted plan.
pub fn bugreport_md(p: &Plan, os: &str, arch: &str, card_model: Option<&str>, head: &str) -> (String, String) {
    let md = p.landing_md();
    let (board, image, v, _) = census_of(&md);
    let (fails, quoted) = failing(p);
    let title = match quoted.first() {
        Some(q) => format!("{board} {image}: {} FAIL", verdict_name(q.splitn(3, ':').nth(2).unwrap_or(q))),
        None => format!("{board} {image}: no FAIL verdict ({v})"),
    };
    let mut s = format!("# {title}\n\n");
    s.push_str("INSTALLER-BUGREPORT (SR433): written by the UnaOS_Installer from a landed boot log after the contribute redaction pass. Nothing was sent; read it, then file it.\n\n");
    s.push_str("## Environment\n\n| | |\n|---|---|\n");
    s.push_str(&format!("| board | `{board}` |\n| image | `{image}` |\n| verdicts | {v} (PASS/FAIL/SKIP) |\n| score | {} |\n", landing_field(&md, "score").unwrap_or_else(|| String::from("-")))); // LANDINGSCORE (SR437): the grade line, read from landing.md like the census
    s.push_str(&format!("| host | {os} {arch} |\n| card | {} |\n", card_model.filter(|m| !m.trim().is_empty()).unwrap_or("-")));
    s.push_str(&format!("| UnaOS commit | `{head}` |\n| landing | `{}` |\n\n", p.name));
    s.push_str(&format!("## Failing verdicts ({fails})\n\n"));
    if quoted.is_empty() {
        s.push_str("none\n\n");
    } else {
        s.push_str("```text\n");
        for q in &quoted {
            s.push_str(q);
            s.push('\n');
        }
        s.push_str("```\n");
        if fails > quoted.len() {
            s.push_str(&format!("… and {} more\n", fails - quoted.len()));
        }
        s.push('\n');
    }
    s.push_str("## Landing files\n\n");
    for f in &p.files {
        s.push_str(&format!("- `{}`: {} bytes, {} value(s) redacted\n", f.name, f.bytes.len(), f.hits.len()));
    }
    s.push_str("\n## Redacted, by class\n\n");
    for c in CLASSES.iter().filter(|c| !c.unredactable()) {
        s.push_str(&format!("- {}: {}\n", c.name(), p.count(*c)));
    }
    s.push_str("\nNever in this report: a serial, a MAC, a Wi-Fi name, a username or home path, a public IP, a typed secret, a key (a key or a password field refuses the report).\n");
    (title, s)
}

/// `[installer] contribute bugreport landing= fails= redacted= ok= why=`.
pub fn bugreport_line(landing: &str, fails: usize, redacted: usize, ok: bool, why: &str) -> String {
    format!("[installer] contribute bugreport landing={landing} fails={fails} redacted={redacted} ok={} why={why}", ok as u8)
}

/// The whole bug-report verb, synchronous: the landing (`dir`, else the newest), the plan, BUGREPORT.md written beside
/// it. Returns the printed lines: the wire line, then the `gh issue create` line or the unredactable hit lines.
pub fn bugreport_at(checkout: &Path, dir: Option<PathBuf>, r: &Redactor, card_model: &dyn Fn(&str) -> Option<String>, slug: &str) -> Vec<String> {
    let Some(dir) = dir.or_else(|| crate::provider::newest_landing(checkout)).filter(|d| d.is_dir()) else {
        return vec![bugreport_line("-", 0, 0, false, "no-landing")];
    };
    let p = match plan(&dir, r) {
        Ok(p) => p,
        Err(why) => return vec![bugreport_line("-", 0, 0, false, why)],
    };
    if p.unredactable() > 0 {
        let mut v = vec![bugreport_line(p.stamp(), 0, p.redacted(), false, "unredactable")];
        v.extend(p.hit_lines(true));
        return v;
    }
    let (_, _, _, device) = census_of(&p.landing_md());
    let model = card_model(&device);
    let (title, body) = bugreport_md(&p, crate::host::Os::this().name(), std::env::consts::ARCH, model.as_deref(), &checkout_head(checkout));
    let (fails, _) = failing(&p);
    if std::fs::write(dir.join(BUGREPORT), body).is_err() {
        return vec![bugreport_line(p.stamp(), fails, p.redacted(), false, "write-error")];
    }
    vec![
        bugreport_line(p.stamp(), fails, p.redacted(), true, "-"),
        format!(
            "[installer] contribute next: (cd {} && gh issue create --repo {slug} --title {} --body-file {BUGREPORT})",
            dir.display(),
            shell_quote(&title)
        ),
    ]
}

/// The card model for a landing's device id through the card face's devices (none in tests, or for `-`).
fn card_model(host_devices: bool, id: &str) -> Option<String> {
    if !host_devices || id == "-" {
        return None;
    }
    crate::card::devices().into_iter().find(|d| d.id == id).map(|d| d.model)
}

// ── the face ────────────────────────────────────────────────────────────────────────────────────

enum Msg {
    Done(Vec<String>),
}

fn say_all(lines: &[String]) {
    for l in lines {
        println!("{l}");
    }
}

/// The contribute face's state. ONE owner: the vessel's serve task (no lock). The commit runs on its own thread, which
/// opens the repository (git_core's `Repository` is not Send) and reports over a channel this struct drains.
pub struct Contribute {
    pub checkout: PathBuf,
    pub home: Option<String>,
    pub verdict: String,
    /// The three next-step lines after a commit.
    pub next: Vec<String>,
    /// Look the card model up through the card face's devices (false in tests).
    host_devices: bool,
    rx: Option<Receiver<Msg>>,
    /// CONTRIBUTEREVIEW (SR445): the reviewed, redacted plan the confirm commits (in memory only), and its review rows.
    pending: Option<Plan>,
    review: Vec<String>,
}

impl Contribute {
    /// The probed checkout (else the one this vessel was built from), $HOME, the card face's devices.
    pub fn probe_in(checkout: Option<PathBuf>) -> Self {
        let home = std::env::var("HOME").ok();
        Self::with(checkout.unwrap_or_else(crate::landing::default_checkout), home, true)
    }

    /// A given checkout and home, no device enumeration (tests).
    pub fn probe_at(checkout: PathBuf, home: Option<String>) -> Self {
        Self::with(checkout, home, false)
    }

    fn with(checkout: PathBuf, home: Option<String>, host_devices: bool) -> Self {
        Contribute {
            checkout,
            home,
            verdict: String::from("after a landing: Check what would be redacted, then Contribute (a local commit; you push it)"),
            next: Vec::new(),
            host_devices,
            rx: None,
            pending: None,
            review: Vec::new(),
        }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// The checkout path (as given and canonical) and $HOME as literals.
    pub fn redactor(&self) -> Redactor {
        let canon = self.checkout.canonicalize().ok().map(|p| p.display().to_string());
        let given = self.checkout.display().to_string();
        let mut r = Redactor::new(Some(&given), self.home.as_deref());
        if let Some(c) = canon.filter(|c| *c != given) {
            r.literals.push((c.into_bytes(), "<checkout>"));
            r.literals.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        }
        r
    }

    /// The face's state line (not a wire line).
    pub fn status(&self) -> String {
        let landing = crate::provider::newest_landing(&self.checkout);
        let repo = Repository::discover(&self.checkout).ok();
        let branch = match (&repo, &landing) {
            (Some(r), Some(d)) => {
                let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if matches!(r.resolve_ref(&format!("refs/heads/contrib/{name}")), Ok(Some((_, Some(_))))) { "exists" } else { "new" }
            }
            _ => "-",
        };
        format!(
            "contribute: landing={} repo={} git_cli={} branch={branch}",
            landing.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| String::from("-")),
            repo.is_some() as u8,
            git_cli() as u8
        )
    }

    /// Drain the commit's verdict; true when anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let Ok(Msg::Done(lines)) = rx.try_recv() else { return false };
        self.rx = None;
        self.review.clear();
        self.verdict = lines.first().cloned().unwrap_or_default();
        self.next = lines.into_iter().skip(1).filter(|l| l.starts_with("[installer] contribute next:")).collect();
        true
    }

    /// `contribute redact-check [<dir>]`, synchronous.
    pub fn check(&mut self, arg: &str) {
        let lines = redact_check(&self.checkout, (!arg.is_empty()).then(|| PathBuf::from(arg)), &self.redactor());
        say_all(&lines);
        self.verdict = lines.first().cloned().unwrap_or_default();
    }

    /// `contribute bugreport [<dir>]`, synchronous: BUGREPORT.md beside the landing and the `gh issue create` line.
    pub fn bugreport(&mut self, arg: &str) {
        let host_devices = self.host_devices;
        let model = |id: &str| card_model(host_devices, id);
        let lines = bugreport_at(&self.checkout, (!arg.is_empty()).then(|| PathBuf::from(arg)), &self.redactor(), &model, &repo_slug(&self.checkout));
        say_all(&lines);
        self.verdict = lines.first().cloned().unwrap_or_default();
        self.next = lines.into_iter().skip(1).filter(|l| l.starts_with("[installer] contribute next:")).collect();
    }

    /// `contribute --yes [<dir>]` on its own thread: the one-shot path (review and commit, no yes asked).
    pub fn start(&mut self, arg: &str) {
        if self.busy() {
            return;
        }
        let checkout = self.checkout.clone();
        let dir = (!arg.is_empty()).then(|| PathBuf::from(arg));
        let r = self.redactor();
        self.pending = None;
        self.review.clear();
        self.run(move |model| contribute_at(&checkout, dir, &r, model, now_secs()));
    }

    /// The commit job on its own thread (it opens the repository there), its lines printed and drained by `poll`.
    fn run<F>(&mut self, job: F)
    where
        F: FnOnce(&dyn Fn(&str) -> Option<String>) -> Vec<String> + Send + 'static,
    {
        let (tx, rx) = channel();
        let host_devices = self.host_devices;
        self.verdict = String::from("committing the landing ...");
        self.next.clear();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let model = |id: &str| card_model(host_devices, id);
            let lines = job(&model);
            say_all(&lines);
            let _ = tx.send(Msg::Done(lines));
        });
    }

    /// CONTRIBUTEREVIEW (SR445): `contribute [<dir>]`, synchronous: the plan redacted and held (in memory only), the review
    /// rows, the wire line with `ok=0 why=review-pending`. Nothing is committed until the confirm.
    pub fn review(&mut self, arg: &str) {
        self.next.clear();
        self.review.clear();
        self.pending = None;
        match review_at(&self.checkout, (!arg.is_empty()).then(|| PathBuf::from(arg)), &self.redactor()) {
            Ok(p) => {
                let l = line(p.stamp(), &p.branch(), p.files.len() + 1, p.redacted(), false, "review-pending");
                say_all(std::slice::from_ref(&l));
                self.verdict = l;
                self.review = review_lines(&p);
                self.pending = Some(p);
            }
            Err(lines) => {
                say_all(&lines);
                self.verdict = lines.first().cloned().unwrap_or_default();
            }
        }
    }

    /// CONTRIBUTEREVIEW (SR445): the confirm: the Confirm button (`stamp` None) or the phrase `contribute yes <stamp>`
    /// (the pending stamp, case ignored). The reviewed plan moves to the commit thread; nothing pending, or another
    /// stamp, runs nothing.
    pub fn confirm(&mut self, stamp: Option<&str>) {
        let Some(p) = self.pending.as_ref() else {
            self.verdict = String::from("nothing to confirm — press Contribute to review first");
            return;
        };
        if stamp.is_some_and(|s| !s.eq_ignore_ascii_case(p.stamp())) {
            self.verdict = format!("type `{}`", phrase(p));
            return;
        }
        let Some(p) = self.pending.take() else { return };
        let checkout = self.checkout.clone();
        self.run(move |model| commit_plan(&checkout, &p, model, now_secs()));
    }

    /// CONTRIBUTEREVIEW (SR445): `contribute cancel`: the reviewed plan dropped, nothing committed.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.review.clear();
        self.verdict = String::from("review dropped — nothing was committed");
    }
}

/// Drain the commit's verdict into `c` (the serve task calls it beside `card::poll`).
pub fn poll(c: &mut Contribute) {
    c.poll();
}

/// The pane INSTALLERVESSEL's window shows (a fresh probe).
pub fn face() -> TetraNode {
    face_of(&Contribute::probe_in(None))
}

/// The pane for a given state: the status, the verdict, the review before a commit (CONTRIBUTEREVIEW SR445), the next
/// steps after a commit or a report, Check, Contribute, Confirm, Cancel and Report bug, and the confirm field (the card's
/// door: its placeholder is the phrase). The shape is the same with or without a review, so a live face patches leaves.
pub fn face_of(c: &Contribute) -> TetraNode {
    let line = |level: &str, text: String| ConsoleLine { level: level.to_string(), source: String::from("contribute"), text };
    let mut lines = vec![line("info", c.status()), line("info", c.verdict.clone())];
    lines.extend(c.review.iter().map(|n| line("info", n.clone())));
    lines.extend(c.next.iter().map(|n| line("info", n.clone())));
    if !git_cli() {
        lines.push(line("warn", String::from("no git on PATH: the commit works, the push needs git")));
    }
    let button = |id: &str, label: &str, cmd: &str| TetraNode::Button { id: id.into(), label: label.into(), action: SMessage::ConsoleInput(cmd.into()) };
    TetraNode::VStack(vec![
        TetraNode::Console(ConsoleTetra { lines, ..Default::default() }),
        TetraNode::HStack(vec![
            button("contribute.check", "Check", "contribute redact-check"),
            button("contribute.commit", "Contribute", "contribute"),
            button("contribute.confirm.yes", "Confirm", "contribute confirm"),
            button("contribute.cancel", "Cancel", "contribute cancel"),
            button("contribute.bugreport", "Report bug", "contribute bugreport"),
        ]),
        TetraNode::TextField {
            id: String::from("contribute.confirm"),
            placeholder: c.pending.as_ref().map(phrase).unwrap_or_else(|| String::from("Contribute first: the review names the phrase")),
            action: TextAction::ConsoleInput,
            secret: false,
        },
    ])
}

/// Route one `SMessage::ConsoleInput` line: `contribute [<landing dir>]` (the review), `contribute confirm`,
/// `contribute yes <stamp>` (the phrase), `contribute cancel`, `contribute --yes [<landing dir>]` (one-shot),
/// `contribute redact-check [<landing dir>]`, `contribute bugreport [<landing dir>]`, `contribute status`. False = not the
/// contribute face's line.
pub fn on_input(c: &mut Contribute, line: &str) -> bool {
    c.poll();
    let t = line.trim();
    let Some(rest) = t.strip_prefix("contribute ").map(str::trim).or((t == "contribute").then_some("")) else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').map(|(v, a)| (v, a.trim())).unwrap_or((rest, ""));
    match verb {
        "redact-check" => c.check(arg),
        "bugreport" => c.bugreport(arg),
        "status" => c.verdict = c.status(),
        _ if c.busy() => c.verdict = String::from("busy committing — wait for the verdict"),
        "confirm" => c.confirm(None),
        "yes" => c.confirm(Some(arg)),
        "cancel" => c.cancel(),
        "--yes" => c.start(arg),
        _ => c.review(rest),
    }
    true
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use git_core::HashKind;
    use std::fs;

    const BOOT1: &[u8] = include_bytes!("../tests/fixtures/landing-boot1.log");
    const BOOT2: &[u8] = include_bytes!("../tests/fixtures/landing-boot2.log");
    const NAME: &str = "landing-20261027-2340";
    const NOW: i64 = 1_793_000_000;
    /// The identifying values the KAT plants; none may reach a committed byte or a printed line.
    const PLANTED: [&str; 8] = ["a4:5e:60:12:34:56", "Peter's Home Net", "0x1a2b3c4d", "0x12345678", "/home/tester", "8.8.4.4", "hunter2", "tester"];
    const KEY: &str = "sk-ant-api03-TESTSECRETabcdefghijklmnop";

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("installercontribute-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sig() -> Signature {
        Signature::new(b"Base Author", b"base@example.invalid", NOW - 100, 0)
    }

    /// A git repository (git_core's own init, no git binary) with one base commit: README.md + the evidence dir.
    fn repo_at(root: &Path) -> PathBuf {
        let checkout = root.join("checkout");
        fs::create_dir_all(checkout.join("docs/dev/evidence")).unwrap();
        let repo = Repository::init(&checkout, false, HashKind::Sha1, "main").unwrap();
        fs::write(checkout.join("README.md"), "# base\n").unwrap();
        fs::write(checkout.join("docs/dev/evidence/.keep"), "").unwrap();
        repo.add(&[b"README.md".as_slice(), b"docs/dev/evidence/.keep".as_slice()]).unwrap();
        repo.commit_as("base", &sig(), &sig()).unwrap();
        checkout
    }

    fn landing_md() -> String {
        String::from(
            "# Landing — /home/tester/cards/card.img, 2026-10-27 23:40Z\n\n- device: `/home/tester/cards/card.img`\n- image: `c1d61903` (the booted kernel's sha)\n- board: `zenbook`\n- boots: 2\n- verdicts: 4 PASS / 1 FAIL / 2 SKIP (the `::` lines)\n",
        )
    }

    /// The landing: the two fixture logs (boot2 with the identifying lines appended), landing.md, PROMPT.md.
    fn lay(checkout: &Path, extra: &[(&str, &str)]) -> PathBuf {
        let d = checkout.join("docs/dev/evidence").join(NAME);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("boot1.log"), BOOT1).unwrap();
        let mut b2 = BOOT2.to_vec();
        b2.extend_from_slice(
            b"[wifi6] sprom mac=a4:5e:60:12:34:56 src=otp -> ok\n:: wifiauth: s8 auth ssid=Peter's Home Net bssid=a4:5e:60:12:34:56 ch=6 -> PASS ::\n:: install: boot volume serial=0x1a2b3c4d guard ::\n[sdhc] cid mid=0x03 psn=0x12345678 mdt=2024-05\n:: [midden] cmd=\"holocron unlock hunter2\" -> ok ::\nnet dns=8.8.4.4 gw=192.168.1.1\n\0\0\0\0",
        );
        fs::write(d.join("boot2.log"), &b2).unwrap();
        fs::write(d.join("landing.md"), landing_md()).unwrap();
        fs::write(d.join("PROMPT.md"), format!("# Landing prompt\n\nYou are the local session in the UnaOS checkout `{}`.\n", checkout.display())).unwrap();
        for (n, t) in extra {
            fs::write(d.join(n), t).unwrap();
        }
        d
    }

    fn redactor(checkout: &Path) -> Redactor {
        Contribute::probe_at(checkout.to_path_buf(), Some(String::from("/home/tester"))).redactor()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<_> = fs::read_dir(dir).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap())).collect();
        v.sort();
        v
    }

    #[test]
    fn contribute_commits_the_landing_and_touches_nothing_else() {
        let root = scratch("commit");
        let checkout = repo_at(&root);
        let dir = lay(&checkout, &[]);
        fs::write(checkout.join("README.md"), "# base\nlocal edit, unstaged\n").unwrap();
        let repo = Repository::discover(&checkout).unwrap();
        let head0 = repo.head().unwrap();
        let index0 = fs::read(checkout.join(".git/index")).unwrap();
        let files0 = snapshot(&dir);
        assert!(repo.is_dirty().unwrap());

        let lines = contribute_at(&checkout, None, &redactor(&checkout), &none, NOW);
        assert!(lines[0].starts_with(&format!("[installer] contribute landing=20261027-2340 branch=contrib/{NAME} files=5 redacted=")), "{lines:?}");
        assert!(lines[0].ends_with(" ok=1 why=-"), "{lines:?}");
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[2].contains(&format!("push origin contrib/{NAME}")), "{lines:?}");

        // HEAD, the index, the working tree: exactly as they were.
        let repo = Repository::discover(&checkout).unwrap();
        assert_eq!(repo.head().unwrap(), head0);
        assert_eq!(fs::read(checkout.join(".git/index")).unwrap(), index0);
        assert_eq!(fs::read_to_string(checkout.join("README.md")).unwrap(), "# base\nlocal edit, unstaged\n");
        assert_eq!(snapshot(&dir), files0);
        assert!(repo.is_dirty().unwrap());

        // The branch: HEAD as parent, the base tree kept, the landing + CONTRIBUTION.md added.
        let (_, id) = repo.resolve_ref(&format!("refs/heads/contrib/{NAME}")).unwrap().unwrap();
        let (k, d) = repo.read(&id.unwrap()).unwrap();
        assert_eq!(k, Kind::Commit);
        let c = Commit::parse(repo.hash, &d).unwrap();
        assert_eq!(c.parents(), vec![head0.1.unwrap()]);
        let flat = repo.flatten_tree(&c.tree().unwrap()).unwrap();
        let blob = |p: &str| repo.read(&flat.get(p.as_bytes()).unwrap_or_else(|| panic!("{p} missing")).1).unwrap().1;
        assert_eq!(blob("README.md"), b"# base\n");
        let pre = format!("docs/dev/evidence/{NAME}/");
        for f in ["boot1.log", "boot2.log", "landing.md", "PROMPT.md", CONTRIBUTION] {
            blob(&format!("{pre}{f}"));
        }
        assert_eq!(blob(&format!("{pre}boot1.log")), BOOT1, "the fixture log is committed byte-identical");
        let b2 = String::from_utf8_lossy(&blob(&format!("{pre}boot2.log"))).into_owned();
        assert!(b2.contains("mac=xx:xx:xx:xx:xx:xx") && b2.contains("ssid=<ssid> bssid=xx:xx:xx:xx:xx:xx ch=6"), "{b2}");
        assert!(b2.contains("serial=<serial>") && b2.contains("psn=<serial>") && b2.contains("cmd=\"holocron unlock ***\""), "{b2}");
        assert!(b2.contains("dns=<ip> gw=192.168.1.1") && b2.ends_with("\0\0\0\0"), "{b2}");
        let census = String::from_utf8_lossy(&blob(&format!("{pre}{CONTRIBUTION}"))).into_owned();
        assert!(census.contains("- board: `zenbook`") && census.contains("- verdicts: 4/1/2") && census.contains("- mac: 2"), "{census}");
        assert!(String::from_utf8_lossy(&blob(&format!("{pre}PROMPT.md"))).contains("checkout `<checkout>`"));

        // No planted value in any committed byte of the commit, nor in the message, nor in a printed line.
        let mut all: Vec<u8> = d.clone();
        for (_, oid) in flat.values() {
            all.extend_from_slice(&repo.read(oid).unwrap().1);
        }
        let all = String::from_utf8_lossy(&all).into_owned();
        for v in PLANTED.iter().chain([checkout.display().to_string().as_str()].iter()) {
            assert!(!all.contains(v), "{v} reached the commit");
            assert!(!lines[0].contains(v), "{v} printed");
        }

        // Again: the branch exists, nothing moves.
        let again = contribute_at(&checkout, None, &redactor(&checkout), &none, NOW);
        assert!(again[0].contains(&format!("branch=contrib/{NAME} files=0")) && again[0].ends_with("ok=0 why=branch-exists"), "{again:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn contribute_redaction_kat() {
        let r = Redactor::new(Some("/home/tester/src/UnaOS"), Some("/home/tester"));
        // The two fixture logs carry nothing that identifies anyone: byte-identical, no hit.
        for f in [BOOT1, BOOT2] {
            let (out, hits) = r.redact(f);
            assert_eq!(out, f);
            assert!(hits.is_empty(), "{hits:?}");
        }
        use Class::*;
        let kat: &[(&str, &str, &[Class])] = &[
            ("[wifi6] sprom mac=a4:5e:60:12:34:56 src=otp -> ok", "[wifi6] sprom mac=xx:xx:xx:xx:xx:xx src=otp -> ok", &[Mac]),
            (
                ":: wifiauth: s9 assoc ssid=Peter's Home Net bssid=a4-5e-60-12-34-56 cap=0x0431 -> PASS ::",
                ":: wifiauth: s9 assoc ssid=<ssid> bssid=xx:xx:xx:xx:xx:xx cap=0x0431 -> PASS ::",
                &[Ssid, Mac],
            ),
            ("pci 00:1f.3 ssid=1043:1a2b irq=11 pin=1", "pci 00:1f.3 ssid=1043:1a2b irq=11 pin=1", &[]),
            (":: install: boot volume serial=0x1a2b3c4d — guard ARMED ::", ":: install: boot volume serial=<serial> — guard ARMED ::", &[Serial]),
            ("[sdhc] cid mid=0x03 oid=SD psn=0x12345678 mdt=2024-05", "[sdhc] cid mid=0x03 oid=SD psn=<serial> mdt=2024-05", &[Serial]),
            ("[lock] fixture credential src=env user=peter pw_bytes=6", "[lock] fixture credential src=env user=<user> pw_bytes=6", &[User]),
            ("panicked at /home/peter/.cargo/registry/src/a.rs:10:5", "panicked at /home/<user>/.cargo/registry/src/a.rs:10:5", &[User]),
            ("- device: `/Users/peter/cards/card.img`", "- device: `/Users/<user>/cards/card.img`", &[User]),
            ("image /Volumes/PETERSD/card.img", "image /Volumes/<volume>/card.img", &[User]),
            ("checkout /home/tester/src/UnaOS/docs and /home/tester/x", "checkout <checkout>/docs and ~/x", &[User, User]),
            ("help: edit /home/<user>/NOTES.TXT", "help: edit /home/<user>/NOTES.TXT", &[]),
            ("lease ip=192.168.1.20 gw=10.0.0.1 dns=8.8.8.8 lo=127.0.0.1", "lease ip=192.168.1.20 gw=10.0.0.1 dns=<ip> lo=127.0.0.1", &[Ip]),
            ("version 1.2.3 at 00:12:01.500 and 1.2.3.4.5", "version 1.2.3 at 00:12:01.500 and 1.2.3.4.5", &[]),
            (":: [midden] cmd=\"holocron unlock hunter2\" -> ok ::", ":: [midden] cmd=\"holocron unlock ***\" -> ok ::", &[Cmd]),
            (":: [midden] cmd=\"holocron unlock ***\" -> ok ::", ":: [midden] cmd=\"holocron unlock ***\" -> ok ::", &[]),
            (":: [midden] cmd=\"ls /home/<user>\" -> ok ::", ":: [midden] cmd=\"ls /home/<user>\" -> ok ::", &[]),
            ("the provider key shape is sk-ant- (prose)", "the provider key shape is sk-ant- (prose)", &[]),
            ("psk-len=8 wpa=unbuilt", "psk-len=8 wpa=unbuilt", &[]),
            ("held key sk-ant-api03-TESTSECRETabcdefghijklmnop", "held key sk-ant-api03-TESTSECRETabcdefghijklmnop", &[Key]),
            ("wifi join psk=hunter22", "wifi join psk=hunter22", &[SecretField]),
            ("-----BEGIN OPENSSH PRIVATE KEY-----", "-----BEGIN OPENSSH PRIVATE KEY-----", &[Key]),
        ];
        for (input, want, classes) in kat {
            let mut hits = Vec::new();
            let out = r.redact_line(input.as_bytes(), &mut hits);
            assert_eq!(String::from_utf8_lossy(&out), *want, "{input}");
            let mut h = hits.clone();
            h.sort();
            let mut c = classes.to_vec();
            c.sort();
            assert_eq!(h, c, "{input}");
            // A second pass changes nothing (the placeholders are stable).
            let mut again = Vec::new();
            assert_eq!(r.redact_line(&out, &mut again), out, "{input}");
        }
        // Lines and padding survive: hit line numbers are 1-based.
        let (out, hits) = r.redact(b"ok\nmac=a4:5e:60:12:34:56\n\0\0");
        assert_eq!(out, b"ok\nmac=xx:xx:xx:xx:xx:xx\n\0\0");
        assert_eq!(hits, vec![(2, Class::Mac)]);
    }

    #[test]
    fn contribute_refuses_an_unredactable_landing_and_leaves_no_branch() {
        let root = scratch("unredactable");
        let checkout = repo_at(&root);
        lay(&checkout, &[("REPLY.md", &format!("# Reply\n\nYour key {KEY} works.\n"))]);
        let lines = contribute_at(&checkout, None, &redactor(&checkout), &none, NOW);
        assert!(lines[0].ends_with("ok=0 why=unredactable") && lines[0].contains("branch=- files=0"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "[installer] contribute redact file=REPLY.md line=3 class=key"), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("TESTSECRET")), "the key was printed: {lines:?}");
        let repo = Repository::discover(&checkout).unwrap();
        assert!(!matches!(repo.resolve_ref(&format!("refs/heads/contrib/{NAME}")), Ok(Some((_, Some(_))))));

        let check = redact_check(&checkout, None, &redactor(&checkout));
        assert!(check[0].starts_with("[installer] contribute redact-check landing=20261027-2340 files=5 redacted=") && check[0].ends_with(" unredactable=1"), "{check:?}");
        assert!(check.iter().all(|l| PLANTED.iter().all(|v| !l.contains(v)) && !l.contains("TESTSECRET")), "{check:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn contribute_refusals_by_name() {
        let root = scratch("refusals");
        // A checkout that is not a git repository.
        let plain = root.join("plain");
        fs::create_dir_all(plain.join("docs/dev/evidence")).unwrap();
        lay(&plain, &[]);
        let l = contribute_at(&plain, None, &redactor(&plain), &none, NOW);
        assert!(l[0].ends_with("ok=0 why=no-git"), "{l:?}");
        // A repository with no landing.
        let checkout = repo_at(&root);
        let l = contribute_at(&checkout, None, &redactor(&checkout), &none, NOW);
        assert_eq!(l, vec![line("-", "-", 0, 0, false, "no-landing")]);
        assert_eq!(l[0], "[installer] contribute landing=- branch=- files=0 redacted=0 ok=0 why=no-landing");
        let l = contribute_at(&checkout, Some(root.join("nowhere")), &redactor(&checkout), &none, NOW);
        assert!(l[0].ends_with("why=no-landing"), "{l:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn contribute_face_and_input_route() {
        let root = scratch("face");
        let checkout = repo_at(&root);
        lay(&checkout, &[]);
        let mut c = Contribute::probe_at(checkout.clone(), Some(String::from("/home/tester")));
        assert!(c.status().starts_with(&format!("contribute: landing={NAME} repo=1 git_cli=")) && c.status().ends_with("branch=new"), "{}", c.status());
        assert!(!on_input(&mut c, "provider send"));
        assert!(!on_input(&mut c, "landing read"));
        assert!(!on_input(&mut c, "contributed"));
        assert!(on_input(&mut c, "contribute status"));
        assert!(on_input(&mut c, "contribute redact-check"));
        assert!(c.verdict.starts_with("[installer] contribute redact-check landing=20261027-2340 files=4 "), "{}", c.verdict);
        assert!(on_input(&mut c, "contribute"));
        assert!(c.verdict.ends_with("ok=0 why=review-pending") && !c.busy(), "{}", c.verdict); // CONTRIBUTEREVIEW (SR445)
        assert!(on_input(&mut c, "contribute confirm"));
        for _ in 0..2000 {
            if !c.busy() {
                break;
            }
            c.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!c.busy(), "the commit never answered");
        assert!(c.verdict.ends_with("ok=1 why=-"), "{}", c.verdict);
        assert_eq!(c.next.len(), 3);
        assert!(c.status().ends_with("branch=exists"));
        let TetraNode::VStack(kids) = face_of(&c) else { panic!("the face is a VStack") };
        let TetraNode::Console(con) = &kids[0] else { panic!("the console first") };
        assert!(con.lines.iter().any(|l| l.text.contains("push origin")));
        let _ = fs::remove_dir_all(&root);
    }

    // ── CONTRIBUTEREVIEW (SR445) ────────────────────────────────────────────────────────────────

    fn settle(c: &mut Contribute) {
        for _ in 0..2000 {
            if !c.busy() {
                break;
            }
            c.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!c.busy(), "the commit never answered");
    }

    fn has_branch(checkout: &Path, branch: &str) -> bool {
        let repo = Repository::discover(checkout).unwrap();
        matches!(repo.resolve_ref(&format!("refs/heads/{branch}")), Ok(Some((_, Some(_)))))
    }

    /// The first `contribute` reviews (redacted, held in memory, no branch, no value shown); the confirm commits exactly
    /// the reviewed plan; a review of a committed landing refuses before it shows anything.
    #[test]
    fn contribute_plan_then_confirm_commits() {
        let root = scratch("review-confirm");
        let checkout = repo_at(&root);
        let dir = lay(&checkout, &[]);
        let files0 = snapshot(&dir);
        let mut c = Contribute::probe_at(checkout.clone(), Some(String::from("/home/tester")));
        assert!(on_input(&mut c, "contribute"));
        assert!(!c.busy());
        let redacted = c.pending.as_ref().expect("the plan is held").redacted();
        assert_eq!(c.verdict, line("20261027-2340", &format!("contrib/{NAME}"), 5, redacted, false, "review-pending"));
        assert!(c.review.iter().any(|l| l.starts_with("boot2.log: ") && l.ends_with(" redacted")), "{:?}", c.review);
        assert!(c.review.iter().any(|l| l.starts_with("  boot2.log:") && l.contains("ssid=<ssid>")), "{:?}", c.review);
        assert!(c.review.iter().any(|l| l.contains("`contribute yes 20261027-2340`")), "{:?}", c.review);
        let checkout_s = checkout.display().to_string();
        for v in PLANTED.iter().chain([checkout_s.as_str()].iter()) {
            assert!(c.review.iter().chain([&c.verdict]).all(|l| !l.contains(v)), "{v} shown in the review");
        }
        let TetraNode::VStack(kids) = face_of(&c) else { panic!("the face is a VStack") };
        let TetraNode::TextField { placeholder, .. } = &kids[2] else { panic!("the confirm field third") };
        assert_eq!(placeholder.as_str(), "contribute yes 20261027-2340");
        let TetraNode::Console(con) = &kids[0] else { panic!("the console first") };
        assert!(con.lines.iter().any(|l| l.text.contains("ssid=<ssid>")));
        assert!(!has_branch(&checkout, &format!("contrib/{NAME}")), "the review committed");

        assert!(on_input(&mut c, "contribute confirm"));
        settle(&mut c);
        assert!(c.verdict.ends_with("ok=1 why=-") && c.verdict.contains(&format!("branch=contrib/{NAME} files=5 redacted={redacted} ")), "{}", c.verdict);
        assert_eq!(c.next.len(), 3);
        assert!(c.pending.is_none() && c.review.is_empty());
        assert!(has_branch(&checkout, &format!("contrib/{NAME}")));
        assert_eq!(snapshot(&dir), files0, "the landing on disk is untouched");

        assert!(on_input(&mut c, "contribute"));
        assert!(c.verdict.ends_with("ok=0 why=branch-exists") && c.pending.is_none() && c.review.is_empty(), "{}", c.verdict);
        let _ = fs::remove_dir_all(&root);
    }

    /// A review never commits: cancel drops it, a wrong stamp runs nothing, a confirm with nothing pending runs nothing.
    #[test]
    fn contribute_plan_without_confirm_leaves_no_branch() {
        let root = scratch("review-cancel");
        let checkout = repo_at(&root);
        lay(&checkout, &[]);
        let mut c = Contribute::probe_at(checkout.clone(), Some(String::from("/home/tester")));
        assert!(on_input(&mut c, "contribute confirm"));
        assert!(!c.busy() && c.verdict.starts_with("nothing to confirm"), "{}", c.verdict);
        assert!(on_input(&mut c, "contribute"));
        assert!(c.verdict.ends_with("why=review-pending"), "{}", c.verdict);
        assert!(on_input(&mut c, "contribute yes 20261027-0000"));
        assert!(!c.busy() && c.pending.is_some(), "a wrong stamp confirmed");
        assert_eq!(c.verdict, "type `contribute yes 20261027-2340`");
        assert!(on_input(&mut c, "contribute cancel"));
        assert!(!c.busy() && c.pending.is_none() && c.review.is_empty());
        assert!(on_input(&mut c, "contribute confirm"));
        assert!(!c.busy() && c.verdict.starts_with("nothing to confirm"), "{}", c.verdict);
        assert!(!has_branch(&checkout, &format!("contrib/{NAME}")), "a review without a yes committed");
        let _ = fs::remove_dir_all(&root);
    }

    /// `contribute --yes` is the one-shot path (no review step); the typed phrase after a review commits too.
    #[test]
    fn contribute_yes_is_one_shot() {
        let root = scratch("yes");
        let checkout = repo_at(&root);
        lay(&checkout, &[]);
        let mut c = Contribute::probe_at(checkout.clone(), Some(String::from("/home/tester")));
        assert!(on_input(&mut c, "contribute --yes"));
        settle(&mut c);
        assert!(c.verdict.ends_with("ok=1 why=-") && c.verdict.contains(&format!("branch=contrib/{NAME} files=5 ")), "{}", c.verdict);
        assert!(c.review.is_empty() && c.pending.is_none());
        assert_eq!(c.next.len(), 3);
        assert!(has_branch(&checkout, &format!("contrib/{NAME}")));

        let second = checkout.join("docs/dev/evidence/landing-20261028-0100");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("landing.md"), landing_md()).unwrap();
        assert!(on_input(&mut c, &format!("contribute {}", second.display())));
        assert!(c.verdict.contains("branch=contrib/landing-20261028-0100 files=2 ") && c.verdict.ends_with("ok=0 why=review-pending"), "{}", c.verdict);
        assert!(!has_branch(&checkout, "contrib/landing-20261028-0100"));
        assert!(on_input(&mut c, "contribute yes 20261028-0100"));
        settle(&mut c);
        assert!(c.verdict.ends_with("ok=1 why=-") && c.verdict.contains("branch=contrib/landing-20261028-0100 files=2 "), "{}", c.verdict);
        assert!(has_branch(&checkout, "contrib/landing-20261028-0100"));
        let _ = fs::remove_dir_all(&root);
    }
    // ── INSTALLERBUGREPORT (SR433) ──────────────────────────────────────────────────────────────

    const FAIL3: &str = ":: WIFIJOIN: bssid=a4:5e:60:12:34:56 cfg=/home/tester/wifi.conf -> FAIL ::\n";

    #[test]
    fn bugreport_writes_a_redacted_report() {
        let root = scratch("bugreport");
        let checkout = repo_at(&root);
        let dir = lay(&checkout, &[("boot3.log", FAIL3)]);
        let repo = Repository::discover(&checkout).unwrap();
        let head0 = repo.head().unwrap();
        let files0 = snapshot(&dir);

        let lines = bugreport_at(&checkout, None, &redactor(&checkout), &none, "<owner>/<repo>");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("[installer] contribute bugreport landing=20261027-2340 fails=2 redacted=") && lines[0].ends_with(" ok=1 why=-"), "{lines:?}");
        assert!(
            lines[1].starts_with("[installer] contribute next: (cd ")
                && lines[1].ends_with(" && gh issue create --repo <owner>/<repo> --title 'zenbook c1d61903: PUMPWEDGE FAIL' --body-file BUGREPORT.md)"),
            "{lines:?}"
        );

        let body = fs::read_to_string(dir.join(BUGREPORT)).unwrap();
        assert!(body.starts_with("# zenbook c1d61903: PUMPWEDGE FAIL\n"), "{body}");
        assert!(body.contains("| image | `c1d61903` |") && body.contains("| verdicts | 4/1/2 (PASS/FAIL/SKIP) |"), "{body}");
        assert!(body.contains("| UnaOS commit | `") && !body.contains("| UnaOS commit | `-` |"), "{body}");
        assert!(body.contains("## Failing verdicts (2)"), "{body}");
        assert!(body.contains("boot2.log:7: [00:12:01.500] PUMPWEDGE: age_ms=2171"), "{body}");
        assert!(body.contains("boot3.log:1: :: WIFIJOIN: bssid=xx:xx:xx:xx:xx:xx cfg=~/wifi.conf -> FAIL ::"), "{body}");
        assert!(body.find("boot2.log:7:").unwrap() < body.find("boot3.log:1:").unwrap(), "boot order");
        for f in ["boot1.log", "boot2.log", "boot3.log", "landing.md", "PROMPT.md"] {
            assert!(body.contains(&format!("- `{f}`: ")), "{f} not listed: {body}");
        }
        assert!(body.contains("- mac: 3"), "{body}");
        for v in PLANTED.iter().chain([checkout.display().to_string().as_str()].iter()) {
            assert!(!body.contains(v), "{v} reached BUGREPORT.md");
            assert!(!lines[0].contains(v), "{v} printed");
        }
        // The `next:` line names the landing dir to cd into (the person's own path, as contribute's next steps do), and
        // no planted value.
        assert!(PLANTED.iter().all(|v| !lines[1].contains(v)), "{lines:?}");

        // The landing's originals byte-identical, no branch made, HEAD unchanged.
        let mut after = snapshot(&dir);
        after.retain(|(n, _)| n != BUGREPORT);
        assert_eq!(after, files0);
        let repo = Repository::discover(&checkout).unwrap();
        assert_eq!(repo.head().unwrap(), head0);
        assert!(!matches!(repo.resolve_ref(&format!("refs/heads/contrib/{NAME}")), Ok(Some((_, Some(_))))));

        // A re-run never reads its own report: the same plan, the same report.
        let p = plan(&dir, &redactor(&checkout)).unwrap();
        assert!(p.files.iter().all(|f| f.name != BUGREPORT));
        let again = bugreport_at(&checkout, Some(dir.clone()), &redactor(&checkout), &none, "<owner>/<repo>");
        assert_eq!(again, lines);
        assert_eq!(fs::read_to_string(dir.join(BUGREPORT)).unwrap(), body);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bugreport_refuses_unredactable_and_writes_nothing() {
        let root = scratch("bugreport-unredactable");
        let checkout = repo_at(&root);
        let dir = lay(&checkout, &[("REPLY.md", &format!("# Reply\n\nYour key {KEY} works.\n"))]);
        let lines = bugreport_at(&checkout, None, &redactor(&checkout), &none, "<owner>/<repo>");
        assert!(lines[0].starts_with("[installer] contribute bugreport landing=20261027-2340 fails=0 ") && lines[0].ends_with("ok=0 why=unredactable"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "[installer] contribute redact file=REPLY.md line=3 class=key"), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("TESTSECRET") && !l.contains("gh issue")), "{lines:?}");
        assert!(!dir.join(BUGREPORT).exists(), "an unredactable landing wrote a report");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bugreport_no_landing() {
        let root = scratch("bugreport-none");
        let checkout = repo_at(&root);
        let l = bugreport_at(&checkout, None, &redactor(&checkout), &none, "<owner>/<repo>");
        assert_eq!(l, vec![String::from("[installer] contribute bugreport landing=- fails=0 redacted=0 ok=0 why=no-landing")]);
        let l = bugreport_at(&checkout, Some(root.join("nowhere")), &redactor(&checkout), &none, "<owner>/<repo>");
        assert!(l[0].ends_with("why=no-landing"), "{l:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bugreport_repo_slug() {
        for (url, want) in [
            ("https://github.com/unaos-dev/UnaOS.git", Some("unaos-dev/UnaOS")),
            ("https://github.com/unaos-dev/UnaOS", Some("unaos-dev/UnaOS")),
            ("https://ghtoken123@github.com/unaos-dev/UnaOS.git/", Some("unaos-dev/UnaOS")),
            ("ssh://git@github.com:22/unaos-dev/UnaOS.git", Some("unaos-dev/UnaOS")),
            ("git@github.com:unaos-dev/UnaOS.git", Some("unaos-dev/UnaOS")),
            ("https://gitlab.com/unaos-dev/UnaOS.git", None),
            ("/srv/git/UnaOS", None),
            ("file:///srv/git/UnaOS", None),
            ("https://github.com/unaos-dev", None),
        ] {
            assert_eq!(slug_of(url).as_deref(), want, "{url}");
        }
        let root = scratch("bugreport-slug");
        let checkout = repo_at(&root);
        assert_eq!(repo_slug(&checkout), "<owner>/<repo>");
        assert_eq!(repo_slug(&root.join("nowhere")), "<owner>/<repo>");
        let cfg = checkout.join(".git/config");
        let mut c = fs::read_to_string(&cfg).unwrap();
        c.push_str("\n[remote \"origin\"]\n\turl = https://ghtoken123@github.com/unaos-dev/UnaOS.git\n");
        fs::write(&cfg, c).unwrap();
        assert_eq!(repo_slug(&checkout), "unaos-dev/UnaOS");
        let dir = lay(&checkout, &[]);
        let lines = bugreport_at(&checkout, None, &redactor(&checkout), &none, &repo_slug(&checkout));
        assert!(lines[1].contains("gh issue create --repo unaos-dev/UnaOS --title 'zenbook c1d61903: PUMPWEDGE FAIL' "), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("ghtoken123")), "{lines:?}");
        assert!(!fs::read_to_string(dir.join(BUGREPORT)).unwrap().contains("ghtoken123"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bugreport_input_route_and_title_rules() {
        let root = scratch("bugreport-face");
        let checkout = repo_at(&root);
        lay(&checkout, &[]);
        let mut c = Contribute::probe_at(checkout.clone(), Some(String::from("/home/tester")));
        assert!(on_input(&mut c, "contribute bugreport"));
        assert!(c.verdict.starts_with("[installer] contribute bugreport landing=20261027-2340 fails=1 ") && c.verdict.ends_with("ok=1 why=-"), "{}", c.verdict);
        assert_eq!(c.next.len(), 1);
        assert!(c.next[0].contains("--body-file BUGREPORT.md"), "{:?}", c.next);
        assert!(!c.busy());
        let TetraNode::VStack(kids) = face_of(&c) else { panic!("the face is a VStack") };
        let TetraNode::HStack(buttons) = &kids[1] else { panic!("the buttons second") };
        assert!(buttons.iter().any(|b| matches!(b, TetraNode::Button { id, .. } if id == "contribute.bugreport")));
        let TetraNode::Console(con) = &kids[0] else { panic!("the console first") };
        assert!(con.lines.iter().any(|l| l.text.contains("gh issue create")));

        assert_eq!(verdict_name("[00:12:01.500] PUMPWEDGE: age_ms=2171 -> FAIL :: pump=9@cpu7"), "PUMPWEDGE");
        assert_eq!(verdict_name(":: WIFIJOIN: tx -> FAIL ::"), "WIFIJOIN");
        assert_eq!(verdict_name(":: odd line -> FAIL ::"), "odd line -> FAIL ::");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(boot_order("boot10.log").0, 10);
        assert!(boot_order("boot2.log") < boot_order("boot10.log"));
        assert_eq!(landing_field("- image: `c1d61903` (the booted kernel's sha)\n", "image").as_deref(), Some("c1d61903"));
        assert_eq!(landing_field("- board: `zenbook`\n", "board").as_deref(), Some("zenbook"));
        let _ = fs::remove_dir_all(&root);
    }
}
