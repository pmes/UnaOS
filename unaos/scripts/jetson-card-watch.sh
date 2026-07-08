#!/bin/bash
# jetson-card-watch.sh — SD/USB card insert alerter for the Jetson bench, WITH target-board ID.
#
# The jetson sibling of the rMBP card-watch.sh, plus the part Peter asked for: because all three
# tracks (jetson / rmbp / pi4) flash an identically-labelled "UNAOS" stick, when a card mounts this
# looks INSIDE it (scripts/identify-card.sh) and says whether it is FOR THIS (jetson) session or for
# another one — so the wrong session never grabs a card that isn't its own.
#
# Arm it behind the session's Monitor tool (or `tail -f` a log): the instant Peter inserts or pulls a
# card the session wakes with the event — no "it's in" message needed. Poll-based (2 s), diff of
# `diskutil list external`, one line per changed disk. On a mount it prints the platform verdict and,
# for a jetson card, the exact boot-log-capture command to run next.
#
# Usage:  scripts/jetson-card-watch.sh          (foreground; run behind Monitor / run_in_background)
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
IDENTIFY="${HERE}/identify-card.sh"

snapshot() {
    # "disk2|UNAOS|2.0 GB" one line per external whole-disk, from its first partition line.
    # The "1:" line is:  "1:" TYPE NAME... SIZE_NUM SIZE_UNIT PARTITION_ID  — so the identifier is
    # the LAST field, the size the two before it, and the (possibly multi-word) name the rest.
    diskutil list external 2>/dev/null | awk '
        /^\/dev\/disk[0-9]+ \(external, physical\):/ { d=$1; sub("/dev/","",d); sub(":","",d) }
        d && $1=="1:" {
            name=""; for (i=3; i<=NF-3; i++) name = name (name?" ":"") $i
            print d "|" name "|" $(NF-2) " " $(NF-1); d=""
        }'
}

# Print the platform verdict for a just-mounted card at $1 (a /Volumes/... path).
classify() {
    local mp="$1" line plat reason
    line="$("$IDENTIFY" "$mp" 2>/dev/null)"
    plat="${line%%$'\t'*}"; reason="${line#*$'\t'}"
    case "$plat" in
        JETSON)
            echo "  ★ THIS CARD IS FOR JETSON (this session): ${reason}"
            echo "     → boot-log action:  ${HERE}/jetson-bench-connect.sh"
            echo "       (then in the UEFI Shell: connect -r ; map -r ; FSx:\\EFI\\BOOT\\BOOTAA64.EFI)" ;;
        RMBP)
            echo "  → card is for the rMBP (x86) session — NOT jetson; leaving it. (${reason})" ;;
        PI4)
            echo "  → card is for the Pi 4 session — NOT jetson; leaving it. (${reason})" ;;
        *)
            echo "  ? UNKNOWN card — not recognizably jetson/rmbp/pi4. (${reason})"
            echo "    (inspect by hand:  ${IDENTIFY} \"$mp\")" ;;
    esac
}

prev="$(snapshot)"
echo "jetson-card-watch: armed ($(echo "$prev" | grep -c . ) external disk(s) present) — inspecting mounts with identify-card.sh"
while true; do
    sleep 2
    cur="$(snapshot)"
    if [ "$cur" != "$prev" ]; then
        # New disks.
        while IFS='|' read -r d name size; do
            [ -z "$d" ] && continue
            if ! grep -q "^$d|" <<<"$prev"; then
                echo "CARD INSERTED: $d ${name:-<no label>} ${size}"
                # Report the mount point once it appears (up to ~6 s), then classify it.
                mp=""
                for _ in 1 2 3; do
                    mp="$(diskutil info "${d}s1" 2>/dev/null | awk -F': +' '/Mount Point/ {print $2}')"
                    [ -n "$mp" ] && break
                    sleep 2
                done
                if [ -n "$mp" ]; then
                    echo "CARD MOUNTED: $d at $mp"
                    classify "$mp"
                else
                    echo "  (no mount point yet — 'diskutil mount ${d}s1' then '${IDENTIFY} /Volumes/<name>')"
                fi
            fi
        done <<<"$cur"
        # Gone disks.
        while IFS='|' read -r d name size; do
            [ -z "$d" ] && continue
            grep -q "^$d|" <<<"$cur" || echo "CARD REMOVED: $d ${name:-<no label>}"
        done <<<"$prev"
        prev="$cur"
    fi
done
