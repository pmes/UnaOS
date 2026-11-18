# rowmerge3.py <line> : word-level 3-way merge of one handlers.md line at a fold STOP (ours/theirs conflict block at <line>), base = merge-base's line. Prints conflicts count; writes only if 0.
import subprocess,sys,tempfile,os
ln=int(sys.argv[1]); f='docs/dev/blueprint/handlers.md'
L=open(f).read().split('\n'); i=ln-1
assert L[i].startswith('<<<<<<<') and L[i+2]=='=======' and L[i+4].startswith('>>>>>>>')
ours,theirs=L[i+1],L[i+3]
mb=subprocess.run(['git','merge-base','HEAD','MERGE_HEAD'],capture_output=True,text=True).stdout.strip()
base=subprocess.run(['git','show',f'{mb}:{f}'],capture_output=True,text=True).stdout.split('\n')[ln-1]
d=tempfile.mkdtemp(dir='/root/unaos-bench/scratch/cloud/wave36/tmp')
P=[]
for n,t in (('o',ours),('b',base),('t',theirs)):
    p=os.path.join(d,n); open(p,'w').write('\n'.join(t.split(' '))+'\n'); P.append(p)
r=subprocess.run(['git','merge-file','-p',P[0],P[1],P[2]],capture_output=True,text=True)
out=r.stdout
c=out.count('\n<<<<<<< ')+(1 if out.startswith('<<<<<<< ') else 0)
print('conflicts',c,'len ours',len(ours),'base',len(base),'theirs',len(theirs))
if c==0:
    merged=' '.join(out.rstrip('\n').split('\n'))
    print('merged len',len(merged),'expected',len(ours)+len(theirs)-len(base))
    L[i:i+5]=[merged]; open(f,'w').write('\n'.join(L))
else:
    open(os.path.join(d,'m'),'w').write(out); print('see',os.path.join(d,'m'))
