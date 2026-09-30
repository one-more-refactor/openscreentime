#!/usr/bin/env python3
"""Inline icons, marks and rings into board.tpl.html → brand-board.html"""
import os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen

HERE = gen.HERE
T = {"light": gen.L, "dark": gen.D}
TRACK = {"light": {"paper": "#d5d1c9", "card": "#e6e3dd"}, "dark": {"paper": "#423d37", "card": "#302c27"}}

def icon(name, cls="ic"):
    s = gen.icon_svg(name).strip()
    return s.replace("<svg ", f'<svg class="{cls}" aria-hidden="true" ', 1)

def ring(d=120, frac=0.0, state="fill", color="brand", theme="light", track="card", tick=True, cls=""):
    t = T[theme]
    tick = str(tick).lower() not in ("0", "false", "no")
    d = float(d); frac = float(frac)
    sw = round(d*(0.09 if d >= 120 else 0.08 if d >= 48 else 0.07))
    col = t.get(color, color)
    tk = t["ink"] if tick and d >= 40 else "none"
    body = gen.ring_svg_inner(d/2, d/2, d/2 - sw/2 - (sw*0.35 if tick and d >= 40 else 0), sw, frac, col,
                              TRACK[theme][track], tk, tick_w=round(sw*0.43, 1), tick_len=round(sw*1.6, 1),
                              state=state, dash_color=t["ink3"])
    if not (tick and d >= 40):
        body = body.rsplit("\n  ", 1)[0]  # drop the tick element
    c = f' class="{cls}"' if cls else ""
    return f'<svg{c} width="{int(d)}" height="{int(d)}" viewBox="0 0 {d:g} {d:g}" aria-hidden="true">{body}</svg>'

def mark(size=64, theme="light", frac=0.40, state="fill", color=None, cls="", tile=None):
    t = T[theme]
    body = gen.mark_inner(t, frac=float(frac), state=state, color=(t.get(color, color) if color else None), size=64)
    if tile:
        # favicon geometry on a tile (dark preview)
        stroke = "#302c27" if theme == "dark" else "#e0dcd4"
        body = gen.squircle(64, t["surface"] if theme == "dark" else t["bg"], stroke=stroke, sw=1) + gen.ring_svg_inner(
            32, 32, 21, 9, 0.40, t["brand"], t["track"], t["ink"], tick_w=4, tick_len=15)
    c = f' class="{cls}"' if cls else ""
    return f'<svg{c} width="{size}" height="{size}" viewBox="0 0 64 64" aria-hidden="true">{body}</svg>'

def file(name):
    with open(os.path.join(HERE, name)) as f:
        s = f.read().strip()
    return s

def filesz(name, size):
    s = file(name)
    s = re.sub(r'width="[^"]*" height="[^"]*"', f'width="{size}" height="{size}"', s, count=1)
    return s

def kv(argstr):
    out = {}
    for part in argstr.split(","):
        if "=" in part:
            k, v = part.split("=", 1); out[k.strip()] = v.strip()
    return out

def sub(m):
    kind, args = m.group(1), m.group(2) or ""
    if kind == "icon":
        return icon(*[a.strip() for a in args.split(",")])
    if kind == "ring":
        return ring(**kv(args))
    if kind == "mark":
        return mark(**{k: (int(v) if k == "size" else v) for k, v in kv(args).items()})
    if kind == "file":
        return file(args.strip())
    if kind == "filesz":
        n, sz = [a.strip() for a in args.split(",")]
        return filesz(n, sz)
    raise SystemExit(f"unknown placeholder {kind}")

def main():
    tpl = open(os.path.join(HERE, "board.tpl.html")).read()
    out = re.sub(r"\{\{(icon|ring|mark|filesz|file):([^}]*)\}\}", sub, tpl)
    open(os.path.join(HERE, "board.html"), "w").write(out)
    print("built", len(out), "bytes")

if __name__ == "__main__":
    main()
