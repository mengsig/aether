import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { records } from "../memory/shared.tsx";

export function configuredModels(value: unknown): aether.ModelInfo[] {
  const models = records<aether.ModelInfo>(value);
  invariant(models.every(m => typeof m.configured === "boolean"), "model availability is explicit");
  invariant(models.every(m => m.model && m.model.provider.length > 0 && m.model.model.length > 0), "models have provider-qualified identities");
  return models.filter(m => m.configured);
}

export function workspaceValidation(value: unknown): aether.WorkspaceValidation | undefined {
  if (value === null || value === undefined) return undefined;
  invariant(typeof value === "object" && "valid" in value && typeof value.valid === "boolean", "workspace validation has a verdict");
  const validation = value as aether.WorkspaceValidation;
  invariant(!validation.valid || (typeof validation.path === "string" && validation.path.startsWith("/")), "valid workspace has an absolute canonical path");
  return validation;
}

export function orderedEvents(events: readonly aether.Event[]): aether.Event[] {
  invariant(events.every(e => typeof e.id === "string" && e.id.length > 0), "events have durable identities");
  invariant(events.every(e => Number.isSafeInteger(e.seq) && e.seq > 0), "events have positive durable sequences");
  return [...new Map(events.map(e => [e.id, e])).values()].sort((a, b) => b.seq - a.seq);
}
