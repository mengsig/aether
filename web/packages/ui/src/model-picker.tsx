// ModelPicker: one way to choose a model everywhere. Options come from the daemon's `list_models`
// and `providers` replies (the feature screen reads them through core); the picker groups them
// by provider, marks readiness and billing, and hands back the `provider/model` text the forms
// already send. Nothing about the protocol changes.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "./classes.ts";
import { Picker } from "./picker.tsx";
import type { PickerOption } from "./picker-model.ts";

export type Billing = "subscription" | "api" | "unknown";

/** The `provider/model` text a form sends for a model. */
export function modelRefText(ref: aether.ModelRef): string {
  invariant(ref.provider.length > 0 && ref.model.length > 0, "a model ref names its provider and model");
  return `${ref.provider}/${ref.model}`;
}

/**
 * How a model is paid for, as Noesis decides it: a provider the daemon lists as an engine (a CLI
 * signed in to a subscription) spends that subscription; every other provider bills per request.
 * Unknown until the `providers` reply names the provider.
 */
export function billingOf(model: aether.ModelInfo, providers: readonly aether.ProviderInfo[]): Billing {
  const provider = providers.find((entry) => entry.provider === model.model.provider);
  if (provider === undefined) return "unknown";
  invariant(typeof provider.engine === "boolean", `provider ${provider.provider} says whether it is an engine`);
  return provider.engine ? "subscription" : "api";
}

export interface ModelOptionsSettings {
  /** API-billed models are listed; off, only models known to spend a subscription are offered (Noesis's default). */
  readonly allowApiBilled?: boolean | undefined;
  /** Models whose provider is not signed in are listed, disabled, so the person sees what a login would add. */
  readonly showUnconfigured?: boolean | undefined;
}

/** Picker options for `models`, grouped by provider, ready ones first within a provider. */
export function modelOptions(models: readonly aether.ModelInfo[], providers: readonly aether.ProviderInfo[], settings: ModelOptionsSettings = {}): PickerOption[] {
  invariant(models.every((model) => typeof model.configured === "boolean"), "model readiness is explicit");
  const allowApi = settings.allowApiBilled ?? true;
  const options: PickerOption[] = [];
  const seen = new Set<string>();
  for (const model of [...models].sort((a, b) => Number(b.configured) - Number(a.configured))) {
    const id = modelRefText(model.model);
    if (seen.has(id)) continue;
    seen.add(id);
    const billing = billingOf(model, providers);
    if (!model.configured && settings.showUnconfigured !== true) continue;
    if (!allowApi && billing !== "subscription") continue;
    options.push(modelOption(model, id, billing));
  }
  return options;
}

function modelOption(model: aether.ModelInfo, id: string, billing: Billing): PickerOption {
  const tier = model.tier ? model.tier.replace(/_/g, " ") : null;
  const context = model.capabilities.context_window ? `${Math.round(model.capabilities.context_window / 1000)}k context` : null;
  const description = [tier, context].filter((part): part is string => part !== null).join(" · ");
  const badge = !model.configured ? "Not signed in" : model.is_default ? "Default" : billing === "subscription" ? "Subscription" : billing === "api" ? "API-billed" : undefined;
  const badgeTone = !model.configured ? "warning" : model.is_default ? "accent" : billing === "api" ? "neutral" : undefined;
  return {
    id, label: model.name, group: model.provider_name, disabled: !model.configured, keywords: [id, ...(model.aliases ?? [])].join(" "),
    ...(description === "" ? {} : { description }), ...(badge === undefined ? {} : { badge }), ...(badgeTone === undefined ? {} : { badgeTone }),
  };
}

export interface ModelPickerProps extends ModelOptionsSettings {
  readonly label?: string | undefined;
  readonly models: readonly aether.ModelInfo[];
  /** The `providers` reply, which says how each provider is billed; without it no billing is shown. */
  readonly providers?: readonly aether.ProviderInfo[] | undefined;
  /** The `provider/model` text, or "" for the default option when one is offered. */
  readonly value: string;
  readonly onChange: (ref: string) => void;
  /** The first option, with id "": "Default (first ready subscription model)"; null offers none. */
  readonly defaultLabel?: string | null | undefined;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  readonly disabled?: boolean | undefined;
  readonly className?: string | undefined;
  readonly "data-testid"?: string | undefined;
}

export function ModelPicker({ label = "Model", models, providers = [], value, onChange, defaultLabel = null, hint, error, disabled, className, ...settings }: ModelPickerProps) {
  const options = modelOptions(models, providers, settings);
  const all = defaultLabel === null ? options : [{ id: "", label: defaultLabel, badge: "Default", badgeTone: "accent" as const }, ...options];
  const ready = options.filter((option) => !option.disabled).length;
  const empty = ready === 0 ? "No model is ready: sign in to a provider under Settings → Providers." : undefined;
  return (
    <Picker label={label} value={value} onChange={onChange} options={all} placeholder={ready === 0 ? "No model ready" : "Choose a model"} searchable={all.length >= 8}
      hint={hint ?? empty} error={error} disabled={disabled || all.length === 0} className={className} data-testid={settings["data-testid"]} />
  );
}
