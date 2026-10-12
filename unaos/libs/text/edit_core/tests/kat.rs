// SPDX-License-Identifier: LGPL-3.0-or-later
// EDITCORE (SR73): known-answer tests, one block per operation of the core.

use edit_core::{Buffer, Direction, IndentStyle, LineEnding, Motion, Pos, SearchOpts, Selection, Text, Unit};

fn s(b: &Buffer) -> String {
    b.to_string()
}
fn carets(b: &Buffer) -> Vec<usize> {
    b.selections().iter().map(|x| x.head).collect()
}

// ---- piece table + line index ------------------------------------------------------------------------

#[test]
fn piece_table_insert_delete_slice() {
    let mut t = Text::new("hello world");
    assert!(t.insert(5, ","));
    assert!(t.insert(t.len(), "!"));
    assert!(t.insert(0, ">> "));
    assert_eq!(t.to_string(), ">> hello, world!");
    assert!(t.delete(3, 9));
    assert_eq!(t.to_string(), ">>  world!");
    assert_eq!(t.slice(4, 9), "world");
    // refused: past the end, inside a UTF-8 sequence
    let mut u = Text::new("aé");
    assert!(!u.insert(2, "x"), "inside é");
    assert!(!u.delete(0, 2));
    assert!(!u.insert(9, "x"));
    assert_eq!(u.to_string(), "aé");
    t.check_invariants().unwrap();
}

#[test]
fn typing_extends_one_piece() {
    let mut t = Text::new("ab");
    for (i, c) in "xyz".chars().enumerate() {
        assert!(t.insert(1 + i, &c.to_string()));
    }
    assert_eq!(t.to_string(), "axyzb");
    assert_eq!(t.piece_count(), 3, "orig-left, one add piece, orig-right");
}

#[test]
fn line_index_and_positions() {
    let mut t = Text::new("one\r\ntwo\nthree");
    assert_eq!(t.line_count(), 3);
    assert_eq!((t.line_start(1), t.line_start(2)), (5, 9));
    assert_eq!(t.line(0), "one");
    assert_eq!(t.line_ending(0), LineEnding::CrLf);
    assert_eq!(t.line_ending(1), LineEnding::Lf);
    assert_eq!(t.line_ending(2), LineEnding::None);
    assert_eq!(t.offset_to_pos(7), Pos::new(1, 2));
    assert_eq!(t.pos_to_offset(Pos::new(2, 3)), 12);
    assert_eq!(t.pos_to_offset(Pos::new(0, 99)), 3, "clamps to content end, never inside CRLF");
    assert_eq!(t.pos_to_offset(Pos::new(9, 0)), t.len());
    assert!(t.insert(6, "\nX\n"));
    assert_eq!(t.line_count(), 5);
    assert_eq!(t.line(1), "t");
    assert_eq!(t.line(2), "X");
    assert!(t.delete(0, 7));
    assert_eq!(t.to_string(), "X\nwo\nthree");
    assert_eq!(t.line_count(), 3);
    t.check_invariants().unwrap();
}

#[test]
fn crlf_preserved_through_load_edit_save() {
    let src = "a\r\nb\nc\r\n";
    let mut b = Buffer::new(src);
    b.set_cursor(b.pos_to_offset(Pos::new(1, 1)));
    b.insert("!");
    assert_eq!(s(&b), "a\r\nb!\nc\r\n");
    assert_eq!(b.line_ending(0), LineEnding::CrLf);
    assert_eq!(b.line_ending(1), LineEnding::Lf);
}

// ---- grapheme motion ----------------------------------------------------------------------------------

fn walk_right(text: &str) -> Vec<usize> {
    let b = Buffer::new(text);
    let mut v = vec![0];
    let mut at = 0;
    while at < b.len() {
        at = b.next_grapheme(at);
        v.push(at);
    }
    v
}
fn walk_left(text: &str) -> Vec<usize> {
    let b = Buffer::new(text);
    let mut v = vec![b.len()];
    let mut at = b.len();
    while at > 0 {
        at = b.prev_grapheme(at);
        v.push(at);
    }
    v.reverse();
    v
}

