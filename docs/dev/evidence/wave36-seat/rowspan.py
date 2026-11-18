# rowspan.py <tip> <trunk-parent> [line=90]: the tip's row differs from its trunk parent's by ONE contiguous span (trunk's row + only the arc's clause). Replace that span's old text in HEAD's row with the new text. Handles a conflict block or a plain line.
import subprocess,sys,os
tip,base=sys.argv[1],sys.argv[2]; ln=int(sys.argv[3]) if len(sys.argv)>3 else 90; f='docs/dev/blueprint/handlers.md'
g=lambda r: subprocess.run(['git','show',f'{r}:{f}'],capture_output=True,text=True).stdout.split('\n')[ln-1]
a,b,o=g(base),g(tip),g('HEAD')
p=os.path.commonprefix([a,b]); s=os.path.commonprefix([a[::-1],b[::-1]])
p=len(p); s=min(len(s),len(a)-p,len(b)-p)
old,new=a[p:len(a)-s],b[p:len(b)-s]
print('span old',len(old),repr(old[:70]),'| new',len(new),repr(new[:70]))
for n in (200,120,80,50,30):
    ctx=a[max(0,p-n):p]; key=ctx+old
    if o.count(key)==1: break
else:
    # duplicated spans: take the SAME occurrence index the edit has in the base row
    ctx=a[max(0,p-80):p]; key=ctx+old
    k=a[:p-len(ctx)].count(key)
    if o.count(key)<=k: sys.exit(f'no anchor: base has occurrence {k}, HEAD has {o.count(key)}')
    q=-1
    for _ in range(k+1): q=o.index(key,q+1)
    o_=o[:q]+ctx+new+o[q+len(key):]; print('occurrence',k,'of',o.count(key)); key=None
out=o.replace(key,ctx+new) if key else o_
L=open(f).read().split('\n'); i=ln-1
if L[i].startswith('<<<<<<<'): L[i:i+5]=[out]
else: L[i]=out
open(f,'w').write('\n'.join(L)); print('written',len(o),'->',len(out))
