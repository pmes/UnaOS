# LUMENFAST (rmbp-ledger B507) — Lumen's first line under a second

Cut from 668cdd95 (the merge19 fold + flight 27). Branch `exec-rmbp-lumenfast`.

## Finding (flight 27, `f27-boot1.log`)
- `[lumen] first_line_ms=17227 … key_ms=1183 … font_ms=15737` and `first_line_ms=14140 … font_ms=12553`.
- **font_ms**: `user-lumen/src/txt.rs` reads three DejaVu faces (`font_kib=1769`) over `SYS_PATH_READ` in
  32 KiB steps (PATH_IO_MAX) — ~57 calls, each one a fresh `vfs_mount_table()` + UnaFS `resolve_path` +
  `read_inode` + `read_data` under `with_unafs` (~250 ms of fixed cost per call on this medium; the kernel's own
  `[kfont] load … font_kib=3341 … ms=5129` read the same files in 256 KiB steps). The parse and the 285
  advances are not the cost. The kernel has held these exact bytes since 08:25:26 (`KERNELFONT faces=10`,
  `Box::leak`ed `&'static [u8]` in `video::text::tt::load`).
- **spawn_ms=6394**: not the ELF load (`start_at_ms` = `spawn_at_ms`+1). It is APPRES's FIRST sight of
  LUMEN.ELF (`[appres] sighted` 08:31:42 -> `sight … attrs=18 on=inode,types` 08:31:48) inside
  `wm::app_name_arm_launch`, run by the fixture AFTER the spawn — six seconds of UnaFS attribute writes racing
  the program's own start (key, tls, history, fonts all go through the same `with_unafs`).
- **key_ms**: two Holocron relay round trips (`[holocron] relay verb=status` 08:31:42, `verb=get -> not-found`
  08:31:43) then the key-file `SYS_STAT` fallback on UnaFS. No split on the wire says which half is the second.

## Seam (R79)
The kernel is the FULFILLER of `SYS_PATH_READ` (selfdiag.rs, CHARTER: Kernel — fulfiller). `/system/fonts/*`
reads already run under the kernel's authority (`principal_for`), and the kernel's KERNELFONT engine holds the
very bytes those paths name. M1 makes the fulfiller answer a read of a face the kernel holds from that
resident copy — the same file, one store (no second font implementation, no new ABI, no new syscall). Lumen
still parses the face with `font_core` (the one engine). A `SYS_PATH_WRITE` to a resident path drops it.

## Milestones
- **M1** `video::text` resident registry (filled by `tt::load`) + `selfdiag::path_fulfil` serves face reads
  from it. Witness: `[kfont] resident path=<p> kib=<n> -> served-from-memory` once per face.
- **M2** LUMENCRASH's spawn step sights the image BEFORE the spawn (`sight_ms=`), so `spawn_ms` is the
  load alone and the sight no longer races the program's start. `[lumencrash] … spawn_ms=<n> sight_ms=<n>`.
- **M3** Lumen's split line carries `key_holo_ms=` (the Holocron round trips; the key-file half is
  `key_ms - key_holo_ms`).
- **M4** `tests lumenfast`: the spawn step re-run with a 1000 ms bound.
  `:: LUMENFAST: sight_ms=<n> spawn_ms=<n> first_line=<n> font_served_kib=<n> -> PASS|FAIL :: bound_ms=1000`.

## Witness the next flight reads
`[lumen] first_line_ms=<under 1000> … font_ms=<under 200> key_holo_ms=<n>`, `[kfont] resident path=/system/fonts/DejaVuSans.ttf kib=… -> served-from-memory`,
`:: LUMENCRASH: spawned=1 first_line=<under 1000> -> PASS`, `:: LUMENFAST: … -> PASS`.

## Owed
- The per-call fixed cost of UnaFS reads (`vfs_mount_table()` rebuilt per syscall, `with_unafs` masked
  polling) — the root under JOBSCAN and HIDSTALL too; not this arc's.
- The shell's `bg` launch (shell.rs, same-line fold) still sights after the spawn; first sight per image per
  volume is still ~6 s of attribute writes, now off the fixture's program-start window only.
- key_ms is split, not cut: the next flight's `key_holo_ms` says whether Holocron or the key-file stat owns it.
- A face rewritten by a path other than `SYS_PATH_WRITE` keeps the resident (kernel-drawn) copy until reboot —
  the desktop draws the same stale bytes, so Lumen and the desktop agree.