#[test]
fn grapheme_motion_combining_marks() {
    // e + U+0301 + U+0323 is one cluster; so is the precomposed é.
    let t = "e\u{301}\u{323}xé";
    assert_eq!(walk_right(t), vec![0, 5, 6, 8]);
    assert_eq!(walk_left(t), walk_right(t));
}

#[test]
fn grapheme_motion_zwj_emoji() {
    // family: man ZWJ woman ZWJ girl (one cluster, 18 bytes); flag: two RIs; skin tone modifier.
    let fam = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let t = format!("a{fam}\u{1F1FA}\u{1F1F8}\u{1F44D}\u{1F3FD}b");
    let r = walk_right(&t);
    assert_eq!(r, vec![0, 1, 1 + 18, 1 + 18 + 8, 1 + 18 + 8 + 8, 1 + 18 + 8 + 8 + 1]);
    assert_eq!(walk_left(&t), r);
}

#[test]
fn grapheme_motion_crlf_is_one_step() {
    let t = "ab\r\ncd";
    assert_eq!(walk_right(t), vec![0, 1, 2, 4, 5, 6]);
    assert_eq!(walk_left(t), vec![0, 1, 2, 4, 5, 6]);
    let mut b = Buffer::new(t);
    b.set_cursor(4);
    b.move_cursors(Motion::Left, false);
    assert_eq!(b.cursor(), 2, "Left from the start of line 1 lands at line 0's content end, not inside CRLF");
}

#[test]
fn grapheme_motion_hangul_jamo() {
    // L V T conjoining jamo form one syllable cluster; a precomposed LV + T too.
    let t = "\u{1100}\u{1161}\u{11A8}\u{AC00}\u{11A8}a";
    assert_eq!(walk_right(t), vec![0, 9, 15, 16]);
    assert_eq!(walk_left(t), walk_right(t));
}

#[test]
fn backspace_deletes_whole_cluster() {
    let mut b = Buffer::new("x\u{1F468}\u{200D}\u{1F469}");
    b.set_cursor(b.len());
    b.delete(Unit::Grapheme, Direction::Backward);
    assert_eq!(s(&b), "x");
}

// ---- word motion --------------------------------------------------------------------------------------

#[test]
fn word_motion() {
    let mut b = Buffer::new("let foo_bar = can't(3.14);\n  next");
    let mut stops = vec![];
    for _ in 0..9 {
        b.move_cursors(Motion::WordRight, false);
        stops.push(b.cursor());
    }
    // let | foo_bar | = | can't | ( | 3.14 | ) | ; | next
    assert_eq!(stops, vec![3, 11, 13, 19, 20, 24, 25, 26, 33]);
    b.move_cursors(Motion::WordLeft, false);
    assert_eq!(b.cursor(), 29);
    b.move_cursors(Motion::WordLeft, false);
    assert_eq!(b.cursor(), 25, "crosses the line ending, skipping whitespace");
}

#[test]
fn delete_word_both_ways() {
    let mut b = Buffer::new("alpha beta gamma");
    b.set_cursor(10);
    b.delete(Unit::Word, Direction::Backward);
    assert_eq!(s(&b), "alpha  gamma");
    b.delete(Unit::Word, Direction::Forward);
    assert_eq!(s(&b), "alpha ");
}

#[test]
fn delete_line_both_ways_and_delete_lines() {
    let mut b = Buffer::new("one\ntwo three\nfour");
    b.set_cursor(7);
    b.delete(Unit::Line, Direction::Backward);
    assert_eq!(s(&b), "one\n three\nfour");
    b.delete(Unit::Line, Direction::Forward);
    assert_eq!(s(&b), "one\n\nfour");
    b.delete(Unit::Line, Direction::Backward);
    assert_eq!(s(&b), "one\nfour", "at the line start: the ending before it");
    b.set_cursor(1);
    b.delete_lines();
    assert_eq!(s(&b), "four");
    let mut c = Buffer::new("a\nb");
    c.set_cursor(3);
    c.delete_lines();
    assert_eq!(s(&c), "a", "the last line takes the ending before it");
}

