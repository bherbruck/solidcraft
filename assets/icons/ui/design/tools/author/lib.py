import math, os, re, tomllib
OUT=os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),'svg','model')
PALETTE=tomllib.load(open(os.path.join(os.path.dirname(os.path.dirname(OUT)),'palette.toml'),'rb'))
def source(role): return PALETTE['roles'][role].get('source', PALETTE['roles'][role]['light'])
INK=source('ink'); GAP=source('gap'); GUIDE=source('guide')
P1,P2,P3=(source('passive-'+str(i)) for i in (1,2,3))
A1,A2,A3=(source('active-'+str(i)) for i in (1,2,3))
OW=PALETTE['strokes']['geometry']; AW=PALETTE['strokes']['action']; NW=PALETTE['strokes']['navigation']
R3=math.sqrt(3)
def f(v):
    s=('%.2f'%v).rstrip('0').rstrip('.')
    return '0' if s in('-0','') else s
def pts(ps): return ''.join(('M' if i==0 else 'L')+f(x)+' '+f(y) for i,(x,y) in enumerate(ps))
def poly(ps, close=True): return pts(ps)+('Z' if close else '')
def ell(cx,cy,rx,ry=None):
    ry=rx if ry is None else ry
    return 'M%s %sA%s %s 0 1 0 %s %sA%s %s 0 1 0 %s %sZ'%(f(cx-rx),f(cy),f(rx),f(ry),f(cx+rx),f(cy),f(rx),f(ry),f(cx-rx),f(cy))
def arcpts(cx,cy,rx,ry,t0,t1):
    """arc along ellipse from angle t0 to t1 (degrees, screen coords: 0=right, 90=down). returns path 'M..A..' """
    a0,a1=math.radians(t0),math.radians(t1)
    p0=(cx+rx*math.cos(a0),cy+ry*math.sin(a0)); p1=(cx+rx*math.cos(a1),cy+ry*math.sin(a1))
    large=1 if abs(t1-t0)>180 else 0; sweep=1 if t1>t0 else 0
    return 'M%s %sA%s %s 0 %d %d %s %s'%(f(p0[0]),f(p0[1]),f(rx),f(ry),large,sweep,f(p1[0]),f(p1[1]))
def arcto(rx,ry,x,y,large=0,sweep=1): return 'A%s %s 0 %d %d %s %s'%(f(rx),f(ry),large,sweep,f(x),f(y))
def ellarc(cx,cy,rx,ry,t0,t1): return arcpts(cx,cy,rx,ry,t0,t1)
def rrect(x0,y0,x1,y1,r):
    return ('M%s %sL%s %sA%s %s 0 0 1 %s %sL%s %sA%s %s 0 0 1 %s %sL%s %sA%s %s 0 0 1 %s %sL%s %sA%s %s 0 0 1 %s %sZ'%
     (f(x0+r),f(y0),f(x1-r),f(y0),f(r),f(r),f(x1),f(y0+r),f(x1),f(y1-r),f(r),f(r),f(x1-r),f(y1),f(x0+r),f(y1),f(r),f(r),f(x0),f(y1-r),f(x0),f(y0+r),f(r),f(r),f(x0+r),f(y0)))

