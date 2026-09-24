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
# USAGE:  bash scripts/banner-cert.sh <artifact> <banner-feature-list> [<arch>]
#           <artifact>             the built kernel ELF (target/x86_64_esp/kernel.elf)
#           <banner-feature-list>  exactly the comma-separated list the `⚡ kernel features:` banner
#                                  printed for this build — NOT $KERNEL_FEATURES, NOT the knob line.
#                                  The banner is the claim under test; passing anything else makes
#                                  the gate vacuous.
#           <arch>                 x86_64 | aarch64. OPTIONAL and IGNORED for an ELF: the script
#                                  reads the arch out of the artifact's own ELF header (e_machine at
#                                  offset 0x12) so that no argument and no `cond` can lie about which
#                                  machine the bytes under test are for. It is read ONLY for a FLAT
#                                  image (`kernel8.img` has no ELF header), and the census line says
#                                  which of the two happened on every run. A flat image with no arch
#                                  argument leaves the arch `unknown`, and an arch-qualified cond
#                                  term is then UNSATISFIED — the row stays UNVERIFIABLE rather than
#                                  being guessed in either direction.
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
#      6b. AND THEN FOLLOW EACH NEGATED TERM INTO THE CODE THAT MAKES IT TRUE, BECAUSE A `cfg_attr`
#      IS A LINT DIRECTIVE AND NOT THE GATE (CERTCOND, 2026-09-15). `main.rs:79`'s `cfg_attr` is an
#      `allow(unreachable_code)` — it names four features that CAN make the tail unreachable, and it
#      is deliberately coarse, because a lint that is allowed too widely costs nothing. The gate is
#      each feature's own early-exit, and one of the four is narrower than the `cfg_attr` says:
#      `usbdebug`'s terminal loop is `#[cfg(all(feature = "usbdebug", not(all(target_arch = "x86_64",
#      feature = "wc"))))]` (`main.rs:~1155`), so on x86_64 + `wc` — the rMBP's flight configuration —
#      `usbdebug` is compiled and the tail is NOT deleted. Copied faithfully from a document that was
#      never a contract, the cond over-refused on exactly the build this tree flies: `witness` printed
#      UNVERIFIABLE while its token MEASURED 1 hit on that artifact, and the gate's coverage was one
#      row narrower than its table claimed for two arcs. The term is now
#      `!usbdebug@except:x86_64+wc`. THE RULE: a negated term is seeded from the `#[cfg]` on the code
#      that does the deleting, and a `cfg_attr`/comment is a pointer to that code, never the source.
#   7. SEED FROM THE FEATURE'S *UNCONDITIONAL* CALLER, NOT FROM ITS MOST INTERESTING FUNCTION — and
#      MEASURE THE ROW ON THE LEANEST BUILD THAT ARMS THE FEATURE, NOT ON THE RICHEST (SMALLFIX3,
#      2026-09-15). This is step 5's trap with one turn more on it, and it is the one that shipped:
#      the `wc` row was seeded on `[wc-x] activate DECLINE reason=fb-not-ready latch=released`, from
#      `video/desktop_uefi.rs::activate` — a function whose ONE caller is the Kepler takeover
#      (`main.rs:1141` says so in prose: "`desktop_uefi::activate` has exactly one caller"). `activate`
#      is `wc` code by module gate, so the row read as correct, and it MEASURED correct: the seeding
#      build was the rmbp flight-7 knob line, which arms `UNAOS_KEPLER_TAKEOVER=1`, so the caller was
#      compiled and the linker kept the function. Every leaner `wc` build — `UNAOS_WC=1 ./arroyo
#      esp-x86`, and `test-fat`, which is how the compositor is gated in QEMU where no Kepler exists —
#      drops the whole function and the row MISSINGs at 0 hits, reddening the verb before QEMU starts.
#      Measured on two x86 artifacts built the same day: with kepler, `wc-x] activate` = 6 hits; without
#      it, 0 — while `[wc-x]` = 26 in BOTH, so the feature was compiled and printing either way.
#      TWO RULES COME OUT OF IT. (a) A row's token belongs in the code path the feature's OWN knob
#      makes live with nothing else armed: `wc`'s is now `desktop_app_service`, whose call site
#      (`main.rs:5999`) is gated `#[cfg(feature = "wc")]` and nothing more. (b) A row measured only on a
#      rich knob line is measured on the configuration LEAST likely to expose this defect — so measure
#      the leanest arming build, or measure both and record that they agree.
#      AND IT WAS NOT A ONE-OFF: `smolnet` was the SAME DEFECT, found the same day by the same audit,
#      with `witness` in the caller's gate instead of a kepler knob. Its row was seeded on
#      `:: SOCK-3: no free address-space slot`, a literal in
#      `arch/x86_64/syscall.rs::sock3_launcher`. That function is gated
#      `all(feature = "smolnet", target_arch = "x86_64")` — correct — but its ONLY callers are
#      `main.rs:902` and `:909`, both `all(target_arch = "x86_64", feature = "witness", feature =
#      "smolnet")`. `smolnet` is DEFAULT-ON, so it rides every x86 media banner, and `witness` is OFF
#      for every media verb, so the linker drops the launcher from exactly the builds that ship:
#      `UNAOS_IVB=1 ./arroyo esp-x86` (no other knobs) exited 1 with `smolnet … hits=0 -> MISSING`.
#      Measured, with `smoltcp` = 137 hits on that same ELF as the control proving the feature WAS
#      compiled. Re-seeded on `:: SOCK-1: smoltcp icmp echo` from `smolnet::witness_tick`, whose
#      caller (`drivers/e1000.rs:1194`, inside `service_net`) carries that same
#      `all(feature = "smolnet", target_arch = "x86_64")` and nothing else, so it is reached on every
#      default boot's service pass. BOTH POLARITIES MEASURED: 1 hit on three x86 artifacts (a
#      witness-free `UNAOS_IVB=1` media build, the flight-7 line, and a no-kepler `UNAOS_WC=1` build);
#      0 hits on an `esp-arm` artifact, where `arm_features` strips `smolnet`, with `smoltcp` = 0
#      there as the corroborating control. Two rows, one shape, one day — which is why 7(a) and 7(b)
#      are rules and not an anecdote.
#   8. MEASURE it: build media with the feature armed and
#      `LC_ALL=C grep -a -o -F -- '<token>' <artifact> | wc -l` must be > 0. Mark the row `measured`.
#      A row seeded from source reading alone is marked `unmeasured-here` and stays that way until
#      an artifact for that arch proves it.
#   9. AND THEN KNOW WHAT THE GREEN ROW DID NOT BUY YOU (PHASE31WIT, rmbp B112, 2026-09-16). A CERT
#      ROW PROVES THE LITERAL IS PRESENT IN THE ARTIFACT; IT NEVER PROVES THE BOOT REACHED IT, AND IT
#      NEVER PROVES THE CAPTURE KEPT IT — the flight's own line is the reachability witness, the
#      replay is the retention witness, and a knob whose product is witness lines needs ALL THREE.
#      This row is where the three came apart. `bar1exp-uc` certified `measured` off the flown ELF,
#      and flight 9 proved the arm RAN (`mmio-map … uc=128 wc-kept=0` against flight 8's `uc=113
#      wc-kept=15`) — yet the literal had ZERO hits in the whole capture, and the arc opened by
#      hunting a cfg arm that was never wrong. The line was emitted and then discarded: VPERF-WC had
#      hoisted its emit site to `kernel_main`'s first statement (`BPACE: fb-wc t=0ms`), the bench
#      rMBP has no 16550 (`uart16550=absent carrier=ftdi-mirror`), and its only carrier is a
#      drop-oldest ring that does not replay until `ftdi:console-up` at 23437 ms. The control that
#      settled it needed no build at all: `:: video: WRITER seeded` is UNCONDITIONAL on every x86_64
#      build, sits three statements later, and was ALSO at zero hits — no cfg arm can explain that,
#      only an evicted ring. THE RULE: before reading a missing witness as a code defect, measure the
#      capture against the carrier — replay bytes against ring `CAP`, and whether the log begins
#      mid-token. `:: FTDI-CAP: replayed=… cap=… lost=… head_cut=…` now states exactly that on every
#      boot (`drivers/xhci/ftdi.rs::late_verdict`), so it costs one grep and not a flight.
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
#           compiles a CALLER in — see seeding steps 5 and 6). Commas are AND. Three term forms beyond a
#           bare name, each added because a real row needed it:
#             `a+b`  OR-GROUP — either alternative makes the literal exist. The shape of a module
#                    with two independently gated callers (`ga10bprobe5a+witness` for `ga10b_fw`).
#                    (BANNERCERT2.)
#             `!a`   NEGATED — the literal exists only when `a` is OFF, because `a` makes the code
#                    the literal lives in UNREACHABLE. `main.rs:79` says in its own `cfg_attr` that
#                    `baremetal`, `bootlog`, `usbdebug` and `tegra` each make `kernel_main`'s post-GUI
#                    tail unreachable — so on a Pi `kernel8` image `witness` IS compiled and the
#                    literal that lives in that tail provably is not. (BANNERCERT2.)
#             `!a@except:x+y`
#                    NEGATED WITH AN EXCEPTION — `a` makes the literal unreachable EXCEPT on a build
#                    that satisfies EVERY term after `@except:`, which may name an ARCH (`x86_64`,
#                    `aarch64` — matched against the artifact's own ELF header, never against an
#                    argument) or a FEATURE (matched against the banner). `+` is AND here, not OR:
#                    the spec is one configuration, copied from the source's `not(all(...))`.
#                    (CERTCOND, 2026-09-15, and the row that needed it is `witness`.)
#           When a term is unsatisfied the row prints UNVERIFIABLE naming it — loud, never silent,
#           never a pass. `-` for none. A cond this grammar cannot parse is exit 2 before any verdict
#           is printed, the same way a short token is: a gate that cannot read its own table is not a
#           green one.
#
#           WHY THE EXCEPTION FORM IS A CONJUNCTION AND NOT A BARE ARCH (CERTCOND). The obvious
#           spelling is `!usbdebug@aarch64`, read "usbdebug only kills this literal on aarch64". It is
#           the WRONG shape for the only row that needs it, and wrong in the reddening direction.
#           `main.rs:~1155` gates the usbdebug terminal loop — the thing that deletes the post-GUI
#           tail — on `all(feature = "usbdebug", not(all(target_arch = "x86_64", feature = "wc")))`.
#           So the exemption is an ARCH *and* A FEATURE together, and the loop still compiles on
#           x86_64 WITHOUT `wc` (the knob's original purpose: pre-GUI bring-up on a card with no
#           compositor). A bare-arch term would mark that build's `witness` row checkable, find the
#           literal correctly absent, and print MISSING — a false red on a real configuration, where
#           today's over-refusal is only a loud silence. LAWS §5: wrong-strict is worse than
#           wrong-lenient. The term copies the `not(all(...))` it comes from, so it cannot be wrong in
#           a way the source is not.
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
#
# ⚠ THIS IS A HEREDOC: every line inside it is parsed as a `feature|token|cond|state` row. A `#`
# comment placed between rows becomes a row whose token is empty, and the registry self-check then
# reds the whole gate with "token for '# ...' is 0 bytes". Comments go HERE, above the function.
# (DIRNS learned that the expensive way, 2026-09-15.)
#
# `irqstorage` (DIRNS, LEDGER SO20, 2026-09-15) was UNREGISTERED until this row, so ANY media build
# that armed it — `UNAOS_IRQSTORAGE=1 ./arroyo esp-x86`, and therefore `test-fat` — exited 2 with NO
# VERDICT and never reached QEMU. That is why the STOR-1 knob-on witnesses are OPTIONAL in
# x86-fat.spec and why x86's live-storage path had no media gate at all; DIRNS needed one, so it
# added the row rather than dropping the knob. The token is a literal in `drivers/xhci/irqstorage.rs`
# and its control is the strongest shape this tree has: the MODULE is gated
# `#[cfg(all(target_arch = "x86_64", feature = "irqstorage"))] pub mod irqstorage;`
# (drivers/xhci/mod.rs:25), so knob-off the file is never lexed and the string cannot exist.
# Measured on the DIRNS artifact: `LC_ALL=C grep -a -c -F` = 1 knob-on, and 0 for a known-absent
# control string on the same artifact.
#
# `uvc` (CAMERA1, rmbp-ledger B143, 2026-09-22) — the USB Video Class census. The row exists
# BEFORE the knob's first media build, on purpose: DIRNS's finding one paragraph up is that a knob
# with no row here exits 2 (NO VERDICT) on the first `esp-x86` that arms it, and stops the verb
# before QEMU. Token: `[uvc] commit=withheld`, 21 bytes, the withheld-commit witness in
# `drivers/uvc.rs::probe` — a function reached from `ehci::Controller::configure_hid` behind
# `#[cfg(feature = "uvc")]` AND NOTHING ELSE, which is what rule 7(a) above demands (the leanest
# arming build keeps it; there is no second knob in the call chain to drop the function). The
# module itself is gated `#[cfg(all(target_arch = "x86_64", feature = "uvc"))] pub mod uvc;`
# (drivers/mod.rs), so knob-off the file is never lexed and the string cannot exist. BOTH
# POLARITIES MEASURED on two x86_64 artifacts built the same hour from this tree
# (`cargo build --release --target x86_64-unaos.json --features …,wc,ehcihid,witness`, with and
# without `uvc`): `LC_ALL=C grep -a -o -F '[uvc] commit=withheld' | wc -l` = 1 knob-on, 0 knob-off,
# while the `ehcihid` row's own token measured 1 on BOTH as the positive control proving the
# knob-off artifact was a real EHCI-carrying build and not an empty one.
# ---------------------------------------------------------------------------------------------
bc_table() {
cat <<'TABLE'
witness|:: U1a: no application processors online — ring-3 demo SKIPPED ::|!baremetal,!bootlog,!usbdebug@except:x86_64+wc,!tegra|measured(1)
wc|[wc-x] desktop-app DECLINE reason=no-storage name=/|-|measured(1)
wcg-paygo|[wc-g] paygo win=|witness|measured
wcdvalve|[wc-d] valve CLOSED util~|witness|measured
logts|:: LOGWIT-1 probe seq=|witness|measured
usbdebug|USB-DEBUG: ptr report (rel)|-|measured
ehcihid|:: EHCI-HID: DMAR: no ACPI DMAR table|-|measured
kbdwit|SILENCE-ENDED-HALTED|ehcihid|measured
smc|:: SMC-BATT: sweep failed (present=false)|-|measured
smcwalk|:: SMC-SCOUT: idx|smc|measured
sdw|[sdhc-w] cmd24 lba=|-|measured
sdw-ro|sdw-ro=1 wp-pin=unread write-path=unread -> sdhc=ro reason=opt-out|-|measured
sdhcblk|the staged file is FRAGMENTED; the permit describes exactly one LBA interval|-|measured
sdwrite|:: SDWRITE-POSTURE: posture=|witness|measured
irqstorage|:: bx-blockreq: no block device|-|measured(1)
bt|vendor-classed: RF/Bluetooth subclass+protocol|-|measured
btc|REACHED — a BR/EDR link was established|-|measured
smolnet|:: SOCK-1: smoltcp icmp echo|-|measured(1)
wifi|:: wifi: firmware NOT staged — no program-source block device; searched|-|measured
wifi2|:: wifi2: upload NOT ATTEMPTED uploaded-bytes=|-|measured
wifi3|:: wifi2: ucode upload words=|-|measured
wifi4|:: wifi4: REFUSED reason=wifi3-upload-not-proven|-|measured(1)
nvidia-kepler|:: kepler: probe-abort bar0-unmapped|-|measured
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
ahci|:: AHCI: port=|-|measured
bar1wedge|:: BAR1WEDGE: rung=first-stall|-|measured
bar1exp-uc|:: x86 bar1exp: UC arm ARMED via=|-|measured
beam|:: BEAMX86: head=|nvidia-kepler,nvidia-kepler-takeover|measured
ftdirx|:: FTDIRX: first byte rx=|-|measured
gen7r8|:: gen7: r8 begin rung=R8 wake=|-|measured
nvidia-kepler-kfbind|:: KFBIND: pbdma[|-|measured
nvidia-kepler-kdhead|:: KDHEAD: end rung=KD14|-|measured
nvidia-kepler-ctrladdr|:: kepler: ctrladdr |-|measured
nvidia-kepler-ctrlbind|:: kepler: ctrlbind |-|measured
nvidia-kepler-vblank|:: kepler: vblank |-|measured
hda|[hda] census|-|measured
hda-tone|:: HDA-TONE:|-|measured
uvc|[uvc] commit=withheld|-|measured(1)
login|[login] screen open window=|-|measured
loginst|:: LOGIN: users+session|-|measured
ioapic|[ioapic] census ioapics=|-|measured(2)
tegra|:: tegra: JB5 — XUSB domain not ON at handoff|-|measured(1)
tegrasmp|:: AARCH64 SMP: ORIN-SMP-3 — DTB /cpus named no cores (dtb=@|-|measured(1)
apsrun|:: [apsrun] cpu |-|measured(2)
bsptick|:: [orinbsptick] arming PERIODIC CNTP at EL|tegra|measured(1)
bsprun|:: [orinbsprun] boot core |tegra|measured(1)
sdmmc|:: UNAFS: native unafs volume MOUNTED read-only on SdMmc|tegra|measured(1)
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
pirast|:: RAST: no mailbox framebuffer (headless boot) — cube demo skipped ::|-|measured(1)
desktop_firmware|[deskfw] activate DECLINE reason=no-panel|-|measured(1)
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
# THE FIRST OCCUPANT. On this gate's first armed run (2026-09-15,
# `esp-x86`, flight-7 knob line) `sdwrite` was a MISSING: arroyo named it on the banner of every
# verb and `builder/src/main.rs`, which is what compiles the kernel the media boots, had no entry
# for it. It was registered here for exactly as long as it took to get the ruling — arroyo is
# right, `sdwrite` rides every image — and the builder gained
# `if std::env::var("UNAOS_NOSDWRITE").is_err() { feats.push("sdwrite"); }` beside its siblings in
# the same arc. The cert then read 28/28 with `SDWRITE-POSTURE` hits>0, and the row came out. That
# round trip is the whole intended lifetime of a row here: register, fix, delete.
#
# IT HAS NOW BEEN OCCUPIED TWICE, AND IS EMPTY AGAIN. The second row was `ehcihid`, entered by this
# gate's first armed AARCH64 run (2026-09-15, BANNERCERT2, `./arroyo esp-arm` with no knobs at all —
# the most default build this tree has) and removed in the commit that fixed it, one arc later. It
# was the same CLASS as `sdwrite` seen from the other arch: DEFAULT-ON in arroyo (`:337`), named on
# the banner of every aarch64 media build, and structurally incapable of putting one byte in an
# aarch64 artifact (`drivers/mod.rs:9` gates the module on `target_arch = "x86_64"`). Measured: the
# row's token 0 hits AND `LC_ALL=C grep -a -o -F 'EHCI-HID'` 0. `arm_features` stripped its TWIN
# `kbdwit` and not the driver it instruments; it now strips both (`arroyo:2219`). The executor
# registered it rather than taking the line, because unlike its twenty siblings this strip re-hashes
# every aarch64 media image once — and got the ruling (Peter, 2026-09-15): the recorded card shas are
# history in MANIFEST files, not a contract; a lie in the banner is. Register, fix, delete — twice
# now, and the table is empty both times it mattered.
bc_registered() {
cat <<'REGISTERED'
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

# THE ARCH OF THE BYTES UNDER TEST, read out of the artifact itself. An ELF says what machine it is
# for in `e_machine`, two little-endian bytes at offset 0x12 (0x3E x86-64, 0xB7 aarch64), after the
# 4-byte `\x7fELF` magic and the `EI_DATA` endianness byte at offset 5. Reading it here rather than
# taking it from the command line is the whole point of the arch-qualified cond term: a `cond` can
# then be wrong about the SOURCE, which a human can check, but it can never be wrong about the
# ARTIFACT, which is the thing the gate exists to interrogate. A flat image (`kernel8.img`) has no
# header at all and answers `flat`; the caller may then name the arch, and the census says so.
bc_arch_of() {  # bc_arch_of <file> -> x86_64 | aarch64 | elf-machine-<n> | flat
    local magic ei_data b18 b19 m
    magic="$(LC_ALL=C od -An -tx1 -N 4 -- "$1" 2>/dev/null | tr -d ' \n')"
    [ "$magic" = "7f454c46" ] || { echo "flat"; return 0; }
    ei_data="$(LC_ALL=C od -An -tu1 -j 5  -N 1 -- "$1" | tr -d ' \n')"
    b18="$(    LC_ALL=C od -An -tu1 -j 18 -N 1 -- "$1" | tr -d ' \n')"
    b19="$(    LC_ALL=C od -An -tu1 -j 19 -N 1 -- "$1" | tr -d ' \n')"
    if [ "${ei_data:-1}" = "2" ]; then m=$(( b18 * 256 + b19 )); else m=$(( b19 * 256 + b18 )); fi
    case "$m" in
        62)  echo "x86_64" ;;
        183) echo "aarch64" ;;
        *)   echo "elf-machine-${m}" ;;
    esac
}

bc_name_ok() {  # a feature or arch name: the character set arroyo's knob map can actually produce
    case "$1" in ''|*[!A-Za-z0-9_.-]*) return 1 ;; *) return 0 ;; esac
}

