# Build the synthetic GSUB 1.1 FeatureVariations probe and parse it back independently.
import struct
def u16(*v): return b''.join(struct.pack('>H', x) for x in v)
def u32(*v): return b''.join(struct.pack('>I', x) for x in v)
b = bytearray()
b += u16(1, 1) + u16(14, 34, 48) + u32(94)              # header 0..14
b += u16(1) + b'DFLT' + u16(8)                           # ScriptList 14..24
b += u16(4) + u16(0)                                     # Script 24..28
b += u16(0, 0xFFFF, 1, 0)                                # LangSys 28..36
b += u16(1) + b'ss01' + u16(8)                           # FeatureList 36..44
b += u16(0, 1, 0)                                        # Feature ss01 -> lookup 0, 44..50
b += u16(2, 6, 26)                                       # LookupList 50..56
for delta in (1, 2):                                     # two type-1 single substitutions, 20 B each
    b += u16(1, 0, 1, 8) + u16(1, 6, delta) + u16(1, 1, 1)
assert len(b) == 94
b += u16(1, 0) + u32(1) + u32(16, 30)                    # FeatureVariations 96..112
b += u16(1) + u32(6)                                     # ConditionSet 112..118
b += u16(1, 0, 0x2000, 0x4000)                           # Condition fmt 1: axis 0 in [0.5, 1.0], 118..126
b += u16(1, 0, 1) + u16(0) + u32(12)                     # FeatureTableSubstitution 126..138
b += u16(0, 1, 1)                                        # alternate Feature -> lookup 1, 138..144
assert len(b) == 142
# independent parse
g = bytes(b)
maj, mi, sl, fl, ll, fv = struct.unpack('>HHHHHI', g[:14])
assert (maj, mi, sl, fl, ll, fv) == (1, 1, 14, 34, 48, 94)
fcount = struct.unpack('>H', g[fl:fl+2])[0]; ftag = g[fl+2:fl+6]; foff = struct.unpack('>H', g[fl+6:fl+8])[0]
feat = g[fl+foff:]; assert struct.unpack('>HHH', feat[:6]) == (0, 1, 0)
lc, l0, l1 = struct.unpack('>HHH', g[ll:ll+6]); assert lc == 2
for lo, d in ((l0, 1), (l1, 2)):
    L = g[ll+lo:]; t, f, n, so = struct.unpack('>HHHH', L[:8]); st = L[so:]
    fm, co, dg = struct.unpack('>HHH', st[:6]); cov = st[co:]
    assert (t, n, fm, dg) == (1, 1, 1, d) and struct.unpack('>HHH', cov[:6]) == (1, 1, 1)
t = g[fv:]; vmaj, vmi, cnt = struct.unpack('>HHI', t[:8]); cs, fts = struct.unpack('>II', t[8:16])
c = t[cs:]; cc = struct.unpack('>H', c[:2])[0]; co = struct.unpack('>I', c[2:6])[0]
fmt, ax, lo, hi = struct.unpack('>HHhh', c[co:co+8]); assert (fmt, ax, lo/16384, hi/16384) == (1, 0, 0.5, 1.0)
s = t[fts:]; smaj, smi, sc, fi, ao = struct.unpack('>HHHHI', s[:12]); alt = s[ao:]
assert (sc, fi) == (1, 0) and struct.unpack('>HHH', alt[:6]) == (0, 1, 1)
print('probe ok, %d bytes' % len(g))
rows = []
for i in range(0, len(g), 16):
    rows.append('    ' + ', '.join('0x%02X' % x for x in g[i:i+16]) + ',')
print('\n'.join(rows))
