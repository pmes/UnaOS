#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFDIAG M2 + M3 (rmbp-ledger B324, R82): `APPS/DIAG.ELF` — the smart installer's diagnosis program. Peter:
// "the smart installer will need to use vein in order to auto-diagnose so like with this GPU issue when
// self-hosting you can make changes and reboot until the driver works."
//
// ONE RUN (no argv yet: a bare `diag` runs it in the foreground; app note flags 0):
//   1. `/var/log/boot.last` → n; `/var/log/boot.<n>.witness` (the kernel's bootwit wrote it) → the FAIL lines.
//   2. Each FAIL line's owner — `TAG<TAB>path<TAB>line` — streamed out of `/system/witness-owners.txt` (else
//      `/boot/system/witness-owners.txt`; generated at build by scripts/witness-owners.py).
//   3. The owner's section (±40 lines around the site) from the selfhost tree `src extract` materialised
//      (`/boot/SRC` beside a UnaFS root, `/SRC` on a FAT root). No tree ⇒ the prompt says so; apply is
//      skipped and the program names the verb.
//   4. ONE request through Vein (vein_core + vein_ring3, the LUMENAPP library shape; Principia's `vein`
//      namespace decides the provider) asking for ONE unified diff. Offline (no key) the Echo provider
//      answers: `diag_core::fixture` holds the canned diff for the canned FAIL line.
//   5. The diff is applied with diag_core's applier: EVERY hunk of EVERY file is resolved first (old side
//      exact, ±3 lines of drift); one mismatch refuses the whole answer. Then each file streams to
//      `<file>~dgn`, is copied back and the temp is unlinked.
//   6. `/var/log/diag.<n>.md` records the prompt, the answer and the outcome; the NEXT boot's kernel appends
//      that boot's verdict on the same tags (`## next boot <m>`).
// The rebuild and reboot are SH-5 (no native toolchain): the program prints the bench command. `diag --loop`
// is OWED until SH-5 (and until a program has an argv).
//
// I/O is SYS_PATH_READ (59) / SYS_PATH_WRITE (60), fulfilled by the kernel over the VFS (UNAOS_SELFDIAG=1).
//
// WIRE. `:: SELFDIAG: start provider=<claude|echo> boot=<n|none> tree=<path|none> owners=<path|none> ::`, per
// FAIL line `[selfdiag] fail tag=<T> owner=<path>:<line>|none`, `[selfdiag] ask prompt=<b> answer=<b>
// outcome=<o>`, then `:: SELFDIAG: fails=<n> asked=<n> patched=<n> refused=<n> -> PASS ::` (SKIP: no boot log;
// FAIL: an I/O or provider failure, with `stage=`).

use diag_core::apply::{self, ReadAt, Span};
use diag_core::{diff, fixture, owners, prompt, record, witness, Out};
use una_abi::{PATH_IO_HDR_LEN, PATH_IO_MAX, PATH_IO_PATH_MAX, PATH_W_MKDIRS, PATH_W_TRUNC, PATH_W_UNLINK, SYS_EXIT, SYS_PATH_READ, SYS_PATH_WRITE};
use vein_core::claude::{Event, Msg, Params};
use vein_core::prefs::{KeyState, Plan};
use vein_ring3::sys::{sys, write};

const MAX_FAILS: usize = 16;
const MAX_ASK: usize = 4;
const SEC_CAP: usize = 6 * 1024;
const RADIUS: u32 = 40;
const OWNERS: [&str; 2] = ["/system/witness-owners.txt", "/boot/system/witness-owners.txt"];
const TREES: [&str; 2] = ["/boot/SRC", "/SRC"];

