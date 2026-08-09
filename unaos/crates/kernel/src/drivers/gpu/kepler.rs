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
    // ENGINE_STATUS (host `+0xC00`, spec §2) — **DERIVED (untested)**. No boot
    // has yet written this port from inside the falcon; the FENCE image is the
    // first, and its result is only interpretable if this derivation holds. It
    // is asserted here for the same reason as the four above: so the index in
    // the image and the index in this table cannot drift.
    const _: () = assert!(falcon_io(0xC00) == 0x30000); // ENGINE_STATUS, DERIVED
}

/// FECS microcode images, byte-exact.
///
/// Falcon instructions are variable-length byte sequences; IMEM is written a
/// `u32` at a time. The **byte listing is authoritative** here — the packed
/// `[u32]` images are produced from it by [`pack128`] at compile time, so the
/// listing in `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` and the words the
/// host uploads cannot drift apart. Every instruction in both images — all 128
/// bytes of each, padding included — is pinned by a `const _` assertion below,
/// built out of the instruction constructors (`mov_i8`, `iowr`, `bra`, …) and
/// out of [`regs::falcon_io`] for the port immediates. Coverage is contiguous
/// by construction: there are no unpinned holes for a hand-typed byte to hide
/// in. This is the check that would have caught pull 33's raw-host-offset
/// listing, and the four malformed instructions (`f1 38`, `f0 38`, `f4 2b`,
/// `b0 52`) that survived three review rounds because the old assertions
/// checked only immediates while the comments read correctly.
///
/// ## Two images, and why they are two
///
/// [`ECHO_A_BYTES`] is the **falcon-execution test**: command in, ACK out,
/// phase stamps. It runs mid-sequence. It must therefore contain **no `$r8`
/// setup, no `iord I[$r8]`, and no `0x409504` in any form** — the first access
/// to `0x409504` (WRCMD_CMD) faults and wedges every subsequent read in the
/// FECS unit for the rest of the boot (spec §5.4, s31/s32/s34), so an echo test
/// that reads it poisons the unit it is testing and invalidates every FECS
/// observation printed after it. That absence is asserted mechanically, not
/// promised in a comment.
///
/// [`POKE_A_BYTES`] is the same skeleton **plus** the `$r8 = falcon_io(0x504)`
/// setup and the `iord`. Reading the poison offset from the falcon side, where
/// the host side faults, is a legitimate experiment — it just has to be the
/// last thing the kepler leg does, so it executes exactly once, at the terminal
/// phase, immediately before the host's terminal `fecs_write(bar0, 0x409504, 0)`
/// (spec §10).
///
/// Retained from pull 34 (R3-AMEND):
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
    ///
    /// These go through [`regs::falcon_io`] deliberately: it carries the
    /// `& 0xffc` mask, and it is the single point where the host-offset →
    /// IO-index derivation lives. Do **not** add a second `falcon_io` in this
    /// module — a local one shadows the real one and silently drops the mask.
    pub const IO_MAILBOX0: u32 = falcon_io(0x040);
    pub const IO_MAILBOX1: u32 = falcon_io(0x044);
    pub const IO_CC_SCRATCH0: u32 = falcon_io(0x800);
    pub const IO_CC_SCRATCH1: u32 = falcon_io(0x804);
    /// WRCMD_CMD — the poison offset, falcon-side. Host reads of `0x409504`
    /// fault and wedge the unit (spec §5.4); [`POKE_A_BYTES`] reads it with an
    /// `iord` from inside the falcon instead. Referenced by POKE only.
    pub const IO_WRCMD_CMD: u32 = falcon_io(0x504);

    /// Iteration bound on the echo/poke poll loop, **in Falcon instructions**.
    ///
    /// Chosen as `0x0010_0000` = 1_048_576 iterations. Sizing argument: the
    /// loop body is 8 instructions, so the bound is ~8.4M Falcon instructions —
    /// milliseconds of Falcon time. The host writes the command word one
    /// `fecs_write` plus one `serial_println!` after `CPUCTL <= 2`, and at s37
    /// the ucode had already consumed it by the host's *first* poll
    /// (`iters=0`). The bound is therefore ~3 orders of magnitude larger than
    /// the window it must cover: it exists to guarantee termination (spec
    /// §5.1), not to be reached. If `phase` ever comes back as
    /// [`PHASE_A_BOUND`], the command never arrived — that is a real finding,
    /// not a tuning problem.
    ///
    /// ⚠ This is a **Falcon** budget. It is not a host MMIO read count and must
    /// never be used as one: at ~1 µs per BAR0 read it would be ~1 s of boot
    /// spent spinning. The host side uses [`HOST_ACK_ITERS`].
    pub const ECHO_BOUND: u32 = 0x0010_0000;

    /// Host-side poll bound, in **host MMIO reads**, matching the bound the
    /// `ucode-echo` leg has used since pull 33.
    pub const HOST_ACK_ITERS: u32 = 100_000;

    // Phase stamps, written to MAILBOX1 (`I[0x1100]`).
    pub const PHASE_A_PRELOOP: u8 = 0x01;
    pub const PHASE_A_POSTREAD: u8 = 0x02;
    pub const PHASE_A_PREACK: u8 = 0x03;
    pub const PHASE_A_POSTACK: u8 = 0x04;

    /// Exit-by-bound stamp, **as the host reads it back**.
    ///
    /// The ucode reaches it with `mov $r0, 0xbd` — a *signed* I8 immediate, so
    /// the register (and therefore MAILBOX1) holds `0xFFFF_FFBD`, not `0xBD`.
    /// envydis prints that instruction as `mov $r0 -0x43`. Declaring this `u8 =
    /// 0xBD` is what made the exit-by-bound branch of the host verdict
    /// unreachable: it compared a sign-extended read against a truncated
    /// constant and never matched. The byte in the image is still `0xbd`; the
    /// assertion below takes `PHASE_A_BOUND as u8` to check it.
    pub const PHASE_A_BOUND: u32 = 0xFFFF_FFBD;

    /// Image A — the ECHO test. **No `$r8`, no `0x409504`.**
    ///
    /// ```text
    /// // Addr | Bytes       | Instruction         | Note
    /// // -----|-------------|---------------------|-------------------------------------
    /// // 0x00 | f0 17 00    | mov   $r1, 0x00     | low half of I[CC_SCRATCH[0]]
    /// // 0x03 | f0 13 02    | sethi $r1, 0x02     | $r1 = 0x20000                (s37)
    /// // 0x06 | f1 27 00 01 | mov   $r2, 0x0100   | low half of I[CC_SCRATCH[1]]
    /// // 0x0a | f0 23 02    | sethi $r2, 0x02     | $r2 = 0x20100                (s37)
    /// // 0x0d | f0 37 01    | mov   $r3, 0x01     | the ack value
    /// // 0x10 | f1 67 00 10 | mov   $r6, 0x1000   | $r6 = I[MAILBOX0]            (s29)
    /// // 0x14 | f1 77 00 11 | mov   $r7, 0x1100   | $r7 = I[MAILBOX1]            (s30)
    /// // 0x18 | f0 57 00    | mov   $r5, 0x00     | loop counter, low half
    /// // 0x1b | f1 53 10 00 | sethi $r5, 0x0010   | $r5 = 0x00100000 = ECHO_BOUND
    /// // 0x1f | f0 07 01    | mov   $r0, 0x01     | phase 0x01
    /// // 0x22 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-loop)
    /// // poll:
    /// // 0x25 | cf 14 00    | iord  $r4, I[$r1]   | read the command word
    /// // 0x28 | d0 64 00    | iowr  I[$r6], $r4   | MAILBOX0 = VALUE READ  <-- split obs.
    /// // 0x2b | f0 07 02    | mov   $r0, 0x02     |
    /// // 0x2e | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-read)
    /// // 0x31 | b0 44 02    | cmpu b32 $r4, 0x02  | host said "quit"?
    /// // 0x34 | f4 0b 2b    | bra eq, +0x2b       | -> 0x5f cmd2_exit
    /// // 0x37 | b0 44 01    | cmpu b32 $r4, 0x01  | host said "ack"?
    /// // 0x3a | f4 1b 14    | bra ne, +0x14       | -> 0x4e dec, keep polling
    /// // 0x3d | f0 07 03    | mov   $r0, 0x03     |
    /// // 0x40 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-ack)
    /// // 0x43 | d0 23 00    | iowr  I[$r2], $r3   | CC_SCRATCH[1] = 1 (ACK)
    /// // 0x46 | f0 07 04    | mov   $r0, 0x04     |
    /// // 0x49 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-ack)
    /// // 0x4c | f8 02       | exit                | terminal state: phase=04
    /// // dec:
    /// // 0x4e | b6 52 01    | sub b32 $r5, 0x01   | subop 2 on the b6 form
    /// // 0x51 | b0 54 00    | cmpu b32 $r5, 0x00  |
    /// // 0x54 | f4 1b d1    | bra ne, -0x2f       | -> 0x25 poll
    /// // 0x57 | f0 07 bd    | mov   $r0, 0xbd     | EXIT BY BOUND ($r0 = FFFFFFBD)
    /// // 0x5a | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = FFFFFFBD
    /// // 0x5d | f8 02       | exit                |
    /// // cmd2_exit:
    /// // 0x5f | f0 07 04    | mov   $r0, 0x04     |
    /// // 0x62 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-ack)
    /// // 0x65 | f8 02       | exit                |
    /// // 0x67 | 00 …        | (padding)           | 25 bytes -> 128 bytes = 32 words
    /// ```
    #[rustfmt::skip]
    pub const ECHO_A_BYTES: [u8; 128] = [
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
        0xb0, 0x44, 0x02,             // cmpu b32 $r4, 0x2
        0xf4, 0x0b, 0x2b,             // bra eq, +0x2b -> 0x5f cmd2_exit
        0xb0, 0x44, 0x01,             // cmpu b32 $r4, 0x1
        0xf4, 0x1b, 0x14,             // bra ne, +0x14 -> 0x4e dec
        0xf0, 0x07, 0x03,             // mov   $r0, 0x03
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xd0, 0x23, 0x00,             // iowr  I[$r2], $r3   (ACK)
        0xf0, 0x07, 0x04,             // mov   $r0, 0x04
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xb6, 0x52, 0x01,             // dec: sub b32 $r5, 0x1
        0xb0, 0x54, 0x00,             // cmpu b32 $r5, 0x0
        0xf4, 0x1b, 0xd1,             // bra ne, -0x2f -> 0x25 poll
        0xf0, 0x07, 0xbd,             // mov   $r0, 0xbd
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xf0, 0x07, 0x04,             // cmd2_exit: mov $r0, 0x04
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00
    ];

    /// Image B — the terminal POKE. Identical skeleton to [`ECHO_A_BYTES`],
    /// with the `$r8 = falcon_io(0x504)` setup restored at `0x10`/`0x14` and the
    /// constant ACK replaced by `iord $r4, I[$r8]` → `iowr I[$r2], $r4`, so
    /// `CC_SCRATCH[1]` carries the **value the falcon read out of `0x409504`**.
    ///
    /// Everything after `0x14` is the ECHO listing shifted by +7 bytes (the
    /// width of the two `$r8` instructions), which is why all three branch
    /// displacements differ from ECHO's by exactly the same amount.
    ///
    /// ```text
    /// // Addr | Bytes       | Instruction         | Note
    /// // -----|-------------|---------------------|-------------------------------------
    /// // 0x00 | f0 17 00    | mov   $r1, 0x00     | low half of I[CC_SCRATCH[0]]
    /// // 0x03 | f0 13 02    | sethi $r1, 0x02     | $r1 = 0x20000
    /// // 0x06 | f1 27 00 01 | mov   $r2, 0x0100   | low half of I[CC_SCRATCH[1]]
    /// // 0x0a | f0 23 02    | sethi $r2, 0x02     | $r2 = 0x20100
    /// // 0x0d | f0 37 01    | mov   $r3, 0x01     | (unused in B; kept for prologue parity)
    /// // 0x10 | f1 87 00 41 | mov   $r8, 0x4100   | low half of I[WRCMD_CMD]
    /// // 0x14 | f0 83 01    | sethi $r8, 0x01     | $r8 = 0x14100 = falcon_io(0x504)
    /// // 0x17 | f1 67 00 10 | mov   $r6, 0x1000   | $r6 = I[MAILBOX0]
    /// // 0x1b | f1 77 00 11 | mov   $r7, 0x1100   | $r7 = I[MAILBOX1]
    /// // 0x1f | f0 57 00    | mov   $r5, 0x00     | loop counter, low half
    /// // 0x22 | f1 53 10 00 | sethi $r5, 0x0010   | $r5 = ECHO_BOUND
    /// // 0x26 | f0 07 01    | mov   $r0, 0x01     |
    /// // 0x29 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-loop)
    /// // poll:
    /// // 0x2c | cf 14 00    | iord  $r4, I[$r1]   | read the command word
    /// // 0x2f | d0 64 00    | iowr  I[$r6], $r4   | MAILBOX0 = VALUE READ
    /// // 0x32 | f0 07 02    | mov   $r0, 0x02     |
    /// // 0x35 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-read)
    /// // 0x38 | b0 44 02    | cmpu b32 $r4, 0x02  |
    /// // 0x3b | f4 0b 2e    | bra eq, +0x2e       | -> 0x69 cmd2_exit
    /// // 0x3e | b0 44 01    | cmpu b32 $r4, 0x01  |
    /// // 0x41 | f4 1b 17    | bra ne, +0x17       | -> 0x58 dec
    /// // 0x44 | f0 07 03    | mov   $r0, 0x03     |
    /// // 0x47 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (pre-poke)
    /// // 0x4a | cf 84 00    | iord  $r4, I[$r8]   | ⛔ THE POKE: read 0x409504
    /// // 0x4d | d0 24 00    | iowr  I[$r2], $r4   | CC_SCRATCH[1] = the word read
    /// // 0x50 | f0 07 04    | mov   $r0, 0x04     |
    /// // 0x53 | d0 70 00    | iowr  I[$r7], $r0   | MAILBOX1 = phase (post-poke)
    /// // 0x56 | f8 02       | exit                |
    /// // dec:
    /// // 0x58 | b6 52 01    | sub b32 $r5, 0x01   |
    /// // 0x5b | b0 54 00    | cmpu b32 $r5, 0x00  |
    /// // 0x5e | f4 1b ce    | bra ne, -0x32       | -> 0x2c poll
    /// // 0x61 | f0 07 bd    | mov   $r0, 0xbd     | EXIT BY BOUND ($r0 = FFFFFFBD)
    /// // 0x64 | d0 70 00    | iowr  I[$r7], $r0   |
    /// // 0x67 | f8 02       | exit                |
    /// // cmd2_exit:
    /// // 0x69 | f0 07 04    | mov   $r0, 0x04     |
    /// // 0x6c | d0 70 00    | iowr  I[$r7], $r0   |
    /// // 0x6f | f8 02       | exit                |
    /// // 0x71 | 00 …        | (padding)           | 15 bytes -> 128 bytes = 32 words
    /// ```
    #[rustfmt::skip]
    pub const POKE_A_BYTES: [u8; 128] = [
        0xf0, 0x17, 0x00,             // mov   $r1, 0x00
        0xf0, 0x13, 0x02,             // sethi $r1, 0x02
        0xf1, 0x27, 0x00, 0x01,       // mov   $r2, 0x0100
        0xf0, 0x23, 0x02,             // sethi $r2, 0x02
        0xf0, 0x37, 0x01,             // mov   $r3, 0x1
        0xf1, 0x87, 0x00, 0x41,       // mov   $r8, 0x4100
        0xf0, 0x83, 0x01,             // sethi $r8, 0x01 ($r8 = 0x14100)
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
        0xb0, 0x44, 0x02,             // cmpu b32 $r4, 0x2
        0xf4, 0x0b, 0x2e,             // bra eq, +0x2e -> 0x69 cmd2_exit
        0xb0, 0x44, 0x01,             // cmpu b32 $r4, 0x1
        0xf4, 0x1b, 0x17,             // bra ne, +0x17 -> 0x58 dec
        0xf0, 0x07, 0x03,             // mov   $r0, 0x03
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xcf, 0x84, 0x00,             // iord  $r4, I[$r8]   THE POKE
        0xd0, 0x24, 0x00,             // iowr  I[$r2], $r4
        0xf0, 0x07, 0x04,             // mov   $r0, 0x04
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xb6, 0x52, 0x01,             // dec: sub b32 $r5, 0x1
        0xb0, 0x54, 0x00,             // cmpu b32 $r5, 0x0
        0xf4, 0x1b, 0xce,             // bra ne, -0x32 -> 0x2c poll
        0xf0, 0x07, 0xbd,             // mov   $r0, 0xbd
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xf0, 0x07, 0x04,             // cmd2_exit: mov $r0, 0x04
        0xd0, 0x70, 0x00,             // iowr  I[$r7], $r0
        0xf8, 0x02,                   // exit
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
    ];

    /// Pack a 128-byte Falcon instruction stream into the 32 little-endian
    /// `u32` words IMEMD expects.
    pub const fn pack128(b: &[u8; 128]) -> [u32; 32] {
        let mut out = [0u32; 32];
        let mut w = 0;
        while w < 32 {
            let i = w * 4;
            out[w] = (b[i] as u32)
                | ((b[i + 1] as u32) << 8)
                | ((b[i + 2] as u32) << 16)
                | ((b[i + 3] as u32) << 24);
            w += 1;
        }
        out
    }

    pub const UCODE_CTX_ECHO_A: [u32; 32] = pack128(&ECHO_A_BYTES);
    pub const UCODE_CTX_POKE_A: [u32; 32] = pack128(&POKE_A_BYTES);

    // ---------------------------------------------------------------------
    // Instruction constructors (envytools Falcon ISA v4; docs/hw/falcon/*.rst).
    //
    // Every assertion below is written against these, never against a byte
    // literal with the intent in a trailing comment. A literal `0xf4, 0x2b`
    // "bra eq" reads fine and is not `bra eq`; `bra(BRA_EQ, …)` cannot be
    // wrong in that way.
    // ---------------------------------------------------------------------

    /// `bra e` — predicate byte of the `f4` form.
    const BRA_EQ: u8 = 0x0b;
    /// `bra ne`.
    const BRA_NE: u8 = 0x1b;

    const fn mov_i8(reg: u8, imm: u8) -> [u8; 3] {
        [0xf0, (reg << 4) | 0x07, imm]
    }
    const fn mov_i16(reg: u8, imm: u16) -> [u8; 4] {
        [0xf1, (reg << 4) | 0x07, (imm & 0xFF) as u8, (imm >> 8) as u8]
    }
    const fn sethi_i8(reg: u8, imm: u8) -> [u8; 3] {
        [0xf0, (reg << 4) | 0x03, imm]
    }
    const fn sethi_i16(reg: u8, imm: u16) -> [u8; 4] {
        [0xf1, (reg << 4) | 0x03, (imm & 0xFF) as u8, (imm >> 8) as u8]
    }
    const fn cmpu_b32_i8(reg: u8, imm: u8) -> [u8; 3] {
        [0xb0, (reg << 4) | 0x04, imm]
    }
    const fn sub_b32_i8(reg: u8, imm: u8) -> [u8; 3] {
        [0xb6, (reg << 4) | 0x02, imm]
    }
    const fn bra(cc: u8, disp: u8) -> [u8; 3] {
        [0xf4, cc, disp]
    }
    /// `iord $r<dst>, I[$r<ptr>]`
    const fn iord(dst: u8, ptr: u8) -> [u8; 3] {
        [0xcf, (ptr << 4) | dst, 0x00]
    }
    /// `iowr I[$r<ptr>], $r<src>` — **asynchronous** IO write, opcode `0xd0`.
    const fn iowr(ptr: u8, src: u8) -> [u8; 3] {
        [0xd0, (ptr << 4) | src, 0x00]
    }
    /// `iowrs I[$r<ptr>], $r<src>` — **synchronous** IO write, opcode `0xd1`.
    ///
    /// One nibble apart from [`iowr`] and a whole different guarantee: the `s`
    /// form blocks until the write has actually landed. Writing `0xd0` where
    /// `0xd1` was meant is invisible in a hex blob and produces a program that
    /// "runs" while its observable never appears — which is exactly how a magic
    /// write has been lost before. Every FENCE store uses this constructor, and
    /// the FENCE lattice asserts the async form appears nowhere in the image.
    const fn iowrs(ptr: u8, src: u8) -> [u8; 3] {
        [0xd1, (ptr << 4) | src, 0x00]
    }
    const fn exit_inst() -> [u8; 2] {
        [0xf8, 0x02]
    }

    const fn slice2(b: &[u8], at: usize) -> [u8; 2] {
        [b[at], b[at + 1]]
    }
    const fn slice3(b: &[u8], at: usize) -> [u8; 3] {
        [b[at], b[at + 1], b[at + 2]]
    }
    const fn slice4(b: &[u8], at: usize) -> [u8; 4] {
        [b[at], b[at + 1], b[at + 2], b[at + 3]]
    }
    const fn eq2(a: &[u8; 2], b: &[u8; 2]) -> bool {
        a[0] == b[0] && a[1] == b[1]
    }
    const fn eq3(a: &[u8; 3], b: &[u8; 3]) -> bool {
        a[0] == b[0] && a[1] == b[1] && a[2] == b[2]
    }
    const fn eq4(a: &[u8; 4], b: &[u8; 4]) -> bool {
        a[0] == b[0] && a[1] == b[1] && a[2] == b[2] && a[3] == b[3]
    }
    /// Resolve a branch at `at` by arithmetic: `at + sign_extend(disp)`.
    ///
    /// ⚠ **The Falcon branch displacement is relative to the address of the
    /// branch instruction itself, not to the following instruction.** envydis
    /// resolves `bra` against the instruction's own start address, and every
    /// image in this module is authored under that rule. Re-deriving it as
    /// `at + 3 + disp` — "PC already points past the instruction", the
    /// intuition from most other ISAs — shifts every target in every image by
    /// exactly the instruction width, which is a class of bug that reads
    /// perfectly in a listing and lands in the middle of the previous
    /// instruction on silicon. This function and [`bra_to`] are inverses of
    /// one another under that rule; assert BOTH on every branch and the two
    /// derivations have to meet at the label address.
    const fn bra_target(b: &[u8], at: usize) -> isize {
        at as isize + b[at + 2] as i8 as isize
    }
    /// `bra <cc>, <label>` — the displacement **computed** from the branch's own
    /// address and the label's address, never typed.
    ///
    /// This is the half of the lattice that a byte checksum cannot be: a
    /// hand-typed displacement is only ever compared against a number derived
    /// independently from the two addresses the listing names. Typing the same
    /// wrong displacement in the image and in the assertion is not possible,
    /// because the assertion does not contain a displacement at all.
    const fn bra_to(cc: u8, at: usize, target: usize) -> [u8; 3] {
        let d = target as isize - at as isize;
        assert!(d >= -128 && d <= 127); // I8 displacement form
        bra(cc, d as u8)
    }
    /// A port immediate narrowed to the `mov_i16` form, with the narrowing
    /// CHECKED rather than assumed.
    ///
    /// `IO_MAILBOX0 as u16` is correct today only because the port happens to
    /// be `0x1000`. A port above 16 bits needs a `mov`/`sethi` pair, not a wider
    /// immediate — so a bare `as u16` on one would not merely truncate a number,
    /// it would assert the wrong instruction FORM and silently drop the high
    /// half. This turns that into a build failure.
    const fn port_i16(port: u32) -> u16 {
        assert!(port <= 0xFFFF, "port needs a mov/sethi pair, not a bare I16");
        port as u16
    }
    /// True if the 3-byte instruction `needle` occurs anywhere in `b`.
    const fn contains3(b: &[u8], needle: &[u8; 3]) -> bool {
        let mut i = 0;
        while i + 3 <= b.len() {
            if eq3(&slice3(b, i), needle) {
                return true;
            }
            i += 1;
        }
        false
    }
    /// True if the 4-byte instruction `needle` occurs anywhere in `b`.
    const fn contains4(b: &[u8], needle: &[u8; 4]) -> bool {
        let mut i = 0;
        while i + 4 <= b.len() {
            if eq4(&slice4(b, i), needle) {
                return true;
            }
            i += 1;
        }
        false
    }
    /// True if every byte from `from` to the end of the image is zero.
    ///
    /// Bounded by `b.len()`, not by a literal — these helpers took a `[u8; 128]`
    /// when 128 was the only image size, and a 192-byte image passed through a
    /// `< 128` bound would leave its last 64 bytes unexamined while still
    /// reporting a clean tail.
    const fn zero_tail(b: &[u8], from: usize) -> bool {
        let mut i = from;
        while i < b.len() {
            if b[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    // ---------------------------------------------------------------------
    // ECHO — full coverage. The `at` values below are contiguous: each
    // assertion's offset is the previous one's offset plus that instruction's
    // width, from 0x00 through 0x67, and `zero_tail` closes 0x67..128. There
    // is no byte in this array that no assertion looks at.
    // ---------------------------------------------------------------------
    const _: () = {
        let b = &ECHO_A_BYTES;
        // prologue — ports derived, never hand-written (spec §3)
        assert!(eq3(&slice3(b, 0x00), &mov_i8(1, (IO_CC_SCRATCH0 & 0xFF) as u8)));
        assert!(eq3(&slice3(b, 0x03), &sethi_i8(1, (IO_CC_SCRATCH0 >> 16) as u8)));
        assert!(eq4(&slice4(b, 0x06), &mov_i16(2, (IO_CC_SCRATCH1 & 0xFFFF) as u16)));
        assert!(eq3(&slice3(b, 0x0a), &sethi_i8(2, (IO_CC_SCRATCH1 >> 16) as u8)));
        assert!(eq3(&slice3(b, 0x0d), &mov_i8(3, 1))); // the ack value
        assert!(eq4(&slice4(b, 0x10), &mov_i16(6, port_i16(IO_MAILBOX0))));
        assert!(eq4(&slice4(b, 0x14), &mov_i16(7, port_i16(IO_MAILBOX1))));
        assert!(eq3(&slice3(b, 0x18), &mov_i8(5, 0x00)));
        assert!(eq4(&slice4(b, 0x1b), &sethi_i16(5, (ECHO_BOUND >> 16) as u16)));
        assert!(eq3(&slice3(b, 0x1f), &mov_i8(0, PHASE_A_PRELOOP)));
        assert!(eq3(&slice3(b, 0x22), &iowr(7, 0)));
        // poll: @0x25
        assert!(eq3(&slice3(b, 0x25), &iord(4, 1)));
        assert!(eq3(&slice3(b, 0x28), &iowr(6, 4)));
        assert!(eq3(&slice3(b, 0x2b), &mov_i8(0, PHASE_A_POSTREAD)));
        assert!(eq3(&slice3(b, 0x2e), &iowr(7, 0)));
        assert!(eq3(&slice3(b, 0x31), &cmpu_b32_i8(4, 2)));
        assert!(eq3(&slice3(b, 0x34), &bra_to(BRA_EQ, 0x34, 0x5f))); // cmd2_exit
        assert!(bra_target(b, 0x34) == 0x5f); // …and back the other way
        assert!(eq3(&slice3(b, 0x37), &cmpu_b32_i8(4, 1)));
        assert!(eq3(&slice3(b, 0x3a), &bra_to(BRA_NE, 0x3a, 0x4e))); // dec
        assert!(bra_target(b, 0x3a) == 0x4e);
        assert!(eq3(&slice3(b, 0x3d), &mov_i8(0, PHASE_A_PREACK)));
        assert!(eq3(&slice3(b, 0x40), &iowr(7, 0)));
        assert!(eq3(&slice3(b, 0x43), &iowr(2, 3))); // CC_SCRATCH[1] = $r3 = 1
        assert!(eq3(&slice3(b, 0x46), &mov_i8(0, PHASE_A_POSTACK)));
        assert!(eq3(&slice3(b, 0x49), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x4c), &exit_inst()));
        // dec: @0x4e
        assert!(eq3(&slice3(b, 0x4e), &sub_b32_i8(5, 1)));
        assert!(eq3(&slice3(b, 0x51), &cmpu_b32_i8(5, 0)));
        assert!(eq3(&slice3(b, 0x54), &bra_to(BRA_NE, 0x54, 0x25))); // poll
        assert!(bra_target(b, 0x54) == 0x25);
        assert!(eq3(&slice3(b, 0x57), &mov_i8(0, PHASE_A_BOUND as u8)));
        assert!(eq3(&slice3(b, 0x5a), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x5d), &exit_inst()));
        // cmd2_exit: @0x5f
        assert!(eq3(&slice3(b, 0x5f), &mov_i8(0, PHASE_A_POSTACK)));
        assert!(eq3(&slice3(b, 0x62), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x65), &exit_inst()));
        assert!(zero_tail(b, 0x67));

        // ⛔ THE POINT OF THE ARC, asserted rather than promised: the ECHO
        // image contains no path to 0x409504. Neither the port setup nor the
        // read appears anywhere in the 128 bytes.
        assert!(!contains4(b, &mov_i16(8, (IO_WRCMD_CMD & 0xFFFF) as u16)));
        assert!(!contains3(b, &sethi_i8(8, (IO_WRCMD_CMD >> 16) as u8)));
        assert!(!contains3(b, &iord(4, 8)));
    };

    // The s37-acked prologue is preserved word-for-word (the four words that
    // executed on metal at sitting #37 must not have moved), in both images.
    const _: () = assert!(UCODE_CTX_ECHO_A[0] == 0xf000_17f0);
    const _: () = assert!(UCODE_CTX_ECHO_A[1] == 0x27f1_0213);
    const _: () = assert!(UCODE_CTX_ECHO_A[2] == 0x23f0_0100);
    const _: () = assert!(UCODE_CTX_ECHO_A[3] == 0x0137_f002);
    const _: () = assert!(UCODE_CTX_POKE_A[0] == 0xf000_17f0);
    const _: () = assert!(UCODE_CTX_POKE_A[1] == 0x27f1_0213);
    const _: () = assert!(UCODE_CTX_POKE_A[2] == 0x23f0_0100);
    const _: () = assert!(UCODE_CTX_POKE_A[3] == 0x0137_f002);

    // The counter is initialised to exactly ECHO_BOUND in each image — read
    // back out of the `sethi` immediate where it actually sits.
    const _: () =
        assert!(((ECHO_A_BYTES[0x1d] as u32) | ((ECHO_A_BYTES[0x1e] as u32) << 8)) << 16 == ECHO_BOUND);
    const _: () =
        assert!(((POKE_A_BYTES[0x24] as u32) | ((POKE_A_BYTES[0x25] as u32) << 8)) << 16 == ECHO_BOUND);

    // ---------------------------------------------------------------------
    // POKE — full coverage, same contiguity property, 0x00 through 0x71.
    // ---------------------------------------------------------------------
    const _: () = {
        let b = &POKE_A_BYTES;
        assert!(eq3(&slice3(b, 0x00), &mov_i8(1, (IO_CC_SCRATCH0 & 0xFF) as u8)));
        assert!(eq3(&slice3(b, 0x03), &sethi_i8(1, (IO_CC_SCRATCH0 >> 16) as u8)));
        assert!(eq4(&slice4(b, 0x06), &mov_i16(2, (IO_CC_SCRATCH1 & 0xFFFF) as u16)));
        assert!(eq3(&slice3(b, 0x0a), &sethi_i8(2, (IO_CC_SCRATCH1 >> 16) as u8)));
        assert!(eq3(&slice3(b, 0x0d), &mov_i8(3, 1)));
        // ⛔ the poison port, derived — this is the one place 0x504 may appear
        assert!(eq4(&slice4(b, 0x10), &mov_i16(8, (IO_WRCMD_CMD & 0xFFFF) as u16)));
        assert!(eq3(&slice3(b, 0x14), &sethi_i8(8, (IO_WRCMD_CMD >> 16) as u8)));
        assert!(eq4(&slice4(b, 0x17), &mov_i16(6, port_i16(IO_MAILBOX0))));
        assert!(eq4(&slice4(b, 0x1b), &mov_i16(7, port_i16(IO_MAILBOX1))));
        assert!(eq3(&slice3(b, 0x1f), &mov_i8(5, 0x00)));
        assert!(eq4(&slice4(b, 0x22), &sethi_i16(5, (ECHO_BOUND >> 16) as u16)));
        assert!(eq3(&slice3(b, 0x26), &mov_i8(0, PHASE_A_PRELOOP)));
        assert!(eq3(&slice3(b, 0x29), &iowr(7, 0)));
        // poll: @0x2c
        assert!(eq3(&slice3(b, 0x2c), &iord(4, 1)));
        assert!(eq3(&slice3(b, 0x2f), &iowr(6, 4)));
        assert!(eq3(&slice3(b, 0x32), &mov_i8(0, PHASE_A_POSTREAD)));
        assert!(eq3(&slice3(b, 0x35), &iowr(7, 0)));
        assert!(eq3(&slice3(b, 0x38), &cmpu_b32_i8(4, 2)));
        assert!(eq3(&slice3(b, 0x3b), &bra_to(BRA_EQ, 0x3b, 0x69))); // cmd2_exit
        assert!(bra_target(b, 0x3b) == 0x69);
        assert!(eq3(&slice3(b, 0x3e), &cmpu_b32_i8(4, 1)));
        assert!(eq3(&slice3(b, 0x41), &bra_to(BRA_NE, 0x41, 0x58))); // dec
        assert!(bra_target(b, 0x41) == 0x58);
        assert!(eq3(&slice3(b, 0x44), &mov_i8(0, PHASE_A_PREACK)));
        assert!(eq3(&slice3(b, 0x47), &iowr(7, 0)));
        assert!(eq3(&slice3(b, 0x4a), &iord(4, 8))); // ⛔ the poke
        assert!(eq3(&slice3(b, 0x4d), &iowr(2, 4))); // CC_SCRATCH[1] = value read
        assert!(eq3(&slice3(b, 0x50), &mov_i8(0, PHASE_A_POSTACK)));
        assert!(eq3(&slice3(b, 0x53), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x56), &exit_inst()));
        // dec: @0x58
        assert!(eq3(&slice3(b, 0x58), &sub_b32_i8(5, 1)));
        assert!(eq3(&slice3(b, 0x5b), &cmpu_b32_i8(5, 0)));
        assert!(eq3(&slice3(b, 0x5e), &bra_to(BRA_NE, 0x5e, 0x2c))); // poll
        assert!(bra_target(b, 0x5e) == 0x2c);
        assert!(eq3(&slice3(b, 0x61), &mov_i8(0, PHASE_A_BOUND as u8)));
        assert!(eq3(&slice3(b, 0x64), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x67), &exit_inst()));
        // cmd2_exit: @0x69
        assert!(eq3(&slice3(b, 0x69), &mov_i8(0, PHASE_A_POSTACK)));
        assert!(eq3(&slice3(b, 0x6c), &iowr(7, 0)));
        assert!(eq2(&slice2(b, 0x6f), &exit_inst()));
        assert!(zero_tail(b, 0x71));

        // The poke happens exactly once: one `iord I[$r8]` in the image.
        let mut n = 0;
        let mut i = 0;
        while i + 3 <= 128 {
            if eq3(&slice3(b, i), &iord(4, 8)) {
                n += 1;
            }
            i += 1;
        }
        assert!(n == 1);
    };

    // =====================================================================
    // FENCE — the falcon asserts CHAN_VALID.
    //
    // The experiment: PFIFO refuses to validate our channel with `err=0x2`.
    // Host writes to ENGINE_STATUS have never satisfied it (spec §6 refutation
    // 7 — CHAN_CUR/CHAN_NEXT take, ENGINE_STATUS stays 0). Candidate 1 is that
    // the context-valid assertion must ORIGINATE FROM THE FALCON, the way a
    // real context-switch completion would. This image writes CHAN_VALID to
    // ENGINE_STATUS from inside FECS, reads it back so we learn whether the bit
    // even took, holds it while the host re-validates the channel, and clears it
    // on command.
    //
    // What this measures is CHANNEL VALIDATION — the host writes PFIFO_CHAN[1]
    // and reads the error at 0x252c, apples-to-apples with the existing
    // validate legs. It does NOT submit a runlist: the real runlist submit
    // (0x2270/0x2274) is downstream of this leg and untouched by it.
    //
    // Decisive both ways: `err=0` proves PFIFO only trusts falcon-originated
    // state; `err=0x2` eliminates the candidate and points at engine binding.
    // =====================================================================

    /// ENGINE_STATUS (`+0xC00`), falcon-side. **DERIVED (untested)** — see the
    /// assertion in [`super::regs`]. This is the register the whole arc turns on.
    pub const IO_ENGINE_STATUS: u32 = falcon_io(0xC00);

    /// `ENGINE_STATUS.CHAN_VALID` — bit 1. The same value the host has written
    /// here fruitlessly since s35; the variable under test is *who* writes it.
    pub const CHAN_VALID: u32 = 0x0000_0002;

    /// FENCE's identity, written to MAILBOX0 by its first executed instruction.
    ///
    /// This is what separates three otherwise identical silences: `mb0 ==
    /// FENCE_MAGIC` means this image is uploaded and running; `mb0 == MB_SEED`
    /// means nothing ran at all; anything else — notably a value left by the
    /// ECHO leg that ran before us — means IMEM held stale bytes and the
    /// upload did not take. Without it, "the mailbox did not change" and "a
    /// different program is running" read exactly alike.
    pub const FENCE_MAGIC: u32 = 0xA55E_7A55;

    /// Host→falcon commands on CC_SCRATCH[0].
    pub const CMD_FENCE_ASSERT: u32 = 1;
    pub const CMD_FENCE_CLEAR: u32 = 3;

    // FENCE phase stamps on MAILBOX1. Every one of these is a *distinct*
    // terminal or transitional state; the host's gate compares against these
    // constants and never against a hex literal.
    pub const PHASE_FENCE_PRELOOP: u8 = 0x01;
    pub const PHASE_FENCE_PREASSERT: u8 = 0x03;
    pub const PHASE_FENCE_ASSERTED: u8 = 0x04;
    pub const PHASE_FENCE_DONE: u8 = 0x05;

    /// poll2's give-up marker — **distinct from [`PHASE_A_BOUND`]**, and for the
    /// same reason that constant exists: `mov $r0, 0xbc` is a *signed* I8, so the
    /// register and MAILBOX1 hold `0xFFFF_FFBC`.
    ///
    /// The two bounds must not share a value. `PHASE_A_BOUND` on MAILBOX1 means
    /// "the falcon never saw the ASSERT command"; `PHASE_FENCE_BOUND2` means "it
    /// asserted, held, and never saw the CLEAR command". Those license opposite
    /// conclusions about the host↔FECS channel, and a single marker would make
    /// them untellable.
    ///
    /// ⚠ Both are `u32` on purpose. `0xFFFF_FFBD` as a `u8` truncates to `0xBD`
    /// and never matches the sign-extended word the host reads back; and as a
    /// word it satisfies any naive `> 0` or `>= 3` progress test, so the host
    /// gate must test these constants for EQUALITY and must do it BEFORE any
    /// range test.
    pub const PHASE_FENCE_BOUND2: u32 = 0xFFFF_FFBC;

    /// Image FENCE — assert `CHAN_VALID` into `ENGINE_STATUS` from inside FECS.
    ///
    /// Register map:
    ///   `$r1` = `I[CC_SCRATCH[0]]` host→falcon command
    ///   `$r2` = `I[CC_SCRATCH[1]]` falcon→host ack — its OWN observable, so
    ///           "acked" and "mb0 changed" are two facts and not one
    ///   `$r3` = `CHAN_VALID` (2)
    ///   `$r4` = scratch (value read)
    ///   `$r5` = poll1 budget      `$r9` = poll2 budget (separate, so poll2 does
    ///           not inherit whatever poll1 left over)
    ///   `$r6` = `I[MAILBOX0]`     `$r7` = `I[MAILBOX1]`
    ///   `$r8` = `I[ENGINE_STATUS]`  ⭐ the target of the whole arc
    ///
    /// Every store is `iowrs` (`0xd1`, synchronous). The lattice asserts the
    /// async `0xd0` form appears nowhere in the image.
    ///
    /// ```text
    /// // Addr | Bytes       | Instruction           | Note
    /// // -----|-------------|-----------------------|-----------------------------------
    /// // 0x00 | f0 17 00    | mov   $r1, 0x00       | low half of I[CC_SCRATCH[0]]
    /// // 0x03 | f0 13 02    | sethi $r1, 0x02       | $r1 = 0x20000                (s37)
    /// // 0x06 | f1 27 00 01 | mov   $r2, 0x0100     | low half of I[CC_SCRATCH[1]]
    /// // 0x0a | f0 23 02    | sethi $r2, 0x02       | $r2 = 0x20100                (s37)
    /// // 0x0d | f1 67 00 10 | mov   $r6, 0x1000     | $r6 = I[MAILBOX0]            (s29)
    /// // 0x11 | f1 77 00 11 | mov   $r7, 0x1100     | $r7 = I[MAILBOX1]            (s30)
    /// // 0x15 | f0 87 00    | mov   $r8, 0x00       | low half of I[ENGINE_STATUS]
    /// // 0x18 | f0 83 03    | sethi $r8, 0x03       | $r8 = 0x30000     DERIVED, untested
    /// // 0x1b | f0 37 02    | mov   $r3, 0x02       | CHAN_VALID
    /// // 0x1e | f1 07 55 7a | mov   $r0, 0x7A55     |
    /// // 0x22 | f1 03 5e a5 | sethi $r0, 0xA55E     | $r0 = A55E7A55 = FENCE_MAGIC
    /// // 0x26 | d1 60 00    | iowrs I[$r6], $r0     | MAILBOX0 = MAGIC  <-- first store
    /// // 0x29 | f0 57 00    | mov   $r5, 0x00       |
    /// // 0x2c | f1 53 10 00 | sethi $r5, 0x0010     | $r5 = 00100000 = poll1 budget
    /// // 0x30 | f0 97 00    | mov   $r9, 0x00       |
    /// // 0x33 | f1 93 10 00 | sethi $r9, 0x0010     | $r9 = 00100000 = poll2 budget
    /// // 0x37 | f0 07 01    | mov   $r0, 0x01       |
    /// // 0x3a | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = phase 01 (pre-loop)
    /// // poll1:                                     | wait for CMD_FENCE_ASSERT
    /// // 0x3d | cf 14 00    | iord  $r4, I[$r1]     | read the command word
    /// // 0x40 | b0 44 01    | cmpu b32 $r4, 0x01    | CMD_FENCE_ASSERT?
    /// // 0x43 | f4 0b 14    | bra eq, +0x14         | -> 0x57 do_assert
    /// // 0x46 | b6 52 01    | sub b32 $r5, 0x01     |
    /// // 0x49 | b0 54 00    | cmpu b32 $r5, 0x00    |
    /// // 0x4c | f4 1b f1    | bra ne, -0x0f         | -> 0x3d poll1
    /// // giveup1:                                   | fall through on exhaustion
    /// // 0x4f | f0 07 bd    | mov   $r0, 0xbd       | $r0 = FFFFFFBD = PHASE_A_BOUND
    /// // 0x52 | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = FFFFFFBD
    /// // 0x55 | f8 02       | exit                  | never saw the ASSERT command
    /// // do_assert:
    /// // 0x57 | f0 07 03    | mov   $r0, 0x03       |
    /// // 0x5a | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = phase 03 (pre-assert)
    /// // 0x5d | d1 83 00    | iowrs I[$r8], $r3     | ⭐ ENGINE_STATUS = CHAN_VALID
    /// // 0x60 | cf 84 00    | iord  $r4, I[$r8]     | read ENGINE_STATUS back
    /// // 0x63 | d1 64 00    | iowrs I[$r6], $r4     | MAILBOX0 = what it reads as
    /// // 0x66 | d1 23 00    | iowrs I[$r2], $r3     | CC_SCRATCH[1] = 2 (ACK, own obs.)
    /// // 0x69 | f0 07 04    | mov   $r0, 0x04       |
    /// // 0x6c | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = phase 04 (holding)
    /// // poll2:                                     | hold across the host's validate
    /// // 0x6f | cf 14 00    | iord  $r4, I[$r1]     |
    /// // 0x72 | b0 44 03    | cmpu b32 $r4, 0x03    | CMD_FENCE_CLEAR?
    /// // 0x75 | f4 0b 14    | bra eq, +0x14         | -> 0x89 do_clear
    /// // 0x78 | b6 92 01    | sub b32 $r9, 0x01     | poll2's OWN counter
    /// // 0x7b | b0 94 00    | cmpu b32 $r9, 0x00    |
    /// // 0x7e | f4 1b f1    | bra ne, -0x0f         | -> 0x6f poll2
    /// // giveup2:                                   | fall through on exhaustion
    /// // 0x81 | f0 07 bc    | mov   $r0, 0xbc       | $r0 = FFFFFFBC, DISTINCT marker
    /// // 0x84 | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = FFFFFFBC
    /// // 0x87 | f8 02       | exit                  | asserted, never saw CLEAR
    /// // do_clear:
    /// // 0x89 | f0 47 00    | mov   $r4, 0x00       |
    /// // 0x8c | d1 84 00    | iowrs I[$r8], $r4     | ENGINE_STATUS = 0 (unwind)
    /// // 0x8f | f0 07 05    | mov   $r0, 0x05       |
    /// // 0x92 | d1 70 00    | iowrs I[$r7], $r0     | MAILBOX1 = phase 05 (done)
    /// // 0x95 | f8 02       | exit                  | terminal: clean unwind
    /// // 0x97 | 00 …        | (padding)             | 151 bytes -> 192 = 48 words
    /// ```
    #[rustfmt::skip]
    pub const FENCE_A_BYTES: [u8; 192] = [
        0xf0, 0x17, 0x00,             // mov   $r1, 0x00
        0xf0, 0x13, 0x02,             // sethi $r1, 0x02
        0xf1, 0x27, 0x00, 0x01,       // mov   $r2, 0x0100
        0xf0, 0x23, 0x02,             // sethi $r2, 0x02
        0xf1, 0x67, 0x00, 0x10,       // mov   $r6, 0x1000
        0xf1, 0x77, 0x00, 0x11,       // mov   $r7, 0x1100
        0xf0, 0x87, 0x00,             // mov   $r8, 0x00
        0xf0, 0x83, 0x03,             // sethi $r8, 0x03   ($r8 = 0x30000)
        0xf0, 0x37, 0x02,             // mov   $r3, 0x02    CHAN_VALID
        0xf1, 0x07, 0x55, 0x7a,       // mov   $r0, 0x7A55
        0xf1, 0x03, 0x5e, 0xa5,       // sethi $r0, 0xA55E  ($r0 = FENCE_MAGIC)
        0xd1, 0x60, 0x00,             // iowrs I[$r6], $r0  MAILBOX0 = MAGIC
        0xf0, 0x57, 0x00,             // mov   $r5, 0x00
        0xf1, 0x53, 0x10, 0x00,       // sethi $r5, 0x0010  poll1 budget
        0xf0, 0x97, 0x00,             // mov   $r9, 0x00
        0xf1, 0x93, 0x10, 0x00,       // sethi $r9, 0x0010  poll2 budget
        0xf0, 0x07, 0x01,             // mov   $r0, 0x01
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xcf, 0x14, 0x00,             // poll1: iord $r4, I[$r1]
        0xb0, 0x44, 0x01,             // cmpu b32 $r4, 0x01
        0xf4, 0x0b, 0x14,             // bra eq, +0x14 -> 0x57 do_assert
        0xb6, 0x52, 0x01,             // sub b32 $r5, 0x01
        0xb0, 0x54, 0x00,             // cmpu b32 $r5, 0x00
        0xf4, 0x1b, 0xf1,             // bra ne, -0x0f -> 0x3d poll1
        0xf0, 0x07, 0xbd,             // giveup1: mov $r0, 0xbd
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xf0, 0x07, 0x03,             // do_assert: mov $r0, 0x03
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xd1, 0x83, 0x00,             // iowrs I[$r8], $r3   ⭐ THE ASSERT
        0xcf, 0x84, 0x00,             // iord  $r4, I[$r8]
        0xd1, 0x64, 0x00,             // iowrs I[$r6], $r4
        0xd1, 0x23, 0x00,             // iowrs I[$r2], $r3   ACK
        0xf0, 0x07, 0x04,             // mov   $r0, 0x04
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xcf, 0x14, 0x00,             // poll2: iord $r4, I[$r1]
        0xb0, 0x44, 0x03,             // cmpu b32 $r4, 0x03
        0xf4, 0x0b, 0x14,             // bra eq, +0x14 -> 0x89 do_clear
        0xb6, 0x92, 0x01,             // sub b32 $r9, 0x01
        0xb0, 0x94, 0x00,             // cmpu b32 $r9, 0x00
        0xf4, 0x1b, 0xf1,             // bra ne, -0x0f -> 0x6f poll2
        0xf0, 0x07, 0xbc,             // giveup2: mov $r0, 0xbc
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xf8, 0x02,                   // exit
        0xf0, 0x47, 0x00,             // do_clear: mov $r4, 0x00
        0xd1, 0x84, 0x00,             // iowrs I[$r8], $r4   ENGINE_STATUS = 0
        0xf0, 0x07, 0x05,             // mov   $r0, 0x05
        0xd1, 0x70, 0x00,             // iowrs I[$r7], $r0
        0xf8, 0x02,                   // exit
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    /// Pack a 192-byte Falcon instruction stream into 48 little-endian words.
    pub const fn pack192(b: &[u8; 192]) -> [u32; 48] {
        let mut out = [0u32; 48];
        let mut w = 0;
        while w < 48 {
            let i = w * 4;
            out[w] = (b[i] as u32)
                | ((b[i + 1] as u32) << 8)
                | ((b[i + 2] as u32) << 16)
                | ((b[i + 3] as u32) << 24);
            w += 1;
        }
        out
    }

    pub const UCODE_CTX_FENCE_A: [u32; 48] = pack192(&FENCE_A_BYTES);

    // ---------------------------------------------------------------------
    // FENCE — full coverage, 0x00 through 0x97, `zero_tail` closing 0x97..192.
    //
    // Read this as the bytes checked against the listing by arithmetic the
    // author did not perform. (For BRANCHES specifically, the paired
    // `bra_to`/`bra_target` asserts are exact inverses — the second is implied
    // by the first, and what it buys is one-sided-redefinition detection, not a
    // second opinion. See spec §11.3.) The listing above
    // was written first and the bytes typed from it; these assertions rebuild
    // the instructions from the *decoded* fields — opcode via the constructor,
    // register index via `reg << 4`, port immediate via `falcon_io()`, branch
    // displacement via `bra_to()` from the two label addresses — and check them
    // against the bytes. The two derivations meet at the listing. A byte-literal
    // assertion (`b[0x43] == 0xf4 && b[0x44] == 0x0b && b[0x45] == 0x14`) would
    // be a checksum of the same typing that produced the image and would agree
    // with a wrong displacement in both places; `bra_to(BRA_EQ, 0x43, 0x57)`
    // contains no displacement at all, so it cannot.
    // ---------------------------------------------------------------------
    const _: () = {
        let b = &FENCE_A_BYTES;
        // prologue — every port DERIVED through falcon_io, never hand-written
        assert!(eq3(&slice3(b, 0x00), &mov_i8(1, (IO_CC_SCRATCH0 & 0xFF) as u8)));
        assert!(eq3(&slice3(b, 0x03), &sethi_i8(1, (IO_CC_SCRATCH0 >> 16) as u8)));
        assert!(eq4(&slice4(b, 0x06), &mov_i16(2, (IO_CC_SCRATCH1 & 0xFFFF) as u16)));
        assert!(eq3(&slice3(b, 0x0a), &sethi_i8(2, (IO_CC_SCRATCH1 >> 16) as u8)));
        assert!(eq4(&slice4(b, 0x0d), &mov_i16(6, port_i16(IO_MAILBOX0))));
        assert!(eq4(&slice4(b, 0x11), &mov_i16(7, port_i16(IO_MAILBOX1))));
        assert!(eq3(&slice3(b, 0x15), &mov_i8(8, (IO_ENGINE_STATUS & 0xFF) as u8)));
        assert!(eq3(&slice3(b, 0x18), &sethi_i8(8, (IO_ENGINE_STATUS >> 16) as u8)));
        assert!(eq3(&slice3(b, 0x1b), &mov_i8(3, CHAN_VALID as u8)));
        // the magic, reassembled from the constant rather than re-typed
        assert!(eq4(&slice4(b, 0x1e), &mov_i16(0, (FENCE_MAGIC & 0xFFFF) as u16)));
        assert!(eq4(&slice4(b, 0x22), &sethi_i16(0, (FENCE_MAGIC >> 16) as u16)));
        assert!(eq3(&slice3(b, 0x26), &iowrs(6, 0)));
        assert!(eq3(&slice3(b, 0x29), &mov_i8(5, 0x00)));
        assert!(eq4(&slice4(b, 0x2c), &sethi_i16(5, (ECHO_BOUND >> 16) as u16)));
        assert!(eq3(&slice3(b, 0x30), &mov_i8(9, 0x00)));
        assert!(eq4(&slice4(b, 0x33), &sethi_i16(9, (ECHO_BOUND >> 16) as u16)));
        assert!(eq3(&slice3(b, 0x37), &mov_i8(0, PHASE_FENCE_PRELOOP)));
        assert!(eq3(&slice3(b, 0x3a), &iowrs(7, 0)));
        // poll1: @0x3d
        assert!(eq3(&slice3(b, 0x3d), &iord(4, 1)));
        assert!(eq3(&slice3(b, 0x40), &cmpu_b32_i8(4, CMD_FENCE_ASSERT as u8)));
        assert!(eq3(&slice3(b, 0x43), &bra_to(BRA_EQ, 0x43, 0x57))); // do_assert
        assert!(bra_target(b, 0x43) == 0x57);
        assert!(eq3(&slice3(b, 0x46), &sub_b32_i8(5, 1)));
        assert!(eq3(&slice3(b, 0x49), &cmpu_b32_i8(5, 0)));
        assert!(eq3(&slice3(b, 0x4c), &bra_to(BRA_NE, 0x4c, 0x3d))); // poll1
        assert!(bra_target(b, 0x4c) == 0x3d);
        // giveup1: @0x4f — reached by falling out of poll1
        assert!(eq3(&slice3(b, 0x4f), &mov_i8(0, PHASE_A_BOUND as u8)));
        assert!(eq3(&slice3(b, 0x52), &iowrs(7, 0)));
        assert!(eq2(&slice2(b, 0x55), &exit_inst()));
        // do_assert: @0x57
        assert!(eq3(&slice3(b, 0x57), &mov_i8(0, PHASE_FENCE_PREASSERT)));
        assert!(eq3(&slice3(b, 0x5a), &iowrs(7, 0)));
        assert!(eq3(&slice3(b, 0x5d), &iowrs(8, 3))); // ⭐ ENGINE_STATUS <- $r3
        assert!(eq3(&slice3(b, 0x60), &iord(4, 8))); // read it back
        assert!(eq3(&slice3(b, 0x63), &iowrs(6, 4))); // MAILBOX0 <- readback
        assert!(eq3(&slice3(b, 0x66), &iowrs(2, 3))); // CC_SCRATCH[1] <- ack
        assert!(eq3(&slice3(b, 0x69), &mov_i8(0, PHASE_FENCE_ASSERTED)));
        assert!(eq3(&slice3(b, 0x6c), &iowrs(7, 0)));
        // poll2: @0x6f
        assert!(eq3(&slice3(b, 0x6f), &iord(4, 1)));
        assert!(eq3(&slice3(b, 0x72), &cmpu_b32_i8(4, CMD_FENCE_CLEAR as u8)));
        assert!(eq3(&slice3(b, 0x75), &bra_to(BRA_EQ, 0x75, 0x89))); // do_clear
        assert!(bra_target(b, 0x75) == 0x89);
        assert!(eq3(&slice3(b, 0x78), &sub_b32_i8(9, 1))); // poll2's own counter
        assert!(eq3(&slice3(b, 0x7b), &cmpu_b32_i8(9, 0)));
        assert!(eq3(&slice3(b, 0x7e), &bra_to(BRA_NE, 0x7e, 0x6f))); // poll2
        assert!(bra_target(b, 0x7e) == 0x6f);
        // giveup2: @0x81
        assert!(eq3(&slice3(b, 0x81), &mov_i8(0, PHASE_FENCE_BOUND2 as u8)));
        assert!(eq3(&slice3(b, 0x84), &iowrs(7, 0)));
        assert!(eq2(&slice2(b, 0x87), &exit_inst()));
        // do_clear: @0x89
        assert!(eq3(&slice3(b, 0x89), &mov_i8(4, 0)));
        assert!(eq3(&slice3(b, 0x8c), &iowrs(8, 4))); // ENGINE_STATUS <- 0
        assert!(eq3(&slice3(b, 0x8f), &mov_i8(0, PHASE_FENCE_DONE)));
        assert!(eq3(&slice3(b, 0x92), &iowrs(7, 0)));
        assert!(eq2(&slice2(b, 0x95), &exit_inst()));
        assert!(zero_tail(b, 0x97));

        // Both counters are seeded to exactly ECHO_BOUND, read back out of the
        // `sethi` immediates where they actually sit.
        assert!(((b[0x2e] as u32) | ((b[0x2f] as u32) << 8)) << 16 == ECHO_BOUND);
        assert!(((b[0x35] as u32) | ((b[0x36] as u32) << 8)) << 16 == ECHO_BOUND);

        // The two give-up markers are DIFFERENT bytes, in the image and not just
        // in the constants — this is the property the host gate depends on to
        // tell "never saw ASSERT" from "never saw CLEAR".
        assert!(b[0x51] != b[0x83]);
        assert!(b[0x51] == PHASE_A_BOUND as u8);
        assert!(b[0x83] == PHASE_FENCE_BOUND2 as u8);

        // ⛔ Every store in this image is the SYNCHRONOUS form. The async `iowr`
        // (0xd0) differs from `iowrs` (0xd1) by one bit, reads identically in a
        // hex blob, and produces a program whose observables silently never
        // arrive. Not one 0xd0 store to any of our five ports may exist.
        assert!(!contains3(b, &iowr(6, 0)));
        assert!(!contains3(b, &iowr(6, 4)));
        assert!(!contains3(b, &iowr(7, 0)));
        assert!(!contains3(b, &iowr(2, 3)));
        assert!(!contains3(b, &iowr(8, 3)));
        assert!(!contains3(b, &iowr(8, 4)));

        // ⛔ FENCE must never touch 0x409504: the first access to that offset
        // faults and wedges every subsequent read in the unit for the rest of
        // the boot (spec §5.4), which would void the verdict this arc exists to
        // collect. Neither the port setup nor an access may appear.
        assert!(!contains4(b, &mov_i16(8, (IO_WRCMD_CMD & 0xFFFF) as u16)));
        assert!(!contains3(b, &sethi_i8(8, (IO_WRCMD_CMD >> 16) as u8)));

        // The assert happens exactly once, and the clear exactly once.
        let mut n_assert = 0;
        let mut n_clear = 0;
        let mut i = 0;
        while i + 3 <= 192 {
            if eq3(&slice3(b, i), &iowrs(8, 3)) {
                n_assert += 1;
            }
            if eq3(&slice3(b, i), &iowrs(8, 4)) {
                n_clear += 1;
            }
            i += 1;
        }
        assert!(n_assert == 1);
        assert!(n_clear == 1);
    };

    // The FENCE image fits inside the single 0x40-word IMEM page the upload pads
    // and the TLB attests. If it ever outgrew the page, the pad loop would
    // underflow and the tail of the program would never be written.
    const _: () = assert!(UCODE_CTX_FENCE_A.len() <= 0x40);
}

