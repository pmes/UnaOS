// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EHCI-3 schedule structures — Queue Heads, qTDs, the periodic frame list, and small DMA
//! buffers. All controller-visible memory comes off the kernel heap, which is identity-mapped
//! (the virtual address returned by the allocator IS the physical/bus address — the same
//! invariant the xHCI driver relies on, xhci ring.rs), so the `as u64` cast of an allocation is
//! exactly the pointer programmed into the controller. Panther Point advertises 32-bit
//! addressing only (HCCPARAMS.64bit=0, EHCI-1 metal evidence); the heap already lands < 4 GiB,
//! and `assert_dma32` makes the assumption crash-proof at bring-up instead of silently corrupt.

use alloc::alloc::{alloc_zeroed, Layout};

/// Terminate bit for horizontal/next pointers (bit 0).
pub const PTR_TERMINATE: u32 = 1;
/// Frame-list / horizontal-link type field (bits 2:1): 01 = QH.
pub const PTR_TYPE_QH: u32 = 1 << 1;

// ---- qTD token (dword 2) ----
pub const QTD_ACTIVE: u32 = 1 << 7;
pub const QTD_HALTED: u32 = 1 << 6;
/// Any transaction-fatal status bit: halted / data-buffer error / babble / transaction error /
/// missed microframe. (Bit 0 ping / bit 1 split-X-state are transient split bookkeeping.)
pub const QTD_ERR_MASK: u32 = (1 << 6) | (1 << 5) | (1 << 4) | (1 << 3) | (1 << 2);
pub const QTD_PID_OUT: u32 = 0 << 8;
pub const QTD_PID_IN: u32 = 1 << 8;
pub const QTD_PID_SETUP: u32 = 2 << 8;
/// Error counter: 3 retries per transaction (the spec default; 0 would mean retry forever).
pub const QTD_CERR3: u32 = 3 << 10;
pub const QTD_IOC: u32 = 1 << 15;
pub const QTD_TOTAL_SHIFT: u32 = 16; // total bytes to transfer, bits 30:16
pub const QTD_DT: u32 = 1 << 31; // data toggle (consumed when the QH has DTC=1)

// ---- QH endpoint characteristics (dword 1) ----
pub const QH_EPS_FULL: u32 = 0 << 12;
pub const QH_EPS_LOW: u32 = 1 << 12;
pub const QH_EPS_HIGH: u32 = 2 << 12;
/// Data-Toggle Control: take DT from each qTD token (software tracks the toggle per endpoint —
/// the re-arm idiom below flips it explicitly, so the QH never owns hidden toggle state).
pub const QH_DTC: u32 = 1 << 14;
/// Head of the async reclamation list (exactly one QH on the async ring carries it).
pub const QH_HEAD: u32 = 1 << 15;
pub const QH_MPS_SHIFT: u32 = 16; // max packet size, bits 26:16
/// Control-Endpoint flag: set ONLY for a Full/Low-Speed control endpoint reached through a TT
/// (the controller then runs the SSPLIT/CSPLIT control dance itself). Never set for HS.
pub const QH_CTL_EP: u32 = 1 << 27;

// ---- QH endpoint capabilities (dword 2) ----
pub const QH_SMASK_SHIFT: u32 = 0; // interrupt schedule mask (start-split µframes), bits 7:0
pub const QH_CMASK_SHIFT: u32 = 8; // split completion mask (complete-split µframes), bits 15:8
pub const QH_HUBADDR_SHIFT: u32 = 16; // TT hub device address, bits 22:16
pub const QH_PORT_SHIFT: u32 = 23; // TT hub downstream port, bits 29:23
/// High-bandwidth multiplier — must be non-zero on every QH the controller executes (EHCI 3.6;
/// 0 is undefined). 1 = one transaction per µframe, the only rate this driver uses.
pub const QH_MULT1: u32 = 1 << 30;

