# Changelog

All notable changes to OpenScreenTime, newest first. Sections below `0.4.0`
were written when the product was called Sentinel and keep that name — they
describe what actually shipped. Each version's section becomes
the GitHub Release notes verbatim (see `.github/workflows/build.yml`), so it
is written to be read by a person: the first paragraph says what changed in
plain language, the bullets carry the detail. Unreleased work accumulates
under `[Unreleased]` and moves into a version section when a release is cut.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning:
[SemVer](https://semver.org/) — pre-1.0, a minor bump means new features, a
patch bump means fixes only. The agent self-updates by comparing this version
(`x.y.z`, from the crate metadata) against its server's bundled build.

The project is **alpha**: every release is published as a pre-release and
there is no stable version. See the notice at the top of `README.md`.

## [Unreleased]

**Headline: the house clock. Set it once. It keeps time.** OpenScreenTime is
now the clock on the kitchen wall, not a cop at the door — one look, one ring
and one set of words from the console to the lock. A stop is a real lock on
its own screen that can never strand anyone; only real use counts; signing in
has two doors and no passwords or authenticator apps; an adult can keep time
for just themselves; and the server looks after itself.

**For parents**
- **A rebuilt console.** Family, a person's page in two parts — **Today**
  (time left, requests, Pause, Give 15/30 min, where the time went, the keys)
  and **Rules** (daily limit, when screens can be on, bedtime, what's blocked,
  earning time) — **Computers** and **Settings**. Every ring means time used
  today, filling clockwise from a tick at twelve; red means time's up and
  nothing else; a pause is calm, never red. Icons, type and colours come from
  one brand board.
- **One rules model.** The "Protection" slider that overwrote your rules is
  gone; so are "Block account", "Ping" (now "Is it answering?") and the
  Profiles, Events and Approvals pages.
- **Sign-in: two doors.** Type your name and your own computer shows a
  6-digit code, or use a passkey. Pausing, giving time and changing rules just
  work; only the keys (unlock and recovery codes, passkeys, pairing, Who's
  who) ask you to confirm it's you. Lost every passkey? The operator runs
  `openscreentime-server recover <name>` for a one-time link.
- **Who's who.** Every login on a computer is its own person; a login nobody
  has sorted says so on the Family page.
- **Fair numbers.** The day's time is one budget across all of a person's
  computers, filed under each computer's own local day, so "today" is the
  same everywhere and midnight no longer raises false alarms.

**For children and teens**
- **The lock.** When time is up the screen switches to the OpenScreenTime
  lock on its own console — "Time's up for today", "Bedtime until 07:00",
  "Paused by a parent". Apps are paused, not closed. The code field has the
  keyboard, and **Ask for more time** is right there. A computer that can't
  show the lock freezes nobody.
- **Warnings at 15, 5 and 1 minute** before every stop, as normal
  notifications — even on GNOME, without a tray.
- **Only real use counts:** the session on screen, with a key, the mouse or
  sound in the last five minutes. A locked screen, a closed lid or an SSH login
  costs nothing. Changing the clock changes nothing either.
- The math and wait challenges are gone; the unlock code is the one way a
  parent opens the lock.

**For an adult keeping their own time**
- **My computer:** your own daily limit (a hard stop with the same warnings),
  focus hours, and sites you block for yourself — blocked in your focus hours,
  or all day. Nobody else sees your apps or sites, and a household of one gets
  a console that says "It's just you so far".

**For the operator**
- **One command:** `deploy/setup.sh --domain …` writes `.env`, starts the
  stack and prints the one-time setup link. `OST_PUBLIC_URL` is the one
  setting; the passkey domain, origin and cookies derive from it.
- **It runs itself:** starts at boot, backs up nightly, updates daily and
  rolls back — image and database — if the new version isn't healthy.
  `/health` checks the database. Server problems (a failed backup or update,
  the database gone) reach your phone once per incident.
- **Computers update from your server**, never downgrade, refuse a build that
  can't run there, and roll back on their own if an update crash-loops. The
  server now bundles a desktop build (app window, graphical lock, companion)
  next to the headless one; `install.sh` picks the right one. Desktop builds
  need glibc 2.35 or newer.
- Event ingest is idempotent and enrollment retry-safe; a half-finished
  install can simply be run again.

**Fixed**
- **Tamper level 3 needs `--tamper-max` on the computer.** The server (or
  `agent.toml`) could raise a computer to level 3 without it. Now a request
  above the computer's ceiling is capped at 1 and says so: the ack carries
  `capped: true` and the console gets a `tamper_level_capped` event.
- **Power off, reboot and suspend work again for everyone.** A polkit rule
  denied them to every login but root — parents and adults on their own
  computers included — and kept laptops from sleeping. With the day's time
  and every stop kept on disk, a restart or a suspend isn't a way around a
  stop. The only rule left is level 3's "can't stop the agent" (root and
  `ost-admin` exempt, and nothing else granted); below level 3 the agent
  deletes the old rule file on its next start.
