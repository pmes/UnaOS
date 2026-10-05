# LUMENUX — the Lumen window, like the Claude app (rmbp-ledger B348)

Branch `exec-rmbp-lumenux`, cut from 6e89f508. Builds on LUMENAPP (B323, R82). No new knob: `UNAOS_LUMEN=1`
(feature `lumen`) arms `tests lumen`; LUMEN.ELF is built on every image.

## Design

**Finding (B348).** LUMEN.ELF (B323) is a chat line and a stream: the transcript is a 32 KiB byte log
re-wrapped on every paint (nothing older than the log is reachable, and the log is also the context sent),
replies are raw text, there is no copy, nothing survives a relaunch, and the footer shows only provider /
model / first-token latency.

**Seam: shared-core (`vein_core`, no_std, host-tested) + the ring-3 program + two kernel verbs.**
The pure parts go into `unaos/libs/sys/vein_core` where host `cargo test` proves them and the kernel's
`tests lumen` re-runs them: `md` (the markdown line renderer), `history` (the conversation-file codec),
`scroll` (the rendered-row ring), and the stream decoder's usage counts. The syscall-backed file surface is
`vein_ring3::files`. The ring-3 program (`crates/user-lumen`) is the only consumer that draws. The kernel
gains the two clipboard verbs `video/clipboard.rs` named and did not build (`SYS_CLIP_SET` 63 /
`SYS_CLIP_GET` 64, over its `set`/`get`), appended at the una-abi tail, x86 dispatch only (aarch64 has no
LUMEN image; the numbers fall to its unknown-syscall default). 59..=62 are taken on `exec-rmbp-merge12`
(SELFDIAG `SYS_PATH_READ`/`SYS_PATH_WRITE`, RING3ABI2 `SYS_WHOAMI`, PROFILE2 `SYS_PROF`), hence 63/64.

**The AST question.** QUARRY2's `video/richtext.rs` (on `exec-rmbp-quarry2`, not in this tree) renders a
whole document into an `alloc::Vec` of display bytes plus `Span { start, end, tint, bold }`. LUMEN.ELF has
no allocator and renders a stream LINE BY LINE as it arrives (block state = the open fence carried between
lines), so `vein_core::md` keeps the same SHAPE — display bytes + `Span { start, end, tint, bold, italic }`
with `Tint` named as richtext's (`Plain`, `Heading`, `Dim`, `Code`, `Quote`, plus `Link`) — but writes into
caller buffers one source line at a time. richtext can be re-expressed over `md::line` (a loop that feeds it
each line into a Vec) when Tabula's core exists; it is not shared today because it is alloc-only and lives in
the kernel tree.

**Milestones.**
- **M1 SCROLLBACK.** The transcript log grows to 1 MiB (the bytes the history file holds); the context sent
  is the newest turns that fit 32 KiB (the 48 KiB request body less escaping room), never a half turn. A ring of
  rendered lines (`vein_core::scroll::Ring`, 12-byte records: source offset, display slice, role, block,
  indent, fence bit) is relaid incrementally (only from the last source line of the open turn) while a reply
  streams, fully on a resize or a log compaction. Keys: Up/Down a line, Shift+PgUp/PgDn (INPUT_EV_ACTION
  22/23, already delivered to a focused ring-3 window by `pack_input`) a page, Ctrl/Cmd+Home/End (24/25) top /
  bottom, the wheel (INPUT_EV_WHEEL, already in the ABI) three lines per detent. A scrollbar at the right
  edge shows position and extent. Bare PageUp/PageDown have no decoded byte on either HID path (xhci usage
  0x4B/0x4E map to 0) — not this arc's.
- **M2 MARKDOWN.** `vein_core::md::line`: ATX headings, `**bold**`/`__bold__`, `*italic*`/`_italic_`,
  bullet (`-`/`*`/`+`) and numbered lists with a hanging indent, fenced code (the open fence carried, so a
  fence still streaming stays monospace-tinted), `` `inline code` ``, `[text](url)` shown as `text (url)`,
  block quotes, thematic breaks. The window draws bold as a double strike, italic as a 1-px shear, code on a
  tinted ground.
