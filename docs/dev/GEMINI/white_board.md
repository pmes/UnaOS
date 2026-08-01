# WHITE BOARD — GR13, 2026-08-01

**This is a whiteboard, not a record.** Wiped and rewritten every round. Durable copies live in
commit messages, `docs/dev/`, and the track memory.

This round it carries one thing: **what I need from Peter.** Nothing here is a status report —
the status is in `~/unaos-bench/PLAYBOOK-x86.md`, which is delivered as a file every time it
changes.

---

## 1. The kepler runlist block — two defects, and the file isn't mine

**What I need:** leave it to the kepler lane, or grant me the file.

`kepler.rs` is the kepler lane's declared file and `wt/kepler-poke-x86` is unmerged, so I
stopped rather than touch it. Both defects are live on trunk **now**, independent of that
branch, and I verified both against source and capture:

- **The beacon overwrite.** `:485-490` writes three runlist entries; `:540-542` writes
  `0xBEAC0001..0xBEAC0008` over `runlist_off + 0..31`, covering all six words; nothing restores
  them before `:1014-1015` submits the playlist. The chip is handed a three-entry playlist whose
  entries are beacon words.
- **A poll that can never succeed.** `:1024` breaks on `(pl_rd_len & 0xFFF) == 1`, but `:1015`
  submits `LEN=3`, the capture reads `playlist_rd_len=00100003` (`& 0xFFF = 3`), and the
  driver's own witness one line later says `entries=3 OCCUPIED`. So it runs all 100,000
  iterations and issues **200,000 BAR0 reads across PCIe on every boot.**

Option 2 costs one `merge-tree` re-check, which I'd run either way. Option 1 costs nothing but
leaves both on trunk until that lane next runs.

## 2. `bg_place_cpu` — a policy call, not a fix

**What I need:** reserve, spread, or leave.

Since SCHED-X86 the shell *is* `x86_render_service`, so every `bg`/`run` starts its ring-3
program on the render core. Correction to what I said earlier: this is **not** a deadlock — the
storage syscall handler is IF-masked, so it can't be preempted holding `XHCI_CONTROLLER`. It's
an operator-facing placement problem: a foreground `run` degrades the panel for its duration.
`worker_cpu(0)` now exists and is the obvious answer, but which policy is yours.

## 3. Durability — nine commits exist only on this disk

**What I need:** a push, when you're ready.

```
git push origin UnaOS-gemini
```

`origin/UnaOS-gemini` is still `c90599f1`. `git for-each-ref --contains` returns **zero** remote
refs for all seven tips: trunk's two commits, the four arcs, and both GPU-lane branches. The
`wt/` branches are your call; the trunk commits are the ones I'd want backed.

## 4. When do you want the boot?

**What I need:** whether a sitting is imminent.

Staging is gated on that — I don't stage speculatively. Everything else is ready on my side
within minutes of the merge matrix returning. Four instrument families ride this one boot, and
three of the readings are cross-checks that no single-arc boot can produce.

## 5. `serial-analyzer.py` is silently dead on this rig

**What I need:** whether fixing it is in my lane.

It parses **nothing** on an rmbp capture — verified by running it against four real boots:
header printed, zero output, exit 0. Two causes: its boot splitter requires `' MARK '` *and*
`' boot'` in the same line, which is an Orin capture format this bench doesn't emit; and its
witness predicate is `::.*witness.*::`, which misses BPACE, the CCS lines (no `::` at all),
both placement lines, and — worst — GPACE's `OVERLAP` tripwire, the one line that convicts the
instrument. Three edits in one function. It lives in `~/unaos-bench/tools/`, which no brief
has ever assigned.

## 6. The `/on` settle path is inherited, not measured

**Not a question — a standing item for whoever gets a `/on` capture first.**

The settle is now conditional: 150 ms when we energised a port, 100 ms when firmware had already
powered them. Every rMBP port reads `/pre` on all seven captured boots, so **the 150 branch has
never been measured by anyone.** The first `/on` capture from any track is what would let it be
trimmed. Relevant to Pi and Jetson, who inherit this constant — already relayed to the Fox seat.

---

## Answered this round, recorded so it isn't re-asked

- **s58 keyboard silence ≠ Pi PA5c.** Delinked on evidence: PA5c had kb, mouse *and* storage
  never enumerating, with zero port-connect events over a full 30 s budget — controller-level.
  s58 is one silent endpoint on a chain whose sibling streamed throughout. Different layers.
- **`pci-usb d=4620ms` is not 57% of a real boot.** It's bench-media-only; the same tag reads
  113 ms on a default build. I reported the wrong figure and corrected it.
- **CCSMARGIN is n=7, not n=1.** Seven captured boots, byte-identical, all ports `/pre`.
