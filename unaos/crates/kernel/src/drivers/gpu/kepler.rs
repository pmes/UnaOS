use super::detect::GpuInfo;
use crate::drivers::pci::PciScanner;

pub mod regs {
    // PMC — Master Control
    pub const NV_PMC_BOOT_0: usize = 0x0000_0000;   // chip ID, stepping
    pub const NV_PMC_BOOT_1: usize = 0x0000_0004;   // revision
    pub const NV_PMC_ENABLE: usize = 0x0000_0200;   // engine enable mask
    pub const NV_PMC_INTR_0: usize = 0x0000_0100;   // interrupt status
    pub const NV_PMC_INTR_EN: usize = 0x0000_0140;  // interrupt enable

    // PBUS — Bus Control
    pub const NV_PBUS_PCI_NV_0: usize = 0x0000_1800; // PCI vendor/device mirror
    pub const NV_PBUS_PCI_NV_1: usize = 0x0000_1804; // PCI status/command mirror

    // PFB — Framebuffer (VRAM) Controller
    pub const NV_PFB_BASE: usize = 0x0010_0000;
    pub const NV_PFB_RAM_AMOUNT: usize = 0x0010_F20C; // Kepler VRAM size register in MB (PBFB_BROADCAST + MEM_AMOUNT)

    // PFIFO — Command Submission / Pushbuffer
    pub const NV_PFIFO_BASE: usize = 0x0000_2000;

    // PGRAPH — 2D/3D Graphics Engine
    pub const NV_PGRAPH_BASE: usize = 0x0040_0000;

    // PDISPLAY — Display Engine
    pub const NV_PDISPLAY_BASE: usize = 0x0061_0000;
    pub const NV_PDISPLAY_SIZE: usize = 0x0001_0000; // Scan 64KB of display engine regs

    /// Host register offset (from the Falcon unit base) → Falcon IO index.
    ///
    /// Microcode reaches the unit registers with `iowr`/`iord` on IO space, NOT
    /// at the host MMIO offset. Metal-proven at s29: MAILBOX0 (host `+0x040`)
    /// written via `iowrs I[0x1000]` returned the exact authored `F00DFACE`.
    /// This has been derived wrong twice (pull 25's flat `0x40`, pull 33's raw
    /// `0x800`/`0x804`) — derive ucode port immediates here, never by hand.
    /// See `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §3.
    pub const fn falcon_io(host_off: u32) -> u32 {
        (host_off & 0xffc) << 6
    }

    // The two mappings metal has proven, checked at compile time.
    const _: () = assert!(falcon_io(0x040) == 0x1000); // MAILBOX0, s29
    const _: () = assert!(falcon_io(0x044) == 0x1100); // MAILBOX1, s30 heartbeat
    const _: () = assert!(falcon_io(0x800) == 0x20000); // CC_SCRATCH[0], s37
    const _: () = assert!(falcon_io(0x804) == 0x20100); // CC_SCRATCH[1], s37
}

/// FECS microcode images, byte-exact.
///
/// Falcon instructions are variable-length byte sequences; IMEM is written a
/// `u32` at a time. The **byte listing is authoritative** here — the packed
/// `[u32]` images are produced from it by [`pack92`] at compile time, so the
/// listing in `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` and the words the
/// host uploads cannot drift apart. Every IO port immediate in the listing is
/// checked against [`regs::falcon_io`] by const assertion below (§3 of the
/// spec: derive the port, never hand-write it).
///
/// Pull 34 (R3-AMEND) retrofit of the s37-acked pull-33 echo skeleton:
///   * the ucode reports the **value it read** into MAILBOX0 (`I[0x1000]`), so
///     the ack is no longer a single undifferentiated observable — a stuck
///     `CC_SCRATCH[1]` and a genuine echo now look different;
///   * a **phase counter** is stamped into MAILBOX1 (`I[0x1100]`) before and
///     after each risky IO step, so a fault localises to an instruction;
///   * the poll loop is **bounded** (spec §5.1 — the standing defect raised at
///     land review `bc5fe3fc`), and exit-by-bound reports a distinct phase.
pub mod ucode {
    use super::regs::falcon_io;

    /// Falcon IO indices, derived — never hand-written (spec §3).
    pub const IO_MAILBOX0: u32 = falcon_io(0x040);
    pub const IO_MAILBOX1: u32 = falcon_io(0x044);
    pub const IO_CC_SCRATCH0: u32 = falcon_io(0x800);
    pub const IO_CC_SCRATCH1: u32 = falcon_io(0x804);

    /// Iteration bound on the echo poll loop.
    ///
    /// Chosen as `0x0010_0000` = 1_048_576 iterations. Sizing argument: the
    /// loop body is 8 instructions, so the bound is ~8.4M Falcon instructions —
    /// milliseconds of Falcon time. The host writes the command word one
    /// `mmio_write` plus one `serial_println!` after `CPUCTL <= 2`, and at s37
    /// the ucode had already consumed it by the host's *first* poll
    /// (`iters=0`). The bound is therefore ~3 orders of magnitude larger than
    /// the window it must cover: it exists to guarantee termination (spec
    /// §5.1), not to be reached. If `phase` ever comes back as
    /// [`PHASE_A_BOUND`]/[`PHASE_B_BOUND`], the command never arrived — that is
    /// a real finding, not a tuning problem.
    pub const ECHO_BOUND: u32 = 0x0010_0000;

    // Phase stamps. Image A uses 0x01..0x04, image B 0x11..0x14, so MAILBOX1
    // alone names which image ran (pull 25 distinct-magic discipline).
    pub const PHASE_A_PRELOOP: u8 = 0x01;
    pub const PHASE_A_POSTREAD: u8 = 0x02;
    pub const PHASE_A_PREACK: u8 = 0x03;
    pub const PHASE_A_POSTACK: u8 = 0x04;
    pub const PHASE_A_BOUND: u8 = 0xBD;
    pub const PHASE_B_PRELOOP: u8 = 0x11;
    pub const PHASE_B_POSTREAD: u8 = 0x12;
    pub const PHASE_B_PREACK: u8 = 0x13;
    pub const PHASE_B_POSTACK: u8 = 0x14;
    pub const PHASE_B_BOUND: u8 = 0xBE;

    /// Image A — indexed IO ports (s37-proven prologue), **down-counting**
    /// bound via `sub b32 $r5, 0x1`.
    ///
    /// ```text
    /// // Addr | Bytes       | Instruction         | Note
    /// // -----|-------------|---------------------|-------------------------------------
    /// // 0x00 | f0 17 00    | mov   $r1, 0x00     | low half of I[CC_SCRATCH[0]]
    /// // 0x03 | f0 13 02    | sethi $r1, 0x02     | $r1 = 0x20000                (s37)
    /// // 0x06 | f1 27 00 01 | mov   $r2, 0x0100   | low half of I[CC_SCRATCH[1]]
    /// // 0x0a | f0 23 02    | sethi $r2, 0x02     | $r2 = 0x20100                (s37)
    /// // 0x0d | f0 37 01    | mov   $r3, 0x1      | the ack value
    /// // 0x10 | f1 67 00 10 | mov   $r6, 0x1000   | $r6 = I[MAILBOX0]            (s29)
    /// // 0x14 | f1 77 00 11 | mov   $r7, 0x1100   | $r7 = I[MAILBOX1]            (s30)
    /// // 0x18 | f0 57 00    | mov   $r5, 0x00     | loop counter, low half
    /// // 0x1b | f1 53 10 00 | sethi $r5, 0x0010   | $r5 = 0x00100000 = ECHO_BOUND
    /// // 0x1f | f0 07 01    | mov   $r0, 0x01     | phase 0x01
    /// // 0x22 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-loop)
    /// // poll:
    /// // 0x25 | cf 14 00    | iord  $r4, I[$r1]   | RISKY: read the command word
    /// // 0x28 | d0 64 00    | iowr  I[$r6], $r4   | MAILBOX0 = VALUE READ  <-- split obs.
    /// // 0x2b | f0 07 02    | mov   $r0, 0x02     |
    /// // 0x2e | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-read)
    /// // 0x31 | b0 44 01    | cmpu b32 $r4, 0x1   | is it the test command?
    /// // 0x34 | f4 1b 14    | bra ne, +0x14       | -> 0x48 (dec), keep polling
    /// // 0x37 | f0 07 03    | mov   $r0, 0x03     |
    /// // 0x3a | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-ack)
    /// // 0x3d | d0 23 00    | iowr  I[$r2], $r3   | RISKY: CC_SCRATCH[1] = 1 (ACK)
    /// // 0x40 | f0 07 04    | mov   $r0, 0x04     |
    /// // 0x43 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-ack)
    /// // 0x46 | f8 02       | exit                | terminal state: phase=04
    /// // dec:
    /// // 0x48 | b0 52 01    | sub  b32 $r5, 0x1   | A's variable: subop 2
    /// // 0x4b | b0 54 00    | cmpu b32 $r5, 0x0   |
    /// // 0x4e | f4 1b d7    | bra ne, -0x29       | -> 0x25 (poll)
    /// // 0x51 | f0 07 bd    | mov   $r0, 0xbd     | EXIT BY BOUND
    /// // 0x54 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = 0xBD
    /// // 0x57 | f8 02       | exit                |
    /// // 0x59 | 00 00 00    | (padding)           | 92 bytes = 23 words
    /// ```
    #[rustfmt::skip]
    pub const ECHO_A_BYTES: [u8; 92] = [
        0xf0, 0x17, 0x00,             // mov   $r1, 0x00
        0xf0, 0x13, 0x02,             // sethi $r1, 0x02
        0xf1, 0x27, 0x00, 0x01,       // mov   $r2, 0x0100
        0xf0, 0x23, 0x02,             // sethi $r2, 0x02
        0xf0, 0x37, 0x01,             // mov   $r3, 0x1
        0xf1, 0x67, 0x00, 0x10,       // mov   $r6, 0x1000
        0xf1, 0x77, 0x00, 0x11,       // mov   $r7, 0x1100
        0xf0, 0x57, 0x00,             // mov   $r5, 0x00
        0xf1, 0x53, 0x10, 0x00,       // sethi $r5, 0x0010
        0xf0, 0x07, 0x01,             // mov   $r0, 0x01
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xcf, 0x14, 0x00,             // poll: iord $r4, I[$r1]
        0xd0, 0x64, 0x00,             // iowr  I[$r6], $r4
        0xf0, 0x07, 0x02,             // mov   $r0, 0x02
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xb0, 0x44, 0x01,             // cmpu b32 $r4, 0x1
        0xf4, 0x1b, 0x14,             // bra ne, +0x14 -> dec
        0xf0, 0x07, 0x03,             // mov   $r0, 0x03
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xd0, 0x23, 0x00,             // iowr  I[$r2], $r3   (ACK)
        0xf0, 0x07, 0x04,             // mov   $r0, 0x04
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xb0, 0x52, 0x01,             // dec: sub b32 $r5, 0x1
        0xb0, 0x54, 0x00,             // cmpu b32 $r5, 0x0
        0xf4, 0x1b, 0xd7,             // bra ne, -0x29 -> poll
        0xf0, 0x07, 0xbd,             // mov   $r0, 0xbd
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0x00, 0x00, 0x00,             // padding
    ];

