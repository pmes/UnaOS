# PRINCIPIA2 — the policy engine's loose ends (LEDGER SR32; B287, B300, R81)

Branch `exec-host-principia2`, cut from `676ca0e9`. Host-native; the kernel is NOT edited (its
adoption is named below, for the fold).

## Finding

1. **No single schema.** The kernel declared its nine `system.*` keys as bare string constants
   (`kernel/src/prefs.rs` `mod key`) with their ranges scattered across call sites (`settings.rs`
   `int(key::VOLUME, 0, 16)`, `powerui.rs` `peek_int(.., 0, 100)`, `prefs_core::display` for
   brightness). Vein's sixteen `vein.*` keys were declared NOWHERE: they existed as `get("...")` calls
   in `gneiss_pal::api::{provider,embed}` and as doc comments. Settings on the kernel and Principia on
   the host could drift with nothing to catch it (B287).
2. **The host `PrefSet` did not clamp** (the kernel side clamps brightness; B300's owed note in
   `prefs_core::display`). A host write of `system.display.brightness = 0` stored a dark panel.
3. **R81's embedder default was not a rule.** It lived only inside `gneiss_pal::EmbedConfig::from_prefs`
   (and there it differs from R81 as ruled: see Owed).
4. **The byte surface** — the kernel answers PREF_GET/SET/LIST bodies; the host had only the typed
   Synapse enum, so "matches the kernel byte for byte" was unprovable.

## What landed

