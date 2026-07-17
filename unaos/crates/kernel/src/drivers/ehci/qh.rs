// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EHCI-3 schedule structures — Queue Heads, qTDs, the periodic frame list, and small DMA
//! buffers. All controller-visible memory lives in STATIC pools inside the kernel image (the
//! driver's DMA needs are small and fixed), and every pointer handed to the controller is
//! resolved through the live page tables (`phys_of` → translate()) — never the heap's
//! virt==phys shortcut. Probe-3/5 history: heap-resident schedules at ~538 MB HSE-master-
//! aborted on the metal Panther Point EHCI (while xHCI DMA'd the same region fine); the
//! kernel-image pools also probe the low-physical range firmware's own EHCI DMA used. All
//! addresses are enforced < 4 GiB (CTRLDSSEGMENT is pinned to 0).

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

/// Resolve one DMA object's PHYSICAL address via the live page tables and enforce the
/// 32-bit + alignment contract (Panther Point DMA must be < 4 GiB; CTRLDSSEGMENT=0). Returns
/// `None` (caller traces + skips, R9 discipline) instead of handing the controller a pointer
/// it would truncate or that isn't mapped. Objects are <= 4 KiB and suitably aligned, so a
/// single translate of the base covers the whole object (nothing straddles a page).
pub fn phys_of<T>(p: *const T, align: u64) -> Option<u64> {
    let phys = crate::arch::x86_64::memory::translate(p as u64)?;
    if phys == 0 || phys >= 0x1_0000_0000 || phys % align != 0 {
        return None;
    }
    Some(phys)
}

/// A 64-byte-aligned transfer buffer for the static pool.
#[repr(C, align(64))]
pub struct Buf64(pub [u8; 64]);

/// A 256-byte, 64-byte-aligned control-DATA buffer (EHCI-4 M2). The EP0 data stage reads into
/// this: enumeration descriptors stay ≤ 64 B (unchanged), but a HID **report descriptor**
/// (the trackpad path) is routinely larger — this holds one so the report parser sees the whole
/// thing and the doc can capture it verbatim. Still < one 4 KiB page from the page-aligned pool,
/// so a single qTD buffer-page pointer covers it (buf[1..5] stay zero); < 4 GiB by construction.
#[repr(C, align(64))]
pub struct Buf256(pub [u8; 256]);

/// One statically-allocated interrupt-endpoint slot (QH + single re-armed qTD + report buffer).
#[repr(C)]
pub struct IntSlot {
    pub qh: Qh,
    pub qtd: Qtd,
    pub buf: Buf64,
}

/// Static DMA pool for one controller — probe-5 metal experiment AND the permanent shape: the
/// driver's DMA needs are small and fixed, so they live in the kernel image (low physical
/// memory, where firmware's own EHCI DMA demonstrably worked) instead of the heap. All
/// controller-visible pointers are derived via translate() rather than the heap's virt==phys
/// shortcut, so this is correct under any kernel mapping.
#[repr(C, align(4096))]
pub struct DmaPool {
    pub frame_list: [u32; 1024], // 4 KiB, 4 KiB-aligned (leads the struct)
    /// The async reclamation-list HEAD (H=1): a permanently-INACTIVE dummy, exactly as Linux/
    /// EDK2 shape it. Probe-7 metal finding: executing transfers ON the self-linked H-QH
    /// (spec-legal, QEMU-tolerated) master-aborts Panther Point's async engine — the H-QH is
    /// load-bearing for its empty-list reclamation logic and must stay idle.
    pub qh_head: Qh,
    /// The work QH the EP0 transfers actually run on, linked behind the head.
    pub qh_ctrl: Qh,
    pub qtd_setup: Qtd,
    pub qtd_data: Qtd,
    pub qtd_status: Qtd,
    pub setup_buf: Buf64,
    /// EHCI-4 M2: 256 B so the EP0 data stage can read a full HID report descriptor (the trackpad
    /// report path), not just the ≤ 64 B enumeration descriptors. Behaviour-neutral for the small
    /// reads — they simply do not use the extra room.
    pub data_buf: Buf256,
    pub int_slots: [IntSlot; 4],
}

pub const MAX_CONTROLLERS: usize = 2;
pub const MAX_INT_EPS: usize = 4;

/// The pools (one per EHCI function; the 2012 rMBP has exactly two). Extra functions beyond
/// MAX_CONTROLLERS are skipped with a trace by the caller.
pub static mut DMA_POOLS: [DmaPool; MAX_CONTROLLERS] = unsafe { core::mem::zeroed() };

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
