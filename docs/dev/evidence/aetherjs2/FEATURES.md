# AETHERJS2 (LEDGER SR106): the 2026 JavaScript surface: feature table

Branch `exec-host-aetherjs2`, cut from `98d9f842`. Compiled with `cargo check -p js_core -p aether --tests`
(rc=0 at every commit). Under R107 nothing here was run: every row's test is written and owed to the fold.
The **Test** column names the `#[test]` that asserts the feature's observable, in
`unaos/libs/web/js_core/tests/modern.rs` (JS) or `handlers/aether/tests/js2026.rs` (AE).

## Added by this arc

| Feature | Spec | Where | Test |
|---|---|---|---|
| `Intl` namespace: `getCanonicalLocales`, `supportedValuesOf`, `supportedLocalesOf` on every service, UTS #35 tag canonicalization, lookup ResolveLocale over en / en-GB / en-US (others resolve to en-US and say so in `resolvedOptions().locale`) | ECMA-402 §6.2, §8.3, §9.2 | `js_core/src/builtins/intl/mod.rs`, `locale.rs` | JS `intl_list_relative_segmenter_locale` |
| `Intl.NumberFormat`: decimal / percent / currency (48 ISO codes, symbol / narrowSymbol / code / name, accounting) / unit (45 sanctioned units + X-per-Y), standard / scientific / engineering / compact, the digit options with roundingPriority, all nine roundingModes, roundingIncrement, trailingZeroDisplay, useGrouping always / auto / min2 / false, five signDisplays, BigInt and exact numeric-string input, `formatToParts`, `formatRange(ToParts)` | ECMA-402 §15 (NumberFormat v3) | `intl/number.rs`, `intl/data.rs` | JS `intl_number_format` |
| `Number` / `BigInt` `toLocaleString` through NumberFormat | ECMA-402 §19.2.1, §19.3.1 | `builtins/number.rs`, `bigint.rs` | JS `intl_number_format` |
| `Intl.DateTimeFormat`: components and dateStyle / timeStyle, hour12 / hourCycle / `-u-hc-`, fractionalSecondDigits, dayPeriod, era, six timeZoneName styles, 84 IANA zones (current rule), offset zones, the UTC aliases, `formatToParts`, `formatRange(ToParts)`; CLDR 42+ spacing (U+202F before AM/PM, " at ", U+2009 range dash) | ECMA-402 §11 | `intl/datetime.rs`, `intl/data.rs` | JS `intl_date_time_format` |
| `Date.prototype.toLocaleString` / `toLocaleDateString` / `toLocaleTimeString` (the required / defaults table; "Invalid Date") | ECMA-402 §20.4.1–3 | `builtins/date.rs` | JS `intl_date_time_format` |
| `Intl.Collator` (three-level UCA-shaped key over NFD; sensitivity, numeric, caseFirst, ignorePunctuation) and `String.prototype.localeCompare` through it (canonical equivalence still 0) | ECMA-402 §10, §19.1.1 | `intl/collator.rs`, `builtins/string.rs` | JS `intl_collator` |
| `Intl.PluralRules` (en cardinal + ordinal, digit options, selectRange) | ECMA-402 §16 | `intl/plural.rs` | JS `intl_plural_rules` |
| `Intl.ListFormat` (conjunction / disjunction / unit; en vs en-GB serial comma) | ECMA-402 §13 | `intl/list.rs` | JS `intl_list_relative_segmenter_locale` |
| `Intl.RelativeTimeFormat` (long / short / narrow, numeric always / auto) | ECMA-402 §17 | `intl/relative.rs` | JS `intl_list_relative_segmenter_locale` |
| `Intl.Segmenter` (UAX #29 graphemes incl. ZWJ / emoji modifiers / RI pairs / Hangul, words with isWordLike, sentences; `containing`, iteration) | ECMA-402 §18 | `intl/segmenter.rs` | JS `intl_list_relative_segmenter_locale` |
| `Intl.Locale` (options, maximize / minimize, getters, getHourCycles / getCalendars / getTextInfo) | ECMA-402 §14 | `intl/locale.rs` | JS `intl_list_relative_segmenter_locale` |
| `Intl.DurationFormat` (long / short / narrow / digital, per-unit styles and displays, fractional sub-seconds) | ECMA-402 2025 (DurationFormat) | `intl/duration.rs` | JS `intl_duration_format_and_display_names` |
| `Intl.DisplayNames` (86 languages + 16 dialect names, 64 regions, 14 scripts, currencies, 8 calendars, 12 date-time fields) | ECMA-402 §12 | `intl/displaynames.rs` | JS `intl_duration_format_and_display_names` |
| `Array.fromAsync` (async / sync / array-like inputs, mapper, iterator closing) | ES2026 Array.fromAsync | `builtins/es2026.rs` | JS `es2026_builtins` |
| `Error.isError` | ES2026 Error.isError | `builtins/es2026.rs` | JS `es2026_builtins` |
| `Math.sumPrecise` (exact sum, −0 for empty, NaN / ±∞ states) | ES2026 Math.sumPrecise | `builtins/es2026.rs` | JS `es2026_builtins` |
| `Uint8Array.fromBase64` / `fromHex`, `.toBase64` / `.toHex` / `.setFromBase64` / `.setFromHex` | ES2026 (base64 proposal) | `builtins/es2026.rs` | JS `es2026_builtins` |
| `Iterator.concat` | ES2026 Iterator Sequencing | `builtins/iterator.rs` | JS `es2026_builtins` |
| `Map` / `WeakMap` `getOrInsert` / `getOrInsertComputed` | upsert (stage 4) | `builtins/es2026.rs` | JS `es2026_builtins` |
| `Error.captureStackTrace` (de-facto; libraries probe for it) | not in ECMA-262 | `builtins/es2026.rs` | JS `es2026_builtins` |
| Explicit resource management, library half: `Symbol.dispose` / `asyncDispose`, `SuppressedError`, `DisposableStack`, `AsyncDisposableStack`, iterator disposal | ES2026 explicit resource management | `builtins/es2026.rs`, `builtins/symbol.rs`, `vm/heap.rs` | JS `explicit_resource_management_library` |
| `import.meta.resolve` (host hook `resolve_module_specifier`) | HTML §8.1.5.4 HostGetImportMetaProperties | `vm/module.rs`, `aether/src/js/host.rs` | JS `modules_tla_dynamic_import_meta`, AE `import_maps_resolve_every_module_path` |
| Import maps: `<script type="importmap">` (parse, merge, imports + scopes + integrity), every specifier through "resolve a module specifier" | HTML §8.1.5.3, §8.1.5.5–6, §4.12.1.1 | `aether/src/js/importmap.rs`, `loader.rs`, `host.rs` | AE `import_maps_resolve_every_module_path` |
| `unhandledrejection` / `rejectionhandled` (PromiseRejectionEvent tasks; console only when not canceled) | HTML §8.1.5.7 | `aether/src/js/mod.rs`, `host.rs` | AE `event_loop_rejection_notification_tasks` |
| Animation frames: a callback canceled by an earlier one of the same frame is skipped; one timestamp per frame; microtasks between callbacks | HTML §8.1.7.3 "run the animation frame callbacks" | `aether/src/js/window.rs` | AE `event_loop_animation_frame_rules` |
| `scheduler.postTask` / `scheduler.yield`, `TaskController` / `TaskSignal` | WICG Prioritized Task Scheduling | `aether/src/js/window.rs` | AE `event_loop_scheduler_priorities` |
| Inline event handler scope chain (element, form owner, document as object environments) | HTML §8.1.8.1 | `aether/src/js/events.rs` | AE `inline_handler_scope_chain_and_unscopables` |
| `@@unscopables` on Element, Document, DocumentFragment, CharacterData, DocumentType | WebIDL §3.7.3; DOM §4.2.6, §4.2.8 | `aether/src/js/dom.rs` | AE `inline_handler_scope_chain_and_unscopables` |
| `structuredClone(value, { transfer })` (ArrayBuffers detached), Float16Array serializable | HTML §2.7.11, §2.7.4 | `aether/src/js/broadcast.rs` | AE `structured_clone_and_abort_signals` |

## Present before this arc, now held by a test

| Feature | Spec | Test |
|---|---|---|
| ES modules: static graph, dynamic `import()`, top-level await | ECMA-262 §16.2 | JS `modules_tla_dynamic_import_meta` |
| `<script type=module>` / defer / async order | HTML §4.12.1 (SR63) | AE `import_maps_resolve_every_module_path` |
| `for await`, async generators (and the async-from-sync job order) | ECMA-262 §14.7.5, §27.6 | JS `es2025_language_surface`, `es2026_builtins` |
| Class fields, static blocks, private methods, `#x in o` | ECMA-262 §15.7 | JS `es2025_language_surface` |
| `WeakRef`, `FinalizationRegistry` | ECMA-262 §26.1–2 | JS `es2025_language_surface` |
| `Array.prototype.{at, findLast, toSorted, toReversed, with}` | ECMA-262 §23.1.3 | JS `es2025_language_surface` |
| `Object.groupBy` | ECMA-262 §20.1.2.11 | JS `es2025_language_surface` |
| RegExp `v` flag, named groups, duplicate named groups | ECMA-262 §22.2 | JS `es2025_language_surface` |
| Set methods, `Promise.withResolvers`, `Promise.try` | ES2024–2025 | JS `es2025_language_surface` |
| `structuredClone` graph / types / DataCloneError | HTML §2.7 (SR94) | AE `structured_clone_and_abort_signals` |
| `AbortController` / `AbortSignal` through `fetch`, `any`, `timeout` | DOM §3.1–3.2; Fetch "fetch method" abort steps (SR70) | AE `structured_clone_and_abort_signals` |
| Microtasks before the next task; a checkpoint between scripts; timers as tasks in due order | HTML §8.1.7.3, §8.6 | AE `event_loop_microtasks_before_the_next_task`, `event_loop_checkpoint_between_scripts`, `event_loop_timers_are_tasks_in_due_order` |

## Still refused (owed)

| Feature | Spec | Why / where it would go |
|---|---|---|
| `using` / `await using` declarations | ES2026 explicit resource management (UsingDeclaration, AwaitUsingDeclaration) | parser (contextual `using`), scope analysis and compiler disposal at every block exit; the library it calls is in. A compiler change cannot be run here (R107), so it was not started. |
| `Temporal` | TC39 Temporal | a new library the size of Intl. |
| Locale data beyond en / en-GB / en-US; numbering systems other than `latn`; non-Gregorian calendars; collation tailorings | ECMA-402 §9.1, §11, §15, §10 | CLDR data in tree (R83: no ICU). |
| Historical time zone transitions (each zone formats with its current rule) | ECMA-262 §21.4.1 (LocalTZA); ECMA-402 §11 | the tz database as data (SR55 / SR63 owed). |
| `formatRange` sharing fields ("Jan 15 – 16, 2024") | ECMA-402 §11.5.9 PartitionDateTimeRangePattern | both ends are written in full with the U+2009 dash. |
| Inline caches / shapes | SR63 owed (performance) | no performance arc. |
| Exact `innerText` | HTML §3.2.7 (SR63 owed) | layout territory (AETHERSTYLE3 is live there). |
| `MessageChannel` / `MessagePort` as a real task source; `requestIdleCallback` deadlines | HTML §9.4, Idle Callbacks | both are `setTimeout` shims in `handlers/aether/src/api/platform.rs`, outside this arc's files. |
| External import maps (`<script type="importmap" src>`) | HTML §4.12.1.1 step 27 | refused by the standard too: the element fires `error`, as here. |
