# pi4-regression.spec — the Pi 4 kernel8 chain.
#   QEMU gate:  ./arroyo kernel8-test      → unaos/target/serial-pi.log   (default window: 60 s)
#     MBENCH-HONEST (2026-07-25): the window is no longer something the reader has to remember. The
#     `kernel8-test` DEFAULT is 60 s (it was 8 s, which never finished a boot), the command REPLAYS this
#     spec itself and exits with mbench's status, and a capture that stops short is reported TRUNCATED /
#     INCONCLUSIVE (exit 3) rather than as a pass or a regression — see the END-OF-RUN MARKERS below.
#     The measurements that fixed the window are kept here because they are the evidence:
#     35 -> 60 at BGRUN-2. The old 35 s was ~10% of headroom over the chain and the margin was
#     MACHINE-DEPENDENT, not fixed: BGRUN-2's leg-3 dwell (2 s, plus STAT.ELF's yield-amplified cost
#     while QEMU's degraded SYS_SLEEP_MS makes it spin) tipped slower hosts past the end, dropping the
#     twelve witnesses that print LAST (K8b-snap, K8c-snapread, K6-migrate, all BANDY-*) — a truncation
#     that reads as a regression in arcs nobody touched. Measured on the arc branch: 24 s and 27 s ->
#     44/54, 30 s/35 s/45 s/60 s -> 54/54 on one host while another host failed at 35; at 60 s the last
#     required witness landed ~40% into the log (line 763 of 1917), so the margin is ~1.5x the chain
#     rather than a tenth. Do not trim this back toward the chain length — the tail is what breaks first
#     and it breaks SILENTLY.
#     Re-measured 2026-07-25 (MBENCH-HONEST, this host, 63 witnesses): 8 s -> 41/63 TRUNCATED;
#     60 s -> 63/63 PASS, last required witness (`BANDY-ACL`) at line 1035 of 1682. The 8 s default
#     this arc removed was not marginal — it stopped less than a third of the way through the chain.
#     FLAKE-1 (2026-07-28): the exit contract GAINS a fourth code and loses none. 0 PASS / 1 FAIL /
#     3 TRUNCATED are exactly as above; NEW `4` = HARNESS FLAKE — QEMU never produced a capture, so the
#     run is neither a pass, a regression, nor a truncation. It exists because a `kernel8-test 150` on
#     this bench finished GREEN with no serial-pi.log at all: mbench errored `[Errno 2] No such file or
#     directory` and a downstream `&&` masked it. Cause is a check-then-bind race on the QMP port (the
#     `lsof` pre-scan runs, QEMU binds seconds later after the build, and a concurrent worktree gate can
#     win the bind and kill ours before `-serial file:` creates the log) — unclosable atomically from
#     shell, so `test_kernel8()` now gates on LIVENESS (pid alive + log non-empty within
#     UNAOS_K8T_LIVE_SECS, default 5) and RETRIES on the next port, twice, loudly; exhausted retries or
#     an empty log at assert time exit 4 instead of reaching mbench with nothing to replay.
#     Second FLAKE-1 measurement, and why TRUNCATED now mentions host load: the sufficient window is
#     LOAD-DEPENDENT. Same host, same image, 2026-07-28 — 150 s -> TRUNCATED under concurrent
#     build/QEMU load; 210 s -> PASS clean. This is the same MACHINE-DEPENDENT margin noted above,
#     observed within ONE machine over time rather than between machines.
#   Metal:      ~/pi-serial.log (pi-bench-connect.sh bridge capture)
#
# Metal caveat (unaos-hazards): some real-Pi boots bring up only 3 of 4 cores and
# CAPSTONE self-skips ("capstone skipped (needs >= 3 online APs)") — scheduler-track
# variance, orthogonal to the syscall chain. A power-cycle usually restores 6/6.
# On such a boot the CAPSTONE directives below report as misses; the 23-PASS chain
# and the K1/F2/F3 witnesses must still hold.

# --- END-OF-RUN MARKERS (MBENCH-HONEST) --------------------------------------------
# The header above documents the truncation trap; these two lines are what let the TOOL
# enforce it instead of the reader remembering. A capture that reaches neither is
# reported TRUNCATED / INCONCLUSIVE (exit 3) — never PASS, and never FAIL — so a short
# log can no longer be read as a regression in an arc that touched nothing.
#
# WHY THESE TWO, and why they are trustworthy:
#   1. `:: SCHED: task 'el0-midden' -> core N ::` is the scheduler's own placement line
#      for MIDDEN.BIN, and `bandy_rt_launcher` — which spawns it — is documented in
#      arch/aarch64/syscall.rs as LAST IN THE CHAIN of the u7_launcher fixture cascade.
#      Reaching it means the boot got through every earlier fixture in this spec. It is
#      emitted by `sched::spawn_user_slot` under `#[cfg(feature = "pi")]`, i.e. on the
#      Pi target and on `kernel8-test` alike, and it is STRUCTURAL, not a verdict: no
#      regression in any witness below can suppress it, so a real regression still
#      reads FAIL rather than hiding behind "inconclusive".
#   2. `:: BANDY-RT:` covers the launcher's honest early exits (no card / MIDDEN.BIN
#      absent / staging failed / midden failed to load). Those boots also ran to the end
#      of the chain, so they must fail on their missing witnesses — not read as short.
# A capture that ends MID-LINE (no terminating newline) is truncated regardless: that is
# direct evidence QEMU was killed while the kernel was still writing.
#
# Known narrow gap, stated rather than hidden: marker 1 lands at the midden SPAWN, and
# the five BANDY-RT/EQ/WR/EQ2/ACL verdicts print when midden EXITS (measured 2026-07-25:
# spawn at line 956, last verdict at line 1035 of a 1682-line 60 s capture). A capture
# severed inside that ~79-line window, exactly on a newline, reports FAIL rather than
# TRUNCATED. The bias is deliberate: both are red, and marker 1 is the last STRUCTURAL
# line available — pinning anything later would mean pinning a verdict, which is what
# would let a genuine regression disguise itself as a short log.
COMPLETE :: SCHED: task 'el0-midden' -> core
COMPLETE :: BANDY-RT:

# --- the aggregate: 23 fixture verdicts -------------------------------------------
COUNT 23 -> PASS

# --- scheduler capstone: all 6 sync primitives in one boot -------------------------
COUNT 6 CAPSTONE \w+: PASS
REQUIRE CAPSTONE COMPLETE

