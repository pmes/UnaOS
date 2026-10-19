// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! UVC — the USB Video Class census and the PROBE negotiation (`UNAOS_UVC=1`, default OFF).
//!
//! # WHAT THIS RUNG IS, AND WHAT IT DELIBERATELY IS NOT
//!
//! The 2012 rMBP's built-in FaceTime HD camera (`05ac:8510`) is on the EHCI bus of **every** boot
//! this kernel has ever taken, one hop below the root-mounted hub, and the only line it has ever
//! produced is the EHCI HID walk's shrug:
//!
//! ```text
//! :: EHCI-HID: [0] M1 hub-downstream device addr=2 05ac:8510 class=0xef speed=HS depth=1 (parent hub 1 port 1) tt=(hub 0 port 0) == witness ::
//! :: EHCI-HID: [0] addr 2 has no HID interrupt-IN endpoint — nothing to arm ::
//! ```
//!
//! Class `0xef` is Miscellaneous/Common with an Interface Association Descriptor (USB 2.0 IAD ECN,
//! Table 9-Z: `bDeviceClass = 0xEF`, `bDeviceSubClass = 0x02`, `bDeviceProtocol = 0x01`) — the
//! device telling the host "my interfaces are grouped into functions, read the IADs". A camera's
//! IAD has `bFunctionClass = CC_VIDEO (0x0E)` (USB Video Class 1.1 §3.5, Table 3-1).
//!
//! **THIS RUNG IS CONTROL TRANSFERS ONLY.** It reads descriptors on EP0 and negotiates
//! `VS_PROBE_CONTROL` on EP0, and it stops there — deliberately, one step short of the thing that
//! would make a picture:
//!
//! * **no isochronous pipe is opened.** The EHCI driver drives EP0 and interrupt-IN QHs; it has no
//!   isochronous transfer descriptor (iTD/siTD) path at all, and this Panther Point's async engine
//!   master-aborts every schedule fetch (PROBE-14, `ehci/mod.rs`), so the periodic-QH discipline
//!   the rest of the driver is built on is the only one available. Building that is the NEXT rung.
//! * **`VS_COMMIT_CONTROL` is never sent.** UVC 1.1 §4.3.1.1: a successful `SET_CUR` on
//!   `VS_COMMIT_CONTROL` is what "selects the video format and frame" — the device arms its
//!   streaming state machine and expects the host to select a non-zero alternate setting and start
//!   pulling isochronous data. Committing a format this arc cannot then read would leave the
//!   camera armed for a stream nobody drains, for the rest of the boot, on the same controller as
//!   the internal keyboard and trackpad. So the wire says so, once, by name:
//!   `[uvc] commit=withheld reason=no-iso-pipe`.
//! * **no `SET_CUR` on any VideoControl control.** Brightness, exposure, focus and the rest are
//!   read as CAPABILITY BITMAPS out of the descriptors (UVC 1.1 Tables 3-6 and 3-8) and are never
//!   written. A probe that changes the device is not a probe.
//!
//! `SET_CUR` on `VS_PROBE_CONTROL` IS sent, and that is not an exception to the paragraph above:
//! UVC 1.1 §4.3.1.1 defines the Probe control as the NEGOTIATION channel — "the host sets the
//! desired values and the device returns the values it can actually support" — with no effect on
//! the device's streaming state until Commit. It is the one write whose whole purpose is to ask a
//! question, and the answer (`dwMaxPayloadTransferSize`) is precisely what the next rung needs in
//! order to pick an alternate setting.
//!
//! # CLEAN ROOM
//!
//! Sources, both public specifications from the USB Implementers Forum, cited inline by section
//! and table number throughout this file: **USB Device Class Definition for Video Devices,
//! revision 1.1** (the descriptor layouts in §3.6–§3.9 and Appendix A's class/subtype/request
//! codes, §4.3.1.1's Probe/Commit block, Appendix B's terminal types), its **Uncompressed** and
//! **Motion-JPEG Payload** companion documents (the format and frame descriptor tables, §3.1/§3.2
//! of each), and **USB 2.0** (§9.4/§9.5/§9.6 standard requests and descriptors, Table 9-13's
//! endpoint `wMaxPacketSize` encoding, and the IAD ECN). No third-party driver source, naming or
//! constants-by-name were consulted; every constant below is written out of a spec table and
//! carries the table it came from.
//!
//! # SHAPE
//!
//! [`probe`] is handed the 64-byte configuration-descriptor window `configure_hid` already holds,
//! the device's `bConfigurationValue`, the EP0 data buffer, and a closure over the EHCI driver's
//! own `control()` — the same helper the HID walk and the Bluetooth census issue every one of
//! their transfers through. It owns no hardware, maps nothing, and allocates nothing.
//!
//! [`parse`] is a PURE function over a descriptor byte-slice, and the whole census is its return
//! value. That split is what makes [`selftest`] possible on a machine with no camera: the same
//! parser is fed a hand-built descriptor set written from the spec's tables and its output is
//! asserted field by field. The self-test is not a smoke test — mutate one offset in [`parse`] and
//! it prints `-> FAIL` with the field, the value it read and the value the fixture declares.
//!
//! # BOUNDS
//!
//! Every control transfer is issued exactly ONCE. `control()` is internally bounded (it is the
//! same call the enumeration walk bounds itself with), and a failure is reported on one line
//! naming the STAGE and the REQUEST and then abandons the device — there is no retry loop in this
//! file, by construction, because the failure mode a camera on a shared bus can inflict is a
//! retry storm on the controller the keyboard is enumerating through.

use core::sync::atomic::{AtomicBool, Ordering};

// ── USB 2.0 descriptor types (§9.4, Table 9-5, plus the IAD ECN) ────────────────────────────────
const DT_INTERFACE: u8 = 0x04;
const DT_ENDPOINT: u8 = 0x05;
const DT_INTERFACE_ASSOCIATION: u8 = 0x0B;

// ── UVC 1.1 Appendix A.1/A.2: class and subclass codes ──────────────────────────────────────────
const CC_VIDEO: u8 = 0x0E;
const SC_VIDEOCONTROL: u8 = 0x01;
const SC_VIDEOSTREAMING: u8 = 0x02;

// ── UVC 1.1 Appendix A.4: class-specific descriptor types ───────────────────────────────────────
const CS_INTERFACE: u8 = 0x24;

// ── UVC 1.1 Appendix A.5: VideoControl interface descriptor subtypes ────────────────────────────
const VC_HEADER: u8 = 0x01;
const VC_INPUT_TERMINAL: u8 = 0x02;
const VC_OUTPUT_TERMINAL: u8 = 0x03;
const VC_PROCESSING_UNIT: u8 = 0x05;

// ── UVC 1.1 Appendix A.6: VideoStreaming interface descriptor subtypes ──────────────────────────
const VS_INPUT_HEADER: u8 = 0x01;
const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
const VS_FRAME_UNCOMPRESSED: u8 = 0x05;
const VS_FORMAT_MJPEG: u8 = 0x06;
const VS_FRAME_MJPEG: u8 = 0x07;

// ── UVC 1.1 Appendix B.2: input terminal types ──────────────────────────────────────────────────
const ITT_CAMERA: u16 = 0x0201;

// ── UVC 1.1 Appendix A.8: class-specific request codes ──────────────────────────────────────────
const RQ_SET_CUR: u8 = 0x01;
const RQ_GET_CUR: u8 = 0x81;
const RQ_GET_MIN: u8 = 0x82;
const RQ_GET_MAX: u8 = 0x83;
const RQ_GET_DEF: u8 = 0x87;

// ── UVC 1.1 Appendix A.9.8: VideoStreaming interface control selectors ──────────────────────────
const VS_PROBE_CONTROL: u8 = 0x01;

/// UVC 1.1 §4.3.1.1, Table 4-47. The 1.0 block is 26 bytes; 1.1 appends `dwClockFrequency`,
/// `bmFramingInfo`, `bPreferedVersion`, `bMinVersion` and `bMaxVersion` for 34. We ASK for the
/// larger one and let the device's short packet tell us which it speaks — the transferred byte
/// count is the answer, so no version inference is needed and none is made.
const PROBE_LEN_ASK: u16 = 34;
/// The 1.0 block: every field this rung prints lives inside it, so this is the honest floor.
const PROBE_LEN_MIN: usize = 26;

/// `bmRequestType` for a class request to an INTERFACE (USB 2.0 §9.3.1): device-to-host, class,
/// interface recipient.
const BMREQ_GET_INTF: u8 = 0xA1;
/// The same, host-to-device.
const BMREQ_SET_INTF: u8 = 0x21;

// Census capacities. Deliberately small and deliberately LOUD when exceeded — a census that
// silently drops the format the next rung wants is worse than one that says it dropped it.
const MAX_FMT: usize = 4;
const MAX_FRAME: usize = 8;
const MAX_ALT: usize = 8;
const MAX_INTERVALS: usize = 8;

/// One frame descriptor (Uncompressed payload §3.2 Table 3-2 / MJPEG payload §3.2 Table 3-2 —
/// the two are field-for-field identical and are parsed by one arm for that reason).
#[derive(Clone, Copy)]
pub struct Frame {
    pub index: u8,
    pub width: u16,
    pub height: u16,
    /// `dwDefaultFrameInterval`, in 100 ns units as the spec stores it.
    pub default_interval: u32,
    /// `bFrameIntervalType`: 0 = CONTINUOUS (min/max/step follow), n = n discrete values follow.
    pub interval_type: u8,
    pub n_intervals: usize,
    pub intervals: [u32; MAX_INTERVALS],
    /// Only meaningful when `interval_type == 0`.
    pub min_interval: u32,
    pub max_interval: u32,
    pub interval_step: u32,
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            index: 0,
            width: 0,
            height: 0,
            default_interval: 0,
            interval_type: 0,
            n_intervals: 0,
            intervals: [0; MAX_INTERVALS],
            min_interval: 0,
            max_interval: 0,
            interval_step: 0,
        }
    }
}

/// What KIND of payload a format descriptor declares. `Other` is not a failure — it is the honest
/// reading of a format this rung does not parse the frame table of (frame-based, stream-based,
/// MPEG2-TS, DV), and it is printed as such rather than dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Uncompressed,
    Mjpeg,
    Other,
}

/// One format descriptor and the frame descriptors that follow it.
#[derive(Clone, Copy)]
pub struct Fmt {
    pub index: u8,
    pub kind: Kind,
    /// Uncompressed payload §3.1 Table 3-1 `guidFormat`. The first four bytes of a UVC format GUID
    /// are its FourCC (the GUID is `<fourcc>-0000-0010-8000-00AA00389B71`), which is why the census
    /// line prints both. Zero for a format that carries no GUID (MJPEG has none).
    pub guid: [u8; 16],
    pub bits_per_pixel: u8,
    pub default_frame: u8,
    /// `bNumFrameDescriptors` AS DECLARED. Kept apart from `nframes` (what we actually parsed) so
    /// a truncated descriptor window is visible on the line rather than inferred from it.
    pub declared_frames: u8,
    pub nframes: usize,
    pub frames: [Frame; MAX_FRAME],
}

impl Default for Fmt {
    fn default() -> Self {
        Fmt {
            index: 0,
            kind: Kind::Other,
            guid: [0; 16],
            bits_per_pixel: 0,
            default_frame: 0,
            declared_frames: 0,
            nframes: 0,
            frames: [Frame::default(); MAX_FRAME],
        }
    }
}

/// One alternate setting of the VideoStreaming interface and its isochronous IN endpoint. The NEXT
/// rung picks one of these: `mps * mult` bytes per microframe is the bandwidth the endpoint
/// reserves, and `dwMaxPayloadTransferSize` from the probe block is what it has to cover.
#[derive(Clone, Copy, Default)]
pub struct AltEp {
    pub alt: u8,
    /// Endpoint number, low 4 bits of `bEndpointAddress` (the direction bit is implied — only
    /// IN endpoints are recorded).
    pub ep: u8,
    /// `wMaxPacketSize` bits 10..0 (USB 2.0 Table 9-13).
    pub mps: u16,
    /// Transactions per microframe: `wMaxPacketSize` bits 12..11, plus one (USB 2.0 Table 9-13 —
    /// the field encodes "additional transactions", so 0 means one transaction per microframe).
    pub mult: u8,
}

/// The whole census, as a value. Every printed line and every self-test assertion reads THIS —
/// nothing re-walks the descriptors, so the wire and the fixture are scoring the same parser.
#[derive(Clone, Copy)]
pub struct Census {
    pub have_iad: bool,
    pub iad_first: u8,
    pub iad_count: u8,
    pub iad_subclass: u8,

    pub have_vc: bool,
    pub vc_intf: u8,
    pub bcd_uvc: u16,
    pub vc_total_len: u16,
    pub clock_hz: u32,
    pub in_collection: u8,

    pub have_it: bool,
    pub it_id: u8,
    pub it_type: u16,
    pub it_ctrl_len: u8,
    pub it_ctrl: u32,

    pub have_pu: bool,
    pub pu_id: u8,
    pub pu_src: u8,
    pub pu_ctrl_len: u8,
    pub pu_ctrl: u32,

    pub have_ot: bool,
    pub ot_id: u8,
    pub ot_type: u16,
    pub ot_src: u8,

    pub have_vs: bool,
    pub vs_intf: u8,
    pub vs_ep: u8,
    pub vs_terminal_link: u8,
    pub declared_formats: u8,

    pub nfmt: usize,
    pub fmts: [Fmt; MAX_FMT],
    pub nalt: usize,
    pub alts: [AltEp; MAX_ALT],

    /// The walk ran off the end of the window mid-descriptor. NOT a parse failure — it is the
    /// EP0 data buffer's 256-byte ceiling meeting a configuration descriptor larger than it, which
    /// is a real and expected outcome for a camera and must never be silent.
    pub ran_short: bool,
    /// A capacity above was hit. Same rule: said, never inferred.
    pub overflowed: bool,
}

impl Default for Census {
    fn default() -> Self {
        Census {
            have_iad: false,
            iad_first: 0,
            iad_count: 0,
            iad_subclass: 0,
            have_vc: false,
            vc_intf: 0,
            bcd_uvc: 0,
            vc_total_len: 0,
            clock_hz: 0,
            in_collection: 0,
            have_it: false,
            it_id: 0,
            it_type: 0,
            it_ctrl_len: 0,
            it_ctrl: 0,
            have_pu: false,
            pu_id: 0,
            pu_src: 0,
            pu_ctrl_len: 0,
            pu_ctrl: 0,
            have_ot: false,
            ot_id: 0,
            ot_type: 0,
            ot_src: 0,
            have_vs: false,
            vs_intf: 0,
            vs_ep: 0,
            vs_terminal_link: 0,
            declared_formats: 0,
            nfmt: 0,
            fmts: [Fmt::default(); MAX_FMT],
            nalt: 0,
            alts: [AltEp::default(); MAX_ALT],
            ran_short: false,
            overflowed: false,
        }
    }
}

#[inline]
fn le16(b: &[u8], o: usize) -> u16 {
    (b[o] as u16) | ((b[o + 1] as u16) << 8)
}

#[inline]
fn le32(b: &[u8], o: usize) -> u32 {
    (b[o] as u32) | ((b[o + 1] as u32) << 8) | ((b[o + 2] as u32) << 16) | ((b[o + 3] as u32) << 24)
}

/// A control bitmap of `n` bytes (n is 1..=4 in every UVC table that has one) as a `u32`. The
/// spec's `bmControls` is little-endian and variable-length; widening it here means the printed
/// line and the self-test compare ONE number instead of a byte array whose length is itself data.
fn bitmap(b: &[u8], o: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..n.min(4) {
        if o + i < b.len() {
            v |= (b[o + i] as u32) << (8 * i);
        }
    }
    v
}

/// THE CANDIDATE GATE, run on the 64-byte window `configure_hid` already holds, before one byte of
/// new wire traffic.
///
/// A configuration descriptor puts its first IAD immediately after the 9-byte configuration
/// descriptor (USB 2.0 IAD ECN: an IAD "must be located before the interface descriptors of the
/// interfaces it associates"), so a video function's IAD is inside the first 64 bytes of every
/// composite camera this test is meant to catch — the same property `bt_cfg_has_candidate` relies
/// on for the Bluetooth census. Returning false means `probe` issues NO transfer and prints one
/// line, so every non-camera on the bus costs a walk of at most 64 bytes.
pub fn cfg_has_video_iad(cfg: &[u8]) -> bool {
    let mut off = 0usize;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 {
            break;
        }
        if cfg[off + 1] == DT_INTERFACE_ASSOCIATION
            && off + 8 <= cfg.len()
            && cfg[off + 4] == CC_VIDEO
        {
            return true;
        }
        off += len;
    }
    false
}