fn exit(code: u64) -> ! {
    sys(SYS_EXIT, code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

struct Mem {
    wit: [u8; 80 * 1024],
    own: [u8; PATH_IO_MAX],
    sec: [[u8; SEC_CAP]; MAX_ASK],
    prompt: [u8; 36 * 1024],
    answer: [u8; 32 * 1024],
    rec: [u8; 96 * 1024],
    req: [u8; PATH_IO_HDR_LEN + PATH_IO_PATH_MAX + PATH_IO_MAX],
    outb: [u8; PATH_IO_MAX],
    scratch: [u8; 8 * 1024],
    opath: [[u8; diff::PATH_MAX]; MAX_FAILS],
    opath_n: [usize; MAX_FAILS],
    oline: [u32; MAX_FAILS],
    key: [u8; 320],
}

static mut MEM: Mem = Mem {
    wit: [0; 80 * 1024],
    own: [0; PATH_IO_MAX],
    sec: [[0; SEC_CAP]; MAX_ASK],
    prompt: [0; 36 * 1024],
    answer: [0; 32 * 1024],
    rec: [0; 96 * 1024],
    req: [0; PATH_IO_HDR_LEN + PATH_IO_PATH_MAX + PATH_IO_MAX],
    outb: [0; PATH_IO_MAX],
    scratch: [0; 8 * 1024],
    opath: [[0; diff::PATH_MAX]; MAX_FAILS],
    opath_n: [0; MAX_FAILS],
    oline: [0; MAX_FAILS],
    key: [0; 320],
};
static mut BUFS: vein_ring3::Buffers = vein_ring3::Buffers::new();

// ---- path I/O (SYS_PATH_READ / SYS_PATH_WRITE) ---------------------------------------------------------

fn hdr(b: &mut [u8], path: &str, flags: u16, off: u64) -> usize {
    let p = path.as_bytes();
    b[0..2].copy_from_slice(&(p.len() as u16).to_le_bytes());
    b[2..4].copy_from_slice(&flags.to_le_bytes());
    b[4..8].copy_from_slice(&[0; 4]);
    b[8..16].copy_from_slice(&off.to_le_bytes());
    b[PATH_IO_HDR_LEN..PATH_IO_HDR_LEN + p.len()].copy_from_slice(p);
    PATH_IO_HDR_LEN + p.len()
}

/// Read up to `buf.len()` (≤ PATH_IO_MAX per call) bytes of `path` from `off`.
fn pread(path: &str, off: u64, buf: &mut [u8]) -> i64 {
    if path.len() > PATH_IO_PATH_MAX {
        return una_abi::EINVAL;
    }
    let mut rq = [0u8; PATH_IO_HDR_LEN + PATH_IO_PATH_MAX];
    let n = hdr(&mut rq, path, 0, off);
    let cap = buf.len().min(PATH_IO_MAX);
    sys(SYS_PATH_READ, rq.as_ptr() as u64, n as u64, buf.as_mut_ptr() as u64, cap as u64)
}

/// Read the whole file into `buf` (cut at its end). Returns the bytes, or -errno.
fn pread_all(path: &str, buf: &mut [u8]) -> Result<usize, i64> {
    let mut n = 0usize;
    while n < buf.len() {
        let k = pread(path, n as u64, &mut buf[n..]);
        if k < 0 {
            return Err(k);
        }
        if k == 0 {
            break;
        }
        n += k as usize;
    }
    Ok(n)
}

/// One SYS_PATH_WRITE (data ≤ PATH_IO_MAX) through `req`.
fn pwrite1(req: &mut [u8], path: &str, flags: u16, off: u64, data: &[u8]) -> i64 {
    if path.len() > PATH_IO_PATH_MAX || data.len() > PATH_IO_MAX {
        return una_abi::EINVAL;
    }
    let n = hdr(req, path, flags, off);
    req[n..n + data.len()].copy_from_slice(data);
    sys(SYS_PATH_WRITE, req.as_ptr() as u64, (n + data.len()) as u64, 0, 0)
}

/// Create-or-truncate `path` (parents made) and write `data`, chunked.
fn pwrite_file(req: &mut [u8], path: &str, data: &[u8]) -> Result<(), i64> {
    let first = data.len().min(PATH_IO_MAX);
    let r = pwrite1(req, path, PATH_W_TRUNC | PATH_W_MKDIRS, 0, &data[..first]);
    if r < 0 {
        return Err(r);
    }
    let mut off = first;
    while off < data.len() {
        let k = (data.len() - off).min(PATH_IO_MAX);
        let r = pwrite1(req, path, 0, off as u64, &data[off..off + k]);
        if r <= 0 {
            return Err(if r < 0 { r } else { una_abi::EIO });
        }
        off += r as usize;
    }
    Ok(())
}

/// A file read through SYS_PATH_READ as diag_core's `ReadAt`.
struct PathSrc<'a> {
    path: &'a str,
}

impl ReadAt for PathSrc<'_> {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<usize, i64> {
        let k = pread(self.path, off, buf);
        if k < 0 { Err(k) } else { Ok(k as usize) }
    }
}

