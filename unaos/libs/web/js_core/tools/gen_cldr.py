#!/usr/bin/env python3
"""INTLCLDR (LEDGER SR137): generate src/cldr/tables.rs and src/cldr/collation.rs from CLDR 46.

Usage:
  python3 -I tools/gen_cldr.py fetch <dir>   sparse fetch: unicode-org/cldr-json tag 46.0.0 (the per-locale JSON and the
                                             supplemental files) and unicode-org/cldr tag release-46 (the root collation
                                             common/uca/allkeys_CLDR.txt and the tailorings common/collation/*.xml)
  python3 -I tools/gen_cldr.py gen <dir>     write the two tables

R83: no ICU. CLDR is data; this script turns it into compact tables the engine decodes lazily. The fetch is a
generation-time step, never a test: the generated tables are committed.

LOCALE TABLES (tables.rs). Every data locale is one `Loc`: a `blob` (all of its strings, each slot terminated by
U+0001, in one fixed slot order shared by every locale) and a sparse index `idx` holding the byte offset of every 16th
slot. Reading slot k: start at idx[k / 16], skip k % 16 terminators, read to the next one — no decoding of anything
the caller does not read. Slot values: "" = inherit from the CLDR parent locale (`parent`), U+0002 = explicitly
absent (CLDR's "no inheritance" marker, resolved here), anything else = the value. A child locale stores only the
slots that differ from its parent's resolved value, so en-GB / en-AU / es-MX cost a few hundred bytes. The slot order
is (generated `slot::` constants and key lists):

  numbers   default / native numbering system, minimum grouping digits, the 13 symbols of latn and of the native
            system, the decimal / percent / scientific / currency / currency-alphaNextToNumber / accounting /
            accounting-alphaNextToNumber patterns of both, compact short / long / currency-short (powers 3..14 x the six
            plural categories), the currency unit patterns (six categories), the compound-unit pattern of each width;
  currencies CUR_KEYS x (display name, symbol, narrow symbol, six plural names);
  units     UNIT_KEYS x (long, short, narrow) x (six plural patterns, per-unit pattern);
  dates     months (format / stand-alone x abbreviated / narrow / wide x 12), days (x abbreviated / narrow / short /
            wide x 7), day periods (abbreviated / narrow / wide x 12 ids), eras, date / time / date-time / at-time
            formats, the time-zone append item, the interval fallback, AF_KEYS (availableFormats), IF_KEYS
            (intervalFormats, "skeleton/greatest-difference"), and the japanese / buddhist / roc calendars (era names,
            date formats, CAL_AF_KEYS where they differ from the Gregorian pattern);
  relative  8 fields x 3 widths x (relative -2..2, future x 6, past x 6, display name), plus the display names of
            era / weekday / day period / zone;
  lists     9 types x (start, middle, end, 2);
  names     LANG_KEYS, REGION_KEYS, SCRIPT_KEYS, CALNAME_KEYS, the locale display / separator / key-type patterns;
  zones     the 7 GMT / region / fallback formats, META_KEYS x 6 (long / short x generic / standard / daylight),
            ZONE_KEYS x 6 (the zone-specific names).

Supplemental (shared): PLURALS (cardinal and ordinal rules compiled to the compact form `cat:rel&rel|rel;` with
relations `operand[%mod]=ranges` or `!=`, and the plural ranges), NUMSYS (the digits of every numeric numbering
system), CUR_DIGITS, LIKELY (likely subtags: bare languages, und-Region, und-Script, and the language-region keys of
the locales here), HOUR_PREF (timeData), DAY_PERIODS (dayPeriodRuleSet), ZONE_META (zone -> metazone), JA_ERAS.

COLLATION (collation.rs). The CLDR root collation (allkeys_CLDR.txt, the DUCET with CLDR's changes) as two sorted
arrays: ROOT_KEYS (the code point sequence, a single code point or a contraction, as a string) and ROOT_CES (the
packed collation elements `primary << 16 | secondary << 5 | tertiary`, bit 15 of the secondary unused; a
variable-weighted element carries bit 31 of the second word); decomposable code points are left out because the
collator normalizes to NFD first, code points without an entry take the UCA implicit weights. Each tailoring is
resolved HERE against the root weights (fractional weights: a tailored primary is root_primary << 16 + k,
secondaries and tertiaries likewise widened) and stored as its own sorted (string -> collation elements) table that
the collator consults before the root.
"""
import os, sys, json, re, urllib.request, concurrent.futures as cf, unicodedata

J = 'https://raw.githubusercontent.com/unicode-org/cldr-json/46.0.0/cldr-json/'
C = 'https://raw.githubusercontent.com/unicode-org/cldr/release-46/'
HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, '..', 'src', 'cldr')

# The data locales (CLDR names) and their CLDR parents (parentLocales.json, else truncation, else root).
LOCS = ['en', 'en-001', 'en-GB', 'en-AU', 'en-CA', 'en-IN', 'de', 'fr', 'es', 'es-419', 'es-MX', 'it', 'pt', 'nl', 'sv', 'da',
        'pl', 'ru', 'uk', 'tr', 'ja', 'zh', 'zh-Hant', 'ko', 'ar', 'he', 'hi', 'th', 'vi', 'id', 'fa', 'bn']
MAIN = [('cldr-numbers-full', 'numbers'), ('cldr-numbers-full', 'currencies'), ('cldr-dates-full', 'ca-gregorian'),
        ('cldr-dates-full', 'dateFields'), ('cldr-dates-full', 'timeZoneNames'), ('cldr-cal-japanese-full', 'ca-japanese'),
        ('cldr-cal-buddhist-full', 'ca-buddhist'), ('cldr-cal-roc-full', 'ca-roc'), ('cldr-units-full', 'units'),
        ('cldr-misc-full', 'listPatterns'), ('cldr-localenames-full', 'languages'), ('cldr-localenames-full', 'territories'),
        ('cldr-localenames-full', 'scripts'), ('cldr-localenames-full', 'localeDisplayNames')]
SUPP = ['plurals', 'ordinals', 'pluralRanges', 'numberingSystems', 'currencyData', 'likelySubtags', 'timeData', 'dayPeriods',
        'metaZones', 'parentLocales', 'calendarData', 'aliases']
COLL = ['root', 'de', 'es', 'sv', 'da', 'tr', 'ja', 'zh']


def fetch(d):
    files = []
    for l in LOCS:
        for pkg, f in MAIN:
            files.append((J + '%s/main/%s/%s.json' % (pkg, l, f), 'main/%s/%s.json' % (l, f)))
    for s in SUPP:
        files.append((J + 'cldr-core/supplemental/%s.json' % s, 'supplemental/%s.json' % s))
    files.append((J + 'cldr-core/availableLocales.json', 'availableLocales.json'))
    files.append((C + 'common/uca/allkeys_CLDR.txt', 'uca/allkeys_CLDR.txt'))
    for c in COLL:
        files.append((C + 'common/collation/%s.xml' % c, 'collation/%s.xml' % c))

    def get(u):
        url, p = u
        dst = os.path.join(d, p)
        if not os.path.exists(dst):
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            open(dst, 'wb').write(urllib.request.urlopen(url, timeout=60).read())
    with cf.ThreadPoolExecutor(16) as ex:
        list(ex.map(get, files))


