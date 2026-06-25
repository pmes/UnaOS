// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Minimal hand-rolled TCP: a multi-connection passive echo listener ([`TcpListener`]) plus a
//! single active-open client ([`TcpClient`]).
//!
//! Scope (deliberately small): the listener accepts up to [`MAX_CONNS`] simultaneous
//! connections, demultiplexed by the remote `(IP, port)` into a fixed table of [`TcpConn`]
//! slots (a free slot is `None`; there is no `Listen` state). Per connection: passive open,
//! in-order delivery with a few buffered out-of-order extents for reassembly, a one-segment send
//! window with retransmission on an adaptive RTO (RFC 6298 + Karn's algorithm), no window
//! scaling/options, lenient receiver (incoming TCP checksum not verified; outgoing is).
//! Per-connection states: SynRcvd -> Established -> LastAck, then the slot is freed. Each
//! received segment produces at most one response segment (the driver sends one reply per RX
//! frame), which suffices because the ACK/echo/FIN can be folded into a single segment.

use crate::ethernet::{self, EthernetFrame, EtherType};
use crate::ipv4::{self, Ipv4Header};

/// TCP protocol number in the IPv4 header.
pub const PROTO_TCP: u8 = 6;

// TCP flag bits (byte 13 of the header).
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

/// Wraparound-safe TCP sequence comparison: is `a` strictly before `b` in sequence space?
/// (RFC 1982 serial-number arithmetic — compares the signed 32-bit difference.)
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
/// Wraparound-safe: is `a` strictly after `b`?
fn seq_gt(a: u32, b: u32) -> bool {
    seq_lt(b, a)
}

/// Zero-copy parser for a TCP segment.
pub struct TcpSegment<'a> {
    buffer: &'a [u8],
    data_offset: usize,
}

impl<'a> TcpSegment<'a> {
    pub fn new(buffer: &'a [u8]) -> Option<Self> {
        if buffer.len() < 20 {
            return None;
        }
        let data_offset = ((buffer[12] >> 4) as usize) * 4;
        if data_offset < 20 || data_offset > buffer.len() {
            return None;
        }
        Some(Self { buffer, data_offset })
    }

    pub fn source_port(&self) -> u16 {
        u16::from_be_bytes([self.buffer[0], self.buffer[1]])
    }
    pub fn dest_port(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }
    pub fn seq(&self) -> u32 {
        u32::from_be_bytes([self.buffer[4], self.buffer[5], self.buffer[6], self.buffer[7]])
    }
    pub fn ack(&self) -> u32 {
        u32::from_be_bytes([self.buffer[8], self.buffer[9], self.buffer[10], self.buffer[11]])
    }
    pub fn flags(&self) -> u8 {
        self.buffer[13]
    }
    pub fn window(&self) -> u16 {
        u16::from_be_bytes([self.buffer[14], self.buffer[15]])
    }
    pub fn payload(&self) -> &'a [u8] {
        &self.buffer[self.data_offset..]
    }
}