/// `root` + `/` + `rel` into `b`.
fn join<'b>(b: &'b mut [u8; 256], root: &str, rel: &str, suffix: &str) -> Option<&'b str> {
    let mut o = Out::new(&mut b[..]);
    o.s(root).s("/").s(rel).s(suffix);
    let n = o.done()?;
    if n > PATH_IO_PATH_MAX {
        return None;
    }
    core::str::from_utf8(&b[..n]).ok()
}

// ---- the wire -------------------------------------------------------------------------------------------

fn say(o: &Out<'_>) {
    write(o.bytes());
    write(b"\n");
}

fn verdict(fails: usize, asked: usize, patched: usize, refused: usize, word: &str, extra: &str) -> ! {
    let mut b = [0u8; 256];
    let mut o = Out::new(&mut b);
    o.s(":: SELFDIAG: fails=").dec(fails as u64).s(" asked=").dec(asked as u64).s(" patched=").dec(patched as u64).s(" refused=").dec(refused as u64);
    if !extra.is_empty() {
        o.s(" ").s(extra);
    }
    o.s(" -> ").s(word).s(" ::");
    say(&o);
    exit(if word == "FAIL" { 1 } else { 0 })
}

fn note(parts: &[&str]) {
    let mut b = [0u8; 512];
    let mut o = Out::new(&mut b);
    o.s("[selfdiag] ");
    for p in parts {
        o.s(p);
    }
    say(&o);
}

/// Stream the owners table and fill each FAIL tag's first owner.
struct Owners<'m> {
    own: &'m mut [u8; PATH_IO_MAX],
    opath: &'m mut [[u8; diff::PATH_MAX]; MAX_FAILS],
    opath_n: &'m mut [usize; MAX_FAILS],
    oline: &'m mut [u32; MAX_FAILS],
}

