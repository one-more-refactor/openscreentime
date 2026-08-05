import { Navigate, Route, Routes } from "react-router-dom";
import { SessionProvider, useSession } from "./lib/session";
import { ToastProvider } from "./lib/toast";
import { Shell } from "./layout/Shell";
import { Login } from "./pages/Login";
import { Family } from "./pages/Family";
import { ChildDetail } from "./pages/ChildDetail";
import { AddChild } from "./pages/AddChild";
import { Settings } from "./pages/Settings";
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

export function App() {
  return (
    <SessionProvider>
      <ToastProvider>
      <Routes>
        <Route path="/login" element={<Login />} />
        <Route
          element={
            <RequireAuth>
              <Shell />
            </RequireAuth>
          }
        >
          {/* Home is the family, not the fleet. */}
          <Route index element={<Family />} />
          <Route path="/family" element={<Family />} />
          <Route path="/child/:key" element={<ChildDetail />} />
          <Route path="/add" element={<AddChild />} />
          <Route path="/settings" element={<Settings />} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
      </ToastProvider>
    </SessionProvider>
  );
}
