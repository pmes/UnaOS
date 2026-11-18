import sys,subprocess,difflib
f,branch,prefix=sys.argv[1:4]
base=sys.argv[4] if len(sys.argv)>4 else subprocess.check_output(['git','merge-base','HEAD',branch],text=True).strip()
def show(ref): return subprocess.check_output(['git','show',f'{ref}:{f}'],text=True).split('\n')
def row(L):
    r=[i for i,l in enumerate(L) if l.startswith(prefix)]; assert len(r)==1,(prefix,len(r)); return r[0]
H=show('HEAD'); B=show(base); X=show(branch)
h=H[row(H)]; b=B[row(B)]; x=X[row(X)]
def ed(a,c): return [(t,i1,i2,c[j1:j2]) for t,i1,i2,j1,j2 in difflib.SequenceMatcher(None,a,c,autojunk=False).get_opcodes() if t!='equal']
eb=ed(b,x); eh=ed(b,h)
eb2=[e for e in eb if e not in eh]
for t,i1,i2,_ in eb2:
    for t2,k1,k2,_ in eh:
        if not (i2<=k1 or k2<=i1): sys.exit(f'OVERLAP {(i1,i2)} vs {(k1,k2)}: {b[min(i1,k1)-60:max(i2,k2)+60]!r}')
out=[];pos=0
for e in sorted(eb2+eh,key=lambda e:(e[1],e[2])):
    t,i1,i2,rep=e; out.append(b[pos:i1]); out.append(rep.replace('|','/') if e in eb2 else rep); pos=i2
new=''.join(out)+b[pos:]
assert new.count('|')==h.count('|'), 'pipes'
assert new.count('(')-new.count(')')==h.count('(')-h.count(')'), 'parens'
H[row(H)]=new
open(f,'w').write('\n'.join(H)); print('spliced',prefix,len(h),'->',len(new),'base',base[:9])
