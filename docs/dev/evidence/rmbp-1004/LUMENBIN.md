# LUMENBIN — Lumen as a ring-3 window on the UnaOS desktop (rmbp-ledger B305)

Peter, 2026-10-04: "port vessels/lumen to UnaOS".

## Design

**Finding.** Lumen (`vessels/lumen`) is GTK/quartzite/Tokio and cannot build for the kernel target. The
desktop already runs ring-3 windowed programs (`user-pulse` = PULSE.ELF, `user-vug` = VUG.ELF) and since
BANDY3 a ring-3 program can FULFIL bus verbs (`user-prefs` = PREFS.BIN). There was no chat window on
UnaOS and no second implementation should be built in the kernel (R79).

**Seam.** The vessel is wiring (ROADMAP §3b principle 5). `crates/user-lumen` → `APPS/LUMEN.BIN` owns a
window and four bus verbs and nothing else: no model, no key, no network. The model is VEIN.BIN, the
ring-3 fulfiller of `CHAT_SEND 130 / CHAT_REPLY 131 / CHAT_CANCEL 132 / CHAT_STATUS 133` (VEINCORE, in
parallel). The kernel's part is plumbing only: the relay it already has (bus_route.rs), one desktop
action (⌘K = `Action::ClearView`, code 41), one dock pin row, one verb, one fixture.

**Wire used** (VEINCORE.md was not yet written at cut; the brief's fallback, reconciled at the fold):
ChatSend `[conv_id u32 LE][text]`; ChatReply `[conv_id u32][seq u32][done u8][text]`; ChatStatus
request empty, reply `[ready u8][provider up to NUL][model]`; ChatCancel `[conv_id u32]`. Streaming is
PULLED: the relay answers each request exactly once, so on `done=0` LUMEN sends CHAT_REPLY
`[conv_id][seq+1]` for the next chunk. No fulfiller = the kernel's own `-ENOENT` reply, rendered as
"no provider: start VEIN.BIN".

**Budget.** The whole program (code + data + bss + main stack) lives in the 16 KiB program window, and
the receive buffer must be `BUS_FRAME_MAX` = 4148 bytes. x86 layout: text 0x0..0x1df2, data 0x2000 (the
witness block), bss to 0x37e0, main stack 2 KiB below 0x4000; asserted in the link script. Scrollback is
1 KiB; the input line 160 bytes. The UI stays alive through a receiver THREAD (384-byte stack) parked in
`SYS_MRECV`; fallback is one blocking receive per outstanding request.

**Milestones.**
- M1 — the window: `crates/user-lumen` (transcript word-wrapped and bottom-anchored, user `>` / note `*`
  prefixes, input line with caret, Enter sends, Ctrl-K/⌘K clears, Esc cancels and never closes (R24),
  wheel/Up/Down scroll, INPUT_EV_WIN_RESIZE re-layout, provider/model footer); glyphs from the `font8x8`
  crate the kernel already links (no third bitmap copy); arroyo `build_user_lumen_x86`, builder staging
  `APPS/LUMEN.BIN`, USER_CHECK_MATRIX rows (x86 + aarch64 type-check).
- M2 — the bus and the desktop: the chat verbs on the wire as above; `Action::ClearView` (⌘K, code 41) in
  the keymap/theme/clipboard/shortcuts tables; knob `UNAOS_LUMEN=1` → feature `lumen`; dock pin row
  `lumen` (default-pinned only when the feature is on); verb `lumen` (HOST_VERBS row) = `bg /apps/LUMEN.BIN`.
- M3 — `vessels/lumen/README.md`: one paragraph naming LUMEN.BIN as the on-UnaOS face.
- M4 — fixture `tests lumen` (x86 + `wc` + `lumen`): load LUMEN.BIN off the volume, focus it, inject
  `hi` + Enter through `user_input_enqueue`, read the program's witness block out of its slot; then spawn
  VEIN.BIN if staged and send again, else SKIP with the reason.

**Witness.** `:: LUMEN: window=1 sent=<n> replies=<n> enoent=<0|1> -> PASS|SKIP ::` (fixture), with
`:: LUMEN: start win=<id> threaded=<0|1> verbs=130..133 ::` and one `:: LUMEN: window=.. ::` progress
line per exchange from the program itself.

**Owed.** Shift+Enter newline (INPUT_EV_KEY_DOWN carries no modifier bits); the aarch64 IMAGE (text is
over 8 KiB on aarch64, so the data page lands at 0x3000 and the link assert refuses — type-checks only);
paste (no ring-3 clipboard read verb); push streaming (needs a kernel relay that admits several replies
per request); the chat verb constants move to una-abi at the VEINCORE fold.

New kernel file: `crates/kernel/src/lumen.rs` — `//! CHARTER: Kernel — wm` (a fixture over the window
and input plumbing; it fulfils nothing).
