// A Noesis model picked from what the daemon offers, grouped by how it is paid for. A model that is
// not ready, or API-billed while API models are off, shows but cannot be chosen.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "./model.ts";

const AUTOMATIC = "";

/** Why `id` cannot be saved as a Noesis model, or undefined when it can; "" picks automatically. */
export function modelError(id: string, offers: readonly aether.NoesisModelOffer[] | undefined, allowApi: boolean): string | undefined {
  if (id === AUTOMATIC) return undefined;
  const offer = offers?.find((entry) => entry.id === id);
  if (offer === undefined) return "This daemon does not offer that model. Pick one from the list.";
  if (!offer.ready) return "That model is not ready on this daemon. Pick a ready one.";
  if (offer.billing === "api" && !allowApi) return "That model is billed per request. Allow API-billed models first.";
  return undefined;
}

function optionLabel(offer: aether.NoesisModelOffer, allowApi: boolean): string {
  if (!offer.ready) return `${offer.id} — not ready`;
  if (offer.billing === "api" && !allowApi) return `${offer.id} — needs API-billed models allowed`;
  return offer.id;
}

export interface ModelChooserProps {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly offers: readonly aether.NoesisModelOffer[] | undefined;
  readonly allowApi: boolean;
  readonly disabled: boolean;
  /** What the daemon resolved the choice to, or why it could not. */
  readonly resolved?: aether.NoesisModelChoice | null | undefined;
  readonly onChange: (value: string) => void;
}

export function ModelChooser({ id, label, value, offers, allowApi, disabled, resolved, onChange }: ModelChooserProps) {
  const known = offers?.some((offer) => offer.id === value) ?? false;
  const error = modelError(value, offers, allowApi);
  invariant(id.trim() !== "" && label.trim() !== "", "a model chooser is labelled");
  invariant(new Set(offers?.map((offer) => offer.id)).size === (offers?.length ?? 0), "the daemon offers each model once");
  const groups = [
    { label: "Subscription — covered by your plan", offers: offers?.filter((offer) => offer.billing === "subscription") ?? [] },
    { label: "API — billed per request", offers: offers?.filter((offer) => offer.billing === "api") ?? [] },
  ].filter((group) => group.offers.length > 0);
  return (
    <div className="learn-model">
      <label className="learn-model-label" htmlFor={id}>{label}</label>
      <select id={id} className="learn-select" value={value} disabled={disabled} aria-invalid={error !== undefined || undefined}
        aria-describedby={`${id}-note`} onChange={(event) => onChange(event.target.value)}>
        <option value={AUTOMATIC}>Automatic — the first ready subscription model</option>
        {value !== AUTOMATIC && !known && <option value={value} disabled>{value} — unknown here</option>}
        {groups.map((group) => (
          <optgroup key={group.label} label={group.label}>
            {group.offers.map((offer) => <option key={offer.id} value={offer.id} disabled={!offer.ready || (offer.billing === "api" && !allowApi)}>{optionLabel(offer, allowApi)}</option>)}
          </optgroup>
        ))}
      </select>
      <p id={`${id}-note`} className="learn-muted" data-error={error !== undefined || undefined}>{noteOf(error, resolved, offers)}</p>
    </div>
  );
}

function noteOf(error: string | undefined, resolved: aether.NoesisModelChoice | null | undefined, offers: readonly aether.NoesisModelOffer[] | undefined): string {
  if (error !== undefined) return error;
  if (resolved?.refusal) return `Teaching cannot run now: ${resolved.refusal}`;
  if (resolved?.model) return `Teaching runs on ${resolved.model}.`;
  return offers === undefined ? "This daemon does not list its models." : "Noesis uses this model when it is ready.";
}
