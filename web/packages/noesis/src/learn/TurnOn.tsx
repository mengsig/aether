// The one guided state every learning screen shows while Noesis is off: what it does, what it spends,
// and one primary action. On the daemon's host that action opens the settings with the switch already on.

import type { aether } from "@aether-ui/protocol";
import { Button, EmptyState } from "@aether-ui/ui";
import { useHostAdmin } from "./data.ts";
import { invariant } from "./model.ts";
import { useSettingsDraft } from "./SettingsSheet.tsx";

export function TurnOnNoesis({ conn, status, onSettings }: { conn: string; status: aether.NoesisStatus; onSettings: () => void }) {
  invariant(!status.enabled, "the turn-on state shows only while Noesis is off");
  const host = useHostAdmin(conn);
  const draft = useSettingsDraft(status.settings);
  const turnOn = () => {
    draft.update({ enabled: true });
    onSettings();
  };
  const action = host
    ? <><Button variant="primary" data-testid="noesis-turn-on-action" onClick={turnOn}>Turn on Noesis…</Button><Button variant="quiet" onClick={onSettings}>Open Noesis settings</Button></>
    : <><Button variant="primary" onClick={onSettings}>Open Noesis settings</Button><span className="learn-muted">Only the daemon's own host can turn it on.</span></>;
  return (
    <div className="learn-off" data-testid="noesis-off">
      <p className="ui-eyebrow learn-off-status">Noesis is off</p>
      <div className="learn-turn-on" data-testid="noesis-turn-on">
        <EmptyState title="Turn on Noesis" action={<div className="learn-actions">{action}</div>}>
          <p>Noesis teaches through questions: it finds where you stand, plans a path to your goal and keeps track of what you understand.</p>
          <p>It uses your subscription models, within daily budgets you set. While it is off, nothing is read or sent.</p>
        </EmptyState>
      </div>
    </div>
  );
}
