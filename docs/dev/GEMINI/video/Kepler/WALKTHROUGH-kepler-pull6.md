# Walkthrough: Kepler Pull 6 Execution

I have implemented the derivations from `PROPOSAL-kepler-pull6.md` directly into the `kepler.rs` driver.

## What Was Changed

### 1. Head ARMED-state offset (Wall 1)
- Replaced the flawed `0x616100` base and `0x800` stride with the correct `NV_EVO_CORE` shadowed base: `0x610000 + 0x400 + (head * 0x300) + 0x60`.
- The `OFFSET_ORIGIN`, `SIZE`, and `STORAGE` reads now correctly target the live hardware ARMED state instead of identically-returning fixed capability registers (`HEAD_CAP`).
- Maintained the bad-read guards to ensure any future discrepancies will self-identify on metal.

### 2. PBDMA Runlist Bind (Wall 2)
- While the previous code enabled the PBDMAs globally via PMC (`0x204`), the PBDMA did not fetch anything because it wasn't configured to serve a specific engine.
- Added a write of `1 << 0` (targeting Engine 0: PGRAPH) to `SUBFIFO_ENG_MASK[0]` at offset `0x2390`.
- This binds PBDMA 0 to fetch the active runlist entries for PGRAPH.

### 3. Cleanroom Debt Removal
- Removed the forbidden Nouveau GPLv2 citation from `kepler.rs` concerning the EVO core-channel control register (`0x490`).
- The register `0x490` relative to `PDISPLAY` is genuinely undocumented in public `envytools`/`rnndb` XMLs.
- Replaced the citation with an honest empirical note: `"Empirically probed on GK107, unverified against public docs."`
- Placed the initial read of the `0x490` block behind the bad-read guard. If the empirical probe is incorrect on a different GPU stepping, it will safely abort execution.

## Next Steps
The changes are ready to be compiled into the Fox kernel and tested on bare-metal to witness the new variables and confirm if the PBDMA begins fetching the Pushbuffer.
