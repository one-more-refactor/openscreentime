#!/usr/bin/env python3
"""The README's hero GIF: set up the server, set up a computer, see it online.

Drawn in the brand (brand/board.html): warm paper, ink, the one green, the
ring mark with its tick at twelve, Figtree for words and a mono face only for
what is typed in a terminal. Every terminal line is a real string from
deploy/setup.sh, deploy/lib.sh or server/install.sh.

    python3 docs/assets/gen_demo.py docs/assets/deploy-demo.gif

Needs Pillow. Fonts come from the repo (client/fonts/).
"""
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
FONTS = os.path.join(HERE, "..", "..", "client", "fonts")
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "deploy-demo.gif")

# ---- brand tokens (web/src/theme.css, light) --------------------------------
BG = (0xF4, 0xF2, 0xEE)       # warm paper
SURFACE = (0xFF, 0xFF, 0xFF)
SURFACE2 = (0xEC, 0xEB, 0xE6)
LINE = (0xE6, 0xE3, 0xDD)
TRACK = (0xD5, 0xD1, 0xC9)    # line-2, the ring track
INK = (0x1E, 0x1C, 0x19)
INK2 = (0x57, 0x54, 0x4E)
INK3 = (0x72, 0x6E, 0x66)
BRAND = (0x2E, 0x7D, 0x46)
BRAND_TINT = (0xE4, 0xF1, 0xE8)
BRAND_INK = (0x1C, 0x5C, 0x33)

SS = 2                        # draw at 2x, then downsample: smooth rings and text
W, H = 960, 600


def px(v):
    return int(round(v * SS))


def figtree(size, weight):
    f = ImageFont.truetype(os.path.join(FONTS, "Figtree.ttf"), px(size))
    f.set_variation_by_axes([weight])
    return f


MONO = ImageFont.truetype(os.path.join(FONTS, "SpaceMono-Regular.ttf"), px(15))
F_LOCK_OPEN = figtree(22, 500)
F_LOCK_ST = figtree(22, 700)
F_LINE = figtree(15, 500)
F_STEP = figtree(15, 600)
F_NAME = figtree(18, 700)
F_META = figtree(13, 500)
F_TAG = figtree(13, 600)
F_BIG = figtree(20, 700)

CW = MONO.getlength("M") / SS  # mono advance, in 1x px
LH = 27                        # terminal line height
PAD = 56                       # card inner padding
CARD = (24, 24, W - 24, H - 24)
BODY_Y = 128


# ---- the ring (brand/gen.py geometry) ---------------------------------------
def ring(d, cx, cy, r, sw, frac, color=BRAND, tick=True):
    """Track, then the arc from twelve o'clock clockwise (butt start, round
    end), then the tick on top. Sizes in 1x px."""
    R = r + sw / 2
    box = [px(cx - R), px(cy - R), px(cx + R), px(cy + R)]
    d.ellipse(box, outline=TRACK, width=px(sw))
    if frac > 0:
        d.arc(box, -90, -90 + 360 * frac, fill=color, width=px(sw))
        a = -math.pi / 2 + 2 * math.pi * frac
        ex, ey = cx + r * math.cos(a), cy + r * math.sin(a)
        d.ellipse([px(ex - sw / 2), px(ey - sw / 2), px(ex + sw / 2), px(ey + sw / 2)], fill=color)
    if tick:
        tw, tl = sw * 3 / 7, sw * 11 / 7
        d.rounded_rectangle(
            [px(cx - tw / 2), px(cy - r - tl / 2), px(cx + tw / 2), px(cy - r + tl / 2)],
            radius=px(tw / 2), fill=INK)


def mark(d, x, y, size):
    """The static mark: a 64-unit box, ring r 22, stroke 7, fixed at 40%."""
    s = size / 64
    ring(d, x + 32 * s, y + 32 * s, 22 * s, 7 * s, 0.40)


def check(d, x, y, color=BRAND):
    d.line([(px(x), px(y + 1)), (px(x + 4), px(y + 5)), (px(x + 11), px(y - 4))],
           fill=color, width=px(2), joint="curve")


def text(d, x, y, s, font, fill):
    d.text((px(x), px(y)), s, font=font, fill=fill)


def width(s, font):
    return font.getlength(s) / SS