# THE COND PARSER'S REFUSAL. Prints the reason and returns 0 when <cond> is not in the grammar. A
# cond the gate cannot parse is not a lenient cond and not a strict one — it is a row whose meaning
# nobody knows, so it exits 2 before any verdict, exactly as a short token does.
bc_cond_bad() {  # bc_cond_bad <cond>
    local cond="$1" c body spec t
    [ "$cond" = "-" ] && return 1
    [ -z "$cond" ] && { echo "the cond field is EMPTY (write '-' for no condition)"; return 0; }
    case "$cond" in *,,*|,*|*,) echo "empty term (a stray comma)"; return 0 ;; esac
    for c in ${cond//,/ }; do
        case "$c" in
            '!'*)
                body="${c#!}"
                case "$body" in
                    *@*)
                        spec="${body#*@}"; body="${body%%@*}"
                        case "$spec" in
                            except:*) spec="${spec#except:}" ;;
                            *) echo "term '${c}': the only qualifier after '@' is 'except:'"; return 0 ;;
                        esac
                        [ -n "$spec" ] || { echo "term '${c}': '@except:' with an empty specification"; return 0; }
                        case "$spec" in *++*|+*|*+) echo "term '${c}': empty alternative inside '@except:'"; return 0 ;; esac
                        for t in ${spec//+/ }; do
                            bc_name_ok "$t" || { echo "term '${c}': '${t}' is not a feature or arch name"; return 0; }
                        done ;;
                esac
                [ -n "$body" ] || { echo "term '${c}': '!' with no feature name"; return 0; }
                bc_name_ok "$body" || { echo "term '${c}': '${body}' is not a feature name"; return 0; } ;;
            *@*) echo "term '${c}': '@except:' qualifies a NEGATED term only — write '!<feature>@except:<spec>'"; return 0 ;;
            *+*)
                case "$c" in *++*|+*|*+) echo "term '${c}': empty alternative in an OR-group"; return 0 ;; esac
                for t in ${c//+/ }; do
                    bc_name_ok "$t" || { echo "term '${c}': '${t}' is not a feature name"; return 0; }
                done ;;
            *) bc_name_ok "$c" || { echo "term '${c}': not a feature name"; return 0; } ;;
        esac
    done
    return 1
}

