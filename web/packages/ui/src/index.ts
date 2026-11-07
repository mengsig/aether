/// <reference types="vite/client" />
// @aether-ui/ui: tokens, primitives and the shell (CONTRACTS.md §2.1, §4).
// It imports only React and protocol types (§2.2); the app wires it to core.

import "./tokens.css";
import "./base.css";
import "./primitives.css";
import "./shell.css";

export { classes } from "./classes.ts";
export { ThemeProvider, useTheme, resolveTheme, applyTheme, systemPrefersDark, isThemePreference, THEME_PREFERENCES } from "./theme.tsx";
export type { ThemePreference, ResolvedTheme, ThemeProviderProps } from "./theme.tsx";
export { Button, type ButtonProps, type ButtonVariant } from "./button.tsx";
export { Field, type FieldProps } from "./field.tsx";
export { Menu, nextEnabled, type MenuItem, type MenuProps } from "./menu.tsx";
export { Dialog, Sheet, type DialogProps } from "./dialog.tsx";
export { Tabs, type TabItem, type TabsProps } from "./tabs.tsx";
export { Toast, ToastRegion, type ToastMessage } from "./toast.tsx";
export { Tooltip } from "./tooltip.tsx";
export { Badge, StatusPill, statusToken, STATUSES, type Status, type Tone } from "./badge.tsx";
export { Skeleton, EmptyState, ErrorState, type EmptyStateProps, type ErrorStateProps } from "./states.tsx";
export { VirtualList, visibleRange, type VirtualListProps, type VirtualRange } from "./virtual-list.tsx";
export { NavRail, navSections, navHref, activeHref, NAV_SECTIONS, type NavItem, type NavSection, type NavRailProps } from "./nav-rail.tsx";
export { CommandPalette, filterCommands, type Command, type CommandPaletteProps } from "./command-palette.tsx";
export { EntityActionMenu, EntityActionProvider, actionsFor, runAction } from "./entity-actions.tsx";
export { ConnectionList } from "./shell-parts.tsx";
export { AppShell, screenFor, type AppShellProps, type ShellMatch } from "./app-shell.tsx";
export { FOCUSABLE } from "./focus.ts";
