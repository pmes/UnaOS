# CLICKDEAD — the pointer is not mis-routed on render6, it stopped reporting

**Question (orin 15 brief).** On the render6 boot the pointer is dead after the EL1 drop: no button,
no motion, all boot. Who re-queues the mouse interrupt-IN read after each report, and why does that
stop (or never start) while the keyboard's does not?

**Answer in one line.** Nobody had to: the pointer endpoint delivered exactly ONE transfer event for
the whole boot, so the re-arm loop never ran a second time. The failure is in the xHCI pointer
pipeline, upstream of every consumer — the `orinclick` routing, `wc_click_route`, the cursor path
and the window layer are exonerated by the driver's own ungated witness. **Hypothesis (a).**

Evidence excerpt: `render6-boot1.log` (3944 lines, PURE, anchor `KELF max=0x2db188`).
Control: `../orin14/render4-boot1.log` (1879 lines, anchor `KELF max=0x2d4400`).
Every count below was measured in this worktree with the command shown; logs read with `awk`, never
bare `grep` (control bytes).

---

## 1. The control table — render4 vs render6, measured

`L4=docs/dev/evidence/orin14/render4-boot1.log`, `L6=docs/dev/evidence/orin15/render6-boot1.log`.

| what | command (`awk`, run on each log) | render4 (control) | render6 |
|---|---|---|---|
| decoded pointer reports (witness lines) | `awk '/MOUSE-1: [0-9]+ reports/' L \| wc -l` | **49** | **1** |
| highest report count reached | `awk '/MOUSE-1: [0-9]+ reports/' L \| tail -1` | `1536 reports, last dx=4 dy=-1` | `1 reports, last dx=0 dy=0 buttons=0x00` |
| pump saw a pointer | `awk '/JD20/' L \| wc -l` | 19 (`pointer live (relative mouse…)` + 18 edges) | **0** |
| button edges at the pump | `awk '/JD20 . pointer BUTTON/' L \| wc -l` | **18** | **0** |
| cursor composed | `awk '/cursor3. rollup scope=live planned/' L \| tail -1` | `planned=434 offers=441 taken=411 … -> COMPOSED` | **no such line** (only `present tail=repaint offers=0 taken=0 -> BRACKETED`) |
| HID interrupt-IN errors | `awk '/interrupt-IN error/' L \| wc -l` | **0** | **0** |
| `orinclick` present | `awk '/orinclick/' L \| wc -l` | 0 (knob absent — A20's recipe gap) | 47 census + 1 arm |

The two boots enumerate the same tree, in the same order, with the same codes:
`HUB slot 3` (HS, 4 ports) → `slot 4` `1c4f:0034` on hub port 2, `POINTER INTERRUPT IN EP FOUND:
0x81, MPS: 4, Interval: 10, RELATIVE boot-mouse (proto 2)`, `Configure-Endpoint` `Code=1`,
`SET_PROTOCOL(boot) OK for boot-mouse (slot 4, iface 0)`; `slot 5` `1c4f:0002` keyboard + absolute
pointer on hub port 4. Both logs carry `HUB slot 1 status-change Configure-Endpoint code 17` and
`HUB slot 3 status-change Configure-Endpoint code 8` (ledger B2) — **identical in the control**, so
the hub SCE failure is not the discriminator.

Both boots also print `:: MOUSE-1: 1 reports, last dx=0 dy=0 buttons=0x00 ==` at the same place —
`L4:285`, `L6:286`, immediately after `>>> HID SET_CONFIGURATION COMPLETE <<<` and *before*
`SET_PROTOCOL(boot)`. That is the device's spontaneous idle report answering the first armed read.
It is the LAST pointer report render6 ever decoded; in render4 the count reaches 32 at `L4:645` and
1536 by `L4:1641`.

The keyboard on the same hub, the same event ring and the same pump kept working across the drop on
render6: `:: tegra: JD2 — KEY 's' ::` / `'t'` / `0x0d` at `L6:821-823`, and at `L6:3806-3813` the
Print Screen (HID 0x46) decodes and 6.9 MB of PNG go out over BOT. So `poll_events` ran, the event
ring drained, and the storage path was alive after the drop.

---

## 2. The mechanism, with file:line

All line numbers are `unaos/crates/kernel/src/…` at `8ab82761`.

### 2.1 The counter that convicts the pipeline and clears the consumers

`drivers/xhci/mod.rs:4770-4781`:

```rust
let n = self.slots[slot_id as usize].mouse_report_count.wrapping_add(1);
self.slots[slot_id as usize].mouse_report_count = n;
if n == 1 || n % 32 == 0 {
```

This is an **unconditional** `serial_println!` — no `#[cfg]`, no knob — and it sits INSIDE the
driver, at `mod.rs:4772`, **after** the decode and the `crate::pal::push_pointer_report(...)` call at
`mod.rs:4738-4746` but before the read is re-armed at `mod.rs:4786`. Its counter is bumped once per
decoded, non-dup pointer report.

It printed `n = 1` and never printed `n = 32`. Therefore **fewer than 32 pointer reports were
decoded in ten minutes with a hand on the mouse**. Nothing downstream of `mod.rs:4738` can suppress
that counter: not the `orinclick` gate, not `orin_click` (`arch/aarch64/display_tegra.rs:1309`, whose
`CLK_BTN.fetch_add` at `:1314` is the first statement of the body with no guard before it), not
`wc_click_route`'s owner/focus arms (`arch/aarch64/syscall.rs:14290`, `:14402`, `:14463`), not
`wm::hit_test`, not the cursor's `sprite_plan`/`overlay_open` admission (`video/wm.rs:4978`,
`:5155`), not the 64-deep `EVENT_QUEUE`. The consumer half is exonerated by construction.