# ============================================================================================ helpers

CATS = ['zero', 'one', 'two', 'few', 'many', 'other']
D = None


def load(rel):
    return json.load(open(os.path.join(D, rel), encoding='utf-8'))


def main_of(loc, f):
    j = load('main/%s/%s.json' % (loc, f))
    return j['main'][loc]


def rs_str(s):
    out = []
    for ch in s:
        o = ord(ch)
        if ch == '\\':
            out.append('\\\\')
        elif ch == '"':
            out.append('\\"')
        elif o < 0x20 or o == 0x7f or (0x80 <= o < 0xa0):
            out.append('\\u{%x}' % o)
        else:
            out.append(ch)
    return '"' + ''.join(out) + '"'


# ECMA-402 Table 2 (sanctioned simple units) and the CLDR compound units with their own names.
SIMPLE_UNITS = ['acre', 'bit', 'byte', 'celsius', 'centimeter', 'day', 'degree', 'fahrenheit', 'fluid-ounce', 'foot', 'gallon',
                'gigabit', 'gigabyte', 'gram', 'hectare', 'hour', 'inch', 'kilobit', 'kilobyte', 'kilogram', 'kilometer', 'liter',
                'megabit', 'megabyte', 'meter', 'microsecond', 'mile', 'mile-scandinavian', 'milliliter', 'millimeter',
                'millisecond', 'minute', 'month', 'nanosecond', 'ounce', 'percent', 'petabyte', 'pound', 'second', 'stone',
                'terabit', 'terabyte', 'week', 'yard', 'year']
COMPOUND_UNITS = ['kilometer-per-hour', 'mile-per-hour', 'meter-per-second', 'mile-per-gallon', 'liter-per-kilometer']

CURRENCIES = sorted(set('''AED AFN ALL AMD ARS AUD AZN BAM BDT BGN BHD BOB BRL BYN CAD CHF CLP CNY COP CRC CZK DKK DOP DZD EGP ETB EUR
GBP GEL GHS GTQ HKD HRK HUF IDR ILS INR IQD IRR ISK JMD JOD JPY KES KHR KRW KWD KZT LAK LBP LKR MAD MDL MKD MMK MNT MXN MYR NGN
NOK NPR NZD OMR PEN PHP PKR PLN PYG QAR RON RSD RUB SAR SEK SGD THB TND TRY TWD TZS UAH UGX USD UYU UZS VES VND XAF XDR XOF
XPF YER ZAR ZMW'''.split()))

FIELDS = ['year', 'quarter', 'month', 'week', 'day', 'hour', 'minute', 'second']
WIDTHS3 = ['', '-short', '-narrow']
LIST_TYPES = ['standard', 'standard-short', 'standard-narrow', 'or', 'or-short', 'or-narrow', 'unit', 'unit-short', 'unit-narrow']
DP_IDS = ['am', 'pm', 'midnight', 'noon', 'morning1', 'morning2', 'afternoon1', 'afternoon2', 'evening1', 'evening2', 'night1', 'night2']
SYMS = ['decimal', 'group', 'percentSign', 'plusSign', 'minusSign', 'approximatelySign', 'exponential', 'perMille', 'infinity', 'nan',
        'timeSeparator', 'superscriptingExponent', 'list']
PATS = ['decimal', 'percent', 'scientific', 'currency', 'currency-alpha', 'accounting', 'accounting-alpha']
JA_ERA_KEYS = ['232', '233', '234', '235', '236']
CAL_AF_FILTER = re.compile(r'[yGU]')


# ============================================================================================ locale data

def parent_of(loc, plmap):
    if loc in plmap:
        p = plmap[loc]
        return None if p in ('root', 'und') else p
    if '-' in loc:
        return loc.rsplit('-', 1)[0]
    return None


