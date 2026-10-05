# deterministic 48 kHz stereo s16 test signal: voiced speech-like pulses through formants, a chord,
# noise bursts, a transient train, a chirp, a quiet tail. usage: sig.py seconds out.raw
import math, struct, sys, random
sec = float(sys.argv[1]); N = int(48000 * sec); random.seed(7)
def biquad(x, f, q, fs=48000):
    w = 2*math.pi*f/fs; a = math.sin(w)/(2*q); c = math.cos(w)
    b0, b1, b2, a0, a1, a2 = a, 0, -a, 1+a, -2*c, 1-a
    y = []; x1 = x2 = y1 = y2 = 0.0
    for v in x:
        o = (b0*v + b1*x1 + b2*x2 - a1*y1 - a2*y2)/a0
        x2, x1, y2, y1 = x1, v, y1, o; y.append(o)
    return y
# glottal pulse train with drifting pitch 110..220 Hz
src = []; ph = 0.0
for n in range(N):
    f0 = 140 + 60*math.sin(2*math.pi*0.7*n/48000)
    ph += f0/48000
    src.append(1.0 if ph >= 1.0 else 0.0)
    if ph >= 1.0: ph -= 1.0
sp = [0.0]*N
for f, q, g in ((700, 8, 1.0), (1220, 10, 0.6), (2600, 12, 0.3), (3400, 12, 0.15)):
    y = biquad(src, f, q)
    for i in range(N): sp[i] += g*y[i]
L = []; R = []
for n in range(N):
    t = n/48000
    seg = (t*4/sec)
    l = r = 0.0
    env = 0.5*(1-math.cos(2*math.pi*min(1, (t % 0.5)/0.5)))
    if seg < 1.5:
        v = 0.9*sp[n]*env; l += v; r += 0.8*v
    if 1.0 <= seg < 2.5:
        for k, f in enumerate((261.6, 329.6, 392.0, 523.3, 1046.5)):
            a = 0.08/(k+1)**0.5
            l += a*math.sin(2*math.pi*f*t + k); r += a*math.sin(2*math.pi*f*1.003*t + 2*k)
    if 2.0 <= seg < 3.0:
        g = 0.15 if int(t*10) % 2 else 0.03
        l += g*(random.random()*2-1); r += g*(random.random()*2-1)
    if 2.5 <= seg < 3.5:
        if (n % 4800) < 48: l += 0.7*(1 - (n % 4800)/48); r -= 0.6*(1 - (n % 4800)/48)
        f = 100*math.exp(t*2.5); l += 0.1*math.sin(2*math.pi*f*t); r += 0.1*math.cos(2*math.pi*f*t)
    if seg >= 3.5:
        l += 0.002*math.sin(2*math.pi*440*t); r += 0.001*(random.random()*2-1)
    L.append(l); R.append(r)
with open(sys.argv[2], 'wb') as fo:
    for a, b in zip(L, R):
        fo.write(struct.pack('<hh', max(-32768, min(32767, int(a*20000))), max(-32768, min(32767, int(b*20000)))))
