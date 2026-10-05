// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Echo fixture (M4): with no key and no network the Echo provider answers the canned FAIL line with a
//! canned diff, so the whole pipe — witness → owner → prompt → answer → parse → locate → apply → record —
//! runs on the metal. The kernel's `tests selfdiag` lays [`SOURCE`] at [`PATH`] under a scratch tree, the
//! program's Echo mode answers [`answer`] for the same line.

/// The canned failing line.
pub const LINE: &str = ":: SDFIX: probe=1 -> FAIL ::";
pub const TAG: &str = "SDFIX";
/// The fixture's owner path (relative to the scratch tree root) and the line that prints it.
pub const PATH: &str = "unaos/sdfix.rs";
pub const SITE: u32 = 6;

/// The fixture source as laid.
pub const SOURCE: &str = concat!(
    "// SDFIX - the SELFDIAG fixture source.\n",
    "fn probe() -> u32 {\n",
    "    let ready = 0;\n",
    "    ready\n",
    "}\n",
    "fn witness() { let ok = probe() == 1; say(\":: SDFIX: probe={} -> {} ::\", ok); }\n",
    "fn say(_f: &str, _ok: bool) {}\n",
);

/// The fixture source after the canned diff.
pub const PATCHED: &str = concat!(
    "// SDFIX - the SELFDIAG fixture source.\n",
    "fn probe() -> u32 {\n",
    "    let ready = 1;\n",
    "    ready\n",
    "}\n",
    "fn witness() { let ok = probe() == 1; say(\":: SDFIX: probe={} -> {} ::\", ok); }\n",
    "fn say(_f: &str, _ok: bool) {}\n",
);

/// The canned answer, shaped like a model's: reasoning, then one fenced diff. Its hunk is stated one line
/// late on purpose, so the fuzz path is what the fixture proves.
pub const ANSWER: &str = "probe() returns the unset `ready`; the witness wants 1.\n\n```diff\n--- a/unaos/sdfix.rs\n+++ b/unaos/sdfix.rs\n@@ -3,4 +3,4 @@\n fn probe() -> u32 {\n-    let ready = 0;\n+    let ready = 1;\n     ready\n }\n```\n";

/// The Echo provider's answer to a FAIL line: the canned diff for the canned line, else nothing.
pub fn answer(fail_line: &str) -> Option<&'static str> {
    if fail_line.trim_end() == LINE { Some(ANSWER) } else { None }
}

/// The owners row the fixture stages.
pub const OWNERS: &str = "SDFIX\tunaos/sdfix.rs\t6\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{emit, resolve, Span};
    use crate::diff;
    extern crate std;

    #[test]
    fn canned_pipe() {
        assert_eq!(crate::owners::lookup(OWNERS, TAG), Some((PATH, SITE)));
        let p = diff::parse(diff::extract(answer(LINE).unwrap()).unwrap()).unwrap();
        assert_eq!(p.files[0].path, PATH);
        let hs = p.hunks_of(&p.files[0]);
        let mut sp = [Span::default(); 1];
        let mut sc = [0u8; 512];
        let mut src: &[u8] = SOURCE.as_bytes();
        resolve(&mut src, hs, &mut sp, &mut sc, 0).unwrap();
        let mut out = std::vec::Vec::new();
        emit(&mut src, hs, &sp, &mut sc, &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        assert_eq!(out, PATCHED.as_bytes());
    }
}
