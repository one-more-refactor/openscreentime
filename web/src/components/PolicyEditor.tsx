import type {
  EarnTask,
  NetworkLockdown,
  Policy,
  StreakNudge,
  TimeWindow,
  UnlockChallenge,
} from "../types";
import { Toggle } from "./Toggle";
import { TextInput, Select } from "./TextInput";
import { TagInput } from "./TagInput";
import { TimeRange } from "./TimeRange";
import { Button } from "./Button";
import { WEEKDAY_LABELS } from "../lib/format";
import { isDomain, isIp } from "../lib/validate";

const domainError = (v: string) =>
  isDomain(v) ? null : `"${v}" is not a valid domain — use example.com or *.example.com.`;

interface Props {
  value: Policy;
  onChange: (next: Policy) => void;
  readOnly?: boolean;
  /**
   * Draft parent-PIN edit, kept OUTSIDE the policy object (it's sent to the
   * API as a separate `parent_pin` field, never round-tripped through
   * `policy.parent_pin_hash`). `undefined` = untouched (save preserves the
   * existing PIN); `""` = explicit clear; non-empty = a new PIN to set.
   */
  parentPin?: string;
  onParentPinChange?: (pin: string | undefined) => void;
}

type LockdownFlag = "force_dns" | "block_doh" | "block_dot" | "block_tor" | "block_vpn";

const LOCKDOWN_TOGGLES: {
  key: LockdownFlag;
  label: string;
  hint: string;
}[] = [
  {
    key: "force_dns",
    label: "FORCE DNS",
    hint: "Block plaintext DNS bypass — routes all lookups through the filtered resolver above.",
  },
  {
    key: "block_doh",
    label: "BLOCK DoH",
    hint: "Block DNS-over-HTTPS — stops browsers tunneling lookups past the filter.",
  },
  {
    key: "block_dot",
    label: "BLOCK DoT",
    hint: "Block DNS-over-TLS — same bypass, a different encrypted channel.",
  },
  {
    key: "block_tor",
    label: "BLOCK TOR",
    hint: "Block Tor — stops the Tor anonymity network and .onion sites.",
  },
  {
    key: "block_vpn",
    label: "BLOCK VPN",
    hint: "Block common VPN ports — stops WireGuard, OpenVPN, and IPsec tunnels used to route around filtering.",
  },
];

const NO_LOCKDOWN: NetworkLockdown = {
  force_dns: false,
  block_doh: false,
  block_dot: false,
  block_tor: false,
  block_vpn: false,
};