fn find_owners(m: Owners<'_>, path: &str, tags: &[&str]) -> Result<(), i64> {
    let mut off = 0u64;
    let mut left = tags.len();
    loop {
        let k = pread(path, off, &mut m.own[..]);
        if k < 0 {
            return Err(k);
        }
        if k == 0 {
            return Ok(());
        }
        let chunk = &m.own[..k as usize];
        let end = match chunk.iter().rposition(|&c| c == b'\n') {
            Some(i) => i + 1,
            None if (k as usize) < m.own.len() => chunk.len(),
            None => return Ok(()),
        };
        let text = core::str::from_utf8(&chunk[..end]).unwrap_or("");
        for (i, t) in tags.iter().enumerate() {
            if m.opath_n[i] != 0 {
                continue;
            }
            if let Some((p, l)) = owners::lookup(text, t) {
                let n = p.len().min(diff::PATH_MAX);
                m.opath[i][..n].copy_from_slice(&p.as_bytes()[..n]);
                m.opath_n[i] = n;
                m.oline[i] = l;
                left -= 1;
            }
        }
        if left == 0 {
            return Ok(());
        }
        off += end as u64;
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let m = unsafe { &mut *core::ptr::addr_of_mut!(MEM) };

    // The provider: Principia's `vein` namespace and the key file, the LUMENAPP rule.
    let cfg = vein_ring3::prefs::Config::read();
    let (key, key_n) = vein_ring3::key::read(cfg.key_file(), &mut m.key);
    let plan = cfg.plan(key);
    let provider = match plan {
        Plan::Claude { .. } => "claude",
        Plan::Echo(_) => "echo",
    };

    // 1. The boot log.
    let mut lb = [0u8; 32];
    let boot = match pread("/var/log/boot.last", 0, &mut lb) {
        k if k > 0 => {
            let mut e = k as usize;
            while e > 0 && (lb[e - 1] == b'\n' || lb[e - 1] == b'\r' || lb[e - 1] == b' ') {
                e -= 1;
            }
            diag_core::parse_dec(&lb[..e])
        }
        _ => None,
    };
    let tree = TREES.iter().copied().find(|t| {
        let mut one = [0u8; 1];
        pread(t, 0, &mut one) == una_abi::EISDIR
    });
    let owners_path = OWNERS.iter().copied().find(|p| {
        let mut one = [0u8; 1];
        pread(p, 0, &mut one) >= 0
    });
    {
        let mut b = [0u8; 256];
        let mut o = Out::new(&mut b);
        o.s(":: SELFDIAG: start provider=").s(provider).s(" boot=");
        match boot {
            Some(n) => {
                o.dec(n);
            }
            None => {
                o.s("none");
            }
        }
        o.s(" tree=").s(tree.unwrap_or("none")).s(" owners=").s(owners_path.unwrap_or("none")).s(" ::");
        say(&o);
    }
    let Some(boot) = boot else {
        note(&["no boot log on this volume (/var/log/boot.last): the kernel writes it on a UnaFS root under UNAOS_SELFDIAG=1"]);
        verdict(0, 0, 0, 0, "SKIP", "boot=none")
    };
    let mut pb = [0u8; 256];
    let wpath = {
        let mut o = Out::new(&mut pb[..]);
        o.s("/var/log/boot.").dec(boot).s(".witness");
        let n = o.len();
        core::str::from_utf8(&pb[..n]).unwrap_or("")
    };
    let wn = match pread_all(wpath, &mut m.wit) {
        Ok(n) => n,
        Err(e) => {
            note(&["cannot read ", wpath]);
            let mut b = [0u8; 48];
            let mut o = Out::new(&mut b);
            o.s("stage=boot-log err=-").dec(e.unsigned_abs());
            let n = o.len();
            verdict(0, 0, 0, 0, "FAIL", core::str::from_utf8(&b[..n]).unwrap_or(""))
        }
    };
    let wit = match core::str::from_utf8(&m.wit[..wn]) {
        Ok(s) => s,
        Err(e) => unsafe { core::str::from_utf8_unchecked(&m.wit[..e.valid_up_to()]) },
    };

    // 2. The FAIL lines and their owners.
    let mut fl: [&str; MAX_FAILS] = [""; MAX_FAILS];
    let mut tags: [&str; MAX_FAILS] = [""; MAX_FAILS];
    let mut nf = 0usize;
    let mut total = 0usize;
    for l in witness::fails(wit) {
        total += 1;
        if nf < MAX_FAILS {
            fl[nf] = l;
            tags[nf] = witness::tag(l).unwrap_or("?");
            nf += 1;
        }
    }
    if total == 0 {
        note(&["boot ", "has no FAIL line: nothing to diagnose"]);
        verdict(0, 0, 0, 0, "PASS", "")
    }
    if let Some(op) = owners_path {
        if let Err(_e) = find_owners(Owners { own: &mut m.own, opath: &mut m.opath, opath_n: &mut m.opath_n, oline: &mut m.oline }, op, &tags[..nf]) {
            note(&["owners table unreadable: ", op]);
        }
    }
    for i in 0..nf {
        let mut b = [0u8; 400];
        let mut o = Out::new(&mut b);
        o.s("[selfdiag] fail tag=").s(tags[i]).s(" owner=");
        if m.opath_n[i] > 0 {
            o.put(&m.opath[i][..m.opath_n[i]]).s(":").dec(m.oline[i] as u64);
        } else {
            o.s("none");
        }
        say(&o);
    }

    // 3. The prompt: the first MAX_ASK FAIL lines, each with its owner's section from the tree.
    let asked = nf.min(MAX_ASK);
    let mut secn = [(0usize, 0u32); MAX_ASK];
    for i in 0..asked {
        let (Some(t), true) = (tree, m.opath_n[i] > 0) else { continue };
        let rel = core::str::from_utf8(&m.opath[i][..m.opath_n[i]]).unwrap_or("");
        let mut jb = [0u8; 256];
        let Some(fp) = join(&mut jb, t, rel, "") else { continue };
        let first = m.oline[i].saturating_sub(RADIUS).max(1);
        if let Ok((n, _)) = apply::read_lines(&mut PathSrc { path: fp }, first, 2 * RADIUS + 1, &mut m.sec[i], &mut m.scratch) {
            secn[i] = (n, first);
        }
    }
    let plen = {
        let mut o = Out::new(&mut m.prompt);
        prompt::begin(&mut o, boot, total, tree);
        let room = (36 * 1024 - 2048) / asked.max(1);
        for i in 0..asked {
            let owner = if m.opath_n[i] > 0 { core::str::from_utf8(&m.opath[i][..m.opath_n[i]]).ok().map(|p| (p, m.oline[i])) } else { None };
            let section = if secn[i].0 > 0 { core::str::from_utf8(&m.sec[i][..secn[i].0]).ok().map(|t| (t, secn[i].1)) } else { None };
            prompt::item(&mut o, i, &prompt::Item { line: fl[i], tag: tags[i], owner, section }, room);
        }
        prompt::end(&mut o);
        o.len()
    };
    let ptext = core::str::from_utf8(&m.prompt[..plen]).unwrap_or("");

    // 4. Ask.
    let mut an = 0usize;
    let mut failed: Option<(&'static str, i64)> = None;
    match plan {
        Plan::Echo(_) => {
            let a = fl[..asked].iter().find_map(|l| fixture::answer(l)).unwrap_or("NO-PATCH: the echo provider (offline, no key) has a canned answer only for the SELFDIAG fixture line.\n");
            let n = a.len().min(m.answer.len());
            m.answer[..n].copy_from_slice(&a.as_bytes()[..n]);
            an = n;
        }
        Plan::Claude { send_key } => {
            let ep = cfg.endpoint().unwrap_or(vein_core::prefs::DEFAULT_ENDPOINT);
            let p = Params::new(cfg.model(), vein_core::claude::DEFAULT_MAX_TOKENS, prompt::SYSTEM);
            let keystr = if send_key && key == KeyState::UnaFs { vein_core::prefs::key_from_file(&m.key[..key_n]) } else { None };
            let bufs = unsafe { &mut *core::ptr::addr_of_mut!(BUFS) };
            let msgs = core::iter::once(Msg { role: vein_core::Role::User, text: ptext });
            let ans = &mut m.answer;
            let mut on = |e: Event<'_>| {
                if let Event::Text(t) = e {
                    let k = t.len().min(ans.len() - an);
                    ans[an..an + k].copy_from_slice(&t.as_bytes()[..k]);
                    an += k;
                }
            };
            match vein_ring3::prepare(&ep, &p, msgs, keystr, bufs).and_then(|req| vein_ring3::send(&ep, req, bufs, &mut on)) {
                Ok(o) if o.status == 200 => {}
                Ok(o) => failed = Some(("http-status", o.status as i64)),
                Err(s) => failed = Some((s.name(), s.code())),
            }
        }
    }
    let answer = match core::str::from_utf8(&m.answer[..an]) {
        Ok(s) => s,
        Err(e) => unsafe { core::str::from_utf8_unchecked(&m.answer[..e.valid_up_to()]) },
    };

    // 5. Apply.
    let mut patched = 0usize;
    let mut refused = 0usize;
    let outcome: &str;
    let mut why = [0u8; 64];
    let mut why_n = 0usize;
    if failed.is_some() {
        outcome = "provider-failed";
    } else if let Some(dt) = diff::extract(answer) {
        match diff::parse(dt) {
            Err(r) => {
                refused = 1;
                outcome = "refused";
                why_n = Out::new(&mut why).s(r.as_str()).len();
            }
            Ok(_) if tree.is_none() => outcome = "no-tree",
            Ok(patch) => match apply_patch(&mut m.req, &mut m.outb, &mut m.scratch, tree.unwrap_or(""), &patch) {
                Ok(n) => {
                    patched = n;
                    outcome = "patched";
                }
                Err((r, path_i)) => {
                    refused = 1;
                    outcome = "refused";
                    let mut o = Out::new(&mut why);
                    o.s(r.as_str()).s(" file=").dec(path_i as u64);
                    why_n = o.len();
                }
            },
        }
    } else {
        outcome = if answer.trim_start().starts_with("NO-PATCH") { "no-patch" } else { "no-diff" };
    }
    {
        let mut b = [0u8; 256];
        let mut o = Out::new(&mut b);
        o.s("[selfdiag] ask provider=").s(provider).s(" prompt=").dec(plen as u64).s(" answer=").dec(an as u64).s(" outcome=").s(outcome);
        if why_n > 0 {
            o.s(" why=").put(&why[..why_n]);
        }
        if let Some((s, c)) = failed {
            o.s(" stage=").s(s).s(" code=");
            if c < 0 {
                o.s("-");
            }
            o.dec(c.unsigned_abs());
        }
        say(&o);
    }
    if outcome == "no-tree" {
        note(&["no selfhost tree on this machine: run `src extract`, then diag again"]);
    }

    // 6. The record.
    let rn = {
        let mut o = Out::new(&mut m.rec);
        record::head(&mut o, boot, provider, &tags[..nf], asked, patched, refused, outcome);
        record::section(&mut o, "prompt", &m.prompt[..plen]);
        record::section(&mut o, "answer", &m.answer[..an]);
        record::rebuild(&mut o);
        o.len()
    };
    let mut rp = [0u8; 64];
    let rpath = {
        let mut o = Out::new(&mut rp[..]);
        o.s("/var/log/diag.").dec(boot).s(".md");
        let n = o.len();
        core::str::from_utf8(&rp[..n]).unwrap_or("")
    };
    if let Err(e) = pwrite_file(&mut m.req, rpath, &m.rec[..rn]) {
        let mut b = [0u8; 48];
        let mut o = Out::new(&mut b);
        o.s("stage=record err=-").dec(e.unsigned_abs());
        let n = o.len();
        verdict(total, asked, patched, refused, "FAIL", core::str::from_utf8(&b[..n]).unwrap_or(""));
    }
    note(&["recorded ", rpath]);
    if patched > 0 {
        note(&["rebuild on the bench (SH-5 owes the on-metal build): ", record::REBUILD]);
    }
    note(&["diag --loop (rebuild, reboot, re-read until the line passes) is owed until SH-5"]);
    if failed.is_some() {
        verdict(total, asked, patched, refused, "FAIL", "stage=provider");
    }
    verdict(total, asked, patched, refused, "PASS", "")
}

/// Resolve every hunk of every file, then stream each to `<file>~dgn`, copy it back, unlink the temp.
/// Returns the files patched, or the refusal and the file's index (nothing written on a refusal).
fn apply_patch(req: &mut [u8], outb: &mut [u8; PATH_IO_MAX], scratch: &mut [u8], root: &str, patch: &diff::Patch<'_>) -> Result<usize, (diff::Refuse, usize)> {
    let mut spans = [Span::default(); diff::MAX_HUNKS];
    let mut base = 0usize;
    for (fi, f) in patch.files().iter().enumerate() {
        let hs = patch.hunks_of(f);
        let mut jb = [0u8; 256];
        let fp = join(&mut jb, root, f.path, "").ok_or((diff::Refuse::BadPath, fi))?;
        apply::resolve(&mut PathSrc { path: fp }, hs, &mut spans[base..base + hs.len()], scratch, fi).map_err(|r| (r, fi))?;
        base += hs.len();
    }
    let mut base = 0usize;
    let mut done = 0usize;
    for (fi, f) in patch.files().iter().enumerate() {
        let hs = patch.hunks_of(f);
        let mut jb = [0u8; 256];
        let mut tb = [0u8; 256];
        let fp = join(&mut jb, root, f.path, "").ok_or((diff::Refuse::BadPath, fi))?;
        let tp = join(&mut tb, root, f.path, "~dgn").ok_or((diff::Refuse::BadPath, fi))?;
        // Stream the patched file to the temp, buffered in `outb`.
        let mut filled = 0usize;
        let mut off = 0u64;
        let mut flags = PATH_W_TRUNC;
        {
            let mut sink = |b: &[u8]| -> Result<(), i64> {
                let mut b = b;
                while !b.is_empty() {
                    let k = b.len().min(outb.len() - filled);
                    outb[filled..filled + k].copy_from_slice(&b[..k]);
                    filled += k;
                    b = &b[k..];
                    if filled == outb.len() {
                        let r = pwrite1(req, tp, flags, off, &outb[..filled]);
                        if r < 0 {
                            return Err(r);
                        }
                        off += filled as u64;
                        filled = 0;
                        flags = 0;
                    }
                }
                Ok(())
            };
            apply::emit(&mut PathSrc { path: fp }, hs, &spans[base..base + hs.len()], scratch, &mut sink).map_err(|r| (r, fi))?;
        }
        let r = pwrite1(req, tp, flags, off, &outb[..filled]);
        if r < 0 {
            return Err((diff::Refuse::Io(r), fi));
        }
        let total = off + filled as u64;
        // Copy back over the original, then drop the temp.
        let mut pos = 0u64;
        let mut first = true;
        while pos < total || first {
            let k = pread(tp, pos, &mut outb[..]);
            if k < 0 {
                return Err((diff::Refuse::Io(k), fi));
            }
            let r = pwrite1(req, fp, if first { PATH_W_TRUNC } else { 0 }, pos, &outb[..k as usize]);
            if r < 0 {
                return Err((diff::Refuse::Io(r), fi));
            }
            first = false;
            if k == 0 {
                break;
            }
            pos += k as u64;
        }
        let _ = pwrite1(req, tp, PATH_W_UNLINK, 0, &[]);
        base += hs.len();
        done += 1;
    }
    Ok(done)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: SELFDIAG: panic -> FAIL ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — a console program (flags 0), so a bare `diag`
/// runs in the foreground. Kept by the x86 link script under a PT_NOTE header.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
