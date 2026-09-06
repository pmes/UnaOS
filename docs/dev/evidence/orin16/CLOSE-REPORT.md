# orin 16 CLOSE REPORT (2026-09-06) — render7 flew, four probes flew, 13 folds gated; render8 STAGED and ON THE CARD (unflown; orin 17 boots it); the landing is orin 17's step 2

> **STATUS at orin 16 close (15:4xZ): the round is complete on hw-jetson; render8 render8-20260906T1532Z-c24d951 is on the card UNFLOWN; the landing (and this report's final battery section) belongs to orin 17. Written by executor CLOSEDOCS,** Written by executor CLOSEDOCS at 2026-09-06T15:0xZ, before the render8 flight and before the
> landing. The seat finalises it: the tip below moved while this was being written (KEYDOORS-FIX folded
> mid-draft), every "PENDING" line is a live state, and the render8 verdicts are not in it. Nothing here is
> a landing record — that is [`LANDING-REPORT-4.md`](LANDING-REPORT-4.md).

Not a trunk landing on its own: this arc (orin 15 + orin 16) stays on `hw-jetson` until the render8 flight
and the peer-acked `--no-ff` merge.

---

## 1. State at close

| fact | value |
|---|---|
| branch | `hw-jetson` |
| tip at write time | `6aef5227` (`docs/dev: FIXTURE_FLAKES Class 3 — PTRDEAD's backlog leg loses its entry to a foreign consumer`) — **moving**: the tip was `8b696271` when this draft opened and the KEYDOORS-FIX fold (nine commits, `0d76ff24`…`6aef5227`) landed during it |
| `origin/hw-jetson` | `906e3aef` |
| commits owed to origin | **36** (`906e3aef..6aef5227`) — re-count at the seat's final commit |
| arc size vs trunk | **88** commits, `f49ea1e7..6aef5227`, merge-base `671a0334` |
| trunk `main` | `f49ea1e7` (unmoved through the TRUNKPREVIEW check; re-verify with a fresh `ls-remote` at the landing) |
| ledger gate | `unaos/scripts/ledger-check.sh` exit **0** — "105 rows in 2 ledger file(s) + RULINGS" |

**Owed pushes (Peter — the seat never pushes).** One line: `git push origin hw-jetson`. It carries orin 15's
tail and the whole orin 16 round. Peers owe their own: `hw-rmbp` ~14 pushes (rmbp 13's count, incl.
`245760d2` knoboff, `b41739f7` grants, `44c7887b` resolver); `hw-pi4` per pi 7.

---

## 2. What flew today

Two flight documents carry the verdicts; this section is a pointer, not a re-derivation.

### render7 — the full desktop boot ([`FLIGHT-RESULT-render7.md`](FLIGHT-RESULT-render7.md))

Image `render7-20260906T0445Z-7be8155` (hw-jetson `7be81559`), kernel.elf `c6dc3960ef7ae164…`, ELF max
vaddr `0x2f5e40`. One boot, 11:41:29Z → 12:03:48Z, 22.3 minutes on the glass, `/dev/ttyACM1`, butler pid
`756188`. Excerpt `render7-boot1.log` (11650 lines, anchored at the loader's `KELF … max=0x2f5e40` line —
the anchor was mis-filed in `unknown.log`, not lost, and is now first in the committed excerpt).

Headline verdicts: **A15** PASS (pass 5) · **A16** PASS rung 2 (the CCPLEX consumes the TCU RX mailbox, 7
bytes, mailbox left empty) · **A18** PASS (fourth) · **A19** WIRE PASS — and the **pixel leg has since
PASSED too** (`0d76ff24`, [`A19-render7.md`](A19-render7.md): SCREEN0 band 0/60200, pass 2, can-fire proved
both ways) · **A20 routing** PASS (19 presses routed) · **A22** ROW 2 of TCURX-DESIGN §7 · **A25** FLOWN
(R21 satisfied: the `View` menu publishes to the bar and picks work) · **A26** PASS-WEAK · **A27** PASS
(8 of 11 grabs placed; render6's dead-drag signature gone) · **A8** FLOWN WITH A DEFECT (the quarry opens
and lists nothing) · **A17** FAIL, gap stands.

Eleven rows were **opened** by this one boot: A28 (VFS root has no backend), A29→SO1, A30, A31→SO2,
A32→SO3, A33→SO4, A34, A35→SO5, A36 (cites rmbp's SR2, deliberately not duplicated), A37, A38.

Two of them the wire found and the glass did not: **A37** (serial RX double-delivers and reorders — two
readers, no sequencing, no dedup; `tste\r` arrived as `cmd="st"` burst and `cmd="tetste"` paced) and **A38**
(a witness line that quotes another witness token inflates every token count — `quarry_open=4` over three
windows).

One correction from rmbp 12's review is recorded in the report itself: the drag pacer's `fed` vs `applied`
ratio is **designed coalescing, not frame loss** (`fed = admitted + coalesced` closes exactly in all eight
gestures, `composites == moves` in all eight). **No defect row was opened for it** and A27 stays PASS.

Two verdicts are deliberately weak and must stay weak: **A20's instrument was aboard but UNTRIGGERED** (the
pointer never died on this boot, so `dup=0 nobuf=0` is unfalsified, not validated), and **A26** announced
once with one line dropped, so the 256-line census never printed.

### The four probes ([`PROBES-2026-09-06.md`](PROBES-2026-09-06.md))

Four separate boots the same day, in order, same port and butler. Every one passed A15 5/5 and carried zero
`AARCH64 EXCEPTION` lines.

| probe | image | row | verdict |
|---|---|---|---|
| tick1 | `tick1-20260906T0013Z-6139327` | A21 | **PASS** — `tmax=33000` (13.2× threshold), census `passes 1 → 43,561,864`, `el2=0`. Gap #1's first question is YES; `bsprun` became askable |
| sdmmcwrite | `sdmmcwrite-20260906T0137Z-a05c2c8` | A23 | **PASS** — `[sdmmc] write lba=2047 -> OK (512/512 match, 2499 µs)`; flew behind the [`SDMMC-T1.md`](SDMMC-T1.md) adjudication (SAFE-TO-FLY); no EL3 SError, so the FWALL conviction is confined to the vendor pad block |
| net4 | `net4-20260906T0138Z-a05c2c8` | A12 | **measured NEGATIVE** — `distinct buffers-written(count=2)=[0,17,-1,-1]`, `below4g=1`, no alias anywhere in the boot. R19 wording: the placement **failed under these conditions**, the code is KEPT. Two hypotheses refuted as the boot's product: the inbound-iATU alias is ACQUITTED, the un-serviced RDU latch is REFUTED |
| ga10bprobe2 | `ga10bprobe2-20260906T0139Z-a05c2c8` | A24 | **PASS (rung 2)** — `boot0=0xb7b000a1 -> POWERED chipset=0x17b arch=0x17 (Ampere) impl=0xb rev=0xa1`, first direct GA10B identification from UnaOS on metal; restore symmetric, board left as found; rung 3 became askable |

---

## 3. The round — every executor

Nine-executor fleet, three waves, all round executors landed by 15:1xZ. Grants are numbered as they were
asked of rmbp 13 (the support/grant seat this round; rmbp 12 closed after PANELFIX).

| # | executor | finding, one line | fold sha / status | grant | gate | kernel8 |
|---|---|---|---|---|---|---|
| 1 | **FIXPANEL** | PANEL-REVIEW V-1..V-4 all four CONFIRMED and fixed; dismiss split taken in rmbp's shape; `wm.rs` untouched | `42cf16f9` → folded **`453e88b3`** | rmbp 12 GRANT 13:2xZ | check 0 / knob-check 0 / test-arm 0 (`gates-453e88b3/`) | identical `8ff7c1d1` |
| 2 | **PROBERESULT** | the four probe boots scored against their design tables | docs only, folded into **`bd7e13c5`** | — | — | — |
| 3 | **DESKFIX** | A30 root cause: `ARMED` never cleared on close (`pulsewin.rs:508`). SO5 root cause: two sprites at two block scales (`cursor.rs` 9·s vs `pal.rs:382` 9·(s+1)). A38's three self-quoting witnesses fixed | `0fced841` → folded **`42ad642b`**; docs `8d9627f5` | rmbp #2 / B19 GRANTED 14:3xZ — **`pal.rs` hunk REFUSED as written** (callers enumerated; dropping the +1 shrinks x86's cursor and undoes the midden-trails fix), reshaped to converge UPWARD, owner rmbp | gates-42ad642b (incl. knob-armed check) | identical `8ff7c1d1` + falsifier |
| 4 | **NET4B** | NET5 + `UNAOS_NET5=1` ring RE-FETCH probe; **net4's buffer-17 verdict did not follow from its own witness** → A12 re-read | `7e6afde7` → folded **`906e3aef`** | orin lane, no grant | check 0 / test-arm 0 | identical `8ff7c1d1` |
| 5 | **SERIALRX-DEDUP** | A37 mechanism = two masters (SPE and our RBR poll) on one UARTC RX FIFO; fix = park the RBR read while the mailbox is armed (`policy=mbox-only`), `[rxmerge]` witness, ordering rule const-evaluated | `f0b0ddf7` → folded **`1b0e33d8`** + ledger `c2073e85`/`684cfd89` | orin lane, no grant | gates-1b0e33d8 | identical `8ff7c1d1` |
| 6 | **CRYSTAL** | A34 root cause: `crystal.rs fire()` never called `power.rs`'s PSCI hook. SO4 = crystal group flush right | `8a49d6ca` → folded **`c355fe34`** + **`bb513370`** + **`064e0baa`** | rmbp #4a+#4b GRANTED 14:1xZ; pi 7 ACKED 16:4xZ with two fold-time edits (boot-chain wording; leg-4 behaviour change above the `action=stub` labelling) | gates-064e0baa | Pi desktop image **MOVES** (honesty fixes), acked |
| 7 | **MENUBAR2** | A10 root cause = `key_escape` unreachable from the Orin/Pi key drains; SO3 app menu (About/Quit); SO2 alignment; two live defects surfaced in design (`press_at` gated on `LIVE==0`; app box over the brand mark's press corner) | `0829b9c7` → folded **`b768331a`** | rmbp #5 GRANTED 14:2xZ (4 files, `wm.rs` NOT touched); pi 7 ACKED the Pi drain 13:5xZ with a row condition | gates-064e0baa | identical; main.rs 8949 |
| 8 | **WINID** | (a) S4: `SHELL_REOPEN` drained only by `x86_render_service`; (b) NEW: window id reused after a route-dropped close — five id caches held across close, only fbcon's cleared | `c3d8f6b7` → folded **`b19b2865`** + **`99c153ca`** (Pi drain) | rmbp #6 GRANTED 14:1xZ both halves + the Pi-drain main.rs extension; pi 7 GRANTED (b) 14:2xZ | gates-99c153ca (check, knob check, test-arm, x86 test, kernel8, `UNAOS_PIDESK=1 kernel8-test`) | identical **after a re-cut** — the first cut `a discarded first cut` moved it |
| 9 | **WINID2** | the SIXTH holder, `wcg.rs:455 SEAM_WIN`, registered on `wcg::begin` | `dfe3584d` → folded **`049d2a48`** + **`72555685`** | rmbp #6 addendum 14:3xZ; condition closed at the tip 14:5xZ | gates-winid2 — **run 1 RED** (`[wc-g] … -> BLIT`, SO7 class), re-run 2 PASS 119/119; pair recorded in [`MBENCH-FLAKY.md`](MBENCH-FLAKY.md) | identical but **VACUOUS** (witness-gated — see §5) |
| 10 | **ROOTFS** | A28: `/` = native UnaFS, `/fat` = `BlockSource::Default` — the Orin has neither volume; `UNAOS_SDMMCROOT=1` binds both to the card FAT via TegraSd, read-only | `952f48e9` → folded **`45d02b4b`** | rmbp #7 GRANTED 14:2xZ with one symmetry condition (`target_arch = "aarch64"` on the shell.rs same-line cfg, statement before the first `//`) | gates-99c153ca | identical |
| 11 | **GA10B3** | rung 3 (read-only firmware-residue inventory) + rung 3b (first GA10B MMIO writes); clk 236 = `TEGRA234_CLK_GPC1CLK`, `-22` = BPMP argument rejection | `10fac74d` → folded **`5fc5506a`** | orin lane | ledger union expected and taken | identical; ELF max `0x2fac88` in the executor build |
| 12 | **BSPRUN** | arc 2 already landed 2026-08-25 (`ea182855`+`a28879de`) — **the A21 row was stale**; adds `[bsprun] host core=0 el=1 -> HOSTING`, `el0 first-run`, matrix leg `arm-tegra-bsprun-el0` | `8e864463` → rebase `0d4d4358` → folded **`ff983f3f`** | rmbp B18 GRANTED (hunks 7→7, 7→7 + a genuine tail append; P7 trap checked, not assumed; `git apply --check` rc 0) | check 0 / test-arm 0 | identical `8ff7c1d1` |
| 13 | **REBASE** | folded the two mis-based worktrees onto `hw-jetson`; `fold/prtscr` `e3355ad4` MOOT (PRTSCR3 supersedes) | `0d4d4358` used; body citation corrected `syscall.rs:8335` → `sched.rs:3394` | — | — | identical |
| 14 | **PRTSCR-ASYNC** | `a86e3268` — **WITHDRAWN as a fold candidate** | not folded | rmbp 13 16:0xZ **BLOCKING**: `Job::begin` used `mount_program_source`, the call PRTSCR-VOL replaced → would refuse forever on the rMBP | — | — |
| 15 | **PRTSCR3** | PRTSCR-VOL taken from hw-rmbp byte-for-byte; sliced `Job` on the VOL ladder + `Refusal::Vanished`; Orin per-pass cadence | `d12344db`/`6128706f`/`9905ddd7` → folded **`cd533543`**, **`d7eec583`**, **`fc91eef9`**, **`5f8f392f`** + ledger `1fba879f` | rmbp #3b GRANTED 14:2xZ; pi 7 fold edit 14:3xZ (the `usb_backed()` invariant comment) | gates-prtscr3 (check, knob check, WC test 150, test-arm, PRTSCRST test-fat, kernel8) | **MOVES** `8ff7c1d1`→`3f14337c`; value relayed to pi |
| 16 | **KEYDOORS** | read-only audit: IN-RING-ONLY = `quarry::key_route` (`live.rs:1887`, sole caller `aarch64/syscall.rs:13211`) — the quarry at boot cannot be closed or navigated by keyboard; F2 supstate drain, F3 Pi peek TAB-not-Esc; x86-only `instgui` + `drag_cancel` focus-key arm | `KEYDOORS.md` sent to rmbp | — | — | — |
| 17 | **KEYDOORS-FIX** | **F0 NEW and the round's sharpest self-catch**: `b768331a`'s own A10 comment was inserted **mid-line, ahead of** TABKEY's call on the Orin drain → TAB dead text for the whole arc. F1 quarry `key_route` reaches the shell doors; F2 supstate drain + new matrix leg (compiled by nothing before); F3 Pi peek Esc; QUARRYDOOR fixture (real x86 door, go-red proven) | branch tip `78087241`; **folded mid-draft** as `0f6a12d2`…`6aef5227` | **rmbp #8 GRANTED, all four (a/b/c/d); pi 7 acked the Pi behaviours** (`ROUND-QUEUE.md` LANDED entry; tracked in git at `docs/dev/OS/orin-ledger.md` A10 — *"KEYDOORS-FIX folded (orin 16, rmbp #8, pi ack)"*). The `drag_cancel` twin was rmbp's to accept, not orin's to fold. **`accca478`'s nine lines in `video/crystal.rs` are OUTSIDE #8** and have no grant record — the one open ack question, LANDING-REPORT-4 §3.3/§3.4 | to re-run on the fold tip | identical; main.rs 8949 |
| 18 | **TRUNKPREVIEW** | throwaway merge of `8d9627f5` onto `f49ea1e7`: **LANDABLE** | `trunkpreview/PREVIEW.md` | — | full battery, every leg 0 (§6 of the landing report) | `8ff7c1d1` |
| 19 | **STAGE8** | bench-side `stage-render8.sh` (refuses a dirty tree, an unmapped knob, a banner miss, a sha mismatch, a TODO), `KNOBS-render8.env`, `QUESTIONS-render8.md`, `scorers-render8.sh` + can-fire selftest | `~/unaos-bench/scratch/orin16/stage8/` | — | — | — |
| 20 | **SO7 line** (seat + pi 7) | the MBENCH intermittent red narrowed to one mechanism (§5) | `ffcb3788`, `038a825d`, `89a7208b`, `c4991e30`, `8b696271` | — | ledger-check 0 on each | — |

---

## 4. Peer review — the tally

### Grants received (orin asked, peers answered)

| ask | family | peer + when | condition carried into the fold |
|---|---|---|---|
| PANELFIX | `video/winmenu.rs` | rmbp 12, 13:2xZ | applied as measured |
| #2 / B19 | `video/pulsewin.rs`, `video/dock.rs` | rmbp 13, 14:3xZ | A30 granted; `pal.rs` refused, reshaped upward |
| B18 | `arch/aarch64/sched.rs` | rmbp 13 | ordering-only change; P7 trap checked |
| #3b | `video/prtscr.rs` | rmbp 13, 14:2xZ | + pi 7's `usb_backed()` invariant comment (14:3xZ) |
| #4a/#4b | `src/power.rs`, `video/crystal.rs`, `video/menubar.rs` | rmbp 13, 14:1xZ; pi 7 ack 16:4xZ | commit-body path `src/power.rs`; exact cfg prose; two pi fold-time edits |
| #5 | `video/winmenu.rs`, `menubar.rs`, `crystal.rs`, `main.rs` | rmbp 13, 14:2xZ; pi 7 ack 13:5xZ | A10 row names the CLASS; selftest chain order verified at the folded tip |
| #6 (+addendum) | `video/wm.rs`, `fbcon.rs`, `instgui.rs`, `pulsewin.rs`, `quarry/live.rs`, `main.rs`, `wcg.rs` — **not `dock.rs`**, which is #2/B19's file (verified: `git show --stat b19b2865`) | rmbp 13, 14:1xZ / 14:3xZ; pi 7 (b) 14:2xZ | WINID-video.patch = 9 hunks / 5 video files; register the sixth holder or name it unregistered with a reason; ORINWM1 paragraph |
| #7 | `fs/vfs.rs`, `video/shell.rs` | rmbp 13, 14:2xZ | shell.rs same-line cfg symmetry |
| #8 | KEYDOORS-FIX `main.rs` (F0/F1/F2/F3 hunks), `arch/x86_64/syscall.rs`, `video/quarry/live.rs`, `unaos/scripts/knob-hygiene.sh` | **rmbp 13 — GRANTED, all four (a/b/c/d)**; pi 7 acked the Pi behaviours (`ROUND-QUEUE.md` LANDED; `orin-ledger.md` A10) | fold only the F0/F1/F2/F3 `main.rs` hunks; the `drag_cancel` twin is rmbp's. **`accca478`'s `video/crystal.rs` nine lines are outside this grant** — no record found, open at the announce (LANDING-REPORT-4 §3.4) |

### Grants and work given away

SR2's fix ownership went to rmbp (patch cut here, "cut it"); SO1 came back to the focus seat, patch-first;
the aarch64 `drag_cancel("focus-key")` twin went to rmbp as a patch, theirs to cut or accept;
`PI-shellreopen.patch` went to pi 7 and returned as a grant.

### Corrections, both directions

**Peers correcting orin.**
- **rmbp's V-1 shape** — PANEL-REVIEW's V-1 was not just "confirm it": rmbp specified the dismiss split, and
  FIXPANEL took that shape rather than its own.
- **rmbp's `pal.rs` refusal** (§3 row 3) — the SO5 fix as orin wrote it would have shrunk x86's cursor and
  undone the midden-trails fix; converge upward instead.
- **rmbp's PRTSCR-ASYNC block** — one line (`Job::begin`'s mount call) made the whole executor unfoldable.
- **pi 7 on the `strip::key_escape` naming** — rmbp's path-slip tally listed "strip vs crystal key_escape"
  as one of three slips; it was **actually right**. pi 7 supplied the real constraint: `video/strip::key_escape`
  (MENUBAR `adb3b1cd`) is **absent on hw-pi4 until the landing**, so the handler must be named in prose, not
  as a resolvable reference. A landing-lag absence, not a wrong path.