    /// Image B — the A/B fallback on the one **new** encoding this pull needs.
    ///
    /// A and B are byte-identical except for the counter arithmetic: the
    /// `sub`/`add` subopcode in the `0xb0` (b32, register + I8 immediate) form
    /// is the only instruction here that metal has not already run. `cmpu` is
    /// metal-proven at subop 4 (s37, `b0 44 01`), which fixes the subop table
    /// as `0=add, 1=adc, 2=sub, 3=sbb, 4=cmpu`. A takes `sub` (subop 2) and
    /// counts DOWN from `+ECHO_BOUND`; B takes `add` (subop 0, the least
    /// disputable entry of any ALU table) and counts UP from `-ECHO_BOUND`
    /// (`0xFFF0_0000`). Both exit the loop when `$r5 == 0`, both bound at
    /// exactly `ECHO_BOUND` iterations. B also carries the 0x1x phase stamps so
    /// MAILBOX1 names the winner without reference to the ack.
    ///
    /// Deltas from image A (same addresses — every substituted instruction is
    /// the same length):
    ///
    /// ```text
    /// // 0x1b | f1 53 f0 ff | sethi $r5, 0xfff0   | $r5 = -ECHO_BOUND
    /// // 0x21 |          11 | mov   $r0, 0x11     | phase (pre-loop)
    /// // 0x2d |          12 | mov   $r0, 0x12     | phase (post-read)
    /// // 0x39 |          13 | mov   $r0, 0x13     | phase (pre-ack)
    /// // 0x42 |          14 | mov   $r0, 0x14     | phase (post-ack)
    /// // 0x48 | b0 50 01    | add b32 $r5, 0x1    | B's variable: subop 0
    /// // 0x53 |          be | mov   $r0, 0xbe     | EXIT BY BOUND
    /// ```
    #[rustfmt::skip]
    pub const ECHO_B_BYTES: [u8; 92] = [
        0xf0, 0x17, 0x00,
        0xf0, 0x13, 0x02,
        0xf1, 0x27, 0x00, 0x01,
        0xf0, 0x23, 0x02,
        0xf0, 0x37, 0x01,
        0xf1, 0x67, 0x00, 0x10,
        0xf1, 0x77, 0x00, 0x11,
        0xf0, 0x57, 0x00,
        0xf1, 0x53, 0xf0, 0xff,       // sethi $r5, 0xfff0  (= -ECHO_BOUND)
        0xf0, 0x07, 0x11,             // mov   $r0, 0x11
        0xd0, 0x70, 0x00,
        0xcf, 0x14, 0x00,             // poll:
        0xd0, 0x64, 0x00,
        0xf0, 0x07, 0x12,             // mov   $r0, 0x12
        0xd0, 0x70, 0x00,
        0xb0, 0x44, 0x01,
        0xf4, 0x1b, 0x14,
        0xf0, 0x07, 0x13,             // mov   $r0, 0x13
        0xd0, 0x70, 0x00,
        0xd0, 0x23, 0x00,             // (ACK)
        0xf0, 0x07, 0x14,             // mov   $r0, 0x14
        0xd0, 0x70, 0x00,
        0xf8, 0x02,
        0xb0, 0x50, 0x01,             // dec: add b32 $r5, 0x1
        0xb0, 0x54, 0x00,
        0xf4, 0x1b, 0xd7,
        0xf0, 0x07, 0xbe,             // mov   $r0, 0xbe
        0xd0, 0x70, 0x00,
        0xf8, 0x02,
        0x00, 0x00, 0x00,
    ];

    /// Pack a 92-byte Falcon instruction stream into the 23 little-endian
    /// `u32` words IMEMD expects.
    pub const fn pack92(b: &[u8; 92]) -> [u32; 23] {
        let mut out = [0u32; 23];
        let mut w = 0;
        while w < 23 {
            let i = w * 4;
            out[w] = (b[i] as u32)
                | ((b[i + 1] as u32) << 8)
                | ((b[i + 2] as u32) << 16)
                | ((b[i + 3] as u32) << 24);
            w += 1;
        }
        out
    }

    pub const UCODE_CTX_ECHO_A: [u32; 23] = pack92(&ECHO_A_BYTES);
    pub const UCODE_CTX_ECHO_B: [u32; 23] = pack92(&ECHO_B_BYTES);

    /// Reconstruct a `mov`(I8)+`sethi`(I8) port pair from the byte listing.
    const fn port_i8_sethi_i8(b: &[u8; 92], mov_at: usize, sethi_at: usize) -> u32 {
        (b[mov_at + 2] as u32) | ((b[sethi_at + 2] as u32) << 16)
    }
    /// Reconstruct a `mov`(I16) port immediate from the byte listing.
    const fn port_i16(b: &[u8; 92], mov_at: usize) -> u32 {
        (b[mov_at + 2] as u32) | ((b[mov_at + 3] as u32) << 8)
    }

    // ⭐ Spec §3: every port immediate in both images is the derived value.
    // These are the assertions that would have caught pull 33's raw-host-offset
    // listing at compile time instead of at proposal review.
    const _: () = assert!(port_i8_sethi_i8(&ECHO_A_BYTES, 0x00, 0x03) == IO_CC_SCRATCH0);
    const _: () = assert!(
        (port_i16(&ECHO_A_BYTES, 0x06) | ((ECHO_A_BYTES[0x0c] as u32) << 16)) == IO_CC_SCRATCH1
    );
    const _: () = assert!(port_i16(&ECHO_A_BYTES, 0x10) == IO_MAILBOX0);
    const _: () = assert!(port_i16(&ECHO_A_BYTES, 0x14) == IO_MAILBOX1);
    const _: () = assert!(port_i8_sethi_i8(&ECHO_B_BYTES, 0x00, 0x03) == IO_CC_SCRATCH0);
    const _: () = assert!(
        (port_i16(&ECHO_B_BYTES, 0x06) | ((ECHO_B_BYTES[0x0c] as u32) << 16)) == IO_CC_SCRATCH1
    );
    const _: () = assert!(port_i16(&ECHO_B_BYTES, 0x10) == IO_MAILBOX0);
    const _: () = assert!(port_i16(&ECHO_B_BYTES, 0x14) == IO_MAILBOX1);

    // The s37-acked prologue is preserved word-for-word (the four words that
    // executed on metal at sitting #37 must not have moved).
    const _: () = assert!(UCODE_CTX_ECHO_A[0] == 0xf000_17f0);
    const _: () = assert!(UCODE_CTX_ECHO_A[1] == 0x27f1_0213);
    const _: () = assert!(UCODE_CTX_ECHO_A[2] == 0x23f0_0100);
    const _: () = assert!(UCODE_CTX_ECHO_A[3] == 0x0137_f002);

    // The counter is initialised to exactly ECHO_BOUND (A) / -ECHO_BOUND (B).
    const _: () = assert!(((ECHO_A_BYTES[0x1d] as u32) | ((ECHO_A_BYTES[0x1e] as u32) << 8)) << 16 == ECHO_BOUND);
    const _: () = assert!(((ECHO_B_BYTES[0x1d] as u32) | ((ECHO_B_BYTES[0x1e] as u32) << 8)) << 16 == ECHO_BOUND.wrapping_neg());

    // A and B differ only in the counter arithmetic, the counter seed and the
    // phase magics — nothing else may drift between the pair.
    const _: () = {
        let mut i = 0;
        while i < 92 {
            let allowed = i == 0x1d || i == 0x1e   // counter seed (sethi imm)
                || i == 0x21 || i == 0x2d || i == 0x39 || i == 0x42 || i == 0x53 // phase magics
                || i == 0x49; // sub(2) vs add(0) subopcode
            assert!(allowed || ECHO_A_BYTES[i] == ECHO_B_BYTES[i]);
            i += 1;
        }
    };
}

/// s26/s28 FTDI-ring budget: the 0x640000 window is PARKED (triple-refuted),
/// and its four 256-row dumps cost ~54 KiB of the 64 KiB drop-oldest boot ring
/// (drivers/xhci/ftdi.rs) — enough to evict the display and ucode legs from the
/// capture. Values are still collected and summarised; only the dense rows are
/// silenced. Flip to re-enable the raw dumps.
const MIRROR_HDR_DENSE: bool = false;