- **"What can a parent see?" tells the truth for older teens.** It said
  "apps and sites" to everyone; a parent sees an older teen's apps only. The
  page now says what the server returns in `parent_sees` — the same rule
  that decides what the parent's view shows.
- **An adult's page shows a parent their minutes, and that's all.** Their
  moments (time's up, a blocked app, a code typed) showed on the person page.
  The server now leaves events under an adult's or self-managed person's
  login out of `/api/events` and a computer's `recent_events` for everyone
  but them, and the page no longer asks.
- **A parent's own rules are theirs, even from another parent.** Only
  members' own rules were protected; one parent could read and change
  another's through `/api/profiles/:id`. Now that's a 403 for anyone but the
  person, who changes them through `/api/me/rules`.
- **The enroll token stays out of `ps`.** `install.sh` took the token from
  `OST_TOKEN` and then passed it to `ost enroll --token`, in every user's
  process list. It now hands it over in the environment; `ost enroll` reads
  `OST_TOKEN` (or `--token -` for stdin) when `--token` is absent.
- **CI tests what ships, and fails when a test does.** Steps ran `cargo test
  | tee` without `pipefail`, so a failing test passed. Now every step runs
  `bash -eo pipefail`; the server job runs every DB-backed test (the
  ledger's too — one variable, `OST_TEST_DATABASE_URL` or `DATABASE_URL`,
  for all) and fails if they would skip; the client is tested as the
  headless and the `gui,tray` build; the web runs `bun test`.
- **Pause everything says "Press and hold to pause."** It said "for a
  second"; the hold is 600 ms.
- **Website rules work on a stock Debian or Ubuntu desktop.** Its
  NetworkManager carries the dnsmasq *program*, so the installer installed
  nothing and nothing was filtered. It now installs the dnsmasq service,
  set up to run next to systemd-resolved, and makes `/etc/resolv.conf` a file
  the filter owns where it was resolved's link (the link comes back when
  OpenScreenTime leaves) — before, the first network change turned the filter
  off, and with a block in force left the computer with no DNS at all.
  Installed computers set it up on their next update.
- **One thing that can't be applied never takes the rest with it.** DNS, the
  firewall, its lockdown rules and the VPN are applied one by one; a failing
  one is reported on its own and the others still apply.
- **"Can't filter websites" is said once.** A computer that can't apply
  part of its rules sends one moment per part (as a warning, not a critical
  alert), not one per start — a restart or a reboot with the same gap is
  not news.
- **Time given takes the lock down at once**, instead of up to 10 seconds
  later with "This computer is stopped for now" on screen.
- **The stop time the warnings announce holds still.** In the last minute it
  could move by a second and change "ends at 03:24" to "ends at 03:23".
- **A removed computer keeps nothing of OpenScreenTime**: the binary, its
  state, its config and the companions still running are gone too (packages
  it installed stay, with their own config back), and an old sign-in code no
  longer pops up again.
