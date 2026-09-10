# BRIEF AMENDMENT 03 (16:00Z) — Peter's scenario, binding; applied as a FOLLOW-UP COMMIT on exec-orin22-bootroot
Peter: "if I hand you the microSD in the Orin to overwrite, the Orin should boot off it no different from the SD
card in the USB reader, and I should be able to leave the USB reader in, see and browse it in quarry, and not have
it tied in with the microSD boot other than it mounts the card for reading."
1. MULTI-MATCH is counted per DISK (block source), not per file. Exactly one disk → bind. Several disks match →
   root on the one the loader reported (`boot_volume_serial()` ≠ 0 and equal to one matched volume's BS_VolID);
   no loader word, or the word matches none → REFUSE `reason=multiple-kernels matches=N` (unchanged shape).
   Two copies on ONE disk = one disk (bind; note `files=2` in the witness).
2. EVERY OTHER enumerated disk carrying a FAT volume is mounted at a bus-named point WITH ITS SOURCE'S OWN WRITE
   POSTURE (`FatBackend::read_only()` ← `BlockSource::write_veto`, fat.rs:~676-700: Usb = writable (the Pi's verified
   BOT WRITE(10) path — pi 9 16:15Z: do NOT force read-only, that is a behaviour change on the Pi), TegraSd = vetoed
   in every cfg, so `/sd` on the Orin is read-only BY THE VETO, not by this mount): `/usb` (xHCI
   mass storage — on the Orin the reader card is `Global`), `/sd` (an SD/MMC controller slot: `TegraSd`, `Sdhc`).
   Nothing on a non-root disk influences root. Witness: `[vfs] disk mounted /usb source=global rw=<yes|no> ::` per mount (posture from read_only()).
3. Mutations: (m2') two disks with the kernel + loader word → binds the loader's; (m2'') two disks, loader word 0 →
   REFUSE; (m4) non-root disk present → its mount line appears and `ls /usb` (or `/sd`) lists it; (m5) `rw=` on the line equals `!read_only()` of that source (Usb yes, TegraSd no).

## AMENDMENT 03 v2 (16:25Z) — PETER: "booting dumb means booting dumb. If it sees another UnaOS disk it is home
## soil and nothing more." SUPERSEDES items 1 and 3 above.
1'. NO REFUSAL on multiple matches. The FIRST disk (in enumeration order) that carries the running kernel is root;
    every other matching disk is home soil — mounted like any other non-root disk. The witness names them all:
    `[vfs] root = boot volume serial=0x… source=… match=… matches=N home=<src:path,…> ::`. The loader serial is
    NOT used to select or to refuse (its `volume_serials` doc says it over-approximates in the REFUSING direction only;
    root does not consult it at all). `reason=multiple-kernels` is DELETED from the vocabulary.
2'. DEDUPE BY DEVICE, not by source variant: on the tegra build `publish_usb_geometry` (block.rs:705-709) writes the
    SAME device into BLOCK_DEVICE (Default) and USB_BLOCK_DEVICE (Usb). The walk and the non-root mounts key on
    device identity (num_blocks + the volume's BS_VolID, or the BlockDeviceId if one exists) so one card is never
    walked or mounted twice under two names (rmbp 16's C1 aliasing, confirmed at the declaration site).
3'. Mount-point collisions: index — `/usb`, `/usb1`, `/sd`, `/sd1`…; witness which device got which point.
Mutations replace m2'/m2'': (m2) a second disk with the kernel → root = first found, second mounted at its bus point,
`matches=2` on the wire; (m6) the aliased Default/Usb device → walked once, mounted once.
4'. (pi 9, 16:35Z, verified at 98213b7f fat.rs:678-686) `BlockSource::Default`'s write_veto is CONDITIONAL on
    FRGUARD's `default_writable()` (x86 arm) — a runtime state, not a volume property. The `rw=` field is computed
    from the SAME `read_only()` call that builds the mount (one sample), and mutation m5 asserts consistency within
    one call only; no fixed `rw=` expectation for a Default-sourced mount anywhere (spec rows included).
5'. (rmbp 16 + Peter, 16:55Z — Peter: "not making assumptions and not tying them together, possibly staining the
    testing of the newer version.") THE WINDOW MUST BE VERSION-UNIQUE: a 4 KiB .text slice can be byte-identical
    across two builds that differ elsewhere, so a newer kernel could root on an older install's disk (first-found).
    Include the build stamp in the compared bytes: `UNAOS_GIT_SHA` (arroyo:52-54 exports `git rev-parse --short=8`,
    `option_env!`-embedded — verify the site) goes into a `.text`-placed stamp (`#[link_section]`, magic + sha) that
    the window covers. Witness prints `sha=<8hex>` beside `match=`. Limit stated at the site: two DIRTY builds from
    one commit share a sha (same-commit builds must stay byte-identical for the identity gates, so no per-build nonce).
    Mutation (m7): two images from different commits on two disks → each boots to its own disk; the witness sha
    equals the image's. If UNAOS_GIT_SHA is empty in some build path, the stamp says `sha=unstamped` and the walk
    still runs on code bytes (announced, not silent).
6'. (rmbp 16, 17:00Z, taken into this arc) the `export UNAOS_GIT_SHA=` line under the `BUILD-SHA-1` marker in `arroyo` (grep the marker; it is :50 at 98213b7f and :54 on hw-rmbp — never cite the number): append a marker derived from the TRACKED diff only when
    the tree is dirty (`git diff --quiet HEAD || suffix="-d$(git diff HEAD | sha256sum | cut -c1-6)"`-shaped;
    verify `git diff` scope), so clean builds keep exact bytes (same-commit identity gates untouched; the knoboff
    guard at arroyo:~6990 needs one value per process — a suffix computed once on that line is one value) and two dirty
    states stop colliding. Limit at the site: untracked-only differences are invisible (by design — hashing
    untracked paths sweeps in build noise).

## AMENDMENT 03 v3 (19:55Z) — PETER: "what if the disk has a label? here again you are hard coding /usb0 and /usb1
## are meaningless outside the kernel." SUPERSEDES item 3' (bus-named, indexed mount points).
3''. Non-root volumes mount at `/volumes/<LABEL>` — the FAT volume label read from the volume (`FatFs::label()`,
    fat.rs), trimmed; no label → the volume serial as `%08X`; a duplicate name → numeric suffix (` 1`, ` 2`, the
    macOS shape); the witness names device and mount: `[vfs] volume mounted /volumes/UNAOS-PI source=global rw=… ::`.
    No bus, slot, or index in any path. `/`, `/boot`, `/apps` unchanged. Friend disks' UnaFS volumes: mount under
    the same tree if UnaFS carries a name; else record "unafs volume on <device>, unnamed — not mounted" in the
    witness and leave it (a stranger/friend volume is never touched). Applied as a SEAT/follow-on commit after
    FOLLOWUP reports (its brief carries the superseded bus naming; no channel into it).
    Verified at the tip: `FatFs::label()` exists (fat.rs:2105); UnaFS carries a `name: String` (unafs.rs:1014 — check
    what it names: the volume or a file) — if it is the volume name, friend UnaFS volumes mount at `/volumes/<name>`
    the same way.
    CORRECTION: unafs.rs:1014 `name` is a NativeAclRow's file name, NOT a volume name — whether UnaFS has a volume
    label is UNVERIFIED; the follow-on commit checks the superblock/declaration site before mounting friend UnaFS
    volumes by name. Peter (20:00Z): no label → a fallback is needed → the volume SERIAL (on the volume, not invented).

## AMENDMENT 03 v4 (20:10Z) — PETER: "joe user might get scared by some crazy disk name appearing if you use the
## serial… is it possible to know if there's no volume name set or a name containing illegal possibly even harmful
## volume name meant to overflow memory." SUPERSEDES 3'' where they differ.
3'''. NAME RULES for `/volumes/<name>`:
  - Source: the FAT label — an 11-byte FIXED field (BPB BS_VolLab and/or the root-dir ATTR_VOLUME_ID entry; read
    both, prefer the root-dir entry as formatters do). Copy EXACTLY 11 bytes into a fixed buffer; never trust a
    length from the medium (overflow impossible by construction — state this at the site).
  - Unnamed is a KNOWN value, not an absence: all 0x20, or `NO NAME` → mount as `/volumes/Untitled` (` 1`, ` 2`
    on collision). The serial NEVER appears in the path; it appears in the witness line only.
  - Sanitize by WHITELIST: printable ASCII in the FAT label charset (A–Z, 0–9, space, and the punctuation FAT
    permits: `! # $ % & ' ( ) - @ ^ _ ` { } ~`); every other byte → `_`; trim trailing spaces; refuse `.`/`..`/
    empty → `Untitled`. NEVER `/`, NUL, control bytes, or bytes ≥ 0x80 in a path.
  - If ANY byte was altered: witness `[vfs] volume mounted /volumes/<name> source=… rw=… label_raw=<22 hex> ::`
    so a suspicious card announces itself. Unaltered: `label_raw` omitted.
  - RED-first: a fixture volume with label bytes `..\x00/\x7f\xff…` → mounts as `/volumes/Untitled` (or the
    sanitized survivor) with `label_raw=` on the wire; the path resolver never sees a separator.
  - Same rules for a UnaFS volume name if the superblock carries one (verify at the declaration site first).
BOUNDING (pi 9, 20:50Z, replaces every "use `\b`" instruction): choose the bound by what the sibling token starts
with — `\b` only against word-char extension; `(?=\s|$)` when a sibling differs by a non-word char (hyphen,
punctuation); never a trailing space. Every new spec row ships with a negative control that MATCHES the sibling it
must exclude (executed, exit shown), not just a green run.

BOUNDING TABLE (ships with the rule; quote the table, never a summary of it):
  wire: [wc-h] rollup win=1 scope=window-band emit=6 declines=0 -> TEAR-FREE
    scope=window            (trailing space)  -> FALSE HIT   parser strips it
    scope=window\b                            -> FALSE HIT   '-' is non-word; boundary exists
    scope=window(?=\s|$)                      -> correct
    scope=window .*declines=                  -> correct
  wire: span_blocks=20480 ...
    span_blocks=2048\b                        -> correctly rejects   digit IS a word char
