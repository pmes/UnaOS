# GITCORE — UnaOS's own git (LEDGER SR59)

Branch `exec-vcs-gitcore`. Crate: `unaos/libs/vcs/git_core` (`no_std` + `alloc`, `#![forbid(unsafe_code)]`,
zero third-party crates). Face: Vaire (`handlers/vaire`) moves from `gix` 0.88 to it, and gix plus its
gix-* crates leave `Cargo.lock` (1005 lock lines removed; `cargo tree -p vaire -e normal | grep -c gix` = 0).

## Finding

Every UnaOS session lives in a git repository, and until this arc the format was handled by someone
else's implementation (DEPS SR31: `gix`, chicken wire). Nothing on the metal could read a repository.
git_core is a pure core (bytes in, bytes out) that both rings can link; the on-disk repository and the
HTTP transport sit behind the `std` and `http` features.

## What it implements (spec sections)

| module | spec | notes |
|---|---|---|
| `crypto_core::sha1` | FIPS 180-4 §6.1 | added to CRYPTOCORE for git ids; plain SHA-1 (no sha1dc detector, see ceiling) |
| `hash` | hash-function-transition | SHA-1 and SHA-256 object formats (`extensions.objectFormat`) |
| `object` | gitformat (object model) | blob/tree/commit/tag; commits and tags kept as ordered headers + message, so `gpgsig`, `mergetag`, unknown headers round-trip |
| `deflate` | RFC 1951 + RFC 1950 | **the first DEFLATE encoder in the tree**: LZ77 hash chains, lazy matching, zlib level table, each block costed as stored / fixed / dynamic and the cheapest emitted; length-limited Huffman by Kraft repair; complete codes always |
| `zlib` | RFC 1950 | inflate through pixel_core's decoder, with the consumed-byte count packs need |
| `loose` | gitrepository-layout | encode/decode/verify |
| `delta` | gitformat-pack "deltified representation" | apply + encoder (16-byte rolling-hash block index, backward extension, 64 KiB copy cap) |
| `pack` | gitformat-pack | pack v2/v3 read, idx v1/v2 read, idx v2 write, OFS/REF deltas, index-pack (thin bases from the caller), pack writer with git's delta window (kind / name-hash / size order, depth 50, window 10), multi-pack-index read (PNAM/OIDF/OIDL/OOFF/LOFF) |
| `refs` | git-check-ref-format, git-pack-refs, reflog | loose, symbolic, packed-refs with peeled lines, reflog lines, name validation |
| `config` | git-config(1) syntax | git's parser rules incl. whitespace folding, quotes, escapes, continuation, legacy `[a.b]`, includes / includeIf gitdir via a loader, typed bool/int, minimal-diff `set`/`unset` |
| `ignore` | gitignore(5), gitattributes(5) | git's `wildmatch` (incl. `**` rules, POSIX classes), list precedence, parent-exclusion, attributes with macros (`binary` built in) |
| `index` | gitformat-index | v2/v3/v4 (prefix compression), extended flags, TREE/REUC parse, every extension preserved byte-for-byte; split index refused by name |
| `diff::xdiff` | libxdiff as git ships it | a Rust **port** of xprepare/xdiffi/xhistogram/xemit (LibXDiff, LGPL-2.1-or-later; histogram from JGit, EDL-1.0 — both compatible with this crate's LGPL-3.0-or-later): line classes, trim + discard pre-pass, Myers with git's cost heuristics, `--minimal`, histogram with Myers fallback, group sliding + indent heuristic, hunk grouping, `def_ff` function names carried across hunks, git's `sane_truncate_line` on hunk headers |
| `diff` | git-diff(1), diffcore(7) | tree walk in git's order (file↔dir as delete+add), rename detection (exact → unique basenames at 75% → 4-candidate matrix at 50%, span-hash similarity), patch rendering (extended headers, unique abbreviations with auto `core.abbrev`, C-quoted paths, TAB after names with spaces, binary, type change split, submodule text), `--stat` (80 columns, `...` truncation, graph scaling, `{a => b}`). Written from observed behaviour, not from git's GPL-2.0-only diff.c/diffcore sources |
| `protocol` | gitprotocol-v2, -pack, -http, -capabilities | pkt-line, v2 advertisement, ls-refs, fetch (want/have/done, deepen, shallow-info, wanted-refs, side-band demux), receive-pack v0 push + report-status (side-band), dumb `info/refs` / `objects/info/packs` |
| `repo` (std) | gitrepository-layout | discover (`.git` dirs, `gitdir:` files, linked worktrees via `commondir`, bare), global+local config, object db with delta-base cache and alternates, refs with lock files and reflog, rev-parse (`~n ^n ^{tree} ^{commit} ^{} rev:path`, short ids), unique abbreviation, status (porcelain v1), is_dirty, checkout, add, write-tree, commit (`commit` / `commit (initial)` reflog) |
| `remote` (http) | gitprotocol-http | clone (bare / worktree / `--depth` ⇒ single-branch), fetch with one negotiation round, push with client-side fast-forward check, dumb clone — all over http_core's host transport (SR51) |

`git://` and SSH are out of scope: UnaOS speaks git over HTTP(S), which http_core carries with UnaOS's
own TLS.

## Oracle method (every number below is a run of `cargo test --release -p git_core -p vaire`)

The `git` CLI (2.43.0, on the host) is the oracle; tests shell out with a hermetic environment
(`GIT_CONFIG_NOSYSTEM`, private `HOME`, fixed identities and dates).

| test | proof |
|---|---|
| `crypto_core` sha1 | FIPS examples (abc, 448-bit, million `a`), empty blob id, streaming splits |
| `deflate` | round trip through pixel_core's inflater at all 10 levels on 8 corpora (consumed-byte count exact); each block type forced and observed; **reference zlib (python3 `zlib`) inflates 32/32 streams byte-identical**; ratio within 1% of zlib level 6 on this crate's sources (16 843 vs 16 693 bytes) |
| `loose` | objects git_core writes pass `git fsck --strict --full` silently, SHA-1 and SHA-256; commit id equals `git commit-tree`'s, tag id equals `git mktag`'s; git-written objects decode and verify; a parsed git commit rebuilt from fields is byte-identical |
| `formats` | packed-refs and reflog byte-identical round trip; 25 names agree with `git check-ref-format`; `git config --list --includes` byte-equal on an adversarial file; typed values equal `--type=bool/int`; edits read back by `git config --get`; gitignore: 41 paths, the ignored set equals `git ls-files -o -i --exclude-standard`; attributes equal `git check-attr -a` on 11 paths |
| `pack` | our packs pass `git verify-pack -v`; **`git index-pack` on our pack produces an idx byte-identical to ours**; our index-pack on git's `repack -adf --depth=50 --window=50` pack reproduces git's idx byte-for-byte; every object parses and re-serializes identically; fsck on a repo whose only pack is ours; midx lookups agree with each pack idx; **this repository's shallow history (depth 8: 3814 objects) indexed with git's idx reproduced byte-identically, every object round-tripped, re-packed by git_core, `git verify-pack` + `git fsck --full --strict` clean** |
| `diff` | **`git diff A B` and `git diff --stat A B` byte-equal on 200 first-parent commit pairs of this repository: 400/400 (44.9 MB of git output)**; `--histogram` 100/100 on 50 pairs; a corners fixture (exact / basename / matrix renames, binary, mode change, file→symlink, quoted UTF-8 path, path with space, no newline at EOF, empty files) byte-equal |
| `worktree` | index v2, v3 (skip-worktree), v4, v4+UNTR byte-identical round trip and equal to `git ls-files -s`; TREE root equals `HEAD^{tree}`; status equals `git status --porcelain` (10 entries: M, MM, D, A, mode change, untracked dirs collapsed, ignored dirs hidden); checkout leaves `git status` empty; write-tree equals `git write-tree`; **a commit made by git_core has the same id as `git commit`'s, and HEAD and branch reflogs are byte-identical**; rev-parse forms agree |
| `http` | against `git http-backend` (tests/http_server.py): **bare clone — packed-refs, HEAD, object set equal to `git clone --bare`, and the received pack bytes identical**; `--depth 1` clone object set and shallow file equal; worktree clone is clean under `git status`; fetch negotiates (3 new objects, not the history); push of two refs reports `ok`, server fsck clean; non-fast-forward refused by the client, and by the server (`receive.denyNonFastForwards`, forced) reported as `ng`; **this repository (depth 50) cloned over HTTP: 9901 objects, 31.6 MB pack byte-identical to git's clone, packed-refs/HEAD equal, shallow boundary equal as a set**; dumb clone of a half-packed, half-loose repository fsck-clean with refs equal |
| `fuzz` | 21 500 mutants (packs, idx, index v2/v4, config, deltas, loose, objects, protocol responses; half of the checksummed formats re-checksummed so mutations reach the parsers) — 0 panics; at `GITCORE_FUZZ_SCALE=20`, 430 000 mutants — 0 panics. Two real bugs found and fixed: an unbounded preallocation from a pack's object count, a v4 index varint running into the trailer |
| vaire | its full suite (status, dirty, detached, repo bolts, ledger chain) green on git_core |

KAT / oracle counts: SHA-1 5 FIPS/known vectors; deflate 80 level round trips + 18 forced blocks + 32
reference-zlib streams; 400 + 100 + 2 diff outputs; 4 index forms; ~3.8k + 87×2 pack objects; 9901
objects over HTTP.

## Fixtures

None committed: every fixture is built at test time by the git CLI or taken from this repository's own
history (a local clone of itself). Nothing over 200 KB is in git; `vectors.txt` is not needed.

## Third-party crates

None in git_core (dependencies: crypto_core, pixel_core, http_core — all UnaOS). Vaire now links
git_core + crypto_core instead of gix; it keeps `anyhow`, `serde`, `toml` (utilities, unchanged) and
`tempfile` (dev). No crate was added by this arc.

## Ceiling (what is NOT implemented)

- SHA-1 collision DETECTION (git's sha1dc). Digests are identical for every non-crafted input; a
  SHAttered-style block is hashed, not flagged. Vaire's `COLLISION_MARKER` is now a sentinel only.
- Userdiff drivers (`diff=<driver>` funcname regexes, textconv), `--word-diff`, `-C` copies, break
  detection, `--color`; whitespace-ignoring options; the `diff.statNameWidth`/`statGraphWidth` knobs.
- `git diff` against the worktree/index (only tree↔tree is rendered; status covers worktree state).
- Status: no rename detection in status, submodule (gitlink) worktree state treated clean, filters
  (`text`/`eol`/`filter` attributes, autocrlf) not applied when hashing worktree files.
- Index: split index (`link`) refused; sparse-index directory entries untested; `index.skipHash` read only.
- Config: `includeIf onbranch:` / `hasconfig:` not evaluated; system config read only when present.
- Packs: pack v3 objects beyond v2 semantics, `.bitmap`/`.rev`/commit-graph not read or written;
  multi-pack-index write; incremental midx chains.
- Protocol: fetch negotiation is one round (ACK/ready then `done`), no `filter` (partial clone),
  no packfile-uris, no push options/atomic; thin packs are not requested (and refused if sent).
- Dates for commit identities: raw `<secs> <tz>` / `@secs` only (git's approxidate is not ported);
  without GIT_*_DATE the offset is +0000.
- The kernel link (a checkout on UnaFS) is the follow-on the row names.

## What stays owed

1. sha1dc (Stevens–Shumow) in crypto_core, proven on the SHAttered PDFs (fetched, sha-pinned).
2. The kernel path: git_core is `no_std` already; a UnaFS-backed `ObjectSource` + checkout in ring 0.
3. Userdiff drivers and worktree diffs for Gneiss's forge views.
4. commit-graph / bitmaps for large-history performance; multi-round negotiation.

## Continuing

`cargo test --release -p git_core` runs everything (needs `git` and `python3`; the self-history tests
skip if the clone fails). Knobs: `GITCORE_SELF_DEPTH` (pack round trip), `GITCORE_DIFF_PAIRS`,
`GITCORE_FUZZ_SCALE`. The xdiff port mirrors libxdiff function by function (names kept), so a change
in git's xdiff can be followed line by line.

## Handler charter

No new handler: git_core is a shared core under **Vaire** (CODEX §2, the Loom: repositories, mirrors,
dev trees), which already owns repositories and their bus surface (`GetDiff`).
