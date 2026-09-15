#!/usr/bin/env bash
# BANNERCERT — the `⚡ kernel features:` banner and the built artifact must AGREE, or the build is red.
#
# THE INCIDENT THIS EXISTS FOR (docs/dev/QUEUE.md §5, 2026-09-13 "A FEATURE GATE KEYED ON ONE
# SPELLING OF A VERB, AND THE BUILD LOG LIED ABOUT IT"): `unaos/arroyo` armed `ga10bprobe5a` from a
# single string equality against ONE of the six spellings the dispatcher accepts for the jetson
# media verb. Five of six spellings built a feature set the banner did not describe — `esp-jetson`
# built `ga10bprobe5,ga10bprobe5a`, the `esp-jetson-img` step that follows it in the same chain
# rebuilt `ga10bprobe5` alone and OVERWROTE kernel.elf — and the banner printed the rung either way.
# A card cut from that media booted, printed no rung, and looked like a clean flight. `./arroyo
# check` was green; the diff was green; the banner was green. The only thing that caught it was
# `LC_ALL=C grep -a -o -F '<witness>' kernel.elf` on the ARTIFACT (LAWS §5: "An instrument's
# presence is proven in the artifact, never in the diff, the check or the banner").
#
# So: for every feature the banner names, this script asserts the artifact carries that feature's
# CERTIFYING STRING — a literal that exists in the built image if and only if the feature is
# compiled in — and for a small set of features the banner does NOT name it asserts their control
# string is ABSENT, which is the same defect seen from the other side (a feature quietly compiled
# in that the banner never claimed).
#
# USAGE:  bash scripts/banner-cert.sh <artifact> <banner-feature-list>
#           <artifact>             the built kernel ELF (target/x86_64_esp/kernel.elf)
#           <banner-feature-list>  exactly the comma-separated list the `⚡ kernel features:` banner
#                                  printed for this build — NOT $KERNEL_FEATURES, NOT the knob line.
#                                  The banner is the claim under test; passing anything else makes
#                                  the gate vacuous.
#
# WHAT IT FOUND ON ITS FIRST ARMED RUN (2026-09-15, rmbp flight-7 knob line, `esp-x86`), AND FIXED
# IN THE SAME ARC: `arroyo:1992` put `sdwrite` on the banner of every verb while
# `builder/src/main.rs` — which is what compiles the kernel the media actually boots — never read
# it, so every x86 media image cut since A60 shipped without the feature the log claimed. One
# `esp-x86` run printed both lists and they differed by exactly that name. Ruling: arroyo is right,
# `sdwrite` rides every image; the builder gained the knob beside its siblings and the cert now
# reads 28/28. The gate found it; nobody was looking.
#
# OUTPUT:  one line per feature — `feature=<f> witness=<token> hits=<n> -> OK|MISSING|...`
# EXIT:    0 every named feature certified (registered divergences, below, do not red)
#          1 a MISSING (banner named it, the artifact does not carry it) or a LEAK (a control
#            string for a feature the banner did NOT name is in the artifact)
#          2 NO VERDICT — a banner feature with no row in the table below, or a row whose artifact
#            is not on disk. An unchecked check is never SILENTLY skipped (LAWS §5): a feature this
#            table does not know about is a loud 2, never a pass.
#
# THE TABLE IS THE GATE. Adding a feature to arroyo's knob map without adding a row here makes the
# next build that arms it exit 2 by name. That is the intended failure: the fix is one row.
#
# HOW A ROW IS SEEDED (do this, do not guess):
#   1. Find a string literal whose ONLY occurrences in crates/ sit under `#[cfg(feature = "<f>")]`
#      (or inside a module whose `pub mod` is so gated, or a feature <f> implies in Cargo.toml).
#   2. It must be >= 9 bytes. Shorter literals get immediate-encoded by LLVM and never reach
#      .rodata, so a short token measures 0 on a build that DOES carry the feature (LAWS §5).
#      This script refuses to run with a shorter token registered.
#   3. Cut the token at the first `{` — `format_args!` splits a format string into the literal
#      pieces BETWEEN its holes, so only the piece before the first hole is one contiguous string.
#   4. NEVER seed from a literal a `const fn` consumes. BANNERCERT2 found `ga10bprobe5` seeded on the
#      64-hex-character vendor digest at `ga10b_fw.rs:75` — which is the argument to
#      `const fn hx(s: &str) -> [u8; 32]`, evaluated at COMPILE TIME. Only the 32 decoded bytes reach
#      the image; the ASCII string never exists in any artifact on any arch, so that row measured 0 on
#      a build that carried the feature and would have measured 0 forever. A token must be a literal
#      the RUNNING code prints or compares, not one the compiler folds away.
#   5. THE ROW'S `cond` MUST NAME THE CALLER'S GATE, not only the module's. Same finding, same row: the
#      whole `ga10b_fw` module is gated on `ga10bprobe5` alone — and with neither of its two callers
#      compiled in (`ga10b_ignite`, under `ga10bprobe5a`; the QEMU fixture at `main.rs:1749`, under
#      `witness`) the linker garbage-collects the module and every byte of its `.rodata` with it. A
#      feature can be in the cargo feature set, compile, and still put NOTHING in the artifact because
#      nothing calls it. Seed the cond from the call sites (`grep -rn '<module>::'`), not from the
#      `pub mod` line.
#   6. A FEATURE CAN BE COMPILED IN AND ITS LITERAL STILL ABSENT BECAUSE THE CONFIGURATION MAKES THAT
#      CODE UNREACHABLE — the negated-cond case, and BANNERCERT2 met it on the Pi. `witness`'s token
#      lives in `kernel_main`'s post-GUI tail; `main.rs:79` declares in its own `cfg_attr` that
#      `baremetal`, `bootlog`, `usbdebug` and `tegra` each make that tail unreachable, and the
#      compiler is told so. So a `UNAOS_WITNESS=1 ./arroyo kernel8` image carries the whole witness
#      battery and CANNOT carry that string — a row with no cond called it a MISSING and would have
#      red-lined every Pi desktop build. Write the cond as `!baremetal,!bootlog,!usbdebug,!tegra`,
#      copied from the `cfg_attr` rather than guessed.
#   7. MEASURE it: build media with the feature armed and
#      `LC_ALL=C grep -a -o -F -- '<token>' <artifact> | wc -l` must be > 0. Mark the row `measured`.
#      A row seeded from source reading alone is marked `unmeasured-here` and stays that way until
#      an artifact for that arch proves it.
#
# ROW FORMAT:  feature|token|cond|state
#   token   the certifying string. A LEADING `!` inverts the row: the token must be ABSENT when the
#           feature is ON (used where the only gated literal is the not-compiled-in message).
#           A LEADING `@boot ` routes the check to EFI/BOOT/BOOTX64.EFI beside the artifact instead
#           of the artifact itself (for cross-crate boot-info ABI knobs, whose code is in the
#           bootloader, not the kernel).
#           The single word `-` means NO CERTIFYING STRING EXISTS; `cond` then carries the reason.
#           Such a row prints NOWITNESS — loud, named, counted in the summary, never silent.
#   cond    comma-separated features that must ALSO be on the banner for the token to exist at all
#           (the literal sits under a nested cfg, OR its module is dead-stripped unless one of these
#           compiles a CALLER in — see seeding steps 5 and 6). Commas are AND. Two term forms beyond a bare
#           name, each added by BANNERCERT2 because a real row needed it:
#             `a+b`  OR-GROUP — either alternative makes the literal exist. The shape of a module
#                    with two independently gated callers (`ga10bprobe5a+witness` for `ga10b_fw`).
#             `!a`   NEGATED — the literal exists only when `a` is OFF, because `a` makes the code
#                    the literal lives in UNREACHABLE. `witness`'s token sits in `kernel_main`'s
#                    post-GUI tail, and `main.rs:79` says in its own `cfg_attr` that `baremetal`,
#                    `bootlog`, `usbdebug` and `tegra` each make that tail unreachable — so on a Pi
#                    `kernel8` image `witness` IS compiled and that literal provably is not.
#           When a term is unsatisfied the row prints UNVERIFIABLE naming it — loud, never silent,
#           never a pass. `-` for none.
#   state   measured | measured(N) | unmeasured-here — which artifact has actually proven this token.
#           `measured(N)` carries the HIT COUNT the proving artifact returned, which is the strongest
#           form: a later re-seed that drops the count to a different number is visible without a
#           rebuild of the old image. The 28 x86 rows are plain `measured` (their counts are in the
#           BANNERCERT arc report); every aarch64 row was measured by BANNERCERT2 and carries its N.
#           The summary counts anything starting `measured` as measured.

