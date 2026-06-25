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


def recv_tcp(s, my_port):
    """Wait for a TCP segment from the guest addressed to our client port. Returns
    (flags, seq, ack, payload, ip_ck_ok, tcp_ck_ok) or None on timeout."""
    deadline = time.time() + 8
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


def respond_to_guest_probes(s, t0, budget_s=18.0, want_pings=1):
    """Validate the guest's OUTBOUND path. We impersonate the gateway 10.0.2.2: answer the
    guest's ARP who-has 10.0.2.2 and reply to its ICMP echo *requests* (checking the requests'
    checksums). The guest originates these via its boot connectivity self-test (and `ping`).
    Returns True once we've answered an ARP and >= want_pings valid echo requests."""
    print("--- outbound test: impersonating gateway %s (answering guest ARP+ICMP) ---" % ipstr(GW_IP))
    arped = False
    pings = 0
    deadline = time.time() + budget_s
    while time.time() < deadline and not (arped and pings >= want_pings):
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

        if etype == 0x0800 and len(f) >= 34:  # IPv4
            if (f[14] >> 4) != 4 or f[23] != 1:  # IPv4 + proto ICMP
                continue
            ihl = (f[14] & 0x0F) * 4
            total = struct.unpack(">H", f[16:18])[0]
            src_ip, dst_ip = f[26:30], f[30:34]
            if dst_ip != GW_IP:
                continue
            icmp = f[14 + ihl:14 + total]  # exact ICMP message (exclude L2 padding)
            if len(icmp) < 8 or icmp[0] != 8:  # echo request only
                continue
            ip_ck_ok = cksum(f[14:14 + ihl]) == 0
            icmp_ck_ok = cksum(icmp) == 0
            ident = struct.unpack(">H", icmp[4:6])[0]
            seq = struct.unpack(">H", icmp[6:8])[0]
            data = icmp[8:]
            # Build the echo reply: same body, type 8 -> 0, recompute checksum, swap src/dst.
            reply_icmp = bytearray(icmp)
            reply_icmp[0] = 0
            reply_icmp[2:4] = b"\x00\x00"
            reply_icmp[2:4] = struct.pack(">H", cksum(reply_icmp))
            send_frame(s, eth(guest_mac, GW_MAC, 0x0800, ipv4(GW_IP, src_ip, 1, bytes(reply_icmp))))
            # Count it only if BOTH checksums validate AND the protocol fields are exactly what
            # the kernel is contracted to emit (identifier, payload, source IP) — not merely a
            # well-formed packet. (We still replied above so the guest's path can complete.)
            fields_ok = (ident == GUEST_PING_IDENT and data == GUEST_PING_PAYLOAD and src_ip == GUEST_IP)
            if ip_ck_ok and icmp_ck_ok and fields_ok:
                pings += 1
                print("[%5.1fs] <- guest ICMP echo request id=0x%04x seq=%d data=%r ip_cksum=OK icmp_cksum=OK -> replied   [GUEST PING %d]"
                      % (time.time() - t0, ident, seq, data, pings))
            else:
                print("[%5.1fs] <- guest ICMP echo request REJECTED ip_ck=%s icmp_ck=%s id=0x%04x(exp 0x%04x) data=%r(exp %r) src=%s(exp %s)"
                      % (time.time() - t0, ip_ck_ok, icmp_ck_ok, ident, GUEST_PING_IDENT,
                         data, GUEST_PING_PAYLOAD, ipstr(src_ip), ipstr(GUEST_IP)))

    if not arped:
        print("FAIL: guest never ARP-resolved the gateway %s" % ipstr(GW_IP))
        return False
    if pings < want_pings:
        print("FAIL: guest sent %d/%d valid ICMP echo requests" % (pings, want_pings))
        return False
    print("<- guest outbound verified: ARP-resolved gateway + %d valid echo requests   [GUEST OUTBOUND OK]" % pings)
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

    # --- TCP echo: handshake + data echo + teardown (tests the hand-rolled TCP) ---
    c_isn, cport = 0x2000, 5555
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, c_isn, 0, SYN, 4096, MY_IP, GUEST_IP, b""))))
    print("-> TCP SYN to 10.0.2.15:7")
    sa = recv_tcp(s, cport)
    if sa is None or (sa[0] & (SYN | ACK)) != (SYN | ACK) or sa[2] != (c_isn + 1):
        print("FAIL: no/invalid SYN-ACK (%s)" % (sa,))
        return 1
    s_isn = sa[1]
    print("<- TCP SYN-ACK seq=%d ack=%d   [TCP HANDSHAKE OK]" % (s_isn, sa[2]))
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, c_isn + 1, s_isn + 1, ACK, 4096, MY_IP, GUEST_IP, b""))))
    tdata = b"tcp-echo-test"
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, c_isn + 1, s_isn + 1, PSH | ACK, 4096, MY_IP, GUEST_IP, tdata))))
    print("-> TCP data %r" % tdata)
    echo = recv_tcp(s, cport)
    if echo is None or echo[3] != tdata or not echo[4] or not echo[5]:
        print("FAIL: no/invalid TCP echo (got %s)" % (echo,))
        return 1
    print("<- TCP echo %r  ip_cksum=OK tcp_cksum=OK   [TCP ECHO OK]" % (echo[3],))
    n = len(tdata)
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, c_isn + 1 + n, s_isn + 1 + n, FIN | ACK, 4096, MY_IP, GUEST_IP, b""))))
    print("-> TCP FIN")
    fin = recv_tcp(s, cport)
    if fin is None or (fin[0] & FIN) == 0:
        print("FAIL: no FIN-ACK from guest (%s)" % (fin,))
        return 1
    print("<- TCP FIN-ACK   [TCP CLOSE OK]")
    send_frame(s, eth(guest_mac, MY_MAC, 0x0800,
                      ipv4(MY_IP, GUEST_IP, 6, tcp(cport, 7, c_isn + 2 + n, s_isn + 2 + n, ACK, 4096, MY_IP, GUEST_IP, b""))))

    print("ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
