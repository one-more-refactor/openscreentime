# What OpenScreenTime knows about you

This is for you — the person whose computer it is, not the parent who set it
up. Every line is checked against the code. If something here is wrong, that
is a bug in the product, not a gap we meant to leave.

## What it is

OpenScreenTime is a program (`openscreentime`) that runs as root on this
computer. It counts your screen time, blocks what your household chose to
block, and stops the screen when your time is up. It reports to a server your
family runs — not to us, and not to anyone else. It isn't hidden: it's the
OpenScreenTime window (`ost app`), your own page in the console (`/me`),
`systemctl status openscreentime-agent`, and a process you can see.

## What a parent sees, by age

Your age bracket decides how much of your day a parent sees. The server
decides this every time a parent looks (`server/src/usage.rs`, `hub_exposure`).

| You are | A parent sees |
|---|---|
| **Little** (0–6), **Kid** (6–12), **Younger teen** (12–16) | Your time today and on past days. **Which apps were open**, in minutes, and when in the day. **Which sites this computer looked up**, as a count per site (for example "youtube.com ×40"). |
| **Older teen** (16–18) | Your time today and on past days. Which apps were open, in minutes, and when. **Not the sites.** |
| **Adult** (18+), keeping your own time | Your minutes, today and on past days. **Not your apps, not your sites, not your own rules, not what happened in your sessions** — the server refuses. |

Up to older teen, a parent also sees what happened on your computers: when it
was paused or resumed, when time ran out, time you asked for or were given,
when an unlock code was typed (right or wrong), when a blocked app was closed,
when someone tried to get around the rules, and when someone signed in to the
console from it. For an adult, or anyone keeping their own time, the server
leaves out everything that happened under your login; only what happened to
the computer itself (a pause, someone deleting its firewall) still shows.

Three details that matter:

- **Sites are counted per computer, not per person.** The computer's resolver
  doesn't know who asked. If you share a computer with someone younger, a
  parent looking at *their* day sees the sites that computer looked up —
  yours included.
- **Apps count while they're open**, not while they're in front. The computer
  can't tell which window you're looking at.
- **"Looked up" is not "visited".** It's the name your computer asked for:
  one visit can mean many lookups, and background apps look things up too.
  It's activity, not a history. What the computer looks up on its own is
  left out (the list is below), and so are OpenScreenTime's own lookups.

You see the same picture of your own day on your own page — except that on a
computer you share, the site list is left off, so you don't see someone
else's.

## What the computer sends

Everything the agent sends the server, and nothing else:

- **The computer**: its name, operating system, public IP address, agent
  version, which OS logins exist, who is logged in right now, and whether
  enforcement is working.
- **Your time**: seconds of screen time, per login, per day.
- **Apps**: which apps were open, in seconds per hour, per login — by name
  ("Firefox", "Text Editor", "Minecraft"): the apps the computer's app menu
  lists, and the well-known ones OpenScreenTime can block. Background parts
  of the desktop (what starts with every login, the shell, services) are not
  apps and aren't sent. This is sent for everyone, adults included; the
  server only shows it as the table above allows.
