# Using OpenScreenTime — a guide for parents

The console, page by page. It assumes the server is running — see
[`DEPLOY.md`](DEPLOY.md). What your family can and can't see about each other
is in [`TRANSPARENCY.md`](TRANSPARENCY.md); read it before you set anyone up.

The short version: **everything works until you block it, and what you block
is really blocked.** The console tells you what actually happened, not what
it hopes happened — a pause that hasn't reached a computer yet says
"Pausing…".

## Signing in

**The first time**, open the setup link the installer printed
(`https://<your-server>/#setup=…`): **Create your household** asks for your
name, then makes a passkey. Nobody else can do this after you.

**After that, two doors** ([`AUTH.md`](AUTH.md)):

1. Type your **name** → your own computer shows a 6-digit code (in its
   OpenScreenTime window, as a notification, or with `ost code`) → type it
   in. For this you need your own computer set up: **Computers → Add a
   computer → Mine**, or **Add my computer** on the Family page.
2. **Sign in with a passkey** — no name needed.

`ost login` on your own computer opens the console already signed in.

**Add a second passkey** under **Settings → Security → Passkeys** (both
parents, or a spare before you travel). You can't remove your last one.
**Lost every passkey and no computer set up?** Whoever runs the server can
give you a one-time sign-in link: `openscreentime-server recover <name>`
([`OPERATIONS.md`](OPERATIONS.md)). It opens Settings so you can add a new
passkey.

Inside, everything just works. Only the keys — unlock and recovery codes,
passkeys, the phone pairing, Who's who, a new install line — ask you to
**confirm it's you** (your passkey, or a code on your own computer). That
lasts 15 minutes.

## Family — the home page

- **The verdict** at the top says how the day is going: "Everyone is within
  their time.", "Mia asked for more.", "Noah is paused."
- **Pause everything** stops every computer in the house. Press and hold to
  do it, so it never happens by accident; **Resume everything** is one tap.
  The note that follows has **Undo**. A computer that's offline pauses when
  it's back, and the note says so.
- **One card per person**: their ring (time used today), their bracket and
  computer, and time left ("27 min left of 1 h 15 min", "12 min today · no
  limit set"). Cards that need you come first: someone asking, then paused,
  then out of time. Your own card is last and says "private to you".
- **Requests** wait on the card: "Asked for 15 more minutes · 4 min ago",
  with **Give 15 min** and **Not now**.
- **Notices** only when something is wrong: a computer that has been offline
  (outside its allowed away time), or a login nobody has sorted yet — with a
  link to **Who's who**.

## A person — Today

Open anyone from the Family page or the **Today** list in the rail.

- **The ring and time left**, what they've used, what you gave, and when
  screens next stop ("Screens stop at 20:00 for bedtime.").
- **Pause / Resume** their computers, and **Give 15 min / Give 30 min** (up
  to 4 hours at once). Given time is extra today and also lifts a stop for
  that long.
- **Where the time went**, as their age allows: which apps were open (in
  minutes, and when in the day) and which sites their computer looked up (as
  a count). Older teens: apps only. Adults: their minutes, nothing more. Sites
  are counted per computer, so a shared computer mixes everyone's lookups.
- **Moments** from the last two days: paused, resumed, time's up, time given,
  someone trying to get around the rules. Quiet on a normal day.
- **Their computers**: Online, Offline ("It keeps today's rules"), or Not set
  up yet.
- **Keys**: the computer's **unlock code** (it changes every 30 seconds and
  works even when the computer is offline) and **recovery codes** — eight
  one-time spares for when your phone isn't to hand. Make them early.

**Edit** next to their name changes their name, face and age. Changing the
age doesn't rewrite their rules.

## A person — Rules

Every change is saved when you make it and reaches their computer within a
minute.

- **Daily limit**: 0 to 8 hours in 15-minute steps. 0 means no limit. One
  limit covers all their computers.
- **When screens can be on**: hours for school days and for the weekend (or
  any time), and a **bedtime**. A window can run past midnight.
