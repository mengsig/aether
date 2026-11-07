// The chat composer docked under a transcript: a growing textarea that sends on Enter, a Stop for
// the running turn, and a `/` menu of commands. The draft outlives the screen until it is delivered.

import { usePermission } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useId, useState, type KeyboardEvent } from "react";
import { DraftError, sendCommand } from "../controls/commands.ts";
import { RunNote, useRun } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { DRAFT_LIMIT, draftKey, useDraft, type Draft } from "./drafts.ts";
import { EndedNotice, endedReason } from "./ended.tsx";
import { optionId, pick, completion, SlashMenu, useMenu, type MenuOption, type MenuState } from "./menu.tsx";
import { heldUnknown, parseLine, type SlashCommand } from "./slash.ts";
import "./composer.css";

export interface ComposerProps {
  readonly conn: ConnId;
  readonly agent: aether.Agent;
  readonly commands: readonly SlashCommand[];
  /** Why a `/name` no command knows is held here (the view-mode catalog is missing), or null to send it. */
  readonly unknownHeld: string | null;
}

/** Sends a line: a known command with `run` runs here, anything else goes to the agent as typed. */
function useSubmit(conn: ConnId, agent: aether.Agent, commands: readonly SlashCommand[], unknownHeld: string | null, draft: Draft) {
  const run = useRun(conn);
  const [problem, setProblem] = useState<string | null>(null);
  /** `typed` is the draft the line came from: a menu pick completes it, so the two can differ. */
  const submit = async (line: string, typed = line) => {
    invariant(agent.id !== "", "the composer sends to a named agent");
    const local = parseLine(line, commands);
    try {
      if (local?.command.run) {
        local.command.run(local.args);
        setProblem(null);
        draft.delivered(typed);
        return;
      }
      const held = heldUnknown(line, commands, unknownHeld);
      if (held !== null) throw new DraftError(held);
      const command = sendCommand(agent.id, line, false, line.trimStart().startsWith("/"));
      setProblem(null);
      if (await run.send(command, "Sent")) draft.delivered(typed);
    } catch (error) {
      if (!(error instanceof DraftError)) throw error;
      setProblem(error.message);
    }
  };
  const stop = () => run.send(aether.interrupt({ agent: agent.id }), "Stopped");
  return { run, problem, submit, stop };
}

/** The menu's keys while it is open; returns whether the key was the menu's. */
function menuKey(event: KeyboardEvent, menu: MenuState, choose: (option: MenuOption) => void, complete: (option: MenuOption) => void): boolean {
  const option = menu.options[menu.active];
  invariant(option !== undefined, "an open menu has a highlighted option");
  switch (event.key) {
    case "ArrowDown": menu.move(1); return true;
    case "ArrowUp": menu.move(-1); return true;
    case "Tab": if (event.shiftKey) return false; complete(option); return true;
    case "Escape": menu.dismiss(); return true;
    case "Enter": if (event.shiftKey) return false; choose(option); return true;
    default: return false;
  }
}

export function Composer({ conn, agent, commands, unknownHeld }: ComposerProps) {
  invariant(conn !== "" && agent.id !== "", "a composer belongs to a connection and an agent");
  const draft = useDraft(draftKey(conn, agent.id, "message"));
  const { run, problem, submit, stop } = useSubmit(conn, agent, commands, unknownHeld, draft);
  const menu = useMenu(draft.text, commands);
  const listbox = useId();
  const reasonId = useId();
  const refusal = usePermission(conn, { cmd: "send_message", to: agent.id }).refusal;
  const stopRefusal = usePermission(conn, aether.interrupt({ agent: agent.id })).refusal;
  const ended = endedReason(agent);
  const busy = run.status.state === "busy";
  /** Why `line` cannot go now, or null; an app command runs even where the agent takes nothing. */
  const blockedFor = (line: string): string | null => {
    if (parseLine(line, commands)?.command.run != null) return null;
    return ended === null ? refusal : "This agent takes no more messages";
  };
  const blocked = blockedFor(draft.text);
  const canSend = draft.text.trim() !== "" && blocked === null && !busy;
  /** Every send, typed or picked from the menu, passes the same gate as the Send button. */
  const send = (line: string) => {
    invariant(line.trim() !== "", "only a non-empty line is sent");
    if (busy || blockedFor(line) !== null) return;
    void submit(line, draft.text);
  };
  const choose = (option: MenuOption) => {
    const chosen = pick(option);
    if (chosen.kind === "complete") draft.set(chosen.text);
    else send(chosen.line);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return;
    const handled = menu.open ? menuKey(event, menu, choose, (option) => draft.set(completion(option))) : event.key === "Enter" && !event.shiftKey;
    if (!handled) return;
    event.preventDefault();
    if (!menu.open && canSend) send(draft.text);
  };
  return (
    <form className="cmp" aria-label={`Message ${agent.name}`} onSubmit={(event) => { event.preventDefault(); if (canSend) send(draft.text); }}>
      {ended === null ? null : <EndedNotice conn={conn} agent={agent} />}
      <div className="cmp-box" data-busy={busy || undefined}>
        {menu.open ? <SlashMenu id={listbox} menu={menu} onPick={choose} /> : null}
        <textarea className="cmp-input" rows={1} maxLength={DRAFT_LIMIT} value={draft.text} placeholder={`Message ${agent.name}, or / for commands`}
          aria-label={`Message ${agent.name}`} role="combobox" aria-autocomplete="list" aria-haspopup="listbox"
          aria-expanded={menu.open} aria-controls={menu.open ? listbox : undefined}
          aria-activedescendant={menu.open ? optionId(listbox, menu.active) : undefined}
          aria-describedby={blocked === null ? undefined : reasonId}
          onChange={(event) => draft.set(event.target.value)} onKeyDown={onKeyDown} />
        <div className="cmp-bar">
          <span className="cmp-hint" id={reasonId} data-blocked={blocked === null ? undefined : true}>{blocked ?? "Enter sends · Shift+Enter adds a line · / for commands"}</span>
          {agent.status.state === "running" ? (
            <Button size="sm" variant="secondary" disabled={stopRefusal !== null} title={stopRefusal ?? "Stop the current turn; the agent keeps its context"} onClick={() => void stop()}>Stop</Button>
          ) : null}
          <Button size="sm" variant="primary" type="submit" disabled={!canSend}>Send</Button>
        </div>
      </div>
      {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
      {draft.problem ? <p className="b3-note" data-state="failed">{draft.problem}</p> : null}
      <RunNote status={run.status} />
    </form>
  );
}
