# Using Sentinel — a guide for parents

This is the day-to-day guide to the web console: enrolling devices, setting policy, granting
time, and reading what the system is telling you. It assumes the server is already running —
see [`docs/DEPLOY.md`](DEPLOY.md) for standing it up. This document is about the product, not
the infrastructure.

The short version of what this product is: **default-deny, honestly reported.** Nothing is
allowed until you allow it, and the console never tells you something happened when it only
queued. Where there's a real limitation — an offline device you can't lock instantly, a kid
with root who can ultimately unplug the machine — this guide says so instead of pretending
otherwise.

## First login & passkeys

Sentinel is passkey-only — no passwords, anywhere. The first time anyone opens the console,
the **FIRST ADMIN** tab on the login page is open: enter an email and register a passkey, and
the tenant is bootstrapped around that account. The instant that first admin exists, the
registration endpoints start refusing new accounts (`403 registration_closed`) — a public
Sentinel URL can't be hijacked by whoever finds it first. If you need a second parent to have
their own admin login later, the existing admin sets `SENTINEL_OPEN_REGISTRATION=1` on the
server briefly (see DEPLOY.md), or — more simply — adds another passkey to the *same* account.

To add a second passkey (e.g. so both parents can unlock the console from their own phone or
laptop, or so you have a backup if you lose one device): go to **Settings → Passkeys** and
click **+ ADD PASSKEY**. Every passkey is listed with when it was added and last used, and can
be removed individually — except your last one. Sentinel will not let you delete your only
passkey; you'd lock yourself out, and there's no password reset to fall back on. Add a spare
before you travel.

## Enrolling a device

From **Devices**, click **+ ADD DEVICE** and give it a name (e.g. "Living Room PC"). The
device is created in a `pending` state and you're shown a one-line install command:

```sh
curl -fsSL https://your-server/install.sh | sudo SENTINEL_TOKEN=<token> sh -s -- --server https://your-server
```

Run that as root on the target Linux machine. It downloads a sha256-verified agent binary,
enrolls it against the token, and installs it as a systemd service. There's also a manual
path (build from source, `enroll` + `install-service`) behind the "MANUAL INSTALL" disclosure
if you're not using the prebuilt binary.

The enroll token is **single-use and expires after 24 hours**. If it expires before you get to
the device, or you need to re-run the install, open the device's detail page and click **SHOW
ENROLL COMMAND** (only available while the device is still `pending`) to generate a fresh
token with a new 24-hour window. Once a device has actually enrolled it holds its own bearer
token and this option disappears — a re-enroll at that point means deleting and re-adding the
device.

