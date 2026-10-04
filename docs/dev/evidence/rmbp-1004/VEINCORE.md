# VEINCORE — Vein on UnaOS: a ring-3 VEIN.BIN owns the chat verbs (ledger B304)

Branch `exec-rmbp-veincore`, cut from `b42b87cc`. Seam: **shared-core + fulfiller** (the
midden_core / prefs_core shape: one `no_std` core both rings link; ring 3 owns the verbs; the kernel
only relays). Knob: `UNAOS_VEIN=1` → feature `vein` (implies `busreg`).

## THE WIRE (read this first — LUMENBIN renders from it)

All integers little-endian. Every chat frame is an ordinary BANDY v1 frame (52-byte header, 4 KiB body
ceiling, kernel-stamped principal). The codec of record is `unaos/libs/sys/vein_core/src/wire.rs`; the
KATs there are the spec.

| Verb | Tag | Direction | Body |
| :--- | ---: | :--- | :--- |
| `BUS_VERB_CHAT_SEND` | 130 | REQUEST caller → VEIN | `conv_id u32` · `text` (UTF-8, 1..=4092 bytes) |
| (its replies) | 130 | REPLY VEIN → caller (relayed) | **ChatReply**: `conv_id u32` · `seq u16` · `done u8` (0/1) · `rsvd u8` = 0 · `text` (0..=4088 bytes) |
| `BUS_VERB_CHAT_REPLY` | 131 | REQUEST **kernel** → VEIN only (the relay companion's answer, injected by the shell verb `vein rsp`) | ChatReply, as above |
| `BUS_VERB_CHAT_CANCEL` | 132 | REQUEST caller → VEIN | `conv_id u32` |
| `BUS_VERB_CHAT_STATUS` | 133 | REQUEST caller → VEIN | empty |
| (its reply) | 133 | REPLY | **ChatStatus**: `ready u8` · `plen u8` · `mlen u8` · `rsvd u8` = 0 · `provider[plen]` · `model[mlen]` |

**A long answer is a SEQUENCE of REPLY frames on the caller's one correlation id.** `seq` counts 0,1,2…
per answer. Every non-final frame carries header `status = BUS_STATUS_MORE` (= 1) and `done = 0`; the
final frame carries `status = 0` and `done = 1`. An error ends the stream with a negative errno and an
empty body (the frozen error-reply rule): `-ECANCELED` (-125) when a ChatCancel cut it, `-EIO` when the
provider failed, `-EINVAL` for a malformed body. ChatCancel's own reply: status 0, empty body (0 also when
nothing was in flight). A caller renders frames in arrival order; the relay preserves order (one
mailbox FIFO per row). A ring-3 program other than the kernel sending verb 131 is refused `-EACCES`.

## M1 — design

**Finding.** Lumen (`vessels/lumen`, GTK/Tokio) and Vein (`handlers/vein`, Tokio/reqwest) are host-only.
ROADMAP §3b: port the bus, not the binary. BANDY3 (`bus_route.rs`) lets a ring-3 program own tags
128..=255. Nothing owns chat on the metal.

**What `bus_route` did with a multi-frame answer (read, `fulfiller_reply`).** Exactly ONE reply per
correlation: the pending entry is `take_pending`'d on the first fulfiller REPLY, so a second frame with
the same relay id is `-ENOENT` to the fulfiller and never reaches the caller. And `frame_parse` refuses a
REPLY whose nonzero status carries a body. **The `more` flag (one line of design):** a fulfiller REPLY
with `status == BUS_STATUS_MORE` (= 1, the only positive status on the wire, never an errno) is delivered
WITH its body and LEAVES the pending entry open; status 0 or an errno closes it. Code: `frame_parse`
admits a body under `BUS_STATUS_MORE` (same line), `fulfiller_reply` peeks instead of takes for MORE,
`deliver` keeps the body for MORE. A full caller mailbox is `-EAGAIN` to the fulfiller (pending kept),
so VEIN.BIN yields and retries; the caller's 16-deep mailbox is the flow control.

**The kernel's other part: `bus_route::inject`.** The serial wire is the SHELL (SERIALDOOR: "the wire is
a console"), and ring 3 has no console read. So the relay companion's answer comes in as a shell line,
`vein rsp …`, and the kernel builds a REQUEST frame (verb 131, corr 0, principal = the reserved KERNEL
record) straight into the fulfiller's mailbox — no pending entry, fire-and-forget. That is relay
plumbing, not chat logic; the kernel never parses a chat body beyond base64-decoding the shell argument.

**vein_core** (`unaos/libs/sys/vein_core`, `#![no_std]`, zero deps, `#![forbid(unsafe_code)]`):
* `wire` (no alloc): the four bodies above, encode/decode over borrowed slices, KATs. Also the
  relay line codec: base64 (std alphabet, padded) and the `[vein-relay]` line format.
* `provider` (no alloc): `trait Provider { fn name(); fn model(); fn ready(); fn begin(conv, text, sink) }`
  with `Echo` (the prompt reversed by chars, annotated `echo: `) and `Relay` (writes a REQ line through
  a caller-supplied `LineOut`, answers later when the 131 frames arrive). ChatReply chunking lives here.
