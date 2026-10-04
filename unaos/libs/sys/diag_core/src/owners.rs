// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The owners table, generated at build by `scripts/witness-owners.py` from every `":: TAG:` literal under
//! `unaos/crates/kernel/src` and staged as `/system/witness-owners.txt`. One row per site:
//! `TAG<TAB>repo-relative path<TAB>line`; rows of one tag are ordered with the sites whose source line
//! names `FAIL` first. `#` lines are comments.

/// The first (best) owner of `tag`: `(repo path, 1-based line)`.
pub fn lookup<'a>(table: &'a str, tag: &str) -> Option<(&'a str, u32)> {
    for l in crate::lines(table) {
        if l.starts_with('#') {
            continue;
        }
        let mut f = l.split('\t');
        let (Some(t), Some(p), Some(n)) = (f.next(), f.next(), f.next()) else { continue };
        if t == tag {
            let n = crate::parse_dec(n.trim().as_bytes())? as u32;
            return Some((p, n));
        }
    }
    None
}

/// Rows in the table (a staging sanity number).
pub fn rows(table: &str) -> usize {
    crate::lines(table).filter(|l| !l.starts_with('#') && l.split('\t').count() >= 3).count()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lookup() {
        let t = "# witness owners\nAHCI\tunaos/crates/kernel/src/drivers/ahci.rs\t120\nKVBLANK8\tunaos/a.rs\t7\nKVBLANK8\tunaos/b.rs\t9\n";
        assert_eq!(super::lookup(t, "KVBLANK8"), Some(("unaos/a.rs", 7)));
        assert_eq!(super::lookup(t, "NOPE"), None);
        assert_eq!(super::rows(t), 3);
    }
}
