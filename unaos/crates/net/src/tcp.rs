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

//! Minimal hand-rolled TCP — just enough for a single-connection echo listener.
//!
//! Scope (deliberately small): one connection at a time, passive open only, in-order
//! data, no retransmission (relies on the peer over a reliable local link), no window
//! scaling/options, lenient receiver (incoming TCP checksum not verified; outgoing is).
//! State: Listen -> SynRcvd -> Established -> LastAck -> Listen. Each received segment
//! produces at most one response segment (the driver sends one reply per RX frame), which
//! is sufficient because we ACK/echo/FIN can each be folded into a single segment.

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Listen,
    SynRcvd,
    Established,
    LastAck,
}

/// A single-connection TCP echo listener.
pub struct TcpEcho {
    listen_port: u16,
    state: State,
    window: u16,
    isn: u32,        // ramps per connection to avoid TIME_WAIT collisions
    remote_ip: [u8; 4],
    remote_mac: [u8; 6],
    remote_port: u16,
    snd_nxt: u32, // next sequence number we will send
    rcv_nxt: u32, // next sequence number we expect to receive
}

impl TcpEcho {
    pub fn new(listen_port: u16) -> Self {
        Self {
            listen_port,
            state: State::Listen,
            window: 4096,
            isn: 0x0001_0000,
            remote_ip: [0; 4],
            remote_mac: [0; 6],
            remote_port: 0,
            snd_nxt: 0,
            rcv_nxt: 0,
        }
    }

    fn is_current_peer(&self, ip: [u8; 4], port: u16) -> bool {
        self.remote_ip == ip && self.remote_port == port
    }

    /// Emit one segment to the current peer, building the full Eth/IPv4/TCP frame into `out`.
    fn emit(&self, out: &mut [u8], our_ip: [u8; 4], our_mac: [u8; 6], flags: u8, payload: &[u8]) -> Option<usize> {
        let tcp_len = write_segment(
            out.get_mut(34..)?,
            self.listen_port,
            self.remote_port,
            self.snd_nxt,
            self.rcv_nxt,
            flags,
            self.window,
            our_ip,
            self.remote_ip,
            payload,
        )?;
        ipv4::write_header(out.get_mut(14..34)?, our_ip, self.remote_ip, PROTO_TCP, tcp_len)?;
        ethernet::write_header(out.get_mut(0..14)?, self.remote_mac, our_mac, EtherType::Ipv4.as_u16())?;
        Some(14 + 20 + tcp_len)
    }

    /// Handle a received frame. Returns `Some(len)` with a response frame in `out`, or `None`
    /// if the frame isn't a TCP segment for our listener (so the caller falls back to ingress)
    /// or no response is warranted. Runs the echo state machine.
    pub fn handle(&mut self, frame: &[u8], our_ip: [u8; 4], our_mac: [u8; 6], out: &mut [u8]) -> Option<usize> {
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

        // RST from the current peer tears the connection down.
        if flags & RST != 0 {
            if self.state != State::Listen && self.is_current_peer(src_ip, src_port) {
                self.state = State::Listen;
            }
            return None;
        }

        match self.state {
            State::Listen => {
                // Passive open: a bare SYN starts a connection.
                if flags & SYN != 0 && flags & ACK == 0 {
                    self.remote_ip = src_ip;
                    self.remote_mac = src_mac;
                    self.remote_port = src_port;
                    self.rcv_nxt = their_seq.wrapping_add(1); // SYN consumes one sequence number
                    self.isn = self.isn.wrapping_add(0x0001_0000);
                    self.snd_nxt = self.isn;
                    // Emit first; only advance state once the SYN-ACK serialized (defensive —
                    // an empty-payload SYN-ACK always fits, but keep the no-desync invariant).
                    let n = self.emit(out, our_ip, our_mac, SYN | ACK, &[])?;
                    self.snd_nxt = self.snd_nxt.wrapping_add(1); // our SYN consumes one
                    self.state = State::SynRcvd;
                    Some(n)
                } else {
                    None
                }
            }

            State::SynRcvd => {
                if !self.is_current_peer(src_ip, src_port) {
                    return None;
                }
                // Expect the ACK completing the handshake.
                if flags & ACK != 0 && their_ack == self.snd_nxt {
                    self.state = State::Established;
                    // The handshake ACK may already carry data — process it.
                    self.on_data(out, our_ip, our_mac, their_seq, flags, payload)
                } else {
                    None
                }
            }

            State::Established => {
                if !self.is_current_peer(src_ip, src_port) {
                    return None;
                }
                self.on_data(out, our_ip, our_mac, their_seq, flags, payload)
            }

            State::LastAck => {
                if self.is_current_peer(src_ip, src_port)
                    && flags & ACK != 0
                    && their_ack == self.snd_nxt
                {
                    // Our FIN is acknowledged — connection closed, ready for the next.
                    self.state = State::Listen;
                }
                None
            }
        }
    }

    /// Established-state handling of an in-order segment that may carry data and/or FIN.
    /// Folds the data echo, its ACK, and (if present) our FIN into a single response segment.
    fn on_data(
        &mut self,
        out: &mut [u8],
        our_ip: [u8; 4],
        our_mac: [u8; 6],
        their_seq: u32,
        flags: u8,
        payload: &[u8],
    ) -> Option<usize> {
        // Only process in-order segments. A retransmit / out-of-order segment gets a
        // duplicate ACK of what we have, without re-echoing.
        if their_seq != self.rcv_nxt {
            return self.emit(out, our_ip, our_mac, ACK, &[]);
        }

        let has_fin = flags & FIN != 0;
        if payload.is_empty() && !has_fin {
            return None; // a bare ACK — nothing to send
        }

        // Echo the data back; set FIN if the peer is closing (we have nothing more to send).
        let mut resp_flags = ACK | PSH;
        if has_fin {
            resp_flags |= FIN;
        }
        // Serialize the response FIRST and only advance the sequence/state once it succeeds:
        // a segment too large to echo into the TX buffer is then dropped cleanly (the peer
        // retransmits and is handled identically) rather than desynchronizing the connection.
        let n = self.emit(out, our_ip, our_mac, resp_flags, payload)?;
        self.rcv_nxt = self.rcv_nxt.wrapping_add(payload.len() as u32);
        self.snd_nxt = self.snd_nxt.wrapping_add(payload.len() as u32);
        if has_fin {
            // A FIN consumes one sequence number on each side.
            self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
            self.snd_nxt = self.snd_nxt.wrapping_add(1);
            self.state = State::LastAck;
        }
        Some(n)
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
/// [`TcpEcho`]. It connects (SYN → SYN-ACK → ACK), optionally sends one payload, records the
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
                // FIN's sequence position) — matches TcpEcho's defensive in-order handling.
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
