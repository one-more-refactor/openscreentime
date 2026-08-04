import { useCallback, useEffect, useState } from "react";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { listDevices, listEarnRequests } from "../api";
import type { Device } from "../types";
import { DotMatrix } from "../components/DotMatrix";
import { goneDarkDays } from "../lib/format";

const POLL_MS = 20_000;

// Ambient fleet data for the header glyph strip + nav counts. Polled quietly;
// failures leave the previous snapshot in place (pages surface their own errors).
function useFleet() {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [pending, setPending] = useState<number | null>(null);

  const tick = useCallback(async () => {
    try {
      setDevices(await listDevices());
    } catch {
      /* ambient — keep last snapshot */
    }
    try {
      setPending((await listEarnRequests("pending")).length);
    } catch {
      /* ambient */
    }
  }, []);

  useEffect(() => {
    void tick();
    const id = window.setInterval(() => void tick(), POLL_MS);
    const onVis = () => document.visibilityState === "visible" && void tick();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      window.clearInterval(id);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [tick]);

  return { devices, pending };
}

// The signature: one live LED cell per device — the fleet at a glance.
function FleetStrip({ devices }: { devices: Device[] | null }) {
  if (!devices || devices.length === 0) return null;
  const cellClass = (d: Device) =>
    d.status === "online"
      ? "fleet-cell fleet-cell-ok"
      : d.status === "locked"
        ? "fleet-cell fleet-cell-locked"
        : d.status === "pending" || goneDarkDays(d.status, d.last_seen) !== null
          ? "fleet-cell fleet-cell-warn"
          : "fleet-cell";
  return (
    <div
      className="flex items-center gap-2.5"
      title="One cell per device — green online, red locked, amber pending or gone dark, dim offline"
    >
      <span className="label hidden sm:inline">FLEET</span>
      <span className="fleet-cells" role="img" aria-label={`Fleet: ${devices.length} devices`}>
        {devices.map((d, i) => (
          <i
            key={d.id}
            className={cellClass(d)}
            style={d.status === "online" ? { animationDelay: `${(i % 4) * 0.8}s` } : undefined}
          />
        ))}
      </span>
    </div>
  );
}

interface NavEntry {
  to: string;
  label: string;
  count?: number | null;
  warn?: boolean;
}

function NavList({ entries, onNavigate }: { entries: NavEntry[]; onNavigate?: () => void }) {
  return (
    <nav className="flex flex-col py-3 flex-1" aria-label="Main">
      {entries.map((n) => (
        <NavLink
          key={n.to}
          to={n.to}
          onClick={onNavigate}
          className="focusable flex items-center justify-between gap-2.5 px-5 py-2.5 font-mono uppercase tracking-label text-[0.6875rem] transition-colors"
          style={({ isActive }) => ({
            color: isActive ? "var(--fg)" : "var(--fg-dim)",
            borderRight: isActive ? "2px solid var(--accent)" : "2px solid transparent",
          })}
        >
          <span>{n.label}</span>
          {n.count != null && n.count > 0 && (
            <span
              className="tabular-nums text-[0.625rem]"
              style={{ color: n.warn ? "var(--warn)" : "var(--fg-faint)" }}
            >
              {String(n.count).padStart(2, "0")}
            </span>
          )}
        </NavLink>
      ))}
    </nav>
  );
}

export function Shell() {
  const { me, mock, logout } = useSession();
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();
  const { devices } = useFleet();
  const [menuOpen, setMenuOpen] = useState(false);

  async function handleLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  // Two destinations. Everything else — devices, tokens, events, profiles —
  // is machinery that now lives inside a child's page or runs unattended.
  // A console about your children should not have a fleet menu.
  const entries: NavEntry[] = [
    { to: "/", label: "FAMILY" },
    { to: "/settings", label: "SETTINGS" },
  ];

  const railFooter = (
    <div className="p-4 border-t" style={{ borderColor: "var(--line)" }}>
      <button
        onClick={toggle}
        className="focusable w-full text-left label mb-3 hover:text-fg"
        style={{ color: "var(--fg-faint)" }}
      >
        THEME · {theme.toUpperCase()}
      </button>
      {mock && (
        <div
          className="mb-3 flex items-center gap-2 border rounded px-2 py-1.5"
          style={{ borderColor: "var(--warn)" }}
        >
          <span className="led led-glow-warn" style={{ background: "var(--warn)" }} />
          <span className="label" style={{ color: "var(--warn)" }}>
            MOCK DATA
          </span>
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <p className="dot text-[0.6875rem] text-fg truncate">
            {me?.admin.display_name ?? "ADMIN"}
          </p>
          <p className="text-[0.625rem] truncate" style={{ color: "var(--fg-faint)" }}>
            {me?.admin.email ?? ""}
          </p>
        </div>
        <button
          onClick={handleLogout}
          className="focusable label hover:text-accent flex-none"
          style={{ color: "var(--fg-dim)" }}
        >
          LOGOUT
        </button>
      </div>
    </div>
  );

  return (
    <div className="min-h-screen flex flex-col">
      {/* Top bar: wordmark · fleet glyph strip · admin */}
      <header
        className="sticky top-0 z-40 flex items-center gap-4 px-4 lg:px-6 h-14 border-b"
        style={{ borderColor: "var(--line)", background: "var(--bg)" }}
      >
        <button
          className="focusable lg:hidden flex flex-col justify-center gap-1 w-8 h-8 items-center"
          onClick={() => setMenuOpen(true)}
          aria-label="Open navigation"
          aria-expanded={menuOpen}
        >
          <span className="block w-4 h-px" style={{ background: "var(--fg)" }} />
          <span className="block w-4 h-px" style={{ background: "var(--fg)" }} />
          <span className="block w-4 h-px" style={{ background: "var(--fg)" }} />
        </button>
        <NavLink to="/" className="focusable flex items-center" aria-label="Sentinel home">
          <DotMatrix text="SENTINEL" dot={3} color="var(--fg)" />
        </NavLink>
        <span className="flex-1" />
        <FleetStrip devices={devices} />
        <span className="label hidden md:inline" style={{ color: "var(--fg-dim)" }}>
          {me ? `${me.admin.display_name} · admin` : ""}
        </span>
      </header>

      <div className="flex flex-1 min-h-0">
        {/* Desktop rail */}
        <aside
          className="hidden lg:flex w-56 flex-none flex-col border-r sticky top-14 h-[calc(100vh-3.5rem)]"
          style={{ borderColor: "var(--line)", background: "var(--surface)" }}
        >
          <NavList entries={entries} />
          {railFooter}
        </aside>

        {/* Mobile slide-over */}
        {menuOpen && (
          <div className="fixed inset-0 z-50 lg:hidden" role="dialog" aria-modal="true" aria-label="Navigation">
            <div
              className="absolute inset-0 dotgrid"
              style={{ background: "rgba(0,0,0,0.72)" }}
              onClick={() => setMenuOpen(false)}
              aria-hidden
            />
            <div
              className="relative w-64 h-full flex flex-col border-r"
              style={{ borderColor: "var(--line-2)", background: "var(--surface)" }}
            >
              <div
                className="flex items-center justify-between px-5 h-14 border-b flex-none"
                style={{ borderColor: "var(--line)" }}
              >
                <DotMatrix text="SENTINEL" dot={2.5} color="var(--fg)" />
                <button
                  onClick={() => setMenuOpen(false)}
                  className="focusable text-fg-faint hover:text-fg text-sm"
                  aria-label="Close navigation"
                >
                  ✕
                </button>
              </div>
              <NavList entries={entries} onNavigate={() => setMenuOpen(false)} />
              {railFooter}
            </div>
          </div>
        )}

        {/* Main */}
        <main className="flex-1 min-w-0">
          <div className="max-w-[1200px] mx-auto px-4 sm:px-6 lg:px-8 py-6 lg:py-8">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
}

interface HeaderProps {
  title: string;
  stat?: React.ReactNode;
  actions?: React.ReactNode;
}

export function PageHeader({ title, stat, actions }: HeaderProps) {
  return (
    <header className="flex items-end justify-between gap-4 mb-8 flex-wrap">
      <div className="flex items-end gap-6 flex-wrap">
        <h1 className="dot text-xl text-fg">{title}</h1>
        {stat}
      </div>
      {actions && <div className="flex items-center gap-2 flex-wrap">{actions}</div>}
    </header>
  );
}
