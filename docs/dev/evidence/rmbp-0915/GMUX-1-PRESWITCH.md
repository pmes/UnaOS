# GMUX-1 — the pre-switch line the flight-5 capture already carried

`docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md` §5 called this "the cheapest open item in the
register — it costs no boot at all": the gmux ladder's pre-switch gate prints the state it is
about to judge at `igpu.rs:1214`, *before* it refuses at `:1245`, so the register that stopped
flight 5 at `highest=00/10` has been named on the wire since 2026-08-28 and nobody had read it
back. This file reads it. No boot, no build, no code change.

Run 2026-09-15 on branch `exec-rmbp-gpudocs`, cut from `hw-rmbp` **9a7b09a4**. Kernel paths are
under `unaos/crates/kernel/src/`; every `file:line` below was re-read with `sed -n` at commit
time.

## 1. The capture, and how many boots are in it

    ls -la ~/unaos-bench/capture/rmbp9-flight5/
      -rw-r--r--  1316881  Aug 28 11:57  ttyUSB0.log     (8 651 lines)
      -rw-r--r--        0  Aug 28 11:42  reader.err

One boot. The capture has a single `[NVIDIA] Initialization complete` at `[25954ms]`, a single
`igpu: PROTOCOL PROVEN`, a single `[GMUX] REFUSED`, and one `SHARD: about` rollup at
`[41119ms]`. There is no second boot to compare against, so every statement below is scoped to
that one boot.

The bench capture is outside git. The in-tree reading of the same flight is
`docs/dev/evidence/rmbp9/FLIGHT5-POSTMORTEM.md`.

## 2. The line

Read with `awk`, never bare `grep` (LAWS §5 — the bracketed tags in this log are character
classes to `grep`):

    awk 'index($0,"igpu-dpy")' ~/unaos-bench/capture/rmbp9-flight5/ttyUSB0.log

returns exactly two lines, and they are consecutive:

```
[  25955ms] :: igpu-dpy: pre-switch state DDC=0x02 SW_DISP=0x03 SW_EXT=0x01 DISP=0x03 EXT=0x21 sw_ext_state=UNACCEPTED ext_state=kepler-owned ::
[  25955ms] :: igpu-dpy: LADDER highest=00/10 name=harness ok=0 pending=0 gmux=UNTOUCHED why=pre-switch-not-accepted elapsed_ms=1 ::
```

with the refusal itself printed by the outer handler at the same millisecond:

```
[  25955ms] :: igpu: [GMUX] REFUSED: pre-switch-not-accepted (status: 0x00000000) ::
```

**The rung labelled its own offender.** `sw_ext_state=UNACCEPTED` is `ext_state_name(p_ext)`
(`igpu.rs:280`) applied to the `SW_EXT` field; `ext_state=kepler-owned` is the same function
applied to `EXT`. One field of the five is outside the accepted set, and the wire says which.

## 3. What the mux state was at the moment of the refusal

Decoding each field against the constants at `igpu.rs:236-300` and the port map comment at
`igpu.rs:238-243` (`GMUX_PORT_SWITCH_DISPLAY 0x10 · GMUX_PORT_SWITCH_DDC 0x28 ·
GMUX_PORT_SWITCH_EXTERNAL 0x40`; `GMUX_SWITCH_DDC_IGD 0x1 / _DIS 0x2 ·
GMUX_SWITCH_DISPLAY_IGD 0x2 / _DIS 0x3`, EXTERNAL sharing the DISPLAY encoding):

