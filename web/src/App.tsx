import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { SessionProvider, useSession } from "./lib/session";
import { ToastProvider } from "./lib/toast";
import { ConfirmProvider } from "./lib/confirm";
import { Shell } from "./layout/Shell";
import { Login } from "./pages/Login";
import { Welcome } from "./pages/Welcome";
import { Family } from "./pages/Family";
import { ChildDetail } from "./pages/ChildDetail";
import { Devices } from "./pages/Devices";
import { AddChild } from "./pages/AddChild";
import { Settings } from "./pages/Settings";
import { Me } from "./pages/Me";
import { Enroll2FA } from "./pages/Enroll2FA";
import { useEffect, useState } from "react";
import * as api from "./api";
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

/**
 * First-login second-factor gate. A hub account (owner/parent) with no factor
 * enrolled is sent through a one-time setup on the way in, so the step-up
 * "type the 6 digits" prompt is never a dead end for a factor nobody set up.
 * Members are exempt (their page needs no step-up). Dismissible for the session
 * only — the console never hard-locks you out of itself.
 */
function TwoFactorGate({ children }: { children: React.ReactNode }) {
  const { me } = useSession();
  const isHub = !!me && me.account?.role !== "member";
  const [need, setNeed] = useState<boolean | null>(null);

  useEffect(() => {
    let alive = true;
    if (!isHub) {
      setNeed(false);
      return;
    }
    let deferred = false;
    try {
      deferred = sessionStorage.getItem("ost-2fa-defer") === "1";
    } catch {
      /* ignore */
    }
    if (deferred) {
      setNeed(false);
      return;
    }
    api
      .getTwoFactorStatus()
      .then((s) => alive && setNeed(!s.totp_enrolled && !s.telegram_available))
      .catch(() => alive && setNeed(false)); // never block on a status hiccup
    return () => {
      alive = false;
    };
  }, [isHub]);

  if (need === null) {
    return (
      <div className="min-h-screen flex items-center justify-center gap-3">
        <StatusLed tone="ok" pulse />
        <span className="label">Loading…</span>
      </div>
    );
  }
  if (need) return <Enroll2FA who={me?.account?.display_name} onDone={() => setNeed(false)} />;
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
                  <TwoFactorGate>
                    <Shell />
                  </TwoFactorGate>
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
