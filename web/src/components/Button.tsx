import type { ButtonHTMLAttributes, ReactNode } from "react";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "danger-solid";
type Size = "sm" | "md" | "lg";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  children: ReactNode;
}

// One button grammar (docs/DESIGN.md §5): a pill in Figtree 600 with a single
// gentle press. Primary is the brand green — the action to take. Secondary and
// ghost support it; danger is an outline at rest and only goes solid for the
// final destructive confirm.
const base =
  "focusable inline-flex items-center justify-center gap-2 border font-sans font-semibold rounded-full transition-[color,border-color,background-color,box-shadow,transform] duration-150 active:scale-[0.98] disabled:active:scale-100 disabled:opacity-45 disabled:cursor-not-allowed select-none whitespace-nowrap";

const sizes: Record<Size, string> = {
  sm: "text-[0.875rem] px-3.5 min-h-[36px]",
  md: "text-[0.9375rem] px-4 min-h-[44px]",
  lg: "text-[1.0625rem] px-5 min-h-[52px]",
};

const variants: Record<Variant, string> = {
  primary:
    "text-white bg-[var(--brand)] border-[var(--brand)] shadow-[var(--shadow-1)] hover:bg-[var(--brand-strong)] hover:border-[var(--brand-strong)]",
  secondary:
    "text-[var(--ink)] bg-[var(--surface)] border-[var(--line-2)] hover:bg-[var(--surface-2)]",
  ghost:
    "text-[var(--ink-2)] bg-transparent border-transparent hover:bg-[var(--surface-2)] hover:text-[var(--ink)]",
  danger:
    "text-[var(--stop)] bg-transparent border-[var(--accent-dim)] hover:bg-[var(--stop-tint)] hover:border-[var(--stop)]",
  "danger-solid":
    "text-white bg-[var(--stop)] border-[var(--stop)] hover:brightness-95",
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