// Structured form over the full Policy jsonb (docs/API.md). Zero-trust framing.
export function PolicyEditor({
  value,
  onChange,
  readOnly,
  parentPin,
  onParentPinChange,
}: Props) {
  // ---- immutable patch helpers ----
  const patch = (p: Partial<Policy>) => onChange({ ...value, ...p });
  const set = <K extends keyof Policy>(key: K, v: Policy[K]) =>
    patch({ [key]: v } as Partial<Policy>);

  const lockdown = value.lockdown ?? NO_LOCKDOWN;
  const setLockdown = (v: NetworkLockdown) => set("lockdown", v);
  const pinIsSet = !!value.parent_pin_hash;
  const clearingPin = parentPin === "";

  const ports = (arr: number[]) =>
    arr.map(String);
  const parsePorts = (strs: string[]) =>
    Array.from(
      new Set(
        strs
          .map((s) => parseInt(s.trim(), 10))
          .filter((n) => Number.isInteger(n) && n >= 0 && n <= 65535),
      ),
    ).sort((a, b) => a - b);

  return (
    <div className="flex flex-col gap-4">
      {/* Zero-trust banner */}
      <div
        className="flex items-center gap-3 border rounded px-3 py-2.5"
        style={{ borderColor: "var(--accent-dim)", background: "var(--surface-2)" }}
      >
        <span className="led led-glow-crit" style={{ background: "var(--accent)" }} />
        <span className="dot text-[0.6875rem]" style={{ color: "var(--accent)" }}>
          BLOCKED BY DEFAULT
        </span>
        <span className="label">ADD EXCEPTIONS BELOW · ZERO-TRUST</span>
      </div>

      {/* DNS */}
      <Section title="DNS FILTERING" mode="DEFAULT DENY">
        <TagInput
          label="ALLOWLIST — REACHABLE DOMAINS"
          values={value.dns.allowlist}
          onChange={(v) => set("dns", { ...value.dns, allowlist: v })}
          placeholder="school.edu, *.wikipedia.org"
          tone="ok"
          validate={domainError}
        />
        <TagInput
          label="BLOCKLIST — EXPLICIT BLOCKS"
          values={value.dns.blocklist}
          onChange={(v) => set("dns", { ...value.dns, blocklist: v })}
          placeholder="add domain"
          tone="crit"
          validate={domainError}
        />
        <div className="grid sm:grid-cols-2 gap-4 items-start">
          <div className="pt-1">
            <Toggle
              label="SAFE SEARCH"
              hint="force safe-search on major engines"
              checked={value.dns.safe_search}
              onChange={(v) => set("dns", { ...value.dns, safe_search: v })}
              disabled={readOnly}
            />
          </div>
          <TextInput
            label="FILTERED UPSTREAM RESOLVER"
            value={value.dns.upstream}
            onChange={(e) => set("dns", { ...value.dns, upstream: e.target.value })}
            placeholder="1.1.1.2"
            disabled={readOnly}
            aria-invalid={!isIp(value.dns.upstream)}
            hint={
              isIp(value.dns.upstream)
                ? undefined
                : "Must be an IP address, e.g. 1.1.1.2."
            }
          />
        </div>
      </Section>

      {/* Firewall */}
      <Section title="FIREWALL" mode="DEFAULT DENY">
        <div className="grid sm:grid-cols-2 gap-4">
          <TagInput
            label="ALLOW OUTBOUND PORTS"
            values={ports(value.firewall.allow_outbound_ports)}
            onChange={(v) =>
              set("firewall", { ...value.firewall, allow_outbound_ports: parsePorts(v) })
            }
            placeholder="53, 80, 443"
            tone="ok"
          />
          <TagInput
            label="ALLOW INBOUND PORTS"
            values={ports(value.firewall.allow_inbound_ports)}
            onChange={(v) =>
              set("firewall", { ...value.firewall, allow_inbound_ports: parsePorts(v) })
            }
            placeholder="(none)"
            tone="ok"
          />
        </div>
      </Section>

      {/* Network lockdown */}
      <Section
        title="NETWORK LOCKDOWN"
        aside={
          <span className="label" style={{ color: "var(--fg-faint)" }}>
            ANTI-BYPASS
          </span>
        }
      >
        <p className="text-[0.625rem] leading-relaxed" style={{ color: "var(--fg-faint)" }}>
          Closes off the common ways a device can dodge the filters above. Turn on what applies —
          each toggle adds its own firewall rule.
        </p>
        <div className="grid sm:grid-cols-2 gap-4">
          {LOCKDOWN_TOGGLES.map(({ key, label, hint }) => (
            <Toggle
              key={key}
              label={label}
              hint={hint}
              checked={lockdown[key]}
              onChange={(v) => setLockdown({ ...lockdown, [key]: v })}
              disabled={readOnly}
            />
          ))}
        </div>
        <TextInput
          label="OFFLINE HARD-LOCKDOWN AFTER (DAYS)"
          type="number"
          min={0}
          className="max-w-[16rem]"
          value={lockdown.offline_lockdown_days ?? 0}
          disabled={readOnly}
          onChange={(e) => {
            const days = Math.max(0, parseInt(e.target.value || "0", 10));
            // Omit the field when 0 so the serialized policy stays byte-
            // identical with the crate's skip-default serde output.
            const { offline_lockdown_days: _drop, ...flags } = lockdown;
            setLockdown(days > 0 ? { ...flags, offline_lockdown_days: days } : flags);
          }}
          hint="0 = never. If the device can't reach this server for N days it locks itself; the parent PIN always unlocks."
        />
      </Section>

      {/* Screen time */}
      <Section
        title="SCREEN TIME"
        aside={
          <Toggle
            checked={value.screen_time.enabled}
            onChange={(v) => set("screen_time", { ...value.screen_time, enabled: v })}
            disabled={readOnly}
          />
        }
      >
        <div className={value.screen_time.enabled ? "" : "opacity-40 pointer-events-none"}>
          <div className="flex flex-col gap-4">
            <TextInput
              label="DAILY LIMIT (MINUTES)"
              type="number"
              min={0}
              className="max-w-[12rem]"
              value={value.screen_time.daily_limit_minutes}
              onChange={(e) =>
                set("screen_time", {
                  ...value.screen_time,
                  daily_limit_minutes: Math.max(0, parseInt(e.target.value || "0", 10)),
                })
              }
              disabled={readOnly}
            />

            {/* Schedule rows */}
            <div className="flex flex-col gap-2">
              <span className="label">ALLOWED WINDOWS</span>
              {value.screen_time.schedule.map((w, i) => (
                <ScheduleRow
                  key={i}
                  window={w}
                  readOnly={readOnly}
                  onChange={(nw) => {
                    const schedule = value.screen_time.schedule.slice();
                    schedule[i] = nw;
                    set("screen_time", { ...value.screen_time, schedule });
                  }}
                  onRemove={() =>
                    set("screen_time", {
                      ...value.screen_time,
                      schedule: value.screen_time.schedule.filter((_, j) => j !== i),
                    })
                  }
                />
              ))}
              {!readOnly && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="self-start"
                  onClick={() =>
                    set("screen_time", {
                      ...value.screen_time,
                      schedule: [
                        ...value.screen_time.schedule,
                        { days: [1, 2, 3, 4, 5], start: "15:00", end: "20:00" },
                      ],
                    })
                  }
                >
                  + ADD WINDOW
                </Button>
              )}
            </div>

            {/* Bedtime */}
            <div className="flex flex-col gap-2">
              <Toggle
                label="BEDTIME — HARD BLOCK"
                checked={value.screen_time.bedtime !== null}
                onChange={(v) =>
                  set("screen_time", {
                    ...value.screen_time,
                    bedtime: v ? { start: "21:00", end: "07:00" } : null,
                  })
                }
                disabled={readOnly}
              />
              {value.screen_time.bedtime && (
                <TimeRange
                  start={value.screen_time.bedtime.start}
                  end={value.screen_time.bedtime.end}
                  disabled={readOnly}
                  onChange={(start, end) =>
                    set("screen_time", { ...value.screen_time, bedtime: { start, end } })
                  }
                />
              )}
            </div>
          </div>
        </div>
      </Section>

      {/* App limits intentionally absent: the agent does not enforce them
          (contract §9). The field stays in the Policy type for forward compat. */}

      {/* Gamification */}
      <Section title="GAMIFICATION">
        <div className="flex flex-col gap-5">
          {/* earn-time */}
          <div className="flex flex-col gap-3">
            <Toggle
              label="EARN-TIME TASKS"
              hint="kids earn screen-time by completing tasks"
              checked={value.gamification.earn_time.enabled}
              onChange={(v) =>
                set("gamification", {
                  ...value.gamification,
                  earn_time: { ...value.gamification.earn_time, enabled: v },
                })
              }
              disabled={readOnly}
            />
            <div
              className={
                value.gamification.earn_time.enabled
                  ? "flex flex-col gap-2"
                  : "flex flex-col gap-2 opacity-40 pointer-events-none"
              }
            >
              {value.gamification.earn_time.tasks.map((t, i) => (
                <EarnTaskRow
                  key={i}
                  task={t}
                  readOnly={readOnly}
                  onChange={(nt) => {
                    const tasks = value.gamification.earn_time.tasks.slice();
                    tasks[i] = nt;
                    set("gamification", {
                      ...value.gamification,
                      earn_time: { ...value.gamification.earn_time, tasks },
                    });
                  }}
                  onRemove={() =>
                    set("gamification", {
                      ...value.gamification,
                      earn_time: {
                        ...value.gamification.earn_time,
                        tasks: value.gamification.earn_time.tasks.filter((_, j) => j !== i),
                      },
                    })
                  }
                />
              ))}
              {!readOnly && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="self-start"
                  onClick={() =>
                    set("gamification", {
                      ...value.gamification,
                      earn_time: {
                        ...value.gamification.earn_time,
                        tasks: [
                          ...value.gamification.earn_time.tasks,
                          {
                            id: `task-${value.gamification.earn_time.tasks.length + 1}`,
                            label: "",
                            reward_minutes: 15,
                          },
                        ],
                      },
                    })
                  }
                >
                  + ADD TASK
                </Button>
              )}
            </div>
          </div>

          {/* lockout */}
          <div className="grid sm:grid-cols-2 gap-4 items-start">
            <Toggle
              label="LOCKOUT CHALLENGE"
              hint="require a challenge to keep going"
              checked={value.gamification.lockout.enabled}
              onChange={(v) =>
                set("gamification", {
                  ...value.gamification,
                  lockout: { ...value.gamification.lockout, enabled: v },
                })
              }
              disabled={readOnly}
            />
            <Select
              label="UNLOCK CHALLENGE"
              value={value.gamification.lockout.unlock_challenge}
              disabled={readOnly || !value.gamification.lockout.enabled}
              onChange={(e) =>
                set("gamification", {
                  ...value.gamification,
                  lockout: {
                    ...value.gamification.lockout,
                    unlock_challenge: e.target.value as UnlockChallenge,
                  },
                })
              }
            >
              <option value="math">MATH</option>
              <option value="wait">WAIT</option>
              <option value="parent_pin">PARENT PIN</option>
            </Select>
          </div>

          {/* streaks */}
          <div className="flex flex-col gap-3">
            <Toggle
              label="STREAK NUDGES"
              hint="gentle reminders for healthy habits"
              checked={value.gamification.streaks.enabled}
              onChange={(v) =>
                set("gamification", {
                  ...value.gamification,
                  streaks: { ...value.gamification.streaks, enabled: v },
                })
              }
              disabled={readOnly}
            />
            <div
              className={
                value.gamification.streaks.enabled
                  ? "flex gap-2 flex-wrap"
                  : "flex gap-2 flex-wrap opacity-40 pointer-events-none"
              }
            >
              {(["bedtime", "breaks"] as StreakNudge[]).map((n) => {
                const active = value.gamification.streaks.nudges.includes(n);
                return (
                  <button
                    key={n}
                    type="button"
                    disabled={readOnly}
                    onClick={() =>
                      set("gamification", {
                        ...value.gamification,
                        streaks: {
                          ...value.gamification.streaks,
                          nudges: active
                            ? value.gamification.streaks.nudges.filter((x) => x !== n)
                            : [...value.gamification.streaks.nudges, n],
                        },
                      })
                    }
                    className="focusable border rounded px-3 py-1.5 text-[0.625rem] font-mono uppercase tracking-label transition-colors"
                    style={{
                      borderColor: active ? "var(--fg)" : "var(--line-2)",
                      background: active ? "var(--fg)" : "transparent",
                      color: active ? "var(--bg)" : "var(--fg-dim)",
                    }}
                  >
                    {n}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </Section>

      {/* Parent PIN */}
      <Section
        title="PARENT PIN"
        aside={
          <span
            className="label"
            style={{ color: pinIsSet ? "var(--fg)" : "var(--fg-faint)" }}
          >
            {pinIsSet ? "PIN IS SET" : "NO PIN SET"}
          </span>
        }
      >
        <p className="text-[0.625rem] leading-relaxed" style={{ color: "var(--fg-faint)" }}>
          Used on the device to override a lockout or unlock enforcement when it can't reach the
          server. Enter a new PIN to set or replace it — the current PIN is never shown here.
        </p>
        <div className="flex flex-wrap items-end gap-3">
          <TextInput
            label={pinIsSet ? "NEW PIN" : "SET PIN"}
            type="password"
            autoComplete="new-password"
            className="max-w-[12rem]"
            placeholder={clearingPin ? "PIN WILL BE CLEARED" : "••••"}
            value={clearingPin ? "" : parentPin ?? ""}
            disabled={readOnly || clearingPin}
            onChange={(e) => onParentPinChange?.(e.target.value || undefined)}
            hint={
              parentPin !== undefined && !clearingPin && parentPin.length < 4
                ? "Must be at least 4 characters."
                : undefined
            }
          />
          {!readOnly && (pinIsSet || parentPin !== undefined) && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => onParentPinChange?.(clearingPin ? undefined : "")}
            >
              {clearingPin ? "UNDO CLEAR" : "CLEAR PIN"}
            </Button>
          )}
        </div>
      </Section>
    </div>
  );
}

