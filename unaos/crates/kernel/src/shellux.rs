//! SHELLUX (R75) — the desktop shell's line editor grows history (Up/Down), Tab completion (verbs, then
//! directory entries through the mount table) and control keys (Ctrl-C/L/A/E/U/W).
//!
//! Bytes arrive from `main.rs::handle_key` exactly as the HID fold makes them: Up `0x1F`, Down `0x1E`,
//! Tab `0x09`, Ctrl-letter `0x01..0x1A`. The core [`key`] is PURE over its arguments (line, caret holder,
//! history, verb list, directory lister) so the `shellux` fixture drives it with scripted sources; the
//! [`console_key`] wrapper binds it to the real `Console`, `CMD_HISTORY`, `HOST_VERBS` and the mount table.
//! The history store and `history` verb are BASICS' (`shell.rs`); `cd`/`pwd` are JD4's.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::video::termsel::LineSel;

/// What one key did beyond editing the line.
#[derive(Default, Debug)]
pub struct Out {
    /// Repaint owed: 0 none, 1 the input line, 2 the whole terminal.
    pub repaint: u8,
    /// Lines to print into the scrollback (candidates, `^C`).
    pub print: Vec<String>,
    /// Ctrl-L: clear the scrollback (the `clear` verb's path, `Console::clear`).
    pub clear: bool,
}

/// History walk state. `idx` 0 = editing the live line; k = the k-th newest. `shown` is the text the walk
/// last placed, so any other edit makes the walk stale without a hook on Enter.
pub struct Nav { idx: usize, draft: String, shown: String }
impl Nav { pub const fn new() -> Self { Nav { idx: 0, draft: String::new(), shown: String::new() } } }
static NAV: crate::sync::Mutex<Nav> = crate::sync::Mutex::new(Nav::new());

/// Does the line editor take this byte?
pub fn wants(c: u8) -> bool { matches!(c, 0x01 | 0x03 | 0x05 | 0x09 | 0x0C | 0x15 | 0x17 | 0x1E | 0x1F) }

fn set_line(line: &mut String, sel: &mut LineSel, text: &str) {
    line.clear();
    line.push_str(text);
    sel.end_selection();
    sel.set_caret(line.len(), line.len());
}

/// One editor key. `hist` oldest first; `verbs` the registered words; `ls(dir)` lists a directory as typed.
pub fn key(c: u8, line: &mut String, sel: &mut LineSel, hist: &[String], nav: &mut Nav,
           verbs: &[&str], ls: &dyn Fn(&str) -> Vec<(String, bool)>) -> Out {
    let mut o = Out::default();
    if nav.idx > 0 && *line != nav.shown { nav.idx = 0; }
    let caret = sel.caret_col(line.len());
    match c {
        0x1F => { // Up
            if hist.is_empty() { return o; }
            if nav.idx == 0 { nav.draft = line.clone(); }
            nav.idx = core::cmp::min(nav.idx + 1, hist.len());
            let t = hist[hist.len() - nav.idx].clone();
            set_line(line, sel, &t); nav.shown = t; o.repaint = 1;
        }
        0x1E => { // Down
            if nav.idx == 0 { return o; }
            nav.idx -= 1;
            let t = if nav.idx == 0 { nav.draft.clone() } else { hist[hist.len() - nav.idx].clone() };
            set_line(line, sel, &t); nav.shown = t; o.repaint = 1;
        }
        0x03 => { // Ctrl-C: cancel the line (the shell tracks no foreground child at this seam)
            line.clear(); sel.end_selection(); sel.set_caret(0, 0); nav.idx = 0;
            o.print.push(String::from("^C")); o.repaint = 2;
        }
        0x0C => { o.clear = true; o.repaint = 2; } // Ctrl-L
        0x01 => { sel.end_selection(); sel.set_caret(0, line.len()); o.repaint = 1; }
        0x05 => { sel.end_selection(); sel.set_caret(line.len(), line.len()); o.repaint = 1; }
        0x15 => { // Ctrl-U: kill from the line start to the caret
            line.replace_range(..caret, ""); sel.end_selection(); sel.set_caret(0, line.len()); o.repaint = 1;
        }
        0x17 => { // Ctrl-W: kill the word before the caret
            let b = line.as_bytes();
            let mut s = caret;
            while s > 0 && b[s - 1] == b' ' { s -= 1; }
            while s > 0 && b[s - 1] != b' ' { s -= 1; }
            line.replace_range(s..caret, ""); sel.end_selection(); sel.set_caret(s, line.len()); o.repaint = 1;
        }
        0x09 => complete(line, sel, caret, verbs, ls, &mut o),
        _ => {}
    }
    o
}

