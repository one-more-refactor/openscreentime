import type { ButtonHTMLAttributes, ReactNode } from "react";

type Variant = "primary" | "danger" | "ghost";
type Size = "sm" | "md";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  children: ReactNode;
}

// Mono-outline buttons. `primary` = hairline outline that inverts on hover,
// `danger` = accent-red (used rarely, for destructive/critical actions).
const base =
  "focusable inline-flex items-center justify-center gap-2 border font-mono uppercase tracking-label rounded transition-colors disabled:opacity-40 disabled:cursor-not-allowed select-none whitespace-nowrap";

const sizes: Record<Size, string> = {
  sm: "text-[0.625rem] px-2.5 py-1",
  md: "text-[0.6875rem] px-3.5 py-2",
};

const variants: Record<Variant, string> = {
  primary:
    "border-line-2 text-fg bg-transparent hover:bg-fg hover:text-bg hover:border-fg",
  danger:
    "border-accent text-accent bg-transparent hover:bg-accent hover:text-white",
  ghost:
    "border-transparent text-fg-dim hover:text-fg hover:border-line",
};

export function Button({
  variant = "primary",
  size = "md",
  className = "",
  children,
  ...rest
}: Props) {
  return (
    <button
      className={`${base} ${sizes[size]} ${variants[variant]} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}
