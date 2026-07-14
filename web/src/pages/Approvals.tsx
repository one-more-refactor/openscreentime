import { useState } from "react";
import { approveEarnRequest, denyEarnRequest, listEarnRequests } from "../api";
import type { EarnRequest } from "../types";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { PageHeader } from "../layout/Shell";
import { Button, ErrorPanel, Panel, Stat, StatusLed } from "../components";
import { Empty, Loading } from "./Devices";
import { pad2, relTime } from "../lib/format";

// Earn-time approval queue (contract §4): kids request minutes for completed
// tasks; approving credits the ledger and pushes `credit_time` to the device.
export function Approvals() {
  const pending = useAsync<EarnRequest[]>(() => listEarnRequests("pending"), []);
  const { toast } = useToast();
  const [decidedFilter, setDecidedFilter] = useState<"approved" | "denied">("approved");
  const decided = useAsync<EarnRequest[]>(
    () => listEarnRequests(decidedFilter),
    [decidedFilter],
  );
  const [busyId, setBusyId] = useState<string | null>(null);

  const list = pending.data ?? [];

  async function decide(r: EarnRequest, approve: boolean) {
    setBusyId(r.id);
    try {
      await (approve ? approveEarnRequest(r.id) : denyEarnRequest(r.id));
      pending.setData((prev) => (prev ?? []).filter((x) => x.id !== r.id));
      decided.reload();
      toast(
        approve
          ? `Approved — ${r.user_display_name ?? r.os_username} gets +${r.minutes} min.`
          : "Request denied.",
        approve ? "ok" : "warn",
      );
    } catch (e) {
      toast(errMsg(e, "Couldn't record the decision — try again."));
    } finally {
      setBusyId(null);
    }
  }

  return (
    <>
      <PageHeader
        title="APPROVALS"
        stat={
          <Stat
            value={pad2(list.length)}
            caption="PENDING"
            size="lg"
            accent={false}
          />
        }
      />

      <div className="flex flex-col gap-6">
        <Panel title="EARN-TIME REQUESTS" refCode="AP-01">
          {pending.loading ? (
            <Loading />
          ) : pending.error ? (
            <ErrorPanel
              title="Couldn't load pending requests"
              detail={pending.error}
              onRetry={pending.reload}
            />
          ) : list.length === 0 ? (
            <Empty label="NO PENDING REQUESTS — ALL CLEAR." />
          ) : (
            <ul className="flex flex-col">
              {list.map((r) => (
                <li
                  key={r.id}
                  className="flex items-center gap-4 py-3 border-b last:border-b-0 flex-wrap"
                  style={{ borderColor: "var(--line)" }}
                >
                  <StatusLed tone="warn" pulse />
                  <div className="flex-1 min-w-0">
                    <p className="text-xs text-fg">
                      <span className="dot">{r.user_display_name ?? r.os_username}</span>
                      {" asks "}
                      <span className="dot" style={{ color: "var(--ok)" }}>
                        +{r.minutes} min
                      </span>
                      {" — "}
                      {r.task_label}
                    </p>
                    <p className="text-[0.625rem] mt-0.5" style={{ color: "var(--fg-faint)" }}>
                      {r.device_name ?? r.device_id} · requested {relTime(r.created_at)} ago
                    </p>
                  </div>
                  <div className="flex items-center gap-2 flex-none">
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={busyId === r.id}
                      onClick={() => void decide(r, true)}
                    >
                      APPROVE +{r.minutes}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busyId === r.id}
                      onClick={() => void decide(r, false)}
                    >
                      DENY
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          title="RECENTLY DECIDED"
          refCode="AP-02"
          aside={
            <div className="flex items-center gap-1">
              {(["approved", "denied"] as const).map((s) => (
                <button
                  key={s}
                  onClick={() => setDecidedFilter(s)}
                  className="focusable label border rounded px-2 py-1 transition-colors"
                  style={{
                    borderColor: decidedFilter === s ? "var(--fg)" : "var(--line-2)",
                    color: decidedFilter === s ? "var(--fg)" : "var(--fg-dim)",
                  }}
                  aria-pressed={decidedFilter === s}
                >
                  {s.toUpperCase()}
                </button>
              ))}
            </div>
          }
        >
          {decided.loading ? (
            <Loading />
          ) : decided.error ? (
            <ErrorPanel
              title="Couldn't load decided requests"
              detail={decided.error}
              onRetry={decided.reload}
            />
          ) : (decided.data ?? []).length === 0 ? (
            <Empty label={`NO ${decidedFilter.toUpperCase()} REQUESTS YET`} />
          ) : (
            <ul className="flex flex-col">
              {(decided.data ?? []).map((r) => (
                <li
                  key={r.id}
                  className="flex items-center gap-4 py-2.5 border-b last:border-b-0 flex-wrap"
                  style={{ borderColor: "var(--line)" }}
                >
                  <StatusLed tone={r.status === "approved" ? "ok" : "idle"} />
                  <div className="flex-1 min-w-0">
                    <p className="text-xs" style={{ color: "var(--fg-dim)" }}>
                      <span className="dot text-fg">{r.user_display_name ?? r.os_username}</span>
                      {" · "}+{r.minutes} min · {r.task_label}
                    </p>
                  </div>
                  <span className="label flex-none" style={{ color: "var(--fg-faint)" }}>
                    {r.status.toUpperCase()} {r.decided_at ? relTime(r.decided_at) : ""}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </>
  );
}