fn complete(line: &mut String, sel: &mut LineSel, caret: usize, verbs: &[&str],
            ls: &dyn Fn(&str) -> Vec<(String, bool)>, o: &mut Out) {
    let pre = String::from(&line[..caret]);
    let ws = pre.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let word = &pre[ws..];
    let first = pre[..ws].trim().is_empty();
    // (replacement text for the word's tail after `base`, display name, is_dir)
    let (base, mut cands): (String, Vec<(String, bool)>) = if first {
        (String::new(), verbs.iter().filter(|v| v.starts_with(word)).map(|v| (String::from(*v), false)).collect())
    } else {
        let (dir, leaf) = match word.rfind('/') { Some(i) => (&word[..=i], &word[i + 1..]), None => ("", word) };
        let lower = leaf.to_ascii_lowercase();
        (String::from(dir), ls(dir).into_iter().filter(|(n, _)| n.to_ascii_lowercase().starts_with(&lower)).collect())
    };
    cands.sort(); cands.dedup();
    if cands.is_empty() { return; }
    let done = if cands.len() == 1 {
        let (n, d) = &cands[0];
        format!("{}{}{}", base, n, if first || !*d { " " } else { "/" })
    } else {
        let mut names = String::new();
        for (n, d) in &cands { if !names.is_empty() { names.push_str("  "); } names.push_str(n); if *d { names.push('/'); } }
        o.print.push(names);
        // extend to the longest common prefix of the candidates (case of the first)
        let f = cands[0].0.as_bytes();
        let mut k = f.len();
        for (n, _) in &cands[1..] {
            let m = n.as_bytes().iter().zip(f).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            k = core::cmp::min(k, m);
        }
        format!("{}{}", base, &cands[0].0[..k])
    };
    let mut nl = String::from(&line[..ws]);
    nl.push_str(&done);
    let at = nl.len();
    nl.push_str(&line[caret..]);
    *line = nl;
    sel.end_selection(); sel.set_caret(at, line.len());
    o.repaint = 2;
}

/// The real binding: `Console` line + caret, the shell's history, the verb table, the mount table.
pub fn console_key(c: u8, console: &mut crate::console::Console) -> u8 {
    let hist = crate::shell::history_lines();
    let verbs = crate::shell::verb_names();
    let o = { let mut nav = NAV.lock(); key(c, &mut console.current_input, &mut console.sel, &hist, &mut nav, &verbs, &crate::shell::complete_ls) };
    if o.clear { console.clear(); }
    for l in &o.print { console.println(l); }
    o.repaint
}

