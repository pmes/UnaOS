// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Witness lines: the tree's verdict grammar is `:: TAG: <facts> -> PASS|FAIL|SKIP ::` (the arrow and the
//! closing `::` may be followed by nothing else). `:: BOOT: … ::` is the boot's one measurement (QUIETBOOT).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Skip,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Skip => "SKIP",
        }
    }
}

/// The verdict of a witness line, or `None` when it is not one. The LAST `-> ` decides.
pub fn verdict(line: &str) -> Option<Verdict> {
    let l = line.trim_end();
    if !l.starts_with(":: ") {
        return None;
    }
    let i = l.rfind("-> ")?;
    let w = &l[i + 3..];
    let w = w.split(|c: char| c == ' ' || c == ':').next().unwrap_or("");
    match w {
        "PASS" => Some(Verdict::Pass),
        "FAIL" => Some(Verdict::Fail),
        "SKIP" => Some(Verdict::Skip),
        _ => None,
    }
}

/// The boot line.
pub fn is_boot(line: &str) -> bool {
    line.starts_with(":: BOOT:")
}

/// Whether the boot log keeps this line.
pub fn keep(line: &str) -> bool {
    is_boot(line) || verdict(line).is_some()
}

/// The line's tag (`:: TAG:` → `TAG`): 1..=32 bytes of `[A-Za-z0-9_-]`.
pub fn tag(line: &str) -> Option<&str> {
    let r = line.strip_prefix(":: ")?;
    let end = r.find(':')?;
    let t = &r[..end];
    if t.is_empty() || t.len() > 32 || !t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return None;
    }
    Some(t)
}

/// The FAIL lines of a witness file (comment lines `#` skipped).
pub fn fails(text: &str) -> impl Iterator<Item = &str> {
    crate::lines(text).filter(|l| !l.starts_with('#') && verdict(l) == Some(Verdict::Fail))
}

/// The verdict of the LAST line in `text` that carries `tag`.
pub fn last_verdict(text: &str, t: &str) -> Option<Verdict> {
    let mut v = None;
    for l in crate::lines(text) {
        if tag(l) == Some(t) {
            if let Some(x) = verdict(l) {
                v = Some(x);
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar() {
        assert_eq!(verdict(":: KVBLANK8: lost_at=3 -> FAIL ::"), Some(Verdict::Fail));
        assert_eq!(verdict(":: AHCI: port=0 -> PASS ::"), Some(Verdict::Pass));
        assert_eq!(verdict(":: EXECNAME: SKIP (no program source mounted at /apps) ::"), None);
        assert_eq!(verdict(":: X: a -> b -> SKIP ::"), Some(Verdict::Skip));
        assert_eq!(verdict("[x] -> FAIL"), None);
        assert!(keep(":: BOOT: firmware->loader=1 ms loader->desktop=2 ms total=3 lines=4 ::"));
        assert_eq!(tag(":: GEN7R8: verdict=x -> FAIL ::"), Some("GEN7R8"));
        assert_eq!(tag(":: [fatverb] ls -> NO VOLUME ::"), None);
        let t = ":: A: -> PASS ::\n:: B: x -> FAIL ::\n# :: C: -> FAIL ::\n:: A: again -> FAIL ::\n";
        assert_eq!(fails(t).count(), 2);
        assert_eq!(last_verdict(t, "A"), Some(Verdict::Fail));
        assert_eq!(last_verdict(t, "Z"), None);
    }
}
