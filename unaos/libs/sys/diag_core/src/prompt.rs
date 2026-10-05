// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Vein request: one system prompt and one user turn that carries the boot's FAIL lines, each paired
//! with its owner (`path:line`) and the owner's section as it stands in the selfhost tree, and asks for ONE
//! unified diff against those files. Built into a caller buffer ([`crate::Out`]); a section that would not
//! fit is cut at a line boundary and the cut is said in the text.

use crate::Out;

pub const SYSTEM: &str = "You are the self-diagnosis step of the UnaOS installer, running on the machine being brought up. \
You are shown the witness lines that FAILED on the last boot (the kernel prints `:: TAG: facts -> PASS|FAIL|SKIP ::`), \
each with the kernel source file that prints it and the section around that line. Propose the smallest change to the \
source that should make the failing lines pass on the next boot. Answer with exactly ONE unified diff in a ```diff fenced \
block: `--- a/<path>` and `+++ b/<path>` with the repository-relative paths shown, `@@ -l,n +l,n @@` hunks with 3 lines \
of unchanged context copied exactly from the section, no other files. Before the diff, at most five lines of reasoning. \
If no source change is warranted (the fault is hardware or configuration), answer `NO-PATCH:` and the reason instead.";

/// One failing line and what is known about its owner.
pub struct Item<'a> {
    pub line: &'a str,
    pub tag: &'a str,
    /// `(repo path, site line)` from the owners table.
    pub owner: Option<(&'a str, u32)>,
    /// `(text, first line number)` read from the selfhost tree.
    pub section: Option<(&'a str, u32)>,
}

/// Header of the user turn.
pub fn begin(o: &mut Out<'_>, boot: u64, fails: usize, tree: Option<&str>) {
    o.s("Boot ").dec(boot).s(" of this machine (an x86_64 Apple MacBook Pro Retina, 2012) printed ").dec(fails as u64).s(" FAIL witness line(s).\n");
    match tree {
        Some(t) => {
            o.s("The selfhost source tree is at ").s(t).s(" on the machine; the paths below are relative to the repository root.\n\n");
        }
        None => {
            o.s("The selfhost source tree is NOT extracted on this machine (`src extract` has not run), so no source is attached.\n\n");
        }
    }
}

/// One item: the line, its owner, its section with line numbers in a gutter the diff must NOT copy.
pub fn item(o: &mut Out<'_>, k: usize, it: &Item<'_>, room: usize) {
    o.s("## ").dec(k as u64 + 1).s(". ").s(it.tag).s("\n");
    o.s("```text\n").s(it.line).s("\n```\n");
    match it.owner {
        Some((p, l)) => {
            o.s("Printed by `").s(p).s("` at line ").dec(l as u64).s(".\n");
        }
        None => {
            o.s("No owner is recorded for this tag in /system/witness-owners.txt.\n");
        }
    }
    if let (Some((text, first)), Some((p, _))) = (it.section, it.owner) {
        o.s("Section of `").s(p).s("` from line ").dec(first as u64).s(" (the `NNNNN| ` gutter is not part of the file):\n```rust\n");
        let mut ln = first;
        let start = o.len();
        let mut cut = false;
        for l in crate::lines(text.strip_suffix('\n').unwrap_or(text)) {
            if o.len() - start + l.len() + 8 > room {
                cut = true;
                break;
            }
            let mut g = [b' '; 5];
            let mut v = ln;
            let mut i = 5;
            while i > 0 {
                i -= 1;
                g[i] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            o.put(&g).s("| ").s(l).s("\n");
            ln += 1;
        }
        o.s("```\n");
        if cut {
            o.s("(section cut at line ").dec(ln as u64).s(" to fit the request)\n");
        }
    }
    o.s("\n");
}

/// The ask that closes the user turn.
pub fn end(o: &mut Out<'_>) {
    o.s("Reply with one unified diff against the files above (or NO-PATCH: and the reason).\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn builds() {
        let mut b = [0u8; 4096];
        let mut o = Out::new(&mut b);
        begin(&mut o, 7, 1, Some("/boot/SRC"));
        item(&mut o, 0, &Item { line: ":: SDFIX: probe=1 -> FAIL ::", tag: "SDFIX", owner: Some(("unaos/fx.rs", 3)), section: Some(("a\nb\n", 2)) }, 1000);
        end(&mut o);
        let s = core::str::from_utf8(o.bytes()).unwrap();
        assert!(s.contains("    2| a\n    3| b\n"));
        assert!(s.contains("Printed by `unaos/fx.rs` at line 3."));
        assert!(!o.overflowed());
    }
}
