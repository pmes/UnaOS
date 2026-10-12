#!/bin/bash
# foldleg.sh — FOLDLEG (FOLDBUILD, rmbp-ledger B517): the x86 kernel + loader BUILT under THE image build line, a fold leg.
#
# WHY. Image 21 did not compile. merge20's fold gate was green (x86 metal shape, aarch64 desktop, tegra, builder, gates,
# host tests) and the bench's `esp-x86` under the image line failed on three lines — E0252 in fs/jobs.rs, E0069 x2 in
# winmemory::save_frame (the bench's 2ff7d418, 0493e1f6, c2ff8706) — because the image's knob SET is not any leg's knob
# set, and the line itself lived only as a chain of STATE paragraphs no gate could read. FLIGHT28.md §2: "a kernel build
# belongs in every fold's legs". The line is now ONE file (scripts/image-line.env); this leg builds under it.
#
# WHAT IT RUNS. Exactly the kernel + bootloader cargo builds `./arroyo esp-x86` runs — the builder's OWN env→feature
# derivation (builder/src/main.rs), invoked the way esp_x86 invokes it (`cargo +nightly run --release` in builder/),
# with `--build-only`: the builder stops after the loader build. (The one env difference from esp-x86: arroyo's
# load-time UNAOS_GIT_SHA stamp is not exported here — an option_env! version string, not a compile input.) No user blobs,
# no ESP, no card, no QEMU. In particular NOT build_user_vug_x86 -> build_selfbuild_x86 -> SELFBUILD5, the path that
# downloads the LLVM source (B501's bootleg died on disk there): this leg never enters it, and asserts so.
#
# THE TWO STAGES. outer (default): clear every UNAOS_* the caller's shell carries, export the file's knobs (a `:?`
# bench-local value that is unset is SKIPPED when no compile reads it — the builder and the kernel sources are
# grepped for it — and REFUSED, rc 3, when one does), then run the inner stage in that env. inner: asks the tree's
# own arroyo for its `⚡ kernel features:` banner under that env (`arroyo state`: the banner prints at load), runs the
# builder, prints the verdict. Both stages are this script, so a tree that predates FOLDBUILD builds the same way.
#
# usage: ./arroyo foldleg [--line <image-line.env>]   (= foldleg.sh <unaos dir> [--line <file>])
#        The tree built is <unaos dir>'s (its own builder, its own sources); the line defaults to that tree's
#        scripts/image-line.env, else this script's own copy (a tree that predates FOLDBUILD).
# exit:  0 PASS · 1 FAIL (a cargo build failed) · 2 NO VERDICT (no line, an armed SELFBUILD knob, staging touched)
#        3 REFUSED (a `:?` value a compile reads is unset)
# wire:  FOLDLEG: x86 kernel+loader built under image-line.env @<sha-of-file> features=<n> -> PASS|FAIL rc=<n>
set -u
U="${1:?usage: foldleg.sh <unaos dir> [--line <image-line.env>] | --inner <line> <sha>}"; shift
U="$(cd "$U" && pwd)"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "${1:-}" = "--inner" ]; then
    LINE="$2" SHA="$3"
    log="$U/target/foldleg-builder.log"; mkdir -p "$U/target"
    lld="$U/target/selfbuild/lld/src.tar.xz"; stamp="$(mktemp "$U/target/.foldleg.XXXXXX")"
    # A tree whose builder predates FOLDBUILD has no --build-only: package-only, test-f and source-along off, so it
    # still stops before QEMU and fetches nothing (the kernel build, the part this leg judges, is identical).
    extra=() path="builder --build-only"
    grep -q -- '--build-only' "$U/builder/src/main.rs" \
        || { extra=(UNAOS_PACKAGE_ONLY=1 UNAOS_TESTF=0 UNAOS_NOSRC=1); path="builder UNAOS_PACKAGE_ONLY (pre-FOLDBUILD tree)"; echo "FOLDLEG: note — this tree's builder predates --build-only; ran with ${extra[*]} (no QEMU, no fetch)"; }
    # arroyo's banner for THIS env, from the tree's own knob map (printed at load for every verb but the kernel8 family).
    ( cd "$U" && ./arroyo state 2>&1 ) | sed 's/\x1b\[[0-9;]*m//g' | awk 'index($0,"⚡ kernel features:") { print; exit }'
    t0=$(date +%s)
    ( cd "$U/builder" && env "${extra[@]}" cargo +nightly run --release -- --build-only ) > "$log" 2>&1
    rc=$?
    secs=$(( $(date +%s) - t0 ))
    feats="$(awk 'index($0,"   kernel features: ") { sub(/^.*   kernel features: /,""); l=$0 } END { print l }' "$log")"
    nf="$(printf '%s' "$feats" | tr ',' '\n' | awk 'NF && !s[$0]++ { n++ } END { print n + 0 }')"   # distinct names (the builder pushes gen7 once per gen7 knob)
    echo "FOLDLEG: builder features (the list cargo got): ${feats:-<none printed>}"
    # The errors, if any, as rustc printed them (awk, never bare grep — LAWS).
    awk '/^error(\[E[0-9]+\])?:/ { n++; if (n <= 12) { print "FOLDLEG: " $0; want = 1; next } } want && /^ *--> / { print "FOLDLEG:   " $0; want = 0 }' "$log"
    touched=no
    [ -f "$lld" ] && [ "$lld" -nt "$stamp" ] && touched=yes
    [ -d "$U/target/selfbuild" ] && [ -n "$(find "$U/target/selfbuild" -newer "$stamp" -print -quit 2>/dev/null)" ] && touched=yes
    rm -f "$stamp"
    echo "FOLDLEG: selfbuild=not-armed path=${path} llvm_src_touched=${touched} build_s=${secs} log=${log#$U/}"
    if [ "$touched" = yes ]; then
        echo "FOLDLEG: x86 kernel+loader built under image-line.env @${SHA} features=${nf} -> NO VERDICT rc=2 (SELFBUILD staging ran)"; exit 2
    fi
    if [ "$rc" -eq 0 ]; then
        echo "FOLDLEG: x86 kernel+loader built under image-line.env @${SHA} features=${nf} -> PASS rc=0"; exit 0
    fi
    echo "FOLDLEG: x86 kernel+loader built under image-line.env @${SHA} features=${nf} -> FAIL rc=${rc}"; exit 1
