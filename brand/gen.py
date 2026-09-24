#!/usr/bin/env python3
"""OpenScreenTime brand assets — one generator, every file.

Writes:  icons/<name>.svg (24-grid monoline set), mark*.svg, lockup-*.svg,
         app-icon*.svg, favicon.svg, tray-*.svg, og.svg
"""
import math, os, functools
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

HERE = os.path.dirname(os.path.abspath(__file__))
FIG = os.path.join(HERE, "..", "client", "fonts", "Figtree.ttf")

# ---- tokens ----------------------------------------------------------------
L = dict(bg="#f4f2ee", surface="#ffffff", track="#d5d1c9", line="#e6e3dd", ink="#1e1c19",
         ink2="#57544e", ink3="#726e66", brand="#2e7d46", warn="#8a6300", stop="#b3151c")
D = dict(bg="#171614", surface="#201e1b", track="#423d37", line="#302c27", ink="#f3f0ea",
         ink2="#b4aea4", ink3="#8b857b", brand="#46b06a", warn="#d8ab4a", stop="#e5595c")

# ---- the mark: geometry in a 64 box ----------------------------------------
# ring r=22 (diameter 44 = 69% of box), stroke 7 (16% of ring diameter),
# arc starts at the 12 tick (butt) and sweeps clockwise, round end cap,
# tick: 3 wide, 11 long (overhangs the track by 2 each side), painted last.
def arc_pt(cx, cy, r, frac):
    a = -math.pi/2 + 2*math.pi*frac
    return cx + r*math.cos(a), cy + r*math.sin(a)