/// THE PARSER. Pure, total, and the only thing that reads a descriptor byte in this file.
///
/// The walk is the standard USB descriptor walk (USB 2.0 §9.5: a configuration descriptor is a
/// concatenation of descriptors, each led by its own `bLength`), with the class-specific arms
/// selected by the class/subclass of the interface descriptor most recently seen — which is what
/// the spec requires, since `CS_INTERFACE` (0x24) means different things under VideoControl and
/// VideoStreaming (UVC 1.1 §3.7 vs §3.9) and the byte itself does not say which.
pub fn parse(cfg: &[u8]) -> Census {
    let mut c = Census::default();
    let mut off = 0usize;
    let (mut in_vc, mut in_vs, mut cur_alt) = (false, false, 0u8);
    let mut cur_fmt: Option<usize> = None;

    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 {
            break; // a zero-length descriptor is unwalkable; stop rather than spin
        }
        if off + len > cfg.len() {
            c.ran_short = true;
            break;
        }
        let d = &cfg[off..off + len];
        match d[1] {
            DT_INTERFACE_ASSOCIATION if len >= 8 => {
                // USB 2.0 IAD ECN Table 9-Z: bFirstInterface[2] bInterfaceCount[3]
                // bFunctionClass[4] bFunctionSubClass[5].
                if d[4] == CC_VIDEO && !c.have_iad {
                    c.have_iad = true;
                    c.iad_first = d[2];
                    c.iad_count = d[3];
                    c.iad_subclass = d[5];
                }
            }
            DT_INTERFACE if len >= 9 => {
                // USB 2.0 Table 9-12: bInterfaceNumber[2] bAlternateSetting[3] bNumEndpoints[4]
                // bInterfaceClass[5] bInterfaceSubClass[6].
                cur_alt = d[3];
                in_vc = d[5] == CC_VIDEO && d[6] == SC_VIDEOCONTROL;
                in_vs = d[5] == CC_VIDEO && d[6] == SC_VIDEOSTREAMING;
                if in_vc {
                    c.have_vc = true;
                    c.vc_intf = d[2];
                }
                if in_vs {
                    c.have_vs = true;
                    c.vs_intf = d[2];
                }
                cur_fmt = None;
            }
            CS_INTERFACE if in_vc && len >= 3 => match d[2] {
                // UVC 1.1 §3.7.2, Table 3-3: bcdUVC[3..5] wTotalLength[5..7]
                // dwClockFrequency[7..11] bInCollection[11].
                VC_HEADER if len >= 12 => {
                    c.bcd_uvc = le16(d, 3);
                    c.vc_total_len = le16(d, 5);
                    c.clock_hz = le32(d, 7);
                    c.in_collection = d[11];
                }
                // UVC 1.1 §3.7.2.1, Table 3-4: bTerminalID[3] wTerminalType[4..6]
                // bAssocTerminal[6] iTerminal[7]. For a CAMERA terminal (wTerminalType ==
                // ITT_CAMERA) §3.7.2.3 Table 3-6 extends it: wObjectiveFocalLengthMin[8..10]
                // wObjectiveFocalLengthMax[10..12] wOcularFocalLength[12..14] bControlSize[14]
                // bmControls[15..].
                VC_INPUT_TERMINAL if len >= 8 => {
                    c.have_it = true;
                    c.it_id = d[3];
                    c.it_type = le16(d, 4);
                    if c.it_type == ITT_CAMERA && len >= 15 {
                        c.it_ctrl_len = d[14];
                        c.it_ctrl = bitmap(d, 15, d[14] as usize);
                    }
                }
                // UVC 1.1 §3.7.2.2, Table 3-5: bTerminalID[3] wTerminalType[4..6]
                // bAssocTerminal[6] bSourceID[7] iTerminal[8].
                VC_OUTPUT_TERMINAL if len >= 9 => {
                    c.have_ot = true;
                    c.ot_id = d[3];
                    c.ot_type = le16(d, 4);
                    c.ot_src = d[7];
                }
                // UVC 1.1 §3.7.2.5, Table 3-8: bUnitID[3] bSourceID[4] wMaxMultiplier[5..7]
                // bControlSize[7] bmControls[8..8+n] iProcessing[8+n].
                VC_PROCESSING_UNIT if len >= 8 => {
                    c.have_pu = true;
                    c.pu_id = d[3];
                    c.pu_src = d[4];
                    c.pu_ctrl_len = d[7];
                    c.pu_ctrl = bitmap(d, 8, d[7] as usize);
                }
                _ => {}
            },
            CS_INTERFACE if in_vs && len >= 3 => match d[2] {
                // UVC 1.1 §3.9.2.1, Table 3-13: bNumFormats[3] wTotalLength[4..6]
                // bEndpointAddress[6] bmInfo[7] bTerminalLink[8].
                VS_INPUT_HEADER if len >= 9 => {
                    c.declared_formats = d[3];
                    c.vs_ep = d[6];
                    c.vs_terminal_link = d[8];
                }
                // Uncompressed payload §3.1.1, Table 3-1: bFormatIndex[3]
                // bNumFrameDescriptors[4] guidFormat[5..21] bBitsPerPixel[21]
                // bDefaultFrameIndex[22].
                VS_FORMAT_UNCOMPRESSED if len >= 23 => {
                    if c.nfmt < MAX_FMT {
                        let mut f = Fmt::default();
                        f.index = d[3];
                        f.kind = Kind::Uncompressed;
                        f.declared_frames = d[4];
                        f.guid.copy_from_slice(&d[5..21]);
                        f.bits_per_pixel = d[21];
                        f.default_frame = d[22];
                        c.fmts[c.nfmt] = f;
                        cur_fmt = Some(c.nfmt);
                        c.nfmt += 1;
                    } else {
                        c.overflowed = true;
                        cur_fmt = None;
                    }
                }
                // MJPEG payload §3.1.1, Table 3-1: bFormatIndex[3] bNumFrameDescriptors[4]
                // bmFlags[5] bDefaultFrameIndex[6]. No GUID — the format IS the identifier.
                VS_FORMAT_MJPEG if len >= 7 => {
                    if c.nfmt < MAX_FMT {
                        let mut f = Fmt::default();
                        f.index = d[3];
                        f.kind = Kind::Mjpeg;
                        f.declared_frames = d[4];
                        f.default_frame = d[6];
                        c.fmts[c.nfmt] = f;
                        cur_fmt = Some(c.nfmt);
                        c.nfmt += 1;
                    } else {
                        c.overflowed = true;
                        cur_fmt = None;
                    }
                }
                // Uncompressed payload §3.1.2 Table 3-2 and MJPEG payload §3.1.2 Table 3-2 —
                // IDENTICAL layouts, which is why one arm parses both and the subtype only
                // decides which format it belongs to:
                //   bFrameIndex[3] bmCapabilities[4] wWidth[5..7] wHeight[7..9]
                //   dwMinBitRate[9..13] dwMaxBitRate[13..17] dwMaxVideoFrameBufferSize[17..21]
                //   dwDefaultFrameInterval[21..25] bFrameIntervalType[25]
                //   then, if bFrameIntervalType == 0: dwMinFrameInterval, dwMaxFrameInterval,
                //   dwFrameIntervalStep; else bFrameIntervalType x dwFrameInterval.
                VS_FRAME_UNCOMPRESSED | VS_FRAME_MJPEG if len >= 26 => {
                    if let Some(fi) = cur_fmt {
                        if c.fmts[fi].nframes < MAX_FRAME {
                            let mut fr = Frame::default();
                            fr.index = d[3];
                            fr.width = le16(d, 5);
                            fr.height = le16(d, 7);
                            fr.default_interval = le32(d, 21);
                            fr.interval_type = d[25];
                            if fr.interval_type == 0 {
                                if len >= 38 {
                                    fr.min_interval = le32(d, 26);
                                    fr.max_interval = le32(d, 30);
                                    fr.interval_step = le32(d, 34);
                                }
                            } else {
                                let n = (fr.interval_type as usize).min(MAX_INTERVALS);
                                for i in 0..n {
                                    let o = 26 + i * 4;
                                    if o + 4 <= len {
                                        fr.intervals[fr.n_intervals] = le32(d, o);
                                        fr.n_intervals += 1;
                                    }
                                }
                                if (fr.interval_type as usize) > MAX_INTERVALS {
                                    c.overflowed = true;
                                }
                            }
                            let n = c.fmts[fi].nframes;
                            c.fmts[fi].frames[n] = fr;
                            c.fmts[fi].nframes += 1;
                        } else {
                            c.overflowed = true;
                        }
                    }
                }
                _ => {}
            },
            // USB 2.0 §9.6.6, Table 9-13: bEndpointAddress[2] bmAttributes[3]
            // wMaxPacketSize[4..6] bInterval[6]. bmAttributes bits 1..0 == 01 is isochronous.
            // Recorded for VideoStreaming alternates only — that is the set the next rung chooses
            // from, and the VideoControl interface's optional interrupt endpoint is not one.
            DT_ENDPOINT if in_vs && len >= 7 => {
                if d[2] & 0x80 != 0 && (d[3] & 0x03) == 0x01 {
                    if c.nalt < MAX_ALT {
                        let w = le16(d, 4);
                        c.alts[c.nalt] = AltEp {
                            alt: cur_alt,
                            ep: d[2] & 0x0F,
                            mps: w & 0x07FF,
                            mult: (((w >> 11) & 0x03) as u8) + 1,
                        };
                        c.nalt += 1;
                    } else {
                        c.overflowed = true;
                    }
                }
            }
            _ => {}
        }
        off += len;
    }
    c
}

/// UVC 1.1 §4.3.1.1, Table 4-47 — the six fields this rung reports, out of a Probe/Commit block.
#[derive(Clone, Copy, Default)]
pub struct ProbeBlock {
    pub len: usize,
    pub hint: u16,
    pub format_index: u8,
    pub frame_index: u8,
    pub frame_interval: u32,
    pub max_video_frame_size: u32,
    pub max_payload_transfer_size: u32,
}

/// Table 4-47 offsets: bmHint[0..2] bFormatIndex[2] bFrameIndex[3] dwFrameInterval[4..8]
/// wKeyFrameRate[8..10] wPFrameRate[10..12] wCompQuality[12..14] wCompWindowSize[14..16]
/// wDelay[16..18] dwMaxVideoFrameSize[18..22] dwMaxPayloadTransferSize[22..26].
pub fn parse_probe(b: &[u8]) -> Option<ProbeBlock> {
    if b.len() < PROBE_LEN_MIN {
        return None;
    }
    Some(ProbeBlock {
        len: b.len(),
        hint: le16(b, 0),
        format_index: b[2],
        frame_index: b[3],
        frame_interval: le32(b, 4),
        max_video_frame_size: le32(b, 18),
        max_payload_transfer_size: le32(b, 22),
    })
}

/// Build a Probe block to SET. Only the four fields UVC 1.1 §4.3.1.1 says the host "shall" supply
/// for a negotiation are written; the rest are left zero, which is the spec's own instruction for
/// fields the host has no preference about ("the device returns the value it will use").
fn build_probe(out: &mut [u8], format_index: u8, frame_index: u8, interval: u32) {
    for b in out.iter_mut() {
        *b = 0;
    }
    // bmHint bit 0 = dwFrameInterval is fixed (Table 4-48): we are asking for THIS frame rate, so
    // the device may vary the other fields but not that one.
    out[0] = 0x01;
    out[1] = 0x00;
    out[2] = format_index;
    out[3] = frame_index;
    out[4] = interval as u8;
    out[5] = (interval >> 8) as u8;
    out[6] = (interval >> 16) as u8;
    out[7] = (interval >> 24) as u8;
}

// ── The wire ────────────────────────────────────────────────────────────────────────────────────

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Uncompressed => "uncompressed",
        Kind::Mjpeg => "mjpeg",
        Kind::Other => "other",
    }
}

/// The first four bytes of a UVC format GUID are its FourCC, in ASCII. Non-printable bytes become
/// `.` so a malformed GUID can never inject control characters into the serial log.
fn fourcc(guid: &[u8; 16]) -> [char; 4] {
    let mut out = ['.'; 4];
    for i in 0..4 {
        let b = guid[i];
        out[i] = if (0x20..0x7f).contains(&b) { b as char } else { '.' };
    }
    out
}

fn print_census(idx: usize, addr: u8, c: &Census) {
    crate::census_println!(
        "[uvc] vc ctrl={} addr={} intf={} bcdUVC={:#06x} clock_hz={} vc_total_len={} in_collection={} iad first={} count={} sub={:#04x}",
        idx, addr, c.vc_intf, c.bcd_uvc, c.clock_hz, c.vc_total_len, c.in_collection,
        c.iad_first, c.iad_count, c.iad_subclass
    );
    if c.have_it {
        crate::census_println!(
            "[uvc] vc term=input id={} type={:#06x}{} ctrl_len={} ctrl={:#010x}",
            c.it_id, c.it_type,
            if c.it_type == ITT_CAMERA { " (camera)" } else { "" },
            c.it_ctrl_len, c.it_ctrl
        );
    }
    if c.have_pu {
        crate::census_println!(
            "[uvc] vc unit=processing id={} src={} ctrl_len={} ctrl={:#010x}",
            c.pu_id, c.pu_src, c.pu_ctrl_len, c.pu_ctrl
        );
    }
    if c.have_ot {
        crate::census_println!(
            "[uvc] vc term=output id={} type={:#06x} src={}",
            c.ot_id, c.ot_type, c.ot_src
        );
    }
    crate::census_println!(
        "[uvc] vs intf={} ep={:#04x} terminal_link={} formats_declared={} formats_seen={}",
        c.vs_intf, c.vs_ep, c.vs_terminal_link, c.declared_formats, c.nfmt
    );
    for f in c.fmts[..c.nfmt].iter() {
        let cc = fourcc(&f.guid);
        crate::census_println!(
            "[uvc] vs fmt={} kind={} fourcc={}{}{}{} guid={:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} bpp={} default_frame={} frames={}/{}",
            f.index, kind_name(f.kind), cc[0], cc[1], cc[2], cc[3],
            f.guid[0], f.guid[1], f.guid[2], f.guid[3], f.guid[4], f.guid[5], f.guid[6], f.guid[7],
            f.guid[8], f.guid[9], f.guid[10], f.guid[11], f.guid[12], f.guid[13], f.guid[14], f.guid[15],
            f.bits_per_pixel, f.default_frame, f.nframes, f.declared_frames
        );
        for fr in f.frames[..f.nframes].iter() {
            // dwFrameInterval is in 100 ns units (Uncompressed payload §3.2): /10 is microseconds.
            if fr.interval_type == 0 {
                crate::census_println!(
                    "[uvc] frame fmt={} idx={} {}x{} default_us={} intervals=range min_us={} max_us={} step_us={}",
                    f.index, fr.index, fr.width, fr.height, fr.default_interval / 10,
                    fr.min_interval / 10, fr.max_interval / 10, fr.interval_step / 10
                );
            } else {
                // A list, printed as a list — the census is evidence, not a summary. Up to
                // MAX_INTERVALS of them; `n=` names the DECLARED count so a truncation shows.
                let iv = &fr.intervals[..fr.n_intervals];
                crate::census_println!(
                    "[uvc] frame fmt={} idx={} {}x{} default_us={} intervals=list n={}/{} us=[{} {} {} {} {} {} {} {}]",
                    f.index, fr.index, fr.width, fr.height, fr.default_interval / 10,
                    fr.n_intervals, fr.interval_type,
                    iv.first().copied().unwrap_or(0) / 10,
                    iv.get(1).copied().unwrap_or(0) / 10,
                    iv.get(2).copied().unwrap_or(0) / 10,
                    iv.get(3).copied().unwrap_or(0) / 10,
                    iv.get(4).copied().unwrap_or(0) / 10,
                    iv.get(5).copied().unwrap_or(0) / 10,
                    iv.get(6).copied().unwrap_or(0) / 10,
                    iv.get(7).copied().unwrap_or(0) / 10
                );
            }
        }
    }
    for a in c.alts[..c.nalt].iter() {
        crate::census_println!(
            "[uvc] alt={} ep=IN{} mps={} mult={} per_uframe={}",
            a.alt, a.ep, a.mps, a.mult, a.mps as u32 * a.mult as u32
        );
    }
    if c.ran_short || c.overflowed {
        crate::census_println!(
            "[uvc] census INCOMPLETE ran_short={} overflowed={} — the EP0 data buffer is 256 B (qh::Buf256) and a camera's configuration descriptor is routinely larger; the lines above are the wire truth for the WINDOW, not for the device",
            c.ran_short, c.overflowed
        );
    }
}

// ── The self-test ───────────────────────────────────────────────────────────────────────────────