def extract(loc):
    """The slot values of one locale, keyed by slot name (absent = not a key)."""
    v = {}
    n = main_of(loc, 'numbers')['numbers']
    ns_def = n['defaultNumberingSystem']
    native = n.get('otherNumberingSystems', {}).get('native', 'latn')
    alt = native if native != 'latn' else ns_def
    if ns_def != 'latn':
        alt = ns_def
    v['ns.default'] = ns_def  # (a `_numbers` override such as ja's y=jpanyear, 元年 for year 1, is not carried)
    v['ns.alt'] = alt if alt != 'latn' and ('symbols-numberSystem-' + alt) in n else ''
    v['ns.native'] = native
    v['ns.mingroup'] = str(n['minimumGroupingDigits'])
    for which, ns in (('latn', 'latn'), ('alt', v['ns.alt'])):
        if not ns:
            continue
        sym = n['symbols-numberSystem-' + ns]
        for s in SYMS:
            if s in sym:
                v['sym.%s.%s' % (which, s)] = sym[s]
        dec = n['decimalFormats-numberSystem-' + ns]
        v['pat.%s.decimal' % which] = dec['standard']
        v['pat.%s.percent' % which] = n['percentFormats-numberSystem-' + ns]['standard']
        v['pat.%s.scientific' % which] = n['scientificFormats-numberSystem-' + ns]['standard']
        cur = n['currencyFormats-numberSystem-' + ns]
        v['pat.%s.currency' % which] = cur['standard']
        v['pat.%s.currency-alpha' % which] = cur.get('standard-alphaNextToNumber', cur['standard'])
        v['pat.%s.accounting' % which] = cur['accounting']
        v['pat.%s.accounting-alpha' % which] = cur.get('accounting-alphaNextToNumber', cur['accounting'])
        if which == 'latn':
            for si, style in enumerate(['short', 'long']):
                for k, p in dec[style]['decimalFormat'].items():
                    m = re.fullmatch(r'1(0+)-count-(\w+)', k)
                    if m:
                        v['compact.%d.%d.%s' % (si, len(m.group(1)), m.group(2))] = p
            if 'short' in cur:
                for k, p in cur['short']['standard'].items():
                    m = re.fullmatch(r'1(0+)-count-(\w+)', k)
                    if m:
                        v['compact.2.%d.%s' % (len(m.group(1)), m.group(2))] = p
            for c in CATS:
                k = 'unitPattern-count-' + c
                if k in cur:
                    v['curunit.' + c] = cur[k]
    cj = main_of(loc, 'currencies')['numbers']['currencies']
    for code in CURRENCIES:
        c = cj.get(code)
        if not c:
            continue
        if 'displayName' in c:
            v['cur.%s.name' % code] = c['displayName']
        v['cur.%s.symbol' % code] = c.get('symbol', code)
        v['cur.%s.narrow' % code] = c.get('symbol-alt-narrow', c.get('symbol', code))
        for cat in CATS:
            k = 'displayName-count-' + cat
            if k in c:
                v['cur.%s.%s' % (code, cat)] = c[k]
    uj = main_of(loc, 'units')['units']
    for w in ['long', 'short', 'narrow']:
        keys = uj[w]
        for u in SIMPLE_UNITS + COMPOUND_UNITS:
            ck = [k for k in keys if '-' in k and k.split('-', 1)[1] == u and isinstance(keys[k], dict)]
            if not ck:
                continue
            e = keys[ck[0]]
            for cat in CATS:
                k = 'unitPattern-count-' + cat
                if k in e:
                    v['unit.%s.%s.%s' % (u, w, cat)] = e[k]
            if 'perUnitPattern' in e:
                v['unit.%s.%s.per' % (u, w)] = e['perUnitPattern']
            if 'displayName' in e:
                v['unit.%s.%s.name' % (u, w)] = e['displayName']
        v['unitper.' + w] = keys['per']['compoundUnitPattern']
    g = main_of(loc, 'ca-gregorian')['dates']['calendars']['gregorian']
    for ctx in ['format', 'stand-alone']:
        for wi, w in enumerate(['abbreviated', 'narrow', 'wide']):
            for m in range(12):
                v['month.%s.%s.%d' % (ctx, w, m)] = g['months'][ctx][w][str(m + 1)]
        for w in ['abbreviated', 'narrow', 'short', 'wide']:
            for di, dk in enumerate(['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat']):
                v['day.%s.%s.%d' % (ctx, w, di)] = g['days'][ctx][w][dk]
    for w in ['abbreviated', 'narrow', 'wide']:
        dp = g['dayPeriods']['format'][w]
        for k in DP_IDS:
            if k in dp:
                v['dp.%s.%s' % (w, k)] = dp[k]
    for w, ek in [('wide', 'eraNames'), ('abbreviated', 'eraAbbr'), ('narrow', 'eraNarrow')]:
        for e in ['0', '1']:
            v['era.%s.%s' % (w, e)] = g['eras'][ek][e]
    for s in ['full', 'long', 'medium', 'short']:
        v['df.' + s] = g['dateFormats'][s]
        v['tf.' + s] = g['timeFormats'][s]
        v['dtf.' + s] = g['dateTimeFormats'][s]
        at = g.get('dateTimeFormats-atTime', {}).get('standard', {})
        if s in at:
            v['dtat.' + s] = at[s]
    v['append.tz'] = g['dateTimeFormats'].get('appendItems', {}).get('Timezone', '{0} {1}')
    for k, p in g['dateTimeFormats']['availableFormats'].items():
        if '-alt-' in k or '-count-' in k:
            continue
        v['af.' + k] = p
    for k, p in g['dateTimeFormats']['intervalFormats'].items():
        if k == 'intervalFormatFallback':
            v['if.fallback'] = p
            continue
        if '-alt-' in k:
            continue
        for gd, pp in p.items():
            if '-alt-' in gd:
                continue
            v['if.%s/%s' % (k, gd)] = pp
    for cal in ['japanese', 'buddhist', 'roc']:
        cj2 = main_of(loc, 'ca-' + cal)['dates']['calendars'][cal]
        eras = JA_ERA_KEYS if cal == 'japanese' else (['0'] if cal == 'buddhist' else ['0', '1'])
        for w, ek in [('wide', 'eraNames'), ('abbreviated', 'eraAbbr'), ('narrow', 'eraNarrow')]:
            for i, e in enumerate(eras):
                if e in cj2['eras'][ek]:
                    v['cal.%s.era.%s.%d' % (cal, w, i)] = cj2['eras'][ek][e]
        for s in ['full', 'long', 'medium', 'short']:
            if s in cj2.get('dateFormats', {}):
                x = cj2['dateFormats'][s]
                v['cal.%s.df.%s' % (cal, s)] = x['_value'] if isinstance(x, dict) else x
        for k, p in cj2.get('dateTimeFormats', {}).get('availableFormats', {}).items():
            if '-alt-' in k or '-count-' in k or not CAL_AF_FILTER.search(k):
                continue
            if v.get('af.' + k) != p:
                v['cal.%s.af.%s' % (cal, k)] = p
    fj = main_of(loc, 'dateFields')['dates']['fields']
    for f in FIELDS + ['era', 'weekday', 'dayperiod', 'zone']:
        for w in WIDTHS3:
            e = fj.get(f + w)
            if not e:
                continue
            if 'displayName' in e:
                v['rel.%s%s.name' % (f, w)] = e['displayName']
            for r in ['-2', '-1', '0', '1', '2']:
                k = 'relative-type-' + r
                if k in e:
                    v['rel.%s%s.%s' % (f, w, r)] = e[k]
            for fp in ['future', 'past']:
                rt = e.get('relativeTime-type-' + fp, {})
                for cat in CATS:
                    k = 'relativeTimePattern-count-' + cat
                    if k in rt:
                        v['rel.%s%s.%s.%s' % (f, w, fp, cat)] = rt[k]
    lj = main_of(loc, 'listPatterns')['listPatterns']
    for t in LIST_TYPES:
        e = lj['listPattern-type-' + t]
        for part in ['start', 'middle', 'end', '2']:
            v['list.%s.%s' % (t, part)] = e[part]
    ldn = main_of(loc, 'languages')['localeDisplayNames']['languages']
    for k, s in ldn.items():
        v['lang.' + k] = s
    for k, s in main_of(loc, 'territories')['localeDisplayNames']['territories'].items():
        v['region.' + k] = s
    for k, s in main_of(loc, 'scripts')['localeDisplayNames']['scripts'].items():
        v['script.' + k] = s
    dn = main_of(loc, 'localeDisplayNames')['localeDisplayNames']
    for k, s in dn['types'].get('calendar', {}).items():
        v['calname.' + k] = s
    for k in ['localePattern', 'localeSeparator', 'localeKeyTypePattern']:
        v['ldp.' + k] = dn['localeDisplayPattern'][k]
    tz = main_of(loc, 'timeZoneNames')['dates']['timeZoneNames']
    for k in ['hourFormat', 'gmtFormat', 'gmtZeroFormat', 'regionFormat', 'regionFormat-type-daylight', 'regionFormat-type-standard',
              'fallbackFormat']:
        v['tz.' + k] = tz[k]
    for mz, e in tz.get('metazone', {}).items():
        for ln in ['long', 'short']:
            for kind in ['generic', 'standard', 'daylight']:
                if kind in e.get(ln, {}):
                    v['meta.%s.%s.%s' % (mz, ln, kind)] = e[ln][kind]

    def walk(node, path):
        for k, e in node.items():
            if not isinstance(e, dict):
                continue
            if 'exemplarCity' in e or 'long' in e or 'short' in e:
                zid = '/'.join(path + [k])
                for ln in ['long', 'short']:
                    for kind in ['generic', 'standard', 'daylight']:
                        if kind in e.get(ln, {}):
                            v['zone.%s.%s.%s' % (zid, ln, kind)] = e[ln][kind]
            else:
                walk(e, path + [k])
    walk(tz.get('zone', {}), [])
    return v


