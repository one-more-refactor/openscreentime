import { useEffect, useState } from "react";
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { useFamily, minutesLeft } from "../lib/family";
import type { FamilyChild } from "../types";
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
  { to: "/me", label: "My screen time" },
  { to: "/settings", label: "Settings" },
];

/** A small lock, open or shut. The shackle swings on a CSS transition. */
export function LockGlyph({ open, size = 14 }: { open: boolean; size?: number }) {
  return (
    <svg
      className="lockglyph"
      data-open={open}
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="5" y="11" width="14" height="10" rx="2" />
      <path className="lockglyph-shackle" d="M8 11V8a4 4 0 0 1 8 0v3" />
    </svg>
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
  const { me } = useSession();

  // Close the mobile drawer on navigation — leaving it open over the new page
  // is the classic drawer bug.
  useEffect(() => {
    setMenuOpen(false);
  }, [pathname]);

  // A member has one page and no navigation to anywhere else: no rail, no
  // drawer, no family list of siblings (members never see each other). The
  // page itself carries the wordmark and the sign-out.
  if (me?.account?.role === "member") {
    return (
      <div className="shell shell-member">
        <main className="shell-main">
          <Outlet />
        </main>
      </div>
    );
  }

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