// ---- motion ------------------------------------------------------------------------------------------

#[test]
fn line_and_vertical_motion() {
    let mut b = Buffer::new("abcdef\nxy\nlonger line");
    b.set_cursor(4);
    b.move_cursors(Motion::Down, false);
    assert_eq!(b.offset_to_pos(b.cursor()), Pos::new(1, 2), "clamped to the short line");
    b.move_cursors(Motion::Down, false);
    assert_eq!(b.offset_to_pos(b.cursor()), Pos::new(2, 2));
    b.move_cursors(Motion::LineEnd, false);
    assert_eq!(b.cursor(), b.len());
    b.move_cursors(Motion::LineStart, false);
    assert_eq!(b.offset_to_pos(b.cursor()), Pos::new(2, 0));
    b.move_cursors(Motion::Up, false);
    b.move_cursors(Motion::Up, false);
    assert_eq!(b.cursor(), 0);
    b.move_cursors(Motion::DocEnd, true);
    assert_eq!(b.selections(), &[Selection::new(0, b.len())]);
    b.move_cursors(Motion::Left, false);
    assert_eq!(b.cursor(), 0, "Left collapses a selection to its start");
}

// ---- selections and multiple cursors -----------------------------------------------------------------

#[test]
fn cursors_sorted_and_merged() {
    let mut b = Buffer::new("0123456789");
    b.set_selections(&[Selection::new(6, 8), Selection::cursor(2), Selection::new(7, 9), Selection::cursor(2)]);
    assert_eq!(b.selections(), &[Selection::cursor(2), Selection::new(6, 9)]);
    b.add_cursor(9);
    assert_eq!(b.selections().len(), 2, "a cursor touching a selection's end merges into it");
}

#[test]
fn multi_cursor_insert() {
    let mut b = Buffer::new("a\nb\nc");
    b.set_selections(&[Selection::cursor(1), Selection::cursor(3), Selection::cursor(5)]);
    let ch = b.insert(";");
    assert_eq!(s(&b), "a;\nb;\nc;");
    assert_eq!(carets(&b), vec![2, 5, 8]);
    assert_eq!(ch.len(), 3);
    b.insert("!");
    assert_eq!(s(&b), "a;!\nb;!\nc;!");
    b.undo();
    assert_eq!(s(&b), "a\nb\nc", "multi-cursor typing is one group");
    assert_eq!(carets(&b), vec![1, 3, 5], "undo restores every cursor");
}

#[test]
fn typing_replaces_selections() {
    let mut b = Buffer::new("foo bar foo");
    b.set_selections(&[Selection::new(0, 3), Selection::new(8, 11)]);
    b.insert("X");
    assert_eq!(s(&b), "X bar X");
    assert_eq!(b.selections(), &[Selection::cursor(1), Selection::cursor(7)]);
}

// ---- replace, newline, indent ------------------------------------------------------------------------

#[test]
fn replace_and_set_text() {
    let mut b = Buffer::new("hello world");
    let ch = b.replace(6..11, "there");
    assert_eq!(s(&b), "hello there");
    assert_eq!(ch[0].start, 6);
    assert_eq!(ch[0].old_end, 11);
    // out of range never panics
    b.replace(100..200, "!");
    assert_eq!(s(&b), "hello there!");
    let ch = b.set_text("hello where!");
    assert_eq!(ch.len(), 1);
    assert_eq!((ch[0].start, ch[0].old_end, ch[0].text.as_str()), (6, 7, "w"), "minimal single edit");
    assert!(b.set_text("hello where!").is_empty());
}