# ---- the script ---------------------------------------------------------------
# ("step", n, words)        a Figtree caption: where you are
# ("type", lines)           a typed command (continuation lines allowed)
# ("out", line[, ok])       terminal output; ok=True adds the green check
# ("link", url)             the setup link, green
# ("gap",)                  half a line
# ("hold", frames)
# ("clear",)                a fresh terminal
# ("card",)                 the family card, online
TOKEN = "k7Qm…3x"
PROG = [
    ("step", 1, "On your server"),
    ("type", ["deploy/setup.sh --domain ost.example.com"]),
    ("hold", 6),
    ("out", "==> generating .env with fresh secrets"),
    ("out", "==> starting the stack"),
    ("out", "==> waiting for the server to report healthy on 127.0.0.1:8080"),
    ("out", "==> taking the first database backup"),
    ("out", "==> OpenScreenTime is up and healthy on 127.0.0.1:8080.", True),
    ("gap",),
    ("out", "Open this one-time setup link and create your household:"),
    ("link", "https://ost.example.com/#setup=9c1f…e4"),
    ("hold", 34),
    ("clear",),
    ("step", 2, "On the computer you look after"),
    ("type", ["curl -fsSL https://ost.example.com/install.sh \\",
              f"  | sudo OST_TOKEN={TOKEN} sh -s -- --server https://ost.example.com"]),
    ("hold", 6),
    ("out", "Selected the 'desktop' agent build."),
    ("out", "Installed /usr/local/bin/openscreentime"),
    ("out", "Enrolling against https://ost.example.com ..."),
    ("out", "Installing systemd service ..."),
    ("out", "Device enrolled — it should appear online in the console within a minute.", True),
    ("hold", 10),
    ("card",),
    ("hold", 60),
]


def base():
    img = Image.new("RGB", (px(W), px(H)), BG)
    d = ImageDraw.Draw(img)
    x0, y0, x1, y1 = CARD
    # shadow-1, then the card
    d.rounded_rectangle([px(x0), px(y0 + 2), px(x1), px(y1 + 2)], radius=px(16), fill=(232, 229, 223))
    d.rounded_rectangle([px(x0), px(y0), px(x1), px(y1)], radius=px(16), fill=SURFACE,
                        outline=LINE, width=px(1))
    # lockup: mark box 0.96 S, gap 0.22 S; "Open" 500 ink-2, "ScreenTime" 700 ink
    S = 22
    lx, ly = x0 + PAD - 8, y0 + 34
    mark(d, lx, ly - 0.96 * S / 2 + 2, 0.96 * S * 1.35)
    tx = lx + 0.96 * S * 1.35 + 0.22 * S
    text(d, tx, ly - 13, "Open", F_LOCK_OPEN, INK2)
    text(d, tx + width("Open", F_LOCK_OPEN), ly - 13, "ScreenTime", F_LOCK_ST, INK)
    tag = "Set it once. It keeps time."
    text(d, x1 - PAD + 8 - width(tag, F_LINE), ly - 8, tag, F_LINE, INK3)
    d.line([(px(x0 + 1), px(y0 + 76)), (px(x1 - 1), px(y0 + 76))], fill=LINE, width=px(1))
    return img, d


def draw_card(d, y):
    """The family card: the ring filling clockwise from the tick, a face, the
    time left, and the computer — Online."""
    x0, x1 = CARD[0] + PAD - 8, CARD[2] - PAD + 8
    d.rounded_rectangle([px(x0), px(y), px(x1), px(y + 92)], radius=px(16), fill=SURFACE,
                        outline=TRACK, width=px(1))
    cx, cy = x0 + 50, y + 46
    ring(d, cx, cy, 26, 4.5, 0.25)
    d.ellipse([px(cx - 19), px(cy - 19), px(cx + 19), px(cy + 19)], fill=(0xD6, 0xEF, 0xE0))
    text(d, cx - width("M", F_BIG) / 2, cy - 13, "M", F_BIG, (0x1F, 0x6B, 0x45))
    text(d, x0 + 96, y + 20, "Mia", F_NAME, INK)
    text(d, x0 + 96, y + 50, "Kid · Mia's laptop", F_META, INK3)
    left = "45 min"
    text(d, x0 + 330, y + 20, left, F_NAME, INK)
    text(d, x0 + 330, y + 50, "left of 1 h", F_META, INK3)
    tag = "Online"
    tw = width(tag, F_TAG) + 40
    tx = x1 - 24 - tw
    d.rounded_rectangle([px(tx), px(y + 32), px(tx + tw), px(y + 60)], radius=px(14), fill=BRAND_TINT)
    check(d, tx + 12, y + 46, BRAND_INK)
    text(d, tx + 30, y + 37, tag, F_TAG, BRAND_INK)


