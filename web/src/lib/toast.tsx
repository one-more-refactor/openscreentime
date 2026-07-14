import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

export type ToastTone = "ok" | "warn" | "crit";

interface ToastItem {
  id: number;
  tone: ToastTone;
  message: string;
}

interface ToastApi {
  /** Show a toast. Defaults to `crit` since most call sites report failures. */
  toast: (message: string, tone?: ToastTone) => void;
}

const Ctx = createContext<ToastApi | null>(null);

const toneColor: Record<ToastTone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  crit: "var(--crit)",
};

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setItems((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const toast = useCallback(
    (message: string, tone: ToastTone = "crit") => {
      const id = nextId.current++;
      setItems((prev) => [...prev.slice(-3), { id, tone, message }]);
      window.setTimeout(() => dismiss(id), tone === "crit" ? 8000 : 5000);
    },
    [dismiss],
  );

  const api = useMemo(() => ({ toast }), [toast]);

  return (
    <Ctx.Provider value={api}>
      {children}
      <div
        className="fixed bottom-4 left-1/2 -translate-x-1/2 z-[70] flex flex-col gap-2 w-[min(28rem,calc(100vw-2rem))]"
        role="status"
        aria-live="polite"
      >
        {items.map((t) => (
          <div
            key={t.id}
            className="relative flex items-start gap-3 border rounded px-3 py-2.5 font-mono text-xs"
            style={{
              borderColor: toneColor[t.tone],
              background: "var(--surface)",
              color: "var(--fg)",
            }}
          >
            <span className="tick tick-tl" />
            <span className="tick tick-tr" />
            <span className="tick tick-bl" />
            <span className="tick tick-br" />
            <span
              className={`led mt-1 ${t.tone === "crit" ? "led-glow-crit" : t.tone === "warn" ? "led-glow-warn" : "led-glow-ok"}`}
              style={{ background: toneColor[t.tone] }}
              aria-hidden
            />
            <p className="flex-1 min-w-0 break-words">{t.message}</p>
            <button
              onClick={() => dismiss(t.id)}
              className="focusable flex-none text-sm leading-none"
              style={{ color: "var(--fg-faint)" }}
              aria-label="Dismiss notification"
            >
              ✕
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
