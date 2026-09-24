import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Icon, type IconName } from "../components/Icon";

export type ToastTone = "ok" | "warn" | "crit";

export interface ToastAction {
  label: string;
  run: () => void | Promise<void>;
}

interface ToastItem {
  id: number;
  tone: ToastTone;
  message: string;
  action?: ToastAction;
}

interface ToastApi {
  /** A short note after something happened. `action` is usually "Undo". */
  toast: (message: string, tone?: ToastTone, action?: ToastAction) => void;
}

const Ctx = createContext<ToastApi | null>(null);

const ICON: Record<ToastTone, IconName> = { ok: "check", warn: "warning", crit: "warning" };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setItems((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const toast = useCallback(
    (message: string, tone: ToastTone = "ok", action?: ToastAction) => {
      const id = nextId.current++;
      setItems((prev) => [...prev.slice(-2), { id, tone, message, action }]);
      // Long enough to read and reach the undo; failures stay a little longer.
      window.setTimeout(() => dismiss(id), tone === "crit" ? 9000 : action ? 8000 : 5000);
    },
    [dismiss],
  );

  const api = useMemo(() => ({ toast }), [toast]);

  return (
    <Ctx.Provider value={api}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className="toast" data-tone={t.tone}>
            <Icon name={ICON[t.tone]} size={20} className="toast-ic" />
            <p className="toast-msg">{t.message}</p>
            {t.action && (
              <button
                type="button"
                className="btn btn-quiet btn-sm toast-action"
                onClick={() => {
                  dismiss(t.id);
                  void t.action?.run();
                }}
              >
                {t.action.label}
              </button>
            )}
            <button
              type="button"
              onClick={() => dismiss(t.id)}
              className="btn-icon btn-icon-sm"
              aria-label="Dismiss"
            >
              <Icon name="close" size={16} />
            </button>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export function useToast(): ToastApi {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useToast must be used within <ToastProvider>");
  return ctx;
}

export function errMsg(e: unknown, fallback: string): string {
  return e instanceof Error && e.message ? e.message : fallback;
}