#[test]
fn newline_auto_indent_keeps_ending() {
    let mut b = Buffer::new("fn x() {\r\n    body\r\n}");
    b.set_cursor(b.pos_to_offset(Pos::new(1, 8)));
    b.newline();
    assert_eq!(s(&b), "fn x() {\r\n    body\r\n    \r\n}");
    b.insert("more");
    assert_eq!(b.line(2), "    more");
    let mut lf = Buffer::new("\tif a:");
    lf.set_cursor(lf.len());
    lf.newline();
    assert_eq!(s(&lf), "\tif a:\n\t", "last line has no ending: the file default (LF)");
}

#[test]
fn indent_outdent_tabs_and_spaces() {
    let mut b = Buffer::new("a\n\nb\nc");
    b.set_selections(&[Selection::new(0, 5)]); // lines 0..=2 touched; line 3 untouched
    b.indent();
    assert_eq!(s(&b), "    a\n\n    b\nc", "empty lines stay empty");
    b.outdent();
    assert_eq!(s(&b), "a\n\nb\nc");
    b.set_indent_style(IndentStyle::Tabs);
    b.set_cursor(b.len());
    b.indent();
    assert_eq!(s(&b), "a\n\nb\n\tc");
    b.outdent();
    assert_eq!(s(&b), "a\n\nb\nc");
    let mut p = Buffer::new("  two");
    p.outdent();
    assert_eq!(s(&p), "two", "outdent removes up to a unit of spaces");
    // a selection ending at a line start does not touch that line
    let mut q = Buffer::new("x\ny");
    q.set_selections(&[Selection::new(0, 2)]);
    q.indent();
    assert_eq!(s(&q), "    x\ny");
}

// ---- undo grouping -----------------------------------------------------------------------------------

#[test]
fn undo_type_five_chars_is_one_group() {
    let mut b = Buffer::new("");
    for c in ["h", "e", "l", "l", "o"] {
        b.insert(c);
    }
    assert_eq!(b.undo_depth(), 1);
    b.undo();
    assert_eq!(s(&b), "");
    b.redo();
    assert_eq!(s(&b), "hello");
    assert_eq!(b.cursor(), 5, "redo restores the cursors after");
}

#[test]
fn undo_type_move_type_is_two_groups() {
    let mut b = Buffer::new("");
    b.insert("ab");
    b.move_cursors(Motion::Left, false);
    b.insert("X");
    assert_eq!(s(&b), "aXb");
    assert_eq!(b.undo_depth(), 2);
    b.undo();
    assert_eq!(s(&b), "ab");
    assert_eq!(b.cursor(), 1, "undo restores the cursor the group started with");
    b.undo();
    assert_eq!(s(&b), "");
    assert!(b.undo().is_none());
}

#[test]
fn idle_signal_closes_group_and_new_edit_clears_redo() {
    let mut b = Buffer::new("");
    b.insert("a");
    b.close_group();
    b.insert("b");
    assert_eq!(b.undo_depth(), 2);
    b.undo();
    assert!(b.can_redo());
    b.insert("c");
    assert!(!b.can_redo());
    assert_eq!(s(&b), "ac");
}

#[test]
fn backspace_coalesces_and_kinds_split() {
    let mut b = Buffer::new("");
    b.insert("abcd");
    for _ in 0..3 {
        b.delete(Unit::Grapheme, Direction::Backward);
    }
    assert_eq!(s(&b), "a");
    assert_eq!(b.undo_depth(), 2, "typing, then erasing");
    b.undo();
    assert_eq!(s(&b), "abcd");
}

// ---- change events -----------------------------------------------------------------------------------

