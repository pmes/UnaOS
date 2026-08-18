# WALKTHROUGH — igpu: GMUX to IGD with a revert that provably runs

Branch `wt/gmux-igd-x86`. Commits:

| sha | title |
|---|---|
| `a2cf0631` | `docs/igpu: the GMUX-to-IGD plan, before any code` |
| `8a8e864b` | `gpu/igpu: GMUX to IGD — the revert now runs in the stream that armed it` |

Files changed: `unaos/crates/kernel/src/drivers/gpu/igpu.rs` (+386/−1) and
`unaos/crates/kernel/Cargo.toml` (+9). Nothing else.

---

## The starting position, stated accurately

Trunk (`0913b91e`) carried **no gmux switch code at all**. The previous session's work
is committed on the `battery-power-consumption-baseline` branch as `46a17952` (the
switch) and `78b0ee3f` (the shell verb). The whiteboard's "what is already right — keep
it, do not rewrite it" therefore described code that was not in this worktree. Those two
commits were used as the reference for both halves of that instruction: what to preserve
in intent, and where the six defects live.

Two things the whiteboard asserted that turned out not to hold in this tree, both
verified before any code was written:

1. **`gmux_igd` was NOT in `crates/kernel/Cargo.toml`.** `ce5c6f49` wired
   `UNAOS_GMUX_IGD` into `arroyo` and `builder/src/main.rs` in the belief that Cargo.toml
   already had the entry; it did not (it was only ever present on the reference branch).
   `--features gmux_igd` could not have resolved at all. Added here.
2. **The three seams do not exist on trunk** — the `interrupts.rs` 1 kHz `gmux_tick()`
   hook, the `main.rs` `x86_usb_pump` executor call, and the `shell.rs` `gmux-revert`
   verb. All three are outside this lane. See "the seam" below.

---

## The design, and why it is what it is

The brief's item 1 is the whole shape of this arc: *arm from a context whose revert
driver is provably live, or refuse to arm.*

The reference armed from `igpu::init()` (inside `pci::init`) and put every port write of
the revert in `gmux_task_tick()`, called from `x86_usb_pump` — a task spawned ~350 lines
of boot later and only when *(not `rast`)* **and** *(framebuffer non-zero)* **and** *(two
distinct APs online)*. Three live paths therefore ended with the mux switched and no
revert, permanently, until power cycle: the inline-BSP fallback where the pump never
spawns; `rast` builds; and any wedge between the arm and the pump's first pass — SDHC,
storage, SMP, xHCI enumeration and the GUI handoff all sit in that gap.

Since none of the three seams may be written from this lane, the deferred design cannot
be made safe here at all: its executor would have **no caller**. Rather than ship an
executor that cannot run and a `due` flag nothing consumes, the arc collapses the two
contexts into one.

**`gmux_arm_switch_and_revert()` arms, writes, verifies, dwells, reverts and verifies on
a single call stack.** The revert driver is not merely live — it is the next statement.
The only code that executes between the switch and the revert is `gmux_dwell()`, a
bounded spin that calls nothing out.

The cost is real and is stated in the source, the commit and the RUNBOOK rather than
hidden: **boot stalls for the 10 s dwell inside `pci::init`, before xHCI enumeration.**

### What was deliberately NOT shipped

`gmux_tick()` and `gmux_task_tick()` are absent. With no hook in `interrupts.rs` and no
call in `main.rs`, both would be functions with no caller — an instrument that cannot
execute in the state it reports on, which is precisely the class of defect this round
exists to eliminate. The synchronous design needs neither.

`gmux_revert_now()` **is** public, and is a complete, idempotent executor. A one-line
seam in `shell.rs` would turn it into a working `gmux-revert` verb. No claim is made
anywhere — source, commit or RUNBOOK — that such a verb exists in this build. The
RUNBOOK says so in as many words, because the alternative is an operator typing a verb
at a black panel and believing the result.

---

## Item-by-item

### 1 — arm only where the revert can run ✅

`gmux_arm_switch_and_revert()`. Arming context == revert executor. Detailed above.

### 2 — the manual trigger completes the revert itself ✅

```rust
pub fn gmux_revert_now() -> bool {
    let claimed = gmux_state_update(|s| {
        if s.armed { Some(RevertState { armed: false, due: false, ..s }) } else { None }
    });
    let Some(s) = claimed else { /* prints "NOT ARMED — no write issued" */ return false; };
    // ... write the triple and read back ...
}
```

It issues the writes, reads back, compares, and returns the verdict. The reference set
`due = true` and returned; on exactly the paths where the automatic revert was already
dead, that printed *"Manual GMUX revert triggered (if armed)"* while nothing moved. The
not-armed case now prints `no write issued` rather than an unqualified success.

### 3 — refuse to arm on an unproven protocol ✅

The call moved **inside** the `PROTOCOL PROVEN` arm of the `boot_ver_ok && kern_ver_ok`
if/else. In the reference the if/else closed at `igpu.rs:410` and the arm block opened at
`:413`, outside both branches, so the version check gated nothing but a print.

### 4 — the knob-off build is identical to trunk ✅ (verified with a tool)

`read_gmux_trace()` is untouched — inline closures, hard iteration cap of 200, no
`arch::ms()` anywhere. All new code carries
`#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]`. The armed helpers keep the
iteration cap **and** add an ms deadline, so `iters == 0 || ms elapsed > bound` ends the
wait whichever trips first; a stopped BSP timer cannot hang the armed build either.

