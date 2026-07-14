import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { closeSsh, openSsh, sshWsUrl } from "../api";
import { Modal } from "./Modal";
import { Button } from "./Button";
import { StatusLed } from "./StatusLed";

type ConnState = "connecting" | "live" | "closed" | "failed";

interface Target {
  id: string;
  name: string;
}

interface Props {
  /** device to open a shell on; null hides the terminal */
  target: Target | null;
  onClose: () => void;
}

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

// Real remote terminal (contract §3): POST /api/devices/:id/ssh opens a
// session, then GET /api/ssh/:session_id/ws bridges raw bytes both ways.
// Binary frames = terminal bytes; text frames = control JSON.
export function SshTerminal({ target, onClose }: Props) {
  return (
    <Modal
      open={!!target}
      onClose={onClose}
      title={target ? `SSH · ${target.name.toUpperCase()}` : "SSH"}
      size="full"
      closeOnEscape={false}
    >
      {target && <TerminalBody key={target.id} target={target} onClose={onClose} />}
    </Modal>
  );
}

function TerminalBody({ target, onClose }: { target: Target; onClose: () => void }) {
  const mountRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<ConnState>("connecting");
  const [detail, setDetail] = useState<string | null>(null);

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;

    let cancelled = false;
    let ws: WebSocket | null = null;
    let sessionId: string | null = null;
    let closedByServer = false;

    const term = new Terminal({
      cursorBlink: true,
      fontFamily: '"Space Mono", ui-monospace, monospace',
      fontSize: 13,
      theme: {
        background: cssVar("--bg"),
        foreground: cssVar("--fg"),
        cursor: cssVar("--fg"),
        selectionBackground: cssVar("--line-2"),
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(mount);
    fit.fit();

    const encoder = new TextEncoder();
    const sendResize = () => {
      if (ws?.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      }
    };
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* container gone mid-teardown */
      }
    });
    ro.observe(mount);
    term.onResize(sendResize);
    term.onData((d) => {
      if (ws?.readyState === WebSocket.OPEN) ws.send(encoder.encode(d));
    });

    (async () => {
      try {
        const res = await openSsh(target.id);
        sessionId = res.session.id;
        if (cancelled) {
          void closeSsh(sessionId).catch(() => undefined);
          return;
        }
        ws = new WebSocket(sshWsUrl(sessionId));
        ws.binaryType = "arraybuffer";
        ws.onopen = () => {
          if (cancelled) return;
          setState("live");
          sendResize();
          term.focus();
        };
        ws.onmessage = (e) => {
          if (typeof e.data === "string") {
            try {
              const msg = JSON.parse(e.data) as { type?: string; exit_code?: number | null };
              if (msg.type === "closed") {
                closedByServer = true;
                setState("closed");
                setDetail(
                  msg.exit_code == null
                    ? "Session closed by the device."
                    : `Session closed — exit code ${msg.exit_code}.`,
                );
              }
            } catch {
              /* ignore malformed control frames */
            }
            return;
          }
          term.write(new Uint8Array(e.data as ArrayBuffer));
        };
        ws.onerror = () => {
          if (cancelled || closedByServer) return;
          setState("failed");
          setDetail("Connection error — the agent may be offline. Close and retry.");
        };
        ws.onclose = () => {
          if (cancelled || closedByServer) return;
          setState((s) => (s === "failed" ? s : "closed"));
          setDetail((d) => d ?? "Connection closed.");
        };
      } catch (err) {
        if (cancelled) return;
        setState("failed");
        setDetail(
          err instanceof Error
            ? `Couldn't open the session: ${err.message}`
            : "Couldn't open the session.",
        );
      }
    })();

    return () => {
      cancelled = true;
      ro.disconnect();
      ws?.close();
      if (sessionId) void closeSsh(sessionId).catch(() => undefined);
      term.dispose();
    };
  }, [target.id]);

  const led =
    state === "live" ? (
      <StatusLed tone="ok" label="LIVE" pulse />
    ) : state === "connecting" ? (
      <StatusLed tone="warn" label="CONNECTING…" pulse />
    ) : state === "failed" ? (
      <StatusLed tone="crit" label="FAILED" />
    ) : (
      <StatusLed tone="idle" label="CLOSED" />
    );

  return (
    <div className="flex flex-col gap-3 flex-1 min-h-0">
      <div className="flex items-center justify-between gap-3 flex-none">
        {led}
        <div className="flex items-center gap-3">
          <span className="ref">TERM-01 · BYTES BRIDGED VIA AGENT WS</span>
          <Button size="sm" variant="danger" onClick={onClose}>
            CLOSE SESSION
          </Button>
        </div>
      </div>
      <div
        className="ssh-term relative flex-1 min-h-0 border rounded overflow-hidden p-2"
        style={{ borderColor: "var(--line-2)", background: "var(--bg)" }}
      >
        <div ref={mountRef} className="absolute inset-2" />
      </div>
      {detail && (
        <p className="text-[0.6875rem] flex-none" style={{ color: "var(--fg-dim)" }}>
          {detail}
        </p>
      )}
    </div>
  );
}