# --- per-arc verdicts (granular diagnosis when the chain breaks mid-way) -----------
REQUIRE M6b: EL0 fault isolation.*-> PASS
REQUIRE M6g: disk-loaded EL0 program exited ok -> PASS
REQUIRE U4: process model.*-> PASS
REQUIRE U5: capabilities.*-> PASS
REQUIRE U6: general object table.*-> PASS
REQUIRE U6b: real File handles.*-> PASS
REQUIRE U7: cross-process transfer.*-> PASS
# U7FIX (P63 metal-only): the U7 fixtures' GO parks must outlast the LAUNCHER, and the launcher's deadlines
# are wall-clock while a bare-yield park is denominated in ITERATIONS — ~1 ms each under QEMU's emulation but
# a few hundred ns on a real idle A72 core. On metal the child gave up before GO was ever released
# (`child=0x0 used=0 snap=false`, parent stuck at the partial `0x3`). The park primitive is SYS_SLEEP_MS now
# (a real 250 Hz tick on metal; still a cooperative yield under QEMU, where nothing was broken).
# NOTE what this REQUIRE can and cannot do: because SYS_SLEEP_MS degrades to a yield under QEMU, QEMU cannot
# tell the fixed park from the broken one and CANNOT gate the fix itself — that confirmation is the bench's.
# What it DOES gate, on both, is the launcher's new parked-out assertion: neither fixture may have exited
# before its GO was released. That is the fact that names the defect directly, and its absence is what made
# P63 a puzzle. The reported margins are also the early warning — they shrink before they cliff.
REQUIRE \[u7fix\] park margin — child parked [0-9]+ms before GO \(parked_out=0\), parent parked [0-9]+ms before GO \(parked_out=false\); park primitive=SYS_SLEEP_MS budget=0x8000
FORBID \[u7fix\] .*child parked [0-9]+ms before GO \(parked_out=[1-9]
FORBID \[u7fix\] .*parent parked [0-9]+ms before GO \(parked_out=true
REQUIRE U8: revocation trees.*-> PASS
REQUIRE U9: real File writes.*-> PASS
REQUIRE U10: file growth.*-> PASS
REQUIRE U10-create: file create.*-> PASS
REQUIRE U10-delete: file delete.*-> PASS
REQUIRE U11: open-file lifecycle.*-> PASS
REQUIRE U11-defer: cross-process unlink-defers-free.*-> PASS
REQUIRE U11-reuse: sys_unlink slot-recycle.*-> PASS
REQUIRE U11-reap: teardown-last-close reaper.*-> PASS
REQUIRE U6-grants: owner/grants on open.*-> PASS

# --- K1 survive-reboot witnesses (uncounted — not `-> PASS` fixture lines) ---------
REQUIRE K1-persist:.*SURVIVE REBOOT.*PASS
REQUIRE K1-corrupt:.*fails closed to PUBLIC at boot PASS
OPTIONAL K1-atr:.*codec PASS

# --- F2/F3 SMP witnesses (locked leg must be lossless) ------------------------------
REQUIRE F2-witness:.*locked 240000/240000 intact
REQUIRE F3-witness:.*locked 240000/240000 intact

# --- forbidden: card-reported errors + faults (defaults -> FAIL / FAIL :: / PANIC
# --- are always on) -----------------------------------------------------------------
FORBID R1 error status
FORBID programming-busy timeout
FORBID AARCH64 EXCEPTION

# --- K2 live-enforcement witness (uncounted line — REQUIREd here because the launcher
# --- has silent no-verdict exit paths: a green battery without this line = proof not run,
# --- per the K2 security-review note, 2026-07-11) --------------------------------------
REQUIRE K2-liveenf:.*rebuild\+enforce PASS

# --- K3 two-phase durable-first revoke witness (uncounted). METAL-CONFIRMED 2026-07-12
# --- (real Pi 4, kernel a834b8f); promoted from ledger to a hard REQUIRE at that capture. -----
REQUIRE K3-revoke:.*durable-first PASS

# --- K9-PARITY mid-staging-failure discard witness (uncounted, 7 bits, PASS = w=0x7f): a staged ACL
# --- persist that fails PARTWAY leaves no partial-durable row (K3) AND its uncommitted residue can no
# --- longer be flushed by a later persist's commit — closes the K9 lens-B deferred residual in-lane
# --- (with_unafs discards the dirty mount inside the serialized hold). QEMU-proven; metal rides the
# --- next Pi sitting. ---------------------------------------------------------------------------------
REQUIRE K9-parity:.*discarded residue PASS
FORBID K9-parity:.*discarded residue FAIL

# --- UNAFS-K3 RO kernel mount witness (uncounted): the native unafs volume is located by magic,
# --- superblock mounted RO, ls/cat byte-verified against the staged fixture [w=0x1ff]. The BeFS
# --- storage chain reaches silicon (K1/K2 ACL + K3 mount). METAL-CONFIRMED 2026-07-12 (real Pi 4,
# --- x5 boots, kernel 1ccd00c) -> promoted to a hard REQUIRE at that capture. ------------------
REQUIRE K3-mount:.*byte-verified PASS

# --- UNAFS-K4 kernel-write witness (uncounted): create + write a scratch file through the single
# --- coherent mount, force a genuine remount, byte-verify the durable write, delete it, remount
# --- (delete durable), negative path, refcount-consistent tree (the K8a CoW successor of the old
# --- clean-journal bit — the WAL is gone). Self-cleaning (leaves only the staged K3 fixtures).
# --- QEMU-proven via if=sd write-back; the metal write->power-cycle->boot-2 byte-verify rides Peter's bench.
REQUIRE K4-write:.*clean-tree PASS
FORBID K4-write:.*FAIL

# --- UNAFS-K8a copy-on-write witness (uncounted): root generation advances per mutation; a power
# --- cut before the 512 B root flip (autocommit-off crash seam + genuine remount) converges to the
# --- OLD tree; refcounts persist across a remount; commit-path bench counters (CNTPCT ticks +
# --- blocks written) live. Self-cleaning. QEMU-proven via if=sd write-back; metal rides the
# --- attended sitting (incl. the pre-K8 card migration).
REQUIRE K8a-cow:.*PASS
FORBID K8a-cow:.*FAIL

# --- UNAFS-K8b retained-roots (snapshots) + reclamation witness (uncounted): snapshot the committed
# --- tree, overwrite the live file, byte-verify the snapshot's OLD data blocks are untouched (the
# --- never-overwrite + block-sharing core), confirm the retention-aware allocator never hands out a
# --- block a live snapshot holds, drop + eager reclaim (freeing only blocks no live/retained root
# --- still reaches), and a power-cut-mid-drain (enqueue-only + genuine remount) converges (the queue
# --- resumes on remount). Self-cleaning. QEMU-proven via if=sd write-back; metal rides the attended
# --- sitting.
REQUIRE K8b-snap:.*PASS
FORBID K8b-snap:.*FAIL

# --- UNAFS-K8c snapshot-read current-ACL witness (uncounted, 8 bits, PASS = w=0xff): the snapshot
# --- READ path enforces the LIVE object's CURRENT ACL (the "high security" ruling — revocation
# --- reaches the past). Owner + read-grantee read the OLD retained bytes; an impostor is refused from
# --- the snapshot by the SAME evaluator that refuses the live read; a WRITE-ONLY grantee is refused
# --- (rights-aware — the grant must carry CAP_READ, lens A fold); dropping a grant retroactively
# --- refuses the snapshot; and a live-DELETED object fails closed (no current ACL row) even for its
# --- owner — the deleted-object edge, traced. Self-cleaning. QEMU-proven via if=sd write-back; metal
# --- rides the next Pi sitting.
REQUIRE K8c-snapread:.*PASS
FORBID K8c-snapread:.*FAIL

# --- K4-ready native-attr projection codec witness (uncounted). Pure in-RAM codec/selftest
# --- (runs every boot, no card needed) — METAL-CONFIRMED present 2026-07-12, now REQUIRE. -----
REQUIRE K4-ready:.*prefix\) PASS

# --- IMG-SIG code-signing witness (uncounted): the loader mints the IMAGE_SHA256 principal.
# --- METAL-CONFIRMED 2026-07-12 (real Pi 4, kernel a834b8f) → promoted PENDING -> REQUIRE. --------
REQUIRE IMG-SIG:.*residual closed\) PASS
FORBID IMG-SIG:.*FAIL

# --- FATDIRS directory create/remove witness (uncounted): create_dir/remove_dir drive the live
# --- volume end to end. METAL-CONFIRMED 2026-07-12 → promoted PENDING -> REQUIRE. ----------------
REQUIRE FATDIRS:.*delete_located\) PASS
FORBID FATDIRS:.*FAIL

# --- FATMOVE rename/move witness (uncounted): rename_entry/move_entry drive the live volume end to
# --- end (rename in place; move a file across dirs by reference; onto-existing + directory refused).
# --- METAL-CONFIRMED 2026-07-12 (Pi captured it FIRST, freeing the Orin bench) -> REQUIRE. ---------
REQUIRE FATMOVE:.*keep-chain\) PASS
FORBID FATMOVE:.*FAIL

# --- K6 native-attr migration witness (uncounted): the U6 ACL round-trips through the native unafs
# --- attribute volume (codec forward+reverse, the 240-bit-prefix invariant) AND the sidecar migration
# --- is native-before-delete (IMAGE row migrates+verifies+converges across a both-copies power-cut
# --- window; legacy PROGRAM_NAME rows stay fail-closed un-migrated). Folded by the K6 arc per the
# --- M3 lock-strategy verdict rider (Maestro, 2026-07-15); metal capture rides the K6 bench. --------
REQUIRE K6-migrate:.*legacy PROGRAM_NAME stays\) PASS
FORBID K6-migrate:.*FAIL

# --- BANDY-CODEC bus v1 subset codec witness (uncounted): reply bodies byte-compatible with the
# --- HOST serializer (tools/bandy-golden captures — never hand-authored), the UnaOS-native request
# --- header + typed ls/cat/cp payloads frozen, decoding fail-closed at the 4 KiB body ceiling.
# --- BANDY-1 M1 (2026-07-16); read-only/in-RAM, runs every boot. ---------------------------------
REQUIRE BANDY-CODEC:.*decode fail-closed.*PASS
FORBID BANDY-CODEC:.*FAIL

# --- BANDY-CODEC2 write-side codec witness (uncounted): the write/rm/mv request goldens frozen,
# --- the typed WRITE [name_len][name][content] payload (empty + at-ceiling content), decode
# --- fail-closed. A SIBLING of BANDY-CODEC (the BANDY-1 goldens stay byte-identical). BANDY-2 M1
# --- (2026-07-16); read-only/in-RAM, runs every boot. --------------------------------------------
REQUIRE BANDY-CODEC2:.*decode fail-closed.*PASS
FORBID BANDY-CODEC2:.*FAIL

