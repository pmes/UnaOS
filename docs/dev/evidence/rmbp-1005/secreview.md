# SECREVIEW (rmbp-ledger B442) — design

**Finding class.** An adversarial read of the tree cut at merge17 (4ead840a) against `docs/SECURITY.md`: the ring
boundary (both `syscall.rs`, every bus verb body), `unsafe` in the files this wave touched, R99's sacred boot FAT,
R94's root permission model, and the Wi-Fi firmware path. Findings, severity and fixes:
`docs/dev/review/SEC-2026-10-06.md`; one hardening-ledger row each in `docs/SECURITY.md` (§SECREVIEW).

**Seam.** No new store and no new kernel file. Patches land in the owners' files: `video/metrics.rs` (the shared
surface helper the five leaked-surface modules call), `fs/attrsys.rs` (the ONE attribute fulfiller both rings and
both transports call), and host-side fuzz modules in the shared cores (`una-abi`, `prefs_core`) both rings link.

**Milestones.** M1 the leaked-surface overwrite (F1, patch). M2 the system-tree attribute guard (F2, patch). M3 the
bounded host fuzz of every ring-3 body parser (una-abi, prefs_core). M4 the review file and the ledger rows; the
larger findings named as arcs for the seat.

**Witness.** F1 and F2 add no boot line (R80). The next flight reads them as: Settings > Display, choose a smaller
"Looks like" mode, raise a toast/alert, choose the native mode back, raise one again — no `KERNEL PANIC`, no heap
fault, `[settings] display mode=… applied=1` then the toast paints; and a ring-3 `SYS_ATTR_SET` on
`/system/types/<mime>` answers `-EACCES` (the `tests attr` leg is unchanged: its probe lives under `/home` or `/`).

**Owed.** BOOTFATSEAM, PREFSCAP, FWPIN (arc rows in the review file); the remaining ring-0 parser audits SECURITY.md
already lists as open (network, USB descriptors, FAT) were not re-read here.
