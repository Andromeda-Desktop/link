import math, random, sys
random.seed(7)
W = 512
TF = "translate(256 250) rotate(-22) scale(1.04 0.42)" if sys.argv[1:] == ["desktop"] else "translate(256 296) rotate(-34) scale(1 0.4)"

def arm_points(phase, n, spread):
    pts = []
    for i in range(n):
        u = random.random() ** 0.6
        theta = u * 3.4 * math.pi
        r = 18 * math.exp(0.2255 * theta)
        if r > 205: continue
        j = random.gauss(0, spread * (0.4 + r / 205))
        a = theta + phase
        x = (r + j) * math.cos(a) + random.gauss(0, 4)
        y = (r + j) * math.sin(a) + random.gauss(0, 4)
        pts.append((x, y, r))
    return pts

def arm_path(phase, off=0.0):
    d = []
    for k in range(120):
        theta = k / 119 * 3.4 * math.pi
        r = 18 * math.exp(0.2255 * theta) + off
        if r > 200: break
        a = theta + phase
        d.append(f"{'M' if not d else 'L'}{r*math.cos(a):.1f} {r*math.sin(a):.1f}")
    return " ".join(d)

def arm_seg_paths(phase, off=0.0):
    pts=[]
    for k in range(160):
        theta = k / 159 * 3.4 * math.pi
        r = 18 * math.exp(0.2255 * theta) + off
        if r > 205: break
        a = theta + phase
        pts.append((r*math.cos(a), r*math.sin(a), r))
    return pts

def tapered(phase, wmax, color, op, filt, off=0.0):
    pts = arm_seg_paths(phase, off)
    out=[]
    for i in range(0, len(pts)-1):
        x0,y0,r0 = pts[i]; x1,y1,_ = pts[i+1]
        f = r0/205
        w = wmax*(0.35+0.9*f)*(1.0 if f<0.75 else max(0.15,(1-f)/0.25))
        o = op*(1.0 if f<0.8 else max(0.0,(1-f)/0.2))
        out.append(f'<line x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}" stroke="{color}" stroke-opacity="{o:.2f}" stroke-width="{w:.1f}" stroke-linecap="round"/>')
    return f'<g filter="url(#{filt})">'+"".join(out)+'</g>'

def particles(ph, n, spread, rmin, front):
    out=[]
    for (x, y, r) in arm_points(ph, n, spread):
        if r < rmin: continue
        col = random.choice(["#FFFFFF", "#FFFFFF", "#CFE0FF", "#FFE6F4", "#AFC8FF", "#FFF0D0" if r < 70 else "#BFD4FF"])
        rad = random.uniform(0.45, 1.4) if random.random() < 0.88 else random.uniform(1.5, 2.2)
        out.append(f'<ellipse cx="{x:.1f}" cy="{y:.1f}" rx="{rad:.1f}" ry="{rad*2.5:.1f}" fill="{col}" opacity="{random.uniform(0.4,1):.2f}"/>')
    return out

def galaxy(half):
    front = half == 'front'
    out = [f'<g transform="{TF}"><g mask="url(#{half})">']
    if not front:
        out.append('<circle r="225" fill="url(#disc)"/>')
        for ph in (0, math.pi):
            out.append(tapered(ph, 7, "#120A18", 0.55, "thin", -16))
    for ph in (0, math.pi):
        out.append(tapered(ph, 34, "#9FB8FF", 0.45, "armblur"))
        out.append(tapered(ph, 12, "#DCE6FF", 0.3, "armtex"))
        out.append(tapered(ph, 5, "#EEF3FF", 0.5, "thin"))
        out += particles(ph, 820, 12, 70 if front else 0, front)
        out += particles(ph + 0.35, 190, 24, 70 if front else 0, front)
        for (x, y, r) in arm_points(ph + 0.5, 14, 8):
            if r < 70: continue
            out.append(f'<ellipse cx="{x:.1f}" cy="{y:.1f}" rx="4" ry="10" fill="#FF7FC0" opacity="0.26" filter="url(#thin)"/>')
        for (x, y, r) in arm_points(ph, 22, 5):
            if r < 80: continue
            out.append(f'<ellipse cx="{x:.1f}" cy="{y:.1f}" rx="3.5" ry="9" fill="#9FC4FF" opacity="0.5" filter="url(#thin)"/>')
    if not front:
        out.append('<ellipse rx="62" ry="62" fill="url(#bulge)"/>')
    out.append('</g></g>')
    return "\n".join(out)