def ring_svg_inner(cx, cy, r, sw, frac, color, track, tick, tick_w=None, tick_len=None,
                   state="fill", dash_color=None):
    """Ring primitive. state: fill | full | paused | none"""
    tick_w = tick_w if tick_w is not None else round(sw*0.45, 2)
    tick_len = tick_len if tick_len is not None else sw + 4
    out = []
    if state == "paused":
        circ = 2*math.pi*r
        out.append(f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{dash_color or color}" '
                   f'stroke-width="{sw}" stroke-linecap="round" stroke-dasharray="{circ*0.012:.2f} {circ*0.055:.2f}"/>')
    else:
        out.append(f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{track}" stroke-width="{sw}"/>')
        if state == "full":
            frac = 1.0
        if state in ("fill", "full") and frac > 0:
            if frac >= 0.999:
                out.append(f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{color}" stroke-width="{sw}"/>')
            else:
                sx, sy = arc_pt(cx, cy, r, 0)
                ex, ey = arc_pt(cx, cy, r, frac)
                large = 1 if frac > 0.5 else 0
                out.append(f'<path d="M{sx:.2f} {sy:.2f}A{r} {r} 0 {large} 1 {ex:.2f} {ey:.2f}" fill="none" '
                           f'stroke="{color}" stroke-width="{sw}"/>')
                out.append(f'<circle cx="{ex:.2f}" cy="{ey:.2f}" r="{sw/2}" fill="{color}"/>')
    y0 = cy - r - tick_len/2
    out.append(f'<path d="M{cx} {y0:.2f}v{tick_len}" stroke="{tick}" stroke-width="{tick_w}" stroke-linecap="round" fill="none"/>')
    return "\n  ".join(out)

def mark_inner(t, frac=0.40, state="fill", color=None, tick=None, size=64):
    s = size/64
    return ring_svg_inner(32*s, 32*s, 22*s, 7*s, frac, color or t["brand"], t["track"], tick or t["ink"],
                          tick_w=3*s, tick_len=11*s, state=state, dash_color=t["ink3"])

def svg(w, h, body, vb=None):
    vb = vb or f"0 0 {w} {h}"
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}">\n  {body}\n</svg>\n'

def write(name, content):
    p = os.path.join(HERE, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(content)
    return p

# ---- text as outlines (Figtree, instanced) ---------------------------------
@functools.lru_cache(maxsize=None)
def face(weight):
    f = instantiateVariableFont(TTFont(FIG), {"wght": weight})
    return f, f.getGlyphSet(), f.getBestCmap(), f["hmtx"]

def kern_pairs(font):
    """Best-effort pair kerning from GPOS PairPos (format 1 & 2)."""
    pairs = {}
    try:
        gpos = font["GPOS"].table
        for lk in gpos.LookupList.Lookup:
            for st in lk.SubTable:
                if getattr(st, "LookupType", lk.LookupType) == 9:
                    st = st.ExtSubTable
                if st.LookupType != 2:
                    continue
                if st.Format == 1:
                    for i, g in enumerate(st.Coverage.glyphs):
                        for pvr in st.PairSet[i].PairValueRecord:
                            v = pvr.Value1.XAdvance if pvr.Value1 and hasattr(pvr.Value1, "XAdvance") else 0
                            pairs.setdefault((g, pvr.SecondGlyph), v)
                elif st.Format == 2:
                    c1 = st.ClassDef1.classDefs; c2 = st.ClassDef2.classDefs
                    for g1 in st.Coverage.glyphs:
                        k1 = c1.get(g1, 0)
                        for g2, k2 in c2.items():
                            rec = st.Class1Record[k1].Class2Record[k2]
                            v = rec.Value1.XAdvance if rec.Value1 and hasattr(rec.Value1, "XAdvance") else 0
                            if v:
                                pairs.setdefault((g1, g2), v)
    except Exception:
        pass
    return pairs

@functools.lru_cache(maxsize=None)
def kerns(weight):
    return kern_pairs(face(weight)[0])

def text_path(text, weight, size, x, y, tracking=0.0, fill="#000"):
    """Returns (path_element, advance_width_px). Baseline at y."""
    font, gs, cmap, hmtx = face(weight)
    kp = kerns(weight)
    sc = size/1000.0
    pen = SVGPathPen(gs)
    cur = 0.0
    prev = None
    for ch in text:
        g = cmap[ord(ch)]
        if prev:
            cur += kp.get((prev, g), 0)
        tp = TransformPen(pen, (sc, 0, 0, -sc, x + cur*sc, y))
        gs[g].draw(tp)
        cur += hmtx[g][0] + tracking*1000
        prev = g
    d = pen.getCommands()
    return f'<path d="{d}" fill="{fill}"/>', cur*sc

def text_width(text, weight, size, tracking=0.0):
    return text_path(text, weight, size, 0, 0, tracking)[1]

# ---- lockup ----------------------------------------------------------------
# Numbers (S = wordmark font-size):
#   mark box 0.96S, centred on the cap-height midline (baseline - 0.35S)
#   gap box→text 0.22S · "Open" 500 ink-2 · "ScreenTime" 700 ink · tracking -0.02em
def lockup_h(t, S=48, pad=None):
    pad = S*0.25 if pad is None else pad
    box = 0.96*S
    gap = 0.22*S
    tr = -0.02
    w_open = text_width("Open", 500, S, tr)
    w_st = text_width("ScreenTime", 700, S, tr)
    W = pad + box + gap + w_open + w_st + pad
    H = box + 2*pad
    base = pad + box/2 + 0.35*S
    body = [f'<g transform="translate({pad:.2f} {pad:.2f})">{mark_inner(t, size=box)}</g>']
    x = pad + box + gap
    p1, adv = text_path("Open", 500, S, x, base, tr, t["ink2"]); body.append(p1)
    p2, _ = text_path("ScreenTime", 700, S, x + adv, base, tr, t["ink"]); body.append(p2)
    return svg(round(W, 2), round(H, 2), "\n  ".join(body))

def lockup_stacked(t, S=40, mark=None, pad=None):
    mark = mark or 2.4*S
    pad = S*0.4 if pad is None else pad
    gap = 0.55*S
    tr = -0.02
    w_open = text_width("Open", 500, S, tr)
    w_st = text_width("ScreenTime", 700, S, tr)
    W = max(mark, w_open + w_st) + 2*pad
    H = pad + mark + gap + 0.7*S + pad
    body = [f'<g transform="translate({(W-mark)/2:.2f} {pad:.2f})">{mark_inner(t, size=mark)}</g>']
    x = (W - (w_open + w_st))/2
    base = pad + mark + gap + 0.7*S
    p1, adv = text_path("Open", 500, S, x, base, tr, t["ink2"]); body.append(p1)
    p2, _ = text_path("ScreenTime", 700, S, x + adv, base, tr, t["ink"]); body.append(p2)
    return svg(round(W, 2), round(H, 2), "\n  ".join(body))

# ---- squircle tile ---------------------------------------------------------
def squircle(size, fill, stroke=None, sw=1):
    # iOS-style superellipse approximated with cubic beziers (n≈5 feel); 64 box
    s = size
    k = 0.3  # curvature handle — smaller = squarer
    r = s*0.28
    c = r*(1-0.55)  # handle offset
    d = (f"M{r} 0H{s-r}C{s-c} 0 {s} {c} {s} {r}V{s-r}C{s} {s-c} {s-c} {s} {s-r} {s}H{r}"
         f"C{c} {s} 0 {s-c} 0 {s-r}V{r}C0 {c} {c} 0 {r} 0Z")
    st = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
    return f'<path d="{d}" fill="{fill}"{st}/>'

def app_icon(small=False):
    t = L
    body = [squircle(64, t["bg"], stroke="#e0dcd4", sw=1)]
    if small:
        # heavier ring, darker track so a 16px tile still reads
        body.append(ring_svg_inner(32, 32, 21, 9, 0.40, t["brand"], "#cfcac0", t["ink"], tick_w=4, tick_len=15))  # = favicon geometry
    else:
        body.append(mark_inner(t))
    return svg(64, 64, "\n  ".join(body))

def app_icon_symbolic():
    # single-colour (GNOME -symbolic): the whole mark in currentColor, gap between tick and arc
    b = ring_svg_inner(8, 8, 5.5, 2, 0.40, "currentColor", "none", "currentColor", tick_w=1.6, tick_len=4.4)
    return svg(16, 16, b.replace('stroke="none"', 'stroke="currentColor" stroke-opacity=".28"'))

def favicon():
    # paper tile in light, warm-dark tile in dark schemes; mark on top
    body = f'''<style>
    .tile{{fill:#f4f2ee;stroke:#e0dcd4}} .track{{stroke:#cfcac0}} .tick{{stroke:#1e1c19}} .arc{{stroke:#2e7d46;fill:none}} .cap{{fill:#2e7d46}}
    @media (prefers-color-scheme: dark){{ .tile{{fill:#201e1b;stroke:#302c27}} .track{{stroke:#423d37}} .tick{{stroke:#f3f0ea}} .arc{{stroke:#46b06a}} .cap{{fill:#46b06a}} }}
  </style>
  {squircle(64, "#f4f2ee", stroke="#e0dcd4", sw=1).replace('fill="#f4f2ee" stroke="#e0dcd4" stroke-width="1"', 'class="tile" stroke-width="1"')}
  <circle class="track" cx="32" cy="32" r="21" fill="none" stroke-width="9"/>
  <path class="arc" d="M32 11A21 21 0 0 1 44.34 48.99" stroke-width="9"/>
  <circle class="cap" cx="44.34" cy="48.99" r="4.5"/>
  <path class="tick" d="M32 3.5v15" stroke-width="4" stroke-linecap="round" fill="none"/>'''
    return svg(64, 64, body)

def tray(kind):
    # 22px symbolic weight: ring r=8, stroke 3, tick 2 × 7
    cx = 11; cy = 11.5; r = 7.5; sw = 3
    col = {"ok": L["brand"], "low": L["warn"], "stopped": L["stop"], "paused": "currentColor"}[kind]
    frac = {"ok": 0.40, "low": 0.90, "stopped": 1.0, "paused": 0}[kind]
    state = "paused" if kind == "paused" else ("full" if kind == "stopped" else "fill")
    track = "currentColor"
    b = ring_svg_inner(cx, cy, r, sw, frac, col, track, "currentColor", tick_w=2, tick_len=6, state=state)
    # the track is currentColor at low opacity so it adapts to any panel
    b = b.replace(f'stroke="{track}" stroke-width="{sw}"/>', f'stroke="{track}" stroke-opacity=".25" stroke-width="{sw}"/>', 1)
    return svg(22, 22, b)

def og():
    t = L
    W, H = 1200, 630
    body = [f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>']
    # stacked lockup, centred a little above the middle
    S = 76; mark = 176; tr = -0.02
    w_open = text_width("Open", 500, S, tr); w_st = text_width("ScreenTime", 700, S, tr)
    top = 128
    body.append(f'<g transform="translate({(W-mark)/2:.2f} {top})">{mark_inner(t, size=mark)}</g>')
    x = (W - (w_open + w_st))/2
    base = top + mark + 44 + 0.7*S
    p1, adv = text_path("Open", 500, S, x, base, tr, t["ink2"]); body.append(p1)
    p2, _ = text_path("ScreenTime", 700, S, x + adv, base, tr, t["ink"]); body.append(p2)
    line = "Set it once. It keeps time."
    Sl = 34
    wl = text_width(line, 500, Sl, 0)
    pl, _ = text_path(line, 500, Sl, (W-wl)/2, base + 72, 0, t["ink2"]); body.append(pl)
    sub = "Screen time for the whole family, on a server you own."
    Ss = 22
    ws = text_width(sub, 400, Ss, 0)
    ps, _ = text_path(sub, 400, Ss, (W-ws)/2, base + 116, 0, t["ink3"]); body.append(ps)
    return svg(W, H, "\n  ".join(body))

# ---- the icon set (24 grid, 2px stroke, round caps/joins, currentColor) -----
ICONS = {
 "family":       '<circle cx="9" cy="8" r="3.5"/><path d="M3 20a6 6 0 0 1 12 0"/><circle cx="17" cy="10" r="2.5"/><path d="M16.5 20H21a4.5 4.5 0 0 0-3.4-4.4"/>',
 "person":       '<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>',
 "laptop":       '<rect x="4" y="5" width="16" height="11" rx="2"/><path d="M2 19h20"/>',
 "add":          '<path d="M12 5v14M5 12h14"/>',
 "settings":     '<path d="M4 7h9M19 7h1M4 17h5M15 17h5"/><circle cx="16" cy="7" r="2.5"/><circle cx="12" cy="17" r="2.5"/>',
 "clock":        '<circle cx="12" cy="12" r="9"/><path d="M12 3v2.5"/><path d="M12 12l4.3-2.5M12 12l-3-1.7"/>',
 "give-time":    '<path d="M20.9 10.5A9 9 0 1 0 10.5 20.9"/><path d="M12 3v2.5M12 12l-3-1.7M12 12l4.3-2.5"/><path d="M18 15v6M15 18h6"/>',
 "allowed-hours":'<path d="M3 12A9 9 0 1 1 16.5 19.8"/><path d="M16.5 19.8A9 9 0 0 1 3 12" stroke-dasharray="2.5 4.5"/><path d="M12 3v2.5"/>',
 "pause":        '<path d="M9 6v12M15 6v12"/>',
 "play":         '<path d="M8 5.5v13l10-6.5z"/>',
 "stop":         '<rect x="5" y="5" width="14" height="14" rx="3"/>',
 "lock":         '<rect x="5" y="11" width="14" height="10" rx="2.5"/><path d="M8 11V7.5a4 4 0 0 1 8 0V11"/>',
 "unlock":       '<rect x="5" y="11" width="14" height="10" rx="2.5"/><path d="M8 11V7.5a4 4 0 0 1 7.6-1.7"/>',
 "key":          '<circle cx="8" cy="16" r="4"/><path d="M10.8 13.2L21 3M18 6l2.5 2.5M15 9l2 2"/>',
 "passkey":      '<circle cx="9" cy="8" r="4"/><path d="M2 21a7 7 0 0 1 11-5.7"/><circle cx="17.5" cy="14.5" r="2.5"/><path d="M17.5 17v5M17.5 20h2"/>',
 "moon":         '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z"/>',
 "sun":          '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/>',
 "ask":          '<path d="M7 12V7a1.5 1.5 0 0 1 3 0v5M10 11V4.5a1.5 1.5 0 0 1 3 0V11M13 11V6a1.5 1.5 0 0 1 3 0v6"/><path d="M7 12l-1.6-1.6a1.5 1.5 0 0 0-2.2 2.1L7 16.5V17a5 5 0 0 0 5 5h1a5 5 0 0 0 5-5v-5"/>',
 "check":        '<path d="M5 12.5l4.5 4.5L19 7"/>',
 "close":        '<path d="M6 6l12 12M18 6L6 18"/>',
 "arrow-left":   '<path d="M19 12H5M11 6l-6 6 6 6"/>',
 "arrow-right":  '<path d="M5 12h14M13 6l6 6-6 6"/>',
 "chevron-down": '<path d="M6 9l6 6 6-6"/>',
 "chevron-right":'<path d="M9 6l6 6-6 6"/>',
 "globe":        '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3c-2.5 2.5-4 6-4 9s1.5 6.5 4 9M12 3c2.5 2.5 4 6 4 9s-1.5 6.5-4 9"/>',
 "apps":         '<rect x="4" y="4" width="6.5" height="6.5" rx="1.8"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.8"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.8"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.8"/>',
 "block":        '<circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/>',
 "offline":      '<path d="M8.5 18.5h9a4 4 0 0 0 1.6-7.7M15.6 7.5A6 6 0 0 0 6.2 11.2a3.7 3.7 0 0 0 .9 7.3"/><path d="M4 4l16 16"/>',
 "warning":      '<path d="M12 4l9 15.5H3z"/><path d="M12 10v4"/><circle cx="12" cy="17" r=".6" fill="currentColor"/>',
 "sign-out":     '<path d="M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4M15 8l4 4-4 4M19 12H9"/>',
 "copy":         '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M15 9V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h3"/>',
 "more":         '<circle cx="5" cy="12" r=".9" fill="currentColor"/><circle cx="12" cy="12" r=".9" fill="currentColor"/><circle cx="19" cy="12" r=".9" fill="currentColor"/>',
 "home":         '<path d="M4 11l8-7 8 7v8.5a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 19.5z"/><path d="M10 21v-6h4v6"/>',
 "week":         '<rect x="4" y="5" width="16" height="15" rx="2"/><path d="M4 10h16M8 3v4M16 3v4"/>',
 "eye-off":      '<path d="M3 3l18 18"/><path d="M10.5 5.3A10.5 10.5 0 0 1 12 5.2c4.5 0 8 3.3 9.5 6.8a11.5 11.5 0 0 1-2.6 3.6M6.6 6.6A11.8 11.8 0 0 0 2.5 12c1.5 3.5 5 6.8 9.5 6.8 1.6 0 3-.4 4.3-1"/><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2"/>',
 "menu":         '<path d="M4 7h16M4 12h16M4 17h16"/>',
 "bell":         '<path d="M6 16v-5a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 21h4"/>',
 "edit":         '<path d="M4 20l4.5-1 10.5-10.5a2.1 2.1 0 0 0-3-3L5.5 16z"/><path d="M14 7.5l3 3"/>',
 "info":         '<circle cx="12" cy="12" r="9"/><path d="M12 11v5"/><circle cx="12" cy="8" r=".6" fill="currentColor"/>',
 "remove":       '<path d="M4 7h16M9 7V4.5h6V7M6 7l1 13h10l1-13"/>',
 "refresh":      '<path d="M20 12a8 8 0 1 1-2.3-5.7"/><path d="M20 3v5h-5"/>',
}

def icon_svg(name):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" '
            f'stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{ICONS[name]}</svg>\n')

def main():
    for n in ICONS:
        write(f"icons/{n}.svg", icon_svg(n))
    write("mark.svg", svg(64, 64, mark_inner(L)))
    write("mark-dark.svg", svg(64, 64, mark_inner(D)))
    mono = ring_svg_inner(32, 32, 22, 7, 0.40, "currentColor", "currentColor", "currentColor", tick_w=3, tick_len=11)
    mono = mono.replace('stroke="currentColor" stroke-width="7"/>', 'stroke="currentColor" stroke-opacity=".25" stroke-width="7"/>', 1)
    write("mark-mono.svg", svg(64, 64, mono))
    write("lockup-horizontal.svg", lockup_h(L))
    write("lockup-horizontal-dark.svg", lockup_h(D).replace("<svg ", f'<svg style="background:{D["bg"]}" ', 1))
    write("lockup-stacked.svg", lockup_stacked(L))
    write("lockup-stacked-dark.svg", lockup_stacked(D).replace("<svg ", f'<svg style="background:{D["bg"]}" ', 1))
    write("app-icon.svg", app_icon())
    write("app-icon-small.svg", app_icon(small=True))
    write("app-icon-symbolic.svg", app_icon_symbolic())
    write("favicon.svg", favicon())
    for k in ("ok", "low", "stopped", "paused"):
        write(f"tray-{k}.svg", tray(k))
    write("og.svg", og())
    print("ok", len(ICONS), "icons")

if __name__ == "__main__":
    main()
