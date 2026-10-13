# INTLCLDR (LEDGER SR137) — CLDR 46 and tzdata 2025b as data for js_core's Intl, Date and Temporal

R83: no ICU. CLDR and the IANA time zone database are data: two generators turn them into compact tables that
are committed; the fetches are generation-time steps, never tests. Branch `exec-host-intlcldr`, arc base
`2a436e4a`. Written and compiled (`cargo check -p js_core --tests` rc=0 at every commit); no test was run
(R107) — the fold runs them.

## Regenerating

```
python3 -I unaos/libs/web/js_core/tools/gen_cldr.py fetch <dir>   # unicode-org/cldr-json 46.0.0 + unicode-org/cldr release-46
python3 -I unaos/libs/web/js_core/tools/gen_cldr.py gen <dir>     # src/cldr/tables.rs, src/cldr/collation.rs
python3 -I unaos/libs/web/js_core/tools/gen_tz.py fetch <dir>     # eggert/tz 2025b
python3 -I unaos/libs/web/js_core/tools/gen_tz.py gen <dir>       # zic -b fat, then src/tzdata/zones.rs
```

The encodings are documented at the head of each generator; the readers are `src/cldr/mod.rs` and
`src/tzdata/mod.rs`.

## Locales

32 CLDR data locales (the slot blob of a child holds only what differs from its CLDR parent) and 26 alias
tags. Scripts covered: Latin, Cyrillic, Arabic, Hebrew, Devanagari, Bengali, Thai, Han (Hans, Hant), Kana,
Hangul.

| data locale | CLDR parent | alias tags (same data) | default numbering system |
|---|---|---|---|
| en | root | en-US | latn |
| en-001 | en | | latn |
| en-GB | en-001 | | latn |
| en-AU | en-001 | | latn |
| en-CA | en | | latn |
| en-IN | en-001 | | latn |
| de | root | de-DE | latn |
| fr | root | fr-FR | latn |
| es | root | es-ES | latn |
| es-419 | es | | latn |
| es-MX | es-419 | | latn |
| it | root | it-IT | latn |
| pt | root | pt-BR | latn |
| nl | root | nl-NL | latn |
| sv | root | sv-SE | latn |
| da | root | da-DK | latn |
| pl | root | pl-PL | latn |
| ru | root | ru-RU | latn |
| uk | root | uk-UA | latn |
| tr | root | tr-TR | latn |
| ja | root | ja-JP | latn (native: none) |
| zh | root | zh-CN, zh-Hans, zh-Hans-CN | latn (native hanidec) |
| zh-Hant | root | zh-TW, zh-Hant-TW, zh-HK, zh-Hant-HK | latn (native hanidec) |
| ko | root | ko-KR | latn |
| ar | root | ar-EG | latn (native arab) |
| he | root | he-IL | latn |
| hi | root | hi-IN | latn (native deva) |
| th | root | th-TH | latn (native thai); calendar buddhist |
| vi | root | vi-VN | latn |
| id | root | id-ID | latn |
| fa | root | fa-IR | arabext |
| bn | root | bn-BD | beng |

Every numeric numbering system of CLDR (96, including arab, arabext, beng, deva, thai, hanidec) is
available through `-u-nu-` or the `numberingSystem` option; a locale's own symbols apply when it has them
for that system, its latn symbols otherwise.

## Table sizes (bytes, measured by the generators)

| table | file | bytes |
|---|---|---|
| locale data (32 blobs + sparse indexes + key lists) | src/cldr/tables.rs | 1,359,297 |
| collation: root (36,384 code points, 974 contractions, 41,169 elements) | src/cldr/collation.rs | 374,996 |
| collation: zh pinyin (44,887 Han characters) | src/cldr/collation.rs | 366,804 |
| collation: ja (6,695 kanji + kana rules) | src/cldr/collation.rs | 85,530 |
| collation: de-phonebk, es, es-trad, sv, da, tr | src/cldr/collation.rs | 2,293 |
| tz: 340 Zones (23,175 transitions), 257 Links | src/tzdata/zones.rs | 136,081 |
| **total** | | **2,324,281** (budget 3 MB) |

Per locale blob (bytes): en 37,673; en-001 9,351; en-GB 6,123; en-AU 7,801; en-CA 10,161; en-IN 6,980;
de 38,139; fr 41,157; es 39,173; es-419 10,126; es-MX 9,650; it 38,430; pt 40,557; nl 35,487; sv 37,550;
da 35,759; pl 46,970; ru 77,434; uk 82,676; tr 35,729; ja 45,488; zh 36,338; zh-Hant 37,119; ko 39,677;
ar 71,110; he 53,407; hi 73,667; th 75,395; vi 40,802; id 32,079; fa 50,686; bn 77,473.

