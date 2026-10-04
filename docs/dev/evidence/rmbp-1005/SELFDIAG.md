# SELFDIAG — the smart installer reads a boot and asks Vein for the fix (rmbp-ledger B324)

Branch `exec-rmbp-selfdiag`, cut from 8c750d43 (merge11: LUMENAPP's `vein_core` + `vein_ring3`). Ruling R82
(Peter: "the smart installer will need to use vein in order to auto-diagnose so like with this GPU issue
when self-hosting you can make changes and reboot until the driver works"). ROADMAP §1c SH-4. Knob
`UNAOS_SELFDIAG=1` (feature `selfdiag`).

## Design

**Finding (B324).** The loop R82 describes has four missing pieces: (1) a boot's verdicts live only on the
serial wire, nothing on the machine can read them after the boot; (2) no program on the metal pairs a
failing witness with the source that prints it; (3) nothing turns a model's answer into a change of the
selfhost tree; (4) the rebuild and reboot need a native toolchain (SH-5) that does not exist.

**Seam.** Three owners, one per piece.
* The BOOT LOG is the kernel's (kernel-by-ruling: R82 names it; it is the kernel's own wire): `bootwit.rs`
  keeps every `:: TAG: … -> PASS|FAIL|SKIP ::` line and the `:: BOOT:` line of the boot in a bounded
  static ring (a tap at the head of `serial_line::emit_src`, beside QUIETBOOT's tag tally) and writes it
  to `/var/log/boot.<n>.witness` on the UnaFS root (root stats with an inode id) at desktop-ready (from the
  main loop, after the `:: BOOT:` line marked it) and again at `reboot`/`shutdown`. The last 8 boots are
  kept (`boot.<n-8>.witness` is unlinked), `/var/log/boot.last` names the newest `n`. Nothing is printed
  (R80). On FAT root it writes nothing.
* The DIAGNOSIS is the installer ensemble's (CODEX: Vein diagnoses, Principia owns the loop's limits): a
  ring-3 program `APPS/DIAG.ELF` (console, app note flags 0) linking Vein as a library — `vein_core` +
  `vein_ring3`, the LUMENAPP shape — and a new pure `no_std` crate `unaos/libs/sys/diag_core` (host
  unit-tested) holding everything that is not I/O: witness parsing, the owners table, the prompt, the
  unified-diff parser, the hunk locator (exact old side, offset fuzz ±3 lines, refuse on mismatch), the
  streaming patch emitter, the record, the Echo fixture. The kernel fixture links the SAME crate.
* Ring 3 reaches the volume through two new syscalls the kernel FULFILS over the VFS (the ATTRSURF
  pattern): `SYS_PATH_READ` (59) and `SYS_PATH_WRITE` (60), whole paths up to 255 bytes, 32 KiB per call,
  under the caller's principal; the selfhost tree (`/boot/SRC/`, `/SRC/`) is written with the kernel's
  authority (the installer's grant, path-scoped: FAT's volume principal is the kernel). Knob off they are
  not dispatched (ENOSYS).

**Milestones.**
- **M1 BOOT LOG ON DISK** — `bootwit.rs` (capture ring, writer, 8-boot rotation, `boot.last`); hooks in
  `serial_line::emit_src`, `bootpace::boot_line`, `power::{reboot,shutdown}`, the main loop.
- **M2 DIAG.ELF** — `crates/user-diag` → `APPS/DIAG.ELF` (ELF window, the LUMEN link shape: TLS needs it).
  Reads `/var/log/boot.last` → `boot.<n>.witness`, selects the FAIL lines, pairs each with its owner from
  `/system/witness-owners.txt` (else `/boot/system/witness-owners.txt`) — a table `TAG<TAB>path<TAB>line`
  generated at build by `scripts/witness-owners.py` from every `":: TAG:` site under
  `unaos/crates/kernel/src` and staged by the builder as `system/witness-owners.txt` on the ESP and data
  trees — reads the owner's section (±40 lines around the site) from the selfhost tree that `src extract`
  materialised at `/SRC/` on the FAT boot volume (`/boot/SRC/<repo path>`; `git archive` paths, so
  `unaos/crates/kernel/src/...`), and asks for one unified diff. No tree ⇒ it says so and names the verb:
  `src extract`.
- **M3 APPLY + RECORD** — the diff is applied to the selfhost tree (resolve every hunk of every file
  first; any mismatch refuses the whole answer; then stream each file to `<file>~dgn`, copy back, unlink);
  `/var/log/diag.<n>.md` records the prompt, the answer, applied/refused and a `fails:` line the NEXT boot's
  kernel reads to append `## next boot <m>` with each tag's new verdict (`fixed` / `still-failing` /
  `unseen`). Wire: `:: SELFDIAG: fails=<n> asked=<n> patched=<n> refused=<n> -> PASS ::`. The rebuild and
  reboot are SH-5: `diag` prints the host command the bench runs (`[selfdiag] rebuild on the bench: …`);
  `diag --loop` is OWED until SH-5 (a program has no argv yet either).
- **M4 ECHO FIXTURE** — the Echo provider, given the canned FAIL line `:: SDFIX: probe=1 -> FAIL ::`, answers
  a canned diff (`diag_core::fixture`). `tests selfdiag` runs the whole pipe in the kernel over the VFS with
  no network — witness text → FAIL select → owner → prompt → Echo → parse → locate → apply → verify bytes →
  record — and prints `:: SELFDIAG: provider=echo fails=1 patched=1 -> PASS ::`.

**Witness lines.** Kernel fixture: `[selfdiag] fx root=<unafs|fat> dir=<d> prompt=<bytes> answer=<bytes>
hunks=<h>` then `:: SELFDIAG: provider=echo fails=1 asked=1 patched=1 refused=0 bootlog=<written|fat-root|none>
-> PASS ::`. Program: `:: SELFDIAG: start provider=<claude|echo> boot=<n> tree=<path|none> ::`, per FAIL line
`[selfdiag] fail tag=<T> owner=<path>:<line>`, then the verdict line above.

**Stays owed.** SH-5 (rebuild + reboot on the metal, so `diag --loop`); program identity for the
path-write grant (any ring-3 program may write the selfhost tree today — the grant is path-scoped, not
program-scoped; Holocron/installer identity); the aarch64 DIAG image (no ELF window there); the metal boot
(R78); certificate verification (inherited from LUMENAPP).
