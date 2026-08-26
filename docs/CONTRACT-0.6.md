# Contract 0.6 — the passive turn

The operator's reframe, verbatim in spirit: OpenScreenTime has been a strict
gatekeeper wearing a friendly coat. 0.6 turns it into a **passive, visual
family mirror with hard edges only where you explicitly drew them**. It shows
what's happening; it blocks only what you said to block — and what it blocks,
it blocks for real. Help people reduce; don't cut them off.

When this conflicts with older docs (including `OPENSCREENTIME.md`'s bracket
table and `DESIGN.md`'s zero-trust framing), **this wins**.

## 1. The blocking model flips

- **Allow by default, everywhere, every bracket.** No default-deny DNS, no
  allowlist-only mode, no "approved sites only". The network is open.
- **What is blocked is strictly enforced.** The blocklist — categories, apps,
  custom sites — is a hard edge: DNS sinkhole + nftables for the blocked
  apps' and sites' traffic, kernel-level app stops. No soft-block.
- Brackets differ by **limits, autonomy, and which categories come
  pre-blocked** (little/kid: adult content, gambling, dating, VPNs & proxies;
  teens: adult content, gambling), never by network model.
- The rules UI speaks this: "Everything works unless you block it. What you
  block is really blocked." The zero-trust copy ("BLOCKED BY DEFAULT — add
  exceptions") dies.
- `Policy`: the `websites.approved` allowlist concept is retired from
  enforcement (kept parsing-compatible, ignored); `websites.blocked`,
  `blocks.categories`, `blocks.apps` are the whole story.

## 2. Login: the client is the key

The idea: **install the client once, then never touch a terminal again.**

- **Web login = username only.** No email field. No OIDC button for now (the
  server keeps its OIDC endpoints; the UI hides them). No lockout preview on
  the login screen — no marketing panel at all; the glance belongs to the
  console after sign-in, not to the door.
- **The client approves everything, including login.** Entering a username
  sends a login request to that person's online device(s). The agent shows a
  small approval surface — the tray where there is one, a popup (egui)
  otherwise: "Someone is signing in as Mia on the web. Approve?" One click
  approves; the browser session completes. PKCE-style binding: the browser
  holds a code verifier from before the request; only the same browser can
  redeem the approval.
- **Passkey stays as the fallback** for phones and unmanaged browsers — as a
  **discoverable credential**: one "Sign in with passkey" button, no field.
- **Connecting a device is site-mediated**: the console shows the one-line
  GitHub install command; the freshly installed client prints a short code
  once; the console asks for it (or the enroll token flow keeps working) —
  after that single moment, the terminal is never needed again.

## 3. The console shows the day, not the log

- **"My screen time" leaves the nav.** The parent appears **in Family**: their
  own card in the grid, counted in the household's day. A member session
  still gets the one-page view — that page is their whole console.
- **Where the time goes** (kids AND parents): per **app**, per **site**, per
  **device**, per **hour of the day**. v1 attribution on the agent:
  - apps: catalog-matched process sampling (30 s ticks) per OS user;
  - sites: dnsmasq query activity bucketed per hour (labeled as activity —
    honest about being a signal, not a stopwatch);
  - schema: `usage_slices (device_user_id, hour, kind app|site, key, seconds)`,
    reported with heartbeats, summed by the server.
- **No raw logs in the family UX.** The child page's "Recent activity" feed
  is replaced by visual moments (paused, time up, tamper — as cards, only
  when they happened). The full event feed lives with the machinery
  (Devices), for the operator.
- **Icons everywhere they anchor recognition**: every child gets a stable
  avatar (deterministic default, parent-pickable emoji); every catalog app
  gets a glyph tile (bundled SVGs for the known set, letter tiles as
  fallback); the site gets a favicon (the ring).
- **Real introductions**: a parent's first login lands in a setup flow
  (household → first person → connect a computer → healthy defaults), not an
  empty grid. A member's first visit gets the transparency intro.

## 4. Enforcement that knows when it's lying

- The agent must **detect silent failure**, not just log actions: after a
  lock, read back the freezer/session state and *verify the stop is real*;
  after a DNS block, resolve a blocked domain and expect the sinkhole. A
  failed probe is `enforcement_degraded` — a critical event the console
  surfaces as plainly as a tamper.
- "Your tests are fine" is not the bar. The bar is §6.

## 5. Distribution moves to GitHub

- `install.sh` and the agent's daily self-update pull the binary from **GitHub
  releases** (the repo is public), sha256-verified against the release
  manifest. The server's bundled `/api/agent/latest` remains as fallback for
  air-gapped installs.

## 6. Verification: adversaries, not applause

The finish line is an **independent adversarial round**, not a green CI run:
separate testing agents, each with its own view and a container/VM to work
in —

- **the parent**: is the day glanceable, do the flows work end to end;
- **the child**: is the one-page view honest, does asking work, is the stop
  plain;
- **the security auditor**: sessions, tokens, the confirm corner, the
  Telegram surface;
- **the circumventing kid**: clock games, killing the agent, DNS end-runs,
  VT switches, freezer pokes — does the lock actually bite (§4), does the
  server notice under-reporting.

What they find gets fixed before the release is called done.
