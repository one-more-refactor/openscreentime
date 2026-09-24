# Brand — the house clock

OpenScreenTime is the clock on the kitchen wall, not a cop at the door:
everyone reads the same clock, you hang it once, it doesn't nag, it doesn't
lie, and it works for one person too.

`board.html` is the brand board (open it in a browser): the idea, the mark and
its geometry, colour and type, the icon set, the product in the brand, and the
voice. Every file here comes from one source:

```sh
python3 brand/gen.py     # mark, lockups, app/tray/favicon icons, og card, icons/
python3 brand/build.py   # board.html from board.tpl.html
```

`gen.py` needs `fontTools` (the lockup text is outlined from
`client/fonts/Figtree.ttf`). `icons/` are 24 px monoline SVGs (stroke 2, round
caps and joins, `currentColor`) used by both the web console and the device
client — draw a new icon here, never inline in a component.
