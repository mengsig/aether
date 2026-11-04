// A mastery state as a chip: its `--mastery-*` colour, a shape that differs per state and its text
// label, so colour is never the only signal.

import { MASTERY, type Mastery } from "./model.ts";

export function MasteryBadge({ state }: { state: Mastery }) {
  return (
    <span className="learn-mastery" data-mastery={state} title={MASTERY[state].hint}>
      <span className="learn-mastery-mark" aria-hidden="true" />
      <span>{MASTERY[state].label}</span>
    </span>
  );
}
