# LUMENAPP — Lumen on UnaOS is ONE program, like the Claude app (rmbp-ledger B323)

Branch `exec-rmbp-lumenapp`, cut from 776ffbe7. Ruling R82. No knob of its own: `UNAOS_LUMEN=1` (feature
`lumen`) is the only knob, and it now arms only the default dock pin and `tests lumen`.

## Design

**Finding (B323).** Lumen was built as TWO programs: `VEIN.BIN`, a ring-3 daemon that owned the chat
bus verbs 130..133, and `LUMEN.BIN`, a window that relayed to it. The operator had to start a service by
hand before `lumen` worked. Peter: "why can't it run when lumen needs it? what needs to continually run?"
· "just make it work like the claude app. it does not need system services running."

**Seam: shared-core + a ring-3 library, linked by the caller.** Vein is a library and nothing else
runs: the pure parts (Claude Messages request encoder, SSE/JSON stream decoder, HTTP/1.1 response
framing, the prefs-key resolution rules, and the provider-neutral `Transport` trait plus the client that
drives one exchange over it) live in `unaos/libs/sys/vein_core` (`no_std`, host unit-tested with a fake
transport); the syscall-backed half (SYS_RESOLVE, the NETRING3 TCP syscalls, TLS, the PREFS bus reads, the
UnaFS key file) lives in the new ring-3 crate `unaos/libs/sys/vein_ring3`, behind that `Transport` trait.
`LUMEN.ELF` links both; the smart installer's diagnosis program will link the same two crates with no
Lumen (R82 amendment). No daemon, no chat bus verbs, nothing resident: the library runs only inside its
caller, exactly as the host `vessels/lumen` links `handlers/vein`.

**Milestones.**
- **M1 RETIRE.** Delete `crates/user-vein` (VEIN.BIN), `kernel/src/vein_bus.rs`, the `vein` feature, the
  `UNAOS_VEIN` env arm and K8_FEATS arm, `build_user_vein_x86` and its staging/matrix rows, the `vein`
  shell verb (dispatch arm, HOST_VERBS row, help row), `tests vein`, `bus_route::inject` (its only caller
  was `vein rsp`), the una-abi CHAT_* constants (130..133) and ECANCELED, the Relay provider and the chat
  bus codec in vein_core, and the `vein` entry in the aarch64 desktop leg. BANDY3 registration
  (REGISTER=127, FULFIL_MIN=128), the R3PREF demo pair and the multi-frame `BUS_STATUS_MORE` reply stay
  (bus mechanism, not chat).
- **M2 PROVIDER IN RING 3.** vein_core gains `claude` (request encoder, `StreamDecoder`), `http`
  (request head, response head parser, chunked decoder) and `client` (`Transport` trait, `exchange`).
  `vein_ring3` implements `Transport` over the syscalls (TCP; TLS via embedded-tls 0.19, the crate the
  NETRING3 spike measured). `user-lumen` is relinked at `USER_XWIN_VA_X86` (elf model, the owed BIGAPPS
  relink for LUMEN) and staged as `APPS/LUMEN.ELF` with the `.note.unaos.app` note (name "UnaOS",
  type 1, flags bit0 = windowed).
- **M3 KEY AND PREFS.** `vein.provider`, `vein.model`, `vein.endpoint`, `vein.tls` from Principia over
  BUS_VERB_PREF_GET (ns `vein`); the key from `/home/<u>/.config/unaos/vein.key` only when the file
  stats with an inode id (UnaFS root; FAT has no ids) — refused on FAT with an in-window reason. No key
  ⇒ the Echo provider, so the window still works on the FAT card.
- **M4 WITNESS.** `tests lumen` becomes a ring-3-free fixture: LUMEN.ELF is staged and parses (note,
  entry, elf model).

**Witness.** Kernel: `:: LUMENAPP: image=/apps/LUMEN.ELF window=elf provider=<claude|echo>
key=<unafs|none> -> PASS ::`. Program (wire and window): `:: LUMEN: start provider=<p> model=<m>
key=<unafs|none|fat-refused> transport=<tls|http|none> ::` and per answer `:: LUMEN: reply provider=<p>
first_token_ms=<n> bytes=<n> stop=<s> ::`.

**Stays owed.** Certificate verification (the trust store, `/system/trust/roots.pem`, NETRING3 §Trust
store): embedded-tls's `UnsecureProvider` verifies nothing, so the key is sent over unverified TLS only
when the operator opts in with `vein.tls = "insecure"`; the aarch64 LUMEN image (no XWIN on aarch64);
the metal boot (R78).

## As built

