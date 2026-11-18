# l106fix.py <branch> <SRid> [line=106]: rebuild handlers.md line from HEAD's + the branch's `BUILT by <SRid> (...)` clause at its anchor (for an rc=3 "no clause" stop).
import subprocess,sys
b,sr=sys.argv[1],sys.argv[2]; ln=int(sys.argv[3]) if len(sys.argv)>3 else 106
f='docs/dev/blueprint/handlers.md'
ours=subprocess.run(['git','show',f'HEAD:{f}'],capture_output=True,text=True).stdout.split('\n')
theirs=subprocess.run(['git','show',f'{b}:{f}'],capture_output=True,text=True).stdout.split('\n')
o=ours[ln-1]; t=theirs[ln-1]
s=t.index(f' BUILT by {sr} ('); k=t.index('(',s); d=0
for e in range(k,len(t)):
    if t[e]=='(': d+=1
    elif t[e]==')':
        d-=1
        if d==0: break
clause=t[s:e+1]
for n in range(160,20,-1):
    anc=t[s-n:s]
    if o.count(anc)==1: break
else: raise SystemExit('no anchor')
p=o.index(anc)+len(anc); ours[ln-1]=o[:p]+clause+o[p:]
open(f,'w').write('\n'.join(ours)); print('spliced',sr,repr(anc[-50:]),len(clause))