stars = []
for _ in range(120):
    x, y = random.uniform(-125, 637), random.uniform(-125, 637)
    rr = random.choice([0.7, 0.9, 1.1, 1.4, 1.8])
    stars.append(f'<circle cx="{x:.0f}" cy="{y:.0f}" r="{rr}" opacity="{random.uniform(0.3,0.9):.2f}"/>')

import os
V = os.environ.get('V','3')
DESK = sys.argv[1:] == ['desktop']
ADAPT = len(sys.argv) > 1 and not DESK
SQ = '<rect x="-125" y="-125" width="762" height="762"/>' if ADAPT else '<rect width="512" height="512" rx="116"/>'
BG = '<rect x="-125" y="-125" width="762" height="762" fill="url(#space)"/>' if ADAPT else '<rect width="512" height="512" fill="url(#space)"/>'
CX, CY = (256, 250) if DESK else (256, 296)
A = "M116 414 L232 94 L280 94 L396 414 L334 414 L256 186 L178 414 Z"
AFILL = {
 '0': f'<path d="{A}" fill="#C9D4FF" opacity="0.35" filter="url(#aglow)"/><path d="{A}" fill="url(#aface)"/>',
 '1': f'<path d="{A}" fill="url(#dark)"/><path d="{A}" fill="none" stroke="url(#rim)" stroke-width="3"/><path d="{A}" fill="url(#corelit)"/>',
 '3': f'<path d="{A}" fill="#FFE2B0" opacity="0.3" filter="url(#aglow)"/><path d="{A}" fill="url(#aface2)"/><path d="{A}" fill="url(#corelit)"/><g transform="{TF}" mask="url(#front)" opacity="0.5">{tapered(0, 30, "#000", 0.6, "armblur", 6)}{tapered(math.pi, 30, "#000", 0.6, "armblur", 6)}</g>',
}[V]
if DESK:
    S = 'M118 436 L256 104 L394 436'
    AFILL = (f'<path d="{S}" fill="none" stroke="#FFE2B0" stroke-opacity="0.3" stroke-width="64" stroke-linecap="round" stroke-linejoin="round" filter="url(#aglow)"/>'
        f'<path d="{S}" fill="none" stroke="url(#aface2)" stroke-width="64" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<path d="{S}" fill="none" stroke="url(#corelit)" stroke-width="64" stroke-linecap="round" stroke-linejoin="round"/>'
        '<rect x="150" y="282" width="212" height="92" rx="46" fill="url(#visor)" stroke="#FFFFFF" stroke-width="14"/>'
        '<path d="M196 328 Q216 346 236 328" fill="none" stroke="#7CF5E6" stroke-width="10" stroke-linecap="round"/>'
        '<path d="M276 328 Q296 346 316 328" fill="none" stroke="#7CF5E6" stroke-width="10" stroke-linecap="round"/>')
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="{sys.argv[1] if ADAPT else "0 0 512 512"}">
<defs>
<radialGradient id="space" cx="0.5" cy="0.55" r="0.75"><stop offset="0" stop-color="#0A0A0C"/><stop offset="1" stop-color="#000000"/></radialGradient>
<radialGradient id="neb1" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#5B3BD6" stop-opacity="0.45"/><stop offset="1" stop-color="#5B3BD6" stop-opacity="0"/></radialGradient>
<radialGradient id="neb2" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#1E7BFF" stop-opacity="0.35"/><stop offset="1" stop-color="#1E7BFF" stop-opacity="0"/></radialGradient>
<radialGradient id="disc" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#FFE9C2" stop-opacity="0.9"/><stop offset="0.18" stop-color="#E9B8FF" stop-opacity="0.5"/><stop offset="0.55" stop-color="#6E7BFF" stop-opacity="0.22"/><stop offset="1" stop-color="#3A2FA0" stop-opacity="0"/></radialGradient>
<radialGradient id="core" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.25" stop-color="#FFF3D6" stop-opacity="0.95"/><stop offset="0.6" stop-color="#FFC98A" stop-opacity="0.35"/><stop offset="1" stop-color="#FF9F6A" stop-opacity="0"/></radialGradient>
<linearGradient id="aface" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.55" stop-color="#F1F3FA"/><stop offset="1" stop-color="#B9BFD0"/></linearGradient>
<filter id="aglow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="14"/></filter>
<linearGradient id="visor" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1A1A22"/><stop offset="1" stop-color="#050508"/></linearGradient>
<linearGradient id="dark" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1C1C22"/><stop offset="1" stop-color="#0B0B0F"/></linearGradient>
<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF" stop-opacity="0.9"/><stop offset="0.6" stop-color="#FFD9A0" stop-opacity="0.8"/><stop offset="1" stop-color="#8FA0FF" stop-opacity="0.5"/></linearGradient>
<radialGradient id="corelit" gradientUnits="userSpaceOnUse" cx="{CX}" cy="{CY}" r="170"><stop offset="0" stop-color="#FFD59A" stop-opacity="0.75"/><stop offset="0.5" stop-color="#FFB27A" stop-opacity="0.25"/><stop offset="1" stop-color="#FFB27A" stop-opacity="0"/></radialGradient>
<linearGradient id="aface2" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.5" stop-color="#E9ECF6"/><stop offset="1" stop-color="#8C93AE"/></linearGradient>
<radialGradient id="bulge" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.2" stop-color="#FFF1D2" stop-opacity="0.95"/><stop offset="0.55" stop-color="#FFC890" stop-opacity="0.4"/><stop offset="1" stop-color="#FF9F6A" stop-opacity="0"/></radialGradient>
<filter id="armtex" x="-20%" y="-20%" width="140%" height="140%"><feTurbulence type="fractalNoise" baseFrequency="0.06" numOctaves="3" seed="4" result="n"/><feDisplacementMap in="SourceGraphic" in2="n" scale="16" xChannelSelector="R" yChannelSelector="G" result="d"/><feGaussianBlur in="d" stdDeviation="3"/></filter>
<filter id="armblur" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="9"/></filter>
<filter id="thin" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="2"/></filter>
<filter id="haze" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="30"/></filter>
<clipPath id="sq">{SQ}</clipPath>
<linearGradient id="fb" gradientUnits="userSpaceOnUse" x1="0" y1="-30" x2="0" y2="30"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></linearGradient>
<linearGradient id="ff" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="30"><stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient>
<mask id="back" maskUnits="userSpaceOnUse" x="-300" y="-300" width="600" height="600"><rect x="-300" y="-300" width="600" height="600" fill="url(#fb)"/></mask>
<radialGradient id="hole" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="105"><stop offset="0.6" stop-color="#000"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>
<mask id="front" maskUnits="userSpaceOnUse" x="-300" y="-300" width="600" height="600"><rect x="-300" y="-300" width="600" height="600" fill="url(#ff)"/><circle r="105" fill="url(#hole)"/></mask>
</defs>
<g clip-path="url(#sq)">
{BG}
<g fill="#FFFFFF">{"".join(stars)}</g>
{galaxy("back")}
<circle cx="{CX}" cy="{CY}" r="70" fill="url(#core)"/>
{AFILL}
{galaxy("front")}
<circle cx="{CX}" cy="{CY}" r="26" fill="url(#core)" opacity="0.8"/>
</g>
</svg>'''
print(svg)
