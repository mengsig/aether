// The Noesis settings form as a draft: its text fields, their checks while typing, and the patch that
// turns the daemon's settings into the draft. Pure, so the sheet and the off state's "Turn on" share it.

import type { aether } from "@aether-ui/protocol";
import { draftKey, invariant } from "./model.ts";

/** Where the sheet keeps what was typed until it is saved or discarded; each daemon's settings have their own. */
export const settingsDraftKey = (conn: string): string => draftKey("settings", conn);

export type SettingsDraft = {
  readonly enabled: boolean;
  readonly allowApi: boolean;
  readonly interactiveUsd: string;
  readonly backgroundUsd: string;
  readonly backgroundTokens: string;
  /** `provider/model`, or "" for the daemon's default (the first ready subscription model). */
  readonly strong: string;
  readonly cheap: string;
  readonly extraction: "manual" | "auto";
};

export type BudgetField = "interactive_usd_daily" | "background_usd_daily" | "background_tokens_daily";

export function draftOf(settings: aether.NoesisSettingsView): SettingsDraft {
  return {
    enabled: settings.enabled, allowApi: settings.billing.allow_api,
    interactiveUsd: String(settings.budget.interactive_usd_daily), backgroundUsd: String(settings.budget.background_usd_daily),
    backgroundTokens: String(settings.budget.background_tokens_daily),
    strong: settings.models.strong ?? "", cheap: settings.models.cheap ?? "", extraction: settings.extraction.sessions,
  };
}

/** A dollar amount ≥ 0, or why not. */
export function amount(text: string): number | string {
  const value = Number(text);
  return text.trim() !== "" && Number.isFinite(value) && value >= 0 ? value : "Enter an amount of 0 or more";
}

/** A whole token count ≥ 0, or why not. */
export function tokens(text: string): number | string {
  const value = Number(text);
  return text.trim() !== "" && Number.isSafeInteger(value) && value >= 0 ? value : "Enter a whole number of 0 or more";
}

/** Each budget field's parsed value or the error to show under it, checked as the person types. */
export function budgetValues(draft: SettingsDraft): Readonly<Record<BudgetField, number | string>> {
  return {
    interactive_usd_daily: amount(draft.interactiveUsd),
    background_usd_daily: amount(draft.backgroundUsd),
    background_tokens_daily: tokens(draft.backgroundTokens),
  };
}

/** The patch that turns `settings` into `draft`, or the field errors that stop it. */
export function patchOf(settings: aether.NoesisSettingsView, draft: SettingsDraft): { patch: aether.NoesisSettingsPatch; errors: Partial<Record<BudgetField, string>> } {
  const errors: Partial<Record<BudgetField, string>> = {};
  const patch: Record<string, unknown> = {};
  if (draft.enabled !== settings.enabled) patch.enabled = draft.enabled;
  if (draft.allowApi !== settings.billing.allow_api) patch.billing = { allow_api: draft.allowApi };
  const budget: Partial<Record<BudgetField, number>> = {};
  for (const [key, value] of Object.entries(budgetValues(draft)) as [BudgetField, number | string][]) {
    if (typeof value === "string") errors[key] = value;
    else if (value !== settings.budget[key]) budget[key] = value;
  }
  if (Object.keys(budget).length > 0) patch.budget = budget;
  const models: Record<string, string | null> = {};
  if (draft.strong.trim() !== (settings.models.strong ?? "")) models.strong = draft.strong.trim() || null;
  if (draft.cheap.trim() !== (settings.models.cheap ?? "")) models.cheap = draft.cheap.trim() || null;
  if (Object.keys(models).length > 0) patch.models = models;
  if (draft.extraction !== settings.extraction.sessions) patch.extraction = { sessions: draft.extraction };
  invariant(Object.keys(errors).every((key) => key in settings.budget), "only budget fields are checked here");
  invariant(!("budget" in patch) || Object.values(budget).every((value) => value >= 0), "a budget patch carries nonnegative numbers");
  return { patch: patch as aether.NoesisSettingsPatch, errors };
}
