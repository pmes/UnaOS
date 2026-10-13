# TEMPORAL (SR117): generates tzdata.rs. Run: python3 -I gen_tzdata.py ../intl/data.rs tzdata.rs  (reads /usr/share/zoneinfo, tzdata 2025b)
import struct, re, sys
src = open(sys.argv[1]).read()
zones = re.findall(r'^\s*"([A-Za-z_/]+)", "([^"]+)", \d+;', src, re.M)
def read(z):
    b = open('/usr/share/zoneinfo/'+z,'rb').read()
    assert b[:4]==b'TZif'
    def hdr(o):
        return struct.unpack('>6l', b[o+20:o+44])
    isut,isstd,leap,timecnt,typecnt,charcnt = hdr(0)
    o = 44 + timecnt*5 + typecnt*6 + charcnt + leap*8 + isstd + isut
    isut,isstd,leap,timecnt,typecnt,charcnt = hdr(o)
    p = o+44
    times = struct.unpack('>%dq'%timecnt, b[p:p+timecnt*8]); p+=timecnt*8
    idx = b[p:p+timecnt]; p+=timecnt
    types=[]
    for i in range(typecnt):
        off,dst,ai = struct.unpack('>lBB', b[p:p+6]); p+=6; types.append(off)
    p += charcnt + leap*12 + isstd + isut
    footer = b[p:].strip().decode()
    # initial offset: first type (LMT) per RFC 8536 (type 0)
    return types[0], [(t, types[i]) for t,i in zip(times, idx)], footer
tot=0; nonmin=0; mx=0
for z,posix in zones:
    init, tr, foot = read(z)
    # drop no-op transitions
    out=[]; cur=init
    for t,o in tr:
        if o!=cur: out.append((t,o)); cur=o
    tot+=len(out); mx=max(mx,len(out))
    nonmin += sum(1 for t,o in out if t%60 or o%60)
    if foot!=posix: print('FOOTER DIFF', z, foot, posix)
print('transitions',tot,'max',mx,'non-minute',nonmin)

# ---- POSIX rule evaluation (mirrors src/tz.rs) to drop the tail the current rule already reproduces
import datetime
def parse_posix(s):
    i=0
    def name():
        nonlocal i
        if s[i]=='<':
            j=s.index('>',i); n=s[i+1:j]; i=j+1; return n
        j=i
        while i<len(s) and s[i].isalpha(): i+=1
        return s[j:i]
    def tm():
        nonlocal i
        neg=False
        if s[i] in '+-': neg = s[i]=='-'; i+=1
        parts=[]
        while True:
            j=i
            while i<len(s) and s[i].isdigit(): i+=1
            parts.append(int(s[j:i]))
            if i<len(s) and s[i]==':': i+=1; continue
            break
        v=parts[0]*3600+(parts[1]*60 if len(parts)>1 else 0)+(parts[2] if len(parts)>2 else 0)
        return -v if neg else v
    def rule():
        nonlocal i
        if s[i]=='M':
            i+=1; j=s.index(',',i) if ',' in s[i:] else len(s)
            m=re.match(r'(\d+)\.(\d+)\.(\d+)(?:/(-?\d+(?::\d+)?))?', s[i:]); i+=m.end()
            t = 7200
            if m.group(4):
                hh=m.group(4); neg=hh.startswith('-'); hh=hh.lstrip('-').split(':')
                t=int(hh[0])*3600+(int(hh[1])*60 if len(hh)>1 else 0); t=-t if neg else t
            return ('M',int(m.group(1)),int(m.group(2)),int(m.group(3)),t)
        if s[i]=='J':
            i+=1; m=re.match(r'(\d+)(?:/(-?\d+))?', s[i:]); i+=m.end()
            return ('J',int(m.group(1)),0,0,int(m.group(2) or 2)*3600)
        m=re.match(r'(\d+)(?:/(-?\d+))?', s[i:]); i+=m.end()
        return ('N',int(m.group(1)),0,0,int(m.group(2) or 2)*3600)
    sn=name(); so=-tm()
    if i>=len(s): return (so,None)
    dn=name()
    do = -tm() if s[i] not in ',' else so+3600
    i+=1; a=rule(); i+=1; b=rule()
    return (so,(do,a,b))
def rule_day(y,r):
    import calendar
    if r[0]=='M':
        _,m,w,d,_t=r
        first=datetime.date(y,m,1); dim=calendar.monthrange(y,m)[1]
        fwd=(first.weekday()+1)%7
        dd=(d-fwd+7)%7+7*(w-1)
        while dd>=dim: dd-=7
        return (first-datetime.date(1970,1,1)).days+dd
    if r[0]=='J':
        n=r[1]; leap=calendar.isleap(y); extra=1 if leap and n>=60 else 0
        return (datetime.date(y,1,1)-datetime.date(1970,1,1)).days+n-1+extra
    return (datetime.date(y,1,1)-datetime.date(1970,1,1)).days+r[1]