BOOTART="$(dirname "$ART")/EFI/BOOT/BOOTX64.EFI"

ARCHARG="${3:-}"
BC_ARCH="$(bc_arch_of "$ART")"
if [ "$BC_ARCH" = "flat" ]; then
    if [ -n "$ARCHARG" ]; then
        case "$ARCHARG" in
            x86_64|aarch64)
                BC_ARCH="$ARCHARG"
                BC_ARCH_SRC="FLAT image (no ELF header); the caller named this arch in argument 3" ;;
            *)
                echo "banner-cert: NO VERDICT — argument 3 must be x86_64 or aarch64, got '${ARCHARG}'" >&2
                exit 2 ;;
        esac
    else
        BC_ARCH="unknown"
        BC_ARCH_SRC="FLAT image (no ELF header) and the caller named no arch, so every arch-qualified cond term is UNSATISFIED here and its row stays UNVERIFIABLE rather than guessed"
    fi
else
    BC_ARCH_SRC="read from the artifact's OWN ELF header (e_machine at 0x12), never from an argument"
    if [ -n "$ARCHARG" ] && [ "$ARCHARG" != "$BC_ARCH" ]; then
        echo "⚠ banner-cert: argument 3 says '${ARCHARG}' and the ELF header says '${BC_ARCH}' — the HEADER wins."
    fi
