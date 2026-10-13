#!/usr/bin/env python3
# QEMUIMAGE26 driver, part 2: re-plug the door (drive.py exited and closed the socket), retry `tests list`,
# then type every fixture one at a time. Names: the list's own lines if the wire carries them, else the
# source's register()/defer() names (a superset; an absent one answers `no such fixture`).
import os, sys, time, socket, threading, re
SOCK, LOG, OUT, DEADLINE = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
SKIP = {"play"}  # the brief: never mp3 play


def say(m):
    print(f"[drive2 {time.strftime('%H:%M:%S')}] {m}", flush=True)


def logbytes():
    with open(LOG, "rb") as f:
        return f.read()


def wait(needle, cap, start=0):
    n = needle.encode()
    end = time.time() + cap
    while time.time() < end:
        i = logbytes().find(n, start)
        if i >= 0:
            return i
        time.sleep(0.25)
    return -1


s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(SOCK)
s.settimeout(1.0)
say("door re-plugged")


def drain():
    with open(os.path.join(OUT, "cable2.log"), "ab", buffering=0) as c:
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
time.sleep(8)


def door(line):
    for ch in (line + "\r").encode():
        s.sendall(bytes([ch]))
        time.sleep(0.04)


src = [l.strip() for l in open(os.path.join(OUT, "src-names.txt")) if l.strip()]
off = len(logbytes())
door("tests list")
e = wait('[midden] cmd="tests list"', 30, off)
say(f"tests list echo={'yes' if e >= 0 else 'NO'}")
t = wait(" ran at boot", 25, max(e, off))
raw = logbytes()[off:]
open(os.path.join(OUT, "list2.raw"), "wb").write(raw)
names = []
if t >= 0:
    seen = set(src)
    for ln in raw.decode("utf-8", "replace").splitlines():
        m = re.match(r"^(?:\[[^\]]*\]\s*)?([a-z0-9_]+)\s{2,}", ln)
        if m and m.group(1) in seen and m.group(1) not in names:
            names.append(m.group(1))
    say(f"list on the wire: {len(names)} names")
    src_used = False
else:
    names = src
    src_used = True
    say(f"no list tail on the wire; using the source's {len(names)} register()/defer() names")
open(os.path.join(OUT, "names-used.txt"), "w").write(("# source names\n" if src_used else "# wire list\n") + "\n".join(names) + "\n")
res = open(os.path.join(OUT, "progress.txt"), "a")
for i, n in enumerate(names):
    if n in SKIP:
        res.write(f"SKIPPED {n} (brief: never mp3 play)\n")
        res.flush()
        continue
    left = DEADLINE - time.time()
    if left < 15:
        say(f"WALL reached before `{n}` ({i}/{len(names)})")
        res.write(f"STOPPED-AT-WALL {n} {i}/{len(names)}\n")
        res.flush()
        sys.exit(9)
    off = len(logbytes())
    t0 = time.time()
    door(f"tests {n}")
    e = wait(f'[midden] cmd="tests {n}"', min(30, left), off)
    if e < 0:
        say(f"{n}: NO ECHO")
        res.write(f"NOECHO {n}\n")
        res.flush()
        continue
    d = wait(":: TESTS: ran=", min(150, max(1, DEADLINE - time.time())), e)
    dt = time.time() - t0
    if d < 0:
        say(f"{n}: no TESTS ran= ({dt:.0f}s)")
        res.write(f"NODONE {n} {dt:.0f}s\n")
        res.flush()
        continue
    tail = logbytes()[d:d + 240].split(b"\n")[0].decode("utf-8", "replace").strip()
    say(f"{n}: {tail} ({dt:.1f}s)")
    res.write(f"DONE {n} {dt:.1f}s {tail}\n")
    res.flush()
    time.sleep(0.5)
say("LIST END")
res.write("LIST-END\n")
res.flush()
open(os.path.join(OUT, "done"), "w").write("1\n")
time.sleep(3)