def build_slots(all_vals):
    """The global slot order: fixed families first, then keyed families with their key lists."""
    slots = ['ns.default', 'ns.alt', 'ns.native', 'ns.mingroup']
    for which in ['latn', 'alt']:
        slots += ['sym.%s.%s' % (which, s) for s in SYMS]
        slots += ['pat.%s.%s' % (which, p) for p in PATS]
    for st in range(3):
        for p in range(3, 15):
            for c in CATS:
                slots.append('compact.%d.%d.%s' % (st, p, c))
    slots += ['curunit.' + c for c in CATS]
    slots += ['unitper.' + w for w in ['long', 'short', 'narrow']]
    for ctx in ['format', 'stand-alone']:
        for w in ['abbreviated', 'narrow', 'wide']:
            slots += ['month.%s.%s.%d' % (ctx, w, m) for m in range(12)]
    for ctx in ['format', 'stand-alone']:
        for w in ['abbreviated', 'narrow', 'short', 'wide']:
            slots += ['day.%s.%s.%d' % (ctx, w, d) for d in range(7)]
    for w in ['abbreviated', 'narrow', 'wide']:
        slots += ['dp.%s.%s' % (w, k) for k in DP_IDS]
    for w in ['wide', 'abbreviated', 'narrow']:
        slots += ['era.%s.%s' % (w, e) for e in ['0', '1']]
    for f in ['df', 'tf', 'dtf', 'dtat']:
        slots += ['%s.%s' % (f, s) for s in ['full', 'long', 'medium', 'short']]
    slots += ['append.tz', 'if.fallback']
    for cal, n in [('japanese', 5), ('buddhist', 1), ('roc', 2)]:
        for w in ['wide', 'abbreviated', 'narrow']:
            slots += ['cal.%s.era.%s.%d' % (cal, w, i) for i in range(n)]
        slots += ['cal.%s.df.%s' % (cal, s) for s in ['full', 'long', 'medium', 'short']]
    for f in FIELDS:
        for w in WIDTHS3:
            slots += ['rel.%s%s.%s' % (f, w, r) for r in ['-2', '-1', '0', '1', '2']]
            for fp in ['future', 'past']:
                slots += ['rel.%s%s.%s.%s' % (f, w, fp, c) for c in CATS]
            slots.append('rel.%s%s.name' % (f, w))
    for f in ['era', 'weekday', 'dayperiod', 'zone']:
        for w in WIDTHS3:
            slots.append('rel.%s%s.name' % (f, w))
    for t in LIST_TYPES:
        slots += ['list.%s.%s' % (t, p) for p in ['start', 'middle', 'end', '2']]
    slots += ['ldp.localePattern', 'ldp.localeSeparator', 'ldp.localeKeyTypePattern']
    slots += ['tz.' + k for k in ['hourFormat', 'gmtFormat', 'gmtZeroFormat', 'regionFormat', 'regionFormat-type-daylight',
                                  'regionFormat-type-standard', 'fallbackFormat']]
    fixed = list(slots)
    keyed = {}

    def keys_of(prefix, sub_count=None):
        ks = set()
        for vals in all_vals.values():
            for k in vals:
                if k.startswith(prefix):
                    ks.add(k[len(prefix):])
        return ks

    cur_fields = ['name', 'symbol', 'narrow'] + CATS
    keyed['CUR'] = (sorted(CURRENCIES), ['cur.%s.' + f for f in cur_fields])
    unit_fields = []
    for w in ['long', 'short', 'narrow']:
        unit_fields += ['unit.%%s.%s.%s' % (w, c) for c in CATS] + ['unit.%%s.%s.per' % w, 'unit.%%s.%s.name' % w]
    keyed['UNIT'] = (sorted(SIMPLE_UNITS + COMPOUND_UNITS), unit_fields)
    keyed['AF'] = (sorted(keys_of('af.')), ['af.%s'])
    keyed['IF'] = (sorted(k for k in keys_of('if.') if k != 'fallback'), ['if.%s'])
    cal_af = set()
    for cal in ['japanese', 'buddhist', 'roc']:
        cal_af |= keys_of('cal.%s.af.' % cal)
    keyed['CALAF'] = (sorted(cal_af), ['cal.japanese.af.%s', 'cal.buddhist.af.%s', 'cal.roc.af.%s'])
    en = all_vals['en']
    langs = sorted(k[5:] for k in en if k.startswith('lang.'))
    keyed['LANG'] = (langs, ['lang.%s'])
    keyed['REGION'] = (sorted(k[7:] for k in en if k.startswith('region.')), ['region.%s'])
    keyed['SCRIPT'] = (sorted(k[7:] for k in en if k.startswith('script.')), ['script.%s'])
    keyed['CALNAME'] = (sorted(k[8:] for k in en if k.startswith('calname.')), ['calname.%s'])
    metas = set()
    for vals in all_vals.values():
        for k in vals:
            if k.startswith('meta.'):
                metas.add(k.split('.')[1])
    keyed['META'] = (sorted(metas), ['meta.%%s.%s.%s' % (l, k) for l in ['long', 'short'] for k in ['generic', 'standard', 'daylight']])
    zones = set()
    for vals in all_vals.values():
        for k in vals:
            if k.startswith('zone.'):
                zones.add(k[5:].rsplit('.', 2)[0])
    keyed['ZONE'] = (sorted(zones), ['zone.%%s.%s.%s' % (l, k) for l in ['long', 'short'] for k in ['generic', 'standard', 'daylight']])
    bases = {}
    for fam in ['CUR', 'UNIT', 'AF', 'IF', 'CALAF', 'LANG', 'REGION', 'SCRIPT', 'CALNAME', 'META', 'ZONE']:
        keys, fields = keyed[fam]
        bases[fam] = len(slots)
        for key in keys:
            for f in fields:
                slots.append(f % key)
    return fixed, keyed, bases, slots


def const_name(s):
    return re.sub(r'[^A-Za-z0-9]', '_', s).upper()


def compile_plural(rule):
    """`v = 0 and i % 10 = 1 or ...` -> `v=0&i%10=1|...` (the samples after @ dropped)."""
    r = rule.split('@', 1)[0].strip()
    if not r:
        return ''
    ors = []
    for o in r.split(' or '):
        ands = []
        for a in o.split(' and '):
            a = a.replace(' ', '')
            m = re.fullmatch(r'([nivwftce])(?:%(\d+))?(!=|=)([0-9.,]+)', a)
            assert m, a
            op = m.group(1) + ('%' + m.group(2) if m.group(2) else '') + ('!' if m.group(3) == '!=' else '=') + m.group(4)
            ands.append(op)
        ors.append('&'.join(ands))
    return '|'.join(ors)


