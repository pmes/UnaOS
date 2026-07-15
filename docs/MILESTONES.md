# UnaOS milestones

A running, quick-to-digest log of what landed each integration round — one
entry per arc, newest first. Each entry: **what it does**, **how it was tested**
(QEMU + metal), and the commit. Deep detail lives in the per-subsystem docs
under [`dev/OS/`](dev/OS); the ledger of hardening state is in
[`SECURITY.md`](SECURITY.md); direction is [`ROADMAP.md`](ROADMAP.md).

Legend: **✅ metal-confirmed** · **🔬 QEMU-green, metal pending** · dates ISO.

---

## aarch64 SMP — CORE3-FIX (re-derive secondary core id from MPIDR_EL1, MMU-on) — 2026-07-15 ✅✅ METAL-CONFIRMED `hw-pi4`

**What it does:** closes the CORE3-SMP regression — on Pi 4 metal, a `kernel8.img` crossing 1 MiB
brought core 3 up as a phantom "core 0" (id 3 → 0), deterministically, so `CORE_READY[3]` never set
and the CAPSTONE pair failed (30/32). The 2026-07-15 probe bench proved delivery correct (`[03E2X0]`:
MPIDR reads 3 at EL2) and the corruption kernel-side. Disassembly then pinned the mechanism: the
compiler spilled the `core_raw` entry argument to the **stack with the MMU off** (Device/non-cacheable
→ DRAM), and `__secondary_rust` reloaded it **cacheable after `enable_mmu`** for the print, the
`CORE_READY` index, and `wait_and_run` — hitting a stale (zero) L2 line seeded by the BSP's cacheable
run over `SECONDARY_STACKS`. A mismatched-attributes coherency hazard, QEMU-invisible (no cache model),
image-size-deterministic (1 MiB = BCM2711 L2 size, layout-dependent stale-line residency). **The fix
(`crates/kernel/src/arch/aarch64/smp.rs`):** ignore the advisory argument and re-derive the id from
`MPIDR_EL1` *after* `drop_to_el1()` + `enable_mmu()` (`mrs`, `& 0xff`, bounds-check, park on garbage) —
every store/load of the id is now cacheable-coherent; the stale window is deleted, not patched. Asm
stubs unchanged (x0 advisory); virt/Tegra paths out of lane, analogous pattern flagged upward.

**How it was tested:** 🔬 QEMU-green — `./arroyo check` both arches, `kernel8` clean, `kernel8-test`
byte-equivalent (41 PASS, CAPSTONE 6/6 on APs [1,2,3], K3-mount `[w=0x1ff]` + K4-write `[w=0x7f]` +
F2/F3 locked 240000/240000, 0 forbidden), `test-arm` MISSION SUCCESS. Pre- and post-fix disassembly
recorded in `arch_arm64.md §CORE3-SMP FIX` (mrs now after the `SCTLR_EL1` write; advisory x0 no longer
spilled). QEMU brings up 4/4 at every size and can never reproduce the fault — the real verdict was
always metal. **✅✅ METAL-CONFIRMED (2026-07-15 attended bench, Peter physical, boot 1):** a >1 MiB
build (712,464 B) brought **all four cores online with correct ids — no phantom "core 0" — and ran
CAPSTONE 6/6 COMPLETE (workers on cores 2+3), the first full-core boot in the failing regime since
the regression.** Same boot, riders captured: **K3-revoke `[w=0x7f]`** (two-phase durable-first
revoke ordering on the REAL card — previously QEMU-only) + **K5-lockspan `[w=0x3f]`**; plus F2/F3
locked 240000/240000 under true 4-core parallelism, K3-mount `[w=0x1ff]`, K4-write `[w=0x7f]`,
0 forbidden. Stale-fixture caveat: U9/U10/U10-create/U11/U6-grants showed the documented stale-card
signature (probe bench's un-re-prepped card) — NOT regressions; the strict pristine-card 32/32 line
rides the next Pi sitting. Detail: `arch_arm64.md §CORE3-SMP` METAL-CONFIRMED paragraph.

---

## unafs — UNAFS-F1 (dirty-mount recovery: fsck-scavenger + `recover`) — 2026-07-13 🔬 `us-unafs-f1`

**What it does:** gives `unaos/libs/fs/unafs` a real crash-recovery pass for the residue the F2 mutation
engine is *documented* to leave on a program-order power cut. The mutations are crash-**ordered**
(leak-not-dangle), so a crash leaves only two bounded, structurally sound residues: **leaked
blocks** (allocated in the bitmap, reachable from no inode) and **query-orphans** (the one
cross-directory `rename` window where an inode is reachable by the attribute catalog but by no
name). F1 adds a mark-and-sweep scavenger (`fsck.rs`, `UnaFS::fsck`) that walks the volume from
its roots (system blocks + the name tree from the root inode + the catalog, cycle-guarded and
bounded to the volume span, reusing BEFS-HARDEN's `checked_*`/`div_ceil`-clamp extent discipline),
diffs the reachable set against the allocation bitmap, and — in repair mode — heals query-orphans
(scrubbing their catalog entries through the same crash-ordered rewrite path so no query dangles,
then a re-walk sweeps the freed blocks) and returns every leak to the free pool. `UnaFS::recover`
is the host-side dirty-mount entry point: run the scavenger in repair mode, then reset a dirty
journal to clear the flag. Exposed on the CLI as `unafs fsck [--repair]`. **Honest boundary
(carries the F2 fold):** the WAL carries no redo/undo block images, so this is *reconciliation*,
not log replay — and it is best-effort under program-order writes (no write barriers; a reordering
write-back cache can still dangle a pointer — a future arc). Zero on-disk format change; the golden
KATs are byte-identical; the kernel's read-only K3 mount never calls the new paths.

**How it was tested:** 5 new recovery KATs (`unaos/libs/fs/unafs/tests/recovery_logic.rs`) craft each
documented crash window from the crate's public surface (a bare leaked block; an unhooked inode +
its extents with no name/no catalog; a cross-dir `rename` query-orphan; a leaked block + torn
journal) and assert exact leak reclamation, orphan heal, dirty-flag clear, free-space round-trip,
and a clean remount — plus a "never eat live data" control (a healthy nested volume scans clean and
repair frees nothing). `cargo test -p unafs` 70 green with the format KATs and hostile-volume
fixtures byte-identical; `no_std` check clean; `./arroyo check` green both arches; `kernel8-test`
PASS-count byte-equivalent (23/23, trusted K3 fixture still `w=0x1ff`). Metal: none needed
(host-native lib arc; the kernel consumes it unchanged).

---

## Handlers — AMBER-CHARTER (the eviction: engram vault back to vein, amber_bytes back to The Block) — 2026-07-13 🔬 `us-amber`

**QEMU-green n/a (host-native Ring 3 handlers; gates are `cargo test -p vein` + `cargo check -p lumen`).**
Undoes the March charter derail (`3839cff`, Jules), which had extracted vein's durable-memory
`DiskManager`/vault actor and bolted it onto `amber_bytes`, contradicting that handler's `docs/CODEX.md`
charter ("The Block": forensic disk/partition recovery, explicitly *not* a durable-memory service).

- **The eviction (M1):** the whole Semantic Vault actor — `DiskManager` (UnaFS engram store:
  `save_memory` / `search_memories` / `get_latest_engrams` / `load_paged_memories`) and the `ignite`
  storage-actor loop over `StorageSave`/`StorageQuery`/`StorageLoadPaged` — moved from
  `handlers/amber_bytes/src/lib.rs` to a new `handlers/vein/src/vault.rs` (`vein::vault`), its true
  home. The bandy wire shapes (the `SMessage` storage variants, `DispatchRecord`, `Origin`) were
  consumed unchanged — no ontology re-cut; the 56 bandy KATs are byte-identical.
- **AMBER-GUARD moved intact:** the fail-closed mount guard (an existing vault that cannot be
  mounted is left byte-identical on disk — never truncated, never reformatted) and its four
  byte-identity tests moved with the actor and stay green at their new home.
- **amber_bytes returns to The Block (M2):** the crate is now bin-only — the forensic CLI
  (`inspect`/`image`/`search`/`extract`/`wipe`, which survived the drift on-charter) is its sole
  surface. Its `unafs`/`bandy`/`anyhow`/`tokio` dependencies (actor-only) were dropped; README
  restored to the forensic-recovery charter.
- **Consumer rewire:** Lumen's boot (`vessels/lumen/src/main.rs`) now ignites `vein::vault::ignite`
  instead of `amber_bytes::ignite`; the `amber_bytes` path dependency was removed from Lumen. The
  live durable-memory seam (Lumen → the vault serving engrams) is preserved.

**How it was tested:** `cargo test -p vein vault` → 4/4 (the moved AMBER-GUARD byte-identity suite:
fresh-create, corrupt fail-closed, sub-block fail-closed, valid reopen); `cargo test -p amber_bytes`
builds bin-only clean; `cargo check -p lumen` green; `cargo test -p bandy` 60/60 with the 56 KATs
byte-identical (bandy untouched — `git status libs/bandy` empty). Metal: none needed (host-native
Ring 3 arc). Note: vein currently builds on the macOS host (no elessar GTK block encountered).

---

## unafs — BEFS-HARDEN (bound the on-disk parser against hostile/corrupt volumes) — 2026-07-13 🔬 `us-befs`

**What it does:** closes the DoS class the K3 security-tier review confirmed (and the QSIM panel
re-raised for the future in-kernel query path): every on-disk-derived length/count/offset in
`unaos/libs/fs/unafs` previously flowed into an allocation or loop untrusted, so a physically swapped or
corrupted card could panic/OOM-abort the kernel at mount (`bitmap_blocks` → `with_capacity`),
at `ls`/`read` (`inode.size` → capacity overflow; unchecked extent arithmetic), at query time
(catalog/spilled-extent sizes), or inside bincode itself (a crafted `String`/`Vec<u8>` length
prefix pre-allocates from the CLAIMED length before reading a byte — confirmed in bincode 2's
`impl_alloc.rs`, an infallible `vec![0u8; len]`). Now: `Superblock::validate` bounds all geometry
at the parse boundary (exact bitmap-size consistency, journal layout pinned to the WAL constants,
in-bounds root/catalog ids, representable volume span); `SpaceMap::load` and `read_from_extents`
allocate via `try_reserve` (graceful `Err::AllocRefused`); extent arithmetic is `checked_*` with
past-volume targets rejected; hole fills are bounded bulk runs (not O(size) byte-push);
`free_extents` clamps its walk to the volume; spilled-attribute extent sums are overflow-checked on
the query and `get_attribute` paths; and the codec seam decodes under two byte budgets
(`deserialize_block` 8 KiB for superblock/inode/journal-op; `deserialize` 64 MiB ceiling for
extent-backed records). Validation only — zero format change; the public API is source-compatible
(two additive error variants, one additive codec fn).

**How it was tested:** 22 new hostile-volume fixtures (`unaos/libs/fs/unafs/tests/hostile_volume.rs`) craft
corrupt superblocks/inodes/extents/prefixes and assert graceful `Err` — never `#[should_panic]` —
plus positive controls (pristine mount, sparse bulk-fill, decode-under-budget); `cargo test -p
unafs` 64 green with the golden format KATs byte-identical; `no_std` check clean on the kernel
target (`aarch64-unknown-none-softfloat`); `./arroyo check` green both arches; `kernel8-test`
PASS-count byte-equivalent with the trusted K3 fixture still mounting `w=0x1ff`. Metal: none needed
(host-native lib arc; the kernel consumes it unchanged).

---

## Handlers — VAIRE-RITES-1 (the Loom awakens: Bolt manifest + STATUS/Crystal) — 2026-07-13 🔬 `us-vaire`

**QEMU-green n/a (host-native Ring 3 handler; gate is `cargo test -p vaire`).** Vaire graduates
from a single-repo status probe to the STATUS half of the Loom: a **Bolt manifest** (register/list
managed units by kind) and real per-unit **Crystal Color** (Green/Amber/Red).

- **is_dirty un-stubbed (step 0):** `Vaire::look()` reported a hard-coded `is_dirty = false` — a
  documented lie. Now a real check via `gix`'s `status` feature (index-vs-worktree, tree-vs-index;
  untracked files excluded, matching porcelain's tracked-change notion). Proven by a tempdir
  fixture: a freshly committed repo → clean/Green; a modified **tracked** file → dirty/Amber.
- **Bolt manifest:** `Manifest::{register,list,status_of,status_all}` over `Bolt { name, path,
  kind }` with `BoltKind::{GitRepo, Vault, NleProject(reserved)}`. Registration is order-preserving;
  status dispatches on kind and maps to `CrystalColor`.
- **The vault as the first non-git unit:** a read-only `probe_vault` rides UnaFS's fail-closed
  mount check — absent/unreadable/unmountable → Red (bytes untouched, opened `open_read_only`),
  mounts → Green (last-snapshot n/a until SNAP/RITES-2). A corrupt-vault test asserts the on-disk
  bytes are byte-identical after the probe.
- **Tested:** `cargo test -p vaire` → 7/7 (2 is_dirty, 2 manifest, 3 vault); `cargo check -p vaire`
  clean. Lane: `handlers/vaire/**` + its Cargo.toml only (added `gix status` feature, `unafs` dep,
  `tempfile` dev-dep); zero `unaos/` diff. SYNC/SNAP are RITES-2; UnaFS-native versioning is the
  Destiny (behind UNAFS-F1).

## Pi 4 — UNAFS-K4 (BeFS-K4: journaled kernel WRITES on the native unafs volume) — 2026-07-14 ✅ metal-confirmed 2026-07-14 `hw-pi4`

**✅ METAL-CONFIRMED (2026-07-14 attended Pi 4 bench; kernel at `main 2b7bd2d`, kernel8.img 703,952 B,
img sha256 `10d1ade7…f1a0a935`, serial `pi-serial-2026-07-14-113721.log`): 3 cold boots with a GENUINE
power-cut between each — `K4-write PASS [w=0x7f]` on every boot, boot-2 volume remounted CLEAN (no
dirty-mount, no corruption: the journaled write survived a real power loss), full witness chain intact
(23 PASS + K3-mount [w=0x1ff] + K1/K2/K3-revoke/IMG-SIG/FATDIRS/FATMOVE/K4-ready + F2/F3 locked
240000/240000), 0 forbidden. Boot-2 U9/U10/U11/U6-grants showed only the DOCUMENTED stale-fixture
signature (non-self-cleaning FAT fixtures on an un-re-prepped card) — not regressions. ⚠ Separate open
watch-item ESCALATED at this bench: core 3 spin-table did NOT come online on any of the 3 cold boots
(mbench 30/32, the CAPSTONE pair) — now build/size-correlated (up 4/4 on ≤524,232 B builds across 6
archived logs 07-10..12; down on 589 KB and 704 KB) — the runbook names the a834b8f back-to-back bisect;
orthogonal to K4 (all K/F/U witnesses pass on 3 cores; CAPSTONE 6/6 itself stands metal-confirmed from
round-9).** The
kernel's unafs mount becomes read-WRITE: `fs/unafs.rs`'s `SdSectorDevice::write_sector` now routes to
the hardened block layer (`drivers::block::write_block` — emmc2 CMD24 + R1/CMD13 status checks), so the
K3 `Io` stub is retired.

- **M1 — the coherence keystone (the core of the arc).** K3 mounted per call, which is safe only while
  the volume is immutable. Writes make a per-call mount a corruption hazard: two live mounts hold two
  independent in-RAM allocation bitmaps + journal cursors, so a block one frees the other can
  re-hand-out. Every unafs access — read AND write — now flows through a single, process-wide,
  IRQ-masked mount (`with_unafs`, modelled on the F3 `NAMESPACE` lock): one authoritative in-RAM
  bitmap/journal, all operations serialized. Keeping one mount live also means a pure read
  (`uls`/`ucat`/`K3-mount`) never fires the crate's `Drop`-time `sync_metadata` write-back — reads stay
  genuinely read-only. `force_remount` drops the cached instance so the next access re-reads the volume
  from disk (the durability-proof primitive).
- **M2 — journal audit + honest torn-write scope.** The `unaos/libs/fs/unafs` WAL is **intent-logging only**
  (BeginOp/EndOp markers → dirty detection on the next mount; NO undo/redo, NO replay or rollback —
  "Log only for now"). Crash safety therefore comes from write ORDERING inside the crate
  (new-extents-first, single-block metadata swap, free-last → a power cut LEAKS blocks, never dangles a
  reference), not from the journal. The crate's "single-block swap is atomic" claim holds at its 4096 B
  block granularity, but the medium writes 512 B sectors: one `write_block` is eight non-atomic
  `write_sector`s, so a swap is truly atomic only when the live record fits the first 512 B sector.
  **No `unaos/libs/fs/unafs` change** — the audit found the journal partial *by design*, not a gap; the frozen
  format + KATs stay byte-identical. Recovery on a dirty mount does not write (it warns), so a RO
  consumer can still mount-and-warn. Full scope in `SECURITY.md` §K4.
- **M3 — shell write verbs + witness.** `utouch`/`uwrite`/`umkdir`/`urm` (write-through, durable;
  absolute case-sensitive paths, no shell cwd) route through `with_unafs`. Uncounted `:: K4-write: …
  PASS [w=0x7f] ::` witness: create+write `/K4TEST.TXT` → force a genuine remount → byte-verify the
  durable write → delete → remount (delete durable) → negative path → clean journal. Self-cleaning
  (create then delete + journal-head reset), so the volume is left with only the staged K3 fixtures —
  the `if=sd` write-back makes the remount proof real (the K2 M(e) same-image technique). `k3_mount_selftest`
  reworked: its read path now runs through `with_unafs`, and bit4 no longer writes to `base_lba` (with a
  live write path that would have zeroed the superblock) — it now proves the write seam is bound-checked
  (an out-of-range LBA is refused).
- **Gate ALL GREEN:** `check` both arches; `kernel8`; `kernel8-test` mbench **32/32 required witnesses,
  0 forbidden** (23 fixture PASS + CAPSTONE 6/6 byte-equivalent + `K3-mount PASS [w=0x1ff]` +
  `K4-write PASS [w=0x7f]` + F2/F3 locked 240000/240000 + K1/K2/K3/IMG-SIG/FATDIRS/FATMOVE/K4-ready), no
  dirty-mount warning; `test-arm` MISSION SUCCESS; unafs host tests green (kat_vectors 8/8 + hostile_volume
  23/23 + all suites — **KATs untouched**). Zero x86 (the module + dep are aarch64-only). Lane:
  `fs/unafs.rs`, `shell.rs` unafs-verb region, `arch/aarch64/syscall.rs` witness tail, `pi4-regression.spec`.
- **Metal rider (separate, attended — Peter):** write on silicon → REAL power-cycle → boot 2 byte-verifies
  the write survived (the K2 two-boot idiom, now for data). Not attempted here (split mode).

## Pi 4 — UNAFS-K3 (BeFS-K3: kernel read-only mount of a native unafs volume) — 2026-07-12 ✅ `hw-pi4`

**✅ METAL-CONFIRMED (2026-07-12 attended evening bench, real Pi 4, reflashed card with the unafs
partition):** `:: K3-mount: … mounted RO + ls/cat byte-verified PASS [w=0x1ff] ::` captured in FIVE
boots on silicon — the first kernel read of the native unafs filesystem on real hardware. Pristine
boot: 23/23 fixture PASS (U9/U10/U6-grants/U11 all real), F2/F3 parallelism witnesses locked
240000/240000, **0 forbidden** (no R1/CMD13/exception/heal), dirty-mount warn line absent (clean
volume) — mbench **28/30**, missing only the CAPSTONE pair. Interactive evidence (HDMI panel,
serial-FIFO-injected — the pi kernel8 build is `skip_xhci`, no USB keyboard): `uls` → the two
fixtures + "(2 entries)"; `ucat /K3HELLO.TXT` → the pinned text; `ucat /K3NOPE.TXT` → refused
(photo witnessed). ⚠ **Bench observations for the ledger:** (1) core 3 failed spin-table release in
ALL 6 boots this evening (capstone skipped) vs 4/4 cores this morning on `a834b8f` — a flagged
watch-item (environment vs build-correlated undetermined; scheduler untouched by this arc; CAPSTONE
6/6 stands metal-confirmed from round-9 the same day); (2) `resolve_path` reports a missing name as
`FileSystemError::RootMissing` — misleading error name, folded into the BEFS-HARDEN/K4 cleanliness
list. Logs `~/unaos-bench/pi-serial-2026-07-12-{211701,212337}.log` (+3 earlier stale-media boots
that still captured K3-mount PASS).

The first time the native unafs filesystem is read by the kernel — the BeFS chain's
K1 (`no_std` port) → K2 (block adapter) seam consumed end-to-end. Three commits:

- **Warn seam (`unaos/libs/fs/unafs`, carried UNAFS-2 ledger item):** `warnlog::set_warn_hook` — a
  dependency-free `no_std` fn-pointer hook; the torn-journal dirty-mount warning (previously a
  `std`-gated `println!`, silent in a kernel) now reaches the kernel serial console. Host `std`
  behavior unchanged; 29 unafs host tests green.
- **M1 (mount):** kernel `fs/unafs.rs` implements `unafs::adapter::SectorDevice` over
  `drivers::block::read_block` (512 B LBAs; `write_sector` a deliberate `Io` stub — the RO
  guarantee is at the seam, so no path can touch the medium; K4 owns writes). `locate_unafs`
  finds the volume by `UNAFS` superblock magic; `BlockAdapter::for_partition` + `UnaFS::mount`.
  Staging: `arroyo kernel8` builds a real 4 MB unafs volume with the host `unafs` CLI
  (deterministic fixtures `K3HELLO.TXT` + 12 KiB patterned `K3PAT.BIN`) and `make-pi-img.sh`
  carries it as **MBR partition 2** (type 0x7f, written by byte offset — nothing auto-mounts
  during image build), identical for QEMU `if=sd` and the flashed card.
- **M2 (read paths):** the uncounted `:: K3-mount: … PASS [w=0x1ff] ::` witness byte-verifies
  root `ls` (exactly the two fixtures), a single-block read (`K3HELLO.TXT` == pinned text), a
  multi-block/extent-walking read (all 12 KiB of `K3PAT.BIN` match `(i*7+3)&0xFF`), a negative
  resolve, and the RO-seam refusal. Shell gains `uls [path]` / `ucat <path>` (absolute,
  case-sensitive; `ucat` bounded at 8 KiB) over the same mount.

**Tested:** `check` both arches; `kernel8`; `kernel8-test` 29 PASS byte-equivalent + CAPSTONE 6/6 +
all prior witness lines + `K3-mount PASS [w=0x1ff]`, zero R1/CMD13/exception; `test-arm` MISSION.
**Metal (M3) attended-pending:** the card must be **reflashed** from the new image (file-level prep
cannot add partition 2). `query` on metal deferred — the engine is `std`/FP-`sqrt`-gated.

---

## Pi 4 metal — round-9 attended bench (2026-07-12) — ✅ `hw-pi4`

One boot on a real Pi 4 (kernel `a834b8f`) cleared the whole accrued Pi metal backlog. `mbench`
vs `pi4-regression.spec` = **30/30 required, 0 forbidden**, CAPSTONE **6/6** (all 4 cores up), the
23-PASS fixture chain, and **zero** R1/CMD13/AARCH64-EXCEPTION/PANIC or A72 `EC=0` heal lines. Log
`~/unaos-bench/pi-serial-2026-07-12-174020.log`. Flips to **✅ metal-confirmed** and promotes the
spec PENDINGs → REQUIRE:

- **K3** two-phase durable-first revoke — `K3-revoke … durable-first PASS [w=0x7f]` (revoke
  survives reboot; kept grant intact; forced persist-fail → -EIO with the in-RAM grant intact).
- **IMG-SIG** — `IMG-SIG … residual closed) PASS [w=0x7f/0x7f]`; K2-liveenf re-admit by
  name + IMAGE-digest confirmed live.
- **FATDIRS** (create_dir/remove_dir) — `FATDIRS … delete_located) PASS [w=0xff]`.
- **FATMOVE** (rename_entry/move_entry) — `FATMOVE … keep-chain) PASS [w=0x1ff]`. Pi captured
  it FIRST, so the Orin JD10 bench no longer owes the FATMOVE witness.
- **K4-ready** — `K4-ready … prefix) PASS [w=0xff/0xff]` (pure in-RAM codec, rode the boot).
- **F2/F3 under TRUE 4-core parallelism** — both `locked 240000/240000 intact (0 lost)`;
  unlocked lost 120000/240000 (exactly 50%). Serialization holds on silicon (QEMU cannot test
  this leg).

Spec promotions committed here; the granular arc detail is in each arc's own entry below.

## hw-rmbp track — 2026-07-12 (round-9 attended metal bench: STOR-1 S1–S7 + VPERF-WC)

### STOR-1 S1–S7 (the whole interrupt-driven x86 storage chain) ✅ metal-confirmed `hw-rmbp`
- **What:** the full S1–S7 IF-safe interrupt-driven storage arc (service task + submit/block/complete,
  live reads/write-through, synchronous grow/create/delete, shared cross-process backing, the syscall
  NAMESPACE lock, and open-of-ANY-on-disk-file) behind the `irqstorage` knob.
- **Tested — metal (real 2012 rMBP, over FTDI serial, TWO clean boots):** `./arroyo mbench --spec
  round9-rmbp.spec` **PASS 37/37 required + 0 forbidden + 6/6 pending matched, 0 fault**. First metal for
  **S6** (`S6-witness`: locked 240000/240000 intact, **unlocked lost 119996/240000** under real cross-core
  contention — the namespace lock serializes on true SMP, the proof risk 3 deferred), **S7** (`S7-openany`:
  README.TXT resolved dynamically + `owned.bin` refused, the case-insensitive owner-ACL exclusion holds on
  silicon), and the **S4-race** (close-release + teardown-release synchronous delete) + **S4-mf2** witnesses.
  S1–S5 re-confirmed (first metal round-6). Knob-ON usbdebug+videobench media, kernel.elf sha `3083b467`.

### VPERF-WC — framebuffer Write-Combining ✅ metal-confirmed `hw-rmbp`
- **What:** retype the fb identity-map leaves to Write-Combining (PAT PA4) so the CPU coalesces the
  shadow's write-only blits; F1 SFENCE-drain at every flush seam so a panic's tail can't strand in a WC buffer.
- **Tested — metal (real 2012 rMBP):** the `vperf: fbmem` readout FLIPPED from round-6's `pat=WB eff=UC` to
  **`pat=WC eff=WC`** (`fb-wc: retyped 15 leaf(s)`); scroll **dramatically faster (~10×)** and the vug/quartz
  GUI clocked **53.8 fps** (up from round-6's ~7.6 fps, ~7×) — both attended eyeballs; and a deliberate
  `panic` at the GUI console rendered the **full red panic screen, no truncated tail** (the F1 WC-drain, the
  one thing QEMU/TCG cannot witness). VPERF-WC = confirmed.

---

## hw-jetson track — 2026-07-15 (JD18 — read-only tree tools: `find` + `du` + `uptime`)

### JD18 — read-only TREE TOOLS: `find` (recursive glob search), `du` (subtree size tally), `uptime` (`shell.rs`; one additive `clock.rs` helper) ⏳ attended-pending
- **Why:** the file-manager verb set is closed; JD18 adds three read-only surveying tools built entirely from
  primitives already shipped — no new `fat.rs` surface, zero mutation.
- **How (all `shell.rs`, composing the JD9/JD13 `read_dir` SNAPSHOT walk + JD12 `glob_match`, `.`/`..` filtered,
  `CP_MAX_DEPTH` bound = honest `-ELOOP`, honest partial on a mid-walk `-EIO`):**
  - **`find <root> <pattern>`** (one arg = pattern, root defaults to `.`): walks under `<root>`, matches each
    8.3 name with the existing `glob_match` (case-insensitive; `*`/`?`; literal = exact), prints each hit as
    its full canonical path (dirs trailing `/`), then `N match(es), M dir(s) scanned` (dirs = every `read_dir`
    level). Missing root → `-ENOENT`; a FILE root degrades to a POSIX self-match test (`0 dir(s) scanned`).
  - **`du <dir>`** (default cwd): per direct child prints total bytes (a file = its size, a dir = the recursive
    subtree sum), then `total: N byte(s) in M file(s), K dir(s)`. **FAT directory entries report size 0** — only
    file bytes are real. `du FILE` = its one line.
  - **`uptime`:** seconds since boot from a small **additive** `clock::uptime_secs()` (aarch64
    `CNTPCT/CNTFRQ`, x86 `None` — reads the same `monotonic()` source WITHOUT touching the JD17 anchor/`now()`),
    rendered `up HH:MM:SS`; appends the JD17 wall clock when set. x86 → honest "no calibrated counter on this arch".
- **Not in scope:** any mutation, `find -exec`/`-type`, mid-path globs, `du -h`, sorting beyond walk order.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings); `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `kernel8-test` **0 FAIL**; `UNAOS_HUBSTORAGE=1 test 25` MISSION;
  `esp-jetson` links, `tegra:` COUNT unchanged (these verbs add no `tegra:` token; built LAST). Lane clean:
  `shell.rs` + one additive `clock.rs` helper + docs + `jd18-bench.md`; `fat.rs` untouched.
- **Metal:** ⏳ **attended-pending** — `jd18-bench.md`: seed a nested tree → `find` by glob → `du` tallies match
  the seeded sizes → `uptime` sane + monotonic across two reads → all again after a power-cycle. ⚠ `dot_clean`
  BOTH cards.
- **Detail:** [`arch_arm64.md` §JD18](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

---

## hw-jetson track — 2026-07-15 (JD17 — the kernel clock: `setdate`-seeded wall time stamping FAT mtime)

### JD17 — kernel WALL CLOCK: `setdate`-seeded, counter-extended time that stamps FAT mtime (`clock.rs` new; `shell.rs`; `fat.rs` write-side) ⏳ attended-pending
- **Why:** §JD16 surfaced FAT mtime read-only and documented the honest gap — with no RTC, kernel-written
  entries carried an all-zero stamp and `ls -l` showed a dash. JD17 closes it **without fabricating a clock**:
  the operator seeds wall time once per boot, the free-running arch counter extends it, and the FAT
  publication paths stamp mtime from it **when set** (untouched/zero when never set stays honest).
- **How (`clock.rs`, new):** `WallTime` (calendar-validated 1980..=2107); `set(t)` plants an anchor =
  `base_secs` (since the 1980 epoch) + `CNTPCT` at set, under a `spin::Mutex`; `now()` = base + elapsed
  ticks/`CNTFRQ` (the JD3 timerless mechanism), `None` while unset; `fat_stamp()` packs `(time@0x16,
  date@0x18)` bit-exactly inverse to §JD16's `DirEntry::mtime()` decode, `(0,0)` while unset. `from_secs`
  **saturates at end-2107** (no wrap/panic). 2-second resolution, no timezone (local wall time, per §JD16).
- **Frozen x86 (honest):** no calibrated invariant counter is plumbed on x86 — `monotonic()` is `None` there,
  a set clock is frozen at its seeded second; the verbs merely compile. x86 calibration = out of scope.
- **Shell (`shell.rs`):** `date` (prints wall clock or "clock not set") + `setdate YYYY-MM-DD HH:MM[:SS]`
  (seconds optional); `parse_setdate` enforces strict shapes, `clock::set` owns range validation; `CLOCK:`
  help line.
- **FAT write-side (`fat.rs`) — publication paths only, riding the EXISTING `with_dir_lock` RMWs (no new lock,
  no extra I/O):** both create twins (VERBATIM-twinned, pre-zeroed slot ⇒ `(0,0)`-unset byte-identical to
  pre-JD17) + `write_grow` step-4 via a new `write_dir_entry_fields_mtime` sibling that, **when the clock is
  unset, leaves the on-disk words UNTOUCHED** (a host-stamped file rewritten by a clockless kernel keeps its
  stamp — strictly less destructive than zeroing).
- **⚠ Honest gap (design ruling, LC-orin):** the stamp lands ONLY on publication paths. `fat.write_at` (the
  bounded in-place overwrite — dir-untouched/never-grows/never-allocs, leaned on by the x86 S8 witness and S3
  write-through) stays **completely untouched**, so a pure in-place overwrite does NOT refresh mtime this arc.
  Not shell-visible: the panel `write` verb is truncate-recreate (stamped) and append is `write_grow`
  (stamped), so every SHELL mutation stamps; the only unstamped path is the EL0 in-place `sys_write`.
  `rename`/`move` keep the plain sibling on purpose (they preserve mtime).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings); `test-arm 22`
  MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `kernel8-test` **0 FAIL** (protects the shared `fat.rs`);
  `UNAOS_HUBSTORAGE=1 test 25` MISSION (x86, shared shell/fat guard). Wall-clock stamp not headless-reachable
  in-lane → attended card `jd17-bench.md`. Lane: `clock.rs` (new) + `shell.rs` + `fat.rs` write-side (the
  SEAM coordinated with LC-x86/S8 — S8 touches no `fat.rs` code).
- **Metal:** ⏳ **attended-pending** — `jd17-bench.md`: `setdate` → write a file → `ls -l` shows the stamp →
  power-cycle → stamp survives → touch a second file next boot WITHOUT `setdate` → dashes.
- **Detail:** [`arch_arm64.md` §JD17](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

---

## hw-jetson track — 2026-07-14 (JD16 — `ls -l` long listing with real FAT timestamps)

### JD16 — `ls -l`: long listing with REAL FAT last-write timestamps (first `fat.rs` READ-side edit; `shell.rs`) ⏳ attended-pending
- **Why:** every FAT short directory entry already stores a last-write timestamp (offsets 0x16/0x18), but
  JD1–JD15 kept `fat.rs` call-never-edit and never surfaced it. JD16 adds `ls -l` — size + modified timestamp
  + name — by reading (not writing) that field. First arc to touch `fat.rs`, and only its read/parse path.
- **How (bounded `fat.rs` read-side grant):** `DirEntry` gains `mtime_time`/`mtime_date` (the two packed
  on-disk words), filled by `classify_dir_slot` from the same 32-byte slot the walkers already parse — **zero
  extra I/O, every pre-JD16 caller byte-identical**. `DirEntry::mtime()` decodes to a new `FatTimestamp`. No
  write primitive, no serialization, no lock changed — reconciles cleanly with the concurrent x86 STOR-S8
  write-side arc. Creation time (0x0E/0x10) deliberately left unread.
- **FAT format, honest:** epoch **1980-01-01**; resolution **2 seconds** (seconds/2 in the low 5 bits); **no
  timezone** (local wall-clock, presented verbatim). An all-zero pair is the `is_zero()` sentinel.
- **Shell (`shell.rs`):** the `ls` arm parses `-l`/`-L` (JD14 flag convention — filtered from the path, a file
  named `-l` reachable as `./-l`). Plain `ls` unchanged. `ls -l` adds the timestamp column (dir shows `<DIR>`
  + trailing `/`); threads through the same `print_dir_listing` as the JD12 wildcard, so `ls -l *.TXT` works.
  `fmt_mtime` renders a zeroed stamp as a dashed placeholder.
- **⚠ Kernel-written files carry ZERO timestamps (observed, not invented):** the kernel has no RTC; the
  `fat.rs` create path zeroes the whole entry (time/date = 0) and the write/append paths only republish
  size + chain-head — so any OS-created/written file shows the dashed placeholder. That is the honest verdict.
  Host-written files carry their real host timestamp and display faithfully. **A real on-write clock is a
  named FUTURE arc**, not JD16.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings); `test-arm 22`
  MISSION; `kernel8-test` **40 PASS / 0 FAIL** (this battery protects the shared `fat.rs`); `test 25` MISSION
  (x86, `fat.rs` shared there too). As in JD2–JD15 the shell command path is not headless-reachable, so the
  `ls -l` verdict is exercised by the `jd16-bench.md` attended card. Lane: `fat.rs` read-side + `shell.rs`.
- **Metal:** ⏳ **attended-pending** — `jd16-bench.md` checks a host-written file shows its real timestamp,
  a kernel-`touch`ed file shows the dashed placeholder, and an mtime survives a power-cycle.
- **Detail:** [`arch_arm64.md` §JD16](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `us-jd16` (the seat
  assigns the integration hash at merge).

---

## hw-jetson track — 2026-07-14 (JD15 — `-f` tree-replace for `cp -r`/`mv`)

### JD15 — `-f` tree-replace: forced replace of an existing directory-TREE destination (`shell.rs`-only, call-never-edit) ⏳ attended-pending
- **Why:** JD14 bounded `-f` to a single FILE dest — an existing directory tree stayed `-EEXIST` (`cp -r`) or
  `-EISDIR` (`mv -f`), and the operator had to `rm -r` it first. JD15 closes the last flag-family gap:
  `cp -rf` and `mv -f` now REPLACE an existing directory-tree destination, so the forced verbs behave
  uniformly whether the destination is a file or a whole subtree.
- **How (call-never-edit, no `fat.rs` change):** a new `force_remove_existing` helper deletes whatever
  occupies the destination (a FILE via `locate_in_dir` + `delete_located`; a DIRECTORY via the JD13 `rm_tree`
  to empty it, then `remove_dir`), then the caller proceeds down its normal fresh-destination path — `cp -rf`
  builds the fresh tree; `mv -f` relinks (`rename_entry`/`move_entry`) into the freed slot. Composes only
  existing primitives.
- **Semantics:** no-clobber stays the panel DEFAULT — only `-f` opts in. `-n` unchanged; plain `-f` on a FILE
  dest unchanged; a directory dest WITHOUT `-f` still `-EEXIST` (`cp -r`) / lands the source inside it (`mv`
  copy-into idiom). The JD9 self/subtree refusal, the `mv` dir-across-parents refusal (surfaced BEFORE any
  delete-dst-first), and the `cp -rf /` / `rm -rf /` root footgun refusals all stand.
- **⚠ Crash-safe-PARTIAL (the JD13 honest-count discipline):** the destination is deleted BEFORE the fresh
  copy/move, so a power cut in the delete→recreate window leaves the destination ABSENT — never
  half-overwritten or silently merged. No rollback; re-run `cp -rf`/`mv -f` to complete. `-f` tree-replace
  deliberately trades the plain `-EEXIST`/`-EISDIR` refusal for this bounded, honest destructive window.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings — only the pre-existing
  `shutdown` double-`hlt_loop`); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `UNAOS_HUBSTORAGE=1 test 25` MISSION. Zero x86 behavioural change. No `kernel8-test` on the jetson side. As
  in JD2–JD14 the shell command path is not headless-reachable (keystroke-driven, tegra-only), so the
  regression suite proves no breakage and the new behaviour is exercised by the `jd15-bench.md` attended card.
  Lane: only `shell.rs` (fat.rs/console.rs/main.rs/NET arms/unafs verbs untouched).
- **Metal:** ⏳ **attended-pending** — pairs cleanly with any next Orin bench (`jd15-bench.md` builds a tree
  with an existing tree destination, exercises `cp -rf` / `mv -f` replace, and power-cycles for the
  crash-safe-partial durability check).
- **Detail:** [`arch_arm64.md` §JD15](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

## hw-jetson track — 2026-07-14 (JD14 — `-f`/`-n` flag family for `cp`/`mv`/`rm`)

### JD14 — `-f`/force + `-n`/no-clobber flags for `cp`/`mv`/`rm` (`shell.rs`-only, call-never-edit) ✅ metal-confirmed 2026-07-14 `hw-jetson`
- **Why:** the verb set closed at JD13, but the everyday POSIX ergonomics were missing — no way to overwrite a
  destination, and `rm NOSUCH` always complained. JD14 adds the flag family that completes it: `cp -f`/`mv -f`
  overwrite, `rm -f`/`rm -rf` delete quietly, `-n` makes the no-clobber default explicit.
- **How (call-never-edit, no `fat.rs` change):** a new `split_flags` parses bundled short flags (`-rf` ==
  `-r -f`) — which also fixes a latent gap where the old exact-token match never recognized `rm -rf DIR`. The
  flags only gate which existing primitive runs (`copy_file_into` truncate, `delete_located`,
  `rename_entry`/`move_entry`).
- **Semantics (panel-consistent):** no-clobber is now the DEFAULT for `cp` AND `mv` — an existing destination
  FILE is `-EEXIST` unless `-f`. This aligns `cp` with `mv`'s pre-existing default (a deliberate divergence
  from POSIX `cp`, which overwrites silently; the panel favours safety + cp/mv symmetry). `-f` overwrites a
  FILE dest (cp truncates-in-place; mv delete-dst-first); a DIRECTORY dest is never clobbered even with `-f`
  (needs `rm -r`), and `cp -r`'s fresh-tree `-EEXIST` stands regardless of `-f`. `-n` reasserts the default and
  overrides `-f`. `rm -f`/`rm -rf` suppress the missing-target error and no-match wildcard quietly (POSIX); two
  guards are NOT relaxed — `rm -rf /` stays `-EBUSY`, and a wrong-usage `-EISDIR` is still shown.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings — only the pre-existing
  `shutdown` double-`hlt_loop`); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard); `esp-jetson` links, **109 `tegra:` strings** —
  UNCHANGED from JD11–JD13 (the flag strings carry no `tegra:` token; validate by count, not size). Zero x86
  behavioural change. No `kernel8-test` on the jetson side. Lane: only `shell.rs`
  (fat.rs/console.rs/main.rs/NET arms/unafs verbs untouched).
- **Metal:** ✅ **METAL-CONFIRMED 2026-07-14** (attended Orin bench, one card session with JD13; kernel
  `57ae4b2`, serial `jetson-serial-2026-07-14-101517.log`, 5 clean boots / 0 heals). All card sections
  passed: no-clobber default, `-f` overwrite both verbs, `mv -f` dir-dest refused, quiet `rm -f`/`-rf` on
  missing/no-match, bundled `rm -rf DIR`, `rm -rf /` `-EBUSY`, forced-overwrite durable across a real
  power-cycle. Bench-corrected: the card's §4 `cp -rf` criterion was wrong — the fresh-tree `-EEXIST` fires
  on the COMPUTED target (`823b5ba`); silicon confirmed both the copy-INTO nest and the second-run refusal.
- **Detail:** [`arch_arm64.md` §JD14](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

## hw-jetson track — 2026-07-14 (JD13 — recursive `rm -r` on the panel shell)

### JD13 — recursive `rm -r <dir>` (`shell.rs`-only, call-never-edit) ✅ metal-confirmed 2026-07-14 `hw-jetson`
- **Why:** the create/copy/move/delete quadrant had a gap — `rm` was file-only (a directory was `-EISDIR`) and
  `rmdir` removes only an EMPTY directory, so there was no one-command way to delete a subtree. JD13 closes the
  destructive side (`rm -r DOCS`) and multiplies with the JD12 glob (`rm -r OLD*/`). It is the delete twin of
  JD9's `cp -r`, inverted: `cp_tree` creates top-down, `rm_tree` deletes bottom-up (a directory is emptied
  before it is removed).
- **How (call-never-edit, no `fat.rs` change):** the `rm`/`del` arm gained a `-r`/`-R` flag; `fs_rm_recursive`
  is the handler, `rm_tree` the recursion. It composes existing primitives — `read_dir` (JD4) walks each level,
  the `fs_rm` pair (`locate_in_dir` + `delete_located`, JD6) unlinks each file quietly (one summary, not a
  flood), and the `rmdir` primitive (`remove_dir`, FATDIRS) removes each emptied directory. Reuses the JD9
  `CP_MAX_DEPTH = 32` cap (honest `-ELOOP`) and a `RmStats` partial-count tally.
- **Footgun rails:** `-r` is REQUIRED for a directory (without it a dir stays `-EISDIR`, byte-identical to
  pre-JD13); the ROOT is refused `-EBUSY` before any walk (mirrors `rmdir`, folds `rm -r .`/`..` at the root
  into the same tag); a FILE target degrades to a plain `rm` (`rm -r FILE` == `rm FILE`); a mid-tree failure
  stops and reports an honest partial count (dirs/files removed so far) + the failing path/errno, nothing rolled
  back (crash-safe per the U10 `0xE5`-then-free ordering — re-run `rm -r` to clear the remainder).
  **Snapshot-then-delete:** `read_dir` snapshots each level and children re-locate by name, so a delete never
  invalidates the walk — the JD12 glob-safety property carried into the recursion. No `is_descendant` guard is
  needed (no destination ⇒ no self-into-subtree hazard; `CP_MAX_DEPTH` bounds termination).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings — only the pre-existing
  `shutdown` double-`hlt_loop`); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard); `esp-jetson` links, **109 `tegra:` strings** —
  UNCHANGED from JD11/JD12 (the `rm -r` strings carry no `tegra:` token; validate by count, not size). Zero x86
  behavioural change. No `kernel8-test` on the jetson side. Lane: only `shell.rs` (fat.rs/console.rs/main.rs/NET
  arms untouched).
- **Metal:** ✅ **METAL-CONFIRMED 2026-07-14** (attended Orin bench, one card session with JD14; kernel
  `57ae4b2`, serial `jetson-serial-2026-07-14-101517.log`, 1059 KEY / 149 OUT, 5 clean boots / 0 heals /
  0 fatals, CAPSTONE all-6 every boot). Full card: the tree removed with honest counts, every guard fired
  (`-EISDIR`/`-EBUSY`/`-ENOENT`/file-degrade), the glob form cleared several trees, and the power-cycle +
  same-named re-create capstone proved the freed clusters were genuinely released and reused (fresh bytes
  read back, not the deleted tree's).
- **Detail:** [`arch_arm64.md` §JD13](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

## hw-jetson track — 2026-07-13 (JD12 — paging & wildcard globbing on the panel shell)

### JD12 — `head`/`tail` paging + `*`/`?` wildcard globbing (`shell.rs`-only, call-never-edit) ✅ `hw-jetson` (METAL-CONFIRMED 2026-07-13 attended Orin bench, one session with the JD11 confirm; glob copy/move survived a real power cycle)
- **Why:** the classic file-manager verb set closed at JD10 and JD11 made benches self-documenting; JD12 is
  the polish pass — two user-facing conveniences over that set, no new `fat.rs` surface. Paging lets you read
  a long file's head/tail without flooding the scrollback-less panel; globbing multiplies every fs verb
  (`rm *.TMP`, `cp *.TXT DOCS/`, `mv *.LOG ARCHIVE/`), the biggest remaining in-lane win.
- **Paging (M1):** `head <path> [n]` / `tail <path> [n]` — first / last `n` lines (default 10). `head` streams
  from offset 0 via the offset-aware `read_at` and STOPS at `n` newlines (so `head 10` of a huge file never
  slurps it; a 64 KiB ceiling backstops an unterminated line); `tail` reads a bounded 64 KiB window at EOF,
  probes the byte before it to keep or drop a boundary line precisely, and prints the last `n`. `cat`/`head`/
  `tail` share one `render_text`; `cat`'s body moved onto a `cat_render` helper (byte-identical) the wildcard
  `cat` reuses.
- **Globbing (M2–M3):** a single TRAILING glob in a path's last component expands against `read_dir` of its
  parent (case-insensitive 8.3; `*` = any run, `?` = one char, via an iterative star-backtrack matcher).
  Wired into `ls`/`cat` first (non-destructive), then `rm` (multi-target) and `cp`/`mv` (multi-source, LAST
  path = destination; `>1` source requires the destination be an existing directory, else `-ENOTDIR`).
  Expansion is invoked ONLY inside the fs-verb arms — the shared arg-split and the NET arms
  (`netinfo`/`ping`/`arp`/`connect`/`udpsend`/`get`, a sockets-arc lane) are untouched. **Snapshot-then-act**:
  the match list is captured before any mutation, so a `rm *.TXT` never invalidates its own list; no match is
  an honest per-pattern note; the single-source / no-wildcard case is byte-identical to pre-JD12.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches (no new warnings); `test-arm 22`
  MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs`
  guard); `esp-jetson` links, **109 `tegra:` strings** — UNCHANGED from JD11 (the new verbs carry no `tegra:`
  token; the ELF grew to ~725 KB purely from the base's merged SOCK-3/UNAFS-K3, not JD12 — validate by count,
  not size). Zero x86 behavioural change. A 1-lens adversarial review found no data-correctness bug; two
  low-severity truncation-note edges were folded in. No `kernel8-test` on the jetson side.
- **Metal:** ⏳ **ATTENDED-PENDING** — the interactive path only runs on silicon (tegra never runs in QEMU).
  Bench card [`jd12-bench.md`](../unaos/scripts/jd12-bench.md): page a file, then glob `ls`/`cat`/`cp`/`rm`/`mv`
  over a set of same-extension files and confirm on the JD11 serial transcript that the right files are
  touched, a no-match pattern reports honestly, and a multi-source copy onto a non-directory is `-ENOTDIR`.
- **Detail:** [`arch_arm64.md` §JD12](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the seat
  assigns the integration hash at merge).

## hw-jetson track — 2026-07-12 (JD11 — mirroring shell command output to serial)

### JD11 — mirror panel command output to serial for a durable bench transcript (`shell.rs`/`console.rs` lane) ✅ `hw-jetson` (METAL-CONFIRMED 2026-07-13 attended Orin bench: 397 KEY + 77 OUT lines durable on serial; the round-9 output-vanishes gap CLOSED)
- **Why:** the round-9 Orin bench found the panel console has **no scrollback** and shell command *output*
  (`ls`/`cat`/verb results) drew only to the panel — only *keystrokes* echoed to serial
  (`:: tegra: JD2 — KEY … ::`). So verbatim output was uncapturable over the serial bridge / unreplayable by
  mbench, and card readout was the four-verb bench's bottleneck. JD11 mirrors command output to serial too,
  making every future Orin bench self-documenting — a bench-infrastructure multiplier for the whole metal
  program, not just the panel.
- **What (inert, opt-in output sink):** all shell output already converges on one sink, `Console::println`
  (shell.rs calls it for every result), so JD11 mirrors *there* — complete by construction, no per-command
  plumbing. `Console` gains `out_sink: Option<fn(&str)>` (`None` on `new()`); `println` pushes the panel-history
  line as before, then — *after* the push, so a fault in the sink can't lose the panel line — calls the sink if
  set. Off-tegra surfaces (x86 GUI, pi `render_service`, headless) never set it → `println` is byte-for-byte
  unchanged, **zero off-tegra behavioural change**. The tegra `jd2_console_pump` installs `jd2_out_sink` (a
  `cfg(feature = "tegra")` `fn(&str)` in `main.rs`) right after building the `Console`, which emits
  `:: tegra: JD2 — OUT | <line> ::`. Keeping the marker string in the tegra-gated caller means it compiles into
  the tegra kernel alone; the shared `console.rs` carries no `tegra:` literal.
- **Marker format:** shares the `:: tegra: JD2 — …` family with the keystroke marker so one
  `awk '/:: tegra: JD2 —/'` reconstructs the whole interleaved session (keys + output, in order). A
  whole-screen command (`gneiss`/vug) paints the framebuffer directly, not via `println`, so it is honestly
  **not** mirrored — text output only. Ordering/locking: the sink runs synchronously from `println` *after* the
  triggering `KEY` line has printed and released the UART, touches only the serial UART (no `Console`
  re-entrancy, no lock the caller holds) → no new lock ordering, no deadlock.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs`/`console.rs`
  guard); `esp-jetson` links (`540,184 B`), **109 `tegra:` strings** — **up 1** from the JD10 baseline of 108
  (the single new occurrence is the `:: tegra: JD2 — OUT | {} ::` marker; `strings` splits it on the em-dash so
  its `:: tegra: JD2 ` prefix is the counted fragment; shared `console.rs` adds none). First `tegra:`-count
  change since JD2 — validate media by count (109 vs virt ≈ 0/1), not size. Zero x86 behavioural change (sink
  is `None` off-tegra). No `kernel8-test` on the jetson side.
- **Metal:** ⏳ **ATTENDED-PENDING** — the payoff is itself the metal artifact: at the next Orin bench every
  `ls`/`cat`/verb result appears on serial as `:: tegra: JD2 — OUT | … ::`, giving the first durable,
  mbench-able output transcript. Bench card [`jd11-bench.md`](../unaos/scripts/jd11-bench.md): run an
  output-producing command and confirm the panel text is reproduced verbatim on the serial capture, paired
  with its `KEY` lines. ⚠ With JD11 the serial bridge is now the *primary* output-evidence channel — verify it
  captures a full boot BEFORE bench time (§JB1f); a mid-bench freeze costs the transcript.
- **Detail:** [`arch_arm64.md` §JD11](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the
  seat assigns the integration hash at merge).

## hw-jetson track — 2026-07-12 (JD10 — the panel moves & renames: `mv`)

### JD10 — `mv <src> <dst>` on the Orin panel shell (move/rename by relinking one entry, no new fat.rs) 🔬 `hw-jetson`
- **What (`shell.rs` only):** a new `mv`/`move`/`ren`/`rename` dispatch arm routes to `fs_mv`, the last classic
  file-manager verb. It closes the set — navigate (JD4), write (JD5/JD6), shape (JD7), copy (JD8/JD9),
  **move/rename (JD10)**. Unlike `cp -r`, a move is **O(1) by reference**: the file's data never moves, only its
  directory entry is relinked. JD10 consumes the pi4-lane **FATMOVE** seam (`rename_entry`/`move_entry`)
  **call-never-edit** (the FATDIRS/JD7 split), composing it with the JD6 path idioms and the JD9 `is_descendant`
  guard — **NO `fat.rs` logic of its own**.
- **Two dispatches, by parent:** resolve `src` (a ROOT source is `-EBUSY` — no leaf to move AS); decide the
  destination with the `mv SRC DIR/` idiom (an existing directory receives the entry under the source's leaf;
  else DST names it directly). **SAME parent → `rename_entry`** (rewrites the 8.3 name in place; works on files
  AND dirs — an in-place rename leaves `first_cluster`/`.`/`..` correct, so `mv DIR NEWNAME` moves a whole
  subtree with ONE relink, O(1), no `mv -r` needed). **DIFFERENT parents → `move_entry`** (re-publishes the
  entry over the SAME `first_cluster`, then `0xE5`s the old name WITHOUT freeing the chain). Files only — a
  directory across parents needs its `..` rewritten (seam scope) → the seam returns `IsDirectory` → shell
  surfaces `-EISDIR` with the honest remedy.
- **Guards (in order):** (1) if the source is a directory, moving it onto itself or into its own subtree is
  `-EINVAL` (the JD9 `is_descendant` canonical-prefix compare); (2) no-clobber `-EEXIST` — the destination must
  not already exist (mirrors the FATMOVE seam's own dest-exists refusal), EXCEPT when the destination IS the
  source (same parent + same canonical leaf, e.g. `mv FOO.TXT foo.txt`), which `rename_entry` treats as a no-op
  success (its documented same-slot contract).
- **Errno fidelity is shell-side** (the JD6–JD9 pattern): src missing → `-ENOENT`; ROOT src → `-EBUSY`; dst
  parent missing → `-ENOENT`; dst parent is a file → `-ENOTDIR`; dst dir full → `-ENOSPC`; a non-8.3 dst name →
  `-EINVAL`; a directory across parents → `-EISDIR`. On success it echoes `renamed /OLD -> /NEW` (same parent)
  or `moved /A -> /DOCS/A` (across parents), using the seam's returned canonical name.
- **Principal — unchanged, ACL-neutral by construction** (EL1 ASID 0, PUBLIC): a panel `mv` consults no U6
  `OWNED_FILES` ACL. This matters more here — `move_entry` writes a NEW `(dir_lba, dir_off)` slot, so an
  *EL0-owned* file moved from a user path would strand its owner row; but the shell runs as PUBLIC (no ACL row
  consulted or created), so it is ACL-neutral. The owner-row re-key is a future K-line seam (ledgered in the pi4
  FATMOVE `SECURITY.md` note). Crash safety is the seam's job (destination published before the source `0xE5`)
  → a power-cut mid-move leaves a benign duplicate, never a lost chain. No new lock/namespace surface.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard);
  `esp-jetson` links, **108 `tegra:` strings** (unchanged — `mv` strings carry no `tegra:` token). Zero x86
  behavioural change (`shell.rs` compiles both arches; the handler dispatches only on a keystroke). No
  `kernel8-test` on the jetson side — the FATMOVE primitives are gated headless on the pi4 side.
- **Metal:** ✅ **METAL-CONFIRMED (2026-07-12 attended bench)** — the money-shot bench card
  [`jd10-bench.md`](../unaos/scripts/jd10-bench.md) PASSED on the Orin: `mv A.TXT B.TXT` rename → `mv B.TXT
  DOCS/` move → power-cycle → `cat /DOCS/B.TXT` returned `hello alpha` intact; the O(1) directory rename
  `mv DOCS NOTES` carried the whole subtree with one relink; the guards fired honestly (`-EINVAL` dir
  self/descendant, `-EISDIR` cross-parent dir move, `-ENOENT`, `-EEXIST`, `-EBUSY`). This also flips
  **FATMOVE's own metal verdict** — its `move_entry` crash-ordering ran on silicon for the first time,
  serial-clean. Serial `~/unaos-bench/jetson-serial-2026-07-12-180110.log`; 0 heals across 4 boots.
- **Detail:** [`arch_arm64.md` §JD10](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the
  seat assigns the integration hash at merge).

## hw-jetson track — 2026-07-12 (JD9 — the panel copies trees: `cp -r`)

### JD9 — `cp -r <srcdir> <dst>` on the Orin panel shell (recursive copy, compose only, no new fat.rs) 🔬 `hw-jetson`
- **What (`shell.rs` only):** the `cp`/`copy` dispatch arm now parses a `-r`/`-R` flag; `fs_cp_recursive`
  recursively copies a directory tree. JD8 copied a file; JD9 copies a whole subtree — `cp -r DOCS BACKUP`.
  Together with `mkdir`/`rmdir` this closes the copy half of the file-manager verb set (`mv` still waits on a
  future pi4-lane FATMOVE seam). It adds **NO `fat.rs` logic**: it composes primitives that all exist —
  `read_dir` (JD4) walks the source, the FATDIRS `create_dir` seam (JD7 idiom) rebuilds the tree, and the JD8
  per-file streaming copy (refactored into the shared `copy_file_into`) copies each file — all **call-never-edit**.
- **The recursion:** resolve `src` (a ROOT source is `-EINVAL` — no leaf to copy AS; a FILE source degrades
  to a plain file copy, POSIX-friendly). The destination follows the `cp DIR DEST` idiom: an existing dir (or
  root) receives the tree AS `DEST/<src-leaf>`, a not-yet-existing DEST becomes the new tree, an existing file
  is `-ENOTDIR`. `cp_tree` filters `.`/`..` at every level, `create_dir`s each child directory and recurses,
  and streams each child file.
- **Guards (M1/M2):** (1) copying a directory into itself or one of its own descendants is refused `-EINVAL`
  (case-insensitive canonical-path prefix compare — this is what stops an infinite `cp -r DOCS DOCS/SUB`);
  (2) the top-level target must NOT already exist → `-EEXIST` (**the fresh-tree rule** — `cp -r` always
  creates a brand-new tree, never silently merging into or overwriting an existing one; because the top is
  fresh, every directory created below it is inside a freshly-created empty parent, so no child ever collides
  and no existing file is clobbered); (3) recursion is depth-bounded at 32 → `-ELOOP` (honest error, never a
  stack blow-out); (4) a mid-tree failure stops and reports the honest partial count (dirs/files/bytes copied
  before the error) + the failing path + errno — no silent truncation, no hang (every op rides the JD3
  wall-clock BOT pump).
- **Errno fidelity is shell-side** (the JD6/JD7/JD8 pattern): src missing → `-ENOENT`; ROOT src → `-EINVAL`;
  dst is a file → `-ENOTDIR`; self/descendant → `-EINVAL`; target exists → `-EEXIST`; too deep → `-ELOOP`;
  volume/dir full mid-tree → `-ENOSPC` (partial-reported). On success it echoes
  `copied /DOCS/ -> /BACKUP/DOCS/ (N dir(s), M file(s), K bytes)`.
- **Principal — unchanged** (EL1 ASID 0, PUBLIC): `cp -r` reads public sources and creates public
  destinations, no U6 ACL consulted. No new lock/namespace surface — it composes the same F3-locked
  `read_dir`/`create_dir`/`create_in_dir`/`write_grow`/`delete_located`/`read_at` primitives already ledgered.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard);
  `esp-jetson` links, **108 `tegra:` strings** (unchanged — `cp -r` strings carry no `tegra:` token). Zero x86
  behavioural change (`shell.rs` compiles both arches; the handler dispatches only on a keystroke).
- **Metal:** ✅ **METAL-CONFIRMED (2026-07-12 attended bench)** — the money-shot bench card
  [`jd9-bench.md`](../unaos/scripts/jd9-bench.md) PASSED on the Orin: `cp -r SRC DST` and into-dir
  `cp -r SRC BACKUP` (→ `/BACKUP/SRC`), each `(2 dir(s), 2 file(s), 20 bytes)`; power-cycle → `DST/SUB/B.TXT`
  `cat`'d `deep beta`, source tree untouched; guards fired (self-into-descendant `-EINVAL`, `-EEXIST`,
  volume-root `-EINVAL`). Serial `…-180110.log`; 0 heals across 4 boots.
- **Detail:** [`arch_arm64.md` §JD9](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the
  seat assigns the integration hash at merge).

## hw-jetson track — 2026-07-12 (JD8 — the panel copies files: `cp`)

### JD8 — `cp <src> <dst>` on the Orin panel shell (compose read + write, no new fat.rs) 🔬 `hw-jetson`
- **What (`shell.rs` only):** one new command handler `fs_cp` + the `cp`/`copy` dispatch arm. JD4
  navigates, JD5/JD6 write, JD7 shapes; JD8 lets you **duplicate** — `cp README.TXT DOCS/`,
  `cp DOCS/A.TXT B.TXT`. Together the panel is a file manager with the full verb set (`mv` — the last
  verb — waits on a future pi4-lane FATMOVE seam, banked by the round-9 seat pick). It adds **NO `fat.rs`
  logic**: `cp` composes primitives that already exist — the offset-aware read `read_at` and the JD6
  create-or-truncate write path (`create_in_dir` + `write_grow`), all **call-never-edit**.
- **The copy:** resolve `src` (must be a file — a directory source is `-EISDIR`; recursive `cp -r` is a
  JD9 candidate). Decide the destination: if `dst` resolves to an existing DIRECTORY the copy lands as
  `dst/<src-leaf>` (the `cp FILE DIR/` idiom); otherwise `dst` names the file (created, or truncated in
  place if it exists). Refuse copying a file onto itself (same canonical path) → `-EINVAL`.
- **Size handling (M2 decision):** the copy **streams** the source in fixed 32 KiB windows via `read_at`
  feeding `write_grow` — a file of ANY size copies with a bounded heap footprint and **no truncation** and
  **no size ceiling**. `read_at` is existing public `fat.rs` API (the U9/read-path twin of `read_file`), so
  no new primitive was needed; the per-window `write_grow` re-walks the growing destination chain (bounded,
  BOT-pumped — a stall is `-EIO`, never a hang). A future single-pass copy primitive could tighten that
  (JD9 note). Empty-file copy works (0 windows → a fresh 0-length destination).
- **Errno fidelity is shell-side** (same pattern as JD6/JD7, reusing `fat_errno` + shell-owned tags):
  src missing → `-ENOENT`; src is a dir → `-EISDIR`; dst parent missing → `-ENOENT`; dst parent is a
  file → `-ENOTDIR`; volume/dir full → `-ENOSPC`; a non-8.3 dst name → `-EINVAL`; copy-onto-self → `-EINVAL`.
- **Principal — unchanged** (EL1 ASID 0, PUBLIC): `cp` reads a public source and creates a public
  destination, no U6 ACL consulted (the trusted local console). No new lock/namespace surface — it composes
  the same F3-locked primitives JD6/JD7 already ledgered.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard);
  `esp-jetson` links, **108 `tegra:` strings** (unchanged — `cp` strings carry no `tegra:` token). Zero x86
  behavioural change (`shell.rs` compiles both arches; the `cp` handler is dispatched only on a keystroke).
- **Metal:** ✅ **METAL-CONFIRMED (2026-07-12 attended bench)** — the money-shot bench card
  [`jd8-bench.md`](../unaos/scripts/jd8-bench.md) PASSED on the Orin: `cp README.TXT COPIES/` (DIR/ idiom) +
  explicit-name `cp README.TXT COPIES/BACKUP.TXT`; power-cycle → both copies `cat`'d intact after re-boot,
  source byte-untouched. Serial `…-180110.log`; 0 heals across 4 boots.
- **Detail:** [`arch_arm64.md` §JD8](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** on `hw-jetson` (the
  seat assigns the integration hash at merge).

---

## storage lane (`us-unafs2`) — 2026-07-12 (UNAFS-2 — the kernel block adapter)

### UNAFS-2 — 512 B-sector device → unafs 4096 B `BlockDevice`, + GPT/MBR partition offsets 🔬 `us-unafs2`
- **What (host-native, `unaos/libs/fs/unafs/**` only):** the second link in the BeFS convergence chain
  (**BeFS-K2**). K1 gave a `no_std` unafs with a clean 4096 B `BlockDevice` seam; K2 makes a real kernel
  device speak it. New `adapter.rs`: a `no_std` + `alloc` module that presents the kernel's 512 B logical
  sectors (USB/SD, possibly at a partition offset) as unafs's 4096 B blocks. **Zero on-disk-format touch**
  — a pure block-level remap; the frozen serialization and its KATs are untouched.
- **The 512↔4096 mapping (M1):** `BlockAdapter<S>` implements `BlockDevice` over a generic `SectorDevice`
  trait (`read_sector`/`write_sector`/`sector_count`/`flush`). One 4096 B block maps to eight contiguous
  512 B sectors at `base_lba + block*8`, where `base_lba` is the partition offset. `block_count` bounds the
  exposed volume: reads/writes at or beyond it fail `OutOfBounds` before touching the device, and every
  `base_lba + id*8 + i` uses `checked_*` — a crafted or corrupt span can never wrap into an in-bounds
  sector. The seam is generic so it host-tests with `MemSectorDevice` (the 512 B twin of `MemDevice`) and
  the kernel wires its real driver in K3; `&mut S` is itself a `SectorDevice` so a device can be borrowed
  for a probe and handed back.
- **GPT/MBR partition parse (M2):** `parse_partitions` reads the MBR at LBA 0; on a protective GPT entry
  (type `0xEE`) it parses the GPT header at LBA 1 + its entry array, otherwise the four MBR primaries. Bound
  checks throughout: boot signature (`0x55AA`), GPT `"EFI PART"` signature, entry size (≥128) and count
  (≤65536) ranges, `last ≥ first` LBA ordering, and every partition extent validated against the device
  sector count (a partition running past the device is rejected `OutOfBounds`). `locate_unafs` parses the
  table then probes each partition's block 0 for the `UNAFS` superblock magic — identifying the volume by
  its on-disk signature, not a reserved partition type, so any partitioning tool's layout works — and
  returns a `PartitionSpan { base_lba, block_count }` for `BlockAdapter::for_partition`.
- **Seat additions folded in:** bincode pin tightened `"2"` → `"2.0"` (caret-breadth hardening; the 8 KATs
  pass unchanged after the pin — the format defence holds either way); the missing empty-`Vec<DirEntry>`
  KAT added (`0000000000000000` — a bare u64 length prefix of 0, the on-disk form of an empty directory).
- **Tested (host):** `cargo test -p unafs` green — **28 tests**: 6 adapter unit + 10 adapter fixture
  (synthetic GPT/MBR + bound-check negatives + `locate_unafs` magic-probe) + 8 KAT (unchanged count — the
  new empty-`Vec<DirEntry>` golden vector rides inside `kat_direntry`) + 4 pre-existing integration;
  `cargo check -p unafs --no-default-features --target
  aarch64-unknown-none-softfloat` clean (std genuinely absent — the module is `no_std` by construction);
  `cargo check` workspace default-members green (downstream unaffected); `./arroyo check` both arches green
  — **zero `unaos/` (kernel) diff** (K3 wires the driver, not this arc). Metal N/A (host-native library).
- **Ledger note (carried to K3):** the torn-mount warning in `fs.rs` is a `std`-gated `println!`; a `no_std`
  logging seam is wanted before the K3 kernel mount so a dirty-volume warning surfaces on metal. Recorded in
  the K3 next-baton draft.
- **Commits:** M1 mapping · M2 partitions · M3 no_std + docs (see `git log` on `us-unafs2`). Next in the
  BeFS chain: K3 read-only kernel mount of a real USB volume.

---

## storage lane (`us-unafs1`) — 2026-07-12 (UNAFS-1 — the `no_std` port, format frozen)

### UNAFS-1 — `unaos/libs/fs/unafs` → `#![no_std]` + `alloc`, byte layout pinned by golden KATs 🔬 `us-unafs1`
- **What (host-native, `unaos/libs/fs/unafs/**` only):** the road to security-K4-proper starts with a unafs the
  kernel can mount. This is **BeFS-K1**: make `unaos/libs/fs/unafs` compile `#![no_std]` + `alloc` *without changing
  one byte of the on-disk format*. A default-on `std` feature keeps every downstream consumer building
  unchanged; the host-native surface (`FileDevice`, the `io::MappedFile` memmap reader, the bandy event
  bus, the `sqrt`-using semantic query engine) sits behind it, while the on-disk types, the `codec` seam,
  the `BlockDevice` trait, `MemDevice`, and the core `UnaFS` ops are `no_std`.
- **Format-frozen migration:** bincode 1.3.3 → bincode 2.0.1, wrapped in its `legacy()` config
  (little-endian, fixed-int, no limit) behind a single `codec` seam that all 19 call sites route through.
  The bincode-2 split encode/decode errors are unified as `codec::CodecError` (`core::error::Error`, so
  thiserror `#[from]` works in both std and no_std).
- **The anchor — golden-vector KATs first (`tests/kat_vectors.rs`):** every struct that reaches disk —
  `Superblock`, `FileKind`, `Extent`, `AttributeValue`, `Inode`, `CatalogEntry`(+list), `JournalOp`,
  `DirEntry`(+list) — is asserted byte-for-byte against baked-in hex golden vectors (forward and
  roundtrip), with representative and boundary values. The KATs were frozen from the reference bincode-1.3
  encoding, and **passed unchanged** after the bincode-2 migration and the serde `default-features` change
  — the proof the format survived. The vectors are the contract; they are never edited to make a change pass.
- **Tested (host):** `cargo test -p unafs` green — 8 KAT tests + 4 pre-existing integration tests;
  `cargo check -p unafs --no-default-features` clean on host **and on the kernel's bare
  `aarch64-unknown-none-softfloat` target** (std genuinely absent); `cargo check` workspace default-members
  green (downstream unaffected); `./arroyo check` both arches green — **zero `unaos/` (kernel) diff**.
- **Also fixed (in-lane):** a pre-existing red — commit `44525c1` gave `UnaFS` a `Drop` impl, making
  `let device_back = fs.device;` illegal (E0509) in two tests and silently breaking `cargo test -p unafs`.
  Repaired by deriving `Clone` on `MemDevice` (additive) and cloning before the fs drops.
- **Commits:** `9e198a9` (M1 KATs) · `ac5030c` (M2 bincode 2.x) · `aa1dcd5` (M3 no_std). Metal N/A
  (host-native library). Next in the BeFS chain: K2 block adapter (512↔4096 + partition offsets).

---

---

## net-sock1 track — 2026-07-14 (SOCK-6 scope A — TCP server/listen sockets: ring 3 accepts inbound TCP)

### SOCK-6 — `sys_listen` + `sys_accept` (TCP server sockets) 🔬 `net-sock1`
- **What:** scope A of [§1b](ROADMAP.md) SOCK-6 — the **server** side of TCP for ring 3. Two syscalls over the
  persistent smoltcp stack: `sys_listen(handle, port)` (**#26**, arm a passive listener, `CAP_WRITE`) and
  `sys_accept(handle)` (**#27**, poll for an inbound connection, `CAP_READ`). Same `UNAOS_SMOLNET` knob, x86-only,
  byte-identical knob-off / aarch64. First ring-3 surface that **accepts inbound** TCP. Next free syscall: **28**.
- **The mechanism:** `stack_listen` calls `tcp::Socket::listen(port)` (passive, no pump); `stack_accept` pumps a
  bounded non-blocking loop (`ACCEPT_PUMP`, lock-released chunks via `tcp_pump_chunked`) chasing an inbound
  handshake — `Connected` / `Pending` (`-EAGAIN`, ring 3 re-drives, the `connect` poll model) / `NotListening`
  (`-EINVAL`). smoltcp's listener **becomes** the ESTABLISHED connection **in place** (it does not spawn a child),
  so `sys_accept` mints a **fresh `KIND_SOCKET` handle** aliasing the same gen-fenced socket-id (`sock_id_pack`;
  the SOCK-4 multi-handle-to-one-socket pattern) carrying `CAP_READ|CAP_WRITE|CAP_GRANT` — the accepted connection
  is itself transferable (accept→`SYS_XFER`→handler, inetd-style). Single-accept-per-listen (to accept again, open
  + listen a fresh socket). A UDP handle routed to `listen`/`accept` fails closed on the `SockKind` tag before
  smoltcp's typed accessor can panic. Fully static (reuses the SOCK-3 TCP stream rings; no new BSS/heap).
- **Evidence:** slirp's NAT will not open a connection INTO the guest, so the server witness uses the
  `UNAOS_NET=socket` builder mode SOCK-1 already wired (**no builder change**): `scripts/net-inject.py sock6`
  active-opens raw Ethernet frames to the guest's listener (ARP → 3-way handshake → probe), and the **stateful**
  BSP-main-loop witness `smolnet::witness_tick6()` (arm→accept→serve→done) accepts + echoes the probe:
  `:: SOCK-6: smoltcp tcp accept :8080 — received 11 bytes, echoed 11 back — witness OK ::` (confirmed on two
  runs; the injector prints `GUEST SOCK-6 SERVER OK`). Under the **default hermetic slirp** no peer connects in, so
  the witness prints the honest `:: SOCK-6: … listen :8080 armed … — witness PENDING ::` once and keeps listening
  cheaply — the mission stays green.
- **Gates:** hermetic `UNAOS_SMOLNET=1 ./arroyo test 90` MISSION SUCCESS + the SOCK-6 PENDING line, SOCK-1/2/3/5
  witnesses intact; server round-trip `witness OK` via `UNAOS_NET=socket UNAOS_SMOLNET=1` builder + net-inject
  (×2); knob-off `./arroyo test 25` / `test-arm 22` MISSION with NO SOCK-6 line (all code
  `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]` — byte-identical both arches); `check` both arches
  on+off, no new warnings. **Lane:** `arch/x86_64/syscall.rs` + `smolnet.rs` + `drivers/e1000.rs` (one witness
  call) + `scripts/net-inject.py` + docs; `crates/net`/`fat.rs`/`sched.rs`/`builder`/`Cargo.toml` untouched;
  **zero aarch64.**
- **Residuals:** single-accept-per-listen (smoltcp's listener becomes the connection; a persistent-listener
  acceptor pool needs decoupling the per-slot stream buffers from the registry slot — deferred to SOCK-7); the two
  syscalls are the delivered ring-3 surface, but the witness is a stateful BSP-loop poll wrapping the same
  `stack_listen`/`stack_accept` seam, not a dedicated ring-3 accept fixture (deferred); full `copy_from_user` for
  socket buffers still deferred. **Metal-pending** (no wired NIC on any current board — QEMU slirp / the
  socket-netdev injector is the honest gate).

---

## net-sock1 track — 2026-07-14 (SOCK-5 scope B — DHCP via smoltcp: the persistent stack leases its own address)

### SOCK-5 — smoltcp `dhcpv4::Socket` configures the persistent interface 🔬 `net-sock1`
- **What:** scope B of [§1b](ROADMAP.md) SOCK-5 — retire the persistent stack's knob-on dependency on the
  hand-rolled DHCP lease. The smoltcp interface used to copy a *static* address from `e1000::hw_addr()` (the
  address the hand-rolled `crates/net` DHCP had obtained); now it runs its **own** `dhcpv4::Socket` and applies
  the lease itself. Same `UNAOS_SMOLNET` knob, x86-only, byte-identical knob-off / aarch64. **No new syscall**
  (next free stays **26**) and **no new ring-3 surface** — DHCP is kernel-internal interface configuration.
- **The mechanism:** the socket-set storage grows to `NSOCK + 1` for a kernel-internal DHCP socket that is
  **never recorded in `reg`** (so `stack_open*` still sees exactly `NSOCK` ring-3 slots and no teardown touches
  it); `SmolStack` gains a `dhcp: Option<SocketHandle>`. `ensure_stack` builds the interface **address-less /
  route-less** and adds the DHCP socket; `configure_via_dhcp` (called once, right after the build, from the
  large-stack launcher/BSP-witness context) pumps a bounded, clock-free loop until `Event::Configured`, then
  applies the leased address (CIDR) + router (default gateway). On a silent server it **falls back to the static
  `hw_addr` lease + slirp gateway**, so SOCK-1/2/3/4 keep a configured interface either way. Fully static/BSS
  (the `dhcpv4::Socket` carries its own fixed internal storage; no heap).
- **Evidence:** a one-shot kernel witness `:: SOCK-5: smoltcp dhcpv4 lease 10.0.2.20/24 gw 10.0.2.2 — witness
  OK ::`. The proof is self-checking end-to-end: under slirp the lease is `10.0.2.20` — **not** the `10.0.2.15`
  static default the interface used to hard-code — and the SOCK-2 UDP-DNS, SOCK-3 TCP-DNS, and SOCK-4 transfer
  round-trips all still PASS on that DHCP-assigned address.
- **Gates:** `UNAOS_SMOLNET=1 ./arroyo test 90` MISSION SUCCESS + the SOCK-5 witness OK line, SOCK-1/2/3/4
  witnesses intact; knob-off `./arroyo test 25` / `test-arm 22` MISSION with NO SOCK-5 line (all code
  `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]` — byte-identical both arches); `check` both arches
  on+off, no new warnings. **Lane:** `smolnet.rs` + `Cargo.toml` (`socket-dhcpv4` feature) + docs;
  `arch/x86_64/syscall.rs`/`drivers/e1000.rs`/`crates/net`/`fat.rs`/`sched.rs` untouched; **zero aarch64.**
- **Residuals:** one-shot acquisition, no lease renewal (adequate under slirp's effectively-infinite lease; a
  future arc can pump the DHCP socket in `service_net` for renew/rebind); the hand-rolled `crates/net` DHCP
  still runs in the driver (it is the live stack knob-off, and knob-on still leases the driver's own `hw_addr` —
  both clients share the NIC's single MAC so slirp hands them the same address) — fully retiring it belongs to
  the eventual "retire the hand-rolled stack" arc. Metal-pending (SOCK has no metal leg — no wired NIC on any
  current board).

---

## net-sock1 track — 2026-07-14 (SOCK-4 scope B — transferable sockets: a socket cap moves across processes)

### SOCK-4 — transferable sockets (a `KIND_SOCKET` cap moves cross-row, gen-fenced) 🔬 `net-sock1`
- **What:** scope B of [§1b](ROADMAP.md) SOCK-4 — make a socket **capability movable to another process**
  (the U7x/U8x console-cap transfer, socket edition). **No new syscall** (next free stays **26**): a socket
  rides the existing `SYS_XFER`(13)/`SYS_RECV`(14) inbox machinery, which already special-cased `KIND_SOCKET`.
  Same `UNAOS_SMOLNET` knob, x86-only, byte-identical knob-off / aarch64.
- **The two changes that make it real:** (1) `sys_socket` now mints `CAP_READ|CAP_WRITE|CAP_GRANT` — SOCK-2/3
  minted no `CAP_GRANT`, so `SYS_XFER` (which demands it on the source) could never move a socket; `CAP_GRANT`
  cannot be self-added later (rights only attenuate), so transferability is endowed at mint. (2) `sys_recv`
  MIGRATES the socket's registry ownership to the grantee — `sock_valid` is owner-scoped, so a transferred
  handle would fail its owner CHECK unless the persistent socket's `reg` owner follows the cap. The install of
  a received `KIND_SOCKET` cap calls `smolnet::reassign_owner(sid, gen, new_row)` (`xfer_socket_migrate`):
  under the `STACK` lock, iff slot `sid` is present at the SAME generation, ownership moves — only the owner
  field, the smoltcp socket + buffers untouched (a MOVE, so a bound port survives the hand-off).
- **Single-owner, gen-fenced, safe by construction:** a socket has exactly one owner at any instant. After the
  move the grantor's original handle is owner-mismatched (`-EACCES`); `free_row_sockets` frees a socket only
  for its current owner; and the SOCK-3 gen fence rejects any stale old-gen handle to a freed+reused slot — no
  rebind (the U11x `file_desc_validate` discipline). A stale deposit (freed+reused between XFER and RECV)
  fails `reassign_owner`'s gen check → the received handle is dead-on-arrival, never stealing a tenant's
  socket. **This DISCHARGES the SOCK-2 review's warning** ("the moment a future arc makes a socket
  transferable this is a recycled-slot UAF") by construction.
- **Evidence:** a two-fixture ring-3 demo (`sock4-grantor`/`sock4-grantee`, the U7x idiom, on dedicated APs):
  the grantor mints a UDP socket, proves cross-process attenuation (over-rights `SYS_XFER` → `-EACCES`) and
  transfers it (dropping `CAP_GRANT`, single-level); the grantee `SYS_RECV`s it and completes a datagram
  round-trip to slirp's resolver **on the moved socket** (`bind`/`sendto`/`recvfrom` FROM `10.0.2.3:53`); the
  grantor's post-transfer `SYS_SENDTO` through its migrated-away handle is `-EACCES`; single-writer snapshot +
  teardown-clear hold. Plus a kernel-side `sock4_kernel_check` folding the **U11x gen-rebind proof** (grantee
  frees → gen bumps → a fresh socket first-fit-reuses the slot at the new gen → the old-gen handle stays
  `-EACCES`). `:: SOCK-4: transferable sockets — grantee received + round-tripped the moved socket, grantor's
  migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::`.
- **Gates:** `UNAOS_SMOLNET=1 ./arroyo test 90` MISSION SUCCESS + the SOCK-4 PASS line (3/3 deterministic),
  SOCK-1/2/3 witnesses intact; knob-off `./arroyo test`/`test-arm` MISSION with NO SOCK-4 lines (all code
  `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]` — byte-identical both arches); `check` both
  arches on+off, no new warnings. **Lane:** `arch/x86_64/syscall.rs` + `smolnet.rs`; `crates/net`/`fat.rs`/
  `sched.rs`/`Cargo.toml` untouched (reuses the existing socket features); **zero aarch64.**
- **Residuals:** it is a MOVE not a copy (the safe choice for a stateful owner-scoped resource); a double
  transfer is last-recv-wins (no UAF/leak); single-level (no re-delegation — socket revocation trees
  deferred); the demo round-trips UDP but a TCP socket transfers identically (the kind-agnostic kernel check
  covers it). Metal-pending (SOCK has no metal leg — no wired NIC on any current board).

---

## net-sock1 track — 2026-07-12 (SOCK-3 — TCP client sockets: ring 3 gets a byte stream)

### SOCK-3 — `sys_connect`/`sys_send`/`sys_sock_recv` (TCP client) over the persistent smoltcp stack 🔬 `net-sock1`
- **What:** the third [§1b](ROADMAP.md) arc — TCP **client** sockets (numbers **23–25**), ring 3's first
  byte stream. `sys_socket` gains `SOCK_STREAM(1)` → TCP; `sys_connect`(23) active-opens to a peer,
  `sys_send`(24) streams bytes, `sys_sock_recv`(25) reads them (named so — `SYS_RECV = 14` is the
  capability-transfer inbox recv). Same `UNAOS_SMOLNET` knob, x86-only, byte-identical knob-off / aarch64.
- **On the SOCK-2 stack:** a TCP socket rides the existing `STACK` singleton + `reg` registry; a slot now
  carries a `SockKind` tag and, for TCP, its own static stream ring buffers (`TCP_RX/TX_DATA`, 2 KiB each,
  BSS). UDP + TCP share one id space + one generation counter. smoltcp's `socket-tcp` feature added. A UDP
  handle routed to a stream syscall (or vice versa) is `-EACCES` on the kind tag — **before** smoltcp's
  typed accessor can panic (a kernel-integrity guard on a ring-3-chosen syscall/handle pairing).
- **The two SOCK-2-review REQUIRED folds, designed in:** (1) the socket handle value word is now
  **gen-fenced** `(gen<<32)|(sid+1)` (`sock_id_pack`), validated in `socket_id_of` → `smolnet::sock_valid`
  (present, owner-matched, generation-matched — the U11x `file_desc_validate` discipline); `SOCK_GEN[sid]`
  bumps on every free, so a stale handle to a freed+reused slot is `-EACCES`, no rebind — the recycled-slot
  UAF is closed for **all** socket kinds before any socket becomes transferable. (2) every TCP pump
  (`tcp_pump_chunked`) releases the `STACK` lock between `TCP_CHUNK`-sized chunks (re-validating the slot on
  re-acquire), so a cross-CPU socket syscall never spins on `STACK.lock()` for a full ~400 k-iter pump.
- **Non-blocking connect (the design crux):** the IF-masked handler cannot block a multi-RTT handshake, so
  `connect` is non-blocking with a ring-3 poll model — the first call issues the SYN (from state Closed;
  idempotent) and pumps a bounded loop, returning `0` established / `-EINPROGRESS` still handshaking (ring 3
  re-drives) / `-ECONNREFUSED` reset. `send` → count / `-EAGAIN` / `-ENOTCONN`; `recv` → count / `-EAGAIN` /
  `0` at clean end-of-stream (peer FIN + rx drained).
- **Witnesses (slirp resolver `10.0.2.3:53` over DNS-over-TCP — a hermetic 3-way handshake + stream reply
  under the DEFAULT `test` backend, no injector/netdev change):** a kernel-side M1 witness
  `:: SOCK-3: smoltcp tcp connect 10.0.2.3:53 established, 64 bytes back — witness OK ::` and a ring-3 M2
  fixture (`sock3-tcp`, the inline-blob idiom) that poll-connects/sends/poll-recvs and completes a
  round-trip: `:: SOCK-3: ring-3 tcp round-trip — socket/connect/send OK, recv returned a byte stream FROM
  10.0.2.3:53, socket teardown clean -> PASS ::`.
- **Tested (QEMU):** knob-off — `check` green both arches, `test 25` MISSION, `test-arm 22` MISSION (no
  SOCK-3 lines, byte-identical). Knob-on — `UNAOS_SMOLNET=1 check` green both arches, `UNAOS_SMOLNET=1 test
  90` MISSION + both witness lines above; SOCK-1/SOCK-2 witnesses intact; no new warnings. `SECURITY.md`
  gains its TCP row. Metal pending (SOCK has no metal leg — no wired NIC on any current board; QEMU slirp is
  the honest gate).
- **Lane:** `arch/x86_64/syscall.rs` + `smolnet.rs` + `drivers/e1000.rs` (additive) + `Cargo.toml`
  (`socket-tcp`); `crates/net`/`fat.rs`/`sched.rs`/`main.rs` untouched; zero aarch64.
- **Commits:** `8f40af6` (M1 — persistent TCP stack + gen-fence + kernel witness), `4db8167` (M2 — the
  stream syscalls + ring-3 fixture), M3 docs (this entry + `08_NET/networking.md` + `SECURITY.md` + ROADMAP §1b).

---

## net-sock1 track — 2026-07-12 (SOCK-2 — the UDP socket syscall family: ring 3 reaches the network)

### SOCK-2 — `sys_socket`/`bind`/`sendto`/`recvfrom` over a persistent smoltcp `SocketSet` 🔬 `net-sock1`
- **What:** the second [§1b](ROADMAP.md) arc — the UDP socket syscall family (numbers **19–22**), the
  first time ring 3 reaches the network. `sys_socket`(19) mints a UDP socket, `sys_bind`(20) names a
  local port, `sys_sendto`(21)/`sys_recvfrom`(22) move datagrams. Same `UNAOS_SMOLNET` knob, x86-only,
  byte-identical knob-off / aarch64.
- **Persistent stack:** `smolnet.rs` gains a persistent `Interface` + `SocketSet` singleton (`STACK`,
  a `spin::Mutex<Option<SmolStack>>` mirroring `NET_DEVICE`) that outlives individual syscalls (a UDP
  socket must survive between `bind` and `recvfrom`). **Fully static / BSS:** the socket-set storage,
  each socket's packet buffers, and the device RX/TX scratch are `&'static mut` (via `addr_of_mut!` +
  `from_raw_parts_mut`, no autoref-through-raw-deref); no heap. smoltcp's `socket-udp` feature added.
  The ~3 KiB device scratch lives in BSS so only smoltcp's ~2 KiB poll frames touch the caller stack,
  and only on the BSP-witness / IF-masked-syscall paths — never an AP scheduler stack.
- **Sockets as capabilities:** a socket is `KIND_SOCKET` (value word = the set-id, +1-biased) minted
  with `CAP_READ|CAP_WRITE`; send needs `CAP_WRITE`, recv needs `CAP_READ`, both at the SAME
  `handle_resolve` CHECK the File syscalls use — so `SYS_CAP` GRANT / `SYS_XFER` attenuate and transfer
  a socket cap for free. Freed at `clear_handle_row` teardown (`smolnet::free_row_sockets`).
- **Non-blocking recv:** the IF-masked handler cannot block, so `recvfrom` drives a BOUNDED poll pump
  and returns `-EAGAIN` when empty. `sendto`/`recvfrom` carry the peer address as an 8-byte header
  `[ip[4]][port u16 LE][pad]` + payload (fits the 3-arg x86 ABI); user buffers are window-bound-checked
  like `sys_write`/`sys_open`.
- **Witnesses (slirp DNS `10.0.2.3:53` — hermetic under default `test`, no injector/netdev change):**
  a kernel-side M1 witness `:: SOCK-2: smoltcp udp dns query 10.0.2.3:53 -> 64 bytes back — witness OK ::`
  and a ring-3 M2 fixture (`sock2-udp`, the u9x/u11x inline-blob idiom) that makes all four syscalls and
  completes a round-trip: `:: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a
  datagram FROM 10.0.2.3:53, socket teardown clean -> PASS ::`. The fixture runs on an AP racing the
  BSP's hand-rolled `service_net` poll, so it loops sendto+recvfrom (retry a stolen reply).
- **Tested (QEMU):** knob-off — `check` green both arches, `test 25` MISSION, `test-arm 22` MISSION
  (no SOCK-2 lines, byte-identical). Knob-on — `UNAOS_SMOLNET=1 check` green both arches,
  `UNAOS_SMOLNET=1 test 60` MISSION + both witness lines above; SOCK-1's ICMP witness intact; zero new
  warnings. `SECURITY.md` gains its first networking row. Metal pending (needs a wired NIC).
- **Lane:** `arch/x86_64/syscall.rs` + `smolnet.rs` + `drivers/e1000.rs` (additive) + `Cargo.toml`;
  `crates/net`/`fat.rs`/`sched.rs`/`main.rs` untouched; zero aarch64.

### SOCK-1 — smoltcp 0.13.1 + the e1000e `Device` adapter 🔬 `net-sock1`
- **What:** the first arc of the [§1b](ROADMAP.md) migration off the hand-rolled `crates/net` line
  onto **smoltcp** (0.13.1, 0BSD, `no_std`, static buffers). A new `crates/kernel/src/smolnet.rs`
  implements a `smoltcp::phy::Device` (`E1000Phy`) over the existing e1000e RX/TX rings, plus a
  throwaway-per-op `Interface` (10.0.2.15/24, gw 10.0.2.2) carrying an ICMP socket. Knob-on
  (`UNAOS_SMOLNET=1`) the shell's `ping` / `arp` / `netinfo` route through smoltcp; the hand-rolled
  engines stay compiled and still own `connect`/`fetch`/`udpsend` (SOCK-2/3 own the socket rewrite).
- **The Device seam (additive):** three new x86-only, feature-gated accessors on the driver —
  `e1000::raw_rx` (pop one raw L2 frame + recycle the descriptor, no `net::ingress` dispatch),
  `raw_tx` (thin wrapper over the private `transmit`), `hw_addr` (MAC / IP / link). The existing
  `poll()`/`transmit()` paths are untouched. Poll-driven only (never in the MSI handler); fully
  static / stack-local (no heap growth). ARP MAC surfacing: the Device snoops inbound ARP replies
  via `net::arp::learn` (read-only), since smoltcp hides the resolved neighbor MAC.
- **Byte-identical off, x86-only on:** `smoltcp` is an **x86-only optional dependency**; the
  `smolnet` feature + every call site is `#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]`.
  Knob-off pulls no smoltcp and is binary-identical to base (both arches); knob-on aarch64 resolves
  the feature to a no-op (never compiles smoltcp). Knob plumbed in `arroyo` **and**
  `builder/src/main.rs` (the builder rebuilds the kernel).
- **Features enabled:** `medium-ethernet`, `proto-ipv4`, `socket-icmp` (`default-features = false`;
  no `alloc`/`std`). No syscall surface — no `SECURITY.md` row yet (SOCK-2 brings that).
- **Tested (QEMU):** knob-off — `check` green both arches, `test 25` MISSION, `test-arm 22` MISSION
  (zero aarch64 impact). Knob-on — `UNAOS_SMOLNET=1 check` green, `UNAOS_SMOLNET=1 test 60` MISSION +
  the uncounted witness line `:: SOCK-1: smoltcp icmp echo 10.0.2.2 4/4 replies — witness OK ::`
  (slirp's gateway answered all four smoltcp-originated echoes). Metal pending (needs a wired NIC).

## hw-jetson track — 2026-07-12 (JD7 — the panel shapes the tree: `mkdir` / `rmdir`)

### JD7 — `mkdir` / `rmdir` on the Orin panel shell (thin FATDIRS glue) 🔬 `hw-jetson`
- **What (`shell.rs` only):** two new command handlers `fs_mkdir` / `fs_rmdir` + the `mkdir`/`rmdir`
  dispatch arms (DOS `md`/`rd` aliases). JD4 navigates, JD5/JD6 write; JD7 lets you *shape* the tree —
  `mkdir DOCS/DRAFTS`, `rmdir DOCS/OLD` — end to end from the console. It adds **NO `fat.rs` logic**:
  the directory-mutation seam already landed as the pi4-lane FATDIRS arc, and JD7 consumes
  `fat::create_dir`/`remove_dir` **call-never-edit**, exactly as JD6's write path rides `create_in_dir`.
- **`mkdir`:** reuses JD6's `resolve_write_target` to reach the parent, `locate_in_dir`s the leaf FIRST
  (`create_dir` inherits `create_in_dir`'s no-de-dup contract, so an existing name — file OR dir — is an
  honest `-EEXIST`, never a duplicate slot), then `create_dir` allocates + `.`/`..`-inits the child and
  links the parent DIR entry. Success echoes `created directory /DOCS/DRAFTS` (canonical spelling).
- **`rmdir`:** refuses the root LOCALLY first (`-EBUSY` — unnameable, cluster 0 not freeable; also catches
  `rmdir .` at root / `rmdir ..` popping to it), walks to the parent, pre-checks a FILE target → `-ENOTDIR`
  from the shell's own `is_dir` check, then `remove_dir` verifies emptiness (`.`/`..` only) and frees the
  cluster. `rm` stays file-only (a directory is still `-EISDIR`).
- **Errno fidelity is shell-side** (the seam reuses existing `FatError` variants — a new one would break
  `shell.rs`'s exhaustive `fat_errno` match): the shell resolves file-vs-dir-vs-root from the parent walk
  BEFORE the call and emits the POSIX tags itself — `-EEXIST` / `-ENOENT` / `-ENOTDIR` / `-ENOSPC` /
  `-EINVAL` / `-ENOTEMPTY` (mapped from the seam's `IsDirectory`) / `-EBUSY` (root).
- **Principal — unchanged** (EL1 ASID 0, PUBLIC). JD7 adds one caller-side ledger note: now that the EL1
  panel can `create_dir` too, the EL1-panel-vs-EL0 create/create race into the SAME directory is the same
  EXCLUDED_BY_SEQUENCING class as FATDIRS's ledgered `remove_dir` TOCTOU — no concurrent EL1 FS mutators
  run today; same future `fat.rs` namespace-lock fix. Recorded in `SECURITY.md`.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs` guard);
  `esp-jetson` links, **108 `tegra:` strings** (unchanged). Zero x86 behavioural change (`shell.rs` compiles
  both arches; the `mkdir`/`rmdir` handlers are dispatched only on a keystroke).
- **Metal:** ✅ **METAL-CONFIRMED (2026-07-12 attended bench)** — the money-shot bench card
  [`jd7-bench.md`](../unaos/scripts/jd7-bench.md) PASSED on the Orin: `mkdir DOCS/DRAFTS` → `cd` → `write
  NOTE.TXT` → power-cycle → the tree SURVIVED (`cd` back in, `cat` intact) → `rm` → empty `rmdir` freed its
  cluster; `-ENOTEMPTY`/`-EBUSY` probes honest. This also flips **FATDIRS**'s `create_dir`/`remove_dir`
  first-silicon verdict (they ran end-to-end on hardware here for the first time). Serial
  `~/unaos-bench/jetson-serial-2026-07-12-180110.log` (STEP-0 full-boot capture verified); 0 heals across 4 boots.
- **Detail:** [`arch_arm64.md` §JD7](dev/OS/01_BOOT_HAL/arch_arm64.md) + [`SECURITY.md`](SECURITY.md)
  FATDIRS ledger note. **Commit:** on `hw-jetson` (the seat assigns the integration hash at merge).

## hw-pi4 track — 2026-07-12 (K4-ready — the native-attribute projection codec, ahead of the mount)

### K4-ready — `PrincipalRecord`→native `owner`/`grants:*` string codec + `UNAATR1`/`UNAFS` discriminator 🔬 `hw-pi4`
- **Why:** the security K-line's K4 ("migrate-then-delete onto native unafs attributes") is gated on a
  native unafs FILESYSTEM in the kernel — the ROADMAP §2 BeFS convergence (no_std port → block adapter →
  read-only mount → journaled writes + a minimal VFS), NONE of which is in-tree (`fs/mod.rs` mounts only
  FAT; `unaos/libs/fs/unafs` is a std Ring-3 crate). So K4 proper is a multi-arc storage epic. This arc lands the
  ONE piece that needs no mount: the deterministic 1:1 CODEC the migration will use, PINNED + KAT'd now, so
  the exact projection is proven ahead of the mount (seat pick, Peter 2026-07-12).
- **What (`34cdb94`):** in-lane in `arch/aarch64/syscall.rs`, ZERO x86 — `principal_native_string`
  (`PROGRAM_NAME`→ stored `prog:<name>` verbatim; `IMAGE_SHA256`→ `sha256:` + 60 hex of the 30-byte digest
  PREFIX; `NONE`/reserved→ `None`, fail-closed), `grant_native_key` (`grants:<grantee>`),
  `rights_native_value` (`rw`/`r`/`w`/`-`), a no-heap `hex_lower_into`, and `classify_volume_magic`
  (`UNAATR1\0` sidecar vs `UNAFS` native superblock [mirrored from `unaos/libs/fs/unafs/src/superblock.rs`] vs
  Other) — the "tell the FAT bridge apart from a real unafs volume" primitive the FORMAT bullet names. Two
  stale in-lane comments reconciled (`no on-disk owner format yet`; `image_sha256`'s "71 chars at K4").
- **The 240-bit-prefix rule (load-bearing):** an `IMAGE_SHA256` principal stores only the 30-byte (240-bit)
  digest PREFIX; enforcement compares those bytes and `image_of` mints them, so the native `owner` string
  is the 67-char prefix form (`sha256:` + 60 hex), NOT the 71-char full digest (unreconstructable from
  disk). A full-digest projection would make a migrated owner mismatch a fresh mint → un-re-acquirable; the
  prefix form keeps migrated == fresh-mint byte-for-byte. Corrects the earlier IMG-SIG "71 chars" note.
- **Tested (QEMU):** `check` both arches; `kernel8` compiles; `kernel8-test 90` = **29 PASS (23 + CAPSTONE
  6) / 0 FAIL byte-equivalent** + all prior witnesses (K1-atr/persist/corrupt/K2-liveenf/K3-revoke/IMG-SIG/
  FATDIRS + F2/F3 locked 240000/240000) intact + the new uncounted `:: K4-ready: … PASS [w=0xff] ::` (8
  assertions incl. THE MIGRATION LANDMINE — a row round-tripped through the real `atr_serialize_row`/
  `atr_parse_row` codec projects identically to fresh mints), zero real R1/CMD13; `test-arm` MISSION
  SUCCESS. `k4_ready_selftest` is read-only, in-RAM, synthetic (no card, no disk).
- **Still deferred (K4 proper):** the native mount + the migrate-then-delete pass, gated on the BeFS chain.
- **Detail:** [`SECURITY.md` §K1 (K4-ready bullet)](SECURITY.md). **Commit:** `34cdb94` (`hw-pi4`); docs in
  the follow-on.

## hw-pi4 track — 2026-07-14 (K5 — the two ledgered UnaFS-ATR persistence races CLOSED)

### K5 — revoke/re-persist SMP-window (M1) + `atr_ensure` first-create race (M2) 🔬 `hw-pi4`
- **Why:** K3's 3-lens review ledgered two open UnaFS-ATR persistence races. (M1) A concurrent full-row
  re-persist (`atr_persist_grow` from a writer core, or a second owner-incarnation grant) that snapshotted
  OWNED_FILES between a two-phase revoke's disk-narrow and its in-RAM commit could write the still-present
  grant back to disk — RESURRECTING the revoked grantee at the next mount (a fail-OPEN in the SMP direction).
  (M2) `atr_ensure`'s first-CREATE of `UNAFS.ATR` was not `ns`-serialized (the F3 span rule forbids `ns`
  across a multi-cluster grow) — a benign-latent SMP double-create race. Both untriggered today (single EL0
  core) but closable ahead of SMP EL0; both `arch/aarch64/syscall.rs`-only, zero x86.
- **What (M1 — the lock-span):** the two-phase revoke (`sys_fgrant_revoke_2phase`) now holds `ns` across
  snapshot→disk-narrow→in-RAM-commit (one span), and every full-row re-persister (`atr_persist_grants`,
  `atr_persist_grow`) takes its OWNED_FILES snapshot UNDER that same `ns` — gated on a light NAMED-owner
  probe first, so the anonymous battery path takes NO `ns` and does ZERO disk I/O (byte-identical). The disk
  half was extracted to `atr_write_grant_row_locked` (ns-assumed). **Lock-legality:** NAMESPACE is the legal
  spanning lock — `NAMESPACE ⊃ OWNED_FILES` is respected (the OWNED_FILES helpers take-and-release WITHIN,
  so no inner lock is held when `ns` is taken → deadlock-free), and the disk op is the single-sector M1
  `write_at` seam (NOT a grow), so the F3 ns-latency rule holds. The re-persisters and the revoke then
  serialize globally; no stale snapshot can land after a disk-narrow.
- **What (M2 — the create gate):** a dedicated lock-free CAS gate (`ATR_CREATING`) serializes the
  create-if-absent DECISION and double-checks under the gate, WITHOUT holding a lock across the grow (winner
  grows; a contender BAILS its persist rather than spins) — F3-safe. A read-only fast path means steady-state
  persists never contend the gate. **Honest residual:** a persist that loses the CAS is deferred this pass —
  never double-creates, degradation always toward fail-safe (not-yet-persisted).
- **Tested (QEMU):** `check` both arches (x86 unchanged); `kernel8-test` = **23 PASS + CAPSTONE 6 / 0 FAIL
  byte-equivalent** + all prior witnesses (K1-atr/persist/corrupt/K2-liveenf/K3-revoke/IMG-SIG/K4-ready +
  F3 locked 240000/240000) intact + the new uncounted `:: K5-lockspan: … PASS [w=0x3f] ::` (6 assertions:
  reproduces the OLD resurrection at the decomposed-primitive level — the window is REAL — then shows the
  production revoke + re-persist stays narrowed across reboot, the kept grant survives, the create gate is
  not leaked), zero R1/CMD13; `test-arm` MISSION SUCCESS. Deterministic single-core; the full cross-core
  timing race is metal-latent (like the F3 witness).
- **Detail:** [`SECURITY.md` §K1 (K5 bullet)](SECURITY.md). **Commit:** `unafs(aarch64): K5 —
  lock-span the revoke/re-persist window + serialize the ATR first-create` (`hw-pi4`).

## hw-pi4 track — 2026-07-12 (FATMOVE — the fat.rs rename/move seam: `rename_entry` / `move_entry`)

### FATMOVE — `rename_entry` / `move_entry`: additive, lock-correct FAT rename + cross-directory move 🔬 `hw-pi4`
- **Why:** `mv` is the last unimplemented classic file-manager verb on the Orin panel, and its
  move-across-directories needs a genuinely-new `fat.rs` mutation — "unlink the source dir entry WITHOUT
  freeing the chain" (no existing public twin). A mutation bug in arch-shared `fat.rs` hits all three
  platforms, so the seam lands as a **dedicated pi4-lane arc** (the F2/F3/K*/FATDIRS lock lineage is this
  track's expertise), which a future jetson `mv` arc (JD10) consumes call-never-edit (the FATDIRS/JD7 split).
- **What:** two new public `fat.rs` methods next to the FATDIRS block, **ZERO edits to any existing fn**:
  `rename_entry(parent_first_cluster, old_leaf, new_leaf) -> (DirEntry, u64, usize)` (rewrite the 8.3 name
  IN PLACE — a SINGLE dir-sector RMW; `first_cluster`/`size`/`attr` unchanged; works on files AND dirs,
  since an in-place rename never disturbs `.`/`..`) and `move_entry(src_parent, leaf, dst_parent, new_leaf)
  -> (DirEntry, u64, usize)` (write the destination entry over the SAME `first_cluster`/`size`, then `0xE5`
  the source WITHOUT freeing the chain — the data clusters move BY REFERENCE), plus one private single-sector
  `write_dir_entry_name`. They compose `locate_in_dir`/`create_in_dir` + `write_dir_entry_fields`/
  `mark_dir_deleted` (each already riding `DIR_MUTATION`); no `alloc_cluster`, no `free_chain`. The source's
  exact attribute byte is preserved across a move.
- **Crash ordering (invariant 2 — NEVER lose the chain):** `move_entry` publishes the DESTINATION entry
  FIRST, then `0xE5`s the source. A crash between the two leaves a benign DUPLICATE (two names, one chain);
  the operator removes the unwanted one by its entry (`rm OLDNAME`). The reverse order could orphan the
  chain — forbidden. Every window fails toward a leaked/duplicate name, never a lost or aliased chain.
- **Directories (invariant 3):** `rename_entry` renames a directory in place (allowed — `..` untouched);
  `move_entry` of a directory across parents is REFUSED (`IsDirectory`; the `..` rewrite is out of scope).
- **Locking / honest residual (invariants 4 + 5):** every sector RMW is SMP-atomic via the existing
  `DIR_MUTATION` span, never widened to cover both of `move_entry`'s two dir-sector RMWs (cross-sector
  atomicity is **EXCLUDED_BY_SEQUENCING** for EL1 callers — the SAME class FATDIRS ledgered, closed by the
  SAME future `fat.rs` namespace lock). Sound WITHOUT the syscall NAMESPACE lock (EL1 shell callers reach
  `fat.rs` directly). **ACL re-key flag:** the `OWNED_FILES` ACL keys by `(dir_lba, dir_off)` — an in-place
  rename preserves the key, but a `move_entry` writes a new slot, so a future EL0 move of an OWNED file MUST
  re-key the ACL row (+ persisted `UNAFS.ATR` fields) or refuse; ledgered for the K-line + JD10 (no EL0
  plumbing this arc; the EL1 panel is ASID-0 public). **Error fidelity flag:** reuses existing `FatError`
  variants — dest-exists → `Unsupported` (caller pre-checks + surfaces `-EEXIST` locally), dir-move →
  `IsDirectory` (`-EISDIR`); enriching `FatError` is a future jetson-lane change.
- **Tested (QEMU):** `check` green both arches; `kernel8` compiles; `kernel8-test 35` = **23 PASS
  byte-identical** + CAPSTONE 6/6 + all prior witnesses (K1-atr/persist/corrupt, K2-liveenf, K3-revoke,
  IMG-SIG, FATDIRS, K4-ready, F2/F3 locked 240000/240000) intact + the new uncounted
  `:: FATMOVE: … PASS [w=0x1ff] ::` (9 assertions: rename same-head+size / old-gone-new=same-chain / content
  intact / onto-existing refused; move cross-dir by reference / content intact / onto-existing refused /
  directory refused / empty 0-cluster file relinked), zero FAIL, zero R1/CMD13; `test-arm 22` MISSION
  SUCCESS. Zero x86 behavioural change
  (additive; no x86 caller). The `fatmove_check` selftest is fully self-cleaning (leaves the volume pristine).
- **Metal:** the seam's attended money-shot rides a future jetson `mv` (JD10) Orin panel bench, sequenced
  per the code-prerequisite rule; a Pi-side exercise of the witness batches onto the next Pi bench.
- **Detail:** [`SECURITY.md`](SECURITY.md) aarch64 ledger (FATMOVE entry). **Commit:** on `hw-pi4` (the seat
  assigns the integration hash at merge).

## hw-pi4 track — 2026-07-12 (FATDIRS — the fat.rs directory-mutation seam: `create_dir` / `remove_dir`)

### FATDIRS — `create_dir` / `remove_dir`: additive, lock-correct FAT directory create + remove 🔬 `hw-pi4`
- **Why:** the panel's `mkdir`/`rmdir` (jetson JD7) needs directory-mutation logic bigger than JD6's
  thin additive wrappers, and a dir-mutation bug in arch-shared `fat.rs` hits all three platforms —
  so the seam lands as a **dedicated pi4-lane arc** (the F2/F3/K* lock lineage is this track's
  expertise), which JD7 then consumes call-never-edit.
- **What (`cdfe25b`):** two new public `fat.rs` methods next to JD6's dir-aware twins, **ZERO edits to
  any existing fn** (the JD6 additive-exception pattern, now in-lane): `create_dir(parent_first_cluster,
  name) -> (DirEntry, u64, usize)` and `remove_dir(parent_first_cluster, name) -> Vec<u32>` (freed
  clusters), plus one private `init_subdir_cluster`. `create_dir` composes `alloc_cluster`
  (compare-and-claim under `FAT_MUTATION`) → writes `.`/`..` into the zero-filled UNLINKED orphan
  cluster (no lock — unreachable) → `create_in_dir` (0-cluster DIR entry) → `write_dir_entry_fields`
  (publishes the child cluster). `remove_dir` locates, refuses a non-directory and a `first_cluster==0`
  root-like target, verifies the target holds ONLY `.`/`..` (the `read_dir` walk), then `delete_located`
  (0xE5 first, then `free_chain`).
- **Crash ordering (invariant 2):** the child cluster is fully initialized BEFORE the parent link; a
  crash leaks a cluster or leaves the JD6-ledgered `FstClus==0` corner — never a live entry over a
  cluster that later gets freed/aliased.
- **Locking / honest residual (invariant 3 + 5):** every sector RMW is SMP-atomic via the existing
  per-RMW locks; `DIR_MUTATION` is never widened past its documented single-sector span. The one
  residual — `remove_dir`'s emptiness-scan → delete not atomic vs a concurrent `create_in_dir` into
  the same target — is **EXCLUDED_BY_SEQUENCING** today (no concurrent EL1 FS mutators; EL0 rides the
  syscall NAMESPACE lock) and ledgered in `SECURITY.md` like F3's interleave. The internal locking is
  sound WITHOUT the syscall NAMESPACE lock (EL1 shell callers reach `fat.rs` directly). **Error
  fidelity flag:** reuses existing `FatError` variants (adding one would break shell.rs's exhaustive
  `fat_errno` match in the jetson lane) — `-ENOTDIR`/`-ENOTEMPTY` map to `Unsupported`/`IsDirectory`
  today; enriching `FatError` is a future jetson-lane seam change, flagged for JD7.
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `kernel8` compiles;
  `kernel8-test 30` = **23 PASS byte-identical** + CAPSTONE 6/6 + all prior witnesses
  (K1-persist/K1-corrupt/K2-liveenf/K3-revoke/IMG-SIG/F2/F3) intact + the new uncounted
  `:: FATDIRS: … PASS [w=0xff] ::` (8 assertions: `.`/`..` well-formed, file-in-dir, non-empty refused,
  empty rmdir frees+reuses the cluster, root-like + file targets refused), zero FAIL; `test-arm 22`
  MISSION SUCCESS. Zero x86 behavioural change (additive; no x86 caller). The `fatdirs_check` selftest
  is fully self-cleaning (leaves the volume pristine).
- **Metal:** the seam's attended money-shot rides **JD7's Orin panel bench** (`mkdir`/`rmdir` through
  the shell), sequenced per the code-prerequisite rule; a Pi-side exercise of the witness batches onto
  the next Pi bench opportunistically.
- **Detail:** [`SECURITY.md`](SECURITY.md) aarch64 ledger (FATDIRS entry) + [`arch_arm64.md`
  §FATDIRS](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** `cdfe25b` (`hw-pi4`).

## hw-jetson track — 2026-07-11 (JB1f — the unhealed early-vector window, closed)

### JB1f — the healed vectors now cover the whole tegra boot; nest-safe, storm-proof EC0 heal ✅ `hw-jetson`
- **What (`85f74f8`):** the round-6 bench caught the A78AE-1941500 phantom striking fbcon's glyph loop
  fatally, 2/2 boots, inside the window between `mmu_tegra`'s probe-and-spin Part-C vectors (installed
  at the MMU switch) and `exceptions::install` at JM4 — the stretch that mirrors the whole early boot
  log onto the panel, with no heal armed. Fix: (1) install the healed `exceptions.rs` vectors right
  after the mmu-regs banner, before fbcon mirrors (Part C keeps the three-line switch window);
  (2) `__vec_sync` banks ELR/SPSR/SP_EL0 with a runtime-`CurrentEL` bank select, so a nested sync
  fault inside the handler can't retarget the heal's eret (the diagnosis panel's confirmed latent
  defect #1); (3) heal budget 64 → 1024 + a consecutive-same-PC cap (32, the wedged-core stop) +
  `fetch_add` counters + print dedup + a nonzero heal tally at the fatal print and at `install()`
  (defect #2). The diagnosis panel **exonerated the VPERF video rewrite** (all deltas
  `cfg(x86_64)`; padded stride already handled) — root cause is erratum 1941500 relocated by binary
  layout shift; the pattern-sensitivity ledger note lives in `arch_arm64.md §JB1f`.
- **Tested (QEMU, byte-equivalent — the heal never fires there):** `check` + `UNAOS_TEGRA=1 check`
  both arches; `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `kernel8` +
  `kernel8-test` 23 PASS 0 FAIL + CAPSTONE 6/6 + K1 witnesses; `esp-jetson` links, 108 `tegra:`
  strings. Zero x86 delta. **Metal — ✅ CONFIRMED (attended, 2026-07-11, panel-observed, operator
  verdict "pass 100%"):** boots survived `panel LIVE` (the stretch that killed `446abd3` 2/2 that
  morning) through to the interactive shell, and the JD6 bench card completed on the same kernel —
  JD6 flips ✅ with it. ⚠ Host-side serial capture failed mid-bench (no replay log / heal tally);
  detail + the bridge follow-up in `arch_arm64.md §JB1f`.

## hw-jetson track — 2026-07-11 (JD6 — the panel write path reaches the whole tree: subdirectory writes)

### JD6 — `touch` / `write` / `append` / `rm` in ANY subdirectory the shell can `cd` into ✅ `hw-jetson`
- **What (M1, `446b986`):** JD5 was root-only because `fat.rs`'s public mutation API
  (`create_in_root`/`find_located`) hard-codes the root; `fat.rs` mutation is the pi4-K1 lane. Under a
  **round-6 seat-granted narrow ADDITIVE exception** (ccd-coordinated at GATE-0; JD4's `read_dir`
  precedent), two new public `fat.rs` wrappers land adjacent to their root twins, **zero edits to any
  existing fn:** `locate_in_dir(first_cluster, name)` (0 ⇒ `find_located`, else the existing private
  `locate_in_dir_chain`) and `create_in_dir(first_cluster, name, attr)` (0 ⇒ `create_in_root`, else the
  existing private `free_slot_in_dir_chain` + a **verbatim** copy of `create_in_root`'s
  `with_dir_lock` slot-write RMW, both sites cross-referenced "twin — keep in sync"). Every mutation
  rides `DIR_MUTATION`/`FAT_MUTATION` exactly as the root twin; it allocates no clusters, touches no
  FAT. `shell.rs` gains `resolve_write_target` (walks to the parent dir via the read-only
  `resolve_path` → `(parent_first_cluster, leaf, parent_canon)`; root ⇒ 0) and rewires `fs_touch`.
- **What (M2, `a3bc06a`):** `fs_write` routes through the dir-aware twins — `write DOCS/NOTE.TXT hello`
  creates-or-truncates in a subdir; semantics unchanged; the raw `write <lba> <byte>` block form
  untouched (dispatched separately). **What (M3, `2e9ca1b`):** `fs_append`/`fs_rm` routed the same way;
  `resolve_root_name` (the JD5 root-only resolver) retired; DESIGN-NOTE scope updated to whole-tree.
- **Principal — unchanged:** subdirs don't change the principal (EL1 ASID 0 = PUBLIC; §JD5). **Honest
  edges:** parent-is-a-file → `-ENOTDIR`, missing parent → `-ENOENT`, root target → `-EISDIR`, FULL
  directory → `-ENOSPC` (**no subdir-chain extension this arc**), directory `rm` → `-EISDIR` (**`rmdir`
  out of scope** — needs emptiness + `.`/`..` handling + a `fat.rs` primitive this track lacks).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `test-arm 22` MISSION;
  `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 test 25` MISSION (shared `shell.rs`
  guard); `esp-jetson` links, **108 `tegra:` strings** (unchanged — validate by count, not size). Same
  as JD2–JD5 the shell arms dispatch only on a keystroke and tegra never runs in QEMU, so the
  shell-level verdict is **attended-pending**; the twins call the same F3-locked mutation the
  U9/U10/U11 fixtures already exercise headless.
- **Metal — ✅ CONFIRMED (attended, 2026-07-11, panel-observed, "pass 100%"):** the subdir
  money-shot (`cd DOCS`, `write NOTE.TXT …`, power-cycle, `cd DOCS`, `cat`) completed on the
  round-6 bench, on the JB1f-fixed kernel that unblocked it (card `unaos/scripts/jd6-bench.md`;
  serial-capture caveat in `arch_arm64.md §JB1f`).
- **Detail:** [`arch_arm64.md` §JD6](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commits:** `446b986` M1 ·
  `a3bc06a` M2 · `2e9ca1b` M3 (`hw-jetson`).

## ux lane — 2026-07-10 (UI-1 — scale-aware UI, the one-cell cursor, the CPU pulse redesign, `pulse`)

### UI-1 — the UI metrics layer + CPU pulse row + full-screen `pulse` (x86 + aarch64) 🔬 `ux-ui1`
- **What (M1, `76b1136`):** THE METRICS RULE lands as a standing directive — *no absolute pixel
  sizes in UI code*. New `ui.rs` derives an integer `SCALE` from the panel height at surface init
  (`clamp(h/900, 1, 4)`: 1 at ≤900p, 2 at 1800p-class, cap 4) and every UI dimension follows
  (`cell = 8·scale`, `line_h = cell + cell/2`, `margin = line_h`); `GneissPal` grows a provided
  `metrics()`; `draw_text` renders each glyph pixel as a `scale`×`scale` block (scale-1 path
  unchanged). `console.rs` converts off its hardcoded TOP/LINE_H/20-px insets onto the metrics, the
  prompt/input/cursor drawing is factored into one shared path, and **the cursor is BY CONSTRUCTION
  exactly one metrics cell** — fixing the 8×16 cursor standing double the 8×8 text height (the
  clear strip is now exactly `line_h`). `Console::page_rows` (the pager's single source of truth)
  now computes from the derived `line_h`. Evidence at every surface bring-up:
  `:: UI1: scale=N cell=WxH line=H ::`.
- **What (M2, `042eb28`):** the vug CPU meter becomes Peter's sketch — one horizontal row of
  per-core NUMBERED segment bars `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …` (10 fixed segments; filled ∝ load;
  empty segments dim so an idle core reads alive-but-empty, never blank), scale-aware, for however
  many cores `sched::meter_cpu_count()` reports. Sampling factored into the shared `CpuPulse`;
  the **honest two-source rule (VUG-1 M3b) kept verbatim** (sched busy-fraction for scheduler-
  accounted cores — the Orin path, not regressed; own render busy% for the unscheduled demo core,
  logged once). RENDER meter stays.
- **What (M3, `5527160`):** full-screen system monitor (BeOS Pulse homage), `vug::run_pulse` + a `pulse`
  shell arm — the M2 widget larger (double-size segments, one row per core, load %), plus the
  honest system lines available today (core count, uptime ms, live frame counter, frame time +
  FPS while open). vug loop contract: pump-own-input, one present per frame, busy-poll +
  `yield_now` (never WFI — the JB2b/JC3 rule), any key exits, `took_screen` honored. Serial:
  `:: PULSE: live — N cores ::` / `:: PULSE: exit clean — N frames ::`.
- **How tested (QEMU):** per milestone: `./arroyo check` + `UNAOS_TEGRA=1` check green (both
  arches); `test 25` → MISSION + 17 PASS (= base); `test-arm 22` → MISSION (= base);
  `kernel8-test 30` → 23 PASS **set-identical to base** + CAPSTONE 6/6 (UI changes perturb no
  serial fixture). Scripted headless QMP runs (real usb-kbd path): typed `vug` → crystal live/exit
  clean + the numbered pulse row on the screendump (core 1 filled, cores 2–4 dim-but-alive); typed
  `pulse` → live — 4 cores, 963 frames, exit clean, console restored with the one-cell cursor
  (screendumps at 1280×800; the Pi 640×480 shot covers the small end). Attended visual verdict:
  Peter's, at the QEMU GUI / next benches (scale=2 needs the Retina rMBP panel — every QEMU
  target reports ≤900p, so scale>1 is metal-attended).
- **Docs:** `docs/dev/OS/08_VIDEO/engine.md` §0 (THE METRICS RULE — the standing directive), §4
  (pulse-row redesign), §4b (`pulse`); this entry.

## hw-jetson track — 2026-07-10 (JD5 — the write path: the panel becomes a real workstation shell; same-day attended bench — PASS)

### JD5 — `touch` / `write` / `append` / `rm` / `sync` on the panel shell ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What (M1, arch-neutral, `3a143f5`):** `touch <path>` creates a 0-length root file (idempotent);
  `write <path> <text>` is create-or-TRUNCATE storing the exact bytes (truncate = `delete_located` +
  `create_in_root` + `write_grow` — the only create-or-truncate through the PUBLIC API; no in-place
  shrink primitive). The raw `write <lba> <byte>` stays byte-identical for its 2-numeric-arg shape;
  any other shape is the file write. Rides `fat.rs`'s F3-locked PUBLIC mutation API directly (the SVC
  path is EL0/ASID-keyed + out-of-lane; the shell runs at EL1 as **ASID 0** → an ASID-0 create is
  PUBLIC by U6's rule, so shell files are plain public FAT files). **Root-directory only** —
  `create_in_root`/`find_located` are root-only and `fat.rs` is call-never-edit, so a subdir target
  is an honest `-ENOTSUP`.
- **What (M2, arch-neutral, `dfaf180`):** `append <path> <text>` = open-seek-end-write via
  `write_grow` (create-if-absent, like `>>`); `rm <path>` (alias `del`) = `delete_located` (mark
  `0xE5` first, then free the chain — crash-safe order), directory → `-EISDIR`, absent → `-ENOENT`.
- **What (M3, `2531209`):** safety rails made explicit — writes are BOUNDED (`block::write_block`
  rides the SAME JD3 wall-clock BOT pump as reads, so a stalled write times out to `-EIO`, never
  WFI-parks the timerless EL1 core — verified in the driver), CONSISTENT (each `fat.rs` step atomic
  under F3's `FAT_MUTATION`/`DIR_MUTATION`; a mid-sequence failure leaves lost clusters / the old
  smaller size, never a torn volume), and WRITE-THROUGH (`sync` is an honest no-op — no cache).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `UNAOS_HUBSTORAGE=1 test 25`
  MISSION (shared `shell.rs` guard); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `esp-jetson` links, **108 `tegra:` strings** (unchanged — shell-write strings carry no `tegra:`
  token; validate by count, not size). The write PRIMITIVES already run headless via the
  `el0-u10create`/`u10delete`/`u11close` fixtures (identical `create_in_root`/`write_grow`/
  `delete_located`); the SHELL arms are thin glue, dispatched only on a keystroke, so a headless
  shell-write demo is not in-lane — verdict attended-pending like JD2/JD3/JD4.
- **Metal — ✅ PASS (2026-07-10 attended bench, serial `jetson-serial-2026-07-10-165211.log`):** the
  whole battery on the FAT16 `UNAOSRW` card (29 MiB, Alcor reader behind the hub, slot 5; clean enum
  both boots) — boot 1 `write hello.txt` → `cat` → `append` → `cat`, **power-cycle**, boot 2
  `cat hello.txt` **survives** (write-through durability on silicon), `rm` → `-ENOENT`,
  `write docs/x.txt`/`docs/y.txt` → `-ENOTSUP` (file confirmed never created; `ls docs` still works),
  all typed lowercase (case-insensitive 8.3). Zero `BOT pump TIMEOUT`/errors/panics both boots. ⚠ the
  rMBP `UNAOS` card was repurposed as the Orin boot stick — rMBP must re-flash its boot media.
- **Detail:** [`arch_arm64.md` §JD5](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commits:** `3a143f5` M1 ·
  `dfaf180` M2 · `2531209` M3 (`hw-jetson`).

## hw-pi4 track — 2026-07-10 (K1 M1 — the on-disk owner/grants format; persistence stops for a seat decision)

### K1 M1 — `UNAFS.ATR`: persist the U6 ACL across reboot — FORMAT + round-trip 🔬 `hw-pi4`
- **What:** U6's owner/grants ACL is in-RAM and boot-scoped (the owner is an `(asid, gen)`
  incarnation), so a power-cycle reverts every private file to PUBLIC. K1 persists owner + grants
  as on-disk attributes inside a reserved hidden|system FAT file `UNAFS.ATR`. **M1 lands the on-disk
  FORMAT** (a versioned magic header + 16 bounded 256-byte rows — mirroring the 16-row in-RAM table
  — each with a 32-byte kind-tagged owner `PrincipalRecord` + 4 grants, per-header and per-row CRC32,
  a volume binding; single-row `write_at` = one-sector RMW), **its (de)serializers, reserved-file
  helpers built entirely on `fat.rs`'s existing public API (zero `fat.rs` edit), and a round-trip
  self-test** — codec (populated-row byte/field round-trip + CRC-corrupt/bad-magic/wrong-binding
  fail-closed) and on-disk (create the empty image, write one SYNTHETIC row via `write_at` UNDER the
  F3 `NAMESPACE` lock, read back byte-equal, clear it → a valid all-public image).
- **STOP-GATE:** the persistent-principal model is genuinely undefined — a persisted owner cannot be
  a boot-scoped `(asid, gen)`, and UnaOS has no persistent principal (EL0 blobs loaded by name, no
  code-signing / manifest / RTC / uid). Defining it is a TCB-level policy decision above the pi4
  aarch64 lane, so **M1 STOPS and PROPOSES to the seat** a launcher-assigned, kernel-stamped
  principal (default `PROGRAM_NAME`), and wires NO enforcement (enforcement-inert: the 23-PASS
  battery is byte-identical; K1 emits its own `:: K1-atr: ::` line, not a `-> PASS`). Persist +
  mount-rebuild + enforcement are M2/M3/M4, seat-gated.
- **Tested (QEMU):** `check` green both arches (zero x86 change); `kernel8` compiles baremetal;
  `kernel8-test 40` → **23 PASS byte-identical** (sorted diff vs base `f2ad34c`) + CAPSTONE 6/6 +
  F2-witness (240000/240000 locked) + F3-witness (240000/240000 locked) + `:: K1-atr: codec PASS …
  on-disk helpers disk PASS … ENFORCEMENT-INERT ::`, zero R1/CMD13; `test-arm 30` MISSION SUCCESS.
- **Metal:** the disk round-trip creates a real `UNAFS.ATR` on the card (valid, empty/all-public) —
  metal-verify at the next attended bench (a fresh-card re-prep should delete `UNAFS.ATR` too).
- **Detail:** [`SECURITY.md` §K1 M1](SECURITY.md) (the full design section: principal proposal,
  format, fail-closed asymmetry, lock discipline, seat question). **Commit:** the single K1 M1
  commit on `hw-pi4` (see `git log`).

## hw-rmbp track — 2026-07-10 (STOR-1 S1–S3 — interrupt-driven x86 storage behind the `irqstorage` knob; ✅ core mechanism METAL-CONFIRMED)

### STOR-1 S1–S3 — the storage service task + live reads + live in-place write-through ✅ core mechanism METAL-CONFIRMED (2026-07-10) `hw-rmbp`
- **What:** x86 storage syscalls run IF-masked (SFMASK), and the only kernel→sector path is the xHCI
  BOT pump, which `hlt()`s awaiting a transfer event — a `hlt` at IF=0 never wakes, so an in-handler
  disk op hangs the core. The whole staged-buffer family (U6bx staged read / U9x flush queue / U10x
  op-queue) exists to defer disk work to an IF=1 context. STOR-1 lays the IF-safe replacement spine
  (design `unaos/docs/dev/OS/07_USB_STORAGE/x86_interrupt_storage.md`): a scheduled kernel **storage service
  task** (IF=1) owns the BOT pump; a syscall builds a `BlockRequest` on its kernel stack, wakes the
  service task, and **blocks on a per-request semaphore** whose `wait` restores the caller's IF
  snapshot across the switch — so the handler sleeps with IF=1 semantics though it entered IF-masked
  (the crux). All wakeups happen at IF=1, so the wake-only MSI-X handler stays lock-free
  (`sched.rs:28` preserved). **S1** (`7b2f05f`): the service task + submit/block/complete + a
  `bx-blockreq` self-test (LBA0 polled vs service-task read → PASS); a bounded pump timeout →
  `BlockError::Io`, never a hang. **S2** (`b73ba08`): `sys_read` serves a RO staged descriptor from
  the LIVE volume (name-based `find_located`+`read_at`), bounced through a kernel-stack buffer (the
  ring-3 window is the submitter's private CR3 = PML4[2], unreachable by the service task under kernel
  CR3; the kernel stack is in the shared kernel half). **S3** (`fd5de85`): `sys_write_file` writes
  THROUGH in place for a non-growable staged descriptor (SCRATCH.BIN) — synchronous `write_at`, no
  wstage/dirty/FLUSH-queue; the matching read broadens to `FILE_OPNAME==0`; a new witness proves the
  write is on disk pre-drain with an empty flush queue → **close-discards-dirty residual retired**.
- **Knob:** everything behind the `irqstorage` cargo feature (`UNAOS_IRQSTORAGE=1`; mapped in
  `builder` + `arroyo`), x86_64 only — the default build never links `drivers/xhci/irqstorage.rs`, so
  the staged path is **byte-identical**. The live path is per-descriptor + coherent: it fires only for
  in-place-only staged descriptors (`FILE_OPNAME==0`) with a mounted volume + the service task up;
  growable/created descriptors and the no-FAT core fall through unchanged.
- **Tested (QEMU):** `./arroyo check` both arches, knob on + off. Knob-ON `test 40` (non-FAT) = 18
  PASS + MISSION + `bx-blockreq` PASS. Knob-ON `UNAOS_FATIMG=sf ./arroyo test 150` = **23 PASS** (22
  fixtures + the S3 synchronous-write witness), with HELLO.BIN read + SCRATCH.BIN write/read-back
  served LIVE. Knob-OFF `test 40` = 18 PASS + MISSION, `UNAOS_FATIMG=sf test` = 22 PASS,
  **byte-identical** (no STOR-1 lines). `UNAOS_NOSTORAGE` clean both knob states. Lane: new
  `drivers/xhci/irqstorage.rs` + `arch/x86_64/syscall.rs` + `builder`/`arroyo` + `main.rs`; `fat.rs`/
  `block.rs` reused unchanged; **zero aarch64 files**.
- **✅ Metal (2026-07-10 attended bench, real 2012 rMBP):** clean full-chain usbdebug boot over FTDI
  serial — `:: bx-blockreq: PASS ::` (a raw LBA-0 read through the SERVICE TASK matched the polled read →
  **transfer-IRQ I/O + the submit/block/complete handshake work on the real Panther Point xHCI**,
  resolving design risk 1 — the one genuinely unproven thing), `U6bx` HELLO.BIN read LIVE, `S3`
  synchronous write-through + `U9x` SCRATCH.BIN write/read-back LIVE on the real FAT16 card, and the
  entire prior chain U1a→U6gx re-confirmed knob-ON; zero faults/timeouts/deadlock, HELLO.BIN intact.
  Off by default still (the knob is the metal caveat). The bench ran pre-fix code; the review fixes are
  transparent to the witnessed paths, so the confirm stands, and the fixed code re-benches at S4.
- **Review (security-arc, 2 must-fix + 3 notes, all folded before merge):** MF1 — every `REQ_QUEUE`
  hold is IRQ-masked at all 3 sites (closes a same-core service-task-preemption deadlock; masking both
  sides avoids the swap-the-trap trap). MF2 — `sys_open` refuses a writable open of HELLO.BIN
  (`rw && sidx==0 → -EACCES`), modeling immutable EL0 code as read-only — the root-cause close for the
  S3 write-through overwriting the on-disk executable (honest severity: feature-gated code-image
  regression, not a live ring-3 exploit). N1 offset-over-advance ledgered (rides S4), N2 `submit`
  panics off-scheduler, N3 docs-root paths fixed.
- **⚠ test-harness fact:** `./arroyo test-fat sf` is INTERMITTENTLY flaky — the OVMF USB-touch (builder
  ~line 226) sometimes makes the kernel misread the usb-storage geometry as 64 MiB (usb.img size) →
  `parse_bpb` rejects it → `NotFat` → fixtures run in-memory (18 PASS, looks like a regression but
  isn't). `UNAOS_FATIMG=sf ./arroyo test 150` (env at script start) is the RELIABLE FAT form.

### STOR-1 S4 — synchronous grow/create/delete in-syscall (retires the U10x op-queue when on) (2026-07-10) `hw-rmbp`
- **What:** the CONSERVATIVE-HYBRID completion of the STOR-D1 ladder (step S4): grow/create/delete now
  run SYNCHRONOUSLY in the syscall via the storage service task (out of the IF-masked handler), retiring
  the U10x deferred op-queue + its launcher-replay causal-fidelity gap WHEN THE KNOB IS ON. Only the
  DISK OPS became synchronous — the created-file wstage-for-reads, the snapshot-sibling model, and the
  U11x M2 defer LOGIC (`DYN_DELETED_G`/`OPENF_*`) are UNCHANGED (shared-backing cross-process reads are
  S5). **S4a** create — `open_create_new` submits `BlockOp::Create` (idempotent `find_located` →
  `create_in_root`) FIRST, so the file appears on disk in-syscall. **S4b** grow — `sys_write_grow`
  submits `BlockOp::Grow` disk-FIRST, then mirrors the extend into wstage + `FILE_SIZE` (reads still
  serve wstage this arc); no `mark_dirty` (already durable). **S4c** delete at last close — `sys_unlink`
  no longer enqueues a HELD op; the deferred DELETE runs synchronously via `BlockOp::Delete` at the last
  close (`openf_release`). **S4d** the launcher verdicts require the U10 op-queue drained NOTHING knob-on
  (`count == 0`, `u10_drain_verdict`), reading the state the synchronous ops already wrote; a new
  `:: S4: grow/create/delete SYNCHRONOUS … op-queue drained NOTHING … ::` witness.
- **The crux (S4c teardown safety):** `openf_decref`'s last-close release runs from THREE contexts — a
  syscall handler (`files_free`, blocking-safe), the launcher's non-self teardown (blocking-safe), and
  `exit`/reap self-teardown (IF=0 mid-death / no current task — MUST NOT block: blocking would resume a
  task whose CR3 is being freed, or `submit` off a scheduler). `openf_decref` stays pure atomics and
  SIGNALS the caller; `clear_files_row` detects blocking-safety at runtime via `current_user_cr3() !=
  slot_cr3(slot)` (a launcher is a kernel task, `user_cr3 == 0`; `exit` of the slot has `user_cr3 ==` the
  slot's CR3; the reaper has no current → both false). Every DEMO delete-trigger is blocking-safe (the
  unlinkers sweep their own descriptors; the cross-process holders close/tear-down from safe contexts).
  The unsafe-teardown last-close IS reachable (review catch — a cross-process grantee/sharer that exits
  or faults without closing): that branch DEFERS the delete via a `U10OP_DELETE` op (the knob-off
  mechanism), best-effort drained at IF=1, fail-safe if it strands (name blocked + a reclaimable orphan,
  no corruption). Review also folded a created-file in-place-write-through (else knob-on the write is
  dropped at teardown). The clean fix (shared on-disk backing) is S5.
- **Tested (QEMU):** `./arroyo check` both arches, knob on + off. Knob-ON `test 40` (non-FAT) = 18 PASS +
  MISSION (S4 inert without a FAT volume — the deferred/in-memory path). Knob-ON
  `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf test 150` = **24 PASS** (22 fixtures + the S3 + S4 witnesses), with
  u10x-grow / u10cx-create / u10dx-delete / u11m2-unlink / u6gx now SYNCHRONOUS (the U10 op-queue drained
  NOTHING; zero `U10_OVERFLOW`). Knob-OFF `test 40` = 18 PASS + MISSION, `UNAOS_FATIMG=sf test` = 22 PASS,
  **byte-identical** (no S4/synchronous lines — the deferred path verbatim). `UNAOS_NOSTORAGE` clean both
  knob states. Lane: `arch/x86_64/syscall.rs` (routing + launchers) + `drivers/xhci/irqstorage.rs`
  (`BlockOp::{Create,Grow,Delete}` + `submit_*` + service handlers) + `arch/x86_64/sched.rs`
  (`current_user_cr3`); `fat.rs`/`block.rs` reused unchanged; **zero aarch64 files**.
- **Metal:** knob-on is metal-PENDING (the S4 cross-process delete-at-last-close races are metal-only —
  QEMU-TCG will not interleave; design risk 3). Rides the next attended rMBP bench (transfer-IRQ I/O + S4
  create/grow/delete under true SMP + `fsck`).

### STOR-1 S5 — real shared backing for cross-process created-file reads (closes the U11x M2 torn-copy/TOCTOU residuals when on) (2026-07-11) `hw-rmbp`
- **What:** a CREATED-file descriptor's `SYS_READ` now reads the LIVE shared on-disk volume BY NAME
  (`created_read_live` → `submit_read_file`) instead of a private wstage snapshot — so a cross-process (or
  same-process sibling) opener reads a peer's writes (shared backing), retiring the U11x M2 residuals 3
  (torn-copy/disclosure) + 4 (open-vs-unlink TOCTOU) **when the knob is on**. Three coupled changes:
  **C2** `sys_read`'s created branch (gated `FILE_CREATED && FILE_OPNAME != 0 && HELLO_STAGED &&
  service_ready`; GROW.BIN — growable STAGED — keeps its wstage serve, byte-identical); **C3**
  `open_created_sibling` seeds the sibling wstage EMPTY (no snapshot copy — torn-copy closed by
  construction) + stamps identity from the CALLER-verified `nameid` after re-validating the source names it
  (recycle fail-closed `-ENOENT`); **C4** `sys_open_dynamic` re-checks `DYN_DELETED_G` after resolving.
- **Witness:** `s5_shared_backing_witness` (kernel-side, cross-row, reuses DEFER.BIN, silent skip off
  knob-on-FAT, unconditional cleanup) proves non-vacuously that the read SOURCE is shared/live — a sibling
  with an EMPTY private wstage (`WSTAGE_LEN == 0`, the discriminator) reads a peer's POST-OPEN overwrite;
  the u11m2/u6gx EL0 fixtures supply the production-faithful `SYS_READ` DISPATCH proof. NO concurrent stress
  (read/write serialize through the single service task → tearing impossible by construction, not a metal
  race).
- **⚠ Scheduling deadlock found + fixed (QEMU-reproducible):** routing created reads through the single
  service task makes a read BLOCK on it; a NON-preemptible ring-3 fixture busy-spinning (IF=0) on the
  service task's core (u6gx's owner A, cpu 1 == the service task's core) starves it, so u6gx's grantee B's
  cross-core read deadlocks. Fixed by (a) spawning the service task `PRIO_HIGH` (a system service must
  out-rank a spinning user task so a cross-core wake preempts — `poke_for`), and (b) making u6gx's
  cooperative-spin fixtures PREEMPTIBLE knob-on (gated on `s4_sync_storage()`; knob-off byte-identical).
  A malicious busy-spinner on the service core could still DoS created reads on this single-service-task
  design — future scheduler-fairness work, out of scope. (SECURITY.md STOR-1 S5; design §5 risk 4.)
- **Residual (honest):** the source read + the C3 re-check are not atomic → a metal-only recycle-to-a-
  DIFFERENT-name window is NARROWED (sibling names only the caller-verified nameid, so a cross-name recycle
  is caught) but not eliminated; airtight = the S6 namespace lock. No corruption / freed-chain / wrong-file
  LIVE read is possible (single service task serializes read-vs-delete).
- **Gate:** `./arroyo check` both arches (on+off); knob-ON `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf test` = full
  chain 0 FAIL with `:: S3/S4/S5:` witnesses + u10cx/u11m2/u6gx created reads served LIVE + u6gx PASS (the
  deadlock fix); knob-ON `test 40` = MISSION + `bx-blockreq`, NO `:: S5:` line (silent skip, no FAT);
  knob-OFF `test 40` = MISSION and `UNAOS_FATIMG=sf test` = BYTE-IDENTICAL (no S3/S4/S5/PRIO_HIGH lines;
  u6gx passes non-preemptibly); `UNAOS_NOSTORAGE` clean both. Lane: `arch/x86_64/syscall.rs` +
  `drivers/xhci/irqstorage.rs`; `fat.rs`/`block.rs` reused unchanged; **zero aarch64**.
- **Metal:** ✅ **METAL-CONFIRMED 2026-07-11 (round-6 attended rMBP bench, Boot 1 pristine + pristine
  re-confirm boot 3′):** `./arroyo mbench` PASS 29/29 required + 0 forbidden + 0 fault on the real 2012 rMBP
  over FTDI — `:: S5: … LIVE shared backing … -> PASS ::` first-ever on metal, and `:: U6gx: … -> PASS ::` is
  the DEADLOCK-CLOSURE witness under real SMP (the grantee's live cross-core read completes only via the
  PRIO_HIGH wake + timer-eviction of the preemptible spinner → the scheduler deadlock fix works on silicon;
  re-confirmed a 3rd time on the pristine boot 3′). `(irqstorage, PRIO_HIGH)` service line + S4-first-metal +
  `bx-blockreq: PASS` + full U-chain all PASS; HELLO.BIN intact. A reboot on a NON-re-prepped card showed the
  documented stateful-fixture `witness=0x0` false-FAIL (forensics: a 0-byte create-then-cut `DELME.BIN` +
  `FRESH.BIN` + grown `GROW.BIN`); pristine re-prep re-confirmed all-PASS on the SAME kernel → a card-state
  artifact orthogonal to the mechanism (evidence archived). ⚠ Bench-mechanics lesson: `scripts/card-watch.sh`
  (the diskutil insert-poller) trips a Claude-app/macOS-TCC security feature that REVOKES the app's
  removable-volume write access mid-bench — flash all cards FIRST, don't arm it when you still need to write one.

### STOR-1 S6 — the syscall-layer NAMESPACE lock (closes S5 residual 1 airtight) (2026-07-11) `hw-rmbp`
- **What:** an IRQ-masked `SpinMutex` (`NAMESPACE`/`ns_lock`, `arch/x86_64/syscall.rs`, the pi4 F3 `NAMESPACE`
  twin) makes the THREE created-file name sequences MUTUALLY ATOMIC — the sibling-open decision
  (`sys_open_dynamic`), the fresh create (`open_create_new`, now with an ACL-checked idempotent-sibling
  fallback that also closes a create-races-create ownership-theft window), and the `sys_unlink` claim + sweep.
  This closes S5's residual 1 AIRTIGHT: the non-atomic `created_desc_any_row` source-resolve + `== nameid+1`
  re-validate (a metal-only recycle-to-a-different-name / same-name-reincarnation window) is now impossible —
  no unlink/create can interleave inside another sequence's resolve→claim.
- **⚠ The S5 deadlock lesson binds:** the lock is held for the O(1) IN-MEMORY namespace decision ONLY, NEVER
  across a `submit`/BOT pump. The two blocking disk ops are lifted OUT — the idempotent `submit_create` runs
  BEFORE the lock, the last-close `submit_delete` AFTER it (`files_free` split into `files_free_clear` [atomic
  clears, lock-safe] + `openf_release` [blocking, lock-free]; `sys_unlink` performs the delete post-`drop(ns)`).
  Idempotent + single-service-task-serialized disk ops → lifting them out re-introduces no disk race.
  `sys_close`/`files_free` need NOT take the lock (a recycle needs a `files_alloc` REUSE, which only runs inside
  a lock-held open; a bare close mid-resolve just fails the re-validate closed, fail-safe).
- **§6 decision 2 RESOLVED:** `FAT_MUTATION` is NOT activated on x86 — VACUOUS under the single-service-task-
  writer invariant (the pi4 FAT lost-update RMW race is unreachable with exactly one BOT writer); the gap was
  syscall-layer namespace atomicity, not the FAT RMW. `fat.rs` (shared kernel-core) stays untouched — in lane.
- **Carry-overs folded (seat fold-ins):** the `-EIO` synchronous delete now gates its `DYN_DELETED_G` clear on
  delete SUCCESS (`openf_perform_delete`) — a wedged delete leaves the name blocked (fail-SAFE) rather than a
  re-create adopting a stale on-disk entry; + three uncounted mbench witnesses (`S4-mf2` immutable-code RW
  refusal; `S4-race` last-close synchronous-delete outcome; `S6-witness` lock-holds) + a u6gx drain-verdict
  tighten (`count==0` knob-on).
- **Witness:** `s6_witness_launcher` (`:: S6-witness: … witness OK ::`, uncounted, knob-on FAT) — a cross-core
  in-RAM RMW on the SAME `ns_lock`: LOCKED reaches `2*N` intact, the UNLOCKED control loses under contention.
  QEMU actually interleaved this run (locked 240000/240000, unlocked lost ~119800/240000) → the lock closes a
  REAL race (the positive proof; true-parallelism otherwise metal-latent, design risk 3).
- **Gate:** `./arroyo check` both arches (on+off, zero aarch64); knob-ON `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf
  test` = full chain 0 FAIL with `:: S3/S4/S5:` + `:: S4-mf2/S4-race/S6-witness:` witnesses + u10c/u11m2/u6gx
  created reads served LIVE + u6gx PASS; knob-ON `test 40` = MISSION + `bx-blockreq` (no FAT witnesses);
  knob-OFF `test 40` = MISSION and `UNAOS_FATIMG=sf test` = BYTE-IDENTICAL (no S3/S4/S5/S6/witness lines — the
  NAMESPACE lock is uncontended-transparent knob-off); `UNAOS_NOSTORAGE` clean both. Lane: `arch/x86_64/
  syscall.rs` only; `fat.rs`/`block.rs` reused unchanged; **zero aarch64**.
- **Metal:** the lock's true-parallelism proof is metal-only — rides the same attended rMBP bench as S4/S5
  (transfer-IRQ I/O + create/grow/delete + cross-process read/write + the namespace-lock witness + fsck).

### STOR-1 S7 — retire the U6bx staged-open constraint (open resolves ANY on-disk file) (2026-07-12) `hw-rmbp`
- **What:** `sys_open` of a PRE-EXISTING on-disk file no longer requires the file to be in the BSP-staged set
  (`STAGED_NAMES` = HELLO.BIN/SCRATCH.BIN/GROW.BIN). Knob-on, an open of ANY name on the mounted FAT volume that
  is neither staged nor a U10 created name falls through to DYNAMIC on-disk resolution through the storage
  service task — retiring the last U6bx staged-set constraint. The read machinery already resolved arbitrary
  names live (`find_located`); only the OPEN layer was gated to the staged/U10 name tables. A new `BlockOp::Stat`
  (`submit_stat` → `find_located`) returns the on-disk size — the one fact the IF-masked handler cannot get
  itself — and the descriptor's reads route live BY NAME through `submit_read_file` (`sys_read`'s new dynamic
  branch, page-at-a-time so an arbitrary file may exceed the one-page staged bound).
- **READ-ONLY by construction (MF2 generalized):** an arbitrary on-disk open is refused `-EACCES` if writable.
  This opens NO write path to arbitrary files (the S3 in-place write-through resolves by name and could otherwise
  overwrite any resolvable file — the exact hazard MF2 closed for HELLO.BIN, here generalized to every
  non-staged file), and keeps the ACL surface unchanged: a dynamic on-disk file has no `U10_NAMES` id, so it can
  never enter `OWNED_FILES` and is inherently PUBLIC. **Staged/U10 names EXCLUDED — CASE-INSENSITIVELY**
  (security-review CONFIRMED-critical, FIXED before merge): the fallthrough CANONICALIZES the ring-3 name to 8.3
  UPPERCASE first, then excludes it if the canonical form is a staged or U10 name — because `find_located`
  matches case-INSENSITIVELY (`eq_name`: equal-length + `eq_ignore_ascii_case`) while the name tables are
  byte-exact uppercase, so a case-variant like `owned.bin` would otherwise miss both exclusions yet resolve on
  disk to the OWNED file, bypassing the U6gx owner ACL (a confidentiality break). `eq_name`'s equal-LENGTH
  requirement makes uppercasing a PROVABLY COMPLETE defense; a closed created file stays `-ENOENT` in any casing.
  A NEGATIVE witness leg locks it: `sys_open_dynamic("owned.bin")` must be refused. No NAMESPACE lock: a dynamic
  file is outside the U10 mutation namespace (it can't be created/grown/unlinked via the syscall API), so its
  `Stat` block never runs under `ns_lock` (the S5 deadlock class stays closed).
- **The pre-stage buffer became:** retained — knob-OFF it still backs every staged read, and `HELLO_BYTES` still
  serves `sys_spawn`'s program-image copy in BOTH states (no live alternative for spawning off the IF-masked
  path). Knob-ON it is no longer the file-open boundary: all reads — staged (S2/S3), created (S5), and now
  arbitrary on-disk (S7) — serve the live volume.
- **Witness:** `s7_openany_witness` (`:: S7-openany: … resolved dynamically + read its live content off the
  pre-stage set … witness OK ::`, uncounted, knob-on FAT). Drives the REAL dispatcher (`sys_open_dynamic`) on a
  scratch row for README.TXT (a non-staged/non-U10 file every FAT image carries; pre-S7 this exact open was
  `-ENOENT`), then proves a conjunction: the open minted a DYNAMIC descriptor (`FILE_DYNLEN != 0`) stamped with
  the resolved name, sized from the LIVE volume (`FILE_SIZE == 57` on sf), and a read BY THE STORED NAME returned
  the file's known content prefix.
- **Seat fold (S6 fold-in 1):** `install_file_handle`'s handle-table-full unwind documents the non-local
  invariant (pending ⟹ deleted ⟹ the open was already `-EBUSY`) that makes its `openf_release` safe against a
  concurrent last-close; a dynamic descriptor has no name-id, so its unwind skips `openf_release` (vacuous).
- **Security-tier review (3-lens adversarial, 7 agents, folded before merge):** **1 CRITICAL** — the exclusion
  was byte-EXACT-case but `find_located` matches case-INSENSITIVELY, so `owned.bin` bypassed the U6gx owner ACL
  and read the private `OWNED.BIN`; FIXED by uppercase canonicalization + a negative witness leg (above). **2
  LOW** (same defect, two lenses) — the dynamic multi-page read masked a mid-loop I/O error as a short read
  (offset over-advanced, silent hole); FIXED to fail the whole read `-EIO` (matching S2/S3, disk-is-truth). **1
  refuted-but-folded** — `clear_files_row` didn't reset `FILE_DYNLEN` (not exploitable — `files_alloc` resets at
  reuse — but added for discipline + the field's "reset on every teardown" invariant). Re-verified: check both
  knobs clean, knob-on chain 0 FAIL + S7 both-legs witness OK, knob-off byte-identical 22 PASS.
- **Gate:** `./arroyo check` both arches (on+off, zero aarch64, zero new warnings); knob-ON `UNAOS_IRQSTORAGE=1
  UNAOS_FATIMG=sf test 200` = full chain 0 FAIL (S3/S4/S5/S6 + u6gx + u11m2 witnesses intact) + the new
  `:: S7-openany: … witness OK ::`; knob-OFF `test 25` = MISSION and `UNAOS_FATIMG=sf test 200` = BYTE-IDENTICAL
  (all S7 code `irqstorage`-gated or comment-only — no S7/dynamic lines); `UNAOS_NOSTORAGE` clean both. Lane:
  `arch/x86_64/syscall.rs` + `drivers/xhci/irqstorage.rs` (both x86/irqstorage-gated); `fat.rs` untouched
  (§6 decision 2 binds); **zero aarch64**.
- **Residuals (ledgered, out of scope):** arbitrary-file open is READ-ONLY (a writable arbitrary-file path is
  future work); files > 2 GiB are not openable (the `i32` stat channel); a closed U10 created file stays
  `-ENOENT` (preserves created-file + owner-ACL semantics); GROW.BIN reads still serve wstage knob-on.
- **Metal:** S7's arbitrary-file open joins the accrued rMBP bench batch (transfer-IRQ I/O + create/grow/delete +
  cross-process read/write + the namespace-lock witness + this open-any read + fsck) — attended, not owed by the
  QEMU gate.

## hw-pi4 track — 2026-07-10 (K1 M2–M4 — the U6 ACL SURVIVES REBOOT: persist + rebuild + gated enforcement + proofs)

### K1 M2.2/M2.3/M2.4 + M3 + M4 — `UNAFS.ATR` persistence LANDED 🔬 `hw-pi4`
- **What:** turn the M1 FORMAT into a real security property — owner/grants SURVIVE REBOOT. The
  ratified model (Peter + orchestrator seat, 2026-07-10): a launcher-assigned, KERNEL-STAMPED
  principal (default `prog:<name>`), one per 8.3 name, never EL0-set. Cross-reboot enforcement is
  GATED on multi-program by-name spawn — PROVEN now, LIVE when a second launchable named program lands.
- **M2.2** (`fat.rs` + `syscall.rs`): read-only `volume_fingerprint() -> (BS_VolID, count_of_clusters)`
  — the real volume binding (replacing M1's `cluster_size`/`num_fats` placeholder); x86-neutral.
- **M2.3** (write-through persist): `OwnedFile.owner_ppid` + `FileGrant.ppid`, captured at create/grant
  from principals snapshotted before any lock; persist in the syscall handler AFTER the in-RAM update
  (`sys_open`-create/`sys_fgrant`/`sys_unlink`), gated on `owner_ppid.kind != NONE` so anonymous owners
  do ZERO disk I/O; `atr_ensure` self-heals a stale/foreign-binding header.
- **M2.4** (mount-rebuild + gated admission): `atr_rebuild_into_owned` re-resolves each persisted row
  BY NAME + installs it with a NO-LIVE-OWNER sentinel; `owned_access_ok`/`owned_is_owner`/
  `owned_unlink_permitted` gain an ADDITIVE ppid branch (owner-by-name full authority; grantee-by-name
  rights-checked) after the unchanged `(asid, gen)` checks — structural (NONE never matches). The
  real-boot rebuild is gated on `by_name_spawn_multivalued()` (false today).
- **M3** (`k1_persist_launcher`): kernel-side two-phase proof — persist an owned+granted file, simulate
  a reboot, rebuild, enforce with real stamped principals (14-assertion witness `w=0x3fff` after F1/F2).
- **M4** (`k1_corrupt_launcher` + deny-EL0): a TORN on-disk row fails closed to PUBLIC at mount (no
  forged owner); EL0 `SYS_OPEN` of `UNAFS.ATR` denied outright.
- **F1** (adversarial code-review catches): `owned_grant` by-name owner branch; `atr_ensure` splits a
  block READ error from a binding MISMATCH (no more wiping rows on a hiccup); M3 persists `fc=0`.
- **F2** (seat security-tier review, 6/6-refuter must-fixes — all fail-OPEN, latent while gated off):
  `owned_grant` REVOKE/UPDATE arms match a rebuilt grantee by ppid (revoke was silently a no-op +
  re-persisted; update double-slotted); owner teardown converts a NAMED owner to the sentinel instead
  of wiping (a wiped RAM row made `sys_unlink` skip the disk clear → future same-name adoption);
  `sys_unlink` clears the ATR row before the `0xE5`. M3 witnesses revoke-after-rebuild + teardown (→ `w=0x3fff`).
- **Tested (QEMU):** `check` BOTH arches (x86 unchanged); `kernel8` baremetal; `kernel8-test` → **23
  PASS byte-identical** to base (the `:: K1-persist: … PASS ::` + `:: K1-corrupt: … PASS ::` witness
  lines are the ONLY additions, UNCOUNTED) + CAPSTONE 6/6 + F2/F3 witnesses + `:: K1-atr: … disk PASS ::`,
  zero R1/CMD13; `test-arm` MISSION SUCCESS.
- **Metal:** survive-reboot (persist under a `prog:X` stamp → power-cycle → re-acquire) rides the next
  attended bench; M3/M4 self-clean so the stateful card accumulates nothing.
- **Detail:** [`SECURITY.md` §K1](SECURITY.md). **Commits:** K1 M2.3/M2.4/M3/M2.2/M4 on `hw-pi4` (see `git log`).

## hw-pi4 track — 2026-07-11 (K2 — make cross-reboot enforcement LIVE: 2nd/3rd programs + gate flip + grow-repersist + real-program proof)

### K2 M(a)–M(d) — the U6 reboot-surviving ACL is now ENFORCED, proven end-to-end through REAL programs 🔬 `hw-pi4`
- **What:** K1 proved the persist→rebuild→enforce mechanism but GATED it off (only one launchable named
  program existed, so a persisted owner could deny everyone yet be re-acquired by no one — a brick). K2
  supplies the honest precondition and turns it LIVE. **M(a):** two more EL0 programs on the card —
  `K2OWN.BIN` (private `O_CREAT` owner) + `K2IMP.BIN` (non-owner), built as extra `[[bin]]`s of
  `crates/user-blob` and carried by `arroyo kernel8`; three distinct 8.3 names → three distinct
  `prog:<NAME>` principals. **M(b):** `atr_persist_grow` re-persists a named-owner file's
  `first_cluster`/`size` on GROW (anonymous-inert → battery byte-equivalent). **M(c):** flipped
  `by_name_spawn_multivalued()` true, so `atr_maybe_boot_rebuild` reinstalls persisted rows at real boot
  (a QEMU no-op — `UNAFS.ATR` absent at that point; live effect is metal). **M(d):** `k2_liveenf_launcher`
  spawns `K2OWN.BIN` (create+own+persist+grow `K2PRIV.BIN`), SIMULATES a reboot (`owned_clear` + remount +
  `atr_rebuild_into_owned` → sentinel-owned row from disk), re-spawns `K2OWN.BIN` (re-admitted purely BY
  NAME), spawns `K2IMP.BIN` (refused `-EACCES`), then self-cleans. **F2:** a 13-agent adversarial review
  (1/9 confirmed) caught a metal self-clean gap (pre-flight skip returned before the stale-`K2PRIV.BIN`
  cleanup) — fixed.
- **Tested (QEMU):** `check` BOTH arches (x86 unchanged); `kernel8` baremetal; `kernel8-test` → **23 PASS
  byte-equivalent** (the `:: K2-liveenf: … rebuild+enforce PASS [w=0x7f] ::` witness — 7 bits: create+stamp,
  in-RAM owner, disk-survive+rebuild, rebuilt-owned-by-name, owner-re-admitted-by-name, impostor-denied,
  grow-repersist-landed — is the only addition, UNCOUNTED) + CAPSTONE 6/6 + F2/F3 witnesses locked
  240000/240000 + the K1-atr/persist/corrupt lines, zero R1/CMD13; `test-arm` MISSION SUCCESS. Zero x86.
- **Metal: ✅ CONFIRMED (2026-07-11 attended bench, real Pi 4; this session drove, Peter physical).**
  **Part A one-boot:** `MBENCH PASS 25/25` required witnesses, 0 forbidden hits (23 PASS + CAPSTONE 6/6 +
  `K2-liveenf … PASS [w=0x7f]` + K1-persist/corrupt + F2/F3 locked; zero R1/CMD13/EXCEPTION; zero A72 EC=0
  heal lines). **Part B genuine two-boot power-cycle** (the M(e) `UNAOS_K2_LEAVE` knob): boot-1 left
  `K2PRIV.BIN` persisted (owner `prog:K2OWN.BIN`, fc=0x12) → REAL power-cut → boot-2 the LIVE boot rebuild
  reinstalled the row from disk, owner re-admitted BY NAME, impostor refused `-EACCES`, self-cleaned:
  `:: K2-metal: BOOT-2 … SURVIVED a real power-cycle … PASS [w=0x07] ::`. Logs:
  `~/unaos-bench/pi-serial-2026-07-11-112106.log` (A) + `…-112529.log` (B). The cross-reboot ACL now
  survives a real power-cycle on silicon — not just the same-boot simulate.
- **Detail:** [`SECURITY.md` §K1 (K2 bullet)](SECURITY.md). **Commits:** K2 M(a)/M(b)/M(d)/M(c)/M(d)-F1/F2
  on `hw-pi4` (see `git log`).
- **M(e) — the metal money-shot knob (`UNAOS_K2_LEAVE`, attended Pi bench only; follow-on commit atop the
  merged arc):** QEMU can only same-boot SIMULATE the reboot, so the true power-cycle survival needs boot-1
  to LEAVE the persisted row across a real reboot. A new `k2_leave` cargo feature (arroyo enables from
  `UNAOS_K2_LEAVE=1`; OFF ⇒ the normal same-boot battery, byte-identical) swaps `k2_liveenf_launcher` for
  `k2_metal_launcher`: boot-1 (`K2PRIV.BIN` absent) creates+owns+persists+grows then LEAVES the file;
  power-cycle; boot-2 (`K2PRIV.BIN` present) verifies the LIVE `atr_maybe_boot_rebuild` reinstalled the row
  across the power-cycle (owner re-admitted BY NAME, impostor refused), then self-cleans. QEMU-proven via
  same-image `if=sd` write-back (boot-1 `BOOT-1 left … fc=0x14`; boot-2 `BOOT-2 … SURVIVED … PASS [w=0x07]`,
  0 FAIL, CAPSTONE 6/6; boot-2's battery is stateful-degraded, expected). Bench card:
  [`unaos/scripts/k2-metal-bench.md`](../unaos/scripts/k2-metal-bench.md). Normal build byte-identical;
  check both arches OK; zero x86.

### K3 — revoke-persist commit-ordering: the fail-OPEN revoke residual RETIRED 🔬 `hw-pi4`
- **What:** the last fail-OPEN residual in the U6 owner/grants ACL. A `SYS_FGRANT` revoke used to commit
  in-RAM and THEN re-persist best-effort, so a crash / swallowed disk error during the re-persist left the
  OLD grant on disk → the revoked grantee was re-admitted at the next mount. K3 reverses the order for a
  REVOKE of a NAMED grantee on a NAMED-owner file: `sys_fgrant_revoke_2phase` computes the post-revoke grant
  set from the in-RAM snapshot, writes that NARROWED row to disk FIRST (via the extracted
  `atr_write_grant_row`), and commits the in-RAM removal ONLY if the durable write held. A persist failure is
  FAIL-CLOSED — the in-RAM grant is left intact and the caller gets `-EIO`/`-ENODEV`, so RAM and disk never
  silently diverge. Widen (grant/update) and anonymous grantees keep the byte-identical
  in-RAM-then-best-effort order (already fail-closed) → the 23-fixture battery is byte-equivalent.
  **Folded K2-review family items:** `sys_unlink` aborts (`-EIO`) BEFORE the `0xE5` name delete if the
  durable `atr_clear_row` failed (a swallowed clear can no longer strand a stale owner row); the K2
  launchers' `cleaned` probe matches `NotFound` specifically rather than `.is_err()`; two stale "GATED OFF
  today" comments rewritten (the by-name gate is LIVE since K2); bit6's dir-entry-head corroboration noted.
- **Proof:** `k3_revoke_check` (kernel-side, the `k1_persist_check` idiom, no EL0 fixture) drives the
  production two-phase path with real stamped principals: a named-owner file with two grantees, one revoked,
  SURVIVES a simulated reboot (revoked grantee DENIED, kept grantee still admitted) and a FORCED persist
  failure fails closed (`-EIO`, in-RAM grant intact). Emits an uncounted
  `:: K3-revoke: … two-phase durable-first PASS … [w=0x7f] ::` line (7 bits).
- **Tested (QEMU):** `check` BOTH arches (x86 unchanged); `kernel8` baremetal; `kernel8-test` → **29 PASS
  (23 + CAPSTONE 6) / 0 FAIL byte-equivalent** (the `:: K3-revoke: … PASS [w=0x7f] ::` witness is the only
  addition, UNCOUNTED) + CAPSTONE 6/6 + F2/F3 witnesses locked 240000/240000 + K1-atr/persist/corrupt +
  K2-liveenf, zero R1/CMD13; `test-arm` MISSION SUCCESS. Zero x86 (all changes in aarch64 `syscall.rs`).
- **Metal:** 🔬 QEMU-verified; the two-phase revoke's real disk-write ordering rides the next attended Pi
  bench (batched with the standing K1-persist/K1-corrupt/K2 metal watch-items).
- **Detail:** [`SECURITY.md` §K1 (K3 bullet)](SECURITY.md). **Commit:** `syscall(aarch64): K3 — two-phase
  revoke commit-ordering + fixture` on `hw-pi4` (see `git log`).

### IMG-SIG — code-signing: graduate the principal from PROGRAM_NAME to IMAGE_SHA256; the name-collision residual RETIRED 🔬 `hw-pi4`
- **What:** the last honest identity residual the U6 ACL carried — "two blobs with the same 8.3 name are the
  same principal". `load_program_into_slot` (the SOLE principal mint path) now stamps `image_of(&bytes)` =
  `kind = IMAGE_SHA256` over the SHA-256 of the loaded IMAGE, not `program_name(name)`. Two byte-identical
  images share a principal (a re-spawn / a reboot-persisted owner is re-admitted); two DIFFERENT images under
  the same trusted name do NOT (a swapped blob is refused `-EACCES`). In-lane, no new crate: a no_std FIPS
  180-4 SHA-256 (`sha256`/`sha256_compress`); `PrincipalRecord::image_sha256` stores the 30-byte digest PREFIX
  in the fixed `value[30]` field (240-bit identity, NO format bump — the on-disk row/CRC/codec are
  byte-transparent). The K2 launcher/metal fixtures now compute the expected owner via `image_principal_of_file`
  (mount+read+hash, no slot) rather than a name literal.
- **Scope (honest):** image IDENTITY, not authentication — a plain digest, so it does NOT resist an OFFLINE
  attacker who rewrites the plaintext `UNAFS.ATR` row (no HMAC/signature; the Pi 4 has no protected key store).
  It closes ONLINE / same-boot substitution and the cross-reboot same-name-different-code adoption. The K3 SMP
  concurrent-repersist window and the torn-row confidentiality-downgrade residual are UNCHANGED (identity, not
  commit-ordering). Enforcement is principal-agnostic (`PrincipalRecord` compared by value), so the K1/K2/K3
  fixtures — which manually stamp PROGRAM_NAME on scratch ASIDs — are unaffected and the battery is
  byte-equivalent.
- **Proof:** `image_sig_selftest` (last in the U7 chain, read-only, no disk write): FIPS KATs for `""`/`"abc"`
  /a 56-byte padding-overflow message; constant-buffer discrimination (identical→equal, one-byte-diff→distinct,
  IMAGE ≠ PROGRAM_NAME by kind); real-image discrimination (`K2OWN.BIN` vs `K2IMP.BIN` → distinct stable
  IMAGE_SHA256 principals). Emits an uncounted `:: IMG-SIG: … residual closed) PASS [w=0x7f/0x7f] ::`.
- **Tested (QEMU):** `check` BOTH arches (x86 unchanged); `kernel8` baremetal; `kernel8-test` → **29 PASS
  (23 + CAPSTONE 6) / 0 FAIL byte-equivalent** (the `:: IMG-SIG: … PASS ::` witness the only addition,
  UNCOUNTED; K2-liveenf now witnesses re-admit-by-IMAGE-digest, still `PASS [w=0x7f]`) + CAPSTONE 6/6 + F2/F3
  witnesses locked 240000/240000 + K1-atr/persist/corrupt + K3-revoke, zero R1/CMD13; `test-arm` MISSION
  SUCCESS. Zero x86 (all changes in aarch64 `syscall.rs` + the regression spec's OPTIONAL IMG-SIG line).
- **Metal:** 🔬 QEMU-verified; the real-image discrimination through disk-loaded programs rides the next
  attended Pi bench (batched with the standing K1/K2/K3 metal watch-items).
- **Detail:** [`SECURITY.md` §K1 (IMG-SIG bullet)](SECURITY.md). **Commit:** `syscall(aarch64): IMG-SIG —
  IMAGE_SHA256 code-signing principal + witness` on `hw-pi4` (see `git log`).

## hw-jetson track — 2026-07-10 (JD4 — read-side navigation + last dead levers + screen-on-boot; same-day attended bench)

### JD4 — `ls <dir>` / `cd` / `pwd` / `cat <path>` on the panel shell + JB2c/JB9b lever retirement + screen-on-boot ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What (M1, arch-neutral, `5ca6e28`):** the shell grows a working directory and path-aware file
  commands on the read-only FAT walkers. One seat-granted additive `fat.rs` helper —
  `pub read_dir(first_cluster)` (0 = root, the `..`-to-root convention; read-only, NO lock; F3 may
  revisit read-side locking) — everything else in `shell.rs`: cwd as a normalized CANONICAL absolute
  path string, re-resolved from the root each command (a swapped card can never leave a stale chain
  head), lexical `.`/`..` normalization, case-insensitive 8.3 component walk, errno-tagged honest
  errors (`-ENOENT`/`-ENOTDIR`/`-EISDIR`/`-EIO` printed, never swallowed). WRITE path deliberately
  deferred to JD5 (pi4 F3 is about to churn `fat.rs` and a write path wants F3's locks anyway).
- **What (M2, behaviour-neutral, `436d7ef`):** retire the four dead levers JD3 left —
  `jb2c_padctl_powerup`/`jb2c_usb2_trk_clk` + `jb9b_ao_sid_fix`/`jb9b_accept_bypass_sid` (all dead
  behind the inherit gates `JB9H_SKIP_CHAIN`/`JB9_PROBE`) + their orphaned private helpers, −313
  lines. The ⭐ JB9 recipe and BOTH firmware-destroying-lever compile-asserts stay untouched.
- **What (M3, tegra-only, `195ab88`):** screen-on-boot — the panel console appears at the first
  keystroke OR a ~8 s CNTPCT wall-clock deadline (the JD3 timerless idiom), so a panel boot ends at
  a visible prompt instead of waiting for a blind keystroke. Headless boots unchanged (JB2b serial
  evidence contract holds).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `UNAOS_HUBSTORAGE=1 test 25`
  MISSION (shared shell/fat guard); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `esp-jetson` links, **108 `tegra:` strings** (validate media by count, not size).
- **Metal — ✅ PASS (2026-07-10 attended bench, serial `jetson-serial-2026-07-10-135751.log`):**
  screen-on-boot fired 3/3 boots (no key, ~8 s, post-CAPSTONE); zero `JB2c`/`JB9b` lines any boot;
  full navigation sequence on a **FAT16** card (`UNAOSRW` + fresh `DOCS/README.TXT`, Alcor reader
  slot 5) — `diskinfo`/`ls`/`cd docs`/`ls`/`cat readme.txt`/`pwd`/`cd ..`/`cat /docs/readme.txt` +
  `-ENOENT`/`-EISDIR` probes, all typed lowercase (case-insensitive 8.3 proven). Boots 1–2
  reconfirmed the JD3 hub-MSC intermittency (`vid=0000` on route 0x4) with a GRACEFUL settle-window
  fallthrough each time; the tegra bench pattern is a separate data card, not the boot stick.
- **Detail:** [`arch_arm64.md` §JD4](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commits:** `436d7ef` M2 ·
  `5ca6e28` M1 · `195ab88` M3 (`hw-jetson`).

## ux track — 2026-07-10

### TSTE-1 — `tste`: the in-OS self-test suite (x86 + aarch64) 🔬 `ux-tste`
- **What:** running tests no longer needs a host. From a booted shell (x86 GUI, the Orin panel, or
  the serial console) `tste` runs the suite and prints ONE three-section PASS/FAIL/SKIP table:
  **[boot-time]** replays the boot-sequenced fixture verdicts, **[live]** re-runs everything that can
  honestly re-run post-boot, **[skipped]** lists what can't (with reasons). New
  `crates/kernel/src/selftest.rs` owns the suite; `shell.rs` gains a `tste` arm only. `tste` prints
  in the console like `ps` (it does NOT take the screen) and is READ-ONLY toward storage. Every line
  mirrors to serial as `:: TSTE: <name> -> PASS/FAIL/SKIP ::` + a `:: TSTE: N pass M fail K skip
  (+B boot) ::` summary (the QEMU gate).
- **[live] registry:** `sched.introspection` (meter counters readable + monotonic, ≥1 CPU);
  `heap.roundtrip` (alloc/free/realloc round-trip); `video.geometry` (draw_line/fill_triangle
  verified on an OFFSCREEN GneissPal buffer via the trait-default rasteriser — zero touch to the
  visible framebuffer); the six sync primitives `sync.{mutex,rwlock,semaphore,channel,condvar,join}`
  re-verified ON DEMAND with FRESH worker tasks (the coordinator never blocks — on the unscheduled
  x86 BSP a blocking primitive would panic/bail, so all blocking runs in spawned workers it only
  polls with a bounded budget; a broken/hung primitive surfaces as FAIL/SKIP, never a shell hang);
  `storage.{mount,rootwalk,readfile}` (FAT mount, root walk, HELLO.BIN length+content — read-only).
- **M2b boot-verdict replay:** a fixed 64-entry static ring in `selftest.rs` captures every
  `-> PASS`/`-> FAIL` fixture line as it is emitted, via ONE additive hook at the serial `_print`
  seam (both arches) — alloc-free, `try_lock` only, safe from IRQ-masked print contexts, zero change
  to what is printed. This is what lets `tste` replay the boot-sequenced fixtures it cannot itself
  re-execute.
- **SKIP-honesty:** the sync section SKIPs (single core / probe timeout) and the storage section
  SKIPs (no FAT volume) rather than faking a result. The full CROSS-CORE CAPSTONE stress + the EL0
  U-arc fixtures stay boot-sequenced; re-running them on demand needs a launcher refactor — the
  **TSTE-2** horizon, stated in `tste`'s own footer.
- **Tested — QEMU:** `./arroyo check` (x86 + aarch64) and `UNAOS_TEGRA=1 ./arroyo check` green;
  `./arroyo test 25` and `./arroyo test-arm 22` both **MISSION SUCCESS** (boot unperturbed — `tste`
  runs only on command). Live demo via QMP `send-key` to the usb-kbd (`scripts/qmp_type.py`): typing
  `tste` produced 16 boot-replayed + 9 live PASS (`9 pass 0 fail 0 skip (+16 boot)`); under
  `UNAOS_FATIMG=sf` the storage checks PASS too (`12 pass 0 fail 0 skip (+19 boot)`), all six sync
  primitives PASS on real QEMU. Metal/panel verdict rides `./arroyo x86` and the next Orin bench.

## gfx track — 2026-07-10

### VUG-1 — the crystal: `vug` becomes the graphics engine's living demo (x86 + aarch64) 🔬 `gfx-vug`
- **What:** rebuilt `vug` from a static test pattern into a real-time, software-rendered rotating
  quartz crystal (an elongated hexagonal bipyramid — 14 vertices, 24 triangles), drawn through the
  Gneiss PAL. Arch-neutral and float-free: geometry, a two-axis tumble, and perspective projection
  run in Q16.16 fixed point off a 256-entry brad sine table. Solid mode does backface culling
  (screen winding) + painter's-order depth sort + per-face integer Lambert shading (deep amethyst on
  the #1E1E1E Can-Am grey, lilac seam highlights); `vug wire` shows the wireframe; `vug bebox` keeps
  a BeBox tribute; any key exits cleanly back to the shell (the `took_screen` contract). This starts
  the **graphics-engine ledger** — see [`dev/OS/08_VIDEO/engine.md`](dev/OS/08_VIDEO/engine.md).
- **Engine primitives added (additive; fbcon/Console/Screen contracts unchanged):** `draw_line`
  (Bresenham) and `fill_triangle` (scanline) on FrameBuffer/Screen/PAL with damage tracking;
  `pal::pump_and_poll` so a full-screen interactive loop drives its own input (xHCI HID + aarch64
  UART) and exits on a key.
- **M3b corner meters:** a RENDER meter (the honest software "GPU monitor" — per-frame render/present
  cycle span vs whole-frame span => busy %, frame time / FPS, drawn triangles + estimated filled
  pixels) and a BeOS-Pulse-style **CPU pulse** meter (per-core busy/idle from additive relaxed
  lock-free counters bumped at the dispatch/idle points of both schedulers; read via
  `sched::meter_cpu_count`/`meter_cpu_ticks` — introspection only, never on a scheduling path). Both
  render through the damage-tracked back buffer; one present per frame holds. Each meter names the
  seam a real GPU/PMU utilization feed would replace.
- **Tested — QEMU:** `./arroyo check` (x86 + aarch64) and `UNAOS_TEGRA=1 ./arroyo check` green;
  `./arroyo test 25` and `./arroyo test-arm 22` both **MISSION SUCCESS** (vug runs only on command,
  so it does not perturb headless boots — the regression suites are un-regressed by the added
  scheduler counters). **Visual verdict is attended-pending:** `./arroyo x86` (QEMU GUI) and the next
  Orin bench are where the crystal spins. The `:: VUG: crystal live — 24 faces, solid/wire, exit
  clean ::` and `:: VUG: crystal exit clean — N frames ::` serial lines are emitted by `run_crystal`
  when the demo is invoked (GUI-verified-pending; headless gates never type `vug`).
## hw-jetson track — 2026-07-10 (JD3 — code arc + same-day attended bench)

### JD3 — storage behind the hub → real `ls`/`cat` on the Orin panel shell (+ dead-code retirement) ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What:** JD2's panel shell had no disk behind its `ls`/`cat`. JD3 brings up the hubbed
  mass-storage device (the Alcor reader) and gives the shell real files. The shell↔FAT↔block wiring
  is already architecture-neutral, so the work was *when/how* the tegra path drives the block device:
  **(M1)** run the (previously-skipped) `service_storage` SCSI bring-up **inside the pre-drop
  `jb2b_attach` pump**, while the JM4 timer is live — the only place the BOT/control pump's bounded
  `crate::hlt()` waits have a wake source — with a bounded 8 s storage-settle window after the
  keyboard arms so a device that enumerates behind the hub can publish `BLOCK_DEVICE` before the
  drop. **(M2)** make the *post*-drop panel shell's reads work on the timerless EL1 core: the JM6
  drop disables the physical timer but left `timer::LIVE` stale-true, so `arch::hlt()` would
  WFI-park the core forever the first time `ls`/`cat` hit the BOT pump. Fix = `timer::set_not_live()`
  after the drop (→ `hlt()` busy-spins) + convert the shared `pump_until_bot_done` from a fixed
  iteration budget to a `now_cycles`/`hw_wait_budget` **wall-clock** deadline (so a busy-spinning
  `hlt()` doesn't time out in microseconds). The wall-clock change mirrors the Pi's polled EMMC2
  driver (CNTPCT-deadline reads with the timer IRQ off) and is arch-neutral. *The one shared-file
  edit (`drivers/xhci/mod.rs`) is the "xHCI seam" the JD3 brief pre-authorised the jetson track to
  request from the integrator.* **(M3)** retire the dead JB3/JB4/JB5 "revive the halted Falcon"
  machinery — dead since the JB9 inherit pivot (gated const-false, already optimizer-pruned): the
  `!jb9h_skip`/`JB5_RUN_E_REPLAY` call blocks in `main.rs` + the orphaned functions across
  `bpmp_tegra.rs`/`xusb_tegra.rs`/`smmu_tegra.rs` (~840 lines). KEPT: the ⭐ JB9 recipe, BOTH
  firmware-destroying-lever compile-asserts (+ their guard consts), the shared XUSB/MMU register
  consts, the read-only post-attach diagnostics, and the JB9 forensic kit. Behaviour-neutral.
- **Tested — QEMU:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
  `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → **MISSION SUCCESS** (`storage_slot=2 note='ready'`, no BOT
  timeout — the primary guard for the shared BOT-pump change); `./arroyo test-arm 22` → **MISSION
  SUCCESS** (aarch64 BOT-pump guard); `UNAOS_GICV3=1 ./arroyo test-arm 40` → **CAPSTONE 6/6**;
  `esp-jetson kernel.elf` links, **107 `tegra:` strings** (JD2 was 105).
- **Tested — ✅ METAL (2026-07-10, attended; Peter at the Orin):** an SD card in the **Alcor USB reader
  behind the hub** enumerated (`vid=058f pid=6362 … route 0x4 tier 1` → `HUB DOWNSTREAM MASS STORAGE
  (slot 5)`); M1's `service_storage` ran the SCSI bring-up pre-drop (`Disk 'Generic' 'USB SD Reader'
  … 29 MiB` → `READ(10) LBA0 … Passed` → `JD3 — mass storage ready`), and post-drop the panel shell's
  `diskinfo`/`ls`/`cat` read the FAT card — **PASS**, with **zero `BOT pump TIMEOUT`** anywhere (the M2
  timerless wall-clock BOT read + `set_not_live()` proven on silicon). First real filesystem content on
  the Orin panel. Bench note (confirms the flagged risk): the reader's hub-downstream enumeration is
  intermittent — the first boot the hubbed LS/FS devices failed and the 8 s settle window correctly fell
  through to `no mass storage … proceeding` (graceful, no wedge); a re-seat + power-cycle brought it up.
  Serial `~/unaos-bench/jetson-serial-2026-07-10-104357.log`.
- **Detail:** [`arch_arm64.md` §JD3](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** see `git log` (`hw-jetson`).
## hw-rmbp track — 2026-07-10 (U6gx — UnaFS owner/grants ACL, the x86 twin of pi4 U6, Opus-executed)

### U6gx — UnaFS owner/grants: by-name ACL at SYS_OPEN + SYS_FGRANT delegation + F1 owner-only unlink (x86) ✅ METAL-CONFIRMED (2026-07-10) `hw-rmbp`
- **What:** the x86 twin of the aarch64 U6 owner/grants ACL — closes the U11x M2 ledger anchor
  (cross-process open/unlink of a created file was GRANT-FREE on x86). Secure-by-DEFAULT: an
  `O_CREAT` of a NEW name records the creator as OWNER (PRIVATE); the new `O_PUBLIC` mode bit
  opts out to world-access. An open of an existing owned file is admitted only for the owner or a
  principal the owner `SYS_FGRANT`ed (a `CAP_READ|CAP_WRITE` subset), else `-EACCES`; a file with
  no owner row (a STAGED file / an `O_PUBLIC` create / a torn-down owner) is PUBLIC (pre-U6gx
  behaviour). **F1 folded from the start** (pi4 learned it post-hoc): `SYS_UNLINK` is OWNER-only
  for an owned file, so a content grantee cannot `unlink`+`O_CREAT` to steal ownership.
- **How (the x86 substitutions):** the ACL is keyed by the created file's `U10_NAMES` **name-id**
  (a direct index, not pi4's `(dir_lba,dir_off)` — no recycled-key aliasing, no bounded-table
  "full", so `owned_set_owner` is infallible with no fail-closed path); the PRINCIPAL is the
  address-space SLOT fenced by `SLOT_GEN` (the `(ASID, ASID_GEN)` analogue). `OWNED_FILES` is a
  `SpinMutex<[OwnedFile; N_U10_NAMES]>` taken IRQ-masked via `without_interrupts` (syscall + the
  teardown path symmetric — the pi4 `IrqGuard` discipline); cleared at unlink + owner teardown
  (`owned_clear_owner_slot` in `clear_handle_row`, reverting to public). `SYS_FGRANT = 18` names
  the grantee OWNER-SCOPED by a `Child` handle (the `SYS_XFER` idiom); ownership is checked before
  the grantee handle is resolved. Additive on `arch/x86_64/syscall.rs`; **zero aarch64 files**.
- **Tested:** a two-process `u6gx_launcher` (owner A + grantee B on their own cores, GO/SIG
  choreography) witnesses the full matrix incl. the U11x M2 combined path (owner unlink while B
  holds open → deferred; re-create `-EBUSY`; ownership dies at B's last close): `:: U6gx: UnaFS
  owner/grants — non-owner open -EACCES, owner grant admits R|W (content crossed), non-owner grant
  -EACCES, grantee unlink -EACCES (delete owner-only), revoke re-denies, owner unlink defers while
  grantee holds + re-create -EBUSY, ownership dies at last close -> PASS ::`. `./arroyo check` both
  arches; `test 40`/`90` (in-memory core) 17 PASS + U6gx; `test-fat sf` + `p16` **22 PASS 0 FAIL**
  each (21 priors byte-identical + U6gx; `grep 'PASS ::'` reads 21 — it misses U2-0a's `PASS (...)`
  format); `UNAOS_NOSTORAGE=1 test 90` clean skip. Metal: pure syscall logic, storage-gated
  (metal-pending like U8x/U11x — flash-and-watch at the next attended bench).
- **✅ METAL (2026-07-10 attended bench, mid-2012 rMBP, boot 2 of 3):** the FULL x86 chain
  U1→U11m2→U6gx confirmed on silicon in one boot — 23 PASS, zero FAIL: U9x/U10x staged
  writes/grow/create/delete flushed on the real card, U11m2 delete HELD past unlink + released at
  last close AND teardown + DELETED on FAT (chain freed all copies), the complete U6gx
  owner/grants matrix. Bench facts: boot 1 hit 3 `BOT pump TIMEOUT`s (USBSTS PCD set — the
  Generic USB SD reader bounced mid-battery, same intermittent family as the jetson Alcor; clean
  boot 2 had zero) — the BOT layer still has NO endpoint-recovery path if a device stalls
  mid-battery (honest gap; the STOR-D1 ladder's natural home). Boot-1 also proved fixture-arch
  contamination is real: an aarch64 HELLO.BIN on the shared data card fails U2/U4x/U6x with
  "never returned" signatures — keep per-arch HELLO.BIN on bench cards. Boot 3: GUI build
  attended — `tste` three-section table + the vug crystal on the retina panel (vug present rate
  is uncached-GOP-VRAM-bound; a PAT/write-combining arc is PARKED as a candidate).
- **Commit:** see `git log` (`hw-rmbp`).

## hw-pi4 track — 2026-07-10

> **⭐ METAL BENCH (2026-07-10, attended — real Pi 4, Debug Probe serial):** the entire FAT-mutating
> stack confirmed on silicon in one boot off the real EMMC2 card (`@0xfe340000`, 15193 MiB CSD v2):
> **23 PASS** — U9 in-place write, U10 grow/create/delete (real cluster allocation + both-FAT mirror +
> directory RMW on the card), U11 M1/M2/M2b (close, unlink-defers-free, and the reaper freeing a real
> teardown-orphaned chain), and **U6-grants** (owner ACL + the F1 grantee-unlink denial witnessed on
> metal) — plus CAPSTONE 6/6, the 3 expected M6b kills, no leaks/faults, and **zero `M6g: R1 error`
> lines** (the new R1 check rode the whole battery with no false positives on a healthy card).
> Bench note: fixtures are stateful — a battery boot mutates `GROW.BIN`/`SCRATCH.BIN` and creates
> `OWNED.BIN`/`FRESH.BIN`/`B11.BIN`; re-runs on a stale card self-report `(stale image) — demo skipped`
> and are NOT failures. Re-prep = restore pristine `GROW.BIN`+`SCRATCH.BIN`, delete the created files.
> One ancient 31 MB SD-1.0 card (`UNAOSRW`) was refused by the Pi 4 EEPROM bootloader outright
> (no firmware output at all) — prefer the known-good 16 GB card.

### F2 — SMP-hardening of the FAT-mutation seams (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** the whole FAT-mutating stack is metal-confirmed, but the U11-M2b reaper made a SECOND
  cross-core FAT writer permanent while two single-core-only assumptions remained. This arc closes the
  ones in the pi4 lane before SMP scheduling widens (not live today — the reaper's free is
  await-verdict-sequenced strictly after all writers exit).
  - **M1 — `fat::set_fat_entry` lost-update CLOSED.** The all-copies read-modify-write of a FAT sector
    was unserialized: two cores mutating entries in the same sector (or the two mirrored FAT copies)
    could read-before-the-other's-write and clobber an update. A new aarch64-only `FAT_MUTATION`
    spinlock (`fs/fat.rs`), acquired IRQ-masked via `arch::without_interrupts` (non-preemptible → a
    proper IRQ-safe spinlock at any core count, the reaper's `IrqGuard` discipline), serializes the whole
    RMW. Span is bounded to a single FAT-sector RMW — never a free-search / data loop / `mount()` — safe
    because the aarch64 storage path is polled. aarch64-only: x86's FAT path `hlt`s awaiting an async
    xHCI event, so masking IRQs across it would hang (`with_fat_lock` is a zero-cost passthrough there,
    x86 byte-identical). `5645123`.
  - **M2 — the `sys_fgrant` grantee (asid,gen) TOCTOU CLOSED** (`arch/aarch64/syscall.rs`). The grant
    captured the grantee identity as two separate atomic loads; a child exiting + ASID-recycling between
    them could bind the grant to the recycled incarnation (misdelegation → privilege escalation /
    disclosure). Now re-validates `state==PRUNNING && pid && asid` after the gen read (an ASID's gen bumps
    only at teardown, which flips those) → `-ECHILD` on any mismatch. This was surfaced by a 23-agent
    adversarial audit of the U6 (`OWNED_FILES`) + U11 (`OPEN_FILES` / open-vs-unlink) seams. `10e3c65`.
  - **M3 — cross-core witness (with teeth).** An in-RAM stress on the SAME lock (no on-disk FAT touched,
    zero volume risk): a joinable worker on `demo_cpu` + this task's half inline race a non-atomic
    counter. On 4-core QEMU raspi4b (round-robin TCG) the UNLOCKED control raced away **~48%
    (116001/240000)** of its increments while the LOCKED path lost **NONE** — a genuine cross-core
    demonstration that the race is real AND the lock closes it. Emits an `F2-witness:` line, deliberately
    not a `-> PASS` line, so the 23-fixture count stays byte-equivalent. `55451da`.
  - **Audited SOUND / ledgered:** the U11 refcount lifecycle + the U6 owner-check are SMP-sound (verified,
    no change); two windows are benign-by-design. The remaining races are metal-latent
    (excluded-by-sequencing) and ledgered with identified fixes in [`SECURITY.md`](SECURITY.md) — most
    notably the F2 flag's OTHER named leg, `alloc_cluster`'s free-search-then-claim **cluster-aliasing**
    (SIMPLE, compare-and-claim under `FAT_MUTATION`, deferred to avoid restructuring the metal-confirmed
    allocator beyond M1's `set_fat_entry` scope), the directory-sector RMW twin, and the
    open-races-unlink / recycled-slot-ownership races that need a broader UnaFS namespace lock.
- **Tested:** `./arroyo kernel8-test` — **23 PASS** (byte-equivalent) + CAPSTONE 6/6, only the 3 expected
  M6b kills, no leaks/faults, zero R1/CMD13 error lines, **+ the `F2-witness:` line** (locked 240000/240000
  intact, unlocked lost 116001/240000). `./arroyo check` both arches; `./arroyo test-arm` MISSION SUCCESS.
  ✅ **METAL-CONFIRMED (2026-07-10, attended bench, real Pi 4 / EMMC2 card):** the full 23-PASS battery ran
  through the `FAT_MUTATION`-serialized `set_fat_entry` RMW on silicon (so M1's on-disk RMW is metal-verified),
  CAPSTONE 6/6 (all 4 cores up), zero R1/CMD13 errors, only the 3 expected M6b kills; and the F2-witness held
  under TRUE parallelism + preemption — **LOCKED 240000/240000 intact, UNLOCKED control lost 119998/240000
  (~50%, more contended than QEMU's ~48%)**. Serial: `~/unaos-bench/pi-serial-2026-07-10-110417.log`.
- **Lane:** `fs/fat.rs` (aarch64 path — seat-granted for this arc) + `arch/aarch64/syscall.rs`; **zero x86
  behavioural change** (all new code cfg-gated `target_arch = "aarch64"`).

### F3 — the UnaFS namespace / FS-metadata lock (aarch64) — every F2-ledgered race CLOSED ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** F2 serialized the single FAT-sector RMW and ledgered six residual concurrent-FS-mutation
  races (all excluded-by-sequencing today). F3 closes all six, same discipline: behaviorally-transparent
  single-core (23-PASS battery byte-equivalent), cfg-gated aarch64 locks, zero x86 OBSERVABLE-behaviour
  change (the shared `alloc_cluster` body's on-disk write ORDERING changed on x86 too — claim before
  zero-fill; end-state identical, crash-window ordering noted in SECURITY.md).
  - **M1 — `alloc_cluster` compare-and-claim** (the F2 flag's last leg, cluster-aliasing CLOSED). The
    free search stays unlocked; the CLAIM re-reads the candidate entry under `FAT_MUTATION` and sets EOC
    only if still free (loser rescans — bounded retry via the new lock-free `set_fat_entry_inner`).
    Zero-fill moves AFTER the claim (zeroing an unclaimed cluster could scribble a winner's linked
    data); the cluster is EOC-reserved but unlinked during the fill, so no reader sees stale bytes; a
    zero-fail after claim orphans it (benign lost cluster), never aliases.
  - **M2 — `DIR_MUTATION`** (directory-sector RMW lost-update CLOSED). The aarch64-only IRQ-masked twin
    of `FAT_MUTATION` wraps the three directory-sector RMW bodies (`write_dir_entry_fields`,
    `mark_dir_deleted`, `create_in_root`'s slot write — never a scan). FAT + dir locks guard disjoint
    sectors and never nest.
  - **M3 — the NAMESPACE lock** (the four sequence races CLOSED: open-races-unlink stale-chain UAF,
    `owned_clear`-vs-`owned_set_owner` recycled-slot ownership theft, create slot-claim/duplicate-name,
    the `0xE5`-before-mark-pending window's open-vs-unlink half — its last-close half was already U11's
    mark-pending-before-drop under `OPEN_FILES`; `sys_close` takes no ns). One per-mount IRQ-masked SpinMutex (`NsGuard`, in
    `arch/aarch64/syscall.rs`) spans `sys_open`'s `find_located → ACL → incref/files_alloc` (incl. the
    create path) and `sys_unlink`'s `0xE5 → owned_clear → mark-pending → descriptor-drop`. Held across
    the bounded polled directory I/O BY DESIGN, but never across `mount()`/chain frees —
    `files_free_by_dir` now collects orphan heads (`#[must_use]`), freed after the guard drops. Strict
    lock order: `NAMESPACE ⊃ {FAT_MUTATION, DIR_MUTATION, OPEN_FILES, OWNED_FILES, DEFERRED_FREE}`,
    never acquired while an inner lock is held.
  - **M4 — F3-witness** (the F2 pattern): in-RAM cross-core stress of the exact `ns_lock` guard — the
    LOCKED pass loses nothing, the UNLOCKED control races away increments under QEMU RR-TCG. Emits an
    `F3-witness:` line (not a `-> PASS` line). HONEST SCOPE: the full open-vs-unlink disk-sequence
    interleave is not provokable from the single-EL0-core QEMU battery — metal-latent, rides the bench.
- **Tested:** `./arroyo kernel8-test` — **23 PASS** (byte-equivalent) + CAPSTONE 6/6, only the 3 expected
  M6b kills, no leaks/faults, zero R1/CMD13 error lines, + the `F2-witness:` AND `F3-witness:` lines.
  `./arroyo check` both arches; `./arroyo test-arm` MISSION SUCCESS.
  ✅ **METAL-CONFIRMED (2026-07-10, attended bench, real Pi 4 / EMMC2 card, post-merge tip `8757b27`):**
  the full 23-PASS battery ran through every F3-serialized path on silicon (compare-and-claim alloc,
  `DIR_MUTATION` dir RMWs, namespace-locked open/create/unlink sequences) — CAPSTONE 6/6 (all 4 cores),
  zero R1/CMD13, no leaks/faults, only the 3 expected M6b kills, and NO stall around the open/create/
  unlink fixtures (the seat's ns-span polled-I/O latency watch: clear at real card timing). **The
  F3-witness held under TRUE parallelism + preemption: LOCKED 240000/240000 intact, UNLOCKED control
  lost 120000/240000 (exactly 50%)**; the F2-witness re-held identically (120000/240000 this boot).
  Honest scope unchanged: the two-cores-mid-syscall DISK-sequence interleave stays unprovokable until
  multiple EL0 cores run FS syscalls concurrently. Serial: `~/unaos-bench/pi-serial-2026-07-10-134423.log`.
- **Lane:** `fs/fat.rs` (aarch64 path — same seat grant as F2) + `arch/aarch64/syscall.rs`; **zero x86
  behavioural change**.

### emmc2 R1-status hardening — the card's own verdict checked after CMD17/CMD24 (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** `drivers/emmc2.rs`'s polled CMD17 (READ_SINGLE_BLOCK) and CMD24 (WRITE_SINGLE_BLOCK)
  issued the command and moved the data but ignored the card's **R1 status word** — the controller's
  interrupt error bits cover only link-level failures (CRC/timeout/index), so a card-REPORTED error
  (address out of range, write-protect violation, ECC failure, card-locked, …) was silently swallowed
  and a bad read/write looked like success. Both paths now check RESP0 against the SD Physical Layer
  §4.10.1 error mask (`R1_ERROR_MASK = 0xFFF9_8008`) immediately after the command completes, before
  touching the PIO FIFO — symmetric across the two commands (the U9 mirror discipline) — logging one
  `:: M6g: CMDnn R1 error status 0x… ::` diagnostic line and surfacing `BlockError::Io` up through
  `drivers::block` instead of returning fabricated success.
- **Tested:** `./arroyo kernel8-test 35` — **23 PASS** (all prior verdicts present incl. the full
  U9/U10/U11/U6 FAT-mutating battery), CAPSTONE 6/6, only the 3 expected M6b kills, no leaks, and no
  R1-error lines (QEMU's SD model returns clean R1, so the healthy path is proven un-regressed);
  `./arroyo check` both arches green; `./arroyo test-arm 30` MISSION SUCCESS. The error leg itself is
  unreachable under QEMU (its SD model doesn't fabricate card errors) — it is metal-relevant hardening,
  exercised on silicon only when a real card complains.
- **Commit:** see `git log` (`hw-pi4`).

### emmc2 CMD13 post-write status — programming-phase errors surfaced (aarch64) `hw-pi4`
- **What:** completes the R1 arc's story for writes. The R1 check catches a card that REJECTS CMD24,
  but a card can also fail while PROGRAMMING the block after a clean transfer (DAT0 busy phase) — and
  those errors (CARD_ECC_FAILED, generic ERROR, WP_ERASE_SKIP) appear only in a later SEND_STATUS, so
  a discarded write still looked durable. `write_block_512` now waits out programming-busy
  (`ST_DAT_INHIBIT` clear, dedicated 500 ms bound = 2× the spec's 250 ms write timeout — the plain
  100 ms command timeout would flag a slow-but-legal write), then issues CMD13 SEND_STATUS (RCA
  captured at CMD3, carried on `SdCard`) and applies the same `R1_ERROR_MASK` check — error →
  `:: M6g: CMD13 R1 error status … ::` + `BlockError::Io`. Reads are unchanged (no programming phase).
- **Tested:** `./arroyo kernel8-test 35` — **23 PASS**, CAPSTONE 6/6, no R1/CMD13 error lines, no
  busy-timeouts (every write in the U9→U11/U6 FAT battery now exits through the CMD13 check — the
  healthy path is exercised on every single write); `check` both arches; `test-arm 30` MISSION
  SUCCESS. The error leg is metal-only (QEMU never fails programming), same honest scope as R1.
- **Commit:** see `git log` (`hw-pi4`).

## hw-pi4 track — 2026-07-08 (Opus round)

### U6 — UnaFS owner/grants: the by-NAME namespace ACL, enforced at `SYS_OPEN` (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the LAST documented capability gap the U6b→U11 line left. `SYS_OPEN`/`O_CREAT`/`SYS_UNLINK`
  were gated on the HANDLE capability, but the by-NAME namespace itself was NOT ACL'd — any process could
  open/create/unlink any name. This lands the in-kernel enforcement SEAM (the on-disk UnaFS `owner`/`grants:*`
  attributes will feed it once K2/K3/K4 land; no on-disk owner format exists yet and `fat.rs` is shared/off-lane).
  **Secure-by-default (owned-by-default):** an `O_CREAT` of a NEW name records the creating principal as the
  file's OWNER (PRIVATE); the new `O_PUBLIC` mode bit (bit2) opts a create OUT to world-access. An open of an
  EXISTING owned file is admitted only for the owner or a principal the owner GRANTED; else `-EACCES`. A file with
  NO owner row (pre-existing / host-created / `O_PUBLIC`) is PUBLIC — byte-identical to the pre-U6 behaviour.
  - **M1 — the `SYS_OPEN` ACL** (`21baee8`): `OWNED_FILES`, a bounded `SpinMutex<[OwnedFile;16]>` keyed by the FAT
    file-identity `(dir_lba, dir_off)` (the `OPEN_FILES` idiom); owner + up to 4 grants, ALL fenced by the
    `(ASID, ASID_GEN)` incarnation (the recycle fence the file-id/xfer/derivation machinery already uses). Checked
    in `sys_open`'s "nothing-claimed-yet" window (a clean `-EACCES`). A private create that cannot record an owner
    FAILS CLOSED (`-ENOSPC`, undoing the fresh dir entry). The row clears at unlink (FAT may recycle the slot) and
    at OWNER teardown (`clear_handle_row` — the file reverts to PUBLIC, keeping the bounded table self-cleaning);
    its lock is IRQ-masked via the M2b `IrqGuard` (acquired in BOTH syscall and teardown contexts). The two
    cross-process U11 fixtures tag their shared file `O_PUBLIC` (one mode bit each) so B still opens A's file.
  - **M2 — `SYS_FGRANT` delegation** (`8034d0c`): `SYS_FGRANT(file_handle, child_handle, rights)` (= 18). The owner
    grants (a `CAP_READ|CAP_WRITE` subset) or revokes (`rights = 0`) access to a principal named OWNER-SCOPED by a
    `Child` handle it holds (the `SYS_XFER` idiom — no raw pid/ASID from EL0). Ownership is checked BEFORE the
    grantee is resolved (a non-owner is `-EACCES` regardless of argument). The grant is an ACL edge on the FILE —
    nothing lands in the grantee's table; it opens the name and the ACL admits it; a handle it ALREADY holds
    survives a revoke (the ACL gates ACQUISITION, not held caps).
  - **F1 — delete is OWNER-only** (`1ca2e89`, an adversarial-self-review fix): `SYS_UNLINK` is gated on OWNERSHIP,
    not merely the handle's `CAP_WRITE`. A content `CAP_WRITE` grantee (which legitimately opened the file RW)
    could otherwise `unlink` + `O_CREAT` the name to STEAL ownership and lock the real owner out — the M2 gate
    missed it because the demo granted only `CAP_READ`. Now an OWNED file is unlinkable only by its owner; a
    PUBLIC file keeps the prior `CAP_WRITE`-gated unlink (so u10delete / u11defer / u11reuse / u11reap stay
    byte-identical). Two latent hardenings folded in: an ASID-0 create is PUBLIC (ASID 0 is never torn down / gen-
    bumped, so it cannot be a gen-fenced private owner), and `SYS_FGRANT` returns `-EINVAL` for a nonzero rights
    request that names only unsupported bits (instead of silently coercing it to a revoke).
- **Tested — QEMU:** `./arroyo kernel8-test 35` → one new verdict after U11-reap:
  `:: U6-grants: owner/grants on open — non-owner -EACCES, owner grant admits R|W, grantee unlink -EACCES (delete
  owner-only), non-owner grant -EACCES, revoke re-denies -> PASS ::`. A two-process fixture (`el0-uowner-a` OWNER /
  `el0-uowner-b` GRANTEE, the u11defer choreography + the U7 proc-reserve/Child-handle pre-endow): A creates
  `OWNED.BIN` private; B (a different ASID) is DENIED (`-EACCES` — the gap closed); A `SYS_FGRANT`s B read+write →
  B opens RW + reads the matching bytes; B (a non-owner) is refused `SYS_FGRANT` (`-EACCES`) and its `SYS_UNLINK`
  is refused (`-EACCES` — delete is owner-only); A revokes → B is re-denied; A (owner) re-opens its own file
  throughout. **23 PASS** (22 prior byte-identical + U6-grants), **CAPSTONE 6/6**, only the 3 expected M6b kills,
  0 unexpected faults. `./arroyo check` both arches green; `./arroyo test-arm 30` MISSION SUCCESS; `./arroyo
  kernel8` compiles.
- **Lane:** additive on `arch/aarch64/syscall.rs` (the `OWNED_FILES` table + `sys_open` ACL + `SYS_FGRANT` + the
  uowner fixture/launcher, riding the existing `u7_launcher` chain) + docs; **NO `sched.rs` / `fat.rs` / `main.rs`
  change**; zero x86 files.
- **Honest scope (flagged, not a defect):** the owner/grants store is in-kernel, VOLATILE, boot-scoped — there are
  no persistent principals yet (the model's own documented gap), so ownership is meaningful only within a boot and
  only while the owner process lives. A persistent form (and retroactively owning pre-existing/host files) awaits
  the on-disk UnaFS attributes (K2/K3/K4). The x86 twin (U6x) and a `SYS_FGRANT` that endows a directly-usable
  handle (vs. open-by-name) remain future.
- **Metal:** 🔬 QEMU-green, metal-pending. Rides the next Pi 4 boundary alongside U10/U11 (the FAT-mutating stack).
  QEMU exercises the ACL + delegation in full (no timer/IPI needed); the metal watch-item is the same `(ASID,
  ASID_GEN)` teardown-revert path the U11 reaper walks.
- **Commit:** `21baee8` (M1) + `8034d0c` (M2) + `1ca2e89` (F1), branch `hw-pi4`.

### U11 (M2b / U12b) — the teardown-last-close REAPER: deferred-free queue + kernel reaper task (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the ONE honest-scope gap M2a left open. M2a proved the cross-process defer for the
  EXPLICIT-close path (the chain frees at the last `SYS_CLOSE`), but a program that EXITS holding the last
  cross-process open of an `unlink_pending` file cannot free its chain at teardown — that runs IRQ-masked, on the
  dying task's own kernel stack, TTBR0 on the boot root, immediately before `switch_context`, where multi-sector
  polled SD I/O is illegal. M2a therefore LOGGED that as a transient lost-cluster leak (benign, until reboot).
  **M2b actually frees it**, in a block-I/O-legal context, via two additive pieces (both aarch64 `syscall.rs`):
  - **`DEFERRED_FREE`** — a bounded (`NDEFERFREE = 16`) `SpinMutex`-guarded ring of chain heads with its OWN lock
    (separate from `OPEN_FILES`, so the teardown push never contends with live open/close). `clear_files_row`'s
    last-close-of-`unlink_pending` branch now PUSHES the head (`deferred_free_push` — lock + array write + unlock,
    I/O-free, the SAFE twin of M2a's teardown decrement) instead of logging the leak. A FULL queue degrades
    honestly to the M2a log-the-leak behavior — the push NEVER blocks, spins, or does I/O.
  - **`orphan_reaper`** — a forever kernel service task spawned at BOOT via the EXISTING
    `sched::spawn(name, fn, arg, cpu)` service-task API (`main.rs`'s aarch64-baremetal service block, alongside
    `input`/`render`; **NO `sched.rs` change — the feared scheduler hook was not needed**). It drains the queue:
    pop one head UNDER the lock, RELEASE the lock, THEN `free_orphan_chain` (mount + all-FAT-copies free — block
    I/O, legal here: EL1, IRQs enabled, its own stack). It `yield_now`s when empty so it never hogs its core under
    QEMU's cooperative scheduler, and is co-located with the demo VERDICT core so the launcher's bounded
    `yield_now` poll cedes it CPU to drain deterministically.
  Freed EXACTLY once: a teardown-orphaned chain reaches the queue ONLY via `openfile_decref_at` returning
  `Some(fc)` (last-close-of-pending, the row already EMPTY) — queued once, reaped once; the explicit-close path
  frees inline and NEVER queues, so no chain is both freed inline and queued. SMP-safe (a teardown on core X
  pushes, the reaper on core Y drains — one `SpinMutex`, no torn reads, no lost/duplicated entries).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after U11-reuse, one new verdict:
  `:: U11-reap: teardown-last-close reaper — A exits holding the unlinked file open, its chain freed by the reaper
  (all FAT copies) + re-allocatable, no teardown leak -> PASS ::`. A two-process fixture (`el0-u11reap-a`/`-b`,
  the u11defer choreography): A creates + writes + holds `DEFER2.BIN` open; B opens + `SYS_UNLINK`s it (deferred —
  A holds it); A reads its ORIGINAL bytes back (chain alive), then **EXITS WITHOUT CLOSING** — so TEARDOWN is the
  last close. The launcher proves the same three checkpoints as u11defer, but CHECKPOINT-3 is a bounded
  `yield_now` poll of the FAT until the REAPER has freed the chain (all FAT copies) + it is re-allocatable
  (`first_free == f0`); the serial shows `U11-defer: reaper freed teardown-orphaned chain @cluster N` and the M2a
  `"teardown … left allocated (leak)"` line does NOT appear for `DEFER2.BIN` (reaped, not leaked). **22 PASS** (21
  prior byte-identical + U11-reap — sorted scratch-worktree baseline diff vs `d57520f`, a single appended line),
  **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults. `./arroyo test-arm 22` green (baremetal-
  gated); `./arroyo check` both arches, `./arroyo kernel8` compiles; **zero x86 files**.
- **Lane:** aarch64 `syscall.rs` (`DEFERRED_FREE` + `orphan_reaper` + the `clear_files_row` push + the reap
  fixture/launcher) + the aarch64-baremetal service-spawn block of `main.rs` (additive, cfg-scoped) + docs; zero
  x86 files. `SpinMutex` reused from `sched.rs`; **NO `sched.rs` change** and **NO `fat.rs` change** (the reaper
  calls M2a's existing `free_chain`). 🔬 Metal-pending (pure syscall lifecycle + a kernel service task + FAT free;
  QEMU proves the reaper in full — rides the next Pi boundary alongside U10/U11-M1/M2).
- **Seat coalesce-review fix (1 confirmed should-fix, in-lane):** `DEFERRED_FREE` is a bare `spin::Mutex` acquired
  in two IRQ-ASYMMETRIC contexts — `deferred_free_push` in the IRQ-masked teardown, but `deferred_free_pop` in the
  reaper TASK body, which the metal timer PREEMPTS (task bodies run I-unmasked; `SCHED_ACTIVE` on). A timer preempt
  of the reaper while it holds the lock, then a **same-core** teardown push spinning IRQ-masked on it, deadlocks
  that core (run queues never migrate → the preempted holder never releases). Dormant in a healthy ≥2-AP boot (reaper
  on `online.get(1)` vs EL0 fixtures on `online.first()` — distinct cores) but **live in the single-AP fallback**
  (`reaper_cpu`/`vcpu`/`demo_cpu` collapse onto one AP; the pi4's 3/4-core boot variance can produce it). Fixed by a
  local RAII `IrqGuard` (save/restore DAIF — the `sched.rs` `irq_save_mask` idiom, kept local so M2b **still touches
  no `sched.rs`**) masking IRQs across **both** `deferred_free_push`/`pop`, making the hold non-preemptible → a
  proper IRQ-safe spinlock at any core count. Also folded the log nit: `free_orphan_chain` returns `bool`; the reaper
  logs "reaper freed …" only on a SUCCESSFUL free (its error/leak line covers failure). Byte-transparent under QEMU
  (cooperative — no preemption): **22 PASS**, CAPSTONE 6/6, reaper freed cluster 8, no teardown-leak line, 0 faults;
  `check` both arches; `test-arm` clean; zero x86 files. **Flagged for a future SMP-hardening arc:** the
  reaper's downstream `fat::set_fat_entry` read-modify-write is unlocked across cores (no FS-mutation lock spanning
  the RMW) — a latent lost-update/cluster-aliasing race should the OS ever run uncoordinated concurrent FAT writers;
  not live today (the reaper's free is await-verdict-sequenced after all writers exit), and its fix touches `fat.rs`
  (out of this lane). **→ Addressed by the F2 arc (2026-07-10, above):** the lost-update leg is CLOSED (`FAT_MUTATION`
  serializes the `set_fat_entry` RMW, `5645123`) and witnessed cross-core (`55451da`); the cluster-aliasing leg is
  audit-confirmed with its fix specified (compare-and-claim under `FAT_MUTATION`) and ledgered in `SECURITY.md`,
  deferred to avoid restructuring the metal-confirmed allocator beyond M1's scope.

### U11 (M2 / U12) — cross-process unlink-defers-free: a global open-file refcount + deferred chain-free (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the SECOND (and last) of U10's two review notes — **cross-process unlink-while-open** (POSIX
  unlink-defers-free). U10 freed a file's cluster chain the instant `sys_unlink` ran; if ANOTHER process held the
  file open, its live descriptor kept pointing at a chain a later create+grow could first-fit-reuse — a cross-file
  read/write + information disclosure. The fix defers the chain-free to the file's LAST close across ALL processes:
  - **A global (cross-ASID) open-file refcount table** (`OPEN_FILES`, `NOPENFILE = 16` rows, joined by the on-disk
    identity `(dir_lba, dir_off)`), guarded by a single **`SpinMutex`** — the scheduler's low-level spinlock, NOT
    the sleeping `Mutex` (which yields, illegal in the IRQ-masked teardown path). One row is shared by every open
    of a file across every ASID; `refcount` counts its live descriptors. The lock is held ONLY across the short
    table mutation (±refcount, read the stashed chain head) — NEVER across `mount()`/`free_chain` block I/O.
  - **`sys_open` increments, `files_free` decrements.** The increment is BEFORE `files_alloc`, so every increment
    pairs with exactly one decrement — the descriptor's `files_free` (close/teardown) or a one-line
    `openfile_decref_at` on the `files_alloc`-full unwind — with no path where a decrement lands on a row this open
    never incremented (an SMP race the after-`files_alloc` ordering would open). A full table on a NEW identity is
    a clean **`-ENFILE` (`-23`)** with reserve/unwind claim-last on every failure path.
  - **`sys_unlink` marks + defers.** It writes the directory entry `0xE5` FIRST (`fat::mark_dir_deleted` — the
    name is GONE immediately, a re-open is `-ENOENT`), marks the refcount row `unlink_pending` + stashes the chain
    head, then drops all of the caller's descriptors naming the file (`files_free_by_dir`, each decrementing). If
    the caller is the SOLE opener, the last decrement reaches 0 and frees the chain NOW — byte-identical to U10. If
    ANOTHER process still holds it open, the refcount stays > 0 and nothing is freed: that process keeps reading
    its original bytes until its last close.
  - **`sys_close` frees at the last close.** The decrement that drops an `unlink_pending` file's refcount to 0
    returns the stashed chain head, and `sys_close`/`sys_unlink` free it (`fat::free_chain`, all FAT copies) in
    syscall context AFTER releasing the lock — block I/O is legal there.
  - **`fat::delete_located` split** into `mark_dir_deleted(dir_lba, dir_off)` (the `0xE5` write) + `free_chain(
    first_cluster)` (the all-FAT-copies chain free); `delete_located` remains their pre-validated composition for
    the immediate path (the U10 "bad chain → nothing changed" contract holds byte-for-byte).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U11 (M1) PASS, one new verdict:
  `:: U11-defer: cross-process unlink-defers-free — name gone at unlink, reader keeps original bytes, chain freed
  (all FAT copies) + re-allocatable at last close -> PASS ::`. A **two-process** fixture (the U7 GO-word
  choreography, launcher as the single sequencer): process A (`el0-u11defer-a`) creates + writes + reads
  `DEFER.BIN`; process B (`el0-u11defer-b`) opens + `SYS_UNLINK`s it while A holds it open. The launcher re-mounts
  the FAT at THREE checkpoints — after B's unlink the NAME is gone but the chain (cluster `f0`) is STILL allocated
  in all FAT copies; A then seeks + reads its ORIGINAL 16 bytes back (the deferred chain is alive); after A's last
  `SYS_CLOSE` the chain is FREED in all FAT copies + re-allocatable (`first_free == f0`). Both A and B drop their
  handles via explicit syscalls (B's unlink, A's close), so no teardown I/O is needed. **20 PASS** (19 prior +
  U11-defer), **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults, no leak lines. Every prior
  verdict **byte-identical** (sorted scratch-worktree baseline diff vs `d0d12ef` — a single appended U11-defer
  PASS line; the binary-growth `VBAR`/EL0-fault-ELR shift touches only diagnostic addresses, not any verdict).
  `./arroyo test-arm 22` green (baremetal-gated); `./arroyo check` both arches; **zero x86 files** (the `fat.rs`
  split is additive — x86 never calls the new writers).
- **Adversarial review (3 lenses — refcount pairing / double-free+leak+FAT-safety / SMP+teardown lock discipline —
  with per-finding refuters).** Lock discipline, teardown-I/O-safety, SMP incref ordering, and deadlock lenses all
  clean. **1 real should-fix, caught + fixed in-arc:** the refcount row was originally keyed only on
  `(dir_lba, dir_off)`, a directory SLOT which FAT recycles for a new file the moment a delete `0xE5`'s it — so a
  file created in an unlinked-but-still-open file's reused slot could conflate the two files' refcount + deferred
  free (a lost-cluster leak / mis-triggered free; not a UAF/disclosure — refcount stayed exact). This is the M1
  descriptor-slot-reuse hole one level up, which generation tags don't cover. Fixed by (a) excluding
  `unlink_pending` rows from the open-time join (a `0xE5`'d file can't be re-opened by name, so a key match on a
  pending row is a different file → claim a fresh row) and (b) recording each descriptor's row INDEX in
  `FILE_OPENROW`, so every decrement/mark hits that exact row, never a re-searchable key. A targeted re-verify
  confirmed the fix closes it with no new leak/double-free/stale-index path.
- **Seat coalesce-review fix (a SECOND, distinct slot-recycle bug; refuted 0/3 — own follow-up commit).** The
  above fixed the incref/JOIN side; the `sys_unlink` SWEEP side had the same root: `files_free_by_dir` matched
  descriptors by the recyclable `(dir_lba, dir_off)`, so when ONE process held an unlinked file F open AND created
  a new file G in F's recycled `0xE5` slot, a `SYS_UNLINK` of G swept BOTH descriptors and both reached
  `refcount == 0`, but the loop kept only the LAST orphan chain head (`orphan = Some(fc)` overwriting) → the
  earlier chain leaked. A benign lost-cluster leak on the EXPLICIT path (refcount stayed exact, no UAF/double-free),
  distinct from the teardown-only honest-scope leak below. Fixed by freeing EVERY orphan head the sweep produces
  (`files_free_by_dir` is `sys_unlink`-only = block-I/O-legal, so it frees each inline via `free_orphan_chain`; the
  `OPEN_FILES` lock is never held across the I/O). Proven by a deterministic kernel-side check
  (`u11defer_check_double_orphan`, the `u11_check_gen_rebind` style — physically recycle a slot asserting G reused
  F's exact slot, reproduce the two-descriptor/two-pending-row state, run the real sweep, fresh-mount confirm BOTH
  chains free in all FAT copies): `:: U11-reuse: sys_unlink slot-recycle — two files sharing a recycled dir slot
  BOTH free their chains (all FAT copies) -> PASS ::`. **Now 21 PASS** (the prior 20 byte-identical + U11-reuse),
  CAPSTONE 6/6, `./arroyo check` both arches, zero x86 files.
- **Honest scope — M2a (explicit-close path) landed; the teardown-last-close gap is now CLOSED by M2b (above).**
  The cross-process defer is proven for the explicit-close path, and the teardown DECREMENT is done (a short,
  I/O-free `SpinMutex` critical section, safe in the IRQ-masked teardown context). A program that EXITS holding
  the LAST `unlink_pending` open cannot free its chain in teardown (IRQ-masked, dying stack, block I/O illegal),
  so M2a LOGGED it as a transient lost-cluster leak — now REAPED by M2b's deferred-free queue + `orphan_reaper`
  (which turned out to need NO `sched.rs` hook — the existing `sched::spawn` service-task API sufficed).
- **Lane:** aarch64 `syscall.rs` + `fs/fat.rs` (the `delete_located` split — additive, reuses `set_fat_entry`/
  `chain_clusters`) + docs; zero x86 files. `SpinMutex` reused from `sched.rs` (not reimplemented); no `sched.rs`
  change. 🔬 Metal-pending (pure syscall lifecycle + FAT free; rides the next Pi boundary alongside U10/U11-M1).

### U11 (M1) — open-file lifecycle: `SYS_CLOSE` + generation-tagged file-ids (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** gives a `File` descriptor a real end-of-life and a stable identity across slot reuse, closing the FIRST
  of U10's two review notes — the **same-process sibling-handle rebind on slot reuse**. U10 left a per-task FILES
  row as a bare `+1`-biased slot index with NO generation, so after an unlink freed a slot (`files_free_by_dir`
  invalidating a multiply-opened file's descriptors) a later `sys_open` first-fit-REUSED it and a lingering sibling
  handle silently REBOUND to the different file. The fix is the standard slotmap/generation-index:
  - **`FILE_GEN[asid][idx]`** — a per-slot generation counter, bumped on every free (`files_free` — the one free
    primitive `SYS_CLOSE`/`files_free_by_dir`/`sys_open`-unwind all route through — and `clear_files_row` at
    teardown).
  - **Packed file-ids** — the `File` handle's value word now carries `(gen << 32) | (idx + 1)` (`file_id_pack`);
    generation 0 packs to exactly `idx + 1`, byte-identical to the pre-U11 bare file-id.
  - **`file_desc_validate`** — the SINGLE seam decoding a value word to a live descriptor index (range + `FILE_USED`
    + **gen == the slot's current gen**). Every File consumer (`sys_read`/`sys_write_file`/`sys_seek`/`sys_unlink`/
    `sys_close`) funnels through it, so a stale handle to a reused slot is `-EACCES` (a gen mismatch, EVEN when
    `FILE_USED` is true again) — no rebind, and no per-syscall re-derivation (five open-coded checks → one).
  - **`SYS_CLOSE = 17`** — resolves the handle for NO right (a close is always permitted on a handle you hold),
    frees the descriptor slot (bumping the gen), and clears the handle → `0`. A `Console`/`Socket`/`Child` kind is
    `-EINVAL` (object table untouched — not closeable this arc); an unresolvable / already-closed / stale-slot
    handle is `-EBADF` (`-9`) — double-close and use-after-close are clean.
  `handle_resolve` is UNCHANGED (still returns `File(raw)`), so the U6/U9 scaffold checks that resolve File handles
  directly are byte-identically unaffected; the gen validation lives one layer out. For programs that never close,
  open→read/write→teardown is byte-identical to U10 (teardown drops the descriptor exactly as a close would).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U10-delete PASS, one new verdict:
  `:: U11: open-file lifecycle — SYS_CLOSE + gen-tagged file-ids: close/double-close/round-trip OK, stale sibling to
  a reused slot -EACCES (gen mismatch, no rebind), A11 unlinked + B11 present -> PASS ::`. A register-only fixture
  (`el0-u11close`, witness `0x1F`) creates + grow-writes `A11.BIN`, opens it a SECOND time (a sibling descriptor),
  `SYS_UNLINK`s via the first handle (freeing both A11 descriptors, the sibling left lingering on a freed slot),
  REUSES the freed slots by opening + writing `B11.BIN`, then proves a read through the STALE sibling is `-EACCES`
  (the slot is LIVE again for B11, so the denial can ONLY be the stale generation — not a rebind onto B11's bytes),
  and exercises `SYS_CLOSE` → `0` / double-close → `-EBADF` / close→re-open→read round-trip. A kernel-side
  `u11_check_gen_rebind` proves the mechanism in isolation on a scratch ASID (claim → mint id → free (bump) →
  re-claim the same slot → the OLD id is rejected while the slot is live, the FRESH one resolves), and a fresh
  `mount()` confirms `A11.BIN` gone + `B11.BIN` present with its content. **19 PASS** (18 prior + U11),
  **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults. Every prior verdict **byte-identical**
  (sorted scratch-worktree baseline diff vs `ddefc40` — a single appended U11 line; no VBAR/USER_REGION shift
  surfaced in any verdict). `./arroyo test-arm 22` green (baremetal-gated); `./arroyo check` both arches; **zero
  x86 files** (the design is the pi4 lead — its x86 twin U11x is a later rmbp arc).
- **Honest scope:** M1 (generation tags + `SYS_CLOSE`) lands and closes hole 1. **Deferred to the next arc (U11 M2
  / U12):** the SECOND U10 note — **cross-process unlink-while-open** (POSIX unlink-defers-free) — needs a global
  cross-ASID `(dir_lba, dir_off)`-keyed open-file refcount + a `fat::delete_located` split. It was NOT half-landed:
  its blocker is that a program which exits WITHOUT closing must still trigger the deferred chain-free at TEARDOWN,
  and teardown (`exit` → `teardown_user_slot` → `clear_handle_row`) runs **IRQ-masked, on the dying task's kernel
  stack, TTBR0 already repointed to the boot root, right before the context-switch away** — doing multi-sector
  polled SD I/O there (plus a new global Mutex-guarded refcount table + SMP-safety proof) is arc-deep and likely
  wants a reaper, not inline teardown I/O. The `O_CREAT` ambient-namespace gap stays the future UnaFS ACL arc.
- **Lane:** aarch64 `syscall.rs` only — no `fat.rs`/`block.rs`/`arroyo` change (the fixture creates its own files).
- **Metal:** 🔬 QEMU-green, **metal pending** — pure syscall lifecycle (the generation tag + `SYS_CLOSE` are
  QEMU-provable in full); rides the next Pi 4 boundary alongside the still-pending U10 metal-verify.
- **Commit:** on `hw-pi4` (Opus-executed) — see git log.

### U10 — file GROWTH + CREATE + DELETE: cluster allocation, FAT-chain extension, directory mutation (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** gives `fat::write_at` (U9's in-place-only writer) an ALLOCATOR, so `CAP_WRITE` can now **extend and
  create** files — the three restrictions U9 kept (never write the FAT, never allocate, never touch a directory)
  are lifted in rising order of risk. New `fat.rs` primitives, all FAT-safety-critical:
  - **`set_fat_entry`** — writes EVERY FAT copy (`num_fats`, 2 on this card) at the mirrored offset, read-modify-
    write per copy (neighbours preserved), FAT32 reserved high-nibble preserved. A one-FAT write is a corrupt
    volume, so it always mirrors.
  - **`alloc_cluster`** — a bounded first-fit free search over `[2, count+2)`; **zero-fills the cluster BEFORE it
    can join a chain** (no stale-byte information disclosure), marks it EOC in all copies; `-ENOSPC` (new
    `FatError::NoSpace`) when full. Never spins, never returns a reserved/bad/out-of-range cluster.
  - **`find_located` / `write_dir_entry_fields`** — locate a directory entry's on-disk `(LBA, slot)` and RMW its
    `first_cluster`/`size`, so the reader's source of truth (dir `size`) is bumped **last**.
  - **`write_grow`** — walk chain → alloc+zero+link new clusters → RMW the data → publish dir `size` LAST. A
    crash before the last step leaves the OLD smaller size on disk, never a size claiming unwritten clusters.
  - **`create_in_root`** — format an 8.3 name (`format_83`), find a free directory slot (`0x00`/`0xE5`), write a
    fresh 0-length entry; the first grow-write allocates its first cluster. Root-dir full → `-ENOSPC`.
  - **`delete_located`** — crash-safe order: mark the dir entry `0xE5` FIRST, then free the whole chain (all FAT
    copies); a crash mid-delete leaves lost clusters (benign), never a live entry aliasing freed clusters.
  A refactor keeps the read path honest: `fat_entry` delegates to the new `pub fat_entry_copy` (one FAT-offset
  site), and `scan_dir_sector` shares the on-disk 8.3 parse with the new locator via `classify_dir_slot`/`DirSlot`
  (single source of truth). **Syscall surface:** the per-task FILES descriptor gains the dir `(LBA, off)` (captured
  at `sys_open`); `sys_write_file` splits — `len <= bytes-to-EOF` keeps U9's in-place path byte-identical, a write
  past EOF routes to `write_grow` (capped `GROW_WRITE_MAX = 8 KiB`); `SYS_OPEN` gains **`O_CREAT`** (mode bit1,
  endows RW); **`SYS_UNLINK = 16`** deletes via a File+`CAP_WRITE` handle. Grow, create, AND delete are all
  reachable ONLY through `sys_write`'s single `handle_resolve(asid, fd, CAP_WRITE)` CHECK (create via an O_CREAT
  open that endows `CAP_WRITE`), so an RO-opened / revoked (U7/U8 walk inside `handle_resolve`) / wrong-kind handle
  is `-EACCES` and can never mutate the volume — no new enforcement code.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U9 PASS, three new verdicts:
  `:: U10: file growth — … on-disk size grew + appended data present + both FAT copies consistent -> PASS ::`,
  `:: U10-create: file create — … on-disk entry present with right size + content, no duplicate -> PASS ::`,
  `:: U10-delete: file delete — … on-disk entry gone + chain freed (all FAT copies) + cluster re-allocatable -> PASS ::`.
  Three register-only single-slot fixtures (`el0-u10{grow,create,delete}`, witnesses `0x1F`/`0xF`/`0x1F`) drive the
  three ops through the syscall surface; each launcher folds the kernel-side proof — a **fresh `mount()` re-read**
  shows the on-disk `size` grew / the entry appeared / the entry vanished, the appended-or-written bytes are on the
  card, the original clusters survived, both FAT copies agree along the chain, and a deleted file's cluster is free
  again (first-fit re-allocatable). The delete fixture also opens its file TWICE and proves that after unlinking via
  one handle a read through the SIBLING handle is `-EACCES` (no stale reference to the freed chain). **18 PASS** (15 prior + the 3 U10), **CAPSTONE 6/6**, only the 3 expected M6b
  EL0 kills, 0 unexpected faults. Every prior verdict **byte-identical** (sorted scratch-worktree baseline diff vs
  `ca1b765` — only the new U10 lines differ, plus the binary-growth `VBAR_EL1` shift `0xad800`→`0xb1000` and the
  identity-mapped `USER_REGION` address shift it drags along, which move the M6b EL0-fault diagnostic ELR/FAR — the
  M6b **verdict** line is byte-identical). `./arroyo test-arm 22` green (the module is baremetal-gated); `./arroyo
  check` both arches; **zero x86 files** (the new FAT writers are additive — x86 never calls them).
- **Honest scope:** grow + create + delete land. **Deferred:** subdirectories, LFN (long names), rename,
  write-back caching (every mutation is a synchronous RMW to the card), root-directory-chain extension (root-dir
  full → `-ENOSPC`), and UnaFS `owner`/`grants:*` on the namespace ops (open/create/unlink are cap-gated on the
  *handle* for read/write/delete, but the by-name namespace itself is not ACL'd yet — the future UnaFS layer).
  Deletion invalidates ALL of the caller's descriptors naming the file (a same-process double-open leaves the
  sibling fail-safe, `-EACCES`); the remaining gap is CROSS-process (a file open in another process when unlinked
  leaves that process's descriptor stale — needs a global open-file refcount / `SYS_CLOSE`, deferred). An `O_CREAT`
  that then fails to claim a kernel handle leaves a harmless 0-length entry (no kernel leak). Names are strict 8.3
  (trailing-dot / multi-dot / non-representable names → `-EINVAL`).
- **Lane:** the seat's U9 widening stands — the pi4 lane covers the shared `fat.rs`/`block.rs` + the FAT image
  builder in `arroyo` for this arc (`block.rs` untouched this arc). All FAT writers are aarch64-only by call site.
- **Metal:** 🔬 QEMU-green, **metal pending** — rides the next Pi 4 boundary. ⚠ **This is the first arc that
  ALLOCATES + MUTATES the FAT and directory on a real card — the metal-risk beyond U9's single in-place sector
  (multi-sector zero-fill, both-FAT mirrored writes, directory RMW).** The launchers' fresh-mount re-read is the
  proof; the boundary should re-copy pristine `GROW.BIN` (0xC1, 512 B) between runs, as U9 does for `SCRATCH.BIN`.
- **Commit:** on `hw-pi4` (Opus-executed) — M1 `a10c4b5`, M2 `5f1b0a3`, M3 `d02fb9c`, review fixes `dac149a`, docs `45bff90`.

## hw-rmbp track — 2026-07-08 (U9x — File writes + seek, the pi4 U9 twin, Opus-executed)

### U9x M1 — real File writes + seek on x86 as a STAGED WRITE-BACK: `SYS_SEEK`, RW `SYS_OPEN`, File+`CAP_WRITE`-routed `sys_write` into a per-descriptor in-memory buffer (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U9 — gives U6bx's read-only `File`+`CAP_READ` its WRITE half, structurally
  faithful to the pi4 lead at the CAPABILITY layer and diverging ONLY where the hardware forces it. **The
  load-bearing divergence (the same one U6bx's read side pays):** aarch64 U9 writes straight to the SD card
  INSIDE the SVC handler (its EMMC2 driver is PIO). x86 CANNOT — storage is USB-over-xHCI whose BOT pump
  `hlt()`s awaiting async completion, and the SYSCALL handler runs IF-masked (SFMASK clears IF), so a `hlt`
  at IF=0 never wakes: no disk I/O in-handler. So the write is **STAGED** — the write twin of the staged read.
  (1) **`SYS_SEEK = 15`** — a near-verbatim mirror of aarch64 `sys_seek`: File + ANY of `CAP_READ|CAP_WRITE`,
  `-EINVAL` strictly past size (seeking TO size is legal), sets `FILE_OFFSET` (now settable). (2) **RW
  `SYS_OPEN`** — a mode bit in `a2` (`0` = RO/`CAP_READ`, byte-identical to U6bx; `1` = RW/`CAP_READ|CAP_WRITE`;
  backward-safe because the first-entry GPR scrub zeroes `rdx`, so the old U6bx open reads mode 0). A RW open
  claims a per-descriptor **writable staging buffer** from a small fixed pool (`NWSTAGE`, one page each,
  per-slot single-writer, `static mut` mirroring `HELLO_BYTES`), SEEDED from the file's staged content and
  tracked by a `+1`-biased `FILE_WSTAGE` sidecar; reserve/unwind is claim-last (slot→descriptor→handle) on
  every failure path. (3) **Routed writes** — `sys_write` is KIND-DISPATCHED at its single
  `handle_resolve(row, fd, CAP_WRITE)` CHECK: a `Console` streams to serial (byte-identical), a `File` with
  `CAP_WRITE` overwrites its writable buffer IN PLACE at the descriptor offset via `sys_write_file` — a pure
  **memcpy** (IF-masked-handler-safe), clamped to `min(len, size-offset)` (never grows), whole source
  validated up front (`-EFAULT` with no offset move), offset advanced by the count written. (4) **Read-back
  through the SAME cap** — `sys_read` serves a RW descriptor from its writable buffer, so a seek-back-and-read
  WITNESSES the write. The CHECK inherits U7x/U8x for free (the derivation/revocation walk lives inside
  `handle_resolve`, so a revoked write-cap is `-EACCES` at the write with no new code). **Three denials:** a
  File opened RO written to (rights arm), a non-File `Socket` carrying `CAP_WRITE` (kind arm), and a
  U8x-revoked File-write cap all `-EACCES`.
- **Honest scope — M1: IN-MEMORY ONLY, NO DISK WRITE-BACK.** `SCRATCH.BIN` is a const in-memory seed (1 KiB
  of `0xEE`, always present regardless of disk), NOT read from FAT; the read-back is the write witness — there
  is no on-disk "sector changed" check. **M2** (deferred, the completing twin) adds: capturing the FAT cluster
  chain at stage time + a per-descriptor dirty flag + a BSP-side write-back flush pump (reusing the existing
  shell-proven `fat::write_at`→`block::write_block` path from the polled main loop, IF=1) + planting
  `SCRATCH.BIN` on the x86 FAT image. M1 landed as a complete, honest capability milestone rather than
  half-landing the flush. Also deferred: file growth/create/delete/truncate; directory mutation; IF-safe
  interrupt-driven x86 storage (retires the staged-buffer divergence for both read and write); UnaFS
  `grants:*` on `SYS_OPEN` (K2/K3).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → after the U8x PASS: the U9x setup line and
  `:: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES
  (staged in-place, no disk write-back this milestone) -> PASS ::`. The `u9x-write` fixture (single slot,
  register-only apart from the read-back dest, witness `0x1F`) opens `SCRATCH.BIN` RW, seeks to a
  partial-sector offset (520), overwrites a 16-byte sentinel IN PLACE, seeks back and reads it through the
  SAME cap, and proves the RO-write + wrong-kind denials; `u9x_check_revoked_write` stages the U8x-revoked-
  File-write denial over scratch row 5 and demands the handle/file/writable-pool/derivation ledgers all clear.
  **Every prior U1a→U8x verdict byte-identical** (sorted scratch-worktree baseline diff vs `ca1b765` — pure
  append of the U9x lines). Default no-FAT `./arroyo test` stays MISSION SUCCESS AND U9x **runs and PASSes**
  there too — its in-memory scratch needs no FAT volume, which DIVERGES from the HELLO.BIN-dependent demos
  (U2/U4x/U6x/U6bx), all of which skip cleanly without a FAT. `./arroyo check` both arches, **zero aarch64
  files touched** (`arch/x86_64/syscall.rs` only — the write stack `fat.rs`/`block.rs`/`drivers/xhci` untouched).
- **Metal:** CANNOT be metal-confirmed — the standing x86 xHCI enumeration blocker (`task_47291f90`) means the
  rMBP enumerates no mass-storage device, so `block::info()` is None and U9x's storage-gated demo SKIPS on
  metal exactly as U8x/U7x do. QEMU-green is the ceiling until that blocker clears; the gate is NOT relaxed.

### U9x M2 — real disk write-back: FAT cluster capture, a per-descriptor dirty flag, a flush queue past teardown, and the launcher's IF=1 flush + raw-sector re-read (x86) 🔬 `hw-rmbp`
- **What:** completes the pi4 U9 twin — persists M1's staged in-memory write to the FAT on disk, the honest way
  around the IF-masked handler. M1 proved the whole capability surface with an in-memory `wstage` buffer (a
  read-back witnessed the write); M2 FLUSHES that dirty buffer to the FAT via the shell-proven
  `fat::write_at`→`block::write_block` path, at IF=1, OUTSIDE the SYSCALL handler, and the launcher raw-re-reads
  the sector to prove it landed. **THE crux (why M1 stopped here):** the dirty `wstage` buffer is freed at the
  fixture's teardown (`clear_files_row`, IF=0), but the flush needs IF=1 disk I/O — a naive "flush at teardown"
  cannot work. **Resolution — a flush queue that survives teardown, drained by the demo launcher at IF=1.**
  Teardown COPIES each dirty descriptor's bytes + its `(cluster, size, [lo,hi))` into a small static flush queue
  (the copy is self-contained — a stranded entry can never dangle at a freed buffer), then frees the `wstage`
  slot exactly as M1 (so the reviewed teardown-clear proof, `wstage_all_free()`, is undisturbed). The launcher —
  on the demo AP at IF=1, where the xHCI BOT pump self-services its own event ring (drains DMA'd ring memory +
  acks the interrupter via raw MMIO, so it drives from ANY CPU) — drains the queue via `fat::write_at` (in place,
  never grows), then raw-re-reads the sector.
- **The additions:** (1) a launcher **pre-flight** (IF=1, gated on `HELLO_STAGED`, the BSP's FAT-present signal)
  mounts + `find_in_root`s SCRATCH.BIN, validates it (regular file, non-zero chain head, size == the staged/wstage
  length), captures the pre-image bytes at the write offset, and publishes the chain head to `SCRATCH_CLUSTER`
  (Release) BEFORE the fixture opens — the x86 stand-in for pi4 capturing `FILE_CLUSTER` at open (x86 cannot walk
  the FAT in the IF-masked handler). (2) `sys_open` records `FILE_CLUSTER` from it; a per-descriptor `FILE_DIRTY`
  + dirty range `[lo,hi)` set by `sys_write_file` (fresh on the first write, so the flushed span is EXACTLY the
  touched sectors, never from offset 0). (3) The **revoke ordering** — a whole-task TEARDOWN (`clear_files_row`)
  ENQUEUES dirty; a REVOKE / open-unwind (`files_free`) DISCARDS dirty (revoke repudiates the write, so a revoked
  cap never flushes stale bytes). (4) `SCRATCH.BIN` (1 KiB of `0xEE`) planted on the x86 FAT image
  (`make-fat-img.sh`), mirroring the pi4 plant; its bytes equal M1's const seed, so the in-memory core is
  byte-identical whether or not a FAT is present. **Folded the two M1 review notes:** `sys_write_file`'s offset
  advance is now a tx-exact `compare_exchange` claim (CAS-symmetric with `sys_read`), and a writable (`CAP_WRITE`)
  open on `SHARED_ROW` is refused (a shared writable descriptor could race the unsynchronized staging memcpy).
- **DUAL MODE:** disk-backed (a FAT volume backs SCRATCH.BIN — `test-fat sf`) REQUIRES the on-disk write-back
  proof; in-memory (no FAT — plain `./arroyo test` attaches a non-FAT usb.img) runs the M1 core with the flush a
  no-op and does ZERO AP disk I/O (the pre-flight is `HELLO_STAGED`-gated, so a no-FAT run never issues an AP xHCI
  read). The revoke-discard proof is NON-VACUOUS: `u9x_check_revoked_write` first shows (positive control) that a
  dirty descriptor run through `clear_files_row` DOES enqueue a flush, THEN shows the revoke path leaves the queue
  empty — only the contrast proves discard. Verdict evolution: M1 `staged in-place, no disk write-back this
  milestone` → M2 `staged write FLUSHED to FAT (on-disk sector changed + size unchanged)`.
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → the U9x line now proves the on-disk write-back:
  `:: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES, staged
  write FLUSHED to FAT (on-disk sector changed + size unchanged) -> PASS ::`. The launcher's raw re-read of the
  flushed sector (fresh mount, `read_at` chain-walk) finds the 16-byte pattern at offset 520, differing from the
  `0xEE` pre-image, with the directory size unchanged (1024) — the pi4 U9 proof, in QEMU. **All 16 prior
  U1a→U8x PASS/FAIL verdicts byte-identical** (sorted scratch-worktree baseline diff vs `ddefc40`; the ONLY
  verdict delta is the U9x line evolving M1→M2). (Two *descriptive* setup banners show best-effort-console-drop
  jitter — the baseline run itself dropped U7x's banner while this run kept it and dropped U6bx's — a pre-existing
  timing artifact of AP-side console contention, not a behavioral change: every demo ran and PASSed identically.)
  Default no-FAT `./arroyo test 25` stays MISSION SUCCESS AND U9x PASSes its in-memory core (`in-memory core; no
  FAT volume, flush is a no-op`), with zero AP disk I/O. `./arroyo check` both arches, **zero aarch64 files
  touched** (`arch/x86_64/syscall.rs` + `make-fat-img.sh` only — the write stack `fat.rs`/`block.rs`/`drivers/xhci`
  REUSED unchanged via its existing API). Adversarial code review (atomics/ordering, lifecycle/leaks, flush
  correctness, revoke-discard proof), 0 confirmed findings.
- **First concurrent AP-side xHCI BOT I/O in the tree:** the disk-backed flush is the first time a demo AP drives
  the hlt-pumped xHCI BOT engine while the BSP main loop also services xHCI (they serialize on the single
  `XHCI_CONTROLLER` lock). The pump is BOUNDED (a 2000-iter timeout → `Io`), so a failure would be a LOUD verdict
  FAIL, never a hang; `test-fat sf` is its empirical proof.
- **Metal:** unchanged from M1 — CANNOT be metal-confirmed (the xHCI enumeration blocker `task_47291f90` leaves
  `block::info()` None on the real rMBP, so the whole demo SKIPS there; QEMU-green is the ceiling, gate NOT
  relaxed). Unlike pi4 U9 (metal-confirmed on real EMMC2), x86 U9x waits on that blocker.
- **Deferred / tracked:** generation-tagged file-ids (a bare descriptor index lets a revoke+reopen alias a reused
  slot — real but not ring-3-reachable this arc; the x86 twin of pi4 U11, lands as **U11x**). File
  growth/create/delete (U10 twin); IF-safe interrupt-driven x86 storage (retires the staged-buffer divergence +
  the AP-flush detour); UnaFS `grants:*` on `SYS_OPEN`.
- **Commit:** `538a1bf` (`hw-rmbp`, Opus-executed).

### U11x — open-file lifecycle: SYS_CLOSE + generation-tagged file-ids (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U11 M1 (`714daad`) — closes the U9x-tracked revoke+reopen aliasing gap. Before
  this, a per-task FILES descriptor was named to ring 3 by a bare `+1`-biased slot index; when a File revoke
  (`sys_cap_revoke`) freed a slot and a later `sys_open` first-fit-reused it, a lingering sibling handle carrying
  the old index would silently re-bind to the DIFFERENT file now living in that slot (the `FILE_USED` guard lapsed
  the moment the slot went live again). Fix = the standard slotmap/generation-index: a per-slot `FILE_GEN` counter
  bumped LAST on every free (`files_free` — the path SYS_CLOSE + the File-revoke drop + `sys_open`'s unwind route
  through — and `clear_files_row` at teardown); File handle words now pack `(gen << 32) | (idx + 1)`; and
  `file_desc_validate` is the SINGLE seam (range + `FILE_USED` + gen) every File consumer (`sys_read`/
  `sys_write_file`/`sys_seek`/`sys_close`, and the revoke File-drop) funnels through — a stale handle to a reused
  slot is `-EACCES` (a gen mismatch), never a rebind. Gen 0 packs to exactly `idx + 1`, byte-identical to the
  pre-U11x file-id, so every prior scaffold/kernel check that reads the value word is unaffected.
- **SYS_CLOSE (17):** frees the caller's descriptor slot (bumping its gen) + clears the handle word. Requires NO
  right (`handle_resolve(row, handle, 0)`); a non-File kind is `-EINVAL` (left intact), an unresolvable /
  already-closed / stale-slot handle is `-EBADF` (a double-close returns cleanly; a use-after-close is denied). New
  errno `EBADF = -9`. **x86 divergence from pi4:** `files_free` DISCARDS un-flushed dirty bytes (the staged
  write-back is a whole-task-teardown event, `clear_files_row`), so an explicit close of a dirty RW descriptor
  drops the write exactly as a revoke does — the demo closes RO handles; a future arc can make close enqueue the
  flush.
- **The x86 gap is revoke+reopen, not unlink:** x86 has no U10 (create/unlink), so — unlike pi4's fixture, which
  creates + unlinks — the aliasing vector here is a File revoke (which frees the descriptor) then a reopen reusing
  the slot. The gap is NOT ring-3-reachable at U9x (no way to hold a stale file-id across a free), so the ring-3
  fixture (`u11x-close`) proves SYS_CLOSE semantics (open+read → close → double-close `-EBADF` → use-after-close
  `-EACCES` → reopen+read; a 5-bit witness) over the immutable staged SCRATCH.BIN, while `u11x_check_gen_rebind`
  (kernel-side, scratch row 6) is the airtight no-rebind proof: claim a slot + mint its `(gen, idx)`; free it (gen
  bumps); re-claim the SAME slot (first-fit) at the bumped gen; prove the OLD file-id is rejected (gen mismatch)
  EVEN THOUGH the slot is live again, while a FRESH file-id resolves. Chained off `u9x_launcher` in program order;
  storage-gated (chain-inherited) but needs no disk itself (reads the static seed + pure descriptor bookkeeping),
  so it PASSes identically in FAT and non-FAT block-device modes.
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → `:: U11x: open-file lifecycle —
  open+read/close/double-close(-EBADF)/use-after-close(-EACCES)/reopen+read OK, gen-tagged file-id rejects a stale
  sibling to a reused slot -> PASS ::`, with **all 17 prior U1a→U9x verdicts byte-identical**. Default no-FAT
  `./arroyo test 30` stays MISSION SUCCESS and U11x PASSes (block device present, no FAT needed). `./arroyo check`
  both arches green, **zero aarch64 files touched** (`arch/x86_64/syscall.rs` only). Every File value word backing
  a real descriptor is gen-tagged (`sys_open`, the U6bx no-cap plant, the U9x revoke-check plant); the two opaque
  scaffold ids (`0x100`/`0x200`) are untouched and resolve identically.
- **Metal:** storage-gated like the rest of the chain (chained off `u9x_launcher`), so the xHCI enumeration
  blocker (`task_47291f90`) keeps it off metal — QEMU-green is the ceiling. The gen-tag mechanism itself is
  always-on in the syscall path; only the demo is gated.
- **Commit:** `hw-rmbp`, Opus-executed.

### No-storage capability-chain visibility — inline console-cap demos (U5x/U7x/U8x) run on the metal path + `UNAOS_NOSTORAGE` knob 🔬 `hw-rmbp`
- **What:** the direct enabler for the FTDI metal bench. Every U-arc demo gated on `block::info().is_none()` and
  SKIPPED with no block device, so on the metal rMBP (where the SD reader never enumerates over xHCI —
  `task_47291f90` — leaving `block::info()` None) the whole capability chain was invisible. But U5x/U7x/U8x are
  **inline console-cap blobs needing NO storage** (no `SYS_OPEN`, no `staged_bytes`, no disk — they transfer/revoke
  `Console` caps and `sys_write` streams to serial); their gate was control-path DISCIPLINE (a clean no-storage
  boot log), not a functional dependency. With the FTDI cable making the no-storage metal path observable, the gate
  is **deliberately relaxed for those three demos only** (probe + launcher/run sites), surfacing the U5→U8 slice
  (capabilities → cross-process transfer → revocation trees) over the metal console without waiting for the xHCI
  storage fix.
- **Scope + safety:** surgical — the storage-GATED arcs (U2 load/U4x/U6x/U6bx/U9x/U11x, which genuinely need FAT /
  HELLO.BIN) KEEP their gates and still skip; NO protection touched (SMEP/NXE/W^X/page-perms are orthogonal — this
  is a demo-visibility gate); the relaxed demos are the already-reviewed U5x/U7x/U8x fixtures on an additional path,
  not new surface; relative demo order is preserved by the existing `*_LAUNCH_DONE` bounded waits, not the block
  gate. New builder knob **`UNAOS_NOSTORAGE=1`** omits the QEMU `usb-storage` device (block absent) — the QEMU
  analog of the metal no-storage path, and a preview of what the FTDI console replays.
- **Tested — QEMU:** `UNAOS_NOSTORAGE=1 ./arroyo test 90` → `note='no mass-storage device enumerated'` (block
  absent) with U5x/U7x/U8x **PASS**, every storage-gated arc absent (cleanly skipped), and U1a/U1b/U2-0/U3/U3.5
  still PASS. **No regression block-present** (the relaxation is a no-op there): `./arroyo test 30` stays MISSION
  SUCCESS with the applicable chain PASS; `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` runs U1a→U11x with **0 FAIL**.
  `./arroyo check` both arches green, **zero aarch64 files touched** (`arch/x86_64/syscall.rs` + `builder/src/main.rs`).
- **Metal:** this is the metal ENABLER — the U5→U8 console-cap slice becomes metal-confirmable over the FTDI cable
  at the Peter-attended bench; the storage-gated arcs still wait on `task_47291f90`.
- **Commit:** `hw-rmbp`, Opus-executed.

### U10x — file GROWTH + CREATE + DELETE: cluster allocation, FAT-chain extension, directory mutation, all DEFERRED out of the IF-masked handler (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U10 (M1 grow / M2 create / M3 delete), giving U9x's in-place File writer an
  ALLOCATOR — arch-symmetric with pi4 at the capability layer, diverging ONLY where the hardware forces it.
  **The FAT allocator is reused VERBATIM** from the shared `fs/fat.rs` (pi4 landed it: `write_grow`,
  `create_in_root`, `find_located`, `mark_dir_deleted`, `free_chain`, `delete_located`, `first_free_cluster`) —
  the port adds ZERO FS logic, only a read-only `cluster_size()` accessor (rmbp's shared-kernel-core lane) for the
  launcher's cluster-size-aware chain check. **THE load-bearing divergence (the same wall U9x M2 hit):** pi4 does
  grow/create/delete disk I/O straight in the SVC handler (EMMC2 PIO); x86 CANNOT — the SYSCALL handler runs
  IF-masked and the xHCI BOT pump `hlt()`s, so no disk I/O in-handler. So every U10 mutation is **DEFERRED** to the
  demo launcher's IF=1 drain (a SEPARATE U10 op-queue, leaving the metal-confirmed U9x in-place write-back path
  untouched). (1) **GROW** (`SYS_WRITE` past EOF on a growable descriptor) — the extend stages in the descriptor's
  one-page `wstage` buffer in-handler (`sys_write_grow`, size-before-offset, `FILE_SIZE == WSTAGE_LEN <= PAGE`
  invariant); teardown enqueues a `Grow` op the launcher drains via `fat::write_grow`. A per-descriptor
  `FILE_OPNAME` marks it growable (set at open ONLY for the staged GROW.BIN — SCRATCH.BIN stays in-place-only,
  U9x byte-for-byte) and the deferred op ALWAYS names the descriptor's OWN file (no confused-deputy: a RW
  SCRATCH.BIN holder can never mint a GROW.BIN op). (2) **CREATE** (`SYS_OPEN` `O_CREAT` of a name absent from the
  staged set) — a dynamic in-memory created-file model (`sys_open_dynamic`/`open_create_new`/`open_created_sibling`,
  the idempotent 2nd open + the sibling handle) backed by an empty `wstage`; teardown enqueues a `CreateGrow` op
  (`create_in_root`-if-absent + `write_grow`-from-empty). (3) **DELETE** (`SYS_UNLINK = 16`) — gated by the SAME
  single `CAP_WRITE` CHECK as write; a scaffold guard refuses an immutable STAGED file (HELLO.BIN is EL0 code —
  only a `FILE_CREATED` descriptor is unlinkable); marks the name gone for the row (`DYN_DELETED` → a plain
  re-open is `-ENOENT`), invalidates EVERY descriptor of the file (each `files_free` bumps the slot gen, so a
  stale sibling handle is `-EACCES` — the U11x gen-tag), and enqueues a `CreateGrowDelete` op the launcher replays
  (create + grow allocating a real cluster, a mid-op EXISTENCE witness, then `delete_located`).
- **Honest scope + faithful divergences (design + code adversarially reviewed):** create/grow/delete disk I/O all
  DEFER to the launcher — so the x86 DELETE is a launcher-side REPLAY of the fixture's create+grow+unlink (a weaker
  causal exercise than pi4's in-handler unlink of an independently-persisted file), and its bit3 (sibling read
  `-EACCES`) proves gen-invalidation, NOT a freed-chain aliasing fail-safe (physically impossible pre-drain on
  x86). The delete proof is made NON-VACUOUS by the drain's mid-op existence witness + `count == 1` + `drained`
  gating (a no-op drain cannot pass). Grow is one-page-bounded (invisible to the 16-byte demo). The FAT-safety
  invariants ride the shared `fat.rs` verbatim (all `num_fats` copies; zero-fill before chaining; dir `size` last;
  `0xE5` before free). Enqueue and drain are gated on the SAME "FAT present" signal so no op strands and no false
  in-memory PASS can mask a present volume (a code-review follow-up). Deferred, unchanged: subdirs/LFN/rename;
  IF-safe interrupt-driven x86 storage (retires the deferral); UnaFS `owner`/`grants:*` on open/create/unlink.
- **Tested — QEMU:** three fixtures (`u10x-grow` witness `0x1F`, `u10cx-create` `0xF`, `u10dx-delete` `0x1F`),
  each register-only apart from the read-back dest, chained `u9x → u10x → u10cx → u10dx → u11x` (every launcher
  chains the next on ALL paths, so a skip never strands U11x). `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` (FAT32,
  512-B clusters) → on-disk grow (528B, 2-cluster chain, appended + original intact + all FAT copies consistent) /
  create (FRESH.BIN present, size 16, exactly one dir entry) / delete (DELME.BIN gone + chain freed all copies +
  cluster re-allocatable) all **PASS**; **20 PASS / 0 FAIL**, image cross-checked (GROW.BIN 528, FRESH.BIN
  count=1, DELME.BIN count=0). `test-fat p16` (FAT16 fixed-root, 2048-B clusters — the cluster-size-aware grow
  proof: 528B stays in one cluster, chain len 1) also **20 PASS / 0 FAIL**. Default no-FAT `./arroyo test 40`
  stays MISSION SUCCESS with all three U10 demos + U11x **PASS** in the in-memory core (deferred flush a no-op, no
  stranded op). `UNAOS_NOSTORAGE=1 ./arroyo test 90` skips the storage-gated arcs cleanly (console-cap slice
  U5x/U7x/U8x still PASS). `./arroyo check` both arches green, **zero aarch64 files touched** (`arch/x86_64/
  syscall.rs` + the read-only `fat.rs` accessor + the `make-fat-img.sh` GROW.BIN plant). Metal re-run self-heal
  (delete+recreate a prior boot's grown/created/deleted file via pub `fat.rs` primitives) keeps re-runs honest on
  a persistent card. **Capability chain U4→U10 now COMPLETE + arch-symmetric with pi4; unblocks U11x M2.**
- **Metal:** the storage-gated chain runs on real hardware only past `task_47291f90` — now closed by evidence at
  the 2026-07-08 bench (U1→U11x metal-confirmed off a FAT16 SD card), so a future attended bench can metal-confirm
  U10 by re-flashing; the self-heal makes it idempotent across reboots.
- **Commits:** `hw-rmbp` — M1 `6a54a76`, M2 `39ae5c5`, M3 `4471d34`, review fix `91c93b8`; Opus-executed.

### U11x M2 — cross-process open refcounts + unlink-defers-free: the deferred DELETE fires at the LAST close across processes (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U11 M2/M2b (`b88d2ba`/`303e271`) — closes the U10-ledgered cross-process gap: a
  file open in ANOTHER process when unlinked no longer has its delete fire immediately; the deferred
  `CreateGrowDelete` op is enqueued **HELD** and released by the LAST descriptor release across all rows —
  explicit `SYS_CLOSE` or whole-task TEARDOWN (`free_user_space_by_cr3` → `clear_files_row`, the pi4 M2b
  exit-without-close orphan). The launcher's IF=1 drain plays the pi4 reaper (x86 needs no kernel task — the
  release is one atomic store, legal at IF=0).
- **Shape (x86-idiomatic, simpler than pi4 by construction):** the created-file identity space is the static
  `U10_NAMES` table, so the global refcount table is indexed DIRECTLY by name-id (`OPENF_REFS`/`OPENF_PENDING`/
  `OPENF_HELDSLOT`, pure atomics — no lock, no row allocation) instead of pi4's `SpinMutex` table keyed by the
  recyclable `(dir_lba, dir_off)`; and because an O_CREAT re-create of a delete-pending name is REFUSED with the
  new **`-EBUSY`** (until the delete drains — or, no-FAT, until the last-close release), the pi4 recycled-slot-key
  aliasing class (`b863304`) cannot exist here. The per-row `DYN_DELETED` overlay became GLOBAL (`DYN_DELETED_G`):
  unlink hides the name from EVERY row in-handler (plain re-open `-ENOENT` anywhere), cleared exactly when the
  delete completes. New capability: `sys_open` resolves a live created file in ANY private row
  (`created_desc_any_row`) — a cross-process sibling open snapshot-copies the source wstage (incref before
  `install_file_handle`, so its EAGAIN unwind through `files_free` pairs the decref exactly once — every
  release path funnels through the ONE `openf_decref` seam).
- **Proof (`u11m2_launcher`, chained after `u11x_launcher`; TWO phases over one EL0 fixture):** the launcher
  plays "process S" on a REAL allocated scratch slot (held across the phase so the fixture's allocator can never
  claim it): product-path `open_create_new` of DEFER.BIN + pattern write; the EL0 `u11m2-unlink` fixture (own
  slot) then proves witness `0x3F` — cross-row plain-RW open, read-back of the OTHER process's bytes, unlink → 0,
  invalidated-sibling read `-EACCES`, re-open `-ENOENT`, re-create `-EBUSY`. C1 (the DEFER, after fixture
  teardown): op still HELD + refcount 1 + pending + S's descriptor still reads the ORIGINAL bytes
  (cross-process read-after-unlink). Release phase 1 = the SYS_CLOSE core (`files_free`+`handle_clear`);
  phase 2 = the REAL teardown funnel (`free_user_space_by_cr3`). C2: released (drainable / refcount 0 / pending
  consumed). Drain: exactly ONE op, on-disk gone + chain freed in all FAT copies + cluster re-allocatable +
  name re-creatable again. In-memory (no-FAT) mode runs the identical witness + C1/C2 minus disk checks.
- **Review:** adversarial pre-code design review (1 reviewer, 5 confirmed must-fixes folded before coding:
  teardown `CreateGrow` suppressed for delete-pending names — else queue overflow + on-disk resurrection;
  deleted-flag check FIRST in the open path — else the any-row scan un-deletes a pending file; incref pinned
  before handle-install — else the EAGAIN unwind underflows the count and strands the name for the boot;
  scratch row from the real allocator — a hardcoded row a fixture could land on would leak the launcher's
  handle into ring 3; no-FAT release clears the deleted flag — else permanent `-EBUSY`). One code-level find at
  gate time (the wstage pool needed 3 concurrent buffers), fixed as `NWSTAGE = 3`.
- **Gate:** `./arroyo check` both arches (zero aarch64 files); `./arroyo test 40` MISSION SUCCESS, 17 PASS 0
  FAIL (U11m2 in-memory core PASS, all priors unchanged); `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` **21 PASS
  0 FAIL** (U11m2 on-disk PASS appended, 20 priors); `UNAOS_FATIMG=p16 ./arroyo test-fat p16 300` 21 PASS 0
  FAIL (FAT16); `UNAOS_NOSTORAGE=1 ./arroyo test 90` clean skip (storage-gated, chain-inherited).
- **Honest divergences (ledgered in SECURITY.md):** the on-disk half is still the U10 launcher-side REPLAY;
  re-create-while-pending is `-EBUSY` (pi4 allows an immediate re-create as a new file); the cross-row sibling
  open is a SNAPSHOT copy (a concurrent writer in the source row could torn-copy, and a sibling that writes and
  exits would enqueue its own op against `NU10 == 1` — demo-sequenced, product fix = the pi4 lock + per-file
  backing); `SYS_CLOSE` still discards un-flushed dirty bytes (unchanged; now explicitly ledgered).
- **Metal:** storage-gated like the chain; the xHCI fix (`3bee9d6`) is metal-confirmed, so a future attended
  bench can metal-confirm U11m2 directly off the FAT card (self-healing pre-flight keeps re-runs honest).

---
## hw-jetson track — 2026-07-10 (code arc + same-day attended bench)

### JD2 — interactive shell on the Orin panel: keyboard → console → shell over the inherited scanout ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What:** the Orin's first interactive session — join JD1 (pixels) and JB10 (armed USB keyboard) with
  pure software routing, no new hardware touched. `tegra_early_stop` seeds `video::WRITER` with the JD1
  scanout; the JB2b `jb2-kbd` spawn becomes `jd2-console` (`main.rs::jd2_console_pump`, cooperative EL1
  task alongside CAPSTONE, `poll_events`-only/JC3 semantics): the boot log holds the panel until the
  **first keystroke**, then `fbcon::detach()` + a double-buffered `video::Screen` over the scanout and
  every key feeds the shared `handle_key` → `shell::dispatch_command` (keystrokes also echo on serial).
  Headless boots delegate to the JB2b `kbd_pump_body` unchanged. All `cfg(tegra)` — shared
  renderer/console/shell called, never edited.
- **Gate:** `UNAOS_TEGRA=1 ./arroyo check` both arches + `./arroyo test` + `test-arm` green (tegra off in
  QEMU → non-tegra byte-identical by construction). ⚠ `esp-jetson` `kernel.elf` = **378,728 B / 105
  `tegra:` strings** — the console/shell/FAT/font machinery is now linked in (+128 KB vs JD1), past the
  old ~355 KB heuristic; validate tegra media by the `tegra:`-string count, not size.
- **Metal (attended, 2026-07-10, same day):** keyboard ARMED direct-root port 6 slot 4 → pump live at
  EL1 → CAPSTONE → first key flipped the panel to the console (`console OWNS the panel`), `help` ⏎
  dispatched with every key echoed on serial and the output painted on the panel ("it works!"); the
  pump even survived a `gneiss` (vug) dispatch — keys kept flowing, no panic. The first interactive
  UnaOS session on the Orin. This session also flashed the SD card itself (the old EPERM was
  session-specific). Serial: `~/unaos-bench/jetson-serial-2026-07-10-090000.log`.
- **Detail:** [`arch_arm64.md` §JD2](dev/OS/01_BOOT_HAL/arch_arm64.md).

---
## hw-jetson track — 2026-07-08 (code arc — QEMU-green + adversarial-review-clean; USB-behaviour metal-pending)

### JD1 — first pixels: inherit the firmware's live scanout framebuffer ✅ METAL-CONFIRMED (Orin panel, 2026-07-08) `hw-jetson`
- **What:** get the boot log + CAPSTONE onto the Orin panel. JM7 found the panel dark because the UEFI
  GOP is `BltOnly` (no linear framebuffer) — but the firmware's DCE is still scanning out a DRAM carveout.
  **The finding (edk2-nvidia source):** the GOP is `BltOnly` *on purpose* — the default `SocDisplayHandoff`
  is SIMPLEFB, which hands the framebuffer off through the **device tree** instead: a `simple-framebuffer`
  node (geometry) → `memory-region` reserved-memory `reg` (the **physical** scanout base, with
  `iommu-addresses` declaring IOVA==PA). JD1 **inherits** that (the JB6→JB9 "inherit, don't re-init"
  pattern): a pure DTB walk — no display MMIO, no SMMU translation, no double-buffer hazard, no
  EL3-fatal-touch risk. `fdt_tegra::nvdisplay_simplefb` resolves it (+ `jd1_dump` diagnostic twin),
  `display_tegra::jd1_survey` decodes format/geometry + prints the `JD1 — scanout:` verdict,
  `mmu_tegra::map_fb_region` maps the carveout Normal-WB into **both** the EL2 `L1` and the EL1 twin (so
  it survives the JM6 drop), then `jd1_test_pattern` + `fbcon::init` bring the panel up. A read-only
  nvdisplay register sweep (`display_tegra::jd1_dc_survey`, `const JD1_DC_PROBE=false`) is the documented
  bench fallback for the case where the FDT we received carries no handoff node.
- **Register facts** (for the fallback), cross-checked against mainline `drm/tegra` `dc.h`/`hub.c` via a
  4-source research pass + adversarial verify (HIGH trust; the only bench-confirm number is the `0x10000`
  per-head stride): DC block `display@13800000`, per-window aperture `head+0x2800+0xC00·i`,
  `START_ADDR`/`_HI` at `+0x700`/`+0x734` (dword offsets ≪2), stride via `PLANAR_STORAGE`(×64), bit39 = a
  swizzle flag to mask.
- **Verified by construction + a 3-lens adversarial review** (MMU-correctness · off-tegra-neutrality ·
  DTB/scanout-safety, refuter-verified). QEMU never compiles `tegra`, so this is inert in every regression;
  the shared renderer (`video/framebuffer.rs`/`fbcon.rs`/`screen.rs`) is **unchanged** — JD1 only feeds it
  an address + geometry.
- **Tested:** `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test` (x86) + `./arroyo test-arm`
  (aarch64 virt) byte-green (all JD1 code `cfg(feature="tegra")` / inside `tegra_early_stop` → non-tegra
  byte-identical); `esp-jetson` `kernel.elf` **250,416 B / 101 `tegra:` strings** (up from JB10's 241,936 B
  / 90 — the JD1 survey/map/blit + linger code; RED LINE ~355 KB).
- **✅ METAL (2026-07-08, Peter at the Orin):** the firmware published the SIMPLEFB handoff into our FDT
  (`simple-fb /chosen/framebuffer 1920x1200 x8r8g8b8` → `framebuffer@0x279e00000` `0x960000` →
  `scanout base=0x279e00000 (Bgr) sane=true` → `panel LIVE`). On the panel: the colour-bar test pattern
  rendered **pixel-correct** (blue 2nd / red 5th → `Bgr` decode right; clean bars → stride right; framed +
  full-screen → base/geometry right), then fbcon painted the whole boot log + `CAPSTONE COMPLETE` across the
  EL2→EL1 drop. UnaOS's first correct frame on the Orin. `JD1_DC_PROBE` fallback never needed. A 3 s
  `JD1_TEST_PATTERN_HOLD_SECS` (`CNTPCT` busy-wait) keeps the pattern legible before the console takes over.
- **Detail:** [`arch_arm64.md` §JD1](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: **JD2** — route the inherited
  USB keyboard to a live shell on the panel (first interactive UnaOS session on the Orin).

### JB10 — nested-hub descent + FS Evaluate-Context + root-kbd readiness + inherit-path housekeeping ✅ QEMU-green + review-clean / 🔬 metal-pending `hw-jetson`
- **What:** the four JB9-baton follow-ups. **(1) Nested-hub descent** (shared `xhci/mod.rs`, additive,
  hub-FSM is dead code under QEMU): `enumerate_downstream` detects a downstream hub (class `0x09`) and
  pushes it to `hubs_pending` so `service_hubs` descends another tier; `DeviceSlot` gains
  `route_string`/`route_depth`, `bring_up_hub` accumulates the Route String per tier (`| port <<
  (4·depth)`, 5-tier cap), and `address_downstream` programs the DW2 Transaction Translator for LS/FS
  children (HS/SS keep DW2=0 → working VIA-hub path byte-unchanged). **(2) FS EP0 Evaluate-Context**
  (`#[cfg(feature="tegra")]`, `JB10_FS_EVAL_CTX`, HYPOTHESIS): the JB9 baton's "needs a port reset" is
  refuted by the serial — the retry already resets + re-addresses at MPS0=64 and the FS device still
  goes silent, so the tear-down churn itself is the culprit. Adopts Linux `xhci_check_maxpacket`: read
  8 bytes, patch EP0 MPS0 in place via Evaluate Context (TRB 13, EP0 from the *output* context — the
  review caught the source-offset bug), read the full descriptor, no teardown; deferred to
  `service_enum` (`fs-mps-learn` stage), fallback to babble→recover. **(3) Root-keyboard demo:** no
  code — the path already arms a root HID end to end; item 2 helps a FS root keyboard. **(4)
  Housekeeping:** `JB9_PROBE` default-off (diagnostic suite only — recipe gates on `JB9G_NO_HCRST` /
  `JB5_PROBE`, untouched), two compile-time asserts making the FW-destroying levers un-co-enable-able
  with the inherit recipe, JB4 block wrapped in `!jb9h_skip`; forensic kit KEPT (flip back at a bench).
- **Verified by construction + a 5-lens adversarial review (1 CONFIRMED bug fixed pre-commit: the
  Evaluate-Context EP0 source offset).** QEMU cannot exercise items 1–3 (no `usb-hub`, lenient MPS) —
  they land as levers for the next attended bench; item 4 needs no bench.
- **Tested:** `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test` (x86) + `./arroyo
  test-arm` (aarch64 virt) byte-green (root storage/kbd/mouse enumerate — items 1–2 are dead-under-QEMU
  or `cfg(tegra)`, non-tegra byte-identical); `esp-jetson` `kernel.elf` **241,936 B / 90 `tegra:`
  strings** (the JB9_PROBE-off shrink from ~257 KB / ~100+ is intended, NOT a virt clobber; JB10 code
  present: `HUB-BEHIND-HUB`, `tegra fs-mps`).
- **Detail:** [`arch_arm64.md` §JB10](dev/OS/01_BOOT_HAL/arch_arm64.md). Next (attended bench): flash +
  watch nested descent (`storage_slot` past 0), the FS `fs-mps-learn` lever, and a direct-root
  `keyboard ARMED`; then a scoped arc to retire the dead JB3/JB4/JB5 chain code.

---
## hw-jetson track — 2026-07-08 (attended bench, Peter at the Orin)

### JB9 (bench outcome) — ⭐ USB WORKS ON ORIN: inherit + no-HCRST + 64-byte contexts ✅ metal-attended `hw-jetson`
- **The six-arc XUSB mystery is closed, verdict "other":** the fabric was never broken — the JB9f
  inherit-run probe (bare RS=1 on UEFI's halted state, no reset) posted a PSC event into UEFI's own
  >4 GiB event ring in 200 ms. Three real bugs, fixed live at the bench: (1) HCRST kills the
  inherited Falcon's service loop → `JB9G_NO_HCRST` halt-only takeover; (2) the JB3 fabric chain
  mutated a working config → `JB9H_SKIP_CHAIN` (the MC-override-reads-0x0 "torn-down link" that
  founded arc B is what the WORKING config looks like); (3) the Tegra xHC has `HCCPARAMS1.CSZ=1`
  (64-byte contexts) vs the driver's hard-coded 32-byte stride → ADDRESS_DEVICE code-17; fixed
  shared-driver-wide (`context::CTX_WORDS`, Peter-approved), + SuperSpeed+ PSI-5 MPS0 mapping +
  FS EP0 babble-learn retry + DISABLE_SLOT takeover eviction. **Result: both halves of a VIA hub
  fully enumerate on Orin silicon** (descriptors + hub bring-up over real DMA). Also proven: the
  ARU mailbox never answers NS even pre-EBS (JB9d loader bracket); a true PG cycle can't reload the
  FW (ROM has no bare IFR autoboot — ⚠ the cycle DESTROYS the only FW instance); AO IFRDMA regs are
  NS-locked. Next arc: nested-hub descent (devices sat behind hub layers, storage_slot still 0),
  port-7 FS retry reset, direct-port `keyboard ARMED` demo.
- **Detail:** [`arch_arm64.md` §JB9 bench verdict](dev/OS/01_BOOT_HAL/arch_arm64.md).

### JB9 (as-shipped) — FW-liveness without CPUCTL + DMA-path forensics 🔬→✅ see verdict above `hw-jetson`
- **What:** the two kernel-side probes the JB8 verdict demands. **A** (`jb9_fw_alive`, at
  raw-handoff / post-xhc-restart / post-enum-attempt): a CPUCTL-free liveness witness — fw-header
  identity via the ARU ioctl (+checksum/created-time), an ARU scratch heartbeat (two sweeps ~10 ms
  apart), and MSG_ENABLED with a patient 5-attempt/~100 ms retry ladder (vs JB3's one ~200 µs try)
  — one `FW-ALIVE`/`FW-SILENT` verdict line each. **B** (fired at t≈200 ms and t≈5 s inside the
  `jb2b_attach` pump window, i.e. WHILE enable-slot is pending): SMR/S2CR/context-bank dump for
  SID 0xe with an "is TTBR0 our identity table?" verdict (stale UEFI translation = the prime
  IOVA≠PA suspect), MC HOSTR/HOSTW + error log at that instant, the FW-side SID view (ARU
  STREAMID_FIELD + the AO IFR-autoboot trio, base DTB-resolved from padctl reg region 1), and a
  ±2 MiB near-target RAM scan for command-completion TRBs that landed at a wrong PA.
- **Tested:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `esp-jetson`
  links (kernel.elf 269,480 B, 137 `tegra:` strings); `test-arm` green (`storage_slot=1`, zero
  panics — all JB9 code `tegra`-feature + `JB9_PROBE`-gated, QEMU byte-inert).
- **Detail:** [`arch_arm64.md` §JB9](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: the attended bench
  fills the §JB9 verdict — which of {FW idled, SMMU stale context, SID mismatch, other}.

### JB8 — ⭐ METAL VERDICT: the Falcon was NEVER halted — CPUCTL is a CSB-locked register; the real failure is the DMA path ✅ metal-attended (USB reader) `hw-jetson`
- **The decisive read (boot 5, pre-EBS, driver live):** xHC running (`HCH=0 CNR=0`), USB enumeration in
  progress — and `CPUCTL`/`BOOTVEC` still read `0xffffffff` while the ARU fw-header ioctl answers. The
  Falcon is **alive and CSB-locked** (signed FW, raised priv level). Every JB3→JB7 "halted/reset-held"
  verdict read a locked register; JB7's "non-secure wall" dissolves — there was never a stopped core.
- **The real failure:** at kernel time port resets complete, 3 ports link-train to U0 (`CCS=1 PED=1`),
  but `enable-slot` times out — command/event-ring **DMA never touches DRAM, zero faults** (arc B's
  question reopened, now against a live engine). Plus: UEFI never programs FPCI CFG BARs (DT-fixed
  addressing only); auto-boots never connect XhciControllerDxe; generic edk2 XhciDxe's un-gated
  `XhcHaltHC` is the (benign) EBS actor; `DisconnectController` fails `INVALID_PARAMETER` (open).
- **Next:** kernel-side CPUCTL-free FW-liveness probe (mailbox retry + scratch heartbeat) + DMA-path
  forensics (SMMU S2CR/CB binding at write time, stale UEFI SMMU context, MC vs FW StreamID).
- **Detail:** [`arch_arm64.md` §JB8](dev/OS/01_BOOT_HAL/arch_arm64.md), bench verdict subsection.

### JB8 (as-shipped) — pre-EBS Falcon witness + reconnect lever; IFR-autoboot discovery 🔬→✅ see verdict above `hw-jetson`
- **What:** an edk2-nvidia source read (r36.4.0-updates) shows **UEFI never halts the Falcon core** (only
  BPMP PG/reset asserts at EBS — *both* teardown layers carry the JB6 ACPI skip) and **T234 starts it via
  IFR DMA autoboot** (three AO-aperture writes: `IFRDMA_CFG0/1` = fw-buffer PA, `STREAMID` = 0xE; buffer is
  `EfiRuntimeServicesData`, survives EBS) — plain NS MMIO, **no secure world** — so JB7's "NS-unstartable"
  verdict rests on a lever nobody has pulled. JB8 ships the discriminating loader-side probe:
  `jb8_falcon_witness` reads FPCI CFG + CSB `CPUCTL`/`BOOTVEC`/fw-header + `USBSTS.CNR` **pre-ExitBootServices**
  (is the Falcon already dead before handoff?), and a separate `jb8lever` risk media
  (`UNAOS_JB8_LEVER=1`) forces `Disconnect`+`ConnectController` on the `Usb2Hc` handles — a fresh
  `XhciControllerDxe.Start` (PG cycle + IFR load) right before handoff, with a post-reconnect re-read.
- **Tested:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `esp-jetson` links with
  and without the lever feature (kernel.elf 254,536 B, 120 `tegra:` strings); `test-arm` green
  (`storage_slot=1`, zero panics — loader-side, tegra234-DTB-gated, QEMU inert). Metal: pending the
  attended bench (witness media first, lever media only if the witness reads dead).
- **Detail:** [`arch_arm64.md` §JB8](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: bench; then the kernel-side
  IFR restart if the lever proves the path.

### JB7 — arc B refuted, arc A closed at the non-secure wall (Falcon core reset-held, unstartable from NS) ✅ metal-attended (native microSD) `hw-jetson`
- **What:** an offline read of the JB6 run-F serial **refutes arc B** (the "XUSB StreamID mismatch"): the
  MC override **sticks** at `0xe` (`rb=0x0000000e/0x0000000e`; the "reads `0x0`" was the pre-fix
  first-touch), the SMMU stream matches + identity-translates, and the fault census is **clean**
  (`sGFSR=0x0`, CB0 FSR unchanged, `MC INTSTATUS=0x0`, before *and* after attach). Zero faults ⇒ no XUSB
  DMA is even attempted — the empty event ring is **arc A's shadow** (a halted Falcon = a halted xHC
  command engine issues no completion), not a DMA-delivery failure. S2CR bypass (the baton's fix) was
  moreover already refuted on metal (boots 5/6). Added a read-only clock-census probe
  (`bpmp_tegra::jb7_clocks_query`, MRQ_CLK `IS_ENABLED`) to characterise the halt (`feature="tegra"` +
  `JB5_PROBE`-gated → QEMU byte-identical). A BAR2-vs-CFG CSB cross-read (`jb7_csb_cfg_read`) was added
  then **removed** — the metal boot proved the CFG aperture EL3-fatal (below).
- **Metal — ✅ attended (Orin, native microSD, 2026-07-08), arc A CLOSED at the non-secure wall:**
  (1) the alternate FPCI/CFG CSB aperture is **EL3-fatal** — first touch trapped to BL31 (`Unhandled
  Exception in EL3`, `esr_el3=0xbe000011`, EC 0x2F SError) and killed the boot; unrouted post-EBS, probe
  removed. (2) **Boot-medium bisect refuted** — first Falcon-witness boot off the **native microSD slot**
  (not a USB reader) still read `CPUCTL=0xffffffff`; the halt is universal, not the USB-boot path.
  (3) **Clock census** — core clock 269 **ON**, leaf 270/271 gated but not the lever (JB4 enabled them on
  metal; CPUCTL stayed dead) ⇒ core is **reset-held**, not clock-gated. Clean through **CAPSTONE 6/6**.
  With no `resets` on `usb@3610000`, MRQ_RESET can't reach the Falcon and the only BPMP reset (MRQ_PG
  cycle) is retired — so a **non-secure kernel cannot start the halted Falcon** (as JB5 was for revival).
  Next XUSB swing is bootloader/UEFI-side (suppress `XhciControllerDxe.Start` → inherit MB2's live Falcon)
  or a pivot off USB.
- **Tested — QEMU (the DONE gate):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` both arches green
  (probes compile clean, zero new warnings); `UNAOS_TEGRA=1 ./arroyo esp-jetson` links (`kernel.elf`
  254,536 B tegra, 120 `tegra:` strings); virt `test-arm` green (`storage_slot=1`, byte-identical — all
  JB7 code gated off).
- **Detail:** [`arch_arm64.md` §JB7](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: the attended arc-A bench.

### JB6 — "inherit, don't revive" the XUSB Falcon: a bootloader dummy-ACPI table makes UEFI skip its ExitBootServices teardown ✅ teardown-skip metal-proven / 🔬 enum → arcs A+B `hw-jetson`
- **What:** the bootloader installs a minimal, spec-correct dummy **ACPI 2.0 RSDP+XSDT** into the UEFI
  config table immediately before `ExitBootServices` (`install_tegra_acpi_shim`, gated on a `tegra234`
  DTB sniff so QEMU `esp-arm` stays byte-identical). NVIDIA's `XhciControllerDxe.OnExitBootServices`
  self-skips its XUSB teardown when an ACPI table is present, so UnaOS **inherits a powered,
  FPCI-configured XUSB block** instead of a torn-down dead one. The run-F kernel path is
  non-destructive (`JB5_RUN_E_REPLAY=false` retires the domain-power-cycle replay that would kill an
  inherited-live block; read-only witnesses + a `jb6_csb_sweep` diagnostic ship active behind
  `JB5_PROBE`). Preceded by **JB5**, which — 5 probe boots + a full edk2-nvidia source read —
  **refuted** non-secure Falcon revival (MB2 one-shot IFR self-boot; UEFI power-gate vote-refcounted).
- **Metal — ✅ teardown-skip PROVEN (Peter-attended, Orin, 2026-07-08):** A/B at raw handoff,
  no-shim → shim: XUSB power gates `0x0`→**`0x1`**, FPCI `busmaster=0`→**`1`**, BARs
  unprogrammed→**programmed**. Boot ran clean through **CAPSTONE 6/6**. **Not yet enumerating** — two
  root-caused next arcs: **(A)** the inherited Falcon *core* is halted (`CPUCTL=0xffffffff` read via the
  NVIDIA-correct T234 CSB path — the core is in reset / clock-gated behind a live ARU; needs a
  reset-deassert), **(B)** an XUSB MC StreamID mismatch (DMA tagged SID 0 vs the SMMU stream opened for
  SID 0xe → event-ring writes never reach DRAM).
- **Tested — QEMU (the DONE gate):** `./arroyo check` both arches green; `UNAOS_TEGRA=1 ./arroyo
  esp-jetson` links (`kernel.elf` 246 KB, healthy); virt `test-arm` green (`storage_slot=1`) with the
  shim correctly a **no-op** on the QEMU virt DTB (non-tegra path byte-identical).
- **Detail:** [`arch_arm64.md` §JB5+JB6](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: arcs **A** (start the
  halted Falcon core) + **B** (MC StreamID) per the A+B baton.

---

## hw-pi4 track — 2026-07-07 (Opus round, post-Campaign 2)

### U9 — real File writes + seek: EMMC2 sector write, in-place `fat::write_at`, `SYS_SEEK`, File+`CAP_WRITE`-routed `sys_write` (aarch64) ✅ `hw-pi4`
- **What:** gives U6b's read-only `File`+`CAP_READ` its WRITE half — `CAP_WRITE` on a `File` now
  means something. Bottom-up: (1) **EMMC2 sector write** — `emmc2::write_block_512`, the exact mirror
  of the polled CMD17 read path with three deltas (`WRITE_SINGLE_BLOCK`/`cmd(24)` host→card so
  `CMD_DAT_DIR_READ` is omitted; the Buffer-**Write**-Ready bit `INT_WRITE_RDY = 1<<4`, distinct from
  the read path's bit 5; a FIFO that PUSHES 128 LE words, short buffers zero-padded to a full sector);
  same `send_command`, same bounded CNTPCT deadlines. (2) **Block seam** — `block::write_block` routes
  the SD backend to it (previously refused), inside the existing `#[cfg(aarch64, baremetal)]` arm so the
  x86 xHCI write path is byte-identical. (3) **`fat::write_at`** — the write twin of `read_at`: walks
  the file's existing chain, skips to `start`, read-modify-writes only the touched data sectors.
  BOUNDED BY CONSTRUCTION — clamped to `min(size, start+len)` so it **never grows** a file (write
  at/after EOF = 0-byte no-op), visits only in-chain clusters so it **never allocates or writes a FAT
  entry**, and **never touches a directory** (on-disk size + chain head unchanged). (4) **`SYS_SEEK = 15`**
  `(handle, offset)` — absolute seek; CHECK requires a `File` with ANY of `CAP_READ|CAP_WRITE`, past-size
  ⇒ `-EINVAL` (seeking TO `size` is legal), sets the U6b `FILE_OFFSET` (now settable). (5) **Routed
  writes** — `SYS_OPEN` gains a mode bit in `a2` (`0`=RO/`CAP_READ`, `1`=RW/`CAP_READ|CAP_WRITE`);
  `sys_write` is kind-dispatched at its single `handle_resolve(asid, fd, CAP_WRITE)` CHECK — `Console`
  streams to serial (byte-identical), `File`+`CAP_WRITE` overwrites in place at the descriptor offset via
  `sys_write_file`→`fat::write_at` (the write twin of `sys_read`'s clamp/validate/offset discipline).
  Because the U7/U8 derivation-revocation walk lives INSIDE `handle_resolve`, a revoked File-write cap is
  `-EACCES` at the write with **no new code**.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U8 PASS: the U9 setup line and
  `:: U9: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES,
  on-disk sector changed + size unchanged -> PASS ::`. The `el0-u9write` fixture (register-only, single
  slot, witness `0x1F`) opens a DEDICATED scratch file (`SCRATCH.BIN`, 1 KiB of `0xEE` planted by the
  launcher's FAT image — never `HELLO.BIN`) RW, seeks to a partial-sector offset, overwrites a 16-byte
  sentinel in place, seeks back and reads it through the SAME cap, and proves the RO-write + wrong-kind
  `-EACCES` denials; the launcher folds the kernel-side proofs — a fresh-mount raw re-read shows the
  sector CHANGED (to the sentinel, differing from the pre-image) while the directory size did NOT, and a
  scratch-ASID setup proves a U8-revoked File-write cap `-EACCES` (revoking a granted ancestor makes the
  derived `CAP_WRITE` resolve fail — exactly `sys_write`'s only gate). **Every prior verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `e8db35c` — only the U9 lines + the
  binary-growth VBAR shift `0xad000`→`0xad800` differ), 14 prior PASS + U9 = 15, CAPSTONE 6/6, 0
  unexpected faults; `./arroyo test-arm 22` byte-identical (the driver is baremetal-gated); `./arroyo
  check` both arches; **zero x86 files**.
- **Honest scope:** IN-PLACE OVERWRITE ONLY. Deferred (each needs an allocation-policy pass): file
  growth / cluster allocation / create / delete / truncate; directory mutation; write-back caching
  (every write is a synchronous RMW to the card); UnaFS `owner`/`grants:*` on `SYS_OPEN` (the RW mode
  bit is the local precursor; the ACL check rides the kernel UnaFS mount, K2/K3). **Lane:** the seat
  widened the pi4 lane to the shared `fat.rs`/`block.rs` + the FAT image builder for this arc after the
  executor stopped and reported; `fat::write_at` is purely additive (x86 never calls it), the block seam
  + image plant are cfg-/pi-scoped.
- **Metal:** ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-07, same session).** Booted the self-contained
  UNAOS card; the EMMC2 driver identified the real card `@0xfe340000 15193 MiB CSD v2`, mounted its FAT32,
  and the `el0-u9write` fixture opened `SCRATCH.BIN` RW, overwrote it in place with a polled **CMD24**, read
  it back through the same cap, and the launcher's fresh-mount raw re-read confirmed the sector CHANGED while
  the size did NOT — `:: U9: … on-disk sector changed + size unchanged -> PASS ::`, with **15/15 PASS** (all
  prior + U9), **CAPSTONE 6/6** (all 4 cores online), and only the 3 expected M6b EL0 kills. **This is the
  first write the EMMC2 driver has ever issued to a card — the metal-risk write path (buffer-write-ready +
  FIFO push + transfer-complete/DAT0-programming-busy) is now proven on silicon**, the one thing QEMU's
  generic-sdhci could not vouch for.
- **Commit:** on `hw-pi4` (Opus-executed, post-Campaign 2; metal-confirmed same session).

## hw-rmbp track — 2026-07-07 (U8x — the round after Campaign 2, Opus-executed)

### U8x — revocation trees + generation-tagged inboxes: revoke chases derived capabilities; the sys_xfer TOCTOU closes (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U8 on top of U7x, restoring FULL arch parity on the capability chain
  (U4→U8 now symmetric). Closes both escapes U7x's entry documents, slot-keyed throughout. (1) A bounded
  static **derivation ledger** (`MAX_DERIV = 16` nodes: state-exact CAS over a `0`/`RESERVING`/unique-id
  word, Release-publish/Acquire-read — the U7x discipline, no heap) records an edge child→parent at every
  derive: `sys_cap_grant` (local mint) and `SYS_XFER`/`SYS_RECV` (delivered transfer; the node rides the
  inbox slot + the sender-owned record). Revoke = mark ONE node; `handle_resolve` walks child→root
  (bounded, cycle-free by construction) and denies if ANY ancestor is revoked — **no revoke path ever
  writes another row** (U7x's stale-at-use pattern, generalized). `CAP_REVOKE` gets its real semantics:
  `SYS_CAP` REVOKE of a handle CARRYING it kills the whole derivation subtree (re-grants + re-transfers,
  however deep, exactly once, idempotently — a second revoke is `-ECHILD`); without it the drop stays
  local (U5x semantics, unchanged). XREVOKE marks the transfer's node too (id-guarded against reclaim
  ABA), so a re-transferred-onward cap dies with the root transfer — the U7x escape, closed. Node
  lifetime: frees on handle-drop when childless, else a **tombstone** until the subtree drains (cascading,
  CAS-arbitrated free). (2) **Generation-tagged inboxes:** a per-slot generation word (`SLOT_GEN` — the
  x86 twin of pi4's `ASID_GEN`) bumps at teardown before the sweep; deposits stamp it, RECV delivers only
  on an exact match, the sender post-check re-reads it — recipient-exit + slot-recycle + new-tenant-consume
  inside the deposit window can no longer deliver to the wrong tenant, from either side. x86 divergences:
  slot keying (`current_slot()`, `Proc.slot`); the SHARED_ROW stays refused as an endpoint; and the
  fixture conveys its witness via its name-routed `sys_exit` status (x86 has no SYS_REPORT).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 260` → after the U7x PASS line: the U8x setup
  line, both fixture prints (`u8x: write via the grandchild cap`, `u8x: right-less revoke stays local`),
  and `:: U8x: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged inbox,
  ledger clean -> PASS ::` (14/14 PASS). The `u8x-tree` fixture (register-only, single slot) proves
  witness `0xF`: grant→re-grant chain works pre-revoke; parent revoke (with `CAP_REVOKE`) → 0 and its
  double revoke → exactly `-ECHILD`; BOTH descendants `-EACCES` at next use; a right-less revoke stays
  local (the derived copy still writes) + its own double-revoke errno. `u8_kernel_check` drives the REAL
  `sys_xfer_from`/`sys_recv_for` code paths over scratch rows 5/6/7 (all private, `< USER_SLOTS`, none the
  refused SHARED_ROW): an S→R1→R2 re-transfer chain, root XREVOKE ⇒ R1 **and** R2 stale + laundering
  refused (non-vacuous — the re-transferred cap carries `CAP_GRANT`); a deposit stamped before the
  teardown bump is never delivered to the recycled row (record freed; a late XREVOKE honestly `-ENOENT`);
  afterwards rows/inboxes/records/derivation ledger all provably clear. **Every prior U1a→U7x verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `e8db35c` — pure append of the U8x lines; the
  U7x setup *banner* is a best-effort-console drop under peak 3-AP contention, not a regression — its PASS
  verdict, on the same branch-free path, lands byte-identical); default no-FAT `./arroyo test` stays
  MISSION SUCCESS (U8x needs no FAT file, so — like U7x — it runs and PASSes there too); `./arroyo check`
  both arches; **zero aarch64 files touched**.
- **Honest scope:** revocation + generations only. Carried from the aarch64 U8 review: `deriv_drop`
  retries the free-CAS past a racing revoke (a lost-CAS return would leak the node + tombstoned ancestors,
  exhausting the ledger under SMP); the laundering witness is non-vacuous. Deferred: the bandy Ring-3
  delegation wrapper, arbitrary recipients, `File` payload migration, real Socket syscalls, UnaFS
  `grants:*` on open, IF-safe x86 storage (retires the U6bx staged-source divergence).
  **Closed in the same merge window (deriv_drop SeqCst fence):** distinct from the carried
  revoke-vs-free race above, the `deriv_drop` tombstone/free *handshake* was a store-buffering (Dekker)
  race — a concurrent parent-vs-child drop of a cross-process chain could have both sides decline the
  free and leak a ledger node (fail-closed → eventual `-EAGAIN`; **no** UAF/escalation, the free is
  CAS-arbitrated). The identical Release/Acquire orderings shipped in the aarch64 U8 twin, so the fix
  promoted the four handshake ops (DROPPED store + KIDS load on the parent side; KIDS fetch_sub +
  DROPPED load on the child side) to `SeqCst` on **both** arches together — one total order forbids the
  double-stale outcome, the both-free case stays arbitrated by the `DERIV_ID` CAS. Surfaced by the U8x
  pre-merge concurrency lens.
- **Metal:** 🔬 metal-pending. The rMBP boot came up (kernel + USB enumeration fine) but **storage
  never enumerated** — an orthogonal x86 xHCI blocker (the SD-reader `058f:6362` enumerates
  unconfigured behind a hub; a hot-plug `ADDRESS_DEVICE` failed with a USB transaction error across 3
  resets), handed to the USB/xHCI track. So U8x's storage-gated demo **skipped rather than failing** —
  unexercised on metal, not a pass/fail. QEMU-green on `test` + `test-fat`; rides the next rMBP
  boundary once the xHCI enumeration path is fixed.
- **Commit:** on `hw-rmbp` (the round after Campaign 2, Opus-executed).

## hw-rmbp track — 2026-07-07 (Campaign 2, Fable-executed)

### U7x — cross-process capability transfer: inbox-mediated `SYS_XFER`/`SYS_RECV` + sender revoke, single-writer preserved (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U7, restoring full arch parity on the capability chain — kernel-mediated
  delegation that preserves the single-writer-per-row invariant by construction. `SYS_XFER = 13` (dest =
  a `Child` handle in the SENDER's own table — owner-scoped; src must carry `CAP_GRANT`;
  `req & !src_rights != 0 -> -EACCES`) deposits the attenuated descriptor into the recipient's per-SLOT
  **inbox** (`NXFER = 4`; state word `0`/`RESERVING`/unique-tx; every claim/consume/retract/teardown a
  tx-exact CAS); `SYS_RECV = 14` installs it into the CALLER's own row. `SYS_CAP` XREVOKE (op 2) flips
  `XFER_REVOKED_BIT` inside the **sender-owned record's TX word** (`MAX_XFERS = 8`; tx-exact — the pi4
  review-fixed shape, inherited); the received cap goes stale at its next `handle_resolve` via the
  recipient-written `HANDLE_XFER_REC` sidecar; pending-revoked discarded at RECV; post-revoke
  grant/re-transfer laundering refused. pid→row rides a new `Proc.slot` (+1-biased — the `Proc.asid`
  substitution); post-check + retract closes the deposit-vs-exit race; teardown sweeps inbox + records
  and DISOWNS the dying sender's transfers. Payloads: `Console`/`Socket` only. x86 divergences: slot
  keying throughout, and the **SHARED_ROW refused as a transfer endpoint** (`-EACCES` both directions).
  **Folds both U6bx review notes:** the `FILE_OFFSET` advance is now a tx-exact CAS range-claim
  (well-defined for racing SHARED_ROW readers), and `SYS_CAP` REVOKE of a `File` handle frees its FILES
  descriptor (no more open→revoke `-EMFILE` exhaustion).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 180` → after the U6bx PASS line: the U7x setup
  line, `u7x: child prints via the transferred cap` (a REAL console write authorized by a transferred
  capability), and `:: U7x: cross-process transfer — SYS_XFER attenuated, child received + used the cap,
  revoke enforced, single-writer intact -> PASS ::` (14/14 PASS). The launcher-orchestrated script
  proves: over-rights transfer refused; the **single-writer witness** (the child's handle row byte-clear
  while the t1 deposit sits in its inbox, the child provably parked pre-RECV); use-then-revoke ordering;
  the revoked cap `-EACCES`; teardown leaves rows/inboxes/records fully clear. Sequencing divergence
  from pi4: no x86 yield syscall, so each fixture runs on its OWN dedicated AP (3 APs required; fewer
  skips cleanly) with bounded-spin GO/RECV polls; witnesses ride name-routed exit statuses (no
  SYS_REPORT); the child's USE cue is a store to its own RW page. **Every prior U1a→U6bx verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `225ae48` — pure append `33a34,35`);
  default no-FAT `./arroyo test` stays MISSION SUCCESS (U7x needs no FAT file, so — like U5x — it runs
  and PASSes there too); `./arroyo check` both arches; **zero aarch64 files touched**.
- **Honest scope:** single-LEVEL revoke — re-granted/re-transferred copies escape it (revocation TREES =
  the pi4-led U8: derivation records + real `CAP_REVOKE`); one documented TOCTOU residue at the sys_xfer
  post-check (generation-tagged inboxes ride the tree arc); a GRANT-minted duplicate File handle shares
  its descriptor — revoking either frees it, the survivor fails CLOSED.
- **Metal:** none expected (pure syscall logic; rides the next rMBP boundary with the battery).
- **Commit:** on `hw-rmbp` (Campaign 2, Fable-executed; adversarial review panel before merge).

## hw-pi4 track — 2026-07-07 (Campaign 2, Fable-executed)

### U8 — revocation trees + generation-tagged inboxes: revoke chases derived capabilities; the sys_xfer TOCTOU closes (aarch64) 🔬 `hw-pi4`
- **What:** closes U7's two documented escapes. (1) A bounded static **derivation ledger**
  (`MAX_DERIV = 16` nodes: state-exact CAS transitions over a `0`/`RESERVING`/unique-id word,
  Release-publish/Acquire-read — the U7 discipline, no heap) records an edge child→parent at every
  derive: `sys_cap_grant` (local mint) and `SYS_XFER`/`SYS_RECV` (delivered transfer; the node rides
  the inbox slot and the sender-owned record). Revoke = mark ONE node; `handle_resolve` walks
  child→root (bounded, cycle-free by construction) and denies if ANY ancestor is revoked — **no revoke
  path ever writes another ASID's row** (U7's stale-at-use pattern, generalized). `CAP_REVOKE` gets its
  real semantics: `SYS_CAP` REVOKE of a handle CARRYING it kills the whole derivation subtree
  (re-grants + re-transfers, however deep, exactly once, idempotently); without it the drop stays local
  (U5 semantics, unchanged). XREVOKE marks the transfer's node too (id-guarded against reclaim ABA), so
  a re-transferred-onward cap dies with the root transfer — the U7 escape, closed. Node lifetime:
  frees on handle-drop when childless, else a **tombstone** until the subtree drains (cascading,
  CAS-arbitrated free). (2) **Generation-tagged inboxes:** a per-ASID generation word bumps at teardown
  (before the sweep); deposits stamp it, RECV delivers only on an exact match, the sender post-check
  re-reads it — recipient-exit + ASID-recycle + new-tenant-consume inside the deposit window can no
  longer deliver to the wrong tenant, from either side.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U7 PASS: the U8 setup line, both fixture
  prints, and `:: U8: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged
  inbox, ledger clean -> PASS ::`. The `el0-u8tree` fixture (register-only, single slot) proves witness
  `0xF`: grant→re-grant chain works pre-revoke; parent revoke (with `CAP_REVOKE`) → 0 and its double
  revoke → exactly `-ECHILD`; BOTH descendants `-EACCES` at next use; a right-less revoke stays local
  (the derived copy still writes) + its own double-revoke errno. `u8_kernel_check` drives the REAL
  `sys_xfer_from`/`sys_recv_for` code paths over scratch ASIDs: an S→R1→R2 re-transfer chain, root
  XREVOKE ⇒ R1 **and** R2 stale + laundering refused; a deposit stamped before the teardown bump is
  never delivered to the recycled ASID (record freed; a late XREVOKE honestly `-ENOENT`); afterwards
  rows/inboxes/records/derivation ledger all provably clear. **Every prior verdict byte-identical**
  (sorted scratch-worktree baseline diff vs `225ae48` — only the U8 lines + the binary-growth VBAR/ELR
  shift differ), 19/19 prior PASS + U8 = 20, CAPSTONE 6/6, 0 unexpected faults; `./arroyo test-arm 22`
  byte-identical; `./arroyo check` both arches; **zero x86 files**.
- **Honest scope:** revocation + generations only. Deferred: the bandy Ring-3 delegation wrapper,
  arbitrary recipients, `File` payload migration, real Socket syscalls, UnaFS `grants:*` on open. The
  U7 RCsc-codegen footnote stands (the new code keeps the same ordering discipline).
- **Metal:** none expected (pure syscall logic; rides the next Pi boundary with the battery).
- **Commit:** on `hw-pi4` (Campaign 2, Fable-executed; U8 review: 3 lenses, 1 must-fix fixed in-arc + re-gated).

## hw-pi4 track — 2026-07-07 (Campaign 1 — the round-13 sweep, Fable-executed)

### U7 — cross-process capability transfer: inbox-mediated `SYS_XFER`/`SYS_RECV` + sender revoke, single-writer preserved (aarch64) 🔬 `hw-pi4`
- **What:** the first CROSS-process op on the object table — kernel-mediated delegation that preserves
  the single-writer-per-row invariant by construction. `SYS_XFER = 13` (dest = a `Child` handle in the
  SENDER's own table — owner-scoped; src must carry `CAP_GRANT`; `req & !src_rights != 0 -> -EACCES`)
  deposits the attenuated descriptor into the recipient's per-ASID **inbox** (`NXFER = 4`; state word
  `0`/`RESERVING`/unique-tx; every claim/consume/retract/teardown a tx-exact CAS); `SYS_RECV = 14`
  installs it into the CALLER's own row. `SYS_CAP` XREVOKE (op 2) flips the **sender-owned record**
  (`MAX_XFERS = 8`); the received cap goes stale at its next `handle_resolve` via a recipient-written
  sidecar (`HANDLE_XFER_REC`) — nobody ever writes a foreign row, not even to revoke. pid→ASID rides a
  new `Proc.asid` field; a **post-check + retract** closes the deposit-vs-exit race sender-side;
  teardown sweeps inbox + records (recycled ASIDs inherit nothing). Payloads: `Console`/`Socket` only
  (`File` = sender-local descriptor, refused; `Child` = reap delegation, refused).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U6b PASS line: the U7 setup line,
  `u7: child prints via the transferred cap` (a REAL console write authorized by a transferred
  capability), and `:: U7: cross-process transfer — SYS_XFER attenuated, child received + used the cap,
  revoke enforced, single-writer intact -> PASS ::`. The launcher-orchestrated script (GO words +
  cooperative SYS_YIELD polling — deterministic under QEMU) proves: over-rights transfer refused;
  the **single-writer witness** (the child's handle row byte-clear while the t1 deposit sits in its
  inbox, the child provably parked pre-RECV); use-then-revoke ordering; the revoked cap `-EACCES`;
  teardown leaves rows/inboxes/records fully clear. **Every prior M6/U4/U5/U6/U6b verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `5ea4b48` — only the U7 lines + the
  binary-growth VBAR shift differ) + CAPSTONE 6/6 + 19/19 PASS + 0 faults; `./arroyo check` both
  arches; **zero x86 files**.
- **Honest scope:** single-LEVEL revoke — a re-granted or re-transferred copy escapes it (derived
  caps; the revocation-TREE arc adds derivation records); one documented TOCTOU residue at the
  sys_xfer post-check (exit + ASID-recycle + consume between two adjacent checks; generation-tagged
  inboxes ride the tree arc). Deferred: trees, the bandy Ring-3 wrapper, arbitrary recipients,
  `File` payload migration, real Socket syscalls.
- **Metal:** none expected (pure syscall logic; rides at the next Pi boundary with the battery).
- **Review:** five-lens adversarial panel + refuter verification (29 agents) — 2 distinct CONFIRMED
  must-fixes, **both fixed in-arc**: XREVOKE made tx-exact (the revoked flag moved into the record's TX
  word, bit 63 — the stale-revoke-on-reclaimed-record race and the born-revoked window are gone) and
  `u7_build` now scrubs the whole window (U6b's +0x3000 plant landed exactly on the GO word — the
  witness was racy). Notes closed: post-revoke laundering (`-EACCES` on delegation from a revoked
  received cap), sender-ASID-recycle revoke authority (teardown disowns records), `handle_row_is_clear`
  covers the transfer sidecar, planted-Proc truthfulness (EXITED on fixture exit/kill). Ledgered: the
  RCsc-codegen memory-model footnote. Re-gated green after the fixes (19/19 PASS, sorted-diff pure
  append).
- **Commit:** on `hw-pi4` (Campaign 1, Fable-executed; multi-lens adversarial review before merge).

## hw-rmbp track — 2026-07-07 (Campaign 1 — the round-13 sweep, Fable-executed)

### U6bx — real File handles: `SYS_OPEN`/`SYS_READ` through the object table, served from the BSP-staged source (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U6b — makes U6x's `File` **scaffold** real, bringing the arches to
  File-handle parity: `SYS_OPEN = 11` mints a `File` handle carrying `CAP_READ` (first-free, skipping the
  reserved `CONSOLE_FD`; per-task FILES descriptor sidecars keyed `[row][idx]`, file-id = descriptor
  index + 1 so the value word stays clear of the `0`/`u64::MAX` sentinels); `SYS_READ = 12` is the
  enforcement point — `handle_resolve(row, handle, CAP_READ)` must yield a `File`, and a missing right /
  wrong kind / absent handle **all** return `-EACCES` (the `sys_write` Console+`CAP_WRITE` twin). Whole-
  destination validated up front as writable window memory (`-EFAULT`, no offset advance); offset-exact
  sequential reads, `0` = EOF; teardown-clear extends `clear_handle_row` to the FILES row (exit + kill).
- **The honest x86 divergence (the U4x pattern):** pi4 reads the disk INSIDE the SVC handler (EMMC2 is
  PIO). x86 cannot — the xHCI BOT read pump `hlt()`s and the SYSCALL handler is IF-masked — so `SYS_OPEN`
  serves the **BSP-staged set** (pre-read at IF=1 over the proven U2 FAT path; HELLO.BIN today, the same
  `stage_hello` buffer `sys_spawn` uses) and `SYS_READ` serves the staged bytes. The capability layer is
  byte-for-byte the pi4 twin; only the source of the bytes differs. Arbitrary-runtime-file open awaits an
  IF-safe / interrupt-driven x86 storage arc (which retires the divergence entirely).
- **Tested — QEMU:** `./arroyo test-fat sf 180` → after the U6x PASS line: the U6bx setup line and
  `:: U6bx: x86 real File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind
  -EACCES -> PASS ::` (13/13 PASS lines, 0 fault lines). The `u6bx-file` fixture opens HELLO.BIN, reads
  16 bytes through the cap and verifies them against the kernel-planted staged prefix, then proves both
  denial arms (a real-descriptor File with ZERO rights; a Socket WITH `CAP_READ`) — witness `0x1F`; the
  launcher proves the FILES-row teardown-clear kernel-side. **U1a/U1b/U2/U2.5/U3/U3.5/U4x/U5x/U6x all
  PASS byte-identical** (stash-free scratch-worktree baseline diff vs `e74dcb9` — pure append: no-FAT
  `32a33`, test-fat `37a38,39`); default no-FAT `./arroyo test` stays MISSION SUCCESS with U6bx skipping
  cleanly. `./arroyo check` both arches; **0 aarch64 files touched** (main.rs additions cfg-gated x86).
- **Honest scope:** the pre-staged set only; read-only, no seek/write/dirs. Deferred: x86 U7 (the pi4 U7
  twin), IF-safe storage (the divergence-retiring arc), real Socket routing, PCID, copy_from_user.
- **Metal:** pending (pure syscall logic; the staged source rides U2's metal-confirmed FAT path).
- **Review:** five-lens adversarial panel (cap-check / unwind / bounds / concurrency / regression) +
  refuter verification — **0 must-fix**, 2 deferrable notes ledgered in `SECURITY.md` (the non-atomic
  `FILE_OFFSET` advance under concurrent same-SHARED_ROW readers; REVOKE orphaning a File descriptor —
  both shapes shared with the merged pi4 twin; both fold into U7/U7x).
- **Commit:** on `hw-rmbp` (Campaign 1, Fable-executed; multi-lens adversarial review before merge).

## hw-rmbp track — 2026-07-06 (merged round 12, `9cc0326`)

### U6x — the general object table: `(kind, target, rights)`, first-free for all kinds, `CONSOLE_FD` collision closed (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 U6a — generalizes U5x's fixed-shape handle into a general **object
  descriptor**, keyed by the address-space **slot**/`row` where aarch64 keys by ASID. The **kind** rides
  in a parallel sidecar `HANDLE_KIND[[AtomicU8; 8]; USER_SLOTS+1]` (keyed identically to
  `HANDLES`/`HANDLE_RIGHTS`), so the value word keeps U4x/U5x's sentinels **byte-identical** (`0`=Empty,
  `u64::MAX`=`RESERVING`) and nothing else is reserved — a `File(id)`/`Socket(id)` may carry any
  non-sentinel id with no high-bit masking (the STOP-tripwire sentinel collision is structurally
  impossible). `handle_resolve` dispatches `Child`/`Console`/`File`/`Socket` on the sidecar; `File`/`Socket`
  are resolvable **scaffolds** (no fs/net syscall routes through them yet).
- **`CONSOLE_FD` collision closed (the raison d'être + U5x's one design note):** `handle_install` (the
  first-free allocator) now **SKIPS** the reserved `CONSOLE_FD`, so a process that both prints (console cap
  at the fd=1/stdout index) AND spawns 2+ children (auto-allocated at `{0, 2, 3, ..}`) has **zero index
  collision for any interleaving** of installs — the console never clobbers, nor is clobbered by, an
  auto-allocated handle. The `fd=1` stdout ABI stays byte-identical for every existing blob. Every consumer
  is behaviour-preserved: attenuation unchanged, the mint copies the source's kind (never re-kinds) and
  publishes the value LAST; `handle_clear`/`clear_handle_row`/`handle_row_is_clear` also handle the kind.
- **Tested — QEMU:** `./arroyo test-fat part 30` (and `sf`) → after the U5x PASS line: the U6x setup line,
  `u6x: parent print (pre-spawn)` and `u6x: parent print (post-spawn; console survived 2 spawns)`, and
  `:: U6x: x86 general object table — printing spawner + 2 children, no index collision, File/Socket kinds
  resolve -> PASS ::`. `u6x-spawn` is the printing spawner U5x couldn't serve (prints → spawns 2 off the
  reserved index → prints again [console survived] → reaps both); `u6x_kernel_check` proves kernel-side that
  `File`/`Socket` kinds resolve with/without rights and that the exact U5x-breaking interleaving no longer
  collides. **U1a/U1b/U2/U2.5/U3/U3.5/U4x/U5x all PASS byte-identical** (proven by a stash-free scratch-
  worktree baseline diff — pure append of the U6x lines); the default no-FAT `./arroyo test` stays MISSION
  SUCCESS with U6x skipping cleanly (like U4x — its children need the staged program). `./arroyo check` both
  arches; **0 aarch64 files touched**.
- **Honest scope:** the OBJECT TABLE only. Deferred to U7 (the pi4 U7 twin): cross-process handle-transfer
  (`SYS_XFER`, breaks single-writer) + revocation trees (`CAP_REVOKE` still reserved) + the bandy Ring-3
  wrapper; real `File`/`Socket` fs/net routing through these kinds is a later arc. PCID and
  `copy_from_user`/`copy_to_user` stay separately deferred.
- **Metal:** pending (pure syscall logic; the child loads ride U2/U4x's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

### U5x — handles as capabilities: the CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 U5 — turns U4x's handle STRUCTURE into a real **capability**,
  keyed by the address-space **slot** where aarch64 keys by ASID. A handle now carries **rights**
  (`CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE`, in a sidecar `HANDLE_RIGHTS` array keyed
  identically to `HANDLES`, so U4x's `0`/`RESERVING` value-word sentinels stay byte-unperturbed) and
  names a **target** beyond "child pid" (a `HANDLE_CONSOLE = u64::MAX-1` token — two kinds,
  `Child(pid)`/`Console`, no general object table = U6). **The CHECK** is a single
  `handle_resolve(row, idx, req_rights)` at the one lookup point every handle-consuming path uses:
  out-of-range/Empty/`RESERVING` ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4x ownership
  preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒ `-EACCES`. **`SYS_CAP=10`** carries
  GRANT (mints an **attenuated** handle — `req & !src_rights != 0` ⇒ `-EACCES`, so a grant can never
  amplify; requires `CAP_GRANT` on the source) and REVOKE (ownership-based). **`sys_write` routes
  through the table** — `fd` is a handle that must resolve to `Console`+`CAP_WRITE`; no ambient stdout.
- **The x86 divergence (SLOT vs ASID):** the shared kernel window (U1a/U1b/U2 run with `user_cr3 == 0`,
  so `current_slot()` is `None`) has no private slot, so `HANDLES`/`HANDLE_RIGHTS` grow one extra row
  `SHARED_ROW` (index `USER_SLOTS` — the x86 twin of aarch64 ASID 0); `caller_row()` maps `None →
  SHARED_ROW`. The console cap is endowed there in `setup()` (covers U1a/U1b/U2) and per child in
  `sys_spawn`, so every prior print still lands. The fixture conveys its 4-bit witness as its `sys_exit`
  **status**, routed by task name (x86 needs no `SYS_REPORT`).
- **Teardown-clear** (folds U4x's one deferred note): `memory::free_user_space_by_cr3` wipes the slot's
  handle row (values + rights) **before** releasing the used-flag; both teardown paths (normal `exit` +
  the KillSwitch reap) funnel through it, so the clear rides both.
- **Tested — QEMU:** `./arroyo test-fat part 30` (and `sf`) → after the U4x PASS line: the U5x setup
  line, `u5x: cap write` twice (the write-cap + the minted cap reaching the console), and
  `:: U5x: x86 capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced,
  teardown-clear clean -> PASS ::`. **U1a/U1b/U2/U2.5/U3/U3.5/U4x all PASS byte-identical** (routing
  `sys_write` drops no print — every printing process holds the endowed cap); the default no-FAT
  `./arroyo test` stays MISSION SUCCESS with U2/U4x skipping cleanly (U5x, being an inline fixture,
  still runs + PASSes). `./arroyo check` both arches; **0 aarch64 files touched**.
- **Honest scope:** register-only + cooperative fixture; deferred to U6 (the pi4 U6 twin) — bandy
  handle-transfer, a general object table (fs/net kinds, first-free `Console`), cross-process revocation
  trees (`CAP_REVOKE` defined but reserved); PCID and `copy_from_user`/`copy_to_user` stay separately
  deferred; FP/SIMD-across-context-switch stays ledgered (U4x left it register-only).
- **Metal:** pending (pure syscall logic; the child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-rmbp track — 2026-07-05 (landed on `hw-rmbp`, awaiting integration)

### U4x — the process model + per-process handle table (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 M7/U4 — a parent loads a child program into its OWN
  address space, runs it ring-3, and reaps it by an owner-scoped **handle**. **Part 0 (the
  enabler):** the `TSS.RSP0` + per-CPU `syscall_kernel_rsp` install moved from the ring-3
  trampoline into the scheduler **DISPATCH** path (beside U3.5's CR3-at-dispatch), so a task
  RESUMED after a block (which never re-enters the trampoline) gets ITS OWN kernel stack —
  the prerequisite for >1 concurrent user task per core, closing a use-after-free where a
  resumed task's syscall/fault would otherwise land on a sibling's (possibly freed) kernel
  stack. **Part A:** `SYS_SPAWN=8` returns a small handle index into the caller's per-process
  handle table (`HANDLES`, keyed by the caller's address-space **slot** — the x86 stand-in
  for aarch64's ASID, read from the live CR3 via `current_slot`); `SYS_WAIT=9` blocks on the
  child's done-semaphore, returns its status, and **consumes** the handle. `PROCS` (pid-keyed)
  and `HANDLES` (slot-keyed) are separate, static, const-init — the reviewed aarch64 U4 design
  adopted directly. **Part B:** a parent spawns two children and reaps both by distinct
  handles; an orphan's `sys_wait(0)` returns `-ECHILD` (proving the tables are per-process).
- **Load path — an honest x86 divergence:** aarch64 reads `HELLO.BIN` inside the SVC handler
  (its EMMC2 driver is PIO). x86 storage is USB-over-xHCI, whose BOT read pump `hlt()`s to
  await completion — and `hlt` with IF=0 hangs, while the SYSCALL handler is IF-masked. So the
  FAT read is pre-staged off FAT ONCE on the BSP main loop (IF=1, the proven U2 path) and
  `sys_spawn` only memcpys the staged bytes into a fresh slot. Same observable behavior.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 30` (and `test-fat part`/`sf`) →
  `:: U4x: x86 process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  -> PASS ::`, with the two children each printing `hello from disk` (the 2nd/3rd in a full
  boot). **U1a/U1b/U2/U2.5/U3/U3.5 byte-identical** (proven by a pre-change baseline diff — the
  RSP0-at-dispatch move does not disturb the cooperative single-user-task path); U4x skips
  cleanly with no FAT volume (default / `UNAOS_USBSERIAL=1`). `./arroyo check` both arches; **0
  aarch64 files touched**. Two independent adversarial reviews (an 8-lens sweep + a 3-lens deep
  pass on the RSP0-at-dispatch use-after-free, the spawn/wait concurrency+leak+hang surface,
  and the blob's cross-syscall register ABI) each returned **0 findings**.
- **Honest scope:** register-only + cooperative fixtures (IF clear); `MAX_PROCS=4` + the static
  8-slot pool (STOP tripwires); no PCID; no `copy_from_user`/program-by-name; handle rows not
  cleared on slot teardown (harmless today — reapers consume handles; the capability CHECK +
  grant/attenuate/revoke + teardown-clear is U5). **FP/SIMD across a ring-3 context switch stays
  unsaved** (now reachable via Part 0's multi-task-per-core, not just U3.5 preemption) — the
  fixtures are register-only, so the gap is ledgered in SECURITY.md, not closed this arc.
- **Metal:** pending (rides the next reflash / FTDI cable day) — fully QEMU-verifiable (the reap
  wake is a scheduler post; child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-jetson track — 2026-07-05 (landed on `hw-jetson`, awaiting integration)

### JM6 — drop the Orin boot core EL2 → EL1 + run the scheduler/CAPSTONE at EL1 🔬 QEMU-green / ⛔ metal FAILED `hw-jetson`
- **What:** repeats the JC3 drop on the **Orin** (Tegra234, Cortex-A78AE) boot core — it drops
  **EL2 → EL1** and runs the full six-primitive M4 CAPSTONE cooperatively at EL1, the first time
  the scheduler runs on Orin silicon. A new `arch/aarch64/boot_tegra.rs` (the tegra analogue of
  `boot_virt`) arms the EL1&0 regime at **`mmu_tegra`'s already-built identity `L1`** (`MmuInfo::
  ttbr0`) with `SCTLR_EL1.M=1` *while still at EL2* — dormant until the `eret`, so EL1 never runs
  an instruction with its MMU off — then a naked-asm drop (mask DAIF, seed VMPIDR/VPIDR, FP-enable,
  `HCR_EL2.RW`, **disable CNTP**, `SPSR_EL2 = 0x3c5`, `eret` to `x30`). `main.rs::tegra_early_stop`
  gains the JM6 terminus after JM4 (`fbcon::detach` → `boot_tegra::drop_to_el1(mmu.ttbr0)` →
  `percpu::init(0)` → `exceptions::install()` → `sched::run_capstone_boot_core(0)`, never returns).
  Single-core by design: JM5's Orin SMP (`CPU_ON`) is **parked** (metal-blocked on an external
  Tegra BL31/MCE RAS fault) and deliberately not invoked here, so JM6 sidesteps that wall.
- **Tested — QEMU (the DONE gate; Orin is not emulated):** `./arroyo check` both arches +
  `UNAOS_TEGRA=1 ./arroyo check` both legs, no new warnings. Non-regression: virt
  (`UNAOS_GICV3=1 test-arm 45`) SMP 3/3 + JC3 drop + `VBAR_EL1 = 0x7c38c000` + CAPSTONE 6/6 —
  **byte-identical** to JC3 (same VBAR address ⇒ virt binary layout unshifted); Pi (`kernel8-test`)
  **sorted-diff 0**; x86 (`test`) MISSION SUCCESS; `esp-jetson` media links. All JM6 code is
  `tegra`-gated, so every non-tegra build's cfg set (and output) is unchanged.
- **Metal — ⛔ FAILED (Peter-attended, Orin, 2026-07-05, 5 boots):** the boot core **dark-hangs at the
  EL2→EL1 drop.** JM3/JM4 + the heap init all run on silicon (every line through `:: tegra: JM6 —
  dropping … ::` prints), then dark. Localized: the `eret` reaches EL1 (no `VBAR_EL2` illegal-return
  fault), but the **first EL1 instruction fetch aborts** — a `VBAR_EL1` fault vector *and* a raw-UARTC
  sentinel stub armed before the eret both stayed dark, so `.text` is unexecutable at EL1 the instant
  the drop lands. Monitor-independent; `SCTLR_EL1`-independent (the `mmu_tegra` RMW pattern didn't help).
  Needs a dedicated investigation (see `arch_arm64.md` §3 JM6 result for the plan), NOT blind reboots.
  Captures `target/serial-orin-jm6-FAIL{,2,3,4}-*.log`.
- **Honest scope:** the reused EL2-built `L1` is correct for a kernel-only (no-EL0) core but not
  EL1-precise (RAM reads EL0-accessible via AP[1]=1; the device window is nominally EL1-executable
  though no code branches there) — an EL1-precise map is worth building only once EL0 runs on Orin.
  EL0-on-Orin and EL1 timer preemption (needs EL1-non-banking vectors) are follow-on arcs.
- **Commit:** this arc on `hw-jetson` (merge pending the integrator, who records the merge hash).

---

## hw-rmbp track — 2026-07-04 (landed on `hw-rmbp`, awaiting integration)

### U3.5 — preemptible ring 3 (x86) ✅ `hw-rmbp`
- **What:** completes the U3 process abstraction — a ring-3 task can now be dropped
  **preemptible** (`Task.preemptible` sets `RFLAGS.IF` in the `iretq` frame), so the
  local-APIC timer evicts it and other work shares its core. This closes the one-core
  DoS a program that never syscalls (`jmp $`) was. The x86 twin of aarch64 M6e. The
  timer ISR conditionally `swapgs`es on a CPL-3 entry so the scheduler sees the kernel
  per-CPU block, and the per-process **CR3 install moved from the trampoline into the
  scheduler DISPATCH path** so it covers a resumed-after-preemption task (which never
  re-enters the trampoline) as well as first entry. The full user register file is
  saved/restored across preempt/resume by the existing `x86-interrupt` + `switch_context`
  machinery. The cooperative fixtures (U1a/U1b/U2/U2.5/U3) stay `preemptible=false` — IF
  clear, never preempted — so they are byte-identical.
- **Tested — QEMU:** `./arroyo test 25` → `:: U3.5: ring-3 preemption — IRQs-at-ring3=160,
  co-task ran, spinner resumed -> PASS ::` (a preemptible spinner is preempted 160×, a
  kernel co-task on the same core runs to completion = the DoS fix, the spinner's
  private-CR3 counter climbs across preemptions = correct resume, and a watchdog reaps it
  via a scheduler `KillSwitch`). U1a/U1b/U2/U2.5/U3 byte-identical (only the U1a shared-blob
  size and 2 new U3.5 lines differ); coexists with the U2 disk loader (`UNAOS_FATIMG=1` →
  `hello from disk`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT regression
  (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files. Multi-lens
  adversarial review before commit.
- **Honest scope:** per-task opt-in (only the spinner is preemptible); FP/SIMD is NOT saved
  across preemption yet (no FXSAVE/FXRSTOR — the register-only spinner is safe); one user
  task per core (RSP0/`syscall_kernel_rsp` set at first entry only); no PCID.
- **Metal — ★ CONFIRMED (real 2012 rMBP, 2026-07-04, bootlog photo):** `:: SMEP on ::` (real
  supervisor-execute protection active while the preemptible spinner ran) then `:: U3.5: ring-3
  preemption — IRQs-at-ring3=156, co-task ran, spinner resumed -> PASS ::` — the real timer preempted
  the ring-3 spinner 156× and the swapgs-on-ring3-timer + CR3-at-dispatch + reap ran correctly on Ivy
  Bridge, every prior fixture PASS (U1a/U1b/U2-0a/U3 byte-consistent), 0 unexpected faults. The same
  boot also confirmed the U2.5 APIC ms-clock fix on metal: `initcnt=6236 [1 kHz calibrated]`,
  `ms-clock 999 Hz` (the old ~119 Hz reading is gone).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

---

## hw-pi4 track — 2026-07-04 (landed on `hw-pi4`, awaiting integration)

### U6b — real File handles: `SYS_OPEN`/`SYS_READ` routed through the object table via `File` + `CAP_READ` (aarch64) ✅ `hw-pi4`
- **What:** makes U6a's `File` **scaffold** real — the first resource syscall routed through a **non-Console**
  object, and the direct precursor to UnaFS grants (a program opening a disk file under a capability).
  **`SYS_OPEN = 11`**`(name_ptr, name_len)` → `copy_from_user`s the bounded 8.3 name, mounts the single
  read-only FAT volume, finds the top-level entry, records a per-task **open-file descriptor** and installs a
  `File` handle carrying `CAP_READ`; **`SYS_READ = 12`**`(handle, buf, len)` → the CHECK
  `handle_resolve(asid, handle, CAP_READ)` must yield a `File` (a missing right, a non-File kind, or no handle
  all give `-EACCES` — the twin of `sys_write`'s Console+`CAP_WRITE`), then it clamps to `min(len, size-offset)`,
  validates the destination (`user_range_ok(.., writable=true)` — a bad buffer is `-EFAULT` with no read and no
  offset move), reads through a new read-only **offset-aware** FAT reader (`fat::read_at`), `copy_to_user`s, and
  advances the descriptor's offset by the count delivered (`0` = EOF; sequential, no seek). The descriptor lives
  in a small **per-task FILES table** — parallel atomic arrays (`FILE_USED`/`FILE_CLUSTER`/`FILE_SIZE`/
  `FILE_OFFSET`, keyed `[asid][idx]`, `NFILE = 4`), the same lock-free sidecar shape as `HANDLE_RIGHTS`/
  `HANDLE_KIND`; the `File` handle's value word carries the **file-id = descriptor index + 1** (the `+1` bias
  keeps it clear of the value word's `0`/`u64::MAX` sentinels, structurally). **Teardown-clear** extends U5's
  discipline to files: `clear_handle_row` now also clears the FILES row, so a reused ASID starts with no stale
  file, no leaked offset, no aliasable descriptor. **Scope by design:** read-only, flat root, one FAT volume, no
  write/create/delete, no seek, no directory ops, no second mount — `SYS_OPEN` is the hook a later arc's UnaFS
  `owner`/`grants:*` enforcement rides. Lane: `arch/aarch64/syscall.rs` (the FILES table, `SYS_OPEN`/`SYS_READ`,
  the teardown-clear extension, the demo) + a `main.rs` launcher + `fs/fat.rs` (a read-only `read_at` +
  `first_cluster()` getter; `read_file` left **byte-identical** for its M6g/U4 caller); no scheduler primitive,
  no `boot.rs` change (the teardown-clear folds into `clear_handle_row`), no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U6 PASS line: the U6b setup line and `:: U6b: real
  File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::`. The
  `el0-u6bfile` fixture opens `HELLO.BIN`, reads its first 16 bytes through the returned `File` capability and
  verifies they equal the kernel-planted on-disk prefix (`USER_BLOB[..16]` — `HELLO.BIN` on the media *is*
  `USER_BLOB`), then proves the CHECK denies both a **present File lacking `CAP_READ`** (the rights arm) and a
  **`Socket` carrying `CAP_READ`** (the kind arm, `SYS_READ` serves `File` only) with `-EACCES` — witness `0x1F`
  (all five bits). The launcher additionally proves the **file-row teardown-clear** kernel-side (the fixture
  exits holding two live descriptors — its own open + a pre-endowed no-cap File — so `files_row_is_clear`
  transitions false→true when its slot retires). Every M6b/M6d/M6e/M6f/M6g/U4/U5/U6 verdict line
  **byte-identical** (the shared FAT mount does not regress M6g/U4 — both still PASS their disk loads) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2 clean USB
  enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-06)** — Peter booted (non-destructive
  `kernel8.img` swap on the mounted FAT; `HELLO.BIN` was already byte-identical to this build, so the
  bytes-match test is exact), I ran the Debug-Probe serial bridge. On silicon: `:: U6b: real File handles —
  open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::` — the fixture opened
  `HELLO.BIN` and read it through a `File` capability off the **real EMMC2/SD card** (the metal-only EMMC2-first
  leg `SD card @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)`, which QEMU cannot exercise), and
  both `-EACCES` denials held. `EL=1`/`CNTFRQ=54 MHz`, EL0 preempt live (`M6e IRQs-taken-at-EL0=23`,
  `M6f spsentinel=2`), full battery green (M6b `exited=1 killed=3`, M6d ×3, M6f ×3, M6g/U4/U5/U6 PASS), 0
  unexpected faults. (The scheduler CAPSTONE demo sat out this particular boot — only 3 of 4 cores came online,
  and it needs the full 4; that is a known metal SMP AP-bring-up variance in the scheduler track, orthogonal to
  U6b's pure-syscall logic.) Metal log `unaos/target/serial-pi.u6b-metal.log`.
- **Deferred:** file **writes**/create/delete, **seek**/`lseek`, **directory** ops (the natural extensions once
  read-only File handles are proven); real **`Socket`** handles (net syscalls — the fs twin of this arc); UnaFS
  `owner`/`grants:*` checked on `SYS_OPEN` (rides the kernel UnaFS mount, K2/K3); a second mount / media detect.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U6a — the general object table: `(kind, target, rights)` descriptors, first-free for ALL kinds, the `CONSOLE_FD` collision closed (aarch64) ✅ `hw-pi4`
- **What:** generalizes U5's fixed-shape handle into a general **object descriptor**. A handle now
  names one of four **kinds** — `Child(pid)` (U4), `Console` (U5), and the **scaffolds** `File(id)` /
  `Socket(id)` (defined + resolvable via `handle_resolve`, but no fs/net syscall routes through them
  yet — they prove the table is genuinely general, not that fs/net exists). The **kind rides in a
  parallel sidecar** `HANDLE_KIND[[AtomicU8; 8]; USER_SLOTS+1]` (keyed identically to `HANDLES`/
  `HANDLE_RIGHTS`), so the value word keeps U4/U5's sentinels **byte-identical** — `0` = Empty (the
  lock-free allocator's free marker), `u64::MAX` = `RESERVING` — and nothing else is reserved. Picking
  the sidecar over the value word's high bits makes the sentinel-collision STOP tripwire *structurally
  impossible* (a `File`/`Socket` id may be any non-`0`/non-`u64::MAX` word, no masking) and mirrors the
  rights sidecar 1:1 (kind + rights published Release BEFORE the live value; single-writer-per-ASID the
  backstop). **The `CONSOLE_FD` collision is closed** (the arc's raison d'être + U5's one design note):
  U5 pinned the console cap at a fixed index via an unconditional store while `handle_install`'s
  first-free scan allocated from index 0, so a process that both PRINTED and SPAWNED 2+ children could
  auto-allocate a child onto index 1 and have it clobbered by the console install. U6 makes `CONSOLE_FD`
  a **reserved index the first-free allocator SKIPS**: the console lives there by the `fd=1`/stdout
  convention (keeping every prior blob byte-identical), children/objects fill `{0, 2, 3, ..}`, so a
  console cap and N child/object caps coexist with **zero index collision for any interleaving of
  installs**. Every consumer is behaviour-preserved: `handle_resolve` dispatches on the kind sidecar
  (`sys_wait`→`Child`, `sys_write`→`Console`+`CAP_WRITE`, `sys_cap` grant/revoke on any kind); the
  **attenuation invariant is unchanged** and the mint copies the source's kind (attenuate rights, never
  re-kind); `handle_clear`/`clear_handle_row`/`handle_row_is_clear` also handle the kind. Lane:
  `arch/aarch64/syscall.rs` (the descriptor, reserved-index alloc, resolve, kind scaffold, the demo) +
  a `main.rs` launcher; no `boot.rs` change (`clear_handle_row` already wipes the whole row), no
  scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U5 PASS line: the U6 setup line, `u6: parent
  print (pre-spawn)`, `u6: parent print (post-spawn; console survived 2 spawns)`, the two children's
  `hello from EL0`, and `:: U6: general object table — printing spawner + 2 children, no index
  collision, File/Socket kinds resolve -> PASS ::`. The `el0-u6spawn` fixture is the printing spawner U5
  could not serve: it prints, spawns 2 children (distinct auto-allocated handles, both `!= CONSOLE_FD`),
  prints AGAIN (the console cap survived the spawns), and reaps both by handle — witness `0xF`. The
  launcher additionally proves kernel-side that the `File`/`Socket` kinds resolve to their kind with the
  required right (and `Denied`/`-EACCES`-equivalent without) and that the exact U5-breaking
  console-vs-two-children interleaving no longer collides (`u6_kernel_check`). Every
  M6b/M6d/M6e/M6f/M6g/U4/U5 verdict line **byte-identical** (sorted set-diff: the only delta is
  `VBAR_EL1` shifting one page — benign binary growth from the added code — plus the new U6 lines) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2
  clean USB enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** none this arc — U6 is fully QEMU-verifiable (descriptor/allocator/resolve logic;
  the demo's two children ride U4/M7's already-metal-confirmed EMMC2 load path).
- **Deferred:** **U7** — cross-process handle-transfer (`SYS_XFER`, a cross-ASID write discipline that
  breaks the single-writer invariant) + revocation trees (`CAP_REVOKE`, reserved) + the bandy Ring-3
  delegation wrapper; and a later arc — real `File`/`Socket` fs/net syscalls routing through these
  kinds, plus UnaFS `owner`/`grants:*` enforcement on `SYS_OPEN` (rides the kernel UnaFS mount).
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U5 — handles as capabilities: the enforcement CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (aarch64) ✅ `hw-pi4`
- **What:** turns U4's handle STRUCTURE into a real **capability**. A handle now carries
  **rights** — a bitmask `CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE` in a **sidecar**
  `HANDLE_RIGHTS[[AtomicU32; 8]; USER_SLOTS+1]` keyed identically to `HANDLES`, so U4's
  `0`/`RESERVING` value-word sentinel logic stays byte-unperturbed — and names a **target**
  beyond "child pid" (a well-known `HANDLE_CONSOLE = u64::MAX-1` token; two kinds only,
  `CHILD(pid)` and `CONSOLE`, not a general object table — that is U6). The **CHECK** is a
  single `handle_resolve(asid, idx, req_rights)` at the one lookup point every handle-consuming
  path goes through: out-of-range/Empty ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4's
  structural ownership preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒
  `-EACCES`. **`SYS_CAP` (10)** adds grant/attenuate/revoke: GRANT mints a new handle to the
  same target carrying a rights mask that must be a **subset** of the granter's rights on the
  source — the **attenuation (monotonic-decrease) invariant**, `req & !src_rights != 0` ⇒
  `-EACCES` (a grant can never amplify), requiring `CAP_GRANT` on the source; REVOKE drops a
  handle the caller owns (subsequent use ⇒ `-EACCES`/`-ECHILD`). **`sys_write` routes through
  the table** — `fd` is a handle index that must resolve to a `CONSOLE` handle with
  `CAP_WRITE`; no ambient stdout. Every printing EL0 process is **endowed** a `CONSOLE`+
  `CAP_WRITE` cap at `CONSOLE_FD = 1` at spawn/launch (shared window ASID 0 for `el0-hello`;
  each M6f/M6g/U4-child slot), and the `copy_from_user`/all-or-nothing `-EFAULT` path is
  byte-identical (the M6f hostile fixture holds the cap, so its bad-pointer writes still
  `-EFAULT`, not `-EACCES`). **Teardown-clear** folds U4's one deferred note:
  `boot::teardown_user_slot` wipes the whole `HANDLES[asid]` row + rights **before** releasing
  the slot's used-flag (not after — a post-release clear could race a concurrent
  `alloc_user_slot` on another core reclaiming the ASID), so no capability outlives its ASID.
  Lane: `arch/aarch64/syscall.rs` + a `boot.rs` row-clear (in `teardown_user_slot`) + a
  `main.rs` launcher; no scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test 8` → after the U4 PASS line: the U5 setup line,
  `u5: cap write` **twice** (the write-cap write + the write through the minted attenuated
  cap), and `:: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded,
  revoke enforced, teardown-clear clean -> PASS ::`. The `el0-u5cap` fixture proves four
  EL0-observable behaviours against its own table (write-cap OK; a write-less cap → `-EACCES`;
  a grant exceeding the granter's rights rejected while a subset grant works and its handle
  writes; a revoked handle → `-EACCES`) via a witness bitmask (`0xF`), and the launcher proves
  the fifth kernel-side (the fixture's handle row is clear after its slot teardown). Every
  M6b/M6d/M6e/M6f/M6g/U4 line byte-identical (hex/pid-normalized set-diff: only the four new U5
  lines added; all four prior `hello from EL0` land, the M6f `4 hostile … EFAULT` PASS holds,
  U4 PASS holds, CAPSTONE 6/6) and 0 unexpected faults. `check` both arches; x86 `test` MISSION
  SUCCESS; aarch64 virt v2 MISSION SUCCESS + GICv3 JC3 SMP 3/3 + CAPSTONE 6/6 unchanged.
- **Tested — metal:** none this arc — U5 is fully QEMU-verifiable (the checks/grants/revokes
  are pure syscall logic; the reap wake is a scheduler post; the child disk-loads ride
  U4/M7's already-metal-confirmed EMMC2 path). A future reflash would re-exercise the endowed
  prints off the real card, but nothing in U5 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U4 — the process model + per-process handle table (aarch64) ✅ `hw-pi4`
- **What:** the ownership half of the process model — `sys_spawn` now returns a **handle**
  into the *caller's* per-process handle table (not a raw pid) and `sys_wait` takes that
  handle, so reaping is **structurally owner-scoped**: a task can only reap children whose
  handles are in ITS table (folding M7's review note — any task could `sys_wait` any pid —
  by construction). The table is a static, const-init `HANDLES[[AtomicU64; 8]; USER_SLOTS+1]`
  keyed by the caller's **ASID** (read from `TTBR0_EL1[63:48]` synchronously in the SVC
  handler); `PROCS` stays keyed by pid (exit-accounting control blocks) while `HANDLES` is
  keyed by ASID (the spawner's private namespace of child capabilities) — deliberately
  separate. No new syscall number, no new scheduler primitive, no driver, no boot change:
  the whole arc is `arch/aarch64/syscall.rs` + one `main.rs` launcher tweak. `sys_write`
  stays the `fd==1` path (routing a resource syscall through a handle is U5, when there is a
  capability *check* to add). This is the exact substrate U5 turns into capabilities (grant =
  transfer a handle, revoke = clear it; U5 adds the check at this same handle lookup).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → in place of the M7 line: the U4 setup line,
  **four** `hello from EL0` (M6c inline #1, M6g loader #2, the two U4 children #3/#4), and
  `:: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  (per-process handle tables) -> PASS ::`. The demo: a parent (`el0-u4parent`) `sys_spawn`s
  two children and reaps **both by handle**; an ownership negative (`el0-u4orphan`, its own
  slot/ASID) calls `sys_wait(0)` on an Empty handle and gets `-ECHILD`. Every
  M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical (hex/pid-normalized set-diff: only
  the M7 line → the U4 line, `hello` 3→4) and 0 unexpected faults. x86 (`test` +
  `UNAOS_FATIMG=1 test`) functionally byte-identical through the U-lines (seam is
  `baremetal`-only; sole diff is a QEMU timer/scheduling jitter on the async U2 exit at the
  25 s window boundary — reliably present with a longer window; U3/U3.5 untouched); `check`
  both arches; aarch64 virt v2 + GICv3 JM5 SMP 3/3 unchanged.
- **Tested — metal:** none this arc — U4 is fully QEMU-verifiable (every piece is
  scheduler/syscall logic: the handle install/resolve/clear, the owner-scoped reap, the
  two-child spawn/reap, and the `-ECHILD` negative are all deterministic under QEMU raspi4b —
  the reap wake is a scheduler post, not the timer). The child disk-loads ride the same EMMC2
  path M7 already metal-confirmed; a future reflash would show the two extra `hello from EL0`
  off the real card, but nothing in U4 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### M7 — a minimal process model: sys_spawn + sys_wait (aarch64) ✅ `hw-pi4`
- **What:** the reaping half of a process model — an EL0 program can now spawn a child
  program and reap it. **`SYS_SPAWN` (8)** loads the fixed on-disk `HELLO.BIN` into a
  fresh per-task slot and runs it at EL0 as a *child*, returning its pid; **`SYS_WAIT`
  (9)** blocks the caller until that child exits and returns its exit status. A small
  static process table (cap 4) carries, per child, a counting `Semaphore` the child
  posts once (on exit *or* kill) and the parent waits once — a **scheduler** wake, so
  the reap is QEMU-testable, not timer-gated. The child's disk load reuses the M6g
  loader core, refactored into a shared, silent `load_program_into_slot()` (the M6g
  loader reconstructs its own lines from the result — its output stays byte-identical).
  The pid-recording race is closed by a co-location invariant (child queued on the
  caller's core, undispatchable until the parent yields in `sys_wait`), so **no
  scheduler change** was needed. The Pi pioneers roadmap-U4, as it did M6a–M6g.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the M6g lines: `:: M7: process
  model — sys_spawn + sys_wait (parent reaps a disk-loaded child) ::`, a **third** `hello
  from EL0` (the M7 child), `:: M7: parent spawned child pid=<p>, waited, child exited
  status 0 -> PASS ::`, with every M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical
  (hex/pid-normalized set-diff: only the two new M7 marker lines + `hello` 2→3) and 0
  unexpected faults. x86 (`test` + `UNAOS_FATIMG=1 test`) functionally byte-identical
  through the U-lines (seam is `baremetal`-only; sole diffs are QEMU timer-calibration
  jitter); `check` both arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ PASS on silicon. `:: M7: parent spawned
  child pid=41, waited, child exited status 0 -> PASS ::` — the parent `sys_spawn`ed a child
  that loaded `HELLO.BIN` off the **real** card via the EMMC2-first path QEMU cannot exercise
  (`SD card @0xfe340000 — 31116288 blocks (15193 MiB, CSD v2)`), printed the **third** `hello
  from EL0`, and exited status 0; the parent's blocking `sys_wait` was woken by the child's
  scheduler post and reaped it — all under a live timer (EL0 preemption live: M6e
  `IRQs-taken-at-EL0=23`, M6f `spsentinel=3`). Full battery green on metal: M6b `exited=1
  killed=3 PASS`, M6d ×3, M6f ×3, M6g `disk-loaded EL0 program exited ok -> PASS`, CAPSTONE
  6/6, EL=1/CNTFRQ=54 MHz, **0 unexpected faults, 0 FAIL lines**. (Prepped non-destructively by
  swapping `kernel8.img` on the mounted FAT volume — no re-flash needed; the gate-verified
  binary.) Log: `target/serial-pi.m7-metal.log`.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

## Round 6 — 2026-07-04 (landed on track branches; awaiting integration)

### U3 — per-process address spaces (CR3) (x86) 🔬 `hw-rmbp`
- **What:** each ring-3 process now runs in its OWN top-level page table (its own
  CR3) instead of sharing one user window — the x86 mirror of aarch64 M6d. A static
  8-slot pool of page tables each SHARES the kernel half (copies every kernel PML4
  entry except the user-window slot, so the identity map / MMIO / heap / kernel
  stacks are shared) and owns a PRIVATE user window at USER_BASE. The scheduler
  installs a task's CR3 before dropping to ring 3 and restores the kernel CR3 +
  frees the slot on exit. Two processes can map the same address to different
  memory. Plain `mov cr3` (full TLB flush) — PCID (the x86 ASID analogue) deferred.
- **Tested — QEMU:** `./arroyo test 25` → a deterministic kernel isolation probe
  (two spaces, same VA, distinct sentinels, swap CR3 and read → distinct) PASS, and
  two ring-3 tasks each in their own CR3 each read their own private sentinel PASS;
  U1a/U1b/U2/U2.5 byte-identical, no reboot loop; coexists with the U2 disk loader
  (`UNAOS_FATIMG=1`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT
  regression (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files.
  4-lens adversarial review → 0 confirmed findings.
- **Metal — PENDING:** rides the next rMBP reflash (FTDI cable day, ~2026-07-08).
- **Commits:** on `hw-rmbp` (see landing report); unmerged (Fable credits out).

---

## Round 5 — 2026-07-03

### M6g — load a program from storage (aarch64) ✅ `hw-pi4`
- **What:** the Pi twin of x86 U2 — the first *program loaded from the microSD the
  Pi booted from* into the EL0 boundary. A block-layer backend seam lets the
  read-only path dispatch to a new BCM2711 EMMC2/SDHCI microSD driver (PIO,
  single-block CMD17, polled, no writes) beside the untouched xHCI path; the driver
  probes **EMMC2 first, legacy Arasan second** (the reverse of QEMU, so the metal
  base is the first tried). The loader mounts the card's FAT volume, reads
  `HELLO.BIN`, size-checks it, copies it into a fresh per-task M6d slot (EL0-RX/EL1-RO
  before the task exists), and runs it at EL0 (`hello from EL0`). The loaded bytes are
  untrusted — bounded only by size, contained by EL0 + per-page perms + the M6b
  fault-kill net.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → `SD card @0xfe300000 identified —
  131072 blocks (64 MiB, CSD v1)`, `FAT mounted from SD (Fat32)`, `HELLO.BIN loaded
  from SD (51 bytes) -> EL0`, second `hello from EL0`, `disk-loaded EL0 program exited
  ok -> PASS`, with every prior milestone byte-identical and 0 unexpected faults; the
  `UNAOS_SDIMG=0` no-SD control adds exactly the two no-card lines + the loader-skipped
  line. x86 (`test` + `UNAOS_FATIMG=1 test`) byte-identical (seam inert); `check` both
  arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ the driver's EMMC2-first success leg —
  the one QEMU physically cannot exercise — ran on silicon: **no fallback line**, `SD card
  @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)` (the real ~16 GB microSD,
  SDHC/block-addressed — vs QEMU's 64 MiB/CSD v1 legacy fallback), then `FAT mounted from
  SD (Fat32)`, `HELLO.BIN loaded from SD (51 bytes) -> EL0`, second `hello from EL0`,
  `disk-loaded EL0 program exited ok -> PASS`. This reflash also carried M6f's pending
  metal: all three M6f verdicts PASS and the per-task preempt rider went > 0 on silicon
  (`spsentinel=3`); M6b `exited=1 killed=3 PASS`, M6d ×3 PASS, M6e `IRQs-taken-at-EL0=26`,
  CAPSTONE 6/6, 0 unexpected faults. EL=1, CNTFRQ=54 MHz.
- **Commit:** `faad571` (merge) · arcs `11b8191` `a072a48` `683d48c`.

### U2.5 — FTDI USB-serial console (x86) 🔬 `hw-rmbp`
- **What:** a captured console for the serial-less 2012 rMBP. The kernel
  enumerates an FTDI FT232 (0403:6001) on the xHCI bus, configures it
  (115200 8N1), and drains a 64 KiB boot-capture ring — fed by every
  `serial_print!` since the first — out its bulk-OUT endpoint, so the whole early
  boot log replays out the cable. Also folds three U2-review hardening items
  (first-entry x87/MMX scrub, per-CPU `DR7` clear, whole-ring-3-window zero before
  a load) and fixes the APIC ms-clock: the BSP heartbeat now re-arms *after* the
  calibrated rate is stored (it was pinned to the fixed fallback → metal read
  ~119 Hz instead of ~1000).
- **Tested — QEMU:** `UNAOS_USBSERIAL=1 ./arroyo test 25` → `FTDI USB-SERIAL
  DETECTED (0403:6001)`, `FTDI console up`, `FTDI TX mirror -> PASS (~17 KB
  replayed)`, `target/ftdi.log` carries the boot log; coexists with the U2 disk
  loader (`UNAOS_USBSERIAL=1 UNAOS_FATIMG=1` → both PASS) and the usbdebug view.
  No-knob boot log byte-identical but for the `DR7 cleared` line and the APIC
  re-arm. FAT regression (`test-fat part`/`sf`) green; `./arroyo check` both arches.
- **Metal — PENDING (~2026-07-08):** rides the physical FTDI cable (B0CJVC19CF).
  QEMU-only until then; the APIC ~1000 Hz truth and the FTDI console verify on the
  real rMBP on cable day (`UNAOS_USBDEBUG=1` boot, FTDI in a root USB-A port).
- **Commits:** `229d675` (Part 0 folds) · `ab0f975` (APIC re-arm) · `f7c929e` (FTDI console).

### U2 — loadable ring-3 programs from FAT (x86) ✅ `hw-rmbp`
- **What:** the first *real program loaded from disk* into the x86 privilege
  boundary. A flat ring-3 binary (`HELLO.BIN`) is read off a FAT volume,
  validated, copied read-only-from-start into the user code page, and run in
  ring 3 (`hello from disk`). Plus the boundary preconditions that make loading
  untrusted code safe: #DB and #MC on dedicated interrupt stacks (closing a
  user-triggerable CPU-halt), a register scrub at first entry, and self-test
  fixtures for the NMI stack and the CVE-2012-0217 guard.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 25` across all four FAT
  layouts → the three U2 lines + the Part-0 fixtures + U1a/U1b still PASS, full
  USB boot, 0 unexpected faults. `./arroyo check` both arches.
- **Tested — metal (real 2012 MacBook Pro, 2026-07-03):** Realtek USB3 SD reader
  → FAT16 card → `HELLO.BIN` (72 B) → `hello from disk` PASS. *Metal-pending:*
  the #DB-resume path (TCG can't model the single-step-on-SYSCALL trap) and the
  #MC fire path.
- **Commit:** `9cdf397` (merge) · arc `7d8a6bb`.

### M6f — validated user pointers + wider syscalls (aarch64) ✅ `hw-pi4`
- **What:** `copy_from_user`/`copy_to_user` (bounds- and overflow-checked; a bad
  pointer is an error return, not a task kill) plus the first "real" syscalls
  (`yield`, `sleep_ms`, `getpid`, `getinfo`). Also folds five hardening items
  from the M6d review, incl. scrubbing the FP/SIMD registers at first entry.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → the M6f fixtures PASS
  (getinfo round-trip; four hostile pointers all refused with no kills;
  yield/sleep interleave) with every prior milestone still green.
- **Tested — metal (real Pi 4, 2026-07-04, on the M6g reflash):** all three M6f
  verdicts PASS on silicon (getinfo/copy_to_user round-trip, 4 hostile pointers
  refused with 0 kills, yield/sleep interleave) and the per-task EL0 preempt rider
  went > 0 (`spsentinel=3`, QEMU shows all 0) — the timer preempted that slot task
  and it resumed correctly under its own ASID.
- **Commit:** `ee21e30` (merge) · arcs `71ed153` + `e65ffc0`.

### JM2 — Orin headless first light (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** makes the Jetson build boot headless and safe. Gates the QEMU-virt
  SMP path off the Tegra build (it would otherwise touch Tegra memory that
  isn't there), makes the bootloader boot without a display (the shared
  bootloader — the MacBook boots through it too), and adds a boot-diagnostics
  knob that reports the firmware's real serial/display configuration.
- **Tested — QEMU:** full battery byte-stable — `./arroyo check` both arches;
  x86 U1a/U1b PASS; aarch64 virt v2 + GICv3 SMP lines intact; Pi `kernel8-test`
  unchanged. The Tegra feature is off in all of these, so nothing regresses.
- **Tested — metal (real Orin Nano, 2026-07-03):** the boot diagnostics ran on
  silicon (genuinely headless firmware: 0 display handles) and the headless
  path **entered the kernel for the first time on Orin** — which then faulted
  on its first Tegra UART register read because the firmware-handoff page
  tables don't map Tegra device memory. That diagnosis is the next arc (JM3:
  kernel-owned MMU).
- **Held at integration review:** the merge is waiting on a small must-fix —
  the boot-diagnostics DTB table scan compares against a wrong GUID constant
  (it can never match), so the captured "firmware publishes no DTB" line is
  withdrawn as unverified until a re-run with the corrected constant. Fix
  rides at the head of the JM3 arc.
- **Commit:** *(merge pending the fix)* · arcs `811259c` `d382677` `0bd0dae`
  `d0835c0` `27bf835`.

---

## Round 4 — 2026-07-02

### M6d — per-task address spaces + ASIDs (aarch64) ✅ `hw-pi4`
- **What:** every user task gets its own isolated page tables and its own stack,
  ASID-tagged so task switches need no TLB flush. This is what lets two programs
  use the same virtual address for different data — real process isolation.
- **Tested — metal (real Pi 4):** same-VA isolation proven distinct on real A72
  TLBs (QEMU can't test this — it has no TLB model), stack write/readback PASS,
  all under live timer preemption. `4a06a8c`.

### JC2 — PSCI SMP on GICv3 (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** brings up secondary CPU cores on the QEMU-virt GICv3 path via PSCI,
  proven by cross-core interrupts (each core pinged, both directions).
- **Tested — QEMU:** `UNAOS_GICV3=1 ./arroyo test-arm 30` → 3 cores online +
  cross-core SGI both ways; v2 single-core and Pi SMP unchanged. Metal (Orin)
  pending. `18be259`.

## Round 3 — 2026-07-02

### U1b — ring-3 fault isolation + boundary hardening (x86) ✅ `hw-rmbp`
- **What:** a faulting ring-3 program is killed (kernel survives); plus register
  scrubbing, the CVE-2012-0217 guard, an NMI stack, and cross-core W^X.
- **Tested — metal (real 2012 MacBook Pro):** SMEP active, 3 fault-kills with
  correct syndromes, kernel alive past all three. `37d2af8`.

## Round 2 — 2026-07-02

### M6e — preemptible EL0 (aarch64) ✅ `hw-pi4`
- **What:** the timer can preempt a running user task and resume it correctly.
- **Tested — metal (real Pi 4):** 18 preemptions of a spinning EL0 task, all
  resumed correctly. `e62fd4c`.

## Round 1 — 2026-07-02

First rotation of the three-track machine: **U1a** (x86 ring-3 round-trip),
**M6c** (aarch64 loadable blob), **JC1** (GICv3 beside GICv2) — all landed,
reviewed, merged. `637ee5c`.
