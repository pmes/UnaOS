import re,subprocess,sys
f,BR=sys.argv[1],sys.argv[2]
pat=re.compile(r'<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> '+re.escape(BR)+r'\n', re.S)
base=subprocess.run(["git","merge-base","HEAD",BR],capture_output=True,text=True).stdout.strip()
bt=subprocess.run(["git","show",base+":"+f],capture_output=True,text=True).stdout.split("\n")
D="/root/unaos-bench/scratch/m3"; SP=re.compile(r'(?<=[.;)])\s(?=[A-Z`(\[])|(?<=\|)\s|\s(?=\|)|\s·\s')
bad=0
def one(m):
    global bad
    al=m.group(1).rstrip("\n").split("\n"); bl=m.group(2).rstrip("\n").split("\n")
    bk={l[:40]:l for l in bl}; out=[]; used=set()
    for h in al:
        k=h[:40]
        if k in bk:
            used.add(k); b=bk[k]; bs=[x for x in bt if x.startswith(k)]
            if h==b or (bs and b==bs[0]): out.append(h)
            elif bs and h==bs[0]: out.append(b)
            else:
                for n,t in [("base",bs[0] if bs else ""),("head",h),("br",b)]: open(f"{D}/c.{n}","w").write("\n".join(SP.split(t))+"\n")
                r=subprocess.run(["git","merge-file","-p",f"{D}/c.head",f"{D}/c.base",f"{D}/c.br"],capture_output=True,text=True)
                if "<<<<<<<" in r.stdout: print("BOTH-CHANGED CONFLICT:",k); bad+=1; out.append(h)
                else:
                    row=" ".join(l for l in r.stdout.split("\n") if l); row=re.sub(r'\s+\|',' |',row); row=re.sub(r'\|\s+','| ',row); out.append(row); print("merged:",k)
        else: out.append(h)
    for l in bl:
        if l[:40] in used: continue
        # a row the branch carries UNCHANGED from the merge-base but HEAD no longer has was DELETED at the seat
        # (a superseded §5 row retired by a later fold): do not re-add it
        if any(x==l for x in bt): print("dropped (deleted in HEAD):",l[:40]); continue
        out.append(l)
    return "\n".join(out)+"\n"
s=open(f).read(); s=pat.sub(one,s); open(f,"w").write(s); print(f,"markers",s.count("<<<<<<<"),"both-changed-conflicts",bad)
sys.exit(1 if bad or s.count("<<<<<<<") else 0)
