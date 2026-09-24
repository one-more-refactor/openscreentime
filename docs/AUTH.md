# OpenScreenTime — sign-in

How people get into the console, and the one place inside that asks again.
Short on purpose; the code is the authority (`server/src/auth.rs`,
`login_code.rs`, `confirm.rs`, `voucher.rs`, `members.rs`).

## First run

A server with no accounts shows **Create your household**: your name, then a
passkey. That's it. On an internet-facing install it also needs the one-time
setup code (`OST_BOOTSTRAP_TOKEN`), which `deploy/setup.sh` writes to `.env`
and prints as a link — `https://<host>/#setup=<code>` — so opening the link is
enough (the fragment never reaches a server log). Without the link the page
asks for the code. The moment the first account exists, first run is closed
(`403 registration_closed`). With no `OST_BOOTSTRAP_TOKEN` (a local checkout)
first run is open, and the server says so in its log.

## Two doors

1. **Your name → a code on your own computer.** Type your name; the server
   sends a 6-digit code to *your* computer — only to the OS logins linked to
   you, and for a parent only on a computer declared as theirs. It shows up in
   the OpenScreenTime window (the agent opens it), as one desktop
   notification, and in `ost code`. Type it into the browser. The browser that
   asked holds a PKCE verifier, so the code is useless anywhere else. A code
   lasts 5 minutes and allows 5 tries; a computer is asked at most 5 times in
   10 minutes. An unknown or ambiguous name, or nobody's computer online, gets
   a decoy that behaves the same in every answer, so the page doesn't tell a
   stranger who exists. Each successful code sign-in raises an
   `account_login` alert.
2. **Sign in with a passkey.** One tap, no name first: passkeys are created as
   discoverable credentials and the passkey says whose it is.

Plus **Sign in with SSO**, only when OIDC is configured (`OST_OIDC_*`).
And `ost login` on an enrolled computer opens the console already signed in
(a one-time voucher in the URL fragment, for the person behind that OS login;
parents only from their own computer).

Every sign-in gives a session cookie (`ost_session`, HttpOnly, SameSite=Lax,
Secure unless `OST_INSECURE_COOKIES=1`), stored sha256-hashed: 30 days, 7 for a
voucher.

## Whose login is whose

Each OS login on a computer is its own person. A computer set up for someone
("Mia's computer", or "my computer" from Devices) links **one** login to them,
settled when it enrolls: the one the installer picked when `ost enroll` asked
"which login is Mia's?", else the only login, else — on a parent's own
computer — the login the install ran from, else the login with Mia's name.
Nothing is ever guessed onto a parent. Every other login becomes a person of
its own with a child's rules until a parent says otherwise under
**Devices → Who's who** (which asks you to confirm it's you: it decides who
that login signs in as).

## Confirm it's you

Inside, everything just works — pausing, granting time, changing rules. Only
the keys ask again: a computer's unlock code and recovery codes, your passkeys,
pairing tokens, the Telegram pairing, re-pointing an OS login, a fresh enroll
token, VPN configs. The server answers those with `428 step_up_required` until
the session has a live 15-minute confirm window. A fresh sign-in opens it;
after that, either door opens it again: **your passkey**, or **a code on your
own computer** (bound to the session instead of a verifier). An account with
neither signs in again. It's a layer over `/api` (`confirm::require_confirm`),
so a new sensitive route is guarded the day it matches `sensitive()`. The same
layer lets a paused account read but not change anything.

There is no authenticator app and no Telegram tap: Telegram is alerts (and
one-tap answers to time requests), nothing more.

## Unlock codes

Not account sign-in — the other direction: how a parent proves themselves *at
a child's computer*. Each computer has a TOTP secret the agent verifies
offline; the parent reads the live 6-digit **unlock code** in the console and
can make eight one-time **recovery codes** (`server/src/unlock_code.rs`).

## Lost every passkey

Run, as root inside the server container,
`podman exec openscreentime-server /app/openscreentime-server recover <name>`.
It prints a one-time sign-in link for that existing account (single use, 30
minutes); opening it signs you in with "confirm it's you" already done, so the
next step is **Settings → Add a passkey**. Anyone who can run it already owns
the server, so it grants nothing new — and it raises an `account_login` alert.
