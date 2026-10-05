# VEINTURNS — Vein sends the conversation as real turns (LEDGER SR42)

Branch `exec-host-veinturns`, cut from `d93d3eb4`, CLAUDECODE (`exec-host-claudecode` @a36f4aef) merged
first. Host-only (Ring 3); the kernel is not edited.

## Finding

CLAUDECODE (SR38) gave Vein a provider that keeps one Claude Code CLI session across turns — but only for
a caller that sends the conversation as `ChatRequest.messages`. Vein did not: `ProviderSlot::request`
built `ChatRequest::single(system, parts)`, ONE user turn, and whatever Vein knew of the past rode inside
the system prompt (the `[HISTORICAL & SHORT-TERM ENGRAMS]` block — compressed summaries, not turns). So
from Lumen every message was a fresh CLI session (and for the HTTP APIs, a conversation with no turns at
all). A second, hidden defect: the engram compressor called the SAME provider instance after each reply;
on the CLI provider that one-shot request would have replaced the remembered session, so even a
multi-turn Vein would have resumed nothing.

## What landed

| M | What | Where |
| :-- | :-- | :-- |
| M1 | `gneiss_pal::api::Thread`: the conversation as `(role, parts)` messages. `request(system, user)` = the system prompt ALONE in `system`, every earlier turn its own message, the new user turn last; `record(user, reply)` stores the user turn as sent and the reply text byte for byte (so the CLI provider's fingerprint recognises its own conversation); bounded at 64 messages, trimmed to 32 in ONE step starting on a user turn (the window stays stable, a session-keeping provider re-anchors once, not every turn). `ModelProvider` gains `session() -> Option<SessionInfo {id, resumed}>` and `reset_session()` (defaults: stateless no-ops; the CLI provider answers from its last run). Vein: `ProviderSlot::turn(system, &Thread, parts)` replaces the one-turn `request` (the fold-into-system shortcut is gone); the brain loop keeps a `Thread`, records only a completed non-blank reply (no provider accepts an empty assistant turn; a failed turn is never recorded); the slot builds a SECOND provider instance (`side_provider()`) for engram compression so a side call never displaces the conversation's session. `vein_check --turns N` builds its turns with the same `Thread` and ends with `session: one id across N turns` (exit 1 if the id changed). | `libs/gneiss_pal/src/api/{thread,provider,claudecode,mod}.rs`, `libs/gneiss_pal/examples/vein_check.rs`, `handlers/vein/src/{provider,lib}.rs` |
| M2 | Session continuity in Lumen: `/new` clears the thread and calls `reset_session()` (next message: a fresh CLI session); `/undo` drops the last exchange — an edit is `/undo` + the corrected message, and an edited history never matches the CLI provider's fingerprint, so it is a fresh session with the history flattened into its preamble. A provider rebuilt on a `vein` pref change holds no session (next turn fresh). `AppState.session_status` (`session=resumed\|new\|stateless · turns=<n>`) is set after every turn and on `/new`/`/undo`; the GTK status row appends it to the token line. | `handlers/vein/src/lib.rs`, `libs/bandy/src/state.rs`, `libs/quartzite/src/platforms/gtk/workspace/{types,translator,reactor}.rs` |
| M3 | The Settings provider dropdown WRITES: on selection it fires `PrefSet vein.provider` and `PrefSet vein.model` (`menu_choice_prefs` maps a `model_menu` label back to both, so a provider never runs with another provider's model id) onto the UI channel; Vein forwards a `Principia(PrefSet)` from the UI channel to the Synapse; Lumen now runs `principia::serve` on its Synapse, which validates against the schema, persists and broadcasts `PrefChanged`; Vein rebuilds its slot and prints its status line. The label under the dropdown shows the chosen provider's status line, built exactly as Vein builds it (`probe_provider` → `ONLINE (claudecode / default)` or `NO PROVIDER :: <fix>`); the console line and the label share `provider_status`. | `libs/gneiss_pal/src/api/provider.rs`, `handlers/vein/src/lib.rs`, `vessels/lumen/{Cargo.toml,src/main.rs}`, `libs/quartzite/src/platforms/gtk/workspace/sidebar.rs` |
| M4 | This doc. | `docs/dev/evidence/host-1005/VEINTURNS.md` |

## Proof

**Live (this host, real CLI 2.1.289, logged in, subscription-billed, no API key):**

```
$ cargo run -p gneiss_pal --example vein_check -- --provider claudecode --turns 3 \
      --system "You are a terse test responder." "Reply with exactly: Hello from turn one."
vein-check: provider claudecode / model default
vein-check: CLI /opt/node22/bin/claude
── turn 1 (1 messages) ──
Hello from turn one.
[claudecode: session e1041783-3213-40e3-b652-02320a14f5b0 · resumed: false · model claude-sonnet-5-5 · CLI 2.1.289 · 1796 ms · $0.0086]
── turn 2 (3 messages) ──
Hello from turn one. (turn 2)
[claudecode: session e1041783-3213-40e3-b652-02320a14f5b0 · resumed: true · … · 2075 ms · $0.0089]
── turn 3 (5 messages) ──
Hello from turn one. (turn 2) (turn 3)
[claudecode: session e1041783-3213-40e3-b652-02320a14f5b0 · resumed: true · … · 1950 ms · $0.0092]
session: one id across 3 turns: e1041783-3213-40e3-b652-02320a14f5b0
```

Three turns, one session id; the reply on turn 3 carries both earlier turns, i.e. the CLI's own session
remembered them while its stdin held only the new question.

**KATs**

gneiss_pal (64 pass, 7 new): `thread::requests_carry_every_turn_with_its_role_and_the_system_alone`,
`thread::undo_drops_the_last_exchange_and_clear_drops_all`, `thread::the_trim_is_one_step_and_starts_on_a_user_turn`;
`claude_thread_is_n_messages_not_one` (a mock Messages API receives FIVE messages user/assistant/user/assistant/user
and `system` alone); `gemini_thread_is_n_contents` (user/model/user + `systemInstruction`);
`claudecode::a_thread_rides_one_cli_session_and_an_edit_or_new_starts_fresh` (fake CLI replaying the recorded
transcripts: turns 2 and 3 are `--resume 00000000-…cc01` with ONLY the new turn on stdin and `--system-prompt`
still passed, one id across three turns; `undo` + resend is a fresh session with a `<conversation-history>`
preamble; `reset_session` → `session()` is `None` and the next run is fresh);
`provider::menu_labels_are_pref_writes_and_the_status_line_is_shared`.

vein (new): `vein_turns_ride_one_cli_session_and_new_starts_fresh` (Principia's file selects claudecode with
a fake binary; `ProviderSlot::turn` over a `Thread`; status row `session=new · turns=1` then
`session=resumed · turns=2`; a side-instance call between turns does NOT break the resume; `/new` →
`session=new`); `api_providers_send_the_thread_and_read_stateless`; `settings_dropdown_writes_go_through_principia`
(the dropdown's two PrefSets through `Principia::process_impulse` answer `PrefChanged vein.*`, the slot rebuilt from
the file reads `ONLINE (claudecode / default)`, equal to the dropdown's `probe_provider` line).

## Honest ceiling

- **The GTK workspace is not compiled today.** `libs/quartzite/src/platforms/gtk/mod.rs` has
  `// pub mod workspace;` — the sidebar (with the provider dropdown), the translator and the reactor that
  draw the status row are dead code on Linux, and the macOS backend has no settings dropdown or token row.
  The M2/M3 UI edits are written against the live types (`AppState.session_status`, `GuiUpdate`, the
  dropdown) and the logic under them is tested (`menu_choice_prefs`, `probe_provider`, the PrefSet path,
  `session_status`), but no pixel shows them until a backend compiles that surface. The console lines
  (`:: BRAIN :: ONLINE (…)`, `:: BRAIN :: NEW CONVERSATION`) are live.
- **Lumen on Peter's Mac** chatting through the Max login was not exercised (no Mac here); the host proof is
  `vein_check` + the vein slot tests.
- Recall context (directives, semantic memory, engrams) still rides the system prompt by design — it is
  memory, not turns; the short-term engrams can now repeat what the thread already carries.
- The thread lives in Vein's memory: a Lumen restart starts a new conversation (the vault's stored chat is
  not replayed into turns).
- Attachments: the CLI provider refuses them (unchanged); the turn is not recorded, so it never poisons the thread.
- Changing the system prompt between resumed turns relies on the CLI honouring `--system-prompt` with `--resume`
  (it accepted it on all three live turns).

## Owed

1. Compile a host settings surface (re-enable `gtk::workspace` or give the macOS backend a settings sheet) and
   show the dropdown + status row on screen.
2. Lumen on the Mac through the Max login (Peter's eyes).
3. Optionally seed the thread from the vault's last N chat records on start.

## Third-party crates

None added. Lumen gains a path dependency on `principia` (in-tree).