#[test]
fn change_carries_line_range_and_text() {
    let mut b = Buffer::new("l0\nl1\nl2\nl3");
    let ch = b.replace(4..7, "X\nY\nZ");
    assert_eq!(ch.len(), 1);
    let c = &ch[0];
    assert_eq!((c.first_line, c.old_last_line, c.new_last_line), (1, 2, 3));
    assert_eq!(c.text, "X\nY\nZ");
    assert_eq!((c.start_pos, c.old_end_pos, c.new_end_pos), (Pos::new(1, 1), Pos::new(2, 1), Pos::new(3, 1)));
    // replaying the changes onto a mirror reproduces the buffer (what a view does)
    let mut mirror = String::from("l0\nl1\nl2\nl3");
    let mut b2 = Buffer::new(&mirror);
    b2.set_selections(&[Selection::cursor(2), Selection::cursor(5), Selection::cursor(8)]);
    for c in b2.insert("--").into_iter().chain(b2.newline()).chain(b2.undo().unwrap()) {
        mirror.replace_range(c.start..c.old_end, &c.text);
    }
    assert_eq!(mirror, s(&b2));
}

// ---- search ------------------------------------------------------------------------------------------

#[test]
fn search_literal_case_word_wrap() {
    let b = Buffer::new("Foo foo food FOO\nbarfoo foo_bar Straße STRASSE");
    let lit = SearchOpts::default();
    let ci = SearchOpts { case_insensitive: true, whole_word: false };
    let ww = SearchOpts { case_insensitive: true, whole_word: true };
    assert_eq!(b.find_all("foo", lit), vec![4..7, 8..11, 20..23, 24..27]);
    assert_eq!(b.find_all("foo", ci).len(), 6);
    assert_eq!(b.find_all("foo", ww), vec![0..3, 4..7, 13..16]);
    assert_eq!(b.find("foo", lit, 9, Direction::Forward, false), Some(20..23));
    assert_eq!(b.find("foo", lit, 25, Direction::Forward, false), None);
    assert_eq!(b.find("foo", lit, 25, Direction::Forward, true), Some(4..7), "wraps to the top");
    assert_eq!(b.find("foo", lit, 4, Direction::Backward, false), None);
    assert_eq!(b.find("foo", lit, 4, Direction::Backward, true), Some(24..27), "wraps to the bottom");
    assert_eq!(b.find("foo", lit, 20, Direction::Backward, false), Some(8..11));
    assert_eq!(b.find_all("straße", ci).len(), 1, "ß folds to itself; STRASSE is not ß");
    assert_eq!(b.find_all("ÉTÉ", ci).len(), 0);
    assert!(b.find_all("", lit).is_empty());
    let u = Buffer::new("été ÉTÉ");
    assert_eq!(u.find_all("été", ci), vec![0..5, 6..11]);
}

// ---- viewport -----------------------------------------------------------------------------------------

#[test]
fn viewport_soft_wrap_fixed_width() {
    let fixed = |s: &str| s.chars().count() as u32;
    let b = Buffer::new("short\nthe quick brown fox jumps\n\nabcdefghijkl\nend");
    let v = b.viewport(0, 100, 10, &fixed);
    let rows: Vec<(usize, &str, bool)> = v.rows.iter().map(|r| (r.line, r.text.as_str(), r.first)).collect();
    assert_eq!(
        rows,
        vec![
            (0, "short", true),
            (1, "the quick ", true),
            (1, "brown fox ", false),
            (1, "jumps", false),
            (2, "", true),
            (3, "abcdefghij", true),
            (3, "kl", false),
            (4, "end", true),
        ]
    );
    // a window into the middle, row-limited
    let w = b.viewport(1, 2, 10, &fixed);
    assert_eq!(w.rows.len(), 2);
    assert_eq!((w.rows[1].line, w.rows[1].start, w.rows[1].end), (1, 10, 20));
    // width 0 still terminates: one grapheme per row
    let z = Buffer::new("ab\u{301}c").viewport(0, 10, 0, &fixed);
    assert_eq!(z.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["a", "b\u{301}", "c"]);
    // wide measure: CJK at 2 columns each
    let wide = |s: &str| s.chars().map(|c| if c as u32 >= 0x3000 { 2 } else { 1 }).sum::<u32>();
    let cjk = Buffer::new("日本語のテキスト").viewport(0, 10, 6, &wide);
    assert_eq!(cjk.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["日本語", "のテキ", "スト"]);
}
