import { createContext, useCallback, useContext, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import { invariant } from "./classes.ts";

export type ThemePreference = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";
export const THEME_PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

const DARK_QUERY = "(prefers-color-scheme: dark)";

export function isThemePreference(value: unknown): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function resolveTheme(preference: ThemePreference, systemDark: boolean): ResolvedTheme {
  invariant(isThemePreference(preference), `unknown theme preference ${String(preference)}`);
  if (preference === "system") return systemDark ? "dark" : "light";
  return preference;
}

/** Sets `data-theme` on the root element, which selects the token set in tokens.css. */
export function applyTheme(root: HTMLElement, theme: ResolvedTheme): void {
  invariant(theme === "light" || theme === "dark", `unknown theme ${String(theme)}`);
  root.dataset.theme = theme;
}

export function systemPrefersDark(): boolean {
  return typeof matchMedia === "function" && matchMedia(DARK_QUERY).matches;
}

function subscribeSystem(listener: () => void): () => void {
  if (typeof matchMedia !== "function") return () => undefined;
  const query = matchMedia(DARK_QUERY);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}

interface ThemeContextValue {
  readonly preference: ThemePreference;
  readonly resolved: ResolvedTheme;
  readonly setPreference: (preference: ThemePreference) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export interface ThemeProviderProps {
  readonly initialPreference?: ThemePreference;
  /** Persists a change; the provider has already applied it. */
  readonly onPreferenceChange?: (preference: ThemePreference) => void;
  readonly children?: ReactNode;
}

/** Follows the system scheme unless the user picked one, and keeps `<html data-theme>` in step. */
export function ThemeProvider({ initialPreference = "system", onPreferenceChange, children }: ThemeProviderProps) {
  const [preference, setOwn] = useState(initialPreference);
  const systemDark = useSyncExternalStore(subscribeSystem, systemPrefersDark, () => false);
  const resolved = resolveTheme(preference, systemDark);
  useEffect(() => applyTheme(document.documentElement, resolved), [resolved]);
  const setPreference = useCallback((next: ThemePreference) => {
    invariant(isThemePreference(next), `unknown theme preference ${String(next)}`);
    setOwn(next);
    onPreferenceChange?.(next);
  }, [onPreferenceChange]);
  const value = useMemo(() => ({ preference, resolved, setPreference }), [preference, resolved, setPreference]);
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

/** The theme context, or null outside a ThemeProvider (the shell then hides its theme controls). */
export function useTheme(): ThemeContextValue | null {
  return useContext(ThemeContext);
}
