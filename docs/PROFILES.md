# Preset Profiles

Every new tenant is seeded with three `is_preset=true` profiles. They are fully editable (an
edit clones the policy in place — the preset row stays but its `policy` is mutated). Presets can
also be duplicated into `custom` profiles.

All presets are **zero-trust**: DNS and firewall are `default_deny`. The difference between them
is how large the allowlist is, how strict screen-time is, and how much gamification is on.

## `kids` — locked down, playful

- **DNS:** default-deny, small curated allowlist (education, kids' content, the school domain),
  `safe_search: true`, filtered upstream `1.1.1.2`.
- **Firewall:** default-deny; outbound `53, 80, 443` only.
- **Screen time:** 60 min/day; windows 15:00–19:00 weekdays, 09:00–19:00 weekends; bedtime
  20:00–07:00 hard block.
- **App limits:** games 30 min/day.
- **Gamification:** earn-time ON (reading, chores tasks), lockout ON with `math` challenge,
  streaks ON (bedtime + breaks nudges). Full-screen interruptions enabled.

```jsonc
{
  "version": 1,
  "dns": { "mode": "default_deny",
    "allowlist": ["wikipedia.org","khanacademy.org","pbskids.org","scratch.mit.edu","duolingo.com"],
    "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
  "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443], "allow_inbound_ports": [] },
  "screen_time": { "enabled": true, "daily_limit_minutes": 60,
    "schedule": [ {"days":[1,2,3,4,5],"start":"15:00","end":"19:00"},
                  {"days":[0,6],"start":"09:00","end":"19:00"} ],
    "bedtime": { "start":"20:00","end":"07:00" } },
  "app_limits": [ { "match":"steam","daily_limit_minutes":30 } ],
  "gamification": {
    "earn_time": { "enabled": true, "tasks": [
      {"id":"reading","label":"Read for 20 min","reward_minutes":15},
      {"id":"chores","label":"Finish chores","reward_minutes":15} ] },
    "lockout": { "enabled": true, "unlock_challenge": "math" },
    "streaks": { "enabled": true, "nudges": ["bedtime","breaks"] } }
}
```

## `teen` — trusted-but-guarded

- **DNS:** default-deny with a broader allowlist (general web categories via allowlisted major
  domains), `safe_search: true`. Still zero-trust, just roomier.
- **Firewall:** default-deny; outbound `53, 80, 443` (+ `123` NTP).
- **Screen time:** 180 min/day; windows to 21:00 weekdays, 22:00 weekends; bedtime
  22:30–06:30.
- **App limits:** games 90 min/day.
- **Gamification:** earn-time ON (lighter rewards), lockout ON with `wait` challenge (cooldown
  instead of math), streaks ON (breaks only).

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
  "app_limits": [ { "match":"steam","daily_limit_minutes":90 } ],
  "gamification": {
    "earn_time": { "enabled": true, "tasks": [
      {"id":"homework","label":"Finish homework","reward_minutes":20} ] },
    "lockout": { "enabled": true, "unlock_challenge": "wait" },
    "streaks": { "enabled": true, "nudges": ["breaks"] } }
}
```

## `default` — baseline for any newly-enrolled user

Applied automatically to every `device_user` at enrollment until an admin assigns something
else. Zero-trust but minimally intrusive: it protects (default-deny DNS/firewall, safe-search)
without screen-time limits or gamification, so an unclassified account isn't accidentally locked
out — but also isn't wide open.

```jsonc
{
  "version": 1,
  "dns": { "mode": "default_deny",
    "allowlist": ["*"], "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
  "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443,123], "allow_inbound_ports": [] },
  "screen_time": { "enabled": false, "daily_limit_minutes": 0, "schedule": [], "bedtime": null },
  "app_limits": [],
  "gamification": {
    "earn_time": { "enabled": false, "tasks": [] },
    "lockout": { "enabled": false, "unlock_challenge": "wait" },
    "streaks": { "enabled": false, "nudges": [] } }
}
```

> Note on `default`'s DNS `allowlist: ["*"]`: zero-trust posture is preserved structurally
> (mode stays `default_deny`, firewall still restricts ports, safe-search on), but the wildcard
> means an unclassified adult account isn't broken on day one. Tightening this is a one-click
> edit. `kids`/`teen` never use `"*"`.

## Seeding

When a tenant is created the server inserts these three rows verbatim (from a Rust
`presets.rs` module that mirrors this file). Keep `presets.rs` and this doc in sync.
