# FOLD: the five paused executors folded onto the LOGIN14 branch (2026-09-24)

Branch `claude/optimistic-ramanujan-r3qyu5`, cloud session (focus rmbp). Folds, in order, each a `--no-ff`
union merge of the WIP tip the seat committed on 2026-09-23 (rows B201, B202, B199, B200, B203):
BOOTSLOW, LFNMV, WCDMEM, MENULOCK, GLASSFIX3. Conflicts were tail-appended blocks on both sides in every
case; one `}` lost at the WCDMEM/GLASSFIX3 seam in `video/wm.rs` was restored by brace and line arithmetic
against both parents over the base before any compile.

## Gates

Run on the fold tip, recorded here as they report (see the rmbp-queue STATE for the rc list until then).

| gate | result |
|---|---|
| `./arroyo check` on 8de29731 (nightly 2026-07-14, `x86_64` 0.15.5) | rc=1, 19m31s: GATE-BRACES 209/209 balance; **all 84 kernel legs rc=0** (the same 84 the clean-tree baseline has); the rc=1 is the container's, identical to the clean baseline's: the host ring-3 suites that need OpenSSL/ALSA/GTK headers (aether, phonolite, resonance, stria, una), `matrix --test finder` `write_to_readonly_dir_surfaces_loud_denial` (a read-only directory is writable to root, and this container runs as root), and GATE-LEDGER's 245 pre-existing findings (the fold's rows add 0: 245 before and after) |
| the same gate on the previous fold tip 90208e1c | rc=1 in 1.6 s: GATE-BRACES `arch/x86_64/syscall.rs` {=2799 }=2798 — the LFNMV union had lost a `}` at the LOGIN13/LFNMV seam; restored in 8de29731. Two seams, two lost braces (WCDMEM/GLASSFIX3 in `wm.rs` caught by hand arithmetic, this one by the gate): count braces against BOTH parents over the base on every union, and run the gate before the compile |
| `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test 240` on f5efe21a; `mbench --spec x86-wc.spec` and `--spec x86-default.spec` | completion marker reached, full wall 240.7 s; **x86-wc 36/36 required, 0 forbidden; x86-default 24/24, 0 forbidden**. The verb itself exits 1 on fault text: `:: SOCK-3: ring-3 tcp round-trip FAIL — witness=0x1 cleared=true killed=0 done=1 (want 0x1f/true/0/1) ::` — the ring-3 TCP fixture, touched by none of the folded arcs; its attribution is the control row below |
| `./arroyo test-arm` on f5efe21a | rc=0, 20 s wall (no completion source declared for this verb); no `:: LFNMV` line on virt (the aarch64 launcher waits for a block device and virt has none — the Pi's `kernel8-test` is that leg, R39) |

## The fold witnesses on the x86 `wc` wire (green run)

```text
:: LFNMV: sys_rename_long=-ENOENT shell_mv_long=ok alias_leak=false readback=ok (x86_64: SYS_RENAME LFNMV.TMP -> LongNameViaSysRename.txt refused before any write; shell mv /boot/LFNMV.TMP -> /boot/Lo
:: BPACE: root-bind t=3387ms d=35ms ::
:: WCDLATCH: ids=2 calls=24 said=2 want_said=2 counted=48 want_counted=48 -> PASS ::
[wc-d] latch-check forced=16 printed=1 rolled=16 win=30 reroll=0 -> PASS
[wc-d] latch-check forced=16 printed=1 rolled=16 win=31 reroll=0 -> PASS
```

```text
:: GLASSFIX2: sprite same=1 backdrop=9x9 window=9x9 scale=1 owns_paint=1 compositor=9x9 backbuffer=18x18 | cascade overlaps=0 worst=win0-over-win0:0rows minted=4/4 pinned=3/3 n=7 control=1 expect_overlaps=0 tb=34 step=39 panel=1280x800 scale=1 | console=win0 pulse=win0 pulse_over_console=0 -> PASS ::
```

