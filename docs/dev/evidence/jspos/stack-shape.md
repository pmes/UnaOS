# JSPOS (LEDGER SR127) — the stack shape, before and after

Written at the JSPOS commit (parent 12393dd6). Compiled, not run (R107): every "after" below is what
the tests in `unaos/libs/web/js_core/tests/positions.rs` and `handlers/aether/tests/positions.rs`
assert, owed to the fold.

## The program

```js
function inner() {
  throw new Error('deep');
}
function outer() {
  return inner();
}
outer();
```

run as `vm.run_script_str_at(src, "t.js")` (aether: `js::run_classic_no_checkpoint`, file = the running
`<script src>`'s absolute URL, else the document URL).

## `e.stack`

Before (12393dd6, `vm/mod.rs:407` `stack_trace` names only; `builtins/error.rs:155` getter returned the
frames with no header):

```
    at inner
    at outer
    at <anonymous>
```

After (`vm/ops.rs` `stack_trace_at`, `builtins/error.rs` `stack_get` adds the header):

```
Error: deep
    at inner (t.js:2:9)
    at outer (t.js:5:10)
    at t.js:7:1
```

Frame forms (V8's): `    at name (file:line:col)`; `    at new Name (file:line:col)` for a construct
call; `    at file:line:col` for unnamed code (top-level script, anonymous functions); `<anonymous>` as
the file for code with no name (eval, `Function(...)`, `run_script` without a file); `    at name (file)`
for a frame whose pc has no line-table row. Columns are 1-based UTF-16 units, as V8's.

## SyntaxError

Before: `run_script` threw `SyntaxError: <msg>` with no position (`vm/mod.rs:917`).
After (`run_script_at`): the same message; its stack is `SyntaxError: <msg>\n    at t.js:3:9`, the
offending token's line and column, and `vm.error_position` returns it.

## Internal errors — owed to the fold

TypeError / ReferenceError / RangeError raised by the engine (`null.x`, an unbound name) are made by
`vm/mod.rs:396` `make_error`, which calls `vm/mod.rs:407` `stack_trace` (outside JSPOS's files). Until
that body becomes `self.stack_trace_at()` they carry the header and the old frames:

```
TypeError: Cannot read properties of null
    at inner
    at outer
    at <anonymous>
```

## Devtools Console, uncaught error in an inline script

```
Uncaught Error: boom
    at boom (https://positions.test/index.html:3:9)
    at https://positions.test/index.html:5:1
  1 | 
  2 | function boom() {
> 3 |   throw new Error('boom');
    |         ^
  4 | }
  5 | boom();
```

`js::console_lines()` and `js::reported_errors()` keep the one-line form (`Uncaught Error: boom`,
`Error: boom`); the frames and excerpt are `js::console_lines_detailed()`, which the panel reads.
Inline-script lines count from the script element's text, not from the document (the HTML parser keeps
no source line per node).

## Line table cost

One `LineTable` per function: `bytes: Vec<u8>` built once in `Gen::finish` from the compiler's (pc,
offset) rows, plus one `Rc<str>` file name shared by the script's functions. A row is
`uvarint(pc delta) zigzag(line delta) uvarint(col)`, written only when (line, col) changes — 3 bytes for
most rows. Lookup is a linear decode of that one function's table, run when an error is made. No knob.
