import type { ButtonHTMLAttributes, ReactNode } from "react";

type Variant = "primary" | "danger" | "ghost";
type Size = "sm" | "md";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  children: ReactNode;
}

// Silkscreen buttons: mono uppercase, hairline outline that sharpens on hover,
// and the same press (scale 0.97) as every other control in the console.
// `danger` = accent-red — reserved for locked/tamper/destructive actions.
const base =
  "focusable inline-flex items-center justify-center gap-2 border font-mono uppercase tracking-label rounded transition-[color,border-color,background-color,transform] duration-150 active:scale-[0.97] disabled:active:scale-100 disabled:opacity-40 disabled:cursor-not-allowed select-none whitespace-nowrap";

const sizes: Record<Size, string> = {
  sm: "text-[0.625rem] px-2.5 py-1",
  md: "text-[0.625rem] px-3.5 py-2",
};

const variants: Record<Variant, string> = {
  primary:
    "border-line-2 text-fg bg-transparent hover:border-fg disabled:hover:border-line-2",
  danger:
    "border-accent-dim text-accent bg-transparent hover:border-accent disabled:hover:border-accent-dim",
  ghost: "border-transparent text-fg-dim hover:text-fg hover:border-line",
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
