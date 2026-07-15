# README assets

- `deploy-demo.gif` — the animated hero demo (deploy → enroll → online).
- `gen_demo.py` — regenerates it deterministically (no live services needed):
  `python3 docs/assets/gen_demo.py docs/assets/deploy-demo.gif`
  Requires Pillow and a monospace TTF (JetBrains Mono by default). Edit the
  `PROG` list to change the scripted lines; keep the build hash current.