set -u

ART="${1:-}"
BANNER="${2:-}"

if [ -z "$ART" ] || [ $# -lt 2 ]; then
    echo "banner-cert: usage: banner-cert.sh <artifact> <banner-feature-list>" >&2
    exit 2
fi
if [ ! -f "$ART" ]; then
    echo "banner-cert: NO VERDICT — artifact does not exist: ${ART}" >&2
    exit 2
fi

# ---------------------------------------------------------------------------------------------
# THE TOKEN REGISTRY. `measured` rows were proven against an x86 media artifact; `unmeasured-here`
# rows were read out of the source and await an aarch64 artifact (see the arc report).
# ---------------------------------------------------------------------------------------------
bc_table() {
cat <<'TABLE'
witness|:: U1a: no application processors online — ring-3 demo SKIPPED ::|!baremetal,!bootlog,!usbdebug,!tegra|measured
wc|[wc-x] activate DECLINE reason=fb-not-ready latch=released|-|measured
wcg-paygo|[wc-g] paygo win=|witness|measured
wcdvalve|[wc-d] valve CLOSED util~|witness|measured
logts|:: LOGWIT-1 probe seq=|witness|measured
usbdebug|USB-DEBUG: ptr report (rel)|-|measured
ehcihid|:: EHCI-HID: DMAR: no ACPI DMAR table|-|measured
kbdwit|SILENCE-ENDED-HALTED|ehcihid|measured
smc|:: SMC-BATT: sweep failed (present=false)|-|measured
smcwalk|:: SMC-SCOUT: idx|smc|measured
sdhcblk|the staged file is FRAGMENTED; the permit describes exactly one LBA interval|-|measured
sdwrite|:: SDWRITE-POSTURE: posture=|witness|measured
bt|vendor-classed: RF/Bluetooth subclass+protocol|-|measured
btc|REACHED — a BR/EDR link was established|-|measured
smolnet|:: SOCK-3: no free address-space slot|-|measured
wifi|:: wifi: firmware NOT staged — no program-source block device; searched|-|measured
wifi2|:: wifi2: upload NOT ATTEMPTED uploaded-bytes=|-|measured
nvidia-kepler|:: kepler: FENCE ABORT dmactl REFUSED|-|measured
nvidia-kepler-takeover|:: kdisp: takeover-abort no-gop-info ::|nvidia-kepler|measured
nvidia-kepler-fifo|[NVIDIA] Starting PFIFO initialization|nvidia-kepler|measured
nvidia-kepler-ce|VOID — the control bracket moved across this base|-|measured
noaspm|[pcih] aspm cleared rp|-|measured
rtwit|[rtwit] <utf8-error>|-|measured
deadman|[deadman] <utf8-error>|-|measured
intel-ivb|:: igpu: VERDICT: Present but BAR not decoding|-|measured
gen7|:: gen7: r6 next=STOP-window-or-register-block-out-of-range|-|measured
gmux_igd|:: igpu: [GMUX] switched DISPLAY, EXTERNAL, and DDC to IGD|intel-ivb|measured
unaos_ivb|@boot iGPU trace 1 (pre-EBS) collected.|-|measured
tegra|:: tegra: JB5 — XUSB domain not ON at handoff|-|measured(1)
tegrasmp|:: AARCH64 SMP: ORIN-SMP-3 — DTB /cpus named no cores (dtb=@|-|measured(1)
apsrun|:: [apsrun] cpu |-|measured(2)
bsptick|:: [orinbsptick] arming PERIODIC CNTP at EL|tegra|measured(1)
bsprun|:: [orinbsprun] boot core |tegra|measured(1)
sdmmc|:: TEGRA-UNAFS: native unafs volume MOUNTED read-only on TegraSd|tegra|measured(1)
ga10bprobe5|[ga10bfw] window_need=|ga10bprobe5a+witness|measured(1)
ga10bprobe5a|[ga10bprobe5a] -> REFUSED reason=|tegra|measured(13)
baremetal|:: UnaOS bare-metal — Pi 4 microSD-slot boot, serial console (no framebuffer) ::|-|measured(1)
skip_xhci|:: xHCI bring-up SKIPPED (skip_xhci feature): video only, no USB ::|-|measured(1)
smp7|[smp7] cores online=|baremetal|measured(1)
v3d|:: V3D: CT1 did not idle within budget|-|measured(1)
vugpar|:: [spread2] window|baremetal|measured(1)
piusb|early/bringup_inner (P38 context, right before the first RC read)|-|measured(1)
genet|SKIP (no reply — pre-cable / no DHCP is the honest pre-metal state)|baremetal|measured(1)
nettest|:: NET20-GATE: mdns host-name publish battery|baremetal|measured(1)
pirast|:: PI-RAST: no mailbox framebuffer (headless boot) — cube demo skipped ::|-|measured(1)
desktop_firmware|[pidesk] activate DECLINE reason=no-panel|-|measured(1)
quarry|[quarry] DECLINE reason=dock-cannot-host-full-strip panel=|-|measured(1)
wedge2|-|no gated string literal anywhere under cfg(feature="wedge2") — the knob only re-times an existing path; certify it from its serial witness, not from the artifact|unmeasured-here
TABLE
}

# REGISTERED DIVERGENCES. A feature the banner names that the artifact provably does NOT carry,
# whose fix is outside this gate's reach and owned by someone else. They are NOT findings and NOT
# passes: each prints its whole reason, is counted on its own line of the summary, and MUST REACH
# ZERO. The shape is GATE-LEDGER's registered field-count exception, verbatim — the same tree
# already holds one known row open this way rather than deleting the check or the row. An
# UNREGISTERED MISSING still reds, which is the point; this list is short on purpose, and a row
# here is a debt with a name on it, not an exemption.
#
# IT IS EMPTY, AND IT HAS BEEN OCCUPIED ONCE. On this gate's first armed run (2026-09-15,
# `esp-x86`, flight-7 knob line) `sdwrite` was a MISSING: arroyo named it on the banner of every
# verb and `builder/src/main.rs`, which is what compiles the kernel the media boots, had no entry
# for it. It was registered here for exactly as long as it took to get the ruling — arroyo is
# right, `sdwrite` rides every image — and the builder gained
# `if std::env::var("UNAOS_NOSDWRITE").is_err() { feats.push("sdwrite"); }` beside its siblings in
# the same arc. The cert then read 28/28 with `SDWRITE-POSTURE` hits>0, and the row came out. That
# round trip is the whole intended lifetime of a row here: register, fix, delete.
#
# IT IS OCCUPIED AGAIN, BY THE FIRST ARMED AARCH64 RUN (2026-09-15, BANNERCERT2, `./arroyo esp-arm`
# with no knobs at all — the most default build this tree has). One row, `ehcihid`, and it is the
# same CLASS as sdwrite seen from the other arch: a feature that is DEFAULT-ON in arroyo, named on
# the banner of every aarch64 media build, and structurally incapable of putting one byte in an
# aarch64 artifact. The fix is one line and it is NOT in this gate's reach — see the row.
bc_registered() {
cat <<'REGISTERED'
ehcihid|arroyo names `ehcihid` on EVERY aarch64 banner and no aarch64 artifact can carry it. `drivers/mod.rs:9` gates the whole module `#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]`, so on aarch64 the feature emits nothing: measured on this arc's `esp-arm` ELF, the row token has 0 hits AND `LC_ALL=C grep -a -o -F 'EHCI-HID'` has 0 — not a rotted token, an absent feature. WHY IT REACHES THE BANNER: `arroyo:337` appends it default-on (`[ -z "${UNAOS_NOEHCIHID:-}" ]`), and `arm_features` (arroyo:2206) — which strips TWENTY x86-only names from the aarch64 cargo line so aarch64 media stay byte-identical — strips its own TWIN `kbdwit` at :2218 and does NOT strip `ehcihid`. THE FIX IS ONE LINE, `f="${f//,ehcihid,/,}"` beside the kbdwit strip, and it is deliberately NOT taken here because it is not free: removing a name from the cargo feature set shifts `-Cmetadata`, so it re-hashes EVERY aarch64 media image once — a one-time sha256 break on the Pi and Orin flight cards that the boards' seats must call, exactly as each of the other twenty strips was called when it landed. OWNER: the aarch64 seats (orin + pi), one line in `arm_features`; until then this row keeps the lie named, counted and un-green instead of red-lining every aarch64 media build for a defect no aarch64 executor introduced.
REGISTERED
}

# The OFF side. Each control is asserted ABSENT whenever the banner does NOT name that feature.
# Measured hits=0 against an armed x86 flight artifact that carried none of them.
bc_controls() {
cat <<'CONTROLS'
instgui|[wc-x] instgui install-go with NO committed target
selfhost|:: SELFHOST: src payload holds no regular files -> FAIL ::
holocron|:: [hcron] framing fixture leg=roundtrip -> FAIL
videobench|:: vperf: fbmem no framebuffer registered ::
irqstorage|irqstorage::submit called off a scheduled task
CONTROLS
}

bc_hits() {  # bc_hits <file> <token>
    LC_ALL=C grep -a -o -F -- "$2" "$1" 2>/dev/null | wc -l | tr -d ' '
}

bc_in_list() {  # bc_in_list <needle> <comma-list>
    case ",${2}," in *",${1},"*) return 0 ;; *) return 1 ;; esac
}