// ---- sub-parts -------------------------------------------------------------

function Section({
  title,
  mode,
  aside,
  children,
}: {
  title: string;
  mode?: string;
  aside?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section
      className="border rounded"
      style={{ borderColor: "var(--line)", background: "var(--surface)" }}
    >
      <header
        className="flex items-center justify-between gap-3 px-4 h-11 border-b"
        style={{ borderColor: "var(--line)" }}
      >
        <div className="flex items-center gap-2.5">
          <h3 className="dot text-[0.6875rem] text-fg">{title}</h3>
          {mode && (
            <span
              className="label border rounded px-1.5 py-0.5"
              style={{ color: "var(--accent)", borderColor: "var(--accent-dim)" }}
            >
              {mode}
            </span>
          )}
        </div>
        {aside}
      </header>
      <div className="p-4 flex flex-col gap-4">{children}</div>
    </section>
  );
}

function WeekdayPicker({
  days,
  onChange,
  disabled,
}: {
  days: number[];
  onChange: (d: number[]) => void;
  disabled?: boolean;
}) {
  return (
    <div className="inline-flex gap-1">
      {WEEKDAY_LABELS.map((lbl, idx) => {
        const on = days.includes(idx);
        return (
          <button
            key={idx}
            type="button"
            disabled={disabled}
            onClick={() =>
              onChange(on ? days.filter((d) => d !== idx) : [...days, idx].sort())
            }
            className="focusable w-6 h-6 rounded-[3px] border text-[0.625rem] font-mono transition-colors"
            style={{
              borderColor: on ? "var(--fg)" : "var(--line-2)",
              background: on ? "var(--fg)" : "transparent",
              color: on ? "var(--bg)" : "var(--fg-faint)",
            }}
            aria-pressed={on}
          >
            {lbl}
          </button>
        );
      })}
    </div>
  );
}

