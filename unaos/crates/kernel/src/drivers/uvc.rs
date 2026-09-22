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
    serial_println!(
        "[uvc] vc ctrl={} addr={} intf={} bcdUVC={:#06x} clock_hz={} vc_total_len={} in_collection={} iad first={} count={} sub={:#04x}",
        idx, addr, c.vc_intf, c.bcd_uvc, c.clock_hz, c.vc_total_len, c.in_collection,
        c.iad_first, c.iad_count, c.iad_subclass
    );
    if c.have_it {
        serial_println!(
            "[uvc] vc term=input id={} type={:#06x}{} ctrl_len={} ctrl={:#010x}",
            c.it_id, c.it_type,
            if c.it_type == ITT_CAMERA { " (camera)" } else { "" },
            c.it_ctrl_len, c.it_ctrl
        );
    }
    if c.have_pu {
        serial_println!(
            "[uvc] vc unit=processing id={} src={} ctrl_len={} ctrl={:#010x}",
            c.pu_id, c.pu_src, c.pu_ctrl_len, c.pu_ctrl
        );
    }
    if c.have_ot {
        serial_println!(
            "[uvc] vc term=output id={} type={:#06x} src={}",
            c.ot_id, c.ot_type, c.ot_src
        );
    }
    serial_println!(
        "[uvc] vs intf={} ep={:#04x} terminal_link={} formats_declared={} formats_seen={}",
        c.vs_intf, c.vs_ep, c.vs_terminal_link, c.declared_formats, c.nfmt
    );
    for f in c.fmts[..c.nfmt].iter() {
        let cc = fourcc(&f.guid);
        serial_println!(
            "[uvc] vs fmt={} kind={} fourcc={}{}{}{} guid={:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} bpp={} default_frame={} frames={}/{}",
            f.index, kind_name(f.kind), cc[0], cc[1], cc[2], cc[3],
            f.guid[0], f.guid[1], f.guid[2], f.guid[3], f.guid[4], f.guid[5], f.guid[6], f.guid[7],
            f.guid[8], f.guid[9], f.guid[10], f.guid[11], f.guid[12], f.guid[13], f.guid[14], f.guid[15],
            f.bits_per_pixel, f.default_frame, f.nframes, f.declared_frames
        );
        for fr in f.frames[..f.nframes].iter() {
            // dwFrameInterval is in 100 ns units (Uncompressed payload §3.2): /10 is microseconds.
            if fr.interval_type == 0 {
                serial_println!(
                    "[uvc] frame fmt={} idx={} {}x{} default_us={} intervals=range min_us={} max_us={} step_us={}",
                    f.index, fr.index, fr.width, fr.height, fr.default_interval / 10,
                    fr.min_interval / 10, fr.max_interval / 10, fr.interval_step / 10
                );
            } else {
                // A list, printed as a list — the census is evidence, not a summary. Up to
                // MAX_INTERVALS of them; `n=` names the DECLARED count so a truncation shows.
                let iv = &fr.intervals[..fr.n_intervals];
                serial_println!(
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
        serial_println!(
            "[uvc] alt={} ep=IN{} mps={} mult={} per_uframe={}",
            a.alt, a.ep, a.mps, a.mult, a.mps as u32 * a.mult as u32
        );
    }
    if c.ran_short || c.overflowed {
        serial_println!(
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
        selftest();
    }

    if !cfg_has_video_iad(cfg) {
        serial_println!("[uvc] skip addr={} reason=no-video-iad", addr);
        return;
    }

    // A candidate: re-read the configuration descriptor IN FULL before walking it, exactly as the
    // Bluetooth census does and for the same reason — 64 bytes is a stub of a composite device,
    // and a camera's VideoStreaming interface (the half this rung needs) is never inside it.
    // `data_buf` is 256 bytes, so that is the ceiling; `ran_short` on the census line says when
    // the device's own wTotalLength exceeded it, and NOTHING here pretends otherwise.
    let wtotal = if cfg.len() >= 4 { le16(cfg, 2) } else { 0 };
    let want = wtotal.min(256);
    serial_println!(
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
    let c = parse(full);
    print_census(idx, addr, &c);

    if !c.have_vs {
        serial_println!(
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
            serial_println!(
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
    serial_println!("[uvc] configured addr={} cfg_value={} vs_intf={}", addr, config_value, c.vs_intf);

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
                        serial_println!(
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
    serial_println!(
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