BOOTART="$(dirname "$ART")/EFI/BOOT/BOOTX64.EFI"

fail=0; noverdict=0; ok=0; unver=0; nowit=0; reg=0; nmeas=0; nunmeas=0

echo "⚡ banner-cert: artifact=${ART}"
echo "⚡ banner-cert: banner=${BANNER}"

# Registry self-check FIRST: a token under 9 bytes cannot be trusted to reach .rodata, so a table
# that carries one is a broken gate, not a green one.
while IFS='|' read -r f tok cond state; do
    [ -z "${f:-}" ] && continue
    [ "$tok" = "-" ] && continue
    probe="${tok#!}"; probe="${probe#@boot }"
    if [ "${#probe}" -lt 9 ]; then
        echo "❌ banner-cert: registry is broken — token for '${f}' is ${#probe} bytes (< 9); LLVM"
        echo "   immediate-encodes short literals out of .rodata, so this row can never certify."
        exit 2
    fi
    case "$state" in measured*) nmeas=$((nmeas+1)) ;; *) nunmeas=$((nunmeas+1)) ;; esac
done < <(bc_table)

for f in ${BANNER//,/ }; do
    row="$(bc_table | awk -F'|' -v f="$f" '$1==f{print; exit}')"
    if [ -z "$row" ]; then
        echo "feature=${f} witness=<UNREGISTERED> hits=- -> UNREGISTERED"
        echo "   ^ the banner names this feature and scripts/banner-cert.sh has no row for it, so"
        echo "     NOTHING about it was checked in the artifact. Add a row (see the seeding recipe"
        echo "     at the top of that file). This is a NO VERDICT, never a pass."
        noverdict=$((noverdict+1)); continue
    fi
    tok="$(printf '%s' "$row" | cut -d'|' -f2)"
    cond="$(printf '%s' "$row" | cut -d'|' -f3)"

    if [ "$tok" = "-" ]; then
        echo "feature=${f} witness=<none> hits=- -> NOWITNESS (${cond})"
        nowit=$((nowit+1)); continue
    fi

    skip=""
    if [ "$cond" != "-" ]; then
        for c in ${cond//,/ }; do
            case "$c" in
                '!'*)  # a NEGATED term: this feature must be OFF or the literal's code is unreachable
                    bc_in_list "${c#!}" "$BANNER" && { skip="NOT ${c#!}"; break; } ;;
                *+*)   # an OR-group: ANY one of the alternatives makes the literal exist
                    _bc_sat=""
                    for alt in ${c//+/ }; do bc_in_list "$alt" "$BANNER" && { _bc_sat=1; break; }; done
                    [ -n "$_bc_sat" ] || { skip="${c//+/ or }"; break; } ;;
                *)  bc_in_list "$c" "$BANNER" || { skip="$c"; break; } ;;
            esac
        done
    fi
    if [ -n "$skip" ]; then
        case "$skip" in
            NOT\ *) echo "feature=${f} witness=${tok} hits=- -> UNVERIFIABLE (its literal needs '${skip}', and this build carries '${skip#NOT }' — that configuration makes the code the literal lives in UNREACHABLE, so its absence proves nothing about the feature)" ;;
            *)      echo "feature=${f} witness=${tok} hits=- -> UNVERIFIABLE (its literal also needs '${skip}', which this build does not carry)" ;;
        esac
        unver=$((unver+1)); continue
    fi

    want_absent=""; target="$ART"
    case "$tok" in
        '!'*)      want_absent=1; tok="${tok#!}" ;;
    esac
    case "$tok" in
        '@boot '*) tok="${tok#@boot }"; target="$BOOTART"
                   if [ ! -f "$target" ]; then
                       echo "feature=${f} witness=${tok} hits=- -> NOARTIFACT (${target} is not on disk)"
                       noverdict=$((noverdict+1)); continue
                   fi ;;
    esac

    n="$(bc_hits "$target" "$tok")"
    bad=""
    if [ -n "$want_absent" ]; then
        disp="!${tok}"; [ "$n" -eq 0 ] || bad=1
    else
        disp="${tok}";  [ "$n" -gt 0 ] || bad=1
    fi
    if [ -z "$bad" ]; then
        echo "feature=${f} witness=${disp} hits=${n} -> OK"; ok=$((ok+1)); continue
    fi
    reason="$(bc_registered | awk -F'|' -v f="$f" '$1==f{sub($1 "\\|","");print; exit}')"
    if [ -n "$reason" ]; then
        echo "feature=${f} witness=${disp} hits=${n} -> MISSING (REGISTERED DIVERGENCE — not a finding, and it must reach zero)"
        echo "   ${reason}"
        reg=$((reg+1))
    else
        echo "feature=${f} witness=${disp} hits=${n} -> MISSING"; fail=$((fail+1))
    fi