Sentinel can also find things for you: **LAN DISCOVERY** on the Devices page lets an online
device scan its local subnet (ARP + a light port sweep) for other hosts. Results show IP, MAC,
and open ports (the hostname/vendor columns exist but aren't populated yet), each with an
**ENROLL →** shortcut that pre-fills the add-device name. Scans only run when you trigger them — nothing scans in the background.

## Understanding the device list

Each device card shows a status LED with one of four real states:

- **online** — the agent has an active connection to the server right now.
- **offline** — no active connection. This can mean anything from "the laptop is asleep" to
  "someone pulled the network cable." By itself it is not alarming.
- **locked** — an admin lock is currently applied and confirmed delivered to the agent.
- **pending** — created but not yet enrolled; waiting on the install command to run.

Below the LED, the card shows either "SEEN `<relative time>`" for a normal offline device, or
a **GONE DARK `Nd`** badge once a device has been offline for **7 or more days**. That
threshold exists because a brief network hiccup is normal, but a week of silence usually means
something else happened — the agent was killed, the device was wiped, or someone is
deliberately keeping it off the network. Gone dark isn't a special status the server sets; it's
just what the console calls a long, unexplained silence. See "What to do when a device goes
dark" below for what to actually check.

Devices also carry a **TAMPER L3** chip when Level 3 lockdown is enabled (see below), and a
**LOCK PENDING** chip when a lock/unlock command was sent while the device was offline — it
will apply automatically the moment the device reconnects.

## Locking a whole device, honestly

Every device card and the device detail page has a **LOCK** / **UNLOCK** button. Clicking it
sends an immediate command over the agent's live connection. If the device is online, the
lock lands right away and the card flips to `locked`.

If the device is **offline**, clicking LOCK does not lie to you: the card does not flip, and
you get a toast — "LOCK QUEUED — APPLIES WHEN DEVICE RECONNECTS." The command sits queued on
the server and is delivered the instant the agent reconnects. There is no way to force an
instant lock on a device that isn't talking to the server; nothing can be. The same honesty
applies to unlock.

**Tamper resistance** (per device, under **TAMPER RESISTANCE** on the device page) has two
levels. Level 1 is the default on every device: a root-owned, auto-restarting systemd service,
boot persistence, and real-time tamper alerts. Level 3 — "MAXIMUM LOCKDOWN" — additionally
disables TTY switching and locks the systemd unit against a `systemctl stop` from the managed
user. It requires an explicit confirm because **it can lock the admin out too**; the recovery
paths are the parent PIN (`sentinel-agent unlock` run locally on the machine, or typed into
the lockout screen), the local `sentinel-admin` account (exempt from every lockdown rule), or
dropping back to Level 1 from the console. Read the confirmation dialog before you enable it.

And the honest limit that applies to all of this: if the person at the keyboard has physical
access and root, no software lock is unbypassable — only expensive and detectable. Sentinel's
promise is deterrence plus real-time alerting, not magic. See `docs/TAMPER.md` for the full
threat model.

## Profiles & presets

**Profiles** define policy — DNS, firewall, screen time, gamification — and are assigned per
Linux user account on a device (so a shared family computer works correctly; each login gets
its own limits). Every tenant starts with three presets, editable but not deletable, plus
whatever custom profiles you create.

**Kids** — locked down:
- DNS allowlist only: `wikipedia.org`, `khanacademy.org`, `pbskids.org`, `scratch.mit.edu`,
  `duolingo.com`. Safe search forced on.
- Screen time: **60 minutes/day**, allowed windows 15:00–19:00 on weekdays and 09:00–19:00 on
  weekends. Bedtime hard-block 20:00–07:00.
- Full network lockdown: DNS forced, DoH/DoT/Tor/VPN all blocked. Offline hard-lockdown after
  **7 days** with no server contact.
- Earn-time on: "Read for 20 min" and "Finish chores" each worth 15 minutes. Lockout challenge
  is a math problem.

**Teen** — trusted but guarded:
- DNS allowlist: `*.wikipedia.org`, `github.com`, `google.com`, `youtube.com`, `duolingo.com`,
  `*.edu`.
- Screen time: **180 minutes/day**, windows 07:00–21:00 weekdays / 08:00–22:00 weekends.
  Bedtime 22:30–06:30.
- Lighter lockdown: DoH/DoT/Tor blocked, VPN and forced-DNS left off. No offline
  hard-lockdown (`offline_lockdown_days: 0`).
- Earn-time on: "Finish homework" worth 20 minutes. Lockout challenge is a 60-second wait, not
  a puzzle — less friction, since the trust model here is different.

**Default** — baseline for anyone not yet assigned a real profile: DNS allows everything
(`*`) with safe search on, no screen-time limit, no gamification. This exists so a
newly-discovered OS user on an enrolled device isn't silently blocked before you've had a
chance to decide what they should get — assign Kids or Teen (or a custom profile) as soon as
you notice them.

Every profile is edited through the same zero-trust form: DNS allow/block lists, firewall
ports, an anti-bypass "Network Lockdown" section (force DNS, block DoH/DoT/Tor/VPN, and an
offline-lockdown-after-N-days setting), screen time with day/time windows and bedtime,
gamification (earn tasks, lockout challenge type, streak nudges), and the parent PIN. Presets
can be edited in place; custom profiles can be duplicated from any existing one and deleted
once nothing is assigned to them.

## Screen time day-to-day

Limits are tracked **per Linux user account**, not per device — the device detail page's
**USERS · SCREEN TIME TODAY** panel lists every OS user Sentinel has seen on that machine, each
with a usage bar (used minutes vs. earned minutes, reset daily) and a profile picker.

What the child actually experiences, in order, as their time runs out:

1. **10 minutes left** — a one-time nudge: "N MIN LEFT TODAY — GOOD TIME TO FINISH UP."
2. **2 minutes left** — a more urgent one-time nudge: "N MIN LEFT — WRAP UP AND SAVE NOW."
   (If bedtime is configured, a separate "BEDTIME IN N MIN — WIND DOWN" nudge fires in the
   15 minutes before bedtime starts.) Each of these fires at most once per user per day.
3. **Time's up** — a full-screen lockout appears (Duolingo-style: black background, dot grid,
   mono type) along with a **60-second countdown** — "SCREEN PAUSES IN 60 SECONDS. SAVE YOUR
   WORK." Nothing freezes yet. This grace period exists specifically so a freeze never looks
   like a crash and never eats unsaved work.
4. When the 60 seconds elapse, the user's session is frozen (a soft freeze via cgroups — not a
   logout, not a kill, just paused) until more time is available.

If earn-time is enabled on the profile, the lockout screen's primary action offers the first
configured task instead of a bare dismiss (e.g. "EARN 15 MIN — Read for 20 min"). On the
headless agent (no GUI), that offer is auto-filed as a pending earn request the moment the
lockout fires, so the request is already waiting for you by the time anyone asks.

**Granting extra time today**, no request needed: on the device detail page, each user has
**GRANT TIME** buttons for +15 / +30 / +60 minutes. This credits the ledger immediately and
pushes a live update to the agent — the toast says "applies within ~10s," which is the
enforcement tick interval, so it's not instant but it's fast. Grants larger than 240 minutes
are rejected by the server as a sanity check.

