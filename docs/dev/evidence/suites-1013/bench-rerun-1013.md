# Bench re-run after the toolbox gained pkg-config, ALSA, GTK 4, libadwaita, node 22 and Playwright's Chromium (2026-10-13, seat)

Tree: hw-rmbp@0d88e0c9 (image 27b's tip). Packages: `pkg-config libasound2-dev libgtk-4-dev libadwaita-1-dev nodejs npm` (Ubuntu 26.04 toolbox `ubuntu-box`); the apt `chromium` is a snap stub that cannot launch in a toolbox — Playwright's own Chromium (`playwright install chromium`, build 1248) under `~/unaos-bench/scratch/rmbp-0915/local1/pw-browsers`, Playwright under `…/local1/node-root/lib/node_modules` (`NODE_GLOBAL_ROOT`), `AETHER_REF_CHROMIUM` pointed at it.

## The twelve crates HOSTSUITES could not build (SR113): ALL GREEN
`cargo test --no-fail-fast -p aether-shell -p aethervideo-check -p euclase -p lumen -p phonolite -p play-check -p pulse -p quartzite -p resonance -p stria -p una -p vein` → rc=0. aether-shell 3, aethervideo-check 1 (oracle), euclase 23, phonolite 2, play-check 1+1+2 (the audio oracle 12.4 s), pulse 1, quartzite 12, resonance 16+3+2, stria 8+6+11, una 5, vein 25; lumen 0 tests. SR113 closes: untested → green. (A first pass with `--workspace -p …` stopped at aether's six known reds — SR111–SR116 — because cargo fails fast; those are unchanged.)

## EYES against a real Chromium oracle (first scored run on this bench)
- **facet: GREEN** — 22/22 cases (11 golden + 11 @chromium) ssim ≥ 0.995, mismatch 0.0 %.
- **aether: RED — 5 of 25 regressed past the baseline** (ssim_drop 0.02): `01-blog-article 0.994 → 0.847`, `06-table-zebra 0.994 → 0.968`, `08-inline-wrap 0.995 → 0.959`, `13-ua-defaults 0.997 → 0.969`, `15-landing 0.996 → 0.960`. The other 20 hold (07-images-svg 0.929 and 05-form 0.970 were already under their baselines' neighbours; 16/18/19/21 at 1.000). These are LAYOUTCORE/AETHERSTYLE-era renderer regressions against the frozen baseline; the blog article is the worst by far. Logs: `~/unaos-bench/scratch/rmbp-0915/local1/eyes2-{aether,facet}.log`; SCORE.md under `tools/eyes/out/<suite>/` (untracked).
A ledger row is owed for the five (the cloud assigns the id — the bench's SR ids collided across four branches this wave).
