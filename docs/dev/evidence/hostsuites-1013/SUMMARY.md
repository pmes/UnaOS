# HOSTSUITES — `cargo test --workspace --exclude aether` on 441d7e45 (rmbp host, 2026-10-13)

Executor `exec-rmbp-local-hostsuites`, base `441d7e45d`. Host: Linux 7.2.9 Fedora 44 x86_64, uid 1000, git 2.53.0,
no `pkg-config`. `CARGO_TARGET_DIR` outside the repo (scratch). Aether's tests are SUITES' lane and were not run.
Raw excerpts: [`cargo-test-excerpts.txt`](cargo-test-excerpts.txt) (about 105 KB; the full logs stay in the scratch dir).

## Verdict

**The workspace test as briefed did NOT run a single test.** Pass 1 (`cargo test --workspace --exclude aether
--no-fail-fast`) stopped at compile, rc=101:

    error: failed to run custom build command for `alsa-sys v0.4.0`
      The pkg-config command could not be found.

The host has no `pkg-config` and no `.pc` files, so every crate whose dependency graph reaches `pkg-config`
(`cargo tree --workspace -i pkg-config`) cannot build: **aether-shell, aethervideo-check, euclase, lumen, phonolite,
play-check, pulse, quartzite, resonance, stria, una, vein** (gtk4 / libadwaita / cairo / alsa through system-deps).
Those twelve are UNTESTED here, not green (SR113).

Pass 2 excluded exactly those twelve (plus aether) and ran to completion (rc=101 from test failures only; no
compile error; `test262_run` alone took 693 s):

| | count |
|---|---|
| test binaries + doc-test groups with a result line | 324 |
| passed | 1737 |
| **failed** | **10** (8 targets) |
| ignored | 4 |

After the two in-arc fixes, **4 of the 10 are green on re-run; 6 stay red.**

## Per crate (pass 2): result lines · passed · failed · ignored

| crate | results | passed | failed | ignored |
|---|---|---|---|---|
| amber_bytes | 5 | 12 | 0 | 0 |
| amber_core | 3 | 8 | 0 | 0 |
| audio-check | 1 | 0 | 0 | 0 |
| audio_core | 16 | 47 | 0 | 1 |
| aule | 2 | 3 | 0 | 0 |
| av1-check | 1 | 0 | 0 | 0 |
| av1_core | 5 | 42 | 0 | 0 |
| bandy | 3 | 171 | 0 | 0 |
| comscan | 3 | 9 | 0 | 0 |
| crypto-check | 1 | 0 | 0 | 0 |
| crypto_core | 3 | 20 | 0 | 0 |
| css_core | 6 | 13 | 0 | 0 |
| demux_core | 9 | 27 | 0 | 0 |
| diag_core | 2 | 10 | 0 | 0 |
| edit_core | 5 | 30 | 0 | 0 |
| elessar | 2 | 1 | 0 | 0 |
| elf_core | 2 | 6 | 0 | 0 |
| exfat_core | 3 | 11 | 0 | 0 |
| eyes | 1 | 0 | 0 | 0 |
| facet | 5 | 31 | 0 | 0 |
| facet-view | 3 | 3 | 0 | 0 |
| facet_view | 1 | 0 | 0 | 0 |
| font_core | 16 | 48 | 3 | 0 |
| foreman | 5 | 42 | 0 | 0 |
| git_core | 10 | 33 | 1 | 0 |
| gneiss_pal | 4 | 100 | 0 | 0 |
| helm | 3 | 13 | 0 | 0 |
| holocron | 5 | 9 | 0 | 0 |
| holocron-cli | 2 | 1 | 0 | 0 |
| holocron_core | 10 | 43 | 0 | 0 |
| html_core | 8 | 21 | 0 | 0 |
| http_core | 7 | 39 | 2 | 0 |
| ibus | 3 | 12 | 0 | 0 |
| infer_core | 6 | 27 | 0 | 0 |
| jobs_core | 3 | 9 | 0 | 0 |
| js_core | 5 | 8 | 0 | 0 |
| kvstore_core | 2 | 8 | 0 | 0 |
| layout_core | 2 | 0 | 0 | 0 |
| ldso_core | 3 | 8 | 0 | 0 |
| lux | 3 | 9 | 0 | 0 |
| matrix | 6 | 57 | 0 | 0 |
| matrix_core | 2 | 0 | 0 | 0 |
| mica | 5 | 4 | 0 | 0 |
| midden | 2 | 3 | 0 | 0 |
| midden_core | 2 | 34 | 0 | 0 |
| pca9685 | 4 | 17 | 0 | 0 |
| pixel-check | 1 | 0 | 0 | 0 |
| pixel_core | 13 | 34 | 0 | 0 |
| prefs_core | 5 | 59 | 0 | 0 |
| principia | 2 | 25 | 0 | 0 |
| probe_core | 3 | 13 | 0 | 0 |
| psl_core | 2 | 2 | 0 | 0 |
| raw_core | 3 | 6 | 0 | 0 |
| sentinel | 1 | 0 | 0 | 0 |
| sonde | 3 | 7 | 0 | 0 |
| svg_core | 6 | 46 | 0 | 1 |
| syntax_core | 3 | 11 | 3 | 0 |
| tabula | 3 | 48 | 0 | 0 |
| text_host | 2 | 11 | 1 | 0 |
| tls_core | 13 | 51 | 0 | 1 |
| trash_core | 3 | 8 | 0 | 0 |
| type_core | 3 | 8 | 0 | 0 |
| una-abi | 1 | 15 | 0 | 0 |
| una-card | 2 | 1 | 0 | 0 |
| una-res | 1 | 2 | 0 | 0 |
| una_abi | 1 | 0 | 0 | 0 |
| unafs | 34 | 239 | 0 | 1 |
| unafs-cli | 2 | 3 | 0 | 0 |
| unafs_bench | 1 | 0 | 0 | 0 |
| usbnet_core | 2 | 13 | 0 | 0 |
| vaire | 3 | 64 | 0 | 0 |
| vein_core | 2 | 33 | 0 | 0 |
| vein_ring3 | 3 | 13 | 0 | 0 |
| vertex | 1 | 1 | 0 | 0 |
| vp8_core | 3 | 25 | 0 | 0 |
| wifi_core | 2 | 10 | 0 | 0 |