/// TCP checksum over the IPv4 pseudo-header (src/dst IP + proto 6 + TCP length) + segment.
fn checksum(src_ip: [u8; 4], dst_ip: [u8; 4], seg: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += PROTO_TCP as u32;
    sum += seg.len() as u32;
    let mut i = 0;
    while i + 1 < seg.len() {
        sum += u16::from_be_bytes([seg[i], seg[i + 1]]) as u32;
        i += 2;
    }
    if i < seg.len() {
        sum += (seg[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Write a 20-byte (no options) TCP segment + payload into `out`. Returns total length.
#[allow(clippy::too_many_arguments)]
fn write_segment(
    out: &mut [u8],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    payload: &[u8],
) -> Option<usize> {
    let total = 20 + payload.len();
    if out.len() < total {
        return None;
    }
    out[0..2].copy_from_slice(&src_port.to_be_bytes());
    out[2..4].copy_from_slice(&dst_port.to_be_bytes());
    out[4..8].copy_from_slice(&seq.to_be_bytes());
    out[8..12].copy_from_slice(&ack.to_be_bytes());
    out[12] = 5 << 4; // data offset = 5 (20 bytes), reserved 0
    out[13] = flags;
    out[14..16].copy_from_slice(&window.to_be_bytes());
    out[16..18].copy_from_slice(&0u16.to_be_bytes()); // checksum (zero before computing)
    out[18..20].copy_from_slice(&0u16.to_be_bytes()); // urgent pointer
    out[20..total].copy_from_slice(payload);
    let c = checksum(src_ip, dst_ip, &out[..total]);
    out[16..18].copy_from_slice(&c.to_be_bytes());
    Some(total)
}

/// Per-connection state for the multi-connection listener. There is no `Listen` state —
/// the *existence* of a `TcpConn` slot is the connection; a free slot is `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnState {
    SynRcvd,
    Established,
    LastAck,
}

/// Largest payload a connection will accept / echo / buffer for retransmission. Also the
/// advertised receive window, so a conforming peer never sends more than fits the buffer.
const RETX_CAP: usize = 512;
// Adaptive retransmission timer (RFC 6298), in monotonic ticks. The APIC-timer tick is ~1 ms on
// QEMU (empirically), so these durations are coarse, not exact. Each connection measures the RTT
// of cleanly-acknowledged segments and maintains a smoothed estimate (SRTT) and its variation
// (RTTVAR), deriving the RTO as `SRTT + max(G, K*RTTVAR)` (§2). Karn's algorithm excludes
// retransmitted segments from RTT sampling and backs the RTO off (doubling) on each timeout.
//
/// Initial RTO before any RTT sample exists. RFC 6298 §2.1 suggests 1 s; we use ~200 ms — the
/// previous fixed base — which is conservative enough not to fire before a normal ACK on this
/// link yet recovers quickly (it sets the half-open / first-retransmit timing).
const RTO_INIT_TICKS: u64 = 200;
/// Lower bound on the computed RTO (~200 ms, matching Linux's `TCP_RTO_MIN`). On the
/// sub-millisecond QEMU link the estimate floors here, so observed timing matches the old fixed
/// base and we are never *more* aggressive than before; adaptation only raises the RTO above the
/// floor on a slower or higher-variance link.
const RTO_MIN_TICKS: u64 = 200;
/// Upper bound on the RTO / any single backoff interval (~2 s).
const RTO_MAX_TICKS: u64 = 2000;
/// Clock granularity G (one tick): the floor of the variance term in the RTO formula, so a
/// near-zero measured RTT still yields a non-zero margin before the [`RTO_MIN_TICKS`] clamp.
const RTO_CLOCK_GRANULARITY: u32 = 1;
/// Give up (free the connection) after this many retransmissions — this also bounds a half-open
/// (SynRcvd) connection whose handshake ACK never arrives, so it can't wedge a slot forever.
/// With the backoff above this is ~7 s to fully abandon a dead/half-open connection.
const MAX_RETRIES: u8 = 6;

/// One step of the RFC 6298 §2 RTT estimator. Given the current smoothed estimate and whether it
/// already holds a measurement (`valid`), fold in a new RTT sample `r` (ticks) and return the
/// updated `(srtt, rttvar, rto)` — the RTO already clamped to `[RTO_MIN_TICKS, RTO_MAX_TICKS]`.
/// Pure (no `self`) so it is unit-testable in isolation. Uses alpha = 1/8, beta = 1/4, K = 4 via
/// integer shifts; all intermediates stay small (each of `srtt`/`rttvar` is bounded by the
/// `RTO_MAX_TICKS` cap on `r`), so the `u32` arithmetic cannot overflow.
fn rfc6298_step(srtt: u32, rttvar: u32, valid: bool, r: u32) -> (u32, u32, u32) {
    let (srtt, rttvar) = if !valid {
        // First measurement (§2 (2.2)): SRTT = R, RTTVAR = R/2.
        (r, r / 2)
    } else {
        // Subsequent (§2 (2.3)): RTTVAR uses the OLD SRTT, so update RTTVAR *before* SRTT.
        let delta = if srtt > r { srtt - r } else { r - srtt };
        let rttvar = (rttvar * 3 + delta) / 4; // (1 - beta)*RTTVAR + beta*|SRTT - R|
        let srtt = (srtt * 7 + r) / 8; //          (1 - alpha)*SRTT + alpha*R
        (srtt, rttvar)
    };
    // RTO = SRTT + max(G, K*RTTVAR), clamped (§2 (2.4)).
    let variance = (4 * rttvar).max(RTO_CLOCK_GRANULARITY);
    let rto = (srtt.saturating_add(variance) as u64).clamp(RTO_MIN_TICKS, RTO_MAX_TICKS) as u32;
    (srtt, rttvar, rto)
}

/// Capacity of a connection's send byte-stream buffer. Received data is appended here to be
/// echoed; it can hold many segments at once, so several may be in flight (a sliding window)
/// instead of the old one-segment-at-a-time model. Also the constant advertised receive window.
const SND_BUF: usize = 2048;
/// Largest payload we put in one emitted segment (our effective MSS). Kept at [`RETX_CAP`].
const MSS: usize = RETX_CAP;

/// A byte-stream send buffer with TCP sequence accounting, over a contiguous sequence range.
/// Holds the bytes for seqs `[una, una+len)`: the prefix `[una, nxt)` has been sent and is
/// awaiting acknowledgement (in flight), and `[nxt, una+len)` is queued but unsent. Backed by a
/// fixed ring (no allocation). Encapsulated and unit-tested in isolation so the sequence
/// arithmetic — the error-prone heart of a sliding window — is verified independent of the
/// connection state machine. SYN and FIN are *not* stored here; the connection accounts for those
/// one-sequence elements separately (this buffer is pure data bytes).
struct SendRing {
    buf: [u8; SND_BUF],
    head: usize, // ring index of the byte at `una`
    una: u32,    // oldest unacknowledged data sequence number (first byte in the buffer)
    nxt: u32,    // next sequence number to send (una <= nxt <= una + len)
    len: usize,  // bytes buffered = in-flight (nxt-una) + queued (una+len-nxt)
}

impl SendRing {
    /// New, empty buffer whose first data byte will have sequence `data_start` (the ISN + 1).
    fn new(data_start: u32) -> Self {
        Self { buf: [0; SND_BUF], head: 0, una: data_start, nxt: data_start, len: 0 }
    }
    /// Bytes that can still be appended.
    fn free(&self) -> usize {
        SND_BUF - self.len
    }
    /// Bytes sent but not yet acknowledged.
    fn flight_len(&self) -> usize {
        self.nxt.wrapping_sub(self.una) as usize
    }
    /// Have all buffered bytes been sent (nothing left queued)?
    fn fully_sent(&self) -> bool {
        self.nxt == self.una.wrapping_add(self.len as u32)
    }
    /// One past the last buffered byte's sequence (where a FIN would sit).
    fn end_seq(&self) -> u32 {
        self.una.wrapping_add(self.len as u32)
    }
    /// Append data to the queue, bounded by [`free`](Self::free); returns the bytes accepted.
    fn push(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.free());
        let start = (self.head + self.len) % SND_BUF;
        for (i, &b) in data[..n].iter().enumerate() {
            self.buf[(start + i) % SND_BUF] = b;
        }
        self.len += n;
        n
    }
    /// Copy the next sendable segment (queued bytes starting at `nxt`, up to [`MSS`], and limited
    /// so the total in flight stays within the peer's advertised window `wnd`) into `out`.
    /// Returns the byte count, or 0 if nothing is sendable right now. Does NOT advance `nxt` —
    /// the caller invokes [`mark_sent`](Self::mark_sent) only after the segment serializes (the
    /// no-desync rule).
    fn peek_seg(&self, wnd: u16, out: &mut [u8]) -> usize {
        let queued = self.len - self.flight_len();
        let usable = (wnd as usize).saturating_sub(self.flight_len());
        let n = queued.min(MSS).min(usable).min(out.len());
        let start = (self.head + self.flight_len()) % SND_BUF; // ring index of `nxt`
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = self.buf[(start + i) % SND_BUF];
        }
        n
    }
    /// Advance `nxt` after `n` bytes were emitted.
    fn mark_sent(&mut self, n: usize) {
        self.nxt = self.nxt.wrapping_add(n as u32);
    }
    /// Rewind `nxt` to `una` so all in-flight bytes are re-sent (Go-Back-N retransmission).
    fn rewind(&mut self) {
        self.nxt = self.una;
    }
    /// Process a cumulative ACK: free newly acknowledged data bytes from the front and advance
    /// `una`. `ack` is clamped to `[una, una+len]` (acks of our SYN/FIN, which sit outside this
    /// buffer, free at most all the data). Returns the number of data bytes newly freed.
    fn ack(&mut self, ack: u32) -> usize {
        if !seq_gt(ack, self.una) {
            return 0; // old or duplicate ACK — nothing new
        }
        let acked = (ack.wrapping_sub(self.una) as usize).min(self.len);
        self.head = (self.head + acked) % SND_BUF;
        self.len -= acked;
        self.una = self.una.wrapping_add(acked as u32);
        if seq_lt(self.nxt, self.una) {
            self.nxt = self.una; // never let nxt fall behind una
        }
        acked
    }
}

/// Number of out-of-order extents a connection buffers for reassembly. A small ring of slots
/// (vs. a single extent) lets the listener hold several distinct reordered segments at once and
/// deliver them in order as the gaps fill, instead of dropping all but one. Independent of the
/// advertised window (which is a constant pending honest flow control); buffering past the
/// window is bounded and harmless on the trusted link.
const OOO_EXTENTS: usize = 4;

/// One buffered out-of-order segment: payload (and/or a FIN) that arrived ahead of `rcv_nxt`,
/// held until the gap before it fills. A free slot has `used == false`.
#[derive(Clone, Copy)]
struct OooExtent {
    used: bool,
    seq: u32,
    len: usize,
    fin: bool,
    buf: [u8; RETX_CAP],
}

impl OooExtent {
    const EMPTY: Self = Self { used: false, seq: 0, len: 0, fin: false, buf: [0; RETX_CAP] };
}

/// One accepted connection's control block. The listener holds a small fixed table of these
/// and demultiplexes inbound segments to them by the remote `(IP, port)` 2-tuple (the local
/// IP/port are fixed by the listener). Received data is appended to a [`SendRing`] to be echoed,
/// so several echo segments can be in flight at once (a sliding send window bounded by the peer's
/// advertised window), with Go-Back-N retransmission of the oldest unacknowledged data.
struct TcpConn {
    state: ConnState,
    remote_ip: [u8; 4],
    remote_mac: [u8; 6],
    remote_port: u16,
    iss: u32,     // our initial send sequence (the SYN-ACK's seq; consumes one)
    rcv_nxt: u32, // next sequence number we expect to receive
    snd: SendRing, // the echo byte-stream send buffer (data starts at iss + 1)
    snd_wnd: u16,  // the peer's most recently advertised receive window
    syn_acked: bool, // our SYN-ACK has been acknowledged (handshake complete)
    // Close: `peer_fin` latches once the peer's FIN has been received in order; `closing` once we
    // have committed to sending our own FIN (after the send buffer drains) at `fin_seq`.
    peer_fin: bool,
    closing: bool,
    fin_seq: u32,
    fin_sent: bool,
    // Retransmission timer (RFC 6298 adaptive RTO + Karn), timing the OLDEST unacknowledged sent
    // element (the SYN-ACK, the oldest in-flight data, or the FIN). `rto_armed` is true while any
    // such element is outstanding.
    rto_armed: bool,
    un_sent_at: u64,       // tick the timed element (oldest in this busy period) was sent
    un_retx: bool,         // it was retransmitted — Karn excludes it from RTT sampling
    sample_pending: bool,  // one RTT sample is still owed for the current busy period
    rto_deadline: u64,
    retries: u8,
    srtt: u32,       // smoothed round-trip time
    rttvar: u32,     // round-trip time variation
    rto: u32,        // current retransmission timeout
    rtt_valid: bool, // whether srtt/rttvar hold a real measurement yet
    // Reassembly of out-of-order segments (up to OOO_EXTENTS extents ahead of rcv_nxt), drained
    // into the send buffer once the gap before them fills.
    ooo: [OooExtent; OOO_EXTENTS],
}

impl TcpConn {
    fn new(state: ConnState, remote_ip: [u8; 4], remote_mac: [u8; 6], remote_port: u16, iss: u32, rcv_nxt: u32) -> Self {
        Self {
            state, remote_ip, remote_mac, remote_port, iss, rcv_nxt,
            snd: SendRing::new(iss.wrapping_add(1)),
            snd_wnd: 0,
            syn_acked: false,
            peer_fin: false,
            closing: false,
            fin_seq: 0,
            fin_sent: false,
            rto_armed: false,
            un_sent_at: 0, un_retx: false, sample_pending: false,
            rto_deadline: 0, retries: 0,
            srtt: 0, rttvar: 0, rto: RTO_INIT_TICKS as u32, rtt_valid: false,
            ooo: [OooExtent::EMPTY; OOO_EXTENTS],
        }
    }

    fn matches(&self, ip: [u8; 4], port: u16) -> bool {
        self.remote_ip == ip && self.remote_port == port
    }

    /// Our honestly-advertised receive window: the free space in the echo send buffer — i.e. how
    /// many more bytes we can currently accept and echo. Shrinks as the buffer fills and reaches 0
    /// when full, so a conforming peer pauses instead of overrunning us. (`SND_BUF` <= u16::MAX, so
    /// the cast cannot truncate.)
    fn adv_window(&self) -> u16 {
        self.snd.free() as u16
    }

    /// Emit one segment, building the full Eth/IPv4/TCP frame into `out`. `seq` and `ack` are
    /// passed explicitly (rather than read from `self`) so a data echo can ACK the bytes it
    /// carries — and a retransmission can re-send the *original* seq — while the `rcv_nxt`/
    /// `snd_nxt` advance is committed only *after* serialization succeeds (the no-desync
    /// invariant). The advertised window is always our current honest receive capacity.
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        flags: u8,
        seq: u32,
        ack: u32,
        payload: &[u8],
    ) -> Option<usize> {
        let tcp_len = write_segment(
            out.get_mut(34..)?,
            local_port,
            self.remote_port,
            seq,
            ack,
            flags,
            self.adv_window(),
            our_ip,
            self.remote_ip,
            payload,
        )?;
        ipv4::write_header(out.get_mut(14..34)?, our_ip, self.remote_ip, PROTO_TCP, tcp_len)?;
        ethernet::write_header(out.get_mut(0..14)?, self.remote_mac, our_mac, EtherType::Ipv4.as_u16())?;
        Some(14 + 20 + tcp_len)
    }

    /// Arm the retransmission timer for the oldest unacknowledged sent element (SYN-ACK, oldest
    /// in-flight data, or our FIN) if it is not already running. `un_sent_at`/`un_retx` then time
    /// that element for RTT sampling (Karn's algorithm).
    fn arm_rto(&mut self, now: u64) {
        if !self.rto_armed {
            self.rto_armed = true;
            self.un_sent_at = now; // this segment's send time anchors the RTT measurement
            self.un_retx = false;
            self.sample_pending = true; // owe exactly one RTT sample for this busy period
            self.retries = 0;
            self.rto_deadline = now + self.rto as u64;
        }
    }

    /// Fold one RTT measurement `r` (ticks) into this connection's smoothed estimate and
    /// recompute the RTO (RFC 6298 §2). Called only for a clean — non-retransmitted — ACK.
    fn update_rtt(&mut self, r: u64) {
        // Coarse tick clock; bound the sample so a pathologically delayed ACK can't blow up the
        // estimate (and keeps the estimator's u32 arithmetic in range).
        let r = r.min(RTO_MAX_TICKS) as u32;
        let (srtt, rttvar, rto) = rfc6298_step(self.srtt, self.rttvar, self.rtt_valid, r);
        self.srtt = srtt;
        self.rttvar = rttvar;
        self.rto = rto;
        self.rtt_valid = true;
    }

    /// A cumulative ACK advanced our send sequence (freed echo data, or acked the SYN-ACK). Take
    /// the one RTT sample owed for this busy period (the segment timed since the timer last went
    /// idle, unless it was retransmitted — Karn), then keep the retransmit deadline running for
    /// any still-unacknowledged data/FIN, or disarm. We deliberately sample only once per busy
    /// period rather than re-anchoring on each ACK (which would measure inter-ACK gaps, not RTT).
    fn note_ack(&mut self, now: u64) {
        if self.rto_armed && self.sample_pending && !self.un_retx {
            self.update_rtt(now.saturating_sub(self.un_sent_at));
            self.sample_pending = false;
        }
        // Keep the timer running while ANY buffered echo bytes (in flight OR still queued) or our
        // FIN remain. Queued-but-unsendable data arises when the peer advertised a window smaller
        // than it sent and then stalled at a zero window: with the window open the send pump drains
        // it long before the RTO fires, but if it stays shut the retransmit path resends from `una`
        // (a coarse window probe — a real persist timer is a separate item) and, failing that,
        // `tick`'s MAX_RETRIES reaper frees the slot rather than leaking it forever.
        if self.snd.len > 0 || self.fin_sent {
            self.rto_deadline = now + self.rto as u64; // restart the retransmit countdown
            self.retries = 0;
            self.rto_armed = true;
        } else {
            self.rto_armed = false;
            self.retries = 0;
        }
    }

    /// Re-send the oldest unacknowledged element on RTO expiry. Karn's algorithm: mark it
    /// retransmitted (so its ACK isn't RTT-sampled) and back the RTO off by doubling, capped at
    /// [`RTO_MAX_TICKS`] (RFC 6298 §5.5). Go-Back-N for data — rewind the send buffer so the whole
    /// in-flight window re-sends over subsequent passes. Returns the frame, or `None`.
    fn retransmit(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, now: u64) -> Option<usize> {
        self.retries = self.retries.saturating_add(1);
        self.un_retx = true;
        self.rto = ((self.rto as u64 * 2).min(RTO_MAX_TICKS)) as u32;
        self.rto_deadline = now + self.rto as u64;
        if !self.syn_acked {
            return self.emit(out, our_ip, our_mac, local_port, SYN | ACK, self.iss, self.rcv_nxt, &[]);
        }
        if self.snd.len > 0 {
            self.snd.rewind();
            let mut tmp = [0u8; MSS];
            // Resend [una, una+MSS): already within the peer's window when first sent.
            let n = self.snd.peek_seg(u16::MAX, &mut tmp);
            let seq = self.snd.una;
            let frame = self.emit(out, our_ip, our_mac, local_port, PSH | ACK, seq, self.rcv_nxt, &tmp[..n])?;
            self.snd.mark_sent(n);
            return Some(frame);
        }
        if self.fin_sent {
            return self.emit(out, our_ip, our_mac, local_port, FIN | ACK, self.fin_seq, self.rcv_nxt, &[]);
        }
        None
    }

    /// Emit the next sendable segment: queued echo data (bounded by the peer's advertised window
    /// `snd_wnd`), or — once all data is sent and the peer has closed — our FIN. Arms the RTO. One
    /// segment per call (the driver transmits one frame per event); the send buffer drains across
    /// successive `handle`/`tick` calls. Returns `None` when nothing is sendable now.
    fn send_next(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, now: u64) -> Option<usize> {
        let mut tmp = [0u8; MSS];
        let n = self.snd.peek_seg(self.snd_wnd, &mut tmp);
        if n > 0 {
            let seq = self.snd.nxt;
            let frame = self.emit(out, our_ip, our_mac, local_port, PSH | ACK, seq, self.rcv_nxt, &tmp[..n])?;
            self.snd.mark_sent(n);
            self.arm_rto(now);
            return Some(frame);
        }
        if self.closing && !self.fin_sent && self.snd.fully_sent() {
            // The FIN consumes one sequence number after the last echoed byte. Sent even if the
            // peer's window is momentarily zero (it is one byte, and withholding it would stall the
            // close; honest zero-window probing via a persist timer is a separate item).
            let frame = self.emit(out, our_ip, our_mac, local_port, FIN | ACK, self.fin_seq, self.rcv_nxt, &[])?;
            self.fin_sent = true;
            self.state = ConnState::LastAck;
            self.arm_rto(now);
            return Some(frame);
        }
        None
    }

    /// Drive this connection with one received segment (`their_wnd` is the peer's advertised
    /// window). Returns `(response, done)`: `response` is one segment to transmit (if any), and
    /// `done == true` means the connection fully closed and the listener should free its slot.
    #[allow(clippy::too_many_arguments)]
    fn on_segment(
        &mut self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        now: u64,
        flags: u8,
        their_seq: u32,
        their_ack: u32,
        their_wnd: u16,
        payload: &[u8],
    ) -> (Option<usize>, bool) {
        self.snd_wnd = their_wnd;
        match self.state {
            ConnState::SynRcvd => {
                // Complete the handshake on the ACK of our SYN-ACK (it may already carry data).
                if flags & ACK != 0 && their_ack == self.iss.wrapping_add(1) {
                    self.syn_acked = true;
                    self.note_ack(now); // sample the SYN-ACK RTT; nothing else is in flight yet
                    self.state = ConnState::Established;
                    let resp = self.on_established(out, our_ip, our_mac, local_port, now, flags, their_seq, their_ack, payload);
                    (resp, false)
                } else {
                    (None, false)
                }
            }
            ConnState::Established => {
                let resp = self.on_established(out, our_ip, our_mac, local_port, now, flags, their_seq, their_ack, payload);
                (resp, false)
            }
            ConnState::LastAck => {
                // We have sent our FIN and are awaiting its acknowledgement.
                if flags & ACK != 0 {
                    if seq_gt(their_ack, self.fin_seq) {
                        return (None, true); // FIN acknowledged — connection closed
                    }
                    if self.snd.ack(their_ack) > 0 {
                        self.note_ack(now);
                    }
                }
                (None, false)
            }
        }
    }

    /// Established-state processing of one segment: free acknowledged echo data, accept in-order
    /// data into the send buffer (to echo) or buffer it for reassembly, reassemble, then emit the
    /// next sendable segment (which carries the ACK) or a bare (dup-)ACK. Several echo segments
    /// may be in flight at once, bounded by the peer's advertised window.
    #[allow(clippy::too_many_arguments)]
    fn on_established(
        &mut self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        now: u64,
        flags: u8,
        their_seq: u32,
        their_ack: u32,
        payload: &[u8],
    ) -> Option<usize> {
        // Our advertised window before this segment: if it was 0 (send buffer full) the peer has
        // paused, and a later ACK that frees space must trigger a window-update or we deadlock.
        let window_was_closed = self.adv_window() == 0;

        // (1) Acknowledgement: free echo data the peer has now received.
        if flags & ACK != 0 && self.snd.ack(their_ack) > 0 {
            self.note_ack(now);
        }

        // (2) Inbound data / FIN.
        let has_fin = flags & FIN != 0;
        if seq_gt(their_seq, self.rcv_nxt) {
            // A gap ahead of rcv_nxt: buffer for reassembly (the dup-ACK below signals the gap).
            self.buffer_ooo(their_seq, payload, has_fin);
        } else if their_seq == self.rcv_nxt {
            // In order. Accept data only if it fits the send buffer; otherwise leave it for the
            // peer to retransmit (backpressure — we dup-ACK without consuming).
            if !payload.is_empty() {
                if self.snd.free() >= payload.len() {
                    self.snd.push(payload);
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(payload.len() as u32);
                    if has_fin {
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                        self.peer_fin = true;
                    }
                }
            } else if has_fin {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.peer_fin = true;
            }
        }
        // (else seq_lt: old / already-received — dup-ACK below.)

        // (3) Reassemble: push any now-in-order buffered extents into the send buffer.
        self.drain_ooo();
        // (4) Once the peer has closed and all echo data is queued, commit to sending our FIN
        // after it (its sequence sits one past the last queued byte).
        if self.peer_fin && !self.closing {
            self.closing = true;
            self.fin_seq = self.snd.end_seq();
        }

        // (5) Respond: a queued data/FIN segment (whose ACK field carries the now-current window)
        // takes precedence; otherwise a (dup-)ACK if the inbound carried sequence space; otherwise
        // a WINDOW UPDATE if our window just reopened (was 0, now > 0) so a paused peer resumes —
        // without this, advertising a zero window deadlocks. Else nothing.
        if let Some(n) = self.send_next(out, our_ip, our_mac, local_port, now) {
            return Some(n);
        }
        let owe_ack = !payload.is_empty() || has_fin;
        let window_reopened = window_was_closed && self.adv_window() > 0;
        if owe_ack || window_reopened {
            return self.emit(out, our_ip, our_mac, local_port, ACK, self.snd.nxt, self.rcv_nxt, &[]);
        }
        None
    }

    /// Buffer a future segment (data and/or FIN that arrived ahead of `rcv_nxt`) into a free
    /// out-of-order extent for later reassembly. A segment whose `seq` already occupies a slot
    /// refreshes that slot (a duplicate); otherwise the first free slot is used. With all slots
    /// full, the segment is dropped (the peer will retransmit it). Empty, FIN-less segments and
    /// oversized payloads (> RETX_CAP) are ignored.
    fn buffer_ooo(&mut self, seq: u32, payload: &[u8], has_fin: bool) {
        if (payload.is_empty() && !has_fin) || payload.len() > RETX_CAP {
            return;
        }
        let idx = self
            .ooo
            .iter()
            .position(|e| e.used && e.seq == seq)
            .or_else(|| self.ooo.iter().position(|e| !e.used));
        if let Some(i) = idx {
            let e = &mut self.ooo[i];
            e.used = true;
            e.seq = seq;
            e.len = payload.len();
            e.fin = has_fin;
            e.buf[..payload.len()].copy_from_slice(payload);
        }
    }

    /// Reassemble: push every buffered out-of-order extent that is now in order into the send
    /// buffer (to be echoed), advancing `rcv_nxt`. Unlike the old one-segment model, the byte
    /// buffer can absorb all contiguous extents at once (bounded by its free space). A drained
    /// extent carrying a FIN latches `peer_fin`. Stale extents (below `rcv_nxt`) are discarded.
    fn drain_ooo(&mut self) {
        for e in self.ooo.iter_mut() {
            if e.used && seq_lt(e.seq, self.rcv_nxt) {
                e.used = false;
            }
        }
        loop {
            let idx = match self.ooo.iter().position(|e| e.used && e.seq == self.rcv_nxt) {
                Some(i) => i,
                None => break,
            };
            let len = self.ooo[idx].len;
            if self.snd.free() < len {
                break; // doesn't fit yet — leave it buffered (backpressure)
            }
            // Copy out before borrowing `snd` mutably, then queue it for echo.
            let mut tmp = [0u8; RETX_CAP];
            tmp[..len].copy_from_slice(&self.ooo[idx].buf[..len]);
            self.snd.push(&tmp[..len]);
            self.rcv_nxt = self.rcv_nxt.wrapping_add(len as u32);
            let fin = self.ooo[idx].fin;
            self.ooo[idx].used = false;
            if fin {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.peer_fin = true;
                break; // a FIN is the last thing in the stream
            }
        }
    }
}

