STATUS: APPROVED (2026-07-22, reviewer) — this is the dual-instrumentation the rejection asked for. Candidate 2 (HEAD_VAL CRTC readback 0x610000 + 0xA00 + head*0x540 + 0x128) is a genuinely new source with reasoning (EFI may have posted via CRTC not EVO), read alongside Candidate 1 behind the guard, first-non-zero wins — exactly right. Wall 2 + §3 unchanged/approved. Witness the CRTC candidate raw too (head-raw already logs evo+crtc — good). Implement.

# PROPOSAL — Kepler pull 6 (v2)

## Wall 1 — Head Scanout Address (Dual Instrumentation)

**Background & Rejection Analysis:**
The previous proposal used the EVO ARMED state `NV_EVO_CORE` `HEAD` array at `0x610000 + 0x400 + head*0x300 + 0x60`. However, as the review pointed out, this address was already probed in sitting #3 and yielded zero. If the firmware (Mac EFI) initialized the display via VBIOS/CRTC direct register writes rather than the EVO core channel, the EVO armed shadow will legitimately be zero because the EVO channel has not been activated yet. 

**Derivation of Candidate Sources:**
To resolve this in a single boot, we will instrument **both** candidate sources derived from `envytools`. The first non-zero address will win and be selected as the active scanout address.

**Candidate 1: EVO Armed Shadow (NV_EVO_CORE)**
- **File:** `nv_evo.xml`
- **Offset:** `0x400` (`HEAD` array, stride `0x300`), sub-offset `0x60` (`G80_EVO_FB_SETTINGS`, `OFFSET_ORIGIN`).
- **Address:** `0x610000 + 0x400 + head*0x300 + 0x60`.
- **Reasoning:** Expected to be zero if EFI bypassed EVO, but included as Candidate 1 for completeness and fallback if the firmware did use EVO.

**Candidate 2: Direct CRTC/Head Timing Registers (HEAD_VAL)**
- **File:** `g80_pdisplay.xml`
- **Offset:** `HEAD_VAL` array is defined at offset `0xA00` with `stride="0x540"`. Inside this stripe, `FB_POS` is at offset `0x128`, and `FB_SIZE` is at offset `0x118`.
- **Address:** `0x610000 + 0xA00 + head*0x540 + 0x128`.
- **Reasoning:** The `HEAD_VAL` block provides read-only hardware state of the CRTC timings and framebuffer position ("These values are only for reading purposes... They do map to display commands, but not 1:1"). This is the direct CRTC readback mechanism that will reflect the live framebuffer even if the EVO core channel was never initialized by EFI.

**Implementation Plan for `kepler.rs`:**
We will read both candidates behind the bad-read guard.
```rust
let head_evo = regs::NV_PDISPLAY_BASE + 0x400 + (head * 0x300) + 0x60;
let head_crtc = regs::NV_PDISPLAY_BASE + 0xA00 + (head * 0x540) + 0x128;

let addr_evo = mmio_read(bar0, head_evo);
let addr_crtc = mmio_read(bar0, head_crtc);

serial_println!(":: kepler: head-raw head={} evo={:08X} crtc={:08X} ::", head, addr_evo, addr_crtc);

// Select the first valid candidate
let addr = if addr_evo != 0 && (addr_evo & 0xFFF00000) != 0xBAD00000 {
    addr_evo
} else if addr_crtc != 0 && (addr_crtc & 0xFFF00000) != 0xBAD00000 {
    addr_crtc
} else {
    0
};
```
We will then compare the winning `addr` to our expected UEFI GOP address.

## Wall 2 (PBDMA bind) — ACCEPTED
- Kept as previously approved (`SUBFIFO_ENG_MASK[0]=1` at `0x2390`).

## §3 cleanroom — ACCEPTED
- Kept as previously approved (honest empirical comment + bad-read guard on `0x490` read).