- **pi 7 on `wcg` reachability** — corrected the reading of the knob-off Pi image both seats had been using:
  `pub mod wcg;` is `#[cfg(feature = "witness")]` (`video/mod.rs:72-73`) and knob-off `K8_FEATS` has no
  witness, so **wcg is never in the knob-off image** and an identical `kernel8` there is *vacuous*, not proof.
  `SEAM_WIN` stays a real sixth holder; the row's reason changes to a **mis-attributed diagnostic** in exactly
  the images used to gate and score metal flights.
- **pi 7's SO7 discriminators** — the got/want split (every `got` is a legitimate frame colour), `moved=851`,
  `bad_cache == bad_ram`, and finally the attribution to `wcg.rs:412-419`'s already-documented boot-seam
  concurrent writer. This is what turned an intermittent red into one named mechanism.
- **pi 7 on per-tree floors** — `hw-pi4`'s `f25f1601` spec has REQUIRE=118 (the CHROMEBAND leg), so the
  `kernel8-test` denominator becomes 120 when pi lands. Checked against our own pidesk capture: 53
  occurrences of `[wc-b] rollup … amp=1.00x -> UNBANDED`, so 120/120 will hold.
- **rmbp 12 on counting** — "diffstats are decoration; file identity is evidence" (three counting rules gave
  224/231/213 for one artifact).

