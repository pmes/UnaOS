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
/// IP/port are fixed by the listener). Each connection keeps a single outstanding (unacked)
/// segment for retransmission — the echo is naturally lockstep, so a one-segment send window
/// suffices.
struct TcpConn {
    state: ConnState,
    remote_ip: [u8; 4],
    remote_mac: [u8; 6],
    remote_port: u16,
    snd_nxt: u32, // next sequence number we will send
    rcv_nxt: u32, // next sequence number we expect to receive
    // Retransmission of the single outstanding segment.
    unacked: bool,
    un_seq: u32,      // sequence number of the outstanding segment
    un_flags: u8,     // its flags (SYN-ACK / data echo / FIN)
    un_paylen: usize, // payload bytes buffered in un_buf
    un_buf: [u8; RETX_CAP],
    un_sent_at: u64, // tick the outstanding segment was first sent (for RTT sampling)
    un_retx: bool,   // it was retransmitted — Karn's algorithm excludes it from RTT sampling
    rto_deadline: u64, // tick at which to retransmit if still unacked
    retries: u8,
    // Adaptive RTO state (RFC 6298), in ticks. `rto` carries over between segments and only
    // collapses back toward the floor when a clean (non-retransmitted) RTT sample arrives.
    srtt: u32,       // smoothed round-trip time
    rttvar: u32,     // round-trip time variation
    rto: u32,        // current retransmission timeout
    rtt_valid: bool, // whether srtt/rttvar hold a real measurement yet
    // Reassembly of out-of-order segments (up to OOO_EXTENTS extents ahead of rcv_nxt). Each is
    // buffered on arrival and delivered (echoed) once the gap before it fills and the send
    // window is free; extents drain in sequence order, one per freed window.
    ooo: [OooExtent; OOO_EXTENTS],
}

impl TcpConn {
    fn new(state: ConnState, remote_ip: [u8; 4], remote_mac: [u8; 6], remote_port: u16, snd_nxt: u32, rcv_nxt: u32) -> Self {
        Self {
            state, remote_ip, remote_mac, remote_port, snd_nxt, rcv_nxt,
            unacked: false, un_seq: 0, un_flags: 0, un_paylen: 0, un_buf: [0; RETX_CAP],
            un_sent_at: 0, un_retx: false,
            rto_deadline: 0, retries: 0,
            srtt: 0, rttvar: 0, rto: RTO_INIT_TICKS as u32, rtt_valid: false,
            ooo: [OooExtent::EMPTY; OOO_EXTENTS],
        }
    }

    fn matches(&self, ip: [u8; 4], port: u16) -> bool {
        self.remote_ip == ip && self.remote_port == port
    }

