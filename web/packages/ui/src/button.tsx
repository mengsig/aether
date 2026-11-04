import type { ButtonHTMLAttributes, Ref } from "react";
import { classes, invariant } from "./classes.ts";

export type ButtonVariant = "primary" | "secondary" | "quiet" | "danger";
const VARIANTS: ReadonlySet<string> = new Set(["primary", "secondary", "quiet", "danger"]);

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  readonly variant?: ButtonVariant;
  readonly size?: "sm" | "md";
  readonly ref?: Ref<HTMLButtonElement>;
}

/** A button that never submits a form by accident: `type` defaults to "button". */
export function Button({ variant = "secondary", size = "md", type = "button", className, ...rest }: ButtonProps) {
  invariant(VARIANTS.has(variant), `unknown button variant ${variant}`);
  invariant(rest.children !== undefined || rest["aria-label"] !== undefined, "a button needs visible text or an aria-label");
  return <button {...rest} type={type} className={classes("ui-button", className)} data-variant={variant} data-size={size} />;
}