def gen_tables():
    global D
    pl = load('supplemental/parentLocales.json')['supplemental']['parentLocales']['parentLocale']
    vals = {l: extract(l) for l in LOCS}
    fixed, keyed, bases, slots = build_slots(vals)
    sidx = {s: i for i, s in enumerate(slots)}
    parents = {l: parent_of(l, pl) for l in LOCS}
    for l, p in parents.items():
        assert p is None or p in LOCS, (l, p)
    # resolved value per locale (cldr-json is resolved: every locale file carries its inherited values)
    blobs, idxs = {}, {}
    total = 0
    for l in LOCS:
        p = parents[l]
        parts = []
        for s in slots:
            mine = vals[l].get(s)
            if p is None:
                parts.append(mine if mine is not None else '')
            else:
                theirs = vals[p].get(s)
                if mine == theirs:
                    parts.append('')
                elif mine is None:
                    parts.append('\x02')
                else:
                    parts.append(mine)
        for x in parts:
            assert '\x01' not in x
            assert x != '' or True
        # an empty value that means "absent" at root must not be confused with inherit: roots have no parent
        idx = []
        off = 0
        enc = []
        for i, x in enumerate(parts):
            if i % 16 == 0:
                idx.append(off)
            enc.append(x + '\x01')
            off += len((x + '\x01').encode('utf-8'))
        blobs[l] = ''.join(enc)
        idxs[l] = idx
        total += off + 4 * len(idx)
    out = ['// GENERATED by tools/gen_cldr.py from CLDR 46 (unicode-org/cldr-json 46.0.0) — do not edit.',
           '//! Locale data of %d CLDR locales in %d slots each; encoding and slot order: see tools/gen_cldr.py.' % (len(LOCS), len(slots)),
           '']
    out.append('pub struct Loc {\n    pub tag: &\'static str,\n    /// Index into LOCS of the CLDR parent (255: root).\n    pub parent: u8,\n'
               '    /// The language whose plural rules and day periods apply.\n    pub lang: &\'static str,\n'
               '    pub blob: &\'static str,\n    /// Byte offset of every 16th slot in `blob`.\n    pub idx: &\'static [u32],\n}\n')
    out.append('pub const NSLOTS: usize = %d;\n' % len(slots))
    out.append('pub mod slot {')
    for s in fixed:
        out.append('    pub const %s: usize = %d;' % (const_name(s), sidx[s]))
    for fam in bases:
        out.append('    pub const %s_BASE: usize = %d;' % (fam, bases[fam]))
        out.append('    pub const %s_FIELDS: usize = %d;' % (fam, len(keyed[fam][1])))
    out.append('}\n')
    for fam in bases:
        keys = keyed[fam][0]
        out.append('pub static %s_KEYS: &[&str] = &[%s];' % (fam, ', '.join(rs_str(k) for k in keys)))
        total += sum(len(k) + 16 for k in keys)
    out.append('')
    out.append('pub static LOCS: &[Loc] = &[')
    for l in LOCS:
        p = parents[l]
        lang = l.split('-')[0]
        out.append('    Loc { tag: "%s", parent: %d, lang: "%s", blob: %s, idx: &[%s] },' % (
            l, 255 if p is None else LOCS.index(p), lang, rs_str(blobs[l]), ', '.join(str(x) for x in idxs[l])))
    out.append('];\n')
    # ---- the available locale tags: every data locale plus the language-likely-region aliases
    likely = load('supplemental/likelySubtags.json')['supplemental']['likelySubtags']
    avail = [(l, LOCS.index(l)) for l in LOCS]
    for l in LOCS:
        if '-' in l:
            continue
        full = likely.get(l, '')
        parts = full.split('-')
        if len(parts) == 3:
            tag = '%s-%s' % (l, parts[2])
            if tag not in LOCS:
                avail.append((tag, LOCS.index(l)))
    avail += [('zh-Hans', LOCS.index('zh')), ('zh-Hans-CN', LOCS.index('zh')), ('zh-TW', LOCS.index('zh-Hant')),
              ('zh-Hant-TW', LOCS.index('zh-Hant')), ('zh-HK', LOCS.index('zh-Hant')), ('zh-Hant-HK', LOCS.index('zh-Hant'))]
    avail.sort()
    out.append('/// ECMA-402 [[AvailableLocales]]: (tag, index into LOCS).')
    out.append('pub static AVAILABLE: &[(&str, u8)] = &[%s];\n' % ', '.join('("%s", %d)' % a for a in avail))
    # ---- plurals
    card = load('supplemental/plurals.json')['supplemental']['plurals-type-cardinal']
    ordi = load('supplemental/ordinals.json')['supplemental']['plurals-type-ordinal']
    rng = load('supplemental/pluralRanges.json')['supplemental']['plurals']
    langs = sorted(set(l.split('-')[0] for l in LOCS) | {'pt-PT'})
    rows = []
    for lg in langs:
        def comp(tab):
            e = tab.get(lg, {})
            return ';'.join('%s:%s' % (c, compile_plural(e['pluralRule-count-' + c])) for c in CATS if 'pluralRule-count-' + c in e and c != 'other')
        rr = rng.get(lg, {})
        ranges = ';'.join('%s-%s:%s' % (re.match(r'pluralRange-start-(\w+)-end-(\w+)', k).group(1),
                                          re.match(r'pluralRange-start-(\w+)-end-(\w+)', k).group(2), x) for k, x in sorted(rr.items()))
        rows.append('    ("%s", "%s", "%s", "%s"),' % (lg, comp(card), comp(ordi), ranges))
    out.append('/// (language, cardinal rules, ordinal rules, plural ranges) in the compiled form of tools/gen_cldr.py.')
    out.append('pub static PLURALS: &[(&str, &str, &str, &str)] = &[\n%s\n];\n' % '\n'.join(rows))
    ns = load('supplemental/numberingSystems.json')['supplemental']['numberingSystems']
    nrows = sorted((k, e['_digits']) for k, e in ns.items() if e.get('_type') == 'numeric')
    out.append('/// The numeric numbering systems and their ten digits.')
    out.append('pub static NUMSYS: &[(&str, &str)] = &[%s];\n' % ', '.join('("%s", %s)' % (k, rs_str(d)) for k, d in nrows))
    fr = load('supplemental/currencyData.json')['supplemental']['currencyData']['fractions']
    crows = sorted((k, int(e['_digits'])) for k, e in fr.items() if k != 'DEFAULT' and int(e['_digits']) != 2)
    out.append('/// ISO 4217 / CLDR fraction digits of the currencies that do not use 2.')
    out.append('pub static CUR_DIGITS: &[(&str, u8)] = &[%s];\n' % ', '.join('("%s", %d)' % r for r in crows))
    avl = load('availableLocales.json')['availableLocales']['full']
    base_langs = set(a.split('-')[0] for a in avl)
    our_langs = set(l.split('-')[0] for l in LOCS)
    lk = []
    for k, x in likely.items():
        p = k.split('-')
        if (len(p) == 1 and (p[0] in base_langs or p[0] == 'und')) or (p[0] == 'und' and len(p) == 2) or (p[0] in our_langs and len(p) == 2):
            lk.append((k, x))
    lk.sort()
    out.append('/// Likely subtags (UTS #35 §4.3): bare languages with CLDR locales, und-Region, und-Script, and the')
    out.append('/// language-script / language-region keys of the languages here; sorted by key.')
    out.append('pub static LIKELY: &[(&str, &str)] = &[%s];\n' % ', '.join('("%s", "%s")' % r for r in lk))
    td = load('supplemental/timeData.json')['supplemental']['timeData']
    out.append('/// timeData: (region, preferred hour symbol, allowed) sorted by region.')
    out.append('pub static HOUR_PREF: &[(&str, &str, &str)] = &[%s];\n' % ', '.join(
        '("%s", "%s", "%s")' % (k, e['_preferred'], e['_allowed']) for k, e in sorted(td.items())))
    dps = load('supplemental/dayPeriods.json')['supplemental']['dayPeriodRuleSet']
    drows = []
    for lg in sorted(our_langs):
        e = dps.get(lg, {})
        parts = []
        for k, r in sorted(e.items()):
            if '_at' in r:
                parts.append('%s@%d' % (k, int(r['_at'][:2])))
            else:
                parts.append('%s:%d-%d' % (k, int(r['_from'][:2]), int(r['_before'][:2])))
        drows.append('("%s", "%s")' % (lg, ','.join(parts)))
    out.append('/// dayPeriodRuleSet per language: `id:from-before` (hours) and `id@hour`.')
    out.append('pub static DAY_PERIODS: &[(&str, &str)] = &[%s];\n' % ', '.join(drows))
    mz = load('supplemental/metaZones.json')['supplemental']['metaZones']['metazoneInfo']['timezone']
    meta_keys = keyed['META'][0]
    zrows = []

    def walkm(node, path):
        for k, e in node.items():
            if isinstance(e, list):
                cur = [u['usesMetazone']['_mzone'] for u in e if '_to' not in u['usesMetazone']]
                if cur and cur[0] in meta_keys:
                    zrows.append(('/'.join(path + [k]), meta_keys.index(cur[0])))
            else:
                walkm(e, path + [k])
    walkm(mz, [])
    zrows.sort()
    out.append('/// CLDR zone id -> index into META_KEYS (the metazone in use now).')
    out.append('pub static ZONE_META: &[(&str, u16)] = &[%s];\n' % ', '.join('("%s", %d)' % r for r in zrows))
    cd = load('supplemental/calendarData.json')['supplemental']['calendarData']['japanese']['eras']
    jrows = []
    for e in JA_ERA_KEYS:
        y, m, d = cd[e]['_start'].split('-')
        jrows.append('(%d, %d, %d)' % (int(y), int(m), int(d)))
    out.append('/// The start dates (proleptic Gregorian) of the Japanese eras Meiji, Taisho, Showa, Heisei, Reiwa.')
    out.append('pub static JA_ERAS: &[(i32, u8, u8)] = &[%s];\n' % ', '.join(jrows))
    out.insert(2, '//! Table bytes (blobs + sparse indexes + key lists): %d.' % total)
    open(os.path.join(OUT_DIR, 'tables.rs'), 'w', encoding='utf-8').write('\n'.join(out))
    print('locales', len(LOCS), 'slots', len(slots), 'bytes', total)
    for l in LOCS:
        print('  %-8s %7d' % (l, len(blobs[l].encode('utf-8'))))
    return total


