# JD11 bench card — command output mirrored to serial (attended, on the Orin)

The payoff of JD11 IS the bench artifact: shell command **output** (`ls`/`cat`/verb results) now echoes to
the serial UART as `:: tegra: JD2 — OUT | <line> ::`, alongside the keystroke lines
(`:: tegra: JD2 — KEY … ::`) that already existed. Before JD11 the panel console had no scrollback and
output drew only to the panel, so no bench could capture a verbatim, mbench-able output transcript over the
serial bridge. This card confirms the mirror works on silicon and produces a durable transcript for every
future Orin bench. JD11 is `shell.rs`/`console.rs`-lane glue — an inert `Console` output sink that the tegra
console pump installs (`jd2_out_sink`); no `fat.rs` change, zero off-tegra behavioural change. Detail +
rationale: [`arch_arm64.md` §JD11](../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md). Mechanics reused from
JD2–JD10.

## 0. Prep the media (Peter flashes — session cannot write `/Volumes`)
- **Kernel:** rebuild the tegra ESP LAST (any `test-arm` clobbers it) and validate by COUNT:
  `UNAOS_TEGRA=1 ./arroyo esp-jetson` → `strings target/aarch64_esp/kernel.elf | grep -c 'tegra:'`
  must be **109** (up 1 from JD10's 108 — the new `:: tegra: JD2 — OUT` marker; virt clobber ≈ 0/1). Copy
  `EFI` + `kernel.elf` to the boot stick, `dot_clean`, eject.
- **Data card:** a **separate** FAT16 card (the tegra pattern — the boot stick is NOT the block device),
  present AT BOOT, in the reader behind the hub. Any card with (or on which you can `write`/`mkdir`) a couple
  of small 8.3-named files works; the pi4 fixture `UNAOSRW` card is fine.
- Hub-MSC enumeration is intermittently flaky (`vid=0000`); on a miss the shell comes up honestly with
  "no FAT filesystem" — re-seat + power-cycle. (Even that path now mirrors its output line to serial.)

## 1. Connect the serial console — VERIFY CAPTURE FIRST
```
scripts/jetson-bench-connect.sh          # RPi Debug Probe on the TTL header; tail ~/jetson-serial.log
```
⚠ With JD11 the serial bridge is the **primary output-evidence channel** — a mid-bench freeze now costs the
whole output transcript, not just a few markers. The round-6/8 host capture froze mid-bench (probe
re-enumeration suspected) — **confirm the bridge is logging a full boot to `~/unaos-bench/` before spending
bench time.** Screen-on-boot (JD4) brings the panel to a prompt on its own (~8 s). Type on the USB keyboard.

## 2. Confirm output mirrors to serial (THE money shot)
Type these on the panel; after each, look at the serial log for `:: tegra: JD2 — OUT | … ::` lines carrying
the SAME text the panel shows:
```
help                          # multi-line output -> one OUT line per help row on serial
ls                            # directory listing  -> each entry on serial as an OUT line
pwd                           # -> ":: tegra: JD2 — OUT | / ::"
```
Then produce and read back a file so the transcript carries real content:
```
write HELLO.TXT hi from orin  # -> OUT line echoing "wrote N bytes to /HELLO.TXT"
cat HELLO.TXT                 # -> ":: tegra: JD2 — OUT | hi from orin ::"   ← verbatim file content on serial
```
On the host, reconstruct the whole interleaved session (keys + output, in order) from the capture:
```
awk '/:: tegra: JD2 —/' ~/unaos-bench/jetson-serial-*.log      # KEY + OUT lines, in dispatch order
awk -F'OUT \\| ' '/JD2 — OUT/{print $2}' ~/unaos-bench/jetson-serial-*.log | sed 's/ ::$//'   # output-only transcript
```

## 3. Honest-error output also mirrors (must NOT hang, must appear on serial)
```
cat NOSUCH                    # -> OUT line "cat: /NOSUCH: not found (-ENOENT)"  (errno text on serial too)
cd NOSUCH                     # -> OUT line with the -ENOENT tag
```

## Pass criteria
Every text command's **panel output is reproduced verbatim on the serial capture** as
`:: tegra: JD2 — OUT | <line> ::`, paired in dispatch order with its `KEY` lines (a single
`awk '/:: tegra: JD2 —/'` yields the full interleaved session; the output-only filter yields a clean
transcript). `cat` of a file shows the file's bytes on serial; multi-line output (`help`, `ls`) yields one
OUT line per row; errno lines mirror too. No wedge, no dropped lines, no reordering relative to the keys that
triggered them. A whole-screen command (`gneiss`/vug) is honestly NOT mirrored (it paints the framebuffer
directly, not via `println`) — text output only. Capture serial to `~/unaos-bench/`; this transcript is the
durable bench evidence the round-9 bench could not produce.
