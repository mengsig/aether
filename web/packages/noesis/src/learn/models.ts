// The models Noesis may teach with, read from the Aether connection: `list_models` for what exists and
// whether it is ready, `auth_status` for how each provider is paid for. Checked at the boundary.

import { useReread } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { invariant } from "./model.ts";

export interface ModelChoices {
  readonly models: readonly aether.ModelInfo[];
  readonly auth: readonly aether.AuthInfo[];
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

/** The `auth_status` reply's data: one entry per provider. */
export function authList(value: unknown): aether.AuthInfo[] {
  if (value === null || value === undefined) return [];
  invariant(Array.isArray(value), "auth_status answers with a list");
  const auth = value as aether.AuthInfo[];
  invariant(auth.every((a) => typeof a.provider === "string" && a.provider.length > 0), "auth entries name their provider");
  return auth;
}

/** Reads the model choices while `active`; nothing is sent otherwise (a closed sheet, or Noesis staying off). */
export function useModelChoices(conn: string, active: boolean): ModelChoices {
  const models = useReread(conn, "learn:models", active ? { cmd: "list_models" } : null, ["models"]);
  const auth = useReread(conn, "learn:auth", active ? { cmd: "auth_status" } : null, ["auth"]);
  return {
    models: modelList(models?.value), auth: authList(auth?.value),
    loaded: models !== undefined && models.settled > 0, error: models?.error ?? null,
  };
}

/** Whether any listed model is ready, so the sheet can name the next step when none is. */
export function hasReadyModel(models: readonly aether.ModelInfo[]): boolean {
  return models.some((model) => model.configured);
}
