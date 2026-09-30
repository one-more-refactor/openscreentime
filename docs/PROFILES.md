# Rules and the five starting rules

A person's rules are one JSON document, the `Policy` in
[`policy/src/lib.rs`](../policy/src/lib.rs). The server stores it
(`profiles.policy`), the console edits it, the agent enforces it, and all
three read it with the same rules function
([`policy/src/rules.rs`](../policy/src/rules.rs)). Each person has their own
copy; a new person's copy starts from their age bracket's preset
([`server/src/presets.rs`](../server/src/presets.rs)).

## The five starting rules

Every household gets one preset per bracket (`is_preset = true`, kinds
`little`, `kid`, `younger_teen`, `older_teen`, `adult`). All of them are
**allow by default**: DNS `allow_all` through a filtering upstream, firewall
`allow_all` with inbound 22 open, `block_vpn` off, `offline_lockdown_days` 0.

| | Little 0–6 | Kid 6–12 | Younger teen 12–16 | Older teen 16–18 | Adult 18+ |
|---|---|---|---|---|---|
| Daily limit | 45 min | 60 min | 150 min | none | none |
| Screens on | 08:00–19:00 every day | 07:00–20:00 Mon–Fri, 09:00–20:00 Sat–Sun | 07:00–21:00 Mon–Fri, 09:00–22:00 Sat–Sun | any time | any time |
| Bedtime | 19:00–07:00 | 20:00–07:00 | 22:00–06:30 | — | — |
| Upstream resolver | `1.1.1.3` (malware + adult) | `1.1.1.3` | `1.1.1.3` | `1.1.1.2` (malware) | `1.1.1.2` |
| Safe search | on | on | on | off | off |
| Blocked categories | adult, gambling, dating, proxies | adult, gambling, dating, proxies | adult, gambling, proxies | adult, gambling, proxies | — |
| Anti-bypass (force DNS, DoH, DoT, Tor) | on | on | on | on | off |
| Earning time | off | Read for 20 min · Finish chores (15 min each) | Finish homework (20 min) | off | off |
| Can ask for more time | no | yes | yes | yes | no |

Changing someone's age later doesn't rewrite their rules. Older households
may still have the pre-0.4 `kids` / `teen` / `default` presets; those rows
stay valid and editable, and nothing new is made from them.

**Unsorted logins.** A login nobody has sorted yet gets the **Kid** rules on
a child's computer, and the **Adult** rules — which enforce nothing — on a
parent's own computer or when the server re-links a login at startup, so a
guess never locks a parent out ([`server/src/members.rs`](../server/src/members.rs)
`link_os_user`). A parent sorts it under **Computers → Who's who**.

## What the rules mean

`rules::evaluate` answers three questions for the agent and the console:
*allowed now?*, *why not?*, *when is the next stop?*. The cases, and the
shared test vectors both sides check, are in
[`policy/tests/schedule-vectors.json`](../policy/tests/schedule-vectors.json).

- **A limit of 0 is no limit.** Use a pause or allowed hours for "no screens".
- **A day with no window is any time.** Windows use `days` 0 = Sunday … 6 =
  Saturday.
- **A window ending `00:00` runs to midnight**; one ending before it starts
  runs past midnight.
- **An empty window or a whole-day bedtime is refused on save**
  (`rules::validate_screen_time`) **and ignored on the computer** — never a
  24/7 lockout.
- **One budget per person**: a daily limit covers all of a person's computers.
- **One override per person** (a grant, the unlock code at the lock, a
  Resume) beats the limit, bedtime and hours until it ends. A pause beats an
  override.

## The document, field by field

| Field | What it is | Edited in the console |
|---|---|---|
| `version` | Always 1. | — |
| `screen_time.enabled`, `.daily_limit_minutes` | The daily limit (0 = none). | Rules → Daily limit |
| `screen_time.schedule` | Allowed windows `{days, start, end}`. | Rules → When screens can be on |
| `screen_time.bedtime` | `{start, end}`, or null. | Rules → Bedtime |
| `blocks.categories`, `.apps`, `.custom_domains` | One-click blocks from the catalog ([`policy/src/catalog.rs`](../policy/src/catalog.rs)) and sites by name. Blocked domains are sinkholed; a blocked app's processes are closed. Unknown ids expand to nothing. | Rules → Blocked |
| `dns.safe_search` | Rewrites the big search and video sites to their safe modes. | Rules → Safe search |
| `dns.upstream` | The resolver everything is forwarded to. **Must be a literal IP** — it goes verbatim into the nftables ruleset, so the server refuses anything else. | — |
| `dns.mode`, `firewall.mode` | `allow_all`. `default_deny` still parses and the agent still honours it, but the server rewrites any profile using it to `allow_all` at startup. | — |
| `dns.allowlist`, `dns.blocklist` | Retired from the console; still parsed. `["*"]` means "forward everything". | — |
| `firewall.allow_inbound_ports`, `allow_outbound_ports` | Presets keep inbound 22 open so a mistake has a way in. | — |
| `lockdown` | Anti-bypass: `force_dns`, `block_doh`, `block_dot`, `block_tor`, `block_vpn`, and `offline_lockdown_days` (0 = off; the agent floors any other value at 3 days). Omitted when all off. | — |
| `gamification.earn_time` | `{enabled, tasks: [{id, label, reward_minutes}]}`. | Rules → Earning time |
| `gamification.lockout` | **Dead.** `unlock_challenge` (`math` / `wait` / `parent_pin`) is read by nothing; the lock always takes the unlock code. | — |
| `focus` | A self-managed person's own site blocks: `{sites, hours}` — blocked inside the focus hours, all day with none. Omitted when empty. | Me → My focus hours, Sites I block for myself |
| `parent_pin_hash` | Argon2 hash of a legacy **backup code**. Still accepted at the lock, `ost unlock` and `sudo`; nothing in the console sets it. Omitted when unset. | — |

`lockdown`, `blocks`, `focus` and `parent_pin_hash` are left out of the JSON
when empty, so a preset round-trips byte-for-byte through the `Policy` type;
`presets::tests` fails if one doesn't.

## An adult's own rules

An adult (self-managed) sets their own daily limit, focus hours and sites on
their own page, through `GET` / `PUT /api/me/rules` ([`API.md`](API.md)).
The hub can't read or change a member's own rules. The keys (unlock and
recovery codes) are in Settings.

## Network rules on a shared computer

DNS and the firewall are one per computer, so the agent merges the network
rules of everyone on that computer, field by field, strictest wins: blocks
are unioned, safe search and the anti-bypass flags are on if anyone needs
them, the most-filtering resolver is used (`runner.rs`
`effective_network_policy`). Screen time is per person.