pub fn init(gpu: &GpuInfo) {
    serial_println!("[NVIDIA] Initializing Kepler GPU at BDF {}:{}:{}", gpu.bus, gpu.slot, gpu.func);

    // 1. Enable Bus Master and Memory Space
    PciScanner::enable_bus_master(gpu.bus, gpu.slot, gpu.func);

    let bar0 = gpu.bar0_phys as usize;

    let mut bar0_size = 0;
    let mut bar1_base = 0;
    let mut bar1_size = 0;

    unsafe {
        let cmd = crate::arch::pci::read_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04);
        crate::arch::pci::write_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04, cmd & !0x02);

        let bar0_orig = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10);
        crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10, 0xFFFFFFFF);
        let bar0_val = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10);
        crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x10, bar0_orig);
        if bar0_val != 0 && bar0_val != 0xFFFFFFFF {
            bar0_size = (!(bar0_val & !0xF)).wrapping_add(1) as usize;
        }

        let bar1_orig_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
        
        if (bar1_orig_lo & 0x1) != 0 {
            serial_println!("[NVIDIA] Error: BAR1 is I/O space. Probe aborted.");
            serial_println!(":: kepler: probe-abort bar0-unmapped ::");
            return;
        }

        let is_64bit = ((bar1_orig_lo >> 1) & 0x3) == 0x2;
        
        if is_64bit {
            let bar1_orig_hi = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
            bar1_base = (bar1_orig_lo & 0xFFFFFFF0) as usize | ((bar1_orig_hi as usize) << 32);

            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, 0xFFFFFFFF);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18, 0xFFFFFFFF);
            let bar1_val_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
            let bar1_val_hi = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, bar1_orig_lo);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x18, bar1_orig_hi);
            
            let bar1_val = (bar1_val_lo & 0xFFFFFFF0) as u64 | ((bar1_val_hi as u64) << 32);
            if bar1_val != 0 {
                bar1_size = (!bar1_val).wrapping_add(1) as usize;
            }
        } else {
            bar1_base = (bar1_orig_lo & 0xFFFFFFF0) as usize;
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, 0xFFFFFFFF);
            let bar1_val_lo = crate::arch::pci::read_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14);
            crate::arch::pci::write_config_32(gpu.bus as u8, gpu.slot, gpu.func, 0x14, bar1_orig_lo);
            
            if bar1_val_lo != 0 && bar1_val_lo != 0xFFFFFFFF {
                bar1_size = (!(bar1_val_lo & 0xFFFFFFF0)).wrapping_add(1) as usize;
            }
        }

        crate::arch::pci::write_config_16(gpu.bus as u8, gpu.slot, gpu.func, 0x04, cmd);
    }

    if bar0_size == 0 || bar1_size == 0 {
        serial_println!("[NVIDIA] Error: Invalid BAR sizes (BAR0: {} bytes, BAR1: {} bytes). Probe aborted.", bar0_size, bar1_size);
        serial_println!(":: kepler: probe-abort bar1-unmapped ::");
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::memory::map_mmio_window(bar0 as u64, bar0_size);
        crate::arch::memory::map_mmio_window(bar1_base as u64, bar1_size);
        if crate::arch::memory::translate(bar0 as u64).is_none() {
            serial_println!("[NVIDIA] Error: BAR0 physical address (0x{:X}) is not mapped in the identity map. Probe aborted.", bar0);
            serial_println!(":: kepler: probe-abort bar1-not-64bit ::");
            return;
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    {
        serial_println!("[NVIDIA] Error: BAR0 mapping unimplemented on aarch64. Probe aborted.");
        serial_println!(":: kepler: probe-abort bar0-unmapped ::");
        return;
    }


    unsafe {
        // 2. Read NV_PMC_BOOT_0 to identify chip
        let boot_0 = mmio_read(bar0, regs::NV_PMC_BOOT_0);
        let chipset = (boot_0 >> 20) & 0xFF;
        let major = (boot_0 >> 16) & 0xF;
        let minor = boot_0 & 0xFFFF;
        serial_println!("[NVIDIA] Chipset: 0x{:02X}, Stepping: {}.{}", chipset, major, minor);

        if chipset != 0xE7 {
            serial_println!("[NVIDIA] Warning: Expected GK107 (0xE7), found 0x{:02X}", chipset);
        }

        // 3. Verify POST
        let pmc_enable = mmio_read(bar0, regs::NV_PMC_ENABLE);
        serial_println!("[NVIDIA] PMC Enable: 0x{:08X}", pmc_enable);
        if pmc_enable == 0 {
            serial_println!("[NVIDIA] Warning: GPU does not appear to be POST'd (PMC_ENABLE is 0)");
        }

        // 4. Disable Interrupts
        mmio_write(bar0, regs::NV_PMC_INTR_EN, 0);
        serial_println!("[NVIDIA] Disabled interrupts via PMC_INTR_EN");

        // 5. VRAM Detection & Initialization
        let vram_size_mb = mmio_read(bar0, regs::NV_PFB_RAM_AMOUNT) as usize;
        let vram_size = vram_size_mb * 1024 * 1024;
        
        let is_power_of_two = vram_size_mb.is_power_of_two();
        let is_3n_over_4 = (vram_size_mb % 3 == 0) && (vram_size_mb / 3 * 4).is_power_of_two();

        if vram_size < 16 * 1024 * 1024 || vram_size > 32usize * 1024 * 1024 * 1024 || (!is_power_of_two && !is_3n_over_4) {
            serial_println!("[NVIDIA] Error: Absurd VRAM size reported ({} MB). Probe aborted.", vram_size_mb);
            serial_println!(":: kepler: probe-abort vram-size-invalid ::");
            return;
        }
        serial_println!("[NVIDIA] PFB Reported VRAM Size: {} MB", vram_size_mb);

        let mut vram_allocator = VramAllocator::new(bar1_base, bar1_size, vram_size);
        serial_println!("[NVIDIA] Initialized VRAM bump allocator. Total BAR1 visible: {} MB", vram_allocator.total_size >> 20);

        // 6. Display Engine — read-only trace + optional takeover
        let mut kdisp_trace = [0u32; 7];
        
        // Milestone 1: Method-Mirror Backing-Store Beacon Test - Pre-Takeover Dump
        let mut mirror_hdr_pre = [0u32; 256];
        for (i, offset) in (0..=0x3FC).step_by(4).enumerate() {
            let val = mmio_read(bar0, 0x640000 + offset);
            mirror_hdr_pre[i] = val;
            if MIRROR_HDR_DENSE { serial_println!(":: kepler: mirror-hdr pre off={:03X} val={:08X} ::", offset, val); }
        }
        serial_println!(":: kepler: mirror-hdr pre done rows=256 ::");

        let fb_offset = crate::drivers::gpu::kepler_display::takeover_display(
            gpu, bar0, &mut vram_allocator, &mut kdisp_trace,
        );
        serial_println!(":: kdisp: landed trace [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
            kdisp_trace[0], kdisp_trace[1], kdisp_trace[2], kdisp_trace[3],
            kdisp_trace[4], kdisp_trace[5], kdisp_trace[6]);

        // 7. PGRAPH 2D/3D Engine Init (Placeholder)
        // Kepler requires Falcon microcode to fully initialize PGRAPH.
        // We log its presence but leave it disabled to prevent hangs.
        let pgraph_status = mmio_read(bar0, regs::NV_PGRAPH_BASE);
        serial_println!("[NVIDIA] PGRAPH Engine Status (0x400000): 0x{:08X}. Requires firmware for full 2D/3D.", pgraph_status);

        // 8. Phase 4: 3D Foundation - PFIFO and Pushbuffer setup
        if cfg!(feature = "nvidia-kepler-fifo") {
            serial_println!("[NVIDIA] Starting PFIFO initialization...");
            
            // Enable PFIFO and SUBFIFO (PBDMA) in PMC
            let pmc_enable = mmio_read(bar0, regs::NV_PMC_ENABLE);
            mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_enable | 0x100);
            
            // GK104 PBDMA enable (NV_PMC_SUBFIFO_ENABLE at 0x204 in pmc.xml)
            mmio_write(bar0, 0x000204, 0xFFFFFFFF);
            let pbdma_count_mask = mmio_read(bar0, 0x000204);
            let pbdma_count = pbdma_count_mask.count_ones();
            serial_println!(":: kepler: pbdma-count {} ::", pbdma_count);

            // Bind PBDMA 0 to Engine 0 (PGRAPH) by writing mask `1` to SUBFIFO_ENG_MASK[0]
            // According to gf100_pfifo.xml, SUBFIFO_ENG_MASK is at offset 0x390 relative to PFIFO (0x2000).
            let pfifo_base = 0x2000;
            mmio_write(bar0, pfifo_base + 0x390, 1 << 0);
            serial_println!(":: kepler: pbdma-eng-mask set ::");

            let check = mmio_read(bar0, regs::NV_PMC_ENABLE);
            serial_println!("[NVIDIA] NV_PMC_ENABLE after bit 8 set: 0x{:08X}", check);

            if let Some(inst_off) = vram_allocator.alloc(0x1000) {
                if let Some(gpfifo_off) = vram_allocator.alloc(0x1000) {
                    if let Some(userd_off) = vram_allocator.alloc(0x1000) {
                        if let Some(pb_off) = vram_allocator.alloc(64 * 1024) {
                            if let Some(runlist_off) = vram_allocator.alloc(0x1000) {
                                if let Some(fence_off) = vram_allocator.alloc(0x1000) {
                                    serial_println!("[NVIDIA] Allocated Channel Instance, GPFIFO, USERD, PushBuffer, Runlist, Fence.");

                                    let bar1 = vram_allocator.base_phys;
                                    
                                    // Zero memory
                                    for i in 0..(0x1000 / 4) {
                                        unsafe {
                                            core::ptr::write_volatile((bar1 + inst_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + gpfifo_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + userd_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, 0);
                                            core::ptr::write_volatile((bar1 + fence_off + i * 4) as *mut u32, 0);
                                        }
                                    }

                                    let chan_id = 1;

                                    // Setup Channel Instance Block
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + inst_off + 0x08) as *mut u32, (userd_off & 0xFFFFFFFF) as u32);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, ((userd_off >> 32) as u32) | 0x80000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x10) as *mut u32, 0x0000face);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x30) as *mut u32, 0xfffff902);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x48) as *mut u32, (gpfifo_off & 0xFFFFFFFF) as u32);
                                        // limit2 = ORDER 9 (512 entries)
                                        core::ptr::write_volatile((bar1 + inst_off + 0x4C) as *mut u32, ((gpfifo_off >> 32) as u32) | (9 << 16));
                                        core::ptr::write_volatile((bar1 + inst_off + 0x84) as *mut u32, 0x20400000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0x94) as *mut u32, 0x30000000); // VRAM devm=0
                                        core::ptr::write_volatile((bar1 + inst_off + 0x9C) as *mut u32, 0x00000100);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xAC) as *mut u32, 0x0000001f);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xE4) as *mut u32, 0x00000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xE8) as *mut u32, chan_id);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xB8) as *mut u32, 0xf8000000);
                                        core::ptr::write_volatile((bar1 + inst_off + 0xF8) as *mut u32, 0x10003080); // 0x002310
                                        core::ptr::write_volatile((bar1 + inst_off + 0xFC) as *mut u32, 0x10000010); // 0x002350
                                    }

                                    // Witness instance block raws
                                    let ib_08 = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x08) as *const u32) };
                                    let ib_0c = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x0C) as *const u32) };
                                    let ib_48 = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x48) as *const u32) };
                                    let ib_4c = unsafe { core::ptr::read_volatile((bar1 + inst_off + 0x4C) as *const u32) };
                                    serial_println!(":: kepler: inst-raw 08={:08X} 0C={:08X} 48={:08X} 4C={:08X} ::", ib_08, ib_0c, ib_48, ib_4c);

                                    let chid_0 = 1;
                                    let chid_1 = 2;
                                    let chid_2 = 3;
                                    let entry_0 = chid_0;
                                    let entry_1 = chid_1 | (1 << 31);
                                    let entry_2 = (chid_2 << 1) | 1;

                                    // 1. Write Runlist VRAM FIRST
                                    unsafe {
                                        core::ptr::write_volatile((bar1 + runlist_off) as *mut u32, entry_0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 4) as *mut u32, 0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 8) as *mut u32, entry_1);
                                        core::ptr::write_volatile((bar1 + runlist_off + 12) as *mut u32, 0);
                                        core::ptr::write_volatile((bar1 + runlist_off + 16) as *mut u32, entry_2);
                                        core::ptr::write_volatile((bar1 + runlist_off + 20) as *mut u32, 0);
                                    }

                                    let _read_sched_status = |label: &str| {
                                        let err = mmio_read(bar0, 0x252c);
                                        let stat = mmio_read(bar0, 0x263c);
                                        let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                        serial_println!(":: kepler: sched-status {} err={:08X} ({}) stat={:08X} ::", label, err, err_str, stat);
                                    };

                                    // Milestone 1: Method-Mirror Backing-Store Beacon Test
                                    // Pass 0: Baseline dump
                                    let mut rows = 0;
                                    let mut diff_found = false;
                                    for (i, offset) in (0..=0x3FC).step_by(4).enumerate() {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        let pre_val = mirror_hdr_pre[i];
                                        if MIRROR_HDR_DENSE { serial_println!(":: kepler: mirror-hdr pass0 off={:03X} val={:08X} ::", offset, val); }
                                        if val != pre_val {
                                            serial_println!(":: kepler: latch-delta off={:03X} pre={:08X} post={:08X} ::", offset, pre_val, val);
                                            diff_found = true;
                                        }
                                        rows += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass0 done rows={} ::", rows);
                                    if !diff_found {
                                        serial_println!(":: kepler: latch-delta none ::");
                                    }

                                    // Plant Beacons
                                    let pattern = [
                                        0xBEAC0001, 0xBEAC0002, 0xBEAC0003, 0xBEAC0004,
                                        0xBEAC0005, 0xBEAC0006, 0xBEAC0007, 0xBEAC0008,
                                    ];
                                    
                                    unsafe {
                                        // userd
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + userd_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=userd off={:08X} ::", userd_off);
                                        
                                        // pushbuffer
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + pb_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=pb off={:08X} ::", pb_off);
                                        
                                        // runlist
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=runlist off={:08X} ::", runlist_off);
                                    }

                                    // Pass 1: Post-Plant Dump & Scan
                                    let mut rows_pass1 = 0;
                                    let mut beacons_seen = 0;
                                    for offset in (0..=0x3FC).step_by(4) {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        if MIRROR_HDR_DENSE { serial_println!(":: kepler: mirror-hdr pass1 off={:03X} val={:08X} ::", offset, val); }
                                        if val >= 0xBEAC0001 && val <= 0xBEAC0008 {
                                            serial_println!(":: kepler: beacon SEEN off={:03X} val={:08X} ::", offset, val);
                                            beacons_seen += 1;
                                        }
                                        rows_pass1 += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass1 done rows={} ::", rows_pass1);
                                    if beacons_seen == 0 {
                                        serial_println!(":: kepler: beacon none-seen ::");
                                    }

                                    // Delay
                                    for _ in 0..2_000_000 { core::hint::spin_loop(); }

                                    // Pass 2: Volatility Re-Check
                                    let mut rows_pass2 = 0;
                                    for offset in (0..=0x3FC).step_by(4) {
                                        let val = mmio_read(bar0, 0x640000 + offset);
                                        if MIRROR_HDR_DENSE { serial_println!(":: kepler: mirror-hdr pass2 off={:03X} val={:08X} ::", offset, val); }
                                        rows_pass2 += 1;
                                    }
                                    serial_println!(":: kepler: mirror-hdr pass2 done rows={} ::", rows_pass2);
                                    
                                    // M2: Disp-Era USERD Reconnaissance (Read-Only)
                                    let disp_base = 0x610000;
                                    let pdisplay_0 = mmio_read(bar0, disp_base);
                                    let pdisplay_1 = mmio_read(bar0, disp_base + 0x40);
                                    let evo_core = mmio_read(bar0, disp_base + 0x490);
                                    let evo_userd_ptr = mmio_read(bar0, disp_base + 0x494);
                                    serial_println!(":: kepler: disp-userd-recon pdisplay_0={:08X} +40={:08X} evo_0x490={:08X} evo_0x494={:08X} ::", pdisplay_0, pdisplay_1, evo_core, evo_userd_ptr);

                                    // Milestone 2: PGRAPH Falcon Reconnaissance (Pull 18 + Pull 19)
                                    let pmc_en_pre = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-pulse pre={:08X} ::", pmc_en_pre);

                                    mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_en_pre & !(1 << 12));
                                    let pmc_en_off = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-pulse off rb={:08X} ::", pmc_en_off);
                                    
                                    for _ in 0..2_000_000 { core::hint::spin_loop(); }
                                    
                                    mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_en_pre | (1 << 12));
                                    let pmc_en_on = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-pulse on rb={:08X} ::", pmc_en_on);

                                    if (pmc_en_on & (1 << 12)) == 0 {
                                        serial_println!(":: kepler: pgraph-pulse REFUSED ::");
                                    } else {
                                        for _ in 0..2_000_000 { core::hint::spin_loop(); }

                                        // --- K-GPU-4 Pull 23: FECS / GPCCS Falcon Base Recon ---
                                        // s26 fold: dense fal-base dumps are historic (verdicts folded);
                                        // gated off to keep early serial inside the 64K FTDI ring.
                                        let fal_base_dense = false;
                                        for &base in &[0x409000, 0x41A000] {
                                            if fal_base_dense { for pass in 0..2 {
                                                if pass == 1 {
                                                    for _ in 0..2_000_000 { core::hint::spin_loop(); }
                                                }
                                                let tag = if pass == 0 { "fal-base" } else { "fal-base2" };

                                                for offset in (0..=0x1FC).step_by(4) {
                                                    let val = mmio_read(bar0, base + offset);
                                                    let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 || val == 0xBADF1000 { " ABSENT?" } else { "" };
                                                    serial_println!(":: kepler: {} b={:06X} off={:03X} val={:08X}{} ::", tag, base, offset, val, abs);
                                                }
                                            } }
                                            let cpuctl = mmio_read(bar0, base + 0x100);
                                            let imemc = mmio_read(bar0, base + 0x180);
                                            let dmemc = mmio_read(bar0, base + 0x1C0);
                                            serial_println!(":: kepler: fal-base b={:06X} verdict cpuctl={:08X} imemc={:08X} dmemc={:08X} ::", base, cpuctl, imemc, dmemc);

                                            // K-GPU-4 Pull 24: Falcon Sentinel Port Probe
                                            mmio_write(bar0, base + 0x180, 1 << 24); // IMEMC offset=0, AINCW
                                            let imemc_rb = mmio_read(bar0, base + 0x180);
                                            serial_println!(":: kepler: fal-port b={:06X} imemc wr=01000000 rb={:08X} ::", base, imemc_rb);
                                            
                                            mmio_write(bar0, base + 0x184, 0xDEADBEEF);
                                            mmio_write(bar0, base + 0x184, 0xCAFEF00D);
                                            mmio_write(bar0, base + 0x184, 0x12345678);
                                            mmio_write(bar0, base + 0x184, 0xA5A55A5A);
                                            
                                            mmio_write(bar0, base + 0x180, 1 << 25); // reset offset, AINCR
                                            let imem_w0 = mmio_read(bar0, base + 0x184);
                                            let imem_w1 = mmio_read(bar0, base + 0x184);
                                            let imem_w2 = mmio_read(bar0, base + 0x184);
                                            let imem_w3 = mmio_read(bar0, base + 0x184);
                                            serial_println!(":: kepler: fal-port b={:06X} imem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", base, imem_w0, imem_w1, imem_w2, imem_w3);
                                            
                                            mmio_write(bar0, base + 0x1C0, 1 << 24); // DMEMC offset=0, AINCW
                                            let dmemc_rb = mmio_read(bar0, base + 0x1C0);
                                            serial_println!(":: kepler: fal-port b={:06X} dmemc wr=01000000 rb={:08X} ::", base, dmemc_rb);
                                            
                                            mmio_write(bar0, base + 0x1C4, 0xDEADBEEF);
                                            mmio_write(bar0, base + 0x1C4, 0xCAFEF00D);
                                            mmio_write(bar0, base + 0x1C4, 0x12345678);
                                            mmio_write(bar0, base + 0x1C4, 0xA5A55A5A);
                                            
                                            mmio_write(bar0, base + 0x1C0, 1 << 25); // reset offset, AINCR
                                            let dmem_w0 = mmio_read(bar0, base + 0x1C4);
                                            let dmem_w1 = mmio_read(bar0, base + 0x1C4);
                                            let dmem_w2 = mmio_read(bar0, base + 0x1C4);
                                            let dmem_w3 = mmio_read(bar0, base + 0x1C4);
                                            serial_println!(":: kepler: fal-port b={:06X} dmem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", base, dmem_w0, dmem_w1, dmem_w2, dmem_w3);
                                        }

                                        // --- K-GPU-4 Milestone 2: First Ucode Execution (FECS ONLY) ---
                                        // Two candidate IO-port encodings, run A-then-B-only-if-needed (one
                                        // variable per shot, distinct magics so the mailbox names the winner):
                                        //   A: falcon I[0x1000] — the INDEXED scheme, host reg X -> (X & 0xffc) << 6,
                                        //      as nouveau's Kepler FECS/GPCCS ucode computes it (macros.fuc nv_mkio).
                                        //   B: falcon I[0x0040] — the FLAT scheme (host offset used directly), the
                                        //      "GF119+ some engines stopped using indexed accesses" escape hatch.
                                        // s28 correction: the s27 approval amendment specified B only; the indexed
                                        // scheme is the better-evidenced default, so A goes first.
                                        //
                                        // Assembly (envytools Falcon ISA v4; docs/hw/falcon/{arith,io,proc}.rst):
                                        //   f1 17 <lo> <hi>  mov   $r1, PORT     I16 immediate
                                        //   f1 27 ce fa      mov   $r2, 0xface   I16 sign-extended
                                        //   f1 23 0d f0      sethi $r2, 0xf00d   replaces the high half
                                        //   d1 12 00         iowrs I[$r1], $r2   synchronous IO write
                                        //   f8 02            exit
                                        const UCODE_A: [u32; 5] = [0x100017f1, 0xface27f1, 0xf00d23f1, 0xf80012d1, 0x00000002];
                                        const UCODE_B: [u32; 5] = [0x004017f1, 0xbeef27f1, 0xf00d23f1, 0xf80012d1, 0x00000002];
                                        // Pull 34 R3-AMEND: the echo images now live at module scope
                                        // (byte listings + compile-time port/derivation asserts).
                                        use ucode::{UCODE_CTX_ECHO_A, UCODE_CTX_ECHO_B};
                                        const MB_SEED: u32 = 0xA5A5_0000;
                                        // IMEM page granularity: the code TLB marks a page usable only when the
                                        // last word of the 0x40-word page is written (nouveau pads for this reason).
                                        const IMEM_PAGE_WORDS: usize = 0x40;
                                        
                                        let base = 0x409000;
                                        
                                        for &(img_label, img, want) in &[("A", &UCODE_A, 0xF00DFACEu32), ("B", &UCODE_B, 0xF00DBEEFu32)] {
                                            let port = if img_label == "A" { 0x1000 } else { 0x0040 };
                                            serial_println!(":: kepler: ucode img={} ioport={:04X} want={:08X} ::", img_label, port, want);
                                        
                                            // Seed the mailbox so "unchanged" has exactly one meaning.
                                            mmio_write(bar0, base + 0x040, MB_SEED);
                                            let pre_mb0 = mmio_read(bar0, base + 0x040);
                                            let pre_cpuctl = mmio_read(bar0, base + 0x100);
                                            serial_println!(":: kepler: ucode pre mailbox0={:08X} cpuctl={:08X} ::", pre_mb0, pre_cpuctl);
                                        
                                            // Upload, padding the full IMEM page so the code TLB marks it usable.
                                            mmio_write(bar0, base + 0x180, 1 << 24); // IMEMC offset=0, AINCW
                                            mmio_write(bar0, base + 0x188, 0);       // IMEMT tag=0 (matches BOOTVEC=0)
                                            for &word in img.iter() {
                                                mmio_write(bar0, base + 0x184, word);
                                            }
                                            for _ in img.len()..IMEM_PAGE_WORDS {
                                                mmio_write(bar0, base + 0x184, 0);
                                            }
                                            serial_println!(":: kepler: ucode uploaded words={} padded={} ::", img.len(), IMEM_PAGE_WORDS);
                                        
                                            // Page-usable attestation: TLB_CMD PTLB query on virtual page 0.
                                            mmio_write(bar0, base + 0x140, 0x0200_0000);
                                            let tlb_rd = mmio_read(bar0, base + 0x144);
                                            serial_println!(":: kepler: ucode tlb page0={:08X} ::", tlb_rd);
                                        
                                            mmio_write(bar0, base + 0x180, 1 << 25); // IMEMC offset=0, AINCR
                                            let mut verify_ok = true;
                                            let mut rb = [0u32; 5];
                                            for k in 0..img.len() {
                                                rb[k] = mmio_read(bar0, base + 0x184);
                                                if rb[k] != img[k] { verify_ok = false; }
                                            }
                                            serial_println!(":: kepler: ucode verify ok={} w0={:08X} w1={:08X} w2={:08X} w3={:08X} w4={:08X} ::",
                                                if verify_ok { "Y" } else { "N" }, rb[0], rb[1], rb[2], rb[3], rb[4]);
                                        
                                            if !verify_ok {
                                                serial_println!(":: kepler: ucode ABORT verify-mismatch — BOOTVEC/CPUCTL NOT written ::");
                                                break;
                                            }
                                        
                                            let dmactl_pre = mmio_read(bar0, base + 0x10C);
                                            serial_println!(":: kepler: dmactl pre={:08X} ::", dmactl_pre);
                                            mmio_write(bar0, base + 0x10C, dmactl_pre & !1);
                                            let dmactl_post = mmio_read(bar0, base + 0x10C);
                                            serial_println!(":: kepler: dmactl post={:08X} ::", dmactl_post);

                                            if (dmactl_post & 1) != 0 {
                                                serial_println!(":: kepler: dmactl REFUSED ::");
                                                continue;
                                            }

                                            mmio_write(bar0, base + 0x104, 0); // BOOTVEC=0
                                            mmio_write(bar0, base + 0x100, 2); // CPUCTL START_TRIGGER
                                            serial_println!(":: kepler: ucode start cpuctl<=00000002 ::");
                                        
                                            // Bounded poll for STOPPED (bit 4). halt-iters is the discriminator:
                                            // 0 = the poll proved nothing; >0 = the core demonstrably left the idle
                                            // state; max = started and stalled.
                                            let mut halt_iters = 0u32;
                                            for i in 0..100_000u32 {
                                                let c = mmio_read(bar0, base + 0x100);
                                                halt_iters = i;
                                                if (c & 0x10) != 0 { break; }
                                                core::hint::spin_loop();
                                            }
                                        
                                            let post_cpuctl = mmio_read(bar0, base + 0x100);
                                            let post_mb0 = mmio_read(bar0, base + 0x040);
                                            serial_println!(":: kepler: ucode end img={} cpuctl={:08X} mailbox0={:08X} halt-iters={} ::",
                                                img_label, post_cpuctl, post_mb0, halt_iters);
                                        
                                            if post_mb0 != MB_SEED {
                                                serial_println!(":: kepler: ucode EXECUTED img={} mailbox0={:08X} ::", img_label, post_mb0);
                                                break;
                                            }
                                            serial_println!(":: kepler: ucode img={} mailbox unchanged — trying next encoding ::", img_label);
                                        }
                                        
                                        // --- Pull 33: FECS Command Echo Skeleton (A/B Fallback) ---
                                        // Pull 34 R3-AMEND: split observable (MAILBOX0 = the value the
                                        // ucode READ) + phase counter (MAILBOX1) + bounded poll loop.
                                        for &(img_label, img, phase_bound) in &[
                                            ("A", &UCODE_CTX_ECHO_A[..], ucode::PHASE_A_BOUND),
                                            ("B", &UCODE_CTX_ECHO_B[..], ucode::PHASE_B_BOUND),
                                        ] {
                                            serial_println!(":: kepler: ucode-echo img={} bound={} ::", img_label, ucode::ECHO_BOUND);

                                            // Halt engine if running (from previous loop or previous attempt)
                                            let dmactl_pre = mmio_read(bar0, base + 0x10C);
                                            mmio_write(bar0, base + 0x10C, dmactl_pre & !1);

                                            mmio_write(bar0, base + 0x800, 0); // CC_SCRATCH[0]
                                            mmio_write(bar0, base + 0x804, 0); // CC_SCRATCH[1]
                                            // Seed both mailboxes so "unchanged" has exactly one meaning
                                            // (s29 discipline) for the two new observables as well.
                                            mmio_write(bar0, base + 0x040, MB_SEED); // MAILBOX0 <- value read
                                            mmio_write(bar0, base + 0x044, MB_SEED); // MAILBOX1 <- phase

                                            serial_println!(":: kepler: ucode-echo pre CC_SCRATCH[0]={:08X} CC_SCRATCH[1]={:08X} mb0={:08X} mb1={:08X} ::",
                                                mmio_read(bar0, base + 0x800), mmio_read(bar0, base + 0x804),
                                                mmio_read(bar0, base + 0x040), mmio_read(bar0, base + 0x044));

                                            mmio_write(bar0, base + 0x180, 1 << 24); // IMEMC AINCW
                                            mmio_write(bar0, base + 0x188, 0); // IMEMT tag=0
                                            for &word in img.iter() { mmio_write(bar0, base + 0x184, word); }
                                            for _ in img.len()..IMEM_PAGE_WORDS { mmio_write(bar0, base + 0x184, 0); }
                                            
                                            mmio_write(bar0, base + 0x180, 1 << 25); // IMEMC AINCR
                                            let mut verify_echo = true;
                                            for k in 0..img.len() {
                                                if mmio_read(bar0, base + 0x184) != img[k] { verify_echo = false; }
                                            }
                                            if !verify_echo {
                                                serial_println!(":: kepler: ucode-echo ABORT verify-mismatch img={} ::", img_label);
                                                continue;
                                            }
                                            
                                            mmio_write(bar0, base + 0x104, 0); // BOOTVEC=0
                                            mmio_write(bar0, base + 0x100, 2); // CPUCTL START_TRIGGER
                                            serial_println!(":: kepler: ucode-echo start img={} ::", img_label);
                                            
                                            mmio_write(bar0, base + 0x800, 1); // host-cmd
                                            serial_println!(":: kepler: ucode-echo host-cmd CC_SCRATCH[0]={:08X} ::", mmio_read(bar0, base + 0x800));
                                            
                                            let mut ack = 0;
                                            let mut ack_iters = 0;
                                            for i in 0..100_000u32 {
                                                ack = mmio_read(bar0, base + 0x804);
                                                ack_iters = i;
                                                if ack == 1 { break; }
                                                core::hint::spin_loop();
                                            }
                                            
                                            serial_println!(":: kepler: ucode-echo host-ack CC_SCRATCH[1]={:08X} iters={} ::", ack, ack_iters);

                                            // R3-AMEND split observable: mb0 = the word the ucode READ out
                                            // of CC_SCRATCH[0]; mb1 = the phase it last reached. An ack with
                                            // mb0 != 1 would mean CC_SCRATCH[1] moved for some reason other
                                            // than our echo; no ack with a live phase localises the fault.
                                            let mb0 = mmio_read(bar0, base + 0x040);
                                            let phase = mmio_read(bar0, base + 0x044);
                                            serial_println!(":: kepler: ctx-echo img={} ack={:08X} mb0={:08X} phase={:08X} ::",
                                                img_label, ack, mb0, phase);
                                            if phase == phase_bound as u32 {
                                                serial_println!(":: kepler: ctx-echo EXIT-BY-BOUND img={} iters={} — command never observed ::",
                                                    img_label, ucode::ECHO_BOUND);
                                            }
                                            if ack == 1 {
                                                serial_println!(":: kepler: ucode-echo SUCCESS img={} ::", img_label);
                                                break;
                                            } else {
                                                serial_println!(":: kepler: ucode-echo FAILURE img={} ::", img_label);
                                            }
                                        }

                                        // Read-only sweep of the unit window: locates either sentinel wherever it
                                        // actually landed (MAILBOX1 on an off-by-one, INTR on a wrong-port write).
                                        for off in (0..=0x1FC).step_by(4) {
                                            let val = mmio_read(bar0, base + off);
                                            let tag = if val == 0xF00DFACE || val == 0xF00DBEEF { " SENTINEL" } else { "" };
                                            serial_println!(":: kepler: ucode-post off={:03X} val={:08X}{} ::", off, val, tag);
                                        }
                                        // dense old-base recon gated off (FTDI-ring budget).
                                        let old_base_dense = false;
                                        if old_base_dense { for pass in 0..2 {
                                            if pass == 1 {
                                                for _ in 0..2_000_000 { core::hint::spin_loop(); }
                                            }

                                            let cpuctl = mmio_read(bar0, 0x400100);
                                            let bootvec = mmio_read(bar0, 0x400104);
                                            serial_println!(":: kepler: falcon pass{} cpuctl={:08X} bootvec={:08X} ::", pass, cpuctl, bootvec);

                                            let mut falcon_rows = 0;
                                            for offset in (0..=0x1C).step_by(4) {
                                                let val = mmio_read(bar0, 0x400100 + offset);
                                                let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                                serial_println!(":: kepler: falcon core off={:03X} val={:08X}{} ::", 0x100 + offset, val, abs);
                                                falcon_rows += 1;
                                            }
                                            serial_println!(":: kepler: falcon core done rows={} ::", falcon_rows);

                                            let imemc = mmio_read(bar0, 0x400180);
                                            let dmemc = mmio_read(bar0, 0x4001C0);
                                            let abs_i = if imemc == 0xFFFFFFFF || imemc == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                            let abs_d = if dmemc == 0xFFFFFFFF || dmemc == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                            serial_println!(":: kepler: falcon mem imemc={:08X}{} dmemc={:08X}{} ::", imemc, abs_i, dmemc, abs_d);

                                            let mut pgraph_rows = 0;
                                            for offset in (0..=0x7C).step_by(4) {
                                                let val = mmio_read(bar0, 0x400000 + offset);
                                                let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 { " ABSENT?" } else { "" };
                                                serial_println!(":: kepler: pgraph stat off={:03X} val={:08X}{} ::", offset, val, abs);
                                                pgraph_rows += 1;
                                            }
                                            serial_println!(":: kepler: pgraph stat done rows={} ::", pgraph_rows);
                                        } }
                                    }

                                    let pre_wit_mb1 = mmio_read(bar0, 0x409000 + 0x044);
                                    let pre_wit_cpu = mmio_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: hb pre-witness mb1={:08X} cpuctl={:08X} ::", pre_wit_mb1, pre_wit_cpu);

                                    // --- Witness Rematch ---
                                    serial_println!(":: kepler: witness-rematch begin (pgraph on) ::");

                                    // 2. Bind and Enable PFIFO_CHAN for channel 1
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0); 
                                    mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400); 
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12)); 

                                    let err = mmio_read(bar0, 0x252c);
                                    let stat = mmio_read(bar0, 0x263c);
                                    let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                    serial_println!(":: kepler: sched-status post-init err={:08X} ({}) stat={:08X} ::", err, err_str, stat);

                                    let ch_1_0_pre = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let ch_1_4_pre = mmio_read(bar0, 0x800004 + (1 * 8));
                                    serial_println!(":: kepler: PFIFO_CHAN[1] pre-submit: 00={:08X} 04={:08X} ::", ch_1_0_pre, ch_1_4_pre);
                                    
                                    // Witness check
                                    if (ch_1_0_pre & 0xC0000000) != 0xC0000000 {
                                        serial_println!(":: kepler: WITNESS FAILED - bits stripped. Restoring inst_off+0x0C ::");
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, (userd_off >> 32) as u32);
                                        // Re-test PFIFO_CHAN[1] to clear state
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0);
                                        mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400);
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12));
                                        
                                        let err = mmio_read(bar0, 0x252c);
                                        let stat = mmio_read(bar0, 0x263c);
                                        let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                        serial_println!(":: kepler: sched-status post-restore err={:08X} ({}) stat={:08X} ::", err, err_str, stat);
                                    } else {
                                        serial_println!(":: kepler: WITNESS PASSED - bits stuck! ::");
                                    }

                                    // --- POLL-CONTROL leg (GR5, s37): the one variable never varied WITH an
                                    // error readback. Every witness write above is 0xC0000000 = VALID|POLL_ENABLE,
                                    // and the chip's own name for err=2 is NO_POLL ("validated a channel with
                                    // POLL_ENABLE, but poll area is disabled" — sitting #9). Sitting #8 did write
                                    // VALID-only (0x80002000) and the bit still stripped, so we do NOT expect the
                                    // bits to hold — but s8 predates the error readback (pull 10), so nobody has
                                    // ever read err= with POLL_ENABLE CLEAR. If err stays 00000002 while POLL is
                                    // clear, the chip's NO_POLL name does not mean what it says and the reason
                                    // code is a red herring we have honored for 28 sittings. If err CHANGES, the
                                    // new code names the real precondition. Either way it is decisive, and it is
                                    // three writes. The legs above are untouched controls.
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0);
                                    mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400);
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0x80000000 | ((inst_off as u32) >> 12));
                                    let poll_rb = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let poll_err = mmio_read(bar0, 0x252c);
                                    let poll_stat = mmio_read(bar0, 0x263c);
                                    serial_println!(":: kepler: poll-control valid-only chan={:08X} err={:08X} stat={:08X} ::",
                                        poll_rb, poll_err, poll_stat);

                                    let post_wit_scratch = mmio_read(bar0, 0x409000 + 0x804);
                                    let post_wit_cpu = mmio_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: ucode-echo post-witness CC_SCRATCH[1]={:08X} cpuctl={:08X} ::", post_wit_scratch, post_wit_cpu);
                                    
                                    for _ in 0..1_000_000 { core::hint::spin_loop(); }
                                    let final_scratch = mmio_read(bar0, 0x409000 + 0x804);
                                    let final_cpu = mmio_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: ucode-echo final CC_SCRATCH[1]={:08X} cpuctl={:08X} ::", final_scratch, final_cpu);

                                    // --- Pull 28 recon, relocated (GR5, s31 fold): the first access to an
                                    // absent 0x409xxx offset latches a sticky PRI fault and every later read
                                    // of the unit returns BADF1000 (s31: fal-base read real, then all
                                    // post-0x409504 reads poisoned, s30 markers included). Run the recon LAST,
                                    // after every proven read, and bracket it with cpuctl control reads so
                                    // poisoning is observed in-boot rather than inferred.
                                    serial_println!(":: kepler: recon-pre cpuctl={:08X} ::", mmio_read(bar0, 0x409000 + 0x100));
                                    
                                    // Pull 31: Context-Bind Experiment
                                    let ch_id = (inst_off as u32) >> 12;
                                    serial_println!(":: kepler: bind-pre CHAN_CUR={:08X} CHAN_NEXT={:08X} ENGINE_STATUS={:08X} ::",
                                        mmio_read(bar0, 0x409B00), mmio_read(bar0, 0x409B04), mmio_read(bar0, 0x409C00));
                                    
                                    // Write CHAN_CUR and verify
                                    mmio_write(bar0, 0x409B00, ch_id);
                                    let c_cur = mmio_read(bar0, 0x409B00);
                                    if (c_cur >> 16) == 0xBADF {
                                        serial_println!(":: kepler: bind CHAN_CUR FAULT={:08X} (skip rest) ::", c_cur);
                                    } else {
                                        serial_println!(":: kepler: bind CHAN_CUR={:08X} ::", c_cur);
                                        
                                        // Write CHAN_NEXT and verify
                                        mmio_write(bar0, 0x409B04, ch_id);
                                        let c_next = mmio_read(bar0, 0x409B04);
                                        if (c_next >> 16) == 0xBADF {
                                            serial_println!(":: kepler: bind CHAN_NEXT FAULT={:08X} (skip rest) ::", c_next);
                                        } else {
                                            serial_println!(":: kepler: bind CHAN_NEXT={:08X} ::", c_next);
                                            
                                            serial_println!(":: kepler: bind-post ENGINE_STATUS={:08X} ::", mmio_read(bar0, 0x409C00));
                                            
                                            // Explicit post-bind witness leg (PFIFO_CHAN[1] Register)
                                            let pre_rw = mmio_read(bar0, 0x800000 + (1 * 8));
                                            serial_println!(":: kepler: witness pre-rewrite PFIFO_CHAN[1]={:08X} ::", pre_rw);
                                            
                                            let witness_val = 0xC0000000 | ((inst_off as u32) >> 12);
                                            mmio_write(bar0, 0x800000 + (1 * 8), witness_val);
                                            
                                            let witness_post = mmio_read(bar0, 0x800000 + (1 * 8));
                                            serial_println!(":: kepler: witness post-bind PFIFO_CHAN[1]={:08X} ::", witness_post);
                                        }
                                    }
                                    
                                    serial_println!(":: kepler: recon-post cpuctl={:08X} ::", mmio_read(bar0, 0x409000 + 0x100));


                                    // 3. Submit Runlist
                                    mmio_write(bar0, 0x2270, (runlist_off as u32) >> 12); // target=0 (VRAM), addr
                                    mmio_write(bar0, 0x2274, 3); // LEN=3, ENG=0
                                    serial_println!("[NVIDIA] Configured Runlist and bound channel.");

                                    // Wait for PLAYLIST_RD to accept the runlist
                                    let mut pl_rd = 0;
                                    let mut pl_rd_len = 0;
                                    for _ in 0..100_000 {
                                        pl_rd = mmio_read(bar0, 0x2280);
                                        pl_rd_len = mmio_read(bar0, 0x2284);
                                        if pl_rd == ((runlist_off as u32) >> 12) && (pl_rd_len & 0xFFF) == 1 {
                                            break;
                                        }
                                    }
                                    serial_println!(":: kepler: post-bind playlist_rd={:08X} playlist_rd_len={:08X} ::", pl_rd, pl_rd_len);

                                    // --- GR6 runlist-sibling sweep (READ-ONLY; docs/dev/OS/08_VIDEO/gpu_spec.md §2.4.2) ---
                                    // Under the gk104 array shape the host runlist controls are
                                    // RUNLIST[i] base at 0x2270 + i*8 with its submit/length word at +4.
                                    // We only ever submit to i=0. Reading i=1..3 discriminates the three
                                    // surviving readings of the invariant bit 20 in PLAYLIST_RD_LEN
                                    // (id field / commit-BUSY / unconditional status), and answers whether
                                    // a non-PGRAPH (copy-engine) runlist exists — which would make
                                    // FIFO-level method execution reachable without the Falcon ucode era.
                                    // i=2 lands on 0x2280/0x2284, the pair already read above: a built-in
                                    // cross-check on the array-stride assumption itself.
                                    // Writes NOTHING — pull-28 no-unproven-writes rule.
                                    let mut rl_occupied = 0u32;
                                    for i in 1..4usize {
                                        let base_off = 0x2270 + i * 8;
                                        let len_off = base_off + 4;
                                        let rl_base = mmio_read(bar0, base_off);
                                        let rl_len = mmio_read(bar0, len_off);
                                        let bit20 = (rl_len >> 20) & 1;
                                        let engf = (rl_len >> 20) & 0xF;
                                        let entries = rl_len & 0xFFF;
                                        let poison = (rl_base >> 16) == 0xBADF || (rl_base >> 16) == 0xBAD0;
                                        if !poison && rl_base != 0 {
                                            rl_occupied |= 1 << i;
                                        }
                                        serial_println!(
                                            ":: kepler: runlist-scan i={} base_off={:04X} base={:08X} len={:08X} bit20={} engfield={:X} entries={} {} ::",
                                            i, base_off, rl_base, rl_len, bit20, engf, entries,
                                            if poison { "POISON" } else if rl_base != 0 { "OCCUPIED" } else { "empty" }
                                        );
                                    }
                                    serial_println!(
                                        ":: kepler: runlist-scan verdict occupied_mask={:X} alias_i2_base={} alias_i2_len={} ::",
                                        rl_occupied,
                                        if mmio_read(bar0, 0x2280) == pl_rd { "match" } else { "DIVERGES" },
                                        if mmio_read(bar0, 0x2284) == pl_rd_len { "match" } else { "DIVERGES" }
                                    );


                                    let err = mmio_read(bar0, 0x252c);
                                    let stat = mmio_read(bar0, 0x263c);
                                    let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                    serial_println!(":: kepler: sched-status post-submit err={:08X} ({}) stat={:08X} ::", err, err_str, stat);

                                    let ch_1_0_post = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let ch_1_4_post = mmio_read(bar0, 0x800004 + (1 * 8));
                                    serial_println!(":: kepler: PFIFO_CHAN[1] post-submit: 00={:08X} 04={:08X} ::", ch_1_0_post, ch_1_4_post);

                                    // Discriminator readback
                                    for i in 0..3 {
                                        let pbdma_base_i = 0x40000 + (i * 0x2000);
                                        let ch = mmio_read(bar0, pbdma_base_i + 0x120);
                                        let chid_active = ch & 0xFFF;
                                        let is_active = (ch >> 13) & 1;
                                        serial_println!(":: kepler: DISCRIMINATOR pbdma{} ch={:08X} (CHID={} ACTIVE={}) ::", i, ch, chid_active, is_active);
                                    }

                                    let final_err = mmio_read(bar0, 0x252c);
                                    let final_stat = mmio_read(bar0, 0x263c);
                                    serial_println!(":: kepler: witness-rematch end err={:08X} stat={:08X} valid={:08X} ::", final_err, final_stat, ch_1_0_post);

                                    // --- K-GPU-4 Milestone 1: Falcon IMEM/DMEM Probe ---
                                    // s26 fold: old base nonexistent; probe gated off (FTDI-ring budget).
                                    let old_base_probe = false;
                                    if old_base_probe {
                                    // 1. IMEM probe
                                    mmio_write(bar0, 0x400180, 1 << 24); // IMEMC offset=0, auto-increment
                                    let imemc_rb = mmio_read(bar0, 0x400180);
                                    serial_println!(":: kepler: falcon imemc wr=01000000 rb={:08X} ::", imemc_rb);
                                    
                                    mmio_write(bar0, 0x400184, 0xDEADBEEF);
                                    mmio_write(bar0, 0x400184, 0xCAFEF00D);
                                    mmio_write(bar0, 0x400184, 0x12345678);
                                    mmio_write(bar0, 0x400184, 0xA5A55A5A);
                                    
                                    mmio_write(bar0, 0x400180, 1 << 25); // reset offset, AINCR (bit25 = read auto-increment; bit24 only increments on writes)
                                    let imem_w0 = mmio_read(bar0, 0x400184);
                                    let imem_w1 = mmio_read(bar0, 0x400184);
                                    let imem_w2 = mmio_read(bar0, 0x400184);
                                    let imem_w3 = mmio_read(bar0, 0x400184);
                                    serial_println!(":: kepler: falcon imem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", imem_w0, imem_w1, imem_w2, imem_w3);
                                    
                                    // 2. DMEM probe
                                    mmio_write(bar0, 0x4001C0, 1 << 24); // DMEMC offset=0, auto-increment
                                    let dmemc_rb = mmio_read(bar0, 0x4001C0);
                                    serial_println!(":: kepler: falcon dmemc wr=01000000 rb={:08X} ::", dmemc_rb);
                                    
                                    mmio_write(bar0, 0x4001C4, 0xDEADBEEF);
                                    mmio_write(bar0, 0x4001C4, 0xCAFEF00D);
                                    mmio_write(bar0, 0x4001C4, 0x12345678);
                                    mmio_write(bar0, 0x4001C4, 0xA5A55A5A);
                                    
                                    mmio_write(bar0, 0x4001C0, 1 << 25); // reset offset, AINCR (read auto-increment)
                                    let dmem_w0 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w1 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w2 = mmio_read(bar0, 0x4001C4);
                                    let dmem_w3 = mmio_read(bar0, 0x4001C4);
                                    serial_println!(":: kepler: falcon dmem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", dmem_w0, dmem_w1, dmem_w2, dmem_w3);
                                    }

                                    // --- s26 LATE DISPLAY RECAP (FTDI-ring workaround) ---
                                    // The display leg runs before the FTDI link is live and its
                                    // lines can fall off the 64K drop-oldest boot ring. Re-emit
                                    // the display verdict here, inside the surviving window.
                                    serial_println!(":: kdisp: late-recap fb={:08X} ran={} trace [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
                                        fb_offset.unwrap_or(0xFFFFFFFF) as u32,
                                        fb_offset.is_some(),
                                        kdisp_trace[0], kdisp_trace[1], kdisp_trace[2], kdisp_trace[3],
                                        kdisp_trace[4], kdisp_trace[5], kdisp_trace[6]);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    serial_println!("[NVIDIA] Initialization complete (Phases 1-4)");
}

