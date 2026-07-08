#!/bin/bash
# card-watch.sh — emit one line per external-disk attach/detach on this host (macOS).
#
# Bench companion for metal testing: a session arms this behind its Monitor tool, so the
# instant Peter inserts (or pulls) an SD card / USB stick, the session wakes with the event —
# no "it's in" message needed. Poll-based (2 s), diff of `diskutil list external`, one line
# per changed disk: "CARD INSERTED: disk3 UNAOSRW 31.1MB" / "CARD REMOVED: disk3".
# Also emits the mount point once a volume mounts, since prep work needs /Volumes/<name>.
set -u

snapshot() {
    # "disk3|UNAOSRW|31.1 MB" one line per external whole-disk, from its first partition label.
    diskutil list external 2>/dev/null | awk '
        /^\/dev\/disk[0-9]+ \(external, physical\):/ { d=$1; sub("/dev/","",d); sub(":","",d) }
        d && $1=="1:" {
            name=""; for (i=3; i<NF-1; i++) name = name (name?" ":"") $i
            print d "|" name "|" $(NF-1) $NF; d=""
        }'
}

prev="$(snapshot)"
echo "card-watch: armed ($(echo "$prev" | grep -c . ) external disk(s) present)"
while true; do
    sleep 2
    cur="$(snapshot)"
    if [ "$cur" != "$prev" ]; then
        # New disks
        while IFS='|' read -r d name size; do
            [ -z "$d" ] && continue
            if ! grep -q "^$d|" <<<"$prev"; then
                echo "CARD INSERTED: $d ${name:-<no label>} ${size}"
                # Best-effort: report the mount point once it appears (up to ~6 s).
                for _ in 1 2 3; do
                    mp="$(diskutil info "${d}s1" 2>/dev/null | awk -F': +' '/Mount Point/ {print $2}')"
                    [ -n "$mp" ] && { echo "CARD MOUNTED: $d at $mp"; break; }
                    sleep 2
                done
            fi
        done <<<"$cur"
        # Gone disks
        while IFS='|' read -r d name size; do
            [ -z "$d" ] && continue
            grep -q "^$d|" <<<"$cur" || echo "CARD REMOVED: $d ${name:-<no label>}"
        done <<<"$prev"
        prev="$cur"
    fi
done
