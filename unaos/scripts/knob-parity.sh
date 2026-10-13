#!/usr/bin/env bash
# KNOBPARITY — the THREE feature vocabularies of this tree must agree, or a build lies.
#
# THE CLASS THIS CLOSES (docs/dev/QUEUE.md §5, BANNERCERT's sweep clause). `./arroyo check` already
# carries a `knob→builder wiring` probe, and it is a good probe: it parses arroyo's knob map for the
# shape
#     [ -n "${UNAOS_X:-}" ] && _feats="${_feats}<names>,"
# and demands that every such knob whose feature a LITERAL x86 cfg leg names is READ by
# `builder/src/main.rs`. It caught `rastmc`. It did NOT catch `sdwrite`, and the reason is entirely
# structural: `sdwrite` is not armed by a positive knob at all — arroyo appends it DEFAULT-ON
#     [ -z "${UNAOS_NOSDWRITE:-}" ] && _feats="${_feats}sdwrite,"
# so it never matched the probe's `-n` sed pattern, rode the banner of every verb, and for every x86
# media image cut since A60 the builder — which is what compiles the kernel the metal actually boots
# — had no entry for it. Found 2026-09-15 by `scripts/banner-cert.sh` on the ARTIFACT, on that
# gate's first armed run. banner-cert finds such a lie ONE ARTIFACT AT A TIME, and only for a
# feature that (i) was on that run's banner and (ii) has a token row. This script is the other half:
# it compares the VOCABULARIES, needs no build, and answers for every name at once.
#
# THE THREE SETS:
#   (a) ARROYO      every feature name `unaos/arroyo` can append to `$_feats` — knob-gated AND
#                   default-on, top-level map AND the in-function appends `esp_jetson` makes.
#   (b) BUILDER     every feature `builder/src/main.rs` pushes (`feats.push("<name>")`), which is
#                   the ONLY list that reaches the kernel an x86 media image boots.
#   (c) CARGO       every feature declared in `crates/kernel/Cargo.toml`'s `[features]` table.
#
# THE RED (exit 1): a name in (a) that is reachable on an x86 media path with NO environment set —
# i.e. appended from a top-level line with no POSITIVE guard (`[ -z "${UNAOS_NO…:-}" ]`, or no guard
# at all) — and absent from (b). That is exactly the sdwrite class, and it is the one difference
# that is a defect by construction rather than a judgement call: such a feature is on the banner of
# every x86 media build and in none of them.
#
# WHY THE OTHER DIFFERENCES ONLY PRINT. (a)\(b) at large is NOT a defect — most of arroyo's map is
# knob-gated and many of those knobs are aarch64-only (`tegra`, the ga10b probes, the orin ladder),
# where the build invokes cargo directly and the builder is not in the path at all. Deciding which
# of those SHOULD be in the builder is the existing wiring probe's job (it has the cfg matrix to
# decide with); this script prints the sets so the number is visible and quantified, and reds only
# on the class that needs no judgement. (a)\(c) and (b)\(c) should both be EMPTY — a feature name
# that Cargo does not declare is a typo that cargo would reject at build time, so a non-empty set
# there means a path that has never been built.
#
# GO-RED (how this was proven to be able to fail — LAWS §5, an ungated gate is not a gate):
#   comment out `builder/src/main.rs`'s `feats.push("sdwrite")` line, run this script:
#     ❌ knob-parity: sdwrite — default-on in arroyo … and NOT pushed by builder/src/main.rs
#     rc=1
#   restore it, rc=0. Measured 2026-09-15; the log is in this arc's evidence.
#
# USAGE:  bash scripts/knob-parity.sh [<workspace-dir>]     (default: the parent of this script's dir)
# EXIT:   0 parity holds · 1 an x86-media-reachable default-on name is unwired in the builder
#         2 PARSE FAILURE — a control probe below did not find a name this tree provably contains,
#           which means a parser broke, not that the tree is clean. An unchecked check is never a
#           silent pass (LAWS §5), so a broken parser is loud and gives NO verdict.