def arc_samples(d,n=24):
    """points along every elliptical arc (rotation 0) of a path, so a bounding box sees the curve and not only its ends"""
    pts=[]; cur=(0.0,0.0)
    for c,a in re.findall(r'([MLCQAZ])([^MLCQAZ]*)',d):
        v=[float(x) for x in a.split()]
        if c=='Z' or not v: continue
        if c in 'ML':
            cur=(v[-2],v[-1])
        elif c in 'CQ':
            cur=(v[-2],v[-1])
        elif c=='A':
            for i in range(0,len(v),7):
                rx,ry,_,large,sweep,x2,y2=v[i:i+7]; x1,y1=cur
                dx,dy=(x1-x2)/2,(y1-y2)/2
                lam=dx*dx/(rx*rx)+dy*dy/(ry*ry)
                if lam>1: rx*=math.sqrt(lam); ry*=math.sqrt(lam)
                num=rx*rx*ry*ry-rx*rx*dy*dy-ry*ry*dx*dx; den=rx*rx*dy*dy+ry*ry*dx*dx
                co=math.sqrt(max(num/den,0)) if den else 0
                if large==sweep: co=-co
                cxp,cyp=co*rx*dy/ry,-co*ry*dx/rx
                cx,cy=cxp+(x1+x2)/2,cyp+(y1+y2)/2
                t1=math.atan2((dy-cyp)/ry,(dx-cxp)/rx); t2=math.atan2((-dy-cyp)/ry,(-dx-cxp)/rx)
                dt=t2-t1
                if sweep and dt<0: dt+=2*math.pi
                if not sweep and dt>0: dt-=2*math.pi
                for k in range(n+1):
                    t=t1+dt*k/n; pts.append((cx+rx*math.cos(t),cy+ry*math.sin(t)))
                cur=(x2,y2)
    return pts

class Icon:
    def __init__(s,name,title=None):
        s.name=name; s.title=title or name.replace('_',' ').title(); s.els=[]; s.fam='model'
    def fill(s,d,c,rule=None):
        s.els.append('<path d="%s" fill="%s"%s/>'%(d,c,' fill-rule="evenodd"' if rule else '')); return s
    def stroke(s,d,c=INK,w=OW,fill=None,rule=None,dash=None):
        if dash and c not in (GAP,A1,A2,A3): c=GUIDE
        s.els.append('<path d="%s" fill="%s" stroke="%s" stroke-width="%s" stroke-linejoin="round" stroke-linecap="round"%s%s/>'%(d,fill or 'none',c,f(w),' stroke-dasharray="%s"'%dash if dash else '',' fill-rule="evenodd"' if rule else '')); return s
    def dot(s,x,y,r,c):
        s.els.append('<circle cx="%s" cy="%s" r="%s" fill="%s"/>'%(f(x),f(y),f(r),c)); return s
    def ring(s,x,y,r,c=INK,w=OW,fill=None):
        s.els.append('<circle cx="%s" cy="%s" r="%s" fill="%s" stroke="%s" stroke-width="%s" stroke-linejoin="round" stroke-linecap="round"/>'%(f(x),f(y),f(r),fill or 'none',c,f(w))); return s
    def clip_begin(s,cid,d):
        s.els.append('<defs><clipPath id="%s"><path d="%s" fill="none"/></clipPath></defs>'%(cid,d)); s.els.append('<g clip-path="url(#%s)">'%cid); return s
    def clip_end(s):
        s.els.append('</g>'); return s
    def recenter(s):
        """move the finished drawing so the box around all its path points (and strokes) is centred on (32, 32); exact for polygons and lines"""
        def walk(d,fn):
            out=[]
            for c,a in re.findall(r'([MLCQAZ])([^MLCQAZ]*)',d):
                v=a.split(); 
                if c=='Z' or not v: out.append(c); continue
                if c=='A':
                    for i in range(0,len(v),7): v[i+5],v[i+6]=fn(float(v[i+5]),float(v[i+6]))
                else:
                    for i in range(0,len(v),2): v[i],v[i+1]=fn(float(v[i]),float(v[i+1]))
                out.append(c+' '.join(f(float(x)) if not isinstance(x,str) else x for x in v))
            return ''.join(out)
        xs=[];ys=[]
        for e in s.els:
            hw=0
            m=re.search(r'stroke-width="([\d.]+)"',e)
            if m and 'stroke="none"' not in e and ' stroke="' in e: hw=float(m.group(1))/2
            d=re.search(r' d="([^"]+)"',e)
            if d:
                def grab(x,y): xs.extend((x-hw,x+hw)); ys.extend((y-hw,y+hw)); return x,y
                walk(d.group(1),grab)
                for px,py in arc_samples(d.group(1)): xs.extend((px-hw,px+hw)); ys.extend((py-hw,py+hw))
            c=re.search(r'cx="([-\d.]+)" cy="([-\d.]+)" r="([\d.]+)"',e)
            if c: x,y,r=map(float,c.groups()); xs.extend((x-r-hw,x+r+hw)); ys.extend((y-r-hw,y+r+hw))
        dx=32-(min(xs)+max(xs))/2; dy=32-(min(ys)+max(ys))/2
        mv=lambda x,y:(x+dx,y+dy)
        new=[]
        for e in s.els:
            e=re.sub(r' d="([^"]+)"',lambda m:' d="%s"'%walk(m.group(1),mv),e)
            e=re.sub(r'cx="([-\d.]+)" cy="([-\d.]+)"',lambda m:'cx="%s" cy="%s"'%(f(float(m.group(1))+dx),f(float(m.group(2))+dy)),e)
            new.append(e)
        s.els=new; return s
    def write(s,fam='model'):
        d=os.path.join(os.path.dirname(OUT),fam); os.makedirs(d,exist_ok=True)
        body='\n'.join('  '+e for e in s.els)
        open(os.path.join(d,s.name+'.svg'),'w').write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n  <title>%s</title>\n%s\n</svg>\n'%(s.title,body))

