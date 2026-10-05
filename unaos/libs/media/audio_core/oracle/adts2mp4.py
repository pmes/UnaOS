# AUDIOCODEC (SR30): wrap an ADTS AAC stream into MP4, independently of src/mp4.rs, for the container KATs.
# usage: adts2mp4.py in.aac out.m4a [--skip N] [--dur N] [--frag K] [--smpb] [--moov-last]
#   --skip/--dur : edit list media_time / segment duration in samples (no edit list when both are absent)
#   --frag K     : fragmented MP4, K samples per moof (mvex/trex in the moov, tfhd default-base-is-moof)
#   --smpb       : gapless info as an iTunSMPB atom instead of an edit list
#   --moov-last  : mdat before moov
import struct, sys
args = sys.argv[1:]
src, dst = args[0], args[1]
opt = lambda k, d=None: int(args[args.index(k) + 1]) if k in args else d
skip, dur, frag = opt('--skip'), opt('--dur'), opt('--frag')
smpb, moov_last = '--smpb' in args, '--moov-last' in args
d = open(src, 'rb').read()
aus, i, hdr = [], 0, None
while i + 7 <= len(d):
    assert d[i] == 0xFF and d[i + 1] & 0xF6 == 0xF0
    flen = ((d[i + 3] & 3) << 11) | (d[i + 4] << 3) | (d[i + 5] >> 5)
    hl = 7 if d[i + 1] & 1 else 9
    hdr = hdr or d[i:i + 7]
    aus.append(d[i + hl:i + flen]); i += flen
profile, sfi, chc = hdr[2] >> 6, (hdr[2] >> 2) & 15, ((hdr[2] & 1) << 2) | (hdr[3] >> 6)
rate = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350][sfi]
asc = struct.pack('>H', ((profile + 1) << 11) | (sfi << 7) | (chc << 3))
def box(t, *p): b = b''.join(p); return struct.pack('>I', 8 + len(b)) + t + b
def full(t, v, f, *p): return box(t, struct.pack('>I', (v << 24) | f), *p)
def desc(tag, b): return bytes([tag, 0x80, 0x80, 0x80, len(b)]) + b
esds = full(b'esds', 0, 0, desc(3, struct.pack('>HB', 1, 0) + desc(4, bytes([0x40, 0x15]) + b'\0\0\0' + struct.pack('>II', 0, 0) + desc(5, asc)) + desc(6, b'\x02')))
mp4a = box(b'mp4a', b'\0' * 6 + struct.pack('>H', 1) + b'\0' * 8 + struct.pack('>HHHHI', chc if chc != 7 else 8, 16, 0, 0, rate << 16), esds)
n = len(aus)
total = n * 1024
def stbl(aus_in_moov):
    k = len(aus_in_moov)
    return box(b'stbl', full(b'stsd', 0, 0, struct.pack('>I', 1), mp4a),
               full(b'stts', 0, 0, struct.pack('>III', 1 if k else 0, k, 1024) if k else struct.pack('>I', 0)),
               full(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, 1, 1) if k else struct.pack('>I', 0)),
               full(b'stsz', 0, 0, struct.pack('>II', 0, k) + b''.join(struct.pack('>I', len(a)) for a in aus_in_moov)),
               full(b'stco', 0, 0, struct.pack('>I', k) + b''.join(struct.pack('>I', 0xDEADBEEF) for _ in aus_in_moov)))
def moov(chunk_offsets, frag_mode):
    mvhd = full(b'mvhd', 0, 0, struct.pack('>IIII', 0, 0, rate, total) + struct.pack('>IH', 0x10000, 0x100) + b'\0' * 10 + struct.pack('>9I', 0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000) + b'\0' * 24 + struct.pack('>I', 2))
    tkhd = full(b'tkhd', 0, 7, struct.pack('>IIIII', 0, 0, 1, 0, total) + b'\0' * 8 + struct.pack('>HHHH', 0, 0, 0x100, 0) + struct.pack('>9I', 0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000) + struct.pack('>II', 0, 0))
    edts = b''
    if (skip is not None or dur is not None) and not smpb:
        sd = dur if dur is not None else total - (skip or 0)  # movie timescale = sample rate
        edts = box(b'edts', full(b'elst', 0, 0, struct.pack('>IIiI', 1, sd, skip or 0, 0x10000)))
    mdhd = full(b'mdhd', 0, 0, struct.pack('>IIII', 0, 0, rate, total) + struct.pack('>HH', 0x55c4, 0))
    hdlr = full(b'hdlr', 0, 0, struct.pack('>I', 0) + b'soun' + b'\0' * 12 + b'SoundHandler\0')
    st = stbl([] if frag_mode else aus)
    minf = box(b'minf', full(b'smhd', 0, 0, b'\0' * 4), box(b'dinf', full(b'dref', 0, 0, struct.pack('>I', 1), full(b'url ', 0, 1))), st)
    trak = box(b'trak', tkhd, edts, box(b'mdia', mdhd, hdlr, minf))
    extra = b''
    if frag_mode:
        extra = box(b'mvex', full(b'trex', 0, 0, struct.pack('>IIIII', 1, 1, 1024, 0, 0)))
    if smpb:
        txt = ' 00000000 %08X %08X %016X' % (skip or 0, total - (skip or 0) - (dur if dur is not None else total - (skip or 0)), dur if dur is not None else total - (skip or 0))
        ent = box(b'----', full(b'mean', 0, 0, b'com.apple.iTunes'), full(b'name', 0, 0, b'iTunSMPB'), box(b'data', struct.pack('>II', 1, 0) + txt.encode()))
        extra += box(b'udta', full(b'meta', 0, 0, full(b'hdlr', 0, 0, struct.pack('>I', 0) + b'mdir' + b'appl' + b'\0' * 9), box(b'ilst', ent)))
    m = box(b'moov', mvhd, trak, extra)
    # patch the chunk offsets
    for off in chunk_offsets:
        j = m.index(struct.pack('>I', 0xDEADBEEF)); m = m[:j] + struct.pack('>I', off) + m[j + 4:]
    return m
ftyp = box(b'ftyp', b'M4A ', struct.pack('>I', 0), b'M4A isomiso2')
if frag:
    out = ftyp + moov([], True)
    seq = 1
    for s in range(0, n, frag):
        grp = aus[s:s + frag]
        def moof(data_off):
            trun = full(b'trun', 0, 0x201, struct.pack('>Ii', len(grp), data_off) + b''.join(struct.pack('>I', len(a)) for a in grp))
            return box(b'moof', full(b'mfhd', 0, 0, struct.pack('>I', seq)), box(b'traf', full(b'tfhd', 0, 0x20000, struct.pack('>I', 1)), full(b'tfdt', 0, 0, struct.pack('>I', s * 1024)), trun))
        m0 = moof(0); mf = moof(len(m0) + 8)
        out += mf + box(b'mdat', *grp); seq += 1
else:
    mdat_payload = b''.join(aus)
    if moov_last:
        base = len(ftyp) + 8
        offs = []; o = base
        for a in aus: offs.append(o); o += len(a)
        out = ftyp + box(b'mdat', mdat_payload) + moov(offs, False)
    else:
        m0 = moov([0] * n, False)
        base = len(ftyp) + len(m0) + 8
        offs = []; o = base
        for a in aus: offs.append(o); o += len(a)
        out = ftyp + moov(offs, False) + box(b'mdat', mdat_payload)
open(dst, 'wb').write(out)
