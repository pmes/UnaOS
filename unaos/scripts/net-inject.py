#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# Rootless L2 injector for the UnaOS e1000 net stack. Pairs with the builder's
# `UNAOS_NET=socket` mode (QEMU `-netdev socket,listen=127.0.0.1:5555`): QEMU listens,
# this script connects and exchanges raw Ethernet frames with the guest. It sends an
# ARP request and an ICMP echo request to the guest (10.0.2.15) and validates the
# responder's replies — no root/TAP/vmnet needed.
#
# QEMU socket (TCP) framing: each frame is a 4-byte big-endian length + raw frame bytes.
#
# Usage: net-inject.py [host:port]   (default 127.0.0.1:5555)
import socket, struct, sys, time

HOSTPORT = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1:5555"
HOST, PORT = HOSTPORT.rsplit(":", 1)
PORT = int(PORT)

GUEST_IP = bytes([10, 0, 2, 15])
MY_IP = bytes([10, 0, 2, 1])
MY_MAC = bytes([0x52, 0x54, 0x00, 0xAA, 0xBB, 0xCC])
BCAST = b"\xff" * 6

# When testing the guest's OUTBOUND path we impersonate the gateway 10.0.2.2 (the address the
# guest's boot self-test / `ping` probes) and answer its ARP + ICMP echo requests.
GW_IP = bytes([10, 0, 2, 2])
GW_MAC = bytes([0x52, 0x55, 0x0A, 0x00, 0x02, 0x02])
# Protocol-level fields the kernel is contracted to stamp on its echo requests (PING_IDENT /
# PING_PAYLOAD in drivers/e1000.rs). The outbound test asserts these, not just the checksums,
# so a well-formed-but-wrong request can't false-pass. Source must be the guest's static IP.
GUEST_PING_IDENT = 0x554E  # ASCII "UN"
GUEST_PING_PAYLOAD = b"unaos-ping"
# The guest's boot self-test also active-opens a TCP connection to the gateway here and sends
# this probe; we run a tiny gateway-side TCP echo server on this port to complete it.
SELFTEST_TCP_PORT = 7777
GUEST_TCP_PROBE = b"unaos-tcp"
# The guest's boot self-test also sends one outbound UDP datagram to the gateway here; we run a
# gateway-side UDP echo server to complete it.
SELFTEST_UDP_PORT = 9998
GUEST_UDP_PROBE = b"unaos-udp"


def cksum(data):
    s = 0
    for i in range(0, len(data) - 1, 2):
        s += (data[i] << 8) | data[i + 1]
    if len(data) % 2:
        s += data[-1] << 8
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF


def eth(dst, src, etype, payload):
    return dst + src + struct.pack(">H", etype) + payload


def arp_request(target_ip):
    return struct.pack(">HHBBH", 1, 0x0800, 6, 4, 1) + MY_MAC + MY_IP + (b"\x00" * 6) + target_ip


def arp_reply(sender_mac, sender_ip, target_mac, target_ip):
    # op=2 (reply): "sender_ip is-at sender_mac", addressed to (target_mac, target_ip).
    return struct.pack(">HHBBH", 1, 0x0800, 6, 4, 2) + sender_mac + sender_ip + target_mac + target_ip


def ipv4(src, dst, proto, payload):
    total = 20 + len(payload)
    hdr = bytearray(struct.pack(">BBHHHBBH", 0x45, 0, total, 0, 0x4000, 64, proto, 0) + src + dst)
    hdr[10:12] = struct.pack(">H", cksum(hdr))
    return bytes(hdr) + payload


def icmp_echo(ident, seq, data):
    msg = bytearray(struct.pack(">BBHHH", 8, 0, 0, ident, seq) + data)
    msg[2:4] = struct.pack(">H", cksum(msg))
    return bytes(msg)


def udp_checksum(src_ip, dst_ip, seg):
    # IPv4 pseudo-header (src, dst, proto=17, udp len) + the UDP segment.
    s = 0
    for ip in (src_ip, dst_ip):
        s += (ip[0] << 8) | ip[1]
        s += (ip[2] << 8) | ip[3]
    s += 17 + len(seg)
    for i in range(0, len(seg) - 1, 2):
        s += (seg[i] << 8) | seg[i + 1]
    if len(seg) % 2:
        s += seg[-1] << 8
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF


def udp(sport, dport, src_ip, dst_ip, payload):
    seg = bytearray(struct.pack(">HHHH", sport, dport, 8 + len(payload), 0) + payload)
    c = udp_checksum(src_ip, dst_ip, seg) or 0xFFFF
    seg[6:8] = struct.pack(">H", c)
    return bytes(seg)


# TCP flag bits.
FIN, SYN, RST, PSH, ACK = 0x01, 0x02, 0x04, 0x08, 0x10


def tcp_checksum(src_ip, dst_ip, seg):
    s = 0
    for ip in (src_ip, dst_ip):
        s += (ip[0] << 8) | ip[1]
        s += (ip[2] << 8) | ip[3]
    s += 6 + len(seg)
    for i in range(0, len(seg) - 1, 2):
        s += (seg[i] << 8) | seg[i + 1]
    if len(seg) % 2:
        s += seg[-1] << 8
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF


def tcp(sport, dport, seq, ack, flags, window, src_ip, dst_ip, payload):
    seg = bytearray(struct.pack(">HHIIBBHHH", sport, dport, seq, ack, 5 << 4, flags, window, 0, 0) + payload)
    seg[16:18] = struct.pack(">H", tcp_checksum(src_ip, dst_ip, seg))
    return bytes(seg)


def recv_tcp(s, my_port, timeout=8):
    """Wait for a TCP segment from the guest addressed to our client port. Returns
    (flags, seq, ack, payload, ip_ck_ok, tcp_ck_ok) or None on timeout. (The socket's own
    timeout must be >= `timeout`, else recv_frame raises first and this returns early.)"""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            f = recv_frame(s)
        except socket.timeout:
            break
        if not f or len(f) < 34:
            continue
        if struct.unpack(">H", f[12:14])[0] != 0x0800 or (f[14] >> 4) != 4 or f[23] != 6:
            continue
        ihl = (f[14] & 0x0F) * 4
        total = struct.unpack(">H", f[16:18])[0]
        src_ip, dst_ip = f[26:30], f[30:34]
        ip_ck_ok = cksum(f[14:14 + ihl]) == 0
        seg = f[14 + ihl:14 + total]
        if len(seg) < 20 or struct.unpack(">H", seg[2:4])[0] != my_port:
            continue
        seq = struct.unpack(">I", seg[4:8])[0]
        ack = struct.unpack(">I", seg[8:12])[0]
        doff = (seg[12] >> 4) * 4
        flags = seg[13]
        payload = seg[doff:]
        tcp_ck_ok = tcp_checksum(src_ip, dst_ip, seg) == 0
        return (flags, seq, ack, payload, ip_ck_ok, tcp_ck_ok)
    return None


