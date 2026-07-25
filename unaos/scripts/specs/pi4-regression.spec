# pi4-regression.spec — the Pi 4 kernel8 chain.
#   QEMU gate:  ./arroyo kernel8-test 60   → unaos/target/serial-pi.log
#     35 -> 60 at BGRUN-2. The old 35 s was ~10% of headroom over the chain and the margin was
#     MACHINE-DEPENDENT, not fixed: BGRUN-2's leg-3 dwell (2 s, plus STAT.ELF's yield-amplified cost
#     while QEMU's degraded SYS_SLEEP_MS makes it spin) tipped slower hosts past the end, dropping the
#     twelve witnesses that print LAST (K8b-snap, K8c-snapread, K6-migrate, all BANDY-*) — a truncation
#     that reads as a regression in arcs nobody touched. Measured on the arc branch: 24 s and 27 s ->
#     44/54, 30 s/35 s/45 s/60 s -> 54/54 on one host while another host failed at 35; at 60 s the last
#     required witness lands ~40% into the log (line 763 of 1917), so the margin is now ~1.5x the chain
#     rather than a tenth. Do not trim this back toward the chain length — the tail is what breaks first
#     and it breaks SILENTLY.
#   Metal:      ~/pi-serial.log (pi-bench-connect.sh bridge capture)
#
# Metal caveat (unaos-hazards): some real-Pi boots bring up only 3 of 4 cores and
# CAPSTONE self-skips ("capstone skipped (needs >= 3 online APs)") — scheduler-track
# variance, orthogonal to the syscall chain. A power-cycle usually restores 6/6.
# On such a boot the CAPSTONE directives below report as misses; the 23-PASS chain
# and the K1/F2/F3 witnesses must still hold.

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
# ---   exits after 300 auto frames, which is precisely why the bench could not test TAB before — a
# ---   backgrounded UVUG is unfocused, never leaves its auto path, and is gone in seconds. The
# ---   interactive half (TAB between two bg windows) is still bench-only — QEMU has no HID.
REQUIRE BGRUN-ST: spawn->exit->reap PASS
REQUIRE BGRUN-ST: kill mid-run PASS \(pid=[0-9]+, killed — row reaped
REQUIRE BGRUN-ST: persist\+kill PASS \(pid=[0-9]+,
FORBID BGRUN-ST: .*-> FAIL
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
# ---    These REQUIREs assert that the INSTRUMENT RAN, deliberately not what it found: the finding is
# ---    the arc's output, and a FORBID on any verdict would encode a conclusion the chain has not yet
# ---    reached. Read the `-> ` verdict on the `[wc-g] verdict` line, and the per-sample lines under
# ---    it, as data. Witness-feature only. See docs/dev/OS/08_VIDEO/engine.md §WC-G.
REQUIRE \[wc-g\] win=.* fbbad=.* slow=.* ->
REQUIRE \[wc-g\] verdict samples=.* frame_us=.* ->
