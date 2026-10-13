# AETHERDEVTOOLS3 (LEDGER SR128) — Sources, thrown non-errors, console shapes, Network detail, box model

Branch `exec-host-aetherdevtools3`, cut from 4f3a436e. Compiled, not run (R107): every shape below is what
the tests in `handlers/aether/tests/devtools3.rs`, `unaos/libs/web/js_core/tests/positions.rs` and
`libs/quartzite/src/platforms/gtk/devtools_view.rs` assert, owed to the fold.
Compile: `CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave30/target cargo check --workspace --tests --features aether-shell/gtk` rc=0.

## 1. A thrown non-error has a position (js_core)

`Vm.last_throw: Option<(Rc<Code>, u32)>` is set at `Op::Throw` (`vm/interp.rs`) to the frame's code and the
throw op's pc; `vm/ops.rs` `error_position` falls back to it when the value has no [[ErrorData]]. The pc's
line-table row is the `throw` keyword (`compiler/mod.rs` records `Gen::pos(sp.start)` before `Op::Throw`).

```js
function f() {
  throw 'x';        // error_position -> t.js:2:3   (before: None, ErrorEvent 0:0, no excerpt)
}
f();
```

Through Aether, an inline `throw 'nope'` on line 3 of its script now prints in the Console:

```
Uncaught nope
  1 | 
  2 | function f() {
> 3 |   throw 'nope';
    |   ^
  4 | }
  5 | f();
```

and the window's ErrorEvent carries `lineno 3`, `colno 3`. A non-error has no frames, so no `at` lines.

Limit: `last_throw` is the LAST throw statement executed. A non-error that did not come from a `throw`
statement (a native rejecting with a string) after an earlier caught `throw` is placed at that earlier
site. Clearing the field at a catch (`interp.rs` unwind) or at script entry is outside this arc's edits.

## 2. Sources panel (`handlers/aether/src/devtools/sources.rs`, new)