def off_at(p,t):
    so,d=p
    if d is None: return so
    do,a,b=d
    y=(datetime.datetime(1970,1,1)+datetime.timedelta(seconds=t+so)).year
    s=rule_day(y,a)*86400+a[4]-so; e=rule_day(y,b)*86400+b[4]-do
    ind = (s<=t<e) if s<e else not (e<=t<s)
    return do if ind else so
import calendar
total=0; out_rs=[]
for z,posix in zones:
    init,tr,foot=read(z); p=parse_posix(posix)
    out=[]; cur=init
    for t,o in tr:
        if o!=cur: out.append((t,o)); cur=o
    # keep the shortest prefix after which the rule reproduces every remaining transition and the offset between them
    k=len(out)
    while k>0:
        t,o=out[k-1]
        prev = out[k-2][1] if k>=2 else init
        if off_at(p,t)==o and off_at(p,t-1)==prev: k-=1
        else: break
    if z=='Africa/Casablanca': k=len(out)
    # the rule must not invent transitions the zone never had: none strictly inside any gap after the last kept one
    def rule_between(a, b):
        so,d=p
        if d is None: return False
        do,ra,rb=d
        y0 = 1800 if a is None else (datetime.datetime(1970,1,1)+datetime.timedelta(seconds=a)).year - 1
        y1 = (datetime.datetime(1970,1,1)+datetime.timedelta(seconds=b)).year + 1
        for y in range(y0, y1+1):
            for x in (rule_day(y,ra)*86400+ra[4]-so, rule_day(y,rb)*86400+rb[4]-do):
                if (a is None or x > a) and x < b: return True
        return False
    while k < len(out):
        bad = False
        bounds = [out[k-1][0] if k>0 else None] + [t for t,o in out[k:]]
        for a,b in zip(bounds, bounds[1:]):
            if rule_between(a, b): bad = True; break
        if bad: k += 1
        else: break
    kept=out[:k]
    total+=len(kept)
    out_rs.append((z,init,kept))
print('kept',total)
outside=sum(1 for z,i,k in out_rs for t,o in k if not (-2**31<=t<2**31))
offs=set(o for z,i,k in out_rs for t,o in k)|set(i for z,i,k in out_rs)
mxo=max(len(set([i]+[o for t,o in k])) for z,i,k in out_rs)
print('outside i32',outside,'distinct offsets',len(offs),'max per zone',mxo)

# ---- emit the Rust table
def varint(n):
    z = (n << 1) ^ (n >> 63)  # zigzag
    out = bytearray()
    while True:
        b = z & 0x7f; z >>= 7
        if z: out.append(b | 0x80)
        else: out.append(b); break
    return out
lines=[]; total_bytes=0
for z,init,kept in out_rs:
    offs=[init]
    for t,o in kept:
        if o not in offs: offs.append(o)
    blob=bytearray(); prev=0
    for t,o in kept:
        blob += varint(t-prev); blob.append(offs.index(o)); prev=t
    total_bytes += len(blob) + 4*len(offs)
    lit=''.join('\\x%02x'%c for c in blob)
    lines.append('    Hist { id: "%s", offs: &[%s], data: b"%s" },' % (z, ', '.join(str(o) for o in offs), lit))
hdr = '''//! GENERATED by gen_tzdata.py from tzdata 2025b (the TZif v2+ files of /usr/share/zoneinfo) — do not edit.
//! The historical UTC offsets of the 84 IANA zones of `intl::data::ZONES`, up to the instant from which the
//! zone's current POSIX rule (`Zone::posix`) reproduces every later transition. Per zone: `offs[0]` is the
//! offset in effect before the first transition (local mean time); `data` is a run of (zigzag-varint delta
//! of the transition instant in epoch seconds from the previous one, one byte index into `offs`).
//! %d transitions, %d bytes of table (blobs + offset lists).

pub struct Hist {
    pub id: &'static str,
    /// UTC offsets in seconds east of Greenwich.
    pub offs: &'static [i32],
    pub data: &'static [u8],
}

pub static HIST: &[Hist] = &[
''' % (sum(len(k) for _,_,k in out_rs), total_bytes)
open(sys.argv[2],'w').write(hdr + '\n'.join(lines) + '\n];\n')
print('bytes', total_bytes)