(Crates whose only result lines are 0-test doc-test groups show 0 passed; `facet-view`/`facet_view` and
`una-abi`/`una_abi` are one crate's test binaries and its doc-tests.)

## Reds (first failing assertion verbatim) and ledger ids

| ledger | crate · test | first failing assertion | disposition |
|---|---|---|---|
| SR106 | font_core · `kat::tables_match_fonttools` | `no KAT font available on this host` | open — host DejaVu sha differs from the KAT source |
| SR106 | font_core · `name::name_records_match_fonttools` | `no font of the KAT set is present` | open — same cause |
| SR106 | font_core · `raster::coverage_integrates_to_the_outline_area` | `assertion failed: n > 0` | open — same cause |
| SR107 | git_core · `formats::config_matches_git_list` | `assertion \`left == right\` failed` (ours `core.spaced=a   b  c`, git 2.53 `core.spaced=a   b\t\tc`) | open — which git git_core tracks is the arc's call |
| SR108 | http_core · `cache::tests::cache_304_freshen_merges_fields` | `left: Some("W/\"v1\"")` / `right: Some("\"v1\"")` | **fixed** `868e62c1e` (the test, per RFC 9111 §3.2) |
| SR109 | http_core · `url_wpt::wpt_urltestdata` | `WPT urltestdata regressed: 888/896 < 896` | open — URLCORE's UTS #46 refuses 8 `xn--` hosts |
| SR110 | syntax_core · `kat::kat_c` | `c: token 39 differs` (`left: "9 char ¦L'é'¦"`, `right: "9 char ¦L'¦"`) | **fixed** `37858f9db` |
| SR110 | syntax_core · `kat::kat_toml` | `toml: token 6 differs` (`left: "1 escape ¦\\\"¦"`, `right: "1 escape ¦\\\"é¦"`) | **fixed** `37858f9db` |
| SR110 | syntax_core · `kat::incremental_equals_fresh_under_a_seeded_edit_stream` | `end byte index 23 is not a char boundary; it is inside 'é' (bytes 22..24 of string)` | **fixed** `37858f9db` |
| SR111 | text_host · `db::tests::resolution_and_fallback_vs_fontconfig` | `family order of the first 6 fc-match -s sans-serif faces` (left/right in the row) | open — host font set |
| SR112 | matrix · `finder::write_to_readonly_dir_surfaces_loud_denial` (the known root-user red) | as root: `assert!(matches!(out, FsOutcome::Denied { .. }), "got {out:?}")` | passed here as uid 1000; tip carries the skip-as-root guard (`9e3ae90e0`), which is vacuous as root |
| SR113 | the 12 pkg-config crates | `failed to run custom build command for \`alsa-sys v0.4.0\`` | open — host lacks dev packages; untested |

(¦ stands for the KAT lexeme delimiter, a pipe.)

## Fixes

1. **syntax_core `37858f9db`** — the C and TOML KAT fixtures carried a literal `é` where their `.spans` expect an
   escape token: the `\u00e9` escape had been decoded on its way to disk, in the fixture and the expected spans
   alike. Restored `\u00e9` in `tests/kat/{c,toml}.{txt,spans}` (4 lines). Re-run `cargo test -p syntax_core
   --test kat`: 14 passed, 0 failed. **Mechanism observed in this arc:** the agent harness's Write tool turned a
   typed `\u00e9` into a literal `é` in the first draft of this arc's own ledger row. A fixture written through
   it loses its escapes silently; check any `\uXXXX` in an agent-written file with `grep -c` after the write.
2. **http_core `868e62c1e`** — `cache_304_freshen_merges_fields`'s last assertion expected the pre-304 strong
   ETag after `Entry::freshen`, which (its doc comment, RFC 9111 §3.2) replaces every field the 304 carries but
   the stored body's framing and codings. The assertion now expects `W/"v1"`. Re-run of that one test: ok.
   Go-red: the same test on 441d7e45 (pass 2) is the red.

## Not fixed, and why

- SR107 (git_core) is a few lines, but the oracle is whatever `git` the runner has; matching 2.53 reds a runner
  with an older git. Owed: the decision of which git, and a version guard in the test.
- SR109 (URL) is UTS #46 / URL Standard semantics in `html_core::url::idna`, folded the same day by URLCORE,
  whose row says "proof owed to the fold" — this is that proof. Owner's fix.
- SR106, SR111 are facts about the host's font packages; SR113 is the host's missing dev packages.

## Side effect noted

The js_core, audio, infer_core and media tests fetch vectors into the REPO-relative `target/` (not
`CARGO_TARGET_DIR`): `target/test262`, `target/audiotrack-vectors`, `target/infer_core-vectors`,
`target/media-vectors` appeared in the worktree (gitignored). `test262` printed
`fatal: cannot mkdir …/target/test262: File exists` and still passed.