| Milestone | Commit content |
| :--- | :--- |
| M1 | 5dd4e08c — every deletion listed above. GATE-VERBS 100 = 100, GATE-KNOB 0 phantom / 0 dead, knob-parity OK, k8-reach OK (one deferred row `UNAOS_TZ_MIN`, not this arc's). The builder's merge10 fold had dropped two closing braces after the VEIN and LUMEN staging arms; restored. |
| M2+M3 | `vein_core` gains `claude`, `http`, `client`, `prefs`, `json`, `role` (22 host tests, incl. every-split SSE and chunked decoding, a fake-transport exchange, the provider/key rule table); `unaos/libs/sys/vein_ring3` (new); `crates/user-lumen` rewritten as the one program. |
| M4 | `kernel/src/lumen.rs` rewritten ring-3-free; `vein_core` back in the kernel as an OPTIONAL dep behind `lumen` (`lumen = ["dep:vein_core"]`) so the fixture applies the program's own rule. |

**LUMEN.ELF as measured** (`readelf -lnW target/LUMEN-X86.ELF`, arroyo's `user_elf_window_check`): file
127704 B; PT_LOAD R+X at 0x10000200000 (text 114555 B), R (rodata + note), RW (data 224 B, memsz 130288 B:
the transcript, the 48 KiB request body, the 16 KiB SSE line buffer, the TLS record buffers 16640 + 4096);
PT_GNU_STACK 262144; PT_NOTE 24 B, owner `UnaOS`, type 1, desc `01 00 00 00`. `model=elf span=253168
stack=262144 need=519408 cap=4194304`.

**The request.** `POST /v1/messages HTTP/1.1`, `Host`, `Content-Type: application/json`, `Accept:
text/event-stream`, `anthropic-version: 2023-06-01`, `anthropic-beta: server-side-fallback-2026-07-01` (for
models that take `"fallbacks": "default"`), `x-api-key` (only when the rule allows), `Content-Length`,
`Connection: close`. Body: `model` (`vein.model`, default `claude-opus-5-5`), `max_tokens` 8192, `stream:
true`, `system`, the transcript's user/assistant turns (notes never sent; consecutive same-role turns
merged; leading assistant turns dropped). No `thinking` field, no prefill; `stop_reason` handled
(`refusal` and `max_tokens` noted in the window).

**The rule** (`vein_core::prefs::plan`, the same call in LUMEN.ELF and in `tests lumen`):
`vein.provider = "echo"` → Echo. An `http://` `vein.endpoint` → Claude through that relay, key NOT sent.
`https://` (default) → needs the key from UnaFS (none → Echo "no API key"; on FAT → Echo "key refused");
and because no certificate is verified yet, the key is sent only with `vein.tls = "insecure"` (else Echo
with the reason). Every Echo reason is printed in the window.

**The key path is a preference** (`vein.key_file`, conventionally `<home>/.config/unaos/vein.key`): a
ring-3 program has no syscall that tells it its home (the kernel resolves homes through
`fs::users::home_of`, never a literal). Ring-3 `SYS_OPEN` takes at most 40 bytes of path (`MAX_NAME`).

## Witness lines a metal boot should print

`UNAOS_LUMEN=1 UNAOS_NETRING3=1 UNAOS_UNAFS=1` (busreg is not needed by Lumen any more), then `tests lumen`:

    [lumenapp] bytes=127704 entry=0x200000 segs=3 stack=262144 note_flags=Some(1)
    :: LUMENAPP: image=/apps/LUMEN.ELF window=elf provider=echo key=none -> PASS ::

(`entry` is printed as the window offset the elf-model plan rebases to.) Then `lumen` (the window opens):

    :: RING3WIN: model=elf slot=<s> segs=3 span=253168 stack=262144 heap=[…) frames=<n> ::
    :: LUMEN: start provider=echo model=reverse key=none transport=none ::

typing `hi` + Enter: `:: LUMEN: reply provider=echo first_token_ms=<n> bytes=8 stop=end_turn ::`. With
`pref set vein.key_file "/home/<u>/.config/unaos/vein.key"` (the file on the UnaFS root) and `pref set
vein.tls "insecure"`: `provider=claude model=claude-opus-5-5 key=unafs transport=tls`, and per answer
`:: LUMEN: reply provider=claude first_token_ms=<n> bytes=<n> stop=end_turn ::`; a handshake failure is
`:: LUMEN: fail stage=tls-handshake code=-71 tls=<TlsError> ::`.

## Owed

- Certificate verification (the trust store; `vein_ring3::VERIFIES_CERTS = false`).
- A ring-3 "who am I / where is my home" surface (Holocron's), so `vein.key_file` can default.
- The aarch64 LUMEN image (no ELF window on the aarch64 loader; TLS deps are x86_64-only).
- The metal boot (R78): the TLS handshake against api.anthropic.com has never run on the rMBP.
