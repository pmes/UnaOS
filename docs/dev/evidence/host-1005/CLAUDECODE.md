# CLAUDECODE — Vein through a Claude subscription (LEDGER SR38)

Branch `exec-host-claudecode`, cut from `7c7fa62b`. Host-only (Ring 3); the kernel is not edited.

## Finding

Vein had two providers, both paid per token through an API key (Anthropic Messages API, Gemini).
A Claude Pro/Max subscription gives no API key. The sanctioned way a program uses a subscription is
the **Claude Code CLI logged into that account**, driven headless (`claude -p --output-format
stream-json`). The arc adds that CLI as a third `ModelProvider`: the CLI is the client, Vein is its
caller. Nothing reads the CLI's credential store, lifts an OAuth token, or talks to claude.ai.

Observed while recording (Claude Code 2.1.289, this container):

1. **A parent Claude Code session leaks into a child `claude`.** With `CLAUDE_CODE_SESSION_ID`,
   `CLAUDE_CODE_REMOTE_SESSION_ID` and `CLAUDE_CODE_CHILD_SESSION` inherited, `claude -p` reported the
   PARENT's `session_id` (so `--resume` would have continued the parent's transcript). Removing
   those three (and only those) gives the child its own session and keeps its login. The provider
   removes them from every spawn (`INHERITED_SESSION_VARS`); a test proves the fake CLI never sees one.
2. **A refused credential stalls for ~3 minutes.** With a bad key the CLI emits
   `system/api_retry` (`error_status: 401`, `error: "authentication_failed"`) ten times with backoff
   (179 s observed), then a synthetic `assistant` message flagged `"error":"authentication_failed"`,
   `"is_api_error_message":true`, then `result` with `is_error: true, api_error_status: 401`, exit 1.
   The provider fails at the FIRST 401/403 retry and kills the child.
3. The CLI's synthetic error text arrives as an ordinary `assistant` content block; it is never
   shown as the model's reply (the `error` field is checked first).
4. Partial streaming needs `--include-partial-messages`: the CLI then forwards the Messages-API
   stream events (`stream_event` → `content_block_delta`/`text_delta`), so Lumen sees text as it is
   generated, not one blob per message.
5. Prompts go on **stdin** (`claude -p` with no positional prompt reads it): one argv string is
   capped at 128 KiB on Linux and Vein's system prompts carry memory context. System prompts over
   64 KiB go through `--system-prompt-file` (a temp file removed on drop).

## What landed