| M | What | Where |
| :-- | :-- | :-- |
| M1 | `prefs_core::schema::SCHEMA` — 25 rows (9 `system`, 16 `vein`): ns, key, `Kind` (int/float range, bool, string length+printable, enum), `Default` (value / `Rule` / consumer-computed), writers, reader, meaning. `render_markdown` → `docs/dev/PREFS-SCHEMA.md`. Gate `tests/schema_gate.rs`: committed doc == generated; runs `tools/prefs-schema-check.py` (tree scan, exit 1 on an undeclared key) and its `--selftest` (goes red on three planted keys). | `unaos/libs/sys/prefs_core/src/schema.rs`, `tests/schema_gate.rs`, `tools/prefs-schema-check.py`, `docs/dev/PREFS-SCHEMA.md` |
| M2 | `prefs_core::schema::check(ns, key, value) -> Result<Applied{value, clamped}, Refusal>` — the ONE validator. Host `PrefStore::set` runs it and returns `SetOutcome{value, clamped}`; `PrefChanged` carries the stored clamp and `clamped: bool` (serde-omitted when false, so every existing smessage KAT is byte-unchanged; one new KAT freezes the clamped shape). Wrong type / enum miss / over-long / unprintable / NaN → `PrefError`. Undeclared keys pass unchanged. | `schema.rs`, `handlers/principia/src/{prefs,lib}.rs`, `libs/bandy/src/signals.rs` |
| M3 | `prefs_core::rules::{Rule, RuleEnv}` — `Rule::Embedder` (R81: gemini when `vein.gemini.api_key_env`, default `GEMINI_API_KEY`, names a set non-blank variable; else local when `all-MiniLM-L6-v2` is installed — every file of the `tools/una-models` manifest in `${XDG_CACHE_HOME:-$HOME/.cache}/unaos/models/<name>/`; else off) and `Rule::ChatModel` (`vein.provider` default `claude` → `claude-opus-5-5`; gemini → `gemini-3.1-pro-preview`). Host: `PrefStore::effective` / `Principia::effective` = stored → schema default → rule. | `rules.rs`, `handlers/principia/src/prefs.rs` |
| M4 | `prefs_core::wire` — the PREF body codec and `fulfil(&mut dyn Store, verb, body, in_session, out)`, plus the reference `TreeStore` (the kernel's `prefs::set` minus its save). Principia implements `Store` for `PrefStore`; `principia::wire::fulfil`. | `wire.rs`, `handlers/principia/src/wire.rs`, `tests/wire_abi.rs` |

### The SET reply (new)

Unclamped SET: EMPTY body — byte for byte the B300 kernel reply. Clamped SET:
`<stored literal>` NUL `clamped=true` (e.g. `system.display.brightness\x000` → `1\x00clamped=true`).
Schema refusal: -EINVAL (new; such writes were accepted before). Collision / save failure: -EIO (as B300).

## The oracle

There is no external decoder here; the oracles are the contracts already frozen in the tree:

- **the kernel's B300 wire** — the bodies of `prefs::codec_selftest` are re-asserted in
  `wire::tests::kat_the_b300_bodies`; `VERB_*` and `BODY_MAX` are pinned to `una-abi` (`tests/wire_abi.rs`);
- **host vs kernel, byte for byte** — `principia::wire::tests::principia_answers_the_kernel_verbs_byte_for_byte`
  drives one 26-step script (gets, sets in and out of range, refusals, EACCES, collision, lists, a bad verb)
  through prefs_core's `TreeStore` and through Principia's file-backed `PrefStore`: every status and every
  reply byte equal; then the file Principia wrote parses in prefs_core to the reference tree and equals its
  emission byte for byte;
- **Flight 19's dark-panel rule** — `schema::check` on `system.display.brightness` equals
  `display::clamp_brightness` for every x in -40..60;
- **the document** — the gate regenerates it from the table on every run.

## KATs

prefs_core 31 (27 unit: schema 5, rules 6, wire 7, codec 7 pre-existing, display 2 pre-existing; gate 3;
abi 1). principia 23 (4 new: store clamp across every declared int range, handler clamp + refusal, the
embedder rule on a real cache dir, the byte-for-byte cross-test). bandy smessage_kats +1 (clamped
`PrefChanged`). `cargo test -p prefs_core -p principia` green; `cargo test -p bandy` green; prefs_core
checks clean for `aarch64-unknown-none-softfloat` (no_std).

## Keys referenced but undeclared (the scan)

Before this arc, no table declared any key; the scan, run against the new table, finds 25 referenced
keys, all declared (`declared=25 referenced=25 undeclared=0`). The keys that had NO declaration of any
kind before (only `get("...")` calls / doc comments): all 16 `vein.*` keys. Outside the scan, by design:
the `user-prefs` fixture TABLE (BANDY3 R3PREF demo verbs 128/129, not Principia's store) —
`ui.theme`, `ui.font_scale`, `input.pointer_speed`, `power.idle_blank_secs`, `audio.volume`, with no
namespace; its `input.pointer_speed` / `power.idle_blank_secs` already drift from the schema's
`system.pointer.speed` / `system.display.idle_min`.

## Choices a seat may overturn

- `vein.temperature` range `0.0..=2.0` (Gemini's ceiling; Claude's is 1.0 — clamping per provider would
  need a provider-dependent range).
- `vein.max_tokens` `1..=u32::MAX`, `vein.embed.dims` `1..=65536`, `vein.embed.reembed_batch` `1..=100000`,
  `system.dock.pins` ≤256 printable: ceilings chosen here; the consumers had only lower bounds.
- `vein.gemini.auth` enum carries the consumer's aliases (`apikey`, `key`, `adc`, `gcloud_adc`) so no file
  that works today is refused.

## Ceiling / owed

- **Kernel adoption (the fold, kernel not edited here):** `prefs::set` calls `prefs_core::schema::check`
  and stores the clamp; `prefs::bus_fulfil` becomes `prefs_core::wire::fulfil` over a kernel `Store` (its
  `split_addr` / `*_body_parse` are `wire::split_addr` / `parse_*`); `mod key` may reference schema rows.
- **gneiss_pal's embedder default diverges from R81 as encoded:** `EmbedConfig::from_prefs` also
  defaults to Gemini when `vein.gemini.project` is set (gcloud), and never picks `local` by default.
  Owed: it resolves `embed.provider` through `prefs_core::rules::Rule::Embedder` (Peter's word on whether
  a gcloud project counts as "a Gemini key configured").
- A stored out-of-range value already IN a file is not clamped on host load (the kernel clamps
  brightness on load); `effective` answers it raw.
- PrefChanged mailbox delivery on the metal stays BANDY-3.
- No new third-party crate: prefs_core still has zero runtime deps (`una-abi`, in-tree, is a
  dev-dependency only). Principia's `toml` crate (utility, pre-existing) is unchanged.

## Continue

Add a key: one row in `schema.rs`, then `PREFS_SCHEMA_BLESS=1 cargo test -p prefs_core --test schema_gate`
and commit the regenerated `docs/dev/PREFS-SCHEMA.md`. Add a rule: a `Rule` variant, its `describe`, its
`eval`, a branch test per outcome. A new key reference shape the scan does not see: a pattern in
`tools/prefs-schema-check.py` (`refs_in`) plus a planted case in its `--selftest`.