# ============================================================================================ collation

COMMON_S, COMMON_T = 0x20 << 6, 0x02 << 4
UI = None


def unified_ideograph_ranges():
    """P_UNIFIED_IDEOGRAPH of src/unicode/props.rs (UCD 17.0.0) — the table the collator's implicit weights read."""
    src = open(os.path.join(HERE, '..', 'src', 'unicode', 'props.rs'), encoding='utf-8').read()
    m = re.search(r'pub static P_UNIFIED_IDEOGRAPH: \[u32; \d+\] = \[(.*?)\];', src, re.S)
    nums = [int(x) for x in re.findall(r'\d+', m.group(1))]
    return list(zip(nums[0::2], nums[1::2]))


def implicit(cp):
    """UCA §10.1.3 implicit weights (widened): two elements."""
    def ui(c):
        return any(a <= c <= b for a, b in UI)
    if 0x17000 <= cp <= 0x18AFF or 0x18D00 <= cp <= 0x18D8F:
        a, b = 0xFB00, (cp - 0x17000) | 0x8000
    elif 0x1B170 <= cp <= 0x1B2FF:
        a, b = 0xFB01, (cp - 0x1B170) | 0x8000
    elif 0x18B00 <= cp <= 0x18CFF:
        a, b = 0xFB02, (cp - 0x18B00) | 0x8000
    elif ui(cp) and (0x4E00 <= cp <= 0x9FFF or 0xF900 <= cp <= 0xFAFF):
        a, b = 0xFB40 + (cp >> 15), (cp & 0x7FFF) | 0x8000
    elif ui(cp):
        a, b = 0xFB80 + (cp >> 15), (cp & 0x7FFF) | 0x8000
    else:
        a, b = 0xFBC0 + (cp >> 15), (cp & 0x7FFF) | 0x8000
    return [(a << 16, COMMON_S, COMMON_T, 0), (b << 16, 0, 0, 0)]


def parse_allkeys(path):
    root = {}
    for ln in open(path, encoding='utf-8'):
        ln = ln.split('#', 1)[0].strip()
        if not ln or ln.startswith('@'):
            continue
        k, v = ln.split(';', 1)
        cps = tuple(int(x, 16) for x in k.split())
        root[cps] = [(int(p, 16), int(s_, 16), int(t, 16), 1 if a == '*' else 0)
                     for a, p, s_, t in re.findall(r'\[([.*])([0-9A-F]+)\.([0-9A-F]+)\.([0-9A-F]+)\]', v)]
    return root


def widen(ce):
    p, s_, t, v = ce
    return (p << 16, s_ << 6, t << 4, v)


def nfd(s_):
    return unicodedata.normalize('NFD', s_)


class Coll:
    """The collation elements of a string under the root plus the tailoring built so far (longest match on NFD)."""

    def __init__(self, root):
        self.root = root
        self.tail = {}
        self.maxlen = max(len(k) for k in root)

    def ces(self, text):
        cps = [ord(c) for c in nfd(text)]
        out, i = [], 0
        while i < len(cps):
            for n in range(min(max(self.maxlen, 4), len(cps) - i), 0, -1):
                k = tuple(cps[i:i + n])
                if k in self.tail:
                    out += self.tail[k]
                    i += n
                    break
                if k in self.root:
                    out += [widen(c) for c in self.root[k]]
                    i += n
                    break
            else:
                out += implicit(cps[i])
                i += 1
        return out


