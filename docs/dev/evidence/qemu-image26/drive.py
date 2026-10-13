#!/usr/bin/env python3
# QEMUIMAGE26 driver: plug the FTDI door, type the first-user form over QMP, then the tests list over the door.
import os, sys, time, socket, threading
U, SOCK, PORT, LOG, OUT = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5]
WALL = float(sys.argv[6])
sys.path.insert(0, os.path.join(U, "scripts"))
import qmp_type as q


def say(m):
    print(f"[drive {time.strftime('%H:%M:%S')}] {m}", flush=True)


def logbytes():
    try:
        with open(LOG, "rb") as f:
            return f.read()
    except FileNotFoundError:
        return b""


def wait(needle, cap, start=0):
    n = needle.encode()
    end = time.time() + cap
    while time.time() < end:
        b = logbytes()
        i = b.find(n, start)
        if i >= 0:
            return i
        time.sleep(0.25)
    return -1


# 1. the plug (the connection is what attaches the FT232)
end = time.time() + 7200
s = None
while time.time() < end:
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(SOCK)
        break
    except OSError:
        s = None
        time.sleep(0.25)
if s is None:
    say("FTDI socket never accepted")
    sys.exit(1)
t_qemu = time.time()
deadline = t_qemu + WALL
open(os.path.join(OUT, "t_qemu"), "w").write(f"{t_qemu}\n")
say(f"FTDI plugged; wall deadline +{WALL:.0f}s")
s.settimeout(1.0)


def drain():
    with open(os.path.join(OUT, "cable.log"), "wb", buffering=0) as c:
        while True:
            try:
                d = s.recv(4096)
            except (socket.timeout, TimeoutError):
                continue
            except OSError:
                return
            if not d:
                return
            c.write(d)


threading.Thread(target=drain, daemon=True).start()
qmp = q.Qmp(q.connect("127.0.0.1", PORT, 600))
qmp._read_obj()
qmp.execute("qmp_capabilities")
say("QMP up")

# 2. the setter: the first-user form, as twoboot.sh types it
mark = "[login] installer: first-user form open"
if wait(mark, 600) < 0:
    say("NO FORM in 600s")
    sys.exit(3)
say("form open; settle 4s")
time.sleep(4)
keys = [q.qcode(c) for c in "una"] + ["tab"] + [q.qcode(c) for c in "correct-horse"] + ["tab"] \
    + [q.qcode(c) for c in "correct-horse"] + ["ret"]
for k in keys:
    qmp.execute("send-key", {"keys": [{"type": "qcode", "data": k}]})
    time.sleep(0.12)
say("form typed")
if wait("[boot] services up first=", 600) < 0:
    say("NO DESKTOP (services up) in 600s")
    sys.exit(4)
say("desktop: services up")
if wait(":: U2.5: FTDI console up", 120) < 0:
    say("FTDI console never up")
    sys.exit(5)
time.sleep(10)


def door(line):
    for ch in (line + "\r").encode():
        s.sendall(bytes([ch]))
        time.sleep(0.04)


# 3. tests list
off = len(logbytes())
door("tests list")
e = wait('[midden] cmd="tests list"', 60, off)
if e < 0:
    say("no echo for tests list")
    sys.exit(6)
if wait(" ran at boot", 60, e) < 0:
    say("no list tail")
    sys.exit(7)
time.sleep(1)
open(os.path.join(OUT, "list.raw"), "wb").write(logbytes()[e:])
say("list captured -> list.raw; waiting for names.txt")
nf = os.path.join(OUT, "names.txt")
while not os.path.exists(nf):
    if time.time() > deadline:
        say("wall before names")
        sys.exit(8)
    time.sleep(1)
names = [l.split()[0] for l in open(nf) if l.strip() and not l.startswith("#")]
res = open(os.path.join(OUT, "progress.txt"), "a")
for i, n in enumerate(names):
    left = deadline - time.time()
    if left < 20:
        say(f"WALL reached before `{n}` ({i}/{len(names)} typed)")
        res.write(f"STOPPED-AT-WALL {n} {i}/{len(names)}\n")
        res.flush()
        sys.exit(9)
    off = len(logbytes())
    t0 = time.time()
    door(f"tests {n}")
    e = wait(f'[midden] cmd="tests {n}"', min(60, left), off)
    if e < 0:
        say(f"{n}: NO ECHO")
        res.write(f"NOECHO {n}\n")
        res.flush()
        continue
    d = wait(":: TESTS: ran=", min(240, max(1, deadline - time.time())), e)
    dt = time.time() - t0
    if d < 0:
        say(f"{n}: no TESTS ran= in window ({dt:.0f}s)")
        res.write(f"NODONE {n} {dt:.0f}s\n")
        res.flush()
        continue
    tail = logbytes()[d:d + 200].split(b"\n")[0].decode("utf-8", "replace").strip()
    say(f"{n}: {tail} ({dt:.1f}s)")
    res.write(f"DONE {n} {dt:.1f}s {tail}\n")
    res.flush()
    time.sleep(1)
say("LIST END")
res.write("LIST-END\n")
res.flush()
open(os.path.join(OUT, "done"), "w").write("1\n")
time.sleep(5)