Corroboration from the consumer side, in the same direction: `[cursor3] present … offers=0 taken=0`
all boot means `super::cursor::sprite_plan()` returned `None` (`wm.rs:4978-4988`), which is what
`pal::cursor` does when it has never been moved — a cursor with no reports, not a cursor that was
refused.

### 2.2 Who re-arms, and why the keyboard's arming is not "elsewhere"

There is no separate keyboard arming mechanism. `queue_mouse_read` (`mod.rs:14846-14884`) and
`queue_keyboard_read` (`mod.rs:14886-14915`) are structural mirrors, called from mirrored sites:

| when | pointer | keyboard |
|---|---|---|
| enumeration (`SET_CONFIGURATION` complete) | `mod.rs:4310` | `mod.rs:4288` |
| after a decoded report (success/short) | `mod.rs:4786` | `mod.rs:4956` |
| dup-guard's pipeline-preserving exit | `mod.rs:4596` | `mod.rs:4809` |
| non-halting error completion | `mod.rs:4238` | `mod.rs:4250` |
| halted-endpoint recovery | `mod.rs:14837` | `mod.rs:14840` |

`:: tegra: JB2b — keyboard ARMED (slot 5, root port 6) -> PASS ::` (`L6:307`,
`arch/aarch64/piusb.rs:2634`'s tegra twin) is an enumeration *report*, not a second arming path; the
companion line `:: tegra: JB2b — HID: 1 keyboard(s), 2 pointer(s) armed ::` (`L6:356`) is printed on
both boots and says the pointers were armed too.

There is **no `#[cfg]`, no runtime knob, no owner check, no focus check and no desktop check** on any
pointer *control-flow* path that the keyboard path does not share. The only pointer-only gates in
the file are print-only: `mod.rs:4613` (`usbdebug` raw-report dump), `mod.rs:4731` (`usbdebug`
`[hidkeys] button` edge), and `mod.rs:14730-14731` — the whole body of `piusb39_witness`.

So the re-arm loop is `report → decode → queue_mouse_read → doorbell → next report`. It ran once.

### 2.3 The three silent exits, and which one is left

Between "a transfer event for the pointer DCI arrives" and "the read is re-armed" there are exactly
four ways out, and only three of them are silent on a build without `usbdebug`:

1. **Non-1/13 completion, non-halting** → `hid_error_witness` + re-arm (`mod.rs:4229-4241`).
   `hid_error_witness` (`mod.rs:14748-14768`) is **explicitly ungated** — "not knob-gated — a halted
   pointer is a real fault worth one line on any boot" — rate-limited to one line per 500 ms with a
   `[+N suppressed]` tail. **Zero such lines in render6.** Not this.
2. **Non-1/13 completion, HALTING (codes 2/3/4/5/6)** → no re-arm; queued to `hid_halt_pending`
   (`mod.rs:4231-4234`) and drained only by `service_hid_halts` (`mod.rs:14779`), reachable only
   through `service_hid_setproto` (`mod.rs:12874`). Same witness as (1) fires first
   (`endpoint HALTED, queued for un-halt recovery`). **Zero such lines.** Not this either — but see
   §4, because the *hole* is real.
3. **The dup-Success guard's `param == mouse_prev_phys` arm** (`mod.rs:4586-4599`):

   ```rust
   if slot.mouse_expect_phys != 0 && param != slot.mouse_expect_phys {
       let prev = slot.mouse_prev_phys;
       …
       if param != prev && have_buf {
           MOUSE_DISCARD_REARM_COUNT.fetch_add(1, Ordering::Relaxed);
           self.queue_mouse_read(slot_id as u8);
           Self::piusb39_witness("guard");
       }
       return;
   }
   ```

   When `param == prev` (the known Panther-Point duplicate) the completion is consumed, **the read
   is NOT re-armed** — by design, because a fresh read is supposed to already be queued — and on a
   build without `usbdebug` **nothing is printed** (`piusb39_witness` is `#[cfg(feature =
   "usbdebug")]`, `mod.rs:14730-14731`; render6's feature set is `witness,ehcihid,holocron,tegra,
   orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,deskcascade` — no
   `usbdebug`). The same silent no-re-arm also covers `!have_buf`
   (`mouse_data_buffer`/`mouse_ring` gone) and, one level up, the `if let Some(data_buf_ptr) =
   slot.mouse_data_buffer` at `mod.rs:4601` with no `else`.
4. **No transfer event at all** — the controller stops posting for that DCI. Also silent.

(1) and (2) are refuted by the wire. **(3) and (4) are what remain, and neither leaves a mark on
this build.** That is the whole finding: render6 could not distinguish them, because the driver's
accounting for exactly this question is `pub` and bumped unconditionally
(`MOUSE_REARM_COUNT`, `MOUSE_DISCARD_REARM_COUNT`, `MOUSE_ERROR_REARM_COUNT`, `mod.rs:2373-2386`)
while only its **print** is knob-gated.

---

## 3. Which hypothesis the evidence picks, and what falsifies the others

### (a) the mouse read TRB is queued once and never re-armed — **PICKED**

Positive: exactly one decoded report at enumeration and silence for 477 s of hand-on-mouse; the
keyboard, sharing the hub, the event ring and the pump, keeps delivering across the drop; zero error
witnesses, so no completion with a non-1/13 code ever reached the pointer dispatch.

**Falsifier:** a `:: MOUSE-1: 32 reports …` line on a boot where `btn=0`. That would put decoded
reports above the witness threshold with no click routed and move the fault downstream. Absent.

Refined to two sub-mechanisms that render6 cannot separate: **(a1)** completions arrive and the
dup-guard's `param == prev` arm at `mod.rs:4594` consumes them without re-arming; **(a2)** no
completion is posted for the pointer DCI at all. §5 is the instrument that separates them,
and patch v2 splits **(a1)** once more — `DUP-DROP` (the guard ate a recognised duplicate,
repair in the guard) vs. `NOBUF-DROP` (`mouse_data_buffer`/`mouse_ring` gone, repair in the
slot's soft state) — because the two need opposite fixes. §7.2.

### (b) reports arrive but are decoded by a path that never reaches `CLK_BTN`/the cursor — **REFUTED**

`mouse_report_count` (`mod.rs:4772-4773`) is bumped inside the driver, before any consumer, and its
witness is an unconditional `serial_println!`. It stayed at 1. A cfg gate, an owner check
(`syscall.rs:14402`), a focus check, a `desktop_firmware` arm (`syscall.rs:14299`, `:14400`,
`:14401`) or a full `EVENT_QUEUE` can each suppress `CLK_BTN` — none can suppress that counter.

**Falsifier for the refutation:** find any early `return`/`continue`/`?` between the decode entry
(`mod.rs:4601`) and the counter (`mod.rs:4772`) that could consume a report without bumping it.
Scanned `mod.rs:4600-4790`: there is none. The only exits are *before* the decode (the dup guard at
`:4599` and the `if let Some(data_buf_ptr)` at `:4601`), and both are hypothesis (a).

### (c) the mouse slot was reset / evicted after the first report — **REFUTED**

Every teardown path that could clear the slot prints, and clears `mouse_report_count` to 0
(`Slot::reset_soft_state`, `mod.rs:2991`) — so the very next report would print `1 reports` again,
which never happens. Specifically absent from render6 after `L6:303`:

- a second `:: MOUSE-1: HID pointer detected …` for slot 4 (would follow any re-enumeration, from
  `mod.rs:4300-4307`);
- a second `>>> HID SET_CONFIGURATION COMPLETE <<<` for slot 4;
- any `HUB slot 3 port 2:` line after enumeration (`service_hub_changes` →
  `dispose_downstream_slot`, `mod.rs:13873`, `:13974`, `:14017`, `:14063`);
- `xHCI: no driver … releasing port` for slot 4 — the one such line in the log, `L6:333`, names
  slot 6 (`13d3:3549`, class 0xE0), and that path calls `start_next_port()` only, no teardown
  (`mod.rs:4526`);
- any `recover_enumeration` / `command-failed` / `ep0-transfer-failed` line.

And slot 5, the OTHER child of the same hub, kept its keyboard alive all boot — a subtree teardown
would have taken both.

**Falsifier:** any of the five line shapes above appearing after `L6:303`. None do.

---

## 4. A separate, code-proved hole this investigation found (recorded, NOT convicted)

On tegra, a **halted** HID interrupt-IN endpoint can never be recovered after the EL1 drop.

- `service_hid_halts` (`mod.rs:14779`) is the only un-halt path (Reset Endpoint + Set TR Dequeue +
  device `CLEAR_FEATURE(ENDPOINT_HALT)` + re-arm). Its single caller is `service_hid_setproto`
  (`mod.rs:12874`).
- Every tegra post-drop pump calls `poll_events()` and **nothing else**: `main.rs:2853` (JD2 phase
  1), `main.rs:2915` (JD2 phase 2 — the pump the brief names), `arch/aarch64/xusb_tegra.rs:1774`,
  `:1935`, `:2065`. Only the pre-drop enumeration pump pairs them (`xusb_tegra.rs:1845` +
  `:1847`).
- This is **deliberate and must not be "fixed" by adding the call**. `xusb_tegra.rs:1924-1926`
  records why: *"ONLY `poll_events` here — never the `service_*` pumps. Their bounded waits ride
  `crate::hlt()`… have NO wake source and park this core forever."* Folding
  `x.service_hid_setproto()` onto either pump line would hang the board. Closing this hole needs a
  non-`hlt` bounded wait for the un-halt sequence, i.e. design work outside this arc.

Not convicted for CLICKDEAD: reaching `hid_halt_pending` requires a halting completion, and
`hid_error_witness` — ungated — would have printed it. Zero such lines. Filed so the next pointer
death on tegra is scored against it rather than re-derived.

---

## 5. The fix

The mechanism is located but sub-mechanisms (a1)/(a2) are not yet separated, and **the separation
does not need new accounting — it needs a print.** The three counters already exist, are `pub`, and
are bumped unconditionally on every path; render6 carried the answer in three atomics that nothing
read.

### 5.1 Landed (orin's lane) — `[ptrpoll]` + `reports=` on the census

`arch/aarch64/display_tegra.rs`:

- **file tail** (after the last pre-existing line, 5032): `fn ptrpoll_witness(tick) -> u64` under
  `#[cfg(feature = "orinclick")]`. Reads `crate::drivers::xhci::MOUSE_REARM_COUNT`,
  `MOUSE_DISCARD_REARM_COUNT`, `MOUSE_ERROR_REARM_COUNT` (three `Relaxed` loads, no lock, no MMIO)
  and prints one line when they move, plus one at the start so a frozen pipeline is still
  timestamped:

  ```
  [ptrpoll] t=… rearm=… discard=… errrearm=… reports=… base=… decoded=… -> VERDICT
  ```

  Verdicts: `BASELINE` (first census pass, the enumeration arms) · `STREAMING` (reports decoding —
  a dead click above that line is a ROUTING fault) · `GUARD-REARM` (`mod.rs:4586` is re-arming
  mismatched completions and nothing decodes) · `ERROR-REARM` (`mod.rs:4238`) ·
  **`ARMED-NO-COMPLETION`** — the render6 shape, and the one that tells the bench that the pointer
  endpoint went quiet rather than got mis-routed.
- **census line, in place**: `reports={}` appended as the last field before `-> {}`
  (`display_tegra.rs:1518-1520`); no field reordered. Value = `rearm - discard - errrearm`
  (saturating), i.e. arms that followed a **decoded** report plus one enumeration arm per pointer
  that enumerated (`mod.rs:4310`). This board enumerates two pointers, so `reports=2` means **zero**
  decoded reports. render6's value would have been 2.
- **call site**: folded onto the existing `CLK_CENSUS_TICK.store(tick, …);` line inside
  `orin_click_census` — no new cadence, no line added.

`[ptrpoll]` is nine bytes, deliberately longer than eight so it cannot be folded into an LLVM
immediate and must land in `.rodata` — which is what makes the `grep -a` below a *reachability*
proof rather than a compile proof.

Byte-identity: every added item is `#[cfg(feature = "orinclick")]` and sits after the file's last
pre-existing line, so knob-off no line in `display_tegra.rs` moves. Measured, not argued:
`./arroyo kernel8` before and after the change → `kernel8.img` sha256
`d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0`, identical.

### 5.2 Delivered as a patch, NOT committed (rmbp's lane) — `CLICKDEAD-xhci.patch` v2

`drivers/xhci/mod.rs` belongs to the rmbp seat, so the hunks that separate (a1) from (a2) ship
as `docs/dev/evidence/orin15/CLICKDEAD-xhci.patch` with their gate results, never as a commit
to that file. **rmbp 12 granted the file on 2026-09-06 on two binding conditions**, and the
patch in the tree is v2, the revision those conditions produce (§7):

1. **Split the conflated counter.** v1's single `MOUSE_DUP_DROP_COUNT` was bumped on
   `param == prev || !have_buf` — two defects with opposite repairs under one name. v2 declares
   `MOUSE_DUP_DROP_COUNT` (the known duplicate, buffer intact → the repair is in the guard) and
   `MOUSE_NOBUF_DROP_COUNT` (`mouse_data_buffer`/`mouse_ring` gone → a teardown/allocation
   defect; re-arming would be wrong, and the repair is in the slot's soft state). `!have_buf` is
   tested first, so the two arms partition the branch. §7.2.
2. **Keep the counters ungated, in their siblings' house pattern** — cited, not assumed:
   `80ed35a4:MOUSE_REARM_COUNT`'s doc comment, `mod.rs:2373-2377`, *"Bumped unconditionally
   (cheap relaxed adds); only the knob-gated witness prints."* §7.3.

Both `xhci/mod.rs` hunks are folded onto existing lines (`7,7 -> 7,7`, `wc -l` 14917 = 14917),
so the Pi's `panic::Location` layout is untouched. `display_tegra.rs` gains 4 lines at the file
tail, where nothing follows to move. The Pi **image** still moves, because the counters are
ungated by ruling: knob-off `kernel8.img`
`d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` →
`8ff7c1d1f4e8938d9a29df4a094ecc1fe01684350adeef8a577b13c5eb89dc13`, 1,254,984 bytes both. That
cost is the substance of the grant; §7.5 states it in full.

With the patch aboard the next boot reads:

- `dup=` climbing while `reports=` stays flat → **(a1)**: completions ARE arriving and the
  dup-guard is eating the pipeline. The fix is then in the guard: `param == prev` may only skip
  the re-arm when a read is provably still outstanding.
- `nobuf=` climbing → **(a1) with a different repair**: the completions arrive with
  `mouse_data_buffer`/`mouse_ring` already gone. The guard is innocent; the fault is a teardown
  or an allocation that cleared the buffer without disarming the endpoint.
- `dup=0`, `nobuf=0`, `rearm=` flat, `reports=` flat → **(a2)**: the controller posted nothing.
  The fix moves to the endpoint — EP state / doorbell / periodic bandwidth (note `HUB slot 3
  status-change Configure-Endpoint code 8`, Bandwidth Error, ledger B2, on the same HS hub that
  carries the mouse).

### 5.3 What the next boot should show on the wire

Armed with the same knob line plus nothing new:

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 ./arroyo esp-jetson
```

first census pass:

```
[ptrpoll] t=71 rearm=2 discard=0 errrearm=0 reports=2 base=2 decoded=0 -> BASELINE (…)
[orinclick] census seq=1 … focus=0x0 reports=2 -> IDLE-NO-CLICKS
```

and then, with a hand on the mouse, either `reports=` climbing past 2 (the pipeline is alive and the
hunt moves to routing) or a second `[ptrpoll]` line that never comes — which, read together with the
census's `reports=2`, is `ARMED-NO-COMPLETION` stated on the wire instead of inferred.

---

## 6. Gates

Re-run in this arc's worktree (`exec-orin15-clickdead3`, base `80ed35a4`), both states, every leg quoted.

### 6.1 The committed state (patch NOT applied — what the branch carries)

| gate | result |
|---|---|
| `./arroyo check` (committed tree, after the revert) | exit **0** — `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ bootloader OK`, `✅ kernel cfg coverage OK (49 legs)` incl. `✅ arm-tegra-orinclick` and `✅ arm-tegra-render`, `✅ userspace x86_64 OK (4 crates)`, `✅ userspace aarch64 OK (5 crates)`, `✅ midden_core tests OK` |
| `GATE-KNOB` | **OK** — 158 features declared, 157 named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg |
| `GATE-LEDGER` | **OK** — 84 rows in 2 ledger files + RULINGS |
| `./arroyo test-arm 60` | exit **0** — `target/serial-arm.log`: 0 `-> FAIL`, 0 `EXCEPTION`/`panicked` |
| armed jetson build (knob line §5.3) | exit **0** |
| Pi `./arroyo kernel8` sha256, knob-off, at `80ed35a4` (= what this branch carries) | `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0`, 1,254,984 bytes — unchanged from `2de4a58b`, and this arc adds no source change to move it |

### 6.2 The patch applied (gated, then reverted — the branch keeps neither file's hunks)

| gate | result |
|---|---|
| `git apply --check` on `CLICKDEAD-xhci.patch` (v2, against the committed tree) | exit **0** |
| `./arroyo check` patched | exit **0** — `✅ x86_64 OK`, `✅ aarch64 OK`, `✅ bootloader OK`, 49 cfg-gated legs incl. `✅ arm-tegra-orinclick` and `✅ arm-tegra-render`, `✅ userspace x86_64 OK`, `✅ userspace aarch64 OK`, `✅ midden_core tests OK`; GATE-KNOB **OK** (158 features declared, 157 named by a cfg, 0 phantom, 0 dead), GATE-LEDGER **OK** (84 rows) |
| `./arroyo test-arm 60` patched | exit **0** — `target/serial-arm.log`: 0 `-> FAIL`, 0 `EXCEPTION`/`panicked` |
| armed jetson build patched (§5.3 knob line) | exit **0** |
| Pi `./arroyo kernel8` sha256 patched | `8ff7c1d1f4e8938d9a29df4a094ecc1fe01684350adeef8a577b13c5eb89dc13` — **differs** from the unpatched `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` (see §7.5; the counters are ungated by ruling), 1,254,984 bytes both, `wc -l` on `xhci/mod.rs` 14917 = 14917 |
| after `git checkout --` on **both** files | `git diff 80ed35a4 -- unaos/crates/kernel/src/drivers/xhci/mod.rs` → **0 lines**; same for `arch/aarch64/display_tegra.rs` → **0 lines** |

### 6.3 `grep -a -c -F` on `target/aarch64_esp/kernel.elf` — with the known-absent control

A grep that only ever reports hits is not a gate. The four v2 tokens are the control: same
recipe, same tree, absent unpatched and present patched. `-F` is mandatory — `[ptrpoll] t=` and
`` `dup=` `` carry bracket metacharacters that a basic-regex grep would read as a class.

| token (`grep -a -c -F`) | unpatched | patched |
|---|---|---|
| `[ptrpoll] t=` | 1 | 1 |
| ` errrearm=` | 1 | 1 |
| ` reports=` | 2 | 2 |
| `ARMED-NO-COMPLETION` | 1 | 1 |
| `GUARD-REARM` | 1 | 1 |
| `ERROR-REARM` | 1 | 1 |
| **`DUP-DROP`** | **0** | **1** |
| **`NOBUF-DROP`** | **0** | **1** |
| **` dup=`** | **0** | **2** |
| **` nobuf=`** | **0** | **2** |
| `ZZQQ-NOT-A-TOKEN` (nonsense control) | 0 | 0 |

Both artifacts were built in this worktree from the §5.3 knob line, the unpatched one after the
revert — so the two columns differ in the patch and in nothing else.

---

## 7. `CLICKDEAD-xhci.patch` v2 — the hunks that separate (a1) from (a2), and render7's first question

**v2 supersedes v1.** rmbp 12 GRANTED `xhci/mod.rs` on 2026-09-06 on two binding conditions —
split the conflated counter, and keep the counters ungated in their siblings' house pattern.
§7.2 and §7.3 are what those conditions produce; the patch header states the same two, in the
patch itself, so a reviewer reading only the patch sees the grant's terms.

### 7.1 What is in the branch and what is in the patch

The branch `exec-orin15-clickdead3` carries `unaos/crates/kernel/src/drivers/xhci/mod.rs`
**byte-identical to its base** `80ed35a4`. That is a measured claim, not an intention:

```
git diff 80ed35a4 -- unaos/crates/kernel/src/drivers/xhci/mod.rs | wc -l   ->  0
```

The file is rmbp's lane (CLAUDE.md §Worktrees), so the hunks ship as
`docs/dev/evidence/orin15/CLICKDEAD-xhci.patch` with their own gate results, the way A17
shipped `A17-prtscr.patch` for `prtscr.rs`.

The patch carries **six hunks across two files** (v1 had four; v2 gains two in
`display_tegra.rs` for the second verdict and the doc comments it invalidates), and the second
file needs saying out loud: `arch/aarch64/display_tegra.rs` **is** orin's lane, but its hunks
ride in the patch and not in the commit because they name `MOUSE_DUP_DROP_COUNT` and
`MOUSE_NOBUF_DROP_COUNT`, which do not exist until the first file's hunks land. Committing them
alone would leave the branch unbuildable. **Apply both or neither**, and the branch therefore
carries neither:

```
git diff 80ed35a4 -- unaos/crates/kernel/src/arch/aarch64/display_tegra.rs | wc -l   ->  0
```

The claim is pinned to the base **sha**, not to the `hw-jetson` ref, because that ref moves:
during this arc another orin commit landed on it (tip `8f73ff12`, CONSOLEQUIET). Both files
are 0-diff against that tip as well — `git diff 8f73ff12 -- …/xhci/mod.rs` and
`… -- …/arch/aarch64/display_tegra.rs` are both `0` — so neither the base nor the current
track tip disagrees with the branch on either file.

v1's base was `2de4a58b`; FOLDFIX (`80ed35a4`) moved `ptrpoll_witness` from
`display_tegra.rs:5111` to `:5361`, so v1's hunks for that file no longer sat at their stated
offsets. v2's hunks are regenerated against `80ed35a4` and `git apply --check` is **exit 0**
there (§6.2).

### 7.2 THE SPLIT — rmbp condition 1, and why one counter was wrong

v1 bumped a single atomic on `param == prev || !have_buf`. rmbp 12's grant condition names the
defect exactly: that predicate **conflates two failures whose repairs point in opposite
directions**, so a single `dup>0` on the wire would have named a symptom and not a fix.

| the arm | what actually happened | where the repair goes |
|---|---|---|
| `param == slot.mouse_prev_phys`, buffer and ring PRESENT | the known Panther-Point duplicate Success (XHCI_SPURIOUS_SUCCESS, device 0x1e31) for a TD already consumed. The guard skips the re-arm **by design**: a fresh read is supposed to be outstanding, and re-arming would over-arm the interrupt-IN ring (the UI1-MOUSE M2 hazard) | **in the guard** — `param == prev` may only skip the re-arm when a read is *provably* outstanding |
| `slot.mouse_data_buffer` / `slot.mouse_ring` GONE | a teardown or allocation defect. Re-arming here would be flatly **wrong** — there is nothing to arm | **in the slot's soft state** (`Slot::reset_soft_state`, `mod.rs:2991`, and whoever cleared the buffer without disarming the endpoint). The dup discrimination cannot touch this class |

v2 therefore gives each its own ungated `pub AtomicU64` and its own verdict:

| verdict | meaning |
|---|---|
| `-> DUP-DROP` | **(a1)** completions arrive; the guard ate one it recognised as the known duplicate |
| `-> NOBUF-DROP` | **(a1)** completions arrive; the buffer/ring was already gone. **A different fix** |
| `-> ARMED-NO-COMPLETION` | **(a2)** `dup=0` and `nobuf=0`; the endpoint posted nothing — look at EP state, the doorbell and periodic bandwidth, not at the guard |

**Precedence, stated because the two arms overlap.** Reaching the counted exit means
`!(param != prev && have_buf)`, so `!have_buf` and `param == prev` cover it exhaustively but
not disjointly. `!have_buf` is tested **first**: a duplicate arriving after the buffer is gone
scores `NOBUF`, because the missing buffer is the actionable fault and the duplicate is noise
on top of it. The two arms partition the branch, so `dup + nobuf` is its exact total.

### 7.3 UNGATED — rmbp condition 2, cited

The two new statics follow the house pattern of the three they join, and the pattern has a
citation rather than a habit behind it — `80ed35a4:MOUSE_REARM_COUNT`'s doc comment,
`unaos/crates/kernel/src/drivers/xhci/mod.rs:2373-2377`:

> PIUSB-39 witness counters. `MOUSE_REARM_COUNT` = every `queue_mouse_read` the transfer
> dispatch issued; … **Bumped unconditionally (cheap relaxed adds); only the knob-gated witness
> prints.**

So `MOUSE_DUP_DROP_COUNT` and `MOUSE_NOBUF_DROP_COUNT` sit beside
`80ed35a4:MOUSE_REARM_COUNT` (`mod.rs:2377`), `80ed35a4:MOUSE_DISCARD_REARM_COUNT`
(`mod.rs:2380`) and `80ed35a4:MOUSE_ERROR_REARM_COUNT` (`mod.rs:2385`) under that same rule:
the **count** is unconditional, the **print** is knob-gated (`ptrpoll_witness` is
`#[cfg(feature = "orinclick")]`). The alternatives were both refused, and for named reasons:
gating on `orinclick` would put a board-named knob in a shared driver, which
MEMORY:"name by subsystem, not board" forbids; gating on `usbdebug` would rebuild the very
blind spot the patch exists to close.

### 7.4 The mechanism the patch instruments, stated once

Not "a TRB is not re-queued" — **a completion is consumed and the read is never re-armed,
silently**. In `80ed35a4:XhciController::poll_events`, pointer arm
(`mouse_dci == Some(endpoint_id)`, `mod.rs:4560`), the dup-Success guard fires on
`slot.mouse_expect_phys != 0 && param != slot.mouse_expect_phys` (`mod.rs:4587`). Inside it,
`80ed35a4:XhciController::queue_mouse_read` (`mod.rs:14846`) is called **only** under
`param != prev && have_buf` (`mod.rs:4593`). The complement falls through to the bare `return;`
at `mod.rs:4599`, which:

* consumes the transfer event (the ring dequeue pointer has already advanced),
* enqueues **no** new interrupt-IN TD on the pointer DCI and rings no doorbell — the endpoint
  is left with nothing armed, and
* prints nothing, because its only witness `80ed35a4:XhciController::piusb39_witness`
  (`mod.rs:14730-14731`) is `#[cfg(feature = "usbdebug")]`, and `usbdebug` is not in the Orin
  flight recipe.

Every other exit already accounts for itself — `80ed35a4:MOUSE_REARM_COUNT` (`mod.rs:2377`),
`80ed35a4:MOUSE_DISCARD_REARM_COUNT` (`mod.rs:2380`),
`80ed35a4:MOUSE_ERROR_REARM_COUNT` (`mod.rs:2385`), and halting completions go to the
**ungated** `80ed35a4:XhciController::hid_error_witness` (`mod.rs:14748`). This one branch is
the whole blind spot, and on the wire it is indistinguishable from "the controller posted
nothing" — the exact ambiguity render6 died in.

The patch adds the two counters above, the `dup=` and `nobuf=` fields plus the `DUP-DROP` and
`NOBUF-DROP` verdicts in `80ed35a4:ptrpoll_witness` (`display_tegra.rs:5361`), and folds **both
drops** into the witness's movement test (`PTRPOLL_LAST.swap(rearm + dup + nobuf)`) — **without
that, a pipeline being EATEN (rearm flat, drops climbing) would be silently mistaken for one
that is STARVED** and print one line for the whole boot.

**Diagnostic only.** The guard's control flow is unchanged; the dup hazard is real and
PIUSB-39's discrimination stays exactly as it is. Two relaxed `fetch_add`s of which **at most
one executes per completion**, two relaxed loads on the census pass. No lock, no MMIO.

### 7.5 THE COST — line-neutral in `xhci/mod.rs`, and the Pi image MOVES

Three claims, measured separately, because v1's first draft conflated the first and the third.

* **Line-neutral in `drivers/xhci/mod.rs` — YES.** That file compiles into the Pi's
  `kernel8.img`, and a line added anywhere in it moves every `panic::Location` below
  (MEMORY: "cfg does not protect byte identity"). Both hunks are folded onto existing lines and
  both hunk headers read `7,7 -> 7,7`. Position proof (`python3`, base `80ed35a4` vs. patched):

  ```
  line counts     : old 14917  new 14917  -> EQUAL
  differing lines : [2385, 4599]
  MOUSE_ERROR_REARM_COUNT decl : old [2385]  new [2385]
  MOUSE_DUP_DROP_COUNT decl    : new [2385]
  MOUSE_NOBUF_DROP_COUNT decl  : new [2385]
  guard bare `return;`         : old [4599]
  guard folded `return;`       : new [4599]
  ```

* **`arch/aarch64/display_tegra.rs` gains 4 lines (5397 -> 5401) and is NOT line-neutral.**
  rmbp's note is the general rule and it holds: added lines move every `panic::Location`
  **below their site inside that file**. So the claim is measured, not asserted — at `80ed35a4`
  `ptrpoll_witness` occupies `:5361-5397`, i.e. **it is the file tail**, so the set of
  `Location`s below the site is empty and nothing pre-existing in that file moves either. The
  four added lines are the two new verdict arms; every other change in the file is folded onto
  an existing line.

* **BYTE-neutral on the Pi image — NO. `kernel8.img` MOVES, and that is the cost of the
  grant.** Measured knob-off, `./arroyo kernel8`, in this worktree:

  | knob-off `./arroyo kernel8` | `kernel8.img` sha256 | size |
  |---|---|---|
  | patch **not** applied (branch state, = `80ed35a4`) | `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` | 1,254,984 bytes |
  | patch applied | `8ff7c1d1f4e8938d9a29df4a094ecc1fe01684350adeef8a577b13c5eb89dc13` | 1,254,984 bytes |

  Same size, **different content**. The reason is §7.3: the counters are ungated by ruling, not
  by oversight. The cost is **two 8-byte BSS atomics and one relaxed increment** on the
  pointer-completion path in every aarch64 image — no new file bytes, no lock, no MMIO, no
  control-flow change. That cost is the substance of the grant, and it is why this is a patch
  and not a commit.

### 7.6 render7's FIRST metal question

**The witness line is `[ptrpoll]`** — nine bytes, deliberately longer than eight so it cannot
be folded into an LLVM immediate and must land in `.rodata`, which is what makes `grep -a` on
the artifact a reachability proof rather than a compile proof. It already exists (`80ed35a4`);
render7 needs no new code to answer this.

**The question, scored on ONE line and one field.** The first `[ptrpoll]` line rides census
`seq=1`, which on render6 landed at `up=10s` / `t=71` (`render6-boot1.log:701`) — about ten
seconds after `:: tegra: JB2b — keyboard ARMED ::` (`:307`) and its companion
`2 pointer(s) armed` (`:356`). Read its `rearm=` field:

| first `[ptrpoll]` line | verdict | what it means |
|---|---|---|
| `rearm=2` | the reads **STOPPED** | two arms exist and they are the two enumeration arms (`mod.rs:4310`, one per pointer — this board enumerates the relative boot-mouse on slot 4 and the absolute pointer on slot 5). Nothing has completed since enumeration. This is the render6 boot-1 shape, stated instead of inferred. |
| `rearm>2` | the reads **RE-ARMED** | completions are arriving and the pointer pipeline is alive; the dead click above that line is a routing fault and the hunt moves downstream. |

A single sample decides it, which is the point: the movement test in `ptrpoll_witness` needs
two census passes (~20 s) and the boot-1 failure is already fully formed at ten. `reports=` on
the `[orinclick] census` line of the same pass carries the same balance, so the two lines
cannot disagree — both come from the same loads.

With the patch applied, the same first line's `dup=` and `nobuf=` fields answer the follow-up
in the same ten seconds, and v2 answers it one level deeper than v1 could:

| `dup=` | `nobuf=` | reading |
|---|---|---|
| `>0` | `0` | **(a1) eaten by the dup guard** — the repair is the guard's discrimination |
| `0` | `>0` | **(a1) eaten with no buffer** — the repair is the slot's soft state; the guard is innocent |
| `0` | `0`, `rearm=2` | **(a2) starved** — EP state / doorbell / periodic bandwidth |

### 7.7 The known-absent control for the `grep -a` gate

A `grep -a` that only ever reports hits is not a gate. `DUP-DROP`, `NOBUF-DROP`, ` dup=` and
` nobuf=` are the control: all four are **absent** from the unpatched armed artifact and
**present** in the patched one, built from the same recipe on the same tree, and a nonsense
token is 0 in both. Both columns are recorded in §6.3.

> **Reading the census: `nobuf=` is NOT a milder `dup=`.** They are different defects with opposite remedies — `dup` (known duplicate, buffer intact) says re-arm anyway; `nobuf` (`mouse_data_buffer`/`mouse_ring` gone) says the slot's soft state was torn down and re-arming would be wrong. Never add them: `dup + nobuf` equals the silent-exit total by construction, and a census where they do not is itself a bug signal (rmbp 12, 2026-09-06).

> **Baseline chains are per tree (correction to the 7b143041 commit body, pi 7 2026-09-06).** `d73a8981…` → `8ff7c1d1…` is hw-jetson's knob-off `kernel8.img` chain. What pi 7 accepted is that CLICKDEAD moves the knob-off Pi image ONCE for the recorded reason (two ungated BSS atomics + one relaxed increment, the `MOUSE_REARM_COUNT` pattern); pi's own chain (`77690c77…` → `b5c0a3a1…` on hw-pi4 8131cd2d + S7 v2) re-derives its post-fold value on pi's tree. Comparing baseline shas across branches is meaningless; the invariant is no unexplained move within one chain.
