# HOLOCRONROOT (rmbp-ledger B448) — one store root for the secrets store, both rings

Cut from 82319dd6 (exec-rmbp-merge17). ARCHREVIEW F4 (HIGH).

## Finding
Two roots for one store: the kernel's `keyring.rs:61` (`ring_path`) and HOLOCRON.ELF
(`crates/user-holocron/src/main.rs:575`) use `<home>/.config/unaos/holocron`; the host handler
(`handlers/holocron/src/store.rs:33`, `unafs_store.rs:42`) and `holocron_core`'s format doc use
`<home>/.holocron`. Each ring spelled its own literal; nothing named the root once. Flight 22's
`holocron init` wrote its ring under the `.config` root on the rMBP's UnaFS, so the metal has records there.

## The seam (R79: shared-core)
`holocron_core::root` names THE root once: `DIR = ".holocron"`, `RING = ".ring"`, `root(home)`,
`ring_path(home)`, `secret_path(home, ns, name)`, and `LEGACY = [".config/unaos/holocron"]`. Both rings read
it: the kernel's `keyring::ring_path`, HOLOCRON.ELF's `PathStore`, the host `store::default_root` and
`UnaFsStore`. `.holocron` is chosen because it is the format doc's, the host's and SR33's root (the spec
the store was built to); the `.config` spelling was the metal's local choice in B355.

The migration is in the core too (`root::migrate`, generic over two `Store`s, so the host can call it on a
volume the metal wrote): it moves every legacy record ONCE — secrets first, then the ring, then removes the
legacy files. Re-entrant: an interrupted run re-runs (same-bytes ring = same ring); a DIFFERENT ring already
at the new root is a conflict and nothing moves (secrets sealed under another ring key would be unreadable).
AEAD binds `ns`/`name`, never the root, so a moved file still opens. Namespaces come from the caller's
directory listing (`root::namespaces`), filtered by `name::valid` — this carries BTKEYSEAL's namespace too.

## Milestones
- M1 — `holocron_core::root` (+ `Store::remove_ring`, defaulted), host tests on the path and the migration.
- M2 — both rings read it: keyring.rs (ring at either root starts the fulfiller, so the migration can run),
  HOLOCRON.ELF (migrates at fulfiller start), the host store; `tests holocron` names the root.
- M3 — SECURITY.md; this file's tail.

## Witness (the next flight reads)
- `[holocron] root=<home>/.holocron migrate from=.config/unaos/holocron ring=<moved|same|conflict|none> secrets=<n> skipped=<n> -> <ok|partial|conflict>`
  (printed once, only when the legacy root held anything).
- `:: HOLOCRON: serve ring=unafs ... root=.holocron ::` and `:: HOLOCRON: ... root=.holocron ... ::` from `tests holocron`.

## Owed
- the `dotfile | .holocron | B448` row in `unaos/scripts/charter.registry` (seat-held).
- `fs/holocron.rs` (`/HCRON/BTBOND.DAT`, FAT, BT-BOND M1) is a third, non-home store — BTKEYSEAL (B446)'s.

## Result
- M1 `holocron_core::root` + `Store::remove_ring` + tests/root.rs (6 tests); M2 keyring.rs, HOLOCRON.ELF, host
  store/unafs_store read it; M3 SECURITY.md.
- Legs: `cargo test -p holocron_core` 0, `cargo test -p holocron` 0, HOLOCRON.ELF x86 build 0, x86 kernel (the seat's
  metal line) 0, aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet` 0 (tegra leg: keyring
  is not compiled without `lumen`). charter-check 0.
- BTKEYSEAL (B446, exec-rmbp-btkeyseal M1) seals `bt/<addr12>` through HOLOCRON.ELF's bus verbs, so its records land
  in whatever root HOLOCRON.ELF opens; the migration moves every valid namespace (`bt` included, tested).