/// Number of simultaneous connections the echo listener accepts.
pub const MAX_CONNS: usize = 4;

/// A multi-connection TCP echo listener (RFC 862). Accepts up to [`MAX_CONNS`] passive-open
/// connections at once, demultiplexing inbound segments to a fixed table of [`TcpConn`] slots
/// by the remote `(IP, port)`. Per-connection scope: in-order delivery with reassembly, a
/// byte-stream send buffer with a sliding (multi-segment) window bounded by the peer's advertised
/// window and Go-Back-N retransmission ([`tick`](Self::tick) drives the RTO timers and pushes
/// queued echo data), no options, lenient receiver. No allocation (fixed tables).
pub struct TcpListener {
    listen_port: u16,
    isn: u32, // ramps per accepted connection to avoid TIME_WAIT collisions
    conns: [Option<TcpConn>; MAX_CONNS],
}

impl TcpListener {
    pub fn new(listen_port: u16) -> Self {
        Self {
            listen_port,
            isn: 0x0001_0000,
            conns: core::array::from_fn(|_| None),
        }
    }

    /// Number of currently-active connections (for diagnostics).
    pub fn active_conns(&self) -> usize {
        self.conns.iter().filter(|c| c.is_some()).count()
    }

    /// Service the connections. Call periodically with the current monotonic `now`. For the first
    /// connection that needs it, either retransmit one expired segment (exponential backoff; after
    /// [`MAX_RETRIES`] the slot is freed — which also bounds stuck half-open connections) OR push
    /// the next queued echo segment as the peer's window allows, returning that frame. Driving
    /// this every main-loop pass drains each send buffer across passes (one frame per pass).
    pub fn tick(&mut self, now: u64, our_ip: [u8; 4], our_mac: [u8; 6], out: &mut [u8]) -> Option<usize> {
        let local_port = self.listen_port;
        for slot in self.conns.iter_mut() {
            let conn = match slot {
                Some(c) => c,
                None => continue,
            };
            if conn.rto_armed && now >= conn.rto_deadline {
                if conn.retries >= MAX_RETRIES {
                    *slot = None; // give up — frees half-open or stuck connections
                    continue;
                }
                return conn.retransmit(out, our_ip, our_mac, local_port, now);
            }
            if let Some(n) = conn.send_next(out, our_ip, our_mac, local_port, now) {
                return Some(n);
            }
        }
        None
    }