/// A simple bump allocator for VRAM (CPU-visible via BAR1).
pub struct VramAllocator {
    pub base_phys: usize,
    pub total_size: usize,
    pub current_offset: usize,
}

impl VramAllocator {
    pub fn new(bar1_base: usize, bar1_size: usize, vram_size: usize) -> Self {
        let total_size = if vram_size < bar1_size { vram_size } else { bar1_size };
        
        Self {
            base_phys: bar1_base,
            total_size,
            // Skip the first 32MB to avoid stepping on the firmware's GOP framebuffer
            current_offset: 32 * 1024 * 1024,
        }
    }

    /// Allocates a block of VRAM and returns the byte offset from the start of VRAM.
    pub fn alloc(&mut self, size: usize) -> Option<usize> {
        // Align to 4KB (page boundary)
        let aligned_offset = (self.current_offset + 0xFFF) & !0xFFF;
        
        if aligned_offset + size > self.total_size {
            return None; // Out of memory
        }
        
        self.current_offset = aligned_offset + size;
        Some(aligned_offset)
    }
}

/// A GPU Command PushBuffer for PFIFO command submission.
/// Commands are written as 32-bit words (methods and data) and the hardware fetches them via DMA.
pub struct PushBuffer {
    pub vram_phys: usize,
    pub size: usize,
    pub capacity: usize,
    pub write_ptr: usize,
}