- **Blocked**: whole **categories**, single **apps** (open apps are closed on
  their computer), and **sites** by name, plus **safe search**. Nothing is
  blocked until you block it — except what their age starts with
  ([`PROFILES.md`](PROFILES.md)).
- **Earning time**: tasks that earn minutes ("Read for 20 min", 15 min).
  Asking for more time from their computer names the first task; you still
  answer **Give** or **Not now**.
- **Remove**: type their name to confirm.

An adult sets their own rules; their Rules page says so, and you can't change
them.

## Computers

- **Status**: Online, Offline, Away (allowed), Paused, or Not set up yet.
- **Pause / Resume** one computer.
- **Allow offline…** for 1 hour, 4 hours or until tomorrow morning — a laptop
  going on a trip isn't a problem to report.
- **Details**: **Who's who** (which login is which person), "Is it
  answering?", the agent version, **Rename** and **Remove**.
- **Add a computer**: say whose it is (yours, a person's, or shared), name it,
  and paste the line it gives you on that computer, as root:

  ```sh
  (wget -qO- https://your-server/install.sh || curl -fsSL https://your-server/install.sh || echo "echo \"Couldn't download the installer from https://your-server — is the address right and the server up?\" >&2; exit 1") 2>/dev/null | sudo OST_TOKEN=<token> sh -s -- --server https://your-server
  ```

  It works once, within 24 hours. Linux only for now. A computer with a
  desktop gets the build with the app window and the graphical lock. The
  installer also adds what filtering websites needs (dnsmasq and nftables) if
  the computer doesn't have them; if it can't, the computer still keeps time
  and Family says it can't filter websites. On a console without https the
  line ends in `--insecure-http` — fine at home, not across the internet.
- **Remove** frees the computer: the next time it's online it takes
  OpenScreenTime off itself, and anyone paused or out of time there gets their
  screen back.

**Who's who.** Every login on a computer is its own person. Setting up a
computer for Mia links one login to her; any other login becomes a new
person marked "not sorted yet". On a child's computer they get a child's
rules; on your own computer they get rules that enforce nothing, so a guess
can never lock you out of your own machine. Sort them under Who's who.

## Settings

**You** (your name and how you sign in), **Appearance** (light, dark, or
match your system), and **Security** behind "Confirm it's you": passkeys,
every computer's unlock code, the **phone** (pair Telegram to get alerts and
answer requests with one tap), and paired companions if you still have one.

## Keeping time for yourself

On **Me** (or **My computer**, if it's just you): your ring, **My daily
limit** (a hard stop with warnings at 15, 5 and 1 minute), **My focus hours**
and **Sites I block for myself** — blocked during your focus hours, or all day
if you set none — and your week. Nobody else in the household sees your
apps or your sites. (Another parent in the same household can currently
open your rules; a child's view never can.)

## What the person experiences

1. **Warnings** at 15, 5 and 1 minute before any stop — their limit, bedtime,
   the end of their hours, or a pause you planned.
2. **The stop**: the screen switches to the OpenScreenTime lock — "Time's up
   for today", "Bedtime until 07:00", "Paused by a parent" — and their apps
   are paused, not closed. If a stop comes without warning (you just changed
   a rule), they get a short save-your-work countdown first. Your pause is
   immediate.
3. **Ways back**: **Ask for more time** (Kid and teens), you type the
   **unlock code** at the lock (30 minutes, even offline), or you give time
   or resume from the console.

The unlock code also answers `sudo` on a child's computer, so you can
administer it and they can't.

## Honest limits

- **Blocking is at DNS.** Anti-bypass rules (forced DNS, DNS-over-HTTPS and
  -TLS, Tor) are on for everyone under 18, but a determined teenager with a
  hotspot or a VPN you haven't blocked can still get around a block. A blocked
  site looks like a site that won't load; there's no explainer page.
- **An offline computer** keeps today's rules and counts time, but a pause
  you send lands when it's back.
- **Someone with root and physical access** can eventually remove the agent.
  They can't do it quietly: the computer goes dark on your Computers page. See
  [`TAMPER.md`](TAMPER.md).
- **There is no remote shell.** Nothing in OpenScreenTime reaches a terminal
  or the files on a computer.
