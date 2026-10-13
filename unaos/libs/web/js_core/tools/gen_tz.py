#!/usr/bin/env python3
"""INTLCLDR (LEDGER SR137): generate src/tzdata/zones.rs from the IANA time zone database, release 2025b.

Usage:
  python3 -I tools/gen_tz.py fetch <dir>      sparse fetch of the tz source files (eggert/tz on GitHub, tag 2025b)
  python3 -I tools/gen_tz.py gen <dir>        compile <dir> with the reference compiler zic(8) and write the table

The fetch is a generation-time step, never a test: the generated table is committed. `gen` runs
`zic -b fat -d <dir>/zic` over the nine source files (zic is the tz project's own compiler; `-b fat` writes every
transition up to 2037 explicitly and the POSIX TZ rule for the years after it), reads each Zone's TZif v2 body and
footer (RFC 8536), and writes, per Zone:

  types  the local time types as `utoff_seconds * 2 + isdst` (types[0] is the type before the first transition);
  data   one record per transition: zigzag-varint delta (seconds) from the previous transition instant (the first
         from 0), then one byte, the index into `types`; transitions that change neither offset nor isdst dropped;
  posix  the TZif footer (the rule after the last transition);
  cc     the ISO 3166 country of zone.tab ("" when the zone is not in zone.tab).

Links (the `backward` file and the Link lines of the others) map to their target Zone; a Link that zone.tab lists
under its own country is a primary identifier (ECMA-262 §21.4.1.20 note: a Link to a Zone of another country is a
primary identifier), every other Link is an alias of its target.
"""
import os, sys, struct, subprocess, urllib.request

TAG = '2025b'
URL = 'https://raw.githubusercontent.com/eggert/tz/%s/' % TAG
SOURCES = ['africa', 'antarctica', 'asia', 'australasia', 'europe', 'northamerica', 'southamerica', 'etcetera', 'backward']
FILES = SOURCES + ['zone.tab', 'zone1970.tab']
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'src', 'tzdata', 'zones.rs')


def fetch(d):
    os.makedirs(d, exist_ok=True)
    for f in FILES:
        p = os.path.join(d, f)
        if not os.path.exists(p):
            open(p, 'wb').write(urllib.request.urlopen(URL + f, timeout=60).read())


def read_tzif(path):
    b = open(path, 'rb').read()
    assert b[:4] == b'TZif', path

    def hdr(o):
        return struct.unpack('>6l', b[o + 20:o + 44])
    isut, isstd, leap, timecnt, typecnt, charcnt = hdr(0)
    o = 44 + timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut
    isut, isstd, leap, timecnt, typecnt, charcnt = hdr(o)
    p = o + 44
    times = struct.unpack('>%dq' % timecnt, b[p:p + timecnt * 8])
    p += timecnt * 8
    idx = b[p:p + timecnt]
    p += timecnt
    types = []
    for _ in range(typecnt):
        off, dst, _ai = struct.unpack('>lBB', b[p:p + 6])
        p += 6
        types.append((off, dst))
    p += charcnt + leap * 12 + isstd + isut
    footer = b[p:].strip().decode()
    return types, list(zip(times, idx)), footer


def varint(n):
    z = (n << 1) ^ (n >> 63)
    out = bytearray()
    while True:
        c = z & 0x7f
        z >>= 7
        if z:
            out.append(c | 0x80)
        else:
            out.append(c)
            return out


def rs_bytes(b):
    return ''.join(chr(c) if 0x20 <= c < 0x7f and c not in (0x22, 0x5c) else '\\x%02x' % c for c in b)


def gen(d):
    zones, links = [], {}
    for f in SOURCES:
        for ln in open(os.path.join(d, f), encoding='utf-8'):
            t = ln.split('#', 1)[0].split()
            if not t:
                continue
            if t[0] == 'Zone':
                zones.append(t[1])
            elif t[0] == 'Link':
                links[t[2]] = t[1]
    cc = {}
    for ln in open(os.path.join(d, 'zone.tab'), encoding='utf-8'):
        if ln.startswith('#') or not ln.strip():
            continue
        t = ln.rstrip('\n').split('\t')
        cc[t[2]] = t[0]
    out = os.path.join(d, 'zic')
    os.makedirs(out, exist_ok=True)
    subprocess.run(['zic', '-b', 'fat', '-d', out] + [os.path.join(d, f) for f in SOURCES], check=True)
    zones = sorted(set(zones))
    zi = {z: i for i, z in enumerate(zones)}
    rows, total, ntr = [], 0, 0
    for z in zones:
        types, tr, footer = read_tzif(os.path.join(out, z))
        kinds = [types[0]]
        blob = bytearray()
        prev_t, cur = 0, types[0]
        for t, i in tr:
            k = types[i]
            if k == cur:
                continue
            if k not in kinds:
                kinds.append(k)
            blob += varint(t - prev_t)
            blob.append(kinds.index(k))
            prev_t, cur = t, k
            ntr += 1
        assert len(kinds) < 256
        total += len(blob) + 4 * len(kinds) + len(footer) + len(z)
        rows.append('    Zone { id: "%s", types: &[%s], data: b"%s", posix: "%s", cc: "%s" },' % (
            z, ', '.join(str(o * 2 + s) for o, s in kinds), rs_bytes(blob), footer, cc.get(z, '')))
    lrows = []
    for a in sorted(links):
        tgt = links[a]
        while tgt in links:
            tgt = links[tgt]
        primary = a in cc
        total += len(a) + 3
        lrows.append('    Link { id: "%s", target: %d, primary: %s },' % (a, zi[tgt], 'true' if primary else 'false'))
    hdr = '''// GENERATED by tools/gen_tz.py from the IANA tz database %s (eggert/tz, compiled by zic -b fat) — do not edit.
//! The %d Zones of tzdata %s with every transition (%d) up to 2037 and the POSIX rule after it, and its %d Links.
//! Table bytes (transition blobs, type lists, rules, ids): %d. Encoding: see tools/gen_tz.py.

pub struct Zone {
    pub id: &'static str,
    /// Local time types: UTC offset in seconds * 2 + isdst; `types[0]` holds before the first transition.
    pub types: &'static [i32],
    /// Per transition: zigzag-varint delta in seconds from the previous instant (the first from 0), type index.
    pub data: &'static [u8],
    /// The POSIX TZ rule in force after the last transition (the TZif v2 footer).
    pub posix: &'static str,
    /// The zone.tab country ("" for none).
    pub cc: &'static str,
}

pub struct Link {
    pub id: &'static str,
    /// Index into `ZONES`.
    pub target: u16,
    /// Listed in zone.tab under its own country: a primary identifier that shares its target's data.
    pub primary: bool,
}

pub static TZ_VERSION: &str = "%s";

/// Sorted by id (byte order).
pub static ZONES: &[Zone] = &[
''' % (TAG, len(zones), TAG, ntr, len(links), total, TAG)
    src = hdr + '\n'.join(rows) + '\n];\n\n/// Sorted by id (byte order).\npub static LINKS: &[Link] = &[\n' + '\n'.join(lrows) + '\n];\n'
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    open(OUT, 'w', encoding='utf-8').write(src)
    print('zones', len(zones), 'links', len(links), 'transitions', ntr, 'table bytes', total)


if __name__ == '__main__':
    if sys.argv[1] == 'fetch':
        fetch(sys.argv[2])
    else:
        gen(sys.argv[2])