## What reads the tables

- NumberFormat: CLDR patterns (affixes, explicit negative subpattern, primary / secondary grouping),
  minimumGroupingDigits, symbols and digits per numbering system, compact patterns by magnitude and plural
  category, currency symbols / narrow symbols / names / fraction digits / alphaNextToNumber patterns, unit
  patterns by plural category, compound units.
- DateTimeFormat: availableFormats with skeleton matching and width adjustment, date / time / date-time /
  atTime formats, appendItems, intervalFormats for formatRange, names of months / days / eras / day periods
  (flexible day periods from dayPeriodRuleSet), hour cycles (locale default, 12- and 24-hour variants),
  calendars gregory / japanese / buddhist / roc, zone names (zone, metazone, location and GMT formats).
- RelativeTimeFormat, PluralRules (compiled rules, categories, ranges), ListFormat (nine template sets and
  the es / he contextual forms), DisplayNames, DurationFormat (unit patterns, list, time separator).
- Collator: root DUCET/CLDR order with implicit weights; tailorings de-u-co-phonebk, es, es-u-co-trad, sv,
  da (caseFirst upper), tr, ja, zh (pinyin).
- Locale: likely subtags (maximize / minimize), timeData hour cycles, text direction by likely script;
  ResolveLocale with a best-fit script step.
- tz: `PosixTz::named` (history through 2037, POSIX rule after), Date's LocalTZA (also from the host's IANA
  id), DateTimeFormat's zones, Temporal's zones; Links resolve, zone.tab-listed Links stay primary.

## Refused (named)

- Calendars: islamic (all variants), hebrew, chinese, dangi, persian, indian, coptic, ethiopic, iso8601 —
  they resolve to the locale default; Japanese eras before Meiji (1868) show the Gregorian era; ja's
  `y=jpanyear` numbering (元年 for year 1).
- Collation: zh stroke / zhuyin / unihan, ja unihan, the search collations, the tailorings of the other
  locales here (pl, uk, vi, ko, ar, he, hi, th, fa, bn, ...); ja's [reorder Latn Kana Hani] is approximated
  (kanji follow the last kana) and its quaternary level is folded into the tertiary.
- Data not carried: CLDR exemplar cities (the generic location format falls back to the GMT format for a
  multi-zone country), historical metazone mappings (a zone's names follow its current metazone), backzone
  (a primary Link such as Europe/Oslo shares its target's pre-1970 history), number range / approximately
  miscPatterns (the range separator stays "–" / " – "), locales outside the table (they resolve to en-US).

## Tests (written, not run — R107)

`unaos/libs/web/js_core/tests/intl_cldr.rs`: intl_cldr_locale_negotiation, intl_cldr_numbers,
intl_cldr_dates, intl_cldr_relative_time, intl_cldr_plural_rules, intl_cldr_lists,
intl_cldr_display_names, intl_cldr_collation.

`unaos/libs/web/js_core/tests/tz.rs`: tz_new_york_2007_rule_change, tz_moscow_2011_and_2014,
tz_tokyo_without_daylight_time, tz_lord_howe_half_hour_daylight_time, tz_apia_2011_date_line_jump,
tz_names_links_and_primary_identifiers, tz_date_reads_the_host_zone.

Every expected value is read off the CLDR 46 JSON / collation rules or the tz source rules by hand.

## Owed outside this arc's files (not changed here)

- `unaos/libs/web/js_core/tests/modern.rs:70` expects `new Intl.NumberFormat('de-DE').resolvedOptions().locale`
  to be `"en-US"`; with de data it is `"de-DE"` — the expected value must become `"de-DE"`.
- `unaos/libs/web/js_core/tests/modern.rs:158` expects `Intl.NumberFormat.supportedLocalesOf(['fr', 'en-GB']).join()`
  to be `"en-GB"`; it is now `"fr,en-GB"`.
- `docs/dev/evidence/aetherjs2/FEATURES.md:65-66` ("Locale data beyond en / en-GB / en-US …", "Historical time
  zone transitions …") are answered by this arc; the rows should move to the added table with SR137.
- `unaos/libs/web/js_core/tests/t262/runner.rs:42` skips the whole `intl402` directory; no OUT_OF_SCOPE line in
  `tests/t262/meta.rs` was removed (nothing was run to show which intl402 or `canonical-tz` tests now score).
- `unaos/libs/web/js_core/src/builtins/temporal/tzdata.rs` and `gen_tzdata.py` (the 84-zone table) and
  `unaos/libs/web/js_core/src/builtins/intl/data.rs` are no longer read; kept per R20 for the seat to retire.
