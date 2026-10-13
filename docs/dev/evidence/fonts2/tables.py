import struct, sys
for p in sys.argv[1:]:
    d = open(p,'rb').read()
    n = struct.unpack('>H', d[4:6])[0]
    tags = {}
    for i in range(n):
        rec = d[12+16*i:12+16*i+16]
        tag = rec[:4].decode('latin1'); off, ln = struct.unpack('>II', rec[8:16])
        tags[tag] = (off, ln)
    print(p, sorted(tags))
    if 'MVAR' in tags:
        off, ln = tags['MVAR']; m = d[off:off+ln]
        maj, mi, res, vrs, vrc, ivs = struct.unpack('>HHHHHH', m[:12])
        recs = [(m[12+vrs*i:12+vrs*i+4].decode('latin1'),)+struct.unpack('>HH', m[12+vrs*i+4:12+vrs*i+8]) for i in range(vrc)]
        print('  MVAR v%d.%d recsize=%d count=%d ivs=%d' % (maj,mi,vrs,vrc,ivs), recs)
    if 'GSUB' in tags:
        off, ln = tags['GSUB']; g = d[off:off+ln]
        maj, mi = struct.unpack('>HH', g[:4])
        fv = struct.unpack('>I', g[10:14])[0] if mi >= 1 else 0
        print('  GSUB v%d.%d featureVariations=%d' % (maj, mi, fv))
        if fv:
            t = g[fv:]
            vmaj, vmi, cnt = struct.unpack('>HHI', t[:8])
            print('   FV v%d.%d records=%d' % (vmaj, vmi, cnt))
            fl = struct.unpack('>H', g[6:8])[0]
            fcount = struct.unpack('>H', g[fl:fl+2])[0]
            feats = [g[fl+2+6*i:fl+2+6*i+4].decode('latin1') for i in range(fcount)]
            for r in range(cnt):
                cs, fts = struct.unpack('>II', t[8+8*r:16+8*r])
                c = t[cs:]; cc = struct.unpack('>H', c[:2])[0]
                conds = []
                for k in range(cc):
                    co = struct.unpack('>I', c[2+4*k:6+4*k])[0]
                    f, ax, lo, hi = struct.unpack('>HHhh', c[co:co+8])
                    conds.append((f, ax, lo/16384, hi/16384))
                s = t[fts:]; smaj, smi, sc = struct.unpack('>HHH', s[:6])
                subs = [struct.unpack('>HI', s[6+6*k:12+6*k]) for k in range(sc)]
                print('   rec', r, 'conds', conds, 'subs', [(fi, feats[fi]) for fi, _ in subs])
    if 'GPOS' in tags:
        off, ln = tags['GPOS']; g = d[off:off+ln]
        maj, mi = struct.unpack('>HH', g[:4])
        fv = struct.unpack('>I', g[10:14])[0] if mi >= 1 else 0
        print('  GPOS v%d.%d featureVariations=%d' % (maj, mi, fv))