- **Sites**: how many times the computer looked up each site, per hour, for
  the whole computer — counted once, from when the agent is running (a
  restart doesn't count the day again). A browser also looks up some sites on
  its own (the new-tab page's shortcuts), so a count is activity, not proof
  of a visit. Not counted, because no person asked for them:
  - the computer's own update checks and mirrors — `deb.debian.org`,
    `security.debian.org`, `ftp.debian.org` and `ftp.<country>.debian.org`,
    Debian's updater and app catalogue (`ftp-master.debian.org`,
    `appstream.debian.org`), Ubuntu's `archive`, `security`, `ports`, `changelogs`, `motd` and `esm`
    `.ubuntu.com`, `packages.linuxmint.com`, `fedoraproject.org`;
  - app stores' own traffic — `flathub.org`, `snapcraft.io`,
    `snapcraftcontent.com`, GNOME Software's ratings (`odrs.gnome.org`);
  - "am I online?" checks — `nmcheck.gnome.org`, `networkcheck.kde.org`,
    `connectivity-check.ubuntu.com`, `detectportal.firefox.com`;
  - Firefox in the background — `services.mozilla.com`,
    `telemetry.mozilla.org`, `aus5.mozilla.org`, `ads.mozilla.org`,
    `cdn.mozilla.net`, `safebrowsing.googleapis.com`,
    `use-application-dns.net`;
  - the resolver library's own NAT64 check — `ipv4only.arpa`;
  - the clock — `pool.ntp.org`, `ntp.ubuntu.com`;
  - OpenScreenTime itself — its server, and the name it checks its own
    block with (`selftest.openscreentime.internal`).

  Each entry covers the names under it too. A site someone opens is still
  counted: `www.debian.org`, `www.mozilla.org` and `extensions.gnome.org`
  are not on the list. (The list lives in `client/src/attrib.rs`,
  `OS_LOOKUPS`.)
- **Events**: the things listed above (paused, time's up, codes, blocked apps,
  tampering, sign-ins).
- **Your requests** for more time, and the reason if you gave one.

The server keeps the app and site counts for 21 days and events for 90 days.
Daily totals are kept.

## What it can't see

Not built — not collected and hidden, not planned:

- **Your screen.** Nothing takes screenshots.
- **What you type.** To know whether someone is at the computer, the agent
  notices *that* a key was pressed or the mouse moved in the last five
  minutes — never which key, and nothing about it leaves the computer.
- **Your messages, files, camera or microphone.** No code touches them.
- **Pages.** Only the site's name is seen, never the address after it, the
  page, or what's on it.
- **Your phone or other devices.** Only computers with the agent installed.

One thing stays on this computer only: the local resolver's log, with every
name looked up and when. The agent reads it to make the counts above; it is
readable by root only, is cut back at 20 MB, and is never sent
(`/var/lib/openscreentime/dnsq.log`).

## What a parent can do from the console

- **Pause** your computer, and **resume** it.
- **Change your rules**: daily limit, when screens can be on, bedtime, what's
  blocked, safe search, earning time.
- **Give you time**, and answer your requests.
- **Check it's on** ("Is it answering?").
- **Send a sign-in code** to their *own* computer — never to yours.
- **Remove** the computer.

That's all of it. There is **no remote shell**: a parent can't open a
terminal, read your files or run commands on this computer through
OpenScreenTime.

## What you'll notice

- **Warnings at 15, 5 and 1 minute** before any stop — your limit, bedtime,
  the end of allowed hours, or a pause planned ahead.
- **At the stop**, the screen switches to the OpenScreenTime lock. Your apps
  are paused, not closed; unsaved work stays where it was. If the stop
  wasn't announced (say a rule just changed), you get a save-your-work
  countdown first: 30 seconds for Little, 60 for Kid, two minutes for teens.
  A parent's pause is immediate.
- **Getting more time**: choose **Ask for more time** (Kid and teens), or a
  parent types the unlock code at the lock (30 minutes), or gives time or
  resumes from the console.
- **The unlock code** also answers `sudo` on this computer. You can't use
  `sudo` without it; a parent can.
- **Offline**, the computer keeps today's rules and your time. If a parent
  turned on offline lockdown, a computer that can't reach the server for
  several days stops until it does — the unlock code still opens it.

## If you fight it

It can't make tampering impossible if you have root and the machine in your
hands, and it doesn't claim to. What it does:

- Changes to its DNS setting or firewall are put back within seconds.
- Power off, reboot and suspend are never blocked. They don't get round a
  stop either: your time today and a stop are kept on disk, so the computer
  comes back to the same day.
- If the agent stops, systemd restarts it, and a watchdog checks it's alive.
- If its firewall keeps being deleted, it stops every screen and tells a
  parent, in plain words: OpenScreenTime was changed.
- If you remove it, the computer goes quiet on the parent's console — which
  shows up as plainly as a switched-off laptop.

The details are in [`TAMPER.md`](TAMPER.md).
