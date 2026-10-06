# APPEARANCE2 (B473) — appearance-check sees every colour spelling

**Finding (GATEREVIEW F18, plants A1).** `appearance-check.py`'s `\b0x[0-9A-F_]+\b` cannot end inside a word, so a
suffixed literal (`0x00FF_FFFFu32`) never matched; eight-digit literals counted only with alpha 00/FF; a
`from_be_bytes` word and a `const` RGB tuple were invisible. Two real misses stood: `video/text.rs:766`
(`0x00FF_FFFFu32`, the font fixture's surface) and `video/facet_anim.rs:474` (`0xFF00_0000u32`, the base canvas).

**Seam.** A host gate over the kernel's `video/`; the palette stays `video/theme.rs` (role fns + `theme::fixture`).
No knob, no new kernel file, no metal witness.

**M1.** The gate counts: hex with any integer suffix, underscore-grouped or not, six digits or eight with ANY alpha
byte (0xFFFF_xxxx stays a sentinel; masks stay masks); `u32::from_{be,le,ne}_bytes([4 int literals])` as the
word it packs; each 3/4-group of int literals in a `const`/`static` typed `(u8,u8,u8[,u8])` / `[u8; 3|4]` (tables
too). `--selftest` prints 21 plants (12 forms counted, 9 that must stay unseen); the plants also run as the control
probes on every invocation (exit 2 on a miss). Format constants (hash primes, seeds, owner tags, an IPv4 lease, a
PNG length bound, a BAR span, menu-extra kind ids) are rows in `unaos/scripts/appearance.allow` with a reason;
a stale row fails the gate. The two misses move to `theme::fixture::WHITE` and `theme::OPAQUE_BLACK`.

**Witness (host).** `python3 unaos/scripts/appearance-check.py` ->
`appearance-check: literals_outside_theme=0 files=0 certified=0 allow_rows=15 allowed_hits=15 stale=0 plants=21`
(rc 0); `--selftest` -> `plants=21 -> ok` (rc 0).

**Owed.** Scope stays `video/` (painters outside it — shell/ui consoles — are the next widening, a design
question); no `arroyo check` path runs this gate (GATEREVIEW F15, ARC GATEPATH).
