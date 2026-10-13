# KVMLAT2 — the three KVMLAT stops, fixed (rmbp-ledger B617)

Spec: `docs/dev/evidence/kvmlat/KVMLAT.md` "The stopped changes, exact" (B578, B576, B579). Wire cited from
`docs/dev/evidence/qemu-image26/serial.log` (one KVM boot, image-26 tree @62a3901a8). Arc branch `exec-rmbp-kvmlat2`,
cut from 37adc602. One commit per row, each compiled once on the x86 kernel line (kfeats24); the Orin line once at
the end. Nothing here was run: R107, the seat runs the legs at the fold.

| row | cause (wire) | fix | the wire the fold should read |
|---|---|---|---|
| B578 LOCKREG | `[lock] held-too-long name=users.rs:364 at=users.rs:667 … ms=81` (760), `kdf calibrated iters=266666` (759), `kdf_ms=271` (773): `set_password` held `TABLE` across the calibration, the PBKDF2 stretch and the flush | `fs/users.rs` `set_password`: snapshot row, `seq` and the cached count under `TABLE`; drop it; calibrate (`kdf_measure`, split out of `calibrated_iters`) and hash off the lock; re-take, publish only if `seq` and the row are unchanged (else re-hash, at most 3, then `Volume`) | no `[lock] held-too-long name=users.rs:364 at=users.rs:6xx`; `:: LOCKREG: … -> PASS :: held_max_at=<file>:<line>` with the file not `users.rs` (the flush alone stays under the lock) |