| M | What | Where |
| :-- | :-- | :-- |
| M1 | `ClaudeCodeProvider` (`name() = "claudecode"`): spawns `claude -p --output-format stream-json --verbose --include-partial-messages --tools "" [--model M] [--system-prompt S] [--resume ID]`, prompt on stdin, stderr drained off the read path; `CliEvents` translates the NDJSON (`system/init` → session id, model, CLI version; `stream_event` text deltas → `ChatDelta::Text`; `assistant` text only when no partials arrived for that message id; `result` → `ChatDelta::Stop` with `stop_reason` via the Claude provider's `map_stop` and usage = input + cache-creation + cache-read / output). `tools` OFF: Vein asks for a reply, never for the CLI to run commands on the machine. Recorded fixtures + fake CLI on `PATH`. | `libs/gneiss_pal/src/api/claudecode.rs`, `libs/gneiss_pal/tests/fixtures/claudecode/` |
| M2 | Session continuity: after a completed turn the provider remembers `(session_id, fingerprint(messages + reply), len)`; a request that starts with exactly that conversation and continues with only the person's turns is `--resume <id>` with just the new turn(s) on stdin; anything else (other conversation, edited history) is a fresh session with the history flattened into a `<conversation-history>` preamble. A failed turn forgets the session. Error mapping: missing binary / not executable → `ProviderError::Config` naming the path and `vein.claudecode.bin`; the CLI's not-logged-in line (stdout or stderr) → `Config` quoting it verbatim; `result.is_error` → by `api_error_status` (401/403 = login → `Config`; 408/409/429/5xx → `Retryable`; other 4xx → `Request`) else the new `ProviderError::Cli { code, message }`; non-zero exit without a result → `Cli { code, stderr tail }`; exit 0 without a result → `Malformed`. | same |
| M3 | Selection by pref: `ProviderKind::ClaudeCode` (`vein.provider` = `claudecode`, also `claude-code`/`claude_code`), `AuthMode::Cli`, `ProviderConfig.claudecode_bin` from `vein.claudecode.bin` (default `claude` on `PATH`), model default `default` (= pass no `--model`; the plan's own). Schema: `vein.provider` enum gains `claudecode`; new row `vein.claudecode.bin` (string ≤4096 printable, default `"claude"`); rule `chat-model` yields `default` for `claudecode`; `docs/dev/PREFS-SCHEMA.md` regenerated. `MODEL_CHOICES` gains `default (claudecode)`. Lumen path: Lumen → `VeinHandler` → `ProviderSlot::load()` → `ProviderConfig::from_prefs` → `build_provider`; a vein test writes `vein.provider = "claudecode"` into Principia's file (the schema validator accepts it) and gets `ONLINE (claudecode / default)`, and a missing binary reaches the chat verbatim. Check tool: `cargo run -p gneiss_pal --example vein_check -- --provider claudecode [--bin P] [--model M] [--turns 2]` (no `tools/vein-check` existed; an example of the crate that owns the seam adds no crate). | `libs/gneiss_pal/src/api/provider.rs`, `unaos/libs/sys/prefs_core/src/{schema,rules}.rs`, `docs/dev/PREFS-SCHEMA.md`, `handlers/vein/src/provider.rs`, `libs/gneiss_pal/examples/vein_check.rs` |

## The oracle

The CLI's own `result.result` field is its concatenation of the reply. Every replay test asserts the
provider's streamed `Text` deltas concatenate to exactly that string, and that the deltas are the
recorded `text_delta`s in order (two for turn 1, i.e. incremental, not one blob).

**Fixtures — RECORDED from the real CLI** (`/opt/node22/bin/claude`, 2.1.289, logged in on this
host), then trimmed: telemetry, rate-limit, account-shaped and timing fields removed; the session
ids replaced by fixed fake UUIDs (`00000000-0000-4000-8000-00000000cc0N`); message ids kept.

| file | what | bytes |
| :-- | :-- | --: |
| `turn1.jsonl` | fresh session, `--include-partial-messages`: "one, two, three, four, five" in two deltas | ~2.7 K |
| `turn2.jsonl` | the same session `--resume`d: "six, seven, eight" | ~2.5 K |
| `nopartial.jsonl` | without partials: text only in the `assistant` message | ~0.7 K |
| `authfail.jsonl` | a refused key: 2 of the 10 `api_retry` 401s, the flagged synthetic message, the error result | ~1.3 K |

**Hand-written (not recorded):** the plain logged-out line `Not logged in · Please run /login`
(stdout) and `Invalid API key · Please run /login` (stderr). This host's CLI is credentialed by the
host and could not be made to print its logged-out message; the two lines follow the CLI's
documented wording, and detection matches a small set of phrases (`is_login_line`).

**Live run (this host, real CLI, through the provider):**

```
$ vein_check --provider claudecode --turns 2 --system "You are a terse test responder." "Reply with exactly: Hello from turn one."
vein-check: provider claudecode / model default
vein-check: CLI /opt/node22/bin/claude
── turn 1 ──
Hello from turn one.
[claudecode: session f2e610be-… · resumed: false · model claude-sonnet-5-5 · CLI 2.1.289 · 2193 ms · $0.0086]
── turn 2 ──
Hello from turn one. (turn two)
[claudecode: session f2e610be-… · resumed: true · …]
```

One CLI session across both turns; the second turn's stdin carried only the new question.

## KATs (`cargo test -p gneiss_pal`: 57 pass, 19 new)

claudecode.rs, 18: partial deltas in order + stop/usage/session (turn1); assistant-only text
(nopartial); first-401 fail-fast (authfail); API-error result never shown as reply; status
classification (529/429/400/403/no-status); not-logged-in verbatim (stdout line, stderr, exit codes
3/0/signal); invocation args (tools off, system, model); long system → file; refusals (attachment,
prefill, blank); history flattening; resume vs edited history vs same conversation; fingerprint
boundaries; fake CLI on PATH: streamed text == recorded `result`, argv, stdin, parent session var
unset; two-turn `--resume` with only the new turn on stdin, then a fresh one; missing binary; logged
out (stdout + stderr); refused credential answered in < 10 s against a CLI that would sleep 30 s;
non-zero exit carries code + stderr and drops the session. provider.rs, 1: `claudecode` config, no
key, binary pref, model menu. vein, 1: selection from Principia's file + verbatim missing-binary.
prefs_core rules test extended (chat-model → `default`).

## Honest ceiling

- **Lumen sends one-turn requests today.** Vein's `ProviderSlot::request` builds a single user turn
  and carries the conversation in its system prompt, so from Lumen every message is a FRESH CLI
  session (correct, but no `--resume`). Continuity works for any caller that sends the conversation
  as `ChatRequest.messages` (proven by tests and the live run). Moving Vein to multi-turn requests
  is Vein's change, owed below. The session also resets when the slot is rebuilt on a `vein` pref
  change.
- `max_tokens` and `temperature` are not carried (the CLI has no flags); the plan's defaults apply.
- Attachments are refused (`Unsupported`), not sent.
- `vein.claudecode.bin` is printable ASCII ≤ 4096 (the schema's string kind); a non-ASCII path needs
  the binary on `PATH` instead.
- Each request spawns a Node process (~1–2 s overhead before the first token on this host).
- Host-only: the rMBP/metal has no Node; SELFBUILD is the road there. The kernel's
  `rules::provider_pref` treats `claudecode` as not-claude → echo on metal.

## Owed

1. Vein: send the conversation as multi-turn `ChatRequest.messages` (not folded into the system
   prompt) so Lumen chats ride one CLI session.
2. The Settings dropdown (`quartzite` sidebar) lists `default (claudecode)` from `MODEL_CHOICES`;
   writing `vein.provider` from that selection is the settings surface's existing gap, unchanged.
3. Record the genuine logged-out line on a machine whose CLI is not host-credentialed and replace
   the two hand-written lines.

## Third-party crates

None added. `tokio` (already a dependency) gains features `process`, `io-util`, `rt` — utility.
`serde_json`, `futures-util` already present — utilities. The Claude Code CLI itself is an external
program the person installs; it is the client, not linked code.