# --- BANDY-STAMP transport witness (uncounted): principal stamping is KERNEL-only (a caller-
# --- supplied principal field is -EINVAL, never overwritten); replies carry the RESERVED kernel
# --- kind, fail-closed as grantee/owner/persist target; per-ASID mailboxes bounded (depth 16,
# --- -EAGAIN before fulfillment, no cross-ASID leverage); gen-fenced across teardown.
# --- BANDY-1 M2/M5 (2026-07-16); drives the production sys_msend_for path with scratch ids. ----
REQUIRE BANDY-STAMP:.*gen-fenced PASS
FORBID BANDY-STAMP:.*FAIL

# --- BANDY-RT round-trip witness (uncounted): MIDDEN.BIN (program #3) parses ls/cat/cp text at
# --- EL0 into typed native frames, SYS_MSEND -> kernel fulfillment under the stamped IMAGE_SHA256
# --- principal -> SYS_MRECV -> printed replies; the cp copy is byte-exact and private to the
# --- invoker; fully self-cleaning (no metal-card residue). BANDY-1 M4/M5 (2026-07-16). ----------
REQUIRE BANDY-RT:.*self-cleaned PASS
FORBID BANDY-RT:.*FAIL

# --- BANDY-EQ equivalence witness (uncounted, verdict D): a principal denied via the direct
# --- syscall surface is denied via the bus with the BYTE-SAME errno (and allowed <-> allowed),
# --- both legs driven at EL0 by midden through the production paths. ----------------------------
REQUIRE BANDY-EQ:.*both legs at EL0 through the production paths PASS
FORBID BANDY-EQ:.*FAIL

# --- BANDY-2 write-side witnesses (uncounted), driven by midden at EL0 through the production
# --- paths + a kernel-side ACL integrity check. WR: create->cat byte-exact, truncate->cat, rm->cat
# --- -ENOENT, mv->cat(new) byte-exact + cat(old) -ENOENT. EQ2: rm/mv/write of a foreign-owned file
# --- denied-via-bus == denied-via-syscall (byte-same -EACCES). ACL: the denied destructive verbs
# --- left the foreign owner row INTACT (no stale-owner strand / same-name re-adoption — the K1-F2
# --- class), no stolen name, write-side fixtures self-cleaned. BANDY-2 M2/M4 (2026-07-16). --------
REQUIRE BANDY-WR:.*mv->cat\(new\) byte-exact.*PASS
FORBID BANDY-WR:.*FAIL
REQUIRE BANDY-EQ2:.*byte-same -EACCES.*PASS
FORBID BANDY-EQ2:.*FAIL
REQUIRE BANDY-ACL:.*foreign owner row intact.*PASS
FORBID BANDY-ACL:.*FAIL

# --- BANDY-GRANT truncate-preserves-grants witness (uncounted, the BANDY-2 lens-2 fix): the bus
# --- write-truncate (delete-then-recreate) SNAPSHOTS + RESTORES + RE-PERSISTS the file's grant
# --- rows — a content rewrite is not a revoke (the direct twin preserves grants in place, so the
# --- bus must too). Grantee admitted via bus AND direct gate after the truncate, byte-equivalent;
# --- grant durable in the native row; self-cleaned. --------------------------------------------
REQUIRE BANDY-GRANT:.*grant re-persisted durable.*PASS
FORBID BANDY-GRANT:.*FAIL

# NOTE (bench operators): these five are now hard REQUIREs. On a rare no-card / hub-MSC-vid=0000
# boot the card-dependent selftests won't emit — re-seat the data card and re-boot (that IS the
# recovery); don't demote the spec. The 3-of-4-core CAPSTONE variance is separate (see the header).

# --- WC-C window-compositor contracts (uncounted, QEMU + metal) --------------------------------
# --- The WC-C arc changed three things that nothing else machine-checks. Pinned here so a
# --- regression is a spec miss rather than an eyeball miss.
#
# --- 1. UVUG's 300-frame auto checksum. It is a pure function of the final surface, so it is the
# ---    tightest available assertion that the WINDOWED 128x128 render still produces the exact
# ---    pixels it did when the arc landed. WC-C deliberately superseded the pre-WC-C 32x32 value
# ---    0x48221e4101db3924 (see userspace.md); this is the current one.
REQUIRE UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c
#
# --- 1b. INROUTE: the HID->EL0 router witness. The FAIL half was already covered by the default
# ---     `FAIL ::` FORBID, but nothing REQUIRED the PASS — a selftest that stopped running, or one
# ---     whose call site was cfg'd out of the boot, went green by silence. Both halves are pinned
# ---     now, and the `revokes=0` line pins the PRECONDITION the arc fixed: this test must own the
# ---     global input focus for its window, so a slot teardown revoking focus mid-measurement (which
# ---     is what made it flake ~1 boot in 7) fails the gate instead of merely being unlucky.
REQUIRE EL0: input router — routed=2 .*GUI_CHANNEL bypassed :: PASS
REQUIRE \[inroute\] router window — routed=2 stale_dropped=1 revokes=0
FORBID EL0: input router.*FAIL
#
# --- 2. The el0-wcb window-verb ledger, ALL THIRTEEN bits. The literal mask matters: a partial
# ---    mask still prints `witness=0x...` and the verdict already refuses it, but pinning 0x1fff
# ---    here means a silently NARROWED ledger (bits removed from the fixture) also fails.
REQUIRE EL0: window verbs.*witness=0x1fff.*PASS
#
# --- 3. The side-by-side composite. This is the arc's actual claim — two windows drawn in ONE
# ---    compositor pass — and it is the line that gates the per-window checksum lines that follow
# ---    it. Without this REQUIRE a fixture whose second window presented BLANK would still pass
# ---    every other directive, because nothing else reads those checksums.
REQUIRE \[wc-c\] side-by-side windows=2 drawn=2
#
# --- 4. WC-D: the SCAN-OUT VERDICT. Every directive above this one checks a number the kernel
# ---    computed about a surface; none of them looks at the panel. This one re-derives the window's
# ---    destination pixels from the source surface and reads the scan-out buffer back.
# ---    `bad_cache=0` is the half this GATE earns: the blit's stride/pitch arithmetic, upscale
# ---    indexing, colour encoding and clipping are right. `bad_ram` is read after a bare DC IVAC
# ---    (invalidate, NO write-back) and reports whether the pixels reached the memory the HVS scans
# ---    — it is only MEANINGFUL ON METAL, because QEMU does not model the non-coherent scan-out.
# ---    Flush EXTENT is excluded by inspection, not by this directive: draw_window flushes whole
# ---    scanlines over the outer_box, a strict superset of the blitted pixels.
# ---    The FORBID is the half with teeth: a FAIL verdict fails the gate wherever it appears.
# ---    NOTE: the QEMU panel is 640x480 and the bench Pi drives 1920x1200, and the compositor's
# ---    upscale is a FUNCTION of the panel — so this gate exercises scale 1x/3x/4x while the bench
# ---    runs 4x (WC-SCALE's legibility ceiling is 4x at both panel heights, and it is what brings the
# ---    24x16 window down from the old 13x here / 37x there). Run
# ---    `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test` to reproduce the bench
# ---    geometry here; that is the configuration in which the scaled blit was cleared (see
# ---    docs/dev/OS/08_VIDEO/engine.md §WC-D).
REQUIRE \[wc-d\] verify win=.*bad_cache=0 bad_ram=0.*-> PASS
FORBID \[wc-d\] verify .*-> FAIL

# --- 4b. FOCUS-VIS: FOCUS IS VISIBLE, and the SHELL is in the z-order. Every other focus directive
# ---    in this file reports KERNEL STATE — `[wc-c] focus tab-cycle` printed a correct rotation on
# ---    the P59 bench for a panel that never changed, which is exactly the failure this catches.
# ---    `[wc-fv] focus-vis` places two solid-colour windows at ONE origin (so exactly one can own the
# ---    probe pixel) and READS THE SCAN-OUT BACK after each focus move: stack (later window in
# ---    front), raise (focusing the covered window brings it forward), shell (focusing the shell
# ---    slot takes both windows out of those pixels — the "TAB to the prompt and read your output"
# ---    case), reraise (a window comes back from under the shell). All four legs are in the one
# ---    verdict, so a partial regression cannot pass by satisfying a prefix.
# ---    The interactive path that reaches this on metal is TAB, which QEMU cannot press; the
# ---    selftest drives `wm::focus_changed` directly, which is the same seam `wc_focus_key` calls.
REQUIRE \[wc-fv\] focus-vis .*-> PASS
FORBID \[wc-fv\] focus-vis .*-> FAIL