/// s26/s28 FTDI-ring budget: the 0x640000 window is PARKED (triple-refuted),
/// and its four 256-row dumps cost ~54 KiB of the 64 KiB drop-oldest boot ring
/// (drivers/xhci/ftdi.rs) — enough to evict the display and ucode legs from the
/// capture. Values are still collected and summarised; only the dense rows are
/// silenced. Flip to re-enable the raw dumps.
const MIRROR_HDR_DENSE: bool = false;

/// Rate of [`crate::arch::now_cycles`] in Hz, or `None` when it is not known on this
/// platform — the boot-calibrated invariant TSC on x86 (`apic::calibrate`, which runs
/// long before `pci::init` and therefore before this driver), `None` everywhere else.
///
/// `None` is not "zero elapsed": callers that time a bounded poll with this must print
/// raw cycles and say the clock was a guess, never a fabricated millisecond.
#[cfg(target_arch = "x86_64")]
fn poll_hz() -> Option<u64> {
    match crate::arch::apic::tsc_hz() {
        0 => None, // calibration never ran or was rejected
        hz => Some(hz),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn poll_hz() -> Option<u64> {
    None
}

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
        let mut t_last = crate::arch::ms();
        macro_rules! phase {
            ($name:expr) => {
                let t_now = crate::arch::ms();
                serial_println!(":: kdisp: bring-up phase={} d={} ::", $name, t_now.wrapping_sub(t_last));
                t_last = t_now;
            }
        }

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
        // Citation: rnndb/display/nv_evo.xml defines NV_EVO_CORE at 0x640000.
        // It provides the pushbuffer method layout for the EVO core channel.
        let mut mirror_hdr_pre = [0u32; 256];
        for (i, offset) in (0..=0x3FC).step_by(4).enumerate() {
            let val = mmio_read(bar0, 0x640000 + offset);
            mirror_hdr_pre[i] = val;
            if MIRROR_HDR_DENSE { serial_println!(":: kepler: mirror-hdr pre off={:03X} val={:08X} ::", offset, val); }
        }
        serial_println!(":: kepler: mirror-hdr pre done rows=256 ::");
        phase!("pmc_vram_init");

        let fb_offset = crate::drivers::gpu::kepler_display::takeover_display(
            gpu, bar0, &mut vram_allocator, &mut kdisp_trace,
        );
        serial_println!(":: kdisp: landed trace [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
            kdisp_trace[0], kdisp_trace[1], kdisp_trace[2], kdisp_trace[3],
            kdisp_trace[4], kdisp_trace[5], kdisp_trace[6]);
        phase!("kdisp_takeover");

        // 7. PGRAPH 2D/3D Engine Init (Placeholder)
        // Kepler requires Falcon microcode to fully initialize PGRAPH.
        // We log its presence but leave it disabled to prevent hangs.
        let pgraph_status = mmio_read(bar0, regs::NV_PGRAPH_BASE);
        serial_println!("[NVIDIA] PGRAPH Engine Status (0x400000): 0x{:08X}. Requires firmware for full 2D/3D.", pgraph_status);

        // Recon Probe before any engine state modification

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
            fecs_write(bar0, pfifo_base + 0x390, 1 << 0);
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
                                    phase!("pfifo_alloc_zero");

                                    let chan_id = 1;

                                    // Setup Channel Instance Block
                                    //
                                    // ⛔ CLEAN-ROOM: UNAUDITED (docs/MANIFESTO/CLEAN_ROOM_POLICY.md §5).
                                    // The RAMFC instance-block layout below — every offset and every
                                    // magic constant in the writes that follow, through `+0xFC` — has
                                    // NO Group-A provenance. The audit claimed for these constants in
                                    // an earlier round was WITHDRAWN by its own author, who recorded
                                    // that it was performed by reading GPL `nouveau` sources (a §5
                                    // Group-B violation) and that it was flawed on its merits besides,
                                    // having quoted this tree's own code back as if it were canonical.
                                    // No code here was authored from that source; the constants
                                    // predate it. But nothing may claim they are validated until they
                                    // are re-derived from a Group-A-legal source (envytools hwdocs /
                                    // rnndb) or from vendor documentation.
                                    //
                                    // This disclaimer lives HERE, at the writes themselves, and not in
                                    // a proposal citing a line range — the two ranges cited for it so
                                    // far were both wrong, and a warning that does not sit on the code
                                    // it warns about is not a warning.
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

                                    let chid_0 = 1u32;
                                    let chid_1 = 2u32;
                                    let chid_2 = 3u32;
                                    let entry_0 = chid_0;
                                    let entry_1 = chid_1 | (1 << 31);
                                    let entry_2 = (chid_2 << 1) | 1;

                                    // The three runlist entries, six words, in the order the scheduler
                                    // reads them out of the runlist page.
                                    let runlist_words: [u32; 6] = [entry_0, 0, entry_1, 0, entry_2, 0];

                                    // ENTRIES (not words) handed to RUNLIST_SUBMIT (0x2274). The submit
                                    // and the acceptance poll both derive from THIS — never from a
                                    // literal. They drifted apart once already: the submit was raised
                                    // 1 → 3 while the poll kept demanding `(len & 0xFFF) == 1`, a
                                    // predicate that then could not be satisfied on any boot.
                                    const RUNLIST_LEN: u32 = 3;
                                    const _: () = assert!(RUNLIST_LEN as usize * 2 == 6); // words per entry

                                    // Width of the mirror-window beacon plant below: EIGHT words over
                                    // `off + 0..31`, in each of three regions. Everything that undoes
                                    // the plant must be this wide. A restore or a scan narrower than
                                    // the plant reports CLEAN over residue it never looked at — which
                                    // is exactly what the first version of this rebuild did, leaving
                                    // 0xBEAC0007/0xBEAC0008 alive at `runlist_off + 24/28` under a
                                    // `CLEAN` verdict.
                                    const BEACON_PLANT_WORDS: usize = 8;
                                    const _: () = assert!(BEACON_PLANT_WORDS >= 6); // the six authored words fit

                                    // Put the runlist page back the way this driver left it. Called
                                    // twice — here, and again immediately before the submit, because
                                    // the beacon plant below is destructive over the same bytes and
                                    // must stay planted through its pass-2 re-read.
                                    //
                                    // Words 0..5 are the three authored entries. Words 6..7 lie past
                                    // the last entry but are still inside the plant, and are still our
                                    // page: the zeroing loop above clears all 0x1000 bytes of it, so
                                    // zero — not "whatever the plant left" — is the state this driver
                                    // established and the state the page returns to. With LEN=3 the
                                    // scheduler should not read past `+23`, but "should" is not a thing
                                    // to leave beacon words behind on.
                                    let write_runlist = || unsafe {
                                        for (i, w) in runlist_words.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, *w);
                                        }
                                        for i in runlist_words.len()..BEACON_PLANT_WORDS {
                                            core::ptr::write_volatile((bar1 + runlist_off + i * 4) as *mut u32, 0);
                                        }
                                    };

                                    // 1. Write Runlist VRAM FIRST
                                    write_runlist();

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
                                    phase!("runlist_write_and_pass0");

                                    // Plant Beacons
                                    let pattern = [
                                        0xBEAC0001, 0xBEAC0002, 0xBEAC0003, 0xBEAC0004,
                                        0xBEAC0005, 0xBEAC0006, 0xBEAC0007, 0xBEAC0008,
                                    ];

                                    // The plant below is DESTRUCTIVE over `off + 0..31` in THREE
                                    // regions. The runlist's eight words have an author in this
                                    // function (`runlist_words`), so it is rebuilt from the author.
                                    // USERD and the pushbuffer do not:
                                    //
                                    //   * USERD is written exactly once before this point — by the
                                    //     page-zeroing loop above — so its eight words should be zero,
                                    //     and the instance block at `inst_off + 0x08/0x0C` was pointed
                                    //     at it in that state. "Should be" is not "is", which is why
                                    //     the words are read rather than assumed.
                                    //     What the hardware expects of these eight words specifically:
                                    //     envytools `docs/hw/fifo/dma-pusher.rst`, "Channel control
                                    //     area", enumerates every usable address in the area — DMA_PUT
                                    //     0x40, DMA_GET 0x44, REF 0x48, DMA_PUT_HIGH 0x4C, 0x50
                                    //     (GF100+), DMA_CGET 0x54, DMA_MGET 0x58/0x5C, DMA_GET_HIGH
                                    //     0x60, IB_GET 0x88, IB_PUT 0x8C — and none of them lies below
                                    //     0x40. The plant's eight words are 0x00..0x1C, so they overlap
                                    //     no documented field, and this driver's own historic GP_GET/
                                    //     GP_PUT witness (added 1c9e2570, removed 51b98bab at pull 15,
                                    //     which is BEFORE the beacons landed at 200be275 / pull 16)
                                    //     read 0x8C/0x90 — also outside the plant.
                                    //     That narrows the damage; it does not license leaving it. The
                                    //     doc lists what a DRIVER may use, not what the CHIP keeps
                                    //     there, and the same section records that on GF100 the area
                                    //     moved out of BAR0 into VRAM reached through BAR1 — memory the
                                    //     chip DMAs its own channel state into. Zero is the state this
                                    //     driver established for the whole page and zero is what the
                                    //     capture should read back; nothing here invents a value.
                                    //   * The pushbuffer is never initialised by this driver at all:
                                    //     the zeroing loop covers inst/gpfifo/userd/runlist/fence and
                                    //     omits `pb_off`, and no other write to it exists. Its words
                                    //     are untouched VRAM handed out by the bump allocator. There
                                    //     is no correct constant to put back — writing zeros would be
                                    //     a behaviour change smuggled in under the word "restore".
                                    //
                                    // So both are CAPTURED, not authored: read the exact eight words
                                    // the plant is about to destroy, put those same words back once the
                                    // probe's consumers are done. The captured array is the single
                                    // source of truth for the restore, so the plant and the restore
                                    // cannot drift apart the way the runlist's submit and poll did.
                                    let save_beacon_window = |off: usize| -> [u32; 8] {
                                        let mut w = [0u32; 8];
                                        for (i, s) in w.iter_mut().enumerate() {
                                            *s = unsafe {
                                                core::ptr::read_volatile((bar1 + off + i * 4) as *const u32)
                                            };
                                        }
                                        w
                                    };

                                    // THE one write-point for each restored region. Writes the saved
                                    // words back and immediately reads them again: `beacon_resid`
                                    // counts words still holding a beacon value, `mismatch` counts
                                    // words that are not what was saved. Both zero is the only healthy
                                    // state; without the read-back a silently-failed restore would look
                                    // exactly like a good one.
                                    //
                                    // ALL EIGHT WORDS, and `words=8` is printed so the claim is
                                    // checkable. The plant is eight words wide; a restore or a scan
                                    // narrower than the plant reports CLEAN over residue it never
                                    // looked at. That is not hypothetical — the runlist restore as
                                    // a470ba16 first landed it rebuilt and scanned only its six
                                    // authored words, so 0xBEAC0007/0xBEAC0008 survived at
                                    // `runlist_off + 24/28` under a `CLEAN` verdict. That is now
                                    // fixed at its own site (`write_runlist` / the `runlist-rebuild`
                                    // scan, both `BEACON_PLANT_WORDS` wide); the lesson stays because
                                    // the next region added here will be tempted the same way.
                                    //
                                    // `restored=CLEAN` asserts exactly one thing: the eight words the
                                    // plant destroyed are back to the eight words it destroyed. It says
                                    // NOTHING about whether those contents are the right contents for
                                    // the chip — that is a separate question this leg does not answer
                                    // and must not be read as answering.
                                    //
                                    // The read-back is honest about the CPU side: BAR1 is mapped
                                    // PCD|PWT with the PTE PAT bit clear (`arch::memory::map_mmio_window`),
                                    // which selects PAT entry 3 — left at the power-on UC; only PA4 is
                                    // retyped, to WC, and only PTEs that set the PAT bit reach it. UC
                                    // reads are strongly ordered and are never served from a cache line
                                    // or a write-combining buffer, so this really does observe a failed
                                    // write. What it does NOT prove is that the GPU's own fetch path
                                    // (through its VM, not through BAR1) sees the same bytes — that
                                    // question is the whole reason the mirror-window probe exists, and
                                    // this line does not settle it.
                                    let restore_beacon_window = |label: &str, off: usize, w: &[u32; 8]| {
                                        unsafe {
                                            for (i, v) in w.iter().enumerate() {
                                                core::ptr::write_volatile((bar1 + off + i * 4) as *mut u32, *v);
                                            }
                                        }
                                        let mut resid = 0u32;
                                        let mut mismatch = 0u32;
                                        for (i, want) in w.iter().enumerate() {
                                            let got = unsafe {
                                                core::ptr::read_volatile((bar1 + off + i * 4) as *const u32)
                                            };
                                            if (0xBEAC0001..=0xBEAC0008).contains(&got) {
                                                resid += 1;
                                            }
                                            if got != *want {
                                                mismatch += 1;
                                            }
                                        }
                                        serial_println!(
                                            ":: kepler: beacon-restore at={} off={:08X} w=[{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] words={} beacon_resid={} mismatch={} restored={} ::",
                                            label, off,
                                            w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
                                            w.len(), resid, mismatch,
                                            if resid == 0 && mismatch == 0 { "CLEAN" } else { "CORRUPT" }
                                        );
                                    };

                                    let userd_saved = save_beacon_window(userd_off);
                                    let pb_saved = save_beacon_window(pb_off);
                                    // Printed before the plant lines below, so the serial order alone
                                    // proves the capture preceded the destruction. It also witnesses
                                    // the zeroing loop: `at=userd w=[00000000 x8]` is the expected
                                    // reading, and anything else is news about BAR1, not about here.
                                    serial_println!(
                                        ":: kepler: beacon-save at=userd off={:08X} w=[{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
                                        userd_off, userd_saved[0], userd_saved[1], userd_saved[2], userd_saved[3],
                                        userd_saved[4], userd_saved[5], userd_saved[6], userd_saved[7]
                                    );
                                    serial_println!(
                                        ":: kepler: beacon-save at=pb off={:08X} w=[{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
                                        pb_off, pb_saved[0], pb_saved[1], pb_saved[2], pb_saved[3],
                                        pb_saved[4], pb_saved[5], pb_saved[6], pb_saved[7]
                                    );

                                    unsafe {
                                        // userd — DESTRUCTIVE: 8 words over `userd_off + 0..31`, the
                                        // head of the channel control area the chip owns the moment
                                        // PFIFO_CHAN[1] goes VALID. Saved above, put back by
                                        // `restore_beacon_window` immediately before that write.
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + userd_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=userd off={:08X} ::", userd_off);

                                        // pushbuffer — DESTRUCTIVE: 8 words over `pb_off + 0..31`.
                                        // Saved above, put back at the same site as USERD.
                                        for (i, val) in pattern.iter().enumerate() {
                                            core::ptr::write_volatile((bar1 + pb_off + i * 4) as *mut u32, *val);
                                        }
                                        serial_println!(":: kepler: beacon planted at=pb off={:08X} ::", pb_off);

                                        // runlist — DESTRUCTIVE: 8 words over `runlist_off + 0..31`
                                        // covers all six words of all three entries written above,
                                        // plus two words past them that the page-zeroing loop had
                                        // cleared. The pass-1/pass-2 scans below are this plant's
                                        // consumer; `write_runlist()` rebuilds all eight after pass 2,
                                        // before the submit. Do not move the submit above pass 2.
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
                                    phase!("plant_and_pass1");
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
                                    let pdisplay_1 = fecs_read(bar0, disp_base + 0x40);
                                    let evo_core = fecs_read(bar0, disp_base + 0x490);
                                    let evo_userd_ptr = fecs_read(bar0, disp_base + 0x494);
                                    serial_println!(":: kepler: disp-userd-recon pdisplay_0={:08X} +40={:08X} evo_0x490={:08X} evo_0x494={:08X} ::", pdisplay_0, pdisplay_1, evo_core, evo_userd_ptr);
                                    phase!("mirror_passes");

                                    // Milestone 2: PGRAPH Falcon Reconnaissance (Pull 18 + Pull 19)
                                    let pmc_en_pre = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-pulse pre={:08X} ::", pmc_en_pre);

                                    mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_en_pre & !(1 << 12));
                                    let pmc_en_off = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    serial_println!(":: kepler: pgraph-pulse off rb={:08X} ::", pmc_en_off);
                                    
                                    for _ in 0..2_000_000 { core::hint::spin_loop(); }
                                    
                                    mmio_write(bar0, regs::NV_PMC_ENABLE, pmc_en_pre | (1 << 12));
                                    let pmc_en_on = mmio_read(bar0, regs::NV_PMC_ENABLE);
                                    let recon_chan_cur = fecs_read(bar0, 0x409b00);
                                    let recon_chan_next = fecs_read(bar0, 0x409b04);
                                    let recon_engine_status = fecs_read(bar0, 0x409c00);
                                    let recon_engine_trigger = fecs_read(bar0, 0x409c08);
                                    let recon_wrcmd_data = fecs_read(bar0, 0x409500);
                                    serial_println!(":: kepler: recon (healthy: BADF1000/0s, unpowered: BADF1200) CHAN_CUR={:08X} CHAN_NEXT={:08X} ENGINE_STATUS={:08X} ENGINE_TRIGGER={:08X} WRCMD_DATA={:08X} ::",
                                        recon_chan_cur, recon_chan_next, recon_engine_status, recon_engine_trigger, recon_wrcmd_data);

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
                                                    let val = fecs_read(bar0, base + offset);
                                                    let abs = if val == 0xFFFFFFFF || val == 0xBAD0BA20 || val == 0xBADF1000 { " ABSENT?" } else { "" };
                                                    serial_println!(":: kepler: {} b={:06X} off={:03X} val={:08X}{} ::", tag, base, offset, val, abs);
                                                }
                                            } }
                                            let cpuctl = fecs_read(bar0, base + 0x100);
                                            let imemc = fecs_read(bar0, base + 0x180);
                                            let dmemc = fecs_read(bar0, base + 0x1C0);
                                            serial_println!(":: kepler: fal-base b={:06X} verdict cpuctl={:08X} imemc={:08X} dmemc={:08X} ::", base, cpuctl, imemc, dmemc);

                                            // K-GPU-4 Pull 24: Falcon Sentinel Port Probe
                                            fecs_write(bar0, base + 0x180, 1 << 24); // IMEMC offset=0, AINCW
                                            let imemc_rb = fecs_read(bar0, base + 0x180);
                                            serial_println!(":: kepler: fal-port b={:06X} imemc wr=01000000 rb={:08X} ::", base, imemc_rb);
                                            
                                            fecs_write(bar0, base + 0x184, 0xDEADBEEF);
                                            fecs_write(bar0, base + 0x184, 0xCAFEF00D);
                                            fecs_write(bar0, base + 0x184, 0x12345678);
                                            fecs_write(bar0, base + 0x184, 0xA5A55A5A);
                                            
                                            fecs_write(bar0, base + 0x180, 1 << 25); // reset offset, AINCR
                                            let imem_w0 = fecs_read(bar0, base + 0x184);
                                            let imem_w1 = fecs_read(bar0, base + 0x184);
                                            let imem_w2 = fecs_read(bar0, base + 0x184);
                                            let imem_w3 = fecs_read(bar0, base + 0x184);
                                            serial_println!(":: kepler: fal-port b={:06X} imem rb w0={:08X} w1={:08X} w2={:08X} w3={:08X} ::", base, imem_w0, imem_w1, imem_w2, imem_w3);
                                            
                                            fecs_write(bar0, base + 0x1C0, 1 << 24); // DMEMC offset=0, AINCW
                                            let dmemc_rb = fecs_read(bar0, base + 0x1C0);
                                            serial_println!(":: kepler: fal-port b={:06X} dmemc wr=01000000 rb={:08X} ::", base, dmemc_rb);
                                            
                                            fecs_write(bar0, base + 0x1C4, 0xDEADBEEF);
                                            fecs_write(bar0, base + 0x1C4, 0xCAFEF00D);
                                            fecs_write(bar0, base + 0x1C4, 0x12345678);
                                            fecs_write(bar0, base + 0x1C4, 0xA5A55A5A);
                                            
                                            fecs_write(bar0, base + 0x1C0, 1 << 25); // reset offset, AINCR
                                            let dmem_w0 = fecs_read(bar0, base + 0x1C4);
                                            let dmem_w1 = fecs_read(bar0, base + 0x1C4);
                                            let dmem_w2 = fecs_read(bar0, base + 0x1C4);
                                            let dmem_w3 = fecs_read(bar0, base + 0x1C4);
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
                                        use ucode::UCODE_CTX_ECHO_A;
                                        const MB_SEED: u32 = 0xA5A5_0000;
                                        // IMEM page granularity: the code TLB marks a page usable only when the
                                        // last word of the 0x40-word page is written (nouveau pads for this reason).
                                        const IMEM_PAGE_WORDS: usize = 0x40;
                                        
                                        let base = 0x409000;
                                        
                                        for &(img_label, img, want) in &[("A", &UCODE_A, 0xF00DFACEu32), ("B", &UCODE_B, 0xF00DBEEFu32)] {
                                            let port = if img_label == "A" { 0x1000 } else { 0x0040 };
                                            serial_println!(":: kepler: ucode img={} ioport={:04X} want={:08X} ::", img_label, port, want);
                                        
                                            // Seed the mailbox so "unchanged" has exactly one meaning.
                                            fecs_write(bar0, base + 0x040, MB_SEED);
                                            let pre_mb0 = fecs_read(bar0, base + 0x040);
                                            let pre_cpuctl = fecs_read(bar0, base + 0x100);
                                            serial_println!(":: kepler: ucode pre mailbox0={:08X} cpuctl={:08X} ::", pre_mb0, pre_cpuctl);

                                            // Halt before rewriting IMEM, and prove it. Pass A can
                                            // leave the core started-and-stalled (the s28 signature
                                            // 0x00000012 — trigger latched, core refused), and pass B
                                            // then rewrites IMEM underneath it. The unit is at rest on
                                            // the first pass, so this is a no-op there and load-
                                            // bearing on the second.
                                            fecs_write(bar0, base + 0x100, 0x10); // CPUCTL <- STOPPED
                                            let ab_halt = fecs_read(bar0, base + 0x100);
                                            serial_println!(":: kepler: ucode halt img={} cpuctl={:08X} halted={} ::",
                                                img_label, ab_halt, if (ab_halt & 0x10) != 0 { "Y" } else { "N" });

                                            // Upload, padding the full IMEM page so the code TLB marks it usable.
                                            fecs_write(bar0, base + 0x180, 1 << 24); // IMEMC offset=0, AINCW
                                            fecs_write(bar0, base + 0x188, 0);       // IMEMT tag=0 (matches BOOTVEC=0)
                                            for &word in img.iter() {
                                                fecs_write(bar0, base + 0x184, word);
                                            }
                                            for _ in img.len()..IMEM_PAGE_WORDS {
                                                fecs_write(bar0, base + 0x184, 0);
                                            }
                                            serial_println!(":: kepler: ucode uploaded words={} padded={} ::", img.len(), IMEM_PAGE_WORDS);
                                        
                                            // Page-usable attestation: TLB_CMD PTLB query on virtual page 0.
                                            fecs_write(bar0, base + 0x140, 0x0200_0000);
                                            let tlb_rd = fecs_read(bar0, base + 0x144);
                                            serial_println!(":: kepler: ucode tlb page0={:08X} ::", tlb_rd);
                                        
                                            fecs_write(bar0, base + 0x180, 1 << 25); // IMEMC offset=0, AINCR
                                            let mut verify_ok = true;
                                            let mut rb = [0u32; 5];
                                            for k in 0..img.len() {
                                                rb[k] = fecs_read(bar0, base + 0x184);
                                                if rb[k] != img[k] { verify_ok = false; }
                                            }
                                            serial_println!(":: kepler: ucode verify ok={} w0={:08X} w1={:08X} w2={:08X} w3={:08X} w4={:08X} ::",
                                                if verify_ok { "Y" } else { "N" }, rb[0], rb[1], rb[2], rb[3], rb[4]);
                                        
                                            if !verify_ok {
                                                serial_println!(":: kepler: ucode ABORT verify-mismatch — BOOTVEC/CPUCTL NOT written ::");
                                                break;
                                            }
                                        
                                            let dmactl_pre = fecs_read(bar0, base + 0x10C);
                                            serial_println!(":: kepler: dmactl pre={:08X} ::", dmactl_pre);
                                            fecs_write(bar0, base + 0x10C, dmactl_pre & !1);
                                            let dmactl_post = fecs_read(bar0, base + 0x10C);
                                            serial_println!(":: kepler: dmactl post={:08X} ::", dmactl_post);

                                            if (dmactl_post & 1) != 0 {
                                                serial_println!(":: kepler: dmactl REFUSED ::");
                                                continue;
                                            }
fecs_write(bar0, base + 0x104, 0); // BOOTVEC=0
                                            fecs_write(bar0, base + 0x100, 2); // CPUCTL START_TRIGGER
                                            serial_println!(":: kepler: ucode start cpuctl<=00000002 ::");
                                        
                                            // Bounded poll for STOPPED (bit 4). halt-iters is the discriminator:
                                            // 0 = the poll proved nothing; >0 = the core demonstrably left the idle
                                            // state; max = started and stalled.
                                            let mut halt_iters = 0u32;
                                            for i in 0..100_000u32 {
                                                let c = fecs_read(bar0, base + 0x100);
                                                halt_iters = i;
                                                if (c & 0x10) != 0 { break; }
                                                core::hint::spin_loop();
                                            }
                                        
                                            let post_cpuctl = fecs_read(bar0, base + 0x100);
                                            let post_mb0 = fecs_read(bar0, base + 0x040);
                                            serial_println!(":: kepler: ucode end h2h3={} cpuctl={:08X} mailbox0={:08X} halt-iters={} ::",
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
                                        for &(h2h3_label, img, phase_bound) in &[
                                            ("on", &UCODE_CTX_ECHO_A[..], ucode::PHASE_A_BOUND),
                                            ("off", &UCODE_CTX_ECHO_A[..], ucode::PHASE_A_BOUND),
                                        ] {
                                            serial_println!(":: kepler: ucode-echo h2h3={} bound={} ::", h2h3_label, ucode::ECHO_BOUND);

                                            // ⛔ HALT THE CORE BEFORE REWRITING ITS IMEM, and verify the
                                            // halt by readback. This loop re-uploads on every pass, and
                                            // the leg before it started a core; rewriting IMEM underneath
                                            // a running falcon leaves it executing a half-old, half-new
                                            // program that goes on writing the very mailboxes the next
                                            // pass is about to read as its own result. `CPUCTL` bit 4 is
                                            // STOPPED (spec §2.1) — writing it requests the halt, reading
                                            // it back is the only thing that establishes one.
                                            fecs_write(bar0, base + 0x100, 0x10);
                                            let halt_rb = fecs_read(bar0, base + 0x100);
                                            serial_println!(":: kepler: ucode-echo halt h2h3={} cpuctl={:08X} halted={} ::",
                                                h2h3_label, halt_rb, if (halt_rb & 0x10) != 0 { "Y" } else { "N" });
                                            if (halt_rb & 0x10) == 0 {
                                                serial_println!(":: kepler: ucode-echo ABORT halt-refused h2h3={} — IMEM not rewritten under a live core ::", h2h3_label);
                                                continue;
                                            }

                                            let dmactl_pre = fecs_read(bar0, base + 0x10C);
                                            fecs_write(bar0, base + 0x10C, dmactl_pre & !1);

                                            fecs_write(bar0, base + 0x800, 0); // CC_SCRATCH[0]
                                            fecs_write(bar0, base + 0x804, MB_SEED); // CC_SCRATCH[1] (sentinel)
                                            // Seed both mailboxes so "unchanged" has exactly one meaning
                                            // (s29 discipline) for the two new observables as well.
                                            fecs_write(bar0, base + 0x040, MB_SEED); // MAILBOX0 <- value read
                                            fecs_write(bar0, base + 0x044, MB_SEED); // MAILBOX1 <- phase

                                            serial_println!(":: kepler: ucode-echo pre CC_SCRATCH[0]={:08X} CC_SCRATCH[1]={:08X} mb0={:08X} mb1={:08X} ::",
                                                fecs_read(bar0, base + 0x800), fecs_read(bar0, base + 0x804),
                                                fecs_read(bar0, base + 0x040), fecs_read(bar0, base + 0x044));

                                            fecs_write(bar0, base + 0x180, 1 << 24); // IMEMC AINCW
                                            fecs_write(bar0, base + 0x188, 0); // IMEMT tag=0
                                            for &word in img.iter() { fecs_write(bar0, base + 0x184, word); }
                                            for _ in img.len()..IMEM_PAGE_WORDS { fecs_write(bar0, base + 0x184, 0); }
                                            
                                            fecs_write(bar0, base + 0x180, 1 << 25); // IMEMC AINCR
                                            let mut verify_echo = true;
                                            for k in 0..img.len() {
                                                if fecs_read(bar0, base + 0x184) != img[k] { verify_echo = false; }
                                            }
                                            if !verify_echo {
                                                serial_println!(":: kepler: ucode-echo ABORT verify-mismatch h2h3={} ::", h2h3_label);
                                                continue;
                                            }
                                                                                        if h2h3_label == "on" {
                                                fecs_write(bar0, base + 0xC00, 2);
                                                let check_status = fecs_read(bar0, base + 0xC00);
                                                serial_println!(":: kepler: H2/H3 ENGINE_STATUS readback={:08X} ::", check_status);
                                                
                                                fecs_write(bar0, base + 0xC08, 1);
                                                let check_trigger = fecs_read(bar0, base + 0xC08);
                                                serial_println!(":: kepler: H2/H3 ENGINE_TRIGGER readback={:08X} ::", check_trigger);
                                            }
                                            fecs_write(bar0, base + 0x104, 0); // BOOTVEC=0
                                            fecs_write(bar0, base + 0x100, 2); // CPUCTL START_TRIGGER
                                            serial_println!(":: kepler: ucode-echo start h2h3={} ::", h2h3_label);
                                            
                                            fecs_write(bar0, base + 0x800, 1); // host-cmd
                                            serial_println!(":: kepler: ucode-echo host-cmd CC_SCRATCH[0]={:08X} ::", fecs_read(bar0, base + 0x800));
                                            
                                            let mut ack = 0;
                                            let mut ack_iters = 0;
                                            for i in 0..100_000u32 {
                                                ack = fecs_read(bar0, base + 0x804);
                                                ack_iters = i;
                                                if ack != MB_SEED { break; }
                                                core::hint::spin_loop();
                                            }
                                            
                                            serial_println!(":: kepler: ucode-echo host-ack CC_SCRATCH[1]={:08X} iters={} ::", ack, ack_iters);

                                            // R3-AMEND split observable: mb0 = the word the ucode READ out
                                            // of CC_SCRATCH[0]; mb1 = the phase it last reached. An ack with
                                            // mb0 != 1 would mean CC_SCRATCH[1] moved for some reason other
                                            // than our echo; no ack with a live phase localises the fault.
                                            let mb0 = fecs_read(bar0, base + 0x040);
                                            let phase = fecs_read(bar0, base + 0x044);
                                            serial_println!(":: kepler: ctx-echo h2h3={} ack={:08X} mb0={:08X} phase={:08X} class={} ::",
                                                h2h3_label, ack, mb0, phase, classify_fecs_word(ack));
                                            if phase == phase_bound {
                                                serial_println!(":: kepler: ctx-echo EXIT-BY-BOUND h2h3={} iters={} — command never observed ::",
                                                    h2h3_label, ucode::ECHO_BOUND);
                                                serial_println!(":: kepler: H3/H4 arm=Instrument-did-not-run (ECHO_BOUND) ::");
                                            }
                                            // The ECHO ucode acks with the literal `$r3 = 1`. Anything
                                            // else is not an ack: `class` names which not-an-ack it is
                                            // so a wedged unit and a silent falcon do not read alike.
                                            if ack == 1 {
                                                serial_println!(":: kepler: ucode-echo SUCCESS h2h3={} mb0={:08X} ::", h2h3_label, mb0);
                                                break;
                                            } else {
                                                serial_println!(":: kepler: ucode-echo NO-ACK h2h3={} ack={:08X} class={} ::",
                                                    h2h3_label, ack, classify_fecs_word(ack));
                                            }
                                        }

                                        // Read-only sweep of the unit window: locates either sentinel wherever it
                                        // actually landed (MAILBOX1 on an off-by-one, INTR on a wrong-port write).
                                        for off in (0..=0x1FC).step_by(4) {
                                            let val = fecs_read(bar0, base + off);
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

                                    let pre_wit_mb1 = fecs_read(bar0, 0x409000 + 0x044);
                                    let pre_wit_cpu = fecs_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: hb pre-witness mb1={:08X} cpuctl={:08X} ::", pre_wit_mb1, pre_wit_cpu);
                                    phase!("ucode_echo");

                                    // --- Restore USERD and the pushbuffer, before the channel goes live ---
                                    //
                                    // The mirror-window beacon probe planted 0xBEAC0001..8 over
                                    // `userd_off + 0..31` and `pb_off + 0..31`. Its consumers are the
                                    // pass-1 window scan and the pass-2 volatility re-read, both of
                                    // which read BAR0 0x640000 and both of which are long finished:
                                    // there is no read of 0x640000, and no reference to the beacon
                                    // range at all, anywhere between the end of pass 2 and this point.
                                    // So the probe's intent is fully served and the beacons are now
                                    // only damage — restore, never delete, exactly as a470ba16 argued
                                    // for the runlist.
                                    //
                                    // FIRST REAL USE, and why the restore lands HERE:
                                    //   * USERD — the next three writes hand PFIFO the instance block,
                                    //     whose 0x08/0x0C words point at `userd_off`. From the moment
                                    //     PFIFO_CHAN[1] is written VALID|POLL_ENABLE the chip owns that
                                    //     page and may write its own state into it. That write is
                                    //     USERD's first real use; this is the last instruction before it.
                                    //   * PUSHBUFFER — nothing in this driver reads, writes, or points
                                    //     at `pb_off` after the plant: the GPFIFO page is zeroed and no
                                    //     entry is ever written into it, so no fetch of the pushbuffer
                                    //     can be issued. Its first *possible* use is still gated on the
                                    //     same channel-validate below, so the same site is the correct
                                    //     one and no later site is safer.
                                    //
                                    // Latest legal point rather than earliest, for the reason a470ba16
                                    // rebuilt the runlist immediately before the submit: everything
                                    // between pass 2 and here pulses PGRAPH through a PMC reset and
                                    // pokes both falcons' IMEM/DMEM ports. A restore placed before that
                                    // would be witnessed against a state the chip never receives.
                                    restore_beacon_window("userd", userd_off, &userd_saved);
                                    restore_beacon_window("pb", pb_off, &pb_saved);

                                    // --- Witness Rematch ---
                                    serial_println!(":: kepler: witness-rematch begin (pgraph on) ::");

                                    // --- Recon: PFIFO/Channel Validate Strip ---
                                    let class_alive = |v: u32, zero_refute: &'static str| -> &'static str {
                                        let c = classify_fecs_word(v);
                                        if c == "VALUE" { "VALUE,alive" }
                                        else if c == "ZERO" { zero_refute }
                                        else if c == "POISON" { "POISON,severed" }
                                        else { "ABSENT,severed" }
                                    };
                                    let class_zero = |v: u32, val_refute: &'static str| -> &'static str {
                                        let c = classify_fecs_word(v);
                                        if c == "ZERO" { "ZERO,alive" }
                                        else if c == "VALUE" { val_refute }
                                        else if c == "POISON" { "POISON,severed" }
                                        else { "ABSENT,severed" }
                                    };
                                    let class_presubmit = |v: u32| -> &'static str {
                                        let c = classify_fecs_word(v);
                                        if c == "ZERO" { "ZERO,pre-submit" }
                                        else if c == "VALUE" { "VALUE,pre-submit-dirty" }
                                        else if c == "POISON" { "POISON,severed" }
                                        else { "ABSENT,severed" }
                                    };

                                    let inst_base_mem = core::ptr::read_volatile((bar1 + inst_off + 0x10) as *const u32);
                                    let gpfifo_ptr = core::ptr::read_volatile((bar1 + inst_off + 0x48) as *const u32);
                                    let pmc_en = mmio_read(bar0, 0x200);
                                    let subfifo_en = mmio_read(bar0, 0x204);
                                    let eng_mask = mmio_read(bar0, 0x2390);
                                    let playlist_base = mmio_read(bar0, 0x2270);
                                    let playlist_rd = mmio_read(bar0, 0x2280);
                                    let pfifo_intr = mmio_read(bar0, 0x2100);
                                    let pfifo_err = mmio_read(bar0, 0x252c);
                                    let sched_stat = mmio_read(bar0, 0x263c);

                                    serial_println!(":: kepler: recon inst_base_mem={:08X}({}) gpfifo_ptr={:08X}({}) ::",
                                        inst_base_mem, class_alive(inst_base_mem, "ZERO,refutes-memory"),
                                        gpfifo_ptr, class_alive(gpfifo_ptr, "ZERO,refutes-memory"));
                                    serial_println!(":: kepler: recon pmc_en={:08X}({}) subfifo_en={:08X}({}) eng_mask={:08X}({}) ::",
                                        pmc_en, class_alive(pmc_en, "ZERO,refutes-active"),
                                        subfifo_en, class_alive(subfifo_en, "ZERO,refutes-active"),
                                        eng_mask, class_alive(eng_mask, "ZERO,refutes-active"));
                                    serial_println!(":: kepler: recon playlist_base={:08X}({}) playlist_rd={:08X}({}) ::",
                                        playlist_base, class_presubmit(playlist_base),
                                        playlist_rd, class_presubmit(playlist_rd));

                                    if pfifo_err == 2 {
                                        serial_println!(":: kepler: recon pfifo_intr={:08X}({}) pfifo_err={:08X}(VALUE,NO_POLL) sched_stat={:08X}({}) ::",
                                            pfifo_intr, class_zero(pfifo_intr, "VALUE,INTR_PENDING"),
                                            pfifo_err, sched_stat, class_alive(sched_stat, "ZERO,refutes-active"));
                                    } else {
                                        let err_c = classify_fecs_word(pfifo_err);
                                        if err_c == "ZERO" {
                                            serial_println!(":: kepler: recon pfifo_intr={:08X}({}) pfifo_err={:08X}(ZERO,alive) sched_stat={:08X}({}) ::",
                                                pfifo_intr, class_zero(pfifo_intr, "VALUE,INTR_PENDING"),
                                                pfifo_err, sched_stat, class_alive(sched_stat, "ZERO,refutes-active"));
                                        } else if err_c == "POISON" || err_c == "ABSENT" {
                                            serial_println!(":: kepler: recon pfifo_intr={:08X}({}) pfifo_err={:08X}({},severed) sched_stat={:08X}({}) ::",
                                                pfifo_intr, class_zero(pfifo_intr, "VALUE,INTR_PENDING"),
                                                pfifo_err, err_c, sched_stat, class_alive(sched_stat, "ZERO,refutes-active"));
                                        } else {
                                            serial_println!(":: kepler: recon pfifo_intr={:08X}({}) pfifo_err={:08X}(VALUE,err=0x{:X},unnamed) sched_stat={:08X}({}) ::",
                                                pfifo_intr, class_zero(pfifo_intr, "VALUE,INTR_PENDING"),
                                                pfifo_err, pfifo_err,
                                                sched_stat, class_alive(sched_stat, "ZERO,refutes-active"));
                                        }
                                    }

                                    // FENCE Arc: Hypothesis 3 - ENGINE_TRIGGER host handshake (Placement Experiment)
                                    let eng_trig_pre = fecs_read(bar0, 0x409c08);
                                    serial_println!(":: kepler: recon eng_trig_pre={:08X} ::", eng_trig_pre);

                                    if eng_trig_pre == 1 {
                                        serial_println!(":: kepler: eng_trig_pre == 1 (NULL RESULT, write is no-op) ::");
                                    }

                                    fecs_write(bar0, 0x409c08, 1);
                                    let eng_trig_post = fecs_read(bar0, 0x409c08);
                                    serial_println!(":: kepler: recon eng_trig_post={:08X} ::", eng_trig_post);

                                    // 2. Bind and Enable PFIFO_CHAN for channel 1
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0); 
                                    mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400); 
                                    mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12)); 

                                    let err = mmio_read(bar0, 0x252c);
                                    let stat = mmio_read(bar0, 0x263c);
                                    // Review C3/C4: an unnamed wedge value must print what the chip
                                    // actually said, never the retired NO_POLL name.
                                    let err_c = class_zero(err, "VALUE,unnamed");
                                    let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                    serial_println!(":: kepler: sched-status post-init err={:08X} ({}) stat={:08X} ::", err, err_str, stat);

                                    if err == 0 {
                                        serial_println!(":: kepler: H3 arm=Worked ::");
                                    } else if err == 2 {
                                        serial_println!(":: kepler: H3 arm=Did-not-work (STRIPPED) ::");
                                    } else {
                                        serial_println!(":: kepler: H3 arm=Made-it-worse ({}) ::", err_c);
                                    }

                                    let ch_1_0_pre = mmio_read(bar0, 0x800000 + (1 * 8));
                                    let ch_1_4_pre = mmio_read(bar0, 0x800004 + (1 * 8));
                                    serial_println!(":: kepler: PFIFO_CHAN[1] pre-submit: 00={:08X} 04={:08X} ::", ch_1_0_pre, ch_1_4_pre);
                                    
                                    // Witness check
                                    if (ch_1_0_pre & 0xC0000000) != 0xC0000000 {
                                        serial_println!(":: kepler: WITNESS STRIPPED. Restoring inst_off+0x0C ::");
                                        // The restore must reproduce the CANONICAL write, bit for bit.
                                        // The original at the instance-block setup is
                                        // `((userd_off >> 32) as u32) | 0x80000000`; this restore
                                        // dropped the high bit, so a "restored" instance block was
                                        // never the one the channel was built with, and every `err=`
                                        // read after a strip has been taken against a *different*
                                        // instance block than the pre-strip reads.
                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32, ((userd_off >> 32) as u32) | 0x80000000);
                                        // Review C5: NO engine_trigger write-back on any exit. 0x409c08 is an
                                        // edge-semantic doorbell (the lane's own STUDY: DAEMON2CTXCTL_REQ /
                                        // CHSW_PENDING) — it has no pre-image to restore; writing eng_trig_pre
                                        // back would FIRE it again when pre==1 (the common case: :1372-1380
                                        // rings it every FIFO boot), and writing 0 is a no-op. The one write
                                        // at the experiment site leaves no latched residue by the same edge
                                        // semantics, so leaving it alone through the runlist submit is the
                                        // honest unwind. This also keeps the post-restore re-test below a
                                        // clean reading of the inst_off restore alone.

                                        // Re-test PFIFO_CHAN[1] to clear state
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0);
                                        mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400);
                                        mmio_write(bar0, 0x800000 + (1 * 8), 0xC0000000 | ((inst_off as u32) >> 12));
                                        
                                        let err = mmio_read(bar0, 0x252c);
                                        let stat = mmio_read(bar0, 0x263c);
                                        let err_str = if err == 0 || err == 0xFFFFFFFF || err == 0xBAD0BA20 { "absent?" } else { "present" };
                                        serial_println!(":: kepler: sched-status post-restore err={:08X} ({}) stat={:08X} ::", err, err_str, stat);
                                    } else {
                                        serial_println!(":: kepler: WITNESS PASSED - bits stuck! (engine_trigger left untouched — edge doorbell, no residue; see the C5 note on the STRIPPED arm) ::");
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

                                    let post_wit_scratch = fecs_read(bar0, 0x409000 + 0x804);
                                    let post_wit_cpu = fecs_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: ucode-echo post-witness CC_SCRATCH[1]={:08X} cpuctl={:08X} ::", post_wit_scratch, post_wit_cpu);
                                    
                                    for _ in 0..1_000_000 { core::hint::spin_loop(); }
                                    let final_scratch = fecs_read(bar0, 0x409000 + 0x804);
                                    let final_cpu = fecs_read(bar0, 0x409000 + 0x100);
                                    serial_println!(":: kepler: ucode-echo final CC_SCRATCH[1]={:08X} cpuctl={:08X} ::", final_scratch, final_cpu);

                                    // ===========================================================
                                    // FENCE — Candidate 1: does PFIFO only trust a CHAN_VALID that
                                    // ORIGINATED IN THE FALCON?
                                    //
                                    // ⚠ UNFLOWN. This leg has never run on hardware. It cannot be
                                    // exercised in QEMU either: QEMU HAS NO KEPLER, so a green
                                    // `test-x86` proves only that the code took a path that never
                                    // touched a GPU — which is a worse outcome than a hang, because
                                    // it looks like evidence. The only claim this code makes today
                                    // is the compile-time one: the image is what the listing says.
                                    //
                                    // PLACEMENT: after every ECHO observable has been harvested
                                    // (above) and BEFORE the 0x409504 recon block (below). The
                                    // first access to that offset wedges every subsequent read in
                                    // the unit for the rest of the boot (spec §5.4), so a FENCE
                                    // verdict collected after it would be void.
                                    //
                                    // It leaves PFIFO_CHAN[1] holding VALID|POLL_ENABLE, which is
                                    // what the context-bind leg below rewrites anyway.
                                    // ===========================================================
                                    {
                                        use ucode::UCODE_CTX_FENCE_A;
                                        const MB_SEED: u32 = 0xA5A5_0000;
                                        const IMEM_PAGE_WORDS: usize = 0x40;
                                        // IMEMC control bits, named. Bit 24 auto-increments on
                                        // WRITES, bit 25 on READS; setting 24 and then reading back
                                        // through the data port re-reads the same word forever, which
                                        // makes a readback "verify" that cannot fail (spec §4.1).
                                        const IMEMC_AINCW: u32 = 1 << 24;
                                        const IMEMC_AINCR: u32 = 1 << 25;
                                        const TLB_QUERY_PAGE0: u32 = 0x0200_0000;
                                        const TLB_PAGE0_USABLE: u32 = 0x0100_0000;
                                        const CPUCTL_STOPPED: u32 = 0x10;
                                        const CPUCTL_START: u32 = 0x02;
                                        let fb = 0x409000usize;

                                        serial_println!(":: kepler: FENCE begin — falcon-asserted CHAN_VALID, engine_status_port={:05X} (DERIVED, untested) UNFLOWN ::",
                                            ucode::IO_ENGINE_STATUS);

                                        // --- halt, and PROVE the halt, before touching IMEM -------
                                        fecs_write(bar0, fb + 0x100, CPUCTL_STOPPED);
                                        let halt_rb = fecs_read(bar0, fb + 0x100);
                                        let halted = (halt_rb & CPUCTL_STOPPED) != 0;
                                        serial_println!(":: kepler: FENCE halt cpuctl={:08X} halted={} ::",
                                            halt_rb, if halted { "Y" } else { "N" });

                                        if !halted {
                                            serial_println!(":: kepler: FENCE ABORT halt-refused — IMEM NOT rewritten under a live core ::");
                                        } else {
                                            // --- seed ALL FOUR observables, so "unchanged" has one
                                            // meaning for every one of them (spec §4 step 1) -------
                                            fecs_write(bar0, fb + 0x040, MB_SEED);   // MAILBOX0
                                            fecs_write(bar0, fb + 0x044, MB_SEED);   // MAILBOX1 (phase)
                                            fecs_write(bar0, fb + 0x800, 0);         // CC_SCRATCH[0] (cmd)
                                            fecs_write(bar0, fb + 0x804, MB_SEED);   // CC_SCRATCH[1] (ack)
                                            serial_println!(":: kepler: FENCE seed mb0={:08X} mb1={:08X} cc0={:08X} cc1={:08X} ::",
                                                fecs_read(bar0, fb + 0x040), fecs_read(bar0, fb + 0x044),
                                                fecs_read(bar0, fb + 0x800), fecs_read(bar0, fb + 0x804));

                                            // --- upload: IMEMC(base|AINCW), IMEMT tag, stream to
                                            // IMEMD, pad the full page ------------------------------
                                            let img = &UCODE_CTX_FENCE_A[..];
                                            fecs_write(bar0, fb + 0x180, IMEMC_AINCW); // offset 0 + AINCW
                                            fecs_write(bar0, fb + 0x188, 0);           // IMEMT tag=0 == BOOTVEC
                                            for &w in img.iter() {
                                                fecs_write(bar0, fb + 0x184, w);       // IMEMD — the DATA port
                                            }
                                            // The code TLB marks a page usable only once the LAST word
                                            // of the page has been written. Padding is not tidiness.
                                            for _ in img.len()..IMEM_PAGE_WORDS {
                                                fecs_write(bar0, fb + 0x184, 0);
                                            }
                                            let imemc_after = fecs_read(bar0, fb + 0x180);
                                            serial_println!(":: kepler: FENCE uploaded words={} padded-to={} imemc={:08X} ::",
                                                img.len(), IMEM_PAGE_WORDS, imemc_after);

                                            // --- TLB attestation ----------------------------------
                                            fecs_write(bar0, fb + 0x140, TLB_QUERY_PAGE0);
                                            let tlb = fecs_read(bar0, fb + 0x144);
                                            serial_println!(":: kepler: FENCE tlb page0={:08X} usable={} ::",
                                                tlb, if tlb == TLB_PAGE0_USABLE { "Y" } else { "N" });

                                            // --- readback verify through AINCR --------------------
                                            // Catches an upload aimed at the wrong register: had the
                                            // words gone to IMEMC instead of IMEMD, IMEM would still
                                            // hold its previous contents and the compare would miss.
                                            //
                                            // ⚠ It does NOT necessarily miss at word 0. FENCE words
                                            // 0..2 are byte-identical to ECHO's s37 prologue (both
                                            // images open by loading the same two CC_SCRATCH ports),
                                            // so against a stale ECHO image in IMEM the first
                                            // divergence is word 3. Do not read "idx=0" as the
                                            // signature of a failed upload, and do not read a clean
                                            // word 0 as evidence the upload took.
                                            fecs_write(bar0, fb + 0x180, IMEMC_AINCR);
                                            let mut verify_ok = true;
                                            let mut bad_at = 0usize;
                                            let mut bad_got = 0u32;
                                            let mut bad_want = 0u32;
                                            for k in 0..img.len() {
                                                let got = fecs_read(bar0, fb + 0x184);
                                                if got != img[k] && verify_ok {
                                                    verify_ok = false;
                                                    bad_at = k;
                                                    bad_got = got;
                                                    bad_want = img[k];
                                                }
                                            }
                                            if verify_ok {
                                                serial_println!(":: kepler: FENCE verify ok=Y words={} — every word matched ::", img.len());
                                            } else {
                                                serial_println!(":: kepler: FENCE verify ok=N words={} first-bad idx={} got={:08X} want={:08X} ::",
                                                    img.len(), bad_at, bad_got, bad_want);
                                            }

                                            let dmactl_pre = fecs_read(bar0, fb + 0x10C);
                                            fecs_write(bar0, fb + 0x10C, dmactl_pre & !1);
                                            let dmactl_post = fecs_read(bar0, fb + 0x10C);
                                            serial_println!(":: kepler: FENCE dmactl pre={:08X} post={:08X} ::",
                                                dmactl_pre, dmactl_post);

                                            if !verify_ok {
                                                serial_println!(":: kepler: FENCE ABORT verify-mismatch — BOOTVEC/CPUCTL NOT written ::");
                                                fecs_write(bar0, fb + 0x100, CPUCTL_STOPPED);
                                            } else if (dmactl_post & 1) != 0 {
                                                serial_println!(":: kepler: FENCE ABORT dmactl REFUSED — core would not run without a bound context ::");
                                                fecs_write(bar0, fb + 0x100, CPUCTL_STOPPED);
                                            } else {
                                                fecs_write(bar0, fb + 0x104, 0);            // BOOTVEC = 0
                                                fecs_write(bar0, fb + 0x100, CPUCTL_START); // START_TRIGGER
                                                serial_println!(":: kepler: FENCE start bootvec=0 cpuctl<={:08X} ::", CPUCTL_START);

                                                // --- gate 1: did it start? -----------------------
                                                //
                                                // Honest reachable set HERE is {progress, foreign,
                                                // timeout}: the loop breaks on the first non-seed
                                                // read and the pre-loop stamp lands within
                                                // microseconds, so neither bound marker can be the
                                                // first thing observed. The bound arms are kept
                                                // because they cost nothing and because gate3 — after
                                                // the CLEAR command, where a bound exit IS the
                                                // expected failure — shares this vocabulary.
                                                //
                                                // ⚠ The bound markers are tested for EQUALITY and
                                                // FIRST regardless. They are sign-extended words —
                                                // PHASE_A_BOUND is 0xFFFFFFBD — so any `> 0` or
                                                // `>= 3` test reached first would swallow them and
                                                // report forward progress on a program that gave up.
                                                //
                                                // Phase membership is a SET, not a range. FENCE
                                                // writes only {1,3,4,5}; `mb1 == 2` is reachable only
                                                // from a foreign image (ECHO/POKE stamp
                                                // PHASE_A_POSTREAD there) — which is precisely the
                                                // stale-IMEM case FOREIGN-PHASE exists to name, and a
                                                // `1..=5` range would have called it progress.
                                                let is_fence_phase = |v: u32| {
                                                    v == ucode::PHASE_FENCE_PRELOOP as u32
                                                        || v == ucode::PHASE_FENCE_PREASSERT as u32
                                                        || v == ucode::PHASE_FENCE_ASSERTED as u32
                                                        || v == ucode::PHASE_FENCE_DONE as u32
                                                };
                                                let mut mb1 = 0u32;
                                                let mut gate1 = "timeout";
                                                for _ in 0..ucode::HOST_ACK_ITERS {
                                                    mb1 = fecs_read(bar0, fb + 0x044);
                                                    if mb1 == ucode::PHASE_A_BOUND { gate1 = "bound1"; break; }
                                                    if mb1 == ucode::PHASE_FENCE_BOUND2 { gate1 = "bound2"; break; }
                                                    if mb1 == MB_SEED || mb1 == 0 { core::hint::spin_loop(); continue; }
                                                    if is_fence_phase(mb1) { gate1 = "progress"; break; }
                                                    gate1 = "foreign";
                                                    break;
                                                }
                                                match gate1 {
                                                    "progress" => serial_println!(":: kepler: FENCE gate1 PROGRESS mb1={:08X} — the falcon is running our phases ::", mb1),
                                                    "bound1" => serial_println!(":: kepler: FENCE gate1 EXIT-BY-BOUND mb1={:08X} — poll1 exhausted {} iterations, the ASSERT command was never observed ::", mb1, ucode::ECHO_BOUND),
                                                    "bound2" => serial_println!(":: kepler: FENCE gate1 POLL2-GIVEUP mb1={:08X} — asserted, then never observed the CLEAR command ::", mb1),
                                                    "foreign" => serial_println!(":: kepler: FENCE gate1 FOREIGN-PHASE mb1={:08X} — not a phase this image writes ::", mb1),
                                                    _ => serial_println!(":: kepler: FENCE gate1 HOST-TIMEOUT mb1={:08X} after {} host reads — mb1 never left its seed ::", mb1, ucode::HOST_ACK_ITERS),
                                                }

                                                // --- gate 2: WHICH image is running? -------------
                                                let mb0 = fecs_read(bar0, fb + 0x040);
                                                let who = if mb0 == ucode::FENCE_MAGIC {
                                                    "FENCE,uploaded-and-running"
                                                } else if mb0 == MB_SEED {
                                                    "SEED,nothing-ran"
                                                } else {
                                                    "OTHER,stale-IMEM-or-foreign-writer"
                                                };
                                                serial_println!(":: kepler: FENCE magic mb0={:08X} want={:08X} ({}) ::",
                                                    mb0, ucode::FENCE_MAGIC, who);

                                                if mb0 != ucode::FENCE_MAGIC {
                                                    serial_println!(":: kepler: FENCE ABORT wrong-image — no runlist submitted, no verdict claimed ::");
                                                    fecs_write(bar0, fb + 0x100, CPUCTL_STOPPED);
                                                } else {
                                                    // --- the assert -----------------------------
                                                    fecs_write(bar0, fb + 0x800, ucode::CMD_FENCE_ASSERT);
                                                    let mut ack = 0u32;
                                                    let mut ack_iters = 0u32;
                                                    for i in 0..ucode::HOST_ACK_ITERS {
                                                        ack = fecs_read(bar0, fb + 0x804);
                                                        ack_iters = i;
                                                        if ack != MB_SEED { break; }
                                                        core::hint::spin_loop();
                                                    }
                                                    // ack lives on CC_SCRATCH[1] and mb0 now carries
                                                    // the ENGINE_STATUS READBACK. Two registers, two
                                                    // facts: "the falcon reached the assert" and "the
                                                    // bit took" are separately observable. On one
                                                    // register they would be one indistinguishable
                                                    // event.
                                                    let es_falcon = fecs_read(bar0, fb + 0x040);
                                                    let phase = fecs_read(bar0, fb + 0x044);
                                                    let es_host = fecs_read(bar0, fb + 0xC00);
                                                    // THE TREATMENT. These two are not decoration —
                                                    // they gate the verdict below. `acked` = the
                                                    // falcon reached do_assert at all; `took` = the
                                                    // bit is actually set in ENGINE_STATUS.
                                                    let acked = ack != MB_SEED;
                                                    let took = (es_falcon & ucode::CHAN_VALID) != 0;
                                                    serial_println!(":: kepler: FENCE assert ack={:08X} iters={} phase={:08X} eng-status-per-falcon={:08X} eng-status-per-host={:08X} ::",
                                                        ack, ack_iters, phase, es_falcon, es_host);
                                                    serial_println!(":: kepler: FENCE assert acked={} took={} class={} ::",
                                                        if acked { "Y" } else { "N" },
                                                        if took { "Y" } else { "N" },
                                                        classify_fecs_word(es_falcon));

                                                    // --- WHERE DID THE WRITE LAND? -----------------
                                                    // `acked=Y took=N` is the ambiguous reading this
                                                    // arc shipped with: either the derived port
                                                    // 0x30000 is not ENGINE_STATUS and the write
                                                    // landed somewhere else, or the port is right and
                                                    // ENGINE_STATUS ignores a falcon-side write. The
                                                    // two license opposite next moves.
                                                    //
                                                    // A read-only sweep of the FECS host window
                                                    // separates them by finding CHAN_VALID's value
                                                    // where it actually is. Every offset here is
                                                    // proven-live (spec §2) and 0x504 is EXCLUDED —
                                                    // reading it wedges the unit for the boot (§5.4)
                                                    // and would void everything after this point.
                                                    if acked && !took {
                                                        serial_println!(":: kepler: FENCE stray-sweep begin — hunting CHAN_VALID={:08X} across the FECS window (0x504 excluded) ::",
                                                            ucode::CHAN_VALID);
                                                        let mut hits = 0u32;
                                                        for off in (0..=0x1FCusize).step_by(4) {
                                                            let v = fecs_read(bar0, fb + off);
                                                            if v == ucode::CHAN_VALID {
                                                                hits += 1;
                                                                serial_println!(":: kepler: FENCE stray-sweep HIT off={:03X} val={:08X} ::", off, v);
                                                            }
                                                        }
                                                        for &off in &[0x800usize, 0x804, 0xB00, 0xB04, 0xC00, 0xC08] {
                                                            let v = fecs_read(bar0, fb + off);
                                                            if v == ucode::CHAN_VALID {
                                                                hits += 1;
                                                                serial_println!(":: kepler: FENCE stray-sweep HIT off={:03X} val={:08X} ::", off, v);
                                                            }
                                                        }
                                                        // MAILBOX0/1 legitimately hold falcon-written
                                                        // values, so a hit there is not a stray; the
                                                        // interesting hits are anywhere else.
                                                        if hits == 0 {
                                                            serial_println!(":: kepler: FENCE stray-sweep NONE — CHAN_VALID is nowhere in the window; the write did not land in a register we can see, so a WRONG PORT is not ruled out ::");
                                                        } else {
                                                            serial_println!(":: kepler: FENCE stray-sweep hits={} — compare against MAILBOX0(040)/MAILBOX1(044); a hit at any OTHER offset names where the derived port actually points ::", hits);
                                                        }
                                                    }

                                                    // --- re-validate the channel, read the verdict
                                                    // Rebuild the channel exactly as the canonical
                                                    // instance-block setup did, high bit included,
                                                    // then read the channel-validate error at 0x252c.
                                                    // This is a VALIDATE, not a runlist submit; the
                                                    // submit (0x2270/0x2274) is downstream and is not
                                                    // reached or perturbed from here.
                                                    unsafe {
                                                        core::ptr::write_volatile((bar1 + inst_off + 0x0C) as *mut u32,
                                                            ((userd_off >> 32) as u32) | 0x80000000);
                                                    }
                                                    mmio_write(bar0, 0x800000 + (1 * 8), 0);
                                                    mmio_write(bar0, 0x800004 + (1 * 8), 0x00000400);
                                                    let chan_want = 0xC0000000 | ((inst_off as u32) >> 12);
                                                    mmio_write(bar0, 0x800000 + (1 * 8), chan_want);
                                                    let chan_rb = mmio_read(bar0, 0x800000 + (1 * 8));
                                                    let stuck = (chan_rb & 0xC0000000) == 0xC0000000;
                                                    let err = mmio_read(bar0, 0x252c);
                                                    let stat = mmio_read(bar0, 0x263c);
                                                    serial_println!(":: kepler: FENCE chan[1] want={:08X} readback={:08X} VALID-stuck={} ::",
                                                        chan_want, chan_rb, if stuck { "Y" } else { "N" });
                                                    serial_println!(":: kepler: FENCE verdict err={:08X} stat={:08X} ::", err, stat);

                                                    // ⛔ TWO preconditions gate this verdict, and the
                                                    // treatment gate comes FIRST.
                                                    //
                                                    // If the falcon never reached the assert, or
                                                    // reached it and the bit did not take, then no
                                                    // falcon-asserted CHAN_VALID ever existed and the
                                                    // experiment was never performed. `err` would then
                                                    // read 0x2 — the standing result for 28 sittings —
                                                    // and an ungated three-way would print CANDIDATE-1
                                                    // ELIMINATED. That is the EXPECTED answer, so it
                                                    // would be believed, on a boot where the treatment
                                                    // was never applied. The `err == 0` arm is worse
                                                    // still: a false CONFIRMED. Same class of defect
                                                    // that killed bounce 5, and the same argument the
                                                    // `stuck` gate below already makes.
                                                    if !acked || !took {
                                                        serial_println!(":: kepler: FENCE VOID (treatment not applied) acked={} took={} — no falcon-asserted CHAN_VALID existed when the channel was written; err={:08X} says NOTHING about candidate 1 and NO conclusion is drawn ::",
                                                            if acked { "Y" } else { "N" },
                                                            if took { "Y" } else { "N" },
                                                            err);
                                                    } else if !stuck {
                                                        serial_println!(":: kepler: FENCE VOID — VALID did not stick in PFIFO_CHAN[1]; err={:08X} is uninterpretable and NO conclusion is drawn ::", err);
                                                    } else if err == 0 {
                                                        serial_println!(":: kepler: FENCE CANDIDATE-1 CONFIRMED err=00000000 — PFIFO validated the channel with a FALCON-asserted CHAN_VALID ::");
                                                    } else if err == 2 {
                                                        serial_println!(":: kepler: FENCE CANDIDATE-1 ELIMINATED err=00000002 — a falcon-asserted CHAN_VALID does not satisfy PFIFO; the wall is elsewhere (engine binding at submit) ::");
                                                    } else {
                                                        serial_println!(":: kepler: FENCE UNEXPECTED err={:08X} ({}) — neither 0 nor 2; nothing eliminated ::",
                                                            err, class_zero(err, "VALUE,unnamed"));
                                                    }

                                                    // --- unwind: the falcon clears what it set ----
                                                    fecs_write(bar0, fb + 0x800, ucode::CMD_FENCE_CLEAR);
                                                    let mut mb1e = 0u32;
                                                    let mut gate3 = "timeout";
                                                    for _ in 0..ucode::HOST_ACK_ITERS {
                                                        mb1e = fecs_read(bar0, fb + 0x044);
                                                        if mb1e == ucode::PHASE_A_BOUND { gate3 = "bound1"; break; }
                                                        if mb1e == ucode::PHASE_FENCE_BOUND2 { gate3 = "bound2"; break; }
                                                        if mb1e == ucode::PHASE_FENCE_DONE as u32 { gate3 = "done"; break; }
                                                        core::hint::spin_loop();
                                                    }
                                                    match gate3 {
                                                        "done" => serial_println!(":: kepler: FENCE unwind DONE mb1={:08X} ::", mb1e),
                                                        "bound1" => serial_println!(":: kepler: FENCE unwind EXIT-BY-BOUND mb1={:08X} — never saw ASSERT ::", mb1e),
                                                        "bound2" => serial_println!(":: kepler: FENCE unwind POLL2-GIVEUP mb1={:08X} — the CLEAR command was never observed ::", mb1e),
                                                        _ => serial_println!(":: kepler: FENCE unwind HOST-TIMEOUT mb1={:08X} after {} host reads ::", mb1e, ucode::HOST_ACK_ITERS),
                                                    }
                                                    let es_final = fecs_read(bar0, fb + 0xC00);
                                                    serial_println!(":: kepler: FENCE unwind eng-status={:08X} cleared={} ::",
                                                        es_final, if es_final == 0 { "Y" } else { "N" });

                                                    // Park the core halted so the recon block below,
                                                    // and the next boot's rest values, read a quiet
                                                    // unit rather than a running one.
                                                    fecs_write(bar0, fb + 0x100, CPUCTL_STOPPED);
                                                    serial_println!(":: kepler: FENCE end cpuctl={:08X} ::", fecs_read(bar0, fb + 0x100));
                                                }
                                            }
                                        }
                                    }

                                    // --- Pull 28 recon, relocated (GR5, s31 fold): the first access to an
                                    // absent 0x409xxx offset latches a sticky PRI fault and every later read
                                    // of the unit returns BADF1000 (s31: fal-base read real, then all
                                    // post-0x409504 reads poisoned, s30 markers included). Run the recon LAST,
                                    // after every proven read, and bracket it with cpuctl control reads so
                                    // poisoning is observed in-boot rather than inferred.
                                    serial_println!(":: kepler: recon-pre cpuctl={:08X} ::", fecs_read(bar0, 0x409000 + 0x100));
                                    
                                    // Pull 31: Context-Bind Experiment
                                    let ch_id = (inst_off as u32) >> 12;
                                    serial_println!(":: kepler: bind-pre CHAN_CUR={:08X} CHAN_NEXT={:08X} ENGINE_STATUS={:08X} ::",
                                        fecs_read(bar0, 0x409B00), fecs_read(bar0, 0x409B04), fecs_read(bar0, 0x409C00));
                                    phase!("recon_and_witnesses");
                                    
                                    // H4: DMACTL REQUIRE_CTX interacting with CHAN_CUR
                                    let dmactl_bind_pre = fecs_read(bar0, 0x40910C);
                                    fecs_write(bar0, 0x40910C, dmactl_bind_pre | 1);
                                    serial_println!(":: kepler: H4 DMACTL REQUIRE_CTX set pre={:08X} post={:08X} ::", dmactl_bind_pre, fecs_read(bar0, 0x40910C));

                                    // Write CHAN_CUR and verify
                                    fecs_write(bar0, 0x409B00, ch_id);
                                    let c_cur = fecs_read(bar0, 0x409B00);
                                    if (c_cur >> 16) == 0xBADF {
                                        serial_println!(":: kepler: bind CHAN_CUR FAULT={:08X} (skip rest) ::", c_cur);
                                    } else {
                                        serial_println!(":: kepler: bind CHAN_CUR={:08X} ::", c_cur);
                                        
                                        // Write CHAN_NEXT and verify
                                        fecs_write(bar0, 0x409B04, ch_id);
                                        let c_next = fecs_read(bar0, 0x409B04);
                                        if (c_next >> 16) == 0xBADF {
                                            serial_println!(":: kepler: bind CHAN_NEXT FAULT={:08X} (skip rest) ::", c_next);
                                        } else {
                                            serial_println!(":: kepler: bind CHAN_NEXT={:08X} ::", c_next);
                                            
                                            serial_println!(":: kepler: bind-post ENGINE_STATUS={:08X} ::", fecs_read(bar0, 0x409C00));
                                            
                                            // Explicit post-bind witness leg (PFIFO_CHAN[1] Register)
                                            let pre_rw = mmio_read(bar0, 0x800000 + (1 * 8));
                                            serial_println!(":: kepler: witness pre-rewrite PFIFO_CHAN[1]={:08X} ::", pre_rw);
                                            
                                            let witness_val = 0xC0000000 | ((inst_off as u32) >> 12);
                                            mmio_write(bar0, 0x800000 + (1 * 8), witness_val);
                                            
                                            let witness_post = mmio_read(bar0, 0x800000 + (1 * 8));
                                            serial_println!(":: kepler: witness post-bind PFIFO_CHAN[1]={:08X} ::", witness_post);
                                        }
                                    }
                                    phase!("ctx_bind");
                                    
                                    serial_println!(":: kepler: recon-post cpuctl={:08X} ::", fecs_read(bar0, 0x409000 + 0x100));


                                    // 3. Submit Runlist
                                    //
                                    // Rebuild the page first. The mirror-window beacon probe planted
                                    // 0xBEAC0001..8 over `runlist_off + 0..31` — all six words of all
                                    // three entries, plus two words past them — and its consumers (the
                                    // pass-1 scan and the pass-2 volatility re-read) are long done by
                                    // here. Without this the chip is handed a three-entry playlist
                                    // whose entries are beacon words, which is what every capture from
                                    // the pull-16 beacon landing onward actually submitted.
                                    write_runlist();

                                    // Read the page back over the FULL plant width and count what is
                                    // wrong. Scanning only the six authored words would report CLEAN
                                    // over the two beacon words the scan never looked at — the same
                                    // narrower-than-the-plant mistake the restore itself made.
                                    // `words=` is printed so the coverage claim is checkable in the
                                    // capture rather than trusted from the source, and `w=[…]` prints
                                    // what was READ BACK, not what was intended — so a `mismatch`
                                    // names the offending word instead of merely counting it.
                                    //
                                    // `restored=CLEAN` asserts exactly one thing: the eight words the
                                    // plant destroyed are back to what this driver put there. It says
                                    // NOTHING about whether those contents are the right contents for
                                    // the chip — the three entries still use three mutually
                                    // inconsistent encodings, a separate question this leg does not
                                    // answer and must not be read as answering.
                                    //
                                    // The read-back is honest about the CPU side: BAR1 is mapped
                                    // PCD|PWT with the PTE PAT bit clear (`arch::memory::map_mmio_window`),
                                    // selecting PAT entry 3, left at the power-on UC. UC reads are
                                    // never served from a cache line or a write-combining buffer, so a
                                    // failed write really is observed. It does NOT prove the GPU's own
                                    // fetch path sees the same bytes.
                                    let mut rl_beacon_resid = 0u32;
                                    let mut rl_mismatch = 0u32;
                                    let mut rl_read = [0u32; BEACON_PLANT_WORDS];
                                    for (i, got) in rl_read.iter_mut().enumerate() {
                                        let want = runlist_words.get(i).copied().unwrap_or(0);
                                        *got = unsafe {
                                            core::ptr::read_volatile((bar1 + runlist_off + i * 4) as *const u32)
                                        };
                                        if (0xBEAC0001..=0xBEAC0008).contains(got) {
                                            rl_beacon_resid += 1;
                                        }
                                        if *got != want {
                                            rl_mismatch += 1;
                                        }
                                    }
                                    serial_println!(
                                        ":: kepler: runlist-rebuild off={:08X} w=[{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] words={} entries={} beacon_resid={} mismatch={} restored={} ::",
                                        runlist_off,
                                        rl_read[0], rl_read[1], rl_read[2], rl_read[3],
                                        rl_read[4], rl_read[5], rl_read[6], rl_read[7],
                                        rl_read.len(), RUNLIST_LEN,
                                        rl_beacon_resid, rl_mismatch,
                                        if rl_beacon_resid == 0 && rl_mismatch == 0 { "CLEAN" } else { "CORRUPT" }
                                    );

                                    let want_base = (runlist_off as u32) >> 12;
                                    mmio_write(bar0, 0x2270, want_base); // target=0 (VRAM), addr
                                    mmio_write(bar0, 0x2274, RUNLIST_LEN); // ENG=0 | LEN
                                    serial_println!("[NVIDIA] Configured Runlist and bound channel.");

                                    // Wait for PLAYLIST_RD/_RD_LEN to echo the submit.
                                    //
                                    // What this tests, and on whose authority — `docs/dev/OS/08_VIDEO/gpu_spec.md`
                                    // §2.4.1, from eight metal captures across four code revisions:
                                    //   * finding 2 — PLAYLIST_RD (0x2280) holds our runlist page,
                                    //     `runlist_off >> 12`;
                                    //   * finding 1 — PLAYLIST_RD_LEN bits [11:0] are a faithful echo of
                                    //     the length we wrote (LEN=1 → …001, LEN=3 → …003, no exceptions).
                                    // So the predicate is: base echoes, and the entry count echoes
                                    // `RUNLIST_LEN`. It reads the count from the same constant the submit
                                    // used, so the two cannot drift apart again.
                                    //
                                    // Bit 20 is deliberately NOT in the predicate. §2.4.1 leaves H-ID(weak),
                                    // H-BUSY and H-STICKY all standing and §2.4.2's sibling sweep has not
                                    // yet separated them; under H-BUSY the correct wait would be for bit 20
                                    // to CLEAR, the exact opposite of waiting for it to appear. Polling on
                                    // an unresolved bit would be asserting a semantics we have not proven.
                                    //
                                    // Bounded by wall clock, not by iteration count. Every capture on record
                                    // shows the echo already present on the first read, so 10 ms is generous
                                    // by orders of magnitude; the old 100_000-iteration bound with an
                                    // unsatisfiable predicate burned 200_000 BAR0 reads across PCIe on every
                                    // boot and could not distinguish "accepted" from "never accepted".
                                    const PL_POLL_MS: u64 = 10;
                                    // Fallback divisor for the uncalibrated path. `HW_WAIT_BUDGET` is a
                                    // fixed CYCLE count with no defined wall time — its own definition
                                    // (`arch/x86_64/mod.rs`) puts it at 2.5e9 cycles ≈ [0.5 s, 2.5 s]
                                    // across 1–5 GHz parts, ~1.1 s at this board's 2.3 GHz Ivy Bridge
                                    // base — so NO millisecond can be derived from it. Take a fixed
                                    // 1/200 of it (12.5e6 cycles ≈ 2.5–12 ms over that same range) and
                                    // report the budget in cycles, never in a unit the number lacks.
                                    const PL_POLL_FALLBACK_DIV: u64 = 200;
                                    let pl_hz = poll_hz();
                                    let pl_budget = match pl_hz {
                                        Some(hz) => hz.saturating_mul(PL_POLL_MS) / 1000,
                                        None => crate::arch::HW_WAIT_BUDGET / PL_POLL_FALLBACK_DIV,
                                    };
                                    let pl_t0 = crate::arch::now_cycles();
                                    let mut pl_rd;
                                    let mut pl_rd_len;
                                    let mut pl_iters = 0u32;
                                    let pl_hit = loop {
                                        pl_rd = mmio_read(bar0, 0x2280);
                                        pl_rd_len = mmio_read(bar0, 0x2284);
                                        pl_iters += 1;
                                        if pl_rd == want_base && (pl_rd_len & 0xFFF) == RUNLIST_LEN {
                                            break true;
                                        }
                                        if crate::arch::now_cycles().wrapping_sub(pl_t0) >= pl_budget {
                                            break false;
                                        }
                                        core::hint::spin_loop();
                                    };
                                    let pl_cy = crate::arch::now_cycles().wrapping_sub(pl_t0);
                                    // `iters` counts register-pair READS, so its minimum is 1 and `iters=0`
                                    // is an impossible state — a broken instrument, not a quiet zero.
                                    // `iters=1 exit=hit` is the echo already present on the first read;
                                    // `exit=deadline` never means "not looked at".
                                    //
                                    // `waited` and `budget` are selected TOGETHER with the unit and the
                                    // clock label, and printed with the SAME unit, so the two figures are
                                    // always comparable and neither can carry a unit the clock cannot
                                    // support. `clk=guess` means the counter rate is unknown: both are
                                    // then raw cycles, because on that path no millisecond exists to
                                    // print — the earlier form printed `budget=10ms` next to a cycle
                                    // count, a wall-clock claim the fallback had not earned.
                                    let (pl_waited, pl_budget_shown, pl_unit, pl_clk) = match pl_hz {
                                        Some(hz) => (pl_cy.saturating_mul(1000) / hz, PL_POLL_MS, "ms", "tsc"),
                                        None => (pl_cy, pl_budget, "cy", "guess"),
                                    };
                                    serial_println!(
                                        ":: kepler: post-bind playlist_rd={:08X} playlist_rd_len={:08X} exit={} iters={} waited={}{} budget={}{} clk={} want_base={:08X} want_len={} got_len={} ::",
                                        pl_rd, pl_rd_len,
                                        if pl_hit { "hit" } else { "deadline" },
                                        pl_iters, pl_waited, pl_unit, pl_budget_shown, pl_unit, pl_clk,
                                        want_base, RUNLIST_LEN, pl_rd_len & 0xFFF
                                    );

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

                                    // ============ TERMINAL FALCON POKE — 0x409504, from inside ============
                                    // The falcon-side counterpart of the host poke below, and the last
                                    // thing in the kepler leg that runs the FECS core. It reads
                                    // 0x409504 with `iord I[$r8]` — the access the HOST side cannot
                                    // survive (spec §5.4) — and reports the word it got in
                                    // CC_SCRATCH[1]. If the unit wedges, everything it could have
                                    // wedged has already been printed.
                                    //
                                    // This is why the ECHO image no longer carries the read: ECHO runs
                                    // mid-sequence, so a poisoning read inside it invalidated every
                                    // FECS observation from the sweep onwards.
                                    {
                                        let pbase = 0x409000usize;
                                        let img = &ucode::UCODE_CTX_POKE_A[..];
                                        const IMEM_PAGE_WORDS: usize = 0x40;
                                        const MB_SEED: u32 = 0xA5A5_0000;

                                        // Halt the core if the echo leg left it running, and PROVE
                                        // the halt. This comment used to sit above the DMACTL clear
                                        // alone, which is not a halt: clearing REQUIRE_CTX permits a
                                        // core to run, it does not stop one. The echo/FENCE legs
                                        // before this one both start cores.
                                        fecs_write(bar0, pbase + 0x100, 0x10); // CPUCTL <- STOPPED
                                        let poke_halt = fecs_read(bar0, pbase + 0x100);
                                        serial_println!(":: kepler: ucode-poke halt cpuctl={:08X} halted={} ::",
                                            poke_halt, if (poke_halt & 0x10) != 0 { "Y" } else { "N" });

                                        let dmactl_pre = fecs_read(bar0, pbase + 0x10C);
                                        fecs_write(bar0, pbase + 0x10C, dmactl_pre & !1);

                                        fecs_write(bar0, pbase + 0x800, 0);        // CC_SCRATCH[0] cmd
                                        fecs_write(bar0, pbase + 0x804, MB_SEED);  // CC_SCRATCH[1] ack
                                        fecs_write(bar0, pbase + 0x040, MB_SEED);  // MAILBOX0 <- value read
                                        fecs_write(bar0, pbase + 0x044, MB_SEED);  // MAILBOX1 <- phase
                                        serial_println!(":: kepler: ucode-poke pre CC_SCRATCH[0]={:08X} CC_SCRATCH[1]={:08X} mb0={:08X} mb1={:08X} ::",
                                            fecs_read(bar0, pbase + 0x800), fecs_read(bar0, pbase + 0x804),
                                            fecs_read(bar0, pbase + 0x040), fecs_read(bar0, pbase + 0x044));

                                        // Upload. IMEMT tag 0 matches BOOTVEC=0, and the page is padded
                                        // to IMEM_PAGE_WORDS because the code TLB marks a page usable
                                        // only when the LAST word of the 0x40-word page is written.
                                        fecs_write(bar0, pbase + 0x180, 1 << 24);  // IMEMC offset=0, AINCW
                                        fecs_write(bar0, pbase + 0x188, 0);        // IMEMT tag=0
                                        for &word in img.iter() { fecs_write(bar0, pbase + 0x184, word); }
                                        for _ in img.len()..IMEM_PAGE_WORDS { fecs_write(bar0, pbase + 0x184, 0); }
                                        serial_println!(":: kepler: ucode-poke uploaded words={} padded={} ::", img.len(), IMEM_PAGE_WORDS);

                                        fecs_write(bar0, pbase + 0x180, 1 << 25);  // IMEMC offset=0, AINCR
                                        let mut verify_poke = true;
                                        for k in 0..img.len() {
                                            let rb = fecs_read(bar0, pbase + 0x184);
                                            if rb != img[k] {
                                                serial_println!(":: kepler: ucode-poke verify-mismatch word={} wr={:08X} rb={:08X} ::", k, img[k], rb);
                                                verify_poke = false;
                                            }
                                        }

                                        if !verify_poke {
                                            serial_println!(":: kepler: ucode-poke ABORT verify-mismatch — BOOTVEC/CPUCTL NOT written ::");
                                        } else {
                                            // ⚠ LEDGER. FECS_504_READ_TOUCHED is set from `fecs_read`,
                                            // which sees only HOST accesses; a falcon `iord` is invisible
                                            // to it. Set it here, BEFORE arming the core, or the ledger
                                            // reports 504_read_touched=false on exactly the boot where
                                            // the falcon touched 0x409504 first. An instrument's silence
                                            // is evidence only if the instrument can execute in the
                                            // state it reports on.
                                            FECS_504_READ_TOUCHED.store(true, Ordering::SeqCst);

                                            fecs_write(bar0, pbase + 0x104, 0); // BOOTVEC = 0
                                            fecs_write(bar0, pbase + 0x100, 2); // CPUCTL START_TRIGGER
                                            serial_println!(":: kepler: ucode-poke start img=POKE ::");

                                            // Heartbeat poll: wait for the falcon to reach the loop.
                                            // The poll is bounded to ~1000 host reads (~1 ms) so it does
                                            // not delay cmd=1 past ECHO_BOUND (1,048,576 falcon iters).
                                            let mut hb = 0;
                                            let mut hb_iters = 0u32;
                                            for _ in 0..1000 {
                                                hb_iters += 1;
                                                hb = fecs_read(bar0, pbase + 0x044); // MAILBOX1
                                                // Expect 0x02, accept 0x01. If 0, it's just mb0 bleeding over.
                                                if hb != MB_SEED && classify_fecs_word(hb) == "VALUE" { break; }
                                            }
                                            serial_println!(":: kepler: ucode-poke heartbeat hb={:08X} hb_iters={} ::", hb, hb_iters);

                                            fecs_write(bar0, pbase + 0x800, 1); // host-cmd: do the poke

                                            // Host-side bound, in HOST MMIO reads. ECHO_BOUND is a
                                            // falcon instruction budget (1,048,576); spending it here
                                            // would be ~1 s of boot in BAR0 reads.
                                            let mut ack = MB_SEED;
                                            let mut ack_iters = 0u32;
                                            for i in 0..ucode::HOST_ACK_ITERS {
                                                ack = fecs_read(bar0, pbase + 0x804);
                                                ack_iters = i;
                                                if ack != MB_SEED { break; }
                                                core::hint::spin_loop();
                                            }

                                            let mb0 = fecs_read(bar0, pbase + 0x040);
                                            let phase = fecs_read(bar0, pbase + 0x044);
                                            let scratch0 = fecs_read(bar0, pbase + 0x800);
                                            let cpuctl = fecs_read(bar0, pbase + 0x100);
                                            let gpccs = mmio_read(bar0, 0x41A100);

                                            let class = classify_fecs_word(ack);
                                            let get_verdict = |v: u32, exp: u32| -> &'static str {
                                                let c = classify_fecs_word(v);
                                                if c == "VALUE" { if v == exp { "alive" } else { "value-mismatch" } }
                                                else if c == "POISON" { "severed" }
                                                else { c }
                                            };

                                            let scratch0_verdict = get_verdict(scratch0, 0x00000001);
                                            let gpccs_verdict = get_verdict(gpccs, 0x00000010);
                                            let cpuctl_verdict = {
                                                let c = classify_fecs_word(cpuctl);
                                                if c == "VALUE" {
                                                    if (cpuctl & 0x10) != 0 { "alive" } else { "value-mismatch" }
                                                } else if c == "POISON" {
                                                    "severed"
                                                } else {
                                                    c
                                                }
                                            };

                                            serial_println!(":: kepler: ctx-poke img=POKE ack={:08X} mb0={:08X} phase={:08X} scratch0={:08X}({}) cpuctl={:08X}({}) gpccs={:08X}({}) iters={} class={} ::",
                                                ack, mb0, phase, scratch0, scratch0_verdict, cpuctl, cpuctl_verdict, gpccs, gpccs_verdict, ack_iters, class);

                                            if phase == ucode::PHASE_A_BOUND {
                                                serial_println!(":: kepler: ucode-poke EXIT-BY-BOUND img=POKE bound={} — command never observed, 0x409504 NOT read ::",
                                                    ucode::ECHO_BOUND);
                                            } else if ack == MB_SEED {
                                                serial_println!(":: kepler: ucode-poke FAILURE img=POKE iters={} phase={:08X} — no ack ::",
                                                    ack_iters, phase);
                                            } else if class == "VALUE" {
                                                // The falcon read 0x409504 and got a word that is not a
                                                // fault response, not a float and not zero.
                                                serial_println!(":: kepler: ucode-poke SUCCESS img=POKE wrcmd_cmd={:08X} ::", ack);
                                            } else {
                                                serial_println!(":: kepler: ucode-poke {} img=POKE wrcmd_cmd={:08X} ::", class, ack);
                                            }
                                        }
                                    }

                                    // ================= TERMINAL POKE — MUST BE LAST =================
                                    // ⛔ ORDERING CONTRACT. This is the LAST kepler statement of the
                                    // boot. Nothing below it, and nothing later in `init()`, may touch
                                    // the FECS unit — no read, no write, no sweep. Anything added to
                                    // the kepler leg sequence goes ABOVE this block, never below it.
                                    //
                                    // 0x409504 (WRCMD_CMD) is the poison offset: the first ACCESS to it
                                    // faults and wedges every subsequent read in the FECS unit for the
                                    // rest of the boot (s31 discovered, s32 confirmed with its own
                                    // control frame, s34 convicted by elimination — falcon_microcode_
                                    // spec.md §5.4). Pull 28 turned that into a standing ban on
                                    // unproven writes into this unit. Peter lifted the ban on
                                    // 2026-07-26 for EXACTLY this one write; the exemption does not
                                    // generalise.
                                    //
                                    // Value 0 is the least-assumptive available: it asserts no command
                                    // encoding, no bit layout, no field. The question is only whether
                                    // the offset accepts a WRITE at all, given that every read of it
                                    // faults and that nouveau drives this register on gk104 (§5.4).
                                    //
                                    // NO READBACK. A readback would be a read of the poisoning offset —
                                    // the exact access s31 convicted. The witness is therefore printed
                                    // BEFORE the write, so the capture proves the ordering: the line
                                    // appears, then the write happens, and whether the boot survives
                                    // past it is itself the observation. Nothing more is claimed.
                                    
                                    {
                                        let count = FECS_ACCESS_COUNT.load(Ordering::SeqCst);
                                        let first = FECS_FIRST_OFFSET.load(Ordering::SeqCst);
                                        let mut r_idx_str = alloc::string::String::from("none");
                                        let r_idx = FECS_504_READ_INDEX.load(Ordering::SeqCst);
                                        if r_idx != 0xFFFFFFFF { r_idx_str = alloc::format!("{}", r_idx); }
                                        let mut w_idx_str = alloc::string::String::from("none");
                                        let w_idx = FECS_504_WRITE_INDEX.load(Ordering::SeqCst);
                                        if w_idx != 0xFFFFFFFF { w_idx_str = alloc::format!("{}", w_idx); }
                                        serial_println!(":: kepler: fecs-ledger accesses={} first_offset={:08X} 504_read_touched={} 504_read_idx={} 504_write_touched={} 504_write_idx={} ::",
                                            count, first, FECS_504_READ_TOUCHED.load(Ordering::SeqCst), r_idx_str,
                                            FECS_504_WRITE_TOUCHED.load(Ordering::SeqCst), w_idx_str);
                                    }
