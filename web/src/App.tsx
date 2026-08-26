import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { SessionProvider, useSession } from "./lib/session";
import { ToastProvider } from "./lib/toast";
import { ConfirmProvider } from "./lib/confirm";
import { Shell } from "./layout/Shell";
import { Login } from "./pages/Login";
import { Family } from "./pages/Family";
import { ChildDetail } from "./pages/ChildDetail";
import { Devices } from "./pages/Devices";
import { AddChild } from "./pages/AddChild";
import { Settings } from "./pages/Settings";
import { Me } from "./pages/Me";
import { StatusLed } from "./components";

function RequireAuth({ children }: { children: React.ReactNode }) {
  const { me, loading } = useSession();
  if (loading) {
    return (
      <div className="min-h-screen flex items-center justify-center gap-3">
        <StatusLed tone="ok" pulse />
        <span className="label">AUTHENTICATING…</span>
      </div>
    );
  }
  if (!me) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

/**
 * A member (a child, or a self-tracking adult who is not a hub) has exactly
 * one page: their own. The server enforces that (every other /api route is
 * 403 for a member session); this keeps the URL honest about it too, so a
 * bookmarked /family on a child's laptop lands on /me instead of a wall of
 * errors.
 */
function MemberGate({ children }: { children: React.ReactNode }) {
  const { me } = useSession();
  const { pathname } = useLocation();
  if (me?.account?.role === "member" && pathname !== "/me") {
    return <Navigate to="/me" replace />;
  }
  return <>{children}</>;
}

export function App() {
  return (
    <SessionProvider>
      <ToastProvider>
      <Routes>
        <Route path="/login" element={<Login />} />
        <Route
          element={
            <RequireAuth>
              <ConfirmProvider>
                <MemberGate>
                  <Shell />
                </MemberGate>
              </ConfirmProvider>
            </RequireAuth>
          }
        >
          {/* Home is the family, not the fleet. */}
          <Route index element={<Family />} />
          <Route path="/family" element={<Family />} />
          <Route path="/child/:key" element={<ChildDetail />} />
          <Route path="/devices" element={<Devices />} />
          <Route path="/add" element={<AddChild />} />
          <Route path="/settings" element={<Settings />} />
          {/* The person's own page — the only page a member session has. */}
          <Route path="/me" element={<Me />} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      </ToastProvider>
    </SessionProvider>
  );
}