/// The `shellux` fixture: a scripted key sequence against scripted sources, resulting lines checked.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
pub fn selftest() {
    let hist: Vec<String> = ["ls", "pwd", "cat A.TXT"].iter().map(|s| String::from(*s)).collect();
    let verbs = ["cat", "cd", "clear", "ls"];
    let lister = |d: &str| -> Vec<(String, bool)> {
        if d.is_empty() { alloc::vec![(String::from("DOCS"), true), (String::from("README.TXT"), false), (String::from("RELEASE.TXT"), false)] }
        else if d == "DOCS/" { alloc::vec![(String::from("A.TXT"), false)] } else { Vec::new() }
    };
    let (mut line, mut sel, mut nav) = (String::new(), LineSel::new(), Nav::new());
    let run = |keys: &[u8], line: &mut String, sel: &mut LineSel, nav: &mut Nav| -> Vec<String> {
        let mut pr = Vec::new();
        for &k in keys { if (32..127).contains(&k) { let _ = sel.type_byte(k, line); } else { let o = key(k, line, sel, &hist, nav, &verbs, &lister); pr.extend(o.print); } }
        pr
    };
    // history: Up x3 reaches the oldest, a 4th stays, Down x2 comes back, Down past the end restores the draft
    run(b"xy", &mut line, &mut sel, &mut nav);
    run(&[0x1F, 0x1F, 0x1F, 0x1F], &mut line, &mut sel, &mut nav);
    let h1 = line == "ls";
    run(&[0x1E, 0x1E], &mut line, &mut sel, &mut nav);
    let h2 = line == "cat A.TXT";
    run(&[0x1E], &mut line, &mut sel, &mut nav);
    let history = h1 && h2 && line == "xy";
    // completion
    let mut results = [false; 4];
    line.clear(); sel.end_selection(); sel.set_caret(0, 0); nav.idx = 0;
    run(b"cl", &mut line, &mut sel, &mut nav); run(&[0x09], &mut line, &mut sel, &mut nav);
    results[0] = line == "clear ";
    line.clear(); sel.set_caret(0, 0);
    run(b"c", &mut line, &mut sel, &mut nav); let pr = run(&[0x09], &mut line, &mut sel, &mut nav);
    results[1] = line == "c" && pr.len() == 1 && pr[0] == "cat  cd  clear";
    line.clear(); sel.set_caret(0, 0);
    run(b"cat DO", &mut line, &mut sel, &mut nav); run(&[0x09], &mut line, &mut sel, &mut nav);
    run(b"a", &mut line, &mut sel, &mut nav); run(&[0x09], &mut line, &mut sel, &mut nav);
    results[2] = line == "cat DOCS/A.TXT ";
    line.clear(); sel.set_caret(0, 0);
    run(b"cat re", &mut line, &mut sel, &mut nav); let pr = run(&[0x09], &mut line, &mut sel, &mut nav);
    results[3] = line == "cat RE" && pr.len() == 1 && pr[0] == "README.TXT  RELEASE.TXT";
    let completions = results.iter().all(|r| *r);
    // control keys
    let (cc, cl, mut ca, ce, cu, cw);
    line.clear(); sel.set_caret(0, 0);
    run(b"hello world", &mut line, &mut sel, &mut nav);
    cw = { run(&[0x17], &mut line, &mut sel, &mut nav); line == "hello " };
    run(&[0x01], &mut line, &mut sel, &mut nav);
    ca = sel.caret_col(line.len()) == 0;
    run(&[0x05], &mut line, &mut sel, &mut nav);
    ce = sel.caret_col(line.len()) == line.len();
    run(&[0x01], &mut line, &mut sel, &mut nav); run(b"Z", &mut line, &mut sel, &mut nav);
    ca = ca && line == "Zhello " && sel.caret_col(line.len()) == 1;
    run(&[0x15], &mut line, &mut sel, &mut nav);
    cu = line == "hello ";
    let o = key(0x0C, &mut line, &mut sel, &hist, &mut nav, &verbs, &lister);
    cl = o.clear && line == "hello ";
    let o = key(0x03, &mut line, &mut sel, &hist, &mut nav, &verbs, &lister);
    cc = line.is_empty() && o.print.len() == 1 && o.print[0] == "^C";
    let ctrl = cc && cl && ca && ce && cu && cw;
    let cwd = crate::shell::cwd_now();
    let vt = crate::shell::verb_names();
    let verbs_real = ["history", "cd", "pwd", "clear", "ls"].iter().all(|v| vt.contains(v));
    let cwd_ok = cwd.starts_with('/') && verbs_real;
    let ok = history && completions && ctrl && cwd_ok;
    serial_println!("[shellux] c={} l={} a={} e={} u={} w={} comp={:?} verbs_real={}", cc, cl, ca, ce, cu, cw, results, verbs_real);
    serial_println!(":: SHELLUX: history={} completions=verbs+paths ctrl=[c,l,a,e,u,w] cwd={} -> {} ::",
        if history { "up-down" } else { "BAD" }, cwd, if ok { "PASS" } else { "FAIL" });
}
