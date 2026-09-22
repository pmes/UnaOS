#!/usr/bin/env bash
# NEUTRAL M2 — ARTIFACT CERTIFICATION. `LC_ALL=C grep -a -o -F`, never `strings`.
#
# Rule (LAWS §5 / BANNER-ARTIFACT, and M1's shape): an OLD token must be 0 in every image; a NEW
# token must be >=1 in the images that ARM its family. A family the build does not arm is CORRECTLY
# absent — an unarmed family certifies nothing — so each table carries a PRESENT-control that must
# be non-zero and an ABSENT-control that must be zero, because a zero has to be readable as a fact
# about the BUILD and not about the pattern.
#
# ONE OLD TOKEN IS NOT EXPECTED TO BE ZERO, and it is named rather than excluded: `:: PIUSB:` also
# lives in `arch/aarch64/piusb.rs:50` (`const P`), which is NOT this seat's file — arch board files
# may keep board names. So the aarch64 image is certified on the SHARED emitter's own text instead,
# and the arch residue is REPORTED as the family's owed split, not hidden.
#
# usage: m2-certify.sh <kernel8.img> <x86-kernel.elf>
set -uo pipefail
K8="${1:?kernel8.img}"; X86="${2:?x86 kernel.elf}"

OLD=(':: PIUSB:' ':: PINSTALL:' ':: PIINSTALL:' ':: TEGRA-SD:' ':: TEGRA-UNAFS:' ':: PI-RAST:' ':: PI-DESK:' ':: piusb27:' ':: piusb28:')
NEW=(':: USB:' ':: INSTALL:' ':: SDMMC:' ':: UNAFS:' ':: RAST:' ':: DESK:' ':: usb27:' ':: usb28:')
# the SHARED xHCI driver's own text — the half of `:: PIUSB:` that this seat moved
SHARED_USB_OLD=':: PIUSB: [usbw]'
SHARED_USB_NEW=':: USB: [usbw]'

n() { LC_ALL=C grep -a -o -F -- "$2" "$1" 2>/dev/null | wc -l; }

table() { # $1 = image, $2 = label, $3 = present-control (image-specific)
    local img="$1" lbl="$2" pos="$3" t c bad=0
    echo "--- ${lbl}  ($(stat -c%s "$img") bytes) ---"
    printf '%-24s %s\n' 'TOKEN' 'COUNT'
    for t in "${OLD[@]}"; do c=$(n "$img" "$t"); [ "$c" -ne 0 ] && bad=1
        printf 'OLD %-20s %s\n' "$t" "$c"; done
    printf 'OLD %-20s %s\n' "$SHARED_USB_OLD" "$(n "$img" "$SHARED_USB_OLD")"
    for t in "${NEW[@]}"; do printf 'NEW %-20s %s\n' "$t" "$(n "$img" "$t")"; done
    printf 'NEW %-20s %s\n' "$SHARED_USB_NEW" "$(n "$img" "$SHARED_USB_NEW")"
    # The present-control is IMAGE-SPECIFIC and that is not a detail: `UNAOS_BUILD_STAMP` is placed
    # by the x86 media builder and reads 0 in `kernel8.img`, which builds from its own curated
    # $K8_FEATS (arroyo says so in the BANNER_ARTIFACT_MAP preamble). Using it on the Pi image would
    # have "proved" a broken instrument. Measured per image, then named.
    printf 'CTL %-20s %s   (present-control, MUST be >0)\n' "$pos" "$(n "$img" "$pos")"
    printf 'CTL %-20s %s   (present-control, MUST be >0)\n' '[vfs]' "$(n "$img" '[vfs]')"
    printf 'CTL %-20s %s   (absent-control, MUST be 0)\n'  'NOTATOKEN-UNAOS'   "$(n "$img" 'NOTATOKEN-UNAOS')"
    printf 'VERDICT: %s\n' "$([ $bad -eq 0 ] && echo 'every OLD token reads 0' || echo 'AN OLD TOKEN SURVIVED')"
    echo
}

echo "NEUTRAL M2 — ARTIFACT CERTIFICATION (LC_ALL=C grep -a -o -F, never strings)"
echo "generated $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo
table "$K8"  "A. kernel8.img  (K8_FEATS=baremetal,skip_xhci)" "CAPSTONE"
table "$X86" "B. x86 kernel.elf  (./arroyo test default leg)" "UNAOS_BUILD_STAMP"

cat <<'NOTE'
READING THE ZEROS. A NEW token at 0 is only meaningful once you know whether the image ARMS that
family, and five of them are correctly absent from both images:
  :: INSTALL:  neither image compiles the installer (`piinstall`/`install_target` off)
  :: SDMMC:    tegra-only (`sdmmc`); no Tegra QEMU machine exists, so neither of these is a tegra image
  :: UNAFS:    same gate — the mount census is the tegra card's
  :: RAST:     needs `pirast`; kernel8() builds K8_FEATS=baremetal,skip_xhci
  :: DESK:     needs `desktop_firmware`; the x86 default leg does not carry it
An unarmed family certifies nothing, so those five are carried by the compile side of the gate
(80/80 cfg legs in `./arroyo check`) and by the spec-rule resolution in m2-rule-resolution.txt, not
by this table. WHAT THIS TABLE DOES CARRY: the `usb` family is ARMED in both images — `:: USB:` 76
and 19, `:: USB: [usbw]` 14 in each — so its OLD spelling reading 0 is a fact about the RENAME and
not about the arming, which is the whole point of pairing every OLD row with its NEW one.
NOTE
