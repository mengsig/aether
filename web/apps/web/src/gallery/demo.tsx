import { Badge, Button, EmptyState, EntityActionMenu, Field, Menu, StatusPill, STATUSES, Tabs, Tooltip, VirtualList } from "@aether-ui/ui";
import { sampleAgent } from "./fixtures.tsx";

const AGENTS = 10_000;

function AgentRow({ index }: { readonly index: number }) {
  const agent = sampleAgent(index);
  return (
    <div className="gallery-row">
      <code className="gallery-agent">{agent.name}</code>
      <StatusPill status={agent.status} />
      <Badge>{agent.turns} turns</Badge>
    </div>
  );
}

export function DemoScreen() {
  return (
    <div className="gallery">
      <section className="gallery-panel gallery-wide" aria-labelledby="g-agents">
        <header className="gallery-panel-head">
          <h2 id="g-agents">Agents</h2>
          <Badge tone="accent">{AGENTS.toLocaleString("en")} live</Badge>
          <EntityActionMenu entity={{ kind: "agent", conn: "local", id: "agt_demo" }} />
        </header>
        <VirtualList label="Agents" count={AGENTS} rowHeight={44} height={308} renderRow={(index) => <AgentRow index={index} />} />
      </section>
      <section className="gallery-panel" aria-labelledby="g-states">
        <h2 id="g-states">States</h2>
        <div className="gallery-pills">{STATUSES.map((status) => <StatusPill key={status} status={status} />)}</div>
      </section>
      <section className="gallery-panel" aria-labelledby="g-controls">
        <h2 id="g-controls">Controls</h2>
        <Tabs label="Run views" tabs={[
          { id: "plan", label: "Plan", panel: <Field label="Objective" placeholder="What should this run achieve?" hint="Agents see this first." /> },
          { id: "limits", label: "Limits", panel: <Field label="Budget (USD)" defaultValue="abc" error="Enter a number" /> },
        ]} />
        <div className="gallery-buttons">
          <Button variant="primary">Start run</Button>
          <Button>Pause</Button>
          <Tooltip content="Stops every agent in this run"><Button variant="danger">Cancel run</Button></Tooltip>
          <Menu label="More" items={[{ id: "copy", label: "Copy run id", onSelect: () => undefined }, { id: "drop", label: "Delete", tone: "danger", onSelect: () => undefined }]} />
        </div>
      </section>
      <section className="gallery-panel gallery-wide" aria-labelledby="g-empty">
        <h2 id="g-empty" className="gallery-hidden">Empty inbox</h2>
        <EmptyState title="Inbox zero" action={<Button>Refresh</Button>}>Nothing waits on you. New questions from agents land here.</EmptyState>
      </section>
    </div>
  );
}

export function FailingScreen(): never {
  throw new Error("archond at studio.local/archon did not answer within 10 s");
}
