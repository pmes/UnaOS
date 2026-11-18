import sys
f=sys.argv[1]; L=open(f).read().split('\n'); out=[]; i=0; ok=True
while i<len(L):
    if L[i].startswith('<<<<<<<'):
        m=next(k for k in range(i,len(L)) if L[k]=='======='); e=next(k for k in range(m,len(L)) if L[k].startswith('>>>>>>>'))
        head=L[i+1:m]; br=L[m+1:e]
        out+=head+[l for l in br if l not in head]; i=e+1; continue
    out.append(L[i]); i+=1
open(f,'w').write('\n'.join(out))
