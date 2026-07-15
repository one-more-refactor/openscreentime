#!/usr/bin/env python3
"""Generate an on-brand animated terminal demo GIF for the Sentinel README.
Nothing-style: near-black bg, one red accent, LED status dots, corner ticks."""
import sys
from PIL import Image, ImageDraw, ImageFont

OUT = sys.argv[1] if len(sys.argv) > 1 else "deploy-demo.gif"
FONT = "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf"
FONTB = "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf"

W, H = 900, 560
BG = (8, 8, 8)
PANEL = (14, 14, 14)
LINE = (38, 38, 38)
FG = (242, 242, 242)
DIM = (120, 120, 120)
FAINT = (78, 78, 78)
OK = (70, 208, 122)
ACCENT = (215, 25, 33)

f = ImageFont.truetype(FONT, 20)
fb = ImageFont.truetype(FONTB, 20)
fsmall = ImageFont.truetype(FONT, 13)
CW = f.getbbox("M")[2]  # mono char width
LH = 31                 # line height
PAD_X, BODY_Y = 40, 118

# --- program ----------------------------------------------------------------
# ("type", prompt, cmd) | ("led", color, label, value) | ("plain", color, text)
# ("section", text) | ("blank",) | ("hold", frames)
PROG = [
    ("type", "$ ", "deploy/setup.sh --domain sentinel.example.com"),
    ("hold", 8),
    ("led", OK, "generating secrets", "done"),
    ("led", OK, "building server + database", "done"),
    ("led", OK, "waiting for health", "healthy · 856b11b"),
    ("blank",),
    ("section", "ADD DEVICE — paste on the device you're managing"),
    ("type", "$ ", "curl -fsSL https://sentinel.example.com/install.sh | sudo sh"),
    ("hold", 8),
    ("led", OK, "downloading agent  (sha256 verified)", "done"),
    ("led", OK, "enrolling", "done"),
    ("blank",),
    ("online", "test-kid-laptop", "ONLINE"),
    ("hold", 44),
]

frames, durs = [], []
buf = []  # committed lines: list of ("kind", ...)


def draw_led(d, x, y, color):
    r = 5
    # soft glow
    d.ellipse([x - r - 3, y - r - 3, x + r + 3, y + r + 3],
              fill=(color[0] // 6, color[1] // 6, color[2] // 6))
    d.ellipse([x - r, y - r, x + r, y + r], fill=color)


def draw_check(d, x, y, color):
    d.line([(x, y + 2), (x + 4, y + 6), (x + 11, y - 4)], fill=color, width=2)


def base():
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    # panel
    d.rounded_rectangle([16, 16, W - 16, H - 16], radius=10, fill=PANEL, outline=LINE, width=1)
    # corner registration ticks (Panel.tsx signature)
    for cx, cy, dx, dy in [(16, 16, 1, 1), (W - 16, 16, -1, 1), (16, H - 16, 1, -1), (W - 16, H - 16, -1, -1)]:
        d.line([(cx, cy), (cx + 9 * dx, cy)], fill=FAINT, width=1)
        d.line([(cx, cy), (cx, cy + 9 * dy)], fill=FAINT, width=1)
    # header: wordmark + eyebrow
    hx, hy = PAD_X, 44
    d.rectangle([hx, hy, hx + 13, hy + 13], fill=ACCENT)  # the mark
    d.text((hx + 26, hy - 3), "S E N T I N E L", font=fb, fill=FG)
    d.text((W - PAD_X - 210, hy + 1), "zero-trust · self-hosted", font=fsmall, fill=DIM)
    d.line([(PAD_X, 88), (W - PAD_X, 88)], fill=LINE, width=1)
    return img, d


def render(partial=None, cursor=False):
    img, d = base()
    y = BODY_Y
    lines = buf + ([partial] if partial else [])
    for ln in lines:
        kind = ln[0]
        x = PAD_X
        if kind == "blank":
            pass
        elif kind == "type":
            _, prompt, text, shown = ln
            d.text((x, y), prompt, font=f, fill=OK)
            d.text((x + len(prompt) * CW, y), text[:shown], font=f, fill=FG)
            if cursor and partial is ln:
                cx = x + (len(prompt) + shown) * CW
                d.rectangle([cx, y + 3, cx + CW - 2, y + 23], fill=FG)
        elif kind == "led":
            _, color, label, value = ln
            draw_led(d, x + 5, y + 12, color)
            d.text((x + 22, y), label, font=f, fill=FG)
            vx = W - PAD_X - len(value) * CW - 20
            d.text((vx, y), value, font=f, fill=DIM)
            draw_check(d, W - PAD_X - 16, y + 11, OK)
        elif kind == "online":
            _, name, status = ln
            draw_led(d, x + 5, y + 12, OK)
            d.text((x + 22, y), name, font=fb, fill=FG)
            sx = W - PAD_X - len(status) * CW - 18
            # status pill
            d.rounded_rectangle([sx - 12, y - 1, W - PAD_X - 4, y + 25], radius=4, outline=OK, width=1)
            d.text((sx, y), status, font=f, fill=OK)
        elif kind == "section":
            _, text = ln
            d.polygon([(x, y + 4), (x, y + 16), (x + 8, y + 10)], fill=ACCENT)
            d.text((x + 18, y), text, font=f, fill=FG)
        elif kind == "plain":
            _, color, text = ln
            d.text((x, y), text, font=f, fill=color)
        y += LH
    return img


def emit(img, dur=60):
    frames.append(img.convert("P", palette=Image.ADAPTIVE, colors=64))
    durs.append(dur)


# build frames
for op in PROG:
    if op[0] == "type":
        _, prompt, text = op
        cur = ["type", prompt, text, 0]
        step = 2
        for i in range(0, len(text) + 1, step):
            cur[3] = min(i, len(text))
            # blink cursor across sub-frames
            emit(render(partial=cur, cursor=(i // step) % 2 == 0), 45)
        cur[3] = len(text)
        buf.append(cur[:])
        emit(render(), 60)
    elif op[0] == "hold":
        for _ in range(op[1]):
            emit(render(), 55)
    elif op[0] == "blank":
        buf.append(("blank",))
        emit(render(), 40)
    else:
        buf.append(op)
        # small reveal hold
        for _ in range(4):
            emit(render(), 55)

# tail
for _ in range(20):
    emit(render(), 60)

frames[0].save(OUT, save_all=True, append_images=frames[1:], duration=durs,
               loop=0, optimize=True, disposal=2)
print(f"wrote {OUT}: {len(frames)} frames")
