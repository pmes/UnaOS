# rowreplay2.py <arc-sha> [line=90]: at a fold STOP with a conflict block at <line> of handlers.md — HEAD's line + the ARC COMMIT's own token edits (each applied at the FIRST matching left-context occurrence). Prints FAILs (other-copy edits) without stopping.
import subprocess,difflib,sys
arc=sys.argv[1]; ln=int(sys.argv[2]) if len(sys.argv)>2 else 90; f='docs/dev/blueprint/handlers.md'
g=lambda r: subprocess.run(['git','show',f'{r}:{f}'],capture_output=True,text=True).stdout.split('\n')[ln-1]
a,b,o=g(arc+'^'),g(arc),g('HEAD')
ta,tb=a.split(' '),b.split(' ')
sm=difflib.SequenceMatcher(None,ta,tb,autojunk=False); out=o; seen=set(); ok=0
for op,i1,i2,j1,j2 in sm.get_opcodes():
    if op=='equal': continue
    old=' '.join(ta[i1:i2]); new=' '.join(tb[j1:j2])
    for n in (8,6,4,3,2):
        ctx=' '.join(ta[max(0,i1-n):i1]); key=(ctx+' '+old) if old else ctx
        if out.count(key)>=1 or (key,new) in seen: break
    else: print('FAIL',repr(old[:60]),'->',repr(new[:60])); continue
    rep=ctx+((' '+new) if new else '')
    if (key,rep) in seen: continue
    c=out.count(key); p=out.index(key); out=out[:p]+rep+out[p+len(key):]; seen.add((key,rep)); seen.add((key,new)); ok+=1
    print('applied first of',c,repr(old[:40]),'->',repr(new[:60]),len(new))
print('applied',ok,len(o),'->',len(out))
if ok:
    L=open(f).read().split('\n'); i=ln-1; assert L[i].startswith('<<<<<<<'); L[i:i+5]=[out]; open(f,'w').write('\n'.join(L))
