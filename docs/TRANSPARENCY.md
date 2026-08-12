# Transparency: what OpenScreenTime actually does on your machine

This document is for you — the person using the managed device, not the parent who set it
up. It exists because the whole point of OpenScreenTime is that it doesn't lie to you by omission.
Every claim below is backed by the actual agent code, not marketing copy. If something here
turns out to be wrong, that's a bug in the product, not an acceptable gap.

## What this is

OpenScreenTime is a program (`openscreentime`) that runs as root on this computer, filters network
traffic, tracks how long you're logged in, and enforces limits your parent sets. It reports
status back to a server your family controls. It is not hidden — it shows up in your system
tray (if you have the companion running), in `systemctl status`, and as a running process.
It does not pretend to be something else.

## What your parents can see

Everything the agent sends the server is one of these, and nothing else:

- **Device status**: online/offline, public IP, agent version, hostname.
- **The list of OS user accounts on this machine** (usernames, display names, UIDs) — so
  policy can be applied per person.
- **Screen time totals**: minutes used today, per OS user. Not per app, not per window —
  just "logged into an active local session, not idle."
- **Lock/unlock events**: when the device was locked or unlocked, and by what (admin
  command, screen-time expiry, PIN override, offline lockdown).
- **Policy changes**: when a new policy was applied and its version.
- **Tamper events**: see "what happens if you fight it" below — every detected tamper
  attempt, with a severity level.
- **Earn-time requests**: when you pick a task on the lockout screen to earn extra minutes
  (task name and minutes requested), and how your parent decided it.
Two things that used to be on this list are gone entirely, not merely hidden: streak/nudge
events (the app no longer nudges you at all) and LAN discovery scans (it no longer looks at
other devices on your network under any circumstances).

That's the complete list. There is no hidden channel — `client/src/client.rs` is the only
code that talks to the server, and every request body it builds is listed above.

## What they cannot see

Based on reading the entire agent codebase, these are **not implemented** — not hidden
somewhere else, not planned, not collected and just "not shown to you":

- **No screenshots.** Nothing captures the display.
- **No keylogging.** Nothing reads keystrokes outside of the lockout screen's own unlock
  input (which never leaves the device unless it's a parent-PIN check against a hash
  already cached locally).
- **No camera or microphone access.** The agent has no code that touches either.
- **No message or file contents.** The agent doesn't read your browser, chat apps, or
  documents.
- **No per-site browsing history sent to the server.** DNS filtering happens locally (a
  dnsmasq config the agent generates and reloads) — every query is either allowed and
  forwarded or answered with NXDOMAIN, on your machine, on the spot. The agent does not
  log which domains you requested and does not ship a query log anywhere.

- **No remote shell.** Earlier versions of OpenScreenTime let a parent open a root shell on this
  device (always disclosed to you while it was live). That capability has been **removed
  entirely** — there is no remote shell at all anymore, and no code path that could open
  one. The promise got stronger: it's no longer "a shell is never open without you
  knowing", it's "there is no shell". If shells were ever opened on this device in the
  past, those sessions are still visible as `ssh` entries in the event log — the record
  wasn't erased along with the feature.

## What they can do remotely

A parent, from the web dashboard, can push these commands to the agent:

- **Lock the whole device**, immediately, no grace period (this is a deliberate parent
  action, not an automatic enforcement — you get a "LOCKED BY AN ADMIN" screen and every
  user session is frozen right away).
- **Unlock it.**
- **Change policy**: screen time limits, allowed hours, bedtime, DNS allow/block lists,
  firewall rules, tamper level.
- **Grant or deny extra time**, including approving/denying an earn-time request you sent.
- **Scan the local network** to help onboard another device.

That's everything — and everything on that list goes through the same UI and the same
audited command queue. There is **no remote shell**: a parent cannot reach a terminal,
your files, or arbitrary commands on this device through OpenScreenTime. Older versions had a
(always-disclosed) remote shell; it has been removed outright, and any past sessions
remain visible as `ssh` entries in the event log.

## What you'll experience

- **10 minutes and 2 minutes before your time runs out**, you get a nudge ("good time to
  finish up" / "wrap up and save now"), once each per day. If bedtime is configured, you
  also get a wind-down warning up to 15 minutes before it starts.
- **When your time actually runs out** (daily limit, outside allowed hours, or bedtime), a
  full-screen lockout appears immediately — but nothing freezes yet. You get **60 seconds**
  to save your work before the freeze lands. This countdown is shown on screen.
- **The freeze itself pauses your processes** (a cgroup freeze), it does not kill your
  session or destroy unsaved work. Screen-time enforcement is explicitly designed to never
  escalate to terminating your session — only an explicit admin lock (or the offline
  lockdown below) can do that, and only as a last resort if freezing isn't available.
- **Getting back in**: solve a short math challenge for a 5-minute breather, wait out a
  cooldown, request extra time for a task (goes to your parent for approval, and you're
  told clearly if it's denied instead of being left hanging), or have a parent enter their
  PIN — which grants 30 minutes and always works as a master override, on any lockout,
  whether or not that's the configured challenge. A parent physically at the machine can
  always get you unlocked.
- **Admin locks are immediate**, with no 60-second grace — that's a deliberate parent
  action, not an automatic timeout, so the save-your-work courtesy doesn't apply.
- **Offline hard lockdown**: if your parent has turned this on, and the device genuinely
  can't reach the server for a set number of days, the device locks itself down the same
  way an admin lock would. A parent PIN still unlocks it. This exists so that pulling the
  network cable indefinitely isn't a way to escape limits forever — but it only engages
  after days of silence, not a brief outage.

## What happens if you fight it

OpenScreenTime is honest that it cannot make tampering physically impossible if you have root and
physical access to the machine. What it does instead:

- **Local tampering is detected and repaired automatically.** Editing `/etc/resolv.conf`,
  flushing the firewall table, disconnecting the network, or jumping the system clock are
  all checked every ~10 seconds. If any of them drifted, the agent puts them back and files
  a `tamper` event your parent sees, with a severity level.
- **Shutdown, reboot, and suspend are blocked for you** (not root) by default, via a polkit
  rule — you can't power off your way around a lockout. An opt-in stricter mode also blocks
  you from stopping the agent's systemd service and disables switching virtual terminals.
- **If the agent process dies, systemd restarts it immediately**, and a separate watchdog
  checks its heartbeat file on a timer and restarts it again if that goes stale — killing
  the process once doesn't get you anywhere.
- **If you have root, you can ultimately remove the agent.** No software can prevent that,
  and OpenScreenTime doesn't claim otherwise — claiming unbypassable enforcement would be a lie.
  But it is never a silent bypass: the server marks a device offline within minutes of
  losing contact, and that shows up on your parent's dashboard as plainly as if you'd
  smashed the laptop. Going dark is visible, not invisible.

## Why it's built this way

The point of OpenScreenTime isn't to spy on you without your knowledge — it's to enforce agreed
limits (time, content, bedtime) in a way that's checkable. Every mechanism above either
reports something structural (time used, lock state, a policy change) or an
attempt to bypass enforcement. Nothing reports the content of what you do, say, or look at.
If you don't trust that, you don't have to take it on faith: the agent's source is what this
document was written from, line by line, and the tray, the events log, and this file are
supposed to always agree.