- **M3 COPY.** `SYS_CLIP_SET(ptr, len) -> len` / `SYS_CLIP_GET(ptr, cap) -> len`: text only, `CLIP_CAP`
  (4096), the session-epoch ownership `clipboard.rs` already enforces, and the open ownership question
  answered: only the program holding KEYBOARD FOCUS (`USER_INPUT_ACTIVE`) may set or read (`-EACCES`
  otherwise), so a background program can neither overwrite nor sniff the clipboard. Cmd-C (action 3)
  copies the SELECTED reply (default the last; Ctrl-P / Ctrl-N move the selection), `/copy` the last;
  Cmd-V (action 5) pastes into the input line. A reply longer than 4096 bytes is copied up to the last line
  break that fits, and the window says so.
- **M4 HISTORY.** One file per conversation, `/home/<u>/.config/unaos/lumen/NNNN.md`, over whichever file
  surface the kernel offers (`vein_ring3::files::probe`, once): **PATH** = SELFDIAG's `SYS_PATH_READ`/`WRITE`
  with the home from RING3ABI2's `SYS_WHOAMI` (absolute paths through the VFS, UnaFS or FAT, parents made by
  `PATH_W_MKDIRS`) — those are on merge12, not in this cut, so `files::next` carries their numbers and layouts
  verbatim; else **OPEN** = `SYS_OPEN` O_CREAT / `SYS_SEEK` / `SYS_WRITE` on the path RELATIVE to the home
  (the DIRNS resolver starts a relative EL0 path at `/home/<user>`: `.config/unaos/lumen/NNNN.md` is 27
  bytes, inside the 40-byte cap for every user name; the directory must exist — that surface has no mkdir);
  else history is off and the window says why. Append-only: each turn is appended as it
  completes (`vein_core::history::turn`: a `<!-- lumen:user|assistant|note -->` marker line, the markdown,
  a blank line — HTML comments, so the file renders anywhere). On launch the highest-numbered file is
  reloaded; `/new` starts the next number; `/list` lists the conversations with their first line;
  `/open N` reloads one. NNNN is a sequence, not a time: ring 3 has no wall clock (SYS_GETINFO is ticks since
  boot) and SYS_STAT takes absolute paths only, so the file header records the boot-relative ms. Wall-clock
  names are owed.
- **M5 STATUS.** Two footer rows: provider / model, then transport (`tls unverified` until the trust store
  lands — the verified issuer goes there), the token counts from the stream's `message_start` /
  `message_delta` usage (`vein_core::claude` gains `input_tokens`/`output_tokens`, carried on `Outcome`), and
  the first-token latency. `tests lumen` gains `:: LUMENUX: md=ok history=ok clip=ok scroll=<lines> -> PASS ::`.

**Witness.** Kernel (`tests lumen`, after the LUMENAPP/LUMENCRASH lines):
`:: LUMENUX: md=<ok|bad> history=<ok|bad> clip=<ok|bad> scroll=<lines> -> PASS|FAIL ::`. Program:
`:: LUMEN: start … history=<N|off> files=<path|open|off> ::`, per reply `:: LUMEN: reply … in=<n> out=<n> ::`, and
`:: LUMEN: clip set=<n> ::` / `:: LUMEN: history <op> code=<n> ::` on a failure.

**Stays owed.** Wall-clock conversation names; the verified TLS issuer (trust store); the aarch64 LUMEN
image (and with it the aarch64 arms of the three verbs); pointer selection of text inside a reply (the
selection is a whole reply); bare PageUp/PageDown decoding; the metal boot (R78).

## As built

