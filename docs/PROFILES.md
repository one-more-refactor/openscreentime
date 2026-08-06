# Preset Profiles

Every new tenant is seeded with three `is_preset=true` profiles. They are fully editable (an
edit clones the policy in place — the preset row stays but its `policy` is mutated). Presets can
also be duplicated into `custom` profiles.

The `teen` and `default` presets are zero-trust (`default_deny`); **`kids` is not** — see below for why. The difference between them
is how large the allowlist is, how strict screen-time is, how tight the network lockdown is, and
how much gamification is on.

## Shared fields

A few `Policy` fields aren't preset-specific dials — they work the same way (or the same
constraint applies) across `kids`, `teen`, and `default`, so they're documented once here instead
of being repeated in every section below.

### `dns.upstream`

Must be a literal IP address, never a hostname. `normalize_policy` (`server/src/profiles.rs`)
rejects any value that doesn't parse as an `IpAddr` before it's ever stored. This isn't
cosmetic validation: the value is interpolated verbatim into the agent's nftables ruleset
(`ip daddr <upstream> ...`) on the device, so a hostname, typo, or injected nft syntax could
abort the whole ruleset load on the box instead of just failing a DNS lookup. All three presets
use `1.1.1.2` (Cloudflare's filtered/malware-blocking resolver).

### `lockdown` — network anti-bypass

`NetworkLockdown` (`policy/src/lib.rs`) is a set of firewall/DNS rules layered on top of the base
allowlist to stop a managed user from routing around the policy entirely, plus one escalation
knob for when the device goes dark:

| Field | Type | Enforces |
| --- | --- | --- |
| `force_dns` | bool | Blocks plaintext DNS (UDP/TCP 53) egress to anything but the agent's own resolver, so a browser or OS can't be pointed at `8.8.8.8` directly. |
| `block_doh` | bool | Drops the well-known public DNS-over-HTTPS resolver IPs (Cloudflare, Google, Quad9, …) so browsers can't tunnel DNS over HTTPS around the local resolver. |
| `block_dot` | bool | Blocks DNS-over-TLS (TCP 853). |
| `block_tor` | bool | Blocks Tor — known directory-authority/onion-router ports plus `.onion` resolution. |
| `block_vpn` | bool | Blocks common commercial-VPN ports: WireGuard `51820`, OpenVPN `1194`, IPsec/IKE `500`/`4500`. |
| `offline_lockdown_days` | u32 | Days the agent may run without reaching the command server before it escalates to a full parent-PIN lockdown. `0` = never escalate. A device silently cut off from the server is treated as a tamper signal — but because the parent PIN always unlocks locally, a server/VPS outage can never permanently brick the device. |

All five boolean flags default off and `offline_lockdown_days` defaults to `0`. When every field
is at that default, the entire `lockdown` object is omitted from the stored/serialized policy
(`NetworkLockdown::is_default` + `skip_serializing_if`) — a profile with no lockdown configured
has no `lockdown` key at all, which is why the `default` preset below doesn't show one.

### `parent_pin_hash` — parent PIN

The Argon2 hash of the household's parent/master PIN. It's never written directly into policy
JSON; it's derived from the API's `parent_pin` field on profile create/update requests
(`server/src/profiles.rs`):

- non-empty string (minimum 4 characters) → hashed with Argon2 server-side and stored as
  `parent_pin_hash`
- empty string `""` → clears the PIN (removes `parent_pin_hash`)
- field omitted entirely → preserves whatever hash was already stored

The agent verifies an entered PIN against this hash locally, so it keeps working with no server
connection. It's the master unlock on managed devices: a correct PIN grants a 30-minute unlock
grace. The stored value never contains the plaintext PIN. None of the three presets set a PIN out
of the box.

## `kids` — filtered, not walled off

Deliberately **not** zero-trust. The earlier version allowed five domains and denied everything
else, which broke Minecraft, Steam, school portals and `apt` — so in practice an adult either
widened the allowlist until it meant nothing or switched enforcement off. Filtering at the
resolver is stricter in the ways that matter and invisible the rest of the time.

- **DNS:** `allow_all` through a filtering upstream — `1.1.1.3` (Cloudflare for Families:
  malware **and** adult content), `safe_search: true`, plus an explicit blocklist for the things
  that slip past a category filter: web proxies, torrent indexes, gambling, stranger-chat.
- **Firewall:** `allow_all`, with inbound `22` open. Permissive so ordinary software works;
  the lockdown flags below still emit targeted drops, because chain policy and lockdown rules
  are independent.
- **Screen time:** 60 min/day; windows 07:00–20:00 weekdays, 09:00–20:00 weekends; bedtime
  20:00–07:00.
- **Lockdown:** the bypass paths stay shut — `force_dns`, `block_doh`, `block_dot`, `block_tor`
  all `true`. Two are deliberately off: **`block_vpn: false`**, because a parent-managed
  WireGuard profile is a supported feature and enabling both makes the agent apply a tunnel its
  own firewall then kills; and **`offline_lockdown_days: 0`**, because a device that cannot
  reach the server must not brick itself — screen time still applies from the cached policy.
- **Gamification:** earn-time ON (reading, chores tasks), lockout ON with `math` challenge,

```jsonc
{
  "version": 1,
  "dns": { "mode": "allow_all",
    "allowlist": ["*"],
    "blocklist": ["croxyproxy.com","proxysite.com","kproxy.com","hidester.com",
                  "4everproxy.com","whoer.net","hide.me","vpnbook.com",
                  "thepiratebay.org","1337x.to","torrentz2.eu","rarbg.to",
                  "pornhub.com","xvideos.com","xnxx.com","onlyfans.com",
                  "stake.com","bet365.com","roobet.com",
                  "omegle.com","chatroulette.com"],
    "safe_search": true, "upstream": "1.1.1.3" },
  "firewall": { "mode": "allow_all", "allow_outbound_ports": [], "allow_inbound_ports": [22] },
  "screen_time": { "enabled": true, "daily_limit_minutes": 60,
    "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"20:00"},
                  {"days":[0,6],"start":"09:00","end":"20:00"} ],
    "bedtime": { "start":"20:00","end":"07:00" } },
  "lockdown": { "force_dns": true, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": false, "offline_lockdown_days": 0 },
  "gamification": {
    "earn_time": { "enabled": true, "tasks": [
      {"id":"reading","label":"Read for 20 min","reward_minutes":15},
      {"id":"chores","label":"Finish chores","reward_minutes":15} ] },
    "lockout": { "enabled": true, "unlock_challenge": "math" },

}
```

## `teen` — trusted-but-guarded

- **DNS:** default-deny with a broader allowlist (general web categories via allowlisted major
  domains), `safe_search: true`. Still zero-trust, just roomier.
- **Firewall:** default-deny; outbound `53, 80, 443` (+ `123` NTP).
- **Screen time:** 180 min/day; windows to 21:00 weekdays, 22:00 weekends; bedtime
  22:30–06:30.
- **Lockdown:** DoH, DoT, and Tor blocked (`block_doh`/`block_dot`/`block_tor: true`); DNS isn't
  forced and VPN ports aren't blocked (`force_dns`/`block_vpn: false`) — older teens get more
  rope. `offline_lockdown_days: 0`, so there's no offline hard-lockdown escalation.
- **Gamification:** earn-time ON (lighter rewards), lockout ON with `wait` challenge (cooldown
  instead of math).

```jsonc
{
  "version": 1,
  "dns": { "mode": "default_deny",
    "allowlist": ["*.wikipedia.org","github.com","google.com","youtube.com","duolingo.com","*.edu"],
    "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
  "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443,123], "allow_inbound_ports": [] },
  "screen_time": { "enabled": true, "daily_limit_minutes": 180,
    "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"21:00"},
                  {"days":[0,6],"start":"08:00","end":"22:00"} ],
    "bedtime": { "start":"22:30","end":"06:30" } },
  "lockdown": { "force_dns": false, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": false, "offline_lockdown_days": 0 },
  "gamification": {
    "earn_time": { "enabled": true, "tasks": [
      {"id":"homework","label":"Finish homework","reward_minutes":20} ] },
    "lockout": { "enabled": true, "unlock_challenge": "wait" },

}
```

## `default` — baseline for any newly-enrolled user

Applied automatically to every `device_user` at enrollment until an admin assigns something
else. Zero-trust but minimally intrusive: it protects (default-deny DNS/firewall, safe-search)
without screen-time limits or gamification, so an unclassified account isn't accidentally locked
out — but also isn't wide open. It doesn't set `lockdown` at all (every flag would be at its
default, so the field is omitted entirely — see "Shared fields" above) and doesn't set a
`parent_pin_hash`.

```jsonc
{
  "version": 1,
  "dns": { "mode": "default_deny",
    "allowlist": ["*"], "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
  "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443,123], "allow_inbound_ports": [] },
  "screen_time": { "enabled": false, "daily_limit_minutes": 0, "schedule": [], "bedtime": null },
  "gamification": {
    "earn_time": { "enabled": false, "tasks": [] },
    "lockout": { "enabled": false, "unlock_challenge": "wait" },

}
```

> Note on `default`'s DNS `allowlist: ["*"]`: zero-trust posture is preserved structurally
> (mode stays `default_deny`, firewall still restricts ports, safe-search on), but the wildcard
> means an unclassified adult account isn't broken on day one. Tightening this is a one-click
> edit. `kids`/`teen` never use `"*"`.

## Seeding

When a tenant is created the server inserts these three rows verbatim (from a Rust
`presets.rs` module that mirrors this file). Keep `presets.rs` and this doc in sync.
