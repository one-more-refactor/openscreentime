# README assets

- `deploy-demo.gif` — the hero demo: set up the server (`deploy/setup.sh`),
  set up a computer (the `install.sh` one-liner), see it online.
- `gen_demo.py` — regenerates it deterministically (no live services needed):
  `python3 docs/assets/gen_demo.py docs/assets/deploy-demo.gif`
  Needs Pillow; the fonts come from `client/fonts/` (Figtree for words, Space
  Mono for terminal lines only). Colours are the brand tokens
  (`web/src/theme.css`). Edit `PROG` to change the script — keep every
  terminal line a real string from `deploy/` or `server/install.sh`.