    /// Handle a received frame. Returns `Some(len)` with a response frame in `out`, or `None`
    /// if the frame isn't a TCP segment for our listener (so the caller falls back to ingress)
    /// or no response is warranted. `now` is the current monotonic tick (for arming RTO).
    pub fn handle(&mut self, frame: &[u8], now: u64, our_ip: [u8; 4], our_mac: [u8; 6], out: &mut [u8]) -> Option<usize> {
        let eth = EthernetFrame::new(frame)?;
        if eth.ethertype() != EtherType::Ipv4 {
            return None;
        }
        let ip = Ipv4Header::new(eth.payload())?;
        if ip.protocol() != PROTO_TCP || ip.destination_ip() != our_ip || !ip.verify_checksum() {
            return None;
        }
        let seg = TcpSegment::new(ip.payload())?;
        if seg.dest_port() != self.listen_port {
            return None; // not for our listener
        }

        let src_ip = ip.source_ip();
        let src_mac = eth.source_mac();
        let src_port = seg.source_port();
        let flags = seg.flags();
        let their_seq = seg.seq();
        let their_ack = seg.ack();
        let their_wnd = seg.window();
        let payload = seg.payload();

        let existing = self
            .conns
            .iter()
            .position(|c| c.as_ref().map_or(false, |c| c.matches(src_ip, src_port)));

        // RST tears down the matching connection (if any).
        if flags & RST != 0 {
            if let Some(idx) = existing {
                self.conns[idx] = None;
            }
            return None;
        }

        if let Some(idx) = existing {
            let local_port = self.listen_port;
            let conn = self.conns[idx].as_mut().unwrap();
            let (resp, done) =
                conn.on_segment(out, our_ip, our_mac, local_port, now, flags, their_seq, their_ack, their_wnd, payload);
            if done {
                self.conns[idx] = None;
            }
            resp
        } else if flags & SYN != 0 && flags & ACK == 0 {
            // Passive open: a bare SYN to a fresh 4-tuple starts a new connection if a slot
            // is free (table full -> drop the SYN; the peer retransmits).
            let slot = self.conns.iter().position(|c| c.is_none())?;
            self.isn = self.isn.wrapping_add(0x0001_0000);
            let mut conn = TcpConn::new(
                ConnState::SynRcvd, src_ip, src_mac, src_port,
                self.isn, their_seq.wrapping_add(1), // SYN consumes one sequence number
            );
            conn.snd_wnd = their_wnd; // the peer's initial advertised window
            let local_port = self.listen_port;
            // Emit the SYN-ACK first; only commit the slot once it serialized (no-desync). The
            // SYN-ACK occupies `iss`; arming the RTO covers the half-open retransmission timeout.
            let n = conn.emit(out, our_ip, our_mac, local_port, SYN | ACK, conn.iss, conn.rcv_nxt, &[])?;
            conn.arm_rto(now);
            self.conns[slot] = Some(conn);
            Some(n)
        } else {
            None
        }
    }
}

/// State of an active-open (outbound) TCP connection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClientState {
    /// Initial, or torn down by a RST / refused — connection failed.
    Closed,
    /// SYN sent, awaiting the SYN-ACK.
    SynSent,
    /// Handshake complete; data may flow.
    Established,
    /// Our FIN sent, awaiting the peer's FIN/ACK.
    FinWait,
    /// Cleanly closed.
    Done,
}

/// Maximum one-shot payload an outbound connection sends after the handshake (large enough for
/// a minimal HTTP request line + a `Host:` header).
pub const CLIENT_TX_CAP: usize = 128;
/// Maximum bytes of peer response an outbound connection records. Sized so a streaming fetch
/// (e.g. an HTTP response) captures a useful prefix; the buffer also caps how far ahead a peer
/// may send (it is the advertised receive window).
pub const CLIENT_RX_CAP: usize = 2048;

/// A minimal single-connection **active-open** TCP client: the outbound counterpart of
/// [`TcpListener`]. It connects (SYN → SYN-ACK → ACK), optionally sends one payload, records the
/// peer's response, then performs an active close (FIN). Same deliberate limits as the echo
/// listener: one connection, in-order only, no retransmission, no options, lenient receiver
/// (incoming TCP checksum unchecked; outgoing computed). Each received segment yields at most
/// one response segment, matching the driver's one-reply-per-RX-frame model.
pub struct TcpClient {
    state: ClientState,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_mac: [u8; 6],
    remote_port: u16,
    snd_nxt: u32,
    rcv_nxt: u32,
    /// Latched once the handshake completes, so a later RST (which moves us to `Closed`)
    /// doesn't make `established()` retroactively report the connection as never-up.
    was_established: bool,
    tx_payload: [u8; CLIENT_TX_CAP],
    tx_len: usize,
    tx_sent: bool,
    rx_buf: [u8; CLIENT_RX_CAP],
    rx_len: usize,
    /// Set once we have consumed the peer's FIN. Distinguishes active close (we FIN first, the
    /// peer's FIN arrives later in `FinWait`) from passive/simultaneous close (we already saw the
    /// peer's FIN, so a bare ACK of our FIN completes the teardown) — the latter is the common
    /// case for a server that closes after sending its response (e.g. HTTP).
    peer_fin: bool,
    /// Set when streaming closes because the receive buffer filled (a bounded capture) rather than
    /// because the peer FIN'd. We have deliberately stopped reading, so our `rcv_nxt` may sit
    /// mid-stream; the peer's later data/FIN will never line up with it. The teardown therefore
    /// completes on a bare ACK of our FIN (the peer will ACK it), not on an in-order peer FIN —
    /// without this the connection would wedge in `FinWait` until the pump budget drained.
    closed_on_full: bool,
    /// One-shot (false) vs. streaming (true). One-shot closes as soon as it has received back at
    /// least as many bytes as it sent (the echo workload). Streaming keeps ACKing and recording
    /// in-order data until the peer closes (FIN) or the receive buffer fills — for request/
    /// response protocols (e.g. HTTP) whose reply spans many segments and exceeds the request.
    linger: bool,
}