serial_println!(":: kepler: terminal-poke 0x409504 wr=0 (post: no further FECS reads this boot) ::");
                                    fecs_write(bar0, 0x409504, 0);
                                    // ============ NOTHING BELOW THIS LINE MAY TOUCH FECS ============
                                }
                            }
                        }
                    }
                }
            }
        }
        // SEAT BUILD FIX (dda6a16c broke every nvidia-kepler build): `phase!` is declared inside
        // this `unsafe` block and captures its local `t_last`, but the call sat after the block's
        // close at fn scope — `cannot find macro 'phase' in this scope`. Moved inside; the
        // fecs-ledger print below it runs after and costs microseconds, so the phase delta is
        // unchanged in substance.
        phase!("scanout_handover");
    }

                                    {
                                        let count = FECS_ACCESS_COUNT.load(Ordering::SeqCst);
                                        let first = FECS_FIRST_OFFSET.load(Ordering::SeqCst);
                                        let mut r_idx_str = alloc::string::String::from("none");
                                        let r_idx = FECS_504_READ_INDEX.load(Ordering::SeqCst);
                                        if r_idx != 0xFFFFFFFF { r_idx_str = alloc::format!("{}", r_idx); }
                                        let mut w_idx_str = alloc::string::String::from("none");
                                        let w_idx = FECS_504_WRITE_INDEX.load(Ordering::SeqCst);
                                        if w_idx != 0xFFFFFFFF { w_idx_str = alloc::format!("{}", w_idx); }
                                        serial_println!(":: kepler: fecs-ledger accesses={} first_offset={:08X} 504_read_touched={} 504_read_idx={} 504_write_touched={} 504_write_idx={} ::",
                                            count, first, FECS_504_READ_TOUCHED.load(Ordering::SeqCst), r_idx_str,
                                            FECS_504_WRITE_TOUCHED.load(Ordering::SeqCst), w_idx_str);
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

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

pub static FECS_ACCESS_COUNT: AtomicU32 = AtomicU32::new(0);
pub static FECS_FIRST_OFFSET: AtomicU32 = AtomicU32::new(0xFFFFFFFF);
pub static FECS_504_READ_TOUCHED: AtomicBool = AtomicBool::new(false);
pub static FECS_504_READ_INDEX: AtomicU32 = AtomicU32::new(0xFFFFFFFF);
pub static FECS_504_WRITE_TOUCHED: AtomicBool = AtomicBool::new(false);
pub static FECS_504_WRITE_INDEX: AtomicU32 = AtomicU32::new(0xFFFFFFFF);

/// Classify a 32-bit word read back out of the FECS unit.
///
/// This file has always known that not every non-sentinel word is a result;
/// what it did not have was one place that says so. The three non-results:
///
///   * `BADF….` / `BAD0….` — the chip's own fault response. `BADF1000` is what
///     a healthy-but-idle unit returns and `BADF1200` an unpowered one (s31);
///     `BAD0BA20` is the generic form. A poison read must be reported as
///     POISON, never as a value.
///   * `FFFFFFFF` — nothing claimed the cycle; the bus floated high.
///   * `00000000` — carries no information. Every "absent?" test in this file
///     already lumps `0` in with the two above.
///
/// A verdict that carves out only `BADF….` prints SUCCESS for the other three.
pub fn classify_fecs_word(x: u32) -> &'static str {
    if (x >> 16) == 0xBADF || (x >> 16) == 0xBAD0 {
        "POISON"
    } else if x == 0xFFFFFFFF {
        "ABSENT"
    } else if x == 0 {
        "ZERO"
    } else {
        "VALUE"
    }
}