/// Queue Element Transfer Descriptor (EHCI 3.5): 32 bytes, 32-byte aligned. One buffer-page
/// pointer is always enough here — HID descriptors (≤ 64 B) and boot reports (≤ 8 B) never cross
/// a 4 KiB page from a 64-byte-aligned buffer, so buf[1..5] stay zero.
#[repr(C, align(32))]
pub struct Qtd {
    pub next: u32,
    pub alt_next: u32,
    pub token: u32,
    pub buf: [u32; 5],
}

/// Queue Head (EHCI 3.6): 48 bytes, 32-byte aligned. `overlay` is the controller's working copy
/// of the current qTD (next/alt/token/buffers) — priming a transfer = writing `overlay[0]` with
/// the first qTD's physical address and zeroing the overlay token.
#[repr(C, align(32))]
pub struct Qh {
    pub horiz: u32,     // horizontal link pointer (next QH | type, or terminate)
    pub ep_chars: u32,  // dword 1: device addr / endpoint / EPS / DTC / H / MPS / C / RL
    pub ep_caps: u32,   // dword 2: S-mask / C-mask / hub addr / hub port / Mult
    pub current_qtd: u32,
    pub overlay: [u32; 8], // next qTD, alt next, token, 5 buffer pointers
}

/// Assert the identity-map + 32-bit-addressing contract for one DMA allocation. A violation is
/// a bring-up STOP (R9 in the design's pre-registration): panic loudly rather than hand the
/// controller a pointer it will truncate.
fn assert_dma32(phys: u64, align: u64, what: &str) {
    assert!(
        phys != 0 && phys < 0x1_0000_0000 && phys % align == 0,
        "EHCI-HID: {} DMA allocation violates 32-bit/alignment contract: {:#x} (align {})",
        what,
        phys,
        align
    );
}

/// Allocate the 4 KiB periodic frame list (1024 entries), every entry pre-set to Terminate.
/// Returns the physical (== virtual) base.
pub fn alloc_frame_list() -> u64 {
    unsafe {
        let p = alloc_zeroed(Layout::from_size_align(4096, 4096).unwrap()) as *mut u32;
        assert_dma32(p as u64, 4096, "frame list");
        for i in 0..1024 {
            core::ptr::write_volatile(p.add(i), PTR_TERMINATE);
        }
        p as u64
    }
}

/// Allocate one zeroed QH (rounded to 64 B so consecutive QHs never share a cache line).
pub fn alloc_qh() -> *mut Qh {
    unsafe {
        let p = alloc_zeroed(Layout::from_size_align(64, 32).unwrap()) as *mut Qh;
        assert_dma32(p as u64, 32, "QH");
        p
    }
}

/// Allocate one zeroed qTD.
pub fn alloc_qtd() -> *mut Qtd {
    unsafe {
        let p = alloc_zeroed(Layout::from_size_align(32, 32).unwrap()) as *mut Qtd;
        assert_dma32(p as u64, 32, "qTD");
        p
    }
}

/// Allocate a 64-byte transfer buffer (setup packets, descriptors, HID reports — the same
/// 64-byte buffer class the xHCI driver uses).
pub fn alloc_buf64() -> *mut u8 {
    unsafe {
        let p = alloc_zeroed(Layout::from_size_align(64, 64).unwrap());
        assert_dma32(p as u64, 64, "buffer");
        p
    }
}

/// Fill one qTD in place: `total` bytes at `buf_phys` (0 for a zero-length status stage), PID +
/// data-toggle + IOC from `flags`, next pointer chained or terminated. Marks it Active.
pub unsafe fn write_qtd(qtd: *mut Qtd, next_phys: u32, flags: u32, total: u32, buf_phys: u64) {
    (*qtd).next = next_phys;
    (*qtd).alt_next = PTR_TERMINATE;
    (*qtd).buf = [buf_phys as u32, 0, 0, 0, 0];
    // Token written LAST (volatile) so the controller never sees a half-built descriptor.
    core::ptr::write_volatile(
        &mut (*qtd).token,
        QTD_ACTIVE | QTD_CERR3 | (total << QTD_TOTAL_SHIFT) | flags,
    );
}