def tokenize(rules):
    toks, i, n = [], 0, len(rules)
    syntax = set('&<=|/[]#*\' \t\r\n')
    while i < n:
        c = rules[i]
        if c in ' \t\r\n':
            i += 1
        elif c == '#':
            while i < n and rules[i] != '\n':
                i += 1
        elif c == '[':
            j = rules.index(']', i)
            toks.append(('opt', rules[i + 1:j].strip()))
            i = j + 1
        elif c == '&':
            toks.append(('reset',))
            i += 1
        elif c in '<=':
            j = i
            while j < n and rules[j] == c:
                j += 1
            level = 0 if c == '=' else (j - i)
            star = j < n and rules[j] == '*'
            if star:
                j += 1
            toks.append(('rel', level, star))
            i = j
        elif c in '|/':
            toks.append((c,))
            i += 1
        else:
            buf = []
            while i < n and (rules[i] not in syntax or rules[i] == "'"):
                if rules[i] == '\\' and i + 1 < n and rules[i + 1] == 'u':
                    buf.append(chr(int(rules[i + 2:i + 6], 16)))
                    i += 6
                elif rules[i] == "'":
                    j = rules.index("'", i + 1)
                    lit = rules[i + 1:j]
                    if lit == '':
                        buf.append("'")
                    else:
                        buf.append(re.sub(r'\\u([0-9A-Fa-f]{4})', lambda m: chr(int(m.group(1), 16)), lit))
                    i = j + 1
                else:
                    buf.append(rules[i])
                    i += 1
            toks.append(('str', ''.join(buf)))
    return toks


def rules_of(d, file, ctype, alt=None):
    x = open(os.path.join(d, 'collation', file + '.xml'), encoding='utf-8').read()
    for m in re.finditer(r'<collation\s+([^>]*)>\s*(?:<!--.*?-->\s*)?<cr><!\[CDATA\[(.*?)\]\]></cr>', x, re.S):
        attrs = dict(re.findall(r'(\w+)=["\']([^"\']*)["\']', m.group(1)))
        if attrs.get('type') == ctype and attrs.get('alt') == alt and 'draft' not in attrs:
            return m.group(2)
    raise KeyError((file, ctype, alt))


IMPORTS = {'zh-u-co-private-pinyin': ('zh', 'private-pinyin'), 'ja-u-co-private-kana': ('ja', 'private-kana'),
           'de-u-co-phonebk': ('de', 'phonebook'), 'sv-u-co-standard': ('sv', 'standard'), 'da-u-co-standard': ('da', 'standard'),
           'tr-u-co-standard': ('tr', 'standard')}


def bump(prev, level):
    """The elements one step after `prev` at `level` (1 primary, 2 secondary, 3+ tertiary, 0 identical)."""
    if level == 0:
        return list(prev)
    lvl = min(level, 3)
    idx = [k for k, c in enumerate(prev) if c[lvl - 1] != 0]
    lv = idx[-1] if idx else len(prev) - 1
    p, s_, t_, v = prev[lv]
    if lvl == 1:
        p, s_, t_ = p + 1, COMMON_S, COMMON_T
    elif lvl == 2:
        s_, t_ = s_ + 1, COMMON_T
    else:
        t_ = t_ + 1
    return prev[:lv] + [(p, s_, t_, v)]


def apply_rules(d, co, rules, opts, last_regular):
    toks = tokenize(rules)
    i = 0
    prev = None
    while i < len(toks):
        t = toks[i]
        if t[0] == 'opt':
            o = t[1]
            if o.startswith('import '):
                f, ty = IMPORTS[o[7:].strip()]
                apply_rules(d, co, rules_of(d, f, ty), opts, last_regular)
            elif o.startswith('caseFirst'):
                opts['case_upper'] = o.split()[1] == 'upper'
            elif o.startswith('reorder'):
                opts['reorder'] = o.split()[1:]
            i += 1
            continue
        if t[0] == 'reset':
            i += 1
            before = 0
            if toks[i][0] == 'opt' and toks[i][1].startswith('before'):
                before = int(toks[i][1].split()[1])
                i += 1
            if toks[i][0] == 'opt' and toks[i][1] == 'last regular':
                prev = [last_regular]
                i += 1
                continue
            anchor = toks[i][1]
            i += 1
            ce = co.ces(anchor)
            if before:
                lv = [k for k, c in enumerate(ce) if c[before - 1] != 0][-1]
                p, s_, t_, v = ce[lv]
                if before == 1:
                    p -= 0xC000
                elif before == 2:
                    s_ -= 32
                else:
                    t_ -= 8
                ce = ce[:lv] + [(p, s_, t_, v)]
            prev = ce
            continue
        if t[0] == 'rel':
            level, star = t[1], t[2]
            i += 1
            prefix, ext = '', ''
            s_ = toks[i][1]
            i += 1
            if i < len(toks) and toks[i][0] == '|':
                prefix, s_ = s_, toks[i + 1][1]
                i += 2
            if i < len(toks) and toks[i][0] == '/':
                ext = toks[i + 1][1]
                i += 2
            items = list(s_) if star else [s_]
            for it in items:
                if it.startswith('﷐'):
                    continue
                new = bump(prev, level)
                key = tuple(ord(c) for c in nfd(prefix + it))
                co.tail[key] = (co.ces(prefix) if prefix else []) + new + (co.ces(ext) if ext else [])
                prev = new
            continue
        raise ValueError(t)


TAILORINGS = [
    # (id, file, type)
    ('de-phonebk', 'de', 'phonebook'),
    ('es', 'es', 'standard'),
    ('es-trad', 'es', 'traditional'),
    ('sv', 'sv', 'standard'),
    ('da', 'da', 'standard'),
    ('tr', 'tr', 'standard'),
    ('ja', 'ja', 'standard'),
    ('zh', 'zh', 'pinyin'),
]


def pack_root(ce):
    p, s_, t_, v = ce
    assert p < 0x10000 and s_ < 0x400 and t_ < 0x20
    return (p << 16) | (v << 15) | (s_ << 5) | t_


def pack_wide(ce):
    p, s_, t_, v = ce
    assert 0 <= p < 1 << 32 and 0 <= s_ < 1 << 16 and 0 <= t_ < 1 << 15, ce
    return (p << 32) | (s_ << 16) | (t_ << 1) | v


TAILORING_STRUCT = """
/// A tailoring resolved against the root: widened elements `primary << 32 | secondary << 16 | tertiary << 1 | variable`
/// (root weights widened by primary << 16, secondary << 6, tertiary << 4; the low bits order the tailored items).
pub struct Tailoring {
    pub id: &'static str,
    /// The tailoring sets [caseFirst upper].
    pub case_upper: bool,
    /// Single code point keys, sorted.
    pub single: &'static [u32],
    /// Per single key: bit 31 set = one element `anchors[(x >> 16) & 0x7fff]` with `x & 0xffff` added to its primary;
    /// clear = `start << 5 | count` into `ces`.
    pub single_ce: &'static [u32],
    /// Multi-code-point keys (NFD, sorted by bytes): (key, start << 5 | count).
    pub multi: &'static [(&'static str, u32)],
    pub ces: &'static [u64],
    pub anchors: &'static [u64],
}
"""


