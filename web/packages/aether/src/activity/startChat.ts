// Starting a chat, as one explicit state model: the draft is validated, the agent is created, then
// its first message is sent; once the agent exists its settings are fixed, and every retry sends the
// same text to that same agent. Only an explicit restart begins a new chat with other settings.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { FIRST_MESSAGE_LIMIT, type FirstSend } from "./firstMessage.ts";

/** What a chat is created with; fixed once its create command is sent. */
export interface ChatSettings {
  readonly name: string;
  readonly workspace: string;
  readonly model: aether.ModelRef;
  readonly modelName: string;
  readonly effort: aether.Effort;
  readonly network?: aether.NetworkPolicy;
}

interface Fixed {
  readonly settings: ChatSettings;
  /** The first message as it is sent: trimmed once, never edited or parsed afterwards. */
  readonly text: string;
}

export type StartChat =
  | { readonly phase: "editing" }
  | (Fixed & { readonly phase: "creating" })
  /** The create's reply never came, so the chat may exist: only an explicit restart creates another. */
  | (Fixed & { readonly phase: "create-unconfirmed"; readonly reason: string })
  | (Fixed & { readonly phase: "sending"; readonly agent: string })
  | (Fixed & { readonly phase: "refused" | "unconfirmed"; readonly agent: string; readonly reason: string; readonly keepProblem: string | null })
  | (Fixed & { readonly phase: "confirmed"; readonly agent: string });

export type StartEvent =
  | { readonly type: "create"; readonly settings: ChatSettings; readonly text: string }
  | { readonly type: "create-failed" }
  | { readonly type: "create-lost"; readonly reason: string }
  | { readonly type: "created"; readonly agent: string }
  | { readonly type: "retry" }
  | { readonly type: "sent"; readonly result: FirstSend }
  | { readonly type: "restart" };

export const EDITING: StartChat = { phase: "editing" };

/** Why `text` cannot start a chat, or null; checked before anything is created. */
export function firstMessageProblem(text: string): string | null {
  const trimmed = text.trim();
  if (trimmed === "") return "Write the first message";
  if (trimmed.length > FIRST_MESSAGE_LIMIT) {
    return `The first message is too long: ${trimmed.length.toLocaleString("en")} characters, at most ${FIRST_MESSAGE_LIMIT.toLocaleString("en")}`;
  }
  return null;
}

function requirePhase(state: StartChat, phases: readonly StartChat["phase"][], event: StartEvent): void {
  invariant(phases.includes(state.phase), `a start in "${state.phase}" cannot take "${event.type}"`);
}

/** The next state; a transition the model does not allow is a programming error, never ignored. */
export function startChatStep(state: StartChat, event: StartEvent): StartChat {
  switch (event.type) {
    case "create":
      requirePhase(state, ["editing"], event);
      invariant(firstMessageProblem(event.text) === null && event.text === event.text.trim(), "a chat is created from a validated first message");
      invariant(event.settings.workspace.startsWith("/"), "a chat is created in a validated absolute workspace");
      return { phase: "creating", settings: event.settings, text: event.text };
    case "create-failed":
      requirePhase(state, ["creating"], event);
      return EDITING;
    case "create-lost":
      invariant(state.phase === "creating", "only a pending create can lose its reply");
      return { ...fixed(state), phase: "create-unconfirmed", reason: event.reason };
    case "created":
      invariant(state.phase === "creating", "only a pending create makes an agent");
      invariant(event.agent.startsWith("agt_"), "create_agent answers with an agent id");
      return { ...fixed(state), phase: "sending", agent: event.agent };
    case "retry":
      invariant(state.phase === "refused" || state.phase === "unconfirmed", "only an undelivered first message is sent again");
      return { ...fixed(state), phase: "sending", agent: state.agent };
    case "sent":
      return delivered(state, event.result);
    case "restart":
      requirePhase(state, ["create-unconfirmed", "refused", "unconfirmed"], event);
      return EDITING;
  }
}

function delivered(state: StartChat, result: FirstSend): StartChat {
  invariant(state.phase === "sending", "only a first message being sent has an outcome");
  if (result.outcome === "sent") return { ...fixed(state), phase: "confirmed", agent: state.agent };
  invariant(result.text === state.text, "the outcome is for the text this start sent");
  return { ...fixed(state), phase: result.outcome, agent: state.agent, reason: result.reason, keepProblem: result.keepProblem };
}

function fixed(state: Fixed): Fixed {
  return { settings: state.settings, text: state.text };
}