/// A configuration descriptor written out of the spec's tables, byte by byte, with the values
/// chosen so that a mis-read offset cannot alias a correct one (every width, height, id and
/// interval below is distinct from every other number in the fixture).
///
/// Shape: config -> IAD(video) -> VC interface { header, camera input terminal, processing unit,
/// output terminal } -> VS interface alt 0 { input header, uncompressed format + 2 frames (one
/// DISCRETE with two intervals, one CONTINUOUS), MJPEG format + 1 frame } -> VS interface alt 1
/// { isochronous IN endpoint }.
#[rustfmt::skip]
const FIXTURE: [u8; 289] = [
    // ── Configuration descriptor (USB 2.0 §9.6.3, Table 9-10) ──
    // wTotalLength = 289 = 0x0121, little-endian — the sum of every descriptor below, and the
    // length of this array (the walk's `ran_short` flag is asserted false, which is what ties the
    // two together: a descriptor crossing the end of the array would set it).
    9, 0x02, 0x21, 0x01, 2, 1, 0, 0x80, 50,
    // ── Interface Association Descriptor (USB 2.0 IAD ECN, Table 9-Z) ──
    // bFirstInterface=0 bInterfaceCount=2 bFunctionClass=0x0E bFunctionSubClass=0x03 (SC_VIDEO_
    // INTERFACE_COLLECTION, UVC 1.1 §A.2) bFunctionProtocol=0 iFunction=0
    8, 0x0B, 0, 2, 0x0E, 0x03, 0x00, 0,
    // ── VideoControl interface, alt 0 (USB 2.0 Table 9-12) ──
    // bInterfaceNumber=0 alt=0 bNumEndpoints=0 class=0x0E subclass=0x01 protocol=0 iInterface=0
    9, 0x04, 0, 0, 0, 0x0E, 0x01, 0x00, 0,
    // ── VC_HEADER (UVC 1.1 Table 3-3), bLength = 12 + bInCollection ──
    // bcdUVC=0x0100 wTotalLength=51 dwClockFrequency=6_000_000 (0x005B8D80) bInCollection=1
    // baInterfaceNr(1)=1
    13, 0x24, 0x01, 0x00, 0x01, 51, 0, 0x80, 0x8D, 0x5B, 0x00, 1, 1,
    // ── VC_INPUT_TERMINAL, camera (UVC 1.1 Table 3-6), bLength = 15 + bControlSize ──
    // bTerminalID=1 wTerminalType=0x0201 bAssocTerminal=0 iTerminal=0
    // wObjectiveFocalLengthMin=0 wObjectiveFocalLengthMax=0 wOcularFocalLength=0
    // bControlSize=3 bmControls=0x0A0B0C -> little-endian 0x000C0B0A
    18, 0x24, 0x02, 1, 0x01, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0x0A, 0x0B, 0x0C,
    // ── VC_PROCESSING_UNIT (UVC 1.1 Table 3-8), bLength = 8 + bControlSize + 1 ──
    // bUnitID=2 bSourceID=1 wMaxMultiplier=0 bControlSize=2 bmControls=0x1E2D -> 0x00002D1E
    // iProcessing=0
    11, 0x24, 0x05, 2, 1, 0, 0, 2, 0x1E, 0x2D, 0,
    // ── VC_OUTPUT_TERMINAL (UVC 1.1 Table 3-5) ──
    // bTerminalID=3 wTerminalType=0x0101 (TT_STREAMING, §B.1) bAssocTerminal=0 bSourceID=2
    // iTerminal=0
    9, 0x24, 0x03, 3, 0x01, 0x01, 0, 2, 0,
    // ── VideoStreaming interface, alt 0 (no endpoint — UVC 1.1 §3.9) ──
    // bInterfaceNumber=1 alt=0 bNumEndpoints=0 class=0x0E subclass=0x02
    9, 0x04, 1, 0, 0, 0x0E, 0x02, 0x00, 0,
    // ── VS_INPUT_HEADER (UVC 1.1 Table 3-13), bLength = 13 + bControlSize*bNumFormats ──
    // bNumFormats=2 wTotalLength=155 (this header + both formats + all three frames)
    // bEndpointAddress=0x81 bmInfo=0 bTerminalLink=3
    // bStillCaptureMethod=0 bTriggerSupport=0 bTriggerUsage=0 bControlSize=1 bmaControls=0,0
    15, 0x24, 0x01, 2, 155, 0, 0x81, 0, 3, 0, 0, 0, 1, 0, 0,
    // ── VS_FORMAT_UNCOMPRESSED (Uncompressed payload Table 3-1), bLength = 27 ──
    // bFormatIndex=1 bNumFrameDescriptors=2 guid = 'YUY2' + the UVC GUID tail
    // bBitsPerPixel=16 bDefaultFrameIndex=1 bAspectRatioX/Y=0 bmInterlaceFlags=0 bCopyProtect=0
    27, 0x24, 0x04, 1, 2,
    b'Y', b'U', b'Y', b'2', 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    16, 1, 0, 0, 0, 0,
    // ── VS_FRAME_UNCOMPRESSED #1, DISCRETE with 2 intervals (Table 3-2), bLength = 26 + 8 ──
    // bFrameIndex=1 bmCapabilities=0 wWidth=640 wHeight=480
    // dwMinBitRate=147456000 is not asserted; dwMaxVideoFrameBufferSize=614400
    // dwDefaultFrameInterval=333333 (0x00051615, 33.3333 ms = 30 fps)
    // bFrameIntervalType=2, dwFrameInterval[0]=333333, dwFrameInterval[1]=666666 (0x000A2C2A)
    34, 0x24, 0x05, 1, 0, 0x80, 0x02, 0xE0, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x60, 0x09, 0x00,
    0x15, 0x16, 0x05, 0x00,
    2,
    0x15, 0x16, 0x05, 0x00,
    0x2A, 0x2C, 0x0A, 0x00,
    // ── VS_FRAME_UNCOMPRESSED #2, CONTINUOUS (Table 3-2), bLength = 26 + 12 = 38 ──
    // bFrameIndex=2 wWidth=1280 wHeight=720 dwDefaultFrameInterval=400000 (0x00061A80, 25 fps)
    // bFrameIntervalType=0 dwMinFrameInterval=333333 dwMaxFrameInterval=1000000 (0x000F4240)
    // dwFrameIntervalStep=1 (0x00000001)
    38, 0x24, 0x05, 2, 0, 0x00, 0x05, 0xD0, 0x02,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x1C, 0x00,
    0x80, 0x1A, 0x06, 0x00,
    0,
    0x15, 0x16, 0x05, 0x00,
    0x40, 0x42, 0x0F, 0x00,
    0x01, 0x00, 0x00, 0x00,
    // ── VS_FORMAT_MJPEG (MJPEG payload Table 3-1), bLength = 11 ──
    // bFormatIndex=2 bNumFrameDescriptors=1 bmFlags=0 bDefaultFrameIndex=1
    //   ⚠ bmFlags is 0 and not 1, and the mutation sweep is why: with both bytes set to 1, moving
    //   `f.default_frame` from d[6] to d[5] read the SAME value and the self-test stayed at PASS.
    //   Two adjacent fields holding the same number is a fixture that cannot tell them apart.
    11, 0x24, 0x06, 2, 1, 0, 1, 0, 0, 0, 0,
    // ── VS_FRAME_MJPEG #1, DISCRETE with 1 interval (Table 3-2), bLength = 26 + 4 = 30 ──
    // bFrameIndex=1 wWidth=1920 wHeight=1080 dwDefaultFrameInterval=166666 (0x00028B0A, 60 fps)
    // bFrameIntervalType=1 dwFrameInterval[0]=166666
    30, 0x24, 0x07, 1, 0, 0x80, 0x07, 0x38, 0x04,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x1F, 0x00,
    0x0A, 0x8B, 0x02, 0x00,
    1,
    0x0A, 0x8B, 0x02, 0x00,
    // ── VideoStreaming interface, alt 1 — the first streaming alternate ──
    // bInterfaceNumber=1 alt=1 bNumEndpoints=1 class=0x0E subclass=0x02
    9, 0x04, 1, 1, 1, 0x0E, 0x02, 0x00, 0,
    // ── Isochronous IN endpoint (USB 2.0 Table 9-13) ──
    // bEndpointAddress=0x81 bmAttributes=0x05 (isochronous, asynchronous)
    // wMaxPacketSize=0x0C00: bits 10..0 = 0x400 = 1024 bytes, bits 12..11 = 1 = ONE additional
    // transaction per microframe, so mult = 2. bInterval=1.
    //   ⚠ THIS VALUE IS CHOSEN, NOT ARBITRARY, and a mutation sweep is why. With the only
    //   endpoint at 0x1400 (bit 11 CLEAR), widening `parse`'s size mask from 0x07FF to 0x0FFF
    //   changed nothing — 0x1400 & 0x0FFF is still 0x400 — and the self-test stayed at PASS on a
    //   parser that no longer honours Table 9-13's field split. 0x0C00 has bit 11 SET, so the
    //   same mutation reads 3072 and the check fires. The 3-transaction case below keeps the
    //   realistic high-bandwidth shape a camera actually offers.
    7, 0x05, 0x81, 0x05, 0x00, 0x0C, 1,
    // ── VideoStreaming interface, alt 2 — the wider streaming alternate ──
    9, 0x04, 1, 2, 1, 0x0E, 0x02, 0x00, 0,
    // wMaxPacketSize=0x1400 -> 1024 bytes x 3 transactions per microframe (3072 B/uframe)
    7, 0x05, 0x81, 0x05, 0x00, 0x14, 1,
    // ── VideoStreaming interface, alt 3 — THE NEGATIVE CASE ──
    // A VideoStreaming interface may be an OUTPUT one (UVC 1.1 §3.9.2.2, VS_OUTPUT_HEADER — a
    // display sink rather than a camera), and its isochronous endpoint is then an OUT. The census
    // is a list of pipes the next rung can PULL frames from, so an OUT endpoint must be walked
    // past and NOT recorded: `alts` stays 2 with this alternate present.
    //   ⚠ THIS ALTERNATE EXISTS BECAUSE THE SWEEP FOUND THE CHECK COULD NOT FIRE. With only IN
    //   endpoints in the fixture, deleting `parse`'s `d[2] & 0x80 != 0` direction test changed
    //   nothing and the self-test stayed at PASS on a parser that would have offered the next rung
    //   an OUT pipe to read from. A corpus that can produce only one outcome tests nothing
    //   (LAWS §5).
    9, 0x04, 1, 3, 1, 0x0E, 0x02, 0x00, 0,
    // bEndpointAddress=0x02 — isochronous OUT. Same attributes and packet size as alt 2, so the
    // ONLY thing that may keep it out of the census is the direction bit.
    7, 0x05, 0x02, 0x05, 0x00, 0x14, 1,
];

static SELFTEST_RUN: AtomicBool = AtomicBool::new(false);

/// Drive [`parse`] over [`FIXTURE`] and assert its output field by field.
///
/// THE POINT, and why this is not decoration: this kernel will meet exactly one UVC device (the
/// rMBP's own camera) and only on metal, so without this the descriptor parser would first execute
/// on a machine nobody can single-step, against a descriptor set nobody has read, and a wrong
/// offset would present as a plausible-looking wrong number. Here every expected value is written
/// beside the fixture bytes that encode it, so a mutated offset in `parse` has nowhere to hide.
///
/// GO-RED, by construction: each check names its field, what was read and what the fixture
/// declares, and the line ends `-> FAIL`, which `arroyo`'s FAULT_PATTERNS list makes an exit-1 for
/// the whole run. The first failure is reported and the rest are still counted, so one line says
/// how bad it is.
pub fn selftest() -> bool {
    let c = parse(&FIXTURE);
    let mut fails = 0u32;
    let mut first: (&'static str, u64, u64) = ("", 0, 0);

    // `check` is a closure over the two accumulators rather than a macro so that a failing field
    // cannot early-return and hide the ones after it.
    let mut check = |name: &'static str, got: u64, want: u64| {
        if got != want {
            if fails == 0 {
                first = (name, got, want);
            }
            fails += 1;
        }
    };

    check("iad.present", c.have_iad as u64, 1);
    check("iad.first", c.iad_first as u64, 0);
    check("iad.count", c.iad_count as u64, 2);
    check("iad.subclass", c.iad_subclass as u64, 0x03);

    check("vc.present", c.have_vc as u64, 1);
    check("vc.intf", c.vc_intf as u64, 0);
    check("vc.bcdUVC", c.bcd_uvc as u64, 0x0100);
    check("vc.total_len", c.vc_total_len as u64, 51);
    check("vc.clock_hz", c.clock_hz as u64, 6_000_000);
    check("vc.in_collection", c.in_collection as u64, 1);

    check("it.present", c.have_it as u64, 1);
    check("it.id", c.it_id as u64, 1);
    check("it.type", c.it_type as u64, 0x0201);
    check("it.ctrl_len", c.it_ctrl_len as u64, 3);
    check("it.ctrl", c.it_ctrl as u64, 0x000C_0B0A);

    check("pu.present", c.have_pu as u64, 1);
    check("pu.id", c.pu_id as u64, 2);
    check("pu.src", c.pu_src as u64, 1);
    check("pu.ctrl_len", c.pu_ctrl_len as u64, 2);
    check("pu.ctrl", c.pu_ctrl as u64, 0x0000_2D1E);

    check("ot.present", c.have_ot as u64, 1);
    check("ot.id", c.ot_id as u64, 3);
    check("ot.type", c.ot_type as u64, 0x0101);
    check("ot.src", c.ot_src as u64, 2);

    check("vs.present", c.have_vs as u64, 1);
    check("vs.intf", c.vs_intf as u64, 1);
    check("vs.ep", c.vs_ep as u64, 0x81);
    check("vs.terminal_link", c.vs_terminal_link as u64, 3);
    check("vs.formats_declared", c.declared_formats as u64, 2);
    check("vs.formats_seen", c.nfmt as u64, 2);

    if c.nfmt >= 1 {
        let f = &c.fmts[0];
        check("fmt0.index", f.index as u64, 1);
        check("fmt0.kind_uncompressed", (f.kind == Kind::Uncompressed) as u64, 1);
        check("fmt0.fourcc", u32::from_le_bytes([f.guid[0], f.guid[1], f.guid[2], f.guid[3]]) as u64,
              u32::from_le_bytes([b'Y', b'U', b'Y', b'2']) as u64);
        check("fmt0.guid_tail", f.guid[15] as u64, 0x71);
        check("fmt0.bpp", f.bits_per_pixel as u64, 16);
        check("fmt0.default_frame", f.default_frame as u64, 1);
        check("fmt0.declared_frames", f.declared_frames as u64, 2);
        check("fmt0.frames", f.nframes as u64, 2);
        if f.nframes >= 1 {
            let fr = &f.frames[0];
            check("fmt0.frame0.index", fr.index as u64, 1);
            check("fmt0.frame0.width", fr.width as u64, 640);
            check("fmt0.frame0.height", fr.height as u64, 480);
            check("fmt0.frame0.default_interval", fr.default_interval as u64, 333_333);
            check("fmt0.frame0.interval_type", fr.interval_type as u64, 2);
            check("fmt0.frame0.n_intervals", fr.n_intervals as u64, 2);
            check("fmt0.frame0.interval0", fr.intervals[0] as u64, 333_333);
            check("fmt0.frame0.interval1", fr.intervals[1] as u64, 666_666);
        }
        if f.nframes >= 2 {
            let fr = &f.frames[1];
            check("fmt0.frame1.index", fr.index as u64, 2);
            check("fmt0.frame1.width", fr.width as u64, 1280);
            check("fmt0.frame1.height", fr.height as u64, 720);
            check("fmt0.frame1.default_interval", fr.default_interval as u64, 400_000);
            check("fmt0.frame1.interval_type", fr.interval_type as u64, 0);
            check("fmt0.frame1.min_interval", fr.min_interval as u64, 333_333);
            check("fmt0.frame1.max_interval", fr.max_interval as u64, 1_000_000);
            check("fmt0.frame1.interval_step", fr.interval_step as u64, 1);
        }
    }
    if c.nfmt >= 2 {
        let f = &c.fmts[1];
        check("fmt1.index", f.index as u64, 2);
        check("fmt1.kind_mjpeg", (f.kind == Kind::Mjpeg) as u64, 1);
        check("fmt1.default_frame", f.default_frame as u64, 1);
        check("fmt1.frames", f.nframes as u64, 1);
        if f.nframes >= 1 {
            let fr = &f.frames[0];
            check("fmt1.frame0.index", fr.index as u64, 1);
            check("fmt1.frame0.width", fr.width as u64, 1920);
            check("fmt1.frame0.height", fr.height as u64, 1080);
            check("fmt1.frame0.default_interval", fr.default_interval as u64, 166_666);
            check("fmt1.frame0.interval_type", fr.interval_type as u64, 1);
            check("fmt1.frame0.n_intervals", fr.n_intervals as u64, 1);
            check("fmt1.frame0.interval0", fr.intervals[0] as u64, 166_666);
        }
    }

    check("alts", c.nalt as u64, 2);
    if c.nalt >= 1 {
        let a = &c.alts[0];
        check("alt0.alt", a.alt as u64, 1);
        check("alt0.ep", a.ep as u64, 1);
        check("alt0.mps", a.mps as u64, 1024);
        check("alt0.mult", a.mult as u64, 2);
    }
    if c.nalt >= 2 {
        let a = &c.alts[1];
        check("alt1.alt", a.alt as u64, 2);
        check("alt1.ep", a.ep as u64, 1);
        check("alt1.mps", a.mps as u64, 1024);
        check("alt1.mult", a.mult as u64, 3);
    }

    // The walk must consume the fixture exactly: `ran_short` says a descriptor crossed the end,
    // `overflowed` says a capacity was hit. Either on a fixture written to fit is a parser defect.
    check("census.ran_short", c.ran_short as u64, 0);
    check("census.overflowed", c.overflowed as u64, 0);

    // The Probe-block reader is scored on the same principle — a block built by `build_probe` and
    // read back by `parse_probe`, so a mutated offset in either is caught by the other.
    let mut blk = [0u8; PROBE_LEN_MIN];
    build_probe(&mut blk, 7, 5, 333_333);
    match parse_probe(&blk) {
        Some(p) => {
            check("probe.roundtrip.len", p.len as u64, PROBE_LEN_MIN as u64);
            check("probe.roundtrip.hint", p.hint as u64, 0x0001);
            check("probe.roundtrip.format_index", p.format_index as u64, 7);
            check("probe.roundtrip.frame_index", p.frame_index as u64, 5);
            check("probe.roundtrip.frame_interval", p.frame_interval as u64, 333_333);
        }
        None => check("probe.roundtrip.parsed", 0, 1),
    }
    // AND A SYNTHETIC DEVICE REPLY, because the round trip above is not sufficient and this was
    // MEASURED, not reasoned. `build_probe` deliberately leaves `dwMaxVideoFrameSize` and
    // `dwMaxPayloadTransferSize` zero (the host has no preference; the device fills them), so a
    // round trip over that block reads zero at both offsets and reads zero at any WRONG offset
    // too — a check that cannot fire (LAWS §5). Proof: with only the round trip, mutating
    // `parse_probe`'s `max_payload_transfer_size` from `le32(b, 22)` to `le32(b, 21)` left this
    // self-test at PASS. `PROBE_REPLY` carries a DISTINCT value in every field this rung reads, so
    // each offset is now pinned by a number no other field holds. Table 4-47 field by field:
    #[rustfmt::skip]
    const PROBE_REPLY: [u8; PROBE_LEN_MIN] = [
        0x03, 0x00,             // bmHint = 0x0003
        2,                      // bFormatIndex
        3,                      // bFrameIndex
        0x15, 0x16, 0x05, 0x00, // dwFrameInterval = 333333
        0x11, 0x00,             // wKeyFrameRate
        0x22, 0x00,             // wPFrameRate
        0x33, 0x00,             // wCompQuality
        0x44, 0x00,             // wCompWindowSize
        0x55, 0x00,             // wDelay
        0x00, 0x60, 0x09, 0x00, // dwMaxVideoFrameSize      = 614400 (640x480 at 16 bpp)
        0x00, 0x0C, 0x00, 0x00, // dwMaxPayloadTransferSize = 3072   (1024 x 3 per microframe)
    ];
    match parse_probe(&PROBE_REPLY) {
        Some(p) => {
            check("probe.reply.len", p.len as u64, PROBE_LEN_MIN as u64);
            check("probe.reply.hint", p.hint as u64, 0x0003);
            check("probe.reply.format_index", p.format_index as u64, 2);
            check("probe.reply.frame_index", p.frame_index as u64, 3);
            check("probe.reply.frame_interval", p.frame_interval as u64, 333_333);
            check("probe.reply.max_video_frame_size", p.max_video_frame_size as u64, 614_400);
            check("probe.reply.max_payload", p.max_payload_transfer_size as u64, 3_072);
        }
        None => check("probe.reply.parsed", 0, 1),
    }
    // A block one byte short of the 1.0 minimum must be REFUSED, not half-read: the refusal is the
    // only thing standing between a runt reply and six fabricated fields on the wire.
    let runt = [0u8; PROBE_LEN_MIN - 1];
    check("probe.runt_refused", parse_probe(&runt).is_none() as u64, 1);

    // The counts on the PASS line are the CENSUS's own, not literals: a line that restates the
    // fixture would be a claim about a constant, and this one is a measurement of the parser.
    let total_frames: usize = c.fmts[..c.nfmt].iter().map(|f| f.nframes).sum();
    if fails == 0 {
        serial_println!(
            ":: uvc: selftest fixture={}B formats={} frames={} alts={} probe_roundtrip=ok probe_reply=ok -> PASS ::",
            FIXTURE.len(), c.nfmt, total_frames, c.nalt
        );
        true
    } else {
        serial_println!(
            ":: uvc: selftest fixture={}B failures={} first={} got={} want={} -> FAIL ::",
            FIXTURE.len(), fails, first.0, first.1, first.2
        );
        false
    }
}

