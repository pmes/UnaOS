# WHITE BOARD — GR13, 2026-08-01 (refreshed)

**This is a whiteboard, not a record.** Wiped and rewritten every time it changes. It carries
one thing: **what I need from Peter, right now.** Status lives in
`~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — one item

## 1. `git cherry-pick` is blocked by the permission classifier

This is the only thing standing between here and staged media. Denied both chained and
single-commit; I have not tried to route around it.

```
git cherry-pick <sha>
```

**Either:** add a Bash permission rule for it and I fold the fourteen commits singly, verifying
the result against the merge matrix's tree hash — **or** I hand you the ordered list and you run
them, and I verify after.

Everything downstream is mechanical and needs nothing from you until the card goes in:
fold → verify tree → `./arroyo esp-x86` **last** → stage with sha + MANIFEST → re-arm the card
watch → send you the playbook.

---

# ANSWERED — recorded so they are not re-asked

| | was | outcome |
|---|---|---|
| **Kepler runlist ownership** | leave to lane, or grant me the file? | **Granted, scoped**, then **extended** to userd + pushbuffer. Delivered: `wt/runlist-x86` @ `e47aa1bb`, reviewed, all conditions met. |
| **`bg_place_cpu` policy** | reserve / spread / leave | **RESERVE.** Held deliberately until the fold — it needs `worker_cpu(0)`, which exists only on the unmerged placement branch. |
| **Durability** | nine commits on one disk | **You pushed.** Verified by my own fetch. Trunk backed; branch tips still local. |
| **Boot timing** | when do you want it? | **Whenever the jobs are rolled in** — they now are, bar the fold. |
| **`serial-analyzer.py` lane** | is fixing it mine? | **Granted.** Delivered and validated on both an rMBP and a Pi capture. |

---

# CORRECTIONS I OWE, ALREADY MADE

- **`pci-usb d=4620ms` is not 57% of a real boot.** Bench-media only; 113 ms on a default build.
- **CCSMARGIN was never n=1.** Seven captured boots, byte-identical.
- **The `gp_get=0` corruption claim was wider than the evidence.** No in-repo `gp_get=` capture
  and the beacon era ever overlapped — the witness was removed at `51b98bab` (pull 15) and the
  beacons landed at `200be275` (pull 16). Retracted to Fox in writing.
- **KBDWIT's `adv` has no false-clean mode.** I relayed the failure direction backwards. A
  frozen counter gives `adv=0` unconditionally; the rare case is a false *alarm*.
- **The 150 ms settle leg is not blocked on PA6.** The Pi cannot produce that measurement *by
  construction* — firmware always wins the race to PP, so that rig is permanently the `/pre`
  case. It is unmeasurable there, not merely unmeasured.

---

# STANDING, NO DECISION NEEDED

- The **`/on` (energised) settle leg** is inherited, not measured, by anyone. Nothing on this
  bench or the Pi can settle it. It sits at 150 ms — the pre-trim value — so nothing regressed.
- **`unaos/arroyo` has no `-smp` for x86 at all.** The core count lives only in
  `builder/src/main.rs`, and the placement pool now depends on it with zero slack at `-smp 6`.
- Reported and untouched in `kepler.rs`: three mutually inconsistent runlist entry encodings,
  two more count-bounded polls, six untimed `2_000_000`-iteration spins, and the fact that all
  three beacon regions get the **same** eight values — so `beacon SEEN` could never identify
  which structure the mirror window reflects, contrary to pull-16's stated discriminator.