* `model` (feature `alloc`, default on): `Role`, `ChatMessage{role, text}`, `Conversation` (bounded
  history, `MAX_TURNS`), `ChatRequest{system, messages, max_tokens}`, `ChatResponse{text, stop}`,
  `StopReason`. **Names mirror VEINPROV's `libs/gneiss_pal/src/api/mod.rs`** (`ChatRequest`,
  `ChatResponse`, `ChatMessage`); VEINPROV's types are not in this tree — reconciled at the fold.
* `context` (alloc): the pure half of the assembler — `assemble(system, &Conversation) -> ChatRequest`
  and the engram compression prompt (`ENGRAM_SYSTEM`, `engram_prompt`) moved byte-for-byte out of
  `handlers/vein/src/context.rs`. What stays host-side: the `ResilientClient` call, the DiskManager
  retrieval (cortex/vault), gravity scoring, uploads.

VEIN.BIN links `vein_core` with `default-features = false` (no allocator in a 16 KiB ring-3 window).

**Relay line format (the bench bridge).** VEIN.BIN writes, on its console fd (SYS_WRITE fd 1 → serial):

    [vein-relay] REQ <conv> <base64(user text)>

one line, `\n`-terminated, `<conv>` decimal. The host companion runs the host Vein on the text (the
host keeps the history per `<conv>`, with the same `vein_core::Conversation`) and types back into the
serial console, one or more lines, each ≤ 200 characters:

    vein rsp <conv> <seq> <done> <base64(chunk)>

`<seq>` from 0, `<done>` 0 on all but the last. Each line becomes one ChatReply on verb 131 into
VEIN.BIN, which forwards it to the waiting caller. `vein rsp <conv> 0 1 -` with `-` for the body is an
empty final frame. The 20-line bridge is in §Bridge below.

**Provider choice.** VEIN.BIN asks `BUS_VERB_PREF_GET` (16) `vein.provider` (Principia's store,
fulfilled in-kernel by PREFS): `"echo"` | `"relay"` (a TOML string literal comes back quoted; both forms
accepted). Unset / -ENOENT / anything else → `echo`. `claude` is reserved (TLS: NETRING3).

### Milestones

* **M1** vein_core: wire codec + KATs, Provider/Echo/Relay, model, context. `cargo test -p vein_core`.
* **M2** una-abi `BUS_VERB_CHAT_SEND..=STATUS` (130..=133), `BUS_STATUS_MORE`, `ECANCELED`; the relay
  `more` flag; `bus_route::inject`; knob `vein` (Cargo, arroyo env arm, `K8_FEATS` arm).
* **M3** `crates/user-vein` → `APPS/VEIN.BIN` (x86 static ELF, arroyo `build_user_vein_x86`, builder
  staging; check-matrix rows x86 + aarch64).
* **M4** `handlers/vein` links vein_core (engram prompt + model); `tests vein` kernel fixture; shell verb
  `vein` (`vein status` · `vein rsp …`) with its HOST_VERBS row.

### Witness lines

* `bg /apps/VEIN.BIN` → `:: VEIN: registered=4 provider=<echo|relay> served=0 frames=0 -> PASS ::` once
  at start, then per served request `:: VEIN: served=<n> frames=<n> verb=<v> conv=<c> status=<s> ::`.
* `tests vein` → `:: VEINBUS: mode=<live|scratch> kats=<n> frames=<n> more=<n> done=1 cancel=0 status=0 -> PASS ::`.

### Bridge (the bench companion, documented not shipped)

The bench machine holds the rMBP's serial console (the FTDI wire, `/dev/ttyUSB0` here) and answers each
REQ with the host Vein. `VEIN_CMD` is whatever turns a prompt on stdin into an answer on stdout (the host
Vein CLI once VEINPROV lands; `rev` proves the loop). Python 3, pyserial:

```python
import base64, os, re, serial, subprocess, sys
port = serial.Serial(sys.argv[1] if len(sys.argv) > 1 else "/dev/ttyUSB0", 115200, timeout=1)
cmd = os.environ.get("VEIN_CMD", "rev")
req = re.compile(rb"\[vein-relay\] REQ (\d+) ([A-Za-z0-9+/=]*)")
while True:
    m = req.search(port.readline())
    if not m:
        continue
    conv, prompt = int(m.group(1)), base64.b64decode(m.group(2))
    answer = subprocess.run(cmd, shell=True, input=prompt, capture_output=True).stdout
    chunks = [answer[i:i + 120] for i in range(0, len(answer), 120)] or [b""]
    for seq, c in enumerate(chunks):
        done = 1 if seq == len(chunks) - 1 else 0
        body = base64.b64encode(c).decode() if c else "-"
        port.write(f"vein rsp {conv} {seq} {done} {body}\r".encode())
        port.flush()
```

120 bytes of answer is 160 base64 characters (`RSP_B64_MAX`), so every line stays under 200. The shell
prints `[vein] rsp conv=<c> seq=<s> done=<d> bytes=<n> inject=0` per line; `inject=-2` means no VEIN.BIN
owns the verbs, `-11` that its mailbox was full (resend the line).

### What stays owed

`claude` provider on the metal (TLS + DNS: NETRING3); the window (LUMENBIN); an aarch64 VEIN.BIN media
step (only the aarch64 check-matrix row is added); the host bridge script is documented, not shipped;
the metal boot (R78).