// ── The probe ───────────────────────────────────────────────────────────────────────────────────

/// The EP0 transfer seam: `(bmRequestType, bRequest, wValue, wIndex, wLength, dir_in)` ->
/// bytes transferred. This is exactly `ehci::Controller::control`'s signature with the `Target`
/// already bound, which is what lets this whole driver live outside `ehci/mod.rs` and be reached
/// from it by ONE folded call.
pub type Xfer<'a> = &'a mut dyn FnMut(u8, u8, u16, u16, u16, bool) -> Result<u32, &'static str>;

/// THE ENTRY POINT, called once per walked non-hub device from `ehci::Controller::configure_hid`.
///
/// `cfg` is the 64-byte configuration-descriptor window that function already read; `buf` is the
/// shared EP0 data buffer every transfer lands in (`qh::Buf256`), which is why `cfg` is DEAD the
/// moment the first transfer below is issued and is only read before that point.
///
/// # Safety
/// `buf` must be the live EP0 data buffer of the controller `xfer` drives, valid for
/// `qh::Buf256`'s 256 bytes, and `xfer` must be safe to call for the device `cfg` describes.
pub unsafe fn probe(idx: usize, addr: u8, cfg: &[u8], config_value: u8, buf: *mut u8, xfer: Xfer) {
    // The self-test runs ONCE per boot, at the first device this driver is offered, and it runs
    // BEFORE the candidate gate on purpose: a boot with no camera on the bus is exactly the boot
    // where the parser is otherwise never exercised, and that is the boot this gate is for.
    if !SELFTEST_RUN.swap(true, Ordering::Relaxed) {
        if !crate::tests::defer("uvc", uvc_fixture) { selftest(); } // QUIETBOOT3 (B352, R80): a boot fixture — `tests uvc` fires it.
    }

    if !cfg_has_video_iad(cfg) {
        crate::bootlog_println!("[uvc] skip addr={} reason=no-video-iad", addr);
        return;
    }

    // A candidate: re-read the configuration descriptor IN FULL before walking it, exactly as the
    // Bluetooth census does and for the same reason — 64 bytes is a stub of a composite device,
    // and a camera's VideoStreaming interface (the half this rung needs) is never inside it.
    // `data_buf` is 256 bytes, so that is the ceiling; `ran_short` on the census line says when
    // the device's own wTotalLength exceeded it, and NOTHING here pretends otherwise.
    let wtotal = if cfg.len() >= 4 { le16(cfg, 2) } else { 0 }; #[cfg(feature = "sonde")] multi::ident(addr, buf, &mut *xfer); // REALSENSE (LEDGER SR92): WHOSE camera — one GET_DESCRIPTOR(DEVICE) for a video-IAD candidate, AFTER `cfg` (which aliases `buf`) is read for the last time. LINE-NEUTRAL append, before this comment; knob off erases it.
    let want = wtotal.min(256);
    crate::census_println!(
        "[uvc] candidate addr={} cfg_value={} wTotalLength={} reading={}{}",
        addr, config_value, wtotal, want,
        if wtotal > 256 { " (EP0 data buffer is 256 B — the tail of this descriptor is UNREAD)" } else { "" }
    );
    // GET_DESCRIPTOR(CONFIGURATION, 0) — USB 2.0 §9.4.3, bmRequestType 0x80, bRequest 6,
    // wValue = (0x02 << 8) | index. ONE attempt: see the module header on retry storms.
    let n = match xfer(0x80, 6, 0x0200, 0, want, true) {
        Ok(n) if n >= 9 => n as usize,
        Ok(n) => {
            serial_println!(
                "[uvc] abort addr={} stage=cfg-reread req=GET_DESCRIPTOR(CONFIGURATION) wLength={} got={} reason=runt",
                addr, want, n
            );
            return;
        }
        Err(e) => {
            serial_println!(
                "[uvc] abort addr={} stage=cfg-reread req=GET_DESCRIPTOR(CONFIGURATION) wLength={} reason={}",
                addr, want, e
            );
            return;
        }
    };
    let full = core::slice::from_raw_parts(buf as *const u8, n);
    let c = parse(full); #[cfg(feature = "sonde")] multi::after_census(addr, config_value, full, buf, 256, &mut *xfer); // REALSENSE (LEDGER SR92): the per-interface census (depth/ir/color functions, XUs, HID) of the same window, and a D400's three XU GET_CURs; `full` is read before its first transfer and never after. LINE-NEUTRAL append, before this comment.
    print_census(idx, addr, &c);

    if !c.have_vs {
        crate::bootlog_println!(
            "[uvc] probe skipped addr={} reason=no-videostreaming-interface-in-window read={} wTotalLength={}",
            addr, n, wtotal
        );
        return;
    }
    // Pick the first UNCOMPRESSED format and its default frame — the brief's choice, and the right
    // one: an uncompressed frame's size is arithmetic (w*h*bpp/8), so `dwMaxVideoFrameSize` coming
    // back from the device is a number the next rung can CHECK rather than merely record.
    let mut pick: Option<(u8, u8, u32)> = None;
    for f in c.fmts[..c.nfmt].iter() {
        if f.kind != Kind::Uncompressed || f.nframes == 0 {
            continue;
        }
        // bDefaultFrameIndex names a frame by INDEX, not by position (Uncompressed payload §3.1.1);
        // find it, and fall back to the first frame descriptor if the device names one it did not
        // then describe — a real malformation, and named on the line when it happens.
        let fr = f.frames[..f.nframes]
            .iter()
            .find(|fr| fr.index == f.default_frame)
            .unwrap_or(&f.frames[0]);
        if fr.index != f.default_frame {
            crate::bootlog_println!(
                "[uvc] note addr={} fmt={} bDefaultFrameIndex={} names no frame descriptor — falling back to idx={}",
                addr, f.index, f.default_frame, fr.index
            );
        }
        pick = Some((f.index, fr.index, fr.default_interval));
        break;
    }
    let Some((fmt_i, frame_i, interval)) = pick else {
        serial_println!("[uvc] probe skipped addr={} reason=no-uncompressed-format", addr);
        return;
    };

    // Class requests to the VideoStreaming INTERFACE: wValue = selector << 8 (UVC 1.1 §4.2),
    // wIndex = (entity id << 8) | interface number, and the entity for a VS interface control is 0.
    let wvalue = (VS_PROBE_CONTROL as u16) << 8;
    let windex = c.vs_intf as u16;

    // SET_CONFIGURATION first. Until a device is CONFIGURED, USB 2.0 §9.4.3 leaves requests to an
    // interface undefined — the device is entitled to stall every one of them — and the HID walk
    // that called us has not configured this device (it has no HID endpoint to arm). So this is
    // the one piece of device STATE this rung changes, it is the standard request that makes the
    // rest legal, and it is reported.
    if xfer(0x00, 9, config_value as u16, 0, 0, false).is_err() {
        serial_println!(
            "[uvc] abort addr={} stage=set-configuration req=SET_CONFIGURATION({}) reason=stalled-or-timeout",
            addr, config_value
        );
        return;
    }
    crate::bootlog_println!("[uvc] configured addr={} cfg_value={} vs_intf={}", addr, config_value, c.vs_intf);

    // GET_MIN / GET_MAX / GET_DEF, in that order (UVC 1.1 §4.3.1.1 — the negotiation reads the
    // bounds before it asks for anything). Each is ONE transfer; a stall is reported with its
    // stage and its request code and ends the sequence.
    let mut blk_len = 0usize;
    for (stage, req) in [("min", RQ_GET_MIN), ("max", RQ_GET_MAX), ("def", RQ_GET_DEF)] {
        match xfer(BMREQ_GET_INTF, req, wvalue, windex, PROBE_LEN_ASK, true) {
            Ok(got) => {
                let b = core::slice::from_raw_parts(buf as *const u8, got as usize);
                match parse_probe(b) {
                    Some(p) => {
                        blk_len = p.len;
                        crate::census_println!(
                            "[uvc] probe stage={} req={:#04x} len={} bmHint={:#06x} bFormatIndex={} bFrameIndex={} dwFrameInterval={} dwMaxVideoFrameSize={} dwMaxPayloadTransferSize={}",
                            stage, req, p.len, p.hint, p.format_index, p.frame_index,
                            p.frame_interval, p.max_video_frame_size, p.max_payload_transfer_size
                        );
                    }
                    None => serial_println!(
                        "[uvc] probe stage={} req={:#04x} RUNT got={} need>={} — no fields read from it",
                        stage, req, got, PROBE_LEN_MIN
                    ),
                }
            }
            Err(e) => {
                serial_println!(
                    "[uvc] probe stage={} req={:#04x} wValue={:#06x} wIndex={} wLength={} STALLED reason={} — sequence ended, NOT retried",
                    stage, req, wvalue, windex, PROBE_LEN_ASK, e
                );
                return;
            }
        }
    }
    if blk_len < PROBE_LEN_MIN {
        serial_println!(
            "[uvc] probe abort addr={} reason=no-usable-block-length — every GET was a runt",
            addr
        );
        return;
    }

    // SET_CUR(Probe) — the negotiation write. It changes no streaming state (UVC 1.1 §4.3.1.1);
    // the device's answer to the GET_CUR that follows is the whole product of this rung.
    let w = core::slice::from_raw_parts_mut(buf, blk_len);
    build_probe(w, fmt_i, frame_i, interval);
    crate::census_println!(
        "[uvc] probe stage=set req={:#04x} len={} asking bFormatIndex={} bFrameIndex={} dwFrameInterval={} ({} us)",
        RQ_SET_CUR, blk_len, fmt_i, frame_i, interval, interval / 10
    );
    if xfer(BMREQ_SET_INTF, RQ_SET_CUR, wvalue, windex, blk_len as u16, false).is_err() {
        serial_println!(
            "[uvc] probe stage=set req={:#04x} wValue={:#06x} wIndex={} wLength={} STALLED — sequence ended, NOT retried",
            RQ_SET_CUR, wvalue, windex, blk_len
        );
        return;
    }
    match xfer(BMREQ_GET_INTF, RQ_GET_CUR, wvalue, windex, blk_len as u16, true) {
        Ok(got) => {
            let b = core::slice::from_raw_parts(buf as *const u8, got as usize);
            match parse_probe(b) {
                Some(p) => {
                    serial_println!(
                        "[uvc] probe stage=cur req={:#04x} len={} bmHint={:#06x} bFormatIndex={} bFrameIndex={} dwFrameInterval={} dwMaxVideoFrameSize={} dwMaxPayloadTransferSize={}",
                        RQ_GET_CUR, p.len, p.hint, p.format_index, p.frame_index,
                        p.frame_interval, p.max_video_frame_size, p.max_payload_transfer_size
                    );
                    // The negotiation VERDICT, stated as a comparison and not as a hope: a device
                    // is entitled to answer with a different format, frame or interval than the
                    // one asked for, and "it agreed" must be a measurement.
                    serial_println!(
                        "[uvc] probe negotiated asked=({},{},{}) got=({},{},{}) agreed={}",
                        fmt_i, frame_i, interval,
                        p.format_index, p.frame_index, p.frame_interval,
                        p.format_index == fmt_i && p.frame_index == frame_i && p.frame_interval == interval
                    );
                    // What the next rung needs, named on one line so it is not re-derived.
                    serial_println!(
                        "[uvc] next-rung needs alts={} payload_per_transfer={} — the alternate whose mps*mult >= that is the one to select",
                        c.nalt, p.max_payload_transfer_size
                    );
                }
                None => serial_println!(
                    "[uvc] probe stage=cur req={:#04x} RUNT got={} need>={}",
                    RQ_GET_CUR, got, PROBE_LEN_MIN
                ),
            }
        }
        Err(e) => {
            serial_println!(
                "[uvc] probe stage=cur req={:#04x} wValue={:#06x} wIndex={} wLength={} STALLED reason={}",
                RQ_GET_CUR, wvalue, windex, blk_len, e
            );
            return;
        }
    }

    // The line this whole rung is bounded by. See the module header: committing a format the
    // kernel cannot then drain leaves the camera armed for a stream nobody reads, for the rest of
    // the boot, on the controller the keyboard is on.
    serial_println!("[uvc] commit=withheld reason=no-iso-pipe");
}

// ── XHCIISO (rmbp-ledger B534): the stream on xHCI — the commit, the isochronous pipe, the payload, the frames ──
//
// INSERTED between `probe` and the fixture, so not one line of `probe` moves or changes: the EHCI path above
// still ends `commit=withheld reason=no-iso-pipe`, because EHCI still has no iTD/siTD path and that line is
// still TRUE there (and `banner-cert.sh` certifies it in the artifact). This section is the OTHER host:
// `probe_iso` is reached from the xHCI driver's main-loop service (`drivers/xhci/isoch.rs`) for a slot whose
// device class is 0xEF (root port) or whose 64-byte configuration window carries a video IAD (behind a hub).
// It owns no hardware either: every wire action goes through the `IsoHost` the controller hands it — EP0 into
// a 4 KiB buffer (the 256-byte ceiling of §8 in uvc.md is EHCI's buffer, not USB's), and the isochronous IN
// endpoint's Configure Endpoint / ring fill / drop. The frames come back up through `stream_payload`, called
// by the controller's transfer-event claim once per completed Isoch TD.
//
// Sources, as above: UVC 1.1 (and 1.5, whose §2.4.3.3 payload header and §4.3.1.1 Probe/Commit block are the
// same bytes) and USB 2.0 / USB 3.2 for the endpoint and companion descriptors. Every constant names its table.

/// UVC 1.1 Appendix A.9.8 (1.5 Table A-13): `VS_COMMIT_CONTROL`. `VS_PROBE_CONTROL` (0x01) is above.
const VS_COMMIT_CONTROL: u8 = 0x02;
/// USB 3.2 §9.6.7, Table 9-27: `SUPERSPEED_USB_ENDPOINT_COMPANION` descriptor type.
const DT_SS_EP_COMPANION: u8 = 0x30;

/// UVC 1.5 §2.4.3.3 (the Payload Header table; 1.1 §2.4.3.3 is byte-identical): `bmHeaderInfo` bits.
const BFH_FID: u8 = 0x01; // D0 Frame ID — toggles at every frame boundary
const BFH_EOF: u8 = 0x02; // D1 End of Frame — set on the last payload of a frame
const BFH_PTS: u8 = 0x04; // D2 dwPresentationTime (4 bytes) follows bmHeaderInfo
const BFH_SCR: u8 = 0x08; // D3 scrSourceClock (6 bytes: 32-bit STC + 16-bit SOF counter) follows the PTS
const BFH_ERR: u8 = 0x40; // D6 Error — the device says this payload's frame is bad
const BFH_EOH: u8 = 0x80; // D7 End of Header

/// The EP0 data buffer the xHCI host offers (one 4 KiB page, 4 KiB-aligned so the single Data TRB can never
/// cross a 64 KiB boundary — xHCI 1.2 §6.4.1.1/§6.4.1.2.2 data-buffer rule). 4096 covers every configuration
/// descriptor a camera has been seen to declare and the 1024-byte vendor XU command REALSENSE names.
pub const XHCI_EP0_CAP: usize = 4096;

/// The double-buffer ceiling: a frame larger than this is COUNTED, not stored (`store=count-only` on the arm
/// line). 4 MiB covers 1280x720 YUY2 (1 843 200) and the D435i's 848x480 Z16 (814 080) twice over.
const STREAM_STORE_MAX: usize = 4 * 1024 * 1024;

/// The isochronous IN endpoint of one VideoStreaming alternate, read from the full configuration descriptor:
/// the endpoint descriptor (USB 2.0 §9.6.6, Table 9-13) and, on a SuperSpeed device, the companion after it.
#[derive(Clone, Copy, Default)]
pub struct IsoEp {
    pub intf: u8,
    pub alt: u8,
    /// `bEndpointAddress` (direction bit set — only IN endpoints are returned).
    pub addr: u8,
    /// `wMaxPacketSize` bits 10..0.
    pub mps: u16,
    /// `wMaxPacketSize` bits 12..11: ADDITIONAL transactions per microframe (high-speed, Table 9-13).
    pub hs_extra: u8,
    /// `bInterval` as the descriptor holds it (an exponent for HS/SS/FS isochronous: 2^(bInterval-1)).
    pub interval: u8,
    /// A SuperSpeed Endpoint Companion followed the endpoint (USB 3.2 §9.6.7) — the device is at SuperSpeed.
    pub ss: bool,
    /// Companion `bMaxBurst` (0..15: packets per burst minus one).
    pub ss_burst: u8,
    /// Companion `bmAttributes` bits 1..0 for an isochronous endpoint: `Mult` (bursts per interval minus one).
    pub ss_mult: u8,
    /// Companion `wBytesPerInterval`.
    pub ss_bpi: u16,
}

impl IsoEp {
    /// Bytes one service interval can carry: high-speed `mps x (1 + extra)` (USB 2.0 §5.6.3 + Table 9-13);
    /// SuperSpeed `wBytesPerInterval` (USB 3.2 §9.6.7), or `mps x burst x mult` when a device leaves it zero.
    pub fn esit_bytes(&self) -> u32 {
        if self.ss {
            if self.ss_bpi != 0 {
                self.ss_bpi as u32
            } else {
                self.mps as u32 * (self.ss_burst as u32 + 1) * (self.ss_mult as u32 + 1)
            }
        } else {
            self.mps as u32 * (self.hs_extra as u32 + 1)
        }
    }

    /// Transactions (HS) or bursts (SS) per interval, as the census line counts `mult`.
    pub fn mult(&self) -> u8 {
        if self.ss { self.ss_mult + 1 } else { self.hs_extra + 1 }
    }
}

