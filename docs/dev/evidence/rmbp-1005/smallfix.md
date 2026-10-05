# SMALLFIX (B380) — flight 23's five small reds, each cause named on the wire

Branch `exec-rmbp-smallfix`, cut from a60219de. No new kernel file (no CHARTER line owed); the seams are the shared cores.

## Findings (from `flight23/f23-boots.log` and the sources)
1. **KAT2 exit 19** = `syskat2.c` check 19: `ftruncate` shrink 10 -> 5 then grow -> 4103 on `<home>/kat2.tmp`. Flight 23 is the
   first flight on the UnaFS ROOT, so `<home>` is native: `NativeBackend::truncate` answered every shrink `Unsupported` ("UnaFS
   carries no in-place shrink primitive"), the Linux shim's fallback (`sys3::set_len`) then tried truncate-to-0, also refused ->
   `-EIO`. Flight 22's home was FAT (shrink by rewrite worked). Not a futex: the futex timeout of flight 22 is gone.
2. **diag exit=TIMEOUT** printed NOTHING (not even `:: SELFDIAG: start`), so it waited before its first line. DIAG.ELF grew
   263264 -> 308344 bytes since flight 22: merge12 put `vein_ring3::TlsSetup::load()` (the DRBG seed + roots bundle + CCADB
   intermediates parse) at the top of `_start`, before anything is printed, for every run — though diag only needs TLS when it
   sends the key to a TLS endpoint about a FAIL line. Under the six-vug storm that exceeds the 7 s foreground bound.
3. **LUMENCRASH first_line=timeout** is a probe bound, not a crash and not the window table: the spawn succeeded (slot 4, torn
   down cleanly) and Peter's typed `lumen` at 09:24:43 printed `:: LUMEN: start` at 09:24:51 — EIGHT seconds (window, prefs,
   holocron key, the same TlsSetup load, the 1.7 MiB font). The probe allowed 2 s.
4. **Selfbuild silent skips**: the vague lines existed but named no cause ("musl target?"), and `build_selfbuild6_x86`
   `return 0`s after LIB/dyn fails, so the R87 rustc fetch NEVER RAN and nothing said so (LIB/dyn needs clang + ld.lld).
5. **trust-bundle** overwrote `roots.pem.sha256` (and `ctlogs.jsn.sha256`, `SOURCE`) with whatever it fetched.

## The seams (R79)
- M1: the in-place shrink lives in the SHARED UnaFS core `unaos/libs/fs/unafs` (`UnaFS::truncate_data`, both rings link it);
  the kernel's `NativeBackend::truncate` calls it (same inode: the per-object ACL is kept — the reason shrink was refused).
- M2/M3: no kernel seam — diag (ring 3) defers the TLS load to the ask; the probe waits for the real first line.
- M4/M5: build tooling only.

## Milestones
- **M1 KAT2** — `truncate_data` + host test (`cargo test -p unafs --test mutation_logic`); NativeBackend shrink; SYSKAT2 prints its
  fields on failure (`syskat2 fail <id> threads=.. counter=.. futex_waits=.. epoll=.. statx=.. sigreturn=.. checks=<n>`), the
  kernel reports them plus `checks= scratch=`.
- **M2 DIAG** — `[diag] wait on=<prefs|key|boot-last+tree|owners-probe|boot-log|owners+sections|tls-load> ms=<n>` per step; TLS
  loaded only when the plan would send the key over TLS AND there is a FAIL line to ask about; start line says `tls=deferred|unneeded`.
- **M3 LUMENCRASH** — 20 s bound, `first_line=<ms>`; on timeout `waited_ms= bound_ms=`.
- **M4 arroyo** — `SELFBUILDn: not staged <product> reason=<tool missing: X | musl target missing | fetch failed <url> | sha256
  mismatch <url> | build failed <step> | host proof failed <step> | needs LIB/dyn | UNAOS_SELFBUILD6_RUSTC=0>` at every not-staged
  exit, and `SELFBUILD: staged=[..] not_staged=[..]` at the end.
- **M5 trust-bundle** — verify against the COMMITTED pin (git HEAD), the committed SOURCE's own source tried first (the certifi
  wheel, the chromium commit); mismatch -> `trust-bundle: pin mismatch committed=<sha> fetched=<sha> — bundle NOT replaced`,
  exit 1, pin and SOURCE untouched; `--repin` is the deliberate change (then commit).

## Witness lines (the next flight reads)
- `:: LINUXABI-KAT2: threads=4 counter=400000 futex_waits=<n> epoll=ok statx=ok sigreturn=ok checks=26 fail=0 -> PASS ::`
- `[diag] wait on=… ms=<n>` lines, then `:: SELFDIAG: start … tls=unneeded ::` … `:: SELFDIAG: fails=<n> … -> PASS ::`
- `:: LUMENCRASH: spawned=1 first_line=<ms> -> PASS :: bound_ms=20000`
- build log: `SELFBUILDn: not staged … reason=…`, `SELFBUILD: staged=[…] not_staged=[…]`; trust-bundle's pin line.

## Owed
- LUMEN.ELF itself still loads TLS before its first line (8 s under the storm): the same deferral belongs in user-lumen (its
  start line reports `trust=`), a LUMEN arc. A bench with CCADB reachable stages an 8 MiB `inters.pem` that every TLS load parses.
- FAT `truncate` to a non-zero smaller size is still `Unsupported` in the VFS (the shim's rewrite fallback covers it).
- The ctapple / inters pins are not committed, so they still prove only the fetch (said `pin=uncommitted`).
