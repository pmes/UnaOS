# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: python3 derive.py ~/unaos-bench/scratch/rmbp-0915/logs/foldgate/f12-boot1.log
# TPFRAME M1 derivation B: from the flight-12 corpus alone (no wsp table consulted here).
import re,sys
frames=[]
for line in open(sys.argv[1] if len(sys.argv)>1 else 'f12-boot1.log',errors='replace'):
    if 'raw report #' in line and '(58 B)' in line:
        hexs=line.split('(58 B):')[1].split('==')[0].split()
        frames.append(bytes(int(h,16) for h in hexs))
print('frames',len(frames),[len(f) for f in frames])
def le16(f,o): v=f[o]|f[o+1]<<8; return v-65536 if v>=32768 else v
print('constant bytes:',[o for o in range(58) if len({f[o] for f in frames})==1])
print('varying bytes:',[(o,[f[o] for f in frames]) for o in range(58) if len({f[o] for f in frames})>1])
# le16 fields at even offsets: monotonic?
for o in range(0,57,2):
    vs=[le16(f,o) for f in frames]
    d=[vs[i+1]-vs[i] for i in range(len(vs)-1)]
    mono = all(x>0 for x in d) or all(x<0 for x in d)
    print(f'le16@{o:2d} values={vs} deltas={d}{" MONOTONIC" if mono else ""}')
# pairs (a,b): value at b == k * delta of value at a, for consecutive frames
for a in range(0,57,2):
    for b in range(0,57,2):
        if a==b: continue
        for k in (1,2,4,8,10,16):
            ok=True; n=0
            for i in range(len(frames)-1):
                da=le16(frames[i+1],a)-le16(frames[i],a)
                if da==0 or le16(frames[i+1],b)!=k*da: ok=False;break
                n+=1
            if ok and n: print(f'le16@{b} == {k} x delta(le16@{a}) on {n}/{len(frames)-1} consecutive pairs')