set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── --line <file> --board orin|pi: GATE-IMAGE-LINE for the two aarch64 lines (ARMPARITY) ──────────
# The `--line` gate below holds every kernel knob to the x86 builder's env→feature map, and neither
# aarch64 card is built by the builder: `esp-jetson` compiles from arroyo's TOP-LEVEL `_feats` map,
# `kernel8` from its own CURATED `K8_FEATS` arms (it never reads `_feats`). Run over
# image-line-orin.env / image-line-pi.env that gate reds the board-only knobs by construction (SR102).
# `--board` gates each aarch64 line the way the x86 line is gated, against the map that BOARD builds
# from, at the knob → feature → Cargo.toml → banner-cert places of LAWS' "A knob lives in FIVE places":
#   board map   orin: `[ -n "${UNAOS_X:-}" ] && _feats="${_feats}<f>,"` at column 0 (what esp_jetson
#               builds from); pi: `if|elif [ -n "${UNAOS_X:-}" ]; then` … `K8_FEATS="${K8_FEATS},<f>"`
#               inside kernel8(). A kernel knob missing from it is built WITHOUT its feature (red).
#   Cargo.toml  every feature the board map gives the knob is in crates/kernel's [features] (red).
#   banner-cert every such feature has a bc_table row, or the board's armed build stops NO VERDICT (red).
#   k8-reach    pi only: the knob has no `NA` row in k8-reach.registry ("deliberately absent from the
#               Pi image" contradicts a line that arms it) (red).
#   x86 parity  a knob NOT in the board's BOARD_ONLY row below is a SHARED knob and is held to the x86
#               line's two maps exactly as `--line` holds it — arroyo's top-level map AND the builder.
#               A BOARD_ONLY knob is exempt from the builder; a BOARD_ONLY entry the builder DOES map,
#               or that the board map does not know, is a stale table row (red).
# plus `--line`'s shape, comment, duplicate, UNKNOWN and [media] checks; order = the board map's order.
# Exit 2: a control probe failed (a parser broke). usage: knob-parity.sh --line <file> --board orin|pi [<ws>]
#
# BOARD_ONLY — the knobs a board's line may arm that the x86 builder never maps. One row per board;
# a knob joins by an edit HERE, never by an exclusion in the checks below.
BOARD_ONLY_orin="UNAOS_ORINDESK UNAOS_ORINCLICK UNAOS_TEGRA_EL0 UNAOS_ORINCONWIN UNAOS_ORINRENDER"
BOARD_ONLY_pi="UNAOS_SMP7 UNAOS_V3D UNAOS_VUGPAR UNAOS_PIUSB UNAOS_GENET UNAOS_NETTEST UNAOS_PIRAST UNAOS_PIDESK"
if [ "${1:-}" = "--line" ] && [ "${3:-}" = "--board" ]; then
    LINE="${2:?usage: knob-parity.sh --line <file> --board orin|pi [<workspace-dir>]}"
    BOARD="${4:-}"
    case "$BOARD" in
        orin) BONLY=" ${BOARD_ONLY_orin} " ;;
        pi)   BONLY=" ${BOARD_ONLY_pi} " ;;
        *) echo "❌ image-line[${BOARD:-?}]: NO VERDICT — --board takes orin|pi"; exit 2 ;;
    esac
    WS="${5:-$(dirname "$HERE")}"
    ARROYO="${WS}/arroyo"; BUILDER="${WS}/builder/src/main.rs"; KSRC="${WS}/crates/kernel/src"
    CARGO="${WS}/crates/kernel/Cargo.toml"; BCERT="${WS}/scripts/banner-cert.sh"; K8REG="${WS}/scripts/k8-reach.registry"
    for f in "$LINE" "$ARROYO" "$BUILDER" "$CARGO" "$BCERT" "$K8REG"; do
        [ -f "$f" ] || { echo "❌ image-line[${BOARD}]: NO VERDICT — missing ${f}"; exit 2; }
    done
    # (a') arroyo's top-level knob map as knob|line|features — the x86 parity map, and orin's board map.
    amap="$(awk '/^\[ -n "\$\{UNAOS_[A-Z0-9_]+:-\}" \] && _feats="\$\{_feats\}/ {
                 match($0, /UNAOS_[A-Z0-9_]+/); k = substr($0, RSTART, RLENGTH); s = $0
                 while (match(s, /_feats="\$\{_feats\}[A-Za-z0-9_,-]*,"/)) {
                     t = substr(s, RSTART + 17, RLENGTH - 19); s = substr(s, RSTART + RLENGTH); f[k] = f[k] "," t }
                 if (!(k in l)) { l[k] = NR; o[++n] = k } }
             END { for (i = 1; i <= n; i++) { k = o[i]; x = f[k]; gsub(/,,+/, ",", x); sub(/^,/, "", x); print k "|" l[k] "|" x } }' "$ARROYO")"
    # (b') the builder's env→feature map, read exactly as `--line` reads it.
    bmap="$(awk '{ l = $0; i = index(l, "//"); if (i > 0) l = substr(l, 1, i - 1)
                   if (index(l, "feats.push(") && match(l, /env::var\("UNAOS_[A-Z0-9_]+"\)/)) {
                       k = substr(l, RSTART + 10, RLENGTH - 12); if (!(k in s)) { s[k] = 1; print k "|" NR } } }' "$BUILDER")"
    if [ "$BOARD" = orin ]; then
        dmap="$amap"; dname="arroyo's top-level _feats map (esp_jetson)"
    else
        dname="kernel8()'s K8_FEATS arms"
        dmap="$(awk '/^kernel8\(\) \{/ { in8 = 1; next }  in8 && /^\}/ { in8 = 0 }
                     in8 && /^[ \t]*(if|elif) \[ -n "\$\{UNAOS_[A-Z0-9_]+:-\}" \]; then/ { match($0, /UNAOS_[A-Z0-9_]+/); k = substr($0, RSTART, RLENGTH) }
                     in8 && /^    fi/ { k = "" }
                     in8 && k != "" && match($0, /K8_FEATS="\$\{K8_FEATS\},[A-Za-z0-9_,-]*"/) {
                         t = substr($0, RSTART + 22, RLENGTH - 23); f[k] = f[k] "," t
                         if (!(k in l)) { l[k] = NR; o[++n] = k } }
                     END { for (i = 1; i <= n; i++) { k = o[i]; x = f[k]; gsub(/,,+/, ",", x); sub(/^,/, "", x); print k "|" l[k] "|" x } }' "$ARROYO")"
    fi
    cfeat=" $(awk '/^\[features\]/ { f = 1; next } /^\[/ { f = 0 } f && /^[A-Za-z0-9_-]+[ \t]*=/ { n = $1; sub(/[ \t]*=.*$/, "", n); printf "%s ", n }' "$CARGO")"
    brow=" $(awk '/^bc_table\(\) \{/ { t = 1; next } t && /^TABLE$/ { t = 0 } t && /\|/ { split($0, p, "|"); printf "%s ", p[1] }' "$BCERT")"
    k8na=" $(awk '/^UNAOS_[A-Z0-9_]+[ \t]+NA[ \t]/ { printf "%s ", $1 }' "$K8REG")"
    ctl() { echo "❌ image-line[${BOARD}]: NO VERDICT — control probe: $* (parser broke)"; exit 2; }
    printf '%s\n' "$amap" | grep -q '^UNAOS_WC|[0-9]*|\(.*,\)\?wc\(,\|$\)' || ctl "UNAOS_WC→wc not in arroyo's knob map"
    printf '%s\n' "$bmap" | grep -q '^UNAOS_WC|' || ctl "UNAOS_WC not in the builder's env→feature map"
    printf '%s\n' "$dmap" | grep -q '^UNAOS_WITNESS|[0-9]*|\(.*,\)\?witness\(,\|$\)' || ctl "UNAOS_WITNESS→witness not in ${dname}"
    if [ "$BOARD" = orin ]; then printf '%s\n' "$dmap" | grep -q '^UNAOS_TEGRA|[0-9]*|\(.*,\)\?tegra\(,\|$\)' || ctl "UNAOS_TEGRA→tegra not in ${dname}"
    else printf '%s\n' "$dmap" | grep -q '^UNAOS_SMP7|[0-9]*|smp7$' || ctl "UNAOS_SMP7→smp7 (the elif arm) not in ${dname}"; fi
    case "$cfeat" in *" witness "*) ;; *) ctl "witness not in ${CARGO}'s [features]" ;; esac
    case "$brow" in *" witness "*) ;; *) ctl "witness has no row in banner-cert's bc_table" ;; esac
    case "$k8na" in *" UNAOS_AHCI "*) ;; *) ctl "UNAOS_AHCI has no NA row in k8-reach.registry" ;; esac
    rc=0 n=0 nk=0 nm=0 nb=0 prev=0 prevk="" seen=" "
    red() { echo "❌ image-line[${BOARD}]: $*"; rc=1; }
    for k in $BONLY; do
        printf '%s\n' "$dmap" | grep -q "^${k}|" || red "BOARD_ONLY_${BOARD}: ${k} is not in ${dname} — a stale table row"
        printf '%s\n' "$bmap" | grep -q "^${k}|" && red "BOARD_ONLY_${BOARD}: ${k} IS mapped by builder/src/main.rs — not board-only, a stale table row"
    done
    while IFS= read -r raw || [ -n "$raw" ]; do
        n=$((n+1))
        case "$raw" in ''|'#'*) continue ;; esac
        if [[ "$raw" =~ ^export\ (UNAOS_[A-Z0-9_]+)=1[[:space:]]+#\ (.+)$ ]]; then
            k="${BASH_REMATCH[1]}"; cls=kernel
        elif [[ "$raw" =~ ^export\ (UNAOS_[A-Z0-9_]+)=\"\$\{(UNAOS_[A-Z0-9_]+):\?[^}]+\}\"[[:space:]]+#\ \[media\]\ (.+)$ ]]; then
            k="${BASH_REMATCH[1]}"; cls=media
            [ "$k" = "${BASH_REMATCH[2]}" ] || { red "line ${n}: ${k} takes its value from ${BASH_REMATCH[2]}"; continue; }
        else
            red "line ${n}: MALFORMED (want \`export UNAOS_X=1  # <who added it>\` or the [media] form): ${raw:0:100}"; continue
        fi
        case "$seen" in *" $k "*) red "line ${n}: ${k} DUPLICATE"; continue ;; esac
        seen="${seen}${k} "
        al="$(printf '%s\n' "$amap" | awk -F'|' -v k="$k" '$1==k{print $2; exit}')"
        bl="$(printf '%s\n' "$bmap" | awk -F'|' -v k="$k" '$1==k{print $2; exit}')"
        dl="$(printf '%s\n' "$dmap" | awk -F'|' -v k="$k" '$1==k{print $2; exit}')"
        df="$(printf '%s\n' "$dmap" | awk -F'|' -v k="$k" '$1==k{print $3; exit}')"
        if [ "$cls" = kernel ]; then
            nk=$((nk+1))
            if [ -z "$dl" ]; then
                if [ -n "$al" ] || [ -n "$bl" ] || grep -q "\${${k}[:}]" "$ARROYO"; then
                    red "line ${n}: ${k} — not in ${dname} (the ${BOARD} image would be built WITHOUT it)"
                else red "line ${n}: ${k} UNKNOWN — in no map arroyo or the builder keeps"; fi
                continue
            fi
            for f in ${df//,/ }; do
                case "$cfeat" in *" $f "*) ;; *) red "line ${n}: ${k} → ${f} — crates/kernel/Cargo.toml declares no such feature" ;; esac
                case "$brow" in *" $f "*) ;; *) red "line ${n}: ${k} → ${f} — no banner-cert row (an armed ${BOARD} build stops NO VERDICT on it)" ;; esac
            done
            if [ "$BOARD" = pi ]; then case "$k8na" in *" $k "*) red "line ${n}: ${k} has an NA row in k8-reach.registry (deliberately absent from the Pi image) yet the line arms it" ;; esac; fi
            case "$BONLY" in
                *" $k "*) nb=$((nb+1)) ;;
                *) [ -n "$al" ] || red "line ${n}: ${k} — shared (not BOARD_ONLY_${BOARD}) and arroyo's top-level knob map does not map it (the x86 banner would not name it)"
                   [ -n "$bl" ] || red "line ${n}: ${k} — shared (not BOARD_ONLY_${BOARD}) and builder/src/main.rs does not map it (the x86 image would be built WITHOUT it); board-only? add it to BOARD_ONLY_${BOARD}" ;;
            esac
            pos="$dl"
        else
            nm=$((nm+1))
            pos="$(grep -n "\${${k}[:}]" "$ARROYO" | head -1 | cut -d: -f1)"
            [ -n "$pos" ] || { red "line ${n}: ${k} UNKNOWN — [media] but arroyo never reads it"; continue; }
            if [ -n "$bl" ] || grep -q "\"${k}\"" "$BUILDER"; then red "line ${n}: ${k} is marked [media] but builder/src/main.rs reads it"; fi
            if grep -rqs "env!(\"${k}\")" "$KSRC"; then red "line ${n}: ${k} is marked [media] but the kernel reads it with env!/option_env!"; fi
        fi
        if [ "$pos" -lt "$prev" ]; then red "line ${n}: ${k} (arroyo:${pos}) OUT OF ORDER after ${prevk} (arroyo:${prev}) — the file is in ${dname} order"
        else prev="$pos"; prevk="$k"; fi
    done < "$LINE"
    [ $((nk+nm)) -gt 0 ] || red "${LINE} holds no knob"
    if [ "$rc" -eq 0 ]; then echo "✅ image-line[${BOARD}] OK: ${nk} kernel knobs in ${dname}, every feature in Cargo.toml and banner-cert; $((nk-nb)) shared in arroyo's map AND the builder's, ${nb} BOARD_ONLY_${BOARD}; ${nm} [media]; nothing unknown, in board-map order"
    else echo "❌ image-line[${BOARD}]: $(basename "$LINE") -> FAIL"; fi
    exit "$rc"