**Orin correcting peers.**
- **The store-line siting** — rmbp's instruction to register the sixth holder at wcg's *store* line was wrong:
  `seam_glyph_note` runs in print context under a no-lock/no-print contract. WINID2 sited the registration on
  `wcg::begin`, which dominates both readers at one mutex acquisition per boot. rmbp amended **B23** at 14:5xZ.
- **A21's stale row** — BSPRUN found arc 2 had already landed on 2026-08-25; the row said otherwise.
- **A12's unsupported verdict** — NET4B found net4's buffer-17 conclusion did not follow from its own witness.
- **The F0 comment trap** — orin's own `b768331a` (the A10 fold) inserted a comment mid-line **ahead of**
  TABKEY's call, killing TAB on the Orin drain for the whole arc. Found by KEYDOORS-FIX, restored in
  `dbe97e76`/`0f6a12d2`. rmbp's brand-new GATE-APPEND (B24) **did not catch it** — see §5.

---

## 5. Gate defects found this round

1. **P14 — a cross-tree cross-ref is a latent red** (pi 7, 15:3xZ). An `→ X` in a ledger id cell means the
   gate must resolve it; a row that lives on ANOTHER tree cannot be resolved until the fold. A36 was
   de-arrowed and SR2 cited as prose. rmbp then landed a resolver split (`44c7887b`): shared ids (S/P) RED
   if unresolved, seat-prefixed (SR/SO/SP) DEFERRED — printed, counted, STRICT at the landing — which makes
   the `A36 (→ SR2)` form safe again. The general defect stands: **the gate's strictness and the fleet's
   cross-tree working set were out of step.**
