// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `/var/log/diag.<n>.md` — what one diagnosis asked, heard and did, and (appended by the NEXT boot's
//! kernel) what that boot said about the same tags. The `fails:` line is the machine-read part.

use crate::witness::{self, Verdict};
use crate::Out;

/// The command the bench runs to rebuild from the patched tree (SH-5 owes the on-metal build).
pub const REBUILD: &str = "copy SRC/ off the card's FAT partition over a checkout (rsync -a <card>/SRC/ <checkout>/), then: cd <checkout>/unaos && UNAOS_UNAFS=1 UNAOS_SELFDIAG=1 ./arroyo esp-x86 && dd if=target/unaos-x86-card.img of=<card> bs=4M; boot, and run diag again";

/// The record's head: boot, provider, fails (space-separated tags), counts and the outcome word.
pub fn head(o: &mut Out<'_>, boot: u64, provider: &str, tags: &[&str], asked: usize, patched: usize, refused: usize, outcome: &str) {
    o.s("# diag ").dec(boot).s("\n\nprovider: ").s(provider).s("\nfails:");
    for t in tags {
        o.s(" ").s(t);
    }
    o.s("\nasked: ").dec(asked as u64).s("\npatched: ").dec(patched as u64).s("\nrefused: ").dec(refused as u64).s("\noutcome: ").s(outcome).s("\n");
}

/// A fenced section.
pub fn section(o: &mut Out<'_>, title: &str, body: &[u8]) {
    o.s("\n## ").s(title).s("\n\n```text\n").put(body);
    if !body.ends_with(b"\n") {
        o.s("\n");
    }
    o.s("```\n");
}

/// The SH-5 note.
pub fn rebuild(o: &mut Out<'_>) {
    o.s("\n## rebuild and reboot (SH-5 owed: no native toolchain)\n\n").s(REBUILD).s("\n");
}

/// Whether the next boot already filled its verdict.
pub fn has_verdict(md: &str) -> bool {
    md.contains("\n## next boot ")
}

/// The next boot's section for `md`, judged against that boot's witness text. `false` when there is nothing
/// to add (already filled, or no `fails:` line).
pub fn next_boot(md: &str, boot: u64, witness_text: &str, o: &mut Out<'_>) -> bool {
    if has_verdict(md) {
        return false;
    }
    let Some(fl) = crate::lines(md).find(|l| l.starts_with("fails:")) else { return false };
    let tags = fl["fails:".len()..].split(' ').filter(|t| !t.is_empty());
    let (mut pass, mut fail, mut n) = (0usize, 0usize, 0usize);
    o.s("\n## next boot ").dec(boot).s("\n\n");
    for t in tags {
        n += 1;
        let v = witness::last_verdict(witness_text, t);
        o.s(t).s("=").s(v.map_or("absent", |v| v.as_str())).s(" ");
        match v {
            Some(Verdict::Pass) => pass += 1,
            Some(Verdict::Fail) => fail += 1,
            _ => {}
        }
    }
    let word = if n > 0 && pass == n { "fixed" } else if fail > 0 { "still-failing" } else { "unseen" };
    o.s("\nverdict: ").s(word).s("\n");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn record_and_next() {
        let mut b = [0u8; 2048];
        let mut o = Out::new(&mut b);
        head(&mut o, 4, "echo", &["SDFIX", "AHCI"], 1, 1, 0, "patched");
        section(&mut o, "answer", b"x");
        rebuild(&mut o);
        let md = std::string::String::from_utf8(o.bytes().to_vec()).unwrap();
        assert!(md.contains("fails: SDFIX AHCI\n"));
        let mut b2 = [0u8; 256];
        let mut o2 = Out::new(&mut b2);
        assert!(next_boot(&md, 5, ":: SDFIX: probe=1 -> PASS ::\n:: AHCI: x -> FAIL ::\n", &mut o2));
        let s = core::str::from_utf8(o2.bytes()).unwrap();
        assert!(s.contains("SDFIX=PASS AHCI=FAIL"));
        assert!(s.contains("verdict: still-failing"));
        let full = std::format!("{}{}", md, s);
        let mut b3 = [0u8; 64];
        assert!(!next_boot(&full, 6, "", &mut Out::new(&mut b3)));
    }
}
