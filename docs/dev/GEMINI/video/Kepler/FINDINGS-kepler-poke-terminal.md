# FINDINGS — found during the ECHO/POKE split, not fixed

Lane `kepler`, branch `wt/kepler-poke-x86`. Everything here was observed while
doing the arc and deliberately left alone, either because it is out of lane or
out of scope.

---

## 1. The runlist is submitted with beacon words in it — RESOLVED ON TRUNK

**Severity: high. This one invalidates the runlist leg.**

*Fixed on trunk by a470ba16 (write_runlist() + the 8-word runlist-rebuild scan) before the branch merged.*

`kepler.rs` builds three runlist entries:

```
:840  write_volatile(bar1 + runlist_off      , entry_0)
:841  write_volatile(bar1 + runlist_off +  4 , 0)
:842  write_volatile(bar1 + runlist_off +  8 , entry_1)
:843  write_volatile(bar1 + runlist_off + 12 , 0)
:844  write_volatile(bar1 + runlist_off + 16 , entry_2)
:845  write_volatile(bar1 + runlist_off + 20 , 0)
```

and then, ~50 lines later, plants an 8-word beacon pattern over the same bytes:

```
:875  let pattern = [0xBEAC0001, … , 0xBEAC0008];
:894  for (i, val) in pattern.iter().enumerate() {
:895      write_volatile(bar1 + runlist_off + i * 4, *val);
:897  serial_println!(":: kepler: beacon planted at=runlist off={:08X} ::", …)
```

That is `runlist_off + 0 .. +31`, which covers all six words of all three
entries. Nothing between there and the submit restores them:

```
:1373 mmio_write(bar0, 0x2270, (runlist_off as u32) >> 12);  // playlist base
:1374 mmio_write(bar0, 0x2274, 3);                           // LEN=3, ENG=0
```

So the chip is handed a three-entry playlist whose entries are
`0xBEAC0001…0xBEAC0006`. Whatever `PLAYLIST_RD`/`PLAYLIST_RD_LEN` report
afterwards, the runlist leg has never once submitted the entries it built.

Note the polling loop at `:1383` accepts `(pl_rd_len & 0xFFF) == 1` while
`:1374` submitted `3` — worth resolving in the same arc.

Not touched here: the brief names it out of scope, and the fix is a sequencing
decision (move the beacon plant, or re-write the entries after it, or park the
beacons) that belongs with whoever owns the PFIFO leg.

---

## 2. `PHASE_A_BOUND` was a witness that could not fire — FIXED, recorded for the pattern

Fixed in `c071cd09`; recorded here because the *class* of defect is the point.

`PHASE_A_BOUND` was `u8 = 0xBD` and the host compared `phase == phase_bound as
u32`. The ucode reaches it with `mov $r0, 0xbd`, whose I8 immediate is
**signed** — envydis disassembles it as `mov $r0 -0x43`, so MAILBOX1 holds
`0xFFFFFFBD`. The exit-by-bound branch of the `ctx-echo` verdict could not match
on any boot.

Same shape as the four malformed instructions and the same shape as the ledger
defect: an instrument that cannot report the state it exists to report on. Its
silence was being read as "the bound was not reached".

---

## 3. Both images use the same phase magics — NOT FIXED, byte arrays kept verbatim

`PHASE_A_PRELOOP`/`POSTREAD`/`PREACK`/`POSTACK` = `0x01`–`0x04` are stamped by
**both** `ECHO_A_BYTES` and `POKE_A_BYTES`. The module's own comment described
the pull-25 discipline as "image A uses `0x01..0x04`, image B `0x11..0x14`, so
MAILBOX1 alone names which image ran" — that no longer holds.

Not changed because the brief's instruction was to keep the salvage patch's
byte arrays verbatim, and this would edit four bytes in `POKE_A_BYTES`. Each
stamp is an in-place I8 immediate, so no branch displacement moves; it is cheap
whenever someone wants it.

Mitigation in the meantime: the two legs print different labels (`ctx-echo` vs
`ctx-poke`) and run at different points in the boot, so a capture still
distinguishes them — just not from MAILBOX1 alone.

---

## 4. `#[cfg(test)] mod tests` had never compiled — DELETED

It called `pack92`, which does not exist in the module (only `pack128`),
asserted against a `[u8; 92]`, and pinned instruction offsets that had moved.
It could not have compiled at any point after the images grew to 128 bytes.

The reason it survived: nothing runs `cargo test` on the `no_std` kernel crate.
`./arroyo check` is the gate and `#[cfg(test)]` code is invisible to it. **Any
`#[cfg(test)]` code anywhere in `crates/kernel` is in the same position** — it
is unreachable by the gate and unverifiable by construction. Worth a decision at
project level: either wire a `cargo test` target into `arroyo` for the crates
that can host one, or treat `#[cfg(test)]` in the kernel crate as banned and put
the invariants in `const _: () = { … }` blocks, which the gate *does* evaluate.

This arc took the second path locally.

---

## 5. The `ucode-echo` leg runs the same image twice — NOT FIXED, out of scope

```rust
for &(h2h3_label, img, phase_bound) in &[
    ("on",  &UCODE_CTX_ECHO_A[..], ucode::PHASE_A_BOUND),
    ("off", &UCODE_CTX_ECHO_A[..], ucode::PHASE_A_BOUND),
] {
```

Both arms upload `UCODE_CTX_ECHO_A`; the only difference between them is that
the `"on"` arm additionally writes `ENGINE_STATUS`/`ENGINE_TRIGGER`. The `"off"`
arm is a control, which is legitimate — but the variable names (`h2h3`, `img`)
read as if two different images were being compared, which is what pull 33's
A/B structure did. Worth renaming so the leg says what it varies.

Note also that the loop `break`s on SUCCESS, so on a healthy boot the `"off"`
control never runs.

---

## 6. The salvage patch does not apply to this tree — recorded so nobody retries it

`docs/dev/GEMINI/salvage/kepler-echo-poke-split.patch` was cut against
`kepler.rs` blob `422a0d0d`, whose `ucode-echo` verdict was `if ack != MB_SEED`
and whose verify-mismatch arm used `continue`. The tree's verdict is
`if ack == 1`. `git apply --check` fails at hunk `@@ -65,89 +65,60 @@`.

Its two byte arrays were transcribed verbatim (and independently re-verified
with envydis); everything else was rebuilt by hand.
