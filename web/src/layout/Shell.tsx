import { useEffect, useState } from "react";
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { useFamily, minutesLeft } from "../lib/family";
import type { FamilyChild } from "../types";
import { useStepUp } from "../lib/stepup";
import { Wordmark } from "../components/Wordmark";

// The shell is one column of chrome and one column of content — no more.
//
// There used to be a top bar as well, carrying a wordmark, a hamburger and the
// unlocked-countdown chip. It cost 56px of vertical space on every screen to
// show one logo and a chip that is usually absent, and it put the brand above
// the family, which is backwards: the people are the product, the software is
// not. All three moved into the rail, which already existed and already had
// room.

interface NavEntry {
  to: string;
  label: string;
}

const NAV: NavEntry[] = [
  { to: "/", label: "Family" },
  { to: "/devices", label: "Devices" },
  { to: "/settings", label: "Settings" },
];

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
    <div className="armed-chip" title="Changes need no code until this runs out">
      <span className="armed-chip-dot" aria-hidden="true" />
      Unlocked · {Math.floor(s / 60)}:{String(s % 60).padStart(2, "0")}
    </div>
  );
}

function NavList({ onNavigate }: { onNavigate?: () => void }) {
  return (
    <nav className="rail-nav-list" aria-label="Main">
      {NAV.map((n) => (
        <NavLink
          key={n.to}
          to={n.to}
          end={n.to === "/"}
          onClick={onNavigate}
          className="focusable rail-nav"
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
    >
      <span
        className="rail-child-dot"
        style={{ background: `hsl(${hueFor(child.key)} 45% 70%)` }}
        aria-hidden="true"
      />
      <span className="rail-child-name">{child.name}</span>
      {child.pending_requests > 0 && (
        <span
          className="rail-child-asks"
          aria-label={`${child.pending_requests} requests waiting`}
        >
          {child.pending_requests}
        </span>
      )}
      <span className="rail-child-left" data-spent={spent}>
        {left === null
          ? "—"
          : spent
            ? "0m"
            : left >= 60
              ? `${Math.floor(left / 60)}h${String(left % 60).padStart(2, "0")}`
              : `${left}m`}
      </span>
    </NavLink>
  );
}

function Rail({ onNavigate }: { onNavigate?: () => void }) {
  const { children, loading } = useFamily();
  return (
    <div className="rail-body">
      <NavLink
        to="/"
        onClick={onNavigate}
        className="focusable rail-brand"
        aria-label="OpenScreenTime home"
      >
        <Wordmark size={1.0} />
      </NavLink>

      <NavList onNavigate={onNavigate} />

      {/* The rail's family list is a jump list, not a second dashboard: it
          exists so you can reach a child from any page. On the family page
          itself it would just repeat the cards, so it is hidden there. */}
      {(loading || children.length > 0) && (
        <div className="rail-fam">
          <p className="rail-fam-head">Today</p>
          {loading && children.length === 0
            ? [0, 1].map((i) => <span key={i} className="rail-child-wait" aria-hidden="true" />)
            : children.map((c) => (
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
  const [leaving, setLeaving] = useState(false);

  // Signing out is the one navigation that should feel deliberate: the console
  // dims and settles before the login screen replaces it, so it reads as a
  // door closing rather than a page failing to load.
  async function handleLogout() {
    setLeaving(true);
    document.body.dataset.leaving = "true";
    try {
      await logout();
    } finally {
      // Long enough to see the fade, short enough not to feel held up.
      setTimeout(() => {
        delete document.body.dataset.leaving;
        navigate("/login", { replace: true });
      }, 420);
    }
  }

  return (
    <div className="rail-foot">
      <ArmedChip />
      {mock && <p className="rail-mock">Sample data</p>}
      <p className="rail-who">{me?.account?.display_name ?? me?.admin.display_name ?? "Parent"}</p>
      <div className="rail-foot-row">
        <button onClick={toggle} className="focusable rail-foot-btn" type="button">
          {theme === "dark" ? "Light" : "Dark"}
        </button>
        <button
          onClick={handleLogout}
          className="focusable rail-foot-btn"
          type="button"
          disabled={leaving}
        >
          {leaving ? "Signing out…" : "Sign out"}
        </button>
      </div>
    </div>
  );
}

export function Shell() {
  const [menuOpen, setMenuOpen] = useState(false);
  const { pathname } = useLocation();

  // Close the mobile drawer on navigation — leaving it open over the new page
  // is the classic drawer bug.
  useEffect(() => {
    setMenuOpen(false);
  }, [pathname]);

  return (
    <div className="shell">
      {/* Desktop rail */}
      <aside className="shell-rail">
        <Rail />
        <RailFooter />
      </aside>

      {/* Mobile: a single floating trigger instead of a full bar. */}
      <button
        className="focusable shell-menu-btn"
        onClick={() => setMenuOpen(true)}
        aria-label="Open navigation"
        aria-expanded={menuOpen}
        type="button"
      >
        <span aria-hidden="true" />
        <span aria-hidden="true" />
      </button>

      {menuOpen && (
        <div className="shell-drawer" role="dialog" aria-modal="true" aria-label="Navigation">
          <div
            className="shell-drawer-scrim"
            onClick={() => setMenuOpen(false)}
            aria-hidden="true"
          />
          <div className="shell-drawer-panel">
            <Rail onNavigate={() => setMenuOpen(false)} />
            <RailFooter />
          </div>
        </div>
      )}

      <main className="shell-main">
        <Outlet />
      </main>
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