- **No kernel messages over the text lock.**
- **"Where the time went" names the apps people use.** Only the blocking
  catalog could name an app, so an afternoon of Firefox and Text Editor was
  "Nothing yet today". Any app the computer's menu lists now counts while it
  is open (background parts of the desktop don't); `TRANSPARENCY.md` says so.
- **Moments are the right person's.** A parent's own snooze on a child's
  computer showed on the child's page, and a code typed at the child's lock
  never did. Every event about a person is filed under their login (the
  server also reads it from older agents' events); the computer's own — a
  pause, a gap, a lost connection — shows on Computers and on its owner's
  page. A gap the computer has fixed reads as fixed, not red.
- **Signing in by name never just waits.** After 30 s the code page says what
  a code needs (your computer on, your own login) or offers a passkey; when
  the code runs out it says so, with Send a new code and the passkey door —
  the same for every name, so it tells nobody who exists.
- **/me with no computer** says your rules are saved for when one is added,
  instead of promising focus hours nothing enforces.

### Upgrading

- **Migrations run by themselves** on start (0026, 0027, 0030; 0028 and 0029
  don't exist).
- **Logins nobody has sorted get no limits** on a parent's own computer (and
  when the server re-links a login at startup) until you sort them under
  **Computers → Who's who**; on a child's computer they get the Kid rules.
  Where more than one login on a parent's computer was linked to that parent,
  the upgrade unlinks them all — pick yours again in Who's who before you can
  sign in with a code there.
- **The old TOTP 2FA, number-match login approvals and change mode are
  gone.** Sign in with a passkey, or add your own computer and use the code it
  shows.
- **Computers update themselves from your server**; an agent too old to show
  sign-in codes gets them once it has updated.
- **For the graphical lock, a desktop computer needs `cage`.** The agent
  installs it where apt, pacman or dnf has it; without it the lock is a text
  screen on its own console, which works the same.
- An install from before the boot/backup/update timers: run
  `deploy/install-auto-update.sh` once.

## [0.6.1] - 2026-09-14

**Headline: a real app on the device, a warmer console, and a lock that can
never brick the machine.** The client finally shows itself — a proper
OpenScreenTime window a child can open (or that opens on login), in the same
warm, light look as the console, instead of a tray icon GNOME never draws. The
whole console went light-and-friendly by default, and a screen-time stop now
reads calm and on-brand rather than a black alarm screen. Signing in with SSO
for the first time lets you pick your own username. And enforcement can no
longer lock a parent out of their own device.

- **On-device app.** `ost app`: a window showing time left, connection and,
  plainly, what OpenScreenTime can and can't see, with one button to ask for
  more. `install-service` adds an app-grid launcher, an icon, and a login
  autostart; the background companion still delivers notifications and the
  sign-in-approval prompt.
- **Device liveness.** A Ping button beside Pause/Resume on each device; the
  agent answers with its version so a parent can see it is alive.
- **On-brand client.** The lock screen, the app window and the first-run intro
  are all in the warm OpenScreenTime palette now — off-white, real type, the
  activity-ring marque, a calm dark action; red only for a wrong code.
- **Warmer console.** Light is the default look; personal emoji faces for
  children (and a face picker when you add one); calmer motion; a login page a
  password manager can actually read.
- **Pick your name on SSO sign-up.** A first-run SSO login lands on a Welcome
  page to choose a username instead of one derived from your email.
- **Recovery is never locked out.** The firewall always allows SSH from the
  local network, even under a fail-closed lockdown, so a parent can always
  reach a device to unlock it. The server also keeps a connected agent honestly
  shown as "online."

## [0.6.0] - 2026-09-11

**Headline: your name is your key, and the whole thing got red-teamed.** Sign
in with a username and a passkey — no email, anywhere. Every child's page gets
"Keys to the house" (the parent code that always opens their device, held by
OpenScreenTime itself) and a calm Danger zone. A multi-angle audit — brand,
privacy, child psychology and three security red teams — ran against the tree
and its findings are in: two criticals closed, a locked laptop can no longer
be bricked by a dead server, and what a parent can see now scales with a
child's age. Plus everything that had accumulated since 0.5: trust lives at
login, the Telegram bot grows hands, and "My screen time" tells you what you
did.

- **Trust at login (web + server).** A session born from a completed login is
  trusted and mutates freely — no armed window, no veil, no lock in the rail,
  no reduced-presence controls. What still asks, for everyone, is the
  sensitive corner (a device's unlock code and recovery codes, passkeys,
  pairing tokens): one factor opens a 15-minute confirm window, shown only as
  the Security & access gate. A pre-existing session gets `428` on its first
  mutation and one code repairs it for good. Migration `0017`.
- **Sign in with your device, promoted.** `ost login` — the installed client
  vouching for whoever is signed into the computer — is the default way into
  the console on a managed machine, for parents and children. The login page
  says so, leads with the activity ring instead of a lockout preview, and
  speaks sentence case.
- **Telegram companion (server + web).** One bot per deployment
  (`OST_TELEGRAM_BOT_TOKEN`); a parent pairs their personal chat from
  Settings → Security & access with a single-use `/start` code. A paired
  phone gets every alert, can answer a time request with inline ✅/❌ right
  in the message, and serves as a confirm factor — the dialog's Phone tab
  sends one "Was that you?" tap. Long-poll only; nothing listens. The legacy
  `OST_TELEGRAM_CHAT_ID` broadcast still works. Migration `0018`.
- **The week on /me (web + server).** `GET /api/me/history` sums the last 14
  ledger days across the person's devices. The page shows seven bars with
  minutes, the limit as a tick, today outlined and over-limit days in the
  stop color — plus "how does today compare to my usual" and where today's
  minutes went, device by device. All three looks, adults included.
- **Theme slider (web).** The Settings theme control is a three-stop slider
  (Light — Match my system — Dark) that previews live while dragging; the
  broken pinned-mode re-render is fixed at the store.
- **Language.** Modal titles and login stop shouting (sentence case);
  the sample household is neutral.
- **Username + passkey, no email (server + web).** Registration is a
  username and a passkey — the only option, on a fresh install. Login is
  your username → your own computer approves, with "Log in with passkey"
  beneath. Email is retired from the account model and from step-up (TOTP and
  Telegram remain). SSO admins from before 0.6 still match on their retained
  email. Migration `0024`.
- **Keys to the house + Danger zone (web + server).** The rotating parent
  code sits in a calm "Keys to the house" on each child's page. "Block
  account" pauses a child: they can still open their own page and read why,
  their devices lock after a two-minute save-your-work window, and every
  change is refused until a parent lifts it. "Remove child" now erases their
  usage, ledger and events. Migration `0025`.
- **Security — server.** WireGuard `PostUp`/`PreUp` hooks are rejected like
  OpenVPN's (wg-quick ran them as root on every device). A number-match
  sign-in can only be approved from the target account's own login on that
  computer. Sign-in no longer reveals whether a username exists. Heartbeat
  arrays are capped; enroll tokens are hashed at rest (a never-enrolled
  device's pending token must be regenerated).
- **Security — agent.** A parent code at the device clears every whole-device
  lock for good, and `ost unlock` — plus the new root-only `ost recover` — is
  honored by the running agent, so a server that dies while a laptop is
  locked no longer bricks it. The freeze is re-asserted every probe; a
  clock set forward can't mint a fresh budget; bedtime applies to SSH-only
  sessions; a lock refuses to engage when the device has no offline way back.
- **Privacy.** What the hub can see scales with age: apps and sites for
  younger kids, apps only for older teens, nothing for adults and
  self-managed people. Removing a child erases their history.
- **One voice, everywhere.** The first-run intro, the lockout overlay, tray
  notifications and the web overlay all speak sentence case — "Stop — time's
  up for today", the README's own words. DESIGN.md describes the build again;
  the ring is paired with the wordmark (and an OG card); contrast, one focus
  ring and 40 px tap targets throughout. CI is green.
- **Pre-release audit, five lenses (server + agent + web).** An agent can no
  longer name-link an OS login to a parent, and a device may vouch for or
  approve a parent's sign-in only when it is that parent's own declared
  computer (root on a shared kid laptop used to be a parent session). First-run
  registration needs the one-time setup code `deploy/setup.sh` writes to
  `.env` and prints. Decoy sign-in requests now poll exactly like real ones.
  Unmatched `/api` paths are honest 404s, every response carries baseline
  security headers, and the rate limiter trusts the reverse proxy by default.
  On the device, the DNS query log is root-only, and SSH sessions count as
  screen time (they were a loophole).
- **Grandparent-proof (web).** "Remove child" and "Block account" ask first —
  Remove wants the name typed. Section headings and every button are readable
  sentence-case sans. The confirm dialog says what to do when nothing is set
  up yet. "Pause everything" no longer cancels on a drifting thumb, "Resume"
  is ink instead of red, a configured block isn't painted red at rest, and
  light mode gets its three planes back.

## [0.5.0] — 2026-08-23

**Headline: the console owns the keys.** The code a parent types on a
child's computer now comes from OpenScreenTime itself — no authenticator app,
no QR — and the console has an explicit *change mode*: prove it's you once,
change things for fifteen minutes, lock it again. The web got a consistency
and depth pass on top.

- **Change mode (web).** One control in the rail (and the phone drawer): a
  shut lock and *Make changes* while locked; an open lock, the minutes left,
  *Extend* (once) and *Lock* while on. Turning it on or off plays a short
  full-screen veil (≈1.1 s in, ≈0.7 s out; a 150 ms fade under
  `prefers-reduced-motion`). The first locked control you touch asks once;
  nothing pops up again while it is on. A reloaded console asks the server
  whether it is still on (`GET /api/auth/stepup`). Every mutating control sits
  at the same reduced presence while it is off.
- **Unlock codes (web).** *Add a child → step 2* and *Settings → Unlock
  codes* show the live 6-digit code for a computer with a 30-second ring,
  refetched as it rolls; *Recovery codes* makes eight one-time codes (shown
  once, print/copy, "n of 8 left"); *Replace* re-keys the device and warns
  that its recovery codes are cleared. The parent-code QR, the secret text
  and the "write down the backup code" step are gone. Device cards show how
  many recovery codes are left. The parent's *own* authenticator enrolment
  (console 2FA) gains a QR alongside the secret.
- **Consistency & depth (web).** Tokenised elevation (`--elev-1` resting,
  `--elev-2` floating/hover; the dark theme uses a faint top edge instead of a
  drop shadow), the rail as its own plane, hover lift on cards, a pressed
  state on every button, one `PageHead` on every page (eyebrow · title · quiet
  line · actions), one segmented control, `/me` and `/login` on the same
  elevation language. No redesign — the system is the same, less flat.

- **Unlock codes (server).** The device TOTP secret now lives only on the
  server and the agent. `GET /api/devices/{id}/unlock-code` (a sensitive
  read: change mode required) answers the current 6-digit code and how long
  it has left; `POST …/unlock-code/rotate` re-keys and clears recovery codes;
  `POST …/recovery-codes` mints eight one-time 8-digit codes (stored as
  HMAC-SHA256 keyed by the device secret, shown once) and `GET …/recovery-codes`
  reports how many are unused. `POST /api/devices` no longer returns a
  secret, the `/parent-code` routes are gone, and enrolment mints no
  recovery PIN any more. Migration `0016`.
- **Unlock codes (agent).** The policy bundle carries the recovery-code MACs;
  the agent verifies the rotating code and the recovery codes offline (a
  spent recovery code is retired on the device at once and on the server as
  soon as the event lands), and every prompt — lockout overlay, `ost unlock`,
  tray, `sudo` on a managed machine — now asks for the "unlock code from the
  OpenScreenTime console, or a recovery code". `ost status` shows how many
  recovery codes are left. A profile-level `parent_pin_hash` still works as a
  backup code.
- **Change mode (server).** A grant lasts 15 minutes instead of 5.
  `GET /api/auth/stepup` reports it, `POST /api/auth/stepup/lock` ends it,
  `POST /api/auth/stepup/extend` adds 15 minutes once per grant
  (`409 already_extended` after that).
- **Upgrade note.** Authenticator-app entries made for devices under 0.4 keep
  producing valid codes until you *Replace* the device's unlock code; do that
  once, then delete the entries. Devices update themselves from the server.

## [0.4.0] — 2026-08-22

**Headline: it works end to end now.** This release is the one where a
family can actually use it: everyone in the house is an *account* with an age
bracket, the parent's code is an authenticator app instead of a PIN, blocking
is "tap YouTube" instead of typing domains, the console tells the truth about
whether a screen is paused, and a child who opens the console on their own
laptop sees their own page.

- **Everyone has an account.** Children are *members* with a role, an age
  bracket (Little 0–6, Kid 6–12, Younger teen 12–16, Older teen 16–18, Adult)
  and a birthdate the bracket is derived from. Every OS login on a device is
  linked to a person. `GET /api/family` is built from people, not device
  users. Adults (including the parent) can self-track privately.
- **Five bracket presets** replace `kids/teen/default` (the old rows stay
  valid). Each bracket ships with sensible one-click blocks.
- **Parent code = per-device TOTP.** Adding a device shows a QR to scan into
  your authenticator app; the agent verifies codes **offline** (RFC 6238, ±1
  step, single-use, lockout after five misses). Used by the lockout overlay,
  `ost unlock`, the tray, and `sudo` (below). The old recovery PIN survives
  only as the *backup code*.
- **Parent sudo over PAM.** On a managed machine, `sudo` asks for the parent's
  authenticator code (`/etc/pam.d/openscreentime-parent` + a sudoers drop-in
  for the `ost-managed` group). A child can't escalate; a parent can administer
  the laptop without a local password.
- **Block by app and category, one click.** A built-in catalog of apps
  (YouTube, TikTok, Discord, Roblox, Steam, …) and categories (social, adult,
  gambling, games, AI chatbots, VPNs/proxies, …) lives in the policy crate and
  is served at `GET /api/catalog`; the agent expands it into DNS sinkholes and,
  for native clients, process denial. Your AdGuard/Pi-hole is just the DNS
  upstream.
- **Presence is WebSocket-first and lock state is honest.** The agent holds a
  permanent connection (30 s heartbeats, jittered reconnect, HTTP fallback);
  the server marks a device offline the moment the socket closes; `locked` is
  a separate field reported from the kernel's freeze state, and the console
  shows "pausing…" until the device confirms.
- **The child's own page (`/me`)** with three looks — playful (little/kid),
  calm (teens), plain (adults) — picked by bracket, overridable per person.
  Device-voucher autologin now logs in *as the person using that OS account*,
  never as the parent.
- **Ops:** `deploy/push-image.sh` builds the image on a dev box and loads it
  on the host; `deploy/test/` is a systemd container that acts as a managed
  laptop for end-to-end tests; the netavark stale-DNAT trap is documented; the
  server drains on SIGTERM instead of being SIGKILLed at every restart.
- Verified end to end against a real Postgres and a real (containerised)
  managed laptop: enroll → WebSocket presence → catalog sinkhole → pause
  (pending → confirmed) → PAM parent-code sudo (wrong / right / replay) →
  member autologin voucher confined to `/me` → offline/online.

Known limitation: on enrollment every OS login that matches no person by name
is linked to the device's owner (the person you picked in "Add child"). A
second adult account on a child's laptop therefore gets the child's rules
until it is relinked: `POST /api/device-users/{id}/assign-account
{account_id}` moves a login to another person (API only in 0.4; the console
picker comes next).

**Breaking: the product is now OpenScreenTime.** The agent was renamed in the
previous release; this one finishes the job across the server, the deployment
and the docs. There are no compatibility shims for the server side — an
existing deployment needs its `.env` rewritten and its stack recreated.

What changed, and what you have to do about it:

- **Every `SENTINEL_*` environment variable is now `OST_*`** — same names
  otherwise (`OST_PUBLIC_URL`, `OST_TRUST_PROXY`, `OST_OIDC_*`,
  `OST_ALERT_WEBHOOK`, `OST_TOKEN`, …). The old names are not read. Rename
  them in `.env` before redeploying or the server starts with defaults.
- **Container, volume and database names changed**: `sentinel-db` →
  `openscreentime-db`, `sentinel-server` → `openscreentime-server`,
  `sentinel_pgdata` → `ost_pgdata`, and the default Postgres user/database are
  now `openscreentime`. Compose will not adopt the old volume — dump the
  database first (`docs/OPERATIONS.md`) and restore into the new stack, or
  point `POSTGRES_USER`/`POSTGRES_DB` at the old values in `.env`.
- **The admin session cookie is now `ost_session`**, so every logged-in admin
  is signed out once on upgrade. Passkeys themselves are unaffected.
- **New `OST_BIND_ADDR`** (default `127.0.0.1`) publishes the app port on a
  specific host address, for deployments whose reverse proxy runs on a
  different machine. Do not set it to `0.0.0.0`: the server trusts
  `X-Forwarded-For`, so a directly reachable port is a rate-limiter bypass.
- **Crates renamed** to `openscreentime-server` and `openscreentime-policy`.
- **Agent-side identifiers** follow the binary: the nftables table is now
  `inet openscreentime`, the polkit rule `49-openscreentime.rules`, the VPN
  tunnel `wg-quick@openscreentime` / `openvpn-client@openscreentime`, and the
  recovery account is **`ost-admin`** (create it before raising the tamper
  level — the old `sentinel-admin` is no longer exempt from anything).
- The agent tears down a leftover `inet sentinel` table in the same atomic
  `nft` transaction as every apply, so an upgraded device can't be left
  enforcing a stale ruleset that nothing writes to any more.
- Migration files keep their original wording: sqlx checksums them, and
  editing one would fail validation on every existing database.

Headline: **the remote shell is gone**. Sentinel no longer contains a remote
shell at all — everything a parent can do now goes through the UI. The
transparency promise gets simpler and stronger: instead of "a shell is never
open without the device being told", there is no shell to open.

### Removed

- **Breaking**: the remote shell feature, end to end. The
  `POST /api/devices/:id/ssh`, `GET /api/ssh/:id/ws`, and
  `POST /api/ssh/:id/close` routes are gone, the `ssh_open`/`ssh_close`
  command types are gone, and the `ssh_sessions` table is dropped
  (`0008_remove_ssh.sql`). The web terminal and the agent's PTY endpoint are
  removed with them.
- Historical `ssh` **events remain readable** in the event log — the record
  that past sessions happened survives; only the capability is removed.
- A possible future replacement — a secure reverse tunnel carrying native
  SSH+RDP — was considered and deferred; nothing of it ships today.

### Security

A red-team pass on the enforcement and transparency surface. This round lands
the fixes that were surgical and low-risk; the deeper enforcement-logic items
(edge-triggered freeze reconciliation, timezone-anchored day rolls, brick-safe
lockdown gating) are tracked separately.

- **The console now shows the event feed again.** After the Family/Child
  redesign, no reachable page rendered events at all — every tamper the server
  recorded was invisible, quietly breaking the "tampering is never silent"
  promise. The child page now carries a **Recent activity** audit trail, and
  the feed renders the previously-unmapped `evasion`, `enforcement_degraded`,
  and `vpn_profile` types (they used to draw as blank rows) and falls back to
  the raw type name for any future type.
- **Devices no longer force inbound SSH (port 22) open.** The protection slider
  had pinned `allow_inbound_ports: [22]` at every level — a leftover from the
  removed remote shell — exposing the box's own `sshd` (and the polkit-exempt
  `sentinel-admin` account) on every network it joined. The agent opens no
  inbound listener, so this is now `[]`.
- **A device can no longer forge its own lock state.** Command acks were not
  status-guarded, so a device could re-ack any command id it had seen —
  replaying an old `unlock` to appear online, or rewriting the audit timestamp
  on historical rows. Acks now only apply to still-open (`queued`/`sent`)
  commands.
- **Confirmed usage-ledger resets now reach the parent.** The server-side
  anti-cheat `evasion` event was `warn`, but the alert fan-out only pushes
  `critical` — so the one signal the server derives independently of the
  device's honesty never left the console. It is now `critical`.
- **Power masking covers halt and hibernate.** The polkit rule denied
  power-off/reboot/suspend but not `halt`, `hibernate`, or
  `suspend-then-hibernate` — any desktop power menu's Hibernate froze the whole
  machine (agent included) undetected. All power paths are denied now.
- **Level 3 now protects the watchdog units too.** The stop/disable/mask block
  covered `sentinel-agent.service` but not `sentinel-watchdog.{service,timer}` —
  masking the watchdog alone silently disarmed the recovery net. All three
  units are guarded.
- **Enrolling against a plaintext `http://` server is refused** (loopback and
  `.local` excepted for dev/LAN), since plaintext transport makes the
  self-update sha256 check decorative against an on-path attacker.
- Internal server errors no longer leak raw Postgres detail (table/constraint
  names, cast errors) to API clients; the full error is still logged
  server-side.

## [0.3.0] - 2026-08-04

Headline: **VPN profiles**. Drop a WireGuard `.conf` or OpenVPN `.ovpn`
client config on a device's page and that machine routes through your VPN —
the agent keeps the tunnel up, across reboots and config changes, and the
device firewall automatically lets your own tunnel through even with
VPN-blocking lockdown enabled. Plus: server deployments can now update
themselves daily, rolling back automatically if the new version fails its
health check.

### Added

- Per-device VPN profiles: upload in the console (drag & drop), enforced on
  the device as `wg-quick@sentinel` / `openvpn-client@sentinel`. The config
  (it contains private keys) is stored write-only — the console only ever
  shows kind + upload time — and is written on the device root-only. A
  tunnel that isn't actually running (e.g. WireGuard tools not installed)
  raises a critical `enforcement_degraded` alert instead of pretending.
- Removing a profile propagates like setting one: the agent tears the
  tunnel down on its next sync, even in polling mode.
- Optional automatic server updates: `sudo deploy/install-auto-update.sh`
  installs a daily systemd timer around `deploy/update.sh`, which now rolls
  back to the previously running revision if the updated server fails its
  health check — an unattended bad update self-heals.

### Fixed

- The `enforcement_degraded` alerts introduced in 0.2.0 were rejected by the
  database (missing from the events type constraint) — agents buffered and
  retried them forever and the console never saw them. They now land.
- The SSH session reaper's cleanup query was rejected by PostgreSQL on every
  sweep (`make_interval` type mismatch), so sessions stuck in `opening`
  were never cleaned up. Stale sessions now reap after 15 minutes as
  intended.
- Release builds: the Build workflow pinned a Rust older than the project's
  minimum supported version and failed on every run since the MSRV bump;
  release notes extraction produced empty notes on every tag. Both fixed —
  v0.2.0 was the first release to actually ship because of it.

## [0.2.0] - 2026-08-04

The first tagged Sentinel release, and the first one devices can auto-update
to. Headline: **a device that can't actually enforce its DNS rules now says
so loudly in the console** instead of showing a reassuring green light —
previously, on common setups (systemd-resolved distros, missing dnsmasq),
filtering could silently not be in force while everything looked fine.

### Added

- Release pipeline: tagged builds publish the server + static agent binaries,
  checksums, and a container image; enrolled devices pick up new agent
  versions automatically within a day (`auto_update`, on by default).
- `enforcement_degraded` critical events: every gap between "policy accepted"
  and "policy actually enforced" is reported per-cause (no local resolver,
  unpinnable `/etc/resolv.conf`, symlinked resolv.conf) with a plain-language
  explanation of what to do about it.
- `policy_applied` events now carry a `dns_gaps` count, so "applied" and
  "fully enforced" are distinguishable at a glance.

### Fixed

- DNS enforcement no longer hides failures at debug log level: a failed
  `chattr +i` pin, a dnsmasq that failed to start, and re-pin errors in the
  tamper loop all surface as critical events now.
- A symlinked `/etc/resolv.conf` (systemd-resolved's stub) is replaced with a
  real pinned file instead of being written through — the old behavior failed
  open and quiet; the new one fails closed and loud.
- CI: pinned `sqlx-cli` to 0.8.x and raised MSRV to 1.89 so main builds
  reproducibly again.

### Known limitations

- Only the headless x86_64 agent self-updates; desktop builds with the
  `gui`/`tray` features are built locally and must be updated the same way.
- Update trust is sha256-over-TLS from the enrolled server; independent
  binary signing (minisign) is planned.