/// PURE: the isochronous IN endpoint of `(intf, alt)` in a full configuration descriptor, and its companion.
/// `None` when that alternate has no isochronous IN endpoint (an OUT one — the fixture's alt 3 — included).
pub fn iso_ep_of(cfg: &[u8], intf: u8, alt: u8) -> Option<IsoEp> {
    let mut off = 0usize;
    let mut here = false;
    let mut found: Option<IsoEp> = None;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 || off + len > cfg.len() {
            break;
        }
        let d = &cfg[off..off + len];
        match d[1] {
            // USB 2.0 Table 9-12: bInterfaceNumber[2] bAlternateSetting[3].
            DT_INTERFACE if len >= 9 => {
                if found.is_some() {
                    break; // the next interface: no companion can follow any more
                }
                here = d[2] == intf && d[3] == alt;
            }
            // Table 9-13: bEndpointAddress[2] bmAttributes[3] (bits 1..0 = 01 isochronous)
            // wMaxPacketSize[4..6] bInterval[6].
            DT_ENDPOINT if here && len >= 7 && found.is_none() => {
                if d[2] & 0x80 != 0 && (d[3] & 0x03) == 0x01 {
                    let w = le16(d, 4);
                    found = Some(IsoEp {
                        intf,
                        alt,
                        addr: d[2],
                        mps: w & 0x07FF,
                        hs_extra: ((w >> 11) & 0x03) as u8,
                        interval: d[6],
                        ..IsoEp::default()
                    });
                }
            }
            // USB 3.2 Table 9-27: bMaxBurst[2] bmAttributes[3] wBytesPerInterval[4..6]. It immediately follows
            // its endpoint descriptor, so the FIRST companion after `found` is that endpoint's.
            DT_SS_EP_COMPANION if len >= 6 => {
                if let Some(e) = found.as_mut() {
                    if !e.ss {
                        e.ss = true;
                        e.ss_burst = d[2] & 0x0F;
                        e.ss_mult = d[3] & 0x03;
                        e.ss_bpi = le16(d, 4);
                    }
                    return found;
                }
            }
            DT_ENDPOINT if found.is_some() => return found, // another endpoint: no companion for ours
            _ => {}
        }
        off += len;
    }
    found
}

/// PURE: the length of the prefix of `cfg` that holds the configuration, the VideoControl interface and the
/// FIRST VideoStreaming function only — cut at the first interface descriptor (on a descriptor boundary) whose
/// number differs from the first VideoStreaming interface's, once that one has been seen. `parse` keeps ONE
/// `vs_intf` and one format list, so on a multi-function camera (the D435i: depth, IR and colour functions)
/// the census of the whole window mixes three functions' formats and alternates; the census of this prefix is
/// one function's, which is the one this rung streams. Returns `cfg.len()` for a single-function camera.
pub fn first_function_len(cfg: &[u8]) -> usize {
    let mut off = 0usize;
    let mut vs: Option<u8> = None;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 || off + len > cfg.len() {
            break;
        }
        if cfg[off + 1] == DT_INTERFACE && len >= 9 {
            let n = cfg[off + 2];
            match vs {
                Some(v) if v != n => return off,
                None if cfg[off + 5] == CC_VIDEO && cfg[off + 6] == SC_VIDEOSTREAMING => vs = Some(n),
                _ => {}
            }
        }
        off += len;
    }
    cfg.len()
}

/// UVC 1.5 §2.4.3.3: one payload header, decoded.
#[derive(Clone, Copy, Default)]
pub struct PayloadHdr {
    pub len: usize,
    pub fid: bool,
    pub eof: bool,
    pub err: bool,
    pub eoh: bool,
    pub pts: Option<u32>,
    /// (32-bit source time clock, 16-bit SOF counter — bits 10..0 are the 1 kHz SOF number).
    pub scr: Option<(u32, u16)>,
}

/// PURE: decode the header at the front of one isochronous payload. Refused (and the payload dropped by the
/// caller) when `bHeaderLength` is under 2, longer than the payload, or shorter than the fields its own
/// `bmHeaderInfo` declares (2 + 4 for PTS + 6 for SCR) — a header that lies about its length would otherwise
/// shift every image byte after it.
pub fn payload_header(p: &[u8]) -> Result<PayloadHdr, &'static str> {
    if p.len() < 2 {
        return Err("runt");
    }
    let hlen = p[0] as usize;
    let info = p[1];
    if hlen < 2 {
        return Err("hlen<2");
    }
    if hlen > p.len() {
        return Err("hlen>payload");
    }
    let need = 2 + if info & BFH_PTS != 0 { 4 } else { 0 } + if info & BFH_SCR != 0 { 6 } else { 0 };
    if hlen < need {
        return Err("hlen<fields");
    }
    let mut o = 2usize;
    let pts = if info & BFH_PTS != 0 {
        let v = le32(p, o);
        o += 4;
        Some(v)
    } else {
        None
    };
    let scr = if info & BFH_SCR != 0 { Some((le32(p, o), le16(p, o + 4))) } else { None };
    Ok(PayloadHdr {
        len: hlen,
        fid: info & BFH_FID != 0,
        eof: info & BFH_EOF != 0,
        err: info & BFH_ERR != 0,
        eoh: info & BFH_EOH != 0,
        pts,
        scr,
    })
}

/// THE FRAME ASSEMBLER. Pure (it owns two byte buffers and counters, and touches nothing else), so the self-test
/// drives the SAME code the claim path does.
///
/// Frame boundaries, UVC 1.5 §2.4.3.3: a frame is the run of payloads sharing one FID; `EOF` on a payload closes
/// it early (the next payload's FID toggle then finds nothing open). A toggle with no EOF closes it too — EOF is
/// optional for a device, the toggle is not. `ERR` marks the open frame bad. A closed frame that is neither bad
/// nor larger than the store is SWAPPED to the front (the double buffer: the back is written by the claim path,
/// the front is what `latest_frame` hands up) and `seq` advances; its length against `expect`
/// (`dwMaxVideoFrameSize`) sorts it `good` or `short`.
pub struct Assembler {
    back: alloc::vec::Vec<u8>,
    front: alloc::vec::Vec<u8>,
    expect: usize,
    len: usize,
    fid: Option<bool>,
    bad: bool,
    pub front_len: usize,
    pub seq: u32,
    pub payloads: u32,
    pub bytes: u64,
    pub frames: u32,
    pub good: u32,
    pub short: u32,
    pub errf: u32,
    pub over: u32,
    pub bad_hdr: u32,
}

impl Assembler {
    /// `store` bytes per buffer (0 = count-only), `expect` = the frame size the commit block names.
    pub fn new(store: usize, expect: usize) -> Self {
        let mut back = alloc::vec::Vec::new();
        let mut front = alloc::vec::Vec::new();
        if store != 0 && back.try_reserve_exact(store).is_ok() && front.try_reserve_exact(store).is_ok() {
            back.resize(store, 0);
            front.resize(store, 0);
        } else {
            back = alloc::vec::Vec::new();
            front = alloc::vec::Vec::new();
        }
        Assembler {
            back, front, expect, len: 0, fid: None, bad: false, front_len: 0, seq: 0,
            payloads: 0, bytes: 0, frames: 0, good: 0, short: 0, errf: 0, over: 0, bad_hdr: 0,
        }
    }

