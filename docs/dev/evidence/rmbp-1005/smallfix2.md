# SMALLFIX2 — the small reds and leftovers of flights 24/25 (rmbp-ledger B391) — design

Read from `f24-boots.log` / `f25-boots.log` (awk on the tags) and the code; the row's candidate causes were checked, two were wrong.

**(1) SVG opens as text.** Wire: `[openers] TEST.SVG type=text/plain src=sniffed … handler=fileview`. The kernel
never names `pixel_core/svg`, so `pixel_core::mime_of` has no SVG arm and the text sniff wins. Seam: shared-core
(pixel_core + svg_core, the cores Aether/Facet already link). Fix: kernel feature `svg = ["pixel_core/svg"]`, knob
`UNAOS_SVG=1` (arroyo `_feats` arm + builder read + `kernel8` K8_FEATS arm); `fs/filetype.rs` gains `IMAGE_SVG`
(`image/svg+xml`, pixel_core's string) and the `svg` extension row; `fs/assoc.rs` routes it to facet (a 27th type
row, seeded as a missing object on the next boot); Quarry's kind token `svg`; the VIEW open line names
`kind=<k> handler=facet`. A build without the feature still types an `.svg` by its text (and says
`no svg renderer in this build (UNAOS_SVG arms it)` if one is typed by name). ELF growth: measured below.

**(2) QUARRY2 columns=FAIL.** Wire: `[quarry2] fixture columns FAIL: plain list cols=[Size, Modified]` (both flights).
NOT the OPENERS kind tokens nor WINDOWCAP-2: the fold that broke it is merge16's **KFONTPPI** (ec87e05d, the EDID
relatch). Flight 23 latched `ppi=0 scale=1.0`; flights 24/25 `KFONTPPI: … ppi=221 scale=2.5`. Since UIMETRICS Quarry
is physical px at the dpi scale, so the leg's fixture `geometry(1920, 1200)` — PHYSICAL pixels — became a
768x480-logical panel on which TYPE correctly degrades. Fix: the fixture is the bench panel in LOGICAL px
(`ui::px(1920) x ui::px(1200)`, small `ui::px(640) x ui::px(480)`). At 2.5 the chrome cell rounds up (23 px for
22.5) and the logical bench panel's trash list holds 84 columns where all four need 85; the Trash's own column
should be the one that survives, so the Trash ranks ORIGIN, TYPE before SIZE, MODIFIED (the plain list keeps the
pre-QUARRY2 order exactly). One line names the fixture: `[quarry2] fixture panel=<w>x<h> scale=<s> plain=… trash=…`.

**(3) BOOT80 FAIL.** The row's hypothesis (26 types cost 26 reads) is wrong: `ls` reads ONE directory, whatever
its count — flight 25 resolved the same 26-type database in `blocks_read=34` and PASSED (`users_store=nomount`).
Flight 24's 168 blocks / 77 cmds / 353 ms ran at 12:42:10 beside `PRTSCR: refused — capture in flight`: the
counters are GLOBAL, so a concurrent capture writing to UnaFS landed in the resolve's window. Fix: the type-db
leg is measured INSIDE the one UnaFS mount lock (no other UnaFS traffic can interleave) and bounded by the walk
it does: `bound = 2 x depth x ra_blocks` (an inode read and a directory read per level of `/system/types`, each at
most one read-ahead window); the users-store leg (FAT, p1) keeps B350's 64. The window's writes are said
(`foreign_wr=`): the resolve writes nothing, so a non-zero names a concurrent writer.
`:: BOOT80: … store_blocks=<n> types_blocks=<n> bound=<n> from=walk<d>x2xra<k> foreign_wr=<n> … -> PASS ::`

**(4)** `wm::close_all_furniture_except` deleted (no caller since DESKTOPBUILT); `loginfurn.rs:7` and `main.rs:6437`
named `installer_release` / `login::furniture_owed` (deleted by B387) — repointed.
**(5)** R91/R95 sweep: no removable-media citation still says R91 (ledger B376/B380/B383 and MACPARITY already say
R95; MACPARITY's R91 cites are the login-items ruling, correct). RULINGS.md carried a stray diff3 marker line
(`||||||| 3506cc47`) between R94 and R95 — removed (it is on trunk too).

## Milestones
M1 design. M2 SVG (feature, knob x3, filetype/assoc/kind, open line). M3 QUARRY2 fixture + trash rank.
M4 BOOT80 legs and derived bound. M5 dead code, comments, RULINGS marker.

## Witness (the next flight)
`[openers] TEST.SVG type=image/svg+xml src=sniffed opener=facet(db) handler=facet core=pixel_core=ok`;
Quarry double-click: `[quarry] open VIEW path=/system/test-f/TEST.SVG type=image/svg+xml kind=svg handler=facet -> facet …`;
`:: QUARRY2: columns=type,origin … -> PASS ::`; `:: BOOT80: … bound=48 from=walk3x2xra8 … -> PASS ::`.

## Owed
SVG text needs fonts (`decode_at` with the kernel's faces): this arc renders shapes only. BOOT80's FAT leg is still
global-counter measured (no FAT lock to hold). The live 2880x1800 glass at 2.5 shows Size+Type+Origin in the Trash
(no Modified) — a panel fact, not a gate.