pub fn fecs_read(bar0: usize, offset: usize) -> u32 {
    let count = FECS_ACCESS_COUNT.fetch_add(1, Ordering::SeqCst);
    let _ = FECS_FIRST_OFFSET.compare_exchange(
        0xFFFFFFFF, offset as u32, Ordering::SeqCst, Ordering::SeqCst
    );
    if offset == 0x409504 {
        FECS_504_READ_TOUCHED.store(true, Ordering::SeqCst);
        let _ = FECS_504_READ_INDEX.compare_exchange(
            0xFFFFFFFF, count, Ordering::SeqCst, Ordering::SeqCst
        );
    }
    unsafe { core::ptr::read_volatile((bar0 + offset) as *const u32) }
}

pub fn fecs_write(bar0: usize, offset: usize, val: u32) {
    let count = FECS_ACCESS_COUNT.fetch_add(1, Ordering::SeqCst);
    let _ = FECS_FIRST_OFFSET.compare_exchange(
        0xFFFFFFFF, offset as u32, Ordering::SeqCst, Ordering::SeqCst
    );
    if offset == 0x409504 {
        FECS_504_WRITE_TOUCHED.store(true, Ordering::SeqCst);
        let _ = FECS_504_WRITE_INDEX.compare_exchange(
            0xFFFFFFFF, count, Ordering::SeqCst, Ordering::SeqCst
        );
    }
    unsafe { core::ptr::write_volatile((bar0 + offset) as *mut u32, val) }
}

pub unsafe fn mmio_read(base: usize, offset: usize) -> u32 {
    core::ptr::read_volatile((base + offset) as *const u32)
}

pub unsafe fn mmio_write(base: usize, offset: usize, val: u32) {
    core::ptr::write_volatile((base + offset) as *mut u32, val)
}

// NOTE — there is no `#[cfg(test)] mod tests` here any more, deliberately.
//
// The module that used to sit at the bottom of this file did not compile: it
// called `pack92` (only `pack128` exists), asserted against a 92-byte buffer,
// and pinned instruction offsets that had moved. It survived in that state
// because nothing runs `cargo test` on this `no_std` kernel crate — `./arroyo
// check` is the gate, and `#[cfg(test)]` code is invisible to it.
//
// The coverage did not move to nowhere: the `const _: () = { … }` blocks in
// `mod ucode` pin all 128 bytes of BOTH images, contiguously, padding
// included — strictly more than the old tests sampled — and const evaluation
// IS performed by the gate. A test that cannot run is a comment that lies
// about being a test, so it is gone rather than repaired.
