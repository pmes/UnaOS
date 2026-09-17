# Executor-branch census, 2026-09-17 (rmbp seat) — which exec-* branches reached NO track branch

Trigger: FONTS2X (ad62cf09, exec-orin-fonts2x, 2026-09-13) — the shared font blit Quarry needs — sits on its executor branch only; Peter: 'fonts are not arch specific … how much else is lost'.

Method (seat, git only, no judgement per branch yet):
1. every refs/heads/exec-* and refs/remotes/origin/exec-* (526 refs, deduped by name);
2. UNREACHED = tip is an ancestor of none of main, hw-rmbp, hw-jetson, hw-pi4 (local == origin for all four at census time);
3. for each UNREACHED branch: DOCS-ONLY if its diff vs merge-base(main) touches no unaos/**/*.rs, arroyo, builder, *.toml; else CODE with probe=k/n = how many of up to 12 distinctive added code lines (>=50 chars) exist verbatim in ANY track branch's current tree of the same files.
A LOW probe is a SCREEN, not a verdict: a seat that folded a diff by hand rewrote lines (exec-quarry probes 3/12 yet QUARRY is on main as a165798a). A HIGH probe is strong evidence the content landed.

Result: 206 UNREACHED of ~430 unique exec-* branches; 28 DOCS-ONLY, 178 CODE; 30 CODE branches probe below 50% (listed in UNREACHED-LOW-PROBE.txt) — these need a per-branch verdict: FOLDED-BY-HAND (cite the fold sha) / LOST (what it carries, which track owns it) / WIP-SURVIVOR / SUPERSEDED.
Proven LOST so far: FONTS2X ad62cf09 — on no track by ancestry, and its video/font.rs surface blit is absent from every track's font.rs; being carried onto hw-rmbp by QUARRYCLICK 2026-09-17. LOGINBOOT f14f6505 and BOOT-FOCUS fdcba15d are the next two to verify (code, no fold named in any track log).