fi

# ── --line <file>: GATE-IMAGE-LINE (FOLDBUILD, rmbp-ledger B517) ───────────────────────────────
# The image build line lives in ONE file (scripts/image-line.env). This entry checks that file
# against the SAME two vocabularies this script reads below, keyed by KNOB rather than feature:
#   (a') arroyo's knob map    `[ -n "${UNAOS_X:-}" ] && _feats="${_feats}<names>,"` at column 0
#   (b') the builder's map     `std::env::var("UNAOS_X")` on a line that also `feats.push(...)`es,
#                              each line truncated at its first `//` first (the commented-push lesson
#                              below applies unchanged: a commented-out mapping is UNWIRED)
# Every line must be `export UNAOS_X=1  # <who added it>` (a kernel knob: in (a') AND in (b')) or
# `export UNAOS_X="${UNAOS_X:?…}"  # [media] <who>` (a bench-local value: arroyo reads it, the
# builder and the kernel sources never do, so it cannot change what cargo compiles). Red (exit 1):
# a malformed line, a line with no comment, a duplicate, a kernel knob missing from (a') or (b'),
# a `[media]` knob the builder or the kernel reads, a knob arroyo never names (UNKNOWN), knobs out
# of arroyo's order. Exit 2: a control probe (UNAOS_WC in both maps) failed — a parser broke.
# usage: knob-parity.sh --line <image-line.env> [<workspace-dir>]
if [ "${1:-}" = "--line" ]; then
    LINE="${2:?usage: knob-parity.sh --line <image-line.env> [<workspace-dir>]}"
    WS="${3:-$(dirname "$HERE")}"
    ARROYO="${WS}/arroyo"; BUILDER="${WS}/builder/src/main.rs"; KSRC="${WS}/crates/kernel/src"
    for f in "$LINE" "$ARROYO" "$BUILDER"; do
        [ -f "$f" ] || { echo "❌ image-line: NO VERDICT — missing ${f}"; exit 2; }
    done
    amap="$(awk '/^\[ -n "\$\{UNAOS_[A-Z0-9_]+:-\}" \] && _feats="\$\{_feats\}/ {
                 match($0, /UNAOS_[A-Z0-9_]+/); k = substr($0, RSTART, RLENGTH); if (!(k in s)) { s[k] = 1; print k "|" NR } }' "$ARROYO")"
    bmap="$(awk '{ l = $0; i = index(l, "//"); if (i > 0) l = substr(l, 1, i - 1)
                   if (index(l, "feats.push(") && match(l, /env::var\("UNAOS_[A-Z0-9_]+"\)/)) {
                       k = substr(l, RSTART + 10, RLENGTH - 12); if (!(k in s)) { s[k] = 1; print k "|" NR } } }' "$BUILDER")"
    printf '%s\n' "$amap" | grep -q '^UNAOS_WC|' || { echo "❌ image-line: NO VERDICT — control probe: UNAOS_WC not in arroyo's knob map (parser broke)"; exit 2; }
    printf '%s\n' "$bmap" | grep -q '^UNAOS_WC|' || { echo "❌ image-line: NO VERDICT — control probe: UNAOS_WC not in the builder's env→feature map (parser broke)"; exit 2; }
    rc=0 n=0 nk=0 nm=0 prev=0 prevk="" seen=" "
    red() { echo "❌ image-line: $*"; rc=1; }
    while IFS= read -r raw || [ -n "$raw" ]; do
        n=$((n+1))
        case "$raw" in ''|'#'*) continue ;; esac
        if [[ "$raw" =~ ^export\ (UNAOS_[A-Z0-9_]+)=1[[:space:]]+#\ (.+)$ ]]; then
            k="${BASH_REMATCH[1]}"; cls=kernel
        elif [[ "$raw" =~ ^export\ (UNAOS_[A-Z0-9_]+)=\"\$\{(UNAOS_[A-Z0-9_]+):\?[^}]+\}\"[[:space:]]+#\ \[media\]\ (.+)$ ]]; then
            k="${BASH_REMATCH[1]}"; cls=media
            [ "$k" = "${BASH_REMATCH[2]}" ] || { red "line ${n}: ${k} takes its value from ${BASH_REMATCH[2]}"; continue; }
        else
            red "line ${n}: MALFORMED (want \`export UNAOS_X=1  # <who added it>\` or the [media] form): ${raw:0:100}"; continue
        fi
        case "$seen" in *" $k "*) red "line ${n}: ${k} DUPLICATE"; continue ;; esac
        seen="${seen}${k} "
        al="$(printf '%s\n' "$amap" | awk -F'|' -v k="$k" '$1==k{print $2; exit}')"
        bl="$(printf '%s\n' "$bmap" | awk -F'|' -v k="$k" '$1==k{print $2; exit}')"
        if [ "$cls" = kernel ]; then
            nk=$((nk+1))
            if [ -z "$al" ] && [ -z "$bl" ]; then
                if grep -q "\${${k}[:}]" "$ARROYO"; then red "line ${n}: ${k} UNKNOWN as a kernel knob (arroyo names it but maps no feature to it, nor does the builder) — mark it [media] if it only stages the medium"
                else red "line ${n}: ${k} UNKNOWN — in neither arroyo's knob map nor the builder's env→feature map"; fi
                continue
            fi
            [ -n "$al" ] || red "line ${n}: ${k} — builder/src/main.rs:${bl} maps it, arroyo's knob map does not (the banner would not name it)"
            [ -n "$bl" ] || red "line ${n}: ${k} — arroyo:${al} maps it, builder/src/main.rs does not (the image would be built WITHOUT it)"
            pos="${al:-0}"
        else
            nm=$((nm+1))
            pos="$(grep -n "\${${k}[:}]" "$ARROYO" | head -1 | cut -d: -f1)"
            [ -n "$pos" ] || { red "line ${n}: ${k} UNKNOWN — [media] but arroyo never reads it"; continue; }
            if [ -n "$bl" ] || grep -q "\"${k}\"" "$BUILDER"; then red "line ${n}: ${k} is marked [media] but builder/src/main.rs reads it"; fi
            if grep -rqs "env!(\"${k}\")" "$KSRC"; then red "line ${n}: ${k} is marked [media] but the kernel reads it with env!/option_env!"; fi
        fi
        if [ "$pos" -lt "$prev" ]; then red "line ${n}: ${k} (arroyo:${pos}) OUT OF ORDER after ${prevk} (arroyo:${prev}) — the file is in arroyo's knob order"
        else prev="$pos"; prevk="$k"; fi
    done < "$LINE"
    [ $((nk+nm)) -gt 0 ] || red "${LINE} holds no knob"
    if [ "$rc" -eq 0 ]; then echo "✅ image-line OK: ${nk} kernel knobs in arroyo's map AND the builder's, ${nm} [media] knob(s) arroyo stages and nothing compiles, nothing unknown, in knob order"
    else echo "❌ image-line: $(basename "$LINE") -> FAIL"; fi
    exit "$rc"
fi

WS="${1:-$(dirname "$HERE")}"
ARROYO="${WS}/arroyo"
BUILDER="${WS}/builder/src/main.rs"
CARGO="${WS}/crates/kernel/Cargo.toml"

for f in "$ARROYO" "$BUILDER" "$CARGO"; do
    [ -f "$f" ] || { echo "❌ knob-parity: NO VERDICT — missing ${f}" >&2; exit 2; }
done

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# ── (a) ARROYO ────────────────────────────────────────────────────────────────────────────────
# Every `_feats="${_feats}<comma-list>,"` in the file, with its line number. This deliberately does
# NOT restrict to the `[ -n "${UNAOS_…}" ]` shape the wiring probe parses — that restriction is the
# hole sdwrite went through. It catches the top-level map, the `case`-arm appends (`ga10bprobe5a`),
# the `&& { …; }` compound ones (`bsprun`) and `esp_jetson`'s in-function forcing (`tegra`,
# `tegrasmp`, `apsrun`, `bsptick`, `bsprun`, `sdmmc`).
#
# The DEFAULT-ON classification (third column) is what the red keys on:
#   toplevel  the append sits at column 0 — the knob map proper, which every verb runs at script
#             load. `esp_jetson`'s appends are indented inside a function body and are aarch64-only,
#             so they are not x86-media-reachable and must not be demanded of the x86 builder.
#   noguard   the line carries no POSITIVE test: no `[ -n "` (neither `[ -n "${UNAOS_X:-}"` nor the
#             derived-variable form `[ -n "$_p5_metal"`) and no `[ "${UNAOS_X:-}" = "n" ]`. What is
#             left is `[ -z "${UNAOS_NO…:-}" ]` (default-on) or nothing at all (unconditional).
awk '
    /_feats="\$\{_feats\}/ {
        line = $0
        toplevel = (line ~ /^[^ \t]/) ? 1 : 0
        guarded  = (index(line, "[ -n \"") > 0 || line ~ /\[ "\$\{UNAOS_/) ? 1 : 0
        s = line
        while (match(s, /_feats="\$\{_feats\}[A-Za-z0-9_,-]*,"/)) {
            tok = substr(s, RSTART, RLENGTH)
            s   = substr(s, RSTART + RLENGTH)
            sub(/^_feats="\$\{_feats\}/, "", tok)
            sub(/,"$/, "", tok)
            n = split(tok, parts, ",")
            for (i = 1; i <= n; i++)
                if (parts[i] != "")
                    printf "%s|%d|%s\n", parts[i], NR, ((toplevel && !guarded) ? "default-on" : "knob-gated")
        }
    }
' "$ARROYO" > "$TMP/a.raw"

# One row per NAME: its line numbers joined, and default-on if ANY append of it is default-on.
awk -F'|' '
    { k = $1 SUBSEP $2; if (!(k in seen)) { seen[k] = 1; lines[$1] = ($1 in lines) ? lines[$1] "," $2 : $2 }
      if ($3 == "default-on") dflt[$1] = 1 }
    END { for (n in lines) printf "%s|%s|%s\n", n, lines[n], (n in dflt ? "default-on" : "knob-gated") }
' "$TMP/a.raw" | LC_ALL=C sort > "$TMP/a.tsv"
cut -d'|' -f1 "$TMP/a.tsv" | LC_ALL=C sort -u > "$TMP/a.names"
awk -F'|' '$3=="default-on"{print $1"|"$2}' "$TMP/a.tsv" > "$TMP/a.default"
cut -d'|' -f1 "$TMP/a.default" | LC_ALL=C sort -u > "$TMP/a.default.names"

# ── (b) BUILDER ───────────────────────────────────────────────────────────────────────────────
# `feats.push("<name>")` anywhere in the file, including the default-on ones
# (`if …is_err() { feats.push("ehcihid") }`) and the `match`-arm form (`=> feats.push("bar1exp-uc")`).
#
# EVERY LINE IS TRUNCATED AT ITS FIRST `//` FIRST, and that is not tidiness — the go-red drill for
# this script is "comment out the builder's sdwrite push", and a plain `grep feats.push` HAPPILY
# MATCHES THE COMMENTED-OUT LINE, so the first run of the drill came back green. That is the exact
# failure mode this gate exists to catch, seen from the inside: a feature that is named in the file
# and compiled into nothing. A commented-out push is UNWIRED, and this parser must say so. (The
# builder's knob block has no string literal containing `//`, so the truncation costs nothing here.)
awk '
    { line = $0; i = index(line, "//"); if (i > 0) line = substr(line, 1, i - 1)
      s = line
      while (match(s, /feats\.push\("[A-Za-z0-9_-]*"\)/)) {
          tok = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
          sub(/^feats\.push\("/, "", tok); sub(/"\)$/, "", tok)
          printf "%s|%d\n", tok, NR
      } }
' "$BUILDER" \
  | awk -F'|' '{ lines[$1] = ($1 in lines) ? lines[$1] "," $2 : $2 }
               END { for (n in lines) printf "%s|%s\n", n, lines[n] }' \
  | LC_ALL=C sort > "$TMP/b.tsv"
cut -d'|' -f1 "$TMP/b.tsv" | LC_ALL=C sort -u > "$TMP/b.names"

# ── (c) CARGO ─────────────────────────────────────────────────────────────────────────────────
# The `[features]` table of the kernel crate, to its next `[section]`. `default = [...]` is a
# feature name in Cargo's own grammar but not a knob, so it is dropped.
awk '
    /^\[features\]/ { inf = 1; next }
    /^\[/           { inf = 0 }
    inf && /^[A-Za-z0-9_-]+[ \t]*=/ {
        name = $1; sub(/[ \t]*=.*$/, "", name)
        if (name != "default") printf "%s|%d\n", name, NR
    }
' "$CARGO" | LC_ALL=C sort > "$TMP/c.tsv"
cut -d'|' -f1 "$TMP/c.tsv" | LC_ALL=C sort -u > "$TMP/c.names"

# ── CONTROL PROBES ────────────────────────────────────────────────────────────────────────────
# Each asserts a name this tree PROVABLY contains. A miss means the parser above broke (a spacing
# change, a quoting change, a section rename), not that the tree is clean — so it is a loud 2 with
# no verdict, never a green run. NOTE which names are used: `sdwrite` is deliberately NOT a control
# on the BUILDER side, because the go-red drill removes exactly that line and must produce a
# verdict of 1 (a finding), not 2 (a broken gate).
_kp_ctl() {  # _kp_ctl <file> <name> <what>
    grep -qx -- "$2" "$1" && return 0
    echo "❌ knob-parity: NO VERDICT — control probe FAILED: '$2' not found in $3."
    echo "   The parser for that set is broken (idiom or layout changed); this run proves nothing."
    exit 2
}
_kp_ctl "$TMP/a.names"         "wc"      "(a) arroyo's _feats appends"
_kp_ctl "$TMP/a.names"         "sdwrite" "(a) arroyo's _feats appends"
_kp_ctl "$TMP/a.names"         "tegra"   "(a) arroyo's _feats appends (the in-function esp_jetson forcing)"
_kp_ctl "$TMP/a.default.names" "smolnet" "(a) the DEFAULT-ON subset — the classifier is broken"
_kp_ctl "$TMP/b.names"         "wc"      "(b) builder/src/main.rs feats.push"
_kp_ctl "$TMP/c.names"         "witness" "(c) crates/kernel/Cargo.toml [features]"

# ── COUNT FIRST, THEN QUANTIFY ────────────────────────────────────────────────────────────────
NA=$(wc -l < "$TMP/a.names" | tr -d ' ')
ND=$(wc -l < "$TMP/a.default.names" | tr -d ' ')
NB=$(wc -l < "$TMP/b.names" | tr -d ' ')
NC=$(wc -l < "$TMP/c.names" | tr -d ' ')
echo "⚡ knob-parity: (a) arroyo=${NA} names (${ND} of them default-on, x86-media-reachable with no env)"
echo "⚡ knob-parity: (b) builder=${NB} names   (c) crates/kernel Cargo [features]=${NC} names"

# LC_ALL=C on every sort AND on comm: the name set contains `ga10bprobe5` and `ga10bprobe5a`, whose
# relative order differs between a locale collation and byte order, and comm silently emits garbage
# (it only warns) when its inputs disagree with its own ordering. Byte order, everywhere, or the
# difference sets are fiction.
LC_ALL=C comm -23 "$TMP/a.names" "$TMP/b.names" > "$TMP/a_not_b"
LC_ALL=C comm -23 "$TMP/a.names" "$TMP/c.names" > "$TMP/a_not_c"
LC_ALL=C comm -23 "$TMP/b.names" "$TMP/c.names" > "$TMP/b_not_c"

_kp_dump() {  # _kp_dump <file-of-names> <tsv-with-lines> <label>
    local n; n=$(wc -l < "$1" | tr -d ' ')
    echo "── ${3}: ${n}"
    [ "$n" -eq 0 ] && return 0
    local name lines
    while read -r name; do
        lines="$(awk -F'|' -v n="$name" '$1==n{print $2; exit}' "$2")"
        printf '     %-32s %s\n' "$name" "${lines:+@${lines}}"
    done < "$1"
}
_kp_dump "$TMP/a_not_b" "$TMP/a.tsv" "(a)\\(b) arroyo can arm it, the x86 media builder never pushes it  [arroyo lines]"
_kp_dump "$TMP/a_not_c" "$TMP/a.tsv" "(a)\\(c) arroyo names it, crates/kernel declares no such feature  [arroyo lines]"
_kp_dump "$TMP/b_not_c" "$TMP/b.tsv" "(b)\\(c) builder pushes it, crates/kernel declares no such feature  [builder lines]"

# ── THE VERDICT ───────────────────────────────────────────────────────────────────────────────
miss=""
while read -r name; do
    grep -qx -- "$name" "$TMP/b.names" || miss="${miss} ${name}"
done < "$TMP/a.default.names"

rc=0
if [ -n "$miss" ]; then
    for name in $miss; do
        lines="$(awk -F'|' -v n="$name" '$1==n{print $2; exit}' "$TMP/a.default")"
        echo "❌ knob-parity: ${name} — default-on in arroyo (arroyo:${lines}, no positive knob, so it"
        echo "   rides the \`⚡ kernel features:\` banner of EVERY x86 media verb) and NOT pushed by"
        echo "   builder/src/main.rs, which is what compiles the kernel that media boots. Every card"
        echo "   cut from it ships WITHOUT the feature its own build log named. This is the sdwrite"
        echo "   class. Fix: one \`feats.push(\"${name}\")\` line beside its siblings in the builder's"
        echo "   knob-mapping block, under the same condition arroyo uses."
    done
    rc=1
fi
if [ "$rc" -eq 0 ]; then
    echo "✅ knob-parity OK (every default-on arroyo feature is pushed by the x86 media builder)"
fi
exit "$rc"
