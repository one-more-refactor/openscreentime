import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { SessionProvider, useSession } from "./lib/session";
import { ToastProvider } from "./lib/toast";
import { ConfirmProvider } from "./lib/confirm";
import { Shell } from "./layout/Shell";
import { Login } from "./pages/Login";
import { Welcome } from "./pages/Welcome";
import { Family } from "./pages/Family";
import { ChildDetail } from "./pages/ChildDetail";
import { Computers } from "./pages/Computers";
import { AddChild } from "./pages/AddChild";
import { Settings } from "./pages/Settings";
import { Me } from "./pages/Me";
import { Wordmark } from "./components/Wordmark";

function RequireAuth({ children }: { children: React.ReactNode }) {
  const { me, loading } = useSession();
  if (loading) {
    // Checking the session: the lockup, breathing once a second or so — the
    // page's shape, not a spinner.
    return (
      <div className="boot" aria-busy="true" aria-label="Signing in">
        <Wordmark size={1.375} />
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
 * bookmarked page on a child's laptop lands on /me instead of a wall of
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
          <Route path="/welcome" element={<Welcome />} />
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
            <Route path="/child/:key" element={<ChildDetail />} />
            <Route path="/computers" element={<Computers />} />
            <Route path="/add" element={<AddChild />} />
            <Route path="/settings" element={<Settings />} />
            {/* The person's own page — the only page a member session has. */}
            <Route path="/me" element={<Me />} />
            {/* Old addresses, kept working. */}
            <Route path="/devices" element={<Navigate to="/computers" replace />} />
            <Route path="/family" element={<Navigate to="/" replace />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </ToastProvider>
    </SessionProvider>
  );
}
