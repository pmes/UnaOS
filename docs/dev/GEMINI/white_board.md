# WHITE BOARD — 2026-08-06 (GR17, mid-arc)

**Peter's sheet.** What I need from you, right now. Nothing else goes here —
cross-session handoff lives in the baton, per-boot status in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — one push owed, two lane decisions

**1. PUSH OWED** — one batched push covers the whole arc so far (5 commits, `370fa1e0..b935aff8`;
origin is at `59b37373`):

```
git push origin UnaOS-gemini
```

An adversarial review of the M3 commit is still running; if it forces a fix commit, the same
push covers it — I'll update this line if the tip moves.

**2. DECISION — `read_pixel` pays a gratuitous 3× (shared-core `video/framebuffer.rs`).**
It issues three u8 PCIe reads where one u32 fetches all four bytes. One-expression change,
zero coverage change; takes wc-d 2.84 s → ~0.95 s and speeds every future glass read.
Shared kernel-core file, so it is not mine to touch mid-round. Whose lane, and when?

**3. DECISION — the witness-boot per-print tax (~1.29 s, mechanism unattributed).**
Same 229 bring-up lines: 0.69 ms/line witness-off, 6.28 ms/line witness-armed, starting at
`[wc-x] console-route first-paint`. Likely fbcon/serial territory, not `wcg.rs`. Assign it
(here or Gemini or later arc) or park it — it caps any wc-g-only reshape at ~5.8 s kepler.

---

# STATE — two boots staged, bench armed, card is yours

- **Boot P (profiling)**: staged at `flash/gr17-prof/`, kernel `e717e4c4…`, strings-verified.
  Decomposes each 2.87 s wc-g pass into four phases on the wire. Blocked only on media.
- **Boot Q (pay-as-you-go verify)**: staged after the review verdict on M3 — knobs
  `…WITNESS,LOGTS,WCG_PAYGO`; builder plumbing landed so the knob actually reaches media.
- Card carries GR16's witness-ms build (`172a0e07…`) — Boot P needs a new write (on card-in,
  serviced that wake). Waker + media watch armed and verified live; squawk on s73.

# WHAT LANDED THIS SESSION — 5 commits, two findings that move the target

- `370fa1e0` wc-g per-phase profiling (`[wc-g] prof`, `wit_us=` ledger)
- `0a0570cd` analyzer `--wcg` — witness cost decomposed per instrument, selftests green,
  reproduces §10g's numbers bit-identically from the s73 capture
- `0d26303d` §10g correction (below)
- `7551773b` M3 pay-as-you-go reshape (bulk u64 glass reads; `UNAOS_WCG_PAYGO`: lattice pass 1
  + deferred full passes 2–4; x86-gated; under adversarial review)
- `b935aff8` builder knob plumbing

Findings: **98.7 % of each wc-g pass is the glass read-back** (glass byte ≈ 976 ns, pinned
non-circularly via wc-d; full cost model with falsifiable Boot-P predictions in
`~/unaos-bench/scratch/gr17-cost-model.md`). And §10g's "remainder" was real: the **per-print
tax** in Open item 3.
