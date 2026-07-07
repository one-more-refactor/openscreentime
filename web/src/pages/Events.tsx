import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { listEvents, listDevices } from "../api";
import type { Device, Event, EventType, Severity } from "../types";
import { useAsync } from "../lib/useAsync";
import { PageHeader } from "../layout/Shell";
import { EventFeed, Panel, Select, Stat } from "../components";
import { Loading } from "./Devices";
import { pad2 } from "../lib/format";

const EVENT_TYPES: EventType[] = [
  "heartbeat",
  "tamper",
  "lock",
  "unlock",
  "policy_applied",
  "screen_time_exceeded",
  "screen_time_earned",
  "streak",
  "enrolled",
  "discovery_result",
];

const SEVERITIES: Severity[] = ["info", "warn", "critical"];

export function Events() {
  const [params, setParams] = useSearchParams();
  const deviceId = params.get("device_id") ?? "";
  const type = (params.get("type") ?? "") as EventType | "";
  const severity = (params.get("severity") ?? "") as Severity | "";

  const devices = useAsync<Device[]>(listDevices, []);
  const events = useAsync<Event[]>(
    () =>
      listEvents({
        device_id: deviceId || undefined,
        type: type || undefined,
        severity: severity || undefined,
        limit: 200,
      }),
    [deviceId, type, severity],
  );

  const [q, setQ] = useState("");

  function setFilter(key: string, value: string) {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    setParams(next, { replace: true });
  }

  const list = events.data ?? [];
  const filtered = useMemo(
    () =>
      q
        ? list.filter((e) =>
            JSON.stringify(e.payload).toLowerCase().includes(q.toLowerCase()),
          )
        : list,
    [list, q],
  );

  const counts = useMemo(() => {
    const c = { info: 0, warn: 0, critical: 0 };
    for (const e of list) c[e.severity]++;
    return c;
  }, [list]);

  return (
    <>
      <PageHeader
        title="EVENTS"
        stat={
          <div className="flex items-end gap-6">
            <Stat value={pad2(list.length)} caption="LOGGED" size="lg" />
            {counts.critical > 0 && (
              <Stat value={pad2(counts.critical)} caption="CRITICAL" size="md" accent />
            )}
            {counts.warn > 0 && <Stat value={pad2(counts.warn)} caption="WARN" size="md" />}
          </div>
        }
      />

      <Panel
        title="AUDIT LOG"
        aside={
          <div className="flex items-center gap-2 flex-wrap">
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="SEARCH PAYLOAD…"
              className="focusable bg-transparent border rounded px-2 py-1 text-[0.625rem] font-mono uppercase tracking-label text-fg placeholder:text-fg-faint"
              style={{ borderColor: "var(--line-2)" }}
            />
          </div>
        }
      >
        <div className="flex flex-wrap gap-4 mb-4">
          <Select
            label="DEVICE"
            className="w-48"
            value={deviceId}
            onChange={(e) => setFilter("device_id", e.target.value)}
          >
            <option value="">ALL DEVICES</option>
            {(devices.data ?? []).map((d) => (
              <option key={d.id} value={d.id}>
                {d.name.toUpperCase()}
              </option>
            ))}
          </Select>
          <Select
            label="TYPE"
            className="w-48"
            value={type}
            onChange={(e) => setFilter("type", e.target.value)}
          >
            <option value="">ALL TYPES</option>
            {EVENT_TYPES.map((t) => (
              <option key={t} value={t}>
                {t.replace(/_/g, " ").toUpperCase()}
              </option>
            ))}
          </Select>
          <Select
            label="SEVERITY"
            className="w-40"
            value={severity}
            onChange={(e) => setFilter("severity", e.target.value)}
          >
            <option value="">ALL</option>
            {SEVERITIES.map((s) => (
              <option key={s} value={s}>
                {s.toUpperCase()}
              </option>
            ))}
          </Select>
        </div>

        {events.loading ? <Loading /> : <EventFeed events={filtered} emptyLabel="NO MATCHING EVENTS" />}
      </Panel>
    </>
  );
}
