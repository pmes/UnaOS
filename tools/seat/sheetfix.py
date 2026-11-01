import re,subprocess,difflib,sys
br=sys.argv[1]; cut=sys.argv[2]
for p in ['docs/dev/LEDGER.md','docs/dev/OS/rmbp-ledger.md']:
    s=open(p).read()
    m=re.search(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]*\n',s,re.S)
    if m: s=s[:m.start()]+m.group(1)+m.group(2)+s[m.end():]; open(p,'w').write(s); print(p,'unioned')
def rows(ref): return subprocess.run(['git','show',ref+':docs/dev/blueprint/handlers.md'],capture_output=True,text=True).stdout.split('\n')
base=rows(cut); head=rows('HEAD'); b=rows(br)
wt=open('docs/dev/blueprint/handlers.md').read()
if '<<<<<<<' in wt:
    m=re.search(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]*\n',wt,re.S); wt=wt[:m.start()]+m.group(1)+wt[m.end():]
L=wt.split('\n'); assert len(base)==len(head)==len(b)==len(L),(len(base),len(head),len(b),len(L))
for i,(bb,h,r) in enumerate(zip(base,head,b)):
    if r!=bb and h!=bb:
        print('both-changed',i+1,r[:40])
        sm=difflib.SequenceMatcher(None,bb,r,autojunk=False); cur=h
        for op,i1,i2,j1,j2 in reversed(sm.get_opcodes()):
            if op=='equal': continue
            old=bb[i1:i2]; new=r[j1:j2]; ctx=bb[max(0,i1-50):i1]
            j=cur.find(ctx); assert j>=0 and cur.find(ctx,j+1)<0,('ctx',ctx[:30],j)
            k=j+len(ctx); assert cur[k:k+len(old)]==old,(cur[k:k+40],old[:40])
            cur=cur[:k]+new+cur[k+len(old):]
        L[i]=cur
    elif r!=bb: L[i]=r
open('docs/dev/blueprint/handlers.md','w').write('\n'.join(L)); print('lines',len(L)-1)
