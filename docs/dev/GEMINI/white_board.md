# WHITE BOARD — 2026-08-07 (GR20)

## 1. WiFi: which goal? The rMBP on the network, or a BCM4331 driver? They are very different jobs.

**You asked for a WiFi driver. Before anyone writes a register, the honest sizing — because
the three paths differ by more than an order of magnitude and only you can pick the goal.**

Background, and a finding worth having on its own. The seat added a full read-only PCI census
(`b5896c5e`, knob `UNAOS_PCICENSUS=1`, merged) because **every PCI walk this OS had was
targeted** — xHCI, storage class, the iGPU — so the machine has never actually been
enumerated. That immediately exposed a misidentification we have been printing every boot:

> `Found network controller ... vendor 0x14e4 at 3:0.0 — no e1000 driver, skipping`

**That is not the WiFi.** `14e4:16bc` at `3:0.1` is the BCM57765/57785 SDXC reader — function 1
of a Broadcom **GbE + card-reader combo**. Function 0 is its Ethernet MAC (`14e4:16b4`), and
`find_device(0x02, 0x00)` matches the Ethernet subclass and returns on the first hit. A
BCM4331 reports subclass `0x80` and could never have matched that filter, so **the WiFi radio
has never been looked at.** (There is also no RJ45 jack behind that Ethernet MAC on a 15" rMBP.)
The next boot with the census on will print it; falsifiable prediction: `net-class=0x02:2`,
where `0x02:1` refutes the whole picture.

### The three paths

**A — BCM4331 native driver.** Expected `14e4:4331`, class `0x02`/`0x80`, alone behind a
`0:28.x` root port. It is a **SoftMAC** part, so there is no register poke that moves packets:
a BCMA backplane/EROM walk (Linux's `bcma` is ~5k lines), SPROM/OTP parsing for per-board PA
calibration (Apple boards commonly use OTP, the fiddlier path), HT-PHY + 2059 radio init
(thousands of lines of undocumented reverse-engineered tables — and b43's HT-PHY was its
weakest, experimental for most of its life), a **non-redistributable Broadcom microcode blob**
(OpenFWWF does not cover this part — that is a GPLv3 distribution decision for you), DMA rings
and SHM templates, a full 802.11 station layer, then WPA2-PSK (PBKDF2 + EAPOL 4-way + CCMP)
because no real network is open. **Realistic floor: 8,000–15,000 lines**, most of it magic
tables that cannot be validated incrementally — the PHY either comes up or it does not. This is
harder than the Kepler takeover.

**B — USB Ethernet dongle** (AX88179 or CDC-ECM). ~500–1,500 lines on the xHCI stack we already
run, no firmware blob, no PHY tables. The rMBP is on the network the same week.

**C — AR9271 USB WiFi.** Genuinely open firmware, actually wireless. Middle cost.

### What we already have, and why it does not help A

`drivers/e1000.rs` (~1,300 lines) and `smolnet`/smoltcp give us ARP, DHCP, IPv4, ICMP, UDP,
TCP, DNS and SNTP — **the entire stack from Layer 2 up is done and QEMU-proven.** But that
shortens path A by essentially nothing: 100% of A's work is *below* the line we have built and
0% is above it. It shortens B and C enormously — they plug straight into it.

**The seat's read: do A's reconnaissance regardless** (a read-only BCMA EROM walk and SPROM/OTP
read — cheap, falsifiable, the same spirit as the census, and it converts "maybe" into a fact),
then choose. If the goal is *the machine gets on the network*, B is the honest fastest path by
a wide margin. If it must be wireless and open, C. If the goal is genuinely *we own a BCM4331
driver* as an end in itself, A is a multi-arc project and worth naming as one rather than
discovering that halfway in.

**A one-word answer is enough — A, B, or C — and the recon runs either way.**
