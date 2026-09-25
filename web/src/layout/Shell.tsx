// ============================================================================
// The shell (brand board § "Family — the web console home"): one rail, one
// page. The rail carries the lockup, the four places (with their icons), the
// "Today" jump list — every person's ring and time left — and who is signed
// in, with the one sign-out in the whole console.
//
// Below desktop width the rail becomes a drawer, opened from a slim bar that
// carries the lockup and a menu button.
// ============================================================================
import { useCallback, useEffect, useRef, useState } from "react";
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useFamily, minutesLeft, ringTarget, stoppedBy, type StopKind } from "../lib/family";
import type { FamilyChild } from "../types";
import { Wordmark } from "../components/Wordmark";
import { Icon, type IconName } from "../components/Icon";
import { Ring } from "../components/Ring";
import { durationShort } from "../lib/format";
import { usingMock } from "../api";

interface NavEntry {
  to: string;
  label: string;
  icon: IconName;
}

const NAV: NavEntry[] = [
  { to: "/", label: "Family", icon: "family" },
  { to: "/computers", label: "Computers", icon: "laptop" },
  { to: "/settings", label: "Settings", icon: "settings" },
  { to: "/me", label: "Me", icon: "person" },
];

/** A stop in the jump list, by its reason. */
const STOPPED: Record<StopKind, string> = {
  limit: "time's up",
  bedtime: "bedtime",
  outside_hours: "outside hours",
  paused: "paused",
};

/** One person in the jump list: their ring, their name, their time left. */
function TodayRow({ child, onNavigate }: { child: FamilyChild; onNavigate?: () => void }) {
  const left = minutesLeft(child);
  // The ring fills toward used + left — the same number, an unlock's time included.
  const total = ringTarget(child.used_minutes, left);
  const paused = child.locked && child.devices.length > 0;
  // An adult keeping their own time: their limit is theirs, not the rail's.
  const own = child.self_managed === true || child.managed === false || child.age_bracket === "adult";
  const used = own ? null : total && total > 0 ? child.used_minutes / total : total === 0 ? 1 : null;
  const stop = own ? null : stoppedBy(child);
  const tone = own ? undefined : stop || left === 0 ? "stop" : left != null && left <= 15 ? "warn" : undefined;
  const meta = paused
    ? "paused"
    : own
      ? "own rules"
      : stop
        ? STOPPED[stop]
        : left === null
          ? "no limit"
          : left < 60
            ? `${left} min`
            : durationShort(left);
  return (
    <li>
      <NavLink
        to={`/child/${encodeURIComponent(child.key)}`}
        onClick={onNavigate}
        className={({ isActive }) => `today-row${isActive ? " active" : ""}`}
      >
        <Ring size={28} used={used} minutesLeft={left} paused={paused} on="paper" />
        <span className="today-name">{child.name}</span>
        {child.pending_requests > 0 && (
          <span className="today-ask" role="img" aria-label="asked for more time" />
        )}
        <span className="today-left num" data-tone={tone}>
          {meta}
        </span>
      </NavLink>
    </li>
  );
}

function Rail({ onNavigate }: { onNavigate?: () => void }) {
  const { children, requests, loading } = useFamily();
  const { me, mock, logout } = useSession();
  const navigate = useNavigate();
  const [leaving, setLeaving] = useState(false);

  // Signing out is the one navigation that should feel deliberate: the
  // console settles before the sign-in page replaces it.
  async function signOut() {
    setLeaving(true);
    document.body.dataset.leaving = "true";
    try {
      await logout();
    } finally {
      setTimeout(() => {
        delete document.body.dataset.leaving;
        navigate("/login", { replace: true });
      }, 420);
    }
  }

  const name = me?.account?.display_name ?? me?.admin.display_name ?? "You";
  return (
    <>
      <NavLink to="/" onClick={onNavigate} className="rail-brand" aria-label="OpenScreenTime, home">
        <Wordmark />
      </NavLink>

      <nav className="nav" aria-label="Main">
        {NAV.map((n) => (
          <NavLink
            key={n.to}
            to={n.to}
            end={n.to === "/"}
            onClick={onNavigate}
            className={({ isActive }) => `nav-item${isActive ? " active" : ""}`}
          >
            <Icon name={n.icon} size={20} />
            {n.label}
            {n.to === "/" && requests.length > 0 && (
              <span className="nav-count num" aria-label={`${requests.length} waiting`}>
                {requests.length}
              </span>
            )}
          </NavLink>
        ))}
      </nav>

      {(loading || children.length > 0) && (
        <div className="today">
          <p className="today-head">Today</p>
          {loading && children.length === 0 ? (
            [0, 1].map((i) => <span key={i} className="wait today-wait" aria-hidden="true" />)
          ) : (
            <ul className="today-list">
              {children.map((c) => (
                <TodayRow key={c.key} child={c} onNavigate={onNavigate} />
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="rail-foot">
        {mock && <p className="rail-mock">Sample data</p>}
        <div className="rail-me">
          <span className="rail-me-name">{name}</span>
          <button
            type="button"
            className="btn-icon"
            onClick={() => void signOut()}
            disabled={leaving}
            aria-label="Sign out"
            title="Sign out"
          >
            <Icon name="sign-out" size={18} />
          </button>
        </div>
      </div>
    </>
  );
}

/** The rail as a drawer: Escape closes it, focus stays inside while open and
 * returns to the menu button after. */
function Drawer({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const back = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("a, button")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (e.key !== "Tab" || !ref.current) return;
      const els = Array.from(ref.current.querySelectorAll<HTMLElement>("a[href], button:not([disabled])"));
      if (!els.length) return;
      const first = els[0];
      const last = els[els.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      back?.focus?.();
    };
  }, [onClose]);

  return (
    <div className="drawer" role="dialog" aria-modal="true" aria-label="Menu">
      <div className="drawer-scrim" onClick={onClose} aria-hidden="true" />
      <aside className="rail" ref={ref}>
        <button type="button" className="btn-icon drawer-close" onClick={onClose} aria-label="Close menu">
          <Icon name="close" size={20} />
        </button>
        <Rail onNavigate={onClose} />
      </aside>
    </div>
  );
}

export function Shell() {
  // Design review only: ?mock=menu opens the drawer for a screenshot.
  const [menuOpen, setMenuOpen] = useState(
    () => usingMock && new URLSearchParams(window.location.search).get("mock") === "menu",
  );
  const closeMenu = useCallback(() => setMenuOpen(false), []);
  const { pathname } = useLocation();
  const { me } = useSession();

  // Close the drawer on navigation — leaving it open over the new page is
  // the classic drawer bug.
  const lastPath = useRef(pathname);
  useEffect(() => {
    if (lastPath.current === pathname) return;
    lastPath.current = pathname;
    setMenuOpen(false);
  }, [pathname]);

  // A member has one page and nowhere else to go: no rail, no drawer, no
  // list of siblings (members never see each other).
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
      <aside className="rail">
        <Rail />
      </aside>

      <header className="mbar">
        <NavLink to="/" aria-label="OpenScreenTime, home">
          <Wordmark />
        </NavLink>
        <button
          type="button"
          className="btn-icon"
          onClick={() => setMenuOpen(true)}
          aria-label="Menu"
          aria-expanded={menuOpen}
        >
          <Icon name="menu" size={22} />
        </button>
      </header>

      {menuOpen && <Drawer onClose={closeMenu} />}

      <main className="shell-main">
        <Outlet />
      </main>
    </div>
  );
}