Tab strip: `Elements  Console  [Sources]  Network  Storage  Performance  Accessibility` (`Panel::ALL` is 7;
the GTK face's `PANELS` is the same 7 in the same order).

```
Scripts (2)
  1  https://devtools3.test/app/index.html (inline #1)
> 2  https://devtools3.test/app/index.html (inline #2)

https://devtools3.test/app/index.html (inline #2):3
  1 | 
  2 | function boom() {
> 3 |   throw new Error('boom');
  4 | }
  5 | boom();
  6 | 
```

- Scripts: the document's `<script>` elements of a JavaScript type, tree order (a `type=application/json`
  data block is not listed). External: the absolute URL, source from `js::ran_source` (the goto hook in
  `js/mod.rs`: PageState.ran_sources, then the loader's pre-fetched script_sources). Inline: `<doc url>
  (inline #n)`, source = the element's text.
- Source view: `{mark} {n:>w} | {text}`, current line marked `>`, at most 400 lines centred on it.
- A Console frame line (`    at boom (url:3:9)`, `    at url:5:1`, `    at new C (url:l:c)`) is a target:
  `sources::frame_target` parses it, `DevTools::goto(engine, url, line)` / `DevTools::goto_line(engine,
  text)` select the script and line and show Sources. A URL with no script changes nothing (false).
- Inline scripts all carry the document URL as their frames' file (JSPOS), so `resolve` picks the inline
  script the engine ran last under that name when it covers the line, else the first that covers it.

## 3. Console methods (`handlers/aether/src/js/window.rs` `console_method`)

Before (4f3a436e): `group`, `groupCollapsed`, `groupEnd`, `time`, `timeEnd`, `timeLog`, `count`,
`countReset` were `console_noop`; `table` printed as `log`; `assert`, `trace` existed. After:

| call | console line (level) |
|---|---|
| `console.group('outer')` | `outer` (log), then every line indented two spaces per open group |
| `console.group()` | `console.group` |
| `console.groupEnd()` | nothing; depth - 1, never below 0 |
| `console.count()` x2, `count('x')` | `default: 1`, `default: 2`, `x: 1` (info) |
| `console.countReset('nope')` | `Count for 'nope' does not exist` (warn) |
| `console.time('t')` twice | second: `Timer 't' already exists` (warn) |
| `console.timeLog('t', 'mid')` | `t: 0.012 ms mid` (log; ms to three decimals) |
| `console.timeEnd('t')` | `t: 0.034 ms`; again: `Timer 't' does not exist` (warn) |
| `console.table('plain')` | `plain` (as `log`) |

`console.table([{a: 1, b: 'y'}, {a: 2}])`:

```
┌─────────┬───┬─────┐
│ (index) │ a │ b   │
├─────────┼───┼─────┤
│ 0       │ 1 │ 'y' │
│ 1       │ 2 │     │
└─────────┴───┴─────┘
```

`console.table([5, 6])` has one `Values` column. A second argument (an array of names) limits the columns.
The group depth, counters and timers reset with the document (a new page URL, or a page console shorter
than at the last call). `clear`, `profile`, `profileEnd`, `timeStamp` stay no-ops; `assert` is not indented
(it is `console_assert`, outside `console_method`).

Scope note: routing the eight names to `console_method` is the console registration list in
`window.rs` (`for name in [...]`, two lines), not `console_method`'s body; without it the brief's goal is
unreachable. Nothing else in `window.rs` changed.

## 4. Network row detail (`handlers/aether/src/devtools/network.rs`)

A row the HTTP cache recorded gains `response_headers` (the stored fields), `request_headers` (only the
fields `Vary` named: the cache keeps no others) and `preview`:

```
GET     200    fetch/xhr  application/json                 9       0  https://devtools3.test/api/data.json
    Response headers (2)
      content-type: application/json
      vary: accept-language
    Request headers (1, the ones Vary named)
      accept-language: en
    Preview (JSON, 3 lines)
      {
        "n": 1
      }
```

Preview kinds: JSON pretty-printed (two spaces; `application/json`, `text/json`, `*+json`, or an untyped /
`text/plain` body that is an object or array), text as is (64 KiB at most), an image as `PNG image, 16 x 8`
(PNG, GIF, JPEG SOFn, WebP VP8/VP8L/VP8X, BMP — read from the header, whatever the type says), else
`N bytes, binary`; at most 12 preview lines in the pane. A row with nothing recorded: `headers and body:
not recorded`, and the pane prints no block under it. Script requests (`fetch`, XHR) carry none: the fetch
worker's log (`api/fetch.rs` `ScriptRequest`) keeps no fields or body (outside this arc's files).

## 5. Elements box model (`handlers/aether/src/devtools/elements.rs`)

`elements::box_model(engine, id)` reads taffy's `Layout.margin` / `border` / `padding` and the border-box
`size` of the element's first box in tree order; content = size - border - padding. For
`#b { margin: 10px; border: 2px solid; padding: 5px 6px; width: 100px; height: 40px }` the drawing is
(the right margin printed as layout resolves it; shown here as 10):

```
+----------------------------------------+
| margin            10                   |
|    +------------------------------+    |
|    | border       2               |    |
|    |   +----------------------+   |    |
|    |   | padding  5           |   |    |
|    |   |     +----------+     |   |    |
| 10 | 2 |   6 | 100 x 40 | 6   | 2 | 10 |
|    |   |     +----------+     |   |    |
|    |   |          5           |   |    |
|    |   +----------------------+   |    |
|    |              2               |    |
|    +------------------------------+    |
|                   10                   |
+----------------------------------------+
```

Fifteen rows of equal width; the test asserts the row count, the widths, the labels on rows 1/3/5 and the
content row's cells `10 | 2 | 6 | 100 x 40 | 6 | 2`. A zero edge reads `-`. The pane draws it under the
inspection's `box` line when a node is selected.

## 6. Stopped: the GTK frame click needs a bus verb (files outside the brief)

`devtools_view.rs` has the Sources tab and `goto_target(line) -> Option<(url, line)>` (tested), but the
click cannot reach `DevTools::goto`: quartzite does not link the engine and bandy has no verb for it. The
exact change, for the fold or a follow-up arc:

1. `libs/bandy/src/signals.rs`, after `BrowserDevtoolsEval(String)`:
   `/// Chrome → engine: follow a Console frame (DevTools::goto).` `BrowserDevtoolsGoto { url: String, line: u32 },`
   and a KAT in `libs/bandy/tests/smessage_kats.rs` beside `kat_browser_devtools_eval`, plus its arm in the
   name table at `:1424`.
2. `vessels/aether-shell/src/main.rs`, beside `SMessage::BrowserDevtoolsEval(line)` (`:534`):
   `SMessage::BrowserDevtoolsGoto { url, line } => { let _ = self.devtools.goto(&self.engine, &url, line); }`
   and `| SMessage::BrowserDevtoolsGoto { .. }` in the list at `:871`.
3. `devtools_view.rs`: a `gtk4::GestureClick` on `text` whose `released` handler maps (x, y) through
   `text.window_to_buffer_coords(TextWindowType::Widget, ..)` and `text.iter_at_location`, takes that
   line's text, and fires `SMessage::BrowserDevtoolsGoto { url, line }` when `goto_target` reads one.

## Tests (written, not run — owed to the fold)

- `handlers/aether/tests/devtools3.rs`: `sources_panel_lists_scripts_and_follows_a_console_frame`,
  `a_thrown_non_error_gets_its_position_and_excerpt`, `console_groups_counts_timers_and_tables`,
  `network_rows_carry_headers_and_a_body_preview`, `elements_panel_draws_the_selected_nodes_box_model`.
- `unaos/libs/web/js_core/tests/positions.rs`: `a_thrown_non_error_is_placed_at_its_throw_statement`.
- `libs/quartzite/src/platforms/gtk/devtools_view.rs` (unit): `sources_tab_and_frame_targets`.
