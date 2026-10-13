# WIFI9 (B625) — every b43/bcma hardware fact, cited, to its line in our code

R110 (RULINGS.md:141) and R95 §2: the Linux b43 driver (GPL-2.0, `drivers/net/wireless/broadcom/b43/`, torvalds/linux master fetched 2026-10-13 into the executor's scratch, never into the tree) read for HARDWARE FACTS only — register numbers, bits, table values, sequences and their names. Three rows also cite Linux bcma (`include/linux/bcma/bcma.h`, `include/linux/bcma/bcma_regs.h`, `drivers/bcma/core.c`) for the PCI-host DMA translation and IOST.DMA64; that is the R95 §2 precedent one directory over from b43 and is flagged in the B625 row for the seat. No text copied, no function transcribed: the accessor layer, the per-element table cursors, the bounded waits, the audit counters and the receive-only cuts are this tree's own.

Our code paths are repository-relative; `wifi/` = `unaos/crates/kernel/src/wifi/`.

| # | fact | source (file:line) | our code (file:line) |
|---|---|---|---|
| 1 | radio address/data ports (rev>=24) 0x3D8/0x3DA | `b43.h:93-94` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:46-47` |
| 2 | PHY address/data ports 0x3FC/0x3FE | `b43.h:104-105` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:49-50` |
| 3 | PSM_PHY_HDR 0x492, MAC_PHY_FORCE_CLK 0x4 | `b43.h:109, :487` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:52-53` |
| 4 | TSF_CLK_FRAC_LOW/HIGH 0x62E/0x630 | `b43.h:150-151` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:55-56` |
| 5 | IOCTL MACPHYCLKEN 0x10 | `b43.h:498` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:58` |
| 6 | posting read every 24 writes (PHY_VER after PHY writes, MACCTL after radio writes) | `phy_common.h:37, phy_common.c:225-229, :281-284` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:60, :141-148` |
| 7 | PHY routing OFDM 0x400 / EXTG 0x800 / N_BMODE 0xC00 | `phy_common.h:11-26` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:63-65` |
| 8 | HT BBCFG 0x001, RSTCCA 0x4000, RSTRX 0x8000 | `phy_ht.h:8-10` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:68-70` |
| 9 | HT BANDCTL 0x009, 5GHZ bit 0x1 | `phy_ht.h:11-12` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:71-72` |
| 10 | HT TABLE_ADDR/DATALO/DATAHI 0x072/0x073/0x074 | `phy_ht.h:13-15` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:73-75` |
| 11 | HT CLASS_CTL 0x0B0, CCK 0x1, OFDM 0x2, WAITED 0x4 | `phy_ht.h:16-19` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:76-79` |
| 12 | HT BW1..BW6 0x1CE..0x1D3 | `phy_ht.h:33-38` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:80` |
| 13 | HT TXPCTL_CMD_C1 0x1E7 and its enable bits 0x2000/0x4000/0x8000 | `phy_ht.h:39-43, phy_ht.c:536-538` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:81-82` |
| 14 | RF_SEQ_MODE EXTG(0), RF_SEQ_TRIG EXTG(3), RST2RX 0x20, RF_SEQ_STATUS EXTG(4) | `phy_ht.h:72-82` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:83-86` |
| 15 | RF_CTL_CMD 0x810, FORCE 0x1, CHIP0_PU 0x2 | `phy_ht.h:85-87` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:87-89` |
| 16 | RF_CTL_INT_C1..C3 EXTG(0x4c/0x6c/0x8c) | `phy_ht.h:89-91` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:90` |
| 17 | AFE_Cn_OVER / AFE_Cn EXTG(0x110..0x119) | `phy_ht.h:93-98` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:92` |
| 18 | Cn_CLIP1THRES OFDM(0x0E/0x4E/0x8E) | `phy_ht.h:68-70` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:93` |
| 19 | B-PHY BBCFG N_BMODE(0x001), RSTCCA|RSTRX; HT_TEST N_BMODE(0x00A) | `phy_ht.h:110-113` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:94-96` |
| 20 | table address = id << 10 | offset; width bits never written | `tables_phy_ht.h:6-12, tables_phy_ht.c:610-611` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:98-104, :217-223` |
| 21 | table element write: ADDR, DATAHI (32-bit), DATALO | `tables_phy_ht.c:672-697` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:217-223` |
| 22 | table element read: ADDR, DATAHI then DATALO (32-bit), DATALO (8/16) | `tables_phy_ht.c:606-635` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:225-234` |
| 23 | write_few: consecutive offsets from one cursor | `tables_phy_ht.c:702-745` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:236-241` |
| 24 | the 24 init regions, their order, ids, offsets, widths | `tables_phy_ht.c:794-823` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:467-469 (data: wifi/htphy_tables.rs:625-650)` |
| 25 | HT-PHY maskset = select, DATA <- (DATA & mask) | set | `phy_ht.c:1085-1091` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:164-170` |
| 26 | phy_mask / phy_set are maskset(m,0) / maskset(0xFFFF,s) | `phy_common.c:299-319` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:171-176` |
| 27 | HT radio read sets 0x200 in the register | `phy_ht.c:1093-1100` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:178-183` |
| 28 | HT radio write: select, data | `phy_ht.c:1102-1107` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:185-191` |
| 29 | radio mask/set/maskset are read-modify-write | `phy_common.c:233-249` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:192-201` |
| 30 | radio wait: poll (reg & mask) == value every delay up to timeout | `phy_common.c:251-264` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:203-213` |
| 31 | force clock: IOCTL.FGC on/off (BCMA arm) | `phy_common.c:563-580` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:250-255` |
| 32 | MAC PHY clock on: IOCTL.MACPHYCLKEN (BCMA arm) | `main.c:3020-3033` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:257-261` |
| 33 | switch_analog(on): AFE 0x00cd, AFE_OVER 0 per core | `phy_ht.c:1038-1046` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:274-279` |
| 34 | classifier: keep CCK|OFDM|WAITED, replace masked bits | `phy_ht.c:220-234` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:282-288` |
| 35 | force RF sequence: save mode, set 0x3, trigger, poll status 200 x 1 ms, restore | `phy_ht.c:175-194` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:291-308` |
| 36 | reset CCA: forced clock, BBCFG.RSTCCA pulse, RST2RX | `phy_ht.c:236-248` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:311-319` |
| 37 | TX power fix: read 26@0xE8 (32-bit), EXTG(0x108) masked with 0, 7@0x110+i, 13@0x63+4i / 0x73+4i (8-bit) | `phy_ht.c:511-531` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:322-330` |
| 38 | 2059 init pre: RF_CTL_CMD CHIP0_PU off, FORCE on, FORCE off, CHIP0_PU on | `phy_ht.c:134-140` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:337-341` |
| 39 | 2059 rev-1 init table written with ALL routing | `radio_2059.c:323-335 (values radio_2059.c:16-20 in wifi/radio2059.rs:43-47)` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:342-347` |
| 40 | per-core 0x146 |= 0x3 | `phy_ht.c:152-153` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:348-351` |
| 41 | post-init: CAL_RESETN |= 0x78, XTAL_CONFIG2 |= 0x80, 2 ms, both cleared | `phy_ht.c:157-161` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:352-357` |
| 42 | rcal on core 3: RCAL_CONFIG bit0, 0x0BF bit0, 0x19B maskset(0x3,0x2), start bit1, 100 us, stop, wait RCAL_STATUS bit0 (100 us / 1 s), disable, 0xa |= 0x60 | `phy_ht.c:80-104` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:358-368` |
| 43 | rccal: three (MASTER, TRC0) pairs (0x61,0xE9) (0x69,0xD5) (0x73,0x99), X1=0x6E, start 0x55, wait DONE_OSCCAP bit1 (500 us / 5 s), stop 0x15; MASTER bit0 off | `phy_ht.c:107-132` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:369-381` |
| 44 | RFPLL_MASTER bit3 off | `phy_ht.c:168` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:382-383` |
| 45 | 2059 register names (RCAL_CONFIG 0x004 … RCCAL_MASTER 0x17f), C1/C2/C3/ALL routing | `radio_2059.h:9-24` | `wifi/radio2059.rs:22-37 (WIFI8), used unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:36-41` |
| 46 | tune, radio half: 13 synth registers, 8 per-core RX/TX registers x 3 cores, 50 us, PLL cal toggle, 300 us | `phy_ht.c:35-77` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:405-419` |
| 47 | channel row lookup by centre frequency | `radio_2059.c:338-351` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:625-630 (wifi/radio2059.rs:312-315)` |
| 48 | tune, PHY half (2.4 GHz): BANDCTL 5GHZ off, B-PHY out of reset under PSM force-clock, BW1..BW6 | `phy_ht.c:763-787, :311-329` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:420-430` |
| 49 | not ch 14: classifier OFDM on; 2.4 GHz: HT_TEST &= ~0x840 | `phy_ht.c:789-797` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:431-433` |
| 50 | TX power fix in the tune | `phy_ht.c:799-800` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:434` |
| 51 | spur avoid: ch 11 → spuravoid 0; MAC freq fraction 0x6666/0x6 on BCM4331; BBCFG.RSTRX off; reset CCA | `phy_ht.c:733-761, main.c:3049-3066` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:435-440` |
| 52 | 0x017e <- 0x3830 | `phy_ht.c:804` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:441` |
| 53 | 8 ms for the radio to tune | `phy_common.c:440` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:442` |
| 54 | SHM channel cookie (ch number, before the PHY op) | `phy_common.c:425-432, b43.h:255` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:195-200, unaos/crates/kernel/src/wifi/bringup.rs wifi9()` |
| 55 | default 2.4 GHz channel 11 | `phy_ht.c:1074-1077` | `unaos/crates/kernel/src/wifi/bringup.rs wifi9() CH` |
| 56 | 2.4 GHz channel range 1..14 | `phy_ht.c:1064-1069` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:624-627` |
| 57 | op_init: the tables first | `phy_ht.c:881` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:467-469` |
| 58 | op_init register sequence :883-:915 (0x0be, 0x23f-0x241, zero_extg, EXTG(0), AFE_OVER, EXTG(0x103/0x101), 0x20d, EXTG(0x14f), 0x70, 0x1ff, CCK classifier, 0xb1, 0x32f, 0x077, 0x0b4, 0x17e, 0x0b9) | `phy_ht.c:883-915, :250-262` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:471-500` |
| 59 | op_init table writes 7@0x14e/0x15e/0x16e | `phy_ht.c:917-919` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:501-503` |
| 60 | AFE unk1 per core (AFE/OVER sets and masks, 8@5+0x10i = 0) | `phy_ht.c:265-284` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:504-512` |
| 61 | op_init table writes 7@0x130 (9), 7@0x120/0x124, 8@0x00/0x10/0x20, 8@0x08/0x18/0x28, 8@0x0c/0x1c/0x2c | `phy_ht.c:923-942` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:513-524` |
| 62 | maskset 0x280, 0x283, OFDM(0x141), 0x283 | `phy_ht.c:944-947` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:525-528` |
| 63 | tables 0, 1, 40 @0x8 (9, 0xe, 0x13, 0x18) | `phy_ht.c:949-955` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:529-531` |
| 64 | maskset OFDM(0x24/0x64/0xa4) (0x3f, 0xd); EXTG(0x60/0x64/0x80/0x84) |= 1 | `phy_ht.c:957-964` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:532-537` |
| 65 | copy 7@0x144→0x14a, 0x154→0x15a, 0x164→0x16a | `phy_ht.c:966-972` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:538-541` |
| 66 | reset CCA under forced clock; MAC PHY clock on | `phy_ht.c:974-981` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:542-548` |
| 67 | PA override (save RF_CTL_INT, write 0x0400, restore); RST2RX forced (RX2TX not: receive-only) | `phy_ht.c:196-214, :983-986` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:549-561` |
| 68 | classifier(0,0); clip-detection read | `phy_ht.c:989-990, :286-291` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:562-563` |
| 69 | B-PHY init: 16 x N_BMODE(0x88+i) from 0x1E1F step -0x202, 16 x N_BMODE(0x98+i) from 0x3E3F, N_BMODE(0x38) = 0x668 | `phy_ht.c:293-309, :992-993` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:564-575` |
| 70 | late 0x1a@0xc0 contents (128 x 32-bit) | `phy_ht.c:995-996, tables_phy_ht.h:24` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:576 (data: wifi/htphy_tables.rs:589, :654)` |
| 71 | TX power control off | `phy_ht.c:533-554, :1000` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:578-580` |
| 72 | NOT run (receive-only): idle TSSI (TX tone), TX power control setup, TSSI setup, TX power control on | `phy_ht.c:1001-1004, :415-421, :573-727` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:582-585 (named on the wire)` |
| 73 | HT-PHY gated on rev <= 1 | `main.c:4517-4520` | `unaos/crates/kernel/src/wifi/bringup.rs wifi9() gate` |
| 74 | phy_init order: analog on, radio on (2059 init + channel), ops->init, channel | `phy_common.c:83-113, phy_ht.c:1019-1035` | `unaos/crates/kernel/src/wifi/bringup/htphyinit.rs:631-640` |
| 75 | MAC suspended around PHY/radio work | `phy_common.c:451-459` | `unaos/crates/kernel/src/wifi/bringup.rs wifi9() → unaos/crates/kernel/src/wifi/bringup/rx.rs:169-190` |
| 76 | MACCTL bits ENABLED, INFRA, AP, BEACPROMISC, KEEP_BADPLCP, KEEP_CTL, KEEP_BAD, PROMISC, HWPS, AWAKE, DISCPMQ | `b43.h:451-474` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:37-47` |
| 77 | GEN_IRQ_REASON MAC_SUSPENDED 0x1 | `b43.h:532` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:49` |
| 78 | SHM shared UCODESTAT 0x40 (SLEEP 4), PHYVER 0x50, PHYTYPE 0x52, PRMAXTIME 0x74, CHAN 0xA0 | `b43.h:323, :328, :244, :245, :311, :255` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:52-57` |
| 79 | PHYTYPE_HT = 7 | `b43.h:425` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:59` |
| 80 | SHM control word (routing << 16 | word offset), unaligned via DATA_UNALIGNED | `main.c:479-489, :555-569` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:129-143` |
| 81 | power-saving bits: AWAKE on, HWPS off, wait UCODESTAT != SLEEP (100 x 10 us) | `main.c:1145-1200` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:152-165` |
| 82 | MAC suspend: ENABLED off, poll MAC_SUSPENDED 35 x 10 us then 40 x 1 ms | `main.c:2987-3017` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:169-190` |
| 83 | MAC enable: ENABLED on, ack MAC_SUSPENDED, flush reads, awake | `main.c:2958-2983` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:397-404` |
| 84 | opmode: AP/KEEP_*/PROMISC/BEACPROMISC cleared, INFRA set, filter flags, DISCPMQ | `main.c:3103-3160` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:371-377` |
| 85 | probe responses disabled (PRMAXTIME = 1) | `main.c:4914-4918` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:379` |
| 86 | SHM PHY type and revision | `main.c:4931-4933` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:380-382` |
| 87 | FW598 header format for ucode rev >= 598 | `main.c:2699-2700` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:84-85` |
| 88 | DMA64 controller 0 base 0x200 | `b43.h:69` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:62` |
| 89 | DMA64 RX registers RXCTL +0x20, RXINDEX +0x24, RINGLO/HI +0x28/+0x2C, RXSTATUS +0x30, RXERROR +0x34 | `dma.h:113-140` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:63-68` |
| 90 | RXCTL bits ENABLE, FROFF<<1, PARITYDISABLE, ADDREXT | `dma.h:114-120` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:69-73` |
| 91 | RXSTATUS DPTR 0x1FFF, STATE 0xF0000000 (DISABLED 0) | `dma.h:125-127` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:74-75` |
| 92 | 64-bit descriptor: ctl0, ctl1, addr_lo, addr_hi; DTABLEEND, BYTECNT, ADDREXT | `dma.h:142-155` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:76-80` |
| 93 | 8 KiB ring memory, 8 KiB aligned (BCM4331 copies address bit 0x1000 into RXSTATUS) | `dma.h:166, dma.c:407-428` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:81-83` |
| 94 | FW598 frame offset 38 | `dma.h:168` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:85` |
| 95 | descriptor fill: table end on last slot, byte count, ext, translated address | `dma.c:177-211` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:234-252` |
| 96 | address translation in the high word for 64-bit on BCMA PCI (mask 0xC0000000, DMA64_CMT 0x80000000) | `dma.c:37-68, :1033-1047, :1063-1074; bcma.h:481-484; bcma/core.c:140-149` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:89-92, :236-238, :384-387` |
| 97 | engine type from IOST.DMA64 | `dma.c:800-811; bcma_regs.h:37` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:94, :339-344` |
| 98 | parity disabled on BCMA | `dma.c:1076-1081` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:388` |
| 99 | RX controller setup: RXCTL, RINGLO, RINGHI, RXINDEX = slots x 16 | `dma.c:709-729` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:383-395` |
| 100 | RX engine reset: RXCTL 0, wait state DISABLED (10 x 1 ms) | `dma.c:439-475` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:261-271` |
| 101 | current RX slot = (RXSTATUS & DPTR) / 16 | `dma.c:231-239` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:420` |
| 102 | RXINDEX after consumption; back-off by one without the underflow IRQ | `dma.c:1684-1702, :1665-1682` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:430-432` |
| 103 | poison: frame_len 0, plcp+2 bytes 0xFF | `dma.c:570-590` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:246-250` |
| 104 | RX slot: frame_len 0 retried 5 x 2 us; oversize frames dropped slot by slot | `dma.c:1579-1642` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:274-300` |
| 105 | RX header FW598: frame_len @0, mac_status @16, channel @22 | `xmit.h:239-297` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:97-98` |
| 106 | MAC status FCSERR 0x1, PADDING 0x4 | `xmit.h:339-341` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:99-100` |
| 107 | channel status CHAN_ID 0x07F8 >> 3 | `xmit.h:346-347` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:101-102` |
| 108 | skip padding + PLCP (6 bytes); FCS 4 | `xmit.c:679-688, xmit.h:19-20` | `unaos/crates/kernel/src/wifi/bringup/rx.rs:103-104, :318-324` |

Rows: 108.

## What stays refused (receive-only), each on the wire

- `phy_ht.c:984` forced RF sequence RX2TX — the radio is never sequenced into transmit (`[wifi9] htphy-op-init … rx2tx=not-forced`).
- `phy_ht.c:1001` idle TSSI — it plays a TX tone through the sample-play engine (`:592`, `:415-421`): `idle_tssi=not-run(tx-tone)`.
- `phy_ht.c:1002-1004` TX power control setup, TSSI setup, TX power control on: TX-only, and the setup needs the SPROM's PA coefficients.
- `phy_ht.c:742-748`, `:752` → `main.c:1204-1218` the spur-avoid PLL/PMU steps: ChipCommon and the BCMA core clock control, outside the d11 window. Channel 11 takes spuravoid 0.
- `main.c:4956` the MAC address upload; no TX DMA ring, no template RAM, MACCTL.AP cleared, PRMAXTIME = 1: no frame of any kind is transmitted by this rung (`wifi9: end … transmit=none`).

## The three wire lines the bench should expect (one `tests wifi`, flight on the image line + UNAOS_WIFI9=1)

```
:: wifi9: htphy-tables when=after-reinit tables=24 elems=2294 match=2294/2294 table_1a_0xc0=late r2059_init=9/9 -> MATCH — …
:: wifi9: channel-tune ch=11 radio=2059 freq=2462 writes=<n> readback=MATCH cca=rst2rx -> DONE — …
:: wifi9: rx frames=<n> beacons=<n> bytes=<n> ssids=[<ssid>,…] fcs_err=<n> other_chan=<n> oversize=<n> empty=<n> ch=11 window=5000ms tx=none -> SEEN ::
```

then `:: WIFI1: ucode=ok d11=up scan=<beacons> -> PASS ::`. The non-SEEN verdicts name which rung to read: `FCS-ONLY(S5-feedback…)` (the PHY hears energy it cannot decode — the tune/init), `NOTHING(S6-feedback…)` (the ring never advanced — DMA, bus mastering, translation), `FRAMES-NO-BEACON`, `DESCRIPTORS-ONLY`; the first three frames' raw 24-byte RX headers are printed as `[wifi9] rx-raw`.