    pub fn stores(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn front(&self) -> &[u8] {
        &self.front[..self.front_len.min(self.front.len())]
    }

    fn close(&mut self) {
        if self.len == 0 && !self.bad {
            return; // an EOF already closed it; the toggle after finds nothing open
        }
        self.frames = self.frames.wrapping_add(1);
        let over = self.len > self.back.len();
        if self.bad {
            self.errf = self.errf.wrapping_add(1);
        } else if over {
            self.over = self.over.wrapping_add(1);
        } else {
            if self.expect != 0 && self.len != self.expect {
                self.short = self.short.wrapping_add(1);
            } else {
                self.good = self.good.wrapping_add(1);
            }
            core::mem::swap(&mut self.back, &mut self.front);
            self.front_len = self.len;
            self.seq = self.seq.wrapping_add(1);
        }
        self.len = 0;
        self.bad = false;
    }

    /// One isochronous payload (one Isoch TD's bytes). Returns the frames it closed (0, 1 or 2 — a toggle that
    /// closes the previous frame and an EOF that closes this one).
    pub fn push(&mut self, p: &[u8]) -> u32 {
        if p.is_empty() {
            return 0; // a microframe the device had nothing for: normal, not counted
        }
        self.payloads = self.payloads.wrapping_add(1);
        let h = match payload_header(p) {
            Ok(h) => h,
            Err(_) => {
                self.bad_hdr = self.bad_hdr.wrapping_add(1);
                return 0;
            }
        };
        let before = self.frames;
        if let Some(f) = self.fid {
            if f != h.fid {
                self.close();
            }
        }
        self.fid = Some(h.fid);
        if h.err {
            self.bad = true;
        }
        let data = &p[h.len..];
        let room = self.back.len().saturating_sub(self.len);
        let n = data.len().min(room);
        if n != 0 {
            self.back[self.len..self.len + n].copy_from_slice(&data[..n]);
        }
        self.len += data.len(); // counted in full: a frame past the store is `over`, not silently truncated
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        if h.eof {
            self.close();
        }
        self.frames.wrapping_sub(before)
    }
}

/// The xHCI side of the stream, as the class driver sees it. `drivers/xhci/isoch.rs` implements it over one
/// slot; nothing in this file names a register.
pub trait IsoHost {
    /// EP0, the `Xfer` contract: bytes transferred, or the reason. Data lands in / leaves from `buf()`.
    fn control(&mut self, bm: u8, br: u8, wv: u16, wi: u16, wl: u16, dir_in: bool) -> Result<u32, &'static str>;
    fn buf(&mut self) -> *mut u8;
    fn cap(&self) -> usize;
    fn name(&self) -> &'static str;
    /// Configure Endpoint (Add) for `ep` — the controller reserves the bandwidth. Err((completion code, name)).
    fn iso_configure(&mut self, ep: &IsoEp) -> Result<(), (u8, &'static str)>;
    /// Fill the ring and ring the doorbell. Ok((TDs outstanding, lead in microframes, scheduling name)).
    fn iso_start(&mut self, ep: &IsoEp) -> Result<(u32, u32, &'static str), &'static str>;
    /// The unwind of `iso_configure` (Configure Endpoint, Drop).
    fn iso_drop(&mut self, ep: &IsoEp);
}

static STREAM: crate::sync::Mutex<Option<Assembler>> = crate::sync::Mutex::new(None);
static STREAM_ARMED: AtomicBool = AtomicBool::new(false);
/// (addr << 24) | fourcc-less identity for `tests uvc`: addr, width, height, packed.
static STREAM_ADDR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static STREAM_WH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static STREAM_CC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn stream_arm(addr: u8, w: u16, h: u16, cc: [u8; 4], expect: u32) -> bool {
    let store = if expect != 0 && (expect as usize) <= STREAM_STORE_MAX { expect as usize } else { 0 };
    let a = Assembler::new(store, expect as usize);
    let stores = a.stores();
    *STREAM.lock() = Some(a);
    STREAM_ADDR.store(addr as u32, Ordering::Relaxed);
    STREAM_WH.store(((w as u32) << 16) | h as u32, Ordering::Relaxed);
    STREAM_CC.store(u32::from_le_bytes(cc), Ordering::Relaxed);
    STREAM_ARMED.store(true, Ordering::Release); crate::drivers::xhci::uvc::arm(addr, w, h, cc); // UVCCAM (rmbp-ledger B686): the viewer learns the committed geometry and FourCC
    stores
}

/// The stream is gone (unwind, or the device left). The assembler and its buffers are dropped here, in the
/// service context — never on the claim path.
pub fn stream_disarm() {
    STREAM_ARMED.store(false, Ordering::Release); crate::drivers::xhci::uvc::disarm(); // UVCCAM (B686): the viewer closes its window on its next pass
    let _ = STREAM.lock().take();
}

/// THE HAND-UP FROM THE CONTROLLER: one completed Isoch TD's bytes. Called from the xHCI transfer-event claim.
pub fn stream_payload(p: &[u8]) {
    if let Some(a) = STREAM.lock().as_mut() {
        a.push(p);
    }
}

/// THE HAND-UP TO A CONSUMER: the latest complete frame (the front buffer) and its sequence number, under the
/// stream lock (hold it briefly — the claim path waits on it). `false` when no frame has completed yet.
pub fn latest_frame(f: &mut dyn FnMut(&[u8], u32)) -> bool {
    match STREAM.lock().as_ref() {
        Some(a) if a.seq != 0 => {
            f(a.front(), a.seq);
            true
        }
        _ => false,
    }
}

fn cc_str(c: u32) -> [char; 4] {
    fourcc(&{
        let mut g = [0u8; 16];
        g[..4].copy_from_slice(&c.to_le_bytes());
        g
    })
}

/// THE xHCI ENTRY POINT — `probe`'s negotiation, then the commit `probe` withholds. Called once per candidate
/// slot from the controller's main-loop service, never from an event handler (every transfer here pumps the
/// event ring). `cfg` is the first 64 bytes of the configuration descriptor, copied OUT of the EP0 buffer by
/// the caller (so, unlike `probe`'s window, it stays valid across the transfers below).
///
/// The order is the spec's and Linux-shaped where the spec leaves it open: negotiate (Probe), reserve the
/// bandwidth on the host (Configure Endpoint — no device state yet, so a refusal costs nothing), COMMIT
/// (UVC 1.1 §4.3.1.1: the device arms its streaming state machine), SET_INTERFACE to the chosen alternate
/// (USB 2.0 §9.4.10: the endpoint exists on the device from here), then the ring. Each failure names its stage
/// and unwinds what came before it.
pub fn probe_iso(idx: usize, addr: u8, cfg: &[u8], config_value: u8, h: &mut dyn IsoHost) {
    if !SELFTEST_RUN.swap(true, Ordering::Relaxed) {
        if !crate::tests::defer("uvc", uvc_fixture) { selftest(); let _ = stream_selftest(); } // the same once-per-boot rule as `probe`; `tests uvc` fires it
    }
    if !cfg_has_video_iad(cfg) {
        crate::bootlog_println!("[uvc] skip addr={} host={} reason=no-video-iad", addr, h.name());
        return;
    }
    let cap = h.cap();
    let wtotal = if cfg.len() >= 4 { le16(cfg, 2) } else { 0 };
    let want = (wtotal as usize).min(cap) as u16;
    crate::census_println!(
        "[uvc] candidate addr={} host={} cfg_value={} wTotalLength={} reading={} ep0_cap={}{}",
        addr, h.name(), config_value, wtotal, want, cap,
        if wtotal as usize > cap { " (the tail of this descriptor is UNREAD)" } else { "" }
    );
    let buf = h.buf(); #[cfg(feature = "sonde")] multi::ident(addr, buf, &mut |bm, br, wv, wi, wl, di| h.control(bm, br, wv, wi, wl, di)); // ARMSONDE (LEDGER SR101): WHOSE camera, on the xHCI path too — `probe`'s FOLD 1, here before the configuration re-read (`cfg` is the caller's copy, so the 18-byte device read into `buf` destroys nothing). Without it a D435i on any USB-A port (xHCI on every board, the Orin's and the Pi's only bus) was never recorded and `tests sonde` read `SKIP(no-device)` with the camera plugged in.
    let n = match h.control(0x80, 6, 0x0200, 0, want, true) {
        Ok(n) if n >= 9 => (n as usize).min(cap),
        Ok(n) => {
            serial_println!("[uvc] abort addr={} host={} stage=cfg-reread req=GET_DESCRIPTOR(CONFIGURATION) wLength={} got={} reason=runt", addr, h.name(), want, n);
            return;
        }
        Err(e) => {
            serial_println!("[uvc] abort addr={} host={} stage=cfg-reread req=GET_DESCRIPTOR(CONFIGURATION) wLength={} reason={}", addr, h.name(), want, e);
            return;
        }
    };
    // SAFETY: `buf` is the host's EP0 buffer, valid for `cap` bytes, and the transfer above wrote `n` <= cap.
    // `full` is read only until the next transfer, which reuses the buffer.
    let full = unsafe { core::slice::from_raw_parts(buf as *const u8, n) };
    let call = parse(full);
    print_census(idx, addr, &call); crate::drivers::xhci::uvc::census(addr, &call); // UVCCAM (B686): the one-line census (`addr` is the xHCI slot here, the key `note_dev` recorded)
    let cut = first_function_len(full);
    let c = if cut < full.len() { parse(&full[..cut]) } else { call }; #[cfg(feature = "sonde")] let c = multi::depth_fn(full, c); // SONDE2 (LEDGER SR105): a camera whose census has a DEPTH function (first format `Z16 `, probe_core's classification) streams THAT function, not the first one — `parse` of its own interface run in `full`. LINE-NEUTRAL append, before this comment; knob off erases it.
    if cut < full.len() {
        crate::census_println!("[uvc] stream fn intf={} is the first VideoStreaming function (window cut at {} of {})", c.vs_intf, cut, n);
    }
    if !c.have_vs {
        serial_println!("[uvc] probe skipped addr={} host={} reason=no-videostreaming-interface read={} wTotalLength={}", addr, h.name(), n, wtotal);
        return;
    }
    // Every alternate's endpoint, read NOW — the next transfer overwrites `full`.
    let mut eps = [IsoEp::default(); MAX_ALT];
    let mut neps = 0usize;
    for a in c.alts[..c.nalt].iter() {
        if let Some(e) = iso_ep_of(full, c.vs_intf, a.alt) {
            eps[neps] = e;
            neps += 1;
        }
    }
    // The format: `probe`'s rule (the first UNCOMPRESSED format, its default frame) — a size that is arithmetic,
    // so `dwMaxVideoFrameSize` and every assembled frame are numbers the fixture can CHECK.
    let mut pick: Option<(u8, u8, u32, u16, u16, [u8; 4])> = None;
    for f in c.fmts[..c.nfmt].iter() { #[cfg(feature = "sonde")] { if multi::skip_for_z16(&c, f) { continue; } } // SONDE2 (LEDGER SR105): in a function that declares `Z16 `, the depth format is the one committed. LINE-NEUTRAL append, before this comment.
        if f.kind != Kind::Uncompressed || f.nframes == 0 {
            continue;
        }
        let fr = f.frames[..f.nframes].iter().find(|fr| fr.index == f.default_frame).unwrap_or(&f.frames[0]);
        if (fr.width as usize) * (fr.height as usize) * (f.bits_per_pixel as usize) / 8 > STREAM_STORE_MAX {
            continue; // UVCCAM (B686): a default frame past the double-buffer store would be count-only — no picture
        }
        pick = Some((f.index, fr.index, fr.default_interval, fr.width, fr.height, [f.guid[0], f.guid[1], f.guid[2], f.guid[3]]));
        break;
    }
    // UVCCAM (rmbp-ledger B686): no uncompressed format fits the store (or there is none) — commit the first
    // Motion-JPEG format's default frame. MJPEG payload §3.1.1 carries no GUID, so the FourCC is named `MJPG`;
    // the assembler is unchanged (a JPEG shorter than `dwMaxVideoFrameSize` is counted `short` and still published)
    // and `drivers/xhci/uvc.rs` decodes it with `pixel_core::decode_jpeg`.
    if pick.is_none() {
        for f in c.fmts[..c.nfmt].iter() {
            #[cfg(feature = "sonde")]
            {
                if multi::skip_for_z16(&c, f) {
                    continue;
                }
            }
            if f.kind != Kind::Mjpeg || f.nframes == 0 {
                continue;
            }
            let fr = f.frames[..f.nframes].iter().find(|fr| fr.index == f.default_frame).unwrap_or(&f.frames[0]);
            pick = Some((f.index, fr.index, fr.default_interval, fr.width, fr.height, *b"MJPG"));
            break;
        }
    }
    let Some((fmt_i, frame_i, interval, fw, fh, cc4)) = pick else {
        serial_println!("[uvc] probe skipped addr={} host={} reason=no-streamable-format(uncompressed-in-store|mjpeg)", addr, h.name());
        return;
    };
    if neps == 0 {
        serial_println!("[uvc] commit=withheld reason=no-iso-in-alternate addr={} host={} vs_intf={} alts={}", addr, h.name(), c.vs_intf, c.nalt);
        return;
    }
    let wvalue = (VS_PROBE_CONTROL as u16) << 8;
    let windex = c.vs_intf as u16;
    if h.control(0x00, 9, config_value as u16, 0, 0, false).is_err() {
        serial_println!("[uvc] abort addr={} host={} stage=set-configuration req=SET_CONFIGURATION({}) reason=stalled-or-timeout", addr, h.name(), config_value);
        return;
    }
    crate::bootlog_println!("[uvc] configured addr={} host={} cfg_value={} vs_intf={}", addr, h.name(), config_value, c.vs_intf); #[cfg(feature = "sonde")] multi::after_census(addr, config_value, full, buf, cap, &mut |bm, br, wv, wi, wl, di| h.control(bm, br, wv, wi, wl, di)); // ARMSONDE (LEDGER SR101): `probe`'s FOLD 2 on the xHCI path — the multi-function census of the full descriptor and, for a D400, the record plus the three XU GET_CURs. Placed AFTER SET_CONFIGURATION so the XU reads address a configured device, and `full` is still the descriptor here: the only transfer since its read is SET_CONFIGURATION, wLength 0, which has no data stage (`sync_control` touches the buffer only when `w_length > 0`). Every later read in this function is of owned copies (`c`, `eps`, the pick) or of a fresh transfer.
    let mut blk_len = 0usize;
    for (stage, req) in [("min", RQ_GET_MIN), ("max", RQ_GET_MAX), ("def", RQ_GET_DEF)] {
        match h.control(BMREQ_GET_INTF, req, wvalue, windex, PROBE_LEN_ASK, true) {
            Ok(got) => {
                // SAFETY: as `full` above; `got` <= PROBE_LEN_ASK <= cap.
                let b = unsafe { core::slice::from_raw_parts(buf as *const u8, (got as usize).min(cap)) };
                if let Some(p) = parse_probe(b) {
                    blk_len = p.len.min(PROBE_LEN_ASK as usize);
                    crate::census_println!(
                        "[uvc] probe stage={} req={:#04x} len={} bmHint={:#06x} bFormatIndex={} bFrameIndex={} dwFrameInterval={} dwMaxVideoFrameSize={} dwMaxPayloadTransferSize={}",
                        stage, req, p.len, p.hint, p.format_index, p.frame_index, p.frame_interval, p.max_video_frame_size, p.max_payload_transfer_size
                    );
                }
            }
            Err(e) => {
                serial_println!("[uvc] probe stage={} req={:#04x} host={} STALLED reason={} — sequence ended, NOT retried", stage, req, h.name(), e);
                return;
            }
        }
    }
    if blk_len < PROBE_LEN_MIN {
        serial_println!("[uvc] probe abort addr={} host={} reason=no-usable-block-length", addr, h.name());
        return;
    }
    {
        // SAFETY: `buf` valid for `cap` >= blk_len bytes; no transfer is in flight.
        let w = unsafe { core::slice::from_raw_parts_mut(buf, blk_len) };
        build_probe(w, fmt_i, frame_i, interval);
    }
    if let Err(e) = h.control(BMREQ_SET_INTF, RQ_SET_CUR, wvalue, windex, blk_len as u16, false) {
        serial_println!("[uvc] probe stage=set req={:#04x} host={} STALLED reason={} — sequence ended, NOT retried", RQ_SET_CUR, h.name(), e);
        return;
    }
    let mut commit = [0u8; PROBE_LEN_ASK as usize];
    let p = match h.control(BMREQ_GET_INTF, RQ_GET_CUR, wvalue, windex, blk_len as u16, true) {
        Ok(got) => {
            let got = (got as usize).min(blk_len);
            // SAFETY: as above; `got` <= blk_len <= cap.
            let b = unsafe { core::slice::from_raw_parts(buf as *const u8, got) };
            commit[..got].copy_from_slice(b);
            match parse_probe(b) {
                Some(p) => p,
                None => {
                    serial_println!("[uvc] probe stage=cur req={:#04x} host={} RUNT got={} need>={}", RQ_GET_CUR, h.name(), got, PROBE_LEN_MIN);
                    return;
                }
            }
        }
        Err(e) => {
            serial_println!("[uvc] probe stage=cur req={:#04x} host={} STALLED reason={}", RQ_GET_CUR, h.name(), e);
            return;
        }
    };
    serial_println!(
        "[uvc] probe negotiated host={} asked=({},{},{}) got=({},{},{}) agreed={} dwMaxVideoFrameSize={} dwMaxPayloadTransferSize={}",
        h.name(), fmt_i, frame_i, interval, p.format_index, p.frame_index, p.frame_interval,
        p.format_index == fmt_i && p.frame_index == frame_i && p.frame_interval == interval,
        p.max_video_frame_size, p.max_payload_transfer_size
    );
    // The alternate: the SMALLEST whose per-interval bytes cover dwMaxPayloadTransferSize (uvc.md §9.1 — the
    // bandwidth a camera reserves is the alternate's, so the smallest that fits is the polite one). A device
    // that names 0, or more than its widest alternate carries, gets the widest, and the line says which.
    let need = p.max_payload_transfer_size;
    let mut best: Option<usize> = None;
    let mut widest = 0usize;
    for i in 0..neps {
        let b = eps[i].esit_bytes();
        if b > eps[widest].esit_bytes() {
            widest = i;
        }
        if need != 0 && b >= need && best.map(|j| b < eps[j].esit_bytes()).unwrap_or(true) {
            best = Some(i);
        }
    }
    let why = if best.is_some() { "smallest-covering" } else if need == 0 { "widest(payload=0)" } else { "widest(payload-exceeds-all)" };
    let ep = eps[best.unwrap_or(widest)];
    crate::census_println!(
        "[uvc] alt-choice alt={} ep={:#04x} esit={} need={} why={} ss={} burst={} mult={} bpi={}",
        ep.alt, ep.addr, ep.esit_bytes(), need, why, ep.ss, ep.ss_burst as u32 + 1, ep.mult(), ep.ss_bpi
    );
    // Host first: Configure Endpoint changes nothing on the device, so a bandwidth refusal leaves the camera
    // exactly as negotiation left it — uncommitted.
    if let Err((code, name)) = h.iso_configure(&ep) {
        serial_println!("[uvc] commit=withheld reason=configure-endpoint-refused cc={} ({}) host={} alt={} esit={}", code, name, h.name(), ep.alt, ep.esit_bytes());
        return;
    }
    // COMMIT: the negotiated block exactly as GET_CUR returned it (UVC 1.1 §4.3.1.1: "the host shall issue a
    // SET_CUR(Commit) with the values returned by the last GET_CUR(Probe)").
    // SAFETY: `buf` valid for `cap` >= blk_len; no transfer in flight.
    unsafe { core::slice::from_raw_parts_mut(buf, blk_len) }.copy_from_slice(&commit[..blk_len]);
    let cvalue = (VS_COMMIT_CONTROL as u16) << 8;
    if let Err(e) = h.control(BMREQ_SET_INTF, RQ_SET_CUR, cvalue, windex, blk_len as u16, false) {
        h.iso_drop(&ep);
        serial_println!("[uvc] commit=failed stage=commit req=SET_CUR(VS_COMMIT_CONTROL) host={} reason={} — endpoint dropped", h.name(), e);
        return;
    }
    // SET_INTERFACE (USB 2.0 §9.4.10: bmRequestType 0x01, bRequest 11, wValue = alternate, wIndex = interface).
    if let Err(e) = h.control(0x01, 11, ep.alt as u16, ep.intf as u16, 0, false) {
        h.iso_drop(&ep);
        serial_println!("[uvc] commit=failed stage=set-interface alt={} intf={} host={} reason={} — endpoint dropped", ep.alt, ep.intf, h.name(), e);
        return;
    }
    let stores = stream_arm(addr, fw, fh, cc4, p.max_video_frame_size);
    match h.iso_start(&ep) {
        Ok((tds, lead, sched)) => {
            let cc = cc_str(u32::from_le_bytes(cc4));
            serial_println!(
                "[uvc] commit=armed pipe=iso ep={} mps={} mult={} interval={} alt={} esit={} payload={} host={} tds={} lead_uframes={} sched={} frame={}x{} fourcc={}{}{}{} frame_bytes={} store={}",
                ep.addr & 0x0F, ep.mps, ep.mult(), ep.interval, ep.alt, ep.esit_bytes(), need, h.name(), tds, lead, sched,
                fw, fh, cc[0], cc[1], cc[2], cc[3], p.max_video_frame_size, if stores { "double-buffer" } else { "count-only" }
            );
        }
        Err(e) => {
            let _ = h.control(0x01, 11, 0, ep.intf as u16, 0, false); // alternate 0: the device stops streaming
            h.iso_drop(&ep);
            stream_disarm();
            serial_println!("[uvc] commit=failed stage=ring-start host={} reason={} — alt 0 selected, endpoint dropped", h.name(), e);
        }
    }
}

/// The stream half of the self-test: `payload_header`, the `Assembler`, `iso_ep_of` and `first_function_len`,
/// each against bytes written here out of the spec tables, each check naming its field (the `selftest` shape).
fn stream_selftest() -> bool {
    let mut fails = 0u32;
    let mut first: (&'static str, u64, u64) = ("", 0, 0);
    let mut check = |name: &'static str, got: u64, want: u64| {
        if got != want {
            if fails == 0 {
                first = (name, got, want);
            }
            fails += 1;
        }
    };
    // Headers. P2 carries BOTH optional fields with distinct values, so a PTS/SCR offset mutation cannot alias.
    let p2: [u8; 14] = [12, BFH_EOH | BFH_SCR | BFH_PTS | BFH_EOF, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0x23, 0x01, b'E', b'F'];
    match payload_header(&p2) {
        Ok(h) => {
            check("hdr.len", h.len as u64, 12);
            check("hdr.fid", h.fid as u64, 0);
            check("hdr.eof", h.eof as u64, 1);
            check("hdr.eoh", h.eoh as u64, 1);
            check("hdr.pts", h.pts.unwrap_or(0) as u64, 0x1122_3344);
            check("hdr.scr.stc", h.scr.map(|s| s.0).unwrap_or(0) as u64, 0x5566_7788);
            check("hdr.scr.sof", h.scr.map(|s| s.1).unwrap_or(0) as u64, 0x0123);
        }
        Err(_) => check("hdr.parsed", 0, 1),
    }
    check("hdr.refuse.hlen_over", payload_header(&[13, BFH_EOH, 1, 2, 3]).is_err() as u64, 1);
    check("hdr.refuse.fields", payload_header(&[4, BFH_EOH | BFH_PTS, 1, 2, 3, 4]).is_err() as u64, 1);
    check("hdr.refuse.hlen1", payload_header(&[1, BFH_EOH]).is_err() as u64, 1);
    // The assembler: store 6, expect 6. Frame 1 = "ABCD" + "EF" closed by EOF (good); frame 2 = 3 bytes closed by
    // a toggle with no EOF (short); frame 3 = 6 bytes with ERR (errf, not published); frame 4 = 8 bytes (over).
    let mut a = Assembler::new(6, 6);
    let p1: [u8; 6] = [2, BFH_EOH, b'A', b'B', b'C', b'D'];
    let p3: [u8; 9] = [6, BFH_EOH | BFH_PTS | BFH_FID, 9, 9, 9, 9, b'x', b'y', b'z'];
    let p4: [u8; 3] = [2, BFH_EOH, b'q'];
    let p5: [u8; 7] = [2, BFH_EOH | BFH_ERR | BFH_EOF, 1, 2, 3, 4, 5];
    let p8: [u8; 10] = [2, BFH_EOH | BFH_FID, 1, 2, 3, 4, 5, 6, 7, 8];
    let p9: [u8; 2] = [2, BFH_EOH | BFH_FID | BFH_EOF];
    let bad: [u8; 5] = [13, BFH_EOH, 1, 2, 3];
    check("asm.p1.closed", a.push(&p1) as u64, 0);
    check("asm.p2.closed", a.push(&p2) as u64, 1);
    check("asm.f1.seq", a.seq as u64, 1);
    check("asm.f1.len", a.front_len as u64, 6);
    let f1 = a.front().iter().take(8).enumerate().fold(0u64, |acc, (i, b)| acc | ((*b as u64) << (8 * i)));
    check("asm.f1.bytes", f1, u64::from_le_bytes([b'A', b'B', b'C', b'D', b'E', b'F', 0, 0]));
    check("asm.empty.ignored", a.push(&[]) as u64, 0);
    check("asm.p3.closed", a.push(&p3) as u64, 0);
    check("asm.p4.closed", a.push(&p4) as u64, 1);
    check("asm.f2.len", a.front_len as u64, 3);
    check("asm.f2.byte0", a.front().first().copied().unwrap_or(0) as u64, b'x' as u64);
    check("asm.p5.closed", a.push(&p5) as u64, 1);
    check("asm.bad.closed", a.push(&bad) as u64, 0);
    check("asm.p8.closed", a.push(&p8) as u64, 0);
    check("asm.p9.closed", a.push(&p9) as u64, 1);
    check("asm.frames", a.frames as u64, 4);
    check("asm.good", a.good as u64, 1);
    check("asm.short", a.short as u64, 1);
    check("asm.errf", a.errf as u64, 1);
    check("asm.over", a.over as u64, 1);
    check("asm.bad_hdr", a.bad_hdr as u64, 1);
    check("asm.payloads", a.payloads as u64, 8);
    check("asm.bytes", a.bytes, 4 + 2 + 3 + 1 + 5 + 8);
    check("asm.seq", a.seq as u64, 2);
    // Endpoints out of the CAMERA1 fixture: alt 1 is 0x81 / 1024 x (1+1) / bInterval 1, alt 2 is 1024 x 3, alt 3
    // (the isochronous OUT) has no IN endpoint, and the whole fixture is one function.
    match iso_ep_of(&FIXTURE, 1, 1) {
        Some(e) => {
            check("ep.alt1.addr", e.addr as u64, 0x81);
            check("ep.alt1.mps", e.mps as u64, 1024);
            check("ep.alt1.extra", e.hs_extra as u64, 1);
            check("ep.alt1.interval", e.interval as u64, 1);
            check("ep.alt1.ss", e.ss as u64, 0);
            check("ep.alt1.esit", e.esit_bytes() as u64, 2048);
        }
        None => check("ep.alt1.found", 0, 1),
    }
    check("ep.alt2.esit", iso_ep_of(&FIXTURE, 1, 2).map(|e| e.esit_bytes()).unwrap_or(0) as u64, 3072);
    check("ep.alt3.out_refused", iso_ep_of(&FIXTURE, 1, 3).is_none() as u64, 1);
    check("fn.single", first_function_len(&FIXTURE) as u64, FIXTURE.len() as u64);
    // A SuperSpeed alternate: endpoint 0x82, mps 1024, bInterval 1, then its companion (bMaxBurst 15, Mult 2,
    // wBytesPerInterval 0xC000 = 49152) — and a second VideoStreaming function after it, where the cut falls.
    #[rustfmt::skip]
    let ss: [u8; 49] = [
        9, 0x02, 49, 0, 2, 1, 0, 0x80, 50,
        9, DT_INTERFACE, 1, 1, 1, CC_VIDEO, SC_VIDEOSTREAMING, 0, 0,
        7, DT_ENDPOINT, 0x82, 0x05, 0x00, 0x04, 1,
        6, DT_SS_EP_COMPANION, 15, 2, 0x00, 0xC0,
        9, DT_INTERFACE, 2, 0, 0, CC_VIDEO, SC_VIDEOSTREAMING, 0, 0,
        9, DT_INTERFACE, 2, 1, 0, CC_VIDEO, SC_VIDEOSTREAMING, 0, 0,
    ];
    match iso_ep_of(&ss, 1, 1) {
        Some(e) => {
            check("ss.addr", e.addr as u64, 0x82);
            check("ss.flag", e.ss as u64, 1);
            check("ss.burst", e.ss_burst as u64, 15);
            check("ss.mult", e.ss_mult as u64, 2);
            check("ss.bpi", e.ss_bpi as u64, 49_152);
            check("ss.esit", e.esit_bytes() as u64, 49_152);
        }
        None => check("ss.found", 0, 1),
    }
    check("fn.cut", first_function_len(&ss) as u64, 31);
    if fails == 0 {
        serial_println!(":: uvc: stream-selftest hdr=ok asm frames={} good={} short={} errf={} over={} ep=ok ss=ok cut=ok -> PASS ::", a.frames, a.good, a.short, a.errf, a.over);
        true
    } else {
        serial_println!(":: uvc: stream-selftest failures={} first={} got={} want={} -> FAIL ::", fails, first.0, first.1, first.2);
        false
    }
}

/// `tests uvc`, the stream half: frames over two seconds of wall clock from the armed xHCI stream, or SKIP by
/// name when there is none (QEMU's xHCI carries a UVC device only with a host webcam passed through).
fn stream_fixture(parsed: bool) {
    let streamed = stream_selftest();
    let ok = |b: bool| if b { "PASS" } else { "FAIL" };
    if !STREAM_ARMED.load(Ordering::Acquire) {
        // CPUID.01H:ECX bit 31 — set by a hypervisor for its guest (QEMU sets it on its x86 models).
        #[cfg(target_arch = "x86_64")] let vm = core::arch::x86_64::__cpuid(1).ecx & (1 << 31) != 0; #[cfg(not(target_arch = "x86_64"))] let vm = false; // ARMSONDE (SR101): CPUID is x86's; an aarch64 boot names the plain reason (QEMU raspi4b/virt carry no UVC device either way).
        let reason = if vm { "qemu-no-uvc-stream" } else { "no-xhci-uvc-stream" };
        if parsed && streamed {
            serial_println!(":: UVC: selftest=PASS stream_selftest=PASS host=- frames_2s=- fps=- -> SKIP reason={} ::", reason);
        } else {
            serial_println!(":: UVC: selftest={} stream_selftest={} host=- frames_2s=- fps=- -> FAIL ::", ok(parsed), ok(streamed));
        }
        return;
    }
    let snap = || STREAM.lock().as_ref().map(|a| (a.frames, a.good, a.short, a.errf, a.over, a.payloads, a.bytes, a.bad_hdr)).unwrap_or_default();
    let i0 = crate::drivers::xhci::isoch::stats();
    let s0 = snap();
    let t0 = crate::arch::now_cycles();
    let span = crate::arch::hw_wait_budget(); // 2 s of wall clock once the TSC is calibrated
    while crate::arch::now_cycles().wrapping_sub(t0) < span {
        #[cfg(target_arch = "x86_64")] crate::arch::sched::sleep_ticks(10); #[cfg(not(target_arch = "x86_64"))] { if let Ok(mut x) = crate::drivers::xhci::claim() { x.poll_events(); } crate::arch::sched::yield_now(); } // ARMSONDE (SR101): on aarch64 the window DRIVES the event ring itself — x86 has the 1 ms `xhci-poll` task to keep the iso ring harvested while this sleeps; the Orin's drive loop drains no sleepers (xusb_tegra `kbd_pump_body`'s JC3 note) and the `tests` verb runs on the very console pump that polls, so a sleep there would park forever and count nothing.
        core::hint::spin_loop();
    }
    let dt = crate::arch::now_cycles().wrapping_sub(t0).max(1);
    let s1 = snap();
    let i1 = crate::drivers::xhci::isoch::stats();
    let frames = s1.0.wrapping_sub(s0.0);
    #[cfg(target_arch = "x86_64")] let hz = crate::arch::apic::tsc_hz(); #[cfg(target_arch = "aarch64")] let hz = crate::arch::timer::cntfrq(); // ARMSONDE (SR101): `now_cycles` is CNTVCT on aarch64, which counts at CNTFRQ_EL0 (31.25 MHz Orin, 54 MHz Pi 4).
    let fps10 = if hz != 0 { (frames as u64).saturating_mul(10).saturating_mul(hz) / dt } else { frames as u64 * 5 };
    let wh = STREAM_WH.load(Ordering::Relaxed);
    let cc = cc_str(STREAM_CC.load(Ordering::Relaxed));
    let pass = parsed && streamed && frames > 0;
    serial_println!(
        ":: UVC: selftest={} stream_selftest={} host=xhci addr={} {}x{} fourcc={}{}{}{} frames_2s={} fps={}.{} good={} short={} err={} over={} payloads={} bytes={} bad_hdr={} tds={} missed={} overrun={} xact={} dup={} resync={} sched={} -> {}{} ::",
        ok(parsed), ok(streamed), STREAM_ADDR.load(Ordering::Relaxed), wh >> 16, wh & 0xFFFF, cc[0], cc[1], cc[2], cc[3],
        frames, fps10 / 10, fps10 % 10, s1.1.wrapping_sub(s0.1), s1.2.wrapping_sub(s0.2), s1.3.wrapping_sub(s0.3),
        s1.4.wrapping_sub(s0.4), s1.5.wrapping_sub(s0.5), s1.6.wrapping_sub(s0.6), s1.7.wrapping_sub(s0.7),
        i1.tds.wrapping_sub(i0.tds), i1.missed.wrapping_sub(i0.missed), i1.overrun.wrapping_sub(i0.overrun),
        i1.xact.wrapping_sub(i0.xact), i1.dup.wrapping_sub(i0.dup), i1.resync.wrapping_sub(i0.resync),
        if i1.sia { "sia" } else { "frameid" },
        if pass { "PASS" } else { "FAIL" }, if pass || frames != 0 { "" } else { " reason=no-frames" }
    );
}

/// QUIETBOOT3 (rmbp-ledger B352, R80) — the parser self-test as a `tests uvc` fixture (the boot no longer runs it).
fn uvc_fixture() {
    let parsed = selftest(); stream_fixture(parsed); // XHCIISO (rmbp-ledger B534): the stream half — its own self-test, then frames_2s from the armed xHCI stream, or SKIP by name
}

// ── REALSENSE (LEDGER SR92, R112): the multi-interface census and the D400's XU reads ───────────────
//
// APPENDED, and `#[cfg(feature = "sonde")]`-erased whole, so no line above moves and a knob-off image
// (sonde off, uvc on or off) carries this file's pre-REALSENSE text byte for byte. Reached from `probe`
// by two LINE-NEUTRAL folds (the device-descriptor read on the `wtotal` line, the census after
// `parse(full)`), each statement BEFORE its line's first `//`.
//
// WHY A SECOND WALK: `parse` above is the CAMERA1 census of ONE streaming function — `vs_intf` is
// overwritten by every VideoStreaming interface it meets and every format of every interface lands in
// one `fmts` list. A D435i is THREE video functions behind one IAD (depth Z16, infrared Y8, colour YUY2 —
// librealsense `src/ds/d400/d400-factory.cpp:695-697` names the three streams of the D435i's profile) and
// a HID interface for the IMU, and `parse` cannot tell them apart. `multi::census` walks the same bytes
// keeping one record PER VideoStreaming interface number, plus every extension unit (UVC 1.1 §3.7.2.6,
// Table 3-9) and the HID interface. `parse` and its self-test are untouched.
#[cfg(feature = "sonde")]
pub mod multi {
    use super::{le16, le32, CC_VIDEO, CS_INTERFACE, DT_INTERFACE, SC_VIDEOCONTROL, SC_VIDEOSTREAMING};
    use super::{VS_FORMAT_MJPEG, VS_FORMAT_UNCOMPRESSED, VS_FRAME_MJPEG, VS_FRAME_UNCOMPRESSED, VS_INPUT_HEADER, Xfer};
    use crate::drivers::realsense_xu as rs;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

    /// UVC 1.1 Appendix A.5: `VC_EXTENSION_UNIT` (librealsense's backend names the same code,
    /// `src/uvc/uvc-types.h:103`).
    const VC_EXTENSION_UNIT: u8 = 0x06;
    /// USB HID class (HID 1.11 §4.1).
    const CC_HID: u8 = 0x03;

    pub const MAX_VS: usize = 4;
    pub const MAX_XU: usize = 4;

    /// The role a streaming function plays, from its first format's FourCC (librealsense
    /// `src/ds/d400/d400-device.cpp:55-75`: `Z16 ` → Z16/DEPTH, `GREY`/`Y8  ` → Y8/INFRARED,
    /// `YUY2` → YUYV/COLOR; `Y16 ` → Y16 is the infrared imagers' 16-bit form).
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Role {
        Depth,
        Ir,
        Color,
        Unknown,
    }

    /// SONDE2 (LEDGER SR105): the classification is the shared core's (`probe_core::StreamKind::of_uvc_fourcc`,
    /// the table handlers/sonde reads); this file keeps only the descriptor walk, which is the bus's.
    pub fn role_of(cc: [u8; 4]) -> Role {
        use probe_core::StreamKind as K;
        match K::of_uvc_fourcc(cc) {
            Some(K::Depth) => Role::Depth,
            Some(K::Infrared) => Role::Ir,
            Some(K::Color) => Role::Color,
            _ => Role::Unknown,
        }
    }

    /// SONDE2 (LEDGER SR105): `probe_iso`'s fold on its `let c` line. When the full window's census holds a
    /// DEPTH function that is not the one `c` describes, return `parse` of that function's own run of
    /// descriptors — from its first interface descriptor to the next interface descriptor with another
    /// number (USB 2.0 §9.6.5: every alternate of one interface follows it contiguously) — so the
    /// negotiation, the alternate choice and the commit below are the depth function's. `parse` needs no
    /// configuration or VideoControl descriptor in front (it keys each arm on the interface it last saw).
    /// Otherwise `c`, untouched: a single-function webcam streams exactly as before.
    pub fn depth_fn(full: &[u8], c: super::Census) -> super::Census {
        let s = census(full);
        let Some(d) = s.find(Role::Depth) else { return c };
        if c.have_vs && c.vs_intf == d.intf {
            return c;
        }
        let (mut off, mut start, mut end) = (0usize, None, full.len());
        while off + 2 <= full.len() {
            let len = full[off] as usize;
            if len < 2 || off + len > full.len() {
                end = off;
                break;
            }
            if full[off + 1] == DT_INTERFACE && len >= 9 {
                match start {
                    None if full[off + 2] == d.intf => start = Some(off),
                    Some(_) if full[off + 2] != d.intf => {
                        end = off;
                        break;
                    }
                    _ => {}
                }
            }
            off += len;
        }
        let Some(st) = start else { return c };
        let p = super::parse(&full[st..end]);
        if !p.have_vs || p.vs_intf != d.intf {
            return c;
        }
        crate::census_println!("[sonde] stream fn intf={} role=depth window={}..{} of {} (was intf={})", d.intf, st, end, full.len(), c.vs_intf);
        p
    }

    /// SONDE2 (LEDGER SR105): `probe_iso`'s fold on its format loop — skip every format but `Z16 ` in a
    /// function that declares one, so the first-uncompressed rule below lands on the depth format.
    pub fn skip_for_z16(c: &super::Census, f: &super::Fmt) -> bool {
        let z = |g: &[u8; 16]| g[..4] == *b"Z16 ";
        c.fmts[..c.nfmt].iter().any(|x| z(&x.guid)) && !z(&f.guid)
    }

    pub fn role_name(r: Role) -> &'static str {
        match r {
            Role::Depth => "depth",
            Role::Ir => "ir",
            Role::Color => "color",
            Role::Unknown => "other",
        }
    }

    /// One VideoStreaming function (all alternates of one interface number).
    #[derive(Clone, Copy, Default)]
    pub struct VsFn {
        pub intf: u8,
        pub ep: u8,
        pub terminal_link: u8,
        /// Formats declared in the input header / seen walking.
        pub declared_formats: u8,
        pub nfmt: u8,
        /// The FIRST format's FourCC (MJPEG: `MJPG`), bpp and first frame's size.
        pub fourcc: [u8; 4],
        pub bpp: u8,
        pub width: u16,
        pub height: u16,
        pub default_interval: u32,
    }

    impl VsFn {
        pub fn role(&self) -> Role {
            role_of(self.fourcc)
        }
    }

    /// One extension unit.
    #[derive(Clone, Copy, Default)]
    pub struct Xu {
        pub id: u8,
        pub guid: [u8; 16],
        pub ncontrols: u8,
    }

    impl Xu {
        pub fn is_d400_depth(&self) -> bool {
            self.guid == rs::DEPTH_XU_GUID
        }
    }

    #[derive(Clone, Copy, Default)]
    pub struct Streams {
        pub have_vc: bool,
        pub vc_intf: u8,
        pub nvs: usize,
        pub vs: [VsFn; MAX_VS],
        pub nxu: usize,
        pub xus: [Xu; MAX_XU],
        pub hid_intf: Option<u8>,
        pub ran_short: bool,
        pub overflowed: bool,
    }

    impl Streams {
        /// The stream roles in interface order, as `depth,ir,color` (≤ 4 names, `-` when none).
        pub fn label(&self, out: &mut [u8; 32]) -> usize {
            let mut n = 0usize;
            for f in self.vs[..self.nvs].iter() {
                let name = role_name(f.role()).as_bytes();
                if n != 0 && n < out.len() {
                    out[n] = b',';
                    n += 1;
                }
                for &b in name {
                    if n < out.len() {
                        out[n] = b;
                        n += 1;
                    }
                }
            }
            if n == 0 {
                out[0] = b'-';
                n = 1;
            }
            n
        }

        /// The first function playing `role`.
        pub fn find(&self, role: Role) -> Option<&VsFn> {
            self.vs[..self.nvs].iter().find(|f| f.role() == role)
        }

        pub fn depth_xu(&self) -> Option<&Xu> {
            self.xus[..self.nxu].iter().find(|x| x.is_d400_depth())
        }
    }

    /// THE MULTI-INTERFACE WALK. Pure and total, the same descriptor walk as `parse` (USB 2.0 §9.5),
    /// with the class-specific arms keyed by the interface descriptor most recently seen.
    pub fn census(cfg: &[u8]) -> Streams {
        let mut s = Streams::default();
        let mut off = 0usize;
        let (mut in_vc, mut cur): (bool, Option<usize>) = (false, None);
        // Within the current VS function: the first format's index (frames after it are its frames).
        let mut first_fmt_open = false;
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            if len < 2 {
                break;
            }
            if off + len > cfg.len() {
                s.ran_short = true;
                break;
            }
            let d = &cfg[off..off + len];
            match d[1] {
                // USB 2.0 Table 9-12: bInterfaceNumber[2] bInterfaceClass[5] bInterfaceSubClass[6].
                DT_INTERFACE if len >= 9 => {
                    in_vc = d[5] == CC_VIDEO && d[6] == SC_VIDEOCONTROL;
                    if in_vc && !s.have_vc {
                        s.have_vc = true;
                        s.vc_intf = d[2];
                    }
                    if d[5] == CC_HID && s.hid_intf.is_none() {
                        s.hid_intf = Some(d[2]);
                    }
                    if d[5] == CC_VIDEO && d[6] == SC_VIDEOSTREAMING {
                        // A new interface NUMBER is a new function; an alternate setting of the
                        // current one (same number, alt > 0) is not.
                        let same = cur.map(|i| s.vs[i].intf == d[2]).unwrap_or(false);
                        if !same {
                            if s.nvs < MAX_VS {
                                s.vs[s.nvs] = VsFn { intf: d[2], ..VsFn::default() };
                                cur = Some(s.nvs);
                                s.nvs += 1;
                                first_fmt_open = false;
                            } else {
                                s.overflowed = true;
                                cur = None;
                            }
                        }
                    } else {
                        cur = None;
                    }
                }
                // UVC 1.1 §3.7.2.6, Table 3-9: bUnitID[3] guidExtensionCode[4..20] bNumControls[20].
                CS_INTERFACE if in_vc && len >= 21 && d[2] == VC_EXTENSION_UNIT => {
                    if s.nxu < MAX_XU {
                        let mut x = Xu { id: d[3], ncontrols: d[20], ..Xu::default() };
                        x.guid.copy_from_slice(&d[4..20]);
                        s.xus[s.nxu] = x;
                        s.nxu += 1;
                    } else {
                        s.overflowed = true;
                    }
                }
                CS_INTERFACE if len >= 3 && cur.is_some() => {
                    let f = &mut s.vs[cur.unwrap_or(0)];
                    match d[2] {
                        // UVC 1.1 Table 3-13: bNumFormats[3] bEndpointAddress[6] bTerminalLink[8].
                        VS_INPUT_HEADER if len >= 9 => {
                            f.declared_formats = d[3];
                            f.ep = d[6];
                            f.terminal_link = d[8];
                        }
                        // Uncompressed payload Table 3-1: guidFormat[5..21] bBitsPerPixel[21].
                        VS_FORMAT_UNCOMPRESSED if len >= 23 => {
                            if f.nfmt == 0 {
                                f.fourcc.copy_from_slice(&d[5..9]);
                                f.bpp = d[21];
                                first_fmt_open = true;
                            } else {
                                first_fmt_open = false;
                            }
                            f.nfmt = f.nfmt.saturating_add(1);
                        }
                        VS_FORMAT_MJPEG if len >= 7 => {
                            if f.nfmt == 0 {
                                f.fourcc = *b"MJPG";
                                first_fmt_open = true;
                            } else {
                                first_fmt_open = false;
                            }
                            f.nfmt = f.nfmt.saturating_add(1);
                        }
                        // Uncompressed/MJPEG payload Table 3-2 (identical): wWidth[5..7] wHeight[7..9]
                        // dwDefaultFrameInterval[21..25]. The FIRST frame of the FIRST format only.
                        VS_FRAME_UNCOMPRESSED | VS_FRAME_MJPEG if len >= 25 => {
                            if first_fmt_open && f.width == 0 {
                                f.width = le16(d, 5);
                                f.height = le16(d, 7);
                                f.default_interval = le32(d, 21);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            off += len;
        }
        s
    }

    // ── The record `tests sonde` reads (atomics: written once per boot at enumeration) ────────────
    static VIDPID: AtomicU32 = AtomicU32::new(0);
    static ADDR: AtomicU8 = AtomicU8::new(0);
    static ROLES: AtomicU32 = AtomicU32::new(0); // bit 0 depth, 1 ir, 2 color
    static DEPTH_WH: AtomicU32 = AtomicU32::new(0);
    static DEPTH_CC: AtomicU32 = AtomicU32::new(0);
    static XU_UNIT: AtomicU8 = AtomicU8::new(0);
    static RAN_SHORT: AtomicBool = AtomicBool::new(false);
    /// Per `rs::XU_READS` entry: the value read, or `NOT_READ` / `STALLED`.
    static XU_VAL: [AtomicU32; 3] = [AtomicU32::new(NOT_READ), AtomicU32::new(NOT_READ), AtomicU32::new(NOT_READ)];
    pub const NOT_READ: u32 = u32::MAX;
    pub const STALLED: u32 = u32::MAX - 1;
    /// The device descriptor of the candidate being walked (set by `ident`, consumed by `after_census`).
    static PENDING: AtomicU32 = AtomicU32::new(0);

    /// What the boot saw, for `tests sonde`.
    #[derive(Clone, Copy)]
    pub struct Record {
        pub vid: u16,
        pub pid: u16,
        pub addr: u8,
        pub roles: u32,
        pub depth_w: u16,
        pub depth_h: u16,
        pub depth_cc: [u8; 4],
        pub xu_unit: u8,
        pub ran_short: bool,
        pub xu: [u32; 3],
    }

    pub fn record() -> Option<Record> {
        let vp = VIDPID.load(Ordering::Acquire);
        if vp == 0 {
            return None;
        }
        let wh = DEPTH_WH.load(Ordering::Relaxed);
        Some(Record {
            vid: (vp >> 16) as u16,
            pid: vp as u16,
            addr: ADDR.load(Ordering::Relaxed),
            roles: ROLES.load(Ordering::Relaxed),
            depth_w: (wh >> 16) as u16,
            depth_h: wh as u16,
            depth_cc: DEPTH_CC.load(Ordering::Relaxed).to_le_bytes(),
            xu_unit: XU_UNIT.load(Ordering::Relaxed),
            ran_short: RAN_SHORT.load(Ordering::Relaxed),
            xu: [XU_VAL[0].load(Ordering::Relaxed), XU_VAL[1].load(Ordering::Relaxed), XU_VAL[2].load(Ordering::Relaxed)],
        })
    }

    /// FOLD 1 (the `wtotal` line of `probe`): one `GET_DESCRIPTOR(DEVICE)` for a video-IAD candidate, so
    /// the census knows WHOSE camera it is walking. USB 2.0 §9.4.3 / Table 9-8: bmRequestType 0x80,
    /// bRequest 6, wValue 0x0100, 18 bytes; idVendor @8, idProduct @10. ONE attempt; on failure the
    /// device is walked as an unknown camera (the CAMERA1 path is unchanged either way).
    pub fn ident(addr: u8, buf: *mut u8, xfer: Xfer) {
        PENDING.store(0, Ordering::Relaxed);
        match xfer(0x80, 6, 0x0100, 0, 18, true) {
            Ok(n) if n >= 12 => {
                // SAFETY: `buf` is the EP0 data buffer `probe`'s contract names (256 bytes, live for the
                // controller `xfer` drives) and the transfer above just wrote `n` >= 12 bytes into it.
                let b = unsafe { core::slice::from_raw_parts(buf as *const u8, 12) };
                let (vid, pid) = (le16(b, 8), le16(b, 10));
                PENDING.store(((vid as u32) << 16) | pid as u32, Ordering::Relaxed);
                crate::census_println!("[sonde] ident addr={} dev={:04x}:{:04x} d400={}", addr, vid, pid, rs::is_d400(vid, pid));
            }
            Ok(n) => serial_println!("[sonde] ident addr={} req=GET_DESCRIPTOR(DEVICE) got={} reason=runt", addr, n),
            Err(e) => serial_println!("[sonde] ident addr={} req=GET_DESCRIPTOR(DEVICE) reason={}", addr, e),
        }
    }

    /// FOLD 2 (after `parse(full)` in `probe`): the multi-interface census of the same window, printed, and for a
    /// D400 the record `tests sonde` reads, the three XU `GET_CUR`s and (SONDE3, SR108) the HW-monitor depth scale
    /// when `cap` — the EP0 buffer's size: 256 from EHCI, 4 KiB from xHCI — carries it. `full` aliases `buf`.
    pub fn after_census(addr: u8, config_value: u8, full: &[u8], buf: *mut u8, cap: usize, xfer: Xfer) {
        let s = census(full);
        let vp = PENDING.load(Ordering::Relaxed);
        let (vid, pid) = ((vp >> 16) as u16, vp as u16);
        let mut lbl = [0u8; 32];
        let n = s.label(&mut lbl);
        crate::census_println!(
            "[sonde] census addr={} dev={:04x}:{:04x} vc_intf={} fns={} streams={} xus={} hid_intf={} ran_short={} overflowed={}",
            addr, vid, pid, s.vc_intf, s.nvs, core::str::from_utf8(&lbl[..n]).unwrap_or("?"), s.nxu,
            s.hid_intf.map(|i| i as i32).unwrap_or(-1), s.ran_short, s.overflowed
        );
        for f in s.vs[..s.nvs].iter() {
            let cc = super::fourcc(&{
                let mut g = [0u8; 16];
                g[..4].copy_from_slice(&f.fourcc);
                g
            });
            crate::census_println!(
                "[sonde] fn intf={} ep={:#04x} role={} fourcc={}{}{}{} bpp={} {}x{} default_us={} formats={}/{}",
                f.intf, f.ep, role_name(f.role()), cc[0], cc[1], cc[2], cc[3], f.bpp, f.width, f.height,
                f.default_interval / 10, f.nfmt, f.declared_formats
            );
        }
        for x in s.xus[..s.nxu].iter() {
            crate::census_println!(
                "[sonde] xu id={} controls={} guid0={:02x}{:02x}{:02x}{:02x} d400_depth={}",
                x.id, x.ncontrols, x.guid[3], x.guid[2], x.guid[1], x.guid[0], x.is_d400_depth()
            );
        }
        if !rs::is_d400(vid, pid) {
            return;
        }

        // A D400: the record first (all from the census, no traffic), then the reads.
        let mut roles = 0u32;
        for f in s.vs[..s.nvs].iter() {
            roles |= match f.role() {
                Role::Depth => 1,
                Role::Ir => 2,
                Role::Color => 4,
                Role::Unknown => 0,
            };
        }
        if let Some(d) = s.find(Role::Depth) {
            DEPTH_WH.store(((d.width as u32) << 16) | d.height as u32, Ordering::Relaxed);
            DEPTH_CC.store(u32::from_le_bytes(d.fourcc), Ordering::Relaxed);
        }
        ROLES.store(roles, Ordering::Relaxed);
        ADDR.store(addr, Ordering::Relaxed);
        RAN_SHORT.store(s.ran_short, Ordering::Relaxed);
        // The unit: the census's depth XU when the window reached it, else the firmware's fixed id
        // (`ds-private.h:66-67` — unit 3 is the firmware's fact, not the host's).
        let unit = s.depth_xu().map(|x| x.id).unwrap_or(rs::DEPTH_XU_UNIT);
        XU_UNIT.store(unit, Ordering::Relaxed);
        VIDPID.store(vp, Ordering::Release);

        // SET_CONFIGURATION — the standard request that makes interface requests legal (USB 2.0
        // §9.4.7); CAMERA1's probe sends the same one with the same value a few lines later, which is
        // legal and idempotent (§9.4.7: the device stays configured).
        if xfer(0x00, 9, config_value as u16, 0, 0, false).is_err() {
            serial_println!("[sonde] xu abort addr={} stage=set-configuration reason=stalled-or-timeout", addr);
            return;
        }
        let vc = if s.have_vc { s.vc_intf } else { 0 };
        for (i, r) in rs::XU_READS.iter().enumerate() {
            let (wv, wi) = (rs::xu_wvalue(r.ctrl), rs::xu_windex(unit, vc));
            match xfer(0xA1, 0x81, wv, wi, r.len, true) {
                Ok(got) if got as u16 >= r.len => {
                    // SAFETY: as in `ident` — the live EP0 buffer, `got` >= r.len (<= 2) bytes written.
                    let b = unsafe { core::slice::from_raw_parts(buf as *const u8, r.len as usize) };
                    let v = if r.len == 2 { le16(b, 0) as u32 } else { b[0] as u32 };
                    XU_VAL[i].store(v, Ordering::Relaxed);
                    crate::census_println!("[sonde] xu read={} unit={} ctrl={} wValue={:#06x} wIndex={:#06x} value={}", r.name, unit, r.ctrl, wv, wi, v);
                }
                Ok(got) => {
                    XU_VAL[i].store(STALLED, Ordering::Relaxed);
                    serial_println!("[sonde] xu read={} unit={} ctrl={} got={} need={} reason=runt", r.name, unit, r.ctrl, got, r.len);
                }
                Err(e) => {
                    XU_VAL[i].store(STALLED, Ordering::Relaxed);
                    serial_println!("[sonde] xu read={} unit={} ctrl={} STALLED reason={} — NOT retried", r.name, unit, r.ctrl, e);
                }
            }
        }
        // The depth scale's own read is the HW-monitor GET_ADV(etDepthTableControl) — a 1024-byte XU
        // SET_CUR/GET_CUR pair (realsense_xu::hwmon_*). SONDE3 (SR108): sent when `cap` carries it (the
        // xHCI 4 KiB EP0); EHCI's 256-byte buffer keeps `[sonde] hwmon=withheld reason=ep0-buf-256-lt-1024`.
        super::hwmon::depth_units(addr, unit, vc, buf, cap, xfer);
        // (The exchange lives in `super::hwmon`, appended at the file tail, so this module keeps its line
        // count and nothing below it — `ensure_tests` is compiled with `uvc` alone — moves.)
    }
}

/// ARMSONDE (LEDGER SR101) — `tests uvc` registered whether or not a camera was offered. On x86 the EHCI walk
/// calls `probe` for every non-hub device, so the fixture is always deferred there; an aarch64 board has no EHCI,
/// and `probe_iso` runs only for a video-IAD candidate, so with no camera plugged in `tests uvc` was "no such
/// fixture" instead of the parser PASS and the stream half's `-> SKIP reason=no-xhci-uvc-stream`. `defer`
/// de-duplicates by name, so the later `probe_iso` deferral is a no-op. Called from `tests::ensure_shellux`
/// (aarch64 only, so the x86 fixture table is unchanged). Appended at the file tail: no line above moves.
pub fn ensure_tests() {
    let _ = crate::tests::defer("uvc", uvc_fixture);
}

// ── SONDE2 (LEDGER SR105): the depth tap — the armed stream as `tests sonde` and `/dev/probe/depth0` read it ──
//
// APPENDED at the file tail and `#[cfg(feature = "sonde")]`-erased whole: no line above moves, and a knob-off
// image (sonde off, uvc on or off) carries this file's text exactly as before. A child module of this file,
// so it reads the stream's private statics without one of them changing visibility.
#[cfg(feature = "sonde")]
pub mod depth_tap {
    use super::{Ordering, STREAM, STREAM_ADDR, STREAM_ARMED, STREAM_CC, STREAM_WH};

    /// What the armed xHCI stream is: (device address, width, height, FourCC). `None` = nothing armed.
    pub fn armed() -> Option<(u8, u16, u16, [u8; 4])> {
        if !STREAM_ARMED.load(Ordering::Acquire) {
            return None;
        }
        let wh = STREAM_WH.load(Ordering::Relaxed);
        Some((STREAM_ADDR.load(Ordering::Relaxed) as u8, (wh >> 16) as u16, wh as u16, STREAM_CC.load(Ordering::Relaxed).to_le_bytes()))
    }

    /// The assembler's counters: (frames closed, good, short, error, over, payloads, bad headers).
    pub fn counts() -> (u32, u32, u32, u32, u32, u32, u32) {
        STREAM.lock().as_ref().map(|a| (a.frames, a.good, a.short, a.errf, a.over, a.payloads, a.bad_hdr)).unwrap_or_default()
    }

    /// SONDE3 (SR108): the latest complete frame's sequence number (0 = none yet) — a peek, nothing copied, so
    /// the continuous tap can decide whether a pass has work before it touches a frame.
    pub fn latest_seq() -> u32 {
        STREAM.lock().as_ref().map(|a| a.seq).unwrap_or(0)
    }

    /// The latest complete frame: its sequence number and length, and its bytes COPIED into `out` when they
    /// fit `out`'s capacity (cleared first; the caller reserves, so the copy under the stream lock allocates
    /// nothing — a frame that does not fit leaves `out` empty and the length says why). `None` = no frame yet.
    pub fn copy_latest(out: &mut alloc::vec::Vec<u8>) -> Option<(u32, usize)> {
        out.clear();
        let mut got = None;
        super::latest_frame(&mut |b, s| {
            if out.capacity() >= b.len() {
                out.extend_from_slice(b);
            }
            got = Some((s, b.len()));
        });
        got
    }
}

// ── SONDE3 (LEDGER SR108): the camera's own depth scale over the HW-monitor channel ──────────────────────────────
//
// APPENDED at the file tail and `#[cfg(feature = "sonde")]`-erased whole, like `depth_tap`: no line above moves.
#[cfg(feature = "sonde")]
pub mod hwmon {
    use super::{Ordering, Xfer};
    use crate::drivers::realsense_xu as rs;
    use core::sync::atomic::AtomicU32;

    /// The depth scale the camera reported, micrometres per Z16 unit; 0 = not read (withheld on EHCI, or the
    /// exchange failed — its `[sonde] hwmon=` line names the stage). Written once per D400 census.
    static DEPTH_UM: AtomicU32 = AtomicU32::new(0);

    /// The camera's depth units (µm), or 0 when this boot did not read them. `sonde` falls back to
    /// `realsense_xu::DEPTH_UNITS_DEFAULT_UM` and marks the line `(default)`.
    pub fn depth_um() -> u32 {
        DEPTH_UM.load(Ordering::Relaxed)
    }

    /// Called at the end of `multi::after_census` for a D400 (after SET_CONFIGURATION and the three XU reads).
    ///
    /// librealsense's `uvc-option.cpp:132-151` shape: `SET_CUR(DS5_HWMONITOR)` carrying the 1024-byte buffer whose
    /// head is the `GET_ADV(etDepthTableControl, GET_VAL)` command (`ds-options.cpp:336-341`, laid out by
    /// `realsense_xu::hwmon_build_depth_units`), then `GET_CUR(DS5_HWMONITOR)` of 1024 bytes, read by
    /// `realsense_xu::hwmon_reply_depth_units` (opcode echo at 0, data size at 1020, `depth_units` the table's
    /// first u32). ONE attempt each, never retried; a failure names its stage and the scale stays 0. The command
    /// READS the depth table — the SET_CUR is the channel's write half, not a change to the device. `buf` is the
    /// live EP0 data buffer of the controller `xfer` drives, valid for `cap` bytes.
    pub fn depth_units(addr: u8, unit: u8, vc: u8, buf: *mut u8, cap: usize, xfer: Xfer) {
        DEPTH_UM.store(0, Ordering::Relaxed);
        if !rs::hwmon_fits(cap) {
            serial_println!(
                "[sonde] hwmon=withheld reason=ep0-buf-{}-lt-{} depth_scale=default({} um, d400-device.cpp:912)",
                cap, rs::HWMON_BUF, rs::DEPTH_UNITS_DEFAULT_UM
            );
            return;
        }
        let (wv, wi) = (rs::xu_wvalue(rs::CTRL_HWMONITOR), rs::xu_windex(unit, vc));
        let len = rs::HWMON_BUF as u16;
        let built = {
            // SAFETY: `buf` holds `cap` >= HWMON_BUF bytes (checked above) and no transfer is in flight while this
            // slice lives; it ends before the SET_CUR below hands the buffer to the controller.
            let b = unsafe { core::slice::from_raw_parts_mut(buf, rs::HWMON_BUF) };
            rs::hwmon_build_depth_units(b)
        };
        if built == 0 {
            serial_println!("[sonde] hwmon=failed addr={} stage=build reason=buffer-short depth_scale=default({} um)", addr, rs::DEPTH_UNITS_DEFAULT_UM);
            return;
        }
        if let Err(e) = xfer(rs::BM_CLASS_INTF_OUT, rs::RQ_SET_CUR, wv, wi, len, false) {
            serial_println!(
                "[sonde] hwmon=failed addr={} stage=set-cur unit={} wValue={:#06x} wIndex={:#06x} len={} reason={} depth_scale=default({} um) — NOT retried",
                addr, unit, wv, wi, len, e, rs::DEPTH_UNITS_DEFAULT_UM
            );
            return;
        }
        let got = match xfer(rs::BM_CLASS_INTF_IN, rs::RQ_GET_CUR, wv, wi, len, true) {
            Ok(got) => got as usize,
            Err(e) => {
                serial_println!(
                    "[sonde] hwmon=failed addr={} stage=get-cur unit={} len={} reason={} depth_scale=default({} um) — NOT retried",
                    addr, unit, len, e, rs::DEPTH_UNITS_DEFAULT_UM
                );
                return;
            }
        };
        if got < rs::HWMON_BUF {
            serial_println!("[sonde] hwmon=failed addr={} stage=get-cur got={} need={} reason=runt depth_scale=default({} um)", addr, got, rs::HWMON_BUF, rs::DEPTH_UNITS_DEFAULT_UM);
            return;
        }
        // SAFETY: the GET_CUR just wrote `got` >= HWMON_BUF bytes into the live EP0 buffer; the next transfer is
        // the caller's, after this function returns.
        let r = unsafe { core::slice::from_raw_parts(buf as *const u8, rs::HWMON_BUF) };
        let le32 = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
        let (echo, size) = (le32(0), le32(rs::HWMON_DATA_SIZE_OFFSET));
        match rs::hwmon_reply_depth_units(r) {
            Some(um) if um != 0 => {
                DEPTH_UM.store(um, Ordering::Relaxed);
                crate::census_println!(
                    "[sonde] hwmon op=GET_ADV(etDepthTableControl) unit={} set_cur=ok get_cur={} echo={:#x} size={} depth_units={} -> depth_scale={}.{:06}(device)",
                    unit, got, echo, size, um, um / 1_000_000, um % 1_000_000
                );
            }
            Some(_) => serial_println!(
                "[sonde] hwmon=failed addr={} stage=reply echo={:#x} size={} reason=depth-units-zero depth_scale=default({} um)",
                addr, echo, size, rs::DEPTH_UNITS_DEFAULT_UM
            ),
            None => serial_println!(
                "[sonde] hwmon=failed addr={} stage=reply echo={:#x} want_echo={:#x} size={} reason={} depth_scale=default({} um)",
                addr, echo, rs::OP_GET_ADV, size, if echo != rs::OP_GET_ADV as u32 { "error-echo" } else { "short-table" }, rs::DEPTH_UNITS_DEFAULT_UM
            ),
        }
    }
}