def recv_tcp_any(s, timeout=8):
    """Receive the next TCP segment from the guest addressed to ANY of our client ports.
    Returns (dport, flags, seq, ack, payload, ip_ok, tcp_ok) or None on timeout. Used by the
    multi-connection test, which must demultiplex replies for several connections at once."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            f = recv_frame(s)
        except socket.timeout:
            break
        if not f or len(f) < 34:
            continue
        if struct.unpack(">H", f[12:14])[0] != 0x0800 or (f[14] >> 4) != 4 or f[23] != 6:
            continue
        ihl = (f[14] & 0x0F) * 4
        total = struct.unpack(">H", f[16:18])[0]
        src_ip, dst_ip = f[26:30], f[30:34]
        ip_ck_ok = cksum(f[14:14 + ihl]) == 0
        seg = f[14 + ihl:14 + total]
        if len(seg) < 20:
            continue
        dport = struct.unpack(">H", seg[2:4])[0]
        seq = struct.unpack(">I", seg[4:8])[0]
        ack = struct.unpack(">I", seg[8:12])[0]
        doff = (seg[12] >> 4) * 4
        flags = seg[13]
        payload = seg[doff:]
        tcp_ck_ok = tcp_checksum(src_ip, dst_ip, seg) == 0
        return (dport, flags, seq, ack, payload, ip_ck_ok, tcp_ck_ok)
    return None


def send_frame(s, frame):
    s.sendall(struct.pack(">I", len(frame)) + frame)


def recvn(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def recv_frame(s):
    hdr = recvn(s, 4)
    if hdr is None:
        return None
    (n,) = struct.unpack(">I", hdr)
    if n == 0 or n > 65536:
        return None
    return recvn(s, n)


def macstr(b):
    return ":".join("%02x" % x for x in b)


def ipstr(b):
    return ".".join("%d" % x for x in b)


def respond_to_guest_probes(s, t0, budget_s=22.0, want_pings=1):
    """Validate the guest's OUTBOUND path. We impersonate the gateway 10.0.2.2 and answer the
    guest's boot self-test in full: ARP who-has, ICMP echo *requests*, and an active-open TCP
    connection to 10.0.2.2:7777 (handshake -> data echo -> teardown). Every guest packet is
    checked (checksums + the protocol fields the kernel is contracted to emit). Returns True
    once ARP + >= want_pings pings + one complete TCP echo exchange have all been observed."""
    print("--- outbound test: impersonating gateway %s (ARP + ICMP + TCP echo :%d + UDP echo :%d) ---"
          % (ipstr(GW_IP), SELFTEST_TCP_PORT, SELFTEST_UDP_PORT))
    arped = False
    pings = 0
    conn = None       # gateway-side TCP server state for the one expected connection
    s_isn = 0x9000
    tcp_ok = False    # set once a SINGLE connection completes handshake + valid echo + close
    udp_ok = False    # set once a valid outbound UDP datagram is received + echoed
    deadline = time.time() + budget_s
    while time.time() < deadline and not (arped and pings >= want_pings and tcp_ok and udp_ok):
        try:
            f = recv_frame(s)
        except socket.timeout:
            continue
        if f is None:  # EOF / bad framing — peer gone; stop rather than busy-spin to the deadline
            print("connection closed by QEMU (EOF) during outbound test")
            break
        if len(f) < 14:
            continue
        etype = struct.unpack(">H", f[12:14])[0]
        guest_mac = f[6:12]

        if etype == 0x0806 and len(f) >= 42:  # ARP
            op = struct.unpack(">H", f[20:22])[0]
            spa, tpa = f[28:32], f[38:42]  # sender / target protocol address
            if op == 1 and tpa == GW_IP:  # who-has the gateway?
                send_frame(s, eth(guest_mac, GW_MAC, 0x0806, arp_reply(GW_MAC, GW_IP, guest_mac, spa)))
                if not arped:
                    print("[%5.1fs] <- guest ARP who-has %s tell %s  -> replied is-at %s   [GUEST ARP OK]"
                          % (time.time() - t0, ipstr(GW_IP), ipstr(spa), macstr(GW_MAC)))
                    arped = True
            continue

        if etype != 0x0800 or len(f) < 34 or (f[14] >> 4) != 4:  # IPv4 only
            continue
        proto = f[23]
        ihl = (f[14] & 0x0F) * 4
        total = struct.unpack(">H", f[16:18])[0]
        src_ip, dst_ip = f[26:30], f[30:34]
        if dst_ip != GW_IP:
            continue
        ip_ck_ok = cksum(f[14:14 + ihl]) == 0
        l4 = f[14 + ihl:14 + total]  # exact L4 message (exclude L2 padding)

        if proto == 1:  # ICMP echo request
            icmp = l4
            if len(icmp) < 8 or icmp[0] != 8:
                continue
            icmp_ck_ok = cksum(icmp) == 0
            ident = struct.unpack(">H", icmp[4:6])[0]
            seq = struct.unpack(">H", icmp[6:8])[0]
            data = icmp[8:]
            reply_icmp = bytearray(icmp)
            reply_icmp[0] = 0  # type 8 -> 0
            reply_icmp[2:4] = b"\x00\x00"
            reply_icmp[2:4] = struct.pack(">H", cksum(reply_icmp))
            send_frame(s, eth(guest_mac, GW_MAC, 0x0800, ipv4(GW_IP, src_ip, 1, bytes(reply_icmp))))
            fields_ok = (ident == GUEST_PING_IDENT and data == GUEST_PING_PAYLOAD and src_ip == GUEST_IP)
            if ip_ck_ok and icmp_ck_ok and fields_ok:
                pings += 1
                print("[%5.1fs] <- guest ICMP echo request id=0x%04x seq=%d data=%r ip_cksum=OK icmp_cksum=OK -> replied   [GUEST PING %d]"
                      % (time.time() - t0, ident, seq, data, pings))
            else:
                print("[%5.1fs] <- guest ICMP echo request REJECTED ip_ck=%s icmp_ck=%s id=0x%04x(exp 0x%04x) data=%r(exp %r) src=%s(exp %s)"
                      % (time.time() - t0, ip_ck_ok, icmp_ck_ok, ident, GUEST_PING_IDENT,
                         data, GUEST_PING_PAYLOAD, ipstr(src_ip), ipstr(GUEST_IP)))
            continue

        if proto == 17 and len(l4) >= 8:  # UDP (gateway-side echo server)
            sport = struct.unpack(">H", l4[0:2])[0]
            dport = struct.unpack(">H", l4[2:4])[0]
            if dport != SELFTEST_UDP_PORT:
                continue
            udp_len = struct.unpack(">H", l4[4:6])[0]
            data = l4[8:udp_len] if 8 <= udp_len <= len(l4) else l4[8:]
            udp_ck_ok = udp_checksum(src_ip, dst_ip, l4) == 0
            # Echo back, swapping ports (src=our port, dst=guest's ephemeral port).
            send_frame(s, eth(guest_mac, GW_MAC, 0x0800,
                              ipv4(GW_IP, src_ip, 17, udp(SELFTEST_UDP_PORT, sport, GW_IP, src_ip, data))))
            ok = (data == GUEST_UDP_PROBE and udp_ck_ok and ip_ck_ok and src_ip == GUEST_IP)
            udp_ok = udp_ok or ok
            print("[%5.1fs] <- guest UDP %r ip_ck=%s udp_ck=%s src=%s -> echoed   [GUEST UDP %s]"
                  % (time.time() - t0, data, ip_ck_ok, udp_ck_ok, ipstr(src_ip), "OK" if ok else "REJECTED"))
            continue

        if proto == 6 and len(l4) >= 20:  # TCP (gateway-side echo server)
            sport = struct.unpack(">H", l4[0:2])[0]
            dport = struct.unpack(">H", l4[2:4])[0]
            if dport != SELFTEST_TCP_PORT:
                continue
            seq = struct.unpack(">I", l4[4:8])[0]
            doff = (l4[12] >> 4) * 4
            flags = l4[13]
            payload = l4[doff:]
            tcp_ck_ok = tcp_checksum(src_ip, dst_ip, l4) == 0

            def gw_send(seq_n, ack_n, fl, data=b""):
                seg = tcp(SELFTEST_TCP_PORT, sport, seq_n, ack_n, fl, 4096, GW_IP, src_ip, data)
                send_frame(s, eth(guest_mac, GW_MAC, 0x0800, ipv4(GW_IP, src_ip, 6, seg)))

            if flags & RST:
                conn = None
                continue
            if (flags & SYN) and not (flags & ACK):  # active-open SYN
                # Per-connection state — the pass evidence (data_ok/closed) lives here, so two
                # separate partial connections can't jointly satisfy the test.
                conn = {"cport": sport, "cseq": (seq + 1) & 0xFFFFFFFF, "sseq": (s_isn + 1) & 0xFFFFFFFF,
                        "finned": False, "data_ok": False, "closed": False}
                gw_send(s_isn, conn["cseq"], SYN | ACK)
                print("[%5.1fs] <- guest TCP SYN -> SYN-ACK   [GUEST TCP HANDSHAKE OK]" % (time.time() - t0))
                continue
            if conn is None or sport != conn["cport"]:
                continue
            if payload:  # data segment -> ack + echo it back
                # Accept as valid only if in-order (seq == expected), exact probe, checksums OK.
                in_order = (seq == conn["cseq"])
                conn["cseq"] = (seq + len(payload)) & 0xFFFFFFFF
                gw_send(conn["sseq"], conn["cseq"], PSH | ACK, payload)
                conn["sseq"] = (conn["sseq"] + len(payload)) & 0xFFFFFFFF
                ok = (in_order and payload == GUEST_TCP_PROBE and tcp_ck_ok and ip_ck_ok)
                conn["data_ok"] = conn["data_ok"] or ok
                print("[%5.1fs] <- guest TCP data %r seq_ok=%s ip_ck=%s tcp_ck=%s -> echoed   [GUEST TCP ECHO %s]"
                      % (time.time() - t0, payload, in_order, ip_ck_ok, tcp_ck_ok, "OK" if ok else "REJECTED"))
            if flags & FIN:  # active close -> ack the FIN and send our own
                conn["cseq"] = (conn["cseq"] + 1) & 0xFFFFFFFF
                gw_send(conn["sseq"], conn["cseq"], FIN | ACK)
                conn["sseq"] = (conn["sseq"] + 1) & 0xFFFFFFFF
                conn["finned"] = True
                print("[%5.1fs] <- guest TCP FIN -> FIN-ACK" % (time.time() - t0))
                continue
            if (flags & ACK) and conn.get("finned"):  # final ACK of our FIN
                conn["closed"] = True
                if conn["data_ok"] and conn["closed"]:  # this ONE connection did everything
                    tcp_ok = True
                print("[%5.1fs] <- guest TCP final ACK   [GUEST TCP CLOSE %s]"
                      % (time.time() - t0, "OK" if tcp_ok else "(no valid data)"))
            continue

    if not arped:
        print("FAIL: guest never ARP-resolved the gateway %s" % ipstr(GW_IP))
        return False
    if pings < want_pings:
        print("FAIL: guest sent %d/%d valid ICMP echo requests" % (pings, want_pings))
        return False
    if not tcp_ok:
        print("FAIL: guest did not complete one full TCP handshake + valid echo + close")
        return False
    if not udp_ok:
        print("FAIL: guest did not send a valid outbound UDP datagram")
        return False
    print("<- guest outbound verified: ARP + %d ping(s) + TCP connect/echo/close + UDP send/echo   [GUEST OUTBOUND OK]" % pings)
    return True


def test_multi_tcp(s, guest_mac, n=3):
    """Open n SIMULTANEOUS TCP connections to the guest echo listener (:7), interleaved, and
    verify each handshakes, echoes its OWN distinct data, and closes independently — proving
    the connection table demultiplexes by 4-tuple with correct per-connection seq/ack."""
    print("--- multi-conn TCP: %d simultaneous connections to %s:7 ---" % (n, ipstr(GUEST_IP)))
    MASK = 0xFFFFFFFF
    conns = {}
    for i in range(n):
        cport = 40001 + i
        conns[cport] = {"cport": cport, "c_isn": 0x3000 + 0x100 * i,
                        "data": ("conn%d-data!" % i).encode(),
                        "s_isn": None, "echoed": False, "finned": False}

    def send(cport, seq, ack, flags, payload=b""):
        send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                          ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, seq, ack, flags, 4096, MY_IP, GUEST_IP, payload))))

    # Phase 1: SYN all (interleaved), then collect SYN-ACKs in any order.
    for c in conns.values():
        send(c["cport"], c["c_isn"], 0, SYN)
    print("-> %d SYNs sent (interleaved)" % n)
    need = n
    while need > 0:
        r = recv_tcp_any(s)
        if r is None:
            break
        dport, flags, seq, ack, payload, ipok, tcpok = r
        c = conns.get(dport)
        if not c or c["s_isn"] is not None:
            continue
        if (flags & (SYN | ACK)) == (SYN | ACK) and ack == (c["c_isn"] + 1) & MASK and ipok and tcpok:
            c["s_isn"] = seq
            send(c["cport"], (c["c_isn"] + 1) & MASK, (seq + 1) & MASK, ACK)  # complete handshake
            need -= 1
    if any(c["s_isn"] is None for c in conns.values()):
        print("FAIL: only %d/%d connections handshook" % (sum(c["s_isn"] is not None for c in conns.values()), n))
        return False
    print("<- %d/%d SYN-ACKs (all connections open at once)   [MULTI-CONN HANDSHAKE OK]" % (n, n))

    # Phase 2: distinct data on every (concurrently-open) connection, collect echoes.
    for c in conns.values():
        send(c["cport"], (c["c_isn"] + 1) & MASK, (c["s_isn"] + 1) & MASK, PSH | ACK, c["data"])
    print("-> distinct data on all %d connections" % n)
    need = n
    while need > 0:
        r = recv_tcp_any(s)
        if r is None:
            break
        dport, flags, seq, ack, payload, ipok, tcpok = r
        c = conns.get(dport)
        if not c or c["echoed"]:
            continue
        # Validate the echo carries THIS connection's data with correct per-connection seq/ack.
        if (payload == c["data"]
                and seq == (c["s_isn"] + 1) & MASK
                and ack == (c["c_isn"] + 1 + len(c["data"])) & MASK
                and ipok and tcpok):
            c["echoed"] = True
            # ACK the echo promptly (well-behaved peer) so the guest clears its retransmit timer.
            nb = len(c["data"])
            send(c["cport"], (c["c_isn"] + 1 + nb) & MASK, (c["s_isn"] + 1 + nb) & MASK, ACK)
            need -= 1
        elif payload:
            print("   conn :%d echo mismatch payload=%r (want %r)" % (dport, payload, c["data"]))
    if any(not c["echoed"] for c in conns.values()):
        print("FAIL: not every connection echoed its own data with correct seq/ack")
        return False
    print("<- all %d connections echoed their OWN data (per-conn seq/ack verified)   [MULTI-CONN ECHO OK]" % n)

    # Phase 3: FIN every connection, collect FIN-ACKs, send the final ACKs.
    for c in conns.values():
        nb = len(c["data"])
        send(c["cport"], (c["c_isn"] + 1 + nb) & MASK, (c["s_isn"] + 1 + nb) & MASK, FIN | ACK)
    print("-> FIN on all %d connections" % n)
    need = n
    while need > 0:
        r = recv_tcp_any(s)
        if r is None:
            break
        dport, flags, seq, ack, payload, ipok, tcpok = r
        c = conns.get(dport)
        if not c or c["finned"]:
            continue
        if flags & FIN:
            c["finned"] = True
            nb = len(c["data"])
            send(c["cport"], (c["c_isn"] + 2 + nb) & MASK, (c["s_isn"] + 2 + nb) & MASK, ACK)
            need -= 1
    if any(not c["finned"] for c in conns.values()):
        print("FAIL: not all connections closed")
        return False
    print("<- all %d connections closed independently   [MULTI-CONN CLOSE OK]" % n)
    return True


def test_tcp_retransmit(s, guest_mac):
    """Loss injection: prove the listener retransmits an unacknowledged segment after its RTO.
    (1) Data path: send data, receive the echo, then WITHHOLD the ACK -> the guest must
        retransmit the SAME echo. (2) Half-open path: a SYN-ACK whose handshake ACK is withheld
        must also be retransmitted (the mechanism that eventually times out half-open slots)."""
    print("--- TCP retransmission (loss injection) ---")
    s.settimeout(12)  # the retransmit RTO is coarse; allow a generous wait

    def cs(cport, seq, ack, flags, payload=b""):
        send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                          ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, seq, ack, flags, 4096, MY_IP, GUEST_IP, payload))))

    # (1) Data retransmission.
    cport, c_isn = 40060, 0x6000
    cs(cport, c_isn, 0, SYN)
    sa = recv_tcp(s, cport)
    if sa is None or (sa[0] & (SYN | ACK)) != (SYN | ACK):
        print("FAIL: no SYN-ACK for retransmit test (%s)" % (sa,)); return False
    s_isn = sa[1]
    cs(cport, c_isn + 1, s_isn + 1, ACK)
    data = b"retransmit-me"
    cs(cport, c_isn + 1, s_isn + 1, PSH | ACK, data)
    e1 = recv_tcp(s, cport)
    if e1 is None or e1[3] != data:
        print("FAIL: no initial echo (%s)" % (e1,)); return False
    t0 = time.time()
    print("-> sent data, got echo seq=%d; WITHHOLDING ack to force retransmission" % e1[1])
    rx = recv_tcp(s, cport, timeout=10)  # the same segment must come again
    if rx is None or rx[3] != data or rx[1] != e1[1]:
        print("FAIL: echo was not retransmitted (got %s)" % (rx,)); return False
    print("<- echo RETRANSMITTED after %.2fs (same seq=%d data=%r)   [TCP RETRANSMIT OK]"
          % (time.time() - t0, rx[1], rx[3]))
    nb = len(data)
    cs(cport, c_isn + 1 + nb, s_isn + 1 + nb, ACK)            # finally ack it
    cs(cport, c_isn + 1 + nb, s_isn + 1 + nb, FIN | ACK)      # and close
    fin = recv_tcp(s, cport)
    if fin is None or (fin[0] & FIN) == 0:
        print("FAIL: no FIN-ACK after retransmit test (%s)" % (fin,)); return False
    cs(cport, c_isn + 2 + nb, s_isn + 2 + nb, ACK)

    # (2) Half-open SYN-ACK retransmission (the half-open / SYN-flood timeout mechanism).
    hport, h_isn = 40061, 0x6100
    cs(hport, h_isn, 0, SYN)
    sa1 = recv_tcp(s, hport)
    if sa1 is None or (sa1[0] & (SYN | ACK)) != (SYN | ACK):
        print("FAIL: no SYN-ACK for half-open test (%s)" % (sa1,)); return False
    th = time.time()
    print("-> half-open: got SYN-ACK seq=%d; WITHHOLDING the handshake ACK" % sa1[1])
    sa2 = recv_tcp(s, hport, timeout=10)
    if sa2 is None or (sa2[0] & (SYN | ACK)) != (SYN | ACK) or sa2[1] != sa1[1]:
        print("FAIL: SYN-ACK was not retransmitted (got %s)" % (sa2,)); return False
    print("<- SYN-ACK RETRANSMITTED after %.2fs (same seq=%d)   [TCP HALF-OPEN RETRANSMIT OK]"
          % (time.time() - th, sa2[1]))
    cs(hport, sa1[1] + 1, 0, RST)  # free the guest's slot promptly
    s.settimeout(8)
    return True


def main():
    s = None
    for _ in range(80):
        try:
            s = socket.create_connection((HOST, PORT), timeout=2)
            break
        except OSError:
            time.sleep(0.5)
    if s is None:
        print("FAIL: could not connect to QEMU socket netdev at %s:%d" % (HOST, PORT))
        return 1
    print("connected to %s:%d" % (HOST, PORT))
    t0 = time.time()
    time.sleep(3)  # let the guest finish e1000 bring-up + arm its boot self-test

    # --- OUTBOUND: the guest initiates (boot self-test ARP-resolves + pings the gateway). We
    #     impersonate gateway 10.0.2.2 and answer, proving the kernel's outbound build path. ---
    s.settimeout(2)
    if not respond_to_guest_probes(s, t0):
        return 1

    s.settimeout(8)
    # --- ARP: who-has 10.0.2.15 (tests the ARP responder) ---
    send_frame(s, eth(BCAST, MY_MAC, 0x0806, arp_request(GUEST_IP)))
    print("-> ARP request who-has 10.0.2.15")
    guest_mac = None
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            f = recv_frame(s)
        except socket.timeout:
            break
        if not f or len(f) < 42:
            continue
        if struct.unpack(">H", f[12:14])[0] != 0x0806:
            continue
        op = struct.unpack(">H", f[20:22])[0]
        if op == 2 and f[28:32] == GUEST_IP:
            guest_mac = f[22:28]
            print("<- ARP reply: 10.0.2.15 is-at %s   [ARP RESPONDER OK]" % macstr(guest_mac))
            break
    if guest_mac is None:
        print("FAIL: no ARP reply from guest")
        return 1

    # --- ICMP echo request to 10.0.2.15 (tests the Phase 3 echo responder) ---
    ident, seq, data = 0x1234, 1, b"unaos-ping-test!"
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800, ipv4(MY_IP, GUEST_IP, 1, icmp_echo(ident, seq, data))))
    print("-> ICMP echo request to 10.0.2.15")
    icmp_ok = False
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            f = recv_frame(s)
        except socket.timeout:
            break
        if not f or len(f) < 34:
            continue
        if struct.unpack(">H", f[12:14])[0] != 0x0800:
            continue
        if (f[14] >> 4) != 4 or f[23] != 1:  # IPv4 + proto ICMP
            continue
        ihl = (f[14] & 0x0F) * 4
        total = struct.unpack(">H", f[16:18])[0]  # IPv4 total length
        ip_hdr = f[14:14 + ihl]
        icmp = f[14 + ihl:14 + total]  # exact ICMP message (exclude L2 padding)
        if len(icmp) >= 8 and icmp[0] == 0:  # echo reply
            # A valid 1's-complement checksum re-sums to 0 over the covered bytes.
            ip_ck_ok = cksum(ip_hdr) == 0
            icmp_ck_ok = cksum(icmp) == 0
            rident, rseq = struct.unpack(">H", icmp[4:6])[0], struct.unpack(">H", icmp[6:8])[0]
            rdata = icmp[8:8 + len(data)]
            match = rident == ident and rseq == seq and rdata == data
            if match and ip_ck_ok and icmp_ck_ok:
                print("<- ICMP echo reply id=0x%04x seq=%d data=%r  ip_cksum=OK icmp_cksum=OK   [ICMP ECHO OK]"
                      % (rident, rseq, rdata))
                icmp_ok = True
                break
            print("<- ICMP echo reply: id/seq/data match=%s ip_cksum_ok=%s icmp_cksum_ok=%s"
                  % (match, ip_ck_ok, icmp_ck_ok))
    if not icmp_ok:
        print("FAIL: no valid ICMP echo reply from guest")
        return 1

    # --- UDP echo to 10.0.2.15:9999 (tests the Phase 4 UDP echo responder) ---
    sport, dport, udata = 4321, 9999, b"unaos-udp-echo!"
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 17, udp(sport, dport, MY_IP, GUEST_IP, udata))))
    print("-> UDP datagram %d->%d to 10.0.2.15 data=%r" % (sport, dport, udata))
    udp_ok = False
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            f = recv_frame(s)
        except socket.timeout:
            break
        if not f or len(f) < 42:
            continue
        if struct.unpack(">H", f[12:14])[0] != 0x0800 or (f[14] >> 4) != 4 or f[23] != 17:
            continue
        ihl = (f[14] & 0x0F) * 4
        total = struct.unpack(">H", f[16:18])[0]
        src_ip, dst_ip = f[26:30], f[30:34]
        ip_ck_ok = cksum(f[14:14 + ihl]) == 0
        seg = f[14 + ihl:14 + total]  # exact UDP segment (exclude L2 padding)
        if len(seg) < 8:
            continue
        rsport = struct.unpack(">H", seg[0:2])[0]
        rdport = struct.unpack(">H", seg[2:4])[0]
        rpayload = seg[8:]
        udp_ck_ok = udp_checksum(src_ip, dst_ip, seg) == 0
        # Echo swaps ports: reply src=our dst port, reply dst=our src port.
        if rsport == dport and rdport == sport and rpayload == udata and ip_ck_ok and udp_ck_ok:
            print("<- UDP echo %d->%d data=%r  ip_cksum=OK udp_cksum=OK   [UDP ECHO OK]"
                  % (rsport, rdport, rpayload))
            udp_ok = True
            break
        print("<- UDP reply: ports %d->%d data_match=%s ip_ck=%s udp_ck=%s"
              % (rsport, rdport, rpayload == udata, ip_ck_ok, udp_ck_ok))
    if not udp_ok:
        print("FAIL: no valid UDP echo reply from guest")
        return 1

    # --- TCP: multiple simultaneous connections to the echo listener (port 7). This both
    #     exercises the hand-rolled TCP (handshake / data echo / teardown) AND proves the new
    #     connection table demuxes several concurrent connections by 4-tuple. ---
    if not test_multi_tcp(s, guest_mac, n=3):
        return 1

    # --- TCP retransmission / RTO (loss injection: withhold ACKs, expect resends) ---
    if not test_tcp_retransmit(s, guest_mac):
        return 1

    print("ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
