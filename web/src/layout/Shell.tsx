import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";

const NAV = [
  { to: "/devices", label: "DEVICES" },
  { to: "/profiles", label: "PROFILES" },
  { to: "/events", label: "EVENTS" },
  { to: "/settings", label: "SETTINGS" },
];

export function Shell() {
  const { me, mock, logout } = useSession();
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();

  async function handleLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  return (
    <div className="min-h-screen flex">
      {/* Left rail */}
      <aside
        className="w-56 flex-none flex flex-col border-r sticky top-0 h-screen"
        style={{ borderColor: "var(--line)", background: "var(--surface)" }}
      >
        <div className="px-5 py-6 border-b" style={{ borderColor: "var(--line)" }}>
          <div className="flex items-center gap-2">
            <span className="led led-glow-crit" style={{ background: "var(--accent)" }} />
            <span className="wordmark text-sm text-fg">SENTINEL</span>
          </div>
          <p className="label mt-2" style={{ color: "var(--fg-faint)" }}>
            ZERO-TRUST CONTROL
          </p>
        </div>

        <nav className="flex flex-col p-3 gap-1 flex-1">
          {NAV.map((n) => (
            <NavLink
              key={n.to}
              to={n.to}
              className={({ isActive }) =>
                `focusable flex items-center gap-2.5 px-3 py-2.5 rounded font-mono uppercase tracking-label text-xs transition-colors ${
                  isActive
                    ? "text-fg"
                    : "text-fg-dim hover:text-fg hover:bg-surface-2"
                }`
              }
              style={({ isActive }) =>
                isActive ? { background: "var(--surface-2)" } : undefined
              }
            >
              {({ isActive }) => (
                <>
                  <span
                    className="led"
                    style={{
                      width: 6,
                      height: 6,
                      background: isActive ? "var(--accent)" : "var(--fg-faint)",
                    }}
                  />
                  {n.label}
                </>
              )}
            </NavLink>
          ))}
        </nav>

        {/* Admin identity + logout */}
        <div className="p-3 border-t" style={{ borderColor: "var(--line)" }}>
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
              <p className="dot text-[0.625rem] text-fg truncate">
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
      </aside>

      {/* Main */}
      <main className="flex-1 min-w-0">
        <div className="max-w-[1200px] mx-auto px-8 py-8">
          <Outlet />
        </div>
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
      <div className="flex items-end gap-6">
        <h1 className="dot text-xl text-fg">{title}</h1>
        {stat}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}
