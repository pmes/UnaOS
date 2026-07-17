# BATMON-1 — landing report (hw-rmbp)

**Arc:** Apple SMC battery monitor for the 2012 rMBP (Peter's "run off battery with an on-screen
monitor" goal). **Branch:** hw-rmbp. **Base:** main tip `46dca87`. **DONE gate: PASS.**

## What landed

- **`037917e` smc: BATMON-1 — Apple SMC battery monitor (x86, UNAOS_SMC=1)** — the whole arc:
  - NEW `unaos/crates/kernel/src/drivers/smc.rs` — the polled SMC key/value driver (protocol +
    M1 scout + M2 battery module).
  - `arch/x86_64/pci.rs` — `scout()` + one boot battery read, fired once under `#[cfg(feature="smc")]`.
  - `main.rs` — battery `refresh_if_due()` in the usbdebug main loop (metal on-screen path).
  - `vug.rs` — the "BATT" meter-surface hook in `draw_meters` + a refresh on the 200 ms cadence.
  - `drivers/mod.rs`, `Cargo.toml` (`smc` feature), `arroyo`, `builder/src/main.rs` (knob plumbing +
    QEMU `isa-applesmc` attach under `UNAOS_SMC=1`).
  - Doc `unaos/docs/dev/OS/09_PLATFORM/smc_battery.md`.
- **`<this commit>` docs(batmon): landing report** — this file.

Lane held exactly: new `smc.rs` + a vug hook + knob plumbing + doc. Zero aarch64, zero `syscall.rs`,
zero USB/xHCI/EHCI touches.

## Write surface (tripwire-grade) — respected

Every port access is to **0x300 (data)** or **0x304 (command/status)** only, and only under the `smc`
feature. The value-mutating `WRITE_CMD` (0x11) is **never issued** — it is not even a defined
constant. The error port 0x31e is **not** touched (absent-vs-stuck is read from the 0x304 status byte
alone). Every status wait is bounded by an `rdtsc` deadline; a wedged handshake returns
`SmcError::Stuck` and emits a traced STOP-NOTE — never forced. No write-key need arose (none was in
scope). No STOP-tripwire hit.

## Gate results (verbatim)

- **`./arroyo check` both arches, all knob states:**
  - default (knob off): `✅ x86_64 OK` / `✅ aarch64 OK` (only pre-existing warnings).
  - `UNAOS_SMC=1`: `⚡ kernel features: ehcihid,smc` → `✅ x86_64 OK` / `✅ aarch64 OK`, zero smc warnings.
- **Knob-OFF byte-identity:** default kernel `.text` + `.rodata` **SHA256-identical** between the
  working tree and a clean `46dca87` worktree —
  `.text 374adf2a2eec9add0eccca3b2cb5686275a4ceb590be1b5b7e18a0f6f7fb5902`,
  `.rodata c427601c108bcd8708819c1289e7e60786d163fb0b4b2715e0b5200d1370bea6` (both trees). `cmp` =
  TEXT IDENTICAL / RODATA IDENTICAL. (The 24-byte ELF file-size delta is symtab/location metadata;
  declared drift, same class as prior arcs.)
- **Knob-ON QEMU witnesses (`UNAOS_SMC=1 ./arroyo test 40`):**
  - `:: SMC-SCOUT: begin (ports data=0x300 cmd=0x304) ::`
  - `:: SMC-SCOUT: key REV  present len=6 bytes=[01 13 0f 00 00 03] (SMC firmware revision) ::`
  - `:: SMC-SCOUT: key OSK0 present len=32 ... ::`
  - all battery keys + `#KEY` reported **absent** (bounded, no hang);
  - `:: SMC-SCOUT: index enumeration unavailable (no #KEY — QEMU/limited SMC; metal yields the full list) ::`
  - `:: SMC-SCOUT: end (present=Y probed=18 found=2) == witness ::`
  - `:: SMC-BATT: present=false soc=-% volt=-mV amp=-mA full=-mAh rem=-mAh ac=? == witness ::`
  - `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`
  - Confirmed too under `UNAOS_SMC=1 UNAOS_USBDEBUG=1 ./arroyo test 25` (the metal on-screen path):
    same SMC-SCOUT/SMC-BATT + MISSION SUCCESS ×2.
- **Full regression (smc off — no regression):**
  - `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200` → **0 FAIL, 24 PASS**, all S/U witnesses
    present (S4-race, S6, U6gx PASS), no panic/EXCEPTION.
  - `./arroyo test 40` → `MISSION SUCCESS`, 0 FAIL.
  - `UNAOS_NOSTORAGE=1 ./arroyo test 40` → 0 FAIL, 11 PASS (storage-independent chain), clean.

## Review lens (single-lens tier — thin driver, read-only surface)

One lens. **0 MUST-FIX, 0 SHOULD-FIX.** Confirmed: surface confined to {0x300, 0x304}, no WRITE_CMD,
every handshake bounded/reported-not-forced (wrapping-deadline verified safe), read protocol matches
the QEMU isa-applesmc model + real SMC, honest `None`/`present=false` on QEMU, all hooks feature-gated.
**3 notes folded/dispositioned:**
1. (FOLDED) `SMC-BATT` amperage printed `0` for `None` — ambiguous vs a real 0 mA reading. Now prints
   a `-` sentinel for every absent field (`snapshot()` keeps the honest `None`; only the human-facing
   witness applies the sentinel).
2. (FOLDED — documented) The throttle-then-transact sequence isn't atomic under SMP. Every caller is
   BSP-only on this path (single-threaded), so it never interleaves in practice; a corrupt read would
   be bounded (`Stuck`), never a hang. Documented in `refresh_if_due` with the lock-if-ever-SMP note.
3. (FOLDED — comment) Softened the "QEMU implements neither 0x12 nor #KEY" claim to "the build gated
   here" (empirically confirmed by the M1 scout at the gate); the driver handles success/Absent/Stuck
   identically regardless.

Post-fold: `check` both arches clean, witnesses re-confirmed, byte-identity preserved (folds touched
only the cfg-gated `smc.rs`).

## Staged media (esp built LAST, after all `test` runs)

- **Path:** `~/unaos-bench/flash/rmbp/batmon-20260717T135858Z-037917e/esp/`
- **kernel.elf sha256 (staged copy, re-hashed):**
  `a7e0bbe9c8b47ddc7ed47d9e9edeea9e8acf9714f5dd7e012f63cd9158a030fa`
- **Built-from:** hw-rmbp@037917e, knobs `UNAOS_SMC=1 UNAOS_USBDEBUG=1` (MANIFEST line appended).
- Boot-validated in QEMU (MISSION SUCCESS ×2, SMC witnesses fire); validated by booting, not size
  (523,424 B usbdebug kernel — the x86 size band is stale/unreliable per `unaos-hazards`).

## M3 — sitting-assertable trace list (Peter's sitting, NOT this gate)

`UNAOS_SMC=1 UNAOS_USBDEBUG=1` media, on the 2012 rMBP over FTDI serial (and on-screen via fbcon mirror):

1. `:: SMC-SCOUT: key REV  present len=6 bytes=[...] ::` — the protocol works on the real SMC.
2. The `:: SMC-SCOUT: key <B*/#KEY/CH*/AC-W/BC?V> present len=N bytes=[...] ::` block — **the
   machine's true battery-key inventory** (which of the curated keys exist + their payloads). This
   decides M2's final key set and the per-cell fork (BC1V/BC2V/BC3V present or not).
3. If `#KEY` present: `:: SMC-SCOUT: idx N = <NAME> ::` lines — the full index-enumerated key list.
4. `:: SMC-BATT: present=true soc=.. volt=.. amp=.. ::` **tracks reality**: unplug ⇒ discharge
   (amp < 0, soc falls), plug ⇒ charge (amp > 0); the on-screen "BATT" bar follows.
5. **No** `STOP-NOTE handshake stuck` line on the real SMC (bounded waits sized for metal timing).

## Flagged / notes for the integrator

- **M2 is provisional by construction.** Its key decode uses the documented standard 2012 names; the
  exact set + the per-cell fork are gated on the M1 metal inventory (trace #2 above). No per-cell code
  is assumed — the sitting decides whether that fork opens (a follow-on arc if so).
- **No STOP-tripwire hit**; no out-of-lane file needed; no protection weakened.
- Two commits on hw-rmbp, UNMERGED (integrator merges after review). The brief's open item (SMP
  trampoline / 0x8000) did not arise in this lane.