# --- BGRUN-1: background EL0 runs (bg/jobs/kill). Headless-observable core of the contract,
# ---   REQUIREd per the round-1 lens: leg 1 proves spawn->exit->reap (the sole-reaper contract);
# ---   leg 2 proves a killed bg row settles (confirmed-kill reaps in place; no leaked PRUNNING row
# ---   lying "running" under a dead task); leg 3 (BGRUN-2) proves PERSISTENCE — STAT.ELF, an EL0
# ---   window app with no exit condition, is still `Running` after a 2 s dwell and then settles when
# ---   killed. Legs 1 and 2 structurally cannot prove that: ELFHELLO exits in three syscalls and UVUG
# ---   exited after 300 auto frames, which is precisely why the bench could not test TAB before — a
# ---   backgrounded UVUG was unfocused, never left its auto path, and was gone in seconds. The
# ---   interactive half (TAB between two bg windows) is still bench-only — QEMU has no HID.
# --- VUG-BG (this arc): a BACKGROUNDED VUG.ELF now persists as well, so leg 2's kill target no longer
# ---   races its own exit. Leg 3 keeps the persistence proof regardless: STAT.ELF has no exit condition
# ---   AT ALL, focused or not, where VUG's persistence is conditional on the detached bit.
REQUIRE BGRUN-ST: spawn->exit->reap PASS
# --- PROCS-6: the cap itself, pinned. The reclaim leg below drives MAX_PROCS+2 launches, so its own
# ---   PASS text cannot tell you which capacity it exercised; this line can, and a silent regression
# ---   of the cap back to 4 (or a raise past the EL0 slot pool) fails HERE rather than mysteriously.
REQUIRE BGRUN-ST: process table capacity = 6 rows \(bg programs alive at once; EL0 slots 8\)
# --- BGRUN-SCAV: exited-but-unreaped rows must not deny a launch the machine can satisfy. MAX_PROCS+2
# ---   bg launches with NO intervening reap; every one must succeed (8 launches at the PROCS-6 cap, the
# ---   last two served only by the PEXITED scavenge). Goes red on the pre-fix kernel.
REQUIRE BGRUN-ST: slot reclaim PASS
REQUIRE BGRUN-ST: kill mid-run PASS \(pid=[0-9]+, killed — row reaped
REQUIRE BGRUN-ST: persist\+kill PASS \(pid=[0-9]+,
FORBID BGRUN-ST: .*-> FAIL

# --- KILLBOUND: a kill must reach a target PARKED in a kernel wait, and neither of the two bounded
# --- tables may be wedged by killing programs that never got to clean up after themselves. The three
# --- BGRUN-ST legs above all kill RUNNABLE targets (VUG makes syscalls every frame, KVUG spins), so
# --- they passed on the very boot where the operator's Pi wedged. This leg kills a target parked in
# --- `futex_wait` with no waker — the state a windowed app reaches at its frame barrier when its
# --- worker threads are absent — five rounds deep, which is one more than the global thread table
# --- holds. Each round REQUIREs a positive park witness (3 futex waiters) before the kill, then
# --- kill-confirmed + row reaped + ASID drained. Uncounted (no `-> PASS`). QEMU-proven; metal rides
# --- the attended sitting.
REQUIRE KILLBOUND: 5/5 rounds .*PASS
FORBID KILLBOUND: .*-> FAIL
#
# --- 5. WC-E: the SCAN-OUT GROUND TRUTH. Every directive above this one checks a number the KERNEL
# ---    computed; even WC-D's read-back goes through the same `info.stride` it wrote through, so it
# ---    agrees with itself no matter what the display pipe is doing. This one carries what the
# ---    FIRMWARE says it programmed. `row_ok=true` is `pitch == virt_w * bpp` — the identity a
# ---    row-phase garble breaks — and `fit_ok=true` says the allocation holds the whole visible
# ---    image. Pinning both means a firmware that clamps, rounds or refuses any part of the geometry
# ---    we requested fails the gate instead of reaching a panel as unexplained garble.
# ---    See docs/dev/OS/08_VIDEO/engine.md §WC-E.
REQUIRE \[wc-e\] fb-geometry .*row_ok=true fit_ok=true
FORBID \[wc-e\] fb-geometry query FAILED
#
# --- 6. WC-F: the INDEPENDENT read. WC-E states the firmware's geometry; nothing checked it against
# ---    the `FrameBuffer` the compositor actually addresses through, which is a separate object and
# ---    can diverge in base, mapped length or row step with no witness able to see it.
# ---    NOTE what is deliberately NOT pinned here: `stride * bpp == pitch` is an IDENTITY of
# ---    init_framebuffer (`stride = pitch / 4`), not an observation — it cannot be false, and the
# ---    arc's first cut pinned exactly that and proved nothing. The load-bearing field on
# ---    `[wc-f] scanout` is `rowstep_match`: `stride * bpp` against `virt_w * 4`, a row step derived
# ---    from the reported GEOMETRY rather than from the pitch reply, false exactly when the firmware
# ---    pads a row the compositor ignores (`pad=` gives the bytes).
# ---    `[wc-f] twin` renders one known pattern TWICE at the bench's 4x upscale — left through
# ---    put_pixel/info.stride (the compositor's addressing), right through raw stores stepped by
# ---    `virt_w * 4` — and cross-reads each block through the other path. comp_bad/direct_bad are the
# ---    two addressings disagreeing; PASS also requires skipped=0, lost=0 and the full checked count,
# ---    so a probe that compared nothing cannot read as agreement.
# ---    A third line, `[wc-f] ramp`, carries no verdict and is not pinned: its value is the
# ---    PHOTOGRAPHED slope of a marker stepped k_row+4 bytes per mark, which measures the row step the
# ---    HVS actually uses — the one reading no serial number can give, since every number on this wire
# ---    is downstream of the firmware's own claim. `lost=` on it says the marker reached the panel.
# ---    SKIP is TERMINAL only (no framebuffer, no firmware truth, unusable layout, panel too small) and
# ---    is therefore forbidden. A window sitting over the probe strip is retryable, not terminal: it
# ---    emits a one-shot `-> DEFER` — deliberately NOT forbidden — and the probe keeps trying, so a run
# ---    that defers early and passes later stays green.
# ---    All three lines exist under the `witness` feature only. See docs/dev/OS/08_VIDEO/engine.md §WC-F.
REQUIRE \[wc-f\] scanout .*-> PASS
FORBID \[wc-f\] scanout .*-> FAIL
FORBID \[wc-f\] scanout -> SKIP
REQUIRE \[wc-f\] twin .*-> PASS
FORBID \[wc-f\] twin .*-> FAIL
FORBID \[wc-f\] twin -> SKIP

# --- WC-G: the window PRESENT path, instrumented WHILE IT RUNS.
# ---    Every earlier instrument in this chain measured CONVERGED content — a one-shot read-back, a
# ---    static twin, a photographed ramp — and all of them passed while a live window still garbled.
# ---    WC-G samples the non-converged case: four checksums of one surface taken around one blit
# ---    (`app` at the owner's present, `blit` as the copy finds it, `civac` through the coherent
# ---    view, `after` as the copy leaves it), a scan-out read-back of the content rect (`fbbad`), and
# ---    the blit's wall-clock duration (`us`/`slow`). Those legs separate a source race from a
# ---    coherency fault from a blit-path defect from an unbuffered-copy TIMING defect.
# ---    Budgeted at 4 samples per window id; `own=` records whether the blit followed that window's
# ---    own present or was collateral damage-closure repaint (the case where the owner is running
# ---    free at EL0 with nothing serialising it against the copy of its surface).
# ---
# ---    SCOPE, and why there is no global summary line. Three cuts of one were tried and all three
# ---    lied in the same direction. (1) Fire when the FIRST window spends its budget: printed the
# ---    summary before window 2 was sampled at all, including before its own=no collateral-repaint
# ---    sample. (2) Fire when every SAMPLED window has spent its budget: same bug in new clothes —
# ---    the sampled set only holds windows seen so far, so it is trivially true the instant the first
# ---    window finishes, and it reproduced exactly (`scope=exhausted samples=4 windows=1`, before
# ---    window 2 existed). (3) Fire on quiescence: the gate's two apps start more than 3 s apart, so
# ---    an idle gap is not evidence sampling is over — it fired early too (`idle_us=3011902`).
# ---    The lesson is structural: nothing observable inside a boot distinguishes "sampling finished"
# ---    from "the next app has not launched yet". Any global summary is a completeness claim the
# ---    instrument cannot support, and one that overstates its scope is worse than none — it makes
# ---    later contrary evidence look already accounted for. So the rollup is scoped to ONE window and
# ---    fires when that window spends its budget: deterministic, no timer, scope == its own `win=`.
# ---
# ---    The REQUIREs assert the INSTRUMENT RAN, deliberately not what it found: the finding is the
# ---    arc's output. The FORBIDs are the other half, and they are what the global summary was
# ---    reaching for — "no suspect fired anywhere, ever". A FORBID needs no completeness claim: it
# ---    catches a COHER/RACE/BLIT verdict in any window at any point in the boot, including one that
# ---    appears long after every rollup has printed. CLEAN and CLEAN+SLOW stay green — the timing
# ---    finding is this arc's result, not a regression. Witness-feature only.
# ---    See docs/dev/OS/08_VIDEO/engine.md §WC-G.
REQUIRE \[wc-g\] win=.* fbbad=.* slow=.* ->
REQUIRE \[wc-g\] rollup win=.* scope=window .*frame_us=.* ->
FORBID \[wc-g\] .*-> COHER
FORBID \[wc-g\] .*-> RACE
FORBID \[wc-g\] .*-> BLIT

# --- WC-H: the fix WC-G's localization called for — the window layer gets a back buffer.
# ---    WC-G's finding was CLEAN+SLOW: every byte correct at every moment, and the copy still
# ---    guaranteed to be overtaken by the beam (`us=15524` against `rectscan_us=7111`), because a
# ---    window's pixels were poked one at a time into the LIVE scan-out with no vblank sync. WC-H
# ---    composes each window — chrome, title, upscaled content — into a cached-RAM back layer and
# ---    presents its box as contiguous per-row bulk copies, which is the discipline that has always
# ---    made the desktop path (`Screen`'s back buffer + damage-rect flush) clean.
# ---
# ---    `[wc-h]` splits the operation into the two halves that now have different meanings:
# ---    `compose_us` — off-screen, no scan-out can observe it — and `present_us`, the row copies,
# ---    which is the ONLY phase that can still tear. `torn=` compares THAT against the beam's time
# ---    on the box (`rectscan_us`, computed exactly as WC-G computes it, with the same deliberate
# ---    bias toward not reporting a tear). The FORBID on `AT-RISK` is the arc's real assertion: it
# ---    fires if any window's present phase ever outruns the beam again — a regression back to the
# ---    tearing regime, caught in any window at any point in the boot.
# ---
# ---    WHY THE WC-G FORBIDS AND ITS `slow=` LEG ARE UNCHANGED. `[wc-g] us=` brackets the whole of
# ---    `draw_window`, and WC-H did not change what that bracket means — it changed what the copy
# ---    DOES. `slow=yes` therefore no longer implies a torn panel: it says the whole operation
# ---    outran the beam, most of which the beam cannot see. Re-scoping or deleting the leg would
# ---    have destroyed a checksum instrument that is still the only thing separating a source race
# ---    from a coherency fault; the tear question moved to `[wc-h] torn=` instead, which is narrower
# ---    and true. CLEAN+SLOW stays green for the same reason it did under WC-G.
# ---
# ---    Per-sample lines are a best-effort trace and can be FEWER than the rollup's `samples=`:
# ---    there is one pending slot per window id, so two cores compositing the same window
# ---    concurrently lose one line. The rollup's counters are updated at record time and miss
# ---    nothing, which is why the tear assertion is pinned on the rollup verdict.
# ---    Witness-feature only. See docs/dev/OS/08_VIDEO/engine.md §WC-H.
# ---    DECLINES ARE SAMPLES, and that is a correction to this witness's first cut. `stage_window`
# ---    has four fall-back exits — box over the 4 MiB cap, `try_lock` lost to another core,
# ---    allocator refusal, degenerate geometry — and each runs the DIRECT, pre-WC-H path, i.e. the
# ---    tearing regime. Firing only on staged success made the verdict an overclaim: a boot in which
# ---    96 of 100 composites lost the lock to a concurrent desktop flush would have torn
# ---    continuously and still printed TEAR-FREE from its four staged samples, with nothing to
# ---    catch it. So a decline spends budget, prints `-> DIRECT reason=`, and forces the rollup to
# ---    `UNSTAGED` — which the FORBID below catches, and which makes a permanent cap fallback loud
# ---    for free. `fixture` is counted apart and excluded from `declines=`: it is the kernel's own
# ---    one-shot fallback (below), not a failure.
# ---
# ---    THE FIXTURE, and the coverage it restores. Before WC-H every `[wc-d] verify` read a
# ---    directly-drawn window; afterwards every one of them read a staged present, so the fallback
# ---    path stopped being verified against the scan-out at all — coverage traded away silently. A
# ---    witness-only global one-shot latch forces the FIRST composite WC-D is about to verify onto
# ---    the direct path. The gate runs two windows, so exactly one is verified on each path, and the
# ---    REQUIRE below asserts the fallback was actually exercised.
REQUIRE \[wc-h\] win=.* compose_us=.* present_us=.* torn=.* -> BUFFERED
REQUIRE \[wc-h\] win=.* staged=no reason=fixture -> DIRECT
REQUIRE \[wc-h\] rollup win=.* scope=window .*declines=.* -> TEAR-FREE
FORBID \[wc-h\] .*-> AT-RISK
FORBID \[wc-h\] .*-> UNSTAGED

# --- CURSOR-3 — overlay-present path (printed alongside [wc-i], witness-feature only) ----------
#     The rollup reports the sprite mechanism across composite passes: UNWITNESSED on QEMU (no
#     pointer), COMPOSED on metal (overlay taken). See docs/dev/OS/08_VIDEO/engine.md §CURSOR-3.
REQUIRE \[cursor3\] rollup scope=.* planned=.* offers=.* taken=.* adopt=.* repaint=.* ensure=.* stale=.* ->

# --- CURSOR-5 — sprite/compositor coherence residual (printed right after [cursor3]'s rollup) ---
#     P64 (attended): "mouse still spotty [over vug] and causes a flash in the vug display here and
#     there if you tweak the mouse just so". The flash was WC-L's deferred-erase drain calling a FULL
#     `cursor::undraw` from INSIDE an open overlay session: the session's plan still matched, so the
#     staged presents kept composing the arrow onto the panel while the sprite module believed itself
#     off-panel, and the next save-under captured its own fill. CURSOR-5 moved the drain ahead of the
#     bracket and gave `compose_into` a lock-free generation check.
#
#     `drain_insession` is the direct detector for the ordering and must stay 0 — a non-zero count
#     means someone put the drain back inside the bracket. It is scoped to the core that OPENED the
#     session, so the VUGPAR steady state (another core legitimately mid-session while this one
#     drains) does not trip it; that case is absorbed by the generation check and shows up as
#     `stale_compose` instead. UNWITNESSED on QEMU (no HID pointer, so the sprite is never drawn);
#     COHERENT/RESIDUAL on metal.
#     See docs/dev/OS/08_VIDEO/engine.md §CURSOR-5.
REQUIRE \[cursor5\] rollup scope=.* stale_compose=.* adopt_incoh=.* selfsave=.* masked_nosession=.* drain_insession=.* ->
FORBID \[cursor5\] .*-> REGRESSED
FORBID \[cursor5\] .*drain_insession=[1-9]

# --- CURSOR-6 — what the PANEL got, which no earlier cursor counter could reach ------------------
#     P65v2 (attended, pi4-r23s1o): every CURSOR-5 mechanism silent (`-> COHERENT`) while the spotty
#     cursor and the vug-window flash both survived. That is not a contradiction — every CURSOR-3/4/5
#     counter is taken from inside the sprite module's own bookkeeping, and a painter that overwrites
#     the arrow's pixels without consulting the module leaves that bookkeeping self-consistent.
#
#     `[cursor6]` measures the overwrite directly, off a lock-free mirror of the sprite's box
#     (`cursor::live_box_relaxed`) that is readable from inside `wm`'s BlitGuard and the desktop's row
#     loop, where the SPRITE lock may not be taken.
#
#     NOTHING HERE IS FORBIDDEN, and that is a decision rather than an omission.
#
#     `desktop_over` was a FORBID in the arc's first cut, on the reasoning that the render task
#     brackets its own `Screen::flush` (undraw -> pal.render -> repaint) so a live sprite must never
#     be seen by `present_background`. The reasoning is right about the bracket and wrong about the
#     counter. The sprite mirror is deliberately OVER-COUNT-BIASED — `draw_locked` publishes BEFORE
#     it paints, so that the probe can never MISS an overwrite — and the HID router calls
#     `cursor::repaint` from its own core. An arrow arriving while a desktop flush is mid-loop
#     therefore registers a real, healthy, transient overlap. Forbidding it would red a correct metal
#     boot, and a false red costs Peter a bench sitting chasing a bug that is not there (the same
#     trap CURSOR-5's `drain_insession` scoping was written to avoid). It is a VERDICT term
#     (`-> UNBRACKETED`) and a watch-list item instead: a reader who sees it looks there first, and a
#     SUSTAINED count — not a handful — is what would mean the bracket is genuinely broken.
#
#     `present_over` is the metal question this arc exists to ask, so forbidding it would refuse the
#     evidence. `uncover_lost` is printed as `lost/planned` because the fix has a price (each one
#     costs a whole-sprite refresh); it is a number to PRICE, not to fail on.
#
#     The REQUIRE is therefore the whole of the gate's claim: the line is wired and prints. On QEMU
#     every field is 0 and the verdict is UNWITNESSED (no HID pointer, so the sprite is never drawn
#     and the mirror never sets).
#     See docs/dev/OS/08_VIDEO/engine.md §CURSOR-6.
REQUIRE \[cursor6\] rollup scope=.* present_over=.* masked=.* desktop_over=.* mismatch=.* uncover_lost=.* ->

# --- WC-J — a closed window gives its panel rows BACK ------------------------------------------
# ---    P61 (attended): four background vugs, some killed; the operator reported one crash, two
# ---    FROZEN windows and one still running, and `jobs` then showed all four pids exited 0 and
# ---    reaped. The process story was clean, so the frozen windows were pixels, not processes —
# ---    window content still on the panel for owners that were already gone.
# ---
# ---    `[wc-j] vacate` is the single-window half: present a window, prove the panel took its
# ---    colour, close it (once by the explicit `SYS_WIN_CLOSE` path, once by the exit-teardown
# ---    `close_owner` path), and read the vacated box back at five points — content origin, two
# ---    diagonals, title strip, lower border. All five must be DESKTOP_BG byte-for-byte. This half
# ---    passed on the unfixed tip and is kept as the regression floor.
# ---
# ---    `[wc-j] retile` is the half that FAILED there (`old_desktop=false (0/3)`), and it is the
# ---    P61 shape: a real window is never pinned, so the TILER owns its position, and the layout is
# ---    a function of how many windows exist. Closing one window re-tiles the survivors, and the
# ---    closer erased only the box the CLOSED window vacated — never the boxes the survivors
# ---    vacated by MOVING. Before WC-I the desktop's blanket per-tick present and `wm::repaint`
# ---    overwrote those rows within a second; WC-I subtracts the window layer from the desktop's
# ---    damage and drops the blanket re-blit, so the abandoned tile belongs to nobody and stays for
# ---    the rest of the boot. The leg closes one of two tiled windows and requires that the
# ---    survivor MOVED, still reaches the panel at its new box, and left desktop behind at its old
# ---    one. See docs/dev/OS/08_VIDEO/engine.md §WC-J.
REQUIRE \[wc-j\] vacate close_painted=true close_desktop=true .* owner_painted=true owner_desktop=true .* -> PASS
REQUIRE \[wc-j\] retile survivor=.* moved=true painted=true live=true old_desktop=true .* -> PASS
FORBID \[wc-j\] .*-> FAIL

# --- WC-K — the DESKTOP FILL gets the back buffer too -------------------------------------------
# ---    WC-G convicted a SHAPE, not a writer: per-pixel `put_pixel` into the live front framebuffer,
# ---    with no vblank synchronisation, is structurally overtaken by the scan-out (~2x beam overtake
# ---    measured) and latches part-old/part-new. WC-H removed that shape from a window's own pixels.
# ---    `wm::erase` — the desktop-colour fill a close, a move or a re-tile paints over a vacated box
# ---    — kept it: `fill_rect` is `w * h` bounds-checked pokes straight into the memory the HVS is
# ---    scanning. WC-I's own standing note named it as outstanding debt, and WC-J made it heavier by
# ---    routing three more paths (close, close_owner, create_inner retile) through `reclaim`, whose
# ---    first step is that fill — over boxes as large as a whole tile.
# ---
# ---    WC-K stages it, reusing WC-H's machinery rather than inventing a third discipline: the same
# ---    STAGE buffer, the same `try_lock`, the same 4 MiB cap, the same four decline reasons, and the
# ---    same present primitive — bulk `copy_nonoverlapping` runs, one per scanline. Only the composed
# ---    artifact differs: a fill's rows are identical by construction, so ONE row is composed and
# ---    presented `h` times. That is also what lets a full-panel erase (7.6 MB at 1920x1200) stage at
# ---    all instead of declining on the cap — i.e. falling back to the tearing regime for exactly the
# ---    largest boxes, which tear worst.
# ---
# ---    `contig=` IS A LEG, not a comment. The tear-free claim rests on the SHAPE of the present, not
# ---    on the mere presence of a staging buffer: a staged path whose runs fragmented, or overhung
# ---    into the next scanline, would report perfectly good compose/present numbers and still be back
# ---    in the convicted regime. So `stage_fill` CHECKS, per fill, that each run is exactly
# ---    `w * bpp` bytes, fits inside its scanline, and steps by exactly one panel row — and the
# ---    FORBID below stands independently of the timing verdict.
# ---
# ---    DECLINE LINES ARE UNBUDGETED, and that is a deliberate divergence from `[wc-h]`. There the
# ---    successes and declines share one budget, which leaves a boot that starts declining after
# ---    sample 4 silent behind an already-printed rollup. Survivable for a window (which composites
# ---    continuously); not survivable here, because "a direct fill happened" IS this arc's verdict.
# ---    A decline therefore prints whenever it occurs (to a 16-line spam bound), so the FORBID stays
# ---    reachable for the whole boot rather than only until the rollup fires.
# ---
# ---    `scope=fills`, not `scope=boot`: WC-G's lesson repeated — nothing observable inside a boot
# ---    can tell "the erase path is finished" from "the next app has not closed a window yet", so
# ---    the rollup claims only the fills it has seen and the completeness question is the FORBIDs'.
# ---    Witness-feature only. See docs/dev/OS/08_VIDEO/engine.md §WC-K.
REQUIRE \[wc-k\] erase box=.* staged=yes .* contig=yes .* torn=.* -> BUFFERED
REQUIRE \[wc-k\] rollup scope=fills samples=.* noncontig=.* declines=.* -> TEAR-FREE
FORBID \[wc-k\] .*staged=no
FORBID \[wc-k\] .*-> DIRECT
FORBID \[wc-k\] .*contig=no
FORBID \[wc-k\] .*-> SPLIT
FORBID \[wc-k\] .*-> AT-RISK
FORBID \[wc-k\] .*-> UNSTAGED
# --- WC-L: the direct fallback is GONE. P64 (capture pi4-r23s1o) caught WC-K's last resort firing
# ---    twice on focus tab-cycle transitions at ~99% core load --
# ---    `[wc-k] erase box=514x526 staged=no reason=lock -> DIRECT` -- which is the exact
# ---    front-buffer writing shape WC-G convicted and WC-K existed to remove. A fill that cannot
# ---    take the staging lock is now queued as DEFERRED DAMAGE and erased through the staged path
# ---    by the next composite pass, which also re-damages the windows the paint reached (WC-J's
# ---    `reclaim` shape, reused). One frame late is a cost; a torn front-buffer write is a defect.
# ---
# ---    The two FORBIDs above are therefore now unreachable BY CONSTRUCTION rather than by luck:
# ---    no emitter in `wcg` can produce `staged=no` or `-> DIRECT` for `[wc-k]` at all. They stay,
# ---    because they are where a reintroduced fallback would land.
# ---
# ---    `-> DEFERRED` is REQUIRED, not merely permitted: a witness build forces exactly one
# ---    deferral per boot (the one-shot fixture in `wm::stage_fill`), because QEMU has no
# ---    contention of its own and a path whose only witness is a hardware boot is a path that
# ---    regresses between hardware boots -- which is how WC-K shipped a fallback nobody had seen
# ---    fire. The `BUFFERED` line the drain produces one pass later is covered by the REQUIRE above.
# ---
# ---    `staged=drop`/`-> LOST` is a fill that could neither stage nor defer (permanent `geom`/`cap`
# ---    reasons, unreachable on any panel this kernel drives). It feeds `declines=` and so the
# ---    `-> UNSTAGED` FORBID; the explicit FORBID here names the symptom as well as the verdict.
# ---
# ---    `-> STARVED` is the DELIVERY verdict, and it is separate from the tearing one on purpose. A
# ---    deferral that arrives is a latency cost; a deferral that keeps being requeued is a repaint
# ---    that has NOT happened, which on the panel is a dead window's last frame where the desktop
# ---    should be -- the P61 ghost by a new route. `TEAR-FREE` printed over that would be exactly
# ---    WC-K's mistake (a verdict describing the samples it liked rather than the panel), so past
# ---    `E_REDEFER_MAX` requeues the rollup says STARVED instead. It also gets a one-shot
# ---    `scope=starve` line of its own, because the sampled rollup fires at fill 4 and starvation
# ---    by its nature arrives late -- an already-printed rollup cannot retract.
REQUIRE \[wc-k\] erase box=.* staged=defer reason=.* requeued=no -> DEFERRED
FORBID \[wc-k\] .*-> LOST
FORBID \[wc-k\] .*-> STARVED
# --- PULSE-2: the always-running per-core CPU pulse, as an INSTRUMENT PANEL in the standing gap at
# ---    the bottom of the panel -- below the tiled windows, above the PI-UI-2 status line.
# ---
# ---    PULSE-STRIP put it inside the status line itself. On the bench panel `Metrics::for_height(1200)`
# ---    is scale=1, so that band is 12 px and the bars came out ~30x4 -- about a millimetre tall.
# ---    Peter's correction is binding and general: this panel is a TEST-TOOL SURFACE, not a desktop
# ---    imitation, and an instrument gets the room it needs to be read at arm's length. So the pulse
# ---    now owns a band ~1/13 of the panel tall with one labelled bar row per core, and the status
# ---    line goes back to text only.
# ---
# ---    Everything PULSE-STRIP got right is unchanged and still asserted below: no thread (KILLBOUND
# ---    bounds the table at 8), no window (nothing focusable, nothing killable, nothing in the
# ---    z-order), no extra present. WC-I occlusion is still inherited rather than re-implemented --
# ---    the band draws into the `Screen` back buffer and `present_background` subtracts
# ---    `wm::occluders()` from every damaged row.
# ---
# ---    Peter's second directive, once the band existed: "if pulse spans the entire bottom width of
# ---    the screen there will be more leds to show sensitivity. with the better graphics can you have
# ---    a gradient inside each led so it scales super smooth." So each core's bar is a long row of LED
# ---    segments (sensitivity IS segment count, and segment count is width -- which is why the cores
# ---    stack as full-width rows rather than sitting in side-by-side quarters), and the fill is a
# ---    continuous pixel LENGTH with the boundary LED lit in proportion to its coverage, so the meter
# ---    scales smoothly instead of clicking between whole segments.
# ---
# ---    The `armed` line is the creation geometry, and it is the one thing a replay can check about
# ---    the LOOK of a panel nobody can see headless: the reserved band's box, the per-core row pitch,
# ---    the bar's x/width, and the LED metrics. `reserved=` is the number `wm::place` subtracts from
# ---    its vertical budget so no tiled window is laid out over the instrument. All of it derives from
# ---    the panel height and `ui::Metrics`, so a hard-coded pixel would show up here as a constant
# ---    that ignores UNAOS_FB*.
REQUIRE \[pstrip\] armed cores=[0-9]+ panel=\([0-9]+,[0-9]+,[0-9]+x[0-9]+\) row_h=[0-9]+ bar=\(x=[0-9]+,w=[0-9]+\) leds=[0-9]+ led=[0-9]+x[0-9]+ gap=[0-9]+ bands=[0-9]+ full=[0-9]+ strip_h=[0-9]+ reserved=[0-9]+
# ---    The instrument must actually be seated: a zero-width band or a zero-height row is the
# ---    "too small, skipped" degenerate path, and on the gate geometry it means the layout broke.
FORBID \[pstrip\] armed .*panel=\([0-9]+,[0-9]+,0x[0-9]+\)
FORBID \[pstrip\] armed .*row_h=0
# ---    SENSITIVITY FLOOR. The LED count is the whole reason the instrument spans the width; a bar
# ---    that has fallen back to a handful of fat segments is the PULSE-STRIP regression re-entered
# ---    from the other side. Two digits minimum on the gate geometry (the bench panel draws ~140).
FORBID \[pstrip\] armed .*leds=[0-9] led=
# ---    Per-mille full scale, not percent: at ~1400 px of bar a 1% quantum is a 14 px jump and the
# ---    gradient would be stepping under itself. Pin the scale so a revert to percent is caught.
REQUIRE \[pstrip\] armed .*full=1000
# ---    The rollup is the DIRTY-PACING assertion. `samples` counts meter reads (one a second);
# ---    `redraws` counts frames actually drawn and presented. They are deliberately different
# ---    numbers: a second in which no core's load moved a bar segment and the text line did not
# ---    change draws nothing at all. Requiring a rollup whose `skipped=` is non-zero pins that the
# ---    always-running pulse is genuinely paced and not a 1 Hz repaint wearing a flag.
REQUIRE \[pstrip\] rollup samples=[0-9]+ redraws=[0-9]+ skipped=[1-9]
# --- PULSE-3: THE SOURCE, not the pacing.
# ---
# ---    P64, attended, capture pi4-r23s1o. Three vugs held the cores at a sustained 99% and the
# ---    vugband workers churned ~1M context switches a window; the strip printed
# ---    `rollup samples=10 redraws=0 skipped=10` and Peter watched the gradient LEDs sit still.
# ---    Verbatim: "gradient good but pulse not real-time".
# ---
# ---    The dirty test was right and the 1 Hz pace was right. The FEED was wrong. PULSE-STRIP took
# ---    vug's VUG-1 M3b counters (`meter_cpu_ticks` -- CPU_BUSY/CPU_IDLE, bumped once per dispatch
# ---    PASS) and PULSE-2 carried them forward. Those are pass counts, and the scheduler had already
# ---    retired that metric: SCHED-5's own note is "TIME, NOT PASSES ... it counts scheduler
# ---    activity, not CPU time". A core running CPU-bound tasks back to back dispatches at a
# ---    near-constant rate and never reaches the empty-queue branch, so busy/(busy+idle) pins at full
# ---    scale and stays flat while the utilization underneath it wanders. Hence a bar that never
# ---    moved -- and hence `[spinhunt]`'s `load settled c2=53` disagreeing with SCHED's `c2=99%` in
# ---    the same window: two sources, and the panel was reading the wrong one.
# ---
# ---    The strip now reads `sched::core_load().busy_pct_recent` -- the SCHED-5/SCHED-7 rolling
# ---    ~250 ms CNTPCT busy-TIME fraction, the same number `top` and the `SCHED: load` heartbeat
# ---    print, so instrument and console can no longer disagree about one core. `meter_cpu_ticks`
# ---    remains the fallback for a core SCHED-8 reports untracked, which is where VUG-HONESTY's
# ---    PARKED decision lives, so a frozen non-demo core still reads parked and never a fabricated bar.
# ---
# ---    `live=k/n` is the assertion that the strip is ON that feed: k counts cores returning a live
# ---    number. `live=0/n` is the regression exactly -- every core back on the dispatch-pass
# ---    fallback. (k==n is NOT required: a core legitimately outside `run()` is honestly untracked.)
REQUIRE \[pstrip\] src live=[1-9][0-9]*/[0-9]+ quantum=[0-9]+ stepres=([0-9]+px|coarse) mono=(yes|no) (PASS|FAIL|SKIP-GEOM)
FORBID \[pstrip\] src live=0/
# ---    The other half: a real-time source is worth nothing to a meter too coarse to render its
# ---    steps. `stepres=` is what ONE source quantum (1% -> 10 permille) moves the lit length on this
# ---    panel's bar; zero means the display quantizes the feed away and the bars would freeze again
# ---    for a different reason. `mono=` catches a geometry collapsed to a constant fill being read as
# ---    a steady load.
# ---
# ---    A RED MUST NAME THE RIGHT SUBSYSTEM. `stepres` is bar_w/100, so on any panel whose bar is
# ---    under 100 px it is zero for a GEOMETRY reason -- a shrunken UNAOS_FBW, a WC-F reservation
# ---    that grew, a layout regression -- and a blanket `stepres=0px` FORBID would report every one
# ---    of those as a SOURCE regression, i.e. exactly backwards. So the witness refuses to state a
# ---    pixel resolution it cannot attribute: below the bound it prints `stepres=coarse` and verdicts
# ---    SKIP-GEOM, and the geometry FORBIDs on the `armed` line above (`panel=..0x..`, `row_h=0`,
# ---    single-digit `leds=`) are what go red instead. `stepres=0px` is therefore only ever printed
# ---    by a panel wide enough to have resolved the step, where it does mean what this FORBID says.
FORBID \[pstrip\] src .*stepres=0px
FORBID \[pstrip\] src .*mono=no
FORBID \[pstrip\] src .*FAIL
# ---    x86 has no `core_load` and is deliberately untouched by PULSE-3, so there is no live feed to
# ---    be on or off there; the witness reports `live=n/a ... SKIP-ARCH` on that arch rather than
# ---    standing a permanent FAIL in every x86 log. This spec is pi4-only, so SKIP-ARCH must not
# ---    appear here -- an aarch64 boot printing it would mean the cfg gate itself has inverted.
FORBID \[pstrip\] src .*SKIP-ARCH
# ---    `srcdelta=` in the rollup is the replay-visible half of "not real-time": the count of windows
# ---    in which the SOURCE moved, printed beside the count actually drawn. A busy window that reads
# ---    `srcdelta=0` is a stale feed; a window with a large `srcdelta` and `redraws=0` is the dirty
# ---    test swallowing real movement. Neither was legible in the P64 capture, and both are now.
REQUIRE \[pstrip\] rollup .* srcdelta=[0-9]+ rate=
# ---    The busy-loop FORBID. The rate is printed in tenths precisely so this can bite: a rate
# ---    sustained above the strip's own legal ceiling over a rollup window is the strip having become
# ---    a spinner on the render core -- the SCHED-6 regression, re-entered through the pulse.
# ---    PULSE-4 raised the bound 5.0 -> 6.0, because the ceiling moved and the old bound no longer
# ---    had headroom above it. Two independent sources feed rate=: the sample-paced load redraws,
# ---    capped by PSTRIP_PERIOD_MS (4/s at 250 ms), and the status TEXT redraw, which is outside the
# ---    period gate and fires on the composed line's seconds field (~1/s). A busy 10 s window can
# ---    therefore reach exactly 5.0/s legitimately -- a false red on a correctly-paced strip.
# ---    6.0 is deliberately kept as a bound on `rate=` in its full meaning (every present the strip
# ---    causes, whatever drove it) rather than netting the text redraws out: a spinner that repainted
# ---    via the text path would be just as much the regression this catches, and excluding a term
# ---    from the number is how a witness stops measuring the thing it is named after. The margin is
# ---    now 1.0/s over the legal ceiling, so a real spinner (free-running at the event rate, tens/s)
# ---    still trips it by a wide margin.
FORBID \[pstrip\] rollup .*rate=([6-9]|[1-9][0-9]+)\.[0-9]/s
# --- SPINHUNT: a worker thread whose leader exited without joining it must reach a terminus.
# ---
# ---    P61: with several bg vugs launched, killed and relaunched, one core read a SUSTAINED 99%
# ---    while `jobs` listed every vug pid as `exited 0 (reaped)`. Nothing in the process table was
# ---    wrong, because the thing burning the core had no process row: an EL0 WORKER THREAD.
# ---
# ---    `SYS_THREAD_SPAWN` puts several tasks under one address-space slot, and the slot lives until
# ---    the LAST of them exits. Nothing ever made the last one exit. `SYS_EXIT` retired only the
# ---    calling task, so a leader that exited without joining its workers — which VUGGUARD made the
# ---    deliberate behaviour of a killed vug, since joining a non-answering worker parks the parent
# ---    forever — left them running against a parent that no longer existed. A worker whose release
# ---    signal is a yield-poll (`uvug_worker`, and any barrier of that shape) is RUNNABLE, not
# ---    parked: it stays in a run queue and burns its pinned core for the rest of the boot. It is
# ---    also self-sealing — its `THREAD_TABLE` row can only be scavenged after the slot's ASID_GEN
# ---    bump, which needs the teardown the orphan itself prevents.
# ---
# ---    `SYS_EXIT` now terminates the ADDRESS SPACE: un-joined siblings are reaped by the same
# ---    armed-kill machinery a `kill` uses, address-space scoped and owner-less (armed then detached,
# ---    so the last orphan out returns the request slot itself). Nothing is reclaimed early — each
# ---    orphan retires through its own `exit()` and drops the slot refcount itself, so KILLBOUND's
# ---    quiescence-witness discipline is untouched; the fix only makes the 1->0 edge REACHABLE.
# ---
# ---    The leg `bg`s a fixture whose leader spawns two yield-polling workers, waits for both to
# ---    sign in, and exits WITHOUT joining. The positive witness (`2 sibling thread(s) left
# ---    unjoined`) is stated by the leader itself at the only instant it is exactly true — a poller
# ---    on another core can miss that window entirely. The verdict is that the ASID drains to ZERO
# ---    live tasks. A/B with the terminus disabled: `drained=false leftover=2`, and the
# ---    orphan-window load row reads 58-61% on the orphans' core against 0% with the fix. See
# ---    docs/dev/OS/02_KERNEL_CORE/userspace.md SPINHUNT.
REQUIRE \[spinhunt\] SYS_EXIT asid=.* 2 sibling thread\(s\) left unjoined; orphan-reap armed
REQUIRE \[spinhunt\] load orphan-window\(leader gone\)
REQUIRE \[spinhunt\] load settled
REQUIRE SPINHUNT: leader exited status=0 with 2 un-joined yield-polling workers .* drained to 0 live tasks PASS
FORBID SPINHUNT: .*-> FAIL
# --- BG-SPREAD: `bg` parents must be PLACED by load, not stacked on the launcher's core.
# ---
# ---    P62 (attended): four bg vugs, each visibly slower than the last, while the `SCHED: load` row
# ---    stayed flat at c0=51 c1=99 c2=52 c3=0. The meter was right and nothing in it was the bug:
# ---    every launch printed `SCHED: task 'bg-el0' -> core 1 (policy: caller-pinned EL0, no-migrate)`,
# ---    so all four parents shared one core's 100% while c3 idled. Their ELF-2 worker threads already
# ---    spread (`other_online_cpu`); only the parents piled up.
# ---
# ---    CAUSE: `spawn_user_image_bg` inherited `this_cpu()` verbatim from `run_user_image`, where it
# ---    is the sys_spawn CO-LOCATION invariant (the FOREGROUND launcher blocks right after the spawn,
# ---    so the child cannot be dispatched until the parent yields). `bg` does not wait, and EXEC1-M
# ---    already removed the dependence on co-location by publishing the ASID before the spawn — so
# ---    the pin bought nothing and cost the whole spread. It is `CPU_AUTO` (the SCHED-3 least-loaded
# ---    placement the orphan-reaper already uses) now. Placement is still decided ONCE, at spawn:
# ---    EL0 slots stay no-migrate and non-steal-eligible.
# ---
# ---    The leg launches 3 parked, thread-free fixtures, records each parent's chosen core, and then
# ---    kills + reaps all three (table left as found). `distinct >= 2` rather than `== 3`: a
# ---    load-balanced policy may legally reuse a core, and `== 3` would make a correct scheduler flap.
# ---    A/B: on the pre-arc code all three launches run from one witness task on one core, so
# ---    `distinct` is 1 BY CONSTRUCTION and the leg fails on every boot; with CPU_AUTO the rotating
# ---    tie-break in `pick_cpu` gives 2..=3. See docs/dev/OS/02_KERNEL_CORE/userspace.md BG-SPREAD.
REQUIRE BGSPREAD: 3 bg launches over [0-9]+ online cores -> cores [0-9]+,[0-9]+,[0-9]+ distinct=[2-9] \(want >= 2\) PASS
FORBID BGSPREAD: .*-> FAIL

# SPREAD-2 — VUG-PAR band distribution. The `[spread2]` rollup only exists when the image carries the
# `vugpar` feature (UNAOS_VUGPAR=1), which the default suite does not set, so these are FORBIDs rather
# than a REQUIRE: zero hits on a default log, real assertions on a UNAOS_VUGPAR=1 log. Under vugpar the
# rollup reads e.g. `cores 4 bands 60,60,38,60 rows 3755,3149,1781,1939 rpb 6258,5248,4686,3231 ratio 193`.
#
# `ratio` is max/min ROWS-PER-BAND in hundredths, not max/min rows: a core that goes tracked late in a
# window draws fewer bands, and comparing its raw row total would red a perfectly healthy split.
# Normalized, the weights are bounded — headroom runs 100 down to HEADROOM_FLOOR (25), so the fattest
# average band can legitimately be 4x the thinnest and no more. 5x or worse means the weighting
# inverted or ran away (or the `lo == 0` sentinel fired: a core drew bands but no rows all window).
# There is deliberately no `cores 1` tripwire: `nh == 0` exits to the serial path before any rollup,
# so nbands is always >= 2 and such a line cannot print.
FORBID \[spread2\] .* ratio ([5-9][0-9]{2}|[0-9]{4,})