| field | gmux index | read at | value | constant | decode |
| --- | --- | --- | --- | --- | --- |
| `DDC` | `GMUX_SWITCH_DDC` `0x28` (`:252`) | `igpu.rs:1209` | `0x02` | `GMUX_DDC_DIS` `0x02` (`:260`) | DDC routed to the **discrete** GPU |
| `SW_DISP` | `GMUX_SWITCH_DISPLAY` `0x10` (`:254`) | `igpu.rs:1210` | `0x03` | `GMUX_DISPLAY_DIS` `0x03` (`:262`) | display switch target = **discrete** |
| `SW_EXT` | `GMUX_SWITCH_EXTERNAL` `0x40` (`:291`) | `igpu.rs:1211` | `0x01` | **no named constant** | see §5 |
| `DISP` | `GMUX_READ_DISPLAY` `0x11` (`:256`) | `igpu.rs:1212` | `0x03` | `GMUX_DISPLAY_DIS` `0x03` | panel owned by the **discrete** GPU |
| `EXT` | `GMUX_READ_EXTERNAL` `0x41` (`:258`) | `igpu.rs:1213` | `0x21` | `GMUX_EXTERNAL_KEPLER_OWNED` `0x21` (`:275`) | external port **Kepler-owned** |

**Every register that reports mux state read exactly the state G4 proved and this gate was
written to accept.** DDC discrete, panel discrete, external Kepler-owned: that is the 2012 rMBP
norm recorded at `igpu.rs:265-268` ("Metal fact, Boot AK: `pre-switch state DDC=0x02 DISP=0x03
EXT=0x21` … This is the NORM on the 2012 rMBP"), and it is what §5 of the register says the
Kepler owns at every observed instant. Nothing about the firmware's state was unexpected.

## 4. What the refusal actually checked

The gate is one predicate at `igpu.rs:1241-1246`:

```rust
let ext_ok = |v: u32| v == GMUX_EXTERNAL_DIS as u32 || v == GMUX_EXTERNAL_KEPLER_OWNED as u32;
if p_ddc != GMUX_DDC_DIS as u32 || disp != GMUX_DISPLAY_DIS as u32
    || !ext_ok(p_ext) || !ext_ok(ext) {
    return Err(("pre-switch-not-accepted", 0));
}
```

Term by term against the wire, and only the third fails:

| term | operand | value | accepted set | result |
| --- | --- | --- | --- | --- |
| `p_ddc != GMUX_DDC_DIS` | `DDC` | `0x02` | `{0x02}` | **pass** |
| `disp != GMUX_DISPLAY_DIS` | `DISP` (`READ_DISPLAY`) | `0x03` | `{0x03}` | **pass** |
| `!ext_ok(p_ext)` | `SW_EXT` (`SWITCH_EXTERNAL`) | `0x01` | `{0x03, 0x21}` | **FAIL** |
| `!ext_ok(ext)` | `EXT` (`READ_EXTERNAL`) | `0x21` | `{0x03, 0x21}` | **pass** |

`SW_EXT` — the read of gmux index `0x40`, `GMUX_SWITCH_EXTERNAL` — is the single discriminating
datum. It is also the register the code's own comment at `igpu.rs:1231-1233` says "has never
been captured on a Kepler-owned boot, so neither may be assumed DIS". Flight 5 is its first
metal reading, and it is `0x01`: neither `GMUX_EXTERNAL_DIS` (`0x03`) nor
`GMUX_EXTERNAL_KEPLER_OWNED` (`0x21`), and not the `0xFFFFFFFF` timeout sentinel either.

## 5. The verdict: a wrong READ, and the register-level reason

The question GMUX-1 was opened to answer is whether A7's "does not persist" rests on a
firmware-owned state, a wrong write, or a wrong read. **It is a wrong read, and it is ours.**

- **Not a wrong write.** Nothing was written. `gmux=UNTOUCHED` on the LADDER line, `mux_touched`
  is still `false` at the `Err` return (`igpu.rs:1245`), and the refusal is upstream of every
  `gmux_index_write` in the function. On the flight's own record the ladder reached
  `highest=00/10` in `elapsed_ms=1`.
- **Not firmware-owned state.** Every register in the gmux that *reports* ownership read its
  accepted value (§3). The firmware left the machine in precisely the configuration this rung
  was written for.
- **A wrong read.** `0x40` is the **write-target** port, not a status port. The tree says so
  itself in three places, each written for a different purpose and none of them reconciled with
  the gate:
  1. The port map at `igpu.rs:238-243` names `0x40` `GMUX_PORT_SWITCH_EXTERNAL` and pairs the
     DISPLAY switch `0x10` with a separate read register `GMUX_READ_DISPLAY 0x11` (`:256`) and
     the EXTERNAL switch `0x40` with `GMUX_READ_EXTERNAL 0x41` (`:258`). The `SWITCH_*` /
     `READ_*` split is the port pair: one is the command, the other is the status.
  2. The comment at `igpu.rs:1195-1198` states the same split in the gate's own variables:
     "`pre_ext` is the SWITCH_EXTERNAL target register (what the unwind writes back),
     `pre_ext_status` is the READ_EXTERNAL status register (what the MATCH verdict compares)".
  3. The **post**-switch read-back at `igpu.rs:1339-1345` was already relaxed on exactly this
     reasoning: "SWITCH_DISPLAY and SWITCH_EXTERNAL have never been read back on this machine,
     and **write-side switch ports that do not echo their value** would fail this comparison on
     a switch that WORKED — blanking the panel and aborting before the first AUX transaction,
     the whole round spent and nothing learned."

  The third is the finding. The post-switch comparison learned that a write-side port need not
  echo and demoted it to advisory; the **pre**-switch gate at `:1241` still demands that the
  same read of the same write-side port be a member of a status-value set. One boot was spent on
  that asymmetry. `0x01` is not a mux state, because `0x40` does not report one — the encoding
  the gate scored it against (`GMUX_SWITCH_DISPLAY_IGD 0x2 / _DIS 0x3`, shared by EXTERNAL) is
  the encoding of values you *write* to `0x40`, not of values it returns.

- **The read-width lesson is not the explanation here, and it was checked.** Sitting #9 failed
  the gmux under a 3x8-bit read variant whose signature was two indices returning identical
  bytes (register §5, G3). That signature is absent: the five indices returned five values,
  three of them the exactly-correct named constants, and `PROTOCOL PROVEN` fired 411 ms earlier
  at `[25544ms]` with version 3.2.19 and `SW_DISPLAY = 0x03 (DIS)` / `SW_DDC = 0x02 (DIS)`
  through the same 32-bit indexed path. The handshake is sound; the gate's choice of register is
  not.

## 6. What this does to ledger A7

`docs/dev/OS/rmbp-ledger.md` A7 already names the value — "open — `GMUX_SWITCH_EXTERNAL=0x01`
is the blocker" — from the seat-local baton `B11 P6`. This file replaces that with an in-tree,
re-derivable citation and supplies what the baton line could not: `0x01` blocks because the
pre-switch gate reads a write-target port and scores it as a status port, while the three status
ports all read their accepted values. A7's **item** text ("switches and restores on one call
stack") remains false for the separate reason the register §5 already recorded — G7 has never
run, so there is nothing yet that could fail to persist. Only the evidence cell is changed in
this commit.

## 7. The next rung, and the trap in it

The fix is not "delete `!ext_ok(p_ext)`", because `p_ext` is not only gated — it is **restored**.
`igpu.rs:1289` and `:1309` push `unwind.push_gmux(GMUX_SWITCH_EXTERNAL, p_ext as u8)`, and the
comment above each says EXTERNAL is restored to "the pre-image the gate above validated against
the two-constant set". Drop the gate term alone and the unwind writes `0x01` — a value no
encoding in this file names — into the external display mux on every exit path, which is the
silent state change `igpu.rs:270-272` forbids. DISPLAY already shows the right shape: it is
printed, not gated, and the unwind writes the constant `GMUX_DISPLAY_DIS`.

So the rung is one decision with two halves, and it needs no boot to design: gate on the status
registers only (`DDC`, `READ_DISPLAY`, `READ_EXTERNAL`), and restore EXTERNAL to the member
`READ_EXTERNAL` validated rather than to the read of `0x40`. It is carried as `GMUX-2` in
`docs/dev/OS/rmbp-queue.md` §GPU LADDERS. Until it lands, G5, G7, G9 and A7 stay unreachable for
the reason register §5 gives: the ladder refuses before it touches anything.