impl TcpClient {
    /// Create a client that will connect to `remote_ip:remote_port` (MAC already resolved)
    /// from `local_port`, using `isn` as the initial send sequence, and send `payload` after
    /// the handshake (truncated to `CLIENT_TX_CAP`; empty = connect then immediately close).
    pub fn new(
        local_port: u16,
        remote_ip: [u8; 4],
        remote_mac: [u8; 6],
        remote_port: u16,
        isn: u32,
        payload: &[u8],
    ) -> Self {
        let mut tx_payload = [0u8; CLIENT_TX_CAP];
        let tx_len = payload.len().min(CLIENT_TX_CAP);
        tx_payload[..tx_len].copy_from_slice(&payload[..tx_len]);
        Self {
            state: ClientState::Closed,
            local_port,
            remote_ip,
            remote_mac,
            remote_port,
            snd_nxt: isn,
            rcv_nxt: 0,
            was_established: false,
            tx_payload,
            tx_len,
            tx_sent: false,
            rx_buf: [0u8; CLIENT_RX_CAP],
            rx_len: 0,
            peer_fin: false,
            closed_on_full: false,
            linger: false,
        }
    }

    /// Switch this client to streaming/linger receive: after sending the request it keeps
    /// ACKing and recording in-order data until the peer closes (FIN) or the receive buffer
    /// fills, rather than closing as soon as the request has been answered. Use for request/
    /// response protocols (e.g. an HTTP GET) where the reply spans many segments. The request
    /// payload should be non-empty (an empty payload still closes immediately after the
    /// handshake, as in one-shot mode).
    pub fn streaming(mut self) -> Self {
        self.linger = true;
        self
    }

    pub fn state(&self) -> ClientState {
        self.state
    }
    /// True once the handshake completed — latched, so it stays true even after a later RST
    /// resets the live state to `Closed`.
    pub fn established(&self) -> bool {
        self.was_established
    }
    /// True once the connection has settled — nothing more to pump. Meaningful only after
    /// [`open`](Self::open) has run (which moves out of the initial `Closed`); thereafter
    /// `Closed` means the peer refused/reset and `Done` means a clean close.
    pub fn is_done(&self) -> bool {
        matches!(self.state, ClientState::Done | ClientState::Closed)
    }
    /// Bytes of peer response recorded so far.
    pub fn rx_data(&self) -> &[u8] {
        &self.rx_buf[..self.rx_len]
    }

