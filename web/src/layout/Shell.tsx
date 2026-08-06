import { useEffect, useState } from "react";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { useFamily, minutesLeft, type FamilyChild } from "../lib/family";
import { useStepUp } from "../lib/stepup";
import { Wordmark } from "../components/Wordmark";

/** While a step-up grant is live, say so — and how long it has left. */
function ArmedChip() {
  const { armed, armedUntil } = useStepUp();
  const [, tick] = useState(0);
  useEffect(() => {
    if (!armed) return;
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [armed]);
  if (!armed || !armedUntil) return null;
  const s = Math.max(0, Math.round((new Date(armedUntil).getTime() - Date.now()) / 1000));
  return (
    <span className="armed-chip" title="Changes need no code until this runs out">
      UNLOCKED · {Math.floor(s / 60)}:{String(s % 60).padStart(2, "0")}
    </span>
  );
}

// The rail is three things, in Nothing's three layers: where you can go
// (mono nav), how everyone's day is going (the family pulse — glanceable,
// no clicking required), and who you are (footer, smallest). No LED strips,
// no fleet, no machinery.

interface NavEntry {
  to: string;
  label: string;
}

function NavList({ entries, onNavigate }: { entries: NavEntry[]; onNavigate?: () => void }) {
  return (
    <nav className="flex flex-col py-3" aria-label="Main">
      {entries.map((n) => (
        <NavLink
          key={n.to}
          to={n.to}
          onClick={onNavigate}
          className="focusable rail-nav"
          style={({ isActive }) => ({
            // Active = ink, not red. Red is an interrupt, not a location.
            color: isActive ? "var(--fg-display)" : "var(--fg-dim)",
            borderLeft: isActive
              ? "2px solid var(--fg-display)"
              : "2px solid transparent",
          })}
        >
          {n.label}
        </NavLink>
      ))}
    </nav>
  );
}

function hueFor(key: string): number {
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

/** One child in the rail: hue dot, name, minutes left — the pulse at a glance. */
function RailChild({ child, onNavigate }: { child: FamilyChild; onNavigate?: () => void }) {
  const left = minutesLeft(child);
  const spent = left === 0;
  return (
    <NavLink
      to={`/child/${encodeURIComponent(child.key)}`}
      onClick={onNavigate}
      className="focusable rail-child"
      style={({ isActive }) => ({
        color: isActive ? "var(--fg-display)" : "var(--fg)",
        borderLeft: isActive
          ? "2px solid var(--fg-display)"
          : "2px solid transparent",
      })}
    >
      <span
        className="rail-child-dot"
        style={{ background: `hsl(${hueFor(child.key)} 45% 70%)` }}
        aria-hidden="true"
      />
      <span className="rail-child-name">{child.name}</span>
      {child.pendingRequests > 0 && (
        <span className="rail-child-asks" aria-label={`${child.pendingRequests} requests waiting`}>
          {child.pendingRequests}
        </span>
      )}
      <span className="rail-child-left" data-spent={spent}>
        {left === null ? "—" : spent ? "0m" : left >= 60 ? `${Math.floor(left / 60)}h${String(left % 60).padStart(2, "0")}` : `${left}m`}
      </span>
    </NavLink>
  );
}

function Rail({ onNavigate }: { onNavigate?: () => void }) {
  const { children } = useFamily();
  const entries: NavEntry[] = [
    { to: "/", label: "FAMILY" },
    { to: "/devices", label: "DEVICES" },
    { to: "/settings", label: "SETTINGS" },
  ];
  return (
    <div className="flex-1 min-h-0 overflow-y-auto">
      <NavList entries={entries} onNavigate={onNavigate} />
      {children.length > 0 && (
        <div className="rail-fam">
          <p className="rail-fam-head">TODAY</p>
          {children.map((c) => (
            <RailChild key={c.key} child={c} onNavigate={onNavigate} />
          ))}
        </div>
      )}
    </div>
  );
}

function RailFooter() {
  const { me, mock, logout } = useSession();
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();

  async function handleLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  return (
    <div className="rail-foot">
      {mock && <p className="rail-mock">MOCK DATA</p>}
      <p className="rail-who">{me?.account?.display_name ?? me?.admin.display_name ?? "Parent"}</p>
      <div className="rail-foot-row">
        <button onClick={toggle} className="focusable rail-foot-btn">
          THEME · {theme.toUpperCase()}
        </button>
        <button onClick={handleLogout} className="focusable rail-foot-btn">
          LOGOUT
        </button>
      </div>
    </div>
  );
}

export function Shell() {
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <div className="min-h-screen flex flex-col">
      {/* Top bar: wordmark only. Identity lives in the rail, once. */}
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
        <NavLink to="/" className="focusable flex items-center" aria-label="OpenScreenTime home">
          <Wordmark size={1.25} />
        </NavLink>
        <span className="flex-1" />
        <ArmedChip />
      </header>

      <div className="flex flex-1 min-h-0">
        {/* Desktop rail */}
        <aside
          className="hidden lg:flex w-60 flex-none flex-col border-r sticky top-14 h-[calc(100vh-3.5rem)]"
          style={{ borderColor: "var(--line)", background: "var(--bg)" }}
        >
          <Rail />
          <RailFooter />
        </aside>

        {/* Mobile slide-over */}
        {menuOpen && (
          <div className="fixed inset-0 z-50 lg:hidden" role="dialog" aria-modal="true" aria-label="Navigation">
            <div
              className="absolute inset-0"
              style={{ background: "rgba(0,0,0,0.72)" }}
              onClick={() => setMenuOpen(false)}
              aria-hidden
            />
            <div
              className="relative w-64 h-full flex flex-col border-r"
              style={{ borderColor: "var(--line-2)", background: "var(--bg)" }}
            >
              <div
                className="flex items-center justify-between px-5 h-14 border-b flex-none"
                style={{ borderColor: "var(--line)" }}
              >
                <Wordmark size={1.125} />
                <button
                  onClick={() => setMenuOpen(false)}
                  className="focusable text-sm"
                  style={{ color: "var(--fg-dim)" }}
                  aria-label="Close navigation"
                >
                  ✕
                </button>
              </div>
              <Rail onNavigate={() => setMenuOpen(false)} />
              <RailFooter />
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
        <h1 className="text-xl" style={{ color: "var(--fg-display)", fontWeight: 500 }}>
          {title}
        </h1>
        {stat}
      </div>
      {actions && <div className="flex items-center gap-2 flex-wrap">{actions}</div>}
    </header>
  );
}
