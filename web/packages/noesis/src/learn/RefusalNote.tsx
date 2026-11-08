// A refusal shown calmly: a missing or API-only model, or a spent day's budget, points to the
// settings that fix it, with a way to try again once they do.

import { Button } from "@aether-ui/ui";
import { settingsHref } from "./LearnFrame.tsx";
import { invariant, usd, type BudgetUse, type Refusal } from "./model.ts";

function BudgetNote({ budget, onRetry }: { budget: BudgetUse | null; onRetry: (() => void) | undefined }) {
  invariant(budget === null || (budget.spent >= 0 && budget.limit >= 0), "budget use is a nonnegative amount");
  return (
    <div className="learn-callout" role="status" data-kind="budget">
      <p className="learn-callout-title">Today's teaching budget is spent</p>
      <p>{budget === null ? "Noesis stops teaching once the day's budget is used up." : `You have used ${usd(budget.spent)} of your ${usd(budget.limit)} daily teaching budget.`}{" "}
        Raise the budget, or come back tomorrow. Your lesson waits where it is.</p>
      <div className="learn-callout-actions">
        <a className="ui-button" data-variant="secondary" data-size="sm" href={settingsHref()}>Open settings</a>
        {onRetry !== undefined && <Button size="sm" onClick={onRetry}>Try again</Button>}
      </div>
    </div>
  );
}

export function RefusalNote({ refusal, budget = null, onRetry }: { refusal: Refusal; budget?: BudgetUse | null; onRetry?: (() => void) | undefined }) {
  invariant(refusal.message.trim() !== "", "a refusal says why");
  if (refusal.kind === "budget") return <BudgetNote budget={budget} onRetry={onRetry} />;
  if (refusal.kind === "other") return <p className="learn-callout" role="alert">{refusal.message}</p>;
  return (
    <div className="learn-callout" role="status" data-kind="model">
      <p className="learn-callout-title">Noesis needs a model it may use</p>
      <p>{refusal.message}</p>
      <p>Choose a subscription model, or allow API-billed models, in <a className="ui-link" href={settingsHref()}>Noesis settings</a>.</p>
      <p>No model signed in yet? <a className="ui-link" href="#/settings/providers">Sign in to a provider</a> first.</p>
    </div>
  );
}
