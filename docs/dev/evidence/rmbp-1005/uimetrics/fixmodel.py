# Host-side model of the wm fixture geometry (UIMETRICS B372, seat item 3). Mirrors wm.rs formulas.
def px(n,s2): return (n*s2+1)//2
def run(pw,ph,s2,FIX_W,label,wide=448):
    B=px(5,s2); T=px(34,s2); G=px(12,s2); cb=px(12,s2); cw=px(9,s2); btn=px(28,s2)
    glyph=max(1,min(4,s2//2))
    floor=5*G+3*cb+cw
    top=T                        # menu bar on (work_top)
    res=G+btn+2*G                # dock reserve (desktop scene owns backdrop)
    usable=max(1,ph-top-res)
    cap=4*glyph
    def fit(w,h,pw_=pw,us=usable): return max(1,min((pw_-2*B)//w,(us-T-2*B)//h))
    def mws(w,h,pw_=pw,us=usable):
        c=max(1,-(-floor//w)); return c if c<=fit(w,h,pw_,us) else 1
    def place(w,h): return max(max(1,min(pw//2//w, usable//2//h, cap)), mws(w,h))
    def zoom(zw,zh,w,h): return max(min(fit(w,h,zw,zh),cap), mws(w,h,zw,zh))
    def clamp(x,y,w,h,s):
        cwid,ch=w*s,h*s; miny=top+T+B
        maxx=max(B,pw-(cwid+B)); maxy=max(miny,ph-(ch+B))
        return min(max(x,B),maxx), min(max(y,miny),maxy), maxx, maxy
    out={}
    s=place(FIX_W,8); bw=FIX_W*s+2*B
    out['create scale/box']=f"{s} / {bw}"
    out['cluster on box']= bw>=2*B+floor
    # WMD + winsnap: move_to(pw/3, ph/3+T+B), drag +24
    x,y,mx,my=clamp(pw//3,ph//3+T+B,FIX_W,8,s)
    out['WMD/snap drag moves']= (x+24<=mx) or (y+24<=my)
    # winsnap quarters + halves fit
    q_ok=True
    for zw,zh in [(pw//2,usable),(pw-pw//2,usable),(pw//2,usable//2),(pw//2,usable-usable//2)]:
        zs=zoom(zw,zh,FIX_W,8)
        if FIX_W*zs+2*B>zw or 8*zs+T+2*B>zh: q_ok=False
    out['snap zones fit']=q_ok and pw>=256 and ph>=256
    # hit-test ht-a/b at (pw/3, ph/4+T+B)
    x,y,_,_=clamp(pw//3,ph//4+T+B,FIX_W,8,s)
    out['hittest rows placed']= True
    # ctrldecline (pinned scale 1, width floor on wide surface)
    pin=max(FIX_W,floor)
    out['ctrldecline AT_W<=surface']= floor<=wide
    # dmgovlp
    bw1=pin+2*B; bh1=8+T+2*B
    need_w=8+2*(bw1-8)+bw1+B; ya=top+8; yb=ya+bh1+16; need_h=yb+2*(bh1//4)+bh1+8
    out['dmgovlp runs (else SKIP)']= pw>=512 and ph>=400 and need_w<=pw and need_h<=ph
    out['floor']=floor
    print(f"{label:28s}", out)
for FIX_W in (160,576):
    print(f"--- FIX_W={FIX_W}")
    for pw,ph,s2 in [(640,480,2),(1280,800,2),(1920,1200,2),(2880,1800,5),(2560,1600,5),(1440,900,3)]:
        run(pw,ph,s2,FIX_W,f"{pw}x{ph}@{s2/2}")
