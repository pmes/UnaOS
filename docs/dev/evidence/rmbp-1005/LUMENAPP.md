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