# ---- isometric helpers ----
class Iso:
    """x -> right-down, y -> left-down, z up. k = screen units per unit."""
    def __init__(s,k=1.0,ox=32,oy=32): s.k=k; s.ox=ox; s.oy=oy
    def p(s,x,y,z=0): return (s.ox+(x-y)*R3/2*s.k, s.oy+(x+y)*0.5*s.k-z*s.k)
    def fit(s,pts3d,maxw=56,maxh=56):
        s.k=1.0; s.ox=0; s.oy=0
        P=[s.p(*q) for q in pts3d]; w=max(p[0] for p in P)-min(p[0] for p in P); h=max(p[1] for p in P)-min(p[1] for p in P)
        s.k=min(maxw/w,maxh/h); P=[s.p(*q) for q in pts3d]
        s.ox=32-(max(p[0] for p in P)+min(p[0] for p in P))/2; s.oy=32-(max(p[1] for p in P)+min(p[1] for p in P))/2
        return s
    def extrude(s,ic,rings,z0,z1,cols=(P1,P2,P3),w=OW):
        """prism of the plan polygons `rings` (even-odd: holes allowed), z0..z1. Only the edges where the surface changes angle are drawn."""
        def inside(x,y):
            n=0
            for r in rings:
                for i in range(len(r)):
                    (ax,ay),(bx,by)=r[i],r[(i+1)%len(r)]
                    if (ay>y)!=(by>y) and x<ax+(y-ay)*(bx-ax)/(by-ay): n+=1
            return n%2==1
        walls=[]
        for r in rings:
            for i in range(len(r)):
                a,b=r[i],r[(i+1)%len(r)]; dx,dy=b[0]-a[0],b[1]-a[1]; L=math.hypot(dx,dy)
                nx,ny=dy/L,-dx/L; mx,my=(a[0]+b[0])/2,(a[1]+b[1])/2
                if inside(mx+nx*0.01,my+ny*0.01): nx,ny=-nx,-ny
                if nx>0.5 or ny>0.5: walls.append((mx+my,a,b,(nx,ny)))
        for _,a,b,nrm in sorted(walls):
            d=s.poly((a[0],a[1],z1),(b[0],b[1],z1),(b[0],b[1],z0),(a[0],a[1],z0))
            ic.fill(d,cols[2] if nrm[0]>0.5 else cols[1]); ic.stroke(d,INK,w)
        top=''.join(poly([s.p(x,y,z1) for x,y in r]) for r in rings)
        ic.fill(top,cols[0],rule=True); ic.stroke(top,INK,w,rule=True)
    def poly(s,*ps): return poly([s.p(*q) for q in ps])
    def ellipse_r(s,r): return (R3/2*s.k*r*math.sqrt(2), 0.5*s.k*r*math.sqrt(2))
    def box(s,ic,x0,y0,z0,x1,y1,z1,cols=(P1,P2,P3),w=OW,detail=True,ink=INK):
        """draw filled box with outline; faces: top, left(+y), right(+x)"""
        top=s.poly((x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1))
        left=s.poly((x0,y1,z1),(x1,y1,z1),(x1,y1,z0),(x0,y1,z0))
        right=s.poly((x1,y0,z1),(x1,y1,z1),(x1,y1,z0),(x1,y0,z0))
        ic.fill(left,cols[1]).fill(right,cols[2]).fill(top,cols[0])
        sil=s.poly((x0,y0,z1),(x1,y0,z1),(x1,y0,z0),(x1,y1,z0),(x0,y1,z0),(x0,y1,z1))
        ic.stroke(sil,ink,w)
        if detail:
            ic.stroke(poly([s.p(x0,y1,z1),s.p(x1,y1,z1),s.p(x1,y0,z1)],False),ink,w)
            ic.stroke(poly([s.p(x1,y1,z1),s.p(x1,y1,z0)],False),ink,w)