**The earn/approve flow**: the **Approvals** page lists every pending earn request — which
child, which device, which task, how many minutes, how long ago. **APPROVE +N** credits the
same ledger and pushes the same live `credit_time` update as a manual grant; **DENY** tells the
agent to clear the pending state so the child can ask again (rather than leaving it stuck on
"waiting for approval" forever). A **RECENTLY DECIDED** panel below lets you filter past
approvals/denials for a paper trail.

## The parent PIN

The parent PIN is not a console login — it's a **local override**, typed at the device itself,
for when the device can't or shouldn't wait for the server. It's set per profile, under the
profile editor's **PARENT PIN** section (the field never shows the current PIN back to you,
only whether one "IS SET"; typing a new value replaces it, and there's an explicit CLEAR PIN
action). It's hashed with Argon2 on the server and shipped to the agent as a hash — never in
plaintext.

What it unlocks, and for how long:
- Typed into a lockout screen as the answer to a **"PARENT PIN" challenge**, or offered
  alongside any other challenge type as a standing master escape (a parent physically present
  can always get in) — grants **30 minutes** of unlocked time.
- Solving a plain math challenge on its own (no PIN) grants a shorter **5-minute** breather —
  enough to matter, not enough to hand back the evening.
- It's also the only way through a **whole-device admin lock**, an **offline hard-lockdown**
  (see Profiles above — a device that hasn't reached the server in `offline_lockdown_days` days
  locks itself exactly like an admin lock), and a Level 3 tamper lockdown, all without needing
  the server to be reachable.

**If no PIN is configured on a profile, that override path simply does not exist for it** —
this fails closed, not open. A wrong PIN never falls through to grant anything, and an unset
PIN is never treated as "PIN not required." If you want a manual override available at the
device, you have to set one.

## DNS & network filtering, honestly

Every profile is default-deny for both DNS and firewall — nothing resolves or connects unless
you've explicitly allowed it. What the child sees when they hit something blocked, today, is
whatever their browser shows for a domain that doesn't resolve — an NXDOMAIN, "can't reach this
site," "server not found," depending on the browser. **There is no Sentinel-branded explainer
page yet.** It looks like the site is broken or offline, not like a filter. If you're
troubleshooting "the internet doesn't work" complaints, this is the first thing to check —
have them try a site that should be allowed, or check the allowlist on their profile.

The **Network Lockdown** toggles on a profile (Force DNS, Block DoH, Block DoT, Block Tor,
Block VPN) close off the common ways a technically capable kid gets around the filter above —
each is a real firewall rule, not a suggestion. They're on by default in the Kids preset and
mostly off in Teen, reflecting the different trust levels those two presets are built around.

## The events feed

**Events** is the audit log: every lock/unlock, policy application, screen-time exceed,
earn-time request/grant/decision, tamper signal, enrollment, and discovery scan (plus
historical `ssh` entries from the removed remote-shell feature — see below),
filterable by device, type, and severity (info / warn / critical), with a free-text search over
event payloads. It's also linked from each device's detail page, scoped to that device. If
something surprising happened — a device suddenly locked itself, a lockdown engaged, a PIN was
used — this is where to see exactly when and why.

## Remote SSH — removed

Earlier versions had a **SHELL** button that opened a real root terminal to the device,
always disclosed to the child while it was live. That feature has been **removed
entirely** — there is no remote shell at all anymore, and nothing in Sentinel can reach a
terminal or the files on a managed device. Everything you can do as a parent goes through
this console. The promise to the child got simpler and stronger in the process: instead of
"a shell is never open without you knowing," it's now "there is no shell."

If shell sessions were ever opened on a device in the past, they're still visible as `ssh`
entries in the events feed — the audit record survives the feature's removal. A possible
future replacement (a secure reverse tunnel for native SSH and remote desktop) was
considered and deferred; if it ever ships, it will be just as loudly disclosed.

## What to do when a device goes dark

"Gone dark" (the badge that appears after 7+ days offline) is a symptom, not a diagnosis. A few
things can cause it, roughly in order of likelihood:
- The machine is genuinely off or in long-term storage.
- Someone disabled networking or pulled a cable — the agent's NetworkManager guard fires a
  `tamper` event for exactly this if it can (check Events, filtered to that device).
- The agent process was killed or the service stopped — Level 1 tamper protection auto-restarts
  it and reports the stop attempt, but a sufficiently determined and privileged user can still
  win a given round.
- The server itself was unreachable from the device's side for a long stretch — if the
  profile's `offline_lockdown_days` is set (7 days on the Kids preset, off by default on Teen),
  the device will have locked *itself* down once that threshold passed, which is visible as an
  `offline_hard_lockdown` event once it reconnects.

Check that device's **Events** feed first — filter by device and look for `tamper` entries
around the time it went quiet; the agent buffers undeliverable events in memory and re-sends
them on reconnect (a reboot while offline loses that buffer, but the gap itself stays visible
as gone-dark time), so you'll usually get a real answer instead of just silence. If you truly need to act while it's still dark, remember a LOCK you issue now only
applies once the device reconnects (see "Locking a whole device, honestly" above) — there is no
remote kill switch that works against a device that isn't talking to you.
