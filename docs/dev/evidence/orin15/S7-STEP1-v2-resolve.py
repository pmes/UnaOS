#!/usr/bin/env python3
"""S7RECUT — resolve the two merge conflicts of (pi 8131cd2d, base 33dc7811, v1) and
fold pi's two orphaned witnesses onto their host statements in the appended tail block."""
import sys

L = open('merged.rs').read().split('\n')
w_hb = open('w_hb.txt').read()
w_stk = open('w_stk.txt').read()

# ---- conflict 1: lines 5369..5381 (1-based) ------------------------------------------------
assert L[5368] == '<<<<<<< pi' and L[5374] == '=======' and L[5380] == '>>>>>>> v1'
pi_side = L[5369:5374]          # 5 lines
v1_side = L[5375:5380]          # 5 lines
assert len(pi_side) == 5 and len(v1_side) == 5

# v1's livecon line, but with pi's own comment text (pi says `video/pidesk.rs`, jetson says
# `video/desktop_firmware.rs`): rebuild it by stripping the `let t0 = …;` host off pi's line.
pfx = '        let t0 = unaos_kernel::arch::now_cycles(); '
assert pi_side[3].startswith(pfx)
livecon_pi = '        ' + pi_side[3][len(pfx):]
assert livecon_pi.replace('video/pidesk.rs', 'video/desktop_firmware.rs') == v1_side[3]

res1 = list(v1_side)
res1[3] = livecon_pi
assert len(res1) == 5

# ---- conflict 2: pi's inline [sched6] census vs v1's prose --------------------------------
i = L.index('<<<<<<< pi', 5381)
j = L.index('=======', i)
k = L.index('>>>>>>> v1', j)
pi2 = L[i+1:j]
v12 = L[j+1:k]
assert len(pi2) == len(v12) == 29, (len(pi2), len(v12))
# the stk_probe witness rides `prio_witness()`, which moves verbatim into ChannelWait::census.
assert any('stk_probe("render:pass")' in x for x in pi2)
res2 = list(v12)

out = L[:5368] + res1 + L[5381:i] + res2 + L[k+1:]
assert '<<<<<<< ' not in '\n'.join(out) and '>>>>>>> ' not in '\n'.join(out)

# ---- fold the two orphaned witnesses onto their host statements in ChannelWait -------------
hb_host = '        self.passes += 1;'
stk_host = '            unaos_kernel::arch::sched::prio_witness();'
n_hb = n_stk = 0
for n, l in enumerate(out):
    if l == hb_host:
        out[n] = l + ' ' + w_hb
        n_hb += 1
    elif l == stk_host:
        out[n] = l + ' ' + w_stk
        n_stk += 1
assert n_hb == 1 and n_stk == 1, (n_hb, n_stk)

open('after_v2.rs', 'w').write('\n'.join(out))
print('conflict1 %d->%d  conflict2 %d->%d  hb=%d stk=%d  lines %d->%d'
      % (len(pi_side), len(res1), len(pi2), len(res2), n_hb, n_stk,
         len(open('pi.rs').read().split('\n')), len(out)))