| Milestone | Content |
| :--- | :--- |
| M1 SCROLLBACK | `vein_core::scroll` (`Rec` 12 B, `Ring` push/drop-oldest/truncate/rebase, `LUMEN_ROWS` = 131072); LUMEN.ELF lays the transcript into the ring incrementally (`relayout`: from the cursor at the last laid source line), keeps a scrolled-back reader's rows in place while an answer grows, scrollbar thumb on the right edge; keys Up/Down, wheel ×3, actions 22/23/24/25. |
| M2 MARKDOWN | `vein_core::md` (`line`, `wrap`; 6 host tests incl. every-prefix streaming and small-buffer overflow); the window draws headings in a heading ink + double strike, italic as a 1-px shear, code on a tinted ground (whole row inside a fence), links in link ink with the URL dimmed, rules as a line, list hangs. |
| M3 COPY | una-abi `SYS_CLIP_SET`=63, `SYS_CLIP_GET`=64, `CLIP_CAP`=4096; x86 dispatch (same line as SYS_CLOSE/SYS_SBRK, code before the `//`) and bodies at the syscall.rs tail (`clip_focus_ok`: caller slot + 1 == `USER_INPUT_ACTIVE`). LUMEN.ELF: Cmd-C copies the selected reply (Ctrl-P/Ctrl-N select, a bar marks it), `/copy` the last, Cmd-V pastes; longer than 4096 → up to the last line break that fits, flashed `copied N of M bytes`. |
| M4 HISTORY | `vein_core::history` (path, header, turn, parse, title; marker lines escaped); `vein_ring3::files` (probe, read, size, append, exists over PATH or OPEN). Launch reloads the highest NNNN (its newest 256 KiB); `/new`, `/list` (last 20 with titles), `/open N`. Each user turn is appended on send, each reply when it ends. |
| M5 STATUS | `StreamDecoder` gains `input_tokens` (message_start usage: input + cache read + cache creation) and `output_tokens` (message_delta usage), carried on `client::Outcome`; footer row 1 `provider / model`, row 2 `tls unverified|http relay|offline  in N out N  1st Nms`; reply wire gains `in=` `out=`. `tests lumen` gains the LUMENUX line. |

**Measured fit (M1).** `user_elf_window_check` on the stripped LUMEN-X86.ELF: file 157864 B, `model=elf
span=3244104 stack=262144 need=3510344 cap=4194304` — the transcript (1 MiB) + the ring (131072 rows × 12 B =
1.5 MiB) + the 256 KiB file buffer + the 64 KiB append buffer + the 33 KiB PATH request fit with 684 KiB spare
(another ~57000 rows would fit; kept as headroom). ELFENTRY: `insns=31980 rip_refs=798 -> PASS`.

## Witness lines a metal boot should print

`tests lumen` (UNAOS_LUMEN=1), after the LUMENAPP / LUMENCRASH lines:

    :: LUMENUX: md=ok history=ok clip=ok scroll=131072 -> PASS ::

(the `[clip] set len=19 …` / `[clip] get …` lines of the probe precede it). Then `lumen`:

    :: LUMEN: start provider=echo model=reverse key=none transport=none history=off files=off files_code=-38 ::

on this cut's metal shape (no `irqstorage`, no SELFDIAG: ring 3 has no file surface, the window says
"history off …"); on a tree with SELFDIAG + RING3ABI2: `history=<N> files=path`. Typing `hi` + Enter:
`:: LUMEN: reply provider=echo first_token_ms=<n> bytes=8 stop=end_turn in=- out=- ::`; Cmd-C:
`[clip] set len=<n> epoch=<e>` then `:: LUMEN: clip set=<n> ::`.

## Owed

- **The ring-3 file surface on the metal shape.** The metal feature line has no `irqstorage`, so ring-3
  `SYS_OPEN` of any non-staged path is ENOENT — history is off there until SELFDIAG's `SYS_PATH_*` (merge12) is
  in the image, AND the same fact means `vein.key_file` (B323's key read, `vein_ring3::key`) cannot be read on
  metal either: the key path wants the same PATH surface.
- Wall-clock conversation names (ring 3 has no clock; NNNN is a sequence, the header carries boot ms).
- The verified TLS issuer in the status line (trust store); the aarch64 LUMEN image and the aarch64 arms of
  63/64; pointer selection inside a reply; bare PageUp/PageDown decoding (xhci maps usage 0x4B/0x4E to 0).
- The metal boot (R78).
