// One agent's chat: the view mode bar, what it is working on or waiting for, its open questions and
// approvals pinned until answered, the transcript as the mode draws it, a first message the start
// could not deliver, and the composer.

import { selectAgents, selectAttention, selectStreams, useSelector } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { useCallback, useMemo, type KeyboardEvent } from "react";
import { UndeliveredFirst } from "../activity/UndeliveredFirst.tsx";
import { Composer } from "../composer/Composer.tsx";
import { clientCommands, hostCommands, mergeCommands, modeCommands } from "../composer/slash.ts";
import { invariant } from "../fleet/invariant.ts";
import { Approval, Question } from "../inbox/attention.tsx";
import { statusDetail } from "../agent/facts.ts";
import { Conversation } from "./Conversation.tsx";
import { ModeBar } from "./ModeBar.tsx";
import { modeLabel } from "./modes.ts";
import { useViewMode, type ViewModeState } from "./useViewMode.ts";

/** "Working on …" from the live tail's newest call, or what the agent waits for; null when idle. */
function useActivity(conn: ConnId, agent: aether.Agent): { readonly text: string; readonly state: string } | null {
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  const status = agent.status;
  if (status.state === "running") {
    const live = Object.values(streams).filter((s) => s.agent === agent.id && !s.committed);
    const tool = live.flatMap((s) => s.tools).at(-1)?.name;
    const writing = live.some((s) => s.text !== "");
    return { state: "running", text: tool !== undefined ? `Working on ${tool}` : writing ? "Writing a reply" : "Thinking" };
  }
  const detail = statusDetail(status);
  return detail === null ? null : { state: status.state, text: detail };
}

/** Its own component: it follows every live stream, so only it redraws on each delta. */
function ActivityLine({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const activity = useActivity(conn, agent);
  if (activity === null) return null;
  return <p className="conv-activity" role="status" data-state={activity.state}>{activity.text}</p>;
}

function Pinned({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const attention = useSelector(useMemo(() => selectAttention(conn), [conn]));
  const agents = useSelector(useMemo(() => selectAgents(conn), [conn]));
  const questions = attention?.questions.filter((q) => q.agent === agent.id) ?? [];
  const approvals = attention?.approvals.filter((a) => a.agent_id === agent.id && (a.status as { state: string }).state === "pending") ?? [];
  if (questions.length + approvals.length === 0) return null;
  return (
    <section className="conv-pinned" aria-label="Waiting for you">
      <ul className="b3-list">
        {questions.map((q) => <Question key={`q:${q.question_seq}`} conn={conn} question={q} />)}
        {approvals.map((a) => <Approval key={a.id} conn={conn} request={a} name={agents[a.agent_id]?.name ?? agent.name} />)}
      </ul>
    </section>
  );
}

function useChatCommands(conn: ConnId, agent: aether.Agent, view: ViewModeState) {
  const modes = useMemo(() => view.modes.map((m) => ({ name: m.name, label: modeLabel(m), description: m.description ?? "" })), [view.modes]);
  const unavailable = view.problem === null ? "View modes are still loading" : `View modes unavailable: ${view.problem}`;
  const commands = useMemo(
    () => mergeCommands(modeCommands(modes, view.choose, view.toggle, unavailable), hostCommands(agent), clientCommands(conn, agent)),
    [modes, view.choose, view.toggle, unavailable, agent, conn],
  );
  return { commands, unknownHeld: modes.length === 0 ? unavailable : null };
}

export function Chat({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(conn !== "" && agent.id !== "", "a chat belongs to a connection and an agent");
  const view = useViewMode(conn, agent);
  const { commands, unknownHeld } = useChatCommands(conn, agent, view);
  const { details, setDetails } = view;
  const onKeyDown = useCallback((event: KeyboardEvent<HTMLElement>) => {
    if (!event.ctrlKey || event.altKey || event.metaKey || event.key.toLowerCase() !== "t") return;
    event.preventDefault();
    setDetails(!details);
  }, [details, setDetails]);
  const layout = `${view.mode}${view.details ? "+details" : ""}`;
  return (
    <section className="conv-chat" aria-label={`Chat with ${agent.name}`} data-mode={view.mode} data-details={view.details || undefined} onKeyDown={onKeyDown}>
      <div className="conv-chat-head">
        <ModeBar view={view} />
        <ActivityLine conn={conn} agent={agent} />
      </div>
      <Pinned conn={conn} agent={agent} />
      <Conversation conn={conn} agent={agent.id} rules={view.rules} layout={layout} />
      <UndeliveredFirst conn={conn} agent={agent.id} />
      <Composer conn={conn} agent={agent} commands={commands} unknownHeld={unknownHeld} />
    </section>
  );
}