2. **B22 — `arroyo check` from the repo root red-lines its own knob→builder probe** (`BASH_SOURCE`
   unanchored). A **false red**, so it costs a diagnosis every time it fires. Filed by orin as SO6, fixed on
   rmbp as B22 (anchored to `$WORKSPACE_DIR`) — **cross-referenced, not re-filed.** Standing trap until it
   is taken: always run from `unaos/`.
3. **B24 — GATE-APPEND, and its F0 miss.** rmbp built a machine check for the P7 same-line-append trap
   (a statement after the line's first `//` REDs with file:line; refuses at exit 2 if the discriminator
   drifts) — genuinely good, and it ran CLEAN on `d390eb9b` (164 files) and on `1fba879f`. **It does not
   catch F0**: F0 was an inserted *comment marker* placed mid-line **ahead of** a call, not an appended
   statement placed after one. The gate's discriminator is "statement after comment"; the defect class is
   "comment before statement". Same file, same line, opposite direction, invisible.
4. **The 10-minute Bash tool cap is not a red leg.** A gate chain longer than ten minutes dies under the tool
   cap with NO RESULT and exit 1. Read as a failing leg, that is a fabricated red. Rule adopted mid-round
   (first hit `gates-42ad642b`): run gate chains under `nohup` and poll a RESULT file with a Monitor
   until-loop.