def cylinder(ic,X,Y,rx,h,cols=(P1,P2,P3),w=OW,top=True,ry=None,sh=0.35):
    """vertical cylinder; (X,Y) = centre of the top ellipse; side shading strip on the right"""
    ry=rx/R3 if ry is None else ry
    body='M%s %sL%s %s%sL%s %sZ'%(f(X-rx),f(Y),f(X-rx),f(Y+h),arcto(rx,ry,X+rx,Y+h,0,0),f(X+rx),f(Y))
    ic.fill(body,cols[1])
    xs=X+sh*rx; yb=ry*math.sqrt(1-sh*sh)
    strip='M%s %sL%s %s%sL%s %sZ'%(f(xs),f(Y),f(xs),f(Y+h+yb),arcto(rx,ry,X+rx,Y+h,0,0),f(X+rx),f(Y))
    ic.fill(strip,cols[2])
    ic.stroke(body,INK,w)
    if top: ic.stroke(ell(X,Y,rx,ry),INK,w,fill=cols[0])

def clip_poly(subject,clip):
    """Sutherland-Hodgman: the part of polygon `subject` inside the convex polygon `clip` (lists of (x, y))."""
    area=sum(clip[i][0]*clip[(i+1)%len(clip)][1]-clip[(i+1)%len(clip)][0]*clip[i][1] for i in range(len(clip)))
    sgn=1 if area>0 else -1
    def inside(p,a,b): return sgn*((b[0]-a[0])*(p[1]-a[1])-(b[1]-a[1])*(p[0]-a[0]))>=-1e-9
    def cross(p,q,a,b):
        x1,y1,x2,y2,x3,y3,x4,y4=p[0],p[1],q[0],q[1],a[0],a[1],b[0],b[1]
        den=(x1-x2)*(y3-y4)-(y1-y2)*(x3-x4)
        if abs(den)<1e-12: return q
        t=((x1-x3)*(y3-y4)-(y1-y3)*(x3-x4))/den
        return (x1+t*(x2-x1),y1+t*(y2-y1))
    out=list(subject)
    for i in range(len(clip)):
        a,b=clip[i],clip[(i+1)%len(clip)]; inp=out; out=[]
        if not inp: break
        s=inp[-1]
        for e in inp:
            if inside(e,a,b):
                if not inside(s,a,b): out.append(cross(s,e,a,b))
                out.append(e)
            elif inside(s,a,b): out.append(cross(s,e,a,b))
            s=e
    return out
