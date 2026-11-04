// `#/inbox`: everything that waits for a person across the connected daemons — questions, approvals,
// stopped agents — and the recovery review of changes whose reply never came.

import { selectConnections, useSelector } from "@aether-ui/core";
import { EmptyState } from "@aether-ui/ui";
import { useMemo } from "react";
import { StandingNotice } from "../controls/kit.tsx";
import { DaemonAttention } from "./attention.tsx";
import { RecoveryReview } from "./recovery.tsx";
import "../controls/kit.css";

export function InboxScreen() {
  const connections = useSelector(selectConnections);
  const daemons = useMemo(() => Object.values(connections).filter((c) => c.daemon === "aether"), [connections]);
  return (
    <section className="b3-screen" aria-label="Inbox">
      <header className="b3-screen-head">
        <h2 className="b3-screen-title">Inbox</h2>
        <p className="b3-screen-sub">Questions, approvals and agents that need you, newest daemon first.</p>
      </header>
      <RecoveryReview />
      {daemons.length === 0 ? <EmptyState title="No daemon connected">Pair a daemon to see what its agents need. <a className="ui-link" href="#/settings/connections">Connections</a></EmptyState> : null}
      {daemons.map((daemon) => (
        <div key={daemon.id} className="b3-stack">
          {daemons.length > 1 ? <h3 className="b3-section-title">{daemon.label}</h3> : null}
          <StandingNotice conn={daemon.id} />
          <DaemonAttention conn={daemon.id} label={daemon.label} />
        </div>
      ))}
    </section>
  );
}

export default function InboxRoute() {
  return <InboxScreen />;
}
