/// <reference types="vite/client" />
// @aether-ui/ui: tokens, primitives and the shell (CONTRACTS.md §2.1, §4).
// The shared renderer also uses KaTeX and lazy Mermaid; the app wires the kit to core.

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
export { Toast, ToastRegion, type ToastMessage, type ToastRegionProps } from "./toast.tsx";
export { Tooltip } from "./tooltip.tsx";
export { Badge, StatusPill, statusToken, STATUSES, type Status, type Tone } from "./badge.tsx";
export { Skeleton, EmptyState, ErrorState, type EmptyStateProps, type ErrorStateProps } from "./states.tsx";
export { VirtualList, visibleRange, type VirtualListProps, type VirtualRange } from "./virtual-list.tsx";
export { NavRail, navSections, navHref, activeHref, redundantHeading, NAV_SECTIONS, type NavItem, type NavSection, type NavRailProps } from "./nav-rail.tsx";
export { NavIcon } from "./nav-icons.tsx";
export { CommandPalette, filterCommands, type Command, type CommandPaletteProps } from "./command-palette.tsx";
export { EntityActionMenu, EntityActionProvider, actionsFor, runAction } from "./entity-actions.tsx";
export { ConnectionList } from "./shell-parts.tsx";
export { AppShell, screenFor, type AppShellProps, type ShellMatch } from "./app-shell.tsx";
export { PageHeader, type PageHeaderProps } from "./page.tsx";
export { FOCUSABLE } from "./focus.ts";
export { Picker, listKey, SEARCH_FROM, type PickerProps } from "./picker.tsx";
export { groupOptions, filterOptions, moveIndex, typeAhead, type PickerOption, type PickerGroup } from "./picker-model.ts";
export { Combo, type ComboProps } from "./combo.tsx";
export { Switch, Segmented, type SwitchProps, type SegmentedProps, type SegmentOption } from "./switch.tsx";
export { TextArea, type TextAreaProps } from "./textarea.tsx";
export { ModelPicker, modelOptions, modelRefText, billingOf, type ModelPickerProps, type ModelOptionsSettings, type Billing } from "./model-picker.tsx";
export { BarChart, Donut, Sparkline, StatTile, donutArcs, sparkPath, type ChartPoint, type ChartTone, type DonutArc } from "./charts.tsx";

export { Markdown, InlineMarkdown } from "./markdown/markdown.tsx";
export { safeHref } from "./markdown/url.ts";
