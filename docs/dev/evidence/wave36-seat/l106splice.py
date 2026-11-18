# l106splice.py <file> <SRid> <line>: resolve a one-line conflict block (ours/theirs) by inserting theirs' `BUILT by <SRid> (...)` clause into ours at the same preceding anchor.
import sys,subprocess
f,sr,ln=sys.argv[1],sys.argv[2],int(sys.argv[3])
L=open(f).read().split('\n')
i=ln-1; assert L[i].startswith('<<<<<<<'), L[i][:30]
ours=L[i+1]; assert L[i+2]=='=======', L[i+2][:20]; theirs=L[i+3]; assert L[i+4].startswith('>>>>>>>')
s=theirs.index(f' BUILT by {sr} ('); k=theirs.index('(',s); d=0
for e in range(k,len(theirs)):
    if theirs[e]=='(': d+=1
    elif theirs[e]==')':
        d-=1
        if d==0: break
clause=theirs[s:e+1]
# anchor: the longest suffix of theirs[:s] (up to 160 chars) that occurs exactly once in ours
for n in range(160,20,-1):
    anc=theirs[s-n:s]
    if ours.count(anc)==1:
        break
else: sys.exit('no unique anchor')
p=ours.index(anc)+len(anc)
new=ours[:p]+clause+ours[p:]
assert new.count(sr)>=1
L[i:i+5]=[new]; open(f,'w').write('\n'.join(L)); print('spliced',sr,'anchor',repr(anc[-60:]),'clause',len(clause))