Verified by building the kernel both ways with the knob off and comparing the binaries,
not by reading the diff:

| | trunk `0913b91e` | this branch, knob off |
|---|---|---|
| aarch64 sha256 | `9e9f2cd8fc131bc0…` | `9e9f2cd8fc131bc0…` — **identical** |
| x86_64 sha256 | `7e70dbb4905b1bf5…` | `dea25dad2dc2dfbc…` — differs |

The x86_64 difference was then run down rather than waved away:

- **Normalized disassembly diff: 4 lines, all of them objdump's own filename header.**
  Every instruction, in order, is identical. (`objdump -d`, addresses and `.llvm.<hash>`
  symbol suffixes normalized.)
- **Non-symbol string diff: 0 lines.** No message, format string or panic-location text
  differs. There are no `igpu.rs` panic-location strings in either binary.
- The raw sha differs only in LLVM's internal `.llvm.<N>` symbol-name suffixes, which
  hash the module's source text. Comments and `#[cfg]`-stripped items change that hash;
  they change no code.
- Reproducibility was checked first, so "trunk built twice gives the same sha" is a
  measurement and not an assumption: two independent trunk builds both produced
  `7e70dbb4…`. The Cargo.toml change was isolated separately and is inert — trunk
  `igpu.rs` with the new Cargo.toml still produces `7e70dbb4…`.

### 5 — a failed write is not a switch ✅

The gmux write block writes the triple, reads all three back, and decides the verdict **from
the read-back**. The write helpers' booleans are printed (`ddc=ok disp=TIMEOUT ext=ok`)
because they say *where* it broke, but they decide nothing. Each disagreeing register is
named with both values:

```
:: igpu: [GMUX] switch MISMATCH SW_DDC: wrote 0x01, read 0x02 ::
```

The reference logged write timeouts and then printed `Revert Complete` regardless, so a
timed-out DDC write with a landed DISPLAY write — panel on IGD, DDC on discrete — was
indistinguishable from success, and a black screen was indistinguishable from a write
that never happened.

The revert runs **unconditionally**, including after a `MISMATCH`: a partially-landed
switch is the state that most needs putting back.

### 6 — `REVERT_STATE` read-modify-write is atomic ✅

`gmux_state_update()` is a `compare_exchange_weak` loop over the packed `u64` and returns
the **pre-image** on success. `gmux_revert_now()` therefore claims the armed state and
obtains the saved bytes in one indivisible step, so however many contexts call it,
exactly one wins and issues the sequence. `SeqCst` on an independent load and an
independent store — what the reference had — does not give that.

---

## Kept from the reference, on purpose

- **`RevertState` pack/unpack** — one encode point, one decode point. Enforced rather
  than merely stated: `gmux_dwell()` reads `deadline_ms` back out of the packed word
  instead of keeping a second local copy of it.
- **The `0xFFFFFFFF` refuse-to-arm sentinel.** `gmux_index_read()` returns `0xFFFFFFFF`
  on timeout — a value no 8-bit gmux register can produce, which is what makes the
  sentinel unambiguous. A pre-switch read that timed out means there is no known state
  to return to, so nothing is written.
- **Upstream port constants and write order**, cited in the source at the point of use:
  `0x7C2` value, `0x7D0` read-index, `0x7D4` write-index/status; DDC `0x28` → DISPLAY
  `0x10` → EXTERNAL `0x40`; and **the `wait_ready()` between the value write and the
  index write**. Two separate reviews asked for that wait to be removed; both were
  wrong, the instruction is retracted, and `apple-gmux.c` is now cited at the call so it
  is not raised a third time.

---

## The dwell, as an instrument

An instrument's silence is evidence only if the instrument can execute in the state it
reports on. `gmux_dwell()` is bounded by an `arch::ms()` deadline **and** by an
iteration cap that depends on no clock, and it prints **which bound ended it**:

```
:: igpu: [GMUX] dwell ended by=deadline elapsed_ms=10001 iters=871402 (cap=2000000)
```

`arch::ms()` only advances while the BSP timer ISR runs, so a dwell bounded by it alone
could become a permanent stall with the panel dark. `ended_by=itercap` is a real,
readable finding about the timer rather than a mystery. The wall-clock length of the
iteration cap is **not known a priori** — the `iters=` field is what makes it knowable
after one metal boot, and the RUNBOOK asks the operator to record it.

---

## Gate

Both run from `unaos/`, both green on both arches:

```
$ UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 UNAOS_GMUX_IGD=1 ./arroyo check
⚡ kernel features: ehcihid,smc,smolnet,nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,intel-ivb,unaos_ivb,gmux_igd
✅ x86_64 OK
✅ aarch64 OK

$ UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
⚡ kernel features: ehcihid,smc,smolnet,nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,intel-ivb,unaos_ivb
✅ x86_64 OK
✅ aarch64 OK
```

The armed banner ends `…,unaos_ivb,gmux_igd`, which is the check that three previous
rounds of "gate green" never performed — every green they produced was green about a
build with the switch `#[cfg]`-ed out. No new warnings are attributable to `igpu.rs` in
either run.

**No QEMU suite was run, and none should be:** emulation has no gmux at all, so a green
suite would be evidence about nothing. **Nothing in this arc has been near metal.** The
only verdict that exists for this code is a bench boot that has not happened.
