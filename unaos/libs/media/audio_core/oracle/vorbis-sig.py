# deterministic multi-channel test signal at any rate: usage sig2.py seconds rate channels out.raw
import math, struct, sys, random
sec=float(sys.argv[1]); fs=int(sys.argv[2]); ch=int(sys.argv[3]); N=int(fs*sec); random.seed(11)
out=bytearray()
ph=[0.0]*ch
for n in range(N):
    t=n/fs; seg=t/sec
    for c in range(ch):
        v=0.0
        f0=110*(1+c*0.25)*(1+0.3*math.sin(2*math.pi*0.5*t))
        ph[c]+=f0/fs
        # band-limited-ish sawtooth (few harmonics), envelope
        for h in range(1,8):
            if h*f0 < fs/2.2: v+=0.25/h*math.sin(2*math.pi*h*ph[c])
        v*=0.5*(1-math.cos(2*math.pi*min(1,(t%0.75)/0.75)))
        if 0.3<seg<0.6: v+=0.08*(random.random()*2-1)
        if seg>0.5 and (n % int(fs*0.1))<fs//400: v+=0.6*(1-(n % int(fs*0.1))/(fs/400))
        f=200*math.exp(seg*3); 
        if f<fs/2.2: v+=0.05*math.sin(2*math.pi*f*t+c)
        out+=struct.pack('<h',max(-32768,min(32767,int(v*16000))))
open(sys.argv[4],'wb').write(out)
