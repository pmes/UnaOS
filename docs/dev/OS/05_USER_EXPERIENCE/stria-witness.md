# STRIA-WITNESS — the headless witness's one JSON line

LEDGER SR1118. Code: `vessels/stria-view/src/main.rs` (`witness`, the one writer of the line, printing through
`witness_out`; the pieces `transition_json`, `meters_json`, `help_json`, `presses_json`, `blits_json`, `export_json`,
`range_json`, `reel_json`; the key lists `WITNESS_KEYS` and `EXPORT_JSON_KEYS`). Guard: stria-view's test
`nlefaceexportjsondocos_witness_keys` walks the default line's top-level keys against `WITNESS_KEYS` and a held export's
object against `EXPORT_JSON_KEYS`; `nlefacewitnessscriptwordsonetable_optional_keys` arms all nine conditional keys in one run against `WITNESS_OPTIONAL_KEYS`: a new key changes the const, this page and the test in one commit.

## 1. Running it
`stria-view` runs headless when given `--do`, `--out` or `--two-surfaces` (or built without a window backend): it drives
`stria::nle_face::Face` with the messages a window would send, prints ONE line on stdout and exits 0 (1: no frame drawn, or
`--out` not writable).

```text
stria-view --synth --size 320x160 --do at:12:0,split,delete,undo [--out shot.rgba] [--labels]
stria-view --synth (--two-surfaces | --split) --do <script>
stria-view <edl> [--fps N] --src REEL=<media> ... --do <script> [--mute]
```

## 2. The top-level keys in emitted order: the first sixteen on every line (`WITNESS_KEYS`), the rest only as named (`WITNESS_OPTIONAL_KEYS`)
| key | type | when | added by |
|---|---|---|---|
| `playhead` | integer (tick) | always | NLE2 SR201 |
| `tracks` / `clips` | integer | always | NLE2 SR201 |
| `selection` | `"<track>:<idx>"` or null | always | NLE2 SR201 |
| `clip` | `"<at>:<src_in>:<src_out>"` or null | always | NLE2B SR208 |
| `playing` / `rate` | bool / integer | always | NLE2B SR208 |
| `undo` / `zoom` | integer | always | NLE2 SR201 |
| `w` / `h` / `fnv` | integer / integer / 16 hex digits | always (the last blit; with `--split` the tracks blit's) | NLE2 SR201 |
| `errors` | string (`; `-joined, `"` written `'`) | always | NLE2 SR201 |
| `set` | integer (multi-select size) | always | NLE2C SR217 |
| `transition` | object (`track`, `idx`, `at`, `dur`, `kind`, `ease`, `from_black`, `still`) or null | always | NLE3D SR302 (`ease` SR691) |
| `dirty` | bool | always | NLEHOSTSAVE SR677 |
| `meters` | array of `{"track":<n>\|"master","peak":<dBFS>,"rms":<dBFS>}` | the script has `meters` | NLE4C SR301 |
| `help` | `true` | the keys help shows | NLE5DKEYSHELP SR744 |
| `presses` | array of `[<surface>,<taken>]` | `--two-surfaces` or `--split` | SURFACEID SR332 |
| `surfaces` / `fnvs` | integer / array of hex strings | `--split` | NLE2-SPLIT SR339 |
| `export` | object (section 3) | an export job is on the face | NLEFACEEXPORT SR694 |
| `range` | `[<in>,<out>]` | both range marks set | NLE5DEXPORTRANGE SR717 |
| `reel` | string (the reel line) | a clip selected or a dwelt hover | STRIAINFOREEL SR743 |
| `audio` | `"none"` | `--mute` | NLEPLAYAUDIO SR676 |

## 3. The `export` object (`EXPORT_JSON_KEYS`)
`done`, `total` (frames, integer) and `audio` (the audio word) are always there; `pass` and `passes` together only for a
job with more than one pass (SR1097); `eta_secs` once the face has an eta, from the first frame on (SR796); `stalled_secs`
past the stall rule (SR825). Before the first frame the object is exactly `{"done":0,"total":N,"audio":"wav"}` (SR1110).

## 4. The `--do` script (`SCRIPT_WORDS`)
Comma-separated tokens: the transport verbs (`stria::nle_face::VERBS`), and these, regenerated from stria-view's
`SCRIPT_WORDS` (a word, or a prefix ending `:` that takes arguments):

- `at:<tick>:<track>`: a click on that track at that tick
- `ruler:<tick>`: a click on the ruler
- `press:<x>:<y>`: a button press at surface pixels
- `move:<x>:<y>`: the pointer moves there
- `release`: the button up and the click, at the last pointer
- `key:<name>`: the key as GTK sends it (key:space; key:Ctrl+a / key:Meta+a select all)
- `wheel:<n>`: n notches at the last pointer (n > 0 zooms in)
- `marquee:<x0>:<y0>:<x1>:<y1>`: a press, a move and the release
- `shift:<token>`: the token with MOD_SHIFT on its presses (add to the set)
- `alt:<token>`: the token with MOD_ALT on its presses (no snapping)
- `transition:<track>:<idx>`: a click on that transition (selects it)
- `kind:<code>`: the selected transition's kind (D, W007 ...)
- `ease:<name>`: the selected transition's ease (linear, in, out, inout)
- `meters`: feeds the meters the tick at the playhead (the line gains meters)
- `tick:<n>`: the face clock n times (playback steps, the hover dwell)
- `aux:<token>`: the token on the other Surface (--two-surfaces: aux; --split: preview)

A token it cannot read adds `bad token <tok>` to `errors`. Guard: `nlefacewitnessscriptwordsonetable_words_parse`
(every word drives with an example, `bogus` is refused, every word is in USAGE).

## 5. Example
The default line (`stria-view --synth --size 320x160 --do ''`; `fnv` is that synth face's digest at 320x160):

```text
{"playhead":0,"tracks":2,"clips":6,"selection":null,"clip":null,"playing":false,"rate":1,"undo":0,"zoom":8,"w":320,"h":160,"fnv":"6bc0558370511857","errors":"","set":0,"transition":null,"dirty":false}
```