impl PushBuffer {
    pub fn new(vram_phys: usize, size: usize) -> Self {
        Self {
            vram_phys,
            size,
            capacity: size / 4, // 32-bit command words
            write_ptr: 0,
        }
    }

    /// Appends a 32-bit command word to the pushbuffer.
    pub fn push(&mut self, _word: u32) {
        if self.write_ptr < self.capacity {
            // In a real implementation, we would write to the CPU-mapped virtual address of VRAM.
            // unsafe { core::ptr::write_volatile((self.vram_virt + self.write_ptr * 4) as *mut u32, word); }
            self.write_ptr += 1;
        }
    }

    /// Generates an NVIDIA "Set Object" command for a specific GPU class (e.g., Kepler 2D or 3D engine).
    pub fn push_set_object(&mut self, class_id: u32) {
        // Method 0x0000 is typically SetObject
        // Format: [Size: 13 bits] [Subchannel: 3 bits] [Method: 16 bits]
        let header = (1 << 16) | (0 << 13) | 0x0000;
        self.push(header);
        self.push(class_id);
    }
}

pub unsafe fn mmio_read(base: usize, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset) as *const u32)
}

pub unsafe fn mmio_write(base: usize, offset: usize, val: u32) {
    core::ptr::write_volatile((base + offset) as *mut u32, val)
}

