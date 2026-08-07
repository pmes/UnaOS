# WHITE BOARD — 2026-08-07 (GR20)

No open questions.

(Question 1 — iGPU Flight 1 scope — answered: **A, grow the arc.** Full IVB display bring-up
joins Flight 1; the serial link is the debug path.)

(Question 2 — the internal SDXC slot — answered by inspection, and the seat's framing was
wrong: the card is a v1.x SDSC and the defect was ours. Fixed, metal-proven Boot AC, and the
first SD write flew clean on Boot AD.)

(Question 3 — the WiFi goal — answered by Peter 2026-08-07: **A, own the BCM4331 driver.**
The native SoftMAC path, sized honestly at 8,000–15,000 lines: BCMA backplane/EROM walk,
SPROM/OTP calibration, HT-PHY + 2059 radio init, a non-redistributable Broadcom microcode
blob, DMA rings, an 802.11 station layer, then WPA2-PSK. Multi-arc project. First arc is
reconnaissance — a read-only BCMA EROM walk and SPROM/OTP read — which converts the plan's
assumptions into facts before a single control register is written. Boot AE's PCI census
identifies the radio; the seat carries this into the arc plan.)
