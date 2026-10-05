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

## M6 STACKGUARD (added by the seat from MP3HANG's finding)
- **Finding**: every x86 kernel stack is a heap `Box<[u8]>` with a 4 KiB POISONED guard (RENDSTACK), not an unmapped page.
  audio_core's 92,504-byte open frame on the render task's 32 KiB stack: the stack probes stepped every 4 KiB straight
  through the poison into the heap below — no fault, flight 23's silent MP3HANG.
- **Seam**: kernel-by-ruling (scheduler + paging). New file `arch/x86_64/stackguard.rs` (CHARTER: Kernel — driver);
  `memory::stack_guard_page` splits the covering 1 GiB / 2 MiB identity leaf into an identical-mapping table (same PA,
  WXN_LEAF_CARRY bits, PAT moved to its 4 KiB bit) and clears ONE 4 KiB entry's P bit. RENDSTACK's three reasons:
  the page is re-mapped in `Task`'s drop before the slab is freed (the allocator's free-list node); the split keeps the
  device-visible map identical; remote cores drop global entries at their next timer tick (`tlb_sync`, a generation
  compare) — a stale remote entry can only map the page, never fault a live one.
- `STACK_GUARD` 4096 -> 8192 (usable sizes unchanged): a whole page-aligned page always lies inside the span; the rest
  stays the poisoned absorber `guard_state` reads (only the absorber is read now).
- Fault side: a CPL-0 #PF with CR2 in the current task's guard page, or the #DF it escalates to when the frame cannot be
  pushed (IST), prints `[stack] OVERFLOW task=<name> stack=<lo>..<top> fault=<addr> rip=<addr> via=<pf|df> -> task halted`,
  re-lays the absorber and `sched::exit`s the task on a fresh frame at its slab's top. Serial: a bounded wait for the
  lock, then the lock-free panic-mode path if the dead task held it.
- Boot: `[stack] guards armed tasks=<n> page=4096` once at the BSP's join; `tests stackroom` prints
  `[stack] room task=<name> high=<n> of <size> left=<n> guard=<page>` per live task and
  `:: STACKROOM: tasks=<n> armed=<n> live=<n> failed=0 page=4096 -> PASS ::`.
- **Owed**: AP boot stacks (static array in smp.rs), the BSP firmware stack and the IST stacks are not guarded; a task
  that overflows while holding a lock still holds it (the line names the task); remote detection lags up to one tick.