#[cfg(test)]
mod tests {
    use super::regs::falcon_io;
    use super::ucode::*;

    /// The IO derivation itself (spec §3), both metal-proven register families.
    #[test]
    fn falcon_io_matches_the_proven_mappings() {
        assert_eq!(falcon_io(0x040), 0x1000); // MAILBOX0, s29
        assert_eq!(falcon_io(0x044), 0x1100); // MAILBOX1, s30
        assert_eq!(falcon_io(0x800), 0x20000); // CC_SCRATCH[0], s37
        assert_eq!(falcon_io(0x804), 0x20100); // CC_SCRATCH[1], s37
    }

    /// Round-trip: the documented byte listing packs to the words we upload,
    /// and unpacking those words reproduces the listing byte for byte.
    #[test]
    fn echo_images_round_trip_bytes_to_words() {
        for (bytes, words) in [
            (&ECHO_A_BYTES, &UCODE_CTX_ECHO_A),
            (&ECHO_B_BYTES, &UCODE_CTX_ECHO_B),
        ] {
            assert_eq!(pack92(bytes), *words);
            let mut back = [0u8; 92];
            for (w, word) in words.iter().enumerate() {
                back[w * 4..w * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            assert_eq!(&back, bytes);
        }
    }

    /// The four words that executed on metal at sitting #37 must not have moved.
    #[test]
    fn s37_acked_prologue_is_preserved() {
        assert_eq!(
            &UCODE_CTX_ECHO_A[..4],
            &[0xf000_17f0u32, 0x27f1_0213, 0x23f0_0100, 0x0137_f002]
        );
    }

    /// Every port immediate in both images is the value `falcon_io` derives —
    /// this is the check that would have caught pull 33's raw-host-offset
    /// listing (`0x800`/`0x804`) at build time.
    #[test]
    fn every_port_immediate_is_derived_not_hand_written() {
        for bytes in [&ECHO_A_BYTES, &ECHO_B_BYTES] {
            // $r1: mov(I8) @0x00 + sethi(I8) @0x03
            let r1 = bytes[0x02] as u32 | ((bytes[0x05] as u32) << 16);
            // $r2: mov(I16) @0x06 + sethi(I8) @0x0a
            let r2 = bytes[0x08] as u32 | ((bytes[0x09] as u32) << 8) | ((bytes[0x0c] as u32) << 16);
            // $r6/$r7: mov(I16) @0x10 / @0x14
            let r6 = bytes[0x12] as u32 | ((bytes[0x13] as u32) << 8);
            let r7 = bytes[0x16] as u32 | ((bytes[0x17] as u32) << 8);
            assert_eq!(r1, falcon_io(0x800));
            assert_eq!(r2, falcon_io(0x804));
            assert_eq!(r6, falcon_io(0x040));
            assert_eq!(r7, falcon_io(0x044));
        }
    }

    /// Bounded-loop law (spec §5.1): the counter is seeded to exactly
    /// `ECHO_BOUND` iterations in both images, and the loop's only backward
    /// branch is the one guarded by that counter.
    #[test]
    fn echo_loop_is_bounded() {
        let seed_a = ((ECHO_A_BYTES[0x1d] as u32) | ((ECHO_A_BYTES[0x1e] as u32) << 8)) << 16;
        let seed_b = ((ECHO_B_BYTES[0x1d] as u32) | ((ECHO_B_BYTES[0x1e] as u32) << 8)) << 16;
        assert_eq!(seed_a, ECHO_BOUND); // A counts down from +bound with `sub`
        assert_eq!(seed_b, ECHO_BOUND.wrapping_neg()); // B counts up from -bound with `add`
        assert_eq!(ECHO_BOUND, 1_048_576);

        // A: `sub b32 $r5, 0x1` = b0 52 01 ; B: `add b32 $r5, 0x1` = b0 50 01.
        assert_eq!(&ECHO_A_BYTES[0x48..0x4b], &[0xb0, 0x52, 0x01]);
        assert_eq!(&ECHO_B_BYTES[0x48..0x4b], &[0xb0, 0x50, 0x01]);

        // The bound test and its backward branch: cmpu $r5,0 ; bra ne,-0x29 -> poll @0x25.
        for bytes in [&ECHO_A_BYTES, &ECHO_B_BYTES] {
            assert_eq!(&bytes[0x4b..0x4e], &[0xb0, 0x54, 0x00]);
            assert_eq!(&bytes[0x4e..0x51], &[0xf4, 0x1b, 0xd7]);
            let disp = bytes[0x50] as i8 as isize;
            assert_eq!(0x4e_isize + disp, 0x25); // lands on the `iord`
            // The forward `bra ne` at 0x34 lands on the decrement block at 0x48.
            let fwd = bytes[0x36] as i8 as isize;
            assert_eq!(0x34_isize + fwd, 0x48);
            // Both exits are a real `exit` (f8 02), not a fall-off-the-page.
            assert_eq!(&bytes[0x46..0x48], &[0xf8, 0x02]); // after ack
            assert_eq!(&bytes[0x57..0x59], &[0xf8, 0x02]); // after bound
        }
    }

    /// The split observable: MAILBOX0 is written with the value that was read,
    /// and MAILBOX1 is stamped before and after each risky IO step.
    #[test]
    fn split_observable_and_phase_stamps_are_present() {
        for bytes in [&ECHO_A_BYTES, &ECHO_B_BYTES] {
            assert_eq!(&bytes[0x25..0x28], &[0xcf, 0x14, 0x00]); // iord  $r4, I[$r1]
            assert_eq!(&bytes[0x28..0x2b], &[0xd0, 0x64, 0x00]); // iowr  I[$r6], $r4
            assert_eq!(&bytes[0x3d..0x40], &[0xd0, 0x23, 0x00]); // iowr  I[$r2], $r3 (ack)
            // five phase stamps, each an `iowr I[$r7], $r0`
            for at in [0x22usize, 0x2e, 0x3a, 0x43, 0x54] {
                assert_eq!(&bytes[at..at + 3], &[0xd0, 0x70, 0x00]);
            }
        }
        let a = [
            PHASE_A_PRELOOP,
            PHASE_A_POSTREAD,
            PHASE_A_PREACK,
            PHASE_A_POSTACK,
            PHASE_A_BOUND,
        ];
        let b = [
            PHASE_B_PRELOOP,
            PHASE_B_POSTREAD,
            PHASE_B_PREACK,
            PHASE_B_POSTACK,
            PHASE_B_BOUND,
        ];
        for (i, at) in [0x21usize, 0x2d, 0x39, 0x42, 0x53].iter().enumerate() {
            assert_eq!(ECHO_A_BYTES[*at], a[i]);
            assert_eq!(ECHO_B_BYTES[*at], b[i]);
        }
        // Distinct magics: A and B can never be confused in MAILBOX1.
        for x in a {
            assert!(!b.contains(&x));
        }
    }

    /// A/B discipline (spec §5.2): one variable per boot. A and B may differ
    /// only in the counter arithmetic, the counter seed, and the phase magics.
    #[test]
    fn ab_pair_differs_on_exactly_one_variable() {
        let allowed = [0x1dusize, 0x1e, 0x21, 0x2d, 0x39, 0x42, 0x49, 0x53];
        for i in 0..92 {
            if ECHO_A_BYTES[i] != ECHO_B_BYTES[i] {
                assert!(allowed.contains(&i), "unexpected A/B divergence at {i:#x}");
            }
        }
        // The one instruction-level variable: the 0xb0-form subopcode nibble.
        assert_eq!(ECHO_A_BYTES[0x49] & 0x0f, 2); // sub
        assert_eq!(ECHO_B_BYTES[0x49] & 0x0f, 0); // add
        assert_eq!(ECHO_A_BYTES[0x49] >> 4, ECHO_B_BYTES[0x49] >> 4); // same register
    }
}
