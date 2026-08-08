# WHITE BOARD — 2026-08-07 (GR21)

No open questions.

(Q1 — answered by Peter 2026-08-07: **Claude takes DISPLAY; the igpu lane continues its
round; the kepler lane stays on FENCE.** GEN7 is not adopted as a lane; the blob question
moves to the private-repo structure Peter proposed — seat's licensing read delivered in
session, short form: a never-distributed private repo carries no GPL obligations, blobs
load as DATA at runtime like linux-firmware, but nouveau GPL-2.0-only CODE stays out of
the GPL-3 tree regardless, and redistributing blobs or extracted ucode needs NVIDIA's
own grant the day images ship.)

(Q2 — answered: **write on.** The goal is a UnaOS-hosted build and a multi-user UI ASAP,
so SD write ships on once 4c is proven; the FR reserve-once shape remains the first step,
and the roadmap gains the real writable FS + multi-user targets behind it.)

(Q3 — answered by flight before it was asked: **Peter booted AI from the built-in SDXC
slot and it worked.** The capture proves it — `[sdhc] bdf 3:0.1 CARD IDENTIFIED —
124735488 blocks, block-addressed, csd v2` is the 59.5 GiB UNAOS-X86 card in the INTERNAL
slot, its FAT32 volume mounted RO with UNAOS.LOG visible, `gui=2253`, fastest boot on
record. Both one-volume-collapse gates opened in one flight: the firmware boots the
internal slot, and the card is far bigger than 29 MiB.)
