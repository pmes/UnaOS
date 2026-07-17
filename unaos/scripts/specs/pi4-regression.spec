# pi4-regression.spec — the Pi 4 kernel8 chain.
#   QEMU gate:  ./arroyo kernel8-test 35   → unaos/target/serial-pi.log
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