def render(lines, partial=None, cursor=False, card_y=None):
    img, d = base()
    x = CARD[0] + PAD - 8
    y = BODY_Y
    for ln in lines + ([partial] if partial else []):
        kind = ln[0]
        if kind == "step":
            _, n, words = ln
            d.ellipse([px(x), px(y), px(x + 22), px(y + 22)], fill=BRAND_TINT)
            text(d, x + 11 - width(str(n), F_STEP) / 2, y + 1, str(n), F_STEP, BRAND_INK)
            text(d, x + 34, y + 1, words, F_STEP, INK)
            y += LH + 10
        elif kind == "type":
            _, cmd, shown = ln
            left = shown
            for i, part in enumerate(cmd):
                if i == 0:
                    text(d, x, y, "$", MONO, BRAND)
                vis = part[:max(0, left)]
                text(d, x + 2 * CW, y, vis, MONO, INK)
                if cursor and partial is ln and 0 <= left <= len(part):
                    cx = x + (2 + len(vis)) * CW
                    d.rectangle([px(cx), px(y + 3), px(cx + CW - 2), px(y + 21)], fill=INK)
                left -= len(part)
                y += LH
                if left < 0:
                    break
        elif kind == "out":
            s, ok = ln[1], (len(ln) > 2 and ln[2])
            text(d, x, y, s, MONO, INK if ok else INK2)
            if ok:
                check(d, x + width(s, MONO) + 12, y + 11)
            y += LH
        elif kind == "link":
            text(d, x + 2 * CW, y, ln[1], MONO, BRAND_INK)
            d.line([(px(x + 2 * CW), px(y + 22)), (px(x + 2 * CW + width(ln[1], MONO)), px(y + 22))],
                   fill=BRAND, width=px(1))
            y += LH
        elif kind == "gap":
            y += LH // 2
    if card_y is not None:
        draw_card(d, max(card_y, y + 18))
    return img.resize((W, H), Image.LANCZOS)


frames, durs = [], []


def emit(img, ms):
    frames.append(img)
    durs.append(ms)


buf = []
card = None
for op in PROG:
    kind = op[0]
    if kind == "type":
        cmd = op[1]
        total = sum(len(p) for p in cmd)
        cur = ["type", cmd, 0]
        for i in range(0, total + 1, 3):
            cur[2] = i
            emit(render(buf, partial=cur, cursor=(i // 3) % 2 == 0, card_y=card), 40)
        cur[2] = total
        buf.append(("type", cmd, total))
        emit(render(buf, card_y=card), 60)
    elif kind == "hold":
        img = render(buf, card_y=card)
        for _ in range(op[1]):
            emit(img, 60)
    elif kind == "clear":
        buf = []
        emit(render(buf), 200)
    elif kind == "card":
        card = 0
        emit(render(buf, card_y=card), 60)
    else:
        buf.append(op)
        emit(render(buf, card_y=card), 220 if kind == "out" else 120)

# One fixed palette for every frame, built from the brand tokens and the
# anti-aliasing blends between them, so the one green stays that green and
# nothing flickers between frames.
MINT, MINT_INK, SHADOW = (0xD6, 0xEF, 0xE0), (0x1F, 0x6B, 0x45), (232, 229, 223)
PAIRS = [(fg, SURFACE) for fg in (INK, INK2, INK3, BRAND, BRAND_INK, TRACK, LINE, BRAND_TINT, MINT)]
PAIRS += [(BRAND_INK, BRAND_TINT), (MINT_INK, MINT), (BRAND, TRACK), (INK, TRACK), (INK, BRAND),
          (SURFACE, BG), (SHADOW, BG), (LINE, BG), (INK, BRAND_TINT), (TRACK, MINT)]
colors = []
for fg, bg in PAIRS:
    for i in range(1, 12):
        t = i / 11
        c = tuple(round(fg[k] * t + bg[k] * (1 - t)) for k in range(3))
        if c not in colors:
            colors.append(c)
for c in (BG, SURFACE):
    if c not in colors:
        colors.append(c)
colors = colors[:256]
pal = Image.new("P", (1, 1))
pal.putpalette([v for c in colors for v in c] + [0] * (768 - 3 * len(colors)))
out = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]

# Merge runs of identical frames into one longer frame.
merged, mdurs = [], []
for f, ms in zip(out, durs):
    if merged and f.tobytes() == merged[-1].tobytes():
        mdurs[-1] += ms
    else:
        merged.append(f)
        mdurs.append(ms)

merged[0].save(OUT, save_all=True, append_images=merged[1:], duration=mdurs,
               loop=0, optimize=True, disposal=1)
print(f"wrote {OUT}: {len(merged)} frames")