5. **SO7 — the fixture's instrument is not blind, it is unheeded.** The `[wc-d]` verifier ALREADY detects and
   reports that the reference moved under it (`moved=` at `wm.rs:6682`, defined at `:6192`; `moved=851` on
   the final run) and then renders `-> FAIL` anyway. The cheap fix is a distinct `-> MOVED` / `-> RESAMPLE`
   verdict when `moved != 0`, so FAIL keeps meaning "content wrong" and both batteries stay strict.
   **Never relax the spec's FORBID** — that papers over invalid samples. `wm.rs` is rmbp's lane (B26); pi's
   spec gains a `[wc-d] moved=` rule only after the fixture emits the verdict.
6. **Ledger-check must be gated on its EXIT CODE, not its printed line.** Two ledger reds were caught AFTER a
   commit this round and fixed by amend (a bench-side path in a cell; an executor sha cited as
   fixed-unflown). Every docs commit now gates on the exit code.

---

## 6. Ledger — rows added and ticked

`docs/dev/OS/orin-ledger.md` (arch ledger) and `docs/dev/LEDGER.md` (shared), both green under
`ledger-check.sh` (exit 0, 105 rows + RULINGS).

**Orin ledger, new this round: A28–A40.**

| row | one line | state |
|---|---|---|
| A28 | the VFS root has no backend (`/: backend error: unafs-mount`) — not a quarry fault | fixed-unflown `45d02b4b` |
| A29 (→ SO1) | closing the console drops the route; the shell pin cannot bring it back | fixed-unflown `b19b2865` |
| A30 | closing the pulse window reopens it immediately (`ARMED` never cleared) | fixed-unflown (DESKFIX `42ad642b`) |
| A31 (→ SO2) | `View` drop-down placement and typeface; also a witness gap (no rect, no font on the wire) | fixed-unflown `b768331a` |
| A32 (→ SO3) | no application-titled menu, so no Quit anywhere | fixed-unflown `b768331a` |
| A33 (→ SO4) | crystal menu inset from the LEFT; Peter wants the group flush RIGHT | fixed-unflown `bb513370` |
| A34 | Restart and Shut Down inert, and `action=real` lies about it | fixed-unflown `c355fe34` |
| A35 (→ SO5) | the pointer sprite grows over the backdrop | open — root cause found, fix is rmbp's (converge upward) |
| A36 | Print Screen wedges the machine 6–9 s | open — owner rmbp; **cites SR2, not duplicated** |
| A37 | serial RX double-delivers and reorders | fixed-unflown `1b0e33d8` |
| A38 | a witness that quotes another witness inflates every count | S6 family |
| A39 | GA10B rungs 3 / 3b | folded `5fc5506a` |
| A40 (→ SO7) | MBENCH reds intermittently under host build load | open — quiet-box run owed |