done

# The OFF side.
while IFS='|' read -r f tok; do
    [ -z "${f:-}" ] && continue
    bc_in_list "$f" "$BANNER" && continue   # the banner claims it; it is not a control on this build
    n="$(bc_hits "$ART" "$tok")"
    if [ "$n" -eq 0 ]; then echo "control=${f} witness=${tok} hits=${n} -> OK"
    else echo "control=${f} witness=${tok} hits=${n} -> LEAK"; fail=$((fail+1)); fi
done < <(bc_controls)

echo "⚡ banner-cert: ok=${ok} missing/leak=${fail} registered-divergences=${reg} unverifiable=${unver} nowitness=${nowit} noverdict=${noverdict}  (table: ${nmeas} measured, ${nunmeas} unmeasured-here)"
[ "$reg" -gt 0 ] && echo "⚠ banner-cert: ${reg} REGISTERED DIVERGENCE(S) above — the banner lies about them TODAY, by a mechanism this gate cannot fix. They are NOT findings and each must reach zero."

if [ "$fail" -gt 0 ]; then
    echo "❌ banner-cert: the banner and the artifact DISAGREE — this media is red."
    echo "   A MISSING means the build printed a feature it did not compile in; a LEAK means it"
    echo "   compiled in a feature it never printed. Do not cut a card from it."
    exit 1
fi
if [ "$noverdict" -gt 0 ]; then
    echo "❌ banner-cert: NO VERDICT for ${noverdict} banner feature(s) — the gate did not run on them."
    exit 2
fi
if [ "$reg" -gt 0 ]; then
    echo "✅ banner-cert: every feature the banner named is in the artifact, EXCEPT the ${reg} registered divergence(s) named above."
else
    echo "✅ banner-cert: every feature the banner named is in the artifact."
fi
exit 0