fi

fail=0; noverdict=0; ok=0; unver=0; nowit=0; reg=0; nmeas=0; nunmeas=0

echo "⚡ banner-cert: artifact=${ART}"
echo "⚡ banner-cert: banner=${BANNER}"
echo "⚡ banner-cert: arch=${BC_ARCH}  (${BC_ARCH_SRC})"

# Registry self-check FIRST: a token under 9 bytes cannot be trusted to reach .rodata, and a cond
# this grammar cannot parse is a row whose meaning nobody knows. A table that carries either is a
# broken gate, not a green one — so both are exit 2 before one verdict is printed.
while IFS='|' read -r f tok cond state; do
    [ -z "${f:-}" ] && continue
    # A `-` token is a NOWITNESS row and its `cond` field carries the REASON in prose by design
    # (see ROW FORMAT above), so it is never parsed as a condition.
    [ "$tok" = "-" ] && continue
    if why="$(bc_cond_bad "${cond:-}")"; then
        echo "❌ banner-cert: NO VERDICT — the cond for '${f}' is not in the grammar: ${why}"
        echo "   cond='${cond:-}'. The forms are: a bare feature; 'a+b' (OR-group); '!a' (negated);"
        echo "   '!a@except:<arch-or-feature>[+…]' (negated with an exception). Commas are AND."
        exit 2
    fi
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
                    _bc_neg="${c#!}"; _bc_exc=""
                    case "$_bc_neg" in
                        *@except:*) _bc_exc="${_bc_neg#*@except:}"; _bc_neg="${_bc_neg%%@except:*}" ;;
                    esac
                    bc_in_list "$_bc_neg" "$BANNER" || continue   # the killer is OFF: nothing to say
                    if [ -z "$_bc_exc" ]; then skip="NOT ${_bc_neg}"; break; fi
                    # An EXCEPTION: every term of the spec must hold for the code to survive the
                    # killer. Arch terms are answered by the artifact's header, feature terms by the
                    # banner. The FIRST term that fails is the one named, because it is the reason.
                    _bc_miss=""
                    for _bc_t in ${_bc_exc//+/ }; do
                        case "$_bc_t" in
                            x86_64|aarch64)
                                [ "$BC_ARCH" = "$_bc_t" ] || { _bc_miss="the artifact's arch is ${BC_ARCH}, not ${_bc_t}"; break; } ;;
                            *)
                                bc_in_list "$_bc_t" "$BANNER" || { _bc_miss="this build does not carry '${_bc_t}'"; break; } ;;
                        esac
                    done
                    [ -n "$_bc_miss" ] && { skip="EXC ${_bc_neg}|${_bc_exc}|${_bc_miss}"; break; }
                    ;;
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
            EXC\ *) _bc_s="${skip#EXC }"
                    _bc_n="${_bc_s%%|*}"; _bc_r="${_bc_s#*|}"; _bc_e="${_bc_r%%|*}"; _bc_w="${_bc_r#*|}"
                    echo "feature=${f} witness=${tok} hits=- -> UNVERIFIABLE (its literal needs 'NOT ${_bc_n}' EXCEPT on '${_bc_e}'; this build carries '${_bc_n}' and the exception does NOT hold here — ${_bc_w} — so that configuration makes the code the literal lives in UNREACHABLE, and its absence proves nothing about the feature)" ;;
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
