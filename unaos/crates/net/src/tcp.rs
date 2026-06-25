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
//! in-order data only, no retransmission (relies on the peer over a reliable local link), no
//! window scaling/options, lenient receiver (incoming TCP checksum not verified; outgoing is).
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
/// Base retransmission timeout, in monotonic ticks. The APIC-timer tick is ~1 ms on QEMU
/// (empirically), so this is ~200 ms — conservative enough not to fire before a normal ACK on
/// any reasonable link, yet quick to recover. Doubled per retry (capped) for exponential
/// backoff. The absolute rate is uncalibrated, so these are coarse, not exact, durations.
const RTO_BASE_TICKS: u64 = 200;
/// Upper bound on a single backoff interval (~2 s).
const RTO_MAX_TICKS: u64 = 2000;
/// Cap on the backoff left-shift so it can't overflow.
const RTO_BACKOFF_SHIFT_CAP: u8 = 5;
/// Give up (free the connection) after this many retransmissions — this also bounds a half-open
/// (SynRcvd) connection whose handshake ACK never arrives, so it can't wedge a slot forever.
/// With the backoff above this is ~7 s to fully abandon a dead/half-open connection.
const MAX_RETRIES: u8 = 6;

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
    rto_deadline: u64, // tick at which to retransmit if still unacked
    retries: u8,
    // Reassembly of a single out-of-order segment (one extent ahead of rcv_nxt). Buffered on
    // arrival and delivered (echoed) once the gap fills and the send window is free.
    ooo: bool,
    ooo_seq: u32,
    ooo_len: usize,
    ooo_fin: bool,
    ooo_buf: [u8; RETX_CAP],
}

impl TcpConn {
    fn new(state: ConnState, remote_ip: [u8; 4], remote_mac: [u8; 6], remote_port: u16, snd_nxt: u32, rcv_nxt: u32) -> Self {
        Self {
            state, remote_ip, remote_mac, remote_port, snd_nxt, rcv_nxt,
            unacked: false, un_seq: 0, un_flags: 0, un_paylen: 0, un_buf: [0; RETX_CAP],
            rto_deadline: 0, retries: 0,
            ooo: false, ooo_seq: 0, ooo_len: 0, ooo_fin: false, ooo_buf: [0; RETX_CAP],
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

    /// Record a just-sent seq-consuming segment as the outstanding one and arm its RTO timer.
    fn arm(&mut self, now: u64, seq: u32, flags: u8, payload: &[u8]) {
        self.un_seq = seq;
        self.un_flags = flags;
        let n = payload.len().min(RETX_CAP);
        self.un_buf[..n].copy_from_slice(&payload[..n]);
        self.un_paylen = n;
        self.unacked = true;
        self.retries = 0;
        self.rto_deadline = now + RTO_BASE_TICKS;
    }

    /// Re-send the outstanding segment on RTO expiry, with exponential backoff. Returns the
    /// frame length, or `None` if it could not be serialized.
    fn retransmit(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, window: u16, now: u64) -> Option<usize> {
        self.retries = self.retries.saturating_add(1);
        let backoff = (RTO_BASE_TICKS << self.retries.min(RTO_BACKOFF_SHIFT_CAP)).min(RTO_MAX_TICKS);
        self.rto_deadline = now + backoff;
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
        if flags & ACK != 0 && self.unacked && their_ack == self.snd_nxt {
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

        // Future segment (a gap ahead of rcv_nxt): buffer it as the single out-of-order extent
        // for later reassembly, then duplicate-ACK rcv_nxt to signal the gap. We hold only one
        // extent — a second, different future segment is dropped (the peer retransmits it).
        if seq_gt(their_seq, self.rcv_nxt) {
            if (!payload.is_empty() || has_fin)
                && payload.len() <= RETX_CAP
                && (!self.ooo || self.ooo_seq == their_seq)
            {
                self.ooo = true;
                self.ooo_seq = their_seq;
                self.ooo_len = payload.len();
                self.ooo_fin = has_fin;
                self.ooo_buf[..payload.len()].copy_from_slice(payload);
            }
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

    /// Deliver a buffered out-of-order segment that has become in-order, now that the send
    /// window is free (an ACK cleared the outstanding segment). Echoes it exactly like fresh
    /// in-order data. Returns the echo response, or `None` if there is nothing to deliver.
    #[allow(clippy::too_many_arguments)]
    fn drain(&mut self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], local_port: u16, window: u16, now: u64) -> Option<usize> {
        // Discard a buffered extent already covered by rcv_nxt (stale).
        if self.ooo && seq_lt(self.ooo_seq, self.rcv_nxt) {
            self.ooo = false;
        }
        if self.unacked || !self.ooo || self.ooo_seq != self.rcv_nxt {
            return None;
        }
        let len = self.ooo_len;
        let has_fin = self.ooo_fin;
        let mut tmp = [0u8; RETX_CAP];
        tmp[..len].copy_from_slice(&self.ooo_buf[..len]);

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
        // Serialize first; commit + clear the extent only on success (no-desync).
        let n = self.emit(out, our_ip, our_mac, local_port, window, resp_flags, seq, new_rcv, &tmp[..len])?;
        self.rcv_nxt = new_rcv;
        self.snd_nxt = self.snd_nxt.wrapping_add(seqlen);
        self.arm(now, seq, resp_flags, &tmp[..len]);
        self.ooo = false;
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