def gen_collation(d):
    global UI
    UI = unified_ideograph_ranges()
    root = parse_allkeys(os.path.join(d, 'uca', 'allkeys_CLDR.txt'))
    singles = sorted(k[0] for k in root if len(k) == 1 and nfd(chr(k[0])) == chr(k[0]))
    contr = sorted((k for k in root if len(k) > 1), key=lambda k: ''.join(map(chr, k)).encode('utf-8'))
    ces, ns, offs = [], [], []
    for i, cp in enumerate(singles):
        if i % 64 == 0:
            offs.append(len(ces))
        ns.append(len(root[(cp,)]))
        ces += [pack_root(c) for c in root[(cp,)]]
    crow = []
    for k in contr:
        crow.append((''.join(map(chr, k)), len(ces), len(root[k])))
        ces += [pack_root(c) for c in root[k]]
    assert len(ces) < 1 << 16
    total = 4 * len(singles) + len(ns) + 4 * len(offs) + 4 * len(ces) + sum(len(k.encode()) + 20 for k, _, _ in crow)
    out = ['// GENERATED by tools/gen_cldr.py from CLDR 46 (allkeys_CLDR.txt, UCA 16.0.0, and common/collation/*.xml) — do not edit.',
           '//! The CLDR root collation and the tailorings of de (phonebook), es (standard, traditional), sv, da, tr, ja and zh',
           '//! (pinyin), resolved against the root weights. Encoding: see tools/gen_cldr.py.', '']
    out.append('/// Root: single code points (sorted; canonically decomposable ones left out — the collator works on NFD).')
    out.append('pub static ROOT_CP: &[u32] = &[%s];' % ','.join(map(str, singles)))
    out.append('/// Root: the number of collation elements of each ROOT_CP entry.')
    out.append('pub static ROOT_N: &[u8] = &[%s];' % ','.join(map(str, ns)))
    out.append('/// Root: the ROOT_CES index of every 64th ROOT_CP entry.')
    out.append('pub static ROOT_OFF: &[u32] = &[%s];' % ','.join(map(str, offs)))
    out.append('/// Root collation elements: primary << 16 | variable << 15 | secondary << 5 | tertiary.')
    out.append('pub static ROOT_CES: &[u32] = &[%s];' % ','.join(map(str, ces)))
    out.append('/// Root contractions (NFD strings, sorted by bytes): (key, first element, count).')
    out.append('pub static ROOT_CONTR: &[(&str, u16, u8)] = &[%s];' % ', '.join('(%s, %d, %d)' % (rs_str(k), a, n) for k, a, n in crow))
    out.append(TAILORING_STRUCT)
    trows = []
    for tid, f, ty in TAILORINGS:
        co = Coll(root)
        opts = {'case_upper': False, 'reorder': []}
        a_ce = co.ces('a')[0]
        if tid == 'zh':
            # [reorder Hani]: the tailored Han characters take the gap just below the first Latin letter (after the
            # digits, before every script).
            lr = (a_ce[0] - 0xC000, COMMON_S, COMMON_T, 0)
        else:
            # ja [reorder Latn Kana Hani]: the JIS kanji order directly after the last kana.
            kana_max = max(c[0] for cp in range(0x3041, 0x3100) for c in co.ces(chr(cp)) if c[0] < 0xFB00 << 16)
            lr = (kana_max + 0x1000, COMMON_S, COMMON_T, 0)
        apply_rules(d, co, rules_of(d, f, ty), opts, lr)
        anchors = []
        single, single_ce, multi, tces = [], [], [], []
        for key in sorted(co.tail, key=lambda k: ''.join(map(chr, k)).encode('utf-8')):
            cl = co.tail[key]
            assert len(cl) < 32
            if len(key) == 1:
                single.append(key[0])
                if len(cl) == 1 and cl[0][1] == COMMON_S and cl[0][2] == COMMON_T and cl[0][3] == 0:
                    base = (cl[0][0] >> 16) << 16
                    off = cl[0][0] - base
                    a = pack_wide((base, COMMON_S, COMMON_T, 0))
                    if a not in anchors:
                        anchors.append(a)
                    assert anchors.index(a) < 0x8000 and off < 0x10000
                    single_ce.append(0x80000000 | (anchors.index(a) << 16) | off)
                    continue
                single_ce.append((len(tces) << 5) | len(cl))
            else:
                multi.append((''.join(map(chr, key)), (len(tces) << 5) | len(cl)))
            tces += [pack_wide(c) for c in cl]
        order = sorted(range(len(single)), key=lambda k: single[k])
        single = [single[k] for k in order]
        single_ce = [single_ce[k] for k in order]
        name = const_name(tid)
        out.append('static %s_SINGLE: &[u32] = &[%s];' % (name, ','.join(map(str, single))))
        out.append('static %s_SINGLE_CE: &[u32] = &[%s];' % (name, ','.join(map(str, single_ce))))
        out.append('static %s_MULTI: &[(&str, u32)] = &[%s];' % (name, ', '.join('(%s, %d)' % (rs_str(k), x) for k, x in multi)))
        out.append('static %s_CES: &[u64] = &[%s];' % (name, ','.join(map(str, tces))))
        out.append('static %s_ANCHORS: &[u64] = &[%s];' % (name, ','.join(map(str, anchors))))
        trows.append('    Tailoring { id: "%s", case_upper: %s, single: %s_SINGLE, single_ce: %s_SINGLE_CE, multi: %s_MULTI, ces: %s_CES, anchors: %s_ANCHORS },'
                     % (tid, 'true' if opts['case_upper'] else 'false', name, name, name, name, name))
        tb = 8 * len(single) + sum(len(k.encode()) + 20 for k, _ in multi) + 8 * len(tces) + 8 * len(anchors)
        total += tb
        print('  tailoring %-10s single %6d multi %4d ces %5d bytes %d' % (tid, len(single), len(multi), len(tces), tb))
    out.append('\npub static TAILORINGS: &[Tailoring] = &[\n%s\n];\n' % '\n'.join(trows))
    out.insert(3, '//! Table bytes: %d.' % total)
    open(os.path.join(OUT_DIR, 'collation.rs'), 'w', encoding='utf-8').write('\n'.join(out))
    print('collation: root singles', len(singles), 'contractions', len(crow), 'ces', len(ces), 'bytes', total)
    return total


if __name__ == '__main__':
    if sys.argv[1] == 'fetch':
        fetch(sys.argv[2])
    else:
        D = sys.argv[2]
        os.makedirs(OUT_DIR, exist_ok=True)
        gen_tables()
        if len(sys.argv) < 4 or sys.argv[3] != '--no-collation':
            gen_collation(D)