**Shared ledger: SO1–SO5, SO7** (orin's first `SO`-prefixed rows; `S1`–`S32` are frozen). **SO6 is
deliberately absent** — the `arroyo`-from-root false red was fixed on rmbp as **B22** and is cross-referenced
there rather than re-filed here.

**Ticked, not filed.** **S4** gained instance 2 — `SHELL_REOPEN` metal-confirmed dead on aarch64 (Orin drain
`b19b2865`, Pi drain `99c153ca`, unflown on Pi). **S7** gained the class note: S4 and B11 are one defect
class (the x86 render body does what the aarch64 bodies do not), so **S7 steps 2–3 are a bug fix, not
tidiness** — plus pi 7's clause: the drain is per-body, and because `take_shell_reopen` is take-and-clear
(`dock.rs:498` swap), a step-2 merge that leaves two calls in one body silently makes the second dead.

Ticked from render7 and the probes: A1, A7, A8, A10, A12, A15, A16, A17, A18, **A19 (wire + pixel, pass 2)**,
A20, A21, A22, A23, A24, A25, A26, A27.

---

## 7. What is UNFLOWN, and what render8 asks

Nothing in §3 has flown. Every fold in this round is `fixed-unflown`; render8 is the boot that converts them.
The per-row asks, verbatim from the ledgers:

| row | render8 asks |
|---|---|
| A10 | `[winmenu] dismiss reason=esc` |
| A28 | `ls /` lists the card, and `[quarry] open census … entries=N>0` |
| A29 / SO1 | close the console → press the shell tile → `[dock] shell-reopen drained by=orin_render_service -> REOPEN`, and no id reuse across the close |
| A33 / SO4 | the crystal group flush right on the Orin (rmbp flies rMBP geometry before J1) |
| A34 | SYSTEM_OFF/RESET has never been invoked on this metal — **does BL31 answer, or return?** The SMC return in `[crystal] verb=… -> PSCI …` asks it |
| A36 | four fast Print Screens → OK plus a **named** `InFlight` refusal (first Pi flight still owed, SR2) |
| A37 | burst and paced `tste\r` → 5 KEY each, `[rxmerge] dup=0 reorder=0` |
| SO2 | the drop-down under its title, in the bar's own type |
| SO3 | an app menu with About/Quit that acts |
| SO7 | *not a render8 ask* — a quiet-box battery run before the landing |

Also unflown and not render8's to answer: **A20's instrument** (needs a boot in which the pointer actually
dies) · **A26's strong leg** (needs > 256 dropped console lines so the census prints) · **A23's persistence
proof** (the NEXT boot of the sdmmcwrite image, whose `[sdmmc] write prior lba=2047 = witness(…)` line proves
the bytes survived power-off) · **A12's DHCP half** (untested above the driver: `discover=1 offer=0`).

**render8 flight plan** (Peter, 13:0xZ — ONE boot, cable plugged): the render7 knob line + `UNAOS_BSPTICK=1`
+ `bsprun` + `UNAOS_SDMMCROOT=1` + `UNAOS_NET4=1` + `net5` + `UNAOS_GA10B_PROBE3=2` + every green fold. No
separate net boot; if render8 dies inside the net probe, re-fly minus the net knob.

---

## 8. OWED

1. **Quiet-box battery run** (SO7 / A40). The `kernel8-test` leg reds intermittently at ordinary working load
   (the seat's own red fired at load ~3.9; pi 6's truncation family needed 10.84). It must be re-run on a
   quiet box **before** the landing uses it as a gate. If the baseline rate stays non-zero on a quiet box →
   a load-independent fixture or an explicit flaky-leg allowlist, **never a re-run habit** (rmbp 13). rmbp
   B26 carries the J1 side.
2. **First Pi flight for SR2 and S4.** The sliced capture ships unflown on Pi metal on three ungated
   `prtscr::service()` call sites (`main.rs:1199`, `:1689`, `:5935`), no aarch64 gate reaches the path
   (`selftest_once()` parks in its no-writable-volume arm on QEMU raspi4b), and the Pi has no on-card log.
   The Pi `SHELL_REOPEN` drain (`99c153ca`) is unflown too. The next Pi focus seat looks here first.
3. **rmbp's SO4 and SO5 flights.** SO4 (crystal flush right) is arch-neutral and moves x86 bytes — rmbp flies
   it at 1920×1200 before J1. SO5 (cursor upward convergence, `cursor.rs` → `sprite_scale`) is rmbp's to
   write and fly; the `[sprite] … same=` witness ships on render8.
4. **pi's spec FORBIDs.** The `[wc-d] moved=` rule lands in `pi4-regression.spec` **only after** the fixture
   emits the MOVED verdict. Separately: `pi4-regression.spec:576-577` and `:952-954` assert the same lines
   the Orin battery reds on, so the Pi battery is exposed identically with nothing changed.
5. **The MOVED verdict fixture** (rmbp B26, `wm.rs`) — the §5.5 fix. Prerequisite for 4.
6. **KEYDOORS-FIX fold — ask #8.** Folded mid-draft as `0f6a12d2`…`6aef5227`; the gate chain re-runs on the
   fold tip and the A10 row takes its F0 note. The `drag_cancel` twin stays rmbp's to cut or accept.
7. **The S7 drain collapse.** S7 step 2 merges the render bodies and **must** collapse the Orin and Pi drains
   into one. `take_shell_reopen` is take-and-clear, so two calls surviving in one merged body makes the
   second silently dead (pi 7's clause, owed in the row).
8. **Pixel legs and card hygiene.** A19's band read is **done** (`0d76ff24`, pass 2). Still owed: SCREEN1–4
   for A31 (placement/typeface) and A35 (cursor size), SCREEN5 for A33, SCREEN6 for A29. Tidy the card
   before the next load (FLIGHT.md §A.4) or the next capture is named `SCREEN7.PNG`.
9. **Docs reconciliation at the landing.** `screenshot.md` §10/§11 were written on `hw-rmbp` (one-sided +70 at
   J1) — **do not write them here**; reconcile at the landing. The `hw-rmbp` copy of `LEDGER.md` does not yet
   carry `S28`–`S32`; that union is the landing seat's.
10. **Pushes.** `git push origin hw-jetson` (Peter). Peers owe their own; re-verify with a fresh `ls-remote`
    the same turn before any reachability claim.

---

## 9. Process notes worth keeping

- **The clock.** The seat's Z-stamps from ~13:1xZ to 17:0xZ were about three hours fast (real 13:37Z at the
  `gates-42ad642b` restart). Every message from that point uses `date -u`. Timestamps in this report that
  come from the round ledger inherit that skew where they are quoted as they were written.
- **Never cherry-pick a peer's commit whose file carries a later reconcile.** Take the content delta
  (`git diff <mine>..<theirs> -- <files>`) and gate by sha256 identity of the resulting files (rmbp 13,
  PRTSCR-VOL). Cherry-picking produces conflicts and blind hand-resolution.
- **Every fold commit body is `git grep`-checked on the fold tip before it is written** — three path slips
  this round (`syscall.rs:8335`→`sched.rs:3394`, `arch/aarch64/power.rs`→`src/power.rs`, and one that turned
  out to be right).
- **Every spawn's first command is `git rev-parse HEAD`.** The isolation tool cut two worktrees from `main`
  rather than the branch tip this round.
- **The bare `./arroyo check` compiles no video module.** The knob-armed
  `UNAOS_WC=1 UNAOS_TEGRADESK=1 UNAOS_PIDESK=1 ./arroyo check` is the real type-check for `video/` patches;
  both are required in every fold.
