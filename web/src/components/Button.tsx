import type { ButtonHTMLAttributes, ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

export type ButtonVariant = "primary" | "secondary" | "quiet" | "danger" | "danger-solid";
export type ButtonSize = "sm" | "md" | "lg";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** A leading icon from the brand set. */
  icon?: IconName;
  /** Full width. */
  block?: boolean;
  children: ReactNode;
}

/**
 * One button grammar (brand board, docs/DESIGN.md §5): a pill in Figtree 600.
 * Primary is the brand green — the one main action on a screen. Secondary is
 * white with a hairline; quiet is text; danger is red text that only fills
 * for the final destructive confirm. Sizes 36 / 44 / 52 px.
 */
export function buttonClass(
  variant: ButtonVariant = "primary",
  size: ButtonSize = "md",
  block = false,
): string {
  return `btn btn-${variant}${size === "md" ? "" : ` btn-${size}`}${block ? " btn-block" : ""}`;
}

export function Button({
  variant = "primary",
  size = "md",
  icon,
  block = false,
  className = "",
  type = "button",
  children,
  ...rest
}: Props) {
  return (
    <button type={type} className={`${buttonClass(variant, size, block)} ${className}`} {...rest}>
      {icon && <Icon name={icon} size={size === "sm" ? 16 : 18} />}
      {children}
    </button>
  );
}