    /// Emit one segment, building the full Eth/IPv4/TCP frame into `out`. `seq` and `ack` are
    /// passed explicitly (rather than read from `self`) so a data echo can ACK the bytes it
    /// carries — and a retransmission can re-send the *original* seq — while the `rcv_nxt`/
    /// `snd_nxt` advance is committed only *after* serialization succeeds (the no-desync
    /// invariant).
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        window: u16,
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
            window,
            our_ip,
            self.remote_ip,
            payload,
        )?;
        ipv4::write_header(out.get_mut(14..34)?, our_ip, self.remote_ip, PROTO_TCP, tcp_len)?;
        ethernet::write_header(out.get_mut(0..14)?, self.remote_mac, our_mac, EtherType::Ipv4.as_u16())?;
        Some(14 + 20 + tcp_len)
    }

    /// Record a just-sent seq-consuming segment as the outstanding one and arm its RTO timer at
    /// the connection's current (adaptive) `rto`. Stamps the send time and clears the
    /// retransmitted flag so a clean ACK can later sample this segment's RTT (Karn's algorithm).
    fn arm(&mut self, now: u64, seq: u32, flags: u8, payload: &[u8]) {
        self.un_seq = seq;
        self.un_flags = flags;
        let n = payload.len().min(RETX_CAP);
        self.un_buf[..n].copy_from_slice(&payload[..n]);
        self.un_paylen = n;
        self.unacked = true;
        self.retries = 0;
        self.un_sent_at = now;
        self.un_retx = false;
        self.rto_deadline = now + self.rto as u64;
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

    /// Re-send the outstanding segment on RTO expiry. Applies Karn's algorithm: marks the
    /// segment as retransmitted (so its eventual ACK won't be sampled as RTT) and backs the RTO
    /// off by doubling, capped at [`RTO_MAX_TICKS`] (RFC 6298 §5.5). Returns the frame length, or
    /// `None` if it could not be serialized.
    fn retransmit(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, window: u16, now: u64) -> Option<usize> {
        self.retries = self.retries.saturating_add(1);
        self.un_retx = true;
        self.rto = ((self.rto as u64 * 2).min(RTO_MAX_TICKS)) as u32;
        self.rto_deadline = now + self.rto as u64;
        let (flags, seq, ack, paylen) = (self.un_flags, self.un_seq, self.rcv_nxt, self.un_paylen);
        self.emit(out, our_ip, our_mac, local_port, window, flags, seq, ack, &self.un_buf[..paylen])
    }

    /// Drive this connection with one received segment. Returns `(response, done)`: `response`
    /// is the segment to transmit (if any), and `done == true` means the connection has fully
    /// closed and the listener should free its slot.
    #[allow(clippy::too_many_arguments)]
    fn on_segment(
        &mut self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        window: u16,
        now: u64,
        flags: u8,
        their_seq: u32,
        their_ack: u32,
        payload: &[u8],
    ) -> (Option<usize>, bool) {
        // Cumulative ACK: if the peer acknowledges our outstanding segment, stop retransmitting.
        // Karn's algorithm: only the ACK of a segment that was *not* retransmitted gives an
        // unambiguous RTT sample (otherwise we can't tell which transmission it acknowledges).
        if flags & ACK != 0 && self.unacked && their_ack == self.snd_nxt {
            if !self.un_retx {
                self.update_rtt(now.saturating_sub(self.un_sent_at));
            }
            self.unacked = false;
        }
        match self.state {
            ConnState::SynRcvd => {
                // Expect the ACK completing the handshake (it may already carry data).
                if flags & ACK != 0 && their_ack == self.snd_nxt {
                    self.state = ConnState::Established;
                    let resp = self.on_data(out, our_ip, our_mac, local_port, window, now, their_seq, flags, payload);
                    (resp, false)
                } else {
                    (None, false)
                }
            }
            ConnState::Established => {
                // Process this segment, then — if the cumulative ACK above just freed our
                // one-segment send window — deliver a buffered out-of-order segment that has
                // become in-order. We must NOT gate this on on_data() returning None: a normal
                // peer's window-freeing ACK sits at SND.NXT (past the buffered extent), which
                // on_data() sees as "future" and answers with a dup-ACK. So attempt drain()
                // regardless and PREFER its echo over a dup-ACK (only one reply per RX frame);
                // if drain has nothing to deliver it returns None and we keep on_data's response.
                let resp = self.on_data(out, our_ip, our_mac, local_port, window, now, their_seq, flags, payload);
                let resp = self.drain(out, our_ip, our_mac, local_port, window, now).or(resp);
                (resp, false)
            }
            ConnState::LastAck => {
                // Our FIN is acknowledged — connection closed; free the slot.
                if flags & ACK != 0 && their_ack == self.snd_nxt {
                    (None, true)
                } else {
                    (None, false)
                }
            }
        }
    }

    /// Established-state handling of an in-order segment that may carry data and/or FIN.
    /// Folds the data echo, its ACK, and (if present) our FIN into a single response segment.
    #[allow(clippy::too_many_arguments)]
    fn on_data(
        &mut self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        local_port: u16,
        window: u16,
        now: u64,
        their_seq: u32,
        flags: u8,
        payload: &[u8],
    ) -> Option<usize> {
        let has_fin = flags & FIN != 0;

        // Future segment (a gap ahead of rcv_nxt): buffer it as an out-of-order extent for later
        // reassembly, then duplicate-ACK rcv_nxt to signal the gap. We hold up to OOO_EXTENTS
        // distinct extents — once they are all full a further, different future segment is
        // dropped (the peer retransmits it).
        if seq_gt(their_seq, self.rcv_nxt) {
            self.buffer_ooo(their_seq, payload, has_fin);
            return self.emit(out, our_ip, our_mac, local_port, window, ACK, self.snd_nxt, self.rcv_nxt, &[]);
        }
        // Old / already-received (a retransmit below rcv_nxt): duplicate-ACK.
        if seq_lt(their_seq, self.rcv_nxt) {
            return self.emit(out, our_ip, our_mac, local_port, window, ACK, self.snd_nxt, self.rcv_nxt, &[]);
        }
        // Otherwise their_seq == rcv_nxt: this segment is in order.

        if payload.is_empty() && !has_fin {
            return None; // a bare ACK — nothing to send
        }

        // One outstanding segment at a time (a one-segment send window), and only accept what
        // fits our advertised window / retransmit buffer. Otherwise dup-ACK and let the peer
        // retry once the in-flight segment is acknowledged — no data is consumed, so nothing is
        // lost. NOTE: we keep advertising a constant non-zero window even while `unacked`, so a
        // *pipelining* peer's extra segment is dropped+dup-ACKed (then retransmitted) rather
        // than withheld. That's safe but not optimal; honest zero-window flow control (window
        // updates, a persist timer) is deferred to the window-management work. The echo workload
        // is lockstep, so this path is essentially never hit in practice.
        if self.unacked || payload.len() > RETX_CAP {
            return self.emit(out, our_ip, our_mac, local_port, window, ACK, self.snd_nxt, self.rcv_nxt, &[]);
        }

        // Echo the data back; set FIN if the peer is closing (we have nothing more to send).
        let mut resp_flags = ACK | PSH;
        if has_fin {
            resp_flags |= FIN;
        }
        // ACK the data (and FIN) being responded to: compute the post-consume rcv_nxt. Serialize
        // FIRST, commit the sequence advance only on success (no-desync), then arm retransmission.
        let mut new_rcv = self.rcv_nxt.wrapping_add(payload.len() as u32);
        if has_fin {
            new_rcv = new_rcv.wrapping_add(1); // a FIN consumes one sequence number
        }
        let seq = self.snd_nxt;
        let seqlen = payload.len() as u32 + if has_fin { 1 } else { 0 };
        let n = self.emit(out, our_ip, our_mac, local_port, window, resp_flags, seq, new_rcv, payload)?;
        self.rcv_nxt = new_rcv;
        self.snd_nxt = self.snd_nxt.wrapping_add(seqlen);
        self.arm(now, seq, resp_flags, payload);
        if has_fin {
            self.state = ConnState::LastAck;
        }
        Some(n)
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

    /// Deliver the buffered out-of-order extent that has become in-order, now that the send
    /// window is free (an ACK cleared the outstanding segment). Echoes it exactly like fresh
    /// in-order data and frees its slot. At most one extent is delivered per call (one reply per
    /// RX frame / one-segment send window); successive extents drain in sequence order across
    /// successive freed windows. Returns the echo response, or `None` if there is nothing to
    /// deliver.
    #[allow(clippy::too_many_arguments)]
    fn drain(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, window: u16, now: u64) -> Option<usize> {
        // Discard extents already covered by rcv_nxt (stale / superseded by a retransmission).
        for e in self.ooo.iter_mut() {
            if e.used && seq_lt(e.seq, self.rcv_nxt) {
                e.used = false;
            }
        }
        if self.unacked {
            return None;
        }
        // Find the extent that exactly fills the current gap (seq == rcv_nxt).
        let idx = self.ooo.iter().position(|e| e.used && e.seq == self.rcv_nxt)?;
        let len = self.ooo[idx].len;
        let has_fin = self.ooo[idx].fin;
        let mut tmp = [0u8; RETX_CAP];
        tmp[..len].copy_from_slice(&self.ooo[idx].buf[..len]);

        let mut resp_flags = ACK | PSH;
        if has_fin {
            resp_flags |= FIN;
        }
        let mut new_rcv = self.rcv_nxt.wrapping_add(len as u32);
        if has_fin {
            new_rcv = new_rcv.wrapping_add(1);
        }
        let seq = self.snd_nxt;
        let seqlen = len as u32 + if has_fin { 1 } else { 0 };
        // Serialize first; commit + free the extent only on success (no-desync).
        let n = self.emit(out, our_ip, our_mac, local_port, window, resp_flags, seq, new_rcv, &tmp[..len])?;
        self.rcv_nxt = new_rcv;
        self.snd_nxt = self.snd_nxt.wrapping_add(seqlen);
        self.arm(now, seq, resp_flags, &tmp[..len]);
        self.ooo[idx].used = false;
        if has_fin {
            self.state = ConnState::LastAck;
        }
        Some(n)
    }
}

/// Number of simultaneous connections the echo listener accepts.
pub const MAX_CONNS: usize = 4;

/// A multi-connection TCP echo listener (RFC 862). Accepts up to [`MAX_CONNS`] passive-open
/// connections at once, demultiplexing inbound segments to a fixed table of [`TcpConn`] slots
/// by the remote `(IP, port)`. Per-connection scope: in-order data, a one-segment send window
/// with retransmission ([`tick`](Self::tick) drives the RTO timers), no options, lenient
/// receiver. No allocation (fixed table).
pub struct TcpListener {
    listen_port: u16,
    window: u16,
    isn: u32, // ramps per accepted connection to avoid TIME_WAIT collisions
    conns: [Option<TcpConn>; MAX_CONNS],
}

impl TcpListener {
    pub fn new(listen_port: u16) -> Self {
        Self {
            listen_port,
            window: RETX_CAP as u16,
            isn: 0x0001_0000,
            conns: core::array::from_fn(|_| None),
        }
    }

    /// Number of currently-active connections (for diagnostics).
    pub fn active_conns(&self) -> usize {
        self.conns.iter().filter(|c| c.is_some()).count()
    }

    /// Drive retransmission timers. Call periodically with the current monotonic `now`. If a
    /// connection's outstanding segment has passed its RTO, retransmit ONE such segment (with
    /// exponential backoff) and return its frame; after [`MAX_RETRIES`] the connection is
    /// abandoned (slot freed), which also bounds stuck half-open (SynRcvd) connections.
    pub fn tick(&mut self, now: u64, our_ip: [u8; 4], our_mac: [u8; 6], out: &mut [u8]) -> Option<usize> {
        let (local_port, window) = (self.listen_port, self.window);
        for slot in self.conns.iter_mut() {
            let conn = match slot {
                Some(c) => c,
                None => continue,
            };
            if !conn.unacked || now < conn.rto_deadline {
                continue;
            }
            if conn.retries >= MAX_RETRIES {
                *slot = None; // give up — frees half-open or stuck connections
                continue;
            }
            return conn.retransmit(out, our_ip, our_mac, local_port, window, now);
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
            let (local_port, window) = (self.listen_port, self.window);
            let conn = self.conns[idx].as_mut().unwrap();
            let (resp, done) =
                conn.on_segment(out, our_ip, our_mac, local_port, window, now, flags, their_seq, their_ack, payload);
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
            let (local_port, window) = (self.listen_port, self.window);
            // Emit first; only commit the slot once the SYN-ACK serialized. SYN-ACK seq is our
            // ISN, ack is the peer's SYN + 1; then arm it for retransmission (half-open timeout).
            let seq = conn.snd_nxt;
            let n = conn.emit(out, our_ip, our_mac, local_port, window, SYN | ACK, seq, conn.rcv_nxt, &[])?;
            conn.snd_nxt = conn.snd_nxt.wrapping_add(1); // our SYN consumes one
            conn.arm(now, seq, SYN | ACK, &[]);
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

/// Maximum one-shot payload an outbound connection sends after the handshake.
pub const CLIENT_TX_CAP: usize = 64;
/// Maximum bytes of peer response an outbound connection records.
pub const CLIENT_RX_CAP: usize = 256;

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
        }
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
                }
                // Close once we've received at least our payload back (the echo) or the peer
                // is closing in-order; otherwise just ACK and keep waiting.
                if self.rx_len >= self.tx_len || fin_in_order {
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
                // The peer's FIN completes the teardown — only in-order. Record any data the
                // peer coalesced with its FIN before acking, so it isn't silently dropped.
                if flags & FIN != 0 && their_seq == self.rcv_nxt {
                    if !payload.is_empty() {
                        let n = self.record(payload);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
                    }
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1); // FIN consumes one
                    self.state = ClientState::Done;
                    self.emit(out, our_ip, our_mac, ACK, &[])
                } else {
                    // A bare ACK of our FIN, or an out-of-order/duplicate FIN — nothing to send.
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

    fn test_conn() -> TcpConn {
        TcpConn::new(ConnState::Established, [10, 0, 2, 2], [1, 2, 3, 4, 5, 6], 40000, 0x1000, 0x2000)
    }

    #[test]
    fn arm_uses_adaptive_rto_and_clears_retx() {
        let mut c = test_conn();
        c.arm(1000, 0x1000, ACK | PSH, b"hi");
        assert!(c.unacked);
        assert!(!c.un_retx);
        assert_eq!(c.un_sent_at, 1000);
        assert_eq!(c.rto, RTO_INIT_TICKS as u32);
        assert_eq!(c.rto_deadline, 1000 + RTO_INIT_TICKS);
    }

    #[test]
    fn retransmit_applies_karn_backoff() {
        let mut c = test_conn();
        let mut out = [0u8; 1500];
        let (ip, mac) = ([10, 0, 2, 15], [9, 9, 9, 9, 9, 9]);
        c.arm(1000, 0x1000, ACK | PSH, b"hi");
        // Exponential backoff by doubling, capped at RTO_MAX_TICKS; each retransmit marks the
        // segment ambiguous so its ACK won't be RTT-sampled (Karn).
        let mut now = 1300;
        for expect in [400u64, 800, 1600, 2000, 2000] {
            assert!(c.retransmit(&mut out, ip, mac, 7, 512, now).is_some());
            assert!(c.un_retx);
            assert_eq!(c.rto as u64, expect);
            assert_eq!(c.rto_deadline, now + expect);
            now += expect;
        }
        assert_eq!(c.retries, 5);
    }

    #[test]
    fn clean_sample_collapses_backed_off_rto() {
        // After backoff has inflated the RTO, the first clean RTT sample recomputes it from the
        // measurement (here: small RTT -> back to the floor), proving the timer self-corrects.
        let mut c = test_conn();
        let mut out = [0u8; 1500];
        c.arm(1000, 0x1000, ACK | PSH, b"hi");
        c.retransmit(&mut out, [10, 0, 2, 15], [9; 6], 7, 512, 1300);
        assert_eq!(c.rto, 400);
        c.update_rtt(10); // first real measurement
        assert!(c.rtt_valid);
        assert_eq!(c.rto, RTO_MIN_TICKS as u32);
    }

    // Reassembly tests use a fixed local identity; the values are irrelevant to the logic.
    const OUR_IP: [u8; 4] = [10, 0, 2, 15];
    const OUR_MAC: [u8; 6] = [9, 9, 9, 9, 9, 9];

    fn used_extents(c: &TcpConn) -> usize {
        c.ooo.iter().filter(|e| e.used).count()
    }

    #[test]
    fn ooo_buffers_multiple_extents_and_drains_in_order() {
        // rcv_nxt starts at 0x2000. Buffer B/C/D (the gap-filler A is missing), then deliver A
        // (by advancing rcv_nxt) and confirm B, C, D drain in sequence order, one per freed window.
        let mut c = test_conn();
        let mut out = [0u8; 1500];
        c.buffer_ooo(0x2004, b"BBBB", false);
        c.buffer_ooo(0x2008, b"CCCC", false);
        c.buffer_ooo(0x200C, b"DDDD", false);
        assert_eq!(used_extents(&c), 3);
        // No extent fills the gap at rcv_nxt yet.
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_none());

        // A arrives in order -> rcv_nxt reaches B's seq, window free.
        c.rcv_nxt = 0x2004;
        c.unacked = false;
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_some()); // B
        assert_eq!(c.rcv_nxt, 0x2008);
        assert!(c.unacked); // one-segment window now occupied by B's echo
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_none()); // window busy

        c.unacked = false;
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_some()); // C
        assert_eq!(c.rcv_nxt, 0x200C);
        c.unacked = false;
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_some()); // D
        assert_eq!(c.rcv_nxt, 0x2010);
        assert_eq!(used_extents(&c), 0); // all extents delivered
    }

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
    fn ooo_drain_discards_stale() {
        let mut c = test_conn();
        c.buffer_ooo(0x2004, b"BBBB", false);
        assert_eq!(used_extents(&c), 1);
        // rcv_nxt advanced past the buffered extent (e.g. it was superseded) -> drain discards it.
        c.rcv_nxt = 0x2010;
        c.unacked = false;
        let mut out = [0u8; 1500];
        assert!(c.drain(&mut out, OUR_IP, OUR_MAC, 7, 512, 1000).is_none());
        assert_eq!(used_extents(&c), 0);
    }
}
