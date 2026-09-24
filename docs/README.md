# OpenScreenTime documentation

Every doc is written against the code on this branch. Where a doc and the code
disagree, the code is right and the doc is a bug.

## Which doc owns what

One owner per question. No other doc overrides it.

| Question | Owner |
|---|---|
| What the product is and how it behaves; the words it uses | [`PRODUCT.md`](PRODUCT.md) |
| How it looks | [`../brand/board.html`](../brand/board.html) (open it in a browser), then [`DESIGN.md`](DESIGN.md) for the web console and [`DESIGN-CLIENT.md`](DESIGN-CLIENT.md) for the computer itself |
| The HTTP and WebSocket API | [`API.md`](API.md) |
| The database | [`DATA_MODEL.md`](DATA_MODEL.md) |
| The agent on a managed computer | [`AGENT.md`](AGENT.md) |
| Running the server | [`DEPLOY.md`](DEPLOY.md), then [`OPERATIONS.md`](OPERATIONS.md) |
| Signing in | [`AUTH.md`](AUTH.md) |
| What a parent can and can't see | [`TRANSPARENCY.md`](TRANSPARENCY.md) |

## Running a family

| Doc | What it answers |
|---|---|
| [`PARENT-GUIDE.md`](PARENT-GUIDE.md) | The console, page by page: the family, a person's day and rules, computers, settings, keeping time for yourself. |
| [`TRANSPARENCY.md`](TRANSPARENCY.md) | For the person whose computer it is: what a parent sees at each age, what the computer can't see, what a parent can do. |
| [`PROFILES.md`](PROFILES.md) | The rules document, field by field, and the five starting rules by age. |

## Running the server

| Doc | What it answers |
|---|---|
| [`DEPLOY.md`](DEPLOY.md) | The one-command install, the reverse proxy, settings. |
| [`OPERATIONS.md`](OPERATIONS.md) | What runs by itself, backups and restores, alerts, recovering access. |
| [`AGENT.md`](AGENT.md) | The agent: CLI, files it writes, systemd units, the lock, self-update, troubleshooting. |
| [`TAMPER.md`](TAMPER.md) | What is enforced, what is detected, and what we don't claim. |

## Building on it

| Doc | What it answers |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | The technical map. Start here. |
| [`DEVELOPMENT.md`](DEVELOPMENT.md) | The dev loop, tests, the container and VM harnesses. |
| [`TRACKING.md`](TRACKING.md) | How time is measured and how a stop is decided. |
| [`API.md`](API.md), [`DATA_MODEL.md`](DATA_MODEL.md) | The wire contract and the schema. |
| [`BRAND-CLIENT.md`](BRAND-CLIENT.md) | The words on the computer itself. |
| [`../AGENTS.md`](../AGENTS.md) | The short guide for AI agents working in this repo. |

## History

Kept for the record; none of them describes the product today:
[`OPENSCREENTIME.md`](OPENSCREENTIME.md) (the 0.4 brief),
[`CONTRACT-PROD.md`](CONTRACT-PROD.md), [`CONTRACT-0.4.md`](CONTRACT-0.4.md),
[`CONTRACT-0.5.md`](CONTRACT-0.5.md), [`CONTRACT-0.6.md`](CONTRACT-0.6.md)
(build contracts) and [`DESIGN-AUDIT.md`](DESIGN-AUDIT.md) (the audit before
the rebuild).
