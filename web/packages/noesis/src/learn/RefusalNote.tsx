// A refusal shown calmly: a missing or API-only model points to the settings that fix it.

import { settingsHref } from "./LearnFrame.tsx";
import type { Refusal } from "./model.ts";

export function RefusalNote({ refusal }: { refusal: Refusal }) {
  if (refusal.kind === "other") return <p className="learn-callout" role="alert">{refusal.message}</p>;
  return (
    <div className="learn-callout" role="status" data-kind="model">
      <p className="learn-callout-title">Noesis needs a model it may use</p>
      <p>{refusal.message}</p>
      <p>Choose a subscription model, or allow API-billed models, in <a className="ui-link" href={settingsHref()}>Noesis settings</a>.</p>
    </div>
  );
}