    fn emit(&self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], flags: u8, payload: &[u8]) -> Option<usize> {
        // Advertise only the receive-buffer space we actually have, so a conforming peer never
        // sends more than we can record (which would otherwise force us to ACK dropped bytes).
        let window = (CLIENT_RX_CAP - self.rx_len) as u16;
        let tcp_len = write_segment(
            out.get_mut(34..)?,
            self.local_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            flags,
            window,
            our_ip,
            self.remote_ip,
            payload,
        )?;
        ipv4::write_header(out.get_mut(14..34)?, our_ip, self.remote_ip, PROTO_TCP, tcp_len)?;
        ethernet::write_header(out.get_mut(0..14)?, self.remote_mac, our_mac, EtherType::Ipv4.as_u16())?;
        Some(14 + 20 + tcp_len)
    }

    /// Begin the active open: write the SYN into `out` and return its length.
    pub fn open(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6]) -> Option<usize> {
        let n = self.emit(out, our_ip, our_mac, SYN, &[])?;
        self.snd_nxt = self.snd_nxt.wrapping_add(1); // our SYN consumes one sequence number
        self.state = ClientState::SynSent;
        Some(n)
    }

    /// Append peer payload to the receive buffer (bounded), returning how many bytes were kept.
    fn record(&mut self, payload: &[u8]) -> usize {
        let space = CLIENT_RX_CAP - self.rx_len;
        let n = payload.len().min(space);
        self.rx_buf[self.rx_len..self.rx_len + n].copy_from_slice(&payload[..n]);
        self.rx_len += n;
        n
    }

    /// Feed a received frame. Returns `Some(len)` with a response segment in `out`, or `None`
    /// if the frame isn't a segment for this connection (caller falls through) or no response
    /// is warranted. Drives the active-open → established → close state machine.
    pub fn handle(&mut self, frame: &[u8], our_ip: [u8; 4], our_mac: [u8; 6], out: &mut [u8]) -> Option<usize> {
        let eth = EthernetFrame::new(frame)?;
        if eth.ethertype() != EtherType::Ipv4 {
            return None;
        }
        let ip = Ipv4Header::new(eth.payload())?;
        if ip.protocol() != PROTO_TCP
            || ip.source_ip() != self.remote_ip
            || ip.destination_ip() != our_ip
            || !ip.verify_checksum()
        {
            return None;
        }
        let seg = TcpSegment::new(ip.payload())?;
        if seg.source_port() != self.remote_port || seg.dest_port() != self.local_port {
            return None; // not this connection
        }

        let flags = seg.flags();
        let their_seq = seg.seq();
        let their_ack = seg.ack();
        let payload = seg.payload();

        // A RST aborts the connection.
        if flags & RST != 0 {
            self.state = ClientState::Closed;
            return None;
        }

        match self.state {
            ClientState::SynSent => {
                if flags & SYN != 0 && flags & ACK != 0 && their_ack != self.snd_nxt {
                    // A SYN-ACK acknowledging the wrong sequence is a failed handshake — fail
                    // fast (so the connect pump stops) rather than spinning the whole budget.
                    self.state = ClientState::Closed;
                    return None;
                }
                // Expect SYN-ACK acknowledging our SYN.
                if flags & SYN != 0 && flags & ACK != 0 && their_ack == self.snd_nxt {
                    self.rcv_nxt = their_seq.wrapping_add(1); // their SYN consumes one
                    self.state = ClientState::Established;
                    self.was_established = true;
                    if self.tx_len > 0 && !self.tx_sent {
                        // Piggyback the one-shot payload on the handshake-completing ACK.
                        let n = self.emit(out, our_ip, our_mac, PSH | ACK, &self.tx_payload[..self.tx_len])?;
                        self.snd_nxt = self.snd_nxt.wrapping_add(self.tx_len as u32);
                        self.tx_sent = true;
                        Some(n)
                    } else {
                        // Nothing to send: ACK the SYN and immediately begin the active close.
                        let n = self.emit(out, our_ip, our_mac, FIN | ACK, &[])?;
                        self.snd_nxt = self.snd_nxt.wrapping_add(1);
                        self.state = ClientState::FinWait;
                        Some(n)
                    }
                } else {
                    None
                }
            }

            ClientState::Established => {
                // Consume in-order payload (the echo); ignore out-of-order. Advance rcv_nxt by
                // the bytes we actually recorded, never by more (so we never ACK dropped data).
                let mut consumed = false;
                if !payload.is_empty() && their_seq == self.rcv_nxt {
                    let n = self.record(payload);
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
                    consumed = n == payload.len();
                }
                // Honor a FIN only when this segment was fully in-order (rcv_nxt reached the
                // FIN's sequence position) — matches the listener's defensive in-order handling.
                let fin_in_order =
                    flags & FIN != 0 && their_seq.wrapping_add(payload.len() as u32) == self.rcv_nxt;
                if fin_in_order {
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1); // FIN consumes one
                    self.peer_fin = true; // the peer closed first (passive close from here)
                }
                // Decide whether we are done receiving. Streaming keeps going until the peer
                // closes (FIN) or our receive buffer is full (a bounded capture — advertising a
                // zero window would otherwise stall the peer with no app to drain it). One-shot
                // closes as soon as we've received back at least what we sent (the echo).
                let done_receiving = if self.linger {
                    fin_in_order || self.rx_len >= CLIENT_RX_CAP
                } else {
                    self.rx_len >= self.tx_len || fin_in_order
                };
                if done_receiving {
                    // Closing because the bounded buffer filled (not a peer FIN) means we stop
                    // reading mid-stream — remember that so FinWait can finish on a bare ACK.
                    if self.linger && !fin_in_order {
                        self.closed_on_full = true;
                    }
                    let n = self.emit(out, our_ip, our_mac, FIN | ACK, &[])?;
                    self.snd_nxt = self.snd_nxt.wrapping_add(1);
                    self.state = ClientState::FinWait;
                    Some(n)
                } else if consumed {
                    self.emit(out, our_ip, our_mac, ACK, &[])
                } else {
                    None
                }
            }

            ClientState::FinWait => {
                // Active close: the peer's FIN now completes the teardown (only in-order). Record
                // any data the peer coalesced with its FIN before acking, so it isn't dropped.
                if flags & FIN != 0 && their_seq == self.rcv_nxt {
                    if !payload.is_empty() {
                        let n = self.record(payload);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
                    }
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1); // FIN consumes one
                    self.peer_fin = true;
                    self.state = ClientState::Done;
                    self.emit(out, our_ip, our_mac, ACK, &[])
                } else if (self.peer_fin || self.closed_on_full) && flags & ACK != 0 && their_ack == self.snd_nxt {
                    // A bare ACK of our FIN finishes the teardown when either: we already consumed
                    // the peer's FIN (passive/simultaneous close — server closed first), or we
                    // closed because our buffer filled (bounded capture; the peer's remaining data
                    // and FIN sit past our rcv_nxt and would never match Case A). The peer always
                    // ACKs our FIN, so this completes promptly instead of wedging in FinWait.
                    self.state = ClientState::Done;
                    None
                } else {
                    // A duplicate/out-of-order FIN, or an ACK that doesn't yet cover our FIN.
                    None
                }
            }

            ClientState::Closed | ClientState::Done => None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Host-side unit tests for the adaptive RTO machinery. These run under `cargo test -p net`
    //! (the crate is `no_std` only for real cross builds — see `lib.rs`). They are deterministic:
    //! the estimator is pure integer arithmetic and the backoff is a fixed doubling sequence.
    use super::*;

    #[test]
    fn rfc6298_first_sample() {
        // First measurement: SRTT = R, RTTVAR = R/2, RTO = SRTT + 4*RTTVAR (above the floor).
        assert_eq!(rfc6298_step(0, 0, false, 100), (100, 50, 300));
    }

    #[test]
    fn rfc6298_subsequent_stable() {
        // A repeat of the same RTT: RTTVAR shrinks (delta 0), SRTT holds, RTO eases toward floor.
        assert_eq!(rfc6298_step(100, 50, true, 100), (100, 37, 248));
    }

    #[test]
    fn rfc6298_large_jump_raises_rto() {
        // A sudden much larger RTT inflates RTTVAR, so the RTO jumps well above the floor.
        let (srtt, rttvar, rto) = rfc6298_step(100, 50, true, 1000);
        assert_eq!((srtt, rttvar), (212, 262));
        assert_eq!(rto, 212 + 4 * 262); // 1260
        assert!(rto > RTO_MIN_TICKS as u32 && rto < RTO_MAX_TICKS as u32);
    }

    #[test]
    fn rfc6298_floors_at_min() {
        // Near-zero RTT (sub-tick link): the clock-granularity term keeps the margin non-zero,
        // and the RTO clamps up to the floor — never more aggressive than the fixed base.
        assert_eq!(rfc6298_step(0, 0, false, 0).2, RTO_MIN_TICKS as u32);
        assert_eq!(rfc6298_step(0, 0, false, 5).2, RTO_MIN_TICKS as u32);
    }

    #[test]
    fn rfc6298_clamps_at_max() {
        // A pathologically large/variable RTT clamps at the cap, not unbounded.
        assert_eq!(rfc6298_step(2000, 2000, true, 2000).2, RTO_MAX_TICKS as u32);
    }

    #[test]
    fn rfc6298_steady_low_rtt_converges_to_floor() {
        // Feeding a steady small RTT drives the RTO down to the floor and holds it there.
        let (mut srtt, mut rttvar, mut valid, mut rto) = (0u32, 0u32, false, 0u32);
        for _ in 0..20 {
            let s = rfc6298_step(srtt, rttvar, valid, 50);
            srtt = s.0;
            rttvar = s.1;
            rto = s.2;
            valid = true;
        }
        assert_eq!(rto, RTO_MIN_TICKS as u32);
    }

    // A fixed local identity; the exact values are irrelevant to the logic.
    const OUR_IP: [u8; 4] = [10, 0, 2, 15];
    const OUR_MAC: [u8; 6] = [9, 9, 9, 9, 9, 9];

    fn test_conn() -> TcpConn {
        TcpConn::new(ConnState::Established, [10, 0, 2, 2], [1, 2, 3, 4, 5, 6], 40000, 0x1000, 0x2000)
    }

    fn used_extents(c: &TcpConn) -> usize {
        c.ooo.iter().filter(|e| e.used).count()
    }

    // --- Out-of-order buffering / reassembly into the send buffer ---

    #[test]
    fn ooo_drops_when_full() {
        let mut c = test_conn();
        for s in [0x2010u32, 0x2020, 0x2030, 0x2040] {
            c.buffer_ooo(s, b"data", false);
        }
        assert_eq!(used_extents(&c), OOO_EXTENTS);
        // All slots full -> a further distinct future segment is dropped (peer retransmits).
        c.buffer_ooo(0x2050, b"late", false);
        assert_eq!(used_extents(&c), OOO_EXTENTS);
        assert!(!c.ooo.iter().any(|e| e.used && e.seq == 0x2050));
    }

    #[test]
    fn ooo_duplicate_seq_overwrites_slot() {
        let mut c = test_conn();
        c.buffer_ooo(0x2010, b"AAAA", false);
        c.buffer_ooo(0x2010, b"BBBBBB", false); // same seq -> refresh, not a new slot
        assert_eq!(used_extents(&c), 1);
        let e = c.ooo.iter().find(|e| e.used && e.seq == 0x2010).unwrap();
        assert_eq!(e.len, 6);
        assert_eq!(&e.buf[..6], b"BBBBBB");
    }

    #[test]
    fn drain_ooo_pushes_contiguous_extents_into_send_buffer() {
        // rcv_nxt = 0x2000. Three contiguous extents become in order and are all queued for echo
        // at once (the byte buffer absorbs them — no one-segment limit).
        let mut c = test_conn();
        c.buffer_ooo(0x2000, b"AAAA", false);
        c.buffer_ooo(0x2004, b"BBBB", false);
        c.buffer_ooo(0x2008, b"CCCC", false);
        assert_eq!(used_extents(&c), 3);
        c.drain_ooo();
        assert_eq!(c.rcv_nxt, 0x200C);
        assert_eq!(used_extents(&c), 0);
        assert_eq!(c.snd.len, 12); // AAAABBBBCCCC queued for echo
    }

    #[test]
    fn drain_ooo_discards_stale() {
        let mut c = test_conn();
        c.buffer_ooo(0x1F00, b"old", false); // below rcv_nxt 0x2000 -> stale
        c.drain_ooo();
        assert_eq!(used_extents(&c), 0);
        assert_eq!(c.snd.len, 0);
    }

    // --- TcpListener integration (handshake, echo, pipelining, window, retransmit, reassembly,
    //     close) driven through the real entry points: handle() and tick(). ---

    const RMAC: [u8; 6] = [2, 2, 2, 2, 2, 2];
    const RIP: [u8; 4] = [10, 0, 2, 99];
    const RPORT: u16 = 50000;

    /// Build an inbound Ethernet/IPv4/TCP frame from the peer (RIP:RPORT) to us on `dport`.
    #[allow(clippy::too_many_arguments)]
    fn lframe(dport: u16, seq: u32, ack: u32, flags: u8, wnd: u16, payload: &[u8], out: &mut [u8; 1600]) -> usize {
        let tcp_len = write_segment(&mut out[34..], RPORT, dport, seq, ack, flags, wnd, RIP, OUR_IP, payload).unwrap();
        ipv4::write_header(&mut out[14..34], RIP, OUR_IP, PROTO_TCP, tcp_len).unwrap();
        ethernet::write_header(&mut out[0..14], OUR_MAC, RMAC, EtherType::Ipv4.as_u16()).unwrap();
        14 + 20 + tcp_len
    }

    struct Seg {
        flags: u8,
        seq: u32,
        ack: u32,
        window: u16,
        payload: Vec<u8>,
    }

    /// Parse the listener's emitted Ethernet/IPv4/TCP frame.
    fn parse_out(out: &[u8], len: usize) -> Seg {
        assert!(len >= 34);
        let ihl = (out[14] & 0x0F) as usize * 4;
        let total = u16::from_be_bytes([out[16], out[17]]) as usize;
        let seg = &out[14 + ihl..14 + total];
        let doff = (seg[12] >> 4) as usize * 4;
        Seg {
            flags: seg[13],
            seq: u32::from_be_bytes([seg[4], seg[5], seg[6], seg[7]]),
            ack: u32::from_be_bytes([seg[8], seg[9], seg[10], seg[11]]),
            window: u16::from_be_bytes([seg[14], seg[15]]),
            payload: seg[doff..].to_vec(),
        }
    }

    /// Feed one inbound frame to the listener and parse its reply (if any). Sequences the borrows
    /// of `out` so the same buffer can be written by `handle` then read by `parse_out`.
    fn feed(l: &mut TcpListener, inb: &[u8], now: u64, out: &mut [u8]) -> Option<Seg> {
        l.handle(inb, now, OUR_IP, OUR_MAC, out).map(|r| parse_out(out, r))
    }
    /// Run one service tick and parse the emitted segment (if any).
    fn pump(l: &mut TcpListener, now: u64, out: &mut [u8]) -> Option<Seg> {
        l.tick(now, OUR_IP, OUR_MAC, out).map(|r| parse_out(out, r))
    }

    /// Drive a passive-open handshake on a fresh listener; returns `(listener, our_isn)`.
    fn handshake(c_isn: u32) -> (TcpListener, u32) {
        let mut l = TcpListener::new(7);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let n = lframe(7, c_isn, 0, SYN, 4096, &[], &mut inb);
        let sa = feed(&mut l, &inb[..n], 1000, &mut out).unwrap();
        assert_eq!(sa.flags & (SYN | ACK), SYN | ACK);
        assert_eq!(sa.ack, c_isn.wrapping_add(1));
        let s_isn = sa.seq;
        let n = lframe(7, c_isn.wrapping_add(1), s_isn.wrapping_add(1), ACK, 4096, &[], &mut inb);
        assert!(feed(&mut l, &inb[..n], 1001, &mut out).is_none()); // bare ACK -> no reply
        (l, s_isn)
    }

    #[test]
    fn listener_handshake_and_echo() {
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let data = b"hello world";
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK, 4096, data, &mut inb);
        let e = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
        assert_eq!(e.payload, data);
        assert_eq!(e.seq, s_isn + 1);
        assert_eq!(e.ack, c_isn + 1 + data.len() as u32); // acks the data
    }

    #[test]
    fn listener_pipelines_multiple_in_flight() {
        // Three data segments arrive back-to-back without the peer acking the echoes. The byte
        // buffer holds them, so all three are echoed (multiple in flight) — the OLD one-segment
        // model would dup-ACK the 2nd and 3rd instead.
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let segs: [&[u8]; 3] = [b"AAA", b"BBBB", b"CC"];
        let mut cseq = c_isn + 1;
        let mut eseq = s_isn + 1;
        let mut echoed: Vec<u8> = Vec::new();
        for s in segs {
            let n = lframe(7, cseq, s_isn + 1, PSH | ACK, 4096, s, &mut inb);
            let e = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
            assert_eq!(e.payload, s); // each echoed
            assert_eq!(e.seq, eseq); // contiguous, even though earlier echoes are still unacked
            echoed.extend_from_slice(&e.payload);
            cseq += s.len() as u32;
            eseq += s.len() as u32;
        }
        assert_eq!(echoed, b"AAABBBBCC");
    }

    #[test]
    fn listener_respects_peer_window() {
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let data = b"AAAAAAAAAA"; // 10 bytes
        // Peer advertises a window of only 4: we may echo at most 4 bytes until it opens.
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK, 4, data, &mut inb);
        let e = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
        assert_eq!(e.payload.len(), 4);
        assert_eq!(e.seq, s_isn + 1);
        // Window full -> tick can't push more.
        assert!(pump(&mut l, 1003, &mut out).is_none());
        // Peer ACKs the 4 bytes and opens the window -> the remaining 6 go out.
        let n = lframe(7, c_isn + 1 + 10, s_isn + 1 + 4, ACK, 100, &[], &mut inb);
        let e2 = feed(&mut l, &inb[..n], 1004, &mut out).unwrap();
        assert_eq!(e2.payload.len(), 6);
        assert_eq!(e2.seq, s_isn + 1 + 4);
    }

    #[test]
    fn listener_retransmits_on_timeout() {
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let data = b"retransmit-me";
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK, 4096, data, &mut inb);
        let e1 = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
        assert_eq!(e1.payload, data);
        // Before the RTO: nothing to do (all sent, none queued, timer not expired).
        assert!(pump(&mut l, 1100, &mut out).is_none());
        // After the RTO: the same segment is retransmitted.
        let e2 = pump(&mut l, 1002 + RTO_INIT_TICKS + 1, &mut out).unwrap();
        assert_eq!(e2.payload, data);
        assert_eq!(e2.seq, e1.seq);
    }

    #[test]
    fn listener_reassembles_out_of_order() {
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let (a, b): (&[u8], &[u8]) = (b"AAAA", b"BBBB");
        let a_seq = c_isn + 1;
        let b_seq = c_isn + 1 + a.len() as u32;
        // B first (a gap) -> buffered, dup-ACK still expecting A.
        let n = lframe(7, b_seq, s_isn + 1, PSH | ACK, 4096, b, &mut inb);
        let dup = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
        assert!(dup.payload.is_empty());
        assert_eq!(dup.ack, c_isn + 1);
        // A fills the gap -> A and B reassemble and echo (coalesced into one segment), acking both.
        let n = lframe(7, a_seq, s_isn + 1, PSH | ACK, 4096, a, &mut inb);
        let e = feed(&mut l, &inb[..n], 1003, &mut out).unwrap();
        assert_eq!(e.payload, b"AAAABBBB");
        assert_eq!(e.ack, c_isn + 1 + (a.len() + b.len()) as u32);
    }

    #[test]
    fn listener_closes_on_fin() {
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let data = b"bye";
        // data + FIN: we echo the data first (ack covers data + FIN), then send our FIN.
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK | FIN, 4096, data, &mut inb);
        let e = feed(&mut l, &inb[..n], 1002, &mut out).unwrap();
        assert_eq!(e.payload, data);
        assert_eq!(e.flags & FIN, 0); // FIN follows once the echo data has drained
        assert_eq!(e.ack, c_isn + 1 + data.len() as u32 + 1);
        let fin = pump(&mut l, 1003, &mut out).unwrap();
        assert_eq!(fin.flags & FIN, FIN);
        assert_eq!(fin.seq, s_isn + 1 + data.len() as u32);
        // Peer ACKs our FIN -> the slot is freed.
        assert_eq!(l.active_conns(), 1);
        let fin_ack = s_isn + 1 + data.len() as u32 + 1;
        let n = lframe(7, c_isn + 1 + data.len() as u32 + 1, fin_ack, ACK, 4096, &[], &mut inb);
        let _ = feed(&mut l, &inb[..n], 1004, &mut out);
        assert_eq!(l.active_conns(), 0);
    }

    #[test]
    fn listener_zero_window_does_not_wedge() {
        // A peer advertises a window smaller than the data it sends, then stalls at a zero window.
        // The leftover queued echo bytes can't be sent — but the connection must NOT wedge holding
        // a slot forever: the retransmit path coarsely probes and MAX_RETRIES eventually reaps it.
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let data = b"AAAAAAAAAA"; // 10 bytes, peer window only 4
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK, 4, data, &mut inb);
        let e = feed(&mut l, &inb[..n], 1000, &mut out).unwrap();
        assert_eq!(e.payload.len(), 4); // 4 echoed, 6 queued
        // Peer ACKs the 4 echoed bytes but advertises a ZERO window and then goes silent.
        let n = lframe(7, c_isn + 1 + 10, s_isn + 1 + 4, ACK, 0, &[], &mut inb);
        let _ = feed(&mut l, &inb[..n], 1001, &mut out);
        assert_eq!(l.active_conns(), 1);
        // Tick past successive (backed-off) RTOs: the queued data is force-sent (a coarse probe)
        // and the silent peer is reaped instead of leaking the slot.
        let mut now = 1001u64;
        let mut probed = false;
        for _ in 0..(MAX_RETRIES as usize + 2) {
            now += RTO_MAX_TICKS + 1; // jump past any backed-off deadline
            if let Some(seg) = pump(&mut l, now, &mut out) {
                if !seg.payload.is_empty() {
                    probed = true; // queued bytes re-sent despite the zero window
                }
            }
            if l.active_conns() == 0 {
                break;
            }
        }
        assert!(probed, "queued data must be force-sent, not wedged");
        assert_eq!(l.active_conns(), 0, "a silent zero-window peer must be reaped, not leak a slot");
    }

    #[test]
    fn listener_advertises_honest_window() {
        // The advertised window shrinks as the echo send buffer fills with unacked data.
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let chunk = [0x5a_u8; 512];
        let n = lframe(7, c_isn + 1, s_isn + 1, PSH | ACK, 60000, &chunk, &mut inb);
        let e1 = feed(&mut l, &inb[..n], 1000, &mut out).unwrap();
        assert_eq!(e1.window, (SND_BUF - 512) as u16); // honest: free space after buffering 512
        let n = lframe(7, c_isn + 1 + 512, s_isn + 1, PSH | ACK, 60000, &chunk, &mut inb);
        let e2 = feed(&mut l, &inb[..n], 1000, &mut out).unwrap();
        assert_eq!(e2.window, (SND_BUF - 1024) as u16);
    }

    #[test]
    fn listener_window_update_on_reopen() {
        // Fill the echo send buffer (advertise window 0), then ACK it: the listener must emit a
        // WINDOW-UPDATE ACK so a peer that paused on the zero window resumes. Without it, deadlock.
        let c_isn = 0x100;
        let (mut l, s_isn) = handshake(c_isn);
        let mut inb = [0u8; 1600];
        let mut out = [0u8; 1600];
        let chunk = [0x5a_u8; 512];
        let mut cseq = c_isn + 1;
        let mut last = None;
        for _ in 0..8 {
            let n = lframe(7, cseq, s_isn + 1, PSH | ACK, 60000, &chunk, &mut inb);
            let e = feed(&mut l, &inb[..n], 1000, &mut out).unwrap();
            cseq += 512;
            let w = e.window;
            last = Some(e);
            if w == 0 {
                break;
            }
        }
        assert_eq!(last.unwrap().window, 0, "send buffer fills -> advertised window reaches 0");
        let buffered = SND_BUF as u32; // 2048 bytes echoed and unacked
        // Peer (paused on the zero window) now ACKs everything echoed; nothing else is pending.
        let n = lframe(7, c_isn + 1 + buffered, s_isn + 1 + buffered, ACK, 60000, &[], &mut inb);
        let upd = feed(&mut l, &inb[..n], 1001, &mut out).unwrap();
        assert!(upd.payload.is_empty(), "window-update is a pure ACK");
        assert!(upd.window > 0, "window reopened");
        assert_eq!(upd.ack, c_isn + 1 + buffered, "acks the data we received");
    }

    // --- TcpClient streaming/linger receive ---

    /// Build the Ethernet/IPv4/TCP frame a peer would send to `c` (src = remote, dst = us). The
    /// client verifies the IPv4 checksum (computed here) but not the TCP checksum.
    fn peer_seg(c: &TcpClient, flags: u8, seq: u32, ack: u32, payload: &[u8]) -> ([u8; 1600], usize) {
        let mut buf = [0u8; 1600];
        let tcp_len = write_segment(
            &mut buf[34..], c.remote_port, c.local_port, seq, ack, flags, 4096, c.remote_ip, OUR_IP, payload,
        )
        .unwrap();
        ipv4::write_header(&mut buf[14..34], c.remote_ip, OUR_IP, PROTO_TCP, tcp_len).unwrap();
        ethernet::write_header(&mut buf[0..14], OUR_MAC, c.remote_mac, EtherType::Ipv4.as_u16()).unwrap();
        (buf, 14 + 20 + tcp_len)
    }

    #[test]
    fn client_streaming_captures_multi_segment_then_closes_on_fin() {
        // Streaming mode keeps recording across many segments and closes cleanly when the SERVER
        // closes first (passive close) — the common request/response (e.g. HTTP) pattern.
        let mut c = TcpClient::new(40000, [10, 0, 2, 2], [1, 2, 3, 4, 5, 6], 7778, 0x5000, b"REQ").streaming();
        let mut out = [0u8; 1600];
        assert!(c.open(&mut out, OUR_IP, OUR_MAC).is_some());
        assert_eq!(c.state(), ClientState::SynSent);
        // SYN-ACK acking our SYN (snd_nxt = 0x5001) -> client sends the request "REQ".
        let pisn = 0x8000u32;
        let (f, n) = peer_seg(&c, SYN | ACK, pisn, 0x5001, b"");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        assert_eq!(c.state(), ClientState::Established);
        let our_snd = 0x5004u32; // 0x5001 + 3 (the request)

        // Peer streams three in-order segments; the client ACKs each and keeps receiving.
        let parts: [&[u8]; 3] = [b"AAAA", b"BBBBBB", b"CC"];
        let mut pseq = pisn.wrapping_add(1);
        for part in parts {
            let (f, n) = peer_seg(&c, PSH | ACK, pseq, our_snd, part);
            assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
            assert_eq!(c.state(), ClientState::Established);
            pseq = pseq.wrapping_add(part.len() as u32);
        }
        assert_eq!(c.rx_data(), b"AAAABBBBBBCC");

        // Peer FIN -> client consumes it and sends its own FIN (FinWait).
        let (f, n) = peer_seg(&c, FIN | ACK, pseq, our_snd, b"");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        assert_eq!(c.state(), ClientState::FinWait);
        // Peer's bare ACK of our FIN (our snd_nxt = 0x5005) completes the passive close -> Done.
        let (f, n) = peer_seg(&c, ACK, pseq.wrapping_add(1), 0x5005, b"");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_none());
        assert_eq!(c.state(), ClientState::Done);
        assert!(c.is_done());
    }

    #[test]
    fn client_streaming_closes_when_buffer_full() {
        // A response larger than the receive buffer is captured up to the cap, then we close
        // (rather than advertising a zero window and stalling with no app to drain it). Uses
        // 500-byte segments so the one crossing the 2048 cap is TRUNCATED (rcv_nxt lands
        // mid-segment) — then drives the teardown to Done, which would wedge in FinWait if the
        // buffer-full close didn't relax its completion condition (the bug this guards).
        let mut c = TcpClient::new(40000, [10, 0, 2, 2], [1, 2, 3, 4, 5, 6], 7778, 0x5000, b"R").streaming();
        let mut out = [0u8; 4096];
        assert!(c.open(&mut out, OUR_IP, OUR_MAC).is_some());
        let pisn = 0x8000u32;
        let (f, n) = peer_seg(&c, SYN | ACK, pisn, 0x5001, b""); // acks our SYN -> sends "R", snd_nxt 0x5002
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        let chunk = [0x41u8; 500];
        let mut pseq = pisn.wrapping_add(1);
        let mut hit_full = false;
        for _ in 0..6 {
            let (f, n) = peer_seg(&c, PSH | ACK, pseq, 0x5002, &chunk);
            c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out);
            pseq = pseq.wrapping_add(500);
            if c.state() == ClientState::FinWait {
                hit_full = true;
                break;
            }
        }
        assert!(hit_full);
        assert_eq!(c.rx_data().len(), CLIENT_RX_CAP); // captured exactly the cap (4*500 + 48)
        // The peer ACKs our FIN (snd_nxt = "R"(1)+FIN(1) past SYN => 0x5003). This previously
        // wedged: the peer's FIN sits past our mid-stream rcv_nxt, so only a bare-ACK completion
        // reaches Done.
        let (f, n) = peer_seg(&c, ACK, pseq, 0x5003, b"");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_none());
        assert_eq!(c.state(), ClientState::Done);
        assert!(c.is_done());
    }

    #[test]
    fn client_oneshot_closes_after_echo() {
        // Regression: the default one-shot client still closes once it has received its echo back
        // (active close), completing on the peer's FIN — unchanged by the streaming additions.
        let mut c = TcpClient::new(40000, [10, 0, 2, 2], [1, 2, 3, 4, 5, 6], 7, 0x5000, b"hello");
        let mut out = [0u8; 1600];
        assert!(c.open(&mut out, OUR_IP, OUR_MAC).is_some());
        let pisn = 0x8000u32;
        let (f, n) = peer_seg(&c, SYN | ACK, pisn, 0x5001, b""); // -> sends "hello", snd_nxt 0x5006
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        assert_eq!(c.state(), ClientState::Established);
        // Echo of "hello": rx_len(5) >= tx_len(5) -> client closes (FIN), FinWait.
        let (f, n) = peer_seg(&c, PSH | ACK, pisn.wrapping_add(1), 0x5006, b"hello");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        assert_eq!(c.state(), ClientState::FinWait);
        // Peer FIN|ACK (active-close completion, Case A) -> Done.
        let (f, n) = peer_seg(&c, FIN | ACK, pisn.wrapping_add(6), 0x5007, b"");
        assert!(c.handle(&f[..n], OUR_IP, OUR_MAC, &mut out).is_some());
        assert_eq!(c.state(), ClientState::Done);
    }

    // --- SendRing (the listener's byte-stream send buffer) ---

    #[test]
    fn sendring_push_and_queries() {
        let mut r = SendRing::new(100);
        assert_eq!(r.free(), SND_BUF);
        assert_eq!(r.push(b"hello"), 5);
        assert_eq!((r.free(), r.len, r.flight_len()), (SND_BUF - 5, 5, 0));
        assert!(!r.fully_sent()); // 5 bytes queued, none sent
        assert_eq!(r.end_seq(), 105);
    }

    #[test]
    fn sendring_push_bounded_by_free() {
        let mut r = SendRing::new(0);
        let big = [0x41u8; SND_BUF + 100];
        assert_eq!(r.push(&big), SND_BUF); // capped at capacity
        assert_eq!(r.free(), 0);
        assert_eq!(r.push(b"x"), 0); // full -> nothing accepted
    }

    #[test]
    fn sendring_peek_respects_mss_window_and_queue() {
        let mut r = SendRing::new(1000);
        r.push(&[0x42u8; 800]); // 800 queued at [1000,1800)
        let mut out = [0u8; 2048];
        assert_eq!(r.peek_seg(60000, &mut out), MSS); // big window -> MSS-capped
        assert_eq!(r.peek_seg(100, &mut out), 100); // small window -> window-capped
        assert_eq!(r.peek_seg(0, &mut out), 0); // zero window -> nothing
    }

    #[test]
    fn sendring_window_slides() {
        let mut r = SendRing::new(1000);
        r.push(&[0x43u8; 1000]); // [1000,2000)
        let mut out = [0u8; 2048];
        let n1 = r.peek_seg(700, &mut out);
        assert_eq!(n1, MSS); // 512
        r.mark_sent(n1);
        assert_eq!((r.nxt, r.flight_len()), (1512, 512));
        let n2 = r.peek_seg(700, &mut out); // remaining window 700-512 = 188
        assert_eq!(n2, 188);
        r.mark_sent(n2);
        assert_eq!(r.flight_len(), 700);
        assert_eq!(r.peek_seg(700, &mut out), 0); // window full
        assert!(!r.fully_sent()); // 300 still queued
    }

    #[test]
    fn sendring_ack_frees_and_clamps() {
        let mut r = SendRing::new(1000);
        r.push(&[0x44u8; 600]); // [1000,1600)
        let mut out = [0u8; 2048];
        let n = r.peek_seg(60000, &mut out);
        r.mark_sent(n); // sent 512 -> [1000,1512) in flight
        assert_eq!(r.ack(1300), 300); // frees 300
        assert_eq!((r.una, r.len, r.flight_len()), (1300, 300, 212));
        assert_eq!(r.ack(1300), 0); // duplicate
        assert_eq!(r.ack(1200), 0); // old
        assert_eq!(r.ack(1601), 300); // ack past data (e.g. FIN ack) clamps to the 300 data bytes
        assert_eq!((r.una, r.len), (1600, 0));
        assert_eq!(r.flight_len(), 0);
    }

    #[test]
    fn sendring_rewind_resends() {
        let mut r = SendRing::new(1000);
        r.push(&[0x45u8; 400]); // 'E'
        let mut out = [0u8; 2048];
        let n = r.peek_seg(60000, &mut out);
        r.mark_sent(n);
        assert_eq!(r.flight_len(), 400);
        r.rewind(); // RTO: resend from una
        assert_eq!((r.nxt, r.flight_len()), (1000, 0));
        let n2 = r.peek_seg(60000, &mut out);
        assert_eq!(n2, 400);
        assert_eq!(&out[..3], b"EEE");
    }

    #[test]
    fn sendring_content_survives_wrap() {
        // Drive the head near the end of the ring, then push data that wraps the boundary and
        // read it back to prove the ring indexing is correct across the wrap.
        let mut r = SendRing::new(0);
        let mut out = [0u8; 2048];
        r.push(&[0u8; 2000]);
        while !r.fully_sent() {
            let k = r.peek_seg(60000, &mut out);
            r.mark_sent(k);
        }
        r.ack(r.nxt); // free all 2000 -> head now at index 2000, una = 2000
        assert_eq!((r.len, r.head), (0, 2000));
        let data: [u8; 100] = core::array::from_fn(|i| i as u8);
        assert_eq!(r.push(&data), 100); // stored across [2000,2048) then [0,52)
        let n = r.peek_seg(60000, &mut out);
        assert_eq!(n, 100);
        assert_eq!(&out[..100], &data[..]);
    }
}
