# render12 — FLIGHT RESULT (orin 24, Orin Nano metal, 2026-09-09)

**Image:** `render12-20260909T1556Z-1b50376` = exec-orin23-fold 1b50376a (render11 600887c2 + closemin + cursorbg + apsrun + facet + prtscrsrc + netlease + dockid + wintitle + compgate + B98). kernel.elf b7cf54cbc86f2e1bf26f56d8b69ef348b2a43f96fa2530df59e34e93d4f8066e, max_vaddr 0x35b340, stamp 1b50376a. Fifteen-knob line = render11's exactly; banner delta = +apsrun. One gate on the fold 15:42–15:54Z, all legs green (GATE-fold-1b50376a.txt).

**Three boots**, same card (reader, FAT serial 0xde001a13), windows cut at the last `I> MB1` cold-boot line: A1 (idle, 2449 lines), A2 (idle, 13619), A3 (operator tests, 10307). Wire files beside this one. scorer11 on A1: 6/6 PASS exit 0 (scorer11-render12.out).

## Answered on the wire (all three boots)
| question | wire |
|---|---|
| loaded image | `KELF max=0x35b340`; `[vfs] root … sha=1b50376a` == card stamp (STAMP-MATCH by eye) |
| six cores host (R35) | `[bsprun] … online=0x3f el1cores=0x3f hosts=0x3f` |
| DHCP lease | `ip=10.42.0.171/24 gw=10.42.0.1` PASS |
| compositor gate (A46) | `COMPGATE … nested_folds=1 decl_lock=0 -> PASS`; every rollup `torn=0 decl_lock=0` |
| root by content | serial 0xde001a13, `aliased=usb->global`, mounts `/ /apps /boot`, unafs=absent (the card has one FAT partition) |
| close (closemin) | A3: 9 closes, every `hidden_after=[]`, siblings stay |
| titles (R36) | `[wm] alloc … title="Console"/"Quarry"/"Pulse"/"Shell" from=declared` |
| PrtScr volume | `PRTSCR-VOL: rung=1 source=global serial=0xDE001A13 label=UNAOS-ORIN rw=yes -> MOUNTED` |
| faults | 0 exceptions, 0 panics, all three boots |

## Failed, self-reported (executors spawned this session)
| boots | wire | branch |
|---|---|---|
| all | SD slot: `CMD8 no response` then `CMD55 APP_CMD (before ACMD41) FAILED — STOP`; `tegra-sd=absent` | exec-orin24-sdv1 abfa3e55 (fixed-unflown) |
| all | `el0cpus=0x1` every sample; spread census four wide, `khotm=0x0001` — EL0 never leaves core 0 | exec-orin24-core0 |
| A3 | `TEGRA-EL0: slot 4 backing allocation FAILED` ×7; `[quarry] launch REFUSED … no free address-space slot` ×7 | exec-orin24-el0slot |
| A3 | `PRTSCR: encoder declined (OutOfMemory) … 6913793 bytes`; `[facet] refuse … alloc(2304000)` ×3; `[wc-d] verify win=1 -> SKIP (no memory …)` | exec-orin24-mem |
| A3 | `[wc-d] verify win=2 surf=128x128 scale=4x … bad_cache=1632 bad_ram=2176 moved=2208` while every `torn=` reads 0; Peter: "THERE'S A LOT OF TEARING" | exec-orin24-tear |
| all | `unafs=absent`; Peter: "how come you are not showing me the unafs partition as /" | exec-orin24-unafsroot |

## Not evidenced (no witness fired)
cursor sweep (`restore src=` 0 lines), taskbar order (no dock press witness), EL0 placement off core 0 (blocked by core0/el0slot/mem), FRIEND-DIFF positive control (needs the data card mounted — sdv1 — and root write-locked).

## Bench notes
The serial probe was absent at session open (no /dev/ttyACM0; butler started on sight, pid 105655). The data card Peter brought (UNAOS-DATA, plain FAT) was correctly identified by label before any write; the boot card (UNAOS-ORIN, 30.2 GB) was the write target. Both memory-dir and rulebook consolidation happened the same session (3cfe28ad).
