# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: s35 folded — bind TOOK / CHAN_VALID no / post-bind leg VOID by coordinator error; pull 32 invited)

## → kepler-fence session

Fence: s35 results — two real findings, one void line, and the void is the coordinator's fault, not yours.

REAL: your bind writes TOOK. `bind CHAN_CUR=00002000` and `bind CHAN_NEXT=00002000` — real echoes of inst_off>>12, no fault, no poison. The first successful writes into the CTXCTL surface. And `bind-post ENGINE_STATUS=00000000` — CHAN_VALID does not assert from a bare MMIO bind. That is your amendment-3 finding branch, cleanly: CTXCTL state is not built by poking its registers; per your own study, the FECS context ucode is what accepts a context.

VOID: `witness post-bind=80000000` observed nothing. The amendment directed that leg at `inst_off+0x0C` — which is the instance block in PLAIN VRAM; RAM holds whatever you write. The historic strip lives in the PFIFO channel-table REGISTER: write 0xC0000000|inst>>12 to 0x800000+(1*8), read it back, see 00002000. You implemented the amendment faithfully; the amendment was wrong. Logged in the metal log against the coordinator, alongside the pull-25 port error.

UNCHANGED: `witness-rematch end err=00000002 stat=00000005 valid=00002000` — the strip's ninth confirmation, now known to persist even with CHAN_CUR/CHAN_NEXT populated.

PULL 32 INVITATION — the corrected one-liner, propose it (short proposal is fine): after the bind (keep pull 31's sequence exactly as landed), add the REGISTER-side strip test: rewrite PFIFO_CHAN[1] word 0 (0xC0000000 | inst_off>>12) at 0x800000+(1*8), read it back immediately, print pre-bind-style and post-bind-style labels. That is the actual question pull 31 meant to ask: does a populated CHAN_CUR change what PFIFO's channel-table does with the VALID/POLL bits? Also relay the bonus line we captured for your read: `post-bind playlist_rd=00002013 playlist_rd_len=00100003` — decode 0x2013/0x00100003 against your PFIFO knowledge in the proposal if you can (playlist readback: entry visible? length field?).

## → kepler-display session

Display: scale-4 confirmed on glass — Peter: "text looks great." The console question is fully closed; lane idle, nothing owed.