fi

LINE=""
[ "${1:-}" = "--line" ] && LINE="${2:?--line needs a file}"
[ -n "$LINE" ] || { [ -f "$U/scripts/image-line.env" ] && LINE="$U/scripts/image-line.env"; } || true
[ -n "$LINE" ] || LINE="$HERE/image-line.env"
[ -f "$LINE" ] || { echo "FOLDLEG: no image line at ${LINE} -> NO VERDICT rc=2"; exit 2; }
LINE="$(cd "$(dirname "$LINE")" && pwd)/$(basename "$LINE")"
[ -x "$U/arroyo" ] && [ -f "$U/builder/src/main.rs" ] || { echo "FOLDLEG: ${U} is not an unaos dir (no arroyo/builder) -> NO VERDICT rc=2"; exit 2; }
# @<sha> = the last commit that touched the file when the file is clean at HEAD (what a STATE cites), else
# `uncommitted`; blob= is the content's own git hash either way (`git hash-object unaos/scripts/image-line.env`).
_d="$(dirname "$LINE")"
SHA="$(git -C "$_d" log -1 --format=%h -- "$LINE" 2>/dev/null)"
{ [ -n "$SHA" ] && git -C "$_d" diff --quiet HEAD -- "$LINE" 2>/dev/null; } || SHA="uncommitted"
SHA="${SHA} blob=$(git hash-object "$LINE" 2>/dev/null | cut -c1-8)"

# SELFBUILD staging is armed by no knob (esp_x86 runs it unconditionally through the user-blob builds this leg skips);
# a line that names a SELFBUILD/LLD knob is asking for it, and this leg refuses rather than pretend to honour it.
if awk '/^export UNAOS_(SELFBUILD|LLD_LNX)/ { f = 1 } END { exit !f }' "$LINE"; then
    echo "FOLDLEG: $(basename "$LINE") arms a SELFBUILD/LLD knob — this leg never stages the toolchain -> NO VERDICT rc=2"; exit 2
fi

# A clean knob env: the leg builds THE LINE, not the caller's shell — except a `:?` bench-local value the caller set.
declare -A keep=()
for v in $(awk '/^export UNAOS_[A-Z0-9_]+=.*:\?/ { sub(/^export /, ""); sub(/=.*/, ""); print }' "$LINE"); do
    [ -n "${!v:-}" ] && keep[$v]="${!v}"
done
cleared=0
for v in $(compgen -e | awk '/^UNAOS_/'); do [ -n "${keep[$v]:-}" ] && continue; unset "$v"; cleared=$((cleared+1)); done
nk=0 skipped=""
while IFS= read -r raw || [ -n "$raw" ]; do
    case "$raw" in export\ UNAOS_*) ;; *) continue ;; esac
    k="${raw#export }"; k="${k%%=*}"
    if [[ "$raw" =~ :\? ]]; then
        if [ -z "${!k:-}" ]; then
            # Does any COMPILE read it? The builder (env::var) or the kernel (env!/option_env!).
            if grep -q "\"${k}\"" "$U/builder/src/main.rs" || grep -rqs "env!(\"${k}\")" "$U/crates/kernel/src"; then
                echo "FOLDLEG: ${k} is unset and the kernel build reads it — set it (bench-local) -> REFUSED rc=3"; exit 3
            fi
            skipped="${skipped}${skipped:+,}${k}"; continue
        fi
    fi
    eval "$raw" || { echo "FOLDLEG: $(basename "$LINE"): cannot evaluate: ${raw:0:80} -> NO VERDICT rc=2"; exit 2; }
    nk=$((nk+1))
done < "$LINE"
echo "FOLDLEG: tree=${U} line=${LINE} @${SHA} knobs=${nk} caller_knobs_cleared=${cleared} skipped=${skipped:-none}${skipped:+ (media-only: no compile reads it — arroyo stages it onto the card)}"
t0=$(date +%s)
bash "$HERE/foldleg.sh" "$U" --inner "$LINE" "$SHA"
rc=$?
echo "FOLDLEG: runtime_s=$(( $(date +%s) - t0 ))"
exit "$rc"
