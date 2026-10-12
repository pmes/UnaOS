// SPDX-License-Identifier: LGPL-3.0-or-later
// EDITCORE (SR73): the piece table and the Buffer under a 10 000-operation fuzz against a `Vec<char>` model,
// and the line index against a naive count on 50 generated files. xorshift64 PRNG in-test; the seed is
// printed (override with EDITCORE_SEED=<u64>). Never panics is part of the assertion: every op runs.

use edit_core::{Buffer, Direction, IndentStyle, Motion, Selection, Text, Unit};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const ATOMS: &[&str] = &[
    "a", "b", "z", " ", " ", "\t", "\n", "\n", "\r\n", "é", "e\u{301}", "\u{1F468}\u{200D}\u{1F469}", "\u{1F1FA}\u{1F1F8}",
    "日本", "foo", "_x1", ".", "'", "\u{AC00}\u{11A8}", "    ",
];

fn rand_text(r: &mut Rng, max_atoms: usize) -> String {
    (0..r.below(max_atoms + 1)).map(|_| ATOMS[r.below(ATOMS.len())]).collect()
}

fn byte_of(model: &[char], ci: usize) -> usize {
    model[..ci].iter().map(|c| c.len_utf8()).sum()
}
fn char_of(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

fn check(b: &Buffer, model: &[char], step: usize, seed: u64) {
    let want: String = model.iter().collect();
    let got = b.to_string();
    assert_eq!(got, want, "seed {seed} step {step}: text differs from the Vec<char> model");
    b.text().check_invariants().unwrap_or_else(|e| panic!("seed {seed} step {step}: {e}"));
    let naive_lines = model.iter().filter(|&&c| c == '\n').count() + 1;
    assert_eq!(b.line_count(), naive_lines, "seed {seed} step {step}: line count vs naive");
    let mut prev_end = 0;
    for (i, s) in b.selections().iter().enumerate() {
        for x in [s.anchor, s.head] {
            assert!(x <= want.len() && want.is_char_boundary(x), "seed {seed} step {step}: selection off a boundary");
        }
        if i > 0 {
            assert!(s.start() >= prev_end, "seed {seed} step {step}: selections overlap / unsorted");
        }
        prev_end = s.end();
    }
}

fn apply_changes(model: &mut Vec<char>, changes: &[edit_core::Change]) {
    for c in changes {
        let s: String = model.iter().collect();
        let (a, e) = (char_of(&s, c.start), char_of(&s, c.old_end));
        model.splice(a..e, c.text.chars());
    }
}

#[test]
fn fuzz_10000_ops_against_vec_char_model() {
    let seed: u64 = std::env::var("EDITCORE_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5EED_ED17_C0DE_0073);
    println!("EDITCORE fuzz seed={seed}");
    let mut r = Rng(seed);
    let init = rand_text(&mut r, 40);
    let mut b = Buffer::new(&init);
    let mut model: Vec<char> = init.chars().collect();
    let mut undo_snaps: Vec<Vec<char>> = Vec::new();
    let mut redo_snaps: Vec<Vec<char>> = Vec::new();
    let mut counts = [0usize; 12];
    const OPS: usize = 10_000;
    for step in 0..OPS {
        let op = r.below(12);
        counts[op] += 1;
        let before = model.clone();
        let depth = b.undo_depth();
        match op {
            0 | 1 => {
                // replace a random char range with random text (one group)
                let a = r.below(model.len() + 1);
                let e = (a + r.below(8)).min(model.len());
                let t = rand_text(&mut r, 3);
                b.replace(byte_of(&model, a)..byte_of(&model, e), &t);
                model.splice(a..e, t.chars());
            }
            2 => {
                // typing at 1..3 cursors (model: insert at each, right to left)
                let mut cs: Vec<usize> = (0..1 + r.below(3)).map(|_| r.below(model.len() + 1)).collect();
                cs.sort_unstable();
                cs.dedup();
                let sels: Vec<Selection> = cs.iter().map(|&c| Selection::cursor(byte_of(&model, c))).collect();
                b.set_selections(&sels);
                let t = ATOMS[r.below(ATOMS.len())];
                let n = 1 + r.below(4);
                for _ in 0..n {
                    b.insert(t);
                }
                for &c in cs.iter().rev() {
                    for _ in 0..n {
                        model.splice(c..c, t.chars());
                    }
                }
                // the n keystrokes coalesce into one group
                assert!(b.undo_depth() <= depth + 1, "seed {seed} step {step}: typing did not coalesce");
            }
            3 => {
                // single-cursor backspace / forward delete by grapheme (model: the same cluster rule on the model)
                let c = r.below(model.len() + 1);
                let s: String = model.iter().collect();
                let at = byte_of(&model, c);
                b.set_cursor(at);
                let fwd = r.below(2) == 0;
                b.delete(Unit::Grapheme, if fwd { Direction::Forward } else { Direction::Backward });
                // reference: grapheme boundaries of the whole model string (a cluster never crosses an LF)
                let gb = edit_core::unicode::grapheme_boundaries(&s);
                let (x, y) = if fwd {
                    (at, gb.iter().copied().find(|&g| g > at).unwrap_or(s.len()))
                } else {
                    (gb.iter().rev().copied().find(|&g| g < at).unwrap_or(0), at)
                };
                let (cx, cy) = (char_of(&s, x), char_of(&s, y));
                model.drain(cx..cy);
            }
            4 => {
                if b.can_undo() {
                    b.undo();
                    let snap = undo_snaps.pop().expect("model undo stack in step with the buffer's");
                    redo_snaps.push(std::mem::replace(&mut model, snap));
                }
            }
            5 => {
                if b.can_redo() {
                    b.redo();
                    let snap = redo_snaps.pop().expect("model redo stack in step with the buffer's");
                    undo_snaps.push(std::mem::replace(&mut model, snap));
                }
            }
            // The rest have no independent model: their Change lists are replayed onto the model instead,
            // which is exactly the contract a view relies on.
            6 => {
                let ch = b.newline();
                apply_changes(&mut model, &ch);
            }
            7 => {
                b.set_indent_style(if r.below(2) == 0 { IndentStyle::Tabs } else { IndentStyle::Spaces(1 + r.below(8) as u8) });
                let ch = if r.below(2) == 0 { b.indent() } else { b.outdent() };
                apply_changes(&mut model, &ch);
            }
            8 => {
                let unit = [Unit::Word, Unit::Line][r.below(2)];
                let dir = [Direction::Backward, Direction::Forward][r.below(2)];
                let ch = b.delete(unit, dir);
                apply_changes(&mut model, &ch);
            }
            9 => {
                let ch = if r.below(4) == 0 { b.delete_lines() } else { b.insert(&rand_text(&mut r, 2)) };
                apply_changes(&mut model, &ch);
            }
            10 => {
                let m = [
                    Motion::Left, Motion::Right, Motion::WordLeft, Motion::WordRight, Motion::LineStart, Motion::LineEnd,
                    Motion::Up, Motion::Down, Motion::DocStart, Motion::DocEnd,
                ][r.below(10)];
                b.move_cursors(m, r.below(3) == 0);
            }
            _ => {
                // random selections (may overlap; the core must merge) or the idle signal
                if r.below(2) == 0 {
                    b.close_group();
                } else {
                    let v: Vec<Selection> =
                        (0..1 + r.below(4)).map(|_| Selection::new(r.below(b.len() + 2), r.below(b.len() + 2))).collect();
                    b.set_selections(&v);
                }
            }
        }
        // keep the model's undo stack in step: a new group pushed the pre-op state; an edit cleared redo
        if !matches!(op, 4 | 5) {
            if b.undo_depth() > depth {
                undo_snaps.push(before);
            }
            if !b.can_redo() {
                redo_snaps.clear();
            }
        }
        assert_eq!(undo_snaps.len(), b.undo_depth(), "seed {seed} step {step} op {op}: undo depth");
        check(&b, &model, step, seed);
        if model.len() > 4000 {
            // keep the run in a size where the O(n) model stays cheap: drop the first half as one edit
            let half = model.len() / 2;
            b.replace(0..byte_of(&model, half), "");
            let rest = model[half..].to_vec();
            undo_snaps.push(std::mem::replace(&mut model, rest));
            redo_snaps.clear();
            check(&b, &model, step, seed);
        }
    }
    // unwind the whole history: every snapshot must come back in order
    while b.can_undo() {
        b.undo();
        model = undo_snaps.pop().unwrap();
        check(&b, &model, OPS, seed);
    }
    println!(
        "EDITCORE fuzz: {OPS} ops OK (replace {} type {} grapheme-del {} undo {} redo {} newline {} indent {} word/line-del {} lines/insert {} motion {} sel/idle {}), final len={} lines={} pieces={}",
        counts[0] + counts[1], counts[2], counts[3], counts[4], counts[5], counts[6], counts[7], counts[8], counts[9], counts[10], counts[11],
        b.len(), b.line_count(), b.text().piece_count()
    );
}

#[test]
fn line_index_vs_naive_on_50_generated_files() {
    let mut r = Rng(0x0073_F11E_5000_0050);
    for f in 0..50 {
        let s = rand_text(&mut r, 400);
        let t = Text::new(&s);
        let naive: Vec<&str> = s.split('\n').collect();
        assert_eq!(t.line_count(), naive.len(), "file {f}");
        let mut off = 0;
        for (l, raw) in naive.iter().enumerate() {
            assert_eq!(t.line_start(l), off, "file {f} line {l}");
            let content = raw.strip_suffix('\r').filter(|_| l + 1 < naive.len()).unwrap_or(raw);
            assert_eq!(t.line(l), content, "file {f} line {l}");
            off += raw.len() + 1;
        }
        // and the same file built by appending in random chunks matches
        let mut u = Text::new("");
        let mut at = 0;
        while at < s.len() {
            let mut e = (at + 1 + r.below(16)).min(s.len());
            while !s.is_char_boundary(e) {
                e += 1;
            }
            assert!(u.insert(u.len(), &s[at..e]));
            at = e;
        }
        assert_eq!(u, t, "file {f}");
        u.check_invariants().unwrap();
    }
    println!("EDITCORE line index: 50 generated files match the naive split");
}