function ScheduleRow({
  window,
  onChange,
  onRemove,
  readOnly,
}: {
  window: TimeWindow;
  onChange: (w: TimeWindow) => void;
  onRemove: () => void;
  readOnly?: boolean;
}) {
  return (
    <div
      className="flex flex-wrap items-center gap-3 border rounded px-3 py-2"
      style={{ borderColor: "var(--line)", background: "var(--surface-2)" }}
    >
      <WeekdayPicker
        days={window.days}
        disabled={readOnly}
        onChange={(days) => onChange({ ...window, days })}
      />
      <TimeRange
        start={window.start}
        end={window.end}
        disabled={readOnly}
        onChange={(start, end) => onChange({ ...window, start, end })}
      />
      {!readOnly && (
        <button
          type="button"
          onClick={onRemove}
          className="ml-auto text-fg-faint hover:text-accent focusable text-xs"
          aria-label="remove window"
        >
          ✕
        </button>
      )}
    </div>
  );
}

function EarnTaskRow({
  task,
  onChange,
  onRemove,
  readOnly,
}: {
  task: EarnTask;
  onChange: (t: EarnTask) => void;
  onRemove: () => void;
  readOnly?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-end gap-3">
      <TextInput
        label="TASK"
        className="flex-1 min-w-[12rem]"
        value={task.label}
        placeholder="Read for 20 min"
        disabled={readOnly}
        onChange={(e) => onChange({ ...task, label: e.target.value })}
      />
      <TextInput
        label="REWARD MIN"
        type="number"
        min={0}
        className="w-28"
        value={task.reward_minutes}
        disabled={readOnly}
        onChange={(e) =>
          onChange({ ...task, reward_minutes: Math.max(0, parseInt(e.target.value || "0", 10)) })
        }
      />
      {!readOnly && (
        <button
          type="button"
          onClick={onRemove}
          className="text-fg-faint hover:text-accent focusable text-xs pb-2.5"
          aria-label="remove task"
        >
          ✕
        </button>
      )}
    </div>
  );
}
