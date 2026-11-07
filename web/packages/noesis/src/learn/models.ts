// The models Noesis may teach with, read from the Aether connection: `list_models` for what exists and
// whether it is ready, `providers` for how each provider is paid for (an engine spends a subscription,
// as Noesis's own budget decides it). Checked at the boundary.

import { useReread } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { invariant } from "./model.ts";

export interface ModelChoices {
  readonly models: readonly aether.ModelInfo[];
  readonly providers: readonly aether.ProviderInfo[];
  /** False until the first `list_models` reply lands, so an empty list is not mistaken for "none ready". */
  readonly loaded: boolean;
  readonly error: string | null;
}

/** The `list_models` reply's data: every entry names its provider and model and says whether it is ready. */
export function modelList(value: unknown): aether.ModelInfo[] {
  if (value === null || value === undefined) return [];
  invariant(Array.isArray(value), "list_models answers with a list");
  const models = value as aether.ModelInfo[];
  invariant(models.every((m) => typeof m.configured === "boolean"), "model readiness is explicit");
  invariant(models.every((m) => m.model?.provider.length > 0 && m.model.model.length > 0), "models have provider-qualified identities");
  return models;
}

/** The `providers` reply's data: one entry per provider, each saying whether it is an engine. */
export function providerList(value: unknown): aether.ProviderInfo[] {
  if (value === null || value === undefined) return [];
  invariant(Array.isArray(value), "providers answers with a list");
  const providers = value as aether.ProviderInfo[];
  invariant(providers.every((p) => typeof p.provider === "string" && p.provider.length > 0), "provider entries name their provider");
  invariant(providers.every((p) => typeof p.engine === "boolean"), "provider entries say whether they are engines");
  return providers;
}

/** Reads the model choices while `active`; nothing is sent otherwise (a closed sheet, or Noesis staying off). */
export function useModelChoices(conn: string, active: boolean): ModelChoices {
  const models = useReread(conn, "learn:models", active ? { cmd: "list_models" } : null, ["models"]);
  const providers = useReread(conn, "learn:providers", active ? { cmd: "providers" } : null, ["providers"]);
  return {
    models: modelList(models?.value), providers: providerList(providers?.value),
    loaded: models !== undefined && models.settled > 0, error: models?.error ?? null,
  };
}

/** Whether any listed model is ready, so the sheet can name the next step when none is. */
export function hasReadyModel(models: readonly aether.ModelInfo[]): boolean {
  return models.some((model) => model.configured);
}
