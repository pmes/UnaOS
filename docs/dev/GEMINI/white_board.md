# WHITE BOARD — 2026-08-07 (GR20)

## 1. Is there a card in the rMBP's INTERNAL SDXC slot? (One experiment settles the whole SDHC arc.)

**Only you can answer this — it needs a hand on the machine, which is why it is here rather
than decided in the seat.**

Background. You asked where we are on booting from the built-in SD reader as a real,
writable boot disk. Boot AB answers the first half unambiguously, and it is not what the
playbook assumed: **we do not boot from the built-in reader at all.** The boot medium is a
USB card reader on the xHCI/BOT path —

```
xHCI: Disk 'Generic-' 'USB3.0 CRW   -SD' block_size=512 num_blocks=124735488 (60906 MiB)
```

— while the internal Broadcom SDHC controller (PCI `3:0.1`, `14e4:16bc`) is a separate
device that `sdhc.rs` drives and that cannot reach the USB disk at all.

The internal controller itself is healthy. It resets, powers to 3300 mV, clocks to the
400 kHz identification rate, and its Present State reads `0x1fff0000` —
`card-inserted=1 cd-stable=1 cd-pin=1 wp-switch=1` (write-enabled). Then:

```
[sdhc] cmd0 go-idle ok
[sdhc] cmd8 send-if-cond FAILED int=0x00018000 (cmd-timeout)
       — card is pre-v2.00 or absent; this milestone identifies v2.00+ cards only
```

**That CMD8 timeout is the single thing blocking the entire arc**, and it is ambiguous
across three different worlds that the current instrument cannot separate:

- **the slot is empty** and card-detect is phantom (plausible: `cmd0 go-idle ok` proves
  nothing, because CMD0 has no response — that line prints "ok" into an empty slot);
- **the slot holds a pre-v2.00 (SDSC) card**, which legitimately does not answer CMD8;
- **our CMD8 is wrong** (response type, post-CMD0 settling, voltage pattern).

Every downstream step — reading the internal card, registering it as an x86 block backend,
writing to it, and finally booting from it — sits behind this one reading. SDHC-4a (the
write path) is written, gated, QEMU-proven in three arms, and **cannot execute a single
instruction on metal until CMD8 succeeds**, because `identify()` returns before the write
self-test is ever called.

**What would settle it:** put a **known-good SDHC or SDXC card (v2.00+, i.e. anything
modern — a plain 32 GB card is ideal)** into the rMBP's own SDXC slot and fly one boot. No
media changes needed; it can ride whatever boot is next.

- If CMD8 then **succeeds**: the slot works, the arc is unblocked, and the next step is
  reading that card — the write path is already waiting.
- If CMD8 still **fails with a card you know is good**: the bug is ours, in the CMD8
  sequence, and that becomes the arc.

Either answer is worth the boot, and the seat cannot generate either one alone.
