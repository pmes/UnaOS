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


def ipv4(src, dst, proto, payload):
    total = 20 + len(payload)
    hdr = bytearray(struct.pack(">BBHHHBBH", 0x45, 0, total, 0, 0x4000, 64, proto, 0) + src + dst)
    hdr[10:12] = struct.pack(">H", cksum(hdr))
    return bytes(hdr) + payload


def icmp_echo(ident, seq, data):
    msg = bytearray(struct.pack(">BBHHH", 8, 0, 0, ident, seq) + data)
    msg[2:4] = struct.pack(">H", cksum(msg))
    return bytes(msg)


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
    time.sleep(4)  # let the guest finish e1000 bring-up
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
                print("ALL CHECKS PASSED")
                return 0
            print("<- ICMP echo reply: id/seq/data match=%s ip_cksum_ok=%s icmp_cksum_ok=%s"
                  % (match, ip_ck_ok, icmp_ck_ok))
    print("FAIL: no valid ICMP echo reply from guest")
    return 1


if __name__ == "__main__":
    sys.exit(main())
